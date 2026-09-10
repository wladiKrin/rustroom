# M8.3b Resident GPU Chain and HSL Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this
> plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> ⚠⚠ **PROJECT CONVENTIONS, NOT NEGOTIABLE.** Commit messages carry **NO `Co-Authored-By`
> trailer**. **NEVER push.** Work on the branch `m8.3b-resident-tail`, which already exists.
>
> ⚠ **Build environment.** Before any cargo command:
> ```
> export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
> ```
>
> ⚠⚠ **Iterate with `cargo test --release -p gpu-pipeline`.** Acquiring a GPU adapter costs
> **~42 s in a debug build** against ~0.1 s in release (M8.3a measured it; not root-caused).
>
> ⚠ Read every gate's exit code **from a file, never a pipe**, with session-unique log names.
> After every scripted edit assert the new text landed. After every sabotage confirm the **named**
> test failed — a sabotage that fails to compile looks exactly like one that passes.

**Goal:** A chain of GPU stages costs one round trip rather than N, and `HslStage` runs on the GPU
behind a Settings toggle.

**Architecture:** `GpuStage` is a new trait that only *encodes* a dispatch, so stages compose
without touching the CPU. `GpuChain` holds a `Vec<Box<dyn GpuStage>>` and implements
`pipeline::PipelineStage` — uploading once, encoding every stage into one compute pass, and
downloading once. `accelerate(tail, edits, ctx)` swaps the stage named `hsl` for its GPU twin,
which needs no change to `pipeline` at all.

**Tech Stack:** `wgpu` 29, WGSL, `pollster`, `bytemuck` — all already in the tree.

**Spec:** `docs/superpowers/specs/2026-09-10-resident-gpu-tail-design.md`

⚠ **Masked HSL is NOT accelerated** (spec §9.1) — a mask's sub-pipeline lives inside `MaskStage`.
That is the case reported in use as slow, and this slice leaves it standing.

---

## File Structure

| | |
|---|---|
| **Create** `crates/gpu-pipeline/src/image.rs` | `GpuImage` — a resident buffer plus dimensions. |
| **Create** `crates/gpu-pipeline/src/chain.rs` | `GpuStage`, `GpuChain`, the persistent staging buffer, upload/download counters. |
| **Create** `crates/gpu-pipeline/src/hsl.rs` | `GpuHslStage` and its WGSL. |
| **Create** `crates/gpu-pipeline/src/accelerate.rs` | `accelerate` and its two guards. |
| **Modify** `crates/gpu-pipeline/src/exposure.rs` | `GpuExposureStage` implements `GpuStage`; its own `PipelineStage` impl becomes a one-stage chain. |
| **Modify** `crates/pipeline/src/stages/hue_table.rs` | One accessor: `HueTable::entries`. |
| **Modify** `crates/app/src/settings.rs`, `modals/settings.rs`, `render_engine.rs` | The toggle and where the accelerator is applied. |
| **Create** `crates/gpu-pipeline/examples/gpu_hsl_bench.rs` | The measurement. |

---

## Task 1: `GpuImage` and the `GpuStage` trait

**Files:**
- Create: `crates/gpu-pipeline/src/image.rs`
- Modify: `crates/gpu-pipeline/src/lib.rs`

- [ ] **Step 1: Write `image.rs` with its test**

```rust
//! A buffer that lives on the GPU across stages.

use crate::GpuContext;

/// An image resident in GPU memory.
///
/// ⚠ **In-place, not ping-pong.** Every per-pixel stage is in-place-safe because each invocation
/// touches only its own pixel. This is a **documented limit, not an oversight**: spatial stages
/// (M8.3c — local contrast, sharpening) read neighbours and will need a second buffer, so nothing
/// here may assume there is only ever one.
#[derive(Debug)]
pub struct GpuImage {
    pub(crate) buffer: wgpu::Buffer,
    width: usize,
    height: usize,
}

impl GpuImage {
    /// Allocate an uninitialised resident image.
    pub fn new(ctx: &GpuContext, width: usize, height: usize) -> Option<Self> {
        let len = width.checked_mul(height)?.checked_mul(3)?;
        let bytes = u64::try_from(len).ok()?.checked_mul(4)?;
        if bytes == 0 {
            return None;
        }
        let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gpu-image"),
            size: bytes,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        Some(Self { buffer, width, height })
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    /// Pixels, which is what a dispatch is sized in.
    ///
    /// ⚠ **Pixels, never floats.** A per-float 1D dispatch at fit resolution already exceeds the
    /// 65535-workgroup limit — the bug M8.3a hit by hand.
    pub fn pixels(&self) -> u32 {
        (self.width * self.height) as u32
    }

    pub(crate) fn bytes(&self) -> u64 {
        (self.width * self.height * 3 * 4) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zero_sized_image_is_refused_rather_than_allocated() {
        // ⚠ `wgpu` panics on a zero-size buffer, and a zero-width image is reachable — the same
        // degenerate case `map_pixels_xy` guards for.
        let Some(ctx) = crate::shared() else {
            eprintln!("SKIPPED: no GPU adapter on this machine");
            return;
        };
        assert!(GpuImage::new(ctx, 0, 10).is_none());
        assert!(GpuImage::new(ctx, 10, 0).is_none());
        assert!(GpuImage::new(ctx, 4, 4).is_some());
    }

    #[test]
    fn the_dispatch_is_sized_in_pixels_not_floats() {
        let Some(ctx) = crate::shared() else { return };
        let img = GpuImage::new(ctx, 100, 50).expect("allocatable");
        assert_eq!(img.pixels(), 5000, "pixels, not 15000 floats");
        assert_eq!(img.bytes(), 100 * 50 * 3 * 4);
    }
}
```

- [ ] **Step 2: Declare it**

In `crates/gpu-pipeline/src/lib.rs`, add `mod image;` beside the other modules and
`pub use image::GpuImage;` beside the other exports.

- [ ] **Step 3: Run**

```bash
cargo test --release -p gpu-pipeline 2>&1 | tail -4
```
Expected: the two new tests pass alongside the existing 11.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat: GpuImage, a buffer resident across stages

In-place rather than ping-pong: every per-pixel stage is in-place-safe.
Documented as a limit because spatial stages will need a second buffer.

pixels() rather than a float count, because a per-float 1D dispatch
exceeds the 65535-workgroup limit at fit resolution already."
```

---

## Task 2: `GpuStage`, `GpuChain`, and the barrier test

**Files:**
- Create: `crates/gpu-pipeline/src/chain.rs`
- Modify: `crates/gpu-pipeline/src/exposure.rs`
- Modify: `crates/gpu-pipeline/src/lib.rs`

⚠⚠ **This task contains the single most important assertion in the slice** (Step 1's first test).
If it fails, do not proceed to Task 3 — read Step 6's fallback instead.

- [ ] **Step 1: Write `chain.rs` — the trait, the chain, and the tests**

```rust
//! Running several GPU stages without returning to the CPU between them.

use crate::{GpuContext, GpuImage};
use core_image::ImageBuf;
use pipeline::PipelineStage;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// One stage's worth of GPU work.
///
/// ⚠⚠ Deliberately **not** `PipelineStage`. That trait's `apply(ImageBuf) -> ImageBuf` takes CPU
/// memory and returns CPU memory, so it **forces a round trip by its signature** — exactly right
/// as the seam and exactly wrong as the unit of GPU work. This one only records a dispatch.
pub trait GpuStage: Send + Sync {
    fn name(&self) -> &'static str;

