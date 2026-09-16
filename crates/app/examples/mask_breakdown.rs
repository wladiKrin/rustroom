//! Decomposes a mask's own cost — coverage / clone / sub-stages / blend — at rung 3200.
//!
//! Run: `RUSTROOM_DEBUG_MASK=1 cargo run --release -p app --example mask_breakdown <raw>`
//!
//! ⚠ The phase numbers come from temporary instrumentation inside `MaskStage::apply`, printed
//! to stderr behind `RUSTROOM_DEBUG_MASK`. The totals here are whole-frame renders at rung 3200,
//! which is the rung every masked figure in `CLAUDE.md` is quoted at.
use edit_model::{
    ColourRange, Edits, Hsl, LinearGradient, LocalAdjustments, Mask, MaskedAdjustment,
    RadialGradient,
};
use rustroom_app::render_engine::render_once;
use std::time::Instant;

const RUNG: usize = 3200;

fn gradient() -> Mask {
    Mask::Linear(LinearGradient {
        from: [0.5, 0.0],
        to: [0.5, 1.0],
        invert: false,
    })
}
fn radial() -> Mask {
    Mask::Radial(RadialGradient {
        center: [0.5, 0.5],
        radius: [0.12, 0.12],
        feather: 50.0,
        invert: false,
    })
}
fn brush() -> Mask {
    Mask::Brush(edit_model::Brush {
        strokes: vec![edit_model::BrushStroke {
            points: (0..60).map(|i| [0.2 + i as f32 * 0.01, 0.5]).collect(),
            radius: 0.08,
            hardness: 50.0,
            flow: 100.0,
            auto_mask: false,
            tolerance: 25.0,
            ..Default::default()
        }],
        invert: false,
    })
}
fn auto_brush() -> Mask {
    let Mask::Brush(mut b) = brush() else {
        unreachable!()
    };
    b.strokes[0].auto_mask = true;
    Mask::Brush(b)
}
fn colour_range() -> Mask {
    Mask::Colour(ColourRange::default())
}
/// ⚠ A band over the midtones, so a real photograph's pixels actually cross both shoulders. A
/// default range would select almost everything and make the ramp branch nearly unreachable.
fn luminance_range() -> Mask {
    Mask::Luminance(edit_model::LuminanceRange {
        low: 25.0,
        high: 75.0,
        feather: 50.0,
        invert: false,
    })
}
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
/// Every sub-stage this catalogue's one real mask carries that lives in the tail, plus HSL.
fn heavy() -> LocalAdjustments {
    let mut hsl = Hsl::default();
    hsl.bands[0].saturation = 30.0;
    LocalAdjustments {
        exposure: 0.5,
        contrast: 20.0,
        hsl,
        ..Default::default()
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
    let path = std::env::args()
        .nth(1)
        .expect("usage: mask_breakdown <raw>");
    let path = std::path::Path::new(&path);
    let opts = raw_decode::DecodeOptions::preview();
    let d = raw_decode::decode_raw(path, &opts).expect("decode");
    let buf = d.image;
    let scale = opts.source_scale();
    println!("buffer {}x{}, rung {RUNG}\n", buf.width(), buf.height());

    let cases: Vec<(&str, Edits)> = vec![
        ("neutral", Edits::default()),
        (
            "1 masked exposure (gradient)",
            masked(1, gradient, exposure),
        ),
        (
            "1 masked HSL      (gradient)",
            masked(1, gradient, hsl_adjust),
        ),
        (
            "3 masked HSL      (gradient)",
            masked(3, gradient, hsl_adjust),
        ),
        ("1 masked heavy    (gradient)", masked(1, gradient, heavy)),
        (
            "1 masked exposure (small radial)",
            masked(1, radial, exposure),
        ),
        (
            "1 masked exposure (colour range)",
            masked(1, colour_range, exposure),
        ),
        (
            "1 masked exposure (luminance range)",
            masked(1, luminance_range, exposure),
        ),
        (
            "1 masked exposure (brush) uncached",
            masked(1, brush, exposure),
        ),
        (
            "1 masked exposure (auto brush)",
            masked(1, auto_brush, exposure),
        ),
    ];

    // ⚠ The real sidecar, if one sits beside the RAW. This catalogue has exactly one photo
    // with a mask, and a synthetic fixture cannot stand in for it: its brush is 36 strokes and
    // 1,387 points, most of them auto-masked.
    let side = path.with_file_name(format!(
        "{}.json",
        path.file_name().unwrap().to_string_lossy()
    ));
    let real: Option<Edits> = std::fs::read_to_string(&side).ok().and_then(|t| {
        let v: serde_json::Value = serde_json::from_str(&t).ok()?;
        let e = v.get("copies")?.get(0)?.get("edits")?.clone();
        serde_json::from_value(e).ok()
    });

    let mut neutral = 0.0;
    for (label, edits) in &cases {
        eprintln!("=== {label}");
        let ms = best_of(|| {
            render_once(
                &buf,
                edits,
                RUNG,
                scale,
                color::OutputSpace::Srgb,
                pipeline::Resampler::default(),
                None,
            )
        });
        if *label == "neutral" {
            neutral = ms;
        }
        println!("{ms:8.1} ms  ({:+6.1})  {label}", ms - neutral);
    }

    if let Some(e) = real {
        eprintln!("=== REAL SIDECAR");
        let ms = best_of(|| {
            render_once(
                &buf,
                &e,
                RUNG,
                scale,
                color::OutputSpace::Srgb,
                pipeline::Resampler::default(),
                None,
            )
        });
        let mut bare = e.clone();
        bare.masks.clear();
        let ms0 = best_of(|| {
            render_once(
                &buf,
                &bare,
                RUNG,
                scale,
                color::OutputSpace::Srgb,
                pipeline::Resampler::default(),
                None,
            )
        });
        println!(
            "\n{ms:8.1} ms  ({:+6.1})  REAL SIDECAR, {} mask(s)",
            ms - ms0,
            e.masks.len()
        );
        println!("{ms0:8.1} ms  (  +0.0)  the same sidecar with its masks removed");
    }
}
