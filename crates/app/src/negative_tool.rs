//! The film negative's measurements (M10.11a): the base picked, the frame measured, and the one
//! function every reader of the undeveloped decode goes through.
//!
//! ⚠ Lives in the lib, with no egui in sight — `wb_tool`'s and `auto_tone_tool`'s pattern. The canvas
//! supplies the click; everything here is a pure function of a buffer and an `Edits`.
//!
//! ⚠⚠ **The buffer is `Session.buffer`, the undeveloped decode — never the rendered texture**, which
//! is already the positive: measuring it would make a second pick chase the first.

use core_image::ImageBuf;
use edit_model::Edits;

/// Below this many percent of the frame, the shadows; above, the highlights — so a few dust specks,
/// which block the light and print white, do not set the white, and a scratch does not set the black.
pub const LOW: f32 = 0.005;
pub const HIGH: f32 = 0.995;

/// The patch the pick averages: 5 × 5, the white-balance eyedropper's.
pub const PATCH_R: usize = 2;

/// The mean of the `(2r + 1)²` patch of `buffer` centred at `at` (normalised `[u, v]`), or `None` off
/// the photo.
pub fn patch_mean(buffer: &ImageBuf, at: [f32; 2], r: usize) -> Option<[f32; 3]> {
    let (w, h) = (buffer.width(), buffer.height());
    if w == 0 || h == 0 || !(0.0..=1.0).contains(&at[0]) || !(0.0..=1.0).contains(&at[1]) {
        return None;
    }
    let cx = ((at[0] * w as f32) as usize).min(w - 1);
    let cy = ((at[1] * h as f32) as usize).min(h - 1);
    let (mut acc, mut n) = ([0.0f32; 3], 0.0f32);
    for y in cy.saturating_sub(r)..=(cy + r).min(h - 1) {
        for x in cx.saturating_sub(r)..=(cx + r).min(w - 1) {
            let p = buffer.pixel(x, y);
            for c in 0..3 {
                acc[c] += p[c];
            }
            n += 1.0;
        }
    }
    Some(acc.map(|a| a / n))
}

/// The frame's shadow and highlight densities per channel — the [`LOW`] and [`HIGH`] percentiles of
/// `log10(base / pixel)` over a 512 px analysis inside the crop — for the base `edits` holds.
///
/// ⚠ Inside the crop, `auto_tone_tool::source_crop`'s frame: cropping out the rebate and the scanner's
/// border first is what keeps them from becoming the black.
pub fn measure(buffer: &ImageBuf, edits: &Edits) -> Option<([f32; 3], [f32; 3])> {
    let n = &edits.negative;
    if !n.base.iter().all(|&b| b > 0.0) {
        return None;
    }
    let small = buffer.downscale_to(crate::auto_tone_tool::ANALYSIS_DIM);
    let (w, h) = (small.width(), small.height());
    let crop = crate::auto_tone_tool::source_crop(edits.geometry.crop, edits.geometry.orientation);
    let (x0, x1) = (
        (crop.left * w as f32) as usize,
        (crop.right * w as f32).ceil() as usize,
    );
    let (y0, y1) = (
        (crop.top * h as f32) as usize,
        (crop.bottom * h as f32).ceil() as usize,
    );
    let mut d: [Vec<f32>; 3] = Default::default();
    for y in y0..y1.min(h) {
        for x in x0..x1.min(w) {
            let px = pipeline::negative::density(small.pixel(x, y), n);
            for c in 0..3 {
                d[c].push(px[c]);
            }
        }
    }
    if d[0].is_empty() {
        return None;
    }
    let at = |v: &mut Vec<f32>, p: f32| {
        v.sort_by(f32::total_cmp);
        v[((v.len() - 1) as f32 * p).round() as usize]
    };
    let mut low = [0.0; 3];
    let mut high = [0.0; 3];
    for c in 0..3 {
        low[c] = at(&mut d[c], LOW);
        high[c] = at(&mut d[c], HIGH);
    }
    Some((low, high))
}

/// Take the film base from the patch at `at` and measure the frame — the eyedropper's click. `false`,
/// changing nothing, off the photo or on a patch with a black channel, which cannot be film base.
pub fn pick(edits: &mut Edits, buffer: &ImageBuf, at: [f32; 2]) -> bool {
    let Some(base) = patch_mean(buffer, at, PATCH_R) else {
        return false;
    };
    if !base.iter().all(|&b| b > 0.0) {
        return false;
    }
    let before = edits.negative;
    edits.negative.base = base;
    edits.negative.on = true;
    if rebalance(edits, buffer) {
        true
    } else {
        edits.negative = before;
        false
    }
}

/// Measure the frame again for the base already picked — *Auto*, after a crop or on another frame of
/// the roll. `false`, changing nothing, when there is no base or nothing to measure.
pub fn rebalance(edits: &mut Edits, buffer: &ImageBuf) -> bool {
    match measure(buffer, edits) {
        Some((low, high)) => {
            edits.negative.low = low;
            edits.negative.high = high;
            true
        }
        None => false,
    }
}

