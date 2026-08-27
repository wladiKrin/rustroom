# Keyword and menu polish

**Date:** 2026-08-27
**Status:** specced
**Origin:** six items reported from using the finished M4.3c. Every one came from real use.

## The six

1. Typing a name and pressing **Enter** should create the collection / keyword / smart collection.
2. The photo **context menu** should offer *add to collection* and *add keyword*.
3. The context menu should work on a **selection**, not only on one photo.
4. Dialogs are small and cramped → **backlog only**, not built here.
5. It should be possible to **remove a keyword** from photos.
6. **Hovering a filmstrip thumbnail** should show the photo's name.

---

## 1. Enter creates

`TextEdit` returns a `Response`; the idiom is `lost_focus() && key_pressed(Enter)`.

⚠ **The non-empty guard is computed once and shared with the `+` button**, so Enter and `+`
cannot disagree about what is a valid name. Two guards for one rule is how they drift.

⚠ Enter, not plain `lost_focus()`: clicking away is abandoning, not committing — the rule the
copy-rename field already follows.

## 2, 3, 5. One tri-state keyword toggle, shared by both views

The menu gains two submenus:

- **Add to collection ▸** — one entry per collection, applying to the whole selection.
- **Keywords ▸** — one entry per keyword, **checked** when *every* selected file carries it,
  unchecked when none does, and a `–` when some do. Clicking assigns to all, or removes from all
  when all already have it.

⚠⚠ **The tri-state toggle is what makes "remove a keyword" a real feature rather than a second
button**: it is the same control, and it also answers *"what keywords does this photo have"* —
the other recorded gap — without a new panel.

⚠⚠ **The count must be of DIRECT assignments, never of the subtree.** `keyword_members` expands
descendants because a *search* for `Europe` must find `Paris`. A menu toggle removes an
**assignment**, so a photo tagged only `Paris` must show `Europe` unchecked — otherwise
unticking `Europe` appears to do nothing.

⚠ **`Rename` and `Information…` are disabled above one photo**, with a hover saying so. Both are
single-photo by nature; hiding them instead would make the menu change shape.

⚠ **The menu is built for what the click is ABOUT to select.** `menu_select` is applied after the
frame, so right-clicking an unselected photo leaves `lib.selection` holding the *old* set while
the menu is drawn. `selected_files` therefore returns the clicked photo alone when it is not in
the selection.

⚠ **The whole menu body is one shared function**, so the grid and the filmstrip cannot drift —
they already duplicated three buttons between them.

⚠ **The catalog queries live INSIDE the menu closure**, which egui runs only while the menu is
open. Reading them beside the grid would run three queries at 60 Hz — the `apply_view_demosaic`
mistake in another costume, cited nine times in CLAUDE.md.

⚠ Keywords are per **file** and collections per **copy**, so the file list is de-duplicated for
keywords and the copy list is not for collections. `assign_keyword_to_selection` already does the
first; this must not undo it.

### The panel gains a `−` beside its `+`

Completing the existing mechanism rather than adding a second one: the `+` is already there and
`unassign_keyword` already exists and is tested. Both go through one pair of functions the menu
also calls.

## 6. Filmstrip hover

`resp.on_hover_text(entry.label())` — **`label()`, never `file_name`**, the one rule the Library
caption, the rename field and the default export name already share.

⚠ The grid needs none: it draws the caption already.

## Testing

- `keyword_counts` — a file with two keywords, a file with one, one with none; the counts are per
  keyword and **do not include descendants** (its own test, since that is the trap).
- `selected_files` — returns the selection when the clicked photo is in it, and the clicked photo
  alone when it is not.
- Enter and `+` accept exactly the same names (the shared guard).

### Sabotages

- `keyword_counts` counts the subtree → the descendants test fails
- `selected_files` always returns the selection → the not-in-selection test fails
- Enter creates without the non-empty guard → the shared-guard test fails

⚠ Each checked for a `test result:` line, not for `^error:`.

## Accepted caveats

1. **No keyword shown on the thumbnail** — the menu is the only place a photo's keywords appear.
2. **The tri-state is per selection**, so a mixed selection shows `–` and one click makes it
   uniform; there is no "remove from only those that have it" separate from that.
3. Collections in the menu are add-only — removing from a collection is still panel-only, because
   a collection has no per-photo view to remove from.
4. **Dialog sizing is backlogged**, not fixed here.
