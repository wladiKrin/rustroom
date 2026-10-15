//! Lens Blur's shader (M10.16d2): `pipeline::stages::lens_blur` line for line.
//!
//! ⚠⚠ **One constant text.** Every parameter is a uniform, and the per-level work is one entry point
//! per level (`build_l`, `gather_l`, l < [`crate::lens_blur::MAX_LEVELS`]), so the program compiles
//! once — `GpuCache` keys on the text. The constants that ARE baked in are the CPU's own values —
//! the aperture's edges, apothem and areas, the luminance weights, `RCAP`, `RAMP` — formatted by
//! `{:?}`, the shortest text that reads back as the same `f32`.

use edit_model::Bokeh;
use pipeline::lens_blur::aperture;

/// The entry points, in the order the program's pipelines come back.
pub(crate) const BUILD: [&str; 8] = [
    "build_0", "build_1", "build_2", "build_3", "build_4", "build_5", "build_6", "build_7",
];
pub(crate) const GATHER: [&str; 8] = [
    "gather_0", "gather_1", "gather_2", "gather_3", "gather_4", "gather_5", "gather_6", "gather_7",
];
pub(crate) const COMPOSITE: &str = "composite";

/// The bokeh's index in the uniform.
pub(crate) fn kind(b: Bokeh) -> u32 {
    match b {
        Bokeh::Circle => 0,
        Bokeh::Blades => 1,
        Bokeh::Ring => 2,
        Bokeh::CatEye => 3,
    }
}

/// The whole program.
pub(crate) fn shader() -> String {
    let e = aperture::blade_edges();
    let centre = [0.5, 0.5];
    let mut src = format!(
        r#"
const RCAP: f32 = {rcap:?};
const RCAP_I: i32 = {rcap_i};
const RAMP: f32 = {ramp:?};
const LUMA_R: f32 = {lr:?};
const LUMA_G: f32 = {lg:?};
const LUMA_B: f32 = {lb:?};
const APOTHEM: f32 = {apothem:?};
const E0S: f32 = {e0s:?};
const E0C: f32 = {e0c:?};
const E1S: f32 = {e1s:?};
const E1C: f32 = {e1c:?};
const E2S: f32 = {e2s:?};
const E2C: f32 = {e2c:?};
const CIRCLE_AREA: f32 = {circle:?};
const BLADES_AREA: f32 = {blades:?};
const RING_AREA: f32 = {ring:?};
const HALF_DIAG: f32 = {half_diag:?};
"#,
        rcap = pipeline::lens_blur::RCAP as f32,
        rcap_i = pipeline::lens_blur::RCAP,
        ramp = pipeline::lens_blur::RAMP,
        lr = color::PROPHOTO_LUMA[0],
        lg = color::PROPHOTO_LUMA[1],
        lb = color::PROPHOTO_LUMA[2],
        apothem = aperture::apothem(),
        e0s = e[0].0,
        e0c = e[0].1,
        e1s = e[1].0,
        e1c = e[1].1,
        e2s = e[2].0,
        e2c = e[2].1,
        circle = aperture::area(Bokeh::Circle, centre),
        blades = aperture::area(Bokeh::Blades, centre),
        ring = aperture::area(Bokeh::Ring, centre),
        half_diag = std::f32::consts::FRAC_1_SQRT_2,
    );
    src.push_str(BODY);
    for l in 0..BUILD.len() {
        src.push_str(&format!(
            r#"
@compute @workgroup_size(64)
fn build_{l}(@builtin(global_invocation_id) id: vec3<u32>, @builtin(num_workgroups) n: vec3<u32>) {{
    build({l}u, id.x + id.y * n.x * 64u);
}}

@compute @workgroup_size(64)
fn gather_{l}(@builtin(global_invocation_id) id: vec3<u32>, @builtin(num_workgroups) n: vec3<u32>) {{
    gather({l}u, id.x + id.y * n.x * 64u);
}}
"#
        ));
    }
    src
}

/// Everything but the constants and the per-level entry points.
const BODY: &str = r#"
struct Params {
    w: u32,
    h: u32,
    levels: u32,
    kind: u32,
    focus: f32,
    half_range: f32,
    radius_px: f32,
    boost_k: f32,
    l99: f32,
    has_subject: u32,
    pad0: u32,
    pad1: u32,
    // Per level: its width, its height, its scale k, its first cell.
    lv: array<vec4<u32>, 8>,
};

