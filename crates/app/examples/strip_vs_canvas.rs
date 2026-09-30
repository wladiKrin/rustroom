//! Is the filmstrip thumbnail brighter than the canvas shows?
//!
//! ⚠ For an **unedited** photo `load_thumbnail` returns the camera's embedded JPEG untouched —
//! it takes an early return rather than paying a pipeline round trip 830 times for a no-op. So
//! the strip shows the *camera's* rendering, and this reports how far the canvas sits from it
//! under each rendering, which is the whole acceptance measurement for M5.4b.
//!
//! ⚠⚠ **The target is not zero.** The camera's JPEG carries that frame's own metering, which is
//! Auto Tone's job and not the rendering's — so the number to watch is the gap under
//! `Camera + Auto`, and the gap under `Camera` alone should land within the metering spread
//! (0.69 EV across the frames `curve_probe` pooled).
//!
//! ```text
//! cargo run --release -p app --example strip_vs_canvas <raw>...
//! ```

fn mean_luma(rgb: &[f32]) -> f32 {
    let n = rgb.len() / 3;
    (0..n)
        .map(|i| color::luminance([rgb[i * 3], rgb[i * 3 + 1], rgb[i * 3 + 2]]))
        .sum::<f32>()
        / n as f32
}

/// Mean saturation, `(max-min)/max`, over the non-black pixels.
///
/// ⚠ Skipping near-black is what makes this comparable between two renders of different
/// brightness: a pixel at 0.001 has a meaningless ratio, and one render having more of them
/// would move the mean without anything being less colourful.
fn mean_sat(rgb: &[f32]) -> f32 {
    let (mut sum, mut n) = (0.0f32, 0usize);
    for p in rgb.as_chunks::<3>().0 {
        let mx = p[0].max(p[1]).max(p[2]);
        if mx < 0.02 {
            continue;
        }
        sum += (mx - p[0].min(p[1]).min(p[2])) / mx;
        n += 1;
    }
    if n == 0 {
        0.0
    } else {
        sum / n as f32
    }
}

fn render(buf: &core_image::ImageBuf, edits: &edit_model::Edits) -> (f32, f32) {
    let out = pipeline::build_pipeline(
        edits,
        pipeline::RenderScale::new(1.0),
        pipeline::TailRendering::colorimetric(),
    )
    .run(buf.clone());
    (mean_luma(out.data()), mean_sat(out.data()))
}

fn main() {
    let files: Vec<String> = std::env::args().skip(1).collect();
    assert!(!files.is_empty(), "usage: strip_vs_canvas <raw>...");
    println!(
        "{:<16} {:>18} {:>18} {:>18}",
        "file", "strip", "none", "camera"
    );
    println!(
        "{:<16} {:>8} {:>9} {:>8} {:>9} {:>8} {:>9}",
        "", "luma", "sat", "dEV", "sat", "dEV", "sat"
    );
    let mut sums = [0f32; 3];
    let mut n = 0f32;
    for f in &files {
        let path = std::path::Path::new(f);
        let Ok(thumb) = raw_decode::extract_thumbnail(path) else {
            continue;
        };
        let strip: Vec<f32> = thumb
            .rgba
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|p| (0..3).map(move |c| p[c] as f32 / 255.0))
            .collect();
        let Ok(d) = raw_decode::decode_raw(path, &raw_decode::DecodeOptions::linear_scene()) else {
            continue;
        };
        let small = d.image.downscale_to(thumb.width.max(thumb.height));

        let (a, a_sat) = (mean_luma(&strip), mean_sat(&strip));
        let none = render(&small, &edit_model::Edits::default());
        let cam = render(
            &small,
            &edit_model::Edits {
                rendering: Some(edit_model::Rendering::Camera),
                ..Default::default()
            },
        );

        let ev = |x: f32| (a / x).log2();
        println!(
            "{:<16} {a:8.4} {a_sat:9.4} {:+8.2} {:9.4} {:+8.2} {:9.4}",
            path.file_name().unwrap().to_string_lossy(),
            ev(none.0),
            none.1,
            ev(cam.0),
            cam.1
        );
        sums[0] += ev(none.0).abs();
        sums[1] += ev(cam.0).abs();
        // ⚠ Saturation as a *ratio* to the camera's, so the sign is readable: below 1 is duller.
        sums[2] += cam.1 / a_sat;
        n += 1.0;
    }
    println!(
        "\nmean |gap| from the camera:  none {:+.2} EV   camera {:+.2} EV",
        sums[0] / n,
        sums[1] / n
    );
    println!("mean saturation, rendered / camera: {:.3}x", sums[2] / n);
}
