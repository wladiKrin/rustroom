//! The panorama's outline (M10.17b): which canvas pixels any frame covers, Auto crop's rectangle inside
//! them, and the edge extension that keeps the rest from ever being black.
//!
//! ⚠ Black margins are never left in the DNG even when cropped away: Develop's prefix (noise reduction,
//! dehaze, lens blur) runs BEFORE the crop, and would pull black into the picture along the crop's edge.

use core_image::ImageBuf;

/// Which canvas pixels hold the picture.
#[derive(Clone, Debug, PartialEq)]
pub struct Valid {
    pub width: usize,
    pub height: usize,
    pub bits: Vec<bool>,
}

impl Valid {
    pub fn at(&self, x: usize, y: usize) -> bool {
        self.bits[y * self.width + x]
    }
}

/// The largest axis-aligned rectangle of valid pixels, `[left, top, right, bottom)` — `None` when no pixel
/// is valid. The histogram method: per row, each column's run of valid pixels above, and the largest
/// rectangle under that skyline by a stack, O(w · h).
pub fn largest_rectangle(v: &Valid) -> Option<[usize; 4]> {
    let (w, h) = (v.width, v.height);
    let mut heights = vec![0usize; w];
    let mut best: Option<(usize, [usize; 4])> = None;
    let mut stack: Vec<usize> = Vec::with_capacity(w + 1);
    for y in 0..h {
        for (x, hx) in heights.iter_mut().enumerate() {
            *hx = if v.at(x, y) { *hx + 1 } else { 0 };
        }
        stack.clear();
        for x in 0..=w {
            let cur = if x < w { heights[x] } else { 0 };
            while let Some(&top) = stack.last() {
                if heights[top] < cur {
                    break;
                }
                stack.pop();
                let ht = heights[top];
                let left = stack.last().map_or(0, |&s| s + 1);
                let area = ht * (x - left);
                if area > 0 && best.is_none_or(|(a, _)| area > a) {
                    best = Some((area, [left, y + 1 - ht, x, y + 1]));
                }
            }
            stack.push(x);
        }
    }
    best.map(|(_, r)| r)
}

/// Every invalid pixel of `img` takes the value of its nearest valid pixel (a two-pass propagation of the
/// nearest valid pixel's position, 8-connected); valid pixels are left untouched.
pub fn extend_edges(img: &mut ImageBuf, v: &Valid) {
    let (w, h) = (v.width, v.height);
    const NONE: u32 = u32::MAX;
    // Per pixel, the index of the nearest valid pixel found so far.
    let mut near: Vec<u32> = (0..w * h)
        .map(|i| if v.bits[i] { i as u32 } else { NONE })
        .collect();
    let dist = |i: usize, src: u32| -> u64 {
        if src == NONE {
            return u64::MAX;
        }
        let (x, y) = ((i % w) as i64, (i / w) as i64);
        let (sx, sy) = ((src as usize % w) as i64, (src as usize / w) as i64);
        ((x - sx).pow(2) + (y - sy).pow(2)) as u64
    };
    let relax = |near: &mut Vec<u32>, x: usize, y: usize, nb: &[(i64, i64)]| {
        let i = y * w + x;
        let mut best = (dist(i, near[i]), near[i]);
        for &(dx, dy) in nb {
            let (nx, ny) = (x as i64 + dx, y as i64 + dy);
            if nx < 0 || ny < 0 || nx >= w as i64 || ny >= h as i64 {
                continue;
            }
            let src = near[ny as usize * w + nx as usize];
            let d = dist(i, src);
            if d < best.0 {
                best = (d, src);
            }
        }
        near[i] = best.1;
    };
    let forward = [(-1, 0), (-1, -1), (0, -1), (1, -1)];
    let backward = [(1, 0), (1, 1), (0, 1), (-1, 1)];
    for _ in 0..2 {
        for y in 0..h {
            for x in 0..w {
                relax(&mut near, x, y, &forward);
            }
            for x in (0..w).rev() {
                relax(&mut near, x, y, &[(1, 0)]);
            }
        }
        for y in (0..h).rev() {
            for x in (0..w).rev() {
                relax(&mut near, x, y, &backward);
            }
            for x in 0..w {
                relax(&mut near, x, y, &[(-1, 0)]);
            }
        }
    }
    for (i, &src) in near.iter().enumerate() {
        if !v.bits[i] && src != NONE {
            let s = src as usize;
            let p = img.pixel(s % w, s / w);
            img.set_pixel(i % w, i / w, p);
        }
    }
}

