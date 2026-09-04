# Masked spatial adjustments (M7.2) — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A mask carries Texture, Clarity, Sharpening and HSL in addition to the per-pixel
adjustments it already carries.

**Architecture:** `MaskStage` already clones, runs its sub-stages and blends by coverage, so the
stages themselves need no new mechanism. Three fields join `LocalAdjustments`; the shared stage
builder learns them; **the global tail passes them neutral** so it keeps placing them itself. The one
structurally new part is the region margin, which stops being a fixed constant.

**Tech Stack:** Rust, the existing `pipeline` / `edit-model` / `app` crates. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-09-04-masked-spatial-design.md` — read §3 and §4 before Task 2.

---

## Standing rules for every task

- **Build environment, every shell:**
  ```
  export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
  ```
- **No `Co-Authored-By` trailer. Never push.**
- ⚠ **Read every exit code from a file, never a pipe** — `${PIPESTATUS[0]}` is bash and this shell is zsh.
- ⚠⚠ **Sabotage every new test**: break what it names, confirm a `test result:` line appeared, and
  confirm the **named** test failed — cargo prints a lib test's full module path. **A sabotage that
  does not fail is a finding about the fixture before it is a finding about the guard.**
- ⚠ Suffix temp files with `-m72`.

## File structure

| File | Responsibility | Change |
|---|---|---|
| `crates/edit-model/src/lib.rs` | `LocalAdjustments` | +3 fields |
| `crates/pipeline/src/lib.rs` | `adjustment_stages`, `tail_kernel_radius` | rename + 3 stages; sum over masks |
| `crates/pipeline/src/stages/mask.rs` | `MaskStage` | pass `scale` to the builder |
| `crates/app/src/view_region.rs` | viewport maths | **delete `REGION_MARGIN`** + its guard test |
| `crates/app/src/render_engine.rs` | render scale | make two fns `pub(crate)` |
| `crates/app/src/main.rs` | `Session` | `+source_scale`, `+render_scale()` |
| `crates/app/src/develop/canvas/mod.rs` | region request | derived margin |
| `crates/app/src/develop/panel/hsl_bands.rs` | **new** — the shared band block | extract from `adjustments.rs` |
| `crates/app/src/develop/panel/mask_adjust.rs` | **new** — a mask's adjustment sliders | extract from `masks.rs` |

⚠ `masks.rs` is **575 lines** and this adds ~120. The split in Task 7 is why it does not cross ~700.

---

### Task 1: `LocalAdjustments` gains the three fields

**Files:** Modify `crates/edit-model/src/lib.rs`

- [ ] **Step 1: Write the failing test**

In `crates/edit-model/src/lib.rs`'s test module:

```rust
#[test]
fn a_mask_can_carry_the_tail_native_adjustments() {
    // ⚠ The point of the milestone in one assertion: these three are *storable* on a mask.
    // Defaults must stay inert, or every existing sidecar silently gains stages.
    let a = LocalAdjustments {
        local_contrast: LocalContrast { texture: 40.0, clarity: -30.0 },
        sharpening: Sharpening { amount: 55.0, ..Sharpening::default() },
        hsl: Hsl { bands: [HslBand { hue: 12.0, saturation: 0.0, luminance: 0.0 }; 8] },
        ..LocalAdjustments::default()
    };
    assert!(a.is_active(), "a mask carrying only spatial values must count as active");
    assert!(!LocalAdjustments::default().is_active(), "the default must stay inert");
}
```

- [ ] **Step 2: Run it and watch it fail**

Run: `cargo test -p edit-model a_mask_can_carry_the_tail_native_adjustments`
Expected: FAIL — `no field local_contrast on type LocalAdjustments`.

- [ ] **Step 3: Add the fields**

```rust
pub struct LocalAdjustments {
    // ... existing fields unchanged ...
    /// Fine and coarse local contrast, carried by the mask.
    pub local_contrast: LocalContrast,
    /// ⚠ Full parity with the global control, `masking` included — which inside a mask is a
    /// second gate inside a gate, since the mask has already answered *where*. Kept because
    /// parity costs nothing here and an edge gate is defensible for a loose mask over mixed
    /// content. A deliberate choice (2026-09-04), not an oversight.
    pub sharpening: Sharpening,
    /// ⚠ 24 values — an order of magnitude more state than anything else here, and the reason
    /// a mask's panel section is markedly taller than it was.
    pub hsl: Hsl,
}
```

`is_active` is `*self != Self::default()` and needs no change — but **verify** that
`Sharpening::default()` is hand-written with `radius: 1.0, detail: 50.0`, so the derived
`LocalAdjustments::default()` is still inert (`amount: 0.0` means `is_active` is false).

- [ ] **Step 4: Run it and watch it pass**

Run: `cargo test -p edit-model a_mask_can_carry_the_tail_native_adjustments`
Expected: PASS.

- [ ] **Step 5: Update `all_nonneutral` BY HAND — nothing will remind you**

⚠⚠ **Spec §2's finding, and the reason this step exists as its own step.** `edit_model::all_nonneutral`
builds its `LocalAdjustments` with `..Default::default()`, so it **will not fail to compile** and the
three new fields would land **untested by the very fixture that exists to force a decision** — a
construct that names exhaustiveness and does not deliver it through a nested struct.

Give its masked adjustment non-neutral values for all three:

```rust
            adjustments: LocalAdjustments {
                // ... existing non-neutral fields ...
                local_contrast: LocalContrast { texture: 35.0, clarity: -25.0 },
                sharpening: Sharpening { amount: 60.0, ..Sharpening::default() },
                hsl: Hsl { bands: [HslBand { hue: 8.0, saturation: -15.0, luminance: 5.0 }; 8] },
            },
