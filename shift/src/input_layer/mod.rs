pub mod channels;

use std::{
	fs::{File, OpenOptions},
	io,
	os::{
		fd::{AsRawFd, OwnedFd},
		unix::fs::OpenOptionsExt,
	},
	path::Path,
	sync::Arc,
};

use input::{
	DeviceConfigError, Libinput, LibinputInterface, TapButtonMap,
	event::{
		Event, EventTrait, GestureEvent, KeyboardEvent, PointerEvent, SwitchEvent, TouchEvent,
		device::DeviceEvent,
		gesture::{
			GestureEndEvent, GestureEventCoordinates, GestureEventTrait, GestureHoldEvent,
			GesturePinchEvent, GesturePinchEventTrait, GestureSwipeEvent,
		},
		keyboard::{self, KeyboardEventTrait},
		pointer::{self, PointerEventTrait},
		switch::{self, SwitchEventTrait},
		tablet_pad::{self, TabletPadEvent, TabletPadEventTrait},
		tablet_tool::{self, ProximityState, TabletToolEvent, TabletToolEventTrait, TipState},
		touch::{TouchEventPosition, TouchEventSlot, TouchEventTrait},
	},
};
use tab_protocol::{
	AxisOrientation, AxisSource, ButtonState, InputEventPayload, KeyState, SwitchState, SwitchType,
	TabletTool, TabletToolAxes, TabletToolCapability, TabletToolType, TipState as ProtoTipState,
	TouchContact,
};
use thiserror::Error;

use crate::comms::input2server::{InputEvt, InputEvtTx};

#[derive(Debug, Error)]
pub enum InputError {
	#[error("failed to assign libinput seat `{seat}`")]
	AssignSeat { seat: String },
	#[error("io error: {0}")]
	Io(#[from] io::Error),
}

pub struct InputLayer {
	event_tx: InputEvtTx,
	seat: String,
	tap_to_click: bool,
	tap_drag: bool,
	tap_drag_lock: bool,
	tap_button_map: TapButtonMap,
}

impl InputLayer {
	pub fn init(channels: channels::InputEnd) -> Self {
		let event_tx = channels.into_parts();
		let seat = std::env::var("SHIFT_INPUT_SEAT").unwrap_or_else(|_| "seat0".to_string());
		let tap_to_click = env_bool("SHIFT_INPUT_TAP_TO_CLICK", true);
		let tap_drag = env_bool("SHIFT_INPUT_TAP_DRAG", true);
		let tap_drag_lock = env_bool("SHIFT_INPUT_TAP_DRAG_LOCK", false);
		let tap_button_map = match std::env::var("SHIFT_INPUT_TAP_BUTTON_MAP")
			.unwrap_or_else(|_| "lrm".to_string())
			.to_ascii_lowercase()
			.as_str()
		{
			"lmr" => TapButtonMap::LeftMiddleRight,
			_ => TapButtonMap::LeftRightMiddle,
		};
		Self {
			event_tx,
			seat,
			tap_to_click,
			tap_drag,
			tap_drag_lock,
			tap_button_map,
		}
	}

