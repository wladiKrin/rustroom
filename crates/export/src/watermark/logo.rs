//! The image watermark (M10.10a): a logo with alpha, decoded once per export, drawn per photo.
//!
//! ⚠⚠ **CONVERTED INTO THE EXPORT'S SPACE AT DECODE, BECAUSE THE BUFFER IT LANDS ON IS ALREADY
//! THERE.** [`apply`](super::apply) runs after the pipeline's output transform, so a Display P3
//! export's pixels are P3-encoded; laying a logo's sRGB numbers on them unconverted would draw its
//! red more saturated than the file the designer made. ⚠ The file is taken to be **sRGB** — what
//! logos are saved in almost without exception; an embedded profile is not read.
//!
//! ⚠⚠ **PREMULTIPLIED, AND THAT IS WHAT KEEPS THE EDGE CLEAN.** A resize in straight alpha averages
//! the colour of the *transparent* pixels into the edge beside them — black usually, whatever an
//! editor left there sometimes — and it shows as a dark or coloured fringe round the logo.
//! `a_transparent_hole_leaves_the_photo_untouched` hides red there to catch exactly that.
//!
//! ⚠ No drop shadow, unlike the text: a logo carries its own contrast, and a shadow would put a
//! dark copy of its shape beside it that its designer never drew.

use super::{place, Watermark, MARGIN};
use core_image::ImageBuf;
use image::imageops::{self, FilterType};
use image::Rgba32FImage;

/// The longest edge a logo is kept at.
///
/// ⚠ A logo is held as f32 RGBA for the whole run — 16 bytes a pixel — so a 10,000 px file would
/// hold over a gigabyte. Shrunk **once** at decode; no export needs more, since the widest mark is
/// half the long edge.
pub const MAX_LOGO: u32 = 4096;

/// A decoded logo: premultiplied RGBA, encoded in the export's colour space.
pub struct Logo {
    img: Rgba32FImage,
}

impl Logo {
    /// Decode a PNG, JPEG, TIFF or WebP file's bytes into `space`.
    ///
    /// `Err` carries the decoder's reason; never a panic, whatever the bytes.
    pub fn decode(bytes: &[u8], space: color::OutputSpace) -> Result<Logo, String> {
        let mut img = image::load_from_memory(bytes)
            .map_err(|e| e.to_string())?
            .to_rgba32f();
        let (w, h) = img.dimensions();
        if w == 0 || h == 0 {
            return Err("the image is empty".to_string());
        }
        for p in img.pixels_mut() {
            let [r, g, b, a] = p.0;
            // ⚠ sRGB is the identity and is left untouched, so an sRGB export draws the file's own
            // numbers rather than a round trip through linear light.
            let c = if space == color::OutputSpace::Srgb {
                [r, g, b]
            } else {
                space
                    .from_srgb([r, g, b].map(color::srgb_to_linear))
                    .map(|v| space.encode(v))
            };
            let a = a.clamp(0.0, 1.0);
            p.0 = [c[0] * a, c[1] * a, c[2] * a, a];
        }
        if w.max(h) > MAX_LOGO {
            let (nw, nh) = if w >= h {
                (MAX_LOGO, scaled(h, MAX_LOGO, w))
            } else {
                (scaled(w, MAX_LOGO, h), MAX_LOGO)
            };
            img = imageops::resize(&img, nw, nh, FilterType::Lanczos3);
        }
        Ok(Logo { img })
    }

    pub fn width(&self) -> usize {
        self.img.width() as usize
    }

    pub fn height(&self) -> usize {
        self.img.height() as usize
    }

    /// One pixel, premultiplied `[r, g, b, a]`.
    pub fn pixel(&self, x: usize, y: usize) -> [f32; 4] {
        self.img.get_pixel(x as u32, y as u32).0
    }
}

/// `side · to / from`, rounded, at least 1.
fn scaled(side: u32, to: u32, from: u32) -> u32 {
    ((side as f32 * to as f32 / from as f32).round() as u32).max(1)
}

