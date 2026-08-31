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

/// The pitch and yaw two vanishing points ask for, in radians.
///
/// ⚠ Both points are in **centred** coordinates — `x` in ±aspect, `y` in ±1, the space
/// [`super::Homography`] works in. [`solve`] converts for you; this is the bare maths.
///
/// ⚠⚠ The maths: our rotation's bottom row is `[−cθ·sφ, sθ, cθ·cφ]`, and a family becomes parallel
/// exactly when `(R · ray)_z = 0` for its vanishing-point ray `(x, y, f)`. Two families give two
/// equations in two unknowns, in closed form.
pub fn solve_angles(vert: Option<(f32, f32)>, horz: Option<(f32, f32)>) -> (f32, f32) {
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
    (pitch, yaw)
}

/// `tan θ = −f / v` — the one-family solution.
fn solve_one(v: f32, f: f32) -> f32 {
    if v.abs() < DET_EPS {
        return 0.0;
    }
    (-f / v).atan()
}

/// The rotation a set of guides asks for: `(pitch, yaw, roll)` in radians.
///
/// Guides are in `[0, 1]` frame coordinates; `aspect` is width over height.
///
/// ⚠⚠ **The conversion to centred coordinates happens HERE, and getting it wrong was a real bug.**
/// [`solve_angles`] assumes `x ∈ ±aspect`, `y ∈ ±1` with `f = F_NORM`, while a guide is stored in
/// `[0, 1]` — where the half-height is 0.5 rather than 1. Feeding one to the other put the
/// vanishing point in the wrong space and produced a correction that was merely *plausible*.
///
/// ⚠⚠ **The roll is measured from the guides' own directions, not from the vanishing point.**
/// They agree in exact arithmetic — a corrected line's direction *is* the corrected
/// vanishing-point ray — but the vanishing point of two nearly-parallel lines sits enormously far
/// away and moves wildly for a pixel of drawing error, so a roll taken from it is ill-conditioned
/// exactly when the correction is gentlest. Reported in use as the image coming out rotated so
/// that carefully drawn vertical guides ended up not vertical at all.
pub fn solve(vert: &[Guide], horz: &[Guide], aspect: f32) -> (f32, f32, f32) {
    let centred = |p: [f32; 2]| ((p[0] - 0.5) * 2.0 * aspect, (p[1] - 0.5) * 2.0);
    let vp = |gs: &[Guide]| -> Option<(f32, f32)> {
        // ⚠ Two are required: one line has no vanishing point at all.
        let (a, b) = (gs.first()?, gs.get(1)?);
        let to_c = |g: &Guide| Guide {
            from: [centred(g.from).0, centred(g.from).1],
            to: [centred(g.to).0, centred(g.to).1],
        };
        vanishing_point(&to_c(a), &to_c(b))
    };
    let (pitch, yaw) = solve_angles(vp(vert), vp(horz));

    // The transform the pitch and yaw alone produce; the guides' directions are measured through
    // it, which is what is left for the roll to remove.
    let h = super::Homography::from_angles(pitch, yaw, 1.0, 1.0);
    let mean_angle = |gs: &[Guide], vertical: bool| -> Option<f32> {
        // ⚠⚠ **Two, like everything else here** — even though a *single* line does determine the
        // roll on its own (levelling one line is exactly what a ruler tool does). Allowing it
        // would make one guide rotate the photo while two rotate *and* de-keystone it, so the
        // first line would do something and the second something else. One rule: two of a kind,
        // or nothing. Solving roll from one guide is a reasonable feature and deliberately not
        // smuggled in here.
        if gs.len() < 2 {
            return None;
        }
        let mut sum = 0.0f32;
        let mut n = 0u32;
        for g in gs {
            let a = h.output_of(centred(g.from));
            let b = h.output_of(centred(g.to));
            let (mut dx, mut dy) = (b.0 - a.0, b.1 - a.1);
            if dx * dx + dy * dy < DET_EPS {
                continue;
            }
            // ⚠⚠ A line has no direction, only an orientation: it is the same line drawn either
            // way. Without folding both into one half-plane, two guides traced in opposite
            // directions average to roughly a right angle — garbage.
            if (vertical && dy < 0.0) || (!vertical && dx < 0.0) {
                dx = -dx;
                dy = -dy;
            }
            sum += if vertical {
                dx.atan2(dy)
            } else {
                -dy.atan2(dx)
            };
            n += 1;
        }
        (n > 0).then(|| sum / n as f32)
    };
    // ⚠ Verticals first: if both families are drawn they agree, and if only one is drawn it is
    // the one that has an opinion.
    let roll = mean_angle(vert, true)
        .or_else(|| mean_angle(horz, false))
        .unwrap_or(0.0);
    // ⚠ **+roll, not −roll.** Rotating a direction by ψ takes its angle-from-vertical `a` to
    // `a − ψ`, so zeroing it needs ψ = a. The opposite sign doubles the lean instead of removing
    // it, and the test above is what settles it rather than this comment.
    (pitch, yaw, roll)
}

