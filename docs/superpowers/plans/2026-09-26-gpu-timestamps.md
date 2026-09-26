# GPU Timestamps Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Under `RUSTROOM_DEBUG_GPU_TIME=1`, every `GpuChain` run prints the GPU time of each stage, each copy and the quantise, plus the GPU span against the wall submit+wait.

**Architecture:** `GpuContext` requests wgpu's timestamp features when the adapter has them. A new `timing` module owns the query set, the resolve/read buffers and the pure tick→milliseconds assembly. `GpuChain::run_with`, in timing mode only, opens one compute pass per stage with timestamp writes, marks the copies on the encoder, resolves, waits, and prints; the untimed path is unchanged.

**Tech Stack:** Rust, wgpu 29 (`QuerySet`, `ComputePassTimestampWrites`, `CommandEncoder::write_timestamp`, `resolve_query_set`, `Queue::get_timestamp_period`). Spec: `docs/superpowers/specs/2026-09-26-gpu-timestamps-design.md`.

**Conventions that apply:** no `Co-Authored-By` trailer; never push; iterate on this crate with `cargo test --release -p gpu-pipeline <filter>` (adapter acquisition is ~45 s in a debug build, 21 ms in release) and run the full debug gate once at the end; every GPU test skips with `SKIPPED:` when `shared()` is `None`, and these additionally skip when the device has no timestamp queries.

---

### Task 1: The device asks for timestamp queries

**Files:**
- Modify: `crates/gpu-pipeline/src/context.rs` (the `GpuContext` struct, `new`, accessors)

- [ ] **Step 1: Write the failing test**

Append to the `#[cfg(test)] mod tests` at the bottom of `context.rs` (create the module if there is none; check with `grep -n 'cfg(test)' crates/gpu-pipeline/src/context.rs`):

```rust
    /// The device reports whether it can time itself, and a device that can has a tick period.
    #[test]
    fn timestamp_support_is_reported_with_its_period() {
        let Some(ctx) = crate::shared() else {
            eprintln!("SKIPPED: no GPU adapter on this machine");
            return;
        };
        if ctx.timestamps() {
            assert!(ctx.timestamp_period() > 0.0, "a timestamp device has a positive period");
        }
    }
```

- [ ] **Step 2: Run it to see it fail**

Run: `cargo test --release -p gpu-pipeline timestamp_support_is_reported`
Expected: compile error, `no method named timestamps`.

- [ ] **Step 3: Request the features and record them**

In `GpuContext`, add two fields after `max_image_bytes`:

```rust
    /// Whether this device granted `TIMESTAMP_QUERY` and `TIMESTAMP_QUERY_INSIDE_ENCODERS`.
    ///
    /// ⚠⚠ Both, not either: a pass-boundary timestamp needs the first and an encoder mark around a
    /// copy needs the second. Apple silicon grants both; it does NOT grant
    /// `TIMESTAMP_QUERY_INSIDE_PASSES` (wgpu gates that on `AtDispatchBoundary` counter sampling),
    /// which is why `timing.rs` times one PASS per stage rather than one dispatch.
    timestamps: bool,
    /// Nanoseconds per timestamp tick, from the queue.
    timestamp_period: f32,
```

In `new`, replace the `request_device` call and the struct literal:

```rust
        // ⚠ Requested only when the adapter offers them, so this cannot make `request_device`
        // fail where it succeeded before. A device without them runs every chain untimed and
        // `timing::note_unsupported` says so once.
        let wanted = wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
        let features = adapter.features() & wanted;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_features: features,
            required_limits: limits,
            ..Default::default()
        }))
        .ok()?;
        let timestamps = features.contains(wanted);
        let timestamp_period = queue.get_timestamp_period();
        Some(Self {
            device,
            queue,
            cache: Default::default(),
            shared_plane: Default::default(),
            prefix_plane: Default::default(),
            adapter_name: info.name,
            backend: format!("{:?}", info.backend),
            max_image_bytes,
            timestamps,
            timestamp_period,
        })
```

Add the accessors after `max_image_bytes()`:

```rust
    /// Whether chains can time themselves on this device. See the field.
    pub fn timestamps(&self) -> bool {
        self.timestamps
    }

    /// Nanoseconds per timestamp tick.
    pub fn timestamp_period(&self) -> f32 {
        self.timestamp_period
    }
```

- [ ] **Step 4: Run it to see it pass**

Run: `cargo test --release -p gpu-pipeline timestamp_support_is_reported`
Expected: `1 passed` (or `SKIPPED:` on a machine without an adapter).

- [ ] **Step 5: Commit**

