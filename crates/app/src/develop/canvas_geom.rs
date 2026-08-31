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
