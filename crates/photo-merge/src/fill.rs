//! Fill edges (M10.17b): the panorama's empty outline filled from its surroundings by an inpainting network
//! (LaMa in the app, M10.16c1), window by window along the outline.
//!
//! The network sees a fixed square ([`Inpainter::side`]); a panorama's margin is long and thin, so each window
//! is a square of the canvas around the outline, grown until its hole is at most a third of it, resampled to
//! the network's side, filled, and the fill resampled back into the hole only. Filled pixels count as picture
//! for the windows after. The network's fill is smoother than the photo around it, so it is given noise whose
//! strength, per channel, matches the valid pixels' fine detail in that window.
//!
//! ⚠ `photo-merge` never depends on `ai-mask`: the network is handed in as an [`Inpainter`].

use crate::crop::Valid;
use crate::hdr::MergeError;
use core_image::ImageBuf;
use std::sync::atomic::{AtomicBool, Ordering};

/// An inpainting network.
pub trait Inpainter {
    /// The side of the square it fills.
    fn side(&self) -> usize;
    /// `window` (`side × side`, normalised raw camera RGB) filled where `hole` (`side²`, 1 = fill) says.
    fn fill(&mut self, window: &ImageBuf, hole: &[f32]) -> Result<ImageBuf, String>;
}

/// A window's hole is at most this share of it.
pub const MAX_HOLE: f32 = 1.0 / 3.0;

/// Every invalid pixel of `img` filled by `p`; `v` is updated as it goes (all valid at the end).
pub fn fill_edges(
    img: &mut ImageBuf,
    v: &mut Valid,
    p: &mut dyn Inpainter,
    cancel: &AtomicBool,
) -> Result<(), MergeError> {
    let (w, h) = (v.width, v.height);
    if v.bits.iter().all(|&b| b) || w == 0 || h == 0 {
        return Ok(());
    }
    let side = p.side();
    let limit = w.min(h);
    // A strip of hole `t` deep needs a window at least ~3t across for its hole to stay under a third.
    let start = (side as f32).max(3.2 * hole_depth(v) as f32).ceil() as usize;
    let start = start.min(limit).max(1);
    let step = (start * 3 / 4).max(1);
    let mut rng = 0x2545_F491_4F6C_DD1Du64;
    loop {
        let mut progress = false;
        for cy in (0..h).step_by(step) {
            for cx in (0..w).step_by(step) {
                // A square around (cx, cy), grown until its hole is at most a third, kept on the canvas.
                let centre = (cx + start / 2, cy + start / 2);
                let mut size = start;
                let rect = loop {
                    let r = place(centre, size, w, h);
                    let (holes, total) = count(v, r);
                    if holes == 0 || (holes as f32) <= MAX_HOLE * total as f32 || size >= limit {
                        break (r, holes, total);
                    }
                    size = (size * 5 / 4).max(size + 1).min(limit);
                };
                let (r, holes, total) = rect;
                if holes == 0 || holes == total {
                    continue;
                }
                if cancel.load(Ordering::Relaxed) {
                    return Err(MergeError::Cancelled);
                }
                fill_window(img, v, p, r, side, &mut rng)?;
                progress = true;
            }
        }
        if v.bits.iter().all(|&b| b) || !progress {
            return Ok(());
        }
    }
}

/// `[x0, y0, x1, y1)` of a `size` square centred at `c`, moved to lie on the `w × h` canvas.
fn place(c: (usize, usize), size: usize, w: usize, h: usize) -> [usize; 4] {
    let (sw, sh) = (size.min(w), size.min(h));
    let x0 = c.0.saturating_sub(sw / 2).min(w - sw);
    let y0 = c.1.saturating_sub(sh / 2).min(h - sh);
    [x0, y0, x0 + sw, y0 + sh]
}

fn count(v: &Valid, r: [usize; 4]) -> (usize, usize) {
    let mut holes = 0;
    for y in r[1]..r[3] {
        holes += v.bits[y * v.width + r[0]..y * v.width + r[2]]
            .iter()
            .filter(|&&b| !b)
            .count();
    }
    (holes, (r[2] - r[0]) * (r[3] - r[1]))
}

/// The deepest hole pixel's distance to the picture (8-connected steps).
fn hole_depth(v: &Valid) -> usize {
    let (w, h) = (v.width, v.height);
    let far = usize::MAX / 2;
    let mut d: Vec<usize> = v.bits.iter().map(|&b| if b { 0 } else { far }).collect();
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            for (dx, dy) in [(-1i64, 0i64), (-1, -1), (0, -1), (1, -1)] {
                let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                if nx >= 0 && ny >= 0 && nx < w as i64 {
                    d[i] = d[i].min(d[ny as usize * w + nx as usize] + 1);
                }
            }
        }
    }
    for y in (0..h).rev() {
        for x in (0..w).rev() {
            let i = y * w + x;
            for (dx, dy) in [(1i64, 0i64), (1, 1), (0, 1), (-1, 1)] {
                let (nx, ny) = (x as i64 + dx, y as i64 + dy);
                if nx >= 0 && nx < w as i64 && ny < h as i64 {
                    d[i] = d[i].min(d[ny as usize * w + nx as usize] + 1);
                }
            }
        }
    }
    d.into_iter().filter(|&x| x < far).max().unwrap_or(0)
}