```bash
git add crates/gpu-pipeline/src/context.rs
git commit -m "feat(gpu): the device requests timestamp queries when the adapter has them"
```

---

### Task 2: `timing::assemble` and the once-only note, pure and unit-tested

**Files:**
- Create: `crates/gpu-pipeline/src/timing.rs`
- Modify: `crates/gpu-pipeline/src/lib.rs` (add `mod timing;` in alphabetical order after `mod tone_curve;`… no: after `mod srgb_wgsl;` and before `mod tone_curve;`; add `pub use timing::Timings;` beside the other re-exports)

- [ ] **Step 1: Write the failing tests**

Create `crates/gpu-pipeline/src/timing.rs` with only the tests for now:

```rust
//! GPU timestamps: per-stage GPU time for one chain, under `RUSTROOM_DEBUG_GPU_TIME=1`.
//!
//! ⚠⚠ **Apple silicon writes a timestamp only at a compute PASS boundary.** wgpu's Metal backend
//! gates `TIMESTAMP_QUERY_INSIDE_PASSES` on `AtDispatchBoundary` counter sampling, which Apple GPUs
//! do not offer, so a per-stage figure needs one pass per stage. `GpuChain::run_with` opens them
//! only in timing mode; the untimed path is one pass, byte for byte what shipped. The split changes
//! no pixel (M8.3b's sabotage established that), and its cost is visible as the difference between
//! the timed wall printed here and the untimed `RUSTROOM_DEBUG_XFER` wall, so no figure from a
//! timed run is ever quoted as a frame time.
//!
//! ⚠ The assembly is pure and lives apart from the wgpu objects so the arithmetic — order, span,
//! latency, the untimed entry — is tested with no GPU at all.

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    /// Entries come back in mark order, an unmarked entry stays `None`, the span runs from the
    /// earliest begin to the latest end, and the latency is what the wall holds beyond the span.
    #[test]
    fn assemble_orders_spans_and_subtracts() {
        let labels = vec![
            ("upload".to_owned(), None),
            ("exposure".to_owned(), Some(Pair { begin: 0, end: 1 })),
            ("readback".to_owned(), Some(Pair { begin: 2, end: 3 })),
        ];
        // 1 ns per tick: exposure 1_000_000 ticks = 1 ms, readback 2 ms, a 500 µs gap between.
        let ticks = [10_000_000u64, 11_000_000, 11_500_000, 13_500_000];
        let t = assemble(&labels, &ticks, 1.0, 10.0, false);
        assert_eq!(
            t.entries,
            vec![
                ("upload".to_owned(), None),
                ("exposure".to_owned(), Some(1.0)),
                ("readback".to_owned(), Some(2.0)),
            ]
        );
        assert!((t.gpu_span_ms - 3.5).abs() < 1e-9, "span {}", t.gpu_span_ms);
        assert!((t.latency_ms - 6.5).abs() < 1e-9, "latency {}", t.latency_ms);
        assert_eq!(t.wall_ms, 10.0);
        assert!(!t.waited_for_timing);
    }

    /// The period scales ticks: at 0.5 ns per tick the same ticks read half as long.
    #[test]
    fn assemble_applies_the_period() {
        let labels = vec![("s".to_owned(), Some(Pair { begin: 0, end: 1 }))];
        let t = assemble(&labels, &[0, 2_000_000], 0.5, 5.0, true);
        assert_eq!(t.entries[0].1, Some(1.0));
        assert!(t.waited_for_timing);
    }

    /// A pair whose ticks are missing (a resolve that came back short) reads `None` rather than
    /// panicking on the render worker.
    #[test]
    fn assemble_tolerates_missing_ticks() {
        let labels = vec![("s".to_owned(), Some(Pair { begin: 0, end: 1 }))];
        let t = assemble(&labels, &[7], 1.0, 1.0, false);
        assert_eq!(t.entries[0].1, None);
        assert_eq!(t.gpu_span_ms, 0.0);
    }

    /// The unsupported note prints once per flag, and the second call says it did not.
    #[test]
    fn the_unsupported_note_prints_once() {
        let flag = AtomicBool::new(false);
        assert!(note_unsupported_with(&flag), "the first call prints");
        assert!(!note_unsupported_with(&flag), "the second does not");
        assert!(!note_unsupported_with(&flag));
    }
}
```

Add to `lib.rs`, in the module list between `mod srgb_wgsl;` and `mod tone_curve;`:

```rust
mod timing;
```

and among the `pub use` lines, after `pub use sharpen::GpuSharpenStage;`:

```rust
pub use timing::Timings;
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test --release -p gpu-pipeline timing::tests`
Expected: compile errors, `cannot find type Pair`, `cannot find function assemble`.

