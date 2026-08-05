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
