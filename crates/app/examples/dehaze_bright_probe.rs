//! Find the frames where Dehaze over-corrects a bright achromatic subject — the classic
//! dark-channel-prior artifact (snow, white walls, a white cat read as haze).
//!
//! ```text
//! cargo run --release -p app --example dehaze_bright_probe <out-dir> <raw-dir> [top-n] [amount]
//! ```
//!
//! For every RAW in the folder: a preview decode, then a neutral render and a Dehaze render at
//! `amount` (default 60) under the camera rendering (what the GUI shows by default). A pixel is
//! "bright achromatic" when its encoded max channel is above `BRIGHT` and its saturation below
//! `ACHROMATIC`, judged on the **neutral** render. The score is the mean relative darkening of
//! those pixels; the chroma gain says whether a white turned tinted, and the whole-frame effect
//! says how much Dehaze did at all. Frames with fewer than `MIN_FRACTION` of such pixels are
//! listed but not ranked — there is nothing there for the artifact to act on. The top-n frames
//! get a contact sheet, neutral | dehazed, so the ranking can be checked by eye; a second list
//! ranks by overall effect, which is where any genuine haze would be.
//!
//! `PROBE_STEMS=DSCF6546,DSCF6557` restricts the run to named frames, for re-measuring a
//! change on the known worst cases without decoding the whole folder.
//!
//! ⚠ A measurement, not a definition: the thresholds pick candidates for a human to look at.
//!
//! ⚠ What it found (2026-09-08), so nobody re-runs it to learn the same thing: the artifact is
//! mild at +60 (worst frame 6% mean darkening over 415) and severe at +100 on flat white walls
//! (19% mean, 48% worst decile); the frames Dehaze changes most in this folder are kittens in
//! front of a flat lilac wall, not landscapes — this catalogue holds no genuinely hazy frame.
use std::path::PathBuf;

const BRIGHT: f32 = 0.6;
const ACHROMATIC: f32 = 0.12;
const MIN_FRACTION: f32 = 0.03;
const H: u32 = 560;

fn strip(img: &image::RgbImage) -> image::RgbImage {
    let w = (img.width() as f32 * H as f32 / img.height() as f32).round() as u32;
    image::imageops::resize(img, w, H, image::imageops::FilterType::Lanczos3)
}

fn render(small: &core_image::ImageBuf, amount: f32) -> core_image::ImageBuf {
    let edits = edit_model::Edits {
        rendering: Some(edit_model::Rendering::Camera),
        dehaze: edit_model::Dehaze { amount },
        ..Default::default()
    };
    pipeline::build_pipeline(
        &edits,
        pipeline::RenderScale::new(1.0),
        pipeline::TailRendering::colorimetric(),
    )
    .run(small.clone())
}

struct Row {
    stem: String,
    fraction: f32,
    darkening: f32,
    worst_decile: f32,
    /// Mean Oklab chroma gained by the bright achromatic pixels: a white that turned tinted.
    chroma_gain: f32,
    /// Mean absolute Oklab L change over the WHOLE frame: how much Dehaze did at all, which
    /// is highest where there is real haze to remove.
    effect: f32,
}

/// Oklab of an sRGB-encoded pixel.
fn oklab(px: [f32; 3]) -> [f32; 3] {
    color::oklab::linear_srgb_to_oklab([
        color::srgb_to_linear(px[0]),
        color::srgb_to_linear(px[1]),
        color::srgb_to_linear(px[2]),
    ])
}

