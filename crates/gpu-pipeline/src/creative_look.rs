//! A creative look: a 3-D LUT laid over the finished photograph, as a compute shader.
//!
//! ⚠⚠ **The declared [`LutSpace`] decides WHERE this stage runs, not merely how it shapes** — a
//! display-referred LUT modifies the camera rendering, a linear one runs *in place of* it. That
//! placement lives in `pipeline`'s `build_pipeline_split_in_region_with_brush`; this file owns
//! only the shaping, the lookup and the strength blend, exactly as `CreativeLookStage` does.

use crate::{GpuContext, GpuImage, GpuStage};
use edit_model::LutSpace;
use pipeline::{lut::Cube, PipelineStage};
use std::sync::Arc;

/// The space tags the shader branches on. ⚠ Kept beside the `match` that produces them, because a
/// tag and its meaning living apart is how the two come to disagree about which curve is which.
const TAG_LINEAR: u32 = 0;
const TAG_SRGB: u32 = 1;
const TAG_REC709: u32 = 2;

fn tag(space: LutSpace) -> u32 {
    match space {
        LutSpace::Linear => TAG_LINEAR,
        LutSpace::SrgbDisplay => TAG_SRGB,
        LutSpace::Rec709 => TAG_REC709,
    }
}

fn m(mat: &[[f32; 3]; 3]) -> String {
    format!(
        "mat3x3<f32>({}f, {}f, {}f, {}f, {}f, {}f, {}f, {}f, {}f)",
        // ⚠ WGSL's `mat3x3` constructor takes COLUMNS. The CPU's `apply` treats the outer index as
        // a row, so the transpose is here rather than in the multiply — one place, written once.
        mat[0][0],
        mat[1][0],
        mat[2][0], //
        mat[0][1],
        mat[1][1],
        mat[2][1], //
        mat[0][2],
        mat[1][2],
        mat[2][2],
    )
}