/// A pixel of the undeveloped decode as the photo's own stages will see it: the positive when the
/// negative is active, itself otherwise. ⚠⚠ **Every reader of the decode goes through this** — the
/// eyedroppers, Auto tone, the AI masks' input, red-eye detection — or it measures the negative.
pub fn view(rgb: [f32; 3], edits: &Edits) -> [f32; 3] {
    pipeline::negative::positive(rgb, &edits.negative)
}

#[cfg(test)]
mod tests {
    use super::*;
    use edit_model::CropRect;

    const BASE: [f32; 3] = [0.85, 0.55, 0.30];

    /// A scanned strip: the left tenth unexposed film (the base), the rest a ramp of densities
    /// `d0..d1` above it, the same in every channel.
    fn strip(w: usize, h: usize, d0: f32, d1: f32) -> ImageBuf {
        let mut img = ImageBuf::new(w, h);
        let edge = w / 10;
        for y in 0..h {
            for x in 0..w {
                let px = if x < edge {
                    BASE
                } else {
                    let t = (x - edge) as f32 / (w - edge - 1) as f32;
                    let d = d0 + (d1 - d0) * t;
                    BASE.map(|b| b * 10f32.powf(-d))
                };
                img.set_pixel(x, y, px);
            }
        }
        img
    }

    fn close(a: f32, b: f32, tol: f32) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn a_pick_on_the_film_base_sets_it_and_measures_the_frame() {
        let buf = strip(100, 40, 0.2, 1.6);
        let mut e = Edits::default();
        assert!(pick(&mut e, &buf, [0.05, 0.5]));
        let n = e.negative;
        assert!(n.on && n.is_active());
        for (c, want) in BASE.iter().enumerate() {
            assert!(close(n.base[c], *want, 0.01 * want), "{:?}", n.base);
            assert!(
                close(n.low[c], 0.0, 0.01),
                "the strip is the black: {:?}",
                n.low
            );
            assert!(close(n.high[c], 1.6, 0.03), "the densest end: {:?}", n.high);
        }
    }

    #[test]
    fn the_measurement_keeps_to_the_crop() {
        let buf = strip(100, 40, 0.2, 1.6);
        let mut e = Edits::default();
        assert!(pick(&mut e, &buf, [0.05, 0.5]));
        e.geometry.crop = CropRect {
            left: 0.15,
            ..CropRect::default()
        };
        assert!(rebalance(&mut e, &buf));
        assert!(
            e.negative.low.iter().all(|&d| d > 0.15),
            "the strip lies outside the crop: {:?}",
            e.negative.low
        );
    }

    #[test]
    fn a_few_dust_specks_do_not_set_the_white() {
        let mut buf = strip(100, 40, 0.2, 1.6);
        let clean = {
            let mut e = Edits::default();
            pick(&mut e, &buf, [0.05, 0.5]);
            e.negative.high
        };
        // 0.4 % of the frame blocked by dust — denser than anything on the film.
        for i in 0..16 {
            buf.set_pixel(20 + i * 4, 7 + i % 20, BASE.map(|b| b * 1e-4));
        }
        let mut e = Edits::default();
        pick(&mut e, &buf, [0.05, 0.5]);
        for c in 0..3 {
            assert!(
                close(e.negative.high[c], clean[c], 0.01 * clean[c]),
                "{:?} against {clean:?}",
                e.negative.high
            );
        }
    }

    #[test]
    fn rebalancing_measures_the_frame_in_hand() {
        let mut e = Edits::default();
        pick(&mut e, &strip(100, 40, 0.2, 1.6), [0.05, 0.5]);
        let thin = strip(100, 40, 0.1, 0.9);
        assert!(rebalance(&mut e, &thin));
        assert!(
            e.negative.high.iter().all(|&d| close(d, 0.9, 0.03)),
            "{:?}",
            e.negative.high
        );
        // The base stays as picked: it is the roll's, not the frame's.
        assert!(close(e.negative.base[0], BASE[0], 0.01));
    }

    #[test]
    fn a_pick_on_nothing_changes_nothing() {
        let mut e = Edits::default();
        let black = ImageBuf::new(10, 10);
        assert!(
            !pick(&mut e, &black, [0.5, 0.5]),
            "a black patch is no film base"
        );
        assert!(
            !pick(&mut e, &strip(100, 40, 0.2, 1.6), [1.5, 0.5]),
            "off the photo"
        );
        assert_eq!(e.negative, edit_model::Negative::default());
    }

    #[test]
    fn view_is_the_positive_only_when_the_negative_is_active() {
        let px = [0.3, 0.2, 0.1];
        assert_eq!(view(px, &Edits::default()), px);
        let mut e = Edits::default();
        pick(&mut e, &strip(100, 40, 0.2, 1.6), [0.05, 0.5]);
        assert_eq!(view(px, &e), pipeline::negative::positive(px, &e.negative));
        assert_ne!(view(px, &e), px);
    }
}
