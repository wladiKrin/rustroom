//! The compositional guides drawn inside the crop rectangle: thirds, the phi grid, the
//! diagonals, and a square-celled grid.
//!
//! ⚠⚠ **"Composition", never "guide" on its own.** `pipeline::perspective::guides` and
//! `develop/canvas/guide_overlay.rs` already own the word *guide* for the perspective lines you
//! trace onto a building, and the Crop panel already has a control called "Guide Mode". Two
//! unrelated things called the same thing in one panel is how a user learns the wrong model, so
//! everything here says *composition*.
//!
//! ⚠ **No egui.** Every segment is produced in the crop rectangle's own unit square, so the
//! divisions can be asserted exactly with no window open — which is the whole point, because a
//! guide that is drawn at 0.35 instead of 1/3 looks perfectly plausible on screen.

use serde::{Deserialize, Serialize};

/// A line segment in the crop rectangle's unit square: `[0, 0]` its top-left corner, `[1, 1]`
/// its bottom-right.
pub type Segment = ([f32; 2], [f32; 2]);

/// Which compositional overlay the crop rectangle carries.
///
/// ⚠ A deliberate **trim** of Lightroom's seven. The golden *spiral* is excluded because it has
/// four orientations and would need a second cycle key to reach them; the triangle because it is
/// rarely reached for. Both are additive later — they are extra variants and extra arms, and
/// nothing here would have to change shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum CompositionGuide {
    /// No overlay. ⚠ The default, and the reason `Default` may stay derived: it is the first
    /// variant. Move it and this must become a hand-written impl, the way
    /// `RadialGradient::default` and `ColorGrading::default` already are.
    #[default]
    Off,
    Thirds,
    GoldenRatio,
    Diagonals,
    Grid,
}

impl CompositionGuide {
    /// Every state the cycle key visits, in cycle order.
    ///
    /// ⚠ `Off` is **in** the list, not a sixth state outside it: one key that reaches every
    /// state including "none of them" is one rule, where a key that cycles four and a separate
    /// toggle that switches the overlay on is two rules that can disagree about what is showing.
    pub const ALL: [CompositionGuide; 5] = [
        Self::Off,
        Self::Thirds,
        Self::GoldenRatio,
        Self::Diagonals,
        Self::Grid,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Thirds => "Rule of thirds",
            Self::GoldenRatio => "Golden ratio",
            Self::Diagonals => "Diagonals",
            Self::Grid => "Grid",
        }
    }

    /// The next state the cycle key moves to, wrapping through `Off`.
    ///
    /// ⚠ Derived from `ALL` rather than written as its own `match`, so the cycle order and the
    /// dropdown's order cannot drift — the rule `FlagSet::ALL` and `EDIT_GROUP_NAMES` follow.
    pub fn next(self) -> Self {
        let i = Self::ALL.iter().position(|g| *g == self).unwrap_or(0);
        Self::ALL[(i + 1) % Self::ALL.len()]
    }
}

/// The golden section of a unit segment: `1/φ`, the longer part.
///
/// ⚠ Written out rather than computed from a `sqrt` so the constant is greppable, and pinned by
/// `the_golden_divisions_really_are_the_golden_section`, which checks the defining property
/// `x² = 1 − x` rather than re-stating this literal — a test that only compared against the same
/// number could not tell a typo from the truth.
const PHI_LONG: f32 = 0.618_034;
/// The shorter part, `1/φ²`. Symmetric with [`PHI_LONG`] about the centre, which is why the phi
/// grid needs **no mirrored variant** — unlike the golden spiral, which is genuinely chiral.
const PHI_SHORT: f32 = 1.0 - PHI_LONG;

/// How many cells the grid puts across the crop's **shorter** edge.
///
/// ⚠ Even, so the centre line falls on 0.5 — which is what makes the grid symmetric about both
/// axes and therefore usable for centring a subject as well as for levelling a horizon.
const GRID_CELLS_ON_SHORT_EDGE: u32 = 8;

