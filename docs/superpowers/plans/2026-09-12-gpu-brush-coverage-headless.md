# M8.3k plan 1 — the brush coverage rasteriser on the GPU, headless

> **⚠ Conventions that are not negotiable (this repo):** commit messages carry **no
> `Co-Authored-By` trailer**; work happens on branch `m8.3k-gpu-brush-coverage`; **never push** —
> the user pushes to `github.com/wladiKrin/rustroom` himself.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this
> plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A `GpuBrushRaster` in `gpu-pipeline` that produces a `CoverageMap` agreeing with
`pipeline::brush::rasterize_in`, proved on synthetic fixtures **and on the real 36-stroke brush**.

**Architecture:** `rasterize_in`'s dab-building half is extracted into a public `plan_dabs` so the
CPU rasteriser and the GPU one consume **one** dab list. `GpuBrushRaster` uploads that list, runs
**one** compute dispatch — one workgroup per 16×16 tile, cooperatively building a workgroup-shared
list of the dabs touching that tile, then one thread per pixel — and downloads a `u8` map.

**Tech Stack:** Rust, `wgpu` 29 / WGSL, no new dependencies.

**Spec:** `docs/superpowers/specs/2026-09-12-gpu-brush-coverage-design.md` — §4 and §4.1 are the
semantics this plan must reproduce and are quoted where needed.

⚠ **Plan 1 ships nothing a user can see.** `MaskStage` is untouched; wiring is plan 2. The
acceptance criterion is agreement with the CPU, not a speed-up.

---

## File structure

| File | Responsibility | Change |
|---|---|---|
| `crates/pipeline/src/brush/raster.rs` | dab planning + the CPU kernel | **Modify** — extract `plan_dabs`, make `Dab`/`Stroke`/`DabPlan` public |
| `crates/gpu-pipeline/src/brush_raster.rs` | the GPU rasteriser | **Create** |
| `crates/gpu-pipeline/src/brush_wgsl.rs` | the shader source | **Create** |
| `crates/gpu-pipeline/src/lib.rs` | exports | **Modify** — `pub use brush_raster::GpuBrushRaster` |

⚠ `raster.rs` is **1,833 lines** and already well past the ~600 marker. This plan **reduces** its
production half by moving dab planning out, and adds nothing to it. Reported rather than glossed.

⚠ The shader lives in its own module beside `oklab_wgsl` and `luma_wgsl`, following the precedent
that WGSL source is a Rust `&str` constant assembled from shared preludes.

---

## Task 1: Extract `plan_dabs` — a pure refactor

**Files:**
- Modify: `crates/pipeline/src/brush/raster.rs`
- Test: its existing `mod tests`

- [ ] **Step 1: Record the baseline that proves this moved nothing**

```bash
cargo test -p pipeline > /tmp/k1-before.log 2>&1; echo "EXIT=$?"
grep "test result: ok" /tmp/k1-before.log | sed -E 's/.*ok\. ([0-9]+) passed.*/\1/' | awk '{s+=$1} END {print "tests:", s}'
grep -c "^test golden_" /tmp/k1-before.log
```

Expected: `EXIT=0`, and note both numbers. ⚠ **The acceptance for this task is "unchanged at N",
not "still green"** — W0's rule: a refactor that changes the test count moved behaviour.

- [ ] **Step 2: Make the three types public and add `DabPlan`**

In `crates/pipeline/src/brush/raster.rs`, change `struct Dab` to `pub struct Dab` and
`struct Stroke` to `pub struct Stroke`, making every field `pub`, and add after them:

```rust
/// Every dab a brush produces, grouped by stroke, with the box they fall in.
///
/// ⚠⚠ **One dab list, two consumers** — the CPU kernel below and `gpu-pipeline`'s
/// `GpuBrushRaster`. That is the same rule `adjustment_stages` follows for stages: the duplication
/// that would actually hurt is this arithmetic (the `SPACING` interpolation, the per-dab gate
/// reference, the clip rejection, the box), and it exists once.
///
/// ⚠ `x0`/`y0`/`w`/`h` are the **clipped** box, already clamped exactly as the coverage map's is,
/// so a consumer never re-derives it and the two cannot disagree about which pixels exist.
pub struct DabPlan {
    pub strokes: Vec<Stroke>,
    pub x0: usize,
    pub y0: usize,
    pub w: usize,
    pub h: usize,
    /// Coverage everywhere outside the box — 255 for an inverted brush.
    pub outside: u8,
    /// The row span a gating dab can read, if anything gates.
    pub gating_rows: Option<(usize, usize)>,
}
```

- [ ] **Step 2b: Extend the module's re-export list**

⚠ `crates/pipeline/src/brush/mod.rs` re-exports an **explicit** list, not a glob:

```rust
pub use raster::{rasterize, rasterize_in, BasePixels, CoverageMap};
```

So `pipeline::brush::plan_dabs` does not resolve until that line becomes:

```rust
pub use raster::{
    plan_dabs, rasterize, rasterize_in, BasePixels, CoverageMap, Dab, DabPlan, Stroke,
};
```

⚠ Without this the whole of Task 2 fails to compile with `plan_dabs` not found — which reads as a
missing function rather than a missing export.

- [ ] **Step 3: Split `rasterize_in` at the box**

Move the body of `rasterize_in` from its start **through** the `gating_rows` computation into:

```rust
/// Plan every dab, or `None` when there is nothing to rasterise.
///
/// ⚠ Returns `None` for each of the four early-outs `rasterize_in` had — no strokes, a zero
/// frame, an empty clip, no dabs surviving the clip, an empty box — because in every one of them
/// the answer is the same: a map with no box and only `outside`. The caller builds that.
pub fn plan_dabs(
    brush: &Brush,
    geometry: &Geometry,
    frame_w: usize,
    frame_h: usize,
    clip: Option<(usize, usize, usize, usize)>,
    base: Option<&dyn BasePixels>,
) -> Option<DabPlan> {
    // ... the moved body, returning Some(DabPlan { .. }) at the end ...
}
```

`rasterize_in` becomes:

