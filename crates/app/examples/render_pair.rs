//! Render a RAW and its JPEG sibling through the real pipeline at neutral edits, and compare.
//!
//! Both files are the same photograph. The JPEG is the camera's own rendering; the RAW goes
//! through Rustroom's colour path. This says how far apart the two *rendered canvases* are, and
//! in particular whether the RAW comes out more saturated.
//!
//! `cargo run --release -p app --example render_pair <raw> <jpg>`
use core_image::ImageBuf;

/// ⚠⚠ **Bucketed by saturation, not a whole-image mean.** This project has twice recorded that a
/// global mean is the wrong statistic for a colour question and understates it badly — most of a
/// frame is near-neutral, so the interesting pixels are averaged away.
fn stats(img: &ImageBuf, name: &str) {
    let rgb = img.to_rgb8();
    report(
        rgb.as_chunks::<3>().0.iter().map(|p| [p[0], p[1], p[2]]),
        name,
    );
}

fn report(px: impl Iterator<Item = [u8; 3]>, name: &str) {
    // Three buckets by how saturated the pixel is, plus the overall mean for comparison.
    let mut n = [0u64; 3];
    let mut sat = [0.0f64; 3];
    let mut all = (0u64, 0.0f64);
    for p in px {
        let (r, g, b) = (p[0] as f64, p[1] as f64, p[2] as f64);
        let mx = r.max(g).max(b);
        let mn = r.min(g).min(b);
        if mx <= 0.0 {
            continue;
        }
        let s = (mx - mn) / mx;
        all.0 += 1;
        all.1 += s;
        let bucket = if s < 0.15 {
            0
        } else if s < 0.4 {
            1
        } else {
            2
        };
        n[bucket] += 1;
        sat[bucket] += s;
    }
    let pct = |i: usize| 100.0 * n[i] as f64 / all.0 as f64;
    let avg = |i: usize| if n[i] == 0 { 0.0 } else { sat[i] / n[i] as f64 };
    println!(
        "{name:>26}  overall {:.4} | near-neutral {:.0}% @{:.3} | mid {:.0}% @{:.3} | saturated {:.0}% @{:.3}",
        all.1 / all.0 as f64,
        pct(0), avg(0), pct(1), avg(1), pct(2), avg(2)
    );
}

fn main() {
    let raw = std::env::args()
        .nth(1)
        .expect("usage: render_pair <raw> <jpg>");
    let jpg = std::env::args()
        .nth(2)
        .expect("usage: render_pair <raw> <jpg>");
    let edits = edit_model::Edits::default();

    // ⚠ With and without the DCP camera profile, because the app applies one when it is
    // configured and that is the only colour step the RAW path has and the JPEG path does not.
    for profile in [None, std::env::args().nth(3).map(std::path::PathBuf::from)] {
        let named = profile.is_some();
        let opts = raw_decode::DecodeOptions {
            camera_profile: profile,
            ..raw_decode::DecodeOptions::preview()
        };
        let d = raw_decode::decode_raw(std::path::Path::new(&raw), &opts).expect("raw decode");
        let a = d.image.downscale_to(512);
        let a = pipeline::build_pipeline(&edits, pipeline::RenderScale::new(1.0)).run(a);
        stats(
            &a,
            if named {
                "RAW + DCP -> pipeline"
            } else {
                "RAW (built-in) -> pipeline"
            },
        );
        if named {
            if let Some(dir) = std::env::args().nth(4) {
                let rgb = a.to_rgb8();
                image::save_buffer(
                    format!("{dir}/pair_RAW.png"),
                    &rgb,
                    a.width() as u32,
                    a.height() as u32,
                    image::ColorType::Rgb8,
                )
                .unwrap();
            }
        }
    }

    let d = image_decode::decode(std::path::Path::new(&jpg)).expect("jpeg decode");
    let b = d.image.downscale_to(512);
    let b = pipeline::build_pipeline(&edits, pipeline::RenderScale::new(1.0)).run(b);
    stats(&b, "JPEG -> pipeline");
    if let Some(dir) = std::env::args().nth(4) {
        let rgb = b.to_rgb8();
        image::save_buffer(
            format!("{dir}/pair_JPEG.png"),
            &rgb,
            b.width() as u32,
            b.height() as u32,
            image::ColorType::Rgb8,
        )
        .unwrap();
    }

    // And the camera's own rendering, untouched, as the reference both are trying to be near.
    let t = raw_decode::extract_thumbnail(std::path::Path::new(&raw)).expect("embedded");
    report(
        t.rgba.as_chunks::<4>().0.iter().map(|p| [p[0], p[1], p[2]]),
        "camera embedded JPEG",
    );
}
