//! Prices what each kind of drag costs in the PREFIX, which is what M7.3b's reorder changed.
//!
//! ⚠⚠ **A drag re-runs the prefix from the first step whose key it changes**, and the prefix cache
//! serves everything above that. So the cost of a drag is the steps from that resume point down,
//! run on the checkpoint above it. This computes the resume point exactly as `render_engine` does,
//! through `PrefixKey::upto`, rather than assuming where each stage sits: the whole point is that
//! the order moved.
//!
//! ⚠ Built to compile on both `main` before M7.3b and the branch after it, so the two orders can
//! be compared with one tool. Nothing here names a stage.
//!
//! ⚠ Two columns. `cpu` runs every step's CPU stage. `gpu` substitutes each step through
//! `accelerate_prefix_stage` where it has a twin, as the render engine does, **without** the
//! resident-plane handoff between consecutive GPU steps. That handoff saves 17.9–23.5 ms a seam at
//! native (journal), so `gpu` over-states chains of consecutive GPU steps a little.
//!
//! Run: `cargo run --release -p app --example probe_prefix_order <raw> [--full]`

use core_image::ImageBuf;
use edit_model::Edits;
use pipeline::{PipelineStage, PrefixKey, RenderScale, Resampler};
use std::time::Instant;

fn best_of<F: FnMut() -> ImageBuf>(n: usize, mut f: F) -> f64 {
    let mut best = f64::MAX;
    for _ in 0..n {
        let t = Instant::now();
        let out = f();
        best = best.min(t.elapsed().as_secs_f64() * 1000.0);
        std::hint::black_box(&out);
    }
    best
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: probe_prefix_order <raw> [--full]");
    let path = std::path::Path::new(&path);
    let full = std::env::args().any(|a| a == "--full");
    let opts = if full {
        raw_decode::DecodeOptions::linear_scene()
    } else {
        raw_decode::DecodeOptions::preview()
    };
    let d = raw_decode::decode_raw(path, &opts).expect("decode");
    let buf = d.image;
    let sc = RenderScale::new(1.0);
    let res = Resampler::default();
    println!(
        "buffer {}x{} ({:.1} Mpx), {} threads",
        buf.width(),
        buf.height(),
        (buf.width() * buf.height()) as f64 / 1e6,
        rayon::current_num_threads()
    );
    let shot = d.metadata.lens.as_ref().and_then(|l| {
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
    println!(
        "lens profile: {}",
        if shot.is_some() { "matched" } else { "none" }
    );

    // Everything the prefix can hold at once, which is a Guide Mode photo with NR and dehaze.
    let base = Edits {
        lens_correction: Some(edit_model::LensCorrection {
            distortion: true,
            tca: true,
            vignetting: true,
            ..Default::default()
        }),
        noise_reduction: edit_model::NoiseReduction {
            luminance: 30.0,
            color: 25.0,
            ..Default::default()
        },
        dehaze: edit_model::Dehaze { amount: 30.0 },
        perspective: edit_model::Perspective {
            vertical: 20.0,
            ..Default::default()
        },
        geometry: edit_model::Geometry {
            angle: 2.0,
            crop: edit_model::CropRect {
                left: 0.05,
                top: 0.05,
                right: 0.95,
                bottom: 0.95,
            },
            ..Default::default()
        },
        ..Edits::default()
    };
    let drags: Vec<(&str, Edits)> = vec![
        ("straighten", {
            let mut e = base.clone();
            e.geometry.angle = 2.1;
            e
        }),
        ("crop", {
            let mut e = base.clone();
            e.geometry.crop.left = 0.06;
            e
        }),
        ("keystone", {
            let mut e = base.clone();
            e.perspective.vertical = 21.0;
            e
        }),
        ("noise reduction", {
            let mut e = base.clone();
            e.noise_reduction.luminance = 31.0;
            e
        }),
        ("dehaze", {
            let mut e = base.clone();
            e.dehaze.amount = 31.0;
            e
        }),
    ];

    let air = pipeline::prefix_airlight(&base, &buf, res, shot.clone());
    let base_steps = pipeline::build_prefix_steps(&base, sc, shot.clone(), res, Some(air));
    let names: Vec<&str> = base_steps.iter().map(|s| s.stage.name()).collect();
    println!("prefix: {names:?}\n");

    // The checkpoint after each base step, so a drag can start from the right one.
    let mut checkpoints = vec![buf.clone()];
    for s in &base_steps {
        let next = s.stage.apply(checkpoints.last().unwrap().clone());
        checkpoints.push(next);
    }

    let ctx = gpu_pipeline::shared();
    println!(
        "{:16} {:>7} {:>10} {:>10}",
        "drag", "resume", "cpu ms", "gpu ms"
    );
    for (label, e) in &drags {
        let air_e = pipeline::prefix_airlight(e, &buf, res, shot.clone());
        let steps = pipeline::build_prefix_steps(e, sc, shot.clone(), res, Some(air_e));
        // ⚠ The resume point exactly as the cache finds it: the deepest checkpoint whose key both
        // edit sets share.
        let mut resume = 0;
        for n in 1..=steps.len().min(base_steps.len()) {
            if PrefixKey::upto(&steps, n, sc) == PrefixKey::upto(&base_steps, n, sc) {
                resume = n;
            } else {
                break;
            }
        }
        let start = &checkpoints[resume];
        let cpu = best_of(3, || {
            let mut img = start.clone();
            for s in &steps[resume..] {
                img = s.stage.apply(img);
            }
            img
        });
        let gpu = ctx.map(|ctx| {
            best_of(3, || {
                let mut img = start.clone();
                for s in &steps[resume..] {
                    let g = gpu_pipeline::accelerate_prefix_stage(
                        s.stage.name(),
                        e,
                        sc,
                        &img,
                        gpu_pipeline::SeamFacts {
                            lens: shot.as_ref(),
                            resampler: res,
                            airlight: air_e,
                        },
                        ctx,
                    );
                    img = match g {
                        Some(g) => g.apply(img),
                        None => s.stage.apply(img),
                    };
                }
                img
            })
        });
        println!(
            "{label:16} {:>3} of {} {cpu:>10.1} {:>10}",
            resume,
            steps.len(),
            gpu.map_or("—".to_string(), |g| format!("{g:.1}"))
        );
    }
}