	pub async fn run(self) -> Result<(), InputError> {
		let seat = self.seat.clone();
		let tx = self.event_tx;
		let input_config = InputConfig {
			tap_to_click: self.tap_to_click,
			tap_drag: self.tap_drag,
			tap_drag_lock: self.tap_drag_lock,
			tap_button_map: self.tap_button_map,
		};
		tokio::task::spawn_blocking(move || run_blocking(tx, seat, input_config))
			.await
			.map_err(|e| io::Error::other(format!("input task join error: {e}")))?
	}
}

#[derive(Clone, Copy, Debug)]
struct InputConfig {
	tap_to_click: bool,
	tap_drag: bool,
	tap_drag_lock: bool,
	tap_button_map: TapButtonMap,
}

fn env_bool(name: &str, default: bool) -> bool {
	match std::env::var(name) {
		Ok(v) => !matches!(
			v.trim().to_ascii_lowercase().as_str(),
			"0" | "false" | "off" | "no"
		),
		Err(_) => default,
	}
}

fn run_blocking(
	event_tx: InputEvtTx,
	seat: String,
	input_config: InputConfig,
) -> Result<(), InputError> {
	let mut input = Libinput::new_with_udev(Interface);
	input
		.udev_assign_seat(&seat)
		.map_err(|_| InputError::AssignSeat { seat: seat.clone() })?;
	loop {
		let mut pollfd = libc::pollfd {
			fd: input.as_raw_fd(),
			events: libc::POLLIN,
			revents: 0,
		};
		let poll_res = unsafe { libc::poll(&mut pollfd as *mut libc::pollfd, 1, 1000) };
		if poll_res < 0 {
			let err = io::Error::last_os_error();
			if err.kind() == io::ErrorKind::Interrupted {
				continue;
			}
			let _ = event_tx.blocking_send(InputEvt::FatalError {
				reason: Arc::<str>::from(format!("poll failed: {err}")),
			});
			return Err(err.into());
		}
		if poll_res == 0 {
			continue;
		}
		if let Err(e) = input.dispatch() {
			let _ = event_tx.blocking_send(InputEvt::FatalError {
				reason: Arc::<str>::from(format!("dispatch failed: {e}")),
			});
			return Err(e.into());
		}
		for event in &mut input {
			if let Event::Device(DeviceEvent::Added(added)) = &event {
				let mut device = added.device();
				configure_device_tap(&mut device, input_config);
			}
			let Some(payload) = map_event(event) else {
				continue;
			};
			if event_tx.blocking_send(InputEvt::Event(payload)).is_err() {
				return Ok(());
			}
		}
	}
}

fn apply_config_result(result: Result<(), DeviceConfigError>, device_name: &str, setting: &str) {
	match result {
		Ok(()) => tracing::debug!(device = device_name, setting, "applied libinput setting"),
		Err(DeviceConfigError::Unsupported) => {}
		Err(DeviceConfigError::Invalid) => {
			tracing::warn!(
				device = device_name,
				setting,
				"invalid libinput setting value"
			);
		}
	}
}

fn configure_device_tap(device: &mut input::Device, input_config: InputConfig) {
	if device.config_tap_finger_count() == 0 {
		return;
	}
	let device_name = device.name().to_string();
	apply_config_result(
		device.config_tap_set_enabled(input_config.tap_to_click),
		&device_name,
		"tap_to_click",
	);
	apply_config_result(
		device.config_tap_set_drag_enabled(input_config.tap_drag),
		&device_name,
		"tap_drag",
	);
	apply_config_result(
		device.config_tap_set_drag_lock_enabled(input_config.tap_drag_lock),
		&device_name,
		"tap_drag_lock",
	);
	apply_config_result(
		device.config_tap_set_button_map(input_config.tap_button_map),
		&device_name,
		"tap_button_map",
	);
}

struct Interface;

impl LibinputInterface for Interface {
	fn open_restricted(&mut self, path: &Path, flags: i32) -> Result<OwnedFd, i32> {
		OpenOptions::new()
			.custom_flags(flags)
			.read((flags & libc::O_RDONLY != 0) || (flags & libc::O_RDWR != 0))
			.write((flags & libc::O_WRONLY != 0) || (flags & libc::O_RDWR != 0))
			.open(path)
			.map(Into::into)
			.map_err(|err| err.raw_os_error().unwrap_or(libc::EIO))
	}

