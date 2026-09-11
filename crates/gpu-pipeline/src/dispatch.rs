//! The dispatch every per-pixel stage repeats, written once.
//!
//! ⚠⚠ **Extracted rather than copied a fourth time.** `hsl` and `grading` carried the same ~120
//! lines of layout/bind/pipeline/dispatch, differing only in a label, a shader and how many
//! read-only storage buffers they bind; M8.3e adds four more stages of the same shape. Every
//! oversized file in this repo got that way one reasonable-seeming addition at a time.
//!
//! ⚠ `sharpen` is deliberately **not** a client. It encodes up to eight dispatches over shared
//! scratch planes, which is a different thing wearing a similar name, and folding it in here
//! would make this type carry a second shape for one caller.

use crate::{GpuContext, GpuImage};

const WORKGROUP: u32 = 64;

/// ⚠⚠ 65535 workgroups per dimension, so the dispatch is 2-D from the start. A 1-D dispatch
/// passes every small fixture and fails on the first real photograph — a 26 Mpx rung needs
/// 407,000 groups.
const MAX_GROUPS_PER_DIM: u32 = 65535;

/// One per-pixel compute dispatch over the image buffer.
///
/// Bindings are fixed by convention: the image at 0, the uniform at 1, and any read-only
/// storage buffers from 2 upward in the order given.
pub(crate) struct Dispatch<'a> {
    pub label: &'static str,
    /// Full WGSL, with any prelude already concatenated by the caller.
    pub shader: String,
    /// The uniform at binding 1. ⚠ Must be a multiple of 16 bytes.
    pub params: Vec<u8>,
    /// Read-only storage buffers, bound from binding 2 upward.
    pub storages: &'a [&'a [f32]],
}

impl Dispatch<'_> {
    /// The 2-D workgroup split: `(row_groups, col_groups)`.
    ///
    /// ⚠ Computed here rather than in five places that could disagree about it — and `row_groups`
    /// is **also** what every shader needs in its uniform to recover the pixel index, so a caller
    /// that split it differently from the dispatch would read the wrong pixel.
    pub fn grid(pixels: u32) -> (u32, u32) {
        let total = pixels.div_ceil(WORKGROUP);
        let row_groups = total.min(MAX_GROUPS_PER_DIM);
        (row_groups, total.div_ceil(row_groups))
    }

    pub fn encode(self, ctx: &GpuContext, pass: &mut wgpu::ComputePass, img: &GpuImage) {
        let device = &ctx.device;
        let (row_groups, col_groups) = Self::grid(img.pixels());

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
            entry(0, wgpu::BufferBindingType::Storage { read_only: false }),
            entry(1, wgpu::BufferBindingType::Uniform),
        ];
        for i in 0..self.storages.len() as u32 {
            layout_entries.push(entry(
                2 + i,
                wgpu::BufferBindingType::Storage { read_only: true },
            ));
        }

        // ⚠⚠ The module, the layout and the pipeline come from the cache; the uniform, the
        // storages and the bind group do **not**. Measured at 3200x2136: the three cached objects
        // are the ~0.1-0.15 ms per stage that this removes, while the uniform and the LUTs cost
        // 0.01-0.03 ms and carry **this render's edits**, so caching them would add an
        // invalidation risk whose failure mode is a wrong picture rather than a slow one.
        let (layout, pipelines) =
            ctx.cache
                .program(device, self.label, &self.shader, &layout_entries, &["main"]);

        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(self.label),
            size: self.params.len() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        ctx.queue.write_buffer(&uniform, 0, &self.params);

        // ⚠ `.max(4)` because wgpu refuses a zero-sized buffer, the same guard `GpuImage::scratch`
        // carries — a stage may legitimately hand over an empty table.
        let buffers: Vec<wgpu::Buffer> = self
            .storages
            .iter()
            .map(|s| {
                let b = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(self.label),
                    size: (s.len() * 4).max(4) as u64,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                ctx.queue.write_buffer(&b, 0, bytemuck::cast_slice(s));
                b
            })
            .collect();

        let mut bind_entries = vec![
            wgpu::BindGroupEntry {
                binding: 0,
                resource: img.buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: uniform.as_entire_binding(),
            },
        ];
        for (i, b) in buffers.iter().enumerate() {
            bind_entries.push(wgpu::BindGroupEntry {
                binding: 2 + i as u32,
                resource: b.as_entire_binding(),
            });
        }
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(self.label),
            layout: &layout,
            entries: &bind_entries,
        });

        pass.set_pipeline(&pipelines[0]);
        pass.set_bind_group(0, &bind, &[]);
        pass.dispatch_workgroups(row_groups, col_groups, 1);
    }
}
