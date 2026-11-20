# The pipeline's layers: a review, and the version that should replace it

**Date:** 2026-09-26 · **Branch read:** `main` at `2bfa478` (checked out as `m5.1c-parametric-curve`, level with `main`) · **Status:** analysis, nothing built, nothing measured by me

This document reads the prefix, the tail, the checkpoints, the region-scoped path, the GPU seams,
the geometry stage, masks and spot removal as they exist in the tree today, and then proposes the
efficient version of each. Every figure marked **(measured)** is quoted from `docs/JOURNAL.md`,
`docs/BACKLOG.md` or a code comment, with the source named. Every figure marked **(inferred)** is
my arithmetic on those numbers and is not evidence until a probe confirms it. The project's own rule
applies: a claim nobody ran is not a finding.

Files read in full: `crates/pipeline/src/lib.rs` (the builders, keys, region plan), every stage
under `crates/pipeline/src/stages/` that the prefix or the seams touch, `crates/pipeline/src/brush/`,
`crates/app/src/render_engine.rs`, `crates/app/src/prefix_cache.rs`,
`crates/app/src/batch_export/render.rs`, `crates/app/src/view_region.rs`, the canvas's request
site, and `crates/gpu-pipeline/src/{chain,accelerate,geometry,mask,image,context,shared_plane,
dispatch,box_wgsl,quantize}.rs`.

---

## 1. The current design, step by step

### 1.1 The two halves, and the order inside each

The pipeline is one ordered list of `PipelineStage`s built from `Edits`, split at a fixed point
into a **prefix** (corrections of the capture, always whole-buffer, cached) and a **tail**
(adjustments, run on the visible region, never cached). `build_pipeline` is literally
`prefix ++ tail`, so the CLI, the goldens and the app cannot drift apart
([lib.rs:1736](crates/pipeline/src/lib.rs:1736)).

**Prefix, since M7.3b** ([lib.rs:913](crates/pipeline/src/lib.rs:913)):

```
lens correction → noise reduction → dehaze → spot removal → geometry → masked NR/dehaze (one step per mask)
```

Each step is inserted only when active. Lens is first because its calibration is in sensor space.
NR and dehaze sit ahead of geometry so a geometric drag never re-runs them; that order was a
**measured speed trade** (straighten 137 → 35 ms, NR drag 102 → 161 ms at the fit rung, GPU path)
and the image-quality argument for it was retracted (journal, M7.3b). Spots are post-lens,
pre-perspective, because fusing the keystone into the geometry resample deleted the buffer they
used to run on. Geometry is last so every tail stage sees the final cropped pixels, which is also
what makes the vignette post-crop for free. Masked capture stages stay after geometry because their
coverage is rasterised in frame space.

**Tail** ([lib.rs:1457](crates/pipeline/src/lib.rs:1457)):

```
WB → exposure → contrast → tone regions → tone curve (4 curves) → local contrast →
saturation → HSL → [grade-view tap] → colour grading → masks (in list order) →
vignette → sharpening → display rendering → creative look → output transform
```

Every tail stage except local contrast and sharpening is **per-pixel**: a function of one pixel
(and, for the vignette, its position). Masks are the tail's only compound stage: each runs its own
sub-stage list on a clone of the whole image and blends the result back by coverage.

### 1.2 The prefix as steps, and the key that falls out of it

`build_prefix_steps` returns `(PrefixStepKey, Box<dyn PipelineStage>)` pairs. A prefix stage with
no key does not compile, and `PrefixKey::upto(steps, n, scale)` is the cumulative key of the first
`n` steps plus the render scale. The two resampling keys carry the `Resampler`; the masked-capture
key carries only the mask's **capture projection** (shape, refinements, NR, dehaze), so a masked
exposure drag cannot invalidate a whole-buffer denoise
([lib.rs:1319](crates/pipeline/src/lib.rs:1319)).

The dehaze airlight is a separate, memoised fact: `prefix_airlight` replays the prefix up to dehaze
on a 256 px downscale of the source, skipping the resampling family so a Straighten drag cannot
move it; `prefix_airlight_key` is built from the same replay list, so the memo in `PrefixCache`
cannot disagree with the computation ([lib.rs:761](crates/pipeline/src/lib.rs:761)).

### 1.3 The prefix cache (`crates/app/src/prefix_cache.rs`)

Two tiers of `(PrefixKey, max_dim) → Arc<ImageBuf>`:

| tier | holds | cap |
|---|---|---|
| `finals` | complete prefix outputs | 2 entries (fit + native rung) |
| `mids` | every intermediate checkpoint | 6 entries and 512 MiB, evicted **deepest-first** |

