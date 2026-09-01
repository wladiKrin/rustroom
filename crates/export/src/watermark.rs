//! A visible mark laid over the exported image.
//!
//! ⚠⚠ **APPLIED TO THE DISPLAY-ENCODED BUFFER, AFTER THE PIPELINE AND BEFORE THE ENCODE — NOT
//! IN LINEAR LIGHT, AND THAT IS DELIBERATE.** A watermark is not part of the photograph: it is
//! ink laid on the print. "50% opacity" has to mean what it looks like, and a 50% blend in
//! linear light lands at about 73% on screen — visibly wrong, and wrong differently in the
//! shadows than in the highlights. Compositing where the numbers are perceptual is the whole
//! point. Contrast `SharpenStage`, which measures *the photograph* and therefore must be
//! linear.
//!
//! ⚠ Being after the encode transform also makes it survive every format and bit depth for
//! free: `save` quantizes whatever it is handed, so an 8-bit JPEG and a 16-bit TIFF get the
//! same mark from the same code, with no per-format branch to keep in step.
//!
//! ⚠ **Text only.** An image watermark — a PNG logo with alpha — is a contained follow-up: it
//! needs a path field, a file picker, a decode, and an answer to "what happens when the file has
//! moved", which is the same class of problem an export preset's destination already has. The
//! compositing below would serve it unchanged.

use ab_glyph::{point, Font, FontRef, PxScale, ScaleFont};
use core_image::ImageBuf;
use serde::{Deserialize, Serialize};

/// Which corner the mark sits in.
///
/// ⚠ Corners only, no centre. A centred watermark is a *defacement* mark, whose job is to be
/// hard to remove; this one's job is to credit a deliverable, and covering the subject with it
/// is a different feature with different requirements (tiling, rotation, low contrast).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum WatermarkCorner {
    TopLeft,
    TopRight,
    BottomLeft,
    #[default]
    BottomRight,
}

impl WatermarkCorner {
    /// ⚠ The order the UI draws, defined **here** beside the variants rather than in the dialog,
    /// so a fifth corner cannot land in a list the panel does not show — the rule
    /// `EDIT_GROUP_NAMES`, `Tool::ALL` and `FlagSet::ALL` already follow.
    pub const ALL: [WatermarkCorner; 4] = [
        WatermarkCorner::TopLeft,
        WatermarkCorner::TopRight,
        WatermarkCorner::BottomLeft,
        WatermarkCorner::BottomRight,
    ];

    pub fn label(self) -> &'static str {
        match self {
            WatermarkCorner::TopLeft => "Top left",
            WatermarkCorner::TopRight => "Top right",
            WatermarkCorner::BottomLeft => "Bottom left",
            WatermarkCorner::BottomRight => "Bottom right",
        }
    }
}

/// A visible mark laid over the exported image.
///
/// ⚠ Empty text means no watermark, which is what makes the default inert — the same rule
/// `Vignette`, `Sharpening` and `NoiseReduction` follow with an amount of zero.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Watermark {
    pub text: String,
    pub corner: WatermarkCorner,
    /// Cap height as a percentage of the image's **long edge**, 1..=20.
    ///
    /// ⚠ A fraction rather than a point size, so the mark is the same size relative to the
    /// picture whether the export is 800 px wide or 6000 — which is what a "fit within a long
    /// edge" export needs, since the same preset produces both.
    pub size: u8,
    /// 0..=100. ⚠ `0` is fully transparent, which is a legitimate way to switch the mark off
    /// without losing the text you typed.
    pub opacity: u8,
}

impl Default for Watermark {
    /// ⚠⚠ **Hand-written, not derived, and the doc comment is the guard.** `size` and `opacity`
    /// are non-zero at neutral, and the struct carries a *container-level* `#[serde(default)]`
    /// — which fills a missing field from **this** value. A derived `Default` would silently
    /// hand every existing options file a zero-height, fully transparent watermark. Exactly the
    /// trap `RadialGradient::default` and `ColorGrading::default` already record.
    fn default() -> Self {
        Self {
            text: String::new(),
            corner: WatermarkCorner::default(),
            size: 3,
            opacity: 70,
        }
    }
}

impl Watermark {
    pub const SIZE_RANGE: (u8, u8) = (1, 20);

