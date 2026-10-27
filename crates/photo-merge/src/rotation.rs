//! Rotations and pairs (M10.17b): a handheld panorama turns the camera about its lens, so two frames are
//! related by a rotation alone. This module holds the rotation, the fit of one pair (RANSAC, then Horn's
//! closed form on the inliers) and the chain the pairs form; [`crate::rig`] fits them all together.
//!
//! Conventions, everywhere in the panorama: camera axes x right, y down, z forward; a [`Rot`] maps
//! **world → camera**; pixel `(x, y)` of a `w × h` frame at focal length `f` is the camera ray
//! `[(x − cx) / f, (y − cy) / f, 1]` with `cx = (w − 1) / 2`, `cy = (h − 1) / 2`.

use crate::features::Keypoint;

/// A rotation, row-major, world → camera.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rot(pub [f64; 9]);

impl Rot {
    pub fn identity() -> Self {
        Rot([1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0])
    }

    /// Rodrigues: a turn of `|v|` radians about `v`.
    pub fn from_axis_angle(v: [f64; 3]) -> Self {
        let t = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        if t < 1e-15 {
            return Rot([1.0, -v[2], v[1], v[2], 1.0, -v[0], -v[1], v[0], 1.0]);
        }
        let k = v.map(|c| c / t);
        let (s, c) = t.sin_cos();
        let o = 1.0 - c;
        Rot([
            c + k[0] * k[0] * o,
            k[0] * k[1] * o - k[2] * s,
            k[0] * k[2] * o + k[1] * s,
            k[1] * k[0] * o + k[2] * s,
            c + k[1] * k[1] * o,
            k[1] * k[2] * o - k[0] * s,
            k[2] * k[0] * o - k[1] * s,
            k[2] * k[1] * o + k[0] * s,
            c + k[2] * k[2] * o,
        ])
    }

    /// `self · other` — `other` first.
    pub fn compose(&self, other: &Rot) -> Rot {
        let (a, b) = (&self.0, &other.0);
        let mut m = [0.0; 9];
        for i in 0..3 {
            for j in 0..3 {
                m[i * 3 + j] = (0..3).map(|k| a[i * 3 + k] * b[k * 3 + j]).sum();
            }
        }
        Rot(m)
    }

    pub fn transpose(&self) -> Rot {
        let a = &self.0;
        Rot([a[0], a[3], a[6], a[1], a[4], a[7], a[2], a[5], a[8]])
    }

    pub fn apply(&self, v: [f64; 3]) -> [f64; 3] {
        let a = &self.0;
        [0, 1, 2].map(|i| a[i * 3] * v[0] + a[i * 3 + 1] * v[1] + a[i * 3 + 2] * v[2])
    }

    /// Row `i` — the camera's axis `i` in world directions.
    pub fn row(&self, i: usize) -> [f64; 3] {
        [self.0[i * 3], self.0[i * 3 + 1], self.0[i * 3 + 2]]
    }

    /// The turn's angle, radians.
    pub fn angle(&self) -> f64 {
        let tr = self.0[0] + self.0[4] + self.0[8];
        ((tr - 1.0) / 2.0).clamp(-1.0, 1.0).acos()
    }
}

/// One correspondence: pixel `pa` of frame `a` and pixel `pb` of frame `b` see the same direction
/// (full-size pixels).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Corr {
    pub a: usize,
    pub b: usize,
    pub pa: [f64; 2],
    pub pb: [f64; 2],
}

/// The unit camera ray of pixel `p` of a `size` frame at focal `f`.
pub fn unit_ray(p: [f64; 2], f: f64, size: (usize, usize)) -> [f64; 3] {
    let (cx, cy) = ((size.0 as f64 - 1.0) / 2.0, (size.1 as f64 - 1.0) / 2.0);
    let v = [(p[0] - cx) / f, (p[1] - cy) / f, 1.0];
    let n = (v[0] * v[0] + v[1] * v[1] + 1.0).sqrt();
    v.map(|c| c / n)
}

/// Camera ray `c` as a pixel of a `size` frame at focal `f`; `None` behind the camera.
pub fn to_pixel(c: [f64; 3], f: f64, size: (usize, usize)) -> Option<[f64; 2]> {
    if c[2] <= 1e-9 {
        return None;
    }
    let (cx, cy) = ((size.0 as f64 - 1.0) / 2.0, (size.1 as f64 - 1.0) / 2.0);
    Some([c[0] / c[2] * f + cx, c[1] / c[2] * f + cy])
}

