//! Seams (M10.17b): which frame each canvas pixel is taken from. Frames are laid in the row's order; where
//! the next frame overlaps what is already laid, a minimum-cost path crosses the overlap (top to bottom for
//! a horizontal row, left to right for a vertical one) and the next frame takes everything past it.
//!
//! The cost is high where the two frames **disagree** — their gain-matched log luminance differs: a moving
//! person, a branch, a parallax error — where **either is clipped**, and a little on strong edges; outside the
//! overlap it is higher than anything inside, so the path stays in it wherever the overlap exists. So the seam
//! goes AROUND something that moved, which then comes whole from one frame: deghosting with no option.
//!
//! ⚠ The path is found at full resolution (dynamic programming, three moves a row) — the spec allowed a
//! reduced grid refined in a band, but an overlap at the 16,000-px cap is ~11 M cells, a fraction of a second.

use crate::pano_warp::Warped;
use crate::projection::Canvas;
use crate::pyramid::FLOOR;
use crate::rig::Sweep;

/// The weight of a disagreement, per stop of log-luminance difference.
pub const DISAGREE: f32 = 1.0;
/// The cost of a pixel either frame has clipped.
pub const CLIPPED: f32 = 4.0;
/// The weight of the local gradient, per stop per pixel.
pub const EDGE: f32 = 0.1;
/// A disagreement under this many stops is noise and texture, and is not widened.
pub const NOISE_STOPS: f32 = 0.2;
/// How far, in canvas pixels, the seam keeps from a disagreement.
pub const KEEP_AWAY: usize = 48;
/// The cost of a pixel outside the overlap — above anything inside it.
const OUTSIDE: f32 = 1e3;

/// Which frame owns each canvas pixel.
#[derive(Clone, Debug, PartialEq)]
pub struct Labels {
    pub width: usize,
    pub height: usize,
    /// The frame's index, or [`NONE`].
    pub owner: Vec<u8>,
}

/// No frame covers the pixel.
pub const NONE: u8 = u8::MAX;

fn clipped_at(w: &Warped, x: usize, y: usize) -> bool {
    w.index(x, y).is_some_and(|i| w.clipped[i])
}

