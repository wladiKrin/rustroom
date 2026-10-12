//! How far a mask's scene sliders are from Lightroom's offsets today (M10.12b's brainstorm).
//!
//! `cargo run --release -p app --example probe_mask_offsets -- <raw> <out-prefix>`
//!
//! Under one ordinary global edit, each scene slider is applied two ways: globally (what an offset
//! gives where a mask covers fully) and through a mask covering the whole frame (what Rustroom does
//! today — the mask's stages after the global tone curve, saturation, HSL and grading). Prints how
//! far apart the two are and writes `<out-prefix>-<case>.png`: global | mask, side by side.

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let decoded = raw_decode::decode_raw(
        std::path::Path::new(&args[0]),
        &raw_decode::DecodeOptions::linear_scene(),
    )
    .expect("decode");
    let img = decoded.image.downscale_to(1200);
    let base = edit_model::Edits {
        contrast: 25.0,
        highlights: -50.0,
        shadows: 30.0,
        saturation: 15.0,
        tone_curve: edit_model::ToneCurve {
            points: vec![[0.0, 0.0], [0.25, 0.2], [0.75, 0.82], [1.0, 1.0]],
        },
        rendering: Some(edit_model::Rendering::Camera),
        ..Default::default()
    };
    let everything = edit_model::Mask::Linear(edit_model::LinearGradient {
        from: [-1.0, 0.0],
        to: [-0.5, 0.0],
        invert: false,
    });
    let masked = |a: edit_model::LocalAdjustments| {
        let mut e = base.clone();
        e.masks.push(edit_model::MaskedAdjustment {
            mask: everything.clone(),
            adjustments: a,
            ..Default::default()
        });
        e
    };
    type Case = (&'static str, edit_model::Edits, edit_model::Edits);
    let cases: Vec<Case> = vec![
        (
            "exposure +1",
            edit_model::Edits {
                exposure: 1.0,
                ..base.clone()
            },
            masked(edit_model::LocalAdjustments {
                exposure: 1.0,
                ..Default::default()
            }),
        ),
        (
            "contrast +30",
            edit_model::Edits {
                contrast: 55.0,
                ..base.clone()
            },
            masked(edit_model::LocalAdjustments {
                contrast: 30.0,
                ..Default::default()
            }),
        ),
        (
            "highlights -40",
            edit_model::Edits {
                highlights: -90.0,
                ..base.clone()
            },
            masked(edit_model::LocalAdjustments {
                highlights: -40.0,
                ..Default::default()
            }),
        ),
        (
            "temp 6500K over a global 5000K",
            edit_model::Edits {
                white_balance: edit_model::WhiteBalance {
                    kelvin: Some(6500.0),
                    tint: 0.0,
                },
                ..base.clone()
            },
            {
                let mut e = masked(edit_model::LocalAdjustments {
                    white_balance: edit_model::WhiteBalance {
                        kelvin: Some(6500.0),
                        tint: 0.0,
                    },
                    ..Default::default()
                });
                e.white_balance.kelvin = Some(5000.0);
                e
            },
        ),
    ];
    let render = |e: &edit_model::Edits| {
        pipeline::build_pipeline(
            e,
            pipeline::RenderScale::FULL,
            pipeline::TailRendering::default(),
        )
        .run(img.clone())
        .to_rgb8()
    };
    let (w, h) = (img.width(), img.height());
    println!(
        "{w}x{h}, global edit: contrast 25, highlights -50, shadows 30, saturation 15, an S curve"
    );
    println!(
        "{:<32} {:>9} {:>9} {:>11} {:>9} {:>9}",
        "case", "mean Δ", "p99 Δ", "pixels >3", "clip G", "clip M"
    );
    for (name, g, m) in &cases {
        let (a, b) = (render(g), render(m));
        let mut d: Vec<u8> = a.iter().zip(&b).map(|(x, y)| x.abs_diff(*y)).collect();
        let mean = d.iter().map(|&v| v as f64).sum::<f64>() / d.len() as f64;
        let over = a
            .chunks(3)
            .zip(b.chunks(3))
            .filter(|(p, q)| p.iter().zip(q.iter()).any(|(x, y)| x.abs_diff(*y) > 3))
            .count();
        d.sort_unstable();
        let p99 = d[d.len() * 99 / 100];
        let clip = |v: &[u8]| v.chunks(3).filter(|p| p.contains(&255)).count();
        println!(
            "{name:<32} {mean:>9.2} {p99:>9} {:>10.1}% {:>8.1}% {:>8.1}%",
            100.0 * over as f64 / (w * h) as f64,
            100.0 * clip(&a) as f64 / (w * h) as f64,
            100.0 * clip(&b) as f64 / (w * h) as f64,
        );
        let mut strip = vec![0u8; w * 2 * h * 3];
        for y in 0..h {
            let row = y * w * 3;
            strip[y * w * 6..y * w * 6 + w * 3].copy_from_slice(&a[row..row + w * 3]);
            strip[y * w * 6 + w * 3..(y + 1) * w * 6].copy_from_slice(&b[row..row + w * 3]);
        }
        let slug: String = name
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        image::save_buffer(
            format!("{}-{slug}.png", args[1]),
            &strip,
            (w * 2) as u32,
            h as u32,
            image::ColorType::Rgb8,
        )
        .unwrap();
    }
}
