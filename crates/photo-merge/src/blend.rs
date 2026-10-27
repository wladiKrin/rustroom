//! The multi-band blend (M10.17b, Burt & Adelson): each frame's Laplacian pyramid weighted by the Gaussian
//! pyramid of the pixels it owns, summed, normalised by the summed weights level by level, collapsed. A
//! sky's gradient is joined over hundreds of pixels, a brick's detail over a few — so neither a presentness
//! step nor a doubled edge shows at a seam.
//!
//! Each frame's pyramid is built over its own box, aligned to the coarsest level's grid so its levels sit on
//! the canvas's; a frame's uncovered pixels are edge-extended first, so no black leaks into a band.

use crate::crop::Valid;
use crate::pano_warp::Warped;
use crate::projection::Canvas;
use crate::seam::Labels;
use core_image::ImageBuf;
use rayon::prelude::*;

/// The pyramid's levels.
pub const LEVELS: usize = 5;

/// `warped` joined under `labels`; and which pixels any frame covers.
pub fn multiband(
    canvas: &Canvas,
    warped: &[Warped],
    labels: &Labels,
    levels: usize,
) -> (ImageBuf, Valid) {
    let levels = levels.max(1);
    let q = 1usize << (levels - 1);
    let (pw, ph) = (canvas.width.div_ceil(q) * q, canvas.height.div_ceil(q) * q);
    let dims: Vec<(usize, usize)> = (0..levels).map(|l| (pw >> l, ph >> l)).collect();
    let mut acc: Vec<Vec<f32>> = dims.iter().map(|&(w, h)| vec![0.0; w * h * 3]).collect();
    let mut wsum: Vec<Vec<f32>> = dims.iter().map(|&(w, h)| vec![0.0; w * h]).collect();

    for (k, wk) in warped.iter().enumerate() {
        if wk.width == 0 || wk.height == 0 {
            continue;
        }
        let (x0, y0) = (wk.x0 / q * q, wk.y0 / q * q);
        let (x1, y1) = (
            (wk.x0 + wk.width).div_ceil(q) * q,
            (wk.y0 + wk.height).div_ceil(q) * q,
        );
        let (bw, bh) = (x1 - x0, y1 - y0);
        // The frame over its box: its own pixels where it holds the light, else the owner's.
        let rows: Vec<(Vec<f32>, Vec<bool>, Vec<f32>)> = (0..bh)
            .into_par_iter()
            .map(|y| {
                let (mut rgb, mut covered, mut mask) =
                    (vec![0f32; bw * 3], vec![false; bw], vec![0f32; bw]);
                let v = y0 + y;
                for x in 0..bw {
                    let u = x0 + x;
                    if u >= canvas.width || v >= canvas.height {
                        continue;
                    }
                    let owner = labels.owner[v * labels.width + u];
                    // ⚠⚠ Outside its own coverage a frame is given what the panorama shows there — its owner's
                    // pixel — never filler: the coarse levels of a mask near the coverage's edge reach across
                    // it, and edge-extended filler there put windows inside a neighbour's own area 0.05–0.13
                    // stops off on the synthetic row. …and the same where its own pixel is clipped and the
                    // owner's holds the light.
                    let theirs = (owner as usize != k)
                        .then(|| warped.get(owner as usize))
                        .flatten()
                        .filter(|o| o.index(u, v).is_some_and(|i| o.valid[i] && !o.clipped[i]))
                        .and_then(|o| o.at(u, v));
                    let mine = wk
                        .index(u, v)
                        .filter(|&i| wk.valid[i])
                        .map(|i| (wk.clipped[i], wk.at(u, v)));
                    let p = match mine {
                        Some((true, _)) if theirs.is_some() => theirs,
                        Some((_, p)) => p,
                        None => {
                            theirs.or_else(|| warped.get(owner as usize).and_then(|o| o.at(u, v)))
                        }
                    };
                    if let Some(p) = p {
                        rgb[3 * x..3 * x + 3].copy_from_slice(&p);
                        covered[x] = true;
                        if owner as usize == k {
                            mask[x] = 1.0;
                        }
                    }
                }
                (rgb, covered, mask)
            })
            .collect();
        let mask: Vec<f32> = rows.iter().flat_map(|r| r.2.iter().copied()).collect();
        if !mask.iter().any(|&m| m > 0.0) {
            continue;
        }
        let covered: Vec<bool> = rows.iter().flat_map(|r| r.1.iter().copied()).collect();
        let mut img = ImageBuf::from_vec(bw, bh, rows.into_iter().flat_map(|r| r.0).collect());
        crate::crop::extend_edges(
            &mut img,
            &Valid {
                width: bw,
                height: bh,
                bits: covered,
            },
        );
        // Gaussian pyramids of the image and the mask; the image's Laplacian.
        let mut g = vec![(img.data().to_vec(), bw, bh)];
        let mut m = vec![(mask, bw, bh)];
        for _ in 1..levels {
            let (ref im, w, h) = *g.last().unwrap();
            g.push((reduce(im, w, h, 3), w / 2, h / 2));
            let (ref mm, w, h) = *m.last().unwrap();
            m.push((reduce(mm, w, h, 1), w / 2, h / 2));
        }
        for l in 0..levels {
            let (gw, gh) = (g[l].1, g[l].2);
            let band: Vec<f32> = if l + 1 < levels {
                let up = expand(&g[l + 1].0, g[l + 1].1, g[l + 1].2, gw, gh, 3);
                g[l].0.par_iter().zip(&up).map(|(a, b)| a - b).collect()
            } else {
                g[l].0.clone()
            };
            let (ox, oy) = (x0 >> l, y0 >> l);
            let aw = dims[l].0;
            let weights = &m[l].0;
            // The box's rows of the accumulators, one row per task.
            acc[l][oy * aw * 3..(oy + gh) * aw * 3]
                .par_chunks_mut(aw * 3)
                .zip(wsum[l][oy * aw..(oy + gh) * aw].par_chunks_mut(aw))
                .enumerate()
                .for_each(|(y, (arow, wrow))| {
                    for x in 0..gw {
                        let wt = weights[y * gw + x];
                        if wt == 0.0 {
                            continue;
                        }
                        wrow[ox + x] += wt;
                        for c in 0..3 {
                            arow[3 * (ox + x) + c] += wt * band[3 * (y * gw + x) + c];
                        }
                    }
                });
        }
    }
    // Normalise each level by its weights, then collapse from the top — each level freed once it is added,
    // the sum built in place (at the cap a level-0 buffer is ~1.1 GB, and a copy of it is another).
    for l in 0..levels {
        acc[l]
            .par_chunks_mut(3)
            .zip(wsum[l].par_iter())
            .for_each(|(p, &wt)| {
                for v in p.iter_mut() {
                    *v = if wt > 0.0 { *v / wt } else { 0.0 };
                }
            });
    }
    drop(wsum);
    let mut out = acc.pop().expect("a level");
    for l in (0..levels - 1).rev() {
        let mut up = expand(&out, dims[l + 1].0, dims[l + 1].1, dims[l].0, dims[l].1, 3);
        drop(out);
        let band = acc.pop().expect("a level");
        up.par_iter_mut().zip(&band).for_each(|(a, b)| *a += b);
        out = up;
    }
    // The padded canvas's rows moved down onto the canvas's, in place; then cut to size.
    let (w, h) = (canvas.width, canvas.height);
    if pw != w {
        for y in 1..h {
            out.copy_within(3 * y * pw..3 * (y * pw + w), 3 * y * w);
        }
    }
    out.truncate(3 * w * h);
    let bits: Vec<bool> = labels
        .owner
        .iter()
        .map(|&o| o != crate::seam::NONE)
        .collect();
    out.par_chunks_mut(3).zip(&bits).for_each(|(p, &b)| {
        if !b {
            p.fill(0.0);
        }
    });
    (
        ImageBuf::from_vec(w, h, out),
        Valid {
            width: w,
            height: h,
            bits,
        },
    )
}

