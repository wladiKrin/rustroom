//! The merged photo rendered for a person to look at — the dialog's preview and the DNG's own.
//!
//! ⚠ Through the develop pipeline with the edits the photo will open with, so the preview is what
//! Develop will show (bar lens corrections, which need the lens profile the app holds).

use crate::deghost::Mask;
use core_image::ImageBuf;

/// `prophoto` (linear ProPhoto, at `source_scale` of a full-size decode) rendered through `edits` at
/// `long_edge` px, 8-bit sRGB. `as_shot` is the light the white balance starts from.
pub fn render(
    prophoto: &ImageBuf,
    edits: &edit_model::Edits,
    long_edge: usize,
    source_scale: f32,
    as_shot: color::wb::Illuminant,
) -> image::RgbImage {
    let small = prophoto.downscale_to(long_edge);
    let long = |i: &ImageBuf| i.width().max(i.height()).max(1) as f32;
    let scale = pipeline::RenderScale::new(long(&small) / long(prophoto) * source_scale);
    let spec = pipeline::RenderSpec {
        baseline: as_shot,
        airlight: Some(pipeline::prefix_airlight(
            edits,
            prophoto,
            pipeline::Resampler::default(),
            None,
        )),
        ..pipeline::RenderSpec::default()
    };
    let out = pipeline::build_pipeline_spec(edits, scale, spec, pipeline::TailRendering::default())
        .run(small);
    let (w, h) = (out.width() as u32, out.height() as u32);
    image::RgbImage::from_raw(w, h, out.to_rgb8()).expect("sized")
}

/// Red over what deghosting took from one frame alone — 45 % at full strength.
pub fn tint(img: &mut image::RgbImage, overlay: &Mask) {
    let (w, h) = (img.width() as f32, img.height() as f32);
    for (x, y, p) in img.enumerate_pixels_mut() {
        let m = overlay
            .at((x as f32 + 0.5) / w, (y as f32 + 0.5) / h)
            .clamp(0.0, 1.0)
            * 0.45;
        if m > 0.0 {
            let red = [235.0, 40.0, 40.0];
            for c in 0..3 {
                p[c] = (p[c] as f32 * (1.0 - m) + red[c] * m).round() as u8;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_image::ImageBuf;

    #[test]
    fn a_brighter_scene_renders_brighter_and_the_size_is_bounded() {
        let mut img = ImageBuf::new(400, 100);
        for y in 0..100 {
            for x in 0..400 {
                img.set_pixel(x, y, [0.002 * 2f32.powf(x as f32 / 40.0); 3]);
            }
        }
        let out = render(
            &img,
            &edit_model::Edits::default(),
            200,
            1.0,
            color::wb::Illuminant::default(),
        );
        assert_eq!((out.width(), out.height()), (200, 50));
        let g = |x: u32| out.get_pixel(x, 25)[1];
        assert!(
            g(10) < g(100) && g(100) < g(190),
            "{} {} {}",
            g(10),
            g(100),
            g(190)
        );
    }

    #[test]
    fn the_overlay_tints_red_where_it_is_set() {
        let mut img = image::RgbImage::from_pixel(8, 8, image::Rgb([100, 100, 100]));
        let mask = crate::deghost::Mask {
            w: 2,
            h: 2,
            data: vec![1.0, 0.0, 0.0, 0.0],
        };
        tint(&mut img, &mask);
        let (a, b) = (img.get_pixel(0, 0), img.get_pixel(7, 7));
        assert!(a[0] > 150 && a[1] < 100, "{a:?}");
        assert_eq!(*b, image::Rgb([100, 100, 100]));
    }
}
