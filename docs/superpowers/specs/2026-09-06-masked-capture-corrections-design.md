# Masked noise reduction and dehaze — design

**Status:** specced 2026-09-06. Branch `masked-capture-corrections`.

A mask carries **noise reduction** and **dehaze**. Denoise a shadowed corner without smearing the
rest; clear the haze off a distant ridge and leave the foreground alone.

This is **Part B** of `docs/BACKLOG.md`'s *"The rest of the stages inside a mask"*, asked for at
M7.2's merge — *"I'd also like to add the dehaze and rest to masks."* Part A (colour grading and
the tone curve) shipped 2026-09-05. **Part B closes the request.**

---

## 1. ⚠⚠ The recorded blocker rests on a factual error, and correcting it is most of this design

`docs/BACKLOG.md`, `CLAUDE.md`, M7.2's spec §7 and **Part A's own spec** all say the same thing:

> *"Both live in the **prefix**, before geometry; `MaskStage` lives in the tail. So a masked one
> would run after the tone curve … a prefix-side masking stage would have to evaluate coverage at
> prefix resolution **before geometry** — which is a different coordinate space from the one every
> mask is authored in."*

**Noise reduction and dehaze run *after* geometry.** From `build_prefix_steps`, and asserted by its
own tripwire `the_prefix_holds_perspective_spots_geometry_noise_reduction_and_dehaze`:

```
lens correction → perspective → spot removal → GEOMETRY → noise reduction → dehaze
```

