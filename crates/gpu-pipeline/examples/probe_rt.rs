//! What does one GPU round trip cost at each rung?
//!
//! ⚠⚠ A chain of one trivial stage **is** the transfer: M8.3a measured the compute at 0.04 ms
//! against ~6 ms of moving bytes, so what this prints is the toll a run has to be worth paying.
//! It is the other half of `pipeline`'s `probe_stages`, and together they are the evidence behind
//! M8.3e's cost gate.
//!
//! ⚠ It also establishes that the comparison is **scale-free** — 4x the pixels gives ~3.9x the
//! round trip — which is what lets a stage carry one cost constant rather than one per rung.
use core_image::ImageBuf;
use gpu_pipeline::{GpuChain, GpuExposureStage, GpuOutputTransformStage};
use pipeline::PipelineStage;
use std::time::Instant;

fn scene(w: usize, h: usize) -> ImageBuf {
    let mut px = Vec::with_capacity(w * h * 3);
    for i in 0..w * h {
        let t = (i % 4096) as f32 / 4096.0 * 1.2;
        px.extend_from_slice(&[t, t * 0.9, t * 1.1]);
    }
    ImageBuf::from_vec(w, h, px)
}

fn main() {
    let ctx = gpu_pipeline::shared().expect("no adapter");
    println!("adapter: {}", ctx.adapter_name());
    for (w, h) in [(1600usize, 1068usize), (3200, 2136)] {
        let img = scene(w, h);
        let mut best = f64::MAX;
        for _ in 0..5 {
            let c = img.clone();
            let chain = GpuChain::new(
                std::sync::Arc::clone(ctx),
                vec![Box::new(GpuExposureStage::new(
                    0.25,
                    std::sync::Arc::clone(ctx),
                ))],
            );
            let t = Instant::now();
            let out = chain.apply(c);
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            std::hint::black_box(&out);
            best = best.min(ms);
        }
        println!(
            "{w}x{h} ({:.1} MB): one-stage chain {best:.2} ms  <- essentially the round trip",
            (w * h * 12) as f64 / 1e6
        );

        // ⚠⚠ **The byte path, measured end to end rather than derived by subtraction — and the
        // measurement is what showed the derivation was FINE.** M8.3g's components predict
        // 26.80 - 8.42 = 18.4 ms here, and over seven runs this reads **15.7-21.0**, which
        // contains it. M8.3h's spec first claimed the derivation was "wrong by 45%" on the
        // strength of one top-of-band reading of 21.02 — comparing a point prediction against a
        // single sample, which is exactly the error M8.3g's control row exists to expose.
        //
        // ⚠ So the row earns its place for a different reason than first claimed: it gives the
        // **range**, and the range is what makes `TRANSFER_COST` defensible as a conservative
        // number rather than a central one. Quote a band from several runs, never one figure.
        //
        // ⚠ This is the evidence that `TRANSFER_COST` can stay a **single** conservative
        // constant: see its doc comment for the `display_rendering` case that rules out using
        // this number as a second threshold.
        let mut bbest = f64::MAX;
        for _ in 0..5 {
            let chain = GpuChain::new(
                std::sync::Arc::clone(ctx),
                vec![Box::new(GpuOutputTransformStage::new(
                    color::OutputSpace::Srgb,
                    color::RenderingIntent::default(),
                    ctx,
                ))],
            );
            let t = Instant::now();
            let out = chain.run_rgba8(&img);
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            assert!(out.is_some(), "the byte path must be available");
            std::hint::black_box(&out);
            bbest = bbest.min(ms);
        }
        println!(
            "{w}x{h}: one-stage chain, BYTE path  {bbest:.2} ms  <- trailing output_transform"
        );
    }
}
