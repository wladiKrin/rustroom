//! Where things are on the canvas.
//!
//! ⚠⚠ **Every function here works against the *frame* — the whole post-geometry image — never
//! against `img_rect`.** Once a region render is on, `img_rect` covers only the rendered slice, so
//! placing a mask against it makes the mask drift with the view as you pan. That bug shipped, and
//! it had three heads: the overlay's position, the aspect used for hit-testing, and the drag
//! deltas. Lifting the maths here is what makes all three testable without a window.

use eframe::egui;

/// The on-screen rect of the whole post-geometry image.
///
/// `full` is the fit-resolution image size, `rung_k` the current render rung's factor over that
/// fit size, and `scale` the on-screen scale. ⚠ `pan` is added to the viewport centre, so the
/// frame moves with the view while staying the *frame* — which is exactly the distinction
/// `img_rect` loses.
pub fn frame_rect(
    viewport_center: egui::Pos2,
    pan: egui::Vec2,
    full: egui::Vec2,
    rung_k: f32,
    scale: f32,
) -> egui::Rect {
    let size = egui::vec2(full.x * rung_k, full.y * rung_k) * scale;
    egui::Rect::from_center_size(viewport_center + pan, size)
}

/// A screen point as a fraction of the frame, clamped to it.
///
/// ⚠ Clamped because a drag continues outside the image, and a mask handle dragged off-frame must
/// stop at the edge rather than fly away.
pub fn normalized_point(p: egui::Pos2, frame: egui::Rect) -> (f32, f32) {
    let nx = ((p.x - frame.left()) / frame.width()).clamp(0.0, 1.0);
    let ny = ((p.y - frame.top()) / frame.height()).clamp(0.0, 1.0);
    (nx, ny)
}

/// The frame's aspect ratio, for the hit-testing that measures distance in *screen* space.
///
/// ⚠ Normalised mask coordinates are anisotropic, so a distance test needs this correction. Taking
/// it from `img_rect` instead — which happened — silently corrupted hit-testing at high zoom,
/// because a region's aspect is not the frame's.
pub fn frame_aspect(frame: egui::Rect) -> f32 {
    frame.width() / frame.height()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32, w: f32, h: f32) -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(w, h))
    }

    #[test]
    fn the_frame_is_centred_on_the_viewport_plus_the_pan() {
        let r = frame_rect(
            egui::pos2(100.0, 50.0),
            egui::vec2(10.0, -5.0),
            egui::vec2(80.0, 40.0),
            1.0,
            1.0,
        );
        assert_eq!(r.center(), egui::pos2(110.0, 45.0));
        assert_eq!(r.size(), egui::vec2(80.0, 40.0));
    }

    #[test]
    fn the_rung_and_the_scale_both_size_the_frame() {
        // ⚠ Both multiply. A frame sized by only one of them is the bug where the overlay is the
        // right shape and the wrong size, which reads as "the mask is offset".
        let r = frame_rect(
            egui::Pos2::ZERO,
            egui::Vec2::ZERO,
            egui::vec2(80.0, 40.0),
            2.0,
            0.5,
        );
        assert_eq!(r.size(), egui::vec2(80.0, 40.0), "2.0 * 0.5 = 1.0");
        let r2 = frame_rect(
            egui::Pos2::ZERO,
            egui::Vec2::ZERO,
            egui::vec2(80.0, 40.0),
            2.0,
            1.0,
        );
        assert_eq!(r2.size(), egui::vec2(160.0, 80.0));
    }

    #[test]
    fn panning_moves_the_frame_and_nothing_else() {
        // ⚠⚠ The regression guard for the drift bug. The frame must translate with the pan and
        // keep its size — if it were derived from the rendered region instead, it would *shrink*
        // to the visible slice and the mask would appear to follow the view.
        let a = frame_rect(
            egui::pos2(50.0, 50.0),
            egui::Vec2::ZERO,
            egui::vec2(60.0, 30.0),
            1.0,
            1.0,
        );
        let b = frame_rect(
            egui::pos2(50.0, 50.0),
            egui::vec2(25.0, 0.0),
            egui::vec2(60.0, 30.0),
            1.0,
            1.0,
        );
        assert_eq!(a.size(), b.size(), "panning must not resize the frame");
        assert_eq!(b.center() - a.center(), egui::vec2(25.0, 0.0));
    }

    #[test]
    fn a_point_normalises_against_the_frames_own_corner() {
        let f = rect(20.0, 10.0, 80.0, 40.0);
        assert_eq!(normalized_point(egui::pos2(20.0, 10.0), f), (0.0, 0.0));
        assert_eq!(normalized_point(egui::pos2(100.0, 50.0), f), (1.0, 1.0));
        assert_eq!(normalized_point(egui::pos2(60.0, 30.0), f), (0.5, 0.5));
    }

    #[test]
    fn a_point_outside_the_frame_clamps_rather_than_escaping() {
        // ⚠ A drag continues past the image edge; a handle dragged off-frame must stop at the
        // edge, not fly to an unreachable coordinate.
        let f = rect(0.0, 0.0, 100.0, 50.0);
        assert_eq!(normalized_point(egui::pos2(-40.0, -40.0), f), (0.0, 0.0));
        assert_eq!(normalized_point(egui::pos2(500.0, 500.0), f), (1.0, 1.0));
    }

    #[test]
    fn the_aspect_is_the_frames_not_a_regions() {
        // ⚠ The value that was wrongly taken from `img_rect`. On a 2:1 frame the correction is
        // 2.0; a region covering a square slice of it would report 1.0, and every hit test at
        // high zoom would be wrong by that factor.
        assert_eq!(frame_aspect(rect(0.0, 0.0, 100.0, 50.0)), 2.0);
        assert_eq!(frame_aspect(rect(0.0, 0.0, 50.0, 100.0)), 0.5);
    }
}