    /// Whether anything would be drawn.
    ///
    /// ⚠ Trimmed: a mark made of spaces draws nothing but would still cost a rasterization pass
    /// and a full traversal of the buffer.
    pub fn is_active(&self) -> bool {
        !self.text.trim().is_empty() && self.opacity > 0
    }
}

/// The typeface the mark is drawn in.
///
/// ⚠ Ubuntu Light is what egui already renders this application's own text in, so the mark
/// matches the program that made it. See the dependency's note in `Cargo.toml` for why taking
/// it from `epaint_default_fonts` is not a GUI dependency.
const FONT: &[u8] = epaint_default_fonts::UBUNTU_LIGHT;

/// Margin from the edges, as a fraction of the long edge.
const MARGIN: f32 = 0.02;

/// How far the shadow is offset from the text, as a fraction of the cap height.
const SHADOW: f32 = 0.08;

/// A rasterized line of text: coverage in `0..=1`, row-major.
///
/// ⚠ A pure function returning a plain buffer, deliberately: it is the half worth testing, and
/// it can be tested with no image, no file and no compositing in the way.
pub struct TextMask {
    pub width: usize,
    pub height: usize,
    pub coverage: Vec<f32>,
}

/// Rasterize `text` at `px` pixels of nominal height.
///
/// Returns `None` when the text has no drawable glyphs at all — an empty string, or one made
/// only of characters this face does not have.
pub fn rasterize(text: &str, px: f32) -> Option<TextMask> {
    let font = FontRef::try_from_slice(FONT).ok()?;
    let scaled = font.as_scaled(PxScale::from(px));

    // Lay the line out on a baseline at the font's ascent, so descenders have room below it.
    let mut caret = 0.0f32;
    let mut glyphs = Vec::new();
    let mut prev = None;
    for c in text.chars() {
        let id = scaled.glyph_id(c);
        if let Some(p) = prev {
            // ⚠ Kerning is read from the face rather than skipped. Without it a proportional
            // face sets "AV" and "To" visibly loose, which reads as a rendering fault rather
            // than as a style.
            caret += scaled.kern(p, id);
        }
        glyphs.push(id.with_scale_and_position(px, point(caret, scaled.ascent())));
        caret += scaled.h_advance(id);
        prev = Some(id);
    }

    let width = caret.ceil().max(1.0) as usize;
    let height = scaled.height().ceil().max(1.0) as usize;
    let mut coverage = vec![0.0f32; width * height];
    let mut drew = false;
    for g in glyphs {
        let Some(outline) = font.outline_glyph(g) else {
            continue;
        };
        let bounds = outline.px_bounds();
        outline.draw(|gx, gy, c| {
            let x = bounds.min.x as i32 + gx as i32;
            let y = bounds.min.y as i32 + gy as i32;
            if x < 0 || y < 0 || x as usize >= width || y as usize >= height {
                return;
            }
            let i = y as usize * width + x as usize;
            // ⚠ `max`, not `+=`. Adjacent glyphs can overlap by a fraction of a pixel, and
            // accumulating there produces a bright seam down the join.
            coverage[i] = coverage[i].max(c);
            drew = true;
        });
    }
    drew.then_some(TextMask {
        width,
        height,
        coverage,
    })
}

/// Draw `mark` onto a **display-encoded** image, in place.
///
/// A no-op when the mark is inactive or does not fit, so a caller never has to ask first.
pub fn apply(image: &mut ImageBuf, mark: &Watermark) {
    if !mark.is_active() {
        return;
    }
    let (w, h) = (image.width(), image.height());
    let long = w.max(h) as f32;
    let px =
        long * f32::from(
            mark.size
                .clamp(Watermark::SIZE_RANGE.0, Watermark::SIZE_RANGE.1),
        ) / 100.0;
    let Some(mask) = rasterize(mark.text.trim(), px) else {
        return;
    };
    let shadow = (px * SHADOW).round().max(1.0) as usize;
    let margin = (long * MARGIN).round() as usize;
    // The whole mark, shadow included, has to fit inside the margins.
    let total_w = mask.width + shadow;
    let total_h = mask.height + shadow;
    if total_w + 2 * margin > w || total_h + 2 * margin > h {
        // ⚠ Silently skipped rather than clipped or shrunk. A watermark half off the edge looks
        // like a bug in the export; an absent one on a thumbnail-sized file looks like the size
        // slider being too large, which is true and is one drag from fixed.
        return;
    }
    let (ox, oy) = match mark.corner {
        WatermarkCorner::TopLeft => (margin, margin),
        WatermarkCorner::TopRight => (w - margin - total_w, margin),
        WatermarkCorner::BottomLeft => (margin, h - margin - total_h),
        WatermarkCorner::BottomRight => (w - margin - total_w, h - margin - total_h),
    };
    let alpha = f32::from(mark.opacity.min(100)) / 100.0;

    // ⚠⚠ **Shadow first, then the text over it.** White alone vanishes on a bright sky and
    // black alone vanishes on a dark subject; the pair reads on both. This is the same finding
    // the crop overlay reached — a light mark over a dark halo — arrived at there by a user
    // reporting he could not see the white border.
    composite(
        image,
        &mask,
        ox + shadow,
        oy + shadow,
        [0.0, 0.0, 0.0],
        alpha,
    );
    composite(image, &mask, ox, oy, [1.0, 1.0, 1.0], alpha);
}

