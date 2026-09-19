# The Masks panel: icon rows, minimal change — design

**Date:** 2026-09-19. **Branch:** `masks-panel`. **Status:** option **C** of the canvas
"Rustroom Masks Panel" (https://claude.ai/artifact/E7CgHLYEyoV3jwd1os18SB). ⚠ Option A ("menus,
not modes") was approved and built first the same day; the user then preferred C, and the panel
was changed before merge. What A and C share (painted rows, the Shape section, the four
adjustment sections, the glyphs, `MaskKind`, `LocalGroup`) stayed; the Add mask menu, the Refine
row and the header's overlay toggle went. Also carries one unrelated tweak the user asked for at
the same time: larger rail icons.

## Why

The Masks panel after the restyle still reads as a pile. Adding anything takes two rows read
together: a mode chip (New / Add / Subtract / Intersect) that is invisible state, then one of five
"Add …" buttons wrapping over three lines. Masks and their components are the same button style,
only indented, with the × on the left. Below the shape, about forty sliders arrive as one flat
run grouped by separators and two lone collapsing rows. "Show mask", the hint and "Remove all
masks" float loose at the bottom.

## The panel, top to bottom

```
› MASKS •                                       reset
  [ New mask | Add | Subtract | Intersect ]     (segmented; disabled without a selection)
  [ ◐ ][ ◐ ][ ◐ ][ ◐ ][ ◐ ]                     (one icon per kind, name on hover)
  hint: New mask starts a fresh mask; Add, Subtract and Intersect put the next shape onto the selected one.
  ◐ Gradient 1                                 ×
  ◐ Radial 2                    (selected, tinted) ×
      ◐ Radial                  (component being edited)
    − ◐ Brush                                  ×
    ∩ ◐ Luminance                              ×
  ☐ Show mask
› SHAPE                                   Radial
  ☐ Invert · Feather …  (or the brush tool settings) · the kind's hint
› WHITE BALANCE                           Temp, Tint
› TONE •                                  reset
  Exposure … Blacks · Tone Curve (sub-label, inline) · Presence: Texture, Clarity, Dehaze
› COLOR (collapsed)   Vibrance, Saturation, HSL, Color Grading
› DETAIL (collapsed)  Noise Reduction, Sharpening
```

### The MASKS section

- The tool's plain "Masks" heading goes; `Tool::Masks.panel_heading()` returns `None` and the
  section header is the heading. The rule in `tool.rs` becomes "Edit and Masks carry their own
  section headers".
- `section::header` gains an optional **trailing text** (weak, non-interactive) and becomes a
  builder: `Header::new(name).modified(b).resettable(b).trailing(text)`; the return is
  `HeaderOut { state, reset }`. The two existing callers move to the builder.
- MASKS: `modified` = any masks; `reset` = today's "Remove all masks" (same group merge, same
  selection clearing, tooltip says what it deletes).
- **The mode** is a `theme::segmented` over `Option<MaskOp>`: New mask / Add / Subtract /
  Intersect (`MaskOp::label()`). It is visible state (`MaskUi::add_mode`,
  `RustroomApp::mask_add_mode`, as before). Without a selected mask the control is disabled
  with a hover "Select a mask to refine it" and the mode is forced back to New mask, so an op
  can never outlive its selection.
- **The kinds** are one row of five equal icon buttons (`mask_components::kind_button`): the
  kind's glyph, its name on hover and as the accessible name. A click in New mask (or with
  nothing selected) appends a new mask with that kind's template and selects it; in an op mode
  it appends `Refinement { op, mask: template }` to the selected mask and selects the new
  shape. One history entry either way. The templates and labels live in a lib module,
  `mask_kind.rs`: `MaskKind { Gradient, Radial, Brush, Luminance, Colour }`, `ALL`,
  `menu_label()` ("Gradient", "Radial", "Brush", "Luminance range", "Colour range"),
  `template() -> Mask`, `of(&Mask)`.
- A one-line hint under the buttons explains the modes.
- **Mask rows** are hand-painted, 22 pt: a kind glyph, the name (`"{label} {i+1}"` as today), a
  `×` cell at the right registered after the row. Selected: the accent tint (`TINT` as the rail).
  Accessible name = the row's name (`SelectableLabel`, selected state); the × is a `Button`
  named "×" with hover "Delete". Clicking the selected row deselects it, as today.
