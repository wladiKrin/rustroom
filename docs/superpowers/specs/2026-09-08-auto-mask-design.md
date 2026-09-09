# Auto-mask for the brush — design

**Status:** specced 2026-09-08. Branch `m7.6-auto-mask`. The **last M7.6 gap**; M7.6 closes with it.

Lightroom's Auto Mask: while it is on, a brush dab paints only the pixels perceptually close to the
colour that was under the brush centre when that dab was laid down. A stroke along a sky stops at
the ridge; a stroke on a face stops at the hair.

⚠ **This is canvas work, and this file's record says every defect in canvas wiring is found by use
and none by a test** (M3.1c-B six, M5.3b five rounds, the geometry work four). A GUI round is part
of the plan, not a nicety.

---

## 1. Measured first — three numbers decide three open questions

Per the user's instruction and this project's standing rule. **None of the three is guessed in this
spec**; each names the experiment and what each outcome implies.

| # | question | instrument | what it decides |
|---|---|---|---|
| **M1** ✅ | How far apart are the **reference's space** and the **comparison space**? | `auto_mask_space_probe`, kept | **MEASURED 2026-09-08 — the L term is unusable and the chroma terms are fine. See §3.** |
| **M2** | What does disabling `BrushCache` cost per render? | `bench_masked`, a new row: one auto-masked brush at fit and at a 1:1 region | whether the cache must be made auto-mask-aware, or the existing self-rasterising fallback is enough — see §6 |
| **M3** | Does the mask **drift** when a global slider moves after painting? | paint an auto-masked stroke on a real photograph, then sweep exposure ±2 EV and measure coverage change | the question deferred in brainstorming: whether a stable pre-tail buffer is worth its plumbing — see §9 |

⚠ **M1 is the one that can invalidate the design**, so it ran first and its result gated the rest.

### ⚠⚠ M1's result, and the two probe bugs found on the way to it

Four X-T4 frames (DSCF6223, 6296, 6509, 6270), 1600 px, bucketed by displayed Oklab lightness.
**With the camera rendering on — which is what the canvas opens every photo with:**

| displayed L | mean ΔL | mean chroma distance |
|---|---|---|
| 0.0–0.1 | **−0.075 … −0.083** | 0.006–0.008 |
| 0.2–0.3 | −0.015 … −0.001 | 0.013–0.040 |
| 0.4–0.5 | +0.076 … +0.080 | 0.014–0.026 |
| 0.6–0.7 | **+0.140 … +0.143** | 0.002–0.007 |
| 0.8–0.9 | +0.071 … +0.119 | 0.003–0.013 |
| **middle six buckets** | **+0.070 … +0.111** | **~0.010–0.020** |

**ΔL is 7–10× the chroma distance, and its curve is nearly identical on all four frames** —
negative in the deepest shadows, crossing zero near displayed L ≈ 0.28, peaking around +0.14 in
the upper midtones. That shape is the display rendering's own S-curve, and its being
**content-independent** is what makes this the table's second outcome rather than its third.

⚠⚠ **TWO PROBE BUGS WERE FOUND BEFORE THIS NUMBER WAS BELIEVED, AND EITHER WOULD HAVE BEEN
REPORTED AS A FINDING.**

1. **A double encode.** The first probe applied `linear_to_srgb` to the tail's output — but
   `OutputTransform::apply` **already encodes** (`space.encode`). That produced a flat **+0.24**
   offset: a probe bug wearing exactly the shape of the result it was looking for.
2. **⚠⚠ The wrong configuration, which is the more instructive one.** With the bug fixed the
   offset came back at **0.0011 — quantization noise** — and that reads as "the two spaces agree,
   ship it". It is measuring nothing: `Edits::default()` carries `rendering: None`, so that tail
   holds **no display rendering at all**. The canvas opens every photo with
   `Settings::default_rendering`, which is `Camera`. **A neutral fixture removed the very stage the
   measurement was about.** The probe now reports both rows side by side so the distinction cannot
   be lost again — the rendering-off row is the CLI's and the goldens' case and is genuinely ~0.

