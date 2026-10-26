//! Auto align: each frame registered onto the reference — a homography (a handheld shift, turn and a
//! little perspective) and the frame's exposure relative to it, fitted together.
//!
//! **The method** (spec M10.17a): on log luminance ([`LumPyramid`]), the frame is modelled as the
//! reference moved by `H` and offset by `b` stops: `Lf(H·x) ≈ Lref(x) + b`. Coarsest level first, an
//! exhaustive search for the shift (±25 % of the frame) by zero-mean normalised cross-correlation,
//! which the exposure offset cannot fool; then, level by level down to `finest`, Gauss–Newton with
//! Tukey-biweight weights (a moving person gets no vote once it is clearly not the scene), centred on the
//! frame and scaled by half its long edge so every parameter is of order one.
//!
//! A **similarity** (shift, turn, scale) is fitted through every level first; the full 8-parameter
//! homography then refines it at the finest two, and is kept only when its robust residual is at
//! least 1 % lower and its corners stay within 2 % of the diagonal of the similarity's — a
//! homography on a weakly textured frame can wander where a similarity cannot.

use crate::homography::Homography;
use crate::pyramid::{Level, LumPyramid};
use rayon::prelude::*;

/// The coarsest level's long edge, in pixels.
pub const COARSEST: usize = 128;
/// The share of the reference a frame must share valid pixels with to be aligned to it directly.
pub const MIN_OVERLAP: f32 = 0.02;
/// The coarse search's reach, as a share of the frame.
const SEARCH: f32 = 0.25;
const MAX_ITERS: usize = 30;
/// An update moving no corner further than this, in the level's pixels, has converged.
const STOP_PX: f64 = 0.01;
/// The homography is kept only when its residual is at least this much lower…
const HOMOGRAPHY_GAIN: f32 = 0.99;
/// …and its corners stay within this share of the diagonal of the similarity's.
const HOMOGRAPHY_STRAY: f64 = 0.02;

/// How a frame sits on the reference.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Alignment {
    /// Reference pixels → frame pixels, at full size (level 0).
    pub h: Homography,
    /// `log2(frame / reference)` where both hold light — the frame's exposure in stops.
    pub log_gain: f32,
    /// The share of the reference's pixels the frame shares valid pixels with, at the finest level.
    pub overlap: f32,
    /// The fit's robust scale (1.4826 × MAD of the log residuals) at the finest level.
    pub residual: f32,
}

#[derive(Clone, Copy, PartialEq)]
enum Model {
    Similarity,
    Homography,
}

/// Align `frame` onto `reference` — both pyramids of same-size frames — down to level `finest`
/// (0 = full size). `init` starts the fit there instead of from a search; a frame sharing too little
/// with the reference keeps `init` (or the identity) and says so in `overlap`.
pub fn align(
    reference: &LumPyramid,
    frame: &LumPyramid,
    init: Option<&Alignment>,
    finest: usize,
) -> Alignment {
    let top = reference.levels().min(frame.levels()) - 1;
    let finest = finest.min(top);
    let fallback = |overlap: f32| Alignment {
        h: init.map_or(Homography::identity(), |a| a.h),
        log_gain: init.map_or(0.0, |a| a.log_gain),
        overlap,
        residual: f32::INFINITY,
    };
    let begin = match init {
        Some(a) => (a.h, a.log_gain),
        None => match start(reference, frame, top, finest) {
            Some(s) => s,
            None => return fallback(0.0),
        },
    };

    let mut sim = begin;
    for k in (finest..=top).rev() {
        sim = refine(
            reference.level(k),
            frame.level(k),
            sim,
            Model::Similarity,
            k,
        );
    }
    let mut hom = sim;
    for k in (finest..=(finest + 1).min(top)).rev() {
        hom = refine(
            reference.level(k),
            frame.level(k),
            hom,
            Model::Homography,
            k,
        );
    }

    let (r, f) = (reference.level(finest), frame.level(finest));
    let (sim_loss, overlap, sim_scale) = loss(r, f, sim, finest);
    if overlap < MIN_OVERLAP {
        return fallback(overlap);
    }
    let (hom_loss, _, hom_scale) = loss(r, f, hom, finest);
    let (w0, h0) = (reference.level(0).w, reference.level(0).h);
    let diag = ((w0 * w0 + h0 * h0) as f64).sqrt();
    let keep_hom = hom_loss <= HOMOGRAPHY_GAIN * sim_loss
        && hom.0.corner_shift(&sim.0, w0, h0) <= HOMOGRAPHY_STRAY * diag;
    let ((h, log_gain), residual) = if keep_hom {
        (hom, hom_scale)
    } else {
        (sim, sim_scale)
    };
    Alignment {
        h,
        log_gain,
        overlap,
        residual,
    }
}