```

- [ ] **Step 6: Sabotage**

Change `is_active` to `self.exposure != 0.0`. Expected: the named test FAILS.
Restore, and confirm it passes again.

- [ ] **Step 7: Commit**

```bash
git add crates/edit-model/src/lib.rs
git commit -m "feat(edit-model): a mask carries local contrast, sharpening and HSL"
```

---

### Task 2: The shared builder learns the stages — and the global caller passes them neutral

**Files:** Modify `crates/pipeline/src/lib.rs`, `crates/pipeline/src/stages/mask.rs`

⚠⚠ **Read spec §3 first.** `per_pixel_stages` has two consumers. Adding stages naively makes the
global tail apply them **twice**, because `build_pipeline_split` already places
`LocalContrastStage` and `SharpenStage` itself. The fix already exists in that function for
saturation: **the global caller passes neutral.**

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn the_global_tail_does_not_apply_the_mask_only_stages_twice() {
    // ⚠⚠ The trap this task exists to avoid. The global tail places local contrast, HSL and
    // sharpening itself, so the shared builder must receive them NEUTRAL from that caller.
    // Applied twice the image is visibly over-sharpened and every golden moves.
    let edits = Edits {
        local_contrast: edit_model::LocalContrast { texture: 60.0, clarity: 40.0 },
        sharpening: edit_model::Sharpening { amount: 70.0, ..Default::default() },
        ..Edits::default()
    };
    let names = build_pipeline(&edits, RenderScale::FULL).stage_names();
    assert_eq!(
        names.iter().filter(|n| **n == "local_contrast").count(), 1,
        "local contrast ran twice: {names:?}"
    );
    assert_eq!(
        names.iter().filter(|n| **n == "sharpening").count(), 1,
        "sharpening ran twice: {names:?}"
    );
}
```

- [ ] **Step 2: Run it and watch it pass — then make it meaningful**

Run: `cargo test -p pipeline the_global_tail_does_not_apply_the_mask_only_stages_twice`
Expected: **PASS** before any change, because the stages are not in the shared builder yet.
⚠ That is the honest state: the test is a *tripwire for Step 3*, not a red test. Record it as such
rather than pretending it was red — a test that was never red proves nothing until the change lands.

- [ ] **Step 3: Rename, take a scale, add the stages**

