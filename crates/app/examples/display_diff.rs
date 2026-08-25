//! Measure what the display transform actually does to a real photograph.
//!
//! `cargo run --release -p app --example display_diff <raw>`
use rustroom_app::render_engine::render_once;
use std::path::PathBuf;

fn main() {
    let path = PathBuf::from(std::env::args().nth(1).expect("usage: display_diff <raw>"));
    let decoded =
        raw_decode::decode_raw(&path, &raw_decode::DecodeOptions::preview()).expect("decode");
    let buf = decoded.image;
    let edits = edit_model::Edits::default();

    let srgb = render_once(&buf, &edits, 1600, 0.5, color::OutputSpace::Srgb);
    for space in [color::OutputSpace::DisplayP3, color::OutputSpace::AdobeRgb] {
        let other = render_once(&buf, &edits, 1600, 0.5, space);
        let mut max = 0i32;
        let mut sum = 0i64;
        let mut over4 = 0usize;
        let mut n = 0usize;
        for (a, b) in srgb
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .zip(other.rgba.as_chunks::<4>().0.iter())
        {
            for i in 0..3 {
                let d = (a[i] as i32 - b[i] as i32).abs();
                max = max.max(d);
                sum += d as i64;
                if d > 4 {
                    over4 += 1;
                }
                n += 1;
            }
        }
        println!(
            "{:>12}: max {max:>3}/255   mean {:.2}   samples differing by >4: {:.1}%",
            space.label(),
            sum as f64 / n as f64,
            100.0 * over4 as f64 / n as f64
        );
        println!(
            "               histogram identical: {}",
            srgb.hist == other.hist
        );
    }
}
