//! Deghost: what moved between frames is taken from one frame alone.
//!
//! On a quarter-size grid of the reference, every frame's luminance — aligned, scaled to the
//! reference's exposure, in stops — is compared with a **local reference**: the reference frame where
//! it holds the light, else the frame nearest it in exposure that does (in the reference's clipped
//! highlights, the next darker frame). A frame differing from it by more than the level's threshold
//! is marked there; the marks are cleaned (a 3 × 3 vote), grown by the level's radius and feathered
//! into a 0–1 mask that removes that frame's weight in the merge. The local reference's own mask is
//! always 0, so every region is taken whole from one consistent frame.
//!
//! ⚠ The thresholds and radii are **starting points for the author to tune on crops** — photographic
//! judgements are his (the grain precedent). The probe prints all four levels side by side.

use crate::frame::Frame;
use crate::homography::Homography;
use crate::pyramid::FLOOR;

/// How hard deghosting works — Lightroom's four.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Deghost {
    #[default]
    None,
    Low,
    Medium,
    High,
}

impl Deghost {
    pub const ALL: [Deghost; 4] = [Deghost::None, Deghost::Low, Deghost::Medium, Deghost::High];

    pub fn label(self) -> &'static str {
        match self {
            Deghost::None => "None",
            Deghost::Low => "Low",
            Deghost::Medium => "Medium",
            Deghost::High => "High",
        }
    }

    /// How far, in stops, a frame may differ from the local reference before it counts as moved.
    pub fn threshold_ev(self) -> Option<f32> {
        match self {
            Deghost::None => None,
            Deghost::Low => Some(0.6),
            Deghost::Medium => Some(0.4),
            Deghost::High => Some(0.25),
        }
    }

    /// How far a mark grows, as a share of the frame's long edge.
    pub fn radius(self) -> f32 {
        match self {
            Deghost::None => 0.0,
            Deghost::Low => 0.003,
            Deghost::Medium => 0.006,
            Deghost::High => 0.010,
        }
    }
}

/// The grid's cell, in reference pixels.
const CELL: usize = 4;

/// Each frame's ghost mask (`None` where it is never masked) and their union — the overlay — on a
/// quarter-size grid of the reference. `None` and no overlay at [`Deghost::None`].
pub fn masks(
    frames: &[Frame],
    reference: usize,
    to_frame: &[Homography],
    scale: &[f32],
    level: Deghost,
) -> (Vec<Option<Mask>>, Option<Mask>) {
    let n = frames.len();
    let Some(threshold) = level.threshold_ev() else {
        return (vec![None; n], None);
    };
    let (w, h) = (frames[reference].width, frames[reference].height);
    let (qw, qh) = (w.div_ceil(CELL), h.div_ceil(CELL));

    // Each frame's scaled log luminance per cell, `None` where it does not hold the light.
    let planes: Vec<Vec<Option<f32>>> = frames
        .iter()
        .enumerate()
        .map(|(i, f)| cells(f, &to_frame[i], scale[i], w, h, qw, qh))
        .collect();

    // Nearest the reference in exposure first.
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&a, &b| scale[a].log2().abs().total_cmp(&scale[b].log2().abs()));
    order.retain(|&i| i != reference);
    order.insert(0, reference);

    let local: Vec<Option<usize>> = (0..qw * qh)
        .map(|q| order.iter().copied().find(|&i| planes[i][q].is_some()))
        .collect();

    let r = (level.radius() * qw.max(qh) as f32).round().max(1.0) as usize;
    let out: Vec<Option<Mask>> = (0..n)
        .map(|i| {
            let mut marks = vec![0f32; qw * qh];
            let mut any = false;
            for q in 0..qw * qh {
                let Some(l) = local[q] else { continue };
                if l == i {
                    continue;
                }
                if let (Some(a), Some(b)) = (planes[i][q], planes[l][q]) {
                    if (a - b).abs() > threshold {
                        marks[q] = 1.0;
                        any = true;
                    }
                }
            }
            if !any {
                return None;
            }
            let voted: Vec<f32> = box_mean(&marks, qw, qh, 1)
                .into_iter()
                .map(|v| if v > 0.5 { 1.0 } else { 0.0 })
                .collect();
            let grown = dilate(&voted, qw, qh, r);
            let mut data = box_mean(
                &box_mean(&grown, qw, qh, r.div_ceil(2)),
                qw,
                qh,
                r.div_ceil(2),
            );
            for q in 0..qw * qh {
                if local[q] == Some(i) {
                    data[q] = 0.0;
                }
            }
            data.iter()
                .any(|v| *v > 0.0)
                .then_some(Mask { w: qw, h: qh, data })
        })
        .collect();

    let mut union = vec![0f32; qw * qh];
    for m in out.iter().flatten() {
        for (u, v) in union.iter_mut().zip(&m.data) {
            *u = u.max(*v);
        }
    }
    (
        out,
        Some(Mask {
            w: qw,
            h: qh,
            data: union,
        }),
    )
}

