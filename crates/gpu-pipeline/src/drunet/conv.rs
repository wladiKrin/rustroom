//! One convolution on the GPU: the kernel's pipeline, its binding, its uniform and its dispatch, and
//! the buffers it runs on.

use super::arch::Mode;
use crate::GpuContext;

/// One convolution's sizes: its input plane and channels in and out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Dims {
    pub in_w: u32,
    pub in_h: u32,
    pub cin: u32,
    pub cout: u32,
}

impl Dims {
    /// The output plane's size.
    pub(crate) fn out(&self, mode: Mode) -> (u32, u32) {
        match mode {
            Mode::Conv3 => (self.in_w, self.in_h),
            Mode::Down2 => (self.in_w / 2, self.in_h / 2),
            Mode::Up2 => (self.in_w * 2, self.in_h * 2),
        }
    }

    /// The product's rows, columns and depth.
    pub(crate) fn mnk(&self, mode: Mode) -> (u32, u32, u32) {
        let (ow, oh) = self.out(mode);
        match mode {
            Mode::Conv3 => (ow * oh, self.cout, 9 * self.cin),
            Mode::Down2 => (ow * oh, self.cout, 4 * self.cin),
            Mode::Up2 => (self.in_w * self.in_h, 4 * self.cout, self.cin),
        }
    }
}

/// The kernel's `Params`, in its field order.
pub(crate) fn uniform(mode: Mode, d: &Dims, relu: bool, add1: bool, add2: bool) -> [u32; 12] {
    let (ow, oh) = d.out(mode);
    let (m, n, k) = d.mnk(mode);
    [
        d.in_w,
        d.in_h,
        ow,
        oh,
        m,
        n,
        k,
        d.cout,
        relu as u32,
        add1 as u32,
        add2 as u32,
        0,
    ]
}

/// Workgroups: 64 rows by 64 columns each.
pub(crate) fn groups(mode: Mode, d: &Dims) -> (u32, u32) {
    let (m, n, _) = d.mnk(mode);
    (m.div_ceil(64), n.div_ceil(64))
}

/// The binding: the source, the packed weights, the destination, two residuals, the parameters.
pub(crate) fn conv_layout() -> [wgpu::BindGroupLayoutEntry; 6] {
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
    let ro = wgpu::BufferBindingType::Storage { read_only: true };
    [
        entry(0, ro),
        entry(1, ro),
        entry(2, wgpu::BufferBindingType::Storage { read_only: false }),
        entry(3, ro),
        entry(4, ro),
        entry(5, wgpu::BufferBindingType::Uniform),
    ]
}

/// `mode`'s pipeline and its layout — compiled once, kept by the context's cache (keyed on the text).
pub(crate) fn kernel(
    ctx: &GpuContext,
    mode: Mode,
) -> (wgpu::BindGroupLayout, wgpu::ComputePipeline) {
    let (layout, mut pipelines) = ctx.cache.program(
        &ctx.device,
        "drunet-conv",
        &super::wgsl::shader(mode),
        &conv_layout(),
        &["main"],
    );
    (layout, pipelines.remove(0))
}

/// One convolution's bind group. ⚠ `res1`/`res2` must never be `dst`: a buffer bound for writing and
/// reading in one dispatch is a validation error — an unused residual binds a small spare buffer.
pub(crate) fn bind(
    ctx: &GpuContext,
    layout: &wgpu::BindGroupLayout,
    buffers: [&wgpu::Buffer; 5],
    params: &wgpu::Buffer,
) -> wgpu::BindGroup {
    let entries: Vec<wgpu::BindGroupEntry<'_>> = buffers
        .into_iter()
        .chain([params])
        .enumerate()
        .map(|(i, b)| wgpu::BindGroupEntry {
            binding: i as u32,
            resource: b.as_entire_binding(),
        })
        .collect();
    ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("drunet-conv-bind"),
        layout,
        entries: &entries,
    })
}

/// A storage buffer holding `data`.
pub(crate) fn upload(ctx: &GpuContext, label: &'static str, data: &[f32]) -> wgpu::Buffer {
    let b = zeros(ctx, label, data.len());
    ctx.queue.write_buffer(&b, 0, bytemuck::cast_slice(data));
    b
}

/// A storage buffer of `len` floats — written by a kernel, read by the next or copied out.
pub(crate) fn zeros(ctx: &GpuContext, label: &'static str, len: usize) -> wgpu::Buffer {
    ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: (len.max(1) * 4) as u64,
        usage: wgpu::BufferUsages::STORAGE
            | wgpu::BufferUsages::COPY_DST
            | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    })
}

/// The kernel's parameters as a uniform buffer.
pub(crate) fn uniform_buffer(ctx: &GpuContext, params: &[u32; 12]) -> wgpu::Buffer {
    let b = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("drunet-params"),
        size: 48,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    ctx.queue.write_buffer(&b, 0, bytemuck::cast_slice(params));
    b
}

