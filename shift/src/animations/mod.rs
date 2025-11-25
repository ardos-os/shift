pub mod animation;
mod blur;
mod crossfade;
mod slide;
use crate::dma_buf_importer::ExternalTexture;
use crate::renderer::AnimationCanvas;

pub use blur::BlurFade;
pub use crossfade::CrossFade;
pub use slide::{SlideDirection, SlideTransition};

pub use animation::AnimationStateTracker;

#[derive(Clone, Copy)]
pub struct TransitionFrame<'a> {
	#[allow(dead_code)]
	progress: f32,
	tracker: &'a AnimationStateTracker,
}

impl<'a> TransitionFrame<'a> {
	pub const fn new(progress: f32, tracker: &'a AnimationStateTracker) -> Self {
		Self { progress, tracker }
	}

	#[allow(dead_code)]
	pub fn progress(&self) -> f32 {
		self.progress
	}

	pub fn value(&self, id: &str) -> f32 {
		self.tracker.get_animation_progress(id)
	}
}

pub trait Transition: Sync + Send {
	fn timeline(&self) -> AnimationStateTracker;
	fn render(
		&self,
		canvas: &mut AnimationCanvas<'_>,
		primary: &ExternalTexture,
		secondary: Option<&ExternalTexture>,
		frame: TransitionFrame<'_>,
	);
}

static CROSS_FADE_TRANSITION: CrossFade = CrossFade;
static SLIDE_LEFT_TRANSITION: SlideTransition = SlideTransition::new(SlideDirection::Left);
static SLIDE_RIGHT_TRANSITION: SlideTransition = SlideTransition::new(SlideDirection::Right);
static SLIDE_UP_TRANSITION: SlideTransition = SlideTransition::new(SlideDirection::Up);
static SLIDE_DOWN_TRANSITION: SlideTransition = SlideTransition::new(SlideDirection::Down);
static BLUR_TRANSITION: BlurFade = BlurFade;

pub fn resolve_transition(name: &str) -> &'static dyn Transition {
	match name.to_ascii_lowercase().as_str() {
		"slideleft" => &SLIDE_LEFT_TRANSITION,
		"slideright" => &SLIDE_RIGHT_TRANSITION,
		"slideup" => &SLIDE_UP_TRANSITION,
		"slidedown" => &SLIDE_DOWN_TRANSITION,
		"blur" => &BLUR_TRANSITION,
		_ => &CROSS_FADE_TRANSITION,
	}
}
