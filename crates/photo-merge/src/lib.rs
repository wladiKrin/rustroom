//! Photo Merge (M10.17): several photos of one scene into one. **HDR** (M10.17a) — a bracket aligned,
//! deghosted and merged into a floating-point raw of the same camera.
//!
//! ⚠ Zero GUI: `rustroom-cli merge-hdr`, the app's worker and the tests drive it.

pub mod align;
pub mod blend;
pub mod crop;
pub mod deghost;
pub mod exposure;
pub mod features;
pub mod fill;
pub mod frame;
pub mod gain;
pub mod hdr;
pub mod homography;
pub mod lens;
pub mod merge;
pub mod pano;
pub mod pano_warp;
pub mod preview;
pub mod projection;
pub mod pyramid;
pub mod refine;
pub mod rig;
pub mod rotation;
pub mod seam;
#[cfg(test)]
mod synth;
#[cfg(test)]
mod synth_pano;
pub mod warp;
