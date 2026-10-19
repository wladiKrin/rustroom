//! AI denoise's strength (M10.16b3): the noise a photo carries at each brightness, and a tile's σ plane
//! from it — so the shadows, which carry more, are given more. Measured by `ai_denoise`'s own estimator,
//! band by band.

use crate::ai_denoise::{binned, noise_sigma_at, Encoding, BLOCK, NOISE_BIN};
use core_image::ImageBuf;

/// The bands of encoded brightness the curve has — eighths of 0…1.
pub const BANDS: usize = 8;

/// σ for each band, at one pixel, in DRUNet's 0…255 units — uncalibrated.
pub type Curve = [f32; BANDS];

/// A band needs this many blocks to be read; fewer, and it borrows a neighbour's.
const MIN_BLOCKS: usize = 8;

/// The 15 px box the map's brightness is averaged over (radius 7) — the noise must not set it.
const BLUR: usize = 7;

/// The noise `img` carries at each eighth of brightness — `ai_denoise::noise_sigma_at`'s estimator (2 × 2
/// binned, 16 px blocks, the flattest quarter, the median Haar diagonal) on each band's own blocks.
///
/// ⚠ A band with too few blocks, or reading under 0.5 (clipped highlights carry no noise), takes the
/// nearest read band's σ, the darker first; with no band read, every band is the whole photo's estimate.
pub fn noise_curve(img: &ImageBuf, enc: &Encoding) -> Curve {
    let small = binned(img, NOISE_BIN);
    let (w, h) = (small.width(), small.height());
    let e = |x: usize, y: usize| enc.encode(small.pixel(x, y));
    let mut bands: Vec<Vec<(f32, usize, usize)>> = vec![Vec::new(); BANDS];
    for by in 0..h / BLOCK {
        for bx in 0..w / BLOCK {
            let (x0, y0) = (bx * BLOCK, by * BLOCK);
            let (mut act, mut mean, mut n) = (0.0f32, 0.0f32, 0.0f32);
            for y in y0..y0 + BLOCK - 1 {
                for x in x0..x0 + BLOCK - 1 {
                    let (p, r, d) = (e(x, y), e(x + 1, y), e(x, y + 1));
                    for c in 0..3 {
                        act += (p[c] - r[c]).abs() + (p[c] - d[c]).abs();
                        mean += p[c];
                    }
                    n += 3.0;
                }
            }
            let mean = mean / n;
            if (0.02..=0.98).contains(&mean) {
                let b = ((mean * BANDS as f32) as usize).min(BANDS - 1);
                bands[b].push((act / n, x0, y0));
            }
        }
    }
    let read: Vec<Option<f32>> = bands
        .into_iter()
        .map(|mut blocks| {
            if blocks.len() < MIN_BLOCKS {
                return None;
            }
            blocks.sort_by(|a, b| a.0.total_cmp(&b.0));
            blocks.truncate(blocks.len().div_ceil(4));
            let mut coeffs: Vec<f32> = Vec::new();
            for &(_, x0, y0) in &blocks {
                for y in (y0..y0 + BLOCK).step_by(2) {
                    for x in (x0..x0 + BLOCK).step_by(2) {
                        let (a, b, c, d) = (e(x, y), e(x + 1, y), e(x, y + 1), e(x + 1, y + 1));
                        for k in 0..3 {
                            coeffs.push(((a[k] - b[k] - c[k] + d[k]) / 2.0).abs());
                        }
                    }
                }
            }
            let mid = coeffs.len() / 2;
            let (_, m, _) = coeffs.select_nth_unstable_by(mid, f32::total_cmp);
            Some(*m * 1.4826 * 255.0 * NOISE_BIN as f32).filter(|s| *s >= 0.5)
        })
        .collect();
    if read.iter().all(Option::is_none) {
        return [noise_sigma_at(img, enc, NOISE_BIN); BANDS];
    }
    std::array::from_fn(|b| {
        (0..BANDS)
            .flat_map(|d| [b.checked_sub(d), Some(b + d)])
            .flatten()
            .find_map(|i| read.get(i).copied().flatten())
            .expect("a band was read")
    })
}

/// The curve at encoded brightness `v` — linear between the bands' centres, flat past the ends.
pub fn curve_at(c: &Curve, v: f32) -> f32 {
    let x = (v * BANDS as f32 - 0.5).clamp(0.0, (BANDS - 1) as f32);
    let (i, t) = (x.floor() as usize, x.fract());
    let j = (i + 1).min(BANDS - 1);
    c[i] * (1.0 - t) + c[j] * t
}

