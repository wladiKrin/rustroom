//! Pure crop-rectangle interaction math on `edit_model::CropRect` (normalized [0,1]).

use edit_model::CropRect;

const MIN_SIZE: f32 = 0.05; // minimum normalized crop extent

/// Which part of the crop rectangle a point is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
    Left,
    Right,
    Top,
    Bottom,
    Inside,
}

/// Which handle (if any) is under normalized point `p`, within `tol` (normalized).
pub fn hit_test(r: &CropRect, p: [f32; 2], tol: f32) -> Option<Handle> {
    let (x, y) = (p[0], p[1]);
    let near = |a: f32, b: f32| (a - b).abs() <= tol;
    let in_x = x >= r.left - tol && x <= r.right + tol;
    let in_y = y >= r.top - tol && y <= r.bottom + tol;
    if near(x, r.left) && near(y, r.top) {
        return Some(Handle::TopLeft);
    }
    if near(x, r.right) && near(y, r.top) {
        return Some(Handle::TopRight);
    }
    if near(x, r.left) && near(y, r.bottom) {
        return Some(Handle::BottomLeft);
    }
    if near(x, r.right) && near(y, r.bottom) {
        return Some(Handle::BottomRight);
    }
    if near(x, r.left) && in_y {
        return Some(Handle::Left);
    }
    if near(x, r.right) && in_y {
        return Some(Handle::Right);
    }
    if near(y, r.top) && in_x {
        return Some(Handle::Top);
    }
    if near(y, r.bottom) && in_x {
        return Some(Handle::Bottom);
    }
    if x > r.left && x < r.right && y > r.top && y < r.bottom {
        return Some(Handle::Inside);
    }
    None
}

/// Apply a normalized drag `(dx, dy)` to `handle`, clamped to [0,1] with a minimum size.
///
/// `aspect` is the **normalized** aspect the crop is locked to — `pixel_aspect / image_aspect`
/// — or `None` for a free drag. When it is `Some`, every handle keeps that ratio exactly, so
/// dragging a corner of a 1:1 crop produces a smaller 1:1 crop rather than an arbitrary one.
///
/// ⚠ The aspect must be *normalized*, not the pixel ratio. The rect lives in [0,1] on both
/// axes, so a 1:1 pixel crop is only a unit square in normalized space on a square image.
/// Passing the pixel ratio here silently produces the wrong shape on every non-square photo.
pub fn apply_drag(r: CropRect, handle: Handle, dx: f32, dy: f32, aspect: Option<f32>) -> CropRect {
    let free = drag_free(r, handle, dx, dy);
    match aspect {
        // ⚠ Guarded rather than trusted. A zero or non-finite aspect divides through into NaN
        // coordinates, and a NaN rect then compares false against every clamp it meets — the
        // crop would silently stop responding rather than fail loudly.
        Some(a) if a.is_finite() && a > 0.0 => constrain(r, free, handle, a),
        _ => free,
    }
}

/// The unconstrained drag: each edge follows the pointer, clamped to [0,1] and `MIN_SIZE`.
fn drag_free(r: CropRect, handle: Handle, dx: f32, dy: f32) -> CropRect {
    let mut out = r;
    match handle {
        Handle::Inside => {
            let w = out.right - out.left;
            let h = out.bottom - out.top;
            let nl = (out.left + dx).clamp(0.0, 1.0 - w);
            let nt = (out.top + dy).clamp(0.0, 1.0 - h);
            out.left = nl;
            out.right = nl + w;
            out.top = nt;
            out.bottom = nt + h;
        }
        _ => {
            if matches!(handle, Handle::Left | Handle::TopLeft | Handle::BottomLeft) {
                out.left = (out.left + dx).clamp(0.0, out.right - MIN_SIZE);
            }
            if matches!(
                handle,
                Handle::Right | Handle::TopRight | Handle::BottomRight
            ) {
                out.right = (out.right + dx).clamp(out.left + MIN_SIZE, 1.0);
            }
            if matches!(handle, Handle::Top | Handle::TopLeft | Handle::TopRight) {
                out.top = (out.top + dy).clamp(0.0, out.bottom - MIN_SIZE);
            }
            if matches!(
                handle,
                Handle::Bottom | Handle::BottomLeft | Handle::BottomRight
            ) {
                out.bottom = (out.bottom + dy).clamp(out.top + MIN_SIZE, 1.0);
            }
        }
    }
    out
}

