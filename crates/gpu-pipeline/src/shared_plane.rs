//! A one-shot hand-off slot for a GPU buffer two collaborators both need.
//!
//! ⚠⚠ **Why this exists: a masked frame uploads the same plane TWICE.** `MaskStage::apply` hands
//! the auto-mask gate `image.data()`, then hands its first sub-stage a clone of that same `image`
//! — the same bytes, in the same packed `w*h*3` f32 layout a [`crate::GpuImage`] holds. Measured
//! on the catalogue's masked photograph at rung 3200: ~2.1 ms of host memcpy plus ~6–10 ms of
//! device copy, paid twice per frame.
//!
//! ⚠ The two participants never meet — `accelerate` builds the rasteriser and the chain
//! separately — so the hand-off goes through the one thing both hold, the `GpuContext`.
//!
//! ⚠⚠ **Generic over the payload so its rules are testable with no GPU at all.** What this type
//! enforces is bookkeeping — dimensions, ownership, one use, clearing — and none of that is about
//! wgpu. Production instantiates it at `GpuImage`; the tests below use a `u32`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

/// What a deposit carries besides the payload.
struct Deposit<T> {
    payload: T,
    w: usize,
    h: usize,
    /// ⚠⚠ **[`crate::shared`] is process-wide, so without this a preview render and an export
    /// render on two threads could hand each other a plane.** A legitimate hand-off is always
    /// same-thread: `MaskStage::apply` builds its coverage and runs its sub-stages on one call
    /// stack. Stamping the thread makes the cross-thread case *unrepresentable* rather than merely
    /// unlikely — the posture `render_max_dim` and `Session::geometry_mode` were each fixed to.
    by: std::thread::ThreadId,
}

/// How many threads' deposits are held at once. ⚠ A bound, not a working set: the render worker
/// and a few export threads are the real users, and a thread that deposits and exits without a
/// take would otherwise pin a GPU buffer for the life of the process.
pub(crate) const MAX_DEPOSITS: usize = 16;

/// One deposited payload **per thread**.
///
/// ⚠⚠ **Per thread since 2026-09-26, and it was one slot before.** Every legitimate hand-off is
/// same-thread, so a single slot added nothing but interference: another thread's deposit or
/// `clear` evicted this one's between its deposit and its take. `take` already refused the
/// cross-thread case, so the picture was never wrong — the hand-off just silently did not happen,
/// which during a batch export meant every live frame paid the fallback.
pub struct SharedPlane<T> {
    slot: Mutex<Vec<Deposit<T>>>,
    deposits: AtomicUsize,
    takes: AtomicUsize,
}

/// ⚠ Hand-written rather than derived, and the derive is actively wrong here: `#[derive(Default)]`
/// on a generic adds a `T: Default` bound, which a GPU buffer handle cannot satisfy — an empty
/// slot needs no payload at all. Caught by the compiler on the first build.
impl<T> Default for SharedPlane<T> {
    fn default() -> Self {
        Self {
            slot: Mutex::new(Vec::new()),
            deposits: AtomicUsize::new(0),
            takes: AtomicUsize::new(0),
        }
    }
}

/// ⚠ Hand-written rather than derived: `GpuContext` is `Debug` and a derive would demand
/// `T: Debug` of every payload. The counters are the interesting part anyway — the payload is a
/// GPU buffer handle, which prints nothing a reader can use.
impl<T> std::fmt::Debug for SharedPlane<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SharedPlane")
            .field(
                "occupied",
                &self.slot.lock().map(|g| !g.is_empty()).unwrap_or(false),
            )
            .field("deposits", &self.deposits())
            .field("takes", &self.takes())
            .finish()
    }
}

impl<T> SharedPlane<T> {
    /// Empty the slot, dropping whatever is in it.
    ///
    /// ⚠⚠ **This is what stops a stale plane being taken, and it is the guard most likely to be
    /// "simplified" away.** A frame whose coverage comes from `BrushCache`'s prebuilt map
    /// rasterises nothing and so deposits nothing; without a clear, its sub-stage chain would find
    /// the *previous* frame's plane and render those pixels. The failure is a plausible
    /// photograph, not an error — the silent preview/export divergence this project has recorded
    /// more than any other.
    ///
    /// ⚠ **This thread's slot only.** Clearing another thread's deposit is exactly the interference
    /// the per-thread slots exist to remove.
    pub fn clear(&self) {
        let me = std::thread::current().id();
        if let Ok(mut g) = self.slot.lock() {
            g.retain(|d| d.by != me);
        }
    }

