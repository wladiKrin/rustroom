//! The last dispatch: display-referred floats to packed RGBA8, without going through the CPU.
//!
//! ⚠⚠ **Not a `GpuStage`, deliberately.** It replaces no CPU stage, it writes to a *different*
//! buffer, and it runs only when the chain is downloading bytes — so giving it that trait would
//! mean a `cpu()` twin that does not exist and a `cpu_cost` the cost gate must never see.

use crate::{GpuContext, GpuImage};

/// ⚠⚠ **`floor(v * 255.0 + 0.5)`, NEVER `round`.** WGSL's `round` breaks ties to the **even**
/// whole number; Rust's `f32::round`, which `ImageBuf::quantize` uses, breaks them **away from
/// zero**. `v * 255` lands on exact halves routinely — `0.5/255`, `1.5/255`, … — so a
/// transliterated `round` is correct on every ordinary value and off by one level on every tie.
///
/// ⚠ **NaN must reach 0, and `clamp` is not what does it.** On the CPU that happens through the
/// saturating `as u8` cast — `f32::clamp` returns NaN for NaN — so the shader tests for it
/// explicitly, ⚠⚠ **on the BITS** (`is_nan`: exponent all ones, a mantissa). wgpu compiles Metal
/// shaders with Metal's default fast math, which lets the compiler assume no float is NaN and fold
/// a self-comparison — on a Mac the display plane's `all(c == c)` guard let a NaN through and the pixel came
/// out green (2026-11-18; Vulkan on Windows kept it). Integer operations carry no such assumption.
///
/// ⚠ `pack4x8unorm` was considered and not used: its rounding is specified as the same
/// `floor(0.5 + 255 × clamp(..))`, but writing the rule out is what lets a reader check it against
/// `ImageBuf::quantize` without consulting the WGSL spec — and the NaN guard is needed either way.
const SHADER: &str = r#"
struct Params {
    pixels: u32,
    row_groups: u32,
    pad0: u32,
    pad1: u32,
};
@group(0) @binding(0) var<storage, read> src: array<f32>;
@group(0) @binding(1) var<uniform> params: Params;
@group(0) @binding(2) var<storage, read_write> dst: array<u32>;

fn is_nan(v: f32) -> bool {
    return (bitcast<u32>(v) & 0x7fffffffu) > 0x7f800000u;
}

fn q(v: f32) -> u32 {
    let x = select(v, 0.0, is_nan(v));
    return u32(floor(clamp(x, 0.0, 1.0) * 255.0 + 0.5));
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let pixel = id.y * (params.row_groups * 64u) + id.x;
    if (pixel >= params.pixels) { return; }
    let p = pixel * 3u;
    dst[pixel] = q(src[p]) | (q(src[p + 1u]) << 8u) | (q(src[p + 2u]) << 16u) | 0xff000000u;
}
"#;

/// The display's plane too (2026-11-18): [`SHADER`]'s sRGB plane, then the same float pixel converted through the
/// display's table and quantized into a second plane starting at `pixels` — one dispatch, one buffer, one download.
///
/// ⚠⚠ The lookup is `color::DisplayProfile::lookup` LINE FOR LINE: clamp, the square root the grid is spaced by,
/// `floor` capped at `n − 2`, the eight corners, `a + (b − a) · t` along red, green, blue. ⚠ A NaN in any channel makes
/// the CPU's fractions NaN in all three outputs, which quantize to 0 — so a pixel holding a NaN is black here, by an
/// explicit test on its bits (`any(is_nan3(c))`, [`SHADER`]'s note says why never `c == c`), never by whatever `clamp`
/// does with a NaN on this GPU.
const DISPLAY_SHADER: &str = r#"
struct Params {
    pixels: u32,
    row_groups: u32,
    n: u32,
    pad1: u32,
};
@group(0) @binding(0) var<storage, read> src: array<f32>;
@group(0) @binding(1) var<uniform> params: Params;
@group(0) @binding(2) var<storage, read_write> dst: array<u32>;
@group(0) @binding(3) var<storage, read> table: array<f32>;

fn is_nan(v: f32) -> bool {
    return (bitcast<u32>(v) & 0x7fffffffu) > 0x7f800000u;
}

fn is_nan3(c: vec3<f32>) -> vec3<bool> {
    return (bitcast<vec3<u32>>(c) & vec3<u32>(0x7fffffffu)) > vec3<u32>(0x7f800000u);
}

fn q(v: f32) -> u32 {
    let x = select(v, 0.0, is_nan(v));
    return u32(floor(clamp(x, 0.0, 1.0) * 255.0 + 0.5));
}

fn pack(c: vec3<f32>) -> u32 {
    return q(c.x) | (q(c.y) << 8u) | (q(c.z) << 16u) | 0xff000000u;
}

fn entry(i: u32) -> vec3<f32> {
    let k = i * 3u;
    return vec3<f32>(table[k], table[k + 1u], table[k + 2u]);
}