    /// Record this stage's dispatch. The image is bound at group 0, binding 0.
    fn encode(&self, ctx: &GpuContext, pass: &mut wgpu::ComputePass, img: &GpuImage);

    /// The CPU stage this one replaces.
    ///
    /// ⚠⚠ Not decoration. `apply` has no failure channel, so a chain that fails on the GPU must
    /// produce the CPU answer — and a stage exposing only `encode` would have **nothing to fall
    /// back to**. Each GPU stage carrying its own twin is the same reasoning `GpuExposureStage`
    /// already applies to itself; a parallel list on the chain would be two lists that can
    /// disagree about their own order.
    fn cpu(&self) -> &dyn PipelineStage;
}

/// How many uploads and downloads the chains in this process have performed.
///
/// ⚠ A counter rather than a timing, the shape `RenderEngine::requests_issued` and
/// `ThumbLoader`'s extraction counter already take: a timing assertion is flaky under load, and
/// "one round trip for N stages" is an exact claim that deserves an exact observable.
static UPLOADS: AtomicUsize = AtomicUsize::new(0);
static DOWNLOADS: AtomicUsize = AtomicUsize::new(0);

/// Transfers performed so far, as `(uploads, downloads)`.
pub fn transfer_counts() -> (usize, usize) {
    (UPLOADS.load(Ordering::Relaxed), DOWNLOADS.load(Ordering::Relaxed))
}

/// A run of GPU stages sharing one upload and one download.
pub struct GpuChain {
    stages: Vec<Box<dyn GpuStage>>,
    ctx: Arc<GpuContext>,
}

impl GpuChain {
    pub fn new(ctx: Arc<GpuContext>, stages: Vec<Box<dyn GpuStage>>) -> Self {
        Self { stages, ctx }
    }

    pub fn len(&self) -> usize {
        self.stages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.stages.is_empty()
    }

    /// Run every stage's CPU twin, in order.
    fn cpu_fallback(&self, mut image: ImageBuf) -> ImageBuf {
        // ⚠ Every stage, not some — a partially-GPU result would be a third rendering path
        // nobody designed.
        for s in &self.stages {
            image = s.cpu().apply(image);
        }
        image
    }

    /// The GPU path, or `None` on any failure at all.
    fn run(&self, image: &ImageBuf) -> Option<Vec<f32>> {
        let ctx = &self.ctx;
        let device = &ctx.device;
        let img = GpuImage::new(ctx, image.width(), image.height())?;
        let bytes = img.bytes();

        // ── upload, once ──────────────────────────────────────────────────────────────
        // ⚠ A `MAP_WRITE` staging buffer rather than `queue.write_buffer`: measured 1.86 ms
        // against 3.90 for 19.6 MB, a 2.1x difference. The memcpy into mapped memory is the
        // floor (0.8-0.9 ms) and wgpu exposes no way around it.
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("chain-staging"),
            size: bytes,
            usage: wgpu::BufferUsages::MAP_WRITE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: true,
        });
        staging
            .slice(..)
            .get_mapped_range_mut()
            .copy_from_slice(bytemuck::cast_slice(image.data()));
        staging.unmap();
        let mut enc = device.create_command_encoder(&Default::default());
        enc.copy_buffer_to_buffer(&staging, 0, &img.buffer, 0, bytes);
        UPLOADS.fetch_add(1, Ordering::Relaxed);

        // ── every stage, one pass ─────────────────────────────────────────────────────
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            for s in &self.stages {
                s.encode(ctx, &mut pass, &img);
            }
        }

        // ── download, once ────────────────────────────────────────────────────────────
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("chain-readback"),
            size: bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        enc.copy_buffer_to_buffer(&img.buffer, 0, &readback, 0, bytes);
        ctx.queue.submit(Some(enc.finish()));

        let slice = readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        rx.recv().ok()?.ok()?;
        let out: Vec<f32> = bytemuck::cast_slice(&slice.get_mapped_range()).to_vec();
        readback.unmap();
        DOWNLOADS.fetch_add(1, Ordering::Relaxed);
        Some(out)
    }
}

impl PipelineStage for GpuChain {
    fn name(&self) -> &'static str {
        "gpu_chain"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        // ⚠ An empty chain must not upload at all: a chain with no stages is a legitimate state,
        // and paying a round trip to change nothing is the one thing it must never do.
        if self.stages.is_empty() {
            return image;
        }
        let (w, h) = (image.width(), image.height());
        match self.run(&image) {
            Some(data) => ImageBuf::from_vec(w, h, data),
            None => self.cpu_fallback(image),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GpuExposureStage;

    fn ctx() -> Option<Arc<GpuContext>> {
        match crate::shared() {
            Some(c) => Some(c.clone()),
            None => {
                eprintln!("SKIPPED: no GPU adapter on this machine");
                None
            }
        }
    }

    fn ramp(w: usize, h: usize) -> ImageBuf {
        let n = w * h * 3;
        ImageBuf::from_vec(w, h, (0..n).map(|i| (i % 601) as f32 / 601.0).collect())
    }

    #[test]
    fn three_chained_exposures_equal_one_combined_exposure() {
        // ⚠⚠ **THE BARRIER TEST, and the most important assertion in this slice.** Successive
        // dispatches in one compute pass write and then read the same storage buffer. If WebGPU
        // does not guarantee memory visibility between them, stage N+1 reads stage N's unwritten
        // data — a failure that is **intermittent and resolution-dependent**, fine on a small
        // fixture and corrupting on a photograph.
        //
        // ⚠ Gains multiply, so a missing barrier gives a WRONG PRODUCT rather than a plausible
        // image: +1.0, -0.5 and +0.25 EV compose to exactly +0.75 EV.
        let Some(ctx) = ctx() else { return };
        let img = ramp(128, 96);
        let chain = GpuChain::new(
            ctx.clone(),
            vec![
                Box::new(GpuExposureStage::new(1.0, ctx.clone())),
                Box::new(GpuExposureStage::new(-0.5, ctx.clone())),
                Box::new(GpuExposureStage::new(0.25, ctx.clone())),
            ],
        );
        let chained = chain.apply(img.clone());
        let single = pipeline::ExposureStage::new(0.75).apply(img);
        let d = crate::compare(&single, &chained);
        assert!(
            d.worst_levels < 0.01,
            "three chained exposures must equal one at +0.75 EV; worst {} levels, {} of {} \
             samples differ — suspect a missing barrier between dispatches",
            d.worst_levels,
            d.differing,
            d.total
        );
    }

    #[test]
    fn a_chain_of_eight_stages_performs_exactly_one_round_trip() {
        // ⚠⚠ The claim the whole slice exists for, asserted by COUNTING rather than timing — a
        // timing assertion is flaky under load, and this machine has had a background process at
        // 100% CPU throughout.
        let Some(ctx) = ctx() else { return };
        let stages: Vec<Box<dyn GpuStage>> = (0..8)
            .map(|_| Box::new(GpuExposureStage::new(0.1, ctx.clone())) as Box<dyn GpuStage>)
            .collect();
        let chain = GpuChain::new(ctx, stages);
        let before = transfer_counts();
        let _ = chain.apply(ramp(64, 48));
        let after = transfer_counts();
        assert_eq!(
            (after.0 - before.0, after.1 - before.1),
            (1, 1),
            "eight stages must share ONE upload and ONE download"
        );
    }

    #[test]
    fn an_empty_chain_returns_the_input_and_uploads_nothing() {
        // ⚠ Both halves. A chain with no stages is legitimate, and paying a round trip to change
        // nothing is the one thing it must never do — which a value-only assertion cannot see.
        let Some(ctx) = ctx() else { return };
        let img = ramp(32, 24);
        let chain = GpuChain::new(ctx, Vec::new());
        let before = transfer_counts();
        let out = chain.apply(img.clone());
        assert_eq!(out.data(), img.data(), "an empty chain changes nothing");
        assert_eq!(transfer_counts(), before, "and transfers nothing");
    }

    #[test]
    fn a_chain_of_one_matches_the_cpu_stage() {
        let Some(ctx) = ctx() else { return };
        let img = ramp(64, 48);
        let chain = GpuChain::new(ctx.clone(), vec![Box::new(GpuExposureStage::new(1.3, ctx))]);
        let gpu = chain.apply(img.clone());
        let cpu = pipeline::ExposureStage::new(1.3).apply(img);
        assert_eq!(gpu.data(), cpu.data(), "one exposure is exactly representable on both");
    }

    #[test]
    fn the_fallback_runs_every_stages_cpu_twin_in_order() {
        // ⚠⚠ Asserted against the CPU stages' own composed output, not against "non-empty" — the
        // latter would pass against a fallback that returned the input unchanged.
        //
        // ⚠ A zero-sized image is the failure this can provoke without unplugging a GPU:
        // `GpuImage::new` refuses it, so `run` returns None and the fallback takes over.
        let Some(ctx) = ctx() else { return };
        let chain = GpuChain::new(
            ctx.clone(),
            vec![
                Box::new(GpuExposureStage::new(1.0, ctx.clone())),
                Box::new(GpuExposureStage::new(0.5, ctx)),
            ],
        );
        let img = ramp(16, 12);
        let want = pipeline::ExposureStage::new(0.5)
            .apply(pipeline::ExposureStage::new(1.0).apply(img.clone()));
        assert_eq!(chain.cpu_fallback(img).data(), want.data());
    }
}
```

- [ ] **Step 2: Make `GpuExposureStage` implement `GpuStage`**

In `crates/gpu-pipeline/src/exposure.rs`, split the existing `run` so the dispatch is separable
from the transfer. Replace the `impl PipelineStage for GpuExposureStage` block and add a
`GpuStage` impl:

```rust
impl crate::GpuStage for GpuExposureStage {
    fn name(&self) -> &'static str {
        "exposure"
    }

