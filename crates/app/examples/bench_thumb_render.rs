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
    let path = std::env::args().nth(1).expect("usage: bench_thumb_render <raw>");
    let path = std::path::Path::new(&path);
    let edits = edit_model::Edits {
        saturation: -100.0,
        exposure: 0.8,
        ..Default::default()
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
    for (i, px) in thumb.rgba.chunks_exact(4).enumerate() {
        let d = buf.data_mut();
        for c in 0..3 {
            d[i * 3 + c] = color::srgb_to_linear(px[c] as f32 / 255.0);
        }
    }
    let scale = pipeline::RenderScale::new(1.0);
    let out = pipeline::build_pipeline(&edits, scale).run(buf);
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
    let out = pipeline::build_pipeline(&edits, scale).run(small);
    let rgb_b = out.to_rgb8();
    let (bw, bh) = (out.width(), out.height());
    let b = t.elapsed();
    println!(
        "B  half decode + pipeline    {:>8.1} ms  (decode {:.1} + rest {:.1})",
        b.as_secs_f64() * 1000.0,
        decoded.as_secs_f64() * 1000.0,
        (b - decoded).as_secs_f64() * 1000.0
    );

    println!("\nB is {:.1}x the cost of A.", b.as_secs_f64() / a.as_secs_f64());

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
}
