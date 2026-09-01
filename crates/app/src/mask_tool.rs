//! Pure linear-gradient interaction math on `edit_model::LinearGradient` (normalized [0,1]).

use edit_model::{LinearGradient, Mask, RadialGradient};

/// Which edge of a radial mask's ellipse a handle sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

/// The smallest a radius may be dragged. Below this the five handles stack on one point and the
/// shape becomes ungrabbable — the same class of trap the linear gradient's frame clamp avoids.
pub const RADIUS_MIN: f32 = 0.01;
/// The largest. An ellipse twice the frame is a legitimate look; a fast drag producing 40 is
/// not.
pub const RADIUS_MAX: f32 = 2.0;

/// What a drag is moving.
///
/// ⚠ **One flat enum across every shape**, rather than `Handle { Linear(..), Radial(..) }`.
/// `Session.active_mask_handle` is a single field and must hold either, and nesting would not
/// make a mismatched (mask, handle) pair unrepresentable — it would only add a level. So the
/// simplest spelling wins, and `apply_drag`'s catch-all arm handles the pair that cannot occur.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    // Linear.
    From,
    To,
    /// The whole gradient — both endpoints move together.
    Line,
    // Radial.
    Center,
    /// One edge of the ellipse. Dragging it resizes about a fixed centre.
    Edge(Side),
}

/// Which handle (if any) is under normalized point `p`, within `tol`.
///
/// `aspect` is the frame's width divided by its height.
///
/// ⚠ **`tol` is in units of frame height, not normalized units** — unlike
/// [`crate::crop_tool::hit_test`], whose `tol` is normalized. The two are siblings with the
/// same shape and different units, so a tolerance copied from one to the other is wrong by a
/// factor of the aspect ratio on the horizontal axis. It has to be this way here: the whole
/// point of the aspect correction below is that distances are isotropic, and a tolerance
/// compared against them must share their metric.
///
/// ⚠ **Distances are measured in screen space, not normalized space** — the same requirement
/// `LinearGradient::coverage` has, for the same reason. Without it the grab region is
/// anisotropic: on a 3:1 frame you could grab the line from three times further away
/// horizontally than vertically. Working in units of frame height makes it round.
///
/// ⚠ Note this is the *opposite* of `RadialGradient::coverage`, which takes an aspect and
/// ignores it. Coverage of an axis-aligned ellipse survives the anisotropy; a distance does
/// not. Both facts are load-bearing and each is guarded by its own test.
///
/// Takes a `Mask` rather than a shape so the canvas's drag plumbing stays shape-agnostic and is
/// written once.
pub fn hit_test(m: &Mask, p: [f32; 2], aspect: f32, tol: f32) -> Option<Handle> {
    match m {
        Mask::Linear(g) => hit_test_linear(g, p, aspect, tol),
        Mask::Radial(r) => hit_test_radial(r, p, aspect, tol),
        // ⚠ A brush has no handles at all. A stroke is not a shape with grab points, which is
        // the observation that shapes 3.1c-B's canvas work: painting is the interaction, and
        // there is nothing to drag.
        Mask::Brush(_) => None,
        // ⚠ A luminance range has no handles for a stronger reason than the brush: it has no
        // *position* whatsoever. It selects tones, so nothing about it can be anywhere on the
        // canvas, and its two sliders are the whole of its interaction.
        Mask::Luminance(_) => None,
    }
}

/// Endpoints are tested before the line, so aiming at an end moves that end rather than the
/// whole gradient.
fn hit_test_linear(g: &LinearGradient, p: [f32; 2], aspect: f32, tol: f32) -> Option<Handle> {
    // Everything below is in units of frame height.
    let pt = (p[0] * aspect, p[1]);
    let a = (g.from[0] * aspect, g.from[1]);
    let b = (g.to[0] * aspect, g.to[1]);
    let dist = |q: (f32, f32)| ((pt.0 - q.0).powi(2) + (pt.1 - q.1).powi(2)).sqrt();

    if dist(a) <= tol {
        return Some(Handle::From);
    }
    if dist(b) <= tol {
        return Some(Handle::To);
    }

    // Distance to the segment, not the infinite line: past either end it is the endpoint
    // distance, which the two checks above already rejected.
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    if len2 <= f32::EPSILON {
        return None;
    }
    let t = (((pt.0 - a.0) * dx + (pt.1 - a.1) * dy) / len2).clamp(0.0, 1.0);
    let closest = (a.0 + t * dx, a.1 + t * dy);
    if dist(closest) <= tol {
        return Some(Handle::Line);
    }
    None
}

