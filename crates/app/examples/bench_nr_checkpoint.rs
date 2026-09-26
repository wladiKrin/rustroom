//! What is the **post-noise-reduction prefix checkpoint** worth, now that noise reduction *and*
//! dehaze both have GPU twins?
//!
//! ⚠⚠ **This is the measurement M8.3o named as the condition for revisiting its own refusal.**
//! That milestone priced chaining the two prefix stages into one GPU round trip and chose per-step
//! instead, because chaining destroys the checkpoint that lets a Dehaze drag resume *after* noise
//! reduction. Its own note: *"it becomes right if NR is ported and the checkpoint's value is
//! re-measured."* NR was ported at M8.3q. This is the second half.
//!
//! **Two halves, measured separately, because they are different questions.**
//!
//! 1. **The drag, through `RenderEngine`** — the real worker, the real `PrefixCache`, the real
//!    `render_cached`, the real `pack`. Run it under `RUSTROOM_DEBUG_RENDER=1` and the per-step
//!    decomposition lands on stderr interleaved with these rows, which is why **every line this
//!    example prints goes to stderr**: a probe row that prices a stage against a resume the real
//!    chain never took is exactly how M8.6's 12.5–13.3x headline came to be retracted.
//!
//!    Three drags, and the asymmetry between them *is* the finding:
//!      - **A — drag `noise_reduction` itself.** The checkpoint is written, used once, and
//!        invalidated by the next frame, because `PrefixStepKey::NoiseReduction` holds the
//!        parameters. Every write is waste here.
//!      - **B — drag `dehaze`**, which is the next prefix step *downstream* of it. This is the
//!        only interaction in which the checkpoint is a hit: the drag resumes after NR.
//!      - **C — drag `exposure`**, a tail slider. The prefix resumes at its **final** checkpoint,
//!        so the post-NR one contributes nothing at all — the final one does.
//!
//! 2. **The price of keeping it, priced as one round trip.** Keeping the checkpoint means the NR
//!    step must hand back an `ImageBuf`, so its result is downloaded and the dehaze step uploads
//!    the same pixels again on the next line. Two chains of one stage against **one chain of two**
//!    is exactly that difference and nothing else — same stages, same buffer, same rung. A
//!    near-zero-work chain at the same resolution gives the transfer floor beside it.
//!
//! ⚠ It changes no behaviour and decides nothing. `PrefixCache` holding a device buffer is a
//! design question reserved for the user.
//!
//! ```text
//! RUSTROOM_DEBUG_RENDER=1 cargo run --release -p app --example bench_nr_checkpoint <raw>
//! ```

use edit_model::{Dehaze, Edits, NoiseReduction};
use pipeline::{PipelineStage, RenderScale};
use rustroom_app::render_engine::{FrameSpec, RegionRect, RenderEngine};
use std::sync::Arc;
use std::time::Instant;

/// Frames per drag, and how many of them are discarded as warm-up.
///
/// ⚠ The first frames of a drag are not the drag: frame 0 of case B still runs noise reduction
/// (nothing has written the checkpoint yet), and the first GPU encode compiles shaders.
const FRAMES: usize = 14;
const WARM: usize = 3;

/// A per-section offset applied to every slider a drag moves, so that two sections never walk the
/// same ladder.
///
/// ⚠⚠ **Measured into existence.** Without it the region-scoped section resumed at checkpoint 1
/// in the middle of a *noise-reduction* drag: the whole-frame section above it had already cached
/// checkpoints for exactly those luminance values, and `CacheKey` is `(PrefixKey, max_dim)` —
/// which both sections share. The rows then described a resume the drag they claim to measure
/// never takes. The offsets are far below any visible difference and cost identical work; all
/// they do is make the keys distinct, which is the same fix `bench_gpu_tail`'s disjoint exposure
/// ladders already carry for the coverage cache.
fn salt(section: usize) -> f32 {
    section as f32 * 0.137
}

