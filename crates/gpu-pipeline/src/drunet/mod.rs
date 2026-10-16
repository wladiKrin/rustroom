//! DRUNet on the GPU (M10.16b2): the denoise network's 64 convolutions as one tiled product, run tile by
//! tile for the app's denoise job. The weights come in as plain tensors — this crate never parses ONNX.
//! Since M10.16b5 its values are stored in `f16` where the device offers it (sums stay `f32`) and the
//! 3 × 3 convolutions run as Winograd F(2 × 2, 3 × 3) (`winograd`) — no longer the same bits as `tract`,
//! within 1.5e-3 on the real model.

mod arch;
mod conv;
mod pack;
#[cfg(test)]
mod reference;
mod wgsl;
mod winograd;

pub use arch::{Tensor, Weights};

use crate::GpuContext;
use arch::{Arch, Buf, Mode, Op};
use conv::Dims;
use std::collections::HashMap;
use std::sync::Arc;

/// DRUNet on the GPU: its weights uploaded once, its program built once per tile size.
pub struct GpuDrunet {
    ctx: Arc<GpuContext>,
    arch: Arch,
    program: Vec<Op>,
    /// Each convolution's packed weights, in the program's order.
    weights: Vec<wgpu::Buffer>,
    /// One tile size's activations, parameters and bindings — dropped by [`GpuDrunet::release`].
    sized: Option<Sized>,
    /// Stored in `f16` (M10.16b5) — the device's `SHADER_F16`, unless a test asks otherwise.
    half: bool,
    /// The 3 × 3 convolutions as Winograd F(2 × 2, 3 × 3) (M10.16b5).
    winograd: bool,
}

/// What one tile size needs on the GPU.
struct Sized {
    w: usize,
    h: usize,
    bufs: HashMap<Buf, wgpu::Buffer>,
    steps: Vec<Step>,
}

/// One convolution, ready to dispatch as bands — each its own bind group (its `g0`) and row-workgroups.
struct Step {
    pipeline: wgpu::ComputePipeline,
    bands: Vec<(wgpu::BindGroup, u32)>,
    gy: u32,
    /// Winograd's product runs 16 a dispatch; everything else 1.
    gz: u32,
}

/// Whether DRUNet's 3 × 3 convolutions run as Winograd F(2 × 2, 3 × 3) — set by the measurement
/// (M10.16b5, `a_full_size_tile_time`).
const WINOGRAD: bool = true;

/// Whether a `w × h` tile's largest buffer fits the device's limit — scale 0's widest, since each
/// scale doubles the channels and quarters the pixels.
pub(crate) fn fits(arch: &Arch, w: usize, h: usize, max_bytes: u64, winograd: bool) -> bool {
    // ⚠ Winograd's transformed planes are 16 positions × the blocks — ~4× an activation.
    (!winograd || (planes(arch, w, h) * 4) as u64 <= max_bytes)
        && (0..4).all(|s| (arch.widths[s] * (w >> s) * (h >> s) * 4) as u64 <= max_bytes)
        && (4 * w * h * 4) as u64 <= max_bytes
}

/// The values in one Winograd plane of a `w × h` tile: 16 positions × the channels × the 2 × 2 blocks, at
/// the scale needing most — ONE pair (inputs and products) at this size serves every scale, since the
/// convolutions run one after another (a pair per scale held 0.76 GB a 512 px tile against 0.51).
fn planes(arch: &Arch, w: usize, h: usize) -> usize {
    (0..4)
        .map(|s| 16 * arch.widths[s] * (w >> s).div_ceil(2) * (h >> s).div_ceil(2))
        .max()
        .unwrap_or(0)
}

/// The Winograd transforms' binding: the source, the destination, two residuals, the parameters.
fn winograd_layout() -> [wgpu::BindGroupLayoutEntry; 5] {
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
        entry(1, wgpu::BufferBindingType::Storage { read_only: false }),
        entry(2, ro),
        entry(3, ro),
        entry(4, wgpu::BufferBindingType::Uniform),
    ]
}

impl GpuDrunet {
    /// `weights` checked against DRUNet's shape (the first missing or misshapen one named), packed and
    /// uploaded.
    pub fn new(ctx: Arc<GpuContext>, weights: &Weights) -> Result<Self, String> {
        let half = ctx.f16();
        Self::with_precision(ctx, weights, half)
    }

