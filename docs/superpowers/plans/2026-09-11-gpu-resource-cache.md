# M8.3f — a GPU resource cache: Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this
> plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> ⚠⚠ **Conventions that are not negotiable in this repository:** commit messages carry **no
> `Co-Authored-By` trailer**; **never push** — the user pushes himself; work on the feature branch
> `m8.3f-pipeline-cache`, which already exists.

**Goal:** Stop rebuilding, on every render, the wgpu objects the previous render already built —
a shader module and compute pipeline per stage, and an 82 MB staging buffer.

**Architecture:** A `GpuCache` owned by the process-wide `GpuContext`, because `accelerate`
rebuilds every `GpuChain` and `GpuStage` per render and nothing shorter-lived survives. Two halves:
programs keyed on the shader **text**, and one staging buffer grown to the largest seen.

**Tech Stack:** Rust, `wgpu` 29.0.4, WGSL compute shaders.

**Spec:** `docs/superpowers/specs/2026-09-11-gpu-resource-cache-design.md`

---

## Measured baseline this plan is judged against

`gpu_run_bench` at 3200×2136 (6.8 Mpx, 82 MB), TYPICAL four-stage chain, warm rounds:

| item | ms |
|---|---|
| staging buffer allocation | **7.2** |
| GPU wait | 21.5 |
| readback `to_vec` | 6.5 |
| memcpy into staging | 2.6 |
| encode, all four stages | **1.2** |
| image + readback buffer allocation | 0.07 |
| submit | 0.07 |

`gpu_run_bench` TYPICAL currently reads **cpu 70.05 ms / gpu 41.70 ms — 1.68×**.
`bench_gpu_tail` TYPICAL currently reads **79.58 / 51.78 — 1.54×**.

Prediction to be checked in Task 6, not asserted: ~8.4 ms removed, `gpu_run_bench` TYPICAL ≈ 2.1×.

## ⚠ Three encode sites, not two

Found while planning, and it changes the task list:

1. `Dispatch::encode` — serves `hsl`, `grading`, `tone_regions`, `display_rendering`,
   `output_transform`.
2. `GpuSharpenStage::encode` — its own path; **one module and up to eight pipelines** per encode.
3. `GpuExposureStage::pipeline_and_bind` — **its own path too.** Written at M8.3a before `Dispatch`
   existed; M8.3e only made exposure *replaceable*, never a `Dispatch` client. This is why it never
   appeared in the probe's per-stage lines, and by subtraction it is **~0.6 ms of the 1.17 ms**
   encode — half the total. Task 2 folds it into `Dispatch`, which both caches it and deletes ~90
   lines of duplication.

## File structure

| file | responsibility | change |
|---|---|---|
| `crates/gpu-pipeline/src/cache.rs` | the cache: programs + one staging buffer | **create** (~170 lines) |
| `crates/gpu-pipeline/src/lib.rs` | module list | add `mod cache;` |
| `crates/gpu-pipeline/src/context.rs` | owns the cache | add one field |
| `crates/gpu-pipeline/src/dispatch.rs` | per-pixel dispatch | look the program up |
| `crates/gpu-pipeline/src/exposure.rs` | exposure stage | become a `Dispatch` client |
| `crates/gpu-pipeline/src/sharpen.rs` | sharpening stage | look the program up |
| `crates/gpu-pipeline/src/chain.rs` | the run | use the cached staging buffer |
| `CLAUDE.md` | the record | Task 6 |

## The gate, used before every commit

⚠⚠ **Each exit code is read from its own session-unique log file, never through a pipe, and never
sequenced with `&&` or `;` before a commit.** This repo records at least sixteen commits that went
in with clippy failing because the gate was chained to the commit.

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
S=$$-$(date +%s)
cargo fmt --all > /tmp/rr-fmt-$S.log 2>&1; echo "FMT=$?" > /tmp/rr-gate-$S.log
cargo clippy --all-targets --all-features -- -D warnings > /tmp/rr-clippy-$S.log 2>&1; echo "CLIPPY=$?" >> /tmp/rr-gate-$S.log
cargo test --workspace > /tmp/rr-test-$S.log 2>&1; echo "TEST=$?" >> /tmp/rr-gate-$S.log
grep -c "^test result: ok" /tmp/rr-test-$S.log >> /tmp/rr-gate-$S.log
cat /tmp/rr-gate-$S.log
```

Commit **only** when FMT, CLIPPY and TEST all read 0.

⚠ **Iterate with `cargo test --release -p gpu-pipeline`.** Acquiring a GPU adapter costs 44–75 s in
a debug build against 21 ms in release; the full workspace gate pays that once per commit and is
worth it, but not once per edit.

---

## ⚠⚠ Two corrections found by executing this plan, recorded in place

1. **Task 4 had to merge into Task 1.** The plan applied the dead-code rule to `program` and not to
   `staging`, which has no production caller until Task 4 — so `-D warnings` rejected the lib with
   *"method `staging` is never used"* and *"fields `staging` and `allocations` are never read"*.
   The rule was right and under-applied. `chain.rs`'s staging change now lands in Task 1, and
   **Task 4 below is struck through**.
   ⚠ The `misses` field needed `#[cfg(test)]` on the field itself: it is read only from
   `#[cfg(test)]` code, so it is dead in the lib target **for ever**, not merely until Task 4.