/// Re-impose `a` on a dragged rect, anchoring whichever side the handle does not move.
///
/// ⚠ Anchors are the point of this: a corner holds the *opposite corner* still, an edge holds
/// the opposite edge and the perpendicular *centre*. Without that, a locked drag walks the
/// rectangle across the image instead of resizing it in place.
fn constrain(orig: CropRect, free: CropRect, handle: Handle, a: f32) -> CropRect {
    // A translation cannot change the ratio, so there is nothing to impose.
    if handle == Handle::Inside {
        return free;
    }
    let fw = free.right - free.left;
    let fh = free.bottom - free.top;

    let mut w = match handle {
        Handle::Left | Handle::Right => fw,
        Handle::Top | Handle::Bottom => fh * a,
        // ⚠ A corner drives from whichever axis *changed* most, compared in width units.
        //
        // Two wrong rules to avoid, both of which look reasonable: driving from width alone
        // makes a straight-down corner drag do nothing, and taking `fw.max(fh * a)` only ever
        // grows — so it ignores the drag entirely whenever you are shrinking, which is the
        // common case. `a_locked_corner_follows_a_purely_vertical_drag` caught the second.
        _ => {
            let (ow, oh) = (orig.right - orig.left, orig.bottom - orig.top);
            if (fw - ow).abs() >= ((fh - oh) * a).abs() {
                fw
            } else {
                fh * a
            }
        }
    };

    let (max_w, max_h) = room(orig, handle);
    w = w.min(max_w).min(max_h * a);
    // Both dimensions must clear MIN_SIZE, and the ratio fixes one from the other.
    w = w.max(MIN_SIZE).max(MIN_SIZE * a);
    let h = w / a;
    if w > max_w + 1e-4 || h > max_h + 1e-4 {
        // Even the smallest legal rect does not fit in the direction of travel. Refusing the
        // drag is better than emitting a rect that leaves the frame.
        return orig;
    }
    place(orig, handle, w, h)
}

/// How much room the rect has to grow into, in the direction this handle moves.
fn room(r: CropRect, handle: Handle) -> (f32, f32) {
    let cx = (r.left + r.right) / 2.0;
    let cy = (r.top + r.bottom) / 2.0;
    // An edge drag keeps the perpendicular centre fixed, so it can only grow to twice the
    // smaller distance from that centre to the frame.
    let span_x = 2.0 * cx.min(1.0 - cx);
    let span_y = 2.0 * cy.min(1.0 - cy);
    match handle {
        Handle::TopLeft => (r.right, r.bottom),
        Handle::TopRight => (1.0 - r.left, r.bottom),
        Handle::BottomLeft => (r.right, 1.0 - r.top),
        Handle::BottomRight => (1.0 - r.left, 1.0 - r.top),
        Handle::Left => (r.right, span_y),
        Handle::Right => (1.0 - r.left, span_y),
        Handle::Top => (span_x, r.bottom),
        Handle::Bottom => (span_x, 1.0 - r.top),
        Handle::Inside => (1.0, 1.0),
    }
}

/// Lay out a `w` x `h` rect against the side this handle does not move.
fn place(r: CropRect, handle: Handle, w: f32, h: f32) -> CropRect {
    let cx = (r.left + r.right) / 2.0;
    let cy = (r.top + r.bottom) / 2.0;
    let mut out = r;
    match handle {
        Handle::TopLeft => {
            out.left = r.right - w;
            out.top = r.bottom - h;
        }
        Handle::TopRight => {
            out.right = r.left + w;
            out.top = r.bottom - h;
        }
        Handle::BottomLeft => {
            out.left = r.right - w;
            out.bottom = r.top + h;
        }
        Handle::BottomRight => {
            out.right = r.left + w;
            out.bottom = r.top + h;
        }
        Handle::Left => {
            out.left = r.right - w;
            out.top = cy - h / 2.0;
            out.bottom = cy + h / 2.0;
        }
        Handle::Right => {
            out.right = r.left + w;
            out.top = cy - h / 2.0;
            out.bottom = cy + h / 2.0;
        }
        Handle::Top => {
            out.top = r.bottom - h;
            out.left = cx - w / 2.0;
            out.right = cx + w / 2.0;
        }
        Handle::Bottom => {
            out.bottom = r.top + h;
            out.left = cx - w / 2.0;
            out.right = cx + w / 2.0;
        }
        Handle::Inside => {}
    }
    out
}

