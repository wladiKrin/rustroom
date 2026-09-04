//! The glue behind the Auto button — everything except its drawing.
//!
//! ⚠ Lives in the lib, with no egui in sight, because that is where a unit-testable rule belongs:
//! `auto_tone_tool`'s pattern, which this module follows line for line.
//!
//! ⚠⚠ **The buffer it reads is `Session.buffer`, never the rendered texture.** That texture
//! already has the current perspective applied, so lines that are already corrected have no
//! vanishing point left to find — and it is a GPU handle that cannot be sampled anyway.
//! `auto_tone_tool` records the general form of the rule: analysing the render "would make the
//! auto chase its own correction."

use core_image::ImageBuf;
use edit_model::{CropRect, Edits, Geometry};
use lens_profile::LensShot;
use pipeline::perspective::detect;
use pipeline::perspective::guides::Guide;
use pipeline::{GeometryStage, LensCorrectionStage, PipelineStage, Resampler};

/// The analysis runs on a downscale to this long edge.
///
/// ⚠ `auto_tone_tool::ANALYSIS_DIM`'s value and its reasoning: 512 px keeps the button instant on
/// a 26 Mpx buffer, and a building edge that does not survive a downscale to 512 is not the edge a
/// keystone correction is about.
pub const ANALYSIS_DIM: usize = 512;

/// The luminance plane the detector runs on, in **Guide** coordinates.
///
/// ⚠⚠ **Built by running the two REAL prefix stages that precede Guide space**, on a downscale —
/// not by a hand-rolled sampler. `LensCorrectionStage` builds its own `Corrector` from the
/// dimensions of the image it is handed (its own doc: handing one in "would make a size mismatch
/// *representable*, and it would be silently wrong rather than an error"), so running it on the
/// 512 px plane is correct by construction. `GeometryStage` applies the orientation the same way,
/// and `mod stages` is private so its `orient` is unreachable from here in any case. One rule,
/// rather than a second implementation that can disagree with the render.
///
/// ⚠⚠ **The lens step is the part that looks skippable and is not.** The prefix order in
/// `pipeline/src/lib.rs` is LensCorrection → Perspective → SpotRemoval → Geometry, so the decode
/// buffer is *pre*-lens-correction — and lens correction is on by default for RAW. On the raw
/// buffer, barrel distortion **curves every building edge**: each edge then fragments into several
/// Hough lines at slightly different angles, and the vanishing point they imply is biased. The
/// result is not an error; it is a correction that is merely *plausible*, which this project's
/// records name repeatedly as its worst failure shape.
///
/// ⚠ `angle: 0.0` and a full crop: the straighten and the crop are *downstream* of Guide space, so
/// applying them here would put the detected lines in the wrong frame.
pub fn analysis_luma(
    buffer: &ImageBuf,
    edits: &Edits,
    lens: Option<LensShot>,
    resampler: Resampler,
) -> (Vec<f32>, usize, usize) {
    let small = buffer.downscale_to(ANALYSIS_DIM);
    let small = match (edits.lens_correction.is_active(), lens) {
        (true, Some(c)) => LensCorrectionStage::new(
            c,
            lens_profile::Corrections {
                distortion: edits.lens_correction.distortion,
                tca: edits.lens_correction.tca,
                vignetting: edits.lens_correction.vignetting,
                scale: edits.lens_correction.scale,
            },
            resampler,
        )
        .apply(small),
        _ => small,
    };
    let oriented = GeometryStage::new(
        &Geometry {
            orientation: edits.geometry.orientation,
            angle: 0.0,
            crop: CropRect::default(),
        },
        resampler,
    )
    .apply(small);

    let (w, h) = (oriented.width(), oriented.height());
    let mut luma = Vec::with_capacity(w * h);
    for y in 0..h {
        for x in 0..w {
            // ⚠ `color::luminance` — the working space's Y, "the quantity every tuned constant in
            // this pipeline is measured against" (`auto_tone`'s module doc). Not a Rec.709 copy.
            luma.push(color::luminance(oriented.pixel(x, y)));
        }
    }
    (luma, w, h)
}

/// The near-vertical lines detected in this photograph, ready for `guides::solve`.
pub fn detect_verticals(
    buffer: &ImageBuf,
    edits: &Edits,
    lens: Option<LensShot>,
    resampler: Resampler,
) -> Vec<Guide> {
    let (luma, w, h) = analysis_luma(buffer, edits, lens, resampler);
    detect::verticals(&luma, w, h)
}