2. **⚠⚠ One staging buffer was WRONG, and the parallel test suite found it as 28 failures.**
   `Buffer is already mapped`, plus wrong pixels in `a_subnormal_result_is_flushed_to_zero_on_the_gpu`.
   The cache is process-wide while rendering is not — Compare gives each pane its own
   `RenderEngine` on its own thread and the full-resolution worker is a third, so two renders took
   the same buffer and trampled each other. **This would have been a real corruption bug in
   Compare, not a test artefact.** The single slot is now a `Vec` pool with `take_staging` /
   `put_staging`, capped at `MAX_STAGING = 4`, returned only after `poll(Wait)` confirms the
   submission finished. `two_renders_in_flight_never_share_a_buffer` is the regression guard.
   ⚠ "Grow, never shrink" is unchanged: a buffer too small for the render asking is dropped.

## Task 1: `GpuCache`, `Dispatch` and the pooled staging buffer

**Files:**
- Create: `crates/gpu-pipeline/src/cache.rs`
- Modify: `crates/gpu-pipeline/src/lib.rs`
- Modify: `crates/gpu-pipeline/src/context.rs`
- Modify: `crates/gpu-pipeline/src/dispatch.rs`

⚠⚠ **The cache and its first caller land in ONE commit, deliberately.** `GpuCache` is `pub(crate)`
in a library crate, and a `pub(crate)` item with no production caller is dead code that fails
`clippy -D warnings`. This repo records that trap ten times, including M8.3a's Tasks 1 and 2 having
to be squashed for exactly this reason. `#[cfg(test)]` callers do not count.

- [ ] **Step 1: Write `crates/gpu-pipeline/src/cache.rs`**

```rust
//! The wgpu objects that survive a render.
//!
//! ⚠⚠ **Owned by [`crate::GpuContext`] because nothing shorter-lived would work.** `accelerate`
//! rebuilds `GpuChain` and every `GpuStage` box on **every render**, so a cache held by a chain or
//! by a stage is discarded each frame and buys exactly nothing. The context is the process-wide
//! `OnceLock` and is the only thing in this crate that outlives a render.
//!
//! ⚠⚠ **`CLAUDE.md` was wrong by ~8x about which object this milestone is for.** It recorded the
//! gap between M8.3e's 1.54x and its projected ~2x as *"the shader module, the pipeline and every
//! uniform and LUT buffer being recreated on every encode"*, ~10 ms — flagged there as a
//! consistent explanation rather than a measured one. Measured at 3200x2136 (82 MB), TYPICAL
//! four-stage chain, warm: **the whole per-encode cost is 1.2 ms** and **the staging buffer
//! allocation alone is 7.2 ms**. The image buffer (82 MB) costs 0.05 ms and the readback buffer
//! 0.02 — same size, same device. It is the **mapping** that is expensive, not the allocation.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};

/// Everything compiled from one shader's text.
#[derive(Debug)]
struct Program {
    layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    module: wgpu::ShaderModule,
    pipelines: HashMap<&'static str, wgpu::ComputePipeline>,
    misses: usize,
}

/// The wgpu objects that outlive a render.
#[derive(Debug, Default)]
pub(crate) struct GpuCache {
    programs: Mutex<HashMap<String, Program>>,
    staging: Mutex<Option<wgpu::Buffer>>,
    allocations: AtomicUsize,
}

impl GpuCache {
    /// ⚠ A poisoned lock is **recovered from rather than panicked on**: a GPU problem must never
    /// be a reason somebody cannot edit a photograph, the same posture `GpuContext::new` takes in
    /// returning `Option` rather than `Result`.
    fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
        m.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The bind group layout for `shader`, and one compute pipeline per entry point in
    /// `entry_points`, compiling only what is missing.
    ///
    /// ⚠⚠ **Keyed on the shader TEXT, never on `label`.** Every shader in this crate today is a
    /// constant prelude plus a constant body — `luma_wgsl()` and sharpen's `.replace(..)`
    /// substitute *compile-time* constants — so no shader text varies with the edits and a label
    /// key would work **today**. It would then break silently the first time a stage bakes a
    /// parameter into its WGSL, serving a stale pipeline and a plausible-but-wrong picture.
    /// Hashing 2-4 KB costs ~1 us against the 0.10 ms module creation it removes. Fail toward a
    /// miss — `PrefixKey`'s rule.
    ///
    /// ⚠ The layout is held under the **shader** key rather than under a shape key, because
    /// `GpuSharpenStage`'s layout differs from `Dispatch`'s and a shape key would collide them.
    pub(crate) fn program(
        &self,
        device: &wgpu::Device,
        label: &'static str,
        shader: &str,
        layout_entries: &[wgpu::BindGroupLayoutEntry],
        entry_points: &[&'static str],
    ) -> (wgpu::BindGroupLayout, Vec<wgpu::ComputePipeline>) {
        let mut map = Self::lock(&self.programs);
        if !map.contains_key(shader) {
            let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some(label),
                entries: layout_entries,
            });
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &[Some(&layout)],
                immediate_size: 0,
            });
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(shader.to_string().into()),
            });
            map.insert(
                shader.to_string(),
                Program {
                    layout,
                    pipeline_layout,
                    module,
                    pipelines: HashMap::new(),
                    misses: 1,
                },
            );
        }
        // ⚠ Destructured rather than used through `p.`, because `pipelines.entry(..)` takes a
        // mutable borrow while the closure needs `pipeline_layout` and `module` — two borrows of
        // one struct that only a destructuring split makes legal. The compiler says so.
        let Program {
            layout,
            pipeline_layout,
            module,
            pipelines,
            ..
        } = map.get_mut(shader).expect("inserted above when absent");
        let built = entry_points
            .iter()
            .map(|e| {
                pipelines
                    .entry(e)
                    .or_insert_with(|| {
                        device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                            label: Some(label),
                            layout: Some(pipeline_layout),
                            module,
                            entry_point: Some(e),
                            compilation_options: Default::default(),
                            cache: None,
                        })
                    })
                    .clone()
            })
            .collect();
        (layout.clone(), built)
    }

    /// A `MAP_WRITE` staging buffer of at least `bytes`, **mapped and ready to be written**.
    ///
    /// The caller writes into `buffer.slice(..bytes).get_mapped_range_mut()` and then `unmap()`s.
    ///
    /// ⚠ One buffer, grown to the largest seen and never shrunk (user's call, 2026-09-11). A
    /// larger buffer serving a smaller render is correct **by construction**, because the copy out
    /// of it is always given `bytes` rather than the buffer's own size.
    ///
    /// ⚠⚠ Reuse is safe **only because `GpuChain::run` is synchronous** — it blocks on
    /// `poll(Wait)` until readback completes, so the GPU is idle by the time the next render maps.
    /// Measured on a 78.2 MiB buffer: the map costs **0.00-0.01 ms** against an **8-10 ms**
    /// allocation. An asynchronous chain would need a ring of buffers and this reasoning would not
    /// survive it.
    pub(crate) fn staging(&self, device: &wgpu::Device, bytes: u64) -> Option<wgpu::Buffer> {
        let mut slot = Self::lock(&self.staging);
        if let Some(b) = slot.as_ref() {
            if b.size() >= bytes {
                let (tx, rx) = std::sync::mpsc::channel();
                b.slice(..bytes).map_async(wgpu::MapMode::Write, move |r| {
                    let _ = tx.send(r);
                });
                let _ = device.poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                });
                rx.recv().ok()?.ok()?;
                return Some(b.clone());
            }
        }
        let b = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("chain-staging"),
            size: bytes,
            usage: wgpu::BufferUsages::MAP_WRITE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: true,
        });
        self.allocations.fetch_add(1, Ordering::Relaxed);
        *slot = Some(b.clone());
        Some(b)
    }

    /// How many times this shader's **text** has been compiled.
    ///
    /// ⚠⚠ **Per key, never a total.** The cache is genuinely process-wide and `cargo test` runs in
    /// parallel, so a global count is precisely the contaminated observable `GpuChain`'s own
    /// comment records being burned by — process-wide transfer statics read `(6, 4)` where the
    /// truth was `(1, 1)`. A per-key count cannot be moved by another test's shader.
    ///
    /// ⚠ `#[cfg(test)]` is correct here and would be **wrong** for `RenderEngine::requests_issued`
    /// — that one is read from a different crate, which compiles the library without `cfg(test)`.
    /// Every caller of this one is in this crate.
    #[cfg(test)]
    pub(crate) fn misses_for(&self, shader: &str) -> usize {
        Self::lock(&self.programs).get(shader).map_or(0, |p| p.misses)
    }

    /// How many staging buffers have been allocated over this cache's life.
    #[cfg(test)]
    pub(crate) fn allocations(&self) -> usize {
        self.allocations.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two shaders that differ only in the constant, so a cache keyed on the **label** would serve
    /// the first where the second was asked for.
    const DOUBLE: &str = r#"
@group(0) @binding(0) var<storage, read_write> data: array<f32>;
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x < arrayLength(&data)) { data[id.x] = data[id.x] * 2.0; }
}
"#;
    const TRIPLE: &str = r#"
