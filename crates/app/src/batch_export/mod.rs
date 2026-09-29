//! Exporting a selection of photos to JPEG files.
//!
//! ⚠⚠ **NOT sequential — this paragraph said it was until 2026-09-24, and the claim was stale
//! enough to mislead a whole spec section into asserting it.** `run` opens a **`width`-wide**
//! `thread::scope` pool over a shared cursor, where `width = Settings::export_width()` =
//! `export_concurrency` clamped to `[1, cores]`, and each worker takes a whole photo (decode,
//! render, write). The honest consequence is that **peak memory is `width` whole photos** — a
//! 26 Mpx decode is ~312 MB as f32 RGB — which is exactly what that setting exists to bound.
//!
//! The original reasoning below was sound *for one photo at a time* and is kept because it is why
//! the ceiling is the core count rather than something larger: the pipeline stages are already
//! rayon-parallel *internally*, so a wide run contends for the same cores.
//!
//! ⚠ Before requoting anything here, check it against `run`.
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
pub(crate) mod fixtures;
