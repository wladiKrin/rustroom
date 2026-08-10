# Texture & Clarity — design

**Date:** 2026-08-10
**Status:** approved, ready to plan
**Scope:** `edit-model`, `pipeline` (one new stage, one new spatial primitive), `app` (two sliders, one constant)

## What this is

Two bidirectional local-contrast controls:

- **Texture** — the fine band. `+` brings out surface detail (fabric, bark, stubble); `−` smooths it.
- **Clarity** — the coarse band. `+` is the punchy midtone-contrast look; `−` is a glow.

Both are the same operation as `SharpenStage` — an unsharp mask on Rec.709 luma in linear
light, applied back as a scalar RGB gain — differing only in blur radius and response.
Read `crates/pipeline/src/stages/sharpening.rs` first; it is the closest sibling by a wide
margin, and most of the constants and guards below are lifted from it deliberately.

## What this is not

**Dehaze is a separate feature**, deliberately split out before this spec was written. Two
things make it a different animal:

1. **It needs a global statistic.** The standard model (dark-channel prior, He et al. — the
   same paper family as the guided filter already inside `NoiseReductionStage`) estimates an
   airlight `A` from the brightest pixels of the *whole frame's* dark channel. A tail stage
   rendering a 1400×900 crop would estimate a different `A` than the whole frame does, so
   `a_region_render_matches_the_whole_render_cropped` would break in the **interior**, not
   merely at the margin, and panning at 1:1 would visibly shift the image. Fixing that needs
   a new plumbing concept — global statistics computed upstream and handed to the tail,
   alongside `Region` — which is an architectural decision that deserves its own spec.
2. Its radii are the largest of the three, so it is the one most likely to reopen the
   `REGION_MARGIN` question settled below.

Dehaze gets its own brainstorm → spec → plan cycle immediately after this one. M2.3 closes
at the end of *that* cycle, not this one.

## The model

Per pixel, with `l = luma(px)` (Rec.709, linear — the same `luma` helper `sharpening.rs`
uses):

```
d_t  = l − gaussian_blur(l, σ_t · scale)      // σ_t = TEXTURE_SIGMA  =  2.0 full-res px
d_c  = l − wide_blur    (l, σ_c · scale)      // σ_c = CLARITY_SIGMA  = 40.0 full-res px

Δ    = k_t · d_t  +  k_c · d_c · w(l)
gain = clamp(1 + Δ / max(l, LUMA_EPS), 0.0, GAIN_MAX)
out  = px · gain
```

with `k_t = texture/100`, `k_c = clarity/100`, both clamped to `[-1, 1]`.

### One stage, not two

Both deltas are computed from the **same source luma** and summed. Two sequential stages
would make Clarity operate on Texture's *output*: the sliders would interact, and neither
would be independently predictable. Summing deltas from one source makes them exactly
additive, which is a property a test can assert rather than a claim in a comment:

```
out(t, c) == out(t, 0) + out(0, c) − src        (before the GAIN_MAX clamp engages)
```

Working on luma and re-applying as a scalar ratio buys the same three things it buys
`SharpenStage`: no colour fringing on a chromatic edge, `r:g:b` ratios — hence hue and
saturation — preserved exactly, and **no Oklab round trip**, the cost that dominates
`HslStage` and `NoiseReductionStage`.

### Endpoints mean something