/// `rect` as Develop's crop, the share of the canvas on each side.
pub fn normalised(rect: [usize; 4], w: usize, h: usize) -> edit_model::CropRect {
    let (w, h) = (w as f32, h as f32);
    edit_model::CropRect {
        left: rect[0] as f32 / w,
        top: rect[1] as f32 / h,
        right: rect[2] as f32 / w,
        bottom: rect[3] as f32 / h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::Noise;

    fn random_outline(rng: &mut Noise, w: usize, h: usize) -> Valid {
        // A blob: a disc of random centre and radius, minus a random notch — panorama-like ragged edges.
        let (cx, cy) = (rng.next() * w as f64, rng.next() * h as f64);
        let r = (0.3 + 0.6 * rng.next()) * w.max(h) as f64;
        let (nx, ny, nr) = (
            rng.next() * w as f64,
            rng.next() * h as f64,
            rng.next() * 4.0,
        );
        let bits = (0..w * h)
            .map(|i| {
                let (x, y) = ((i % w) as f64, (i / w) as f64);
                (x - cx).hypot(y - cy) < r && (x - nx).hypot(y - ny) > nr
            })
            .collect();
        Valid {
            width: w,
            height: h,
            bits,
        }
    }

    fn brute(v: &Valid) -> usize {
        let mut best = 0;
        for t in 0..v.height {
            for l in 0..v.width {
                for b in t + 1..=v.height {
                    for r in l + 1..=v.width {
                        if (r - l) * (b - t) > best && (t..b).all(|y| (l..r).all(|x| v.at(x, y))) {
                            best = (r - l) * (b - t);
                        }
                    }
                }
            }
        }
        best
    }

    #[test]
    fn the_rectangle_is_maximal() {
        let mut rng = Noise(11);
        for case in 0..200 {
            let (w, h) = (
                3 + (rng.next() * 9.0) as usize,
                3 + (rng.next() * 7.0) as usize,
            );
            let v = random_outline(&mut rng, w, h);
            let area = largest_rectangle(&v).map_or(0, |[l, t, r, b]| (r - l) * (b - t));
            assert_eq!(area, brute(&v), "case {case}: {w} × {h}");
        }
    }

    #[test]
    fn the_rectangle_is_all_valid() {
        let mut rng = Noise(12);
        for _ in 0..50 {
            let v = random_outline(&mut rng, 40, 30);
            if let Some([l, t, r, b]) = largest_rectangle(&v) {
                assert!((t..b).all(|y| (l..r).all(|x| v.at(x, y))));
            }
        }
    }

    #[test]
    fn an_empty_outline_has_no_rectangle() {
        let v = Valid {
            width: 5,
            height: 4,
            bits: vec![false; 20],
        };
        assert_eq!(largest_rectangle(&v), None);
    }

    #[test]
    fn no_pixel_stays_black_after_extension() {
        let mut rng = Noise(13);
        let (w, h) = (60, 40);
        let v = random_outline(&mut rng, w, h);
        let mut img = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                if v.at(x, y) {
                    img.set_pixel(x, y, [0.1 + x as f32 / 100.0, 0.2, 0.3 + y as f32 / 100.0]);
                }
            }
        }
        let before = img.clone();
        extend_edges(&mut img, &v);
        for y in 0..h {
            for x in 0..w {
                let p = img.pixel(x, y);
                if v.at(x, y) {
                    assert_eq!(p, before.pixel(x, y), "valid pixels untouched");
                } else {
                    assert!(p[1] == 0.2, "({x}, {y}) took a valid pixel's value: {p:?}");
                }
            }
        }
    }

    /// The value comes from the NEAREST valid pixel: a column of distinct valid values to the left, and a
    /// gap row continues each value straight across.
    #[test]
    fn extension_takes_the_nearest_value() {
        let (w, h) = (10, 6);
        let bits = (0..w * h).map(|i| i % w < 3).collect();
        let v = Valid {
            width: w,
            height: h,
            bits,
        };
        let mut img = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..3 {
                img.set_pixel(x, y, [y as f32, 0.0, 0.0]);
            }
        }
        extend_edges(&mut img, &v);
        for y in 0..h {
            assert_eq!(img.pixel(9, y)[0], y as f32, "row {y}");
        }
    }

    #[test]
    fn a_rectangle_normalises_to_the_crop() {
        let c = normalised([10, 5, 90, 45], 100, 50);
        assert_eq!((c.left, c.top, c.right, c.bottom), (0.1, 0.1, 0.9, 0.9));
    }
}
