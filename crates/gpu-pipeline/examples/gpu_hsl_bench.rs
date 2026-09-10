//! What does running HSL on the GPU actually save?
//!
//! ⚠ A chain of one stage still pays the full round trip, so the saving is a fraction of HSL's
//! CPU cost rather than all of it. This reports the real figure — the spec deliberately predicts
//! none, because M8.2's spec had to be corrected for quoting a projection as an achieved number.
//!
//! `cargo run --release -p gpu-pipeline --example gpu_hsl_bench [width] [height]`

use edit_model::{Hsl, HslBand};
use gpu_pipeline::{compare, GpuChain, GpuHslStage};
use pipeline::PipelineStage;
use std::time::Instant;

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
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

    let mut hsl = Hsl::default();
    for (i, b) in hsl.bands.iter_mut().enumerate() {
        *b = HslBand {
            hue: (i as f32 * 13.0) % 70.0 - 35.0,
            saturation: (i as f32 * 21.0) % 90.0 - 45.0,
            luminance: (i as f32 * 9.0) % 40.0 - 20.0,
        };
    }
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

    let cpu_stage = pipeline::HslStage::new(&hsl);
    let gpu_chain = GpuChain::new(ctx.clone(), vec![Box::new(GpuHslStage::new(&hsl, ctx))]);

    let mut last = None;
    // ⚠⚠ **Round 0 is not the steady state.** The first encode compiles the shader — measured at
    // ~150 ms against ~10 ms thereafter — which a user pays once on the first render after
    // enabling the toggle. Reported rather than warmed away, because it is real and they will
    // feel it.
    println!("  round      cpu         gpu       speed-up");
    for r in 0..5 {
        let t = Instant::now();
        let cpu = cpu_stage.apply(img.clone());
        let c = ms(t);
        let t = Instant::now();
        let gpu = gpu_chain.apply(img.clone());
        let g = ms(t);
        println!("  {r}      {c:7.2} ms  {g:7.2} ms   {:.2}x", c / g);
        last = Some((cpu, gpu));
    }
    let (cpu, gpu) = last.expect("five rounds ran");
    let d = compare(&cpu, &gpu);
    println!(
        "\n  divergence: worst {:.5} levels, {} of {} samples ({:.3}%)",
        d.worst_levels,
        d.differing,
        d.total,
        d.differing_percent()
    );
}