```rust
pub fn rasterize_in(
    brush: &Brush,
    geometry: &Geometry,
    frame_w: usize,
    frame_h: usize,
    clip: Option<(usize, usize, usize, usize)>,
    base: Option<&dyn BasePixels>,
) -> CoverageMap {
    let outside = if brush.invert { 255 } else { 0 };
    let Some(plan) = plan_dabs(brush, geometry, frame_w, frame_h, clip, base) else {
        return CoverageMap {
            outside,
            ..Default::default()
        };
    };
    rasterize_planned(&plan, brush.invert, base)
}

/// The CPU kernel over an already-planned dab list.
///
/// ⚠ Split out so `plan_dabs` has exactly one caller shape and the GPU rasteriser can consume the
/// same plan. The arithmetic below is **unmoved** — which is what the byte-identical goldens and
/// the unchanged test count establish.
fn rasterize_planned(plan: &DabPlan, invert: bool, base: Option<&dyn BasePixels>) -> CoverageMap {
    // ... the rest of the old body, reading plan.x0/y0/w/h/strokes/gating_rows ...
}
```

⚠ **Move the code; do not retype it.** Every `⚠` comment in the moved region travels with it —
Step 5 checks that.

⚠ The `RUSTROOM_DEBUG_MASK` readout stays in `rasterize_planned`, because its `lab` and `dabs_ms`
timers bracket work that lives there. `strokes`/`dabs` counts now read from `plan`.

- [ ] **Step 4: Verify nothing moved**

```bash
cargo test -p pipeline > /tmp/k1-after.log 2>&1; echo "EXIT=$?"
grep "test result: ok" /tmp/k1-after.log | sed -E 's/.*ok\. ([0-9]+) passed.*/\1/' | awk '{s+=$1} END {print "tests:", s}'
grep -c "^test golden_" /tmp/k1-after.log
```

Expected: `EXIT=0`, the **same** test count as Step 1, and **63** goldens.

⚠⚠ `the_rasterizer_is_bit_identical_to_its_reference` is the test that matters here: it compares
against a **frozen verbatim copy** of the old `rasterize_in` living in the test module, which this
refactor does not touch. If the extraction changed one operation's order, that test fails.

- [ ] **Step 5: Confirm no `⚠` finding was lost in the move**

```bash
git stash -q
grep -h "⚠" crates/pipeline/src/brush/raster.rs | sort > /tmp/k1-warn-before.txt
git stash pop -q
grep -h "⚠" crates/pipeline/src/brush/raster.rs | sort > /tmp/k1-warn-after.txt
comm -23 /tmp/k1-warn-before.txt /tmp/k1-warn-after.txt
```

Expected: **empty**. M8.3i nearly lost six findings to exactly this kind of rewrite; this is W0's
rule applied before committing rather than after.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings > /tmp/k1-clippy.log 2>&1; echo "CLIPPY=$?"
git add crates/pipeline/src/brush/raster.rs
git commit -m "refactor: one dab list, planned once and consumed twice"
```

⚠ Read `CLIPPY=` before committing. This file records at least eighteen occasions on which clippy
caught what a fully green suite did not.

---

## Task 2: The shader and one non-gating stroke

**Files:**
- Create: `crates/gpu-pipeline/src/brush_wgsl.rs`
- Create: `crates/gpu-pipeline/src/brush_raster.rs`
- Modify: `crates/gpu-pipeline/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/gpu-pipeline/src/brush_raster.rs` with only its test module for now:

```rust
//! The brush coverage rasteriser on the GPU.

#[cfg(test)]
mod tests {
    use super::*;
    use edit_model::{Brush, BrushStroke, Geometry};
    use std::sync::Arc;

    fn ctx() -> Option<Arc<crate::GpuContext>> {
        match crate::shared() {
            Some(c) => Some(c.clone()),
            None => {
                eprintln!("SKIPPED: no GPU adapter on this machine");
                None
            }
        }
    }

    /// One plain stroke, no gate, no erase — the smallest case that exercises the tile scan, the
    /// shared list, `dab_alpha` and the composite.
    fn one_stroke() -> Brush {
        Brush {
            strokes: vec![BrushStroke {
                points: (0..20).map(|i| [0.2 + i as f32 * 0.02, 0.5]).collect(),
                radius: 0.03,
                hardness: 40.0,
                flow: 100.0,
                auto_mask: false,
                tolerance: 25.0,
                ..Default::default()
            }],
            invert: false,
        }
    }

    /// ⚠⚠ **The whole-map comparison against the CPU rasteriser, in 8-bit levels.** Coverage *is*
    /// a `u8`, so "within 1 level" is a statement about the stored value rather than about a
    /// rendering of it — which is why this can be far stricter than a pixel-pipeline equivalence
    /// test. M8.3i's frozen-copy bit-identity standard cannot hold here: the gate needs `cbrt` and
    /// a matrix on the GPU, and M8.3a established that GPU and CPU float differ in the last bits.
    fn assert_agrees(gpu: &pipeline::brush::CoverageMap, cpu: &pipeline::brush::CoverageMap) {
        assert_eq!((gpu.x0, gpu.y0, gpu.w, gpu.h), (cpu.x0, cpu.y0, cpu.w, cpu.h), "box");
        assert_eq!(gpu.outside, cpu.outside, "outside");
        let mut worst = 0i32;
        let mut differing = 0usize;
        let mut at = (0usize, 0usize);
        for y in 0..cpu.h {
            for x in 0..cpu.w {
                let i = y * cpu.w + x;
                let d = (gpu.data[i] as i32 - cpu.data[i] as i32).abs();
                if d > 0 {
                    differing += 1;
                }
                if d > worst {
                    worst = d;
                    at = (x, y);
                }
            }
        }
        assert!(
            worst <= 1,
            "worst {worst} levels at {at:?}, {differing} of {} samples differ",
            cpu.w * cpu.h
        );
    }

