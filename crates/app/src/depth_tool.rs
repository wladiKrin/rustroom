//! The Depth range's picker (M10.16a): the band a drag — or a click — on the photo asks for.
//!
//! ⚠ No egui: the gesture is the canvas's (`develop::canvas::mask_overlay`), the rule is here, on plain
//! types — the `crop_tool` / `mask_tool` pattern.

use crate::ai_space::DecodeMap;
use ai_mask::ProbMap;

/// Half the width, in band points, of the band a click sets.
pub const CLICK_HALF: f32 = 5.0;

/// The grid a drag's rectangle is sampled on, at most, per side.
const MAX_SIDE: usize = 64;

/// The depth positions (0 nearest … 1 farthest) under the frame rectangle `rect` — `[x0, y0, x1, y1]`
/// in frame pixels, any corner order — on a grid of at most 64 × 64, read from `map` through `dm`.
///
/// ⚠ `map` must be in the space `dm` maps into: post-lens (`AiSource::lensed`) for a corrector-free
/// `dm`, as the render's own coverage reads it. Points outside the frame are skipped, so a rectangle
/// wholly off the photo has no samples; a zero-area one samples its single point.
pub fn samples(map: &ProbMap, dm: &DecodeMap, rect: [f32; 4]) -> Vec<f32> {
    let (fw, fh) = dm.frame_size();
    let (x0, x1) = (rect[0].min(rect[2]), rect[0].max(rect[2]));
    let (y0, y1) = (rect[1].min(rect[3]), rect[1].max(rect[3]));
    // The grid's sides — one point for a zero-width side, at most `MAX_SIDE`.
    let side = |len: f32| (len.ceil() as usize).clamp(1, MAX_SIDE);
    let (nx, ny) = (side(x1 - x0), side(y1 - y0));
    let at = |lo: f32, hi: f32, i: usize, n: usize| {
        if n == 1 {
            (lo + hi) / 2.0
        } else {
            lo + (hi - lo) * i as f32 / (n - 1) as f32
        }
    };
    let mut out = Vec::with_capacity(nx * ny);
    for j in 0..ny {
        for i in 0..nx {
            let (x, y) = (at(x0, x1, i, nx), at(y0, y1, j, ny));
            if (0.0..fw).contains(&x) && (0.0..fh).contains(&y) {
                let [u, v] = dm.frame_to_decode(x, y);
                out.push(map.at(u, v));
            }
        }
    }
    out
}

/// The band `samples` ask for — their 5th and 95th percentiles, in band points 0..=100, so a stray
/// pixel cannot stretch it; `None` for no samples.
pub fn band(samples: &mut [f32]) -> Option<(f32, f32)> {
    if samples.is_empty() {
        return None;
    }
    samples.sort_by(f32::total_cmp);
    let at = |q: f32| samples[((samples.len() - 1) as f32 * q).round() as usize] * 100.0;
    // ⚠ Rounded to a tenth of a point — the sliders' own resolution — so a byte-quantised map's
    // 79.99 reads as the 80 it is.
    let tenth = |v: f32| (v * 10.0).round() / 10.0;
    Some((tenth(at(0.05)), tenth(at(0.95))))
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

    #[test]
    fn a_drag_over_two_depths_spans_them() {
        let (map, dm) = halves();
        // Dragged right to left — the corner order does not matter.
        let mut s = samples(&map, &dm, [90.0, 40.0, 10.0, 10.0]);
        assert!(s.len() > 100 && s.len() <= 64 * 64, "{}", s.len());
        let (near, far) = band(&mut s).expect("samples");
        assert!((near - 20.0).abs() < 1.5, "{near}");
        assert!((far - 80.0).abs() < 1.5, "{far}");
    }

    #[test]
    fn a_stray_pixel_cannot_stretch_the_band() {
        let mut s = vec![0.3; 999];
        s.push(1.0);
        let (near, far) = band(&mut s).expect("samples");
        assert!((near - 30.0).abs() < 0.5 && far <= 31.0, "{near}..{far}");
    }

    #[test]
    fn a_click_sets_a_narrow_band() {
        assert_eq!(band_at(0.5), (45.0, 55.0));
        assert_eq!(band_at(0.02), (0.0, 7.0), "clamped at the nearest point");
        assert_eq!(band_at(1.0), (95.0, 100.0));
    }

    /// Review Focus 4: a drag of no area samples its one point; one wholly off the photo, nothing.
    #[test]
    fn an_empty_drag_sets_no_band() {
        let (map, dm) = halves();
        let mut point = samples(&map, &dm, [75.0, 25.0, 75.0, 25.0]);
        assert_eq!(point.len(), 1);
        assert_eq!(band(&mut point), Some((80.0, 80.0)));
        let mut off = samples(&map, &dm, [-40.0, -30.0, -10.0, -5.0]);
        assert!(off.is_empty());
        assert_eq!(band(&mut off), None);
        assert_eq!(band(&mut []), None);
    }
}
