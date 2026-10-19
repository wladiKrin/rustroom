//! AI denoise (M10.16b): the denoised photo blended in by Amount — the prefix's first step. The
//! denoising itself ran once, in the background (`rustroom_app::ai_denoise`); this only mixes.

use crate::{ImageSource, PipelineStage};
use core_image::ImageBuf;
use std::sync::Arc;

pub struct AiDenoiseStage {
    /// 0..=100.
    amount: f32,
    src: Arc<dyn ImageSource>,
}

impl AiDenoiseStage {
    pub fn new(amount: f32, src: Arc<dyn ImageSource>) -> Self {
        Self {
            amount: amount.clamp(0.0, 100.0),
            src,
        }
    }
}

impl PipelineStage for AiDenoiseStage {
    fn name(&self) -> &'static str {
        "ai_denoise"
    }

    /// `input + (denoised − input) · amount / 100`; at 100 the denoised photo exactly. ⚠ A frame the
    /// source cannot answer at its exact size passes through untouched — never a misaligned blend.
    fn apply(&self, image: ImageBuf) -> ImageBuf {
        let (w, h) = (image.width(), image.height());
        let Some(d) = self.src.image(w, h) else {
            return image;
        };
        if (d.width(), d.height()) != (w, h) {
            return image;
        }
        let t = self.amount / 100.0;
        if t >= 1.0 {
            return d;
        }
        let mut out = image;
        for (o, n) in out.data_mut().iter_mut().zip(d.data()) {
            *o += (n - *o) * t;
        }
        out
    }
}