- **Component rows** (only for a refined, selected mask): indented 18 pt, an op glyph column
  (blank, −, ∩, +), the kind glyph, the kind name; the component being edited gets the
  inactive weak fill; refinements have a ×. Accessible names unchanged: the base is
  `mask.label()`, a refinement is `Refinement::label()` ("Subtract Radial").
- **Show mask** stays a checkbox, under the list, drawn in every state.
- The kind glyphs are painted by hand in a bin module `mask_glyph.rs` (`paint(painter, rect,
  kind, colour)`): gradient = three lines fading, radial = an ellipse in an ellipse, brush = a
  short thick stroke with a dot, luminance = a circle with its left half filled, colour = a drop
  (circle plus triangle). Not the icon font: the codepoints for these are unknown here and a
  wrong one ships a tofu box.
- With no mask selected: a hint under the checkbox ("Add a mask, then drag its handles on the
  image…").

### The SHAPE section

Header "Shape", not resettable, trailing text = the edited component's `label()`. Body is
today's shape block unchanged: Invert; Feather for radial; the luminance and colour range
sliders with their hints; the brush tool settings with their hint.

### The adjustment sections

`mask_adjust::show` draws four sections through `section::header`, ids salted `("mask", name)`
so they never share open state with the Edit panel's sections of the same name:

- **White Balance** (open): Temp, Tint.
- **Tone** (open): Exposure … Blacks, Tone Curve as a sub-label with the curve inline (today's
  collapsing header goes), Presence: Texture, Clarity, Dehaze.
- **Color** (collapsed): Vibrance, Saturation, HSL, Color Grading sub-label with the segmented
  range, wheel, Hue/Saturation/Luminance, Blending, Balance (today's collapsing header goes).
- **Detail** (collapsed): Noise Reduction, Sharpening, as today.

The dot and reset per section come from `edit-model`: a new `LocalGroup { WhiteBalance, Tone,
Color, Detail }` with `ALL` and `name()`, and on `LocalAdjustments` `group_differs(&self,
LocalGroup) -> bool` and `reset_group(&mut self, LocalGroup)`. Both go through one private
`copy_group(&mut self, from, group)` that destructures `LocalAdjustments` **exhaustively** (no
`..`), so a new field forces a decision about its group — the standing lesson about guards that
enumerate a list. Groups: WhiteBalance = white_balance; Tone = exposure, contrast, highlights,
shadows, whites, blacks, tone_curve, local_contrast, dehaze; Color = vibrance, saturation, hsl,
color_grading; Detail = noise_reduction, sharpening.

`show` keeps returning the curve's rect for the tests.

### Rail icons

`rail.rs`: `GLYPH_SIZE` 22 → 26, `RAIL_WIDTH` 44 → 48, so the tool icons read as the dominant
element. Its own commit.

## Out of scope

Mask names, drag-reordering, thumbnails, a per-mask on/off, the canvas overlay.

## Files

- new lib `crates/app/src/mask_kind.rs`; new bin `develop/panel/mask_glyph.rs`
- new `crates/edit-model/src/local_group.rs` (+ `pub use`)
- `develop/panel/section.rs` (builder, trailing), `adjustments.rs` and `panel/mod.rs`
  (builder call sites), `masks.rs`, `mask_components.rs`, `mask_adjust.rs`, `tool.rs`,
  `rail.rs`
- `docs/JOURNAL.md`, `CLAUDE.md` state

## Testing

- `mask_kind`: every kind's template is its own variant; labels distinct.
- `local_group`: for each group, one field moved → exactly that group differs; `reset_group`
  clears exactly it; a fully-changed set reset group by group ends at default.
- `section`: trailing text is not a widget.
- `masks`: the existing tests re-targeted — adding is the kind's button in New mask mode;
  refining is the op's segment then the kind's button; the op modes are inert without a
  selection and disarm on deselect; the Show mask checkbox flips `show_tint`; the MASKS reset
  removes every mask and clears the selection.
- `mask_components`: the component rows keep their accessible names; every kind button is
  named for accessibility.
- `mask_adjust`: the curve is laid out with Tone open; Color and Detail start collapsed;
  opening Color leaves the mask's grade range alone; the capture sliders reach their fields
  (Dehaze, Smoothness, Masking — "Color" and "Detail" now collide with the section names for
  the harness).
- `tool`: the unheaded tools are Edit and Masks.
- Gate: fmt, clippy `-D warnings`, `cargo test --workspace`, each exit code from its own
  command; then the GUI in release.
