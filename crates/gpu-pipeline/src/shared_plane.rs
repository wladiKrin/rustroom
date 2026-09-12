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

/// A slot holding at most one deposited payload.
pub struct SharedPlane<T> {
    slot: Mutex<Option<Deposit<T>>>,
    deposits: AtomicUsize,
    takes: AtomicUsize,
}

/// ⚠ Hand-written rather than derived, and the derive is actively wrong here: `#[derive(Default)]`
/// on a generic adds a `T: Default` bound, which a GPU buffer handle cannot satisfy — an empty
/// slot needs no payload at all. Caught by the compiler on the first build.
impl<T> Default for SharedPlane<T> {
    fn default() -> Self {
        Self {
            slot: Mutex::new(None),
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
                &self.slot.lock().map(|g| g.is_some()).unwrap_or(false),
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
    pub fn clear(&self) {
        if let Ok(mut g) = self.slot.lock() {
            *g = None;
        }
    }

    /// Park `payload`, replacing anything already there.
    pub fn deposit(&self, w: usize, h: usize, payload: T) {
        if let Ok(mut g) = self.slot.lock() {
            *g = Some(Deposit {
                payload,
                w,
                h,
                by: std::thread::current().id(),
            });
            self.deposits.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Take the deposit, if it is this thread's and describes a `w`×`h` plane.
    ///
    /// ⚠ **Take and clear — one use.** Leaving it would let a second consumer within the same
    /// apply — a mask whose sub-stages form two chains around a CPU stage — take a plane the first
    /// has already mutated in place.
    pub fn take(&self, w: usize, h: usize) -> Option<T> {
        let mut g = self.slot.lock().ok()?;
        // ⚠ Removed unconditionally, *before* the match is checked: a deposit that does not match
        // is stale by definition, and leaving it for a later asker is exactly the staleness this
        // type exists to prevent.
        let d = g.take()?;
        if d.w == w && d.h == h && d.by == std::thread::current().id() {
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
}
