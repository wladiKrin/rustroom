# The Color section, made legible — design

**Date:** 2026-09-24
**Branch:** `color-section-clarity`
**Status:** approved — user chose **A + B + C** of four offered, and sequenced the RGB curves after this

## The report

An amateur photographer reviewing Rustroom found the Develop panel's **Color** section confusing.
Grounded rather than taken on trust — the section holds:

| Block | Visible controls | Values behind them |
|---|---|---|
| Vibrance, Saturation | 2 sliders | 2 |
| HSL | 3-way channel selector + 8 band sliders (Red…Magenta) | **24** |
| Color Grading | checkbox, 4-way range selector, a 2D wheel, 3 sliders | **12** |
| Blending, Balance | 2 sliders | 2 |

**~40 values behind 15 sliders, two segmented selectors, a wheel and a checkbox**, in one section.

Two specific causes, not "colour is hard":

1. ⚠⚠ **"Saturation" appears three times in that one section, meaning three different things** — the
   global slider, HSL's channel, and the grading wheel's slider. "Hue" and "Luminance" appear twice
   each. `hsl_bands.rs`'s own comment already records hitting this: *"put a 'Saturation' header under a
   'Saturation' slider meaning something else"*.
2. It bundles **three different mental models** with nothing naming them: *more/less colour overall*,
   *change one colour*, *tint by brightness*.

⚠ The capability is not the problem and is not being reduced. Every serious raw editor exposes all
three. **Naming and grouping are free; that is what changes.**

## The constraint that shapes this

⚠⚠ **A section's header text IS its edit group's name.** `Section::name()` returns
`EDIT_GROUP_NAMES[i]`, `edit_groups()` is *derived* from `SECTIONS`, and the same names appear in the
Sync dialog and drive "Reset adjustments". So **three real sections would require splitting the `Color`
edit group**: a new field in `EditGroups`, `merge_from`'s destructure, the three exhaustive fixtures, a
serde default on a persisted struct, and three checkboxes in Sync where there is one today.

**Not done here.** The three groupings are delivered as **named blocks inside the Color section**,
reusing `section::header` with `resettable(false)` for the disclosure and sub-labels for the blocks, so
nothing claims an edit group that does not exist. ⚠ `adjustments.rs` already warns about exactly this:
*"a header here would put the panel and the Sync dialog out of step"*.

⚠ If per-block Sync granularity is ever wanted, splitting the group is **its own change** with a
version bump. Recorded, not smuggled in.

## The layout

```
Color                                    ← edit group, unchanged
  Vibrance
  Saturation
  ADVANCED COLOR                         ← collapsible, CLOSED by default, not resettable
    ADJUST INDIVIDUAL COLORS             ← hover: "HSL …"
      [ Hue | Saturation | Luminance ]
      Red · Orange · Yellow · Green · Aqua · Blue · Purple · Magenta
    TINT BY TONE                         ← hover: "Color grading …"
      [ Shadows | Midtones | Highlights | Global ]   + Show range
      (wheel)
      Hue · Strength · Luminance
      ───
      Blending · Balance
```

⚠ **What a newcomer now meets in Color: two sliders.** Everything else is one click away, behind a
name that says what it is for.

⚠ Two collapsibles deep plus a text sub-label — **not** three nested collapsibles, which
`hsl_bands.rs` records as already rejected for costing a third level. The Tone section already nests a
sub-label ("PRESENCE") inside a section.

⚠ American spelling in every label — the codebase's UI strings are "Color", "Color Grading". Prose in
comments uses British; **labels must not**.

## The one rename

**The grading wheel's per-wheel `Saturation` → `Strength`.**

⚠⚠ This is the one genuinely *misleading* collision: the global Saturation and HSL's Saturation channel
both mean **the image's chroma**, while this one means **how much tint is applied**. Same word, opposite
subject.

⚠ **`Hue` and `Luminance` are deliberately NOT renamed.** They repeat by name but not by meaning — both
really are a hue and a luminance, of a hue band and of a tonal range respectively — and Lightroom labels
its wheels the same way, so renaming would cost recognition and buy nothing. A collision of *words* is
not automatically a collision of *meanings*, and only the latter misleads.

**Plain language visible, jargon in the hover.** "ADJUST INDIVIDUAL COLORS" and "TINT BY TONE" are what
a beginner can act on; `HSL` and `color grading` are what an experienced user searches for, so they go
in the hover — the idiom Settings already uses for its descriptions. Needs a `theme::sub_label_hover`,
since `sub_label` returns `()` today.

## Testing

⚠⚠ **The label collision gets a guard that cannot go stale.** Render the Color section through the UI
harness, collect every control's accessibility name, and assert **no two are equal**. That grows by
itself: adding a second "Saturation" fails it, with no list to maintain. This is the defect as a test,
and the opposite of the enumerating guards this repo keeps being bitten by.

Also:
- **Advanced color is closed by default**, and Vibrance and Saturation are reachable without opening it.
- The disclosure is **not** resettable — only the section's own header resets the `Color` group, or two
  reset affordances would claim to reset different things.
- `edit_groups()` still returns exactly the six sections' groups: ⚠ the new collapsible must not have
  changed what "Reset adjustments" touches.
- `theme::sub_label_hover` shows its hover and still uppercases, like `sub_label`.

⚠ What no test reaches: whether an amateur now finds it clearer. That is the whole point, so it is
confirmed by showing it to one — or not at all.

## Deliberately not done

⛔ Splitting the `Color` edit group into three (Sync granularity) — its own change, with a version bump.
⛔ Renaming `Hue` / `Luminance` — collision of words, not of meanings.
⛔ **Per-channel RGB tone curves** — the user's sequencing: this first, then those as a channel selector
on the existing tone-curve widget, in the **Tone** section. ⚠ Worth recording why they are worth doing
after all: they are in Photoshop, Lightroom, Camera Raw, Capture One, darktable, RawTherapee, Affinity
and GIMP — they are the *original* grading tool, and the wheels are the friendlier interface built over
them. Defaulting the selector to the composite means a beginner sees exactly today's widget.
