//! What does `gpu_tail` save on a **real frame**, through the app's own render path?
//!
//! ⚠⚠ **Written because `gpu_run_bench` and the app disagree.** That bench times a bare tail on a
//! 1600×1068 synthetic ramp and reports 2.05×; a user with the toggle on could not see a
//! difference. The lesson this repo already records is *a synthetic bench prices a stage, not a
//! session* — the interactive-rung work found `bench_masked` understating the app by **2.6×**,
//! because a real frame carries the prefix, the pack, the upload and a whole tail rather than
//! three stages.
//!
//! So this drives `RenderEngine` itself — request, poll, the real `render_cached`, the real prefix
//! cache, the real `pack` — on a real RAW, at the rungs the app actually uses.
//!
//! `cargo run --release -p app --example bench_gpu_tail <raw>`

use edit_model::{ColorGrading, Edits, GradeWheel, Sharpening};
use rustroom_app::render_engine::{FrameSpec, RenderEngine};
use std::sync::Arc;
use std::time::Instant;

fn graded() -> ColorGrading {
    ColorGrading {
        shadows: GradeWheel {
            hue: 250.0,
            saturation: 60.0,
            luminance: -20.0,
        },
        highlights: GradeWheel {
            hue: 150.0,
            saturation: 45.0,
            luminance: 15.0,
        },
        blending: 35.0,
        balance: -25.0,
        ..Default::default()
    }
}

fn hsl() -> edit_model::Hsl {
    let mut h = edit_model::Hsl::default();
    for (i, b) in h.bands.iter_mut().enumerate() {
        b.saturation = (i as f32 * 21.0) % 90.0 - 45.0;
        b.hue = (i as f32 * 13.0) % 70.0 - 35.0;
    }
    h
}

fn sharp() -> Sharpening {
    Sharpening {
        amount: 70.0,
        radius: 1.5,
        detail: 50.0,
        masking: 40.0,
    }
}

/// A mask carrying exactly what it is handed, under a full-frame linear gradient.
///
/// ⚠ A gradient rather than a brush on purpose: M8.3i's decomposition puts a gradient's coverage
/// at **0.00 ms**, so these rows measure the sub-stages and nothing else. A brush would bury the
/// effect under coverage rasterisation, which is the column M8.3i already attacked.
fn masked(a: edit_model::LocalAdjustments) -> edit_model::MaskedAdjustment {
    edit_model::MaskedAdjustment {
        mask: edit_model::Mask::Linear(edit_model::LinearGradient {
            from: [0.5, 0.0],
            to: [0.5, 1.0],
            invert: false,
        }),
        refine: Vec::new(),
        adjustments: a,
        disabled: false,
        invert: false,
    }
}

/// One render through the engine, waited for. Returns the wall time of the whole frame.
///
/// ⚠ The engine coalesces, so a request must be **polled to completion** before the next is
/// issued or the measurement times a frame that was skipped.
fn frame_ms(engine: &RenderEngine, edits: &Edits, spec: FrameSpec) -> f64 {
    let t = Instant::now();
    engine.request(edits.clone(), spec);
    loop {
        if engine.poll_latest().is_some() {
            return t.elapsed().as_secs_f64() * 1000.0;
        }
        std::thread::sleep(std::time::Duration::from_micros(200));
    }
}

