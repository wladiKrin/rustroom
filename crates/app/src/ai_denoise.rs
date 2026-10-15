//! AI denoise's processing (M10.16b): what the network sees, how strongly it is asked to denoise, and how
//! its answer comes back — everything around the network itself (`ai_mask::denoise`).
//!
//! ⚠⚠ **The network sees a display image, and its answer comes back as a CORRECTION.** DRUNet was
//! trained on sRGB display images, so the linear ProPhoto decode is encoded first (sRGB primaries, a
//! scale putting the 99.5th-percentile luminance at [`WHITE`], the sRGB transfer, clamped to 0…1). What
//! the network changed is turned back into linear light and ADDED to the original pixel, so clipped
//! highlights and out-of-gamut colours — which it saw clamped — keep their real values.

use core_image::ImageBuf;

/// One tile through the network: `rgb` (`w × h`, interleaved, encoded 0…1) denoised at noise level
/// `sigma` (0…255 units), the same layout back. The CPU's `tract` runner and b2's GPU engine both.
pub trait TileNet {
    fn run(&mut self, rgb: &[f32], w: usize, h: usize, sigma: f32) -> Result<Vec<f32>, String>;
}

impl TileNet for ai_mask::denoise::Drunet {
    fn run(&mut self, rgb: &[f32], w: usize, h: usize, sigma: f32) -> Result<Vec<f32>, String> {
        ai_mask::denoise::Drunet::run(self, rgb, w, h, sigma).map_err(|e| e.to_string())
    }
}

/// The tile's side, and how far neighbours overlap.
pub const TILE: usize = 512;
pub const OVERLAP: usize = 32;

/// Where the 99.5th-percentile luminance lands in the encoding.
pub const WHITE: f32 = 0.9;

/// The encoding the network sees.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Encoding {
    /// Linear sRGB is multiplied by this before the transfer.
    pub scale: f32,
}

/// The sRGB transfer, odd-symmetric so an out-of-gamut negative survives the round trip.
fn oetf(v: f32) -> f32 {
    v.signum() * color::linear_to_srgb(v.abs())
}

fn eotf(v: f32) -> f32 {
    v.signum() * color::srgb_to_linear(v.abs())
}

impl Encoding {
    /// The photo's own encoding: its 99.5th-percentile luminance at [`WHITE`], read from a grid of at
    /// most `pipeline::lens_blur::GRID` samples a side (the highlight level's reason: cheap, and the
    /// same at every resolution).
    pub fn of(img: &ImageBuf) -> Self {
        let (w, h) = (img.width(), img.height());
        if w == 0 || h == 0 {
            return Self { scale: 1.0 };
        }
        let step = w.max(h).div_ceil(pipeline::lens_blur::GRID).max(1);
        let at = |g: usize, n: usize| (g * step + step / 2).min(n - 1);
        let mut lum: Vec<f32> = (0..h.div_ceil(step))
            .flat_map(|gy| (0..w.div_ceil(step)).map(move |gx| (gx, gy)))
            .map(|(gx, gy)| color::luminance(img.pixel(at(gx, w), at(gy, h))))
            .filter(|v| v.is_finite())
            .collect();
        if lum.is_empty() {
            return Self { scale: 1.0 };
        }
        let k = ((lum.len() - 1) as f32 * 0.995).round() as usize;
        let (_, l, _) = lum.select_nth_unstable_by(k, f32::total_cmp);
        Self {
            scale: WHITE / l.max(1e-6),
        }
    }

    /// A linear ProPhoto pixel as the network sees it, NOT clamped.
    fn encode_open(&self, p: [f32; 3]) -> [f32; 3] {
        color::prophoto_to_srgb(p).map(|v| oetf(v * self.scale))
    }

    /// A linear ProPhoto pixel as the network sees it, clamped to 0…1.
    pub fn encode(&self, p: [f32; 3]) -> [f32; 3] {
        self.encode_open(p).map(|v| v.clamp(0.0, 1.0))
    }

    /// An encoded pixel back to linear ProPhoto — `encode`'s inverse where nothing clamped.
    pub fn decode(&self, e: [f32; 3]) -> [f32; 3] {
        color::srgb_to_prophoto(e.map(|v| eotf(v) / self.scale))
    }
}

/// The side of the blocks the noise estimate ranks by flatness.
const BLOCK: usize = 16;

/// The photo's noise level in the network's units (0…255 of the encoding), clamped to 1…40.
///
/// ⚠ The standard robust estimate: the finest-scale detail of every channel — the 2 × 2 Haar diagonal
/// `(a − b − c + d) / 2`, whose spread is the noise's own for independent noise — over the flattest
/// quarter of 16 × 16 blocks (ranked by their mean absolute neighbour difference, clipped blocks left
/// out), as a median absolute value × 1.4826.
pub fn noise_sigma(img: &ImageBuf, enc: &Encoding) -> f32 {
    let (w, h) = (img.width(), img.height());
    let (bw, bh) = (w / BLOCK, h / BLOCK);
    if bw == 0 || bh == 0 {
        return 1.0;
    }
    let e = |x: usize, y: usize| enc.encode(img.pixel(x, y));
    let mut blocks: Vec<(f32, usize, usize)> = Vec::with_capacity(bw * bh);
    for by in 0..bh {
        for bx in 0..bw {
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
                blocks.push((act / n, x0, y0));
            }
        }
    }
    if blocks.is_empty() {
        return 1.0;
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
    (*m * 1.4826 * 255.0).clamp(1.0, 40.0)
}

