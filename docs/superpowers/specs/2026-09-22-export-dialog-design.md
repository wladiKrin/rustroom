# The Export dialog: a preset list beside summarised sections — design

**Date:** 2026-09-22. **Branch:** `export-presets-sheet`. **Status:** option C of the canvas
"Rustroom Export" (https://claude.ai/artifact/GG1AUf12BdF9hhxs7xPgJf), chosen in conversation.

## Why

After the dialogs rework the Export dialog had four tabs, and its Output tab was a grab-bag —
the preset row, the destination, the format grid, quality, the size controls, the demosaic tier
and output sharpening — while nothing on screen said what the other tabs were set to. And the
presets, which are how a photographer exports, were a combo in a row.

## The dialog

`export_settings`, **Wide** (640 × 480 default), title `Export 3 photos` (`Export 1 photo`,
`Export` with none). Two columns under the title bar; the footer as the shell gives it.

**Left, 170 pt, a hairline on its right:** a `sub_label` PRESETS, then one `list_row` per
export preset (name; the row **lit** when the current options equal that preset's; a click
applies it; a `×` cell named "Delete {name}" deletes it through the same confirm-or-not path
`Delete` took), a weak "No presets yet." when there are none, and at the bottom a **+ Save
current…** button (accessible name "Save current as preset…", opening `SaveExportPreset` as
`Save as…` did). Deleting from a row selects that preset first, so
`delete_selected_export_preset` and the confirm dialog keep working unchanged.

**Right, the rest:** the panels' `section::header`s, each with its state as **trailing text**
so the collapsed list is a summary of the whole export:

| section | trailing | body |
|---|---|---|
| Location | the folder's last component, "(none chosen)", or "… (missing)" | `Choose…` and the full path |
| Naming | the first file's rendered name, or "invalid" | the template field, one hint line listing the tokens, the parse error as an error hint |
| File | `JPEG · sRGB · 92` (the quality only when lossy) | Format, Colour space, the quality slider, the banding warning |
| Size | `2048 px long edge` or `original` | the checkbox and the slider |
| Sharpening | the amount, or `off` at 0 | the slider |
| Demosaic | the tier's label | the combo |
| Metadata | the scope's label, lowercased | the grid |
| Watermark | the text, or `off` when empty | the grid and the two sliders |

Location, Naming and File open by default; the rest closed. Open/closed is egui's collapsing
memory, per process, like the Develop panel's sections. The long hints become hovers on their
control (the Settings rule); the naming hint stays inline because it is the token reference.

**Footer:** primary **Export** (`can_export`), dismiss **Cancel**, note: the preset the current
options equal ("Web · 2048 sRGB"), else "changed from “Web · 2048 sRGB”" when one is selected,
else empty.

The summaries are pure functions in a new lib module `export_summary.rs`, each with a test.

## Files

- **New** `crates/app/src/export_summary.rs` (lib): `location`, `file`, `size`, `sharpening`,
  `demosaic`, `metadata`, `watermark`, `matching_preset`.
- `modals/export.rs`: `settings` rewritten; the tab bodies become section bodies;
  `tab_hover` goes; the three tab tests replaced by section, preset-row and footer tests.
- `docs/JOURNAL.md`, `CLAUDE.md`.

## Testing

- lib: each summary on a default and a changed `ExportOptions`.
- dialog: the eight section names are reachable; a preset row is named and lit when the
  options match; "Delete {name}" and "Save current as preset…" are named; "Export" and
  "Cancel" are in the footer; the title carries the count.
- Gate; GUI in release.
