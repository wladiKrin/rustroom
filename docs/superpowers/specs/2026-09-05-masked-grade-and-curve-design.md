# Masked colour grading and tone curve — design

**Status:** specced 2026-09-05. Branch `masked-grade-and-curve`.

A mask carries a **tone curve** and a **three-way colour grade** in addition to everything it
already carries. Dodge and burn with a curve; split-tone a sky; warm a face without warming the
room behind it.

This is **Part A** of `docs/BACKLOG.md`'s *"The rest of the stages inside a mask"*, asked for at
M7.2's merge — *"I'd also like to add the dehaze and rest to masks."* ⚠ **Part B — dehaze and
noise reduction — is a separate slice blocked on a design question**, not on effort; §7 restates
it. Part A does not wait for it and does not pretend to close it.

⚠ **Colour grading's exclusion is re-decided here, not inherited.** It was excluded from masks on
the stated ground that *"`LocalAdjustments` is per-pixel-only and a grade is global look."* The
first half stopped being true at M7.2, which put three spatial stages in a mask. The second half is
a judgement about taste rather than about structure, and a masked split-tone on a sky is an
ordinary request. **Decision (2026-09-05, with the user): both stages ship.**

---

## 1. What already works, and why this is smaller than M7.2

Both stages are **tail-native**: `ToneCurveStage` and `ColorGradingStage` already run in the tail,
beside the four a mask gained at M7.2, and both are strictly per-pixel. So the whole of M7.2's
structurally new part — the derived region margin — is **not in scope and needs no change**:

- `tail_kernel_radius` is untouched. Neither stage reads a neighbour, so neither contributes reach.
- No `Region`, no `RenderScale`, no `PrefixStepKey`, no cache-key question.
- `MaskStage` is untouched. It already clones, runs sub-stages and blends by coverage; two more
  per-pixel stages drop into that list and their semantics are already right.

What is left is M7.2's playbook re-run, and one thing M7.2 did not have to think about (§2).

## 2. The model

`LocalAdjustments` gains two fields, **embedding the existing structs whole** — the rule M7.2
followed for `LocalContrast`, `Sharpening` and `Hsl`, so a mask's control means exactly what the
global control of the same name means:

```rust
pub tone_curve: ToneCurve,        // Vec<[f32; 2]>; empty = identity
pub color_grading: ColorGrading,  // Copy; ⚠ blending is 50.0 at neutral, not 0.0
```

### ⚠⚠ `LocalAdjustments` loses `Copy`, and that was measured rather than assumed

`ToneCurve` holds a `Vec<[f32; 2]>`, so `LocalAdjustments` cannot stay `Copy`. This was the one
thing that could have made Part A structurally expensive, and it does not:

> **Spiked 2026-09-05.** With both fields added and `Copy` removed from the derive,
> `cargo check --workspace --all-targets` is **clean — zero call-site changes**.

`MaskedAdjustment` has not been `Copy` since the brush landed (`Mask::Brush` carries a stroke
list), so nothing downstream was relying on it. Two doc comments assert the `Copy`-ness and must be
corrected in the same commit; there is no code churn beyond that.

### ⚠⚠ And the doc comment above `LocalAdjustments` is already stale — M7.2 left it behind

It currently reads:

> *"What a mask can carry: the **per-pixel** adjustments, and only those. … A spatial stage —
> noise reduction, sharpening, clarity, dehaze, HSL — reads neighbours outside the mask, so
> 'masked' needs a separate decision for each and is out of scope here."*

`local_contrast`, `sharpening` and `hsl` are declared **directly beneath it**. The comment
contradicts the fields it introduces, and has since M7.2. Part A rewrites it to say what the set
actually is: *everything the tail applies per mask, which is now most of the tail* — and to name
the prefix stages (dehaze, noise reduction) as the genuine remainder, with §7's reason.

### Sidecar compatibility

Both fields are additive under `LocalAdjustments`' container-level `#[serde(default)]`, so an
existing sidecar reads them as neutral. **No `Edits::version` bump** — that field is a marker for
future migrations, and nothing here needs one.

⚠ `ColorGrading::default` is **hand-written, not derived**, because `blending` is 50 at neutral;
the container-level `#[serde(default)]` fills a missing field from it. That already holds and
needs nothing here — it is noted because a partial sidecar silently acquiring `blending: 0`
(hard-edged ranges) is the trap `ColorGrading`, `LensCorrection`, `RadialGradient` and
`Perspective` each carry a comment about.

## 3. The pipeline

`adjustment_stages` gains two insertions, each at the position that **mirrors the global chain** —
M7.2's stated rule, and the one that makes what you read top to bottom in the panel be what runs:

```
wb → exposure → contrast → tone regions → TONE CURVE →
local contrast → saturation/vibrance → hsl → COLOUR GRADING → sharpening
```

- **The curve goes after tone regions and before local contrast.** After the tone curve is
  mandatory for local contrast globally — a local contrast measured before it would then be
  re-mapped by it — and confining both to a mask does not change that argument.