    #[test]
    fn one_plain_stroke_agrees_with_the_cpu_rasterizer() {
        let Some(ctx) = ctx() else { return };
        let b = one_stroke();
        let g = Geometry::default();
        let cpu = pipeline::brush::rasterize(&b, &g, 400, 300);
        let plan = pipeline::brush::plan_dabs(&b, &g, 400, 300, None, None)
            .expect("fixture check: the brush must produce dabs");
        let gpu = GpuBrushRaster::new(&ctx)
            .rasterize(&plan, b.invert, None)
            .expect("the GPU raster must succeed on an available adapter");
        assert!(
            cpu.data.iter().any(|&v| v > 0),
            "fixture check: the CPU map must be non-empty"
        );
        assert_agrees(&gpu, &cpu);
    }
}
```

⚠ `brush` must be re-exported from `pipeline`. `crates/pipeline/src/lib.rs` already has
`pub mod brush;` at line 7, so `pipeline::brush::{rasterize, plan_dabs, CoverageMap}` resolve once
Task 1 has made `plan_dabs` public.

- [ ] **Step 2: Declare the module and run the test to verify it fails**

Add to `crates/gpu-pipeline/src/lib.rs`, beside the other modules:

```rust
mod brush_raster;
mod brush_wgsl;
pub use brush_raster::GpuBrushRaster;
```

```bash
cargo test -p gpu-pipeline --release -- one_plain_stroke_agrees 2>&1 | tail -20
```

Expected: **compile error**, `cannot find ... GpuBrushRaster` / `brush_wgsl`. ⚠ Confirm there is
**no `test result:` line** — that is what distinguishes "did not compile" from "passed".

- [ ] **Step 3: Write the shader**

Create `crates/gpu-pipeline/src/brush_wgsl.rs`:

```rust
//! The brush rasteriser's compute shader.
//!
//! ⚠⚠ **One dispatch, one workgroup per 16×16 tile.** The workgroup's 256 threads cooperatively
//! scan the dab list once and append the dabs touching this tile to a `var<workgroup>` array; then
//! each thread rasterises its own pixel from that shared list. The alternative — a pixel testing
//! every dab — is 3.05 Mpx x 3,645 = 11.1 billion tests on the real brush, against 43.4 million
//! this way.
//!
//! ⚠ `MAX_TILE_DABS` is 4095 `u32` plus the counter = 16384 bytes, which is exactly
//! `Limits::default().max_compute_workgroup_storage_size`. `GpuContext` requests
//! `DeviceDescriptor::default()`, so that default is what it gets — verified in `wgpu-types`
//! rather than assumed.

/// Dabs a single tile's shared list can hold before it falls back to re-scanning.
pub const MAX_TILE_DABS: u32 = 4095;
/// Tile edge in pixels. 16x16 = 256 threads, one per pixel.
pub const TILE: u32 = 16;

pub fn source() -> String {
    format!(
        "{}\n{}",
        crate::oklab_wgsl::OKLAB_WGSL,
        BODY.replace("{MAX_TILE_DABS}", &MAX_TILE_DABS.to_string())
            .replace("{TILE}", &TILE.to_string())
    )
}

const BODY: &str = r#"
struct Dab {
    // x, y, r, hard
    pos: vec4<f32>,
    // Oklab reference (xyz) and the gate ceiling (w). ceiling < 0 means "no gate".
    gate: vec4<f32>,
    stroke: u32,
    _pad: vec3<u32>,
};

struct StrokeInfo {
    flow: f32,
    // 1 = erase (Subtract), 0 = paint (Add).
    erase: u32,
    _pad: vec2<u32>,
};

struct Params {
    x0: u32, y0: u32, w: u32, h: u32,
    img_w: u32, img_h: u32,
    dab_count: u32,
    stroke_count: u32,
    invert: u32,
    has_base: u32,
    _pad: vec2<u32>,
};

@group(0) @binding(0) var<storage, read> dabs: array<Dab>;
@group(0) @binding(1) var<storage, read> strokes: array<StrokeInfo>;
@group(0) @binding(2) var<storage, read> base: array<f32>;
@group(0) @binding(3) var<storage, read_write> cov: array<u32>;
@group(0) @binding(4) var<uniform> p: Params;

var<workgroup> tile_dabs: array<u32, {MAX_TILE_DABS}>;
var<workgroup> tile_count: atomic<u32>;
var<workgroup> overflowed: u32;

