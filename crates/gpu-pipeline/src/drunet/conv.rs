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
            Mode::Product => (self.in_w, self.in_h),
        }
    }

    /// The product's rows, columns and depth.
    pub(crate) fn mnk(&self, mode: Mode) -> (u32, u32, u32) {
        let (ow, oh) = self.out(mode);
        match mode {
            Mode::Conv3 => (ow * oh, self.cout, 9 * self.cin),
            Mode::Down2 => (ow * oh, self.cout, 4 * self.cin),
            Mode::Up2 => (self.in_w * self.in_h, 4 * self.cout, self.cin),
            Mode::Product => (self.in_w * self.in_h, self.cout, self.cin),
        }
    }
}

/// The kernel's `Params`, in its field order — `g0` the band's first row-workgroup.
pub(crate) fn uniform(
    mode: Mode,
    d: &Dims,
    relu: bool,
    add1: bool,
    add2: bool,
    g0: u32,
) -> [u32; 12] {
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
        g0,
    ]
}

/// Multiply-adds a submission — ~2.7 ms of the M2. Each convolution runs as bands of row-workgroups this
/// size or smaller, two in flight, so an editing frame on the same queue waits behind two bands at most.
///
/// ⚠ Measured (`editing_frames_while_a_tile_runs`, an editing frame 6.3 ms idle; `a_full_size_tile_time`):
///
/// | submissions | an editing frame during a tile | a tile |
/// |---|---|---|
/// | a tile's 7 levels at once | 1,354 ms | 1.35 s |
/// | one convolution each, waited for | 51 ms | 1.42 s |
/// | bands of 2³⁰, each waited for | 12 ms | 1.91 s |
/// | **bands of 2³⁰, two in flight (this)** | **15 ms** | **1.46 s** |
/// | bands of 2³¹, two in flight | 24 ms | 1.41 s |
pub(crate) const SUBMISSION_MACS: u64 = 1 << 30;

/// `groups.0` split into bands of at most [`SUBMISSION_MACS`]: each `(first, count)`.
pub(crate) fn bands(mode: Mode, d: &Dims) -> Vec<(u32, u32)> {
    let (m, n, k) = d.mnk(mode);
    let (gx, _) = groups(mode, d);
    // ⚠ Winograd's product is 16 products a dispatch.
    let batch = if mode == Mode::Product { 16 } else { 1 };
    let macs = m as u64 * n as u64 * k as u64 * batch;
    let count = macs.div_ceil(SUBMISSION_MACS).clamp(1, gx as u64) as u32;
    let size = gx.div_ceil(count);
    (0..gx)
        .step_by(size as usize)
        .map(|g0| (g0, size.min(gx - g0)))
        .collect()
}