/// The radius of the box the coarse search subtracts, in the coarsest level's pixels.
const HIGHPASS: usize = 4;

/// `l` minus its local mean over valid pixels — texture without the light falling across it.
///
/// ⚠⚠ **Why the search needs it** (found by `a_moving_square_does_not_pull_the_fit`): a scene's light
/// field spans many stops and correlates almost equally well at every small shift, so on raw log
/// luminance the winner was decided by WHICH pixels were valid at each shift — clipped ones drop out —
/// and a frame with a bright moving square locked 79 px off. On the high-passed image only texture
/// correlates, and the exposure offset vanishes with the mean.
fn highpass(l: &Level) -> Level {
    let (w, h) = (l.w, l.h);
    let mut sum = vec![0f64; (w + 1) * (h + 1)];
    let mut cnt = vec![0i64; (w + 1) * (h + 1)];
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let (v, c) = if l.valid[i] {
                (l.l[i] as f64, 1)
            } else {
                (0.0, 0)
            };
            let o = (y + 1) * (w + 1) + x + 1;
            sum[o] = v + sum[o - 1] + sum[o - w - 1] - sum[o - w - 2];
            cnt[o] = c + cnt[o - 1] + cnt[o - w - 1] - cnt[o - w - 2];
        }
    }
    let mut out = Level {
        w,
        h,
        l: vec![0.0; w * h],
        valid: vec![false; w * h],
    };
    let r = HIGHPASS;
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            if !l.valid[i] {
                continue;
            }
            let (x0, y0, x1, y1) = (
                x.saturating_sub(r),
                y.saturating_sub(r),
                (x + r + 1).min(w),
                (y + r + 1).min(h),
            );
            let at = |xx: usize, yy: usize| yy * (w + 1) + xx;
            let s = sum[at(x1, y1)] - sum[at(x0, y1)] - sum[at(x1, y0)] + sum[at(x0, y0)];
            let c = cnt[at(x1, y1)] - cnt[at(x0, y1)] - cnt[at(x1, y0)] + cnt[at(x0, y0)];
            let area = ((x1 - x0) * (y1 - y0)) as i64;
            if c * 2 >= area {
                out.l[i] = l.l[i] - (s / c as f64) as f32;
                out.valid[i] = true;
            }
        }
    }
    out
}

/// How many correlation peaks each map offers the search.
const PEAKS: usize = 3;

/// The ZNCC of `r` against `f` at every whole-pixel shift within ±[`SEARCH`] that shares enough
/// valid pixels, as `(shift, score)`.
fn zncc_map(r: &Level, f: &Level) -> Vec<((i64, i64), f64)> {
    let (sx, sy) = (
        (SEARCH * r.w as f32).round() as i64,
        (SEARCH * r.h as f32).round() as i64,
    );
    let need = ((r.w * r.h) as f32 * 0.1).max(50.0) as usize;
    let shifts: Vec<(i64, i64)> = (-sy..=sy)
        .flat_map(|dy| (-sx..=sx).map(move |dx| (dx, dy)))
        .collect();
    shifts
        .par_iter()
        .filter_map(|&(dx, dy)| {
            let (mut n, mut sa, mut sb, mut saa, mut sbb, mut sab) =
                (0usize, 0f64, 0f64, 0f64, 0f64, 0f64);
            for y in 0..r.h as i64 {
                let fy = y + dy;
                if fy < 0 || fy >= f.h as i64 {
                    continue;
                }
                for x in 0..r.w as i64 {
                    let fx = x + dx;
                    if fx < 0 || fx >= f.w as i64 {
                        continue;
                    }
                    let (ri, fi) = (
                        (y as usize) * r.w + x as usize,
                        (fy as usize) * f.w + fx as usize,
                    );
                    if !(r.valid[ri] && f.valid[fi]) {
                        continue;
                    }
                    let (a, b) = (r.l[ri] as f64, f.l[fi] as f64);
                    n += 1;
                    sa += a;
                    sb += b;
                    saa += a * a;
                    sbb += b * b;
                    sab += a * b;
                }
            }
            if n < need {
                return None;
            }
            let nf = n as f64;
            let cov = sab - sa * sb / nf;
            let (va, vb) = (saa - sa * sa / nf, sbb - sb * sb / nf);
            (va > 1e-12 && vb > 1e-12).then(|| ((dx, dy), cov / (va * vb).sqrt()))
        })
        .collect()
}