/// Where tiles start along a side of `n`: every `TILE − OVERLAP`, the last flush with the end.
fn starts(n: usize) -> Vec<usize> {
    if n <= TILE {
        return vec![0];
    }
    let mut v: Vec<usize> = (0..)
        .map(|i| i * (TILE - OVERLAP))
        .take_while(|&s| s + TILE < n)
        .collect();
    v.push(n - TILE);
    v
}

/// A tile's weight at local `i` of `t` starting at `s` along a side of `n`: rising over [`OVERLAP`]
/// where another tile continues, 1 at the frame's own edges.
fn ramp(i: usize, t: usize, s: usize, n: usize) -> f32 {
    let mut w = 1.0f32;
    if s > 0 {
        w = w.min((i as f32 + 0.5) / OVERLAP as f32);
    }
    if s + t < n {
        w = w.min((t as f32 - i as f32 - 0.5) / OVERLAP as f32);
    }
    w.clamp(1e-3, 1.0)
}

/// `i` mirrored into `0..t` without repeating the edge — the padding past a tile's end.
fn mirror(i: usize, t: usize) -> usize {
    if i < t {
        i
    } else {
        (2 * t).saturating_sub(i + 2).min(t - 1)
    }
}

/// `img` denoised at `sigma`, tile by tile — `progress(done, total)` after each tile, which answers
/// `false` to cancel (`Ok(None)`).
pub fn denoise(
    img: &ImageBuf,
    net: &mut dyn TileNet,
    sigma: f32,
    progress: &dyn Fn(usize, usize) -> bool,
) -> Result<Option<ImageBuf>, String> {
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return Ok(Some(img.clone()));
    }
    let enc = Encoding::of(img);
    let encoded: Vec<[f32; 3]> = (0..w * h)
        .map(|i| enc.encode(img.pixel(i % w, i / w)))
        .collect();
    let (xs, ys) = (starts(w), starts(h));
    let total = xs.len() * ys.len();
    let mut sum = vec![[0f32; 3]; w * h];
    let mut wsum = vec![0f32; w * h];
    let mut done = 0;
    for &ty in &ys {
        for &tx in &xs {
            let (tw, th) = (TILE.min(w), TILE.min(h));
            let (pw, ph) = (
                tw.div_ceil(ai_mask::denoise::MULTIPLE) * ai_mask::denoise::MULTIPLE,
                th.div_ceil(ai_mask::denoise::MULTIPLE) * ai_mask::denoise::MULTIPLE,
            );
            let mut tile = Vec::with_capacity(pw * ph * 3);
            for y in 0..ph {
                for x in 0..pw {
                    let (sx, sy) = (tx + mirror(x, tw), ty + mirror(y, th));
                    tile.extend_from_slice(&encoded[sy * w + sx]);
                }
            }
            let out = net.run(&tile, pw, ph, sigma)?;
            for y in 0..th {
                let wy = ramp(y, th, ty, h);
                for x in 0..tw {
                    let wt = wy * ramp(x, tw, tx, w);
                    let i = (ty + y) * w + tx + x;
                    let o = (y * pw + x) * 3;
                    for c in 0..3 {
                        sum[i][c] += out[o + c] * wt;
                    }
                    wsum[i] += wt;
                }
            }
            done += 1;
            if !progress(done, total) {
                return Ok(None);
            }
        }
    }
    // The correction back in linear light, added to the ORIGINAL pixel.
    let mut result = img.clone();
    for i in 0..w * h {
        let d = sum[i].map(|v| v / wsum[i]);
        let (x, y) = (i % w, i / w);
        let p = img.pixel(x, y);
        let (to, from) = (enc.decode(d), enc.decode(encoded[i]));
        result.set_pixel(x, y, [0, 1, 2].map(|c| p[c] + to[c] - from[c]));
    }
    Ok(Some(result))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A network that returns what it was given.
    struct Identity;
    impl TileNet for Identity {
        fn run(&mut self, rgb: &[f32], _: usize, _: usize, _: f32) -> Result<Vec<f32>, String> {
            Ok(rgb.to_vec())
        }
    }

    /// A network that adds `0.02` to every value.
    struct Lift;
    impl TileNet for Lift {
        fn run(&mut self, rgb: &[f32], w: usize, h: usize, _: f32) -> Result<Vec<f32>, String> {
            assert!(
                w % 8 == 0 && h % 8 == 0,
                "a {w} × {h} tile reached the network"
            );
            Ok(rgb.iter().map(|v| v + 0.02).collect())
        }
    }

    fn image(w: usize, h: usize, mut f: impl FnMut(usize, usize) -> [f32; 3]) -> ImageBuf {
        let mut img = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                img.set_pixel(x, y, f(x, y));
            }
        }
        img
    }

    /// A deterministic Gaussian: Box–Muller over a linear congruential stream.
    struct Gauss(u32);
    impl Gauss {
        fn uniform(&mut self) -> f32 {
            self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            ((self.0 >> 8) as f32 + 0.5) / (1u32 << 24) as f32
        }
        fn next(&mut self) -> f32 {
            let (a, b) = (self.uniform(), self.uniform());
            (-2.0 * a.ln()).sqrt() * (std::f32::consts::TAU * b).cos()
        }
    }

    #[test]
    fn the_encoding_round_trips_where_nothing_clips() {
        let enc = Encoding { scale: 2.0 };
        // ⚠ Built INSIDE sRGB: a ProPhoto colour outside it clamps by design (the next test's case).
        for srgb in [
            [0.05, 0.1, 0.2],
            [0.3, 0.25, 0.2],
            [0.0, 0.0, 0.0],
            [0.4, 0.4, 0.4],
        ] {
            let p = color::srgb_to_prophoto(srgb);
            let back = enc.decode(enc.encode(p));
            for c in 0..3 {
                assert!((back[c] - p[c]).abs() < 1e-5, "{p:?} → {back:?}");
            }
        }
        let e = enc.encode([0.2, 0.2, 0.2]);
        assert!(e.iter().all(|v| (0.0..=1.0).contains(v)));
    }

    /// Review Focus 3: an identity network changes nothing — clipped highlights at three times white
    /// and a ProPhoto green no sRGB can hold included.
    #[test]
    fn clipped_and_out_of_gamut_values_pass_through() {
        let img = image(48, 40, |x, _| match x % 3 {
            0 => [0.2, 0.15, 0.1],
            1 => [3.0, 3.0, 3.0],
            _ => [0.0, 0.6, 0.0],
        });
        let out = denoise(&img, &mut Identity, 8.0, &|_, _| true)
            .expect("runs")
            .expect("not cancelled");
        for (a, b) in img.data().iter().zip(out.data()) {
            assert!((a - b).abs() <= 1e-5 * a.abs().max(1.0), "{a} → {b}");
        }
    }

    /// Gaussian noise of a known σ in the encoding reads back within 10 %; a clean gradient ≈ 0.
    #[test]
    fn noise_sigma_reads_known_noise() {
        let enc = Encoding { scale: 1.5 };
        let mut g = Gauss(7);
        let noisy = image(400, 300, |_, _| {
            enc.decode([0, 1, 2].map(|_| 0.4 + 8.0 / 255.0 * g.next()))
        });
        let s = noise_sigma(&noisy, &enc);
        assert!((s - 8.0).abs() < 0.8, "{s}");
        let clean = image(400, 300, |x, y| {
            enc.decode([(x + y) as f32 / 1400.0 + 0.1; 3])
        });
        assert!(noise_sigma(&clean, &enc) <= 1.5);
    }

    /// Review Focus 4: sizes no tile or multiple of 8 divides — every pixel gets the same correction,
    /// no seam, and no tile reaches the network at a size it cannot run.
    #[test]
    fn odd_sizes_tile_seamlessly() {
        for (w, h) in [(301, 203), (1100, 700), (5, 3)] {
            let img = image(w, h, |x, y| {
                [0.1 + 0.0002 * x as f32, 0.12, 0.08 + 0.0003 * y as f32]
            });
            let enc = Encoding::of(&img);
            let out = denoise(&img, &mut Lift, 8.0, &|_, _| true)
                .expect("runs")
                .expect("not cancelled");
            for y in 0..h {
                for x in 0..w {
                    let p = img.pixel(x, y);
                    let e = enc.encode(p);
                    let want = enc.decode(e.map(|v| v + 0.02));
                    let base = enc.decode(e);
                    let got = out.pixel(x, y);
                    for c in 0..3 {
                        let expect = p[c] + want[c] - base[c];
                        assert!(
                            (got[c] - expect).abs() < 1e-5,
                            "{w}×{h} at ({x}, {y})[{c}]: {} vs {expect}",
                            got[c]
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn progress_counts_tiles_and_cancel_stops() {
        let img = image(1100, 700, |_, _| [0.2; 3]);
        let calls = std::cell::RefCell::new(Vec::new());
        let done = denoise(&img, &mut Identity, 8.0, &|d, t| {
            calls.borrow_mut().push((d, t));
            true
        })
        .expect("runs");
        assert!(done.is_some());
        let seen = calls.borrow().clone();
        assert_eq!(seen.last(), Some(&(6, 6)), "3 × 2 tiles: {seen:?}");
        let cancelled = denoise(&img, &mut Identity, 8.0, &|d, _| d < 2).expect("runs");
        assert!(
            cancelled.is_none(),
            "the answer after the second tile stops it"
        );
    }
}
