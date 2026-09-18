//! Measure what the extra resample costs, in acutance and in time.
//!
//! ⚠ The question the spec deferred: perspective is its own stage, so a corrected photo goes
//! through lens → perspective → straighten, three bilinear resamples where it used to be two.
//! Fusing perspective into `GeometryStage` would recover one, at the cost of teaching `FrameMap`
//! to invert a homography. This says whether that is worth doing.
use edit_model::{Edits, Geometry, Perspective};
use std::time::Instant;

/// Mean absolute Laplacian — a standard acutance proxy. Higher is crisper.
fn acutance(img: &core_image::ImageBuf) -> f64 {
    let (w, h) = (img.width(), img.height());
    let lum = |x: usize, y: usize| {
        let p = img.pixel(x, y);
        (0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]) as f64
    };
    let mut sum = 0.0;
    let mut n = 0u64;
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let l = 4.0 * lum(x, y) - lum(x - 1, y) - lum(x + 1, y) - lum(x, y - 1) - lum(x, y + 1);
            sum += l.abs();
            n += 1;
        }
    }
    sum / n as f64
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: persp_sharpness <raw>");
    let opts = raw_decode::DecodeOptions::linear_scene();
    let dec = raw_decode::decode_raw(std::path::Path::new(&path), &opts).expect("decode");
    let src = dec.image;
    println!("source {}x{}", src.width(), src.height());

    let run = |label: &str, e: &Edits| {
        let t = Instant::now();
        let out = pipeline::build_pipeline(
            e,
            pipeline::RenderScale::FULL,
            pipeline::ProfileRendering::colorimetric(),
        )
        .run(src.clone());
        let ms = t.elapsed().as_millis();
        println!("  {label:34} acutance {:.5}  ({ms} ms)", acutance(&out));
    };

    // A straighten angle is present in all three so the *number of resamples* is what differs,
    // not whether geometry runs at all.
    let straight = Geometry {
        angle: 2.0,
        ..Default::default()
    };
    run("neutral (0 resamples)", &Edits::default());
    run(
        "straighten only (1 resample)",
        &Edits {
            geometry: straight,
            ..Default::default()
        },
    );
    run(
        "straighten + perspective (2)",
        &Edits {
            geometry: straight,
            perspective: Perspective {
                vertical: 40.0,
                ..Default::default()
            },
            ..Default::default()
        },
    );
    run(
        "perspective only (1 resample)",
        &Edits {
            perspective: Perspective {
                vertical: 40.0,
                ..Default::default()
            },
            ..Default::default()
        },
    );
    // ⚠ The magnification-free control: a perspective whose autoscale is 1.0, so the only thing
    // separating it from neutral is the resample itself.
    run(
        "tiny perspective, scale 0 (1 resample)",
        &Edits {
            perspective: Perspective {
                vertical: 1.0,
                scale: 0.0,
                ..Default::default()
            },
            ..Default::default()
        },
    );
}
