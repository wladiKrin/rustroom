//! The GPU seam: `pipeline::PipelineStage` implementations backed by `wgpu` compute shaders.
//!
//! ⚠⚠ **M8.3a is a proof, not a speed-up, and it is measurably SLOWER.** Measured on an Apple M2:
//! compute is **0.04 ms** while moving a 19.6 MB buffer is **~6 ms**, a factor of ~70 — so one GPU
//! stage inside a CPU pipeline costs a round trip to save nothing. That was `CLAUDE.md`'s standing
//! prediction and is now a number. The win needs the whole tail resident, which is M8.3b.
//!
//! ⚠ Nothing in the app uses this crate. See the spec's §8 for why that is deliberate.

mod accelerate;
mod chain;
mod context;
mod equivalence;
mod exposure;
mod hsl;
mod image;

pub use accelerate::accelerate;
pub use chain::{GpuChain, GpuStage};
pub use context::{shared, GpuContext};
pub use equivalence::{compare, Divergence};
pub use exposure::GpuExposureStage;
pub use hsl::GpuHslStage;
pub use image::GpuImage;