@group(0) @binding(0) var<storage, read_write> data: array<f32>;
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x < arrayLength(&data)) { data[id.x] = data[id.x] * 3.0; }
}
"#;

    fn entries() -> [wgpu::BindGroupLayoutEntry; 1] {
        [wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: false },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }]
    }

    #[test]
    fn a_second_lookup_of_the_same_shader_compiles_nothing() {
        let Some(ctx) = crate::shared() else {
            eprintln!("SKIPPED a_second_lookup_of_the_same_shader_compiles_nothing: no adapter");
            return;
        };
        let cache = GpuCache::default();
        cache.program(&ctx.device, "probe", DOUBLE, &entries(), &["main"]);
        cache.program(&ctx.device, "probe", DOUBLE, &entries(), &["main"]);
        assert_eq!(
            cache.misses_for(DOUBLE),
            1,
            "the second lookup compiled the shader again"
        );
    }

    #[test]
    fn two_shaders_sharing_a_label_get_their_own_programs() {
        // ⚠⚠ **The discriminating test, and the label is shared on purpose.** A cache keyed on
        // the label would store TRIPLE's lookup under DOUBLE's entry, so TRIPLE would never be
        // compiled at all and its miss count would read 0 — which is a stage silently rendering
        // through another stage's shader.
        let Some(ctx) = crate::shared() else {
            eprintln!("SKIPPED two_shaders_sharing_a_label_get_their_own_programs: no adapter");
            return;
        };
        let cache = GpuCache::default();
        cache.program(&ctx.device, "probe", DOUBLE, &entries(), &["main"]);
        cache.program(&ctx.device, "probe", TRIPLE, &entries(), &["main"]);
        assert_eq!(cache.misses_for(DOUBLE), 1, "DOUBLE was not compiled");
        assert_eq!(
            cache.misses_for(TRIPLE),
            1,
            "TRIPLE was never compiled, so the key is the label rather than the text"
        );
    }

    #[test]
    fn the_staging_buffer_grows_and_is_reused() {
        let Some(ctx) = crate::shared() else {
            eprintln!("SKIPPED the_staging_buffer_grows_and_is_reused: no adapter");
            return;
        };
        let cache = GpuCache::default();
        let d = &ctx.device;

        cache.staging(d, 4096).expect("first staging buffer").unmap();
        assert_eq!(cache.allocations(), 1);

        cache.staging(d, 4096).expect("same size again").unmap();
        assert_eq!(
            cache.allocations(),
            1,
            "a render of the same size allocated a second buffer"
        );

        cache.staging(d, 8192).expect("a larger render").unmap();
        assert_eq!(cache.allocations(), 2, "a larger render must allocate");

        cache.staging(d, 4096).expect("a smaller render").unmap();
        assert_eq!(
            cache.allocations(),
            2,
            "a smaller render must reuse the larger buffer rather than shrink"
        );
    }
}
```

- [ ] **Step 2: Add the module and the field**

In `crates/gpu-pipeline/src/lib.rs`, add `mod cache;` to the module list, keeping it alphabetical
(between `accelerate` and `chain`).

In `crates/gpu-pipeline/src/context.rs`, add the field to the struct:

```rust
pub struct GpuContext {
    pub(crate) device: wgpu::Device,
    pub(crate) queue: wgpu::Queue,
    /// ⚠ The wgpu objects that outlive a render. Here rather than on `GpuChain` because
    /// `accelerate` rebuilds every chain and stage per render — see `cache.rs`.
    pub(crate) cache: crate::cache::GpuCache,
    adapter_name: String,
    backend: String,
}
```

and to the constructor, after `queue`:

```rust
        Some(Self {
            device,
            queue,
            cache: Default::default(),
            adapter_name: info.name,
            backend: format!("{:?}", info.backend),
        })
