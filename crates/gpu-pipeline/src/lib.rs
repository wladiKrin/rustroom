//! The GPU seam: `pipeline::PipelineStage` implementations backed by `wgpu` compute shaders.
//!
//! ⚠⚠ **M8.3a is a proof, not a speed-up, and it is measurably SLOWER.** Measured on an Apple M2:
//! compute is **0.04 ms** while moving a 19.6 MB buffer is **~6 ms**, a factor of ~70 — so one GPU
//! stage inside a CPU pipeline costs a round trip to save nothing. That was `CLAUDE.md`'s standing
//! prediction and is now a number. The win needs the whole tail resident, which is M8.3b.
//!
//! ⚠ Nothing in the app uses this crate. See the spec's §8 for why that is deliberate.

mod accelerate;
mod box_wgsl;
mod brush_raster;
mod brush_wgsl;
mod cache;
// ⚠ `#[cfg(test)]` because every item in it is reachable only from its own `#[ignore]`d probe.
// A `pub(crate)` item in a LIBRARY whose only callers are `#[cfg(test)]` is dead code for the lib
// target and `clippy -D warnings` rejects it — the trap this file's own history records for bin
// crates, arriving in a library through `pub(crate)`.
#[cfg(test)]
mod box_probe;
mod chain;
mod context;
mod contrast;
mod creative_look;
mod dehaze;
mod dispatch;
mod display_rendering;
mod equivalence;
mod exposure;
mod geometry;
mod grading;
mod hsl;
mod image;
mod lens_correction;
mod local_contrast;
mod luma_wgsl;
mod mask;
mod mask_wgsl;
mod min_wgsl;
mod noise_reduction;
mod oklab_wgsl;
mod output_transform;
mod per_pixel;
mod quantize;
mod saturation;
mod shared_plane;
mod sharpen;
mod srgb_wgsl;
mod timing;
mod tone_curve;
mod tone_regions;
mod vignette;
mod white_balance;

/// Run a GPU attempt, turning a **panic** into `None` so the caller takes the CPU path.
///
/// ⚠⚠ **Because a GPU failure that kills the thread is the one failure this crate's design does
/// not already handle.** Everything else degrades silently: `apply` has no failure channel, so
/// every `?` inside `run_with` lands on `cpu_fallback`. But `wgpu` reports a validation error by
/// **panicking through its uncaptured-error handler**, and rendering happens on a worker thread —
/// so a single bad frame ended the session's rendering entirely. Reported as the sliders doing
/// nothing at 1:1 while the thumbnail kept updating, and cured by switching photos, which builds a
/// new engine and therefore a new worker.
///
/// ⚠ Deliberately wrapped around the **GPU attempt alone**, not the whole render. A panic in the
/// CPU pipeline is a real bug and must still be loud; this converts only "the GPU refused" into
/// "the CPU rendered it", which is what every other GPU failure here already means.
///
/// ⚠ It prints, once per occurrence. A silent fallback would hide a performance cliff — the frame
/// still arrives, several times slower, with nothing to say why.
pub(crate) fn guarded<T>(f: impl FnOnce() -> Option<T>) -> Option<T> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(v) => v,
        Err(_) => {
            eprintln!("rustroom: the GPU path panicked; this frame fell back to the CPU");
            None
        }
    }
}

pub use accelerate::{accelerate, accelerate_prefix_stage, rejoin, SeamFacts, TailFacts};
pub use brush_raster::GpuBrushRaster;

/// How many dabs one 16×16 tile's shared list holds before the rasteriser re-scans.
///
/// ⚠ Exposed so a probe can report the real occupancy against it — the measurement that keeps that
/// constant defensible rather than estimated.
pub fn max_tile_dabs() -> u32 {
    brush_wgsl::MAX_TILE_DABS
}
pub use chain::{GpuChain, GpuStage, Slot};
pub use context::{shared, GpuContext};
pub use contrast::GpuContrastStage;
pub use creative_look::GpuCreativeLookStage;
pub use dehaze::GpuDehazeStage;
pub use display_rendering::GpuDisplayRenderingStage;
pub use equivalence::{compare, Divergence};
pub use exposure::GpuExposureStage;
pub use geometry::GpuGeometryStage;
pub use grading::GpuGradingStage;
pub use hsl::GpuHslStage;
pub use image::GpuImage;
pub use lens_correction::GpuLensCorrectionStage;
pub use local_contrast::GpuLocalContrastStage;
pub use mask::GpuMaskStage;
pub use noise_reduction::GpuNoiseReductionStage;
pub use output_transform::GpuOutputTransformStage;
pub use per_pixel::{fused_dispatches, fusion_enabled, Part, Preludes};
pub use saturation::GpuSaturationStage;
pub use shared_plane::SharedPlane;
pub use sharpen::GpuSharpenStage;
pub use timing::Timings;
pub use tone_curve::GpuToneCurveStage;
pub use tone_regions::GpuToneRegionsStage;
pub use vignette::GpuVignetteStage;
pub use white_balance::GpuWhiteBalanceStage;
