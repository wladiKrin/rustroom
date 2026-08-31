//! The perspective-correction homography — pure maths, no images.
//!
//! ⚠ Deliberately free of `ImageBuf`: every property here is testable with points and floats,
//! which is where this project's defect record says the tests actually pay. M3.1c-A's pure
//! `FrameMap` carried ~30 tests and produced no defect at all, while the canvas around it
//! produced six, none of which a test caught.
//!
//! Coordinates are normalised about the frame centre with **half-height = 1**, so `y` runs ±1
//! and `x` runs ±aspect. A point `(x, y)` is the camera ray `(x, y, f)`.

use edit_model::Perspective;

/// The assumed focal length, in units of the frame's half-height. 2.0 is about a 24 mm lens on
/// full frame.
///
/// ⚠⚠ **Fixed, not read from EXIF, and that is a decision rather than a shortcut.** For *any*
/// assumed `f` there is a `θ` that sends the vertical vanishing point to infinity —
/// `tan θ = −f / y_v` — so the verticals come out exactly parallel whichever value is chosen.
/// What differs is how much the rest of the frame stretches, and that is precisely what the
/// Aspect slider corrects.
///
/// ⚠ Reading `Capture.focal_len` instead would make the tool behave one way on a RAW and another
/// on a JPEG carrying no EXIF — two behaviours that differ invisibly, the failure
/// `render_max_dim` and `Session::geometry_mode` were each fixed for.
///
/// ⚠ The consequence, stated so it is not later filed as a bug: the same Vertical value is **not**
/// the same physical camera pitch on a 16 mm shot and a 200 mm one. It means "this much
/// keystone", judged by eye — which is how Lightroom's manual sliders behave too.
pub const F_NORM: f32 = 2.0;

/// What ±100 on Vertical or Horizontal means, in degrees. A tuning constant; the `hue` bands'
/// ±100 → ±30° precedent.
pub const MAX_ANGLE_DEG: f32 = 30.0;

/// What ±100 on Aspect means, as a stretch factor. `x` is multiplied by `s` and `y` by `1/s`, so
/// the transform is area-preserving to first order.
pub const MAX_ASPECT: f32 = 1.4;

/// Below this an autoscale probe is treated as being on the axis and skipped; it only guards a
/// division.
const PROBE_EPS: f32 = 1e-3;

/// Past this there is nothing usable left in the frame, so the autoscale stops climbing.
const MAX_AUTOSCALE: f32 = 8.0;

/// How close to "everything inside" the autoscale iteration has to get before it stops.
const AUTOSCALE_EPS: f32 = 1e-3;

/// ⚠⚠ **The autoscale ITERATES, and a single pass is not enough — measured, not assumed.**
/// `Corrector::compute_autoscale` gets away with one pass because lens distortion is radial and
/// very nearly linear in scale. A projective map is neither: scaling the output coordinates does
/// **not** divide the source coordinates by the same factor. On a 60 Vertical at 3:2, one pass
/// gives 1.97 and the bottom corners still read source y = 1.26, comfortably outside the frame —
/// the empty wedge the slider exists to remove would simply have stayed.
const MAX_AUTOSCALE_ITERS: usize = 12;

/// A 3×3 projective transform on normalised frame coordinates, together with its inverse.
///
/// The forward direction maps **source → output**. Rendering inverse-maps, so [`Self::source_of`]
/// is what the stage calls.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Homography {
    fwd: [[f32; 3]; 3],
    inv: [[f32; 3]; 3],
}