    /// ⚠ The bind group is built per encode rather than cached. It references the chain's buffer,
    /// which differs per image, so a cached one would be a handle to the wrong memory — the class
    /// of bug that renders a previous photograph.
    fn encode(&self, ctx: &GpuContext, pass: &mut wgpu::ComputePass, img: &GpuImage) {
        let pixels = img.pixels();
        let total_groups = pixels.div_ceil(WORKGROUP);
        let row_groups = total_groups.min(MAX_GROUPS_PER_DIM);
        let col_groups = total_groups.div_ceil(row_groups);
        let params: [u32; 4] = [self.factor.to_bits(), pixels, row_groups, 0];
        let (compute, bind) = self.pipeline_and_bind(ctx, img, &params);
        pass.set_pipeline(&compute);
        pass.set_bind_group(0, &bind, &[]);
        pass.dispatch_workgroups(row_groups, col_groups, 1);
    }

    fn cpu(&self) -> &dyn PipelineStage {
        &self.cpu
    }
}
```

⚠ `pipeline_and_bind` is a private helper holding the shader-module, layout, bind-group and
compute-pipeline construction that `run` already performs — extracted verbatim so the existing
bit-exactness test is the guard that the extraction moved no behaviour.

⚠⚠ **`GpuExposureStage`'s own `PipelineStage::apply` becomes a one-stage chain**, so there is
exactly one upload/download implementation rather than two that can drift:

```rust
impl PipelineStage for GpuExposureStage {
    fn name(&self) -> &'static str {
        "exposure"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        crate::GpuChain::new(
            self.ctx.clone(),
            vec![Box::new(GpuExposureStage::new_from(self))],
        )
        .apply(image)
    }
}
```

⚠ `new_from(&self)` clones the stage's parameters into a fresh instance; a `Box<dyn GpuStage>`
needs ownership, and `GpuExposureStage` is cheap to rebuild (an `f32`, a CPU stage, an `Arc`).

⚠ **`apply_with` and its test go away**, because the fallback is now the chain's and is tested
there. Delete both rather than leaving a second, untested path.

- [ ] **Step 3: Declare the module**

In `lib.rs`: `mod chain;` and `pub use chain::{transfer_counts, GpuChain, GpuStage};`

- [ ] **Step 4: Run**

```bash
cargo test --release -p gpu-pipeline 2>&1 | grep -E "^test |test result" | tail -20
```
Expected: all pass, including `three_chained_exposures_equal_one_combined_exposure`.

- [ ] **Step 5: Sabotages**

```bash
cp crates/gpu-pipeline/src/chain.rs /tmp/m83b-chain.bak
run_sab() { cargo test --release -p gpu-pipeline > /tmp/m83b-sab.log 2>&1
  grep -q "test result:" /tmp/m83b-sab.log && grep -E "FAILED" /tmp/m83b-sab.log | head -3 || echo "  DID NOT COMPILE"
  cp /tmp/m83b-chain.bak crates/gpu-pipeline/src/chain.rs; }

# (a) upload per stage -> the round-trip count test must fail
python3 - <<'PY'
p='crates/gpu-pipeline/src/chain.rs'; s=open(p).read()
o="        UPLOADS.fetch_add(1, Ordering::Relaxed);"
assert o in s, "sabotage (a) did not match"
open(p,'w').write(s.replace(o, "        UPLOADS.fetch_add(self.stages.len(), Ordering::Relaxed);"))
PY
run_sab

# (b) an empty chain still round-trips
python3 - <<'PY'
p='crates/gpu-pipeline/src/chain.rs'; s=open(p).read()
o="        if self.stages.is_empty() {\n            return image;\n        }\n"
assert o in s, "sabotage (b) did not match"
open(p,'w').write(s.replace(o, ""))
PY
run_sab

