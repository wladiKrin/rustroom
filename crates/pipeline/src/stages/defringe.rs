//! Manual defringe (M9.6): the purple and green halos that longitudinal chromatic aberration leaves
//! beside high-contrast edges, anywhere in the frame — which a lens profile's lateral (TCA)
//! correction does not touch.
//!
//! Per pixel, in Oklab:
//! 1. **Edge proximity** — the Sobel magnitude of Oklab lightness, max-filtered over a radius that
//!    scales with the render, mapped through a smoothstep. A fringe lives *beside* an edge.
//! 2. **Hue membership** — 1 inside the purple or green range, feathered [`FEATHER_DEG`] outside,
//!    by circular distance so a range may sit across 0°.
//! 3. **Amount** — the global amount plus every mask's coverage × its local slider, clamped to
//!    0..100. That is what lets a mask at −100 *protect* its area, and one at +50 defringe where
//!    the global amount is 0 (with the global hue ranges).
//! 4. Chroma scaled by `1 − edge · membership · amount / 100`. Lightness is untouched, so a fringe
//!    goes grey rather than dark.
//!
//! ⚠ A purple flower's interior sits far from any strong edge and is left alone; its rim can still
//! be caught, which is exactly what a mask at −100 is for.
//!
//! ⚠ One step **after geometry**, in the frame the masks live in, so global and local are one
//! computation — see `build_prefix_steps`.

use crate::stages::mask::MaskStage;
use crate::{PipelineStage, RenderScale};
use color::oklab::{oklab_to_prophoto, prophoto_to_oklab};
use core_image::ImageBuf;
use edit_model::Defringe;
use rayon::prelude::*;

/// The edge search radius at full resolution, in pixels. Longitudinal CA on a 26 Mpx sensor spreads
/// a few pixels from the edge; the radius shrinks with the render so preview and export agree.
pub const RADIUS_FULL: f32 = 6.0;
/// Hue feather outside a range, degrees.
pub const FEATHER_DEG: f32 = 10.0;
/// Edge strength (half the lightness step across a pixel, see `sobel`) where the correction starts
/// and where it is complete.
const EDGE_LO: f32 = 0.02;
const EDGE_HI: f32 = 0.08;

pub struct DefringeStage {
    global: Defringe,
    /// Every enabled mask carrying a local defringe, with its slider.
    locals: Vec<(MaskStage, f32)>,
    radius: usize,
}

impl DefringeStage {
    pub fn new(global: Defringe, locals: Vec<(MaskStage, f32)>, scale: RenderScale) -> Self {
        Self {
            global,
            locals,
            radius: (RADIUS_FULL * scale.get()).ceil().max(1.0) as usize,
        }
    }
}

fn smoothstep(lo: f32, hi: f32, x: f32) -> f32 {
    let t = ((x - lo) / (hi - lo)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// How far hue `h` (degrees) lies inside `range`, 1 inside, feathered to 0 by circular distance.
pub fn membership(range: [f32; 2], h: f32) -> f32 {
    let (lo, hi) = (range[0].min(range[1]), range[0].max(range[1]));
    let centre = 0.5 * (lo + hi);
    let half = 0.5 * (hi - lo);
    let d = (h - centre).rem_euclid(360.0);
    let d = d.min(360.0 - d);
    1.0 - smoothstep(half, half + FEATHER_DEG, d)
}

/// Sobel magnitude of `l`, scaled so a unit step reads 0.5 on each of the two pixels beside it.
fn sobel(l: &[f32], w: usize, h: usize) -> Vec<f32> {
    let at = |x: isize, y: isize| {
        let x = x.clamp(0, w as isize - 1) as usize;
        let y = y.clamp(0, h as isize - 1) as usize;
        l[y * w + x]
    };
    let mut g = vec![0.0f32; w * h];
    g.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let y = y as isize;
        for (x, o) in row.iter_mut().enumerate() {
            let x = x as isize;
            let gx = (at(x + 1, y - 1) + 2.0 * at(x + 1, y) + at(x + 1, y + 1))
                - (at(x - 1, y - 1) + 2.0 * at(x - 1, y) + at(x - 1, y + 1));
            let gy = (at(x - 1, y + 1) + 2.0 * at(x, y + 1) + at(x + 1, y + 1))
                - (at(x - 1, y - 1) + 2.0 * at(x, y - 1) + at(x + 1, y - 1));
            *o = gx.hypot(gy) / 8.0;
        }
    });
    g
}

