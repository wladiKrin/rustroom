//! Prices the three resampling prefix stages — lens correction, perspective and geometry — at a
//! native rung, and sizes the CPU win a rayon-parallel geometry/perspective would buy.
//!
//! ⚠⚠ **Written before the M8.3r spec, because this project's rule is measure before theorising**
//! — and that rule has corrected the premise of three consecutive milestones. Two things are
//! being asked, and only the first is the one the next-action block names:
//!
//! 1. What do the three actually cost at a native rung, separately? `CLAUDE.md` says lens
//!    correction alone is ~210 ms and is on by default for every RAW.
//! 2. ⚠ `geometry::rotate` and `PerspectiveStage::apply` are **sequential**, where
//!    `LensCorrectionStage` is rayon-parallel over output rows — and perspective's own comment
//!    says "if this ever needs to be faster, both should move together". So part of the number
//!    may be reachable on the CPU for a few lines, which would change what the GPU port is worth.
//!    This probe reimplements both in parallel to measure that, rather than assuming it.
//!
//! Run: `cargo run --release -p app --example probe_resamplers <raw>`

use core_image::ImageBuf;
use edit_model::Edits;
use pipeline::{PipelineStage, RenderScale, Resampler};
use rayon::prelude::*;
use std::time::Instant;

fn best_of<F: FnMut() -> ImageBuf>(n: usize, mut f: F) -> f64 {
    let mut best = f64::MAX;
    for _ in 0..n {
        let t = Instant::now();
        let out = f();
        best = best.min(t.elapsed().as_secs_f64() * 1000.0);
        std::hint::black_box(&out);
    }
    best
}

/// A rayon-parallel rotation, for measurement only — the same inverse map `geometry::rotate` uses.
///
/// ⚠ Parallel over **output** rows, which is sound for the same reason the lens stage's is: this
/// is a gather, so an output pixel reads the whole source and writes only itself.
fn rotate_parallel(src: &ImageBuf, angle_rad: f32, weights: &pipeline::resample::Weights) -> ImageBuf {
    let (w, h) = (src.width(), src.height());
    let cx = (w as f32 - 1.0) / 2.0;
    let cy = (h as f32 - 1.0) / 2.0;
    let (sin, cos) = angle_rad.sin_cos();
    const CH: usize = ImageBuf::CHANNELS;
    let stride = w * CH;
    let mut data = vec![0.0f32; w * h * CH];
    if stride == 0 {
        return ImageBuf::from_vec(w, h, data);
    }
    data.par_chunks_exact_mut(stride)
        .enumerate()
        .for_each(|(y, orow)| {
            for (x, out) in orow.as_chunks_mut::<CH>().0.iter_mut().enumerate() {
                let dx = x as f32 - cx;
                let dy = y as f32 - cy;
                let sx = cx + dx * cos + dy * sin;
                let sy = cy - dx * sin + dy * cos;
                *out = pipeline::resample::sample(src, sx, sy, weights);
            }
        });
    ImageBuf::from_vec(w, h, data)
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: probe_resamplers <raw>");
    let path = std::path::Path::new(&path);
    let opts = raw_decode::DecodeOptions::preview();
    let d = raw_decode::decode_raw(path, &opts).expect("decode");
    let buf = d.image;
    let (w, h) = (buf.width(), buf.height());
    let sc = RenderScale::new(1.0);
    println!(
        "buffer {w}x{h} ({:.1} Mpx), resampler {:?}, {} threads",
        (w * h) as f64 / 1e6,
        Resampler::default(),
        rayon::current_num_threads()
    );

    // The lens profile, resolved exactly as the decode thread does.
    let shot = d.metadata.lens.as_ref().and_then(|l| {
        lens_profile::Catalogue::shared()
            .and_then(|db| db.find(&d.metadata.make, &d.metadata.model, &l.name))
            .map(|profile| lens_profile::LensShot {
                profile,
                shot: lens_profile::Shot {
                    focal: l.focal,
                    aperture: l.aperture,
                },
            })
    });
    println!(
        "lens: {} -> {}",
        d.metadata.lens.as_ref().map(|l| l.name.as_str()).unwrap_or("(none in EXIF)"),
        if shot.is_some() { "PROFILE MATCHED" } else { "no profile" }
    );

    // Every resampling stage active at once, which is what a typical RAW with a straighten and a
    // perspective correction actually renders.
    let edits = Edits {
        lens_correction: Some(edit_model::LensCorrection {
            distortion: true,
            tca: true,
            vignetting: true,
            ..Default::default()
        }),
        perspective: edit_model::Perspective {
            vertical: 30.0,
            ..Default::default()
        },
        geometry: edit_model::Geometry {
            angle: 3.0,
            ..Default::default()
        },
        ..Edits::default()
    };

    let steps = pipeline::build_prefix_steps(&edits, sc, shot.clone(), Resampler::default());
    println!("\n  prefix steps: {}", steps.len());

    // ⚠ Each step is priced on the buffer the one before it produced, which is what the prefix
    // actually hands it — pricing them all on the raw buffer would be a different measurement.
    let mut img = buf.clone();
    let mut total = 0.0;
    for step in &steps {
        let name = step.stage.name();
        let ms = best_of(3, || step.stage.apply(img.clone()));
        total += ms;
        println!("{ms:8.1} ms   {name}");
        img = step.stage.apply(img);
    }
    println!("{total:8.1} ms   ALL THREE (the whole resampling prefix)");

    // ⚠ The clone alone, so the numbers above are not read as pure resampling: `apply` consumes
    // its input, so every measurement here includes one whole-buffer copy.
    let src = buf.clone();
    let clone_ms = best_of(5, || src.clone());
    println!("{clone_ms:8.1} ms   (a bare clone, included in every row above)");

    // The second question: what is sequential geometry costing?
    let weights = pipeline::resample::Weights::new(Resampler::default());
    let seq = best_of(3, || {
        pipeline::GeometryStage::new(
            &edit_model::Geometry {
                angle: 3.0,
                ..Default::default()
            },
            Resampler::default(),
        )
        .apply(src.clone())
    });
    let par = best_of(3, || rotate_parallel(&src, 3f32.to_radians(), &weights));
    println!(
        "\n  geometry's rotation, sequential {seq:.1} ms vs rayon {par:.1} ms — {:.2}x",
        seq / par
    );
}
