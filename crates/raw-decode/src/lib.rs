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
mod float_dng;

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("could not read RAW file: {0}")]
    Io(#[from] std::io::Error),
    #[error("libraw failed to decode: {0}")]
    Backend(String),
    #[error("unexpected decoded format: {0}")]
    Format(String),
}

/// What the camera recorded about the lens, when it recorded anything.
///
/// ⚠ Deliberately does **not** carry `rsraw::LensInfo`'s `min_focal`/`max_focal` or `mounts`.
/// Probing showed the focal range reporting `16-16` for a 16-80 zoom and the mount returning the
/// garbage string `"IL UM"`. The per-shot `focal` below comes from `raw.focal_len()`, which is
/// correct, and the mount is not needed because matching goes through the camera.
#[derive(Debug, Clone, PartialEq)]
pub struct LensMetadata {
    /// e.g. `"FUJIFILM"`.
    pub make: String,
    /// e.g. `"XF16-80mmF4 R OIS WR"` — matches LensFun's `<model>` verbatim for the lenses checked.
    pub name: String,
    /// Focal length of *this shot*, in mm.
    pub focal: f32,
    /// f-number of this shot.
    pub aperture: f32,
}

/// Camera/sensor metadata recovered from the RAW.
#[derive(Debug, Clone)]
pub struct RawMetadata {
    pub width: usize,
    pub height: usize,
    pub make: String,
    pub model: String,
    /// `None` when the file records no usable lens name.
    pub lens: Option<LensMetadata>,
    /// The illuminant the decode balanced the picture to — the camera's own as-shot white.
    ///
    /// ⚠⚠ **This is the zero point of the Kelvin slider, and it is a fact about the file rather
    /// than a choice.** libraw applies `cam_mul` during `dcraw_process`, so a neutral object comes
    /// out as equal RGB in the working space; the temperature that neutrality corresponds to is
    /// this. `None` on any path that cannot recover it, and the caller then falls back to D65 —
    /// which is what a JPEG's baseline genuinely is, so the fallback is a right answer and not a
    /// degraded one.
    pub as_shot: Option<color::wb::Illuminant>,
}

/// A fully decoded RAW: demosaiced RGB plus metadata.
#[derive(Debug, Clone)]
pub struct DecodedRaw {
    pub image: ImageBuf,
    pub metadata: RawMetadata,
}

/// libraw's demosaiced output in **camera space**, white-balanced, before any colour conversion —
/// Develop's decode stopped before our camera matrix (M10.17a, the HDR merge's frames).
#[derive(Debug, Clone)]
pub struct CameraFrame {
    pub width: usize,
    pub height: usize,
    /// Interleaved RGB, exactly as libraw produced it: `(raw − black) / (white − black) × pre_mul`,
    /// clipped at 65535 — a saturated channel reads 65535 (libraw's highlight mode "clip").
    pub samples: Vec<u16>,
    /// The multipliers libraw applied, scaled so the smallest is 1. Dividing a sample by its
    /// channel's undoes the white balance exactly.
    pub pre_mul: [f32; 3],
    /// XYZ → camera, as libraw holds it (its `cam_xyz`; a DNG's `ColorMatrix`, D65).
    pub cam_xyz: [[f32; 3]; 3],
    /// Camera → linear sRGB, libraw's `rgb_cam` — what `decode_raw` builds its matrix from.
    pub rgb_cam: [[f32; 3]; 3],
    pub metadata: RawMetadata,
    pub capture: core_image::Capture,
}

pub use core_image::Thumbnail;

/// Extract and decode the camera's embedded preview thumbnail (fast — no demosaic).
/// Read a RAW's capture metadata without decoding it.
///
/// ⚠ Reads the whole file: `RawImage::open` takes a `&[u8]`, and a truncated prefix was measured
/// and rejected — libraw opens nothing at 1 MB and only half the files at 4 MB, so a prefix read
/// would silently miss part of a library.
pub fn read_capture(path: &Path) -> Result<core_image::Capture, DecodeError> {
    let bytes = std::fs::read(path)?;
    backend_rsraw::read_capture(&bytes)
}