/// Workgroups: `TM` rows by `TN` columns each — the kernel's tile.
pub(crate) fn groups(mode: Mode, d: &Dims) -> (u32, u32) {
    let (m, n, _) = d.mnk(mode);
    (m.div_ceil(super::wgsl::TM), n.div_ceil(super::wgsl::TN))
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

/// `mode`'s pipeline and its layout, storing in `f16` when `half` — compiled once, kept by the context's
/// cache (keyed on the text).
pub(crate) fn kernel(
    ctx: &GpuContext,
    mode: Mode,
    half: bool,
) -> (wgpu::BindGroupLayout, wgpu::ComputePipeline) {
    let (layout, mut pipelines) = ctx.cache.program(
        &ctx.device,
        "drunet-conv",
        &super::wgsl::shader(mode, half),
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

/// A bind group of `buffers` in binding order — the Winograd transforms' (their last is the uniform).
pub(crate) fn bind_n(
    ctx: &GpuContext,
    layout: &wgpu::BindGroupLayout,
    buffers: &[&wgpu::Buffer],
) -> wgpu::BindGroup {
    let entries: Vec<wgpu::BindGroupEntry<'_>> = buffers
        .iter()
        .enumerate()
        .map(|(i, b)| wgpu::BindGroupEntry {
            binding: i as u32,
            resource: b.as_entire_binding(),
        })
        .collect();
    ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("drunet-wino-bind"),
        layout,
        entries: &entries,
    })
}

/// A storage buffer holding `data` — as `f16` when `half`.
pub(crate) fn upload(
    ctx: &GpuContext,
    label: &'static str,
    data: &[f32],
    half: bool,
) -> wgpu::Buffer {
    let b = zeros(ctx, label, data.len(), half);
    if half {
        // ⚠ Padded to an even count: a write must be a multiple of 4 bytes.
        let mut h: Vec<u16> = data
            .iter()
            .map(|v| half::f16::from_f32(*v).to_bits())
            .collect();
        if h.len() % 2 == 1 {
            h.push(0);
        }
        ctx.queue.write_buffer(&b, 0, bytemuck::cast_slice(&h));
    } else {
        ctx.queue.write_buffer(&b, 0, bytemuck::cast_slice(data));
    }
    b
}

/// A storage buffer of `len` values — `f16` (2 bytes) when `half`, else `f32` — written by a kernel, read
/// by the next or copied out. ⚠ Rounded up to 4 bytes: a buffer's size must be a multiple of four.
pub(crate) fn zeros(ctx: &GpuContext, label: &'static str, len: usize, half: bool) -> wgpu::Buffer {
    let bytes = len.max(1) * if half { 2 } else { 4 };
    ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: bytes.div_ceil(4) as u64 * 4,
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

/// `len` values of `buf` read back (stored as `f16` when `half`) — after everything submitted before it.
/// `None` on a failed map.
pub(crate) fn read(
    ctx: &GpuContext,
    buf: &wgpu::Buffer,
    len: usize,
    half: bool,
) -> Option<Vec<f32>> {
    let bytes = (len * if half { 2 } else { 4 }).div_ceil(4) as u64 * 4;
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
    let data: Vec<f32> = if half {
        let raw: Vec<u16> = bytemuck::cast_slice(&slice.get_mapped_range()).to_vec();
        raw.iter()
            .take(len)
            .map(|b| half::f16::from_bits(*b).to_f32())
            .collect()
    } else {
        bytemuck::cast_slice(&slice.get_mapped_range()).to_vec()
    };
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

    /// ⚠ Fails on a non-finite value first: `f32::max` ignores NaN, so an all-NaN output would read as
    /// "no difference" (it did, on the real network — shape (b): a query that finds nothing).
    pub(crate) fn close(a: &[f32], b: &[f32]) -> f32 {
        assert_eq!(a.len(), b.len());
        let bad = a.iter().chain(b).filter(|v| !v.is_finite()).count();
        assert_eq!(bad, 0, "{bad} non-finite values");
        a.iter()
            .zip(b)
            .map(|(x, y)| (x - y).abs())
            .fold(0.0, f32::max)
    }

    /// The precisions this adapter runs: f32 always, f16 where it offers `SHADER_F16`.
    pub(crate) fn precisions(ctx: &GpuContext) -> Vec<bool> {
        if ctx.f16() {
            vec![false, true]
        } else {
            eprintln!("f16: not offered by this adapter — f32 only");
            vec![false]
        }
    }

    /// f32: 1e-4 absolute; f16: 3e-3 of the reference's largest value (half precision keeps ~3 digits).
    fn tolerance(half: bool, want: &[f32]) -> f32 {
        if half {
            3e-3 * want.iter().fold(1.0f32, |m, v| m.max(v.abs()))
        } else {
            1e-4
        }
    }

    fn weight(mode: Mode, d: &Dims) -> Tensor {
        let (cin, cout) = (d.cin as usize, d.cout as usize);
        let shape = match mode {
            Mode::Conv3 => vec![cout, cin, 3, 3],
            Mode::Down2 => vec![cout, cin, 2, 2],
            Mode::Up2 => vec![cin, cout, 2, 2],
            Mode::Product => unreachable!("the product is tested through the network"),
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
        half: bool,
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
        let s = upload(ctx, "test-src", src, half);
        let k = upload(
            ctx,
            "test-weights",
            &crate::drunet::pack::pack(mode, w),
            half,
        );
        let dst = zeros(ctx, "test-dst", out_len, half);
        let r1 = res1.map(|r| upload(ctx, "test-res1", r, half));
        let r2 = res2.map(|r| upload(ctx, "test-res2", r, half));
        let none = zeros(ctx, "test-none", 1, half);
        let params = uniform_buffer(ctx, &uniform(mode, d, relu, r1.is_some(), r2.is_some(), 0));
        let (layout, pipeline) = kernel(ctx, mode, half);
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
        read(ctx, &dst, out_len, half).expect("read back")
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
            for half in precisions(&ctx) {
                let got = run_one(&ctx, half, mode, &d, &src, &w, false, None, None);
                let worst = close(&got, &want);
                let tol = tolerance(half, &want);
                assert!(worst < tol, "{mode:?} {d:?} half {half}: {worst} ≥ {tol}");
            }
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
        for half in precisions(&ctx) {
            let relu = run_one(&ctx, half, Mode::Conv3, &d, &src, &w, true, None, None);
            let want: Vec<f32> = plain.iter().map(|v| v.max(0.0)).collect();
            assert!(close(&relu, &want) < tolerance(half, &want), "half {half}");
            let added = run_one(
                &ctx,
                half,
                Mode::Conv3,
                &d,
                &src,
                &w,
                false,
                Some(&r1),
                Some(&r2),
            );
            let want: Vec<f32> = plain
                .iter()
                .zip(&r1)
                .zip(&r2)
                .map(|((v, a), b)| v + a + b)
                .collect();
            assert!(close(&added, &want) < tolerance(half, &want), "half {half}");
        }
    }
}
