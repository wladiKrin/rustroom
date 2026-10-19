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
        Mode::Product => unreachable!("the reference convolves directly"),
    }
}

fn dims(w: usize, h: usize, cin: usize, cout: usize) -> Dims {
    Dims {
        in_w: w as u32,
        in_h: h as u32,
        cin: cin as u32,
        cout: cout as u32,
    }
}

fn add(a: &[f32], b: &[f32]) -> Vec<f32> {
    a.iter().zip(b).map(|(x, y)| x + y).collect()
}

/// `x + conv(relu(conv(x)))`.
fn block(x: &[f32], wt: &super::Weights, prefix: &str, ch: usize, w: usize, h: usize) -> Vec<f32> {
    let d = dims(w, h, ch, ch);
    let t: Vec<f32> = conv3(x, &d, &wt[&format!("{prefix}.res.0.weight")])
        .into_iter()
        .map(|v| v.max(0.0))
        .collect();
    add(x, &conv3(&t, &d, &wt[&format!("{prefix}.res.2.weight")]))
}

/// DRUNet's forward pass on a CHW `[4, h, w]` input, `[3, h, w]` out — the file's graph, line by line.
pub(crate) fn forward(
    a: &super::arch::Arch,
    wt: &super::Weights,
    input: &[f32],
    width: usize,
    height: usize,
) -> Vec<f32> {
    let (c, nb) = (a.widths, a.blocks);
    let (mut w, mut h) = (width, height);
    let mut cur = conv3(input, &dims(w, h, 4, c[0]), &wt["m_head.weight"]);
    let mut skips = vec![cur.clone()];
    for l in 1..=3 {
        for b in 0..nb {
            cur = block(&cur, wt, &format!("m_down{l}.{b}"), c[l - 1], w, h);
        }
        cur = down2(
            &cur,
            &dims(w, h, c[l - 1], c[l]),
            &wt[&format!("m_down{l}.{nb}.weight")],
        );
        (w, h) = (w / 2, h / 2);
        skips.push(cur.clone());
    }
    for b in 0..nb {
        cur = block(&cur, wt, &format!("m_body.{b}"), c[3], w, h);
    }
    cur = add(&cur, &skips[3]);
    for l in (1..=3).rev() {
        cur = up2(
            &cur,
            &dims(w, h, c[l], c[l - 1]),
            &wt[&format!("m_up{l}.0.weight")],
        );
        (w, h) = (w * 2, h * 2);
        for b in 0..nb {
            cur = block(&cur, wt, &format!("m_up{l}.{}", b + 1), c[l - 1], w, h);
        }
        cur = add(&cur, &skips[l - 1]);
    }
    conv3(&cur, &dims(w, h, c[0], 3), &wt["m_tail.weight"])
}