/// The `k` best local maxima of a correlation map.
fn peaks(map: &[((i64, i64), f64)], k: usize) -> Vec<(i64, i64)> {
    let at: std::collections::HashMap<(i64, i64), f64> = map.iter().copied().collect();
    let mut tops: Vec<((i64, i64), f64)> = map
        .iter()
        .copied()
        .filter(|&((dx, dy), v)| {
            (-1..=1).all(|j| (-1..=1).all(|i| at.get(&(dx + i, dy + j)).is_none_or(|o| *o <= v)))
        })
        .collect();
    tops.sort_by(|a, b| b.1.total_cmp(&a.1));
    tops.into_iter().take(k).map(|t| t.0).collect()
}

/// The median of `f − r` (log) where both are valid at shift `(dx, dy)`.
fn offset_at(r: &Level, f: &Level, (dx, dy): (i64, i64)) -> Option<f32> {
    let mut d: Vec<f32> = Vec::new();
    for y in 0..r.h as i64 {
        for x in 0..r.w as i64 {
            let (fx, fy) = (x + dx, y + dy);
            if fx < 0 || fy < 0 || fx >= f.w as i64 || fy >= f.h as i64 {
                continue;
            }
            let (ri, fi) = (
                (y as usize) * r.w + x as usize,
                (fy as usize) * f.w + fx as usize,
            );
            if r.valid[ri] && f.valid[fi] {
                d.push(f.l[fi] - r.l[ri]);
            }
        }
    }
    median(&mut d)
}

/// Where the fit starts when nothing is known: the coarsest level's correlation peaks — of the log
/// luminance and of its high-pass — each refined as a similarity through the two coarsest levels
/// (never below `finest`), and the one whose residual is smallest wins, among those sharing at
/// least half the best overlap. `None` when no shift shares enough valid pixels.
///
/// ⚠⚠ **Candidates, not the single best peak** — both single-peak searches failed a test here. On
/// plain log luminance the scene's light field correlates almost equally well at every small shift,
/// so which pixels happened to be valid (a bright mover clipped away) decided it, 79 px off; on the
/// high-pass, a repeating texture (window bars, railings, the fixture's sines) has several peaks of
/// nearly one height. A wrong peak does not survive two levels of fitting with a low residual.
fn start(rp: &LumPyramid, fp: &LumPyramid, top: usize, finest: usize) -> Option<(Homography, f32)> {
    let (r, f) = (rp.level(top), fp.level(top));
    let mut cands = peaks(&zncc_map(r, f), PEAKS);
    for p in peaks(&zncc_map(&highpass(r), &highpass(f)), PEAKS) {
        if !cands
            .iter()
            .any(|c| (c.0 - p.0).abs() <= 1 && (c.1 - p.1).abs() <= 1)
        {
            cands.push(p);
        }
    }
    let judge = top.saturating_sub(1).max(finest);
    let scale = f64::powi(2.0, top as i32);
    let fitted: Vec<((Homography, f32), f32, f32)> = cands
        .par_iter()
        .filter_map(|&shift| {
            let b = offset_at(r, f, shift)?;
            let mut fit = (
                Homography::translation(shift.0 as f64, shift.1 as f64).rescaled(scale),
                b,
            );
            for k in (judge..=top).rev() {
                fit = refine(rp.level(k), fp.level(k), fit, Model::Similarity, k);
            }
            let (_, overlap, spread) = loss(rp.level(judge), fp.level(judge), fit, judge);
            spread.is_finite().then_some((fit, overlap, spread))
        })
        .collect();
    let most = fitted.iter().map(|c| c.1).fold(0f32, f32::max);
    fitted
        .into_iter()
        .filter(|c| c.1 >= 0.5 * most)
        .min_by(|a, b| a.2.total_cmp(&b.2))
        .map(|c| c.0)
}