`k = amount/100` with no extra gain constant, so **−100 removes the band and +100 doubles
it**. That is the same kind of defensible endpoint the vignette settled on ("−100 is exactly
black"), and it is what makes the slider explainable without reference to a magic constant.

### Midtone weighting, on Clarity only

```
t    = l / (l + MID_GRAY)        // MID_GRAY = 0.18, so mid-gray maps to exactly 0.5
w(l) = 4·t·(1 − t)               // 1 at mid-gray, falling to 0 at both ends
```

Chosen over `l^(1/2.2)` because it needs no `powf` and still lands mid-gray on 0.5 by
construction.

Without this, +Clarity at a 40 px radius clips skies and blocks up shadows — the standard
complaint about the tool. **Texture does not get it**, and the asymmetry is justified by
amplitude rather than taste: the fine band is low-amplitude and cannot reach the clip on its
own, so weighting it would cost per-pixel work for no visible protection.

Consequence, stated plainly: with `w` applied, "−100 removes the band" holds **in the
midtones** for Clarity, and everywhere for Texture.

### Guards

`LUMA_EPS = 1e-3` and `GAIN_MAX = 4.0` are lifted from `sharpening.rs`. The near-black guard
is load-bearing for exactly the same reason: without it a dark pixel beside a bright edge
divides by ~0 and the ratio explodes. `GAIN_MAX` then bounds the resulting halo at +2 stops.
The gain is clamped at 0 below, so the stage can never emit a negative sample and needs no
separate clamp — the same structural property `VignetteStage` has.

## `spatial::wide_blur` — a third primitive

Clarity's σ = 40 makes a Gaussian unaffordable: `ceil(3σ)` = 120, so 241 taps × 2 passes ×
26 Mpx is ~13 G ops on **every export**. So Clarity gets **three iterated `box_blur` passes**,
which is O(1) in σ — like the NR guided filter — and converges to a Gaussian quickly.

```
r     = round( (√(1 + 4σ²) − 1) / 2 )      // three passes: 3·((2r+1)²−1)/12 = σ²
reach = 3 · r                              // r == 0  ⇒  identity, like gaussian_blur
```

At σ = 40 that is `r = 40`, `reach = 120`, and the realised σ is 40.5 — within 1.3%.

**No `.max(1)` on `r`.** Clamping up would make σ = 0.1 render as a 3-tap box — an effective
σ of 0.82, eight times too wide. Returning a copy instead is both honest and consistent with
`gaussian_blur`, and it restates the reason `gaussian_blur` exists: below σ ≈ 0.87 an integer
box radius cannot represent the blur at all. Clarity only reaches that regime at a render
scale below 0.022 — a 26 Mpx file previewed at 130 px — so it never happens in practice.

This **extends** the split already documented in `spatial.rs` to three regimes rather than
contradicting it:

| primitive | regime | why |
|---|---|---|
| `gaussian_blur` | sub-pixel to a few px | direct kernel is cheap (7 taps at σ=1), and an integer box radius cannot express a sub-pixel blur at all |
| `wide_blur` | tens of px | O(1) in σ, and at that scale the box-iteration error is invisible |
| `box_blur` | any, when a genuine box is wanted | NR's guided filter needs box means, not Gaussian ones |

`wide_blur` inherits `box_blur`'s border behaviour — the *window* clamps, not the samples —
so a constant plane stays exactly constant.

A companion `wide_blur_reach(sigma) -> usize` is public for the margin invariant below,
mirroring why `kernel_radius` is public.

## Pipeline position

**In the tail, after `ToneCurveStage`, before `SaturationStage`.**

- **After the tone curve** is mandatory: a local contrast computed before it would then be
  re-mapped by it. This is the same argument that kept the vignette out of the prefix.
- **Before the colour stages** so saturation and HSL see final tonality. Numerically this
  barely matters — the gain is a scalar and preserves `r:g:b` ratios — but the ordering
  should still say what we mean.
- **Well before sharpening**, which stays last, and before the vignette, which stays the
  final creative overlay.

Resulting order:

```
geometry → noise_reduction │ white_balance → exposure → contrast → tone_regions →
tone_curve → local_contrast → saturation → hsl → vignette → sharpening → output
                              ^^^^^^^^^^
```

The `│` is the prefix/tail cache checkpoint. `local_contrast` is in the **tail**, so it
re-runs on every slider move — correct, since its parameters change often — and it does not
invalidate the noise-reduction cache.

## The margin invariant

`app::view_region::the_region_margin_exceeds_every_tail_kernel_radius` asserts that
`REGION_MARGIN` exceeds the widest neighbourhood any tail stage reads. Today the widest is
sharpening's 15 px. Clarity's reach is **120 px**, against `REGION_MARGIN = 128`. That
passes — with 8 px of slack, which is not slack, it is a trap for whoever touches this next.

So this feature makes two changes:

1. **`REGION_MARGIN`: 128 → 192.** Cost is roughly 20% more pixels in the region tail
   (a 1400×900 viewport goes from 1656×1156 to 1784×1284), i.e. ~6.9 ms → ~8.4 ms. Nothing,
   against 72 px of real slack.
2. **Two functions in `pipeline`, replacing the direct calls to sharpening's
   `max_kernel_radius`.** The two existing call sites want different things, and conflating
   them is how this goes wrong:

   ```rust
   /// Widest neighbourhood any tail stage reads for *these* edits, in rendered px.
   pub fn tail_kernel_radius(edits: &Edits, scale: RenderScale) -> usize;

   /// The same, maximised over every representable parameter — what a margin must exceed.
   pub fn max_tail_kernel_radius(scale: RenderScale) -> usize;
   ```

   `max_tail_kernel_radius` is defined as `tail_kernel_radius` applied to an internal
   worst-case `Edits`, so the two cannot drift. An inactive stage contributes 0.

   Texture and Clarity are **summed, not composed**, so their joint contribution is
   `max(kernel_radius(σ_t·scale), wide_blur_reach(σ_c·scale))`, not a sum — unlike
   sharpening's two internal Gaussians, where the mask blurs `|d|` on top of the first blur
   and the radii genuinely add.

σ_c = 40 was chosen for the look and the margin moved to fit it, not the other way round —
moving the margin is the cheap side of that trade.

The two call sites take one function each:

- `view_region`'s invariant test asserts `REGION_MARGIN > max_tail_kernel_radius(FULL)`.
- `a_region_render_matches_the_whole_render_cropped`'s inset uses
  `tail_kernel_radius(&edits, FULL)` — the actual reach, not the worst case, or the inset
  would swallow the whole comparison. Its edit set also gains a non-neutral `local_contrast`,
  so the weakened-comparison guard is actually exercised — the same correction sharpening had
  to make. **⚠ That forces the test's buffer to grow**: the inset jumps from 9 px to 120 px,
  and its existing `w > 2 * inset` guard cannot hold on a 64×64 buffer. Size the buffer and
  region from `inset` rather than hard-coding a bigger number, so this tracks any future σ.

## `edit-model`

```rust
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct LocalContrast {
    /// Fine-band local contrast, ~[-100, 100]. 0 = off.
    pub texture: f32,
    /// Coarse-band local contrast, ~[-100, 100]. 0 = off.
    pub clarity: f32,
}

impl LocalContrast {
    pub fn is_active(&self) -> bool { self.texture != 0.0 || self.clarity != 0.0 }
}
```

`Default` is all-zero, so unlike `NoiseReduction`/`Vignette`/`Sharpening` there are no
modifier fields and `is_active()` could have been `!= default()`. It is written the same way
anyway, so the four "spatial-ish" params read alike and so adding a modifier later does not
change the rule.

`Edits` gains `local_contrast: LocalContrast`. `#[serde(default)]` on `Edits` means old
sidecars load unchanged.

**Group: Tone.** `Edits::merge_from`'s exhaustive destructure will refuse to compile until
the field is given one. Detail is capture-specific — noise, lens, output size — while Texture
and Clarity are *look*, which is what a preset carries. The doc comment on `EditGroups::tone`
must be updated to say so. The generic partition tests in `edit-model` then cover it with no
new field lists.

## UI

Two sliders, ±100, **inline after Blacks and before Saturation** in the right Develop panel —
Lightroom's Presence grouping, and it reflects that these are tonal rather than detail
controls. No collapsing header: two sliders do not need one, and burying them would misplace
them relative to the tone sliders they belong with.

They inherit undo/redo, the sidecar, presets, copy/paste, sync and export for free, like
every previous slider.

## Tests

### `local_contrast.rs` (unit)

The ones that carry weight:

- **`texture_and_clarity_are_exactly_additive`** — `out(t,c) == out(t,0) + out(0,c) − src`
  on parameters mild enough that `GAIN_MAX` never engages. This is the entire justification
  for one stage rather than two, so it is the test that must exist.
- **`clarity_acts_at_a_coarser_scale_than_texture`** — a fine-sine image responds to Texture
  and barely to Clarity; a coarse ramp the reverse. Without this, a wrong σ passes silently
  and every other test still goes green.
- **`texture_minus_100_removes_the_fine_band`** — the endpoint claim, measured.
- **`midtone_weighting_protects_the_extremes`** — Clarity's change near mid-gray is far
  larger than near 0 and near 1.
- `both_zero_is_an_exact_identity`, `a_flat_region_is_untouched` (`d = 0` ⇒ gain 1),
  `hue_ratios_are_preserved`, `never_emits_negative_or_nan_samples`,
  `radii_scale_with_the_render_scale`, and hard numbers for the stage's kernel reach.

### `spatial.rs` (unit)

- `wide_blur_constant_plane_stays_constant`
- `wide_blur_is_symmetric` (mirror in both axes)
- `wide_blur_reach_matches_three_box_radii`
- `wide_blur_non_positive_sigma_is_identity`
- **`wide_blur_approximates_a_gaussian`** — variance reduction on a fixed noise plane within
  a few percent of `gaussian_blur` at the same σ. Otherwise "approximates a Gaussian" is an
  unverified comment.

### `pipeline/src/lib.rs`

- `local_contrast_runs_after_the_tone_curve_and_before_saturation` (stage-name order)
- `local_contrast_neutral_adds_no_stage`
- `tail_kernel_radius_covers_every_spatial_tail_stage` — including that an inactive stage
  contributes 0, and that `max_tail_kernel_radius` is ≥ `tail_kernel_radius` for any edits
- `all_edits()` gains the field — a compile error until someone decides which half of the
  split it belongs to, which is the point of it being written out explicitly
- `prefix_key_ignores_every_downstream_field` gains a `local_contrast` case

### `app`

- `view_region::the_region_margin_exceeds_every_tail_kernel_radius` — via
  `max_tail_kernel_radius`, against the new `REGION_MARGIN`
- `a_region_render_matches_the_whole_render_cropped` — inset from `tail_kernel_radius(&edits,
  FULL)`, edit set gains a non-neutral `local_contrast`, buffer and region sized from the
  inset

### Goldens: 23 → 27

A new **256×256 `local_contrast_chart()`**: a coarse horizontal ramp, a superimposed fine
sine, and a hard vertical edge. The existing charts are 64×16 and 128×192 — too small for a
120 px reach to measure anything but the global mean.

Cases: `texture_plus`, `texture_minus`, `clarity_plus`, `clarity_minus`.

**All 23 existing references must stay byte-identical** — that is the proof the new stage is
inert at its default, and it is the check that matters more than the four new files.
`UPDATE_GOLDEN=1` regenerates, but the new PNGs must be **looked at**: a golden nobody has
looked at is just a hash.

## Validation on a real photo

Non-optional, and it is where the tuning constants get settled. Precedent: the vignette's
amount model was revised three times, every time from looking at an actual image, and the
spec could not have predicted any of the three.

- `cargo run --release -p app` on the X-T4 `.RAF`, at 1:1 for Texture and at fit for Clarity.
- Judge whether `TEXTURE_SIGMA = 2.0` is a band genuinely distinct from Sharpening's default
  σ = 1.0, and whether `CLARITY_SIGMA = 40.0` reads as "local contrast" rather than "global
  contrast" or "haloed".
- Check a sky/mountain boundary for Clarity halos, and a face for negative-Texture mush.
- Re-measure the preview render against the table in CLAUDE.md.

## Accepted caveats

1. **Texture must be judged at 1:1; Clarity is faithful at fit.** At the X-T4 fit scale of
   0.256, σ_t = 2.0 becomes 0.51 — near-identity, because the detail Texture acts on has
   already been averaged out of the preview. σ_c = 40 becomes 10.2, which is entirely real.
   So the two sliders in the same group disagree about where to look, and that is inherent,
   not a defect. Stated here so it is not rediscovered as a bug.
2. **No halo suppressor on Clarity.** The midtone weight is the mitigation. A `Detail`-style
   suppressor (`sharpening.rs` has one) is a contained follow-up if a real sky/mountain edge
   looks bad — it changes one expression, not the stage's shape.
3. **σ in full-resolution pixels**, like every other radius in the app. Framing-relative
   behaviour already falls out of the export path, which resizes *before* the pipeline so
   radii scale with the output and preserve the look.
4. **`TEXTURE_SIGMA`, `CLARITY_SIGMA` and `w`'s shape are tuning constants.** They will be
   settled by looking at photographs, not by this document.
5. **Negative Texture is not edge-aware**, so it softens edges within its band along with the
   texture. Deliberate: noise reduction already owns edge-aware smoothing in this app, and it
   is upstream where it belongs. A guided-filter base for the negative half is a contained
   follow-up if a real face looks mushy — at the cost of making the tool behave differently
   either side of zero, which users notice.
6. Buffer-native 1:1 is still a **half-size** decode that skips demosaic, so Texture judged
   at 1:1 corresponds to roughly twice the radius the export applies. Bounded judgement, not
   exactness — the existing caveat, restated because this is a detail-level tool.
