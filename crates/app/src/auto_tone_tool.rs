//! The logic behind the Auto button — everything except its drawing.
//!
//! ⚠ Lives in the lib, with no egui in sight, because that is where a unit-testable rule
//! belongs: `wb_tool`'s pattern. The panel supplies the gesture; this is a pure function of a
//! buffer and an `Edits`.
//!
//! ⚠⚠ **The buffer it reads is `Session.buffer`, never the rendered texture.** That texture
//! already has the current exposure and tone applied, so analysing it would make the auto chase
//! its own correction — press Auto twice and it would keep moving. The same trap the
//! white-balance eyedropper records, and the reason `Session` keeps the `Arc` at all.

use core_image::ImageBuf;
use edit_model::{CropRect, Edits, Geometry, Orientation, Quarter};
use pipeline::auto_tone::{self, AutoTone, Percentiles};

/// The analysis runs on a downscale to this long edge.
///
/// ⚠ Percentiles of a 512 px downscale differ negligibly from the full-resolution ones — a
/// percentile is a property of the *distribution*, and downscaling averages neighbours rather
/// than resampling the histogram's shape — and it keeps the button instant on a 26 Mpx buffer.
pub const ANALYSIS_DIM: usize = 512;

/// The crop rectangle expressed in the **buffer's** frame, given the orientation applied to it.
///
/// ⚠⚠ **`Session.buffer` is pre-geometry, and `CropRect` lives in the oriented frame.** On a
/// portrait shot — a quarter turn — a crop taken on the left of the *oriented* image is the
/// *bottom* of the buffer, so analysing the crop rect as given would measure entirely the wrong
/// pixels. The rest of geometry (straighten, lens, perspective) is deliberately ignored: those
/// move pixels by amounts that do not appreciably change a percentile, where a quarter turn
/// swaps the axes outright.
///
/// [`Orientation`] is *mirror, then turn*, so the inverse is *turn back, then mirror*.
pub fn source_crop(crop: CropRect, o: Orientation) -> CropRect {
    // ⚠ An exhaustive match rather than `Quarter::turns`, which is private to `edit-model`. A
    // fifth variant would be a compile error here, which is the property that matters.
    let turns = match o.rotate {
        Quarter::Zero => 0,
        Quarter::Cw90 => 1,
        Quarter::Cw180 => 2,
        Quarter::Cw270 => 3,
    };
    let mut c = crop;
    for _ in 0..(4 - turns) % 4 {
        c = c.rotated_cw();
    }
    if o.mirror {
        c = c.flipped_h();
    }
    c
}

/// The percentiles and the values they imply, for a buffer and the geometry applied to it.
///
/// Returns the percentiles as well so the probe can print what the decision was made from —
/// the numbers are the evidence the user signs off on, and recomputing them anywhere else
/// would be a second implementation that could disagree.
pub fn analyse(buffer: &ImageBuf, geometry: &Geometry) -> (Option<Percentiles>, AutoTone) {
    let small = buffer.downscale_to(ANALYSIS_DIM);
    let crop = source_crop(geometry.crop, geometry.orientation);
    let p = auto_tone::percentiles(&small, Some(crop));
    (p, p.map(auto_tone::solve).unwrap_or_default())
}

