//! What does a **merged** GPU run actually save?
//!
//! ⚠⚠ **Sharpening alone is break-even against the round trip; the win is the RUN** — stages
//! sharing one transfer. So this measures three configurations rather than one stage.
//!
//! ⚠ The third row is the one that moved: at M8.3c an active colour grade sat between `hsl` and
//! `sharpening`, **split the run into two chains**, and took the win from 1.52x to **1.04x**.
//! M8.3d accelerated the grade, so it is now **one chain of three** — the printed `chains` count
//! is what shows that half.
//!
//! ⚠ The spec deliberately predicts no number. M8.2's spec had to be corrected for quoting a
//! projection as an achieved figure, and M8.3b's own "single stage is net-negative" was true of
//! exposure and false generally.
//!
//! `cargo run --release -p gpu-pipeline --example gpu_run_bench [width] [height]`

use edit_model::{Edits, HslBand, Sharpening};
use pipeline::{Pipeline, PipelineStage, RenderScale};
use std::time::Instant;

/// The accelerated tail as one pipeline.
///
/// ⚠ M8.3g splits the tail's last chain out so `render_cached` can ask it for RGBA8; a bench
/// wants the whole thing back, and `rejoin` is what puts it there.
fn accel(
    tail: Pipeline,
    edits: &edit_model::Edits,
    scale: pipeline::RenderScale,
    ctx: &std::sync::Arc<gpu_pipeline::GpuContext>,
) -> Pipeline {
    let (p, c) = gpu_pipeline::accelerate(
        tail,
        edits,
        scale,
        gpu_pipeline::TailFacts {
            region: None,
            profile: Default::default(),
            spec: Default::default(),
            // ⚠ This bench builds no prefix, so there is never a plane to take.
        },
        ctx,
    );
    gpu_pipeline::rejoin(p, c)
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

fn hsl_edits() -> Edits {
    let mut e = Edits::default();
    for (i, b) in e.hsl.bands.iter_mut().enumerate() {
        *b = HslBand {
            hue: (i as f32 * 13.0) % 70.0 - 35.0,
            saturation: (i as f32 * 21.0) % 90.0 - 45.0,
            luminance: (i as f32 * 9.0) % 40.0 - 20.0,
        };
    }
    e
}

fn sharpened(mut e: Edits) -> Edits {
    e.sharpening = Sharpening {
        amount: 70.0,
        radius: 1.5,
        detail: 50.0,
        masking: 40.0,
    };
    e
}

fn main() {
    let mut a = std::env::args().skip(1);
    let w: usize = a.next().and_then(|s| s.parse().ok()).unwrap_or(1600);
    let h: usize = a.next().and_then(|s| s.parse().ok()).unwrap_or(1068);
    let Some(ctx) = gpu_pipeline::shared() else {
        println!("no usable GPU adapter — the CPU path is what would run");
        return;
    };
    println!("adapter: {} ({})", ctx.adapter_name(), ctx.backend());

    let n = w * h * 3;
    let img = core_image::ImageBuf::from_vec(
        w,
        h,
        (0..n).map(|i| (i % 719) as f32 / 719.0 * 1.2).collect(),
    );
    println!(
        "buffer {w}x{h}x3 f32 = {:.1} MB\n",
        (n * 4) as f64 / 1048576.0
    );

    let scale = RenderScale::new(1.0);
    let mut graded = sharpened(hsl_edits());
    graded.color_grading.shadows.saturation = 30.0;
    graded.color_grading.shadows.hue = 250.0;

    // ⚠⚠ **The case M8.3e exists for, and it is the one an ordinary photograph runs.** Exposure,
    // the tone regions and the camera rendering — active on 57%, 50% and **100%** of this
    // catalogue's 40 sidecars, against hsl's 0%. Every stage in it is per-pixel and they are
    // contiguous, because `build_pipeline_split` pushes a stage only when `is_active()`, so the
    // eleven this user never touches are not obstacles in the middle of the tail: they are absent.
    let mut typical = Edits {
        exposure: 0.45,
        whites: 25.0,
        blacks: -20.0,
        ..Edits::default()
    };
    typical.rendering = Some(edit_model::Rendering::Camera);

    // ⚠ The case the cost gate deliberately declines: `output_transform` alone, 28.6 ms of work
    // against a 34 ms transfer. It must report **no chain at all**, and a speed-up of ~1.00x
    // because nothing was accelerated — not a number below 1.00x, which would mean the gate
    // failed to fire.
    let colorimetric = Edits::default();

    let cases: [(&str, Edits); 5] = [
        ("TYPICAL: exposure + tone regions + rendering", typical),
        (
            "unedited, colorimetric (the gate should decline)",
            colorimetric,
        ),
        ("hsl only (M8.3b's case)", hsl_edits()),
        ("hsl + sharpening (one chain)", sharpened(hsl_edits())),
        ("hsl + grade + sharpening (one chain)", graded),
    ];

    for (label, edits) in cases {
        let tail = || {
            pipeline::build_pipeline_split(&edits, scale, pipeline::TailRendering::colorimetric()).1
        };
        let chains = accel(tail(), &edits, scale, ctx)
            .stage_names()
            .iter()
            .filter(|s| **s == "gpu_chain")
            .count();
        println!("── {label} — {chains} chain(s)");

        // ⚠⚠ **Round 0 is not the steady state.** The first encode compiles the shaders, which a
        // user pays once on the first render after enabling the toggle. Reported rather than
        // warmed away, because it is real and they will feel it.
        println!("  round        cpu         gpu     speed-up");
        let mut best = (f64::MAX, f64::MAX);
        let mut last: Option<(core_image::ImageBuf, core_image::ImageBuf)> = None;
        for r in 0..5 {
            let t = Instant::now();
            let cpu = tail().run(img.clone());
            let c = ms(t);
            let g_tail: Pipeline = accel(tail(), &edits, scale, ctx);
            let t = Instant::now();
            let gpu = g_tail.run(img.clone());
            let g = ms(t);
            println!("  {r:>5}   {c:8.2} ms  {g:8.2} ms   {:6.2}x", c / g);
            if r > 0 {
                best = (best.0.min(c), best.1.min(g));
            }
            last = Some((cpu, gpu));
        }
        let (cpu, gpu) = last.expect("five rounds ran");
        let d = gpu_pipeline::compare(&cpu, &gpu);
        println!(
            "  best of 4 (warm): cpu {:.2} ms, gpu {:.2} ms — {:.2}x",
            best.0,
            best.1,
            best.0 / best.1
        );
        println!(
            "  divergence: worst {:.5} levels over {} of {} samples\n",
            d.worst_levels, d.differing, d.total
        );
    }

    // ⚠⚠ **What merging is WORTH, priced directly rather than by subtraction.** The rows above
    // say what the feature delivers; this says what the *merge* delivers, by running the same two
    // stages as two chains — the shape `accelerate` produced before M8.3c, when it built one chain
    // per stage. Subtracting the split row from the merged one would confound the merge with the
    // colour grade's own cost, which is the arithmetic this project has been misled by before.
    let edits = sharpened(hsl_edits());
    let one = || {
        accel(
            pipeline::build_pipeline_split(&edits, scale, pipeline::TailRendering::colorimetric())
                .1,
            &edits,
            scale,
            ctx,
        )
    };
    let two = || {
        // One chain per replaceable stage, everything else untouched — M8.3b's behaviour.
        let stages =
            pipeline::build_pipeline_split(&edits, scale, pipeline::TailRendering::colorimetric())
                .1
                .into_stages();
        let mut out: Vec<Box<dyn PipelineStage>> = Vec::new();
        for s in stages {
            match s.name() {
                "hsl" => out.push(Box::new(gpu_pipeline::GpuChain::new(
                    ctx.clone(),
                    vec![Box::new(gpu_pipeline::GpuHslStage::new(&edits.hsl, ctx))],
                ))),
                "sharpening" => out.push(Box::new(gpu_pipeline::GpuChain::new(
                    ctx.clone(),
                    vec![Box::new(gpu_pipeline::GpuSharpenStage::new(
                        &edits.sharpening,
                        scale,
                        ctx,
                    ))],
                ))),
                _ => out.push(s),
            }
        }
        Pipeline::new(out)
    };
    println!("── what the MERGE is worth: hsl + sharpening, one chain against two");
    let (mut b1, mut b2) = (f64::MAX, f64::MAX);
    for r in 0..5 {
        let p = one();
        let t = Instant::now();
        let _ = p.run(img.clone());
        let m = ms(t);
        let p = two();
        let t = Instant::now();
        let _ = p.run(img.clone());
        let s = ms(t);
        println!("  {r:>5}   merged {m:8.2} ms   split {s:8.2} ms");
        if r > 0 {
            b1 = b1.min(m);
            b2 = b2.min(s);
        }
    }
    println!(
        "  best of 4 (warm): merged {b1:.2} ms, split {b2:.2} ms — the merge saves {:.2} ms\n",
        b2 - b1
    );

    println!(
        "⚠ A speed-up below 1.00x is a finding, not a rounding error: it says the round trip\n\
         ⚠ costs more than the stages saved, and the default should stay off."
    );
}
