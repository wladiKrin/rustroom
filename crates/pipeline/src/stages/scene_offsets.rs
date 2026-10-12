//! A mask's scene sliders as **offsets** to the global ones — Lightroom's model (M10.12b).
//!
//! In place of the global white balance, exposure, contrast and tone regions whenever an active
//! mask carries one of them: per pixel, the value used is `global + Σ coverage_i × offset_i`, then
//! the four stages' own arithmetic in their order, through the functions they themselves call. So a
//! mask's +1 EV at full coverage IS the global +1 EV there, half coverage is +0.5 EV, overlapping
//! masks add — and where no mask covers, the output is bit-identical to the four stages.
//!
//! ⚠⚠ Why it exists: a mask used to run its own white balance, exposure, contrast and tone regions
//! AFTER the global tone curve, saturation, HSL and grading — a mask's +1 EV landed on darks the
//! curve had already shaped (`examples/probe_mask_offsets.rs`: 14.7 levels mean, 85 at p99, 77 % of
//! pixels off by more than 3 levels on DSCF6291).

use crate::stages::{contrast, exposure, tone_regions};
use crate::{MaskStage, PipelineStage, RenderScale};
use core_image::ImageBuf;
use edit_model::{Edits, Geometry, MaskedAdjustment};

/// The global scene values.
struct Scene {
    /// The global white balance's gains (`[1, 1, 1]` when inactive).
    gains: [f32; 3],
    ev: f32,
    contrast: f32,
    /// `[highlights, shadows, whites, blacks]`, −100..=100.
    regions: [f32; 4],
}

/// One mask's contribution: where it covers, and what it adds there.
struct Contribution {
    /// A coverage-only stage: the mask's shape with no adjustments — `coverage_plane` is the one fold.
    coverage: MaskStage,
    ev: f32,
    contrast: f32,
    regions: [f32; 4],
    /// `ln(R)`, `R` the gains of the global white balance shifted by this mask, over the global gains.
    ln_r: [f32; 3],
}

pub struct SceneOffsetsStage {
    global: Scene,
    masks: Vec<Contribution>,
}

impl SceneOffsetsStage {
    /// The stage for `edits`, or `None` when no active mask carries a scene offset — then the tail is
    /// built exactly as before (`build.rs`).
    pub fn new(
        edits: &Edits,
        scale: RenderScale,
        region: Option<crate::Region>,
        geometry: &Geometry,
        brush: Option<&std::sync::Arc<crate::BrushCoverage>>,
        baseline: color::wb::Illuminant,
    ) -> Option<Self> {
        let gains = crate::global_wb_gains(&edits.white_balance, baseline);
        let masks: Vec<Contribution> = edits
            .masks
            .iter()
            .enumerate()
            // ⚠ The position in `edits.masks`, never a counter over contributing masks — the
            // brush rows are indexed by it (`build.rs`' rule for `MaskStage`).
            .filter(|(_, m)| !m.disabled && m.adjustments.has_scene_offsets())
            .map(|(i, m)| {
                let a = &m.adjustments;
                let (temp, tint) = crate::mask_wb_shift(a, baseline);
                let ln_r = if temp == 0.0 && tint == 0.0 {
                    [0.0; 3]
                } else {
                    let shifted =
                        crate::shifted_white_balance(&edits.white_balance, baseline, temp, tint);
                    let r = crate::white_balance_gains(&shifted, baseline);
                    [
                        (r[0] / gains[0]).ln(),
                        (r[1] / gains[1]).ln(),
                        (r[2] / gains[2]).ln(),
                    ]
                };
                Contribution {
                    // ⚠⚠ **No deposit.** The brush coverage cache keys a gating brush's map on the
                    // base `MaskStage` reads, after the global tail; this stage reads before it,
                    // so a map deposited from here would be served to `MaskStage` as a stale hit.
                    coverage: MaskStage::new_in_region(
                        &shape_only(m),
                        scale,
                        region,
                        geometry,
                        brush.map_or(&[][..], |b| b.row(i)),
                        baseline,
                        None,
                    ),
                    ev: a.exposure,
                    contrast: a.contrast,
                    regions: [a.highlights, a.shadows, a.whites, a.blacks],
                    ln_r,
                }
            })
            .collect();
        if masks.is_empty() {
            return None;
        }
        Some(Self {
            global: Scene {
                gains,
                ev: edits.exposure,
                contrast: edits.contrast,
                regions: [edits.highlights, edits.shadows, edits.whites, edits.blacks],
            },
            masks,
        })
    }
}

