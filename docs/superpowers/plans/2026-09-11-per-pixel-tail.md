# M8.3e plan 1 — the four stages a typical photograph runs

> **⚠ Conventions, not negotiable:** commit messages carry **no `Co-Authored-By` trailer**;
> **never push** — the user pushes to `github.com/wladiKrin/rustroom` himself. Branch
> `m8.3e-per-pixel-tail`, already created from `main` at `993b7d8`.
>
> **For agentic workers:** REQUIRED SUB-SKILL — superpowers:executing-plans.
>
> **Build env, before any cargo command:**
> ```
> export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
> ```
> ⚠ Iterate with `cargo test --release -p gpu-pipeline`: acquiring an adapter costs 44–75 s in a
> debug build against 21 ms in release.
>
> ⚠ Read every gate's exit code **from a session-unique log file, never through a pipe**.
> ⚠ Before any sabotage, `cp` the file. **Never `git checkout -- <file>`** — that has destroyed
> uncommitted work in this repo twice.

**Goal:** `exposure`, `tone_regions`, `display_rendering` and `output_transform` run as compute
shaders, so an ordinary photograph's entire tail is one chain and one round trip.

**Spec:** `docs/superpowers/specs/2026-09-11-per-pixel-tail-design.md`.

**Architecture:** each stage is a `GpuStage` in `crates/gpu-pipeline/src/`, carrying its CPU twin
and a row in `REPLACEABLE`. A new cost gate stops a cheap lone run paying for a round trip. The
wgpu boilerplate every per-pixel stage repeats is extracted once first, so the four new stages are
short enough to read.

**Target, measured:** the typical tail is **74.75 ms** of CPU at 3200×2136 against a **34.03 ms**
round trip. Expect **~1.9–2.1×**, not more.

---

### Task 1: Extract the per-pixel dispatch, changing no behaviour

⚠ Done **first and alone**, because four copies of ~120 lines of wgpu boilerplate is precisely how
this repo's oversized files got that way. It is a pure refactor: **its acceptance is that every
existing `gpu-pipeline` test passes unchanged.**

**Files:**
- Create: `crates/gpu-pipeline/src/dispatch.rs`
- Modify: `crates/gpu-pipeline/src/lib.rs` (declare `mod dispatch;`)
- Modify: `crates/gpu-pipeline/src/grading.rs`, `crates/gpu-pipeline/src/hsl.rs`

- [ ] **Step 1: Record what must not change**

Run: `cargo test --release -p gpu-pipeline 2>&1 | tail -3`
Write the passing count down. It must be identical at Step 5.

- [ ] **Step 2: Write `dispatch.rs`**

```rust
//! The dispatch every per-pixel stage repeats, written once.
//!
//! ⚠⚠ **Extracted rather than copied a fourth time.** `hsl` and `grading` had the same ~120 lines
//! of layout/bind/pipeline/dispatch differing only in a label, a shader and how many read-only
//! storage buffers they bind; M8.3e adds four more stages of the same shape. ⚠ `sharpen` is
//! deliberately NOT a client — it encodes up to eight dispatches over scratch planes, which is a
//! different thing wearing a similar name.

use crate::{GpuContext, GpuImage};

const WORKGROUP: u32 = 64;

/// ⚠⚠ 65535 workgroups per dimension, so the dispatch is 2-D from the start. A 1-D dispatch
/// passes every small fixture and fails on the first real photograph — a 26 Mpx rung needs
/// 407,000 groups.
const MAX_GROUPS_PER_DIM: u32 = 65535;

/// One per-pixel compute dispatch over the image buffer.
pub(crate) struct Dispatch<'a> {
    pub label: &'static str,
    /// Full WGSL, prelude already concatenated by the caller.
    pub shader: String,
    /// The uniform at binding 1. ⚠ Must be a multiple of 16 bytes.
    pub params: Vec<u8>,
    /// Read-only storage buffers, bound from binding 2 upward in order.
    pub storages: &'a [&'a [f32]],
}

impl Dispatch<'_> {
    /// The 2-D workgroup split: `(row_groups, col_groups)`.
    ///
    /// ⚠ Computed here rather than in five places that could disagree about it — and `row_groups`
    /// is **also** what every shader needs in its uniform to recover the pixel index, so a caller
    /// that split it differently from the dispatch would read the wrong pixel.
    pub fn grid(pixels: u32) -> (u32, u32) {
        let total = pixels.div_ceil(WORKGROUP);
        let row_groups = total.min(MAX_GROUPS_PER_DIM);
        (row_groups, total.div_ceil(row_groups))
    }

    pub fn encode(self, ctx: &GpuContext, pass: &mut wgpu::ComputePass, img: &GpuImage) {
        let device = &ctx.device;
        let (row_groups, col_groups) = Self::grid(img.pixels());

        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some(self.label),
            source: wgpu::ShaderSource::Wgsl(self.shader.into()),
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(self.label),
            size: self.params.len() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        ctx.queue.write_buffer(&uniform, 0, &self.params);

        let buffers: Vec<wgpu::Buffer> = self
            .storages
            .iter()
            .map(|s| {
                let b = device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(self.label),
                    size: (s.len() * 4).max(4) as u64,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                ctx.queue.write_buffer(&b, 0, bytemuck::cast_slice(s));
                b
            })
            .collect();

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
        let mut layout_entries = vec![
            entry(0, wgpu::BufferBindingType::Storage { read_only: false }),
            entry(1, wgpu::BufferBindingType::Uniform),
        ];
        for i in 0..buffers.len() as u32 {
            layout_entries.push(entry(
                2 + i,
                wgpu::BufferBindingType::Storage { read_only: true },
            ));
        }
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some(self.label),
            entries: &layout_entries,
        });

        let mut bind_entries = vec![
            wgpu::BindGroupEntry {
                binding: 0,
                resource: img.buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: uniform.as_entire_binding(),
            },
        ];
        for (i, b) in buffers.iter().enumerate() {
            bind_entries.push(wgpu::BindGroupEntry {
                binding: 2 + i as u32,
                resource: b.as_entire_binding(),
            });
        }
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some(self.label),
            layout: &layout,
            entries: &bind_entries,
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some(self.label),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let compute = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(self.label),
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        pass.set_pipeline(&compute);
        pass.set_bind_group(0, &bind, &[]);
        pass.dispatch_workgroups(row_groups, col_groups, 1);
    }
}
```

