//! M10.16a's spike, kept: Depth Anything V2 Small through `tract` on real photos — the time to build a
//! size's plan, the time to run it, and the map as a PNG (near white) beside a copy of the photo.
//!
//! ```text
//! cargo run --release --example depth_probe -- <out dir> <long side> <photo>…
//! ```
//! One long side per process, so `/usr/bin/time -l` reports that size's peak memory. The photos are
//! read upright (their EXIF orientation applied), as the app shows the network every photo.

use image::ImageDecoder;

fn upright(path: &str) -> image::RgbImage {
    let mut dec = image::ImageReader::open(path)
        .expect("open")
        .into_decoder()
        .expect("decoder");
    let o = dec.orientation().expect("orientation");
    let mut img = image::DynamicImage::from_decoder(dec).expect("decode");
    img.apply_orientation(o);
    img.to_rgb8()
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (out, long) = (
        std::path::PathBuf::from(&args[1]),
        args[2].parse::<u32>().expect("long"),
    );
    let models = rustroom_app::ai_cache::models_dir().expect("models dir");
    let file = models.join(ai_mask::ModelId::DepthAnythingV2Small.spec().file);
    let t = std::time::Instant::now();
    let mut depth = ai_mask::depth::Depth::load(&file).expect("load");
    println!("load {:.2} s", t.elapsed().as_secs_f64());
    for photo in &args[3..] {
        let img = upright(photo);
        // The first run builds the size's plan; the second is the steady state.
        let t = std::time::Instant::now();
        let raw = depth.run(&img, long).expect("run");
        let first = t.elapsed().as_secs_f64();
        let t = std::time::Instant::now();
        let _ = depth.run(&img, long).expect("run");
        let second = t.elapsed().as_secs_f64();
        println!(
            "{photo}: {}×{} first {first:.2} s, again {second:.2} s",
            raw.w, raw.h
        );
        let p = ai_mask::depth::normalise(&raw, ai_mask::depth::DepthScale::Linear);
        let stem = std::path::Path::new(photo)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("photo");
        image::GrayImage::from_fn(p.w, p.h, |x, y| {
            image::Luma([((1.0 - p.data[(y * p.w + x) as usize]) * 255.0).round() as u8])
        })
        .save(out.join(format!("{stem}_depth_{long}.png")))
        .expect("save");
        image::imageops::resize(&img, p.w, p.h, image::imageops::FilterType::Triangle)
            .save(out.join(format!("{stem}_{long}.png")))
            .expect("save");
    }
}