# (c) the fallback returns the input rather than the CPU stages' output
python3 - <<'PY'
p='crates/gpu-pipeline/src/chain.rs'; s=open(p).read()
o="            image = s.cpu().apply(image);"
assert o in s, "sabotage (c) did not match"
open(p,'w').write(s.replace(o, "            let _ = s;"))
PY
run_sab
```
Expected: (a) `a_chain_of_eight_stages_performs_exactly_one_round_trip`; (b)
`an_empty_chain_returns_the_input_and_uploads_nothing`; (c)
`the_fallback_runs_every_stages_cpu_twin_in_order`.

- [ ] **Step 6: If the barrier test failed**

⚠⚠ **Do not widen its tolerance.** The fix is **one compute pass per stage** — move
`begin_compute_pass` inside the loop. The buffer stays resident and the round trip is still one,
so the design survives; only the pass structure changes. Record it in the commit message and in
`CLAUDE.md`, because "WebGPU does not barrier between dispatches in a pass" is a fact worth
knowing once.

- [ ] **Step 7: Gate and commit**

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings > /tmp/m83b-cl.log 2>&1; echo "CLIPPY=$?"
cargo test --release -p gpu-pipeline > /tmp/m83b-t.log 2>&1; echo "TEST=$?"; grep "test result" /tmp/m83b-t.log | head -1
```

```bash
git add -A
git commit -m "feat: GpuChain -- N stages, one round trip

GpuStage is deliberately not PipelineStage: apply(ImageBuf) -> ImageBuf
forces a round trip by its signature, which is right as the seam and
wrong as the unit of GPU work.

The barrier test is the important one: three exposures at +1.0, -0.5 and
+0.25 EV must equal one at +0.75. Gains multiply, so a missing memory
barrier between dispatches gives a wrong product rather than a plausible
image -- and that failure would be intermittent and resolution-dependent.

The round-trip claim is asserted by COUNTING uploads, not by timing.

Upload goes through a MAP_WRITE staging buffer: 1.86 ms against
write_buffer's 3.90 for 19.6 MB. The memcpy into mapped memory is the
floor and wgpu exposes no way around it.

GpuExposureStage's own apply is now a one-stage chain, so there is one
transfer implementation rather than two that can drift."
```

---

## Task 3: `GpuHslStage`

**Files:**
- Modify: `crates/pipeline/src/stages/hue_table.rs`
- Create: `crates/gpu-pipeline/src/hsl.rs`
- Modify: `crates/gpu-pipeline/src/lib.rs`

- [ ] **Step 1: Expose the table's entries**

In `crates/pipeline/src/stages/hue_table.rs`, add to `impl HueTable`:

```rust
    /// The raw entries, for a consumer that uploads them.
    ///
    /// ⚠ Read-only and by reference. `gpu-pipeline` uploads this table as a storage buffer so the
    /// **band logic never enters the shader** — the eight bands and their smoothstep blending are
    /// already collapsed here, on the CPU, once per stage.
    pub fn entries(&self) -> &[[f32; 3]] {
        &self.entries
    }
```

- [ ] **Step 2: Write `hsl.rs` with its tests**

