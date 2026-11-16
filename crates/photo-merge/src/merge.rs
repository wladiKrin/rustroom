//! The merge: every frame's light on the reference's grid, weighted by how much it can be trusted.
//!
//! Per pixel and frame: the frame's normalised raw signal where the alignment puts it (Catmull-Rom;
//! nothing from a clipped tap or outside), scaled to the reference's exposure, weighted by
//! - its **exposure** (`1 / scale`): a brighter frame caught more light, so less noise — the
//!   noise-optimal weight for a photon-limited sensor;
//! - a **highlight roll-off** on what the frame showed (its brightest white-balanced channel): full
//!   below [`ROLL_START`], smoothly to none at [`ROLL_END`], so no seam appears where a darker frame
//!   takes over;
//! - `1 − ghost`.
//!
//! Where no frame has a weight — every one clipped, or the frame's edge — the frame with the most
//! trust by nearest sample stands, the darkest when all are clipped: a highlight no frame held.

use crate::deghost::Mask;
use crate::frame::Frame;
use crate::homography::Homography;
use crate::warp::Sampler;
use core_image::ImageBuf;
use rayon::prelude::*;

/// Below this share of white (white-balanced), a frame's sample has its full weight.
pub const ROLL_START: f32 = 0.80;
/// At this share of white, none.
pub const ROLL_END: f32 = 0.96;

/// The roll-off for a sample the frame showed at `m` of white.
#[inline]
fn roll(m: f32) -> f32 {
    let t = ((m - ROLL_START) / (ROLL_END - ROLL_START)).clamp(0.0, 1.0);
    1.0 - t * t * (3.0 - 2.0 * t)
}

#[inline]
fn shown(raw: [f32; 3], pre: [f32; 3]) -> f32 {
    (0..3).map(|c| raw[c] * pre[c]).fold(0f32, f32::max)
}

/// Merge `frames` onto the reference's grid: `to_frame[i]` maps reference pixels to frame `i`'s,
/// `scale[i]` is its `reference / frame` exposure, `ghost[i]` its ghost mask. Normalised raw camera
/// RGB at the reference's exposure.
pub fn merge(
    frames: &[Frame],
    reference: usize,
    to_frame: &[Homography],
    scale: &[f32],
    ghost: &[Option<Mask>],
) -> ImageBuf {
    let (w, h) = (frames[reference].width, frames[reference].height);
    let sampler = Sampler::new();
    let darkest = (0..frames.len())
        .max_by(|&a, &b| scale[a].total_cmp(&scale[b]))
        .unwrap_or(reference);
    let data: Vec<f32> = (0..h)
        .into_par_iter()
        .flat_map_iter(|y| {
            let sampler = &sampler;
            (0..w).flat_map(move |x| {
                let (gu, gv) = ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
                let mut acc = [0f32; 3];
                let mut wsum = 0f32;
                for (i, f) in frames.iter().enumerate() {
                    let (u, v) = to_frame[i].apply(x as f64, y as f64);
                    let Some(raw) = sampler.raw_at(f, u as f32, v as f32) else {
                        continue;
                    };
                    let g = ghost[i].as_ref().map_or(0.0, |m| m.at(gu, gv));
                    let wt = roll(shown(raw, f.pre_mul)) * (1.0 - g) / scale[i];
                    if wt > 0.0 && wt.is_finite() {
                        for c in 0..3 {
                            acc[c] += wt * raw[c] * scale[i];
                        }
                        wsum += wt;
                    }
                }
                if wsum > 0.0 {
                    return acc.map(|a| a / wsum);
                }
                fallback(frames, to_frame, scale, darkest, x, y)
            })
        })
        .collect();
    ImageBuf::from_vec(w, h, data)
}

