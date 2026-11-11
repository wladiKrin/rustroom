//! HEIC / HEIF photos (M10.19a) — an iPhone's, Immich's copies of them, a camera's `.HIF` — through libheif.
//!
//! ⚠⚠ **Orientation is the CONTAINER's** (`irot` / `imir`), applied by libheif while decoding; the EXIF
//! orientation tag is ignored, as the HEIF spec says — applying both would turn a portrait twice.
//!
//! ⚠⚠ **Colour is read, never assumed** — unlike a JPEG, which is still sRGB by assumption. An iPhone's
//! photo is Display P3: read as sRGB it loses saturation. An embedded ICC profile goes through LittleCMS;
//! without one the nclx primaries say sRGB/709 or Display P3, both with the sRGB curve; anything else is
//! read as sRGB. ⚠ HDR (PQ/HLG transfer, Apple's gain maps) is not handled: the SDR picture is decoded.

use libheif_rs::{ColorPrimaries, ColorSpace, HeifContext, LibHeif, RgbChroma};
use std::io::Read;
use std::path::Path;
use std::sync::OnceLock;

/// The `ftyp` brands of a HEIF still image (HEVC-coded, or the generic image brands).
const BRANDS: [&[u8; 4]; 9] = [
    b"heic", b"heix", b"heim", b"heis", b"hevc", b"hevx", b"mif1", b"msf1", b"heif",
];

/// Whether `path` is a HEIF file, by its `ftyp` box — never by its extension.
pub fn is_heif(path: &Path) -> bool {
    let mut head = [0u8; 12];
    std::fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut head))
        .is_ok()
        && &head[4..8] == b"ftyp"
        && BRANDS.iter().any(|b| &head[8..12] == *b)
}

/// How a decoded picture's samples are to be read.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Colour {
    Srgb,
    DisplayP3,
    /// An embedded ICC profile.
    Icc(Vec<u8>),
}

/// A decoded, oriented picture: encoded samples in [0, 1], interleaved RGB, and how to read them.
pub(crate) struct Pixels {
    pub width: usize,
    pub height: usize,
    pub samples: Vec<f32>,
    pub colour: Colour,
}

fn err(e: impl std::fmt::Display) -> crate::ImageDecodeError {
    crate::ImageDecodeError::Heif(e.to_string())
}

/// Decode `path`'s primary picture, oriented, at 8 bits or — for a deeper file — 16.
pub(crate) fn decode(path: &Path) -> Result<Pixels, crate::ImageDecodeError> {
    let name = path.to_str().ok_or_else(|| err("the path is not UTF-8"))?;
    let ctx = HeifContext::read_from_file(name).map_err(err)?;
    let handle = ctx.primary_image_handle().map_err(err)?;
    let deep = handle.luma_bits_per_pixel() > 8;
    let chroma = if deep {
        RgbChroma::HdrRgbLe
    } else {
        RgbChroma::Rgb
    };
    let image = LibHeif::new()
        .decode(&handle, ColorSpace::Rgb(chroma), None)
        .map_err(err)?;
    let planes = image.planes();
    let plane = planes
        .interleaved
        .ok_or_else(|| err("no interleaved RGB plane"))?;
    let (w, h) = (plane.width as usize, plane.height as usize);
    let mut samples = Vec::with_capacity(w * h * 3);
    for row in 0..h {
        let line = &plane.data[row * plane.stride..];
        if deep {
            let max = ((1u32 << plane.bits_per_pixel) - 1) as f32;
            for px in line[..w * 6].as_chunks::<2>().0 {
                samples.push(f32::from(u16::from_le_bytes(*px)) / max);
            }
        } else {
            samples.extend(line[..w * 3].iter().map(|&v| f32::from(v) / 255.0));
        }
    }
    // ⚠ The HANDLE's profile: the decoded image does not carry the file's nclx.
    let colour = match (handle.color_profile_raw(), handle.color_profile_nclx()) {
        (Some(icc), _) if !icc.data.is_empty() => Colour::Icc(icc.data),
        (_, Some(n)) if n.color_primaries() == ColorPrimaries::SMPTE_EG_432_1 => Colour::DisplayP3,
        _ => Colour::Srgb,
    };
    Ok(Pixels {
        width: w,
        height: h,
        samples,
        colour,
    })
}

