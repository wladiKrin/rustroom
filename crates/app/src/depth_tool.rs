//! The Depth range's picker (M10.16a): the band a line — or a click — on the photo asks for.
//!
//! ⚠ No egui: the gesture is the canvas's (`develop::canvas::mask_overlay`), the rule is here, on plain
//! types — the `crop_tool` / `mask_tool` pattern.

use crate::ai_space::DecodeMap;
use ai_mask::ProbMap;

/// Half the width, in band points, of the band a click sets.
pub const CLICK_HALF: f32 = 5.0;

/// The depth position (0 nearest … 1 farthest) at frame pixel `p` — the median of the 3 × 3 frame
/// pixels around it, so one stray pixel at the end of a line cannot set a band end — read from `map`
/// through `dm`. `None` when `p` is off the frame.
///
/// ⚠ `map` must be in the space `dm` maps into: post-lens (`AiSource::lensed`) for a corrector-free
/// `dm`, as the render's own coverage reads it.
pub fn depth_at(map: &ProbMap, dm: &DecodeMap, p: [f32; 2]) -> Option<f32> {
    let (fw, fh) = dm.frame_size();
    let inside = |x: f32, y: f32| (0.0..fw).contains(&x) && (0.0..fh).contains(&y);
    if !inside(p[0], p[1]) {
        return None;
    }
    let mut patch: Vec<f32> = (-1..=1)
        .flat_map(|dy| (-1..=1).map(move |dx| (p[0] + dx as f32, p[1] + dy as f32)))
        .filter(|&(x, y)| inside(x, y))
        .map(|(x, y)| {
            let [u, v] = dm.frame_to_decode(x, y);
            map.at(u, v)
        })
        .collect();
    patch.sort_by(f32::total_cmp);
    Some(patch[patch.len() / 2])
}

/// The band a line drawn from depth `a` to depth `b` sets — its two ends, the nearer as Near, in band
/// points 0..=100. The line can be drawn either way.
pub fn band_between(a: f32, b: f32) -> (f32, f32) {
    // ⚠ Rounded to a tenth of a point — the sliders' own resolution — so a byte-quantised map's
    // 79.99 reads as the 80 it is.
    let point = |v: f32| (v.clamp(0.0, 1.0) * 1000.0).round() / 10.0;
    (point(a.min(b)), point(a.max(b)))
}

/// The narrow band a click at `position` sets, clamped to 0..=100.
pub fn band_at(position: f32) -> (f32, f32) {
    let p = position.clamp(0.0, 1.0) * 100.0;
    ((p - CLICK_HALF).max(0.0), (p + CLICK_HALF).min(100.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use edit_model::{Geometry, Perspective};

    /// A 100 × 50 frame over a 100 × 50 decode; the map is 0.2 on the left half, 0.8 on the right.
    fn halves() -> (ProbMap, DecodeMap) {
        let map = ProbMap::from_fn(100, 50, |x, _| if x < 50 { 0.2 } else { 0.8 });
        let dm = DecodeMap::new(
            &Geometry::default(),
            Perspective::default(),
            100,
            50,
            None,
            100,
            50,
        );
        (map, dm)
    }

    /// The author's design (2026-10-14): the line's start is one end of the band, its end the other.
    #[test]
    fn a_line_from_one_depth_to_another_sets_the_band_between_them() {
        let (map, dm) = halves();
        let (near, far) = (
            depth_at(&map, &dm, [10.0, 25.0]).expect("on the photo"),
            depth_at(&map, &dm, [90.0, 25.0]).expect("on the photo"),
        );
        assert_eq!(band_between(near, far), (20.0, 80.0));
        assert_eq!(band_between(far, near), (20.0, 80.0), "drawn the other way");
        assert_eq!(
            band_between(near, near),
            (20.0, 20.0),
            "a line within one depth"
        );
    }

    #[test]
    fn a_stray_pixel_cannot_set_an_end() {
        // 0.3 everywhere but one pixel, exactly where the line starts.
        let map = ProbMap::from_fn(100, 50, |x, y| if (x, y) == (40, 20) { 1.0 } else { 0.3 });
        let (_, dm) = halves();
        let d = depth_at(&map, &dm, [40.5, 20.5]).expect("on the photo");
        assert!((d - 0.3).abs() < 0.01, "{d}");
    }

    #[test]
    fn a_click_sets_a_narrow_band() {
        assert_eq!(band_at(0.5), (45.0, 55.0));
        assert_eq!(band_at(0.02), (0.0, 7.0), "clamped at the nearest point");
        assert_eq!(band_at(1.0), (95.0, 100.0));
    }

    /// Review Focus 4: a line starting or ending off the photo has no depth there.
    #[test]
    fn off_the_photo_there_is_no_depth() {
        let (map, dm) = halves();
        assert_eq!(depth_at(&map, &dm, [-10.0, 25.0]), None);
        assert_eq!(depth_at(&map, &dm, [50.0, 60.0]), None);
        assert!(
            depth_at(&map, &dm, [0.2, 0.2]).is_some(),
            "the corner pixel is on the photo"
        );
    }
}
