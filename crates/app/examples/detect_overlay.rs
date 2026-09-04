//! Write the detector's own analysis plane out as a PNG with the lines it found drawn on it.
//!
//! ⚠⚠ **This is the one diagnostic that separates the two very different bugs behind "the guides
//! are not on the building".** If the lines land on the building *here*, the detector is right and
//! the fault is in how the guides are mapped to the canvas. If they do not, the detector is
//! genuinely choosing the wrong lines and no amount of overlay work will help.
//!
//! The plane written is exactly what `detect::verticals` consumes — post lens correction, post
//! orientation, pre-straighten, pre-perspective, pre-crop — so what you see is what it saw.
//!
//! `cargo run --release -p app --example detect_overlay -- <raw> [more raws...]`

use edit_model::Edits;
use pipeline::perspective::guides::Guide;
use pipeline::Resampler;
use rustroom_app::perspective_detect::{self, ANALYSIS_DIM};

/// Paint a normalised guide across the plane in a solid colour, 3 px wide.
fn draw(rgb: &mut [u8], w: usize, h: usize, g: &Guide, colour: [u8; 3]) {
    let steps = 4 * h.max(w);
    for k in 0..=steps {
        let t = k as f32 / steps as f32;
        let x = (g.from[0] + (g.to[0] - g.from[0]) * t) * (w - 1) as f32;
        let y = (g.from[1] + (g.to[1] - g.from[1]) * t) * (h - 1) as f32;
        for dy in -1i64..=1 {
            for dx in -1i64..=1 {
                let xi = x.round() as i64 + dx;
                let yi = y.round() as i64 + dy;
                if xi < 0 || yi < 0 || xi >= w as i64 || yi >= h as i64 {
                    continue;
                }
                let i = (yi as usize * w + xi as usize) * 3;
                rgb[i..i + 3].copy_from_slice(&colour);
            }
        }
    }
}

/// How this line's votes are distributed along it: total voting pixels, and the longest
/// CONTIGUOUS run of them.
///
/// ⚠⚠ **The distinction decides what the fix is.** `MIN_LENGTH_FRACTION` counts voting pixels
/// anywhere along a candidate, contiguous or not — so a line can reach its threshold from disjoint
/// fragments (a silhouette here, a railing there, a cloud edge) and be declared a building edge.
/// If the winning line's votes are scattered, requiring contiguity is the fix; if they are one run,
/// the line really is a long edge and the problem is which edges we prefer.
fn support(luma: &[f32], w: usize, h: usize, g: &Guide) {
    let enc: Vec<f32> = luma.iter().copied().map(color::linear_to_srgb).collect();
    let sob = pipeline::spatial::sobel(&enc, w, h);
    // The same comparison `detect::verticals` makes: levels across the edge, Sobel step gain 4.
    let floor = detect_floor_levels() / 255.0 * 4.0;
    let (mut total, mut best, mut run) = (0usize, 0usize, 0usize);
    for y in 0..h {
        let t = y as f32 / (h - 1).max(1) as f32;
        let x = ((g.from[0] + (g.to[0] - g.from[0]) * t) * (w - 1) as f32).round() as i64;
        // ⚠ Search a ±2 px band, not the single rounded column. An offset bin is a few pixels
        // wide, so the accumulator's own support is not confined to one column — and a
        // single-column probe UNDERSTATES contiguity for any line that is not exactly axis-aligned,
        // which was the first version of this measurement and would have set the threshold too low.
        let voting = (-2i64..=2).any(|d| {
            let xi = x + d;
            (0..w as i64).contains(&xi) && sob.mag[y * w + xi as usize] >= floor
        });
        if voting {
            total += 1;
            run += 1;
            best = best.max(run);
        } else {
            run = 0;
        }
    }
    println!(
        "      support: {total}/{h} rows vote ({:.0}%), longest contiguous run {best} ({:.0}%)",
        total as f32 * 100.0 / h as f32,
        best as f32 * 100.0 / h as f32
    );
}

/// The floor the shipped detector uses, in 8-bit levels.
fn detect_floor_levels() -> f32 {
    pipeline::perspective::detect::MIN_EDGE_LEVELS
}

