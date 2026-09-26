# Resident Final Checkpoint Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The tail's input stays on the GPU under the key that says what it is, so a tail-slider frame copies it GPU-side instead of uploading 82 MB, and a settle after a handoff downloads it instead of re-rendering.

**Architecture:** `PrefixCache` gains one keyed GPU image, `resident_final: Option<(TailKey, GpuImage)>` with `TailKey = (CacheKey, Option<Rect>)`. `GpuChain::run_with` gains a `Keep` argument (nothing / the pristine input / the output) and hands the kept image back beside the download; four public entry points wrap it. `render_cached_with` fills the slot three ways (a tail miss keeps its upload, the last GPU prefix step keeps its output with or without a download) and runs the tail from it on a hit. The prefix→tail plane, `TailFacts::prefix_offered`, `run_rgba8_from_plane`, `run_to_plane[_from]`, the `handed`/`from_plane` branch and `LOSE_THE_PLANE` go; `Slot::Prefix` stays for step→step. The worker's settle downloads the resident into the CPU cache and skips the tail.

**Tech Stack:** Rust, wgpu 29. Spec: `docs/superpowers/specs/2026-09-26-resident-final-checkpoint-design.md`. Measured basis: the upload half of a tail frame is ~8–10 ms of ~34 at rung 3200 (`docs/PIPELINE-REVIEW-2026-09-26.md` F2); the warm-behind after a handoff is 92–108 ms.

**Conventions that apply:** no `Co-Authored-By` trailer; never push; goldens must not move (`cargo test --workspace golden`); GPU iteration with `cargo test --release -p gpu-pipeline <one filter>` and `cargo test --release -p app <one filter>` (⚠ one filter per invocation); the full debug gate once at the end; every GPU test skips with `SKIPPED:` when `shared()` is `None`; `chain.rs` is at 720 code lines and the report says so.

**One deviation from the spec, stated up front:** the handoff's *last-resort* CPU re-run of the final step stays as three lines behind a failed download. A handoff frame has no CPU input; the fallback for a tail that cannot run from the resident is now *download the resident* (the settle's own path, which also caches it as the final), and only a download that itself fails — a dead GPU — reaches the CPU re-run. The spec's fall-back-to-CPU *path* (`from_plane`, `LOSE_THE_PLANE`, the plane clear) is gone.

---

### Task 1: `GpuImage::download` — the readback the settle and the fallback share

**Files:**
- Modify: `crates/gpu-pipeline/src/image.rs` (a method after `bytes()`, a test)

- [ ] **Step 1: Write the failing test**

Append to `mod tests` in `crates/gpu-pipeline/src/image.rs`:

```rust
    /// The download is the upload's inverse, exactly, and it reads the leading `w × h` pixels
    /// whatever the buffer's capacity — a relabelled image downloads at its relabelled size.
    #[test]
    fn a_download_is_the_uploads_inverse_at_the_relabelled_size() {
        let Some(ctx) = crate::shared() else {
            eprintln!("SKIPPED: no GPU adapter on this machine");
            return;
        };
        let n = 8 * 6 * 3;
        let src = core_image::ImageBuf::from_vec(
            8,
            6,
            (0..n).map(|i| (i % 97) as f32 / 97.0).collect(),
        );
        let mut g = GpuImage::upload(ctx, &src).expect("upload");
        let back = g.download(ctx).expect("download");
        assert_eq!((back.width(), back.height()), (8, 6));
        assert_eq!(back.data(), src.data(), "the round trip must be exact");
        g.relabel(4, 6);
        let back = g.download(ctx).expect("download");
        assert_eq!((back.width(), back.height()), (4, 6));
        assert_eq!(
            back.data(),
            &src.data()[..4 * 6 * 3],
            "a relabelled image downloads its leading pixels"
        );
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test --release -p gpu-pipeline a_download_is_the_uploads_inverse`
Expected: FAIL to compile, `no method named download`.

- [ ] **Step 3: Implement**

After `bytes()` in `impl GpuImage`:

```rust
    /// Read the image back into a CPU buffer: the leading `width × height` pixels, whatever the
    /// capacity.
    ///
    /// ⚠⚠ **Its own submission, ordered after every earlier one on the queue**, so it reads the
    /// finished pixels of a chain that returned without waiting — which is what a handoff chain
    /// does, and what makes the resident final downloadable at the settle. `None` on any GPU
    /// failure, and guarded like every chain run: the caller is the render worker.
    pub fn download(&self, ctx: &GpuContext) -> Option<core_image::ImageBuf> {
        crate::guarded(|| self.download_inner(ctx))
    }

    fn download_inner(&self, ctx: &GpuContext) -> Option<core_image::ImageBuf> {
        let bytes = self.bytes();
        let readback = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gpu-image-download"),
            size: bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = ctx.device.create_command_encoder(&Default::default());
        enc.copy_buffer_to_buffer(&self.buffer, 0, &readback, 0, bytes);
        ctx.queue.submit(Some(enc.finish()));
        let slice = readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = ctx.device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        rx.recv().ok()?.ok()?;
        let data: Vec<f32> = bytemuck::cast_slice(&slice.get_mapped_range()).to_vec();
        readback.unmap();
        Some(core_image::ImageBuf::from_vec(self.width, self.height, data))
    }
```

- [ ] **Step 4: Run it to verify it passes**

Run: `cargo test --release -p gpu-pipeline a_download_is_the_uploads_inverse`
Expected: 1 passed.

- [ ] **Step 5: Commit**

```bash
git add crates/gpu-pipeline/src/image.rs
git commit -m "feat(gpu): GpuImage::download — the readback the settle and the fallback share"
```

---

### Task 2: `GpuChain` — `Keep`, and the four entry points over one `run_with`

**Files:**
- Modify: `crates/gpu-pipeline/src/chain.rs` (`Input` public without `Plane`; `Keep`; `run_with`; entry points; tests)
- Modify: `crates/gpu-pipeline/src/lib.rs:91` (re-export `Input`)

- [ ] **Step 1: Write the failing tests**

Add to `mod tests` in `chain.rs`, after `a_timed_handoff_chain_waits_for_its_readings` (which Step 3 rewrites):

```rust
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
        assert_eq!(back.data(), img.data(), "the kept image is the input, untouched");
        let again = three_stages(&ctx);
        let hit = again.run_rgba8_from(&kept).expect("gpu");
        assert_eq!(again.transfer_counts().0, 0, "a run from the resident uploads nothing");
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
        assert_eq!(chain.transfer_counts().0, 0, "a resident input uploads nothing");
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
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test --release -p gpu-pipeline a_kept`
Expected: FAIL to compile (`run_rgba8_keeping_input`, `run_keeping_output`, `Input` private).

