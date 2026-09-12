//! The brush rasteriser's compute shader.
//!
//! ⚠⚠ **One dispatch, one workgroup per 16×16 tile.** The workgroup's 256 threads cooperatively
//! scan the dab list once and append the dabs touching this tile to a `var<workgroup>` array; then
//! each thread rasterises its own pixel from that shared list. The alternative — a pixel testing
//! every dab — is 3.05 Mpx × 3,645 = **11.1 billion** tests on the real 36-stroke brush, against
//! **43.4 million** this way.
//!
//! ⚠ Not a [`crate::dispatch::Dispatch`] client. That type is `workgroup_size(64)` over *pixels*
//! with bindings fixed by convention; this needs one workgroup per *tile* and a workgroup-shared
//! array. `sharpen` is the existing precedent for a stage that encodes its own dispatch.

/// Dabs a single tile's shared list can hold before it falls back to re-scanning.
///
/// ⚠⚠ **4095 `u32` plus the counter is 16384 bytes, which is exactly
/// `Limits::default().max_compute_workgroup_storage_size`** — verified in `wgpu-types` rather than
/// assumed, and `GpuContext` requests `DeviceDescriptor::default()`, so that default is what it
/// gets. Raising it needs a device that advertises more, which is not something to assume.
///
/// ⚠ The real brush averages **22** dabs per tile (3,645 × ~72 tiles each / ~11,900 tiles), so
/// this is not a number anything realistic approaches — which is precisely why the overflow path
/// has to be reachable by injection in a test. *An unreachable guard is an untested one.*
pub(crate) const MAX_TILE_DABS: u32 = 4095;

/// Tile edge in pixels. 16 × 16 = 256 threads, one per pixel.
pub(crate) const TILE: u32 = 16;

/// The shader, with the Oklab prelude prepended and the two constants substituted.
pub(crate) fn source(max_tile_dabs: u32, count_tests: bool) -> String {
    format!(
        "{}\n{}",
        crate::oklab_wgsl::OKLAB_WGSL,
        BODY.replace("{MAX_TILE_DABS}", &max_tile_dabs.to_string())
            .replace("{TILE}", &TILE.to_string())
            .replace("{COUNT_TESTS}", if count_tests { "true" } else { "false" })
    )
}

const BODY: &str = r#"
struct Dab {
    // x, y, r, hard
    pos: vec4<f32>,
    // Oklab reference in xyz, the gate ceiling in w. A NEGATIVE ceiling means "no gate".
    gate: vec4<f32>,
    stroke: u32,
    pad0: u32,
    pad1: u32,
    pad2: u32,
};

struct StrokeInfo {
    flow: f32,
    // 1 = erase (MaskOp::Subtract), 0 = paint (MaskOp::Add).
    erase: u32,
    pad0: u32,
    pad1: u32,
};

struct Params {
    x0: u32, y0: u32, w: u32, h: u32,
    img_w: u32, img_h: u32,
    dab_count: u32,
    invert: u32,
    has_base: u32,
    tiles_x: u32,
    pad0: u32,
    pad1: u32,
};

@group(0) @binding(0) var<storage, read> dabs: array<Dab>;
@group(0) @binding(1) var<storage, read> strokes: array<StrokeInfo>;
@group(0) @binding(2) var<storage, read> base: array<f32>;
@group(0) @binding(3) var<storage, read_write> cov: array<u32>;
@group(0) @binding(4) var<uniform> p: Params;
@group(0) @binding(5) var<storage, read_write> counters: array<atomic<u32>>;

var<workgroup> tile_dabs: array<u32, {MAX_TILE_DABS}>;
var<workgroup> tile_count: atomic<u32>;
var<workgroup> overflowed: u32;

