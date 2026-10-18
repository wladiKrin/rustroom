//! FFDNet colour (M10.16b6) — AI denoise's default network: twelve 3 × 3 convolutions at half resolution on
//! the tile pixel-unshuffled 2 × 2 beside a plane of its noise level. The packing lives here, shared by the
//! CPU forward pass below and the GPU engine (`gpu_pipeline::GpuFfdnet`), which only runs the convolutions.

use crate::ai_denoise::TileNet;
use ai_mask::denoise::Weight;

/// The colour network's channels: 12 unshuffled colour planes and the σ plane in, 96 between, 12 out.
const IN: usize = 13;
const WIDTH: usize = 96;
const OUT: usize = 12;
/// Its convolutions — `model.{0, 2, …, 22}` in the file, ReLU after every one but the last.
const LAYERS: usize = 12;

/// One 3 × 3 convolution with its bias.
struct Layer {
    w: Vec<f32>,
    b: Vec<f32>,
    cin: usize,
    cout: usize,
}

/// FFDNet on the CPU — the fallback behind the GPU, and the GPU's reference.
pub struct Ffdnet {
    layers: Vec<Layer>,
}

/// The network's input for an interleaved `rgb` tile (`w × h`, encoded 0…1, sides even) at a σ per pixel
/// (0…255): 13 planes of `w/2 × h/2`, CHW — colour `c`'s sub-pixel `(dx, dy)` at plane `c · 4 + dy · 2 + dx`
/// (KAIR's unshuffle), then each 2 × 2 block's mean σ / 255.
pub fn pack(rgb: &[f32], sigma: &[f32], w: usize, h: usize) -> Vec<f32> {
    let (w2, h2) = (w / 2, h / 2);
    let mut x = vec![0f32; IN * w2 * h2];
    for y in 0..h2 {
        for xx in 0..w2 {
            let mut s = 0.0;
            for dy in 0..2 {
                for dx in 0..2 {
                    let p = (2 * y + dy) * w + 2 * xx + dx;
                    for c in 0..3 {
                        x[((c * 4 + dy * 2 + dx) * h2 + y) * w2 + xx] = rgb[p * 3 + c];
                    }
                    s += sigma[p];
                }
            }
            x[(OUT * h2 + y) * w2 + xx] = s / 4.0 / 255.0;
        }
    }
    x
}

/// The network's 12 output planes (`w/2 × h/2`, CHW) back to an interleaved `w × h` image — [`pack`]'s
/// order undone.
pub fn unpack(out: &[f32], w: usize, h: usize) -> Vec<f32> {
    let (w2, h2) = (w / 2, h / 2);
    let mut rgb = vec![0f32; w * h * 3];
    for y in 0..h2 {
        for xx in 0..w2 {
            for dy in 0..2 {
                for dx in 0..2 {
                    let p = (2 * y + dy) * w + 2 * xx + dx;
                    for c in 0..3 {
                        rgb[p * 3 + c] = out[((c * 4 + dy * 2 + dx) * h2 + y) * w2 + xx];
                    }
                }
            }
        }
    }
    rgb
}

/// A 3 × 3 convolution, zero-padded, CHW `w × h` in and out, its bias added and — `relu` — clipped below 0.
fn conv3(input: &[f32], w: usize, h: usize, l: &Layer, relu: bool) -> Vec<f32> {
    use rayon::prelude::*;
    let mut out = vec![0f32; l.cout * w * h];
    out.par_chunks_mut(w * h)
        .enumerate()
        .for_each(|(co, plane)| {
            plane.fill(l.b[co]);
            for ci in 0..l.cin {
                let src = &input[ci * w * h..(ci + 1) * w * h];
                for ky in 0..3 {
                    for kx in 0..3 {
                        let wv = l.w[((co * l.cin + ci) * 3 + ky) * 3 + kx];
                        let dx = kx as isize - 1;
                        let (x0, x1) = ((-dx).max(0) as usize, (w as isize - dx.max(0)) as usize);
                        for y in 0..h {
                            let sy = y as isize + ky as isize - 1;
                            if sy < 0 || sy >= h as isize {
                                continue;
                            }
                            let srow = &src[sy as usize * w..(sy as usize + 1) * w];
                            let drow = &mut plane[y * w..(y + 1) * w];
                            for x in x0..x1 {
                                drow[x] += wv * srow[(x as isize + dx) as usize];
                            }
                        }
                    }
                }
            }
            if relu {
                plane.iter_mut().for_each(|v| *v = v.max(0.0));
            }
        });
    out
}

/// The weights as the GPU engine takes them.
pub fn gpu_weights(w: &[Weight]) -> gpu_pipeline::Weights {
    w.iter()
        .map(|(name, shape, data)| {
            (
                name.clone(),
                gpu_pipeline::Tensor {
                    shape: shape.clone(),
                    data: data.clone(),
                },
            )
        })
        .collect()
}

