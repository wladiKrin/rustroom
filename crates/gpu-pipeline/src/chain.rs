//! Running several GPU stages without returning to the CPU between them.

use crate::{GpuContext, GpuImage};
use core_image::ImageBuf;
use pipeline::PipelineStage;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// One stage's worth of GPU work.
///
/// ⚠⚠ Deliberately **not** `PipelineStage`. That trait's `apply(ImageBuf) -> ImageBuf` takes CPU
/// memory and returns CPU memory, so it **forces a round trip by its signature** — exactly right
/// as the seam and exactly wrong as the unit of GPU work. This one only records a dispatch.
pub trait GpuStage: Send + Sync {
    fn name(&self) -> &'static str;

    /// How many single-channel scratch planes this stage needs.
    ///
    /// ⚠ Defaults to **0**, so every stage written before M8.3c keeps working untouched.
    fn scratch_planes(&self) -> usize {
        0
    }

    /// Record this stage's dispatch. The image is bound at group 0, binding 0.
    ///
    /// ⚠ `scratch` holds exactly [`GpuStage::scratch_planes`] buffers — a slice rather than a
    /// count and an index, so a stage cannot reach past what it asked for. They are **transient**:
    /// the chain lends the same planes to every stage, so nothing written here survives the call.
    fn encode(
        &self,
        ctx: &GpuContext,
        pass: &mut wgpu::ComputePass,
        img: &GpuImage,
        scratch: &[wgpu::Buffer],
    );

    /// The CPU stage this one replaces.
    ///
    /// ⚠⚠ Not decoration. `apply` has no failure channel, so a chain that fails on the GPU must
    /// produce the CPU answer — and a stage exposing only `encode` would have **nothing to fall
    /// back to**. Each GPU stage carrying its own twin is the same reasoning `GpuExposureStage`
    /// already applies to itself; a parallel list on the chain would be two lists that can
    /// disagree about their own order.
    fn cpu(&self) -> &dyn PipelineStage;
}

/// A run of GPU stages sharing one upload and one download.
pub struct GpuChain {
    stages: Vec<Box<dyn GpuStage>>,
    ctx: Arc<GpuContext>,
    /// ⚠ A counter rather than a timing, the shape `RenderEngine::requests_issued` and
    /// `ThumbLoader`'s extraction counter already take: a timing assertion is flaky under load,
    /// and "one round trip for N stages" is an exact claim deserving an exact observable.
    ///
    /// ⚠⚠ **Per chain, not a global static, and that was a real bug rather than a preference.**
    /// Process-wide counters read `(6, 4)` instead of `(1, 1)` because `cargo test` runs tests in
    /// **parallel** and other chains' transfers landed between the before and after reads. The
    /// chain was correct; the observable was contaminated — the same class as the temp-path
    /// collisions this repo already records. Per-chain state cannot be perturbed by anyone else.
    uploads: AtomicUsize,
    downloads: AtomicUsize,
}

impl GpuChain {
    pub fn new(ctx: Arc<GpuContext>, stages: Vec<Box<dyn GpuStage>>) -> Self {
        Self {
            stages,
            ctx,
            uploads: AtomicUsize::new(0),
            downloads: AtomicUsize::new(0),
        }
    }

    /// Transfers **this chain** has performed, as `(uploads, downloads)`.
    pub fn transfer_counts(&self) -> (usize, usize) {
        (
            self.uploads.load(Ordering::Relaxed),
            self.downloads.load(Ordering::Relaxed),
        )
    }

    /// The widest scratch request among this chain's stages.
    ///
    /// ⚠⚠ The **max**, not the sum — the planes are transient within a stage, so stages share
    /// them. Summing would double the memory for nothing.
    pub fn scratch_needed(&self) -> usize {
        self.stages
            .iter()
            .map(|s| s.scratch_planes())
            .max()
            .unwrap_or(0)
    }

    pub fn len(&self) -> usize {
        self.stages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.stages.is_empty()
    }

    /// Run every stage's CPU twin, in order.
    fn cpu_fallback(&self, mut image: ImageBuf) -> ImageBuf {
        // ⚠ Every stage, not some — a partially-GPU result would be a third rendering path
        // nobody designed.
        for s in &self.stages {
            image = s.cpu().apply(image);
        }
        image
    }

