//! M9.13 probe: the real AI mask networks on a real photo, timed, with the mask saved beside it.
//!
//! `cargo run --release -p app --example ai_mask_probe -- <photo.jpg> <models dir> subject [isnet|birefnet]`
//! `cargo run --release -p app --example ai_mask_probe -- <photo.jpg> <models dir> click <x> <y>`
//!
//! `x`/`y` are normalised over the photo as displayed. Writes `<photo stem>.mask.png` (the map)
//! and `<photo stem>.overlay.png` (the photo with the selection tinted red) to the current
//! directory. The models directory holds the files `ai_mask::ModelId::spec().file` names.

use ai_mask::{ModelId, ProbMap, Sam, Subject, SubjectModel};
use std::path::PathBuf;
use std::time::Instant;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let photo = PathBuf::from(&a[1]);
    let models = PathBuf::from(&a[2]);
    // ⚠ Upright, as the app hands it: EXIF orientation applied by `image_decode`'s own loader path.
    let mut dec = image::ImageReader::open(&photo)
        .unwrap()
        .into_decoder()
        .unwrap();
    let o = image::ImageDecoder::orientation(&mut dec).unwrap();
    let mut img = image::DynamicImage::from_decoder(dec).unwrap();
    img.apply_orientation(o);
    let rgb = img.to_rgb8();
    let small = image::imageops::resize(
        &rgb,
        (rgb.width() as f32 * 1024.0 / rgb.width().max(rgb.height()) as f32) as u32,
        (rgb.height() as f32 * 1024.0 / rgb.width().max(rgb.height()) as f32) as u32,
        image::imageops::FilterType::Triangle,
    );
    let t = Instant::now();
    let map: ProbMap = match a[3].as_str() {
        "subject" => {
            let kind = match a.get(4).map(String::as_str) {
                Some("birefnet") => SubjectModel::BiRefNetLite,
                _ => SubjectModel::IsNet,
            };
            let s = Subject::load(&models.join(kind.model().spec().file), kind).unwrap();
            println!("load {:.2} s", t.elapsed().as_secs_f64());
            let t = Instant::now();
            let m = s.run(&small).unwrap();
            println!("run {:.2} s", t.elapsed().as_secs_f64());
            m
        }
        "click" => {
            let p = [a[4].parse().unwrap(), a[5].parse().unwrap()];
            let mut sam = Sam::new(
                &models.join(ModelId::SamEncoder.spec().file),
                &models.join(ModelId::SamDecoder.spec().file),
            );
            let e = sam.embed(&small).unwrap();
            println!("embed {:.2} s", t.elapsed().as_secs_f64());
            let t = Instant::now();
            let m = sam.click(&e, p).unwrap();
            println!("click {:.3} s (first, includes the decoder build)", t.elapsed().as_secs_f64());
            let t = Instant::now();
            let _ = sam.click(&e, p).unwrap();
            println!("click {:.3} s (second)", t.elapsed().as_secs_f64());
            m
        }
        other => panic!("subject | click, not {other}"),
    };
    let stem = photo.file_stem().unwrap().to_string_lossy().into_owned();
    image::GrayImage::from_raw(map.w, map.h, map.data.clone())
        .unwrap()
        .save(format!("{stem}.mask.png"))
        .unwrap();
    let overlay = image::RgbImage::from_fn(small.width(), small.height(), |x, y| {
        let c = map.at(
            (x as f32 + 0.5) / small.width() as f32,
            (y as f32 + 0.5) / small.height() as f32,
        );
        let p = small.get_pixel(x, y).0;
        image::Rgb([
            (p[0] as f32 * (1.0 - 0.5 * c) + 255.0 * 0.5 * c) as u8,
            (p[1] as f32 * (1.0 - 0.5 * c)) as u8,
            (p[2] as f32 * (1.0 - 0.5 * c)) as u8,
        ])
    });
    overlay.save(format!("{stem}.overlay.png")).unwrap();
    let covered = map.data.iter().filter(|&&v| v > 128).count() as f32 / map.data.len() as f32;
    println!("covered {:.1}% — wrote {stem}.mask.png and {stem}.overlay.png", covered * 100.0);
}
