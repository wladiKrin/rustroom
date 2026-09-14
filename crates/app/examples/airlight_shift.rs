//! How stable the dehaze airlight is, and what each candidate definition costs.
//!
//! ⚠⚠ The measurement M8.7's spec commits to taking **before** section 6 is built on. It answers
//! three questions in one decode:
//!
//!   1. **Replay** — how far a 256 px replay of the prefix differs from today's estimate.
//!   2. **Held** — how stale a carried value goes during an ordinary drag. This is what says the
//!      airlight is not a stable quantity at all.
//!   3. **Source** — how far a once-per-photo estimate from the SOURCE buffer differs from the
//!      true post-geometry one, swept across crop severity, because an uncropped airlight must be
//!      wrong exactly in proportion to how much the crop changes what is bright.
//!
//! Usage: `cargo run --release -p app --example airlight_shift <raw> [more raws...]`
use pipeline::{build_prefix_steps, RenderScale, Resampler};

/// 8-bit display levels: what a photograph is judged in. ⚠ A linear difference near black reads as
/// enormous while being invisible — the correction the Whites slider and M8.4's demosaic
/// comparison each needed.
fn levels(a: [f32; 3], b: [f32; 3]) -> f32 {
    let enc = |v: f32| color::linear_to_srgb(v.clamp(0.0, 1.0)) * 255.0;
    (0..3)
        .map(|i| (enc(a[i]) - enc(b[i])).abs())
        .fold(0.0f32, f32::max)
}

fn crop(l: f32, t: f32, r: f32, b: f32) -> edit_model::CropRect {
    edit_model::CropRect {
        left: l,
        top: t,
        right: r,
        bottom: b,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: airlight_shift <raw> [more raws...]");
        std::process::exit(2);
    }

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
        let name = std::path::Path::new(path)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();

        let base = edit_model::Edits {
            dehaze: edit_model::Dehaze { amount: 60.0 },
            noise_reduction: edit_model::NoiseReduction {
                luminance: 30.0,
                color: 30.0,
                detail: 50.0,
                smoothness: 50.0,
            },
            ..Default::default()
        };

        // The TRUE airlight: run the prefix up to dehaze at full resolution and estimate from its
        // output. This is what ships today.
        let truth = |angle: f32, c: edit_model::CropRect| {
            let mut e = base.clone();
            e.geometry.angle = angle;
            e.geometry.crop = c;
            let st = build_prefix_steps(&e, RenderScale::FULL, None, Resampler::CatmullRom);
            let d = st
                .iter()
                .position(|x| x.stage.name() == "dehaze")
                .unwrap_or(st.len());
            let mut cur = img.clone();
            for x in &st[..d] {
                cur = x.stage.apply(cur);
            }
            pipeline::dehaze_airlight(&cur)
        };

        // Candidate B: once per photo, from the SOURCE buffer. Independent of every edit, so its
        // drift under any drag is exactly zero by construction.
        let source = pipeline::dehaze_airlight(&img);

        println!("\n=== {name} ===");
        println!("  source-buffer airlight: {source:.4?}");

        let mild = crop(0.1, 0.1, 0.9, 0.9);
        let t_mild = truth(3.0, mild);

        // (1) replay vs today
        let mut e_mild = base.clone();
        e_mild.geometry.angle = 3.0;
        e_mild.geometry.crop = mild;
        let replayed = pipeline::prefix_airlight(&e_mild, &img, Resampler::CatmullRom, None);
        println!(
            "  (1) REPLAY   vs true            {:>7.3} levels",
            levels(replayed, t_mild)
        );

        // (2) how stale a held value goes
        println!(
            "  (2) HELD     3->4 deg {:>7.3}   3->8 deg {:>7.3}   crop 0.10->0.15 {:>7.3}",
            levels(t_mild, truth(4.0, mild)),
            levels(t_mild, truth(8.0, mild)),
            levels(t_mild, truth(3.0, crop(0.15, 0.15, 0.85, 0.85)))
        );

        // (4) ⚠⚠ THE DECIDING MEASUREMENT: does the replay WOBBLE LESS than the truth does?
        // The truth moves under a drag because the brightest 0.1% of 26 Mpx reshuffles when
        // resampled. The replay estimates from a 256 px copy, where the same 0.1% is ~65 pixels of
        // a heavily averaged image — so it may be far steadier. If it is, the replay is not merely
        // the least-bad option, it is better than what ships.
        let replay_at = |angle: f32, c: edit_model::CropRect| {
            let mut e = base.clone();
            e.geometry.angle = angle;
            e.geometry.crop = c;
            pipeline::prefix_airlight(&e, &img, Resampler::CatmullRom, None)
        };
        let r_mild = replay_at(3.0, mild);
        println!(
            "  (4) REPLAY drift  3->4 deg {:>7.3}   3->8 deg {:>7.3}   crop 0.10->0.15 {:>7.3}",
            levels(r_mild, replay_at(4.0, mild)),
            levels(r_mild, replay_at(8.0, mild)),
            levels(r_mild, replay_at(3.0, crop(0.15, 0.15, 0.85, 0.85)))
        );

        // (3) the source-buffer estimate against the truth, by crop severity
        println!("  (3) SOURCE   vs true, by crop:");
        for (label, c) in [
            ("none          ", crop(0.0, 0.0, 1.0, 1.0)),
            ("mild  .10-.90 ", mild),
            ("moderate .25-.75", crop(0.25, 0.25, 0.75, 0.75)),
            ("tight top-left  ", crop(0.0, 0.0, 0.3, 0.3)),
            ("tight bot-right ", crop(0.7, 0.7, 1.0, 1.0)),
            ("tight centre    ", crop(0.4, 0.4, 0.6, 0.6)),
        ] {
            let t = truth(0.0, c);
            println!(
                "        {label}  true {:.4} {:.4} {:.4}   diff {:>7.3} levels",
                t[0],
                t[1],
                t[2],
                levels(source, t)
            );
        }
    }
    println!(
        "\n⚠ (2) is the finding: if HELD drifts more than REPLAY differs, the airlight is not"
    );
    println!("  stable under a drag and 'match today exactly' is matching something unstable.");
    println!("⚠ (3) is candidate B's entire cost — and its drift under any drag is zero.");
}