/// The picture in the pipeline's linear ProPhoto, interleaved RGB.
pub(crate) fn to_linear_prophoto(p: &Pixels) -> Vec<f32> {
    if let Colour::Icc(icc) = &p.colour {
        if let Some(out) = through_icc(icc, &p.samples) {
            return out;
        }
    }
    let m = match p.colour {
        Colour::DisplayP3 => *p3_to_prophoto(),
        _ => color::spaces::SRGB_TO_PROPHOTO,
    };
    p.samples
        .as_chunks::<3>()
        .0
        .iter()
        .flat_map(|c| {
            color::spaces::apply(
                &m,
                [
                    color::srgb_to_linear(c[0]),
                    color::srgb_to_linear(c[1]),
                    color::srgb_to_linear(c[2]),
                ],
            )
        })
        .collect()
}

/// `samples` through the embedded profile into linear ProPhoto — `None` if LittleCMS cannot read it, and the
/// caller reads the picture as sRGB.
fn through_icc(icc: &[u8], samples: &[f32]) -> Option<Vec<f32>> {
    let src = lcms2::Profile::new_icc(icc).ok()?;
    let dst = linear_prophoto()?;
    let t = lcms2::Transform::<[f32; 3], [f32; 3]>::new(
        &src,
        lcms2::PixelFormat::RGB_FLT,
        &dst,
        lcms2::PixelFormat::RGB_FLT,
        lcms2::Intent::RelativeColorimetric,
    )
    .ok()?;
    let input = samples.as_chunks::<3>().0;
    let mut out = vec![[0.0f32; 3]; input.len()];
    t.transform_pixels(input, &mut out);
    Some(out.into_iter().flatten().collect())
}

/// Linear ProPhoto (ROMM, D50) — the working space, as LittleCMS describes it.
fn linear_prophoto() -> Option<lcms2::Profile> {
    let xy = |x: f64, y: f64| lcms2::CIExyY { x, y, Y: 1.0 };
    let prim = lcms2::CIExyYTRIPLE {
        Red: xy(0.734699, 0.265301),
        Green: xy(0.159597, 0.840403),
        Blue: xy(0.036598, 0.000105),
    };
    let linear = lcms2::ToneCurve::new(1.0);
    lcms2::Profile::new_rgb(&xy(0.3457, 0.3585), &prim, &[&linear, &linear, &linear]).ok()
}

/// Linear Display P3 → linear ProPhoto: the inverse of the export's `PROPHOTO_TO_DISPLAY_P3`, so a P3 file
/// read in and written back out round-trips.
fn p3_to_prophoto() -> &'static [[f32; 3]; 3] {
    static M: OnceLock<[[f32; 3]; 3]> = OnceLock::new();
    M.get_or_init(|| invert(&color::spaces::PROPHOTO_TO_DISPLAY_P3))
}

