//! Spatial helpers for the neighbourhood-based stages.
//!
//! These work on a single-channel plane (`&[f32]` of `w * h` samples) rather than an
//! `ImageBuf`, because the filters that need them operate on decorrelated channels —
//! Oklab L, a, b — not on interleaved RGB.

use rayon::prelude::*;

/// Blur `plane` with a `(2r+1) × (2r+1)` box kernel.
///
/// Separable, and each pass keeps a running sum, so the cost per pixel is **constant
/// regardless of `r`**. That is what makes a radius that scales with resolution
/// affordable — an O(r²) kernel would need ~3600 taps per pixel on a 24 Mpx export.
///
/// Borders clamp the *window*, not the samples: near an edge the window shrinks and the
/// sum is divided by the number of samples actually inside it. That keeps a constant
/// plane exactly constant, which the "treat outside as zero" variant famously does not.
///
/// # Panics
/// Panics if `plane.len() != w * h`.
pub fn box_blur(plane: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    assert_eq!(plane.len(), w * h, "plane size mismatch");
    if r == 0 || w == 0 || h == 0 {
        return plane.to_vec();
    }
    let horizontal = blur_rows(plane, w, h, r);
    blur_cols(&horizontal, w, h, r)
}

/// Horizontal pass: one running sum per row, rows farmed out to rayon.
fn blur_rows(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; w * h];
    out.par_chunks_exact_mut(w)
        .zip(src.par_chunks_exact(w))
        .for_each(|(orow, srow)| {
            // Window for x = 0 is [0, min(r + 1, w)).
            let mut hi = (r + 1).min(w);
            let mut lo = 0usize;
            let mut sum: f32 = srow[..hi].iter().sum();
            for (x, o) in orow.iter_mut().enumerate() {
                *o = sum / (hi - lo) as f32;
                // Slide one step right for the next x.
                if x + r + 1 < w {
                    sum += srow[x + r + 1];
                    hi += 1;
                }
                if x >= r {
                    sum -= srow[x - r];
                    lo += 1;
                }
            }
        });
    out
}

/// Vertical pass: one running sum *per column*, all columns advanced together so every
/// memory access is a contiguous row slice.
///
/// Walking one column at a time would stride by `w` and miss cache on every access; a
/// transpose-blur-transpose would scatter-write the whole buffer four times per blur.
/// This costs about four sequential passes over the data and no extra allocation beyond
/// the accumulator row. It is deliberately not parallelized — it is memory-bandwidth
/// bound, and splitting it into row bands means re-priming `sum` per band. Revisit only
/// if a profile says so.
fn blur_cols(src: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut out = vec![0.0f32; w * h];
    let mut sum = vec![0.0f32; w];

    let mut hi = (r + 1).min(h);
    let mut lo = 0usize;
    for y in 0..hi {
        for (s, v) in sum.iter_mut().zip(&src[y * w..(y + 1) * w]) {
            *s += v;
        }
    }

    for y in 0..h {
        let inv = 1.0 / (hi - lo) as f32;
        for (o, s) in out[y * w..(y + 1) * w].iter_mut().zip(&sum) {
            *o = s * inv;
        }
        if y + r + 1 < h {
            let add = &src[(y + r + 1) * w..(y + r + 2) * w];
            for (s, v) in sum.iter_mut().zip(add) {
                *s += v;
            }
            hi += 1;
        }
        if y >= r {
            let sub = &src[(y - r) * w..(y - r + 1) * w];
            for (s, v) in sum.iter_mut().zip(sub) {
                *s -= v;
            }
            lo += 1;
        }
    }
    out
}

/// Half-width of the kernel for `sigma`: `ceil(3σ)`, where 99.7% of the weight lies.
///
/// Zero for a non-positive or non-finite sigma, which [`gaussian_blur`] treats as the
/// identity. Public because callers need it to reason about how far a pixel's output
/// reaches — see the region margin invariant in the app.
pub fn kernel_radius(sigma: f32) -> usize {
    if sigma.is_finite() && sigma > 0.0 {
        (3.0 * sigma).ceil() as usize
    } else {
        0
    }
}

/// Normalized Gaussian weights of length `2·kernel_radius(sigma) + 1`, index 0 at `-r`.
fn gaussian_kernel(sigma: f32) -> Vec<f32> {
    let r = kernel_radius(sigma) as isize;
    let denom = 2.0 * sigma * sigma;
    let mut k: Vec<f32> = (-r..=r)
        .map(|x| (-((x * x) as f32) / denom).exp())
        .collect();
    let sum: f32 = k.iter().sum();
    for v in k.iter_mut() {
        *v /= sum;
    }
    k
}