/// One frame's cells: the mean scaled raw luminance of the reference pixels a cell covers, read where
/// the alignment puts them, in stops — `None` unless every one is inside, unclipped, below the
/// roll-off's end and above [`FLOOR`] as the frame itself showed it.
fn cells(
    f: &Frame,
    to: &Homography,
    scale: f32,
    w: usize,
    h: usize,
    qw: usize,
    qh: usize,
) -> Vec<Option<f32>> {
    use rayon::prelude::*;
    (0..qh)
        .into_par_iter()
        .flat_map_iter(|qy| {
            (0..qw).map(move |qx| {
                let (mut sum, mut cnt) = (0f32, 0usize);
                for y in qy * CELL..((qy + 1) * CELL).min(h) {
                    for x in qx * CELL..((qx + 1) * CELL).min(w) {
                        let (u, v) = to.apply(x as f64, y as f64);
                        let (u, v) = (u.round(), v.round());
                        if u < 0.0 || v < 0.0 || u as usize >= f.width || v as usize >= f.height {
                            return None;
                        }
                        let (u, v) = (u as usize, v as usize);
                        let shown = f.luma(u, v);
                        if f.clipped(u, v) || shown > crate::merge::ROLL_END || shown < FLOOR {
                            return None;
                        }
                        let raw = f.raw(u, v);
                        sum += 0.25 * raw[0] + 0.5 * raw[1] + 0.25 * raw[2];
                        cnt += 1;
                    }
                }
                (cnt > 0 && sum > 0.0).then(|| (sum / cnt as f32 * scale).log2())
            })
        })
        .collect()
}

/// A separable box mean of radius `r`, the edges clamped.
fn box_mean(v: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let pass = |src: &[f32], horizontal: bool| -> Vec<f32> {
        let mut out = vec![0f32; w * h];
        let (len, lines) = if horizontal { (w, h) } else { (h, w) };
        for line in 0..lines {
            let at = |i: usize| {
                if horizontal {
                    line * w + i
                } else {
                    i * w + line
                }
            };
            let mut prefix = vec![0f32; len + 1];
            for i in 0..len {
                prefix[i + 1] = prefix[i] + src[at(i)];
            }
            for i in 0..len {
                let (a, b) = (i.saturating_sub(r), (i + r + 1).min(len));
                out[at(i)] = (prefix[b] - prefix[a]) / (b - a) as f32;
            }
        }
        out
    };
    pass(&pass(v, true), false)
}

/// A separable square dilation of radius `r` (a cell becomes the largest within `r`).
fn dilate(v: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let pass = |src: &[f32], horizontal: bool| -> Vec<f32> {
        let mut out = vec![0f32; w * h];
        let (len, lines) = if horizontal { (w, h) } else { (h, w) };
        for line in 0..lines {
            let at = |i: usize| {
                if horizontal {
                    line * w + i
                } else {
                    i * w + line
                }
            };
            for i in 0..len {
                let (a, b) = (i.saturating_sub(r), (i + r + 1).min(len));
                out[at(i)] = (a..b).map(|k| src[at(k)]).fold(0f32, f32::max);
            }
        }
        out
    };
    pass(&pass(v, true), false)
}

/// A plane over the reference frame, at its own (smaller) size — a ghost mask, 0–1.
#[derive(Clone, Debug, PartialEq)]
pub struct Mask {
    pub w: usize,
    pub h: usize,
    pub data: Vec<f32>,
}

