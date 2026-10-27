//! The panorama's rig (M10.17b): every frame's rotation and the one focal length they share, fitted
//! together over all the pairs' correspondences, then levelled so the horizon is straight.
//!
//! **Levelling** ("wave correction"): a handheld row is swept by turning about the vertical, so every
//! frame's x axis lies (nearly) in the horizontal plane; the world's up is the direction most
//! perpendicular to all of them — the smallest eigenvector of `Σ xᵢ xᵢᵀ`. A sweep that wanders up and
//! down keeps its x axes level, so it is levelled exactly; a frame held ROLLED tilts its own x axis and
//! pulls the fit by a share of its roll that shrinks with the number of frames
//! (`a_rolled_reference_tilts_the_horizon_less`). A vertical sweep turns about a horizontal axis, which
//! every frame's y axis is perpendicular to: there it is the axis that is levelled.

use crate::rotation::{to_pixel, unit_ray, Corr, Rot};

/// Which way the row runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Sweep {
    Horizontal,
    Vertical,
}

/// Every frame's rotation (world → camera), the shared focal length, and the row's order.
#[derive(Clone, Debug, PartialEq)]
pub struct Rig {
    /// Focal length in pixels at `size`.
    pub focal_px: f64,
    /// Every frame's size.
    pub size: (usize, usize),
    pub rotations: Vec<Rot>,
    /// The chain, left → right (or top → bottom).
    pub order: Vec<usize>,
    /// The middle of the chain: its white balance, exposure and centre are the panorama's.
    pub reference: usize,
    pub sweep: Sweep,
}

impl Rig {
    /// The world direction of pixel `(x, y)` of `frame` (unit length).
    pub fn ray(&self, frame: usize, x: f64, y: f64) -> [f64; 3] {
        self.rotations[frame]
            .transpose()
            .apply(unit_ray([x, y], self.focal_px, self.size))
    }

    /// Where world direction `ray` lands in `frame` — `None` behind the camera; may lie outside it.
    pub fn pixel(&self, frame: usize, ray: [f64; 3]) -> Option<(f64, f64)> {
        to_pixel(self.rotations[frame].apply(ray), self.focal_px, self.size).map(|p| (p[0], p[1]))
    }

    /// The rig for frames `s` times the size — the preview's half decodes to the merge's full ones.
    pub fn rescaled(&self, s: f64) -> Rig {
        Rig {
            focal_px: self.focal_px * s,
            size: (
                (self.size.0 as f64 * s).round() as usize,
                (self.size.1 as f64 * s).round() as usize,
            ),
            ..self.clone()
        }
    }
}

/// Focal length in pixels for a frame whose long edge is `long_px`, from its EXIF focal length and the
/// camera's crop factor (LensFun's, else 1.5 for a maker known only for APS-C); `None` without EXIF.
pub fn focal_guess(meta: &raw_decode::RawMetadata, long_px: usize) -> Option<f64> {
    let lens = meta.lens.as_ref().filter(|l| l.focal > 0.0)?;
    let crop = lens_profile::Catalogue::shared()
        .and_then(|db| db.find(&meta.make, &meta.model, &lens.name))
        .map(|p| p.crop_factor() as f64)
        .or_else(|| {
            APS_C_MAKERS
                .iter()
                .any(|m| meta.make.eq_ignore_ascii_case(m))
                .then_some(1.53)
        })?;
    // The long edge of a 3:2 sensor is 36 mm / crop.
    Some(lens.focal as f64 * crop / 36.0 * long_px as f64)
}

/// Makers whose interchangeable-lens bodies are all APS-C — the crop factor when LensFun has no match.
const APS_C_MAKERS: &[&str] = &["FUJIFILM"];

