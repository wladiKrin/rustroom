//! AI denoise (M10.16b): the denoised photo blended in by Amount — the prefix's first step. The
//! denoising itself ran once, in the background (`rustroom_app::ai_denoise`); this only mixes.

use crate::{ImageSource, PipelineStage};
use core_image::ImageBuf;
use rayon::prelude::*;
use std::sync::Arc;

/// Values a parallel task blends — a whole number of pixels.
const CHUNK: usize = 3 * 16_384;

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
        // ⚠ In place, on every core, reading the result where it lies: single-threaded over a copy it
        // was 110–160 ms a frame at 1:1, re-run each time a partial result landed (M10.16b6).
        let mut out = image;
        out.data_mut()
            .par_chunks_mut(CHUNK)
            .zip(d.data().par_chunks(CHUNK))
            .for_each(|(o, n)| {
                if t >= 1.0 {
                    o.copy_from_slice(n);
                } else {
                    for (o, n) in o.iter_mut().zip(n) {
                        *o += (n - *o) * t;
                    }
                }
            });
        out
    }
}
