//! Features (M10.17b): the points two overlapping frames of a panorama can be matched by.
//!
//! **Corners** by the Harris response on log luminance at a reduced size, kept per cell of a 16 × 16 grid
//! so they spread over the whole frame; **descriptors** as an 8 × 8 sample of an oriented patch (turned to
//! the patch's intensity centroid, ORB's idea), its mean taken out and its length made one. On **log**
//! luminance an exposure difference is a constant, which the mean removes — so a frame a stop darker
//! describes its corners exactly as its neighbour does.
//!
//! ⚠ Not a binary (BRIEF) test, which the spec also allowed: measured on the synthetic scene, oriented
//! BRIEF bits were so correlated that two unrelated places differed by only 12–33 of 256 bits, inside the
//! range of true matches (2–39); 81 % of its matches were right where the normalised patch's are ≥ 90 %.

use crate::frame::Frame;
use crate::pyramid::FLOOR;

/// The long edge corners are found at.
pub const FEATURE_LONG_EDGE: usize = 1500;
/// At most this many corners a frame.
pub const MAX_FEATURES: usize = 2000;
const GRID: usize = 16;
/// Lowe's ratio on SQUARED distances (0.71 on distances, near SIFT's 0.7): measured on the synthetic row,
/// 0.64 kept 207 matches of which 91 % were right and 38 between two unrelated places; 0.5 keeps 172, 98 %
/// right, and under 20 between unrelated places; 0.4 gains little more (151 of 152).
const RATIO: f32 = 0.5;
/// The descriptor sample's spacing, detection pixels.
const SPACING: f32 = 3.0;

/// The descriptor's length: an 8 × 8 sample.
pub const DESC: usize = 64;

/// A corner: where it is in the full-size frame, and its descriptor — zero mean, unit length.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Keypoint {
    pub x: f32,
    pub y: f32,
    pub desc: [f32; DESC],
}

/// The descriptor's patch radius at the detection scale; corners nearer the edge than [`MARGIN`] are
/// dropped, so a patch turned any way stays inside.
const PATCH: f32 = 15.0;
const MARGIN: usize = 24;
/// The Harris constant.
const HARRIS_K: f32 = 0.04;

/// The corners of `f`, found at `long_edge` (or the frame's own size if smaller).
pub fn detect(f: &Frame, long_edge: usize) -> Vec<Keypoint> {
    let k = f.width.max(f.height).div_ceil(long_edge.max(1)).max(1);
    let (w, h) = (f.width / k, f.height / k);
    if w <= 2 * MARGIN || h <= 2 * MARGIN {
        return Vec::new();
    }
    let lum = log_luma(f, k, w, h);
    let g1 = blur(&lum, w, h, 1.0);
    let g2 = blur(&lum, w, h, 2.0);
    let r = harris(&g1, w, h);

    let per_cell = (MAX_FEATURES / (GRID * GRID)).max(1);
    let mut cells: Vec<Vec<(f32, usize, usize)>> = vec![Vec::new(); GRID * GRID];
    for y in MARGIN..h - MARGIN {
        for x in MARGIN..w - MARGIN {
            let v = r[y * w + x];
            if v <= 1e-9 {
                continue;
            }
            let peak = (y - 1..=y + 1)
                .all(|yy| (x - 1..=x + 1).all(|xx| (xx, yy) == (x, y) || r[yy * w + xx] < v));
            if peak {
                cells[(y * GRID / h) * GRID + x * GRID / w].push((v, x, y));
            }
        }
    }
    let mut out = Vec::new();
    for cell in cells.iter_mut() {
        cell.sort_by(|a, b| b.0.total_cmp(&a.0));
        for &(_, x, y) in cell.iter().take(per_cell) {
            let (dx, dy) = subpixel(&r, w, x, y);
            let (px, py) = (x as f32 + dx, y as f32 + dy);
            let angle = orientation(&g2, w, x, y);
            let Some(desc) = describe(&g2, w, h, px, py, angle) else {
                continue;
            };
            out.push(Keypoint {
                x: (px + 0.5) * k as f32 - 0.5,
                y: (py + 0.5) * k as f32 - 0.5,
                desc,
            });
        }
    }
    out.truncate(MAX_FEATURES);
    out
}