Deepest-first is a rule of its own: during a drag of step `g`, every checkpoint below `g` is
stale next frame, so plain LRU evicts the one entry that is still usable (journal, 2026-09-14).
Checkpoint 0 (the bare downscale) is stored only when it is not free. Two more slots live beside
the tiers since 2026-09-26: the airlight memo, and **one GPU-resident checkpoint** keyed by its full
`CacheKey`, which is the input of the step a drag resumes at.

### 1.4 The region-scoped prefix (native rungs only)

At a native rung the canvas asks for the visible rect grown by
`max(tail_kernel_radius(edits), 256)`. `render_cached_with` resumes at the deepest cached
checkpoint and, if a region is wanted, asks `region_plan` whether every step from the resume point
onward implements `input_rect_for`/`apply_region` ([lib.rs:613](crates/pipeline/src/lib.rs:613)).
The plan walks backwards, growing the rect by each local's kernel reach; any decline abandons the
plan. Lens, NR, dehaze and geometry accept; **spot removal and the masked captures decline**. An
area gate refuses to scope when the margined rect exceeds 50% of the frame (swept, journal M8.7).
A scoped output is **never cached**, and it always runs on the **CPU**: `gpu_prefix_steps` stays 0
on this path ([render_engine.rs:1281](crates/app/src/render_engine.rs:1281)).

### 1.5 The GPU seams

Four prefix stages have twins that run as a one-stage `GpuChain`: lens correction (a displacement
grid), NR, dehaze (airlight supplied as uniforms) and, since 2026-09-26, geometry. Each step is
offered to the seam **at apply time**, one stage per round trip, never chained across steps, so the
post-NR checkpoint survives for a dehaze drag (priced at M8.3o).

Three transfer-saving mechanisms sit on top:

- **The prefix plane** (`Slot::Prefix`, a per-thread one-shot `SharedPlane`): a GPU step deposits
  its output; the next GPU step, or the tail's first chain, takes it instead of re-uploading. A CPU
  step in between clears the slot ([render_engine.rs:1313](crates/app/src/render_engine.rs:1313)).
- **The resident input**: the checkpoint the first GPU step of a render starts from is uploaded
  once and kept on the GPU under its key; later frames of the same drag copy it GPU-side.
- **The handoff**: when the *last* prefix step runs on the GPU and the whole tail is one GPU chain
  ending in `output_transform` (sRGB, no region, no proof/visualize/grade view), the step's output
  is never downloaded. No final checkpoint exists for such a frame, so the worker's warm-behind
  re-renders once the channel has been quiet for 150 ms.

The mask slot (`Slot::Mask`) is a separate `SharedPlane` for a gating brush's base plane; the two
must never be merged (M8.14).

### 1.6 The tail's GPU chains

`gpu_pipeline::accelerate` walks the built tail, replaces runs of stages that have twins with one
`GpuChain` each (one upload, one compute pass, one download), recurses one level into masks, and
hands back the trailing chain separately so the byte path can quantise on the GPU and download
27 MB instead of 82 ([accelerate.rs](crates/gpu-pipeline/src/accelerate.rs)). A run is kept only if
its summed `cpu_cost` beats `TRANSFER_COST = 27 ms` per 6.8 Mpx. Inside a chain every stage is its
own dispatch over the whole image; per-pixel stages go through `Dispatch`, spatial ones borrow
shared scratch planes.

### 1.7 Geometry

`GeometryStage` does orientation, keystone, straighten and crop. With a keystone it is **one
gather** through `FusedMap::source_of` (un-crop → un-rotate → un-orient → inverse homography), region-
scopable under any orientation. Without a keystone it runs the original three passes: `orient`
(an index permutation into a new buffer), `rotate` over the **full** canvas, then a crop slice, and
it declines region scoping on a non-identity orientation
([geometry/mod.rs:131](crates/pipeline/src/stages/geometry/mod.rs:131)). The GPU twin always maps
coordinates and receives the map's constants from the same `FusedMap`; it writes its smaller
output into the same buffer and the chain relabels the image.

### 1.8 Masks

`MaskStage::apply` (CPU): resolve coverage per component (analytic shapes and ranges are evaluated
inside the pixel loop; a brush is a bbox-bounded 8-bit `CoverageMap`, prebuilt by `BrushCache` unless
it gates on the image), early-out if inert, **clone the whole image**, run every sub-stage on the
clone, then blend base and adjusted by coverage over the whole frame
([mask.rs](crates/pipeline/src/stages/mask.rs)). Measured at 6.5 Mpx: clone 2.1–3.8 ms, blend
10.3–11.9 ms for a brush, 45–48 ms for a luminance range and 68–111 ms for a colour range, because
the range's Oklab round trip runs inside the blend (`gpu-pipeline/src/mask.rs` constants).

