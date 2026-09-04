# Masked spatial adjustments (M7.2) — design

**Status:** specced 2026-09-04. Branch `m7.2-masked-spatial`.

A mask carries the **tail-native spatial stages** — Texture, Clarity and Sharpening — in addition
to the per-pixel adjustments it already carries. Sharpen the eyes, soften the skin, add clarity to
the sky.

⚠ **Masked noise reduction and dehaze are a separate later slice**, for a structural reason given
in §7, not for lack of effort. M7.2 does not close with them outstanding — this is M3.1's a/b/c
shape, and the milestone's own "Done when" should be read against the tail three.

---

## 1. What already works, and why this is smaller than it looks

`MaskStage` clones the image, runs its sub-stages on the clone, and blends by coverage. **A spatial
stage dropped into that list works mechanically, and its semantics are already right**: the effect
reads neighbours from *outside* the mask, which is what avoids a hard discontinuity at the mask's
edge. Lightroom's local sharpness behaves the same way.

So this milestone is not "make masks spatial". It is three narrower things:

1. `LocalAdjustments` grows two fields, and one shared builder learns two stages (§2).
2. The region render's margin stops being a fixed constant (§4) — **the only part that is
   structurally new**.
3. The cost is measured before anything is optimised (§5).

## 2. The model

`LocalAdjustments` gains two fields, **embedding the existing structs whole**:

```rust
pub struct LocalAdjustments {
    // ... existing per-pixel fields ...
    pub local_contrast: LocalContrast,   // texture, clarity
    pub sharpening: Sharpening,          // amount, radius, detail, masking
}
```

No new parameter types and no divergence between local and global. **Parity is therefore free** —
a mask carries all four sharpening controls, exactly as the global panel does.

⚠ `Sharpening::masking` is kept despite being, in this context, a second gate inside a gate: the
mask has already answered *where*. It is retained because parity costs nothing here and because an
edge gate is occasionally defensible for a loose mask over mixed content. Recorded as a deliberate
choice, not an oversight.

### The exhaustive sites — checked, not assumed, and only one of them fires

An earlier draft of this spec claimed all four exhaustive constructs would refuse to compile. **That
was wrong**, and checking it produced the more useful answer:

- ✅ **`build_pipeline_split`'s own `LocalAdjustments` literal is exhaustive and WILL break.** It is
  the global tail's caller — the exact site that must pass the two new fields as neutral. **So the
  compiler forces §3's rule rather than leaving it to be remembered**, which is the best possible
  place for that decision to be made.
- ⚠ **`merge_from` will NOT break, and that is correct.** It destructures `Edits`, not
  `LocalAdjustments`; a mask travels whole under `MASKS_GROUP`, so a masked sharpening rides along
  with its mask for free.
- ⚠⚠ **`pipeline::all_edits` and `edit_model::all_nonneutral` will NOT break either — and that IS a
  gap.** Both build their `LocalAdjustments` with `..Default::default()`, so two fields whose whole
  purpose is to be non-neutral would land **untested by the very fixtures that exist to force a
  decision**, silently. They must be updated by hand and **nothing will remind anyone**. This is the
  project's own recorded failure shape — a construct that names exhaustiveness and does not deliver
  it through a nested struct — and it is a finding about those fixtures, not only about this task.

## 3. The pipeline, and the trap that is already solved

`per_pixel_stages` has **two consumers**: the global tail, and every mask. Adding spatial stages to
it naively would make the global tail apply them **twice**, since `build_pipeline_split` already
places `LocalContrastStage` and `SharpenStage` itself.

⚠⚠ **That exact problem is already solved in that function, for saturation.** The global caller
passes `saturation: 0.0, vibrance: 0.0` with a comment reading *"passing the real values would
silently move it earlier and change every golden."* Masked local contrast and sharpening follow the
identical rule: **the global caller passes them neutral.**

- `per_pixel_stages` is **renamed `adjustment_stages`** — it is no longer per-pixel. It also starts
  taking a `RenderScale`, which the spatial stages need and the global caller already holds.
- **Order inside a mask mirrors the global chain**, which is the rule a mask's adjustments already
  follow: white balance → exposure → contrast → tone regions → **local contrast** →
  saturation/vibrance → **sharpening last**. Sharpening is last for the reason it is last globally:
  it must not amplify what a later stage would then stretch.

### The proof this is right

