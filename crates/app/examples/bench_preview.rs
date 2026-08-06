//! Times the preview render path (the same `render_once` the GUI worker calls) for a
//! handful of edit sets. Run: `cargo run --release -p app --example bench_preview <raw>`.
use edit_model::{Edits, NoiseReduction};
use rustroom_app::render_engine::render_once;
use std::time::Instant;

const PREVIEW_MAX_DIM: usize = 1600;

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: bench_preview <file>");
    let path = std::path::Path::new(&path);
    let opts = raw_decode::DecodeOptions::preview();
    let d = raw_decode::decode_raw(path, &opts).expect("decode");
    let buf = d.image;
    let scale = opts.source_scale();
    println!(
        "buffer {}x{} (source_scale {scale})",
        buf.width(),
        buf.height()
    );

    let nr = |l: f32, c: f32| Edits {
        noise_reduction: NoiseReduction {
            luminance: l,
            color: c,
            ..Default::default()
        },
        ..Edits::default()
    };
    let mut hsl = edit_model::Hsl::default();
    hsl.bands[0].saturation = 30.0;

    let cases: Vec<(&str, Edits)> = vec![
        ("neutral (output transform only)", Edits::default()),
        ("NR color 50 only", nr(0.0, 50.0)),
        ("NR luminance 50 only", nr(50.0, 0.0)),
        ("NR both 50", nr(50.0, 50.0)),
        (
            "vignette -60",
            Edits {
                vignette: edit_model::Vignette {
                    amount: -60.0,
                    ..Default::default()
                },
                ..Edits::default()
            },
        ),
        (
            "HSL only (for comparison)",
            Edits {
                hsl,
                ..Edits::default()
            },
        ),
    ];

    for (label, edits) in cases {
        let mut best = f64::MAX;
        for _ in 0..5 {
            let t = Instant::now();
            let f = render_once(&buf, &edits, PREVIEW_MAX_DIM, scale);
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            std::hint::black_box(&f);
            best = best.min(ms);
        }
        println!("{best:8.1} ms   {label}");
    }

    // The cached path: what a slider move actually costs once the prefix is warm. Timed
    // through the public split rather than through RenderEngine, so the number is the work
    // itself and not thread hand-off noise.
    {
        use pipeline::{build_pipeline_split, RenderScale};
        let buffer_long = buf.width().max(buf.height());
        let sc = RenderScale::new(scale * PREVIEW_MAX_DIM as f32 / buffer_long as f32);
        let heavy = Edits {
            noise_reduction: edit_model::NoiseReduction {
                luminance: 50.0,
                color: 50.0,
                ..Default::default()
            },
            exposure: 0.5,
            ..Edits::default()
        };
        let (prefix, tail) = build_pipeline_split(&heavy, sc);
        let mid = prefix.run(buf.downscale_to(PREVIEW_MAX_DIM));

        let mut best = f64::MAX;
        for _ in 0..5 {
            let t = Instant::now();
            let out = tail.run(mid.clone());
            let mut rgba = Vec::with_capacity(out.width() * out.height() * 4);
            for px in out.data().chunks_exact(3) {
                rgba.push((px[0].clamp(0.0, 1.0) * 255.0).round() as u8);
                rgba.push((px[1].clamp(0.0, 1.0) * 255.0).round() as u8);
                rgba.push((px[2].clamp(0.0, 1.0) * 255.0).round() as u8);
                rgba.push(255);
            }
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            std::hint::black_box(&rgba);
            best = best.min(ms);
        }
        println!("{best:8.1} ms   NR both 50 + exposure, CACHE HIT");
    }
}