/// As [`detect_verticals`], with the contrast floor supplied in 8-bit levels.
///
/// ⚠ For `detect_probe --sweep` alone: `detect::MIN_EDGE_LEVELS` is a `const`, so a sweep over
/// candidate values needs the floor injected. Production calls [`detect_verticals`], which is
/// defined through the same code path.
pub fn detect_verticals_with_floor(
    buffer: &ImageBuf,
    edits: &Edits,
    lens: Option<LensShot>,
    resampler: Resampler,
    floor_levels: f32,
) -> Vec<Guide> {
    let (luma, w, h) = analysis_luma(buffer, edits, lens, resampler);
    detect::verticals_with_floor(&luma, w, h, floor_levels)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame with two converging near-vertical bright edges, `w` × `h`.
    ///
    /// ⚠ The edges have a **smooth cross-section**, for the reason `detect.rs`'s own `line`
    /// helper records at length: a hard-painted near-vertical line is a *staircase* of vertical
    /// runs and one-pixel risers, which is a fixture about pixel rounding rather than about a
    /// photographic edge.
    fn building(w: usize, h: usize) -> ImageBuf {
        let mut v = vec![0.05f32; w * h * 3];
        for y in 0..h {
            let t = y as f32 / (h - 1) as f32;
            for (a, b) in [(0.25f32, 0.35f32), (0.75, 0.65)] {
                let xc = (a + (b - a) * (1.0 - t)) * w as f32;
                for dx in -4i64..=4 {
                    let xi = match xc.round() as i64 + dx {
                        x if (0..w as i64).contains(&x) => x as usize,
                        _ => continue,
                    };
                    let d = xi as f32 - xc;
                    let cov = (-(d * d) / (1.2 * 1.2)).exp();
                    let val = 0.05 + 0.95 * cov;
                    let i = (y * w + xi) * 3;
                    for c in 0..3 {
                        v[i + c] = v[i + c].max(val);
                    }
                }
            }
        }
        ImageBuf::from_vec(w, h, v)
    }

    fn neutral() -> Edits {
        Edits::default()
    }

    /// The baseline: with no orientation and no lens profile, the detector finds the two edges.
    #[test]
    fn two_edges_in_a_plain_frame_are_found() {
        let b = building(256, 256);
        let found = detect_verticals(&b, &neutral(), None, Resampler::default());
        assert_eq!(found.len(), 2, "expected two lines, got {}", found.len());
    }

    /// ⚠⚠ **Orientation is undone, and nothing else can see this.** On a quarter-turned photo the
    /// buffer's verticals are the frame's horizontals: analysed raw, the detector would find
    /// nothing at all — or worse, find the wrong family and hand `solve` a correction for an axis
    /// nobody asked about. `auto_tone_tool::source_crop`'s test is the model, and its comment is
    /// the reason: "a crop taken on the left of the *oriented* image is the *bottom* of the
    /// buffer."
    #[test]
    fn a_quarter_turn_is_undone_before_detection() {
        // A buffer whose *own* edges run horizontally, in a photo turned a quarter so they are
        // vertical on screen. Smooth-edged, for `building`'s reason.
        let (w, h) = (256usize, 256usize);
        let mut v = vec![0.05f32; w * h * 3];
        for yc in [60.0f32, 190.0] {
            for y in 0..h {
                let d = y as f32 - yc;
                if d.abs() > 4.0 {
                    continue;
                }
                let val = 0.05 + 0.95 * (-(d * d) / (1.2 * 1.2)).exp();
                for x in 0..w {
                    let i = (y * w + x) * 3;
                    for c in 0..3 {
                        v[i + c] = v[i + c].max(val);
                    }
                }
            }
        }
        let b = ImageBuf::from_vec(w, h, v);

        // Unturned, those are horizontals and the vertical detector must find nothing.
        assert!(
            detect_verticals(&b, &neutral(), None, Resampler::default()).is_empty(),
            "fixture check: the buffer's own edges are horizontal"
        );

        let mut e = neutral();
        e.geometry.orientation.rotate_cw();
        let found = detect_verticals(&b, &e, None, Resampler::default());
        assert_eq!(
            found.len(),
            2,
            "the orientation was not undone: {} lines found",
            found.len()
        );
    }

    /// ⚠ The analysis plane is at `ANALYSIS_DIM`, so the button stays instant on a real buffer.
    #[test]
    fn the_analysis_plane_is_downscaled() {
        let b = building(ANALYSIS_DIM * 3, ANALYSIS_DIM * 2);
        let (luma, w, h) = analysis_luma(&b, &neutral(), None, Resampler::default());
        assert_eq!(luma.len(), w * h);
        assert!(
            w.max(h) <= ANALYSIS_DIM,
            "the plane was not downscaled: {w}x{h}"
        );
    }

    /// A flat frame finds nothing, all the way through the glue.
    #[test]
    fn a_flat_frame_finds_nothing_through_the_glue() {
        let b = ImageBuf::from_vec(128, 128, vec![0.3f32; 128 * 128 * 3]);
        assert!(detect_verticals(&b, &neutral(), None, Resampler::default()).is_empty());
    }

    /// ⚠ The straighten angle and the crop are **downstream** of Guide space, so neither may
    /// reach the analysis image. A guide is stored pre-straighten and pre-crop, so applying
    /// either here would put every detected line in a frame that `guides::to_frame` then maps
    /// *again* — the correction would be applied twice and come out merely plausible.
    #[test]
    fn the_straighten_angle_and_the_crop_do_not_reach_the_analysis_image() {
        let b = building(256, 256);
        let plain = detect_verticals(&b, &neutral(), None, Resampler::default());
        let mut e = neutral();
        e.geometry.angle = 12.0;
        e.geometry.crop = CropRect {
            left: 0.3,
            top: 0.1,
            right: 0.7,
            bottom: 0.9,
        };
        let with = detect_verticals(&b, &e, None, Resampler::default());
        assert_eq!(
            plain, with,
            "the straighten or the crop reached the analysis"
        );
    }
}