fn median(v: &mut [f32]) -> Option<f32> {
    if v.is_empty() {
        return None;
    }
    let mid = v.len() / 2;
    Some(*v.select_nth_unstable_by(mid, |a, b| a.total_cmp(b)).1)
}

/// A level's centred, scaled coordinates: `x̂ = (x − c) / s`.
struct Norm {
    cx: f64,
    cy: f64,
    s: f64,
}

impl Norm {
    fn of(l: &Level) -> Self {
        Self {
            cx: (l.w as f64 - 1.0) / 2.0,
            cy: (l.h as f64 - 1.0) / 2.0,
            s: l.w.max(l.h) as f64 / 2.0,
        }
    }

    /// Pixel → normalised, as a homography.
    fn n(&self) -> Homography {
        Homography([
            1.0 / self.s,
            0.0,
            -self.cx / self.s,
            0.0,
            1.0 / self.s,
            -self.cy / self.s,
            0.0,
            0.0,
            1.0,
        ])
    }

    fn n_inv(&self) -> Homography {
        Homography([self.s, 0.0, self.cx, 0.0, self.s, self.cy, 0.0, 0.0, 1.0])
    }
}

/// The frame's log luminance and its gradient at a fractional pixel, bilinear — `None` unless the
/// four pixels around it, and their neighbours, are valid.
struct Sampled {
    l: f32,
    gx: f32,
    gy: f32,
}

struct Grad {
    gx: Vec<f32>,
    gy: Vec<f32>,
    ok: Vec<bool>,
}

fn gradients(f: &Level) -> Grad {
    let (w, h) = (f.w, f.h);
    let mut g = Grad {
        gx: vec![0.0; w * h],
        gy: vec![0.0; w * h],
        ok: vec![false; w * h],
    };
    for y in 1..h.saturating_sub(1) {
        for x in 1..w.saturating_sub(1) {
            let i = y * w + x;
            if f.valid[i] && f.valid[i - 1] && f.valid[i + 1] && f.valid[i - w] && f.valid[i + w] {
                g.gx[i] = (f.l[i + 1] - f.l[i - 1]) * 0.5;
                g.gy[i] = (f.l[i + w] - f.l[i - w]) * 0.5;
                g.ok[i] = true;
            }
        }
    }
    g
}

#[inline]
fn sample(f: &Level, g: &Grad, x: f64, y: f64) -> Option<Sampled> {
    if !(x >= 0.0 && y >= 0.0) {
        return None;
    }
    let (x0, y0) = (x.floor() as usize, y.floor() as usize);
    if x0 + 1 >= f.w || y0 + 1 >= f.h {
        return None;
    }
    let (fx, fy) = ((x - x0 as f64) as f32, (y - y0 as f64) as f32);
    let i = y0 * f.w + x0;
    let idx = [i, i + 1, i + f.w, i + f.w + 1];
    if !idx.iter().all(|&k| g.ok[k]) {
        return None;
    }
    let wts = [
        (1.0 - fx) * (1.0 - fy),
        fx * (1.0 - fy),
        (1.0 - fx) * fy,
        fx * fy,
    ];
    let mut s = Sampled {
        l: 0.0,
        gx: 0.0,
        gy: 0.0,
    };
    for (k, w) in idx.iter().zip(wts) {
        s.l += w * f.l[*k];
        s.gx += w * g.gx[*k];
        s.gy += w * g.gy[*k];
    }
    Some(s)
}

