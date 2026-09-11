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
    /// ⚠⚠ **`#[cfg(test)]` because it is a test observable and nothing else.** Left
    /// unconditional it is a field the library never reads, which `-D warnings` rejects as dead
    /// code — and the honest fix is to say it exists for the tests rather than to `allow` the
    /// lint. `misses_for` is gated the same way for the same reason.
    #[cfg(test)]
    misses: usize,
}

/// The wgpu objects that outlive a render.
#[derive(Debug, Default)]
pub(crate) struct GpuCache {
    programs: Mutex<HashMap<String, Program>>,
    staging: Mutex<Vec<wgpu::Buffer>>,
    allocations: AtomicUsize,
}

/// How many staging buffers the pool will hold.
///
/// ⚠ A bound on *idle* buffers, not on concurrency: a render whose take finds the pool empty
/// allocates its own and simply drops it if the pool is full when it finishes. Four covers the
/// threads that can render at once here — the render worker, the full-resolution worker and
/// Compare's two panes, each of which owns a `RenderEngine`.
const MAX_STAGING: usize = 4;

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
        // ⚠⚠ **Carried across a re-insert rather than set to 1, and a sabotage is what found
        // that.** With `misses: 1` written afresh each time, a cache that compiled on *every*
        // call still reported 1 — so `a_second_run_of_the_same_chain_compiles_nothing` passed
        // against a cache that did nothing at all. Counting compilations rather than describing
        // the current entry is what makes the observable able to see its own absence.
        #[cfg(test)]
        let misses = map.get(shader).map_or(0, |p: &Program| p.misses) + 1;
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
                    #[cfg(test)]
                    misses,
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

    /// Take a `MAP_WRITE` staging buffer of at least `bytes`, **mapped and ready to be written**.
    ///
    /// The caller writes into `buffer.slice(..bytes).get_mapped_range_mut()`, `unmap()`s, and
    /// hands the buffer back with [`GpuCache::put_staging`] once the GPU has finished reading it.
    ///
    /// ⚠⚠ **A pool rather than the single buffer the spec called for, and the reason is
    /// correctness rather than throughput.** The cache is process-wide while rendering is not:
    /// Compare gives each pane its own `RenderEngine` on its own thread, and the full-resolution
    /// worker is a third. Two of them sharing one buffer is not a slow render, it is
    /// `Buffer is already mapped` and, where the timing allows it, one render's pixels appearing
    /// in another's. The parallel test suite reported it as 28 failures the first time this
    /// shipped as one buffer.
    ///
    /// ⚠ Still "grow, never shrink" (user's call, 2026-09-11): a buffer too small for the render
    /// asking is **dropped** rather than kept, because the renders that needed the large one will
    /// keep needing it. A larger buffer serving a smaller render is correct by construction,
    /// since both the mapped slice and the copy out of it are given `bytes`.
    ///
    /// ⚠⚠ Reuse is safe **only because `GpuChain::run` is synchronous** — it blocks on
    /// `poll(Wait)` until readback completes before returning the buffer, so nothing hands back a
    /// buffer the GPU is still reading. Measured on a 78.2 MiB buffer: the map costs
    /// **0.00-0.01 ms** against an **8-10 ms** allocation.
    pub(crate) fn take_staging(&self, device: &wgpu::Device, bytes: u64) -> Option<wgpu::Buffer> {
        let mut pool = Self::lock(&self.staging);
        if let Some(i) = pool.iter().position(|b| b.size() >= bytes) {
            let b = pool.swap_remove(i);
            drop(pool);
            let (tx, rx) = std::sync::mpsc::channel();
            b.slice(..bytes).map_async(wgpu::MapMode::Write, move |r| {
                let _ = tx.send(r);
            });
            let _ = device.poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            });
            rx.recv().ok()?.ok()?;
            return Some(b);
        }
        // Everything held is too small for this render, and will stay too small.
        pool.clear();
        drop(pool);
        let b = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("chain-staging"),
            size: bytes,
            usage: wgpu::BufferUsages::MAP_WRITE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: true,
        });
        self.allocations.fetch_add(1, Ordering::Relaxed);
        Some(b)
    }

    /// Hand a staging buffer back once the GPU has finished reading it.
    ///
    /// ⚠ Dropped rather than pooled past [`MAX_STAGING`], so a burst of concurrent renders cannot
    /// leave the pool holding more than four 82 MB buffers for ever.
    pub(crate) fn put_staging(&self, buffer: wgpu::Buffer) {
        let mut pool = Self::lock(&self.staging);
        if pool.len() < MAX_STAGING {
            pool.push(buffer);
        }
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
        Self::lock(&self.programs)
            .get(shader)
            .map_or(0, |p| p.misses)
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
        let cycle = |bytes: u64| {
            let b = cache.take_staging(d, bytes).expect("a staging buffer");
            b.unmap();
            cache.put_staging(b);
        };

        cycle(4096);
        assert_eq!(cache.allocations(), 1);

        cycle(4096);
        assert_eq!(
            cache.allocations(),
            1,
            "a render of the same size allocated a second buffer"
        );

        cycle(8192);
        assert_eq!(cache.allocations(), 2, "a larger render must allocate");

        cycle(4096);
        assert_eq!(
            cache.allocations(),
            2,
            "a smaller render must reuse the larger buffer rather than shrink"
        );
    }

    #[test]
    fn two_renders_in_flight_never_share_a_buffer() {
        // ⚠⚠ **The regression guard on a defect the spec had and the parallel test suite found.**
        // The cache is process-wide while rendering is not — Compare gives each pane its own
        // `RenderEngine` on its own thread. With a single shared buffer the second take returns
        // the first render's buffer, which is `Buffer is already mapped` at best and one render's
        // pixels inside another's at worst. Both are held mapped here at once, exactly as two
        // concurrent renders would.
        let Some(ctx) = crate::shared() else {
            eprintln!("SKIPPED two_renders_in_flight_never_share_a_buffer: no adapter");
            return;
        };
        let cache = GpuCache::default();
        let d = &ctx.device;

        // ⚠⚠ **The pool must be SEEDED first, and leaving that out is what made the first
        // version of this test pass against the very bug it names.** With nothing ever put back,
        // both takes find an empty pool and allocate, so the take-from-pool branch — the one a
        // buffer-sharing bug lives in — is never reached. A fixture has to contain the thing a
        // wrong implementation would be attracted to.
        let seed = cache
            .take_staging(d, 4096)
            .expect("a buffer to seed the pool with");
        seed.unmap();
        cache.put_staging(seed);
        assert_eq!(
            cache.allocations(),
            1,
            "fixture check: the pool holds one buffer"
        );

        let a = cache
            .take_staging(d, 4096)
            .expect("the first render's buffer");
        let b = cache
            .take_staging(d, 4096)
            .expect("the second render's buffer");
        assert_eq!(
            cache.allocations(),
            2,
            "the second render in flight was handed the first render's buffer"
        );
        a.unmap();
        b.unmap();
    }
}