// ⚠ Transcribed from `pipeline::brush::raster::dab_alpha`. The `hard >= 1 - 1e-4` branch is a fast
// path ONLY — the `q >= 1` return above already excludes the `0/0` case, so it is not a NaN guard
// and removing it changes no output. That is recorded at the Rust constant and repeated here
// because the shape invites the opposite reading.
fn dab_alpha(d: f32, r: f32, hard: f32) -> f32 {
    if (r <= 0.0) { return 0.0; }
    let q = d / r;
    if (q >= 1.0) { return 0.0; }
    if (hard >= 1.0 - 1e-4) { return 1.0; }
    let t = clamp((1.0 - q) / (1.0 - hard), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

// ⚠⚠ Transcribed from `auto_mask_gate`, and it uses ALL THREE Oklab components — the lightness
// term is load-bearing rather than incidental. An earlier CPU design had to drop it because its
// reference was sampled from the *displayed* frame while the pixels came from `MaskStage`'s input;
// `auto_mask_space_probe` measured that lightness disagreement at a mean of +0.07 to +0.11, 7-10x
// the chroma disagreement. Both ends are read from one buffer here, so all three stay.
fn auto_mask_gate(px: vec3<f32>, reference: vec3<f32>, ceiling: f32) -> f32 {
    let d = length(px - reference);
    let lo = ceiling * 0.5;
    if (d <= lo) { return 1.0; }
    if (d >= ceiling) { return 0.0; }
    let q = (d - lo) / (ceiling - lo);
    return 1.0 - q * q * (3.0 - 2.0 * q);
}

// ⚠ `prophoto_to_oklab` takes three SCALARS, not a vec3 — checked in `oklab_wgsl` rather than
// guessed. A vec3 call is a compile error, which is the harmless direction.
fn base_oklab(fx: u32, fy: u32) -> vec3<f32> {
    let i = (fy * p.img_w + fx) * 3u;
    return prophoto_to_oklab(base[i], base[i + 1u], base[i + 2u]);
}

fn touches_tile(d: Dab, tx0: f32, ty0: f32, tx1: f32, ty1: f32) -> bool {
    let r = d.pos.z;
    return !(d.pos.x + r < tx0 || d.pos.x - r >= tx1 || d.pos.y + r < ty0 || d.pos.y - r >= ty1);
}

// ⚠ `MaskOp`'s two rules, and the SAME two `MaskOp::combine` offers between masks. Erase is
// `c * (1 - a)` and paint is the standard "over" — not `min`/`max`, which agree with these at 0
// and 1 and differ at every partial coverage.
fn composite(cover: f32, a: f32, erase: u32) -> f32 {
    if (a <= 0.0) { return cover; }
    if (erase == 1u) { return cover * (1.0 - a); }
    return cover + (1.0 - cover) * a;
}

@compute @workgroup_size({TILE}, {TILE})
fn main(
    @builtin(workgroup_id) wg: vec3<u32>,
    @builtin(local_invocation_index) li: u32,
) {
    let tile_x = wg.x % p.tiles_x;
    let tile_y = wg.x / p.tiles_x;

    // The tile in FRAME coordinates, which is the space every dab centre is in.
    let tx0 = f32(p.x0 + tile_x * {TILE}u);
    let ty0 = f32(p.y0 + tile_y * {TILE}u);
    let tx1 = tx0 + f32({TILE}u);
    let ty1 = ty0 + f32({TILE}u);

    if (li == 0u) {
        atomicStore(&tile_count, 0u);
        overflowed = 0u;
    }
    workgroupBarrier();

    // ⚠ Cooperative: each of the 256 threads strides through the dab list, so the scan costs
    // dab_count/256 iterations per thread rather than dab_count.
    var i = li;
    loop {
        if (i >= p.dab_count) { break; }
        if (touches_tile(dabs[i], tx0, ty0, tx1, ty1)) {
            let slot = atomicAdd(&tile_count, 1u);
            if (slot < {MAX_TILE_DABS}u) {
                tile_dabs[slot] = i;
            } else {
                overflowed = 1u;
            }
        }
        i = i + {TILE}u * {TILE}u;
    }
    workgroupBarrier();

    // This thread's pixel, in box coordinates then frame coordinates.
    let bx = tile_x * {TILE}u + (li % {TILE}u);
    let by = tile_y * {TILE}u + (li / {TILE}u);
    if (bx >= p.w || by >= p.h) { return; }
    let fx = p.x0 + bx;
    let fy = p.y0 + by;
    let px = f32(fx) + 0.5;
    let py = f32(fy) + 0.5;

    // ⚠ Read once per pixel, never per dab — the CPU builds a whole `base_lab` plane for exactly
    // this reason, because dabs overlap heavily and a lazy per-dab conversion would convert the
    // same pixel a dozen times. Touched only when something actually gates.
    var lab = vec3<f32>(0.0, 0.0, 0.0);
    var lab_ready = false;

    // ⚠⚠ **The overflow path re-scans the WHOLE dab list per pixel rather than clipping.** A
    // kernel that silently used the first {MAX_TILE_DABS} would render a plausible-but-wrong mask,
    // which is the failure this whole module is written against. Correct, slower, and confined to
    // the tile that overflowed.
    var count = min(atomicLoad(&tile_count), {MAX_TILE_DABS}u);
    if (overflowed == 1u) { count = p.dab_count; }

    var cover = 0.0;
    var cur_stroke = 0xffffffffu;
    var stroke_max = 0.0;
    var gate_tests = 0u;
    var dab_tests = 0u;

    var k = 0u;
    loop {
        if (k >= count) { break; }
        // ⚠⚠ **Ascending index IS stroke order**, because `plan_dabs` emits dabs stroke-major.
        // That is what lets the shared list stay a plain sorted index list with no stroke
        // boundaries in it, and it is why the scan above must append in index order.
        var di = k;
        if (overflowed == 0u) { di = tile_dabs[k]; }
        let d = dabs[di];

        if (d.stroke != cur_stroke) {
            // ⚠⚠ Composite the stroke that just ENDED, then start the new one. Two levels: `max`
            // within a stroke, composite across strokes in order.
            if (cur_stroke != 0xffffffffu) {
                cover = composite(cover, stroke_max * strokes[cur_stroke].flow, strokes[cur_stroke].erase);
            }
            cur_stroke = d.stroke;
            stroke_max = 0.0;
        }

        let dist = length(vec2<f32>(px - d.pos.x, py - d.pos.y));
        var a = dab_alpha(dist, d.pos.z, d.pos.w);
        dab_tests = dab_tests + 1u;
        // ⚠⚠ **THE SHORT-CIRCUIT BEFORE THE GATE IS LOAD-BEARING.** The gate can only ever REDUCE
        // an alpha, so a dab whose positional alpha already loses the running max cannot change
        // the pixel however its gate resolves — and dabs are laid a quarter-radius apart and
        // overlap heavily, so most of them lose. Gating first is *correct* and dramatically
        // slower, and **every equivalence test would still pass**; only the counter below sees it.
        if (a > stroke_max) {
            if (d.gate.w >= 0.0 && p.has_base == 1u) {
                if (!lab_ready) {
                    lab = base_oklab(fx, fy);
                    lab_ready = true;
                }
                a = a * auto_mask_gate(lab, d.gate.xyz, d.gate.w);
                gate_tests = gate_tests + 1u;
            }
            stroke_max = max(stroke_max, a);
        }
        k = k + 1u;
    }
    // The last stroke, which the loop's stroke-change branch never reaches.
    if (cur_stroke != 0xffffffffu) {
        cover = composite(cover, stroke_max * strokes[cur_stroke].flow, strokes[cur_stroke].erase);
    }

    if (p.invert == 1u) { cover = 1.0 - cover; }
    // ⚠⚠ **NaN reaches 0 explicitly.** WGSL has neither Rust's saturating float-to-int cast nor
    // its NaN-propagating clamp, and `ImageBuf::quantize` relies on the cast. `c != c` is the test.
    //
    // ⚠⚠ **`floor(x + 0.5)`, never `round`.** WGSL's `round` breaks ties to the EVEN whole number
    // while Rust's `f32::round` breaks them away from zero — M8.3g's off-by-one-level bug, correct
    // on every ordinary value and wrong on every exact tie. Do not "tidy" this.
    var q = 0.0;
    if (cover == cover) { q = floor(clamp(cover, 0.0, 1.0) * 255.0 + 0.5); }
    // ⚠ One `u32` per sample rather than a packed byte array: packing four samples into one word
    // means four threads read-modify-writing the same address with no synchronisation, which is a
    // race. The map is 6.5 MB at u8 against 26 MB at u32 and the download is ~1.9 ms either way at
    // this size, so correctness wins. Narrowed on the CPU.
    cov[by * p.w + bx] = u32(q);

    if ({COUNT_TESTS}) {
        atomicAdd(&counters[0], gate_tests);
        atomicAdd(&counters[1], dab_tests);
    }
}
"#;