impl Ffdnet {
    /// The network from the file's weights — every name and shape checked, the first misfit named.
    pub fn from_weights(w: Vec<Weight>) -> Result<Self, String> {
        let mut take = |name: String, shape: Vec<usize>| -> Result<Vec<f32>, String> {
            let (_, got, data) = w
                .iter()
                .find(|x| x.0 == name)
                .ok_or_else(|| format!("FFDNet's {name} is missing"))?;
            if *got != shape || data.len() != shape.iter().product::<usize>() {
                return Err(format!("FFDNet's {name} is {got:?}, not {shape:?}"));
            }
            Ok(data.clone())
        };
        let layers = (0..LAYERS)
            .map(|i| {
                let cin = if i == 0 { IN } else { WIDTH };
                let cout = if i + 1 == LAYERS { OUT } else { WIDTH };
                Ok(Layer {
                    w: take(format!("model.{}.weight", 2 * i), vec![cout, cin, 3, 3])?,
                    b: take(format!("model.{}.bias", 2 * i), vec![cout])?,
                    cin,
                    cout,
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        Ok(Self { layers })
    }

    /// The 12 output planes for [`pack`]'s 13 (`w2 × h2`).
    pub fn forward(&self, input: Vec<f32>, w2: usize, h2: usize) -> Vec<f32> {
        let last = self.layers.len() - 1;
        self.layers
            .iter()
            .enumerate()
            .fold(input, |x, (i, l)| conv3(&x, w2, h2, l, i < last))
    }
}

impl TileNet for Ffdnet {
    fn run(&mut self, rgb: &[f32], w: usize, h: usize, sigma: &[f32]) -> Result<Vec<f32>, String> {
        check(rgb, w, h, sigma)?;
        Ok(unpack(
            &self.forward(pack(rgb, sigma, w, h), w / 2, h / 2),
            w,
            h,
        ))
    }
}

/// A tile FFDNet can take: even, non-empty sides, and the values and noise levels to match.
pub fn check(rgb: &[f32], w: usize, h: usize, sigma: &[f32]) -> Result<(), String> {
    if w == 0 || h == 0 || w % 2 != 0 || h % 2 != 0 {
        return Err(format!("a {w} × {h} tile: FFDNet needs even sides"));
    }
    if rgb.len() != w * h * 3 || sigma.len() != w * h {
        return Err(format!(
            "a {w} × {h} tile with {} values, {} noise levels",
            rgb.len(),
            sigma.len()
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A seeded uniform −0.5…0.5.
    fn rand(n: usize, seed: u64) -> Vec<f32> {
        let mut s = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
        (0..n)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                (s >> 40) as f32 / (1u64 << 24) as f32 - 0.5
            })
            .collect()
    }

    /// The file's 24 weights with random values, scaled by 1 / √fan-in.
    fn fake() -> Vec<Weight> {
        (0..LAYERS)
            .flat_map(|i| {
                let cin = if i == 0 { IN } else { WIDTH };
                let cout = if i + 1 == LAYERS { OUT } else { WIDTH };
                let scale = 2.0 / ((cin * 9) as f32).sqrt();
                let w = rand(cout * cin * 9, i as u64)
                    .into_iter()
                    .map(|v| v * scale)
                    .collect();
                [
                    (format!("model.{}.weight", 2 * i), vec![cout, cin, 3, 3], w),
                    (
                        format!("model.{}.bias", 2 * i),
                        vec![cout],
                        rand(cout, 100 + i as u64),
                    ),
                ]
            })
            .collect()
    }

    #[test]
    fn unpack_inverts_packs_colour_planes() {
        let (w, h) = (6, 4);
        let (w2, h2) = (3, 2);
        let rgb: Vec<f32> = (0..w * h * 3).map(|i| i as f32 / 100.0).collect();
        let sigma: Vec<f32> = (0..w * h).map(|i| i as f32).collect();
        let p = pack(&rgb, &sigma, w, h);
        assert_eq!(p.len(), IN * w2 * h2);
        assert_eq!(unpack(&p[..OUT * w2 * h2], w, h), rgb);
        for y in 0..h2 {
            for x in 0..w2 {
                let at = |dx: usize, dy: usize| sigma[(2 * y + dy) * w + 2 * x + dx];
                let mean = (at(0, 0) + at(1, 0) + at(0, 1) + at(1, 1)) / 4.0 / 255.0;
                assert!((p[(12 * h2 + y) * w2 + x] - mean).abs() < 1e-6, "{x},{y}");
            }
        }
    }

    #[test]
    fn a_misshapen_weight_is_named() {
        let mut w = fake();
        let bad = w.iter_mut().find(|x| x.0 == "model.4.weight").unwrap();
        bad.1 = vec![96, 95, 3, 3];
        bad.2.truncate(96 * 95 * 9);
        let e = Ffdnet::from_weights(w).err().expect("refused");
        assert!(e.contains("model.4.weight"), "{e}");
        assert!(Ffdnet::from_weights(fake()).is_ok(), "positive control");
    }

    /// A smooth 256² picture plus seeded Gaussian noise of σ 25, its PSNR before and after.
    fn known_noise(net: &mut Ffdnet) -> (f64, f64) {
        let side = 256;
        let clean: Vec<f32> = (0..side * side)
            .flat_map(|i| {
                let (x, y) = (
                    (i % side) as f32 / side as f32,
                    (i / side) as f32 / side as f32,
                );
                [
                    0.2 + 0.6 * x,
                    0.5 + 0.3 * (6.0 * y).sin(),
                    0.3 + 0.4 * x * y,
                ]
            })
            .collect();
        let u = rand(clean.len() * 2, 7);
        let noisy: Vec<f32> = clean
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let (a, b) = (
                    (u[2 * i] + 0.5).max(1e-6) as f64,
                    (u[2 * i + 1] + 0.5) as f64,
                );
                let g = (-2.0 * a.ln()).sqrt() * (2.0 * std::f64::consts::PI * b).cos();
                c + (g * 25.0 / 255.0) as f32
            })
            .collect();
        let out = net
            .run(&noisy, side, side, &vec![25.0; side * side])
            .unwrap();
        let psnr = |a: &[f32]| {
            let mse = a
                .iter()
                .zip(&clean)
                .map(|(x, c)| ((x - c) as f64).powi(2))
                .sum::<f64>()
                / a.len() as f64;
            10.0 * (1.0 / mse).log10()
        };
        (psnr(&noisy), psnr(&out))
    }

    fn real() -> Ffdnet {
        let path = std::env::var("RUSTROOM_FFDNET").expect("RUSTROOM_FFDNET");
        Ffdnet::from_weights(ai_mask::pth::read(std::path::Path::new(&path)).expect("read"))
            .expect("weights")
    }

    /// The port against known answers (M10.16b6's spike measured 20.17 → 43.42 dB): a wrong channel order
    /// or a missing bias cannot reach 40.
    #[test]
    #[ignore = "needs the downloaded model; set RUSTROOM_FFDNET"]
    fn the_real_ffdnet_cleans_known_noise() {
        let (before, after) = known_noise(&mut real());
        eprintln!("σ 25 on a smooth picture: {before:.2} dB → {after:.2} dB");
        assert!(after > 40.0, "{after:.2} dB");
    }

    /// Review Focus 4: the tiler's overlap (32) covers FFDNet's ±25 px — the tiled result inside the photo
    /// is the one-pass result within one 8-bit level.
    #[test]
    #[ignore = "needs the downloaded model; set RUSTROOM_FFDNET"]
    fn ffdnet_tiles_match_one_pass_inside() {
        use crate::ai_denoise::{denoise_in, Encoding};
        let (w, h) = (1100, 700);
        let noise = rand(w * h * 3, 3);
        let img = core_image::ImageBuf::from_vec(
            w,
            h,
            (0..w * h * 3)
                .map(|i| {
                    let x = (i / 3 % w) as f32 / w as f32;
                    (0.05 + 0.4 * x + 0.04 * noise[i]).max(0.0)
                })
                .collect(),
        );
        let enc = Encoding::of(&img);
        let mut net = real();
        let sigma = |_: &[f32], w: usize, h: usize| vec![15.0; w * h];
        let tiled = denoise_in(&img, &enc, &mut net, &sigma, &|_, _| true)
            .unwrap()
            .unwrap();
        let encoded: Vec<f32> = (0..w * h)
            .flat_map(|i| enc.encode(img.pixel(i % w, i / w)))
            .collect();
        let once = net.run(&encoded, w, h, &vec![15.0; w * h]).unwrap();
        let (mut worst, mut moved) = (0f32, 0f32);
        for y in 32..h - 32 {
            for x in 32..w - 32 {
                let t = enc.encode(tiled.pixel(x, y));
                for c in 0..3 {
                    let o = once[(y * w + x) * 3 + c].clamp(0.0, 1.0);
                    worst = worst.max((t[c] - o).abs());
                    moved = moved.max((o - encoded[(y * w + x) * 3 + c]).abs());
                }
            }
        }
        eprintln!("inside: tiled vs one pass {worst:e}, the network moved values by {moved:.3}");
        assert!(moved > 0.01, "the network changed nothing");
        assert!(worst < 1.0 / 255.0, "{worst:e}");
    }
}
