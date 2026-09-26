//! What does the camera rendering actually change in an exported file?
//!
//! ⚠⚠ **`docs/BACKLOG.md` says taking this measurement is the FIRST task**, because the sibling
//! half of this bug (lens correction, M8.9) was measured at worst 187 of 255 levels and this half
//! had no number at all. A fix that changes every exported RAW deserves a size before it lands.
//!
//! ⚠ It also checks the two halves **separately**. `pipeline::is_identity` records that the X-T4's
//! own `ProfileToneCurve` is exactly `[[0,0],[1,1]]` — so on this catalogue's camera the curve may
//! contribute nothing and the whole difference be baseline exposure. Reporting one combined number
//! would hide which half matters.
//!
//! ```text
//! cargo run --release -p app --example probe_export_rendering <raw> <profile.dcp>
//! ```
use edit_model::Edits;
use pipeline::{ProfileRendering, RenderScale, RenderSpec, Resampler};

fn main() {
    let mut a = std::env::args().skip(1);
    let raw = a
        .next()
        .expect("usage: probe_export_rendering <raw> <profile.dcp>");
    let dcp = a
        .next()
        .expect("usage: probe_export_rendering <raw> <profile.dcp>");

    let opts = raw_decode::DecodeOptions {
        camera_profile: Some(std::path::PathBuf::from(&dcp)),
        ..raw_decode::DecodeOptions::linear_scene()
    };
    let d = raw_decode::decode_raw(std::path::Path::new(&raw), &opts).expect("decode");
    println!(
        "{} {} — {}x{}",
        d.metadata.make,
        d.metadata.model,
        d.image.width(),
        d.image.height()
    );

    let p = camera_profile::CameraProfile::from_file(std::path::Path::new(&dcp)).expect("profile");
    let curve = p.tone_curve;
    let baseline = p.baseline_exposure;
    println!(
        "profile: curve {} points, baseline_exposure {:?}",
        curve.as_ref().map_or(0, |c| c.len()),
        baseline
    );
    if let Some(c) = &curve {
        let identity = c.len() < 2 || c.iter().all(|q| (q[0] - q[1]).abs() < 1e-4);
        println!(
            "  ⚠ the curve is {}",
            if identity {
                "THE IDENTITY — it renders nothing"
            } else {
                "a real curve"
            }
        );
    }

    // ⚠ A realistic edit set: the rendering interacts with the tone stages, so measuring it on a
    // neutral photograph would price the least interesting case.
    let e = Edits {
        // ⚠⚠ **Without this the whole probe is vacuous, and the first run WAS.** The rendering
        // stage is gated on `edits.applied_rendering()`, which defaults to `Rendering::None` — so
        // a default `Edits` builds no stage at all and every configuration measured 0.00, which
        // reads exactly like "the camera rendering changes nothing". The fixture check below is
        // what separated those two readings.
        rendering: Some(edit_model::Rendering::Camera),
        exposure: 0.3,
        contrast: 20.0,
        highlights: -25.0,
        shadows: 30.0,
        ..Default::default()
    };
    let scale = RenderScale::new(1.0);
    let spec = RenderSpec {
        space: color::OutputSpace::Srgb,
        intent: color::RenderingIntent::default(),
        resampler: Resampler::default(),
        baseline: d.metadata.as_shot.unwrap_or_default(),
        airlight: None,
    };
    let render = |r: ProfileRendering<'_>| {
        let (_, tail) = pipeline::build_pipeline_split_in_region_with_brush(
            &e, scale, None, None, spec, None, r,
        );
        tail.run(d.image.clone())
    };

    let plain = render(ProfileRendering::default());

    // ⚠⚠ **THE FIXTURE CHECK, and without it a 0.00 below is unreadable.** A zero can mean "the
    // camera rendering changes nothing" or "this probe measures nothing", and those are opposite
    // conclusions. A deliberately non-identity curve must move the picture; if it does not, every
    // other number here is worthless.
    let synthetic: Vec<[f32; 2]> = (0..=16)
        .map(|i| {
            let x = i as f32 / 16.0;
            [x, x.powf(0.72)]
        })
        .collect();
    let probe = render(ProfileRendering {
        curve: Some(&synthetic),
        baseline_ev: Some(0.5),
        look: None,
    });
    let fixture = gpu_pipeline::compare(&plain, &probe);
    println!(
        "  {:<30} worst {:>7.2} levels  <- FIXTURE CHECK, must be large",
        "a synthetic curve + 0.5 EV", fixture.worst_levels
    );
    assert!(
        fixture.worst_levels > 1.0,
        "fixture check: a real curve must move the picture, else this probe measures nothing"
    );

    for (label, r) in [
        (
            "curve only",
            ProfileRendering {
                curve: curve.as_deref(),
                baseline_ev: None,
                look: None,
            },
        ),
        (
            "baseline only",
            ProfileRendering {
                curve: None,
                baseline_ev: baseline,
                look: None,
            },
        ),
        (
            "BOTH (what the preview shows)",
            ProfileRendering {
                curve: curve.as_deref(),
                baseline_ev: baseline,
                look: None,
            },
        ),
    ] {
        let got = render(r);
        let d8 = gpu_pipeline::compare(&plain, &got);
        println!(
            "  {label:<30} worst {:>7.2} levels, {:>5.1}% of samples differ",
            d8.worst_levels,
            d8.differing_percent()
        );
    }
}
