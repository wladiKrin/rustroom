# M8.3q — noise reduction on the GPU: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this
> plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

> **⚠ Project conventions, non-negotiable.** Commit messages carry **no `Co-Authored-By`
> trailer**. **Never push** — the user pushes to `github.com/wladiKrin/rustroom` himself. Work
> happens on the branch `m8.3q-noise-reduction-gpu`, which already exists.

**Goal:** Give `NoiseReductionStage` a GPU twin behind the existing `accelerate_prefix_stage`
seam, so an NR drag — the most expensive drag in the prefix at **216.4 ms** — runs on the GPU.

**Architecture:** One new `GpuStage` in `crates/gpu-pipeline/src/noise_reduction.rs`, built from
primitives that already exist: `oklab_wgsl::OKLAB_WGSL` (M8.3d), `box_wgsl::box_pass` (M8.3n) and
the self-guided filter shape from `local_contrast` (M8.3n). Four scratch planes, two sequenced
phases, and an `apply` pass that **recomputes** the Oklab transform rather than storing three
original planes. One new arm in `accelerate_prefix_stage`'s `match`.

**Tech Stack:** Rust, `wgpu` 29 compute shaders, WGSL.

**Spec:** `docs/superpowers/specs/2026-09-13-noise-reduction-gpu-design.md`

---

## Build and test commands

Every task in this plan is verified with these. The GPU tests need `--release`: acquiring an
adapter costs 44–75 s in a debug build against 21 ms in release (M8.3a).

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

- Fast loop: `cargo test --release -p gpu-pipeline`
- Pipeline: `cargo test -p pipeline`
- Full gate: `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo test --workspace` — **each exit code read from its own command**, never through a pipe.

---

## File structure

| File | Responsibility |
|---|---|
| `crates/pipeline/src/stages/noise_reduction.rs` | **Modify.** Make four items `pub` so they can be substituted rather than transcribed. |
| `crates/pipeline/src/lib.rs` | **Modify.** Re-export those four beside the dehaze block. |
| `crates/gpu-pipeline/src/noise_reduction.rs` | **Create.** The shader, the stage, and its tests. |
| `crates/gpu-pipeline/src/lib.rs` | **Modify.** `mod noise_reduction;` + `pub use`. |
| `crates/gpu-pipeline/src/accelerate.rs` | **Modify.** One arm in `accelerate_prefix_stage`; update the test that names `"noise_reduction"` as having no twin. |
| `crates/app/src/render_engine.rs` | **Modify.** Extend the checkpoint-sharing guard to NR. |
| `crates/app/examples/bench_preview.rs` | **Modify.** Fill in the `— \`noise_reduction\` has no GPU twin` line. |

---

## Task 1: Re-export NR's constants and helpers from `pipeline`

**Why:** A substituted constant **cannot** disagree with the CPU; a transcribed one is merely
checked for agreeing. `color`'s own doc records six copies of a Rec.709 luma triple that all went
wrong on one day. This is the rule `dehaze` and `local_contrast` already follow.

**Files:**
- Modify: `crates/pipeline/src/stages/noise_reduction.rs`
- Modify: `crates/pipeline/src/lib.rs`

- [ ] **Step 1: Make the four items public**

In `crates/pipeline/src/stages/noise_reduction.rs`, change these four declarations. The doc
comments above each stay exactly as they are — only `const`/`fn` gains `pub`:

```rust
pub const LUM_RADIUS_FULL: f32 = 6.0;
```

```rust
pub const COL_RADIUS_FULL: (f32, f32) = (8.0, 32.0);
```

```rust
pub const CHROMA_PASSES: usize = 2;
```

```rust
pub fn eps_for_detail(detail: f32) -> f32 {
```

```rust
pub fn chroma_radius_full(smoothness: f32) -> f32 {
```

⚠ That is five items, not four — `COL_RADIUS_FULL` rides along because `chroma_radius_full` reads
it and a reader checking the shader's radius against the source will want both. `scaled_radius` is
**already** public and already re-exported (dehaze needed it).

- [ ] **Step 2: Re-export them from `pipeline`**

In `crates/pipeline/src/lib.rs`, find this line:

```rust
pub use stages::noise_reduction::NoiseReductionStage;
```

Replace it with:

```rust
pub use stages::noise_reduction::NoiseReductionStage;
// ⚠⚠ Re-exported so `gpu-pipeline` SUBSTITUTES these into WGSL rather than transcribing them.
// A substituted constant cannot disagree; a transcribed one is merely checked for agreeing —
// and `color`'s own doc records six copies of a Rec.709 luma triple that all went wrong on one
// day. The same rule the `dehaze` block above follows.
// ⚠ `CHROMA_PASSES` is ALIASED on the way out: a bare name gives a reader no way to tell which
// stage's passes it counts, and `display_rendering` already exports a `CHROMA_BOOST` next to it.
pub use stages::noise_reduction::{
    chroma_radius_full, eps_for_detail, CHROMA_PASSES as NR_CHROMA_PASSES, COL_RADIUS_FULL,
    LUM_RADIUS_FULL,
};
```

- [ ] **Step 3: Verify it compiles and nothing moved**

```bash
cargo test -p pipeline 2>&1 | tail -20
```

Expected: every test passes. This task adds no behaviour, so the count must be **unchanged** —
note it, because "the count moved by exactly N" is the check the later tasks rely on.

- [ ] **Step 4: Commit**

```bash
git add crates/pipeline/src/stages/noise_reduction.rs crates/pipeline/src/lib.rs
git commit -m "refactor: noise reduction's constants are substitutable"
```

---

## Task 2: The shader and the stage

**Files:**
- Create: `crates/gpu-pipeline/src/noise_reduction.rs`
- Modify: `crates/gpu-pipeline/src/lib.rs`

⚠ **Tasks 2 and 3 land as ONE commit.** `GpuNoiseReductionStage` is `pub` in a *library*, so
`pub use` makes it public API and dead-code analysis is satisfied — but its `#[cfg(test)]`-only
constructor helpers are not. Write both tasks, then commit once. This is the trap this project's
history records at least nine times.

- [ ] **Step 1: Write the module**

Create `crates/gpu-pipeline/src/noise_reduction.rs` with exactly this content:

