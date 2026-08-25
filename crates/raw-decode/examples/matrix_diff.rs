//! Characterise where our camera matrix differs from libraw's.
use raw_decode::{decode_raw, DecodeOptions};

fn main() {
    let path = std::env::args().nth(1).expect("usage: matrix_diff <raw>");
    let ours = decode_raw(
        std::path::Path::new(&path),
        &DecodeOptions {
            camera_native: true,
            half_size: true,
            ..DecodeOptions::linear_scene()
        },
    )
    .expect("ours");
    let lib = decode_raw(
        std::path::Path::new(&path),
        &DecodeOptions {
            camera_native: false,
            half_size: true,
            ..DecodeOptions::linear_scene()
        },
    )
    .expect("libraw");

    let tol = 2.0 / 255.0;
    let (mut over, mut n) = (0usize, 0usize);
    let mut over_max_in = 0usize; // of the differing ones, how many had a channel >= 0.99 in ours
    let mut over_neg = 0usize; // how many had a negative channel in ours
    let mut worst = (0.0f32, [0.0f32; 3], [0.0f32; 3]);
    for y in 0..ours.image.height() {
        for x in 0..ours.image.width() {
            let (a, b) = (ours.image.pixel(x, y), lib.image.pixel(x, y));
            let d = (0..3).map(|i| (a[i] - b[i]).abs()).fold(0.0f32, f32::max);
            n += 1;
            if d > tol {
                over += 1;
                if a.iter().any(|v| *v >= 0.99) || b.iter().any(|v| *v >= 0.99) {
                    over_max_in += 1;
                }
                if a.iter().any(|v| *v < 0.0) {
                    over_neg += 1;
                }
                if d > worst.0 {
                    worst = (d, a, b);
                }
            }
        }
    }
    println!("pixels: {n}");
    println!(
        "differing by > 2/255: {over}  ({:.4}%)",
        100.0 * over as f64 / n as f64
    );
    println!(
        "  of those, clipped (a channel >= 0.99): {over_max_in} ({:.1}%)",
        100.0 * over_max_in as f64 / over.max(1) as f64
    );
    println!(
        "  of those, negative in ours:            {over_neg} ({:.1}%)",
        100.0 * over_neg as f64 / over.max(1) as f64
    );
    println!(
        "worst pixel: diff {:.4}  ours {:?}  libraw {:?}",
        worst.0, worst.1, worst.2
    );
}
