use crate::animations::animation::BasicAnimation;
use crate::animations::animation::easing;
use crate::animations::{AnimationStateTracker, Transition, TransitionFrame};
use crate::dma_buf_importer::ExternalTexture;
use crate::renderer::{AnimationCanvas, Transform2D};

pub struct CrossFade;

impl Transition for CrossFade {
	fn timeline(&self) -> AnimationStateTracker {
		AnimationStateTracker::from(BasicAnimation::new("mix", 1.0, easing::ease_in_out_cubic))
	}

	fn render(
		&self,
		canvas: &mut AnimationCanvas<'_>,
		primary: &ExternalTexture,
		secondary: Option<&ExternalTexture>,
		frame: TransitionFrame<'_>,
	) {
		let mix = frame.value("mix").clamp(0.0, 1.0);
		if let Some(secondary) = secondary {
			canvas.draw_texture_tweening(primary, secondary, mix, Transform2D::identity());
		} else {
			canvas.draw_texture(primary, Transform2D::identity());
		}
	}
}