- [ ] **Step 3: Declare it**

In `crates/gpu-pipeline/src/lib.rs`, add `mod dispatch;` beside `mod chain;`.

- [ ] **Step 4: Route `grading` and `hsl` through it**

In `grading.rs`, replace the body of `encode` (everything after computing `params`) with:

```rust
        let (row_groups, _) = crate::dispatch::Dispatch::grid(img.pixels());
        let mut params = [0u8; 16];
        params[0..4].copy_from_slice(&img.pixels().to_le_bytes());
        params[4..8].copy_from_slice(&row_groups.to_le_bytes());
        params[8..12].copy_from_slice(&((self.table.len() / 3) as u32).to_le_bytes());
        crate::dispatch::Dispatch {
            label: "grading",
            shader: format!("{}{}", crate::oklab_wgsl::OKLAB_WGSL, SHADER),
            params: params.to_vec(),
            storages: &[&self.table],
        }
        .encode(ctx, pass, img);
```

Do the same for `hsl.rs`, keeping its own label, shader and table. ⚠ Delete the now-unused
`WORKGROUP` and `MAX_GROUPS_PER_DIM` constants from both files — a `const` with no caller is
`dead_code` under `-D warnings`.

- [ ] **Step 5: The acceptance — the count is unchanged**

Run: `cargo test --release -p gpu-pipeline 2>&1 | tail -3`
Expected: **exactly** the count from Step 1, all passing. ⚠ "Still green" is not the assertion;
*unchanged* is. A refactor that changes the count moved behaviour.

- [ ] **Step 6: Gate and commit**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
L=/tmp/m83e-t1-$$
cargo fmt --all > $L.fmt 2>&1; echo "FMT=$?"
cargo clippy --all-targets --all-features -- -D warnings > $L.clippy 2>&1; echo "CLIPPY=$?"
```
⚠ Both must read 0 **before** committing. At least fifteen times in this repo clippy has caught
what a green suite did not.

```bash
git add -A && git commit -m "refactor: one per-pixel dispatch, shared by hsl and grading"
```

---

### Task 2: The cost gate

**Files:**
- Modify: `crates/gpu-pipeline/src/chain.rs` (the trait gains `cpu_cost`)
- Modify: `crates/gpu-pipeline/src/accelerate.rs` (the run carries both; `flush` decides)
- Modify: `crates/gpu-pipeline/src/{hsl,grading,sharpen,exposure}.rs` (each declares its cost)

- [ ] **Step 1: Write the failing tests**

In `accelerate.rs`'s test module:

```rust
    /// A replaceable stage that costs almost nothing, so the gate has something to refuse.
    /// ⚠ A test stage rather than a real cheap one, so this test cannot be invalidated by a
    /// future re-measurement of any shipped stage's cost.
    struct Trivial(pipeline::ExposureStage);
    impl GpuStage for Trivial {
        fn name(&self) -> &'static str {
            "exposure"
        }
        fn cpu_cost(&self) -> f32 {
            0.5
        }
        fn encode(&self, _: &GpuContext, _: &mut wgpu::ComputePass, _: &GpuImage, _: &[wgpu::Buffer]) {
        }
        fn cpu(&self) -> &dyn PipelineStage {
            &self.0
        }
    }

    /// ⚠⚠ The gate's whole reason for existing: `output_transform` is on **every** photograph, so
    /// without this a neutral frame would build a one-stage chain and pay a 34 ms toll for 28.6 ms
    /// of work — a regression where there is none today.
    #[test]
    fn a_run_cheaper_than_the_transfer_is_left_on_the_cpu() {
        assert!(
            !worth_a_chain(&[0.5, 1.2]),
            "two trivial stages must not be worth a round trip"
        );
    }

    /// ⚠ The other direction, and it is what rules out a stage *count* as the gate: one expensive
    /// stage pays easily — `hsl` alone is a measured 1.56x — while three cheap ones do not.
    #[test]
    fn one_expensive_stage_is_still_worth_a_chain() {
        assert!(worth_a_chain(&[60.0]), "hsl alone must still be accelerated");
    }

    /// ⚠ Fails toward the CPU: an empty run is never a chain.
    #[test]
    fn an_empty_run_is_not_a_chain() {
        assert!(!worth_a_chain(&[]));
    }
```

- [ ] **Step 2: Run them and watch them fail**

Run: `cargo test --release -p gpu-pipeline worth_a_chain 2>&1 | tail -5`
Expected: FAIL, `cannot find function 'worth_a_chain'`.
⚠ Read the **passed count**, not the word `ok`: cargo takes a substring, not a regex, and a filter
matching nothing prints `0 passed … ok`.

- [ ] **Step 3: Add `cpu_cost` to the trait**

In `chain.rs`, inside `pub trait GpuStage`:

```rust
    /// This stage's CPU cost, in milliseconds per 6.8 megapixels.
    ///
    /// ⚠⚠ **Used ONLY to decide whether a run is worth a round trip, never to decide what is
    /// rendered.** It fails toward "run on the CPU", which is always correct — a wrong number
    /// costs milliseconds, never pixels. That is what separates it from the cost heuristic
    /// `PrefixCache` deliberately refused, where a wrong guess would have changed what was cached.
    ///
    /// ⚠ Deliberately has **no default**, so adding a stage forces the measurement rather than
    /// silently inheriting someone else's. `probe_stages` is how the number is obtained.
    fn cpu_cost(&self) -> f32;
