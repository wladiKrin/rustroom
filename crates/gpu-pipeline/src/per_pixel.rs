//! A run of per-pixel GPU twins as ONE dispatch: the GPU side of `pipeline::per_pixel`.
//!
//! Each per-pixel twin describes itself as a [`Part`]: a WGSL function `apply_<name>` with its
//! `Params_<name>` struct, its parameter bytes, and its tables. [`emit`] concatenates a run into
//! one shader with **three bindings whatever the run's length** — the image, one uniform holding
//! a header and every member's parameters at 16-byte-aligned offsets, and one storage buffer
//! holding every member's tables — and a `main` that folds a pixel through the members in order.
//!
//! ⚠⚠ **A lone per-pixel twin is a run of one through the same emitter**, so there is one
//! definition of every stage's shader and the per-stage equivalence tests exercise this code.
//!
//! ⚠ The program cache keys on the shader text, which here is the run's shape plus its table
//! offsets, so a drag compiles once.

use crate::dispatch::Dispatch;
use crate::{GpuContext, GpuImage, GpuStage};

/// Which shared preludes a part's function needs.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Preludes {
    pub srgb: bool,
    pub luma: bool,
    pub oklab: bool,
}

/// One per-pixel twin, as the emitter needs it.
pub struct Part<'a> {
    /// The stage's name; the struct is `Params_<name>` and the function `apply_<name>`.
    pub name: &'static str,
    /// WGSL declaring `struct Params_<name>` and
    /// `fn apply_<name>(p: vec3<f32>, x: u32, y: u32, q: Params_<name>, base: u32) -> vec3<f32>`,
    /// plus any helpers, every one of them suffixed with the stage's name.
    pub wgsl: String,
    /// The parameters, laid out as `Params_<name>`. ⚠ A multiple of 16 bytes, checked by `emit`.
    pub params: Vec<u8>,
    /// The tables, in the order the function indexes them from `base`.
    pub tables: Vec<&'a [f32]>,
    pub preludes: Preludes,
    /// Whether the function reads `x` or `y`. Only the vignette does.
    pub needs_position: bool,
}

/// What `emit` produces: the shader, the uniform bytes, the concatenated tables.
pub(crate) struct Emitted {
    pub shader: String,
    pub params: Vec<u8>,
    pub tables: Vec<f32>,
}

/// The header every fused shader starts its uniform with.
const HEADER_BYTES: usize = 16;