impl PipelineStage for SceneOffsetsStage {
    fn name(&self) -> &'static str {
        "scene_offsets"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        let w = image.width();
        let planes: Vec<Vec<f32>> = self
            .masks
            .iter()
            .map(|m| m.coverage.coverage_plane(&image))
            .collect();
        let g = &self.global;
        image.map_pixels_xy(|x, y, p| {
            let i = y * w + x;
            let (mut ev, mut con, mut reg, mut ln_r) = (g.ev, g.contrast, g.regions, [0.0f32; 3]);
            for (m, plane) in self.masks.iter().zip(&planes) {
                let c = plane[i];
                // ⚠ Skipped at zero, so where no mask covers every sum is the global value
                // bit for bit — never `g + 0.0 × offset`, which is the same here but says less.
                if c == 0.0 {
                    continue;
                }
                ev += c * m.ev;
                con += c * m.contrast;
                for k in 0..4 {
                    reg[k] += c * m.regions[k];
                }
                for k in 0..3 {
                    ln_r[k] += c * m.ln_r[k];
                }
            }
            // The four stages' order: white balance, exposure, contrast, tone regions.
            let gains = [
                g.gains[0] * ln_r[0].exp(),
                g.gains[1] * ln_r[1].exp(),
                g.gains[2] * ln_r[2].exp(),
            ];
            let mut p = [p[0] * gains[0], p[1] * gains[1], p[2] * gains[2]];
            if ev != 0.0 {
                let f = exposure::factor(ev);
                p = [p[0] * f, p[1] * f, p[2] * f];
            }
            // ⚠ Only when non-zero: the pivot arithmetic is not exactly the identity at slope 1,
            // and the four-stage tail inserts no contrast stage at 0.
            if con != 0.0 {
                p = contrast::apply(p, con);
            }
            if reg != [0.0; 4] {
                let a = [
                    reg[0] / 100.0,
                    reg[1] / 100.0,
                    reg[2] / 100.0,
                    reg[3] / 100.0,
                ];
                let gain = tone_regions::gain(color::luminance(p), a).max(0.0);
                p = [p[0] * gain, p[1] * gain, p[2] * gain];
            }
            p
        })
    }
}

