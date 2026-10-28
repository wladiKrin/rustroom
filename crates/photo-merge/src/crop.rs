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

/// Every invalid pixel of `img` takes the value of its nearest valid pixel — exactly, by Euclidean distance
/// (a separable distance transform carrying the source: per column, then the rows' lower envelopes, both in
/// parallel); valid pixels are left untouched.
pub fn extend_edges(img: &mut ImageBuf, v: &Valid) {
    use rayon::prelude::*;
    let (w, h) = (v.width, v.height);
    if w == 0 || h == 0 || v.bits.iter().all(|&b| b) || !v.bits.iter().any(|&b| b) {
        return;
    }
    const FAR: u32 = u32::MAX;
    // Pass 1, per column: the nearest valid row above or below each pixel.
    let mut col_row = vec![FAR; w * h];
    col_row.par_chunks_mut(h).enumerate().for_each(|(x, col)| {
        let mut last = FAR;
        for (y, c) in col.iter_mut().enumerate() {
            if v.bits[y * w + x] {
                last = y as u32;
            }
            *c = last;
        }
        let mut next = FAR;
        for y in (0..h).rev() {
            if v.bits[y * w + x] {
                next = y as u32;
            }
            let d = |r: u32| {
                if r == FAR {
                    u64::MAX
                } else {
                    (r as i64 - y as i64).unsigned_abs()
                }
            };
            if d(next) < d(col[y]) {
                col[y] = next;
            }
        }
    });
    // Pass 2, per row: the lower envelope of the columns' parabolas (Felzenszwalb–Huttenlocher), tracking
    // which column — so each pixel learns its nearest valid pixel exactly.
    let sources: Vec<u32> = (0..h)
        .into_par_iter()
        .flat_map_iter(|y| {
            let g = |x: usize| -> Option<f64> {
                let r = col_row[x * h + y];
                (r != FAR).then(|| ((r as i64 - y as i64) as f64).powi(2))
            };
            let cols: Vec<usize> = (0..w).filter(|&x| g(x).is_some()).collect();
            let mut hull: Vec<usize> = Vec::with_capacity(cols.len());
            let mut from: Vec<f64> = Vec::with_capacity(cols.len());
            let meet = |p: usize, q: usize| {
                let (fp, fq) = (g(p).unwrap(), g(q).unwrap());
                ((fq + (q * q) as f64) - (fp + (p * p) as f64)) / (2.0 * (q as f64 - p as f64))
            };
            for &q in &cols {
                while let Some(&p) = hull.last() {
                    if meet(p, q) <= *from.last().unwrap() {
                        hull.pop();
                        from.pop();
                    } else {
                        break;
                    }
                }
                from.push(hull.last().map_or(f64::NEG_INFINITY, |&p| meet(p, q)));
                hull.push(q);
            }
            let mut k = 0;
            (0..w)
                .map(|x| {
                    while k + 1 < hull.len() && from[k + 1] <= x as f64 {
                        k += 1;
                    }
                    let c = hull[k];
                    col_row[c * h + y] * w as u32 + c as u32
                })
                .collect::<Vec<_>>()
        })
        .collect();
    let data = img.data_mut();
    let picked: Vec<(usize, [f32; 3])> = (0..w * h)
        .into_par_iter()
        .filter(|&i| !v.bits[i])
        .map(|i| {
            let s = sources[i] as usize;
            (i, [data[3 * s], data[3 * s + 1], data[3 * s + 2]])
        })
        .collect();
    for (i, p) in picked {
        data[3 * i..3 * i + 3].copy_from_slice(&p);
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

    /// Each filled pixel's source (encoded in its value) is at the smallest distance any valid pixel is.
    #[test]
    fn extension_is_exactly_nearest() {
        let mut rng = Noise(14);
        for _ in 0..20 {
            let (w, h) = (37, 23);
            let v = random_outline(&mut rng, w, h);
            if !v.bits.iter().any(|&b| b) {
                continue;
            }
            let mut img = ImageBuf::new(w, h);
            for y in 0..h {
                for x in 0..w {
                    if v.at(x, y) {
                        img.set_pixel(x, y, [x as f32, y as f32, 1.0]);
                    }
                }
            }
            extend_edges(&mut img, &v);
            for y in 0..h {
                for x in 0..w {
                    if v.at(x, y) {
                        continue;
                    }
                    let p = img.pixel(x, y);
                    assert_eq!(p[2], 1.0, "({x}, {y}) unfilled");
                    let d2 = |sx: f32, sy: f32| (sx - x as f32).powi(2) + (sy - y as f32).powi(2);
                    let best = (0..h)
                        .flat_map(|sy| (0..w).map(move |sx| (sx, sy)))
                        .filter(|&(sx, sy)| v.at(sx, sy))
                        .map(|(sx, sy)| d2(sx as f32, sy as f32))
                        .fold(f32::MAX, f32::min);
                    assert_eq!(d2(p[0], p[1]), best, "({x}, {y}) took ({}, {})", p[0], p[1]);
                }
            }
        }
    }

    #[test]
    fn a_rectangle_normalises_to_the_crop() {
        let c = normalised([10, 5, 90, 45], 100, 50);
        assert_eq!((c.left, c.top, c.right, c.bottom), (0.1, 0.1, 0.9, 0.9));
    }
}
