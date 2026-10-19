//! AI masks (M9.13): the models behind *Select Subject* and *Select by click*, how they arrive, and
//! how they run.
//!
//! ⚠ Zero GUI, zero pipeline dependency — the app maps the result into the pipeline's coverage.
//! ⚠⚠ Runs on **`tract`** (pure Rust), chosen by measurement in the M9.13 spike: 2.7–2.8× slower than
//! ONNX Runtime on CPU, within the spec's "~3×" rule, and CoreML could not compile BiRefNet at all.
//! No native library, identical on every platform. The spike's table is in the spec.

pub mod denoise;
pub mod depth;
pub mod download;
pub mod infer;
pub mod map;
pub mod prep;
pub mod registry;

pub use infer::{Embedding, InferError, Sam, Subject};
pub use map::ProbMap;
pub use registry::{ModelId, ModelSpec, SubjectModel};

/// Lower-case hex SHA-256 of `bytes` — the AI map cache's key.
pub fn digest_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