/// The labels for `warped` (indexed by frame) laid in `order`.
pub fn cut(canvas: &Canvas, warped: &[Warped], order: &[usize]) -> Labels {
    let (w, h) = (canvas.width, canvas.height);
    let mut owner = vec![NONE; w * h];
    let lum = |p: [f32; 3]| (0.25 * p[0] + 0.5 * p[1] + 0.25 * p[2]).max(FLOOR).log2();
    for (k, &b) in order.iter().enumerate() {
        let wb = &warped[b];
        if k == 0 {
            for y in wb.y0..wb.y0 + wb.height {
                for x in wb.x0..wb.x0 + wb.width {
                    if wb.at(x, y).is_some() {
                        owner[y * w + x] = b as u8;
                    }
                }
            }
            continue;
        }
        // The path runs along `t` (rows for a horizontal row), one `s` per `t`; `b` takes `s ≥ path`.
        let horizontal = canvas.axis == Sweep::Horizontal;
        let (s0, s1, t0, t1) = if horizontal {
            (wb.x0, wb.x0 + wb.width, wb.y0, wb.y0 + wb.height)
        } else {
            (wb.y0, wb.y0 + wb.height, wb.x0, wb.x0 + wb.width)
        };
        let at = |s: usize, t: usize| if horizontal { (s, t) } else { (t, s) };
        let ns = s1 - s0;
        if ns == 0 {
            continue;
        }
        let nt = t1 - t0;
        // Per cell: inside the overlap, its disagreement and the rest of its cost; outside, `None`.
        let b_lum = |x: usize, y: usize| wb.at(x, y).map(lum);
        let cells: Vec<Option<(f32, f32)>> = (0..nt * ns)
            .map(|k| {
                let (x, y) = at(s0 + k % ns, t0 + k / ns);
                let o = owner[y * w + x];
                let ib = wb.index(x, y).filter(|&i| wb.valid[i])?;
                let wa = warped.get(o as usize)?;
                let ia = wa.index(x, y).filter(|&i| wa.valid[i])?;
                let (pa, pb) = (wa.at(x, y)?, wb.at(x, y)?);
                let lb = lum(pb);
                let mut rest = 0.0;
                if wa.clipped[ia] || wb.clipped[ib] {
                    rest += CLIPPED;
                }
                let g = |dx: i64, dy: i64| {
                    let (xx, yy) = (x as i64 + dx, y as i64 + dy);
                    (xx >= 0 && yy >= 0)
                        .then(|| b_lum(xx as usize, yy as usize))
                        .flatten()
                };
                let gx = g(1, 0)
                    .zip(g(-1, 0))
                    .map_or(0.0, |(p, m)| (p - m).abs() / 2.0);
                let gy = g(0, 1)
                    .zip(g(0, -1))
                    .map_or(0.0, |(p, m)| (p - m).abs() / 2.0);
                rest += EDGE * (gx + gy);
                Some(((lum(pa) - lb).abs(), rest))
            })
            .collect();
        // The disagreement widened by a max filter, so the path keeps its distance from what moved: the
        // blend's coarse bands reach that far across a seam and would carry a ghost over.
        let near = max_filter(
            &cells
                .iter()
                .map(|c| c.map_or(0.0, |c| (c.0 - NOISE_STOPS).max(0.0)))
                .collect::<Vec<_>>(),
            ns,
            nt,
            KEEP_AWAY,
        );
        let grid: Vec<f32> = cells
            .iter()
            .zip(&near)
            .map(|(c, d)| c.map_or(OUTSIDE, |(dis, rest)| DISAGREE * (dis + d) + rest))
            .collect();
        let cost = |s: usize, t: usize| grid[(t - t0) * ns + (s - s0)];
        // Where exactly one of the two is clipped the other must be used. ⚠ No margin: one of 8 px pushed
        // ownership out to a frame's coverage edge and cost 0.066 stops on the synthetic row; the blend keeps
        // the clipped light from spreading instead (`blend::multiband`'s fill).
        let clip_map = |want_b: bool| -> Vec<f32> {
            let map: Vec<f32> = (0..nt * ns)
                .map(|k| {
                    let (x, y) = at(s0 + k % ns, t0 + k / ns);
                    let o = owner[y * w + x];
                    let Some(wa) = warped.get(o as usize) else {
                        return 0.0;
                    };
                    let (ca, cb) = (clipped_at(wa, x, y), clipped_at(wb, x, y));
                    let only = if want_b { ca && !cb } else { cb && !ca };
                    if only && wb.at(x, y).is_some() {
                        1.0
                    } else {
                        0.0
                    }
                })
                .collect();
            map
        };
        let (to_b, to_a) = (clip_map(true), clip_map(false));
        // Dynamic programming: the cheapest path, three moves a step.
        let mut acc: Vec<f32> = (0..ns).map(|i| cost(s0 + i, t0)).collect();
        let mut back = vec![0i8; ns * nt];
        for j in 1..nt {
            let row: Vec<f32> = (0..ns).map(|i| cost(s0 + i, t0 + j)).collect();
            let mut next = vec![0f32; ns];
            for i in 0..ns {
                let (mut best, mut mv) = (acc[i], 0i8);
                if i > 0 && acc[i - 1] < best {
                    (best, mv) = (acc[i - 1], -1);
                }
                if i + 1 < ns && acc[i + 1] < best {
                    (best, mv) = (acc[i + 1], 1);
                }
                next[i] = best + row[i];
                back[j * ns + i] = mv;
            }
            acc = next;
        }
        let mut path = vec![0usize; nt];
        let mut i = (0..ns)
            .min_by(|&p, &q| acc[p].total_cmp(&acc[q]))
            .unwrap_or(0);
        for j in (0..nt).rev() {
            path[j] = i;
            i = (i as i64 + back[j * ns + i] as i64) as usize;
        }
        for (j, &p) in path.iter().enumerate() {
            for i in 0..ns {
                let (x, y) = at(s0 + i, t0 + j);
                if wb.at(x, y).is_none() {
                    continue;
                }
                let o = &mut owner[y * w + x];
                if *o == NONE {
                    *o = b as u8;
                    continue;
                }
                // Past the path `b` takes it — unless only one of the two holds the light near here: then that
                // one does, whichever side it is on. One path cannot go round a sky band that runs the whole
                // width of the overlap.
                let k = j * ns + i;
                let (force_b, force_a) = (to_b[k] > 0.0, to_a[k] > 0.0);
                if (force_b && !force_a) || (i >= p && !(force_a && !force_b)) {
                    *o = b as u8;
                }
            }
        }
    }
    Labels {
        width: w,
        height: h,
        owner,
    }
}

