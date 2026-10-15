//! Lens Blur (M10.16d): a synthetic shallow depth of field — each pixel blurred by its distance from the
//! focal plane on the estimated depth map, in three layers so nothing smears across the subject's
//! outline, with discs in the aperture's shape. The spec: `docs/superpowers/specs/2026-10-14-m10.16d-
//! lens-blur-design.md`.

pub mod aperture;
pub mod gather;
pub mod radius;

pub use gather::render;
pub use radius::{radius, Params};

use crate::{PipelineStage, RasterSource};
use core_image::ImageBuf;
use std::sync::Arc;

/// Lens Blur as the prefix's last step. Its planes are resolved at the frame it runs on — the reason
/// M10.16a made a map a lazy [`RasterSource`]: the prefix has not produced that frame when the step is
/// built.
pub struct LensBlurStage {
    lb: edit_model::LensBlur,
    focus: f32,
    depth: Arc<dyn RasterSource>,
    /// The sharp subject's alpha — Subject mode only; Point mode has no separate layer.
    subject: Option<Arc<dyn RasterSource>>,
}

impl LensBlurStage {
    pub fn new(
        lb: &edit_model::LensBlur,
        focus: f32,
        depth: Arc<dyn RasterSource>,
        subject: Option<Arc<dyn RasterSource>>,
    ) -> Self {
        Self {
            lb: *lb,
            focus,
            depth,
            subject,
        }
    }
}

impl PipelineStage for LensBlurStage {
    fn name(&self) -> &'static str {
        "lens_blur"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        let (w, h) = (image.width(), image.height());
        let plane = |s: &dyn RasterSource| -> Vec<f32> {
            let m = s.coverage(w, h);
            (0..h)
                .flat_map(|y| (0..w).map(move |x| (y, x)))
                .map(|(y, x)| m.at(x, y) as f32 / 255.0)
                .collect()
        };
        let depth = plane(&*self.depth);
        let subject = self.subject.as_deref().map(plane);
        let p = Params::new(&self.lb, self.focus, w.max(h));
        render(&image, &depth, subject.as_deref(), &p)
    }
}
