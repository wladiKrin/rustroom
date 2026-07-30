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

/// A decoded embedded preview thumbnail (display RGBA8).
#[derive(Debug, Clone)]
pub struct Thumbnail {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// Extract and decode the camera's embedded preview thumbnail (fast — no demosaic).
pub fn extract_thumbnail(path: &Path) -> Result<Thumbnail, DecodeError> {
    let bytes = std::fs::read(path)?;
    backend_rsraw::extract_thumbnail(&bytes)
}

/// How libraw should render the RAW.
#[derive(Debug, Clone, Copy)]
pub struct DecodeOptions {
    /// Use the camera's own white balance (falls back to auto-WB).
    pub camera_wb: bool,
    /// If true, output linear light (gamma 1.0). If false, libraw's default display gamma.
    pub linear: bool,
    /// If true, disable libraw's automatic brightness stretch (needed for scene-referred).
    pub no_auto_bright: bool,
    /// Decode at libraw half-size — much faster; used for the editing preview.
    pub half_size: bool,
}

impl DecodeOptions {
    /// Scene-referred linear working image (what the develop pipeline consumes).
    pub fn linear_scene() -> Self {
        Self {
            camera_wb: true,
            linear: true,
            no_auto_bright: true,
            half_size: false,
        }
    }

    /// Display-ready sRGB (quick viewable dump — used by `rustroom decode`).
    pub fn display() -> Self {
        Self {
            camera_wb: true,
            linear: false,
            no_auto_bright: false,
            half_size: false,
        }
    }

    /// Fast, scene-referred linear decode for interactive editing (half resolution).
    pub fn preview() -> Self {
        Self {
            camera_wb: true,
            linear: true,
            no_auto_bright: true,
            half_size: true,
        }
    }
}

impl Default for DecodeOptions {
    fn default() -> Self {
        Self::linear_scene()
    }
}

/// Decode a RAW file to an [`ImageBuf`] (f32 RGB) + metadata, per `opts`.
///
/// With [`DecodeOptions::linear_scene`] the samples are linear light in sRGB primaries.
/// With [`DecodeOptions::display`] they are sRGB-encoded (display-referred).
pub fn decode_raw(path: &Path, opts: &DecodeOptions) -> Result<DecodedRaw, DecodeError> {
    let bytes = std::fs::read(path)?;
    backend_rsraw::decode(&bytes, opts)
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
        let decoded =
            decode_raw(&path, &DecodeOptions::linear_scene()).expect("decode should succeed");
        assert!(decoded.metadata.width > 0 && decoded.metadata.height > 0);
        assert_eq!(
            decoded.image.data().len(),
            decoded.metadata.width * decoded.metadata.height * 3
        );
        // A real photo is not uniformly black.
        assert!(decoded.image.data().iter().any(|&v| v > 0.01));
    }

    #[test]
    fn extracts_thumbnail_from_fixture() {
        let Some(path) = fixture() else {
            eprintln!("skipping: set RAW_FIXTURE=/abs/path.raw to run");
            return;
        };
        let t = extract_thumbnail(&path).expect("thumbnail should extract");
        assert!(t.width > 0 && t.height > 0);
        assert_eq!(t.rgba.len(), t.width * t.height * 4);
        assert!(t.rgba.iter().any(|&b| b > 0));
    }
}