`GpuMaskStage` is the same shape in one pass: `seed` (the clone, into four scratch planes),
`coverage`, the sub-stages' own dispatches, `blend`. It declines when any component is a gating
brush without a prebuilt map. `BrushCache` keys a component's map on the brush, the geometry, the
frame size, the region and, for a gating brush, the whole upstream `Edits` truncated to the masks
before it.

The masked NR/dehaze half of a mask is a **prefix** step (`MaskStage::new_capture`), whole-buffer,
CPU-only, cached like any prefix step, and it declines region scoping.

### 1.9 Spot removal

A plain sequential loop over each spot's ring bounding box: gather the source disc and the ring
mean, then blend. Realistic retouching costs **0.05–0.4 ms** on a cached prefix step (backlog,
`probe_spot_cost`), so a GPU port was measured and closed. It declines region scoping, which since
M7.3b means a spotted photo cannot scope NR or dehaze at 1:1.

### 1.10 The render worker

One thread per open photo, one request channel. The worker coalesces to the newest request,
renders, then, after a scoped or handed-off frame, waits 150 ms on the channel and if nothing
arrives renders again whole-buffer to fill the cache (the frame is discarded). A live gesture drops
one ladder rung by default (`DragQuality::Faster`), so the author's fitted view at rung 3200 drags
at 1600 unless he set `Full`.

### 1.11 Export and thumbnails

Export builds the same steps, offers each to the GPU seam with a download per step, and runs the
**tail on the CPU** (`batch_export/render.rs:259`). Thumbnails run the whole pipeline on the CPU.
Export is decode-bound: `Better` demosaic is 6.8 s of an ~8.3–9.2 s photo (journal, 2026-09-16).

### 1.12 Where a frame's time goes today (measured)

A Straighten drag on DSCF6263, rung 3200 (6.8 Mpx), from the journal's TIMING lines:

| part | before drag-frame work | after GPU geometry |
|---|---|---|
| geometry resample | 37.4 ms CPU | GPU, resident input, no download |
| GPU tail | 19.3 (82 MB up, 27 MB down) | ~27 headless submit+wait, ~34 in the app |
| histogram | 12.4 → 2.2 | 2.2 |
| airlight replay | 12.1 → 0 | 0 |
| tail input clone | 2.6 | 0 (handoff) |
| **frame** | **84.5 → 62–65** | **28.8 headless, ~40 in the app** |

Other anchors: a zero-work float round trip at 3200×2136 is **23–27 ms**, a byte one **16–21**
(`probe_rt`, M8.3h); a tail-slider frame after a settle is **~34 ms** in the app with `prefix_ran 0`;
the warm-behind after a Straighten release is **92–108 ms**; at the fit rung an NR drag is **161 ms**
and a dehaze drag **106** (M7.3b); a two-step scoped NR+dehaze drag at 1:1 is **34–37 ms** on the
CPU (M8.12); three GPU box passes on the running-sum body cost a flat **~11 ms** with the horizontal
pass uncoalesced (`box_wgsl.rs`).

---

## 2. Findings

Ordered by what they cost on the path the author actually edits through (whole-frame at rung
3200, GPU tail on), then export, then structure. Each names the file, the evidence, and whether
the cost is measured or inferred.

### F1. Every per-pixel stage is a full-frame pass, on both the CPU and the GPU

**Where:** every stage in §1.1's tail except local contrast and sharpening; `Dispatch::encode`
issues one dispatch per stage ([dispatch.rs:47](crates/gpu-pipeline/src/dispatch.rs:47)); on the
CPU each `apply` allocates a fresh buffer through `map_pixels`/`map_samples`
(`exposure`, `contrast`, `tone_regions`, `tone_curve`, `saturation`, `color_grading`,
`white_balance`, `display_rendering`, `creative_look`, `output_transform`, `vignette`).

**Evidence:** the drag frame's tail is `display_rendering + output_transform` plus the quantise
dispatch, and its submit+wait is ~27 ms headless including the geometry gather. Subtracting the
gather (the journal puts Catmull-Rom's extra over bilinear at ~4.5 ms, so the whole gather plus its
seed copy is plausibly 8–12 ms) leaves **~3–4 ms per full-frame per-pixel dispatch at 6.8 Mpx
(inferred)**. An 82 MB read plus write at the M2's bandwidth is ~1.6 ms, so scalar `array<f32>`
access is running at roughly half of peak, which is ordinary. A tail with WB, exposure, tone
regions, curve, saturation, HSL, grading, rendering and output transform is **nine** such
dispatches: **~30 ms of GPU time for work that is one function per pixel (inferred)**.

