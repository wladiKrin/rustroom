# M8.3c Spatial Stages on the GPU Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this
> plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> ⚠⚠ **PROJECT CONVENTIONS, NOT NEGOTIABLE.** Commit messages carry **NO `Co-Authored-By`
> trailer**. **NEVER push.** Branch `m8.3c-spatial-gpu` already exists.
>
> ⚠ Before any cargo command:
> ```
> export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
> ```
>
> ⚠⚠ **Iterate with `cargo test --release -p gpu-pipeline`** — a debug build pays ~42 s once for
> the adapter.
>
> ⚠ Read every gate's exit code **from a file, never a pipe**, and use a **fresh log name each
> time**: M8.3b wasted two cycles because a wait condition matched the *previous* run's content in
> a reused file. After every scripted edit assert the new text landed; after every sabotage confirm
> the **named** test failed.
>
> ⚠ **Restore a sabotage from a `cp` backup, never `git checkout -- <file>`** — that has destroyed
> uncommitted work in this repo twice.

**Goal:** A GPU chain can run stages needing scratch memory, consecutive GPU stages share one round
trip, and `SharpenStage` runs on the GPU.

**Architecture:** `GpuImage` gains shared scratch planes that are transient *within* a stage, so the
chain's every-stage-reads-and-writes-`front` contract is unchanged. `accelerate` groups consecutive
replaceable stages into one `GpuChain` via a name→builder table. `GpuSharpenStage` uploads the CPU's
own Gaussian kernel.

**Tech Stack:** `wgpu` 29, WGSL.

**Spec:** `docs/superpowers/specs/2026-09-10-spatial-gpu-design.md`

⚠⚠ **Sharpening alone is break-even (+7.1 ms against a ~6 ms round trip). The win is the RUN** —
`hsl` and `sharpen` sharing one transfer. Do not judge this slice by sharpening's own number.

---

## File Structure

| | |
|---|---|
| **Modify** `crates/gpu-pipeline/src/image.rs` | `GpuImage` gains scratch planes. |
| **Modify** `crates/gpu-pipeline/src/chain.rs` | `GpuStage::scratch_planes`, `encode` takes a scratch slice, the chain allocates the max once. |
| **Modify** `crates/gpu-pipeline/src/exposure.rs`, `hsl.rs` | The new `encode` signature; both declare 0 planes. |
| **Create** `crates/gpu-pipeline/src/sharpen.rs` | `GpuSharpenStage` and its WGSL. |
| **Modify** `crates/gpu-pipeline/src/accelerate.rs` | The table and run-merging. |
| **Modify** `crates/pipeline/src/spatial.rs` | `gaussian_kernel` becomes `pub`. |
| **Modify** `crates/app/…` | `gpu_hsl` → `gpu_tail`, with a serde alias. |

---

## Task 1: Scratch planes

**Files:**
- Modify: `crates/gpu-pipeline/src/image.rs`
- Modify: `crates/gpu-pipeline/src/chain.rs`
- Modify: `crates/gpu-pipeline/src/exposure.rs`, `crates/gpu-pipeline/src/hsl.rs`

- [ ] **Step 1: Write the failing tests**

Append to `crates/gpu-pipeline/src/chain.rs`'s `tests` module:

```rust
    /// A stage that asks for scratch and writes a marker into it, so the test can prove the
    /// planes exist and are the right size.
    struct NeedsScratch(usize);

    impl GpuStage for NeedsScratch {
        fn name(&self) -> &'static str {
            "needs_scratch"
        }
        fn scratch_planes(&self) -> usize {
            self.0
        }
        fn encode(
            &self,
            _ctx: &GpuContext,
            _pass: &mut wgpu::ComputePass,
            _img: &GpuImage,
            scratch: &[wgpu::Buffer],
        ) {
            assert_eq!(scratch.len(), self.0, "the chain must lend exactly what was asked for");
        }
        fn cpu(&self) -> &dyn PipelineStage {
            &self.noop
        }
    }

    #[test]
    fn a_chain_allocates_the_widest_scratch_request_once() {
        // ⚠⚠ The **max**, not the sum: the planes are transient WITHIN a stage, so two stages
        // asking for two each can share the same two. Summing would double the memory for
        // nothing, and a test asserting only "at least two" could not tell the two apart.
        let Some(ctx) = ctx() else { return };
        let chain = GpuChain::new(
            ctx,
            vec![Box::new(NeedsScratch::new(1)), Box::new(NeedsScratch::new(3))],
        );
        assert_eq!(chain.scratch_needed(), 3);
    }

    #[test]
    fn a_chain_of_in_place_stages_allocates_no_scratch() {
        // ⚠ So an HSL-only chain costs exactly what it did in M8.3b. Without this, allocating
        // unconditionally would pass every other test while wasting ~26 MB on every render.
        let Some(ctx) = ctx() else { return };
        let chain = GpuChain::new(
            ctx.clone(),
            vec![Box::new(GpuExposureStage::new(1.0, ctx))],
        );
        assert_eq!(chain.scratch_needed(), 0);
    }
```

