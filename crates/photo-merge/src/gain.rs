//! The exposure solve (M10.17b): one factor per frame that brings its normalised raw signal to the
//! reference's — **measured** over the overlaps, never taken from EXIF (M10.17a's rule). Each neighbouring
//! pair's log-luminance difference is the median over mid-tone pixels both frames hold unclipped (a
//! median, so something moving in the overlap does not drag it); the chain's differences, summed from the
//! reference, are the gains.
//!
//! The raw signal is already free of white balance, so one factor per frame suffices; a per-channel gain is
//! left for a measurement to ask for (spec).

use crate::frame::Frame;
use crate::lens::FrameLens;
use crate::rig::Rig;
use crate::warp::Sampler;

/// The mid-tones compared: above ~8 stops under white, below the highlight roll-off.
const DARKEST: f32 = 1.0 / 256.0;
const BRIGHTEST: f32 = 0.7;
/// The overlap is sampled every this many pixels of the first frame.
const STEP: usize = 4;

/// Each frame's factor to the reference (the reference's is 1).
pub fn solve(rig: &Rig, frames: &[Frame], lens: &[FrameLens]) -> Vec<f32> {
    let s = Sampler::new();
    let read = |i: usize, x: f32, y: f32| -> Option<f32> {
        let (v, clipped) = s.raw_at_corrected(&frames[i], &lens[i], x, y)?;
        let l = 0.25 * v[0] + 0.5 * v[1] + 0.25 * v[2];
        (!clipped && (DARKEST..BRIGHTEST).contains(&l)).then_some(l)
    };
    let mut log_gain = vec![0f64; frames.len()];
    for w in rig.order.windows(2) {
        let (a, b) = (w[0], w[1]);
        let mut d: Vec<f32> = Vec::new();
        for y in (0..rig.size.1).step_by(STEP) {
            for x in (0..rig.size.0).step_by(STEP) {
                let Some((u, v)) = rig.pixel(b, rig.ray(a, x as f64, y as f64)) else {
                    continue;
                };
                if let (Some(la), Some(lb)) =
                    (read(a, x as f32, y as f32), read(b, u as f32, v as f32))
                {
                    d.push(la.log2() - lb.log2());
                }
            }
        }
        let m = if d.is_empty() {
            0.0
        } else {
            let mid = d.len() / 2;
            *d.select_nth_unstable_by(mid, f32::total_cmp).1 as f64
        };
        // la · 2^ga = lb · 2^gb.
        log_gain[b] = log_gain[a] + m;
    }
    let base = log_gain[rig.reference];
    log_gain
        .iter()
        .map(|g| 2f64.powf(g - base) as f32)
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::rig::tests::rig_of;
    use crate::synth_pano::{self, Row, Scene};

    fn truth(r: &Row, rig: &Rig) -> Vec<f32> {
        let g_ref = r.shots[rig.reference].gain;
        r.shots.iter().map(|s| g_ref / s.gain).collect()
    }

    fn check(r: &Row, lens: impl Fn() -> FrameLens, what: &str) {
        let f = synth_pano::render(&Scene::new(1), &r.shots, r.w, r.h, r.focal);
        let rig = rig_of(&f, r.focal, (r.w, r.h));
        let lenses: Vec<FrameLens> = f.iter().map(|_| lens()).collect();
        let got = solve(&rig, &f, &lenses);
        for (i, (g, t)) in got.iter().zip(truth(r, &rig)).enumerate() {
            assert!((g / t - 1.0).abs() < 0.005, "{what}: frame {i}: {g} vs {t}");
        }
        assert_eq!(got[rig.reference], 1.0);
    }

    #[test]
    fn gains_are_measured_within_half_a_percent() {
        let mut r = synth_pano::row(5, 0.33, false);
        r.shots.iter_mut().for_each(|s| s.vignette = 0.0);
        check(&r, || FrameLens::NoLens, "flat frames");
    }

    #[test]
    fn gains_ignore_clipped_sky() {
        let mut r = synth_pano::row(4, 0.33, false);
        r.shots.iter_mut().for_each(|s| s.vignette = 0.0);
        // A stop and a half over, looking a little up: its sky clips, and enough ground stays to match on.
        r.shots[3].gain = 2.8;
        r.shots[3].pitch = 4f64.to_radians();
        let f = synth_pano::render(&Scene::new(1), &r.shots[3..], r.w, r.h, r.focal);
        let clipped = (0..r.h)
            .flat_map(|y| (0..r.w).map(move |x| (x, y)))
            .filter(|&(x, y)| f[0].clipped(x, y))
            .count();
        assert!(
            clipped > r.w * r.h / 20,
            "the bright frame must clip its sky ({clipped} px)"
        );
        check(&r, || FrameLens::NoLens, "a clipped sky");
    }

    /// The synthetic vignette (`1 − v·r²`) undone by a lens whose gain is its inverse — the overlaps sit
    /// in both frames' darkened edges, so an uncorrected vignette would bias every ratio.
    /// A lens that undoes `synth_pano`'s vignette (`1 − v·r²`) and moves nothing.
    pub(crate) fn vignette_lens(r: &Row) -> FrameLens {
        let (w, h, v) = (r.w, r.h, r.shots[0].vignette);
        FrameLens::from_fn(w, h, |x, y| {
            let half = ((w * w + h * h) as f32).sqrt() / 2.0;
            let rr = ((x - (w as f32 - 1.0) / 2.0).powi(2) + (y - (h as f32 - 1.0) / 2.0).powi(2))
                .sqrt()
                / half;
            ([[x, y]; 3], 1.0 / (1.0 - v * rr * rr))
        })
    }

    #[test]
    fn vignetting_does_not_bias_the_gains() {
        let r = synth_pano::row(4, 0.33, false);
        check(&r, || vignette_lens(&r), "a vignetted row");
    }
}