In `crates/pipeline/src/lib.rs`, rename `per_pixel_stages` to `adjustment_stages` and give it a
`RenderScale`. Order **mirrors the global chain**, which is the rule a mask's adjustments already
follow:

```rust
/// Build the stages for one set of adjustments.
///
/// ⚠ **One list, two consumers**: the global tail, and every mask.
///
/// ⚠⚠ **Three of these are passed NEUTRAL by the global caller** — local contrast, HSL and
/// sharpening — because the global chain places each itself, at a position each has earned.
/// Passing the real values would apply them twice and move every golden. This is the same rule
/// `saturation` has followed here since masking landed; see the caller.
fn adjustment_stages(
    a: &edit_model::LocalAdjustments,
    baseline: color::wb::Illuminant,
    scale: RenderScale,
) -> Vec<Box<dyn PipelineStage>> {
    // ... white balance, exposure, contrast, tone regions unchanged ...

    // After the tone regions and before saturation, mirroring the global chain: a local
    // contrast measured after a tonal change is the one that describes what you see.
    if a.local_contrast.is_active() {
        out.push(Box::new(LocalContrastStage::new(&a.local_contrast, scale)));
    }
    if a.saturation != 0.0 || a.vibrance != 0.0 {
        out.push(Box::new(SaturationStage::new(a.saturation, a.vibrance)));
    }
    // After saturation for the reason it is there globally: HSL corrects colours that are *in*
    // the picture, so it must see final tonality.
    if a.hsl != edit_model::Hsl::default() {
        out.push(Box::new(HslStage::new(&a.hsl)));
    }
    // ⚠ Last, for the reason it is last globally: sharpening must not amplify anything a later
    // stage would then stretch.
    if a.sharpening.is_active() {
        out.push(Box::new(SharpenStage::new(&a.sharpening, scale)));
    }
    out
}
```

- [ ] **Step 4: Make the exhaustive caller pass them neutral**

`build_pipeline_split`'s `LocalAdjustments` literal is **exhaustive and will refuse to compile** —
that is the compiler forcing this decision rather than leaving it to be remembered. Extend the
existing comment and add the three fields:

```rust
    // ⚠ `saturation`/`vibrance` are passed as **zero** here, deliberately ... (existing text)
    //
    // ⚠⚠ `local_contrast`, `hsl` and `sharpening` are passed **neutral** for the identical
    // reason: this tail places each of them itself, further down. Passing the real values would
    // apply each twice and move every golden.
    let mut tail: Vec<Box<dyn PipelineStage>> = adjustment_stages(
        &edit_model::LocalAdjustments {
            // ... existing fields ...
            saturation: 0.0,
            vibrance: 0.0,
            local_contrast: edit_model::LocalContrast::default(),
            hsl: edit_model::Hsl::default(),
            sharpening: edit_model::Sharpening::default(),
        },
        baseline,
        scale,
    );
```

Update `MaskStage::new` and `MaskStage::new_in_region` in `crates/pipeline/src/stages/mask.rs` to
pass their existing `scale` through to `adjustment_stages`.

- [ ] **Step 5: Update `pipeline::all_edits` BY HAND — it will not break either**

⚠⚠ Same finding as Task 1 Step 5, and the same reason it is its own step: `all_edits` builds its
`LocalAdjustments` with `..Default::default()`, so it compiles unchanged while silently not
exercising the new fields. Give its masked adjustment the same three non-neutral values Task 1 used.

- [ ] **Step 6: Run the whole suite — the goldens are the proof**

Run: `cargo test --all > /tmp/t-m72.log 2>&1; echo "EXIT=$?"`
Expected: **EXIT=0, all 54 goldens byte-identical.**
⚠⚠ **A golden that moves means the global path gained a stage it already had.** Do not regenerate
it. Fix Step 4.

- [ ] **Step 7: Sabotage — the one that matters**

In Step 4's literal, replace `local_contrast: edit_model::LocalContrast::default()` with
`local_contrast: edits.local_contrast`. Run `cargo test -p pipeline`.
Expected: `the_global_tail_does_not_apply_the_mask_only_stages_twice` **FAILS**, and goldens move.
Restore. **This is what converts Step 2's tripwire into a real guard.**

