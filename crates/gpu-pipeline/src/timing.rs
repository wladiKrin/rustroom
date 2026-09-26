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
