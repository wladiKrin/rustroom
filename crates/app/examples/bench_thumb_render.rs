//! What does an *edit-accurate* thumbnail cost, and from what source?
//!
//! Three candidate sources, timed on one RAW:
//!   A. the camera's embedded JPEG, linearized and run through the pipeline
//!   B. a half-size libraw decode, downscaled, run through the pipeline
//!   C. today's behaviour — extract the embedded JPEG and show it as-is
//!
//! `cargo run --release -p app --example bench_thumb_render <raw>`
use std::time::Instant;

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: bench_thumb_render <raw>");
    let path = std::path::Path::new(&path);
    // ⚠ Neutral by default: that isolates "the camera's rendering vs Rustroom's", which is the
    // residual difference a viewer sees when the decode replaces the placeholder. Pass any third
    // argument to use a strong edit instead.
    let edits = if std::env::args().nth(3).is_some() {
        edit_model::Edits {
            saturation: -100.0,
            exposure: 0.8,
            ..Default::default()
        }
    } else {
        edit_model::Edits::default()
    };

    // C — today.
    let t = Instant::now();
    let thumb = raw_decode::extract_thumbnail(path).expect("embedded thumbnail");
    let c = t.elapsed();
    println!(
        "C  extract embedded          {:>8.1} ms  ({}x{})",
        c.as_secs_f64() * 1000.0,
        thumb.width,
        thumb.height
    );

    // A — pipeline over the embedded JPEG. ⚠ Extraction timed separately: the second read hits
    // a warm file cache, so subtracting one whole-run time from another understates the render.
    let t = Instant::now();
    let thumb = raw_decode::extract_thumbnail(path).expect("embedded thumbnail");
    let extract = t.elapsed();
    let t = Instant::now();
    let mut buf = core_image::ImageBuf::new(thumb.width, thumb.height);
    for (i, px) in thumb.rgba.as_chunks::<4>().0.iter().enumerate() {
        let d = buf.data_mut();
        for c in 0..3 {
            d[i * 3 + c] = color::srgb_to_linear(px[c] as f32 / 255.0);
        }
    }
    let scale = pipeline::RenderScale::new(1.0);
    let out =
        pipeline::build_pipeline(&edits, scale, pipeline::TailRendering::colorimetric()).run(buf);
    let rgb_a = out.to_rgb8();
    let (aw, ah) = (out.width(), out.height());
    let render = t.elapsed();
    let a = extract + render;
    println!(
        "A  embedded + pipeline       {:>8.1} ms  (extract {:.1} + render {:.1})",
        a.as_secs_f64() * 1000.0,
        extract.as_secs_f64() * 1000.0,
        render.as_secs_f64() * 1000.0
    );

    // B — half-size decode, downscaled to thumbnail size, then the pipeline.
    let t = Instant::now();
    let dec = raw_decode::decode_raw(path, &raw_decode::DecodeOptions::preview()).expect("decode");
    let decoded = t.elapsed();
    let buf = dec.image;
    let long = buf.width().max(buf.height()) as f32;
    let small = buf.downscale_to(512);
    let scale = pipeline::RenderScale::new(0.5 * 512.0 / long);
    let out =
        pipeline::build_pipeline(&edits, scale, pipeline::TailRendering::colorimetric()).run(small);
    let rgb_b = out.to_rgb8();
    let (bw, bh) = (out.width(), out.height());
    let b = t.elapsed();
    println!(
        "B  half decode + pipeline    {:>8.1} ms  (decode {:.1} + rest {:.1})",
        b.as_secs_f64() * 1000.0,
        decoded.as_secs_f64() * 1000.0,
        (b - decoded).as_secs_f64() * 1000.0
    );

    println!(
        "\nB is {:.1}x the cost of A.",
        b.as_secs_f64() / a.as_secs_f64()
    );

    // Write both so the *fidelity* question can be judged by looking, not by argument.
    let dir = std::env::args().nth(2).unwrap_or_else(|| ".".into());
    image::save_buffer(
        format!("{dir}/thumb_A_from_embedded.png"),
        &rgb_a,
        aw as u32,
        ah as u32,
        image::ColorType::Rgb8,
    )
    .unwrap();
    image::save_buffer(
        format!("{dir}/thumb_B_from_decode.png"),
        &rgb_b,
        bw as u32,
        bh as u32,
        image::ColorType::Rgb8,
    )
    .unwrap();
    println!("wrote thumb_A_from_embedded.png and thumb_B_from_decode.png to {dir}");

    // ⚠ How far apart are they? This is what a viewer sees when the decoded render replaces the
    // placeholder on the canvas.
    // ⚠ The two can differ by a row: the camera's embedded JPEG is cropped slightly tighter than
    // libraw's active area (6240x4160 against 6246x4170 on this body), so compare the overlap.
    // ⚠ The two can differ by a row or a column: the camera's embedded JPEG is cropped slightly
    // tighter than libraw's active area (6240x4160 against 6246x4170 on this body), so compare
    // the shared top-left region rather than refusing.
    let (w, h) = (aw.min(bw), ah.min(bh));
    let mut sum = 0u64;
    let mut max = 0u8;
    let mut over8 = 0usize;
    for y in 0..h {
        for x in 0..w {
            for c in 0..3 {
                let d = rgb_a[(y * aw + x) * 3 + c].abs_diff(rgb_b[(y * bw + x) * 3 + c]);
                sum += d as u64;
                max = max.max(d);
                if d > 8 {
                    over8 += 1;
                }
            }
        }
    }
    let n = w * h * 3;
    println!(
        "A vs B over {w}x{h}: mean {:.2}/255, max {max}, {:.1}% of samples differ by more than 8",
        sum as f64 / n as f64,
        100.0 * over8 as f64 / n as f64
    );
}
