//! Fitting a look: a lattice that maps Rustroom's rendering of a photo onto the camera's JPEG of it.
//!
//! ⚠⚠ **Least squares with a smoothness penalty, solved as an offset from the identity.** A film simulation is
//! learned from whatever colours the photos happened to hold; a lattice node no photo touched must stay where
//! the identity puts it, and one at the edge of the data must not swing — the penalty (a squared Laplacian)
//! carries the fitted correction smoothly into the unseen part of the cube. Measured on 18 X-T4 RAFs
//! (2026-11-11 spike): held-out ΔE 11.4 → 4.5, λ 0.01–1 within 0.3 of each other.

/// The lattice's points along each axis — a 17³ `.cube`.
pub const LATTICE: usize = 17;

/// The smoothness penalty's weight, per sample (the spike's middle value).
const LAMBDA: f32 = 0.1;

/// Conjugate-gradient iterations, at most.
const ITERATIONS: usize = 300;

/// One pixel: Rustroom's sRGB render and the camera JPEG's, both encoded in [0, 1].
pub type Pair = ([f32; 3], [f32; 3]);

/// Pairs worth fitting on, from one photo rendered both ways at `w` × `h`: the central 80 % (the edges are
/// where lens corrections and crops disagree), and only where the JPEG is flat — an edge one pixel off is a
/// colour that was never there.
pub fn samples(ours: &[[f32; 3]], theirs: &[[f32; 3]], w: usize, h: usize) -> Vec<Pair> {
    let mut out = Vec::new();
    if w < 10 || h < 10 || ours.len() != w * h || theirs.len() != w * h {
        return out;
    }
    let lum = |i: usize| theirs[i].iter().sum::<f32>();
    for y in (h / 10).max(1)..(h - h / 10).min(h - 1) {
        for x in (w / 10).max(1)..(w - w / 10).min(w - 1) {
            let i = y * w + x;
            let grad = (lum(i + 1) - lum(i - 1)).abs() + (lum(i + w) - lum(i - w)).abs();
            if grad < 0.08 {
                out.push((ours[i], theirs[i]));
            }
        }
    }
    out
}

/// A fitted look: `LATTICE³` outputs, red varying fastest — the `.cube` order.
#[derive(Debug, Clone, PartialEq)]
pub struct Lattice {
    entries: Vec<[f32; 3]>,
}

fn idx(r: usize, g: usize, b: usize) -> usize {
    (b * LATTICE + g) * LATTICE + r
}

/// A point's eight lattice neighbours and their trilinear weights.
fn weights(p: [f32; 3]) -> [(usize, f32); 8] {
    let s = (LATTICE - 1) as f32;
    let c = p.map(|v| (v.clamp(0.0, 1.0) * s).min(s - 1e-4));
    let i = c.map(|v| v.floor() as usize);
    let f = [c[0] - i[0] as f32, c[1] - i[1] as f32, c[2] - i[2] as f32];
    let mut w = [(0, 0.0); 8];
    let mut k = 0;
    for db in 0..2 {
        for dg in 0..2 {
            for dr in 0..2 {
                let pick = |d: usize, f: f32| if d == 1 { f } else { 1.0 - f };
                w[k] = (
                    idx(i[0] + dr, i[1] + dg, i[2] + db),
                    pick(dr, f[0]) * pick(dg, f[1]) * pick(db, f[2]),
                );
                k += 1;
            }
        }
    }
    w
}

fn identity() -> Vec<[f32; 3]> {
    (0..LATTICE.pow(3))
        .map(|j| {
            let (r, g, b) = (
                j % LATTICE,
                (j / LATTICE) % LATTICE,
                j / (LATTICE * LATTICE),
            );
            [r, g, b].map(|v| v as f32 / (LATTICE - 1) as f32)
        })
        .collect()
}