/// Every rotation and the focal length, fitted to `corrs` (Levenberg–Marquardt, Cauchy loss, pixels).
/// `order` is [`crate::rotation::chain`]'s; the start comes from walking it pair by pair. The result's
/// order runs left → right (top → bottom); its reference is the chain's middle.
pub fn solve(
    n: usize,
    focal_px: f64,
    size: (usize, usize),
    corrs: &[Corr],
    order: Vec<usize>,
) -> Rig {
    let mut order = order;
    // A start: walk the chain, each frame's rotation from its predecessor's by Horn on their corrs.
    let mut rot = vec![Rot::identity(); n];
    for w in order.windows(2) {
        let (a, b) = (w[0], w[1]);
        let (mut from, mut to) = (Vec::new(), Vec::new());
        for c in corrs {
            if (c.a, c.b) == (a, b) {
                from.push(unit_ray(c.pa, focal_px, size));
                to.push(unit_ray(c.pb, focal_px, size));
            } else if (c.a, c.b) == (b, a) {
                from.push(unit_ray(c.pb, focal_px, size));
                to.push(unit_ray(c.pa, focal_px, size));
            }
        }
        let r_ab = crate::rotation::horn(&from, &to).unwrap_or(Rot::identity());
        rot[b] = r_ab.compose(&rot[a]);
    }
    let (sweep, forward) = sweep_of(&rot, &order);
    if !forward {
        order.reverse();
    }
    let reference = order[(order.len().max(1) - 1) / 2];
    let base = rot[reference].transpose();
    for r in rot.iter_mut() {
        *r = r.compose(&base);
    }
    let (rotations, focal_px) = refine(rot, focal_px, size, corrs, reference);
    Rig {
        focal_px,
        size,
        rotations,
        order,
        reference,
        sweep,
    }
}

/// Which way the chain runs, and whether `order` already runs left → right (top → bottom): the signed turns
/// between neighbouring optical axes, summed along the chain — never the ends' axes compared, which a sweep
/// past 180° wraps (a six-frame row is ~200°). Turns are measured about the first frame's own axes.
fn sweep_of(rot: &[Rot], order: &[usize]) -> (Sweep, bool) {
    let (mut yaw, mut pitch, mut yaw_abs, mut pitch_abs) = (0f64, 0f64, 0f64, 0f64);
    for w in order.windows(2) {
        let (a, b) = (rot[w[0]].row(2), rot[w[1]].row(2));
        let c = crate::rotation::cross(a, b);
        let dot = a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        // About +y (down) a turn to the right is positive; about −x a turn downward is.
        let (dy, dp) = (c[1].atan2(dot), (-c[0]).atan2(dot));
        (yaw, pitch, yaw_abs, pitch_abs) = (
            yaw + dy,
            pitch + dp,
            yaw_abs + dy.abs(),
            pitch_abs + dp.abs(),
        );
    }
    if yaw_abs >= pitch_abs {
        (Sweep::Horizontal, yaw >= 0.0)
    } else {
        (Sweep::Vertical, pitch >= 0.0)
    }
}

/// The Cauchy loss's scale, pixels.
const LOSS_PX: f64 = 1.5;
const LM_ITERATIONS: usize = 60;

/// Both directions of every correspondence, as pixel residuals.
fn residuals(rot: &[Rot], f: f64, size: (usize, usize), corrs: &[Corr], out: &mut Vec<f64>) {
    out.clear();
    for c in corrs {
        for (from, to, pf, pt) in [(c.a, c.b, c.pa, c.pb), (c.b, c.a, c.pb, c.pa)] {
            let world = rot[from].transpose().apply(unit_ray(pf, f, size));
            match to_pixel(rot[to].apply(world), f, size) {
                Some(q) => out.extend([q[0] - pt[0], q[1] - pt[1]]),
                None => out.extend([1e3, 1e3]),
            }
        }
    }
}