/// Best of `n` **warm** rounds — round 0 is discarded because the first encode compiles shaders.
fn best(engine: &RenderEngine, edits: &Edits, spec: FrameSpec, n: usize) -> f64 {
    let mut b = f64::MAX;
    for r in 0..n {
        let ms = frame_ms(engine, edits, spec);
        if r > 0 {
            b = b.min(ms);
        }
    }
    b
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: bench_gpu_tail <raw>");
    let path = std::path::Path::new(&path);

    // ⚠ `preview()` — the half-size decode the app opens with, not a full one. Timing the full
    // decode's buffer would measure a rung the app only reaches after the handover.
    let opts = raw_decode::DecodeOptions::preview();
    let d = raw_decode::decode_raw(path, &opts).expect("decode");
    let (bw, bh) = (d.image.width(), d.image.height());
    let buffer = Arc::new(d.image);
    println!("buffer {bw}x{bh}, source_scale {}", opts.source_scale());
    match gpu_pipeline::shared() {
        Some(c) => println!("adapter: {} ({})\n", c.adapter_name(), c.backend()),
        None => {
            println!("no GPU adapter — nothing to compare");
            return;
        }
    }

    // ⚠⚠ **The engine gets the real looks directory, or the creative-look row cannot exist.** The
    // look is resolved on the render worker from `Edits.look.file`, so a bench without a
    // `looks_dir` would render that row with **no look at all** and report a speed-up for a stage
    // that never ran — the W5 failure mode as a benchmark.
    let looks = dirs::config_dir().map(|c| rustroom_app::looks::LookStore::user_default(&c));
    let first_look = looks.as_ref().and_then(|s| s.list().first().cloned());
    let engine = RenderEngine::new(
        buffer,
        opts.source_scale(),
        None,
        d.metadata.as_shot.unwrap_or_default(),
        rustroom_app::decode_worker::ProfileParts {
            looks_dir: looks.as_ref().map(|s| s.dir().to_path_buf()),
            ..Default::default()
        },
        true,
        || {},
    );

    let mut three = Edits {
        hsl: hsl(),
        sharpening: sharp(),
        ..Edits::default()
    };
    three.color_grading = graded();
    let hsl_only = Edits {
        hsl: hsl(),
        ..Edits::default()
    };

    // ⚠⚠ **The case that actually matters, taken from the user's own 40 sidecars rather than
    // invented.** Dehaze is active on 100% of them, exposure on 57%, blacks 55%, whites 50% —
    // and the three accelerated stages on 5%, 2.5% and 2.5%. A bench that only measures the
    // stages we ported measures a photograph nobody here edits.
    let mut typical = Edits {
        exposure: 0.45,
        whites: 25.0,
        blacks: -20.0,
        dehaze: edit_model::Dehaze { amount: 35.0 },
        ..Edits::default()
    };
    // ⚠⚠ **The camera rendering, which this case was missing and which changes what it measures.**
    // `Edits::default()` leaves `rendering` *undecided*, and undecided applies as OFF — but a
    // fresh photo is seeded `Some(Camera)` by `Settings::default_rendering`, and it is active on
    // **100% of this catalogue's 40 sidecars**. Without it this row measured a tail with no
    // display rendering in it: exactly the stage M8.3e exists for.
    typical.rendering = Some(edit_model::Rendering::Camera);

    // ⚠ And the neutral row gets it too, for the same reason: "neutral" for this user means an
    // unedited photo *with* the camera rendering, not a colorimetric one. The colorimetric case is
    // the one the cost gate deliberately leaves on the CPU, and it is named separately below.
    let neutral_rendered = Edits {
        rendering: Some(edit_model::Rendering::Camera),
        ..Edits::default()
    };

    // ⚠⚠ **The case M8.3j exists for.** Masked HSL is the one reported in use as slow, and
    // M8.3i's decomposition put its sub-stages at 68.8 ms of an 82 ms mask — the largest single
    // number left in Phase 8.
    let masked_hsl = Edits {
        masks: vec![masked(edit_model::LocalAdjustments {
            hsl: hsl(),
            ..Default::default()
        })],
        ..Edits::default()
    };
    // ⚠⚠ **The only rows that can see M8.11's composite fold.** Every other masked row carries a
    // bare base, which the twin already accelerated before the fold landed — so a bench without a
    // composite would report the fold as worth nothing, the asymmetry M8.3j recorded when its own
    // bench could only see fixtures.
    //
    // ⚠ Three colour ranges specifically: that is the shape the hoist is for, since the CPU pays
    // an Oklab round trip per component per pixel and the GPU pays one for the chain.
    let colour_range = |low: f32, high: f32| {
        edit_model::Mask::Colour(edit_model::ColourRange {
            low,
            high,
            feather: 30.0,
            min_chroma: 0.02,
            invert: false,
        })
    };
    let composite_ranges = Edits {
        masks: vec![{
            let mut m = masked(edit_model::LocalAdjustments {
                exposure: 0.8,
                ..Default::default()
            });
            m.mask = colour_range(0.0, 60.0);
            m.refine = vec![
                edit_model::Refinement {
                    op: edit_model::MaskOp::Add,
                    mask: colour_range(100.0, 160.0),
                },
                edit_model::Refinement {
                    op: edit_model::MaskOp::Subtract,
                    mask: colour_range(200.0, 260.0),
                },
            ];
            m
        }],
        ..Edits::default()
    };
    // ⚠ A shape refined by a range, which is the ordinary authoring gesture: darken the sky with a
    // gradient, then keep only the blues.
    let composite_mixed = Edits {
        masks: vec![{
            let mut m = masked(edit_model::LocalAdjustments {
                hsl: hsl(),
                ..Default::default()
            });
            m.refine = vec![edit_model::Refinement {
                op: edit_model::MaskOp::Intersect,
                mask: colour_range(150.0, 260.0),
            }];
            m
        }],
        ..Edits::default()
    };
    let masked_hsl_x3 = Edits {
        masks: (0..3)
            .map(|_| {
                masked(edit_model::LocalAdjustments {
                    hsl: hsl(),
                    ..Default::default()
                })
            })
            .collect(),
        ..Edits::default()
    };
    // ⚠⚠ **The control: a mask whose sub-stages the cost gate must REFUSE.** 1.3 ms of work against
    // a 27.0 ms transfer, so this row must not move — one that got faster would mean the gate was
    // reaching the wrong verdict inside a mask.
    //
    // ⚠ It was once described here as "what the catalogue's one masked photograph carries", and
    // that stopped being true on 2026-09-12 when M8.3j's own confirmation check added an HSL to
    // that mask. **The REAL SIDECAR row below is the one that tracks the catalogue; this one is a
    // fixture and is meant to stay fixed.**
    let masked_exposure = Edits {
        masks: vec![masked(edit_model::LocalAdjustments {
            exposure: -3.6,
            ..Default::default()
        })],
        ..Edits::default()
    };

    // ⚠⚠ **The real sidecar, if one sits beside the RAW — and it is the only row here that is not
    // invented.** Every other case is a fixture chosen to isolate a stage; this one is whatever the
    // photographer actually saved, masks included. `mask_breakdown` has carried such a row since
    // M8.3i and this bench did not, which is why M8.3j's first record quoted a synthetic figure for
    // the catalogue's one masked photograph and got it wrong.
    let side = path.with_file_name(format!(
        "{}.json",
        path.file_name().unwrap().to_string_lossy()
    ));
    let real: Option<Edits> = std::fs::read_to_string(&side).ok().and_then(|t| {
        let v: serde_json::Value = serde_json::from_str(&t).ok()?;
        let e = v.get("copies")?.get(0)?.get("edits")?.clone();
        serde_json::from_value(e).ok()
    });
    if let Some(e) = &real {
        println!(
            "real sidecar: {} mask(s){}\n",
            e.masks.len(),
            if e.masks.is_empty() {
                " — this photo exercises no mask row"
            } else {
                ""
            }
        );
    } else {
        println!("no sidecar beside this RAW — the REAL row is skipped\n");
    }

    // ⚠⚠ **The row M8.3m exists for, and it is deliberately NOT a catalogue row.** **0 of 40
    // sidecars use white balance, contrast, the tone curve or saturation globally** — measured
    // before M8.3k, and why plan 2 waited three milestones. What this prices is the **shape**: a
    // photo whose per-pixel head is complete. Before M8.3m `contrast` and `tone_curve` sat between
    // ported stages and split that head into `[exposure]` at 1.3 ms and `[tone_regions]` at 18.0,
    // each refused by the gate — so 19.3 ms of already-ported work was stranded on the CPU by a
    // 1 ms neighbour. Pinned as a decision by
    // `the_per_pixel_head_becomes_one_run_once_every_member_is_ported`.
    let full_head = Edits {
        white_balance: edit_model::WhiteBalance {
            kelvin: None,
            tint: 8.0,
        },
        exposure: 0.4,
        contrast: 25.0,
        highlights: -30.0,
        shadows: 20.0,
        tone_curve: edit_model::ToneCurve {
            points: vec![[0.0, 0.0], [0.3, 0.42], [1.0, 1.0]],
        },
        saturation: 20.0,
        vibrance: 15.0,
        rendering: Some(edit_model::Rendering::Camera),
        ..Edits::default()
    };

    // ⚠⚠ **The rows M8.3n exists for, and like the head row they are deliberately NOT catalogue
    // rows.** Re-scanned at M8.3n: **0 of 40 sidecars use Texture and 1 uses Clarity at -5.0**, so
    // no existing row can see this milestone — the asymmetry M8.3j recorded as the whole mechanism
    // of its own wrong caveat. Split by band and by **sign**, because positive Texture measures
    // against a Gaussian and negative against a guided filter: two different amounts of work
    // behind one slider, measured at 17.9 and 38.7 ms.
    let texture_minus = Edits {
        local_contrast: edit_model::LocalContrast {
            texture: -60.0,
            clarity: 0.0,
        },
        ..Edits::default()
    };
    let local_contrast_both = Edits {
        local_contrast: edit_model::LocalContrast {
            texture: 60.0,
            clarity: 50.0,
        },
        ..Edits::default()
    };
    // ⚠ The head **through** local contrast: before M8.3n this stage sat between `tone_curve` and
    // `saturation` and split the per-pixel head in two, so this is the row that prices the split
    // closing rather than the stage's own cost.
    let head_through_local = Edits {
        local_contrast: edit_model::LocalContrast {
            texture: 60.0,
            clarity: 50.0,
        },
        ..full_head.clone()
    };

    // ⚠⚠ **The rows M8.3p exists for, and like M8.3n's they are deliberately NOT catalogue rows.**
    // Re-scanned at M8.3o: **0 of 40 sidecars use the vignette and 0 use a creative look** — the
    // look is `null` in every one. No existing row can see this milestone.
    //
    // ⚠ The vignette is split by shape because `probe_stages` measured the two branches **5x
    // apart**: 8.57-9.05 ms elliptical against 40.44-45.51 for the superellipse, a `sqrt` against
    // two `powf` and a reciprocal exponent. One row would describe neither.
    let vignette_elliptical = Edits {
        vignette: edit_model::Vignette {
            amount: -60.0,
            ..Default::default()
        },
        ..Edits::default()
    };
    let vignette_superellipse = Edits {
        vignette: edit_model::Vignette {
            amount: -60.0,
            roundness: -60.0,
            ..Default::default()
        },
        ..Edits::default()
    };
    // ⚠ The head **through** the vignette and the look: before this milestone each of them split
    // the tail, and the look additionally left a lone `display_rendering` run the gate refused
    // outright. This row prices the splits closing rather than either stage's own cost.
    let whole_tail = Edits {
        vignette: edit_model::Vignette {
            amount: -60.0,
            ..Default::default()
        },
        look: edit_model::Look {
            file: first_look.as_ref().map(|e| e.file.clone()),
            space: edit_model::LutSpace::SrgbDisplay,
            strength: 100.0,
        },
        ..full_head.clone()
    };
    let look_only = Edits {
        look: edit_model::Look {
            file: first_look.as_ref().map(|e| e.file.clone()),
            space: edit_model::LutSpace::SrgbDisplay,
            strength: 100.0,
        },
        ..Edits::default()
    };

    let colorimetric = Edits::default();
    let mut cases: Vec<(&str, &Edits)> = vec![
        ("TYPICAL of this catalogue", &typical),
        ("the whole per-pixel head", &full_head),
        ("texture -60 (guided)", &texture_minus),
        ("texture +60 + clarity 50", &local_contrast_both),
        ("the head THROUGH local contrast", &head_through_local),
        ("unedited, camera rendering", &neutral_rendered),
        ("unedited, colorimetric", &colorimetric),
        ("hsl only", &hsl_only),
        ("hsl + grade + sharpening", &three),
        ("1 masked HSL (gradient)", &masked_hsl),
        ("3 masked HSL (gradient)", &masked_hsl_x3),
        ("COMPOSITE: 3 colour ranges", &composite_ranges),
        ("COMPOSITE: gradient ∩ colour range", &composite_mixed),
        ("1 masked exposure — the control", &masked_exposure),
        ("vignette -60 (elliptical)", &vignette_elliptical),
        ("vignette -60 roundness -60", &vignette_superellipse),
    ];
    // ⚠ Conditional, and it says so rather than silently vanishing: a look row with no `.cube`
    // installed would measure a tail with no look in it and report a meaningless ratio.
    match &first_look {
        Some(e) => {
            println!(
                "look: {} (from {:?})\n",
                e.file,
                looks.as_ref().map(|s| s.dir())
            );
            cases.push(("creative look (sRGB display)", &look_only));
            cases.push(("the WHOLE tail", &whole_tail));
        }
        None => println!("⚠ no .cube installed — the creative-look rows are SKIPPED, not zero\n"),
    }

    // ⚠⚠ Two rungs, because they are different questions. **1600 is the fit texture**; **3200 is
    // what a fitted photo actually renders at on a large HiDPI window** — the interactive-rung
    // work measured exactly that, 6.8 Mpx against the fit rung's 1.7, and it is the rung this
    // user sees with `drag_quality: Full`.
    for &max_dim in &[1600usize, 3200] {
        let mpx = {
            let long = bw.max(bh) as f64;
            let k = (max_dim as f64 / long).min(1.0);
            (bw as f64 * k) * (bh as f64 * k) / 1.0e6
        };
        println!("── rung {max_dim} — whole image, ~{mpx:.1} Mpx");
        println!("  case                              cpu        gpu     speed-up   saved");
        for (label, edits) in cases.iter().copied() {
            let off = FrameSpec {
                max_dim,
                region: None,
                display_space: color::Display::SRGB,
                intent: color::RenderingIntent::default(),
                resampler: pipeline::Resampler::default(),
                gpu_tail: false,
                // ⚠ The three view modes off: each adds work of its own (the grade range view is
                // ~9 ms at fit) and would blur what is being measured.
                grade_view: None,
                soft_proof: None,
                visualize: None,
            };
            let on = FrameSpec {
                gpu_tail: true,
                ..off
            };
            // ⚠ Warm each configuration's prefix cache *and* its shaders before timing, then
            // alternate — a cold prefix would swamp a tail difference entirely.
            let c = best(&engine, edits, off, 5);
            let g = best(&engine, edits, on, 5);
            println!(
                "  {label:<32} {c:7.2} ms {g:7.2} ms   {:5.2}x  {:+6.2} ms",
                c / g,
                c - g
            );
        }
        // ⚠ Last, and separated, because it is the only row describing a real photograph rather
        // than a fixture built to isolate a stage.
        if let Some(e) = &real {
            let off = FrameSpec {
                max_dim,
                region: None,
                display_space: color::Display::SRGB,
                intent: color::RenderingIntent::default(),
                resampler: pipeline::Resampler::default(),
                gpu_tail: false,
                grade_view: None,
                soft_proof: None,
                visualize: None,
            };
            let on = FrameSpec {
                gpu_tail: true,
                ..off
            };
            let c = best(&engine, e, off, 5);
            let g = best(&engine, e, on, 5);
            println!(
                "  {:<32} {c:7.2} ms {g:7.2} ms   {:5.2}x  {:+6.2} ms",
                format!("REAL SIDECAR, {} mask(s)", e.masks.len()),
                c / g,
                c - g
            );

            // ⚠⚠ **The row above is a cache HIT and cannot show brush coverage at all.** `best`
            // renders the same edits five times, so the auto-mask coverage cache serves every
            // frame after the first and coverage costs nothing — which is exactly the caveat
            // M8.3j's record carries about it.
            //
            // ⚠ Coverage is only re-rasterised when a **global** edit changes the base the gate
            // reads, because `BrushCache` declines an auto-masked brush and the coverage cache's
            // key is invalidated by any global change. So this row varies the exposure between
            // frames — which is what a drag is — and it is the only row that prices the work
            // M8.3k does.
            // ⚠⚠ **The two configurations walk DISJOINT exposure ladders, and the row is
            // worthless without it.** `MaskCoverageKey::base` is the truncated `Edits` plus the
            // rung; `gpu_tail` lives in `FrameSpec` and is **not** in `Edits`, so rendering the
            // same exposure off-then-on inside one round made the CPU frame populate the coverage
            // cache and the GPU frame read it for free — `cov=0.00` on every accelerated render
            // against `cov=62` on every CPU one. The row then reported a tail difference plus a
            // whole rasterization the GPU never paid, which is the opposite of what it claims to
            // price. The 0.003 EV offset is far below any visible difference and costs the
            // same work; all it does is make the two keys distinct.
            let mut drag_off = f64::MAX;
            let mut drag_on = f64::MAX;
            for r in 0..6 {
                let mut d = e.clone();
                d.exposure = 0.05 * (r as f32 + 1.0);
                let a = frame_ms(&engine, &d, off);
                d.exposure += 0.003;
                let b = frame_ms(&engine, &d, on);
                if r > 0 {
                    drag_off = drag_off.min(a);
                    drag_on = drag_on.min(b);
                }
            }
            println!(
                "  {:<32} {drag_off:7.2} ms {drag_on:7.2} ms   {:5.2}x  {:+6.2} ms",
                "REAL SIDECAR, global drag",
                drag_off / drag_on,
                drag_off - drag_on
            );
        }
        println!();
    }

    println!(
        "⚠ These are WHOLE FRAMES through the real engine — prefix (cached), tail, pack.\n\
         ⚠ `gpu_run_bench` times the bare tail on a synthetic ramp and will read higher."
    );
}