/// The centre is tested before the edges, so aiming at the middle of a small ellipse moves the
/// whole shape rather than resizing it — the same preference `hit_test_linear` gives endpoints
/// over the line.
fn hit_test_radial(r: &RadialGradient, p: [f32; 2], aspect: f32, tol: f32) -> Option<Handle> {
    // ⚠ In units of frame height, like the linear case and for the same reason: a tolerance
    // must share the metric of the distances it is compared against. Note this is the opposite
    // of `RadialGradient::coverage`, which needs no correction — coverage of an axis-aligned
    // ellipse survives the anisotropy, but a distance does not.
    let pt = (p[0] * aspect, p[1]);
    let c = (r.center[0] * aspect, r.center[1]);
    let dist = |q: (f32, f32)| ((pt.0 - q.0).powi(2) + (pt.1 - q.1).powi(2)).sqrt();

    if dist(c) <= tol {
        return Some(Handle::Center);
    }
    let (rx, ry) = (r.radius[0] * aspect, r.radius[1]);
    for (side, q) in [
        (Side::Left, (c.0 - rx, c.1)),
        (Side::Right, (c.0 + rx, c.1)),
        (Side::Top, (c.0, c.1 - ry)),
        (Side::Bottom, (c.0, c.1 + ry)),
    ] {
        if dist(q) <= tol {
            return Some(Handle::Edge(side));
        }
    }
    None
}

/// Apply a normalized drag `(dx, dy)` to `handle`, clamped to the frame.
///
/// ⚠ **In place, not `Mask -> Mask`.** A brush carries a `Vec<BrushStroke>`, so the by-value form
/// would clone an entire stroke list on every frame of every drag.
///
/// ⚠ Clamping matters: an endpoint dragged off the canvas would be unreachable afterwards.
pub fn apply_drag(m: &mut Mask, handle: Handle, dx: f32, dy: f32) {
    let mv = |p: [f32; 2]| [(p[0] + dx).clamp(0.0, 1.0), (p[1] + dy).clamp(0.0, 1.0)];
    match (m, handle) {
        (Mask::Linear(g), Handle::From) => g.from = mv(g.from),
        (Mask::Linear(g), Handle::To) => g.to = mv(g.to),
        (Mask::Linear(g), Handle::Line) => {
            g.from = mv(g.from);
            g.to = mv(g.to);
        }
        (Mask::Radial(r), Handle::Center) => r.center = mv(r.center),
        (Mask::Radial(r), Handle::Edge(side)) => resize_radial(r, side, dx, dy),
        // ⚠ A handle belonging to another shape. Unreachable — there is no UI to change a mask's
        // kind, let alone mid-drag — and doing nothing is the right failure for a UI.
        _ => {}
    }
}

/// Resize about a **fixed centre**: each edge grows away from the middle, which is what a marked
/// centre implies and what Lightroom does.
fn resize_radial(r: &mut RadialGradient, side: Side, dx: f32, dy: f32) {
    let (mut rx, mut ry) = (r.radius[0], r.radius[1]);
    match side {
        Side::Right => rx += dx,
        Side::Left => rx -= dx,
        Side::Bottom => ry += dy,
        Side::Top => ry -= dy,
    }
    r.radius = [
        rx.clamp(RADIUS_MIN, RADIUS_MAX),
        ry.clamp(RADIUS_MIN, RADIUS_MAX),
    ];
}

