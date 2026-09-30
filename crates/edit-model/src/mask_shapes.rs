//! The drawn mask shapes: linear and radial gradients, and brush strokes.

use serde::{Deserialize, Serialize};

/// A graduated filter. Coverage ramps from 0 at `from` to 1 at `to`, measured along the axis
/// between them and constant on lines perpendicular to it.
///
/// Endpoints are in **normalised frame coordinates**, as [`CropRect`] is — so a mask means the
/// same thing on the 1600 px preview and on the full-resolution export.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct LinearGradient {
    pub from: [f32; 2],
    pub to: [f32; 2],
    pub invert: bool,
}

impl LinearGradient {
    /// Coverage at `(u, v)` in normalised frame coordinates, in `0.0..=1.0`.
    ///
    /// `aspect` is the frame's width divided by its height.
    ///
    /// ⚠ **The aspect is not decoration.** Coverage is a projection onto the axis, and a
    /// projection needs a metric. In raw normalised space the metric is anisotropic, so lines
    /// perpendicular to the axis *in that space* are not perpendicular on screen — a diagonal
    /// gradient's iso-lines would visibly skew. Scaling `u` by `aspect` restores an isotropic
    /// metric before projecting. Same species of problem as the vignette's `roundness`.
    pub fn coverage(&self, u: f32, v: f32, aspect: f32) -> f32 {
        // Work in units of frame height, where both axes have the same pixel scale.
        let (ax, ay) = (self.from[0] * aspect, self.from[1]);
        let (bx, by) = (self.to[0] * aspect, self.to[1]);
        let (dx, dy) = (bx - ax, by - ay);
        let len2 = dx * dx + dy * dy;
        if len2 <= f32::EPSILON {
            // No axis to project onto. Full coverage rather than NaN: the alternative reaches
            // the blend and paints the frame with garbage.
            return 1.0;
        }
        let t = (((u * aspect) - ax) * dx + (v - ay) * dy) / len2;
        let t = t.clamp(0.0, 1.0);
        if self.invert {
            1.0 - t
        } else {
            t
        }
    }
}

/// A radial filter. Coverage is 1 inside the ellipse and ramps to 0 at its boundary.
///
/// `center` is in **normalised frame coordinates**, as [`CropRect`] is. `radius[0]` is a
/// fraction of the frame's *width* and `radius[1]` a fraction of its *height* — so a mask means
/// the same thing on the 1600 px preview and on the full-resolution export, and the default
/// radius renders as an ellipse *similar to the frame*.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RadialGradient {
    pub center: [f32; 2],
    pub radius: [f32; 2],
    /// 0 = a hard edge at the ellipse, 100 = a ramp all the way from the centre.
    pub feather: f32,
    pub invert: bool,
}

/// ⚠ **Hand-written, not derived** — unlike [`LinearGradient`]. Container-level
/// `#[serde(default)]` fills a missing field from the *struct's* `Default`, so a derived one
/// would give every partial sidecar a zero-radius point-ellipse. `feather: 50` cannot come
/// from `f32::default()` in any case.
impl Default for RadialGradient {
    fn default() -> Self {
        Self {
            center: [0.5, 0.5],
            radius: [0.25, 0.25],
            feather: 50.0,
            invert: false,
        }
    }
}

impl RadialGradient {
    /// Below this the ramp is treated as a hard edge. The only thing between feather 0 and a
    /// division by zero — the same role `WIDTH_EPS` plays in the vignette.
    ///
    /// ⚠ `pub` so a GPU twin can **substitute** it into its shader rather than transcribe it, the
    /// rule `pipeline::SATURATION_EPS` already follows. A substituted constant cannot disagree; a
    /// transcribed one is merely checked for agreeing.
    pub const FEATHER_EPS: f32 = 1e-4;