⚠ `NeedsScratch` needs a `noop: pipeline::ExposureStage` field so `cpu()` has something to return;
give it `NeedsScratch::new(n)` building `Self { n, noop: pipeline::ExposureStage::new(0.0) }` and
change the tuple struct to a named one.

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test --release -p gpu-pipeline 2>&1 | grep -E "^error|cannot find" | head -5
```
Expected: `no method named 'scratch_planes'` / `no method named 'scratch_needed'`.

- [ ] **Step 3: Add scratch to `GpuImage`**

In `crates/gpu-pipeline/src/image.rs`, add to `impl GpuImage`:

```rust
    /// Allocate `n` single-channel planes of `width × height` floats.
    ///
    /// ⚠⚠ **Transient within a stage, and SHARED between the stages of a chain.** A separable
    /// blur goes `front → scratch → front`, so the result lands back where it started and no
    /// stage may assume a plane still holds what it wrote. That is what keeps the chain's
    /// contract — every stage reads and writes `front` — unchanged from M8.3b.
    ///
    /// ⚠ Single-channel, not RGB: every consumer here works on a luma plane, so allocating three
    /// channels would triple the memory for nothing.
    pub(crate) fn scratch(ctx: &GpuContext, width: usize, height: usize, n: usize) -> Vec<wgpu::Buffer> {
        let bytes = (width * height * 4) as u64;
        (0..n)
            .map(|_| {
                ctx.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("gpu-scratch"),
                    size: bytes.max(4),
                    usage: wgpu::BufferUsages::STORAGE,
                    mapped_at_creation: false,
                })
            })
            .collect()
    }
```

⚠ `size` is `.max(4)` because **wgpu refuses a zero-sized buffer**, and a chain asked for scratch
on a degenerate image would otherwise fail at allocation rather than render nothing.

- [ ] **Step 4: Extend `GpuStage` and `GpuChain`**

In `crates/gpu-pipeline/src/chain.rs`, change the trait:

```rust
    /// How many single-channel scratch planes this stage needs.
    ///
    /// ⚠ Defaults to **0**, so every stage written before M8.3c keeps working untouched.
    fn scratch_planes(&self) -> usize {
        0
    }

    /// Record this stage's dispatch. The image is bound at group 0, binding 0.
    ///
    /// ⚠ `scratch` holds exactly [`Self::scratch_planes`] buffers — a slice rather than a count
    /// and an index, so a stage cannot reach past what it asked for.
    fn encode(
        &self,
        ctx: &GpuContext,
        pass: &mut wgpu::ComputePass,
        img: &GpuImage,
        scratch: &[wgpu::Buffer],
    );
```

Add to `impl GpuChain`:

```rust
    /// The widest scratch request among this chain's stages.
    ///
    /// ⚠ The **max**, not the sum — the planes are transient within a stage, so stages share them.
    pub fn scratch_needed(&self) -> usize {
        self.stages.iter().map(|s| s.scratch_planes()).max().unwrap_or(0)
    }
```

And in `run`, before the compute pass:

```rust
        // ⚠ Allocated once for the whole chain and only when something asks — an HSL-only chain
        // allocates nothing, exactly as it did in M8.3b.
        let scratch = GpuImage::scratch(ctx, image.width(), image.height(), self.scratch_needed());
