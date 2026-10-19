//! Winograd F(2 × 2, 3 × 3) for DRUNet's 3 × 3 convolutions (M10.16b5): 16 products for each 2 × 2 block
//! of outputs instead of 36. The weights are transformed once at upload (`U = G g Gᵀ`); a tile's
//! convolution is an input transform (`V = Bᵀ d B` per 4 × 4 patch), the product's batched mode (16
//! products `M = V · U` in one dispatch) and an output transform (`Y = Aᵀ M A`) that carries the ReLU and
//! the two adds.

use super::arch::Tensor;

/// `G` — a 3-tap filter to the 4 positions.
const G: [[f64; 3]; 4] = [
    [1.0, 0.0, 0.0],
    [0.5, 0.5, 0.5],
    [0.5, -0.5, 0.5],
    [0.0, 0.0, 1.0],
];

/// `U = G g Gᵀ` for every `(co, ci)` of a 3 × 3 weight `W[co][ci][ky][kx]`: 16 matrices `[cin][cout]`,
/// position `ξ = i · 4 + j` first — each the product's B for that position.
pub(crate) fn transform_weights(t: &Tensor) -> Vec<f32> {
    let (cout, cin) = (t.shape[0], t.shape[1]);
    let mut u = vec![0f32; 16 * cin * cout];
    for co in 0..cout {
        for ci in 0..cin {
            let w = |ky: usize, kx: usize| t.data[((co * cin + ci) * 3 + ky) * 3 + kx] as f64;
            for i in 0..4 {
                for j in 0..4 {
                    let mut s = 0f64;
                    for (a, ga) in G[i].iter().enumerate() {
                        for (b, gb) in G[j].iter().enumerate() {
                            s += ga * w(a, b) * gb;
                        }
                    }
                    u[(i * 4 + j) * cin * cout + ci * cout + co] = s as f32;
                }
            }
        }
    }
    u
}

/// The two transforms' parameters: the plane's size and channels, the block grid, the store's fusions —
/// the bias (M10.16b6) added before the ReLU.
pub(crate) fn uniform(
    w: u32,
    h: u32,
    c: u32,
    relu: bool,
    bias: bool,
    add1: bool,
    add2: bool,
) -> [u32; 12] {
    [
        w,
        h,
        c,
        w.div_ceil(2),
        h.div_ceil(2),
        relu as u32,
        add1 as u32,
        add2 as u32,
        bias as u32,
        0,
        0,
        0,
    ]
}

/// The transforms' workgroups (64 threads, one per channel and block), split 2-D past the 65,535 limit.
pub(crate) fn groups(w: u32, h: u32, c: u32) -> (u32, u32) {
    let threads = w.div_ceil(2) as u64 * h.div_ceil(2) as u64 * c as u64;
    let g = threads.div_ceil(64).max(1);
    let gx = g.min(65_535) as u32;
    (gx, g.div_ceil(gx as u64) as u32)
}

