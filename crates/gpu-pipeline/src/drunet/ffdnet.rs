//! FFDNet on the GPU (M10.16b6): its chain of 3 × 3 convolutions with biases, on the DRUNet engine's kernel —
//! Winograd, half precision and the bands that keep editing frames smooth. The caller packs the tile (the
//! 2 × 2 unshuffle beside its σ plane, `rustroom_app::ffdnet::pack`) and unpacks the result, so this runs
//! the chain on planes at half resolution.

use super::arch::{Buf, Mode, Op, Weights};
use super::{Graph, Net};
use crate::GpuContext;
use std::sync::Arc;

/// FFDNet on the GPU: its weights uploaded once, its buffers built once per tile size.
pub struct GpuFfdnet {
    net: Net,
}

impl GpuFfdnet {
    /// The chain in `weights` (`model.{0, 2, …}.weight` and `.bias`) checked and uploaded.
    pub fn new(ctx: Arc<GpuContext>, weights: &Weights) -> Result<Self, String> {
        let half = ctx.f16();
        Self::with_precision(ctx, weights, half)
    }

    /// [`GpuFfdnet::new`] storing in `f16` when `half`.
    pub fn with_precision(
        ctx: Arc<GpuContext>,
        weights: &Weights,
        half: bool,
    ) -> Result<Self, String> {
        Self::with_options(ctx, weights, half, super::WINOGRAD)
    }

    pub(crate) fn with_options(
        ctx: Arc<GpuContext>,
        weights: &Weights,
        half: bool,
        winograd: bool,
    ) -> Result<Self, String> {
        let graph = chain(weights)?;
        Ok(Self {
            net: Net::new(ctx, graph, weights, half, winograd)?,
        })
    }

    /// The chain over `input` — its first layer's channels of `w2 × h2`, CHW — to its last layer's, CHW.
    pub fn run_packed(&mut self, input: &[f32], w2: usize, h2: usize) -> Result<Vec<f32>, String> {
        self.net.run_planes(input, w2, h2)
    }

    /// Drop the activations and keep the weights.
    pub fn release(&mut self) {
        self.net.release();
    }
}

