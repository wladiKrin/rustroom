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
pub fn apply_drag(r: CropRect, handle: Handle, dx: f32, dy: f32) -> CropRect {
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

    #[test]
    fn hit_test_corner_and_inside() {
        assert_eq!(hit_test(&r(), [0.2, 0.2], 0.03), Some(Handle::TopLeft));
        assert_eq!(hit_test(&r(), [0.5, 0.5], 0.03), Some(Handle::Inside));
        assert_eq!(hit_test(&r(), [0.05, 0.05], 0.03), None);
    }

    #[test]
    fn drag_right_edge_grows() {
        let out = apply_drag(r(), Handle::Right, 0.1, 0.0);
        assert!((out.right - 0.9).abs() < 1e-6);
        assert!((out.left - 0.2).abs() < 1e-6);
    }

    #[test]
    fn drag_inside_clamps_within_unit() {
        let out = apply_drag(r(), Handle::Inside, 0.5, 0.0);
        assert!(out.right <= 1.0 + 1e-6);
        assert!((out.right - out.left - 0.6).abs() < 1e-6);
    }

    #[test]
    fn min_size_enforced() {
        let out = apply_drag(r(), Handle::Left, 1.0, 0.0);
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
}