- [ ] **Step 3: Write the pure half**

Insert above the `#[cfg(test)]` module in `timing.rs`:

```rust
use std::sync::atomic::{AtomicBool, Ordering};

/// One label's begin and end query indices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Pair {
    pub begin: u32,
    pub end: u32,
}

/// What one timed chain run measured.
#[derive(Clone, Debug, PartialEq)]
pub struct Timings {
    /// In mark order: the upload, each stage by name, `quantise` in bytes mode, `readback`.
    /// `None` where nothing was timed — an upload that never happened because the plane was taken.
    pub entries: Vec<(String, Option<f64>)>,
    /// Earliest begin to latest end on the GPU's clock, in milliseconds.
    pub gpu_span_ms: f64,
    /// Submit to completion on the CPU's clock, in milliseconds.
    pub wall_ms: f64,
    /// `wall_ms - gpu_span_ms`: what the work spent queued or scheduled rather than running.
    pub latency_ms: f64,
    /// Whether this chain waited only because timing asked it to — a handoff chain, which
    /// otherwise submits and returns.
    pub waited_for_timing: bool,
}

/// Build the readings from resolved ticks.
///
/// `labels` holds each label's query pair, or `None` for an entry that was never marked, in mark
/// order; `ticks` is the resolved query set; `period` is nanoseconds per tick.
///
/// ⚠ Pure, so a GPU is not needed to test the order, the span or the subtraction. A pair whose
/// ticks are missing reads `None` rather than panicking: this runs on the render worker.
pub(crate) fn assemble(
    labels: &[(String, Option<Pair>)],
    ticks: &[u64],
    period: f32,
    wall_ms: f64,
    waited_for_timing: bool,
) -> Timings {
    let ms = |t: u64| t as f64 * f64::from(period) / 1e6;
    let mut lo = u64::MAX;
    let mut hi = 0u64;
    let mut entries = Vec::with_capacity(labels.len());
    for (name, pair) in labels {
        let d = pair.and_then(|p| {
            let b = *ticks.get(p.begin as usize)?;
            let e = *ticks.get(p.end as usize)?;
            lo = lo.min(b);
            hi = hi.max(e);
            Some(ms(e.saturating_sub(b)))
        });
        entries.push((name.clone(), d));
    }
    let gpu_span_ms = if hi > lo { ms(hi - lo) } else { 0.0 };
    Timings {
        entries,
        gpu_span_ms,
        wall_ms,
        latency_ms: wall_ms - gpu_span_ms,
        waited_for_timing,
    }
}

impl Timings {
    /// One block per chain on stderr, beside the `RUSTROOM_DEBUG_XFER` line.
    pub fn print(&self, chain: &str, w: usize, h: usize) {
        let note = if self.waited_for_timing {
            "  (waited for timing)"
        } else {
            ""
        };
        eprintln!("GPUTIME {chain} {w}x{h}{note}");
        for (name, d) in &self.entries {
            match d {
                Some(ms) => eprintln!("  {name:<18} {ms:7.2} ms"),
                None => eprintln!("  {name:<18} (taken)"),
            }
        }
        eprintln!(
            "  gpu span {:7.2} ms   wall submit+wait {:7.2} ms   latency {:7.2} ms",
            self.gpu_span_ms, self.wall_ms, self.latency_ms
        );
    }
}

static UNSUPPORTED_NOTED: AtomicBool = AtomicBool::new(false);

/// Say once per process that timing was asked for on a device without timestamp queries.
pub(crate) fn note_unsupported() -> bool {
    note_unsupported_with(&UNSUPPORTED_NOTED)
}

/// The once-only rule on a flag the tests can own. Returns whether it printed.
pub(crate) fn note_unsupported_with(noted: &AtomicBool) -> bool {
    if noted.swap(true, Ordering::Relaxed) {
        return false;
    }
    eprintln!(
        "rustroom: RUSTROOM_DEBUG_GPU_TIME is set but this GPU offers no timestamp queries; chains run untimed"
    );
    true
}
```

- [ ] **Step 4: Run them to see them pass**