```rust
//! Noise reduction as compute shaders — the second **prefix** stage to move, and the cheaper
//! case of the guided filter M8.3o shipped.
//!
//! ⚠⚠ **Self-guided, where dehaze is cross-guided.** `guided_filter(&lum, &lum, ..)` passes the
//! same slice twice, which `spatial::guided_filter`'s pointer-identity shortcut collapses to
//! **two** statistic blurs — dehaze's transmission map is guided by luma and needs all six. So
//! this is `local_contrast`'s negative-Texture shape, not `dehaze`'s.
//!
//! ⚠⚠ **FOUR scratch planes, against dehaze's seven, and that is the milestone's one real design
//! choice.** The prefix always runs whole-buffer, so this is the SECOND stage ever to allocate at
//! full resolution — 99.4 MiB per plane on an X-T4 — and the first one killed the render worker.
//! The two phases are sequenced so the chroma half re-uses planes the luminance half has
//! finished with, and `apply_nr` **recomputes** `prophoto_to_oklab` rather than storing three
//! original planes. That recompute is three `cbrt` and two matrices against 398 MiB, and it is
//! bit-identical by construction: same bytes, same function, and `img` is not written until
//! `apply_nr` itself.

use crate::box_wgsl::{box_pass, use_running_sum};
use crate::{GpuContext, GpuImage, GpuStage};
use edit_model::NoiseReduction;
use pipeline::{PipelineStage, RenderScale};
use std::sync::Arc;

const WORKGROUP: u32 = 64;
const MAX_GROUPS_PER_DIM: u32 = 65535;

/// Which thread count a pass wants — the box blur's running body is per **line**, not per pixel.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Grid {
    Pixel,
    Rows,
    Cols,
}

/// The `(h, v)` grids a box blur at this radius wants.
fn box_grids(r: usize) -> (Grid, Grid) {
    if use_running_sum(r) {
        (Grid::Rows, Grid::Cols)
    } else {
        (Grid::Pixel, Grid::Pixel)
    }
}

/// Bindings, the pixel index, and the two seeding kernels.
///
/// Plane roles, and they change between the two phases **deliberately**:
/// - luminance: `p0` the guide L, `p1`/`p2` the statistics then slope/offset then the guided
///   result in `p1`, `p3` the blur tmp.
/// - chroma: `p0` the a channel, `p2` the b channel, `p3` the blur tmp. `p1` holds the guided
///   luminance across this phase and must not be touched.
///
/// ⚠⚠ **`eps` is a UNIFORM, where `box_pass`'s radius is a literal — and the asymmetry is
/// deliberate.** A literal puts the value in the shader text, which is the program cache's key:
/// `eps` moves with every step of the Detail slider, so a literal would compile a fresh program
/// on every frame of a drag. The radii move only with Smoothness and the render scale, a handful
/// of values, and `box_wgsl`'s own doc says a literal radius is what lets the compiler see a
/// constant trip count.
fn preamble() -> String {
    format!(
        r#"
struct Params {{
    w: u32,
    h: u32,
    pixels: u32,
    row_groups: u32,
    s_lum: f32,
    s_col: f32,
    eps: f32,
    pad0: f32,
}};
@group(0) @binding(0) var<storage, read_write> img: array<f32>;
@group(0) @binding(1) var<uniform> params: Params;
@group(0) @binding(2) var<storage, read_write> p0: array<f32>;
@group(0) @binding(3) var<storage, read_write> p1: array<f32>;
@group(0) @binding(4) var<storage, read_write> p2: array<f32>;
@group(0) @binding(5) var<storage, read_write> p3: array<f32>;

fn index(id: vec3<u32>) -> u32 {{
    return id.y * (params.row_groups * 64u) + id.x;
}}

// The guide: Oklab L, which the guided filter both filters and is guided by.
@compute @workgroup_size(64)
fn seed_l(@builtin(global_invocation_id) id: vec3<u32>) {{
    let i = index(id);
    if (i >= params.pixels) {{ return; }}
    let q = i * 3u;
    p0[i] = prophoto_to_oklab(img[q], img[q + 1u], img[q + 2u]).x;
}}

@compute @workgroup_size(64)
fn square(@builtin(global_invocation_id) id: vec3<u32>) {{
    let i = index(id);
    if (i >= params.pixels) {{ return; }}
    let v = p0[i];
    p1[i] = v * v;
}}

@compute @workgroup_size(64)
fn copy_l(@builtin(global_invocation_id) id: vec3<u32>) {{
    let i = index(id);
    if (i >= params.pixels) {{ return; }}
    p2[i] = p0[i];
}}

// ⚠⚠ `cov` and `var` are the SAME expression in the self-guided case, and only `var` is clamped
// — `guided_filter`'s own comment says why: unlike a variance, a covariance may legitimately be
// negative when `p` and `guide` are anti-correlated, which cannot happen when they are the same
// plane. Kept as two names because it mirrors the CPU line for line.
//
// ⚠ Reads BOTH planes before writing EITHER — it overwrites its own inputs in place.
@compute @workgroup_size(64)
fn slope_offset(@builtin(global_invocation_id) id: vec3<u32>) {{
    let i = index(id);
    if (i >= params.pixels) {{ return; }}
    let mgsq = p1[i];
    let mg = p2[i];
    let cov = mgsq - mg * mg;
    let vr = max(cov, 0.0);
    let s = cov / (vr + params.eps);
    p1[i] = s;
    p2[i] = mg - s * mg;
}}

// `mean_slope * guide + mean_offset`, landing in p1 so the chroma phase may take p0 and p2.
@compute @workgroup_size(64)
fn combine_lum(@builtin(global_invocation_id) id: vec3<u32>) {{
    let i = index(id);
    if (i >= params.pixels) {{ return; }}
    p1[i] = p1[i] * p0[i] + p2[i];
}}

// ⚠ Seeds the chroma channels into planes the luminance phase has finished with. `p1` is the
// guided luminance and is NOT touched.
@compute @workgroup_size(64)
fn seed_c(@builtin(global_invocation_id) id: vec3<u32>) {{
    let i = index(id);
    if (i >= params.pixels) {{ return; }}
    let q = i * 3u;
    let lab = prophoto_to_oklab(img[q], img[q + 1u], img[q + 2u]);
    p0[i] = lab.y;
    p2[i] = lab.z;
}}

// ⚠⚠ Recomputes the Oklab transform rather than reading three stored originals — the four-plane
// decision, and bit-identical because `img` has not been written until this pass.
//
// ⚠ Branches on each strength rather than trusting a plane to hold something harmless. Buffers
// are zero-initialised today, but `0.0 * NaN` is `NaN` and a lerp against an uninitialised plane
// is correct until a driver changes. The CPU stage branches on exactly these two conditions.
@compute @workgroup_size(64)
fn apply_nr(@builtin(global_invocation_id) id: vec3<u32>) {{
    let i = index(id);
    if (i >= params.pixels) {{ return; }}
    let q = i * 3u;
    let lab = prophoto_to_oklab(img[q], img[q + 1u], img[q + 2u]);
    var l = lab.x;
    var a = lab.y;
    var b = lab.z;
    if (params.s_lum > 0.0) {{ l = l + (p1[i] - l) * params.s_lum; }}
    if (params.s_col > 0.0) {{
        a = a + (p0[i] - a) * params.s_col;
        b = b + (p2[i] - b) * params.s_col;
    }}
    let rgb = oklab_to_prophoto(l, a, b);
    // ⚠ Strong smoothing can push a pixel out of gamut; clamped at 0, the convention every other
    // stage uses — and one `compare` is structurally blind to, since a negative sample and 0
    // quantize to the same 8-bit level.
    img[q] = max(rgb.x, 0.0);
    img[q + 1u] = max(rgb.y, 0.0);
    img[q + 2u] = max(rgb.z, 0.0);
}}
"#
    )
}

/// Noise reduction, on the GPU.
pub struct GpuNoiseReductionStage {
    s_lum: f32,
    s_col: f32,
    eps: f32,
    lum_r: usize,
    col_r: usize,
    cpu: pipeline::NoiseReductionStage,
}

impl GpuNoiseReductionStage {
    /// ⚠ Unlike `GpuDehazeStage` this needs **no image** — noise reduction estimates nothing from
    /// the photograph, so every parameter comes from the `Edits` and the render scale. The seam
    /// hands the buffer over regardless, because dehaze's airlight needs it.
    pub fn new(params: &NoiseReduction, scale: RenderScale, _ctx: &Arc<GpuContext>) -> Self {
        Self {
            s_lum: (params.luminance / 100.0).clamp(0.0, 1.0),
            s_col: (params.color / 100.0).clamp(0.0, 1.0),
            eps: pipeline::eps_for_detail(params.detail),
            lum_r: pipeline::scaled_radius(pipeline::LUM_RADIUS_FULL, scale),
            col_r: pipeline::scaled_radius(
                pipeline::chroma_radius_full(params.smoothness),
                scale,
            ),
            cpu: pipeline::NoiseReductionStage::new(params, scale),
        }
    }

    /// The dispatch sequence, in order, each pass with the grid it wants.
    ///
    /// ⚠ The luminance phase finishes into `p1` **before** the chroma phase starts, which is what
    /// lets the two share `p0` and `p2` and keeps the stage at four planes rather than six.
    fn passes(&self) -> Vec<(&'static str, Grid)> {
        let (lb, cb) = (box_grids(self.lum_r), box_grids(self.col_r));
        let mut v: Vec<(&'static str, Grid)> = Vec::new();
        if self.s_lum > 0.0 {
            v.push(("seed_l", Grid::Pixel));
            // The self-guided filter: two statistic blurs, one slope/offset kernel, two more.
            v.push(("square", Grid::Pixel));
            v.push(("copy_l", Grid::Pixel));
            v.extend([("lb_h1", lb.0), ("lb_v1", lb.1)]);
            v.extend([("lb_h2", lb.0), ("lb_v2", lb.1)]);
            v.push(("slope_offset", Grid::Pixel));
            v.extend([("lb_h1", lb.0), ("lb_v1", lb.1)]);
            v.extend([("lb_h2", lb.0), ("lb_v2", lb.1)]);
            v.push(("combine_lum", Grid::Pixel));
        }
        if self.s_col > 0.0 {
            v.push(("seed_c", Grid::Pixel));
            for _ in 0..pipeline::NR_CHROMA_PASSES {
                v.extend([("cb_h0", cb.0), ("cb_v0", cb.1)]);
            }
            for _ in 0..pipeline::NR_CHROMA_PASSES {
                v.extend([("cb_h2", cb.0), ("cb_v2", cb.1)]);
            }
        }
        v.push(("apply_nr", Grid::Pixel));
        v
    }

    /// ⚠ Emitted whatever the sliders say, so one shader text serves every path. The program
    /// cache keys on that text, so a rung with different radii compiles its own — correct, and a
    /// handful of programs rather than one per frame.
    fn shader(&self) -> String {
        let mut src = String::from(crate::oklab_wgsl::OKLAB_WGSL);
        src.push_str(&preamble());
        src.push_str(&box_pass("lb_h1", "lb_v1", "p1", "p1", "p3", self.lum_r));
        src.push_str(&box_pass("lb_h2", "lb_v2", "p2", "p2", "p3", self.lum_r));
        src.push_str(&box_pass("cb_h0", "cb_v0", "p0", "p0", "p3", self.col_r));
        src.push_str(&box_pass("cb_h2", "cb_v2", "p2", "p2", "p3", self.col_r));
        src
    }
}

impl GpuStage for GpuNoiseReductionStage {
    fn name(&self) -> &'static str {
        "noise_reduction"
    }

    /// Measured whole-buffer at 3123x2085 (6.5 Mpx) by `bench_preview`'s native block.
    ///
    /// ⚠ Ships at the bottom of the measured band, the conservative direction — a lower number
    /// makes a step less likely to chain, and the gate must fail toward the CPU.
    fn cpu_cost(&self) -> f32 {
        86.0
    }

    /// ⚠ Four, against dehaze's seven: the self-guided filter needs two statistics where the
    /// general form needs four, and the two phases are sequenced so they share planes. 104 MB at
    /// 6.5 Mpx, 398 MiB at full resolution.
    fn scratch_planes(&self) -> usize {
        4
    }

    fn encode(
        &self,
        ctx: &GpuContext,
        pass: &mut wgpu::ComputePass,
        img: &GpuImage,
        scratch: &[wgpu::Buffer],
    ) {
        let device = &ctx.device;
        let total = img.pixels().div_ceil(WORKGROUP);
        let row_groups = total.min(MAX_GROUPS_PER_DIM);
        let col_groups = total.div_ceil(row_groups);
        let rows = (img.height() as u32).div_ceil(WORKGROUP);
        let cols = (img.width() as u32).div_ceil(WORKGROUP);

        let src = self.shader();
        let seq = self.passes();
        let mut names: Vec<&'static str> = Vec::new();
        for (n, _) in &seq {
            if !names.contains(n) {
                names.push(n);
            }
        }

        let entry = |binding: u32, ty: wgpu::BufferBindingType| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let rw = wgpu::BufferBindingType::Storage { read_only: false };
        let mut layout_entries = vec![entry(0, rw), entry(1, wgpu::BufferBindingType::Uniform)];
        for b in 2..6 {
            layout_entries.push(entry(b, rw));
        }
        let (layout, pipelines) =
            ctx.cache
                .program(device, "noise_reduction", &src, &layout_entries, &names);

        let mut b = [0u8; 32];
        b[0..4].copy_from_slice(&(img.width() as u32).to_le_bytes());
        b[4..8].copy_from_slice(&(img.height() as u32).to_le_bytes());
        b[8..12].copy_from_slice(&img.pixels().to_le_bytes());
        b[12..16].copy_from_slice(&row_groups.to_le_bytes());
        b[16..20].copy_from_slice(&self.s_lum.to_le_bytes());
        b[20..24].copy_from_slice(&self.s_col.to_le_bytes());
        b[24..28].copy_from_slice(&self.eps.to_le_bytes());

        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("noise-reduction-params"),
            size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        ctx.queue.write_buffer(&uniform, 0, &b);

        let mut bound = vec![img.buffer.as_entire_binding(), uniform.as_entire_binding()];
        for s in scratch.iter().take(4) {
            bound.push(s.as_entire_binding());
        }
        let entries: Vec<wgpu::BindGroupEntry<'_>> = bound
            .into_iter()
            .enumerate()
            .map(|(i, resource)| wgpu::BindGroupEntry {
                binding: i as u32,
                resource,
            })
            .collect();
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("noise-reduction-bind"),
            layout: &layout,
            entries: &entries,
        });

        for (name, grid) in &seq {
            let idx = names.iter().position(|n| n == name).expect("pass compiled");
            let (gx, gy) = match grid {
                Grid::Pixel => (row_groups, col_groups),
                Grid::Rows => (rows, 1),
                Grid::Cols => (cols, 1),
            };
            pass.set_pipeline(&pipelines[idx]);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(gx, gy, 1);
        }
    }

    fn cpu(&self) -> &dyn PipelineStage {
        &self.cpu
    }
}
```