    /// Coverage at `(u, v)` in normalised frame coordinates, in `0.0..=1.0`.
    ///
    /// ⚠⚠ **`aspect` is deliberately unused, and this is not an oversight.** Every sibling
    /// mask function needs it: [`LinearGradient::coverage`] projects onto an axis and
    /// `mask_tool::hit_test` measures a screen-space distance, and the anisotropic scaling
    /// between normalised and screen space destroys *perpendicularity* and *distance*
    /// respectively. It does **not** destroy axis-alignment: an axis-aligned ellipse maps to an
    /// axis-aligned ellipse, so the expression below is already a correct on-screen ellipse.
    ///
    /// Rotation is precisely the feature that would bring the correction back, which is the
    /// main reason this shape is axis-aligned. `radial_coverage_is_independent_of_aspect` is
    /// the guard against someone "fixing" the omission.
    ///
    /// The parameter stays in the signature so [`Mask::analytic_coverage`] has one shape across
    /// variants.
    pub fn coverage(&self, u: f32, v: f32, _aspect: f32) -> f32 {
        let (rx, ry) = (self.radius[0], self.radius[1]);
        let c = if rx <= f32::EPSILON || ry <= f32::EPSILON {
            // A degenerate ellipse honestly covers nothing. Unlike `LinearGradient`'s guard,
            // which chooses 1.0 only because its alternative was NaN, 0.0 is the true answer
            // here and cannot paint garbage either.
            0.0
        } else {
            // Normalised elliptical distance: exactly 1.0 on the boundary.
            let dx = (u - self.center[0]) / rx;
            let dy = (v - self.center[1]) / ry;
            let d = (dx * dx + dy * dy).sqrt();
            let f = (self.feather / 100.0).clamp(0.0, 1.0);
            if f <= Self::FEATHER_EPS {
                if d <= 1.0 {
                    1.0
                } else {
                    0.0
                }
            } else {
                // Smoothstep rather than a linear ramp: a linear ramp in radial distance
                // leaves a visible crease ring where the derivative jumps. The vignette's
                // feather made the same choice for the same reason.
                let t = ((1.0 - d) / f).clamp(0.0, 1.0);
                t * t * (3.0 - 2.0 * t)
            }
        };
        if self.invert {
            1.0 - c
        } else {
            c
        }
    }
}

/// One continuous drag of the brush.
///
/// ⚠ `Default` is hand-written, not derived: `flow` must default to **100**, not to
/// `f32::default()`. Container-level `#[serde(default)]` fills a missing field from the struct's
/// `Default`, so a derived one would make every stroke in every pre-flow sidecar invisible. The
/// same trap `RadialGradient`'s feather hit.
///
/// ⚠ **Points are in pre-geometry SOURCE normalised coordinates**, unlike [`LinearGradient`] and
/// [`RadialGradient`], which are in *frame* coordinates. `MaskStage` runs after `GeometryStage`,
/// so a frame-anchored stroke would slide across the image content whenever the crop changed —
/// invisible for a gradient, ruinous for a stroke painted onto a subject. The asymmetry is
/// principled: a gradient is compositional, a stroke is content-referred.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BrushStroke {
    pub points: Vec<[f32; 2]>,
    /// Radius as a fraction of the source image's **long edge**.
    ///
    /// ⚠ The long edge, not the width — `points` are normalised per axis and so live in an
    /// anisotropic space. A radius measured against width but compared against height would make
    /// every dab an ellipse. Choosing an isotropic unit avoids the correction that
    /// [`LinearGradient::coverage`] and `mask_tool::hit_test` both need.
    pub radius: f32,
    /// 0 = maximally soft, 100 = a hard edge.
    ///
    /// ⚠ Arithmetically this is `100 − feather` in [`RadialGradient`]'s maths. It keeps the brush
    /// word because that is what every brush calls it. **Do not "unify" the two** — the inversion
    /// would be silent, since both spellings look plausible.
    pub hardness: f32,
    /// How much of full coverage one pass of this stroke lays down, 0..=100.
    ///
    /// ⚠ **This is the strength control, and `hardness` is not.** Hardness shapes a dab's *edge*;
    /// even at hardness 0 a dab's centre reaches full coverage. Without flow the softest possible
    /// brush still painted at full strength and repeated passes did nothing at all — which is
    /// exactly what it felt like in use, and why this was pulled forward from a later milestone
    /// rather than left deferred.
    ///
    /// ⚠ **Defaults to 100**, so a stroke written before this field existed keeps its meaning.
    pub flow: f32,
    /// Subtract from coverage rather than adding to it.
    pub erase: bool,
    /// Paint only the pixels close to the colour under the brush centre — Lightroom's Auto Mask.
    ///
    /// ⚠ **The flag is baked at paint time**, like `radius`/`hardness`/`flow`/`erase` beside it: a
    /// stroke keeps whatever the tool was set to when it was painted, and toggling the control
    /// later changes only what the *next* stroke does.
    ///
    /// ⚠⚠ **The reference COLOUR is not stored, and that is deliberate.** It is read at render
    /// time from the pixel under each dab's own centre, so both ends of the comparison come from
    /// one buffer in one colour space. An earlier design baked the colour into the stroke and was
    /// abandoned: it put the reference in the *displayed* space while the pixels are in the
    /// working space, and `auto_mask_space_probe` measured that lightness disagreement at a mean
    /// of +0.07..+0.11 — which would have forced the gate to ignore lightness and go blind to any
    /// purely tonal edge. ⚠ The region-invariance argument originally given for baking did not
    /// survive its own sabotage; see the design doc's §2.
    pub auto_mask: bool,
    /// How close counts, 0..=100. Larger selects more.
    pub tolerance: f32,
}