/// The photograph these cases describe: noise reduction on, dehaze on, an ordinary tail.
///
/// ⚠ The camera rendering is `Some(Camera)` because a fresh photo is seeded that way by
/// `Settings::default_rendering` and it is active on 100% of this catalogue's sidecars —
/// `Edits::default()` leaves it *undecided*, which applies as OFF and would measure a tail with
/// no display rendering in it.
fn base_edits() -> Edits {
    Edits {
        noise_reduction: NoiseReduction {
            luminance: 50.0,
            color: 50.0,
            ..Default::default()
        },
        dehaze: Dehaze { amount: 60.0 },
        exposure: 0.5,
        whites: 25.0,
        blacks: -20.0,
        rendering: Some(edit_model::Rendering::Camera),
        ..Edits::default()
    }
}

/// One render through the engine, waited for. The engine coalesces, so a request must be polled
/// to completion before the next is issued or the measurement times a frame that was skipped.
fn frame_ms(engine: &RenderEngine, edits: &Edits, spec: FrameSpec) -> f64 {
    let t = Instant::now();
    engine.request(edits.clone(), spec);
    loop {
        if engine.poll_latest().is_some() {
            return t.elapsed().as_secs_f64() * 1000.0;
        }
        std::thread::sleep(std::time::Duration::from_micros(200));
    }
}

/// A band — min and max — never a single figure. This project retracted a headline for quoting
/// one point reading, and `max` alone is defined entirely by the most extreme sample.
fn band(v: &[f64]) -> (f64, f64, f64) {
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    (s[0], s[s.len() / 2], s[s.len() - 1])
}

