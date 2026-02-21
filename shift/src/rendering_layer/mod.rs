#![allow(dead_code)]

pub mod channels;
pub mod dmabuf_import;
mod egl;

use easydrm::{gl::{self, COLOR_BUFFER_BIT, DEPTH_BUFFER_BIT}, EasyDRM, Monitor, MonitorContextCreationRequest};
use skia_safe::{
	self as skia, FilterMode, MipmapMode, Paint, SamplingOptions, gpu,
	gpu::gl::FramebufferInfo,
};
use std::{
	collections::HashMap,
	hash::Hash,
	io::ErrorKind,
	os::fd::{AsRawFd, FromRawFd, OwnedFd},
	sync::Arc,
};
use tab_protocol::BufferIndex;
use thiserror::Error;
use tokio::{io::unix::AsyncFd, sync::mpsc, task::JoinHandle};
use tracing::warn;

use crate::{
	comms::{
		render2server::{RenderEvt, RenderEvtTx},
		server2render::{RenderCmd, RenderCmdRx},
	},
	monitor::{Monitor as ServerLayerMonitor, MonitorId},
	sessions::SessionId,
};
use channels::RenderingEnd;
use dmabuf_import::{DmaBufTexture, ImportParams as DmaBufImportParams, SkiaDmaBufTexture};
// -----------------------------
// Errors
// -----------------------------

#[derive(Debug, Error)]
pub enum RenderError {
	#[error("easydrm error: {0}")]
	EasyDrmError(#[from] easydrm::EasyDRMError),

	#[error("skia GL interface creation failed")]
	SkiaGlInterface,

	#[error("skia DirectContext creation failed")]
	SkiaDirectContext,

	#[error("skia surface creation failed")]
	SkiaSurface,
}

// -----------------------------
// Per-monitor render state
// -----------------------------

pub struct MonitorRenderState {
	pub gr: gpu::DirectContext,
	pub surface: skia::Surface,
	pub width: usize,
	pub height: usize,
	pub gl: gl::Gles2,
	pub id: MonitorId,
}

impl MonitorRenderState {
	#[tracing::instrument(skip_all)]
	fn new(req: &MonitorContextCreationRequest<'_>) -> Result<Self, RenderError> {
		let interface = gpu::gl::Interface::new_load_with(|s| (req.get_proc_address)(s))
			.ok_or(RenderError::SkiaGlInterface)?;

		let mut gr =
			gpu::direct_contexts::make_gl(interface, None).ok_or(RenderError::SkiaDirectContext)?;

		let surface = skia_surface_for_current_fbo(&mut gr, req.gl, req.width, req.height)?;

		Ok(Self {
			gr,
			surface,
			width: req.width,
			height: req.height,
			gl: req.gl.clone(),
			id: MonitorId::rand(),
		})
	}

	#[tracing::instrument(skip_all, fields(width = width, height = height))]
	fn ensure_surface_size(&mut self, width: usize, height: usize) -> Result<(), RenderError> {
		if self.width == width && self.height == height {
			return Ok(());
		}
		self.width = width;
		self.height = height;
		self.surface = skia_surface_for_current_fbo(&mut self.gr, &self.gl, width, height)?;
		Ok(())
	}

	pub fn canvas(&mut self) -> &skia::Canvas {
		self.surface.canvas()
	}

	pub fn flush(&mut self) {
		self.gr.flush(None);
	}

	pub fn get_server_layer_monitor(monitor: &Monitor<Self>) -> ServerLayerMonitor {
		crate::monitor::Monitor {
			height: monitor.size().1 as _,
			width: monitor.size().0 as _,
			id: monitor.context().id,
			name: format!("Monitor {}", u32::from(monitor.connector_id())),
			refresh_rate: monitor.active_mode().vrefresh(),
		}
	}

