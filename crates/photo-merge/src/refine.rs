//! Dense refinement (M10.17b): the feature rig is good to a few tenths of a pixel; HDR's Gauss–Newton
//! registration (`align`), started from each neighbouring pair's rotation, takes it to hundredths. Each
//! refined pair hands back a grid of exact correspondences for the global fit to solve again.
//!
//! ⚠ The pyramids must be of LENS-CORRECTED frames (`lens::corrected`): two frames are related by a
//! homography only once their distortion is gone — and their VIGNETTING, since the registration models one
//! constant offset between two frames' log luminance: on the synthetic row an uncorrected 25 % falloff left
//! the refined rig at 0.56 px, worse than the features' 0.24; corrected, 0.006 px.

use crate::align::{self, Alignment};
use crate::homography::Homography;
use crate::pyramid::LumPyramid;
use crate::rig::Rig;
use crate::rotation::Corr;

/// The grid each refined pair is sampled on, over the first frame of the pair.
const GRID: (usize, usize) = (16, 12);
/// A refinement that moves the pair's mapping further than this from the rig's (full-size pixels) has
/// wandered, and is not used.
const WANDER_PX: f64 = 3.0;

/// The pair's homography from the rig: frame `a`'s pixels → frame `b`'s, `K Rb Raᵀ K⁻¹`.
pub fn pair_homography(rig: &Rig, a: usize, b: usize) -> Homography {
    let f = rig.focal_px;
    let (cx, cy) = (
        (rig.size.0 as f64 - 1.0) / 2.0,
        (rig.size.1 as f64 - 1.0) / 2.0,
    );
    let k = Homography([f, 0.0, cx, 0.0, f, cy, 0.0, 0.0, 1.0]);
    let k_inv = Homography([1.0 / f, 0.0, -cx / f, 0.0, 1.0 / f, -cy / f, 0.0, 0.0, 1.0]);
    let r = rig.rotations[b].compose(&rig.rotations[a].transpose());
    k.compose(&Homography(r.0)).compose(&k_inv)
}

/// Every neighbouring pair of `rig.order` registered densely from its rig homography down to pyramid level
/// `finest`, and sampled into correspondences; a pair that fails or wanders gives none.
pub fn refine(rig: &Rig, pyramids: &[LumPyramid], finest: usize) -> Vec<Corr> {
    use rayon::prelude::*;
    let pairs: Vec<(usize, usize)> = rig.order.windows(2).map(|w| (w[0], w[1])).collect();
    pairs
        .par_iter()
        .flat_map_iter(|&(a, b)| {
            let h0 = pair_homography(rig, a, b);
            let init = Alignment {
                h: h0,
                log_gain: 0.0,
                overlap: 1.0,
                residual: f32::INFINITY,
            };
            let al = align::refine_from(&pyramids[a], &pyramids[b], &init, finest);
            let (w, h) = (rig.size.0 as f64, rig.size.1 as f64);
            let inside =
                |p: (f64, f64)| (4.0..w - 5.0).contains(&p.0) && (4.0..h - 5.0).contains(&p.1);
            let mut out = Vec::new();
            let mut wandered = !al.residual.is_finite();
            for j in 0..GRID.1 {
                for i in 0..GRID.0 {
                    let p = (
                        (i as f64 + 0.5) / GRID.0 as f64 * w,
                        (j as f64 + 0.5) / GRID.1 as f64 * h,
                    );
                    let q0 = h0.apply(p.0, p.1);
                    if !inside(q0) {
                        continue;
                    }
                    let q = al.h.apply(p.0, p.1);
                    wandered |= (q.0 - q0.0).hypot(q.1 - q0.1) > WANDER_PX;
                    out.push(Corr {
                        a,
                        b,
                        pa: [p.0, p.1],
                        pb: [q.0, q.1],
                    });
                }
            }
            if wandered {
                out.clear();
            }
            out
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::align::COARSEST;
    use crate::rig::tests::{rig_of, worst_px};
    use crate::rig::{level, solve};
    use crate::synth_pano::{self, Scene};

    #[test]
    fn the_pair_homography_maps_as_the_rig_does() {
        let r = synth_pano::row(3, 0.33, false);
        let f = synth_pano::render(&Scene::new(1), &r.shots, r.w, r.h, r.focal);
        let rig = rig_of(&f, r.focal, (r.w, r.h));
        let h = pair_homography(&rig, 0, 1);
        for p in [(400.0, 100.0), (450.0, 300.0)] {
            let (u, v) = h.apply(p.0, p.1);
            let q = rig.pixel(1, rig.ray(0, p.0, p.1)).unwrap();
            assert!((u - q.0).abs() < 1e-6 && (v - q.1).abs() < 1e-6);
        }
    }

    #[test]
    fn refinement_reaches_five_hundredths_of_a_pixel() {
        let r = synth_pano::row(6, 0.33, false);
        let f = synth_pano::render(&Scene::new(1), &r.shots, r.w, r.h, r.focal);
        let rig = rig_of(&f, r.focal, (r.w, r.h));
        let idx: Vec<usize> = (0..6).collect();
        let before = worst_px(&rig, &r, &idx);
        // As the merge does: the pyramids of lens-corrected frames (here the synthetic vignette undone).
        let lens = crate::gain::tests::vignette_lens(&r);
        let pyr: Vec<LumPyramid> = f
            .iter()
            .map(|x| LumPyramid::build(&crate::lens::corrected(x, &lens), COARSEST))
            .collect();
        let corrs = refine(&rig, &pyr, 0);
        assert!(
            corrs.len() >= 5 * 30,
            "{} corrs over five pairs",
            corrs.len()
        );
        let mut fine = solve(6, rig.focal_px, rig.size, &corrs, rig.order.clone());
        level(&mut fine);
        let after = worst_px(&fine, &r, &idx);
        eprintln!(
            "features {before:.3} px → refined {after:.4} px; focal {:.2} vs {}",
            fine.focal_px, r.focal
        );
        assert!(after < 0.05, "{after} px");
    }
}