```

- [ ] **Step 3: Make `Dispatch::encode` use the cache**

In `crates/gpu-pipeline/src/dispatch.rs`, replace the body of `encode` from `let module = …` down
to `let compute = …` with the version below. The layout entries move **above** the buffer creation,
because the cache needs them; everything else keeps its order.

```rust
    pub fn encode(self, ctx: &GpuContext, pass: &mut wgpu::ComputePass, img: &GpuImage) {
        let device = &ctx.device;
        let (row_groups, col_groups) = Self::grid(img.pixels());

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
        for i in 0..self.storages.len() as u32 {
            layout_entries.push(entry(
                2 + i,
                wgpu::BufferBindingType::Storage { read_only: true },
            ));
        }

        // ⚠⚠ The module, the layout and the pipeline come from the cache; the uniform, the
        // storages and the bind group do **not**. Measured at 3200x2136: the three cached objects
        // are the ~0.1-0.15 ms per stage that this removes, while the uniform and the LUTs cost
        // 0.01-0.03 ms and carry **this render's edits**, so caching them would add an
        // invalidation risk whose failure mode is a wrong picture rather than a slow one.
        let (layout, pipelines) = ctx.cache.program(
            device,
            self.label,
            &self.shader,
            &layout_entries,
            &["main"],
        );

        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(self.label),
            size: self.params.len() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        ctx.queue.write_buffer(&uniform, 0, &self.params);

        // ⚠ `.max(4)` because wgpu refuses a zero-sized buffer, the same guard `GpuImage::scratch`
        // carries — a stage may legitimately hand over an empty table.
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

        pass.set_pipeline(&pipelines[0]);
        pass.set_bind_group(0, &bind, &[]);
        pass.dispatch_workgroups(row_groups, col_groups, 1);
    }
```

- [ ] **Step 4: Run the gpu-pipeline tests**

Run: `export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"; cargo test --release -p gpu-pipeline`

Expected: PASS, including the three new `cache::tests`. ⚠ Read the **passed count**, not the word
`ok` — this repo records several runs that printed `ok` having run nothing.

- [ ] **Step 5: Run the full gate and commit**

Run the gate block from the top of this plan. Commit only when FMT, CLIPPY and TEST all read 0.

```bash
git add crates/gpu-pipeline/src/cache.rs crates/gpu-pipeline/src/lib.rs crates/gpu-pipeline/src/context.rs crates/gpu-pipeline/src/dispatch.rs
git commit -F - <<'EOF'
feat: the wgpu objects that survive a render, and Dispatch uses them

A GpuCache on GpuContext holding, per shader TEXT, the module, the bind group
layout and one compute pipeline per entry point; plus one staging buffer.

Owned by the context because nothing shorter-lived would work: accelerate
rebuilds GpuChain and every GpuStage box on every render, so a cache on a
chain or a stage is discarded each frame.

Keyed on the text rather than the label. Every shader here is a constant
prelude plus a constant body, so a label key works today and breaks silently
the first time a stage bakes a parameter into its WGSL -- a stale pipeline
and a plausible-but-wrong picture. Hashing 2-4 KB is ~1 us against the
0.10 ms module creation it removes.

The cache and its first caller land together: a pub(crate) item with no
production caller is dead code under -D warnings, and #[cfg(test)] callers
do not count.
EOF
```

---

## Task 2: `GpuExposureStage` becomes a `Dispatch` client

**Files:**
- Modify: `crates/gpu-pipeline/src/exposure.rs`

⚠ Exposure's shader is exactly `Dispatch`'s shape — binding 0 storage read-write, binding 1
uniform, entry point `main`, a 16-byte uniform, no storage tables. Folding it in deletes ~90 lines
of duplication *and* is what puts its ~0.6 ms behind the cache.

⚠⚠ **The acceptance criterion is bit-exactness.** Exposure's existing equivalence tests assert the
GPU result is **bit-identical** to the CPU stage. The shader text, the params and the grid are all
unchanged by this task, so those tests passing unchanged is the proof the fold moved no behaviour.
If any of them fails, the fold is wrong — do not touch the tolerance.

- [ ] **Step 1: Run the exposure tests first, to establish they pass before the change**

Run: `cargo test --release -p gpu-pipeline exposure`
Expected: PASS. Note the passed count; it must be identical after the change.

- [ ] **Step 2: Make `SHADER` reachable from the crate's tests**

In `crates/gpu-pipeline/src/exposure.rs`, change the declaration:

```rust
pub(crate) const SHADER: &str = r#"
```

⚠ Task 3's carrier test needs the exact cache key, and the key is this text. Deriving it a second
time in the test would be two constructions of one string that can disagree.

