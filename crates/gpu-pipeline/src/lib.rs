//! The GPU seam: `pipeline::PipelineStage` implementations backed by `wgpu` compute shaders.
//!
//! ⚠⚠ **M8.3a is a proof, not a speed-up, and it is measurably SLOWER.** Measured on an Apple M2:
//! compute is **0.04 ms** while moving a 19.6 MB buffer is **~6 ms**, a factor of ~70 — so one GPU
//! stage inside a CPU pipeline costs a round trip to save nothing. That was `CLAUDE.md`'s standing
//! prediction and is now a number. The win needs the whole tail resident, which is M8.3b.
//!
//! ⚠ Nothing in the app uses this crate. See the spec's §8 for why that is deliberate.

mod accelerate;
mod brush_raster;
mod brush_wgsl;
mod cache;
mod chain;
mod context;
mod contrast;
mod dispatch;
mod display_rendering;
mod equivalence;
mod exposure;
mod grading;
mod hsl;
mod image;
mod luma_wgsl;
mod oklab_wgsl;
mod output_transform;
mod quantize;
mod shared_plane;
mod sharpen;
mod tone_curve;
mod tone_regions;
mod white_balance;

pub use accelerate::{accelerate, rejoin};
pub use brush_raster::GpuBrushRaster;

/// How many dabs one 16×16 tile's shared list holds before the rasteriser re-scans.
///
/// ⚠ Exposed so a probe can report the real occupancy against it — the measurement that keeps that
/// constant defensible rather than estimated.
pub fn max_tile_dabs() -> u32 {
    brush_wgsl::MAX_TILE_DABS
}
pub use chain::{GpuChain, GpuStage};
pub use context::{shared, GpuContext};
pub use contrast::GpuContrastStage;
pub use display_rendering::GpuDisplayRenderingStage;
pub use equivalence::{compare, Divergence};
pub use exposure::GpuExposureStage;
pub use grading::GpuGradingStage;
pub use hsl::GpuHslStage;
pub use image::GpuImage;
pub use output_transform::GpuOutputTransformStage;
pub use shared_plane::SharedPlane;
pub use sharpen::GpuSharpenStage;
pub use tone_curve::GpuToneCurveStage;
pub use tone_regions::GpuToneRegionsStage;
pub use white_balance::GpuWhiteBalanceStage;