```

then pass `&scratch` to each `s.encode(ctx, &mut pass, &img, &scratch)`.

- [ ] **Step 5: Update the two existing stages**

`GpuExposureStage::encode` and `GpuHslStage::encode` each gain a `_scratch: &[wgpu::Buffer]`
parameter and declare nothing — the default `scratch_planes` of 0 already covers them.

- [ ] **Step 6: Run**

```bash
cargo test --release -p gpu-pipeline 2>&1 | grep -E "test result|FAILED" | head -3
```
Expected: all pass, count up by 2.

- [ ] **Step 7: Sabotage**

```bash
cp crates/gpu-pipeline/src/chain.rs /tmp/m83c-chain.bak
python3 - <<'PY'
p='crates/gpu-pipeline/src/chain.rs'; s=open(p).read()
o=".map(|s| s.scratch_planes()).max().unwrap_or(0)"
assert o in s, "sabotage did not match"
open(p,'w').write(s.replace(o, ".map(|s| s.scratch_planes()).sum()"))
PY
cargo test --release -p gpu-pipeline > /tmp/m83c-sab1.log 2>&1
grep -q "test result:" /tmp/m83c-sab1.log && grep -E "FAILED" /tmp/m83c-sab1.log | head -2 || echo "  DID NOT COMPILE"
cp /tmp/m83c-chain.bak crates/gpu-pipeline/src/chain.rs
```
Expected: `a_chain_allocates_the_widest_scratch_request_once ... FAILED` (sum gives 4, max gives 3).

- [ ] **Step 8: Gate and commit**

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings > /tmp/m83c-cl1.log 2>&1; echo "CLIPPY=$?"
cargo test --release -p gpu-pipeline > /tmp/m83c-t1.log 2>&1; echo "TEST=$?"
```
⚠ **Both must be 0 before committing.** M8.3a and M8.3b each committed once with clippy failing.

```bash
git add -A
git commit -m "feat: shared scratch planes for spatial GPU stages

Not chain-level ping-pong: a separable blur goes front -> scratch ->
front, so the result lands where it started and every stage still reads
and writes front. The chain's contract from M8.3b is unchanged.

The MAX of the stages' requests, not the sum -- planes are transient
within a stage, so stages share them. And nothing is allocated when
nothing asks, so an HSL-only chain costs what it did before.

scratch_planes defaults to 0, so every existing stage is untouched."
```

---

## Task 2: Run-merging and the substitution table

> ⚠⚠ **EXECUTED AFTER TASK 3, and the plan had the order wrong.** These tests need **two adjacent
> replaceable stages**, and until `GpuSharpenStage` exists the only replaceable stage is `hsl` —
> of which a tail contains exactly one, asserted by M8.3b's own
> `the_tail_contains_at_most_one_stage_named_hsl`. So run-merging is untestable before the second
> stage exists. Task 3 depends on nothing but Task 1's scratch planes and was done first.

**Files:**
- Modify: `crates/gpu-pipeline/src/accelerate.rs`

- [ ] **Step 1: Write the failing tests**

Append to `crates/gpu-pipeline/src/accelerate.rs`'s `tests` module:

```rust
    /// Names of the stages a tail is built from, for readable assertions.
    fn names(p: &Pipeline) -> Vec<&'static str> {
        p.stage_names()
    }

    #[test]
    fn consecutive_gpu_stages_merge_into_one_chain() {
        // With colour grading neutral, `hsl` and `sharpen` are adjacent in the tail — which is
        // the whole reason this slice ports sharpening rather than colour grading.
        let Some(ctx) = ctx() else { return };
        let mut e = with_hsl();
        e.sharpening = edit_model::Sharpening {
            amount: 60.0,
            ..Default::default()
        };
        let (_, tail) = pipeline::build_pipeline_split(&e, pipeline::RenderScale::new(1.0));
        let before = names(&tail);
        assert!(
            before.windows(2).any(|w| w == ["hsl", "sharpen"]),
            "fixture check: hsl and sharpen must be adjacent, got {before:?}"
        );
        let after = names(&accelerate(tail, &e, &ctx));
        assert_eq!(
            after.iter().filter(|n| **n == "gpu_chain").count(),
            1,
            "two adjacent GPU stages must become ONE chain, got {after:?}"
        );
        assert_eq!(after.len(), before.len() - 1, "the pair collapses to one stage");
    }

    #[test]
    fn a_cpu_stage_between_two_gpu_stages_produces_two_chains() {
        // ⚠⚠ **The half that makes the test above mean something.** An implementation that
        // merged every replaceable stage regardless of position would pass it — and would run
        // the CPU stage's work in the wrong order, corrupting the render.
        let Some(ctx) = ctx() else { return };
        let mut e = with_hsl();
        e.sharpening = edit_model::Sharpening {
            amount: 60.0,
            ..Default::default()
        };
        // Colour grading sits between hsl and sharpen and is NOT replaceable.
        e.color_grading.shadows.saturation = 30.0;
        let (_, tail) = pipeline::build_pipeline_split(&e, pipeline::RenderScale::new(1.0));
        let before = names(&tail);
        assert!(
            before.contains(&"color_grading"),
            "fixture check: the grade must be active, got {before:?}"
        );
        let after = names(&accelerate(tail, &e, &ctx));
        assert_eq!(
            after.iter().filter(|n| **n == "gpu_chain").count(),
            2,
            "a CPU stage between them must split the run, got {after:?}"
        );
    }

    #[test]
    fn merging_preserves_order() {
        // ⚠ A regrouping, never a reordering. Every non-replaceable stage must keep its position
        // relative to the chains around it.
        let Some(ctx) = ctx() else { return };
        let mut e = with_hsl();
        e.sharpening = edit_model::Sharpening {
            amount: 60.0,
            ..Default::default()
        };
        e.color_grading.shadows.saturation = 30.0;
        let (_, tail) = pipeline::build_pipeline_split(&e, pipeline::RenderScale::new(1.0));
        let after = names(&accelerate(tail, &e, &ctx));
        let grade = after.iter().position(|n| *n == "color_grading").expect("kept");
        let chains: Vec<usize> = after
            .iter()
            .enumerate()
            .filter(|(_, n)| **n == "gpu_chain")
            .map(|(i, _)| i)
            .collect();
        assert_eq!(chains.len(), 2);
        assert!(
            chains[0] < grade && grade < chains[1],
            "the grade must stay between the two chains, got {after:?}"
        );
    }
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test --release -p gpu-pipeline 2>&1 | grep -E "FAILED|test result" | head -4
```
Expected: the merge tests fail — M8.3b makes one chain **per stage**, so two adjacent stages give
two chains rather than one.