/// The photo's displayed pixel dimensions, without decoding it.
///
/// ⚠ Reads the whole file — see `backend_rsraw::dimensions`.
pub fn dimensions(path: &Path) -> Option<(u32, u32)> {
    let bytes = std::fs::read(path).ok()?;
    backend_rsraw::dimensions(&bytes)
}

pub fn extract_thumbnail(path: &Path) -> Result<Thumbnail, DecodeError> {
    let bytes = std::fs::read(path)?;
    backend_rsraw::extract_thumbnail(&bytes)
}

/// The largest JPEG the camera embedded, at its own size and oriented like the thumbnail —
/// M9.14's camera preview. 4416×2944 on an X-T4, 71% of the RAW's 6240 px on each axis.
pub fn extract_preview(path: &Path) -> Result<Thumbnail, DecodeError> {
    let bytes = std::fs::read(path)?;
    backend_rsraw::extract_preview(&bytes)
}

/// The capture facts and the embedded thumbnail from **one** read of the file (M9.12's import
/// survey). ⚠ Reading dominates both — ~11–15 ms for a 29 MB RAF from an SSD, far more from a
/// card — so calling [`read_capture`] then [`extract_thumbnail`] would read every RAW twice.
/// Each half fails on its own: a file with no thumbnail still has a capture time.
pub fn survey(path: &Path) -> std::io::Result<(Option<core_image::Capture>, Option<Thumbnail>)> {
    let bytes = std::fs::read(path)?;
    Ok((
        backend_rsraw::read_capture(&bytes).ok(),
        backend_rsraw::extract_thumbnail(&bytes).ok(),
    ))
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
    /// libraw `user_qual` 1 — VNG on Bayer. 3.9x faster end-to-end than `Best`.
    ///
    /// ⚠⚠ **Was the default until M8.4, and it was chosen on the wrong content** — hair-strand
    /// separation on a kitten frame, and X-Trans demosaic does not fail on hair. Measured on
    /// foliage it loses real structure: a thin twig that `Better` resolves as a line comes back
    /// as a smeared blob, and a distant treeline goes to mush.
    Fast,
    /// libraw `user_qual` 2 — PPG on Bayer. **The default since M8.4.**
    ///
    /// ⚠ Chosen on a measurement, not a preference. Against `Best` on four foliage frames it
    /// differs by 0.22–0.30 of an 8-bit level on the detail under test and **0.08–0.32 on a flat
    /// control region beside it** — a difference that lives only where detail lives, which is the
    /// signature of a real demosaic difference. `Fast` differs by 1.1–2.5 levels on the *flat*
    /// regions too, i.e. it renders the whole image differently rather than merely losing detail.
    /// At 4× magnification `Better` and `Best` are indistinguishable, for 1.45× less time.
    /// See `docs/superpowers/specs/2026-09-10-demosaic-decision.md`.
    #[default]
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
/// ⚠ `Clone` but **not `Copy`** — `camera_profile` is a `PathBuf`. Options are constructed once
/// per decode and moved, so the loss costs nothing; the same trade `Mask` made for the brush.
#[derive(Debug, Clone)]
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
    ///
    /// ⚠ Every constructor below seeds it from [`Demosaic::default()`] rather than naming a
    /// tier. They used to hardcode `Fast`, which put the default in four places at once — so
    /// when M8.4 moved it, three of them would have kept saying the old thing while still
    /// reading as "the default". The class of disagreement `render_max_dim` and
    /// `Session::geometry_mode` were each fixed for by deriving rather than storing twice.
    pub demosaic: Demosaic,
    /// Apply the camera → working-space matrix ourselves rather than letting libraw do it.
    ///
    /// ⚠ `false` selects libraw's own conversion, and it stays reachable **permanently** as an
    /// oracle: C2 and C3 each change this matrix, and a reference that does not move when the
    /// thing under test does is worth far more than the few lines it costs. It is the same role
    /// the golden harness plays for `pipeline`.
    pub camera_native: bool,
    /// A `.dcp` camera profile to use instead of libraw's matrix.
    ///
    /// ⚠ `None` — the default — keeps C1's libraw path exactly. That is what makes this opt-in
    /// change unable to regress anything, and it keeps the C1 oracle live for comparison.
    /// Discovery and the UI are C4.
    pub camera_profile: Option<std::path::PathBuf>,
}