/// The residual's Tukey-biweight weight for a cut-off `c`: smoothly less as it grows, and none past
/// `c`.
///
/// ⚠ Tukey, not Huber (found by `a_moving_square_does_not_pull_the_fit`): Huber caps an outlier's
/// vote but never removes it, and a fifth of the frame offset one way by something that moved held
/// the fit 0.58 px off; a redescending weight gives it none. It needs a good start, which the
/// candidate search ([`start`]) is there to provide.
#[inline]
fn tukey(r: f32, c: f32) -> f32 {
    let u = r / c;
    if u.abs() >= 1.0 {
        0.0
    } else {
        (1.0 - u * u).powi(2)
    }
}

/// One level of Gauss–Newton from `start` (level-0 homography, offset), returning the level-0 fit.
fn refine(
    r: &Level,
    f: &Level,
    start: (Homography, f32),
    model: Model,
    k: usize,
) -> (Homography, f32) {
    let scale = f64::powi(2.0, k as i32);
    let nm = Norm::of(r);
    let mut hn = nm
        .n()
        .compose(&start.0.rescaled(1.0 / scale))
        .compose(&nm.n_inv());
    let w8 = hn.0[8];
    hn.0.iter_mut().for_each(|v| *v /= w8);
    if model == Model::Similarity {
        let (a, b) = ((hn.0[0] + hn.0[4]) / 2.0, (hn.0[3] - hn.0[1]) / 2.0);
        hn = Homography([a, -b, hn.0[2], b, a, hn.0[5], 0.0, 0.0, 1.0]);
    }
    let mut b = start.1;
    let g = gradients(f);
    let np = if model == Model::Similarity { 4 } else { 8 };
    let n = np + 1;
    let step = ((r.w * r.h) as f64 / 400_000.0).sqrt().max(1.0) as usize;
    let mut kh = f32::INFINITY;

    for _ in 0..MAX_ITERS {
        // One pass: the normal equations under the current Tukey cut-off, and the residuals for the next.
        let rows: Vec<usize> = (0..r.h).step_by(step).collect();
        let (ata, atb, resid) = rows
            .par_iter()
            .fold(
                || (vec![0f64; n * n], vec![0f64; n], Vec::<f32>::new()),
                |(mut ata, mut atb, mut resid), &y| {
                    let mut jac = [0f64; 9];
                    for x in (0..r.w).step_by(step) {
                        let i = y * r.w + x;
                        if !r.valid[i] {
                            continue;
                        }
                        let (xh, yh) = ((x as f64 - nm.cx) / nm.s, (y as f64 - nm.cy) / nm.s);
                        let h = &hn.0;
                        let den = h[6] * xh + h[7] * yh + 1.0;
                        let (u, v) = (
                            (h[0] * xh + h[1] * yh + h[2]) / den,
                            (h[3] * xh + h[4] * yh + h[5]) / den,
                        );
                        let Some(s) = sample(f, &g, u * nm.s + nm.cx, v * nm.s + nm.cy) else {
                            continue;
                        };
                        let res = s.l - b - r.l[i];
                        resid.push(res);
                        let wt = tukey(res, kh) as f64;
                        let (gx, gy) = (s.gx as f64 * nm.s, s.gy as f64 * nm.s);
                        match model {
                            Model::Similarity => {
                                jac[0] = gx * xh + gy * yh;
                                jac[1] = -gx * yh + gy * xh;
                                jac[2] = gx;
                                jac[3] = gy;
                            }
                            Model::Homography => {
                                let p = -(gx * u + gy * v) / den;
                                jac[0] = gx * xh / den;
                                jac[1] = gx * yh / den;
                                jac[2] = gx / den;
                                jac[3] = gy * xh / den;
                                jac[4] = gy * yh / den;
                                jac[5] = gy / den;
                                jac[6] = p * xh;
                                jac[7] = p * yh;
                            }
                        }
                        jac[np] = -1.0;
                        for a in 0..n {
                            atb[a] += wt * jac[a] * res as f64;
                            for c in a..n {
                                ata[a * n + c] += wt * jac[a] * jac[c];
                            }
                        }
                    }
                    (ata, atb, resid)
                },
            )
            .reduce(
                || (vec![0f64; n * n], vec![0f64; n], Vec::new()),
                |mut a, b| {
                    a.0.iter_mut().zip(&b.0).for_each(|(x, y)| *x += y);
                    a.1.iter_mut().zip(&b.1).for_each(|(x, y)| *x += y);
                    a.2.extend(b.2);
                    a
                },
            );
        if resid.len() < 10 * n {
            break;
        }
        let mut abs: Vec<f32> = resid.iter().map(|v| v.abs()).collect();
        let mad = median(&mut abs).unwrap_or(0.0);
        let first = kh.is_infinite();
        kh = (4.685 * 1.4826 * mad).max(1e-3);
        if first {
            // The first pass only measured the scale; weigh with it before stepping.
            continue;
        }
        let mut ata = ata;
        for a in 0..n {
            for c in 0..a {
                ata[a * n + c] = ata[c * n + a];
            }
        }
        let Some(delta) = solve(ata, atb.iter().map(|v| -v).collect(), n) else {
            break;
        };
        let old = hn;
        let h = &mut hn.0;
        match model {
            Model::Similarity => {
                let (a, bb) = (h[0] + delta[0], h[3] + delta[1]);
                h[0] = a;
                h[4] = a;
                h[3] = bb;
                h[1] = -bb;
                h[2] += delta[2];
                h[5] += delta[3];
            }
            Model::Homography => {
                for (p, d) in h.iter_mut().zip(&delta[..8]) {
                    *p += d;
                }
            }
        }
        b += delta[np] as f32;
        let moved = old.corner_shift(&hn, 3, 3) * nm.s;
        if !moved.is_finite() {
            hn = old;
            break;
        }
        if moved < STOP_PX {
            break;
        }
    }
    let hk = nm.n_inv().compose(&hn).compose(&nm.n());
    (hk.rescaled(scale), b)
}

