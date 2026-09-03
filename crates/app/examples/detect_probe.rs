//! What the vertical detector finds in a real photograph, **with lens correction on and off**.
//!
//! ⚠⚠ **This is the one decision in M7.4 that no synthetic fixture can check.** None of the
//! test fixtures carries a lens profile, so the sabotage that skips `LensCorrectionStage` is
//! provably *inert* against the suite — `perspective_detect`'s own sabotage table records it as
//! such. On a real RAW the buffer is pre-lens-correction and barrel distortion curves every
//! building edge, so each edge fragments into several Hough lines at slightly different angles
//! and the vanishing point they imply is biased. The result is not an error; it is a correction
//! that is merely *plausible*, which this project's records name repeatedly as its worst failure
//! shape.
//!
//! ⚠ If the two sets come out identical, that is a finding either way and must be run down
//! rather than reported as "no difference": either the lens step is not being applied at all, or
//! this lens has negligible distortion at this focal length. The probe prints the corrector's own
//! **maximum radial displacement** so the two can be told apart.
//!
//! `cargo run --release -p app --example detect_probe -- <raw> [more raws...]`

use edit_model::Edits;
use pipeline::perspective::guides::{self, Guide};
use pipeline::Resampler;
use rustroom_app::perspective_detect::{self, ANALYSIS_DIM};

/// The angle a guide makes with vertical, in degrees, on a frame of this aspect.
///
/// ⚠ Scaled by the aspect: `[0, 1]` frame coordinates are anisotropic, so a raw `atan2` of the
/// normalised deltas reports an angle that is not the one on screen — the trap
/// `LinearGradient::coverage` and `mask_tool::hit_test` each had to handle.
fn lean_deg(g: &Guide, aspect: f32) -> f32 {
    let dx = (g.to[0] - g.from[0]) * aspect;
    let dy = g.to[1] - g.from[1];
    dx.atan2(dy).to_degrees()
}

fn describe(label: &str, gs: &[Guide], aspect: f32) {
    println!("  {label}: {} found", gs.len());
    for (i, g) in gs.iter().enumerate() {
        println!(
            "    [{i}] ({:.4}, {:.4}) -> ({:.4}, {:.4})   lean {:+.2}°",
            g.from[0],
            g.from[1],
            g.to[0],
            g.to[1],
            lean_deg(g, aspect)
        );
    }
    if gs.len() >= 2 {
        let (pitch, yaw, roll) = guides::solve(gs, &[], aspect);
        println!(
            "    solved: pitch {:+.3} rad ({:+.2}°), yaw {:+.3}, roll {:+.3} rad ({:+.2}°)",
            pitch,
            pitch.to_degrees(),
            yaw,
            roll,
            roll.to_degrees()
        );
        // The sliders the app would actually write, so the probe and the GUI can be compared.
        let to_slider = |rad: f32| {
            (rad.to_degrees() / pipeline::perspective::MAX_ANGLE_DEG * 100.0).clamp(-100.0, 100.0)
        };
        println!(
            "    sliders: Vertical {:+.1}, Horizontal {:+.1}, Straighten {:+.2}°",
            to_slider(pitch),
            to_slider(yaw),
            roll.to_degrees().clamp(-45.0, 45.0)
        );
    }
}

fn main() {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    if paths.is_empty() {
        eprintln!("usage: detect_probe <raw> [more raws...]");
        std::process::exit(2);
    }

    for path in &paths {
        let p = std::path::Path::new(path);
        println!("\n=== {}", p.display());
        let decoded = match raw_decode::decode_raw(p, &raw_decode::DecodeOptions::linear_scene()) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("  decode failed: {e}");
                continue;
            }
        };
        let m = &decoded.metadata;
        println!("  {} {}  {}x{}", m.make, m.model, m.width, m.height);

        // Resolve the lens exactly as the app's decode thread does.
        let lens = m.lens.as_ref().and_then(|l| {
            println!("  lens: {} @ {}mm f/{}", l.name, l.focal, l.aperture);
            let db = lens_profile::Catalogue::bundled().ok()?;
            let profile = db.find(&m.make, &m.model, &l.name)?;
            Some(lens_profile::LensShot {
                profile,
                shot: lens_profile::Shot {
                    focal: l.focal,
                    aperture: l.aperture,
                },
            })
        });
        let Some(lens) = lens else {
            println!("  ⚠ no lens profile matched — the two runs below MUST be identical, and");
            println!("    that says nothing about whether the lens step works.");
            let buf = decoded.image;
            let aspect = buf.width() as f32 / buf.height() as f32;
            let found = perspective_detect::detect_verticals(
                &buf,
                &Edits::default(),
                None,
                Resampler::default(),
            );
            describe("no profile", &found, aspect);
            continue;
        };

        let buf = decoded.image;
        let aspect = buf.width() as f32 / buf.height() as f32;

        // ⚠⚠ How far the correction actually moves a pixel, at the analysis size the detector
        // runs at. This is what distinguishes "the lens step is not being applied" from "this
        // lens has negligible distortion", which the guides alone cannot.
        let small = buf.downscale_to(ANALYSIS_DIM);
        let (sw, sh) = (small.width(), small.height());
        let corrections = lens_profile::Corrections {
            distortion: true,
            tca: true,
            vignetting: true,
            scale: 0.0,
        };
        match lens_profile::Corrector::new(&lens.profile, &lens.shot, sw, sh, corrections) {
            Some(c) => {
                let mut worst = 0.0f32;
                for y in 0..sh {
                    for x in 0..sw {
                        let src = c.source(x as f32, y as f32);
                        // Green channel: the geometric displacement without the TCA split.
                        let d = (src[1][0] - x as f32).hypot(src[1][1] - y as f32);
                        worst = worst.max(d);
                    }
                }
                println!(
                    "  corrector on the {sw}x{sh} analysis plane: max displacement {worst:.2} px, \
                     autoscale {:.4}",
                    c.autoscale()
                );
            }
            None => println!("  ⚠ the profile supplies no corrector for this shot"),
        }

        // Lens correction ON — what the app does by default for a RAW.
        let mut on = Edits::default();
        on.lens_correction.distortion = true;
        on.lens_correction.tca = true;
        on.lens_correction.vignetting = true;
        let with = perspective_detect::detect_verticals(
            &buf,
            &on,
            Some(lens.clone()),
            Resampler::default(),
        );

        // Lens correction OFF — the raw buffer, i.e. the bug this guards against.
        let off = Edits::default();
        let without =
            perspective_detect::detect_verticals(&buf, &off, Some(lens), Resampler::default());

        describe("lens correction ON ", &with, aspect);
        describe("lens correction OFF", &without, aspect);

        if with == without {
            println!("  ⚠ IDENTICAL — read the max-displacement line above to say which case.");
        } else {
            println!("  → the lens step changes the detected geometry.");
        }
    }
}
