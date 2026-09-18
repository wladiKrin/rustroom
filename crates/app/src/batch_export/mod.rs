//! Exporting a selection of photos to JPEG files.
//!
//! **Sequential on purpose.** The pipeline stages are already rayon-parallel *internally*, so
//! exporting several photos at once would contend for the same cores while multiplying peak
//! memory — a 26 Mpx decode is ~312 MB as f32 RGB, and `PipelineStage::apply` consumes and
//! produces buffers. One at a time keeps the ceiling at roughly one image.
//!
//! The decode fn is **injected**, the same shape `DecodeWorker::new` uses, so the whole loop —
//! naming, collisions, cancellation, progress, the failure path — is testable with no image
//! fixtures at all.

mod destination;
mod options;
mod render;
mod worker;

// ⚠⚠ Glob re-exports, so every existing caller keeps working unchanged — this is a **move**, and a
// move that forces its callers to edit their imports is not one.
pub use destination::*;
pub use options::*;
pub use render::*;
pub use worker::*;

#[cfg(test)]
mod fixtures;