- [ ] **Step 3: Delete `pipeline_and_bind` and the two duplicated constants**

Delete from `crates/gpu-pipeline/src/exposure.rs`:
- the whole `fn pipeline_and_bind(..)` method and its doc comment,
- `const WORKGROUP: u32 = 64;`,
- `const MAX_GROUPS_PER_DIM: u32 = 65535;` **and its doc comment**.

⚠ Move that doc comment's content nowhere — `dispatch.rs` already carries the same warning about
the 65535 limit, verbatim in substance. Two copies of one finding is how they drift.

- [ ] **Step 4: Replace `encode`**

```rust
    fn encode(
        &self,
        ctx: &GpuContext,
        pass: &mut wgpu::ComputePass,
        img: &GpuImage,
        // ⚠ In-place and per-pixel, so it needs none — the default `scratch_planes`
        // of 0 already says so and the chain lends it an empty slice.
        _scratch: &[wgpu::Buffer],
    ) {
        // ⚠⚠ A `Dispatch` client since M8.3f. It was written at M8.3a, **before** `Dispatch`
        // existed, and M8.3e made it replaceable without folding it in — so it carried its own
        // ~90-line copy of the layout/bind/pipeline dance and was the one per-pixel stage the
        // shared path did not cover. Measured by subtraction at 3200x2136, that copy was ~0.6 ms
        // of the 1.17 ms per-render encode cost: half of it, for one stage of four.
        let (row_groups, _) = crate::dispatch::Dispatch::grid(img.pixels());
        let params: [u32; 4] = [self.factor.to_bits(), img.pixels(), row_groups, 0];
        crate::dispatch::Dispatch {
            label: "exposure",
            shader: SHADER.to_string(),
            params: bytemuck::cast_slice(&params).to_vec(),
            storages: &[],
        }
        .encode(ctx, pass, img);
    }
```

- [ ] **Step 5: Run the exposure tests and compare the count**

Run: `cargo test --release -p gpu-pipeline exposure`
Expected: PASS, with the **same passed count as Step 1**. The bit-exactness tests are the guard.

- [ ] **Step 6: Run the full gate and commit**

```bash
git add crates/gpu-pipeline/src/exposure.rs
git commit -F - <<'EOF'
refactor: exposure joins the shared dispatch, three milestones late

GpuExposureStage was written at M8.3a before Dispatch existed, and M8.3e made
it replaceable without folding it in -- so it carried its own ~90-line copy of
the layout/bind/pipeline dance and was the one per-pixel stage the shared path
did not cover. It never appeared in the encode probe's per-stage lines for that
reason, and by subtraction it was ~0.6 ms of the 1.17 ms per-render encode
cost: half of it, for one stage of four.

Its shader is exactly Dispatch's shape -- storage at 0, uniform at 1, entry
point main, a 16-byte uniform, no tables -- so the fold changes no shader text,
no params and no grid. The existing bit-exactness tests passing unchanged is
the proof it moved no behaviour.
EOF
```

---

## Task 3: the chain-level carrier tests

**Files:**
- Modify: `crates/gpu-pipeline/src/chain.rs` (tests only)

⚠⚠ **This is the task that matters most.** Every existing equivalence test already covers the
*correct* case, which is exactly why they are not the guard: the dangerous failure is a cache
serving a **stale** object, and no equivalence test can see that. This is the shape that let
creative LUTs ship fully tested and 100% non-functional.

- [ ] **Step 1: Write the two failing tests**

Append to the `mod tests` block in `crates/gpu-pipeline/src/chain.rs`:

```rust
    #[test]
    fn a_second_run_of_the_same_chain_compiles_nothing() {
        // ⚠⚠ The carrier. `cache.rs`'s own tests prove the cache caches; this proves the
        // production path **consults** it — the W5 lesson, where a milestone's maths was fully
        // tested and nothing proved the data ever reached it.
        //
        // ⚠ Asserted as a **delta around the second run** rather than an absolute count, because
        // the cache is process-wide and `cargo test` runs in parallel: another test using this
        // same shader would hit rather than miss, so the delta is 0 either way, while an absolute
        // count would depend on who got there first.
        let Some(ctx) = ctx() else { return };
        let chain = GpuChain::new(
            ctx.clone(),
            vec![Box::new(GpuExposureStage::new(1.0, ctx.clone()))],
        );
        let img = ramp(64, 64);
        let _ = chain.apply(img.clone());
        let before = ctx.cache.misses_for(crate::exposure::SHADER);
        assert!(
            before > 0,
            "fixture check: the first run compiled nothing, so this test proves nothing"
        );
        let _ = chain.apply(img);
        assert_eq!(
            ctx.cache.misses_for(crate::exposure::SHADER),
            before,
            "the second run of an identical chain recompiled its shader"
        );
    }

    #[test]
    fn two_renders_with_different_exposures_do_not_share_a_uniform() {
        // ⚠⚠ The most likely real bug in a caching change, and no equivalence test can see it: a
        // cached **uniform** would give the second render the first render's exposure. Both runs
        // use the identical shader text, so the program cache is hit in both — which is precisely
        // the condition under which a wrongly-cached uniform would go unnoticed.
        let Some(ctx) = ctx() else { return };
        let img = ramp(64, 64);
        let up = GpuChain::new(
            ctx.clone(),
            vec![Box::new(GpuExposureStage::new(1.0, ctx.clone()))],
        )
        .apply(img.clone());
        let down = GpuChain::new(
            ctx.clone(),
            vec![Box::new(GpuExposureStage::new(-1.0, ctx.clone()))],
        )
        .apply(img.clone());

        for (i, ((s, u), d)) in img
            .data()
            .iter()
            .zip(up.data())
            .zip(down.data())
            .enumerate()
        {
            assert_eq!(*u, s * 2.0, "sample {i} did not double");
            assert_eq!(*d, s * 0.5, "sample {i} took the previous render's exposure");
        }
    }

    #[test]
    fn a_small_render_after_a_large_one_is_still_correct() {
        // ⚠⚠ The guard on the grown staging buffer. Once a big render has allocated a large
        // buffer, a small render reuses it — and the copy out of it must be given the **image's**
        // bytes rather than the buffer's size. Getting that wrong is not a slow render, it is
        // reading 82 MB out of a 20 MB image.
        //
        // ⚠ It also asserts the GPU path actually ran: a chain that fell back to the CPU would
        // produce the same pixels and prove nothing about the buffer.
        let Some(ctx) = ctx() else { return };
        let big = ramp(256, 256);
        let _ = GpuChain::new(
            ctx.clone(),
            vec![Box::new(GpuExposureStage::new(1.0, ctx.clone()))],
        )
        .apply(big);

        let small = ramp(32, 16);
        let chain = GpuChain::new(
            ctx.clone(),
            vec![Box::new(GpuExposureStage::new(1.0, ctx.clone()))],
        );
        let out = chain.apply(small.clone());
        assert_eq!(
            chain.transfer_counts(),
            (1, 1),
            "fixture check: the chain fell back to the CPU, so this proves nothing about the buffer"
        );
        assert_eq!(out.width(), 32);
        assert_eq!(out.height(), 16);
        for (i, (s, o)) in small.data().iter().zip(out.data()).enumerate() {
            assert_eq!(*o, s * 2.0, "sample {i} is wrong after a larger render");
        }
    }
```