- [ ] **Step 8: Commit**

```bash
git add crates/pipeline/src/lib.rs crates/pipeline/src/stages/mask.rs
git commit -m "feat(pipeline): masks carry local contrast, HSL and sharpening"
```

---

### Task 3: `tail_kernel_radius` sums over masks

**Files:** Modify `crates/pipeline/src/lib.rs`

⚠⚠ **Sum, not max.** Mask *N* runs on mask *N−1*'s output, so the reaches compose. This is the
M3.1a lesson restated and **it is the one place this design can be quietly wrong.**

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn two_masked_clarities_reach_further_than_one() {
    // ⚠⚠ A ONE-MASK TEST CANNOT SEE SUM-VERSUS-MAX — with a single mask they are identical.
    // Two is the smallest fixture that discriminates, the same shape as the
    // same-month-in-two-years fixture.
    let masked = |clarity: f32| edit_model::MaskedAdjustment {
        mask: edit_model::Mask::default(),
        adjustments: edit_model::LocalAdjustments {
            local_contrast: edit_model::LocalContrast { texture: 0.0, clarity },
            ..Default::default()
        },
    };
    let one = Edits { masks: vec![masked(100.0)], ..Edits::default() };
    let two = Edits { masks: vec![masked(100.0), masked(100.0)], ..Edits::default() };

    let r1 = tail_kernel_radius(&one, RenderScale::FULL);
    let r2 = tail_kernel_radius(&two, RenderScale::FULL);
    assert!(r1 > 0, "a masked clarity must contribute reach at all");
    assert_eq!(r2, 2 * r1, "the reaches compose: two masks reach twice as far, not the same");
}
```

- [ ] **Step 2: Run it and watch it fail**

Run: `cargo test -p pipeline two_masked_clarities_reach_further_than_one`
Expected: FAIL — `assertion failed: r1 > 0`, because masks contribute nothing yet.

- [ ] **Step 3: Sum over the masks**

```rust
pub fn tail_kernel_radius(edits: &Edits, scale: RenderScale) -> usize {
    let sharp = if edits.sharpening.is_active() {
        stages::sharpening::max_kernel_radius(edits.sharpening.radius, scale)
    } else { 0 };
    let local = stages::local_contrast::kernel_reach(&edits.local_contrast, scale);

    // ⚠⚠ **Summed, not maximised.** Mask N runs on mask N-1's output, so a pixel at the region
    // edge reads through every mask in turn. Taking a max here is invisible with one mask and
    // wrong with two — see `two_masked_clarities_reach_further_than_one`.
    let masked: usize = edits
        .masks
        .iter()
        .map(|m| {
            let a = &m.adjustments;
            let s = if a.sharpening.is_active() {
                stages::sharpening::max_kernel_radius(a.sharpening.radius, scale)
            } else { 0 };
            s + stages::local_contrast::kernel_reach(&a.local_contrast, scale)
        })
        .sum();

    sharp + local + masked
}
```

- [ ] **Step 4: Delete `max_tail_kernel_radius`**

Remove it. ⚠ With an unbounded mask stack a fixed worst case is not a meaningful number — that is
precisely what Task 4 replaces. Its only caller is the guard test deleted in Task 4.

- [ ] **Step 5: Run and sabotage**

Run: `cargo test -p pipeline two_masked_clarities_reach_further_than_one` → PASS.
Sabotage: change `.sum()` to `.max().unwrap_or(0)`. Expected: the named test **FAILS** with
`left: <r1>, right: <2*r1>`. Restore.

- [ ] **Step 6: Commit**

```bash
git add crates/pipeline/src/lib.rs
git commit -m "feat(pipeline): the tail's kernel reach sums over masks"
```

---

### Task 4: The region margin is derived, not a constant

**Files:** Modify `crates/app/src/render_engine.rs`, `crates/app/src/main.rs`,
`crates/app/src/develop/canvas/mod.rs`, `crates/app/src/view_region.rs`

⚠⚠ **Read spec §4.** The canvas must ask the *same function* the pipeline uses. A canvas that
computed its own margin would make the reach and the request two facts that can disagree — the
shape `render_max_dim`, `fit_image_size` and `geometry_mode` were each fixed for. The failure mode
is silent: the edge of the *visible* area is subtly wrong and nothing errors.

- [ ] **Step 1: Write the failing test**

In `crates/app/src/main.rs`'s test module (it can reach `Session`):

```rust
#[test]
fn the_render_margin_follows_the_edits_it_will_render() {
    // ⚠ The margin must grow when the edits reach further. A fixed constant cannot: masks chain,
    // so N masked clarities reach N x 120 px and no constant covers an unbounded stack.
    let mut s = test_app::session_for_margin_test();
    let plain = s.render_margin(s.buffer_long);
    s.edits.masks.push(edit_model::MaskedAdjustment {
        mask: edit_model::Mask::default(),
        adjustments: edit_model::LocalAdjustments {
            local_contrast: edit_model::LocalContrast { texture: 0.0, clarity: 100.0 },
            ..Default::default()
        },
    });
    let masked = s.render_margin(s.buffer_long);
    assert!(masked > plain, "the margin ignored a masked clarity: {masked} vs {plain}");
}
```

Add `test_app::session_for_margin_test()` beside the existing fixtures, returning a `Session` with
`buffer_long` and `full_long` set to the same non-zero value (so `RenderScale` is 1.0) and default
edits.

- [ ] **Step 2: Run it and watch it fail**

Run: `cargo test -p app --bin Rustroom the_render_margin_follows_the_edits_it_will_render`
Expected: FAIL — `no method render_margin on Session`.

- [ ] **Step 3: Expose the render scale, once**

In `crates/app/src/render_engine.rs`, change `fn rendered_long_edge` and `fn render_scale` to
`pub(crate) fn`. Do **not** copy their bodies anywhere.

In `crates/app/src/main.rs`, add a `source_scale: f32` field to `Session` (both construction sites
— `finalize_session` and the full-resolution handover — already hold `decoded.source_scale`), and:

```rust
impl Session {
    /// The `RenderScale` the worker will use for a render at `max_dim`.
    ///
    /// ⚠⚠ Calls the engine's own functions rather than re-deriving. The canvas needs this to size
    /// its region request, and a second derivation would be two facts that can disagree — with a
    /// silent failure mode, since the result is a subtly wrong band at the edge of the visible
    /// area rather than an error.
    pub(crate) fn render_scale(&self, max_dim: usize) -> pipeline::RenderScale {
        render_engine::render_scale(
            self.buffer_long,
            render_engine::rendered_long_edge(self.buffer_long, max_dim),
            self.source_scale,
        )
    }

