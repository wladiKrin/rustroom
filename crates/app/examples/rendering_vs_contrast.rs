//! Is the camera rendering just "darker with more contrast"?
//!
//! Fits the closest Exposure + Contrast pair to the rendered image and reports what is left over.
//!
//! ⚠⚠ **Exposure, Contrast and the tone regions all compute a SCALAR gain and multiply RGB by
//! it**, which preserves `(max−min)/max` exactly — so no combination of them can change
//! saturation at all, at any tone. The display rendering curves the three channels
//! independently and can. That is the difference this measures.
//!
//! ```text
//! cargo run --release -p app --example rendering_vs_contrast <raw>...
//! ```

fn sat(p: [f32; 3]) -> f32 {
    let mx = p[0].max(p[1]).max(p[2]);
    let mn = p[0].min(p[1]).min(p[2]);
    if mx <= 1e-5 {
        0.0
    } else {
        (mx - mn) / mx
    }
}

fn luma(p: [f32; 3]) -> f32 {
    0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]
}

fn render(buf: &core_image::ImageBuf, e: &edit_model::Edits) -> core_image::ImageBuf {
    pipeline::build_pipeline(
        e,
        pipeline::RenderScale::new(1.0),
        pipeline::ProfileRendering::colorimetric(),
    )
    .run(buf.clone())
}

fn pixels(img: &core_image::ImageBuf) -> Vec<[f32; 3]> {
    img.data().as_chunks::<3>().0.to_vec()
}

fn rmse(a: &[[f32; 3]], b: &[[f32; 3]]) -> f32 {
    let s: f32 = a
        .iter()
        .zip(b)
        .map(|(x, y)| (0..3).map(|c| (x[c] - y[c]).powi(2)).sum::<f32>())
        .sum();
    (s / (a.len() * 3) as f32).sqrt()
}

fn main() {
    for f in std::env::args().skip(1) {
        let path = std::path::PathBuf::from(&f);
        let Ok(d) = raw_decode::decode_raw(&path, &raw_decode::DecodeOptions::linear_scene())
        else {
            continue;
        };
        let small = d.image.downscale_to(700);

        let cam = pixels(&render(
            &small,
            &edit_model::Edits {
                rendering: Some(edit_model::Rendering::Camera),
                ..Default::default()
            },
        ));

        // The closest "darker with more contrast" the pipeline can express.
        let mut best = (f32::MAX, 0.0f32, 0.0f32, Vec::new());
        for ei in -30..=10 {
            for ci in 0..=20 {
                let (e, c) = (ei as f32 * 0.1, ci as f32 * 5.0);
                let p = pixels(&render(
                    &small,
                    &edit_model::Edits {
                        exposure: e,
                        contrast: c,
                        ..Default::default()
                    },
                ));
                let r = rmse(&cam, &p);
                if r < best.0 {
                    best = (r, e, c, p);
                }
            }
        }
        let (r, e, c, fit) = best;
        println!(
            "\n{}\n  best match: Exposure {e:+.1} EV, Contrast {c:+.0}  ->  residual {:.1}/255",
            path.file_name().unwrap().to_string_lossy(),
            r * 255.0
        );

        // Where the residual lives: saturation, bucketed by tone.
        const NB: usize = 5;
        let names = [
            "deep shadow",
            "shadow",
            "midtone",
            "highlight",
            "near white",
        ];
        let mut sc = [(0f32, 0f32, 0usize); NB];
        for (a, b) in cam.iter().zip(&fit) {
            let l = luma(*a);
            let i = ((l * NB as f32) as usize).min(NB - 1);
            sc[i].0 += sat(*a);
            sc[i].1 += sat(*b);
            sc[i].2 += 1;
        }
        println!(
            "  {:<12} {:>10} {:>10} {:>9}",
            "tone", "rendered", "exp+contr", "delta"
        );
        for (i, (a, b, n)) in sc.iter().enumerate() {
            if *n < 500 {
                continue;
            }
            let (a, b) = (a / *n as f32, b / *n as f32);
            println!(
                "  {:<12} {a:10.4} {b:10.4} {:+8.1}%",
                names[i],
                100.0 * (a / b - 1.0)
            );
        }
    }
}