/// The chain the weights describe: `model.{2i}.weight` `[cout, cin, 3, 3]` with `model.{2i}.bias` `[cout]`,
/// each layer reading the last one's output, ReLU after every one but the last — at one scale, ping-ponging
/// between two work buffers.
pub(crate) fn chain(w: &Weights) -> Result<Graph, String> {
    let mut ops: Vec<Op> = Vec::new();
    let n = (0..)
        .take_while(|i| w.contains_key(&format!("model.{}.weight", 2 * i)))
        .count();
    if n == 0 {
        return Err("no model.0.weight — not an FFDNet file".to_string());
    }
    let mut width = 0;
    for i in 0..n {
        let (name, bias) = (
            format!("model.{}.weight", 2 * i),
            format!("model.{}.bias", 2 * i),
        );
        let s = &w[&name].shape;
        if s.len() != 4 || s[2] != 3 || s[3] != 3 {
            return Err(format!("{name} is {s:?}, not a 3 × 3 convolution"));
        }
        let (cout, cin) = (s[0], s[1]);
        if i > 0 && cin != ops[i - 1].cout {
            return Err(format!(
                "{name} reads {cin} channels after {}",
                ops[i - 1].cout
            ));
        }
        match w.get(&bias) {
            Some(b) if b.shape == [cout] => {}
            _ => return Err(format!("{bias} missing or not [{cout}]")),
        }
        if i + 1 < n {
            width = width.max(cout);
        }
        let src = if i == 0 {
            Buf::Input
        } else {
            Buf::Work(0, 1 + (i + 1) % 2)
        };
        let dst = if i + 1 == n {
            Buf::Output
        } else {
            Buf::Work(0, 1 + i % 2)
        };
        ops.push(Op {
            mode: Mode::Conv3,
            weight: name,
            bias: Some(bias),
            src,
            dst,
            relu: i + 1 < n,
            add1: None,
            add2: None,
            scale_in: 0,
            cin,
            cout,
        });
    }
    Ok(Graph {
        input: ops[0].cin,
        output: ops[n - 1].cout,
        widths: [width, 0, 0, 0],
        ops,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drunet::arch::Tensor;
    use crate::drunet::conv::tests::{ctx, precisions, rand};
    use crate::drunet::conv::Dims;
    use crate::drunet::reference;

    /// A chain `channels[0] → … → channels[last]` with random weights (scaled 1 / √fan-in) and biases.
    fn weights(channels: &[usize], seed: u32) -> Weights {
        let mut w = Weights::new();
        for (i, pair) in channels.windows(2).enumerate() {
            let (cin, cout) = (pair[0], pair[1]);
            let scale = 2.0 / ((cin * 9) as f32).sqrt();
            let data = rand(cout * cin * 9, seed + i as u32)
                .into_iter()
                .map(|v| v * scale)
                .collect();
            w.insert(
                format!("model.{}.weight", 2 * i),
                Tensor {
                    shape: vec![cout, cin, 3, 3],
                    data,
                },
            );
            w.insert(
                format!("model.{}.bias", 2 * i),
                Tensor {
                    shape: vec![cout],
                    data: rand(cout, seed + 50 + i as u32),
                },
            );
        }
        w
    }

    /// The chain on the CPU, straight from the reference convolution: bias, then ReLU but on the last.
    fn reference_chain(w: &Weights, input: &[f32], width: usize, height: usize) -> Vec<f32> {
        let g = chain(w).unwrap();
        let last = g.ops.len() - 1;
        g.ops.iter().enumerate().fold(input.to_vec(), |x, (i, op)| {
            let d = Dims {
                in_w: width as u32,
                in_h: height as u32,
                cin: op.cin as u32,
                cout: op.cout as u32,
            };
            let mut y = reference::conv3(&x, &d, &w[&op.weight]);
            let b = &w[op.bias.as_ref().unwrap()].data;
            for (j, v) in y.iter_mut().enumerate() {
                *v += b[j / (width * height)];
                if i < last {
                    *v = v.max(0.0);
                }
            }
            y
        })
    }

    fn relative(got: &[f32], want: &[f32]) -> f32 {
        let bad = got.iter().filter(|v| !v.is_finite()).count();
        assert_eq!(bad, 0, "{bad} non-finite values");
        let big = want.iter().fold(1.0f32, |m, v| m.max(v.abs()));
        got.iter()
            .zip(want)
            .fold(0f32, |m, (g, w)| m.max((g - w).abs()))
            / big
    }

    /// A small FFDNet-shaped chain (13 → 8 → 8 → 12) on a 20 × 12 plane equals the reference — both
    /// precisions, Winograd on and off.
    #[test]
    fn a_small_ffdnet_matches_the_reference() {
        let Some(ctx) = ctx() else { return };
        let w = weights(&[13, 8, 8, 12], 1);
        let (pw, ph) = (20, 12);
        let input: Vec<f32> = rand(13 * pw * ph, 9).iter().map(|v| v + 0.5).collect();
        let want = reference_chain(&w, &input, pw, ph);
        for half in precisions(&ctx) {
            for winograd in [false, true] {
                let mut net = GpuFfdnet::with_options(ctx.clone(), &w, half, winograd).unwrap();
                let got = net.run_packed(&input, pw, ph).unwrap();
                let err = relative(&got, &want);
                let tol = if half { 1e-2 } else { 1e-4 };
                assert!(err < tol, "half {half} winograd {winograd}: {err}");
            }
        }
    }

    /// The bias joins the sum BEFORE the ReLU: a first layer whose bias drives every sum below zero leaves
    /// zeros, so the second layer returns its own bias everywhere — on both kernel paths.
    #[test]
    fn the_store_adds_the_bias_before_relu() {
        let Some(ctx) = ctx() else { return };
        let mut w = weights(&[4, 8, 6], 2);
        w.get_mut("model.0.bias").unwrap().data = vec![-50.0; 8];
        let (pw, ph) = (10, 6);
        let input: Vec<f32> = rand(4 * pw * ph, 3).iter().map(|v| v + 0.5).collect();
        let b1 = w["model.2.bias"].data.clone();
        for winograd in [false, true] {
            let mut net = GpuFfdnet::with_options(ctx.clone(), &w, false, winograd).unwrap();
            let got = net.run_packed(&input, pw, ph).unwrap();
            for (i, v) in got.iter().enumerate() {
                let want = b1[i / (pw * ph)];
                assert!(
                    (v - want).abs() < 1e-5,
                    "winograd {winograd}: {i} is {v}, not {want}"
                );
            }
        }
    }

    /// An `f16` overflow is an error, never a tile of NaN.
    #[test]
    fn a_non_finite_ffdnet_tile_is_an_error() {
        let Some(ctx) = ctx() else { return };
        if !ctx.f16() {
            return;
        }
        let mut w = weights(&[13, 8, 8, 12], 4);
        for t in w.values_mut() {
            t.data.iter_mut().for_each(|v| *v *= 1e4);
        }
        let mut net = GpuFfdnet::with_precision(ctx, &w, true).unwrap();
        assert!(net.run_packed(&vec![0.9; 13 * 16 * 16], 16, 16).is_err());
    }

    /// Times FFDNet's real shape (13 → 96 × 11 → 12, random weights — the speed does not depend on them) on
    /// a 512 px tile's packed planes (256²): both precisions, Winograd on and off, best of 3.
    #[test]
    #[ignore = "a timing, not a check — run with --release"]
    fn ffdnet_full_size_tile_time() {
        let Some(ctx) = ctx() else { return };
        let mut channels = vec![13];
        channels.extend([96; 11]);
        channels.push(12);
        let w = weights(&channels, 6);
        let input = vec![0.3; 13 * 256 * 256];
        let combos = precisions(&ctx)
            .into_iter()
            .flat_map(|half| [(half, false), (half, true)]);
        for (half, wino) in combos {
            let mut net = GpuFfdnet::with_options(ctx.clone(), &w, half, wino).expect("built");
            let mut best = f64::MAX;
            for _ in 0..3 {
                let t = std::time::Instant::now();
                net.run_packed(&input, 256, 256).expect("ran");
                best = best.min(t.elapsed().as_secs_f64());
            }
            eprintln!(
                "FFDNet, a 512 px tile, half {half}, winograd {wino}: {best:.3} s (best of 3)"
            );
        }
    }

    /// A chain whose layers do not meet is refused with the layer named.
    #[test]
    fn a_broken_chain_is_named() {
        let mut w = weights(&[13, 8, 8, 12], 5);
        w.insert(
            "model.2.weight".to_string(),
            Tensor {
                shape: vec![8, 7, 3, 3],
                data: vec![0.0; 8 * 7 * 9],
            },
        );
        let e = chain(&w).err().expect("refused");
        assert!(e.contains("model.2.weight"), "{e}");
    }
}