/// The narrowest a grid step may be before that axis is dropped entirely.
///
/// ⚠⚠ **Load-bearing, and not a rounding guard.** The grid's cells are square, so the long axis
/// carries `aspect × GRID_CELLS_ON_SHORT_EDGE` of them — on a stitched panorama that is
/// unbounded. This caps the segment count at about a hundred per axis and degrades an extreme
/// crop to horizontal lines only, rather than emitting thousands of lines nobody could see
/// between. It is also what makes the loop below provably terminate.
const MIN_GRID_STEP: f32 = 0.01;

/// How close to the crop's own edge a division may fall before it is dropped.
///
/// ⚠ A division at 0 or 1 lies exactly under the crop border, which is already drawn — so it
/// adds nothing and, at a half-pixel offset, reads as the border being doubled.
const EDGE_EPS: f32 = 1e-4;

/// The guide's line segments in the crop rectangle's unit square.
///
/// `aspect` is the crop's **on-screen** width over its height, which is what makes the grid's
/// cells square. Every other guide ignores it: thirds, the phi grid and the diagonals are all
/// defined by *fractions of each edge*, and a fraction of an edge is aspect-independent.
///
/// ⚠ A degenerate `aspect` — zero, negative, infinite or NaN — yields no segments at all rather
/// than lines at infinity. The crop rect is measured from a live layout, so a frame in which it
/// has collapsed is reachable.
pub fn segments(guide: CompositionGuide, aspect: f32) -> Vec<Segment> {
    match guide {
        CompositionGuide::Off => Vec::new(),
        CompositionGuide::Thirds => axis_divisions(&[1.0 / 3.0, 2.0 / 3.0]),
        CompositionGuide::GoldenRatio => axis_divisions(&[PHI_SHORT, PHI_LONG]),
        // ⚠ Corner to corner, both ways. Lightroom's "Diagonal" is a different figure — four
        // lines at 45° from the corners, the *diagonal method* — which on a non-square crop does
        // not pass through the corners at all. This is the simpler and more widely understood
        // one, and it is what the brief asked for; the 45° variant is an extra arm if wanted.
        CompositionGuide::Diagonals => vec![([0.0, 0.0], [1.0, 1.0]), ([1.0, 0.0], [0.0, 1.0])],
        CompositionGuide::Grid => grid(aspect),
    }
}

/// Vertical lines at each `x` in `fracs` and horizontal lines at each `y`.
fn axis_divisions(fracs: &[f32]) -> Vec<Segment> {
    let mut out = Vec::with_capacity(fracs.len() * 2);
    for f in fracs {
        out.push(([*f, 0.0], [*f, 1.0]));
    }
    for f in fracs {
        out.push(([0.0, *f], [1.0, *f]));
    }
    out
}

/// A grid whose **cells are square**, centred on the crop.
///
/// ⚠⚠ Square cells are why this needs the aspect at all, and they are not decoration: a "regular
/// grid" whose cells are 16:9 is not regular, and a stretched cell makes it harder rather than
/// easier to judge whether something is upright. The short edge sets the cell size; the long edge
/// takes as many whole cells as fit, centred, so the leftover is split evenly at both ends
/// instead of leaving one ragged strip.
fn grid(aspect: f32) -> Vec<Segment> {
    if !aspect.is_finite() || aspect <= 0.0 {
        return Vec::new();
    }
    let step = 1.0 / GRID_CELLS_ON_SHORT_EDGE as f32;
    // The shorter edge gets `step`; the longer one gets the same *distance*, which is a smaller
    // fraction of itself.
    let (step_x, step_y) = if aspect >= 1.0 {
        (step / aspect, step)
    } else {
        (step, step * aspect)
    };
    let mut out = Vec::new();
    for x in centred_divisions(step_x) {
        out.push(([x, 0.0], [x, 1.0]));
    }
    for y in centred_divisions(step_y) {
        out.push(([0.0, y], [1.0, y]));
    }
    out
}