/// Levenberg–Marquardt over every rotation but the reference's (a turn applied on the left) and the log of
/// the focal length, Cauchy-weighted (IRLS), the Jacobian by central differences.
fn refine(
    mut rot: Vec<Rot>,
    mut f: f64,
    size: (usize, usize),
    corrs: &[Corr],
    reference: usize,
) -> (Vec<Rot>, f64) {
    let free: Vec<usize> = (0..rot.len()).filter(|&i| i != reference).collect();
    let np = free.len() * 3 + 1;
    let apply = |rot: &[Rot], f: f64, d: &[f64]| -> (Vec<Rot>, f64) {
        let mut r = rot.to_vec();
        for (k, &i) in free.iter().enumerate() {
            r[i] = Rot::from_axis_angle([d[3 * k], d[3 * k + 1], d[3 * k + 2]]).compose(&rot[i]);
        }
        (r, f * d[np - 1].exp())
    };
    let cost = |res: &[f64]| -> f64 {
        res.chunks(2)
            .map(|p| {
                LOSS_PX * LOSS_PX * (1.0 + (p[0] * p[0] + p[1] * p[1]) / (LOSS_PX * LOSS_PX)).ln()
            })
            .sum()
    };
    let mut res = Vec::new();
    residuals(&rot, f, size, corrs, &mut res);
    let mut current = cost(&res);
    let mut lambda = 1e-3;
    let mut plus = Vec::new();
    let mut minus = Vec::new();
    for _ in 0..LM_ITERATIONS {
        let m = res.len();
        let weights: Vec<f64> = res
            .chunks(2)
            .flat_map(|p| {
                let w = 1.0 / (1.0 + (p[0] * p[0] + p[1] * p[1]) / (LOSS_PX * LOSS_PX));
                [w, w]
            })
            .collect();
        let mut jac = vec![0.0; m * np];
        let eps = 1e-6;
        for p in 0..np {
            let mut d = vec![0.0; np];
            d[p] = eps;
            let (r1, f1) = apply(&rot, f, &d);
            residuals(&r1, f1, size, corrs, &mut plus);
            d[p] = -eps;
            let (r2, f2) = apply(&rot, f, &d);
            residuals(&r2, f2, size, corrs, &mut minus);
            for i in 0..m {
                jac[i * np + p] = (plus[i] - minus[i]) / (2.0 * eps);
            }
        }
        let mut a = vec![0.0; np * np];
        let mut g = vec![0.0; np];
        for i in 0..m {
            let row = &jac[i * np..(i + 1) * np];
            for p in 0..np {
                g[p] += weights[i] * row[p] * res[i];
                for q in 0..np {
                    a[p * np + q] += weights[i] * row[p] * row[q];
                }
            }
        }
        let mut improved = false;
        for _ in 0..10 {
            let mut damped = a.clone();
            for p in 0..np {
                damped[p * np + p] += lambda * a[p * np + p].max(1e-9);
            }
            let Some(step) = solve_linear(&damped, &g.iter().map(|v| -v).collect::<Vec<_>>(), np)
            else {
                lambda *= 10.0;
                continue;
            };
            let (r_new, f_new) = apply(&rot, f, &step);
            residuals(&r_new, f_new, size, corrs, &mut plus);
            let c = cost(&plus);
            if c < current {
                let small = step.iter().all(|v| v.abs() < 1e-10);
                (rot, f, current) = (r_new, f_new, c);
                std::mem::swap(&mut res, &mut plus);
                lambda = (lambda / 3.0).max(1e-9);
                improved = !small;
                break;
            }
            lambda *= 4.0;
        }
        if !improved {
            break;
        }
    }
    (rot, f)
}