/// `log2` of the frame's luminance averaged over `k × k` blocks.
fn log_luma(f: &Frame, k: usize, w: usize, h: usize) -> Vec<f32> {
    use rayon::prelude::*;
    (0..h)
        .into_par_iter()
        .flat_map_iter(|y| {
            (0..w).map(move |x| {
                let mut s = 0f32;
                for yy in y * k..(y + 1) * k {
                    for xx in x * k..(x + 1) * k {
                        s += f.luma(xx, yy);
                    }
                }
                (s / (k * k) as f32).max(FLOOR).log2()
            })
        })
        .collect()
}

/// A separable Gaussian of `sigma`, the edge clamped.
fn blur(img: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    let r = (3.0 * sigma).ceil() as i64;
    let kern: Vec<f32> = (-r..=r)
        .map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp())
        .collect();
    let sum: f32 = kern.iter().sum();
    let kern: Vec<f32> = kern.iter().map(|v| v / sum).collect();
    let pass = |src: &[f32], horizontal: bool| -> Vec<f32> {
        let mut dst = vec![0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                let mut acc = 0f32;
                for (i, kv) in kern.iter().enumerate() {
                    let o = i as i64 - r;
                    let (sx, sy) = if horizontal {
                        ((x as i64 + o).clamp(0, w as i64 - 1) as usize, y)
                    } else {
                        (x, (y as i64 + o).clamp(0, h as i64 - 1) as usize)
                    };
                    acc += kv * src[sy * w + sx];
                }
                dst[y * w + x] = acc;
            }
        }
        dst
    };
    pass(&pass(img, true), false)
}

/// The Harris response of `g` (already smoothed): the structure tensor of its central differences,
/// integrated by a Gaussian of 1.5 px.
fn harris(g: &[f32], w: usize, h: usize) -> Vec<f32> {
    let (mut a, mut b, mut c) = (vec![0f32; w * h], vec![0f32; w * h], vec![0f32; w * h]);
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let i = y * w + x;
            let ix = (g[i + 1] - g[i - 1]) / 2.0;
            let iy = (g[i + w] - g[i - w]) / 2.0;
            (a[i], b[i], c[i]) = (ix * ix, iy * iy, ix * iy);
        }
    }
    let (a, b, c) = (
        blur(&a, w, h, 1.5),
        blur(&b, w, h, 1.5),
        blur(&c, w, h, 1.5),
    );
    (0..w * h)
        .map(|i| a[i] * b[i] - c[i] * c[i] - HARRIS_K * (a[i] + b[i]).powi(2))
        .collect()
}

/// The peak's offset from `(x, y)` by a parabola through it and its neighbours, along each axis.
fn subpixel(r: &[f32], w: usize, x: usize, y: usize) -> (f32, f32) {
    let i = y * w + x;
    let fit = |m: f32, c: f32, p: f32| {
        let d = m - 2.0 * c + p;
        if d < 0.0 {
            (0.5 * (m - p) / d).clamp(-0.5, 0.5)
        } else {
            0.0
        }
    };
    (fit(r[i - 1], r[i], r[i + 1]), fit(r[i - w], r[i], r[i + w]))
}

/// The patch's direction: from its centre to its intensity centroid, the patch mean taken out first —
/// so a constant (an exposure, on log luminance) moves nothing.
fn orientation(g: &[f32], w: usize, x: usize, y: usize) -> f32 {
    let r = PATCH as i64;
    let mut vals = Vec::new();
    for dy in -r..=r {
        for dx in -r..=r {
            if dx * dx + dy * dy <= r * r {
                let v = g[(y as i64 + dy) as usize * w + (x as i64 + dx) as usize];
                vals.push((dx as f32, dy as f32, v));
            }
        }
    }
    let mean = vals.iter().map(|v| v.2).sum::<f32>() / vals.len() as f32;
    let (mut mx, mut my) = (0f32, 0f32);
    for (dx, dy, v) in vals {
        mx += dx * (v - mean);
        my += dy * (v - mean);
    }
    my.atan2(mx)
}