/// Divisions at `0.5` and every multiple of `step` either side of it, inside the unit interval.
///
/// ⚠ Includes the centre. With an even cell count on the short edge that *is* one of the grid
/// lines, and building outward from it is what keeps the figure symmetric on the long edge too.
fn centred_divisions(step: f32) -> Vec<f32> {
    // ⚠ The step guard is what bounds this loop: `d` grows by at least `MIN_GRID_STEP` each
    // time, so it reaches 0.5 in at most 50 iterations.
    if !step.is_finite() || step < MIN_GRID_STEP {
        return Vec::new();
    }
    let mut out = vec![0.5];
    let mut k = 1u32;
    loop {
        let d = k as f32 * step;
        if d >= 0.5 - EDGE_EPS {
            break;
        }
        out.push(0.5 - d);
        out.push(0.5 + d);
        k += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The x of every vertical segment, and the y of every horizontal one.
    fn verticals(segs: &[Segment]) -> Vec<f32> {
        let mut v: Vec<f32> = segs
            .iter()
            .filter(|(a, b)| (a[0] - b[0]).abs() < 1e-6)
            .map(|(a, _)| a[0])
            .collect();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v
    }

    fn horizontals(segs: &[Segment]) -> Vec<f32> {
        let mut h: Vec<f32> = segs
            .iter()
            .filter(|(a, b)| (a[1] - b[1]).abs() < 1e-6)
            .map(|(a, _)| a[1])
            .collect();
        h.sort_by(|a, b| a.partial_cmp(b).unwrap());
        h
    }

    #[test]
    fn off_draws_nothing() {
        assert!(segments(CompositionGuide::Off, 1.5).is_empty());
    }

    #[test]
    fn thirds_divides_at_exactly_a_third() {
        // ⚠ Asserted on the **coordinates**, not on the count. Four lines in a rectangle is true
        // of the phi grid too, and of a grid drawn at 0.35 / 0.65 — which would look entirely
        // plausible and be wrong. This is the leverage the count does not have.
        let s = segments(CompositionGuide::Thirds, 1.5);
        let v = verticals(&s);
        let h = horizontals(&s);
        assert_eq!(v.len(), 2, "two vertical divisions; got {v:?}");
        assert_eq!(h.len(), 2, "two horizontal divisions; got {h:?}");
        for got in [&v, &h] {
            assert!((got[0] - 1.0 / 3.0).abs() < 1e-5, "{got:?} is not a third");
            assert!(
                (got[1] - 2.0 / 3.0).abs() < 1e-5,
                "{got:?} is not two thirds"
            );
        }
        // And each line spans the whole crop, or a "third" would be a stub in the corner.
        for (a, b) in &s {
            let span = (a[0] - b[0]).abs().max((a[1] - b[1]).abs());
            assert!((span - 1.0).abs() < 1e-6, "({a:?},{b:?}) does not span");
        }
    }

    #[test]
    fn the_golden_divisions_really_are_the_golden_section() {
        // ⚠⚠ Checked by the **defining property**, `x² = 1 − x`, not against the same literal the
        // implementation uses — a test comparing `PHI_LONG` to `PHI_LONG` cannot tell a typo from
        // the truth. φ satisfies φ² = φ + 1, so 1/φ satisfies exactly this.
        let s = segments(CompositionGuide::GoldenRatio, 1.5);
        let v = verticals(&s);
        assert_eq!(v.len(), 2, "two vertical divisions; got {v:?}");
        let long = v[1];
        assert!(
            (long * long - (1.0 - long)).abs() < 1e-5,
            "{long} is not the golden section: x² should equal 1 − x"
        );
        assert!(
            (v[0] + v[1] - 1.0).abs() < 1e-5,
            "the two divisions must be mirror images; got {v:?}"
        );
        assert_eq!(v, horizontals(&s), "both axes take the same divisions");
    }

    #[test]
    fn the_golden_ratio_is_not_the_rule_of_thirds() {
        // ⚠ The discriminating half. 0.382 and 0.333 are only 0.049 apart, so a guide that
        // silently fell back to thirds would look right on screen and would be wrong. Without
        // this, a `GoldenRatio => axis_divisions(&[1./3., 2./3.])` typo passes everything above.
        let g = verticals(&segments(CompositionGuide::GoldenRatio, 1.5));
        let t = verticals(&segments(CompositionGuide::Thirds, 1.5));
        assert!(
            (g[0] - t[0]).abs() > 0.03,
            "the phi grid landed on the thirds: {g:?} vs {t:?}"
        );
    }

    #[test]
    fn the_diagonals_run_corner_to_corner() {
        let s = segments(CompositionGuide::Diagonals, 2.0);
        assert_eq!(s.len(), 2);
        // Both diagonals, in either stored order.
        let has = |p: [f32; 2], q: [f32; 2]| {
            s.iter()
                .any(|(a, b)| (*a == p && *b == q) || (*a == q && *b == p))
        };
        assert!(has([0.0, 0.0], [1.0, 1.0]), "no top-left → bottom-right");
        assert!(has([1.0, 0.0], [0.0, 1.0]), "no top-right → bottom-left");
    }

    #[test]
    fn the_grid_cells_are_square_on_a_wide_crop() {
        // ⚠⚠ Aspect 2.0, **not** 1.0. On a square crop scaling the step by the aspect changes
        // nothing at all, so the whole square-cell rule would be a no-op on that fixture — the
        // exact blind spot `a_dab_is_round_on_a_wide_frame` recorded. The cell's *screen* size is
        // its unit size times the edge length, so squareness is `step_x · aspect == step_y`.
        let aspect = 2.0;
        let s = segments(CompositionGuide::Grid, aspect);
        let v = verticals(&s);
        let h = horizontals(&s);
        assert!(
            v.len() > h.len(),
            "a wide crop needs more columns than rows"
        );
        let step_x = v[1] - v[0];
        let step_y = h[1] - h[0];
        assert!(
            (step_x * aspect - step_y).abs() < 1e-5,
            "cells are not square: step_x={step_x} step_y={step_y} aspect={aspect}"
        );
    }

    #[test]
    fn the_grid_cells_are_square_on_a_tall_crop() {
        // ⚠ The other branch of `if aspect >= 1.0`. A portrait crop is not the landscape case
        // reflected in the code — it is a different arm, and only a portrait fixture reaches it.
        let aspect = 0.5;
        let s = segments(CompositionGuide::Grid, aspect);
        let v = verticals(&s);
        let h = horizontals(&s);
        assert!(
            h.len() > v.len(),
            "a tall crop needs more rows than columns"
        );
        let step_x = v[1] - v[0];
        let step_y = h[1] - h[0];
        assert!(
            (step_x * aspect - step_y).abs() < 1e-5,
            "cells are not square: step_x={step_x} step_y={step_y} aspect={aspect}"
        );
    }

    #[test]
    fn the_grid_is_symmetric_about_the_centre() {
        // ⚠ Symmetry is what the centred construction buys, and an off-by-one that anchored the
        // grid at one edge would still produce evenly spaced lines — so spacing alone cannot see
        // it. Every division must have a partner mirrored across 0.5.
        for aspect in [1.0, 1.5, 0.6, 3.0] {
            let s = segments(CompositionGuide::Grid, aspect);
            for axis in [verticals(&s), horizontals(&s)] {
                for x in &axis {
                    let mirror = 1.0 - x;
                    assert!(
                        axis.iter().any(|y| (y - mirror).abs() < 1e-4),
                        "aspect {aspect}: {x} has no mirror in {axis:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_grid_has_a_line_down_the_middle() {
        // An even cell count on the short edge puts one there, and it is the line you centre a
        // subject against.
        let s = segments(CompositionGuide::Grid, 1.5);
        assert!(verticals(&s).iter().any(|x| (x - 0.5).abs() < 1e-5));
        assert!(horizontals(&s).iter().any(|y| (y - 0.5).abs() < 1e-5));
    }

    #[test]
    fn every_segment_stays_strictly_inside_the_crop() {
        // ⚠ Strictly: a division sitting *on* the crop border is already drawn by the border
        // itself, and at a half-pixel offset it reads as the border being doubled. Swept over
        // every guide and several aspects, because the off-by-one is a property of the loop
        // rather than of any one guide.
        for g in CompositionGuide::ALL {
            for aspect in [1.0, 1.5, 0.4, 2.35, 8.0] {
                for (a, b) in segments(g, aspect) {
                    for p in [a, b] {
                        assert!(
                            (0.0..=1.0).contains(&p[0]) && (0.0..=1.0).contains(&p[1]),
                            "{g:?} at {aspect}: {p:?} is outside the crop"
                        );
                    }
                    if g != CompositionGuide::Diagonals {
                        // The diagonals *are* corner lines; everything else must not touch an edge.
                        let on_edge = |v: f32| v.abs() < EDGE_EPS || (v - 1.0).abs() < EDGE_EPS;
                        let vertical = (a[0] - b[0]).abs() < 1e-6;
                        let along = if vertical { a[0] } else { a[1] };
                        assert!(
                            !on_edge(along),
                            "{g:?} at {aspect}: a division sits on the crop border at {along}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn a_degenerate_aspect_yields_no_segments_rather_than_infinities() {
        // ⚠ The crop rect is measured from a live layout, so a collapsed frame is reachable — and
        // a NaN coordinate would be painted as a line to nowhere rather than failing loudly.
        for aspect in [0.0, -2.0, f32::NAN, f32::INFINITY] {
            assert!(
                segments(CompositionGuide::Grid, aspect).is_empty(),
                "aspect {aspect} produced grid lines"
            );
            // The aspect-independent guides still work, and must still be finite.
            for g in [
                CompositionGuide::Thirds,
                CompositionGuide::GoldenRatio,
                CompositionGuide::Diagonals,
            ] {
                for (a, b) in segments(g, aspect) {
                    assert!(
                        a.iter().chain(b.iter()).all(|v| v.is_finite()),
                        "{g:?} at {aspect} emitted a non-finite point"
                    );
                }
            }
        }
    }

    #[test]
    fn an_extreme_aspect_drops_an_axis_rather_than_drawing_hundreds_of_lines() {
        // ⚠ `MIN_GRID_STEP` doing its job. Square cells on a 40:1 panorama would want ~320
        // columns; the guard drops them and leaves the rows, which are still useful for a
        // horizon. Without the guard this is unbounded in the crop's aspect.
        let s = segments(CompositionGuide::Grid, 40.0);
        assert!(verticals(&s).is_empty(), "the columns should have dropped");
        assert!(!horizontals(&s).is_empty(), "the rows must survive");
        assert!(s.len() < 200, "{} segments is too many", s.len());
    }

    #[test]
    fn cycling_visits_every_guide_once_and_returns_to_off() {
        // ⚠ Over `ALL`, so a sixth guide that the cycle key cannot reach is a failure rather than
        // a dropdown entry with no shortcut.
        let mut seen = vec![CompositionGuide::Off];
        let mut g = CompositionGuide::Off;
        for _ in 0..CompositionGuide::ALL.len() {
            g = g.next();
            if g != CompositionGuide::Off {
                seen.push(g);
            }
        }
        assert_eq!(g, CompositionGuide::Off, "the cycle must return to Off");
        seen.sort_by_key(|g| format!("{g:?}"));
        seen.dedup();
        assert_eq!(
            seen.len(),
            CompositionGuide::ALL.len(),
            "the cycle missed a guide: {seen:?}"
        );
    }

    #[test]
    fn every_guide_names_itself() {
        for g in CompositionGuide::ALL {
            assert!(!g.label().is_empty(), "{g:?} has no label");
        }
    }

    #[test]
    fn the_default_is_off() {
        // ⚠ Asserted as a **value**, not as "Default is derived". An overlay drawn over every
        // photograph by default is a change nobody asked for, and the derive only gives `Off`
        // while it is the first variant.
        assert_eq!(CompositionGuide::default(), CompositionGuide::Off);
    }
}
