//! **The W5 track A checkpoint measurement.** What a `.cube` actually does through this pipeline
//! at each declared [`LutSpace`], on a real RAW.
//!
//! ⚠⚠ **This probe draws no conclusion and tunes nothing.** Per the spec's §7 and
//! `PARALLEL-WORK.md` §6, the space/placement convention is the user's call and this only
//! supplies the evidence. Do not add a "and therefore X" line to its output.
//!
//! ```text
//! cargo run --release -p app --example look_probe <raw>... [--cube <file>]...
//! ```
//!
//! With no `--cube`, it uses the three synthetic tables below. They are stand-ins with a stated
//! purpose, not substitutes for a third-party look: `IDENTITY` isolates the shaper's own error,
//! `FILMIC` has the shape of a film emulation (an S-curve, the thing that double-renders), and
//! `GRADE` has the shape of a colourist's grade (a teal/orange split that changes nothing tonally).

use edit_model::{Edits, Look, LutSpace, Rendering};
use pipeline::lut::Cube;
use std::sync::Arc;

fn write_cube<F: Fn([f32; 3]) -> [f32; 3]>(n: usize, f: F) -> String {
    let mut s = format!("LUT_3D_SIZE {n}\n");
    for b in 0..n {
        for g in 0..n {
            for r in 0..n {
                let d = (n - 1) as f32;
                let o = f([r as f32 / d, g as f32 / d, b as f32 / d]);
                s.push_str(&format!("{} {} {}\n", o[0], o[1], o[2]));
            }
        }
    }
    s
}

/// A filmic S-curve applied per channel — the shape of a film emulation, and therefore the shape
/// that double-renders when it is stacked on the camera rendering.
fn filmic(v: f32) -> f32 {
    // A plain smooth S about 0.5. Deliberately simple: the point is the *shape*, and any
    // contrast-adding curve exhibits the stacking.
    let x = v.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// Teal shadows, orange highlights, with luminance left alone — the shape of a grade.
fn grade(p: [f32; 3]) -> [f32; 3] {
    let y = 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2];
    let t = (y - 0.5) * 0.12;
    [(p[0] + t).clamp(0.0, 1.0), p[1], (p[2] - t).clamp(0.0, 1.0)]
}

fn stats(img: &core_image::ImageBuf) -> (f32, f32, f32, f32) {
    let (mut sum, mut n) = (0f64, 0usize);
    let (mut clipped_lo, mut clipped_hi) = (0usize, 0usize);
    let mut sat = 0f64;
    for y in 0..img.height() {
        for x in 0..img.width() {
            let p = img.pixel(x, y);
            let l = 0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2];
            sum += l as f64;
            n += 1;
            if p.iter().any(|c| *c <= 0.0005) {
                clipped_lo += 1;
            }
            if p.iter().any(|c| *c >= 0.9995) {
                clipped_hi += 1;
            }
            let mx = p[0].max(p[1]).max(p[2]);
            if mx > 0.02 {
                sat += ((mx - p[0].min(p[1]).min(p[2])) / mx) as f64;
            }
        }
    }
    let n = n.max(1) as f64;
    (
        (sum / n) as f32,
        (clipped_lo as f64 / n) as f32,
        (clipped_hi as f64 / n) as f32,
        (sat / n) as f32,
    )
}

fn render(
    img: core_image::ImageBuf,
    edits: &Edits,
    cube: Option<&Arc<Cube>>,
) -> core_image::ImageBuf {
    let (prefix, tail) = pipeline::build_pipeline_split_in_region_with_brush(
        edits,
        pipeline::RenderScale::new(1.0),
        None,
        None,
        pipeline::RenderSpec::default(),
        None,
        pipeline::ProfileRendering {
            look: cube,
            ..Default::default()
        },
    );
    tail.run(prefix.run(img))
}