```

- [ ] **Step 4: Declare each shipped stage's cost**

Measured by `cargo run --release -p pipeline --example probe_stages` at 3200×2136, and by
`bench_gpu_tail` for the three already ported. Add to each stage's `impl GpuStage`:

| file | body |
|---|---|
| `exposure.rs` | `fn cpu_cost(&self) -> f32 { 1.3 }` |
| `hsl.rs` | `fn cpu_cost(&self) -> f32 { 60.0 }` |
| `grading.rs` | `fn cpu_cost(&self) -> f32 { 48.0 }` |
| `sharpen.rs` | `fn cpu_cost(&self) -> f32 { 54.0 }` |

⚠ Each carries a one-line comment naming where the number came from. ⚠⚠ The three ported ones are
derived from the bench's *deltas at 1600×1068 scaled by 3.9*, because they can no longer be
measured directly at 3200 through `probe_stages` — say so at the constant rather than implying
they were measured the same way.

- [ ] **Step 5: The gate itself**

In `accelerate.rs`:

```rust
/// What one round trip costs, in the same units as [`GpuStage::cpu_cost`] — milliseconds per 6.8
/// megapixels, measured by `gpu-pipeline`'s `probe_rt` on an Apple M2.
///
/// ⚠⚠ **The comparison is scale-free, which is why this is one number and not one per rung.**
/// Both sides are linear in pixel count — the transfer moves `w*h*12` bytes, a per-pixel stage
/// does `w*h` work — so the ratio does not depend on resolution. Measured rather than assumed:
/// 4x the pixels gave 3.9x the round trip, 8.71 -> 34.03 ms.
const TRANSFER_COST: f32 = 34.0;

/// Is this run worth a round trip?
fn worth_a_chain(costs: &[f32]) -> bool {
    costs.iter().sum::<f32>() > TRANSFER_COST
}
```

- [ ] **Step 6: Make the run carry both halves**

⚠⚠ **The run currently holds only the GPU stage, so a refused run would have nothing to put
back** — and `GpuStage::cpu()` hands out a borrow that cannot be pushed into the output. Change
the run's element type and let `flush` choose:

```rust
    let mut out: Vec<Box<dyn PipelineStage>> = Vec::new();
    // ⚠ Both halves: the built GPU stage, and the original it would replace. `flush` decides.
    let mut run: Vec<(Box<dyn GpuStage>, Box<dyn PipelineStage>)> = Vec::new();
    for s in tail.into_stages() {
        match REPLACEABLE.iter().find(|(n, _)| *n == s.name()) {
            Some((_, build)) => run.push((build(edits, scale, ctx), s)),
            None => {
                flush(&mut run, &mut out, ctx);
                out.push(s);
            }
        }
    }
    flush(&mut run, &mut out, ctx);
    Pipeline::new(out)
}

/// Turn any pending run into a chain — or put it back on the CPU if it is not worth the transfer.
fn flush(
    run: &mut Vec<(Box<dyn GpuStage>, Box<dyn PipelineStage>)>,
    out: &mut Vec<Box<dyn PipelineStage>>,
    ctx: &Arc<GpuContext>,
) {
    if run.is_empty() {
        return;
    }
    let taken = std::mem::take(run);
    let costs: Vec<f32> = taken.iter().map(|(g, _)| g.cpu_cost()).collect();
    if worth_a_chain(&costs) {
        out.extend([Box::new(GpuChain::new(
            ctx.clone(),
            taken.into_iter().map(|(g, _)| g).collect(),
        )) as Box<dyn PipelineStage>]);
    } else {
        // ⚠ In order, so a refused run keeps its position exactly as the CPU stages had it.
        out.extend(taken.into_iter().map(|(_, c)| c));
    }
}
```

- [ ] **Step 7: Run the tests**

Run: `cargo test --release -p gpu-pipeline 2>&1 | tail -3`
Expected: PASS, three more than Task 1's count.

- [ ] **Step 8: Sabotage, and confirm the named test fails**

```bash
cp crates/gpu-pipeline/src/accelerate.rs /tmp/acc.bak
# make the gate always accept
sed -i '' 's/costs.iter().sum::<f32>() > TRANSFER_COST/true/' crates/gpu-pipeline/src/accelerate.rs
grep -n "fn worth_a_chain" -A 2 crates/gpu-pipeline/src/accelerate.rs   # ⚠ assert the edit landed
cargo test --release -p gpu-pipeline 2>&1 | tail -5
cp /tmp/acc.bak crates/gpu-pipeline/src/accelerate.rs                   # ⚠ restore from the copy
```
Expected: `a_run_cheaper_than_the_transfer_is_left_on_the_cpu` and `an_empty_run_is_not_a_chain`
fail. ⚠ Confirm a `test result:` line appears at all — a sabotage that fails to compile reads
exactly like one that passes.

- [ ] **Step 9: Gate and commit**

Both exit codes read from files, as Task 1 Step 6.
```bash
git add -A && git commit -m "feat: a run is chained only when it is worth the round trip"
```

---

### Task 3: Exposure joins the table

⚠ The whole task is one row: `GpuExposureStage` has existed since M8.3a as the seam's proof and
was never made reachable.

**Files:** Modify `crates/gpu-pipeline/src/accelerate.rs`.

- [ ] **Step 1: Write the failing test**

```rust
    /// ⚠ Exposure alone must NOT be a chain — 1.3 ms of work against a 34 ms transfer — which is
    /// the gate and the table row tested together.
    #[test]
    fn exposure_alone_is_not_worth_a_chain() {
        let Some(ctx) = ctx() else { return };
        let mut e = Edits::default();
        e.exposure = 0.45;
        let (_, tail) = pipeline::build_pipeline_split_in_region(&e, SCALE, None);
        let names: Vec<_> = accelerate(tail, &e, SCALE, &ctx)
            .into_stages()
            .iter()
            .map(|s| s.name())
            .collect();
        assert!(
            names.contains(&"exposure"),
            "expected the CPU exposure stage, got {names:?}"
        );
    }