fn mul(a: &[[f32; 3]; 3], b: &[[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let mut m = [[0.0f32; 3]; 3];
    for (i, row) in m.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            *cell = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    m
}

/// The adjugate over the determinant.
///
/// ⚠ A projective matrix is only defined up to scale, so dividing by the determinant is
/// cosmetic — but keeping it is what makes `fwd · inv == I` exactly, and therefore testable.
fn invert(m: &[[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let cof = |r: usize, col: usize| -> f32 {
        let rs: [usize; 2] = if r == 0 {
            [1, 2]
        } else if r == 1 {
            [0, 2]
        } else {
            [0, 1]
        };
        let cs: [usize; 2] = if col == 0 {
            [1, 2]
        } else if col == 1 {
            [0, 2]
        } else {
            [0, 1]
        };
        let d = m[rs[0]][cs[0]] * m[rs[1]][cs[1]] - m[rs[0]][cs[1]] * m[rs[1]][cs[0]];
        if (r + col).is_multiple_of(2) {
            d
        } else {
            -d
        }
    };
    let det = m[0][0] * cof(0, 0) + m[0][1] * cof(0, 1) + m[0][2] * cof(0, 2);
    // A degenerate matrix cannot arise from the bounded angles this builds, but dividing by zero
    // would give a whole frame of NaN rather than an error, so fall back to the identity.
    if det.abs() < f32::EPSILON {
        return [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    }
    let mut out = [[0.0f32; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            // Transposed: the inverse is the adjugate, i.e. the transpose of the cofactor matrix.
            *cell = cof(j, i) / det;
        }
    }
    out
}

fn apply(m: &[[f32; 3]; 3], p: (f32, f32)) -> (f32, f32) {
    let x = m[0][0] * p.0 + m[0][1] * p.1 + m[0][2];
    let y = m[1][0] * p.0 + m[1][1] * p.1 + m[1][2];
    let w = m[2][0] * p.0 + m[2][1] * p.1 + m[2][2];
    if w.abs() < f32::EPSILON {
        // The point is on the horizon and has no finite image. Push it far away rather than
        // returning NaN, so the sampler simply reads outside the frame and gets black.
        return (x * 1e6, y * 1e6);
    }
    (x / w, y / w)
}

impl Homography {
    /// Build from angles directly, in radians.
    ///
    /// ⚠ Separate from [`Self::from_edits`] on purpose: it lets the geometric tests state their
    /// own angles and stay independent of [`MAX_ANGLE_DEG`], so re-tuning a slider range cannot
    /// break a test about the maths.
    pub fn from_angles(pitch: f32, yaw: f32, aspect_s: f32, scale_k: f32) -> Self {
        let f = F_NORM;
        // (x, y, 1) → the ray (x, y, f).
        let k_in = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, f]];
        // (X, Y, Z) → (fX, fY, Z), which after the homogeneous divide is (fX/Z, fY/Z).
        let k_out = [[f, 0.0, 0.0], [0.0, f, 0.0], [0.0, 0.0, 1.0]];
        let (sp, cp) = pitch.sin_cos();
        let r_pitch = [[1.0, 0.0, 0.0], [0.0, cp, -sp], [0.0, sp, cp]];
        let (sy, cy) = yaw.sin_cos();
        let r_yaw = [[cy, 0.0, sy], [0.0, 1.0, 0.0], [-sy, 0.0, cy]];
        let stretch = [
            [aspect_s, 0.0, 0.0],
            [0.0, 1.0 / aspect_s, 0.0],
            [0.0, 0.0, 1.0],
        ];
        let zoom = [[scale_k, 0.0, 0.0], [0.0, scale_k, 0.0], [0.0, 0.0, 1.0]];

        let r = mul(&r_pitch, &r_yaw);
        let fwd = mul(&zoom, &mul(&stretch, &mul(&k_out, &mul(&r, &k_in))));
        Self {
            inv: invert(&fwd),
            fwd,
        }
    }

    /// Map the sliders onto angles. One place, so [`Self::unscaled`] and [`Self::from_edits`]
    /// cannot disagree about what a slider value means.
    fn angles(p: &Perspective) -> (f32, f32, f32) {
        let to_rad = |v: f32| (v / 100.0) * MAX_ANGLE_DEG.to_radians();
        (
            to_rad(p.vertical),
            to_rad(p.horizontal),
            MAX_ASPECT.powf(p.aspect / 100.0),
        )
    }

    /// Build from the sliders **without** the zoom — `scale_k` is 1.0.
    ///
    /// ⚠ Two steps rather than one because the autoscale is computed *from* the un-zoomed
    /// transform: see [`Self::autoscale`].
    pub fn unscaled(p: &Perspective) -> Self {
        let (pitch, yaw, s) = Self::angles(p);
        Self::from_angles(pitch, yaw, s, 1.0)
    }

    /// The full transform for these sliders at this frame aspect, autoscale included.
    pub fn from_edits(p: &Perspective, aspect: f32) -> Self {
        let full = Self::autoscale(p, aspect);
        // 0..100 interpolates 1.0 → autoscale, exactly as `LensCorrection::scale` does.
        let k = 1.0 + (full - 1.0) * (p.scale.clamp(0.0, 100.0) / 100.0);
        let (pitch, yaw, s) = Self::angles(p);
        Self::from_angles(pitch, yaw, s, k)
    }

    /// Where in the source an output point comes from. Normalised coordinates both sides.
    pub fn source_of(&self, p: (f32, f32)) -> (f32, f32) {
        apply(&self.inv, p)
    }

    /// Where a source point lands in the output.
    pub fn output_of(&self, p: (f32, f32)) -> (f32, f32) {
        apply(&self.fwd, p)
    }

    /// The scale that just fills the frame with real image data.
    ///
    /// ⚠ Iterates, because one pass is not enough — see [`MAX_AUTOSCALE_ITERS`]. Each round
    /// rebuilds the transform at the scale found so far and asks what is still hanging outside;
    /// the sequence contracts quickly, converging in three or four rounds at realistic angles.
    pub fn autoscale(p: &Perspective, aspect: f32) -> f32 {
        let (pitch, yaw, stretch) = Self::angles(p);
        let mut k = 1.0f32;
        for _ in 0..MAX_AUTOSCALE_ITERS {
            let need = Self::from_angles(pitch, yaw, stretch, k).overflow(aspect);
            if need <= 1.0 + AUTOSCALE_EPS {
                break;
            }
            k = (k * need).min(MAX_AUTOSCALE);
        }
        k.clamp(1.0, MAX_AUTOSCALE)
    }

    /// The largest factor by which any probe's source lies outside the frame; 1.0 = all inside.
    ///
    /// ⚠ Probes the four corners and four edge midpoints, the set
    /// `Corrector::compute_autoscale` uses.
    fn overflow(&self, aspect: f32) -> f32 {
        let (ax, ay) = (aspect, 1.0);
        let probes = [
            (-ax, -ay),
            (ax, -ay),
            (-ax, ay),
            (ax, ay),
            (0.0, -ay),
            (0.0, ay),
            (-ax, 0.0),
            (ax, 0.0),
        ];
        let mut scale = 1.0f32;
        for p in probes {
            let s = self.source_of(p);
            if p.0.abs() > PROBE_EPS {
                scale = scale.max((s.0 / p.0).abs());
            }
            if p.1.abs() > PROBE_EPS {
                scale = scale.max((s.1 / p.1).abs());
            }
        }
        scale
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ASPECT: f32 = 1.5;

    /// ⚠ Probes with **leverage**. A projective error displaces a point in proportion to its
    /// distance from the axis, and M3.1c-A recorded a rotation sign error that survived because
    /// its probe sat at 0.34 of the half-width. These sit at the corners.
    #[test]
    fn the_homography_round_trips_at_the_corners() {
        let h = Homography::from_angles(0.3, -0.2, 1.2, 1.1);
        for p in [(-1.5, -1.0), (1.5, -1.0), (-1.5, 1.0), (1.5, 1.0)] {
            let back = h.source_of(h.output_of(p));
            assert!(
                (back.0 - p.0).abs() < 1e-4 && (back.1 - p.1).abs() < 1e-4,
                "{p:?} round-tripped to {back:?}"
            );
        }
    }

    /// ⚠⚠ **The test that matters, and it must not be a golden.** A golden only ever catches a
    /// *change*, and would bake a wrong convention into its reference on the first run — the
    /// lesson `the_tint_lands_at_the_hue_the_wheel_names` was added for.
    ///
    /// Two world-vertical lines converge to a vanishing point at `(0, y_v)`. The pitch that
    /// straightens them satisfies `tan θ = −f / y_v`, derived here independently rather than read
    /// back out of the implementation. After correction the two lines must be parallel.
    #[test]
    fn the_predicted_pitch_makes_converging_verticals_parallel() {
        let y_v = 4.0f32;
        let theta = (-F_NORM / y_v).atan();
        let h = Homography::from_angles(theta, 0.0, 1.0, 1.0);

        let vp = (0.0f32, y_v);
        let along =
            |base: (f32, f32), t: f32| (base.0 + (vp.0 - base.0) * t, base.1 + (vp.1 - base.1) * t);
        let dir = |base: (f32, f32)| {
            let a = h.output_of(along(base, 0.0));
            let b = h.output_of(along(base, 0.4));
            (b.0 - a.0, b.1 - a.1)
        };
        let (ax, ay) = dir((-1.0, -1.0));
        let (bx, by) = dir((1.0, -1.0));
        // Parallel ⟺ the 2-D cross product vanishes. Normalised, so the tolerance means an angle.
        let na = (ax * ax + ay * ay).sqrt();
        let nb = (bx * bx + by * by).sqrt();
        let cross = (ax / na) * (by / nb) - (ay / na) * (bx / nb);
        assert!(
            cross.abs() < 1e-3,
            "corrected verticals are not parallel: cross = {cross}"
        );
    }

    /// ⚠ The complement: without the correction those same lines genuinely do converge, or the
    /// test above is measuring nothing.
    #[test]
    fn the_uncorrected_lines_really_do_converge() {
        let h = Homography::from_angles(0.0, 0.0, 1.0, 1.0);
        let vp = (0.0f32, 4.0);
        let dir = |base: (f32, f32)| {
            let a = h.output_of(base);
            let b = h.output_of((
                base.0 + (vp.0 - base.0) * 0.4,
                base.1 + (vp.1 - base.1) * 0.4,
            ));
            (b.0 - a.0, b.1 - a.1)
        };
        let (ax, ay) = dir((-1.0, -1.0));
        let (bx, by) = dir((1.0, -1.0));
        let na = (ax * ax + ay * ay).sqrt();
        let nb = (bx * bx + by * by).sqrt();
        let cross = (ax / na) * (by / nb) - (ay / na) * (bx / nb);
        assert!(cross.abs() > 0.1, "fixture check: the lines must converge");
    }

    /// ⚠ Sign conventions, one assertion per axis. A test that only checks "something moved"
    /// cannot tell a sign error from a correct implementation.
    ///
    /// A positive pitch must move the **top** of the frame outward relative to the bottom — that
    /// is what un-leans a building photographed from below.
    #[test]
    fn a_positive_pitch_widens_the_top_relative_to_the_bottom() {
        let h = Homography::from_angles(0.3, 0.0, 1.0, 1.0);
        let top = h.output_of((1.0, -1.0)).0;
        let bottom = h.output_of((1.0, 1.0)).0;
        assert!(
            top > bottom,
            "top half-width {top} should exceed bottom {bottom}"
        );
    }

    /// The yaw equivalent, about the other axis.
    ///
    /// ⚠ Positive yaw expands the **right**, mirroring positive pitch expanding the top. An
    /// earlier version of this test asserted "left" and was simply wrong about the convention —
    /// measured, the right corner goes to |y| = 1.238 against the left's 0.907.
    #[test]
    fn a_positive_yaw_heightens_the_right_relative_to_the_left() {
        let h = Homography::from_angles(0.0, 0.3, 1.0, 1.0);
        let left = h.output_of((-1.0, 1.0)).1;
        let right = h.output_of((1.0, 1.0)).1;
        assert!(
            right > left,
            "right half-height {right} should exceed left {left}"
        );
    }

    /// ⚠ `diag(s, 1/s)` claims to be area-preserving; assert it rather than trusting the shape.
    #[test]
    fn aspect_stretches_one_axis_and_shrinks_the_other() {
        let h = Homography::from_angles(0.0, 0.0, 1.25, 1.0);
        let p = h.output_of((1.0, 1.0));
        assert!((p.0 - 1.25).abs() < 1e-5, "x stretched to {}", p.0);
        assert!((p.1 - 0.8).abs() < 1e-5, "y shrank to {}", p.1);
    }

    /// ⚠⚠ **Both halves.** That the autoscale bounds the frame is only meaningful alongside the
    /// fact that without it the frame really is empty at the corners.
    #[test]
    fn the_autoscale_pulls_every_probe_back_inside() {
        let p = Perspective {
            vertical: 60.0,
            ..Default::default()
        };
        let bare = Homography::unscaled(&p);
        let k = Homography::autoscale(&p, ASPECT);
        assert!(k > 1.0, "a strong keystone must need scaling, got {k}");

        let outside = |h: &Homography| {
            [
                (-ASPECT, -1.0),
                (ASPECT, -1.0),
                (-ASPECT, 1.0),
                (ASPECT, 1.0),
            ]
            .iter()
            .any(|&q| {
                let s = h.source_of(q);
                s.0.abs() > ASPECT + 1e-3 || s.1.abs() > 1.0 + 1e-3
            })
        };
        assert!(
            outside(&bare),
            "fixture check: uncorrected corners are empty"
        );
        assert!(
            !outside(&Homography::from_edits(&p, ASPECT)),
            "at scale 100 no corner may read outside the source"
        );
    }

    /// ⚠ Scale 0 must leave the gaps, or the slider's low end does nothing.
    ///
    /// ⚠ Asks about *any* corner, not a chosen one. A keystone pushes two corners out and pulls
    /// the other two in — an earlier version of this test probed the top-right, which under a
    /// positive Vertical is one of the two that comes **inward**, and so failed against
    /// perfectly correct code.
    #[test]
    fn scale_zero_leaves_the_corners_empty() {
        let p = Perspective {
            vertical: 60.0,
            scale: 0.0,
            ..Default::default()
        };
        let h = Homography::from_edits(&p, ASPECT);
        let any_outside = [
            (-ASPECT, -1.0),
            (ASPECT, -1.0),
            (-ASPECT, 1.0),
            (ASPECT, 1.0),
        ]
        .iter()
        .any(|&q| {
            let s = h.source_of(q);
            s.0.abs() > ASPECT + 1e-3 || s.1.abs() > 1.0 + 1e-3
        });
        assert!(
            any_outside,
            "at scale 0 some corner must still read outside"
        );
    }

    /// A neutral `Perspective` must be the identity, which is what lets the stage be skipped and
    /// all 47 goldens stay byte-identical.
    #[test]
    fn a_neutral_perspective_is_the_identity() {
        let h = Homography::from_edits(&Perspective::default(), ASPECT);
        for p in [(0.0, 0.0), (ASPECT, 1.0), (-ASPECT, -1.0), (0.7, -0.3)] {
            let q = h.source_of(p);
            assert!(
                (q.0 - p.0).abs() < 1e-5 && (q.1 - p.1).abs() < 1e-5,
                "{p:?} moved to {q:?}"
            );
        }
    }

    /// ⚠⚠ **The regression guard for the bug this file was written with.** A single ratio pass —
    /// what `Corrector::compute_autoscale` does, and what this originally copied — returns 1.97
    /// on this input where the true answer is 3.31. Scaling by 1.97 leaves the bottom corners
    /// still reading source y = 1.26, so the empty wedge the slider exists to remove would simply
    /// have stayed there. Sabotage: replace the loop body with a single `overflow` call.
    #[test]
    fn one_autoscale_pass_is_not_enough() {
        let p = Perspective {
            vertical: 60.0,
            ..Default::default()
        };
        let one_pass = Homography::unscaled(&p).overflow(ASPECT);
        let converged = Homography::autoscale(&p, ASPECT);
        assert!(
            converged > one_pass * 1.2,
            "converged {converged} should far exceed the single pass {one_pass}"
        );
    }

    /// ⚠ The cap is real and it bites at the extremes: at Vertical 100 the autoscale clamps at
    /// `MAX_AUTOSCALE` and the frame is **not** fully filled. Asserted rather than left to be
    /// discovered, because a corner that stays empty at scale 100 reads as a bug.
    #[test]
    fn an_extreme_keystone_clamps_and_does_not_fully_fill() {
        let p = Perspective {
            vertical: 100.0,
            ..Default::default()
        };
        assert_eq!(Homography::autoscale(&p, ASPECT), MAX_AUTOSCALE);
    }
}