@group(0) @binding(0) var<storage, read_write> img: array<f32>;
@group(0) @binding(1) var<uniform> params: Params;
@group(0) @binding(2) var<storage, read> depth: array<u32>;
@group(0) @binding(3) var<storage, read> alpha: array<u32>;
// 12 a cell: per layer (background, foreground) the mean colour, the covered fraction, the mean radius
// in working pixels, the normaliser.
@group(0) @binding(4) var<storage, read_write> src: array<f32>;
// 8 a cell: per layer the received light and its weight.
@group(0) @binding(5) var<storage, read_write> got: array<f32>;

fn byte_of(word: u32, i: u32) -> f32 {
    return f32((word >> ((i & 3u) * 8u)) & 255u) / 255.0;
}

fn depth_at(i: u32) -> f32 {
    return byte_of(depth[i >> 2u], i);
}

fn alpha_at(i: u32) -> f32 {
    if (params.has_subject == 0u) {
        return 0.0;
    }
    return byte_of(alpha[i >> 2u], i);
}

// `radius::radius`.
fn radius_at(d: f32) -> f32 {
    return params.radius_px * clamp((abs(d - params.focus) - params.half_range) / RAMP, 0.0, 1.0);
}

// `gather::level_of`: the finest level at which the disc spans at most RCAP working pixels.
fn level_of(c: f32) -> u32 {
    var l = 0u;
    loop {
        if (l >= 31u || c * (1.0 / f32(1u << (l + 1u))) <= RCAP) {
            break;
        }
        l = l + 1u;
    }
    return l;
}

fn pixel(i: u32) -> vec3<f32> {
    return vec3<f32>(img[3u * i], img[3u * i + 1u], img[3u * i + 2u]);
}

// `gather::boost`, per pixel.
fn boosted(i: u32) -> vec3<f32> {
    let p = pixel(i);
    if (params.boost_k <= 0.0) {
        return p;
    }
    let l = LUMA_R * p.x + LUMA_G * p.y + LUMA_B * p.z;
    let t = clamp((l - params.l99) / (2.0 * params.l99), 0.0, 1.0);
    let f = 1.0 + params.boost_k * t * t * (3.0 - 2.0 * t);
    return p * f;
}

fn inside(b: bool) -> f32 {
    return select(0.0, 1.0, b);
}

fn cat_shift(pos: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(0.6 * (2.0 * pos.x - 1.0), 0.6 * (2.0 * pos.y - 1.0));
}

// `aperture::weight`.
fn weight(o: vec2<f32>, pos: vec2<f32>) -> f32 {
    let r = sqrt(o.x * o.x + o.y * o.y);
    switch params.kind {
        case 0u: {
            return inside(r <= 1.0);
        }
        case 1u: {
            return inside(
                abs(o.x * E0C + o.y * E0S) <= APOTHEM
                && abs(o.x * E1C + o.y * E1S) <= APOTHEM
                && abs(o.x * E2C + o.y * E2S) <= APOTHEM
            );
        }
        case 2u: {
            if (r > 1.0) {
                return 0.0;
            }
            let t = clamp((r - 0.55) / 0.4, 0.0, 1.0);
            return 0.35 + 0.65 * t * t * (3.0 - 2.0 * t);
        }
        default: {
            let hs = cat_shift(pos) / 2.0;
            let a = sqrt((o.x - hs.x) * (o.x - hs.x) + (o.y - hs.y) * (o.y - hs.y));
            let b = sqrt((o.x + hs.x) * (o.x + hs.x) + (o.y + hs.y) * (o.y + hs.y));
            return inside(a <= 1.0 && b <= 1.0);
        }
    }
}

// `aperture::area`.
fn area_at(pos: vec2<f32>) -> f32 {
    switch params.kind {
        case 0u: {
            return CIRCLE_AREA;
        }
        case 1u: {
            return BLADES_AREA;
        }
        case 2u: {
            return RING_AREA;
        }
        default: {
            let s = cat_shift(pos);
            let d = min(sqrt(s.x * s.x + s.y * s.y), 2.0);
            return 2.0 * acos(d / 2.0) - (d / 2.0) * sqrt(4.0 - d * d);
        }
    }
}

// `gather::cover`: 2 × 2 sub-tests at the disc's edge.
fn cover(dx: i32, dy: i32, rq: f32, pos: vec2<f32>) -> f32 {
    let dist = sqrt(f32(dx * dx + dy * dy));
    if ((dist - HALF_DIAG) / rq > 1.0) {
        return 0.0;
    }
    var inner = 0.5;
    if (params.kind == 1u) {
        inner = APOTHEM;
    }
    let x = f32(dx);
    let y = f32(dy);
    if ((dist + HALF_DIAG) / rq <= inner) {
        return weight(vec2<f32>(x / rq, y / rq), pos);
    }
    return 0.25 * (weight(vec2<f32>((x - 0.25) / rq, (y - 0.25) / rq), pos)
        + weight(vec2<f32>((x + 0.25) / rq, (y - 0.25) / rq), pos)
        + weight(vec2<f32>((x - 0.25) / rq, (y + 0.25) / rq), pos)
        + weight(vec2<f32>((x + 0.25) / rq, (y + 0.25) / rq), pos));
}