- [ ] **Step 3: Replace `accelerate` with the table and the run-merge**

```rust
/// A stage this can replace, by the name `PipelineStage::name` reports.
///
/// ⚠⚠ **Still name-matching, and still stringly-typed.** M8.3b recorded that this does not scale
/// to a dozen stages and that the right answer then is a `StageAccelerator` hook `pipeline` calls
/// at construction. A table is the smaller step that stops an if-chain growing; it is **not** a
/// fix. ⚠ The hook loses today for the reason it did in M8.3b: `RenderSpec` is `Copy + PartialEq`
/// and an `Arc<dyn Trait>` is neither.
///
/// ⚠ The builder takes the `RenderScale` as well as the `Edits`, because `SharpenStage`'s sigma is
/// in **full-resolution pixels** and only the scale says what that is at this rung. `GpuHslStage`
/// ignores it; a builder signature that omitted it would make the sharpen entry unwritable.
type Builder = fn(&Edits, RenderScale, &Arc<GpuContext>) -> Box<dyn GpuStage>;

const REPLACEABLE: &[(&str, Builder)] = &[
    ("hsl", |e, _, ctx| Box::new(GpuHslStage::new(&e.hsl, ctx))),
    ("sharpen", |e, scale, ctx| {
        Box::new(GpuSharpenStage::new(&e.sharpening, scale, ctx))
    }),
];

/// Replace runs of GPU-capable stages in `tail` with chains.
///
/// ⚠⚠ **Consecutive stages share ONE chain, and therefore one round trip.** That is the point:
/// sharpening alone is break-even against the transfer, and only the run is worth having.
///
/// ⚠ A regrouping, never a reordering — a stage this cannot replace ends the current run and keeps
/// its position.
///
/// ⚠ Only the **global** tail. A mask's own stages live inside `MaskStage`, which this sees as one
/// opaque stage.
pub fn accelerate(
    tail: Pipeline,
    edits: &Edits,
    scale: RenderScale,
    ctx: &Arc<GpuContext>,
) -> Pipeline {
    let mut out: Vec<Box<dyn PipelineStage>> = Vec::new();
    let mut run: Vec<Box<dyn GpuStage>> = Vec::new();
    for s in tail.into_stages() {
        match REPLACEABLE.iter().find(|(n, _)| *n == s.name()) {
            Some((_, build)) => run.push(build(edits, scale, ctx)),
            None => {
                flush(&mut run, &mut out, ctx);
                out.push(s);
            }
        }
    }
    flush(&mut run, &mut out, ctx);
    Pipeline::new(out)
}

/// Turn any pending run into a chain.
fn flush(
    run: &mut Vec<Box<dyn GpuStage>>,
    out: &mut Vec<Box<dyn PipelineStage>>,
    ctx: &Arc<GpuContext>,
) {
    if !run.is_empty() {
        out.push(Box::new(GpuChain::new(ctx.clone(), std::mem::take(run))));
    }
}
```

⚠⚠ **`accelerate` gains a `scale` parameter, and that is a signature change with two call sites** —
`render_engine.rs`'s `build_tail` and this module's own tests. The scale is already in scope at
both; the compiler names them. Without it the sharpen builder cannot be written at all, since a
sigma in full-resolution pixels means nothing without the rung it is being rendered at.

- [ ] **Step 4: Run**

```bash
cargo test --release -p gpu-pipeline 2>&1 | grep -E "test result|FAILED" | head -4
```

- [ ] **Step 5: Sabotage — merge regardless of position**

