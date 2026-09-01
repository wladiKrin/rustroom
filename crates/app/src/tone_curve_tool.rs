//! Pure tone-curve interaction math on `edit_model::ToneCurve`'s control points.
//!
//! The `crop_tool` / `mask_tool` / `panel_order` pattern: every rule about hit-testing,
//! dragging, inserting and removing a control point lives here, taking plain types, so it is
//! testable with no window in existence. The bin module beside it only draws and wires egui
//! events to these functions.
//!
//! ⚠⚠ **THE WIDGET'S AXES ARE DISPLAY-REFERRED; THE STORED POINTS ARE NOT.** `ToneCurveStage`
//! runs in the pipeline's linear working space, so a control point's `x` is a *linear light*
//! value — and mid-grey is linear 0.214, not 0.5. Drawn on linear axes the entire shadow range
//! (sRGB 0 to 0.25) occupies the leftmost **5%** of the widget, which is not a control anyone
//! can place a point in. So every position that crosses this module's boundary — a pointer
//! position in, a plotted sample out — is in **display space**, warped by the sRGB transfer
//! function, and converted at the boundary. The stored values are untouched, and the curve is
//! *sampled through the real stage* rather than redrawn, so what is plotted is exactly the
//! function the pipeline applies.
//!
//! ⚠ The cost, stated: the interpolation happens in linear x, so the smoothing between two
//! knots is not symmetric on screen. The drawn curve is still exactly the true function — it
//! is honest, not approximated — but a spline that looks evenly rounded on screen would need
//! the stage to interpolate in display space, which changes what an existing sidecar means.

use pipeline::ToneCurveStage;

/// What an empty stored curve means, and what the widget materialises on the first edit.
pub const IDENTITY: [[f32; 2]; 2] = [[0.0, 0.0], [1.0, 1.0]];

/// The narrowest gap two neighbouring control points may have, in **display** x.
///
/// ⚠ Display rather than linear, because it exists to keep two points separable *by the eye
/// and the pointer*. In linear x the same gap would be invisibly tight in the shadows and
/// enormous in the highlights.
const MIN_GAP: f32 = 0.02;

/// A stored (linear) value as a widget coordinate.
pub fn to_display(v: f32) -> f32 {
    color::linear_to_srgb(v.clamp(0.0, 1.0))
}

/// A widget coordinate as a stored (linear) value.
pub fn from_display(u: f32) -> f32 {
    color::srgb_to_linear(u.clamp(0.0, 1.0))
}

/// The points to edit: the stored list, or the identity when it is empty.
pub fn editable(points: &[[f32; 2]]) -> Vec<[f32; 2]> {
    if points.len() < 2 {
        IDENTITY.to_vec()
    } else {
        points.to_vec()
    }
}

/// Whether `points` is exactly the identity, so it can be stored as an empty list.
///
/// ⚠ Storing empty rather than `[[0,0],[1,1]]` is what keeps `Edits::default()` neutral and
/// keeps `ToneCurveStage` out of the pipeline entirely on an unedited photo — `build_pipeline`
/// tests `points.is_empty()`.
pub fn is_identity(points: &[[f32; 2]]) -> bool {
    points.len() == 2 && points[0] == IDENTITY[0] && points[1] == IDENTITY[1]
}

/// The index of the control point nearest display-space `p`, within `tol` display units.
pub fn hit(points: &[[f32; 2]], p: [f32; 2], tol: f32) -> Option<usize> {
    let mut best: Option<(usize, f32)> = None;
    for (i, q) in points.iter().enumerate() {
        let (dx, dy) = (to_display(q[0]) - p[0], to_display(q[1]) - p[1]);
        let d2 = dx * dx + dy * dy;
        if d2 <= tol * tol && best.is_none_or(|(_, b)| d2 < b) {
            best = Some((i, d2));
        }
    }
    best.map(|(i, _)| i)
}