    /// Park `payload`, replacing anything already there.
    pub fn deposit(&self, w: usize, h: usize, payload: T) {
        let me = std::thread::current().id();
        if let Ok(mut g) = self.slot.lock() {
            // Replaces only THIS thread's deposit.
            g.retain(|d| d.by != me);
            // ⚠ The oldest goes first when the bound is reached: a thread that deposited and
            // exited is the likeliest owner of the oldest entry.
            if g.len() >= MAX_DEPOSITS {
                g.remove(0);
            }
            g.push(Deposit {
                payload,
                w,
                h,
                by: me,
            });
            self.deposits.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// How many threads' deposits are held right now.
    #[cfg(test)]
    pub(crate) fn held(&self) -> usize {
        self.slot.lock().map(|g| g.len()).unwrap_or(0)
    }

    /// Take the deposit, if it is this thread's and describes a `w`×`h` plane.
    ///
    /// ⚠ **Take and clear — one use.** Leaving it would let a second consumer within the same
    /// apply — a mask whose sub-stages form two chains around a CPU stage — take a plane the first
    /// has already mutated in place.
    pub fn take(&self, w: usize, h: usize) -> Option<T> {
        let me = std::thread::current().id();
        let mut g = self.slot.lock().ok()?;
        // ⚠ THIS thread's deposit, removed unconditionally *before* the size is checked: a deposit
        // that does not match is stale by definition, and leaving it for a later asker is exactly
        // the staleness this type exists to prevent. Another thread's deposit is not visible here.
        let at = g.iter().position(|d| d.by == me)?;
        let d = g.remove(at);
        if d.w == w && d.h == h {
            self.takes.fetch_add(1, Ordering::Relaxed);
            Some(d.payload)
        } else {
            None
        }
    }

    /// How many deposits this slot has accepted.
    pub fn deposits(&self) -> usize {
        self.deposits.load(Ordering::Relaxed)
    }

    /// How many deposits have been taken.
    ///
    /// ⚠⚠ **The observable, and it exists for [`crate::GpuChain::transfer_counts`]' reason.** A
    /// shared upload changes no pixel any test can read — the same bytes reach the same buffer by
    /// a different route — so without a counter the whole mechanism could be deleted and every
    /// equivalence test in the workspace would stay green. That is the W5 failure mode.
    ///
    /// ⚠⚠ Read it only from a **privately constructed** [`crate::GpuContext`]. On the process-wide
    /// [`crate::shared`] one it is contaminated by whatever else is rendering, which is precisely
    /// the bug that made `GpuChain`'s transfer counters per chain rather than global.
    pub fn takes(&self) -> usize {
        self.takes.load(Ordering::Relaxed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_deposit_is_taken_once_and_then_gone() {
        let s: SharedPlane<u32> = Default::default();
        s.deposit(4, 5, 7);
        assert_eq!(s.take(4, 5), Some(7));
        assert_eq!(s.take(4, 5), None, "a deposit is good for exactly one use");
        assert_eq!((s.deposits(), s.takes()), (1, 1));
    }

    #[test]
    fn an_empty_slot_yields_nothing() {
        let s: SharedPlane<u32> = Default::default();
        assert_eq!(s.take(4, 5), None);
        assert_eq!(s.takes(), 0);
    }

    /// ⚠ The dimensions are what say a deposit describes the plane being asked for. A mask
    /// rendering a region and a chain rendering the whole frame would otherwise trade buffers.
    #[test]
    fn a_deposit_of_another_size_is_refused() {
        let s: SharedPlane<u32> = Default::default();
        s.deposit(4, 5, 7);
        assert_eq!(s.take(5, 4), None, "transposed dimensions are not a match");
        assert_eq!(s.takes(), 0);
    }

    /// ⚠⚠ A refused deposit must also be **gone**, not left for the next asker — it is stale by
    /// definition, and leaving it is the staleness this type exists to prevent.
    #[test]
    fn a_refused_deposit_is_dropped_rather_than_left_behind() {
        let s: SharedPlane<u32> = Default::default();
        s.deposit(4, 5, 7);
        assert_eq!(s.take(9, 9), None);
        assert_eq!(s.take(4, 5), None, "the stale deposit must not survive");
    }

    #[test]
    fn clearing_discards_the_deposit() {
        let s: SharedPlane<u32> = Default::default();
        s.deposit(4, 5, 7);
        s.clear();
        assert_eq!(s.take(4, 5), None);
    }

    /// ⚠⚠ The cross-thread case, which [`crate::shared`] being process-wide makes real: a preview
    /// render and an export render must never hand each other a plane.
    #[test]
    fn another_threads_deposit_is_refused() {
        let s: std::sync::Arc<SharedPlane<u32>> = Default::default();
        let d = s.clone();
        std::thread::spawn(move || d.deposit(4, 5, 7))
            .join()
            .unwrap();
        assert_eq!(
            s.take(4, 5),
            None,
            "a plane deposited on another thread must not be taken here"
        );
        assert_eq!(s.takes(), 0);
    }

    /// ⚠⚠ **Another thread must not DISTURB my deposit either (2026-09-26).** One slot per
    /// context meant a batch-export thread's `clear` or `deposit` could land between the render
    /// worker's deposit and its take — correct, because `take` refuses another thread's plane, but
    /// silently falling back on every live frame during an export. Found as a test that passed
    /// alone and failed in the parallel suite.
    #[test]
    fn another_threads_traffic_leaves_my_deposit_alone() {
        let s: std::sync::Arc<SharedPlane<u32>> = Default::default();
        s.deposit(4, 5, 7);
        let d = s.clone();
        std::thread::spawn(move || {
            d.deposit(4, 5, 99);
            d.clear();
            d.deposit(8, 8, 1);
        })
        .join()
        .unwrap();
        assert_eq!(
            s.take(4, 5),
            Some(7),
            "my deposit must survive another thread"
        );
    }

    /// The per-thread slots are bounded: a thread that deposits and never returns cannot pin
    /// planes forever.
    #[test]
    fn abandoned_deposits_are_bounded() {
        let s: std::sync::Arc<SharedPlane<u32>> = Default::default();
        for i in 0..(MAX_DEPOSITS as u32 + 8) {
            let d = s.clone();
            std::thread::spawn(move || d.deposit(1, 1, i))
                .join()
                .unwrap();
        }
        assert!(s.held() <= MAX_DEPOSITS, "held {}", s.held());
    }
}