/// `m`'s shape — base, refinements, whole-mask Invert — with no adjustments: what its coverage is.
pub(crate) fn shape_only(m: &MaskedAdjustment) -> MaskedAdjustment {
    MaskedAdjustment {
        mask: m.mask.clone(),
        refine: m.refine.clone(),
        invert: m.invert,
        adjustments: Default::default(),
        disabled: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edit_model::{LinearGradient, LocalAdjustments, Mask, RadialGradient};

    /// A small frame of varied colours across the whole range, headroom included.
    fn scene(w: usize, h: usize) -> ImageBuf {
        let mut img = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let t = x as f32 / (w - 1) as f32;
                let s = y as f32 / (h - 1) as f32;
                img.set_pixel(x, y, [t * 1.2, 0.1 + 0.6 * s, 0.05 + 0.5 * (1.0 - t) * s]);
            }
        }
        img
    }

    fn everything() -> Mask {
        Mask::Linear(LinearGradient {
            from: [-1.0, 0.0],
            to: [-0.5, 0.0],
            invert: false,
        })
    }

    fn masked(mask: Mask, a: LocalAdjustments) -> MaskedAdjustment {
        MaskedAdjustment {
            mask,
            adjustments: a,
            ..Default::default()
        }
    }

    fn stage(e: &Edits) -> SceneOffsetsStage {
        SceneOffsetsStage::new(
            e,
            RenderScale::FULL,
            None,
            &Geometry::default(),
            None,
            color::wb::Illuminant::default(),
        )
        .expect("a mask with a scene offset builds the stage")
    }

    fn ev(v: f32) -> LocalAdjustments {
        LocalAdjustments {
            exposure: v,
            ..Default::default()
        }
    }

    #[test]
    fn half_coverage_is_half_an_offset() {
        let ramp = Mask::Linear(LinearGradient {
            from: [0.0, 0.0],
            to: [1.0, 0.0],
            invert: false,
        });
        let m = masked(ramp, ev(1.0));
        let e = Edits {
            masks: vec![m.clone()],
            ..Default::default()
        };
        let img = scene(64, 16);
        let cov = MaskStage::new(&shape_only(&m), RenderScale::FULL, &Geometry::default())
            .coverage_plane(&img);
        let out = stage(&e).apply(img.clone());
        let x = 32;
        let c = cov[8 * 64 + x];
        assert!(c > 0.2 && c < 0.8, "fixture: partial coverage, {c}");
        let want = img.pixel(x, 8)[0] * exposure::factor(c);
        assert!(
            (out.pixel(x, 8)[0] - want).abs() < 1e-6,
            "{} vs {want}",
            out.pixel(x, 8)[0]
        );
    }

    #[test]
    fn overlapping_masks_add() {
        let e = Edits {
            masks: vec![masked(everything(), ev(0.5)), masked(everything(), ev(0.5))],
            ..Default::default()
        };
        let img = scene(32, 8);
        let want = crate::ExposureStage::new(1.0).apply(img.clone());
        assert_eq!(stage(&e).apply(img), want);
    }

    #[test]
    fn an_inverted_mask_offsets_outside_its_shape() {
        let disc = Mask::Radial(RadialGradient {
            center: [0.5, 0.5],
            radius: [0.25, 0.25],
            feather: 0.0,
            invert: false,
        });
        let mut m = masked(disc, ev(1.0));
        m.invert = true;
        let e = Edits {
            masks: vec![m],
            ..Default::default()
        };
        let img = scene(64, 64);
        let out = stage(&e).apply(img.clone());
        assert_eq!(out.pixel(32, 32), img.pixel(32, 32), "inside: no offset");
        let (a, b) = (out.pixel(2, 2)[0], img.pixel(2, 2)[0] * 2.0);
        assert!((a - b).abs() < 1e-6, "outside: +1 EV, {a} vs {b}");
    }

    #[test]
    fn a_waiting_ai_mask_offsets_nothing() {
        let mut m = masked(Mask::Ai(edit_model::AiMask::default()), ev(1.0));
        m.invert = true;
        let e = Edits {
            masks: vec![m],
            ..Default::default()
        };
        let img = scene(32, 8);
        assert_eq!(stage(&e).apply(img.clone()), img);
    }

    #[test]
    fn a_region_is_offset_as_that_part_of_the_whole() {
        let ramp = Mask::Linear(LinearGradient {
            from: [0.1, 0.0],
            to: [0.9, 1.0],
            invert: false,
        });
        let e = Edits {
            masks: vec![masked(ramp, ev(1.0))],
            ..Default::default()
        };
        let img = scene(64, 48);
        let whole = stage(&e).apply(img.clone());
        let (x0, y0, w, h) = (20, 10, 30, 25);
        let region = crate::Region {
            full_w: 64,
            full_h: 48,
            x0,
            y0,
        };
        let part = SceneOffsetsStage::new(
            &e,
            RenderScale::FULL,
            Some(region),
            &Geometry::default(),
            None,
            color::wb::Illuminant::default(),
        )
        .unwrap()
        .apply(img.crop(x0, y0, w, h));
        assert_eq!(part, whole.crop(x0, y0, w, h));
    }
}
