//! A contact sheet for judging the display rendering: for each frame, the colorimetric render,
//! the camera rendering, and the camera's own JPEG, side by side at the same height.
//!
//! ```text
//! cargo run --release -p app --example rendering_sheet <out-dir> <raw>...
//! ```

const H: u32 = 620;

fn strip(img: &image::DynamicImage) -> image::RgbImage {
    let w = (img.width() as f32 * H as f32 / img.height() as f32).round() as u32;
    image::imageops::resize(&img.to_rgb8(), w, H, image::imageops::FilterType::Lanczos3)
}

fn main() {
    let mut args = std::env::args().skip(1);
    let out = args
        .next()
        .expect("usage: rendering_sheet <out-dir> <raw>...");
    let out = std::path::PathBuf::from(out);
    std::fs::create_dir_all(&out).unwrap();

    for f in args {
        let path = std::path::PathBuf::from(&f);
        let stem = path.file_stem().unwrap().to_string_lossy().to_string();
        let thumb = raw_decode::extract_thumbnail(&path).expect("thumb");
        let d = raw_decode::decode_raw(&path, &raw_decode::DecodeOptions::linear_scene())
            .expect("decode");
        let small = d.image.downscale_to(1600);

        let mut panels = Vec::new();
        for r in [edit_model::Rendering::None, edit_model::Rendering::Camera] {
            let edits = edit_model::Edits {
                rendering: Some(r),
                ..Default::default()
            };
            let px = pipeline::build_pipeline(
                &edits,
                pipeline::RenderScale::new(1.0),
                pipeline::ProfileRendering::colorimetric(),
            )
            .run(small.clone());
            let rgb = export::to_rgb8(&px);
            let img = image::RgbImage::from_raw(px.width() as u32, px.height() as u32, rgb)
                .expect("panel");
            panels.push(strip(&image::DynamicImage::ImageRgb8(img)));
        }
        // The camera's own JPEG, the thing the other two are being judged against.
        let cam = image::RgbaImage::from_raw(thumb.width as u32, thumb.height as u32, thumb.rgba)
            .expect("thumb buffer");
        panels.push(strip(&image::DynamicImage::ImageRgba8(cam)));

        let gap = 12u32;
        let total: u32 = panels.iter().map(|p| p.width()).sum::<u32>() + gap * 2;
        let mut sheet = image::RgbImage::from_pixel(total, H, image::Rgb([24, 24, 24]));
        let mut x = 0;
        for p in &panels {
            image::imageops::overlay(&mut sheet, p, x as i64, 0);
            x += p.width() + gap;
        }
        let dst = out.join(format!("{stem}.jpg"));
        sheet.save(&dst).unwrap();
        println!(
            "{}  (colorimetric | camera rendering | camera JPEG)",
            dst.display()
        );
    }
}
