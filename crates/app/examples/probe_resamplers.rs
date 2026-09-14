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
fn rotate_parallel(
    src: &ImageBuf,
    angle_rad: f32,
    weights: &pipeline::resample::Weights,
) -> ImageBuf {
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

/// A rayon-parallel homography, for measurement only — the same inverse map
/// `PerspectiveStage::apply` uses.
///
/// ⚠ Parallel over **output** rows, sound for the same reason the rotation's is: a gather.
///
/// ⚠⚠ It builds the **real** `Homography` rather than reimplementing the transform — a hand-rolled
/// copy would measure a different map and report it as this one.
fn perspective_parallel(
    src: &ImageBuf,
    params: &edit_model::Perspective,
    weights: &pipeline::resample::Weights,
) -> ImageBuf {
    let (w, h) = (src.width(), src.height());
    let cx = (w as f32 - 1.0) / 2.0;
    let cy = (h as f32 - 1.0) / 2.0;
    let aspect = cx / cy;
    let hom = pipeline::perspective::Homography::from_edits(params, aspect);
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
                let n = ((x as f32 - cx) / cy, (y as f32 - cy) / cy);
                let s = hom.source_of(n);
                *out = pipeline::resample::sample(src, cx + s.0 * cy, cy + s.1 * cy, weights);
            }
        });
    ImageBuf::from_vec(w, h, data)
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: probe_resamplers <raw>");
    let path = std::path::Path::new(&path);
    // ⚠⚠ **`full` picks the FULL-RESOLUTION decode, which is what 1:1 actually renders from.**
    // Every earlier reading from this probe used `preview()` — the half-size buffer at 6.5 Mpx —
    // and a 1:1 view renders from the 26 Mpx one, FOUR times the pixels. Quoting the preview
    // figure as though it described the zoomed case is what made "visibly quicker at 1:1" wrong.
    let full = std::env::args().any(|a| a == "--full");
    let opts = if full {
        raw_decode::DecodeOptions::linear_scene()
    } else {
        raw_decode::DecodeOptions::preview()
    };
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
        d.metadata
            .lens
            .as_ref()
            .map(|l| l.name.as_str())
            .unwrap_or("(none in EXIF)"),
        if shot.is_some() {
            "PROFILE MATCHED"
        } else {
            "no profile"
        }
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

    let steps = pipeline::build_prefix_steps(&edits, sc, shot.clone(), Resampler::default(), None);
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

    let pp = edit_model::Perspective {
        vertical: 30.0,
        ..Default::default()
    };
    // ⚠ Reached through `build_prefix_steps` rather than by constructing the stage: `pipeline`
    // re-exports `GeometryStage` but NOT `PerspectiveStage`, so this is the only route from
    // outside the crate — and it is also the one the prefix itself takes.
    let psteps = pipeline::build_prefix_steps(
        &Edits {
            perspective: pp,
            ..Edits::default()
        },
        sc,
        None,
        Resampler::default(),
        None,
    );
    let pstage = psteps
        .iter()
        .find(|s| s.stage.name() == "perspective")
        .expect("fixture check: there must be a perspective step");
    let pseq = best_of(3, || pstage.stage.apply(src.clone()));
    let ppar = best_of(3, || perspective_parallel(&src, &pp, &weights));
    println!(
        "  perspective,             sequential {pseq:.1} ms vs rayon {ppar:.1} ms — {:.2}x",
        pseq / ppar
    );

    // ⚠⚠ **THE ROW THAT MATTERS IS A DRAG, NOT A STAGE — M8.3q's lesson applied here.** The three
    // rows above price each stage in isolation, and what a photographer feels is a *gesture*. A
    // Straighten drag re-runs only the steps from `geometry` onward: the lens-correction and
    // perspective checkpoints are keyed on edits the drag does not touch, so they stay valid.
    //
    // ⚠⚠ **So lens correction is CACHED throughout a Straighten drag and contributes nothing to
    // it** — which means porting it to the GPU, item 3 of the agreed order, does not touch this
    // symptom at all. Item 3 is the right answer to a cold render and a rung change; it is the
    // wrong answer to "the straighten drag is slow".
    {
        use pipeline::{build_pipeline_split_in_region, Rect, Region};
        // The common case: a RAW with lens corrections on (the default) and a straighten being
        // dragged. Perspective is left out deliberately — most photographs carry none, and it
        // would only be one more cached checkpoint the drag never reaches.
        let drag = Edits {
            lens_correction: Some(edit_model::LensCorrection {
                distortion: true,
                tca: true,
                vignetting: true,
                ..Default::default()
            }),
            geometry: edit_model::Geometry {
                angle: 3.0,
                ..Default::default()
            },
            ..Edits::default()
        };
        let dsteps =
            pipeline::build_prefix_steps(&drag, sc, shot.clone(), Resampler::default(), None);
        let mut checkpoints = vec![buf.clone()];
        for step in &dsteps {
            checkpoints.push(step.stage.apply(checkpoints.last().unwrap().clone()));
        }
        let mid = checkpoints.last().unwrap();
        // ⚠ The tail renders a REGION, which is what the app does at a native rung — the prefix
        // above does not, and that asymmetry is the finding rather than an artefact of the probe.
        let (rw, rh) = (1400.min(mid.width()), 900.min(mid.height()));
        let region = Region {
            full_w: mid.width(),
            full_h: mid.height(),
            x0: 0,
            y0: 0,
        };
        let (_, tail) = build_pipeline_split_in_region(&drag, sc, Some(region));
        println!("\n  a drag resumes at its own step; everything upstream stays cached:");
        for start in 0..=dsteps.len() {
            let label = if start == dsteps.len() {
                "a TAIL slider (nothing in the prefix re-runs)".to_string()
            } else {
                format!("a `{}` drag", dsteps[start].stage.name())
            };
            let mut best = f64::MAX;
            for _ in 0..3 {
                let t = Instant::now();
                let mut img = checkpoints[start].clone();
                for step in &dsteps[start..] {
                    img = step.stage.apply(img);
                }
                let sub = img.crop(0, 0, rw, rh);
                std::hint::black_box(tail.run(sub));
                best = best.min(t.elapsed().as_secs_f64() * 1000.0);
            }
            println!("{best:8.1} ms   {label}");
        }

        // ⚠⚠ **M8.6's own row: the same drag, with the last prefix step producing ONLY the
        // region.** A projection is not a result — this crate has had three composed projections
        // miss — so the milestone is judged by this line rather than by arithmetic on the one
        // above it.
        {
            let last = dsteps.len() - 1;
            let before = &checkpoints[last];
            let (ow, oh) = dsteps[last]
                .stage
                .output_dims(before.width(), before.height());
            let rect = Rect {
                x: 0,
                y: 0,
                w: rw.min(ow),
                h: rh.min(oh),
            };
            let mut best = f64::MAX;
            for _ in 0..3 {
                let t = Instant::now();
                let sub = dsteps[last]
                    .stage
                    .apply_region(&pipeline::RegionSrc::whole(before), rect)
                    .expect("fixture check: the last step must accept a region");
                std::hint::black_box(tail.run(sub));
                best = best.min(t.elapsed().as_secs_f64() * 1000.0);
            }
            println!("{best:8.1} ms   a `geometry` drag, REGION-SCOPED");
        }
        println!("           (the tail rendered a {rw}x{rh} region)");
    }
}
