//! A brush mask's coverage: the maps a render deposits and takes back, and the rasteriser seam.

use crate::brush;

/// Pre-rasterized coverage for a photo's brush masks, indexed by **`[mask][component]`** —
/// position in `Edits::masks`, then position within that mask's composite, component 0 being its
/// base shape.
///
/// ⚠ Optional and additive, following the `Region` precedent: [`build_pipeline`] and
/// [`build_pipeline_split`] delegate with `None`, so the CLI, export and every golden compile
/// untouched.
#[derive(Default)]
pub struct BrushCoverage {
    /// ⚠ Nested because a composite can hold a brush in **any** component. Flattening with a
    /// prefix sum was the alternative and was rejected: the sum would have to be recomputed
    /// identically by the stage and by the cache, and two derivations of one index is precisely
    /// how a sparse-versus-dense bug enters — this codebase has had four.
    pub maps: Vec<Vec<Option<std::sync::Arc<brush::CoverageMap>>>>,
    /// Where a stage reports coverage it had to build itself.
    ///
    /// ⚠⚠ **The read side is what a caller supplies; this is what a stage gives back.** A brush
    /// that gates on the image can only be rasterized where the image is — inside
    /// `MaskStage::apply` — so `render_engine`, which owns the cache key, would otherwise never
    /// see the map it wants to keep. See the coverage-cache spec's §4.0.
    ///
    /// ⚠ `Mutex` rather than `RefCell`: this crosses into stages that must stay `Sync`. It is
    /// uncontended — `apply` runs sequentially, once per mask per tail.
    pub built: Vec<Vec<std::sync::Mutex<Option<std::sync::Arc<brush::CoverageMap>>>>>,
}

impl BrushCoverage {
    pub fn get(&self, i: usize, j: usize) -> Option<std::sync::Arc<brush::CoverageMap>> {
        self.maps.get(i)?.get(j).and_then(|m| m.clone())
    }

    /// This mask's whole component row, or an empty slice when it has none.
    pub fn row(&self, i: usize) -> &[Option<std::sync::Arc<brush::CoverageMap>>] {
        self.maps.get(i).map_or(&[], |r| r.as_slice())
    }

    /// A coverage set with one empty deposit slot per component. `counts[i]` is mask `i`'s
    /// component count — `1 + refine.len()`.
    pub fn with_slots(counts: &[usize]) -> Self {
        Self {
            maps: counts.iter().map(|&n| vec![None; n]).collect(),
            built: counts
                .iter()
                .map(|&n| (0..n).map(|_| std::sync::Mutex::new(None)).collect())
                .collect(),
        }
    }

    /// Report coverage built at component `j` of mask `i`. Out of range is ignored.
    ///
    /// ⚠ Ignored rather than panicking: the mask list can change between this set being made and a
    /// stage running, and a render is not worth aborting over a slot that no longer exists.
    pub fn deposit(&self, i: usize, j: usize, map: std::sync::Arc<brush::CoverageMap>) {
        if let Some(slot) = self.built.get(i).and_then(|r| r.get(j)) {
            if let Ok(mut g) = slot.lock() {
                *g = Some(map);
            }
        }
    }

    /// Take what was reported at component `j` of mask `i`, clearing the slot.
    ///
    /// ⚠ Clearing is what stops a map surviving into a render it does not describe — the caller
    /// stores it under a key computed for *this* render.
    pub fn take_built(&self, i: usize, j: usize) -> Option<std::sync::Arc<brush::CoverageMap>> {
        self.built
            .get(i)?
            .get(j)
            .and_then(|s| s.lock().ok()?.take())
    }
}

/// Builds a brush's coverage from an already-planned dab list.
///
/// ⚠⚠ **This exists because `MaskStage` cannot call `gpu-pipeline`.** That crate depends on
/// `pipeline` and its own header records that nothing may depend on it — so a GPU rasteriser is
/// **injected** rather than referenced, exactly as [`SubStageAccelerator`] is and for the same
/// reason. A `pipeline` that reached upward would put wgpu into the CLI, `export` and all 63
/// goldens.
///
/// ⚠ `Send + Sync` because [`PipelineStage`] is, and a mask's coverage is built inside a stage
/// that then blends through `map_pixels_xy`'s rayon rows.
pub trait CoverageRasterizer: Send + Sync {
    /// Called once as every mask's `apply` finishes, on **every** exit path and on a panic.
    ///
    /// ⚠⚠ **This exists so an implementation can drop state scoped to ONE apply, and without it a
    /// shared GPU plane would be taken by the wrong frame.** A frame whose coverage comes from
    /// `BrushCache`'s prebuilt map rasterises nothing and so deposits nothing — and a sub-stage
    /// chain that then found a *previous* apply's plane still resident would render those pixels.
    /// The failure is a plausible photograph, not an error.
    ///
    /// ⚠⚠ **It hangs off the END of the apply, and moving it there was forced by a defect rather
    /// than chosen.** It used to run at the *top*, which reads as the safer place — clear first,
    /// then work. But a plane deposited *for* this mask is deposited by the stage **before** it:
    /// `GpuChain::offer_output` parks the very buffer the gate is about to need. A clear at the
    /// top destroys that fresh deposit before its only consumer can ask for it, so the hand-off
    /// silently never fires — no wrong pixels, just a permanently dead optimisation.
    ///
    /// ⚠ Clearing at the end is also **strictly tighter**, not merely different: a deposit can no
    /// longer outlive the apply that made it even by one stage, where a top-clear let it sit in
    /// the slot for the whole gap between two masks.
    ///
    /// ⚠ Deliberately **not** folded into [`Self::rasterize`]: a mask calls this even when it goes
    /// on to rasterise nothing at all, which is exactly the case that makes it necessary.
    ///
    /// ⚠ Defaulted to nothing, so an implementation holding no per-apply state ignores it.
    fn end_mask(&self) {}

