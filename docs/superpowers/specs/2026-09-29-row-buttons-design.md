# Row buttons: always lightly visible, popping on hover, tighter

**Date:** 2026-09-29 · **Branch:** `m10.6a-immich-timeline` (a round on M10.6a's rows, confirmed by use
the same day) · **Status:** design approved

The author, after trying M10.6a: *"the x and download button in the immich folder, as well as the x
buttons in all the other folders, are always, lightly visible with a greyed out color, and their color
only pops when hovering above said folder. Also make the layout of all buttons tighter."* Scope chosen:
**every list row's buttons** — one change to `list_row`, so the rail (folders, collections, smart
collections, keywords, Immich albums) and the snapshot and export-preset lists alike.

## Today

`list_row::row_with` registers every action cell but **paints the glyphs only while the pointer is on
the row** — P-13 of the first-session review (*"a × on every folder row at all times read as a standing
threat"*). Each cell is `ROW_HEIGHT` (30 pt) wide for a 16 pt glyph. ⚠ **This reverses P-13, by the
author's decision (2026-09-29).**

## Behaviour

- **Always drawn**, on every row, whatever the pointer does.
- **Three states, existing tokens only** (`theme::tokens`):
  - at rest: **`text3`** — the counts' grey; 3.6:1 on the dark panel, 3.1:1 on the light one, above
    `DESIGN.md` §10's 3:1 non-text floor;
  - the pointer anywhere on the row: **`text`**, the name's colour — the pop;
  - the pointer on one cell: that cell also gets the **`raised`** fill (radius `tokens::RADIUS`) — a
    button's face — so the one a click will hit is plain (§9: hover changes fill only). ⚠ Not `hover`,
    as first written: the row under the pointer already carries the `hover` fill, so a `hover` cell on it
    would be invisible. `raised` is darker than `hover` in the dark theme and lighter in the light one,
    so it shows in both.
- **A disabled action** (Tag/Untag with nothing picked) is `text3` at 45 % in every state, and gets no
  fill: it neither pops nor claims the click.
- **Tighter:** each cell is **20 pt** wide — a new `tokens::ROW_ACTION`, since sizes are tokens — and
  stays the row's 30 pt tall (§10's 28 pt target). The count gap is unchanged; a name gains 10 pt per
  action before it is elided.

## Placement

`theme/tokens.rs` (the constant) and `list_row.rs` (the cells, the colours, the painting). `DESIGN.md`
§7.6 rewritten for the new rule, §5 gains the token, §13 logs the decision; the `list_row` comment that
cites P-13 says it was reversed and by whom.

## Tests (`list_row`)

- **Always drawn:** with no pointer, both glyphs are painted.
- **The pop:** the glyph colour at rest is `text3` and under a pointer on the row is `text` — asserted
  different first (standing lesson (c)).
- **Tighter:** two actions' cell centres, read from AccessKit, are `ROW_ACTION` apart.
- The existing `every_count_in_the_rail_ends_at_one_x` still holds.
- **Sabotages:** paint only under the pointer again; one colour for rest and hover; cells back at
  `ROW_HEIGHT` — each must turn a test red.