M2 and M3 remain tuning and scope.

---

## 2. ⚠⚠ THE ARGUMENT THIS SECTION ORIGINALLY MADE WAS WRONG, AND ITS OWN SABOTAGE IS WHAT SHOWED THAT

**Retracted 2026-09-09, during implementation.** This section claimed that sampling each dab's
reference at render time was `dehaze::airlight` in a new place — that a dab panned off-region would
have no reference, so coverage would depend on the viewport and
`a_region_render_matches_the_whole_render_cropped` would break. **That was the load-bearing
justification for the whole design, and it does not hold.**

**The sabotage was attempted three times and passed every time.** Not a fixture fault in the end;
the mechanism explains it:

1. A dab whose disc misses the clip is **dropped**, so it contributes nothing either way.
2. A dab kept with its centre *outside* the clip lies within **one radius** of the boundary — a
   narrow band, not the whole mask.
3. ⚠⚠ **Within a stroke, dabs combine with `max`, and they are spaced `SPACING` — a quarter
   radius — apart.** So essentially every pixel in that band is *also* covered at full alpha by
   dabs whose centres are inside the region and whose references are right. The few wrong ones are
   washed out.

Two of the other reasons given for baking evaporated in the building as well. **`BrushCache` had
to be disabled for a gating brush either way**, because the gate reads pixels per-pixel whatever
the reference's origin — so it was never an argument between the two designs.

⚠⚠ **And the sting: baking is what forced §3's compromise.** A stored reference lives in the
*displayed* space while the pixels are in the working space, which is exactly why the lightness
term measured unusable and the gate was briefly chroma-only and blind to tonal edges. **Reading
both ends from one buffer removes the disagreement rather than working around it**, and needs no
`refs` in the model, no sampling in the gesture, and no `sample_reference` — less code and a
strictly more capable tool.

**So the reference is read at render time, from the pixel under each dab's own centre** (user's
call, 2026-09-09, after this retraction was reported).

### What it costs, pinned rather than hoped

⚠ A dab whose centre lies outside the rendered region has no pixel there. `BaseView` **clamps to
the nearest edge pixel** rather than answering black, so the error is small and local instead of
absurd. The residual is bounded by `an_auto_masked_clipped_raster_agrees_away_from_the_clip_edge`,
which asserts exact agreement outside a one-radius band and deliberately promises nothing inside
it. ✅ The **master region invariant passes** with an auto-masked brush under this design — checked,
not assumed.

⚠ The reference is *the colour that is there now*, so the mask re-evaluates as upstream edits move
the pixels. That is Lightroom's own semantic, and the opposite of the baked design's "the colour
you painted against". It is the one property baking genuinely had, and it was given up knowingly.

---

## 3. The comparison space — and the measurement that now argues AGAINST storing a reference

Both ends of the comparison are read from **the same buffer**: the pixel at this position, and the
pixel under the dab's own centre, both linear ProPhoto, both converted with
`color::oklab::prophoto_to_oklab`. There is no space mismatch to reason about, so the gate uses the
**full Oklab distance including lightness** — which is what lets it see a purely tonal edge.

⚠⚠ **M1 is still the reason this is right, it has simply changed sides.** It measured the
disagreement between the displayed frame (where a stored reference would come from) and
`MaskStage`'s input (where the pixels come from):

| displayed L | mean ΔL | mean chroma distance |
|---|---|---|
| 0.0–0.1 | **−0.075 … −0.083** | 0.006–0.008 |
| 0.4–0.5 | +0.076 … +0.080 | 0.014–0.026 |
| 0.6–0.7 | **+0.140 … +0.143** | 0.002–0.007 |
| **middle six buckets** | **+0.070 … +0.111** | **~0.010–0.020** |

ΔL is 7–10× the chroma distance and its curve is nearly identical on all four frames — the display
rendering's S-curve, not the picture. **A stored reference would therefore have had to ignore
lightness**, and the tool would have been blind to a white wall against a grey one.
`auto_mask_space_probe` is kept, and `auto_mask_gate`'s doc points at it so nobody reintroduces a
stored reference without re-reading it.