```

- [ ] **Step 2: Run it and watch it fail**

Run: `cargo test --release -p gpu-pipeline exposure_alone 2>&1 | tail -5`
Expected: PASS already — exposure is not yet replaceable. ⚠ That is fine and is *why* it is
written now: it pins the behaviour **before** the table row, so Step 4 proves the gate is what
keeps it true rather than the absence of the row.

- [ ] **Step 3: Add the row**

In `REPLACEABLE`:
```rust
    ("exposure", |e, _, ctx| {
        Box::new(GpuExposureStage::new(e.exposure, ctx.clone()))
    }),
```
⚠ Keep the table in **tail order** — `exposure` goes first, before `hsl`. The order does not
affect lookup, and a table that reads in pipeline order is one a reader can check against
`build_pipeline_split_in_region`.

- [ ] **Step 4: Run the tests**

Run: `cargo test --release -p gpu-pipeline 2>&1 | tail -3`
Expected: PASS, unchanged count from Task 2 plus one. ⚠ `exposure_alone_is_not_worth_a_chain` now
passes **because of the gate**, which is the assertion.

- [ ] **Step 5: Gate and commit**

```bash
git add -A && git commit -m "feat: exposure becomes replaceable, three milestones after it was written"
```

---

### Task 4: `GpuToneRegionsStage`

**Files:**
- Modify: `crates/pipeline/src/stages/tone_regions.rs` (expose a table)
- Create: `crates/gpu-pipeline/src/tone_regions.rs`
- Modify: `crates/gpu-pipeline/src/lib.rs`, `crates/gpu-pipeline/src/accelerate.rs`

- [ ] **Step 1: Write the failing test for the table's resolution**

In `tone_regions.rs`'s test module:

```rust
    /// ⚠⚠ **This table does not exist on the CPU, so building it introduces an error the CPU
    /// stage does not have** — unlike `tone_curve` and `display_rendering`, which are already
    /// tables. The size is therefore measured rather than chosen, the recipe
    /// `DisplayRenderingStage`'s own 4096 came from.
    ///
    /// ⚠ The bound is a tenth of an 8-bit level on the **gain**, which is what the pixel sees.
    #[test]
    fn the_gain_table_resolves_the_curve() {
        let s = ToneRegionsStage::new(60.0, -40.0, 80.0, -30.0);
        let lut = s.gain_lut();
        let n = lut.len();
        let mut worst: f32 = 0.0;
        for i in 0..=4000 {
            let l = i as f32 / 4000.0 * ToneRegionsStage::LUT_TOP;
            let t = (l / ToneRegionsStage::LUT_TOP).clamp(0.0, 1.0) * (n - 1) as f32;
            let j = (t.floor() as usize).min(n - 1);
            let k = (j + 1).min(n - 1);
            let from_table = lut[j] + (t - j as f32) * (lut[k] - lut[j]);
            worst = worst.max((from_table - s.gain_for_luma(l)).abs());
        }
        assert!(
            worst < 0.1 / 255.0,
            "the table drifts {worst} from the curve; raise LUT_SIZE"
        );
    }
```

- [ ] **Step 2: Run it and watch it fail**

Run: `cargo test -p pipeline the_gain_table_resolves 2>&1 | tail -5`
Expected: FAIL, `no method named 'gain_lut'`.

- [ ] **Step 3: Expose the table from the CPU stage**

In `tone_regions.rs`:

```rust
impl ToneRegionsStage {
    /// The largest luminance the gain table resolves.
    ///
    /// ⚠ Scene-referred luminance is unbounded, but the weights are shaped on the **display
    /// encoding** of it, which saturates — so past this the gain is flat and the table's last
    /// entry is the right answer for every value above it.
    pub const LUT_TOP: f32 = 4.0;

    /// ⚠⚠ Built **here**, by the stage that owns `gain_for_luma`, so there is still exactly one
    /// implementation of the weight shaping. A GPU stage rebuilding it in WGSL is how the two
    /// come to disagree about the display encode — and a wrong gain renders a plausible
    /// photograph rather than an error.
    pub fn gain_lut(&self) -> Vec<f32> {
        (0..Self::LUT_SIZE)
            .map(|i| self.gain_for_luma(i as f32 / (Self::LUT_SIZE - 1) as f32 * Self::LUT_TOP))
            .collect()
    }

