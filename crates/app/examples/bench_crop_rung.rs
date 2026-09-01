//! Why a cropped photo goes unresponsive on the first slider move, measured.
//!
//! Crop mode **pins** the render to the fit rung. On exit, a crop makes the image smaller, so
//! `fit` is larger and `tier_max_dim` doubles — and `max_dim` is a *pre-geometry* cap, so the
//! **prefix** (geometry, lens correction, noise reduction, dehaze, perspective) re-runs on 4× the
//! pixels with a cold cache. This times exactly that, split into prefix and tail, so the reported
//! "several seconds" can be attributed rather than guessed at.
//!
//! Run: `cargo run --release -p app --example bench_crop_rung <raw>`

use edit_model::{CropRect, Edits, Geometry};
use pipeline::{RenderScale, Resampler};
use std::time::Instant;

const PREVIEW_MAX_DIM: usize = 1600;

fn best_of<T>(n: usize, mut f: impl FnMut() -> T) -> (T, f64) {
    let mut best = f64::MAX;
    let mut out = None;
    for _ in 0..n {
        let t = Instant::now();
        let v = f();
        best = best.min(t.elapsed().as_secs_f64() * 1000.0);
        out = Some(v);
    }
    (out.unwrap(), best)
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: bench_crop_rung <raw>");
    let path = std::path::Path::new(&path);

    // ⚠ The **full** decode, not `preview()`. The GUI upgrades to it ~0.8 s after a photo settles,
    // so it is what a session is actually holding by the time anyone crops.
    let opts = raw_decode::DecodeOptions::linear_scene();
    let d = raw_decode::decode_raw(path, &opts).expect("decode");
    // ⚠ The same resolution the GUI does, on the same data — a benchmark that skipped the lens
    // profile would measure a pipeline nobody runs, since lens correction is on by default and is
    // the expensive part of the prefix.
    // ⚠ Inlined rather than reused: `decode::resolve_lens` is `pub(crate)` in a **bin** module,
    // which an example cannot reach. Same lookup, same data.
    let lens = d.metadata.lens.as_ref().and_then(|l| {
        lens_profile::Catalogue::shared()
            .and_then(|db| db.find(&d.metadata.make, &d.metadata.model, &l.name))
            .map(|profile| lens_profile::LensShot {
                profile,
                shot: lens_profile::Shot {
                    focal: l.focal,
                    aperture: l.aperture,
                },
            })
    });
    let lens_name = d
        .metadata
        .lens
        .as_ref()
        .map(|l| l.name.clone())
        .unwrap_or_else(|| "none".into());
    let buf = std::sync::Arc::new(d.image);
    let buffer_long = buf.width().max(buf.height());
    let fit_cap = buffer_long.min(PREVIEW_MAX_DIM);
    println!(
        "buffer {}x{}   fit_cap {fit_cap}   next rung {}",
        buf.width(),
        buf.height(),
        (fit_cap * 2).min(buffer_long)
    );
    println!(
        "lens: {:?}",
        lens_name
    );

    // ⚠⚠ Lens corrections ON, as the GUI has them. `Edits::default()` is strictly neutral and
    // leaves them off; `Settings.auto_lens_correction` (default on) turns them on for any photo
    // with no sidecar yet. A benchmark using the bare default measures a pipeline nobody runs —
    // and lens correction is the expensive stage, at a measured 7.7x the comparable one.
    let lens_on = edit_model::LensCorrection {
        distortion: true,
        tca: true,
        vignetting: true,
        ..Default::default()
    };

    // A 50% centre crop — the case reported.
    let cropped = Edits {
        lens_correction: lens_on,
        geometry: Geometry {
            crop: CropRect {
                left: 0.25,
                top: 0.25,
                right: 0.75,
                bottom: 0.75,
            },
            ..Default::default()
        },
        ..Edits::default()
    };
    // The same photo with one tail slider moved, which is what "moving edit sliders" does.
    let mut cropped_exposed = cropped.clone();
    cropped_exposed.exposure = 0.35;

    for (label, max_dim) in [
        ("fit rung   (what Crop mode pins to)", fit_cap),
        ("next rung  (what a 50% crop climbs to)", (fit_cap * 2).min(buffer_long)),
    ] {
        println!("\n=== {label}: max_dim {max_dim} ===");
        let scale =
            RenderScale::new(max_dim as f32 / buffer_long as f32);

        // Prefix and tail separately, so the cost can be attributed.
        let ((prefix, tail), build_ms) = best_of(3, || {
            pipeline::build_pipeline_split_in_region_with_brush(
                &cropped,
                scale,
                None,
                None,
                color::OutputSpace::Srgb,
                lens.clone(),
                Resampler::default(),
            )
        });
        let (src, down_ms) = best_of(3, || core_image::ImageBuf::clone(&buf).downscale_to(max_dim));
        println!(
            "  downscale_to      {down_ms:8.1} ms   -> {}x{}",
            src.width(),
            src.height()
        );

        let (mid, prefix_ms) = best_of(3, || prefix_run(&cropped, scale, &lens, max_dim, &buf));
        println!("  PREFIX (cold)     {prefix_ms:8.1} ms   -> {}x{}", mid.width(), mid.height());

        let (_, tail_ms) = best_of(5, || {
            let (_, t) = pipeline::build_pipeline_split_in_region_with_brush(
                &cropped_exposed,
                scale,
                None,
                None,
                color::OutputSpace::Srgb,
                lens.clone(),
                Resampler::default(),
            );
            t.run(mid.clone())
        });
        println!("  TAIL  (per slider){tail_ms:8.1} ms   <- this is what a warm cache costs");
        println!("  build_pipeline    {build_ms:8.1} ms");
        let _ = tail;
    }

    println!(
        "\nThe first slider after leaving Crop pays PREFIX + TAIL at the higher rung;\n\
         every one after that pays TAIL alone, because the prefix is cached."
    );
}

fn prefix_run(
    edits: &Edits,
    scale: RenderScale,
    lens: &Option<lens_profile::LensShot>,
    max_dim: usize,
    buf: &std::sync::Arc<core_image::ImageBuf>,
) -> core_image::ImageBuf {
    let (prefix, _) = pipeline::build_pipeline_split_in_region_with_brush(
        edits,
        scale,
        None,
        None,
        color::OutputSpace::Srgb,
        lens.clone(),
        Resampler::default(),
    );
    prefix.run(core_image::ImageBuf::clone(buf).downscale_to(max_dim))
}
