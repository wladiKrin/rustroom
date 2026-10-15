//! The kernel's WGSL: DRUNet's convolutions as one tiled matrix product — 64 rows (pixels) × 64 columns
//! (output channels) a workgroup, 16 deep, 4 × 4 outputs a thread — in three index modes. `MODE` is a
//! module constant, so each mode compiles to its own specialised pipeline.

use super::arch::Mode;

/// The kernel for `mode`.
pub(crate) fn shader(mode: Mode) -> String {
    format!("const MODE: u32 = {}u;\n{BODY}", mode as u32)
}

/// The kernel's tile — rows (pixels) and columns (output channels) a workgroup; `groups` divides by them.
pub(crate) const TM: u32 = 64;
pub(crate) const TN: u32 = 64;

const BODY: &str = r#"
struct Params {
  in_w: u32, in_h: u32, out_w: u32, out_h: u32,
  m: u32, n: u32, k: u32, cout: u32,
  relu: u32, add1: u32, add2: u32, g0: u32,
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
// [k][m / 4] and [k][n / 4]: k-row kk's rows (or columns) 4q … 4q + 3 at kk · 16 + q.
var<workgroup> sa: array<vec4<f32>, 256>;
var<workgroup> sb: array<vec4<f32>, 256>;

fn ld(i: u32, ok: bool) -> f32 {
  if (ok) { return src[i]; }
  return 0.0;
}

// ⚠⚠ Written for the Metal compiler naga hands it to, and every choice MEASURED (M10.16b2,
// `a_full_size_tile_time`, a 512 px tile of the full network on the M2, best of 3):
//
// | kernel | a tile |
// |---|---|
// | 4 × 4 outputs a thread in an array of accumulators indexed by the loop counter | 7.82 s |
// | 4 × 4 in four named vec4s, every runtime division hoisted out of the loop over k | 1.77 s |
// | **8 × 8 a thread, 64 threads, a 64 × 64 tile, 16 deep (this)** | **1.35 s** |
// | … 32 deep | 1.63 s |
// | … 256 threads on a 256 × 64 tile | 3.19 s |
// | … the next step's loads prefetched into registers | 1.46 s |
// | … each thread's two vec4s 32 rows apart (no bank conflicts) | 1.35 s, kept |
//
// ⚠ The array spilled every multiply-add to thread memory; more threads or registers cost occupancy.
@compute @workgroup_size(8, 8)
fn main(@builtin(local_invocation_id) lid: vec3<u32>, @builtin(workgroup_id) wid: vec3<u32>) {
  let t = lid.y * 8u + lid.x;
  // ⚠ `g0`: this band's first row-workgroup — a convolution runs as several submissions.
  let m0 = (wid.x + p.g0) * TM;
  let n0 = wid.y * TN;
  // This thread's load slots, the same every k-step: four adjacent rows (pixels) 4q … 4q + 3 and four
  // adjacent columns, at k-offsets t / 16 + 4r within the step.
  let q = t % 16u;
  let k_first = t / 16u;
  let in_hw = p.in_w * p.in_h;
  let ms = vec4<u32>(m0 + 4u * q) + vec4<u32>(0u, 1u, 2u, 3u);
  let ok = ms < vec4<u32>(p.m);
  var py = vec4<i32>(0);
  var px = vec4<i32>(0);
  if (MODE != 2u) {
    py = vec4<i32>(ms / p.out_w);
    px = vec4<i32>(ms % p.out_w);
  }
  let ns = vec4<u32>(n0 + 4u * q) + vec4<u32>(0u, 1u, 2u, 3u);
  let nok = ns < vec4<u32>(p.n);
  var acc00 = vec4<f32>(0.0);
  var acc01 = vec4<f32>(0.0);
  var acc02 = vec4<f32>(0.0);
  var acc03 = vec4<f32>(0.0);
  var acc04 = vec4<f32>(0.0);
  var acc05 = vec4<f32>(0.0);
  var acc06 = vec4<f32>(0.0);
  var acc07 = vec4<f32>(0.0);
  var acc10 = vec4<f32>(0.0);
  var acc11 = vec4<f32>(0.0);
  var acc12 = vec4<f32>(0.0);
  var acc13 = vec4<f32>(0.0);
  var acc14 = vec4<f32>(0.0);
  var acc15 = vec4<f32>(0.0);
  var acc16 = vec4<f32>(0.0);
  var acc17 = vec4<f32>(0.0);
  for (var k0 = 0u; k0 < p.k; k0 = k0 + TK) {
    for (var r = 0u; r < 4u; r = r + 1u) {
      let kk = k_first + 4u * r;
      let k = k0 + kk;
      var a = vec4<f32>(0.0);
      var b = vec4<f32>(0.0);
      if (k < p.k) {
        if (MODE == 0u) {
          let ci = k / 9u;
          let tap = k % 9u;
          let iy = py + vec4<i32>(i32(tap / 3u) - 1);
          let ix = px + vec4<i32>(i32(tap % 3u) - 1);
          let inb = ok & (iy >= vec4<i32>(0)) & (ix >= vec4<i32>(0))
            & (iy < vec4<i32>(i32(p.in_h))) & (ix < vec4<i32>(i32(p.in_w)));
          let at = vec4<u32>(ci * in_hw) + vec4<u32>(iy) * p.in_w + vec4<u32>(ix);
          a = vec4<f32>(ld(at.x, inb.x), ld(at.y, inb.y), ld(at.z, inb.z), ld(at.w, inb.w));
        } else if (MODE == 1u) {
          let ci = k / 4u;
          let tap = k % 4u;
          let at = vec4<u32>(ci * in_hw) + (vec4<u32>(2 * py) + vec4<u32>(tap / 2u)) * p.in_w
            + vec4<u32>(2 * px) + vec4<u32>(tap % 2u);
          a = vec4<f32>(ld(at.x, ok.x), ld(at.y, ok.y), ld(at.z, ok.z), ld(at.w, ok.w));
        } else {
          let at = vec4<u32>(k * in_hw) + ms;
          a = vec4<f32>(ld(at.x, ok.x), ld(at.y, ok.y), ld(at.z, ok.z), ld(at.w, ok.w));
        }
        let wb = k * p.n;
        b = vec4<f32>(
          select(0.0, wts[wb + ns.x], nok.x),
          select(0.0, wts[wb + min(ns.y, p.n - 1u)], nok.y),
          select(0.0, wts[wb + min(ns.z, p.n - 1u)], nok.z),
          select(0.0, wts[wb + min(ns.w, p.n - 1u)], nok.w));
      }
      sa[kk * 16u + q] = a;
      sb[kk * 16u + q] = b;
    }
    workgroupBarrier();
    for (var s = 0u; s < TK; s = s + 1u) {
      let a0 = sa[s * 16u + lid.x];
      let a1 = sa[s * 16u + 8u + lid.x];
      let b0 = sb[s * 16u + lid.y];
      let b1 = sb[s * 16u + 8u + lid.y];
      acc00 = acc00 + a0 * b0.x;
      acc01 = acc01 + a0 * b0.y;
      acc02 = acc02 + a0 * b0.z;
      acc03 = acc03 + a0 * b0.w;
      acc04 = acc04 + a0 * b1.x;
      acc05 = acc05 + a0 * b1.y;
      acc06 = acc06 + a0 * b1.z;
      acc07 = acc07 + a0 * b1.w;
      acc10 = acc10 + a1 * b0.x;
      acc11 = acc11 + a1 * b0.y;
      acc12 = acc12 + a1 * b0.z;
      acc13 = acc13 + a1 * b0.w;
      acc14 = acc14 + a1 * b1.x;
      acc15 = acc15 + a1 * b1.y;
      acc16 = acc16 + a1 * b1.z;
      acc17 = acc17 + a1 * b1.w;
    }
    workgroupBarrier();
  }
  // acc_rc's component i: row m0 + 32r + 4 · lid.x + i, column n0 + 32 · (c / 4) + 4 · lid.y + c % 4 —
  // a thread's two vec4s 32 rows apart, so neighbouring threads read neighbouring vec4s (no bank
  // conflicts in workgroup memory).
  let mr = m0 + 4u * lid.x;
  let nc = n0 + 4u * lid.y;
  store(acc00, mr + 0u, nc + 0u);
  store(acc01, mr + 0u, nc + 1u);
  store(acc02, mr + 0u, nc + 2u);
  store(acc03, mr + 0u, nc + 3u);
  store(acc04, mr + 0u, nc + 32u);
  store(acc05, mr + 0u, nc + 33u);
  store(acc06, mr + 0u, nc + 34u);
  store(acc07, mr + 0u, nc + 35u);
  store(acc10, mr + 32u, nc + 0u);
  store(acc11, mr + 32u, nc + 1u);
  store(acc12, mr + 32u, nc + 2u);
  store(acc13, mr + 32u, nc + 3u);
  store(acc14, mr + 32u, nc + 32u);
  store(acc15, mr + 32u, nc + 33u);
  store(acc16, mr + 32u, nc + 34u);
  store(acc17, mr + 32u, nc + 35u);
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

fn put(v0: f32, m: u32, n: u32) {
  if (m >= p.m || n >= p.n) { return; }
  var v = v0;
  if (p.relu != 0u) { v = max(v, 0.0); }
  let o = out_at(m, n);
  if (p.add1 != 0u) { v = v + res1[o]; }
  if (p.add2 != 0u) { v = v + res2[o]; }
  dst[o] = v;
}

fn store(acc: vec4<f32>, m: u32, n: u32) {
  put(acc.x, m, n);
  put(acc.y, m + 1u, n);
  put(acc.z, m + 2u, n);
  put(acc.w, m + 3u, n);
}
"#;
