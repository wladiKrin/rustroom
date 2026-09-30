//! M6.4c: how much of the author's own photographs falls outside sRGB, and what each intent does
//! to it — the evidence the default intent is chosen from.
//!
//! Every edited RAW (a `.RAF.json` sidecar beside it) is rendered through the real prefix and tail
//! into **ProPhoto**, the identity, so the working-space colour survives. Per photo it reports:
//! - the share of pixels outside sRGB;
//! - of those, the share at Oklab lightness ≥ 1 — over-bright colour that both intents take to
//!   white at constant lightness, where the old clip kept it saturated;
//! - the old per-channel clip's mean and worst Oklab hue shift on the out-of-gamut pixels.
//!
//! For the most affected photos it writes `<out>/<stem>.png`: the most affected window rendered
//! **old clip | relative colorimetric | perceptual | adaptive-L0 prototype**, magnified 2×.
//!
//! ⚠ Approximations, stated: brush masks render without coverage, no lens correction, and the
//! half-size decode. None of them touches colour at the output transform, which is what is
//! compared.
//!
//! ```text
//! cargo run --release -p app --example probe_gamut_crops <out-dir> <dir>...
//! ```
use color::{GamutMap, OutputSpace, RenderingIntent};
use core_image::ImageBuf;
use pipeline::{RenderScale, RenderSpec, TailRendering};
use std::path::{Path, PathBuf};

const LONG: usize = 1600;
const WIN: usize = 200;
const MAG: usize = 2;
const TOP: usize = 6;

fn main() {
    let mut args = std::env::args().skip(1);
    let out = PathBuf::from(
        args.next()
            .expect("usage: probe_gamut_crops <out> <dir>..."),
    );
    std::fs::create_dir_all(&out).unwrap();
    let dcp = dirs_profile();
    let prof = camera_profile::CameraProfile::from_file(&dcp).expect("the X-T4 profile");
    let mut raws: Vec<PathBuf> = Vec::new();
    for d in args {
        for e in std::fs::read_dir(&d).unwrap().flatten() {
            let p = e.path();
            if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("raf"))
                && rustroom_app::sidecar::load(&p).is_some()
            {
                raws.push(p);
            }
        }
    }
    raws.sort();
    assert!(
        !raws.is_empty(),
        "no edited RAWs found — the probe would report nothing"
    );
    println!("{} edited RAWs", raws.len());

    let mut rows: Vec<(f32, PathBuf, ImageBuf)> = Vec::new();
    println!(
        "{:<16} {:>8} {:>9} {:>10} {:>10}",
        "photo", "out%", "bright%", "clip hue°", "worst°"
    );
    for p in &raws {
        let edits = rustroom_app::sidecar::load(p).unwrap();
        let d = match raw_decode::decode_raw(
            p,
            &raw_decode::DecodeOptions {
                camera_profile: Some(dcp.clone()),
                ..raw_decode::DecodeOptions::preview()
            },
        ) {
            Ok(d) => d,
            Err(e) => {
                println!("{}: {e}", p.display());
                continue;
            }
        };
        let full_long = d.metadata.width.max(d.metadata.height).max(1) as f32;
        let img = d.image.downscale_to(LONG);
        let scale = RenderScale::new(img.width().max(img.height()) as f32 / full_long);
        let spec = RenderSpec {
            space: OutputSpace::ProPhoto,
            baseline: d.metadata.as_shot.unwrap_or_default(),
            airlight: Some(pipeline::prefix_airlight(
                &edits,
                &img,
                pipeline::Resampler::default(),
                None,
            )),
            ..RenderSpec::default()
        };
        let (prefix, tail) = pipeline::build_pipeline_split_in_region_with_brush(
            &edits,
            scale,
            None,
            None,
            spec,
            None,
            TailRendering {
                curve: prof.tone_curve.as_deref(),
                baseline_ev: prof.baseline_exposure,
                look: None,
            },
        );
        // ProPhoto-encoded output → linear working space.
        let working = tail
            .run(prefix.run(img))
            .map_pixels(|c| c.map(|v| OutputSpace::ProPhoto.decode(v)));
        let (n, mut out, mut bright, mut hue_sum, mut hue_worst) = (
            working.width() * working.height(),
            0usize,
            0usize,
            0f32,
            0f32,
        );
        for y in 0..working.height() {
            for x in 0..working.width() {
                let c = working.pixel(x, y);
                if !color::is_out_of_gamut(c, OutputSpace::Srgb) {
                    continue;
                }
                out += 1;
                let [l, a, b] = color::oklab::prophoto_to_oklab(c);
                if l >= 1.0 {
                    bright += 1;
                    continue;
                }
                if a.hypot(b) < 0.02 {
                    continue;
                }
                let h = hue(old_clip(c));
                let d = hue_diff_deg(h, b.atan2(a));
                hue_sum += d;
                hue_worst = hue_worst.max(d);
            }
        }
        let stem = p.file_stem().unwrap().to_string_lossy().to_string();
        let share = out as f32 / n as f32 * 100.0;
        println!(
            "{:<16} {:>7.2}% {:>8.1}% {:>10.2} {:>10.1}",
            stem,
            share,
            if out > 0 {
                bright as f32 / out as f32 * 100.0
            } else {
                0.0
            },
            if out > bright {
                hue_sum / (out - bright) as f32
            } else {
                0.0
            },
            hue_worst
        );
        rows.push((share, p.clone(), working));
    }

    rows.sort_by(|a, b| b.0.total_cmp(&a.0));
    for (share, p, working) in rows.into_iter().take(TOP) {
        let (x0, y0) = worst_window(&working);
        let crop = working.crop(x0, y0, WIN.min(working.width()), WIN.min(working.height()));
        let panels = [
            encode(&crop, old_clip),
            encode(&crop, |c| mapped(RenderingIntent::RelativeColorimetric, c)),
            encode(&crop, |c| mapped(RenderingIntent::Perceptual, c)),
            encode(&crop, adaptive_l0),
        ];
        let stem = p.file_stem().unwrap().to_string_lossy().to_string();
        let dest = out.join(format!("{stem}.png"));
        save_row(&dest, &panels, crop.width(), crop.height());
        println!(
            "{stem}: {share:.2}% out, window at ({x0},{y0}) → {}",
            dest.display()
        );
    }
}

