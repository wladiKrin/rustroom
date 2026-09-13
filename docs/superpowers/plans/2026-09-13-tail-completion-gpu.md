# M8.3p — the vignette and the creative look on the GPU

> **Conventions, not negotiable:** commit messages carry **no `Co-Authored-By` trailer**;
> **never push**; work on a branch. Read each exit code from its **own** command, never through a
> pipe. After any scripted edit, **assert the new text is present**. After any sabotage, confirm
> the **named** test failed — a compile error reads exactly like a pass.

**Goal:** port `vignette` and `creative_look`, so an unmasked tail is always exactly one chain.

**Spec:** `docs/superpowers/specs/2026-09-13-tail-completion-gpu-design.md`

---

### Task 1: `pipeline::vignette_params` — one derivation, two callers

**Files:** `crates/pipeline/src/stages/vignette.rs`, `crates/pipeline/src/lib.rs`

- [ ] Add `pub struct VignetteParams { wu, wv, expo, elliptical, e0, e1, k }` and
      `pub fn vignette_params(p: &Vignette, fw: usize, fh: usize) -> VignetteParams`, moving the
      `Shape::new` / `lerp` / `e0`/`e1`/`k` arithmetic into it verbatim.
- [ ] Rewrite `VignetteStage::apply` to call it, so the CPU path **is** the reference.
- [ ] Re-export from `lib.rs`.
- [ ] **Acceptance: all 63 goldens byte-identical.** That is the proof the extraction moved
      nothing. Not "still green" — byte-identical.
- [ ] Commit.

### Task 2: `Cube` accessors

**Files:** `crates/pipeline/src/lut.rs`

- [ ] `pub fn entries(&self) -> &[[f32; 3]]`, `pub fn dmin(&self) -> [f32; 3]`,
      `pub fn dmax(&self) -> [f32; 3]`.
- [ ] ⚠ **Land with their first caller (Task 5)** — a `pub fn` with no caller in a lib is fine,
      but these are trivially small; fold into Task 5's commit if clippy objects.

### Task 3: `accelerate` gains `Option<Region>`

**Files:** `crates/gpu-pipeline/src/accelerate.rs`, `crates/app/src/render_engine.rs`

- [ ] Add `region: Option<Region>` after `scale`, threaded into `Builder`.
- [ ] ⚠ **Not** into `RenderSpec` — `build_pipeline_split_in_region` takes `region` beside
      `render_spec`, so folding it in gives the region two homes in one call.
- [ ] Update the one production call site and every test.
- [ ] **Acceptance: the workspace builds and the test count is UNCHANGED.** A signature change
      that moves a test count has moved behaviour.
- [ ] Commit.

### Task 4: `GpuVignetteStage`

**Files:** `crates/gpu-pipeline/src/vignette.rs` (new)

- [ ] Uniforms `x0, y0, elliptical` (u32) + `hw, hh, wu, wv, expo, e0, e1, k` (f32).
- [ ] `encode` calls `pipeline::vignette_params(p, fw, fh)`, where `(fw, fh)` is the region's
      frame or the image's own dimensions when there is no region.
- [ ] Shader per §3.3 — smoothstep written out, not WGSL's built-in.
- [ ] Tests:
  - `the_stage_really_vignettes_rather_than_copying` (fixture check first)
  - `every_shape_matches_the_cpu` — **including negative roundness**, or the `pow` branch is never
    reached
  - `a_scene_referred_highlight_is_not_dragged_down` — **raw floats**, the only test that can see
    the lift's `max(0.0)`
  - `a_region_is_shaded_as_that_part_of_the_whole`
- [ ] **Sabotages:** drop the lift's `max`; use the image dims instead of the frame's; force
      `elliptical` always.
- [ ] Commit.

### Task 5: `GpuCreativeLookStage`

**Files:** `crates/gpu-pipeline/src/creative_look.rs` (new), `crates/pipeline/src/lut.rs`

- [ ] Storage buffer = the cube's own entries, flat. Uniforms: `size`, `dmin[3]`, `dmax[3]`,
      `blend`, `shaped` tag.
- [ ] Matrices **substituted** from `color::spaces::{PROPHOTO_TO_SRGB, SRGB_TO_PROPHOTO}`.
- [ ] ⚠ Domain remap **then** clamp, never the reverse.
- [ ] ⚠ `blend == 0.0` returns the pixel untouched — a correctness guard, not a speed path.
- [ ] Tests:
  - `an_identity_cube_returns_the_input_in_every_space` — the behavioural guard on the
    transcribed transfer constants
  - `a_real_look_matches_the_cpu`
  - `strength_zero_is_bit_exactly_the_input`
  - `a_declared_domain_is_honoured` — a cube with `LUT_3D_INPUT_RANGE`
  - `a_linear_look_takes_no_shaper`
- [ ] **Sabotages:** clamp before the remap; shape a `Linear` look; transcribe a wrong matrix row.
- [ ] Commit.

### Task 6: `REPLACEABLE`, the invariant, and the cost gate

**Files:** `crates/gpu-pipeline/src/accelerate.rs`

- [ ] Two rows, in **tail order**: `vignette` after `sharpening`, `creative_look` after
      `display_rendering` and before `output_transform`.
- [ ] ⚠ `vignette`'s builder is the second to need something beyond `Edits` + `scale` — it takes
      the region.
- [ ] `every_tail_stage_is_replaceable_or_a_mask` over `all_edits()`.
- [ ] `an_unmasked_tail_is_exactly_one_chain`.
- [ ] **Sabotage:** remove a row and watch the invariant test fail.
- [ ] Commit.

### Task 7: Measure, then the gate

- [ ] `probe_stages` at 3200×2136, three runs → `cpu_cost` at the **bottom** of each band.
- [ ] **Measure the cube upload** at 33³ and 65³. Record the number whatever it says.
- [ ] Add `bench_gpu_tail` rows: `vignette -60`, `a creative look`, `the whole tail with both`.
- [ ] ⚠ **Re-measure the real thing, not the components.** This crate has over-predicted a whole
      frame three times from component sums (M8.3g ~2×, M8.3f 8× in attribution, M8.3o ~18%).
- [ ] **The full gate, each exit code from its own command**, asserting the test count moved by
      **exactly** the number added and that 63 goldens ran and none moved.
- [ ] Update `CLAUDE.md` and launch the GUI.