/// The two arm end-points of each corner bracket, as `(from, to)` segments — eight in all.
///
/// ⚠⚠ **Every arm points INWARD.** At a full-frame crop the rectangle sits exactly on the image
/// boundary, and `painter_at(img_rect)` clips to that boundary — so an arm drawn outward would be
/// half-clipped and read as a stray tick rather than a corner. That is the case the crop overlay
/// is hardest to see in, and the one this exists for.
///
/// ⚠ The arm length shrinks for a small crop, so the four brackets can never meet in the middle
/// and turn into a solid outline.
pub fn corner_brackets(rect: egui::Rect, arm: f32) -> Vec<(egui::Pos2, egui::Pos2)> {
    // At most a third of the shorter side, so opposite brackets always leave a gap.
    let arm = arm
        .min(rect.width() / 3.0)
        .min(rect.height() / 3.0)
        .max(1.0);
    let mut out = Vec::with_capacity(8);
    for (corner, sx, sy) in [
        (rect.left_top(), 1.0, 1.0),
        (rect.right_top(), -1.0, 1.0),
        (rect.left_bottom(), 1.0, -1.0),
        (rect.right_bottom(), -1.0, -1.0),
    ] {
        out.push((corner, egui::pos2(corner.x + sx * arm, corner.y)));
        out.push((corner, egui::pos2(corner.x, corner.y + sy * arm)));
    }
    out
}

/// A short bar centred on each edge of `rect`, the handle for dragging that edge alone. Clamped
/// like the brackets so a tiny crop keeps its four bars apart.
pub fn edge_bars(rect: egui::Rect, len: f32) -> Vec<(egui::Pos2, egui::Pos2)> {
    let half = (len / 2.0)
        .min(rect.width() / 6.0)
        .min(rect.height() / 6.0)
        .max(1.0);
    let c = rect.center();
    vec![
        (
            egui::pos2(c.x - half, rect.top()),
            egui::pos2(c.x + half, rect.top()),
        ),
        (
            egui::pos2(c.x - half, rect.bottom()),
            egui::pos2(c.x + half, rect.bottom()),
        ),
        (
            egui::pos2(rect.left(), c.y - half),
            egui::pos2(rect.left(), c.y + half),
        ),
        (
            egui::pos2(rect.right(), c.y - half),
            egui::pos2(rect.right(), c.y + half),
        ),
    ]
}

#[cfg(test)]
mod bracket_tests {
    use super::*;

    fn r() -> egui::Rect {
        egui::Rect::from_min_max(egui::pos2(10.0, 20.0), egui::pos2(210.0, 140.0))
    }

    /// ⚠⚠ The property the full-frame case depends on: nothing may stick out past the rectangle,
    /// because the painter clips to the image and an overhanging arm would be cut in half.
    #[test]
    fn every_bracket_arm_stays_inside_the_rectangle() {
        for (a, b) in corner_brackets(r(), 24.0) {
            for p in [a, b] {
                assert!(
                    r().contains(p),
                    "{p:?} is outside {:?} and would be clipped away",
                    r()
                );
            }
        }
    }