    /// The GPU path, or `None` on any failure at all.
    fn run(&self, image: &ImageBuf) -> Option<Vec<f32>> {
        let ctx = &self.ctx;
        let device = &ctx.device;
        let img = GpuImage::new(ctx, image.width(), image.height())?;
        let bytes = img.bytes();

        // ── upload, once ──────────────────────────────────────────────────────────────
        // ⚠ A `MAP_WRITE` staging buffer rather than `queue.write_buffer`: measured 1.86 ms
        // against 3.90 for 19.6 MB, a 2.1x difference. The memcpy into mapped memory is the
        // floor (0.8-0.9 ms) and wgpu exposes no way around it.
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("chain-staging"),
            size: bytes,
            usage: wgpu::BufferUsages::MAP_WRITE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: true,
        });
        staging
            .slice(..)
            .get_mapped_range_mut()
            .copy_from_slice(bytemuck::cast_slice(image.data()));
        staging.unmap();
        let mut enc = device.create_command_encoder(&Default::default());
        enc.copy_buffer_to_buffer(&staging, 0, &img.buffer, 0, bytes);
        self.uploads.fetch_add(1, Ordering::Relaxed);

        // ── every stage, one pass ─────────────────────────────────────────────────────
        //
        // ⚠⚠ **WebGPU DOES guarantee memory visibility between dispatches within one compute
        // pass** — measured, not read: `three_chained_exposures_equal_one_combined_exposure`
        // passes, and gains multiply, so a missing barrier would give a wrong product rather than
        // a plausible image.
        //
        // ⚠ And the alternative is inert rather than wrong: a sabotage putting **each dispatch in
        // its own pass** also passes every test. So if a backend ever turns out to lack the
        // in-pass barrier, moving `begin_compute_pass` inside the loop costs nothing and the
        // buffer still stays resident. Recorded because a sabotage that does not fail is a finding
        // about the code, not a formality.
        //
        // ⚠ Scratch is allocated **once for the whole chain, and only when something asks** — so
        // an HSL-only chain allocates nothing and costs exactly what it did in M8.3b.
        let scratch = GpuImage::scratch(ctx, image.width(), image.height(), self.scratch_needed());
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            for s in &self.stages {
                s.encode(ctx, &mut pass, &img, &scratch);
            }
        }

        // ── download, once ────────────────────────────────────────────────────────────
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("chain-readback"),
            size: bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        enc.copy_buffer_to_buffer(&img.buffer, 0, &readback, 0, bytes);
        ctx.queue.submit(Some(enc.finish()));

        let slice = readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        rx.recv().ok()?.ok()?;
        let out: Vec<f32> = bytemuck::cast_slice(&slice.get_mapped_range()).to_vec();
        readback.unmap();
        self.downloads.fetch_add(1, Ordering::Relaxed);
        Some(out)
    }
}

impl PipelineStage for GpuChain {
    fn name(&self) -> &'static str {
        "gpu_chain"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        // ⚠ An empty chain must not upload at all: a chain with no stages is a legitimate state,
        // and paying a round trip to change nothing is the one thing it must never do.
        if self.stages.is_empty() {
            return image;
        }
        let (w, h) = (image.width(), image.height());
        match self.run(&image) {
            Some(data) => ImageBuf::from_vec(w, h, data),
            None => self.cpu_fallback(image),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GpuExposureStage;

    fn ctx() -> Option<Arc<GpuContext>> {
        match crate::shared() {
            Some(c) => Some(c.clone()),
            None => {
                eprintln!("SKIPPED: no GPU adapter on this machine");
                None
            }
        }
    }

    fn ramp(w: usize, h: usize) -> ImageBuf {
        let n = w * h * 3;
        ImageBuf::from_vec(w, h, (0..n).map(|i| (i % 601) as f32 / 601.0).collect())
    }

    #[test]
    fn three_chained_exposures_equal_one_combined_exposure() {
        // ⚠⚠ **THE BARRIER TEST, and the most important assertion in this slice.** Successive
        // dispatches in one compute pass write and then read the same storage buffer. If WebGPU
        // does not guarantee memory visibility between them, stage N+1 reads stage N's unwritten
        // data — a failure that is **intermittent and resolution-dependent**, fine on a small
        // fixture and corrupting on a photograph.
        //
        // ⚠ Gains multiply, so a missing barrier gives a WRONG PRODUCT rather than a plausible
        // image: +1.0, -0.5 and +0.25 EV compose to exactly +0.75 EV.
        let Some(ctx) = ctx() else { return };
        let img = ramp(128, 96);
        let chain = GpuChain::new(
            ctx.clone(),
            vec![
                Box::new(GpuExposureStage::new(1.0, ctx.clone())),
                Box::new(GpuExposureStage::new(-0.5, ctx.clone())),
                Box::new(GpuExposureStage::new(0.25, ctx.clone())),
            ],
        );
        let chained = chain.apply(img.clone());
        let single = pipeline::ExposureStage::new(0.75).apply(img);
        let d = crate::compare(&single, &chained);
        assert!(
            d.worst_levels < 0.01,
            "three chained exposures must equal one at +0.75 EV; worst {} levels, {} of {} \
             samples differ — suspect a missing barrier between dispatches",
            d.worst_levels,
            d.differing,
            d.total
        );
    }

    #[test]
    fn a_chain_of_eight_stages_performs_exactly_one_round_trip() {
        // ⚠⚠ The claim the whole slice exists for, asserted by COUNTING rather than timing — a
        // timing assertion is flaky under load, and this machine has had a background process at
        // 100% CPU throughout.
        let Some(ctx) = ctx() else { return };
        let stages: Vec<Box<dyn GpuStage>> = (0..8)
            .map(|_| Box::new(GpuExposureStage::new(0.1, ctx.clone())) as Box<dyn GpuStage>)
            .collect();
        let chain = GpuChain::new(ctx, stages);
        let _ = chain.apply(ramp(64, 48));
        assert_eq!(
            chain.transfer_counts(),
            (1, 1),
            "eight stages must share ONE upload and ONE download"
        );
    }

    #[test]
    fn an_empty_chain_returns_the_input_and_uploads_nothing() {
        // ⚠ Both halves. A chain with no stages is legitimate, and paying a round trip to change
        // nothing is the one thing it must never do — which a value-only assertion cannot see.
        let Some(ctx) = ctx() else { return };
        let img = ramp(32, 24);
        let chain = GpuChain::new(ctx, Vec::new());
        let out = chain.apply(img.clone());
        assert_eq!(out.data(), img.data(), "an empty chain changes nothing");
        assert_eq!(chain.transfer_counts(), (0, 0), "and transfers nothing");
    }

    #[test]
    fn a_chain_of_one_matches_the_cpu_stage() {
        let Some(ctx) = ctx() else { return };
        let img = ramp(64, 48);
        let chain = GpuChain::new(ctx.clone(), vec![Box::new(GpuExposureStage::new(1.3, ctx))]);
        let gpu = chain.apply(img.clone());
        let cpu = pipeline::ExposureStage::new(1.3).apply(img);
        assert_eq!(
            gpu.data(),
            cpu.data(),
            "one exposure is exactly representable on both"
        );
    }

    /// A stage that asks for scratch and checks what the chain lends it.
    ///
    /// ⚠ It carries a `noop` only because `cpu()` must return something — a `GpuStage` cannot be
    /// written without a twin, which is the point of that method.
    struct NeedsScratch {
        n: usize,
        noop: pipeline::ExposureStage,
    }

    impl NeedsScratch {
        fn new(n: usize) -> Self {
            Self {
                n,
                noop: pipeline::ExposureStage::new(0.0),
            }
        }
    }

    impl GpuStage for NeedsScratch {
        fn name(&self) -> &'static str {
            "needs_scratch"
        }
        fn scratch_planes(&self) -> usize {
            self.n
        }
        fn encode(
            &self,
            _ctx: &GpuContext,
            _pass: &mut wgpu::ComputePass,
            _img: &GpuImage,
            scratch: &[wgpu::Buffer],
        ) {
            assert_eq!(
                scratch.len(),
                self.n,
                "the chain must lend exactly what was asked for"
            );
        }
        fn cpu(&self) -> &dyn PipelineStage {
            &self.noop
        }
    }