impl DecodeOptions {
    /// Scene-referred linear working image (what the develop pipeline consumes).
    pub fn linear_scene() -> Self {
        Self {
            camera_wb: true,
            linear: true,
            no_auto_bright: true,
            half_size: false,
            demosaic: Demosaic::default(),
            camera_native: true,
            camera_profile: None,
        }
    }

    /// Display-ready sRGB (quick viewable dump — used by `rustroom decode`).
    pub fn display() -> Self {
        Self {
            camera_wb: true,
            linear: false,
            no_auto_bright: false,
            half_size: false,
            demosaic: Demosaic::default(),
            // ⚠ Deliberately libraw's own conversion. This is a debug dump, not an export path,
            // and it wants a baked sRGB image rather than the pipeline's working space.
            camera_native: false,
            camera_profile: None,
        }
    }

    /// Fast, scene-referred linear decode for interactive editing (half resolution).
    pub fn preview() -> Self {
        Self {
            camera_wb: true,
            linear: true,
            no_auto_bright: true,
            half_size: true,
            demosaic: Demosaic::default(),
            camera_native: true,
            camera_profile: None,
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
///
/// ⚠ M10.17a: a floating-point linear DNG — the HDR merge's result — is read by
/// [`float_dng`](crate::float_dng), never libraw, which clips it (see that module).
pub fn decode_raw(path: &Path, opts: &DecodeOptions) -> Result<DecodedRaw, DecodeError> {
    let bytes = std::fs::read(path)?;
    if float_dng::is_float_linear(&bytes) {
        return float_dng::decode(&bytes, opts);
    }
    backend_rsraw::decode(&bytes, opts)
}

/// Whether `path` is a floating-point linear DNG — an HDR merge's result — which Rustroom reads
/// itself. Reads the file's header and IFDs (a whole-file read today, as every RAW path here).
pub fn is_float_dng(path: &Path) -> bool {
    std::fs::read(path).is_ok_and(|b| float_dng::is_float_linear(&b))
}

/// Decode a RAW to libraw's camera-space output — see [`CameraFrame`]. `camera_native` and
/// `camera_profile` are ignored (no colour conversion happens); everything else is as
/// [`decode_raw`].
pub fn decode_camera(path: &Path, opts: &DecodeOptions) -> Result<CameraFrame, DecodeError> {
    let bytes = std::fs::read(path)?;
    backend_rsraw::decode_camera(&bytes, opts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_preview_is_larger_than_the_thumbnail_and_oriented_the_same_way() {
        // ⚠ Gated on the fixture, so it skips without it — a green run does not mean this ran.
        let Ok(path) = std::env::var("RAW_FIXTURE") else {
            return;
        };
        let p = std::path::Path::new(&path);
        let thumb = extract_thumbnail(p).expect("thumbnail");
        let preview = extract_preview(p).expect("preview");
        assert!(
            preview.width > thumb.width && preview.height > thumb.height,
            "the preview is not larger than the thumbnail: {}x{} vs {}x{}",
            preview.width,
            preview.height,
            thumb.width,
            thumb.height
        );
        assert_eq!(
            preview.width > preview.height,
            thumb.width > thumb.height,
            "the two disagree about the orientation"
        );
        assert_eq!(preview.rgba.len(), preview.width * preview.height * 4);
    }

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
    fn the_default_is_better_and_every_constructor_takes_it() {
        // ⚠ Two assertions doing two different jobs, and the pair is what keeps either of them
        // meaningful. The first pins the *value*, which is a decision (M8.4: `Fast` loses real
        // foliage structure, `Better` is indistinguishable from `Best` at 4×); the rest pin the
        // *wiring*, so a constructor cannot go back to naming a tier and drift from it. Written
        // against `Demosaic::default()` rather than a literal on purpose — a second literal is
        // exactly what this change removed.
        assert_eq!(Demosaic::default(), Demosaic::Better);
        assert_eq!(DecodeOptions::linear_scene().demosaic, Demosaic::default());
        assert_eq!(DecodeOptions::preview().demosaic, Demosaic::default());
        assert_eq!(DecodeOptions::display().demosaic, Demosaic::default());
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

    /// ⚠ The one test that can see the as-shot solve against a *real* camera rather than a
    /// synthetic matrix. It is deliberately loose — a plausible daylight range — because the
    /// point is that the number is a temperature at all, not that it is one particular value:
    /// the DNG solve behind it carries McCamy's ~2% approximation and the file's own
    /// multipliers, neither of which this test should be pinning.
    ///
    /// ⚠ Requires `RAW_FIXTURE`; without it this **skips**, so a green CI run is no evidence.
    #[test]
    fn a_raw_reports_the_illuminant_it_was_balanced_to() {
        let Some(path) = fixture() else {
            eprintln!("skipping: set RAW_FIXTURE=/abs/path/to.raw to run this test");
            return;
        };
        let d = decode_raw(&path, &DecodeOptions::preview()).expect("decodes");
        let i = d
            .metadata
            .as_shot
            .expect("a RAW knows what it was balanced to");
        assert!(
            (1500.0..=15000.0).contains(&i.kelvin),
            "{}K is not a temperature any camera balanced to",
            i.kelvin
        );
        // ⚠⚠ **The assertion that catches the wrong matrix.** A camera's as-shot neutral is a
        // real light, and real lights — daylight, tungsten, even fluorescent — sit close to the
        // locus. Solving from `rgb_cam` instead of `cam_xyz` lands **0.084 off it, a tint of
        // 140 on a ±100 control**, and still produces a perfectly plausible-looking Kelvin. Only
        // the distance sees it.
        assert!(
            i.tint.abs() < 60.0,
            "{:.0}K tint {:.1} is too far off the locus to be a real illuminant — \
             this is what solving from the wrong matrix looks like",
            i.kelvin,
            i.tint
        );
        eprintln!("as shot: {:.0}K, tint {:.1}", i.kelvin, i.tint);
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

    /// M10.17a: the camera-space decode is Develop's decode stopped before our matrix — applying
    /// the matrix to it must give `decode_raw`'s pixels **exactly** (the same f32 operations).
    /// ⚠ Gated on `RAW_FIXTURE`; it skips without it.
    #[test]
    fn decode_camera_then_our_matrix_is_decode_raw() {
        let Some(path) = fixture() else {
            eprintln!("skipping: set RAW_FIXTURE=/abs/path/to.raw to run this test");
            return;
        };
        let cam = decode_camera(&path, &DecodeOptions::preview()).expect("camera decode");
        let raw = decode_raw(&path, &DecodeOptions::preview()).expect("decode");
        assert_eq!(
            (cam.width, cam.height),
            (raw.image.width(), raw.image.height())
        );
        let m = color::camera::camera_to_prophoto(cam.rgb_cam);
        let mut worst = 0f32;
        for (px, want) in cam.samples.chunks(3).zip(raw.image.data().chunks(3)) {
            let got = color::spaces::apply(&m, [px[0], px[1], px[2]].map(|s| s as f32 / 65535.0));
            for c in 0..3 {
                worst = worst.max((got[c] - want[c]).abs());
            }
        }
        eprintln!("decode_camera vs decode_raw: worst {worst}");
        assert_eq!(worst, 0.0, "the camera decode is not Develop's decode");
        assert_eq!(cam.metadata.as_shot, raw.metadata.as_shot);
    }

    /// The multipliers libraw applied, as it applied them: scaled so the smallest is 1 (probed on
    /// DSCF7618, 2026-10-26: 1.937, 1, 1.636). ⚠ Gated on `RAW_FIXTURE`.
    #[test]
    fn pre_mul_is_normalised_to_a_minimum_of_one() {
        let Some(path) = fixture() else {
            eprintln!("skipping: set RAW_FIXTURE=/abs/path/to.raw to run this test");
            return;
        };
        let cam = decode_camera(&path, &DecodeOptions::preview()).expect("camera decode");
        let min = cam.pre_mul.iter().cloned().fold(f32::INFINITY, f32::min);
        eprintln!("pre_mul {:?}", cam.pre_mul);
        assert!((min - 1.0).abs() < 1e-6, "{:?}", cam.pre_mul);
        assert!(cam.pre_mul.iter().all(|m| m.is_finite() && *m >= 1.0));
    }

    /// dcraw's `cam_xyz_coeff`, ours, gives the `rgb_cam` libraw computed for the camera — the
    /// property the float DNG reader (M10.17a) depends on. ⚠ Gated on `RAW_FIXTURE`.
    #[test]
    fn our_rgb_cam_from_cam_xyz_matches_libraws() {
        let Some(path) = fixture() else {
            eprintln!("skipping: set RAW_FIXTURE=/abs/path/to.raw to run this test");
            return;
        };
        let cam = decode_camera(&path, &DecodeOptions::preview()).expect("camera decode");
        let ours = color::camera::rgb_cam_from_cam_xyz(&cam.cam_xyz).expect("invertible");
        let worst = ours
            .iter()
            .flatten()
            .zip(cam.rgb_cam.iter().flatten())
            .map(|(a, b)| (a - b).abs())
            .fold(0f32, f32::max);
        eprintln!("rgb_cam ours vs libraw: worst {worst}");
        assert!(worst < 1e-4, "ours {ours:?} libraw {:?}", cam.rgb_cam);
    }

    /// M10.17a: libraw leaves `cam_xyz` empty for a DNG; the camera decode derives one that gives
    /// libraw's own `rgb_cam` back — a merge of DNGs must write a usable colour matrix.
    #[test]
    fn a_dngs_camera_decode_carries_a_usable_cam_xyz() {
        let path = test_dir::path("camera-decode-dng").join("rendered.dng");
        let mut img = core_image::ImageBuf::new(40, 30);
        img.set_pixel(0, 0, [1.0, 1.0, 1.0]);
        img.set_pixel(1, 0, [0.2, 0.4, 0.6]);
        export::dng::save(&img, &path).unwrap();
        let c = decode_camera(&path, &DecodeOptions::linear_scene()).expect("decodes");
        let back = color::camera::rgb_cam_from_cam_xyz(&c.cam_xyz).expect("a usable cam_xyz");
        for (a, b) in back.iter().flatten().zip(c.rgb_cam.iter().flatten()) {
            assert!((a - b).abs() < 1e-4, "{back:?} vs {:?}", c.rgb_cam);
        }
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

    #[test]
    fn our_camera_matrix_reproduces_libraws() {
        // ⚠⚠ **The acceptance test for this whole slice.** C1 moves a well-tested transform out
        // of libraw and into our code for zero user-visible gain, so the only claim it makes is
        // "the render is unchanged" — and this is the only test that can check it.
        //
        // ⚠ Gated on `RAW_FIXTURE`, so it cannot run in CI without a file. Stated as a
        // limitation rather than hidden: run it locally before trusting this slice.
        let Some(path) = fixture() else {
            eprintln!("skipping: set RAW_FIXTURE=/abs/path/to.raw to run this test");
            return;
        };
        let ours = decode_raw(
            &path,
            &DecodeOptions {
                camera_native: true,
                half_size: true,
                ..DecodeOptions::linear_scene()
            },
        )
        .expect("ours");
        let libraw = decode_raw(
            &path,
            &DecodeOptions {
                camera_native: false,
                half_size: true,
                ..DecodeOptions::linear_scene()
            },
        )
        .expect("libraw");

        assert_eq!(ours.image.width(), libraw.image.width());
        assert_eq!(ours.image.height(), libraw.image.height());

        // ⚠⚠ Compared **clamped**, and that is the whole subtlety of this test. libraw works in
        // 16-bit and clamps to [0, 1]; we apply the matrix in f32 and keep what falls outside.
        // Those differences are not errors — a scene-referred ProPhoto pipeline exists precisely
        // to carry highlights above 1.0 and out-of-gamut negatives, and clipping belongs in the
        // output transform and nowhere else. Measured on an X-T4 frame: only 0.0085% of samples
        // differ at all, and the worst is ours 1.2543 against libraw's clamped 1.0.
        //
        // So there are two assertions. Clamped agreement proves the matrix is right — a wrong one
        // would show as a cast across the whole frame. And every *unclamped* disagreement must be
        // explained by libraw having clipped, which is what stops "we differ everywhere" from
        // hiding behind the clamp.
        let clamp = |v: f32| v.clamp(0.0, 1.0);
        let (mut max, mut sum, mut n) = (0.0f32, 0.0f64, 0usize);
        let mut unexplained = 0usize;
        for y in 0..ours.image.height() {
            for x in 0..ours.image.width() {
                let (a, b) = (ours.image.pixel(x, y), libraw.image.pixel(x, y));
                for i in 0..3 {
                    let d = (clamp(a[i]) - clamp(b[i])).abs();
                    max = max.max(d);
                    sum += d as f64;
                    n += 1;
                    // Raw disagreement not accounted for by libraw sitting on a clamp boundary.
                    if (a[i] - b[i]).abs() > 2.0 / 255.0 && b[i] > 0.0 && b[i] < 1.0 {
                        unexplained += 1;
                    }
                }
            }
        }
        let mean = sum / n as f64;
        eprintln!("camera matrix vs libraw (clamped): max {max:.5}, mean {mean:.6}");
        eprintln!("unexplained disagreements: {unexplained}");
        assert_eq!(
            unexplained, 0,
            "differences that libraw's clamping does not explain — the matrix is wrong"
        );
        assert!(
            max < 2.0 / 255.0,
            "max clamped per-channel difference {max} exceeds the tolerance — the matrix is wrong"
        );
    }

    /// v0.1 adds `.raw`, which other software uses too: a file that is not a camera's is an error, never a panic.
    #[test]
    fn a_file_that_is_not_a_raw_is_an_error_not_a_panic() {
        let d = test_dir::TempDir::new("raw-junk");
        let p = d.path().join("notes.raw");
        std::fs::write(&p, b"not a camera file").unwrap();
        assert!(decode_raw(&p, &DecodeOptions::default()).is_err());
    }

    /// v0.1: a four-colour sensor (a CMYG Canon PowerShot Pro70, an RGBE Sony DSC-F828) has no 3×3 camera
    /// matrix, so the editing decode lets libraw convert it — it used to fail with "expected 3 colors". Set
    /// `RAW_FIXTURE_4COLOR=/abs/path` to one such file to run it (camera files are not committed).
    #[test]
    fn a_four_colour_sensor_decodes_for_editing() {
        let Ok(path) = std::env::var("RAW_FIXTURE_4COLOR") else {
            eprintln!("skipping: set RAW_FIXTURE_4COLOR=/abs/path.crw to run this test");
            return;
        };
        let d = decode_raw(Path::new(&path), &DecodeOptions::preview());
        assert!(d.is_ok(), "{:?}", d.err());
        let d = d.unwrap();
        assert!(d.image.width() > 0 && d.image.height() > 0);
    }
}
