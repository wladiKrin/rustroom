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
///
/// ⚠⚠ **`pub` so a GPU twin uploads THESE weights rather than recomputing them.** Two
/// implementations of a Gaussian is how the two come to disagree about a sigma nobody re-derives —
/// the rule `HueTable::entries` already follows for the hue table, and for the same reason.
pub fn gaussian_kernel(sigma: f32) -> Vec<f32> {
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

/// Number of box-blur passes [`wide_blur`] uses. Three is where the iterated box response
/// gets within a few percent of a true Gaussian; the cost is linear in this and the accuracy
/// gain past three is not worth a fourth pass over the data.
/// ⚠ `pub` so the GPU twin iterates the SAME number of box passes. Two definitions of "how
/// many" is how a preview stops being evidence about the render.
pub const WIDE_PASSES: usize = 3;

/// Box radius whose [`WIDE_PASSES`]-fold iteration has variance `sigma^2`.
///
/// Each pass of radius `r` has variance `((2r+1)^2 - 1)/12`, so three of them give
/// `((2r+1)^2 - 1)/4 = sigma^2`, i.e. `r = (sqrt(1 + 4*sigma^2) - 1)/2`.
///
/// Returns 0 — meaning "identity", exactly as [`kernel_radius`] does — for a non-positive,
/// non-finite, or **too small** sigma. There is deliberately no `.max(1)`: a box radius of 1
/// is an effective sigma of 0.82, so clamping up would render `sigma = 0.1` eight times too
/// wide. Below sigma ~= 0.87 an integer box radius cannot represent the blur at all, which
/// is precisely why [`gaussian_blur`] exists for the small-sigma regime.
pub fn wide_blur_radius(sigma: f32) -> usize {
    if !sigma.is_finite() || sigma <= 0.0 {
        return 0;
    }
    let r = ((1.0 + 4.0 * sigma * sigma).sqrt() - 1.0) / 2.0;
    r.round().max(0.0) as usize
}

/// The furthest an output sample of [`wide_blur`] can reach into its input.
///
/// Public because callers must reason about it: a stage using this in the pipeline *tail*
/// runs on a cropped region, so the renderer has to keep at least this much margin outside
/// what it displays. See `crate::tail_kernel_radius`.
pub fn wide_blur_reach(sigma: f32) -> usize {
    WIDE_PASSES * wide_blur_radius(sigma)
}

/// Gaussian-like blur for **large** sigma, built from iterated [`box_blur`] passes.
///
/// The third primitive in this module, and the three cover three regimes:
///
/// | primitive | regime | why |
/// |---|---|---|
/// | [`gaussian_blur`] | sub-pixel to a few px | a direct kernel is cheap there (7 taps at σ=1), and an integer box radius cannot express a sub-pixel blur at all |
/// | [`wide_blur`] | tens of px | O(1) in sigma, and at that scale the box-iteration error is invisible |
/// | [`box_blur`] | any, when a genuine box is wanted | the guided filter in noise reduction needs box means, not Gaussian ones |
///
/// A Gaussian at sigma = 40 would need 241 taps per pass — about 13 G operations on a
/// 26 Mpx export. This is three passes of running sums regardless of sigma.
///
/// Inherits `box_blur`'s border behaviour (the *window* clamps, not the samples), so a
/// constant plane stays exactly constant. Returns a copy when the sigma is too small to
/// represent — see [`wide_blur_radius`].
///
/// # Panics
/// Panics if `plane.len() != w * h`.
pub fn wide_blur(plane: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    assert_eq!(plane.len(), w * h, "plane size mismatch");
    let r = wide_blur_radius(sigma);
    if r == 0 || w == 0 || h == 0 {
        return plane.to_vec();
    }
    let mut out = box_blur(plane, w, h, r);
    for _ in 1..WIDE_PASSES {
        out = box_blur(&out, w, h, r);
    }
    out
}

/// Guided filter (He et al.): edge-preserving smoothing of `p`, with edges taken from
/// `guide`. O(1) per pixel in `r`, because every window statistic is a [`box_blur`].
///
/// `slope` and `offset` are the paper's `a` and `b`, renamed because `a` and `b` are the
/// Oklab chroma channels in the noise-reduction stage that calls this.
///
/// The behaviour lives entirely in `slope`. Where the guide's local variance greatly exceeds
/// `eps` — an edge — `slope → 1` and the pixel follows the guide. Where it is far below
/// `eps` — flat, or noise — `slope → 0` and the pixel becomes its local mean.
///
/// **`p == guide` is the self-guided special case** noise reduction uses. Dehaze uses the
/// general form: it filters the transmission map, guided by image luma, which is what pulls
/// the blocky patch-minimum transmission back onto the real depth edges.
///
/// # Panics
/// Panics if `p.len() != w * h` or `guide.len() != w * h`.
pub fn guided_filter(p: &[f32], guide: &[f32], w: usize, h: usize, r: usize, eps: f32) -> Vec<f32> {
    assert_eq!(p.len(), w * h, "p size mismatch");
    assert_eq!(guide.len(), w * h, "guide size mismatch");

    // ⚠⚠ **When `p` IS `guide`, `mean_p` is `mean_guide` and `cross` is `guide_sq`** — so the
    // general form computes four statistic box blurs where two suffice, and builds two
    // whole-image temporaries it does not need. Both self-guided callers (noise reduction and
    // negative Texture) pay that today.
    //
    // Measured at 3200x2136: **40.6 -> 28.3 ms, and bit-identical**, because the arithmetic that
    // survives is the same arithmetic in the same order. Dehaze uses the general form — it
    // filters the transmission map guided by luma — and is untouched.
    //
    // ⚠ Detected by **pointer identity**, not by comparing contents: comparing would cost a full
    // pass over the data to save two, and two callers that mean "the same plane" pass the same
    // slice. A caller passing an equal-but-distinct plane simply takes the general path, which is
    // correct and merely slower — the check fails toward doing more work, never toward doing the
    // wrong work.
    let self_guided = std::ptr::eq(p.as_ptr(), guide.as_ptr()) && p.len() == guide.len();

    let guide_sq: Vec<f32> = guide.iter().map(|v| v * v).collect();
    let mean_guide = box_blur(guide, w, h, r);
    let mean_guide_sq = box_blur(&guide_sq, w, h, r);

    // ⚠ Borrowed rather than cloned in the self-guided case: a clone would hand back the ~27 MB
    // per plane that the skipped blurs were the point of not spending.
    let mean_p_owned = (!self_guided).then(|| box_blur(p, w, h, r));
    let mean_cross_owned = (!self_guided).then(|| {
        let cross: Vec<f32> = guide.iter().zip(p).map(|(g, v)| g * v).collect();
        box_blur(&cross, w, h, r)
    });
    let mean_p: &[f32] = mean_p_owned.as_deref().unwrap_or(&mean_guide);
    let mean_cross: &[f32] = mean_cross_owned.as_deref().unwrap_or(&mean_guide_sq);

    let mut slope = Vec::with_capacity(p.len());
    let mut offset = Vec::with_capacity(p.len());
    for (((&mg, &mp), &mgsq), &mc) in mean_guide
        .iter()
        .zip(mean_p.iter())
        .zip(mean_guide_sq.iter())
        .zip(mean_cross.iter())
    {
        // Cancellation between two nearly equal sums can push the variance slightly negative
        // on perfectly flat areas. The covariance is *not* clamped: unlike a variance it can
        // legitimately be negative when `p` and `guide` are anti-correlated.
        let var = (mgsq - mg * mg).max(0.0);
        let cov = mc - mg * mp;
        let s = cov / (var + eps);
        slope.push(s);
        offset.push(mp - s * mg);
    }

    let mean_slope = box_blur(&slope, w, h, r);
    let mean_offset = box_blur(&offset, w, h, r);
    mean_slope
        .iter()
        .zip(mean_offset.iter())
        .zip(guide.iter())
        .map(|((&ms, &mo), &g)| ms * g + mo)
        .collect()
}

/// Sliding-window minimum over one contiguous line, window `[i-r, i+r]` clamped to the ends.
///
/// Monotonic deque: indices are held with strictly increasing values, so the front is always
/// the minimum of the current window. Each index is pushed and popped at most once, which
/// makes this **O(1) amortised per sample regardless of `r`** — the same reason `box_blur`
/// keeps running sums.
///
/// A deque rather than the van Herk / Gil-Werman block decomposition specifically because the
/// window **clamps** at the borders instead of reading outside, which a deque expresses
/// directly and block decomposition does not.
pub fn min_1d(line: &[f32], r: usize) -> Vec<f32> {
    let n = line.len();
    let mut out = vec![0.0f32; n];
    if n == 0 {
        return out;
    }
    let mut deque: std::collections::VecDeque<usize> = std::collections::VecDeque::new();
    let mut next = 0usize;
    for (o, dst) in out.iter_mut().enumerate() {
        // Admit everything the window's right edge has reached.
        let hi = (o + r).min(n - 1);
        while next <= hi {
            while let Some(&b) = deque.back() {
                if line[b] >= line[next] {
                    deque.pop_back();
                } else {
                    break;
                }
            }
            deque.push_back(next);
            next += 1;
        }
        // Retire everything the left edge has passed.
        let lo = o.saturating_sub(r);
        while let Some(&f) = deque.front() {
            if f < lo {
                deque.pop_front();
            } else {
                break;
            }
        }
        *dst = line[*deque.front().expect("window is never empty")];
    }
    out
}

/// Minimum over a `(2r+1)²` window — the dark channel's patch operator.
///
/// Separable, because a rectangular window's minimum is the minimum of per-row minima.
///
/// # Panics
/// Panics if `plane.len() != w * h`.
pub fn min_filter(plane: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
    assert_eq!(plane.len(), w * h, "plane size mismatch");
    if r == 0 || w == 0 || h == 0 {
        return plane.to_vec();
    }

    // Horizontal: rows are contiguous and independent.
    let mut rows = vec![0.0f32; w * h];
    rows.par_chunks_exact_mut(w)
        .zip(plane.par_chunks_exact(w))
        .for_each(|(orow, srow)| orow.copy_from_slice(&min_1d(srow, r)));

    // Vertical: gather each column into scratch, reduce, scatter back. Unlike `box_blur`'s
    // vertical pass there is no running state to carry, so columns are independent and rayon
    // splits them directly; the gather is the price of reusing the same 1-D routine.
    let mut out = vec![0.0f32; w * h];
    let cols: Vec<Vec<f32>> = (0..w)
        .into_par_iter()
        .map(|x| {
            let column: Vec<f32> = (0..h).map(|y| rows[y * w + x]).collect();
            min_1d(&column, r)
        })
        .collect();
    for (x, column) in cols.iter().enumerate() {
        for (y, &v) in column.iter().enumerate() {
            out[y * w + x] = v;
        }
    }
    out
}

/// A Sobel gradient field: the per-pixel derivative in x and y, and its magnitude.
///
/// ⚠⚠ **The direction is the point, not a bonus.** `crates/app/src/visualize.rs`'s `edge_view` is
/// already a high-pass over luminance and cannot be used for line detection, because
/// `|luma − blur|` says *how much* an edge there is and nothing about which way it runs — so a
/// near-vertical filter built on it would have to accumulate every edge in the frame and throw
/// most of them away. It is also in the app lib, where `pipeline` cannot reach it. This is a
/// different operator, not a duplicate.
///
/// ⚠ Edges **replicate**, the convention [`box_blur`] and [`gaussian_blur`] already follow here,
/// so a constant plane comes back with exactly zero gradient rather than a frame-wide border
/// artifact.
#[derive(Debug, Clone, Default)]
pub struct Gradient {
    pub gx: Vec<f32>,
    pub gy: Vec<f32>,
    pub mag: Vec<f32>,
}

/// The Sobel gradient of a single-channel plane.
///
/// ⚠ A size mismatch returns an empty field rather than panicking, unlike the blurs above: the
/// only caller is a detector whose honest answer for an unusable plane is "no lines found".
pub fn sobel(plane: &[f32], w: usize, h: usize) -> Gradient {
    if w == 0 || h == 0 || plane.len() != w * h {
        return Gradient::default();
    }
    let at = |x: i64, y: i64| -> f32 {
        let xi = x.clamp(0, w as i64 - 1) as usize;
        let yi = y.clamp(0, h as i64 - 1) as usize;
        plane[yi * w + xi]
    };
    let mut g = Gradient {
        gx: vec![0.0; w * h],
        gy: vec![0.0; w * h],
        mag: vec![0.0; w * h],
    };
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            let (tl, tc, tr) = (at(x - 1, y - 1), at(x, y - 1), at(x + 1, y - 1));
            let (ml, mr) = (at(x - 1, y), at(x + 1, y));
            let (bl, bc, br) = (at(x - 1, y + 1), at(x, y + 1), at(x + 1, y + 1));
            let gx = (tr + 2.0 * mr + br) - (tl + 2.0 * ml + bl);
            let gy = (bl + 2.0 * bc + br) - (tl + 2.0 * tc + tr);
            let i = y as usize * w + x as usize;
            g.gx[i] = gx;
            g.gy[i] = gy;
            g.mag[i] = (gx * gx + gy * gy).sqrt();
        }
    }
    g
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
    fn wide_blur_radius_solves_the_three_pass_variance() {
        // Three box passes of radius r have variance 3*((2r+1)^2 - 1)/12 = ((2r+1)^2 - 1)/4,
        // so matching a target sigma means (2r+1)^2 = 1 + 4*sigma^2.
        assert_eq!(wide_blur_radius(40.0), 40); // (sqrt(6401) - 1)/2 = 39.5 -> 40
        assert_eq!(wide_blur_reach(40.0), 120);
        // Below sigma ~= 0.87 an integer box radius cannot represent the blur at all, so
        // there is deliberately no `.max(1)`: clamping up would render sigma 0.1 as an
        // effective sigma of 0.82, eight times too wide.
        assert_eq!(wide_blur_radius(0.1), 0);
        assert_eq!(wide_blur_radius(0.0), 0);
        assert_eq!(wide_blur_radius(-3.0), 0);
        assert_eq!(wide_blur_radius(f32::NAN), 0);
    }

    #[test]
    fn wide_blur_below_the_representable_sigma_is_identity() {
        let p = vec![1.0, 2.0, 3.0, 4.0];
        assert_eq!(wide_blur(&p, 2, 2, 0.1), p);
        assert_eq!(wide_blur(&p, 2, 2, 0.0), p);
    }

    #[test]
    fn wide_blur_constant_plane_stays_constant() {
        // Inherited from box_blur: the *window* clamps at the border, not the samples.
        let p = vec![0.375f32; 41 * 33];
        let out = wide_blur(&p, 41, 33, 4.0);
        for (i, v) in out.iter().enumerate() {
            assert!((v - 0.375).abs() < 1e-6, "sample {i} = {v}");
        }
    }

    #[test]
    fn wide_blur_matches_the_requested_sigma() {
        // A separable filter's centre row is proportional to its 1D kernel, so normalizing
        // it and taking the second moment recovers the realised sigma directly. This is what
        // makes "approximates a Gaussian" a measured claim rather than a comment.
        const N: usize = 121;
        const SIGMA: f32 = 8.0;
        let mut p = vec![0.0f32; N * N];
        p[(N / 2) * N + N / 2] = 1.0;
        let out = wide_blur(&p, N, N, SIGMA);

        let row = &out[(N / 2) * N..(N / 2 + 1) * N];
        let sum: f32 = row.iter().sum();
        let var: f32 = row
            .iter()
            .enumerate()
            .map(|(x, &w)| {
                let d = x as f32 - (N / 2) as f32;
                w / sum * d * d
            })
            .sum();
        let realised = var.sqrt();

        // Compare against the sigma the *rounded* radius actually encodes, not the requested
        // one — sqrt(((2r+1)^2 - 1)/4). At sigma 8 the radius rounds 7.52 up to 8, which is
        // a realised sigma of 8.49; asserting 8.0 to within a few percent would fail for a
        // correct implementation.
        let r = wide_blur_radius(SIGMA);
        let modelled = (((2 * r + 1).pow(2) - 1) as f32 / 4.0).sqrt();
        assert!(
            (realised - modelled).abs() / modelled < 0.03,
            "realised sigma {realised} vs modelled {modelled}"
        );
        // Rounding the radius to an integer is the only source of error, and it is bounded.
        assert!(
            (realised - SIGMA).abs() / SIGMA < 0.10,
            "realised sigma {realised} is too far from the requested {SIGMA}"
        );
    }

    #[test]
    fn wide_blur_impulse_response_is_a_symmetric_bell() {
        // Rules out an off-by-one in the iteration, which a variance check alone would miss.
        const N: usize = 81;
        let mut p = vec![0.0f32; N * N];
        p[(N / 2) * N + N / 2] = 1.0;
        let out = wide_blur(&p, N, N, 6.0);
        let c = N / 2;
        let peak = out[c * N + c];
        for y in 0..N {
            for x in 0..N {
                if (x, y) != (c, c) {
                    assert!(out[y * N + x] < peak, "({x},{y}) exceeds the centre");
                }
                assert!((out[y * N + x] - out[y * N + (N - 1 - x)]).abs() < 1e-6);
                assert!((out[y * N + x] - out[(N - 1 - y) * N + x]).abs() < 1e-6);
            }
        }
        // Monotone away from the centre along the centre row.
        for x in c..N - 1 {
            assert!(
                out[c * N + x] >= out[c * N + x + 1] - 1e-7,
                "not monotone at {x}"
            );
        }
    }

    #[test]
    fn three_box_passes_are_much_closer_to_a_gaussian_than_one() {
        // Pins the *number* of iterations, not just the radius. One box pass is a
        // flat-topped kernel with hard shoulders; three passes are a quadratic B-spline,
        // within a few percent of a Gaussian everywhere.
        //
        // A direct peak-height comparison against a Gaussian at the *requested* sigma would
        // be wrong — the rounded radius makes wide_blur's realised sigma 6% wider, which is
        // a 12% lower peak in 2D. Comparing normalized profiles against the same reference
        // sidesteps that entirely.
        const N: usize = 121;
        const SIGMA: f32 = 8.0;
        let mut p = vec![0.0f32; N * N];
        p[(N / 2) * N + N / 2] = 1.0;

        let gauss = gaussian_blur(&p, N, N, SIGMA);
        let wide = wide_blur(&p, N, N, SIGMA);
        let once = box_blur(&p, N, N, wide_blur_radius(SIGMA));

        // A separable filter's centre row is proportional to its 1D kernel.
        let profile = |v: &[f32]| {
            let row = &v[(N / 2) * N..(N / 2 + 1) * N];
            let s: f32 = row.iter().sum();
            row.iter().map(|w| w / s).collect::<Vec<f32>>()
        };
        let g = profile(&gauss);
        let dev = |a: &[f32]| {
            a.iter()
                .zip(&g)
                .map(|(x, y)| (x - y).abs())
                .fold(0.0f32, f32::max)
        };
        let d1 = dev(&profile(&once));
        let d3 = dev(&profile(&wide));
        assert!(d3 * 2.0 < d1, "three passes {d3} vs one pass {d1}");

        let peak = g.iter().fold(0.0f32, |a, &b| a.max(b));
        assert!(
            d3 < 0.15 * peak,
            "three passes deviate {d3} from a Gaussian whose peak is {peak}"
        );
    }

    /// Straightforward O(n·r) sliding minimum, for the deque implementation to be checked
    /// against. Deliberately dumb: its only job is to be obviously correct.
    fn naive_min(plane: &[f32], w: usize, h: usize, r: usize) -> Vec<f32> {
        let mut out = vec![0.0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                let mut m = f32::INFINITY;
                for yy in y.saturating_sub(r)..=(y + r).min(h - 1) {
                    for xx in x.saturating_sub(r)..=(x + r).min(w - 1) {
                        m = m.min(plane[yy * w + xx]);
                    }
                }
                out[y * w + x] = m;
            }
        }
        out
    }

    #[test]
    fn min_filter_radius_zero_is_identity() {
        let p = vec![3.0, 1.0, 4.0, 1.0];
        assert_eq!(min_filter(&p, 2, 2, 0), p);
    }

    #[test]
    fn min_filter_constant_plane_stays_constant() {
        // The window clamps at the border rather than reading outside, so no edge darkens.
        let p = vec![0.375f32; 9 * 7];
        let out = min_filter(&p, 9, 7, 2);
        for (i, v) in out.iter().enumerate() {
            assert!((v - 0.375).abs() < 1e-6, "sample {i} = {v}");
        }
    }

    #[test]
    fn min_1d_known_case() {
        // r = 1, so each output is the min of itself and its immediate neighbours, with the
        // window clamped at both ends.
        let line = [5.0, 3.0, 8.0, 1.0, 9.0];
        assert_eq!(min_1d(&line, 1), vec![3.0, 3.0, 1.0, 1.0, 1.0]);
        // A radius wider than the line gives the global minimum everywhere.
        assert_eq!(min_1d(&line, 99), vec![1.0; 5]);
    }

    #[test]
    fn min_filter_known_two_dimensional_case() {
        // A single low value in the middle of a 3x3 spreads to the whole plane at r = 1.
        let mut p = vec![1.0f32; 9];
        p[4] = 0.25;
        assert_eq!(min_filter(&p, 3, 3, 1), vec![0.25; 9]);
        // At a corner it reaches only the cells within the clamped window.
        let mut q = vec![1.0f32; 9];
        q[0] = 0.5;
        let out = min_filter(&q, 3, 3, 1);
        assert_eq!(out[8], 1.0, "the far corner must be out of reach");
        assert_eq!(out[4], 0.5, "the centre is within r=1 of the corner");
    }

    #[test]
    fn min_filter_matches_a_naive_reference() {
        // ⚠ The decisive test. A monotonic deque is easy to get subtly wrong — an off-by-one
        // in when the window opens or closes — in ways every other test here still passes.
        let (w, h) = (37usize, 23usize);
        let mut state: u32 = 0x1234_5678;
        let p: Vec<f32> = (0..w * h)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                state as f32 / u32::MAX as f32
            })
            .collect();
        for r in [1usize, 2, 3, 7, 20, 50] {
            let got = min_filter(&p, w, h, r);
            let want = naive_min(&p, w, h, r);
            assert_eq!(got, want, "radius {r}");
        }
    }

    /// The self-guided filter exactly as `noise_reduction` computed it before this was
    /// generalised — kept here as the reference the shared version must reproduce.
    fn legacy_self_filter(p: &[f32], w: usize, h: usize, r: usize, eps: f32) -> Vec<f32> {
        let squared: Vec<f32> = p.iter().map(|v| v * v).collect();
        let mean = box_blur(p, w, h, r);
        let mean_sq = box_blur(&squared, w, h, r);
        let mut slope = Vec::with_capacity(p.len());
        let mut offset = Vec::with_capacity(p.len());
        for (&m, &msq) in mean.iter().zip(mean_sq.iter()) {
            let var = (msq - m * m).max(0.0);
            let s = var / (var + eps);
            slope.push(s);
            offset.push((1.0 - s) * m);
        }
        let mean_slope = box_blur(&slope, w, h, r);
        let mean_offset = box_blur(&offset, w, h, r);
        mean_slope
            .iter()
            .zip(mean_offset.iter())
            .zip(p.iter())
            .map(|((&ms, &mo), &v)| ms * v + mo)
            .collect()
    }

    fn noisy_plane(w: usize, h: usize) -> Vec<f32> {
        let mut state: u32 = 0x9E37_79B9;
        (0..w * h)
            .map(|i| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                // Structure plus noise, so `slope` actually varies across the plane.
                let base = if (i % w) < w / 2 { 0.2 } else { 0.7 };
                base + (state as f32 / u32::MAX as f32 - 0.5) * 0.05
            })
            .collect()
    }

    #[test]
    fn guided_filter_reproduces_the_self_guided_form() {
        // ⚠ The acceptance test for the refactor. With `p == guide` the general form reduces
        // algebraically to the old one — but `offset` is now `mean_p - slope*mean_I` where it
        // used to be `(1 - slope)*mean`, which can differ in the last f32 ulp. A tight
        // tolerance rather than equality says exactly that, instead of pretending.
        let (w, h) = (48usize, 32usize);
        let p = noisy_plane(w, h);
        for (r, eps) in [(3usize, 1e-3f32), (6, 1e-4), (12, 1e-5)] {
            let want = legacy_self_filter(&p, w, h, r, eps);
            let got = guided_filter(&p, &p, w, h, r, eps);
            for (i, (a, b)) in got.iter().zip(&want).enumerate() {
                assert!(
                    (a - b).abs() < 1e-6,
                    "r={r} eps={eps} sample {i}: {a} vs {b}"
                );
            }
        }
    }

    /// The **general** form, frozen verbatim as it stood before the self-guided shortcut.
    ///
    /// ⚠⚠ It must NEVER be updated to match `guided_filter`. If the two disagree, that is the
    /// finding — the shortcut's whole claim is that the arithmetic which survives is the same
    /// arithmetic in the same order. The `rasterize_in` frozen copy (M8.3i) is the precedent, and
    /// its first draft's mistake was to *call* the function under test, which compares it against
    /// itself and passes forever.
    fn guided_filter_general_reference(
        p: &[f32],
        guide: &[f32],
        w: usize,
        h: usize,
        r: usize,
        eps: f32,
    ) -> Vec<f32> {
        let guide_sq: Vec<f32> = guide.iter().map(|v| v * v).collect();
        let cross: Vec<f32> = guide.iter().zip(p).map(|(g, v)| g * v).collect();

        let mean_guide = box_blur(guide, w, h, r);
        let mean_p = box_blur(p, w, h, r);
        let mean_guide_sq = box_blur(&guide_sq, w, h, r);
        let mean_cross = box_blur(&cross, w, h, r);

        let mut slope = Vec::with_capacity(p.len());
        let mut offset = Vec::with_capacity(p.len());
        for (((&mg, &mp), &mgsq), &mc) in mean_guide
            .iter()
            .zip(mean_p.iter())
            .zip(mean_guide_sq.iter())
            .zip(mean_cross.iter())
        {
            let var = (mgsq - mg * mg).max(0.0);
            let cov = mc - mg * mp;
            let s = cov / (var + eps);
            slope.push(s);
            offset.push(mp - s * mg);
        }

        let mean_slope = box_blur(&slope, w, h, r);
        let mean_offset = box_blur(&offset, w, h, r);
        mean_slope
            .iter()
            .zip(mean_offset.iter())
            .zip(guide.iter())
            .map(|((&ms, &mo), &g)| ms * g + mo)
            .collect()
    }

    #[test]
    fn the_self_guided_shortcut_is_bit_identical() {
        // ⚠ The fixture needs real structure at the filter's own scale, or every implementation
        // agrees: on a flat plane the variance is 0 everywhere and `slope` is 0 whatever the
        // statistics say. `noisy_plane` carries a step plus noise for exactly that reason.
        let (w, h) = (48usize, 32usize);
        let p = noisy_plane(w, h);
        for (r, eps) in [
            (1usize, 2e-4f32),
            (2, 2e-4),
            (3, 1e-3),
            (6, 1e-4),
            (12, 1e-5),
        ] {
            let want = guided_filter_general_reference(&p, &p, w, h, r, eps);
            let got = guided_filter(&p, &p, w, h, r, eps);
            assert_eq!(
                want, got,
                "the self-guided shortcut must be bit-identical at r={r} eps={eps}"
            );
        }
    }

    #[test]
    fn a_cross_guided_call_still_takes_the_general_path() {
        // ⚠⚠ The other half, and the one a self-guided test structurally cannot see: dehaze
        // filters the transmission map guided by luma, so `p != guide` and all four statistic
        // blurs are genuinely needed. A shortcut that fired unconditionally would return
        // `mean_guide` where `mean_p` belongs — a smooth, plausible transmission map that is
        // simply the wrong one.
        let (w, h) = (48usize, 32usize);
        let guide = noisy_plane(w, h);
        let blurry: Vec<f32> = guide.iter().map(|v| 0.4 + v * 0.2).collect();
        let want = guided_filter_general_reference(&blurry, &guide, w, h, 4, 1e-6);
        let got = guided_filter(&blurry, &guide, w, h, 4, 1e-6);
        assert_eq!(
            want, got,
            "a cross-guided call must be untouched by the shortcut"
        );
    }

    #[test]
    fn guided_filter_sharpens_a_blurry_edge_onto_the_guides() {
        // The property that makes it *cross*-guided, and precisely what dehaze needs: a
        // blocky, smeared transmission map snapped back onto the image's real depth edges.
        //
        // ⚠ Note what is *not* asserted. An earlier version of this test fed in a nearly flat
        // `p` and expected it to acquire the guide's step. It does not, and should not: with
        // `p` flat the covariance is ~0, so `slope` is ~0 and the output is just `p`'s local
        // mean either side. The filter transfers *structure `p` already has*, sharpened to
        // the guide's edges — it does not paint the guide onto an unrelated signal.
        let (w, h) = (32usize, 16usize);
        let step = w / 2;
        let guide: Vec<f32> = (0..w * h)
            .map(|i| if (i % w) < step { 0.1 } else { 0.9 })
            .collect();
        // The same edge, smeared over ±6 px — a blurry version of the guide's step.
        let blurry: Vec<f32> = (0..w * h)
            .map(|i| {
                let x = (i % w) as f32 - step as f32;
                0.5 + 0.3 * (x / 6.0).clamp(-1.0, 1.0)
            })
            .collect();
        let out = guided_filter(&blurry, &guide, w, h, 4, 1e-6);

        let row = 8 * w;
        let got_jump = out[row + step] - out[row + step - 1];
        let src_jump = blurry[row + step] - blurry[row + step - 1];
        assert!(
            got_jump > src_jump * 3.0,
            "the edge was not sharpened: {src_jump} -> {got_jump}"
        );
        // And it stays monotone — a sharpened edge, not an overshoot ringing either side.
        assert!(out[row + 2] < out[row + w - 3]);
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

    /// A vertical edge: the gradient points across it, so `gx` is large and `gy` is ~0.
    #[test]
    fn a_vertical_edge_gives_a_horizontal_gradient() {
        let (w, h) = (16, 16);
        let mut p = vec![0.0f32; w * h];
        for y in 0..h {
            for x in 8..w {
                p[y * w + x] = 1.0;
            }
        }
        let g = sobel(&p, w, h);
        let i = 8 * w + 8;
        assert!(g.mag[i] > 0.5, "no magnitude on the edge: {}", g.mag[i]);
        assert!(
            g.gx[i].abs() > 4.0 * g.gy[i].abs().max(1e-6),
            "gx {} should dominate gy {}",
            g.gx[i],
            g.gy[i]
        );
    }

    /// ⚠ The mirror case, and it is what makes the test above mean something: a test that only
    /// checked a vertical edge would pass against an implementation that reported a horizontal
    /// gradient for everything.
    #[test]
    fn a_horizontal_edge_gives_a_vertical_gradient() {
        let (w, h) = (16, 16);
        let mut p = vec![0.0f32; w * h];
        for y in 8..h {
            for x in 0..w {
                p[y * w + x] = 1.0;
            }
        }
        let g = sobel(&p, w, h);
        let i = 8 * w + 8;
        assert!(g.mag[i] > 0.5, "no magnitude on the edge: {}", g.mag[i]);
        assert!(
            g.gy[i].abs() > 4.0 * g.gx[i].abs().max(1e-6),
            "gy {} should dominate gx {}",
            g.gy[i],
            g.gx[i]
        );
    }

    /// A flat field has no edges anywhere, or every photograph would be full of them.
    #[test]
    fn a_flat_field_has_no_gradient() {
        let g = sobel(&vec![0.42; 32 * 32], 32, 32);
        let max = g.mag.iter().copied().fold(0.0f32, f32::max);
        assert!(max < 1e-5, "a flat field had a gradient: {max}");
    }

    /// Degenerate sizes must return empty rather than panic — the guard `downscale_to` and
    /// `map_pixels_xy` each needed.
    #[test]
    fn sobel_handles_the_degenerate_sizes() {
        assert!(sobel(&[], 0, 0).mag.is_empty());
        assert!(sobel(&[1.0], 1, 1).mag.len() == 1);
    }
}
