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
    /// M10.14f: the incremental state of the one Auto Mask component being painted — the render's brush cache keeps
    /// it across frames, `MaskStage` paints through it (`brush::incremental::rasterize_gated`).
    pub painting: Option<Painting>,
}

/// Where a gating brush's growing stroke is painted from, frame to frame.
pub struct Painting {
    /// `(mask, component)`.
    pub at: (usize, usize),
    /// What produced the base the dabs gate on — a change rebuilds the state.
    pub base_key: u64,
    pub state: std::sync::Arc<std::sync::Mutex<Option<brush::incremental::Incremental>>>,
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
            painting: None,
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

/// A map-reading component's coverage — an AI mask's or a depth range's (M10.16a) — resolved at the
/// frame size the stage RUNS at.
///
/// ⚠⚠ **Why lazy, and only in the prefix:** a mask's Noise reduction, Dehaze and Defringe are prefix
/// steps, built before the prefix has produced the frame they run on, so no frame-sized map can be
/// handed to them in advance — the reason `build_prefix_steps` gave them NONE, and an AI component
/// covered nothing there until M10.16a. A brush answers the same problem by rasterising itself when
/// handed none; this is that shape for a raster the app owns. The tail keeps its prebuilt maps.
pub trait RasterSource: Send + Sync {
    /// Coverage over a whole `frame_w × frame_h` frame.
    fn coverage(&self, frame_w: usize, frame_h: usize) -> brush::CoverageMap;
}

/// Where `build_prefix_steps` finds each map-reading component's [`RasterSource`] — the app's maps
/// (M10.16a). Injected the way `lens` and `airlight` are; `None` everywhere a caller has no maps (the
/// CLI, the goldens, the airlight replay).
pub trait AiRasters {
    /// The source for `mask` — an `Ai` or `Depth` component — and its map's identity, or `None` when
    /// its map has not arrived (it then covers nothing, `CoverageSource::Raster`'s rule).
    ///
    /// ⚠⚠ The identity goes into the step's cache key: without it a map arriving after a checkpoint
    /// was cached would leave the stale, uncovered picture standing. It must follow the map's CONTENT.
    fn source(&self, mask: &edit_model::Mask) -> Option<(std::sync::Arc<dyn RasterSource>, u64)>;

    /// A raw map read whole (M10.16d's Lens Blur) — its plane at the frame the step runs at, `0..=255`,
    /// and its identity (`source`'s rule: it follows the CONTENT); `None` until it arrives.
    fn plane(&self, _which: MapPlane) -> Option<(std::sync::Arc<dyn RasterSource>, u64)> {
        None
    }

    /// Lens Blur's focal depth for `lb`, 0 (near) ..= 1 (far) — `None` while a map it is measured on
    /// has not arrived.
    fn lens_focus(&self, _lb: &edit_model::LensBlur) -> Option<f32> {
        None
    }

    /// AI denoise's result (M10.16b) — the corrected photo, resolved at the frame the step runs at —
    /// and its identity; `None` until the job has finished for this photo.
    fn denoised(&self) -> Option<(std::sync::Arc<dyn ImageSource>, u64)> {
        None
    }

    /// A removal's fill (M10.16c1) and its identity (`source`'s rule: it follows the CONTENT); `None`
    /// until the job has computed it — the removal then pastes nothing.
    fn fill(
        &self,
        _r: &edit_model::Removal,
    ) -> Option<(std::sync::Arc<crate::stages::remove::Fill>, u64)> {
        None
    }
}

/// A whole image a prefix step reads, resolved at the frame it runs on (M10.16b: the denoised photo) —
/// [`RasterSource`]'s shape for an RGB image. `None` for a size it cannot answer exactly.
///
/// ⚠ Shared, never copied: at full resolution the denoised photo is 312 MB, and copying it on every prefix
/// run at 1:1 was most of a 110–160 ms stutter each time a partial result landed (M10.16b6).
pub trait ImageSource: Send + Sync {
    fn image(&self, w: usize, h: usize) -> Option<std::sync::Arc<core_image::ImageBuf>>;
}

/// A raw map [`AiRasters::plane`] hands over whole, beside a mask component's coverage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapPlane {
    /// The depth positions — 0 nearest, 255 farthest.
    Depth,
    /// The Subject map's alpha — through the AI mask's Refine curve at 0, so hair keeps its partial cover.
    Subject,
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
            painting: None,
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