fn bilinear(g: &[f32], w: usize, h: usize, x: f32, y: f32) -> f32 {
    let (x, y) = (x.clamp(0.0, (w - 1) as f32), y.clamp(0.0, (h - 1) as f32));
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(w - 1), (y0 + 1).min(h - 1));
    let (tx, ty) = (x - x0 as f32, y - y0 as f32);
    let top = g[y0 * w + x0] * (1.0 - tx) + g[y0 * w + x1] * tx;
    let bot = g[y1 * w + x0] * (1.0 - tx) + g[y1 * w + x1] * tx;
    top * (1.0 - ty) + bot * ty
}

/// The patch at `(x, y)` turned by `angle`, sampled 8 × 8 at [`SPACING`]; `None` for a flat patch.
fn describe(g: &[f32], w: usize, h: usize, x: f32, y: f32, angle: f32) -> Option<[f32; DESC]> {
    let (s, c) = angle.sin_cos();
    let mut d = [0f32; DESC];
    for (i, v) in d.iter_mut().enumerate() {
        let (u, t) = (
            ((i % 8) as f32 - 3.5) * SPACING,
            ((i / 8) as f32 - 3.5) * SPACING,
        );
        *v = bilinear(g, w, h, x + c * u - s * t, y + s * u + c * t);
    }
    let mean = d.iter().sum::<f32>() / DESC as f32;
    d.iter_mut().for_each(|v| *v -= mean);
    let norm = d.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm < 1e-3 {
        return None;
    }
    d.iter_mut().for_each(|v| *v /= norm);
    Some(d)
}

fn distance(a: &[f32; DESC], b: &[f32; DESC]) -> f32 {
    a.iter().zip(b).map(|(p, q)| (p - q) * (p - q)).sum()
}

/// For each of `from`, its nearest in `to` and whether it passes the ratio test.
fn nearest(from: &[Keypoint], to: &[Keypoint]) -> Vec<Option<usize>> {
    use rayon::prelude::*;
    from.par_iter()
        .map(|p| {
            let (mut best, mut second, mut at) = (f32::MAX, f32::MAX, None);
            for (j, q) in to.iter().enumerate() {
                let d = distance(&p.desc, &q.desc);
                if d < best {
                    (second, best, at) = (best, d, Some(j));
                } else if d < second {
                    second = d;
                }
            }
            at.filter(|_| best < RATIO * second)
        })
        .collect()
}

