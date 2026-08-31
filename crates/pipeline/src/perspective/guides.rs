//! Solving camera rotation from drawn guide lines — pure maths, no images.
//!
//! ⚠⚠ **lensfun's `mod_pc` is deliberately NOT used, and that reverses an earlier backlog note.**
//! It returns a 3×3 matrix built from its own three angles and its own *estimated* focal length.
//! Writing the answer into our sliders would mean decomposing that matrix back into our pitch and
//! yaw at our fixed [`super::F_NORM`] — not exactly representable, so the image would visibly
//! shift the moment the solve finished. Solving in our own model is exact by construction,
//! closed-form, and needs no SVD.

use super::F_NORM;

/// One drawn guide, in frame-normalised coordinates on the **raw** frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Guide {
    pub from: [f32; 2],
    pub to: [f32; 2],
}

/// Which family a guide belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// Should end up vertical on screen.
    Vertical,
    /// Should end up horizontal on screen.
    Horizontal,
}

/// Below this a determinant is treated as degenerate; it only guards a division.
const DET_EPS: f32 = 1e-6;

/// Beyond this a vanishing point is treated as being at infinity — that family is already
/// parallel, so it asks for no rotation about its axis.
const AT_INFINITY: f32 = 1e4;

impl Guide {
    /// ⚠ Inferred from the line rather than chosen from a menu: you draw along a building edge and
    /// it is obviously a vertical. `>=` puts an exact 45° line in `Vertical`, which is arbitrary
    /// but must be *decided* — the alternative is a line with no family at all.
    pub fn family(&self) -> Family {
        let dx = (self.to[0] - self.from[0]).abs();
        let dy = (self.to[1] - self.from[1]).abs();
        if dy >= dx {
            Family::Vertical
        } else {
            Family::Horizontal
        }
    }
}

/// Where two guides of one family meet — the family's vanishing point.
///
/// ⚠ `None` when they are parallel (or a guide is degenerate): the vanishing point is then at
/// infinity, which means **that family is already correct** and asks for no correction. Returning
/// `None` rather than a huge number keeps that case explicit at the call site.
pub fn vanishing_point(a: &Guide, b: &Guide) -> Option<(f32, f32)> {
    let (p, r) = (a.from, [a.to[0] - a.from[0], a.to[1] - a.from[1]]);
    let (q, s) = (b.from, [b.to[0] - b.from[0], b.to[1] - b.from[1]]);
    let det = r[0] * s[1] - r[1] * s[0];
    if det.abs() < DET_EPS {
        return None;
    }
    let t = ((q[0] - p[0]) * s[1] - (q[1] - p[1]) * s[0]) / det;
    let v = (p[0] + t * r[0], p[1] + t * r[1]);
    if !v.0.is_finite() || !v.1.is_finite() || v.0.abs() > AT_INFINITY || v.1.abs() > AT_INFINITY {
        return None;
    }
    Some(v)
}

/// The rotation a set of guides asks for: `(pitch, yaw, roll)` in radians.
///
/// ⚠⚠ The maths: our rotation's bottom row is `[−cθ·sφ, sθ, cθ·cφ]`, and a family becomes parallel
/// exactly when `(R · ray)_z = 0` for its vanishing-point ray `(x, y, f)`. Two families give two
/// equations in two unknowns, in closed form.
pub fn solve(vert: Option<(f32, f32)>, horz: Option<(f32, f32)>) -> (f32, f32, f32) {
    let f = F_NORM;
    let (pitch, yaw) = match (vert, horz) {
        (Some((a, b)), Some((c, d))) => {
            let denom = a * d - b * c;
            if denom.abs() < DET_EPS || (b - d).abs() < DET_EPS {
                // The two vanishing points are collinear with the centre, or level with each
                // other: the pair says nothing extra, so fall back to the vertical alone.
                (solve_one(b, f), 0.0)
            } else {
                let yaw = (-f * (b - d) / denom).atan();
                let pitch = (yaw.sin() * (a - c) / (b - d)).atan();
                (pitch, yaw)
            }
        }
        // ⚠ The single-family cases are the general one with the other angle zero, not special
        // cases bolted on: with φ = 0 the equation is `tθ·b + f = 0`, which is literally the
        // formula `the_predicted_pitch_makes_converging_verticals_parallel` already pins.
        (Some((_, b)), None) => (solve_one(b, f), 0.0),
        (None, Some((c, _))) => (0.0, -solve_one(c, f)),
        (None, None) => (0.0, 0.0),
    };
    (pitch, yaw, roll_for(vert, horz, pitch, yaw, f))
}

