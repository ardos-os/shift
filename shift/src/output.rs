use easydrm::MonitorContextCreationRequest;

use crate::egl::Egl;
use std::time::{Duration, Instant};

use crate::renderer::{BlurPipeline, PingPongBuffers, QuadRenderer};
use tab_server::MonitorIdStorage;

pub struct OutputContext {
	monitor_id: Option<String>,
	pub egl: Egl,
	pub renderer: QuadRenderer,
	pub blur_pipeline: BlurPipeline,
	pub blur_buffers: PingPongBuffers,
	fps: FpsCounter,
}

impl OutputContext {
	pub fn new(request: &MonitorContextCreationRequest<'_>) -> Self {
		let egl = Egl::load_with(request.get_proc_address);
		let renderer = QuadRenderer::new(request.gl).expect("failed to initialize renderer");
		let blur_pipeline = BlurPipeline::new(request.gl).expect("failed to initialize blur pipeline");
		let blur_buffers =
			PingPongBuffers::new(request.gl).expect("failed to initialize ping-pong buffers");
		Self {
			monitor_id: None,
			egl,
			renderer,
			blur_pipeline,
			blur_buffers,
			fps: FpsCounter::new(),
		}
	}
	pub fn monitor_id(&self) -> Option<&str> {
		self.monitor_id.as_deref()
	}

	pub fn record_frame(&mut self) -> Option<f32> {
		self.fps.tick()
	}
}

impl MonitorIdStorage for OutputContext {
	fn monitor_id(&self) -> Option<&str> {
		self.monitor_id()
	}

	fn set_monitor_id(&mut self, id: String) {
		self.monitor_id = Some(id);
	}
}

struct FpsCounter {
	last: Instant,
	frames: u32,
}

impl FpsCounter {
	fn new() -> Self {
		Self {
			last: Instant::now(),
			frames: 0,
		}
	}

	fn tick(&mut self) -> Option<f32> {
		self.frames += 1;
		let elapsed = self.last.elapsed();
		if elapsed >= Duration::from_secs(1) {
			let fps = self.frames as f32 / elapsed.as_secs_f32();
			self.frames = 0;
			self.last = Instant::now();
			Some(fps)
		} else {
			None
		}
	}
}
