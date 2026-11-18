//! What the display conversion costs `pack` per frame (2026-11-18): a 3200×2136 frame quantized with no conversion,
//! through the manual P3 matrix, and through a monitor profile's table — the calls `render_engine::pack` makes.
//! The GPU fast paths a non-sRGB display turns off are priced in M8.3g, not here.
//!
//!     cargo run --release --example display_pack_cost

use core_image::ImageBuf;
use std::time::Instant;

fn main() {
    let (w, h) = (3200, 2136);
    let mut img = ImageBuf::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let (u, v) = (x as f32 / w as f32, y as f32 / h as f32);
            img.set_pixel(x, y, [u, v, 1.0 - u * v]);
        }
    }
    let m27q = color::edid::Primaries {
        red: (0.6553, 0.3242),
        green: (0.2090, 0.7139),
        blue: (0.1465, 0.0508),
        white: (0.3135, 0.3291),
        gamma: 2.2,
    };
    let t = Instant::now();
    let profile = color::DisplayProfile::from_primaries("M27Q", &m27q).expect("a profile");
    println!(
        "building the table: {:.1} ms",
        t.elapsed().as_secs_f64() * 1e3
    );
    let displays = [
        ("sRGB (no conversion)", color::Display::SRGB),
        (
            "Display P3 (matrix)",
            color::Display::Space(color::OutputSpace::DisplayP3),
        ),
        ("M27Q profile (table)", color::Display::Profile(profile)),
    ];
    for (name, d) in displays {
        let mut best = f64::MAX;
        for _ in 0..7 {
            let t = Instant::now();
            let rgba = if d.is_srgb() {
                img.to_rgba8()
            } else {
                img.to_rgba8_with(move |c| d.convert(c))
            };
            best = best.min(t.elapsed().as_secs_f64() * 1e3);
            std::hint::black_box(rgba);
        }
        println!("{name:<22} {best:6.1} ms (best of 7)");
    }
}
