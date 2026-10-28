//! A frame laid onto the panorama's canvas (M10.17b): canvas pixel → its world direction → the frame's
//! pixel → the lens-corrected read — ONE resample, Catmull-Rom, with the frame's exposure gain applied.

use crate::frame::Frame;
use crate::lens::FrameLens;
use crate::projection::Canvas;
use crate::rig::Rig;
use crate::warp::Sampler;

/// One frame on the canvas, within its bounding box.
#[derive(Clone, Debug)]
pub struct Warped {
    pub x0: usize,
    pub y0: usize,
    pub width: usize,
    pub height: usize,
    /// Normalised raw camera RGB, gain applied; 0 where not valid.
    pub rgb: Vec<f32>,
    pub valid: Vec<bool>,
    /// The read touched a clipped sample.
    pub clipped: Vec<bool>,
}

impl Warped {
    /// The local index of canvas pixel `(u, v)`, when it is inside the box.
    #[inline]
    pub fn index(&self, u: usize, v: usize) -> Option<usize> {
        (u >= self.x0 && v >= self.y0 && u < self.x0 + self.width && v < self.y0 + self.height)
            .then(|| (v - self.y0) * self.width + (u - self.x0))
    }

    /// Canvas pixel `(u, v)`'s value, when the frame covers it.
    #[inline]
    pub fn at(&self, u: usize, v: usize) -> Option<[f32; 3]> {
        let i = self.index(u, v).filter(|&i| self.valid[i])?;
        Some([self.rgb[3 * i], self.rgb[3 * i + 1], self.rgb[3 * i + 2]])
    }
}