/// The robust loss of a fit at level `k`: mean Huber loss under its own MAD scale, the overlap,
/// and the scale itself.
fn loss(r: &Level, f: &Level, fit: (Homography, f32), k: usize) -> (f32, f32, f32) {
    let hk = fit.0.rescaled(1.0 / f64::powi(2.0, k as i32));
    let g = gradients(f);
    let mut res: Vec<f32> = (0..r.h)
        .into_par_iter()
        .flat_map_iter(|y| {
            let (g, hk) = (&g, &hk);
            (0..r.w).filter_map(move |x| {
                let i = y * r.w + x;
                if !r.valid[i] {
                    return None;
                }
                let (u, v) = hk.apply(x as f64, y as f64);
                sample(f, g, u, v).map(|s| s.l - fit.1 - r.l[i])
            })
        })
        .collect();
    let overlap = res.len() as f32 / (r.w * r.h) as f32;
    let mut abs: Vec<f32> = res.iter().map(|v| v.abs()).collect();
    let Some(mad) = median(&mut abs) else {
        return (f32::INFINITY, overlap, f32::INFINITY);
    };
    let scale = 1.4826 * mad;
    let kh = (1.345 * scale).max(1e-4);
    let mean = res
        .iter_mut()
        .map(|r| {
            let a = r.abs();
            if a <= kh {
                0.5 * a * a
            } else {
                kh * (a - 0.5 * kh)
            }
        })
        .sum::<f32>()
        / res.len().max(1) as f32;
    (mean, overlap, scale)
}