	#[tracing::instrument(skip_all, fields(monitor_id = %self.id))]
	fn draw_texture(&mut self, texture: &mut SkiaDmaBufTexture) -> Result<(), RenderError> {
		let Some(image) = texture.image(&mut self.gr) else {
			return Err(RenderError::SkiaSurface);
		};
		let rect = skia::Rect::from_wh(self.width as f32, self.height as f32);
		let sampling = SamplingOptions::new(FilterMode::Nearest, MipmapMode::Nearest);
		let mut paint = Paint::default();
		paint.set_argb(255, 255, 255, 255);
		self
			.canvas()
			.draw_image_rect_with_sampling_options(image, None, rect, sampling, &paint);
		Ok(())
	}

}

#[derive(Default, Debug)]
struct MonitorSurfaceState {
	current_buffer: Option<BufferSlot>,
	pending_buffer: Option<BufferSlot>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct SlotKey {
	monitor_id: MonitorId,
	session_id: SessionId,
	buffer: BufferSlot,
}

impl SlotKey {
	fn new(monitor_id: MonitorId, session_id: SessionId, buffer: BufferSlot) -> Self {
		Self {
			monitor_id,
			session_id,
			buffer,
		}
	}
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum BufferSlot {
	Zero,
	One,
}

#[derive(Debug)]
enum FenceEvent {
	Signaled { key: SlotKey },
	Failed { key: SlotKey, reason: Arc<str> },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct DeferredRelease {
	monitor_id: MonitorId,
	session_id: SessionId,
	buffer: BufferSlot,
	target_flip: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SlotOwner {
	Client,
	Shift,
}

impl BufferSlot {
	fn from_index(idx: usize) -> Option<Self> {
		match idx {
			0 => Some(Self::Zero),
			1 => Some(Self::One),
			_ => None,
		}
	}
}

impl From<BufferIndex> for BufferSlot {
	fn from(value: BufferIndex) -> Self {
		match value {
			BufferIndex::Zero => BufferSlot::Zero,
			BufferIndex::One => BufferSlot::One,
		}
	}
}

impl From<BufferSlot> for BufferIndex {
	fn from(value: BufferSlot) -> Self {
		match value {
			BufferSlot::Zero => BufferIndex::Zero,
			BufferSlot::One => BufferIndex::One,
		}
	}
}

// -----------------------------
// Rendering layer
// -----------------------------

pub struct RenderingLayer {
	drm: EasyDRM<MonitorRenderState>,
	command_rx: Option<RenderCmdRx>,
	event_tx: RenderEvtTx,
	known_monitors: HashMap<MonitorId, ServerLayerMonitor>,
	monitor_state: HashMap<(MonitorId, SessionId), MonitorSurfaceState>,
	slots: HashMap<SlotKey, SkiaDmaBufTexture>,
	slot_ownership: HashMap<SlotKey, SlotOwner>,
	fence_event_tx: mpsc::UnboundedSender<FenceEvent>,
	fence_event_rx: mpsc::UnboundedReceiver<FenceEvent>,
	fence_waiters: HashMap<SlotKey, JoinHandle<()>>,
	flip_counters: HashMap<MonitorId, u64>,
	deferred_releases: Vec<DeferredRelease>,
	current_session: Option<SessionId>,
}

impl RenderingLayer {
	#[tracing::instrument(skip_all)]
	pub fn init(channels: RenderingEnd) -> Result<Self, RenderError> {
		let (command_rx, event_tx) = channels.into_parts();
		let drm = EasyDRM::init(|req| {
			// O EasyDRM chama isto com o contexto do monitor já válido/current.
			MonitorRenderState::new(req).expect("MonitorRenderState::new failed")
		})?;
		let (fence_event_tx, fence_event_rx) = mpsc::unbounded_channel();

		Ok(Self {
			drm,
			command_rx: Some(command_rx),
			event_tx,
			known_monitors: HashMap::new(),
			monitor_state: HashMap::new(),
			slots: HashMap::new(),
			slot_ownership: HashMap::new(),
			fence_event_tx,
			fence_event_rx,
			fence_waiters: HashMap::new(),
			flip_counters: HashMap::new(),
			deferred_releases: Vec::new(),
			current_session: None,
		})
	}

	#[tracing::instrument(skip_all)]
	pub async fn run(mut self) -> Result<(), RenderError> {
		let mut command_rx = self
			.command_rx
			.take()
			.expect("render command channel missing");
		let current = self.collect_monitors();
		self
			.emit_event(RenderEvt::Started {
				monitors: current.clone(),
			})
			.await;
		self.known_monitors = current.into_iter().map(|m| (m.id, m)).collect();

		'e: loop {
			// Mantém as surfaces a seguir ao tamanho real do monitor
			let monitor_ids: Vec<MonitorId> = self.drm.monitors().map(|mon| mon.context().id).collect();
			let current_session = self.current_session;
			if let Some(s) = current_session {
				for id in &monitor_ids {
					self.monitor_state.entry((*id, s)).or_default();
				}
			}
			for mon in self.drm.monitors_mut() {
				if mon.can_render() && mon.make_current().is_ok() {
					unsafe { mon.gl().ClearColor(1.0, 0.0, 0.0, 1.0); }
					unsafe{mon.gl().Clear(COLOR_BUFFER_BIT | DEPTH_BUFFER_BIT); };

					let monitor_id = mon.context().id;
					let mode = mon.active_mode();
					let (w, h) = (mode.size().0 as usize, mode.size().1 as usize);
					let context = mon.context_mut();
					context.ensure_surface_size(w, h)?;

					let key = current_session
						.and_then(|session_id| {
							let state = self
								.monitor_state
								.entry((monitor_id, session_id))
								.or_default();
							state
								.current_buffer
								.map(|buffer| SlotKey::new(monitor_id, session_id, buffer))
						});
					let texture = key.and_then(|key| {
						if self.slot_ownership.get(&key).copied() != Some(SlotOwner::Shift) {
							return None;
						}
						self.slots.get_mut(&key)
					});
					if let Some(texture) = texture {
						if let Err(e) = context.draw_texture(texture) {
							warn!(%monitor_id, "failed to draw client texture: {e:?}");
						}
					}

					context.flush();
				}
			}
			{
				let page_flip_span = tracing::span!(tracing::Level::TRACE, "drm_page_flip_ioctl");
				let _page_flip_enter = page_flip_span.enter();

				let page_flipped_monitors = self
					.drm
					.monitors()
					.filter(|m| m.was_drawn())
					.map(|m| m.context().id)
					.collect::<Vec<_>>();
				self.drm.swap_buffers()?;
				self
					.process_deferred_releases(&page_flipped_monitors)
					.await;

				self
					.emit_event(RenderEvt::PageFlip {
						monitors: page_flipped_monitors,
					})
					.await;
			}
			'l: loop {
				tokio::select! {
					cmd = command_rx.recv() => {
						if let Some(cmd) = cmd {
							if !self.handle_command(cmd).await? {
								break 'e;
							}
						} else {
							warn!("server→renderer channel closed, shutting down renderer");
							break 'e;
						}
					}
					result = self.drm.poll_events_async() => {
						result?;
						self.sync_monitors().await;
						break 'l;
					}
					fence_evt = self.fence_event_rx.recv() => {
						if let Some(fence_evt) = fence_evt {
							self.handle_fence_event(fence_evt).await;
						}
					}
				}
			}
		};
		warn!("shutting down renderer");
		Ok(())
	}
	pub fn drm(&self) -> &EasyDRM<MonitorRenderState> {
		&self.drm
	}