// ⚠ Transcribed from `pipeline::brush::raster::dab_alpha`. The `hard >= 1 - 1e-4` branch is a fast
// path only — the `q >= 1` return above already excludes the `0/0` case.
fn dab_alpha(d: f32, r: f32, hard: f32) -> f32 {
    if (r <= 0.0) { return 0.0; }
    let q = d / r;
    if (q >= 1.0) { return 0.0; }
    if (hard >= 1.0 - 1e-4) { return 1.0; }
    let t = clamp((1.0 - q) / (1.0 - hard), 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

// ⚠⚠ Transcribed from `auto_mask_gate`, and it uses ALL THREE Oklab components — the lightness
// term is load-bearing, not incidental. An earlier CPU design had to drop it because it compared
// against a reference sampled from the displayed frame; both ends come from one buffer now.
fn auto_mask_gate(px: vec3<f32>, reference: vec3<f32>, ceiling: f32) -> f32 {
    let d = length(px - reference);
    let lo = ceiling * 0.5;
    if (d <= lo) { return 1.0; }
    if (d >= ceiling) { return 0.0; }
    let q = (d - lo) / (ceiling - lo);
    return 1.0 - q * q * (3.0 - 2.0 * q);
}

fn base_oklab(fx: u32, fy: u32) -> vec3<f32> {
    let i = (fy * p.img_w + fx) * 3u;
    return prophoto_to_oklab(vec3<f32>(base[i], base[i + 1u], base[i + 2u]));
}

fn touches_tile(d: Dab, tx0: f32, ty0: f32, tx1: f32, ty1: f32) -> bool {
    let r = d.pos.z;
    return !(d.pos.x + r < tx0 || d.pos.x - r >= tx1 || d.pos.y + r < ty0 || d.pos.y - r >= ty1);
}

@compute @workgroup_size({TILE}, {TILE})
fn main(
    @builtin(workgroup_id) wg: vec3<u32>,
    @builtin(local_invocation_index) li: u32,
) {
    let tiles_x = (p.w + {TILE}u - 1u) / {TILE}u;
    let tile_x = wg.x % tiles_x;
    let tile_y = wg.x / tiles_x;

    // The tile in FRAME coordinates, which is the space every dab is in.
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

    // ⚠ Read once per pixel, never per dab — the CPU builds a whole `base_lab` plane for the same
    // reason. Only touched when something actually gates.
    var lab = vec3<f32>(0.0);
    var lab_ready = false;

    let n = min(atomicLoad(&tile_count), {MAX_TILE_DABS}u);
    let count = select(n, p.dab_count, overflowed == 1u);

    var cover = 0.0;
    var cur_stroke = 0xffffffffu;
    var stroke_max = 0.0;

    var k = 0u;
    loop {
        if (k >= count) { break; }
        // ⚠ Ascending index IS stroke order, because dabs are stored stroke-major. That is what
        // lets the shared list stay a plain sorted index list with no stroke boundaries in it.
        let di = select(tile_dabs[k], k, overflowed == 1u);
        let d = dabs[di];

        if (d.stroke != cur_stroke) {
            // ⚠⚠ Composite the stroke that just ended, THEN start the new one. Two levels: `max`
            // within a stroke, composite across strokes. `min`/`max` between strokes agrees at 0
            // and 1 and differs at every partial coverage.
            if (cur_stroke != 0xffffffffu) {
                let a = stroke_max * strokes[cur_stroke].flow;
                if (a > 0.0) {
                    if (strokes[cur_stroke].erase == 1u) {
                        cover = cover * (1.0 - a);
                    } else {
                        cover = cover + (1.0 - cover) * a;
                    }
                }
            }
            cur_stroke = d.stroke;
            stroke_max = 0.0;
        }

        let dist = length(vec2<f32>(px - d.pos.x, py - d.pos.y));
        var a = dab_alpha(dist, d.pos.z, d.pos.w);
        // ⚠⚠ THE SHORT-CIRCUIT BEFORE THE GATE IS LOAD-BEARING. The gate can only ever REDUCE an
        // alpha, so a dab already losing the running max cannot change the pixel however its gate
        // resolves — and dabs overlap heavily, so most lose. Gating first is correct and
        // dramatically slower, and every equivalence test would still pass.
        if (a > stroke_max) {
            if (d.gate.w >= 0.0 && p.has_base == 1u) {
                if (!lab_ready) {
                    lab = base_oklab(fx, fy);
                    lab_ready = true;
                }
                a = a * auto_mask_gate(lab, d.gate.xyz, d.gate.w);
            }
            stroke_max = max(stroke_max, a);
        }
        k = k + 1u;
    }
    // The last stroke.
    if (cur_stroke != 0xffffffffu) {
        let a = stroke_max * strokes[cur_stroke].flow;
        if (a > 0.0) {
            if (strokes[cur_stroke].erase == 1u) {
                cover = cover * (1.0 - a);
            } else {
                cover = cover + (1.0 - cover) * a;
            }
        }
    }

    if (p.invert == 1u) { cover = 1.0 - cover; }
    // ⚠⚠ NaN reaches 0 explicitly: WGSL has neither Rust's saturating float-to-int cast nor its
    // NaN-propagating clamp. ⚠ And `floor(x + 0.5)` rather than `round`, because WGSL's `round`
    // breaks ties to EVEN while Rust's breaks them away from zero — M8.3g's off-by-one-level bug.
    var q = 0.0;
    if (cover == cover) { q = floor(clamp(cover, 0.0, 1.0) * 255.0 + 0.5); }
    let out_i = by * p.w + bx;
    // ⚠ One `u32` per sample rather than a packed byte array: `cov` is downloaded and narrowed on
    // the CPU. A packed write needs read-modify-write across threads sharing a word, which is a
    // race, and the map is 6.5 MB at u8 against 26 MB at u32 — the download is ~1.9 ms either way
    // at this size, and correctness beats it.
    cov[out_i] = u32(q);
}
"#;
```

⚠⚠ **The `u32`-per-sample output is a deliberate cost, recorded so nobody "optimises" it into a
race.** Packing four coverage bytes into one word means four threads read-modify-writing the same
address with no synchronisation. Plan 2 may revisit it with a second narrowing dispatch.

- [ ] **Step 4: Write `GpuBrushRaster`**

Prepend to `crates/gpu-pipeline/src/brush_raster.rs` (above its `mod tests`):

```rust
//! The brush coverage rasteriser on the GPU.
//!
//! ⚠⚠ **Not a `GpuStage`, and that was checked rather than assumed.** That trait's `encode` takes
//! a `GpuImage` and produces an image in place, and its scratch is image-sized single-channel f32
//! planes. This needs a variable-length dab buffer and produces a `u8` coverage map — so forcing
//! it into `GpuStage` would mean sizing scratch by the image for data that is not image-shaped.
//! The same reasoning `GpuStage` itself rests on against `PipelineStage`.

use crate::{brush_wgsl, GpuContext};
use pipeline::brush::{BasePixels, CoverageMap, DabPlan};
use std::sync::Arc;

/// One dab as the shader sees it. 48 bytes, `repr(C)`.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GpuDab {
    pos: [f32; 4],
    gate: [f32; 4],
    stroke: u32,
    _pad: [u32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GpuStroke {
    flow: f32,
    erase: u32,
    _pad: [u32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    x0: u32,
    y0: u32,
    w: u32,
    h: u32,
    img_w: u32,
    img_h: u32,
    dab_count: u32,
    stroke_count: u32,
    invert: u32,
    has_base: u32,
    _pad: [u32; 2],
}

pub struct GpuBrushRaster {
    ctx: Arc<GpuContext>,
}

impl GpuBrushRaster {
    pub fn new(ctx: &Arc<GpuContext>) -> Self {
        Self { ctx: ctx.clone() }
    }
}
```

⚠ The `rasterize` method's body — flattening the plan into `GpuDab`/`GpuStroke`, uploading the
base image when anything gates, the bind group, the dispatch, the readback, and narrowing `u32`
back to `u8` — is written in this step following `GpuChain::run_with`'s existing pattern
(`chain.rs:171–268`): a `MAP_WRITE` staging buffer for the upload, `poll(Wait)`, then `map_async`
for the readback. **Read those lines before writing it** rather than inventing a second upload
idiom; `ctx.cache.take_staging`/`put_staging` is the pool it must use.

The signature is:

```rust
    /// Rasterise a planned dab list. `None` when the adapter cannot allocate.
    ///
    /// ⚠ `base` is required only when something gates; `DabPlan::gating_rows` says whether it
    /// does, and a plan that gates with no base rasterises **positionally** — the documented
    /// degradation `rasterize_in` already has, not an error.
    pub fn rasterize(
        &self,
        plan: &DabPlan,
        invert: bool,
        base: Option<(&[f32], usize, usize)>,
    ) -> Option<CoverageMap>
```

⚠ `base` is `(&[f32], w, h)` rather than `&dyn BasePixels`: the GPU needs the **whole plane** as
contiguous bytes to upload, and `BasePixels` is a per-pixel accessor that cannot be uploaded. Plan 2
hands it `image.data()` directly, which is exactly the resident buffer M8.3j already uploads.

- [ ] **Step 5: Run the test to verify it passes**

```bash
cargo test -p gpu-pipeline --release -- one_plain_stroke_agrees 2>&1 | tail -20
```

Expected: `test result: ok. 1 passed`.

⚠ If it fails on the **box** assertion, `plan_dabs` and the map disagree about clamping — fix
`plan_dabs`, not the test. If it fails on levels, read the reported `worst` and coordinates before
touching anything: a uniform off-by-one is the `round`-versus-`floor(x+0.5)` trap, an edge-only
difference is the tile/frame coordinate origin, and a whole-map difference is `dab_alpha`.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings > /tmp/k2-clippy.log 2>&1; echo "CLIPPY=$?"
git add crates/gpu-pipeline/
git commit -m "feat: a brush's dabs rasterise on the GPU, one tile per workgroup"
```

---

## Task 3: Stroke order, erase, flow and invert

**Files:**
- Test: `crates/gpu-pipeline/src/brush_raster.rs`

- [ ] **Step 1: Write the failing tests**

Add inside `mod tests`:

```rust
    /// ⚠⚠ **The two-level combine, and the only fixture that can see it flattened.** Two strokes
    /// overlapping at partial coverage: `max` between them agrees with the composite at 0 and 1 and
    /// differs everywhere in between, so a kernel that took a single flat `max` over all dabs
    /// passes every single-stroke test and fails this one.
    #[test]
    fn two_overlapping_strokes_composite_rather_than_max() {
        let Some(ctx) = ctx() else { return };
        let soft = |y: f32| BrushStroke {
            points: (0..12).map(|i| [0.25 + i as f32 * 0.03, y]).collect(),
            radius: 0.05,
            hardness: 0.0,
            flow: 45.0,
            auto_mask: false,
            tolerance: 25.0,
            ..Default::default()
        };
        let b = Brush {
            strokes: vec![soft(0.48), soft(0.52)],
            invert: false,
        };
        let g = Geometry::default();
        let cpu = pipeline::brush::rasterize(&b, &g, 400, 300);
        // ⚠ The fixture check that makes the test mean something: there must be samples strictly
        // between 0 and 255, or `max` and the composite would agree everywhere.
        assert!(
            cpu.data.iter().any(|&v| v > 10 && v < 245),
            "fixture check: the overlap must be at PARTIAL coverage"
        );
        let plan = pipeline::brush::plan_dabs(&b, &g, 400, 300, None, None).unwrap();
        let gpu = GpuBrushRaster::new(&ctx).rasterize(&plan, b.invert, None).unwrap();
        assert_agrees(&gpu, &cpu);
    }

    /// ⚠ Erase is `c * (1 - a)`, the `MaskOp::Subtract` rule — not `min`. A paint stroke with an
    /// erase stroke over it at partial flow is where the two diverge.
    #[test]
    fn an_erase_stroke_over_a_paint_stroke_agrees() {
        let Some(ctx) = ctx() else { return };
        let mut paint = BrushStroke {
            points: (0..14).map(|i| [0.2 + i as f32 * 0.04, 0.5]).collect(),
            radius: 0.06,
            hardness: 30.0,
            flow: 100.0,
            auto_mask: false,
            tolerance: 25.0,
            ..Default::default()
        };
        paint.erase = false;
        let mut rub = paint.clone();
        rub.erase = true;
        rub.flow = 60.0;
        rub.radius = 0.03;
        let b = Brush {
            strokes: vec![paint, rub],
            invert: false,
        };
        let g = Geometry::default();
        let cpu = pipeline::brush::rasterize(&b, &g, 400, 300);
        assert!(
            cpu.data.iter().any(|&v| v > 10 && v < 245),
            "fixture check: the erase must leave PARTIAL coverage"
        );
        let plan = pipeline::brush::plan_dabs(&b, &g, 400, 300, None, None).unwrap();
        let gpu = GpuBrushRaster::new(&ctx).rasterize(&plan, b.invert, None).unwrap();
        assert_agrees(&gpu, &cpu);
    }

    /// ⚠ Stroke ORDER is observable once erase is in play: erase-then-paint restores where
    /// paint-then-erase removes. A kernel that walked its tile list unsorted would pass the two
    /// tests above and fail this one.
    #[test]
    fn swapping_two_strokes_changes_the_answer_and_both_agree() {
        let Some(ctx) = ctx() else { return };
        let mut paint = BrushStroke {
            points: (0..14).map(|i| [0.2 + i as f32 * 0.04, 0.5]).collect(),
            radius: 0.06,
            hardness: 30.0,
            flow: 80.0,
            auto_mask: false,
            tolerance: 25.0,
            ..Default::default()
        };
        paint.erase = false;
        let mut rub = paint.clone();
        rub.erase = true;
        rub.flow = 70.0;
        let g = Geometry::default();
        let forward = Brush {
            strokes: vec![paint.clone(), rub.clone()],
            invert: false,
        };
        let reverse = Brush {
            strokes: vec![rub, paint],
            invert: false,
        };
        let a = pipeline::brush::rasterize(&forward, &g, 400, 300);
        let z = pipeline::brush::rasterize(&reverse, &g, 400, 300);
        assert_ne!(
            a.data, z.data,
            "fixture check: the order must be observable, or this test proves nothing"
        );
        let r = GpuBrushRaster::new(&ctx);
        for (b, cpu) in [(forward, a), (reverse, z)] {
            let plan = pipeline::brush::plan_dabs(&b, &g, 400, 300, None, None).unwrap();
            assert_agrees(&r.rasterize(&plan, b.invert, None).unwrap(), &cpu);
        }
    }

    /// ⚠ An inverted brush with **no strokes** covers EVERYTHING — `outside` is 255, not 0, and
    /// `plan_dabs` returns `None`, so this is a property of the caller rather than of the kernel.
    /// It is asserted here because it was a real bug found in use on the CPU side.
    #[test]
    fn an_inverted_brush_with_no_strokes_covers_everything() {
        let b = Brush {
            strokes: Vec::new(),
            invert: true,
        };
        let g = Geometry::default();
        let cpu = pipeline::brush::rasterize(&b, &g, 400, 300);
        assert_eq!(cpu.outside, 255);
        assert!(!cpu.is_inert());
        assert!(
            pipeline::brush::plan_dabs(&b, &g, 400, 300, None, None).is_none(),
            "no strokes must plan no dabs, so the GPU path is never entered"
        );
    }

    /// ⚠ And an inverted brush that DOES paint, so the kernel's own `1 - cover` is exercised.
    #[test]
    fn an_inverted_painted_brush_agrees() {
        let Some(ctx) = ctx() else { return };
        let mut b = one_stroke();
        b.invert = true;
        let g = Geometry::default();
        let cpu = pipeline::brush::rasterize(&b, &g, 400, 300);
        let plan = pipeline::brush::plan_dabs(&b, &g, 400, 300, None, None).unwrap();
        let gpu = GpuBrushRaster::new(&ctx).rasterize(&plan, b.invert, None).unwrap();
        assert_agrees(&gpu, &cpu);
    }
```

- [ ] **Step 2: Run them**

```bash
cargo test -p gpu-pipeline --release -- _strokes_composite an_erase_stroke swapping_two_strokes an_inverted 2>&1 | tail -25
```

Expected: all pass if Task 2's shader is right. ⚠ **If `two_overlapping_strokes_composite_rather_than_max`
fails, the stroke-change composite in the kernel is wrong — that is the bug this test exists for,
and the fix is the kernel, never the tolerance.**

- [ ] **Step 3: Sabotage-verify each guard**

For each of the three below, apply it, confirm the named test **fails** with a `test result:` line
present, then restore from the backup and `touch` the file.

```bash
cp crates/gpu-pipeline/src/brush_wgsl.rs /tmp/k3-wgsl.bak
```

1. **Flatten the two levels** — replace the stroke-change composite with `cover = max(cover, a);`.
   Expect `two_overlapping_strokes_composite_rather_than_max` to fail.
2. **Ignore stroke order** — replace `cover * (1.0 - a)` in *both* composite sites with
   `cover + (1.0 - cover) * a`, so erase becomes paint. Expect
   `an_erase_stroke_over_a_paint_stroke_agrees` **and**
   `swapping_two_strokes_changes_the_answer_and_both_agree` to fail.
3. **`round` for `floor(x + 0.5)`** — expect a one-level failure somewhere. ⚠ **If this one
   passes, that is a finding about the fixture, not a formality**: no sample landed on an exact
   tie. Record it as inert rather than counting it, exactly as M8.3i recorded three inert
   sabotages.

```bash
cp /tmp/k3-wgsl.bak crates/gpu-pipeline/src/brush_wgsl.rs && touch crates/gpu-pipeline/src/brush_wgsl.rs
```

- [ ] **Step 4: Commit**

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings > /tmp/k3-clippy.log 2>&1; echo "CLIPPY=$?"
git add crates/gpu-pipeline/
git commit -m "test: stroke order, erase, flow and invert agree with the CPU rasterizer"
```

---

## Task 4: The auto-mask gate

**Files:**
- Test: `crates/gpu-pipeline/src/brush_raster.rs`

- [ ] **Step 1: Write the failing test**

Add inside `mod tests`:

```rust
    /// A base whose halves differ in Oklab, so the gate genuinely discriminates rather than
    /// passing everything. ⚠ The same shape `raster.rs`'s own `HalfBase` fixture uses.
    fn half_base(w: usize, h: usize) -> (Vec<f32>, usize, usize) {
        let mut v = vec![0.0f32; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let p = if x < w / 2 {
                    [0.05, 0.20, 0.05]
                } else {
                    [0.60, 0.15, 0.50]
                };
                let i = (y * w + x) * 3;
                v[i..i + 3].copy_from_slice(&p);
            }
        }
        (v, w, h)
    }

    struct SliceBase<'a>(&'a [f32], usize);
    impl pipeline::brush::BasePixels for SliceBase<'_> {
        fn at(&self, x: usize, y: usize) -> [f32; 3] {
            let i = (y * self.1 + x) * 3;
            [self.0[i], self.0[i + 1], self.0[i + 2]]
        }
    }

    /// ⚠⚠ **The gate, and the fixture is what makes it a test.** A stroke crossing the colour
    /// boundary must be cut off at it — so the CPU map is first asserted to contain both painted
    /// and unpainted samples *within the stroke's own box*, or a gate that passed everything would
    /// agree trivially.
    #[test]
    fn an_auto_masked_stroke_agrees_with_the_cpu_gate() {
        let Some(ctx) = ctx() else { return };
        let (plane, bw, bh) = half_base(400, 300);
        let b = Brush {
            strokes: vec![BrushStroke {
                // Starts in the dark half and crosses into the light one.
                points: (0..24).map(|i| [0.2 + i as f32 * 0.02, 0.5]).collect(),
                radius: 0.03,
                hardness: 60.0,
                flow: 100.0,
                auto_mask: true,
                tolerance: 25.0,
                ..Default::default()
            }],
            invert: false,
        };
        let g = Geometry::default();
        let base = SliceBase(&plane, bw);
        let cpu = pipeline::brush::rasterize_in(&b, &g, bw, bh, None, Some(&base));
        let painted = cpu.data.iter().filter(|&&v| v > 200).count();
        let blocked = cpu.data.iter().filter(|&&v| v < 20).count();
        assert!(
            painted > 100 && blocked > 100,
            "fixture check: the gate must both paint and block ({painted} painted, {blocked} blocked)"
        );
        let plan = pipeline::brush::plan_dabs(&b, &g, bw, bh, None, Some(&base)).unwrap();
        assert!(
            plan.gating_rows.is_some(),
            "fixture check: the plan must report a gating span"
        );
        let gpu = GpuBrushRaster::new(&ctx)
            .rasterize(&plan, b.invert, Some((&plane, bw, bh)))
            .unwrap();
        assert_agrees(&gpu, &cpu);
    }

    /// ⚠ A gating plan with **no base** paints positionally — the documented degradation
    /// `rasterize_in` already has (the CLI, and the tint before a frame lands), not an error. Both
    /// sides must degrade the same way.
    #[test]
    fn a_gating_plan_with_no_base_paints_positionally_on_both_sides() {
        let Some(ctx) = ctx() else { return };
        let mut b = one_stroke();
        b.strokes[0].auto_mask = true;
        let g = Geometry::default();
        let cpu = pipeline::brush::rasterize_in(&b, &g, 400, 300, None, None);
        let plan = pipeline::brush::plan_dabs(&b, &g, 400, 300, None, None).unwrap();
        let gpu = GpuBrushRaster::new(&ctx).rasterize(&plan, b.invert, None).unwrap();
        assert_agrees(&gpu, &cpu);
    }
```

⚠ `assert_agrees`'s `worst <= 1` bound is the one place the gate's GPU/CPU float difference can
show. **If it exceeds 1, do not widen it without reading the reported figure**: the coverage is a
`u8`, so a 2-level disagreement means the Oklab round trip differs by enough to cross two
quantisation steps, which is a finding about `prophoto_to_oklab` in WGSL rather than a tolerance to
adjust. M8.3d's `the_wgsl_prelude_agrees_with_color_oklab` is the test that would localise it.

- [ ] **Step 2: Run them**

```bash
cargo test -p gpu-pipeline --release -- an_auto_masked_stroke a_gating_plan_with_no_base 2>&1 | tail -20
```

Expected: both pass.

- [ ] **Step 3: Sabotage the gate**

```bash
cp crates/gpu-pipeline/src/brush_wgsl.rs /tmp/k4-wgsl.bak
```

1. **Drop the lightness term** — `length(px.yz - reference.yz)`. Expect
   `an_auto_masked_stroke_agrees_with_the_cpu_gate` to fail. ⚠ This is the exact term an earlier
   CPU design had to drop, and the reason it was restored; a fixture whose halves differed only in
   chroma could not see it, which is why `half_base` differs in **both**.
2. **Gate before the short-circuit** — move the `auto_mask_gate` multiply above `if (a > stroke_max)`.
   ⚠⚠ **Expect this to PASS.** It is *correct* and merely slower, and the spec says so: every
   equivalence test passes against it. **Record it as inert** and rely on Task 5's counter, which is
   the only thing that can see it.

```bash
cp /tmp/k4-wgsl.bak crates/gpu-pipeline/src/brush_wgsl.rs && touch crates/gpu-pipeline/src/brush_wgsl.rs
```

- [ ] **Step 4: Commit**

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings > /tmp/k4-clippy.log 2>&1; echo "CLIPPY=$?"
git add crates/gpu-pipeline/
git commit -m "test: the auto-mask gate agrees, lightness term included"
```

---

## Task 5: The overflow path, and the counter that sees the short-circuit

**Files:**
- Modify: `crates/gpu-pipeline/src/brush_wgsl.rs`, `crates/gpu-pipeline/src/brush_raster.rs`

- [ ] **Step 1: Make the tile capacity injectable, and write the overflow test**

`brush_wgsl::source()` already substitutes `{MAX_TILE_DABS}`. Add
`pub fn source_with(max_tile_dabs: u32) -> String` beside it and have `source()` delegate with
`MAX_TILE_DABS`; give `GpuBrushRaster` a `#[cfg(test)] fn with_capacity(ctx, cap)`.

⚠ **The capacity must be injectable, because no real brush comes near 4,095 dabs in one 16×16
tile** — the real one averages 22. An overflow path that cannot be reached is an untested path, and
this project records that *"an unreachable guard is an untested one"* for `visualize`'s tool gate
and for the lens-correction identity fill.

```rust
    /// ⚠⚠ **The overflow path must RE-SCAN, never clip.** A tile whose shared list fills falls
    /// back to walking the whole dab list per pixel — correct, slower, confined to that tile. A
    /// kernel that silently used the first 4,095 would render a plausible-but-wrong mask, which is
    /// the failure mode this whole file is written against.
    #[test]
    fn a_tile_that_overflows_its_shared_list_still_agrees() {
        let Some(ctx) = ctx() else { return };
        let b = one_stroke();
        let g = Geometry::default();
        let cpu = pipeline::brush::rasterize(&b, &g, 400, 300);
        let plan = pipeline::brush::plan_dabs(&b, &g, 400, 300, None, None).unwrap();
        // ⚠ A capacity of 1 forces every tile the stroke touches down the overflow path, which is
        // the only way to reach it at all.
        let gpu = GpuBrushRaster::with_capacity(&ctx, 1)
            .rasterize(&plan, b.invert, None)
            .unwrap();
        assert_agrees(&gpu, &cpu);
    }
```

- [ ] **Step 2: Run it, then sabotage the overflow branch**

```bash
cargo test -p gpu-pipeline --release -- a_tile_that_overflows 2>&1 | tail -15
```

Expected: pass. Then change `let count = select(n, p.dab_count, overflowed == 1u);` to
`let count = n;` — the silent-clip bug — and expect the test to **fail**. Restore.

- [ ] **Step 3: Add the dab-test counter**

⚠⚠ **This is the only guard that can see the short-circuit dropped**, which Task 4 Step 3 confirmed
is invisible to every equivalence test. Add a second storage binding, written only under test:

In the shader, guarded by a substituted `{COUNT_TESTS}` flag that `source_with` sets to `0` in
production, increment an `atomic<u32>` each time the gate is **evaluated**. Then:

```rust
    /// ⚠⚠ **A COUNTER, not a flag, and it counts GATE EVALUATIONS rather than describing state.**
    /// An observable describing the current state cannot see its own absence — the
    /// `PARALLEL_STROKES` lesson, and the reason `Ran { bytes }` exists. Dropping the
    /// short-circuit is *correct* and merely slower, so nothing in the output can distinguish the
    /// two and only an event count can.
    ///
    /// ⚠ The bound is a ratio rather than an absolute: gate evaluations must be a small fraction
    /// of dab-pixel tests, because dabs are laid a quarter-radius apart and most lose the running
    /// max. Measured on this fixture it is far below a tenth; asserting a tenth leaves room for
    /// the fixture to change without the test going vacuous.
    #[test]
    fn the_short_circuit_keeps_the_gate_off_most_dabs() {
        let Some(ctx) = ctx() else { return };
        let (plane, bw, bh) = half_base(400, 300);
        let mut b = one_stroke();
        b.strokes[0].auto_mask = true;
        b.strokes[0].radius = 0.06;
        let g = Geometry::default();
        let base = SliceBase(&plane, bw);
        let plan = pipeline::brush::plan_dabs(&b, &g, bw, bh, None, Some(&base)).unwrap();
        let r = GpuBrushRaster::new(&ctx);
        let (_, gate_tests, dab_tests) = r
            .rasterize_counting(&plan, b.invert, Some((&plane, bw, bh)))
            .unwrap();
        assert!(dab_tests > 0, "fixture check: dabs must be tested at all");
        assert!(
            (gate_tests as f64) < (dab_tests as f64) * 0.1,
            "the gate ran on {gate_tests} of {dab_tests} dab tests — the short-circuit is gone"
        );
    }
```

- [ ] **Step 4: Verify the counter sees the sabotage**

Move the gate multiply above `if (a > stroke_max)` again and expect
`the_short_circuit_keeps_the_gate_off_most_dabs` to **fail** — this time it must, which is the
whole point of the counter.

```bash
cargo test -p gpu-pipeline --release -- the_short_circuit_keeps 2>&1 | grep -E "^test |test result:|the gate ran"
```

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings > /tmp/k5-clippy.log 2>&1; echo "CLIPPY=$?"
git add crates/gpu-pipeline/
git commit -m "test: the overflow path re-scans, and a counter sees the short-circuit"
```

---

## Task 6: The real 36-stroke brush, at scale

**Files:**
- Create: `crates/gpu-pipeline/examples/brush_raster_probe.rs`

- [ ] **Step 1: Write the probe**

A synthetic fixture cannot stand in for the real brush — 36 strokes, 1,387 points, 3,645 dabs, 22
of them auto-masked, over a 1510×2019 box. The probe decodes a RAW, loads its sidecar, plans the
dabs, rasterises both ways and reports the worst disagreement and both timings.

```
cargo run --release -p gpu-pipeline --example brush_raster_probe <raw>
```

⚠ It is an **example, not a test**: it needs a real RAW and a real sidecar, and a fixture-gated
test is no evidence in CI — the rule `RAW_FIXTURE` already follows. ⚠ And it prints timings, which
a test must never assert on: *a benchmark is not a test.*

- [ ] **Step 2: Run it on the real photograph**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo run --release -p gpu-pipeline --example brush_raster_probe ~/Pictures/fuji/DSCF6274.RAF
```

Expected: worst disagreement **≤ 1 level** over all 3.05 Mpx, and the GPU time well under the CPU's
62.3–70.1 ms.

⚠ Quote the result as a **band over three runs**, not a single figure — M8.3h had to retract a
headline for quoting the top of a band as the value. ⚠ Note how idle the machine is.

⚠⚠ **If the worst disagreement exceeds 1 level, stop and report rather than widening anything.**
At 3.05 Mpx a handful of samples sitting on a quantisation boundary is plausible and a systematic
difference is not, so report **how many** samples differ as well as the worst — that is what tells
the two apart.

- [ ] **Step 3: Commit**

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings > /tmp/k6-clippy.log 2>&1; echo "CLIPPY=$?"
git add crates/gpu-pipeline/examples/brush_raster_probe.rs
git commit -m "probe: the GPU rasterizer against the CPU on the real 36-stroke brush"
```

---

## Task 7: The gate

- [ ] **Step 1: Run the three gates, each exit code from its own command**

```bash
cargo fmt --all
cargo fmt --all --check > /tmp/k7-fmt.log 2>&1; echo "FMT=$?"
cargo clippy --all-targets --all-features -- -D warnings > /tmp/k7-clippy.log 2>&1; echo "CLIPPY=$?"
cargo test --workspace > /tmp/k7-test.log 2>&1; echo "TEST=$?"
```

Expected: `FMT=0`, `CLIPPY=0`, `TEST=0`.

- [ ] **Step 2: Confirm the goldens, the skips and the count**

```bash
echo "goldens:  $(grep -c '^test golden_' /tmp/k7-test.log)"
echo "GPU skips: $(grep -c 'SKIPPED: no GPU adapter' /tmp/k7-test.log)"
grep "test result: ok" /tmp/k7-test.log | sed -E 's/.*ok\. ([0-9]+) passed.*/\1/' | awk '{s+=$1} END {print "tests:", s}'
grep "test result:" /tmp/k7-test.log | grep -v " 0 failed" || echo "no failures"
```

Expected: **63** goldens, **0** skips, **no failures**, and a count of 2331 + the tests this plan
adds. ⚠ State the arithmetic — "2331 → N, which is the M tests added" — rather than "still green".

⚠⚠ **The goldens prove only that the Task 1 refactor moved nothing.** They never touch the GPU
(`grep -rln gpu tests` returns nothing), so they are blind to every line of the shader. Say that
when reporting.

- [ ] **Step 3: Confirm no `⚠` finding was lost across the whole branch**

```bash
git diff main..HEAD -- crates/ | grep '^-' | grep '⚠'
```

Expected: **empty**. Task 1 is a move, so nothing should be removed.

- [ ] **Step 4: Report, and stop**

Report: the branch, the three exit codes, the test-count arithmetic, the golden count, the probe's
band, and that **plan 1 ships nothing user-visible** — `MaskStage` is untouched and wiring is
plan 2.

⚠ **Do not merge and do not push.** The user merges (fast-forward, his call) and pushes himself.