⚠⚠ **TWO PROBE BUGS WERE FOUND BEFORE THAT NUMBER WAS BELIEVED, AND EITHER WOULD HAVE BEEN
REPORTED AS A FINDING.**

1. **A double encode.** The first probe applied `linear_to_srgb` to the tail's output — but
   `OutputTransform::apply` **already encodes**. That produced a flat **+0.24**: a probe bug
   wearing exactly the shape of the result it was looking for.
2. **⚠⚠ The wrong configuration, which is the more instructive one.** With that fixed the offset
   came back at **0.0011 — quantization noise** — which reads as "the two spaces agree, ship it".
   It was measuring nothing: `Edits::default()` carries `rendering: None`, so that tail holds **no
   display rendering at all**, while the canvas opens every photo with `Camera`. **A neutral
   fixture had removed the very stage the measurement was about.** The probe reports both rows
   side by side now.

---

## 4. Model — `edit-model`

`BrushStroke` gains **two** fields, joining `radius`/`hardness`/`flow`/`erase` as things baked at
paint time:

```rust
/// Paint only the pixels close to the colour under the brush centre. Baked at paint time.
pub auto_mask: bool,
/// How close counts, 0..=100. Larger selects more.
pub tolerance: f32,
```

⚠ **No reference colour is stored** — see §2. An earlier draft of this spec added
`refs: Vec<[f32; 3]>` parallel to `points`; it was removed when the argument for it was retracted,
which also removed the model's only alignment hazard and kept sidecars smaller.

⚠ `BrushStroke::Default` is **hand-written already** and absorbs both, so every existing sidecar
and all 59 goldens are untouched. That byte-identity is the acceptance criterion, not a nicety —
the same proof `flow` needed. ⚠ Sabotage-verified: a *field-level* `#[serde(default)]` fails the
guard with `left: 0.0, right: 35.0`, which is the trap `write_xmp` and `snapshot_strip_height` both
document.

⚠ **No `EditGroups` change.** A `BrushStroke` lives inside a `Mask` inside `Edits.masks`, which
travels whole under `MASKS_GROUP`. ⚠⚠ **And the three exhaustive fixtures do NOT apply here, which
is a correction to this spec's own earlier claim**: `pipeline::all_edits`,
`edit_model::all_nonneutral` and `edit_clipboard`'s payload all build their masks as `Mask::Linear`
or `Mask::default()` — **none contains a `Mask::Brush`** — and these are not `Edits` fields anyway.
M7.2's finding applied because *its* fields were on `LocalAdjustments`, which those fixtures
construct directly. Coverage went into `a_brush_round_trips_through_json` instead.

`Brush::needs_base()` is true when any stroke is auto-masked; §6 is what it is for.

---

## 5. The gate — `pipeline/src/brush/raster.rs`

Per-dab coverage becomes

```
dab(p) · gate(ΔE(base(p), ref_dab), tolerance)
```

applied **before** the per-stroke `max`, which is what per-dab references force and is the reason
this cannot live in `MaskStage`'s pixel loop beside the range masks.

- **ΔE is Euclidean distance in Oklab** over (L, a, b), with L included or dropped per M1.
- **The gate is a smoothstep, not a hard cut.** `1 − smoothstep(t·(1−SOFT), t, ΔE)`, so coverage
  falls off across a band rather than at an edge. ⚠ A hard cut would alias badly along a ridge —
  and this file already records `FEATHER_EPS` and `HARDNESS_EPS` as the family of guards a hard
  boundary needs; a continuous gate needs none of them.
- **`tolerance` maps to a ΔE ceiling by a constant swept at implementation time, not chosen here.**
  ⚠ The vignette took three revisions before its feel was right and the colour range mask's
  constants survived first contact; neither outcome is predictable from a spec. The sweep is a
  named task.

**Seam:**

