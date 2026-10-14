//! Turning a photo into a network's input, and its output into a [`ProbMap`] — pure functions, the
//! recipes the M9.13 spike verified.

use crate::map::ProbMap;
use image::RgbImage;

/// ImageNet statistics — BiRefNet's normalisation.
pub const IMAGENET_MEAN: [f32; 3] = [0.485, 0.456, 0.406];
pub const IMAGENET_STD: [f32; 3] = [0.229, 0.224, 0.225];

/// `img` resized to `side × side`, as NCHW `(p/255 − mean) / std`.
pub fn nchw(img: &RgbImage, side: u32, mean: [f32; 3], std: [f32; 3]) -> Vec<f32> {
    nchw_wh(img, side, side, mean, std)
}

/// `img` resized to `w × h`, as NCHW `(p/255 − mean) / std` — [`nchw`] without the square.
pub fn nchw_wh(img: &RgbImage, w: u32, h: u32, mean: [f32; 3], std: [f32; 3]) -> Vec<f32> {
    let small = image::imageops::resize(img, w, h, image::imageops::FilterType::Triangle);
    let plane = (w * h) as usize;
    let mut out = vec![0f32; 3 * plane];
    for (x, y, p) in small.enumerate_pixels() {
        let i = (y * w + x) as usize;
        for c in 0..3 {
            out[c * plane + i] = (p[c] as f32 / 255.0 - mean[c]) / std[c];
        }
    }
    out
}

/// The size `(w, h)` with the long side scaled to `long` — SAM's `ResizeLongestSide`.
pub fn long_side(w: u32, h: u32, long: u32) -> (u32, u32) {
    let s = long as f32 / w.max(h).max(1) as f32;
    (
        ((w as f32 * s) + 0.5).floor().max(1.0) as u32,
        ((h as f32 * s) + 0.5).floor().max(1.0) as u32,
    )
}

/// `img` with its long side at 1024, as HWC RGB 0–255 — the MobileSAM encoder's input.
pub fn hwc_long_side(img: &RgbImage) -> (Vec<f32>, u32, u32) {
    let (rw, rh) = long_side(img.width(), img.height(), 1024);
    let small = image::imageops::resize(img, rw, rh, image::imageops::FilterType::Triangle);
    (
        small.into_raw().into_iter().map(f32::from).collect(),
        rw,
        rh,
    )
}

pub fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

/// A `side × side` plane of values as a map of `w × h`, through `f`.
pub fn square_to_map(plane: &[f32], side: u32, w: u32, h: u32, f: impl Fn(f32) -> f32) -> ProbMap {
    let sq = ProbMap::from_fn(side, side, |x, y| f(plane[(y * side + x) as usize]));
    ProbMap::from_fn(w, h, |x, y| {
        sq.at((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32)
    })
}

/// SAM's `low_res_masks` (256² logits) as a map of the resized image `rw × rh`.
///
/// ⚠⚠ **Not the decoder's full-size `masks` output**: segment-anything 1.0's ONNX wrapper freezes that
/// crop to the landscape dummy used at export, so it is wrong for every portrait — a click on a tower
/// selected a column at 0.42 of the width where the tower is at 0.60 (M9.13 spike). This is SAM's
/// own postprocessing: upscale to 1024², crop `[0..rh, 0..rw]`, logits through a sigmoid.
pub fn sam_low_res_to_map(low: &[f32], rw: u32, rh: u32) -> ProbMap {
    assert_eq!(low.len(), 256 * 256, "low_res_masks is 256²");
    let sq = ProbMap::from_fn(256, 256, |x, y| sigmoid(low[(y * 256 + x) as usize]));
    // Pixel (x, y) of the resized image sits at (x, y) of the 1024² canvas, i.e. (x/1024, y/1024).
    ProbMap::from_fn(rw, rh, |x, y| {
        sq.at((x as f32 + 0.5) / 1024.0, (y as f32 + 0.5) / 1024.0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nchw_normalises_per_channel_into_planes() {
        let img = RgbImage::from_pixel(4, 4, image::Rgb([255, 0, 128]));
        let v = nchw(&img, 2, [0.5; 3], [0.5; 3]);
        assert_eq!(v.len(), 3 * 4);
        assert!((v[0] - 1.0).abs() < 1e-6, "R plane: (1 − 0.5)/0.5");
        assert!((v[4] + 1.0).abs() < 1e-6, "G plane: (0 − 0.5)/0.5");
        assert!(v[8].abs() < 0.01, "B plane: ~0");
    }

    /// M10.16a: the depth network takes the photo at its own aspect, not squashed to a square.
    #[test]
    fn nchw_wh_resizes_to_a_non_square_size_in_planes() {
        // Left half white, right half black.
        let img = RgbImage::from_fn(40, 20, |x, _| {
            if x < 20 {
                image::Rgb([255, 255, 255])
            } else {
                image::Rgb([0, 0, 0])
            }
        });
        let v = nchw_wh(&img, 4, 2, [0.0; 3], [1.0; 3]);
        assert_eq!(v.len(), 3 * 4 * 2);
        // Plane 0, row 1: white … black, the two middle columns blended by the Triangle filter
        // (measured 0.87 / 0.13) — so the layout is asserted on the outer columns and the order.
        let r = &v[4..8];
        assert!(r[0] > 0.95 && r[3] < 0.05, "{r:?}");
        assert!(r[1] > r[2] + 0.5, "{r:?}");
        // The blue plane starts at 2·w·h and repeats the pattern.
        assert!(v[16] > 0.95 && v[19] < 0.05);
    }

    #[test]
    fn the_long_side_is_1024_for_either_orientation() {
        assert_eq!(long_side(6240, 4160, 1024), (1024, 683));
        assert_eq!(long_side(4160, 6240, 1024), (683, 1024));
    }

    /// ⚠ The portrait case is the one the frozen-crop trap broke, so it is the one tested.
    #[test]
    fn low_res_masks_crop_to_a_portrait_image_correctly() {
        // Positive logits only in the canvas columns 0..341 (a third of 1024), rows everywhere.
        let low: Vec<f32> = (0..256 * 256)
            .map(|i| if (i % 256) < 85 { 8.0 } else { -8.0 })
            .collect();
        let m = sam_low_res_to_map(&low, 683, 1024);
        assert_eq!((m.w, m.h), (683, 1024));
        // Column 300 of a 683-wide image is canvas column 300 — inside the positive third.
        assert!(
            m.at(300.5 / 683.0, 0.5) > 0.9,
            "{}",
            m.at(300.5 / 683.0, 0.5)
        );
        // Column 500 is canvas column 500 — outside it. A stretched mapping would put it at 750.
        assert!(m.at(500.5 / 683.0, 0.5) < 0.1);
    }
}
