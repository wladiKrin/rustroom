//! The crate's entry points on HEIF files (M10.19a) — the constructed fixtures in `testdata/`.

use crate::heif::tests::fixture;

#[test]
fn a_heif_decodes_into_the_working_space() {
    let d = crate::decode(&fixture("srgb.heic")).unwrap();
    assert_eq!((d.width, d.height), (64, 48));
    // The red half, linear ProPhoto: sRGB red is (0.53, 0.10, 0.02) or so there — red leads, by far.
    let [r, g, b] = d.image.pixel(8, 24);
    assert!(r > 0.4 && g < 0.2 && b < 0.1, "{r} {g} {b}");
}

#[test]
fn a_heifs_thumbnail_and_preview_are_srgb_and_upright() {
    let t = crate::thumbnail(&fixture("rot.heic")).unwrap();
    // Upright: portrait, 40 × 60 scaled to 512 high like every thumbnail.
    assert_eq!((t.width, t.height), (341, 512));
    let p = crate::preview(&fixture("srgb.heic")).unwrap();
    let at = |x: usize, y: usize| &p.rgba[(y * p.width + x) * 4..][..4];
    // Back to sRGB: the red half within a few levels of (255, 0, 0), the grey near 128.
    assert!(at(8, 24)[0] > 240 && at(8, 24)[1] < 20, "{:?}", at(8, 24));
    assert!(
        (i32::from(at(56, 24)[1]) - 128).abs() < 8,
        "{:?}",
        at(56, 24)
    );
}

#[test]
fn a_heifs_exif_reaches_the_capture() {
    let c = crate::read_capture(&fixture("srgb.heic")).unwrap();
    assert_eq!(c.camera.as_deref(), Some("APPLE iPhone 15 Pro"), "{c:?}");
    assert_eq!(c.aperture, Some(1.8));
    let want = chrono::NaiveDate::from_ymd_opt(2026, 7, 14)
        .and_then(|d| d.and_hms_opt(18, 30, 5))
        .map(|t| t.and_utc().timestamp());
    assert_eq!(c.taken, want);
}