/// Drive one drag: `n` frames, each with a different value of the slider `vary` moves.
///
/// ⚠ The slider must actually move between frames. `PrefixStepKey` is derived from the edits, so
/// two frames with the same value are a cache **hit** and would measure nothing.
fn drag(
    engine: &RenderEngine,
    spec: FrameSpec,
    label: &str,
    vary: impl Fn(&mut Edits, usize),
) -> (f64, f64, f64) {
    let mut kept = Vec::new();
    eprintln!("### DRAG {label}");
    for k in 0..FRAMES {
        let mut e = base_edits();
        vary(&mut e, k);
        let ms = frame_ms(engine, &e, spec);
        if k >= WARM {
            kept.push(ms);
        }
    }
    band(&kept)
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: bench_nr_checkpoint <raw>");
    let path = std::path::Path::new(&path);

    // ⚠ `preview()` — the half-size decode the app opens with, so the rungs below are the ones a
    // fitted photo actually renders at before the full-resolution handover.
    let opts = raw_decode::DecodeOptions::preview();
    let d = raw_decode::decode_raw(path, &opts).expect("decode");
    let (bw, bh) = (d.image.width(), d.image.height());
    let buffer = Arc::new(d.image);
    let buffer_long = bw.max(bh);
    eprintln!(
        "buffer {bw}x{bh} ({:.1} Mpx), source_scale {}",
        (bw * bh) as f64 / 1e6,
        opts.source_scale()
    );
    let Some(ctx) = gpu_pipeline::shared() else {
        eprintln!("no GPU adapter — this whole question is about a GPU round trip; stopping");
        return;
    };
    eprintln!("adapter: {} ({})", ctx.adapter_name(), ctx.backend());

    let engine = RenderEngine::new(
        Arc::clone(&buffer),
        opts.source_scale(),
        None,
        d.metadata.as_shot.unwrap_or_default(),
        Default::default(),
        true,
        || {},
    );

    // ── Part 1: the drags, through the real engine ──────────────────────────────────────────
    //
    // ⚠⚠ Two rungs, and they are different questions. **1600 is the fit texture**; **3200 is what
    // a fitted photo renders at on a large HiDPI window with `drag_quality: Full`** — the rung
    // M8.3o and M8.3q were both measured at, 3123x2085 on an X-T4.
    let mut section = 0usize;
    for &max_dim in &[1600usize, 3200] {
        let rendered = buffer_long.min(max_dim);
        let mpx = {
            let k = rendered as f64 / buffer_long as f64;
            (bw as f64 * k) * (bh as f64 * k) / 1.0e6
        };
        for &gpu in &[false, true] {
            section += 1;
            let sa = salt(section);
            let spec = FrameSpec {
                max_dim,
                region: None,
                display_space: color::OutputSpace::Srgb,
                intent: color::RenderingIntent::default(),
                resampler: pipeline::Resampler::default(),
                gpu_tail: gpu,
                grade_view: None,
                soft_proof: None,
                visualize: None,
            };
            eprintln!(
                "\n== rung {max_dim} (~{mpx:.1} Mpx), whole frame, gpu_tail {}",
                if gpu { "ON" } else { "off" }
            );
            // Warm the shaders and every upstream checkpoint before anything is timed.
            for _ in 0..2 {
                frame_ms(&engine, &base_edits(), spec);
            }

            let a = drag(
                &engine,
                spec,
                "A: `noise_reduction` (the stage ITSELF)",
                |e, k| {
                    e.noise_reduction.luminance = 40.0 + k as f32 + sa;
                },
            );
            let b = drag(
                &engine,
                spec,
                "B: `dehaze` (DOWNSTREAM, in the prefix)",
                |e, k| {
                    e.dehaze.amount = 50.0 + k as f32 + sa;
                },
            );
            let c = drag(&engine, spec, "C: `exposure` (a TAIL slider)", |e, k| {
                e.exposure = 0.3 + 0.02 * k as f32 + sa * 0.001;
            });
            eprintln!("  drag                                   min     median      max");
            for (label, v) in [
                ("A  drag noise_reduction (itself)", a),
                ("B  drag dehaze (downstream prefix)", b),
                ("C  drag exposure (tail)", c),
            ] {
                eprintln!("  {label:<34} {:7.1} {:9.1} {:8.1} ms", v.0, v.1, v.2);
            }
            eprintln!(
                "  B - A = {:+.1} ms median — what the post-NR checkpoint SAVES per downstream frame",
                b.1 - a.1
            );
        }
    }

    // ⚠ One region-scoped row, because it qualifies everything above. At a native rung **with a
    // viewport region** the whole prefix takes `run_region_plan` on the CPU and `gpu_prefix_steps`
    // stays 0 — no GPU prefix step runs, so no checkpoint is downloaded and none is written
    // (a scoped output is deliberately never cached). The checkpoint question does not arise
    // there at all, and a report that measured only `region: None` would not know it.
    {
        let spec = FrameSpec {
            max_dim: 3200,
            region: Some(RegionRect {
                x: bw / 4,
                y: bh / 4,
                w: (bw / 3).min(1600),
                h: (bh / 3).min(1000),
            }),
            display_space: color::OutputSpace::Srgb,
            intent: color::RenderingIntent::default(),
            resampler: pipeline::Resampler::default(),
            gpu_tail: true,
            grade_view: None,
            soft_proof: None,
            visualize: None,
        };
        eprintln!("\n== rung 3200 WITH a viewport region, gpu_tail ON (the scoped path)");
        for _ in 0..2 {
            frame_ms(&engine, &base_edits(), spec);
        }
        let sa = salt(section + 1);
        let a = drag(&engine, spec, "A(scoped): `noise_reduction`", |e, k| {
            e.noise_reduction.luminance = 40.0 + k as f32 + sa;
        });
        let b = drag(&engine, spec, "B(scoped): `dehaze`", |e, k| {
            e.dehaze.amount = 50.0 + k as f32 + sa;
        });
        eprintln!(
            "  A  drag noise_reduction            {:7.1} {:9.1} {:8.1} ms",
            a.0, a.1, a.2
        );
        eprintln!(
            "  B  drag dehaze                     {:7.1} {:9.1} {:8.1} ms",
            b.0, b.1, b.2
        );
    }

    // ── Part 2: what the checkpoint COSTS to keep, priced as exactly one round trip ──────────
    //
    // ⚠⚠ Two chains of one stage against one chain of two. Same two stages, same input buffer,
    // same rung — the *only* difference is that the split path downloads the post-NR pixels into
    // an `ImageBuf` and uploads them again. That difference is the checkpoint's write price, and
    // it is what M8.3o refused to spend and what it refused to save.
    for &max_dim in &[1600usize, 3200] {
        let rendered = buffer_long.min(max_dim);
        let sc = RenderScale::new(opts.source_scale() * rendered as f32 / buffer_long as f32);
        let base = if buffer_long <= max_dim {
            (*buffer).clone()
        } else {
            buffer.downscale_to(max_dim)
        };
        let e = base_edits();
        // ⚠ The CPU stage supplies the image the dehaze twin estimates its airlight from, so both
        // configurations construct an identical dehaze stage. Construction is outside every timed
        // region in both, so the airlight estimate is priced in neither.
        let post_nr =
            pipeline::NoiseReductionStage::new(&e.noise_reduction, sc).apply(base.clone());

        let nr = || {
            Box::new(gpu_pipeline::GpuNoiseReductionStage::new(
                &e.noise_reduction,
                sc,
                ctx,
            )) as Box<dyn gpu_pipeline::GpuStage>
        };
        let dz = || {
            // ⚠ The airlight is supplied rather than estimated from `post_nr`, matching the
            // engine after 2026-09-17. Estimating here would price a stage the app no longer builds.
            Box::new(gpu_pipeline::GpuDehazeStage::new(
                &e.dehaze,
                sc,
                pipeline::prefix_airlight(&e, &post_nr, pipeline::Resampler::default(), None),
                ctx,
            )) as Box<dyn gpu_pipeline::GpuStage>
        };

        let split_nr = gpu_pipeline::GpuChain::new(ctx.clone(), vec![nr()]);
        let split_dz = gpu_pipeline::GpuChain::new(ctx.clone(), vec![dz()]);
        let fused = gpu_pipeline::GpuChain::new(ctx.clone(), vec![nr(), dz()]);
        // A near-zero-work chain at the same resolution: upload, one multiply per sample,
        // download. The transfer floor, beside the difference above.
        let floor = gpu_pipeline::GpuChain::new(
            ctx.clone(),
            vec![Box::new(gpu_pipeline::GpuExposureStage::new(
                0.0,
                ctx.clone(),
            ))],
        );

        let mut s = Vec::new();
        let mut f = Vec::new();
        let mut z = Vec::new();
        let mut nr_only = Vec::new();
        for r in 0..FRAMES {
            let t = Instant::now();
            let mid = split_nr.apply(base.clone());
            let out = split_dz.apply(mid);
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            std::hint::black_box(&out);

            let t = Instant::now();
            let out = fused.apply(base.clone());
            let mf = t.elapsed().as_secs_f64() * 1000.0;
            std::hint::black_box(&out);

            let t = Instant::now();
            let out = floor.apply(base.clone());
            let mz = t.elapsed().as_secs_f64() * 1000.0;
            std::hint::black_box(&out);

            let t = Instant::now();
            let out = split_nr.apply(base.clone());
            let mn = t.elapsed().as_secs_f64() * 1000.0;
            std::hint::black_box(&out);

            if r >= WARM {
                s.push(ms);
                f.push(mf);
                z.push(mz);
                nr_only.push(mn);
            }
        }
        let (s0, s1, s2) = band(&s);
        let (f0, f1, f2) = band(&f);
        let (z0, z1, z2) = band(&z);
        let (n0, n1, n2) = band(&nr_only);
        eprintln!(
            "\n== the checkpoint's PRICE at rung {max_dim} ({}x{}, {:.1} Mpx)",
            base.width(),
            base.height(),
            (base.width() * base.height()) as f64 / 1e6
        );
        eprintln!("  row                                     min     median      max");
        eprintln!(
            "  NR + dehaze, SPLIT (checkpoint kept){:7.1} {:9.1} {:8.1} ms",
            s0, s1, s2
        );
        eprintln!(
            "  NR + dehaze, FUSED (no checkpoint) {:8.1} {:9.1} {:8.1} ms",
            f0, f1, f2
        );
        eprintln!(
            "  `noise_reduction` chain ALONE      {:8.1} {:9.1} {:8.1} ms",
            n0, n1, n2
        );
        eprintln!(
            "  zero-work round trip (floor)       {:8.1} {:9.1} {:8.1} ms",
            z0, z1, z2
        );
        eprintln!(
            "  SPLIT - FUSED = {:+.1} ms median — the download+upload the checkpoint costs",
            s1 - f1
        );
    }
}
