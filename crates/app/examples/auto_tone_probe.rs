//! What Auto Tone actually does to a real photograph.
//!
//! ⚠⚠ **The "done when" of M5.4a is a judgement about a photograph, which an agent must not sign
//! off** (`docs/PARALLEL-WORK.md` §6). This produces the evidence instead: the three percentiles
//! the decision was made from, the three values it chose, and a before/after pair to look at.
//! `MEDIAN_TARGET`, `WHITE_TARGET` and `BLACK_TARGET` are starting values the user tunes from
//! exactly this output.
//!
//! ⚠ It goes through `auto_tone_tool::analyse` — the button's own path, including the 512 px
//! downscale and the crop mapping — rather than calling `pipeline::auto_tone` directly, so what
//! it prints is what the button would do and not a second implementation that could disagree.
//!
//! `cargo run --release -p app --example auto_tone_probe <raw> [out_dir]`
use rustroom_app::render_engine::render_once;
use std::path::PathBuf;

fn main() {
    let path = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("usage: auto_tone_probe <raw> [out_dir]"),
    );
    let out_dir = std::env::args().nth(2);
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "photo".into());

    let decoded =
        raw_decode::decode_raw(&path, &raw_decode::DecodeOptions::preview()).expect("decode");
    let buf = decoded.image;

    let neutral = edit_model::Edits::default();
    let (p, auto) = rustroom_app::auto_tone_tool::analyse(
        &buf,
        &neutral.geometry,
        &neutral.negative,
        pipeline::auto_tone::MEDIAN_TARGET,
    );

    println!("{stem}  {}x{}", buf.width(), buf.height());
    match p {
        Some(p) => println!(
            "  percentiles   p_lo(0.1%) {:.5}   median {:.5}   p_hi(99.9%) {:.5}",
            p.lo, p.median, p.hi
        ),
        None => println!("  percentiles   none (empty region)"),
    }
    println!(
        "  auto          exposure {:+.3} EV   whites {:+.1}   blacks {:+.1}",
        auto.exposure, auto.whites, auto.blacks
    );
    // What the median is being moved *to*, so the target can be judged rather than trusted.
    if let Some(p) = p {
        println!(
            "  median lands at {:.5} (target {:.2})",
            p.median * 2f32.powf(auto.exposure),
            pipeline::auto_tone::MEDIAN_TARGET
        );
    }

    let mut edited = edit_model::Edits::default();
    rustroom_app::auto_tone_tool::apply(&mut edited, &buf, None);

    if let Some(dir) = out_dir {
        std::fs::create_dir_all(&dir).expect("out dir");
        for (edits, tag) in [(&neutral, "before"), (&edited, "after")] {
            let f = render_once(
                &buf,
                edits,
                1600,
                0.5,
                color::OutputSpace::Srgb,
                pipeline::Resampler::default(),
                None,
            );
            // `render_once` hands back RGBA8 in the display space; JPEG wants RGB.
            let rgb: Vec<u8> = f
                .rgba
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| [p[0], p[1], p[2]])
                .collect();
            let out = format!("{dir}/{stem}_{tag}.jpg");
            image::save_buffer(
                &out,
                &rgb,
                f.width as u32,
                f.height as u32,
                image::ColorType::Rgb8,
            )
            .expect("write jpeg");
            println!("  wrote {out}");
        }
    }
}
