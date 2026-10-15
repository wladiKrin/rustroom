//! The CPU reference (tests only): the three convolutions done naively over CHW planes with `f64` sums,
//! and DRUNet's forward pass written straight from the graph — never from `program`, so a test against
//! it checks the program too.

use super::arch::{Mode, Tensor};
use super::conv::Dims;

fn at(src: &[f32], c: usize, y: isize, x: isize, w: usize, h: usize) -> f64 {
    if y < 0 || x < 0 || y >= h as isize || x >= w as isize {
        return 0.0;
    }
    src[c * w * h + y as usize * w + x as usize] as f64
}

/// 3 × 3, zero-padded by 1: `W[co][ci][ky][kx]`.
pub(crate) fn conv3(src: &[f32], d: &Dims, wt: &Tensor) -> Vec<f32> {
    let (w, h, cin, cout) = (
        d.in_w as usize,
        d.in_h as usize,
        d.cin as usize,
        d.cout as usize,
    );
    let mut out = vec![0f32; cout * w * h];
    for co in 0..cout {
        for y in 0..h {
            for x in 0..w {
                let mut s = 0f64;
                for ci in 0..cin {
                    for ky in 0..3 {
                        for kx in 0..3 {
                            let v = at(src, ci, y as isize + ky - 1, x as isize + kx - 1, w, h);
                            s += v * wt.data[((co * cin + ci) * 3 + ky as usize) * 3 + kx as usize]
                                as f64;
                        }
                    }
                }
                out[co * w * h + y * w + x] = s as f32;
            }
        }
    }
    out
}

/// 2 × 2, stride 2: `W[co][ci][ky][kx]`.
pub(crate) fn down2(src: &[f32], d: &Dims, wt: &Tensor) -> Vec<f32> {
    let (w, h, cin, cout) = (
        d.in_w as usize,
        d.in_h as usize,
        d.cin as usize,
        d.cout as usize,
    );
    let (ow, oh) = (w / 2, h / 2);
    let mut out = vec![0f32; cout * ow * oh];
    for co in 0..cout {
        for y in 0..oh {
            for x in 0..ow {
                let mut s = 0f64;
                for ci in 0..cin {
                    for ky in 0..2 {
                        for kx in 0..2 {
                            let v = src[ci * w * h + (2 * y + ky) * w + 2 * x + kx] as f64;
                            s += v * wt.data[((co * cin + ci) * 2 + ky) * 2 + kx] as f64;
                        }
                    }
                }
                out[co * ow * oh + y * ow + x] = s as f32;
            }
        }
    }
    out
}

/// 2 × 2 transposed, stride 2: `W[ci][co][ky][kx]` — each input pixel spreads into a 2 × 2 block.
pub(crate) fn up2(src: &[f32], d: &Dims, wt: &Tensor) -> Vec<f32> {
    let (w, h, cin, cout) = (
        d.in_w as usize,
        d.in_h as usize,
        d.cin as usize,
        d.cout as usize,
    );
    let (ow, oh) = (2 * w, 2 * h);
    let mut out = vec![0f32; cout * ow * oh];
    for co in 0..cout {
        for y in 0..h {
            for x in 0..w {
                for ky in 0..2 {
                    for kx in 0..2 {
                        let mut s = 0f64;
                        for ci in 0..cin {
                            s += src[ci * w * h + y * w + x] as f64
                                * wt.data[((ci * cout + co) * 2 + ky) * 2 + kx] as f64;
                        }
                        out[co * ow * oh + (2 * y + ky) * ow + 2 * x + kx] = s as f32;
                    }
                }
            }
        }
    }
    out
}

pub(crate) fn run(mode: Mode, src: &[f32], d: &Dims, wt: &Tensor) -> Vec<f32> {
    match mode {
        Mode::Conv3 => conv3(src, d, wt),
        Mode::Down2 => down2(src, d, wt),
        Mode::Up2 => up2(src, d, wt),
    }
}
