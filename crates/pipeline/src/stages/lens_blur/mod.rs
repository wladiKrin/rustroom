//! Lens Blur (M10.16d): a synthetic shallow depth of field — each pixel blurred by its distance from the
//! focal plane on the estimated depth map, in three layers so nothing smears across the subject's
//! outline, with discs in the aperture's shape. The spec: `docs/superpowers/specs/2026-10-14-m10.16d-
//! lens-blur-design.md`.

pub mod aperture;
pub mod gather;
pub mod radius;

pub use gather::{highlight_level, level_of, render, GRID, RCAP};
pub use radius::{radius, Params, RAMP};

use crate::{AiRasters, MapPlane, PipelineStage, RasterSource};
use core_image::ImageBuf;
use std::sync::Arc;

/// What the step reads besides its settings: the focal depth and the planes with their identities.
pub struct Inputs {
    pub focus: f32,
    pub depth: (Arc<dyn RasterSource>, u64),
    /// Subject mode only.
    pub subject: Option<(Arc<dyn RasterSource>, u64)>,
}

/// Lens Blur's inputs from the maps, or `None` while one it needs has not arrived.
///
/// ⚠⚠ **The ONE lookup** — `build_prefix_steps` keys and builds the CPU step from it, and the GPU seam
/// builds the twin from it (M10.16d2), so the two cannot disagree about which planes or focus a frame
/// was blurred with.
pub fn inputs(lb: &edit_model::LensBlur, ai: &dyn AiRasters) -> Option<Inputs> {
    let depth = ai.plane(MapPlane::Depth)?;
    let subject = match lb.focus {
        edit_model::LensFocus::Subject => Some(ai.plane(MapPlane::Subject)?),
        edit_model::LensFocus::Point { .. } => None,
    };
    let focus = ai.lens_focus(lb)?;
    Some(Inputs {
        focus,
        depth,
        subject,
    })
}

/// A plane at `w × h`, row-major, `0..=255` — what both the CPU step and the GPU twin read.
pub fn plane_bytes(src: &dyn RasterSource, w: usize, h: usize) -> Vec<u8> {
    let m = src.coverage(w, h);
    (0..h)
        .flat_map(|y| (0..w).map(move |x| (x, y)))
        .map(|(x, y)| m.at(x, y))
        .collect()
}

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
            plane_bytes(s, w, h)
                .into_iter()
                .map(|b| b as f32 / 255.0)
                .collect()
        };
        let depth = plane(&*self.depth);
        let subject = self.subject.as_deref().map(plane);
        let p = Params::new(&self.lb, self.focus, w.max(h));
        render(&image, &depth, subject.as_deref(), &p)
    }
}
