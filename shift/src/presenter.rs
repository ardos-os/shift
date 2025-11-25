use std::collections::HashMap;

use easydrm::{EasyDRM, Monitor, gl};

use crate::animations::{AnimationStateTracker, Transition, TransitionFrame, resolve_transition};
use crate::dma_buf_importer::ExternalTexture;
use crate::error::{FrameAck, RenderError};
use crate::output::OutputContext;
use crate::renderer::{AnimationCanvas, Transform2D};
use tab_server::{MonitorRenderSnapshot, RenderSnapshot, RenderTransition};
use tracing::info;

pub struct FramePresenter {
	active_transition: Option<ActiveTransition>,
}

struct ActiveTransition {
	transition: &'static dyn Transition,
	state: AnimationStateTracker,
	last_progress: f32,
}

impl ActiveTransition {
	fn new(transition: &'static dyn Transition) -> Self {
		Self {
			state: transition.timeline(),
			transition,
			last_progress: 0.0,
		}
	}

	fn is_same_transition(&self, other: &'static dyn Transition) -> bool {
		std::ptr::eq(self.transition, other)
	}

	fn frame(&mut self, progress: f32) -> TransitionFrame<'_> {
		let delta = progress - self.last_progress;
		self.last_progress = progress;
		self.state.update(delta);
		TransitionFrame::new(progress, &self.state)
	}
}

impl FramePresenter {
	pub fn new() -> Self {
		Self {
			active_transition: None,
		}
	}

	pub fn render(
		&mut self,
		snapshot: &RenderSnapshot<'_, ExternalTexture>,
		easydrm: &mut EasyDRM<OutputContext>,
	) -> Result<FrameAck, RenderError> {
		let mut rendered = Vec::new();
		let monitor_lookup: HashMap<_, _> = snapshot
			.monitors
			.iter()
			.map(|m| (m.monitor_id, m))
			.collect();
		let transition_context = self.transition_context(snapshot.transition.as_ref());
		for monitor in easydrm.monitors_mut() {
			if !monitor.can_render() {
				continue;
			}
			let monitor_id = match monitor.context().monitor_id() {
				Some(id) => id.to_string(),
				None => continue,
			};
			let Some(snapshot_monitor) = monitor_lookup.get(monitor_id.as_str()) else {
				continue;
			};
			let sessions = render_single_monitor(
				monitor_id.as_str(),
				monitor,
				snapshot_monitor,
				snapshot.transition.as_ref(),
				transition_context,
				snapshot.active_session_id,
			)?;
			for session_id in sessions {
				rendered.push((monitor_id.clone(), session_id));
			}
		}
		Ok(rendered)
	}

	fn transition_context<'a>(
		&'a mut self,
		transition: Option<&'a RenderTransition<'a>>,
	) -> Option<TransitionContext<'a>> {
		let Some(trans) = transition else {
			self.active_transition = None;
			return None;
		};
		if trans.progress >= 1.0 {
			self.active_transition = None;
			return None;
		}
		let progress = trans.progress as f32;
		let resolved = resolve_transition(trans.animation);
		let needs_reset = match self.active_transition.as_ref() {
			Some(existing) => !existing.is_same_transition(resolved),
			None => true,
		};
		if needs_reset {
			self.active_transition = Some(ActiveTransition::new(resolved));
		}
		let active = self.active_transition.as_mut().unwrap();
		let transition_ptr = active.transition;
		let frame = active.frame(progress);
		Some(TransitionContext {
			transition: transition_ptr,
			frame,
		})
	}
}

#[derive(Clone, Copy)]
struct TransitionContext<'a> {
	transition: &'static dyn Transition,
	frame: TransitionFrame<'a>,
}

fn render_single_monitor(
	monitor_id: &str,
	monitor: &mut Monitor<OutputContext>,
	snapshot_monitor: &MonitorRenderSnapshot<'_, ExternalTexture>,
	transition: Option<&RenderTransition>,
	transition_context: Option<TransitionContext<'_>>,
	active_session_id: Option<&str>,
) -> Result<Vec<String>, RenderError> {
	let previous_session_id = transition.and_then(|t| t.previous_session_id);
	monitor
		.make_current()
		.map_err(|e| RenderError::MakeCurrent(e.to_string()))?;
	let (width, height) = monitor.size();
	let gl = monitor.gl();
	gl!(gl, Viewport(0, 0, width as i32, height as i32));
	gl!(gl, ClearColor(0.0, 0.0, 0.0, 1.0));
	gl!(gl, Clear(gl::COLOR_BUFFER_BIT));

	let (maybe_presented, needs_fps) = {
		let (width_i32, height_i32) = (width as i32, height as i32);
		let context = monitor.context_mut();
		let mut canvas = AnimationCanvas::new(
			&context.renderer,
			&context.blur_pipeline,
			&mut context.blur_buffers,
			(width_i32, height_i32),
		);

		if let (Some(trans), Some(ctx)) = (transition, transition_context) {
			if let (Some(prev_tex), Some(new_tex), Some(prev_id), Some(new_id)) = (
				snapshot_monitor.previous_texture,
				snapshot_monitor.active_texture,
				trans.previous_session_id,
				active_session_id,
			) {
				ctx
					.transition
					.render(&mut canvas, prev_tex, Some(new_tex), ctx.frame);
				let mut presented = Vec::new();
				push_presented(&mut presented, Some(prev_id));
				push_presented(&mut presented, Some(new_id));
				(Some(presented), true)
			} else {
				(None, false)
			}
		} else if let (Some(texture), Some(session_id)) =
			(snapshot_monitor.active_texture, active_session_id)
		{
			let mut presented = Vec::new();
			canvas.draw_texture(texture, Transform2D::identity());
			push_presented(&mut presented, Some(session_id));
			(Some(presented), true)
		} else if let (Some(texture), Some(session_id)) =
			(snapshot_monitor.previous_texture, previous_session_id)
		{
			let mut presented = Vec::new();
			canvas.draw_texture(texture, Transform2D::identity());
			push_presented(&mut presented, Some(session_id));
			(Some(presented), true)
		} else {
			(None, false)
		}
	};

	if let Some(presented) = maybe_presented {
		if needs_fps {
			if let Some(fps) = monitor.context_mut().record_frame() {
				info!(monitor_id = monitor_id, fps = fps, "Shift FPS");
			}
		}
		return Ok(presented);
	}

	clear_monitor(monitor)?;
	Ok(Vec::new())
}

fn clear_monitor(monitor: &mut Monitor<OutputContext>) -> Result<(), RenderError> {
	monitor
		.make_current()
		.map_err(|e| RenderError::MakeCurrent(e.to_string()))?;
	let (width, height) = monitor.size();
	let gl = monitor.gl();
	gl!(gl, Viewport(0, 0, width as i32, height as i32));
	gl!(gl, ClearColor(0.0, 0.0, 0.0, 1.0));
	gl!(gl, Clear(gl::COLOR_BUFFER_BIT));
	Ok(())
}

fn push_presented(vec: &mut Vec<String>, id: Option<&str>) {
	if let Some(id) = id {
		if !vec.iter().any(|existing| existing == id) {
			vec.push(id.to_string());
		}
	}
}