- [ ] **Step 2: Register the module**

In `crates/gpu-pipeline/src/lib.rs`, add `mod noise_reduction;` to the `mod` list, in alphabetical
position — between `mod min_wgsl;` and `mod oklab_wgsl;`:

```rust
mod min_wgsl;
mod noise_reduction;
mod oklab_wgsl;
```

And add the re-export, in alphabetical position between `GpuLocalContrastStage` and
`GpuOutputTransformStage`:

```rust
pub use local_contrast::GpuLocalContrastStage;
pub use noise_reduction::GpuNoiseReductionStage;
pub use output_transform::GpuOutputTransformStage;
```

- [ ] **Step 3: Check it compiles**

```bash
cargo build --release -p gpu-pipeline 2>&1 | tail -20
```

Expected: builds. Do **not** commit yet — Task 3's tests land in the same commit.

---

## Task 3: The tests for the stage

**Files:**
- Modify: `crates/gpu-pipeline/src/noise_reduction.rs` (append the test module)

- [ ] **Step 1: Append the test module**

Append this to the end of `crates/gpu-pipeline/src/noise_reduction.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use core_image::ImageBuf;

    /// ⚠ Scale 1.0, so the radii are the unscaled ones: `LUM_RADIUS_FULL = 6` (below
    /// `RUNNING_SUM_MIN_RADIUS`, so the guided filter takes the **naive** body) and a chroma
    /// radius of 8..32 (above it, so the chroma blur takes the **running** body). Both production
    /// shapes in one fixture.
    fn scale() -> RenderScale {
        RenderScale::new(1.0)
    }

    fn ctx() -> Option<Arc<GpuContext>> {
        match crate::shared() {
            Some(c) => Some(c.clone()),
            None => {
                eprintln!("SKIPPED: no GPU adapter on this machine");
                None
            }
        }
    }

    /// A noisy scene with real structure, and the three properties are load-bearing.
    ///
    /// (1) **Noise on every channel**, decorrelated, or there is nothing for either half to
    /// remove. (2) **A hard edge**, because the guided filter's whole job is to keep one — a
    /// fixture without one cannot tell the filter from a plain blur. (3) **Saturated colour**, or
    /// the chroma half acts on `a`/`b` that are near zero and a wrong blur is invisible.
    ///
    /// ⚠ 200x160 so the chroma reach fits: two passes at r=32 reach 64 px, comfortably inside
    /// the short edge.
    fn noisy(w: usize, h: usize) -> ImageBuf {
        let mut img = ImageBuf::new(w, h);
        let mut s: u32 = 0x1234_5678;
        let mut rnd = move || {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            (s as f32 / u32::MAX as f32) - 0.5
        };
        for y in 0..h {
            for x in 0..w {
                let base = if x < w / 2 {
                    [0.18f32, 0.42, 0.55]
                } else {
                    [0.62, 0.20, 0.12]
                };
                let mut px = [0.0f32; 3];
                for c in 0..3 {
                    px[c] = (base[c] + 0.06 * rnd()).max(0.0);
                }
                img.set_pixel(x, y, px);
            }
        }
        img
    }

    fn params(luminance: f32, color: f32) -> NoiseReduction {
        NoiseReduction {
            luminance,
            color,
            ..NoiseReduction::default()
        }
    }

    fn chained(ctx: &Arc<GpuContext>, p: &NoiseReduction, img: ImageBuf) -> ImageBuf {
        let stage = GpuNoiseReductionStage::new(p, scale(), ctx);
        crate::GpuChain::new(ctx.clone(), vec![Box::new(stage)]).apply(img)
    }

    /// ⚠⚠ **Without this every assertion below is vacuous** — a stage that did nothing would
    /// match the CPU perfectly on a fixture the CPU also leaves alone. The W5 failure mode.
    #[test]
    fn the_fixture_is_one_this_stage_actually_moves() {
        let img = noisy(200, 160);
        for (l, c) in [(60.0f32, 0.0f32), (0.0, 60.0), (60.0, 60.0)] {
            let out =
                pipeline::NoiseReductionStage::new(&params(l, c), scale()).apply(img.clone());
            let moved = img
                .data()
                .iter()
                .zip(out.data())
                .filter(|(a, b)| a != b)
                .count();
            assert!(
                moved > img.data().len() / 2,
                "fixture check: NR {l}/{c} must move most of the fixture, moved {moved}"
            );
        }
    }

    /// ⚠⚠ **All three configurations, because `apply_nr` BRANCHES on each strength.** A test of
    /// "both on" cannot see a luminance-only path that reads an uninitialised chroma plane, and
    /// the reverse. This is the only place the two branches are exercised apart.
    ///
    /// ⚠ Bounded in 8-bit levels, not bit-identical: NR inherits all three recorded divergence
    /// classes — subnormal flush-to-zero (M8.3a), FMA contraction in the lerp and in
    /// `combine_lum` (M8.3m), and the box blur's division (M8.3n).
    #[test]
    fn every_configuration_matches_the_cpu() {
        let Some(ctx) = ctx() else { return };
        for (l, c) in [(60.0f32, 0.0f32), (0.0, 60.0), (60.0, 60.0), (100.0, 100.0)] {
            let img = noisy(200, 160);
            let p = params(l, c);
            let cpu = pipeline::NoiseReductionStage::new(&p, scale()).apply(img.clone());
            let gpu = chained(&ctx, &p, img);
            let d = crate::compare(&cpu, &gpu);
            assert!(
                d.worst_levels < 1.0,
                "NR {l}/{c}: worst {} levels over {} of {} samples",
                d.worst_levels,
                d.differing,
                d.total
            );
        }
    }

    /// ⚠ The Detail slider sets `eps`, which is a **uniform** rather than a shader literal — so a
    /// stage that never wrote it, or wrote it at the wrong offset, would render every Detail
    /// value alike and the test above could not see it.
    #[test]
    fn the_detail_slider_reaches_the_shader() {
        let Some(ctx) = ctx() else { return };
        let img = noisy(200, 160);
        let mut out = Vec::new();
        for detail in [0.0f32, 100.0] {
            let p = NoiseReduction {
                luminance: 80.0,
                detail,
                ..NoiseReduction::default()
            };
            out.push(chained(&ctx, &p, img.clone()));
        }
        assert!(
            !crate::compare(&out[0], &out[1]).is_exact(),
            "Detail 0 and Detail 100 must not render alike"
        );
    }

    /// ⚠⚠ **`compare` quantizes to 8-bit levels, where a negative sample and 0 are THE SAME
    /// VALUE** — so it is structurally blind to a missing clamp, and every test above stays green
    /// with the floor deleted. M8.3e's finding about the output transform, met for the seventh
    /// time. Strong smoothing pushes a saturated pixel out of gamut, which is what the fixture's
    /// two saturated halves are for.
    #[test]
    fn no_sample_comes_back_negative() {
        let Some(ctx) = ctx() else { return };
        let img = noisy(200, 160);
        let p = params(100.0, 100.0);
        let cpu = pipeline::NoiseReductionStage::new(&p, scale()).apply(img.clone());
        assert!(
            cpu.data().contains(&0.0),
            "fixture check: the CPU must actually reach the clamp here"
        );
        let gpu = chained(&ctx, &p, img);
        for (i, v) in gpu.data().iter().enumerate() {
            assert!(
                *v >= 0.0,
                "sample {i} came back {v} — a level comparison cannot see this"
            );
        }
    }

    /// ⚠ The name is what `PrefixStepKey`'s neighbours and the seam's lookup match on.
    #[test]
    fn the_stage_is_named_like_the_cpu_one() {
        let Some(ctx) = ctx() else { return };
        assert_eq!(
            GpuNoiseReductionStage::new(&params(50.0, 50.0), scale(), &ctx).name(),
            pipeline::NoiseReductionStage::new(&params(50.0, 50.0), scale()).name()
        );
    }

    /// ⚠⚠ **The plane budget as a decision rather than a comment.** Four is what the two-phase
    /// sequencing plus the recompute in `apply_nr` buy; a change that stored the originals would
    /// need six and would double the full-resolution allocation on the one stage where that is
    /// the known hazard.
    #[test]
    fn the_stage_asks_for_four_planes() {
        let Some(ctx) = ctx() else { return };
        assert_eq!(
            GpuNoiseReductionStage::new(&params(50.0, 50.0), scale(), &ctx).scratch_planes(),
            4
        );
    }

    /// ⚠ Both radii take the body the sweep chose, and neither is near the crossover — so this
    /// pins which side each lands on rather than the constant's exact value.
    #[test]
    fn each_radius_takes_the_body_the_sweep_chose() {
        let Some(ctx) = ctx() else { return };
        let s = GpuNoiseReductionStage::new(&params(50.0, 50.0), scale(), &ctx);
        assert!(
            !use_running_sum(s.lum_r),
            "the guided filter's r={} takes the naive body",
            s.lum_r
        );
        assert!(
            use_running_sum(s.col_r),
            "the chroma blur's r={} takes the running body",
            s.col_r
        );
    }
}
```

