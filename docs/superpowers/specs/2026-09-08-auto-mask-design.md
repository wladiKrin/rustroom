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
| **M1** | How far apart are the **reference's space** and the **comparison space**? | a probe rendering a real `.RAF`, dumping the Oklab of a chosen pixel as (a) `readout_bytes` decoded, (b) `MaskStage`'s incoming `base` | whether a reference sampled from the displayed frame can be compared against the pre-mask buffer at all — see §2 |
| **M2** | What does disabling `BrushCache` cost per render? | `bench_masked`, a new row: one auto-masked brush at fit and at a 1:1 region | whether the cache must be made auto-mask-aware, or the existing self-rasterising fallback is enough — see §6 |
| **M3** | Does the mask **drift** when a global slider moves after painting? | paint an auto-masked stroke on a real photograph, then sweep exposure ±2 EV and measure coverage change | the question deferred in brainstorming: whether a stable pre-tail buffer is worth its plumbing — see §9 |

⚠ **M1 is the one that can invalidate the design**, so it runs first and its result is a gate on
Task 2 of the plan. M2 and M3 are tuning and scope.

---

## 2. ⚠⚠ THE FINDING THAT DECIDES THE ARCHITECTURE: A RENDER-TIME REFERENCE IS THE DEHAZE AIRLIGHT AGAIN

Per-dab references (the user's choice) can be obtained two ways, and only one of them survives a
region render.

**B — sample at render time.** Store nothing; each dab reads its reference out of the base buffer
at its own centre when the pipeline runs. Reference and pixel are then in the same space *by
construction*, which is attractive.

⚠⚠ **And it fails for exactly the reason masked dehaze was blocked for two milestones.**
`MaskStage::apply` runs on **the buffer it is handed**, which under a region render is the
*region*. A dab whose centre has been panned off-screen has no reference to read, so its coverage
would be computed differently — or not at all — depending on where the viewport happens to be.
**That is "the photograph changes as you pan"**, the same failure `dehaze::airlight` has, arriving
in a new place. It would also break `a_region_render_matches_the_whole_render_cropped`, which is
this repo's strongest single guard.

⚠ Dehaze's answer was to move into the **prefix**, which always runs whole-buffer. That escape is
**not available here**: brush coverage is resolved in `MaskStage`, in the tail, and moving the
masks loop into the prefix is a different and much larger milestone.

**A — bake the reference into the stroke.** Each point stores the colour sampled under the brush at
the moment it was laid down; it travels in `Edits`. **A reference that travels in the op-list is
available under any region, in the CLI, and in export, because it is not sampled at render time at
all.** The region invariant is untouched.

**A ships.** ⚠ Its cost is that the sampled reference and the compared pixel are in *different
spaces* — §3 — and that cost is measurable where B's is structural.

---

## 3. The comparison space, and the one thing M1 must settle

The working space is **linear ProPhoto**. The two ends of the comparison sit at different points in
the chain:

- the **pixel**: `MaskStage`'s incoming buffer — the global tail's output for a tail mask, i.e.
  *before* the display rendering S-curve, the creative LUT and the output transform;
- the **reference**: sampled at paint time from `RustroomApp.readout_bytes` (`main.rs:873`, an
  `Arc<Vec<u8>>` plus dims, already retained for the colour readout at no extra memory), which is
  the **displayed** frame — *after* all three.

Both convert into **Oklab**, which is absolute, so the comparison itself is well defined:
`prophoto_to_oklab` for the pixel, `srgb_to_linear` then `linear_srgb_to_oklab` for the reference.
⚠ Storing the reference as **display-referred sRGB 0..1** is the `LuminanceRange` precedent — that
type stores `linear_to_srgb(luminance(rgb)) * 100` and does the conversion at the single call site
in `pipeline::stages::mask`, keeping `edit-model` free of a `color` dependency. Same division here.

⚠⚠ **But Oklab being absolute does not make the two ends comparable, because the S-curve has moved
one of them.** A scene value near mid-grey renders close to where it started; shadows and
highlights are compressed hard. A systematic offset in L between reference and pixel biases a
*threshold*, which is what the tolerance is.

**M1 measures that offset.** Three outcomes, decided in advance so the measurement cannot be
rationalised after the fact:

- **offset small across the tonal range** (say under ~0.02 in Oklab L over the middle two thirds):
  ship as specced, record the residual as a caveat.
- **offset large but monotonic**: compare on **chroma and hue only**, dropping L from the distance.
  ⚠ This weakens the tool exactly where the M7.6 brief said it matters — a grey rock against a blue
  sky is a *chroma* edge and would still work, but sky-against-brighter-sky would not.
- **offset large and content-dependent**: the reference must be sampled from a buffer in the
  pixel's own space, which means the render worker hands the canvas that buffer — the "exact and
  live" option costed in brainstorming and declined. **Stop and report**, do not build it silently.

---

## 4. Model — `edit-model`

`BrushStroke` gains three fields, joining `radius`/`hardness`/`flow`/`erase` as things baked at
paint time:

```rust
/// Paint only pixels close to the colour under the brush centre. Baked at paint time.
pub auto_mask: bool,
/// How close counts, 0..=100. Larger selects more.
pub tolerance: f32,
/// One reference colour per point, parallel to `points`. **Display-referred sRGB 0..1.**
///
/// ⚠ Empty unless `auto_mask` — a non-auto stroke stores nothing, so no existing sidecar grows.
/// ⚠ Parallel to `points` rather than a field on a point struct: `points: Vec<[f32; 2]>` is the
/// serialized shape and changing it would churn every sidecar for a field most strokes lack.
pub refs: Vec<[f32; 3]>,
```

⚠ `BrushStroke::Default` is **hand-written already** and absorbs the three, so every sidecar
written before today loads unchanged and all 59 goldens stay byte-identical. That byte-identity is
the acceptance criterion for Task 1, not a nicety — the same proof `flow` needed.

⚠ **`refs` and `points` must not be allowed to disagree in length.** The rasteriser takes
`refs.get(i)` and treats a missing entry as "not auto-masked for this dab", which degrades to plain
positional coverage rather than panicking on a hand-edited sidecar — the forgiving posture every
loader here takes. A test pins that a truncated `refs` is survivable.

⚠ **No `EditGroups` change.** A `BrushStroke` lives inside a `Mask` inside `Edits.masks`, which
travels whole under `MASKS_GROUP` — M7.2 recorded that `merge_from` destructures `Edits`, not
`LocalAdjustments`, and the same holds a level deeper. ⚠ **The three exhaustive fixtures
(`pipeline::all_edits`, `edit_model::all_nonneutral`, `edit_clipboard`'s payload) must still be
checked by hand**: M7.2's sharpest process finding was that *a construct that names exhaustiveness
does not deliver it through a nested struct* — those three build their brushes with
`..Default::default()`, so three fields whose whole purpose is to be non-neutral would land
untested, silently.

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