/// The lattice's discrete Laplacian of `v` (one channel), with the boundary's missing neighbours left out.
fn laplacian(v: &[f32]) -> Vec<f32> {
    let n = LATTICE as i32;
    let mut o = vec![0.0; v.len()];
    for b in 0..n {
        for g in 0..n {
            for r in 0..n {
                let j = idx(r as usize, g as usize, b as usize);
                let (mut sum, mut count) = (0.0, 0.0);
                for (dr, dg, db) in [
                    (-1, 0, 0),
                    (1, 0, 0),
                    (0, -1, 0),
                    (0, 1, 0),
                    (0, 0, -1),
                    (0, 0, 1),
                ] {
                    let (rr, gg, bb) = (r + dr, g + dg, b + db);
                    if (0..n).contains(&rr) && (0..n).contains(&gg) && (0..n).contains(&bb) {
                        sum += v[idx(rr as usize, gg as usize, bb as usize)];
                        count += 1.0;
                    }
                }
                o[j] = count * v[j] - sum;
            }
        }
    }
    o
}

/// The look that best maps each pair's first colour onto its second.
pub fn fit(pairs: &[Pair]) -> Lattice {
    let m = LATTICE.pow(3);
    let ident = identity();
    let mut entries = ident.clone();
    if pairs.is_empty() {
        return Lattice { entries };
    }
    let wts: Vec<[(usize, f32); 8]> = pairs.iter().map(|(x, _)| weights(*x)).collect();
    // ⚠ Scaled by samples per node, so the balance between the data and the smoothness does not depend on
    // how many photos were fitted.
    let scale = LAMBDA * pairs.len() as f32 / m as f32;
    for c in 0..3 {
        // (WᵀW + λLᵀL) x = Wᵀ(y − x₀): the offset from the identity.
        let apply = |x: &[f32]| -> Vec<f32> {
            let mut o = vec![0.0; m];
            for w in &wts {
                let v: f32 = w.iter().map(|(j, a)| a * x[*j]).sum();
                for (j, a) in w {
                    o[*j] += a * v;
                }
            }
            let l = laplacian(&laplacian(x));
            for j in 0..m {
                o[j] += scale * l[j] + 1e-6 * x[j];
            }
            o
        };
        let mut rhs = vec![0.0; m];
        for ((x, y), w) in pairs.iter().zip(&wts) {
            let base = x[c].clamp(0.0, 1.0);
            for (j, a) in w {
                rhs[*j] += a * (y[c] - base);
            }
        }
        let mut x = vec![0.0f32; m];
        let mut r = rhs;
        let mut p = r.clone();
        let mut rr: f32 = r.iter().map(|v| v * v).sum();
        for _ in 0..ITERATIONS {
            if rr < 1e-12 {
                break;
            }
            let ap = apply(&p);
            let a = rr / p.iter().zip(&ap).map(|(a, b)| a * b).sum::<f32>();
            for j in 0..m {
                x[j] += a * p[j];
                r[j] -= a * ap[j];
            }
            let rn: f32 = r.iter().map(|v| v * v).sum();
            for j in 0..m {
                p[j] = r[j] + rn / rr * p[j];
            }
            rr = rn;
        }
        for j in 0..m {
            entries[j][c] = (ident[j][c] + x[j]).clamp(0.0, 1.0);
        }
    }
    Lattice { entries }
}

impl Lattice {
    /// The look applied to `p`, trilinearly — what the pipeline's `.cube` sampling does.
    pub fn lookup(&self, p: [f32; 3]) -> [f32; 3] {
        let mut o = [0.0; 3];
        for (j, w) in weights(p) {
            for (c, v) in o.iter_mut().enumerate() {
                *v += w * self.entries[j][c];
            }
        }
        o
    }

