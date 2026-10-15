//! The kernel's WGSL: DRUNet's convolutions as one tiled matrix product — 64 rows (pixels) × 64 columns
//! (output channels) a workgroup, 16 deep, 4 × 4 outputs a thread — in three index modes. `MODE` is a
//! module constant, so each mode compiles to its own specialised pipeline.

use super::arch::Mode;

/// The kernel for `mode`.
pub(crate) fn shader(mode: Mode) -> String {
    format!("const MODE: u32 = {}u;\n{BODY}", mode as u32)
}

const BODY: &str = r#"
struct Params {
  in_w: u32, in_h: u32, out_w: u32, out_h: u32,
  m: u32, n: u32, k: u32, cout: u32,
  relu: u32, add1: u32, add2: u32, pad: u32,
}
@group(0) @binding(0) var<storage, read> src: array<f32>;
@group(0) @binding(1) var<storage, read> wts: array<f32>;
@group(0) @binding(2) var<storage, read_write> dst: array<f32>;
@group(0) @binding(3) var<storage, read> res1: array<f32>;
@group(0) @binding(4) var<storage, read> res2: array<f32>;
@group(0) @binding(5) var<uniform> p: Params;

const TM: u32 = 64u;
const TN: u32 = 64u;
const TK: u32 = 16u;
var<workgroup> sa: array<f32, 1024>; // [k][m]
var<workgroup> sb: array<f32, 1024>; // [k][n]

// The product's row m, column k: an input value, zero outside the image.
fn a_at(m: u32, k: u32) -> f32 {
  if (m >= p.m || k >= p.k) { return 0.0; }
  let in_hw = p.in_w * p.in_h;
  if (MODE == 0u) {
    let y = i32(m / p.out_w);
    let x = i32(m % p.out_w);
    let ci = k / 9u;
    let tap = k % 9u;
    let iy = y + i32(tap / 3u) - 1;
    let ix = x + i32(tap % 3u) - 1;
    if (iy < 0 || ix < 0 || iy >= i32(p.in_h) || ix >= i32(p.in_w)) { return 0.0; }
    return src[ci * in_hw + u32(iy) * p.in_w + u32(ix)];
  }
  if (MODE == 1u) {
    let y = m / p.out_w;
    let x = m % p.out_w;
    let ci = k / 4u;
    let tap = k % 4u;
    return src[ci * in_hw + (2u * y + tap / 2u) * p.in_w + 2u * x + tap % 2u];
  }
  return src[k * in_hw + m];
}

// Where row m, column n lands: the output plane's pixel — a quadrant's, for the transposed convolution.
fn out_at(m: u32, n: u32) -> u32 {
  if (MODE == 2u) {
    let y = m / p.in_w;
    let x = m % p.in_w;
    let q = n / p.cout;
    let co = n % p.cout;
    return co * p.out_w * p.out_h + (2u * y + q / 2u) * p.out_w + 2u * x + q % 2u;
  }
  return n * p.out_w * p.out_h + m;
}

@compute @workgroup_size(16, 16)
fn main(@builtin(local_invocation_id) lid: vec3<u32>, @builtin(workgroup_id) wid: vec3<u32>) {
  let t = lid.y * 16u + lid.x;
  let m0 = wid.x * TM;
  let n0 = wid.y * TN;
  // acc[j][i]: column n0 + lid.y + 16j, row m0 + lid.x + 16i — neighbouring threads, neighbouring pixels.
  var acc: array<vec4<f32>, 4>;
  for (var k0 = 0u; k0 < p.k; k0 = k0 + TK) {
    for (var r = 0u; r < 4u; r = r + 1u) {
      let e = t + r * 256u;
      sa[e] = a_at(m0 + e % TM, k0 + e / TM);
      let k = k0 + e / TN;
      let n = n0 + e % TN;
      var w = 0.0;
      if (k < p.k && n < p.n) { w = wts[k * p.n + n]; }
      sb[e] = w;
    }
    workgroupBarrier();
    for (var kk = 0u; kk < TK; kk = kk + 1u) {
      let a = vec4<f32>(sa[kk * TM + lid.x], sa[kk * TM + lid.x + 16u],
                        sa[kk * TM + lid.x + 32u], sa[kk * TM + lid.x + 48u]);
      for (var j = 0u; j < 4u; j = j + 1u) {
        acc[j] = acc[j] + a * sb[kk * TN + lid.y + 16u * j];
      }
    }
    workgroupBarrier();
  }
  for (var j = 0u; j < 4u; j = j + 1u) {
    let n = n0 + lid.y + 16u * j;
    for (var i = 0u; i < 4u; i = i + 1u) {
      let m = m0 + lid.x + 16u * i;
      if (m < p.m && n < p.n) {
        var v = acc[j][i];
        if (p.relu != 0u) { v = max(v, 0.0); }
        let o = out_at(m, n);
        if (p.add1 != 0u) { v = v + res1[o]; }
        if (p.add2 != 0u) { v = v + res2[o]; }
        dst[o] = v;
      }
    }
  }
}
"#;
