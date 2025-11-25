use crate::animations::animation::BasicAnimation;
use crate::animations::animation::easing;
use crate::animations::{AnimationStateTracker, Transition, TransitionFrame};
use crate::dma_buf_importer::ExternalTexture;
use crate::renderer::{AnimationCanvas, Transform2D};

#[derive(Clone, Copy)]
pub enum SlideDirection {
	Left,
	Right,
	Up,
	Down,
}

pub struct SlideTransition {
	direction: SlideDirection,
}

impl SlideTransition {
	pub const fn new(direction: SlideDirection) -> Self {
		Self { direction }
	}
}

impl Transition for SlideTransition {
	fn timeline(&self) -> AnimationStateTracker {
		AnimationStateTracker::from(BasicAnimation::new("slide", 1.0, easing::ease_in_out_cubic))
	}

	fn render(
		&self,
		canvas: &mut AnimationCanvas<'_>,
		primary: &ExternalTexture,
		secondary: Option<&ExternalTexture>,
		frame: TransitionFrame<'_>,
	) {
		let progress = frame.value("slide").clamp(0.0, 1.0);
		let (primary_transform, secondary_transform) = slide_transforms(self.direction, progress);
		canvas.draw_texture(primary, primary_transform);
		if let Some(next) = secondary {
			canvas.draw_texture(next, secondary_transform);
		}
	}
}

fn slide_transforms(direction: SlideDirection, progress: f32) -> (Transform2D, Transform2D) {
	let distance = 2.0; // full screen in clip-space
	let mut primary = Transform2D::identity();
	let mut secondary = Transform2D::identity();
	match direction {
		SlideDirection::Left => {
			primary.translate[0] = -distance * progress;
			secondary.translate[0] = distance * (1.0 - progress);
		}
		SlideDirection::Right => {
			primary.translate[0] = distance * progress;
			secondary.translate[0] = -distance * (1.0 - progress);
		}
		SlideDirection::Up => {
			primary.translate[1] = distance * progress;
			secondary.translate[1] = -distance * (1.0 - progress);
		}
		SlideDirection::Down => {
			primary.translate[1] = -distance * progress;
			secondary.translate[1] = distance * (1.0 - progress);
		}
	}
	(primary, secondary)
}
