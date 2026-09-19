# The Crop & Geometry panel: four sections — design

**Date:** 2026-09-19. **Branch:** `geometry-panel`. **Status:** option A of the canvas
"Rustroom Geometry Panel" (https://claude.ai/artifact/F4fiydZVBoiFGwPdngK8aQ), approved in
conversation.

## Why

The panel is a plain heading and five small-print sub-labels. Seven aspect chips in one row clip
or wrap at the panel's width, five composition chips wrap over two lines, Straighten sits between
Composition and Perspective though it is a crop decision, the guides that feed Perspective are a
block after it with a line of small print, the Lens block appears and disappears at the top, and
Reset geometry floats at the bottom.

## The panel, top to bottom

```
› CROP •                                        reset
  [⟲][⟳]  [⬌][⬍]                                [Swap]
  [ Free | 1:1 | 3:2 | 4:3 | 16:9 | W:H ]      (segmented; wraps rather than clips)
                                    Custom [3] : [2]   (only while W:H is lit)
  Composition guide                 [Rule of thirds ▾]
› STRAIGHTEN •                                  reset
  Angle ────────●──────── 0.0°  [level]
› PERSPECTIVE •                                 reset
  Vertical, Horizontal, Aspect ⬍ ⬌, Scale
  [Guide mode] [Auto | Refine guides] [Clear]
› LENS                                   XF23mmF2 R WR
  Lens scale ───────────  (or a line saying why there is nothing to scale)
```

- `Tool::Crop.panel_heading()` returns `None`: the sections are the headings. The `tool.rs`
  rule becomes "Edit, Crop and Masks carry their own section headers".
- Four `section::header`s, ids salted `("geo", name)`, all open by default.
- **The dot and reset per section** come from `edit-model`: a new `GeoGroup { Crop, Straighten,
  Perspective }` with `ALL` and `name()`, and on `Edits` `geo_group_differs(&self, GeoGroup)` and
  `reset_geo_group(&mut self, GeoGroup)` through one `copy_geo_group` that destructures
  `Geometry` exhaustively. Crop = `geometry.crop` + `geometry.orientation`; Straighten =
  `geometry.angle`; Perspective = `perspective`. A Perspective reset also clears the session's
  guides, as editing its sliders by hand does. The old "Reset geometry" button goes; the three
  resets together do what it did.
- **Crop.** Rotate left/right and flip horizontal/vertical as 28 pt buttons with the glyphs the
  panel already uses (they are known to render), a gap between the pairs, Swap right-aligned in
  the same row. The aspect is `theme::segmented_sized` at 28 pt in the body face over
  `Chip` (Free, 1:1, 3:2, 4:3, 16:9, W:H); the lit segment is derived from the lock as today,
  and a click does what the chip did (`lock_to`, or `None` for Free). "Custom" is renamed
  "W:H". The custom fields sit right-aligned on their own row while W:H is lit. The composition
  guide is a `ComboBox` on a row with a "Composition guide" label; its hover keeps "Press O to
  cycle".
- **Straighten.** The angle slider, labelled "Angle", sharing its row with a 28 pt toggle at
  the right that arms the draw-a-line tool: a hand-painted level glyph, accessible name
  "Straighten by drawing a line" (the harness and screen readers keep today's label), hover as
  today.
- **Perspective.** The four sliders with their hovers, then one row: `Guide mode` (a toggle),
  `Auto` or `Refine guides`, `Clear` (enabled with guides). The small-print line joins the Guide
  mode hover.
- **Lens.** Always drawn, trailing text = the lens name, or "no profile". Body: the "Lens scale"
  slider when a profile matched and distortion is on (the label stays "Lens scale": Perspective
  has a "Scale" and two controls must not share a name); otherwise one weak line saying why
  ("No profile for this lens." / "Distortion is off in the Edit panel's Lens section."). Not
  resettable here: the Lens edit group has its own section in the Edit panel.

## Out of scope

The canvas overlay; a Crop / Straighten / Perspective mode (option C); icons from the font.

## Files

- new `crates/edit-model/src/geo_group.rs` (+ `pub use`)
- `develop/panel/geometry.rs` (the body, tests re-targeted), `tool.rs`
- `docs/JOURNAL.md`, `CLAUDE.md` state

## Testing

- `geo_group`: one moved field differs in exactly its group; reset clears exactly it; resetting
  all three reaches the default geometry and perspective.
- `geometry`: composition through the combo (click its current label, then a guide); "Custom" →
  "W:H"; the reset test becomes per section (a modified Perspective's reset clears it and leaves
  the crop; a modified Crop's reset leaves the angle); the level toggle by its accessible name;
  Lens: the slider only with a profile and distortion, the section header always.
- `tool`: the unheaded tools are Edit, Crop, Masks.
- Gate as usual; GUI in release.