- **The grade goes after HSL and before sharpening.** HSL corrects colours that are *in* the
  picture; a grade is laid on top. Reversed, an HSL band tweak would act on the tint just added.
  Globally the grade also runs after the tone curve and local contrast because its ranges are
  selected on **lightness**, so anything remapping lightness later would slide pixels between
  ranges — that ordering is preserved exactly inside a mask.

Activation predicates mirror the global ones exactly:

```rust
if !a.tone_curve.points.is_empty() { … ToneCurveStage::new(&a.tone_curve.points) }
if a.color_grading.is_active()     { … ColorGradingStage::new(&a.color_grading) }
```

### ⚠⚠ The global caller passes both NEUTRAL, and the compiler forces that decision

`build_pipeline_split`'s exhaustive `LocalAdjustments` literal already passes `saturation`,
`vibrance`, `local_contrast`, `hsl` and `sharpening` neutral, because **the global tail places each
of those itself, further down, at a position each has earned.** `ToneCurveStage` and
`ColorGradingStage` are placed by that same tail and follow the identical rule — passing the real
values would apply both **twice** and move every golden.

⚠ **That literal genuinely fires.** Verified by the spike: adding the two fields fails
`crates/pipeline/src/lib.rs:531` with

```
error[E0063]: missing fields `color_grading` and `tone_curve` in initializer of `LocalAdjustments`
```

which is the best possible place for the decision to be forced.

### The exhaustive sites — checked, not assumed

M7.2's correction stands and is reused rather than re-derived:

| site | fires? | why |
|---|---|---|
| `build_pipeline_split`'s literal | ✅ **compile error** | exhaustive struct literal, no `..` |
| `Edits::merge_from`'s destructure | ❌ nothing needed | destructures `Edits`, not `LocalAdjustments` — a mask travels whole under `MASKS_GROUP` |
| `EditGroups` | ❌ nothing needed | same reason |
| `pipeline::all_edits` (`pipeline/src/lib.rs:1054`) | ⚠ **silent** | builds its `LocalAdjustments` with `..Default::default()` |
| `edit_model::all_nonneutral` (`edit-model/src/lib.rs:2317`) | ⚠ **silent** | same |

⚠⚠ **The last two must be updated BY HAND, and they will not break.** Two fields whose entire
purpose is to be non-neutral would otherwise land untested by the very fixtures that exist to force
a decision about them. Both already carry M7.2's warning comment saying exactly this — *"a construct
that names exhaustiveness does not deliver it through a nested struct"* — so the reminder is in
place and the job is to honour it.

⚠ `all_nonneutral`'s mask must carry a **non-identity** curve and a grade with at least one wheel
turned up, or `is_active` reads false and the fixture proves nothing about them.

## 4. The panel

`mask_adjust::show` gains two **collapsed blocks, closed by default**, reusing both widgets
unchanged — the straight reuse `hsl_bands::show` got at M7.2, honouring `adjustments.rs`'s own
*"One layout learned once"*:

- `tone_curve::show(ui, &mut a.tone_curve, ed)` — fully self-contained. It draws its own
  "Tone Curve" heading and Reset button, needs no histogram and no buffer, and returns the editor's
  `egui::Rect` **specifically so a headless test can aim a drag at a point on the curve** rather
  than hardcoding a screen coordinate. That is exactly what §6's wiring test needs.
- `color_wheel::show(…)` plus the Hue/Saturation/Luminance sliders for the selected wheel, then
  Blending and Balance.

### Decisions

- **⚠ `RustroomApp` gains `mask_grade_range`**, mirroring `mask_hsl_channel`. A mask must not share
  the global panel's selected wheel, or switching back to the global panel silently shows a
  different range than it did.
- **⚠ Blending and Balance are carried inside a mask.** They are properties of the *range
  decomposition* shared by all four wheels, not of any one wheel, so omitting them would make a
  masked grade a different control from the global one wearing the same name — the failure this
  project has on record from the Whites slider. Parity costs two sliders. This is the same
  deliberate call M7.2 made for `Sharpening::masking`.
- **⚠ The collapsed state lives in egui memory** via `CollapsingHeader`, so no field on
  `RustroomApp`. Its id path is the same for every mask, so **the open/closed state is shared
  across masks** — recorded as a deliberate choice, not discovered later as a bug: you want the
  block to stay open as you click through masks comparing them.
- **⚠ Only the SELECTED mask draws its adjustments** (`masks.rs:123`), so this is one curve widget
  per frame and the panel cost is per-selected-mask, not per-mask. The collapsed blocks are still
  right — a single selected mask's section is already the tallest thing in the panel.

### Placement

⚠ **All of it lands in `mask_adjust.rs` (60 lines), not `masks.rs` (614).** `masks.rs` is already
over the ~600 line marker this project asks to be reported, and what is growing it is its test
module. `mask_adjust.rs` exists precisely because M7.2 needed somewhere for a mask's controls that
was not the shape-properties file; this is the second thing it holds.

## 5. Cost — measured, not assumed

