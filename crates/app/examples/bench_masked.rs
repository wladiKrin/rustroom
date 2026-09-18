//! Times masked spatial adjustments — the M7.2 question: what does it cost to put Clarity,
//! Texture, Sharpening or HSL *inside* a mask, and does that argue for a bounding-box
//! optimisation or for a hue LUT?
//!
//! Run: `cargo run --release -p app --example bench_masked <raw>`
//!
//! ⚠⚠ **The masked NR and dehaze rows are the COLD cost, and the two columns say opposite
//! things about them on purpose.** Both run in the **prefix**, so the `fit` loop — which calls
//! `render_once` and rebuilds the prefix every iteration — reports what it costs *when you move
//! that slider*. The `native region` loop times the tail alone on a warm prefix, so those two
//! rows read ~0, **which is the correct answer**: a slider that is not theirs re-renders from the
//! checkpoint after them and pays nothing. Read the fit column as "on change", not "per frame".
//!
//! ⚠ Two things this measures deliberately, because the decision turns on both:
//!
//! 1. **One mask against three.** `tail_kernel_radius` sums over masks and mask N runs on
//!    mask N-1's output, so the cost is linear in the number of masks with nothing shared.
//! 2. **A frame-covering linear gradient against a small radial.** They cost the *same* today,
//!    which is exactly the evidence that no bounding box exists — and the radial is the only
//!    one of the two a bounding box could ever help.
use edit_model::{
    ColorGrading, ColourRange, Dehaze, Edits, GradeWheel, Hsl, LinearGradient, LocalAdjustments,
    LocalContrast, LuminanceRange, Mask, MaskedAdjustment, NoiseReduction, RadialGradient,
    ToneCurve,
};
use pipeline::PipelineStage;
use rustroom_app::render_engine::render_once;
use std::time::Instant;

const PREVIEW_MAX_DIM: usize = 1600;

/// A mask spanning the frame top to bottom. No bounding box can shrink this one — it is the
/// honest worst case, and the shape a "darken the sky" gradient really has.
fn gradient() -> Mask {
    Mask::Linear(LinearGradient {
        from: [0.5, 0.0],
        to: [0.5, 1.0],
        invert: false,
    })
}

/// A small radial — a face, a flower. ⚠ This is the ONLY mask kind in this file a bounding box
/// would help: its bbox is 0.24 x 0.24 of the frame, i.e. ~5.8% of the pixels.
fn radial() -> Mask {
    Mask::Radial(RadialGradient {
        center: [0.5, 0.5],
        radius: [0.12, 0.12],
        feather: 50.0,
        invert: false,
    })
}

/// A luminance range, at its default band.
///
/// ⚠⚠ **Its coverage is evaluated per pixel, and `lightness` runs `linear_to_srgb` — a `powf` —
/// on every one of them.** A bounding box is worth *nothing* here by construction: the selected
/// pixels are wherever that tonal band happens to fall, which is typically scattered over the
/// whole frame. If the cheap adjustments are slow under this mask and fast under a gradient, the
/// coverage evaluation is the cost and no bbox can touch it.
fn lum_range() -> Mask {
    Mask::Luminance(LuminanceRange::default())
}

/// A colour range, at its default 0°–120° band.
///
/// ⚠ **The most expensive coverage of the four**: `prophoto_to_oklab` is three `cbrt`, then
/// `atan2` and `hypot`, per pixel, per frame. `HslStage`'s own decomposition put the Oklab round
/// trip at ~27% of that stage and `atan2` at ~13% — this mask pays both before any adjustment
/// runs at all.
fn colour_range() -> Mask {
    Mask::Colour(ColourRange::default())
}

/// A painted brush — one diagonal stroke across the frame.
///
/// ⚠⚠ **THIS ROW DOES NOT MEASURE WHAT THE APP PAYS, AND MUST NOT BE READ AS IF IT DID.** Both
/// loops below build their pipelines with **no prebuilt coverage**, so `MaskStage::apply`
/// rasterizes the stroke *on every single call*. The running app never does that: its
/// `BrushCache` rasterizes once and hands the map in. Read this row as the price of the cache —
/// what a brush would cost without it — and not as a drag frame.
fn brush() -> Mask {
    Mask::Brush(edit_model::Brush {
        strokes: vec![edit_model::BrushStroke {
            points: (0..40)
                .map(|i| [i as f32 / 40.0, i as f32 / 40.0])
                .collect(),
            radius: 0.06,
            ..Default::default()
        }],
        invert: false,
    })
}

