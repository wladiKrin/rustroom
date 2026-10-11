//! How big one exported photo is rendered (M10.10b): the decode size that lands the **cropped**
//! picture on its target, and the last pixel or the enlargement after the render.
//!
//! ⚠⚠ **Predicted, not measured.** The final size comes from `pipeline::prefix_output_dims` — the
//! prefix steps' own answers, no pixels — so the decode can be shrunk *before* the pipeline, which is
//! what keeps a 2048 px export rendering 2048 px of work rather than all of a 26 MP frame.

use core_image::ImageBuf;
use edit_model::Edits;
use export::sizing::{OutputSize, SizePlan};

/// The size one photo is decoded at and comes out at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Sizing {
    /// The long edge the linear decode is shrunk to — the source's own when nothing is shrunk.
    pub(super) decode_long: usize,
    pub(super) plan: SizePlan,
    /// The render is enlarged after the pipeline to reach the plan.
    pub(super) enlarge: bool,
}

impl Sizing {
    /// The render scale for a source whose long edge is `source_long` — what the spatial stages
    /// multiply their radii by, so a shrunk export is not given full-resolution radii.
    pub(super) fn render_scale(&self, source_long: usize) -> pipeline::RenderScale {
        pipeline::RenderScale::new(self.decode_long as f32 / source_long.max(1) as f32)
    }
}

/// The sizing of a photo decoded at `source` (`w`, `h`) with `edits`, at `size`.
pub(super) fn for_photo(edits: &Edits, source: (usize, usize), size: &OutputSize) -> Sizing {
    let (sw, sh) = source;
    let source_long = sw.max(sh);
    let (fw, fh) = pipeline::prefix_output_dims(edits, sw, sh);
    let plan = size.plan(fw, fh);
    if plan.width >= fw || plan.height >= fh || source_long == 0 {
        return Sizing {
            decode_long: source_long,
            plan,
            enlarge: plan.width > fw || plan.height > fh,
        };
    }
    // ⚠ The smallest decode whose cropped output still covers the target, so the render is
    // trimmed rather than stretched. Crop bounds round, so the first guess can fall a pixel short;
    // the walk upward is a step or two, each a prediction rather than a render.
    let ratio = (plan.width as f64 / fw as f64).max(plan.height as f64 / fh as f64);
    let mut long = ((source_long as f64 * ratio) - 1e-9).ceil().max(1.0) as usize;
    while long < source_long {
        let (dw, dh) = ImageBuf::downscaled_dims(sw, sh, long);
        let (ow, oh) = pipeline::prefix_output_dims(edits, dw, dh);
        if ow >= plan.width && oh >= plan.height {
            break;
        }
        long += 1;
    }
    Sizing {
        decode_long: long.min(source_long),
        plan,
        enlarge: false,
    }
}