- [ ] **Step 2: Run the tests**

```bash
cargo test --release -p gpu-pipeline noise_reduction 2>&1 | tail -30
```

Expected: 7 tests pass (or skip with "no GPU adapter"). ⚠ Read the **passed count**, not the word
`ok` — `cargo test <filter>` takes a substring and a filter matching nothing prints
`0 passed ... ok`, which reads as green.

- [ ] **Step 3: Gate and commit Tasks 2 and 3 together**

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings > /tmp/m83q-clippy.log 2>&1; echo "CLIPPY=$?"
cargo test --release -p gpu-pipeline > /tmp/m83q-test.log 2>&1; echo "TEST=$?"
```

Expected: `CLIPPY=0` and `TEST=0`. Both read from their own command, never through a pipe.

```bash
git add crates/gpu-pipeline/src/noise_reduction.rs crates/gpu-pipeline/src/lib.rs
git commit -m "feat(gpu): noise reduction as compute shaders"
```

---

## Task 4: Fire the seam

**Files:**
- Modify: `crates/gpu-pipeline/src/accelerate.rs`

- [ ] **Step 1: Add the arm**

In `accelerate_prefix_stage`, find:

```rust
    let gpu: Box<dyn GpuStage> = match name {
        "dehaze" => Box::new(crate::GpuDehazeStage::new(&edits.dehaze, scale, image, ctx)),
        _ => return None,
    };
