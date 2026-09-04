# W5 track C — M7.1b colour range masks

*Design, 2026-09-04, settled in W5-0 with the user. **None of these decisions is the implementer's
to re-open.** If you meet a question this spec does not answer, `PARALLEL-WORK.md` §6 applies: stop
and report it rather than choosing.*

---

## 1. What it is

A fifth `Mask` variant selecting on **colour** rather than position or lightness — the companion to
M7.1a's `Mask::Luminance`, whose own doc comment already anticipated it:

> Named for the quantity it selects on, **leaving room for a colour range beside it**.

---

## 2. It mirrors `LuminanceRange`, with one deliberate exception

```rust
pub struct ColourRange {
    /// Lower edge of the fully-covered band, in Oklab hue degrees 0..=360.
    pub low: f32,
    /// Upper edge. ⚠ May be LESS than `low` — see §3.
    pub high: f32,
    /// 0 = a hard edge at each end, 100 = the widest ramp. Smoothstep, as everywhere.
    pub feather: f32,
    /// Below this chroma a pixel is not selected at all — see §4.
    pub min_chroma: f32,
    pub invert: bool,
}
```

⚠ `feather` is named and scaled to match `RadialGradient::feather`, the vignette's and
`LuminanceRange`'s, so the app keeps **one feather idiom rather than four**, and the ramp is a
**smoothstep** for the reason `LuminanceRange` records: a linear ramp leaves a visible crease where
its derivative jumps, which on a smooth sky is exactly the artefact a range mask is reached for to
avoid.

⚠ `ColourRange::default()` must be **hand-written**, not derived — `LuminanceRange`'s comment
explains why, and here a derived default gives `feather: 0`, `min_chroma: 0` and a band of zero
width at 0°. **Default to a recognisable band with a soft shoulder** — an obviously-a-mask starting
point, as `LuminanceRange` chose the darker half.

---

## 3. ⚠⚠ Hue WRAPS, and this is the one place it must not mirror `LuminanceRange`

Lightness is an interval; hue is a circle. **Reds sit either side of 0° in Oklab** — sRGB red is at
**29.23°** and magenta at **328.36°**, both in `BAND_HUES` — so without wrapping, *the single most
commonly masked colour is the one the control cannot express.*

So `low > high` means the band crosses 0°: `340 → 20` selects red.

⚠ This needs **its own test**, because every non-wrapping range passes whether or not wrapping
works: `a_range_across_zero_selects_red_and_not_everything_else`. Assert both halves — that red is
covered *and* that green is not — since a wrap bug that selects everything passes a coverage-only
assertion.

⚠ Rejected alternatives, recorded so they are not re-proposed: rotating the axis so red is centred
only *moves* the seam somewhere the user cannot know about; and expressing a wrapped range as two
masks needs mask union, which is **M7.1c and does not exist**.

---

## 4. ⚠⚠ The chroma gate is load-bearing, not a refinement

A near-neutral pixel's hue is **sensor noise**. `HslStage` measured this directly and it is recorded
in `CLAUDE.md`:

> chroma ~0.003 at ±0.6% noise vs ~0.06 for a real colour … **measured 37.8× noise amplification**
> on a near-neutral patch … With the taper: 1.03×.

So coverage fades out as chroma approaches `min_chroma`, using the same smoothstep taper
`CHROMA_FADE` gives HSL. Without it, a colour range mask over a grey sky selects a speckle pattern.

⚠ It is **exposed as a control**, not fixed: what counts as "coloured enough" genuinely varies
between a pastel skin tone and a saturated sign, which is precisely the judgement HSL's fixed
`CHROMA_FADE = 0.02` cannot make.

---

## 5. Where the code goes

| unit | home |
| --- | --- |
| `ColourRange` + the variant | `edit-model/src/lib.rs` |
| coverage | `pipeline/src/stages/mask.rs`, in `analytic_coverage` |
| the panel controls | `develop/panel/masks.rs` |

⚠⚠ **`Mask::coverage` became `analytic_coverage(..) -> Option<f32>` for exactly this reason** — a
`Mask::Brush => 0.0` arm would compile, run, and silently produce a mask covering nothing.
`CoverageSource`'s match is **exhaustive on purpose** and a catch-all there was a live trap the
range-mask work already removed once.