/// Move control point `i` to display-space `to`, clamped.
///
/// ⚠ An endpoint keeps its own `x` and moves only in `y`. Forcing it to 0 or 1 would silently
/// rewrite a hand-written sidecar whose curve deliberately starts partway along — the stage
/// extends flat outside the point range, which is a usable thing to have asked for.
pub fn drag(points: &mut [[f32; 2]], i: usize, to: [f32; 2]) {
    let n = points.len();
    if i >= n {
        return;
    }
    let y = from_display(to[1]);
    if i == 0 || i == n - 1 {
        points[i][1] = y;
        return;
    }
    let lo = to_display(points[i - 1][0]) + MIN_GAP;
    let hi = to_display(points[i + 1][0]) - MIN_GAP;
    // ⚠ Neighbours closer together than two gaps cannot arise from `insert`, but a hand-edited
    // sidecar can hold them. Land on the midpoint rather than letting `clamp` panic on lo > hi.
    let x = if lo > hi {
        0.5 * (lo + hi)
    } else {
        to[0].clamp(0.0, 1.0).clamp(lo, hi)
    };
    points[i] = [from_display(x), y];
}

/// Insert a control point at display-space `at`, returning its index.
///
/// `None` when it would sit outside the existing range or closer than [`MIN_GAP`] to a
/// neighbour — a point the pointer could never separate again is worse than no point.
pub fn insert(points: &mut Vec<[f32; 2]>, at: [f32; 2]) -> Option<usize> {
    let dx = at[0].clamp(0.0, 1.0);
    let i = points.iter().position(|p| to_display(p[0]) > dx)?;
    if i == 0 {
        return None;
    }
    if dx < to_display(points[i - 1][0]) + MIN_GAP || dx > to_display(points[i][0]) - MIN_GAP {
        return None;
    }
    points.insert(i, [from_display(dx), from_display(at[1].clamp(0.0, 1.0))]);
    Some(i)
}

/// Remove control point `i`. Refuses the two endpoints, which define the curve's range.
pub fn remove(points: &mut Vec<[f32; 2]>, i: usize) -> bool {
    if i == 0 || i + 1 >= points.len() {
        return false;
    }
    points.remove(i);
    true
}