    /// `None` declines, and the caller falls back to the CPU rasteriser.
    ///
    /// ⚠⚠ **Declining must always be safe, and every implementation should prefer it when
    /// unsure.** An adapter that cannot allocate, a box too small to repay a round trip, or a plan
    /// this implementation does not handle all return `None` and cost only the attempt. **Fail
    /// toward the CPU** — `GpuStage::cpu_cost`'s rule and `PrefixKey`'s, because a wrong answer
    /// here is wrong pixels rather than lost milliseconds.
    fn rasterize(
        &self,
        plan: &brush::DabPlan,
        invert: bool,
        base: Option<brush::BaseRef<'_>>,
    ) -> Option<brush::CoverageMap>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_map() -> std::sync::Arc<crate::brush::CoverageMap> {
        std::sync::Arc::new(crate::brush::CoverageMap {
            outside: 42,
            ..Default::default()
        })
    }

    /// ⚠⚠ The write side exists because `MaskStage` is the only thing holding the image, so it is
    /// the only thing that can build a gating brush's coverage — and `render_engine`, which owns
    /// the key, has to be able to see what it built. See the coverage-cache spec's §4.0.
    #[test]
    fn a_deposited_map_can_be_taken_back_out() {
        let c = BrushCoverage::with_slots(&[1, 3]);
        assert!(c.take_built(1, 2).is_none(), "nothing deposited yet");
        c.deposit(1, 2, a_map());
        assert!(c.take_built(1, 2).is_some(), "the deposit must be readable");
        assert!(
            c.take_built(1, 2).is_none(),
            "taking it must clear the slot, so a stale map cannot be read twice"
        );
    }

    /// ⚠⚠ A composite's components must not see each other's coverage. Filing a refinement's map
    /// under its base's index is a **stale hit** — the shape renders positionally while the tint
    /// shows the gate working — and that is the one direction this cache must never fail in.
    #[test]
    fn a_deposit_lands_under_its_own_component() {
        let c = BrushCoverage::with_slots(&[1, 3]);
        c.deposit(1, 2, a_map());
        assert!(
            c.take_built(1, 1).is_none(),
            "a sibling component must not see it"
        );
        assert!(c.take_built(0, 0).is_none(), "another mask must not see it");
        assert!(
            c.take_built(1, 2).is_some(),
            "and it is still where it was put"
        );
    }

    /// ⚠ An index past the end is ignored rather than panicking: the mask list can change between
    /// this set being made and a stage running, exactly as `suppressed_spot`'s index can.
    #[test]
    fn depositing_out_of_range_is_ignored() {
        let c = BrushCoverage::with_slots(&[1]);
        c.deposit(7, 0, a_map());
        c.deposit(0, 9, a_map());
        assert!(c.take_built(0, 0).is_none());
    }

    /// ⚠ The read side reads by `(mask, component)` now, and `row` is what the masks loop hands
    /// a stage — one lookup rather than a per-component fetch it would have to index correctly.
    #[test]
    fn the_prebuilt_side_reads_by_mask_and_component() {
        let c = BrushCoverage {
            maps: vec![vec![None], vec![None, Some(a_map())]],
            built: Vec::new(),
        };
        assert!(c.get(0, 0).is_none());
        assert!(c.get(1, 1).is_some());
        assert!(
            c.get(1, 0).is_none(),
            "the base of a composite is its own slot"
        );
        assert!(c.get(9, 0).is_none(), "past the end is None, never a panic");
        assert_eq!(c.row(1).len(), 2);
        assert!(
            c.row(9).is_empty(),
            "a mask with no row hands back an empty slice"
        );
    }
}