    /// Two arms per corner, four corners, and each arm actually starts at its corner.
    #[test]
    fn there_are_two_arms_at_each_of_the_four_corners() {
        let segs = corner_brackets(r(), 24.0);
        assert_eq!(segs.len(), 8);
        for c in [
            r().left_top(),
            r().right_top(),
            r().left_bottom(),
            r().right_bottom(),
        ] {
            assert_eq!(
                segs.iter().filter(|(a, _)| *a == c).count(),
                2,
                "corner {c:?} should have two arms"
            );
        }
    }

    /// ⚠ A tiny crop must not turn into a solid outline: the arms shorten so opposite brackets
    /// always leave a gap. Without the clamp a 30 px crop with a 24 px arm would meet in the
    /// middle and the corners would stop reading as corners.
    #[test]
    fn the_arms_shorten_rather_than_meeting_on_a_small_crop() {
        let small = egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(30.0, 30.0));
        for (a, b) in corner_brackets(small, 24.0) {
            let len = (b - a).length();
            assert!(
                len <= small.width() / 3.0 + 1e-3,
                "arm of {len} is more than a third of the crop"
            );
        }
    }
}

/// The widest stroke the crop overlay paints, in screen points.
pub const CROP_HALO_WIDTH: f32 = 6.0;

/// The clip the crop overlay's *marks* are painted into.
///
/// ⚠⚠ **Wider than the image, and that is the whole point.** `line_segment` centres its stroke on
/// the path, so a 6 pt halo drawn along the crop boundary puts 3 pt of itself on each side. At a
/// **full-frame** crop that boundary *is* the image edge, so clipping to the image cuts every mark
/// in half — the halo worst of all, being the widest. Reported in use as *"parts of the lines are
/// outside and cannot be seen"*.
///
/// ⚠ The dimming rectangles keep the **tight** clip: they darken what falls outside the crop, and
/// there is no photograph beyond the image to darken.
pub fn overlay_clip(img_rect: egui::Rect) -> egui::Rect {
    img_rect.expand(CROP_HALO_WIDTH / 2.0)
}

#[cfg(test)]
mod overlay_clip_tests {
    use super::*;

    fn img() -> egui::Rect {
        egui::Rect::from_min_max(egui::pos2(10.0, 20.0), egui::pos2(210.0, 140.0))
    }

    /// ⚠⚠ The regression guard for the reported bug: at a **full-frame** crop every mark lies on
    /// the image edge, so the clip must hold the full stroke width on both sides of it. Without
    /// the expansion the outer half of every line is cut away.
    #[test]
    fn the_clip_holds_the_whole_stroke_of_a_mark_on_the_image_edge() {
        let clip = overlay_clip(img());
        let half = CROP_HALO_WIDTH / 2.0;
        // A full-frame crop: the brackets sit exactly on the image boundary.
        for (a, b) in corner_brackets(img(), 22.0) {
            for p in [a, b] {
                let stroke = egui::Rect::from_center_size(p, egui::vec2(half * 2.0, half * 2.0));
                assert!(
                    clip.contains_rect(stroke),
                    "the stroke around {p:?} is cut by the clip {clip:?}"
                );
            }
        }
    }

    /// ⚠ It expands rather than shrinking. An earlier fix inset the *geometry* instead, which
    /// keeps the marks whole but moves them off the crop line they are supposed to mark.
    #[test]
    fn the_overlay_clip_contains_the_image_rather_than_shrinking_it() {
        assert!(overlay_clip(img()).contains_rect(img()));
    }
}

/// Which guide endpoint is within `tol` of `p`, as `(guide index, 0 = from | 1 = to)`.
///
/// ⚠ **Nearest wins, not first.** Two endpoints can end up overlapping after a drag, and
/// "whichever was drawn first" would make one of them permanently unreachable.
///
/// ⚠ Takes the guides' **displayed** positions rather than the guides themselves — you grab what
/// you can see, and with a live solve the two differ.
pub fn guide_endpoint_at_shown(
    segments: &[([f32; 2], [f32; 2])],
    p: [f32; 2],
    tol: f32,
) -> Option<(usize, usize)> {
    let mut best: Option<((usize, usize), f32)> = None;
    for (i, (a, b)) in segments.iter().enumerate() {
        for (e, pt) in [(0usize, *a), (1usize, *b)] {
            let d = ((pt[0] - p[0]).powi(2) + (pt[1] - p[1]).powi(2)).sqrt();
            if d <= tol && best.is_none_or(|(_, bd)| d < bd) {
                best = Some(((i, e), d));
            }
        }
    }
    best.map(|(k, _)| k)
}

