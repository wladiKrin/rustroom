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
    // ⚠ Split by saturation, because the whole-image mean is a misleading statistic here: a
    // HueSatMap is the identity at saturation 0, so on a mostly-desaturated frame it moves the
    // average almost not at all while changing saturated colour a great deal.
    let (mut sat_sum, mut sat_n) = (0.0f64, 0usize);
    for y in 0..base.image.height() {
        for x in 0..base.image.width() {
            let (p, q) = (base.image.pixel(x, y), prof.image.pixel(x, y));
            let saturated = color::huesatmap::rgb_to_hsv(q)[1] > 0.5;
            for i in 0..3 {
                if saturated {
                    sat_sum += (q[i] - p[i]).abs() as f64;
                    sat_n += 1;
                }
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
    // ⚠ Written out because the numbers above cannot answer the question this slice actually
    // raises: does it look like a plausible camera rendering? That is judged by eye.
    for (name, img) in [
        ("/tmp/no_profile.jpg", &base),
        ("/tmp/with_profile.jpg", &prof),
    ] {
        let rendered = pipeline::build_pipeline(
            &edit_model::Edits::default(),
            pipeline::RenderScale::new(0.5),
            pipeline::TailRendering::colorimetric(),
        )
        .run(img.image.clone());
        export::save(
            &rendered,
            Path::new(name),
            export::ExportFormat::Jpeg,
            92,
            color::OutputSpace::Srgb,
        )
        .expect("write");
        println!("wrote {name}");
    }

    println!(
        "among saturated pixels (s > 0.5, {:.1}% of the frame): mean |diff| {:.6}",
        100.0 * sat_n as f64 / n as f64,
        sat_sum / sat_n.max(1) as f64
    );
}