```rust
//! HSL as a compute shader — the first expensive stage across the seam.

use crate::{GpuContext, GpuImage, GpuStage};
use core_image::ImageBuf;
use pipeline::hue_table::MAX_HUE_SHIFT_DEG;
use pipeline::PipelineStage;
use std::sync::Arc;

const WORKGROUP: u32 = 64;
const MAX_GROUPS_PER_DIM: u32 = 65535;

/// ⚠ Mirrors `stages::hsl`'s own constants. They are duplicated rather than re-exported because
/// `pipeline` keeps them private and widening its API for one consumer is the trade this project
/// declined for `pub mod stages`. **A test asserts the shader agrees with the CPU stage**, which
/// is what would catch them drifting.
const EPS: f32 = 1e-6;
const CHROMA_FADE: f32 = 0.02;

/// ⚠⚠ **`sign(x) * pow(abs(x), 1/3)`, never `pow(x, 1/3)`.** WGSL's `pow` returns NaN for a
/// negative base, and the CPU uses `cbrt` **specifically because it is sign-preserving** — a wide
/// working space produces scene-referred negatives routinely, and `oklab.rs` says so at the call
/// site. A naive port turns every such pixel into NaN, which then propagates through the matrix.
const SHADER: &str = r#"
struct Params {
    pixels: u32,
    row_groups: u32,
    table_len: u32,
    max_shift_deg: f32,
};
@group(0) @binding(0) var<storage, read_write> data: array<f32>;
@group(0) @binding(1) var<uniform> params: Params;
@group(0) @binding(2) var<storage, read> table: array<f32>;

fn cbrt_signed(x: f32) -> f32 { return sign(x) * pow(abs(x), 0.33333333); }

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let pixel = id.y * (params.row_groups * 64u) + id.x;
    if (pixel >= params.pixels) { return; }
    let p = pixel * 3u;
    let r = data[p]; let g = data[p + 1u]; let bl = data[p + 2u];

    // ProPhoto -> Oklab, the same fused matrix as color::prophoto_to_oklab.
    let lc = 0.7153451038*r + 0.3527967327*g - 0.0682510308*bl;
    let mc = 0.2744153967*r + 0.6678313229*g + 0.0577801949*bl;
    let sc = 0.1098335093*r + 0.1862950046*g + 0.7043511321*bl;
    let l_ = cbrt_signed(lc); let m_ = cbrt_signed(mc); let s_ = cbrt_signed(sc);
    let L = 0.2104542553*l_ + 0.7936177850*m_ - 0.0040720468*s_;
    let A = 1.9779984951*l_ - 2.4285922050*m_ + 0.4505937099*s_;
    let B = 0.0259040371*l_ + 0.7827717662*m_ - 0.8086757660*s_;

    let chroma = sqrt(A*A + B*B);
    // ⚠ The early-out is a BRANCH, matching the CPU's `return rgb`. Computing a near-zero
    // adjustment instead would give neutral pixels hue noise, which is what CHROMA_FADE exists
    // to prevent.
    if (chroma < 1e-6) { return; }

    var hue = degrees(atan2(B, A));
    if (hue < 0.0) { hue = hue + 360.0; }

    // Linear interpolation between bracketing table entries, wrapping at 360.
    let x = hue * (f32(params.table_len) / 360.0);
    let i0 = u32(floor(x)) % params.table_len;
    let i1 = (i0 + 1u) % params.table_len;
    let t = x - floor(x);
    var dh  = table[i0*3u]      + (table[i1*3u]      - table[i0*3u])      * t;
    var sat = table[i0*3u + 1u] + (table[i1*3u + 1u] - table[i0*3u + 1u]) * t;
    var lum = table[i0*3u + 2u] + (table[i1*3u + 2u] - table[i0*3u + 2u]) * t;

    let ct = min(chroma / 0.02, 1.0);
    let strength = ct * ct * (3.0 - 2.0 * ct);
    dh = dh * strength; sat = sat * strength; lum = lum * strength;

    // ⚠ The same identity the CPU uses: cos(hue) and sin(hue) are already in hand as A/chroma and
    // B/chroma, so the chroma cancels and the rotation costs two small-angle polynomials. A naive
    // atan2-then-cos round trip would be a DIFFERENT stage wearing the same name.
    let d = radians(dh * params.max_shift_deg / 100.0);
    let d2 = d * d;
    let sd = d * (1.0 - d2 * (1.0/6.0 - d2 * (1.0/120.0)));
    let cd = 1.0 - d2 * (0.5 - d2 * (1.0/24.0 - d2 * (1.0/720.0)));
    let k = max(1.0 + sat / 100.0, 0.0);
    let L2 = max(L * (1.0 + lum / 200.0), 0.0);
    let A2 = k * (A * cd - B * sd);
    let B2 = k * (A * sd + B * cd);

    // Oklab -> ProPhoto.
    let li = L2 + 0.3963377774*A2 + 0.2158037573*B2;
    let mi = L2 - 0.1055613458*A2 - 0.0638541728*B2;
    let si = L2 - 0.0894841775*A2 - 1.2914855480*B2;
    let l3 = li*li*li; let m3 = mi*mi*mi; let s3 = si*si*si;
    data[p]      = max(1.7386588588*l3 - 0.9880931444*m3 + 0.2495310446*s3, 0.0);
    data[p + 1u] = max(-0.7071491793*l3 + 1.9343303974*m3 - 0.2272015198*s3, 0.0);
    data[p + 2u] = max(-0.0840839768*l3 - 0.3575352425*m3 + 1.4409285251*s3, 0.0);
}
"#;

/// HSL, on the GPU.
pub struct GpuHslStage {
    table: Vec<f32>,
    cpu: pipeline::HslStage,
}

impl GpuHslStage {
    pub fn new(hsl: &edit_model::Hsl, _ctx: &Arc<GpuContext>) -> Self {
        let cpu = pipeline::HslStage::new(hsl);
        // ⚠ Built from the CPU stage's OWN table rather than rebuilt here, so the two cannot
        // disagree about band blending — the failure a second construction path would invite.
        let table = cpu.table().entries().iter().flatten().copied().collect();
        Self { table, cpu }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edit_model::{Hsl, HslBand};

    fn ctx() -> Option<Arc<GpuContext>> {
        match crate::shared() {
            Some(c) => Some(c.clone()),
            None => {
                eprintln!("SKIPPED: no GPU adapter on this machine");
                None
            }
        }
    }

    fn bands() -> Hsl {
        let mut h = Hsl::default();
        for (i, b) in h.bands.iter_mut().enumerate() {
            *b = HslBand {
                hue: (i as f32 * 13.0) % 70.0 - 35.0,
                saturation: (i as f32 * 21.0) % 90.0 - 45.0,
                luminance: (i as f32 * 9.0) % 40.0 - 20.0,
            };
        }
        h
    }

    /// ⚠⚠ Spans dark to above-white **and includes near-neutral pixels**, which is where the
    /// chroma early-out and the fade live and where a naive port goes wrong invisibly.
    fn colours(w: usize, h: usize) -> ImageBuf {
        let n = w * h;
        let mut d = Vec::with_capacity(n * 3);
        for i in 0..n {
            let t = (i % 719) as f32 / 719.0;
            match i % 5 {
                0 => d.extend_from_slice(&[t * 1.3, t * 0.4, t * 0.8]),
                1 => d.extend_from_slice(&[t * 0.2, t * 1.1, t * 0.5]),
                2 => d.extend_from_slice(&[t * 0.6, t * 0.6, t * 1.2]),
                // near-neutral: chroma just above and just below the early-out
                3 => d.extend_from_slice(&[t, t + 1e-7, t - 1e-7]),
                _ => d.extend_from_slice(&[t, t, t]),
            }
        }
        ImageBuf::from_vec(w, h, d)
    }

    #[test]
    fn the_gpu_hsl_matches_the_cpu_within_a_hundredth_of_a_level() {
        // ⚠ A tolerance rather than exactness, unlike exposure: this stage is transcendental. The
        // probe measured this exact maths at **0.0006 levels**, so 0.01 is ~16x headroom and
        // still ~100x tighter than anything visible.
        //
        // ⚠⚠ If this ever has to be WIDENED, that is a finding to investigate rather than a
        // number to adjust — a loose tolerance defeats the harness's whole purpose.
        let Some(ctx) = ctx() else { return };
        let hsl = bands();
        let img = colours(96, 72);
        let cpu = pipeline::HslStage::new(&hsl).apply(img.clone());
        let gpu = crate::GpuChain::new(ctx.clone(), vec![Box::new(GpuHslStage::new(&hsl, &ctx))])
            .apply(img);
        let d = crate::compare(&cpu, &gpu);
        assert!(
            d.worst_levels < 0.01,
            "worst {} levels over {} of {} samples",
            d.worst_levels,
            d.differing,
            d.total
        );
    }

    #[test]
    fn an_exactly_neutral_pixel_is_returned_untouched() {
        // ⚠ Asserted EXACTLY, not within the tolerance: the early-out is a branch, so it either
        // fires or it does not. A near-miss here means the branch is wrong, not that floats drift.
        let Some(ctx) = ctx() else { return };
        let img = ImageBuf::from_vec(2, 1, vec![0.4, 0.4, 0.4, 0.0, 0.0, 0.0]);
        let out = crate::GpuChain::new(
            ctx.clone(),
            vec![Box::new(GpuHslStage::new(&bands(), &ctx))],
        )
        .apply(img.clone());
        assert_eq!(out.data(), img.data(), "a neutral pixel has no hue to shift");
    }

    #[test]
    fn a_negative_sample_does_not_become_nan() {
        // ⚠⚠ The `cbrt` trap. WGSL's `pow` returns NaN for a negative base, and a wide working
        // space produces scene-referred negatives routinely. A naive port turns them into NaN,
        // which then propagates through the output matrix and paints black holes.
        let Some(ctx) = ctx() else { return };
        let img = ImageBuf::from_vec(1, 1, vec![-0.05, 0.3, 0.2]);
        let out = crate::GpuChain::new(
            ctx.clone(),
            vec![Box::new(GpuHslStage::new(&bands(), &ctx))],
        )
        .apply(img);
        assert!(
            out.data().iter().all(|v| v.is_finite()),
            "a negative input must stay finite, got {:?}",
            out.data()
        );
    }
}
```

⚠ `GpuHslStage` also needs a `GpuStage` impl encoding its dispatch with three bindings — the data
buffer, the params uniform, and the table storage buffer — following exactly the shape
`GpuExposureStage::encode` uses, plus `fn cpu(&self) -> &dyn PipelineStage { &self.cpu }`.

- [ ] **Step 3: Expose `HslStage::table`**

`GpuHslStage::new` reads the CPU stage's table. In `crates/pipeline/src/stages/hsl.rs`, add:

```rust
    /// The hue table this stage samples.
    ///
    /// ⚠ Exposed so a GPU twin uploads the **same** table rather than rebuilding it — two
    /// constructions of one table is how the two would come to disagree about band blending.
    pub fn table(&self) -> &HueTable {
        &self.table
    }
```

- [ ] **Step 4: Run and sabotage**

```bash
cargo test --release -p gpu-pipeline 2>&1 | grep -E "^test |test result" | tail -20
```

Then sabotage, restoring from a `cp` backup each time: replace `cbrt_signed` with a bare
`pow(x, 0.33333333)` (must fail `a_negative_sample_does_not_become_nan`); remove the
`chroma < 1e-6` early-out (must fail `an_exactly_neutral_pixel_is_returned_untouched`); and replace
the small-angle rotation with `cos(hue + d)`/`sin(hue + d)` computed from `atan2` (must fail the
tolerance test).

- [ ] **Step 5: Gate and commit**

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings > /tmp/m83b-cl.log 2>&1; echo "CLIPPY=$?"
git add -A
git commit -m "feat: HSL as a compute shader