- [ ] **Step 2: Run them — the first two must PASS, the third must PASS**

Run: `cargo test --release -p gpu-pipeline chain::tests`

Expected: all PASS. ⚠ Task 4 is what makes the third one meaningful; until the staging buffer is
cached it passes trivially, and the step below records that rather than pretending otherwise.

- [ ] **Step 3: Commit**

Run the gate, then:

```bash
git add crates/gpu-pipeline/src/chain.rs
git commit -F - <<'EOF'
test: the carriers for the resource cache

cache.rs's own tests prove the cache caches; these prove the production path
consults it, and that what must NOT be cached is not.

- a second run of the same chain compiles nothing, asserted as a delta around
  the second run because the cache is process-wide and cargo test runs in
  parallel;
- two renders at different exposures do not share a uniform -- the most likely
  real bug in a caching change, and one no equivalence test can see, since
  both runs hit the program cache by construction;
- a small render after a large one is still correct, which is the guard on the
  grown staging buffer and currently passes trivially, before Task 4.
EOF
```

---

## ~~Task 4: the cached staging buffer~~ — MERGED INTO TASK 1, see the corrections above

**Files:**
- Modify: `crates/gpu-pipeline/src/chain.rs`

- [ ] **Step 1: Replace the staging block in `GpuChain::run`**

Replace this:

```rust
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
```

with this:

```rust
        // ⚠⚠ **Cached, and this is the largest single saving in M8.3f.** Measured at 3200x2136:
        // creating this buffer fresh costs **7.2 ms per render**, while the 82 MB image buffer
        // costs 0.05 ms and the 82 MB readback buffer 0.02 — same size, same device. It is
        // `mapped_at_creation: true` that is expensive, not the allocation, and reuse replaces it
        // with a `map_async` measured at **0.00-0.01 ms**.
        //
        // ⚠ `slice(..bytes)` rather than `slice(..)`: the cached buffer may be **larger** than
        // this render needs, and the copy below is likewise given `bytes`. That is what makes a
        // grown buffer serving a smaller render correct by construction.
        let staging = ctx.cache.staging(device, bytes)?;
        staging
            .slice(..bytes)
            .get_mapped_range_mut()
            .copy_from_slice(bytemuck::cast_slice(image.data()));
        staging.unmap();
```

⚠ The `enc.copy_buffer_to_buffer(&staging, 0, &img.buffer, 0, bytes)` line below is **already**
given `bytes` and needs no change — which is what `a_small_render_after_a_large_one_is_still_correct`
is guarding.

- [ ] **Step 2: Run the gpu-pipeline tests**

Run: `cargo test --release -p gpu-pipeline`
Expected: PASS, all of them, including the equivalence tests. A wrong staging size shows up as
either a wgpu validation panic or wrong pixels.

- [ ] **Step 3: Run the goldens**

Run: `cargo test --release -p pipeline --test golden 2>&1 | tail -5`
Expected: **63 passed**. ⚠ Read the count; the goldens never touch this crate, so a change here
moving one would mean something very wrong.

- [ ] **Step 4: Run the full gate and commit**

```bash
git add crates/gpu-pipeline/src/chain.rs
git commit -F - <<'EOF'
feat: the staging buffer survives the render, worth 7.2 ms

Measured at 3200x2136: creating the 82 MB MAP_WRITE staging buffer fresh costs
7.2 ms per render, where the 82 MB image buffer costs 0.05 ms and the 82 MB
readback buffer 0.02 -- same size, same device. It is mapped_at_creation: true
that is expensive, not the allocation.

Reuse replaces it with a map_async measured at 0.00-0.01 ms. That is safe only
because run() is synchronous: it blocks on poll(Wait) until readback completes,
so the GPU is idle by the time the next render maps. An asynchronous chain
would need a ring of buffers.

One buffer, grown to the largest seen and never shrunk (user's call). A larger
buffer serving a smaller render is correct by construction, because both the
mapped slice and the copy are given the image's bytes rather than the buffer's
size.
EOF
```

---

## Task 5: `GpuSharpenStage` uses the cache

