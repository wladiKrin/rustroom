//! A box blur in two shapes, chosen by radius — the one genuinely new GPU primitive M8.3n needed.
//!
//! ⚠⚠ **The obvious shader LOSES to the CPU at Clarity's radius, and that is what shapes this
//! module.** `spatial::box_blur` keeps a running sum, so it is **O(1) in the radius** — 4.68 ms at
//! r=40 against 4.63 at r=2. A GPU thread has no running sum to keep, so a naive gather is O(r):
//! Clarity's 81-tap window x 2 passes x 3 iterations is 486 taps per pixel against the CPU's five.
//!
//! Measured on an Apple M2 at 3200x2136, three box passes, timed against a zero-pass run so the
//! subtraction isolates the blur from the round trip (`box_probe`):
//!
//! | r | 1 | 2 | 3 | 4 | 6 | **8** | 10 | 16 | 24 | 40 |
//! |---|---|---|---|---|---|---|---|---|---|---|
//! | naive | 4.06 | 4.96 | 6.41 | 7.57 | 10.29 | 11.83 | 12.71 | 21.89 | 29.19 | 46.89 |
//! | running | 11.36 | 11.17 | 11.30 | 11.32 | 17.40 | **10.20** | 11.52 | 11.70 | 12.35 | 11.64 |
//!
//! The running sum is **flat in the radius**, as O(1) predicts; the naive one is linear. ⚠ The
//! running row's 17.40 at r=6 is noise, not a bump — it sits above both neighbours in a series
//! otherwise flat to within 2 ms across a forty-fold change in radius, and a running sum admits no
//! r-dependence by construction.
//!
//! ⚠ What the running sum costs is **coalescing**: on the horizontal pass adjacent threads walk
//! adjacent *rows*, so their reads sit `w` floats apart. That is the ~11 ms floor visible at every
//! radius, and it is why the naive shader wins below the threshold despite doing more arithmetic.
//! The vertical pass has the opposite property, which is exactly why `blur_cols` advances every
//! column together rather than walking one at a time.

/// Where the two cross, **swept rather than guessed**.
///
/// ⚠⚠ Two constants in this crate were reasoned about instead of swept and both were badly wrong:
/// `MIN_GPU_BOX_PX` was low by ~8x, and `MAX_TILE_DABS` was sized by a *limit* rather than by the
/// data and made the whole brush rasteriser 5x slower than the CPU it replaced.
///
/// The sweep above crosses between r=6 and r=8. The four radii that actually occur are far from
/// it in both directions — Texture is r=1 (fit) or r=2 (native), Clarity is r=10 (fit) or r=40
/// (native) — so the exact value is not load-bearing; only the side of it each lands on is.
pub(crate) const RUNNING_SUM_MIN_RADIUS: usize = 7;

/// Which body a radius gets.
pub(crate) fn use_running_sum(r: usize) -> bool {
    r >= RUNNING_SUM_MIN_RADIUS
}

/// Emit a separable box-blur pair: `name_h` blurs `src` into `tmp`, `name_v` blurs `tmp` into
/// `dst`. Passing the same plane for `src` and `dst` blurs it in place, which is what lets a
/// statistic be computed without a destination of its own.
///
/// ⚠⚠ **Both bodies implement `box_blur`'s border rule: the WINDOW shrinks and the sum is divided
/// by the count actually inside it.** That is not `gaussian_blur`'s rule, which replicates the
/// edge sample — the two give different answers at a border, and only the first keeps a constant
/// plane exactly constant. It is also why this cannot reuse `sharpen.rs`'s `blur_h`.
///
/// ⚠ The radius is substituted as a **literal**, so the caller's program cache key distinguishes
/// one radius from another and the compiler sees a constant trip count.
///
/// The surrounding shader must define `params.w`, `params.h`, `params.pixels` and `index(id)`.
pub(crate) fn box_pass(
    name_h: &str,
    name_v: &str,
    src: &str,
    dst: &str,
    tmp: &str,
    r: usize,
) -> String {
    box_pass_forcing(name_h, name_v, src, dst, tmp, r, use_running_sum(r))
}

/// [`box_pass`] with the variant chosen explicitly.
///
/// ⚠ Exists so `box_probe` can time **both** bodies at the same radius, which is what the sweep in
/// this module's header is made of. Production never calls it: the threshold is not a parameter,
/// it is the measurement's conclusion.
pub(crate) fn box_pass_forcing(
    name_h: &str,
    name_v: &str,
    src: &str,
    dst: &str,
    tmp: &str,
    r: usize,
    running_sum: bool,
) -> String {
    if running_sum {
        running(name_h, name_v, src, dst, tmp, r)
    } else {
        naive(name_h, name_v, src, dst, tmp, r)
    }
}

