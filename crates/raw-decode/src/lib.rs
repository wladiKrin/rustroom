//! RAW decoding: file on disk -> RGB `ImageBuf` + camera metadata.
//!
//! The libraw backend (via the `rsraw` crate) is an implementation detail; the public
//! API here is backend-agnostic so it can be swapped (e.g. for pure-Rust `rawler`)
//! without touching downstream code.
//!
//! NOTE (Phase 0 / M0.2 spike): [`decode_raw`] currently returns libraw's *default-
//! processed* output — demosaiced, camera white balance applied, and sRGB-gamma
//! encoded (display-referred). That is exactly what we want to prove decoding works
//! and to write a viewable image. Phase 1 / M1.1 replaces this with a linear,
//! scene-referred extraction feeding the develop pipeline.

use core_image::ImageBuf;
use std::path::Path;

mod backend_rsraw;

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("could not read RAW file: {0}")]
    Io(#[from] std::io::Error),
    #[error("libraw failed to decode: {0}")]
    Backend(String),
    #[error("unexpected decoded format: {0}")]
    Format(String),
}

/// Camera/sensor metadata recovered from the RAW.
#[derive(Debug, Clone)]
pub struct RawMetadata {
    pub width: usize,
    pub height: usize,
    pub make: String,
    pub model: String,
}

/// A fully decoded RAW: demosaiced RGB plus metadata.
#[derive(Debug, Clone)]
pub struct DecodedRaw {
    pub image: ImageBuf,
    pub metadata: RawMetadata,
}

/// Decode a RAW file to an [`ImageBuf`] (f32 RGB, samples in `0.0..=1.0`) and metadata.
pub fn decode_raw(path: &Path) -> Result<DecodedRaw, DecodeError> {
    let bytes = std::fs::read(path)?;
    backend_rsraw::decode(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// Set `RAW_FIXTURE=/abs/path/to.raw` to exercise a real decode locally / in CI.
    fn fixture() -> Option<PathBuf> {
        std::env::var("RAW_FIXTURE")
            .ok()
            .map(PathBuf::from)
            .filter(|p| p.exists())
    }

    #[test]
    fn decodes_fixture_to_nonempty_image() {
        let Some(path) = fixture() else {
            eprintln!("skipping: set RAW_FIXTURE=/abs/path/to.raw to run this test");
            return;
        };
        let decoded = decode_raw(&path).expect("decode should succeed");
        assert!(decoded.metadata.width > 0 && decoded.metadata.height > 0);
        assert_eq!(
            decoded.image.data().len(),
            decoded.metadata.width * decoded.metadata.height * 3
        );
        // A real photo is not uniformly black.
        assert!(decoded.image.data().iter().any(|&v| v > 0.01));
    }
}