impl Mask {
    /// The value at normalised `(u, v)` ∈ [0, 1]², bilinear between cell centres.
    pub fn at(&self, u: f32, v: f32) -> f32 {
        let x = (u * self.w as f32 - 0.5).clamp(0.0, (self.w - 1) as f32);
        let y = (v * self.h as f32 - 0.5).clamp(0.0, (self.h - 1) as f32);
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = ((x0 + 1).min(self.w - 1), (y0 + 1).min(self.h - 1));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let p = |xx: usize, yy: usize| self.data[yy * self.w + xx];
        (p(x0, y0) * (1.0 - fx) + p(x1, y0) * fx) * (1.0 - fy)
            + (p(x0, y1) * (1.0 - fx) + p(x1, y1) * fx) * fy
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::homography::Homography;
    use crate::merge::merge;
    use crate::synth::{self, Mover};

    const W: usize = 320;
    const H: usize = 240;

    fn ids(n: usize) -> Vec<Homography> {
        vec![Homography::identity(); n]
    }

    fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> (f64, f64, f64, f64) {
        (x0 * W as f64, y0 * H as f64, x1 * W as f64, y1 * H as f64)
    }

    /// The mean relative error of the merge's green against the scene inside `r` — away from its
    /// edge by 6 px, where the feathering legitimately mixes.
    fn error_inside(
        out: &core_image::ImageBuf,
        truth: &core_image::ImageBuf,
        r: (f64, f64, f64, f64),
    ) -> f64 {
        let (mut s, mut n) = (0f64, 0usize);
        for y in (r.1 as usize + 6)..(r.3 as usize - 6) {
            for x in (r.0 as usize + 6)..(r.2 as usize - 6) {
                let t = truth.pixel(x, y)[1];
                s += ((out.pixel(x, y)[1] - t) / t).abs() as f64;
                n += 1;
            }
        }
        s / n as f64
    }

    /// Something darker appears in the brightest frame — the frame the merge trusts most in the
    /// shadows. At Medium it is gone (the reference's view stands); at None it shows — the control.
    #[test]
    fn a_moving_square_is_gone_at_medium() {
        let r = rect(0.2, 0.2, 0.4, 0.45);
        let mover = Mover {
            frame: 2,
            rect: r,
            gain: 0.25,
        };
        let (frames, truth) = synth::bracket(W, H, &[-2.0, 0.0, 2.0], &ids(3), 12, Some(mover));
        let scale = [4.0, 1.0, 0.25];
        let run = |level: Deghost| {
            let (ghost, _) = masks(&frames, 1, &ids(3), &scale, level);
            merge(&frames, 1, &ids(3), &scale, &ghost)
        };
        let (gone, shown) = (
            error_inside(&run(Deghost::Medium), &truth, r),
            error_inside(&run(Deghost::None), &truth, r),
        );
        eprintln!("inside the mover: Medium {gone:.4}, None {shown:.4}");
        assert!(
            shown > 0.10,
            "the control: at None the mover must show ({shown})"
        );
        assert!(gone < 0.03, "at Medium the mover is still there ({gone})");
    }

    #[test]
    fn a_still_scene_has_an_empty_mask() {
        let (frames, _) = synth::bracket(W, H, &[-2.0, 0.0, 2.0], &ids(3), 13, None);
        let (_, overlay) = masks(&frames, 1, &ids(3), &[4.0, 1.0, 0.25], Deghost::High);
        let o = overlay.expect("an overlay at High");
        let marked = o.data.iter().filter(|v| **v > 0.5).count() as f32 / o.data.len() as f32;
        eprintln!("still scene, High: {:.4} of the frame marked", marked);
        assert!(marked < 0.005, "{marked}");
    }

    /// Where the reference clipped, the next-darker frame is the local reference: something moving
    /// in the darkest frame there is taken from the frame next to the reference.
    ///
    /// ⚠ The rectangle and the bracket are chosen so the NEXT-darker frame holds the light under the
    /// reference's clipping — the first versions' frame 1 clipped there too (in 10 % of the pixels the
    /// second time), leaving the darkest frame the only witness, which nothing can overrule.
    #[test]
    fn the_local_reference_takes_over_where_the_reference_clipped() {
        let r = rect(0.74, 0.66, 0.86, 0.8);
        let mover = Mover {
            frame: 2,
            rect: r,
            gain: 0.25,
        };
        let (frames, truth) = synth::bracket(W, H, &[0.0, -2.0, -4.0], &ids(3), 14, Some(mover));
        let inside = || {
            (r.1 as usize..r.3 as usize)
                .flat_map(move |y| (r.0 as usize..r.2 as usize).map(move |x| (x, y)))
        };
        let clipped = inside().filter(|&(x, y)| frames[0].clipped(x, y)).count();
        let held = inside()
            .filter(|&(x, y)| frames[0].clipped(x, y) && !frames[1].clipped(x, y))
            .count();
        eprintln!("under the mover: reference clipped {clipped}, of them frame 1 held {held}");
        assert!(
            held > 300,
            "frame 1 must hold the light where the reference clipped ({held})"
        );
        let lost = inside().filter(|&(x, y)| frames[1].clipped(x, y)).count();
        assert!(
            lost < 20,
            "frame 1 must hold it everywhere under the mover ({lost} clipped)"
        );
        let scale = [1.0, 4.0, 16.0];
        let run = |level: Deghost| {
            let (ghost, _) = masks(&frames, 0, &ids(3), &scale, level);
            merge(&frames, 0, &ids(3), &scale, &ghost)
        };
        let (gone, shown) = (
            error_inside(&run(Deghost::Medium), &truth, r),
            error_inside(&run(Deghost::None), &truth, r),
        );
        eprintln!("under the clipped reference: Medium {gone:.4}, None {shown:.4}");
        assert!(
            shown > 0.05,
            "the control: at None the mover must show ({shown})"
        );
        assert!(gone < 0.03, "{gone}");
    }

    #[test]
    fn the_overlay_marks_what_was_taken_from_one_frame() {
        let r = rect(0.2, 0.2, 0.4, 0.45);
        let mover = Mover {
            frame: 2,
            rect: r,
            gain: 0.25,
        };
        let (frames, _) = synth::bracket(W, H, &[-2.0, 0.0, 2.0], &ids(3), 15, Some(mover));
        let (_, overlay) = masks(&frames, 1, &ids(3), &[4.0, 1.0, 0.25], Deghost::Medium);
        let o = overlay.expect("an overlay");
        let at = |x: f64, y: f64| o.at((x / W as f64) as f32, (y / H as f64) as f32);
        assert!(
            at((r.0 + r.2) / 2.0, (r.1 + r.3) / 2.0) > 0.9,
            "the mover's centre"
        );
        assert!(at(0.8 * W as f64, 0.2 * H as f64) < 0.01, "far from it");
        assert!(masks(&frames, 1, &ids(3), &[4.0, 1.0, 0.25], Deghost::None)
            .1
            .is_none());
    }
}