**Files:**
- Modify: `crates/gpu-pipeline/src/sharpen.rs`

⚠ Sharpen builds **one module and up to eight compute pipelines** per encode — more pipeline
objects than every other stage combined.

- [ ] **Step 1: Move `names` above the layout, and replace the module/layout/pipeline creation**

In `GpuSharpenStage::encode`, delete the `let module = device.create_shader_module(..)` block, the
`let layout = device.create_bind_group_layout(..)` block and the
`let pipeline_layout = device.create_pipeline_layout(..)` block, and delete the
`device.create_compute_pipeline(..)` call inside the `for name in names` loop.

Insert, immediately after the `let src = SHADER.replace(..)…;` chain:

```rust
        // ⚠ The mask passes sit between `difference` and `apply_gain` because `apply_gain` reads
        // the mask and `magnitude` reads the difference — the order is the data dependency, not a
        // convention.
        let names: Vec<&'static str> = if self.masking {
            PASSES[..4]
                .iter()
                .chain(MASK_PASSES.iter())
                .chain(PASSES[4..].iter())
                .copied()
                .collect()
        } else {
            PASSES.to_vec()
        };

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
        let ro = wgpu::BufferBindingType::Storage { read_only: true };

        // ⚠⚠ One lookup for the module, the layout and **all eight** pipelines. Sharpen creates
        // more pipeline objects per encode than every other stage put together, so it is the
        // largest single beneficiary of the program cache even though it is not a `Dispatch`
        // client — it encodes eight dispatches over shared scratch planes, which is a different
        // shape wearing a similar name.
        //
        // ⚠ The masking and non-masking cases ask for different entry-point lists against the
        // **same** shader text, which the cache handles: the entry is shared and the three mask
        // pipelines are compiled the first time masking is on.
        let (layout, pipelines) = ctx.cache.program(
            &ctx.device,
            "sharpen",
            &src,
            &[
                entry(0, rw),
                entry(1, wgpu::BufferBindingType::Uniform),
                entry(2, ro),
                entry(3, ro),
                entry(4, rw),
                entry(5, rw),
                entry(6, rw),
                entry(7, rw),
            ],
            &names,
        );
```

⚠ The existing `let entry = …`, `let rw = …` and `let ro = …` bindings further down are now
duplicates — delete those, keeping only the copies introduced above. The `let bound = [..]` array
and the `let bind = device.create_bind_group(..)` call are unchanged and still use `&layout`.

Replace the dispatch loop at the end of `encode` with:

```rust
        for compute in &pipelines {
            pass.set_pipeline(compute);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(row_groups, col_groups, 1);
        }
```

- [ ] **Step 2: Run the sharpen tests**

Run: `cargo test --release -p gpu-pipeline sharpen`
Expected: PASS, same count as before. The equivalence tests compare against the CPU stage, and the
pass **order** is what they would catch if the list were rebuilt wrongly.

- [ ] **Step 3: Run the full gate and commit**

```bash
git add crates/gpu-pipeline/src/sharpen.rs
git commit -F - <<'EOF'
feat: sharpening's eight pipelines come from the cache

Sharpen builds one module and up to eight compute pipelines per encode, more
pipeline objects than every other stage put together, so it is the largest
single beneficiary of the program cache -- despite not being a Dispatch client,
since it encodes eight dispatches over shared scratch planes.

The masking and non-masking cases ask for different entry-point lists against
the same shader text; the cache shares the entry and compiles the three mask
pipelines the first time masking is on.
EOF
```

---

## Task 6: sabotage, measure, record

**Files:**
- Modify: `CLAUDE.md`

⚠⚠ **Take a `cp` backup before every sabotage and restore from it.** Never
`git checkout -- <file>` — this repo has destroyed uncommitted work that way twice, and both
incidents are recorded.

- [ ] **Step 1: Sabotage — key the cache on the label**

```bash
cp crates/gpu-pipeline/src/cache.rs /tmp/cache.rs.bak
```

In `GpuCache::program`, replace every use of `shader` as the key with `label` (the three sites:
`contains_key`, `insert`, `get_mut`), and in `misses_for` leave the lookup as it is.

Run: `cargo test --release -p gpu-pipeline cache::tests`
Expected: **FAIL** — `two_shaders_sharing_a_label_get_their_own_programs`, with
"TRIPLE was never compiled, so the key is the label rather than the text".

Restore: `cp /tmp/cache.rs.bak crates/gpu-pipeline/src/cache.rs` then
`touch crates/gpu-pipeline/src/cache.rs`

⚠ The `touch` is not optional: `cp` can give the file an older mtime than the artifact built from
the sabotaged source, and cargo will then reuse that artifact. This repo records exactly that
producing seven failures against correct code.

- [ ] **Step 2: Sabotage — never consult the cache**

```bash
cp crates/gpu-pipeline/src/cache.rs /tmp/cache.rs.bak
```

In `GpuCache::program`, change `if !map.contains_key(shader)` to `if true`.

Run: `cargo test --release -p gpu-pipeline chain::tests`
Expected: **FAIL** — `a_second_run_of_the_same_chain_compiles_nothing`.

Restore and `touch` as above.

- [ ] **Step 3: Sabotage — copy the buffer's size rather than the image's**

```bash
cp crates/gpu-pipeline/src/chain.rs /tmp/chain.rs.bak
```

In `GpuChain::run`, change `staging.slice(..bytes)` to `staging.slice(..)`.

Run: `cargo test --release -p gpu-pipeline chain::tests`
Expected: **FAIL** — `a_small_render_after_a_large_one_is_still_correct`, either on the
`copy_from_slice` length mismatch or on the transfer-count fixture check.

Restore and `touch` as above.

- [ ] **Step 4: Sabotage — skip the map wait**

