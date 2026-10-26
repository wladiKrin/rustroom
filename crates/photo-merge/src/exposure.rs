//! Each frame's exposure relative to the reference, **measured from the pixels** — never read from the
//! file alone: nominal shutter speeds are rounded (1/125 is really 1/128) and libraw's
//! `adjust_maximum` moves a clipped frame's white by a few per cent (15361 of 16383 on DSCF7618).

use crate::align::Alignment;
use crate::frame::Frame;
use crate::warp::Sampler;

/// The sample grid's long edge.
const GRID: usize = 1024;
/// Fewer comparable samples than this and the alignment's offset stands.
const MIN_SAMPLES: usize = 1000;
/// Both frames must show a sample in this band (white-balanced, of white) for it to count: above
/// the noise, below the highlight roll-off.
const BAND: (f32, f32) = (0.02, 0.8);

/// `reference / frame` for the light both hold: the median of their raw signals' ratio over a
/// ≤1024-px grid of the reference, where both are inside [`BAND`]. Falls back to `2^-log_gain` with
/// fewer than 1,000 samples.
pub fn measure(reference: &Frame, frame: &Frame, a: &Alignment) -> f32 {
    let fallback = 2f32.powf(-a.log_gain);
    let step = (reference.width.max(reference.height) / GRID).max(1);
    let sampler = Sampler::new();
    let shown = |raw: [f32; 3], pre: [f32; 3]| (0..3).map(|c| raw[c] * pre[c]).fold(0f32, f32::max);
    let lum = |raw: [f32; 3]| 0.25 * raw[0] + 0.5 * raw[1] + 0.25 * raw[2];
    let mut ratios = Vec::new();
    for y in (0..reference.height).step_by(step) {
        for x in (0..reference.width).step_by(step) {
            if reference.clipped(x, y) {
                continue;
            }
            let r = reference.raw(x, y);
            if !(BAND.0..=BAND.1).contains(&shown(r, reference.pre_mul)) {
                continue;
            }
            let (u, v) = a.h.apply(x as f64, y as f64);
            let Some(f) = sampler.raw_at(frame, u as f32, v as f32) else {
                continue;
            };
            if !(BAND.0..=BAND.1).contains(&shown(f, frame.pre_mul)) {
                continue;
            }
            let (lr, lf) = (lum(r), lum(f));
            if lf > 0.0 {
                ratios.push(lr / lf);
            }
        }
    }
    if ratios.len() < MIN_SAMPLES {
        return fallback;
    }
    let mid = ratios.len() / 2;
    let m = *ratios.select_nth_unstable_by(mid, |a, b| a.total_cmp(b)).1;
    if m.is_finite() && m > 0.0 {
        m
    } else {
        fallback
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::align::Alignment;
    use crate::homography::Homography;
    use crate::synth;

    /// ⚠ The file says two stops; the light says 1.94. Nominal shutter speeds are rounded (1/125 is
    /// 1/128) and libraw moves a clipped frame's white — the measurement wins.
    #[test]
    fn the_scale_is_measured_not_read() {
        let id = [Homography::identity(); 2];
        let (mut frames, _) = synth::bracket(320, 240, &[-1.94, 0.0], &id, 7, None);
        frames[0].ev = Some(-2.0);
        let a = Alignment {
            h: Homography::identity(),
            log_gain: -2.0,
            overlap: 1.0,
            residual: 0.0,
        };
        let scale = measure(&frames[1], &frames[0], &a);
        let want = 2f32.powf(1.94);
        eprintln!("measured {scale}, true {want}");
        assert!((scale / want - 1.0).abs() < 0.005, "{scale} vs {want}");
    }

    /// With nothing comparable, the alignment's offset stands.
    #[test]
    fn without_samples_the_alignment_offset_stands() {
        let (frames, _) = synth::bracket(64, 48, &[0.0], &[Homography::identity()], 8, None);
        let black = crate::frame::Frame::new(64, 48, vec![0; 64 * 48 * 3], synth::PRE_MUL, None);
        let a = Alignment {
            h: Homography::identity(),
            log_gain: -1.5,
            overlap: 0.0,
            residual: 0.0,
        };
        assert!((measure(&frames[0], &black, &a) - 2f32.powf(1.5)).abs() < 1e-5);
    }
}