impl Default for BrushStroke {
    fn default() -> Self {
        Self {
            points: Vec::new(),
            radius: 0.0,
            hardness: 0.0,
            flow: 100.0,
            erase: false,
            auto_mask: false,
            tolerance: 35.0,
        }
    }
}

/// A painted mask: an ordered list of strokes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Brush {
    pub strokes: Vec<BrushStroke>,
    /// Flip coverage, as both analytic shapes do.
    pub invert: bool,
}

impl Brush {
    /// Whether rasterizing this brush needs the image, not only the strokes.
    ///
    /// ⚠⚠ **This is what makes the dangerous state unrepresentable rather than documented.** A
    /// prebuilt coverage map is rasterized without pixels, so a gating brush served from
    /// `render_engine`'s `BrushCache` would paint positionally while the canvas tint showed it
    /// stopping at an edge — a silent divergence between preview and export, which is the failure
    /// mode this project has recorded more than any other. The cache reads this and declines,
    /// which routes such a brush to `MaskStage`'s own rasterizing path, where the buffer is in
    /// hand.
    ///
    pub fn needs_base(&self) -> bool {
        self.strokes.iter().any(|s| s.auto_mask)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Mask;

    #[test]
    fn a_stroke_written_before_auto_mask_loads_unchanged() {
        // ⚠ The guard on every existing sidecar. `BrushStroke`'s `Default` is hand-written, so a
        // missing key must come from *it* and not from `bool`/`f32`/`Vec`'s own defaults — the
        // trap `write_xmp` and `snapshot_strip_height` both document, where a field-level
        // `#[serde(default)]` silently ships a feature disabled or a value at zero.
        let json =
            r#"{"points":[[0.1,0.5]],"radius":0.1,"hardness":40.0,"flow":100.0,"erase":false}"#;
        let s: BrushStroke = serde_json::from_str(json).unwrap();
        assert!(!s.auto_mask, "an old stroke must not become auto-masked");
        assert_eq!(s.tolerance, BrushStroke::default().tolerance);
        assert_eq!(
            s.points,
            vec![[0.1, 0.5]],
            "the existing fields must still load"
        );
    }

    #[test]
    fn a_brush_needs_a_base_only_when_a_stroke_gates() {
        // ⚠ Both directions. A `needs_base` stuck at `true` would cost every brush its coverage
        // cache; one stuck at `false` would serve a gating brush a positional map, which is the
        // silent preview/export divergence `BrushCache` declines in order to prevent.
        let brush = |auto: bool| Brush {
            strokes: vec![BrushStroke {
                points: vec![[0.5, 0.5]],
                auto_mask: auto,
                ..Default::default()
            }],
            invert: false,
        };
        assert!(brush(true).needs_base());
        assert!(!brush(false).needs_base(), "auto_mask off means no gate");
        assert!(
            !Brush::default().needs_base(),
            "an empty brush needs nothing"
        );
    }

    #[test]
    fn coverage_ramps_from_zero_at_from_to_one_at_to() {
        let g = LinearGradient {
            from: [0.0, 0.0],
            to: [1.0, 0.0],
            invert: false,
        };
        assert!((g.coverage(0.0, 0.0, 1.0) - 0.0).abs() < 1e-6);
        assert!((g.coverage(1.0, 0.0, 1.0) - 1.0).abs() < 1e-6);
        assert!((g.coverage(0.5, 0.0, 1.0) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn coverage_clamps_outside_the_axis() {
        let g = LinearGradient {
            from: [0.25, 0.0],
            to: [0.75, 0.0],
            invert: false,
        };
        assert_eq!(g.coverage(0.0, 0.0, 1.0), 0.0);
        assert_eq!(g.coverage(1.0, 0.0, 1.0), 1.0);
    }

    #[test]
    fn coverage_is_constant_perpendicular_to_the_axis() {
        // A graduated filter's whole point: it varies along one direction only.
        let g = LinearGradient {
            from: [0.0, 0.0],
            to: [1.0, 0.0],
            invert: false,
        };
        let a = g.coverage(0.3, 0.0, 1.0);
        for v in [0.1, 0.5, 0.9] {
            assert!((g.coverage(0.3, v, 1.0) - a).abs() < 1e-6);
        }
    }

    #[test]
    fn invert_mirrors_the_ramp() {
        let g = LinearGradient {
            from: [0.0, 0.0],
            to: [1.0, 0.0],
            invert: true,
        };
        assert!((g.coverage(0.0, 0.0, 1.0) - 1.0).abs() < 1e-6);
        assert!((g.coverage(1.0, 0.0, 1.0) - 0.0).abs() < 1e-6);
    }

    #[test]
    fn a_diagonal_gradient_is_not_skewed_by_a_non_square_frame() {
        // ⚠⚠ The subtle one, and the reason `coverage` takes an aspect at all.
        //
        // A *line* through two points survives anisotropic scaling — but lines *perpendicular*
        // to it do not. Compute coverage in raw normalised space and a diagonal gradient's
        // iso-lines skew on screen, which reads as a rendering glitch rather than a coordinate
        // error. This is the vignette's `roundness` problem in a different hat.
        //
        // On a 2:1 frame, take two points equidistant from the axis *in pixel space* and
        // assert they get the same coverage.
        let aspect = 2.0f32;
        let g = LinearGradient {
            from: [0.0, 0.0],
            to: [1.0, 1.0],
            invert: false,
        };
        // In units of frame height the axis runs (0,0) -> (2,1); its perpendicular is
        // (1,-2)/sqrt(5). Offsets are converted back to normalised by dividing x by `aspect`.
        let (px, py) = (1.0f32 / 5.0f32.sqrt(), -2.0f32 / 5.0f32.sqrt());
        let d = 0.1f32;
        let c1 = g.coverage(0.5 + d * px / aspect, 0.5 + d * py, aspect);
        let c2 = g.coverage(0.5 - d * px / aspect, 0.5 - d * py, aspect);
        assert!(
            (c1 - c2).abs() < 1e-5,
            "perpendicular offsets gave {c1} and {c2} — the gradient is skewed"
        );
    }

    #[test]
    fn a_degenerate_gradient_covers_everything() {
        // Both endpoints equal: no axis to project onto. Returns full coverage rather than
        // dividing by zero — the safe direction, since the alternative is NaN reaching the
        // blend and painting the frame with garbage.
        let g = LinearGradient {
            from: [0.5, 0.5],
            to: [0.5, 0.5],
            invert: false,
        };
        assert_eq!(g.coverage(0.1, 0.9, 1.5), 1.0);
    }

    /// An ellipse in the middle of the frame, half as wide as the frame and 60% as tall.
    fn radial() -> RadialGradient {
        RadialGradient {
            center: [0.5, 0.5],
            radius: [0.25, 0.3],
            feather: 50.0,
            invert: false,
        }
    }

    #[test]
    fn radial_coverage_is_one_at_the_centre_and_zero_outside() {
        let r = radial();
        assert!((r.coverage(0.5, 0.5, 1.0) - 1.0).abs() < 1e-6);
        // Well outside on each axis.
        assert_eq!(r.coverage(0.95, 0.5, 1.0), 0.0);
        assert_eq!(r.coverage(0.05, 0.5, 1.0), 0.0);
        assert_eq!(r.coverage(0.5, 0.95, 1.0), 0.0);
        assert_eq!(r.coverage(0.5, 0.05, 1.0), 0.0);
        // Exactly on the boundary: the ramp has reached zero.
        assert!(r.coverage(0.75, 0.5, 1.0).abs() < 1e-6);
    }

    #[test]
    fn radial_coverage_is_independent_of_aspect() {
        // ⚠⚠ The load-bearing test of this whole feature, and the only thing standing between
        // this design and a future reader "fixing" the missing aspect correction that three
        // sibling functions all genuinely need.
        //
        // An axis-aligned ellipse is immune to the anisotropic scaling between normalised and
        // screen space, because that scaling maps an axis-aligned ellipse to an axis-aligned
        // ellipse. `LinearGradient::coverage` needs the correction only because it projects
        // onto an axis, and *perpendicularity* is what anisotropic scaling destroys.
        //
        // ⚠ The probe sits off-centre on BOTH axes. A point on either axis would be
        // insensitive to a correction applied to the other one — which is the precise shape of
        // the mistake `hit_testing_the_line_is_aspect_correct` made in 3.1a.
        let r = radial();
        let (u, v) = (0.62, 0.58);
        let base = r.coverage(u, v, 1.0);
        assert!(
            base > 0.0 && base < 1.0,
            "probe must land on the ramp: {base}"
        );
        for aspect in [0.33, 1.5, 3.0, 10.0] {
            assert!(
                (r.coverage(u, v, aspect) - base).abs() < 1e-6,
                "aspect {aspect} changed coverage: {} vs {base}",
                r.coverage(u, v, aspect)
            );
        }
    }

    #[test]
    fn feather_zero_is_a_hard_edge() {
        let r = RadialGradient {
            feather: 0.0,
            ..radial()
        };
        // Just inside and just outside the boundary at u = 0.75.
        assert_eq!(r.coverage(0.74, 0.5, 1.0), 1.0);
        assert_eq!(r.coverage(0.76, 0.5, 1.0), 0.0);

        // ⚠⚠ **The probe that actually tests the `FEATHER_EPS` guard is this one**, exactly on
        // the boundary — and the two above cannot substitute for it.
        //
        // Without the guard the ramp computes `(1.0 - d) / 0.0`. For `d < 1` that is `+inf`,
        // which clamps to 1.0; for `d > 1` it is `-inf`, which clamps to 0.0. IEEE infinity
        // arithmetic therefore produces the *correct* answer everywhere except at `d == 1.0`,
        // where it is `0.0 / 0.0` — NaN, which `f32::clamp` propagates rather than clamping,
        // and which would reach the mask blend and paint garbage.
        //
        // This was found by deleting the guard and watching the two assertions above pass. A
        // test that names a guard it cannot detect the removal of is the exact failure this
        // codebase has now hit four times.
        //
        // `center[0] + radius[0]` is 0.5 + 0.25, all exact in binary, so `d` is exactly 1.0.
        let on_boundary = r.coverage(0.75, 0.5, 1.0);
        assert!(!on_boundary.is_nan(), "the boundary must not produce NaN");
        assert_eq!(on_boundary, 1.0, "a hard edge includes its own boundary");
    }

    #[test]
    fn feather_one_hundred_ramps_from_the_centre() {
        let r = RadialGradient {
            feather: 100.0,
            ..radial()
        };
        // At the centre, full; halfway to the boundary, strictly between; at it, zero.
        assert!((r.coverage(0.5, 0.5, 1.0) - 1.0).abs() < 1e-6);
        let mid = r.coverage(0.625, 0.5, 1.0);
        assert!(mid > 0.0 && mid < 1.0, "{mid}");
        assert!(r.coverage(0.75, 0.5, 1.0).abs() < 1e-6);
        // Contrast with feather 50, where the inner half is still flat at full coverage.
        assert!((radial().coverage(0.625, 0.5, 1.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn radial_invert_flips_coverage() {
        let r = RadialGradient {
            invert: true,
            ..radial()
        };
        assert!(r.coverage(0.5, 0.5, 1.0).abs() < 1e-6);
        assert!((r.coverage(0.95, 0.5, 1.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_zero_radius_radial_is_inert_not_nan() {
        // ⚠ Reachable only from a hand-edited sidecar — `Default` and the panel both build a
        // usable ellipse — but a NaN here would reach the blend and paint the frame with
        // garbage, exactly as `LinearGradient`'s zero-length guard prevents.
        //
        // The guard yields pre-invert coverage 0.0 and then falls through the invert, so an
        // inverted point-ellipse covers everything. That is the limit of the surrounding
        // behaviour rather than a special case: as the radius shrinks, an inverted mask covers
        // all but a vanishing point.
        let r = RadialGradient {
            radius: [0.0, 0.0],
            ..radial()
        };
        assert_eq!(r.coverage(0.5, 0.5, 1.0), 0.0);
        assert_eq!(r.coverage(0.1, 0.9, 2.0), 0.0);
        assert!(!r.coverage(0.5, 0.5, 1.0).is_nan());
        let inv = RadialGradient { invert: true, ..r };
        assert_eq!(inv.coverage(0.5, 0.5, 1.0), 1.0);
    }

    #[test]
    fn a_radial_default_is_a_usable_ellipse() {
        // ⚠ NOT a derived `Default`. Container-level `#[serde(default)]` fills missing fields
        // from the struct's `Default`, so a derived one would give every partial sidecar a
        // zero-radius point-ellipse — and `feather: 50` cannot come from `f32::default()`
        // under any arrangement.
        let d = RadialGradient::default();
        assert_eq!(d.center, [0.5, 0.5]);
        assert!(d.radius[0] > 0.0 && d.radius[1] > 0.0);
        assert_eq!(d.feather, 50.0);
        assert!(!d.invert);
        // It covers its own centre, which a derived default would not.
        assert!((d.coverage(0.5, 0.5, 1.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_partial_radial_json_gets_the_hand_written_defaults() {
        let r: RadialGradient = serde_json::from_str(r#"{"center":[0.25,0.75]}"#).unwrap();
        assert_eq!(r.center, [0.25, 0.75]);
        assert_eq!(r.radius, RadialGradient::default().radius);
        assert_eq!(r.feather, 50.0);
    }

    #[test]
    fn a_radial_round_trips_through_json() {
        let src = Mask::Radial(RadialGradient {
            center: [0.25, 0.75],
            radius: [0.4, 0.1],
            feather: 12.5,
            invert: true,
        });
        let back: Mask = serde_json::from_str(&serde_json::to_string(&src).unwrap()).unwrap();
        assert_eq!(back, src);
    }

    #[test]
    fn a_brush_defaults_to_no_strokes() {
        // An empty brush must be inert rather than covering everything: a mask added but not yet
        // painted has to do nothing at all.
        assert!(Brush::default().strokes.is_empty());
    }

    #[test]
    fn a_brush_round_trips_through_json() {
        let src = Brush {
            invert: false,
            strokes: vec![
                // ⚠ One stroke carries the auto-mask fields non-neutrally, so the round trip
                // actually covers them. A pair that both defaulted would serialize nothing new
                // and this test would keep passing against a `refs` that never persisted.
                BrushStroke {
                    points: vec![[0.1, 0.2], [0.3, 0.4]],
                    radius: 0.05,
                    hardness: 60.0,
                    flow: 100.0,
                    erase: false,
                    auto_mask: true,
                    tolerance: 42.0,
                },
                BrushStroke {
                    points: vec![[0.5, 0.5]],
                    radius: 0.02,
                    hardness: 100.0,
                    flow: 100.0,
                    erase: true,
                    ..Default::default()
                },
            ],
        };
        let back: Brush = serde_json::from_str(&serde_json::to_string(&src).unwrap()).unwrap();
        assert_eq!(back, src);
    }

    #[test]
    fn a_partial_stroke_json_gets_defaults() {
        let s: BrushStroke = serde_json::from_str(r#"{"points":[[0.5,0.5]]}"#).unwrap();
        assert_eq!(s.points, vec![[0.5, 0.5]]);
        assert!(!s.erase, "a stroke paints unless it says otherwise");
    }

    #[test]
    fn the_analytic_masks_answer_coverage_directly() {
        // Both existing shapes are pure functions of position, so they answer `Some`. The brush
        // will not, which is what this method's `Option` exists to express.
        let lin = Mask::Linear(LinearGradient {
            from: [0.0, 0.0],
            to: [1.0, 0.0],
            invert: false,
        });
        assert_eq!(lin.analytic_coverage(0.5, 0.5, 1.0), Some(0.5));
        let rad = Mask::Radial(RadialGradient::default());
        assert_eq!(rad.analytic_coverage(0.5, 0.5, 1.0), Some(1.0));
    }

    #[test]
    fn a_brush_has_no_analytic_coverage() {
        // ⚠ The whole reason `analytic_coverage` returns an Option. A `0.0` here would compile,
        // run, and produce a mask that silently covers nothing.
        let m = Mask::Brush(Brush::default());
        assert_eq!(m.analytic_coverage(0.5, 0.5, 1.0), None);
    }
}