```rust
/// Linear working-space RGB at a **frame** pixel, for the auto-mask gate.
pub trait BasePixels {
    fn at(&self, x: usize, y: usize) -> [f32; 3];
}

pub fn rasterize_in(
    brush: &Brush, geometry: &Geometry,
    frame_w: usize, frame_h: usize,
    clip: Option<(usize, usize, usize, usize)>,
    base: Option<&dyn BasePixels>,
) -> CoverageMap
```

`rasterize` and the old `rasterize_in` become thin `None` wrappers, so the existing call sites and
every brush test compile with a one-line edit — the `Region` precedent, which bought "zero
call-site churn" twice before.

⚠⚠ **The dangerous state is `base: None` with auto-mask strokes present**, which would render
positional-only while the tint showed edge-stopping: a silent divergence between preview and
export, which is the failure mode this repo has recorded more than any other. **It is closed
structurally, not by a comment** — see §6.

---

## 6. Where the base comes from, and the cache

`MaskStage::apply` already rasterises its own map when handed no prebuilt one (`mask.rs:231`) —
the path the CLI and export walk. **At that point `image` is in hand and has not yet been cloned**,
so the sampler is a thin view over it, offset by the region origin exactly as the existing `clip`
is. No new buffer, no new allocation, and preview / CLI / export agree by construction because all
three take this path.

⚠ **`render_engine`'s `BrushCache` must therefore refuse to build a map for a brush that needs
one.** `Brush::needs_base()` is true when any stroke is auto-masked; the cache returns `None` for
such a brush, so `CoverageSource::Painted { prebuilt: None }` routes to the fallback above. A named
test asserts the cache returns `None` there, and its sabotage — returning a positional map — must
fail it. **This makes the `base: None` hazard unrepresentable in the render path rather than
documented.**

⚠ The cost is that an auto-mask brush re-rasterises on every render. `rasterize_in`'s `clip` and
`MaskStage::new_in_region` both already exist, so this rides the **region** path — the M3.1c-B
record puts whole-frame at ~24 ms and region-clipped at ~1.6 ms at 1:1. **M2 measures it.** If it
is material, the answer is to widen `BrushCache`'s key with a cheap identity of the base buffer,
not to reintroduce the unkeyed map.

---

## 7. Canvas

⚠ Placement per `CLAUDE.md` §5 throughout: **testable logic in the lib, the gesture in the bin.**

- **Reference sampling** — `brush_tool::sample_reference(readout, dims, region, frame_pt) ->
  Option<[f32; 3]>`, pure, in the lib. The gesture in `develop/canvas/mask_overlay.rs` calls it
  once per appended point and pushes into `refs`. ⚠ `None` when the point falls outside the
  rendered region; the dab then stores no reference and paints positionally, which is the same
  degradation §4 specifies for a truncated `refs` — **one rule, not two.**
- **Tint** — `mask_tint::paint_tint` takes the `readout_bytes` `Arc` the app already retains for
  the colour readout and applies the same gate, so the stroke is seen stopping at the ridge while
  it is painted. ⚠ Its cache key gains a **buffer identity** (`Arc::as_ptr`) so the tint refreshes
  when a render lands. ⚠⚠ **The key must not gain a screen rect** — M3.1c-B's sixth bug was a
  cached screen rect surviving a pan, itself a regression of 3.1a. *Cache the expensive thing,
  never the placement.*
  - ⚠ **This threads a new argument through four signatures, and `mask_tint`'s own doc says why
    that was avoided before.** `readout_bytes` lives on `RustroomApp` (`main.rs:873`), while the
    tint's memo is a `thread_local!` precisely so nothing had to be threaded down to a free
    function reached from inside closures already holding `&mut app`. **The buffer is data the
    tint cannot do without**, unlike the memo, so it goes down the chain — `canvas::show` →
    `mask_overlay` → `paint_tint`. ⚠ If that pushes any of them past clippy's argument limit, the
    answer is this project's standing one: a params struct of borrows, as `FrameSpec`, `NameField`,
    `AspectLock` and `GradeUi` each already are — **not** a lint allowance.
