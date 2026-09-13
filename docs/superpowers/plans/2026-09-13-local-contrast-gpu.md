# M8.3n — local contrast on the GPU

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans.
> ⚠⚠ **Commit messages carry NO `Co-Authored-By` trailer. NEVER push.** Branch is
> `m8.3n-local-contrast-gpu`, already created.
> ⚠ Read every gate's exit code **from its own command**, never through a pipe and never
> sequenced with `;` after the commit.

**Goal:** Texture and Clarity run as compute shaders, so a mask carries 10 of 10 sub-stages on the
GPU and nothing left in the tail can split a mask's chain.

**Architecture:** One `GpuStage` encoding up to 17 dispatches into one compute pass over 5 shared
scratch planes — the `sharpen.rs` shape. Two box-blur shader bodies chosen by radius (naive O(r)
below 7, running-sum O(1) at or above), emitted into a **generated** shader so the radius is a
compile-time constant.

**Tech Stack:** wgpu 29 / WGSL, `pipeline::spatial`, rayon.

**Spec:** `docs/superpowers/specs/2026-09-13-local-contrast-gpu-design.md`

---

## Task 1: Slim the self-guided `guided_filter`

**Why first:** a `cpu_cost` set against code with a known 30% of fat in it is the M8.3h staleness
failure. The costs in Task 6 must be measured *after* this.

**Files:**
- Modify: `crates/pipeline/src/spatial.rs`
- Test: same file's `#[cfg(test)]` module

- [ ] **Step 1: Freeze the current implementation as a test-only reference**