**All 54 goldens must stay byte-identical.** A golden that moves means the global path gained a
stage it already had — the double-application above. This is the acceptance criterion for §3, and
it is stronger than any assertion that could be written by hand.

⚠ `per_pixel_stages_ignore_the_render_scale` survives unchanged **and gets stronger**. It asserts
scale is a no-op for the global path, which stays true only because the global caller passes neutral
spatial values. It quietly becomes the guard that the global path did not accidentally gain them.

## 4. The region margin — the structurally new part

`REGION_MARGIN` is a fixed `192`, against a present worst case of `135`. Masked clarity makes a
fixed margin **structurally insufficient rather than merely tight**: mask *N* runs on mask *N−1*'s
output, so *N* masks each carrying clarity reach *N* × 120 px into the input. No constant covers an
unbounded stack.

**`REGION_MARGIN` is deleted. The canvas sizes its request from
`pipeline::tail_kernel_radius(&edits, scale)` — the same function the pipeline uses.**

⚠⚠ **Deriving is the point, not a convenience.** A canvas that computed its own margin would make
the pipeline's reach and the canvas's request **two facts that can disagree** — the shape
`render_max_dim`, `fit_image_size` and `geometry_mode` were each fixed for. One fact, read twice.
The failure mode if they ever disagreed is silent: the *edge of the visible area* is subtly wrong,
and nothing errors.

⚠ **`tail_kernel_radius` sums over masks; it does not take a maximum.** Masks chain, so the reaches
compose. This is the M3.1a lesson restated, and **it is the one place this design can be quietly
wrong.**

`max_tail_kernel_radius` and its `REGION_MARGIN > worst` guard are both **removed**: with an
unbounded stack a fixed worst case is not a meaningful number. That is what deriving replaces.

### Accepted cost

The region grows with the edits: roughly **+22% pixels for one masked clarity, +78% for three**, at
native zoom. Paid only by photos that actually carry those adjustments.

## 5. Cost, measured before it is optimised

Global clarity is a measured **+8.8 ms** at preview, plus a clone (0.5 ms fit / 2.1 ms native) — and
that is **per mask**. Three masked-clarity masks is roughly +27 ms on a 13 ms tail.

**An early task measures it** — masked clarity at fit and at a native region, one mask and three —
in a kept example beside `bench_preview`. **The number decides** whether the bounding-box
optimisation ships here or is deferred a fourth time.

⚠ The bounding box only helps **brush and radial** masks. A linear gradient covers half the frame,
and range and colour masks have no spatial bbox at all — they are per-pixel predicates over the
whole frame. Any estimate of its value has to say which mask kind it is talking about.

## 6. Testing — the three that would otherwise name what they cannot detect

This project has recorded **eighteen** instances of a test naming a bug it could not see. These are
the three this feature invites:

1. **A one-mask margin test cannot see sum-versus-max** — with a single mask the two are identical.
   The test needs **two** masks carrying clarity, or it passes against the wrong arithmetic. Same
   shape as the same-month-in-two-years fixture.
2. **`a_region_render_matches_the_whole_render_cropped` must gain a *masked* spatial edit.** It has
   been silently insufficient in exactly this way once before: when sharpening landed, it kept
   passing because its edit set contained no sharpening. Left alone it will do so again.
3. **"Masked clarity changes the pixels" passes with the mask ignored entirely.** The discriminating
   half is the other direction — a pixel at coverage 0 must be **bit-identical** to the no-mask
   render, not merely close.

Plus: at least one new golden for a masked spatial edit, and the 54 existing ones byte-identical.

⚠ Every new test gets the sabotage step, and **a sabotage that does not fail is a finding about the
fixture** before it is a finding about the guard.

## 7. What this deliberately does not deliver

1. **Masked noise reduction and dehaze.** Both live in the **prefix**, before geometry; `MaskStage`
   lives in the tail. A masked one would therefore run *after* the tone curve — denoising grain the
   curve has already stretched, which is a different operation from the global stage wearing the
   same name. Dehaze additionally estimates a frame-global airlight, which has no obvious meaning
   confined to a mask. Their own slice.
2. **Masked HSL.** Per-pixel and nearly free, but a different feature — per-hue local adjustment —
   rather than the spatial this milestone names.
3. **The bounding-box optimisation**, unless §5's measurement asks for it.
4. **Skin softening is *not* among the gaps**, though the absence of masked NR suggests it: Texture
   is bipolar and −100 removes the fine band, softening edges within it. The commonest use of the
   feature is covered by the tail three.