/// Separable Gaussian blur of a single-channel plane, with a **float** sigma.
///
/// Sits *alongside* [`box_blur`] rather than replacing it — they serve opposite regimes.
/// `box_blur` is O(1) in its radius, which is what makes noise reduction's 6–32 px windows
/// affordable. Sharpening works at σ = 0.5–3 px, where a direct kernel is cheaper (7 taps at
/// σ=1) **and** where an integer box radius cannot represent a sub-pixel blur at all.
///
/// Kernel radius `ceil(3σ)`, weights normalized to sum 1, and out-of-bounds reads **replicate
/// the edge sample** — so a constant plane stays exactly constant. Both passes are parallel
/// over rows, which `box_blur`'s vertical pass could not be because it carries a running sum.
///
/// `sigma <= 0` (or non-finite) returns a copy.
///
/// # Panics
/// Panics if `plane.len() != w * h`.
pub fn gaussian_blur(plane: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    assert_eq!(plane.len(), w * h, "plane size mismatch");
    if kernel_radius(sigma) == 0 || w == 0 || h == 0 {
        return plane.to_vec();
    }
    let kernel = gaussian_kernel(sigma);
    let horizontal = gauss_rows(plane, w, h, &kernel);
    gauss_cols(&horizontal, w, h, &kernel)
}

/// Horizontal pass. Rows are independent, so rayon splits them directly.
fn gauss_rows(src: &[f32], w: usize, h: usize, k: &[f32]) -> Vec<f32> {
    let r = (k.len() - 1) / 2;
    let mut out = vec![0.0f32; w * h];
    out.par_chunks_exact_mut(w)
        .zip(src.par_chunks_exact(w))
        .for_each(|(orow, srow)| {
            for (x, o) in orow.iter_mut().enumerate() {
                let mut acc = 0.0f32;
                for (i, &kw) in k.iter().enumerate() {
                    // Clamp the *sample index* (replicate), not the window: `x + i - r`
                    // saturating at 0 on the left, at `w - 1` on the right.
                    let sx = (x + i).saturating_sub(r).min(w - 1);
                    acc += kw * srow[sx];
                }
                *o = acc;
            }
        });
    out
}