/// Concatenate `parts` into one shader, one uniform and one table buffer.
///
/// ⚠ Pure, so the layout — offsets, bases, order — is tested with no GPU.
pub(crate) fn emit(parts: &[Part<'_>], pixels: u32, row_groups: u32, w: u32) -> Emitted {
    let mut preludes = Preludes::default();
    let mut needs_position = false;
    let mut params =
        Vec::with_capacity(HEADER_BYTES + parts.iter().map(|p| p.params.len()).sum::<usize>());
    params.extend_from_slice(&pixels.to_le_bytes());
    params.extend_from_slice(&row_groups.to_le_bytes());
    params.extend_from_slice(&w.to_le_bytes());
    params.extend_from_slice(&0u32.to_le_bytes());
    let mut tables: Vec<f32> = Vec::new();
    let mut members = String::new();
    let mut bodies = String::new();
    let mut calls = String::new();
    // ⚠ A stage's struct and function are declared ONCE per run, however often the stage occurs:
    // a production tail never repeats a name, but a chain of eight exposures is a legitimate test
    // fixture, and naga rejects a duplicate declaration. Each occurrence still gets its own
    // parameters and tables.
    let mut declared: std::collections::HashSet<&'static str> = std::collections::HashSet::new();
    for (k, part) in parts.iter().enumerate() {
        assert!(
            !part.params.is_empty() && part.params.len() % 16 == 0,
            "{}'s params are {} bytes, not a positive multiple of 16",
            part.name,
            part.params.len()
        );
        preludes.srgb |= part.preludes.srgb;
        preludes.luma |= part.preludes.luma;
        preludes.oklab |= part.preludes.oklab;
        needs_position |= part.needs_position;
        let base = tables.len();
        for t in &part.tables {
            tables.extend_from_slice(t);
        }
        params.extend_from_slice(&part.params);
        members.push_str(&format!("    s{k}: Params_{},\n", part.name));
        if declared.insert(part.name) {
            bodies.push_str(&part.wgsl);
            bodies.push('\n');
        }
        calls.push_str(&format!(
            "    p = apply_{}(p, x, y, P.s{k}, {base}u);\n",
            part.name
        ));
    }
    // ⚠ wgpu refuses a zero-sized buffer, the guard `Dispatch` and `GpuImage::scratch` carry.
    if tables.is_empty() {
        tables.push(0.0);
    }
    let position = if needs_position {
        "    let x = pixel % P.hdr.w;\n    let y = pixel / P.hdr.w;\n"
    } else {
        "    let x = 0u;\n    let y = 0u;\n"
    };
    let shader = format!(
        "{srgb}{luma}{oklab}{bodies}
struct Header {{
    pixels: u32,
    row_groups: u32,
    w: u32,
    pad: u32,
}};
struct Params {{
    hdr: Header,
{members}}};
@group(0) @binding(0) var<storage, read_write> data: array<f32>;
@group(0) @binding(1) var<uniform> P: Params;
@group(0) @binding(2) var<storage, read> tables: array<f32>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {{
    let pixel = id.y * (P.hdr.row_groups * 64u) + id.x;
    if (pixel >= P.hdr.pixels) {{ return; }}
{position}    let i = pixel * 3u;
    var p = vec3<f32>(data[i], data[i + 1u], data[i + 2u]);
{calls}    data[i] = p.x;
    data[i + 1u] = p.y;
    data[i + 2u] = p.z;
}}
",
        srgb = if preludes.srgb {
            crate::srgb_wgsl::SRGB_WGSL
        } else {
            ""
        },
        luma = if preludes.luma {
            crate::luma_wgsl::luma_wgsl()
        } else {
            String::new()
        },
        oklab = if preludes.oklab {
            crate::oklab_wgsl::OKLAB_WGSL
        } else {
            ""
        },
    );
    Emitted {
        shader,
        params,
        tables,
    }
}

thread_local! {
    // Fused dispatches encoded on this thread. ⚠ Thread-local for the reason every counter in
    // this crate is: libtest runs tests in parallel. A counter, not a flag, because a flag cannot
    // see its own absence.
    static FUSED_DISPATCHES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Fused dispatches encoded on this thread so far.
///
/// ⚠ The observable for a run INSIDE a mask, which `GpuChain::encode_units` cannot see: the mask
/// is one single unit to the chain, and its sub-stages fuse within its own `encode`.
pub fn fused_dispatches() -> usize {
    FUSED_DISPATCHES.with(|c| c.get())
}

/// Encode one fused dispatch over `parts`.
pub(crate) fn encode_parts(
    parts: &[Part<'_>],
    ctx: &GpuContext,
    pass: &mut wgpu::ComputePass,
    img: &GpuImage,
) {
    FUSED_DISPATCHES.with(|c| c.set(c.get() + 1));
    let pixels = img.pixels();
    let (row_groups, _) = Dispatch::grid(pixels);
    let e = emit(parts, pixels, row_groups, img.width() as u32);
    Dispatch {
        label: "per_pixel",
        shader: e.shader,
        params: e.params,
        storages: &[&e.tables],
    }
    .encode(ctx, pass, img);
}

/// A per-pixel twin's own `encode`: a run of one through the emitter.
///
/// ⚠ What every per-pixel twin's `GpuStage::encode` calls, so the trait stays total and no twin
/// keeps a standalone shader of its own. A twin whose `per_pixel` is `None` for these dimensions
/// — the vignette when inactive — encodes nothing, as its CPU `apply` returns the image untouched.
pub(crate) fn encode_single(
    stage: &dyn GpuStage,
    ctx: &GpuContext,
    pass: &mut wgpu::ComputePass,
    img: &GpuImage,
) {
    if let Some(part) = stage.per_pixel(img.width(), img.height()) {
        encode_parts(std::slice::from_ref(&part), ctx, pass, img);
    }
}

/// Whether the environment switched fusion off. Read once.
pub fn fusion_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("RUSTROOM_NO_FUSION").is_none())
}

/// One thing a chain encodes: a fused run of per-pixel twins, or one other stage.
pub(crate) enum Unit<'a> {
    Fused(Vec<Part<'a>>),
    Single(&'a dyn GpuStage),
}

impl Unit<'_> {
    /// The names, joined — what a timed run labels its pass with.
    pub(crate) fn label(&self) -> String {
        match self {
            Unit::Fused(parts) => parts.iter().map(|p| p.name).collect::<Vec<_>>().join("+"),
            Unit::Single(s) => s.name().to_owned(),
        }
    }

    /// This unit's output size. A fused run never changes it.
    pub(crate) fn output_dims(&self, w: usize, h: usize) -> (usize, usize) {
        match self {
            Unit::Fused(_) => (w, h),
            Unit::Single(s) => s.output_dims(w, h),
        }
    }

    pub(crate) fn encode(
        &self,
        ctx: &GpuContext,
        pass: &mut wgpu::ComputePass,
        img: &GpuImage,
        scratch: &[wgpu::Buffer],
    ) {
        match self {
            Unit::Fused(parts) => encode_parts(parts, ctx, pass, img),
            Unit::Single(s) => s.encode(ctx, pass, img, scratch),
        }
    }
}

/// The unit starting at `stages[i]` for a `w` x `h` image, and the index after it.
///
/// ⚠ Decided at the current dimensions, because a size-changing stage earlier in the chain may
/// have relabelled the image, and a part is built for the size it will run at. With `fuse` off,
/// a per-pixel twin is a fused run of one — the same emitter, so the A/B measures the grouping
/// and nothing else.
pub(crate) fn next_unit<'a>(
    stages: &'a [Box<dyn GpuStage>],
    i: usize,
    w: usize,
    h: usize,
    fuse: bool,
) -> (Unit<'a>, usize) {
    match stages[i].per_pixel(w, h) {
        None => (Unit::Single(stages[i].as_ref()), i + 1),
        Some(first) => {
            let mut parts = vec![first];
            let mut j = i + 1;
            if fuse {
                while j < stages.len() {
                    match stages[j].per_pixel(w, h) {
                        Some(p) => {
                            parts.push(p);
                            j += 1;
                        }
                        None => break,
                    }
                }
            }
            (Unit::Fused(parts), j)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part<'a>(name: &'static str, params: Vec<u8>, table: &'a [f32]) -> Part<'a> {
        Part {
            name,
            wgsl: format!(
                "struct Params_{name} {{ a: f32, b: f32, c: f32, d: f32 }};\n\
                 fn apply_{name}(p: vec3<f32>, x: u32, y: u32, q: Params_{name}, base: u32) \
                 -> vec3<f32> {{ return p; }}"
            ),
            params,
            tables: vec![table],
            preludes: Preludes::default(),
            needs_position: false,
        }
    }

    /// The header first, then each member's bytes at the next 16-byte boundary; tables laid down
    /// in order with each member's base passed as a literal; members called in list order.
    #[test]
    fn emit_lays_out_params_tables_and_calls_in_order() {
        let ta = [1.0f32, 2.0, 3.0];
        let tb = [7.0f32];
        let parts = [
            part("aa", vec![1u8; 16], &ta),
            part("bb", vec![2u8; 32], &tb),
        ];
        let e = emit(&parts, 100, 2, 10);
        assert_eq!(e.params.len(), 16 + 16 + 32);
        assert_eq!(&e.params[0..4], &100u32.to_le_bytes());
        assert_eq!(&e.params[4..8], &2u32.to_le_bytes());
        assert_eq!(&e.params[8..12], &10u32.to_le_bytes());
        assert!(e.params[16..32].iter().all(|&b| b == 1));
        assert!(e.params[32..64].iter().all(|&b| b == 2));
        assert_eq!(e.tables, vec![1.0, 2.0, 3.0, 7.0]);
        let a = e
            .shader
            .find("p = apply_aa(p, x, y, P.s0, 0u);")
            .expect("aa called first");
        let b = e
            .shader
            .find("p = apply_bb(p, x, y, P.s1, 3u);")
            .expect("bb's base is 3");
        assert!(a < b, "members are called in list order");
        assert!(e
            .shader
            .contains("    s0: Params_aa,\n    s1: Params_bb,\n"));
        assert!(
            !e.shader.contains("let x = pixel %"),
            "no member needs position"
        );
    }

    /// A part that needs position makes `main` derive x and y; no tables still binds one word.
    #[test]
    fn emit_derives_position_only_when_asked_and_never_binds_an_empty_table() {
        let mut p = part("vv", vec![0u8; 16], &[]);
        p.needs_position = true;
        let e = emit(std::slice::from_ref(&p), 1, 1, 1);
        assert!(e.shader.contains("let x = pixel % P.hdr.w;"));
        assert_eq!(e.tables, vec![0.0]);
    }

    /// The same stage twice in one run declares its struct and function once and is called
    /// twice, each call with its own parameters.
    #[test]
    fn emit_declares_a_repeated_stage_once_and_calls_it_twice() {
        let t: [f32; 0] = [];
        let parts = [part("ee", vec![1u8; 16], &t), part("ee", vec![2u8; 16], &t)];
        let e = emit(&parts, 1, 1, 1);
        assert_eq!(e.shader.matches("struct Params_ee").count(), 1);
        assert_eq!(e.shader.matches("fn apply_ee(").count(), 1);
        assert!(e.shader.contains("p = apply_ee(p, x, y, P.s0, 0u);"));
        assert!(e.shader.contains("p = apply_ee(p, x, y, P.s1, 0u);"));
        assert_eq!(e.params.len(), 16 + 32);
    }

    /// A part whose params are not a multiple of 16 is a bug in that part, caught here.
    #[test]
    #[should_panic(expected = "not a positive multiple of 16")]
    fn emit_refuses_misaligned_params() {
        let p = part("xx", vec![0u8; 12], &[]);
        let _ = emit(std::slice::from_ref(&p), 1, 1, 1);
    }
}
