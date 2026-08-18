//! A masked group of per-pixel adjustments.
//!
//! Runs ordinary stages on a copy of the image and blends the result back by the mask's
//! coverage. ⚠ **Every existing stage is reused completely unchanged** — nothing else in the
//! pipeline learns that masking exists, which is what keeps the `PipelineStage` seam opaque
//! for the eventual GPU port.

use crate::{per_pixel_stages, PipelineStage, Region, RenderScale};
use core_image::ImageBuf;
use edit_model::{Geometry, MaskedAdjustment};

/// Where a mask's coverage comes from.
///
/// ⚠ Resolved **once at construction**, so `apply` matches on it outside the pixel loop. This is
/// the shape a brush forces: `Mask::analytic_coverage` is a pure function of position and a brush
/// is sampled data, so the two cannot share one code path without one of them lying.
enum CoverageSource {
    /// Linear and radial — a pure function of position.
    Analytic(edit_model::Mask),
    /// Brush.
    ///
    /// ⚠ Kept **unrasterized** when no prebuilt map is supplied: the frame's dimensions are not
    /// known until `apply`, because with no region the frame *is* the image and the pipeline is
    /// built before the image flows through it. That path is correct and slow, and it is the one
    /// the CLI and export walk — they render once and have nothing to reuse.
    Painted {
        brush: edit_model::Brush,
        geometry: Geometry,
        /// A map the caller already rasterized, from `render_engine`'s cache.
        ///
        /// ⚠ It **must** have been built at the same `fw`/`fh`. The cache key includes them, so
        /// this holds by construction — a mismatch would silently sample the wrong pixels rather
        /// than erroring, which is why the key must never lose them.
        prebuilt: Option<std::sync::Arc<crate::brush::CoverageMap>>,
    },
}

pub struct MaskStage {
    // ⚠ The `MaskedAdjustment` itself is deliberately not retained. Everything needed is
    // extracted at construction — the coverage source and the sub-stage list — and keeping a copy
    // as well was dead weight that `-D warnings` caught the moment the coverage lookup stopped
    // going through it.
    source: CoverageSource,
    stages: Vec<Box<dyn PipelineStage>>,
    region: Option<Region>,
}

impl MaskStage {
    /// For a render covering the whole frame.
    pub fn new(masked: &MaskedAdjustment, scale: RenderScale, geometry: &Geometry) -> Self {
        Self::new_in_region(masked, scale, None, geometry, None)
    }

