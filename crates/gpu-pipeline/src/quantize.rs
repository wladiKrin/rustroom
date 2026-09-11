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
/// explicitly. `v != v` is true only for NaN.
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

fn q(v: f32) -> u32 {
    let x = select(v, 0.0, v != v);
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

/// Record the quantize dispatch, writing packed RGBA8 into `dst`.
///
/// ⚠ `dst` is a **separate** buffer rather than the image's own. Packing in place would have
/// invocation `i` write index `i` while invocation `j > i / 3` still needs to read index `i` — a
/// read/write hazard between invocations of one dispatch, which no barrier can order.
pub(crate) fn encode(
    ctx: &GpuContext,
    pass: &mut wgpu::ComputePass,
    img: &GpuImage,
    dst: &wgpu::Buffer,
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
    let layout_entries = [
        entry(0, wgpu::BufferBindingType::Storage { read_only: true }),
        entry(1, wgpu::BufferBindingType::Uniform),
        entry(2, wgpu::BufferBindingType::Storage { read_only: false }),
    ];
    // ⚠ Through the cache, like every other program: keyed on the shader text, so this compiles
    // once per process rather than once per render.
    let (layout, pipelines) =
        ctx.cache
            .program(device, "quantize", SHADER, &layout_entries, &["main"]);

    let params: [u32; 4] = [img.pixels(), row_groups, 0, 0];
    let uniform = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("quantize"),
        size: 16,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    ctx.queue
        .write_buffer(&uniform, 0, bytemuck::cast_slice(&params));

    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("quantize"),
        layout: &layout,
        entries: &[
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
        ],
    });
    pass.set_pipeline(&pipelines[0]);
    pass.set_bind_group(0, &bind, &[]);
    pass.dispatch_workgroups(row_groups, col_groups, 1);
}
