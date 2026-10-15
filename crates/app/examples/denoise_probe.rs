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
//! cargo run --release --example denoise_probe -- frame [--gpu] <out dir> <drunet.onnx> <raf>
//! cargo run --release --example denoise_probe -- parity <drunet.onnx> <raf>
//! cargo run --release --example denoise_probe -- strength <out dir> <drunet.onnx> <raf>…
//! ```
//!
//! `strength` (the author's call 2026-10-16, "measure, then I pick"): the noise against brightness, and a
//! dark, a mid-tone and a busy crop each through today's σ, 2× it, a per-brightness noise map and the map at 2×,
//! with the noise each leaves.
//!
//! M10.16b2: `frame --gpu` runs the job's own network (`denoised::network`, the GPU first); `parity`
//! denoises four 512 px tiles of a real frame on `tract` and on the GPU and prints how far apart they are
//! and how long each took.

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

fn frame(out: &std::path::Path, model: &str, raf: &str, gpu: bool) {
    let path = std::path::Path::new(model);
    let mut net: Box<dyn rustroom_app::ai_denoise::TileNet + Send> = if gpu {
        rustroom_app::denoised::network(path.parent().expect("a folder"), true).expect("network")
    } else {
        Box::new(ai_mask::denoise::Drunet::load(path).expect("model"))
    };
    let img = decode(raf);
    let enc = Encoding::of(&img);
    let sigma = noise_sigma(&img, &enc);
    println!("{raf}: {}x{}, σ {sigma:.2}", img.width(), img.height());
    let t = std::time::Instant::now();
    let done = denoise(&img, net.as_mut(), sigma, &|d, n| {
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

/// Four 512 px tiles of `raf` — its darkest, its busiest, its centre, a corner — on `tract` and on the
/// GPU, in the network's own 0…1 units.
fn parity(model: &str, raf: &str) {
    let path = std::path::Path::new(model);
    let mut cpu = ai_mask::denoise::Drunet::load(path).expect("tract");
    let ctx = gpu_pipeline::shared().cloned().expect("a GPU adapter");
    let weights = rustroom_app::denoised::gpu_weights(path).expect("weights");
    let mut gpu = gpu_pipeline::GpuDrunet::new(ctx, &weights).expect("the GPU network");
    let img = decode(raf);
    let enc = Encoding::of(&img);
    let sigma = noise_sigma(&img, &enc);
    let (w, h) = (img.width(), img.height());
    let [dark, busy] = windows(&img, &enc);
    let tiles = [
        ("dark", dark),
        ("busy", busy),
        ("centre", ((w - SIDE) / 2, (h - SIDE) / 2)),
        ("corner", (0, 0)),
    ];
    println!("{raf}: σ {sigma:.2}");
    for (name, (x0, y0)) in tiles {
        let crop = img.crop(x0, y0, SIDE, SIDE);
        let rgb: Vec<f32> = (0..SIDE * SIDE)
            .flat_map(|i| enc.encode(crop.pixel(i % SIDE, i / SIDE)))
            .collect();
        let t = std::time::Instant::now();
        let a = cpu.run(&rgb, SIDE, SIDE, sigma).expect("tract");
        let tc = t.elapsed().as_secs_f64();
        let t = std::time::Instant::now();
        let b = gpu.run(&rgb, SIDE, SIDE, sigma).expect("GPU");
        let tg = t.elapsed().as_secs_f64();
        let d: Vec<f32> = a.iter().zip(&b).map(|(x, y)| (x - y).abs()).collect();
        let max = d.iter().fold(0f32, |m, v| m.max(*v));
        let mean = d.iter().sum::<f32>() / d.len() as f32;
        println!(
            "  {name:6}: max {max:.2e} ({:.3} of 255), mean {mean:.2e}; tract {tc:.2} s, GPU {tg:.3} s",
            max * 255.0
        );
    }
}

/// The binned (2 × 2) image's 16 px blocks: each its mean encoded brightness, its activity, its Haar
/// coefficients — the estimator's own reading, kept per block so it can be sorted by brightness.
fn blocks(img: &core_image::ImageBuf, enc: &Encoding) -> Vec<(f32, f32, Vec<f32>)> {
    const B: usize = 16;
    let (w, h) = (img.width() / 2, img.height() / 2);
    let e = |x: usize, y: usize| {
        let mut acc = [0f32; 3];
        for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            let p = img.pixel(2 * x + dx, 2 * y + dy);
            for c in 0..3 {
                acc[c] += p[c] / 4.0;
            }
        }
        enc.encode(acc)
    };
    let mut out = Vec::new();
    for by in 0..h / B {
        for bx in 0..w / B {
            let (x0, y0) = (bx * B, by * B);
            let (mut act, mut mean, mut n) = (0f32, 0f32, 0f32);
            let mut coeffs = Vec::new();
            for y in y0..y0 + B - 1 {
                for x in x0..x0 + B - 1 {
                    let (p, r, d) = (e(x, y), e(x + 1, y), e(x, y + 1));
                    for c in 0..3 {
                        act += (p[c] - r[c]).abs() + (p[c] - d[c]).abs();
                        mean += p[c];
                    }
                    n += 3.0;
                    if (x - x0) % 2 == 0 && (y - y0) % 2 == 0 {
                        let q = e(x + 1, y + 1);
                        for c in 0..3 {
                            coeffs.push(((p[c] - r[c] - d[c] + q[c]) / 2.0).abs());
                        }
                    }
                }
            }
            out.push((mean / n, act / n, coeffs));
        }
    }
    out
}

/// σ (0…255, at one pixel — × 2 for the binning) from the flattest quarter of `blocks`.
fn sigma_of(mut b: Vec<&(f32, f32, Vec<f32>)>) -> Option<f32> {
    if b.len() < 4 {
        return None;
    }
    b.sort_by(|x, y| x.1.total_cmp(&y.1));
    b.truncate(b.len().div_ceil(4));
    let mut c: Vec<f32> = b.iter().flat_map(|x| x.2.iter().copied()).collect();
    let mid = c.len() / 2;
    let (_, m, _) = c.select_nth_unstable_by(mid, f32::total_cmp);
    Some(*m * 1.4826 * 255.0 * 2.0)
}

const BINS: usize = 8;

/// σ against brightness: eight bands of encoded brightness, each its σ by the estimator on its own blocks
/// (a band with too few blocks takes its neighbour's).
fn noise_curve(img: &core_image::ImageBuf, enc: &Encoding) -> [f32; BINS] {
    let all = blocks(img, enc);
    let mut curve = [f32::NAN; BINS];
    for (b, s) in curve.iter_mut().enumerate() {
        let lo = b as f32 / BINS as f32;
        let hi = lo + 1.0 / BINS as f32;
        *s = sigma_of(all.iter().filter(|x| x.0 >= lo && x.0 < hi).collect()).unwrap_or(f32::NAN);
    }
    for _ in 0..BINS {
        for b in 0..BINS {
            if curve[b].is_nan() {
                let n = [b.wrapping_sub(1), b + 1]
                    .into_iter()
                    .filter_map(|i| curve.get(i).copied())
                    .find(|v| !v.is_nan());
                if let Some(v) = n {
                    curve[b] = v;
                }
            }
        }
    }
    curve
}

/// The curve at brightness `v`, linear between the bands' centres.
fn curve_at(curve: &[f32; BINS], v: f32) -> f32 {
    let x = (v * BINS as f32 - 0.5).clamp(0.0, (BINS - 1) as f32);
    let (i, t) = (x.floor() as usize, x.fract());
    let j = (i + 1).min(BINS - 1);
    curve[i] * (1.0 - t) + curve[j] * t
}

/// A crop's noise map: each pixel's σ from the curve at its brightness, the brightness box-blurred over
/// 15 px so the noise itself does not set it.
fn noise_map(rgb: &[f32], side: usize, curve: &[f32; BINS], factor: f32) -> Vec<f32> {
    let luma: Vec<f32> = (0..side * side)
        .map(|i| (rgb[i * 3] + rgb[i * 3 + 1] + rgb[i * 3 + 2]) / 3.0)
        .collect();
    let r = 7isize;
    (0..side * side)
        .map(|i| {
            let (x, y) = ((i % side) as isize, (i / side) as isize);
            let (mut s, mut n) = (0f32, 0f32);
            for dy in -r..=r {
                for dx in -r..=r {
                    let (xx, yy) = (x + dx, y + dy);
                    if xx >= 0 && yy >= 0 && xx < side as isize && yy < side as isize {
                        s += luma[yy as usize * side + xx as usize];
                        n += 1.0;
                    }
                }
            }
            (curve_at(curve, s / n) * factor).clamp(1.0, 40.0)
        })
        .collect()
}

/// The noise an encoded `side × side` crop holds — the estimator on the crop alone, decoded back to the
/// linear light it reads.
fn left(rgb: &[f32], side: usize, enc: &Encoding) -> f32 {
    let lin: Vec<f32> = rgb
        .chunks(3)
        .flat_map(|p| enc.decode([p[0], p[1], p[2]]))
        .collect();
    let img = core_image::ImageBuf::from_vec(side, side, lin);
    sigma_of(blocks(&img, enc).iter().collect()).unwrap_or(f32::NAN)
}

fn strength(out: &std::path::Path, model: &str, rafs: &[String]) {
    let path = std::path::Path::new(model);
    let ctx = gpu_pipeline::shared().cloned().expect("a GPU adapter");
    let weights = rustroom_app::denoised::gpu_weights(path).expect("weights");
    let mut net = gpu_pipeline::GpuDrunet::new(ctx, &weights).expect("the GPU network");
    for raf in rafs {
        let stem = std::path::Path::new(raf)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("photo")
            .to_string();
        let img = decode(raf);
        let enc = Encoding::of(&img);
        let today = noise_sigma(&img, &enc);
        let curve = noise_curve(&img, &enc);
        println!("{stem}: today's σ {today:.2}");
        println!(
            "  σ by brightness (eighths): {}",
            curve.map(|v| format!("{v:.1}")).join(" · ")
        );
        let [dark, busy] = windows(&img, &enc);
        let mid = mid_window(&img, &enc);
        let cell = SIDE as u32 + 6;
        let mut sheet = image::RgbImage::from_pixel(5 * cell, 6 * cell, image::Rgb([40, 40, 40]));
        // ⚠ The busy crop shows the cost — texture a stronger σ smooths — where the flat ones show the
        // noise.
        let crops = [("dark", dark), ("mid", mid), ("busy", busy)];
        for (row, (name, (x0, y0))) in crops.into_iter().enumerate() {
            let crop = img.crop(x0, y0, SIDE, SIDE);
            let rgb: Vec<f32> = (0..SIDE * SIDE)
                .flat_map(|i| enc.encode(crop.pixel(i % SIDE, i / SIDE)))
                .collect();
            let flat = |s: f32| vec![s.min(40.0); SIDE * SIDE];
            let variants: Vec<(&str, Vec<f32>)> = vec![
                ("today", flat(today)),
                ("2×", flat(2.0 * today)),
                ("map", noise_map(&rgb, SIDE, &curve, 1.0)),
                ("map 2×", noise_map(&rgb, SIDE, &curve, 2.0)),
            ];
            let mut line = format!("  {name:4} crop: noise in {:.2}", left(&rgb, SIDE, &enc));
            let mut shown = vec![rgb.clone()];
            for (label, map) in &variants {
                let mean = map.iter().sum::<f32>() / map.len() as f32;
                let d = net.run_map(&rgb, SIDE, SIDE, map).expect("GPU");
                line += &format!(
                    " · {label} (σ {mean:.1}) leaves {:.2}",
                    left(&d, SIDE, &enc)
                );
                shown.push(d);
            }
            println!("{line}");
            for (c, v) in shown.iter().enumerate() {
                let im = image::RgbImage::from_fn(SIDE as u32, SIDE as u32, |x, y| {
                    let i = (y as usize * SIDE + x as usize) * 3;
                    image::Rgb([0, 1, 2].map(|k| (v[i + k].clamp(0.0, 1.0) * 255.0).round() as u8))
                });
                let x = c as u32 * cell;
                let y = row as u32 * 2 * cell;
                image::imageops::overlay(&mut sheet, &im, x as i64, y as i64);
                let q = SIDE as u32 / 4;
                let zoom = image::imageops::resize(
                    &image::imageops::crop_imm(&im, q, q, 2 * q, 2 * q).to_image(),
                    SIDE as u32,
                    SIDE as u32,
                    image::imageops::FilterType::Nearest,
                );
                image::imageops::overlay(&mut sheet, &zoom, x as i64, (y + cell) as i64);
            }
        }
        sheet
            .save(out.join(format!("{stem}_strength.png")))
            .expect("sheet");
    }
}

/// The flattest window of mid-tone brightness (encoded green 0.35…0.6), on a 256 grid.
fn mid_window(img: &core_image::ImageBuf, enc: &Encoding) -> (usize, usize) {
    let (w, h) = (img.width(), img.height());
    let mut best = (f32::MAX, (0, 0));
    for y0 in (0..h.saturating_sub(SIDE)).step_by(256) {
        for x0 in (0..w.saturating_sub(SIDE)).step_by(256) {
            let (mut sum, mut grad, mut n) = (0f32, 0f32, 0f32);
            for y in (y0..y0 + SIDE).step_by(8) {
                for x in (x0..x0 + SIDE - 8).step_by(8) {
                    let a = enc.encode(img.pixel(x, y));
                    let b = enc.encode(img.pixel(x + 4, y));
                    sum += a[1];
                    grad += (a[1] - b[1]).abs();
                    n += 1.0;
                }
            }
            let mean = sum / n;
            if (0.35..=0.6).contains(&mean) && grad / n < best.0 {
                best = (grad / n, (x0, y0));
            }
        }
    }
    best.1
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
        let gpu = args[2] == "--gpu";
        let a = if gpu { &args[3..] } else { &args[2..] };
        let out = std::path::PathBuf::from(&a[0]);
        std::fs::create_dir_all(&out).expect("out");
        return frame(&out, &a[1], &a[2], gpu);
    }
    if args[1] == "parity" {
        return parity(&args[2], &args[3]);
    }
    if args[1] == "strength" {
        let out = std::path::PathBuf::from(&args[2]);
        std::fs::create_dir_all(&out).expect("out");
        return strength(&out, &args[3], &args[4..]);
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
