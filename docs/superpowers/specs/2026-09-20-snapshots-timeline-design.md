# The snapshot strip as a timeline — design

**Date:** 2026-09-20. **Branch:** `snapshots-timeline`. **Status:** option B of the canvas
"Rustroom Snapshots" (https://claude.ai/artifact/7vkX5EcCpcP7BgaVfGzXy8), chosen in conversation
with the copy cell added to every row.

## Why

The strip was the last list without the section chrome: a plain "Snapshots (n)" label with a
`+`, rows of a selectable label and a caption, and Rename / Make virtual copy / Delete in a
right-click menu nothing points to. Nothing said which snapshot the photo is at now.

## The strip

Still a resizable bottom panel under the adjustments, drawn under every tool.

```
› SNAPSHOTS                                   3 snapshots
  ◌  Now      4 changes since “Warm”           [+ Save]
  ●  Warm                     15:41            ⧉   ×
  ○  Snapshot 2               15:07            ⧉   ×
  ○  Before the crop          yesterday 14:32  ⧉   ×
```

- **Header**: `section::header`, trailing text the count ("3 snapshots", "1 snapshot",
  "no snapshots"); collapsing hides the timeline and the panel sits at its minimum height.
- **The Now row** is not a snapshot. A dashed dot, "Now" in the weak face, and what the edits
  have done since the newest snapshot: "n changes since “Warm”" from the history (the entries
  pushed after the most recent one equal to that snapshot's edits; "changed since “Warm”" when
  that entry has been undone past), "same as “Warm”" when the edits equal it, "no snapshots yet"
  when there are none. It carries **+ Save** (accessible name "Take snapshot", the menu's
  wording), so taking a snapshot reads as pinning what is now.
- **One row per snapshot, newest first**, on a vertical hairline: a dot per row, filled with
  the **accent** when the photo's current edits equal that snapshot ("you are here"), hollow
  otherwise; the name in the body face; the time in the weak face — `HH:MM` today, `yesterday
  HH:MM`, else `Mon D HH:MM`; then two cells at the right, **⧉ copy** ("Make a virtual copy
  from {name}") and **× delete** ("Delete {name}"). A click restores, as today. A double-click
  or the menu's Rename opens the inline field, which commits on Enter and on a click elsewhere
  as today (those two tests stay). The context menu keeps its three items.
- The rows go through `list_row::row`, which gains `rect` and `double_clicked` in its output so
  the strip can paint the dots and the hairline in the rows' gutter; the glyph cells take an
  icon-font character (`icons::COPY`, Phosphor `copy`, verified in the bundled cmap).

## Files

- `crates/app/src/history.rs` (lib): `steps_since(&self, is: impl Fn(&T) -> bool) ->
  Option<usize>`, the count of entries after the most recent matching one up to the present.
- **New** `crates/app/src/when.rs` (lib): `fn short(taken: i64, now: i64) -> String`.
- `icons.rs`: `COPY`. `list_row.rs`: `RowOut::{rect, double_clicked}`.
- `develop/panel/snapshots.rs`: rewritten drawing; `apply` unchanged.
- `docs/JOURNAL.md`, `CLAUDE.md`.

## Testing

- lib: `steps_since` counts from the latest match and is `None` without one; `when::short`
  picks the three forms.
- strip: present under every tool (the Take-snapshot button, by its new name); a named
  snapshot shows its name; the matching row reports selected and a changed one does not; the
  copy and delete cells are named; the Now row says "same as" after a restore; the two rename
  commit tests as before.
- Gate; GUI in release.