- [ ] **Step 3: Implement**

(a) `Input` becomes public and loses `Plane`; add `Keep`:

```rust
/// Where a chain's input comes from.
#[derive(Clone, Copy)]
pub enum Input<'a> {
    /// Uploaded through staging (or taken from a deposited plane).
    Cpu(&'a ImageBuf),
    /// Already on the GPU: copied GPU-side, no upload. See [`GpuChain::apply_from`].
    Resident(&'a GpuImage),
}

/// What a run hands back beside its download.
///
/// ⚠⚠ **The resident final (2026-09-26).** The tail's input is kept on the GPU under its key
/// — either the pristine upload of a tail miss (`Input`) or the last prefix step's result
/// (`Output`) — so the next tail frame copies it GPU-side instead of uploading 82 MB.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Keep {
    Nothing,
    /// A GPU-side copy of the input, taken before any stage is encoded.
    Input,
    /// The chain's own image, after every stage; never deposited as well.
    Output,
}
```

Update the `Download::None` doc to say "See [`GpuChain::run_keeping_output`]".

(b) Delete `run_to_plane`, `run_to_plane_from`, `run_rgba8_from_plane`. Replace `apply_from` and add the three new entry points:

```rust
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
```

`run_rgba8` and `apply` pass `Keep::Nothing` and take `.0` / `(data, _)`.

(c) `run_with`:

```rust
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
```

Delete the `Input::Plane` refusal (`if matches!(input, Input::Plane(..)) && !shared_taken`). In the staging match, delete the `(false, Input::Plane(..))` arm: `(true, _) => None,`. After the staging match, before the stages:

```rust
        // ⚠⚠ **The kept input is copied BEFORE any stage is encoded**, in the same encoder, so it
        // is the pristine upload — what the tail's next frame will run from. A GPU-side copy of
        // 82 MB costs ~1 ms against the 8-10 ms upload it saves every frame after this one.
        let kept_input = match keep {
            Keep::Input => {
                let k = GpuImage::new(ctx, iw, ih)?;
                enc.copy_buffer_to_buffer(&img.buffer, 0, &k.buffer, 0, bytes);
                Some(k)
            }
            Keep::Nothing | Keep::Output => None,
        };
```

The `Download::None` branch becomes:

```rust
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
            drop(staging);
            // (timing block unchanged)
            // (RUSTROOM_DEBUG_XFER print unchanged)
            return Some((Vec::new(), kept));
        }
```

And in the download path, replace the deposit block with:

```rust
        // ⚠⚠ **Deposited HERE and nowhere earlier** ... (comment unchanged)
        // ⚠ Or kept: an output the caller asked for goes to the caller, never to the slot as well.
        let kept = if keep == Keep::Output {
            Some(img)
        } else {
            if let Some(slot) = self.offer_output {
                ctx.plane(slot).deposit(ow, oh, img);
            }
            kept_input
        };
        let out: Vec<u8> = slice.get_mapped_range().to_vec();
        ...
        Some((out, kept))
```

(d) Rewrite the timed handoff test:

```rust
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
```

(e) `lib.rs:91`: `pub use chain::{GpuChain, GpuStage, Input, Slot};`

- [ ] **Step 4: Run the chain's tests**

Run: `cargo test --release -p gpu-pipeline chain::`
Expected: all pass, including the three new ones and the rewritten timed test.

- [ ] **Step 5: Commit**

```bash
git add crates/gpu-pipeline/src/chain.rs crates/gpu-pipeline/src/lib.rs
git commit -m "feat(gpu): GpuChain keeps its input or its output on the GPU; the plane-shaped tail entry points go"
```

---

### Task 3: `accelerate` — the tail is never offered the prefix plane

**Files:**
- Modify: `crates/gpu-pipeline/src/accelerate.rs:424-505` (drop `prefix_offered`; outer walk gets `None`), tests at 1655–1712, 1976, 2034, 3150
- Modify: `crates/gpu-pipeline/examples/gpu_run_bench.rs:40`, `crates/app/examples/bench_preview.rs:529,572`, `crates/app/examples/probe_brush_base_producer.rs:109,157`, `crates/app/examples/probe_prefix_tail_seam.rs:167`

- [ ] **Step 1: Rewrite the test**

Replace `the_handed_back_chain_takes_the_prefix_plane` with:

```rust
    /// ⚠⚠ **The outer tail's chain takes NO plane — since the resident final (2026-09-26).** Its
    /// input comes from `PrefixCache::resident_final` under a key, which is what makes a stale
    /// hit unrepresentable; the anonymous prefix slot is step→step only. A tail offered
    /// `Slot::Prefix` would take a step's leftover at matching dimensions and render a plausible
    /// photograph — the sabotage this pins is `Some(crate::Slot::Prefix)` in `accelerate`'s
    /// outer walk.
    #[test]
    fn the_handed_back_chain_takes_no_plane() {
        let Some(ctx) = ctx() else { return };
        let e = with_hsl();
        let (_, chain) = accelerate(
            tail_of(&e),
            &e,
            SCALE,
            TailFacts {
                region: None,
                profile: ProfileRendering::default(),
                spec: RenderSpec::default(),
            },
            &ctx,
        );
        let chain = chain.expect("fixture check: this tail ends in a chain");
        assert_eq!(chain.takes_from(), None);
    }
```

- [ ] **Step 2: Implement**

Delete the `prefix_offered` field and its doc from `TailFacts`. In `accelerate`, the outer `walk` call's last argument becomes `None` with this comment replacing the current one:

```rust
        // ⚠⚠ **`None`: the outer tail takes no plane (2026-09-26).** Its input is the resident
        // final, looked up by KEY in `PrefixCache`, so the anonymous slot is step→step only. The
        // one thing it must never take is a mask's leftover —
        // `the_outer_tails_own_chain_never_takes_a_masks_leftover_plane` pins that.
        None,
```

Update the `flush` comment at ~604 (`mark_shared` here too… `the_handed_back_chain_takes_the_prefix_plane` pins it) to: "⚠ `mark_shared` here too, for the recursive walk's sake: a mask whose whole sub-list is one chain lands in this arm."

