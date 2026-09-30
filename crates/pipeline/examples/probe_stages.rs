//! Per-stage CPU cost in the tail, at a native rung.
//!
//! ⚠ This is the evidence behind M8.3e's cost gate: a run is worth a GPU round trip only when the
//! CPU work it replaces exceeds the transfer, and these are the numbers that "exceeds" is
//! measured against. Run it beside `gpu-pipeline`'s `probe_rt`, which prices the transfer itself.
//!
//! ⚠⚠ A stage absent from a row is not neutral — it is **not in the pipeline at all**, because
//! `build_pipeline_split_in_region` pushes a stage only when `is_active()`. That is why the
//! per-pixel tail of an ordinary photograph is contiguous and can be one chain.
use core_image::ImageBuf;
use edit_model::Edits;
use pipeline::TailRendering;
use pipeline::{build_pipeline_split_in_region, RenderScale};
use std::time::Instant;

/// A named edit that switches exactly one tail stage on.
type Case = (&'static str, fn(&mut Edits));

fn scene(w: usize, h: usize) -> ImageBuf {
    let mut px = Vec::with_capacity(w * h * 3);
    for i in 0..w * h {
        let t = (i % 4096) as f32 / 4096.0 * 1.2;
        px.extend_from_slice(&[t, t * 0.9, t * 1.1]);
    }
    ImageBuf::from_vec(w, h, px)
}

/// Best of five, so a scheduling hiccup cannot be read as a cost.
fn time(img: &ImageBuf, e: &Edits) -> (f64, Vec<&'static str>) {
    let names = build_pipeline_split_in_region(
        e,
        RenderScale::new(1.0),
        None,
        TailRendering::colorimetric(),
    )
    .1
    .into_stages()
    .iter()
    .map(|s| s.name())
    .collect();
    let mut best = f64::MAX;
    for _ in 0..5 {
        let c = img.clone();
        let (_, tail) = build_pipeline_split_in_region(
            e,
            RenderScale::new(1.0),
            None,
            TailRendering::colorimetric(),
        );
        let t = Instant::now();
        let out = tail.run(c);
        best = best.min(t.elapsed().as_secs_f64() * 1000.0);
        std::hint::black_box(&out);
    }
    (best, names)
}

fn main() {
    let (w, h) = (3200usize, 2136);
    let img = scene(w, h);
    println!("{w}x{h} ({:.1} Mpx)", (w * h) as f64 / 1e6);

    let (base, names) = time(&img, &Edits::default());
    println!("baseline {names:?}: {base:.2} ms");

    let cases: Vec<Case> = vec![
        ("white_balance", |e| e.white_balance.kelvin = Some(4000.0)),
        ("exposure", |e| e.exposure = 0.45),
        ("contrast", |e| e.contrast = 30.0),
        ("tone_regions", |e| {
            e.whites = 25.0;
            e.blacks = -20.0;
        }),
        ("tone_curve", |e| {
            e.tone_curve.points = vec![[0.0, 0.0], [0.4, 0.5], [1.0, 1.0]]
        }),
        ("saturation", |e| e.saturation = 20.0),
        // ⚠⚠ The one stage `probe_stages` did not cover until M8.3n, which is why this file's
        // own numbers could never say what local contrast costs. Split by band **and by sign**,
        // because positive Texture measures against a Gaussian and negative against a guided
        // filter — two different amounts of work behind one slider.
        ("texture_plus", |e| e.local_contrast.texture = 60.0),
        ("texture_minus", |e| e.local_contrast.texture = -60.0),
        ("clarity", |e| e.local_contrast.clarity = 60.0),
        ("texture+clarity", |e| {
            e.local_contrast.texture = 60.0;
            e.local_contrast.clarity = 60.0;
        }),
        // ⚠ Both shapes, because the superellipse branch costs two `powf` per pixel where the
        // elliptical one costs a `sqrt` — one number for "the vignette" would describe neither.
        ("vignette_elliptical", |e| e.vignette.amount = -60.0),
        ("vignette_superellipse", |e| {
            e.vignette.amount = -60.0;
            e.vignette.roundness = -60.0;
        }),
        ("display_rendering", |e| {
            e.rendering = Some(edit_model::Rendering::Camera)
        }),
    ];
    for (name, f) in cases {
        let mut e = Edits::default();
        f(&mut e);
        let (t, _) = time(&img, &e);
        println!("  + {name:<20} {t:6.2} ms   (delta {:+.2})", t - base);
    }

    // The case that decides the milestone: everything an ordinary photograph here runs.
    let mut typical = Edits {
        exposure: 0.45,
        whites: 25.0,
        blacks: -20.0,
        ..Edits::default()
    };
    typical.rendering = Some(edit_model::Rendering::Camera);
    let (t, names) = time(&img, &typical);
    println!("TYPICAL {names:?}: {t:.2} ms");
}