On the CPU the same nine stages each allocate and write 82 MB (313 MB at export resolution), so the
tail is nine passes of memory traffic plus nine `vec![0.0; n]` page-fault storms.

**Cost:** the largest single inefficiency left in the tail on both backends, and it grows with
every stage added.

### F2. A tail-slider drag re-uploads the final checkpoint every frame

**Where:** [render_engine.rs:1467](crates/app/src/render_engine.rs:1467) clones the final
checkpoint into `input` (2.5 ms, measured); the tail chain then uploads it through staging
(memcpy 2.3 ms measured, plus the device copy, of the same order). `prefix_offered` is false when
no prefix step ran, so nothing takes the plane.

**Evidence:** "the following tail edits ran with `prefix_ran 0` at ~34 ms" (journal, GPU
geometry). Of that, the round trip's upload half is **~8–10 ms (inferred)** from the `probe_rt`
decomposition. The mechanism to remove it already exists: `PrefixCache::resident` keeps one image
under a `CacheKey`, and `GpuChain::run_with(Input::Resident, ..)` copies GPU-side.

**Cost:** ~8–10 ms of every tail-slider frame at the fit rung, which is the most common gesture.

### F3. During a drag, every GPU prefix step below the dragged one still downloads a checkpoint

**Where:** the prefix loop ([render_engine.rs:1315](crates/app/src/render_engine.rs:1315)) runs
each GPU step with `Download::Float` and inserts the result into the cache; only the **last** step
may hand off. An NR drag on a photo with dehaze runs NR (download 82 MB), dehaze (takes the plane,
download 82 MB), geometry (takes the plane, handoff).

**Evidence:** a float download is 6.3 ms of copy+map plus 6.9 ms of `to_vec` at rung 3200
(M8.3g); the backlog's own note: "during a drag that checkpoint is written, used once, and
invalidated by the very next frame". M7.3b's NR drag is 161 ms at the fit rung.

**Cost:** ~13 ms per intermediate GPU step per drag frame at rung 3200, ~25–30 at 6246 (measured
per download). The checkpoints are worth having **only on settle**, which is exactly when the
warm-behind runs.

### F4. The handoff decides eligibility after running the step, and its fallback is the CPU stage

**Where:** [render_engine.rs:1355](crates/app/src/render_engine.rs:1355) runs the last step to the
plane, then builds the tail, then asks whether the tail is one chain
([render_engine.rs:1632](crates/app/src/render_engine.rs:1632)). If not, the plane is cleared and
the step is **re-run on the CPU** from the checkpoint before it.

**Evidence:** a gating brush mask whose coverage key includes `geometry` misses `BrushCache` on
every frame of a geometric drag, so `GpuMaskStage` declines, the CPU `MaskStage` sits in `pipe`,
and every frame pays GPU geometry (wasted) plus CPU geometry (37 ms) plus a 27 MB-less float tail.
Also, on a handoff that fails, `gpu_prefix_uploads` is added twice ([render_engine.rs:1367](crates/app/src/render_engine.rs:1367) and [render_engine.rs:1416](crates/app/src/render_engine.rs:1416)) — an observable, not a pixel.

**Cost:** on such photos the handoff makes a geometric drag **slower** than no handoff. The spec
(§4) said the tail is built first; the code builds it after.

### F5. The warm-behind runs the whole tail and throws the frame away

**Where:** [render_engine.rs:405](crates/app/src/render_engine.rs:405) calls `render_cached` with
`region: None`; the tail runs, `pack` runs, the histogram runs, and the frame is dropped.

**Evidence:** warm-behind 92–108 ms at rung 3123 (journal). The tail's share of that is the GPU
tail's ~27–34 ms plus the histogram and pack.

**Cost:** ~30 ms of extra latency on every release, for pixels nobody sees.

### F6. The tail is built up to three times per render, and the prefix stages are built with it

**Where:** `build_pipeline_split_tapped` always calls `build_prefix_steps` and the caller discards
the prefix ([lib.rs:1484](crates/pipeline/src/lib.rs:1484)); `prefix_airlight_key` builds every
prefix **stage** at `RenderScale::FULL` to read their keys ([lib.rs:844](crates/pipeline/src/lib.rs:844));
`render_cached_with` builds the tail once for sRGB, again for a proof, again for the grade view.
`GeometryStage::new` and `LensCorrectionStage::new` each build a `Weights` table (6144 kernel
evaluations for Lanczos), `MaskStage::new_capture` clones every mask.