/// The binomial 1-4-6-4-1, then every second sample: `w × h` → `w/2 × h/2`, `ch` channels, edges clamped.
fn reduce(src: &[f32], w: usize, h: usize, ch: usize) -> Vec<f32> {
    const K: [f32; 5] = [1.0 / 16.0, 4.0 / 16.0, 6.0 / 16.0, 4.0 / 16.0, 1.0 / 16.0];
    let (ow, oh) = (w / 2, h / 2);
    let mut tmp = vec![0f32; ow * h * ch];
    tmp.par_chunks_mut(ow * ch)
        .enumerate()
        .for_each(|(y, row)| {
            for x in 0..ow {
                for (j, k) in K.iter().enumerate() {
                    let sx = (2 * x as i64 + j as i64 - 2).clamp(0, w as i64 - 1) as usize;
                    for c in 0..ch {
                        row[x * ch + c] += k * src[(y * w + sx) * ch + c];
                    }
                }
            }
        });
    let mut out = vec![0f32; ow * oh * ch];
    out.par_chunks_mut(ow * ch)
        .enumerate()
        .for_each(|(y, row)| {
            for (j, k) in K.iter().enumerate() {
                let sy = (2 * y as i64 + j as i64 - 2).clamp(0, h as i64 - 1) as usize;
                for (o, t) in row.iter_mut().zip(&tmp[sy * ow * ch..(sy + 1) * ow * ch]) {
                    *o += k * t;
                }
            }
        });
    out
}