fn main() {
    let mut args = std::env::args().skip(1);
    let out = PathBuf::from(
        args.next()
            .expect("usage: dehaze_bright_probe <out-dir> <raw-dir> [top-n] [amount]"),
    );
    let dir = PathBuf::from(args.next().expect("usage: <raw-dir>"));
    let top_n: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(8);
    let amount: f32 = args.next().and_then(|s| s.parse().ok()).unwrap_or(60.0);
    std::fs::create_dir_all(&out).unwrap();

    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)
        .expect("read dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.extension()
                .map(|e| e.to_string_lossy().eq_ignore_ascii_case("raf"))
                .unwrap_or(false)
        })
        .collect();
    paths.sort();
    // Optional: restrict to named stems, for re-measuring known frames.
    if let Ok(list) = std::env::var("PROBE_STEMS") {
        let keep: Vec<String> = list.split(',').map(|s| s.trim().to_string()).collect();
        paths.retain(|p| {
            let stem = p.file_stem().unwrap().to_string_lossy().to_string();
            keep.contains(&stem)
        });
    }

    let mut rows = Vec::new();
    let mut renders: std::collections::HashMap<String, (image::RgbImage, image::RgbImage)> =
        Default::default();
    for path in &paths {
        let stem = path.file_stem().unwrap().to_string_lossy().to_string();
        let d = match raw_decode::decode_raw(path, &raw_decode::DecodeOptions::preview()) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("{stem}: decode failed: {e}");
                continue;
            }
        };
        let small = d.image.downscale_to(800);
        let neutral = render(&small, 0.0);
        let dehazed = render(&small, amount);
        let (w, h) = (neutral.width(), neutral.height());

        // Per-pixel relative darkening on the bright achromatic set.
        let mut drops: Vec<f32> = Vec::new();
        let mut chroma_sum = 0.0f32;
        let mut effect_sum = 0.0f32;
        for (n, dz) in neutral
            .data()
            .as_chunks::<3>()
            .0
            .iter()
            .zip(dehazed.data().as_chunks::<3>().0)
        {
            let (on, od) = (oklab(*n), oklab(*dz));
            effect_sum += (on[0] - od[0]).abs();
            let mx = n[0].max(n[1]).max(n[2]);
            let mn = n[0].min(n[1]).min(n[2]);
            if mx < BRIGHT {
                continue;
            }
            if (mx - mn) / mx > ACHROMATIC {
                continue;
            }
            let ln = color::luminance(*n);
            let ld = color::luminance(*dz);
            drops.push((ln - ld) / ln.max(1e-4));
            chroma_sum += od[1].hypot(od[2]) - on[1].hypot(on[2]);
        }
        let effect = effect_sum / (w * h) as f32;
        let fraction = drops.len() as f32 / (w * h) as f32;
        drops.sort_by(|a, b| b.total_cmp(a));
        let darkening = if drops.is_empty() {
            0.0
        } else {
            drops.iter().sum::<f32>() / drops.len() as f32
        };
        let worst_decile = if drops.is_empty() {
            0.0
        } else {
            let k = (drops.len() / 10).max(1);
            drops[..k].iter().sum::<f32>() / k as f32
        };
        let chroma_gain = if drops.is_empty() {
            0.0
        } else {
            chroma_sum / drops.len() as f32
        };
        println!(
            "{stem}: bright-achromatic {:5.1}%  mean darkening {:5.1}%  worst decile {:5.1}%  chroma gain {:+.4}  effect {:.4}",
            fraction * 100.0,
            darkening * 100.0,
            worst_decile * 100.0,
            chroma_gain,
            effect
        );
        let to_rgb = |b: &core_image::ImageBuf| {
            image::RgbImage::from_raw(b.width() as u32, b.height() as u32, export::to_rgb8(b))
                .expect("rgb")
        };
        renders.insert(stem.clone(), (to_rgb(&neutral), to_rgb(&dehazed)));
        rows.push(Row {
            stem,
            fraction,
            darkening,
            worst_decile,
            chroma_gain,
            effect,
        });
    }

    let mut ranked: Vec<&Row> = rows.iter().filter(|r| r.fraction >= MIN_FRACTION).collect();
    ranked.sort_by(|a, b| b.darkening.total_cmp(&a.darkening));
    println!(
        "\n== ranked by mean darkening of bright achromatic pixels (>= {:.0}% of frame), at +{amount} ==",
        MIN_FRACTION * 100.0
    );
    for (i, r) in ranked.iter().take(top_n).enumerate() {
        println!(
            "{:2}. {}  fraction {:5.1}%  darkening {:5.1}%  worst decile {:5.1}%  chroma gain {:+.4}  effect {:.4}",
            i + 1,
            r.stem,
            r.fraction * 100.0,
            r.darkening * 100.0,
            r.worst_decile * 100.0,
            r.chroma_gain,
            r.effect
        );
        let (n, d) = &renders[&r.stem];
        let (a, b) = (strip(n), strip(d));
        let gap = 12u32;
        let mut sheet =
            image::RgbImage::from_pixel(a.width() + b.width() + gap, H, image::Rgb([24, 24, 24]));
        image::imageops::overlay(&mut sheet, &a, 0, 0);
        image::imageops::overlay(&mut sheet, &b, (a.width() + gap) as i64, 0);
        let dst = out.join(format!("{:02}_{}.jpg", i + 1, r.stem));
        sheet.save(&dst).unwrap();
        println!("    {}  (neutral | dehaze +{amount})", dst.display());
    }

    // The other list: where Dehaze does the most, which is where the haze is. A guard against
    // the bright-object artifact must leave these alone, so they are the regression set.
    let mut by_effect: Vec<&Row> = rows.iter().collect();
    by_effect.sort_by(|a, b| b.effect.total_cmp(&a.effect));
    println!("\n== ranked by overall effect (mean |dL| over the frame): the haziest frames ==");
    for (i, r) in by_effect.iter().take(top_n).enumerate() {
        println!(
            "{:2}. {}  effect {:.4}  bright-achromatic {:5.1}%  darkening {:5.1}%",
            i + 1,
            r.stem,
            r.effect,
            r.fraction * 100.0,
            r.darkening * 100.0
        );
        let (n, d) = &renders[&r.stem];
        let (a, b) = (strip(n), strip(d));
        let gap = 12u32;
        let mut sheet =
            image::RgbImage::from_pixel(a.width() + b.width() + gap, H, image::Rgb([24, 24, 24]));
        image::imageops::overlay(&mut sheet, &a, 0, 0);
        image::imageops::overlay(&mut sheet, &b, (a.width() + gap) as i64, 0);
        let dst = out.join(format!("hazy_{:02}_{}.jpg", i + 1, r.stem));
        sheet.save(&dst).unwrap();
    }
}