    /// [`GpuDrunet::new`] storing in `f16` when `half` — the tests' way to run both precisions.
    pub fn with_precision(
        ctx: Arc<GpuContext>,
        weights: &Weights,
        half: bool,
    ) -> Result<Self, String> {
        Self::with_options(ctx, weights, half, WINOGRAD)
    }

    /// [`GpuDrunet::with_precision`], and Winograd for the 3 × 3 convolutions or not.
    pub fn with_options(
        ctx: Arc<GpuContext>,
        weights: &Weights,
        half: bool,
        winograd: bool,
    ) -> Result<Self, String> {
        let half = half && ctx.f16();
        let arch = Arch::of(weights)?;
        let program = arch::program(&arch);
        let packed: Vec<Vec<f32>> = program
            .iter()
            .map(|op| {
                let t = &weights[&op.weight];
                if winograd && op.mode == Mode::Conv3 {
                    winograd::transform_weights(t)
                } else {
                    pack::pack(op.mode, t)
                }
            })
            .collect();
        let uploaded = crate::guarded(|| {
            Some(
                packed
                    .iter()
                    .map(|p| conv::upload(&ctx, "drunet-weights", p, half))
                    .collect::<Vec<_>>(),
            )
        })
        .ok_or("the GPU failed")?;
        Ok(Self {
            ctx,
            arch,
            program,
            weights: uploaded,
            sized: None,
            half,
            winograd,
        })
    }

    /// `rgb` (`w × h`, interleaved, 0…1) denoised at noise level `sigma` (0…255 units) — the same layout
    /// back; `tract`'s `Drunet::run` contract. Sides must be multiples of 8.
    pub fn run(&mut self, rgb: &[f32], w: usize, h: usize, sigma: f32) -> Result<Vec<f32>, String> {
        self.run_map(rgb, w, h, &vec![sigma; w * h])
    }

    /// [`GpuDrunet::run`] with a noise level per pixel (`w × h`, 0…255 units) — DRUNet reads σ as a plane,
    /// so the shadows may be given more than the highlights.
    pub fn run_map(
        &mut self,
        rgb: &[f32],
        w: usize,
        h: usize,
        sigma: &[f32],
    ) -> Result<Vec<f32>, String> {
        if sigma.len() != w * h {
            return Err(format!("{} noise levels for a {w} × {h} tile", sigma.len()));
        }
        if w == 0 || h == 0 || !w.is_multiple_of(8) || !h.is_multiple_of(8) {
            return Err(format!("a {w} × {h} tile: sides must be multiples of 8"));
        }
        if rgb.len() != w * h * 3 {
            return Err(format!("{} values for a {w} × {h} tile", rgb.len()));
        }
        if !fits(&self.arch, w, h, self.ctx.max_image_bytes(), self.winograd) {
            return Err(format!("a {w} × {h} tile is too large for this GPU"));
        }
        let out = crate::guarded(|| self.run_inner(rgb, w, h, sigma))
            .ok_or_else(|| "the GPU failed".to_string())?;
        // ⚠ An `f16` overflow is an error, never a tile of NaN — the caller falls back rather than
        // caching it as the photo.
        if out.iter().any(|v| !v.is_finite()) {
            return Err("the network overflowed half precision".to_string());
        }
        Ok(out)
    }

    /// Drop the activations and Winograd's planes — ~0.5 GB for a 512 px tile in `f16`, twice that in
    /// `f32` — and keep the weights.
    pub fn release(&mut self) {
        self.sized = None;
    }

