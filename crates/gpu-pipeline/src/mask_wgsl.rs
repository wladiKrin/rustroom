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
    // ⚠ `aspect` is the FRAME's, `fw / fh`, exactly as `MaskStage::apply` derives it — a region's
    // own aspect is a different number and would skew a diagonal gradient's iso-lines.
    aspect: f32,
    pad1: f32,
    pad2: f32,
    pad3: f32,
};

// One component of the composite.
//
// ⚠⚠ **Per-component values live HERE rather than in the uniform, and that is what makes a
// composite expressible at all.** A uniform holds one component's worth; a mask is a base plus
// its refinements, so the count is not known until the mask is. The *chain* is still unrolled
// into the shader text at build time — only the numbers travel.
//
// ⚠ A 16-byte-aligned `vec4` forces the stride to a multiple of 16, so the tail is padded to 64
// rather than left at 52 and silently mis-strided by the driver.
struct Comp {
    // `from.xy`/`to.xy` for a linear gradient, `center.xy`/`radius.xy` for a radial one, and
    // `low`/`high`/`min_chroma` for a range.
    a: vec4<f32>,
    feather: f32,
    pad0: f32,
    invert: u32,
    // A painted component's value everywhere outside its box.
    outside: u32,
    // A painted component's box, and where its packed coverage starts in `packed`.
    //
    // ⚠⚠ **`off` is what lets several painted components share ONE buffer.** The binding count is
    // fixed, so a composite of three brushes cannot have three coverage bindings; they are
    // concatenated instead and each component carries its own word offset.
    box_x0: u32,
    box_y0: u32,
    box_w: u32,
    box_h: u32,
    off: u32,
    pad2: u32,
    pad3: u32,
    pad4: u32,
};
@group(0) @binding(0) var<storage, read_write> img: array<f32>;
@group(0) @binding(1) var<uniform> params: Params;
@group(0) @binding(2) var<storage, read_write> base_r: array<f32>;
@group(0) @binding(3) var<storage, read_write> base_g: array<f32>;
@group(0) @binding(4) var<storage, read_write> base_b: array<f32>;
@group(0) @binding(5) var<storage, read_write> cov: array<f32>;
@group(0) @binding(6) var<storage, read> packed: array<u32>;
@group(0) @binding(7) var<storage, read> comps: array<Comp>;
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
fn painted_at(fx: u32, fy: u32, j: u32) -> f32 {
    let c = comps[j];
    if (fx < c.box_x0 || fy < c.box_y0
        || fx >= c.box_x0 + c.box_w || fy >= c.box_y0 + c.box_h) {
        return f32(c.outside) / 255.0;
    }
    let k = (fy - c.box_y0) * c.box_w + (fx - c.box_x0);
    // ⚠ `off` is in WORDS, and each word holds four samples — so the sample index is offset by
    // `off * 4`, not by `off`. Adding `off` to the word index instead reads a neighbour's
    // coverage shifted by three quarters of a buffer, which is a plausible mask and not an error.
    let word = packed[c.off + (k >> 2u)];
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
fn cov_linear(u: f32, v: f32, aspect: f32, j: u32) -> f32 {
    let p = comps[j];
    // ⚠ Work in units of frame HEIGHT, where both axes have the same pixel scale. A projection
    // needs a metric, and raw normalised space is anisotropic — without this a diagonal gradient's
    // iso-lines visibly skew. Same species of problem as the vignette's roundness.
    let ax = p.a.x * aspect;
    let ay = p.a.y;
    let bx = p.a.z * aspect;
    let by = p.a.w;
    let dx = bx - ax;
    let dy = by - ay;
    let len2 = dx * dx + dy * dy;
    // ⚠ Full coverage rather than NaN. The alternative reaches the blend and paints garbage.
    if (len2 <= F32_EPSILON) { return select(1.0, 0.0, p.invert == 1u); }
    let t = clamp((((u * aspect) - ax) * dx + (v - ay) * dy) / len2, 0.0, 1.0);
    return select(t, 1.0 - t, p.invert == 1u);
}

fn cov_radial(u: f32, v: f32, j: u32) -> f32 {
    let p = comps[j];
    let rx = p.a.z;
    let ry = p.a.w;
    var c = 0.0;
    // ⚠ A degenerate ellipse honestly covers nothing. Unlike the linear guard, which chooses 1.0
    // only because its alternative was NaN, 0.0 is the true answer here.
    if (rx > F32_EPSILON && ry > F32_EPSILON) {
        let dx = (u - p.a.x) / rx;
        let dy = (v - p.a.y) / ry;
        let d = sqrt(dx * dx + dy * dy);
        let f = clamp(p.feather / 100.0, 0.0, 1.0);
        if (f <= RADIAL_FEATHER_EPS) {
            c = select(0.0, 1.0, d <= 1.0);
        } else {
            // ⚠ Smoothstep rather than a linear ramp: a linear ramp in radial distance leaves a
            // visible crease ring where the derivative jumps.
            let t = clamp((1.0 - d) / f, 0.0, 1.0);
            c = t * t * (3.0 - 2.0 * t);
        }
    }
    return select(c, 1.0 - c, p.invert == 1u);
}
"#;

/// The two **pixel-dependent** kinds: a luminance range and a colour range.
///
/// ⚠⚠ **These are what make the coverage dispatch's POSITION load-bearing.** They read the pixel,
/// so running them after the sub-stages would key the mask on its own output — lifting the shadows
/// moves them out of the band that selected them. For a painted or analytic component the position
/// provably cannot matter, which is why that sabotage was inert until now.
pub(crate) const RANGES: &str = r#"
// ⚠⚠ **The scene facts arrive as arguments rather than being recomputed, and that is the whole
// point of the fold.** `BaseFacts` memoises `lightness` and `hue_chroma` on the CPU because a
// composite can hold several range components; the GPU does better, because the chain is known
// when the shader is built — `coverage_entry` hoists each fact above the chain when any component
// needs it and emits nothing when none does. Three colour components then cost ONE Oklab round
// trip, which is the property `crates/pipeline`'s thread-local conversion counter already asserts.
fn cov_luminance(l: f32, j: u32) -> f32 {
    let p = comps[j];
    // ⚠ Sorted rather than trusted: the sliders can be dragged past each other and the sidecar is
    // hand-editable. An unsorted pair silently makes the band empty, which reads as the mask
    // having stopped working rather than as a value being wrong.
    let lo = min(p.a.x, p.a.y);
    let hi = max(p.a.x, p.a.y);
    let w = clamp(p.feather / 100.0, 0.0, 1.0) * LUM_FEATHER_SCALE;
    var c = 0.0;
    if (w <= LUM_FEATHER_EPS) {
        c = select(0.0, 1.0, l >= lo && l <= hi);
    } else {
        let rising = clamp((l - (lo - w)) / w, 0.0, 1.0);
        let falling = clamp(((hi + w) - l) / w, 0.0, 1.0);
        let t = min(rising, falling);
        c = t * t * (3.0 - 2.0 * t);
    }
    return select(c, 1.0 - c, p.invert == 1u);
}

// ⚠⚠ **WGSL has no `rem_euclid`, and `x % 360` is NOT it.** WGSL's `%` keeps the sign of the
// dividend, so a negative hue — which `atan2` produces for everything below 0 degrees, i.e. half
// the colour wheel — comes back negative and lands outside every band. `ColourRange::coverage_at`
// takes BOTH its span and its offset with `rem_euclid`, which is what makes the wrapped and
// unwrapped cases one expression with no branch. The failure is a seam on magentas, not an error.
fn rem_euclid_360(x: f32) -> f32 {
    return x - floor(x / 360.0) * 360.0;
}

fn cov_colour(hue: f32, chroma: f32, j: u32) -> f32 {
    let p = comps[j];
    let span = rem_euclid_360(p.a.y - p.a.x);
    let offset = rem_euclid_360(hue - p.a.x);
    let w = clamp(p.feather / 100.0, 0.0, 1.0) * COL_FEATHER_SCALE;
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
    var gate = clamp((chroma - p.a.z) / COL_CHROMA_FADE, 0.0, 1.0);
    gate = gate * gate * (3.0 - 2.0 * gate);
    let c = band * gate;
    return select(c, 1.0 - c, p.invert == 1u);
}
"#;

/// How one refinement folds onto the coverage built so far.
///
/// ⚠⚠ **These are `MaskOp::combine`'s three rules, and they must stay exactly those.** That
/// function is itself the brush raster's own algebra — `Add` *is* the across-strokes paint step
/// and `Subtract` *is* `erase` — so a fourth spelling here would make subtract-between-components
/// disagree with erase-within-a-brush at every partial coverage, silently. `min`/`max` agree with
/// these at 0 and 1 and differ everywhere between, which is exactly how such a drift hides.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Fold {
    Add,
    Subtract,
    Intersect,
}

