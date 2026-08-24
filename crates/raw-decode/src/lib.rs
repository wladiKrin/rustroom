//! RAW decoding: file on disk -> RGB `ImageBuf` + camera metadata.
//!
//! The libraw backend (via the `rsraw` crate) is an implementation detail; the public
//! API here is backend-agnostic so it can be swapped (e.g. for pure-Rust `rawler`)
//! without touching downstream code.
//!
//! NOTE (Phase 0 / M0.2 spike): [`decode_raw`] currently returns libraw's *default-
//! processed* output — demosaiced, camera white balance applied, in ProPhoto primaries for
//! the editing path (sRGB-gamma
//! encoded (display-referred). That is exactly what we want to prove decoding works
//! and to write a viewable image. Phase 1 / M1.1 replaces this with a linear,
//! scene-referred extraction feeding the develop pipeline.

use core_image::ImageBuf;
use serde::{Deserialize, Serialize};
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

pub use core_image::Thumbnail;

/// Extract and decode the camera's embedded preview thumbnail (fast — no demosaic).
pub fn extract_thumbnail(path: &Path) -> Result<Thumbnail, DecodeError> {
    let bytes = std::fs::read(path)?;
    backend_rsraw::extract_thumbnail(&bytes)
}

/// Which demosaic algorithm libraw should use, as a **speed/quality tier**.
///
/// ⚠ Labelled by tier rather than by algorithm on purpose. libraw's `user_qual` names
/// (bilinear/VNG/PPG/AHD) describe *Bayer* algorithms, and on **X-Trans** libraw routes every
/// setting through Markesteijn variants instead — measured: `user_qual` 3 and 4 produce
/// **byte-identical** output at identical cost on an X-T4. A label that is false on the
/// user's own camera is worse than no label; the tier is true on every sensor.
///
/// Measured full-resolution on an X-T4 `.RAF` (`unpack` a flat ~845 ms on top of each):
/// `Fastest` 400 ms, `Fast` 2,049 ms, `Better` 6,888 ms, `Best` 10,575 ms. Reproduce with
/// `cargo run --release -p raw-decode --example demosaic_probe <raw> <out-dir>`.
///
/// ⚠ **Inert for a `half_size` decode**, which is what the editing preview uses: libraw skips
/// demosaic entirely there (2×2 binning). This is an export-time decision and cannot be
/// previewed — see `a_half_size_decode_ignores_the_demosaic_setting`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Demosaic {
    /// libraw `user_qual` 0 — bilinear on Bayer.
    Fastest,
    /// libraw `user_qual` 1 — VNG on Bayer. The default: 3.9x faster end-to-end than `Best`,
    /// and visibly better than `Fastest`.
    #[default]
    Fast,
    /// libraw `user_qual` 2 — PPG on Bayer.
    Better,
    /// libraw `user_qual` 3 — AHD on Bayer. ⚠ On X-Trans this is where the scale tops out:
    /// `user_qual` 4 and above are identical to it.
    Best,
}

impl Demosaic {
    /// Every tier, in order, for populating a dropdown.
    pub const ALL: [Demosaic; 4] = [
        Demosaic::Fastest,
        Demosaic::Fast,
        Demosaic::Better,
        Demosaic::Best,
    ];

    /// The libraw `user_qual` value. The single place this mapping exists.
    pub fn user_qual(self) -> i32 {
        match self {
            Demosaic::Fastest => 0,
            Demosaic::Fast => 1,
            Demosaic::Better => 2,
            Demosaic::Best => 3,
        }
    }

    /// The label shown in the UI.
    pub fn label(self) -> &'static str {
        match self {
            Demosaic::Fastest => "Fastest",
            Demosaic::Fast => "Fast",
            Demosaic::Better => "Better",
            Demosaic::Best => "Best",
        }
    }
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
    /// Demosaic quality. ⚠ Inert when `half_size` is true — libraw skips demosaic entirely
    /// for a half-size decode, so the editing preview cannot show this setting.
    pub demosaic: Demosaic,
}

impl DecodeOptions {
    /// Scene-referred linear working image (what the develop pipeline consumes).
    pub fn linear_scene() -> Self {
        Self {
            camera_wb: true,
            linear: true,
            no_auto_bright: true,
            half_size: false,
            demosaic: Demosaic::Fast,
        }
    }

    /// Display-ready sRGB (quick viewable dump — used by `rustroom decode`).
    pub fn display() -> Self {
        Self {
            camera_wb: true,
            linear: false,
            no_auto_bright: false,
            half_size: false,
            demosaic: Demosaic::Fast,
        }
    }

    /// Fast, scene-referred linear decode for interactive editing (half resolution).
    pub fn preview() -> Self {
        Self {
            camera_wb: true,
            linear: true,
            no_auto_bright: true,
            half_size: true,
            demosaic: Demosaic::Fast,
        }
    }