fn dirs_profile() -> PathBuf {
    let home = std::env::var("HOME").unwrap();
    Path::new(&home).join("Library/Application Support/rustroom/profiles/FUJIFILM X-T4.dcp")
}

fn hue(c: [f32; 3]) -> f32 {
    let [_, a, b] = color::oklab::prophoto_to_oklab(c);
    b.atan2(a)
}

fn hue_diff_deg(a: f32, b: f32) -> f32 {
    let d = (a - b).rem_euclid(std::f32::consts::TAU);
    d.min(std::f32::consts::TAU - d).to_degrees()
}

/// The transform this milestone replaced: matrix, per-channel clamp, and back to working.
fn old_clip(c: [f32; 3]) -> [f32; 3] {
    let lin = OutputSpace::Srgb.from_working(c).map(|v| v.clamp(0.0, 1.0));
    color::srgb_to_prophoto(lin)
}

/// PROTOTYPE, not shipped: Ottosson's "adaptive L0" gamut clip (α = 0.05). Hue fixed; the
/// colour is projected toward a lightness `L0` on the grey axis that depends on its own lightness
/// and chroma, so a dark saturated colour lightens a little instead of losing its chroma.
fn adaptive_l0(c: [f32; 3]) -> [f32; 3] {
    if !color::is_out_of_gamut(c, OutputSpace::Srgb) {
        return c;
    }
    let [l, a, b] = color::oklab::prophoto_to_oklab(c);
    if l <= 0.0 {
        return [0.0; 3];
    }
    if l >= 1.0 {
        return [1.0; 3];
    }
    let ch = a.hypot(b);
    if ch <= 0.0 {
        return c;
    }
    let alpha = 0.05f32;
    let ld = l - 0.5;
    let e1 = 0.5 + ld.abs() + alpha * ch;
    let l0 = 0.5 * (1.0 + ld.signum() * (e1 - (e1 * e1 - 2.0 * ld.abs()).max(0.0).sqrt()));
    let at = |t: f32| color::oklab::oklab_to_prophoto([l0 + t * (l - l0), t * a, t * b]);
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..24 {
        let mid = 0.5 * (lo + hi);
        if color::is_out_of_gamut(at(mid), OutputSpace::Srgb) {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    at(lo)
}

fn mapped(intent: RenderingIntent, c: [f32; 3]) -> [f32; 3] {
    GamutMap::for_space(OutputSpace::Srgb).map(intent, c)
}

/// sRGB bytes of `img` after `f` (working → working), through the real encode.
fn encode(img: &ImageBuf, f: impl Fn([f32; 3]) -> [f32; 3]) -> Vec<u8> {
    let mut v = Vec::with_capacity(img.width() * img.height() * 3);
    for y in 0..img.height() {
        for x in 0..img.width() {
            let lin = OutputSpace::Srgb.from_working(f(img.pixel(x, y)));
            for c in lin {
                v.push((OutputSpace::Srgb.encode(c) * 255.0).round() as u8);
            }
        }
    }
    v
}

/// The `WIN`² window with the most out-of-sRGB pixels, on a coarse grid.
fn worst_window(img: &ImageBuf) -> (usize, usize) {
    let (w, h) = (img.width(), img.height());
    let mut best = (0usize, 0usize, 0usize);
    let step = WIN / 4;
    let mut y = 0;
    while y + WIN <= h {
        let mut x = 0;
        while x + WIN <= w {
            let mut n = 0;
            for yy in (y..y + WIN).step_by(2) {
                for xx in (x..x + WIN).step_by(2) {
                    n += color::is_out_of_gamut(img.pixel(xx, yy), OutputSpace::Srgb) as usize;
                }
            }
            if n > best.2 {
                best = (x, y, n);
            }
            x += step;
        }
        y += step;
    }
    (best.0, best.1)
}

fn save_row(dest: &Path, panels: &[Vec<u8>], w: usize, h: usize) {
    let gutter = 12;
    let (pw, ph) = (w * MAG, h * MAG);
    let total_w = panels.len() * pw + (panels.len() - 1) * gutter;
    let mut img =
        image::RgbImage::from_pixel(total_w as u32, ph as u32, image::Rgb([255, 255, 255]));
    for (k, p) in panels.iter().enumerate() {
        let ox = k * (pw + gutter);
        for y in 0..ph {
            for x in 0..pw {
                let i = ((y / MAG) * w + x / MAG) * 3;
                img.put_pixel(
                    (ox + x) as u32,
                    y as u32,
                    image::Rgb([p[i], p[i + 1], p[i + 2]]),
                );
            }
        }
    }
    img.save(dest).unwrap();
}