/// `tan θ = −f / v` — the one-family solution.
fn solve_one(v: f32, f: f32) -> f32 {
    if v.abs() < DET_EPS {
        return 0.0;
    }
    (-f / v).atan()
}

/// The residual roll, in radians, to hand to Straighten.
///
/// ⚠ After `R` the vanishing-point ray has `z = 0` by construction, so it is a direction *in the
/// image plane*. The angle between it and screen-vertical (or -horizontal) is what is left over,
/// and correcting it is exactly what the Straighten slider does — so guided levels the horizon
/// too, as Lightroom's Upright does.
fn roll_for(
    vert: Option<(f32, f32)>,
    horz: Option<(f32, f32)>,
    pitch: f32,
    yaw: f32,
    f: f32,
) -> f32 {
    let rotate = |r: (f32, f32, f32)| -> (f32, f32) {
        let (sp, cp) = pitch.sin_cos();
        let (sy, cy) = yaw.sin_cos();
        // R = R_pitch · R_yaw, applied to the ray.
        let (x, y, z) = (cy * r.0 + sy * r.2, r.1, -sy * r.0 + cy * r.2);
        (x, cp * y - sp * z)
    };
    if let Some((a, b)) = vert {
        let (x, y) = rotate((a, b, f));
        // Screen-vertical is (0, ±1); the residual is how far the direction leans off it.
        -x.atan2(y.abs().max(DET_EPS))
    } else if let Some((c, d)) = horz {
        let (x, y) = rotate((c, d, f));
        y.atan2(x.abs().max(DET_EPS))
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::perspective::Homography;

    fn g(from: [f32; 2], to: [f32; 2]) -> Guide {
        Guide { from, to }
    }

    /// ⚠ The only genuinely ambiguous input is an exact 45°, and it must land somewhere rather
    /// than nowhere.
    #[test]
    fn the_family_is_inferred_from_the_line_and_forty_five_degrees_is_vertical() {
        assert_eq!(g([0.0, 0.0], [0.1, 1.0]).family(), Family::Vertical);
        assert_eq!(g([0.0, 0.0], [1.0, 0.1]).family(), Family::Horizontal);
        assert_eq!(g([0.0, 0.0], [1.0, 1.0]).family(), Family::Vertical);
        // Direction must not matter: a line drawn the other way is the same line.
        assert_eq!(g([0.1, 1.0], [0.0, 0.0]).family(), Family::Vertical);
    }

    /// ⚠ Parallel guides mean that family is **already correct**, not an error and not a huge
    /// number. Returning `None` keeps that explicit at the call site.
    #[test]
    fn parallel_guides_have_no_vanishing_point() {
        assert_eq!(
            vanishing_point(&g([-1.0, -1.0], [-1.0, 1.0]), &g([1.0, -1.0], [1.0, 1.0])),
            None
        );
        assert_eq!(solve(None, None), (0.0, 0.0, 0.0));
    }

    #[test]
    fn two_converging_guides_meet_at_their_vanishing_point() {
        let v = vanishing_point(&g([-1.0, -1.0], [-0.5, 1.0]), &g([1.0, -1.0], [0.5, 1.0]))
            .expect("they converge");
        assert!(v.0.abs() < 1e-4, "should meet on the axis, got {v:?}");
        // ⚠ **Positive** y: these lines get closer as y increases, so they meet *above* the
        // frame. Parametrising the first, x = 0 at t = 2, where y = −1 + 4 = 3. An earlier
        // version of this assertion guessed −3 and failed against correct code.
        assert!((v.1 - 3.0).abs() < 1e-3, "got {v:?}");
    }

    /// ⚠⚠ **The test that matters.** Rather than re-deriving the algebra, this drives the solver
    /// with the vanishing point a *known* rotation produces and asserts it recovers that
    /// rotation — validating against the forward model the stage actually uses, which is the
    /// discipline `FrameMap` follows by testing against `GeometryStage` itself.
    ///
    /// ⚠ Probes with **leverage**: the angles are large enough that a sign error cannot hide.
    #[test]
    fn the_solver_recovers_the_pitch_that_produced_the_vanishing_point() {
        for want in [-0.35f32, -0.15, 0.2, 0.4] {
            // A world-vertical family under pitch `want` vanishes where (R·ray)_z = 0, i.e. at
            // y = −f / tan(want). Derived here, independently of the solver.
            let vv = (0.0, -F_NORM / want.tan());
            let (pitch, yaw, _) = solve(Some(vv), None);
            assert!(
                (pitch - want).abs() < 1e-3,
                "wanted pitch {want}, got {pitch}"
            );
            assert_eq!(yaw, 0.0, "a vertical family alone asks for no yaw");
        }
    }

    /// ⚠ The end-to-end property, through the real transform: the solved angles must actually make
    /// the guides' family parallel. This is what a golden could never check.
    #[test]
    fn the_solved_angles_really_do_make_the_family_parallel() {
        let a = g([-1.0, -1.0], [-0.4, 1.0]);
        let b = g([1.0, -1.0], [0.4, 1.0]);
        let vv = vanishing_point(&a, &b).expect("converging");
        let (pitch, yaw, _) = solve(Some(vv), None);
        let h = Homography::from_angles(pitch, yaw, 1.0, 1.0);
        let dir = |gu: &Guide| {
            let p = h.output_of((gu.from[0], gu.from[1]));
            let q = h.output_of((gu.to[0], gu.to[1]));
            let (dx, dy) = (q.0 - p.0, q.1 - p.1);
            let n = (dx * dx + dy * dy).sqrt();
            (dx / n, dy / n)
        };
        let (ax, ay) = dir(&a);
        let (bx, by) = dir(&b);
        let cross = ax * by - ay * bx;
        assert!(cross.abs() < 1e-3, "not parallel after solving: {cross}");
    }

    /// ⚠ Both families together must engage **both** angles. A solver that quietly ignored the
    /// horizontal family would still pass every test above.
    #[test]
    fn a_horizontal_family_engages_the_yaw() {
        let (pitch, yaw, _) = solve(Some((0.5, -6.0)), Some((7.0, 0.4)));
        assert!(pitch.abs() > 1e-3, "pitch should be engaged, got {pitch}");
        assert!(yaw.abs() > 1e-3, "yaw should be engaged, got {yaw}");
    }

    /// ⚠ A tilted family asks for roll; a symmetric one asks for none. Both halves, because a
    /// roll term stuck at zero and one that always fires are equally wrong.
    #[test]
    fn a_tilted_family_asks_for_roll_and_a_symmetric_one_does_not() {
        let straight = vanishing_point(&g([-1.0, -1.0], [-0.4, 1.0]), &g([1.0, -1.0], [0.4, 1.0]))
            .expect("converging");
        let (_, _, roll) = solve(Some(straight), None);
        assert!(
            roll.abs() < 1e-3,
            "a symmetric family needs no roll: {roll}"
        );

        let tilted = vanishing_point(&g([-1.0, -1.0], [0.2, 1.0]), &g([1.0, -1.0], [1.4, 1.0]))
            .expect("converging");
        let (_, _, roll) = solve(Some(tilted), None);
        assert!(roll.abs() > 1e-3, "a leaning family needs roll: {roll}");
    }
}