	fn collect_monitors(&self) -> Vec<ServerLayerMonitor> {
		self
			.drm
			.monitors()
			.map(MonitorRenderState::get_server_layer_monitor)
			.collect()
	}

	#[tracing::instrument(skip_all)]
	async fn sync_monitors(&mut self) {
		let current_list = self.collect_monitors();
		let mut current_map = HashMap::new();
		for monitor in current_list {
			if !self.known_monitors.contains_key(&monitor.id) {
				self
					.emit_event(RenderEvt::MonitorOnline {
						monitor: monitor.clone(),
					})
					.await;
			}
			current_map.insert(monitor.id, monitor);
		}
		let removed_ids = self
			.known_monitors
			.keys()
			.filter(|removed_id| !current_map.contains_key(removed_id))
			.copied()
			.collect::<Vec<_>>();
		for removed_id in removed_ids {
			self
				.emit_event(RenderEvt::MonitorOffline {
					monitor_id: removed_id,
				})
				.await;
			self.monitor_state.retain(|(mon, _), _| *mon != removed_id);
			self.cleanup_monitor_slots(removed_id);
		}
		self.known_monitors = current_map;
	}

	pub fn drm_mut(&mut self) -> &mut EasyDRM<MonitorRenderState> {
		&mut self.drm
	}

	fn texture_for_monitor(&self, monitor_id: MonitorId) -> Option<&SkiaDmaBufTexture> {
		let session_id = self.current_session?;
		let state = self.monitor_state.get(&(monitor_id, session_id))?;
		let buffer = state.current_buffer?;
		let key = SlotKey::new(monitor_id, session_id, buffer);
		self.slots.get(&key)
	}

