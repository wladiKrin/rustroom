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
//!
//! Or, to choose `MIN_EDGE_LEVELS` from a labelled corpus rather than by assertion:
//!
//! `cargo run --release -p app --example detect_probe -- --sweep <labels-file>`

use core_image::ImageBuf;
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

fn probe_lenses(paths: &[String]) {
    for path in paths {
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
        let on = Edits {
            lens_correction: Some(edit_model::LensCorrection {
                distortion: true,
                tca: true,
                vignetting: true,
                ..Default::default()
            }),
            ..Default::default()
        };
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

// ---------------------------------------------------------------------------------------------
// The sweep: choosing MIN_EDGE_LEVELS from a labelled corpus rather than by assertion.
// ---------------------------------------------------------------------------------------------

/// One labelled frame.
struct Labelled {
    path: std::path::PathBuf,
    /// Whether the detector *should* find two verticals here.
    fire: bool,
}

/// Read a labels file: `<fire|silent> <path>` per line; `#` comments and blank lines ignored.
///
/// ⚠ The file is deliberately **not committed** — it names photographs on one machine, the
/// convention this project already uses for `RAW_FIXTURE` and `DCP_FIXTURE`.
fn read_labels(p: &std::path::Path) -> std::io::Result<Vec<Labelled>> {
    let text = std::fs::read_to_string(p)?;
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((verdict, path)) = line.split_once(char::is_whitespace) else {
            eprintln!(
                "  ⚠ line {}: no path after the verdict, skipping: {line}",
                n + 1
            );
            continue;
        };
        let fire = match verdict.trim() {
            "fire" => true,
            "silent" => false,
            other => {
                eprintln!("  ⚠ line {}: unknown verdict {other:?}, skipping", n + 1);
                continue;
            }
        };
        out.push(Labelled {
            path: std::path::PathBuf::from(path.trim()),
            fire,
        });
    }
    Ok(out)
}

/// Precision, recall, false fires and misses for one floor value.
///
/// ⚠⚠ **Recall is the half that matters and the half the old evidence lacked.** The statistic
/// track C had was a fire *rate* — "fires on 54% of 70" — against which a floor that silences
/// everything scores perfectly and is useless. Only recall against the frames that *should* fire
/// exposes that.
fn score(results: &[(bool, bool)]) -> (f32, f32, usize, usize) {
    let tp = results.iter().filter(|(want, got)| *want && *got).count();
    let fp = results.iter().filter(|(want, got)| !*want && *got).count();
    let misses = results.iter().filter(|(want, got)| *want && !*got).count();
    let precision = if tp + fp == 0 {
        f32::NAN
    } else {
        tp as f32 / (tp + fp) as f32
    };
    let recall = if tp + misses == 0 {
        f32::NAN
    } else {
        tp as f32 / (tp + misses) as f32
    };
    (precision, recall, fp, misses)
}

/// The candidate floors the sweep reports, in 8-bit levels.
const SWEEP: &[f32] = &[2.0, 4.0, 6.0, 8.0, 12.0, 16.0, 24.0, 32.0, 48.0, 64.0, 96.0];

fn sweep(labels_path: &std::path::Path) {
    let labels = match read_labels(labels_path) {
        Ok(l) if !l.is_empty() => l,
        Ok(_) => {
            eprintln!("{} holds no usable labels.", labels_path.display());
            eprintln!("Write one `fire <path>` or `silent <path>` per line.");
            std::process::exit(2);
        }
        Err(e) => {
            eprintln!("cannot read {}: {e}", labels_path.display());
            eprintln!("The labels file is not in the repo by design — see the module doc.");
            std::process::exit(2);
        }
    };
    let want_fire = labels.iter().filter(|l| l.fire).count();
    println!(
        "{} labelled frames: {want_fire} should fire, {} should stay silent\n",
        labels.len(),
        labels.len() - want_fire
    );

    // ⚠ Decode once per frame and hold the analysis inputs, not once per (frame × floor): a RAW
    // decode is seconds and the sweep has eight rungs.
    let mut prepared: Vec<(&Labelled, ImageBuf, Edits, Option<lens_profile::LensShot>)> =
        Vec::new();
    for l in &labels {
        let decoded =
            match raw_decode::decode_raw(&l.path, &raw_decode::DecodeOptions::linear_scene()) {
                Ok(d) => d,
                Err(e) => {
                    eprintln!("  ⚠ {}: decode failed: {e}", l.path.display());
                    continue;
                }
            };
        let m = &decoded.metadata;
        let lens = m.lens.as_ref().and_then(|li| {
            let db = lens_profile::Catalogue::bundled().ok()?;
            let profile = db.find(&m.make, &m.model, &li.name)?;
            Some(lens_profile::LensShot {
                profile,
                shot: lens_profile::Shot {
                    focal: li.focal,
                    aperture: li.aperture,
                },
            })
        });
        // Lens correction ON, as the app does by default for a RAW.
        let edits = Edits {
            lens_correction: Some(edit_model::LensCorrection {
                distortion: true,
                tca: true,
                vignetting: true,
                ..Default::default()
            }),
            ..Default::default()
        };
        prepared.push((l, decoded.image, edits, lens));
    }
    if prepared.is_empty() {
        eprintln!("nothing decoded — no sweep to report.");
        std::process::exit(2);
    }

    println!("levels  precision  recall  false-fires  misses");
    let mut wrong_at: Vec<(f32, Vec<String>)> = Vec::new();
    for &floor in SWEEP {
        let mut results = Vec::new();
        let mut wrong = Vec::new();
        for (l, buf, edits, lens) in &prepared {
            let found = perspective_detect::detect_verticals_with_floor(
                buf,
                edits,
                lens.clone(),
                Resampler::default(),
                floor,
            );
            let got = found.len() >= 2;
            if got != l.fire {
                let name = l
                    .path
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| l.path.display().to_string());
                wrong.push(format!(
                    "{name} ({})",
                    if l.fire { "missed" } else { "fired" }
                ));
            }
            results.push((l.fire, got));
        }
        let (p, r, fp, misses) = score(&results);
        println!("{floor:5.1}      {p:5.2}    {r:5.2}     {fp:6}   {misses:7}");
        wrong_at.push((floor, wrong));
    }

    println!("\nWhich frames each floor gets wrong — judge by the photographs, not the number:");
    for (floor, wrong) in &wrong_at {
        if wrong.is_empty() {
            println!("  {floor:5.1}  (none)");
        } else {
            println!("  {floor:5.1}  {}", wrong.join(", "));
        }
    }
    println!(
        "\n⚠ Read the value off the curve and record the numbers in MIN_EDGE_LEVELS' doc comment.\n\
         ⚠ Prefer recall where the curve is flat: a miss is a button that looks broken with nothing\n\
         \x20  on screen to act on, while a false fire draws a visible, editable guide. That is the\n\
         \x20  same asymmetry MAX_LEAN_DEG was chosen on."
    );
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.split_first() {
        Some((first, rest)) if first == "--sweep" => match rest {
            [labels] => sweep(std::path::Path::new(labels)),
            _ => {
                eprintln!("usage: detect_probe --sweep <labels-file>");
                std::process::exit(2);
            }
        },
        Some(_) => probe_lenses(&args),
        None => {
            eprintln!("usage: detect_probe <raw> [more raws...]");
            eprintln!("       detect_probe --sweep <labels-file>");
            std::process::exit(2);
        }
    }
}