```bash
cp crates/gpu-pipeline/src/accelerate.rs /tmp/m83c-acc.bak
python3 - <<'PY'
p='crates/gpu-pipeline/src/accelerate.rs'; s=open(p).read()
o="                flush(&mut run, &mut out, ctx);\n                out.push(s);"
assert o in s, "sabotage did not match"
open(p,'w').write(s.replace(o, "                out.push(s);"))
PY
cargo test --release -p gpu-pipeline > /tmp/m83c-sab2.log 2>&1
grep -q "test result:" /tmp/m83c-sab2.log && grep -E "FAILED" /tmp/m83c-sab2.log | head -3 || echo "  DID NOT COMPILE"
cp /tmp/m83c-acc.bak crates/gpu-pipeline/src/accelerate.rs
```
Expected: `a_cpu_stage_between_two_gpu_stages_produces_two_chains` and `merging_preserves_order`.

- [ ] **Step 6: Gate and commit**

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings > /tmp/m83c-cl2.log 2>&1; echo "CLIPPY=$?"
git add -A
git commit -m "feat: merge consecutive GPU stages into one chain

Consecutive stages share ONE round trip, which is the point: sharpening
alone is break-even against the transfer and only the run is worth
having.

⚠ The test that matters is the negative one -- a CPU stage between two
GPU stages must produce TWO chains. Without it, an implementation that
merged everything regardless of position would pass the positive test
while running the CPU stage's work in the wrong order.

A table replaces the if-chain. ⚠ Still name-matching and still not a fix
for stringly-typed lookup; the StageAccelerator hook remains the right
answer once a dozen stages are accelerated."
```

---

## Task 3: `GpuSharpenStage`

**Files:**
- Modify: `crates/pipeline/src/spatial.rs`
- Create: `crates/gpu-pipeline/src/sharpen.rs`
- Modify: `crates/gpu-pipeline/src/lib.rs`

- [ ] **Step 1: Expose the kernel**

In `crates/pipeline/src/spatial.rs`, change `fn gaussian_kernel` to `pub fn gaussian_kernel` and
give it:

```rust
/// The normalized Gaussian weights for `sigma`, `2·kernel_radius(sigma) + 1` of them.
///
/// ⚠ `pub` so a GPU twin uploads **these** weights rather than recomputing them. Two
/// implementations of a Gaussian is how the two come to disagree about a σ nobody re-derives —
/// the rule `HueTable::entries` already follows.
```

- [ ] **Step 2: Write `sharpen.rs`**

The stage holds the two kernels and the parameters, all computed on the CPU:

```rust
//! Sharpening as compute shaders — the first stage that reads its neighbours.

use crate::{GpuContext, GpuImage, GpuStage};
use edit_model::Sharpening;
use pipeline::{PipelineStage, RenderScale};
use std::sync::Arc;

const WORKGROUP: u32 = 64;
const MAX_GROUPS_PER_DIM: u32 = 65535;