/// The rotation taking each `from[i]` closest to `to[i]` (unit vectors) in least squares — Horn's
/// quaternion method; `None` for fewer than two or a degenerate set.
pub fn horn(from: &[[f64; 3]], to: &[[f64; 3]]) -> Option<Rot> {
    if from.len() < 2 || from.len() != to.len() {
        return None;
    }
    let spread = from.iter().skip(1).any(|v| {
        let c = cross(from[0], *v);
        c[0] * c[0] + c[1] * c[1] + c[2] * c[2] > 1e-12
    });
    if !spread {
        return None;
    }
    let mut sm = [[0.0f64; 3]; 3];
    for (a, b) in from.iter().zip(to) {
        for i in 0..3 {
            for j in 0..3 {
                sm[i][j] += a[i] * b[j];
            }
        }
    }
    let [[sxx, sxy, sxz], [syx, syy, syz], [szx, szy, szz]] = sm;
    let n = [
        [sxx + syy + szz, syz - szy, szx - sxz, sxy - syx],
        [syz - szy, sxx - syy - szz, sxy + syx, szx + sxz],
        [szx - sxz, sxy + syx, -sxx + syy - szz, syz + szy],
        [sxy - syx, szx + sxz, syz + szy, -sxx - syy + szz],
    ];
    let (vals, vecs) = jacobi4(n);
    let best = (0..4).max_by(|&i, &j| vals[i].total_cmp(&vals[j]))?;
    let [w, x, y, z] = [0, 1, 2, 3].map(|r| vecs[r][best]);
    Some(Rot([
        w * w + x * x - y * y - z * z,
        2.0 * (x * y - w * z),
        2.0 * (x * z + w * y),
        2.0 * (x * y + w * z),
        w * w - x * x + y * y - z * z,
        2.0 * (y * z - w * x),
        2.0 * (x * z - w * y),
        2.0 * (y * z + w * x),
        w * w - x * x - y * y + z * z,
    ]))
}

pub(crate) fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// The eigenvalues and eigenvectors (as columns) of a symmetric `N × N` matrix, cyclic Jacobi.
pub(crate) fn jacobi<const N: usize>(mut a: [[f64; N]; N]) -> ([f64; N], [[f64; N]; N]) {
    let mut v = [[0.0; N]; N];
    for (i, row) in v.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    for _ in 0..64 {
        let off: f64 = (0..N)
            .flat_map(|i| (0..N).filter(move |&j| j != i).map(move |j| (i, j)))
            .map(|(i, j)| a[i][j] * a[i][j])
            .sum();
        if off < 1e-30 {
            break;
        }
        for p in 0..N {
            for q in p + 1..N {
                if a[p][q].abs() < 1e-300 {
                    continue;
                }
                let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
                let sign = if theta >= 0.0 { 1.0 } else { -1.0 };
                let t = sign / (theta.abs() + (theta * theta + 1.0).sqrt());
                let c = 1.0 / (t * t + 1.0).sqrt();
                let s = t * c;
                for k in 0..N {
                    let (akp, akq) = (a[k][p], a[k][q]);
                    a[k][p] = c * akp - s * akq;
                    a[k][q] = s * akp + c * akq;
                }
                for k in 0..N {
                    let (apk, aqk) = (a[p][k], a[q][k]);
                    a[p][k] = c * apk - s * aqk;
                    a[q][k] = s * apk + c * aqk;
                }
                for row in v.iter_mut() {
                    let (vkp, vkq) = (row[p], row[q]);
                    row[p] = c * vkp - s * vkq;
                    row[q] = s * vkp + c * vkq;
                }
            }
        }
    }
    (std::array::from_fn(|i| a[i][i]), v)
}

fn jacobi4(a: [[f64; 4]; 4]) -> ([f64; 4], [[f64; 4]; 4]) {
    jacobi(a)
}

/// What RANSAC found for one pair: `b`'s camera = `r_ab` · `a`'s camera, and the agreeing correspondences.
#[derive(Clone, Debug)]
pub struct PairFit {
    pub r_ab: Rot,
    pub inliers: Vec<Corr>,
}

/// RANSAC's reprojection threshold, full-size pixels.
pub const INLIER_PX: f64 = 3.0;
/// A pair is connected at this many inliers.
pub const MIN_INLIERS: usize = 25;
const ITERATIONS: usize = 500;

