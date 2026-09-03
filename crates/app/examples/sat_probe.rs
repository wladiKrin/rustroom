//! Where the camera's extra saturation lives.
//!
//! `strip_vs_canvas` measures a whole-image mean: our render is **0.947×** the camera's
//! saturation. This asks whether that deficit is flat or varies with tone, because a flat one is
//! honestly modelled by a scalar and a varying one is not.
//!
//! ⚠ Our render goes through the **real pipeline** with the camera rendering on, so what is being
//! compared is what the app actually shows against what the camera actually wrote.
//!
//! ```text
//! cargo run --release -p app --example sat_probe <raw>...
//! ```

const NB: usize = 8;

fn sat(p: [f32; 3]) -> f32 {
    let mx = p[0].max(p[1]).max(p[2]);
    if mx < 0.02 {
        return f32::NAN;
    }
    (mx - p[0].min(p[1]).min(p[2])) / mx
}

fn luma(p: [f32; 3]) -> f32 {
    0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]
}

fn main() {
    // Summed across every frame, so the buckets are pooled rather than averaged per photo.
    let mut ours = [(0f64, 0usize); NB];
    let mut cam = [(0f64, 0usize); NB];

    for f in std::env::args().skip(1) {
        let path = std::path::PathBuf::from(&f);
        let Ok(thumb) = raw_decode::extract_thumbnail(&path) else {
            continue;
        };
        let Ok(d) = raw_decode::decode_raw(&path, &raw_decode::DecodeOptions::linear_scene())
        else {
            continue;
        };
        let long = thumb.width.max(thumb.height);
        let small = d.image.downscale_to(long);
        if small.width().abs_diff(thumb.width) > 2 {
            continue;
        }
        let out = pipeline::build_pipeline(
            &edit_model::Edits {
                rendering: edit_model::Rendering::Camera,
                ..Default::default()
            },
            pipeline::RenderScale::new(1.0),
        )
        .run(small);

        for y in 0..out.height().min(thumb.height) {
            for x in 0..out.width().min(thumb.width) {
                let o = out.pixel(x, y);
                let i = (y * thumb.width + x) * 4;
                let c = [
                    thumb.rgba[i] as f32 / 255.0,
                    thumb.rgba[i + 1] as f32 / 255.0,
                    thumb.rgba[i + 2] as f32 / 255.0,
                ];
                let (so, sc) = (sat(o), sat(c));
                if !so.is_finite() || !sc.is_finite() {
                    continue;
                }
                // ⚠ Bucketed by the **camera's** luma for both, so the two are compared over the
                // same pixels. Bucketing each by its own would pair different populations.
                let b = ((luma(c) * NB as f32) as usize).min(NB - 1);
                ours[b].0 += so as f64;
                ours[b].1 += 1;
                cam[b].0 += sc as f64;
                cam[b].1 += 1;
            }
        }
    }

    println!(
        "{:>12} {:>10} {:>10} {:>9} {:>10}",
        "tone", "ours", "camera", "ratio", "n"
    );
    let (mut num, mut den) = (0f64, 0f64);
    for b in 0..NB {
        if ours[b].1 < 2000 {
            continue;
        }
        let (o, c) = (ours[b].0 / ours[b].1 as f64, cam[b].0 / cam[b].1 as f64);
        println!(
            "{:>5.2}-{:<6.2} {o:10.4} {c:10.4} {:9.3}x {:10}",
            b as f32 / NB as f32,
            (b + 1) as f32 / NB as f32,
            o / c,
            ours[b].1
        );
        num += ours[b].0;
        den += cam[b].0;
    }
    println!("\npooled ours/camera: {:.4}x", num / den);
}