/// The same stroke with Auto Mask on.
///
/// ⚠⚠ It differs from [`brush`] in **exactly one thing**, so the two rows subtract to the gate's
/// own cost. ⚠ That is *not* the cost of `BrushCache` declining a gating brush: both loops here
/// build their pipelines with no prebuilt coverage, so neither can see the cache at all. The
/// refusal's price is the [`brush`] row itself, which an auto-masked brush pays on every render
/// in addition to this.
fn auto_brush() -> Mask {
    let Mask::Brush(mut b) = brush() else {
        unreachable!()
    };
    b.strokes[0].auto_mask = true;
    b.strokes[0].tolerance = 40.0;
    Mask::Brush(b)
}

fn clarity() -> LocalAdjustments {
    LocalAdjustments {
        local_contrast: LocalContrast {
            texture: 0.0,
            clarity: 70.0,
        },
        ..Default::default()
    }
}

/// A masked exposure — **the cheapest adjustment this pipeline has**, and the one reported as
/// stuttering. One sub-stage, per-pixel, no table, no kernel, no neighbourhood.
///
/// ⚠⚠ **That is exactly what makes it the diagnostic row.** Every other masked row in this file
/// carries something expensive, so its cost tells you about the adjustment. This one has almost
/// no adjustment in it, so whatever it costs above the `global exposure` row **is the mask
/// machinery itself** — the clone, the coverage evaluation and the blend — with nothing else to
/// hide behind.
fn exposure() -> LocalAdjustments {
    LocalAdjustments {
        exposure: 0.5,
        ..Default::default()
    }
}

fn hsl_adjust() -> LocalAdjustments {
    let mut hsl = Hsl::default();
    hsl.bands[0].saturation = 30.0;
    LocalAdjustments {
        hsl,
        ..Default::default()
    }
}

/// A masked tone curve. ⚠ `ToneCurveStage::new` builds a **1024**-entry LUT per construction,
/// against colour grading's 256 — so of the two stages Part A added, the curve is the larger
/// table build, which is the opposite of what the backlog entry implied.
fn curve() -> LocalAdjustments {
    LocalAdjustments {
        tone_curve: ToneCurve {
            points: vec![[0.0, 0.0], [0.4, 0.6], [1.0, 1.0]],
        },
        ..Default::default()
    }
}

/// A masked split tone. ⚠ The **third** consumer of `ColorGradingStage::new`, whose 256-entry
/// table is built per stage — i.e. per mask per render rather than per pixel.
fn grade() -> LocalAdjustments {
    LocalAdjustments {
        color_grading: ColorGrading {
            shadows: GradeWheel {
                hue: 240.0,
                saturation: 45.0,
                luminance: -10.0,
            },
            highlights: GradeWheel {
                hue: 50.0,
                saturation: 40.0,
                luminance: 8.0,
            },
            ..ColorGrading::default()
        },
        ..Default::default()
    }
}

/// A masked denoise. ⚠⚠ **Whole-buffer by construction** — it runs in the *prefix*, so a small
/// radial pays for the entire frame. That is the strongest case yet made for the deferred
/// bounding box, and it is still not this milestone.
fn denoise() -> LocalAdjustments {
    LocalAdjustments {
        noise_reduction: NoiseReduction {
            luminance: 50.0,
            color: 50.0,
            ..NoiseReduction::default()
        },
        ..Default::default()
    }
}

/// A masked dehaze. ⚠ Its airlight comes from the whole frame, which is why it is in the prefix.
fn haze() -> LocalAdjustments {
    LocalAdjustments {
        dehaze: Dehaze { amount: 60.0 },
        ..Default::default()
    }
}