fn stage_names(edits: &Edits, cube: Option<&Arc<Cube>>) -> String {
    let (_, tail) = pipeline::build_pipeline_split_in_region_with_brush(
        edits,
        pipeline::RenderScale::new(1.0),
        None,
        None,
        pipeline::RenderSpec::default(),
        None,
        pipeline::ProfileRendering {
            look: cube,
            ..Default::default()
        },
    );
    tail.stage_names().join(" → ")
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut raws = Vec::new();
    let mut cube_paths = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "--cube" && i + 1 < args.len() {
            cube_paths.push(args[i + 1].clone());
            i += 2;
        } else {
            raws.push(args[i].clone());
            i += 1;
        }
    }

    let mut cubes: Vec<(String, Arc<Cube>)> = Vec::new();
    for p in &cube_paths {
        match std::fs::read_to_string(p).ok().map(|t| Cube::parse(&t)) {
            Some(Ok(c)) => cubes.push((p.clone(), Arc::new(c))),
            Some(Err(e)) => println!("!! {p}: {e}"),
            None => println!("!! {p}: unreadable"),
        }
    }
    if cubes.is_empty() {
        println!("(no --cube given: using synthetic stand-ins — see this file's header)\n");
        cubes.push((
            "SYNTHETIC identity 33³".into(),
            Arc::new(Cube::parse(&write_cube(33, |p| p)).unwrap()),
        ));
        cubes.push((
            "SYNTHETIC filmic S-curve 33³ (film-emulation shape)".into(),
            Arc::new(
                Cube::parse(&write_cube(33, |p| {
                    [filmic(p[0]), filmic(p[1]), filmic(p[2])]
                }))
                .unwrap(),
            ),
        ));
        cubes.push((
            "SYNTHETIC teal/orange 33³ (grade shape)".into(),
            Arc::new(Cube::parse(&write_cube(33, grade)).unwrap()),
        ));
    }

    // The identity round-trip diagnostic's own input, independent of whatever `--cube` supplied.
    let identity = Arc::new(Cube::parse(&write_cube(33, |p| p)).unwrap());

    println!("== stage lists (rendering = Camera) ==");
    for space in LutSpace::ALL {
        let e = Edits {
            rendering: Rendering::Camera,
            look: Look {
                file: Some("x.cube".into()),
                space,
                strength: 100.0,
            },
            ..Default::default()
        };
        println!(
            "{:>14}: {}",
            space.label(),
            stage_names(&e, cubes.first().map(|(_, c)| c))
        );
    }
    let no_look = Edits {
        rendering: Rendering::Camera,
        ..Default::default()
    };
    println!("{:>14}: {}", "no look", stage_names(&no_look, None));

    for f in &raws {
        let path = std::path::PathBuf::from(f);
        let Ok(d) = raw_decode::decode_raw(&path, &raw_decode::DecodeOptions::linear_scene())
        else {
            println!("\n!! {f}: could not decode");
            continue;
        };
        let small = d.image.downscale_to(900);
        println!("\n== {f} ({}×{}) ==", small.width(), small.height());
        println!(
            "{:<46} {:>8} {:>9} {:>9} {:>8}",
            "case", "mean L", "shadow%", "hilite%", "sat"
        );

        let base = render(small.clone(), &no_look, None);
        let (m, lo, hi, s) = stats(&base);
        println!(
            "{:<46} {:>8.4} {:>8.2}% {:>8.2}% {:>8.4}",
            "camera rendering, no look",
            m,
            lo * 100.0,
            hi * 100.0,
            s
        );

        let flat = render(small.clone(), &Edits::default(), None);
        let (m, lo, hi, s) = stats(&flat);
        println!(
            "{:<46} {:>8.4} {:>8.2}% {:>8.2}% {:>8.4}",
            "colorimetric, no look",
            m,
            lo * 100.0,
            hi * 100.0,
            s
        );

        // ⚠⚠ **The number the space convention actually turns on.** An identity `.cube` declared
        // in a display space should give back exactly what the rendering produced; the shaper is
        // a 3×3 matrix and a power law each way, so it does not, and this says by how much *on
        // real content* rather than on a synthetic probe. Reported against an 8-bit level, which
        // is the unit the difference would have to be visible in.
        for space in [LutSpace::SrgbDisplay, LutSpace::Rec709] {
            let e = Edits {
                rendering: Rendering::Camera,
                look: Look {
                    file: Some("x.cube".into()),
                    space,
                    strength: 100.0,
                },
                ..Default::default()
            };
            // ⚠⚠ A **genuine** identity, never `cubes[0]`. With `--cube` supplied that was
            // whichever file was named first, so this line reported a real look's effect under an
            // "identity round trip" label — handed a film emulation it printed `worst 55.972 of an
            // 8-bit level, 100.000% of pixels`, which reads as a catastrophic shaper bug and is
            // nothing of the kind. The diagnostic is about the shaper, so it must supply its own
            // input.
            let out = render(small.clone(), &e, Some(&identity));
            let (mut worst, mut over) = (0f32, 0usize);
            let mut n = 0usize;
            for y in 0..out.height() {
                for x in 0..out.width() {
                    let (a, b) = (out.pixel(x, y), base.pixel(x, y));
                    let d = (0..3).map(|c| (a[c] - b[c]).abs()).fold(0.0f32, f32::max);
                    worst = worst.max(d);
                    if d > 1.0 / 255.0 {
                        over += 1;
                    }
                    n += 1;
                }
            }
            println!(
                "  identity round trip @ {:<13} worst {:.3} of an 8-bit level, {:.3}% of pixels over 1 level",
                space.label(),
                worst * 255.0,
                over as f64 / n.max(1) as f64 * 100.0
            );
        }

        for (name, cube) in &cubes {
            for space in LutSpace::ALL {
                let e = Edits {
                    rendering: Rendering::Camera,
                    look: Look {
                        file: Some("x.cube".into()),
                        space,
                        strength: 100.0,
                    },
                    ..Default::default()
                };
                let out = render(small.clone(), &e, Some(cube));
                let (m, lo, hi, s) = stats(&out);
                // ⚠ The BASENAME, not the first 30 characters of the path: four files in one
                // directory truncated to four identical labels, so every row read the same and
                // the table could not be attributed to its LUT at all. A synthetic label has no
                // separator, so `file_name` returns it whole and those rows are unaffected.
                let base = std::path::Path::new(name)
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| name.clone());
                let short: String = base.chars().take(30).collect();
                println!(
                    "{:<46} {:>8.4} {:>8.2}% {:>8.2}% {:>8.4}",
                    format!("{short} @ {}", space.label()),
                    m,
                    lo * 100.0,
                    hi * 100.0,
                    s
                );
            }
        }
    }
}