#[cfg(test)]
mod guide_hit_tests {
    use super::*;
    fn gs() -> Vec<([f32; 2], [f32; 2])> {
        vec![([0.1, 0.1], [0.2, 0.9]), ([0.8, 0.1], [0.7, 0.9])]
    }

    #[test]
    fn an_endpoint_within_tolerance_is_found_and_others_are_not() {
        assert_eq!(
            guide_endpoint_at_shown(&gs(), [0.105, 0.105], 0.03),
            Some((0, 0))
        );
        assert_eq!(
            guide_endpoint_at_shown(&gs(), [0.71, 0.89], 0.03),
            Some((1, 1))
        );
        assert_eq!(guide_endpoint_at_shown(&gs(), [0.5, 0.5], 0.03), None);
    }

    /// ⚠ Nearest, not first. With two endpoints inside the tolerance, taking the first would make
    /// the other unreachable for as long as they overlap.
    #[test]
    fn the_nearest_endpoint_wins_when_two_are_in_range() {
        let g = vec![([0.50, 0.50], [0.5, 0.9]), ([0.52, 0.50], [0.9, 0.5])];
        assert_eq!(
            guide_endpoint_at_shown(&g, [0.519, 0.50], 0.05),
            Some((1, 0))
        );
        assert_eq!(
            guide_endpoint_at_shown(&g, [0.501, 0.50], 0.05),
            Some((0, 0))
        );
    }
}

/// Which guide's **line** passes within `tol` of `p`, nearest first.
///
/// ⚠ Distance to the *segment*, not to the infinite line: a guide's extension across the rest of
/// the frame is not part of it, and hit-testing the infinite line would make a short guide near
/// the edge swallow clicks right across the picture.
///
/// ⚠ The points are the guide's **displayed** positions, so the caller maps them first — you
/// click what you can see.
pub fn guide_near(segments: &[([f32; 2], [f32; 2])], p: [f32; 2], tol: f32) -> Option<usize> {
    let mut best: Option<(usize, f32)> = None;
    for (i, (a, b)) in segments.iter().enumerate() {
        let (vx, vy) = (b[0] - a[0], b[1] - a[1]);
        let (wx, wy) = (p[0] - a[0], p[1] - a[1]);
        let len2 = vx * vx + vy * vy;
        // A zero-length guide is a point; clamping `t` to 0 measures to it correctly.
        let t = if len2 > 1e-12 {
            ((wx * vx + wy * vy) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let (dx, dy) = (wx - t * vx, wy - t * vy);
        let d = (dx * dx + dy * dy).sqrt();
        if d <= tol && best.is_none_or(|(_, bd)| d < bd) {
            best = Some((i, d));
        }
    }
    best.map(|(i, _)| i)
}

#[cfg(test)]
mod guide_near_tests {
    use super::*;

    fn segs() -> Vec<([f32; 2], [f32; 2])> {
        vec![([0.2, 0.1], [0.2, 0.9]), ([0.8, 0.1], [0.8, 0.9])]
    }

    #[test]
    fn a_click_on_the_line_finds_it_and_one_away_does_not() {
        assert_eq!(guide_near(&segs(), [0.21, 0.5], 0.03), Some(0));
        assert_eq!(guide_near(&segs(), [0.79, 0.3], 0.03), Some(1));
        assert_eq!(guide_near(&segs(), [0.5, 0.5], 0.03), None);
    }

    /// ⚠⚠ The segment, not the infinite line. This click is exactly on the *extension* of guide 0
    /// beyond its end — hit-testing the infinite line would return `Some(0)` and let a short guide
    /// near one edge swallow clicks right across the picture.
    #[test]
    fn the_extension_beyond_an_end_is_not_part_of_the_guide() {
        assert_eq!(guide_near(&segs(), [0.2, 0.99], 0.03), None);
        // ...while just inside the end still hits.
        assert_eq!(guide_near(&segs(), [0.2, 0.88], 0.03), Some(0));
    }

    /// ⚠ Nearest wins, so two crossing guides both in range resolve to the closer one.
    #[test]
    fn the_nearest_line_wins() {
        let s = vec![([0.0, 0.50], [1.0, 0.50]), ([0.0, 0.54], [1.0, 0.54])];
        assert_eq!(guide_near(&s, [0.5, 0.505], 0.05), Some(0));
        assert_eq!(guide_near(&s, [0.5, 0.535], 0.05), Some(1));
    }
}