/// `rendered` brought to exactly the plan's size: a pixel or two trimmed from the middle, or enlarged
/// in linear light (`space` is what it is encoded in).
///
/// ⚠ A trim of more than two pixels is never taken — it would cut picture — and the render is
/// resampled instead; [`for_photo`]'s prediction makes that unreachable, and this keeps a wrong
/// prediction from becoming a cropped photograph.
pub(super) fn fit(rendered: ImageBuf, plan: &SizePlan, space: color::OutputSpace) -> ImageBuf {
    let (w, h) = (rendered.width(), rendered.height());
    let (tw, th) = (plan.width, plan.height);
    if (w, h) == (tw, th) {
        return rendered;
    }
    if w >= tw && h >= th && w - tw <= 2 && h - th <= 2 {
        return rendered.crop((w - tw) / 2, (h - th) / 2, tw, th);
    }
    export::sizing::resample_encoded(&rendered, space, tw, th)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::batch_export::fixtures::*;
    use crate::batch_export::{export_one, ExportOptions, Written};
    use export::sizing::{FitMode, LengthUnit};
    use export::ExportFormat;

    /// Export `src` with `edits` at `size` as a JPEG: the outcome, the bytes, and the size read back.
    fn export(
        tag: &str,
        src: ImageBuf,
        edits: &Edits,
        size: OutputSize,
    ) -> (Written, Vec<u8>, (u32, u32)) {
        let d = TempDir::new(tag);
        let o = ExportOptions {
            size,
            ..opts(&d, None)
        };
        let dest = d.0.join("a.jpg");
        let w = export_one(
            source(src),
            edits,
            &dest,
            &o,
            CPU,
            &export::FileMetadata::default(),
            None,
            None,
        )
        .expect("it exports");
        let bytes = std::fs::read(&dest).unwrap();
        let img = image::load_from_memory(&bytes).unwrap();
        (w, bytes, (img.width(), img.height()))
    }

    fn jfif_density(bytes: &[u8]) -> u16 {
        assert_eq!(&bytes[6..11], b"JFIF\0");
        assert_eq!(bytes[13], 1, "per inch");
        u16::from_be_bytes([bytes[14], bytes[15]])
    }

    #[test]
    fn a_print_size_gives_exact_pixels_and_carries_its_ppi() {
        let size = OutputSize {
            fit: FitMode::Dimensions,
            unit: LengthUnit::Cm,
            width: 10.0,
            height: 5.0,
            ppi: 50,
            ..OutputSize::default()
        };
        let (w, bytes, dims) = export("size_print", flat(600, 300), &Edits::default(), size);
        assert_eq!(dims, (197, 98));
        assert_eq!(jfif_density(&bytes), 50);
        assert!(!w.below_ppi);
    }

    #[test]
    fn a_quarter_turned_photo_is_boxed_as_it_is_seen() {
        let mut e = Edits::default();
        e.geometry.orientation.rotate = edit_model::Quarter::Cw90;
        let size = OutputSize {
            fit: FitMode::WidthHeight,
            width: 200.0,
            height: 100.0,
            ..OutputSize::default()
        };
        let (_, _, dims) = export("size_turned", flat(300, 600), &e, size);
        assert_eq!(dims, (200, 100));
    }

    #[test]
    fn enlarging_reaches_the_size() {
        let size = OutputSize {
            dont_enlarge: false,
            ..OutputSize::long_edge_px(300)
        };
        let (w, _, dims) = export("size_up", flat(100, 50), &Edits::default(), size);
        assert_eq!(dims, (300, 150));
        assert!(!w.below_ppi);
    }

    #[test]
    fn dont_enlarge_keeps_the_pixels_and_lowers_the_ppi() {
        let size = OutputSize {
            fit: FitMode::LongEdge,
            unit: LengthUnit::Cm,
            long_edge: 10.0,
            ppi: 50,
            ..OutputSize::default()
        };
        let (w, bytes, dims) = export("size_keep", flat(100, 50), &Edits::default(), size);
        assert_eq!(dims, (100, 50));
        assert!(w.below_ppi);
        assert_eq!(jfif_density(&bytes), 25);
    }

    #[test]
    fn a_dng_is_sized_and_carries_no_ppi() {
        let d = TempDir::new("size_dng");
        let o = ExportOptions {
            format: ExportFormat::Dng,
            size: OutputSize::long_edge_px(64),
            ..opts(&d, None)
        };
        let dest = d.0.join("a.dng");
        export_one(
            source(flat(128, 64)),
            &Edits::default(),
            &dest,
            &o,
            CPU,
            &export::FileMetadata::default(),
            None,
            None,
        )
        .unwrap();
        let decoded = raw_decode::decode_raw(&dest, &raw_decode::DecodeOptions::linear_scene())
            .expect("libraw opens it");
        assert_eq!((decoded.image.width(), decoded.image.height()), (64, 32));
    }

    #[test]
    fn the_decode_is_the_smallest_whose_output_covers_the_target() {
        let mut e = Edits::default();
        e.geometry.crop.right = 0.5;
        let s = for_photo(&e, (400, 200), &OutputSize::long_edge_px(100));
        assert_eq!((s.plan.width, s.plan.height), (100, 100));
        assert_eq!(s.decode_long, 200);
        assert!(!s.enlarge);
        let s = for_photo(&e, (400, 200), &OutputSize::default());
        assert_eq!(
            (s.decode_long, s.plan.width, s.plan.height),
            (400, 200, 200)
        );
        let up = OutputSize {
            dont_enlarge: false,
            ..OutputSize::long_edge_px(300)
        };
        let s = for_photo(&Edits::default(), (100, 50), &up);
        assert_eq!((s.decode_long, s.enlarge), (100, true));
    }

    #[test]
    fn fit_trims_a_pixel_from_the_middle_and_enlarges_in_linear_light() {
        let mut img = ImageBuf::new(102, 51);
        for y in 0..51 {
            for x in 0..102 {
                img.set_pixel(x, y, [x as f32 / 101.0, 0.5, 0.5]);
            }
        }
        let plan = |w, h| export::sizing::SizePlan {
            width: w,
            height: h,
            ppi: 300.0,
            below_ppi: false,
        };
        let trimmed = fit(img.clone(), &plan(100, 50), color::OutputSpace::Srgb);
        assert_eq!((trimmed.width(), trimmed.height()), (100, 50));
        assert_eq!(
            trimmed.pixel(0, 0)[0],
            1.0 / 101.0,
            "one column off each side"
        );
        let up = fit(img, &plan(204, 102), color::OutputSpace::Srgb);
        assert_eq!((up.width(), up.height()), (204, 102));
    }
}
