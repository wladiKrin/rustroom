//! What does one GPU chain run cost at a given size, warm, and how much of it is the gather?
//!
//! ⚠ Written for GPU geometry's measurement gate (2026-09-26): the tail's `submit` measured ~15 ms
//! at 6.8 Mpx for two trivial stages, so a resident input might remove less than an upload's worth.
//! Uses `RUSTROOM_DEBUG_XFER` output from `GpuChain::run_with` for the split.
//!
//! Run: `RUSTROOM_DEBUG_XFER=1 cargo run --release -p app --example probe_gpu_run_cost <raw> [rung]`

use pipeline::{RenderScale, Resampler};

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: probe_gpu_run_cost <raw> [rung]");
    let rung: usize = args.next().map_or(3200, |r| r.parse().expect("rung"));
    let d = raw_decode::decode_raw(
        std::path::Path::new(&path),
        &raw_decode::DecodeOptions::linear_scene(),
    )
    .expect("decode");
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
    let img = d.image.downscale_to(rung);
    eprintln!("image {}x{}", img.width(), img.height());
    let ctx = gpu_pipeline::shared().expect("a GPU");
    let edits = edit_model::Edits {
        lens_correction: Some(edit_model::LensCorrection {
            distortion: true,
            tca: true,
            vignetting: true,
            ..Default::default()
        }),
        noise_reduction: edit_model::NoiseReduction {
            luminance: 30.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let air = [0.1f32; 3];
    for name in ["lens_correction", "noise_reduction"] {
        eprintln!("-- {name}");
        for _ in 0..4 {
            let g = gpu_pipeline::accelerate_prefix_stage(
                name,
                &edits,
                RenderScale::new(img.width().max(img.height()) as f32 / 6246.0),
                &img,
                gpu_pipeline::SeamFacts {
                    lens: lens.as_ref(),
                    resampler: Resampler::default(),
                    airlight: air,
                    ai: None,
                },
                ctx,
            )
            .expect("a GPU twin");
            let t = std::time::Instant::now();
            let out = pipeline::PipelineStage::apply(&g, img.clone());
            eprintln!("   apply {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
            std::hint::black_box(out);
        }
    }
}