/// Vertical pass. Unlike `box_blur`'s, this carries no running state, so each output row
/// can be accumulated independently — and every read is still a contiguous row slice.
fn gauss_cols(src: &[f32], w: usize, h: usize, k: &[f32]) -> Vec<f32> {
    let r = (k.len() - 1) / 2;
    let mut out = vec![0.0f32; w * h];
    out.par_chunks_exact_mut(w)
        .enumerate()
        .for_each(|(y, orow)| {
            for (i, &kw) in k.iter().enumerate() {
                let sy = (y + i).saturating_sub(r).min(h - 1);
                let srow = &src[sy * w..(sy + 1) * w];
                for (o, s) in orow.iter_mut().zip(srow) {
                    *o += kw * s;
                }
            }
        });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn radius_zero_is_identity() {
        let p = vec![1.0, 2.0, 3.0, 4.0];
        assert_eq!(box_blur(&p, 2, 2, 0), p);
    }

    #[test]
    fn constant_plane_stays_constant() {
        // The classic edge-normalization bug: if out-of-bounds samples are treated as
        // zero instead of shrinking the window, the borders darken here.
        let p = vec![0.375f32; 7 * 5];
        let out = box_blur(&p, 7, 5, 2);
        for (i, v) in out.iter().enumerate() {
            assert!((v - 0.375).abs() < 1e-6, "sample {i} = {v}");
        }
    }

    #[test]
    fn known_one_dimensional_case() {
        let p = vec![0.0, 0.0, 1.0, 0.0, 0.0];
        let out = box_blur(&p, 5, 1, 1);
        let third = 1.0 / 3.0;
        let expect = [0.0, third, third, third, 0.0];
        for (o, e) in out.iter().zip(expect) {
            assert!((o - e).abs() < 1e-6, "{out:?} vs {expect:?}");
        }
    }

    #[test]
    fn known_two_dimensional_case() {
        // A single 1 in the middle of a 3x3, r=1. With windows clamped to the image the
        // corner sees 4 samples, an edge 6, the centre 9 — so this one assertion covers
        // separability, edge normalization and the running sums at once.
        let mut p = vec![0.0f32; 9];
        p[4] = 1.0;
        let out = box_blur(&p, 3, 3, 1);
        let expect = [
            1.0 / 4.0,
            1.0 / 6.0,
            1.0 / 4.0,
            1.0 / 6.0,
            1.0 / 9.0,
            1.0 / 6.0,
            1.0 / 4.0,
            1.0 / 6.0,
            1.0 / 4.0,
        ];
        for (i, (o, e)) in out.iter().zip(expect).enumerate() {
            assert!((o - e).abs() < 1e-6, "sample {i}: {o} vs {e}");
        }
    }

    #[test]
    fn radius_larger_than_image_gives_the_global_mean() {
        let p: Vec<f32> = (0..12).map(|i| i as f32).collect();
        let mean = p.iter().sum::<f32>() / 12.0;
        let out = box_blur(&p, 4, 3, 50);
        for v in &out {
            assert!((v - mean).abs() < 1e-5, "{v} vs {mean}");
        }
    }

    #[test]
    fn kernel_radius_is_three_sigma_rounded_up() {
        assert_eq!(kernel_radius(1.0), 3);
        assert_eq!(kernel_radius(0.5), 2); // ceil(1.5)
        assert_eq!(kernel_radius(3.0), 9);
        // A non-positive or non-finite sigma has no kernel at all; gaussian_blur treats
        // that as the identity.
        assert_eq!(kernel_radius(0.0), 0);
        assert_eq!(kernel_radius(-1.0), 0);
        assert_eq!(kernel_radius(f32::NAN), 0);
    }

    #[test]
    fn non_positive_sigma_is_identity() {
        let p = vec![1.0, 2.0, 3.0, 4.0];
        assert_eq!(gaussian_blur(&p, 2, 2, 0.0), p);
        assert_eq!(gaussian_blur(&p, 2, 2, -2.0), p);
    }

    #[test]
    fn gaussian_constant_plane_stays_constant() {
        // Requires both halves: weights normalized to sum 1, *and* out-of-bounds reads
        // replicating the edge sample rather than contributing zero.
        let p = vec![0.375f32; 9 * 7];
        let out = gaussian_blur(&p, 9, 7, 1.5);
        for (i, v) in out.iter().enumerate() {
            assert!((v - 0.375).abs() < 1e-6, "sample {i} = {v}");
        }
    }

    #[test]
    fn gaussian_kernel_matches_the_analytic_weights() {
        // Independently recomputed from exp(-x^2 / 2*sigma^2), then normalized.
        let sigma = 1.0f32;
        let k = gaussian_kernel(sigma);
        assert_eq!(k.len(), 7, "radius 3 => 7 taps");
        let raw: Vec<f32> = (-3..=3)
            .map(|x| (-(x as f32 * x as f32) / (2.0 * sigma * sigma)).exp())
            .collect();
        let sum: f32 = raw.iter().sum();
        for (i, (&got, &r)) in k.iter().zip(raw.iter()).enumerate() {
            assert!(
                (got - r / sum).abs() < 1e-6,
                "tap {i}: {got} vs {}",
                r / sum
            );
        }
        // A hard number, so the test is not just the implementation restated.
        assert!((k[3] - 0.399_05).abs() < 1e-4, "centre tap {}", k[3]);
        assert!((k.iter().sum::<f32>() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn impulse_response_is_symmetric_and_peaks_at_the_centre() {
        let (w, h) = (11usize, 11usize);
        let mut p = vec![0.0f32; w * h];
        p[5 * w + 5] = 1.0;
        let out = gaussian_blur(&p, w, h, 1.0);
        let centre = out[5 * w + 5];
        for y in 0..h {
            for x in 0..w {
                if (x, y) != (5, 5) {
                    assert!(out[y * w + x] < centre, "({x},{y}) exceeds the centre");
                }
                // Mirror symmetry in both axes.
                assert!((out[y * w + x] - out[y * w + (w - 1 - x)]).abs() < 1e-6);
                assert!((out[y * w + x] - out[(h - 1 - y) * w + x]).abs() < 1e-6);
            }
        }
        // Energy is conserved (edges replicate, so nothing leaks out of a centred impulse).
        assert!((out.iter().sum::<f32>() - 1.0).abs() < 1e-4);
    }

    #[test]
    fn larger_sigma_reduces_variance_more() {
        let (w, h) = (32usize, 32usize);
        let mut state: u32 = 0x1234_5678;
        let p: Vec<f32> = (0..w * h)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                0.5 + (state as f32 / u32::MAX as f32 - 0.5) * 0.2
            })
            .collect();
        let var = |v: &[f32]| {
            let m = v.iter().sum::<f32>() / v.len() as f32;
            v.iter().map(|x| (x - m) * (x - m)).sum::<f32>() / v.len() as f32
        };
        let small = var(&gaussian_blur(&p, w, h, 0.5));
        let large = var(&gaussian_blur(&p, w, h, 2.0));
        assert!(
            large < small,
            "sigma 0.5 var {small}, sigma 2.0 var {large}"
        );
        assert!(small < var(&p));
    }

    #[test]
    fn blur_preserves_horizontal_mirror_symmetry() {
        // An asymmetric implementation (e.g. an off-by-one in the sliding window) shows
        // up here even when the totals happen to look right.
        let w = 6;
        let src: Vec<f32> = vec![0.1, 0.9, 0.3, 0.3, 0.9, 0.1];
        let out = box_blur(&src, w, 1, 2);
        for i in 0..w / 2 {
            assert!(
                (out[i] - out[w - 1 - i]).abs() < 1e-6,
                "{i}: {} vs {}",
                out[i],
                out[w - 1 - i]
            );
        }
    }
}