	fn cleanup_monitor_slots(&mut self, monitor_id: MonitorId) {
		self.slots.retain(|key, _| key.monitor_id != monitor_id);
		self.slot_ownership.retain(|key, _| key.monitor_id != monitor_id);
		self.flip_counters.remove(&monitor_id);
		self
			.deferred_releases
			.retain(|item| item.monitor_id != monitor_id);
		let remove = self
			.fence_waiters
			.keys()
			.filter(|key| key.monitor_id == monitor_id)
			.copied()
			.collect::<Vec<_>>();
		for key in remove {
			if let Some(waiter) = self.fence_waiters.remove(&key) {
				waiter.abort();
			}
		}
	}

	fn cleanup_session_slots(&mut self, session_id: SessionId) {
		self.slots.retain(|key, _| key.session_id != session_id);
		self.slot_ownership.retain(|key, _| key.session_id != session_id);
		self.monitor_state.retain(|(_, sess), _| *sess != session_id);
		self
			.deferred_releases
			.retain(|item| item.session_id != session_id);
		let remove = self
			.fence_waiters
			.keys()
			.filter(|key| key.session_id == session_id)
			.copied()
			.collect::<Vec<_>>();
		for key in remove {
			if let Some(waiter) = self.fence_waiters.remove(&key) {
				waiter.abort();
			}
		}
	}

	#[tracing::instrument(skip_all, fields(session_id = %session_id, monitor_id = %payload.monitor_id))]
	fn import_framebuffers(
		&mut self,
		payload: tab_protocol::FramebufferLinkPayload,
		dma_bufs: [OwnedFd; 2],
		session_id: SessionId,
	) {
		let Ok(monitor_id) = payload.monitor_id.parse::<MonitorId>() else {
			warn!(monitor_id = %payload.monitor_id, "invalid monitor id in framebuffer link");
			return;
		};

		let mut imported = Vec::new();
		let mut found_monitor = false;
		for mon in self.drm.monitors_mut() {
			if mon.context().id != monitor_id {
				continue;
			}
			found_monitor = true;
			if let Err(e) = mon.make_current() {
				warn!(%monitor_id, "failed to make monitor current: {e:?}");
				break;
			}
			let gl = mon.context().gl.clone();
			let proc_loader = |symbol: &str| mon.get_proc_address(symbol);
			for (idx, fd) in dma_bufs.into_iter().enumerate() {
				let Some(slot) = BufferSlot::from_index(idx) else {
					continue;
				};
				let params = DmaBufImportParams {
					width: payload.width,
					height: payload.height,
					stride: payload.stride,
					offset: payload.offset,
					fourcc: payload.fourcc,
					fd,
				};
				match DmaBufTexture::import(&gl, &proc_loader, params).and_then(|texture| {
					texture.to_skia(format!(
						"session_{}_monitor_{}_buffer_{}",
						session_id, monitor_id, idx
					))
				}) {
					Ok(texture) => imported.push((slot, texture)),
					Err(e) => {
						warn!(%monitor_id, ?slot, "failed to import dmabuf: {e:?}");
					}
				}
			}
			break;
		}

		if !found_monitor {
			warn!(%monitor_id, "framebuffer link for unknown monitor");
			return;
		}

		for (slot, texture) in imported {
			let key = SlotKey::new(monitor_id, session_id, slot);
			self.slots.insert(key, texture);
			self.slot_ownership.insert(key, SlotOwner::Client);
		}
	}

	fn queue_buffer_release(
		&mut self,
		monitor_id: MonitorId,
		session_id: SessionId,
		buffer: BufferSlot,
	) {
		let target_flip = self
			.flip_counters
			.get(&monitor_id)
			.copied()
			.unwrap_or(0)
			.saturating_add(1);
		if self.deferred_releases.iter().any(|item| {
			item.monitor_id == monitor_id
				&& item.session_id == session_id
				&& item.buffer == buffer
		}) {
			return;
		}
		self.deferred_releases.push(DeferredRelease {
			monitor_id,
			session_id,
			buffer,
			target_flip,
		});
	}