The prefix's output **is** the post-geometry image — `CLAUDE.md` says so in the region-rendering
section (*"the prefix output **is** the post-geometry image, so screen→region is a plain scale and
offset with no inverse geometry"*). That is exactly the space every mask is authored in.

**So the coordinate-space objection is dead.** It was the load-bearing half of the blocker, and it
was never true.

⚠ The sentence propagated through four documents, including one written yesterday. It is corrected
in all of them by this milestone.

### The second objection is weaker than it reads

> *"A masked one runs after the tone curve — a different operation from the global stage wearing
> the same name."*

That is already true of **every masked adjustment**. Global exposure runs before the global tone
curve; masked exposure runs after it, because `MaskStage` sits in the tail. It has been true and
accepted since M3.1a. It is not a property of noise reduction.

### ⚠⚠ The real blocker is different, and it is dehaze-only

`MaskStage::apply` runs its sub-stages on **the image it is handed**. In a region render that is
the *region*, not the frame. `dehaze::airlight` takes a whole `ImageBuf` and reads the brightest
0.1% of it.

**So a tail-side masked dehaze would recompute its airlight from whatever is on screen, and the
photograph would change as you panned.** The global dehaze escapes this only because the prefix
always runs whole-buffer — its own code comment says exactly that:

> *"⚠ Being in the prefix is what makes a frame-global airlight possible at all: the prefix always
> runs whole-buffer, so no region plumbing and no margin growth is needed."*

**That is the finding this design is built around, and the placement below dissolves it rather than
working around it.**

### For scale

Dehaze reaches ~80 full-resolution px (`PATCH_RADIUS_FULL` 20 + `REFINE_RADIUS_FULL` 60); noise
reduction ~32 (`COL_RADIUS_FULL` up to 32). Both are **under** clarity's 120, which the tail's
derived margin already handles — so reach was never the problem either.

## 2. The model

`LocalAdjustments` gains two fields, embedding the existing structs whole — the rule M7.2 and Part A
both followed, so a mask's control means what the global control of the same name means:

```rust
pub noise_reduction: NoiseReduction,
pub dehaze: Dehaze,
```

Both are `Copy`, so nothing structural changes. Part A already paid the `Copy` cost when `ToneCurve`
arrived, and there is nothing left to pay.

Both are additive under the container-level `#[serde(default)]`, so existing sidecars read them as
neutral. **No `Edits::version` bump.**

⚠ `NoiseReduction::is_active` and `Dehaze::is_active` are **amount-only** — Detail and Smoothness
are modifiers that shape an effect which is off until Luminance or Color is non-zero. Reused, not
reinvented.

## 3. ⚠⚠ The split, and the discipline that makes it self-checking

A mask's adjustments are now built by **two** functions:

| function | builds | runs |
|---|---|---|
| `capture_stages(a, scale)` | noise reduction, then dehaze | **prefix** |
| `adjustment_stages(a, baseline, scale)` | everything else (unchanged) | **tail** |

`MaskStage` gains a second constructor — `new_capture` — identical to `new_in_region` except that it
calls `capture_stages` and always takes `region: None`. **Same type, same coverage code, two
clearly-named entry points.**

**The neutral-pass discipline now runs in both directions, which is what keeps it honest:**

- the **prefix** builds its capture `MaskStage` from a **projection**: a `MaskedAdjustment` whose
  `LocalAdjustments` keeps *only* `noise_reduction` and `dehaze` and is neutral everywhere else;
- the **tail's** exhaustive literal in `build_pipeline_split` passes those two **neutral**, forced
  at compile time by `E0063` exactly as Part A was.

⚠ `capture_stages` orders noise reduction before dehaze, mirroring the global prefix for the reason
recorded there: *"dehaze is a contrast stretch that amplifies noise."*

⚠ A capture `MaskStage` is inserted **only when that mask's NR or dehaze is active**, so a mask
carrying only tail adjustments costs nothing in the prefix.

## 4. Placement

```
lens → perspective → spots → geometry → noise reduction → dehaze → [capture masks]  ||  tail …
```

After the global pair, mirroring the tail's own rule that a mask acts on the global look rather than
being re-mapped by it.

**`Region` is always `None` here.** The prefix is whole-buffer, which is precisely what makes the
frame-global airlight correct *and stable under pan*. The blocker is answered by placement.

**`tail_kernel_radius` is untouched.** Nothing new lands in the tail, so nothing new reaches across
a region edge and the derived margin needs no change.

## 5. The cache key

```rust
PrefixStepKey::MaskedCapture(Vec<MaskedAdjustment>)
```

holding **the same projected values the stage was built from**, so the key and the stage cannot
drift — the property `build_prefix_steps` exists to guarantee, since `PrefixKey` is *defined
through* it. `PrefixStepKey::SpotRemoval(Vec<Spot>)` is the precedent for a `Vec` in a key.

⚠⚠ **The projection is what stops the prefix cache thrashing, and it is the most important detail
in this document.** Keying on the whole `MaskedAdjustment` would make **every masked-exposure drag
re-run masked noise reduction** — a whole-buffer 117 ms stage at native, on a slider that has
nothing to do with it. The test must pin **both directions**: the key changes when a mask's NR
changes, and does **not** change when its exposure does.

⚠ `PrefixKey::new` passes `brush: None` when deriving the key, exactly as it already passes
`lens: None`, and for the same reason: *a coverage map is needed to run the stage, never to key
it.* Construction stays cheap because `CoverageSource::Painted` only clones the stroke list —
rasterisation happens in `apply`.

## 6. ⚠ Accepted: a range mask selects on pre-tonal pixels in its prefix half

`MaskStage` computes coverage from **the buffer it is applied to**. The analytic masks are
normalised geometry and are unaffected. A **luminance or colour range mask** selects on pixel
values, so its prefix half selects on pre-tonal pixels and its tail half on post-tonal ones — one
mask, two selections.

**Accepted (2026-09-06), with the reasoning recorded rather than left to be rediscovered:**

- Noise reduction and dehaze are corrections of the **capture**, so selecting them on scene-linear
  tones is arguably the *more* correct answer rather than a compromise.
- It is what the global stages already do: global NR is applied to pre-tonal data too.

⚠ It gets a **test that pins the behaviour**, so it is a documented property rather than something
found later and filed as a bug.

## 7. Panel

**Mirroring the global panel, not the pipeline** (user's call, 2026-09-06):

- **Dehaze** beside Texture and Clarity, where the global `tone` section puts it.
- **Noise reduction's four sliders** (Luminance, Color, Detail, Smoothness) beside Sharpening at
  the bottom, where the global Detail section puts them.

⚠⚠ **This breaks `mask_adjust.rs`'s own stated rule and the comment must be rewritten to say why.**
That file says *"the order mirrors the global chain … it is the order `adjustment_stages` inserts
them in, so what you read top to bottom is what runs."* `CLAUDE.md` records the opposite for these
two stages specifically:

> *"Panel order deliberately does NOT follow pipeline order … The pipeline runs NR and dehaze
> before white balance because that is where the maths wants them; NR is judged last, at 1:1, once
> tone is settled. Panel follows workflow, pipeline follows maths."*

Two recorded rules in genuine conflict; the global panel's is the one that wins, because *"one
layout learned once"* is what stops the two panels naming the same controls differently.

## 8. Two stale things this fixes in passing

- **`MaskStage::new_in_region` says *"No spatial sub-stage yet: every adjustment a mask can carry is
  per-pixel"*, with a dead `let _ = scale;` beneath it.** Both have been false since M7.2 put local
  contrast and sharpening inside a mask, and `scale` is in fact used on the next line.
- **The "before geometry" sentence**, in `BACKLOG.md`, `CLAUDE.md`, M7.2's spec and Part A's spec.

## 9. Testing

### The acceptance criterion

**All 57 goldens byte-identical**, plus one masked-NR and one masked-dehaze golden. Both stages are
inert at neutral (`is_active` is amount-only), so anything that moves is a bug.

### ⚠⚠ The strongest guard is one that already exists

Add masked NR and dehaze to **`a_region_render_matches_the_whole_render_cropped`**'s edit set. If
the airlight — or anything else here — ever leaked into a region-dependent path, that test fails.
**It is the whole reason for the prefix placement, so it is the test that proves the placement.**

### The rest

- **The projection test**, both directions (§5).
- **The double-application guard extended from five fields to seven.** ⚠ Part A found it named three
  of five and was therefore blind to a whole milestone; it is now the guard most likely to be left
  behind again, and Part B is its next test.
- **The carrier test**: the panel's sliders reach the mask's own fields, driven through the real
  `mask_adjust::show`. W5's record is why.
- **A range-mask test** pinning the pre-tonal selection of §6.
- **Order**: the capture stages run after the global dehaze, in `edits.masks` order.

### The sabotages that must be watched failing

1. Pass the real NR/dehaze from the global prefix caller → double application.
2. Key on the whole `MaskedAdjustment` → the projection test's "does not change" half fails.
3. Give the capture `MaskStage` a `Region` → the region invariant fails.
4. Draw the panel sliders against a scratch value → the carrier test fails.

## 10. Cost

Whole-buffer, from `CLAUDE.md`'s existing table: NR 50/50 is **31.4 ms at fit, 117.4 at native**;
dehaze 60 is **32.3 / 115.6**. So a masked one is the most expensive thing a mask can carry — but
**it is in the prefix, so it is cached**: paid when its own slider moves, not on every frame.

`bench_masked` gains masked-NR and masked-dehaze rows. ⚠ Its existing loop times `render_once`,
which builds the prefix fresh each time, so it measures the **cold** cost; the cached cost is the
one a user feels and the bench should say which it is reporting.

**No optimisation is in scope.** If a number is bad it is reported and scoped separately — M7.2's
handling of the bounding box, and the reason the hue LUT became its own milestone.

## 11. What this deliberately does not deliver

- **No bounding box** — deferred a sixth time. ⚠ Note it would help *more* here than anywhere: a
  capture mask runs whole-buffer by construction, so a small radial pays for the entire frame at
  full prefix cost. That is the strongest case yet made for it, and it is still not this milestone.
- **No change to the tail**, and therefore none to `tail_kernel_radius`.
- **Skin softening was never the reason for this milestone** — Texture at −100 already covers it,
  as M7.2 recorded. Part B is for genuine noise and genuine haze.
- **The range-mask asymmetry of §6 ships**, documented and tested rather than removed.