Run: `cargo test --release -p gpu-pipeline timing::tests`
Expected: `4 passed`. (`note_unsupported`, `Timings::print` and `Pair` are unused until Task 3; if clippy's dead-code lint would object at the gate, Task 3 wires them before the gate runs.)

- [ ] **Step 5: Commit**

```bash
git add crates/gpu-pipeline/src/timing.rs crates/gpu-pipeline/src/lib.rs
git commit -m "feat(gpu): timing::assemble — ticks to per-entry milliseconds, span and latency"
```

---

### Task 3: The `Recorder`: query set, buffers, marks, resolve, finish

**Files:**
- Modify: `crates/gpu-pipeline/src/timing.rs`

No unit test can construct these without a GPU; Task 4's chain tests are their tests.

- [ ] **Step 1: Write the recorder**

Insert into `timing.rs` after the `Timings` impl and before `static UNSUPPORTED_NOTED`:

```rust
/// The query set and buffers for one timed chain run.
///
/// ⚠ Built per run rather than cached: a run's pair count is its stage count plus three, and the
/// objects are a few hundred bytes. Caching would add an invalidation rule for a debug path.
pub(crate) struct Recorder {
    set: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    read: wgpu::Buffer,
    labels: Vec<(String, Option<Pair>)>,
    next: u32,
    capacity: u32,
}

impl Recorder {
    /// Room for `pairs` begin/end pairs.
    pub(crate) fn new(ctx: &crate::GpuContext, pairs: u32) -> Self {
        let capacity = pairs.max(1) * 2;
        let set = ctx.device.create_query_set(&wgpu::QuerySetDescriptor {
            label: Some("gpu-time"),
            ty: wgpu::QueryType::Timestamp,
            count: capacity,
        });
        // ⚠ A resolve destination's size and offset must be multiples of 256.
        let align = wgpu::QUERY_RESOLVE_BUFFER_ALIGNMENT;
        let bytes = (u64::from(capacity) * u64::from(wgpu::QUERY_SIZE)).div_ceil(align) * align;
        let resolve = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gpu-time-resolve"),
            size: bytes,
            usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let read = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gpu-time-read"),
            size: bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        Self {
            set,
            resolve,
            read,
            labels: Vec::new(),
            next: 0,
            capacity,
        }
    }

    /// Reserve a begin/end pair under `label`.
    ///
    /// # Panics
    /// Past capacity: the caller sized the recorder from its own stage count, so this is a bug
    /// in the caller and not a runtime condition.
    pub(crate) fn pair(&mut self, label: &str) -> Pair {
        assert!(
            self.next + 2 <= self.capacity,
            "timing recorder over capacity at {label}"
        );
        let p = Pair {
            begin: self.next,
            end: self.next + 1,
        };
        self.next += 2;
        self.labels.push((label.to_owned(), Some(p)));
        p
    }

    /// Record a label with no marks — an upload that never happened.
    pub(crate) fn skipped(&mut self, label: &str) {
        self.labels.push((label.to_owned(), None));
    }

    /// A compute pass's timestamp writes for `p`.
    pub(crate) fn pass_writes(&self, p: Pair) -> wgpu::ComputePassTimestampWrites<'_> {
        wgpu::ComputePassTimestampWrites {
            query_set: &self.set,
            beginning_of_pass_write_index: Some(p.begin),
            end_of_pass_write_index: Some(p.end),
        }
    }

    /// An encoder mark at `index` — a pair's begin or end — for the copies outside any pass.
    pub(crate) fn mark(&self, enc: &mut wgpu::CommandEncoder, index: u32) {
        enc.write_timestamp(&self.set, index);
    }

    /// Encode the resolve and the copy into the readable buffer. Called after the last mark and
    /// before the encoder is finished.
    pub(crate) fn resolve(&self, enc: &mut wgpu::CommandEncoder) {
        if self.next == 0 {
            return;
        }
        enc.resolve_query_set(&self.set, 0..self.next, &self.resolve, 0);
        enc.copy_buffer_to_buffer(
            &self.resolve,
            0,
            &self.read,
            0,
            u64::from(self.next) * u64::from(wgpu::QUERY_SIZE),
        );
    }

    /// Wait for the submission, read the ticks and assemble.
    ///
    /// `submitted_at` is when the caller submitted; the wall is measured from it to the end of
    /// this wait. On the ordinary path the caller has already waited, so the wait here returns at
    /// once and the wall equals its own stopwatch; on a handoff chain this IS the wait.
    pub(crate) fn finish(
        self,
        ctx: &crate::GpuContext,
        submitted_at: std::time::Instant,
        waited_for_timing: bool,
    ) -> Option<Timings> {
        let bytes = u64::from(self.next) * u64::from(wgpu::QUERY_SIZE);
        if bytes == 0 {
            return None;
        }
        let slice = self.read.slice(..bytes);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = ctx.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        rx.recv().ok()?.ok()?;
        let wall_ms = submitted_at.elapsed().as_secs_f64() * 1e3;
        // ⚠ Read as 8-byte chunks rather than a `cast_slice`, which would panic on an unaligned
        // mapping; the render worker must not die for a debug readout.
        let ticks: Vec<u64> = slice
            .get_mapped_range()
            .chunks_exact(8)
            .map(|c| u64::from_le_bytes(c.try_into().expect("8 bytes")))
            .collect();
        self.read.unmap();
        Some(assemble(
            &self.labels,
            &ticks,
            ctx.timestamp_period(),
            wall_ms,
            waited_for_timing,
        ))
    }
}
```

- [ ] **Step 2: Build**

Run: `cargo build --release -p gpu-pipeline`
Expected: builds. If `wgpu::QUERY_RESOLVE_BUFFER_ALIGNMENT` or `wgpu::QUERY_SIZE` is not re-exported, replace them with the literals `256` and `8` and a comment naming `wgpu_types::QUERY_RESOLVE_BUFFER_ALIGNMENT` / `QUERY_SIZE` as the source.

- [ ] **Step 3: Commit**

```bash
git add crates/gpu-pipeline/src/timing.rs
git commit -m "feat(gpu): timing::Recorder — a query set, its buffers, marks, resolve and finish"
```

---

### Task 4: The chain times itself in timing mode

**Files:**
- Modify: `crates/gpu-pipeline/src/chain.rs` (`GpuChain` fields and `new`; two new methods; `run_with`)
- Test: `crates/gpu-pipeline/src/chain.rs` tests module

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `chain.rs`. `GpuContrastStage` is constructed as `accelerate.rs` does: `GpuContrastStage::new(amount, &ctx)`.

```rust
    /// A chain whose stages move pixels, timing OFF whatever the environment says — so a test
    /// run under `RUSTROOM_DEBUG_GPU_TIME` still tests the untimed path where it means to.
    fn three_stages(ctx: &Arc<GpuContext>) -> GpuChain {
        let mut c = GpuChain::new(
            ctx.clone(),
            vec![
                Box::new(GpuExposureStage::new(1.0, ctx.clone())),
                Box::new(crate::GpuContrastStage::new(25.0, ctx)),
                Box::new(GpuExposureStage::new(-0.5, ctx.clone())),
            ],
        );
        c.set_timing(false);
        c
    }

    fn timing_ctx() -> Option<Arc<GpuContext>> {
        let ctx = ctx()?;
        if !ctx.timestamps() {
            eprintln!("SKIPPED: no timestamp queries on this GPU");
            return None;
        }
        Some(ctx)
    }

    /// One pass per stage changes no byte: the timed run IS the untimed run, measured.
    #[test]
    fn a_timed_run_produces_the_untimed_bytes() {
        let Some(ctx) = timing_ctx() else { return };
        let img = ramp(64, 48);
        let want = three_stages(&ctx).run_rgba8(&img).expect("gpu");
        assert_ne!(want, img.to_rgba8(), "the fixture must move pixels");
        let mut timed = three_stages(&ctx);
        timed.set_timing(true);
        let got = timed.run_rgba8(&img).expect("gpu");
        assert_eq!(got, want, "one pass per stage must change no byte");
    }

    /// ⚠⚠ The observable. A sabotage that keeps the single pass reports one stage entry, not
    /// three, and fails the name list.
    #[test]
    fn a_timed_run_reports_one_entry_per_stage_in_order() {
        let Some(ctx) = timing_ctx() else { return };
        let mut chain = three_stages(&ctx);
        chain.set_timing(true);
        chain.run_rgba8(&ramp(64, 48)).expect("gpu");
        let t = chain.last_timings().expect("a timed run keeps its readings");
        let names: Vec<&str> = t.entries.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(
            names,
            ["upload", "exposure", "contrast", "exposure", "quantise", "readback"]
        );
        for (name, d) in &t.entries {
            let d = d.unwrap_or_else(|| panic!("{name} was marked and must have a duration"));
            assert!(
                d > 0.0 && d <= t.wall_ms,
                "{name}: {d:.3} ms against a wall of {:.3}",
                t.wall_ms
            );
        }
        assert!(t.gpu_span_ms > 0.0 && t.gpu_span_ms <= t.wall_ms, "span {}", t.gpu_span_ms);
        assert!(!t.waited_for_timing, "an ordinary chain waited on its own account");
    }

    /// The float path has no quantise; the readback is still marked.
    #[test]
    fn a_timed_float_run_has_no_quantise_entry() {
        let Some(ctx) = timing_ctx() else { return };
        let mut chain = three_stages(&ctx);
        chain.set_timing(true);
        let _ = chain.apply(ramp(32, 32));
        let t = chain.last_timings().expect("timed");
        let names: Vec<&str> = t.entries.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["upload", "exposure", "contrast", "exposure", "readback"]);
    }

    /// A handoff chain normally submits and returns; timed, it waits and says so.
    #[test]
    fn a_timed_handoff_chain_waits_for_its_readings() {
        let Some(ctx) = timing_ctx() else { return };
        let mut chain = three_stages(&ctx);
        chain.offer_output(Slot::Prefix);
        chain.set_timing(true);
        let out = chain.run_to_plane(&ramp(32, 32));
        // ⚠ Clear whatever was deposited, whether or not the run succeeded: a plane left in the
        // slot would be found by the next test on this thread.
        ctx.prefix_plane().clear();
        out.expect("the handoff path must be available here");
        let t = chain.last_timings().expect("timed");
        assert!(t.waited_for_timing);
        let names: Vec<&str> = t.entries.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["upload", "exposure", "contrast", "exposure"]);
    }

    /// Timing off keeps nothing and costs nothing: no query set, no readings.
    #[test]
    fn an_untimed_run_keeps_no_timings() {
        let Some(ctx) = ctx() else { return };
        let chain = three_stages(&ctx);
        chain.run_rgba8(&ramp(16, 16)).expect("gpu");
        assert!(chain.last_timings().is_none());
    }
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test --release -p gpu-pipeline timed_`
Expected: compile errors, `no method named set_timing`, `no method named last_timings`.

- [ ] **Step 3: Fields, constructor and the two methods**

In `GpuChain`, add after `offer_output: Option<Slot>,`:

```rust
    /// Whether this chain times itself. See `timing.rs`.
    ///
    /// ⚠ Read from `RUSTROOM_DEBUG_GPU_TIME` at construction and settable per chain, so a test can
    /// turn it on for one chain without touching the process environment — which is shared by
    /// every test running beside it.
    timing: bool,
    /// What the last timed run measured, for the tests and for a probe that wants a table.
    last_timings: std::sync::Mutex<Option<crate::timing::Timings>>,
```

In `new`, add to the struct literal:

```rust
            timing: std::env::var_os("RUSTROOM_DEBUG_GPU_TIME").is_some(),
            last_timings: std::sync::Mutex::new(None),
```

Add after `transfer_counts`:

```rust
    /// Turn per-stage GPU timing on or off for this chain. See `timing.rs`.
    pub fn set_timing(&mut self, on: bool) {
        self.timing = on;
    }

    /// What the last timed run measured, or `None` if no run was timed.
    pub fn last_timings(&self) -> Option<crate::timing::Timings> {
        self.last_timings.lock().ok()?.clone()
    }

    /// The chain's stages, joined — the label every debug line uses.
    fn label(&self) -> String {
        self.stages
            .iter()
            .map(|s| s.name())
            .collect::<Vec<_>>()
            .join("+")
    }
```

- [ ] **Step 4: Wire the recorder into `run_with`**

In `run_with`, immediately after `let ctx = &self.ctx; let device = &ctx.device;`, add:

```rust
        // ⚠ One recorder per timed run, sized from this chain: the upload, one pair per stage,
        // the quantise and the readback. `None` keeps the untimed path byte for byte.
        let mut rec = if !self.timing {
            None
        } else if ctx.timestamps() {
            Some(crate::timing::Recorder::new(
                ctx,
                self.stages.len() as u32 + 3,
            ))
        } else {
            crate::timing::note_unsupported();
            None
        };
```

Replace the upload block — from `let t_memcpy = std::time::Instant::now();` through the end of the `let staging = match ... ;` statement — with:

```rust
        let t_memcpy = std::time::Instant::now();
        let mut enc = device.create_command_encoder(&Default::default());
        // ⚠ The upload mark brackets whatever moves the input in: the staging copy, the resident
        // copy, or nothing at all for a taken plane, which the block then prints as `(taken)`.
        let up = match (shared_taken, input) {
            (true, _) | (false, Input::Plane(..)) => {
                if let Some(r) = rec.as_mut() {
                    r.skipped("upload");
                }
                None
            }
            _ => rec.as_mut().map(|r| r.pair("upload")),
        };
        if let (Some(r), Some(p)) = (rec.as_ref(), up) {
            r.mark(&mut enc, p.begin);
        }
        let staging = match (shared_taken, input) {
            (true, _) | (false, Input::Plane(..)) => None,
            // ⚠⚠ A GPU-resident input is COPIED, never used in place: stages write into `img`, and
            // the resident image must survive for the next frame of the drag. A GPU-side copy
            // costs a fraction of the staging upload it replaces, and counts as no upload.
            (false, Input::Resident(r)) => {
                enc.copy_buffer_to_buffer(&r.buffer, 0, &img.buffer, 0, bytes);
                None
            }
            (false, Input::Cpu(image)) => {
                let staging = ctx.cache.take_staging(device, bytes)?;
                staging
                    .slice(..bytes)
                    .get_mapped_range_mut()
                    .copy_from_slice(bytemuck::cast_slice(image.data()));
                staging.unmap();
                enc.copy_buffer_to_buffer(&staging, 0, &img.buffer, 0, bytes);
                self.uploads.fetch_add(1, Ordering::Relaxed);
                Some(staging)
            }
        };
        if let (Some(r), Some(p)) = (rec.as_ref(), up) {
            r.mark(&mut enc, p.end);
        }
```

(The two `⚠` comments inside the `staging` match are today's; keep them verbatim.)

Replace the pass block — the `{ let mut pass = enc.begin_compute_pass(...); for s in &self.stages { ... } if let Some(p) = &packed { crate::quantize::encode(...) } }` block — with:

```rust
        match rec.as_mut() {
            None => {
                let mut pass = enc.begin_compute_pass(&Default::default());
                for s in &self.stages {
                    s.encode(ctx, &mut pass, &img, &scratch);
                    // ⚠ Relabelled BETWEEN stages, so a stage after a resize dispatches over the
                    // new size. wgpu 29's pass holds no borrow of `img`, which is what makes this
                    // legal.
                    let (w, h) = s.output_dims(img.width(), img.height());
                    img.relabel(w, h);
                }
                // ⚠ Inside the **same** pass, so it sees every stage's writes under the in-pass
                // barrier this chain already depends on and measures.
                if let Some(p) = &packed {
                    crate::quantize::encode(ctx, &mut pass, &img, p);
                }
            }
            Some(r) => {
                // ⚠⚠ ONE PASS PER STAGE, timing mode only. Apple silicon writes a timestamp at a
                // pass boundary and nowhere inside one, so this is the only way to a per-stage
                // figure. M8.3b's sabotage established that the split changes no pixel; its cost
                // is what `Timings::print` shows against the untimed wall.
                for s in &self.stages {
                    let p = r.pair(s.name());
                    let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some(s.name()),
                        timestamp_writes: Some(r.pass_writes(p)),
                    });
                    s.encode(ctx, &mut pass, &img, &scratch);
                    drop(pass);
                    let (w, h) = s.output_dims(img.width(), img.height());
                    img.relabel(w, h);
                }
                if let Some(pk) = &packed {
                    let p = r.pair("quantise");
                    let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some("quantise"),
                        timestamp_writes: Some(r.pass_writes(p)),
                    });
                    crate::quantize::encode(ctx, &mut pass, &img, pk);
                }
            }
        }
```

In the `Download::None` branch, replace from `let slot = self.offer_output?;` through `return Some(Vec::new());` with:

```rust
            let slot = self.offer_output?;
            if let Some(r) = rec.as_ref() {
                r.resolve(&mut enc);
            }
            let t_submit = std::time::Instant::now();
            ctx.queue.submit(Some(enc.finish()));
            ctx.plane(slot).deposit(ow, oh, img);
            // ⚠⚠ **Dropped, NOT returned to the staging cache.** Nothing waited for the GPU, so the
            // copy out of this buffer may still be in flight; handing it back would let the next
            // upload overwrite it mid-copy — a plausible wrong frame. wgpu keeps a dropped buffer
            // alive until its submission completes, which is exactly the lifetime needed.
            drop(staging);
            // ⚠ Timed, a handoff chain waits — the one thing it otherwise never does. The block
            // says so, and the untimed `RUSTROOM_DEBUG_XFER` line is the control.
            if let Some(r) = rec.take() {
                if let Some(t) = r.finish(ctx, t_submit, true) {
                    t.print(&self.label(), iw, ih);
                    if let Ok(mut g) = self.last_timings.lock() {
                        *g = Some(t);
                    }
                }
            }
            if std::env::var("RUSTROOM_DEBUG_XFER").is_ok() {
                eprintln!(
                    "CHAIN {:>26} {}x{} -> plane {}x{} took={} memcpy={:.2} (no download)",
                    self.label(),
                    iw,
                    ih,
                    ow,
                    oh,
                    shared_taken,
                    d_memcpy.as_secs_f64() * 1e3,
                );
            }
            return Some(Vec::new());
```

In the download section, replace the two lines `enc.copy_buffer_to_buffer(src, 0, &readback, 0, out_bytes);` and `ctx.queue.submit(Some(enc.finish()));` with:

```rust
        let rb = rec.as_mut().map(|r| r.pair("readback"));
        if let (Some(r), Some(p)) = (rec.as_ref(), rb) {
            r.mark(&mut enc, p.begin);
        }
        enc.copy_buffer_to_buffer(src, 0, &readback, 0, out_bytes);
        if let (Some(r), Some(p)) = (rec.as_ref(), rb) {
            r.mark(&mut enc, p.end);
            r.resolve(&mut enc);
        }
        ctx.queue.submit(Some(enc.finish()));
```

After `rx.recv().ok()?.ok()?;` and `let d_submit = t_submit.elapsed();`, add:

```rust
        // ⚠ After the chain's own wait, so `finish`'s wait returns at once and the wall it records
        // is this chain's own submit+wait.
        if let Some(r) = rec.take() {
            if let Some(t) = r.finish(ctx, t_submit, false) {
                t.print(&self.label(), iw, ih);
                if let Ok(mut g) = self.last_timings.lock() {
                    *g = Some(t);
                }
            }
        }
```

Finally, both existing `RUSTROOM_DEBUG_XFER` `eprintln!`s build the label inline with `self.stages.iter().map(|s| s.name()).collect::<Vec<_>>().join("+")`; replace that expression with `self.label()` in both.

- [ ] **Step 5: Run the new tests**

Run: `cargo test --release -p gpu-pipeline timed_`
Expected: `5 passed` (`a_timed_run_produces_the_untimed_bytes`, `a_timed_run_reports_one_entry_per_stage_in_order`, `a_timed_float_run_has_no_quantise_entry`, `a_timed_handoff_chain_waits_for_its_readings`, `an_untimed_run_keeps_no_timings`).

- [ ] **Step 6: Run the whole crate**

Run: `cargo test --release -p gpu-pipeline`
Expected: all pass, 0 failed, the three `#[ignore]`d probes ignored.

- [ ] **Step 7: The sabotage**

In the `Some(r)` arm of the pass match, temporarily open ONE pass for every stage (move `begin_compute_pass` above the `for` and reserve a single pair labelled `"stages"`). Run `cargo test --release -p gpu-pipeline a_timed_run_reports_one_entry_per_stage_in_order`. Expected: FAIL on the name list (`["upload", "stages", "quantise", "readback"]`). Revert the sabotage with `git checkout -- crates/gpu-pipeline/src/chain.rs` is NOT possible (the task's real change is uncommitted); revert by hand and re-run the test to green.

- [ ] **Step 8: Commit**

```bash
git add crates/gpu-pipeline/src/chain.rs
git commit -m "feat(gpu): RUSTROOM_DEBUG_GPU_TIME — one pass per stage, GPU time per entry, span against wall"
```

---

### Task 5: The probe's header, the gate, and a first measurement

**Files:**
- Modify: `crates/app/examples/probe_straighten_drag.rs:1-10` (doc header only)

- [ ] **Step 1: Say where the table comes from**

After the `//! Run:` line in the header, add:

```rust
//! With `RUSTROOM_DEBUG_GPU_TIME=1` as well, every chain prints a `GPUTIME` block: the GPU time of
//! each stage, the copies and the quantise, and the GPU span against the wall submit+wait. ⚠ A
//! timed run opens one pass per stage, so quote its wall only beside the untimed one.
```

- [ ] **Step 2: The gate, each exit code from its own command**

```bash
cargo fmt --all -- --check; echo FMT=$?
CARGO_TARGET_DIR=target/clippy cargo clippy --workspace --all-targets -- -D warnings; echo CLIPPY=$?
cargo test --workspace 2>&1 | tail -30; echo TEST=$?
```

Expected: `FMT=0 CLIPPY=0 TEST=0`, with the per-binary `test result:` lines summed and 78 goldens unmoved. ⚠ Read the exit codes, never the `gpu skips` grep. ⚠ Never run two gates concurrently.

- [ ] **Step 3: One measurement, reported as measured**

Run, on the author's photo and rung (the journal's DSCF6263 at 3200; substitute a RAW that exists on this machine):

```bash
RUSTROOM_DEBUG_RENDER=1 RUSTROOM_DEBUG_XFER=1 cargo run --release -p app --example probe_straighten_drag <raw> 3200 2>&1 | grep -E '^CHAIN|^render:' | tail -6
RUSTROOM_DEBUG_RENDER=1 RUSTROOM_DEBUG_GPU_TIME=1 cargo run --release -p app --example probe_straighten_drag <raw> 3200 2>&1 | grep -A8 '^GPUTIME' | tail -20
```

Record in the commit message and the report: the untimed `CHAIN` wall, the timed block's per-stage numbers, span, wall and latency, and the machine's load average. Do not quote the timed wall as a frame time.

- [ ] **Step 4: Commit**

```bash
git add crates/app/examples/probe_straighten_drag.rs
git commit -m "docs: the drag probe prints per-stage GPU time under RUSTROOM_DEBUG_GPU_TIME"
```
