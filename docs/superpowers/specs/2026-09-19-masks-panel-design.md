# The Masks panel: menus, not modes — design

**Date:** 2026-09-19. **Branch:** `masks-panel`. **Status:** option A of the canvas
"Rustroom Masks Panel" (https://claude.ai/artifact/E7CgHLYEyoV3jwd1os18SB), approved in
conversation. Also carries one unrelated tweak the user asked for at the same time: larger rail
icons.

## Why

The Masks panel after the restyle still reads as a pile. Adding anything takes two rows read
together: a mode chip (New / Add / Subtract / Intersect) that is invisible state, then one of five
"Add …" buttons wrapping over three lines. Masks and their components are the same button style,
only indented, with the × on the left. Below the shape, about forty sliders arrive as one flat
run grouped by separators and two lone collapsing rows. "Show mask", the hint and "Remove all
masks" float loose at the bottom.

## The panel, top to bottom

```
› MASKS •                         overlay   reset
                                   [+ Add mask ▾]
  ◐ Gradient 1                                 ×
  ◐ Radial 2                    (selected, tinted) ×
      ◐ Radial                  (component being edited)
    − ◐ Brush                                  ×
    ∩ ◐ Luminance                              ×
    Refine  [Add ▾] [Subtract ▾] [Intersect ▾]
    hint: Add widens the mask, Subtract cuts out of it, Intersect keeps only where both reach.
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
- `section::header` gains two optional trailers, drawn left of `reset`: a **toggle** (label,
  on/off, returns clicked; accessible as a checkbox named by its label) and a **trailing text**
  (weak, non-interactive). `Header` becomes a builder: `Header::new(name).modified(b)
  .resettable(b).toggle(label, on).trailing(text)`; the return is `HeaderOut { state, reset,
  toggled }`. The two existing callers move to the builder.
- MASKS: `modified` = any masks; `reset` = today's "Remove all masks" (same group merge, same
  selection clearing, tooltip says what it deletes); toggle `overlay` = `show_mask_tint`, the
  same flag the View menu's "Show mask overlay" flips.
- **Add mask** is one `menu_button("Add mask")` right-aligned in a row pinned to its width
  (`allocate_ui_with_layout`, never a bare `with_layout` — the 2026-09-19 growth bug). Its
  items are the five kinds; clicking one appends a new mask with that kind's template, selects
  it, and is one history entry. The templates and menu labels move to a lib module,
  `mask_kind.rs`: `MaskKind { Gradient, Radial, Brush, Luminance, Colour }`, `ALL`,
  `menu_label()` ("Gradient", "Radial", "Brush", "Luminance range", "Colour range"),
  `template() -> Mask`, with the existing doc comments on each default.
- **Mask rows** are hand-painted, 22 pt: a kind glyph, the name (`"{label} {i+1}"` as today), a
  `×` cell at the right registered after the row. Selected: the accent tint (`TINT` as the rail).
  Accessible name = the row's name (`SelectableLabel`, selected state); the × is a `Button`
  named "×" with hover "Delete". Clicking the selected row deselects it, as today.
- **Component rows** (only for a refined, selected mask): indented 18 pt, an op glyph column
  (blank, −, ∩, +), the kind glyph, the kind name; the component being edited gets the
  inactive weak fill; refinements have a ×. Accessible names unchanged: the base is
  `mask.label()`, a refinement is `Refinement::label()` ("Subtract Radial").
- **Refine row** under the components, indented: weak "Refine", then three `menu_button`s
  labelled by `MaskOp::label()` (Add, Subtract, Intersect), each listing the five kinds. A
  click appends `Refinement { op, mask: kind.template() }` and selects the new component. Hover
  text on each op from `chip_label()` plus a sentence. The mode chips, `chips()`,
  `MaskUi::add_mode` and `RustroomApp::mask_add_mode` are removed.
- The kind glyphs are painted by hand in a bin module `mask_glyph.rs` (`paint(painter, rect,
  kind, colour)`): gradient = three lines fading, radial = an ellipse in an ellipse, brush = a
  short thick stroke with a dot, luminance = a circle with its left half filled, colour = a drop
  (circle plus triangle). Not the icon font: the codepoints for these are unknown here and a
  wrong one ships a tofu box.
- With no masks: the Add mask row and a hint ("Add a mask, then drag its handles on the
  image…"). With a mask selected and no refinement: no component rows, but the Refine row.

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
- `develop/panel/section.rs` (builder, toggle, trailing), `adjustments.rs` and `panel/mod.rs`
  (builder call sites), `masks.rs`, `mask_components.rs`, `mask_adjust.rs`, `tool.rs`,
  `main.rs` + `test_app.rs` (drop `mask_add_mode`), `rail.rs`
- `docs/JOURNAL.md`, `CLAUDE.md` state

## Testing

- `mask_kind`: every kind's template is its own variant; labels distinct.
- `local_group`: for each group, one field moved → exactly that group differs; `reset_group`
  clears exactly it; a fully-changed set reset group by group ends at default.
- `section`: a toggle is reachable by its label and reports a click; trailing text is not a
  widget.
- `masks`: the existing tests re-targeted — adding goes through "Add mask" then the kind's
  label; refining through the op's menu then the kind; the overlay toggle flips `show_tint`;
  the MASKS reset removes every mask and clears the selection.
- `mask_components`: the component rows keep their accessible names; the Refine row offers
  the three ops.
- `mask_adjust`: the curve is laid out with Tone open; Color and Detail start collapsed;
  opening Color leaves the mask's grade range alone; the capture sliders reach their fields
  (Dehaze, Smoothness, Masking — "Color" and "Detail" now collide with the section names for
  the harness).
- `tool`: the unheaded tools are Edit and Masks.
- Gate: fmt, clippy `-D warnings`, `cargo test --workspace`, each exit code from its own
  command; then the GUI in release.
