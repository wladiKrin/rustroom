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

    /// This stage's CPU cost, in milliseconds per 6.8 megapixels.
    ///
    /// ⚠⚠ **Used ONLY to decide whether a run is worth a round trip, never to decide what is
    /// rendered.** It fails toward "run on the CPU", which is always correct — a wrong number
    /// costs milliseconds, never pixels. That is what separates it from the cost heuristic
    /// `PrefixCache` deliberately refused, where a wrong guess would have changed what was
    /// cached and therefore what you saw.
    ///
    /// ⚠ Deliberately has **no default**, so adding a stage forces the measurement rather than
    /// silently inheriting someone else's. `pipeline`'s `probe_stages` is how the number is got.
    fn cpu_cost(&self) -> f32;

    /// How many single-channel scratch planes this stage needs.
    ///
    /// ⚠ Defaults to **0**, so every stage written before M8.3c keeps working untouched.
    fn scratch_planes(&self) -> usize {
        0
    }

    /// This stage's output size for an input of `w` x `h`. Default: unchanged.
    ///
    /// ⚠⚠ **Only GPU geometry overrides it (2026-09-26)**, and it may only shrink or permute —
    /// the chain writes the output into the same buffer (see `GpuImage::relabel`). It must agree
    /// with the CPU twin's `PipelineStage::output_dims`; the geometry twin delegates to it.
    fn output_dims(&self, w: usize, h: usize) -> (usize, usize) {
        (w, h)
    }

    /// This stage as one per-pixel function for a `w` x `h` image, or `None`.
    ///
    /// ⚠⚠ **`None` is the default, so a stage that reads neighbours declines BY CONSTRUCTION** —
    /// sharpening, local contrast, NR, dehaze, lens, geometry and a mask inherit it and keep their
    /// own `encode`. The twelve per-pixel twins return a [`crate::per_pixel::Part`], and the
    /// chain folds a run of them into one dispatch; see that module.
    fn per_pixel(&self, _w: usize, _h: usize) -> Option<crate::per_pixel::Part<'_>> {
        None
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

/// Where a chain's input comes from.
#[derive(Clone, Copy)]
pub enum Input<'a> {
    /// Uploaded through staging (or taken from a deposited plane).
    Cpu(&'a ImageBuf),
    /// Already on the GPU: copied GPU-side, no upload. See [`GpuChain::apply_from`].
    Resident(&'a GpuImage),
}

/// What the chain downloads.
///
/// ⚠⚠ The modes share **one** upload, one compute pass and one submit — the whole point is that
/// only the *download* differs, so a second code path would be two things that could come to
/// disagree about how a chain runs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Download {
    /// No readback at all: the output stays on the GPU, handed to the caller or deposited for the
    /// next chain. See [`GpuChain::run_keeping_output`].
    None,
    /// The resident f32 image: `w * h * 12` bytes.
    Float,
    /// Packed RGBA8, quantized by a final dispatch: `w * h * 4` bytes.
    Rgba8,
}

/// What a run hands back beside its download.
///
/// ⚠⚠ **The resident final (2026-09-26).** The tail's input is kept on the GPU under its key —
/// either the pristine upload of a tail miss (`Input`) or the last prefix step's result
/// (`Output`) — so the next tail frame copies it GPU-side instead of uploading 82 MB.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Keep {
    Nothing,
    /// A GPU-side copy of the input, taken before any stage is encoded.
    Input,
    /// The chain's own image, after every stage; never deposited as well.
    Output,
}

/// Which hand-off slot a chain means.
///
/// ⚠⚠ **Two slots, and they must never be merged.** `Mask` is the plane a mask's gate uploaded,
/// offered to that mask's first sub-stage; `Prefix` is one GPU prefix step's output, offered to
/// the next step or to the tail's first chain. Merging them would let the outer tail's chain take
/// a mask's leftover, which is exactly what
/// `the_outer_tails_own_chain_never_takes_a_masks_leftover_plane` was written — and sabotaged — to
/// prevent. See `GpuContext::prefix_plane`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    /// A mask gate's base plane, living for one `MaskStage::apply`.
    Mask,
    /// A GPU prefix step's output, living until a CPU step clears it.
    Prefix,
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
    /// Whether this chain may take a base plane a mask's gate deposited.
    ///
    /// ⚠⚠ **False unless `accelerate::walk` marked it, and only the FIRST sub-stage of a mask is
    /// ever marked.** `MaskStage::apply` clones the image and pipes it through its sub-stages in
    /// order, so only `stages[0]` receives the plane the gate was handed; a chain at index 1 gets
    /// whatever index 0 produced, and taking the deposit there would render a picture that is
    /// plausible and wrong. Opt-in means the rule is decided where the position is known — at the
    /// push — and nowhere else.
    take_shared: Option<Slot>,
    offer_output: Option<Slot>,
    /// Whether this chain times itself. See `timing.rs`.
    ///
    /// ⚠ Read from `RUSTROOM_DEBUG_GPU_TIME` at construction and settable per chain, so a test can
    /// turn it on for one chain without touching the process environment — which is shared by
    /// every test running beside it.
    timing: bool,
    /// What the last timed run measured, for the tests and for a probe that wants a table.
    last_timings: std::sync::Mutex<Option<crate::timing::Timings>>,
    /// Whether runs of per-pixel twins fuse. From `RUSTROOM_NO_FUSION` at construction; settable
    /// per chain for the tests, like `timing`.
    fusion: bool,
    /// Fused runs and single stages this chain has encoded — the observable, since a fused chain
    /// and an unfused one produce the same picture by design.
    fused_units: AtomicUsize,
    single_units: AtomicUsize,
}

impl GpuChain {
    pub fn new(ctx: Arc<GpuContext>, stages: Vec<Box<dyn GpuStage>>) -> Self {
        Self {
            stages,
            ctx,
            uploads: AtomicUsize::new(0),
            downloads: AtomicUsize::new(0),
            take_shared: None,
            offer_output: None,
            timing: std::env::var_os("RUSTROOM_DEBUG_GPU_TIME").is_some(),
            last_timings: std::sync::Mutex::new(None),
            fusion: crate::per_pixel::fusion_enabled(),
            fused_units: AtomicUsize::new(0),
            single_units: AtomicUsize::new(0),
        }
    }

    /// Offer this chain's **output** buffer to a consumer that needs the same bytes next.
    ///
    /// ⚠⚠ **Opt-in, and set only on a chain flushed immediately before a mask that declined.**
    /// `SharedPlane` is one-shot and thread-stamped for the reason its own docs give — a plane
    /// taken by the wrong frame renders *a plausible photograph, not an error* — so the fewer
    /// producers the slot has, the fewer ways that happens. Always-on would deposit for every
    /// chain in the tail, most of which nobody takes.
    ///
    /// ⚠ Called by `accelerate::walk` and by nothing else, like [`Self::allow_shared_input`].
    pub fn offer_output(&mut self, slot: Slot) {
        self.offer_output = Some(slot);
    }

    /// Allow this chain to take a base plane deposited by a mask's gate, instead of uploading
    /// its own input.
    ///
    /// ⚠ Called by `accelerate::walk` and by nothing else — see [`Self::take_shared`].
    pub fn allow_shared_input(&mut self, slot: Slot) {
        self.take_shared = Some(slot);
    }