```bash
cp crates/gpu-pipeline/src/cache.rs /tmp/cache.rs.bak
```

In `GpuCache::staging`, delete the `let _ = device.poll(..)` line.

Run: `cargo test --release -p gpu-pipeline`

⚠⚠ **This one is EXPECTED TO BE INERT or to hang, and either outcome is a finding rather than a
formality.** Record which it was in the commit message. A sabotage that does not fail says
something about the code — here, either that the map completes synchronously on this backend, or
that `get_mapped_range_mut` panics on an unmapped buffer and the failure is loud but not the
named test's. Do not invent a test to cover it.

Restore and `touch` as above. Then confirm the tree is clean: `git status --short` must be empty.

- [ ] **Step 5: Measure — the synthetic bench**

⚠ Check the machine is quiet first (`ps aux | sort -k3 -rn | head -3`): this repo records a bench
run discarded for contention, and M8.3e's own table carries a caveat about an unrelated `julia`
job holding a core.

Run: `cargo run --release -p gpu-pipeline --example gpu_run_bench 3200 2136`

Record the TYPICAL row's warm best-of-4 against the **1.68×** baseline at the top of this plan.

- [ ] **Step 6: Measure — the real engine**

Run: `cargo run --release -p app --example bench_gpu_tail ~/Pictures/fuji/DSCF6291.RAF`

Record every row against M8.3e's table (TYPICAL 79.58 / 51.78 — 1.54×).

⚠⚠ **This is the number the milestone is judged by, not the synthetic one.** They disagree
(70.05 / 41.70 against 79.58 / 51.78), and this project has twice been misled by quoting a
projection as a result.

- [ ] **Step 7: Record it in `CLAUDE.md`**

Insert an `**M8.3f — a GPU resource cache (2026-09-11).**` section immediately **before** the
`**M8.3e — the per-pixel tail on the GPU` section, and update the RESUME block's *State at
handoff* and *NEXT ACTION*.

⚠ It must record, because each one is a finding rather than a feature note:

1. **`CLAUDE.md`'s own ~10 ms accounting was wrong by ~8× about which object**, and the
   measurement that reversed it — with the full table.
2. **`mapped_at_creation: true` is what costs 7.2 ms**, proved by the 82 MB image buffer costing
   0.05 ms and the 82 MB readback buffer 0.02.
3. **The central risk was probed before designing**: `map_async(Write)` at 0.00–0.01 ms against an
   8–10 ms allocation, and that it is safe **only** because `run()` is synchronous.
4. **`GpuExposureStage` was never a `Dispatch` client** — written at M8.3a, made replaceable at
   M8.3e, folded in here; ~0.6 ms of the 1.17 ms encode for one stage of four.
5. **The cache lives on `GpuContext` because `accelerate` rebuilds every chain and stage per
   render** — a placement that is forced rather than chosen.
6. **Keyed on the shader text, never the label**, and why a label key would work today and fail
   silently later.
7. **Per-key miss counts rather than a total**, and that this is the contaminated-observable trap
   `GpuChain::transfer_counts` already records.
8. The **measured** result from Step 6, and whether it met the ~2.1× prediction.
9. The sabotage outcomes, **including the inert one from Step 4**.
10. **Accepted caveats**, from spec §10: ~82 MB resident and never released; nothing helps export;
    `gpu_tail` still off by default; the first render still compiles shaders and `wgpu::PipelineCache`
    is not taken; and ⚠⚠ **the 8-bit readback is still worth more than this whole milestone** —
    21.5 ms of GPU wait and 6.5 ms of readback copy are untouched.

- [ ] **Step 8: Launch the GUI and commit**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
nohup cargo run --release -p app > /tmp/rustroom-gui.log 2>&1 &
```

⚠ Say what to look at: **an ordinary photo with only exposure/whites/blacks and the camera
rendering, with `gpu_tail` on** — the picture must not change, and it should be faster than M8.3e.
A caching bug shows as the *previous* photo's edits appearing, which is why the "different
exposures do not share a uniform" test exists.

Run the full gate, then:

```bash
git add CLAUDE.md
git commit -F - <<'EOF'
docs: M8.3f recorded, and the accounting it corrects

<the measured numbers from Steps 5 and 6, the four sabotage outcomes including
the inert one, and the caveats -- written when the numbers are in hand>
EOF
```

---

## Self-review

**Spec coverage.** §2 the measurement → recorded in Task 6 Step 7. §2.1 the probe → Task 6 Step 7
item 3. §3 scope both halves → Tasks 1, 4, 5. §4 placement on `GpuContext` → Task 1 Step 2. §5 the
pipeline half, shader-text key, layout under the shader key → Task 1 Steps 1 and 3, Task 5. §6 the
staging half, grow-never-shrink, `bytes` not buffer size → Task 1 Step 1, Task 4. §7 what is not
cached → Task 1 Step 3's comment and the uniform test in Task 3. §8 all five tests → Tasks 1 and 3,
plus the goldens in Task 4 Step 3; the four sabotages → Task 6 Steps 1–4. §9 measure both benches →
Task 6 Steps 5–6. §10 caveats → Task 6 Step 7 item 10.

**Gap found and closed:** the spec did not anticipate `GpuExposureStage` having its own encode
path. Task 2 added.

**Type consistency:** `GpuCache::program(device, label, shader, layout_entries, entry_points) ->
(BindGroupLayout, Vec<ComputePipeline>)` is used with that exact signature in Task 1 Step 3 and
Task 5 Step 1. `GpuCache::staging(device, bytes) -> Option<Buffer>` matches Task 4 Step 1.
`misses_for(&str)` and `allocations()` match their test call sites. `exposure::SHADER` is made
`pub(crate)` in Task 2 Step 2 before Task 3 Step 1 reads it.