/// ⚠ Mirrors `stages::sharpening`'s private constants. The equivalence test is what catches them
/// drifting.
const MASK_SIGMA: f32 = 2.0;
const MASK_MAX: f32 = 0.08;
const MAX_AMOUNT: f32 = 1.5;
const HALO_KNEE: f32 = 0.05;
const LUMA_EPS: f32 = 1e-3;
const GAIN_MAX: f32 = 4.0;
/// ⚠ The **working space's** luma row, not Rec.709 — `color::PROPHOTO_LUMA`. A Rec.709 triple here
/// would be the seventh copy of the constant that became wrong when the working space changed.
const LUMA: [f32; 3] = [0.2880402, 0.7118741, 0.0000857];
```

The shader has five entry points, dispatched in order into one pass: `luma`, `blur_h`, `blur_v`,
`difference`, `apply` — plus `magnitude`, a second `blur_h`/`blur_v` and `mask` when masking > 0.

⚠⚠ **The sample index must replicate exactly as the CPU does.** The CPU is
`(x + i).saturating_sub(r).min(w - 1)`. WGSL indices are **unsigned**, so write it as:

```wgsl
// ⚠⚠ `x + i` first, THEN subtract with a floor — `x + i - r` underflows to a huge number when
// x < r, which clamps to w-1 instead of 0 and puts a fringe down the left and top edges only.
let t = x + i;
let sx = min(select(0u, t - r, t >= r), w - 1u);
```

⚠ The kernel weights arrive as a storage buffer, uploaded from `pipeline::spatial::gaussian_kernel`.

⚠ **Masking 0 skips the mask dispatches entirely**, matching the CPU's `Option<Vec<f32>>` — both
the cheap path and what avoids `smoothstep(0, 0, ..)` being degenerate.

`scratch_planes()` returns **4**: luma, blurred, difference, mask.

- [ ] **Step 3: Write the tests**

```rust
    /// ⚠⚠ **A hard edge, not a ramp.** Sharpening is *about* edges, and a smooth ramp would let a
    /// wrong kernel or wrong edge replication pass — the mistake the M8.4 demosaic probe made by
    /// landing on road-sign lettering instead of foliage.
    fn edged(w: usize, h: usize) -> ImageBuf {
        let mut d = Vec::with_capacity(w * h * 3);
        for y in 0..h {
            for x in 0..w {
                let v = if x < w / 2 { 0.15 } else { 0.75 };
                // A little colour, so a luma-weighting error is visible.
                d.extend_from_slice(&[v, v * 0.9, v * 1.1]);
            }
        }
        ImageBuf::from_vec(w, h, d)
    }

    fn params(masking: f32) -> Sharpening {
        Sharpening {
            amount: 70.0,
            radius: 1.5,
            detail: 50.0,
            masking,
        }
    }

    fn gpu(ctx: &Arc<GpuContext>, p: &Sharpening, img: ImageBuf) -> ImageBuf {
        crate::GpuChain::new(
            ctx.clone(),
            vec![Box::new(GpuSharpenStage::new(p, RenderScale::FULL, ctx))],
        )
        .apply(img)
    }

    #[test]
    fn the_gpu_matches_the_cpu_with_masking_off() {
        let Some(ctx) = ctx() else { return };
        let p = params(0.0);
        let img = edged(96, 72);
        let cpu = pipeline::SharpenStage::new(&p, RenderScale::FULL).apply(img.clone());
        let out = gpu(&ctx, &p, img);
        let d = crate::compare(&cpu, &out);
        assert!(d.worst_levels < 0.01, "worst {} levels", d.worst_levels);
    }

    #[test]
    fn the_gpu_matches_the_cpu_with_masking_on() {
        // ⚠ A different code path: a second Gaussian and a smoothstep. One test without the
        // other covers half the stage.
        let Some(ctx) = ctx() else { return };
        let p = params(60.0);
        let img = edged(96, 72);
        let cpu = pipeline::SharpenStage::new(&p, RenderScale::FULL).apply(img.clone());
        let out = gpu(&ctx, &p, img);
        let d = crate::compare(&cpu, &out);
        assert!(d.worst_levels < 0.01, "worst {} levels", d.worst_levels);
    }

    #[test]
    fn the_left_and_top_edges_match_the_cpus_replication() {
        // ⚠⚠ **Where unsigned underflow in the sample index shows, and NOWHERE ELSE.** `x + i - r`
        // wraps to a huge number when x < r, clamping to w-1 instead of 0 — a fringe down the
        // left and top edges that every interior pixel would hide.
        let Some(ctx) = ctx() else { return };
        let p = params(0.0);
        let (w, h) = (64usize, 48usize);
        let img = edged(w, h);
        let cpu = pipeline::SharpenStage::new(&p, RenderScale::FULL).apply(img.clone());
        let out = gpu(&ctx, &p, img);
        for y in 0..h {
            for x in 0..4usize {
                let i = (y * w + x) * 3;
                let d = crate::compare(
                    &ImageBuf::from_vec(1, 1, cpu.data()[i..i + 3].to_vec()),
                    &ImageBuf::from_vec(1, 1, out.data()[i..i + 3].to_vec()),
                );
                assert!(d.worst_levels < 0.01, "left edge at ({x},{y}): {} levels", d.worst_levels);
            }
        }
        for x in 0..w {
            for y in 0..4usize {
                let i = (y * w + x) * 3;
                let d = crate::compare(
                    &ImageBuf::from_vec(1, 1, cpu.data()[i..i + 3].to_vec()),
                    &ImageBuf::from_vec(1, 1, out.data()[i..i + 3].to_vec()),
                );
                assert!(d.worst_levels < 0.01, "top edge at ({x},{y}): {} levels", d.worst_levels);
            }
        }
    }

    #[test]
    fn it_asks_for_four_scratch_planes() {
        let Some(ctx) = ctx() else { return };
        assert_eq!(
            GpuSharpenStage::new(&params(60.0), RenderScale::FULL, &ctx).scratch_planes(),
            4
        );
    }