/// ⚠⚠ **The primaries matrices are SUBSTITUTED from `color::spaces`**, never transcribed. A
/// substituted constant cannot disagree; a transcribed one is merely checked for agreeing — the
/// rule `luma_wgsl` records, and which `color`'s own doc justifies with six copies of a Rec.709
/// triple that all went wrong on one day.
///
/// ⚠ The **transfer** constants are the published sRGB and BT.709 ones, written out: they are not
/// `pub` anywhere in this workspace, and `rec709_and_srgb_are_not_the_same_curve` in `pipeline`
/// is the guard that the two never become aliases of each other. Nothing here is tuned.
///
/// ⚠⚠ **The domain remap comes FIRST and the clamp SECOND.** Clamping the raw value to `[0, 1]`
/// before normalising out of the declared domain is `Cube::lookup`'s own recorded trap wearing a
/// different hat, and it misplaces every sample while still producing a plausible image.
fn wgsl() -> String {
    format!(
        r#"
struct Params_creative_look {{
    size: u32,
    space: u32,
    pad0: u32,
    pad1: u32,
    dmin: vec3<f32>,
    blend: f32,
    dmax: vec3<f32>,
    pad2: f32,
}};

const PP_TO_SRGB_LOOK: mat3x3<f32> = {pp_to_srgb};
const SRGB_TO_PP_LOOK: mat3x3<f32> = {srgb_to_pp};

fn encode_creative_look(v: f32, space: u32) -> f32 {{
    let c = clamp(v, 0.0, 1.0);
    if (space == {tag_srgb}u) {{
        // sRGB's transfer: a 1/2.4 power with a 12.92 slope below 0.0031308.
        if (c <= 0.0031308) {{ return c * 12.92; }}
        return 1.055 * pow(c, 1.0 / 2.4) - 0.055;
    }}
    // ⚠ BT.709's is NOT sRGB's, though the two are routinely confused: a 0.45 power with a 4.5
    // slope below 0.018.
    if (c < 0.018) {{ return 4.5 * c; }}
    return 1.099 * pow(c, 0.45) - 0.099;
}}

fn decode_creative_look(v: f32, space: u32) -> f32 {{
    let c = clamp(v, 0.0, 1.0);
    if (space == {tag_srgb}u) {{
        if (c <= 0.04045) {{ return c / 12.92; }}
        return pow((c + 0.055) / 1.055, 2.4);
    }}
    if (c < 0.081) {{ return c / 4.5; }}
    return pow((c + 0.099) / 1.099, 1.0 / 0.45);
}}

/// The entry at integer cube coordinates. ⚠ Red varies fastest — the format's own ordering, and
/// the same index `Cube::at` computes.
fn cube_at_creative_look(r: u32, g: u32, b: u32, size: u32, base: u32) -> vec3<f32> {{
    let i = base + ((b * size + g) * size + r) * 3u;
    return vec3<f32>(tables[i], tables[i + 1u], tables[i + 2u]);
}}

fn lookup_creative_look(rgb: vec3<f32>, q: Params_creative_look, base: u32) -> vec3<f32> {{
    let n = q.size - 1u;
    let fn_ = f32(n);
    // ⚠⚠ Normalise out of the declared domain FIRST, then clamp. The other order is the recorded
    // bug.
    let t = (rgb - q.dmin) / (q.dmax - q.dmin);
    let x = clamp(t, vec3<f32>(0.0), vec3<f32>(1.0)) * fn_;
    let fl = floor(x);
    // ⚠ `min(.., n - 1)` matches the CPU's `saturating_sub(1)`; `n` is at least 1 because
    // `Cube::parse` refuses a size below 2.
    let lo = min(vec3<u32>(fl), vec3<u32>(n - 1u));
    let fr = x - vec3<f32>(lo);
    let hi = min(lo + vec3<u32>(1u), vec3<u32>(n));

    let c000 = cube_at_creative_look(lo.x, lo.y, lo.z, q.size, base);
    let c100 = cube_at_creative_look(hi.x, lo.y, lo.z, q.size, base);
    let c010 = cube_at_creative_look(lo.x, hi.y, lo.z, q.size, base);
    let c110 = cube_at_creative_look(hi.x, hi.y, lo.z, q.size, base);
    let c001 = cube_at_creative_look(lo.x, lo.y, hi.z, q.size, base);
    let c101 = cube_at_creative_look(hi.x, lo.y, hi.z, q.size, base);
    let c011 = cube_at_creative_look(lo.x, hi.y, hi.z, q.size, base);
    let c111 = cube_at_creative_look(hi.x, hi.y, hi.z, q.size, base);
    // ⚠ `a + (b - a) * t` and not `a*(1-t) + b*t`: the first returns `a` exactly at t = 0, which
    // is what makes an identity cube exact at every grid point. The CPU writes it the same way.
    let c00 = c000 + (c100 - c000) * fr.x;
    let c10 = c010 + (c110 - c010) * fr.x;
    let c01 = c001 + (c101 - c001) * fr.x;
    let c11 = c011 + (c111 - c011) * fr.x;
    let a = c00 + (c10 - c00) * fr.y;
    let b = c01 + (c11 - c01) * fr.y;
    return a + (b - a) * fr.z;
}}

fn look_creative_look(px: vec3<f32>, q: Params_creative_look, base: u32) -> vec3<f32> {{
    // ⚠⚠ **`Linear` takes NO shaper**, and that single branch is what makes a linear LUT a
    // *rendering* rather than a grade: the data here is already scene-linear in the working
    // space, so a LUT authored against linear input wants it untouched. Shaping it would apply a
    // transfer the LUT then applies again — a plausible photograph, not an error.
    if (q.space == {tag_linear}u) {{ return lookup_creative_look(px, q, base); }}
    // ⚠ The primaries matrix as well as the transfer. Rec.709 shares sRGB's primaries and differs
    // only in transfer, so one matrix serves both — the definition, not an approximation.
    let lin = PP_TO_SRGB_LOOK * px;
    let shaped = vec3<f32>(
        encode_creative_look(lin.x, q.space),
        encode_creative_look(lin.y, q.space),
        encode_creative_look(lin.z, q.space),
    );
    let out = lookup_creative_look(shaped, q, base);
    let back = vec3<f32>(
        decode_creative_look(out.x, q.space),
        decode_creative_look(out.y, q.space),
        decode_creative_look(out.z, q.space),
    );
    return SRGB_TO_PP_LOOK * back;
}}

fn apply_creative_look(p: vec3<f32>, x: u32, y: u32, q: Params_creative_look, base: u32) -> vec3<f32> {{
    // ⚠ The short-circuit at 0, mirroring the CPU stage's.
    //
    // ⚠⚠ **Recorded INERT rather than counted: removing it fails nothing, and provably cannot.**
    // The blend is written `a + (b - a) * t`, and for any FINITE `b` that is bit-exactly `a` at
    // `t = 0` in IEEE arithmetic — the residue the CPU stage's comment warns about belongs to the
    // other spelling, `a*(1-t) + b*t`. The shaper's `pow` takes a clamped non-negative base, so a
    // non-finite `b` is unreachable here. Kept as defence in depth and because it mirrors the CPU
    // line for line; it becomes load-bearing the moment anything can hand this a NaN.
    if (q.blend == 0.0) {{ return p; }}
    let out = look_creative_look(p, q, base);
    return p + (out - p) * q.blend;
}}
"#,
        pp_to_srgb = m(&color::spaces::PROPHOTO_TO_SRGB),
        srgb_to_pp = m(&color::spaces::SRGB_TO_PROPHOTO),
        tag_linear = TAG_LINEAR,
        tag_srgb = TAG_SRGB,
    )
}

