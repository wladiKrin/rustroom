//! The WGSL a masked group of adjustments runs: seed, coverage, blend.
//!
//! ⚠⚠ **Four dispatch groups in one compute pass, and the ORDER is the data dependency rather
//! than a convention.** `seed` copies the base into three planes; `coverage` reads the *base*
//! image; the sub-stages then turn that image into the adjusted one in place; `blend` mixes the
//! two by the coverage plane. Running `coverage` after the sub-stages would key the mask on its
//! own output — lifting the shadows moves them out of the band that selected them — which is
//! exactly what `Composite::at`'s `base_px` parameter exists to prevent on the CPU, and it
//! produces a plausible photograph rather than an error.

/// The constants every kernel needs, **substituted from their Rust definitions** rather than
/// typed, so the two cannot drift.
///
/// ⚠ The rule `luma_wgsl` and `saturation` already follow. `f32::EPSILON` in particular is a
/// number nobody should be re-typing into a shader.
pub(crate) fn constants() -> String {
    format!(
        "const F32_EPSILON: f32 = {:?};\n\
         const RADIAL_FEATHER_EPS: f32 = {:?};\n\
         const LUM_FEATHER_EPS: f32 = {:?};\n\
         const LUM_FEATHER_SCALE: f32 = {:?};\n\
         const COL_FEATHER_EPS: f32 = {:?};\n\
         const COL_FEATHER_SCALE: f32 = {:?};\n\
         const COL_CHROMA_FADE: f32 = {:?};\n",
        f32::EPSILON,
        edit_model::RadialGradient::FEATHER_EPS,
        edit_model::LuminanceRange::FEATHER_EPS,
        edit_model::LuminanceRange::FEATHER_SCALE,
        edit_model::ColourRange::FEATHER_EPS,
        edit_model::ColourRange::FEATHER_SCALE,
        edit_model::ColourRange::CHROMA_FADE,
    )
}

/// The bindings and the index helper every entry point shares.
///
/// ⚠ `base_r/g/b` and `cov` are the chain's scratch planes, and the stage keeps the **first four**
/// for itself. See `GpuMaskStage::scratch_planes` for why lending the rest onward is what keeps
/// `GpuChain`'s "nothing written here survives the call" contract intact.
pub(crate) const PRELUDE: &str = r#"
struct Params {
    pixels: u32,
    row_groups: u32,
    // ⚠ The width of the buffer being rendered, which under a region render is the REGION's
    // width, not the frame's. It is what turns a linear index back into an (x, y).
    w: u32,
    pad0: u32,
    // Where this buffer sits in the frame, and how big the frame is.
    //
    // ⚠⚠ Coverage is defined over the FRAME, never over the region. A region-relative index
    // slides the mask across the photograph as you pan — the note `MaskStage::apply` already
    // carries, and the bug the mask overlay shipped once.
    x0: u32,
    y0: u32,
    fw: u32,
    fh: u32,
    // The painted component's box, and its value everywhere outside it.
    box_x0: u32,
    box_y0: u32,
    box_w: u32,
    box_h: u32,
    outside: u32,
    invert: u32,
    pad1: u32,
    pad2: u32,
    // An analytic component's own numbers: `from.xy`/`to.xy` for a linear gradient,
    // `center.xy`/`radius.xy` for a radial one.
    //
    // ⚠ `aspect` is the FRAME's, `fw / fh`, exactly as `MaskStage::apply` derives it — a region's
    // own aspect is a different number and would skew a diagonal gradient's iso-lines.
    a0: f32,
    a1: f32,
    a2: f32,
    a3: f32,
    feather: f32,
    aspect: f32,
    pad4: f32,
    pad5: f32,
};
@group(0) @binding(0) var<storage, read_write> img: array<f32>;
@group(0) @binding(1) var<uniform> params: Params;
@group(0) @binding(2) var<storage, read_write> base_r: array<f32>;
@group(0) @binding(3) var<storage, read_write> base_g: array<f32>;
@group(0) @binding(4) var<storage, read_write> base_b: array<f32>;
@group(0) @binding(5) var<storage, read_write> cov: array<f32>;
@group(0) @binding(6) var<storage, read> packed: array<u32>;
"#;