fn invert(m: &[[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let m: [[f64; 3]; 3] = m.map(|r| r.map(f64::from));
    let cof =
        |r0: usize, r1: usize, c0: usize, c1: usize| m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0];
    let adj = [
        [cof(1, 2, 1, 2), -cof(0, 2, 1, 2), cof(0, 1, 1, 2)],
        [-cof(1, 2, 0, 2), cof(0, 2, 0, 2), -cof(0, 1, 0, 2)],
        [cof(1, 2, 0, 1), -cof(0, 2, 0, 1), cof(0, 1, 0, 1)],
    ];
    let det = m[0][0] * adj[0][0] + m[0][1] * adj[1][0] + m[0][2] * adj[2][0];
    adj.map(|r| r.map(|v| (v / det) as f32))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Constructed fixtures — see `testdata/README.md` for how each was made.
    pub(crate) fn fixture(name: &str) -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("testdata")
            .join(name)
    }

    #[test]
    fn a_heif_is_known_by_its_box_not_its_name() {
        assert!(is_heif(&fixture("srgb.heic")));
        // The positive control above; a renamed PNG and the script beside the fixtures are not HEIF.
        let dir = test_dir::TempDir::new("heif-sniff");
        let png = dir.path().join("fake.heic");
        image::RgbImage::new(2, 2)
            .save_with_format(&png, image::ImageFormat::Png)
            .unwrap();
        assert!(!is_heif(&png));
        assert!(!is_heif(&fixture("mkexif.py")));
    }

    #[test]
    fn the_p3_inverse_undoes_the_exports_matrix() {
        let back = color::spaces::apply(&color::spaces::PROPHOTO_TO_DISPLAY_P3, [0.2, 0.5, 0.1]);
        let there = color::spaces::apply(p3_to_prophoto(), back);
        for (a, b) in there.iter().zip([0.2, 0.5, 0.1]) {
            assert!((a - b).abs() < 1e-5, "{there:?}");
        }
    }

    #[test]
    fn an_srgb_file_decodes_its_left_half_red() {
        let p = decode(&fixture("srgb.heic")).unwrap();
        assert_eq!(
            (p.width, p.height, p.colour.clone()),
            (64, 48, Colour::Srgb)
        );
        let px = |x: usize, y: usize| &p.samples[(y * p.width + x) * 3..][..3];
        // Lossy: within a few levels of (255, 0, 0) and (128, 128, 128), away from the edge.
        assert!(px(8, 24)[0] > 0.9 && px(8, 24)[1] < 0.1, "{:?}", px(8, 24));
        assert!((px(56, 24)[1] - 0.5).abs() < 0.04, "{:?}", px(56, 24));
    }

    #[test]
    fn a_display_p3_file_is_read_as_p3_not_srgb() {
        let p = decode(&fixture("p3.heic")).unwrap();
        assert_eq!(p.colour, Colour::DisplayP3);
        let as_p3 = &to_linear_prophoto(&p)[..3];
        let as_srgb = &to_linear_prophoto(&Pixels {
            colour: Colour::Srgb,
            ..decode(&fixture("p3.heic")).unwrap()
        })[..3];
        // ⚠ The two readings must differ first, or the next check proves nothing.
        // Measured: 0.067 in red and 0.039 in blue — in ProPhoto a greener green moves the other two channels.
        let apart = as_p3
            .iter()
            .zip(as_srgb)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f32::max);
        assert!(apart > 0.03, "{as_p3:?} {as_srgb:?}");
        // Lossless (0, 200, 0) in P3, by the matrices.
        let g = color::srgb_to_linear(200.0 / 255.0);
        let want = color::spaces::apply(p3_to_prophoto(), [0.0, g, 0.0]);
        for (a, b) in as_p3.iter().zip(want) {
            assert!((a - b).abs() < 2e-3, "{as_p3:?} vs {want:?}");
        }
    }

    #[test]
    fn an_embedded_icc_profile_reads_as_its_primaries() {
        // ⚠ The profile is the export's own Display P3: through LittleCMS it must land where the nclx path does.
        let icc = decode(&fixture("icc.heic")).unwrap();
        assert!(matches!(icc.colour, Colour::Icc(_)));
        let nclx = decode(&fixture("p3.heic")).unwrap();
        let (a, b) = (
            &to_linear_prophoto(&icc)[..3],
            &to_linear_prophoto(&nclx)[..3],
        );
        for (x, y) in a.iter().zip(b) {
            assert!((x - y).abs() < 0.01, "{a:?} vs {b:?}");
        }
    }

    #[test]
    fn a_ten_bit_file_keeps_its_precision() {
        let p = decode(&fixture("ten.heic")).unwrap();
        // 50 % grey at 10 bits is 512/1023 (or 511); 8 bits could only say 128/255 = 0.50196.
        let v = p.samples[0];
        assert!(
            (v - 0.5).abs() < 0.002 && (v - 128.0 / 255.0).abs() > 1e-4,
            "{v}"
        );
    }

    #[test]
    fn the_containers_rotation_is_applied() {
        // Stored 60 × 40 with a red stripe down the left, rotated a quarter clockwise by the container.
        let p = decode(&fixture("rot.heic")).unwrap();
        assert_eq!((p.width, p.height), (40, 60));
        let px = |x: usize, y: usize| &p.samples[(y * p.width + x) * 3..][..3];
        assert!(
            px(20, 2)[0] > 0.8 && px(20, 2)[1] < 0.2,
            "the stripe is along the top: {:?}",
            px(20, 2)
        );
        assert!(px(20, 50)[0] < 0.6, "{:?}", px(20, 50));
    }
}
