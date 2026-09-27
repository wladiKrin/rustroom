//! M9.10: how another application's rendering of a Rustroom DNG compares with Rustroom's own
//! export of the same edit — the done-when's "rendered image intact", measured.
//!
//! ```text
//! cargo run --release -p export --example probe_dng_compare <reference.tif> <crop.png> <other.tif>...
//! ```
//! Prints, per file, the mean and 99th-percentile absolute difference in 8-bit levels, the mean
//! signed difference per channel, and the exposure difference in EV (mean luminance ratio). Writes
//! a side-by-side of a centre crop: reference first, then each file.

fn load(p: &str) -> image::Rgb32FImage {
    image::open(p)
        .unwrap_or_else(|e| panic!("{p}: {e}"))
        .to_rgb32f()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (reference, crop_out, others) = (&args[0], &args[1], &args[2..]);
    let a = load(reference);
    let (w, h) = a.dimensions();
    println!("reference {reference}: {w}x{h}");
    let mut crops = vec![a.clone()];
    for p in others {
        let b = load(p);
        if b.dimensions() != (w, h) {
            println!("{p}: size {:?} differs from the reference", b.dimensions());
            continue;
        }
        let mut diffs: Vec<f32> = Vec::with_capacity((w * h * 3) as usize);
        let mut signed = [0f64; 3];
        let (mut la, mut lb) = (0f64, 0f64);
        for (pa, pb) in a.pixels().zip(b.pixels()) {
            for c in 0..3 {
                let d = (pb[c] - pa[c]) * 255.0;
                diffs.push(d.abs());
                signed[c] += d as f64;
            }
            let lum = |p: &image::Rgb<f32>| 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2];
            la += lum(pa) as f64;
            lb += lum(pb) as f64;
        }
        let n = (w * h) as f64;
        diffs.sort_by(|x, y| x.total_cmp(y));
        let mean = diffs.iter().map(|&d| d as f64).sum::<f64>() / diffs.len() as f64;
        let p99 = diffs[diffs.len() * 99 / 100];
        println!(
            "{p}: mean |Δ| {mean:.2} levels, p99 {p99:.1}, max {:.0}; mean Δ R/G/B {:+.2} {:+.2} {:+.2}; exposure {:+.2} EV (encoded-luminance ratio)",
            diffs.last().unwrap(),
            signed[0] / n,
            signed[1] / n,
            signed[2] / n,
            (lb / la).log2() * 2.2,
        );
        crops.push(b);
    }
    // A centre crop of 480×480 from each, side by side.
    let s = 480u32.min(w).min(h);
    let (x0, y0) = ((w - s) / 2, (h - s) / 2);
    let mut out = image::RgbImage::new(s * crops.len() as u32 + 8 * (crops.len() as u32 - 1), s);
    for (k, img) in crops.iter().enumerate() {
        let ox = k as u32 * (s + 8);
        for y in 0..s {
            for x in 0..s {
                let p = img.get_pixel(x0 + x, y0 + y);
                out.put_pixel(
                    ox + x,
                    y,
                    image::Rgb(p.0.map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8)),
                );
            }
        }
    }
    out.save(crop_out).unwrap();
    println!("crops → {crop_out}");
}