/// The mask's clone, on the GPU.
///
/// ⚠⚠ **A dispatch, not `copy_buffer_to_buffer`.** The latter is an *encoder* operation and
/// cannot be recorded inside a compute pass. `GpuLensCorrectionStage` seeds its planes with a
/// shader for exactly this reason, and it is what lets the whole mask stay in one pass.
pub(crate) const SEED: &str = r#"
@compute @workgroup_size(64)
fn seed(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.y * (params.row_groups * 64u) + id.x;
    if (i >= params.pixels) { return; }
    let b = i * 3u;
    base_r[i] = img[b];
    base_g[i] = img[b + 1u];
    base_b[i] = img[b + 2u];
}
"#;

/// A painted component, read from an uploaded [`pipeline::brush::CoverageMap`].
///
/// ⚠⚠ **Mirrors `CoverageMap::at` exactly, box and `outside` value included.** That `outside`
/// field is load-bearing and its absence was a real bug found in use: an inverted brush covers the
/// whole frame *except* what was painted, so returning 0 outside the box confined the inversion to
/// a rectangle around the strokes, which grew as more was painted.
///
/// ⚠ Samples are packed four per `u32` because WGSL has no `u8`. At rung 3200 that is 6.5 MB
/// rather than 26 MB as `f32` — the transfer the spec's §5.5 priced at ~2 ms.
pub(crate) const PAINTED: &str = r#"
fn painted_at(fx: u32, fy: u32) -> f32 {
    if (fx < params.box_x0 || fy < params.box_y0
        || fx >= params.box_x0 + params.box_w || fy >= params.box_y0 + params.box_h) {
        return f32(params.outside) / 255.0;
    }
    let k = (fy - params.box_y0) * params.box_w + (fx - params.box_x0);
    let word = packed[k >> 2u];
    let byte = (word >> ((k & 3u) * 8u)) & 255u;
    return f32(byte) / 255.0;
}
"#;

/// The analytic shapes — transcribed **expression for expression** from
/// [`edit_model::LinearGradient::coverage`] and [`edit_model::RadialGradient::coverage`].
///
/// ⚠⚠ **`cov_radial` takes no aspect, and that is not an oversight.** Every sibling needs one —
/// a linear gradient projects onto an axis and `mask_tool::hit_test` measures a screen distance,
/// and the anisotropic scaling between normalised and screen space destroys perpendicularity and
/// distance respectively. It does **not** destroy axis-alignment: an axis-aligned ellipse maps to
/// an axis-aligned ellipse, so the expression below is already a correct on-screen ellipse.
/// `radial_coverage_is_independent_of_aspect` guards the CPU against someone "fixing" it, and
/// `the_radial_kernel_ignores_the_aspect_exactly_as_the_cpu_does` guards this.
pub(crate) const ANALYTIC: &str = r#"
fn cov_linear(u: f32, v: f32, aspect: f32) -> f32 {
    // ⚠ Work in units of frame HEIGHT, where both axes have the same pixel scale. A projection
    // needs a metric, and raw normalised space is anisotropic — without this a diagonal gradient's
    // iso-lines visibly skew. Same species of problem as the vignette's roundness.
    let ax = params.a0 * aspect;
    let ay = params.a1;
    let bx = params.a2 * aspect;
    let by = params.a3;
    let dx = bx - ax;
    let dy = by - ay;
    let len2 = dx * dx + dy * dy;
    // ⚠ Full coverage rather than NaN. The alternative reaches the blend and paints garbage.
    if (len2 <= F32_EPSILON) { return select(1.0, 0.0, params.invert == 1u); }
    let t = clamp((((u * aspect) - ax) * dx + (v - ay) * dy) / len2, 0.0, 1.0);
    return select(t, 1.0 - t, params.invert == 1u);
}

