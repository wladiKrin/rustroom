//! Replays a Straighten drag on a real photo through the app's own `RenderEngine`, so every part
//! of a drag frame can be timed with `RUSTROOM_DEBUG_RENDER=1`.
//!
//! ⚠ Uses the photo's **own sidecar edits** (copy 0) and its matched lens profile, and renders at
//! the rung the user's window actually used, because a drag frame's cost is a property of the
//! photo and the rung, not of a fixture. The camera profile's tone curve is **not** resolved here
//! (that lives in the binary's `decode.rs`), so the rendering stage runs with its built-in curve —
//! the same stage, the same cost.
//!
//! Run: `RUSTROOM_DEBUG_RENDER=1 cargo run --release -p app --example probe_straighten_drag <raw> [rung]`

use rustroom_app::render_engine::{FrameSpec, RenderEngine};
use std::sync::Arc;
use std::time::Instant;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .expect("usage: probe_straighten_drag <raw> [rung]");
    let rung: usize = args.next().map_or(3200, |r| r.parse().expect("rung"));
    let path = std::path::Path::new(&path);
    // ⚠ The FULL decode: the app hands over to it after opening, and a rung of 3200 is then a
    // downscale of the 6246 px buffer rather than an upscale of the preview.
    let opts = raw_decode::DecodeOptions::linear_scene();
    let d = raw_decode::decode_raw(path, &opts).expect("decode");
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
    let base = rustroom_app::sidecar::load(path).unwrap_or_default();
    eprintln!(
        "buffer {}x{}, lens {}, sidecar edits {}",
        d.image.width(),
        d.image.height(),
        if lens.is_some() { "matched" } else { "none" },
        if base == edit_model::Edits::default() {
            "none"
        } else {
            "loaded"
        }
    );
    let engine = RenderEngine::new(
        Arc::new(d.image),
        opts.source_scale(),
        lens,
        d.metadata.as_shot.unwrap_or_default(),
        rustroom_app::decode_worker::ProfileParts::default(),
        true,
        || {},
    );
    let frame = |angle: f32| -> f64 {
        let mut e = base.clone();
        e.geometry.angle = angle;
        let t = Instant::now();
        engine.request(
            e,
            FrameSpec {
                max_dim: rung,
                region: None,
                display_space: color::OutputSpace::Srgb,
                // ⚠ `PROBE_BILINEAR=1` isolates the gather's arithmetic: 2x2 taps against 4x4.
                resampler: if std::env::var_os("PROBE_BILINEAR").is_some() {
                    pipeline::Resampler::Bilinear
                } else {
                    pipeline::Resampler::default()
                },
                gpu_tail: true,
                grade_view: None,
                soft_proof: None,
                visualize: None,
            },
        );
        loop {
            if engine.poll_latest().is_some() {
                return t.elapsed().as_secs_f64() * 1000.0;
            }
            std::thread::sleep(std::time::Duration::from_micros(200));
        }
    };
    // Warm: the first frames compile shaders and fill the lens checkpoint.
    for a in [0.0, 0.1, 0.2] {
        frame(a);
    }
    eprintln!("---- drag ----");
    // ⚠ Median of 40, not a mean of 12: single runs varied 30-41 ms on an idle machine, and a
    // mean is dragged by the occasional frame a warm-behind render collided with.
    let n = 40;
    let mut all: Vec<f64> = (0..n).map(|i| frame(0.5 + 0.05 * i as f32)).collect();
    all.sort_by(|a, b| a.total_cmp(b));
    eprintln!(
        "median wall per drag frame: {:.1} ms  (p10 {:.1}, p90 {:.1}, n {n})",
        all[n / 2],
        all[n / 10],
        all[n * 9 / 10]
    );
}
