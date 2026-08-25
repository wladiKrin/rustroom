//! Compare the DCP render against libraw's matrix.
//!
//! `cargo run --release -p raw-decode --example dcp_compare <raw> <dcp>`
use raw_decode::{decode_raw, DecodeOptions};
use std::path::{Path, PathBuf};

fn main() {
    let mut a = std::env::args().skip(1);
    let raw = PathBuf::from(a.next().expect("usage: dcp_compare <raw> <dcp>"));
    let dcp = PathBuf::from(a.next().expect("usage: dcp_compare <raw> <dcp>"));

    let base = decode_raw(
        Path::new(&raw),
        &DecodeOptions {
            half_size: true,
            ..DecodeOptions::linear_scene()
        },
    )
    .expect("libraw path");
    let prof = decode_raw(
        Path::new(&raw),
        &DecodeOptions {
            half_size: true,
            camera_profile: Some(dcp),
            ..DecodeOptions::linear_scene()
        },
    )
    .expect("dcp path");

    let (mut max, mut sum, mut n) = (0.0f32, 0.0f64, 0usize);
    // Mean per channel, to show whether the difference is a cast or noise.
    let mut chan = [0.0f64; 3];
    for y in 0..base.image.height() {
        for x in 0..base.image.width() {
            let (p, q) = (base.image.pixel(x, y), prof.image.pixel(x, y));
            for i in 0..3 {
                let d = q[i] - p[i];
                max = max.max(d.abs());
                sum += d.abs() as f64;
                chan[i] += d as f64;
                n += 1;
            }
        }
    }
    let px = n / 3;
    println!(
        "DCP vs libraw: max {max:.5}  mean |diff| {:.6}",
        sum / n as f64
    );
    println!(
        "signed mean per channel: R {:+.5}  G {:+.5}  B {:+.5}",
        chan[0] / px as f64,
        chan[1] / px as f64,
        chan[2] / px as f64
    );
    println!("  (a small difference is the expected result — see the spec)");
}