    /// ⚠ Measured by `the_gain_table_resolves_the_curve`, not chosen.
    pub const LUT_SIZE: usize = 1024;
}
```
and change `fn gain_for_luma` to `pub fn gain_for_luma`.

- [ ] **Step 4: Run it; raise `LUT_SIZE` until it passes**

Run: `cargo test -p pipeline the_gain_table_resolves 2>&1 | tail -5`
Expected: PASS. ⚠ If it fails, **raise `LUT_SIZE`** (1024 → 2048 → 4096) and record the value that
passes in its comment. ⚠⚠ Do **not** loosen the bound — that is the "assertion adjusted to fit
the code" trap this repo records repeatedly.

- [ ] **Step 5: Write the GPU stage's failing tests**

Create `crates/gpu-pipeline/src/tone_regions.rs` with this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use core_image::ImageBuf;
    use edit_model::Edits;

    fn ctx() -> Option<Arc<GpuContext>> {
        match crate::shared() {
            Some(c) => Some(c.clone()),
            None => {
                eprintln!("SKIPPED: no GPU adapter on this machine");
                None
            }
        }
    }

    fn edits() -> Edits {
        Edits {
            highlights: -60.0,
            shadows: 45.0,
            whites: 70.0,
            blacks: -35.0,
            ..Edits::default()
        }
    }

    /// ⚠⚠ Spans dark, mid, **above white** and **negative**, plus a neutral — the regions where a
    /// wrong clamp hides and an in-gamut ramp cannot see it.
    fn spread() -> Vec<f32> {
        vec![
            0.02, 0.03, 0.05, 0.18, 0.18, 0.18, 0.80, 0.20, 0.35, 1.60, 1.20, 0.90, -0.50, 0.02,
            0.03, 0.35, 0.62, 0.28,
        ]
    }

    fn chained(ctx: &Arc<GpuContext>, e: &Edits, img: ImageBuf) -> ImageBuf {
        crate::GpuChain::new(
            ctx.clone(),
            vec![Box::new(GpuToneRegionsStage::new(e, ctx))],
        )
        .apply(img)
    }

    fn cpu_of(e: &Edits) -> pipeline::ToneRegionsStage {
        pipeline::ToneRegionsStage::new(e.highlights, e.shadows, e.whites, e.blacks)
    }

    /// ⚠ The band is wider than the transcendental stages' 0.01 because this stage's table does
    /// not exist on the CPU, so the table error is real and is bounded by
    /// `the_gain_table_resolves_the_curve` at a tenth of a level. ⚠⚠ If it has to be widened
    /// further that is a finding to investigate, never a number to adjust.
    #[test]
    fn the_gpu_tone_regions_match_the_cpu_within_a_tenth_of_a_level() {
        let Some(ctx) = ctx() else { return };
        let e = edits();
        let img = ImageBuf::from_vec(6, 1, spread());
        let cpu = cpu_of(&e).apply(img.clone());
        let gpu = chained(&ctx, &e, img);
        let d = crate::compare(&cpu, &gpu);
        assert!(
            d.worst_levels < 0.1,
            "worst {} levels over {} of {} samples",
            d.worst_levels,
            d.differing,
            d.total
        );
    }

    /// ⚠⚠ Every equivalence test above passes against a GPU **and** CPU stage that both do
    /// nothing, because they compare the two against each other. This says the fixture is one
    /// the stage actually moves — and the goldens cannot see this crate at all.
    #[test]
    fn the_stage_really_moves_the_tones_rather_than_copying() {
        let Some(ctx) = ctx() else { return };
        let img = ImageBuf::from_vec(6, 1, spread());
        let out = chained(&ctx, &edits(), img.clone());
        let d = crate::compare(&img, &out);
        assert!(
            d.worst_levels > 5.0,
            "the fixture must be one the tone regions change; moved only {} levels",
            d.worst_levels
        );
    }

    /// ⚠⚠ A luminance above the table's top must take the last entry, not be dragged back — the
    /// **index** is clamped, never the value. Scene-referred highlights are routine, and this is
    /// the identical trap `ProfileHueSatMap`'s `v` and the grade's `lab.x` are each documented
    /// for. ⚠ On an in-gamut fixture it is invisible, and the result is still a plausible picture.
    #[test]
    fn a_luminance_above_the_tables_top_keeps_its_headroom() {
        let Some(ctx) = ctx() else { return };
        let e = edits();
        let img = ImageBuf::from_vec(2, 1, vec![6.0, 5.5, 5.0, 0.4, 0.4, 0.4]);
        let cpu = cpu_of(&e).apply(img.clone());
        let gpu = chained(&ctx, &e, img);
        let d = crate::compare(&cpu, &gpu);
        assert!(d.worst_levels < 0.1, "worst {} levels", d.worst_levels);
        // ⚠ The fixture check: the bright pixel must still BE bright, or this would pass against
        // two implementations that both crushed it.
        assert!(
            gpu.data()[0] > 3.0,
            "the highlight was crushed to {}",
            gpu.data()[0]
        );
    }

    #[test]
    fn it_asks_for_no_scratch_planes() {
        let Some(ctx) = ctx() else { return };
        let chain = crate::GpuChain::new(
            ctx.clone(),
            vec![Box::new(GpuToneRegionsStage::new(&edits(), &ctx))],
        );
        assert_eq!(chain.scratch_needed(), 0);
    }
}
```

- [ ] **Step 6: Run them and watch them fail**

Run: `cargo test --release -p gpu-pipeline tone_regions 2>&1 | tail -5`
Expected: FAIL to compile — the module does not exist.

- [ ] **Step 7: Write the stage**

```rust
//! The four tone-region sliders as a compute shader.
//!
//! ⚠⚠ **The whole four-slider model is one gain that depends on luminance alone**, so it is a 1-D
//! table and the shader has no weight shaping in it at all. That matters beyond brevity: the
//! weights are computed on **display-encoded** luminance, which is the fix for "the Whites slider
//! did nothing on most photographs", and re-deriving it in WGSL is how the two would drift.

use crate::{GpuContext, GpuImage, GpuStage};
use pipeline::PipelineStage;
use std::sync::Arc;

const SHADER: &str = r#"
struct Params { pixels: u32, row_groups: u32, lut_len: u32, pad0: u32, top: f32, pad1: f32, pad2: f32, pad3: f32 };
@group(0) @binding(0) var<storage, read_write> data: array<f32>;
@group(0) @binding(1) var<uniform> params: Params;
@group(0) @binding(2) var<storage, read> lut: array<f32>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let pixel = id.y * (params.row_groups * 64u) + id.x;
    if (pixel >= params.pixels) { return; }
    let p = pixel * 3u;
    let r = data[p]; let g = data[p + 1u]; let b = data[p + 2u];
    let l = prophoto_luminance(r, g, b);

    // ⚠⚠ clamp the INDEX, never l itself -- a scene-referred highlight above the table's top
    // takes the last entry, where the gain is already flat.
    let t = clamp(l / params.top, 0.0, 1.0) * f32(params.lut_len - 1u);
    let i = min(u32(floor(t)), params.lut_len - 1u);
    let j = min(i + 1u, params.lut_len - 1u);
    let gain = max(mix(lut[i], lut[j], t - f32(i)), 0.0);

    data[p] = r * gain; data[p + 1u] = g * gain; data[p + 2u] = b * gain;
}
"#;

pub struct GpuToneRegionsStage {
    table: Vec<f32>,
    cpu: pipeline::ToneRegionsStage,
}

impl GpuToneRegionsStage {
    pub fn new(e: &edit_model::Edits, _ctx: &Arc<GpuContext>) -> Self {
        let cpu = pipeline::ToneRegionsStage::new(e.highlights, e.shadows, e.whites, e.blacks);
        // ⚠ The CPU stage's own table. Never rebuilt here.
        let table = cpu.gain_lut();
        Self { table, cpu }
    }
}

impl GpuStage for GpuToneRegionsStage {
    fn name(&self) -> &'static str {
        "tone_regions"
    }
    /// Measured at 3200x2136 by `pipeline`'s `probe_stages`: +18.10 ms.
    fn cpu_cost(&self) -> f32 {
        18.0
    }
    fn encode(&self, ctx: &GpuContext, pass: &mut wgpu::ComputePass, img: &GpuImage, _: &[wgpu::Buffer]) {
        let (row_groups, _) = crate::dispatch::Dispatch::grid(img.pixels());
        let mut params = [0u8; 32];
        params[0..4].copy_from_slice(&img.pixels().to_le_bytes());
        params[4..8].copy_from_slice(&row_groups.to_le_bytes());
        params[8..12].copy_from_slice(&(self.table.len() as u32).to_le_bytes());
        params[16..20]
            .copy_from_slice(&pipeline::ToneRegionsStage::LUT_TOP.to_le_bytes());
        crate::dispatch::Dispatch {
            label: "tone_regions",
            shader: format!("{}{}", crate::luma_wgsl::LUMA_WGSL, SHADER),
            params: params.to_vec(),
            storages: &[&self.table],
        }
        .encode(ctx, pass, img);
    }
    fn cpu(&self) -> &dyn PipelineStage {
        &self.cpu
    }
}
```

