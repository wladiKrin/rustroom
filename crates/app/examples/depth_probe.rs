//! M10.16a's spike, kept: Depth Anything V2 Small through `tract` on real photos — the time to build a
//! size's plan, the time to run it, and the map as a PNG (near white) beside a copy of the photo.
//!
//! ```text
//! cargo run --release --example depth_probe -- <out dir> <long side> <photo>…
//! cargo run --release --example depth_probe -- <out dir> sheet <photo>…
//! ```
//! `sheet` (Task 11) writes `<stem>_sheet.png` per photo, for the author's choices — row A the scale
//! (the photo, linear disparity, equalised), row B the edges (a crop of the photo's centre, then the
//! map there unsnapped, snapped at radius 8 and 16), row C the stored size (1024, 1536, 2048, each
//! snapped at a radius in proportion) — all from one inference at `depth_map::RECIPE.infer_long`.
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

/// The map as a grey image, near white.
fn grey(m: &ai_mask::ProbMap) -> image::RgbImage {
    image::RgbImage::from_fn(m.w, m.h, |x, y| {
        let g = 255 - m.data[(y * m.w + x) as usize];
        image::Rgb([g, g, g])
    })
}

/// `img`'s central crop — a third of each side — scaled to `side` wide.
fn centre_crop(img: &image::RgbImage, side: u32) -> image::RgbImage {
    let (w, h) = (img.width() / 3, img.height() / 3);
    let crop = image::imageops::crop_imm(img, w, h, w, h).to_image();
    let tall = (side as f32 * h as f32 / w as f32).round() as u32;
    image::imageops::resize(&crop, side, tall, image::imageops::FilterType::Nearest)
}

fn fit(img: &image::RgbImage, side: u32) -> image::RgbImage {
    let k = side as f32 / img.width().max(img.height()) as f32;
    let (w, h) = (
        (img.width() as f32 * k) as u32,
        (img.height() as f32 * k) as u32,
    );
    image::imageops::resize(img, w, h, image::imageops::FilterType::Triangle)
}

fn sheet(out: &std::path::Path, photos: &[String], depth: &mut ai_mask::depth::Depth) {
    use ai_mask::depth::DepthScale;
    use image::GenericImage;
    let r = rustroom_app::depth_map::RECIPE;
    for photo in photos {
        let img = upright(photo);
        let at = |long: u32| fit(&img, long);
        let raw = depth.run(&at(r.store_long), r.infer_long).expect("run");
        let finish = |scale, store: u32, radius| {
            let pos = ai_mask::depth::normalise(&raw, scale);
            let recipe = rustroom_app::depth_map::Recipe {
                store_long: store,
                radius,
                scale,
                ..r
            };
            grey(&rustroom_app::depth_map::finish(&pos, &at(store), &recipe))
        };
        let cell = 420u32;
        let row_a = [
            fit(&img, cell),
            fit(&finish(DepthScale::Linear, 1536, 8), cell),
            fit(&finish(DepthScale::Equalised, 1536, 8), cell),
        ];
        let row_b = [
            centre_crop(&at(1536), cell),
            centre_crop(&finish(r.scale, 1536, 0), cell),
            centre_crop(&finish(r.scale, 1536, 8), cell),
            centre_crop(&finish(r.scale, 1536, 16), cell),
        ];
        let row_c = [
            centre_crop(&finish(r.scale, 1024, 5), cell),
            centre_crop(&finish(r.scale, 1536, 8), cell),
            centre_crop(&finish(r.scale, 2048, 11), cell),
        ];
        let rows = [&row_a[..], &row_b[..], &row_c[..]];
        let height: u32 = rows
            .iter()
            .map(|r| r.iter().map(|i| i.height()).max().unwrap_or(0) + 8)
            .sum();
        let mut sheet =
            image::RgbImage::from_pixel(cell * 4 + 24, height, image::Rgb([40, 40, 40]));
        let mut y = 0;
        for row in rows {
            for (i, tile) in row.iter().enumerate() {
                sheet
                    .copy_from(tile, i as u32 * (cell + 8), y)
                    .expect("fits");
            }
            y += row.iter().map(|i| i.height()).max().unwrap_or(0) + 8;
        }
        let stem = std::path::Path::new(photo)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("photo");
        sheet
            .save(out.join(format!("{stem}_sheet.png")))
            .expect("save");
        println!("{photo}: sheet written");
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(2).map(String::as_str) == Some("sheet") {
        let models = rustroom_app::ai_cache::models_dir().expect("models dir");
        let file = models.join(ai_mask::ModelId::DepthAnythingV2Small.spec().file);
        let mut depth = ai_mask::depth::Depth::load(&file).expect("load");
        return sheet(std::path::Path::new(&args[1]), &args[3..], &mut depth);
    }
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
