//! Remove dust on real photos (M10.16c2): `cargo run --release -p app --example dust_probe -- OUT RAF…`.
//!
//! For each RAF: the finds at Sensitivity 30, 50 and 70 with the time, an overview of the photo with every
//! find at 50 circled (`OUT/<name>_where.png`), and a sheet of the finds magnified and stretched, so faint
//! dust shows (`OUT/<name>_dust.png`). The default sensitivity is the author's call from these.

use core_image::ImageBuf;
use rustroom_app::ai_denoise::Encoding;
use rustroom_app::dust_tool::{find, Find};

fn decode(path: &str) -> ImageBuf {
    raw_decode::decode_raw(
        std::path::Path::new(path),
        &raw_decode::DecodeOptions::linear_scene(),
    )
    .expect("decode")
    .image
}

/// The photo, display-encoded and downscaled to `long`, every find circled in red.
fn overview(img: &ImageBuf, enc: &Encoding, finds: &[Find], long: usize) -> image::RgbImage {
    let small = img.downscale_to(long);
    let (w, h) = (small.width(), small.height());
    let mut out = image::RgbImage::from_fn(w as u32, h as u32, |x, y| {
        let p = enc.encode(small.pixel(x as usize, y as usize));
        image::Rgb(p.map(|v| (v * 255.0).round() as u8))
    });
    for f in finds {
        let (cx, cy) = (f.centre[0] * w as f32, f.centre[1] * h as f32);
        let r = (f.radius * w.max(h) as f32 * 2.5).max(6.0);
        for k in 0..96 {
            let a = k as f32 / 96.0 * std::f32::consts::TAU;
            let (x, y) = ((cx + r * a.cos()) as i64, (cy + r * a.sin()) as i64);
            if x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h {
                out.put_pixel(x as u32, y as u32, image::Rgb([255, 40, 40]));
            }
        }
    }
    out
}

/// Each find's surroundings — eight radii across, at least 48 px — magnified to `cell` and stretched to its
/// own range, so a few percent of darkening is visible.
fn sheet(img: &ImageBuf, finds: &[Find], cell: u32) -> image::RgbImage {
    let (w, h) = (img.width(), img.height());
    let cols = 6u32;
    let n = finds.len().min(24) as u32;
    let rows = n.div_ceil(cols).max(1);
    let mut out = image::RgbImage::from_pixel(cols * cell, rows * cell, image::Rgb([30, 30, 30]));
    for (k, f) in finds.iter().take(24).enumerate() {
        let side = (f.radius * w.max(h) as f32 * 8.0).max(48.0) as usize;
        let (cx, cy) = (f.centre[0] * w as f32, f.centre[1] * h as f32);
        let x0 = (cx as usize)
            .saturating_sub(side / 2)
            .min(w.saturating_sub(side));
        let y0 = (cy as usize)
            .saturating_sub(side / 2)
            .min(h.saturating_sub(side));
        let lum: Vec<f32> = (0..side * side)
            .map(|i| color::luminance(img.pixel(x0 + i % side, y0 + i / side)))
            .collect();
        let (lo, hi) = lum
            .iter()
            .fold((f32::MAX, f32::MIN), |(a, b), &v| (a.min(v), b.max(v)));
        let span = (hi - lo).max(1e-6);
        let (ox, oy) = ((k as u32 % cols) * cell, (k as u32 / cols) * cell);
        for y in 0..cell {
            for x in 0..cell {
                let (sx, sy) = (
                    (x as usize * side / cell as usize).min(side - 1),
                    (y as usize * side / cell as usize).min(side - 1),
                );
                let v = ((lum[sy * side + sx] - lo) / span * 255.0) as u8;
                out.put_pixel(ox + x, oy + y, image::Rgb([v, v, v]));
            }
        }
    }
    out
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = std::path::PathBuf::from(&args[0]);
    std::fs::create_dir_all(&out).expect("out dir");
    for path in &args[1..] {
        let name = std::path::Path::new(path)
            .file_stem()
            .unwrap()
            .to_string_lossy()
            .to_string();
        let img = decode(path);
        let mut line = format!("{name} ({} × {}):", img.width(), img.height());
        for s in [30.0, 50.0, 70.0] {
            let t = std::time::Instant::now();
            let n = find(&img, s).len();
            line += &format!(" {s:.0} → {n} ({:.2} s)", t.elapsed().as_secs_f64());
        }
        println!("{line}");
        let finds = find(&img, 50.0);
        for f in &finds {
            println!(
                "  find {:.4} {:.4} r {:.1} px c {:.3}",
                f.centre[0],
                f.centre[1],
                f.radius * img.width().max(img.height()) as f32,
                f.contrast
            );
        }
        let enc = Encoding::of(&img);
        overview(&img, &enc, &finds, 1600)
            .save(out.join(format!("{name}_where.png")))
            .expect("overview");
        sheet(&img, &finds, 160)
            .save(out.join(format!("{name}_dust.png")))
            .expect("sheet");
    }
}