/// Fit frames `pair.0` and `pair.1`'s matches `m` (indices into `a` and `b`) to a rotation at focal
/// `focal_px`; `None` when fewer than [`MIN_INLIERS`] agree.
pub fn fit_pair(
    pair: (usize, usize),
    a: &[Keypoint],
    b: &[Keypoint],
    m: &[(usize, usize)],
    focal_px: f64,
    size: (usize, usize),
) -> Option<PairFit> {
    if m.len() < MIN_INLIERS {
        return None;
    }
    let pa: Vec<[f64; 2]> = m
        .iter()
        .map(|&(i, _)| [a[i].x as f64, a[i].y as f64])
        .collect();
    let pb: Vec<[f64; 2]> = m
        .iter()
        .map(|&(_, j)| [b[j].x as f64, b[j].y as f64])
        .collect();
    let ua: Vec<[f64; 3]> = pa.iter().map(|p| unit_ray(*p, focal_px, size)).collect();
    let ub: Vec<[f64; 3]> = pb.iter().map(|p| unit_ray(*p, focal_px, size)).collect();
    let agree = |r: &Rot| -> Vec<usize> {
        (0..m.len())
            .filter(|&k| {
                to_pixel(r.apply(ua[k]), focal_px, size)
                    .is_some_and(|q| (q[0] - pb[k][0]).hypot(q[1] - pb[k][1]) < INLIER_PX)
            })
            .collect()
    };
    let mut state = 0x9E37_79B9_7F4A_7C15u64 ^ ((pair.0 as u64) << 32 | pair.1 as u64);
    let mut pick = || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        (state % m.len() as u64) as usize
    };
    let mut best: Vec<usize> = Vec::new();
    for _ in 0..ITERATIONS {
        let (i, j) = (pick(), pick());
        if i == j {
            continue;
        }
        let Some(r) = horn(&[ua[i], ua[j]], &[ub[i], ub[j]]) else {
            continue;
        };
        let inl = agree(&r);
        if inl.len() > best.len() {
            best = inl;
        }
    }
    // Refit on the inliers, twice: the minimal sample's rotation is rough.
    let mut r = None;
    for _ in 0..2 {
        if best.len() < 2 {
            return None;
        }
        let fit = horn(
            &best.iter().map(|&k| ua[k]).collect::<Vec<_>>(),
            &best.iter().map(|&k| ub[k]).collect::<Vec<_>>(),
        )?;
        best = agree(&fit);
        r = Some(fit);
    }
    let r_ab = r?;
    (best.len() >= MIN_INLIERS).then(|| PairFit {
        r_ab,
        inliers: best
            .iter()
            .map(|&k| Corr {
                a: pair.0,
                b: pair.1,
                pa: pa[k],
                pb: pb[k],
            })
            .collect(),
    })
}

