//! Pure linear-gradient interaction math on `edit_model::LinearGradient` (normalized [0,1]).

use edit_model::LinearGradient;

/// What a drag is moving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    From,
    To,
    /// The whole gradient — both endpoints move together.
    Line,
}

/// Which handle (if any) is under normalized point `p`, within `tol`.
///
/// `aspect` is the frame's width divided by its height.
///
/// ⚠ **Distances are measured in screen space, not normalized space** — the same requirement
/// `LinearGradient::coverage` has, for the same reason. Without it the grab region is
/// anisotropic: on a 3:1 frame you could grab the line from three times further away
/// horizontally than vertically. Working in units of frame height makes it round.
///
/// Endpoints are tested before the line, so aiming at an end moves that end rather than the
/// whole gradient.
pub fn hit_test(g: &LinearGradient, p: [f32; 2], aspect: f32, tol: f32) -> Option<Handle> {
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

/// Apply a normalized drag `(dx, dy)` to `handle`, clamped to the frame.
///
/// ⚠ Clamping matters: an endpoint dragged off the canvas would be unreachable afterwards.
pub fn apply_drag(g: LinearGradient, handle: Handle, dx: f32, dy: f32) -> LinearGradient {
    let mv = |p: [f32; 2]| [(p[0] + dx).clamp(0.0, 1.0), (p[1] + dy).clamp(0.0, 1.0)];
    let mut out = g;
    match handle {
        Handle::From => out.from = mv(g.from),
        Handle::To => out.to = mv(g.to),
        Handle::Line => {
            out.from = mv(g.from);
            out.to = mv(g.to);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A gradient running left to right across the middle of a square frame.
    fn horizontal() -> LinearGradient {
        LinearGradient {
            from: [0.2, 0.5],
            to: [0.8, 0.5],
            invert: false,
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
        // ⚠ The same non-square trap `LinearGradient::coverage` needed, in its third guise.
        // On a 3:1 frame a point 0.05 *normalised* below a horizontal axis is only 0.05 frame
        // *heights* away — but a point 0.05 normalised to the side is 0.15 heights away.
        // Distance to the axis must be measured in screen space or the grab region is
        // anisotropic: generous vertically, mean horizontally.
        let g = LinearGradient {
            from: [0.2, 0.5],
            to: [0.8, 0.5],
            invert: false,
        };
        let aspect = 3.0;
        // 0.06 normalised below the axis = 0.06 heights: outside a 0.04 tolerance.
        assert_eq!(hit_test(&g, [0.5, 0.56], aspect, 0.04), None);
        // 0.03 normalised below = 0.03 heights: inside it.
        assert_eq!(hit_test(&g, [0.5, 0.53], aspect, 0.04), Some(Handle::Line));
    }

    #[test]
    fn dragging_an_endpoint_leaves_the_other_alone() {
        let g = horizontal();
        let out = apply_drag(g, Handle::From, 0.1, -0.2);
        assert_eq!(out.from, [0.3, 0.3]);
        assert_eq!(out.to, g.to, "the far end must not move");
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
        let out = apply_drag(g, Handle::Line, 0.1, 0.05);
        let close =
            |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6;
        assert!(close(out.from, [0.3, 0.55]), "{:?}", out.from);
        assert!(close(out.to, [0.9, 0.55]), "{:?}", out.to);
    }

    #[test]
    fn dragging_clamps_to_the_frame() {
        // An endpoint dragged off the canvas would be unreachable afterwards.
        let g = horizontal();
        let out = apply_drag(g, Handle::To, 5.0, 5.0);
        assert_eq!(out.to, [1.0, 1.0]);
    }

    #[test]
    fn invert_survives_a_drag() {
        // It is not a positional property, so no drag should disturb it.
        let g = LinearGradient {
            invert: true,
            ..horizontal()
        };
        assert!(apply_drag(g, Handle::Line, 0.1, 0.1).invert);
        assert!(apply_drag(g, Handle::From, 0.1, 0.1).invert);
    }
}