/// The transforms' WGSL, storing in `f16` when `half`: entry points `input` and `output`.
pub(crate) fn shader(half: bool) -> String {
    let head = if half {
        "enable f16;\nalias S = f16;"
    } else {
        "alias S = f32;"
    };
    let mut loads = String::new();
    for i in 0..4 {
        for j in 0..4 {
            loads += &format!("  let d{i}{j} = at(c, y0 + {i}, x0 + {j});\n");
        }
    }
    // t = Bᵀ d (rows), V = t B (columns); Bᵀ = [[1,0,-1,0],[0,1,1,0],[0,-1,1,0],[0,1,0,-1]].
    let mut input = String::new();
    for j in 0..4 {
        input += &format!(
            "  let t0{j} = d0{j} - d2{j};\n  let t1{j} = d1{j} + d2{j};\n  let t2{j} = d2{j} - d1{j};\n  let t3{j} = d1{j} - d3{j};\n"
        );
    }
    for r in 0..4 {
        let v = [
            format!("t{r}0 - t{r}2"),
            format!("t{r}1 + t{r}2"),
            format!("t{r}2 - t{r}1"),
            format!("t{r}1 - t{r}3"),
        ];
        for (k, e) in v.iter().enumerate() {
            input += &format!("  dst[{}u * stride + c * nb + b] = S({e});\n", r * 4 + k);
        }
    }
    // t = Aᵀ m (two rows), Y = t A; Aᵀ = [[1,1,1,0],[0,1,-1,-1]].
    let mut output = String::new();
    for i in 0..4 {
        for j in 0..4 {
            output += &format!(
                "  let m{i}{j} = f32(src[{}u * stride + c * nb + b]);\n",
                i * 4 + j
            );
        }
    }
    for j in 0..4 {
        output += &format!(
            "  let t0{j} = m0{j} + m1{j} + m2{j};\n  let t1{j} = m1{j} - m2{j} - m3{j};\n"
        );
    }
    for r in 0..2 {
        output += &format!(
            "  put(c, 2u * by + {r}u, 2u * bx, t{r}0 + t{r}1 + t{r}2);\n  put(c, 2u * by + {r}u, 2u * bx + 1u, t{r}1 - t{r}2 - t{r}3);\n"
        );
    }
    format!(
        r#"{head}
struct T {{
  w: u32, h: u32, c: u32, nbx: u32,
  nby: u32, relu: u32, add1: u32, add2: u32,
  bias: u32, p1: u32, p2: u32, p3: u32,
}}
@group(0) @binding(0) var<storage, read> src: array<S>;
@group(0) @binding(1) var<storage, read_write> dst: array<S>;
@group(0) @binding(2) var<storage, read> res1: array<S>;
@group(0) @binding(3) var<storage, read> res2: array<S>;
@group(0) @binding(4) var<uniform> p: T;
@group(0) @binding(5) var<storage, read> bias_of: array<S>;

// The plane's value at (y, x) of channel c — zero outside, the convolution's own padding.
fn at(c: u32, y: i32, x: i32) -> f32 {{
  if (y < 0 || x < 0 || y >= i32(p.h) || x >= i32(p.w)) {{ return 0.0; }}
  return f32(src[c * p.w * p.h + u32(y) * p.w + u32(x)]);
}}

// One output pixel, cropped at an odd edge, with its bias, ReLU and the two adds.
fn put(c: u32, y: u32, x: u32, v0: f32) {{
  if (y >= p.h || x >= p.w) {{ return; }}
  var v = v0;
  if (p.bias != 0u) {{ v = v + f32(bias_of[c]); }}
  if (p.relu != 0u) {{ v = max(v, 0.0); }}
  let o = c * p.w * p.h + y * p.w + x;
  if (p.add1 != 0u) {{ v = v + f32(res1[o]); }}
  if (p.add2 != 0u) {{ v = v + f32(res2[o]); }}
  dst[o] = S(v);
}}

// V[ξ][c][block] = Bᵀ d B of each 4 × 4 input patch.
@compute @workgroup_size(64)
fn input(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {{
  let nb = p.nbx * p.nby;
  let idx = gid.x + gid.y * nwg.x * 64u;
  if (idx >= nb * p.c) {{ return; }}
  let c = idx / nb;
  let b = idx % nb;
  let y0 = i32(2u * (b / p.nbx)) - 1;
  let x0 = i32(2u * (b % p.nbx)) - 1;
  let stride = p.c * nb;
{loads}{input}}}

// Y = Aᵀ M A of each block's 16 products, stored with the fusions.
@compute @workgroup_size(64)
fn output(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {{
  let nb = p.nbx * p.nby;
  let idx = gid.x + gid.y * nwg.x * 64u;
  if (idx >= nb * p.c) {{ return; }}
  let c = idx / nb;
  let b = idx % nb;
  let by = b / p.nbx;
  let bx = b % p.nbx;
  let stride = p.c * nb;
{output}}}
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The weight transform against its definition on one known filter: an identity tap (centre 1)
    /// becomes `G e₁ e₁ᵀ Gᵀ` — the outer product of G's middle column with itself.
    #[test]
    fn the_weight_transform_is_g_g_gt() {
        let mut data = vec![0.0; 9];
        data[4] = 1.0;
        let u = transform_weights(&Tensor {
            shape: vec![1, 1, 3, 3],
            data,
        });
        let col = [0.0, 0.5, -0.5, 0.0];
        for i in 0..4 {
            for j in 0..4 {
                assert!(
                    (u[i * 4 + j] - (col[i] * col[j]) as f32).abs() < 1e-7,
                    "{i},{j}"
                );
            }
        }
    }
}