⚠ This needs a **`prophoto_luminance` WGSL helper**, which does not exist yet. Add
`crates/gpu-pipeline/src/luma_wgsl.rs` on `oklab_wgsl.rs`'s model, with the coefficients
substituted from `color::PROPHOTO_LUMA` so there is no second copy of a luma triple — ⚠⚠ this
repo has already had **six** copies of a Rec.709 triple all become wrong on the same day.

- [ ] **Step 8: Wire it up and run**

Add `mod tone_regions;` / `pub use tone_regions::GpuToneRegionsStage;` to `lib.rs`, and the
`REPLACEABLE` row in tail position after `exposure`.

Run: `cargo test --release -p gpu-pipeline 2>&1 | tail -3` — expected PASS.

- [ ] **Step 9: Sabotage**

Replace the table with a constant `1.0` fill; confirm the equivalence test **and**
`the_stage_really_moves_the_tones_rather_than_copying` fail. Restore from a `cp` backup.

- [ ] **Step 10: Gate and commit**

```bash
git add -A && git commit -m "feat: the tone regions as a compute shader, from the CPU stage's own gain table"
```

---

### Task 5: `GpuDisplayRenderingStage`

**Files:**
- Modify: `crates/pipeline/src/stages/display_rendering.rs` (expose `lut()` and `lut_max()`)
- Create: `crates/gpu-pipeline/src/display_rendering.rs`
- Modify: `crates/gpu-pipeline/src/lib.rs`, `accelerate.rs`

- [ ] **Step 1: Expose the table**

```rust
    /// The table and the largest scene value it resolves.
    ///
    /// ⚠ Both, always together: an index computed against the wrong top is a smoothly wrong
    /// picture rather than an error. ⚠ 4096 entries is measured — at 1024 it drifts 0.61 of an
    /// 8-bit level — so a caller must not resample it.
    pub fn lut(&self) -> &[f32] {
        &self.lut
    }
    pub fn lut_max(&self) -> f32 {
        self.lut_max
    }
```

- [ ] **Step 2: Write the failing tests**

In a new `crates/gpu-pipeline/src/display_rendering.rs` test module (`ctx()` and `spread()` as in
Task 4 — copy them, they are four lines and a shared helper crate is not worth a module):

```rust
    fn cpu_of() -> pipeline::DisplayRenderingStage {
        pipeline::DisplayRenderingStage::new(None, None)
    }

    fn chained(ctx: &Arc<GpuContext>, img: ImageBuf) -> ImageBuf {
        crate::GpuChain::new(
            ctx.clone(),
            vec![Box::new(GpuDisplayRenderingStage::new(None, None, ctx))],
        )
        .apply(img)
    }

    #[test]
    fn the_gpu_rendering_matches_the_cpu_within_a_tenth_of_a_level() {
        let Some(ctx) = ctx() else { return };
        let img = ImageBuf::from_vec(6, 1, spread());
        let cpu = cpu_of().apply(img.clone());
        let gpu = chained(&ctx, img);
        let d = crate::compare(&cpu, &gpu);
        assert!(
            d.worst_levels < 0.1,
            "worst {} levels over {} of {} samples",
            d.worst_levels,
            d.differing,
            d.total
        );
    }

    #[test]
    fn the_stage_really_renders_rather_than_copying() {
        let Some(ctx) = ctx() else { return };
        let img = ImageBuf::from_vec(6, 1, spread());
        let out = chained(&ctx, img.clone());
        let d = crate::compare(&img, &out);
        assert!(
            d.worst_levels > 20.0,
            "the rendering must move the fixture; moved only {} levels",
            d.worst_levels
        );
    }

    /// ⚠⚠ **The one tonal stage that deliberately does NOT preserve hue.** Curving the three
    /// channels independently is where a camera rendering's shadow saturation and its roll toward
    /// white come from. A shader that "fixed" that by curving luminance and scaling RGB would be
    /// a different stage wearing the same name — ⚠ and every other test here would still pass,
    /// because `spread()`'s neutral and near-neutral samples cannot see it.
    #[test]
    fn a_saturated_pixel_shifts_hue_exactly_as_the_cpu_does() {
        let Some(ctx) = ctx() else { return };
        // A deep saturated red in shadow: the region where a per-channel curve and a
        // luminance-preserving one disagree most.
        let img = ImageBuf::from_vec(1, 1, vec![0.22, 0.02, 0.015]);
        let cpu = cpu_of().apply(img.clone());
        let gpu = chained(&ctx, img.clone());
        let d = crate::compare(&cpu, &gpu);
        assert!(d.worst_levels < 0.1, "worst {} levels", d.worst_levels);
        // ⚠ The fixture check: the two implementations must have something to disagree about.
        // A ratio preserved exactly would mean the CPU stage is hue-preserving after all, and
        // this test would be asserting nothing.
        let (i, o) = (img.data(), gpu.data());
        let before = i[0] / i[1];
        let after = o[0] / o[1];
        assert!(
            (before - after).abs() > 0.5,
            "the channel ratio must move: {before} -> {after}"
        );
    }
```