// The `pos` closure of `Level::gather`: over the PADDED frame, `lw · k`.
fn pos_of(l: u32, cx: u32, cy: u32) -> vec2<f32> {
    let v = params.lv[l];
    let k = f32(v.z);
    let fw = f32(v.x * v.z);
    let fh = f32(v.y * v.z);
    return vec2<f32>(
        clamp((f32(cx) + 0.5) * k / fw, 0.0, 1.0),
        clamp((f32(cy) + 0.5) * k / fh, 0.0, 1.0)
    );
}

// The source's normaliser — `Level::gather`'s `norm`.
fn norm_of(l: u32, cx: u32, cy: u32, rad: f32) -> f32 {
    let rq = max(rad, 1e-3);
    let at = pos_of(l, cx, cy);
    if (rq >= 3.0) {
        return 1.0 / (rq * rq * area_at(at));
    }
    let reach = i32(ceil(rq)) + 1;
    var sum = 0.0;
    for (var dy = -reach; dy <= reach; dy = dy + 1) {
        for (var dx = -reach; dx <= reach; dx = dx + 1) {
            sum = sum + cover(dx, dy, rq, at);
        }
    }
    if (sum > 0.0) {
        return 1.0 / sum;
    }
    return 0.0;
}

fn store(b: u32, l: u32, cx: u32, cy: u32, col: vec3<f32>, f: f32, r: f32, k: u32) {
    var c = vec3<f32>(0.0);
    var frac = 0.0;
    var rad = 0.0;
    var norm = 0.0;
    if (f > 0.0) {
        c = col / f;
        rad = r / f / f32(k);
        frac = f / f32(k * k);
        norm = norm_of(l, cx, cy, rad);
    }
    src[b] = c.x;
    src[b + 1u] = c.y;
    src[b + 2u] = c.z;
    src[b + 3u] = frac;
    src[b + 4u] = rad;
    src[b + 5u] = norm;
}

// `Level::build`, as a gather: one cell sums its k × k block, row-major — the CPU's order.
fn build(l: u32, j: u32) {
    let v = params.lv[l];
    let lw = v.x;
    let lh = v.y;
    let k = v.z;
    if (j >= lw * lh) {
        return;
    }
    let cx = j % lw;
    let cy = j / lw;
    let near_edge = params.focus - params.half_range;
    var col0 = vec3<f32>(0.0);
    var col1 = vec3<f32>(0.0);
    var f0 = 0.0;
    var f1 = 0.0;
    var r0 = 0.0;
    var r1 = 0.0;
    let ye = min((cy + 1u) * k, params.h);
    let xe = min((cx + 1u) * k, params.w);
    for (var y = cy * k; y < ye; y = y + 1u) {
        for (var x = cx * k; x < xe; x = x + 1u) {
            let i = y * params.w + x;
            let wgt = 1.0 - alpha_at(i);
            let d = depth_at(i);
            let c = radius_at(d) * wgt;
            if (c < 0.5 || wgt <= 0.0 || level_of(c) != l) {
                continue;
            }
            let p = boosted(i);
            if (d < near_edge) {
                col1 = col1 + p * wgt;
                f1 = f1 + wgt;
                r1 = r1 + c * wgt;
            } else {
                col0 = col0 + p * wgt;
                f0 = f0 + wgt;
                r0 = r0 + c * wgt;
            }
        }
    }
    let b = (v.w + j) * 12u;
    store(b, l, cx, cy, col0, f0, r0, k);
    store(b + 6u, l, cx, cy, col1, f1, r1, k);
}