/// `a · x = b` for a small symmetric system, by Gaussian elimination with partial pivoting.
fn solve(mut a: Vec<f64>, mut b: Vec<f64>, n: usize) -> Option<Vec<f64>> {
    for col in 0..n {
        let piv =
            (col..n).max_by(|&i, &j| a[i * n + col].abs().total_cmp(&a[j * n + col].abs()))?;
        if a[piv * n + col].abs() < 1e-12 {
            return None;
        }
        if piv != col {
            for c in 0..n {
                a.swap(piv * n + c, col * n + c);
            }
            b.swap(piv, col);
        }
        for row in col + 1..n {
            let f = a[row * n + col] / a[col * n + col];
            for c in col..n {
                a[row * n + c] -= f * a[col * n + c];
            }
            b[row] -= f * b[col];
        }
    }
    let mut x = vec![0f64; n];
    for row in (0..n).rev() {
        let s: f64 = (row + 1..n).map(|c| a[row * n + c] * x[c]).sum();
        x[row] = (b[row] - s) / a[row * n + row];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::homography::Homography;
    use crate::pyramid::LumPyramid;
    use crate::synth::{self, turn, Mover};

    const W: usize = 512;
    const H: usize = 384;

    fn pyramids(frames: &[crate::frame::Frame]) -> Vec<LumPyramid> {
        frames
            .iter()
            .map(|f| LumPyramid::build(f, COARSEST))
            .collect()
    }

    /// ⚠⚠ The acceptance number: a handheld shift and turn, frames two stops either side of the
    /// reference, recovered to within five hundredths of a pixel at every corner.
    #[test]
    fn a_shift_and_a_turn_are_recovered_within_five_hundredths_of_a_pixel() {
        let truth = [
            turn(0.7, 13.4, -6.2, W, H),
            Homography::identity(),
            turn(-0.4, -8.3, 4.1, W, H),
        ];
        let (frames, _) = synth::bracket(W, H, &[-2.0, 0.0, 2.0], &truth, 1, None);
        let p = pyramids(&frames);
        for i in [0, 2] {
            let a = align(&p[1], &p[i], None, 0);
            let err = a.h.corner_shift(&truth[i], W, H);
            eprintln!(
                "frame {i}: corner error {err:.4} px, gain {:.4}",
                a.log_gain
            );
            assert!(err < 0.05, "frame {i}: {err} px");
        }
    }

    #[test]
    fn a_little_perspective_is_recovered() {
        let mut keystone = turn(0.3, 5.0, 2.0, W, H);
        keystone.0[6] = 1.2e-5;
        keystone.0[7] = -0.8e-5;
        let truth = [Homography::identity(), keystone];
        let (frames, _) = synth::bracket(W, H, &[0.0, -1.0], &truth, 2, None);
        let p = pyramids(&frames);
        let a = align(&p[0], &p[1], None, 0);
        let err = a.h.corner_shift(&truth[1], W, H);
        eprintln!("perspective: corner error {err:.4} px");
        assert!(err < 0.05, "{err} px");
    }

    /// The frame's exposure relative to the reference falls out of the fit.
    #[test]
    fn the_exposure_offset_comes_out_of_the_alignment() {
        let truth = [Homography::translation(3.3, -2.1), Homography::identity()];
        let (frames, _) = synth::bracket(W, H, &[-2.0, 0.0], &truth, 3, None);
        let p = pyramids(&frames);
        let a = align(&p[1], &p[0], None, 0);
        assert!((a.log_gain + 2.0).abs() < 0.01, "log gain {}", a.log_gain);
    }

    /// A fifth of the frame changed in one frame — someone walking through — must not pull the fit.
    #[test]
    fn a_moving_square_does_not_pull_the_fit() {
        let truth = [Homography::identity(), turn(0.5, 7.0, 3.0, W, H)];
        let mover = Mover {
            frame: 1,
            rect: (60.0, 60.0, 60.0 + 0.45 * W as f64, 60.0 + 0.45 * H as f64),
            gain: 3.0,
        };
        let (frames, _) = synth::bracket(W, H, &[0.0, 1.0], &truth, 4, Some(mover));
        let p = pyramids(&frames);
        let a = align(&p[0], &p[1], None, 0);
        let err = a.h.corner_shift(&truth[1], W, H);
        eprintln!("mover: corner error {err:.4} px");
        assert!(err < 0.1, "{err} px");
    }

    /// A black frame shares nothing with the reference: no panic, nothing non-finite, the
    /// identity, and an overlap that says so.
    #[test]
    fn a_frame_with_nothing_in_common_falls_back_to_identity() {
        let (frames, _) = synth::bracket(W, H, &[0.0], &[Homography::identity()], 5, None);
        let black = crate::frame::Frame::new(W, H, vec![0; W * H * 3], synth::PRE_MUL, None);
        let a = align(
            &LumPyramid::build(&frames[0], COARSEST),
            &LumPyramid::build(&black, COARSEST),
            None,
            0,
        );
        assert!(a.overlap < MIN_OVERLAP, "overlap {}", a.overlap);
        assert_eq!(a.h, Homography::identity());
        assert!(a.log_gain.is_finite());
    }
}