/// Snap to a normalized aspect (width/height in normalized units), centered and maximal.
/// `norm_aspect` = pixel_aspect / image_aspect, where image_aspect = image_w / image_h.
pub fn snap_to_aspect(norm_aspect: f32) -> CropRect {
    let (w, h) = if norm_aspect >= 1.0 {
        (1.0, 1.0 / norm_aspect)
    } else {
        (norm_aspect, 1.0)
    };
    CropRect {
        left: (1.0 - w) / 2.0,
        top: (1.0 - h) / 2.0,
        right: (1.0 + w) / 2.0,
        bottom: (1.0 + h) / 2.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r() -> CropRect {
        CropRect {
            left: 0.2,
            top: 0.2,
            right: 0.8,
            bottom: 0.8,
        }
    }

    /// A free drag, so the pre-existing tests keep their assertions **verbatim** through the
    /// new `aspect` parameter — the same routing `mask_tool::apply_drag` used when it changed
    /// shape. Their passing is what says the free path did not move.
    fn dragged(rect: CropRect, handle: Handle, dx: f32, dy: f32) -> CropRect {
        apply_drag(rect, handle, dx, dy, None)
    }

    /// Width / height of a rect, in normalized units.
    fn ratio(c: &CropRect) -> f32 {
        (c.right - c.left) / (c.bottom - c.top)
    }

    fn inside_frame(c: &CropRect) -> bool {
        c.left >= -1e-4 && c.top >= -1e-4 && c.right <= 1.0 + 1e-4 && c.bottom <= 1.0 + 1e-4
    }

    #[test]
    fn hit_test_corner_and_inside() {
        assert_eq!(hit_test(&r(), [0.2, 0.2], 0.03), Some(Handle::TopLeft));
        assert_eq!(hit_test(&r(), [0.5, 0.5], 0.03), Some(Handle::Inside));
        assert_eq!(hit_test(&r(), [0.05, 0.05], 0.03), None);
    }

    #[test]
    fn drag_right_edge_grows() {
        let out = dragged(r(), Handle::Right, 0.1, 0.0);
        assert!((out.right - 0.9).abs() < 1e-6);
        assert!((out.left - 0.2).abs() < 1e-6);
    }

    #[test]
    fn drag_inside_clamps_within_unit() {
        let out = dragged(r(), Handle::Inside, 0.5, 0.0);
        assert!(out.right <= 1.0 + 1e-6);
        assert!((out.right - out.left - 0.6).abs() < 1e-6);
    }

    #[test]
    fn min_size_enforced() {
        let out = dragged(r(), Handle::Left, 1.0, 0.0);
        assert!(out.right - out.left >= MIN_SIZE - 1e-6);
    }

    #[test]
    fn snap_square_is_centered() {
        let sq = snap_to_aspect(1.0);
        assert!((sq.left - 0.0).abs() < 1e-6 && (sq.right - 1.0).abs() < 1e-6);
        let wide = snap_to_aspect(2.0);
        assert!((wide.bottom - wide.top - 0.5).abs() < 1e-6);
        assert!((wide.right - wide.left - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_locked_corner_drag_keeps_the_ratio() {
        // The headline behaviour: shrinking a 1:1 crop by its corner gives a smaller 1:1 crop.
        // ⚠ A *diagonal* drag, because a real pointer never moves on one axis alone — and a
        // zero component is exactly the blind spot that let a cross-axis leak pass unnoticed in
        // the radial-mask work.
        let out = apply_drag(r(), Handle::BottomRight, -0.15, -0.08, Some(1.0));
        assert!(
            (ratio(&out) - 1.0).abs() < 1e-4,
            "ratio drifted to {}",
            ratio(&out)
        );
        // The opposite corner is the anchor and must not have moved.
        assert!((out.left - 0.2).abs() < 1e-6);
        assert!((out.top - 0.2).abs() < 1e-6);
    }

    #[test]
    fn a_locked_corner_follows_a_purely_vertical_drag() {
        // ⚠ The discriminating case for "which axis drives the size". Driving from width alone
        // compiles, keeps the ratio, and does *nothing* when the pointer moves straight down —
        // a bug the ratio assertion above cannot see.
        let out = apply_drag(r(), Handle::BottomRight, 0.0, -0.2, Some(1.0));
        assert!(
            out.bottom < 0.8 - 1e-3,
            "a downward corner drag did not resize: bottom {}",
            out.bottom
        );
        assert!((ratio(&out) - 1.0).abs() < 1e-4);
    }

    #[test]
    fn a_locked_edge_drag_keeps_the_ratio_and_the_perpendicular_centre() {
        // An edge has only one axis of its own, so the other must grow about the centre —
        // otherwise the rect creeps up or down the frame as you widen it.
        let before = r();
        let cy = (before.top + before.bottom) / 2.0;
        let out = apply_drag(before, Handle::Right, -0.2, 0.0, Some(1.0));
        assert!((ratio(&out) - 1.0).abs() < 1e-4);
        assert!(
            ((out.top + out.bottom) / 2.0 - cy).abs() < 1e-4,
            "the rect drifted vertically"
        );
        assert!((out.left - 0.2).abs() < 1e-6, "the anchored edge moved");
    }

    #[test]
    fn a_locked_drag_stays_inside_the_frame() {
        // Dragging hard outward must bound the rect without breaking the ratio — the two
        // requirements pull against each other, which is why `room` exists.
        for (h, dx, dy) in [
            (Handle::BottomRight, 5.0, 5.0),
            (Handle::TopLeft, -5.0, -5.0),
            (Handle::Right, 5.0, 0.0),
            (Handle::Top, 0.0, -5.0),
        ] {
            let out = apply_drag(r(), h, dx, dy, Some(1.5));
            assert!(inside_frame(&out), "{h:?} left the frame: {out:?}");
            assert!(
                (ratio(&out) - 1.5).abs() < 1e-3,
                "{h:?} broke the ratio: {}",
                ratio(&out)
            );
        }
    }

    #[test]
    fn a_locked_drag_respects_the_minimum_size() {
        // Collapsing a locked crop must stop at MIN_SIZE on *both* axes, not just the driven
        // one — at an extreme ratio the derived axis is the one that vanishes first.
        for a in [1.0, 0.25, 4.0] {
            let out = apply_drag(r(), Handle::BottomRight, -5.0, -5.0, Some(a));
            assert!(
                out.right - out.left >= MIN_SIZE - 1e-4 && out.bottom - out.top >= MIN_SIZE - 1e-4,
                "aspect {a} collapsed to {out:?}"
            );
        }
    }

    #[test]
    fn a_locked_move_is_still_a_plain_translation() {
        // ⚠ `Inside` must skip the constraint entirely. Running it through `place` would
        // re-anchor the rect to a corner, so dragging the middle would jump it.
        let out = apply_drag(r(), Handle::Inside, 0.1, 0.05, Some(1.0));
        assert!((out.right - out.left - 0.6).abs() < 1e-6);
        assert!((out.left - 0.3).abs() < 1e-6);
        assert!((out.top - 0.25).abs() < 1e-6);
    }

    #[test]
    fn a_degenerate_aspect_is_ignored_rather_than_producing_nan() {
        // ⚠ `w / a` with a = 0 is inf, and inf - inf is NaN; a NaN rect then compares false
        // against every later clamp, so the crop would quietly stop responding. Guarded at the
        // door instead.
        for a in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            let out = apply_drag(r(), Handle::BottomRight, -0.1, -0.1, Some(a));
            assert!(
                out.left.is_finite()
                    && out.top.is_finite()
                    && out.right.is_finite()
                    && out.bottom.is_finite(),
                "aspect {a} produced {out:?}"
            );
        }
    }
}
