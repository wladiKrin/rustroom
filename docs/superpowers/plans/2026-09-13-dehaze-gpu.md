# M8.3o — dehaze on the GPU, and a seam for the prefix

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:executing-plans.
> ⚠⚠ **No `Co-Authored-By` trailer. NEVER push.** Branch `m8.3o-dehaze-gpu`, already created.
> ⚠ Read every gate's exit code from **its own command**, never through a pipe, never after `;`.

**Goal:** Dehaze runs as compute shaders, behind a new seam that substitutes a **prefix** step —
the first acceleration outside the tail.

**Architecture:** `accelerate_prefix` mirrors `accelerate` one level up: substitute a `PrefixStep`'s
`stage` by name, leave its `key` alone, gate per step on the existing `TRANSFER_COST`. `GpuDehazeStage`
encodes its passes into one compute pass; the airlight stays on the CPU and arrives as uniforms.

**Spec:** `docs/superpowers/specs/2026-09-13-dehaze-gpu-design.md`

---

## Task 1: `min_wgsl` — the sliding-window minimum

**Files:** create `crates/gpu-pipeline/src/min_wgsl.rs`; modify `lib.rs`, `box_probe.rs`.

- [ ] **Step 1: The shader**

`pub(crate) fn min_pass(name_h, name_v, src, dst, tmp, r) -> String`, the `box_pass` shape.

⚠⚠ **Naive only — there is no running form.** A minimum is **not invertible**, so the trick that
made Clarity affordable has no analogue. Affordable here only because `PATCH_RADIUS_FULL = 20`
scales to **r=10**, measured at 4.49 ms where Clarity's r=40 cost 46.9.

⚠⚠ **The window CLAMPS, the samples do not** — `box_blur`'s rule, not `gaussian_blur`'s. That is
exactly why `spatial::min_1d` uses a monotonic deque rather than van Herk / Gil-Werman, whose block
decomposition cannot express a shrinking window. Transliterate the deque's *semantics*, not its
implementation: per output, `min` over `[i-r, i+r] ∩ [0, n)`.

- [ ] **Step 2: Tests, in `box_probe`'s module (it already has the plumbing)**

Extend `BoxProbe` with a `min` mode, then:

```
the_min_shader_matches_min_filter()            // r = 3, 10, 20 against `spatial::min_filter`
a_constant_plane_survives_the_min_shader()     // ⚠ the ONLY test separating the two border rules
```

⚠ A minimum needs **no** rounding slack — it selects an input sample rather than computing one — so
assert **bit-identity**, which is stronger than anything M8.3n could claim.

- [ ] **Step 3: Sabotage** — divide the window as if it were a box (`2r+1` fixed bounds, ignoring
the image edge) and watch the constant-plane test fail. Restore from a `cp` backup, never
`git checkout`.

- [ ] **Step 4: Commit.**

---

## Task 2: `GpuDehazeStage`

**Files:** create `crates/gpu-pipeline/src/dehaze.rs`; modify `lib.rs`; make `pipeline`'s constants
`pub`.

- [ ] **Step 1: Widen what must be substituted**

`OMEGA`, `T_MIN`, `A_MIN`, `AIRLIGHT_TOLERANCE`, `PATCH_RADIUS_FULL`, `REFINE_RADIUS_FULL`,
`REFINE_EPS` become `pub`, plus `airlight` (already `pub`) and `scaled_radius`. Each gets the note
the local-contrast constants carry: **substituted, never transcribed.**

- [ ] **Step 2: The passes**

Planes: `p0` guide (luma), `p1` normalized→dark, `p2` coarse→refined, `p3` aux, `p4` tmp.

| pass | does |
|---|---|
| `seed` | `p0 = luma(img)` via `luma_wgsl`; `p1 = min_c(img_c / A_c)` |
| `min_h`/`min_v` | the dark channel at r = `scaled_radius(PATCH_RADIUS_FULL)`, `p1 ↔ p4` |
| `coarse` | `p2 = tolerant(1 - OMEGA*p1, dist(img, A), TOL)`; ⚠ `dist` is the **largest per-channel** distance, recomputed from `img` rather than stored |
| guided | the **cross**-guided filter: `p` = `p2`, `guide` = `p0`, r = `scaled_radius(REFINE_RADIUS_FULL)` — **six** box blurs, M8.3n's self-guided shortcut does not apply |
| `apply` | `t_k = 1 - mag*(1-t)`; remove → `(I-A)/max(t_k, T_MIN) + A`, add → `I*t_k + A*(1-t_k)`; `max(.., 0)` |

