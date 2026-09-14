//! How far the dehaze airlight moves when it is estimated from a 256 px replay of the prefix
//! rather than from dehaze's own full-resolution input.
//!
//! ⚠⚠ The measurement M8.7's spec commits to taking **before** the change is built on. Three
//! dehaze goldens move either way; what this says is whether a *photograph* moves.
//!
//! Usage: `cargo run --release -p app --example airlight_shift <raw> [more raws...]`
use pipeline::{build_prefix_steps, RenderScale, Resampler};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: airlight_shift <raw> [more raws...]");
        std::process::exit(2);
    }
    println!(
        "{:<26} {:>24} {:>24} {:>9}",
        "file", "airlight today", "airlight replayed", "8-bit"
    );
    let mut worst_overall = 0.0f32;
    for path in &args {
        let dec = match raw_decode::decode_raw(
            std::path::Path::new(path),
            &raw_decode::DecodeOptions::linear_scene(),
        ) {
            Ok(d) => d,
            Err(e) => {
                eprintln!("{path}: decode failed: {e}");
                continue;
            }
        };
        let img = dec.image;

        // ⚠ A crop, an angle AND noise reduction, so the prefix genuinely has steps between the
        // source and dehaze. That is the only case where the two definitions can disagree at all —
        // with dehaze as the first step the replay reduces to exactly today's computation.
        let mut edits = edit_model::Edits {
            dehaze: edit_model::Dehaze { amount: 60.0 },
            noise_reduction: edit_model::NoiseReduction {
                luminance: 30.0,
                color: 30.0,
                detail: 50.0,
                smoothness: 50.0,
            },
            ..Default::default()
        };
        edits.geometry.crop = edit_model::CropRect {
            left: 0.1,
            top: 0.1,
            right: 0.9,
            bottom: 0.9,
        };
        edits.geometry.angle = 3.0;

        let steps = build_prefix_steps(&edits, RenderScale::FULL, None, Resampler::CatmullRom);
        let dehaze_at = steps
            .iter()
            .position(|s| s.stage.name() == "dehaze")
            .expect("fixture check: dehaze must be in the prefix");

        // TODAY: run the prefix up to dehaze at full resolution, estimate from its output.
        let mut cur = img.clone();
        for s in &steps[..dehaze_at] {
            cur = s.stage.apply(cur);
        }
        let today = pipeline::dehaze_airlight(&cur);

        // REPLAYED: downscale the SOURCE to 256, run the same steps at that scale, estimate.
        let replayed = pipeline::prefix_airlight(&edits, &img, Resampler::CatmullRom, None);

        // ⚠ Reported in 8-bit display levels, which is what a photograph is judged in. A linear
        // difference near black reads as enormous while being invisible — the correction the
        // Whites slider and M8.4's demosaic comparison each needed.
        let enc = |v: f32| color::linear_to_srgb(v.clamp(0.0, 1.0)) * 255.0;
        let worst = (0..3)
            .map(|i| (enc(today[i]) - enc(replayed[i])).abs())
            .fold(0.0f32, f32::max);
        worst_overall = worst_overall.max(worst);
        let name: String = std::path::Path::new(path)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .chars()
            .take(26)
            .collect();
        println!(
            "{name:<26} {:>24} {:>24} {worst:>9.3}",
            format!("{:.4} {:.4} {:.4}", today[0], today[1], today[2]),
            format!("{:.4} {:.4} {:.4}", replayed[0], replayed[1], replayed[2]),
        );

        // ⚠⚠ The OTHER candidate's cost: carrying the last TRUE airlight through a gesture. What
        // matters there is not "replay vs true" but "true before the drag vs true after" — how
        // stale a held value actually goes while you drag. Measured rather than guessed.
        let true_at = |angle: f32, crop: edit_model::CropRect| {
            let mut e = edits.clone();
            e.geometry.angle = angle;
            e.geometry.crop = crop;
            let st = build_prefix_steps(&e, RenderScale::FULL, None, Resampler::CatmullRom);
            let d = st
                .iter()
                .position(|x| x.stage.name() == "dehaze")
                .unwrap_or(st.len());
            let mut c = img.clone();
            for x in &st[..d] {
                c = x.stage.apply(c);
            }
            pipeline::dehaze_airlight(&c)
        };
        let base_crop = edits.geometry.crop;
        let a0 = true_at(3.0, base_crop);
        let drift = |b: [f32; 3]| {
            (0..3)
                .map(|i| (enc(a0[i]) - enc(b[i])).abs())
                .fold(0.0f32, f32::max)
        };
        let d_small = drift(true_at(4.0, base_crop));
        let d_large = drift(true_at(8.0, base_crop));
        let d_crop = drift(true_at(
            3.0,
            edit_model::CropRect {
                left: 0.15,
                top: 0.15,
                right: 0.85,
                bottom: 0.85,
            },
        ));
        println!(
            "{:<26} held-airlight drift: 3->4 deg {d_small:.3}   3->8 deg {d_large:.3}   \
             crop 0.1->0.15 {d_crop:.3}",
            ""
        );
    }
    println!("\nWORST across all frames: {worst_overall:.3} of an 8-bit level");
    println!("  under ~1.0  -> invisible, proceed as designed");
    println!("  over  ~3.0  -> changes a photograph, and should change the design");
}