/// One mask whose base is a gradient, with `n` refinements of one kind composited onto it.
///
/// ⚠⚠ **The row this milestone exists to price.** Read every composite row against
/// `1 masked exposure (gradient)`, which is the same adjustment over the same pixels with no
/// refinement at all — the gap *is* the per-component cost, and nothing else here answers it.
///
/// ⚠ `Add` throughout, so coverage widens rather than narrowing: a `Subtract` row would shrink the
/// area the sub-stages blend into and confound the component's cost with a smaller effect.
///
/// ⚠⚠ **MEASURED (X-T4 DSCF6223, 3123x2085 buffer, release, best of 5), and it overturned the
/// design's own prediction in two places.** Against `1 masked exposure (gradient)`:
///
/// | extra component | fit @1600 | region 1400x900 |
/// |---|---|---|
/// | + 1 radial | +3.7 ms | +2.2 ms |
/// | + 1 luminance | **+7.1 ms** | +4.7 ms |
/// | + 1 colour | +16.5 ms | +13.6 ms |
/// | + 3 colour | +32.1 ms | +20.1 ms |
/// | + 1 brush (uncached) | +12.5 ms | +13.5 ms |
///
/// **(1) "A luminance component is free once the base facts are shared" was FALSE.** The base here
/// is a *gradient*, which computes no lightness at all — so the luminance component is the
/// **first** consumer and has nothing to share with. Sharing pays from the *second*
/// pixel-dependent component onward, never on the first. "Shared" is not "free".
///
/// **(2) "Three colour components cost one Oklab round trip" is TRUE and the inference from it was
/// wrong.** The conversion counter proves exactly one conversion; the cost still climbs +16.5 →
/// +32.1, i.e. ~+7.8 ms per extra component. So `ColourRange::coverage_at` — the band weighting,
/// the smoothstep, the chroma gate — is the dominant per-component cost, not the conversion.
/// `docs/BACKLOG.md` puts the round trip at 42% of *one* range mask's coverage; shared across
/// three, the remaining 58% x 3 dominates.
///
/// ⚠ What the sharing is actually worth, **measured by disabling the memo** rather than modelled:
///
/// | `+ 3 colour` | with sharing | without | saving |
/// |---|---|---|---|
/// | fit | 50.2 ms | 65.2 ms | **15.0 ms (23%)** |
/// | region | 30.7 ms | 45.3 ms | **14.6 ms (32%)** |
///
/// and `+ 1 colour` measures 34.6 against 35.0 — identical within noise, which is exactly what a
/// memo with nothing to share must do, and is what confirms the mechanism rather than a
/// coincidence.
fn composite(n: usize, refine: fn() -> Mask, adj: fn() -> LocalAdjustments) -> Edits {
    Edits {
        masks: vec![MaskedAdjustment {
            mask: gradient(),
            refine: (0..n)
                .map(|_| edit_model::Refinement {
                    op: edit_model::MaskOp::Add,
                    mask: refine(),
                })
                .collect(),
            adjustments: adj(),
        }],
        ..Edits::default()
    }
}

fn masked(n: usize, mask: fn() -> Mask, adj: fn() -> LocalAdjustments) -> Edits {
    Edits {
        masks: (0..n)
            .map(|_| MaskedAdjustment {
                mask: mask(),
                refine: Vec::new(),
                adjustments: adj(),
            })
            .collect(),
        ..Edits::default()
    }
}

/// The same exposure move with **no mask around it**.
///
/// ⚠⚠ **The reference row this file has never had, and without it a masked delta means nothing.**
/// Every other line here is measured against `neutral`, which answers "what does this feature
/// cost". The stutter question is a different one — "what does the *mask* cost" — and only the
/// gap between this row and `1 masked exposure` answers it, because both apply the identical
/// adjustment to the identical pixels.
fn global_exposure() -> Edits {
    Edits {
        exposure: 0.5,
        ..Edits::default()
    }
}

