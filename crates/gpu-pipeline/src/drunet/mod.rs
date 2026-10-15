//! DRUNet on the GPU (M10.16b2): the denoise network's 64 convolutions as one tiled product in three
//! modes, run tile by tile for the app's denoise job. The weights come in as plain tensors — this crate
//! never parses ONNX.

mod arch;
mod conv;
mod pack;
#[cfg(test)]
mod reference;
mod wgsl;

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
}

/// Whether a `w × h` tile's largest buffer fits the device's limit — scale 0's widest, since each
/// scale doubles the channels and quarters the pixels.
pub(crate) fn fits(arch: &Arch, w: usize, h: usize, max_bytes: u64) -> bool {
    (0..4).all(|s| (arch.widths[s] * (w >> s) * (h >> s) * 4) as u64 <= max_bytes)
        && (4 * w * h * 4) as u64 <= max_bytes
}

impl GpuDrunet {
    /// `weights` checked against DRUNet's shape (the first missing or misshapen one named), packed and
    /// uploaded.
    pub fn new(ctx: Arc<GpuContext>, weights: &Weights) -> Result<Self, String> {
        let arch = Arch::of(weights)?;
        let program = arch::program(&arch);
        let packed: Vec<Vec<f32>> = program
            .iter()
            .map(|op| pack::pack(op.mode, &weights[&op.weight]))
            .collect();
        let uploaded = crate::guarded(|| {
            Some(
                packed
                    .iter()
                    .map(|p| conv::upload(&ctx, "drunet-weights", p))
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
        })
    }

    /// `rgb` (`w × h`, interleaved, 0…1) denoised at noise level `sigma` (0…255 units) — the same layout
    /// back; `tract`'s `Drunet::run` contract. Sides must be multiples of 8.
    pub fn run(&mut self, rgb: &[f32], w: usize, h: usize, sigma: f32) -> Result<Vec<f32>, String> {
        if w == 0 || h == 0 || !w.is_multiple_of(8) || !h.is_multiple_of(8) {
            return Err(format!("a {w} × {h} tile: sides must be multiples of 8"));
        }
        if rgb.len() != w * h * 3 {
            return Err(format!("{} values for a {w} × {h} tile", rgb.len()));
        }
        if !fits(&self.arch, w, h, self.ctx.max_image_bytes()) {
            return Err(format!("a {w} × {h} tile is too large for this GPU"));
        }
        crate::guarded(|| self.run_inner(rgb, w, h, sigma))
            .ok_or_else(|| "the GPU failed".to_string())
    }

    /// Drop the activations — ~0.5 GB for a 512 px tile — and keep the weights.
    pub fn release(&mut self) {
        self.sized = None;
    }

    fn run_inner(&mut self, rgb: &[f32], w: usize, h: usize, sigma: f32) -> Option<Vec<f32>> {
        if self.sized.as_ref().map(|s| (s.w, s.h)) != Some((w, h)) {
            self.sized = None;
            self.sized = Some(self.build(w, h));
        }
        let sized = self.sized.as_ref()?;
        let n = w * h;
        let mut input = vec![sigma / 255.0; 4 * n];
        for i in 0..n {
            for c in 0..3 {
                input[c * n + i] = rgb[i * 3 + c];
            }
        }
        let ctx = &self.ctx;
        ctx.queue
            .write_buffer(&sized.bufs[&Buf::Input], 0, bytemuck::cast_slice(&input));
        // ⚠⚠ ONE BAND A SUBMISSION, EACH WAITED FOR BEFORE THE NEXT (Review Focus 5). The render engine
        // shares this queue and Metal runs it in order: with a tile's levels submitted at once an editing
        // frame queued behind all of them — 1,354 ms median against 9.4 idle; with one convolution a
        // submission, 51 ms (`editing_frames_while_a_tile_runs`). Bands of `SUBMISSION_MACS` bound it.
        let mut in_flight: Option<wgpu::SubmissionIndex> = None;
        for (bind, gx, gy, pipeline) in sized.steps.iter().flat_map(|s| {
            s.bands
                .iter()
                .map(move |(b, gx)| (b, *gx, s.gy, &s.pipeline))
        }) {
            let mut enc = ctx.device.create_command_encoder(&Default::default());
            {
                let mut pass = enc.begin_compute_pass(&Default::default());
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, bind, &[]);
                pass.dispatch_workgroups(gx, gy, 1);
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
        let out = conv::read(ctx, &sized.bufs[&Buf::Output], 3 * n)?;
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
                    conv::zeros(ctx, "drunet-activation", channels(b) * (w >> s) * (h >> s))
                });
            }
        }
        let spare = conv::zeros(ctx, "drunet-spare", 1);
        let mut kernels: HashMap<Mode, (wgpu::BindGroupLayout, wgpu::ComputePipeline)> =
            HashMap::new();
        let steps = self
            .program
            .iter()
            .zip(&self.weights)
            .map(|(op, weights)| {
                let d = Dims {
                    in_w: (w >> op.scale_in) as u32,
                    in_h: (h >> op.scale_in) as u32,
                    cin: op.cin as u32,
                    cout: op.cout as u32,
                };
                let (layout, pipeline) = kernels
                    .entry(op.mode)
                    .or_insert_with(|| conv::kernel(ctx, op.mode))
                    .clone();
                let res = |r: Option<Buf>| r.map_or(&spare, |b| &bufs[&b]);
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
                Step {
                    pipeline,
                    bands,
                    gy: conv::groups(op.mode, &d).1,
                }
            })
            .collect();
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
        let mut net = GpuDrunet::new(ctx, &weights).expect("built");
        for (w, h) in [(16, 16), (40, 24)] {
            let rgb: Vec<f32> = rand(w * h * 3, (w * h) as u32)
                .iter()
                .map(|v| v + 0.5)
                .collect();
            let got = net.run(&rgb, w, h, 25.0).expect("ran");
            let want = reference::forward(&arch, &weights, &chw_with_sigma(&rgb, w, h, 25.0), w, h);
            assert!(
                want.iter().any(|v| v.abs() > 0.05),
                "the fixture is not all zero"
            );
            let worst = hwc_vs_chw(&got, &want, w, h);
            assert!(worst < 1e-4, "{w}×{h}: {worst}");
        }
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
        // 64 channels × 512² × 4 bytes = 64 MiB at scale 0, the largest buffer.
        assert!(fits(&arch, 512, 512, 64 << 20));
        assert!(!fits(&arch, 512, 512, (64 << 20) - 1));
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
        let mut net = GpuDrunet::new(ctx, &random_weights(&arch, 5)).expect("built");
        let rgb = vec![0.5; 512 * 512 * 3];
        let mut best = f64::MAX;
        for _ in 0..3 {
            let t = std::time::Instant::now();
            net.run(&rgb, 512, 512, 10.0).expect("ran");
            best = best.min(t.elapsed().as_secs_f64());
        }
        eprintln!("a 512 px tile: {:.3} s (best of 3)", best);
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