```

Replace with:

```rust
    let gpu: Box<dyn GpuStage> = match name {
        "dehaze" => Box::new(crate::GpuDehazeStage::new(&edits.dehaze, scale, image, ctx)),
        // ⚠ Takes no image — noise reduction estimates nothing from the photograph, where
        // dehaze's airlight is a property of *this* frame. The buffer is handed over regardless,
        // because one signature serves both.
        "noise_reduction" => Box::new(crate::GpuNoiseReductionStage::new(
            &edits.noise_reduction,
            scale,
            ctx,
        )),
        _ => return None,
    };
```

- [ ] **Step 2: Update the test that names NR as having no twin**

⚠ This is the designed forcing function: `every_other_prefix_stage_is_left_alone`'s doc says
*"Noise reduction is the one that will not, and naming it here is what makes adding it a
deliberate act rather than a silent one."* It is doing its job. Replace the whole test with:

```rust
    /// ⚠ Every remaining prefix stage stays on the CPU. The three resamplers are the ones that
    /// will not — they are item 3 of the agreed order and are one mechanism, not three — and
    /// naming them here is what makes adding one a deliberate act rather than a silent one.
    #[test]
    fn every_other_prefix_stage_is_left_alone() {
        let Some(ctx) = ctx() else { return };
        let img = hazy(64, 64);
        for name in ["geometry", "lens_correction", "perspective"] {
            assert!(
                accelerate_prefix_stage(name, &hazed_edits(60.0), SCALE, &img, &ctx).is_none(),
                "`{name}` has no GPU twin and must be left alone"
            );
        }
    }
