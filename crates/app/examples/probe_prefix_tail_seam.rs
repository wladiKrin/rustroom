//! What does the prefix→tail round trip actually cost, and on which path?
//!
//! ⚠⚠ **Measured BEFORE the seam is designed, because the backlog entry for it says in as many
//! words to** — *"price this as an EXPORT-side win, or measure the path the gesture actually takes
//! first."* Two things about that entry needed checking before anything was built:
//!
//! 1. **It is a whole-frame win, not a drag win.** On the region-scoped path
//!    `render_engine.rs:1190` keeps `gpu_prefix_steps` at 0 — *"a scoped chain is the CPU's"* — so
//!    there is no GPU prefix step, and therefore no round trip, on the path M8.12 made the
//!    interactive one.
//! 2. **The cheap shape saves only half.** The prefix step's download also feeds
//!    `cache.insert(key_at(i + 1), …)`, which wants an `ImageBuf`. Handing the buffer to the tail
//!    removes the *upload*; removing the download as well is what "`PrefixCache` holds a
//!    `GpuImage`" means.
//!
//! So this prints, per chain, the halves that a resident buffer would actually delete. Read
//! `memcpy` (host, deleted outright) beside `submit` (fused device upload + dispatches + download
//! — only its upload part goes) and `tovec` (host, deleted only by the big shape).
//!
//! ```text
//! RUSTROOM_DEBUG_XFER=1 cargo run --release -p app --example probe_prefix_tail_seam <raw>
//! ```
use edit_model::Edits;
use pipeline::{PipelineStage, RenderScale, RenderSpec, Resampler, TailRendering};

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: probe_prefix_tail_seam <raw>");
    // ⚠ Full decode, not `preview()`: this item is an EXPORT-side question, and a half-size buffer
    // would price a rung nobody exports at.
    let opts = raw_decode::DecodeOptions::default();
    let d = raw_decode::decode_raw(std::path::Path::new(&path), &opts).expect("decode");
    let buffer = d.image;
    println!(
        "buffer {}x{} ({:.1} Mpx)",
        buffer.width(),
        buffer.height(),
        (buffer.width() * buffer.height()) as f64 / 1e6
    );

    let Some(ctx) = gpu_pipeline::shared() else {
        println!("⚠ no GPU context — this probe cannot answer anything");
        return;
    };
    let scale = RenderScale::new(1.0);
    let resampler = Resampler::default();

    // ⚠⚠ Dehaze is the prefix step this entry was measured on, and it has a GPU twin. Without a
    // prefix stage that `accelerate_prefix_stage` accepts there is **no seam at all** and the
    // probe would report a zero that means "wrong fixture", not "no cost".
    // ⚠⚠ **TWO GPU prefix steps, not one, and the difference is the whole point of this item.**
    // The backlog entry says the prize is *"the difference between one round trip per frame and
    // four"* — a fixture with a single accelerated prefix step measures the FLOOR and would have
    // been reported as the answer. NR sits directly above dehaze in the prefix, and both have
    // twins, so the pair is what a real edit set with noise reduction on actually builds.
    let mut e = Edits {
        dehaze: edit_model::Dehaze { amount: 45.0 },
        noise_reduction: edit_model::NoiseReduction {
            luminance: 40.0,
            detail: 50.0,
            color: 35.0,
            smoothness: 50.0,
        },
        ..Default::default()
    };
    // ⚠ A tail worth a chain, in the shape `bench_gpu_tail` calls typical of this catalogue.
    e.exposure = 0.4;
    e.contrast = 25.0;
    e.saturation = 20.0;
    e.vibrance = 15.0;
    e.local_contrast.clarity = 30.0;
    e.highlights = -30.0;
    e.shadows = 40.0;

    let spec = RenderSpec {
        space: color::OutputSpace::Srgb,
        intent: color::RenderingIntent::default(),
        resampler,
        baseline: d.metadata.as_shot.unwrap_or_default(),
        airlight: None,
    };
    let airlight = pipeline::prefix_airlight(&e, &buffer, resampler, None);
    let steps = pipeline::build_prefix_steps(&e, scale, None, resampler, Some(airlight));
    println!(
        "prefix steps: {:?}",
        steps.iter().map(|s| s.stage.name()).collect::<Vec<_>>()
    );
    // ⚠ A fixture check on the SHAPE, not just the names: one accelerated prefix step has no
    // step-to-step seam at all, so a run that measured only that would report a floor as an answer.
    assert!(
        steps.len() >= 2,
        "fixture check: need at least two prefix steps, got {:?}",
        steps.iter().map(|s| s.stage.name()).collect::<Vec<_>>()
    );

    // ⚠ Four frames, for a BAND. The first pays cold allocations the shipped path pays once per session; a
    // second photo in a batch export pays the warm cost, and a batch is what M8.10 made this.
    for frame in 0..4 {
        eprintln!("\n── frame {frame}");
        let t_all = std::time::Instant::now();
        let mut img = buffer.clone();
        let mut accelerated_steps = 0;
        for step in &steps {
            let g = gpu_pipeline::accelerate_prefix_stage(
                step.stage.name(),
                &e,
                scale,
                &img,
                gpu_pipeline::SeamFacts {
                    lens: None,
                    resampler,
                    airlight,
                },
                ctx,
            );
            let t = std::time::Instant::now();
            img = match g {
                Some(mut g) => {
                    // ⚠ The production wiring, so the step->step seam this probe reports is the
                    // one that ships rather than one the probe arranged for itself.
                    g.allow_shared_input(gpu_pipeline::Slot::Prefix);
                    g.offer_output(gpu_pipeline::Slot::Prefix);
                    accelerated_steps += 1;
                    g.apply(img)
                }
                None => {
                    ctx.prefix_plane().clear();
                    step.stage.apply(img)
                }
            };
            eprintln!(
                "   prefix {:>16} {:7.1} ms",
                step.stage.name(),
                t.elapsed().as_secs_f64() * 1e3
            );
        }
        let tail =
            pipeline::build_pipeline_split(&e, scale, pipeline::TailRendering::colorimetric()).1;
        // ⚠⚠ **A fixture check, because the first run of this probe reported a 0.0 ms tail and
        // that is not a measurement, it is an empty pipeline.** A seam has two sides; a probe that
        // silently prices only one is the thin-fixture error this session has already made twice.
        assert!(
            !tail.stage_names().is_empty(),
            "fixture check: the tail must have stages, got none"
        );
        eprintln!("   tail stages: {:?}", tail.stage_names());
        // ⚠⚠ **`rejoin`, and dropping the trailing chain is what made this probe report a 0.0 ms
        // tail.** `accelerate` hands back a pipeline PLUS a trailing chain, and for this edit set
        // the whole tail went into that chain — so discarding it ran an empty pipeline and the
        // probe cheerfully printed a zero.
        //
        // ⚠ One honest difference from production: `render_cached` runs the trailing chain itself
        // with an **RGBA8** download (27 MB rather than 82 at rung 3200, M8.3g), where `rejoin`
        // takes the f32 path. That inflates the tail's `tovec`, and changes nothing about the
        // UPLOAD, which is the half this seam would delete.
        let (tail, trailing) = gpu_pipeline::accelerate(
            tail,
            &e,
            scale,
            gpu_pipeline::TailFacts {
                region: None,
                profile: TailRendering::default(),
                spec,
                // ⚠⚠ **False on purpose, so run A below genuinely uploads.** This probe measures
                // the seam by running the SAME chain both ways; letting `accelerate` wire the take
                // would make run A and run B identical and the saving would read as zero.
            },
            ctx,
        );
        let mut trailing = trailing.expect("fixture check: this edit set must end in a chain");
        let t = std::time::Instant::now();
        let mid = tail.run(img);
        let d_rest = t.elapsed();

        // ── A: the tail chain as it ships — it uploads ───────────────────────────────
        let t = std::time::Instant::now();
        let _ = trailing.apply(mid.clone());
        let d_upload = t.elapsed();

        // ── B: the same chain handed a resident plane ────────────────────────────────
        //
        // ⚠⚠ **The plane is the REAL one — the last prefix step's output, still parked.** An
        // earlier version of this probe deposited a freshly allocated `GpuImage` with undefined
        // contents, which prices the transfers correctly but proves nothing about the pixels. Once
        // the prefix loop above carries the production wiring there is a genuine plane in the slot,
        // and `take` is one-shot, so run A above (which is not allowed to take) leaves it for
        // run B.
        //
        // ⚠ Both halves go at once and cannot be separated in situ: the host memcpy AND the
        // device-side `copy_buffer_to_buffer` are skipped together when a chain takes, and the
        // device copy shares its submit with every dispatch. The backlog entry could only *infer*
        // the device half; this measures the pair.
        trailing.allow_shared_input(gpu_pipeline::Slot::Prefix);
        let t = std::time::Instant::now();
        let _ = trailing.apply(mid);
        let d_resident = t.elapsed();

        let saved = d_upload.as_secs_f64() * 1e3 - d_resident.as_secs_f64() * 1e3;
        eprintln!(
            "   tail rest {:6.1} | chain uploading {:7.1} vs resident {:7.1}  => SAVED {:6.1} ms \
             | whole frame {:.1} ms, {accelerated_steps} GPU prefix step(s)",
            d_rest.as_secs_f64() * 1e3,
            d_upload.as_secs_f64() * 1e3,
            d_resident.as_secs_f64() * 1e3,
            saved,
            t_all.elapsed().as_secs_f64() * 1e3,
        );
    }
}