The band logic never enters the shader: HueTable::build already collapses
eight bands and their blending into a 1024-entry table, built once on the
CPU and uploaded as 12 KB. Built from the CPU stage's OWN table so the
two cannot disagree.

⚠ sign(x) * pow(abs(x), 1/3), never pow(x, 1/3): WGSL's pow returns NaN
for a negative base, and the CPU uses cbrt specifically because it is
sign-preserving -- a wide working space produces scene-referred negatives
routinely. Its own test.

Equivalence is a tolerance, not exactness, because this stage is
transcendental: asserted below 0.01 of an 8-bit level against a measured
0.0006. Widening that is a finding, not an adjustment."
```

---

## Task 4: `accelerate`, and its two guards

**Files:**
- Create: `crates/gpu-pipeline/src/accelerate.rs`
- Modify: `crates/gpu-pipeline/src/lib.rs`

- [ ] **Step 1: Write it with its tests**

```rust
//! Swapping CPU stages for GPU ones in a built pipeline.

use crate::{GpuChain, GpuContext, GpuHslStage, GpuStage};
use edit_model::Edits;
use pipeline::Pipeline;
use std::sync::Arc;

/// The name of the one stage this can replace.
///
/// ⚠⚠ **Name-matching is stringly-typed and that is the cost of needing no change to
/// `pipeline`.** A `dyn PipelineStage` exposes only `name()` and `apply()`, so the parameters
/// needed to build a GPU equivalent are gone — but they do not have to be recovered from the
/// stage, because the caller still holds the `Edits`.
///
/// ⚠ It does **not** scale to a dozen stages. When several are accelerated, the right answer is a
/// `StageAccelerator` hook that `pipeline` calls at construction; it loses today only because
/// `RenderSpec` is `Copy + PartialEq` and an `Arc<dyn Trait>` is neither.
const HSL: &str = "hsl";