```

- [ ] **Step 3: Add the carrier for the new arm**

Add these two tests immediately after `a_dehaze_prefix_step_becomes_a_chain`:

```rust
    fn denoised_edits(luminance: f32, color: f32) -> Edits {
        Edits {
            noise_reduction: edit_model::NoiseReduction {
                luminance,
                color,
                ..edit_model::NoiseReduction::default()
            },
            ..Edits::default()
        }
    }

    /// ⚠ The seam's second arm exists and fires. Without this the whole milestone could be fully
    /// tested and never reached — the W5 failure mode, which this crate has now recorded twice.
    #[test]
    fn a_noise_reduction_prefix_step_becomes_a_chain() {
        let Some(ctx) = ctx() else { return };
        let img = hazy(200, 160);
        let g = accelerate_prefix_stage(
            "noise_reduction",
            &denoised_edits(60.0, 60.0),
            SCALE,
            &img,
            &ctx,
        )
        .expect("noise reduction must be replaceable");
        assert_eq!(g.name(), "gpu_chain");
    }

    /// ⚠ The picture must be the same however the step ran — that is what makes a checkpoint
    /// written by either path interchangeable rather than merely equal in its key.
    #[test]
    fn an_accelerated_noise_reduction_step_renders_the_same_picture() {
        let Some(ctx) = ctx() else { return };
        let e = denoised_edits(70.0, 70.0);
        let img = hazy(200, 160);
        let steps = pipeline::build_prefix_steps(&e, SCALE, None, pipeline::Resampler::Bilinear);
        let step = steps
            .iter()
            .find(|s| s.stage.name() == "noise_reduction")
            .expect("fixture check: there must be a noise_reduction step");
        let cpu = step.stage.apply(img.clone());
        let gpu = accelerate_prefix_stage("noise_reduction", &e, SCALE, &img, &ctx)
            .expect("noise reduction must be replaceable")
            .apply(img.clone());
        assert!(
            !crate::compare(&img, &cpu).is_exact(),
            "fixture check: the step must actually change the picture"
        );
        let d = crate::compare(&cpu, &gpu);
        assert!(
            d.worst_levels < 1.0,
            "worst {} levels over {} of {} samples",
            d.worst_levels,
            d.differing,
            d.total
        );
    }
```

- [ ] **Step 4: Run the accelerate tests**

```bash
cargo test --release -p gpu-pipeline accelerate 2>&1 | tail -30
```

Expected: all pass. Note the passed count.

- [ ] **Step 5: Gate and commit**

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings > /tmp/m83q-clippy.log 2>&1; echo "CLIPPY=$?"
cargo test --release -p gpu-pipeline > /tmp/m83q-test.log 2>&1; echo "TEST=$?"
```

Expected: `CLIPPY=0`, `TEST=0`.

```bash
git add crates/gpu-pipeline/src/accelerate.rs
git commit -m "feat: noise reduction across the prefix seam"
```

---

## Task 5: Extend the checkpoint-sharing guard

**Why:** `PrefixStepKey` is derived from the **edits**, never from the stage — so a checkpoint
written by the CPU path is a legitimate hit for the GPU path and the reverse. M8.3o asserted that
for dehaze. With a second stage across the seam it is a claim again until something tests it, and
the dangerous direction is the same: a stale hit renders the *previous* frame's pixels, which is a
plausible photograph rather than an error.

**Files:**
- Modify: `crates/app/src/render_engine.rs`

- [ ] **Step 1: Read the existing fixture helper**

```bash
grep -n "fn rendered_hazy" -A 30 crates/app/src/render_engine.rs
```

Note its signature and how it builds its `Edits` and `FrameSpec` — the new test mirrors it with a
noise-reduction edit instead of a dehaze one.

- [ ] **Step 2: Add a denoised fixture and the guard**

Immediately after `a_checkpoint_written_by_either_path_is_a_hit_for_the_other`, add a
`rendered_denoised` helper that is `rendered_hazy` with its `Edits` replaced by

```rust
        let edits = Edits {
            noise_reduction: edit_model::NoiseReduction {
                luminance: 60.0,
                color: 60.0,
                ..edit_model::NoiseReduction::default()
            },
            ..Edits::default()
        };
```

and this test beside it:

```rust
    /// ⚠⚠ **The same property for the SECOND stage across the seam, and it is a claim again.**
    /// `PrefixStepKey` is derived from the edits, never from the stage — so a checkpoint written
    /// by one path must be a legitimate hit for the other. The dangerous direction is a stale
    /// hit, which renders the previous frame's pixels: a plausible photograph, not an error.
    #[test]
    fn a_noise_reduction_checkpoint_is_shared_between_the_paths() {
        if gpu_pipeline::shared().is_none() {
            eprintln!("SKIPPED: no GPU adapter on this machine");
            return;
        }
        let mut cache = PrefixCache::default();
        let (_, first) = rendered_denoised(true, &mut cache);
        assert_eq!(first.gpu_prefix_steps, 1, "fixture check: the GPU ran it");
        let (_, second) = rendered_denoised(false, &mut cache);
        assert_eq!(
            second.prefix_stages, 0,
            "the CPU path must hit the checkpoint the GPU path wrote"
        );
    }
```