    /// How far beyond the visible rect a region render must reach, in rendered pixels.
    pub(crate) fn render_margin(&self, max_dim: usize) -> usize {
        pipeline::tail_kernel_radius(&self.edits, self.render_scale(max_dim))
    }
}
```

- [ ] **Step 4: Use it at the call site**

In `crates/app/src/develop/canvas/mod.rs`, replace `view_region::REGION_MARGIN`:

```rust
                            Some(view_region::with_margin(
                                vis,
                                iw,
                                ih,
                                // ⚠ Derived from the edits actually being rendered, through the
                                // same function the pipeline uses. See `Session::render_margin`.
                                s.render_margin(max_dim),
                            ))
```

- [ ] **Step 5: Delete the constant and its guard**

In `crates/app/src/view_region.rs` remove `REGION_MARGIN` and the
`the_region_margin_exceeds_every_tail_kernel_radius` test. ⚠ Leave a comment where the constant was
saying **why** there is no constant any more, or the next reader will helpfully reintroduce one:

```rust
//! ⚠⚠ There is deliberately **no `REGION_MARGIN` constant.** Masks chain, so N masked clarities
//! reach N x 120 px into the input and no fixed number covers an unbounded stack. The margin is
//! derived per render by `Session::render_margin`, from the same `pipeline::tail_kernel_radius`
//! the pipeline itself uses.
```

- [ ] **Step 6: Run and sabotage**

Run: `cargo test -p app --bin Rustroom the_render_margin_follows_the_edits_it_will_render` → PASS.
Sabotage: make `render_margin` return `192`. Expected: the named test **FAILS**. Restore.

- [ ] **Step 7: Commit**

```bash
git add crates/app/src
git commit -m "feat(app): derive the region margin from the edits being rendered"
```

---

### Task 5: The region-equality test gains a masked spatial edit

**Files:** Modify `crates/pipeline/src/lib.rs` (the `a_region_render_matches_the_whole_render_cropped` test)

⚠⚠ **This test has been silently insufficient in exactly this way once before.** When sharpening
landed it kept passing because its edit set contained no sharpening. Left alone it will do so again.

- [ ] **Step 1: Add a masked spatial edit to its edit set**

Add to that test's `Edits` a mask carrying `local_contrast: LocalContrast { texture: 0.0, clarity:
80.0 }`. Its inset already derives from `tail_kernel_radius`, so it tracks the new reach with no
other change. ⚠ Check the buffer is still larger than `2 * inset`; the inset roughly doubles, so
**the fixture buffer and region must grow with it** — they are already derived from `inset`, so
confirm rather than assume.

- [ ] **Step 2: Run it**

Run: `cargo test -p pipeline a_region_render_matches_the_whole_render_cropped`
Expected: PASS. If it fails, the region maths does not cover masked spatial reach — that is a **real
bug in Task 3 or 4**, not a test to loosen. ⚠ **Never widen the inset to make it pass.**

- [ ] **Step 3: Sabotage**

Temporarily set the inset to `0`. Expected: FAIL on an interior byte mismatch, proving the test can
still see an edge band. Restore.

- [ ] **Step 4: Commit**

```bash
git add crates/pipeline/src/lib.rs
git commit -m "test(pipeline): the region-equality test carries a masked spatial edit"
```

---

### Task 6: A golden, and the confinement test

**Files:** Modify `crates/pipeline/tests/golden.rs`

- [ ] **Step 1: Write the confinement test**

```rust
#[test]
fn a_masked_clarity_leaves_uncovered_pixels_bit_identical() {
    // ⚠⚠ "Masked clarity changes the pixels" passes with the mask ignored entirely. THIS is the
    // discriminating half: outside the mask the result must be bit-identical to no mask at all —
    // not close, identical.
    // ⚠ `check_golden_with` compares against a file; this test compares two renders to each
    // other, so it drives the pipeline directly. There is no `render` helper — do not invent one.
    let chart = local_contrast_chart();
    let plain = build_pipeline(&Edits::default(), RenderScale::FULL).run(chart.clone());
    let masked = build_pipeline(&edits_with_masked_clarity(), RenderScale::FULL).run(chart);

    // A pixel the mask does not cover, well outside its coverage.
    assert_eq!(
        plain.pixel(1, 1), masked.pixel(1, 1),
        "the masked clarity leaked outside its mask"
    );
    // And the fixture must actually be one the mask changes somewhere, or this proves nothing.
    assert_ne!(
        plain.data(), masked.data(),
        "fixture check: the mask changed nothing at all, so this test proves nothing"
    );
}
```

Write `edits_with_masked_clarity()` as a linear-gradient mask covering only one side of the chart,
carrying `clarity: 80.0`, so pixel (1, 1) is genuinely at coverage 0.

- [ ] **Step 2: Run it and watch it fail if the mask leaks**

Run: `cargo test -p pipeline a_masked_clarity_leaves_uncovered_pixels_bit_identical`
Expected: PASS. A failure means the blend is wrong, not the test.

- [ ] **Step 3: Add the golden**

Add a `golden_mask_clarity` test calling `check_golden_with("mask_clarity", &edits,
local_contrast_chart())` — the naming convention `golden_mask_brush_exposure` already follows.
Generate with `UPDATE_GOLDEN=1 cargo test -p pipeline golden_mask_clarity`.
⚠ **Measure the generated PNG before accepting it**: assert it does not clip, the way
`the_grading_goldens_do_not_clip` does. A clipped golden cannot tell one coverage from another.

- [ ] **Step 4: Confirm the other 54 are byte-identical**

Run: `cargo test -p pipeline > /tmp/g-m72.log 2>&1; echo "EXIT=$?"` and confirm 55 golden tests ran.

- [ ] **Step 5: Sabotage**

In `MaskStage::apply`, force the blend factor to `1.0`. Expected: the confinement test **FAILS**.
Restore.

- [ ] **Step 6: Commit**

```bash
git add crates/pipeline/tests
git commit -m "test(pipeline): a masked clarity is confined to its mask, with a golden"
```

---

### Task 7: The panel — extract the shared HSL block, split the mask adjustments

**Files:** Create `crates/app/src/develop/panel/hsl_bands.rs`, `crates/app/src/develop/panel/mask_adjust.rs`; modify `adjustments.rs`, `masks.rs`, `panel/mod.rs`

⚠ `masks.rs` is 575 lines and this adds ~120. ⚠ `adjustments.rs` already carries the HSL band block
and its own comment reads *"One layout learned once"* — so it is **extracted and shared, not copied**.

- [ ] **Step 1: Extract the band block, unchanged**

Move the existing `Hue │ Saturation │ Luminance` block from `adjustments.rs` into
`develop/panel/hsl_bands.rs` as:

```rust
/// The eight band sliders for one HSL channel, with the segmented channel selector above them.
///
/// ⚠ Shared by the global panel and by a mask. Duplicating it is how the two would drift on band
/// order or range — and `HSL_BAND_NAMES` is already the single source of truth for the names.
pub fn show(ui: &mut egui::Ui, hsl: &mut edit_model::Hsl, channel: &mut HslChannel, ed: &mut Edited)
```

⚠ **The existing `adjustments.rs` HSL tests must pass unchanged** — that is the proof the extraction
moved no behaviour. Do not edit them.

- [ ] **Step 2: Run the existing tests**

Run: `cargo test -p app --bin Rustroom hsl`
Expected: PASS, unchanged.

- [ ] **Step 3: Move the mask adjustment sliders into their own module**

Move the mask's Temp…Vibrance block out of `masks.rs` into `develop/panel/mask_adjust.rs` as
`pub fn show(ui, a: &mut LocalAdjustments, channel: &mut HslChannel, ed: &mut Edited)`, then append
the new controls **in the pipeline's own order**, which is what a mask's adjustments already mirror:

```rust
    // ... existing Temp..Vibrance ...
    ui.separator();
    ed.slider(ui, &mut a.local_contrast.texture, -100.0..=100.0, "Texture");
    ed.slider(ui, &mut a.local_contrast.clarity, -100.0..=100.0, "Clarity");
    ui.separator();
    hsl_bands::show(ui, &mut a.hsl, channel, ed);
    ui.separator();
    // ⚠ Full parity, `Masking` included — a second gate inside a gate, kept deliberately.
    ed.slider(ui, &mut a.sharpening.amount, 0.0..=100.0, "Amount");
    ed.slider(ui, &mut a.sharpening.radius, 0.5..=3.0, "Radius");
    ed.slider(ui, &mut a.sharpening.detail, 0.0..=100.0, "Detail");
    ed.slider(ui, &mut a.sharpening.masking, 0.0..=100.0, "Masking");