/// Where a guide point drawn on the **raw** frame appears once the correction is applied.
///
/// ⚠⚠ **This is what makes a live solve usable.** With the correction visible, a guide left at its
/// raw position would slide off the feature it was traced onto — so it is mapped through the same
/// transform the pixels take: perspective, then the straighten rotation. Both move together, so
/// the line stays on the edge.
///
/// Coordinates are `[0, 1]` across the frame both ways; `aspect` is width over height.
pub fn to_frame(p: [f32; 2], h: &super::Homography, angle_rad: f32, aspect: f32) -> [f32; 2] {
    // → centred, in units of the half-height, which is isotropic so the rotation is a plain one.
    let c = ((p[0] - 0.5) * 2.0 * aspect, (p[1] - 0.5) * 2.0);
    let q = h.output_of(c);
    // ⚠ `GeometryStage` inverse-maps with `-angle`, so the forward direction is `+angle`.
    let (sin, cos) = angle_rad.sin_cos();
    let r = (q.0 * cos - q.1 * sin, q.0 * sin + q.1 * cos);
    [r.0 / (2.0 * aspect) + 0.5, r.1 / 2.0 + 0.5]
}

/// The inverse of [`to_frame`] — where a point on the corrected frame came from on the raw one.
pub fn from_frame(p: [f32; 2], h: &super::Homography, angle_rad: f32, aspect: f32) -> [f32; 2] {
    let r = ((p[0] - 0.5) * 2.0 * aspect, (p[1] - 0.5) * 2.0);
    let (sin, cos) = (-angle_rad).sin_cos();
    let q = (r.0 * cos - r.1 * sin, r.0 * sin + r.1 * cos);
    let c = h.source_of(q);
    [c.0 / (2.0 * aspect) + 0.5, c.1 / 2.0 + 0.5]
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
        assert_eq!(solve_angles(None, None), (0.0, 0.0));
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
            let (pitch, yaw) = solve_angles(Some(vv), None);
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
        let (pitch, yaw) = solve_angles(Some(vv), None);
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

    /// ⚠⚠ **The two-family round trip, and the sabotage step is what forced it.** An earlier
    /// version only asserted that both angles were non-zero — which a solver with the *wrong
    /// numerator* passes happily, because a wrong angle is still an angle. Swapping `(b − d)` for
    /// `(a − c)` in the yaw formula was **not caught** by that test at all.
    ///
    /// This derives the vanishing points from a known rotation instead. Both families become
    /// parallel exactly when their ray satisfies `−cθ·sφ·x + sθ·y + cθ·cφ·f = 0` — a *line* of
    /// vanishing points, so any two distinct points on it are a valid `(vertical, horizontal)`
    /// pair. Solving them must return the angles that drew the line.
    #[test]
    fn the_solver_recovers_both_angles_from_the_vanishing_points_they_produce() {
        for (want_p, want_y) in [(0.25f32, -0.3f32), (-0.2, 0.35), (0.4, 0.15)] {
            let (sp, cp) = want_p.sin_cos();
            let (sy, cy) = want_y.sin_cos();
            // x on the vanishing line for a given y.
            let x_at = |y: f32| (sp * y + cp * cy * F_NORM) / (cp * sy);
            let (b, d) = (-4.0f32, 2.0f32);
            let (pitch, yaw) = solve_angles(Some((x_at(b), b)), Some((x_at(d), d)));
            assert!(
                (pitch - want_p).abs() < 1e-3,
                "wanted pitch {want_p}, got {pitch}"
            );
            assert!(
                (yaw - want_y).abs() < 1e-3,
                "wanted yaw {want_y}, got {yaw}"
            );
        }
    }

    /// ⚠ Both families together must engage **both** angles — kept alongside the round trip
    /// above because it states the weaker property in a form that reads at a glance.
    #[test]
    fn a_horizontal_family_engages_the_yaw() {
        let (pitch, yaw) = solve_angles(Some((0.5, -6.0)), Some((7.0, 0.4)));
        assert!(pitch.abs() > 1e-3, "pitch should be engaged, got {pitch}");
        assert!(yaw.abs() > 1e-3, "yaw should be engaged, got {yaw}");
    }

    /// ⚠⚠ **The property the whole feature exists for, and the one a user actually checks:**
    /// after solving, the lines you drew along the building's edges come out **vertical** — not
    /// merely parallel to each other. Reported in use as the image being rotated so that
    /// carefully drawn vertical guides ended up not vertical at all.
    ///
    /// ⚠ Asserted through `to_frame`, the same mapping the canvas draws with, so this measures
    /// what is on screen rather than an intermediate.
    #[test]
    fn the_guides_end_up_vertical_after_the_solve() {
        const ASPECT: f32 = 1.5;
        // A leaning, converging pair — off-centre, so a naive roll term has something to get
        // wrong, and asymmetric, so "it happened to be symmetric" cannot explain a pass.
        let gs = vec![g([0.18, 0.05], [0.30, 0.95]), g([0.55, 0.05], [0.52, 0.95])];
        let (pitch, yaw, roll) = solve(&gs, &[], ASPECT);
        let h = Homography::from_angles(pitch, yaw, 1.0, 1.0);
        for gu in &gs {
            let a = to_frame(gu.from, &h, roll, ASPECT);
            let b = to_frame(gu.to, &h, roll, ASPECT);
            // Vertical on screen means no horizontal run. In [0,1] frame units, scale x by the
            // aspect so the comparison is in the same units both ways.
            let (dx, dy) = ((b[0] - a[0]) * ASPECT, b[1] - a[1]);
            assert!(
                dx.abs() < 0.01 * dy.abs().max(1e-6),
                "guide {gu:?} came out at dx {dx}, dy {dy} — not vertical"
            );
        }
    }

    /// ⚠ The complement: without the correction those same guides are visibly *not* vertical, or
    /// the test above is measuring nothing.
    #[test]
    fn the_guides_are_not_vertical_before_the_solve() {
        const ASPECT: f32 = 1.5;
        let gu = g([0.18, 0.05], [0.30, 0.95]);
        let h = Homography::from_angles(0.0, 0.0, 1.0, 1.0);
        let a = to_frame(gu.from, &h, 0.0, ASPECT);
        let b = to_frame(gu.to, &h, 0.0, ASPECT);
        let (dx, dy) = ((b[0] - a[0]) * ASPECT, b[1] - a[1]);
        assert!(dx.abs() > 0.05 * dy.abs(), "fixture check: it must lean");
    }

    /// ⚠⚠ **A line has no direction, only an orientation.** Drawing the second guide bottom-to-top
    /// instead of top-to-bottom must not change the answer — without folding both into one
    /// half-plane before averaging, two guides traced in opposite directions average to roughly a
    /// right angle.
    #[test]
    fn drawing_a_guide_the_other_way_round_changes_nothing() {
        const ASPECT: f32 = 1.5;
        let a = g([0.18, 0.05], [0.30, 0.95]);
        let b = g([0.55, 0.05], [0.52, 0.95]);
        let flipped = g(b.to, b.from);
        let one = solve(&[a, b], &[], ASPECT);
        let two = solve(&[a, flipped], &[], ASPECT);
        assert!(
            (one.2 - two.2).abs() < 1e-4,
            "roll differed: {} vs {}",
            one.2,
            two.2
        );
    }

    /// ⚠ A symmetric pair needs no roll — the other half, since a roll stuck at zero and one that
    /// always fires are equally wrong.
    #[test]
    fn a_symmetric_pair_asks_for_no_roll() {
        let gs = vec![g([0.3, 0.05], [0.38, 0.95]), g([0.7, 0.05], [0.62, 0.95])];
        let (_, _, roll) = solve(&gs, &[], 1.5);
        assert!(roll.abs() < 1e-3, "a symmetric pair needs no roll: {roll}");
    }

    /// ⚠ The pair must be exact inverses, or a dragged endpoint drifts away from the cursor a
    /// little more on every frame. Probes with leverage — corners, not the centre.
    #[test]
    fn the_frame_mapping_round_trips() {
        let h = Homography::from_angles(0.25, -0.15, 1.1, 1.0);
        for p in [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0], [0.3, 0.8]] {
            let back = from_frame(to_frame(p, &h, 0.07, 1.5), &h, 0.07, 1.5);
            assert!(
                (back[0] - p[0]).abs() < 1e-4 && (back[1] - p[1]).abs() < 1e-4,
                "{p:?} round-tripped to {back:?}"
            );
        }
    }

    /// ⚠ A neutral transform must leave a point exactly where it is, or every guide shifts the
    /// instant the sub-mode opens.
    #[test]
    fn a_neutral_transform_moves_no_guide_point() {
        let h = Homography::from_angles(0.0, 0.0, 1.0, 1.0);
        for p in [[0.0, 0.0], [1.0, 1.0], [0.42, 0.17]] {
            let q = to_frame(p, &h, 0.0, 1.5);
            assert!(
                (q[0] - p[0]).abs() < 1e-5 && (q[1] - p[1]).abs() < 1e-5,
                "{p:?} moved to {q:?}"
            );
        }
    }
}