/// Lay `logo` on a display-encoded image in `mark`'s corner, `mark.image_width` % of the long edge
/// wide, blended by its own alpha × `mark.opacity`.
pub(super) fn draw(image: &mut ImageBuf, logo: &Logo, mark: &Watermark) {
    let (w, h) = (image.width(), image.height());
    let long = w.max(h) as f32;
    let (lo, hi) = Watermark::IMAGE_WIDTH_RANGE;
    let pct = f32::from(mark.image_width.clamp(lo, hi));
    let tw = ((long * pct / 100.0).round() as u32).max(1);
    let (lw, lh) = logo.img.dimensions();
    let th = scaled(lh, tw, lw);
    let margin = (long * MARGIN).round() as usize;
    let Some((ox, oy)) = place(mark.corner, (w, h), margin, (tw as usize, th as usize)) else {
        return;
    };
    let resized;
    let sized = if (tw, th) == (lw, lh) {
        &logo.img
    } else {
        // ⚠ Per photo, because a selection's photos can differ in size; the decode is per run.
        resized = imageops::resize(&logo.img, tw, th, FilterType::Lanczos3);
        &resized
    };
    let op = f32::from(mark.opacity.min(100)) / 100.0;
    for (x, y, p) in sized.enumerate_pixels() {
        // ⚠ Lanczos rings: alpha can leave [0, 1] and a channel can exceed its alpha, which in
        // premultiplied terms is a colour brighter than white. Both are clamped back.
        let a = p.0[3].clamp(0.0, 1.0);
        if a <= 0.0 {
            continue;
        }
        let c = [p.0[0], p.0[1], p.0[2]].map(|v| v.clamp(0.0, a));
        let k = a * op;
        let (px, py) = (ox + x as usize, oy + y as usize);
        let d = image.pixel(px, py);
        image.set_pixel(
            px,
            py,
            [
                d[0] * (1.0 - k) + c[0] * op,
                d[1] * (1.0 - k) + c[1] * op,
                d[2] * (1.0 - k) + c[2] * op,
            ],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::watermark::{apply, Watermark, WatermarkCorner, WatermarkKind};
    use color::OutputSpace;

    /// A PNG file's bytes, `w`×`h`, each pixel from `f` as straight RGBA.
    fn png(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> Vec<u8> {
        let img = image::RgbaImage::from_fn(w, h, |x, y| image::Rgba(f(x, y)));
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut out, image::ImageFormat::Png)
            .expect("encode a test PNG");
        out.into_inner()
    }

    fn white(w: u32, h: u32) -> Logo {
        Logo::decode(&png(w, h, |_, _| [255; 4]), OutputSpace::Srgb).expect("decode")
    }

    fn grey(w: usize, h: usize, v: f32) -> ImageBuf {
        let mut img = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                img.set_pixel(x, y, [v, v, v]);
            }
        }
        img
    }

    fn mark(width: u8, opacity: u8, corner: WatermarkCorner) -> Watermark {
        Watermark {
            kind: WatermarkKind::Image,
            image: "logo.png".into(),
            image_width: width,
            opacity,
            corner,
            ..Watermark::default()
        }
    }

    fn changed(a: &ImageBuf, b: &ImageBuf) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        for y in 0..a.height() {
            for x in 0..a.width() {
                if a.pixel(x, y) != b.pixel(x, y) {
                    out.push((x, y));
                }
            }
        }
        out
    }

    #[test]
    fn a_transparent_hole_leaves_the_photo_untouched() {
        // ⚠⚠ The hidden half is RED, and that is the discriminating choice: a resize in straight
        // alpha averages that red into the white edge beside it — the fringe premultiplying exists to
        // prevent — and ignoring the alpha altogether paints the whole hole. Both show here.
        let logo = Logo::decode(
            &png(40, 20, |x, _| {
                if x < 20 {
                    [255, 0, 0, 0]
                } else {
                    [255, 255, 255, 255]
                }
            }),
            OutputSpace::Srgb,
        )
        .expect("decode");
        let base = grey(400, 300, 0.5);
        let mut img = base.clone();
        apply(
            &mut img,
            &mark(20, 100, WatermarkCorner::BottomRight),
            Some(&logo),
        );
        let px: Vec<[f32; 3]> = (0..300)
            .flat_map(|y| (0..400).map(move |x| (x, y)))
            .map(|(x, y)| img.pixel(x, y))
            .collect();
        assert!(
            px.iter().any(|p| p[0] >= 0.99 && p[1] >= 0.99),
            "the opaque half was not drawn"
        );
        for p in &px {
            assert!(p[0] <= p[1] + 0.02, "the hidden red leaked: {p:?}");
        }
        // The logo is 80×40 here, half of it a hole: about 1600 pixels move, plus the few columns
        // the resampling softens at the join. All 3200 would mean the hole was drawn.
        let n = changed(&img, &base).len();
        assert!((1500..=1920).contains(&n), "{n} pixels changed");
    }

    #[test]
    fn a_half_transparent_logo_blends_by_its_alpha() {
        // ⚠⚠ Found by sabotage: with `k = op` — the alpha ignored — every other test here stayed
        // green, because a pixel with no alpha is skipped before `k` is read and an opaque one has
        // `a = 1`. Only an alpha strictly between the two can tell them apart.
        let logo = Logo::decode(&png(40, 20, |_, _| [255, 255, 255, 128]), OutputSpace::Srgb)
            .expect("decode");
        let mut img = grey(400, 300, 0.5);
        apply(
            &mut img,
            &mark(20, 100, WatermarkCorner::BottomRight),
            Some(&logo),
        );
        let a = 128.0 / 255.0;
        let want = 0.5 * (1.0 - a) + a;
        let brightest = img.data().iter().copied().fold(0.0f32, f32::max);
        assert!((brightest - want).abs() < 0.01, "{brightest}, want {want}");
    }

    #[test]
    fn opacity_scales_the_blend() {
        let logo = white(40, 20);
        let brightest = |o: u8| {
            let mut img = grey(400, 300, 0.5);
            apply(
                &mut img,
                &mark(20, o, WatermarkCorner::BottomRight),
                Some(&logo),
            );
            img.data().iter().copied().fold(0.0f32, f32::max)
        };
        let (full, half) = (brightest(100), brightest(50));
        assert!((full - 1.0).abs() < 0.01, "full {full}");
        assert!((half - 0.75).abs() < 0.01, "half {half}");
    }

    #[test]
    fn the_width_follows_the_long_edge() {
        let logo = white(40, 20);
        let span = |w: usize, h: usize| {
            let base = grey(w, h, 0.5);
            let mut img = base.clone();
            apply(
                &mut img,
                &mark(20, 100, WatermarkCorner::TopLeft),
                Some(&logo),
            );
            let cols: Vec<usize> = changed(&img, &base).iter().map(|p| p.0).collect();
            assert!(!cols.is_empty(), "{w}x{h}: nothing was drawn");
            cols.iter().max().unwrap() - cols.iter().min().unwrap() + 1
        };
        for (w, h, want) in [(400, 300, 80), (800, 600, 160), (300, 400, 80)] {
            let got = span(w, h);
            assert!(
                got.abs_diff(want) <= 1,
                "{w}x{h}: {got} columns, want {want}"
            );
        }
    }

    #[test]
    fn the_logo_lands_in_its_corner() {
        let logo = white(40, 20);
        let base = grey(400, 300, 0.5);
        for corner in WatermarkCorner::ALL {
            let mut img = base.clone();
            apply(&mut img, &mark(20, 100, corner), Some(&logo));
            let moved = changed(&img, &base);
            assert!(!moved.is_empty(), "{corner:?}: nothing was drawn");
            let (want_left, want_top) = match corner {
                WatermarkCorner::TopLeft => (true, true),
                WatermarkCorner::TopRight => (false, true),
                WatermarkCorner::BottomLeft => (true, false),
                WatermarkCorner::BottomRight => (false, false),
            };
            for (x, y) in moved {
                assert_eq!(x < 200, want_left, "{corner:?}: ink at ({x},{y})");
                assert_eq!(y < 150, want_top, "{corner:?}: ink at ({x},{y})");
            }
        }
    }

    #[test]
    fn a_p3_export_moves_red_by_the_matrix() {
        let red = png(1, 1, |_, _| [255, 0, 0, 255]);
        let srgb = Logo::decode(&red, OutputSpace::Srgb)
            .expect("decode")
            .pixel(0, 0);
        assert_eq!(srgb, [1.0, 0.0, 0.0, 1.0], "sRGB is the identity");
        let p3 = Logo::decode(&red, OutputSpace::DisplayP3)
            .expect("decode")
            .pixel(0, 0);
        // ⚠ Differ first — the comparison below would also pass if both were left as sRGB red.
        let moved = (p3[0] - 1.0).abs() + p3[1] + p3[2];
        assert!(moved > 0.05, "P3 left sRGB red alone: {p3:?}");
        let want = OutputSpace::DisplayP3.convert_srgb8([255, 0, 0]);
        for c in 0..3 {
            let w = f32::from(want[c]) / 255.0;
            assert!((p3[c] - w).abs() <= 1.0 / 255.0, "{p3:?} against {want:?}");
        }
        assert_eq!(p3[3], 1.0);
    }

    #[test]
    fn a_logo_that_does_not_fit_is_skipped() {
        // 50% of a 40 px long edge is 20 px wide, so a 1:4 logo is 80 px tall — taller than 30.
        let logo = white(10, 40);
        let base = grey(40, 30, 0.5);
        let mut img = base.clone();
        apply(
            &mut img,
            &mark(50, 100, WatermarkCorner::BottomRight),
            Some(&logo),
        );
        assert_eq!(img.data(), base.data());
    }

    #[test]
    fn a_broken_file_is_an_error() {
        assert!(Logo::decode(b"not an image", OutputSpace::Srgb).is_err());
        let whole = png(40, 20, |_, _| [255; 4]);
        assert!(Logo::decode(&whole[..whole.len() / 2], OutputSpace::Srgb).is_err());
    }

    #[test]
    fn a_huge_logo_is_shrunk_once() {
        let logo = white(5000, 10);
        assert_eq!(logo.width(), MAX_LOGO as usize);
        assert_eq!(logo.height(), 8);
    }

    #[test]
    fn each_kind_draws_only_itself() {
        let logo = white(40, 20);
        let base = grey(400, 300, 0.5);
        // An image mark with no logo to hand — a missing copy — draws nothing.
        let mut img = base.clone();
        apply(&mut img, &mark(20, 100, WatermarkCorner::TopLeft), None);
        assert_eq!(img.data(), base.data());
        // A text mark with no text draws nothing, whatever logo is passed.
        let mut img = base.clone();
        let text = Watermark {
            kind: WatermarkKind::Text,
            ..mark(20, 100, WatermarkCorner::TopLeft)
        };
        apply(&mut img, &text, Some(&logo));
        assert_eq!(img.data(), base.data());
    }
}