/// `a · x = b` by Gaussian elimination with partial pivoting; `None` when singular.
pub(crate) fn solve_linear(a: &[f64], b: &[f64], n: usize) -> Option<Vec<f64>> {
    let mut m: Vec<f64> = Vec::with_capacity(n * (n + 1));
    for i in 0..n {
        m.extend_from_slice(&a[i * n..(i + 1) * n]);
        m.push(b[i]);
    }
    let w = n + 1;
    for col in 0..n {
        let piv =
            (col..n).max_by(|&i, &j| m[i * w + col].abs().total_cmp(&m[j * w + col].abs()))?;
        if m[piv * w + col].abs() < 1e-300 {
            return None;
        }
        for k in 0..w {
            m.swap(col * w + k, piv * w + k);
        }
        for r in 0..n {
            if r != col {
                let factor = m[r * w + col] / m[col * w + col];
                for k in col..w {
                    m[r * w + k] -= factor * m[col * w + k];
                }
            }
        }
    }
    Some((0..n).map(|i| m[i * w + n] / m[i * w + i]).collect())
}

/// Wave correction: turn the world so its up is the one the frames agree on (see the module).
pub fn level(rig: &mut Rig) {
    let r_ref = rig.rotations[rig.reference];
    // The axis every frame should be perpendicular to, and where it belongs in the world.
    let (axis_row, agree, target) = match rig.sweep {
        Sweep::Horizontal => (0, r_ref.row(1).map(|c| -c), [0.0, -1.0, 0.0]),
        Sweep::Vertical => (1, r_ref.row(0), [1.0, 0.0, 0.0]),
    };
    let mut m = [[0.0f64; 3]; 3];
    for r in &rig.rotations {
        let v = r.row(axis_row);
        for i in 0..3 {
            for j in 0..3 {
                m[i][j] += v[i] * v[j];
            }
        }
    }
    let (vals, vecs) = crate::rotation::jacobi(m);
    let Some(k) = (0..3).min_by(|&i, &j| vals[i].total_cmp(&vals[j])) else {
        return;
    };
    let mut up = [vecs[0][k], vecs[1][k], vecs[2][k]];
    if up[0] * agree[0] + up[1] * agree[1] + up[2] * agree[2] < 0.0 {
        up = up.map(|c| -c);
    }
    let axis = crate::rotation::cross(up, target);
    let s = (axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]).sqrt();
    let c = up[0] * target[0] + up[1] * target[1] + up[2] * target[2];
    if s < 1e-15 {
        return;
    }
    let q = Rot::from_axis_angle(axis.map(|v| v / s * s.atan2(c)));
    let qt = q.transpose();
    for r in rig.rotations.iter_mut() {
        *r = r.compose(&qt);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::features::{detect, matches, Keypoint, FEATURE_LONG_EDGE};
    use crate::frame::Frame;
    use crate::rotation::{chain, fit_pair};
    use crate::synth_pano::{self, Row, Scene, Shot};

    /// Features → every pair → the chain → `solve` → `level`: the rig of a synthetic row's frames.
    pub(crate) fn rig_of(frames: &[Frame], focal_guess: f64, size: (usize, usize)) -> Rig {
        let k: Vec<Vec<Keypoint>> = frames
            .iter()
            .map(|f| detect(f, FEATURE_LONG_EDGE))
            .collect();
        let mut corrs = Vec::new();
        let mut pairs = Vec::new();
        for i in 0..frames.len() {
            for j in i + 1..frames.len() {
                if let Some(fit) = fit_pair(
                    (i, j),
                    &k[i],
                    &k[j],
                    &matches(&k[i], &k[j]),
                    focal_guess,
                    size,
                ) {
                    pairs.push((i, j, fit.inliers.len()));
                    corrs.extend(fit.inliers);
                }
            }
        }
        let order = chain(frames.len(), &pairs).expect("a row");
        let mut rig = solve(frames.len(), focal_guess, size, &corrs, order);
        level(&mut rig);
        rig
    }

    /// The worst disagreement, in pixels, between the rig and the truth over every neighbouring pair:
    /// a grid of `a`'s pixels that `b` sees, mapped by each.
    pub(crate) fn worst_px(rig: &Rig, r: &Row, idx: &[usize]) -> f64 {
        let mut worst = 0f64;
        for (ia, a) in idx.iter().enumerate() {
            for (ib, b) in idx.iter().enumerate() {
                if ia == ib {
                    continue;
                }
                for y in (0..r.h).step_by(20) {
                    for x in (0..r.w).step_by(20) {
                        let p = [x as f64, y as f64];
                        let Some(t) =
                            synth_pano::map(&r.shots[*a], &r.shots[*b], r.focal, r.w, r.h, p)
                        else {
                            continue;
                        };
                        let g = rig.pixel(ib, rig.ray(ia, p[0], p[1])).expect("in front");
                        worst = worst.max((g.0 - t[0]).hypot(g.1 - t[1]));
                    }
                }
            }
        }
        worst
    }

    fn row_frames(r: &Row) -> Vec<Frame> {
        synth_pano::render(&Scene::new(1), &r.shots, r.w, r.h, r.focal)
    }

    /// The angle between the rig's up and the truth's, degrees — the horizon's tilt.
    pub(crate) fn tilt_deg(rig: &Rig, shots: &[Shot], idx: &[usize]) -> f64 {
        // Truth: world → camera of the rig's reference; the rig's world up mapped into the truth's.
        let shot = &shots[idx[rig.reference]];
        let m = shot.rotation();
        let truth = Rot([
            m[0][0], m[1][0], m[2][0], m[0][1], m[1][1], m[2][1], m[0][2], m[1][2], m[2][2],
        ]);
        let rig_up_in_cam = rig.rotations[rig.reference].apply([0.0, -1.0, 0.0]);
        let up = truth.transpose().apply(rig_up_in_cam);
        (-up[1]).clamp(-1.0, 1.0).acos().to_degrees()
    }

    #[test]
    fn three_and_six_frames_recover_their_rotations() {
        for n in [3, 6] {
            let r = synth_pano::row(n, 0.33, false);
            let rig = rig_of(&row_frames(&r), r.focal, (r.w, r.h));
            let idx: Vec<usize> = (0..n).collect();
            let px = worst_px(&rig, &r, &idx);
            let ferr = (rig.focal_px - r.focal).abs() / r.focal;
            eprintln!(
                "{n} frames: worst {px:.3} px, focal off {:.3} %",
                ferr * 100.0
            );
            assert!(px < 0.5, "{n} frames: {px} px");
            assert!(ferr < 0.01, "{n} frames: focal {}", rig.focal_px);
            assert_eq!(rig.order, idx, "left to right");
            assert_eq!(rig.reference, (n - 1) / 2);
            assert_eq!(rig.sweep, Sweep::Horizontal);
        }
    }

    #[test]
    fn shuffled_frames_give_the_same_rig() {
        let r = synth_pano::row(4, 0.33, false);
        let frames = row_frames(&r);
        let idx = [2usize, 0, 3, 1];
        let shuffled: Vec<Frame> = idx.iter().map(|&i| frames[i].clone()).collect();
        let rig = rig_of(&shuffled, r.focal, (r.w, r.h));
        let px = worst_px(&rig, &r, &idx);
        assert!(px < 0.5, "{px} px");
        let physical: Vec<usize> = rig.order.iter().map(|&k| idx[k]).collect();
        assert_eq!(physical, vec![0, 1, 2, 3], "still left to right");
    }

    /// A row whose reference looks up 5° and whose frames wander ±0.6° keeps every x axis level, so
    /// levelling is exact. ⚠ The control: without `level`, the tilt is the reference's 5°.
    #[test]
    fn the_horizon_is_level_though_the_reference_is_pitched() {
        let mut r = synth_pano::row(5, 0.33, false);
        for s in r.shots.iter_mut() {
            s.roll = 0.0;
        }
        r.shots[2].pitch = (-5f64).to_radians();
        let frames = row_frames(&r);
        let idx: Vec<usize> = (0..5).collect();
        let rig = rig_of(&frames, r.focal, (r.w, r.h));
        let t = tilt_deg(&rig, &r.shots, &idx);
        assert!(t < 0.05, "tilt {t}°");
        let mut unlevelled = rig.clone();
        unlevelled.rotations = rig
            .rotations
            .iter()
            .map(|q| q.compose(&rig.rotations[rig.reference].transpose()))
            .collect();
        assert!(tilt_deg(&unlevelled, &r.shots, &idx) > 4.0, "the control");
    }

    /// One frame rolled 2° pulls the fit by part of its roll (the module's note): measured, the six-frame
    /// row is under 1°, about a third of what using the reference as it was held would give.
    #[test]
    fn a_rolled_reference_tilts_the_horizon_less() {
        let mut r = synth_pano::row(6, 0.33, false);
        for s in r.shots.iter_mut() {
            s.roll = 0.0;
            s.pitch = 0.0;
        }
        r.shots[2].roll = 2f64.to_radians();
        let rig = rig_of(&row_frames(&r), r.focal, (r.w, r.h));
        let idx: Vec<usize> = (0..6).collect();
        assert_eq!(rig.reference, 2);
        let t = tilt_deg(&rig, &r.shots, &idx);
        eprintln!("rolled reference: tilt {t:.3}°");
        assert!(t < 1.0, "tilt {t}°");
    }

    #[test]
    fn a_portrait_row_is_levelled() {
        let mut r = synth_pano::row(4, 0.33, true);
        r.shots[1].pitch = (-3f64).to_radians();
        for s in r.shots.iter_mut() {
            s.roll = 0.0;
        }
        let rig = rig_of(&row_frames(&r), r.focal, (r.w, r.h));
        let idx: Vec<usize> = (0..4).collect();
        assert!(worst_px(&rig, &r, &idx) < 0.5);
        assert!(tilt_deg(&rig, &r.shots, &idx) < 0.05);
        assert_eq!(rig.sweep, Sweep::Horizontal);
    }

    #[test]
    fn a_vertical_sweep_runs_top_to_bottom() {
        let mut r = synth_pano::row(3, 0.33, false);
        for (i, s) in r.shots.iter_mut().enumerate() {
            (s.yaw, s.pitch, s.roll) = (0.0, (i as f64 - 1.0) * 0.45, 0.0);
        }
        let rig = rig_of(&row_frames(&r), r.focal, (r.w, r.h));
        assert_eq!(rig.sweep, Sweep::Vertical);
        // A positive pitch looks up (`synth_pano::camera_to_world`), so frame 2 is the top.
        assert_eq!(rig.order, vec![2, 1, 0]);
    }

    #[test]
    fn a_frame_without_focal_length_takes_the_fit() {
        let r = synth_pano::row(5, 0.4, false);
        let rig = rig_of(&row_frames(&r), r.focal * 1.2, (r.w, r.h));
        let ferr = (rig.focal_px - r.focal).abs() / r.focal;
        assert!(ferr < 0.01, "focal {} from a guess 20 % off", rig.focal_px);
    }

    #[test]
    fn focal_guess_reads_exif_and_the_crop_factor() {
        let meta = |lens: &str| raw_decode::RawMetadata {
            width: 6240,
            height: 4160,
            make: "FUJIFILM".into(),
            model: "X-T4".into(),
            lens: Some(raw_decode::LensMetadata {
                make: "FUJIFILM".into(),
                name: lens.into(),
                focal: 23.0,
                aperture: 2.0,
            }),
            as_shot: None,
        };
        // An X-T4's sensor is 23.5 mm wide: 23 mm on 6240 px is 6107 px; LensFun's crop factor is 1.53.
        let f = focal_guess(&meta("XF23mmF2 R WR"), 6240).expect("guessed");
        assert!((f - 6107.0).abs() / 6107.0 < 0.03, "{f}");
        let unknown = focal_guess(&meta("Mystery 23mm"), 6240).expect("by the maker");
        assert!((unknown - f).abs() / f < 0.03, "{unknown}");
        let mut none = meta("x");
        none.lens = None;
        assert!(focal_guess(&none, 6240).is_none());
    }
}