    /// Size of the decoded image relative to a full-resolution decode of the same file.
    ///
    /// The develop pipeline's spatial stages scale their pixel radii by how far the
    /// render is from full resolution, and this is the first of the two factors.
    /// `RawMetadata` cannot supply it — it reports the *processed* dimensions, which are
    /// already halved here — but `half_size` is our own flag, so the ratio is exact.
    pub fn source_scale(&self) -> f32 {
        if self.half_size {
            0.5
        } else {
            1.0
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
/// With [`DecodeOptions::linear_scene`] the samples are linear light in **ProPhoto (ROMM)
/// primaries** — the pipeline's working space.
///
/// ⚠ [`DecodeOptions::display`] still asks libraw for sRGB, because it feeds `rustroom decode`'s
/// viewable dump rather than the pipeline. The two paths differ deliberately.
/// With [`DecodeOptions::display`] they are sRGB-encoded (display-referred).
pub fn decode_raw(path: &Path, opts: &DecodeOptions) -> Result<DecodedRaw, DecodeError> {
    let bytes = std::fs::read(path)?;
    backend_rsraw::decode(&bytes, opts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_scale_reflects_half_size_decoding() {
        // The pipeline's spatial stages need to know how far a decode is from full res.
        // Metadata cannot answer that (it reports the *processed* size), but the
        // half_size flag can, exactly.
        assert_eq!(DecodeOptions::preview().source_scale(), 0.5);
        assert_eq!(DecodeOptions::linear_scene().source_scale(), 1.0);
        assert_eq!(DecodeOptions::display().source_scale(), 1.0);
    }
    use std::path::PathBuf;

    /// Set `RAW_FIXTURE=/abs/path/to.raw` to exercise a real decode locally / in CI.
    fn fixture() -> Option<PathBuf> {
        std::env::var("RAW_FIXTURE")
            .ok()
            .map(PathBuf::from)
            .filter(|p| p.exists())
    }

    #[test]
    fn demosaic_maps_to_libraw_user_qual() {
        // ⚠ Hard numbers, because this mapping is the entire contract with libraw and a
        // silent off-by-one would change every export's quality with nothing failing.
        assert_eq!(Demosaic::Fastest.user_qual(), 0);
        assert_eq!(Demosaic::Fast.user_qual(), 1);
        assert_eq!(Demosaic::Better.user_qual(), 2);
        assert_eq!(Demosaic::Best.user_qual(), 3);
    }

    #[test]
    fn the_default_is_fast_not_libraws_own_default() {
        // libraw's own default is 3 (AHD), which is what we inherited by never setting it —
        // ~10.6 s of a ~11.4 s decode. `Fast` is 2.0 s and visibly better than bilinear.
        assert_eq!(Demosaic::default(), Demosaic::Fast);
        assert_eq!(DecodeOptions::linear_scene().demosaic, Demosaic::Fast);
        assert_eq!(DecodeOptions::preview().demosaic, Demosaic::Fast);
    }

    #[test]
    fn demosaic_round_trips_json() {
        for d in Demosaic::ALL {
            let back: Demosaic = serde_json::from_str(&serde_json::to_string(&d).unwrap()).unwrap();
            assert_eq!(d, back);
        }
    }

    #[test]
    fn demosaic_names_are_stable_in_json() {
        // The name is part of the on-disk format for export presets and settings. Renaming a
        // variant would silently reset a saved preset to the default.
        assert_eq!(serde_json::to_string(&Demosaic::Fast).unwrap(), "\"Fast\"");
        assert_eq!(serde_json::to_string(&Demosaic::Best).unwrap(), "\"Best\"");
    }

    #[test]
    fn a_half_size_decode_ignores_the_demosaic_setting() {
        // ⚠ The claim the whole UI rests on: this is an *export-time* setting because a
        // half-size decode skips demosaic entirely (2x2 binning), so the editing preview
        // cannot show it. Asserted rather than commented.
        let Some(path) = fixture() else {
            eprintln!("skipping: set RAW_FIXTURE=/abs/path/to.raw to run this test");
            return;
        };
        let half = |d: Demosaic| {
            decode_raw(
                &path,
                &DecodeOptions {
                    demosaic: d,
                    ..DecodeOptions::preview()
                },
            )
            .expect("decode")
            .image
        };
        assert_eq!(
            half(Demosaic::Fastest).data(),
            half(Demosaic::Best).data(),
            "a half-size decode must be unaffected by the demosaic setting"
        );
    }

    #[test]
    fn a_full_size_decode_honours_the_demosaic_setting() {
        // The other half: if this failed, `user_qual` was never reaching libraw and the whole
        // feature would be an inert dropdown.
        let Some(path) = fixture() else {
            eprintln!("skipping: set RAW_FIXTURE=/abs/path/to.raw to run this test");
            return;
        };
        let full = |d: Demosaic| {
            decode_raw(
                &path,
                &DecodeOptions {
                    demosaic: d,
                    ..DecodeOptions::linear_scene()
                },
            )
            .expect("decode")
            .image
        };
        assert_ne!(
            full(Demosaic::Fastest).data(),
            full(Demosaic::Best).data(),
            "the demosaic setting never reached libraw"
        );
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
        assert!(
            t.width <= 512 && t.height <= 512,
            "thumbnail should be downscaled"
        );
        assert_eq!(t.rgba.len(), t.width * t.height * 4);
        assert!(t.rgba.iter().any(|&b| b > 0));
    }
}
