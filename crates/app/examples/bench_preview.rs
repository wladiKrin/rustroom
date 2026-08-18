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
            "brush ~50 dabs",
            Edits {
                masks: vec![edit_model::MaskedAdjustment {
                    mask: edit_model::Mask::Brush(edit_model::Brush {
                        // ⚠ Two endpoints, not 50 literal points: the rasterizer interpolates
                        // dabs at a quarter-radius spacing, so a stroke spanning 0.6 of the frame
                        // at radius 0.02 yields roughly 50 dabs — which is what costs time.
                        // Recording 50 points would measure something the app never produces,
                        // because painting decimates to exactly this spacing.
                        strokes: vec![edit_model::BrushStroke {
                            points: vec![[0.2, 0.5], [0.8, 0.5]],
                            radius: 0.02,
                            hardness: 50.0,
                            erase: false,
                        }],
                        invert: false,
                    }),
                    adjustments: edit_model::LocalAdjustments {
                        exposure: 0.5,
                        ..Default::default()
                    },
                }],
                ..Edits::default()
            },
        ),
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
            "sharpening 70 (amount/radius/detail)",
            Edits {
                sharpening: edit_model::Sharpening {
                    amount: 70.0,
                    ..Default::default()
                },
                ..Edits::default()
            },
        ),
        (
            "sharpening 70 + masking 60 (second gaussian)",
            Edits {
                sharpening: edit_model::Sharpening {
                    amount: 70.0,
                    masking: 60.0,
                    ..Default::default()
                },
                ..Edits::default()
            },
        ),
        (
            "texture 70 (small gaussian)",
            Edits {
                local_contrast: edit_model::LocalContrast {
                    texture: 70.0,
                    clarity: 0.0,
                },
                ..Edits::default()
            },
        ),
        (
            "clarity 70 (wide_blur, 3 box passes)",
            Edits {
                local_contrast: edit_model::LocalContrast {
                    texture: 0.0,
                    clarity: 70.0,
                },
                ..Edits::default()
            },
        ),
        (
            "texture 70 + clarity 70 (both bands)",
            Edits {
                local_contrast: edit_model::LocalContrast {
                    texture: 70.0,
                    clarity: 70.0,
                },
                ..Edits::default()
            },
        ),
        (
            "dehaze 60 (prefix: min filter + guided filter)",
            Edits {
                dehaze: edit_model::Dehaze { amount: 60.0 },
                ..Edits::default()
            },
        ),
        (
            "dehaze 60 + NR both 50 (both prefix stages)",
            Edits {
                dehaze: edit_model::Dehaze { amount: 60.0 },
                noise_reduction: NoiseReduction {
                    luminance: 50.0,
                    color: 50.0,
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

    // A region tail render: what a slider move costs while zoomed in, once the native
    // prefix is warm.
    {
        use pipeline::{build_pipeline_split_in_region, Region, RenderScale};
        let buffer_long = buf.width().max(buf.height());
        let sc = RenderScale::new(scale);
        let heavy = Edits {
            noise_reduction: edit_model::NoiseReduction {
                luminance: 50.0,
                color: 50.0,
                ..Default::default()
            },
            exposure: 0.5,
            ..Edits::default()
        };
        let t0 = Instant::now();
        let (prefix, _) = build_pipeline_split_in_region(&heavy, sc, None);
        let mid = prefix.run(buf.downscale_to(buffer_long));
        println!(
            "{:8.1} ms   native prefix, ONE-OFF ({}x{})",
            t0.elapsed().as_secs_f64() * 1000.0,
            mid.width(),
            mid.height()
        );

        let (rw, rh) = (1400.min(mid.width()), 900.min(mid.height()));
        let region = Region {
            full_w: mid.width(),
            full_h: mid.height(),
            x0: 0,
            y0: 0,
        };
        let sub = mid.crop(0, 0, rw, rh);
        let (_, tail) = build_pipeline_split_in_region(&heavy, sc, Some(region));
        let mut best = f64::MAX;
        for _ in 0..5 {
            let t = Instant::now();
            std::hint::black_box(tail.run(sub.clone()));
            best = best.min(t.elapsed().as_secs_f64() * 1000.0);
        }
        println!("{best:8.1} ms   region tail {rw}x{rh}, CACHE HIT");
    }

    // What each prefix checkpoint actually saves. Timed through the public `build_prefix_steps`
    // rather than through `RenderEngine`, so the number is the work itself and not thread
    // hand-off noise — the same choice the cache-hit section above makes.
    //
    // Measured on the X-T4 `.RAF` (3123x2085 half-size buffer -> 1600 px preview, release,
    // best of 5) with geometry, noise reduction and dehaze all active:
    //
    //   101.2 ms  geometry change  (resume at the downscale)
    //    87.5 ms  NR change        (resume after geometry)  -- sheds the 13.7 ms rotation
    //    49.7 ms  dehaze change    (resume after NR)        -- was 95.4 ms whole, 1.9x
    //    14.4 ms  tail slider      (resume at the final)
    //
    // The dehaze line is the feature: it is what a Dehaze drag costs per frame with noise
    // reduction on, against the "dehaze 60 + NR both 50" whole-render line above.
    {
        use pipeline::{build_pipeline_split_in_region, build_prefix_steps, RenderScale};
        let buffer_long = buf.width().max(buf.height());
        let sc = RenderScale::new(scale * PREVIEW_MAX_DIM as f32 / buffer_long as f32);
        let heavy = Edits {
            geometry: edit_model::Geometry {
                angle: 3.0,
                ..Default::default()
            },
            noise_reduction: NoiseReduction {
                luminance: 50.0,
                color: 50.0,
                ..Default::default()
            },
            dehaze: edit_model::Dehaze { amount: 60.0 },
            exposure: 0.5,
            ..Edits::default()
        };
        let steps = build_prefix_steps(&heavy, sc);
        let (_, tail) = build_pipeline_split_in_region(&heavy, sc, None);
        println!("\n  prefix steps: {}", steps.len());

        // Build every checkpoint once.
        let mut checkpoints = vec![buf.downscale_to(PREVIEW_MAX_DIM)];
        for step in &steps {
            let next = step.stage.apply(checkpoints.last().unwrap().clone());
            checkpoints.push(next);
        }

        // Resuming at checkpoint `start` means running steps[start..] plus the tail and pack.
        for start in 0..=steps.len() {
            let label = match start {
                0 => "geometry change  (resume at the downscale)",
                1 => "NR change        (resume after geometry)",
                2 => "dehaze change    (resume after NR)",
                _ => "tail slider      (resume at the final)",
            };
            let mut best = f64::MAX;
            for _ in 0..5 {
                let t = Instant::now();
                let mut img = checkpoints[start].clone();
                for step in &steps[start..] {
                    img = step.stage.apply(img);
                }
                let out = tail.run(img);
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
            println!("{best:8.1} ms   {label}");
        }
    }

    // ⚠ The rasterizer alone, at the frame sizes that actually matter. The cases above run
    // against the half-size decode; painting at 1:1 on a full-resolution buffer rasterizes at the
    // FULL frame size, which is where the incremental-splatting question lives.
    {
        let brush = edit_model::Brush {
            strokes: vec![edit_model::BrushStroke {
                points: vec![[0.2, 0.5], [0.8, 0.5]],
                radius: 0.02,
                hardness: 50.0,
                erase: false,
            }],
            invert: false,
        };
        let g = edit_model::Geometry::default();
        println!();
        for (label, fw, fh) in [
            ("fit 1600x1068", 1600usize, 1068usize),
            ("half 3123x2085", 3123, 2085),
            ("full 6246x4170", 6246, 4170),
        ] {
            let mut best = f64::MAX;
            for _ in 0..5 {
                let t = std::time::Instant::now();
                let cov = pipeline::brush::rasterize(&brush, &g, fw, fh);
                std::hint::black_box(&cov);
                best = best.min(t.elapsed().as_secs_f64() * 1000.0);
            }
            println!("{best:8.2} ms   brush rasterize, {label}");
        }
    }

    // The RGBA8 pack, at both render resolutions. It runs on every frame, including the ones
    // the prefix checkpoints made cheap, so it is measured on its own rather than buried in a
    // whole-frame number.
    //
    // Measured on the X-T4 `.RAF` (release, best of 9). Two independent multipliers, kept
    // separate so the credit goes where it is due:
    //
    //                                       fit 1600x1068   native 3123x2085
    //   `Vec::push` loop (the original)          4.59 ms          17.81 ms
    //   preallocate + sequential fill            1.16 ms           4.42 ms
    //   + parallel in 8192-px blocks             0.46 ms           2.21 ms
    //
    // ⚠ Most of the win is the first step, not the threading — `Vec::push` costs a capacity
    // check per byte and does not vectorise. And ⚠ per-*pixel* rayon granularity measured
    // 1.64 ms, i.e. slower than not threading at all; see `PACK_BLOCK_PX` in `core-image`.
    {
        let long = buf.width().max(buf.height());
        for (label, max_dim) in [("fit", PREVIEW_MAX_DIM), ("native", long)] {
            let img = buf.downscale_to(max_dim);
            let mut best = f64::MAX;
            for _ in 0..9 {
                let t = Instant::now();
                let bytes = img.to_rgba8();
                let ms = t.elapsed().as_secs_f64() * 1000.0;
                std::hint::black_box(&bytes);
                best = best.min(ms);
            }
            println!(
                "{best:8.2} ms   RGBA8 pack, {label} ({}x{})",
                img.width(),
                img.height()
            );
        }
    }
}
