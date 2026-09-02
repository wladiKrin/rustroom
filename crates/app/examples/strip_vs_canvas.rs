//! Is the filmstrip thumbnail brighter than the canvas shows?
//!
//! ⚠ For an **unedited** photo `load_thumbnail` returns the camera's embedded JPEG untouched —
//! it takes an early return rather than paying a pipeline round trip 830 times for a no-op. So
//! the strip shows the *camera's* rendering while the canvas shows Rustroom's own scene-linear
//! one, and this reports how far apart they are and in which direction.
//!
//! ```text
//! cargo run --release -p app --example strip_vs_canvas <raw>
//! ```

fn mean_luma(rgb: &[f32]) -> f32 {
    let n = rgb.len() / 3;
    (0..n)
        .map(|i| color::luminance([rgb[i * 3], rgb[i * 3 + 1], rgb[i * 3 + 2]]))
        .sum::<f32>()
        / n as f32
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: strip_vs_canvas <raw>");
    let path = std::path::Path::new(&path);

    // The strip: the camera's embedded JPEG, exactly as an unedited row shows it.
    let thumb = raw_decode::extract_thumbnail(path).expect("thumbnail");
    let strip: Vec<f32> = thumb
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| (0..3).map(move |c| p[c] as f32 / 255.0))
        .collect();

    // The canvas: a neutral render through the real pipeline, then the output transform.
    let d =
        raw_decode::decode_raw(path, &raw_decode::DecodeOptions::linear_scene()).expect("decode");
    let long = thumb.width.max(thumb.height);
    let small = d.image.downscale_to(long);
    let out = pipeline::build_pipeline(
        &edit_model::Edits::default(),
        pipeline::RenderScale::new(1.0),
    )
    .run(small);
    let canvas: Vec<f32> = out.data().to_vec();

    let (a, b) = (mean_luma(&strip), mean_luma(&canvas));
    println!(
        "strip  (camera JPEG)   mean luma {a:.4}   {}x{}",
        thumb.width, thumb.height
    );
    println!(
        "canvas (our render)    mean luma {b:.4}   {}x{}",
        out.width(),
        out.height()
    );
    println!(
        "the strip is {:.1}% {} than the canvas  ({:+.2} EV)",
        100.0 * (a / b - 1.0).abs(),
        if a > b { "BRIGHTER" } else { "darker" },
        (a / b).log2()
    );
}