    /// The look as a `.cube`, titled `title` — what the looks folder holds and `pipeline::lut` reads.
    pub fn to_cube(&self, title: &str) -> String {
        let mut s = format!(
            "# Learned by Rustroom from the camera's own JPEGs\nTITLE \"{}\"\nLUT_3D_SIZE {LATTICE}\n",
            title.replace('"', "'")
        );
        for e in &self.entries {
            s.push_str(&format!("{:.6} {:.6} {:.6}\n", e[0], e[1], e[2]));
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A constructed "film": warmer, more contrast, a little desaturated — smooth, as a simulation is.
    fn film(p: [f32; 3]) -> [f32; 3] {
        let l = (p[0] + p[1] + p[2]) / 3.0;
        let s = |v: f32| (v - 0.5) * 1.2 + 0.5;
        [
            s(l + (p[0] - l) * 0.8) + 0.04,
            s(l + (p[1] - l) * 0.8),
            s(l + (p[2] - l) * 0.8) - 0.04,
        ]
        .map(|v| v.clamp(0.0, 1.0))
    }

    /// A deterministic spread of colours, avoiding the cube's corners like a real photo does.
    fn colours(n: usize, seed: u32) -> Vec<[f32; 3]> {
        let mut s = seed;
        let mut next = || {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            0.1 + 0.8 * (s >> 8) as f32 / (1u32 << 24) as f32
        };
        (0..n).map(|_| [next(), next(), next()]).collect()
    }

    fn mean_error(f: impl Fn([f32; 3]) -> [f32; 3], pts: &[[f32; 3]]) -> f32 {
        pts.iter()
            .map(|p| {
                let (a, b) = (f(*p), film(*p));
                (0..3).map(|c| (a[c] - b[c]).abs()).sum::<f32>() / 3.0
            })
            .sum::<f32>()
            / pts.len() as f32
    }

    #[test]
    fn a_smooth_film_is_recovered_on_colours_it_never_saw() {
        let train: Vec<Pair> = colours(4000, 1).into_iter().map(|p| (p, film(p))).collect();
        let lut = fit(&train);
        let held_out = colours(500, 99);
        let before = mean_error(|p| p, &held_out);
        let after = mean_error(|p| lut.lookup(p), &held_out);
        // ⚠ The two must differ first: the film moves colours by several percent.
        assert!(
            before > 0.03,
            "the constructed film barely moves anything: {before}"
        );
        assert!(after < before / 5.0, "{before} → {after}");
    }

    #[test]
    fn where_no_photo_had_colour_the_look_stays_near_the_identity() {
        // Only dull, dark colours seen: saturated reds must not be flung anywhere.
        let train: Vec<Pair> = colours(2000, 3)
            .into_iter()
            .map(|p| p.map(|v| v * 0.4))
            .map(|p| (p, film(p)))
            .collect();
        let lut = fit(&train);
        let red = lut.lookup([0.95, 0.05, 0.05]);
        assert!(red[0] > 0.8 && red[1] < 0.25 && red[2] < 0.25, "{red:?}");
    }

    #[test]
    fn the_cube_text_reads_back_as_the_same_look() {
        let train: Vec<Pair> = colours(2000, 5).into_iter().map(|p| (p, film(p))).collect();
        let lut = fit(&train);
        let cube = pipeline::lut::Cube::parse(&lut.to_cube("Classic \"Chrome\" (X-T4)")).unwrap();
        assert_eq!(cube.size, LATTICE);
        for p in colours(50, 7) {
            let (a, b) = (lut.lookup(p), cube.lookup(p));
            for c in 0..3 {
                assert!((a[c] - b[c]).abs() < 1e-4, "{a:?} vs {b:?}");
            }
        }
    }

    #[test]
    fn samples_skip_the_border_and_edges() {
        let (w, h) = (40, 30);
        // Theirs: flat left half, flat right half — an edge down the middle.
        let theirs: Vec<[f32; 3]> = (0..w * h)
            .map(|i| if i % w < w / 2 { [0.2; 3] } else { [0.8; 3] })
            .collect();
        let ours = vec![[0.5; 3]; w * h];
        let s = samples(&ours, &theirs, w, h);
        let inner = (w - 2 * (w / 10)) * (h - 2 * (h / 10));
        assert!(!s.is_empty() && s.len() < inner, "{} of {inner}", s.len());
        assert!(
            samples(&ours[..10], &theirs, w, h).is_empty(),
            "mismatched sizes give nothing"
        );
    }
}
