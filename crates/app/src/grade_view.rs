//! The grade range view: where a colour-grading range reaches on the photograph.
//!
//! ⚠ A **view mode, not an edit** — `visualize`'s rule. It never reaches `Edits`, the sidecar,
//! an export or the CLI. The engine takes the plane at the grading stage's *input* (the tail's
//! `before_grade` half), which is the only place the ranges are exact: everything after it —
//! the grade's own luminance offsets, the masks, the display rendering S-curve, the creative
//! LUT — moves every pixel's lightness.

use core_image::ImageBuf;
use rayon::prelude::*;

/// Which range the view shows.
///
/// ⚠ Three, not four: Global is a constant added after the weighted sum, not a range, so there
/// is nothing of it to show. The panel's four-way selector maps onto this with `Global => None`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GradeRangeView {
    Shadows,
    Midtones,
    Highlights,
}

impl GradeRangeView {
    pub const ALL: [GradeRangeView; 3] = [Self::Shadows, Self::Midtones, Self::Highlights];

    /// The index into `range_weights`'s `[shadows, midtones, highlights]`.
    pub fn index(self) -> usize {
        match self {
            Self::Shadows => 0,
            Self::Midtones => 1,
            Self::Highlights => 2,
        }
    }
}

/// The tint the plane is painted in: teal. ⚠ Not red — the brush tint is red and the two must
/// never be confusable — and not one of the four theme accents, so it cannot read as active
/// state. Off the skin-tone axis, for the reason the accent list excludes amber.
pub const TINT: [u8; 3] = [0, 190, 210];

/// One byte per pixel: the selected range's weight at that pixel's Oklab lightness, 0..=255.
///
/// ⚠⚠ Two things in the per-pixel line are load-bearing. **`prophoto_to_oklab`, the ProPhoto
/// pair** — the buffer is in the working space, and the stage records that mixing the pairs
/// "produces a plausible-looking colour shift that is very hard to attribute later"; here it
/// would misplace the ranges plausibly. **The clamp on the index, as the stage's table lookup
/// does** — a pixel above white reads as a highlight for both, so the two agree on the
/// brightest pixels, which are the ones a highlight wheel is for.
///
/// ⚠ `range_weights` is the stage's own, never a copy: the stage, the panel strip and this view
/// therefore cannot drift.
///
/// ⚠⚠ **The weights come from a 256-entry table, interpolated — the stage's own design, for the
/// stage's own reason, and it was MEASURED into place rather than reasoned.** Calling
/// `range_weights` per pixel is a `powf` for the width and three `exp`, and it cost **15.3 ms at
/// fit and 25.3 ms at a 2000×1400 region** (X-T4, release, best of 5) — the plane was the
/// most expensive thing in the frame while the spec had predicted single digits. Tabulated,
/// the per-pixel work is three `cbrt` and one lerp; the numbers after are in `CLAUDE.md`. The
/// table is built per plane, so Blending and Balance are read exactly once, from the stage.
pub fn coverage(buf: &ImageBuf, range: GradeRangeView, blending: f32, balance: f32) -> Vec<u8> {
    let (w, h) = (buf.width(), buf.height());
    if w == 0 || h == 0 {
        return Vec::new();
    }
    let k = range.index();
    // ⚠ 256 with linear interpolation, like the stage: the weight curves are smooth, so the
    // interpolation error is ~1e-4 of a weight, well inside the byte the plane is quantised to.
    // A nearest-entry lookup would NOT do — at Blending 0 the curve climbs ~12 weight units per
    // unit L, i.e. ~12 of 255 between adjacent entries.
    const TABLE: usize = 256;
    let table: Vec<f32> = (0..TABLE)
        .map(|i| {
            let l = i as f32 / (TABLE - 1) as f32;
            pipeline::ColorGradingStage::range_weights(l, blending, balance)[k]
        })
        .collect();
    let mut out = vec![0u8; w * h];
    // Parallel over rows: each row is a substantial task and reads nothing but itself, so the
    // result is identical to the sequential loop.
    out.par_chunks_mut(w)
        .zip(buf.data().par_chunks(w * 3))
        .for_each(|(row_out, row_in)| {
            for (dst, px) in row_out.iter_mut().zip(row_in.as_chunks::<3>().0) {
                let l = color::oklab::prophoto_to_oklab(*px)[0].clamp(0.0, 1.0);
                let x = l * (TABLE - 1) as f32;
                let i = (x as usize).min(TABLE - 2);
                let f = x - i as f32;
                let wgt = table[i] * (1.0 - f) + table[i + 1] * f;
                *dst = (wgt * 255.0).round().clamp(0.0, 255.0) as u8;
            }
        });
    out
}