/// The maximum over a `(2r + 1)²` square around each cell of a `w × h` grid (separable).
fn max_filter(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let pass = |src: &[f32], horizontal: bool| -> Vec<f32> {
        let mut out = vec![0f32; w * h];
        let (n, m) = if horizontal { (h, w) } else { (w, h) };
        let idx = |line: usize, k: usize| {
            if horizontal {
                line * w + k
            } else {
                k * w + line
            }
        };
        let mut window: std::collections::VecDeque<(usize, f32)> =
            std::collections::VecDeque::new();
        for line in 0..n {
            window.clear();
            let mut next = 0;
            for k in 0..m {
                while next < m && next <= k + r {
                    let v = src[idx(line, next)];
                    while window.back().is_some_and(|b| b.1 <= v) {
                        window.pop_back();
                    }
                    window.push_back((next, v));
                    next += 1;
                }
                while window.front().is_some_and(|f| f.0 + r < k) {
                    window.pop_front();
                }
                out[idx(line, k)] = window.front().map_or(0.0, |f| f.1);
            }
        }
        out
    };
    pass(&pass(src, true), false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pano_warp::tests::stage;
    use crate::synth_pano::{self, Scene};

    /// Every covered pixel has an owner that covers it, and every uncovered one has none.
    #[test]
    fn every_pixel_is_owned_by_a_frame_that_covers_it() {
        let s = stage(synth_pano::row(4, 0.33, false), Scene::new(1), true);
        let l = cut(&s.canvas, &s.warped, &s.rig.order);
        for v in (0..l.height).step_by(3) {
            for u in (0..l.width).step_by(3) {
                let o = l.owner[v * l.width + u];
                let any = s.warped.iter().any(|w| w.at(u, v).is_some());
                if o == NONE {
                    assert!(!any, "({u}, {v}) is covered but unowned");
                } else {
                    assert!(
                        s.warped[o as usize].at(u, v).is_some(),
                        "({u}, {v}) owned by {o}, which does not cover it"
                    );
                }
            }
        }
    }

    #[test]
    fn the_seam_avoids_clipped_sky() {
        let mut r = synth_pano::row(3, 0.4, false);
        r.shots.iter_mut().for_each(|s| s.vignette = 0.0);
        r.shots[2].gain = 2.8;
        r.shots[2].pitch = 4f64.to_radians();
        let s = stage(r, Scene::new(1), true);
        let l = cut(&s.canvas, &s.warped, &s.rig.order);
        let (mut overlap_clipped, mut taken_clipped) = (0usize, 0usize);
        for v in 0..l.height {
            for u in 0..l.width {
                let (a, b) = (&s.warped[1], &s.warped[2]);
                let (Some(ia), Some(ib)) = (a.index(u, v), b.index(u, v)) else {
                    continue;
                };
                if !(a.valid[ia] && b.valid[ib]) || !b.clipped[ib] || a.clipped[ia] {
                    continue;
                }
                overlap_clipped += 1;
                taken_clipped += (l.owner[v * l.width + u] == 2) as usize;
            }
        }
        assert!(
            overlap_clipped > 500,
            "the fixture must clip frame 2 in the overlap ({overlap_clipped})"
        );
        assert!(
            taken_clipped * 100 < overlap_clipped,
            "{taken_clipped} of {overlap_clipped} clipped pixels taken"
        );
    }
}
