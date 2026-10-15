//! M10.16b's spike, kept: AI denoise on real RAFs — the automatic σ against sheets at σ 4 · 6 · 8 · 10 ·
//! 12 (a shadow and a detail crop each, 100 % and the centre at 200 %, a green bar under the column
//! nearest the estimate), one full frame's time, and the cache file's size.
//!
//!
//! `calibrate` (M10.16b's Task 10): the same sheets with the columns at 0.5 · 0.75 · 1 · 1.5 · 2 × each
//! photo's own estimate, the green bar under 1 × — the author's look sets `ai_denoise::CALIBRATION`.
//!
//! ```text
//! cargo run --release --example denoise_probe -- <out dir> <drunet.onnx> <raf>…
//! cargo run --release --example denoise_probe -- calibrate <out dir> <drunet.onnx> <raf>…
//! cargo run --release --example denoise_probe -- frame <out dir> <drunet.onnx> <raf>
//! ```

use rustroom_app::ai_denoise::{denoise, denoise_in, noise_sigma, Encoding};

const SIDE: usize = 512;
const SIGMAS: [f32; 5] = [4.0, 6.0, 8.0, 10.0, 12.0];
/// `calibrate`'s columns, as multiples of the estimate.
const FACTORS: [f32; 5] = [0.5, 0.75, 1.0, 1.5, 2.0];

fn decode(path: &str) -> core_image::ImageBuf {
    raw_decode::decode_raw(
        std::path::Path::new(path),
        &raw_decode::DecodeOptions::linear_scene(),
    )
    .expect("decode")
    .image
}

/// The darkest window above black and the busiest one, on a 256 grid, in the encoding.
fn windows(img: &core_image::ImageBuf, enc: &Encoding) -> [(usize, usize); 2] {
    let (w, h) = (img.width(), img.height());
    let mut wins = Vec::new();
    for y0 in (0..h.saturating_sub(SIDE)).step_by(256) {
        for x0 in (0..w.saturating_sub(SIDE)).step_by(256) {
            let (mut sum, mut grad, mut n) = (0.0f32, 0.0f32, 0.0f32);
            for y in (y0..y0 + SIDE).step_by(8) {
                for x in (x0..x0 + SIDE - 8).step_by(8) {
                    let a = enc.encode(img.pixel(x, y));
                    let b = enc.encode(img.pixel(x + 4, y));
                    sum += a[1];
                    grad += (a[1] - b[1]).abs();
                    n += 1.0;
                }
            }
            wins.push((x0, y0, sum / n, grad / n));
        }
    }
    let dark = wins
        .iter()
        .filter(|w| w.2 > 0.05)
        .min_by(|a, b| a.2.total_cmp(&b.2))
        .copied()
        .expect("a dark window");
    let busy = wins
        .iter()
        .max_by(|a, b| a.3.total_cmp(&b.3))
        .copied()
        .expect("a busy window");
    [(dark.0, dark.1), (busy.0, busy.1)]
}

/// A crop as the network sees it, 8-bit.
fn shown(img: &core_image::ImageBuf, enc: &Encoding) -> image::RgbImage {
    image::RgbImage::from_fn(img.width() as u32, img.height() as u32, |x, y| {
        let e = enc.encode(img.pixel(x as usize, y as usize));
        image::Rgb(e.map(|v| (v * 255.0).round() as u8))
    })
}