```

- [ ] **Step 4: Run, then sabotage**

```bash
cargo test --release -p gpu-pipeline 2>&1 | grep -E "sharpen::|test result" | head -8
```

Sabotages, each restored from a `cp` backup and each expected to fail the named test:
`min(t - r, w - 1u)` without the lower-bound guard → `the_left_and_top_edges_match_the_cpus_replication`;
skipping the mask dispatches when masking > 0 → `the_gpu_matches_the_cpu_with_masking_on`;
recomputing the kernel weights in the shader instead of uploading → both equivalence tests;
`scratch_planes` returning 3 → `it_asks_for_four_scratch_planes`.

- [ ] **Step 5: Gate and commit**

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings > /tmp/m83c-cl3.log 2>&1; echo "CLIPPY=$?"
git add -A
git commit -m "feat: sharpening as compute shaders

The first stage that reads its neighbours. Five dispatches (nine with
masking) inside one compute pass, which M8.3b proved carries a memory
barrier between dispatches.

⚠⚠ The kernel weights are the CPU's own, uploaded rather than
recomputed: two implementations of a Gaussian is how they come to
disagree about a sigma nobody re-derives.

⚠⚠ WGSL indices are UNSIGNED, so the CPU's (x + i).saturating_sub(r)
needs an explicit floor -- x + i - r wraps to a huge number when x < r
and clamps to w-1 instead of 0. That is a fringe down the left and top
edges only, so those edges have their own test.

The fixture is a hard edge rather than a ramp: sharpening is about
edges, and a ramp would let a wrong kernel pass."
```

---

## Task 4: Rename the setting

**Files:**
- Modify: `crates/app/src/settings.rs`, `modals/settings.rs`, `settings_apply.rs`,
  `render_engine.rs`, `session_flow.rs`, `main.rs`, `multi/pane.rs`

- [ ] **Step 1: Write the failing migration test**

In `crates/app/src/settings.rs`'s tests:

```rust
    #[test]
    fn a_settings_file_holding_gpu_hsl_still_turns_the_gpu_on() {
        // ⚠⚠ **Renaming a serialised field is a MIGRATION, not a rename.** `Settings` carries a
        // container-level `#[serde(default)]`, so without the alias an existing file holding
        // `"gpu_hsl": true` would silently fall back to `false` — the user's choice discarded with
        // nothing to notice it. The `EditGroups::effects` → `vignette` rename set this precedent,
        // and its alias test exists because the same trap was live there.
        let f = TempFile::new("gpu_hsl_alias");
        std::fs::write(&f.0, r#"{"gpu_hsl": true}"#).unwrap();
        assert!(
            SettingsStore::new(f.0.clone()).load().gpu_tail,
            "an old gpu_hsl must carry over to gpu_tail"
        );
    }
```

- [ ] **Step 2: Rename with the alias**

```rust
    /// Run GPU-capable **global** tail stages — currently HSL and sharpening — on the GPU.
    ///
    /// ⚠⚠ **`#[serde(alias = "gpu_hsl")]` is a migration, not decoration.** See the test.
    ///
    /// ⚠ Renamed from `gpu_hsl` at M8.3c because the old name governed more than HSL, and a
    /// setting that silently covers more than it claims is what the `Effects` edit group and the
    /// "Reset crop" hover text were each corrected for.
    ///
    /// ⚠ Defaults to `false`: the CPU path is the oracle.
    #[serde(default, alias = "gpu_hsl")]
    pub gpu_tail: bool,
```

Then rename every use — `FrameSpec::gpu_hsl` → `gpu_tail`, `Session::gpu_hsl` → `gpu_tail`,
`apply_gpu_hsl` → `apply_gpu_tail`. The compiler names every site.

- [ ] **Step 3: Update the dialog**

```rust
            ui.horizontal(|ui| {
                ui.label("Rendering");
                ui.checkbox(&mut gpu_tail, "Use the GPU");
            });
            theme::hint(
                ui,
                "Runs the HSL and sharpening stages on the graphics card. Off by \
             default: the CPU is the reference, and the two agree to about a \
             hundredth of an 8-bit level, so the picture should not change. \
             ⚠ Does not apply inside a mask, and the first render after switching \
             it on spends about a sixth of a second compiling shaders.",
            );
```

- [ ] **Step 4: Gate**

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings > /tmp/m83c-cl4.log 2>&1; echo "CLIPPY=$?"
cargo test --workspace > /tmp/m83c-ws.log 2>&1; echo "TEST=$?"
grep -o "test result: ok\. [0-9]* passed" /tmp/m83c-ws.log | awk '{s+=$4} END {print "TOTAL:", s}'
echo "goldens run: $(grep -c '^test golden_' /tmp/m83c-ws.log)"; git status --short crates/pipeline/tests/
echo "skipped: $(grep -c SKIPPED /tmp/m83c-ws.log)"
```
Expected: 63 goldens run, **none moved**, 0 skipped.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat: gpu_hsl becomes gpu_tail, with a serde alias