/// One thread per **pixel**, re-summing the window.
///
/// ⚠ Not bit-identical to the CPU, and it is the **more** accurate of the two: re-summing five
/// samples is a shorter accumulation than a running sum carried across 3200 pixels, so the CPU's
/// is the one that drifts. The divergence is bounded by the CPU's own accumulated error.
fn naive(name_h: &str, name_v: &str, src: &str, dst: &str, tmp: &str, r: usize) -> String {
    format!(
        r#"
@compute @workgroup_size(64)
fn {name_h}(@builtin(global_invocation_id) id: vec3<u32>) {{
    let i = index(id);
    if (i >= params.pixels) {{ return; }}
    let y = i / params.w;
    let x = i - y * params.w;
    let lo = select(0u, x - {r}u, x >= {r}u);
    let hi = min(x + {r}u + 1u, params.w);
    var acc = 0.0;
    for (var t = lo; t < hi; t = t + 1u) {{ acc = acc + {src}[y * params.w + t]; }}
    {tmp}[i] = acc / f32(hi - lo);
}}

@compute @workgroup_size(64)
fn {name_v}(@builtin(global_invocation_id) id: vec3<u32>) {{
    let i = index(id);
    if (i >= params.pixels) {{ return; }}
    let y = i / params.w;
    let x = i - y * params.w;
    let lo = select(0u, y - {r}u, y >= {r}u);
    let hi = min(y + {r}u + 1u, params.h);
    var acc = 0.0;
    for (var t = lo; t < hi; t = t + 1u) {{ acc = acc + {tmp}[t * params.w + x]; }}
    {dst}[i] = acc / f32(hi - lo);
}}
"#
    )
}

/// One thread per **line**, carrying a running sum — a transliteration of `spatial::blur_rows`
/// and `blur_cols`.
///
/// ⚠⚠ Same priming (sum `[0, min(r+1, w))` ascending), same slide (one add, one subtract), same
/// divisor (the live window count). **So this can be bit-identical**, which nothing else ported in
/// this crate can claim since M8.3m found FMA contraction — because nothing else is free of
/// multiply-adds.
///
/// ⚠ Indexed by `id.x` alone: the caller dispatches `(groups, 1)` workgroups for a line pass, so
/// `id.y` is always 0. That holds up to 65535 x 64 = 4.2 M lines, which no photograph approaches.
fn running(name_h: &str, name_v: &str, src: &str, dst: &str, tmp: &str, r: usize) -> String {
    format!(
        r#"
@compute @workgroup_size(64)
fn {name_h}(@builtin(global_invocation_id) id: vec3<u32>) {{
    let y = id.x;
    if (y >= params.h) {{ return; }}
    let row = y * params.w;
    var hi = min({r}u + 1u, params.w);
    var lo = 0u;
    var sum = 0.0;
    for (var t = 0u; t < hi; t = t + 1u) {{ sum = sum + {src}[row + t]; }}
    for (var x = 0u; x < params.w; x = x + 1u) {{
        {tmp}[row + x] = sum / f32(hi - lo);
        if (x + {r}u + 1u < params.w) {{ sum = sum + {src}[row + x + {r}u + 1u]; hi = hi + 1u; }}
        if (x >= {r}u) {{ sum = sum - {src}[row + x - {r}u]; lo = lo + 1u; }}
    }}
}}

@compute @workgroup_size(64)
fn {name_v}(@builtin(global_invocation_id) id: vec3<u32>) {{
    let x = id.x;
    if (x >= params.w) {{ return; }}
    var hi = min({r}u + 1u, params.h);
    var lo = 0u;
    var sum = 0.0;
    for (var t = 0u; t < hi; t = t + 1u) {{ sum = sum + {tmp}[t * params.w + x]; }}
    for (var y = 0u; y < params.h; y = y + 1u) {{
        {dst}[y * params.w + x] = sum / f32(hi - lo);
        if (y + {r}u + 1u < params.h) {{ sum = sum + {tmp}[(y + {r}u + 1u) * params.w + x]; hi = hi + 1u; }}
        if (y >= {r}u) {{ sum = sum - {tmp}[(y - {r}u) * params.w + x]; lo = lo + 1u; }}
    }}
}}
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⚠⚠ The threshold as a **decision**, so "which shader does Clarity get" is pinned rather
    /// than believed. The four radii are the ones that actually occur: `texture_guided_radius` at
    /// fit and native, and `wide_blur_radius(CLARITY_SIGMA * scale)` at fit and native.
    #[test]
    fn each_real_radius_selects_the_shader_the_sweep_chose() {
        assert!(!use_running_sum(1), "Texture at a fit rung");
        assert!(!use_running_sum(2), "Texture at a native rung");
        assert!(use_running_sum(10), "Clarity at a fit rung");
        assert!(use_running_sum(40), "Clarity at a native rung");
    }

    /// ⚠ The two bodies are genuinely different code, not one with a flag — a test that only
    /// checked the boolean could pass against an emitter that ignored it.
    #[test]
    fn the_two_bodies_really_differ() {
        let n = box_pass("h", "v", "a", "a", "b", 2);
        let r = box_pass("h", "v", "a", "a", "b", 40);
        assert!(n.contains("index(id)"), "the naive body is per pixel");
        assert!(r.contains("let y = id.x;"), "the running body is per line");
        assert_ne!(n, r);
    }
}