/// `relative`: the columns at [`FACTORS`] × the estimate, not at [`SIGMAS`].
fn sheets(out: &std::path::Path, model: &str, rafs: &[String], relative: bool) {
    let mut net = ai_mask::denoise::Drunet::load(std::path::Path::new(model)).expect("model");
    for raf in rafs {
        let stem = std::path::Path::new(raf)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("photo")
            .to_string();
        let img = decode(raf);
        let enc = Encoding::of(&img);
        let t = std::time::Instant::now();
        let est = noise_sigma(&img, &enc);
        println!(
            "{stem}: estimate σ {est:.2} ({:.0} ms), encoding scale {:.3}",
            t.elapsed().as_secs_f64() * 1000.0,
            enc.scale
        );
        let sigmas: Vec<f32> = if relative {
            FACTORS.iter().map(|f| (f * est).clamp(1.0, 40.0)).collect()
        } else {
            SIGMAS.to_vec()
        };
        println!("  columns after the input: σ {sigmas:.2?}");
        let cols = sigmas.len() as u32 + 1;
        let cell = SIDE as u32 + 6;
        let mut sheet =
            image::RgbImage::from_pixel(cols * cell, 4 * cell + 16, image::Rgb([40, 40, 40]));
        let nearest = sigmas
            .iter()
            .enumerate()
            .min_by(|a, b| (a.1 - est).abs().total_cmp(&(b.1 - est).abs()))
            .map(|(i, _)| i + 1)
            .expect("sigmas");
        for (row, (x0, y0)) in windows(&img, &enc).into_iter().enumerate() {
            let crop = img.crop(x0, y0, SIDE, SIDE);
            let mut variants = vec![shown(&crop, &enc)];
            for &s in &sigmas {
                let d = denoise_in(&crop, &enc, &mut net, s, &|_, _| true)
                    .expect("run")
                    .expect("not cancelled");
                variants.push(shown(&d, &enc));
            }
            for (c, v) in variants.iter().enumerate() {
                let x = c as u32 * cell;
                let y = row as u32 * 2 * cell;
                image::imageops::overlay(&mut sheet, v, x as i64, y as i64);
                let q = SIDE as u32 / 4;
                let zoom = image::imageops::resize(
                    &image::imageops::crop_imm(v, q, q, 2 * q, 2 * q).to_image(),
                    SIDE as u32,
                    SIDE as u32,
                    image::imageops::FilterType::Nearest,
                );
                image::imageops::overlay(&mut sheet, &zoom, x as i64, (y + cell) as i64);
            }
        }
        // The green bar under the column nearest the estimate.
        for x in nearest as u32 * cell..nearest as u32 * cell + SIDE as u32 {
            for y in 4 * cell + 4..4 * cell + 14 {
                sheet.put_pixel(x, y, image::Rgb([60, 200, 90]));
            }
        }
        let suffix = if relative { "calibrate" } else { "sigma" };
        sheet
            .save(out.join(format!("{stem}_{suffix}.png")))
            .expect("sheet");
    }
}

fn frame(out: &std::path::Path, model: &str, raf: &str) {
    let mut net = ai_mask::denoise::Drunet::load(std::path::Path::new(model)).expect("model");
    let img = decode(raf);
    let enc = Encoding::of(&img);
    let sigma = noise_sigma(&img, &enc);
    println!("{raf}: {}x{}, σ {sigma:.2}", img.width(), img.height());
    let t = std::time::Instant::now();
    let done = denoise(&img, &mut net, sigma, &|d, n| {
        eprintln!("  tile {d}/{n}  {:.0} s", t.elapsed().as_secs_f64());
        true
    })
    .expect("run")
    .expect("not cancelled");
    println!("a full frame: {:.1} s", t.elapsed().as_secs_f64());
    let t = std::time::Instant::now();
    rustroom_app::denoise_cache::store(out, "probe", &done).expect("store");
    let bytes = std::fs::metadata(out.join("probe.rrdn"))
        .expect("file")
        .len();
    println!(
        "the cache file: {:.1} MB, written in {:.1} s",
        bytes as f64 / 1e6,
        t.elapsed().as_secs_f64()
    );
    let t = std::time::Instant::now();
    let back = rustroom_app::denoise_cache::load(out, "probe").expect("load");
    println!(
        "read back in {:.1} s ({}x{})",
        t.elapsed().as_secs_f64(),
        back.width(),
        back.height()
    );
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args[1] == "sigma" {
        // ⚠ Spike: the estimate at three scales, to see which tracks the eye.
        for raf in &args[2..] {
            let img = decode(raf);
            let enc = Encoding::of(&img);
            let at = |b| rustroom_app::ai_denoise::noise_sigma_at(&img, &enc, b);
            println!(
                "{raf}: σ at bin 1 {:.2} · bin 2 {:.2} · bin 4 {:.2}",
                at(1),
                at(2),
                at(4)
            );
        }
        return;
    }
    if args[1] == "frame" {
        let out = std::path::PathBuf::from(&args[2]);
        std::fs::create_dir_all(&out).expect("out");
        return frame(&out, &args[3], &args[4]);
    }
    if args[1] == "calibrate" {
        let out = std::path::PathBuf::from(&args[2]);
        std::fs::create_dir_all(&out).expect("out");
        return sheets(&out, &args[3], &args[4..], true);
    }
    let out = std::path::PathBuf::from(&args[1]);
    std::fs::create_dir_all(&out).expect("out");
    sheets(&out, &args[2], &args[3..], false);
}
