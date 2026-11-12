//! M9.11 probe: the red-eye stage on a real photo, with the eye as stored in its sidecar.
//!
//! Prints, per 1-px distance ring from the eye's centre, the redness before and after the stage
//! and how many pixels in that ring stay visibly red, then writes before/after crops ×8.
//!
//! `cargo run --release -p app --example red_eye_probe -- <image> <cx> <cy> <radius> <size> <darken> <out-prefix>`

use edit_model::{Eye, EyeKind};
use pipeline::{redness, PipelineStage, RedEyeStage};
use std::path::Path;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let f = |i: usize| a[i].parse::<f32>().unwrap();
    let dec = image_decode::decode(Path::new(&a[1])).expect("decode");
    let mut img = dec.image;
    for y in 0..img.height() {
        for x in 0..img.width() {
            img.set_pixel(x, y, color::srgb_to_prophoto(img.pixel(x, y)));
        }
    }
    let eye = Eye {
        decode: false,
        centre: [f(2), f(3)],
        radius: f(4),
        kind: EyeKind::Red,
        pupil_size: f(5),
        darken: f(6),
        catchlight: None,
    };
    let (w, h) = (img.width(), img.height());
    let (cx, cy) = (
        eye.centre[0] * (w - 1) as f32,
        eye.centre[1] * (h - 1) as f32,
    );
    let r = eye.radius * w.max(h) as f32;
    let out = RedEyeStage::new(&[eye]).apply(img.clone());
    println!("centre ({cx:.1}, {cy:.1}) radius {r:.1} px");
    println!("ring  n   ρ_before(mean/max)  ρ_after(mean/max)  after>0.15  lum_before");
    for ring in 0..((r * 1.6) as usize) {
        let (mut n, mut sb, mut mb, mut sa, mut ma, mut red, mut lum) =
            (0, 0.0, 0.0f32, 0.0, 0.0f32, 0, 0.0);
        for y in (cy - r * 2.0) as usize..=(cy + r * 2.0) as usize {
            for x in (cx - r * 2.0) as usize..=(cx + r * 2.0) as usize {
                let d = (x as f32 - cx).hypot(y as f32 - cy);
                if d as usize != ring {
                    continue;
                }
                let (b, af) = (redness(img.pixel(x, y)), redness(out.pixel(x, y)));
                n += 1;
                sb += b;
                mb = mb.max(b);
                sa += af;
                ma = ma.max(af);
                red += (af > 0.15) as usize;
                lum += color::luminance(img.pixel(x, y));
            }
        }
        println!(
            "{ring:>4} {n:>3}   {:.2} / {:.2}         {:.2} / {:.2}        {red:>3}       {:.3}",
            sb / n as f32,
            mb,
            sa / n as f32,
            ma,
            lum / n as f32
        );
    }
    // Crops ×8, sRGB-encoded.
    let half = (r * 2.5) as usize;
    let (x0, y0) = (cx as usize - half, cy as usize - half);
    for (name, buf) in [("before", &img), ("after", &out)] {
        let s = 8;
        let side = 2 * half;
        let mut px = vec![0u8; side * s * side * s * 3];
        for y in 0..side * s {
            for x in 0..side * s {
                let p = color::prophoto_to_srgb(buf.pixel(x0 + x / s, y0 + y / s));
                for c in 0..3 {
                    let v = color::linear_to_srgb(p[c].clamp(0.0, 1.0));
                    px[(y * side * s + x) * 3 + c] = (v * 255.0).round() as u8;
                }
            }
        }
        image::save_buffer(
            format!("{}_{name}.png", a[7]),
            &px,
            (side * s) as u32,
            (side * s) as u32,
            image::ColorType::Rgb8,
        )
        .unwrap();
    }
}