fn best_of<F: FnMut() -> T, T>(mut f: F) -> f64 {
    let mut best = f64::MAX;
    for _ in 0..5 {
        let t = Instant::now();
        let v = f();
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        std::hint::black_box(&v);
        best = best.min(ms);
    }
    best
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: bench_masked <raw>");
    let path = std::path::Path::new(&path);
    let opts = raw_decode::DecodeOptions::preview();
    let d = raw_decode::decode_raw(path, &opts).expect("decode");
    let buf = d.image;
    let scale = opts.source_scale();
    let buffer_long = buf.width().max(buf.height());
    println!(
        "buffer {}x{} (source_scale {scale})\n",
        buf.width(),
        buf.height()
    );

    let cases: Vec<(&str, Edits)> = vec![
        ("neutral", Edits::default()),
        // The stutter rows. ⚠ Read them against `global exposure`, not against `neutral`.
        ("global exposure  (no mask)", global_exposure()),
        (
            "1 masked exposure (gradient)",
            masked(1, gradient, exposure),
        ),
        (
            "3 masked exposure (gradient)",
            masked(3, gradient, exposure),
        ),
        (
            "1 masked exposure (small radial)",
            masked(1, radial, exposure),
        ),
        (
            "1 masked exposure (luminance range)",
            masked(1, lum_range, exposure),
        ),
        (
            "1 masked exposure (colour range)",
            masked(1, colour_range, exposure),
        ),
        (
            "1 masked exposure (brush) ⚠uncached",
            masked(1, brush, exposure),
        ),
        // ⚠⚠ **Read this against the row above, which is the same stroke ungated.** The
        // difference is the gate's own cost — the box's Oklab conversion plus a distance and a
        // smoothstep per dab per covered pixel — and NOT the cost of `BrushCache` declining it,
        // which neither loop here can see because both are uncached by construction. ⚠ An
        // auto-masked brush pays the row above *as well*, since its cache really is declined.
        (
            "1 masked exposure (brush, auto-masked)",
            masked(1, auto_brush, exposure),
        ),
        ("1 masked clarity  (gradient)", masked(1, gradient, clarity)),
        ("3 masked clarity  (gradient)", masked(3, gradient, clarity)),
        (
            "1 masked HSL      (gradient)",
            masked(1, gradient, hsl_adjust),
        ),
        (
            "3 masked HSL      (gradient)",
            masked(3, gradient, hsl_adjust),
        ),
        ("1 masked curve    (gradient)", masked(1, gradient, curve)),
        ("3 masked curve    (gradient)", masked(3, gradient, curve)),
        ("1 masked grade    (gradient)", masked(1, gradient, grade)),
        ("3 masked grade    (gradient)", masked(3, gradient, grade)),
        (
            "1 masked clarity  (small radial)",
            masked(1, radial, clarity),
        ),
        ("1 masked curve    (small radial)", masked(1, radial, curve)),
        ("1 masked grade    (small radial)", masked(1, radial, grade)),
        ("1 masked NR       (gradient)", masked(1, gradient, denoise)),
        ("1 masked dehaze   (gradient)", masked(1, gradient, haze)),
        (
            "1 masked NR       (small radial)",
            masked(1, radial, denoise),
        ),
        (
            "1 masked HSL      (small radial)",
            masked(1, radial, hsl_adjust),
        ),
        // ⚠⚠ **M7.1c: what one more component costs.** Against `1 masked exposure (gradient)`.
        // The prediction being checked, written before any of this was measured: an analytic
        // component is a few flops; a luminance component is free once the base facts are shared;
        // three colour components cost ONE Oklab round trip rather than three; a brush component
        // costs one more rasterisation.
        (
            "  + 1 radial       (composite)",
            composite(1, radial, exposure),
        ),
        (
            "  + 1 luminance    (composite)",
            composite(1, lum_range, exposure),
        ),
        (
            "  + 1 colour       (composite)",
            composite(1, colour_range, exposure),
        ),
        (
            "  + 3 colour       (composite)",
            composite(3, colour_range, exposure),
        ),
        (
            "  + 1 brush ⚠uncached (composite)",
            composite(1, brush, exposure),
        ),
    ];

    // --- Fit: a whole-image render at the preview cap, which is what a slider move costs
    // when the photo is shown fitted. Comparable to bench_preview's own lines.
    println!("  fit, whole image (render_once at {PREVIEW_MAX_DIM} px)");
    let mut fit_neutral = 0.0;
    for (label, edits) in &cases {
        let ms = best_of(|| {
            render_once(
                &buf,
                edits,
                PREVIEW_MAX_DIM,
                scale,
                color::OutputSpace::Srgb,
                pipeline::Resampler::default(),
                None,
            )
        });
        if *label == "neutral" {
            fit_neutral = ms;
        }
        println!("{ms:8.1} ms  ({:+6.1})  {label}", ms - fit_neutral);
    }

    // --- Native region: the tail alone on a warm prefix, which is what a slider move costs
    // while zoomed in. The prefix is identical for every case here — masks are tail-only —
    // so it is built once and the difference between lines is entirely the masks.
    {
        use pipeline::{build_pipeline_split_in_region, Region, RenderScale};
        let sc = RenderScale::new(scale);
        let (prefix, _) = build_pipeline_split_in_region(
            &Edits::default(),
            sc,
            None,
            pipeline::ProfileRendering::colorimetric(),
        );
        let mid = prefix.run(buf.downscale_to(buffer_long));
        let (rw, rh) = (1400.min(mid.width()), 900.min(mid.height()));
        let region = Region {
            full_w: mid.width(),
            full_h: mid.height(),
            x0: 0,
            y0: 0,
        };
        let sub = mid.crop(0, 0, rw, rh);
        println!("\n  native region tail {rw}x{rh} (warm prefix)");
        let mut reg_neutral = 0.0;
        for (label, edits) in &cases {
            let (_, tail) = build_pipeline_split_in_region(
                edits,
                sc,
                Some(region),
                pipeline::ProfileRendering::colorimetric(),
            );
            let ms = best_of(|| tail.run(sub.clone()));
            if *label == "neutral" {
                reg_neutral = ms;
            }
            println!("{ms:8.1} ms  ({:+6.1})  {label}", ms - reg_neutral);
        }

        // --- Does the cost scale with the pixels the mask runs over? That is the whole
        // bounding-box question: a bbox covering fraction f of the frame can only ever cut the
        // masked *delta* to about f times itself. Measured at two region sizes, a quarter apart.
        println!("\n  does the masked delta scale with area? (bounding-box premise)");
        for (label, w, h) in [
            ("1400x900", 1400.min(mid.width()), 900.min(mid.height())),
            ("700x450", 700.min(mid.width()), 450.min(mid.height())),
        ] {
            let region = Region {
                full_w: mid.width(),
                full_h: mid.height(),
                x0: 0,
                y0: 0,
            };
            let sub = mid.crop(0, 0, w, h);
            let base = {
                let (_, t) = build_pipeline_split_in_region(
                    &Edits::default(),
                    sc,
                    Some(region),
                    pipeline::ProfileRendering::colorimetric(),
                );
                best_of(|| t.run(sub.clone()))
            };
            let cl = {
                let e = masked(1, gradient, clarity);
                let (_, t) = build_pipeline_split_in_region(
                    &e,
                    sc,
                    Some(region),
                    pipeline::ProfileRendering::colorimetric(),
                );
                best_of(|| t.run(sub.clone())) - base
            };
            let hs = {
                let e = masked(1, gradient, hsl_adjust);
                let (_, t) = build_pipeline_split_in_region(
                    &e,
                    sc,
                    Some(region),
                    pipeline::ProfileRendering::colorimetric(),
                );
                best_of(|| t.run(sub.clone())) - base
            };
            println!("           {label:>9}: clarity {cl:+6.1} ms   HSL {hs:+6.1} ms");
        }

        // --- Where does a masked exposure's time actually GO? `MaskStage::apply` is three
        // things, and only one of them is the adjustment the user asked for. Timing them
        // separately means the total above can be *accounted for* rather than attributed by
        // eye — and this project has reasoned this area's cost wrong four times running.
        //
        // ⚠ The blend here reproduces `MaskStage`'s own closure for the analytic case. It is a
        // deliberate copy: the point is to price that exact loop, and calling the real stage
        // would fold all three parts back into one number again.
        println!("\n  where a masked exposure's time goes (region {rw}x{rh}, gradient)");
        let clone_ms = best_of(|| sub.clone());
        let exp = pipeline::ExposureStage::new(0.5);
        let exp_ms = best_of(|| exp.apply(sub.clone())) - clone_ms;
        let adjusted = exp.apply(sub.clone());
        let m = gradient();
        let (fw, fh) = (mid.width(), mid.height());
        let aspect = fw as f32 / fh as f32;
        let blend_ms = best_of(|| {
            sub.map_pixels_xy(|x, y, base| {
                let u = x as f32 / fw as f32;
                let v = y as f32 / fh as f32;
                let c = m
                    .analytic_coverage(u, v, aspect)
                    .unwrap_or(0.0)
                    .clamp(0.0, 1.0);
                let adj = adjusted.pixel(x, y);
                [
                    base[0] + (adj[0] - base[0]) * c,
                    base[1] + (adj[1] - base[1]) * c,
                    base[2] + (adj[2] - base[2]) * c,
                ]
            })
        });
        println!(
            "           clone {clone_ms:5.1}   exposure {exp_ms:5.1}   blend {blend_ms:5.1}   sum {:5.1} ms",
            clone_ms + exp_ms + blend_ms
        );
    }

    // --- WHAT A DRAG FRAME ACTUALLY COSTS, at fit, which is where the stutter was reported.
    //
    // ⚠⚠ **Neither column above measures this, and that is why this section exists.** The `fit`
    // column calls `render_once`, which rebuilds the **prefix** every iteration — the running app
    // caches it, so that column overstates a drag. The `native region` column times
    // `tail.run(..)` alone, which understates one: it stops at the pipeline and the app then
    // packs the result to RGBA8 and buckets a histogram **on every frame**, in `pack`.
    //
    // A drag frame is `tail + to_rgba8 + histogram`. Measuring the two halves separately is the
    // point: if the pack is comparable to the tail, then the *baseline* frame is expensive and no
    // amount of work on `MaskStage` can fix a stutter — which is exactly the reading the masked
    // rows above force, since a gradient-masked exposure adds under 3 ms.
    //
    // ⚠⚠ **AND IT SWEEPS THE RUNG, WHICH IS THE WHOLE POINT OF THE SECOND ROW OF NUMBERS.**
    // `fit_cap` is `min(buffer_long, 1600)`, but `tier_max_dim` climbs a doubling ladder as soon
    // as the viewport is bigger than the fit texture — so on a large or HiDPI window a photo that
    // *looks* fitted is rendered at a **native rung, as a region covering the whole frame**. Every
    // other number in this file is a 1600 px number; if the app is really sitting on the top rung,
    // none of them describe what the user is waiting for.
    {
        use pipeline::{build_pipeline_split_in_region, Region, RenderScale};
        let sc = RenderScale::new(scale);
        for rung in [PREVIEW_MAX_DIM, buffer_long] {
            let src = buf.downscale_to(rung);
            let (prefix, _) = build_pipeline_split_in_region(
                &Edits::default(),
                sc,
                None,
                pipeline::ProfileRendering::colorimetric(),
            );
            let mid = prefix.run(src);
            // At a native rung the app renders a *region*; with the whole photo visible that
            // region covers the whole frame, which is the case timed here.
            let region = (rung > PREVIEW_MAX_DIM).then_some(Region {
                full_w: mid.width(),
                full_h: mid.height(),
                x0: 0,
                y0: 0,
            });
            let mpx = (mid.width() * mid.height()) as f64 / 1e6;
            println!(
                "\n  a drag frame at rung {rung} ({}x{} = {mpx:.1} Mpx, warm prefix): tail + pack",
                mid.width(),
                mid.height()
            );
            for (label, edits) in [
                ("neutral", Edits::default()),
                ("global exposure", global_exposure()),
                ("masked exposure (gradient)", masked(1, gradient, exposure)),
                (
                    "masked exposure (small radial)",
                    masked(1, radial, exposure),
                ),
            ] {
                let (_, tail) = build_pipeline_split_in_region(
                    &edits,
                    sc,
                    region,
                    pipeline::ProfileRendering::colorimetric(),
                );
                let tail_ms = best_of(|| tail.run(mid.clone())) - best_of(|| mid.clone());
                let out = tail.run(mid.clone());
                let pack_ms = best_of(|| out.to_rgba8());
                let bytes = out.to_rgba8();
                let hist_ms = best_of(|| rustroom_app::histogram::histogram_rgba(&bytes));
                println!(
                    "   tail {tail_ms:5.1}  pack {pack_ms:5.1}  hist {hist_ms:5.1}  = {:5.1} ms  {label}",
                    tail_ms + pack_ms + hist_ms
                );
            }
        }
    }
}