Delete every `prefix_offered: false,` (and the three-line comment above the one in `tail_of`'s helper at ~1665) in `accelerate.rs` and the four examples.

- [ ] **Step 3: Verify**

Run: `cargo test --release -p gpu-pipeline accelerate::` — all pass.
Run: `cargo build --release --examples -p app -p gpu-pipeline` — builds.
Run: `grep -rn 'prefix_offered' crates` — empty (⚠ positive control: `grep -rn 'takes_from' crates | head -1` finds something).

- [ ] **Step 4: Commit**

```bash
git add crates/gpu-pipeline/src/accelerate.rs crates/gpu-pipeline/examples/gpu_run_bench.rs crates/app/examples
git commit -m "refactor(gpu): the tail is never offered the prefix plane; TailFacts::prefix_offered goes"
```

---

### Task 4: `PrefixCache` — the resident final and the settle

**Files:**
- Modify: `crates/app/src/prefix_cache.rs` (type, field, four methods, tests)

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `prefix_cache.rs`:

```rust
    fn gpu() -> Option<&'static gpu_pipeline::GpuContext> {
        match gpu_pipeline::shared() {
            Some(c) => Some(c),
            None => {
                eprintln!("SKIPPED: no GPU adapter on this machine");
                None
            }
        }
    }

    fn small(seed: f32) -> ImageBuf {
        ImageBuf::from_vec(
            6,
            4,
            (0..6 * 4 * 3).map(|i| (i as f32 * 0.01 + seed) % 1.0).collect(),
        )
    }

    /// The slot is keyed on the checkpoint AND the rect: a hit needs both, and a miss replaces.
    #[test]
    fn the_resident_final_is_keyed_on_the_checkpoint_and_the_rect() {
        let Some(ctx) = gpu() else { return };
        let mut cache = PrefixCache::default();
        let rect = pipeline::Rect { x: 1, y: 1, w: 2, h: 2 };
        let key = (fake_key(1), None);
        assert!(cache.resident_final(&key).is_none(), "empty to start");
        let img = gpu_pipeline::GpuImage::upload(ctx, &small(0.1)).expect("upload");
        cache.keep_resident_final(key.clone(), img);
        assert!(cache.resident_final(&key).is_some(), "a hit under its own key");
        assert!(
            cache.resident_final(&(fake_key(2), None)).is_none(),
            "another checkpoint misses"
        );
        assert!(
            cache.resident_final(&(fake_key(1), Some(rect))).is_none(),
            "the same checkpoint at a rect misses"
        );
        let crop = gpu_pipeline::GpuImage::upload(ctx, &small(0.5)).expect("upload");
        cache.keep_resident_final((fake_key(1), Some(rect)), crop);
        assert!(
            cache.resident_final(&key).is_none(),
            "one slot: keeping the crop replaced the whole frame"
        );
    }

    /// ⚠⚠ **The settle downloads a whole-frame resident that has no CPU final, once.** A second
    /// settle finds the final and does nothing; a rect resident is never a final.
    #[test]
    fn the_settle_downloads_the_resident_final_once_and_never_a_crop() {
        let Some(ctx) = gpu() else { return };
        let mut cache = PrefixCache::default();
        let src = small(0.2);
        let key = (fake_key(3), None);
        cache.keep_resident_final(
            key.clone(),
            gpu_pipeline::GpuImage::upload(ctx, &src).expect("upload"),
        );
        assert!(cache.settle_from_resident(ctx), "the first settle downloads");
        let got = cache.get(&fake_key(3)).expect("the download is the final");
        assert_eq!(got.data(), src.data(), "sample for sample");
        assert_eq!(cache.final_len(), 1);
        assert!(!cache.settle_from_resident(ctx), "a second settle has nothing to do");
        assert!(
            cache.resident_final(&key).is_some(),
            "and the resident survives the settle for the tail frames to come"
        );
        let rect = pipeline::Rect { x: 0, y: 0, w: 3, h: 2 };
        cache.keep_resident_final(
            (fake_key(4), Some(rect)),
            gpu_pipeline::GpuImage::upload(ctx, &small(0.3)).expect("upload"),
        );
        assert!(
            !cache.settle_from_resident(ctx),
            "a region crop is not a final and must never be cached as one"
        );
        assert_eq!(cache.final_len(), 1);
    }
```

(`fake_key` exists in the module already, at ~`render_engine.rs:3252`? No — check `prefix_cache.rs`'s tests for `fake_key`; if it lives only in `render_engine.rs`, add to `prefix_cache.rs`'s tests:)

```rust
    fn fake_key(tag: usize) -> CacheKey {
        let e = edit_model::Edits {
            exposure: tag as f32,
            ..Default::default()
        };
        let steps = pipeline::build_prefix_steps(
            &e,
            pipeline::RenderScale::default(),
            None,
            pipeline::Resampler::default(),
            None,
        );
        (PrefixKey::upto(&steps, 0, pipeline::RenderScale::default()), tag)
    }
```

(⚠ Verify `RenderScale::default()` exists; if not, use the constructor `render_engine.rs:3252`'s `fake_key` uses — copy that function verbatim.)

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test --release -p app prefix_cache::tests::the_`
Expected: FAIL to compile.

- [ ] **Step 3: Implement**

After `CacheKey`:

```rust
/// What the tail ran from: the final checkpoint, and the region rect when the frame was scoped
/// to one. See [`PrefixCache::resident_final`].
pub(crate) type TailKey = (CacheKey, Option<pipeline::Rect>);
```

Field, after `resident`:

```rust
    /// ONE tail input held on the GPU, with its key. See [`Self::resident_final`].
    resident_final: Option<(TailKey, gpu_pipeline::GpuImage)>,
```

(`with_mid_budget`: `resident_final: None,`.) Methods, after `resident`:

```rust
    /// The tail's input under `key`, if it is the one held on the GPU.
    ///
    /// ⚠⚠ **The resident final (2026-09-26).** After a settle every tail-slider frame used to clone
    /// the CPU final (2.5 ms), memcpy it into staging (2.3 ms) and copy it up, before the tail ran:
    /// ~8-10 ms of a ~34 ms frame at rung 3200, on the most common gesture there is. The tail's
    /// chain copies from this image instead (~1 ms for 82 MB). ⚠ The KEY is the whole mechanism:
    /// the one-shot plane it replaces was anonymous, so it needed a thread stamp, a dimension
    /// check and a clear to be safe, and a stale take still rendered a plausible photograph. Here
    /// a hit under the wrong key is unrepresentable.
    ///
    /// ⚠ One slot: a miss replaces it, a rung switch costs one upload. Only ever READ by the tail,
    /// which copies from it, so it is never written after it is kept.
    pub(crate) fn resident_final(&self, key: &TailKey) -> Option<&gpu_pipeline::GpuImage> {
        self.resident_final
            .as_ref()
            .filter(|(k, _)| k == key)
            .map(|(_, r)| r)
    }

    /// Keep `img` as the tail's input under `key`, replacing whatever was held.
    pub(crate) fn keep_resident_final(&mut self, key: TailKey, img: gpu_pipeline::GpuImage) {
        self.resident_final = Some((key, img));
    }

    /// The settle after a handoff: download a whole-frame resident that has no CPU final into
    /// the `Final` tier. `true` when it did.
    ///
    /// ⚠⚠ **This replaces a whole render.** A handoff frame leaves no CPU checkpoint, and the
    /// warm-behind used to re-run the last prefix step and a whole tail nobody saw — 92-108 ms
    /// measured — to leave one. The bytes are the same: the step already ran on the GPU, and this
    /// reads its output. ⚠ A rect resident is a region crop, never a final: the scoped drag's
    /// settle renders whole-buffer, as it always did.
    pub(crate) fn settle_from_resident(&mut self, ctx: &gpu_pipeline::GpuContext) -> bool {
        let Some(((key, None), r)) = self.resident_final.as_ref() else {
            return false;
        };
        if self.finals.iter().any(|(k, _)| k == key) {
            return false;
        }
        let key = key.clone();
        let Some(img) = r.download(ctx) else {
            return false;
        };
        self.insert(key, Arc::new(img), Tier::Final);
        true
    }
```

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test --release -p app prefix_cache::tests::the_`
Expected: 2 passed (plus any existing `the_` tests in that module).

- [ ] **Step 5: Commit**

```bash
git add crates/app/src/prefix_cache.rs
git commit -m "feat(app): PrefixCache holds the tail's input on the GPU under its key, and settles it by download"
```

---

### Task 5: `render_cached_with` — fill the slot three ways, run the tail from it

**Files:**
- Modify: `crates/app/src/render_engine.rs` (`Ran`; the `LOSE_THE_PLANE` hook → `REFUSE_THE_RESIDENT`; the loop 1150–1480; the tail 1600–1740; the `Ran` build; tests 2692–2810 and new ones)

- [ ] **Step 1: Write the failing tests**

Replace `a_handoff_the_tail_cannot_take_falls_back` (2745–2803) with:

```rust
    /// ⚠ A handoff frame whose tail cannot run from the resident must fall back, not fail — and
    /// the fallback is the settle's own path: download the resident, cache it as the final, render
    /// the ordinary way. ⚠ Every tail stage has a GPU twin, so no real edit set refuses the
    /// resident; the test refuses it deliberately (`REFUSE_THE_RESIDENT`), which is the run-time
    /// failure being guarded.
    #[test]
    fn a_handoff_whose_tail_cannot_run_from_the_resident_downloads_it() {
        if gpu_pipeline::shared().is_none() {
            eprintln!("SKIPPED: no GPU adapter on this machine");
            return;
        }
        let mut cache = PrefixCache::default();
        let mut e = hsl_edits();
        e.geometry.angle = 3.0;
        REFUSE_THE_RESIDENT.with(|l| l.set(true));
        let (frame, ran) = render_cached_with(
            &hazy_buffer(),
            &e,
            FrameSpec {
                gpu_tail: true,
                ..FrameSpec::fit(200)
            },
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::ProfileRendering::default(),
            &mut cache,
            &mut BrushCache::default(),
            true,
        );
        REFUSE_THE_RESIDENT.with(|l| l.set(false));
        assert!(ran.handoff, "fixture check: the step handed off");
        assert!(!ran.resident_final_hit, "the tail was refused the resident");
        assert_eq!(
            cache.final_len(),
            1,
            "the fallback downloads the resident and caches it as the final"
        );
        let (want, _) = render_cached(
            &hazy_buffer(),
            &e,
            FrameSpec {
                gpu_tail: false,
                ..FrameSpec::fit(200)
            },
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::ProfileRendering::default(),
            &mut PrefixCache::default(),
            &mut BrushCache::default(),
        );
        assert_eq!(frame.rgba, want.rgba, "the fallback renders the CPU picture");
    }

    /// A tail-slider frame with a fixed prefix, at a rung, with `gpu_tail` on.
    fn tail_frame(exposure: f32, angle: f32, rung: usize, cache: &mut PrefixCache) -> (RenderedFrame, Ran) {
        let mut e = hsl_edits();
        e.exposure = exposure;
        e.geometry.angle = angle;
        render_cached(
            &hazy_buffer(),
            &e,
            FrameSpec {
                gpu_tail: true,
                ..FrameSpec::fit(rung)
            },
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::ProfileRendering::default(),
            cache,
            &mut BrushCache::default(),
        )
    }

    /// ⚠⚠ **THE claim (2026-09-26): a tail-slider frame after the first uploads NOTHING.** The first
    /// frame's tail uploads the final and keeps it; the second copies it GPU-side. Both halves:
    /// the counter, and a picture byte-identical to the same edits through a fresh cache.
    #[test]
    fn a_tail_slider_frame_uploads_once_and_then_hits_the_resident_final() {
        if gpu_pipeline::shared().is_none() {
            eprintln!("SKIPPED: no GPU adapter on this machine");
            return;
        }
        let mut cache = PrefixCache::default();
        // ⚠ No prefix step at all: the final IS checkpoint 0, the bare buffer.
        let (_, first) = tail_frame(0.3, 0.0, 200, &mut cache);
        assert_eq!(first.prefix_stages, 0, "fixture check: a bare prefix");
        assert_eq!(first.tail_uploads, 1, "the first tail frame uploads");
        assert!(!first.resident_final_hit);
        let (second, ran) = tail_frame(0.6, 0.0, 200, &mut cache);
        assert_eq!(ran.tail_uploads, 0, "the second must upload nothing");
        assert!(ran.resident_final_hit);
        let (fresh, _) = tail_frame(0.6, 0.0, 200, &mut PrefixCache::default());
        assert_eq!(second.rgba, fresh.rgba, "a hit renders what a miss renders");
        assert_eq!(second.hist, fresh.hist);
        // ⚠ A rung change is another checkpoint: it misses and uploads its own input.
        let (other, ran) = tail_frame(0.6, 0.0, 120, &mut cache);
        assert_eq!(ran.tail_uploads, 1, "a rung change uploads");
        let (fresh, _) = tail_frame(0.6, 0.0, 120, &mut PrefixCache::default());
        assert_eq!(other.rgba, fresh.rgba);
    }

    /// ⚠⚠ **A prefix edit between two tail frames re-fills the slot from the step's own output**,
    /// so the tail still uploads nothing — and the picture follows the edit. The sabotage that
    /// ignores the key serves the old geometry to the new tail and fails the byte comparison.
    #[test]
    fn a_prefix_edit_between_two_tail_frames_refills_the_resident_from_the_step() {
        if gpu_pipeline::shared().is_none() {
            eprintln!("SKIPPED: no GPU adapter on this machine");
            return;
        }
        let mut cache = PrefixCache::default();
        let (_, a) = tail_frame(0.3, 2.0, 200, &mut cache);
        assert_eq!(a.gpu_prefix_steps, 1, "fixture check: geometry ran on the GPU");
        let (_, b) = tail_frame(0.6, 2.0, 200, &mut cache);
        assert!(b.resident_final_hit, "fixture check: the tail hits after the step");
        assert_eq!(b.tail_uploads, 0);
        let (c_frame, c) = tail_frame(0.6, 3.0, 200, &mut cache);
        assert_eq!(c.gpu_prefix_steps, 1, "the geometry re-ran");
        assert_eq!(c.tail_uploads, 0, "and its output fed the tail without an upload");
        let (fresh, _) = tail_frame(0.6, 3.0, 200, &mut PrefixCache::default());
        assert_eq!(c_frame.rgba, fresh.rgba, "the picture follows the edit");
        assert_ne!(
            c_frame.rgba, b_bytes(&mut cache),
            "fixture check: the two angles render different pictures"
        );
    }
```

(Replace the last `assert_ne!` with a plain check that angle 2 and angle 3 differ, computed before `c`: `let (b_frame, b) = ...; ... assert_ne!(c_frame.rgba, b_frame.rgba, ...)`. Write it that way; the helper `b_bytes` does not exist.)

```rust
    /// At 1:1 the tail's input is a crop, and the key carries the rect: two frames at one rect
    /// upload once; a pan uploads again, and renders the panned picture.
    #[test]
    fn at_one_to_one_the_resident_final_is_keyed_on_the_rect() {
        if gpu_pipeline::shared().is_none() {
            eprintln!("SKIPPED: no GPU adapter on this machine");
            return;
        }
        let at = |exposure: f32, rect: RegionRect, cache: &mut PrefixCache| {
            let mut e = hsl_edits();
            e.exposure = exposure;
            render_cached(
                &hazy_buffer(),
                &e,
                FrameSpec {
                    gpu_tail: true,
                    region: Some(rect),
                    ..FrameSpec::fit(200)
                },
                1.0,
                None,
                color::wb::Illuminant::default(),
                pipeline::ProfileRendering::default(),
                cache,
                &mut BrushCache::default(),
            )
        };
        let a = RegionRect { x: 10, y: 10, w: 60, h: 40 };
        let b = RegionRect { x: 70, y: 50, w: 60, h: 40 };
        let mut cache = PrefixCache::default();
        let (_, first) = at(0.3, a, &mut cache);
        assert_eq!(first.tail_uploads, 1);
        let (_, second) = at(0.6, a, &mut cache);
        assert_eq!(second.tail_uploads, 0, "the same rect hits");
        assert!(second.resident_final_hit);
        let (panned, ran) = at(0.6, b, &mut cache);
        assert_eq!(ran.tail_uploads, 1, "a pan is another crop and must upload");
        let (fresh, _) = at(0.6, b, &mut PrefixCache::default());
        assert_eq!(panned.rgba, fresh.rgba, "and renders the panned picture");
    }

    /// A handoff frame's resident, downloaded, IS the final checkpoint the ordinary path caches.
    #[test]
    fn a_handed_off_residents_download_equals_the_cached_final() {
        if gpu_pipeline::shared().is_none() {
            eprintln!("SKIPPED: no GPU adapter on this machine");
            return;
        }
        let ctx = gpu_pipeline::shared().expect("checked");
        let mut with = PrefixCache::default();
        let (_, ran) = handed(3.0, true, &mut with);
        assert!(ran.handoff, "fixture check");
        assert_eq!(with.final_len(), 0, "fixture check: no CPU final after a handoff");
        assert!(with.settle_from_resident(ctx), "the settle downloads it");
        let mut without = PrefixCache::default();
        let (_, ran) = handed(3.0, false, &mut without);
        assert!(!ran.handoff);
        let a = with.finals[0].1.clone();
        let b = without.finals[0].1.clone();
        assert_eq!(with.finals[0].0, without.finals[0].0, "the same key");
        assert_eq!(a.data(), b.data(), "the same checkpoint, sample for sample");
    }
```

Keep `handed`, `a_handed_off_frame_renders_the_same_bytes_and_caches_no_final` as they are.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test --release -p app render_engine::tests::a_tail_slider`
Expected: FAIL to compile (`tail_uploads`, `resident_final_hit`, `REFUSE_THE_RESIDENT`).

- [ ] **Step 3: Implement — `Ran` and the hook**

`Ran` (after `handoff`):

```rust
    /// How many uploads the TAIL's chain paid. ⚠ The resident final's observable (2026-09-26): a
    /// tail-slider frame after the first must read 0.
    pub tail_uploads: usize,
    /// Whether the tail ran from the resident final rather than from a CPU image.
    pub resident_final_hit: bool,
```

Replace the `LOSE_THE_PLANE` thread-local with:

```rust
thread_local! {
    /// Test-only: refuse the tail its resident final on a handoff frame, so the download fallback
    /// runs. Every tail stage has a GPU twin, so no real edit set produces the refusal.
    static REFUSE_THE_RESIDENT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}
```

Update the doc on `render_cached` (`/// [`render_cached_with`] with the handoff OFF — every checkpoint cached…`) — unchanged in meaning; keep.

- [ ] **Step 4: Implement — the loop**

Replace lines 1150–1170 (`gpu_prefix_steps` … `prefix_offered`) with:

```rust
    let mut gpu_prefix_steps = 0usize;
    let mut gpu_prefix_uploads = 0usize;
    // ⚠⚠ **When the last step may run WITHOUT a download.** A whole-frame render, sRGB display,
    // and none of the views that need float pixels (proof, visualize, grade range). Its output
    // then lives only as the resident final; the tail runs from it, and the settle downloads it.
    let handoff_ok = handoff
        && gpu_tail
        && region.is_none()
        && grade_view.is_none()
        && visualize.is_none()
        && soft_proof.is_none()
        && display_space == color::OutputSpace::Srgb;
    // The output size of a prefix whose last step handed off — there is no CPU image to ask.
    let mut handed: Option<(usize, usize)> = None;
```

(`prefix_ctx` stays.) In the loop, replace from `if let Some(g) = accelerated.as_mut() {` through the end of the `img = Arc::new(match accelerated { ... })` block with:

```rust
            let is_last = i + 1 == steps.len();
            if let Some(g) = accelerated.as_mut() {
                g.allow_shared_input(gpu_pipeline::Slot::Prefix);
                // ⚠ The last step keeps its output for the tail instead of offering it to a next
                // step there is not: one buffer, one owner.
                if !is_last {
                    g.offer_output(gpu_pipeline::Slot::Prefix);
                }
            }
            // ⚠⚠ **THE LAST STEP KEEPS ITS OUTPUT ON THE GPU — the resident final (2026-09-26).**
            // With a download it is also the CPU checkpoint, as always; without one (`handoff_ok`)
            // it is the tail's only input, and `img` is left as the checkpoint before this step.
            // Any `None` falls through to the ordinary path below.
            let kept_last = match (is_last, accelerated.as_ref()) {
                (true, Some(g)) => {
                    let ctx = gpu_pipeline::shared();
                    let from_resident = if i == start {
                        ctx.and_then(|ctx| cache.resident(&key_at(i), &img, ctx))
                    } else {
                        None
                    };
                    let run = match from_resident {
                        Some((r, uploaded)) => {
                            gpu_prefix_uploads += usize::from(uploaded);
                            g.run_keeping_output(gpu_pipeline::Input::Resident(r), !handoff_ok)
                        }
                        None => g.run_keeping_output(gpu_pipeline::Input::Cpu(&img), !handoff_ok),
                    };
                    gpu_prefix_uploads += g.transfer_counts().0;
                    run
                }
                _ => None,
            };
            if let Some((out, kept)) = kept_last {
                gpu_prefix_steps += 1;
                let dims = (kept.width(), kept.height());
                cache.keep_resident_final((key_at(i + 1), None), kept);
                match out {
                    Some(out) => {
                        img = Arc::new(out);
                        if debug_steps {
                            eprintln!(
                                "    [{i}] {:>16} {:8.1} ms   -> {}x{} ({:.1} Mpx) KEPT",
                                step.stage.name(),
                                t_step.elapsed().as_secs_f64() * 1000.0,
                                img.width(),
                                img.height(),
                                (img.width() * img.height()) as f64 / 1e6,
                            );
                        }
                        cache.insert(key_at(i + 1), img.clone(), tier_at(i + 1));
                    }
                    None => {
                        handed = Some(dims);
                        if debug_steps {
                            eprintln!(
                                "    [{i}] {:>16} {:8.1} ms   -> {}x{} HANDED OFF (no download)",
                                step.stage.name(),
                                t_step.elapsed().as_secs_f64() * 1000.0,
                                dims.0,
                                dims.1,
                            );
                        }
                    }
                }
                break;
            }
            img = Arc::new(match accelerated {
                Some(g) => {
                    // (the existing comment block on take-and-offer stays verbatim)
                    gpu_prefix_steps += 1;
                    let from_resident = if i == start {
                        let ctx = gpu_pipeline::shared();
                        ctx.and_then(|ctx| cache.resident(&key_at(i), &img, ctx))
                            .and_then(|(r, uploaded)| {
                                gpu_prefix_uploads += usize::from(uploaded);
                                g.apply_from(r)
                            })
                    } else {
                        None
                    };
                    let out = match from_resident {
                        Some(out) => out,
                        None => g.apply((*img).clone()),
                    };
                    gpu_prefix_uploads += g.transfer_counts().0;
                    out
                }
                None => {
                    // (the existing THE CLEAR comment stays; drop its `prefix_offered` line)
                    if let Some(ctx) = &prefix_ctx {
                        ctx.prefix_plane().clear();
                    }
                    step.stage.apply((*img).clone())
                }
            });
```

⚠ `break` after a kept last step skips the loop's own `debug_steps` print and `cache.insert` — both are done inside the block above, which is why the `KEPT` arm inserts itself. ⚠ Delete the two `prefix_offered = true;` lines and the `prefix_offered = false;` line.

After the loop, `(pw, ph)` and `input` stay as they are (`handed.unwrap_or(...)`, `match (handed, rect)`), with the comment "⚠ No CPU input after a handoff: the tail's input is the resident final."

- [ ] **Step 5: Implement — the tail**

`build_tail`'s `TailFacts` loses `prefix_offered`. Replace the whole `from_plane` block (`let mut from_plane: Option<Packed> = None; let input: ImageBuf = match input { … }`) with:

```rust
    // ⚠⚠ **The tail's input, and where it lives (2026-09-26).** `tail_key` names it: the final
    // checkpoint, at this rect. `PrefixCache::resident_final` holds it on the GPU under exactly
    // that key when a previous frame ran from it, or the last prefix step just kept it. A tail
    // that is one GPU chain runs FROM it; a tail that cannot (a CPU stage first, a GPU failure)
    // gets a CPU image — and on a handoff frame that means downloading the resident, which is
    // the settle's own path and caches the final as a side effect. Nothing else in this function
    // needs to know which happened.
    let tail_key = (key_at(steps.len()), rect);
    let mut tail_uploads = 0usize;
    let mut resident_final_hit = false;
    let refused = {
        #[cfg(test)]
        {
            REFUSE_THE_RESIDENT.with(|l| l.get())
        }
        #[cfg(not(test))]
        {
            false
        }
    };
    let input: ImageBuf = match input {
        Some(i) => i,
        None => {
            // ⚠ A handoff frame: no CPU image exists. The resident final must serve the tail
            // directly; if it cannot, it is downloaded and cached as the final.
            let t_dl = std::time::Instant::now();
            let ctx = gpu_pipeline::shared();
            let from_gpu = ctx.filter(|_| !refused).and_then(|ctx| {
                let (pipe, chain) = build_tail(color::OutputSpace::Srgb);
                let chain = chain.filter(|_| pipe.stage_names().is_empty())?;
                let r = cache.resident_final(&tail_key)?;
                let bytes = chain.run_rgba8_from(r)?;
                let _ = ctx;
                Some(Packed::Rgba8 {
                    bytes,
                    w: pw,
                    h: ph,
                })
            });
            match from_gpu {
                Some(p) => {
                    resident_final_hit = true;
                    if debug_steps {
                        eprintln!(
                            "  TIMING tail from resident {:6.1} ms",
                            t_dl.elapsed().as_secs_f64() * 1000.0
                        );
                    }
                    early = Some(p);
                    ImageBuf::new(0, 0)
                }
                None => match ctx.filter(|ctx| cache.settle_from_resident(ctx)) {
                    Some(_) => (*cache
                        .get(&key_at(steps.len()))
                        .expect("settle_from_resident inserted the final"))
                    .clone(),
                    // ⚠ The last resort — the GPU refused even a download. The step re-runs on
                    // the CPU from the checkpoint before it, which `img` still is.
                    None => {
                        let last = steps.last().expect("a handoff implies a prefix step");
                        let out = Arc::new(last.stage.apply((*img).clone()));
                        cache.insert(key_at(steps.len()), out.clone(), Tier::Final);
                        (*out).clone()
                    }
                },
            }
        }
    };
```

⚠ `early` must be declared before this block: `let mut early: Option<Packed> = None;` (it replaces `from_plane`). ⚠ The `settle_from_resident` arm: `Option::filter` on a `&GpuContext` closure returning bool — write it as `match ctx { Some(ctx) if cache.settle_from_resident(ctx) => …, _ => last resort }` if the borrow checker objects to the closure borrowing `cache` mutably.

Then the plain sRGB arm (`Some(c) if !float_needed =>`) becomes:

```rust
                    Some(c) if !float_needed => {
                        let t_cpu = std::time::Instant::now();
                        let mid = pipe.run(input);
                        let (w, h) = (mid.width(), mid.height());
                        let t_gpu = std::time::Instant::now();
                        // ⚠⚠ **From the resident final when the tail is one chain**, else a
                        // miss that keeps its upload for the next frame. A tail with a CPU stage
                        // first has an input that is not the checkpoint, and keeps nothing.
                        let whole = pipe.stage_names().is_empty();
                        let got = match cache.resident_final(&tail_key).filter(|_| whole) {
                            Some(r) => {
                                resident_final_hit = true;
                                c.run_rgba8_from(r)
                            }
                            None if whole => c.run_rgba8_keeping_input(&mid).map(|(b, kept)| {
                                cache.keep_resident_final(tail_key.clone(), kept);
                                b
                            }),
                            None => c.run_rgba8(&mid),
                        };
                        tail_uploads += c.transfer_counts().0;
                        if debug_steps { /* unchanged print */ }
                        match got {
                            Some(b) => (Packed::Rgba8 { bytes: b, w, h }, None),
                            None => (Packed::Float(pipeline::PipelineStage::apply(&c, mid)), None),
                        }
                    }
```

⚠ `pipe.run(input)` consumes `input`; `mid` is the image the chain runs on — when `whole`, `mid` **is** `input` (an empty pipeline returns its input), so keeping `mid` keeps the tail's input. And the float arm (`other =>`):

```rust
                    other => {
                        // ⚠ The float path keeps the tail's input too, when the tail is one chain:
                        // a non-sRGB display, visualize, or the proof's sRGB run.
                        match other {
                            Some(c) if pipe.stage_names().is_empty() => {
                                let out = match cache.resident_final(&tail_key) {
                                    Some(r) => {
                                        resident_final_hit = true;
                                        c.apply_from(r)
                                    }
                                    None => None,
                                };
                                let out = match out {
                                    Some(o) => o,
                                    None => {
                                        let (o, kept) = c.apply_keeping_input(input);
                                        if let Some(k) = kept {
                                            cache.keep_resident_final(tail_key.clone(), k);
                                        }
                                        o
                                    }
                                };
                                tail_uploads += c.transfer_counts().0;
                                (Packed::Float(out), None)
                            }
                            other => (
                                Packed::Float(gpu_pipeline::rejoin(pipe, other).run(input)),
                                None,
                            ),
                        }
                    }
```

⚠ On the hit path `input` is unused and dropped — fine; on `apply_from` failing (`None`) the miss path consumes it.

The `(out, plane)` match head: `let (out, plane) = match (early, grade_view) { (Some(p), _) => (p, None), (None, grade_view) => … }` — same shape as today with `from_plane` renamed. Delete `let handoff_taken = from_plane.is_some();` and compute `handoff: handed.is_some()` in the `Ran` build. The `TIMING build_tail` print drops `takes {:?}` and `prefix_offered {}` and their arguments.

`Ran` build:

```rust
        Ran {
            prefix_stages,
            gpu_prefix_steps,
            gpu_prefix_uploads,
            handoff: handed.is_some(),
            region_scoped,
            bytes,
            tail_uploads,
            resident_final_hit,
        },
```

- [ ] **Step 6: Run the render-engine GPU tests**

Run, one filter each:
- `cargo test --release -p app render_engine::tests::a_tail_slider`
- `cargo test --release -p app render_engine::tests::a_prefix_edit_between`
- `cargo test --release -p app render_engine::tests::at_one_to_one`
- `cargo test --release -p app render_engine::tests::a_handed_off`
- `cargo test --release -p app render_engine::tests::a_handoff_whose`
- `cargo test --release -p app render_engine::tests::a_straighten_drag`
- `cargo test --release -p app render_engine::` (the whole module)

Expected: all pass.

- [ ] **Step 7: Sabotage — ignore the key**

Temporarily make `PrefixCache::resident_final` return the slot without the key comparison. Run `cargo test --release -p app render_engine::tests::at_one_to_one` — must FAIL on "renders the panned picture". Run `cargo test --release -p app prefix_cache::tests::the_resident_final_is_keyed` — must FAIL. Revert.

- [ ] **Step 8: Commit**

```bash
git add crates/app/src/render_engine.rs
git commit -m "feat(app): the tail runs from the resident final — the prefix→tail plane, the handoff fallback and LOSE_THE_PLANE go"
```

---

### Task 6: The worker's settle downloads the resident

**Files:**
- Modify: `crates/app/src/render_engine.rs` (struct field, `new`, the warm-behind, accessor; test at 2805–2825)

- [ ] **Step 1: Write the failing test**

Replace `a_handed_off_frame_is_warmed_behind` with:

```rust
    /// ⚠⚠ **A handed-off frame is settled by a DOWNLOAD, not a render (2026-09-26).** The
    /// warm-behind used to re-run the last step and a whole tail nobody saw; now it reads the
    /// resident final into the CPU cache — `settle_downloads` says so, `warm_renders` stays 0 —
    /// and the next tail frame renders what a from-scratch render renders.
    #[test]
    fn a_handed_off_frame_is_settled_by_a_download() {
        if gpu_pipeline::shared().is_none() {
            eprintln!("SKIPPED: no GPU adapter on this machine");
            return;
        }
        let engine = chained_engine(&hazy_buffer());
        let mut e = hsl_edits();
        e.geometry.angle = 3.0;
        let spec = FrameSpec {
            gpu_tail: true,
            ..FrameSpec::fit(200)
        };
        engine.request(e.clone(), spec);
        let _ = wait_for_frame(&engine);
        wait_until(|| engine.settle_downloads() == 1, "the settle download");
        std::thread::sleep(WARM_BEHIND * 3);
        assert_eq!(engine.warm_renders(), 0, "nothing was re-rendered");
        e.exposure = 0.7;
        engine.request(e.clone(), spec);
        let frame = wait_for_frame(&engine);
        let (want, _) = render_cached(
            &hazy_buffer(),
            &e,
            spec,
            1.0,
            None,
            color::wb::Illuminant::default(),
            pipeline::ProfileRendering::default(),
            &mut PrefixCache::default(),
            &mut BrushCache::default(),
        );
        assert_eq!(frame.rgba, want.rgba, "the settled final feeds the next tail frame");
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --release -p app render_engine::tests::a_handed_off_frame_is_settled`
Expected: FAIL to compile (`settle_downloads`).

- [ ] **Step 3: Implement**

Field after `warmed`:

```rust
    /// How many settles downloaded the resident final instead of rendering. See `warmed`.
    settle_downloads: Arc<std::sync::atomic::AtomicUsize>,
```

In `new`: `let settle_downloads = Arc::new(AtomicUsize::new(0)); let settles_worker = Arc::clone(&settle_downloads);` beside `warmed`; inside the thread `let settle_downloads = settles_worker;`; the struct literal gains `settle_downloads,`. The `Err(RecvTimeoutError::Timeout)` arm becomes:

```rust
                        Err(RecvTimeoutError::Timeout) => {
                            let t1 = std::time::Instant::now();
                            // ⚠⚠ **A handoff frame's final is on the GPU: download it, and skip
                            // the tail nobody sees (2026-09-26).** 92-108 ms of re-render became
                            // one readback. A scoped drag has no whole-frame resident and renders
                            // whole-buffer as it always did.
                            let settled = ran.handoff
                                && gpu_pipeline::shared()
                                    .is_some_and(|ctx| cache.settle_from_resident(ctx));
                            if settled {
                                settle_downloads
                                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                if debug_render {
                                    eprintln!(
                                        "  settle: {:6.1} ms  downloaded the resident final",
                                        t1.elapsed().as_secs_f64() * 1000.0
                                    );
                                }
                            } else {
                                let (_, warm) = render_cached(/* unchanged */);
                                warmed.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                if debug_render { /* unchanged */ }
                            }
                        }
```

Accessor after `warm_renders`:

```rust
    /// How many settles downloaded the resident final rather than rendering. See `warmed`.
    pub fn settle_downloads(&self) -> usize {
        self.settle_downloads
            .load(std::sync::atomic::Ordering::Relaxed)
    }
```

The worker's `render:` debug line gains `tail_up {} hit {}` with `ran.tail_uploads` and `ran.resident_final_hit`.

- [ ] **Step 4: Run**

Run: `cargo test --release -p app render_engine::tests::a_handed_off_frame_is_settled` — passes.
Run: `cargo test --release -p app render_engine::tests::warms` — the three warm-behind tests still pass.

- [ ] **Step 5: Commit**

```bash
git add crates/app/src/render_engine.rs
git commit -m "feat(app): the settle after a handoff downloads the resident final instead of re-rendering"
```

---

### Task 7: Gate, measure, record

- [ ] **Step 1: The gate, each exit code read on its own**

```bash
cargo fmt --all -- --check; echo "fmt=$?"
CARGO_TARGET_DIR=target/clippy cargo clippy --workspace --all-targets -- -D warnings; echo "clippy=$?"
cargo test --workspace 2>&1 | tail -30; echo "tests=$?"
```

Goldens: `cargo test --workspace golden` — 78, none moved.

- [ ] **Step 2: Measure**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
uptime
cargo run --release -p app --example bench_gpu_tail -- <the DSCF6263 RAW> 2>&1 | grep -A1 'REAL SIDECAR'
RUSTROOM_DEBUG_RENDER=1 cargo run --release -p app --example probe_straighten_drag -- <the DSCF6263 RAW> 2>&1 | grep -E 'settle:|warm behind|render:' | tail -12
```

(⚠ Find the RAW path with `grep -n 'DSCF6263' docs/JOURNAL.md | head -3`.) Compare against the branch point: `git stash` is not needed — the "before" figures are in the journal's per-pixel-fusion section (21.6 ms global drag; 92–108 ms warm-behind). Report both with the load.

- [ ] **Step 3: Journal, CLAUDE.md, spec**

`docs/JOURNAL.md`: a new `### 2026-09-26 — the resident final checkpoint: the tail's input stays on the GPU (branch `resident-final`)` section above the per-pixel-fusion one, plus its Contents line. It records: what was built, the three fill paths, the measured before/after with load, the spec deviation (the last-resort CPU re-run), what was removed, the sabotage that was run, and the test count. `CLAUDE.md`'s State block: overwrite the test count and add one line for the resident final under the M7.3b line; nothing appended. The spec gets a one-line amendment under §3 naming the last-resort re-run.

- [ ] **Step 4: Commit and launch**

```bash
git add docs/JOURNAL.md CLAUDE.md docs/superpowers/specs/2026-09-26-resident-final-checkpoint-design.md
git commit -m "docs: the resident final checkpoint in the journal and CLAUDE.md — measured before and after"
cargo run --release -p app &
```

Report: the numbers with load, the deviation, `chain.rs`'s code line count (over 600), and that nothing was pushed.

---

## Self-review

- **Spec coverage.** §1 slot → Task 4. §2 three fill paths → Task 5 (tail miss keeps input; last step with download keeps output; handoff keeps output). §3 tail from resident, removals → Tasks 2, 3, 5. §4 settle → Tasks 4, 6. §5 entry points → Task 2 (`run_rgba8_keeping_input`, `apply_keeping_input`, `run_rgba8_from`, `run_keeping_output`). §6 observables and tests → Tasks 5, 6 (`tail_uploads`, `resident_final_hit`, `settle_downloads`; the six listed tests). §7 measurement → Task 7. §8 placement → as listed.
- **Placeholders.** None: every code step is complete. Two "verify X exists" notes (`fake_key`, `RenderScale::default`) carry their fallback.
- **Type consistency.** `TailKey = (CacheKey, Option<pipeline::Rect>)`; `rect: Option<RegionRect>` where `RegionRect` *is* `pipeline::Rect` (`render_engine.rs:81`), so `(key_at(steps.len()), rect)` types directly. `run_keeping_output(Input, bool) -> Option<(Option<ImageBuf>, GpuImage)>` in Task 2 matches every use in Task 5. `keep_resident_final(TailKey, GpuImage)`, `resident_final(&TailKey) -> Option<&GpuImage>`, `settle_from_resident(&GpuContext) -> bool` match Tasks 5 and 6.