	async fn process_deferred_releases(&mut self, flipped_monitors: &[MonitorId]) {
		for monitor_id in flipped_monitors {
			let next = self
				.flip_counters
				.get(monitor_id)
				.copied()
				.unwrap_or(0)
				.saturating_add(1);
			self.flip_counters.insert(*monitor_id, next);
		}
		let mut ready = Vec::new();
		self.deferred_releases.retain(|item| {
			let flips = self
				.flip_counters
				.get(&item.monitor_id)
				.copied()
				.unwrap_or(0);
			if flips >= item.target_flip {
				ready.push(*item);
				false
			} else {
				true
			}
		});
		for item in ready {
			let key = SlotKey::new(item.monitor_id, item.session_id, item.buffer);
			self.slot_ownership.insert(key, SlotOwner::Client);
			let release_fence = self.create_release_fence_for_monitor(item.monitor_id);
			self
				.emit_event(RenderEvt::BufferConsumed {
					session_id: item.session_id,
					monitor_id: item.monitor_id,
					buffer: item.buffer.into(),
					release_fence,
				})
				.await;
		}
	}

	fn create_release_fence_for_monitor(&mut self, monitor_id: MonitorId) -> Option<OwnedFd> {
		let mon = self
			.drm
			.monitors_mut()
			.find(|m| m.context().id == monitor_id)?;
		if let Err(e) = mon.make_current() {
			warn!(%monitor_id, "failed to make monitor current for release fence: {e:?}");
			return None;
		}
		let (fence_fd, sync) = match mon.create_egl_fence() {
			Ok(v) => v,
			Err(e) => {
				warn!(%monitor_id, "failed to create egl release fence: {e}");
				return None;
			}
		};
		if !sync.is_null() {
			let egl = egl::Egl::load_with(|s| mon.get_proc_address(s));
			if egl.DestroySync.is_loaded() {
				unsafe {
					egl.DestroySync(egl.GetCurrentDisplay(), sync.cast());
				}
			}
		}
		if fence_fd < 0 {
			return None;
		}
		Some(unsafe { OwnedFd::from_raw_fd(fence_fd) })
	}
}

impl RenderingLayer {
	#[tracing::instrument(skip_all)]
	async fn handle_command(&mut self, cmd: RenderCmd) -> Result<bool, RenderError> {
		match cmd {
			RenderCmd::Shutdown => {
				warn!("received shutdown request from server");
				return Ok(false);
			}
			RenderCmd::FramebufferLink {
				payload,
				dma_bufs,
				session_id,
			} => {
				self.import_framebuffers(payload, dma_bufs, session_id);
			}
			RenderCmd::SetActiveSession { session_id } => {
				self.current_session = session_id;
			}
			RenderCmd::SessionRemoved { session_id } => {
				self.cleanup_session_slots(session_id);
				if self.current_session == Some(session_id) {
					self.current_session = None;
				}
			}
			RenderCmd::SwapBuffers {
				monitor_id,
				buffer,
				session_id,
				acquire_fence,
			} => {
				let slot = BufferSlot::from(buffer);
				let monitor_known = self.known_monitors.contains_key(&monitor_id);
				let slot_key = SlotKey::new(monitor_id, session_id, slot);
				let slot_known = self.slots.contains_key(&slot_key);
				if !monitor_known || !slot_known {
					let reason: Arc<str> = if !monitor_known {
						"unknown_monitor"
					} else {
						"unlinked_buffer"
					}
					.into();
					self
						.emit_event(RenderEvt::BufferRequestRejected {
							session_id,
							monitor_id,
							buffer,
							reason,
						})
						.await;
				} else {
					let has_acquire_fence = acquire_fence.is_some();
					if let Some(fence_fd) = acquire_fence {
						self.spawn_acquire_fence_waiter(slot_key, fence_fd);
					} else {
						if let Some(waiter) = self.fence_waiters.remove(&slot_key) {
							waiter.abort();
						}
					}
					let state = self
						.monitor_state
						.entry((monitor_id, session_id))
						.or_default();
					let previous = state.current_buffer;
					state.pending_buffer = Some(slot);
					self.slot_ownership.insert(slot_key, SlotOwner::Shift);
					if !has_acquire_fence {
						state.current_buffer = Some(slot);
						state.pending_buffer = None;
						if let Some(previous) = previous.filter(|prev| *prev != slot) {
							self.queue_buffer_release(monitor_id, session_id, previous);
						}
					}
					self
						.emit_event(RenderEvt::BufferRequestAck {
							session_id,
							monitor_id,
							buffer,
						})
						.await;
				}
			}
		}

		Ok(true)
	}

	#[tracing::instrument(skip_all)]
	async fn emit_event(&self, event: RenderEvt) {
		if let Err(e) = self.event_tx.send(event).await {
			warn!("failed to send renderer event to server: {e}");
		}
	}