In `spatial.rs`'s test module, add a verbatim copy of today's `guided_filter` body named
`guided_filter_reference`, with a doc comment saying it must **never** be updated to match — if
the two disagree, that is the finding. (The M8.3i precedent: `rasterize_in`'s frozen copy.)

- [ ] **Step 2: Write the failing bit-identity test**

```rust
#[test]
fn the_self_guided_shortcut_is_bit_identical() {
    // ⚠ Needs real structure at the filter's own scale, or every implementation agrees: a flat
    // plane has zero variance everywhere and `slope` is 0 whatever the statistics say.
    let (w, h) = (61usize, 47);
    let g: Vec<f32> = (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            let edge = if x > w / 2 { 0.7 } else { 0.15 };
            edge + 0.03 * ((x * 7 + y * 13) % 11) as f32 / 11.0
        })
        .collect();
    for r in [1usize, 2, 5] {
        let want = guided_filter_reference(&g, &g, w, h, r, 2e-4);
        let got = guided_filter(&g, &g, w, h, r, 2e-4);
        assert_eq!(want, got, "the self-guided shortcut must be bit-identical at r={r}");
    }
}
```

Run: `cargo test -p pipeline -- the_self_guided_shortcut_is_bit_identical`
Expected: PASS already (nothing changed yet) — this is the guard, not a red test.

- [ ] **Step 3: Take the shortcut**

In `guided_filter`, detect the self-guided case by pointer identity of the slices and skip the two
duplicate statistic blurs:

```rust
    // ⚠⚠ **When `p == guide`, `mean_p` IS `mean_guide` and `cross` IS `guide_sq`** — so the
    // general form computes four statistic box blurs where two suffice. Detected by pointer
    // identity rather than by comparing contents, which would cost a full pass to save two.
    //
    // Measured at 3200x2136: 40.6 -> 28.3 ms, **bit-identical**, because the arithmetic that
    // survives is the same arithmetic in the same order. Noise reduction is self-guided for the
    // same reason and gains the same 30%; dehaze uses the general form and is unaffected.
    let self_guided = std::ptr::eq(p.as_ptr(), guide.as_ptr()) && p.len() == guide.len();

    let mean_guide = box_blur(guide, w, h, r);
    let guide_sq: Vec<f32> = guide.iter().map(|v| v * v).collect();
    let mean_guide_sq = box_blur(&guide_sq, w, h, r);
    let (mean_p, mean_cross) = if self_guided {
        (mean_guide.clone(), mean_guide_sq.clone())
    } else {
        let cross: Vec<f32> = guide.iter().zip(p).map(|(g, v)| g * v).collect();
        (box_blur(p, w, h, r), box_blur(&cross, w, h, r))
    };
```

⚠ Keep the rest of the function **exactly** as it is, `var`/`cov` clamping included — the
covariance is deliberately unclamped and the variance is not, and collapsing them costs 4.7e-6 of
divergence.

- [ ] **Step 4: Verify**

Run: `cargo test -p pipeline -- the_self_guided_shortcut_is_bit_identical guided`
Expected: PASS.

Run: `cargo test -p pipeline --test golden 2>&1 | tail -3`
Expected: all pass; the three NR goldens and the two dehaze goldens must not move.

- [ ] **Step 5: Sabotage**

Change `self_guided` to `true` unconditionally and confirm the dehaze goldens fail (dehaze uses
the general form with `p != guide`). Restore from a `cp` backup — ⚠ **never `git checkout`**, which
has destroyed uncommitted work in this repo twice.

- [ ] **Step 6: Commit**

```bash
git add -A && git commit -m "perf(pipeline): the self-guided guided filter does half the blurs

When p == guide, mean_p IS mean_guide and cross IS guide_sq, so the general
form computed four statistic box blurs where two suffice. Bit-identical and
30% faster (40.6 -> 28.3 ms at 3200x2136). Noise reduction is self-guided and
gains the same; dehaze uses the general form and is unaffected.

Lands before the GPU port because setting a cpu_cost against code with known
fat in it is the M8.3h staleness failure."
```

---

## Task 2: Widen the constants `local_contrast` keeps private

**Files:** Modify `crates/pipeline/src/stages/local_contrast.rs`, `crates/pipeline/src/lib.rs`

- [ ] **Step 1: Make the substituted constants and helpers `pub`**

`MID_GRAY`, `LUMA_EPS`, `GAIN_MAX`, `CLARITY_HALO_KNEE`, `CLARITY_HALO_FLOOR`,
`TEXTURE_GUIDED_EPS` become `pub`; `texture_guided_radius` becomes `pub`. Re-export each from
`lib.rs` under a `LOCAL_CONTRAST_` prefix where the bare name would be ambiguous:

```rust
pub use stages::local_contrast::{
    texture_guided_radius, CLARITY_HALO_FLOOR, CLARITY_HALO_KNEE, CLARITY_SIGMA,
    LOCAL_CONTRAST_GAIN_MAX, LOCAL_CONTRAST_LUMA_EPS, MID_GRAY, TEXTURE_GUIDED_EPS, TEXTURE_SIGMA,
};
```

Rename the two ambiguous ones at their definitions (`LUMA_EPS` -> `LOCAL_CONTRAST_LUMA_EPS`,
`GAIN_MAX` -> `LOCAL_CONTRAST_GAIN_MAX`), because `sharpening` has constants of the same names and
a reader hitting `pipeline::LUMA_EPS` must not have to guess which stage it belongs to.

Add to each: `/// ⚠ `pub` so `gpu-pipeline` can SUBSTITUTE it into WGSL rather than transcribe it.
A substituted constant cannot disagree; a transcribed one is merely checked for agreeing.`

Also `pub const WIDE_PASSES: usize = 3;` in `spatial.rs`, re-exported — the GPU stage must iterate
the same number of box passes.

- [ ] **Step 2: Verify and commit**

Run: `cargo build --workspace` — expected: clean.
Commit: `refactor(pipeline): expose local contrast's constants for substitution`

---

## Task 3: The box-blur shaders and the swept threshold

**Files:**
- Create: `crates/gpu-pipeline/src/box_wgsl.rs`
- Modify: `crates/gpu-pipeline/src/lib.rs`, `crates/gpu-pipeline/src/box_probe.rs`

- [ ] **Step 1: Write `box_wgsl.rs`**

Exports:
- `pub(crate) const RUNNING_SUM_MIN_RADIUS: usize = 7;` with the sweep table in its doc comment.
- `pub(crate) fn use_running_sum(r: usize) -> bool { r >= RUNNING_SUM_MIN_RADIUS }`
- `pub(crate) fn box_pass(name_h: &str, name_v: &str, src: &str, dst: &str, tmp: &str, r: usize)
  -> String` — emits the two entry points, choosing the body by `use_running_sum(r)` and
  substituting `r` as a literal.

Both bodies implement `box_blur`'s border rule: **the window shrinks and the sum is divided by the
count actually inside it** — not edge replication, which is `gaussian_blur`'s rule and gives a
different answer at a border.

The running body is a transliteration of `blur_rows`/`blur_cols`: prime by summing
`[0, min(r+1, w))` ascending, then one add and one subtract per step, dividing by the live count.

- [ ] **Step 2: Point `box_probe.rs` at the production shaders**

Rewrite `BoxProbe` to build its shader from `box_pass`, so the probe times and tests **the code
that ships** rather than a copy. Keep the `#[ignore]`d timing probe.

- [ ] **Step 3: Write the equivalence tests**

```rust
/// ⚠⚠ The running-sum shader transliterates `blur_rows`/`blur_cols` exactly — same priming, same
/// slide, same divisor — so it must be **bit-identical**. Nothing else in this crate can claim
/// that since M8.3m found FMA contraction, because nothing else is free of multiply-adds.
#[test]
fn the_running_sum_shader_is_bit_identical_to_box_blur() { /* r = 8, 16, 40 */ }

/// ⚠ The naive shader re-sums the window, which is a **different** accumulation from a running
/// sum carried across the row — and the more accurate of the two, since the CPU's is the one that
/// drifts. Bounded in 8-bit levels, as `saturation`'s is.
#[test]
fn the_naive_shader_agrees_with_box_blur_within_a_hundredth_of_a_level() { /* r = 1, 2, 4 */ }

/// ⚠⚠ **A constant plane must stay EXACTLY constant.** That is the property separating the
/// shrinking window from edge replication, and no photographic fixture can see it — both rules
/// give plausible numbers at a border.
#[test]
fn a_constant_plane_survives_both_shaders_exactly() {}

/// ⚠ The threshold as a DECISION, so "which shader does Clarity get" is pinned rather than
/// believed. Swept between r=6 and r=8; see `RUNNING_SUM_MIN_RADIUS`.
#[test]
fn each_real_radius_selects_the_shader_the_sweep_chose() {
    assert!(!use_running_sum(1), "Texture at fit");
    assert!(!use_running_sum(2), "Texture at native");
    assert!(use_running_sum(10), "Clarity at fit");
    assert!(use_running_sum(40), "Clarity at native");
}
```

- [ ] **Step 4: Verify, sabotage, commit**

Run: `cargo test --release -p gpu-pipeline -- box`
Sabotage: make the running body divide by `2*r+1` instead of the live count — expect the
constant-plane test **and** the bit-identity test to fail. Restore from a `cp` backup.

Commit: `feat(gpu): a box blur in two shapes, chosen by radius`

---

## Task 4: `GpuLocalContrastStage`

**Files:**
- Create: `crates/gpu-pipeline/src/local_contrast.rs`
- Modify: `crates/gpu-pipeline/src/lib.rs`

- [ ] **Step 1: The generated shader**

`fn shader(tex_r, cla_r, guided: bool, kernel_len) -> String`, concatenating `luma_wgsl()`, the box
passes and the stage's own kernels. Bindings mirror `sharpen.rs`:

| binding | contents |
|---|---|
| 0 | `img` (rw) |
| 1 | `params` (uniform) |
| 2 | `kern` (ro) — the Gaussian, uploaded from `spatial::gaussian_kernel` |
| 3..7 | `p0`..`p4` (rw) — the five scratch planes |

Plane roles, exactly as the spec's table: `p0` = luma, `p1` = delta, `p2` = work, `p3` = aux,
`p4` = tmp.

Entry points:

| pass | does |
|---|---|
| `luma_pass` | `p0 = luma(img)`, **and `p1 = 0`** — scratch is not zeroed, and folding it here saves a dispatch |
| `copy_l_to_work` / `copy_l_to_aux` | `p2 = p0` / `p3 = p0` |
| `square` | `p2 = p0 * p0` |
| `tbox_h2`/`tbox_v2` | box at `tex_r`, `p2 <-> p4` |
| `tbox_h3`/`tbox_v3` | box at `tex_r`, `p3 <-> p4` |
| `cbox_h`/`cbox_v` | box at `cla_r`, `p2 <-> p4` |
| `gauss_h`/`gauss_v` | separable Gaussian, `p0 -> p4 -> p2` |
| `slope_offset` | reads `p2` = mean_g_sq and `p3` = mean_g; writes `p2` = slope, `p3` = offset |
| `acc_texture` | `p1 += k_t * (p0 - p2)` |
| `acc_texture_guided` | `p1 += k_t * (p0 - (p2 * p0 + p3))` |
| `acc_clarity` | `p1 += k_c * (p0 - p2) * midtone_weight(p0) * halo_suppression(p0 - p2, floor)` |
| `apply_gain` | `gain = clamp(1 + p1 / max(p0, EPS), 0, GAIN_MAX)`, scalar RGB multiply |

⚠⚠ `slope_offset` must read **both** planes before writing **either**:

```wgsl
    let mgsq = p2[i];
    let mg = p3[i];
    // ⚠⚠ `cov` and `var` are the SAME expression in the self-guided case, and only `var` is
    // clamped — `guided_filter`'s own comment says why a covariance may legitimately be negative.
    // Collapsing them to `var / (var + eps)` costs 4.7e-6 of divergence on flat areas, measured.
    let cov = mgsq - mg * mg;
    let var = max(cov, 0.0);
    let s = cov / (var + EPS);
    p2[i] = s;
    p3[i] = mg - s * mg;
```

- [ ] **Step 2: The dispatch sequence**

⚠⚠ **Each pass carries its own thread count.** A running-sum pass wants one thread per *line*, not
per pixel; dispatching `w*h` threads for 2136 rows of work prices the scheduler rather than the
algorithm. Build `Vec<(&'static str, (u32, u32))>`:

- Texture, `k_t > 0`: `gauss_h`, `gauss_v`, `acc_texture`
- Texture, `k_t < 0`: `square`, `tbox_h2`, `tbox_v2`, `copy_l_to_aux`, `tbox_h3`, `tbox_v3`,
  `slope_offset`, `tbox_h2`, `tbox_v2`, `tbox_h3`, `tbox_v3`, `acc_texture_guided`
- Clarity: `copy_l_to_work`, then `cbox_h`/`cbox_v` × `WIDE_PASSES`, then `acc_clarity`

⚠ Texture finishes into `p1` **before** Clarity starts, which is what lets the two bands share
`p2`–`p4`. ⚠ Both bands read `p0` and never each other's output — that is what makes the sliders
exactly additive, and a shader that chained them would be a different stage wearing the same name.

⚠ `ctx.cache.program` keys on the shader **text**, so the substituted radii give each rung its own
program. That is correct and costs one compile per distinct (radius, variant) pair.

- [ ] **Step 3: `scratch_planes` and `cpu_cost`**

```rust
    /// ⚠ **Five whatever the sliders say**, so the chain's allocation does not depend on an edit
    /// — the rule `GpuSharpenStage::scratch_planes` states. 136 MB at 6.8 Mpx, against
    /// sharpening's 109, and it is allocated per CHAIN rather than per stage.
    fn scratch_planes(&self) -> usize { 5 }
```

`cpu_cost` is filled in by Task 6 from a measurement; use `0.0` here and make the commit message
say it is provisional, or hold this task's commit until Task 6 — **prefer holding it**, so no
commit ships a constant nobody measured.

- [ ] **Step 4: Verify it builds**

Run: `cargo clippy --all-targets --all-features -p gpu-pipeline > /tmp/cl-$$.log 2>&1; echo "CLIPPY=$?"`

---

## Task 5: Equivalence tests

**Files:** `crates/gpu-pipeline/src/local_contrast.rs`'s test module

- [ ] **Step 1: A fixture with structure at both scales**

⚠⚠ A flat plane has `l - base == 0` everywhere and **every** shader passes on it. The fixture needs
a coarse ramp (for Clarity), a fine high-frequency band (for Texture), a hard edge (for the halo
suppressor), and values **below 0 and above 1** — the working space is scene-referred.

Minimum useful size is set by the radii: at least `4 * cla_r` across, so use a scale that keeps
`cla_r` small, or a fixture of ~200x120.

- [ ] **Step 2: The fixture check, first**

```rust
/// ⚠⚠ **Without this every assertion below is vacuous.** A stage that did nothing would match the
/// CPU perfectly, which is how creative LUTs (W5) shipped fully tested and 100% dead.
#[test]
fn the_fixture_is_one_this_stage_actually_moves() {}
```

- [ ] **Step 3: One equivalence test per path**

`texture_plus`, `texture_minus`, `clarity`, and **both together** — the last is the only case that
exercises the shared `p0` and the summed `p1`. Bound in 8-bit levels; this stage is full of
multiply-adds, so bit-identity is off the table (M8.3m's FMA finding).

- [ ] **Step 4: ⚠⚠ The negative clamp, on RAW FLOATS**

```rust
/// ⚠⚠ `compare` quantizes to 8-bit levels, where **a negative sample and 0 are the same value** —
/// so it is structurally blind to a missing clamp. That blind spot is M8.3e's finding about the
/// output transform and it appeared three more times in M8.3m. The stage's doc claims it "can
/// never emit a negative sample"; this is the only test that can see the claim.
#[test]
fn no_sample_comes_back_negative() {}
```

Drive it with a large negative Texture on a fixture with near-black pixels beside a bright edge,
where the gain's numerator goes strongly negative.

- [ ] **Step 5: The name test**

`GpuLocalContrastStage::name()` must equal the CPU stage's — six order-asserting tests match on it.

- [ ] **Step 6: Sabotage each**

- Drop the `max(cov, 0.0)` on `var` -> expect `texture_minus` to fail.
- Chain the bands (Clarity reads Texture's output) -> expect the both-bands test to fail and the
  single-band ones to pass, which is what proves the additivity guard is the one that sees it.
- Drop the halo suppressor -> expect `clarity` to fail.
- Drop `clamp(.., 0.0, ..)` -> expect **only** `no_sample_comes_back_negative` to fail.

⚠ Confirm each sabotage **compiled** before believing a pass: a sabotage that fails to compile
reads exactly like one that was caught, and a run with `0 passed` reads as green.

- [ ] **Step 7: Commit** — Tasks 4 and 5 land together if `cpu_cost` is still unmeasured.

---

## Task 6: Measure, wire in, and pin the decision

**Files:** `crates/gpu-pipeline/src/accelerate.rs`, `crates/gpu-pipeline/src/local_contrast.rs`

- [ ] **Step 1: Re-measure after Task 1's slimming**

```bash
cargo run --release -p pipeline --example probe_stages
```

Run it **three times** and quote a band. ⚠ Set `cpu_cost` at the **bottom** of the band — the
conservative direction, since a lower number makes a run less likely to chain and the gate must
fail toward the CPU.

⚠⚠ The stage has **four** costs (texture ±, clarity, both) and one `cpu_cost`. Use the **cheapest
active configuration** the stage can be in, which is the conservative choice: a run must not chain
on the strength of a cost the current edits do not incur. Compute it from the params in `new`.

- [ ] **Step 2: Add the `REPLACEABLE` row**

In **tail order**, between `tone_curve` and `saturation`:

```rust
    ("local_contrast", |e, spec, scale, ctx| {
        Box::new(GpuLocalContrastStage::new(&e.local_contrast, scale, ctx))
    }),
```

- [ ] **Step 3: The decision test**

```rust
/// ⚠⚠ Porting this makes the per-pixel head **contiguous through to `hsl`** — `local_contrast`
/// was the last CPU stage between `tone_curve` and `saturation`. A photo using Texture or Clarity
/// split its head into two runs the gate refused; it is now one run that pays.
#[test]
fn local_contrast_no_longer_splits_the_per_pixel_head() {}
```

Assert the chain count and the stage names before and after, reading each `cpu_cost` off the real
stage rather than repeating a literal — M8.3h's rule, so the test still describes the truth if a
cost is re-measured.

- [ ] **Step 4: Verify**

Run: `cargo test --release -p gpu-pipeline`
Run: `cargo test -p pipeline --test golden 2>&1 | tail -3` — **63 goldens, none moved.**

- [ ] **Step 5: Commit**

---

## Task 7: Bench, gate, document

- [ ] **Step 1: A bench row that can see this milestone**

Add `local contrast` rows to `bench_gpu_tail` — `texture_minus` alone (the expensive case) and
`texture+clarity`. ⚠ The existing rows structurally cannot see this milestone, which is the
asymmetry M8.3j recorded as the whole mechanism of its own wrong caveat.

- [ ] **Step 2: Re-scan the catalogue before quoting it**

```bash
cargo run --release -p app --example sidecar_scan ~/Pictures/fuji
```

⚠⚠ **A caveat measured against a catalogue is invalidated by editing that catalogue**, and
"confirm it by use" is precisely an instruction to edit it. Re-scan; do not quote the spec's
figures from memory.

- [ ] **Step 3: The full gate, each exit code from its own command**

```bash
cargo fmt --all --check > /tmp/fmt-$$.log 2>&1; echo "FMT=$?"
cargo clippy --all-targets --all-features -- -D warnings > /tmp/cl-$$.log 2>&1; echo "CLIPPY=$?"
cargo test --workspace > /tmp/t-$$.log 2>&1; echo "TEST=$?"
grep -c "^test golden_" /tmp/t-$$.log
grep -c "SKIPPED: no GPU adapter" /tmp/t-$$.log
```

⚠ Assert the test count moved by **exactly** the number of tests added — "still green" is not the
acceptance criterion.

- [ ] **Step 4: Update `CLAUDE.md`** with the measurement, the caveats, and the sweep table.

- [ ] **Step 5: Launch the GUI** and say what to look at.