/// The frames' order, from the connected pairs `(a, b, inliers)`: the maximum spanning tree on inlier
/// counts, which for one row is a path; the path is returned from one of its ends (which end is
/// [`crate::rig::solve`]'s to decide, once the rotations say which lies left). `Err` names the frames
/// outside the largest connected group — or is empty when the tree is not a path (a grid, a fan).
pub fn chain(n: usize, pairs: &[(usize, usize, usize)]) -> Result<Vec<usize>, Vec<usize>> {
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(p: &mut [usize], i: usize) -> usize {
        let mut r = i;
        while p[r] != r {
            r = p[r];
        }
        p[i] = r;
        r
    }
    let mut sorted: Vec<&(usize, usize, usize)> = pairs
        .iter()
        .filter(|p| p.0 < n && p.1 < n && p.0 != p.1)
        .collect();
    sorted.sort_by(|x, y| y.2.cmp(&x.2).then((x.0, x.1).cmp(&(y.0, y.1))));
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    for &&(a, b, _) in &sorted {
        let (ra, rb) = (find(&mut parent, a), find(&mut parent, b));
        if ra != rb {
            parent[ra] = rb;
            adj[a].push(b);
            adj[b].push(a);
        }
    }
    let roots: Vec<usize> = (0..n).map(|i| find(&mut parent, i)).collect();
    let largest = (0..n)
        .max_by_key(|&r| {
            (
                roots.iter().filter(|&&x| x == r).count(),
                std::cmp::Reverse(r),
            )
        })
        .map(|r| roots[r]);
    let outside: Vec<usize> = (0..n).filter(|&i| Some(roots[i]) != largest).collect();
    if !outside.is_empty() {
        return Err(outside);
    }
    if adj.iter().any(|a| a.len() > 2) {
        return Err(Vec::new());
    }
    let start = (0..n).find(|&i| adj[i].len() <= 1).unwrap_or(0);
    let mut order = vec![start];
    let mut prev = usize::MAX;
    let mut at = start;
    while let Some(&next) = adj[at].iter().find(|&&j| j != prev) {
        order.push(next);
        (prev, at) = (at, next);
    }
    Ok(order)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::{detect, matches, FEATURE_LONG_EDGE};
    use crate::synth_pano::{self, Scene};

    fn norm(v: [f64; 3]) -> [f64; 3] {
        let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        v.map(|c| c / n)
    }

    #[test]
    fn horn_recovers_a_rotation() {
        let r = Rot::from_axis_angle([0.1, -0.3, 0.05]);
        let from: Vec<[f64; 3]> = [
            [0.1, 0.2, 1.0],
            [-0.4, 0.1, 1.0],
            [0.3, -0.5, 1.0],
            [0.0, 0.0, 1.0],
        ]
        .map(norm)
        .to_vec();
        let to: Vec<[f64; 3]> = from.iter().map(|v| r.apply(*v)).collect();
        let got = horn(&from, &to).expect("fits");
        assert!(got.compose(&r.transpose()).angle() < 1e-9);
        let two = horn(&from[..2], &to[..2]).expect("two points fix a rotation");
        assert!(two.compose(&r.transpose()).angle() < 1e-9);
        assert!(horn(&from[..1], &to[..1]).is_none());
    }

    #[test]
    fn axis_angle_and_its_transpose_undo_each_other() {
        let r = Rot::from_axis_angle([0.4, 0.2, -0.7]);
        assert!(r.compose(&r.transpose()).angle() < 1e-12);
        assert!((r.angle() - (0.16f64 + 0.04 + 0.49).sqrt()).abs() < 1e-12);
    }

    /// The truth for a synthetic pair: `b`'s camera from `a`'s.
    fn truth(a: &synth_pano::Shot, b: &synth_pano::Shot) -> Rot {
        let t = |s: &synth_pano::Shot| {
            let m = s.rotation();
            Rot([
                m[0][0], m[0][1], m[0][2], m[1][0], m[1][1], m[1][2], m[2][0], m[2][1], m[2][2],
            ])
        };
        // camera→world is `rotation()`; world→camera its transpose.
        t(b).transpose().compose(&t(a))
    }

    #[test]
    fn a_pair_is_fitted_with_its_inliers() {
        let r = synth_pano::row(2, 0.35, false);
        let f = synth_pano::render(&Scene::new(1), &r.shots, r.w, r.h, r.focal);
        let (a, b) = (
            detect(&f[0], FEATURE_LONG_EDGE),
            detect(&f[1], FEATURE_LONG_EDGE),
        );
        let m = matches(&a, &b);
        let fit = fit_pair((0, 1), &a, &b, &m, r.focal, (r.w, r.h)).expect("connected");
        assert!(
            fit.inliers.len() >= MIN_INLIERS && fit.inliers.iter().all(|c| (c.a, c.b) == (0, 1))
        );
        let err = fit
            .r_ab
            .compose(&truth(&r.shots[0], &r.shots[1]).transpose())
            .angle();
        assert!(err.to_degrees() < 0.05, "{} degrees off", err.to_degrees());
    }

    #[test]
    fn an_unrelated_pair_is_not_connected() {
        let r = synth_pano::row(1, 0.35, false);
        let f1 = synth_pano::render(&Scene::new(1), &r.shots, r.w, r.h, r.focal);
        let f2 = synth_pano::render(&Scene::new(2), &r.shots, r.w, r.h, r.focal);
        let (a, b) = (
            detect(&f1[0], FEATURE_LONG_EDGE),
            detect(&f2[0], FEATURE_LONG_EDGE),
        );
        assert!(fit_pair((0, 1), &a, &b, &matches(&a, &b), r.focal, (r.w, r.h)).is_none());
    }

    #[test]
    fn the_chain_of_a_row_is_a_path() {
        // Shuffled: the row is 2-0-3-1; the weak 2-3 link is not the tree's.
        let pairs = [(0, 2, 300), (0, 3, 280), (3, 1, 310), (2, 3, 30)];
        let order = chain(4, &pairs).expect("a row");
        assert!(
            order == vec![2, 0, 3, 1] || order == vec![1, 3, 0, 2],
            "{order:?}"
        );
    }

    #[test]
    fn an_unrelated_frame_is_named_unconnected() {
        assert_eq!(chain(4, &[(0, 1, 100), (1, 2, 90)]), Err(vec![3]));
    }

    #[test]
    fn a_grid_is_not_a_row() {
        // Frame 0 has three strong neighbours.
        assert_eq!(
            chain(4, &[(0, 1, 100), (0, 2, 100), (0, 3, 100)]),
            Err(vec![])
        );
    }

    #[test]
    fn two_frames_are_a_row() {
        assert!(matches!(
            chain(2, &[(1, 0, 50)]).as_deref(),
            Ok([0, 1]) | Ok([1, 0])
        ));
    }
}
