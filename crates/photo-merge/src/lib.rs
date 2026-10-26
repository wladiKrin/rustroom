//! Photo Merge (M10.17): several photos of one scene into one. **HDR** (M10.17a) — a bracket aligned,
//! deghosted and merged into a floating-point raw of the same camera.
//!
//! ⚠ Zero GUI: `rustroom-cli merge-hdr`, the app's worker and the tests drive it.

pub mod align;
pub mod deghost;
pub mod exposure;
pub mod frame;
pub mod homography;
pub mod merge;
pub mod pyramid;
#[cfg(test)]
mod synth;
pub mod warp;