/// Frame `i` of `rig` onto `canvas`, read through `lens`, times `gain`.
pub fn warp(
    canvas: &Canvas,
    rig: &Rig,
    i: usize,
    f: &Frame,
    lens: &FrameLens,
    gain: f32,
) -> Warped {
    use rayon::prelude::*;
    let (fw, fh) = (rig.size.0 as f64, rig.size.1 as f64);
    // The box: the frame's outline on the canvas, a pixel of margin for the kernel.
    let (mut x, mut y) = ((f64::MAX, f64::MIN), (f64::MAX, f64::MIN));
    let n = 64;
    for k in 0..=n {
        let t = k as f64 / n as f64;
        for (px, py) in [
            (t * (fw - 1.0), 0.0),
            (t * (fw - 1.0), fh - 1.0),
            (0.0, t * (fh - 1.0)),
            (fw - 1.0, t * (fh - 1.0)),
        ] {
            if let Some((u, v)) = canvas.point(rig.ray(i, px, py)) {
                x = (x.0.min(u), x.1.max(u));
                y = (y.0.min(v), y.1.max(v));
            }
        }
    }
    let clamp = |v: f64, hi: usize| (v.max(0.0) as usize).min(hi);
    let (x0, x1) = (
        clamp(x.0.floor() - 1.0, canvas.width),
        clamp(x.1.ceil() + 2.0, canvas.width),
    );
    let (y0, y1) = (
        clamp(y.0.floor() - 1.0, canvas.height),
        clamp(y.1.ceil() + 2.0, canvas.height),
    );
    let (width, height) = (x1.saturating_sub(x0), y1.saturating_sub(y0));
    let s = Sampler::new();
    let rows: Vec<(Vec<f32>, Vec<bool>, Vec<bool>)> = (0..height)
        .into_par_iter()
        .map(|j| {
            let (mut rgb, mut valid, mut clipped) = (
                vec![0f32; width * 3],
                vec![false; width],
                vec![false; width],
            );
            for k in 0..width {
                let Some(d) = canvas.ray((x0 + k) as f64, (y0 + j) as f64) else {
                    continue;
                };
                let Some((px, py)) = rig.pixel(i, d) else {
                    continue;
                };
                if let Some((v, c)) = s.raw_at_corrected(f, lens, px as f32, py as f32) {
                    for ch in 0..3 {
                        rgb[3 * k + ch] = v[ch] * gain;
                    }
                    (valid[k], clipped[k]) = (true, c);
                }
            }
            (rgb, valid, clipped)
        })
        .collect();
    let mut w = Warped {
        x0,
        y0,
        width,
        height,
        rgb: Vec::with_capacity(width * height * 3),
        valid: Vec::with_capacity(width * height),
        clipped: Vec::with_capacity(width * height),
    };
    for (rgb, valid, clipped) in rows {
        w.rgb.extend(rgb);
        w.valid.extend(valid);
        w.clipped.extend(clipped);
    }
    w
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::projection::{Projection, MAX_LONG_EDGE};
    use crate::rotation::Rot;
    use crate::synth_pano::{self, Row, Scene};

    /// A synthetic row ready to stitch: frames, the fitted rig, the vignette-undoing lens, measured gains,
    /// the canvas and every frame warped onto it.
    pub(crate) struct Stage {
        pub r: Row,
        pub scene: Scene,
        pub rig: Rig,
        pub canvas: Canvas,
        pub warped: Vec<Warped>,
    }

    pub(crate) fn stage(r: Row, scene: Scene, gains: bool) -> Stage {
        let frames = synth_pano::render(&scene, &r.shots, r.w, r.h, r.focal);
        let lens: Vec<FrameLens> = frames
            .iter()
            .map(|_| crate::gain::tests::vignette_lens(&r))
            .collect();
        // As the merge does: the feature rig, refined densely on lens-corrected frames, solved again.
        let coarse = crate::rig::tests::rig_of(&frames, r.focal, (r.w, r.h));
        let pyr: Vec<_> = frames
            .iter()
            .zip(&lens)
            .map(|(f, l)| {
                let c = crate::lens::corrected(f, l);
                crate::pyramid::LumPyramid::build(&c, crate::align::COARSEST)
            })
            .collect();
        let corrs = crate::refine::refine(&coarse, &pyr, 0);
        let n = frames.len();
        let mut rig = crate::rig::solve(
            n,
            coarse.focal_px,
            coarse.size,
            &corrs,
            coarse.order.clone(),
        );
        crate::rig::level(&mut rig);
        let g = if gains {
            crate::gain::solve(&rig, &frames, &lens)
        } else {
            vec![1.0; frames.len()]
        };
        let canvas = Canvas::plan(&rig, Projection::Cylindrical, MAX_LONG_EDGE).unwrap();
        let warped = (0..frames.len())
            .map(|i| warp(&canvas, &rig, i, &frames[i], &lens[i], g[i]))
            .collect();
        Stage {
            r,
            scene,
            rig,
            canvas,
            warped,
        }
    }

    impl Stage {
        /// The truth at canvas pixel `(u, v)`: the scene's radiance there, in the reference frame's exposure.
        pub(crate) fn truth(&self, u: usize, v: usize) -> Option<[f32; 3]> {
            let d = self.canvas.ray(u as f64, v as f64)?;
            let m = self.r.shots[self.rig.reference].rotation();
            let truth = Rot([
                m[0][0], m[1][0], m[2][0], m[0][1], m[1][1], m[2][1], m[0][2], m[1][2], m[2][2],
            ]);
            let to_rig = self.rig.rotations[self.rig.reference]
                .transpose()
                .compose(&truth);
            let g = self.r.shots[self.rig.reference].gain;
            let world = to_rig.transpose().apply(d);
            // Off the scene's hard lines, which no sampled frame holds (three pixels at 420 px a radian).
            if Scene::near_a_line(world, 0.007) {
                return None;
            }
            Some(self.scene.radiance(world).map(|c| c * g))
        }
    }

    #[test]
    fn a_warped_frame_holds_the_scene() {
        let mut r = synth_pano::row(3, 0.33, false);
        r.shots.iter_mut().for_each(|s| s.gain = 1.0);
        let s = stage(r, Scene::new(1), false);
        let w = &s.warped[1];
        let (mut err, mut n) = (0f64, 0usize);
        for v in (w.y0..w.y0 + w.height).step_by(7) {
            for u in (w.x0..w.x0 + w.width).step_by(7) {
                let (Some(got), Some(t)) = (w.at(u, v), s.truth(u, v)) else {
                    continue;
                };
                if t[1] > 0.01 && t[1] < 0.6 {
                    err += ((got[1] / t[1]).log2().abs()) as f64;
                    n += 1;
                }
            }
        }
        assert!(n > 1000, "{n} samples");
        let mean = err / n as f64;
        // The frame's own pixels against the truth (no warp) measure 0.016 stops — its shot noise.
        eprintln!("warped against the truth: {mean:.4} stops");
        assert!(mean < 0.03, "mean error {mean} stops");
    }

    /// Wiring: a canvas pixel's value is exactly the corrected read at the rig's pixel, times the gain —
    /// one resample, through the lens.
    #[test]
    fn the_warp_reads_through_the_lens_once() {
        let r = synth_pano::row(2, 0.33, false);
        let frames = synth_pano::render(&Scene::new(1), &r.shots, r.w, r.h, r.focal);
        let rig = crate::rig::tests::rig_of(&frames, r.focal, (r.w, r.h));
        let canvas = Canvas::plan(&rig, Projection::Cylindrical, MAX_LONG_EDGE).unwrap();
        let lens = crate::lens::tests::xf23(r.w, r.h);
        let w = warp(&canvas, &rig, 0, &frames[0], &lens, 1.5);
        let s = Sampler::new();
        let (mut checked, mut moved) = (0, 0);
        for v in (w.y0..w.y0 + w.height).step_by(13) {
            for u in (w.x0..w.x0 + w.width).step_by(13) {
                let Some(got) = w.at(u, v) else { continue };
                let (x, y) = rig
                    .pixel(0, canvas.ray(u as f64, v as f64).unwrap())
                    .unwrap();
                let (want, _) = s
                    .raw_at_corrected(&frames[0], &lens, x as f32, y as f32)
                    .unwrap();
                for c in 0..3 {
                    assert!((got[c] - want[c] * 1.5).abs() < 1e-6);
                }
                let plain = s.raw_at(&frames[0], x as f32, y as f32);
                moved += plain.is_some_and(|p| (p[1] * 1.5 - got[1]).abs() > 1e-4) as usize;
                checked += 1;
            }
        }
        assert!(
            checked > 200 && moved > checked / 4,
            "{moved} of {checked} reads moved by the lens"
        );
    }
}