/// No frame has a weight here: the nearest sample of the frame trusted most — by exposure and
/// roll-off, clipping allowed — or the darkest frame's when every one is clipped.
fn fallback(
    frames: &[Frame],
    to_frame: &[Homography],
    scale: &[f32],
    darkest: usize,
    x: usize,
    y: usize,
) -> [f32; 3] {
    let near = |i: usize| -> Option<(usize, usize)> {
        let (u, v) = to_frame[i].apply(x as f64, y as f64);
        let (u, v) = (u.round(), v.round());
        let f = &frames[i];
        (u >= 0.0 && v >= 0.0 && (u as usize) < f.width && (v as usize) < f.height)
            .then_some((u as usize, v as usize))
    };
    let best = (0..frames.len())
        .filter_map(|i| {
            let (u, v) = near(i)?;
            let f = &frames[i];
            let trust = if f.clipped(u, v) {
                0.0
            } else {
                roll(shown(f.raw(u, v), f.pre_mul)) / scale[i]
            };
            Some((i, trust))
        })
        .max_by(|a, b| a.1.total_cmp(&b.1));
    let pick = match best {
        Some((i, t)) if t > 0.0 => i,
        _ => darkest,
    };
    let (u, v) = near(pick).unwrap_or((
        x.min(frames[pick].width - 1),
        y.min(frames[pick].height - 1),
    ));
    frames[pick].raw(u, v).map(|c| c * scale[pick])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::Frame;
    use crate::homography::Homography;
    use crate::synth;

    fn ids(n: usize) -> Vec<Homography> {
        vec![Homography::identity(); n]
    }

    /// ⚠ Non-finite FIRST — `f32::max` ignores NaN, so a NaN output would otherwise read as fine.
    fn assert_finite(img: &ImageBuf) {
        let bad = img.data().iter().filter(|v| !v.is_finite()).count();
        assert_eq!(bad, 0, "{bad} non-finite samples");
    }

    /// ⚠⚠ Wherever any frame held the light, the merge is the scene — within its noise. −2/0/+2 over
    /// a 12-stop scene, three claims, each against the truth:
    /// - **no bias**: the signed error averages under 0.5 %;
    /// - the highlights the reference **clipped** come back within the dark frame's own noise;
    /// - where the reference held the light, the merge is **less noisy than the reference alone**
    ///   (the control: merging must buy something).
    ///
    /// ⚠ Not "mean |error| < 1 %": the fixture's shot noise is ~5 % a pixel near the noise floor, so
    /// that number measured the fixture (1.08 %), not the merge — `(d)` of AGENTS.md's shapes.
    #[test]
    fn radiance_is_recovered_where_any_frame_held_it() {
        let (w, h) = (320, 240);
        let (frames, truth) = synth::bracket(w, h, &[-2.0, 0.0, 2.0], &ids(3), 9, None);
        let out = merge(&frames, 1, &ids(3), &[4.0, 1.0, 0.25], &[None, None, None]);
        assert_finite(&out);
        let (mut signed, mut n) = (0f64, 0usize);
        let (mut hi_err, mut hi_n) = (0f64, 0usize);
        let (mut merged, mut alone, mut both_n) = (0f64, 0f64, 0usize);
        for y in 2..h - 2 {
            for x in 2..w - 2 {
                let t = truth.pixel(x, y)[1];
                if frames[0].clipped(x, y) || t < 1.0 / 256.0 {
                    continue;
                }
                let e = ((out.pixel(x, y)[1] - t) / t) as f64;
                signed += e;
                n += 1;
                if frames[1].clipped(x, y) {
                    hi_err += e.abs();
                    hi_n += 1;
                } else {
                    merged += e.abs();
                    alone += ((frames[1].raw(x, y)[1] - t) / t).abs() as f64;
                    both_n += 1;
                }
            }
        }
        let (bias, hi, m, a) = (
            signed / n as f64,
            hi_err / hi_n as f64,
            merged / both_n as f64,
            alone / both_n as f64,
        );
        eprintln!(
            "bias {bias:.5}; clipped highlights {hi:.4} over {hi_n} px; \
             merged {m:.4} vs reference alone {a:.4} over {both_n} px"
        );
        assert!(
            hi_n > 1000,
            "the test must cover highlights the reference clipped"
        );
        assert!(bias.abs() < 0.005, "bias {bias}");
        assert!(hi < 0.02, "the clipped highlights came back at {hi}");
        assert!(m < 0.8 * a, "merging bought nothing: {m} vs {a}");
    }

    /// A smooth ramp through the hand-over from the bright frame to the dark one: no step.
    #[test]
    fn the_hand_over_leaves_no_step() {
        let (w, h) = (400, 4);
        let step = 4.0 / w as f32;
        let frame = |ev: f32| {
            let px: Vec<u16> = (0..w * h)
                .flat_map(|i| {
                    let r = 0.1 * 2f32.powf(step * (i % w) as f32) * 2f32.powf(ev);
                    [r; 3].map(|v| (v * 65535.0).round().clamp(0.0, 65535.0) as u16)
                })
                .collect();
            Frame::new(w, h, px, [1.0; 3], Some(ev))
        };
        let frames = [frame(0.0), frame(-2.0)];
        let out = merge(&frames, 0, &ids(2), &[1.0, 4.0], &[None, None]);
        assert_finite(&out);
        let mut worst = 0f32;
        for x in 3..w - 4 {
            let (a, b) = (out.pixel(x, 1)[1], out.pixel(x + 1, 1)[1]);
            worst = worst.max(((b / a).log2() - step).abs());
        }
        eprintln!("worst step error {worst} (a step is {step})");
        assert!(
            worst < 0.5 * step,
            "a step of {worst} stops at the hand-over"
        );
    }

    #[test]
    fn where_every_frame_clipped_the_darkest_stands() {
        let full = |ev| Frame::new(8, 8, vec![65535; 8 * 8 * 3], [2.0, 1.0, 1.5], Some(ev));
        let out = merge(
            &[full(0.0), full(-2.0)],
            0,
            &ids(2),
            &[1.0, 4.0],
            &[None, None],
        );
        let p = out.pixel(4, 4);
        for (c, m) in [2.0f32, 1.0, 1.5].iter().enumerate() {
            assert!((p[c] - 4.0 / m).abs() < 1e-4, "{p:?}");
        }
    }

    /// A black frame, a white one and a moved one together: nothing non-finite anywhere.
    #[test]
    fn the_merge_is_finite_everywhere() {
        let motion = [Homography::identity(), synth::turn(1.0, 9.0, -4.0, 96, 64)];
        let (mut frames, _) = synth::bracket(96, 64, &[0.0, -2.0], &motion, 10, None);
        frames.push(Frame::new(
            96,
            64,
            vec![0; 96 * 64 * 3],
            synth::PRE_MUL,
            None,
        ));
        frames.push(Frame::new(
            96,
            64,
            vec![65535; 96 * 64 * 3],
            synth::PRE_MUL,
            None,
        ));
        let to = [
            motion[0],
            motion[1],
            Homography::identity(),
            Homography::identity(),
        ];
        let out = merge(
            &frames,
            0,
            &to,
            &[1.0, 4.0, 1.0, 1.0],
            &[None, None, None, None],
        );
        assert_finite(&out);
    }

    #[test]
    fn a_one_frame_merge_is_the_frame() {
        let (frames, _) = synth::bracket(40, 30, &[0.0], &ids(1), 11, None);
        let out = merge(&frames, 0, &ids(1), &[1.0], &[None]);
        // ⚠ Within 1e-6, not exactly: a sample under the roll-off is `(w·raw) / w`, which rounds.
        for y in 0..30 {
            for x in 0..40 {
                let (o, f) = (out.pixel(x, y), frames[0].raw(x, y));
                for c in 0..3 {
                    assert!(
                        (o[c] - f[c]).abs() <= 1e-6 * f[c].abs().max(1e-3),
                        "({x}, {y}) {o:?} {f:?}"
                    );
                }
            }
        }
    }
}
