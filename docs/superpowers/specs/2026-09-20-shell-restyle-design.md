# The top bar and the Library in one language — design

**Date:** 2026-09-20. **Branch:** `shell-restyle`. **Status:** option A of the canvas "Rustroom
Top Bar & Library" (https://claude.ai/artifact/GTy76KeL7FHx3cdzkQssPW), approved in conversation.

## Why

The top bar is one row of egui defaults: Library / Develop are two selectable labels at the
same size, colour and weight as every other control, Grid / Compare / Survey three more of the
same, and search, Filter, sort, the zoom icons, Reindex and the format checkboxes all sit at one
weight. Nothing says which control is the app's mode. The Library body has none of the Develop
panel's language: plain 14 pt words head its lists, rows are loose buttons with the count baked
into the label, and the grid cells overlay the marks on the picture.

## The top bar

One row, ~40 pt, both views. Left to right:

- **The app mode**, a 32 pt segmented pill with icon + label — Library (`squares-four`),
  Develop (`sliders`) — and the lit half filled with the **accent**. It is the one accent-filled
  control in the bar, so it answers "where am I" at a glance. Accessible names stay "Library" /
  "Develop".
- **Library:** the layout mode as a second, 28 pt pill with icons — Grid (`grid-four`),
  Compare (`columns`), Survey (`grid-nine`) — the same weight for all three, hovers as today
  (`Thumbnails (G)` …). Then the search field, 200 pt, its hint carrying the magnifying glass;
  then Filter with a funnel glyph. ⚠ The Filter button keeps its descriptive active label
  (`Filter: 3★ ⚑`) rather than the canvas's count badge: that wording was a use-reported fix and
  a count says less. The right-aligned cluster, rightmost first: a ⋯ menu holding Reindex; the
  formats as **chips** (a dot, accent when shown, dim when hidden, then `RAF 120`) — the same
  multi-select semantics as the checkboxes; a thumbnail-size **slider** (80 pt, saved when the
  drag settles) with the `arrows-out-simple` glyph; the sort combo with a `sort-ascending` glyph
  in its text; then the status / export progress in weak text.
- **Develop:** undo/redo as 30 × 28 pt icon buttons; the four zoom actions abutted in one dark
  well (−, Fit, 1:1, +) — momentary, none lit; Before / After as a labelled toggle with its
  glyph; and **Export…** as the one accent-filled button, at the far right, running
  `Command::Export`. The colour readout and the status stay where they are, left of it.

## The Library body

- **The left panel** takes the Develop panel's section chrome: `section::header` with the
  count as trailing text (`FOLDERS 3`, `COLLECTIONS 154`), the chevron leading the name. The
  header row gains a `draggable` flag so the panel's section reordering keeps working through
  it (`Sense::click_and_drag`; a click that became a drag is not a click, by egui's contract).
  The rows go through a new `list_row` painter: name, count in weak text, the accent tint on
  the open one, and the actions at the right edge as glyph cells — `×` delete/forget, `+` add
  the selection, `−` untag, a gear for a smart collection's rule — each named for a screen
  reader ("Delete Portraits"). The new-item fields stay. The Dates tree is unchanged.
- **The grid cell:** the thumbnail with rounded corners; the copy badge as today; the filename
  centred; and the marks in a **row under the picture** (stars left, flag and colour label
  right) instead of over it. The selected cell's border is the accent (the old fixed blue
  goes), and its name is drawn in the strong text colour. The filmstrip's cells keep the
  overlay marks — a strip cell has no room under the picture — and Compare / Survey keep
  `paint_marks`.

## Files

- `crates/app/src/icons.rs`: LIBRARY, DEVELOP, GRID, COMPARE, SURVEY, FILTER, SORT, EXPORT,
  MORE, THUMB_SIZE, GEAR (verified against the bundled 2.1.2 cmap and the 2.1.2 stylesheet).
- **New** `crates/app/src/segmented.rs` (bin): the three `segmented*` helpers and
  `SEGMENTED_HEIGHT` move here from `theme.rs` (re-exported, callers untouched — `theme.rs` is
  923 lines), plus `pill` (content-sized, icon + label, optional accent) and `chip`.
- **New** `crates/app/src/list_row.rs` (bin): the panel row painter.
- `develop/panel/section.rs`: `Header::draggable`, `HeaderOut::response`.
- `collections_panel.rs` (rewrite of the drawing; the action enum and its dispatch unchanged),
  `thumb.rs` (`cell_with`, `marks_row`, accent border), `library.rs` (the cell), `shell.rs`
  (the bar), `develop/mod.rs` (the toolbar).
- `docs/JOURNAL.md`, `CLAUDE.md`.

## Testing

- `segmented`: a pill's items are reachable by their labels and a click switches; its width
  is content-sized, not the available width; a chip reports its state as a checkbox.
- `list_row`: a click on the row opens, a click on an action does not; the actions are named.
- `section`: a draggable header still toggles on a click.
- `thumb`: a grid cell with a marks row keeps its size however its thumbnail loads (the
  existing shape test, extended); the marks row's height is constant.
- `shell`: the filter label tests as before; every new icon renders (the `icons::ALL` sweep).
- Gate; GUI in release.