/// Matches between `a` and `b` as index pairs: each `a`'s nearest `b` by descriptor distance, kept when it
/// passes the ratio test and `a` is in turn `b`'s nearest.
pub fn matches(a: &[Keypoint], b: &[Keypoint]) -> Vec<(usize, usize)> {
    let (ab, ba) = (nearest(a, b), nearest(b, a));
    ab.iter()
        .enumerate()
        .filter_map(|(i, j)| j.filter(|&j| ba[j] == Some(i)).map(|j| (i, j)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth_pano::{self, Row, Scene};

    /// How many of `m` land within `px` of where the truth puts them.
    fn consistent(
        m: &[(usize, usize)],
        a: &[Keypoint],
        b: &[Keypoint],
        r: &Row,
        i: usize,
        j: usize,
        px: f64,
    ) -> usize {
        m.iter()
            .filter(|&&(ia, ib)| {
                let p = [a[ia].x as f64, a[ia].y as f64];
                synth_pano::map(&r.shots[i], &r.shots[j], r.focal, r.w, r.h, p)
                    .is_some_and(|q| (q[0] - b[ib].x as f64).hypot(q[1] - b[ib].y as f64) < px)
            })
            .count()
    }

    #[test]
    fn matches_survive_a_stop_of_exposure() {
        let mut r = synth_pano::row(3, 0.35, false);
        r.shots[0].gain = 1.0;
        r.shots[1].gain = 0.5;
        r.shots[1].roll = 0.0;
        let f = synth_pano::render(&Scene::new(1), &r.shots[..2], r.w, r.h, r.focal);
        let (a, b) = (
            detect(&f[0], FEATURE_LONG_EDGE),
            detect(&f[1], FEATURE_LONG_EDGE),
        );
        let m = matches(&a, &b);
        let ok = consistent(&m, &a, &b, &r, 0, 1, 2.0);
        eprintln!(
            "{} + {} corners, {} matches, {ok} consistent",
            a.len(),
            b.len(),
            m.len()
        );
        assert!(m.len() >= 100, "{} matches", m.len());
        assert!(ok as f64 >= 0.9 * m.len() as f64, "{ok} of {}", m.len());
    }

    /// The control for the test above: matching is not merely finding pairs everywhere.
    #[test]
    fn unrelated_frames_barely_match() {
        let r = synth_pano::row(1, 0.35, false);
        let f1 = synth_pano::render(&Scene::new(1), &r.shots, r.w, r.h, r.focal);
        let f2 = synth_pano::render(&Scene::new(2), &r.shots, r.w, r.h, r.focal);
        let m = matches(
            &detect(&f1[0], FEATURE_LONG_EDGE),
            &detect(&f2[0], FEATURE_LONG_EDGE),
        );
        assert!(m.len() < 20, "{} matches between two places", m.len());
    }

    #[test]
    fn matching_is_rotation_tolerant() {
        let r = synth_pano::row(3, 0.35, false);
        assert!(
            r.shots[1].roll.abs() > 0.03,
            "the second frame is the rolled one"
        );
        let f = synth_pano::render(&Scene::new(1), &r.shots[..2], r.w, r.h, r.focal);
        let (a, b) = (
            detect(&f[0], FEATURE_LONG_EDGE),
            detect(&f[1], FEATURE_LONG_EDGE),
        );
        let m = matches(&a, &b);
        let ok = consistent(&m, &a, &b, &r, 0, 1, 2.0);
        assert!(ok >= 100, "{ok} consistent of {} matches", m.len());
    }

    #[test]
    fn corners_spread_over_the_frame_and_stay_under_the_cap() {
        let r = synth_pano::row(1, 0.35, false);
        let f = synth_pano::render(&Scene::new(1), &r.shots, r.w, r.h, r.focal);
        let k = detect(&f[0], FEATURE_LONG_EDGE);
        assert!(k.len() <= MAX_FEATURES);
        let left = k.iter().filter(|p| p.x < r.w as f32 / 3.0).count();
        let right = k.iter().filter(|p| p.x > 2.0 * r.w as f32 / 3.0).count();
        assert!(
            left > k.len() / 6 && right > k.len() / 6,
            "{left} left, {right} right of {}",
            k.len()
        );
    }

    /// A frame reduced for detection still reports corners in full-size pixels.
    #[test]
    fn a_reduced_detection_reports_full_size_positions() {
        let r = synth_pano::row(3, 0.35, false);
        let mut shots = r.shots.clone();
        shots[1].roll = 0.0;
        let f = synth_pano::render(&Scene::new(1), &shots[..2], r.w, r.h, r.focal);
        let (a, b) = (detect(&f[0], 240), detect(&f[1], 240));
        let m = matches(&a, &b);
        let ok = consistent(&m, &a, &b, &Row { shots, ..r }, 0, 1, 4.0);
        assert!(
            ok >= 20 && ok as f64 >= 0.8 * m.len() as f64,
            "{ok} of {}",
            m.len()
        );
    }
}