- [ ] **Step 3: Run them and watch them fail**

Run: `cargo test --release -p gpu-pipeline display_rendering 2>&1 | tail -5` — FAIL to compile.

- [ ] **Step 4: Write the stage**

Shader body, using the `luma_wgsl` prelude from Task 4:

```wgsl
    let p = pixel * 3u;
    let c = vec3<f32>(data[p], data[p + 1u], data[p + 2u]);
    // ⚠ Per channel, not on luminance with a scalar gain -- see the stage's doc comment.
    let o = vec3<f32>(curve(c.x), curve(c.y), curve(c.z));
    let y = prophoto_luminance(o.x, o.y, o.z);
    // ⚠ 1 - y^4 is flat through the midtones, and the shadow knee is there because what reads as
    // saturation near black is chroma NOISE.
    let shadow = min(y / params.shadow_knee, 1.0);
    let s = 1.0 + (params.chroma_boost - 1.0) * shadow * (1.0 - y * y * y * y);
    // ⚠ Clamped: a bright saturated pixel scaled up can leave [0,1], and one channel clipping
    // while its neighbours do not would shift the hue.
    data[p]      = clamp(y + (o.x - y) * s, 0.0, 1.0);
    data[p + 1u] = clamp(y + (o.y - y) * s, 0.0, 1.0);
    data[p + 2u] = clamp(y + (o.z - y) * s, 0.0, 1.0);
```

with `curve(x)` a table lookup that mirrors `DisplayRenderingStage::map` exactly, including:
⚠ a non-finite or non-positive input reads the **first** entry, and anything past `lut_max` takes
the last, which the curve has already flattened to 1.

`CHROMA_BOOST` and `SHADOW_KNEE` must be `pub` in the CPU stage and passed as uniforms, **not**
re-typed into WGSL.

`cpu_cost` is **25.7**, from `probe_stages`.

- [ ] **Step 5: Run, sabotage, gate, commit**

Sabotage: replace the per-channel curve with a luminance curve plus a scalar gain. Expected: the
hue test and the equivalence test both fail. Restore from a `cp` backup.

```bash
git add -A && git commit -m "feat: the display rendering as a compute shader"
```

---

### Task 6: `GpuOutputTransformStage`

**Files:**
- Create: `crates/gpu-pipeline/src/output_transform.rs`
- Modify: `crates/gpu-pipeline/src/lib.rs`, `accelerate.rs`
- Modify: `crates/color/src/output_space.rs` if the matrices are not already `pub`

- [ ] **Step 1: Write the failing tests**

```rust
    use color::OutputSpace;

    fn chained(ctx: &Arc<GpuContext>, space: OutputSpace, img: ImageBuf) -> ImageBuf {
        crate::GpuChain::new(
            ctx.clone(),
            vec![Box::new(GpuOutputTransformStage::new(space, ctx))],
        )
        .apply(img)
    }

    /// ⚠⚠ **WGSL's `pow` returns NaN for a negative base, and out-of-gamut colour reaches this
    /// stage with negative components BY DESIGN** — that is the point of a wide working space.
    /// The CPU's `encode` clamps to [0,1] first, so the shader must clamp first too. ⚠ An
    /// in-gamut fixture cannot see this, because after the matrix every component is already
    /// positive — which is exactly how it would ship.
    #[test]
    fn an_out_of_gamut_colour_does_not_become_nan() {
        let Some(ctx) = ctx() else { return };
        // A saturated ProPhoto green: outside sRGB, so the matrix gives it a negative red.
        let img = ImageBuf::from_vec(1, 1, vec![0.05, 0.9, 0.08]);
        // ⚠ The fixture check first: if this is not actually out of gamut the test proves
        // nothing, and which colours fall outside sRGB is not obvious by eye.
        let lin = OutputSpace::Srgb.from_working([0.05, 0.9, 0.08]);
        assert!(
            lin.iter().any(|c| *c < 0.0),
            "fixture must be out of gamut, got {lin:?}"
        );
        let gpu = chained(&ctx, OutputSpace::Srgb, img.clone());
        assert!(
            gpu.data().iter().all(|v| v.is_finite()),
            "out-of-gamut colour must stay finite, got {:?}",
            gpu.data()
        );
        // ⚠ Finite alone would pass against a shader that zeroed everything.
        let cpu = pipeline::OutputTransform::new(OutputSpace::Srgb).apply(img);
        let d = crate::compare(&cpu, &gpu);
        assert!(d.worst_levels < 0.1, "worst {} levels", d.worst_levels);
    }

    /// ⚠ All four spaces: three transfer curves and four matrices are seven chances to wire the
    /// wrong one, and ProPhoto's matrix must be exactly the identity.
    ///
    /// ⚠⚠ The fixture is **coloured**, not a grey ramp. A neutral survives every one of the four
    /// matrices nearly unchanged, so a test that used one would pass with any two of them swapped
    /// — the same blindness that let a row swap through the P3 matrix in the display-profile work.
    #[test]
    fn every_output_space_matches_the_cpu() {
        let Some(ctx) = ctx() else { return };
        for space in [
            OutputSpace::Srgb,
            OutputSpace::DisplayP3,
            OutputSpace::AdobeRgb,
            OutputSpace::ProPhoto,
        ] {
            let img = ImageBuf::from_vec(6, 1, spread());
            let cpu = pipeline::OutputTransform::new(space).apply(img.clone());
            let gpu = chained(&ctx, space, img);
            let d = crate::compare(&cpu, &gpu);
            assert!(
                d.worst_levels < 0.1,
                "{space:?}: worst {} levels over {} of {} samples",
                d.worst_levels,
                d.differing,
                d.total
            );
        }
    }

    /// ⚠ And the spaces must not all agree, or the loop above would pass with one matrix wired
    /// four times.
    #[test]
    fn the_four_spaces_differ_from_one_another() {
        let Some(ctx) = ctx() else { return };
        let img = ImageBuf::from_vec(6, 1, spread());
        let srgb = chained(&ctx, OutputSpace::Srgb, img.clone());
        for other in [
            OutputSpace::DisplayP3,
            OutputSpace::AdobeRgb,
            OutputSpace::ProPhoto,
        ] {
            let d = crate::compare(&srgb, &chained(&ctx, other, img.clone()));
            assert!(
                d.worst_levels > 1.0,
                "{other:?} renders the same as sRGB; moved {} levels",
                d.worst_levels
            );
        }
    }
```