    /// For a render covering only part of the frame — coverage is still measured against the
    /// full frame, so a cropped region is masked exactly as that part of the whole would be.
    ///
    /// ⚠ `geometry` is needed only by a brush, whose strokes are in **pre-geometry source**
    /// coordinates and must be mapped forward into frame pixels. The analytic shapes ignore it.
    pub fn new_in_region(
        masked: &MaskedAdjustment,
        scale: RenderScale,
        region: Option<Region>,
        geometry: &Geometry,
        prebuilt: Option<std::sync::Arc<crate::brush::CoverageMap>>,
    ) -> Self {
        // No spatial sub-stage yet: every adjustment a mask can carry is per-pixel. The stroke
        // radius scales through `FrameMap`, not through this.
        let _ = scale;
        let source = match &masked.mask {
            edit_model::Mask::Brush(b) => CoverageSource::Painted {
                brush: b.clone(),
                geometry: *geometry,
                prebuilt,
            },
            other => CoverageSource::Analytic(other.clone()),
        };
        Self {
            source,
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

        // ⚠ Whole-frame, never region-sized. Coverage is defined over the frame, so a
        // region-sized raster would move under panning — the same class of bug the mask overlay
        // hit in 3.1a.
        let coverage = match &self.source {
            CoverageSource::Analytic(_) => None,
            CoverageSource::Painted {
                prebuilt: Some(c), ..
            } => Some(std::sync::Arc::clone(c)),
            CoverageSource::Painted {
                brush, geometry, ..
            } => Some(std::sync::Arc::new(crate::brush::rasterize(
                brush, geometry, fw, fh,
            ))),
        };
        // ⚠ An **optimisation, not a correctness guard** — verified by deleting it and watching
        // every test still pass. An empty coverage map reads 0 everywhere, and blending at
        // coverage 0 is bit-exactly the identity, so the output is already right without this.
        // What it saves is a full-image clone plus a full run of every sub-stage, which for an
        // unpainted brush is the entire cost of the stage.
        if coverage.as_ref().is_some_and(|c| c.is_inert()) {
            return image;
        }

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
            let c = match (&self.source, &coverage) {
                (CoverageSource::Analytic(m), _) => {
                    m.analytic_coverage(u, v, aspect).unwrap_or(0.0)
                }
                // ⚠ Indexed by FRAME pixel, not by the region-relative `(x, y)`.
                (CoverageSource::Painted { .. }, Some(cov)) => {
                    cov.at(x0 + x, y0 + y) as f32 / 255.0
                }
                (CoverageSource::Painted { .. }, None) => 0.0,
            }
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
        let stage = MaskStage::new(
            &masked([0.0, 0.0], [1.0, 0.0]),
            RenderScale::FULL,
            &Geometry::default(),
        );
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
        let stage = MaskStage::new(
            &MaskedAdjustment::default(),
            RenderScale::FULL,
            &Geometry::default(),
        );
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

        let whole = MaskStage::new(&m, RenderScale::FULL, &Geometry::default()).apply(flat(fw, fh));

        let region = Region {
            full_w: fw,
            full_h: fh,
            x0,
            y0,
        };
        let part = MaskStage::new_in_region(
            &m,
            RenderScale::FULL,
            Some(region),
            &Geometry::default(),
            None,
        )
        .apply(flat(rw, rh));

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

    use edit_model::{Brush, BrushStroke};

    fn painted() -> MaskedAdjustment {
        MaskedAdjustment {
            mask: Mask::Brush(Brush {
                strokes: vec![BrushStroke {
                    points: vec![[0.5, 0.5]],
                    radius: 0.2,
                    hardness: 100.0,
                    flow: 100.0,
                    erase: false,
                }],
                invert: false,
            }),
            adjustments: LocalAdjustments {
                exposure: 1.0,
                ..Default::default()
            },
        }
    }

    #[test]
    fn a_brush_mask_brightens_only_where_it_was_painted() {
        let stage = MaskStage::new(&painted(), RenderScale::FULL, &Geometry::default());
        let out = stage.apply(flat(80, 40));
        // `flat` is mid-grey 0.5, so +1 EV lifts the painted core to 1.0.
        assert!(
            out.pixel(40, 20)[0] > 0.9,
            "the painted centre must be lifted"
        );
        assert!(
            (out.pixel(2, 2)[0] - 0.5).abs() < 1e-6,
            "the unpainted corner must be untouched"
        );
    }

    #[test]
    fn an_unpainted_brush_mask_changes_nothing() {
        // ⚠ A mask added but not yet painted must be exactly inert — not merely close. This
        // pins *inertness*, not the early return above: that return is an optimisation, and
        // removing it leaves this test passing (checked).
        let m = MaskedAdjustment {
            mask: Mask::Brush(Brush::default()),
            ..painted()
        };
        let stage = MaskStage::new(&m, RenderScale::FULL, &Geometry::default());
        let src = flat(80, 40);
        let out = stage.apply(src.clone());
        assert_eq!(out, src);
    }

    #[test]
    fn a_brush_mask_in_a_region_matches_the_same_part_of_the_whole() {
        // The region path must agree with the whole-frame path pixel for pixel, which is what
        // makes zooming safe. The same property `a_region_render_matches_the_whole_render_cropped`
        // pins for the pipeline at large.
        let whole =
            MaskStage::new(&painted(), RenderScale::FULL, &Geometry::default()).apply(flat(80, 40));
        let region = Region {
            full_w: 80,
            full_h: 40,
            x0: 30,
            y0: 10,
        };
        let got = MaskStage::new_in_region(
            &painted(),
            RenderScale::FULL,
            Some(region),
            &Geometry::default(),
            None,
        )
        .apply(flat(20, 20));
        for y in 0..20 {
            for x in 0..20 {
                assert_eq!(
                    got.pixel(x, y),
                    whole.pixel(30 + x, 10 + y),
                    "at ({x}, {y})"
                );
            }
        }
    }

    #[test]
    fn the_analytic_masks_are_unaffected() {
        // The refactor must not disturb the two existing shapes.
        let m = MaskedAdjustment {
            mask: Mask::Radial(edit_model::RadialGradient::default()),
            adjustments: LocalAdjustments {
                exposure: 1.0,
                ..Default::default()
            },
        };
        let out = MaskStage::new(&m, RenderScale::FULL, &Geometry::default()).apply(flat(80, 40));
        assert!(
            out.pixel(40, 20)[0] > 0.9,
            "the ellipse centre must be lifted"
        );
        assert!(
            (out.pixel(1, 1)[0] - 0.5).abs() < 1e-6,
            "the corner is outside"
        );
    }
}