fn lookup(rgb: vec3<f32>) -> vec3<f32> {
    let n = params.n;
    let x = sqrt(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0))) * f32(n - 1u);
    let lo = min(vec3<u32>(floor(x)), vec3<u32>(n - 2u));
    let fr = x - vec3<f32>(lo);
    let dy = n;
    let dz = n * n;
    let i = (lo.z * n + lo.y) * n + lo.x;
    let c000 = entry(i);
    let c100 = entry(i + 1u);
    let c010 = entry(i + dy);
    let c110 = entry(i + dy + 1u);
    let c001 = entry(i + dz);
    let c101 = entry(i + dz + 1u);
    let c011 = entry(i + dz + dy);
    let c111 = entry(i + dz + dy + 1u);
    let x00 = c000 + (c100 - c000) * fr.x;
    let x10 = c010 + (c110 - c010) * fr.x;
    let x01 = c001 + (c101 - c001) * fr.x;
    let x11 = c011 + (c111 - c011) * fr.x;
    let a = x00 + (x10 - x00) * fr.y;
    let b = x01 + (x11 - x01) * fr.y;
    return a + (b - a) * fr.z;
}

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let pixel = id.y * (params.row_groups * 64u) + id.x;
    if (pixel >= params.pixels) { return; }
    let p = pixel * 3u;
    let c = vec3<f32>(src[p], src[p + 1u], src[p + 2u]);
    dst[pixel] = pack(c);
    var d = vec3<f32>(0.0);
    if (!any(is_nan3(c))) { d = lookup(c); }
    dst[params.pixels + pixel] = pack(d);
}
"#;

/// Record the quantize dispatch, writing packed RGBA8 into `dst` — and, given a display's table, the display's plane
/// after it ([`DISPLAY_SHADER`]; `dst` is then twice as long).
///
/// ⚠ `dst` is a **separate** buffer rather than the image's own. Packing in place would have
/// invocation `i` write index `i` while invocation `j > i / 3` still needs to read index `i` — a
/// read/write hazard between invocations of one dispatch, which no barrier can order.
pub(crate) fn encode(
    ctx: &GpuContext,
    pass: &mut wgpu::ComputePass,
    img: &GpuImage,
    dst: &wgpu::Buffer,
    display: Option<&color::DisplayProfile>,
) {
    let device = &ctx.device;
    let (row_groups, col_groups) = crate::dispatch::Dispatch::grid(img.pixels());
    let entry = |binding: u32, ty: wgpu::BufferBindingType| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    let mut layout_entries = vec![
        entry(0, wgpu::BufferBindingType::Storage { read_only: true }),
        entry(1, wgpu::BufferBindingType::Uniform),
        entry(2, wgpu::BufferBindingType::Storage { read_only: false }),
    ];
    if display.is_some() {
        layout_entries.push(entry(
            3,
            wgpu::BufferBindingType::Storage { read_only: true },
        ));
    }
    let (label, shader) = match display {
        None => ("quantize", SHADER),
        Some(_) => ("quantize-display", DISPLAY_SHADER),
    };
    // ⚠ Through the cache, like every other program: keyed on the shader text, so this compiles
    // once per process rather than once per render.
    let (layout, pipelines) = ctx
        .cache
        .program(device, label, shader, &layout_entries, &["main"]);

    let n = display.map_or(0, |d| d.n() as u32);
    let params: [u32; 4] = [img.pixels(), row_groups, n, 0];
    let uniform = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("quantize"),
        size: 16,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    ctx.queue
        .write_buffer(&uniform, 0, bytemuck::cast_slice(&params));

    // ⚠ The table uploaded per run, as the per-pixel stages upload theirs: 431 KB at 33³. ⛔ A cached upload was
    // measured and taken out (2026-11-18): the chain's encode span went 0.64 → 0.52 ms, within its noise. The
    // entries are the CPU's own (`DisplayProfile::table`), never sampled a second time.
    let table = display.map(|d| {
        let t = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("quantize-display-table"),
            size: std::mem::size_of_val(d.table()) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        ctx.queue
            .write_buffer(&t, 0, bytemuck::cast_slice(d.table()));
        t
    });
    let mut entries = vec![
        wgpu::BindGroupEntry {
            binding: 0,
            resource: img.buffer.as_entire_binding(),
        },
        wgpu::BindGroupEntry {
            binding: 1,
            resource: uniform.as_entire_binding(),
        },
        wgpu::BindGroupEntry {
            binding: 2,
            resource: dst.as_entire_binding(),
        },
    ];
    if let Some(t) = &table {
        entries.push(wgpu::BindGroupEntry {
            binding: 3,
            resource: t.as_entire_binding(),
        });
    }
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some(label),
        layout: &layout,
        entries: &entries,
    });
    pass.set_pipeline(&pipelines[0]);
    pass.set_bind_group(0, &bind, &[]);
    pass.dispatch_workgroups(row_groups, col_groups, 1);
}