fn cov_radial(u: f32, v: f32) -> f32 {
    let rx = params.a2;
    let ry = params.a3;
    var c = 0.0;
    // ⚠ A degenerate ellipse honestly covers nothing. Unlike the linear guard, which chooses 1.0
    // only because its alternative was NaN, 0.0 is the true answer here.
    if (rx > F32_EPSILON && ry > F32_EPSILON) {
        let dx = (u - params.a0) / rx;
        let dy = (v - params.a1) / ry;
        let d = sqrt(dx * dx + dy * dy);
        let f = clamp(params.feather / 100.0, 0.0, 1.0);
        if (f <= RADIAL_FEATHER_EPS) {
            c = select(0.0, 1.0, d <= 1.0);
        } else {
            // ⚠ Smoothstep rather than a linear ramp: a linear ramp in radial distance leaves a
            // visible crease ring where the derivative jumps.
            let t = clamp((1.0 - d) / f, 0.0, 1.0);
            c = t * t * (3.0 - 2.0 * t);
        }
    }
    return select(c, 1.0 - c, params.invert == 1u);
}
"#;

/// The two **pixel-dependent** kinds: a luminance range and a colour range.
///
/// ⚠⚠ **These are what make the coverage dispatch's POSITION load-bearing.** They read the pixel,
/// so running them after the sub-stages would key the mask on its own output — lifting the shadows
/// moves them out of the band that selected them. For a painted or analytic component the position
/// provably cannot matter, which is why that sabotage was inert until now.
pub(crate) const RANGES: &str = r#"
fn cov_luminance(px: vec3<f32>) -> f32 {
    // ⚠⚠ `linear_to_srgb` clamps its input, and that IS the index clamp — the selector is clamped
    // while the pixel keeps its headroom. A specular highlight therefore selects at the top of the
    // scale and still carries every bit of its latitude into the adjustment. Clamping the *pixel*
    // would be the `ProfileHueSatMap` trap, which this project has met three times.
    let l = linear_to_srgb(prophoto_luminance(px.x, px.y, px.z)) * 100.0;
    // ⚠ Sorted rather than trusted: the sliders can be dragged past each other and the sidecar is
    // hand-editable. An unsorted pair silently makes the band empty, which reads as the mask
    // having stopped working rather than as a value being wrong.
    let lo = min(params.a0, params.a1);
    let hi = max(params.a0, params.a1);
    let w = clamp(params.feather / 100.0, 0.0, 1.0) * LUM_FEATHER_SCALE;
    var c = 0.0;
    if (w <= LUM_FEATHER_EPS) {
        c = select(0.0, 1.0, l >= lo && l <= hi);
    } else {
        let rising = clamp((l - (lo - w)) / w, 0.0, 1.0);
        let falling = clamp(((hi + w) - l) / w, 0.0, 1.0);
        let t = min(rising, falling);
        c = t * t * (3.0 - 2.0 * t);
    }
    return select(c, 1.0 - c, params.invert == 1u);
}

// ⚠⚠ **WGSL has no `rem_euclid`, and `x % 360` is NOT it.** WGSL's `%` keeps the sign of the
// dividend, so a negative hue — which `atan2` produces for everything below 0 degrees, i.e. half
// the colour wheel — comes back negative and lands outside every band. `ColourRange::coverage_at`
// takes BOTH its span and its offset with `rem_euclid`, which is what makes the wrapped and
// unwrapped cases one expression with no branch. The failure is a seam on magentas, not an error.
fn rem_euclid_360(x: f32) -> f32 {
    return x - floor(x / 360.0) * 360.0;
}