/// Separable square max filter of radius `r`.
fn max_filter(v: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut rows = vec![0.0f32; w * h];
    rows.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let src = &v[y * w..(y + 1) * w];
        for (x, o) in row.iter_mut().enumerate() {
            let (a, b) = (x.saturating_sub(r), (x + r).min(w - 1));
            *o = src[a..=b].iter().copied().fold(0.0, f32::max);
        }
    });
    let mut out = vec![0.0f32; w * h];
    out.par_chunks_mut(w).enumerate().for_each(|(y, row)| {
        let (a, b) = (y.saturating_sub(r), (y + r).min(h - 1));
        for (x, o) in row.iter_mut().enumerate() {
            *o = (a..=b).map(|yy| rows[yy * w + x]).fold(0.0, f32::max);
        }
    });
    out
}

impl PipelineStage for DefringeStage {
    fn name(&self) -> &'static str {
        "defringe"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        let (w, h) = (image.width(), image.height());
        if w == 0 || h == 0 {
            return image;
        }
        let lab: Vec<[f32; 3]> = (0..w * h)
            .into_par_iter()
            .map(|i| prophoto_to_oklab(image.pixel(i % w, i / w)))
            .collect();
        let l: Vec<f32> = lab.iter().map(|p| p[0]).collect();
        let edge = max_filter(&sobel(&l, w, h), w, h, self.radius);
        let planes: Vec<(Vec<f32>, f32)> = self
            .locals
            .iter()
            .map(|(m, amount)| (m.coverage_plane(&image), *amount))
            .collect();
        let g = self.global;
        image.map_pixels_xy(|x, y, c| {
            let i = y * w + x;
            let e = smoothstep(EDGE_LO, EDGE_HI, edge[i]);
            if e <= 0.0 {
                return c;
            }
            let local: f32 = planes.iter().map(|(p, a)| p[i] * a).sum();
            let [ll, a, b] = lab[i];
            let hue = b.atan2(a).to_degrees();
            let purple =
                (g.purple_amount + local).clamp(0.0, 100.0) * membership(g.purple_hue, hue);
            let green = (g.green_amount + local).clamp(0.0, 100.0) * membership(g.green_hue, hue);
            let wgt = (e * purple.max(green) / 100.0).clamp(0.0, 1.0);
            if wgt <= 0.0 {
                return c;
            }
            let k = 1.0 - wgt;
            oklab_to_prophoto([ll, a * k, b * k])
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A dark/bright vertical edge at x = 20 of a 96×16 frame, with a 3 px band of `fringe` on
    /// the bright side, and a 30 px patch of the same colour at x 60..90, whose centre is further
    /// from its own rim than the edge radius.
    fn chart(fringe: [f32; 3]) -> ImageBuf {
        let mut img = ImageBuf::new(96, 16);
        for y in 0..16 {
            for x in 0..96 {
                let c = if x < 20 {
                    [0.02, 0.02, 0.02]
                } else if x < 23 || (60..90).contains(&x) {
                    fringe
                } else {
                    [0.6, 0.6, 0.6]
                };
                img.set_pixel(x, y, c);
            }
        }
        img
    }

    /// A purple of moderate lightness — its Oklab hue is asserted, not assumed.
    fn purple() -> [f32; 3] {
        let c = color::srgb_to_prophoto([0.35, 0.12, 0.55]);
        let [_, a, b] = prophoto_to_oklab(c);
        let h = b.atan2(a).to_degrees().rem_euclid(360.0);
        assert!(
            membership(Defringe::DEFAULT_PURPLE_HUE, h) > 0.99,
            "fixture: the purple's hue {h}° must sit inside the default range"
        );
        c
    }

    fn chroma(c: [f32; 3]) -> f32 {
        let [_, a, b] = prophoto_to_oklab(c);
        a.hypot(b)
    }

    fn purple_only(amount: f32) -> Defringe {
        Defringe {
            purple_amount: amount,
            ..Default::default()
        }
    }

    #[test]
    fn a_fringe_beside_an_edge_is_greyed_and_the_same_colour_away_from_it_survives() {
        let img = chart(purple());
        let out = DefringeStage::new(purple_only(100.0), Vec::new(), RenderScale::FULL)
            .apply(img.clone());
        let before = chroma(img.pixel(21, 8));
        assert!(before > 0.1, "fixture: the fringe must be saturated");
        assert!(
            chroma(out.pixel(21, 8)) < 0.1 * before,
            "the fringe kept {} of {before}",
            chroma(out.pixel(21, 8))
        );
        // ⚠ Far from the dark/bright edge the patch's own rim is an edge too, so its centre is
        // what must survive — the flower's interior, not its outline.
        assert_eq!(
            out.pixel(75, 8),
            img.pixel(75, 8),
            "the isolated patch's centre moved"
        );
    }

    #[test]
    fn lightness_is_untouched() {
        let img = chart(purple());
        let out = DefringeStage::new(purple_only(100.0), Vec::new(), RenderScale::FULL)
            .apply(img.clone());
        let l0 = prophoto_to_oklab(img.pixel(21, 8))[0];
        let l1 = prophoto_to_oklab(out.pixel(21, 8))[0];
        assert!((l0 - l1).abs() < 1e-4, "{l0} → {l1}");
    }

    /// A fringe outside the purple range survives purple defringe — and is caught by green's.
    #[test]
    fn the_hue_range_selects_which_fringe() {
        let green = color::srgb_to_prophoto([0.15, 0.5, 0.2]);
        let img = chart(green);
        let by_purple = DefringeStage::new(purple_only(100.0), Vec::new(), RenderScale::FULL)
            .apply(img.clone());
        assert_eq!(by_purple.pixel(21, 8), img.pixel(21, 8));
        let by_green = DefringeStage::new(
            Defringe {
                green_amount: 100.0,
                ..Default::default()
            },
            Vec::new(),
            RenderScale::FULL,
        )
        .apply(img.clone());
        assert!(chroma(by_green.pixel(21, 8)) < 0.2 * chroma(img.pixel(21, 8)));
    }

    #[test]
    fn a_zero_amount_is_the_identity() {
        let img = chart(purple());
        let out = DefringeStage::new(Defringe::default(), Vec::new(), RenderScale::FULL)
            .apply(img.clone());
        assert_eq!(out.data(), img.data());
    }

    /// The local slider: a mask over the left half. −100 protects that half from a global 100;
    /// +50 defringes it with the global at 0 — and in both the other half is the control.
    #[test]
    fn a_mask_protects_at_minus_100_and_adds_at_plus_50() {
        let mask = |amount: f32| {
            let m = edit_model::MaskedAdjustment {
                mask: edit_model::Mask::Linear(edit_model::LinearGradient {
                    // Meant to cover the left (the fringe at x = 21) and not x = 44 — asserted
                    // below from the plane itself rather than from a reading of this literal.
                    from: [0.40, 0.5],
                    to: [0.30, 0.5],
                    invert: false,
                }),
                refine: Vec::new(),
                adjustments: edit_model::LocalAdjustments {
                    defringe: amount,
                    ..Default::default()
                },
                disabled: false,
            };
            let stage = MaskStage::new_capture(
                &m,
                RenderScale::FULL,
                &edit_model::Geometry::default(),
                &[],
                None,
            );
            vec![(stage, amount)]
        };
        let mut img = chart(purple());
        // A second edge + fringe: bright 30..43, fringe 43..46, dark 46..56.
        for y in 0..16 {
            for x in 30..56 {
                img.set_pixel(
                    x,
                    y,
                    if x < 43 {
                        [0.6; 3]
                    } else if x < 46 {
                        purple()
                    } else {
                        [0.02; 3]
                    },
                );
            }
        }
        let (covered, open) = ((21, 8), (44, 8));
        let plane = mask(1.0)[0].0.coverage_plane(&img);
        assert!(plane[8 * 96 + 21] > 0.99, "fixture: x = 21 must be covered");
        assert!(plane[8 * 96 + 44] < 0.01, "fixture: x = 44 must not be");
        let protected = DefringeStage::new(purple_only(100.0), mask(-100.0), RenderScale::FULL)
            .apply(img.clone());
        assert_eq!(
            protected.pixel(covered.0, covered.1),
            img.pixel(covered.0, covered.1)
        );
        assert!(chroma(protected.pixel(open.0, open.1)) < 0.2 * chroma(img.pixel(open.0, open.1)));

        let added = DefringeStage::new(Defringe::default(), mask(50.0), RenderScale::FULL)
            .apply(img.clone());
        assert!(
            chroma(added.pixel(covered.0, covered.1))
                < 0.8 * chroma(img.pixel(covered.0, covered.1))
        );
        assert_eq!(added.pixel(open.0, open.1), img.pixel(open.0, open.1));
    }

    #[test]
    fn membership_is_circular_and_feathered() {
        assert_eq!(membership([280.0, 340.0], 310.0), 1.0);
        assert_eq!(membership([280.0, 340.0], 200.0), 0.0);
        let edge = membership([280.0, 340.0], 345.0);
        assert!(edge > 0.0 && edge < 1.0, "{edge}");
        // A range across 0°: [-10, 30] holds 355° and 10°.
        assert_eq!(membership([-10.0, 30.0], 355.0), 1.0);
        assert_eq!(membership([-10.0, 30.0], 10.0), 1.0);
    }
}