⚠⚠ **CORRECTED 2026-09-04: this section said "the new variant returns `Some(..)`", and that is
wrong.** `Mask::Colour(_)` returns **`None`**, exactly as `Mask::Luminance(_)` already does one line
above it, whose own comment states the reason: *"A function of the pixel, not of `(u, v)`."*
`analytic_coverage` is handed a **position**; a colour range selects on the pixel's hue and chroma,
which are not in scope there, so `Some(_)` could only be a number invented from coordinates — a mask
silently selecting by position while claiming to select by colour, which is a worse version of the
`=> 0.0` fallback the `Option` exists to prevent. Coverage is answered from inside `MaskStage`'s
pixel loop, where the sample is in hand, as `CoverageSource::Range`'s own comment anticipated.
⚠ The implementer flagged this rather than stalling the branch, and was right on both counts; the
sentence was a generic gloss that contradicted the sibling variant this one is modelled on.

⚠ The variant is **appended** to the `Mask` enum — `PARALLEL-WORK.md` rule 3.

⚠⚠ **This track writes NO `Edits` field.** `Mask` variants live in `edit-model` but are not the
`Edits` field list, and track A owns that list plus its five exhaustive sites this wave. If you find
yourself editing `merge_from`'s destructure, `EditGroups`, `pipeline::all_edits`,
`edit_model::all_nonneutral` or `edit_clipboard`'s payload — **stop and report**; that is track A's
surface.

---

## 6. Canvas: nothing is drawn, deliberately

No handles — a colour range has no position, exactly as `Mask::Luminance(_) => {}` already records
in `mask_overlay.rs`.

⚠⚠ **And no coverage tint**, for the reason `CLAUDE.md` states about the luminance mask:

> the only pixels the canvas has are the rendered texture, which **already has this mask's
> adjustment applied**, so a tint from them would show the mask selecting the tones it has just
> moved. Doing it honestly needs coverage against the pre-mask buffer, which lives on the render
> worker: a different feature.

That reasoning transfers unchanged to colour. Do not add a tint on the assumption it is missing.

---

## 7. Testing

- **⚠⚠ Wrapping**, per §3 — the test no non-wrapping fixture can see.
- **The chroma gate**: a near-neutral pixel is not selected however well its hue matches. ⚠ Build the
  fixture from a *noisy* near-neutral patch, not a perfectly grey one — a perfect grey has an
  undefined hue and passes against a broken gate too.
- **Feather 0 is a hard edge** at both ends. ⚠ `RadialGradient`'s `feather_zero_is_a_hard_edge`
  probed either side of the boundary and **never on it**, and so passed with its guard deleted
  because IEEE infinity clamped to the right answer. Probe **on** the boundary.
- **Invert flips coverage**, as every other shape does.
- **Band hues agree with the canonical colours** — reuse `BAND_HUES`' approach: recompute from
  `#FF0000`…`#FF00FF` rather than hardcoding, and **fix the table, never the tolerance**.
- **Goldens: append, never insert.** ⚠ Track A also adds goldens this wave.
- Existing mask tests pass **verbatim** — that is the evidence the variant changed nothing.

⚠ **Every sabotage confirmed with the edit verified present, a `test result:` line seen, and the
NAMED test failing.** Nine sabotages passed while proving nothing in the two sessions before this
one; three were the *fixture's* fault, not the guard's. **If a sabotage does not fail, suspect the
fixture first.**

---

## 8. Accepted caveats

1. **Nothing is drawn on the canvas** (§6), so the mask is judged by its effect.
2. **Per-pixel adjustments only**, inherited from M3.1a — a colour range carries the same subset
   every mask does.
3. **No colour picker.** Clicking a colour on the canvas to set the band is more discoverable and is
   Lightroom's affordance, but it needs canvas interaction — the contended surface — and rule 5 says
   split headless from canvas. It is the obvious follow-up.
4. **No intersection with other masks** — that is M7.1c.
5. **Hue only.** Selecting on hue *and* saturation as two independent bands is strictly more
   expressive and is two ranges, two feathers and a composition rule to explain; this track's value
   is that it mirrors something already learned.

---

## 9. Done when

A colour range mask can select the reds in a photograph — including across 0° — without speckling on
a near-neutral sky, its controls read like the luminance range's, and every existing mask test
passes unchanged.