/// `len` floats of `buf` read back — after everything submitted before it. `None` on a failed map.
pub(crate) fn read(ctx: &GpuContext, buf: &wgpu::Buffer, len: usize) -> Option<Vec<f32>> {
    let bytes = (len * 4) as u64;
    let readback = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("drunet-readback"),
        size: bytes,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    enc.copy_buffer_to_buffer(buf, 0, &readback, 0, bytes);
    ctx.queue.submit(Some(enc.finish()));
    let slice = readback.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |r| {
        let _ = tx.send(r);
    });
    let _ = ctx.device.poll(wgpu::PollType::Wait {
        submission_index: None,
        timeout: None,
    });
    rx.recv().ok()?.ok()?;
    let data: Vec<f32> = bytemuck::cast_slice(&slice.get_mapped_range()).to_vec();
    readback.unmap();
    Some(data)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::drunet::arch::{Mode, Tensor};
    use crate::drunet::reference;
    use crate::GpuContext;
    use std::sync::Arc;

    pub(crate) fn ctx() -> Option<Arc<GpuContext>> {
        match crate::shared() {
            Some(c) => Some(c.clone()),
            None => {
                eprintln!("SKIPPED: no GPU adapter on this machine");
                None
            }
        }
    }

    /// Values in −0.5…0.5 from a seed — xorshift, so a test's data never moves.
    pub(crate) fn rand(n: usize, seed: u32) -> Vec<f32> {
        let mut s = seed.wrapping_mul(2_654_435_761).max(1);
        (0..n)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                (s as f32 / u32::MAX as f32) - 0.5
            })
            .collect()
    }

    fn close(a: &[f32], b: &[f32]) -> f32 {
        assert_eq!(a.len(), b.len());
        a.iter()
            .zip(b)
            .map(|(x, y)| (x - y).abs())
            .fold(0.0, f32::max)
    }

    fn weight(mode: Mode, d: &Dims) -> Tensor {
        let (cin, cout) = (d.cin as usize, d.cout as usize);
        let shape = match mode {
            Mode::Conv3 => vec![cout, cin, 3, 3],
            Mode::Down2 => vec![cout, cin, 2, 2],
            Mode::Up2 => vec![cin, cout, 2, 2],
        };
        let len = shape.iter().product();
        Tensor {
            shape,
            data: rand(len, 17),
        }
    }

    /// One convolution through the production pieces — the pipeline, the pack, the uniform, the
    /// groups — and its output read back.
    #[allow(clippy::too_many_arguments)]
    fn run_one(
        ctx: &GpuContext,
        mode: Mode,
        d: &Dims,
        src: &[f32],
        w: &Tensor,
        relu: bool,
        res1: Option<&[f32]>,
        res2: Option<&[f32]>,
    ) -> Vec<f32> {
        let (ow, oh) = d.out(mode);
        let out_len = (d.cout * ow * oh) as usize;
        let s = upload(ctx, "test-src", src);
        let k = upload(ctx, "test-weights", &crate::drunet::pack::pack(mode, w));
        let dst = zeros(ctx, "test-dst", out_len);
        let r1 = res1.map(|r| upload(ctx, "test-res1", r));
        let r2 = res2.map(|r| upload(ctx, "test-res2", r));
        let none = zeros(ctx, "test-none", 1);
        let params = uniform_buffer(ctx, &uniform(mode, d, relu, r1.is_some(), r2.is_some()));
        let (layout, pipeline) = kernel(ctx, mode);
        let bind = bind(
            ctx,
            &layout,
            [
                &s,
                &k,
                &dst,
                r1.as_ref().unwrap_or(&none),
                r2.as_ref().unwrap_or(&none),
            ],
            &params,
        );
        let mut enc = ctx.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bind, &[]);
            let (gx, gy) = groups(mode, d);
            pass.dispatch_workgroups(gx, gy, 1);
        }
        ctx.queue.submit(Some(enc.finish()));
        read(ctx, &dst, out_len).expect("read back")
    }

    /// Each mode against the naive reference, on sizes and channel counts that are not multiples of the
    /// tile — the head's 4 inputs, the tail's 3 outputs, 70 columns past one 64-wide tile.
    #[test]
    fn each_mode_matches_the_reference() {
        let Some(ctx) = ctx() else { return };
        for (mode, d) in [
            (
                Mode::Conv3,
                Dims {
                    in_w: 13,
                    in_h: 7,
                    cin: 4,
                    cout: 70,
                },
            ),
            (
                Mode::Conv3,
                Dims {
                    in_w: 9,
                    in_h: 11,
                    cin: 5,
                    cout: 3,
                },
            ),
            (
                Mode::Down2,
                Dims {
                    in_w: 10,
                    in_h: 6,
                    cin: 6,
                    cout: 9,
                },
            ),
            (
                Mode::Up2,
                Dims {
                    in_w: 5,
                    in_h: 3,
                    cin: 7,
                    cout: 70,
                },
            ),
        ] {
            let w = weight(mode, &d);
            let src = rand((d.cin * d.in_w * d.in_h) as usize, 3);
            let want = reference::run(mode, &src, &d, &w);
            let got = run_one(&ctx, mode, &d, &src, &w, false, None, None);
            let worst = close(&got, &want);
            assert!(worst < 1e-4, "{mode:?} {d:?}: {worst}");
        }
    }

    /// ReLU, then the two adds, in the store.
    #[test]
    fn the_store_fuses_relu_and_both_adds() {
        let Some(ctx) = ctx() else { return };
        let d = Dims {
            in_w: 8,
            in_h: 8,
            cin: 16,
            cout: 16,
        };
        let w = weight(Mode::Conv3, &d);
        let src = rand(16 * 64, 5);
        let (r1, r2) = (rand(16 * 64, 7), rand(16 * 64, 9));
        let plain = reference::run(Mode::Conv3, &src, &d, &w);
        assert!(
            plain.iter().any(|v| *v < -0.01),
            "the fixture has negatives to clamp"
        );
        let relu = run_one(&ctx, Mode::Conv3, &d, &src, &w, true, None, None);
        let want: Vec<f32> = plain.iter().map(|v| v.max(0.0)).collect();
        assert!(close(&relu, &want) < 1e-4);
        let added = run_one(&ctx, Mode::Conv3, &d, &src, &w, false, Some(&r1), Some(&r2));
        let want: Vec<f32> = plain
            .iter()
            .zip(&r1)
            .zip(&r2)
            .map(|((v, a), b)| v + a + b)
            .collect();
        assert!(close(&added, &want) < 1e-4);
    }
}
