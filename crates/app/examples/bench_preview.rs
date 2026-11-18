//! Times the preview render path (the same `render_once` the GUI worker calls) for a
//! handful of edit sets. Run: `cargo run --release -p app --example bench_preview <raw>`.
use edit_model::{Edits, NoiseReduction};
use pipeline::PipelineStage;
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
                            flow: 100.0,
                            erase: false,
                            ..Default::default()
                        }],
                        invert: false,
                    }),
                    refine: Vec::new(),
                    adjustments: edit_model::LocalAdjustments {
                        exposure: 0.5,
                        ..Default::default()
                    },
                    disabled: false,
                    invert: false,
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
        (
            "colour grading (split tone)",
            Edits {
                color_grading: edit_model::ColorGrading {
                    shadows: edit_model::GradeWheel {
                        hue: 240.0,
                        saturation: 70.0,
                        luminance: -20.0,
                    },
                    highlights: edit_model::GradeWheel {
                        hue: 45.0,
                        saturation: 70.0,
                        luminance: 15.0,
                    },
                    ..Default::default()
                },
                ..Edits::default()
            },
        ),
    ];

    for (label, edits) in cases {
        let mut best = f64::MAX;
        for _ in 0..5 {
            let t = Instant::now();
            let f = render_once(
                &buf,
                &edits,
                PREVIEW_MAX_DIM,
                scale,
                color::Display::SRGB,
                pipeline::Resampler::default(),
                None,
            );
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
        let (prefix, tail) =
            build_pipeline_split(&heavy, sc, pipeline::TailRendering::colorimetric());
        let mid = prefix.run(buf.downscale_to(PREVIEW_MAX_DIM));

        let mut best = f64::MAX;
        for _ in 0..5 {
            let t = Instant::now();
            let out = tail.run(mid.clone());
            let mut rgba = Vec::with_capacity(out.width() * out.height() * 4);
            for px in out.data().as_chunks::<3>().0 {
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
        let (prefix, _) = build_pipeline_split_in_region(
            &heavy,
            sc,
            None,
            pipeline::TailRendering::colorimetric(),
        );
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
        let (_, tail) = build_pipeline_split_in_region(
            &heavy,
            sc,
            Some(region),
            pipeline::TailRendering::colorimetric(),
        );
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
        let steps = build_prefix_steps(&heavy, sc, None, pipeline::Resampler::Bilinear, None, None);
        let (_, tail) = build_pipeline_split_in_region(
            &heavy,
            sc,
            None,
            pipeline::TailRendering::colorimetric(),
        );
        println!("\n  prefix steps: {}", steps.len());

        // Build every checkpoint once.
        let mut checkpoints = vec![buf.downscale_to(PREVIEW_MAX_DIM)];
        for step in &steps {
            let next = step.stage.apply(checkpoints.last().unwrap().clone());
            checkpoints.push(next);
        }

        // Resuming at checkpoint `start` means running steps[start..] plus the tail and pack.
        // ⚠⚠ **These labels were hardcoded and WRONG.** `build_prefix_steps` pushes a stage only
        // when it is active, so the list's length and order depend on the edits — and this printed
        // "geometry change" for a list that contains no geometry at all. Derived from the steps
        // now, which is the same rule `EDIT_GROUP_NAMES` and the GPU hint string already follow.
        for start in 0..=steps.len() {
            let label = if start == steps.len() {
                "tail slider (nothing in the prefix re-runs)".to_string()
            } else {
                format!("drag `{}` (resume before it)", steps[start].stage.name())
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
                for px in out.data().as_chunks::<3>().0 {
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

    // ⚠⚠ **The same decomposition at a NATIVE rung, which is the one that decides whether the
    // prefix is worth moving to the GPU.** The block above runs at `PREVIEW_MAX_DIM`; with
    // `drag_quality: Full` a fitted photo on a large HiDPI window renders at **rung 3200**, so a
    // Dehaze drag pays these numbers rather than those. The prefix also runs **whole-buffer**
    // whatever the zoom, so there is no region to shrink it.
    {
        use pipeline::{build_pipeline_split_in_region, build_prefix_steps, RenderScale};
        let buffer_long = buf.width().max(buf.height());
        let sc = RenderScale::new(scale);
        let heavy = Edits {
            noise_reduction: NoiseReduction {
                luminance: 50.0,
                color: 50.0,
                ..Default::default()
            },
            dehaze: edit_model::Dehaze { amount: 60.0 },
            exposure: 0.5,
            ..Edits::default()
        };
        let steps = build_prefix_steps(&heavy, sc, None, pipeline::Resampler::Bilinear, None, None);
        let (_, tail) = build_pipeline_split_in_region(
            &heavy,
            sc,
            None,
            pipeline::TailRendering::colorimetric(),
        );
        let base = buf.downscale_to(buffer_long);
        println!(
            "\n  prefix steps at a NATIVE rung ({}x{}, {:.1} Mpx): {}",
            base.width(),
            base.height(),
            (base.width() * base.height()) as f64 / 1e6,
            steps.len()
        );

        let mut checkpoints = vec![base];
        for step in &steps {
            let next = step.stage.apply(checkpoints.last().unwrap().clone());
            checkpoints.push(next);
        }
        // ⚠⚠ **Labels DERIVED from the steps, never enumerated** — the block below this one
        // hardcoded three names against a list whose length depends on the edits, so it printed
        // "geometry change" for whatever happened to be first. A label that enumerates a list
        // goes wrong the moment the list changes, which is this file's most-repeated finding.
        for start in 0..=steps.len() {
            let label = if start == steps.len() {
                "tail slider (nothing in the prefix re-runs)".to_string()
            } else {
                format!("drag `{}` (resume before it)", steps[start].stage.name())
            };
            let mut best = f64::MAX;
            for _ in 0..5 {
                let t = Instant::now();
                let mut img = checkpoints[start].clone();
                for step in &steps[start..] {
                    img = step.stage.apply(img);
                }
                let out = tail.run(img);
                let ms = t.elapsed().as_secs_f64() * 1000.0;
                std::hint::black_box(&out);
                best = best.min(ms);
            }
            println!("{best:8.1} ms   {label}");
        }

        // ⚠⚠ **The same steps with the GPU seam offered, which is what M8.3o is judged by.** This
        // project has over-predicted a whole frame by ~2x (M8.3g) and been right in magnitude
        // while wrong by 8x in attribution (M8.3f) by composing component measurements — so the
        // real thing is measured here rather than added up from `box_probe`'s rows.
        if let Some(ctx) = gpu_pipeline::shared() {
            println!("  with the GPU seam offered ({}):", ctx.adapter_name());
            for (i, step) in steps.iter().enumerate() {
                let name = step.stage.name();
                let Some(g) = gpu_pipeline::accelerate_prefix_stage(
                    name,
                    &heavy,
                    sc,
                    &checkpoints[i],
                    // ⚠ No lens profile here: these fixtures carry no camera metadata, so
                    // `lens_correction` is not among the steps and the seam is never asked
                    // about it. `probe_lens` is what prices that stage.
                    gpu_pipeline::SeamFacts {
                        lens: None,
                        resampler: pipeline::Resampler::Bilinear,
                        // ⚠ The replay airlight, as the engine supplies it — not an estimate from
                        // the checkpoint, which is the defect fixed on 2026-09-17.
                        airlight: pipeline::prefix_airlight(
                            &heavy,
                            &checkpoints[i],
                            pipeline::Resampler::Bilinear,
                            None,
                        ),
                        ai: None,
                    },
                    ctx,
                ) else {
                    println!("           —    `{name}` has no GPU twin");
                    continue;
                };
                let mut best = f64::MAX;
                for _ in 0..5 {
                    let t = Instant::now();
                    let out = g.apply(checkpoints[i].clone());
                    best = best.min(t.elapsed().as_secs_f64() * 1000.0);
                    std::hint::black_box(&out);
                }
                println!("{best:8.1} ms   `{name}` ALONE on the GPU");
            }
        }

        // ⚠⚠ **The WHOLE dehaze-drag frame, with the tail accelerated too — the thing the user
        // actually waits for.** The rows above price one stage each; this prices the frame, which
        // is what "still sluggish" is about. It also counts the **round trips**, because the
        // strong hypothesis is that there are TWO — one for the prefix step and one for the tail —
        // and that more than half the frame is moving bytes rather than computing.
        if let Some(ctx) = gpu_pipeline::shared() {
            let gpu_dehaze = steps
                .iter()
                .enumerate()
                .find(|(_, s)| s.stage.name() == "dehaze")
                .and_then(|(i, s)| {
                    gpu_pipeline::accelerate_prefix_stage(
                        s.stage.name(),
                        &heavy,
                        sc,
                        &checkpoints[i],
                        gpu_pipeline::SeamFacts {
                            lens: None,
                            resampler: pipeline::Resampler::Bilinear,
                            // ⚠ The replay airlight, as the engine supplies it — not an estimate from
                            // the checkpoint, which is the defect fixed on 2026-09-17.
                            airlight: pipeline::prefix_airlight(
                                &heavy,
                                &checkpoints[i],
                                pipeline::Resampler::Bilinear,
                                None,
                            ),
                            ai: None,
                        },
                        ctx,
                    )
                    .map(|g| (i, g))
                });
            if let Some((i, g)) = gpu_dehaze {
                let (t2, chain) = gpu_pipeline::accelerate(
                    build_pipeline_split_in_region(
                        &heavy,
                        sc,
                        None,
                        pipeline::TailRendering::colorimetric(),
                    )
                    .1,
                    &heavy,
                    sc,
                    gpu_pipeline::TailFacts {
                        region: None,
                        rendering: pipeline::TailRendering::default(),
                        spec: pipeline::RenderSpec::default(),
                        // ⚠ The bench applies its prefix stage by hand, without offering.
                    },
                    ctx,
                );
                let t2 = gpu_pipeline::rejoin(t2, chain);
                let mut best = f64::MAX;
                for _ in 0..5 {
                    let t = Instant::now();
                    let mid = g.apply(checkpoints[i].clone());
                    let out = t2.run(mid);
                    best = best.min(t.elapsed().as_secs_f64() * 1000.0);
                    std::hint::black_box(&out);
                }
                println!("{best:8.1} ms   DEHAZE DRAG, prefix step AND tail on the GPU");
            }
        }

        // ⚠⚠ **The NR DRAG, which is the row M8.3q is judged by and which no existing row can
        // see.** Noise reduction is the FIRST prefix step, so moving its slider re-runs NR *and*
        // dehaze *and* the tail — 216.4 ms on the CPU against a Dehaze drag's 132.0, which makes
        // it the most expensive drag anywhere in the prefix. The rows above price one stage each;
        // this prices the frame, which is what a photographer waits for.
        if let Some(ctx) = gpu_pipeline::shared() {
            let from = steps
                .iter()
                .position(|s| s.stage.name() == "noise_reduction")
                .filter(|_| !steps.is_empty());
            if let Some(from) = from {
                let (t2, chain) = gpu_pipeline::accelerate(
                    build_pipeline_split_in_region(
                        &heavy,
                        sc,
                        None,
                        pipeline::TailRendering::colorimetric(),
                    )
                    .1,
                    &heavy,
                    sc,
                    gpu_pipeline::TailFacts {
                        region: None,
                        rendering: pipeline::TailRendering::default(),
                        spec: pipeline::RenderSpec::default(),
                        // ⚠ The bench applies its prefix stage by hand, without offering.
                    },
                    ctx,
                );
                let t2 = gpu_pipeline::rejoin(t2, chain);
                // ⚠ Every prefix step from noise reduction onward, each offered the seam — that
                // is what a drag actually re-runs, and it is why a per-stage number understates
                // it. A step with no twin stays on the CPU here, exactly as it would in the app.
                let mut best = f64::MAX;
                for _ in 0..5 {
                    let t = Instant::now();
                    let mut img = checkpoints[from].clone();
                    for step in steps.iter().skip(from) {
                        img = match gpu_pipeline::accelerate_prefix_stage(
                            step.stage.name(),
                            &heavy,
                            sc,
                            &img,
                            gpu_pipeline::SeamFacts {
                                lens: None,
                                resampler: pipeline::Resampler::Bilinear,
                                // ⚠ The replay airlight, as the engine supplies it — not an estimate from
                                // the checkpoint, which is the defect fixed on 2026-09-17.
                                airlight: pipeline::prefix_airlight(
                                    &heavy,
                                    &img,
                                    pipeline::Resampler::Bilinear,
                                    None,
                                ),
                                ai: None,
                            },
                            ctx,
                        ) {
                            Some(g) => g.apply(img),
                            None => step.stage.apply(img),
                        };
                    }
                    let out = t2.run(img);
                    best = best.min(t.elapsed().as_secs_f64() * 1000.0);
                    std::hint::black_box(&out);
                }
                println!("{best:8.1} ms   NR DRAG, prefix steps AND tail on the GPU");
            }
        }

        // ⚠ Each prefix stage alone, with no tail, so the spec prices what a GPU run would
        // actually replace rather than inferring it from a difference of two totals.
        for (i, step) in steps.iter().enumerate() {
            let mut best = f64::MAX;
            for _ in 0..5 {
                let t = Instant::now();
                let out = step.stage.apply(checkpoints[i].clone());
                best = best.min(t.elapsed().as_secs_f64() * 1000.0);
                std::hint::black_box(&out);
            }
            println!("{best:8.1} ms   {} ALONE (whole buffer)", step.stage.name());
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
                flow: 100.0,
                erase: false,
                ..Default::default()
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
        // ⚠ The number the live-rendering decision rests on: a full-resolution frame with the
        // coverage clipped to a viewport-sized region, which is what painting at 1:1 now does.
        let mut best = f64::MAX;
        for _ in 0..5 {
            let t = std::time::Instant::now();
            let cov = pipeline::brush::rasterize_in(
                &brush,
                &g,
                6246,
                4170,
                Some((2400, 1600, 1400, 900)),
                None,
            );
            std::hint::black_box(&cov);
            best = best.min(t.elapsed().as_secs_f64() * 1000.0);
        }
        println!("{best:8.2} ms   brush rasterize CLIPPED to 1400x900, full 6246x4170");
    }

    // ⚠⚠ **THE SWEEP THAT SETS `SCOPE_MAX_AREA_PCT`, and the row M8.7 is judged by.** Every row
    // above prices a *stage*; this prices a **drag frame** — the whole chain from the resume point
    // plus the tail — the whole-buffer way and the region-scoped way, at several region sizes.
    //
    // ⚠ A constant nobody can re-measure is a constant nobody can defend. `MIN_GPU_BOX_PX` shipped
    // low by ~8x and `MAX_TILE_DABS` was sized by a *limit* rather than by the data and measured 5x
    // slower than the CPU it replaced; both were reasoned about instead of swept.
    //
    // ⚠⚠ The margined share is what the gate reads, **not** the visible share. A local's reach is
    // in full-resolution pixels, so at a native rung a modest viewport margins out a long way —
    // which is exactly why a share threshold rather than an absolute size.
    //
    // Measured on `DSCF6296.RAF`, chain `perspective -> geometry -> noise_reduction -> dehaze`,
    // dragging `geometry`, frame 3123x2085, best of 3 (⚠ load average 3.6, not a quiet machine):
    //
    //   region      margined   scoped    whole-buffer
    //    10%           2.9%      8.7 ms      200.4 ms   22.9x
    //    30%          13.8%     34.5        204.8        5.94x
    //    50%          32.7%     78.6        222.4        2.83x
    //    60%          45.1%    109.4        213.1        1.95x   <- SCOPE_MAX_AREA_PCT sits here
    //    70%          59.6%    146.2        218.3        1.49x
    //    85%          85.0%    203.3        226.7        1.12x
    //   100%         100.0%    236.6        229.9        0.97x
    //
    // ⚠⚠ Parity on the frame alone is at ~100% and that is the **wrong** number to set the gate
    // from, because the scoped output is never cached — see `SCOPE_MAX_AREA_PCT`'s own doc for the
    // per-gesture arithmetic that puts the threshold at 50 instead.
    {
        use pipeline::{
            build_pipeline_split_in_region, build_prefix_steps, region_plan, run_region_plan, Rect,
            Region, RenderScale,
        };
        let buffer_long = buf.width().max(buf.height());
        let sc = RenderScale::new(scale);
        // The reporter's chain, minus lens correction — no profile is available here, and it is a
        // gather like the two below it, so the chain's *shape* is the same: gathers then locals.
        let heavy = Edits {
            perspective: edit_model::Perspective {
                vertical: 12.0,
                ..Default::default()
            },
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
            ..Edits::default()
        };
        // ⚠ Without a supplied airlight `DehazeStage` declines a region outright, and the sweep
        // would silently measure a chain that stops one step short.
        let airlight = pipeline::prefix_airlight(&heavy, &buf, pipeline::Resampler::Bilinear, None);
        let steps = build_prefix_steps(
            &heavy,
            sc,
            None,
            pipeline::Resampler::Bilinear,
            Some(airlight),
            None,
        );
        let base = buf.downscale_to(buffer_long);

        // A Straighten drag: everything above `geometry` stays cached, everything from it onward
        // re-runs. That is the gesture the milestone exists for.
        let start = steps
            .iter()
            .position(|s| s.stage.name() == "geometry")
            .unwrap_or(0);
        let mut warm = base.clone();
        for s in &steps[..start] {
            warm = s.stage.apply(warm.clone());
        }
        let frame = {
            let (mut w, mut h) = (warm.width(), warm.height());
            for s in &steps[start..] {
                let (ow, oh) = s.stage.output_dims(w, h);
                w = ow;
                h = oh;
            }
            (w, h)
        };
        println!(
            "\n  SCOPED DRAG SWEEP — chain {:?}, dragging `{}`, frame {}x{} ({:.1} Mpx)",
            steps.iter().map(|s| s.stage.name()).collect::<Vec<_>>(),
            steps[start].stage.name(),
            frame.0,
            frame.1,
            (frame.0 * frame.1) as f64 / 1e6,
        );

        for pct in [10usize, 20, 30, 40, 50, 60, 70, 85, 100] {
            // A viewport-shaped rect centred in the frame, `pct` of each axis.
            let rw = (frame.0 * pct / 100).max(1);
            let rh = (frame.1 * pct / 100).max(1);
            let visible = Rect {
                x: (frame.0 - rw) / 2,
                y: (frame.1 - rh) / 2,
                w: rw,
                h: rh,
            };
            let Some(plan) = region_plan(&steps, start, visible, warm.width(), warm.height())
            else {
                println!("      —        {rw}x{rh}: the chain declined a region");
                continue;
            };
            let margined = plan.out_rects[0].w as u64 * plan.out_rects[0].h as u64;
            let share = margined as f64 * 100.0 / (frame.0 as u64 * frame.1 as u64) as f64;

            let region = Region {
                full_w: frame.0,
                full_h: frame.1,
                x0: visible.x,
                y0: visible.y,
            };
            let (_, tail) = build_pipeline_split_in_region(
                &heavy,
                sc,
                Some(region),
                pipeline::TailRendering::colorimetric(),
            );

            let mut whole_ms = f64::MAX;
            for _ in 0..3 {
                let t = Instant::now();
                let mut img = warm.clone();
                for s in &steps[start..] {
                    img = s.stage.apply(img);
                }
                let sub = img.crop(visible.x, visible.y, visible.w, visible.h);
                std::hint::black_box(tail.run(sub));
                whole_ms = whole_ms.min(t.elapsed().as_secs_f64() * 1000.0);
            }

            let mut scoped_ms = f64::MAX;
            for _ in 0..3 {
                let t = Instant::now();
                let out = run_region_plan(&steps, start, &warm, &plan).expect("the plan runs");
                std::hint::black_box(tail.run(out));
                scoped_ms = scoped_ms.min(t.elapsed().as_secs_f64() * 1000.0);
            }

            println!(
                "{scoped_ms:8.1} ms   region {rw}x{rh} ({pct}% of each axis), margined {:.1}% of the frame  \
                 — whole-buffer {whole_ms:8.1} ms, {:.2}x",
                share,
                whole_ms / scoped_ms,
            );
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