- [ ] **Step 3: Run it**

```bash
cargo test --release -p app --lib render_engine 2>&1 | tail -30
```

Expected: passes. If `first.gpu_prefix_steps` is 0, the seam is not firing for NR — go back to
Task 4 rather than weakening the assertion.

- [ ] **Step 4: Gate and commit**

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings > /tmp/m83q-clippy.log 2>&1; echo "CLIPPY=$?"
cargo test --release -p app --lib > /tmp/m83q-test.log 2>&1; echo "TEST=$?"
```

Expected: `CLIPPY=0`, `TEST=0`.

```bash
git add crates/app/src/render_engine.rs
git commit -m "test: a noise-reduction checkpoint is shared between the paths"
```

---

## Task 6: Sabotage verification

**Why:** Every `⚠ watched failing` note in `CLAUDE.md` is one of these. When a sabotage does **not**
fail, that is a finding about the test, not a formality to wave through.

⚠⚠ **A sabotage that fails to COMPILE reads exactly like one that passes.** Check for a
`test result:` line before believing anything. Every sabotage below changes a *value*, never
deletes a `{placeholder}`.

⚠ **Take a copy before each sabotage and restore from the copy** — `git checkout <file>` reverts
the *whole* file and has destroyed uncommitted work in this project before.

- [ ] **Step 1: Run each sabotage**

For each row: `cp <file> /tmp/sab.bak`, apply the change, run the command, confirm a
`test result: FAILED` line naming the predicted test, then `cp /tmp/sab.bak <file>`.

| # | Sabotage | File | Predicted to fail |
|---|---|---|---|
| 1 | `eps` interpolated arithmetically: in `eps_for_detail`, `EPS_AT_DETAIL_0 + (EPS_AT_DETAIL_100 - EPS_AT_DETAIL_0) * t` | `pipeline/src/stages/noise_reduction.rs` | `eps_spans_two_decades_geometrically` |
| 2 | `NR_CHROMA_PASSES` loops collapsed to `for _ in 0..1` in `passes()` | `gpu-pipeline/src/noise_reduction.rs` | `every_configuration_matches_the_cpu` |
| 3 | `let vr = cov;` in `slope_offset` (the clamp dropped) | `gpu-pipeline/src/noise_reduction.rs` | `every_configuration_matches_the_cpu` |
| 4 | Lerp inverted in `apply_nr`: `l = p1[i] + (l - p1[i]) * params.s_lum;` | `gpu-pipeline/src/noise_reduction.rs` | `every_configuration_matches_the_cpu` |
| 5 | Radius scaling dropped: `lum_r: pipeline::LUM_RADIUS_FULL as usize` | `gpu-pipeline/src/noise_reduction.rs` | `every_configuration_matches_the_cpu` at a fit scale — ⚠ see step 2 |
| 6 | `eps` never written: delete the `b[24..28]` line | `gpu-pipeline/src/noise_reduction.rs` | `the_detail_slider_reaches_the_shader` |
| 7 | The seam's arm removed (`"noise_reduction"` arm deleted) | `gpu-pipeline/src/accelerate.rs` | `a_noise_reduction_prefix_step_becomes_a_chain` |
| 8 | Zero clamp removed: `img[q] = rgb.x;` and the two below it | `gpu-pipeline/src/noise_reduction.rs` | `no_sample_comes_back_negative` **only** |

Command for all of them:

```bash
cargo test --release -p gpu-pipeline 2>&1 | tail -30
```

For #1, use `cargo test -p pipeline 2>&1 | tail -20`.

- [ ] **Step 2: Handle sabotage 5 honestly**

At `RenderScale::new(1.0)` the scaling is the identity, so sabotage 5 changes **nothing** and the
test suite stays green. That is a finding about the fixture, not about the code. Add this test to
`crates/gpu-pipeline/src/noise_reduction.rs`'s test module and re-run the sabotage against it:

```rust
    /// ⚠⚠ **At scale 1.0 the radius scaling is the identity, so every test above is blind to it
    /// being dropped entirely.** A fit rung is the only place the two disagree — and it is the
    /// rung the app spends most of its time at.
    #[test]
    fn the_radii_follow_the_render_scale() {
        let Some(ctx) = ctx() else { return };
        let fit = RenderScale::new(0.25);
        let p = params(50.0, 50.0);
        let native = GpuNoiseReductionStage::new(&p, scale(), &ctx);
        let small = GpuNoiseReductionStage::new(&p, fit, &ctx);
        assert_eq!(
            small.lum_r,
            pipeline::scaled_radius(pipeline::LUM_RADIUS_FULL, fit),
            "the guided radius must follow the render scale"
        );
        assert!(
            small.lum_r < native.lum_r && small.col_r < native.col_r,
            "a fit rung must use smaller radii than a native one"
        );
    }
```

Re-run sabotage 5 and confirm this test fails.

- [ ] **Step 3: Record every outcome**

Write down, for each of the eight: **CAUGHT** (naming the test), or **NOT CAUGHT**. A sabotage
that is not caught means the test is blind — write the discriminating test rather than moving on.
A sabotage that is *provably inert* is recorded as inert with the reason, never counted as caught.

- [ ] **Step 4: Confirm the tree is clean, then commit the new test**

```bash
git status --short
```

Expected: only `crates/gpu-pipeline/src/noise_reduction.rs` modified (the new test from step 2).
Anything else means a sabotage was not restored.

```bash
cargo fmt
git add crates/gpu-pipeline/src/noise_reduction.rs
git commit -m "test: the radii follow the render scale"
```

---

## Task 7: Measure, and fill in the bench

**Files:**
- Modify: `crates/app/examples/bench_preview.rs`
- Modify: `crates/gpu-pipeline/src/noise_reduction.rs` (only if the band disagrees with 86.0)

- [ ] **Step 1: Find the placeholder line**

```bash
grep -n "has no GPU twin" -B 20 crates/app/examples/bench_preview.rs
```

The surrounding block already calls `gpu_pipeline::accelerate_prefix_stage(name, &heavy, sc, &checkpoints[i], ctx)`
per step and times `dehaze`. With Task 4 landed, that call now returns `Some` for
`noise_reduction` too — so the placeholder branch is already dead and the timing runs. Confirm by
reading the block; if the name is special-cased, remove the special case so the loop prices
whichever steps have a twin.

- [ ] **Step 2: Run the bench three times**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
uptime
for i in 1 2 3; do
  cargo run --release -p app --example bench_preview ~/Pictures/fuji/DSCF6296.RAF 2>&1 \
    | grep -E "NATIVE rung|ALONE|GPU|drag"
done
uptime
```

