# The Presets panel: a sectioned list — design

**Date:** 2026-09-20. **Branch:** `presets-panel`. **Status:** option A of the canvas "Rustroom
Presets Panel" (https://claude.ai/artifact/SNkHg7KEWwUDR9xofcpgri), approved in conversation.

## Why

A plain heading, two full-width text buttons (one of which comes and goes), and one row per
preset made of a `×` button and a name button. Nothing says which preset the photo already has,
nothing says what a preset carries, and "No presets yet." is body text.

## The panel, top to bottom

```
› PRESETS                                    5 presets
                                        [+ Save current…]
  ☆ Bright & airy      all settings                    ×
  ☆ Film · warm        Tone · Color        (tinted)    ×
  ☆ Portrait base      Tone · Color · Detail           ×
                                    ↩ Undo delete “Moody”
```

- `Tool::panel_heading` goes altogether: every tool now draws its own section header, so the
  method and the heading row in `panel/mod.rs` are removed, with their test.
- **PRESETS** header, trailing text = the count ("5 presets", "1 preset", "no presets"). Not
  resettable: there is no "clear all presets".
- **Save current…** right-aligned under the header in a row pinned to its width, opening the
  naming dialog as today. Accessible name "Save current as preset…" (the old label), so the
  harness and a screen reader keep the sentence.
- **One row per preset** through `mask_components::row`: a star glyph (`RowGlyph::Preset`,
  hand-painted), the name, and in weak meta what it carries — `presets::groups_summary`:
  "all settings" when every edit group is on, else the on groups' names joined with " · ".
  Clicking the row applies the preset to the selection exactly as the name button did; the `×`
  ("Delete {name}") deletes with the confirm guard as today. The row is **tinted** when
  `Preset::matches(&edits)`: merging the preset into the open photo's current edits would
  change nothing, so the photo already has it. The hover on a row lists the groups in full.
- With no presets: one weak line, "No presets yet. Save current… keeps this photo's settings."
- **Undo delete** as a small right-aligned button under the list while that is what undo would
  do (same `ActionKind::PresetDelete` peek), labelled "↩ Undo delete “name”" as today.
- The "Applied … to n photos" message stays in the status bar; the canvas's echo line under
  the list is dropped — the status bar already says it, and a second copy would be one more
  piece of state to age.

## Files

- `crates/app/src/presets.rs` (lib): `groups_summary(EditGroups) -> String`,
  `Preset::matches(&self, &Edits) -> bool`, tests
- `develop/panel/presets.rs` (rewrite), `mask_components.rs` (`RowGlyph::Preset`),
  `mask_glyph.rs` (the star), `tool.rs` + `panel/mod.rs` (no `panel_heading`)
- `docs/JOURNAL.md`, `CLAUDE.md` state

## Testing

- lib: `groups_summary` says "all settings" for `EditGroups::ALL`, names the groups otherwise;
  `matches` is true for a preset whose groups carry what the edits already are and false when
  they would change something.
- panel: saving opens the dialog; deleting asks while the guard is armed and not otherwise;
  applying with nothing selected says so; an empty list offers nothing to delete; Undo delete
  appears only when that is what undo would do; the matching preset's row reports selected.
- Gate as usual; GUI in release.
