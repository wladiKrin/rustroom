# Composition guides as a View feature

*Design, 2026-09-01. Extends M5.5a, which shipped the guides Crop-tool-only.*

The compositional guides stop being a Crop-tool overlay and become a **View** feature, drawn under
every Develop tool. Requested after using M5.5a: *"should we add that as a general feature, not
restricting it to geometry? Being able to toggle that in view would be nice."*

---

## Why this is small

Almost all of it exists. `Settings.composition_guide` persists, `O` cycles it, the **View → Composition
guide** menu entry is already there (`menu.rs:212`), the Geometry panel has its chip row, and
`composition_guides::segments` computes the four geometries as pure functions.

**The control is done. Only the drawing is scoped to one tool.** `draw_guides` is called from inside
`draw_crop_overlay`, which the canvas invokes only under `Tool::Crop` — so the setting and its menu
entry currently do nothing in Edit, Masks or Presets. This moves one call.

---

## 1. The rule

> **Guides divide what will be the final photograph.**

| Context | Rectangle | Why |
| --- | --- | --- |
| **Crop** | the crop rect | `preview_edits` strips the crop in geometry mode, so the rect on screen *is* the future photograph |
| **Edit · Masks · Presets** | the frame | the crop is applied, so the frame *is* the photograph |
| **Guide Mode** (perspective) | **none** | see below |

⚠ This is **one** rule, not two behaviours. It only looks like two because the Crop tool is the one
place where the photograph-to-be and the frame on screen differ.

### ⚠⚠ Guide Mode is the exception, and it is deliberate

Perspective Guide Mode lives inside the Crop tool, so "all four tools" formally includes it. But
there the preview strips the crop **and** no crop rect is drawn, so a composition guide would divide
the *uncropped* frame — a rectangle the user is not going to keep.

**A guide that divides the wrong rectangle is worse than no guide, because it will be trusted.** The
whole value of a thirds line is that a third is actually a third. Guide Mode is also transient — you
enter it to solve verticals and leave — so the exception is short-lived.

⚠ Rejected: drawing against the *stored* crop rect while the preview shows the uncropped frame. It is
technically the most correct and reads as a bug, because the lines would sit over a region that
corresponds to nothing visible.

### ⚠ Coordinates: `frame_rect`, clipped to `img_rect`

The rule the mask overlay and the perspective guide overlay already follow, for the reason recorded
against both: once a region render is on, `img_rect` covers only the **rendered slice**, not the
frame. Guides must divide the whole photograph, so every coordinate is frame-relative and `img_rect`
survives only as the clip. Getting this wrong is invisible at fit and wrong at every zoom.

---

## 2. Scope

**All four tools** (user's call). The guides are non-interactive, so they cannot fight a gesture; `O`
is one keystroke away when they are in the way. An exception would cost more to remember than it
saves.

⚠ The known risk, accepted rather than designed around: **clutter in Masks**, where the canvas
already carries a coverage tint, handle circles and a brush cursor. Better met in use than
pre-empted with a rule.

⚠ Confusion with the perspective guides was considered and measured away: those are **5 pt cyan
(80, 200, 255) and amber (255, 190, 70) with a halo**, against the composition guides' **0.8 pt grey
at alpha 150**. They cannot be mistaken for one another.

---

## 3. What does not change

No new setting, no new key, no new menu entry, no new geometry, and **no change to the Crop tool's
current behaviour**. The Geometry panel keeps its chip row: that is two entry points for one
*value*, which this project accepts — contrast the Presets panel, removed for being two homes for
one *feature*.

---

## 4. Testing

⚠⚠ **`draw_guides` currently has no test that it is called at all.** Its own comment says so:
*"with the `draw_guides` call deleted they all stay green and the feature is silently absent"*.
Moving that call is precisely when that gap bites, so closing it is part of this change rather than
a nicety.

Three properties earn named tests:

1. **Drawn under every tool.** A loop over `Tool::ALL` — the only shape that can see "shipped for one
   tool only", which is the exact defect being fixed. Same argument as
   `the_strip_is_present_under_every_tool`.
2. **Not drawn in Guide Mode.** The exception, and therefore the thing most likely to be deleted by
   someone tidying up later.
3. **Zoomed, they divide the frame and not the viewport.** Needs a region render in the fixture;
   without it this can only be caught by eye, and it is invisible at fit — the failure mode that
   produced two recorded bugs in the mask and guide overlays.

⚠ Sabotage each: deleting the call, drawing in Guide Mode, and substituting `img_rect` for
`frame_rect`. The third is the one a fit-resolution fixture cannot see.

---

## 5. Accepted caveats

1. **Clutter in Masks**, as above.
2. **No guides in the Library** — there is no image canvas there, and a grid over a thumbnail grid
   would mean nothing.
3. **No per-photo guide**; the choice is global, like the rest of `Settings`.
4. **Guide Mode shows none**, which is an exception met without warning.
5. **Still four guides** — no golden spiral (four orientations, so it needs a second cycle key) and
   no triangle. Unchanged from M5.5a.