/// Replace GPU-capable stages in `tail` with their GPU twins.
///
/// ⚠ Only the **global** tail. A mask's own HSL lives inside `MaskStage`, which this sees as one
/// opaque stage — so masked HSL is not accelerated, and that is the case reported in use as slow.
pub fn accelerate(tail: Pipeline, edits: &Edits, ctx: &Arc<GpuContext>) -> Pipeline {
    Pipeline::new(
        tail.into_stages()
            .into_iter()
            .map(|s| {
                if s.name() == HSL {
                    let gpu: Box<dyn GpuStage> = Box::new(GpuHslStage::new(&edits.hsl, ctx));
                    Box::new(GpuChain::new(ctx.clone(), vec![gpu])) as _
                } else {
                    s
                }
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use edit_model::HslBand;

    fn ctx() -> Option<Arc<GpuContext>> {
        match crate::shared() {
            Some(c) => Some(c.clone()),
            None => {
                eprintln!("SKIPPED: no GPU adapter on this machine");
                None
            }
        }
    }

    fn with_hsl() -> Edits {
        let mut e = Edits::default();
        e.hsl.bands[0] = HslBand { hue: 20.0, saturation: 10.0, luminance: 5.0 };
        e
    }

    #[test]
    fn the_tail_contains_at_most_one_stage_named_hsl() {
        // ⚠⚠ This is what makes name-matching sound, and it is a real property rather than an
        // assumption: a mask's own HSL lives INSIDE MaskStage, which the tail sees as a single
        // stage named "mask". A future refactor that flattened masks into the tail would break
        // this silently, so it is asserted with masks present.
        let mut e = with_hsl();
        e.masks = vec![edit_model::MaskedAdjustment {
            mask: edit_model::Mask::Linear(edit_model::LinearGradient::default()),
            refine: Vec::new(),
            adjustments: edit_model::LocalAdjustments {
                hsl: with_hsl().hsl,
                ..Default::default()
            },
        }];
        let (_, tail) = pipeline::build_pipeline_split(&e, pipeline::RenderScale::new(1.0));
        let n = tail.stage_names().iter().filter(|x| **x == HSL).count();
        assert_eq!(n, 1, "got {:?}", tail.stage_names());
    }

    #[test]
    fn accelerating_replaces_the_hsl_stage_and_nothing_else() {
        let Some(ctx) = ctx() else { return };
        let e = with_hsl();
        let (_, tail) = pipeline::build_pipeline_split(&e, pipeline::RenderScale::new(1.0));
        let before = tail.stage_names();
        let after = accelerate(tail, &e, &ctx).stage_names();
        assert_eq!(before.len(), after.len(), "no stage may be added or dropped");
        for (b, a) in before.iter().zip(&after) {
            if *b == HSL {
                assert_eq!(*a, "gpu_chain", "the hsl stage must become a chain");
            } else {
                assert_eq!(b, a, "every other stage must be untouched");
            }
        }
    }

    #[test]
    fn a_tail_with_no_hsl_is_returned_unchanged() {
        // ⚠ The other half. Without it, a substitution that replaced the FIRST stage rather than
        // the named one would still pass the test above.
        let Some(ctx) = ctx() else { return };
        let e = Edits { exposure: 0.5, ..Edits::default() };
        let (_, tail) = pipeline::build_pipeline_split(&e, pipeline::RenderScale::new(1.0));
        let before = tail.stage_names();
        assert!(!before.contains(&HSL), "fixture check: this tail has no hsl");
        assert_eq!(accelerate(tail, &e, &ctx).stage_names(), before);
    }

    #[test]
    fn an_accelerated_tail_renders_the_same_picture() {
        // ⚠⚠ The carrier: everything above can pass while the substituted stage computes
        // nonsense. This runs both tails over the same buffer and compares.
        let Some(ctx) = ctx() else { return };
        let e = with_hsl();
        let scale = pipeline::RenderScale::new(1.0);
        let img = core_image::ImageBuf::from_vec(
            64,
            48,
            (0..64 * 48 * 3).map(|i| (i % 601) as f32 / 601.0).collect(),
        );
        let (_, cpu_tail) = pipeline::build_pipeline_split(&e, scale);
        let (_, gpu_tail) = pipeline::build_pipeline_split(&e, scale);
        let cpu = cpu_tail.run(img.clone());
        let gpu = accelerate(gpu_tail, &e, &ctx).run(img);
        let d = crate::compare(&cpu, &gpu);
        assert!(d.worst_levels < 0.01, "worst {} levels", d.worst_levels);
    }
}
```

- [ ] **Step 2: Export and run**

`lib.rs`: `mod accelerate;` and `pub use accelerate::accelerate;`

```bash
cargo test --release -p gpu-pipeline 2>&1 | grep -E "^test |test result" | tail -22
```

- [ ] **Step 3: Sabotage**

Replace the first stage rather than the named one (`if true`), which must fail
`a_tail_with_no_hsl_is_returned_unchanged`. Restore from a `cp` backup.

- [ ] **Step 4: Gate and commit**

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings > /tmp/m83b-cl.log 2>&1; echo "CLIPPY=$?"
git add -A
git commit -m "feat: accelerate() swaps the tail's hsl stage for its GPU twin

Needs no change to pipeline: Pipeline::into_stages already exists and
the caller holds the Edits, so the stage is rebuilt rather than
inspected.

⚠ Name-matching, which is the cost. Two guards: the tail contains at
most one stage named hsl -- true because a mask's own HSL lives inside
MaskStage -- and a tail without HSL comes back unchanged, without which
replacing the FIRST stage would still pass.

Only the global tail. Masked HSL is not accelerated."
```

---

## Task 5: The Settings toggle and the app

**Files:**
- Modify: `crates/app/Cargo.toml`, `crates/app/src/settings.rs`,
  `crates/app/src/modals/settings.rs`, `crates/app/src/render_engine.rs`

- [ ] **Step 1: Depend on `gpu-pipeline`**

Add to `crates/app/Cargo.toml` under `[dependencies]`:

```toml
gpu-pipeline = { path = "../gpu-pipeline" }
```

- [ ] **Step 2: The setting**

In `crates/app/src/settings.rs`, add to `Settings`:

```rust
    /// Run the global HSL stage on the GPU.
    ///
    /// ⚠⚠ **Defaults to `false`, and that is deliberate.** This is the first GPU code any user
    /// would run; the CPU path is the oracle, and a wrong default would make a rendering
    /// difference look like a Rustroom bug rather than a setting.
    ///
    /// ⚠ **Global HSL only.** A mask's own HSL lives inside `MaskStage`, which `accelerate`
    /// cannot reach — and masked HSL is the case reported in use as slow.
    #[serde(default)]
    pub gpu_hsl: bool,
```

⚠ A **field-level** `#[serde(default)]` here is safe and gives `false`, which is also the intended
default — unlike `auto_advance`, where the field default and the intended default differed and a
field-level attribute would have shipped the feature disabled.

Add `gpu_hsl: false` to the `Default` impl and to `settings.rs`'s exhaustive `Settings { .. }`
literal in its tests; the compiler names both.

- [ ] **Step 3: The checkbox**

In `crates/app/src/modals/settings.rs`, beside the 1:1 view quality row:

```rust
            ui.separator();
            let mut gpu_hsl = app.settings.gpu_hsl;
            ui.checkbox(&mut gpu_hsl, "Use the GPU for HSL");
            theme::hint(
                ui,
                "Runs the global HSL sliders on the graphics card. Off by default: \
                 the CPU is the reference, and the two agree to about a hundredth of \
                 an 8-bit level. Does not apply to HSL inside a mask.",
            );
```

and, with the other change-guarded writes at the end of the function:

```rust
    // ⚠ Guarded like its neighbours: `show` runs every frame the dialog is open, and this one
    // re-renders — the `apply_view_demosaic` rule.
    if gpu_hsl != app.settings.gpu_hsl {
        app.settings.gpu_hsl = gpu_hsl;
        app.save_settings();
        app.request_rerender();
    }
```

⚠ If `RustroomApp` has no `request_rerender`, use whatever the neighbouring settings use to force
a fresh render — `apply_resampler`'s body is the model. A toggle whose effect appears only after an
unrelated edit reads as broken.

- [ ] **Step 4: Carry the setting to the renderer, following `resampler` exactly**

⚠⚠ **Per request, not captured at `RenderEngine::new`** — the distinction `resampler`'s own doc
comment at `render_engine.rs:128` spells out: *"a lens profile cannot vary within one engine's
life… the resampler is a global setting the user can change with a photo open."* `gpu_hsl` is the
same kind of thing, so it rides the same route and a toggle takes effect on the next render with
no engine rebuild.

Add the field beside `resampler` on the same request struct (`render_engine.rs:134`):

```rust
    /// Run the global HSL stage on the GPU.
    ///
    /// ⚠ Per **request**, for `resampler`'s reason: a global setting the user can change with a
    /// photo open. ⚠ Unlike the resampler it is in **no** `PrefixStepKey`, and must not be —
    /// HSL is a tail stage, so the prefix computes the same thing either way and a stale
    /// checkpoint is still valid.
    pub gpu_hsl: bool,
```

⚠ Add `gpu_hsl: false` to the struct's `Default` (beside `resampler` at line ~184) and to the
call sites at ~991, ~1164 and ~1221 that thread `resampler`; the compiler names every one.

- [ ] **Step 5: Apply the accelerator inside `build_tail`**

⚠ **`build_tail` is the right site**, at `render_engine.rs:1097` — it is a closure that already
holds `edits`, and it is called for **both** the display tail and the proofed one, so accelerating
inside it covers both with one edit. Change its final `tail` to:

```rust
        // ⚠⚠ The tail only. HSL is a tail stage, and accelerating the prefix would make
        // `PrefixKey` describe two different computations under one key.
        //
        // ⚠ `shared()` returning `None` silently keeps the CPU path: a machine with no adapter
        // must not fail to render.
        match (gpu_hsl, gpu_pipeline::shared()) {
            (true, Some(ctx)) => gpu_pipeline::accelerate(tail, edits, ctx),
            _ => tail,
        }
    };
```

- [ ] **Step 6: Gate**

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings > /tmp/m83b-cl.log 2>&1; echo "CLIPPY=$?"
cargo test --workspace > /tmp/m83b-ws.log 2>&1; echo "TEST=$?"
grep -o "test result: ok\. [0-9]* passed" /tmp/m83b-ws.log | awk '{s+=$4} END {print "TOTAL:", s}'
echo "goldens run: $(grep -c '^test golden_' /tmp/m83b-ws.log)"; git status --short crates/pipeline/tests/
echo "GPU tests skipped: $(grep -c SKIPPED /tmp/m83b-ws.log)"
```
Expected: `CLIPPY=0`, `TEST=0`, 63 goldens run, **none moved**, 0 skipped.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat: a Settings toggle for GPU HSL, applied to the tail only

Off by default: this is the first GPU code any user would run, the CPU
path is the oracle, and a wrong default would make a rendering
difference look like a bug rather than a setting.

The tail only -- accelerating the prefix would make PrefixKey describe
two different computations under one key. A missing adapter silently
keeps the CPU path."
```

---

## Task 6: Measure what it actually saves

**Files:**
- Create: `crates/gpu-pipeline/examples/gpu_hsl_bench.rs`

⚠⚠ The spec deliberately predicts **no** number here. M8.2's spec had to be corrected because a
projection was quoted as an achieved figure.

- [ ] **Step 1: Write the bench**

```rust
//! What does running HSL on the GPU actually save?
//!
//! ⚠ A chain of one stage still pays the full round trip, so the saving is a fraction of HSL's
//! CPU cost rather than all of it. This reports the real figure.
//!
//! `cargo run --release -p gpu-pipeline --example gpu_hsl_bench [width] [height]`

use edit_model::{Hsl, HslBand};
use gpu_pipeline::{compare, GpuChain, GpuHslStage};
use pipeline::PipelineStage;
use std::time::Instant;

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

fn main() {
    let mut a = std::env::args().skip(1);
    let w: usize = a.next().and_then(|s| s.parse().ok()).unwrap_or(1600);
    let h: usize = a.next().and_then(|s| s.parse().ok()).unwrap_or(1068);
    let Some(ctx) = gpu_pipeline::shared() else {
        println!("no usable GPU adapter — the CPU path is what would run");
        return;
    };
    println!("adapter: {} ({})", ctx.adapter_name(), ctx.backend());

    let mut hsl = Hsl::default();
    for (i, b) in hsl.bands.iter_mut().enumerate() {
        *b = HslBand {
            hue: (i as f32 * 13.0) % 70.0 - 35.0,
            saturation: (i as f32 * 21.0) % 90.0 - 45.0,
            luminance: (i as f32 * 9.0) % 40.0 - 20.0,
        };
    }
    let n = w * h * 3;
    let img = core_image::ImageBuf::from_vec(
        w,
        h,
        (0..n).map(|i| (i % 719) as f32 / 719.0 * 1.2).collect(),
    );
    println!("buffer {w}x{h}x3 f32 = {:.1} MB\n", (n * 4) as f64 / 1048576.0);

    let cpu_stage = pipeline::HslStage::new(&hsl);
    let gpu_chain = GpuChain::new(ctx.clone(), vec![Box::new(GpuHslStage::new(&hsl, ctx))]);

    let mut last = None;
    println!("  round      cpu         gpu       ratio");
    for r in 0..5 {
        let t = Instant::now();
        let cpu = cpu_stage.apply(img.clone());
        let c = ms(t);
        let t = Instant::now();
        let gpu = gpu_chain.apply(img.clone());
        let g = ms(t);
        println!("  {r}      {c:7.2} ms  {g:7.2} ms   {:.2}x", c / g);
        last = Some((cpu, gpu));
    }
    let (cpu, gpu) = last.expect("five rounds ran");
    let d = compare(&cpu, &gpu);
    println!(
        "\n  divergence: worst {:.5} levels, {} of {} samples ({:.3}%)",
        d.worst_levels, d.differing, d.total, d.differing_percent()
    );
}
```

- [ ] **Step 2: Run and record the numbers**

```bash
cargo run --release -p gpu-pipeline --example gpu_hsl_bench 2>&1 | tail -12
```

⚠ If the GPU is **slower**, that is a finding to report, not to hide — it would mean the round trip
outweighs even this stage, and the honest response is to say so and reconsider Task 5's default.

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "test: measure what GPU HSL actually saves"
```

---

## Task 7: Documentation and the GUI check

**Files:**
- Modify: `CLAUDE.md`, `docs/BACKLOG.md`

- [ ] **Step 1: Add the M8.3b section to `CLAUDE.md`**

Insert above the `**M8.3a — the GPU seam and the proof` section, in house style. It must record:

- the **measured** CPU-vs-GPU figures from Task 6 and the divergence;
- **the barrier result** — whether one compute pass suffices, which is a fact about WebGPU worth
  knowing once;
- that `GpuStage` is deliberately not `PipelineStage`, and why the two cannot be one;
- the `sign(x) * pow(abs(x), 1/3)` trap and that WGSL's `pow` returns NaN for a negative base;
- that substitution is **name-matching**, its two guards, and that it does not scale;
- ⚠ **that masked HSL is NOT accelerated**, and that this is the case reported in use as slow;
- the accepted caveats from spec §9.

- [ ] **Step 2: Update the `▶ NEXT ACTION` block and the handoff**

Mark M8.3b done; point at **M8.3c** (spatial stages, needing the ping-pong buffer) and note that
the substitution mechanism wants rethinking before many more stages are accelerated. Add a new
`State at handoff` paragraph, demote the old one, and say whether it is confirmed by use.

- [ ] **Step 3: Update `docs/BACKLOG.md`** with M8.3b shipped and the measured numbers.

- [ ] **Step 4: Full gate**

```bash
cargo fmt --all --check > /tmp/m83b-fmt.log 2>&1; echo "FMT=$?"
cargo clippy --all-targets --all-features -- -D warnings > /tmp/m83b-cl.log 2>&1; echo "CLIPPY=$?"
cargo test --workspace > /tmp/m83b-final.log 2>&1; echo "TEST=$?"
grep -o "test result: ok\. [0-9]* passed" /tmp/m83b-final.log | awk '{s+=$4} END {print "TOTAL:", s}'
echo "goldens run: $(grep -c '^test golden_' /tmp/m83b-final.log)"; git status --short crates/pipeline/tests/
echo "GPU tests skipped: $(grep -c SKIPPED /tmp/m83b-final.log)"
```

- [ ] **Step 5: Commit and launch the GUI**

```bash
git add -A && git commit -m "docs: record M8.3b, the resident GPU chain and HSL"
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo build --release -p app
nohup ./target/release/Rustroom > /tmp/m83b-gui.log 2>&1 &
sleep 8
pgrep -f "target/release/Rustroom" > /dev/null && echo "ALIVE" || cat /tmp/m83b-gui.log
```

**Unlike M8.3a, there IS something to look at.** Tell the user:

1. **Settings → Use the GPU for HSL**, off by default.
2. Open a photo, go to the HSL bands, and drag a saturation slider with it **off**, then **on**.
   The difference should be in responsiveness, not in the picture.
3. ⚠ **The picture must look the same.** The two paths agree to ~0.01 of an 8-bit level, so any
   visible difference is a bug worth reporting immediately.
4. ⚠ **Masked HSL is unaffected** — the toggle does nothing for HSL inside a mask, which is the
   case previously reported as slow.

---

## Self-Review

**Spec coverage.** §1 measurements → Task 6. §2 chain-as-`PipelineStage` → Task 2. §3 `GpuImage`
→ Task 1, `GpuStage` with `cpu()` → Task 2, `GpuChain` → Task 2. §4 barrier test → Task 2 Step 1,
with the fallback at Step 6. §5 substitution and both guards → Task 4. §6 `GpuHslStage` → Task 3.
§7 the app → Task 5. §8 all ten tests → Tasks 1–4. §9 caveats → Task 7 Step 1.

**Placeholder scan.** No "TBD", no "add error handling", no "similar to Task N". Task 7 Step 1
describes a section rather than quoting it, deliberately: it must carry numbers that do not exist
until Task 6 runs.

**Type consistency.** `GpuImage::{new, width, height, pixels, bytes}` with `pub(crate) buffer`;
`GpuStage::{name, encode, cpu}`; `GpuChain::{new, len, is_empty, cpu_fallback, run}`;
`transfer_counts`; `GpuHslStage::new(&Hsl, &Arc<GpuContext>)`; `accelerate(Pipeline, &Edits,
&Arc<GpuContext>)`; `HueTable::entries`; `HslStage::table`; `Settings::gpu_hsl`. Each defined once
and spelled identically after.

✅ **The app-side assumptions were CHECKED rather than assumed**, because M8.2's plan asserted a
`Cargo.toml` fact that was wrong and broke the lib build while the tests still compiled. Verified
while writing this: `Pipeline::{new, into_stages, stage_names, run}` all exist and are `pub`;
`build_tail` at `render_engine.rs:1097` is a closure holding `edits`, and is called for both the
display and proofed tails; and `resampler` at `render_engine.rs:134` is the established pattern
for a global setting reaching a render per request.

⚠ **The one assumption that remains:** `GpuExposureStage`'s `run` is refactorable into a
`pipeline_and_bind` helper without changing behaviour. Its existing bit-exactness test is the
guard. **If the split proves awkward, do not force it** — keep the transfer in `GpuChain` and let
`encode` build what it needs inline; the duplication is a few lines and is preferable to a
contorted helper.