    /// Whether `mark_shared` let this chain take a deposited plane.
    ///
    /// ⚠⚠ Exists because after M8.3n **every mask sub-stage is ported**, so a chain behind a CPU
    /// stage is no longer constructible from any `Edits` — the same shape M8.3e hit when
    /// `output_transform` made "a tail with nothing replaceable" unbuildable, and M8.3h when a
    /// trailing run stopped being refusable. The rule is therefore observed directly rather than
    /// through a render, which is **stronger** than what it replaces: it needs no deposit, no GPU
    /// run and no inference from a pixel.
    pub fn shares_input(&self) -> bool {
        self.take_shared.is_some()
    }

    /// Which slot this chain would take from, if any.
    ///
    /// ⚠ Separate from [`Self::shares_input`] so a test can pin **which** slot without a render —
    /// cross-wiring the two is the sabotage the slot guards are written against, and it changes no
    /// pixel in a fixture where only one slot is ever occupied.
    pub fn takes_from(&self) -> Option<Slot> {
        self.take_shared
    }

    /// Which slot this chain deposits into, if any.
    pub fn offers_to(&self) -> Option<Slot> {
        self.offer_output
    }

    /// Transfers **this chain** has performed, as `(uploads, downloads)`.
    pub fn transfer_counts(&self) -> (usize, usize) {
        (
            self.uploads.load(Ordering::Relaxed),
            self.downloads.load(Ordering::Relaxed),
        )
    }

    /// Turn per-stage GPU timing on or off for this chain. See `timing.rs`.
    pub fn set_timing(&mut self, on: bool) {
        self.timing = on;
    }

    /// Fuse runs of per-pixel twins, or encode each on its own.
    pub fn set_fusion(&mut self, on: bool) {
        self.fusion = on;
    }

    /// `(fused runs, single stages)` this chain has encoded.
    pub fn encode_units(&self) -> (usize, usize) {
        (
            self.fused_units.load(Ordering::Relaxed),
            self.single_units.load(Ordering::Relaxed),
        )
    }