#[cfg(test)]
mod tests {
    use crate::{GpuChain, GpuExposureStage};
    use core_image::ImageBuf;

    /// CAPTURED — the author's M27Q (`color/testdata/README.md`).
    fn m27q() -> &'static color::DisplayProfile {
        let e = color::edid::parse(include_bytes!("../../color/testdata/m27q.edid")).expect("edid");
        color::DisplayProfile::from_primaries("M27Q", &e.primaries).expect("a profile")
    }

    /// Every quantization tie, then a negative, a value above 1 and a NaN in one pixel (pixel 255), then a spread of
    /// colours and as many near black, where a display's curve is steepest.
    fn fixture() -> ImageBuf {
        let mut px: Vec<f32> = Vec::new();
        for k in 0..255u32 {
            let half = (2 * k + 1) as f32 / 510.0;
            px.extend_from_slice(&[half, half, half]);
        }
        px.extend_from_slice(&[-0.25, 1.5, f32::NAN]);
        px.extend_from_slice(&[0.0, 1.0, 0.5]);
        let mut s: u32 = 4242;
        let mut next = || {
            s = s.wrapping_mul(1_103_515_245).wrapping_add(12345);
            (s >> 8) as f32 / (1u32 << 24) as f32
        };
        for _ in 0..2048 {
            px.extend_from_slice(&[next(), next(), next()]);
            px.extend_from_slice(&[next() * 0.08, next() * 0.08, next() * 0.08]);
        }
        let w = px.len() / 3;
        ImageBuf::from_vec(w, 1, px)
    }

    fn chain() -> Option<GpuChain> {
        let Some(ctx) = crate::shared().cloned() else {
            eprintln!("SKIPPED: no GPU adapter on this machine");
            return None;
        };
        // ⚠ An identity chain (0 EV multiplies by exactly 1.0): only the quantize is under test.
        Some(GpuChain::new(
            ctx.clone(),
            vec![Box::new(GpuExposureStage::new(0.0, ctx))],
        ))
    }

    /// ⚠⚠ The display's plane is what the CPU would have converted and quantized — a profile through its own table, a
    /// fixed space through its exact formula — within ONE level, not bit-exact: the blend is a chain of multiply-adds a
    /// GPU compiler may fuse, and WGSL's `sqrt` is not correctly rounded (a fixed space's table is also a third of a
    /// step from its formula, `color::display`). The sRGB plane beside it is the plain quantize's, byte for byte.
    #[test]
    fn the_display_plane_is_what_the_cpu_would_have_converted() {
        let Some(chain) = chain() else { return };
        let img = fixture();
        let plain = chain.run_rgba8(&img, None).expect("the GPU path");
        assert!(plain.display.is_none(), "no table, no second plane");
        let displays = [
            color::Display::Profile(m27q()),
            color::Display::Space(color::OutputSpace::DisplayP3),
            color::Display::Space(color::OutputSpace::AdobeRgb),
        ];
        for display in displays {
            let got = chain
                .run_rgba8(&img, display.gpu_table())
                .expect("the GPU path");
            assert_eq!(got.srgb, plain.srgb, "{display:?}: the sRGB plane moved");
            let shown = got.display.expect("a second plane");
            let want = img.to_rgba8_with(move |c| display.convert(c));
            assert_eq!(shown.len(), want.len(), "byte count");
            let moved = shown
                .iter()
                .zip(&got.srgb)
                .filter(|(a, b)| a.abs_diff(**b) > 5)
                .count();
            assert!(
                moved > 100,
                "premise: {display:?} moves the picture ({moved})"
            );
            let worst = shown.iter().zip(&want).map(|(a, b)| a.abs_diff(*b)).max();
            let off = shown.iter().zip(&want).filter(|(a, b)| a != b).count();
            eprintln!(
                "{display:?}: worst {worst:?} level(s), {off} of {} bytes differ",
                want.len()
            );
            assert!(
                worst <= Some(1),
                "{display:?}: {worst:?} levels from the CPU"
            );
            let nan = 255 * 4;
            assert_eq!(
                &want[nan..nan + 4],
                &[0, 0, 0, 255],
                "the CPU: a NaN pixel is black"
            );
            assert_eq!(
                &shown[nan..nan + 4],
                &[0, 0, 0, 255],
                "a NaN pixel is black"
            );
        }
    }

    /// A run from the resident input — the handoff's and the tail hit's — downloads the same two planes as the
    /// upload it kept.
    #[test]
    fn a_resident_run_downloads_the_same_two_planes() {
        let Some(chain) = chain() else { return };
        let img = fixture();
        let table = color::Display::Profile(m27q()).gpu_table();
        let (first, kept) = chain
            .run_rgba8_keeping_input(&img, table)
            .expect("the GPU path");
        assert!(first.display.is_some(), "fixture check: two planes");
        let again = chain.run_rgba8_from(&kept, table).expect("the GPU path");
        assert_eq!(again, first, "the resident run converts as the upload did");
    }
}