**Cost:** ~1–2 ms per render (inferred; nobody has timed it). Small, but it is on every frame and
the fix is a keys-only builder plus a per-kernel `Weights` cache.

### F7. The CPU spatial primitives have serial passes

**Where:** `blur_cols` is single-threaded by design ([spatial.rs:65](crates/pipeline/src/spatial.rs:65),
"deliberately not parallelized ... revisit only if a profile says so"); `guided_filter`'s
slope/offset loop and its final combine are plain `iter()` ([spatial.rs:278](crates/pipeline/src/spatial.rs:278));
`min_filter`'s vertical scatter writes with stride `w` ([spatial.rs:392](crates/pipeline/src/spatial.rs:392));
NR's and dehaze's blend loops are serial.

**Evidence:** NR's guided filter is 4 box blurs (2 vertical passes each serial), its chroma half 4
more; dehaze's is 6 plus a min filter; clarity's wide blur is 3. A serial vertical running-sum pass
over a 104 MB plane is memory-bound on one core. The comment's worry, re-priming the sum per
**row** band, does not apply to splitting **columns**: each column's sum is independent, so a
column-band split is bit-identical.

**Cost:** these run on the region-scoped 1:1 drag path (CPU-only) and on export. Unmeasured; a
profile of `NoiseReductionStage::apply` at 26 Mpx would settle it.

### F8. The GPU box blur's horizontal pass is uncoalesced

**Where:** `box_wgsl::running` gives each thread a row, so adjacent threads read `w` floats apart
([box_wgsl.rs](crates/gpu-pipeline/src/box_wgsl.rs)); the module's own header records the ~11 ms
floor per three passes and lists coalescing as the cause. Noted as an open item in `CLAUDE.md`.

**Cost:** every running-sum blur in NR, dehaze and clarity pays it. Roughly 2–3 ms per horizontal
pass at 6.8 Mpx (inferred from the 11 ms per three passes), so ~15–20 ms of a GPU dehaze step.

### F9. Masks run their sub-stages over the whole frame whatever the coverage covers

**Where:** `MaskStage::apply` clones and runs every sub-stage over the full image; `GpuMaskStage`
dispatches every sub-stage over `img.pixels()`. The 3.1-era comment "a bounding-box optimisation is
deferred" is still there ([mask.rs](crates/pipeline/src/stages/mask.rs)).

**Cost:** for a brush or radial covering a fraction of the frame, the sub-stage work is wasted in
proportion. For per-pixel sub-stages this vanishes under F1's fix (the blend becomes part of the
fused function); for masked clarity/sharpen a bbox-plus-margin dispatch is the remaining lever.

### F10. Spot removal declines region scoping, which now blocks NR and dehaze at 1:1

**Where:** `SpotRemovalStage` implements neither `input_rect_for` nor `apply_region`; since M7.3b it
sits between dehaze and geometry, so on a spotted photo a scoped plan from NR or dehaze cannot form
(journal, M7.3b caveat).

**Cost:** an NR/dehaze drag at 1:1 on a spotted photo falls back to whole-buffer (~300 ms at 6246).
A region-aware spot stage is ~30 lines: `input_rect_for` grows `out` to include every spot's
destination and source bounding boxes (the ring included), `apply_region` runs the same loop with
an origin offset against `src.full` dimensions.

### F11. The no-keystone CPU geometry path rotates the whole canvas and then crops

**Where:** [geometry/mod.rs:131](crates/pipeline/src/stages/geometry/mod.rs:131): `orient` (a new
buffer), `rotate` (a new full-canvas buffer), `apply_crop` (a third). The fused path already gathers
only the cropped rect and computes the same float sequence (`x0 + x` then un-rotate, both exact in
f32 for integer inputs), and the orientation permutation lands on whole pixels where every kernel's
phase-0 row is `[0, .., 1, .., 0]`.

**Cost:** work proportional to the cropped-away area plus two allocations, on whole-frame CPU
renders (GPU off, export). Not on the drag path any more.

### F12. The resident-input slot and `FINALS_CAP` do not account for the interactive rung

**Where:** `PrefixCache::resident` holds one image; `FINALS_CAP = 2`. `DragQuality::Faster` renders a
gesture one rung below the idle rung, so a drag-then-release alternates two rungs.