	fn close_restricted(&mut self, fd: OwnedFd) {
		let _ = File::from(fd);
	}
}

fn map_event(event: Event) -> Option<InputEventPayload> {
	match event {
		Event::Keyboard(KeyboardEvent::Key(key)) => Some(InputEventPayload::Key {
			device: device_id(&key),
			time_usec: key.time_usec(),
			key: key.key(),
			state: map_key_state(key.key_state()),
		}),
		Event::Pointer(pointer) => map_pointer_event(pointer),
		Event::Touch(touch) => map_touch_event(touch),
		Event::Tablet(tablet) => map_tablet_event(tablet),
		Event::TabletPad(tablet_pad) => map_tablet_pad_event(tablet_pad),
		Event::Gesture(gesture) => map_gesture_event(gesture),
		Event::Switch(SwitchEvent::Toggle(toggle)) => {
			let switch = match toggle.switch() {
				Some(switch::Switch::Lid) => SwitchType::Lid,
				Some(switch::Switch::TabletMode) => SwitchType::TabletMode,
				Some(_) | None => return None,
			};
			Some(InputEventPayload::SwitchToggle {
				device: device_id(&toggle),
				time_usec: toggle.time_usec(),
				switch,
				state: match toggle.switch_state() {
					switch::SwitchState::On => SwitchState::On,
					switch::SwitchState::Off => SwitchState::Off,
				},
			})
		}
		Event::Device(_) => None,
		_ => None,
	}
}

fn map_pointer_event(event: PointerEvent) -> Option<InputEventPayload> {
	match event {
		PointerEvent::Motion(motion) => Some(InputEventPayload::PointerMotion {
			device: device_id(&motion),
			time_usec: motion.time_usec(),
			x: 0.0,
			y: 0.0,
			dx: motion.dx(),
			dy: motion.dy(),
			unaccel_dx: motion.dx_unaccelerated(),
			unaccel_dy: motion.dy_unaccelerated(),
		}),
		PointerEvent::MotionAbsolute(motion) => Some(InputEventPayload::PointerMotionAbsolute {
			device: device_id(&motion),
			time_usec: motion.time_usec(),
			x: motion.absolute_x(),
			y: motion.absolute_y(),
			x_transformed: motion.absolute_x_transformed(65535),
			y_transformed: motion.absolute_y_transformed(65535),
		}),
		PointerEvent::Button(button) => Some(InputEventPayload::PointerButton {
			device: device_id(&button),
			time_usec: button.time_usec(),
			button: button.button(),
			state: match button.button_state() {
				pointer::ButtonState::Pressed => ButtonState::Pressed,
				pointer::ButtonState::Released => ButtonState::Released,
			},
		}),
		#[allow(deprecated)]
		PointerEvent::Axis(axis) => {
			let orientation = if axis.has_axis(pointer::Axis::Vertical) {
				AxisOrientation::Vertical
			} else if axis.has_axis(pointer::Axis::Horizontal) {
				AxisOrientation::Horizontal
			} else {
				return None;
			};
			let axis_selector = match orientation {
				AxisOrientation::Vertical => pointer::Axis::Vertical,
				AxisOrientation::Horizontal => pointer::Axis::Horizontal,
			};
			let source = match axis.axis_source() {
				pointer::AxisSource::Wheel => AxisSource::Wheel,
				pointer::AxisSource::Finger => AxisSource::Finger,
				pointer::AxisSource::Continuous => AxisSource::Continuous,
				pointer::AxisSource::WheelTilt => AxisSource::WheelTilt,
			};
			Some(InputEventPayload::PointerAxis {
				device: device_id(&axis),
				time_usec: axis.time_usec(),
				orientation,
				delta: axis.axis_value(axis_selector),
				delta_discrete: axis
					.axis_value_discrete(axis_selector)
					.map(|v| v.round() as i32),
				source,
			})
		}
		_ => None,
	}
}

fn map_touch_event(event: TouchEvent) -> Option<InputEventPayload> {
	match event {
		TouchEvent::Down(down) => Some(InputEventPayload::TouchDown {
			device: device_id(&down),
			time_usec: down.time_usec(),
			contact: TouchContact {
				id: down.slot().map(|slot| slot as i32).unwrap_or(-1),
				x: down.x(),
				y: down.y(),
				x_transformed: down.x_transformed(65535),
				y_transformed: down.y_transformed(65535),
			},
		}),
		TouchEvent::Up(up) => Some(InputEventPayload::TouchUp {
			device: device_id(&up),
			time_usec: up.time_usec(),
			contact_id: up.slot().map(|slot| slot as i32).unwrap_or(-1),
		}),
		TouchEvent::Motion(motion) => Some(InputEventPayload::TouchMotion {
			device: device_id(&motion),
			time_usec: motion.time_usec(),
			contact: TouchContact {
				id: motion.slot().map(|slot| slot as i32).unwrap_or(-1),
				x: motion.x(),
				y: motion.y(),
				x_transformed: motion.x_transformed(65535),
				y_transformed: motion.y_transformed(65535),
			},
		}),
		TouchEvent::Frame(frame) => Some(InputEventPayload::TouchFrame {
			time_usec: frame.time_usec(),
		}),
		TouchEvent::Cancel(cancel) => Some(InputEventPayload::TouchCancel {
			time_usec: cancel.time_usec(),
		}),
		_ => None,
	}
}

fn map_gesture_event(event: GestureEvent) -> Option<InputEventPayload> {
	match event {
		GestureEvent::Swipe(swipe) => match swipe {
			GestureSwipeEvent::Begin(begin) => Some(InputEventPayload::GestureSwipeBegin {
				device: device_id(&begin),
				time_usec: begin.time_usec(),
				fingers: begin.finger_count() as u32,
			}),
			GestureSwipeEvent::Update(update) => Some(InputEventPayload::GestureSwipeUpdate {
				device: device_id(&update),
				time_usec: update.time_usec(),
				fingers: update.finger_count() as u32,
				dx: update.dx(),
				dy: update.dy(),
			}),
			GestureSwipeEvent::End(end) => Some(InputEventPayload::GestureSwipeEnd {
				device: device_id(&end),
				time_usec: end.time_usec(),
				cancelled: end.cancelled(),
			}),
			_ => None,
		},
		GestureEvent::Pinch(pinch) => match pinch {
			GesturePinchEvent::Begin(begin) => Some(InputEventPayload::GesturePinchBegin {
				device: device_id(&begin),
				time_usec: begin.time_usec(),
				fingers: begin.finger_count() as u32,
			}),
			GesturePinchEvent::Update(update) => Some(InputEventPayload::GesturePinchUpdate {
				device: device_id(&update),
				time_usec: update.time_usec(),
				fingers: update.finger_count() as u32,
				dx: update.dx(),
				dy: update.dy(),
				scale: update.scale(),
				rotation: update.angle_delta(),
			}),
			GesturePinchEvent::End(end) => Some(InputEventPayload::GesturePinchEnd {
				device: device_id(&end),
				time_usec: end.time_usec(),
				cancelled: end.cancelled(),
			}),
			_ => None,
		},
		GestureEvent::Hold(hold) => match hold {
			GestureHoldEvent::Begin(begin) => Some(InputEventPayload::GestureHoldBegin {
				device: device_id(&begin),
				time_usec: begin.time_usec(),
				fingers: begin.finger_count() as u32,
			}),
			GestureHoldEvent::End(end) => Some(InputEventPayload::GestureHoldEnd {
				device: device_id(&end),
				time_usec: end.time_usec(),
				cancelled: end.cancelled(),
			}),
			_ => None,
		},
		#[allow(unreachable_patterns)]
		_ => None,
	}
}

fn map_tablet_event(event: TabletToolEvent) -> Option<InputEventPayload> {
	match event {
		TabletToolEvent::Proximity(proximity) => Some(InputEventPayload::TableToolProximity {
			device: device_id(&proximity),
			time_usec: proximity.time_usec(),
			in_proximity: matches!(proximity.proximity_state(), ProximityState::In),
			tool: map_tablet_tool(&proximity),
		}),
		TabletToolEvent::Axis(axis) => Some(InputEventPayload::TabletToolAxis {
			device: device_id(&axis),
			time_usec: axis.time_usec(),
			tool: map_tablet_tool(&axis),
			axes: TabletToolAxes {
				x: axis.x(),
				y: axis.y(),
				pressure: axis.pressure_has_changed().then(|| axis.pressure()),
				distance: axis.distance_has_changed().then(|| axis.distance()),
				tilt_x: axis.tilt_x_has_changed().then(|| axis.tilt_x()),
				tilt_y: axis.tilt_y_has_changed().then(|| axis.tilt_y()),
				rotation: axis.rotation_has_changed().then(|| axis.rotation()),
				slider: axis.slider_has_changed().then(|| axis.slider_position()),
				wheel_delta: axis.wheel_has_changed().then(|| axis.wheel_delta()),
				buttons: Vec::new(),
			},
		}),
		TabletToolEvent::Tip(tip) => Some(InputEventPayload::TabletToolTip {
			device: device_id(&tip),
			time_usec: tip.time_usec(),
			tool: map_tablet_tool(&tip),
			state: match tip.tip_state() {
				TipState::Down => ProtoTipState::Down,
				TipState::Up => ProtoTipState::Up,
			},
		}),
		TabletToolEvent::Button(button) => Some(InputEventPayload::TabletToolButton {
			device: device_id(&button),
			time_usec: button.time_usec(),
			tool: map_tablet_tool(&button),
			button: button.button(),
			state: map_button_state(button.button_state()),
		}),
		_ => None,
	}
}

fn map_tablet_pad_event(event: TabletPadEvent) -> Option<InputEventPayload> {
	match event {
		TabletPadEvent::Button(button) => Some(InputEventPayload::TablePadButton {
			device: device_id(&button),
			time_usec: button.time_usec(),
			button: button.button_number(),
			state: map_button_state(button.button_state()),
		}),
		TabletPadEvent::Ring(ring) => Some(InputEventPayload::TablePadRing {
			device: device_id(&ring),
			time_usec: ring.time_usec(),
			ring: ring.number(),
			position: ring.position(),
			source: match ring.source() {
				tablet_pad::RingAxisSource::Finger => AxisSource::Finger,
				tablet_pad::RingAxisSource::Unknown => AxisSource::Continuous,
			},
		}),
		TabletPadEvent::Strip(strip) => Some(InputEventPayload::TablePadStrip {
			device: device_id(&strip),
			time_usec: strip.time_usec(),
			strip: strip.number(),
			position: strip.position(),
			source: match strip.source() {
				tablet_pad::StripAxisSource::Finger => AxisSource::Finger,
				tablet_pad::StripAxisSource::Unknown => AxisSource::Continuous,
			},
		}),
		#[allow(unreachable_patterns)]
		_ => None,
	}
}

fn map_key_state(state: keyboard::KeyState) -> KeyState {
	match state {
		keyboard::KeyState::Pressed => KeyState::Pressed,
		keyboard::KeyState::Released => KeyState::Released,
	}
}

fn map_button_state(state: pointer::ButtonState) -> ButtonState {
	match state {
		pointer::ButtonState::Pressed => ButtonState::Pressed,
		pointer::ButtonState::Released => ButtonState::Released,
	}
}

fn map_tablet_tool(event: &(impl TabletToolEventTrait + EventTrait)) -> TabletTool {
	let tool = event.tool();
	TabletTool {
		serial: tool.serial(),
		tool_type: map_tablet_tool_type(tool.tool_type()),
		capability: TabletToolCapability {
			pressure: tool.has_pressure(),
			distance: tool.has_distance(),
			tilt: tool.has_tilt(),
			rotation: tool.has_rotation(),
			slider: tool.has_slider(),
			wheel: tool.has_wheel(),
		},
	}
}

fn map_tablet_tool_type(tool_type: Option<tablet_tool::TabletToolType>) -> TabletToolType {
	match tool_type {
		Some(tablet_tool::TabletToolType::Pen) => TabletToolType::Pen,
		Some(tablet_tool::TabletToolType::Eraser) => TabletToolType::Eraser,
		Some(tablet_tool::TabletToolType::Brush) => TabletToolType::Brush,
		Some(tablet_tool::TabletToolType::Pencil) => TabletToolType::Pencil,
		Some(tablet_tool::TabletToolType::Airbrush) => TabletToolType::Airbrush,
		Some(tablet_tool::TabletToolType::Mouse) => TabletToolType::Mouse,
		Some(tablet_tool::TabletToolType::Lens) => TabletToolType::Lens,
		None => TabletToolType::Pen,
		#[allow(unreachable_patterns)]
		_ => TabletToolType::Pen,
	}
}

fn device_id(event: &impl EventTrait) -> u32 {
	let device = event.device();
	let sysname = device.sysname();
	let mut hash = 2166136261u32;
	for b in sysname.as_bytes() {
		hash ^= u32::from(*b);
		hash = hash.wrapping_mul(16777619);
	}
	if hash == 0 { 1 } else { hash }
}