/// A creative look, on the GPU.
pub struct GpuCreativeLookStage {
    /// The cube's own entries, flattened once per construction for the storage binding.
    ///
    /// ⚠ Flattened rather than rebuilt: this is `Cube::entries` reinterpreted, so the GPU and the
    /// CPU cannot come to disagree about the table.
    flat: Vec<f32>,
    size: u32,
    space: LutSpace,
    dmin: [f32; 3],
    dmax: [f32; 3],
    blend: f32,
    cpu: pipeline::CreativeLookStage,
}

impl GpuCreativeLookStage {
    pub fn new(cube: &Arc<Cube>, space: LutSpace, strength: f32, _ctx: &Arc<GpuContext>) -> Self {
        let flat = cube.entries().iter().flatten().copied().collect();
        Self {
            flat,
            size: cube.size as u32,
            space,
            dmin: cube.dmin(),
            dmax: cube.dmax(),
            blend: (strength / 100.0).clamp(0.0, 1.0),
            cpu: pipeline::CreativeLookStage::new(Arc::clone(cube), space, strength),
        }
    }
}

impl GpuStage for GpuCreativeLookStage {
    fn name(&self) -> &'static str {
        "creative_look"
    }

    /// Measured at 3200x2136 by this module's own `probe_look`, three runs.
    ///
    /// ⚠⚠ **THE SHAPER DOMINATES, NOT THE LOOKUP, and that reverses the obvious expectation.**
    /// Display-referred reads **96.48-99.96 ms** — the most expensive stage in the tail, more than
    /// `hsl` — while `Linear`, which takes no shaper at all, reads **20.72-22.15**. Six `powf` per
    /// pixel against a trilinear sample of a table that fits in cache.
    ///
    /// ⚠⚠ **And the cube's SIZE is not the cost: 33³ and 65³ measure the same to within noise**
    /// (0.43 MB against 3.30 MB of table). Anyone reaching for a smaller LUT to make this cheaper
    /// would be optimising the wrong thing.
    ///
    /// ⚠ So this branches, and it can do so honestly: [`LutSpace::is_the_rendering`] is **the one
    /// place that rule is written down** — the pipeline's placement and the panel's warning both
    /// read it there — so this reads it rather than copying it. Contrast the vignette, whose own
    /// 5x branch is decided inside a private `Shape::new`, and which therefore ships one
    /// conservative number rather than putting that rule in a second place.
    ///
    /// ⚠ Each is the bottom of its band, the conservative direction: a lower number makes a run
    /// less likely to chain, and the gate must fail toward the CPU.
    fn cpu_cost(&self) -> f32 {
        if self.space.is_the_rendering() {
            20.7
        } else {
            96.4
        }
    }

    fn encode(
        &self,
        ctx: &GpuContext,
        pass: &mut wgpu::ComputePass,
        img: &GpuImage,
        // ⚠ Per-pixel and in-place-safe, so the default `scratch_planes` of 0 covers it.
        _scratch: &[wgpu::Buffer],
    ) {
        crate::per_pixel::encode_single(self, ctx, pass, img);
    }

    fn per_pixel(&self, _w: usize, _h: usize) -> Option<crate::per_pixel::Part<'_>> {
        // ⚠ `vec3` in a uniform is 16-byte aligned, so `dmin`/`dmax` each occupy a full block with
        // a scalar tucked into the fourth slot. Laid out to match the WGSL struct exactly.
        let mut params = [0u8; 48];
        params[0..4].copy_from_slice(&self.size.to_le_bytes());
        params[4..8].copy_from_slice(&tag(self.space).to_le_bytes());
        for (i, v) in self.dmin.iter().enumerate() {
            params[16 + i * 4..20 + i * 4].copy_from_slice(&v.to_le_bytes());
        }
        params[28..32].copy_from_slice(&self.blend.to_le_bytes());
        for (i, v) in self.dmax.iter().enumerate() {
            params[32 + i * 4..36 + i * 4].copy_from_slice(&v.to_le_bytes());
        }
        Some(crate::per_pixel::Part {
            name: "creative_look",
            wgsl: wgsl(),
            params: params.to_vec(),
            tables: vec![&self.flat],
            preludes: crate::per_pixel::Preludes::default(),
            needs_position: false,
        })
    }

    fn cpu(&self) -> &dyn PipelineStage {
        &self.cpu
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_image::ImageBuf;

    fn ctx() -> Option<Arc<GpuContext>> {
        match crate::shared() {
            Some(c) => Some(c.clone()),
            None => {
                eprintln!("SKIPPED: no GPU adapter on this machine");
                None
            }
        }
    }

    /// A `.cube` whose entries are produced by `f`. ⚠ Red varies fastest — the format's ordering,
    /// and getting it wrong here would make every test agree with a shader that also had it wrong.
    fn cube_of(size: usize, f: impl Fn([f32; 3]) -> [f32; 3], header: &str) -> Arc<Cube> {
        let n = (size - 1) as f32;
        let mut t = format!("LUT_3D_SIZE {size}\n{header}\n");
        for b in 0..size {
            for g in 0..size {
                for r in 0..size {
                    let o = f([r as f32 / n, g as f32 / n, b as f32 / n]);
                    t.push_str(&format!("{} {} {}\n", o[0], o[1], o[2]));
                }
            }
        }
        Arc::new(Cube::parse(&t).expect("the fixture cube must parse"))
    }

    fn identity(size: usize) -> Arc<Cube> {
        cube_of(size, |c| c, "")
    }

    /// A look that is obviously not the identity: warm it, and lift the greens.
    fn warm(size: usize) -> Arc<Cube> {
        cube_of(
            size,
            |c| {
                [
                    (c[0] * 1.25).clamp(0.0, 1.0),
                    (c[1] * 0.5 + 0.25).clamp(0.0, 1.0),
                    (c[2] * 0.7).clamp(0.0, 1.0),
                ]
            },
            "",
        )
    }

    /// ⚠ Spans muted, saturated, near-black and **above white**, so the shaper's clamp and the
    /// cube's edge are both exercised.
    fn spread() -> ImageBuf {
        ImageBuf::from_vec(
            6,
            1,
            vec![
                0.40, 0.38, 0.36, //
                0.80, 0.12, 0.10, //
                0.18, 0.18, 0.18, //
                1.40, 0.90, 0.30, //
                0.02, 0.03, 0.05, //
                0.05, 0.35, 0.62, //
            ],
        )
    }

    fn chained(
        ctx: &Arc<GpuContext>,
        cube: &Arc<Cube>,
        space: LutSpace,
        strength: f32,
        img: ImageBuf,
    ) -> ImageBuf {
        crate::GpuChain::new(
            ctx.clone(),
            vec![Box::new(GpuCreativeLookStage::new(
                cube, space, strength, ctx,
            ))],
        )
        .apply(img)
    }

    /// ⚠⚠ **The fixture check, and without it every equivalence assertion is vacuous.** The GPU is
    /// compared against the CPU, so a shader that did nothing would agree perfectly with a CPU
    /// stage that also did nothing.
    #[test]
    fn the_fixture_is_one_this_stage_actually_moves() {
        let img = spread();
        let out = pipeline::CreativeLookStage::new(warm(17), LutSpace::SrgbDisplay, 100.0)
            .apply(img.clone());
        let moved = img
            .data()
            .iter()
            .zip(out.data())
            .filter(|(a, b)| (*a - *b).abs() > 1e-3)
            .count();
        assert!(
            moved >= 12,
            "fixture check: the look must move most of this fixture, moved {moved} of 18"
        );
    }

    /// ⚠⚠ **Every space, and `Linear` is the one that matters most.** It takes NO shaper, and that
    /// single branch is what makes a linear LUT a rendering rather than a grade — getting it wrong
    /// applies a transfer the LUT then applies again, which is a plausible photograph.
    #[test]
    fn the_gpu_matches_the_cpu_in_every_space() {
        let Some(ctx) = ctx() else { return };
        let cube = warm(17);
        for space in LutSpace::ALL {
            for strength in [100.0f32, 45.0] {
                let img = spread();
                let cpu = pipeline::CreativeLookStage::new(Arc::clone(&cube), space, strength)
                    .apply(img.clone());
                let gpu = chained(&ctx, &cube, space, strength, img);
                let d = crate::compare(&cpu, &gpu);
                assert!(
                    d.worst_levels < 1.0,
                    "{space:?} at strength {strength} diverged: {d:?}"
                );
            }
        }
    }

    /// ⚠⚠ **An identity cube must return the input in EVERY space**, which is the one assertion
    /// that can see a shaper and its inverse being mismatched: a wrong pair still round-trips
    /// *approximately*, so only measuring the margin against the input catches it.
    #[test]
    fn an_identity_cube_returns_the_input_in_every_space() {
        let Some(ctx) = ctx() else { return };
        let cube = identity(33);
        // ⚠⚠ **The fixture is DERIVED from sRGB colours, not transcribed as ProPhoto ones** — and
        // the first version of this test failed at 31 levels against correct code because of it.
        // ProPhoto's gamut is far wider than sRGB's, so a plausible-looking ProPhoto triple such
        // as `[0.1, 0.3, 0.5]` converts to a **negative** sRGB red, which the shaper clamps. That
        // clamp is the stage's behaviour, not a defect; only a colour that is genuinely inside
        // sRGB's gamut can round-trip, and the only honest way to get one is to start there.
        let fixture: Vec<f32> = [[0.5f32, 0.4, 0.3], [0.2, 0.6, 0.35], [0.05, 0.08, 0.12]]
            .iter()
            .flat_map(|c| color::spaces::srgb_to_prophoto(*c))
            .collect();
        for space in LutSpace::ALL {
            let img = ImageBuf::from_vec(3, 1, fixture.clone());
            let out = chained(&ctx, &cube, space, 100.0, img.clone());
            let d = crate::compare(&img, &out);
            assert!(
                d.worst_levels < 1.0,
                "an identity cube must return the input in {space:?}: {d:?}"
            );
        }
    }

    /// ⚠⚠ **A strength of zero must be BIT-EXACTLY the input, and a lerp cannot deliver that.**
    /// The shaper clamps out-of-gamut values, so running it at zero and lerping back would leave a
    /// residue on saturated colours — a look dialled to zero that is not quite no look. The
    /// short-circuit is a correctness guard, not a speed one, which is why this reads raw floats.
    #[test]
    fn a_strength_of_zero_is_bit_exactly_the_input() {
        let Some(ctx) = ctx() else { return };
        let img = spread();
        let out = chained(&ctx, &warm(17), LutSpace::SrgbDisplay, 0.0, img.clone());
        assert_eq!(
            img.data(),
            out.data(),
            "strength 0 must leave every sample untouched"
        );
    }

    /// ⚠⚠ **A declared input domain must be honoured, and ignoring it is SILENTLY wrong** — the
    /// cube is simply indexed over the wrong interval, misplacing every sample while still
    /// producing a plausible image. A real ACES LMT declares `LUT_3D_INPUT_RANGE 0.0927 1.0196`.
    #[test]
    fn a_declared_domain_is_honoured() {
        let Some(ctx) = ctx() else { return };
        let cube = cube_of(
            17,
            |c| [c[0], c[1] * 0.5, c[2]],
            // ⚠⚠ A REAL ACES LMT's declared range, and the specific numbers matter: a domain whose
            // top is **above 1.0** is what makes "clamp before the remap" distinguishable from
            // "remap then clamp". With a domain inside [0, 1] the two agree on every sample the
            // fixture carries, and the sabotage passes while the bug is real.
            "LUT_3D_INPUT_RANGE 0.0927061729 1.0196260353",
        );
        let img = spread();
        let cpu = pipeline::CreativeLookStage::new(Arc::clone(&cube), LutSpace::Linear, 100.0)
            .apply(img.clone());
        // ⚠ Fixture check: the domain must actually bite, or this test passes against a shader
        // that ignores it.
        let plain = cube_of(17, |c| [c[0], c[1] * 0.5, c[2]], "");
        let unshifted =
            pipeline::CreativeLookStage::new(plain, LutSpace::Linear, 100.0).apply(img.clone());
        assert!(
            crate::compare(&cpu, &unshifted).worst_levels > 4.0,
            "fixture check: the declared domain must change the answer"
        );
        let gpu = chained(&ctx, &cube, LutSpace::Linear, 100.0, img);
        let d = crate::compare(&cpu, &gpu);
        assert!(d.worst_levels < 1.0, "the declared domain diverged: {d:?}");
    }

    /// ⚠ The name is what `Pipeline::stage_names` reports and what the order-asserting tests match
    /// on, so a GPU substitution must be invisible to them.
    #[test]
    fn the_stage_is_named_like_the_cpu_one() {
        let Some(ctx) = ctx() else { return };
        let cube = identity(2);
        assert_eq!(
            GpuCreativeLookStage::new(&cube, LutSpace::SrgbDisplay, 100.0, &ctx).name(),
            pipeline::CreativeLookStage::new(cube, LutSpace::SrgbDisplay, 100.0).name()
        );
    }

    /// The kept instrument: what the look costs on the CPU, and what its table costs to upload.
    ///
    /// ⚠ `#[ignore]`d and **release-only meaningful**, like `box_probe`'s `probe_box`. A constant
    /// nobody can re-measure is a constant nobody can defend.
    ///
    /// Run: `cargo test --release -p gpu-pipeline probe_look -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn probe_look() {
        use std::time::Instant;
        let (w, h) = (3200usize, 2136);
        let mut px = Vec::with_capacity(w * h * 3);
        for i in 0..w * h {
            let t = (i % 4096) as f32 / 4096.0 * 1.2;
            px.extend_from_slice(&[t, t * 0.9, t * 1.1]);
        }
        let img = ImageBuf::from_vec(w, h, px);
        for size in [33usize, 65] {
            let cube = warm(size);
            for space in [LutSpace::SrgbDisplay, LutSpace::Linear] {
                let stage = pipeline::CreativeLookStage::new(Arc::clone(&cube), space, 100.0);
                let mut best = f64::MAX;
                for _ in 0..5 {
                    let c = img.clone();
                    let t = Instant::now();
                    let out = stage.apply(c);
                    best = best.min(t.elapsed().as_secs_f64() * 1000.0);
                    std::hint::black_box(&out);
                }
                println!("cpu {size}^3 {space:?}: {best:.2} ms");
            }
            println!(
                "  table {size}^3: {} entries, {:.2} MB",
                cube.size.pow(3),
                cube.size.pow(3) as f64 * 12.0 / 1e6
            );
        }
    }
}