/// `dst = dst·(1 − a) + colour·a`, with `a = coverage · alpha`.
fn composite(image: &mut ImageBuf, mask: &TextMask, ox: usize, oy: usize, rgb: [f32; 3], a: f32) {
    for y in 0..mask.height {
        for x in 0..mask.width {
            let c = mask.coverage[y * mask.width + x];
            if c <= 0.0 {
                continue;
            }
            let k = c * a;
            let px = image.pixel(ox + x, oy + y);
            image.set_pixel(
                ox + x,
                oy + y,
                [
                    px[0] * (1.0 - k) + rgb[0] * k,
                    px[1] * (1.0 - k) + rgb[1] * k,
                    px[2] * (1.0 - k) + rgb[2] * k,
                ],
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grey(w: usize, h: usize, v: f32) -> ImageBuf {
        let mut img = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                img.set_pixel(x, y, [v, v, v]);
            }
        }
        img
    }

    fn marked(text: &str) -> Watermark {
        Watermark {
            text: text.into(),
            ..Watermark::default()
        }
    }

    fn total_ink(a: &ImageBuf, b: &ImageBuf) -> f32 {
        a.data()
            .iter()
            .zip(b.data())
            .map(|(x, y)| (x - y).abs())
            .sum()
    }

    #[test]
    fn the_default_is_inert_and_its_other_fields_are_not_zero() {
        // ⚠ Both halves. A derived `Default` would also be inert (empty text), so "inert" alone
        // cannot see the bug this guards — the sabotage is `#[derive(Default)]`, which leaves a
        // zero size and zero opacity for every options file that predates these fields.
        let d = Watermark::default();
        assert!(!d.is_active());
        assert_eq!(d.size, 3, "a derived Default would give 0");
        assert_eq!(d.opacity, 70, "a derived Default would give 0");
    }

    #[test]
    fn an_empty_or_transparent_mark_leaves_the_image_untouched() {
        let base = grey(200, 120, 0.5);
        for m in [
            Watermark::default(),
            marked("   "),
            Watermark {
                opacity: 0,
                ..marked("(c) me")
            },
        ] {
            let mut img = base.clone();
            apply(&mut img, &m);
            assert_eq!(img.data(), base.data(), "{m:?}");
        }
    }

    #[test]
    fn a_mark_writes_pixels_and_only_near_its_corner() {
        // ⚠ The discriminating half is *where*: a watermark that draws is easy, one that lands
        // in the corner it was asked for is what a corner setting means.
        let base = grey(400, 300, 0.5);
        for corner in WatermarkCorner::ALL {
            let mut img = base.clone();
            apply(
                &mut img,
                &Watermark {
                    corner,
                    ..marked("Rustroom")
                },
            );
            assert!(
                total_ink(&img, &base) > 0.0,
                "{corner:?}: nothing was drawn"
            );
            // Split the frame into quadrants and demand every changed pixel sit in the right one.
            let (mid_x, mid_y) = (200, 150);
            let (want_left, want_top) = match corner {
                WatermarkCorner::TopLeft => (true, true),
                WatermarkCorner::TopRight => (false, true),
                WatermarkCorner::BottomLeft => (true, false),
                WatermarkCorner::BottomRight => (false, false),
            };
            for y in 0..300 {
                for x in 0..400 {
                    if img.pixel(x, y) != base.pixel(x, y) {
                        assert_eq!(x < mid_x, want_left, "{corner:?}: ink at ({x},{y})");
                        assert_eq!(y < mid_y, want_top, "{corner:?}: ink at ({x},{y})");
                    }
                }
            }
        }
    }

    #[test]
    fn opacity_scales_how_far_the_pixels_move() {
        let base = grey(400, 300, 0.5);
        let ink = |o: u8| {
            let mut img = base.clone();
            apply(
                &mut img,
                &Watermark {
                    opacity: o,
                    ..marked("Rustroom")
                },
            );
            total_ink(&img, &base)
        };
        let (half, full) = (ink(50), ink(100));
        assert!(half > 0.0 && full > half, "half {half}, full {full}");
        // ⚠⚠ **Roughly half, not exactly half — and the first version of this assertion was
        // FALSE about correct code.** Each blend is linear in alpha, but the shadow and the text
        // *overlap*, and the second composite acts on the first one's output, so the total
        // movement carries a small quadratic term. Measured at 0.489 of full. The band still
        // discriminates the sabotage it exists for: alpha squared would put half at ~0.25 of
        // full, and a clamped or ignored opacity at 1.0.
        let ratio = half / full;
        assert!(
            (0.42..=0.58).contains(&ratio),
            "half {half}, full {full}, ratio {ratio}"
        );
    }

    #[test]
    fn the_mark_scales_with_the_image_rather_than_with_the_pixel() {
        // ⚠⚠ **THE FIRST VERSION OF THIS TEST COULD NOT SEE WHAT IT NAMED — instance seventeen.**
        // It called `rasterize` directly with a pre-computed pixel height, so it asserted that
        // the *rasterizer* scales with its argument, which is true by construction. The property
        // that matters lives in `apply`'s size→pixels mapping, and the sabotage that replaces
        // that fraction with a fixed pixel count left it green.
        //
        // This is what makes one export preset work at 800 px and at 6000: the mark's *fraction*
        // of the frame is what stays fixed, not its pixel height.
        let ink_height = |w: usize, h: usize| {
            let base = grey(w, h, 0.5);
            let mut img = base.clone();
            apply(
                &mut img,
                &Watermark {
                    size: 6,
                    ..marked("Rustroom")
                },
            );
            let rows: Vec<usize> = (0..h)
                .filter(|&y| (0..w).any(|x| img.pixel(x, y) != base.pixel(x, y)))
                .collect();
            assert!(!rows.is_empty(), "{w}x{h}: nothing was drawn");
            rows[rows.len() - 1] - rows[0] + 1
        };
        let (small, large) = (ink_height(400, 300), ink_height(800, 600));
        let ratio = large as f32 / small as f32;
        assert!(
            (ratio - 2.0).abs() < 0.15,
            "small {small}, large {large}, ratio {ratio}"
        );
    }

    #[test]
    fn a_mark_too_large_for_the_frame_is_skipped_rather_than_clipped() {
        // 20% of a 40 px long edge is 8 px of text — but the text is long, so it cannot fit.
        let base = grey(40, 30, 0.5);
        let mut img = base.clone();
        apply(
            &mut img,
            &Watermark {
                size: 20,
                ..marked("a very long piece of credit text indeed")
            },
        );
        assert_eq!(img.data(), base.data());
    }

    #[test]
    fn the_mark_carries_a_shadow_as_well_as_light_ink() {
        // ⚠ Both, and this is the test that can see either one missing: white alone vanishes on
        // a bright sky, black alone on a dark subject.
        let base = grey(400, 300, 0.5);
        let mut img = base.clone();
        apply(&mut img, &marked("Rustroom"));
        let mut lighter = false;
        let mut darker = false;
        for (a, b) in img.data().iter().zip(base.data()) {
            if a > b {
                lighter = true;
            }
            if a < b {
                darker = true;
            }
        }
        assert!(lighter, "no light ink");
        assert!(darker, "no shadow");
    }

    #[test]
    fn rasterizing_nothing_drawable_yields_no_mask() {
        assert!(rasterize("", 20.0).is_none());
        // A run of spaces has glyphs with advance and no outline.
        assert!(rasterize("   ", 20.0).is_none());
    }
}