/// Where the selection lands after the mask at `removed` is deleted from a list of `len`.
///
/// ⚠ Deleting the selected mask clears the selection rather than moving it to a neighbour —
/// the same rule `Selection::clamp` follows for the Library, and for the same reason: silently
/// selecting a *different* thing is worse than selecting nothing, because the next slider drag
/// would then edit something the user never chose.
pub fn selection_after_delete(
    selected: Option<usize>,
    removed: usize,
    len: usize,
) -> Option<usize> {
    let sel = selected?;
    if sel >= len || removed >= len {
        return None;
    }
    match sel.cmp(&removed) {
        std::cmp::Ordering::Equal => None,
        std::cmp::Ordering::Greater => Some(sel - 1),
        std::cmp::Ordering::Less => Some(sel),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A gradient running left to right across the middle of a square frame.
    fn horizontal_gradient() -> LinearGradient {
        LinearGradient {
            from: [0.2, 0.5],
            to: [0.8, 0.5],
            invert: false,
        }
    }

    fn horizontal() -> Mask {
        Mask::Linear(horizontal_gradient())
    }

    /// Apply a drag to a copy and return it.
    ///
    /// ⚠ `apply_drag` mutates in place — a brush's stroke list must not be cloned on every drag
    /// frame — but these tests read better as expressions, and keeping their assertions
    /// *unchanged* is the regression argument for that signature change. Takes `&Mask` so the
    /// call sites do not churn again when `Mask` loses `Copy`.
    fn dragged(m: &Mask, handle: Handle, dx: f32, dy: f32) -> Mask {
        let mut out = m.clone();
        apply_drag(&mut out, handle, dx, dy);
        out
    }

    /// The gradient inside a mask, for tests that assert on endpoints.
    fn gradient_of(m: Mask) -> LinearGradient {
        match m {
            Mask::Linear(g) => g,
            other => panic!("expected a linear gradient, got {other:?}"),
        }
    }

    #[test]
    fn an_endpoint_is_grabbed_in_preference_to_the_line() {
        // The pointer is on the axis *and* on an endpoint. Moving the whole gradient when the
        // user aimed at an end would be the more annoying of the two mistakes.
        let g = horizontal();
        assert_eq!(hit_test(&g, [0.2, 0.5], 1.0, 0.04), Some(Handle::From));
        assert_eq!(hit_test(&g, [0.8, 0.5], 1.0, 0.04), Some(Handle::To));
    }

    #[test]
    fn the_line_is_grabbed_between_the_endpoints() {
        let g = horizontal();
        assert_eq!(hit_test(&g, [0.5, 0.5], 1.0, 0.04), Some(Handle::Line));
    }

    #[test]
    fn nothing_is_grabbed_far_from_the_gradient() {
        let g = horizontal();
        assert_eq!(hit_test(&g, [0.5, 0.9], 1.0, 0.04), None);
        assert_eq!(hit_test(&g, [0.5, 0.1], 1.0, 0.04), None);
    }

    #[test]
    fn hit_testing_the_line_is_aspect_correct() {
        // ⚠ The same non-square trap `LinearGradient::coverage` needed, in its third guise:
        // distance to the axis must be measured in screen space, or the grab region is
        // anisotropic — on a 3:1 frame you could grab the line from three times further away
        // horizontally than vertically.
        //
        // ⚠⚠ The axis here is **vertical** and the probe points are offset **horizontally**,
        // and that is the whole point. An earlier version used a horizontal axis with the
        // point directly below it — where the perpendicular distance is purely vertical, so
        // scaling x cannot change the answer. It passed with the correction removed, i.e. it
        // could not detect the very bug it names. A test is only as good as the property it
        // actually exercises.
        let g = Mask::Linear(LinearGradient {
            from: [0.5, 0.2],
            to: [0.5, 0.8],
            invert: false,
        });
        let aspect = 3.0;
        // 0.03 normalised to the side of a vertical axis is 0.09 frame *heights* away on a
        // 3:1 frame — outside a 0.04 tolerance. Without the correction it measures 0.03 and
        // is wrongly grabbed.
        assert_eq!(hit_test(&g, [0.53, 0.5], aspect, 0.04), None);
        // 0.01 normalised = 0.03 heights: inside the tolerance either way.
        assert_eq!(hit_test(&g, [0.51, 0.5], aspect, 0.04), Some(Handle::Line));
    }

    #[test]
    fn dragging_an_endpoint_leaves_the_other_alone() {
        let g = horizontal();
        let out = gradient_of(dragged(&g, Handle::From, 0.1, -0.2));
        assert_eq!(out.from, [0.3, 0.3]);
        assert_eq!(
            out.to,
            horizontal_gradient().to,
            "the far end must not move"
        );
    }

    #[test]
    fn dragging_the_line_moves_both_ends_equally() {
        // The gradient translates: its direction and length are unchanged, which is what
        // makes "reposition without reshaping" possible.
        //
        // ⚠ Tolerance, not `assert_eq!`: 0.8f32 + 0.1f32 is 0.9000000358, one ULP off the
        // 0.9f32 literal (0.8999999762) — the same class of trap the orientation work hit
        // ("1.0 - (1.0 - 0.1) is 0.100000024"). `crop_tool`'s own arithmetic-result tests use
        // the same 1e-6 tolerance for the same reason.
        let g = horizontal();
        let out = gradient_of(dragged(&g, Handle::Line, 0.1, 0.05));
        let close =
            |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6;
        assert!(close(out.from, [0.3, 0.55]), "{:?}", out.from);
        assert!(close(out.to, [0.9, 0.55]), "{:?}", out.to);
    }

    #[test]
    fn dragging_clamps_to_the_frame() {
        // An endpoint dragged off the canvas would be unreachable afterwards.
        let g = horizontal();
        let out = gradient_of(dragged(&g, Handle::To, 5.0, 5.0));
        assert_eq!(out.to, [1.0, 1.0]);
    }

    #[test]
    fn invert_survives_a_drag() {
        // It is not a positional property, so no drag should disturb it.
        let g = Mask::Linear(LinearGradient {
            invert: true,
            ..horizontal_gradient()
        });
        assert!(gradient_of(dragged(&g, Handle::Line, 0.1, 0.1)).invert);
        assert!(gradient_of(dragged(&g, Handle::From, 0.1, 0.1)).invert);
    }

    #[test]
    fn deleting_a_mask_keeps_the_selection_valid() {
        // Deleting *before* the selection shifts it down; deleting the selection itself drops
        // it; deleting after leaves it alone. The same class of rule `Selection::clamp`
        // already owns for the Library grid.
        assert_eq!(selection_after_delete(Some(2), 0, 3), Some(1));
        assert_eq!(selection_after_delete(Some(2), 2, 3), None);
        assert_eq!(selection_after_delete(Some(0), 2, 3), Some(0));
        assert_eq!(selection_after_delete(None, 1, 3), None);
        // ⚠ The out-of-range guard is real, reachable logic, not just the `sel?` early
        // return — a regression that dropped it or flipped its comparison would pass every
        // assertion above.
        assert_eq!(
            selection_after_delete(Some(5), 1, 3),
            None,
            "selection past the end"
        );
        assert_eq!(
            selection_after_delete(Some(1), 5, 3),
            None,
            "removed past the end"
        );
    }

    #[test]
    fn deleting_the_last_mask_clears_the_selection() {
        assert_eq!(selection_after_delete(Some(0), 0, 1), None);
    }

    #[test]
    fn a_zero_length_gradient_grabs_nothing_away_from_its_point() {
        // ⚠ Both endpoints equal: there is no axis to project onto, and `len2` is exactly
        // zero. Correct by inspection — the endpoint checks catch anything near the shared
        // point and the guard returns `None` rather than dividing — but "correct by
        // inspection" is what a test is for. A NaN escaping here would reach the canvas as a
        // handle that can never be grabbed again.
        let g = Mask::Linear(LinearGradient {
            from: [0.5, 0.5],
            to: [0.5, 0.5],
            invert: false,
        });
        assert_eq!(hit_test(&g, [0.1, 0.1], 1.0, 0.04), None);
        assert_eq!(hit_test(&g, [0.5, 0.5], 1.0, 0.04), Some(Handle::From));
    }

    /// An ellipse in the middle of a square frame.
    fn ellipse() -> RadialGradient {
        RadialGradient {
            center: [0.5, 0.5],
            radius: [0.25, 0.2],
            feather: 50.0,
            invert: false,
        }
    }

    fn radial_of(m: Mask) -> RadialGradient {
        match m {
            Mask::Radial(r) => r,
            other => panic!("expected a radial, got {other:?}"),
        }
    }

    #[test]
    fn the_centre_is_grabbed_in_preference_to_an_edge() {
        let m = Mask::Radial(ellipse());
        assert_eq!(hit_test(&m, [0.5, 0.5], 1.0, 0.04), Some(Handle::Center));
        assert_eq!(
            hit_test(&m, [0.75, 0.5], 1.0, 0.04),
            Some(Handle::Edge(Side::Right))
        );
        assert_eq!(
            hit_test(&m, [0.25, 0.5], 1.0, 0.04),
            Some(Handle::Edge(Side::Left))
        );
        assert_eq!(
            hit_test(&m, [0.5, 0.3], 1.0, 0.04),
            Some(Handle::Edge(Side::Top))
        );
        assert_eq!(
            hit_test(&m, [0.5, 0.7], 1.0, 0.04),
            Some(Handle::Edge(Side::Bottom))
        );
        assert_eq!(hit_test(&m, [0.9, 0.9], 1.0, 0.04), None);
    }

    #[test]
    fn a_tiny_ellipse_gives_the_centre_to_the_pointer_over_its_edges() {
        // ⚠ The discriminating case for the ordering above: with a radius inside the
        // tolerance, the centre and all four edge handles are simultaneously in range.
        // Reordering the checks changes the answer here and nowhere else.
        let m = Mask::Radial(RadialGradient {
            radius: [0.01, 0.01],
            ..ellipse()
        });
        assert_eq!(hit_test(&m, [0.5, 0.5], 1.0, 0.04), Some(Handle::Center));
    }

    #[test]
    fn hit_testing_an_edge_handle_is_aspect_correct() {
        // ⚠⚠ Designed against the 3.1a lesson. The probe is offset along the axis where the
        // scaling actually changes the answer: the **top** handle, probed **horizontally**, on
        // a 3:1 frame. A vertical offset from the same handle would be untouched by the
        // scaling and would pass with the correction deleted — the exact mistake
        // `hit_testing_the_line_is_aspect_correct` originally made.
        let m = Mask::Radial(RadialGradient {
            center: [0.5, 0.5],
            radius: [0.1, 0.2],
            ..ellipse()
        });
        let aspect = 3.0;
        // The top handle sits at normalised (0.5, 0.3). A 0.03 normalised offset in x is 0.09
        // frame *heights* on a 3:1 frame — outside a 0.04 tolerance. Without the correction it
        // measures 0.03 and is wrongly grabbed.
        assert_eq!(hit_test(&m, [0.53, 0.3], aspect, 0.04), None);
        // 0.01 normalised = 0.03 heights: inside the tolerance either way.
        assert_eq!(
            hit_test(&m, [0.51, 0.3], aspect, 0.04),
            Some(Handle::Edge(Side::Top))
        );
    }

    #[test]
    fn dragging_an_edge_resizes_about_a_fixed_centre() {
        // Lightroom's behaviour, and what a marked centre implies. Each side grows away from
        // the centre, so the left edge widens the ellipse on a negative dx.
        //
        // ⚠⚠ **Every drag here is diagonal, and that is the whole point.** An earlier version
        // dragged the horizontal edges with `dy = 0.0` and the vertical ones with `dx = 0.0`,
        // which makes a cross-axis leak invisible — `ry += dy` with `dy == 0` is a no-op. It
        // passed with `Side::Right` sabotaged to change *both* radii. A real mouse drag
        // essentially never has a zero component, so that was a live bug class the test could
        // not see. Found by running the sabotage step; the same species as the FEATHER_EPS
        // guard in `edit-model`.
        //
        // A horizontal edge constrains x only and must discard the drag's y entirely.
        let m = Mask::Radial(ellipse());
        let close = |a: f32, b: f32| (a - b).abs() < 1e-6;

        let right = radial_of(dragged(&m, Handle::Edge(Side::Right), 0.1, 0.07));
        assert!(close(right.radius[0], 0.35), "{:?}", right.radius);
        assert!(
            close(right.radius[1], 0.2),
            "a horizontal edge must discard dy: {:?}",
            right.radius
        );
        assert_eq!(right.center, [0.5, 0.5], "the centre must not move");

        let left = radial_of(dragged(&m, Handle::Edge(Side::Left), -0.1, -0.07));
        assert!(close(left.radius[0], 0.35), "{:?}", left.radius);
        assert!(
            close(left.radius[1], 0.2),
            "a horizontal edge must discard dy: {:?}",
            left.radius
        );
        assert_eq!(left.center, [0.5, 0.5]);

        let bottom = radial_of(dragged(&m, Handle::Edge(Side::Bottom), 0.09, 0.05));
        assert!(close(bottom.radius[1], 0.25), "{:?}", bottom.radius);
        assert!(
            close(bottom.radius[0], 0.25),
            "a vertical edge must discard dx: {:?}",
            bottom.radius
        );
        assert_eq!(bottom.center, [0.5, 0.5]);

        let top = radial_of(dragged(&m, Handle::Edge(Side::Top), -0.09, -0.05));
        assert!(close(top.radius[1], 0.25), "{:?}", top.radius);
        assert!(
            close(top.radius[0], 0.25),
            "a vertical edge must discard dx: {:?}",
            top.radius
        );
    }

    #[test]
    fn dragging_the_centre_leaves_the_radii_alone() {
        let m = Mask::Radial(ellipse());
        let out = radial_of(dragged(&m, Handle::Center, 0.1, -0.2));
        let close =
            |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6;
        assert!(close(out.center, [0.6, 0.3]), "{:?}", out.center);
        assert!(close(out.radius, ellipse().radius), "{:?}", out.radius);
    }

    #[test]
    fn the_radial_centre_clamps_to_the_frame() {
        // A centre dragged off the canvas would be unreachable afterwards — the same rule the
        // linear gradient's endpoints follow.
        let m = Mask::Radial(ellipse());
        assert_eq!(
            radial_of(dragged(&m, Handle::Center, 5.0, 5.0)).center,
            [1.0, 1.0]
        );
        assert_eq!(
            radial_of(dragged(&m, Handle::Center, -5.0, -5.0)).center,
            [0.0, 0.0]
        );
    }

    #[test]
    fn radii_clamp_to_a_usable_range() {
        // ⚠ The minimum is what keeps the ellipse grabbable: a zero-radius shape has all five
        // handles stacked on one point and is inert. The maximum stops a fast drag producing
        // an absurd value while still allowing an ellipse twice the frame, a legitimate look.
        let m = Mask::Radial(ellipse());
        let shrunk = radial_of(dragged(&m, Handle::Edge(Side::Right), -5.0, 0.0));
        assert_eq!(shrunk.radius[0], RADIUS_MIN);
        let grown = radial_of(dragged(&m, Handle::Edge(Side::Bottom), 0.0, 50.0));
        assert_eq!(grown.radius[1], RADIUS_MAX);
    }

    #[test]
    fn radial_invert_survives_a_drag() {
        let m = Mask::Radial(RadialGradient {
            invert: true,
            ..ellipse()
        });
        assert!(radial_of(dragged(&m, Handle::Center, 0.1, 0.1)).invert);
        assert!(radial_of(dragged(&m, Handle::Edge(Side::Right), 0.1, 0.0)).invert);
    }

    #[test]
    fn radial_feather_survives_a_drag() {
        // Like `invert`, not a positional property.
        let m = Mask::Radial(RadialGradient {
            feather: 17.0,
            ..ellipse()
        });
        assert_eq!(
            radial_of(dragged(&m, Handle::Center, 0.1, 0.1)).feather,
            17.0
        );
    }

    #[test]
    fn a_mismatched_handle_leaves_the_mask_unchanged() {
        // ⚠ Unreachable in the app — there is no UI to change a mask's kind, let alone
        // mid-drag — but the flat `Handle` enum admits the pair, so the degradation is worth
        // pinning. Doing nothing is the right failure for a UI; panicking is not.
        let radial = Mask::Radial(ellipse());
        assert_eq!(dragged(&radial, Handle::Line, 0.1, 0.1), radial);
        let linear = horizontal();
        assert_eq!(dragged(&linear, Handle::Center, 0.1, 0.1), linear);
    }
}
