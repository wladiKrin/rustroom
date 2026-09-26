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
//! ⚠⚠ **The copies are NOT marked, and that was found by a hang rather than decided.** The spec
//! asked for encoder timestamps around the upload and readback copies
//! (`TIMESTAMP_QUERY_INSIDE_ENCODERS`, which the M2 grants). wgpu 29's Metal backend implements a
//! `write_timestamp` outside a pass by deferring it onto a dummy blit encoder — its own source says
//! "here be dragons" — and the command buffer then never reaches `Completed`; its `Device::wait`
//! loop checks for nothing else, so `poll(Wait)` spins forever on the render worker. Measured
//! 2026-09-26: with the two marks in, `a_timed_run_produces_the_untimed_bytes` hangs; without
//! them it passes in 0.04 s. So a block lists the passes only, and the summary's `copies+latency`
//! is the wall's remainder beyond them: the device-side copies and the queue latency together. The
//! host halves of the copies are on the `RUSTROOM_DEBUG_XFER` line already (`memcpy`, `tovec`).
//!
//! ⚠ The assembly is pure and lives apart from the wgpu objects so the arithmetic — order, span,
//! remainder — is tested with no GPU at all.

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
    /// In pass order: each stage by name, then `quantise` in bytes mode. `None` where a pair's
    /// ticks did not come back, which no shipped path produces.
    pub entries: Vec<(String, Option<f64>)>,
    /// The first pass's begin to the last pass's end on the GPU's clock, in milliseconds. ⚠ The
    /// copies before and after are outside it — see the module doc.
    pub gpu_span_ms: f64,
    /// Submit to completion on the CPU's clock, in milliseconds.
    pub wall_ms: f64,
    /// `wall_ms - gpu_span_ms`: the device-side copies and the queue latency together — what the
    /// wall holds beyond the passes.
    pub rest_ms: f64,
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
        rest_ms: wall_ms - gpu_span_ms,
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
                None => eprintln!("  {name:<18} (no reading)"),
            }
        }
        eprintln!(
            "  gpu span {:7.2} ms   wall submit+wait {:7.2} ms   copies+latency {:7.2} ms",
            self.gpu_span_ms, self.wall_ms, self.rest_ms
        );
    }
}

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

    /// A compute pass's timestamp writes for `p`.
    pub(crate) fn pass_writes(&self, p: Pair) -> wgpu::ComputePassTimestampWrites<'_> {
        wgpu::ComputePassTimestampWrites {
            query_set: &self.set,
            beginning_of_pass_write_index: Some(p.begin),
            end_of_pass_write_index: Some(p.end),
        }
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
            .as_chunks::<8>()
            .0
            .iter()
            .map(|c| u64::from_le_bytes(*c))
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
        assert!((t.rest_ms - 6.5).abs() < 1e-9, "rest {}", t.rest_ms);
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