    fn run_inner(&mut self, rgb: &[f32], w: usize, h: usize, sigma: &[f32]) -> Option<Vec<f32>> {
        if self.sized.as_ref().map(|s| (s.w, s.h)) != Some((w, h)) {
            self.sized = None;
            self.sized = Some(self.build(w, h));
        }
        let sized = self.sized.as_ref()?;
        let n = w * h;
        let mut input = vec![0.0; 4 * n];
        for i in 0..n {
            input[3 * n + i] = sigma[i] / 255.0;
            for c in 0..3 {
                input[c * n + i] = rgb[i * 3 + c];
            }
        }
        let ctx = &self.ctx;
        // The input in the network's storage precision — rewritten into the Input buffer in place.
        let staged = conv::upload(ctx, "drunet-input", &input, self.half);
        let mut enc = ctx.device.create_command_encoder(&Default::default());
        enc.copy_buffer_to_buffer(&staged, 0, &sized.bufs[&Buf::Input], 0, staged.size());
        ctx.queue.submit(Some(enc.finish()));
        // ⚠⚠ ONE BAND A SUBMISSION, EACH WAITED FOR BEFORE THE NEXT (Review Focus 5). The render engine
        // shares this queue and Metal runs it in order: with a tile's levels submitted at once an editing
        // frame queued behind all of them — 1,354 ms median against 9.4 idle; with one convolution a
        // submission, 51 ms (`editing_frames_while_a_tile_runs`). Bands of `SUBMISSION_MACS` bound it.
        let mut in_flight: Option<wgpu::SubmissionIndex> = None;
        for (bind, gx, gy, gz, pipeline) in sized.steps.iter().flat_map(|s| {
            s.bands
                .iter()
                .map(move |(b, gx)| (b, *gx, s.gy, s.gz, &s.pipeline))
        }) {
            let mut enc = ctx.device.create_command_encoder(&Default::default());
            {
                let mut pass = enc.begin_compute_pass(&Default::default());
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, bind, &[]);
                pass.dispatch_workgroups(gx, gy, gz);
            }
            // ⚠ Two in flight: the next band is queued before the last is waited for, so the GPU never
            // idles between them (one at a time cost +34 % a tile) and a frame waits behind two at most.
            let index = ctx.queue.submit(Some(enc.finish()));
            if let Some(before) = in_flight.replace(index) {
                let _ = ctx.device.poll(wgpu::PollType::Wait {
                    submission_index: Some(before),
                    timeout: None,
                });
            }
        }
        let out = conv::read(ctx, &sized.bufs[&Buf::Output], 3 * n, self.half)?;
        let mut hwc = vec![0f32; 3 * n];
        for i in 0..n {
            for c in 0..3 {
                hwc[i * 3 + c] = out[c * n + i];
            }
        }
        Some(hwc)
    }

    /// Every buffer, parameter block and binding of a `w × h` tile.
    fn build(&self, w: usize, h: usize) -> Sized {
        let ctx = &self.ctx;
        let channels = |b: Buf| match b {
            Buf::Input => 4,
            Buf::Output => 3,
            Buf::Skip(s) | Buf::Work(s, _) => self.arch.widths[s],
        };
        let mut bufs: HashMap<Buf, wgpu::Buffer> = HashMap::new();
        for op in &self.program {
            for b in [Some(op.src), Some(op.dst), op.add1, op.add2]
                .into_iter()
                .flatten()
            {
                bufs.entry(b).or_insert_with(|| {
                    let s = b.scale();
                    conv::zeros(
                        ctx,
                        "drunet-activation",
                        channels(b) * (w >> s) * (h >> s),
                        self.half,
                    )
                });
            }
        }
        let spare = conv::zeros(ctx, "drunet-spare", 1, self.half);
        let mut kernels: HashMap<Mode, (wgpu::BindGroupLayout, wgpu::ComputePipeline)> =
            HashMap::new();
        // Winograd's transformed planes: one pair, shared by every scale's 3 × 3 convolutions.
        let wino = self.winograd.then(|| {
            let n = planes(&self.arch, w, h);
            (
                conv::zeros(ctx, "drunet-wino-v", n, self.half),
                conv::zeros(ctx, "drunet-wino-m", n, self.half),
            )
        });
        let transforms = self.winograd.then(|| {
            ctx.cache.program(
                &ctx.device,
                "drunet-wino",
                &winograd::shader(self.half),
                &winograd_layout(),
                &["input", "output"],
            )
        });
        let mut steps: Vec<Step> = Vec::new();
        for (op, weights) in self.program.iter().zip(&self.weights) {
            let d = Dims {
                in_w: (w >> op.scale_in) as u32,
                in_h: (h >> op.scale_in) as u32,
                cin: op.cin as u32,
                cout: op.cout as u32,
            };
            let res = |r: Option<Buf>| r.map_or(&spare, |b| &bufs[&b]);
            if let (Mode::Conv3, Some((tl, tp)), Some((v, m))) = (op.mode, &transforms, &wino) {
                let transform = |c: u32, relu: bool, a1: Option<Buf>, a2: Option<Buf>| {
                    conv::uniform_buffer(
                        ctx,
                        &winograd::uniform(d.in_w, d.in_h, c, relu, a1.is_some(), a2.is_some()),
                    )
                };
                // The input's 4 × 4 patches transformed.
                let (gx, gy) = winograd::groups(d.in_w, d.in_h, d.cin);
                let params = transform(d.cin, false, None, None);
                let bind = conv::bind_n(ctx, tl, &[&bufs[&op.src], v, &spare, &spare, &params]);
                steps.push(Step {
                    pipeline: tp[0].clone(),
                    bands: vec![(bind, gx)],
                    gy,
                    gz: 1,
                });
                // The 16 products.
                let pd = Dims {
                    in_w: d.in_w.div_ceil(2) * d.in_h.div_ceil(2),
                    in_h: 1,
                    cin: d.cin,
                    cout: d.cout,
                };
                let (layout, pipeline) = kernels
                    .entry(Mode::Product)
                    .or_insert_with(|| conv::kernel(ctx, Mode::Product, self.half))
                    .clone();
                let bands = conv::bands(Mode::Product, &pd)
                    .into_iter()
                    .map(|(g0, count)| {
                        let params = conv::uniform_buffer(
                            ctx,
                            &conv::uniform(Mode::Product, &pd, false, false, false, g0),
                        );
                        let bind =
                            conv::bind(ctx, &layout, [v, weights, m, &spare, &spare], &params);
                        (bind, count)
                    })
                    .collect();
                steps.push(Step {
                    pipeline,
                    bands,
                    gy: conv::groups(Mode::Product, &pd).1,
                    gz: 16,
                });
                // The products back to pixels, with the fusions.
                let (gx, gy) = winograd::groups(d.in_w, d.in_h, d.cout);
                let params = transform(d.cout, op.relu, op.add1, op.add2);
                let bind = conv::bind_n(
                    ctx,
                    tl,
                    &[m, &bufs[&op.dst], res(op.add1), res(op.add2), &params],
                );
                steps.push(Step {
                    pipeline: tp[1].clone(),
                    bands: vec![(bind, gx)],
                    gy,
                    gz: 1,
                });
                continue;
            }
            let (layout, pipeline) = kernels
                .entry(op.mode)
                .or_insert_with(|| conv::kernel(ctx, op.mode, self.half))
                .clone();
            let bands = conv::bands(op.mode, &d)
                .into_iter()
                .map(|(g0, count)| {
                    let params = conv::uniform_buffer(
                        ctx,
                        &conv::uniform(
                            op.mode,
                            &d,
                            op.relu,
                            op.add1.is_some(),
                            op.add2.is_some(),
                            g0,
                        ),
                    );
                    let bind = conv::bind(
                        ctx,
                        &layout,
                        [
                            &bufs[&op.src],
                            weights,
                            &bufs[&op.dst],
                            res(op.add1),
                            res(op.add2),
                        ],
                        &params,
                    );
                    (bind, count)
                })
                .collect();
            steps.push(Step {
                pipeline,
                bands,
                gy: conv::groups(op.mode, &d).1,
                gz: 1,
            });
        }
        Sized { w, h, bufs, steps }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drunet::arch::{names, Arch};
    use crate::drunet::conv::tests::{ctx, rand};
    use crate::drunet::reference;

    /// Random weights for `a`, scaled by `1 / √fan-in` so activations stay bounded down the chain.
    fn random_weights(a: &Arch, seed: u32) -> Weights {
        names(a)
            .into_iter()
            .enumerate()
            .map(|(i, (n, shape))| {
                let len: usize = shape.iter().product();
                // ⚠ The transposed convolutions (`m_upL.0.weight`, `[in, out, 2, 2]`) read `in` values
                // an output; every other weight `in · kh · kw`.
                let transposed = n.starts_with("m_up") && n.split('.').count() == 3;
                let fan_in = if transposed {
                    shape[0]
                } else {
                    shape[1] * shape[2] * shape[3]
                };
                let scale = 2.0 / (fan_in as f32).sqrt();
                let data = rand(len, seed.wrapping_add(i as u32))
                    .into_iter()
                    .map(|v| v * scale)
                    .collect();
                (n, Tensor { shape, data })
            })
            .collect()
    }

    fn chw_with_sigma(rgb: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
        let n = w * h;
        let mut out = vec![sigma / 255.0; 4 * n];
        for i in 0..n {
            for c in 0..3 {
                out[c * n + i] = rgb[i * 3 + c];
            }
        }
        out
    }

    /// The worst difference relative to the reference's largest value.
    fn hwc_vs_chw(got: &[f32], want: &[f32], w: usize, h: usize) -> f32 {
        let n = w * h;
        // ⚠ NaN first — `f32::max` ignores it (see `conv::tests::close`).
        let bad = got.iter().chain(want).filter(|v| !v.is_finite()).count();
        assert_eq!(bad, 0, "{bad} non-finite values");
        let big = want.iter().fold(1.0f32, |m, v| m.max(v.abs()));
        let mut worst = 0f32;
        for i in 0..n {
            for c in 0..3 {
                worst = worst.max((got[i * 3 + c] - want[c * n + i]).abs());
            }
        }
        worst / big
    }

    /// A small DRUNet with random weights: the GPU program equals the reference forward pass written
    /// straight from the graph — every skip, residual, ReLU and resample where the file puts it.
    #[test]
    fn a_small_drunet_matches_the_reference() {
        let Some(ctx) = ctx() else { return };
        let arch = Arch {
            widths: [4, 8, 16, 32],
            blocks: 2,
        };
        let weights = random_weights(&arch, 11);
        let combos = crate::drunet::conv::tests::precisions(&ctx)
            .into_iter()
            .flat_map(|half| [(half, false), (half, true)]);
        for (half, wino) in combos {
            let mut net =
                GpuDrunet::with_options(ctx.clone(), &weights, half, wino).expect("built");
            // ⚠ 40 × 24 reaches 5 × 3 at the deepest scale: Winograd's blocks cropped at an odd edge.
            for (w, h) in [(16, 16), (40, 24)] {
                let rgb: Vec<f32> = rand(w * h * 3, (w * h) as u32)
                    .iter()
                    .map(|v| v + 0.5)
                    .collect();
                let got = net.run(&rgb, w, h, 25.0).expect("ran");
                let want =
                    reference::forward(&arch, &weights, &chw_with_sigma(&rgb, w, h, 25.0), w, h);
                assert!(
                    want.iter().any(|v| v.abs() > 0.05),
                    "the fixture is not all zero"
                );
                let worst = hwc_vs_chw(&got, &want, w, h);
                let tol = if half { 1e-2 } else { 1e-4 };
                assert!(worst < tol, "{w}×{h} half {half} winograd {wino}: {worst}");
            }
        }
    }

    /// A σ plane (M10.16b's strength spike): a constant one is exactly the scalar run, and a varying one
    /// changes the picture where it varies.
    #[test]
    fn a_noise_map_is_the_scalar_run_when_constant() {
        let Some(ctx) = ctx() else { return };
        let arch = Arch {
            widths: [4, 8, 16, 32],
            blocks: 1,
        };
        let mut net = GpuDrunet::new(ctx, &random_weights(&arch, 4)).unwrap();
        let (w, h) = (16, 16);
        let rgb: Vec<f32> = rand(w * h * 3, 9).iter().map(|v| v + 0.5).collect();
        let scalar = net.run(&rgb, w, h, 20.0).unwrap();
        let flat = net.run_map(&rgb, w, h, &vec![20.0; w * h]).unwrap();
        assert_eq!(scalar, flat);
        let mut map = vec![20.0; w * h];
        map[..w * h / 2].fill(60.0);
        let varied = net.run_map(&rgb, w, h, &map).unwrap();
        assert!(varied
            .iter()
            .zip(&scalar)
            .any(|(a, b)| (a - b).abs() > 1e-4));
        assert!(
            net.run_map(&rgb, w, h, &[20.0; 3]).is_err(),
            "a map of the wrong size"
        );
    }

    /// Review Focus 1: asked for f32, the kernel is b2's — within the f32 tolerance of the reference.
    #[test]
    fn the_f32_path_is_kept_without_half_precision() {
        let Some(ctx) = ctx() else { return };
        let arch = Arch {
            widths: [4, 8, 16, 32],
            blocks: 1,
        };
        let w = random_weights(&arch, 6);
        let rgb: Vec<f32> = rand(16 * 16 * 3, 3).iter().map(|v| v + 0.5).collect();
        let mut full = GpuDrunet::with_precision(ctx, &w, false).unwrap();
        assert!(!full.half);
        let want = reference::forward(&arch, &w, &chw_with_sigma(&rgb, 16, 16, 20.0), 16, 16);
        let got = full.run(&rgb, 16, 16, 20.0).unwrap();
        assert!(hwc_vs_chw(&got, &want, 16, 16) < 1e-4);
    }

    #[test]
    fn sides_must_be_multiples_of_eight() {
        let Some(ctx) = ctx() else { return };
        let arch = Arch {
            widths: [4, 8, 16, 32],
            blocks: 1,
        };
        let mut net = GpuDrunet::new(ctx, &random_weights(&arch, 1)).unwrap();
        assert!(net.run(&[0.5; 12 * 8 * 3], 12, 8, 10.0).is_err());
        assert!(net.run(&[0.5; 16 * 8 * 3], 16, 8, 10.0).is_ok());
    }

    /// Review Focus 3: a tile whose largest buffer passes the device's limit is refused, never allocated.
    #[test]
    fn a_tile_the_device_cannot_hold_is_refused() {
        let arch = Arch {
            widths: [64, 128, 256, 512],
            blocks: 4,
        };
        // 64 channels × 512² × 4 bytes = 64 MiB at scale 0, the largest activation.
        assert!(fits(&arch, 512, 512, 64 << 20, false));
        assert!(!fits(&arch, 512, 512, (64 << 20) - 1, false));
        // Winograd's: 16 positions × 64 channels × 256² blocks × 4 bytes = 256 MiB.
        assert!(fits(&arch, 512, 512, 256 << 20, true));
        assert!(!fits(&arch, 512, 512, (256 << 20) - 1, true));
    }

    /// Review Focus 4: the activations go with `release`, the weights stay, and a run after it works.
    #[test]
    fn release_frees_the_activations() {
        let Some(ctx) = ctx() else { return };
        let arch = Arch {
            widths: [4, 8, 16, 32],
            blocks: 1,
        };
        let mut net = GpuDrunet::new(ctx, &random_weights(&arch, 2)).unwrap();
        net.run(&[0.5; 16 * 16 * 3], 16, 16, 10.0).unwrap();
        assert!(net.sized.is_some());
        net.release();
        assert!(net.sized.is_none());
        assert!(net.run(&[0.5; 16 * 16 * 3], 16, 16, 10.0).is_ok());
    }

    /// What a tile holds beyond the direct kernel's activations is ONE pair of Winograd planes, at its
    /// largest scale's size — the 3 × 3 convolutions run one after another, so every scale shares it.
    /// ⚠ A pair per scale held 960 · N values for an N-pixel tile against this pair's 512 · N: 0.76 GB a
    /// 512 px tile in `f16` against b2's 0.51 (M10.16b5's review).
    #[test]
    fn winograd_holds_one_pair_of_planes() {
        let Some(ctx) = ctx() else { return };
        let arch = Arch {
            widths: [64, 128, 256, 512],
            blocks: 1,
        };
        let w = random_weights(&arch, 3);
        let held = |winograd: bool| {
            let net = GpuDrunet::with_options(ctx.clone(), &w, false, winograd).unwrap();
            conv::ZEROED.with(|z| z.set(0));
            let _sized = net.build(128, 128);
            conv::ZEROED.with(|z| z.get())
        };
        let (direct, wino) = (held(false), held(true));
        // Scale 0's blocks: 16 positions × 64 channels × 64 × 64 blocks, f32.
        let pair = 2 * 16 * 64 * 64 * 64 * 4;
        assert!(wino > direct, "Winograd allocated no planes: {wino} vs {direct}");
        assert!(
            wino - direct <= pair,
            "Winograd's planes {} bytes, one pair {pair}",
            wino - direct
        );
    }

    /// An `f16` overflow comes back as an error, never as a tile of NaN: the app's `Engine` then hands the
    /// job to `tract`, where a NaN would have been cached on disk as the photo (M10.16b5's review).
    #[test]
    fn a_non_finite_tile_is_an_error() {
        let Some(ctx) = ctx() else { return };
        if !ctx.f16() {
            return;
        }
        let arch = Arch {
            widths: [4, 8, 16, 32],
            blocks: 1,
        };
        // Weights ×1e4 a layer: past `f16`'s 65,504 within the first levels.
        let w: Weights = random_weights(&arch, 4)
            .into_iter()
            .map(|(n, t)| {
                let data = t.data.iter().map(|v| v * 1e4).collect();
                (n, Tensor { shape: t.shape, data })
            })
            .collect();
        let mut net = GpuDrunet::with_precision(ctx, &w, true).unwrap();
        let got = net.run(&[0.9; 16 * 16 * 3], 16, 16, 30.0);
        assert!(got.is_err(), "an overflowed tile came back as values");
    }

    /// Times a full-size DRUNet (random weights — the speed does not depend on them) on a 512 px tile:
    /// the kernel's own number, without the app or the model file (Task 6's tuning loop).
    #[test]
    #[ignore = "a timing, not a check — run with --release"]
    fn a_full_size_tile_time() {
        let Some(ctx) = ctx() else { return };
        let arch = Arch {
            widths: [64, 128, 256, 512],
            blocks: 4,
        };
        let w = random_weights(&arch, 5);
        let rgb = vec![0.5; 512 * 512 * 3];
        let combos = crate::drunet::conv::tests::precisions(&ctx)
            .into_iter()
            .flat_map(|half| [(half, false), (half, true)]);
        for (half, wino) in combos {
            let mut net = GpuDrunet::with_options(ctx.clone(), &w, half, wino).expect("built");
            let mut best = f64::MAX;
            for _ in 0..3 {
                let t = std::time::Instant::now();
                net.run(&rgb, 512, 512, 10.0).expect("ran");
                best = best.min(t.elapsed().as_secs_f64());
            }
            eprintln!(
                "a 512 px tile, half {half}, winograd {wino}: {:.3} s (best of 3)",
                best
            );
        }
    }

    /// Review Focus 5: an editing frame's GPU time — a 1600 px exposure chain, its upload, dispatch and
    /// download — idle and while a full-size tile runs on another thread (one submission a level).
    #[test]
    #[ignore = "a timing, not a check — run with --release"]
    fn editing_frames_while_a_tile_runs() {
        use pipeline::PipelineStage;
        let Some(ctx) = ctx() else { return };
        let frame = core_image::ImageBuf::from_vec(1600, 1067, vec![0.3; 1600 * 1067 * 3]);
        let median = |ctx: &Arc<GpuContext>| {
            let mut times: Vec<f64> = (0..15)
                .map(|_| {
                    let t = std::time::Instant::now();
                    let _ = crate::GpuExposureStage::new(0.3, ctx.clone()).apply(frame.clone());
                    t.elapsed().as_secs_f64() * 1000.0
                })
                .collect();
            times.sort_by(f64::total_cmp);
            (times[7], times[14])
        };
        let idle = median(&ctx);
        let arch = Arch {
            widths: [64, 128, 256, 512],
            blocks: 4,
        };
        let mut net = GpuDrunet::new(ctx.clone(), &random_weights(&arch, 5)).expect("built");
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let s = stop.clone();
        let job = std::thread::spawn(move || {
            let rgb = vec![0.5; 512 * 512 * 3];
            while !s.load(std::sync::atomic::Ordering::Relaxed) {
                net.run(&rgb, 512, 512, 10.0).expect("ran");
            }
        });
        std::thread::sleep(std::time::Duration::from_millis(500));
        let busy = median(&ctx);
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        job.join().expect("joined");
        eprintln!(
            "an editing frame: idle median {:.1} ms (worst {:.1}), during a tile median {:.1} ms (worst {:.1})",
            idle.0, idle.1, busy.0, busy.1
        );
    }

    /// A wrong file is refused at construction — the shape check, not a garbage run.
    #[test]
    fn weights_of_another_network_are_refused() {
        let Some(ctx) = ctx() else { return };
        let mut w = random_weights(
            &Arch {
                widths: [4, 8, 16, 32],
                blocks: 1,
            },
            3,
        );
        w.remove("m_head.weight");
        assert!(GpuDrunet::new(ctx, &w).is_err());
    }
}
