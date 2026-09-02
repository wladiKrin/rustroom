//! Measure the camera's own rendering — scene-linear luminance in, camera JPEG luminance out —
//! over as many frames as you hand it, and emit control points ready to paste into
//! `display_rendering::BUILT_IN`.
//!
//! ⚠⚠ **Each frame's camera JPEG carries that frame's own metering decision, and pooling raw
//! frames would smear the curve with it.** So every frame is first rescaled by the gain that puts
//! its own middle grey at [`GREY`] — which is exactly the design's "the stage owns the shape,
//! Auto Tone owns the level" written as arithmetic. The spread of those gains across frames *is*
//! the metering variation, and it is reported, because if it were small the whole split would be
//! unnecessary.
//!
//! `cargo run --release -p app --example curve_probe <raw>...`

/// Middle grey, matching `display_rendering::GREY`.
const GREY: f32 = 0.18;
const T_MIN: f32 = -8.0;
const T_MAX: f32 = 5.0;

/// Bin edges in stops relative to grey — the domain the curve is defined over.
const NB: usize = 26;

fn edge(i: usize) -> f32 {
    GREY * 2f32.powf(T_MIN + (T_MAX - T_MIN) * i as f32 / NB as f32)
}

/// One frame's measured pairs, as (scene luminance, camera display-linear luminance).
fn measure(path: &std::path::Path) -> Option<Vec<(f32, f32)>> {
    let thumb = raw_decode::extract_thumbnail(path).ok()?;
    let d = raw_decode::decode_raw(path, &raw_decode::DecodeOptions::linear_scene()).ok()?;
    let long = thumb.width.max(thumb.height);
    let small = d.image.downscale_to(long);
    // ⚠ Both readers apply the camera's flip, so the two should agree; a frame where they do not
    // is skipped rather than sampled at mismatched coordinates, which would pair a highlight with
    // a shadow and quietly flatten the curve.
    if small.width().abs_diff(thumb.width) > 2 || small.height().abs_diff(thumb.height) > 2 {
        eprintln!("  {} skipped: geometry mismatch", path.display());
        return None;
    }
    let (tw, th) = (thumb.width, thumb.height);
    let mut out = Vec::new();
    for y in (0..small.height().min(th)).step_by(2) {
        for x in (0..small.width().min(tw)).step_by(2) {
            let s = color::luminance(small.pixel(x, y));
            let i = (y * tw + x) * 4;
            let p = &thumb.rgba[i..i + 3];
            let c = color::luminance([
                color::srgb_to_linear(p[0] as f32 / 255.0),
                color::srgb_to_linear(p[1] as f32 / 255.0),
                color::srgb_to_linear(p[2] as f32 / 255.0),
            ]);
            out.push((s, c));
        }
    }
    Some(out)
}

/// Median camera output per scene bin, `None` where a bin is too thin to trust.
fn binned(pairs: &[(f32, f32)]) -> Vec<Option<f32>> {
    let mut bins: Vec<Vec<f32>> = vec![Vec::new(); NB];
    for &(s, c) in pairs {
        if s < edge(0) || s >= edge(NB) {
            continue;
        }
        // log2 is monotone, so the bin index is direct arithmetic rather than a scan.
        let t = (s / GREY).log2();
        let b = (((t - T_MIN) / (T_MAX - T_MIN)) * NB as f32) as usize;
        if b < NB {
            bins[b].push(c);
        }
    }
    bins.iter_mut()
        .map(|v| {
            if v.len() < 200 {
                return None;
            }
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            Some(v[v.len() / 2])
        })
        .collect()
}

/// The scene luminance the camera renders as middle grey — this frame's metering, in one number.
fn grey_point(b: &[Option<f32>]) -> Option<f32> {
    for i in 0..NB - 1 {
        let (Some(a), Some(c)) = (b[i], b[i + 1]) else {
            continue;
        };
        if a <= GREY && c > GREY {
            let f = (GREY - a) / (c - a);
            let mid = |k: usize| (edge(k) * edge(k + 1)).sqrt();
            // Interpolate in log space, which is where the samples are evenly spaced.
            return Some((mid(i).ln() + f * (mid(i + 1).ln() - mid(i).ln())).exp());
        }
    }
    None
}

fn main() {
    let files: Vec<String> = std::env::args().skip(1).collect();
    assert!(!files.is_empty(), "usage: curve_probe <raw>...");

    let mut pooled: Vec<(f32, f32)> = Vec::new();
    let mut greys: Vec<f32> = Vec::new();
    for f in &files {
        let p = std::path::Path::new(f);
        let Some(pairs) = measure(p) else { continue };
        let b = binned(&pairs);
        let Some(g) = grey_point(&b) else {
            eprintln!("  {} skipped: no grey crossing", p.display());
            continue;
        };
        greys.push(g);
        let gain = GREY / g;
        println!(
            "{:<16} grey at scene {:.4}  -> gain {:.3}x ({:+.2} EV)",
            p.file_name().unwrap().to_string_lossy(),
            g,
            gain,
            gain.log2()
        );
        pooled.extend(pairs.iter().map(|&(s, c)| (s * gain, c)));
    }
    assert!(!greys.is_empty(), "no frame produced a curve");

    greys.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let (lo, hi) = (greys[0], greys[greys.len() - 1]);
    println!(
        "\n{} frames. Metering spread: grey point {:.4}..{:.4}, {:.2} EV.",
        greys.len(),
        lo,
        hi,
        (hi / lo).log2()
    );

    let b = binned(&pooled);
    println!(
        "\n{:>8} {:>10} {:>10} {:>8}",
        "EV", "scene", "camera", "ratio"
    );
    for (i, v) in b.iter().enumerate() {
        let Some(c) = v else { continue };
        let mid = (edge(i) * edge(i + 1)).sqrt();
        println!(
            "{:8.2} {:10.4} {:10.4} {:8.2}x",
            (mid / GREY).log2(),
            mid,
            c,
            c / mid
        );
    }

    // The control points, in the normalised log-exposure the spline is defined over.
    println!("\nBUILT_IN candidate:");
    for (i, v) in b.iter().enumerate() {
        let Some(c) = v else { continue };
        let mid = (edge(i) * edge(i + 1)).sqrt();
        let u = (((mid / GREY).log2() - T_MIN) / (T_MAX - T_MIN)).clamp(0.0, 1.0);
        println!("    [{u:.3}, {c:.4}],");
    }
}
