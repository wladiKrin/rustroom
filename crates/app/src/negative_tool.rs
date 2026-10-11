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

/// The densities film can have, above its base. ⚠⚠ **What lies outside is not film and is left out of
/// the measurement** (found in use, 2026-10-11): the bare backlight through a sprocket hole is brighter
/// than the base (below 0), and the holder around the film is opaque — measured at 3.1 to 4.8 on a
/// camera scan, where the densest negative reaches about 2.5.
pub const FILM_DENSITY: (f32, f32) = (-0.05, 3.0);

/// The share of each edge left out of the measurement: wide on an **uncropped** frame, whose edges are
/// the scan's surroundings — holder, rebate, the next frame — and narrow once the crop says where the
/// picture is. Negative Lab Pro's *border buffer*.
pub const BORDER_UNCROPPED: f32 = 0.15;
pub const BORDER_CROPPED: f32 = 0.02;

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
/// ⚠ Inside the crop, `auto_tone_tool::source_crop`'s frame, less a border ([`BORDER_UNCROPPED`],
/// [`BORDER_CROPPED`]), and only densities film can have ([`FILM_DENSITY`]) — so the scan's holder,
/// its sprocket holes and the next frame's edge do not become the white or the black.
pub fn measure(buffer: &ImageBuf, edits: &Edits) -> Option<([f32; 3], [f32; 3])> {
    let n = &edits.negative;
    if !n.base.iter().all(|&b| b > 0.0) {
        return None;
    }
    let small = buffer.downscale_to(crate::auto_tone_tool::ANALYSIS_DIM);
    let (w, h) = (small.width(), small.height());
    let crop = crate::auto_tone_tool::source_crop(edits.geometry.crop, edits.geometry.orientation);
    let border = if edits.geometry.crop == edit_model::CropRect::default() {
        BORDER_UNCROPPED
    } else {
        BORDER_CROPPED
    };
    let (cw, ch) = (crop.right - crop.left, crop.bottom - crop.top);
    let (left, right) = (crop.left + border * cw, crop.right - border * cw);
    let (top, bottom) = (crop.top + border * ch, crop.bottom - border * ch);
    let (x0, x1) = (
        (left * w as f32) as usize,
        (right * w as f32).ceil() as usize,
    );
    let (y0, y1) = (
        (top * h as f32) as usize,
        (bottom * h as f32).ceil() as usize,
    );
    let (lo, hi) = FILM_DENSITY;
    let mut d: [Vec<f32>; 3] = Default::default();
    for y in y0..y1.min(h) {
        for x in x0..x1.min(w) {
            let px = pipeline::negative::density(small.pixel(x, y), n);
            if px.iter().all(|v| (lo..=hi).contains(v)) {
                for c in 0..3 {
                    d[c].push(px[c]);
                }
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

/// The film's γ for the panel's *Film contrast* (±100): 0 is a colour negative's 0.6, +100 halves it
/// (a contrastier positive), −100 doubles it.
pub fn gamma_of(contrast: f32) -> f32 {
    0.6 * 2f32.powf(-contrast / 100.0)
}

/// The *Film contrast* a γ reads as — [`gamma_of`]'s inverse.
pub fn contrast_of(gamma: f32) -> f32 {
    -100.0 * (gamma.max(1e-3) / 0.6).log2()
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

    /// The density [`strip`]'s ramp has at `u` across the frame — where an uncropped frame's border
    /// ([`BORDER_UNCROPPED`]) stops the measurement, its densest is read, not the very edge.
    fn ramp_at(u: f32, d0: f32, d1: f32) -> f32 {
        d0 + (d1 - d0) * (u - 0.1) / 0.9
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
            // ⚠ Not the strip itself: an uncropped frame's border is left out of the measurement.
            assert!((0.0..0.35).contains(&n.low[c]), "the thin end: {:?}", n.low);
            let want = ramp_at(1.0 - BORDER_UNCROPPED, 0.2, 1.6);
            assert!(
                close(n.high[c], want, 0.03),
                "the densest measured: {:?}",
                n.high
            );
        }
    }

    /// ⚠⚠ Found in use (2026-10-11, an Ilford HP5 camera scan): the uncropped frame held the black
    /// holder around the film — a quarter of it, densities 3.1 to 4.8 — so the 99.5th percentile
    /// landed there and the picture, at most 1.55, came out black. A scan's surroundings are not film:
    /// what is brighter than the base or denser than film gets is left out, and so is the border of
    /// an uncropped frame.
    #[test]
    fn a_scan_with_its_holder_and_sprocket_holes_measures_the_picture() {
        let (w, h) = (200usize, 300usize);
        let mut img = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
                let px = if !(0.12..0.95).contains(&u) || !(0.12..0.85).contains(&v) {
                    BASE.map(|b| b * 1e-4) // the holder: opaque
                } else if (0.15..0.2).contains(&u) && (v * 20.0).fract() < 0.4 {
                    BASE.map(|b| (b * 1.4).min(1.0)) // a sprocket hole: the bare backlight
                } else if (0.3..0.8).contains(&u) && (0.2..0.7).contains(&v) {
                    let d = 0.25 + 1.25 * (u - 0.3) / 0.5; // the picture
                    BASE.map(|b| b * 10f32.powf(-d))
                } else {
                    BASE // the rebate
                };
                img.set_pixel(x, y, px);
            }
        }
        let mut e = Edits::default();
        assert!(pick(&mut e, &img, [0.25, 0.78]), "the rebate is film base");
        for c in 0..3 {
            assert!(
                close(e.negative.high[c], 1.5, 0.05),
                "the picture's densest, not the holder: {:?}",
                e.negative.high
            );
            assert!(
                e.negative.low[c] >= -0.05,
                "not a sprocket hole: {:?}",
                e.negative.low
            );
        }
    }

    /// ⚠ The border's own job, which the density filter cannot do: an uncropped scan edged by
    /// something film-dense — the next frame, a dark band like the 2.2 one along the HP5 scan's bottom
    /// — must not set the white. ⚠ Asserted against the picture's own densest, never against a value
    /// derived from `BORDER_UNCROPPED`: a sabotaged border of 0 left such a test green.
    #[test]
    fn a_film_dense_edge_of_an_uncropped_scan_does_not_set_the_white() {
        let (w, h) = (200usize, 200usize);
        let mut img = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
                let d = if v > 0.9 {
                    2.4 // a band along the edge: dense, but a density film can have
                } else {
                    0.2 + 1.3 * u.clamp(0.2, 0.8) // the picture, 0.46..1.24
                };
                img.set_pixel(x, y, BASE.map(|b| b * 10f32.powf(-d)));
            }
        }
        let mut e = Edits {
            negative: edit_model::Negative {
                on: true,
                base: BASE,
                ..edit_model::Negative::default()
            },
            ..Edits::default()
        };
        assert!(rebalance(&mut e, &img));
        for d in e.negative.high {
            assert!(
                close(d, 1.24, 0.03),
                "the band set the white: {:?}",
                e.negative.high
            );
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
        let want = ramp_at(1.0 - BORDER_UNCROPPED, 0.1, 0.9);
        assert!(
            e.negative.high.iter().all(|&d| close(d, want, 0.03)),
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

    /// The panel's Film contrast: ±100 around a colour negative's 0.6, and back.
    #[test]
    fn film_contrast_maps_onto_the_films_gamma_and_back() {
        assert_eq!(gamma_of(0.0), 0.6);
        assert!((gamma_of(100.0) - 0.3).abs() < 1e-6, "contrastier");
        assert!((gamma_of(-100.0) - 1.2).abs() < 1e-6, "softer");
        for c in [-100.0, -37.5, 0.0, 12.0, 100.0] {
            assert!((contrast_of(gamma_of(c)) - c).abs() < 1e-3, "{c}");
        }
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