/// The plane as RGBA bytes for a texture: [`TINT`] with alpha `weight / 2`, so full membership
/// reads as a 50% tint and a soft range edge reads as a soft edge — the brush tint's rule.
pub fn tint_rgba(plane: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(plane.len() * 4);
    for &a in plane {
        out.extend_from_slice(&[TINT[0], TINT[1], TINT[2], a / 2]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_image::ImageBuf;

    fn buf_of(pixels: &[[f32; 3]]) -> ImageBuf {
        let mut b = ImageBuf::new(pixels.len(), 1);
        for (i, p) in pixels.iter().enumerate() {
            b.set_pixel(i, 0, *p);
        }
        b
    }

    /// For a grey, Oklab L is Y^(1/3) to within 1e-6 (the ProPhoto→LMS rows sum to 1), which
    /// gives an expected value derived *without* `prophoto_to_oklab` — so this is not the
    /// function checked against itself.
    #[test]
    fn a_grey_ramp_matches_the_stages_weights() {
        let greys = [0.0f32, 0.02, 0.1, 0.18, 0.5, 0.9, 1.0];
        let buf = buf_of(&greys.map(|y| [y, y, y]));
        for range in GradeRangeView::ALL {
            for blending in [0.0f32, 50.0, 100.0] {
                for balance in [-60.0f32, 0.0, 60.0] {
                    let plane = coverage(&buf, range, blending, balance);
                    assert_eq!(plane.len(), greys.len());
                    for (i, y) in greys.iter().enumerate() {
                        let l = y.cbrt().clamp(0.0, 1.0);
                        let w = pipeline::ColorGradingStage::range_weights(l, blending, balance)
                            [range.index()];
                        let expected = (w * 255.0).round() as i32;
                        assert!(
                            (plane[i] as i32 - expected).abs() <= 1,
                            "{range:?} blending {blending} balance {balance} grey {y}: \
                             plane {} vs {expected}",
                            plane[i]
                        );
                    }
                }
            }
        }
    }

    /// ⚠⚠ The ranges are selected on Oklab L, NOT luminance — M3.3's rule. A saturated blue has
    /// an L well below its luminance's cube root: this pixel is a shadow by L and a midtone by
    /// luminance. Sabotage: `color::luminance(px).cbrt()` in `coverage` fails it.
    #[test]
    fn a_chromatic_pixel_is_placed_by_oklab_lightness() {
        let px = [0.01f32, 0.025, 0.45];
        // The premise, asserted rather than trusted: the two routes disagree on the range.
        let by_l = pipeline::ColorGradingStage::range_weights(
            color::oklab::prophoto_to_oklab(px)[0],
            0.0,
            0.0,
        )[0];
        let by_y =
            pipeline::ColorGradingStage::range_weights(color::luminance(px).cbrt(), 0.0, 0.0)[0];
        assert!(by_l > 0.9, "premise: by L this is a shadow, got {by_l}");
        assert!(by_y < 0.1, "premise: by luminance it is not, got {by_y}");

        let plane = coverage(&buf_of(&[px]), GradeRangeView::Shadows, 0.0, 0.0);
        assert!(
            plane[0] > 230,
            "the view did not follow Oklab L: {}",
            plane[0]
        );
    }

    /// A scene-referred pixel above white (`L > 1`) is a pure highlight, as the stage reads it.
    /// ⚠ The clamp in `coverage` mirrors the stage's table-index clamp for parity; the weights
    /// are normalised, so this test pins the *behaviour*, not the clamp.
    #[test]
    fn a_pixel_above_white_is_a_highlight() {
        let buf = buf_of(&[[1.6, 1.6, 1.6]]);
        assert_eq!(coverage(&buf, GradeRangeView::Highlights, 0.0, 0.0)[0], 255);
        assert_eq!(coverage(&buf, GradeRangeView::Shadows, 0.0, 0.0)[0], 0);
    }

    #[test]
    fn tint_rgba_is_the_fixed_tint_with_alpha_from_the_weight() {
        let rgba = tint_rgba(&[0, 255, 128]);
        assert_eq!(
            rgba,
            vec![0, 190, 210, 0, 0, 190, 210, 127, 0, 190, 210, 64],
            "one RGBA quad per byte: TINT, alpha = weight / 2"
        );
    }

    #[test]
    fn an_empty_buffer_gives_an_empty_plane() {
        assert!(coverage(&ImageBuf::new(0, 0), GradeRangeView::Midtones, 50.0, 0.0).is_empty());
    }
}