/// A tile's σ plane: each pixel's brightness (`rgb` interleaved, encoded 0…1) averaged over 15 px, the
/// curve there × `factor`, clamped 1…40 — DRUNet's own range.
pub fn noise_map(rgb: &[f32], w: usize, h: usize, c: &Curve, factor: f32) -> Vec<f32> {
    // A summed-area table of the luma, one row and column of zeros ahead.
    let mut sat = vec![0f64; (w + 1) * (h + 1)];
    for y in 0..h {
        let mut row = 0f64;
        for x in 0..w {
            let i = (y * w + x) * 3;
            row += ((rgb[i] + rgb[i + 1] + rgb[i + 2]) / 3.0) as f64;
            sat[(y + 1) * (w + 1) + x + 1] = sat[y * (w + 1) + x + 1] + row;
        }
    }
    let mut out = Vec::with_capacity(w * h);
    for y in 0..h {
        let (y0, y1) = (y.saturating_sub(BLUR), (y + BLUR + 1).min(h));
        for x in 0..w {
            let (x0, x1) = (x.saturating_sub(BLUR), (x + BLUR + 1).min(w));
            let s = sat[y1 * (w + 1) + x1] - sat[y0 * (w + 1) + x1] - sat[y1 * (w + 1) + x0]
                + sat[y0 * (w + 1) + x0];
            let v = (s / ((x1 - x0) * (y1 - y0)) as f64) as f32;
            out.push((curve_at(c, v) * factor).clamp(1.0, 40.0));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(w: usize, h: usize, mut f: impl FnMut(usize, usize) -> [f32; 3]) -> ImageBuf {
        let mut img = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                img.set_pixel(x, y, f(x, y));
            }
        }
        img
    }

    /// A ramp of brightness whose noise falls as it brightens: the curve reads each band's noise within
    /// 20 %, and falls.
    #[test]
    fn noise_curve_tracks_noise_rising_toward_the_shadows() {
        let enc = Encoding { scale: 1.0 };
        let (w, h) = (1024, 512);
        let mut rng = 0x1234_5678u32;
        let mut gauss = || {
            let mut u = || {
                rng ^= rng << 13;
                rng ^= rng >> 17;
                rng ^= rng << 5;
                (rng as f32 + 1.0) / (u32::MAX as f32 + 2.0)
            };
            let (a, b) = (u(), u());
            (-2.0 * a.ln()).sqrt() * (std::f32::consts::TAU * b).cos()
        };
        let want = |v: f32| 12.0 - 8.0 * v; // σ in 0…255 units, 12 at black, 4 at white
        let img = image(w, h, |x, _| {
            let v = 0.05 + 0.9 * x as f32 / w as f32;
            let s = want(v) / 255.0;
            enc.decode([v + s * gauss(), v + s * gauss(), v + s * gauss()])
        });
        let c = noise_curve(&img, &enc);
        for (b, &cb) in c.iter().enumerate().take(BANDS - 1).skip(1) {
            let v = (b as f32 + 0.5) / BANDS as f32;
            assert!(
                (cb - want(v)).abs() < 0.2 * want(v),
                "band {b}: {cb} vs {}",
                want(v)
            );
        }
        assert!(c[1] > c[6], "the curve does not fall: {c:?}");
    }

    /// Review Focus 1: one band of blocks — every band filled from it, none NaN.
    #[test]
    fn a_photo_with_one_band_fills_every_band() {
        let enc = Encoding { scale: 1.0 };
        let img = image(256, 256, |x, y| {
            let n = ((x * 7 + y * 13) % 11) as f32 / 255.0;
            enc.decode([0.5 + n, 0.5 + n, 0.5 + n])
        });
        let c = noise_curve(&img, &enc);
        assert!(c.iter().all(|v| v.is_finite() && *v >= 0.5), "{c:?}");
        assert!(
            c.iter().all(|v| (v - c[4]).abs() < 1e-6),
            "all from the one band: {c:?}"
        );
    }

    #[test]
    fn a_flat_curve_gives_a_flat_map() {
        let rgb: Vec<f32> = (0..32 * 16 * 3).map(|i| (i % 97) as f32 / 97.0).collect();
        let m = noise_map(&rgb, 32, 16, &[5.0; BANDS], 2.0);
        assert_eq!(m.len(), 32 * 16);
        assert!(m.iter().all(|v| (v - 10.0).abs() < 1e-5));
    }

    /// Review Focus 2: a lone bright pixel in the dark keeps the dark's σ — itself too, which is what the
    /// blur is for (unblurred, its own σ would drop from 20 to 6; a neighbour's would not move either way).
    #[test]
    fn the_map_reads_blurred_brightness() {
        let (w, h) = (64, 64);
        let mut rgb = vec![0.05; w * h * 3];
        let i = 32 * w + 32;
        rgb[i * 3..i * 3 + 3].fill(1.0);
        let c: Curve = [20.0, 18.0, 16.0, 14.0, 12.0, 10.0, 8.0, 6.0];
        let m = noise_map(&rgb, w, h, &c, 1.0);
        assert!((m[i] - m[0]).abs() < 0.5, "the pixel: {} vs {}", m[i], m[0]);
        assert!(
            (m[i + 1] - m[0]).abs() < 0.5,
            "its neighbour: {} vs {}",
            m[i + 1],
            m[0]
        );
    }

    /// Review Focus 5: the 2× never takes σ past 40, nor under 1.
    #[test]
    fn the_map_is_clamped() {
        let rgb = vec![0.5; 8 * 8 * 3];
        assert!(noise_map(&rgb, 8, 8, &[30.0; BANDS], 2.0)
            .iter()
            .all(|v| *v == 40.0));
        assert!(noise_map(&rgb, 8, 8, &[0.1; BANDS], 2.0)
            .iter()
            .all(|v| *v == 1.0));
    }
}