- [ ] **Step 2: Run them and watch them fail**

Run: `cargo test --release -p gpu-pipeline output_transform 2>&1 | tail -5` — FAIL to compile.

- [ ] **Step 3: Write the stage**

The matrix goes in the uniform as three `vec4<f32>` rows (⚠ WGSL's std140-ish alignment: a
`mat3x3` in a uniform is padded to three `vec4`s anyway, so writing rows explicitly avoids a
silent layout mismatch). The transfer is selected by a `u32` in the uniform:

```wgsl
fn encode_channel(v: f32, mode: u32) -> f32 {
    // ⚠⚠ Clamp FIRST. `pow` is NaN for a negative base and out-of-gamut colour is negative here.
    let c = clamp(v, 0.0, 1.0);
    if (mode == 0u) {           // sRGB and Display P3 share this curve -- gamma 2.2 is a common
                                // and invisible error.
        if (c <= 0.0031308) { return c * 12.92; }
        return 1.055 * pow(c, 1.0 / 2.4) - 0.055;
    }
    if (mode == 1u) { return pow(c, 1.0 / ADOBE_RGB_GAMMA); }
    if (c < PROPHOTO_TOE_END) { return c * 16.0; }
    return pow(c, 1.0 / 1.8);
}
```

⚠ `cpu_cost` is **28.6** — the largest in the tail, and the reason the gate exists.

⚠⚠ The stage name must be `"output_transform"`, matching what M8.3b's rename left in
`OutputTransform::name`. Check it rather than assuming; a table row keyed on a name nothing
produces is silently inert, and every test would still pass.

- [ ] **Step 4: Run, sabotage, gate, commit**

Sabotage 1: drop the clamp inside `encode_channel`. Expected: the out-of-gamut test fails.
Sabotage 2: use gamma 2.2 for sRGB. Expected: `every_output_space_matches_the_cpu` fails.
Restore from `cp` backups between each.

```bash
git add -A && git commit -m "feat: the output transform as a compute shader, the largest stage in the tail"
```

---

### Task 7: Measure it, and say what it bought

**Files:** Modify `crates/gpu-pipeline/examples/gpu_run_bench.rs`, `crates/app/examples/bench_gpu_tail.rs`.

- [ ] **Step 1: Add the typical-tail row to both benches**

`gpu_run_bench` gets a configuration of the four stages; `bench_gpu_tail` already has a
`TYPICAL of this catalogue` case — ⚠ **it must be given `rendering: Some(Camera)`**, which it
currently lacks, or it measures a tail the user does not have.

- [ ] **Step 2: Run both on a quiet machine**

```bash
cargo run --release -p gpu-pipeline --example gpu_run_bench
cargo run --release -p app --example bench_gpu_tail ~/Pictures/fuji/DSCF6291.RAF
```
⚠⚠ **Nothing else running.** M8.3c's first bench read 49.55 ms of CPU for *less* work than a later
37.97 ms because `cargo test --workspace` was saturating the cores; it was discarded rather than
quoted.

- [ ] **Step 3: Record the numbers, including the ones that disappoint**

Expected ~1.9–2.1× at rung 3200. ⚠ If the measured win is materially below that, **say so and
find out why** rather than quoting the projection — the likeliest cause is the per-encode shader
rebuild, now paid eight times rather than three, and the answer is a pipeline cache.

- [ ] **Step 4: The full gate, each exit code from its own file**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
L=/tmp/m83e-final-$$
cargo fmt --all --check > $L.fmt 2>&1; echo "FMT=$?"
cargo clippy --all-targets --all-features -- -D warnings > $L.clippy 2>&1; echo "CLIPPY=$?"
cargo test --workspace > $L.test 2>&1; echo "TEST=$?"
grep -c "^test golden_" $L.test        # ⚠ must be 63
grep -c "SKIPPED" $L.test              # ⚠ must be 0 -- a skipped GPU test is not a passing one
```
⚠ All three green **together**, on the same tree. The goldens run on the CPU, so none may move.

- [ ] **Step 5: Launch the GUI**

```bash
cargo run --release -p app &
```
Say what to look at: **an ordinary photo with only exposure/whites/blacks and the camera
rendering** — previously the case the GPU setting did nothing for, now the case it does the most
for. The picture must not change.

- [ ] **Step 6: Record it in `CLAUDE.md` and commit**

⚠ Including the accepted caveats, the neutral-frame case the gate deliberately declines, and the
8-bit readback this port makes possible and does not take.