/// Refine straight vertical guides drawn at the given frame fractions, and report how far each
/// moved — the measurement `SNAP_RADIUS` is chosen from.
///
/// ⚠ Amber is the line as drawn, green the refined one, so the move is visible on the PNG. A guide
/// that did not move prints `(UNCHANGED)` and its two colours coincide.
fn probe_refine(path: &str, at: &[(f32, f32)]) {
    let p = std::path::Path::new(path);
    let decoded = match raw_decode::decode_raw(p, &raw_decode::DecodeOptions::linear_scene()) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{}: decode failed: {e}", p.display());
            std::process::exit(2);
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
    let mut edits = Edits::default();
    edits.lens_correction.distortion = true;
    edits.lens_correction.tca = true;
    edits.lens_correction.vignetting = true;

    let drawn: Vec<Guide> = at
        .iter()
        .map(|&(top, bot)| Guide {
            from: [top, 0.0],
            to: [bot, 1.0],
        })
        .collect();
    let (luma, w, h) = perspective_detect::analysis_luma(
        &decoded.image,
        &edits,
        lens.clone(),
        Resampler::default(),
    );
    let out = perspective_detect::refine_guides(
        &decoded.image,
        &edits,
        lens,
        Resampler::default(),
        &drawn,
    );

    let mut rgb = vec![0u8; w * h * 3];
    for (i, &l) in luma.iter().enumerate() {
        let v = (color::linear_to_srgb(l) * 255.0).round().clamp(0.0, 255.0) as u8;
        rgb[i * 3..i * 3 + 3].copy_from_slice(&[v, v, v]);
    }
    println!("{}: {w}x{h}", p.display());
    for (d, r) in drawn.iter().zip(&out) {
        // ⚠ The same metric `refine` uses: the largest gap at either end, as a fraction of width.
        let gap = (r.from[0] - d.from[0]).abs().max((r.to[0] - d.to[0]).abs());
        println!(
            "  drawn {:.4}..{:.4} -> refined {:.4}..{:.4}   moved {:.4} of frame width{}",
            d.from[0],
            d.to[0],
            r.from[0],
            r.to[0],
            gap,
            if r == d { "  (UNCHANGED)" } else { "" }
        );
        draw(&mut rgb, w, h, d, [255, 200, 40]);
        draw(&mut rgb, w, h, r, [40, 220, 120]);
    }
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let outp = std::env::temp_dir().join(format!("refine_{stem}.png"));
    match image::save_buffer(&outp, &rgb, w as u32, h as u32, image::ColorType::Rgb8) {
        Ok(()) => println!("  wrote {}", outp.display()),
        Err(e) => eprintln!("  could not write {}: {e}", outp.display()),
    }
}

fn main() {
    let paths: Vec<String> = std::env::args().skip(1).collect();
    if paths.is_empty() {
        eprintln!("usage: detect_overlay <raw> [more raws...]");
        eprintln!("       detect_overlay --refine <raw> <fraction> [more fractions...]");
        std::process::exit(2);
    }
    if paths[0] == "--refine" {
        let Some((raw, at)) = paths[1..].split_first() else {
            eprintln!("usage: detect_overlay --refine <raw> <fraction> [more fractions...]");
            std::process::exit(2);
        };
        // ⚠ `top:bottom` so a guide can LEAN. Measuring a leaning building edge with a straight
        // vertical probe measures the wrong thing — the gap at one end is large however well the
        // line is placed — and a person tracing an edge traces its lean. A bare `x` means vertical.
        let at: Vec<(f32, f32)> = at
            .iter()
            .filter_map(|s| match s.split_once(':') {
                Some((a, b)) => Some((a.parse().ok()?, b.parse().ok()?)),
                None => {
                    let x: f32 = s.parse().ok()?;
                    Some((x, x))
                }
            })
            .collect();
        if at.is_empty() {
            eprintln!("give at least one guide as `x` or `top:bottom`, e.g. 0.30 or 0.89:0.97");
            std::process::exit(2);
        }
        probe_refine(raw, &at);
        return;
    }
    for path in &paths {
        let p = std::path::Path::new(path);
        let decoded = match raw_decode::decode_raw(p, &raw_decode::DecodeOptions::linear_scene()) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("{}: decode failed: {e}", p.display());
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
        let mut edits = Edits::default();
        edits.lens_correction.distortion = true;
        edits.lens_correction.tca = true;
        edits.lens_correction.vignetting = true;

        let (luma, w, h) = perspective_detect::analysis_luma(
            &decoded.image,
            &edits,
            lens.clone(),
            Resampler::default(),
        );

        // The plane as the detector sees it, display-encoded so it is legible on screen.
        let mut rgb = vec![0u8; w * h * 3];
        for (i, &l) in luma.iter().enumerate() {
            let v = (color::linear_to_srgb(l) * 255.0).round().clamp(0.0, 255.0) as u8;
            rgb[i * 3..i * 3 + 3].copy_from_slice(&[v, v, v]);
        }

        let found = perspective_detect::detect_verticals(
            &decoded.image,
            &edits,
            lens,
            Resampler::default(),
        );
        println!("{}: {w}x{h}, {} lines", p.display(), found.len());
        for (n, g) in found.iter().enumerate() {
            println!(
                "  [{n}] ({:.4}, {:.4}) -> ({:.4}, {:.4})",
                g.from[0], g.from[1], g.to[0], g.to[1]
            );
            let colour = if n == 0 {
                [255, 40, 40]
            } else {
                [40, 160, 255]
            };
            draw(&mut rgb, w, h, g, colour);
            support(&luma, w, h, g);
        }

        let stem = p
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let out = std::env::temp_dir().join(format!("detect_{stem}.png"));
        match image::save_buffer(&out, &rgb, w as u32, h as u32, image::ColorType::Rgb8) {
            Ok(()) => println!("  wrote {}", out.display()),
            Err(e) => eprintln!("  could not write {}: {e}", out.display()),
        }
        let _ = ANALYSIS_DIM;
    }
}
