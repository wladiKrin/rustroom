# Rearranging the Library panel's sections

**Date:** 2026-08-27
**Status:** design agreed, ready to plan.
**Asked as:** *"it would be nice to be able to rearrange the left outer panel, e.g. by dragging
folders below collections."*

## What this is

The Library panel's five sections — Folders, Collections, Smart collections, Keywords, Dates — are
collapsible and their open state is persisted, but their **order is literal code**: five
`section(..)` calls in `collections_panel::show`. This makes the order a value the user can drag.

## The blocker, and why it is not the drag

⚠⚠ **The sections are not data.** Each body is a distinct closure over different `app` fields, so
there is no list to permute. The drag gesture is the easy half; making the order a value is the
work, and it has to come first.

⚠ **One trap is already defused.** `picked` and `selected` used to be locals of the Collections
block that the Keywords block happened to see, because they were sequential statements in one
closure — moving Smart between them broke it, and the compiler caught it. Both are now read
**before** the panel, which is what makes an arbitrary order possible at all. Any future state
shared between sections must be read there too, or the order stops being permutable.

## The facts that shaped the design

Read out of the vendored egui 0.35 source, not assumed:

1. **`Ui::dnd_drag_source` registers a drag `interact` over the *whole* rect it wraps** (`ui.rs`
   line ~2679). Wrapping a section would therefore sense drags over every button, text field and
   list inside it — and hit-test ties go to whatever registered **last**, which is the tie-break
   rule that already broke Library thumbnails once. The drag source cannot be the section.
2. **The drag source's contents are simultaneously the sensing rect and the floating ghost.** A
   grip-only source gives a ghost that is just the grip; a header-sized ghost means the header
   senses drag. The API cannot give both without fighting it.
3. **`CollapsingHeader` has no custom-header slot**, but `CollapsingState::show_header` does — so a
   header can carry a grip.

## Architecture

### Sections become data

```rust
/// One section of the Library panel.
pub enum PanelSection { Folders, Collections, Smart, Keywords, Dates }
```

`Settings.panel_order: Vec<PanelSection>`, and `collections_panel::show` loops it, matching each
variant to its existing body **verbatim**. ⚠ The `match` is the whole trick: it lets five closures
over five different sets of `app` fields stay exactly as they are while their *order* becomes a
value.

⚠ **A `Vec`, not `[PanelSection; 5]`.** `settings.json` is hand-editable, so a stored order can be
the wrong length, list a section twice, or omit one. `Settings::panel_order()` returns a **repaired**
list — duplicates dropped, missing sections appended in default order — so **a section can never
become unreachable**, whatever is in the file. That is `FormatFilter`'s hidden-by-exception
robustness, and the reason the raw field is not read directly anywhere.

⚠ **The open/closed bools stay as the named `PanelSections` struct.** The `match` hands each arm
its own `&mut bool`. Nothing about collapsing changes, and no existing setting migrates.

### The grip

`section()` is rewritten on `CollapsingState::show_header`, because `CollapsingHeader` has no slot
for header content (fact 3). The grip is **six dots painted with `painter.circle_filled`** into a
small allocated rect that senses `Sense::drag()`.

⚠⚠ **Painted rather than a font glyph, deliberately.** This project has three recorded cases of a
chosen glyph being absent from the bundled fonts and shipping as tofu — the rail's four glyphs, the
mark badges, the keyword ticks — each found only by probing. A drag grip *is* six dots; drawing it
needs no font, no probe, and no coverage test, and it takes the theme's colour for free.

⚠ **The grip is the only thing that drags.** Click the header to collapse, drag the grip to
reorder; the two cannot be confused, which is what facts 1 and 2 rule out for any other choice.
`on_hover_cursor(CursorIcon::Grab)` is what makes it discoverable.

### Placement is a pure function

```rust
/// Where a section dragged to `pointer_y` should land, given each header's top edge.
pub fn drop_index(header_tops: &[f32], pointer_y: f32) -> usize;

/// The order after moving the section at `from` to position `to`.
pub fn moved(order: &[PanelSection], from: usize, to: usize) -> Vec<PanelSection>;
```

While a drag is held, `drop_index` gives the target and an insertion line is painted there; on
release, `moved` produces the new order.

⚠⚠ **This is why egui's payload machinery was rejected, and it is the main reason for the whole
approach.** With `dnd_drag_source`/`dnd_drop_zone` the placement logic lives inside egui and there
is nothing to test; here it is two pure functions with no `Ui` in sight. **Panel interaction is
exactly this project's untested territory** — the standing warning says to treat canvas and panel
work as untested, and this is the one part of the change that can be made otherwise.

### Persistence

Rides `Settings` beside `panel_sections`, written when the drag **settles** rather than per frame —
the `apply_view_demosaic` rule, now cited more than a dozen times in `CLAUDE.md`.

## Testing

- **`drop_index`**: above the first header, below the last, exactly on a boundary, and an empty
  list. ⚠ The boundary case is the one a naive implementation gets wrong and no manual test would
  notice.
- **`moved`**: forward, backward, to the same position (a no-op), and out-of-range indices, which
  must be clamped rather than panic — the order comes from a hand-editable file.
- **`Settings::panel_order()` repairs**: a stored order missing a section appends it; a duplicate is
  dropped; an empty stored order gives the full default; an unknown variant cannot occur because
  serde rejects it and the container-level `#[serde(default)]` then gives the default order.
- **Round trip through `Settings`**, including a settings file written before the field existed —
  the sibling of `a_settings_file_from_before_the_snapshot_strip_still_loads`.
- ⚠ **The drag itself, the grip's appearance and the insertion line are not testable here** and are
  verified by hand with the app running. Said plainly rather than covered by a test that only looks
  like it covers them.

## Accepted caveats

1. **No reset to default order** — an unwanted drag is undone by dragging back.
2. **No undo for a reorder.** The undo log is scoped to photos and global actions, and a panel
   layout change is neither; it would be the first purely cosmetic entry.
3. **The grip is a new affordance that has to be noticed.** The grab cursor on hover is the whole
   mitigation.
4. **The order is global**, not per folder or per catalog — the same scope `panel_sections` has.
5. **Only these five sections.** The panel has no other content to order.