**Cost:** one extra 82 MB upload per gesture (the resident misses on the release frame and again on
the next drag's first frame). Minor; two slots keyed by rung would make it zero.

### F13. `MaskCoverageKey::base` clones the whole `Edits` per gating component per frame

**Where:** [render_engine.rs:829](crates/app/src/render_engine.rs:829) `base_id` clones `Edits` and
truncates the mask list, once per component per `get_or_build`, and the key is compared by `==`.

**Cost:** O(masks² × stroke points) copies per frame. Sub-millisecond for realistic photos; a
content hash would make it O(1). Low priority.

### F14. The export tail is CPU-only

**Where:** `batch_export/render.rs:259` `tail.run(img)`. With a GPU prefix step before it, the
last deposit is cleared unused.

**Cost:** a nine-stage per-pixel tail at 26 Mpx is nine 313 MB passes (~300–500 ms, inferred),
against an 8–9 s photo dominated by demosaic. Low value until decode gets faster.

### F15. The ~10 ms app-versus-probe gap is unexplained

**Where:** journal, 2026-09-26: tail submit+wait 36.8 ms in the app against ~26.8 headless with the
window's upload frozen. Sharing a device is declined on that measurement.

**Candidates not yet tested:** the render worker is a plain `std::thread`; on macOS a Metal
command buffer committed from a thread at default QoS competes with the compositor at lower
priority. Raising the worker to `QOS_CLASS_USER_INTERACTIVE` (one `pthread_set_qos_class_self_np`
call at the top of the worker) is a five-line experiment. GPU timestamp queries
(`wgpu::Features::TIMESTAMP_QUERY`, available on Metal) would separate execution time from
scheduling latency and are the instrument every GPU item below wants anyway.

---

## 3. The improved design

### 3.1 The shape

Keep the two halves, the order inside each, the key derivation and the cache tiers: they are
right, measured, and guarded. Change three things about how pixels move:

1. **Per-pixel stages compose into one function.** A `PerPixel` trait (`fn px(&self, [f32;3]) ->
   [f32;3]`, with an `(x, y)` variant for the vignette and for mask coverage) is implemented by every
   per-pixel stage, and its `apply` is defined as `map_pixels(px)`. A `FusedPerPixel` stage holds a
   `Vec<Box<dyn PerPixel>>` and folds them in **one** pass. On the GPU each per-pixel twin exposes a
   WGSL function body plus its bindings, and the fused stage emits one shader whose `main` folds them.
   Bit-identical to the sequential form on the CPU (the same operations in the same order per
   pixel; intermediate buffers never changed a value) and within the existing twin tolerances on the
   GPU. The tail then becomes:

   ```
   [fused A: WB … curve] → local contrast → [fused B: saturation, HSL, grading, per-pixel masks, vignette]
   → sharpening → [fused C: display rendering, look, output transform, quantise]
   ```

   A photo without local contrast or sharpening is **one dispatch**. A mask whose adjustments are
   all per-pixel is `lerp(base, f_mask(base), cov(x, y))` inside the fused function: no clone, no
   separate blend, no extra dispatch, and coverage sampled once per pixel. Only a mask carrying
   clarity or sharpening keeps the clone-and-blend shape, and only for that spatial part.

2. **Pixels stay on the GPU across frames, keyed rather than parked.** Replace the one-shot
   `Slot::Prefix` at the prefix→tail seam with a **keyed resident final checkpoint**: after any
   whole-frame render whose last prefix step ran on the GPU, the output stays resident under its
   final `CacheKey` (the handoff frame deposits it there instead of into the slot). The tail's chain
   runs from `Input::Resident` (a GPU-side copy) whenever the final key matches, which is every
   tail-slider frame. When the prefix ran on the CPU, the final checkpoint is uploaded once on the
   first tail frame and then hits. The mask slot stays a `SharedPlane`; its lifetime really is one
   `apply`.

3. **A drag downloads nothing; the settle materialises.** From the resume point, every GPU-capable
   step runs resident and chained (one submit), the last one lands in the resident final, and the
   tail runs from it. No CPU checkpoint is written during the drag. The warm-behind, which already
   exists for exactly this, runs the steps individually with downloads and fills the cache, and it
   **skips the tail**. The M8.3o "never chain" rule stays true of the settle pass, where the
   checkpoints are wanted, and stops applying to the live frame, where they are provably useless.

Everything else in the review is a local fix inside that shape.

### 3.2 The changes, in the order they should be taken

Each row names the finding, the expected gain with its basis, and the risk. "Fit" means the
author's whole-frame rung-3200 view; figures are per frame unless stated.

| # | change | finding | expected gain | risk / note |
|---|---|---|---|---|
| 0 | **GPU timestamp queries** in `GpuChain::run_with`, printed under `RUSTROOM_DEBUG_XFER` per dispatch | F1, F8, F15 | none directly; it turns every "inferred" below into a number | Metal supports `TIMESTAMP_QUERY`; wgpu's `QuerySet` API. Do this first, per the project's own rule. |
| 1 | **Resident final checkpoint** (§3.1 item 2): `PrefixCache` gains a second keyed resident; `GpuChain::run_rgba8_from(&GpuImage)`; tail chains take `Input::Resident` when the final key matches; the handoff deposits into it | F2 | **~8–10 ms** off every tail-slider frame at fit (inferred from the round-trip decomposition); the tail input clone (2.5 ms measured) goes with it | VRAM: one more image (82 MB at 3200, 313 at 6246). The key makes a stale hit unrepresentable, which is the property the one-shot slot needed a thread stamp and a clear to approximate. |
| 2 | **Warm-behind renders the prefix only** | F5 | ~30 ms less release latency (the tail's measured share) | Needs a `prefix_only` flag on `render_cached_with`; nothing else changes. |
| 3 | **Decide handoff eligibility from the built tail, before the last step runs**; fall back by **downloading the plane** (`GpuChain::download_plane`) rather than re-running the CPU stage | F4 | removes a 37 ms CPU re-run on every drag frame of a photo with a gating brush; fixes the double-counted upload observable | The coverage spec needs `(pw, ph)` before the loop; `output_dims` already predicts it. |
| 4 | **No-download drags**: chain the GPU steps from the resume point into one `GpuChain` on the live frame; settle pass unchanged | F3 | **~13 ms per intermediate GPU step** at fit (measured download cost), ~25–30 at 6246; an NR+dehaze+geometry drag loses two downloads | The airlight is already supplied, so chaining is safe (M8.3o's reason for per-step was the checkpoint, which the settle now provides). |
| 5 | **Per-pixel fusion** (§3.1 item 1), CPU first (bit-identical, goldens must not move), then the WGSL emitter | F1, F9 | **GPU: ~3 ms per per-pixel stage removed** (inferred), so a nine-stage tail drops ~25 ms; a per-pixel mask drops its clone, blend and one dispatch per sub-stage. **CPU (export, GPU off): one pass instead of nine**, no per-stage allocation | The largest change here. The fused shader's text is the tail's *shape*, so the program cache still hits across a drag. Sabotage: reorder two stages inside the fold and the curve-order test must fail. |
| 6 | **In-place per-pixel CPU stages** as a stepping stone to 5: `apply(mut image)` mutates `data_mut()` | F1 | one allocation and one write pass fewer per stage on the CPU; bit-identical | Trivial; obsoleted by 5 for the tail but still right for any per-pixel stage that stays standalone. |
| 7 | **Keys-only prefix builder** and a per-kernel `Weights` cache (`OnceLock` per `Resampler`) | F6 | ~1–2 ms per frame (inferred) | Mechanical; `PrefixKey::new`, `prefix_airlight_key` and the tail builder's discarded prefix all switch to it. |
| 8 | **Parallel `blur_cols` by column bands**; `par_iter` in `guided_filter`, dehaze and NR; a blocked transpose in `min_filter` | F7 | unmeasured; the region-scoped 1:1 drag (34–37 ms for NR+dehaze at ~1 Mpx) and export NR/dehaze at 26 Mpx are the beneficiaries | Column bands are bit-identical; the `par_iter` conversions are too (independent elements). `min_filter`'s change is a memory-order change only. |
| 9 | **Coalesced horizontal box pass**: a workgroup-cooperative prefix sum (load a row segment coalesced into shared memory, scan, write; box = difference of two prefix reads), or transpose → column kernel → transpose | F8 | roughly half of ~15–20 ms per GPU dehaze step, a third of NR's (inferred) | `box_probe` exists to sweep it; keep the naive body below the crossover. |
| 10 | **`SpotRemovalStage::input_rect_for` / `apply_region`** | F10 | restores scoped NR/dehaze drags at 1:1 on spotted photos (whole-buffer ~300 ms → ~35) | Read `src.full` for normalisation, offset by `src.origin`; the ring bbox is the reach. |
| 11 | **Bbox dispatch for masked spatial sub-stages** (brush/radial with a bounded box): run clarity/sharpen only over box + reach | F9 | proportional to the uncovered area | Needs a dispatch rect on `GpuStage::encode` and a crop-run-paste on the CPU. After 5 only spatial sub-stages need it. |
| 12 | **Fused CPU geometry for every non-identity geometry** (keep the slice for crop-only) | F11 | the cropped-away fraction of a whole-frame CPU resample plus two allocations | Must be byte-identical to the three-pass chain; the argument is in F11, the goldens are the check. |
| 13 | **Two resident-input slots keyed by rung; `FINALS_CAP = 3`** | F12 | one 82 MB upload per gesture | Trivial. |
| 14 | **Hash `MaskCoverageKey::base`** | F13 | sub-ms | Only if a profile shows it. |

**What these add up to (inferred, to be replaced by row 0's numbers):** the author's Straighten
drag frame at fit goes from ~40 ms in the app to roughly geometry gather + one fused dispatch +
readback, i.e. **~20 ms if the app-versus-probe gap stays, ~12 without it**; a tail-slider frame
from ~34 to **~15**; an NR drag from 161 to **~110**, most of which is NR's own GPU work and F8's
blurs.

### 3.3 The design-level options, which are the author's call

- **One resample for the whole geometric chain** (backlog's "three passes to one"). Apply
  vignetting correction as a separate per-pixel gain stage first (it is a gain, not a resample),
  then `NR → dehaze → spots → fused(lens distortion + TCA + orientation + keystone + straighten +
  crop)`. Lens distortion enters the gather through the `DisplacementGrid` the GPU twin already
  uses (bilinear lookup, 0.002 px on the XF16-80 at step 8). Gains: no second resample's softening
  (M7.3b measured the second pass at ~20% of acutance; the first bilinear pass at 48%), one gather
  instead of two, and **spots anchored in sensor space**, which closes the "toggling lens correction
  shifts a spot" caveat. Costs: three gathers per pixel for TCA (the lens twin does this already);
  `fill_row`'s exact lensfun evaluation is replaced by the grid on the CPU too, so the **lens
  goldens move** (0.8 levels mean on a real frame, up to 28 on the synthetic chart at step 8), which
  is a photographic judgement rather than a regression. Dehaze still sees corrected vignetting.
  NR moves ahead of the distortion correction, which M7.3b found costs 92 ms per NR drag for no
  gain, so NR would then be the *first* step and its checkpoint the most stable one; the drag
  numbers need re-measuring under this order before deciding.
- **Masks' scene sliders at the global slider's position** (the milestone split off from M7.3b).
  Under fusion this is natural: a mask's exposure becomes a per-pixel parameter offset
  `ev(x, y) = ev_global + Σ cov_i(x, y) · ev_i` inside the fused function, which is Lightroom's
  model and is cheaper than output blending, not dearer. It changes the picture, so it is a spec
  of its own; fusion should be built with a per-pixel parameter path so this does not need a second
  design later.
- **GPU twins for the masked capture stages**, so a straighten on a photo with masked NR does not
  re-run whole-buffer CPU NR every frame. The pieces exist (`GpuMaskStage` shape, NR/dehaze
  twins, scratch budgeting). Worth building only when such masks appear in use; the catalogue holds
  none.
- **The export tail on the GPU** (F14). Decode-bound today; revisit after a faster demosaic.

### 3.4 What stays as it is, and why

- The prefix order and the prefix/tail split point: measured, and every alternative on record was
  measured worse or retracted.
- The cache tiers, deepest-first eviction, the derived keys, the key-from-projection rule for
  masked captures, the airlight memo: all correct, all guarded, all cheap.
- The region-scoped path being CPU-only: at ~1–2 Mpx the round trip (7 ms at 1.7 Mpx measured) is
  a third of the work; a GPU scoped chain would save ~10 ms per step at best. F7's parallelism is
  the better lever there.
- Spot removal on the CPU (measured and closed). F10 is region scoping, not a port.
- Two wgpu devices (declined on a measurement). F15 names the untested candidates instead.
- The `SharedPlane` for the mask slot; only the prefix seam changes to a keyed resident.

---

## 4. What I did not measure, and what would settle it

- Per-dispatch GPU time (row 0). Everything marked inferred about the tail rests on subtracting
  the gather from a 27 ms submit+wait.
- The CPU cost of `build_prefix_steps` and `Weights::new` per frame (F6).
- The serial share of NR and dehaze on the CPU at 26 Mpx (F7).
- Whether the render worker's thread QoS explains the app gap (F15).
- Fusion's actual GPU saving: emit the fused shader for `display_rendering + output_transform +
  quantise` alone and time it against the three dispatches, before building the general emitter.

Two correctness notes found on the way, both benign today: the double-counted
`gpu_prefix_uploads` on a failed handoff (F4), and the handoff spec's "tail built first" not being
what the code does (F4). No pixel-changing defect was found in the paths read.