/// Coarse `cw × chh` up to `w × h`: coarse sample `x` sits on fine `2x`, odd fines the mean of their two —
/// bilinear, the same operator building the Laplacian and collapsing it, so one frame reconstructs exactly.
fn expand(src: &[f32], cw: usize, chh: usize, w: usize, h: usize, ch: usize) -> Vec<f32> {
    let pick = |i: usize, n: usize| -> (usize, usize) {
        let a = (i / 2).min(n - 1);
        let b = if i % 2 == 1 {
            (i / 2 + 1).min(n - 1)
        } else {
            a
        };
        (a, b)
    };
    let mut out = vec![0f32; w * h * ch];
    out.par_chunks_mut(w * ch).enumerate().for_each(|(y, row)| {
        let (ya, yb) = pick(y, chh);
        for x in 0..w {
            let (xa, xb) = pick(x, cw);
            for c in 0..ch {
                let v = src[(ya * cw + xa) * ch + c]
                    + src[(ya * cw + xb) * ch + c]
                    + src[(yb * cw + xa) * ch + c]
                    + src[(yb * cw + xb) * ch + c];
                row[x * ch + c] = v / 4.0;
            }
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pano_warp::tests::{stage, Stage};
    use crate::seam::{cut, NONE};
    use crate::synth_pano::{self, Scene};

    /// The worst 16-px window's mean error against the truth, stops, over the whole panorama — a seam that
    /// shows is a window whose mean is off.
    fn worst_window(s: &Stage, img: &ImageBuf, v: &Valid) -> f64 {
        let mut worst = 0f64;
        for wy in (0..v.height.saturating_sub(16)).step_by(8) {
            for wx in (0..v.width.saturating_sub(16)).step_by(8) {
                let (mut sum, mut n) = (0f64, 0usize);
                for y in wy..wy + 16 {
                    for x in wx..wx + 16 {
                        let Some(t) = s.truth(x, y).filter(|_| v.at(x, y)) else {
                            continue;
                        };
                        if t[1] > 0.01 && t[1] < 0.6 {
                            sum += (img.pixel(x, y)[1] / t[1]).log2() as f64;
                            n += 1;
                        }
                    }
                }
                if n > 200 {
                    worst = worst.max((sum / n as f64).abs());
                }
            }
        }
        worst
    }

    /// ⚠ The control: the same row joined with no gains and no blend (each pixel its owner's, straight) has
    /// windows a third of a stop off — so a passing blend is what removed them.
    #[test]
    fn no_seam_is_visible() {
        let s = stage(synth_pano::row(6, 0.33, false), Scene::new(1), true);
        let l = cut(&s.canvas, &s.warped, &s.rig.order);
        let (img, v) = multiband(&s.canvas, &s.warped, &l, LEVELS);
        let worst = worst_window(&s, &img, &v);

        let raw = stage(synth_pano::row(6, 0.33, false), Scene::new(1), false);
        let lr = cut(&raw.canvas, &raw.warped, &raw.rig.order);
        let mut straight = ImageBuf::new(raw.canvas.width, raw.canvas.height);
        for y in 0..lr.height {
            for x in 0..lr.width {
                let o = lr.owner[y * lr.width + x];
                if o != NONE {
                    straight.set_pixel(x, y, raw.warped[o as usize].at(x, y).unwrap());
                }
            }
        }
        let vr = Valid {
            width: lr.width,
            height: lr.height,
            bits: lr.owner.iter().map(|&o| o != NONE).collect(),
        };
        let control = worst_window(&raw, &straight, &vr);
        eprintln!(
            "worst window: blended {worst:.4} stops, the straight join without gains {control:.4}"
        );
        assert!(control > 0.3, "the control must show the seams ({control})");
        assert!(worst < 0.03, "{worst} stops");
    }

    #[test]
    fn a_blend_of_one_frame_is_that_frame() {
        let s = stage(synth_pano::row(1, 0.33, false), Scene::new(1), true);
        let l = cut(&s.canvas, &s.warped, &s.rig.order);
        let (img, v) = multiband(&s.canvas, &s.warped, &l, LEVELS);
        let w = &s.warped[0];
        for y in (0..v.height).step_by(5) {
            for x in (0..v.width).step_by(5) {
                match w.at(x, y) {
                    Some(p) => {
                        assert!(v.at(x, y));
                        let got = img.pixel(x, y);
                        assert!(
                            got.iter().zip(p).all(|(g, w)| (g - w).abs() < 1e-5),
                            "({x}, {y})"
                        );
                    }
                    None => assert!(!v.at(x, y)),
                }
            }
        }
    }

    /// A dark square in ONE frame, standing in the overlap: the result shows it whole or not at all —
    /// never cut, never a ghost. ⚠ The control: a seam forced straight down the overlap's middle cuts it.
    #[test]
    fn a_moving_square_is_whole_once() {
        let mut r = synth_pano::row(3, 0.45, false);
        r.shots.iter_mut().for_each(|s| s.vignette = 0.0);
        // Halfway between frames 1 and 2 — the overlap's middle — a dark square in frame 2 only.
        let lon = (r.shots[1].yaw + r.shots[2].yaw) / 2.0;
        r.shots[2].square = Some([lon, -0.15, 0.03]);
        let s = stage(r, Scene::new(1), true);
        let square = |l: &Labels, img: &ImageBuf| -> (usize, usize, usize) {
            let (mut present, mut ghost, mut n, mut owned_by_2) = (0, 0, 0, 0usize);
            for y in 0..l.height {
                for x in 0..l.width {
                    let Some(d) = s.canvas.ray(x as f64, y as f64) else {
                        continue;
                    };
                    let Some(t) = s.truth(x, y) else { continue };
                    // The square's own pixels, by the scene's coordinates.
                    let m = s.r.shots[s.rig.reference].rotation();
                    let truth = crate::rotation::Rot([
                        m[0][0], m[1][0], m[2][0], m[0][1], m[1][1], m[2][1], m[0][2], m[1][2],
                        m[2][2],
                    ]);
                    let to_rig = s.rig.rotations[s.rig.reference].transpose().compose(&truth);
                    let (lo, la) = synth_pano::Scene::lon_lat(to_rig.transpose().apply(d));
                    if (lo - lon).abs() > 0.026
                        || (la + 0.15).abs() > 0.026
                        || l.owner[y * l.width + x] == NONE
                    {
                        continue;
                    }
                    n += 1;
                    owned_by_2 += (l.owner[y * l.width + x] == 2) as usize;
                    let ratio = img.pixel(x, y)[1] / t[1];
                    // The square is 0.15 of the scene: under a quarter it is there, under two thirds a ghost.
                    if ratio < 0.25 {
                        present += 1;
                    } else if ratio < 0.67 {
                        ghost += 1;
                    }
                }
            }
            eprintln!("  the square's pixels: {owned_by_2} of {n} from frame 2");
            (present, ghost, n)
        };
        let l = cut(&s.canvas, &s.warped, &s.rig.order);
        let (img, _) = multiband(&s.canvas, &s.warped, &l, LEVELS);
        let (present, ghost, n) = square(&l, &img);
        eprintln!("square: {present} present, {ghost} ghost of {n}");
        assert!(n > 300, "the square must be on the canvas ({n})");
        assert!(
            present * 20 >= n * 19 || present * 20 <= n,
            "cut: {present} of {n} present"
        );
        assert!(ghost * 50 < n, "{ghost} of {n} ghosted");

        // The control: frame 1 left of the overlap's middle, frame 2 right of it.
        let mut forced = l.clone();
        let mid = s
            .canvas
            .point(s.rig.ray(1, s.r.w as f64 - 1.0, s.r.h as f64 / 2.0))
            .unwrap()
            .0
            / 2.0
            + s.canvas
                .point(s.rig.ray(2, 0.0, s.r.h as f64 / 2.0))
                .unwrap()
                .0
                / 2.0;
        for y in 0..forced.height {
            for x in 0..forced.width {
                let i = y * forced.width + x;
                if s.warped[1].at(x, y).is_some() && s.warped[2].at(x, y).is_some() {
                    forced.owner[i] = if (x as f64) < mid { 1 } else { 2 };
                }
            }
        }
        let (img, _) = multiband(&s.canvas, &s.warped, &forced, LEVELS);
        let (present, _, n) = square(&forced, &img);
        assert!(
            present * 20 < n * 19 && present * 20 > n,
            "the straight seam must cut it: {present} of {n}"
        );
    }
}