The setting governs more than HSL now, so its name, label and hint were
all wrong -- the class the Effects edit group and the Reset crop hover
text were each corrected for.

⚠ Renaming a serialised field is a migration: Settings carries a
container-level serde(default), so without the alias an existing file
holding gpu_hsl: true would silently fall back to false. Its own test."
```

---

## Task 5: Measure the run

**Files:**
- Create: `crates/gpu-pipeline/examples/gpu_run_bench.rs`

⚠⚠ **This is the task that says whether the slice was worth it.** The spec predicts no number.

- [ ] **Step 1: Write a bench comparing three configurations**

The probe builds a real tail from `Edits` with HSL and sharpening both active, and times:
**(a)** the CPU tail, **(b)** the accelerated tail, and **(c)** the accelerated tail with colour
grading active, so the split-run cost is measured rather than asserted.

```rust
//! What does a merged GPU run actually save?
//!
//! ⚠ Sharpening alone is break-even against the round trip; the win is `hsl` + `sharpen` sharing
//! one transfer. This measures the run, and the third row measures what an active colour grade
//! costs by splitting it.
//!
//! `cargo run --release -p gpu-pipeline --example gpu_run_bench [width] [height]`
```

Report each configuration's CPU and GPU time over five rounds, plus the divergence, exactly as
`gpu_hsl_bench` does.

- [ ] **Step 2: Run and record**

```bash
cargo run --release -p gpu-pipeline --example gpu_run_bench 2>&1 | tail -16
```
⚠ If the merged run is **not** faster than the CPU tail, that is a finding to report and a reason
to reconsider the default — not something to bury.

- [ ] **Step 3: Commit**

---

## Task 6: Documentation and the GUI

**Files:** `CLAUDE.md`, `docs/BACKLOG.md`

- [ ] **Step 1: Add the M8.3c section to `CLAUDE.md`**, above M8.3b's, in house style. It must
record: the **measured** run figures from Task 5; that sharpening alone is break-even and the win
is the run; scratch-planes-not-ping-pong and why; the unsigned-index trap; that the kernel is
uploaded rather than recomputed; the setting rename and its migration; and the caveats from spec §8
— **especially that local contrast is not ported and an active colour grade splits the run**.

- [ ] **Step 2: Update the `▶ NEXT ACTION` block and the handoff**, pointing at **M8.3d** (the mask
loop, where masked HSL lives) and noting local contrast's three blur kinds.

- [ ] **Step 3: Update `docs/BACKLOG.md`.**

- [ ] **Step 4: Full gate** — `fmt --check`, `clippy`, `test --workspace`, 63 goldens run and none
moved, 0 skipped. **Use a fresh log name.**

- [ ] **Step 5: Commit and launch the GUI**

Tell the user: Settings → **Rendering → Use the GPU** now covers HSL **and** sharpening; the
picture must not change; **an active colour grade splits the run** so the win is smaller on those
photos; and masked stages are still unaffected.

---

## Self-Review

**Spec coverage.** §1 measurements → Task 5. §2 scratch planes → Task 1. §3 run-merging → Task 2.
§4 the table → Task 2. §5 the rename and its migration → Task 4. §6 `GpuSharpenStage` → Task 3.
§7 all eight tests → Tasks 1–3. §8 caveats → Task 6.

**Placeholder scan.** ⚠ **Task 3 Step 2 describes the shader rather than quoting it in full**, and
that is a deliberate exception: the WGSL is ~120 lines of five entry points, and the two things
that actually go wrong — the unsigned sample index and the uploaded kernel — are quoted exactly.
Task 5 describes the bench rather than quoting it, because its numbers do not exist until it runs.

**Type consistency.** `GpuImage::scratch`; `GpuStage::{name, scratch_planes, encode, cpu}` with
`encode(&self, ctx, pass, img, scratch: &[wgpu::Buffer])`; `GpuChain::scratch_needed`;
`GpuSharpenStage::new(&Sharpening, RenderScale, &Arc<GpuContext>)`; `accelerate(tail, edits, scale,
ctx)`; `Settings::gpu_tail`; `apply_gpu_tail`.

⚠ **Two errors in this plan's first draft, found by the self-review and fixed in place:**
`GpuImage::scratch` carried a pointless second `.map` over an undefined helper, and the
`REPLACEABLE` table called an `e_scale(e)` that does not exist. The second was the useful one —
it surfaced that **`accelerate` has no `RenderScale` and needs one**, which is a signature change
with two call sites rather than a typo. Both are corrected above; the scale parameter is now part
of Task 2's step text.