/// The curve as `n` display-space samples, ready to plot.
///
/// ⚠⚠ **Sampled through the real `ToneCurveStage`, never re-derived.** The widget must draw
/// the function the pipeline applies — including its LUT quantisation — or the two can drift
/// and the preview stops being evidence about the render. It costs one LUT build per frame
/// (1024 spline evaluations), which is far below anything the panel notices.
pub fn curve_points(points: &[[f32; 2]], n: usize) -> Vec<[f32; 2]> {
    let n = n.max(2);
    let stage = ToneCurveStage::new(points);
    (0..n)
        .map(|i| {
            let u = i as f32 / (n - 1) as f32;
            [u, to_display(stage.sample(from_display(u)))]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(v: f32) -> f32 {
        to_display(v)
    }

    #[test]
    fn an_empty_curve_edits_as_the_identity() {
        // A photo with no curve is the overwhelmingly common starting state, and the widget
        // must have two endpoints to draw.
        assert_eq!(editable(&[]), IDENTITY.to_vec());
        assert!(is_identity(&editable(&[])));
    }

    #[test]
    fn a_stored_curve_is_edited_as_it_stands() {
        let pts = vec![[0.0, 0.0], [0.5, 0.7], [1.0, 1.0]];
        assert_eq!(editable(&pts), pts);
        assert!(!is_identity(&pts));
    }

    #[test]
    fn the_display_warp_round_trips() {
        for i in 0..=20 {
            let u = i as f32 / 20.0;
            assert!((to_display(from_display(u)) - u).abs() < 1e-5, "u={u}");
        }
    }

    #[test]
    fn mid_grey_sits_near_the_middle_of_the_widget() {
        // ⚠⚠ The whole reason the axes are warped, stated as a test rather than as a comment.
        // Linear mid-grey is 0.214, so on unwarped axes a "midtone" control point would sit at
        // a fifth of the width and the shadows would be unreachable.
        let mid = from_display(0.5);
        assert!(
            (mid - 0.2140).abs() < 1e-3,
            "sRGB 0.5 is linear {mid}, not 0.5",
        );
        assert!((to_display(mid) - 0.5).abs() < 1e-5);
    }

    #[test]
    fn hit_finds_the_nearest_point_and_only_within_tolerance() {
        let pts = vec![[0.0, 0.0], [0.2, 0.3], [1.0, 1.0]];
        let mid = [d(0.2), d(0.3)];
        assert_eq!(hit(&pts, mid, 0.05), Some(1));
        // Just outside the tolerance in x.
        assert_eq!(hit(&pts, [mid[0] + 0.06, mid[1]], 0.05), None);
        assert_eq!(hit(&pts, [d(0.0), d(0.0)], 0.05), Some(0));
        assert_eq!(hit(&pts, [d(1.0), d(1.0)], 0.05), Some(2));
    }

    #[test]
    fn hit_measures_both_axes() {
        // ⚠ A hit test that ignored `y` would grab a point the pointer is nowhere near, which
        // on a steep curve is a different point entirely. The probe is far in `y` and exact
        // in `x`, so only a two-axis distance rejects it.
        let pts = vec![[0.0, 0.0], [0.2, 0.3], [1.0, 1.0]];
        assert_eq!(hit(&pts, [d(0.2), d(0.3)], 0.05), Some(1));
        assert_eq!(hit(&pts, [d(0.2), d(0.9)], 0.05), None);
    }

    #[test]
    fn dragging_an_endpoint_moves_only_its_output() {
        let mut pts = IDENTITY.to_vec();
        drag(&mut pts, 0, [0.7, d(0.4)]);
        assert_eq!(pts[0][0], 0.0, "the endpoint's input must not move");
        assert!((pts[0][1] - 0.4).abs() < 1e-5);
        drag(&mut pts, 1, [0.2, d(0.6)]);
        assert_eq!(pts[1][0], 1.0);
        assert!((pts[1][1] - 0.6).abs() < 1e-5);
    }

    #[test]
    fn an_endpoint_partway_along_keeps_where_it_is() {
        // ⚠ The stage extends flat outside the point range, so a curve that starts at 0.2 is a
        // usable thing to have written. Snapping the endpoint to 0 on the first drag would
        // silently rewrite it.
        let mut pts = vec![[0.2, 0.1], [0.9, 0.8]];
        drag(&mut pts, 0, [0.0, d(0.3)]);
        assert!((pts[0][0] - 0.2).abs() < 1e-6, "x moved to {}", pts[0][0]);
    }

    #[test]
    fn an_interior_point_cannot_be_dragged_past_its_neighbours() {
        let mut pts = vec![[0.0, 0.0], [from_display(0.3), 0.2], [1.0, 1.0]];
        // Far to the left, past the first endpoint.
        drag(&mut pts, 1, [-5.0, d(0.5)]);
        assert!(to_display(pts[1][0]) >= MIN_GAP - 1e-5);
        // Far to the right, past the last.
        drag(&mut pts, 1, [5.0, d(0.5)]);
        assert!(to_display(pts[1][0]) <= 1.0 - MIN_GAP + 1e-5);
        // And the order is still strictly ascending, which the spline depends on.
        assert!(pts[0][0] < pts[1][0] && pts[1][0] < pts[2][0]);
    }

    #[test]
    fn a_drag_clamps_its_output_into_range() {
        let mut pts = IDENTITY.to_vec();
        drag(&mut pts, 0, [0.0, 5.0]);
        assert!((pts[0][1] - 1.0).abs() < 1e-5);
        drag(&mut pts, 1, [1.0, -5.0]);
        assert_eq!(pts[1][1], 0.0);
    }

    #[test]
    fn inserting_puts_the_point_in_order_and_reports_where() {
        let mut pts = IDENTITY.to_vec();
        assert_eq!(insert(&mut pts, [0.5, 0.6]), Some(1));
        assert_eq!(pts.len(), 3);
        assert!(pts[0][0] < pts[1][0] && pts[1][0] < pts[2][0]);
        assert!((to_display(pts[1][0]) - 0.5).abs() < 1e-5);
        assert!((to_display(pts[1][1]) - 0.6).abs() < 1e-5);
        // A second point to the left of the first lands before it.
        assert_eq!(insert(&mut pts, [0.25, 0.2]), Some(1));
        assert!(pts.windows(2).all(|w| w[0][0] < w[1][0]));
    }

    #[test]
    fn inserting_on_top_of_an_existing_point_is_refused() {
        // ⚠ Two points closer than the pointer can separate is a control nobody can undo
        // except by deleting one of them blind.
        let mut pts = IDENTITY.to_vec();
        insert(&mut pts, [0.5, 0.5]);
        assert_eq!(insert(&mut pts, [0.5 + MIN_GAP * 0.5, 0.4]), None);
        assert_eq!(pts.len(), 3, "nothing was added");
    }

    #[test]
    fn inserting_outside_the_endpoints_is_refused() {
        // Beyond the last point there is no segment to insert into, and before the first the
        // curve is flat by definition.
        let mut pts = vec![[from_display(0.2), 0.1], [from_display(0.8), 0.9]];
        assert_eq!(insert(&mut pts, [0.05, 0.5]), None);
        assert_eq!(insert(&mut pts, [0.95, 0.5]), None);
        assert_eq!(pts.len(), 2);
    }

    #[test]
    fn an_endpoint_cannot_be_removed() {
        // ⚠ Removing one would change the curve's *range*, not merely its shape — everything
        // outside the range is clamped flat — and there is no gesture that puts it back.
        let mut pts = vec![[0.0, 0.0], [0.5, 0.6], [1.0, 1.0]];
        assert!(!remove(&mut pts, 0));
        assert!(!remove(&mut pts, 2));
        assert_eq!(pts.len(), 3);
        assert!(remove(&mut pts, 1));
        assert_eq!(pts, IDENTITY.to_vec());
    }

    #[test]
    fn the_plotted_curve_is_the_identity_diagonal_for_an_identity_curve() {
        for p in curve_points(&IDENTITY, 33) {
            assert!((p[1] - p[0]).abs() < 3e-3, "{p:?}");
        }
    }

    #[test]
    fn the_plotted_curve_agrees_with_the_stage_it_will_be_rendered_by() {
        // ⚠⚠ The point of routing the plot through `ToneCurveStage`: the widget cannot show a
        // curve the pipeline does not apply. Sabotaging `curve_points` to plot the straight
        // chords instead fails this.
        let pts = vec![[0.0, 0.0], [0.25, 0.15], [0.75, 0.85], [1.0, 1.0]];
        let stage = ToneCurveStage::new(&pts);
        for p in curve_points(&pts, 41) {
            let want = to_display(stage.sample(from_display(p[0])));
            assert!(
                (p[1] - want).abs() < 1e-5,
                "at {}: {} vs {want}",
                p[0],
                p[1]
            );
        }
    }

    #[test]
    fn a_lifted_midpoint_plots_above_the_diagonal() {
        // The end-to-end sanity check: drag a point up, and the drawn curve rises with it.
        let mut pts = IDENTITY.to_vec();
        insert(&mut pts, [0.5, 0.7]);
        let plotted = curve_points(&pts, 65);
        let mid = plotted.iter().find(|p| (p[0] - 0.5).abs() < 0.01).unwrap();
        assert!((mid[1] - 0.7).abs() < 0.01, "midpoint plots at {}", mid[1]);
        assert!(plotted[16][1] > plotted[16][0], "shadows lifted too");
    }
}
