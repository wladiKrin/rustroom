//! Does a GPU chain actually precede a declining gating-brush mask?
//!
//! ⚠⚠ **Task 1 of the resident-base plan, and the milestone changes shape if this fails.** Reading
//! `accelerate.rs:523` says a `GpuChain` is flushed immediately before a mask that declines, so a
//! `GpuImage` exists just before its download and could be offered to `GpuBrushRaster`. Two things
//! could make that never happen in practice:
//!
//! - the mask is the **first** tail stage, so `run` is empty and `flush` returns `None`;
//! - `worth_a_chain` refuses the run, so the stages go back on the CPU and no `GpuImage` is built.
//!
//! **Building a hand-off with no producer is the W5 failure mode — green and dead.** This checks
//! before anything is built.
//!
//! ```text
//! cargo run --release -p app --example probe_brush_base_producer <raw>
//! ```

use edit_model::{Brush, BrushStroke, Edits, LocalAdjustments, Mask, MaskedAdjustment};
use pipeline::{RenderScale, RenderSpec, Resampler, TailRendering};

fn gating_mask() -> MaskedAdjustment {
    // ⚠ `auto_mask: true` is the whole point — that is what makes `GpuMaskStage::new` decline and
    // leaves the CPU `MaskStage` to rasterise through `GpuBrushRaster`.
    let mask = Mask::Brush(Brush {
        strokes: vec![BrushStroke {
            decode: false,
            points: vec![[0.35, 0.35], [0.5, 0.5], [0.65, 0.4]],
            radius: 0.08,
            hardness: 50.0,
            flow: 100.0,
            erase: false,
            // ⚠ The flag lives on the STROKE, not the brush — it is baked at paint time.
            auto_mask: true,
            tolerance: 35.0,
        }],
        ..Default::default()
    });
    MaskedAdjustment {
        mask,
        adjustments: LocalAdjustments {
            exposure: 0.7,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: probe_brush_base_producer <raw>");
    let opts = raw_decode::DecodeOptions::preview();
    let d = raw_decode::decode_raw(std::path::Path::new(&path), &opts).expect("decode");
    let buffer = d.image;
    println!("buffer {}x{}", buffer.width(), buffer.height());

    let Some(ctx) = gpu_pipeline::shared() else {
        println!("⚠ no GPU context — this probe cannot answer anything");
        return;
    };
    let scale = RenderScale::new(1.0);
    let spec = RenderSpec {
        space: color::OutputSpace::Srgb,
        intent: color::RenderingIntent::default(),
        resampler: Resampler::default(),
        baseline: d.metadata.as_shot.unwrap_or_default(),
        airlight: None,
    };

    // ⚠⚠ Three weights, because a THIN fixture answers the wrong question. `flush` only builds a
    // chain when `worth_a_chain` says the accumulated run repays a round trip, so two cheap
    // per-pixel stages can refuse where a real edit set would not. A fixture too small to trigger
    // the mechanism measures the fixture — the error this session already made once, on a margin
    // sweep whose largest setting exceeded its own image.
    for (label, weight) in [
        ("NO global edits", 0),
        ("LIGHT globals (exposure + contrast)", 1),
        ("REALISTIC globals (the catalogue's own shape)", 2),
    ] {
        let mut e = Edits {
            masks: vec![gating_mask()],
            ..Default::default()
        };
        if weight >= 1 {
            e.exposure = 0.4;
            e.contrast = 25.0;
        }
        if weight >= 2 {
            // ⚠ The stages `bench_gpu_tail` calls "TYPICAL of this catalogue" — the ones an actual
            // drag has active around it.
            e.saturation = 20.0;
            e.vibrance = 15.0;
            e.local_contrast.clarity = 30.0;
            e.local_contrast.texture = 25.0;
            e.highlights = -30.0;
            e.shadows = 40.0;
        }

        let tail =
            pipeline::build_pipeline_split(&e, scale, pipeline::TailRendering::colorimetric()).1;
        let before: Vec<&str> = tail.stage_names();
        let (after, trailing) = gpu_pipeline::accelerate(
            tail,
            &e,
            scale,
            gpu_pipeline::TailFacts {
                region: None,
                rendering: TailRendering::default(),
                spec,
                // ⚠ This probe builds no prefix at all — it asks about a MASK's producer.
            },
            ctx,
        );
        let names: Vec<&str> = after.stage_names();
        let mask_at = names.iter().position(|n| *n == "mask");
        let chain_before = mask_at.is_some_and(|i| names[..i].contains(&"gpu_chain"));

        println!("\n── {label}");
        println!("  tail before accelerate: {before:?}");
        println!("  tail after  accelerate: {names:?}");
        println!("  trailing chain: {}", trailing.is_some());
        match (mask_at, chain_before) {
            (None, _) => println!("  ⚠⚠ NO `mask` stage in the tail — the fixture is wrong"),
            (Some(_), true) => println!(
                "  ✅ a `gpu_chain` PRECEDES the mask — there is a GpuImage to offer, so the \
                 producer fires"
            ),
            (Some(_), false) => println!(
                "  ⛔ NO `gpu_chain` before the mask — nothing to offer. This configuration cannot \
                 be helped by a resident base, and the milestone must not claim it."
            ),
        }

        // ⚠⚠ **Task 6: the shape above is not the win — run it.** `stage=` in the `BRUSHGPU` line
        // is the host memcpy the resident base deletes, and it is the only number this milestone
        // is allowed to claim. On the taking path it must read **0.00**; anything else means the
        // take missed and the mechanism is dead while every test stays green.
        //
        // ⚠ Five frames, because the first carries a cold allocation and a warm drag is what a
        // photographer holds. Read the BAND across frames 2-5, never frame 1 alone.
        if weight == 2 && std::env::var("RUSTROOM_DEBUG_MASK").is_ok() {
            eprintln!("  ── five frames of the REALISTIC config (read `stage=`):");
            for _ in 0..5 {
                let tail = pipeline::build_pipeline_split(
                    &e,
                    scale,
                    pipeline::TailRendering::colorimetric(),
                )
                .1;
                let (p, _) = gpu_pipeline::accelerate(
                    tail,
                    &e,
                    scale,
                    gpu_pipeline::TailFacts {
                        region: None,
                        rendering: TailRendering::default(),
                        spec,
                    },
                    ctx,
                );
                let t = std::time::Instant::now();
                let out = p.run(buffer.clone());
                eprintln!(
                    "     frame {:.1} ms -> {}x{}",
                    t.elapsed().as_secs_f64() * 1e3,
                    out.width(),
                    out.height()
                );
            }
        }
    }
}