	fn spawn_acquire_fence_waiter(&mut self, key: SlotKey, fence_fd: OwnedFd) {
		if let Some(prev) = self.fence_waiters.remove(&key) {
			prev.abort();
		}
		let tx = self.fence_event_tx.clone();
		let handle = tokio::spawn(async move {
			let afd = match AsyncFd::new(fence_fd) {
				Ok(afd) => afd,
				Err(e) => {
					let _ = tx.send(FenceEvent::Failed {
						key,
						reason: format!("failed to register fence fd: {e}").into(),
					});
					return;
				}
			};
			loop {
				let mut guard = match afd.readable().await {
					Ok(guard) => guard,
					Err(e) => {
						let _ = tx.send(FenceEvent::Failed {
							key,
							reason: format!("fence wait failed: {e}").into(),
						});
						return;
					}
				};
				match guard.try_io(|inner| {
					let fd = inner.as_raw_fd();
					let mut poll_fd = nix::libc::pollfd {
						fd,
						events: (nix::libc::POLLIN | nix::libc::POLLERR | nix::libc::POLLHUP) as i16,
						revents: 0,
					};
					let result = unsafe { nix::libc::poll(&mut poll_fd as *mut nix::libc::pollfd, 1, 0) };
					if result > 0
						&& (poll_fd.revents
							& (nix::libc::POLLIN | nix::libc::POLLERR | nix::libc::POLLHUP) as i16)
							!= 0
					{
						Ok(())
					} else {
						Err(std::io::Error::new(
							ErrorKind::WouldBlock,
							"fence not signaled yet",
						))
					}
				}) {
					Ok(Ok(())) => {
						let _ = tx.send(FenceEvent::Signaled { key });
						return;
					}
					Ok(Err(e)) => {
						let _ = tx.send(FenceEvent::Failed {
							key,
							reason: format!("fence poll failed: {e}").into(),
						});
						return;
					}
					Err(_would_block) => continue,
				}
			}
		});
		self.fence_waiters.insert(key, handle);
	}

	async fn handle_fence_event(&mut self, event: FenceEvent) {
		match event {
			FenceEvent::Signaled { key } => {
				self.fence_waiters.remove(&key);
				if let Some(state) = self.monitor_state.get_mut(&(key.monitor_id, key.session_id)) {
					if state.pending_buffer == Some(key.buffer) {
						let previous = state.current_buffer;
						state.current_buffer = Some(key.buffer);
						state.pending_buffer = None;
						if let Some(previous) = previous.filter(|prev| *prev != key.buffer) {
							self.queue_buffer_release(key.monitor_id, key.session_id, previous);
						}
					}
				}
			}
			FenceEvent::Failed { key, reason } => {
				self.fence_waiters.remove(&key);
				warn!(%key.monitor_id, %key.session_id, buffer = ?key.buffer, %reason, "fence waiter failed, promoting pending buffer");
				if let Some(state) = self.monitor_state.get_mut(&(key.monitor_id, key.session_id)) {
					if state.pending_buffer == Some(key.buffer) {
						let previous = state.current_buffer;
						state.current_buffer = Some(key.buffer);
						state.pending_buffer = None;
						if let Some(previous) = previous.filter(|prev| *prev != key.buffer) {
							self.queue_buffer_release(key.monitor_id, key.session_id, previous);
						}
					}
				}
			}
		}
	}
}

// -----------------------------
// Skia surface helper
// -----------------------------

fn skia_surface_for_current_fbo(
	gr: &mut gpu::DirectContext,
	gl: &gl::Gles2,
	width: usize,
	height: usize,
) -> Result<skia::Surface, RenderError> {
	let mut fbo: i32 = 0;
	unsafe {
		gl.GetIntegerv(gl::FRAMEBUFFER_BINDING, &mut fbo);
	}

	let fb_info = FramebufferInfo {
		fboid: fbo as u32,
		format: gpu::gl::Format::RGBA8.into(),
		protected: gpu::Protected::No,
	};

	let backend_rt = gpu::backend_render_targets::make_gl(
		(width as i32, height as i32),
		0, // samples
		8, // stencil
		fb_info,
	);

	gpu::surfaces::wrap_backend_render_target(
		gr,
		&backend_rt,
		gpu::SurfaceOrigin::BottomLeft,
		skia::ColorType::RGBA8888,
		None,
		None,
	)
	.ok_or(RenderError::SkiaSurface)
}
