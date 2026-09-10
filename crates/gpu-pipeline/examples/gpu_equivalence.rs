//! M8.3a's measurement: what does crossing the seam cost, and do the two paths agree?
//!
//! ⚠⚠ **This exists so the round trip is a number in the repo rather than a claim in a
//! document.** M8.2's spec had to be corrected because a projected figure was quoted as an
//! achieved one; `bench_thumb_cache` and `demosaic_foliage` are the precedent.
//!
//! ⚠ It is also the only END-TO-END proof this slice has, because nothing in the app uses the GPU
//! path (spec §8). A probe is not the app, and that distinction is real.
//!
//! ⚠ Run it in **release**. In a debug build acquiring the adapter alone takes ~44-75 s.
//! `cargo run --release -p gpu-pipeline --example gpu_equivalence [width] [height]`

use gpu_pipeline::{compare, GpuExposureStage};
use pipeline::PipelineStage;
use std::time::Instant;

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.0
}

fn main() {
    let mut a = std::env::args().skip(1);
    let w: usize = a.next().and_then(|s| s.parse().ok()).unwrap_or(1600);
    let h: usize = a.next().and_then(|s| s.parse().ok()).unwrap_or(1068);

    let t = Instant::now();
    let Some(ctx) = gpu_pipeline::shared() else {
        println!("no usable GPU adapter — the CPU path is what would run");
        return;
    };
    println!(
        "adapter: {} ({})   init {:.1} ms",
        ctx.adapter_name(),
        ctx.backend(),
        ms(t)
    );

    // ⚠ Scene-referred values, including above 1.0 and below 0: a shader that clamped would look
    // correct on a 0..1 fixture and destroy every highlight on a real photograph.
    let n = w * h * 3;
    let data: Vec<f32> = (0..n).map(|i| (i % 997) as f32 / 500.0 - 0.1).collect();
    let img = core_image::ImageBuf::from_vec(w, h, data);
    println!(
        "buffer {w}x{h}x3 f32 = {:.1} MB",
        (n * 4) as f64 / 1048576.0
    );

    let cpu_stage = pipeline::ExposureStage::new(1.3);
    let gpu_stage = GpuExposureStage::new(1.3, ctx.clone());

    println!("\n  round      cpu         gpu       ratio");
    let mut last = None;
    for round in 0..3 {
        let t = Instant::now();
        let cpu = cpu_stage.apply(img.clone());
        let cpu_ms = ms(t);

        let t = Instant::now();
        let gpu = gpu_stage.apply(img.clone());
        let gpu_ms = ms(t);

        println!(
            "  {round}      {cpu_ms:7.2} ms  {gpu_ms:7.2} ms   {:.1}x slower on GPU",
            gpu_ms / cpu_ms
        );
        last = Some((cpu, gpu));
    }

    let (cpu, gpu) = last.expect("three rounds ran");
    let d = compare(&cpu, &gpu);
    println!(
        "\n  divergence: worst {:.4} levels, {} of {} samples ({:.4}%)",
        d.worst_levels,
        d.differing,
        d.total,
        d.differing_percent()
    );
    println!(
        "  {}",
        if d.is_exact() {
            "BIT-EXACT — as expected: the gain is a uniform, so the shader does one multiply"
        } else {
            "⚠ NOT bit-exact. Exposure should be, unless a subnormal is involved. \
             Suspect the plumbing before suspecting float drift."
        }
    );
    println!(
        "\n  ⚠ The GPU is SLOWER, by design: one stage pays a full round trip to save ~0.04 ms\n\
           of compute. The win needs the whole tail resident, which is M8.3b."
    );
}