    #[test]
    fn a_chain_allocates_the_widest_scratch_request_once() {
        // ⚠⚠ The **max**, not the sum: the planes are transient WITHIN a stage, so two stages
        // asking for two each can share the same two. Summing would double the memory for
        // nothing, and a test asserting only "at least two" could not tell the two apart.
        let Some(ctx) = ctx() else { return };
        let chain = GpuChain::new(
            ctx,
            vec![
                Box::new(NeedsScratch::new(1)),
                Box::new(NeedsScratch::new(3)),
            ],
        );
        assert_eq!(chain.scratch_needed(), 3);
    }

    #[test]
    fn a_chain_of_in_place_stages_allocates_no_scratch() {
        // ⚠ So an HSL-only chain costs exactly what it did in M8.3b. Without this, allocating
        // unconditionally would pass every other test while wasting ~26 MB on every render.
        let Some(ctx) = ctx() else { return };
        let chain = GpuChain::new(ctx.clone(), vec![Box::new(GpuExposureStage::new(1.0, ctx))]);
        assert_eq!(chain.scratch_needed(), 0);
    }

    #[test]
    fn a_stage_receives_the_planes_it_asked_for() {
        // ⚠ The carrier for the two above: `scratch_needed` can be right while `run` lends the
        // wrong slice, and only a stage that looks at what it was handed can see that.
        let Some(ctx) = ctx() else { return };
        let chain = GpuChain::new(ctx, vec![Box::new(NeedsScratch::new(2))]);
        let _ = chain.apply(ramp(16, 12));
        assert_eq!(chain.transfer_counts(), (1, 1), "the chain really ran");
    }

    #[test]
    fn the_fallback_runs_every_stages_cpu_twin_in_order() {
        // ⚠⚠ Asserted against the CPU stages' own composed output, not against "non-empty" — the
        // latter would pass against a fallback that returned the input unchanged.
        let Some(ctx) = ctx() else { return };
        let chain = GpuChain::new(
            ctx.clone(),
            vec![
                Box::new(GpuExposureStage::new(1.0, ctx.clone())),
                Box::new(GpuExposureStage::new(0.5, ctx)),
            ],
        );
        let img = ramp(16, 12);
        let want = pipeline::ExposureStage::new(0.5)
            .apply(pipeline::ExposureStage::new(1.0).apply(img.clone()));
        assert_eq!(chain.cpu_fallback(img).data(), want.data());
    }
}