- **Panel** — an `Auto Mask` checkbox and a `Tolerance` slider in `develop/panel/masks.rs`, beside
  Size / Hardness / Flow, written into the stroke at paint time. ⚠ `masks.rs` is **624 lines** and
  already over the ~600 marker; the tool-settings row is what grows, and this is reported rather
  than silently added to.

---

## 8. Tests

- **Goldens: one new, `mask_brush_auto`**, on a chart with a hard colour edge the stroke crosses,
  so a gate that never fires and one that always fires produce visibly different references. ⚠
  **The 59 existing must stay byte-identical**, which is the proof `auto_mask: false` is inert.
- ⚠ **A golden alone cannot prove the gate is wired**, the W5 creative-LUT lesson: a stage that
  never runs produces a reference identical to the ungated render and the golden passes against it
  forever. So `an_auto_masked_stroke_really_stops_at_the_edge` renders the same stroke with and
  without `auto_mask` and asserts a substantial fraction of samples differ.
- **A carrier test**, likewise from W5: the panel's checkbox and slider must be proven to reach
  `BrushStroke`, and the sampled reference proven to reach `refs` — driving the real gesture, not a
  scratch struct.
- **A headless `RawInput` drag-lifecycle test.** ⚠ The record is explicit that these would have
  caught **three of M3.1c-B's six** canvas bugs and that the seam exists and is simply unused for
  the canvas.
- **A region test**: an auto-masked brush must satisfy
  `a_region_render_matches_the_whole_render_cropped`. That is the guard on §2's whole argument, and
  its sabotage is to sample the reference at render time — which must fail it.
- **Sabotage every one**, and confirm the *named* test fails rather than that some suite passes.
  ⚠ This file records at least six ways a sabotage reports a false result; check the sabotage
  compiled and that a `test result:` line appeared.
- **⚠ A GUI round.** Sky-to-ridge, face-to-hair, an erasing auto-masked stroke, tolerance at both
  ends, and a **1:1 pan with an auto-masked brush active** — the last is what §6's region path and
  §2's whole argument exist for, and no test fully stands in for it.

---

## 9. Open, and deliberately not settled here

- **M3 decides whether the comparison buffer moves to a stable pre-tail one.** As specced the mask
  is compared against the global tail's output, so pushing exposure after painting moves every
  pixel while the stored reference stays put. ⚠ The M3.1c-A argument — *a gradient is
  compositional, a stroke is content-referred* — says a stroke's selection should be stable; the
  range masks say the opposite is livable. **Measure, then decide**, and the plumbing (handing
  `MaskStage` the prefix output) is a separate change if it is wanted.

## 10. Out of scope, stated so it is not later filed as an omission

1. **No flood fill and no connectivity** — a dab-local gate, as Lightroom does it. A stroke that
   crosses a ridge still paints on both sides of it wherever the colours match.
2. **No edge detection or guided refinement** of the coverage map.
3. **An erasing stroke gates the same way**, which is the simple rule and not obviously the right
   one for every case.
4. **The tint samples the post-mask displayed frame**, so it is an approximation of the render's
   pre-mask comparison; the size of that gap is M1's second output.
5. **Outside the rendered region at zoom the tint falls back to positional**, since there is no
   pixel data there.
6. **No auto-mask on the analytic masks or the range masks** — they are not painted.
7. **M7.1c boolean mask operations and M7.3b remain next**, untouched.
8. ~~A purely tonal edge is not detected~~ — **no longer a caveat.** It was one only while the
   reference was stored; reading both ends from one buffer restores the lightness term, and
   `the_gate_sees_a_purely_tonal_edge` pins it.
9. **⚠ Within one dab radius of a rendered region's edge, coverage is approximate** — a dab whose
   centre falls outside the region references a clamped edge pixel. Bounded by
   `an_auto_masked_clipped_raster_agrees_away_from_the_clip_edge`, and washed out in practice by
   `max`-combining over densely spaced dabs; the master region invariant passes.
10. **⚠ The mask re-evaluates against current pixels**, so a large upstream change moves what it
    selects. Lightroom's own semantic, and the property the retracted baked design would have had
    instead.