⚠ **A correction to the shape of the question, found while speccing.** Colour grading builds a
**256-entry** LUT per stage construction; the tone curve builds a **1024-entry** one
(`tone_curve.rs:5`). So the backlog's note — *"grading's own LUT is built per stage from constants,
so a mask carrying one pays a table build per render"* — is right about grading and **understates
the curve, which builds the larger table of the two.** Both are per render, not per pixel.

`bench_masked` gains masked-curve and masked-grade rows, at **fit** and at a **native region**,
matching M7.2's table shape so the numbers are directly comparable:

```
cargo run --release -p app --example bench_masked <raw>
```

Reference points from M7.2 on the same machine (X-T4, 3123×2085 half-size buffer, release, best of
5): neutral 12.7 ms at fit; one masked clarity +10.1; one masked HSL +33.1, since improved to +23.0
by the hue LUT.

⚠ **Measuring is required rather than optional here**, and the reason is this file's own record:
HSL's cost was mis-reasoned **three times**, every one from comparing against colour grading instead
of decomposing — and the hue LUT then found the band weighting at 42% where `cos`/`sin` was under
10%. A third consumer of `ColorGradingStage::new` is exactly the kind of thing that gets estimated
and is wrong.

**No optimisation is in scope.** If a number is bad, it is reported and scoped separately — M7.2's
handling of the bounding box, and the reason the hue LUT became its own milestone rather than
riding along.

## 6. Testing

### The acceptance criterion

**All 55 goldens byte-identical.** Both stages are inert at neutral — an empty `points` vec inserts
no stage, and `ColorGrading::is_active` is wheels-only — so anything that moves is a bug, not a
nicety. Plus one new golden per stage (a masked curve, a masked grade) on the existing chart.

### ⚠⚠ The wiring test is the one that matters

W5's record is the argument, and it is the strongest one in this file: **creative LUTs had four
passing pipeline tests and shipped 100% non-functional**, because nothing proved the table ever
reached the pipeline. *Ask what carries the data to the tested code, and test that.*

Here the carrier is the panel → `LocalAdjustments` → `adjustment_stages` path. So:

- A headless test that **drives the real curve widget inside a mask** and asserts the mask's
  `tone_curve.points` changed — aimed using the `egui::Rect` `tone_curve::show` returns, never a
  hardcoded coordinate.
- A test that a masked grade with a wheel turned up **inserts `ColorGradingStage`** into that
  mask's sub-pipeline, and that a neutral one inserts nothing.

### The sabotages that must be watched failing

1. **Double application** — pass the real values in `build_pipeline_split`'s literal. Must fail a
   named test **and move goldens**. This was the strongest guard in M7.2 and is the strongest here.
2. **Wrong order inside the mask** — move the grade before HSL, or the curve after local contrast.
   Needs a test that pins the sub-pipeline's stage order, since a plausible image comes out either
   way.
3. **The fixture omission** — leave `all_edits` / `all_nonneutral` on `..Default::default()`. ⚠ This
   one compiles and passes; the guard is the reminder comment plus a test asserting the fixtures'
   mask really carries a non-neutral curve and grade.

### ⚠ Read the passed-count on every filtered run

M7.2 hit the false-pass trap **twice in one session, by someone who had just quoted it** — once
because `--bin Rustroom` filtered away a lib module's test (`0 passed; 325 filtered out`, exit 0),
once because a filter matched nothing. `render_engine` and the panel modules sit on opposite sides
of that boundary. `test result: ok` says nothing about what ran.

### ⚠ And run clippy's exit code from a file, not through a pipe

At least the ninth recorded instance of clippy catching what a green suite did not — most recently
an orphaned `///` and an unused `use` left by removing a guard test.

## 7. What this deliberately does not deliver

- **⚠⚠ Masked dehaze and masked noise reduction — Part B, blocked on a design question.** Both
  live in the **prefix**, before geometry, while `MaskStage` lives in the **tail**. So a masked one
  would run *after* the tone curve, denoising or dehazing pixels the curve has already stretched —
  **a different operation from the global stage wearing the same name**, which is the Whites-slider
  failure this project has on record. Dehaze additionally estimates a frame-global airlight from the
  brightest 0.1% of pixels, which has no obvious meaning confined to a region. `docs/BACKLOG.md`
  carries the three ways out and their costs. **B needs a brainstorm before a plan.**
- **⚠ Skin softening is NOT among the gaps**, and thinking it is would be the trap: Texture is
  bipolar and −100 removes the fine band, so M7.2 already covers the commonest reason anyone
  reaches for NR-in-a-mask.
- **No bounding box.** Deferred a fifth time; unchanged from M7.2, where it was deferred on a
  measurement rather than a guess — worth ~17× on a small radial or brush mask and **nothing at all**
  on a gradient or either range mask.
- **No optimisation of either new stage.** §5 measures; it does not tune.
- **A mask's panel section grows again**, mitigated by the collapsed blocks but not removed.
- **`rustroom render` applies a masked grade and curve like any other edit** — no new limitation,
  since both come from `Edits` and need no I/O.