```

⚠ The mask needs **its own** `HslChannel` state on `Session`, not the global panel's — otherwise
switching to a mask silently changes which channel the global panel is showing.

- [ ] **Step 4: Write the wiring test**

```rust
#[test]
fn a_selected_mask_shows_the_spatial_sliders_and_they_edit_it() {
    // ⚠ That the slider exists proves nothing about it being wired to the mask's own field.
    let mut s = session_with_one_selected_mask();
    let ed = drag_in_panel("Clarity", &mut s);
    assert!(ed.changed(), "dragging Clarity reported no edit");
    assert_ne!(
        s.edits.masks[0].adjustments.local_contrast.clarity, 0.0,
        "the Clarity slider is not wired to the selected mask"
    );
}
```

- [ ] **Step 5: Run, sabotage, commit**

Run: `cargo test -p app --bin Rustroom a_selected_mask_shows_the_spatial_sliders_and_they_edit_it`
Sabotage: point the slider at a scratch local instead of `a.local_contrast.clarity`.
Expected: the named test **FAILS**. Restore.

```bash
git add crates/app/src/develop/panel
git commit -m "feat(app): a mask's panel carries texture, clarity, HSL and sharpening"
```

---

### Task 8: Measure, then decide

**Files:** Create `crates/app/examples/bench_masked.rs`

⚠⚠ **Read spec §5.** HSL is the expensive one, **not** clarity: global HSL is +34.9 ms at preview
against clarity's +8.8 ms. Three masked-HSL masks is roughly +105 ms on a 13 ms tail.

- [ ] **Step 1: Write the example**

Beside `bench_preview`, timing best-of-5 for: neutral; one masked clarity; three masked clarities;
one masked HSL; three masked HSL — each at **fit** and at a **native region**.

- [ ] **Step 2: Run it on a real file**

```bash
cargo run --release -p app --example bench_masked "$RAW_FIXTURE"
```

- [ ] **Step 3: Record the numbers in the spec and decide**

Append a measured table to spec §5 and answer both questions **from the numbers, not from taste**:
does the bounding box ship here, and does the hue LUT become the next milestone?
⚠ The bounding box helps only brush and radial masks; a linear gradient covers half the frame and
range/colour masks have no bbox at all. **Say which mask kind any recommendation is about.**

- [ ] **Step 4: Commit**

```bash
git add crates/app/examples/bench_masked.rs docs/superpowers/specs/2026-09-04-masked-spatial-design.md
git commit -m "perf(app): measure masked spatial cost, and record what it decides"
```

---

### Task 9: Gates, then the human checklist

- [ ] **Step 1: Run all three gates, reading each exit code from a file**

```bash
cargo fmt --all
cargo fmt --all -- --check > /tmp/f-m72.log 2>&1; echo "fmt EXIT=$?"
cargo clippy --all-targets --all-features -- -D warnings > /tmp/c-m72.log 2>&1; echo "clippy EXIT=$?"
cargo test --all > /tmp/t-m72.log 2>&1; echo "test EXIT=$?"
grep -E "^test result:" /tmp/t-m72.log | awk '{p+=$4; f+=$6} END {print "passed="p" failed="f}'
```

Expected: three zeros; 1975 + the new tests; **55 goldens, 54 of them byte-identical.**

- [ ] **Step 2: Launch the GUI and say what to look at**

```bash
cargo run --release -p app
```

- [ ] **Step 3: Write the checklist a human must run**

⚠ **Claim none of these as done.** `AXIsProcessTrusted()` is false here, so synthetic input is
silently dropped and `screencapture` fails.

1. A brush mask on a face, **Clarity −60**: the skin softens, the background does not.
2. The same mask, **Texture −100**: skin smooths while the coarse markings survive. *This is the
   skin-softening tool, and it is why deferring masked NR costs nothing.*
3. **Sharpening amount 80** on an eye mask, with global sharpening also on: the eye gets **both**.
4. **Masked HSL**, greens −40 saturation under a mask over foliage: the foliage desaturates and
   identically-coloured foliage outside the mask does not.
5. ⚠ **Zoom to 1:1 and pan with two masked clarities active.** This is the one the derived margin
   exists for — watch the *edges* of the viewport for a band that differs from the interior.
6. ⚠ **Drag a masked HSL slider at 1:1** and say whether it is usable. §5's measurement predicts
   this is the worst case in the feature.

- [ ] **Step 4: Update `CLAUDE.md` and commit**

Add the M7.2 section: what shipped, the neutral-passing rule, the derived margin and why no constant
replaced it, the measured costs, and the accepted caveats from spec §7.

```bash
git add -A
git commit -m "docs: close M7.2 — masked spatial adjustments"
```