    fn count_unit(&self, unit: &crate::per_pixel::Unit<'_>) {
        let counter = match unit {
            crate::per_pixel::Unit::Fused(_) => &self.fused_units,
            crate::per_pixel::Unit::Single(_) => &self.single_units,
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    /// What the last timed run measured, or `None` if no run was timed.
    pub fn last_timings(&self) -> Option<crate::timing::Timings> {
        self.last_timings.lock().ok()?.clone()
    }

    /// The chain's stages, joined — the label every debug line uses.
    fn label(&self) -> String {
        self.stages
            .iter()
            .map(|s| s.name())
            .collect::<Vec<_>>()
            .join("+")
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
    fn cpu_fallback(&self, image: ImageBuf) -> ImageBuf {
        // ⚠ Every stage, not some — a partially-GPU result would be a third rendering path
        // nobody designed. Through the fused runner, like every other CPU list.
        pipeline::run_stages(self.stages.iter().map(|s| s.cpu()), image)
    }

    /// The name of the stage this chain ends with, if any.
    ///
    /// ⚠ [`crate::accelerate`] asks so it can decide whether the tail's output is
    /// display-referred. Asked rather than inferred from "`output_transform` is always last",
    /// which is true of today's pipeline and is an assumption rather than a guarantee.
    /// Run the chain on an image that is ALREADY on the GPU, downloading the result.
    ///
    /// ⚠⚠ **The resident input (2026-09-26).** A drag of a prefix step resumes from the same
    /// checkpoint on every frame, so re-uploading it each frame is pure waste: 2.3 ms of memcpy
    /// plus the staging copy at 6.8 Mpx. The caller keeps the checkpoint on the GPU and hands it
    /// here. `None` on any GPU failure — the caller then runs the CPU stage, as every seam does.
    pub fn apply_from(&self, resident: &GpuImage) -> Option<ImageBuf> {
        let (w, h) = self.output_dims(resident.width(), resident.height());
        crate::guarded(|| self.run_with(Input::Resident(resident), Download::Float, Keep::Nothing))
            .map(|(data, _)| ImageBuf::from_vec(w, h, bytemuck::cast_slice(&data).to_vec()))
    }

    /// Run the chain and hand its OUTPUT back as a GPU image, downloaded too or not at all.
    ///
    /// ⚠⚠ **The resident final (2026-09-26).** The last prefix step's output is the tail's input;
    /// kept on the GPU under its key, every later tail frame copies it GPU-side. With `download`
    /// the float image comes back as well, for the CPU checkpoint cache; without it nothing waits
    /// for the GPU — the handoff — and the image is read later by [`GpuImage::download`], ordered
    /// after this submission. `None` on any GPU failure; the caller runs the step the ordinary way.
    pub fn run_keeping_output(
        &self,
        input: Input<'_>,
        download: bool,
    ) -> Option<(Option<ImageBuf>, GpuImage)> {
        let (iw, ih) = match input {
            Input::Cpu(i) => (i.width(), i.height()),
            Input::Resident(r) => (r.width(), r.height()),
        };
        let (w, h) = self.output_dims(iw, ih);
        let mode = if download {
            Download::Float
        } else {
            Download::None
        };
        let (data, kept) = crate::guarded(|| self.run_with(input, mode, Keep::Output))?;
        let out = download.then(|| ImageBuf::from_vec(w, h, bytemuck::cast_slice(&data).to_vec()));
        Some((out, kept?))
    }

    /// Run the chain on an image already on the GPU, quantised to 8-bit: the tail on a hit.
    pub fn run_rgba8_from(&self, resident: &GpuImage) -> Option<Vec<u8>> {
        if self.stages.is_empty() {
            return None;
        }
        crate::guarded(|| self.run_with(Input::Resident(resident), Download::Rgba8, Keep::Nothing))
            .map(|(d, _)| d)
    }

    /// [`Self::run_rgba8`], also handing back the PRISTINE input as a GPU image: the tail on a
    /// miss, which keeps what it uploaded for the next frame.
    pub fn run_rgba8_keeping_input(&self, image: &ImageBuf) -> Option<(Vec<u8>, GpuImage)> {
        if self.stages.is_empty() {
            return None;
        }
        let (d, kept) =
            crate::guarded(|| self.run_with(Input::Cpu(image), Download::Rgba8, Keep::Input))?;
        Some((d, kept?))
    }

    /// [`PipelineStage::apply`] with the pristine input handed back too: the float tail on a
    /// miss. Falls back exactly as `apply` does, and then keeps nothing.
    pub fn apply_keeping_input(&self, image: ImageBuf) -> (ImageBuf, Option<GpuImage>) {
        if self.stages.is_empty() {
            return (image, None);
        }
        let (w, h) = self.output_dims(image.width(), image.height());
        match crate::guarded(|| self.run_with(Input::Cpu(&image), Download::Float, Keep::Input)) {
            Some((data, kept)) => (
                ImageBuf::from_vec(w, h, bytemuck::cast_slice(&data).to_vec()),
                kept,
            ),
            None => (self.cpu_fallback(image), None),
        }
    }

    /// The chain's output size for an input of `w` x `h`: every stage's `output_dims`, folded.
    pub fn output_dims(&self, w: usize, h: usize) -> (usize, usize) {
        self.stages
            .iter()
            .fold((w, h), |(w, h), s| s.output_dims(w, h))
    }

    pub fn last_name(&self) -> Option<&'static str> {
        self.stages.last().map(|s| s.name())
    }

    /// Run the chain and download **RGBA8** rather than f32.
    ///
    /// ⚠⚠ Sound **only** when this chain's output is display-referred `[0, 1]`, which is what
    /// [`crate::accelerate`] establishes before handing the chain over: it is the tail's last
    /// stage, and its own last stage is `output_transform`. A caller reaching past that would
    /// quantize scene-referred values, which do not fit in a byte.
    ///
    /// ⚠ `None` on any GPU failure, like the float path. The caller falls back through
    /// [`PipelineStage::apply`], which retries once and then takes the CPU path — one wasted
    /// attempt on a path that is rare, rather than a second fallback implementation here.
    pub fn run_rgba8(&self, image: &ImageBuf) -> Option<Vec<u8>> {
        if self.stages.is_empty() {
            return None;
        }
        // ⚠ Guarded for the same reason `apply` is — this one's `None` reaches a caller that
        // retries through `apply`, so a panic here would otherwise kill the worker just the same.
        crate::guarded(|| self.run_with(Input::Cpu(image), Download::Rgba8, Keep::Nothing))
            .map(|(d, _)| d)
    }

    /// The GPU path, or `None` on any failure at all. Returns the download (empty in
    /// [`Download::None`]) and whatever `keep` asked for.
    fn run_with(
        &self,
        input: Input<'_>,
        mode: Download,
        keep: Keep,
    ) -> Option<(Vec<u8>, Option<GpuImage>)> {
        let (iw, ih) = match input {
            Input::Cpu(image) => (image.width(), image.height()),
            Input::Resident(r) => (r.width(), r.height()),
        };
        let ctx = &self.ctx;
        let device = &ctx.device;
        // ⚠ One recorder per timed run, sized from this chain: one pair per stage and one for
        // the quantise. `None` keeps the untimed path byte for byte.
        let mut rec = if !self.timing {
            None
        } else if ctx.timestamps() {
            Some(crate::timing::Recorder::new(
                ctx,
                self.stages.len() as u32 + 1,
            ))
        } else {
            crate::timing::note_unsupported();
            None
        };
        // ⚠⚠ **The shared upload (M8.3l).** When this chain is a mask's first sub-stage, the
        // plane it is about to upload is the one that mask's auto-mask gate uploaded moments ago —
        // the same bytes, in the same `w*h*3` f32 layout, already resident. Taking it deletes a
        // host memcpy and a device copy of the whole frame: ~2.1 ms and ~6-10 ms respectively at
        // rung 3200. `take` refuses anything that is not this thread's deposit at these exact
        // dimensions, and a refusal costs only the lock.
        let shared = self
            .take_shared
            .and_then(|slot| ctx.plane(slot).take(iw, ih));
        let shared_taken = shared.is_some();
        let mut img = match shared {
            Some(img) => img,
            None => GpuImage::new(ctx, iw, ih)?,
        };
        let bytes = img.bytes();
        // ⚠⚠ The output size, known before anything is encoded. Everything after the pass — the
        // quantise buffer, the readback, the deposit — is sized from THIS, never from `image`.
        let (ow, oh) = self.output_dims(iw, ih);

        // ── upload, once ──────────────────────────────────────────────────────────────
        // ⚠ A `MAP_WRITE` staging buffer rather than `queue.write_buffer`: measured 1.86 ms
        // against 3.90 for 19.6 MB, a 2.1x difference. The memcpy into mapped memory is the
        // floor (0.8-0.9 ms) and wgpu exposes no way around it.
        // ⚠⚠ **Cached, and this is the largest single saving in M8.3f.** Measured at 3200x2136:
        // creating this buffer fresh costs **7.2 ms per render**, while the 82 MB image buffer
        // costs 0.05 ms and the 82 MB readback buffer 0.02 — same size, same device. It is
        // `mapped_at_creation: true` that is expensive, not the allocation, and reuse replaces it
        // with a `map_async` measured at **0.00-0.01 ms**.
        //
        // ⚠ `slice(..bytes)` rather than `slice(..)`: the cached buffer may be **larger** than
        // this render needs, and the copy below is likewise given `bytes`. That is what makes a
        // grown buffer serving a smaller render correct by construction.
        //
        // ⚠⚠ **Skipped entirely when the plane was shared** — there is nothing to copy and no
        // staging buffer to ask the pool for. The counter moves with it: a chain that shared its
        // input has genuinely uploaded nothing, and `transfer_counts` must say so or the claim
        // this milestone makes is unobservable.
        // ⚠⚠ **In situ, because the decomposition below prices a SYNTHETIC split.** That harness
        // gives each phase its own submit, which is not what ships: here the device-side upload
        // copy, every dispatch and the download copy share one submit, so only the host halves and
        // the fused GPU half are separable without changing what is measured. The backlog entry
        // for the prefix->tail seam says in as many words that isolating the download "needs a
        // `gpu-pipeline` change" — this is that change, and it is a print, not a path.
        let t_memcpy = std::time::Instant::now();
        let mut enc = device.create_command_encoder(&Default::default());
        // ⚠ Not marked: an encoder timestamp around this copy hangs wgpu 29's Metal backend — see
        // `timing.rs`'s module doc. The copy is inside the block's `copies+latency` instead.
        let staging = match (shared_taken, input) {
            (true, _) => None,
            // ⚠⚠ A GPU-resident input is COPIED, never used in place: stages write into `img`, and
            // the resident image must survive for the next frame of the drag. A GPU-side copy
            // costs a fraction of the staging upload it replaces, and counts as no upload.
            (false, Input::Resident(r)) => {
                enc.copy_buffer_to_buffer(&r.buffer, 0, &img.buffer, 0, bytes);
                None
            }
            (false, Input::Cpu(image)) => {
                let staging = ctx.cache.take_staging(device, bytes)?;
                staging
                    .slice(..bytes)
                    .get_mapped_range_mut()
                    .copy_from_slice(bytemuck::cast_slice(image.data()));
                staging.unmap();
                enc.copy_buffer_to_buffer(&staging, 0, &img.buffer, 0, bytes);
                self.uploads.fetch_add(1, Ordering::Relaxed);
                Some(staging)
            }
        };
        // ⚠⚠ **The kept input is copied BEFORE any stage is encoded**, in the same encoder, so it
        // is the pristine upload — what the tail's next frame will run from. A GPU-side copy of
        // 82 MB costs ~1 ms against the 8-10 ms upload it saves on every frame after this one.
        let kept_input = match keep {
            Keep::Input => {
                let k = GpuImage::new(ctx, iw, ih)?;
                enc.copy_buffer_to_buffer(&img.buffer, 0, &k.buffer, 0, bytes);
                Some(k)
            }
            Keep::Nothing | Keep::Output => None,
        };

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
        let scratch = GpuImage::scratch(ctx, iw, ih, self.scratch_needed());
        // ⚠ The packed target is allocated only in bytes mode. Measured, an 82 MB buffer costs
        // 0.05 ms to create and a 27 MB one proportionally less, so this is not worth caching —
        // unlike the staging buffer, whose cost was `mapped_at_creation` rather than allocation.
        let packed = match mode {
            Download::Float | Download::None => None,
            Download::Rgba8 => Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("chain-packed"),
                size: (ow * oh * 4) as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            })),
        };
        match rec.as_mut() {
            None => {
                let mut pass = enc.begin_compute_pass(&Default::default());
                let mut i = 0;
                while i < self.stages.len() {
                    // ⚠ A run of per-pixel twins is ONE unit and one dispatch — `per_pixel.rs`.
                    let (unit, next) = crate::per_pixel::next_unit(
                        &self.stages,
                        i,
                        img.width(),
                        img.height(),
                        self.fusion,
                    );
                    self.count_unit(&unit);
                    unit.encode(ctx, &mut pass, &img, &scratch);
                    // ⚠ Relabelled BETWEEN units, so a unit after a resize dispatches over the
                    // new size. wgpu 29's pass holds no borrow of `img`, which is what makes this
                    // legal.
                    let (w, h) = unit.output_dims(img.width(), img.height());
                    img.relabel(w, h);
                    i = next;
                }
                // ⚠ Inside the **same** pass, so it sees every stage's writes under the in-pass
                // barrier this chain already depends on and measures.
                if let Some(p) = &packed {
                    crate::quantize::encode(ctx, &mut pass, &img, p);
                }
            }
            Some(r) => {
                // ⚠⚠ ONE PASS PER UNIT, timing mode only. Apple silicon writes a timestamp at a
                // pass boundary and nowhere inside one, so this is the only way to a per-unit
                // figure. M8.3b's sabotage established that the split changes no pixel; its cost
                // is what `Timings::print` shows against the untimed wall. A fused run is one unit
                // and reports as one pass under its members' joined names.
                let mut i = 0;
                while i < self.stages.len() {
                    let (unit, next) = crate::per_pixel::next_unit(
                        &self.stages,
                        i,
                        img.width(),
                        img.height(),
                        self.fusion,
                    );
                    self.count_unit(&unit);
                    let label = unit.label();
                    let p = r.pair(&label);
                    let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some(&label),
                        timestamp_writes: Some(r.pass_writes(p)),
                    });
                    unit.encode(ctx, &mut pass, &img, &scratch);
                    drop(pass);
                    let (w, h) = unit.output_dims(img.width(), img.height());
                    img.relabel(w, h);
                    i = next;
                }
                if let Some(pk) = &packed {
                    let p = r.pair("quantise");
                    let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                        label: Some("quantise"),
                        timestamp_writes: Some(r.pass_writes(p)),
                    });
                    crate::quantize::encode(ctx, &mut pass, &img, pk);
                }
            }
        }

        // ── download, once ────────────────────────────────────────────────────────────
        // ⚠⚠ In bytes mode this moves **27 MB instead of 82** at rung 3200. Measured: f32 costs
        // 6.33 ms of copy+map plus a 6.87 ms `to_vec`, against 2.59 + 2.19 for RGBA8 — so **8.4 ms**,
        // which is a **2.8x** saving rather than the 4x the size ratio suggests, because a download
        // carries a **1.28 ms fixed cost** that no size reduction touches.
        let d_memcpy = t_memcpy.elapsed();
        // ⚠⚠ **No readback: submit, deposit, return.** The next chain on this queue is ordered
        // after this submission, so it reads the finished pixels without this thread waiting.
        // That wait and the 82 MB copy out are what a handoff exists to skip.
        if mode == Download::None {
            // ⚠ No readback: submit, and either hand the output to the caller or deposit it for
            // the next chain. One or the other — a buffer cannot be in two places — and a run
            // that would do neither has nowhere to put its pixels.
            if keep != Keep::Output && self.offer_output.is_none() {
                return None;
            }
            if let Some(r) = rec.as_ref() {
                r.resolve(&mut enc);
            }
            let t_submit = std::time::Instant::now();
            ctx.queue.submit(Some(enc.finish()));
            let kept = if keep == Keep::Output {
                Some(img)
            } else {
                if let Some(slot) = self.offer_output {
                    ctx.plane(slot).deposit(ow, oh, img);
                }
                kept_input
            };
            // ⚠⚠ **Dropped, NOT returned to the staging cache.** Nothing waited for the GPU, so the
            // copy out of this buffer may still be in flight; handing it back would let the next
            // upload overwrite it mid-copy — a plausible wrong frame. wgpu keeps a dropped buffer
            // alive until its submission completes, which is exactly the lifetime needed.
            drop(staging);
            // ⚠ Timed, a handoff chain waits — the one thing it otherwise never does. The block
            // says so, and the untimed `RUSTROOM_DEBUG_XFER` line is the control.
            if let Some(r) = rec.take() {
                if let Some(t) = r.finish(ctx, t_submit, true) {
                    t.print(&self.label(), iw, ih);
                    if let Ok(mut g) = self.last_timings.lock() {
                        *g = Some(t);
                    }
                }
            }
            if std::env::var("RUSTROOM_DEBUG_XFER").is_ok() {
                eprintln!(
                    "CHAIN {:>26} {}x{} -> plane {}x{} took={} memcpy={:.2} (no download)",
                    self.label(),
                    iw,
                    ih,
                    ow,
                    oh,
                    shared_taken,
                    d_memcpy.as_secs_f64() * 1e3,
                );
            }
            return Some((Vec::new(), kept));
        }
        let t_submit = std::time::Instant::now();
        let (src, out_bytes) = match &packed {
            Some(p) => (p, (ow * oh * 4) as u64),
            None => (&img.buffer, img.bytes()),
        };
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("chain-readback"),
            size: out_bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        // ⚠ The readback copy is not marked either (`timing.rs`); the resolve goes in behind it so
        // the ticks travel in the same submission.
        enc.copy_buffer_to_buffer(src, 0, &readback, 0, out_bytes);
        if let Some(r) = rec.as_ref() {
            r.resolve(&mut enc);
        }
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
        let d_submit = t_submit.elapsed();
        // ⚠ After the chain's own wait, so `finish`'s wait returns at once and the wall it records
        // is this chain's own submit+wait.
        if let Some(r) = rec.take() {
            if let Some(t) = r.finish(ctx, t_submit, false) {
                t.print(&self.label(), iw, ih);
                if let Ok(mut g) = self.last_timings.lock() {
                    *g = Some(t);
                }
            }
        }
        let t_read = std::time::Instant::now();
        // ⚠⚠ **Deposited HERE and nowhere earlier**, after `poll(Wait)` has confirmed the
        // submission finished — the GPU writes this buffer during the submit, so offering it any
        // sooner would hand a consumer pixels that are still being written. The same rule that
        // governs when the staging buffer goes back to the pool and when `GpuBrushRaster` deposits.
        //
        // ⚠ The consumer is a gating brush's gate in the CPU `MaskStage` that follows this chain:
        // it needs exactly these bytes as its base and would otherwise stage a second copy —
        // measured at 2.04-2.23 ms of host memcpy for a 78 MB frame.
        //
        // ⚠ A deposit nobody takes is cleared by the enclosing mask's `end_mask`, which stops it
        // outliving its frame.
        //
        // ⚠ Or KEPT: an output the caller asked for goes to the caller, never to the slot as well.
        let kept = if keep == Keep::Output {
            Some(img)
        } else {
            if let Some(slot) = self.offer_output {
                ctx.plane(slot).deposit(ow, oh, img);
            }
            kept_input
        };
        let out: Vec<u8> = slice.get_mapped_range().to_vec();
        readback.unmap();
        self.downloads.fetch_add(1, Ordering::Relaxed);
        if std::env::var("RUSTROOM_DEBUG_XFER").is_ok() {
            eprintln!(
                "CHAIN {:>26} {}x{} in={:.1}MB out={:.1}MB took={} memcpy={:.2} submit={:.2} \
                 tovec={:.2}",
                self.label(),
                iw,
                ih,
                bytes as f64 / 1.0e6,
                out_bytes as f64 / 1.0e6,
                shared_taken,
                d_memcpy.as_secs_f64() * 1e3,
                d_submit.as_secs_f64() * 1e3,
                t_read.elapsed().as_secs_f64() * 1e3,
            );
        }
        // ⚠ Returned only **here**, after `poll(Wait)` has confirmed the submission finished —
        // the GPU reads this buffer during the submit, so handing it back any earlier would let
        // the next render map a buffer still being read. On the `?` paths above it is simply
        // dropped and the next render allocates; that is rare and costs one allocation.
        // ⚠ Only when this render actually took one: a chain that shared its input never asked the
        // pool and must not hand it a buffer it does not own.
        if let Some(st) = staging {
            ctx.cache.put_staging(st);
        }
        Some((out, kept))
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
        let (w, h) = self.output_dims(image.width(), image.height());
        match crate::guarded(|| self.run_with(Input::Cpu(&image), Download::Float, Keep::Nothing)) {
            Some((data, _)) => ImageBuf::from_vec(w, h, bytemuck::cast_slice(&data).to_vec()),
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

    /// ⚠⚠ **THE test of this milestone.** The GPU's bytes must be what the CPU would have
    /// quantized — not close to it, *identical* to it, because both sides are a defined rounding
    /// of the same number and a tolerance would hide exactly the tie bug below.
    ///
    /// ⚠⚠ **The fixture is the test.** WGSL's `round` breaks ties to the EVEN whole number while
    /// Rust's `f32::round` breaks them AWAY FROM ZERO, and `v * 255` lands on exact halves
    /// routinely — so a shader using `round` is correct on every ordinary value and off by one
    /// level on every tie. A ramp that never lands on a half cannot see that. This fixture carries
    /// every tie `(2k+1)/510`, plus a negative, a value above 1, and a NaN.
    #[test]
    fn the_gpu_bytes_are_what_the_cpu_would_have_quantized() {
        let Some(ctx) = ctx() else { return };
        let mut px: Vec<f32> = Vec::new();
        for k in 0..255u32 {
            // (2k+1)/510 * 255 == k + 0.5 exactly
            let half = (2 * k + 1) as f32 / 510.0;
            px.extend_from_slice(&[half, half, half]);
        }
        px.extend_from_slice(&[-0.25, 1.5, f32::NAN]);
        px.extend_from_slice(&[0.0, 1.0, 0.5]);
        let w = px.len() / 3;
        let img = ImageBuf::from_vec(w, 1, px);
        // ⚠ An identity chain: exposure at 0 EV multiplies by exactly 1.0, so the only thing under
        // test is the quantization. A chain that *changed* the pixels would still pass, but a
        // failure could then be the stage rather than the rounding.
        let chain = GpuChain::new(
            ctx.clone(),
            vec![Box::new(GpuExposureStage::new(0.0, ctx.clone()))],
        );
        let want = chain.apply(img.clone()).to_rgba8();
        let got = chain
            .run_rgba8(&img)
            .expect("the GPU path must be available here");
        assert_eq!(got.len(), want.len(), "byte count");
        let bad: Vec<(usize, u8, u8)> = got
            .iter()
            .zip(&want)
            .enumerate()
            .filter(|(_, (g, w))| g != w)
            .map(|(i, (g, w))| (i, *g, *w))
            .take(6)
            .collect();
        assert!(
            bad.is_empty(),
            "GPU vs CPU quantize, first few (index, gpu, cpu): {bad:?}"
        );
    }

    /// ⚠ The chain must know what it ends in — `accelerate` asks, to decide whether the tail's
    /// output is display-referred and therefore safe to quantize on the GPU.
    #[test]
    fn a_chain_reports_the_name_of_its_last_stage() {
        let Some(ctx) = ctx() else { return };
        let chain = GpuChain::new(
            ctx.clone(),
            vec![
                Box::new(GpuExposureStage::new(0.0, ctx.clone())),
                Box::new(crate::GpuOutputTransformStage::new(
                    color::OutputSpace::Srgb,
                    color::RenderingIntent::default(),
                    &ctx,
                )),
            ],
        );
        assert_eq!(chain.last_name(), Some("output_transform"));
        assert_eq!(GpuChain::new(ctx.clone(), vec![]).last_name(), None);
    }

    /// TEMPORARY probe: what is inside the round trip, and what would a 4x smaller readback buy?
    ///
    /// `cargo test --release -p gpu-pipeline -- --ignored --nocapture probe_breakdown`
    #[test]
    #[ignore]
    fn probe_breakdown() {
        use std::time::Instant;
        let Some(ctx) = ctx() else { return };
        let (w, h) = (3200usize, 2136);
        let img_cpu = ramp(w, h);
        let edits = edit_model::Edits {
            exposure: 0.4,
            highlights: -30.0,
            shadows: 25.0,
            ..Default::default()
        };
        let stages: Vec<Box<dyn GpuStage>> = vec![
            Box::new(GpuExposureStage::new(0.4, ctx.clone())),
            Box::new(crate::GpuToneRegionsStage::new(&edits, &ctx)),
            Box::new(crate::GpuDisplayRenderingStage::new(None, None, &ctx)),
            Box::new(crate::GpuOutputTransformStage::new(
                color::OutputSpace::Srgb,
                color::RenderingIntent::default(),
                &ctx,
            )),
        ];
        // warm: compile every shader and seed the staging pool
        let warm = GpuChain::new(
            ctx.clone(),
            vec![Box::new(GpuExposureStage::new(0.0, ctx.clone()))],
        );
        std::hint::black_box(warm.apply(img_cpu.clone()));

        let device = &ctx.device;
        let mut b = [f64::MAX; 12];
        for _ in 0..5 {
            let gi = GpuImage::new(&ctx, w, h).unwrap();
            let bytes = gi.bytes();

            // 1. CPU memcpy into the mapped staging buffer
            let t = Instant::now();
            let staging = ctx.cache.take_staging(device, bytes).unwrap();
            staging
                .slice(..bytes)
                .get_mapped_range_mut()
                .copy_from_slice(bytemuck::cast_slice(img_cpu.data()));
            staging.unmap();
            b[0] = b[0].min(t.elapsed().as_secs_f64() * 1000.0);

            // 2. device-side upload copy, on its own submit
            let t = Instant::now();
            let mut e = device.create_command_encoder(&Default::default());
            e.copy_buffer_to_buffer(&staging, 0, &gi.buffer, 0, bytes);
            ctx.queue.submit(Some(e.finish()));
            let _ = device.poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            });
            b[1] = b[1].min(t.elapsed().as_secs_f64() * 1000.0);

            // 3. the four dispatches
            let t = Instant::now();
            let mut e = device.create_command_encoder(&Default::default());
            {
                let mut pass = e.begin_compute_pass(&Default::default());
                for s in &stages {
                    s.encode(&ctx, &mut pass, &gi, &[]);
                }
            }
            ctx.queue.submit(Some(e.finish()));
            let _ = device.poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            });
            b[2] = b[2].min(t.elapsed().as_secs_f64() * 1000.0);

            // 4. device-side download copy + map, f32 (what ships today)
            let rb = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: bytes,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let t = Instant::now();
            let mut e = device.create_command_encoder(&Default::default());
            e.copy_buffer_to_buffer(&gi.buffer, 0, &rb, 0, bytes);
            ctx.queue.submit(Some(e.finish()));
            let sl = rb.slice(..);
            let (tx, rx) = std::sync::mpsc::channel();
            sl.map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
            let _ = device.poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            });
            rx.recv().unwrap().unwrap();
            b[3] = b[3].min(t.elapsed().as_secs_f64() * 1000.0);

            // 5. the to_vec
            let t = Instant::now();
            let v: Vec<f32> = bytemuck::cast_slice(&sl.get_mapped_range()).to_vec();
            b[4] = b[4].min(t.elapsed().as_secs_f64() * 1000.0);
            std::hint::black_box(&v);
            rb.unmap();

            // 6. the same download at a QUARTER the size -- what RGBA8 would move.
            //    The bytes are meaningless; only the transfer is being measured.
            for (i, frac) in [(6usize, 3u64), (7, 12), (8, 82_000)].into_iter() {
                let n = (bytes / frac).max(256) & !255;
                let t = Instant::now();
                let r = device.create_buffer(&wgpu::BufferDescriptor {
                    label: None,
                    size: n,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                });
                let mut e = device.create_command_encoder(&Default::default());
                e.copy_buffer_to_buffer(&gi.buffer, 0, &r, 0, n);
                ctx.queue.submit(Some(e.finish()));
                let sl = r.slice(..);
                let (tx, rx) = std::sync::mpsc::channel();
                sl.map_async(wgpu::MapMode::Read, move |x| {
                    let _ = tx.send(x);
                });
                let _ = device.poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                });
                rx.recv().unwrap().unwrap();
                b[i] = b[i].min(t.elapsed().as_secs_f64() * 1000.0);
                let t = Instant::now();
                let v: Vec<u8> = sl.get_mapped_range().to_vec();
                b[i + 3] = b[i + 3].min(t.elapsed().as_secs_f64() * 1000.0);
                std::hint::black_box(&v);
                r.unmap();
            }
            let quarter = bytes / 3;
            let rb8 = device.create_buffer(&wgpu::BufferDescriptor {
                label: None,
                size: quarter,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let t = Instant::now();
            let mut e = device.create_command_encoder(&Default::default());
            e.copy_buffer_to_buffer(&gi.buffer, 0, &rb8, 0, quarter);
            ctx.queue.submit(Some(e.finish()));
            let sl8 = rb8.slice(..);
            let (tx, rx) = std::sync::mpsc::channel();
            sl8.map_async(wgpu::MapMode::Read, move |r| {
                let _ = tx.send(r);
            });
            let _ = device.poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            });
            rx.recv().unwrap().unwrap();
            let v8: Vec<u8> = sl8.get_mapped_range().to_vec();
            b[5] = b[5].min(t.elapsed().as_secs_f64() * 1000.0);
            std::hint::black_box(&v8);
            rb8.unmap();

            ctx.cache.put_staging(staging);
        }
        println!(
            "\n=== round trip at {w}x{h} ({:.0} MB f32), best of 5, separate submits",
            (w * h * 12) as f64 / 1e6
        );
        println!("  1 staging memcpy (CPU)      {:6.2} ms", b[0]);
        println!("  2 upload copy   (device)    {:6.2} ms", b[1]);
        println!("  3 four dispatches           {:6.2} ms", b[2]);
        println!("  4 download f32 copy+map     {:6.2} ms", b[3]);
        println!("  5 to_vec of 82 MB (CPU)     {:6.2} ms", b[4]);
        println!("  -- download scaling, copy+map  +  to_vec:");
        for (label, c, v) in [
            ("82 MB (1/1, f32 today)", b[3], b[4]),
            ("27 MB (1/3, = RGBA8)", b[6], b[9]),
            ("6.8 MB (1/12)", b[7], b[10]),
            ("~256 B (fixed cost)", b[8], b[11]),
        ] {
            println!("     {label:24} {c:6.2}  +  {v:6.2}  =  {:6.2} ms", c + v);
        }
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
        fn cpu_cost(&self) -> f32 {
            99.0
        }

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

    #[test]
    fn a_second_run_of_the_same_chain_compiles_nothing() {
        // ⚠⚠ The carrier. `cache.rs`'s own tests prove the cache caches; this proves the
        // production path **consults** it — the W5 lesson, where a milestone's maths was fully
        // tested and nothing proved the data ever reached it.
        //
        // ⚠ Asserted as a **delta around the second run** rather than an absolute count, because
        // the cache is process-wide and `cargo test` runs in parallel: another test using this
        // same shader would hit rather than miss, so the delta is 0 either way, while an absolute
        // count would depend on who got there first.
        let Some(ctx) = ctx() else { return };
        // ⚠ The text the cache is keyed on is what the emitter produces for a run of one, at this
        // image's size — a lone per-pixel twin has no shader of its own any more.
        let stage = GpuExposureStage::new(1.0, ctx.clone());
        let (row_groups, _) = crate::dispatch::Dispatch::grid(64 * 64);
        let shader = crate::per_pixel::emit(
            &[GpuStage::per_pixel(&stage, 64, 64).expect("exposure is per-pixel")],
            64 * 64,
            row_groups,
            64,
        )
        .shader;
        let chain = GpuChain::new(ctx.clone(), vec![Box::new(stage)]);
        let img = ramp(64, 64);
        let _ = chain.apply(img.clone());
        let before = ctx.cache.misses_for(&shader);
        assert!(
            before > 0,
            "fixture check: the first run compiled nothing, so this test proves nothing"
        );
        let _ = chain.apply(img);
        assert_eq!(
            ctx.cache.misses_for(&shader),
            before,
            "the second run of an identical chain recompiled its shader"
        );
    }

    #[test]
    fn two_renders_with_different_exposures_do_not_share_a_uniform() {
        // ⚠⚠ The most likely real bug in a caching change, and no equivalence test can see it: a
        // cached **uniform** would give the second render the first render's exposure. Both runs
        // use the identical shader text, so the program cache is hit in both — which is precisely
        // the condition under which a wrongly-cached uniform would go unnoticed.
        let Some(ctx) = ctx() else { return };
        let img = ramp(64, 64);
        let up = GpuChain::new(
            ctx.clone(),
            vec![Box::new(GpuExposureStage::new(1.0, ctx.clone()))],
        )
        .apply(img.clone());
        let down = GpuChain::new(
            ctx.clone(),
            vec![Box::new(GpuExposureStage::new(-1.0, ctx.clone()))],
        )
        .apply(img.clone());

        for (i, ((s, u), d)) in img
            .data()
            .iter()
            .zip(up.data())
            .zip(down.data())
            .enumerate()
        {
            assert_eq!(*u, s * 2.0, "sample {i} did not double");
            assert_eq!(
                *d,
                s * 0.5,
                "sample {i} took the previous render's exposure"
            );
        }
    }

    #[test]
    fn a_small_render_after_a_large_one_is_still_correct() {
        // ⚠⚠ The guard on the pooled staging buffer. Once a big render has allocated a large
        // buffer, a small render reuses it — and both the mapped slice and the copy out of it
        // must be given the **image's** bytes rather than the buffer's size. Getting that wrong
        // is not a slow render, it is reading 82 MB out of a 20 MB image.
        //
        // ⚠ It also asserts the GPU path actually ran: a chain that fell back to the CPU would
        // produce the same pixels and prove nothing about the buffer.
        let Some(ctx) = ctx() else { return };
        let big = ramp(256, 256);
        let _ = GpuChain::new(
            ctx.clone(),
            vec![Box::new(GpuExposureStage::new(1.0, ctx.clone()))],
        )
        .apply(big);

        let small = ramp(32, 16);
        let chain = GpuChain::new(
            ctx.clone(),
            vec![Box::new(GpuExposureStage::new(1.0, ctx.clone()))],
        );
        let out = chain.apply(small.clone());
        assert_eq!(
            chain.transfer_counts(),
            (1, 1),
            "fixture check: the chain fell back to the CPU, so this proves nothing about the buffer"
        );
        assert_eq!(out.width(), 32);
        assert_eq!(out.height(), 16);
        for (i, (s, o)) in small.data().iter().zip(out.data()).enumerate() {
            assert_eq!(*o, s * 2.0, "sample {i} is wrong after a larger render");
        }
    }

    /// A stage that keeps the top half of the image. Its output is the leading prefix of the
    /// buffer, so its GPU encode moves nothing: it tests the chain's SIZE plumbing and nothing else.
    struct TopHalf(TopHalfCpu);
    struct TopHalfCpu;
    impl PipelineStage for TopHalfCpu {
        fn name(&self) -> &'static str {
            "top_half"
        }
        fn apply(&self, image: ImageBuf) -> ImageBuf {
            let (w, h) = (image.width(), image.height());
            image.crop(0, 0, w, h / 2)
        }
        fn output_dims(&self, w: usize, h: usize) -> (usize, usize) {
            (w, h / 2)
        }
    }
    impl GpuStage for TopHalf {
        fn name(&self) -> &'static str {
            "top_half"
        }
        fn cpu_cost(&self) -> f32 {
            1000.0
        }
        fn output_dims(&self, w: usize, h: usize) -> (usize, usize) {
            (w, h / 2)
        }
        fn encode(
            &self,
            _: &GpuContext,
            _: &mut wgpu::ComputePass,
            _: &GpuImage,
            _: &[wgpu::Buffer],
        ) {
        }
        fn cpu(&self) -> &dyn PipelineStage {
            &self.0
        }
    }

    /// ⚠⚠ **A stage may change the image's size (GPU geometry, 2026-09-26)**, and everything after
    /// it must see the NEW size: a later stage's dispatch, the quantise, the readback, the deposit
    /// and the image handed back. An exposure AFTER the resize proves the relabel reaches later
    /// stages; the byte count and the plane's dimensions prove it reaches the ends.
    #[test]
    fn a_size_changing_stage_is_honoured_to_the_end_of_the_chain() {
        let Some(ctx) = ctx() else { return };
        let src = ramp(32, 20);
        let chain = || {
            GpuChain::new(
                ctx.clone(),
                vec![
                    Box::new(TopHalf(TopHalfCpu)),
                    Box::new(GpuExposureStage::new(1.0, ctx.clone())),
                ],
            )
        };
        let c = chain();
        let out = c.apply(src.clone());
        assert_eq!(
            c.transfer_counts(),
            (1, 1),
            "fixture: the GPU path must have run"
        );
        assert_eq!((out.width(), out.height()), (32, 10));
        let want = src.crop(0, 0, 32, 10);
        for (i, (w, o)) in want.data().iter().zip(out.data()).enumerate() {
            assert_eq!(*o, w * 2.0, "sample {i}");
        }
        assert_eq!(c.output_dims(32, 20), (32, 10));

        let bytes = chain().run_rgba8(&src).expect("the GPU path");
        assert_eq!(bytes.len(), 32 * 10 * 4);

        let mut offering = chain();
        offering.offer_output(Slot::Mask);
        let _ = offering.apply(src.clone());
        assert!(
            ctx.plane(Slot::Mask).take(32, 20).is_none(),
            "deposited at the INPUT size"
        );
        let _ = offering.apply(src);
        assert!(
            ctx.plane(Slot::Mask).take(32, 10).is_some(),
            "not deposited at the output size"
        );
    }

    /// A chain whose stages move pixels, timing OFF whatever the environment says — so a test
    /// run under `RUSTROOM_DEBUG_GPU_TIME` still tests the untimed path where it means to.
    fn three_stages(ctx: &Arc<GpuContext>) -> GpuChain {
        let mut c = GpuChain::new(
            ctx.clone(),
            vec![
                Box::new(GpuExposureStage::new(1.0, ctx.clone())),
                Box::new(crate::GpuContrastStage::new(25.0, ctx)),
                Box::new(GpuExposureStage::new(-0.5, ctx.clone())),
            ],
        );
        c.set_timing(false);
        // ⚠ Fusion off too, so the per-stage timing tests keep measuring one pass per stage.
        c.set_fusion(false);
        c
    }

    fn timing_ctx() -> Option<Arc<GpuContext>> {
        let ctx = ctx()?;
        if !ctx.timestamps() {
            eprintln!("SKIPPED: no timestamp queries on this GPU");
            return None;
        }
        Some(ctx)
    }

    /// One pass per stage changes no byte: the timed run IS the untimed run, measured.
    #[test]
    fn a_timed_run_produces_the_untimed_bytes() {
        let Some(ctx) = timing_ctx() else { return };
        let img = ramp(64, 48);
        let want = three_stages(&ctx).run_rgba8(&img).expect("gpu");
        assert_ne!(want, img.to_rgba8(), "the fixture must move pixels");
        let mut timed = three_stages(&ctx);
        timed.set_timing(true);
        let got = timed.run_rgba8(&img).expect("gpu");
        assert_eq!(got, want, "one pass per stage must change no byte");
    }

    /// ⚠⚠ The observable. A sabotage that keeps the single pass reports one stage entry, not
    /// three, and fails the name list.
    #[test]
    fn a_timed_run_reports_one_entry_per_stage_in_order() {
        let Some(ctx) = timing_ctx() else { return };
        let mut chain = three_stages(&ctx);
        chain.set_timing(true);
        chain.run_rgba8(&ramp(64, 48)).expect("gpu");
        let t = chain
            .last_timings()
            .expect("a timed run keeps its readings");
        let names: Vec<&str> = t.entries.iter().map(|(n, _)| n.as_str()).collect();
        // ⚠ Passes only: the copies cannot be marked on Metal (`timing.rs`).
        assert_eq!(names, ["exposure", "contrast", "exposure", "quantise"]);
        for (name, d) in &t.entries {
            let d = d.unwrap_or_else(|| panic!("{name} was marked and must have a duration"));
            assert!(
                d > 0.0 && d <= t.wall_ms,
                "{name}: {d:.3} ms against a wall of {:.3}",
                t.wall_ms
            );
        }
        assert!(
            t.gpu_span_ms > 0.0 && t.gpu_span_ms <= t.wall_ms,
            "span {}",
            t.gpu_span_ms
        );
        assert!(
            !t.waited_for_timing,
            "an ordinary chain waited on its own account"
        );
    }

    /// The float path has no quantise.
    #[test]
    fn a_timed_float_run_has_no_quantise_entry() {
        let Some(ctx) = timing_ctx() else { return };
        let mut chain = three_stages(&ctx);
        chain.set_timing(true);
        let _ = chain.apply(ramp(32, 32));
        let t = chain.last_timings().expect("timed");
        let names: Vec<&str> = t.entries.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["exposure", "contrast", "exposure"]);
    }

    /// A handoff chain normally submits and returns; timed, it waits and says so.
    #[test]
    fn a_timed_handoff_chain_waits_for_its_readings() {
        let Some(ctx) = timing_ctx() else { return };
        let mut chain = three_stages(&ctx);
        chain.set_timing(true);
        let out = chain.run_keeping_output(Input::Cpu(&ramp(32, 32)), false);
        out.expect("the handoff path must be available here");
        let t = chain.last_timings().expect("timed");
        assert!(t.waited_for_timing);
        let names: Vec<&str> = t.entries.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["exposure", "contrast", "exposure"]);
    }

    /// ⚠⚠ **The kept input is the PRISTINE upload** — copied before any stage touched it — and a
    /// later run from it produces the bytes an upload would. Asserted both ways: the kept image
    /// downloads to the input, and `run_rgba8_from` on it equals `run_rgba8` on the input.
    #[test]
    fn a_kept_input_is_the_pristine_upload_and_serves_the_next_run() {
        let Some(ctx) = ctx() else { return };
        let img = ramp(40, 30);
        let chain = three_stages(&ctx);
        let (bytes, kept) = chain.run_rgba8_keeping_input(&img).expect("gpu");
        assert_eq!(chain.transfer_counts().0, 1, "the miss uploads once");
        let plain = chain.run_rgba8(&img).expect("gpu");
        assert_eq!(bytes, plain, "keeping the input must not change a byte");
        let back = kept.download(&ctx).expect("download");
        assert_eq!(
            back.data(),
            img.data(),
            "the kept image is the input, untouched"
        );
        let again = three_stages(&ctx);
        let hit = again.run_rgba8_from(&kept).expect("gpu");
        assert_eq!(
            again.transfer_counts().0,
            0,
            "a run from the resident uploads nothing"
        );
        assert_eq!(hit, plain, "and renders the same bytes");
        // The float twin, for the proof and the other float views.
        let (out, kept) = three_stages(&ctx).apply_keeping_input(img.clone());
        let kept = kept.expect("kept on the float path too");
        assert_eq!(kept.download(&ctx).expect("download").data(), img.data());
        assert_eq!(
            out.data(),
            pipeline::PipelineStage::apply(&three_stages(&ctx), img.clone()).data()
        );
    }

    /// ⚠⚠ **The kept output is the chain's result, with and without a download.** The no-download
    /// form returns before the GPU finishes, so its correctness rests on queue order: the download
    /// submitted afterwards must read the finished pixels.
    #[test]
    fn a_kept_output_is_the_chains_result_with_or_without_a_download() {
        let Some(ctx) = ctx() else { return };
        let img = ramp(40, 30);
        let want = pipeline::PipelineStage::apply(&three_stages(&ctx), img.clone());
        let (out, kept) = three_stages(&ctx)
            .run_keeping_output(Input::Cpu(&img), true)
            .expect("gpu");
        let out = out.expect("a download was asked for");
        assert_eq!(out.data(), want.data(), "the downloaded output");
        assert_eq!(
            kept.download(&ctx).expect("download").data(),
            want.data(),
            "the kept output is the same pixels"
        );
        let (out, kept) = three_stages(&ctx)
            .run_keeping_output(Input::Cpu(&img), false)
            .expect("gpu");
        assert!(out.is_none(), "no download was asked for");
        assert_eq!(
            kept.download(&ctx).expect("download").data(),
            want.data(),
            "the handoff's output, read after the fact"
        );
        // And from a resident input: the drag's steady state.
        let resident = GpuImage::upload(&ctx, &img).expect("upload");
        let chain = three_stages(&ctx);
        let (_, kept) = chain
            .run_keeping_output(Input::Resident(&resident), false)
            .expect("gpu");
        assert_eq!(
            chain.transfer_counts().0,
            0,
            "a resident input uploads nothing"
        );
        assert_eq!(kept.download(&ctx).expect("download").data(), want.data());
        assert_eq!(
            resident.download(&ctx).expect("download").data(),
            img.data(),
            "and the resident input survives untouched"
        );
    }

    /// A kept output is handed to the caller, never ALSO deposited: one buffer, one owner.
    #[test]
    fn a_kept_output_is_not_deposited_as_well() {
        let Some(ctx) = ctx() else { return };
        ctx.prefix_plane().clear();
        let before = ctx.prefix_plane().deposits();
        let mut chain = three_stages(&ctx);
        chain.offer_output(Slot::Prefix);
        let (_, kept) = chain
            .run_keeping_output(Input::Cpu(&ramp(16, 16)), false)
            .expect("gpu");
        assert_eq!((kept.width(), kept.height()), (16, 16));
        assert_eq!(
            ctx.prefix_plane().deposits(),
            before,
            "the output went to the caller, so nothing was deposited"
        );
    }

    /// Timing off keeps nothing and costs nothing: no query set, no readings.
    #[test]
    fn an_untimed_run_keeps_no_timings() {
        let Some(ctx) = ctx() else { return };
        let chain = three_stages(&ctx);
        chain.run_rgba8(&ramp(16, 16)).expect("gpu");
        assert!(chain.last_timings().is_none());
    }

    /// Eight per-pixel twins are ONE fused unit, and with fusion off they are eight — through
    /// the same emitter, so the two agree within the twins' bound.
    #[test]
    fn a_run_of_per_pixel_twins_is_one_fused_unit() {
        let Some(ctx) = ctx() else { return };
        let stages = |ctx: &Arc<GpuContext>| -> Vec<Box<dyn GpuStage>> {
            vec![
                Box::new(crate::GpuWhiteBalanceStage::new([1.05, 0.98, 1.02], ctx)),
                Box::new(GpuExposureStage::new(0.4, ctx.clone())),
                Box::new(crate::GpuContrastStage::new(15.0, ctx)),
                Box::new(crate::GpuToneRegionsStage::new(
                    &edit_model::Edits {
                        highlights: -20.0,
                        shadows: 25.0,
                        ..Default::default()
                    },
                    ctx,
                )),
                Box::new(crate::GpuToneCurveStage::new(
                    &[[0.0, 0.0], [0.4, 0.5], [1.0, 1.0]],
                    ctx,
                )),
                Box::new(crate::GpuSaturationStage::new(20.0, 10.0, ctx)),
                Box::new(crate::GpuDisplayRenderingStage::new(None, None, ctx)),
                Box::new(crate::GpuOutputTransformStage::new(
                    color::OutputSpace::Srgb,
                    color::RenderingIntent::default(),
                    ctx,
                )),
            ]
        };
        let img = ramp(64, 48);
        let mut fused = GpuChain::new(ctx.clone(), stages(&ctx));
        fused.set_fusion(true);
        let a = fused.apply(img.clone());
        assert_eq!(fused.encode_units(), (1, 0), "one fused run, no single");
        let mut apart = GpuChain::new(ctx.clone(), stages(&ctx));
        apart.set_fusion(false);
        let b = apart.apply(img.clone());
        assert_eq!(apart.encode_units(), (8, 0), "eight runs of one");
        let d = crate::equivalence::compare(&a, &b);
        assert!(
            d.worst_levels <= 0.25,
            "fused and unfused differ by {} levels",
            d.worst_levels
        );
        assert_ne!(
            a.data(),
            img.data(),
            "fixture check: the chain moves pixels"
        );
    }

    /// A non-per-pixel stage splits the run: sharpening between two per-pixel twins gives three
    /// units, two fused and one single.
    #[test]
    fn a_spatial_stage_ends_a_fused_run() {
        let Some(ctx) = ctx() else { return };
        let mut chain = GpuChain::new(
            ctx.clone(),
            vec![
                Box::new(GpuExposureStage::new(0.3, ctx.clone())),
                Box::new(crate::GpuSharpenStage::new(
                    &edit_model::Sharpening {
                        amount: 40.0,
                        radius: 1.0,
                        detail: 50.0,
                        masking: 0.0,
                    },
                    pipeline::RenderScale::FULL,
                    &ctx,
                )),
                Box::new(crate::GpuContrastStage::new(10.0, &ctx)),
            ],
        );
        chain.set_fusion(true);
        let _ = chain.apply(ramp(32, 32));
        assert_eq!(chain.encode_units(), (2, 1));
    }

    /// A timed fused run reports its members as one pass, labelled with all of them.
    #[test]
    fn a_timed_fused_run_is_one_pass() {
        let Some(ctx) = timing_ctx() else { return };
        let mut chain = three_stages(&ctx);
        chain.set_fusion(true);
        chain.set_timing(true);
        chain.run_rgba8(&ramp(32, 32)).expect("gpu");
        let t = chain.last_timings().expect("timed");
        let names: Vec<&str> = t.entries.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["exposure+contrast+exposure", "quantise"]);
    }
}
