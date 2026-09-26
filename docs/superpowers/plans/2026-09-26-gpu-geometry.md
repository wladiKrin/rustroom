# GPU geometry with a resident input — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:executing-plans. Checkbox steps.

**Goal:** a Straighten drag frame at rung 3200 in ~20–25 ms (from ~63) on DSCF6263.

**Architecture:** see `docs/superpowers/specs/2026-09-26-gpu-geometry-design.md`. Four layers, each
measured before the next: a size-changing chain, the geometry twin (plain), the resident input, and
the no-download handoff.

**Gate:** `cargo fmt --check` · `CARGO_TARGET_DIR=target/clippy cargo clippy --workspace --all-targets -- -D warnings` · `cargo test --workspace` (filter by name, never `-p`). **Measure:** `RUSTROOM_DEBUG_RENDER=1 RUSTROOM_DEBUG_XFER=1 ./target/release/examples/probe_straighten_drag ~/Pictures/fuji/DSCF6263.RAF 3200`.

---

### Task 1: Size-changing GPU stages (`crates/gpu-pipeline/src/{chain.rs,image.rs}`)
- [ ] Test first (chain.rs tests): a toy `HalveWidth` stage (`output_dims` = `(w/2, h)`, encode
  copies the left half's pixels to the front of the buffer). Assert that `GpuChain::apply` returns a
  `(w/2, h)` image equal to the CPU twin's, that `run_rgba8` returns `w/2*h*4` bytes, and that a
  deposited plane is taken at `(w/2, h)` and refused at `(w, h)`.
- [ ] `GpuStage::output_dims(&self, w, h) -> (usize, usize)` defaulting to `(w, h)`.
  `GpuImage::relabel(&mut self, w, h)`, asserting `w*h <= capacity`, and keeping a `capacity`
  field. In `run_with`, relabel `img` after each stage's `encode`. Size `packed`, the readback,
  `deposit` and the returned dimensions from the **final** label. `apply` builds its `ImageBuf` at
  the final dims (`GpuChain::output_dims` folds the stages).
- [ ] Gate, commit.

### Task 2: `GpuGeometryStage` (`crates/gpu-pipeline/src/geometry.rs`, new)
- [ ] Tests first: GPU vs CPU `GeometryStage` (Catmull-Rom and Bilinear) on a structured chart,
  for identity, crop only, `Cw90`+mirror, `Cw270`, angle ±3°, a keystone, and all combined. Assert
  the fixture moves pixels (CPU output ≠ input crop), then a worst difference within the lens
  twin's bound (8-bit levels ≤ 1 after `to_rgb8`). Assert the output dims equal
  `GeometryStage::output_dims`.
- [ ] The stage: built from `(&Geometry, Perspective, Resampler, &ImageBuf)` (input dims known),
  with `cpu = GeometryStage::new(..).with_perspective(..)`, `output_dims` delegating to the CPU
  stage, and 3 scratch planes. The shader has `seed` (input pixels → planes), then `gather` over the
  output pixels, with the map transcribed from `FusedMap::source_of`. Pass crop origin, the rotation
  (flag + sin/cos + oriented centre), the orientation (quarter + mirror), `w1`/`h1`, source centre
  and the homography's **inverse matrix** (read from `Homography`; add
  `pub fn inverse_matrix(&self) -> [[f32; 3]; 3]` in `pipeline::perspective`), plus
  `sample`/`axis` exactly as the lens twin's but three-channel. Weights uploaded from `Weights::lut`.
- [ ] Register `"geometry"` in `accelerate_prefix_stage`, and drop it from
  `every_other_prefix_stage_is_left_alone`. Measure `cpu_cost` with `probe_gpu_run_cost`
  (add a `geometry` row).
- [ ] Gate, commit. **Measure the drag** (expected ≈ the plain twin, ~CPU parity).

### Task 3: Resident input (`gpu-pipeline` + `crates/app/src/{prefix_cache.rs,render_engine.rs}`)
- [ ] gpu-pipeline: `GpuImage::upload(ctx, &ImageBuf) -> Option<GpuImage>` (staging copy, submit),
  and `GpuChain::apply_from(&self, input: &GpuImage) -> Option<ImageBuf>`, which copies
  `input → img` with `copy_buffer_to_buffer` and counts **no upload**.
- [ ] app: `PrefixCache::resident: Option<(CacheKey, Arc<GpuImage>)>`. In `render_cached`'s prefix
  loop, for the **first** step run when it is GPU-accelerated: on a hit use `apply_from(resident)`;
  on a miss upload `img` into a new resident under `key_at(i)`, then `apply_from`.
- [ ] Tests (render_engine tests, GPU-gated): two consecutive Straighten frames. The second
  performs zero uploads on its geometry chain (expose the last prefix chain's transfer counts on
  `Ran`), and its frame equals a cold `render_once` within the twins' tolerance.
- [ ] Gate, commit, measure.

### Task 4: Handoff without a download
- [ ] gpu-pipeline: `GpuChain::run_to_plane(&self, input: &GpuImage) -> Option<(usize, usize)>`:
  no readback, no map wait, deposits in `offer_output`. And
  `GpuChain::run_rgba8_from_plane(&self, w, h) -> Option<Vec<u8>>`, which requires the plane
  (`None` if absent).
- [ ] app `render_cached`: predict the prefix output dims (fold `output_dims`). **Before** the
  prefix loop, when region is None, not `float_needed`, no grade view and GPU tail on: build the
  tail against the predicted dims. If its chain covers every tail stage (`pipe` has no CPU stages)
  **and** the last prefix step is GPU-accelerated and is the first step run, use `run_to_plane`
  then `run_rgba8_from_plane`. Record `ran.handoff = true` and insert **no** final checkpoint. Any
  `None` falls back to the CPU step plus the ordinary path.
- [ ] Worker: warm-behind also when `ran.handoff`, with a request flag that disables handoff for
  the warm render.
- [ ] Tests: a handoff frame downloads zero float bytes and caches no final. After a quiet channel
  the final checkpoint exists (`warmed` counter). The handoff frame's bytes equal a non-handoff
  frame's bytes exactly (same GPU chain, same input).
- [ ] Gate, commit, **measure: acceptance ~20–25 ms**.

### Task 5: Docs, GUI
- [ ] Journal entry with every measured row, the backlog's GPU-geometry decline entry reopened
  and closed, CLAUDE.md State and Next action. Remove nothing measured.
- [ ] Launch the GUI with `RUSTROOM_DEBUG_RENDER=1` for the author to drag.