// `Level::gather`: the dense pass over the disc of reach RCAP — dy, then dx, then the layer.
fn gather(l: u32, p: u32) {
    let v = params.lv[l];
    let lw = v.x;
    let lh = v.y;
    if (p >= lw * lh) {
        return;
    }
    let px = i32(p % lw);
    let py = i32(p / lw);
    var n0 = vec3<f32>(0.0);
    var n1 = vec3<f32>(0.0);
    var d0 = 0.0;
    var d1 = 0.0;
    for (var dy = -RCAP_I; dy <= RCAP_I; dy = dy + 1) {
        let qy = py + dy;
        if (qy < 0 || qy >= i32(lh)) {
            continue;
        }
        for (var dx = -RCAP_I; dx <= RCAP_I; dx = dx + 1) {
            let qx = px + dx;
            if (qx < 0 || qx >= i32(lw)) {
                continue;
            }
            let q = u32(qy) * lw + u32(qx);
            let d2 = f32(dx * dx + dy * dy);
            let b = (v.w + q) * 12u;
            for (var layer = 0u; layer < 2u; layer = layer + 1u) {
                let o = b + layer * 6u;
                let f = src[o + 3u];
                if (f <= 0.0) {
                    continue;
                }
                let rq = max(src[o + 4u], 1e-3);
                if (d2 > (rq + 1.0) * (rq + 1.0)) {
                    continue;
                }
                let sw = cover(-dx, -dy, rq, pos_of(l, u32(qx), u32(qy)));
                if (sw <= 0.0) {
                    continue;
                }
                let e = f * sw * src[o + 5u];
                let light = vec3<f32>(src[o], src[o + 1u], src[o + 2u]) * e;
                if (layer == 0u) {
                    n0 = n0 + light;
                    d0 = d0 + e;
                } else {
                    n1 = n1 + light;
                    d1 = d1 + e;
                }
            }
        }
    }
    let g = (v.w + p) * 8u;
    got[g] = n0.x;
    got[g + 1u] = n0.y;
    got[g + 2u] = n0.z;
    got[g + 3u] = d0;
    got[g + 4u] = n1.x;
    got[g + 5u] = n1.y;
    got[g + 6u] = n1.z;
    got[g + 7u] = d1;
}

// `Gathered::add_into` for every level, then `composite`: the three layers, back to front.
fn composite_px(i: u32) {
    if (i >= params.w * params.h) {
        return;
    }
    let x = i % params.w;
    let y = i / params.w;
    var n0 = vec3<f32>(0.0);
    var n1 = vec3<f32>(0.0);
    var d0 = 0.0;
    var d1 = 0.0;
    for (var l = 0u; l < params.levels; l = l + 1u) {
        let v = params.lv[l];
        let k = f32(v.z);
        let lw = v.x;
        let lh = v.y;
        let fy = clamp((f32(y) + 0.5) / k - 0.5, 0.0, f32(lh - 1u));
        let y0 = u32(floor(fy));
        let ty = fract(fy);
        let y1 = min(y0 + 1u, lh - 1u);
        let fx = clamp((f32(x) + 0.5) / k - 0.5, 0.0, f32(lw - 1u));
        let x0 = u32(floor(fx));
        let tx = fract(fx);
        let x1 = min(x0 + 1u, lw - 1u);
        var js = array<u32, 4>(y0 * lw + x0, y0 * lw + x1, y1 * lw + x0, y1 * lw + x1);
        var ts = array<f32, 4>((1.0 - tx) * (1.0 - ty), tx * (1.0 - ty), (1.0 - tx) * ty, tx * ty);
        for (var t = 0u; t < 4u; t = t + 1u) {
            let wt = ts[t];
            if (wt <= 0.0) {
                continue;
            }
            let g = (v.w + js[t]) * 8u;
            n0 = n0 + vec3<f32>(got[g], got[g + 1u], got[g + 2u]) * wt;
            d0 = d0 + got[g + 3u] * wt;
            n1 = n1 + vec3<f32>(got[g + 4u], got[g + 5u], got[g + 6u]) * wt;
            d1 = d1 + got[g + 7u] * wt;
        }
    }
    let s = pixel(i);
    let a = alpha_at(i);
    let c = radius_at(depth_at(i)) * (1.0 - a);
    let t = clamp(1.0 - c / 1.5, 0.0, 1.0);
    var res = s;
    if (t < 1.0 && d0 > 1e-6) {
        let bg = n0 / d0;
        res = bg + (s - bg) * t;
    }
    if (a >= 1.0) {
        res = s;
    } else if (a > 0.0) {
        res = res + (s - res) * a;
    }
    let cov = min(d1, 1.0);
    if (cov > 0.0) {
        let fg = n1 / d1;
        res = res + (fg - res) * cov;
    }
    img[3u * i] = res.x;
    img[3u * i + 1u] = res.y;
    img[3u * i + 2u] = res.z;
}

@compute @workgroup_size(64)
fn composite(@builtin(global_invocation_id) id: vec3<u32>, @builtin(num_workgroups) n: vec3<u32>) {
    composite_px(id.x + id.y * n.x * 64u);
}
"#;
