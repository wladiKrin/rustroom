//! Times masked spatial adjustments — the M7.2 question: what does it cost to put Clarity,
//! Texture, Sharpening or HSL *inside* a mask, and does that argue for a bounding-box
//! optimisation or for a hue LUT?
//!
//! Run: `cargo run --release -p app --example bench_masked <raw>`
//!
//! ⚠ Two things this measures deliberately, because the decision turns on both:
//!
//! 1. **One mask against three.** `tail_kernel_radius` sums over masks and mask N runs on
//!    mask N-1's output, so the cost is linear in the number of masks with nothing shared.
//! 2. **A frame-covering linear gradient against a small radial.** They cost the *same* today,
//!    which is exactly the evidence that no bounding box exists — and the radial is the only
//!    one of the two a bounding box could ever help.
use edit_model::{
    ColorGrading, Edits, GradeWheel, Hsl, LinearGradient, LocalAdjustments, LocalContrast, Mask,
    MaskedAdjustment, RadialGradient, ToneCurve,
};
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

fn clarity() -> LocalAdjustments {
    LocalAdjustments {
        local_contrast: LocalContrast {
            texture: 0.0,
            clarity: 70.0,
        },
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

fn masked(n: usize, mask: fn() -> Mask, adj: fn() -> LocalAdjustments) -> Edits {
    Edits {
        masks: (0..n)
            .map(|_| MaskedAdjustment {
                mask: mask(),
                adjustments: adj(),
            })
            .collect(),
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
        (
            "1 masked HSL      (small radial)",
            masked(1, radial, hsl_adjust),
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
        let (prefix, _) = build_pipeline_split_in_region(&Edits::default(), sc, None);
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
            let (_, tail) = build_pipeline_split_in_region(edits, sc, Some(region));
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
                let (_, t) = build_pipeline_split_in_region(&Edits::default(), sc, Some(region));
                best_of(|| t.run(sub.clone()))
            };
            let cl = {
                let e = masked(1, gradient, clarity);
                let (_, t) = build_pipeline_split_in_region(&e, sc, Some(region));
                best_of(|| t.run(sub.clone())) - base
            };
            let hs = {
                let e = masked(1, gradient, hsl_adjust);
                let (_, t) = build_pipeline_split_in_region(&e, sc, Some(region));
                best_of(|| t.run(sub.clone())) - base
            };
            println!("           {label:>9}: clarity {cl:+6.1} ms   HSL {hs:+6.1} ms");
        }
    }
}
