//! A masked group of per-pixel adjustments.
//!
//! Runs ordinary stages on a copy of the image and blends the result back by the mask's
//! coverage. ⚠ **Every existing stage is reused completely unchanged** — nothing else in the
//! pipeline learns that masking exists, which is what keeps the `PipelineStage` seam opaque
//! for the eventual GPU port.

use crate::{per_pixel_stages, PipelineStage, Region, RenderScale};
use core_image::ImageBuf;
use edit_model::MaskedAdjustment;

pub struct MaskStage {
    masked: MaskedAdjustment,
    stages: Vec<Box<dyn PipelineStage>>,
    region: Option<Region>,
}

impl MaskStage {
    /// For a render covering the whole frame.
    pub fn new(masked: &MaskedAdjustment, scale: RenderScale) -> Self {
        Self::new_in_region(masked, scale, None)
    }

    /// For a render covering only part of the frame — coverage is still measured against the
    /// full frame, so a cropped region is masked exactly as that part of the whole would be.
    pub fn new_in_region(
        masked: &MaskedAdjustment,
        scale: RenderScale,
        region: Option<Region>,
    ) -> Self {
        // No spatial sub-stage yet: every adjustment a mask can carry is per-pixel. 3.1c's
        // brush will need the scale, for the stroke radius.
        let _ = scale;
        Self {
            masked: *masked,
            stages: per_pixel_stages(&masked.adjustments),
            region,
        }
    }
}

impl PipelineStage for MaskStage {
    fn name(&self) -> &'static str {
        "mask"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        let (w, h) = (image.width(), image.height());
        if w == 0 || h == 0 || self.stages.is_empty() {
            return image;
        }

        // ⚠ Without a region the buffer *is* the frame — byte for byte the whole-frame path.
        let (fw, fh, x0, y0) = match self.region {
            Some(r) => (r.full_w, r.full_h, r.x0, r.y0),
            None => (w, h, 0, 0),
        };
        if fw == 0 || fh == 0 {
            return image;
        }
        let aspect = fw as f32 / fh as f32;

        // Run the sub-stages on a copy. The clone is the price of reusing every stage
        // unchanged; a bounding-box optimisation is deferred to 3.1b/c, where masks are small
        // enough for it to pay.
        let mut adjusted = image.clone();
        for stage in &self.stages {
            adjusted = stage.apply(adjusted);
        }

        // ⚠ `map_pixels_xy` rather than a hand-written loop: it is rayon-parallel over rows
        // and is the idiom `VignetteStage` already uses for position-dependent work. The
        // closure captures `&adjusted`, which is `Sync` because `ImageBuf` is a `Vec<f32>`.
        image.map_pixels_xy(|x, y, base| {
            // Pixel position in the *frame's* normalised coordinates.
            let u = (x0 + x) as f32 / fw as f32;
            let v = (y0 + y) as f32 / fh as f32;
            // Task 7 replaces this with a `CoverageSource` resolved outside the loop; for now
            // the two analytic shapes are the only ones that exist.
            let c = self
                .masked
                .mask
                .analytic_coverage(u, v, aspect)
                .unwrap_or(0.0)
                .clamp(0.0, 1.0);
            let adj = adjusted.pixel(x, y);
            [
                base[0] + (adj[0] - base[0]) * c,
                base[1] + (adj[1] - base[1]) * c,
                base[2] + (adj[2] - base[2]) * c,
            ]
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edit_model::{LinearGradient, LocalAdjustments, Mask};

    /// A flat mid-grey image.
    fn flat(w: usize, h: usize) -> ImageBuf {
        let mut img = ImageBuf::new(w, h);
        for s in img.data_mut() {
            *s = 0.5;
        }
        img
    }

    fn masked(from: [f32; 2], to: [f32; 2]) -> MaskedAdjustment {
        MaskedAdjustment {
            mask: Mask::Linear(LinearGradient {
                from,
                to,
                invert: false,
            }),
            adjustments: LocalAdjustments {
                exposure: 1.0,
                ..Default::default()
            },
        }
    }

    #[test]
    fn coverage_zero_leaves_the_pixel_untouched_and_one_applies_fully() {
        // The two endpoints of the blend. +1 EV doubles a linear sample, so mid-grey 0.5
        // becomes 1.0 where coverage is full and stays 0.5 where it is zero.
        let stage = MaskStage::new(&masked([0.0, 0.0], [1.0, 0.0]), RenderScale::FULL);
        let img = stage.apply(flat(64, 16));
        let left = img.pixel(0, 8)[0];
        let right = img.pixel(63, 8)[0];
        assert!((left - 0.5).abs() < 1e-4, "left edge moved: {left}");
        assert!(
            (right - 1.0).abs() < 2e-2,
            "right edge should be near +1 EV: {right}"
        );
    }

    #[test]
    fn an_inactive_adjustment_set_is_a_no_op() {
        // No stages to run means no clone and no blend — and byte-identical output, which is
        // what keeps the existing goldens unchanged when `masks` is empty.
        let stage = MaskStage::new(&MaskedAdjustment::default(), RenderScale::FULL);
        let before = flat(8, 8);
        let after = stage.apply(before.clone());
        assert_eq!(after, before);
    }

    #[test]
    fn a_region_render_matches_the_whole_frame_cropped() {
        // ⚠ The load-bearing test, and a direct descendant of the vignette's. A mask is
        // defined in *frame* coordinates while the 1:1 view renders a crop, so the region's
        // own dimensions must never be mistaken for the frame's. Get this wrong and masks
        // slide as you zoom.
        let (fw, fh) = (64usize, 32usize);
        let (x0, y0, rw, rh) = (10usize, 6usize, 20usize, 12usize);
        let m = masked([0.0, 0.0], [1.0, 1.0]);

        let whole = MaskStage::new(&m, RenderScale::FULL).apply(flat(fw, fh));

        let region = Region {
            full_w: fw,
            full_h: fh,
            x0,
            y0,
        };
        let part =
            MaskStage::new_in_region(&m, RenderScale::FULL, Some(region)).apply(flat(rw, rh));

        for y in 0..rh {
            for x in 0..rw {
                assert_eq!(
                    part.pixel(x, y),
                    whole.pixel(x0 + x, y0 + y),
                    "region pixel ({x},{y}) differs from the whole frame's ({},{})",
                    x0 + x,
                    y0 + y
                );
            }
        }
    }
}
