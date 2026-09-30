//! What does dropping a rung actually buy while PANNING at zoom?
//!
//! Run: `cargo run --release -p app --example bench_pan_rung <raw>`
//!
//! ⚠⚠ **This exists to settle one decision and nothing else.** The interactive rung was measured
//! at 137.9 ms on a *fitted, whole-image* render of 6.8 Mpx, and that is the number its design
//! rests on. Zoomed, a render is region-bounded to the viewport: at the reporter's 7.72x on a
//! 2167x1229 pt canvas the region is about 951x539 rendered pixels, half a megapixel. If dropping
//! a rung there is worth single-digit milliseconds, it is not worth a 2x blur across the whole
//! view.
//!
//! ⚠ Panning changes no edits, so the prefix is **cached** and only the tail runs. The prefix is
//! therefore built once per rung and excluded from the timing, which is what makes this the cost
//! of a pan rather than the cost of a first render.
//!
//! ⚠ Read the two rungs as a RATIO, not as a prediction of a frame time. `bench_masked`'s own
//! note applies: a synthetic bench prices a stage, not a session, and it understated the running
//! app by 2.6x once.
use edit_model::Edits;
use pipeline::RenderScale;
use std::time::Instant;

const FIT_CAP: usize = 1600;
/// The reporter's canvas and zoom, read off the `RUSTROOM_DEBUG_PLACE` log.
const AVAIL: (f32, f32) = (2167.2, 1229.5);
const FIT: f32 = 1.151217;
const ZOOM: f32 = 7.7168;

fn best_of<T>(n: usize, mut f: impl FnMut() -> T) -> (T, f64) {
    let mut best = f64::INFINITY;
    let mut out = None;
    for _ in 0..n {
        let t = Instant::now();
        let v = f();
        best = best.min(t.elapsed().as_secs_f64() * 1000.0);
        out = Some(v);
    }
    (out.unwrap(), best)
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: bench_pan_rung <raw>");
    let path = std::path::Path::new(&path);
    let opts = raw_decode::DecodeOptions::linear_scene();
    let d = raw_decode::decode_raw(path, &opts).expect("decode");
    let lens = d.metadata.lens.as_ref().and_then(|l| {
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
    let buf = std::sync::Arc::new(d.image);
    let buffer_long = buf.width().max(buf.height());
    println!("buffer {}x{}", buf.width(), buf.height());

    // ⚠ Lens corrections ON and a realistic tail, as the GUI has them: a bench on
    // `Edits::default()` prices a pipeline nobody runs.
    let edits = Edits {
        lens_correction: Some(edit_model::LensCorrection {
            distortion: true,
            tca: true,
            vignetting: true,
            scale: 0.0,
        }),
        exposure: 0.35,
        contrast: 20.0,
        sharpening: edit_model::Sharpening {
            amount: 40.0,
            ..edit_model::Sharpening::default()
        },
        ..Edits::default()
    };

    println!("\nregion tail, prefix cached (what a pan frame costs):");
    for rung in [FIT_CAP * 2, buffer_long] {
        let scale = RenderScale::new(rung as f32 / buffer_long as f32);
        let k = rung as f32 / FIT_CAP as f32;
        let px_per_rendered = FIT * ZOOM / k;
        let src = core_image::ImageBuf::clone(&buf).downscale_to(rung);
        let (pw, ph) = (src.width(), src.height());
        let rw = ((AVAIL.0 / px_per_rendered).ceil() as usize).min(pw);
        let rh = ((AVAIL.1 / px_per_rendered).ceil() as usize).min(ph);

        let (prefix, _) = pipeline::build_pipeline_split_in_region_with_brush(
            &edits,
            scale,
            None,
            None,
            pipeline::RenderSpec::default(),
            lens.clone(),
            pipeline::TailRendering::default(),
        );
        let mid = prefix.run(src);
        // The region as the canvas derives it, in rendered pixels, taken from the frame's middle.
        let (x0, y0) = ((pw - rw) / 2, (ph - rh) / 2);
        let region = pipeline::Region {
            full_w: pw,
            full_h: ph,
            x0,
            y0,
        };
        let cropped = mid.crop(x0, y0, rw, rh);
        let (_, tail_ms) = best_of(5, || {
            let (_, tail) = pipeline::build_pipeline_split_in_region_with_brush(
                &edits,
                scale,
                Some(region),
                None,
                pipeline::RenderSpec::default(),
                lens.clone(),
                pipeline::TailRendering::default(),
            );
            tail.run(cropped.clone())
        });
        println!(
            "  rung {rung:>5}  region {rw}x{rh} ({:.2} Mpx)  TAIL {tail_ms:8.2} ms",
            (rw * rh) as f64 / 1e6
        );
    }
    // ⚠⚠ **The margin sweep: what a bigger margin costs, and how far it lets you pan before the
    // region has to be re-rendered.** `with_margin` expands the visible rect on every side, so the
    // area grows as `(w + 2m)(h + 2m)` while the pan distance it buys grows only as `m`. The
    // question is where that stops being worth it.
    let rung = buffer_long;
    let scale = RenderScale::new(rung as f32 / buffer_long as f32);
    let k = rung as f32 / FIT_CAP as f32;
    let px_per_rendered = FIT * ZOOM / k;
    let src = core_image::ImageBuf::clone(&buf).downscale_to(rung);
    let (pw, ph) = (src.width(), src.height());
    let (prefix, _) = pipeline::build_pipeline_split_in_region_with_brush(
        &edits,
        scale,
        None,
        None,
        pipeline::RenderSpec::default(),
        lens.clone(),
        pipeline::TailRendering::default(),
    );
    let mid = prefix.run(src);
    let vw = ((AVAIL.0 / px_per_rendered).ceil() as usize).min(pw);
    let vh = ((AVAIL.1 / px_per_rendered).ceil() as usize).min(ph);
    println!("\nmargin sweep at rung {rung} (visible {vw}x{vh} rendered px):");
    println!(
        "{:>8}{:>14}{:>12}{:>14}{:>16}",
        "margin", "region", "Mpx", "tail ms", "pan before re-render"
    );
    for m in [0usize, 64, 128, 256, 512, 1024] {
        let rw = (vw + 2 * m).min(pw);
        let rh = (vh + 2 * m).min(ph);
        let (x0, y0) = ((pw - rw) / 2, (ph - rh) / 2);
        let region = pipeline::Region {
            full_w: pw,
            full_h: ph,
            x0,
            y0,
        };
        let cropped = mid.crop(x0, y0, rw, rh);
        let (_, ms) = best_of(3, || {
            let (_, tail) = pipeline::build_pipeline_split_in_region_with_brush(
                &edits,
                scale,
                Some(region),
                None,
                pipeline::RenderSpec::default(),
                lens.clone(),
                pipeline::TailRendering::default(),
            );
            tail.run(cropped.clone())
        });
        println!(
            "{m:>8}{:>14}{:>12.2}{ms:>14.2}{:>13.0} pt",
            format!("{rw}x{rh}"),
            (rw * rh) as f64 / 1e6,
            m as f32 * px_per_rendered
        );
    }

    println!(
        "\nPanning changes no edits, so this tail is the whole per-frame cost. If the two rows\n\
         are within a few ms, the rung drop buys nothing while panning and costs a 2x blur."
    );
}