/// Which scene facts the chain needs computed above it.
///
/// ⚠ Derived from the components rather than passed by a caller who might disagree with them —
/// see `GpuMaskStage::facts`.
#[derive(Clone, Copy, Default)]
pub(crate) struct Facts {
    pub(crate) luma: bool,
    pub(crate) oklab: bool,
}

/// The coverage entry point: the scene facts, then the chain, unrolled.
///
/// ⚠⚠ **ONE dispatch for the whole composite, however many components it has.** The alternative —
/// a dispatch per component — would read `img` again after each one, and `img` stops being the
/// base the moment the sub-stages run.
///
/// ⚠⚠ **The chain is unrolled into the shader TEXT, while only the numbers travel in `comps`.**
/// The text is the program cache's key, so a composite of a given *shape* compiles once and is
/// reused as its sliders move — M8.3d's rule rather than a new one. A loop over `comps` with a
/// `kind` tag would compile once for every mask in the app and branch per pixel instead.
///
/// ⚠⚠ **It reads `img`, which at this point is still the BASE.** That is why the dispatch is
/// recorded before the sub-stages: a mask keyed on its own output chases its effect — lifting the
/// shadows moves them out of the band that selected them — which is exactly what `Composite::at`'s
/// `base_px` parameter exists to prevent on the CPU. ⚠ A mask with no pixel-dependent component
/// reads no pixel at all, so for *those* the ordering is provably inert; it becomes load-bearing
/// the moment a luminance or colour component joins the chain.
///
/// ⚠ The base is a separate parameter from the refinements, so "a refinement in position zero" is
/// unrepresentable rather than guarded — the shape `Composite { base, refine }` already has.
///
/// `invert` is the whole-mask Invert (M10.12a): `1 − a` after the last fold.
pub(crate) fn coverage_entry(
    base: &str,
    refine: &[(Fold, String)],
    invert: bool,
    facts: Facts,
) -> String {
    let mut hoist = String::new();
    if facts.luma || facts.oklab {
        hoist.push_str(
            "    let b = i * 3u;\n    let px = vec3<f32>(img[b], img[b + 1u], img[b + 2u]);\n",
        );
    }
    if facts.luma {
        // ⚠⚠ `linear_to_srgb` clamps its input, and that IS the index clamp — the selector is
        // clamped while the pixel keeps its headroom. A specular highlight therefore selects at
        // the top of the scale and still carries every bit of its latitude into the adjustment.
        // Clamping the *pixel* would be the `ProfileHueSatMap` trap, met three times in this repo.
        hoist.push_str(
            "    let lum = linear_to_srgb(prophoto_luminance(px.x, px.y, px.z)) * 100.0;\n",
        );
    }
    if facts.oklab {
        // ⚠ Chroma is NOT clamped and must not be — it is the gate's own input, and a wide-gamut
        // colour legitimately reads above anything sRGB can express.
        hoist.push_str(
            "    let lab = prophoto_to_oklab(px.x, px.y, px.z);\n\
             \x20   let hue = degrees(atan2(lab.z, lab.y));\n\
             \x20   let chroma = sqrt(lab.y * lab.y + lab.z * lab.z);\n",
        );
    }

    // ⚠ Each component is clamped to 0..1 before it is folded, exactly as `Composite::at` clamps
    // `source_at`'s result. The ops are only order-preserving on that range: `Add` with a b above
    // 1 drives coverage past 1, and `Subtract` with one below 0 *raises* it.
    let mut chain = format!("    var a = clamp({base}, 0.0, 1.0);\n");
    for (op, expr) in refine {
        chain.push_str(&match op {
            // Standard "over": adds a fraction of what is still uncovered.
            Fold::Add => format!("    a = a + (1.0 - a) * clamp({expr}, 0.0, 1.0);\n"),
            Fold::Subtract => format!("    a = a * (1.0 - clamp({expr}, 0.0, 1.0));\n"),
            Fold::Intersect => format!("    a = a * clamp({expr}, 0.0, 1.0);\n"),
        });
    }
    // ⚠ The whole-mask Invert (M10.12a): after the LAST fold, as `Composite::at` does. The flag is
    // in the shader text, so an inverted mask is its own cached program.
    if invert {
        chain.push_str("    a = 1.0 - a;\n");
    }

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
{hoist}{chain}    cov[i] = clamp(a, 0.0, 1.0);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn colour(j: usize) -> String {
        format!("cov_colour(hue, chroma, {j}u)")
    }

    /// ⚠⚠ **The property the CPU has a dedicated conversion counter for, asserted here on the
    /// generated text because that is where the GPU decides it.** `BaseFacts` memoises per pixel;
    /// the shader hoists once above the chain, so three colour components must emit exactly one
    /// `prophoto_to_oklab` call however long the chain is.
    #[test]
    fn three_colour_components_hoist_one_oklab_round_trip() {
        let refine = vec![(Fold::Add, colour(1)), (Fold::Intersect, colour(2))];
        let s = coverage_entry(
            &colour(0),
            &refine,
            false,
            Facts {
                luma: false,
                oklab: true,
            },
        );
        assert_eq!(
            s.matches("prophoto_to_oklab(px").count(),
            1,
            "the Oklab round trip must be hoisted exactly once:\n{s}"
        );
        assert_eq!(
            s.matches("cov_colour").count(),
            3,
            "fixture check: all three components must still be in the chain:\n{s}"
        );
    }

    /// ⚠ A mask with no pixel-dependent component reads no pixel at all — which is also why the
    /// coverage-before-sub-stages ordering is provably inert for such a mask, and load-bearing
    /// only once a range joins the chain.
    #[test]
    fn a_chain_that_needs_no_pixel_never_reads_the_image() {
        let s = coverage_entry("cov_radial(u, v, 0u)", &[], false, Facts::default());
        assert!(
            !s.contains("img["),
            "an analytic-only chain must not touch `img`:\n{s}"
        );
        assert!(
            !s.contains("let lum") && !s.contains("let hue"),
            "and it must hoist nothing:\n{s}"
        );
    }

    /// ⚠ A luminance component hoists the luma and NOT the Oklab pair, and the reverse. Hoisting
    /// both unconditionally would cost a colour mask an sRGB encode and a luminance mask a full
    /// Oklab round trip, per pixel, for nothing.
    #[test]
    fn each_fact_is_hoisted_only_when_something_reads_it() {
        let lum = coverage_entry(
            "cov_luminance(lum, 0u)",
            &[],
            false,
            Facts {
                luma: true,
                oklab: false,
            },
        );
        assert!(lum.contains("let lum"), "{lum}");
        assert!(!lum.contains("let hue"), "{lum}");

        let col = coverage_entry(
            &colour(0),
            &[],
            false,
            Facts {
                luma: false,
                oklab: true,
            },
        );
        assert!(col.contains("let hue"), "{col}");
        assert!(!col.contains("let lum"), "{col}");
    }

    /// ⚠⚠ **Three folds that emitted the same text would render three identical pictures and pass
    /// every equivalence test**, because each would be compared against a CPU composite using the
    /// same wrong op. The grading goldens' rule: several plausible outputs prove nothing unless
    /// they are shown to differ.
    #[test]
    fn the_three_folds_emit_three_different_rules() {
        let of = |op: Fold| coverage_entry("B", &[(op, "R".into())], false, Facts::default());
        let (a, s, i) = (of(Fold::Add), of(Fold::Subtract), of(Fold::Intersect));
        assert_ne!(a, s);
        assert_ne!(a, i);
        assert_ne!(s, i);
    }

    /// ⚠ Every component is clamped before it folds, exactly as `Composite::at` clamps
    /// `source_at`'s result. The ops are only order-preserving on `0..=1`.
    #[test]
    fn every_component_is_clamped_before_it_folds() {
        let s = coverage_entry("B", &[(Fold::Add, "R".into())], false, Facts::default());
        assert!(s.contains("clamp(B, 0.0, 1.0)"), "{s}");
        assert!(s.contains("clamp(R, 0.0, 1.0)"), "{s}");
    }

    #[test]
    fn the_whole_mask_invert_flips_after_the_last_fold() {
        let plain = coverage_entry(
            "B",
            &[(Fold::Subtract, "R".into())],
            false,
            Facts::default(),
        );
        let inv = coverage_entry("B", &[(Fold::Subtract, "R".into())], true, Facts::default());
        assert!(
            !plain.contains("a = 1.0 - a;"),
            "an uninverted mask must not flip"
        );
        let flip = inv.find("a = 1.0 - a;").expect("the inverted mask flips");
        let fold = inv
            .find("a = a * (1.0 - clamp(R")
            .expect("the fold is there");
        assert!(flip > fold, "the flip must come after the last fold");
    }
}