⚠ The guided filter needs `mean_p` and `mean_cross` as well as `mean_guide`/`mean_guide_sq`, so it
is six box blurs and two extra planes beyond M8.3n's five. Count them honestly in `scratch_planes`.

⚠⚠ **The airlight is computed CPU-side in `new` and passed as three uniforms.** It already runs on
a fixed 256 px downscale — deliberately, so the estimate is render-scale-independent — which makes
it negligible *and* makes both paths identical **by construction** rather than by comparison.

- [ ] **Step 3: Tests**

```
the_fixture_is_one_this_stage_actually_moves()   // first, or everything below is vacuous
both_signs_match_the_cpu()                       // bounded in levels; divides + multiply-adds
the_airlight_is_the_cpus_own()                   // a second estimate is a different photograph
no_sample_comes_back_negative()                  // ⚠ RAW FLOATS — `compare` cannot see a clamp at 0
the_stage_is_named_like_the_cpu_one()
```

⚠ The fixture must carry **real haze structure**: the dark-channel prior must hold (periodic dark
cells on a pitch under the patch width), the transmission must reach near zero somewhere or the
airlight is unobservable, and the guided reach is **twice its radius**. The CPU stage's own tests
record all three and why three earlier fixtures were wrong.

- [ ] **Step 4: Sabotages** — drop the `T_MIN` floor; drop the zero clamp (expect **only** the raw-float
test); use `mean_guide` for `mean_p` (the self-guided shortcut, which is **wrong** here); transcribe
`OMEGA` as 1.0. Confirm each **compiled** before believing a pass.

- [ ] **Step 5: Commit** (Tasks 2 and 3 may need to land together — see Task 3's note).

---

## Task 3: `accelerate_prefix`, the seam

**Files:** create/modify `crates/gpu-pipeline/src/accelerate.rs`; modify `crates/app/src/render_engine.rs`.

- [ ] **Step 1: The function**

```rust
pub fn accelerate_prefix(
    steps: Vec<PrefixStep>, edits: &Edits, scale: RenderScale, ctx: &Arc<GpuContext>,
) -> Vec<PrefixStep>
```

Substitute by name from a `PREFIX_REPLACEABLE` table; gate each step on `cpu_cost() > TRANSFER_COST`;
wrap the accepted stage in a one-stage `GpuChain`.

⚠⚠ **`key` is left alone, and that is the whole safety argument.** `PrefixStepKey` is derived from
the **edits**, not the stage, so a checkpoint written by the CPU path is a legitimate hit for the
GPU path and the reverse. ⚠ `gpu_tail` must therefore **not** enter any key — the rule the
resampler follows and HSL deliberately does not.

⚠ **Per step, never chained.** Chaining loses the post-NR checkpoint, which is what lets a Dehaze
drag resume after NR; the spec prices both and per-step wins the case that matters.

- [ ] **Step 2: Wire it in** at `render_engine.rs:1080`, beside the existing `accelerate` call and
behind the same setting.

⚠ `accelerate_prefix` is a `pub fn` with no caller until this step, so **Tasks 2 and 3 land together**
if clippy objects — the lib-crate dead-code trap this file records repeatedly.

- [ ] **Step 3: Tests**

```
a_dehaze_prefix_step_becomes_a_chain()
a_cheap_prefix_step_is_left_alone()             // ⚠ hand-build it; nothing cheap is replaceable yet
the_substitution_leaves_every_key_untouched()   // ⚠⚠ the cache-compatibility guard
a_checkpoint_written_by_either_path_is_a_hit_for_the_other()
the_prefix_renders_the_same_picture()
```

- [ ] **Step 4: Sabotage** — mutate a step's `key` on substitution and watch the cache tests fail.

- [ ] **Step 5: Commit.**

---

## Task 4: Measure, gate, document

- [ ] **Step 1: Re-measure the real thing**, not the components — this project has over-predicted a
whole frame by 2× (M8.3g) and mis-attributed by 8× (M8.3f) from component sums. Add a Dehaze row to
`bench_gpu_tail` **and** re-run `bench_preview`'s native block.

- [ ] **Step 2: Re-scan the catalogue** before quoting any figure — a caveat measured against a
catalogue is invalidated by editing it, and "confirm by use" is an instruction to edit it.

- [ ] **Step 3: The full gate, each exit code from its own command**, asserting the test count moved
by **exactly** the number added and that **63 goldens ran and none moved**.

- [ ] **Step 4: Update `CLAUDE.md`** — the measurement, the caveats, and the honest headline that
this is **~1.8× and roughly half of it is transfer**.

- [ ] **Step 5: Launch the GUI** and say what to look at.