fn cov_colour(px: vec3<f32>) -> f32 {
    let lab = prophoto_to_oklab(px.x, px.y, px.z);
    let hue = degrees(atan2(lab.z, lab.y));
    // ⚠ Chroma is NOT clamped and must not be — it is the gate's own input, and a wide-gamut
    // colour legitimately reads above anything sRGB can express.
    let chroma = sqrt(lab.y * lab.y + lab.z * lab.z);

    let span = rem_euclid_360(params.a1 - params.a0);
    let offset = rem_euclid_360(hue - params.a0);
    let w = clamp(params.feather / 100.0, 0.0, 1.0) * COL_FEATHER_SCALE;
    var band = 0.0;
    if (w <= COL_FEATHER_EPS) {
        band = select(0.0, 1.0, offset <= span);
    } else {
        // Distance outside the band, measured the short way round from whichever edge is nearer.
        let past = max(offset - span, 0.0);
        let before = max(360.0 - offset, 0.0);
        let t = clamp(1.0 - min(past, before) / w, 0.0, 1.0);
        band = t * t * (3.0 - 2.0 * t);
    }
    // ⚠⚠ The chroma gate, applied BEFORE `invert`. Inverting must give "everything except these
    // colours", and a near-neutral pixel has no colour to be excepted — putting the gate after the
    // flip makes an inverted mask select every grey in the photograph, at full strength, which is
    // precisely the speckle the gate exists to prevent.
    var gate = clamp((chroma - params.a2) / COL_CHROMA_FADE, 0.0, 1.0);
    gate = gate * gate * (3.0 - 2.0 * gate);
    let c = band * gate;
    return select(c, 1.0 - c, params.invert == 1u);
}
"#;

/// The coverage entry point, with the component's own expression substituted in.
///
/// ⚠⚠ **ONE dispatch for the whole composite**, which is the rule the fold depends on — see the
/// spec's §5.4. With a single component there is nothing to fold yet and the shape is already the
/// one the fold slots into.
///
/// ⚠⚠ **It reads `img`, which at this point is still the BASE.** That is why the dispatch is
/// recorded before the sub-stages: a mask keyed on its own output chases its effect — lifting the
/// shadows moves them out of the band that selected them — which is exactly what `Composite::at`'s
/// `base_px` parameter exists to prevent on the CPU.
pub(crate) fn coverage_entry(base_expr: &str) -> String {
    format!(
        r#"
@compute @workgroup_size(64)
fn coverage(@builtin(global_invocation_id) id: vec3<u32>) {{
    let i = id.y * (params.row_groups * 64u) + id.x;
    if (i >= params.pixels) {{ return; }}
    let x = i % params.w;
    let y = i / params.w;
    let fx = params.x0 + x;
    let fy = params.y0 + y;
    // ⚠ FRAME-normalised, exactly as `MaskStage::apply` computes them.
    let u = f32(fx) / f32(params.fw);
    let v = f32(fy) / f32(params.fh);
    let b = i * 3u;
    let px = vec3<f32>(img[b], img[b + 1u], img[b + 2u]);
    cov[i] = clamp({base_expr}, 0.0, 1.0);
}}
"#
    )
}

/// `out = base + (adjusted - base) * coverage`, per channel.
///
/// ⚠ A multiply-add the GPU fuses, which is M8.3m's recorded divergence class: it rounds **once**
/// where the CPU rounds twice, so the GPU is the *more* accurate of the two. Not something to
/// "fix", and the reason this stage cannot claim bit-identity.
pub(crate) const BLEND: &str = r#"
@compute @workgroup_size(64)
fn blend(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.y * (params.row_groups * 64u) + id.x;
    if (i >= params.pixels) { return; }
    let c = cov[i];
    let b = i * 3u;
    img[b]      = base_r[i] + (img[b]      - base_r[i]) * c;
    img[b + 1u] = base_g[i] + (img[b + 1u] - base_g[i]) * c;
    img[b + 2u] = base_b[i] + (img[b + 2u] - base_b[i]) * c;
}
"#;
