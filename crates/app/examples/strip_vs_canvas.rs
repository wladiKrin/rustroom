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

fn render(buf: &core_image::ImageBuf, edits: &edit_model::Edits) -> f32 {
    let out = pipeline::build_pipeline(edits, pipeline::RenderScale::new(1.0)).run(buf.clone());
    mean_luma(out.data())
}

fn main() {
    let files: Vec<String> = std::env::args().skip(1).collect();
    assert!(!files.is_empty(), "usage: strip_vs_canvas <raw>...");
    println!(
        "{:<16} {:>8} {:>10} {:>10} {:>10}",
        "file", "strip", "none", "camera", "camera+auto"
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

        let a = mean_luma(&strip);
        let none = render(&small, &edit_model::Edits::default());
        let cam = render(
            &small,
            &edit_model::Edits {
                rendering: edit_model::Rendering::Camera,
                ..Default::default()
            },
        );
        // ⚠ Auto Tone measures the *scene*, so it runs on the buffer, not on a render — the same
        // way the panel's button does.
        let mut auto = edit_model::Edits {
            rendering: edit_model::Rendering::Camera,
            ..Default::default()
        };
        // ⚠ The same baseline the app would pass, so the probe measures what the app does.
        rustroom_app::auto_tone_tool::apply(&mut auto, &small, None);
        let cam_auto = render(&small, &auto);

        let ev = |x: f32| (a / x).log2();
        println!(
            "{:<16} {a:8.4} {:+9.2}E {:+9.2}E {:+9.2}E",
            path.file_name().unwrap().to_string_lossy(),
            ev(none),
            ev(cam),
            ev(cam_auto)
        );
        sums[0] += ev(none).abs();
        sums[1] += ev(cam).abs();
        sums[2] += ev(cam_auto).abs();
        n += 1.0;
    }
    println!(
        "\nmean |gap| from the camera:  none {:+.2} EV   camera {:+.2} EV   camera+auto {:+.2} EV",
        sums[0] / n,
        sums[1] / n,
        sums[2] / n
    );
}
