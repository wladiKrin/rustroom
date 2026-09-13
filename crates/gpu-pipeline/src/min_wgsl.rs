//! A sliding-window minimum — the dark channel's primitive, and the one place M8.3n's trick fails.
//!
//! ⚠⚠ **A minimum is NOT invertible, so there is no running form.** `box_blur` slides by adding
//! the entering sample and subtracting the leaving one, which is what makes it O(1) in the radius
//! and what `box_wgsl`'s running body transliterates. A minimum cannot subtract: once the running
//! minimum leaves the window there is nothing to recover it from. So the only GPU shape is the
//! **naive O(r) gather**, and this is affordable purely because the radius is small —
//! `PATCH_RADIUS_FULL = 20` scales to **r=10**, measured at 4.49 ms, where Clarity's r=40 cost
//! 46.9 and lost to the CPU outright.
//!
//! ⚠ The CPU's `min_1d` uses a monotonic deque, which IS O(1) amortised — so unlike the box blur
//! this is a case where the CPU keeps an advantage the GPU cannot take. It is simply small enough
//! not to matter here.

/// Emit a separable minimum pair: `name_h` reduces `src` into `tmp`, `name_v` reduces `tmp` into
/// `dst`. Separable because `min` over a rectangle is `min` of row-`min`s.
///
/// ⚠⚠ **The WINDOW clamps, the samples do not** — `box_blur`'s border rule, not
/// `gaussian_blur`'s. Near an edge the window shrinks; it does not repeat the edge sample. That is
/// precisely why `spatial::min_1d` uses a deque rather than van Herk / Gil-Werman, whose block
/// decomposition cannot express a shrinking window — its own doc says so.
///
/// ⚠ For a minimum the two rules happen to agree (repeating a sample already in the window cannot
/// change a minimum), so this is **not** observable here the way it is for a box blur. Written to
/// the same rule anyway, because a reader comparing the two shaders must not have to work that
/// out, and because the equivalence test is against `min_filter`, which shrinks.
///
/// The surrounding shader must define `params.w`, `params.h`, `params.pixels` and `index(id)`.
pub(crate) fn min_pass(
    name_h: &str,
    name_v: &str,
    src: &str,
    dst: &str,
    tmp: &str,
    r: usize,
) -> String {
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
    var m = {src}[y * params.w + lo];
    for (var t = lo + 1u; t < hi; t = t + 1u) {{ m = min(m, {src}[y * params.w + t]); }}
    {tmp}[i] = m;
}}

@compute @workgroup_size(64)
fn {name_v}(@builtin(global_invocation_id) id: vec3<u32>) {{
    let i = index(id);
    if (i >= params.pixels) {{ return; }}
    let y = i / params.w;
    let x = i - y * params.w;
    let lo = select(0u, y - {r}u, y >= {r}u);
    let hi = min(y + {r}u + 1u, params.h);
    var m = {tmp}[lo * params.w + x];
    for (var t = lo + 1u; t < hi; t = t + 1u) {{ m = min(m, {tmp}[t * params.w + x]); }}
    {dst}[i] = m;
}}
"#
    )
}