/// Set Exposure, Whites and Blacks from the photograph's own histogram.
///
/// ⚠ **Three fields, and no others.** Contrast, highlights and shadows are a taste the user did
/// not ask for; three values that mean "put the black point, the white point and the middle
/// where they belong" are explainable and undoable in one Ctrl+Z.
pub fn apply(edits: &mut Edits, buffer: &ImageBuf) -> AutoTone {
    let (_, a) = analyse(buffer, &edits.geometry);
    edits.exposure = a.exposure;
    edits.whites = a.whites;
    edits.blacks = a.blacks;
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grey(w: usize, h: usize, v: f32) -> ImageBuf {
        ImageBuf::from_vec(w, h, vec![v; w * h * 3])
    }

    /// A ramp with 0.2 % of its pixels pinned dark and bright, so the solve returns **non-zero**
    /// whites and blacks.
    ///
    /// ⚠⚠ The uniform grey this test used first could not see a sabotage that dropped the whites
    /// solve, because a grey has no tails and the correct answer there *is* 0. A fixture whose
    /// expected value equals the bug's output is no test at all — the standing lesson, met from
    /// the fixture's side rather than the assertion's.
    fn graded(w: usize, h: usize) -> ImageBuf {
        let n = w * h;
        let mut data = Vec::with_capacity(n * 3);
        for i in 0..n {
            let v = 0.05 + 0.25 * (i as f32 / (n - 1) as f32);
            data.extend_from_slice(&[v, v, v]);
        }
        let pin = n / 500;
        for k in 0..pin {
            let dark = (k * 997) % n;
            let bright = (k * 991 + 13) % n;
            for c in 0..3 {
                data[dark * 3 + c] = 0.014;
                data[bright * 3 + c] = 0.93;
            }
        }
        ImageBuf::from_vec(w, h, data)
    }

    fn all_orientations() -> Vec<Orientation> {
        let mut out = Vec::new();
        for rotate in [Quarter::Zero, Quarter::Cw90, Quarter::Cw180, Quarter::Cw270] {
            for mirror in [false, true] {
                out.push(Orientation { rotate, mirror });
            }
        }
        out
    }

    /// The forward map: where a source rect lands after the orientation is applied. ⚠ Written
    /// out of [`Orientation`]'s own definition — *mirror, then turn* — so the round trip below
    /// tests `source_crop` against the meaning of the type rather than against itself.
    fn forward(crop: CropRect, o: Orientation) -> CropRect {
        let mut c = crop;
        if o.mirror {
            c = c.flipped_h();
        }
        let turns = match o.rotate {
            Quarter::Zero => 0,
            Quarter::Cw90 => 1,
            Quarter::Cw180 => 2,
            Quarter::Cw270 => 3,
        };
        for _ in 0..turns {
            c = c.rotated_cw();
        }
        c
    }

    #[test]
    fn the_crop_maps_back_through_every_orientation() {
        // ⚠ An asymmetric rect: a centred or square one is fixed by several of the eight
        // transforms, so it could not tell a wrong inverse from the right one.
        let c = CropRect {
            left: 0.1,
            top: 0.2,
            right: 0.4,
            bottom: 0.9,
        };
        for o in all_orientations() {
            let back = source_crop(forward(c, o), o);
            for (a, b) in [
                (back.left, c.left),
                (back.top, c.top),
                (back.right, c.right),
                (back.bottom, c.bottom),
            ] {
                // f32 equality does not survive `1.0 - (1.0 - 0.1)`; a millionth of the frame
                // is sub-pixel on any image.
                assert!((a - b).abs() < 1e-6, "{o:?}: {back:?} vs {c:?}");
            }
        }
    }

    #[test]
    fn a_quarter_turn_really_moves_the_crop() {
        // ⚠ The discriminating half. A `source_crop` that returned its argument unchanged would
        // pass the round trip above for `Quarter::Zero` and — far worse — an identity
        // implementation passes *nothing else in the suite*, because every other test here uses
        // a crop the orientation happens to fix. The left edge of an oriented portrait frame is
        // the bottom of the buffer.
        let c = CropRect {
            left: 0.0,
            top: 0.0,
            right: 0.25,
            bottom: 1.0,
        };
        let o = Orientation {
            rotate: Quarter::Cw90,
            mirror: false,
        };
        let s = source_crop(c, o);
        assert!(
            (s.top - 0.75).abs() < 1e-6 && (s.bottom - 1.0).abs() < 1e-6,
            "a left-edge crop under Cw90 must be the buffer's bottom edge, got {s:?}"
        );
        assert!(
            (s.left - 0.0).abs() < 1e-6 && (s.right - 1.0).abs() < 1e-6,
            "{s:?}"
        );
    }

    #[test]
    fn apply_writes_the_three_fields_and_nothing_else() {
        // ⚠ Every other field is set to a non-default value first, so a wildcard write — or an
        // `Edits::default()` assignment — cannot pass. The crop and the masks are the two the
        // user would most notice losing.
        let mut e = Edits {
            contrast: 40.0,
            highlights: -30.0,
            shadows: 25.0,
            saturation: 15.0,
            vibrance: -10.0,
            ..Default::default()
        };
        e.geometry.crop.left = 0.25;
        e.geometry.angle = 3.0;
        e.tone_curve.points = vec![[0.0, 0.0], [0.4, 0.6], [1.0, 1.0]];

        let got = apply(&mut e, &graded(500, 200));
        assert_eq!(e.exposure, got.exposure);
        assert_eq!(e.whites, got.whites);
        assert_eq!(e.blacks, got.blacks);
        // ⚠ A test that asserted something FALSE about correct code: this first demanded a
        // *positive* exposure, carried over from the uniform-0.04 fixture it replaced. The ramp's
        // median sits a shade above `MEDIAN_TARGET`, so a small negative correction is right. The
        // *direction* is asserted in `pipeline::auto_tone`, where the median is exact; what
        // matters here is that the analysis engaged and reached the fields.
        assert!(
            got.exposure.abs() > 0.01 && got.exposure.is_finite(),
            "the analysis should have moved the exposure, got {got:?}"
        );
        // ⚠ The fixture's own premise, asserted before anything rests on it: with no real tails
        // the two clip fields are 0 whatever `apply` does with them.
        assert!(
            got.whites != 0.0,
            "fixture check: the whites solve must engage"
        );
        assert!(
            got.blacks != 0.0,
            "fixture check: the blacks solve must engage"
        );

        assert_eq!(e.contrast, 40.0, "contrast must be left alone");
        assert_eq!(e.highlights, -30.0, "highlights must be left alone");
        assert_eq!(e.shadows, 25.0, "shadows must be left alone");
        assert_eq!(e.saturation, 15.0);
        assert_eq!(e.vibrance, -10.0);
        assert_eq!(e.geometry.crop.left, 0.25, "the crop must survive");
        assert_eq!(e.geometry.angle, 3.0);
        assert_eq!(e.tone_curve.points.len(), 3, "the curve must survive");
    }

    #[test]
    fn apply_analyses_the_cropped_region_of_the_buffer() {
        // ⚠⚠ The load-bearing test of this module: a bright border *outside* the crop must not
        // move the result, and the crop is expressed in the **oriented** frame while the buffer
        // is not. Under `Cw90` the oriented crop's left quarter is the buffer's bottom quarter,
        // so an implementation that skipped `source_crop` would measure the buffer's *left*
        // quarter — which here is the bright border, giving a completely different exposure.
        let (w, h) = (200usize, 100usize);
        let mut buf = grey(w, h, 2.0);
        for y in (h * 3 / 4)..h {
            for x in 0..w {
                buf.set_pixel(x, y, [0.04, 0.04, 0.04]);
            }
        }
        let mut e = Edits::default();
        e.geometry.orientation = Orientation {
            rotate: Quarter::Cw90,
            mirror: false,
        };
        e.geometry.crop = CropRect {
            left: 0.0,
            top: 0.0,
            right: 0.25,
            bottom: 1.0,
        };
        let dark = apply(&mut e, &buf);

        let mut whole = Edits {
            geometry: Geometry {
                orientation: e.geometry.orientation,
                ..Default::default()
            },
            ..Default::default()
        };
        let all = apply(&mut whole, &buf);
        assert!(
            dark.exposure > all.exposure + 1.0,
            "the crop must select the dark quarter: cropped {dark:?} vs whole {all:?}"
        );
        assert!(
            (dark.exposure - (auto_tone::MEDIAN_TARGET / 0.04).log2()).abs() < 0.05,
            "the cropped region is uniform 0.04, so the exposure is exact: {dark:?}"
        );
    }

    #[test]
    fn the_analysis_is_a_downscale_of_the_buffer_not_the_buffer() {
        // ⚠ A frame larger than `ANALYSIS_DIM` must still give the same answer — the downscale
        // is a cost decision, not a change of result. Uniform, so any sampling agrees.
        let big = grey(ANALYSIS_DIM * 3, ANALYSIS_DIM * 2, 0.08);
        let small = grey(64, 42, 0.08);
        let g = Geometry::default();
        let (_, a) = analyse(&big, &g);
        let (_, b) = analyse(&small, &g);
        assert!((a.exposure - b.exposure).abs() < 1e-5, "{a:?} vs {b:?}");
    }

    #[test]
    fn an_empty_buffer_is_the_neutral_answer() {
        let mut e = Edits::default();
        assert_eq!(apply(&mut e, &ImageBuf::new(0, 0)), AutoTone::default());
        assert_eq!(e.exposure, 0.0);
    }
}