Record: the load average at both ends, and for each run the `noise_reduction ALONE (whole buffer)`
CPU figure, the new `noise_reduction ALONE on the GPU` figure, and the `drag noise_reduction` row.

- [ ] **Step 3: Set `cpu_cost` from the band**

`cpu_cost` ships at the **bottom** of the measured CPU band — the conservative direction, because
a lower number makes a step less likely to chain and the gate must fail toward the CPU. It is
currently **86.0**, from a single best-of-5 reading of 86.1 ms. If the three-run band's floor is
below 86.0, lower it to that floor rounded down and update the doc comment with the band. If the
band's floor is at or above 86.0, leave it and record the band in the doc comment anyway.

- [ ] **Step 4: Commit the measurement**

```bash
git add crates/app/examples/bench_preview.rs crates/gpu-pipeline/src/noise_reduction.rs
git commit -m "bench: price noise reduction on the GPU"
```

---

## Task 8: The full gate

- [ ] **Step 1: Run all three, each exit code from its own command**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo fmt --check > /tmp/m83q-fmt.log 2>&1; echo "FMT=$?"
cargo clippy --all-targets --all-features -- -D warnings > /tmp/m83q-clippy.log 2>&1; echo "CLIPPY=$?"
cargo test --workspace > /tmp/m83q-test.log 2>&1; echo "TEST=$?"
```

Expected: `FMT=0`, `CLIPPY=0`, `TEST=0`.

- [ ] **Step 2: Assert the goldens ran and none moved**

```bash
grep -c "^test golden_" /tmp/m83q-test.log
grep -c "FAILED" /tmp/m83q-test.log
```

Expected: **63** goldens, **0** failures. ⚠ A golden that is *skipped* prints nothing, so counting
is what distinguishes "ran and passed" from "never ran".

- [ ] **Step 3: Assert the test count moved by exactly the number added**

```bash
grep -E "^test result: ok\. [0-9]+ passed" /tmp/m83q-test.log \
  | awk '{s+=$4} END {print "TOTAL PASSED:", s}'
grep -c "SKIPPED: no GPU adapter" /tmp/m83q-test.log
```

The baseline before this branch is **2417**. This plan adds: 8 in `gpu-pipeline/noise_reduction`
(seven from Task 3 plus one from Task 6), 2 in `accelerate` (Task 4), 1 in `render_engine`
(Task 5) — **11**, so the expected total is **2428**.

⚠⚠ **If it is not exactly 2428, decompose per crate before believing either number.** M8.3o found
a factual error in M8.3n's record that way: a gate run one commit early describes a tree nobody
shipped, and only "the count moved by exactly N" catches it. Expected GPU skips: **0** on this
machine.

- [ ] **Step 4: Launch the GUI for the confirmation check**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo run --release -p app
```

Run it in the background. What to say to the user, in this order:

1. ⚠⚠ **Start at 1:1 with noise reduction on**, because that is the configuration that killed the
   render worker when dehaze landed. NR is the second stage ever to allocate a full-resolution GPU
   image — 398 MiB of scratch on an X-T4. It should simply render.
2. Drag **Luminance** and then **Color** at a native rung with `Settings → Rendering → "Use the
   GPU"` on. It should be visibly smoother, and ⚠⚠ **the picture must not change**.
3. ⚠ **The half a still frame cannot catch is the chroma blur's border.** `box_blur`'s window
   shrinks at an edge and the sum is divided by the count inside it; the alternative reading
   darkens a margin by roughly half. Look at the **edges of the frame with Color up**. It is
   M8.3n's hazard arriving in a second stage, guarded by `a_constant_plane_stays_uniform`.
4. ⚠ Also worth a look: **noise reduction together with dehaze**, which exercises the checkpoint
   sharing in the direction the tests cannot — the CPU writes the post-NR checkpoint and the GPU
   reads it, or the reverse. A stale-checkpoint bug shows as **the previous NR amount persisting**.

- [ ] **Step 5: Do NOT merge and do NOT push**

Report the gate results, the measurement, the sabotage table and the confirmation checklist. The
user decides whether to merge. **Never push.**

---

## Task 9: Record it in `CLAUDE.md`

- [ ] **Step 1: Add the milestone section**

Add an **M8.3q** section immediately above the existing `**M8.3p — the tail completed ...**`
heading, following that section's shape exactly: the honest headline first (the measured ratio,
and the 216.4 → measured figure for an NR drag), the measured table, the plane-layout decision,
the sabotage outcomes including any recorded as inert, and a numbered **Accepted caveats** list
carrying §9 of the spec.

- [ ] **Step 2: Update the RESUME HERE block**

- Replace the **State at handoff** paragraph with this milestone's, and demote the previous one to
  `⚠ Superseded handoff follows:` exactly as every prior milestone does.
- Replace the **▶ NEXT ACTION** with item 3 of the agreed order — **the three resamplers
  (geometry, perspective, lens correction) as ONE mechanism** — and demote the current one to
  `⚠ Superseded next action follows:`.
- In the agreed systematic order list, mark item 2 ✅ and note the measured result beside it.
- Update the **"the prefix is still 5 of 6 on the CPU"** reframe: it is **4 of 6** now.

- [ ] **Step 3: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: record M8.3q, noise reduction on the GPU"
```

---

## Self-review notes

**Spec coverage:** §1 → Task 7. §2 scope-out items appear in no task, which is correct. §3 → Tasks
1–2. §4 → Task 2's module doc, plus `the_stage_asks_for_four_planes` in Task 3. §5 → Task 1. §6 →
Tasks 3, 4, 5, 6. §7 → Task 7. §8 → Task 8 step 4. §9 → Task 9.

**Type consistency:** `GpuNoiseReductionStage::new(&NoiseReduction, RenderScale, &Arc<GpuContext>)`
is used identically in Task 2's definition, Task 3's tests and Task 4's seam arm. `pipeline`'s
re-exports are named `LUM_RADIUS_FULL`, `COL_RADIUS_FULL`, `NR_CHROMA_PASSES`, `eps_for_detail`,
`chroma_radius_full` in Task 1 and read with exactly those names in Task 2.

**Known gap, stated rather than hidden:** Task 5's `rendered_denoised` is described by the edit it
differs by rather than reproduced in full, because `rendered_hazy` is a local test helper whose
exact body must be read at execution time. Step 1 of that task is the read.