/// One window: down (or up) to the network's side, filled, back into the hole only, with noise.
fn fill_window(
    img: &mut ImageBuf,
    v: &mut Valid,
    p: &mut dyn Inpainter,
    r: [usize; 4],
    side: usize,
    rng: &mut u64,
) -> Result<(), MergeError> {
    let (ww, wh) = (r[2] - r[0], r[3] - r[1]);
    let span = |i: usize, n: usize, origin: usize| {
        let a = origin + i * n / side;
        (a, (origin + (i + 1) * n / side).max(a + 1))
    };
    let mut window = ImageBuf::new(side, side);
    let mut hole = vec![0f32; side * side];
    for j in 0..side {
        let (ya, yb) = span(j, wh, r[1]);
        for i in 0..side {
            let (xa, xb) = span(i, ww, r[0]);
            let (mut sum, mut n, mut gaps) = ([0f32; 3], 0f32, 0usize);
            for y in ya..yb {
                for x in xa..xb {
                    if v.at(x, y) {
                        let q = img.pixel(x, y);
                        (0..3).for_each(|c| sum[c] += q[c]);
                        n += 1.0;
                    } else {
                        gaps += 1;
                    }
                }
            }
            if gaps * 2 > (yb - ya) * (xb - xa) {
                hole[j * side + i] = 1.0;
            }
            window.set_pixel(i, j, sum.map(|s| if n > 0.0 { s / n } else { 0.0 }));
        }
    }
    let out = p.fill(&window, &hole).map_err(MergeError::Fill)?;
    // The picture's fine detail per channel: neighbouring valid pixels' differences, /√2.
    let (mut s2, mut n) = ([0f64; 3], 0f64);
    for y in r[1]..r[3] {
        for x in r[0]..r[2] - 1 {
            if v.at(x, y) && v.at(x + 1, y) {
                let (a, b) = (img.pixel(x, y), img.pixel(x + 1, y));
                (0..3).for_each(|c| s2[c] += ((a[c] - b[c]) as f64).powi(2) / 2.0);
                n += 1.0;
            }
        }
    }
    let sigma = s2.map(|s| if n > 0.0 { (s / n).sqrt() as f32 } else { 0.0 });
    let mut gauss = || {
        let mut next = || {
            *rng = rng.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = *rng;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            ((z ^ (z >> 31)) >> 11) as f64 / (1u64 << 53) as f64
        };
        let (a, b) = (next().max(1e-12), next());
        ((-2.0 * a.ln()).sqrt() * (std::f64::consts::TAU * b).cos()) as f32
    };
    for y in r[1]..r[3] {
        for x in r[0]..r[2] {
            if v.at(x, y) {
                continue;
            }
            // Bilinear in the network's square, at this pixel's centre.
            let tx = ((x - r[0]) as f32 + 0.5) * side as f32 / ww as f32 - 0.5;
            let ty = ((y - r[1]) as f32 + 0.5) * side as f32 / wh as f32 - 0.5;
            let (tx, ty) = (
                tx.clamp(0.0, (side - 1) as f32),
                ty.clamp(0.0, (side - 1) as f32),
            );
            let (x0, y0) = (tx.floor() as usize, ty.floor() as usize);
            let (x1, y1) = ((x0 + 1).min(side - 1), (y0 + 1).min(side - 1));
            let (fx, fy) = (tx - x0 as f32, ty - y0 as f32);
            let (a, b, c, d) = (
                out.pixel(x0, y0),
                out.pixel(x1, y0),
                out.pixel(x0, y1),
                out.pixel(x1, y1),
            );
            let q: [f32; 3] = std::array::from_fn(|k| {
                let top = a[k] * (1.0 - fx) + b[k] * fx;
                let bot = c[k] * (1.0 - fx) + d[k] * fx;
                top * (1.0 - fy) + bot * fy + sigma[k] * gauss()
            });
            img.set_pixel(x, y, q);
            v.bits[y * v.width + x] = true;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::Noise;

    /// The stand-in network: every hole pixel the mean of the window's picture — flat, as smooth as a fill
    /// can be. It records each window's hole share.
    struct Mean {
        holes: Vec<f32>,
        cancel_after: Option<(usize, &'static AtomicBool)>,
    }

    impl Inpainter for Mean {
        fn side(&self) -> usize {
            64
        }
        fn fill(&mut self, window: &ImageBuf, hole: &[f32]) -> Result<ImageBuf, String> {
            self.holes
                .push(hole.iter().filter(|&&h| h > 0.5).count() as f32 / hole.len() as f32);
            if let Some((n, flag)) = self.cancel_after {
                if self.holes.len() >= n {
                    flag.store(true, Ordering::Relaxed);
                }
            }
            let (mut sum, mut n) = ([0f32; 3], 0f32);
            for (i, h) in hole.iter().enumerate() {
                if *h <= 0.5 {
                    let px = window.pixel(i % window.width(), i / window.width());
                    (0..3).for_each(|c| sum[c] += px[c]);
                    n += 1.0;
                }
            }
            let mean = sum.map(|s| s / n.max(1.0));
            let mut out = window.clone();
            for (i, h) in hole.iter().enumerate() {
                if *h > 0.5 {
                    out.set_pixel(i % window.width(), i / window.width(), mean);
                }
            }
            Ok(out)
        }
    }

    /// A noisy flat picture (σ 0.01 a channel) inside a ragged outline: a top margin up to 30 px deep, a
    /// left one up to 25, both corners empty.
    fn picture(w: usize, h: usize) -> (ImageBuf, Valid) {
        let mut rng = Noise(5);
        let mut img = ImageBuf::new(w, h);
        let bits: Vec<bool> = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f64, (i / w) as f64);
                y > 15.0 + 15.0 * (x / 37.0).sin() && x > 12.0 + 13.0 * (y / 23.0).cos()
            })
            .collect();
        for (i, _) in bits.iter().enumerate().filter(|b| *b.1) {
            let p = [0, 1, 2].map(|c| 0.2 + 0.05 * c as f32 + 0.01 * rng.gauss() as f32);
            img.set_pixel(i % w, i / w, p);
        }
        (
            img,
            Valid {
                width: w,
                height: h,
                bits,
            },
        )
    }

    fn mean_net() -> Mean {
        Mean {
            holes: Vec::new(),
            cancel_after: None,
        }
    }

    #[test]
    fn every_hole_pixel_is_filled_and_valid_pixels_are_untouched() {
        let (mut img, mut v) = picture(400, 260);
        let before = (img.clone(), v.clone());
        fill_edges(&mut img, &mut v, &mut mean_net(), &AtomicBool::new(false)).expect("fills");
        assert!(v.bits.iter().all(|&b| b), "every pixel valid after");
        for i in 0..v.bits.len() {
            let (x, y) = (i % v.width, i / v.width);
            if before.1.bits[i] {
                assert_eq!(img.pixel(x, y), before.0.pixel(x, y), "({x}, {y}) moved");
            } else {
                let p = img.pixel(x, y);
                assert!((p[1] - 0.25).abs() < 0.06, "({x}, {y}) filled with {p:?}");
            }
        }
    }

    #[test]
    fn the_fill_has_the_neighbours_noise() {
        let (mut img, mut v) = picture(400, 260);
        let holes = v.clone();
        fill_edges(&mut img, &mut v, &mut mean_net(), &AtomicBool::new(false)).unwrap();
        // The fine detail (a pixel minus its right neighbour, /√2) of the fill against the picture's.
        let sigma = |want_hole: bool| {
            let (mut s, mut n) = (0f64, 0f64);
            for y in 0..img.height() {
                for x in 0..img.width() - 1 {
                    let i = y * img.width() + x;
                    if holes.bits[i] == !want_hole && holes.bits[i + 1] == !want_hole {
                        let d = (img.pixel(x, y)[1] - img.pixel(x + 1, y)[1]) as f64 / 2f64.sqrt();
                        s += d * d;
                        n += 1.0;
                    }
                }
            }
            (s / n).sqrt()
        };
        let (fill, photo) = (sigma(true), sigma(false));
        assert!(
            fill > photo / 1.5 && fill < photo * 1.5,
            "fill σ {fill}, picture σ {photo}"
        );
    }

    #[test]
    fn no_window_holds_more_hole_than_a_third() {
        let (mut img, mut v) = picture(400, 260);
        let mut net = mean_net();
        fill_edges(&mut img, &mut v, &mut net, &AtomicBool::new(false)).unwrap();
        assert!(!net.holes.is_empty());
        let worst = net.holes.iter().cloned().fold(0f32, f32::max);
        // The share at the network's side, after resampling — a few points above the canvas's own.
        assert!(worst <= MAX_HOLE + 0.03, "a window {worst} hole");
    }

    #[test]
    fn a_cancel_stops_the_walk() {
        static FLAG: AtomicBool = AtomicBool::new(false);
        let (mut img, mut v) = picture(400, 260);
        let mut net = Mean {
            holes: Vec::new(),
            cancel_after: Some((1, &FLAG)),
        };
        let r = fill_edges(&mut img, &mut v, &mut net, &FLAG);
        assert!(matches!(r, Err(MergeError::Cancelled)), "{r:?}");
        assert_eq!(net.holes.len(), 1, "no window after the cancel");
    }

    #[test]
    fn a_full_picture_needs_no_window() {
        let (mut img, _) = picture(80, 60);
        let mut v = Valid {
            width: 80,
            height: 60,
            bits: vec![true; 4800],
        };
        let mut net = mean_net();
        fill_edges(&mut img, &mut v, &mut net, &AtomicBool::new(false)).unwrap();
        assert!(net.holes.is_empty());
    }
}
