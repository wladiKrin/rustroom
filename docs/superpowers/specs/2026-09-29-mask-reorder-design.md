# Reordering masks and a mask's components

**Date:** 2026-09-29 · **Branch:** `mask-reorder` · **Status:** design approved in brainstorm

Backlog entry *"Reorder a mask's components"* (recorded 2026-09-10, the author's *"the reordering is
something we should do at some point"*), widened in brainstorm to the masks themselves.

## Why both orders matter

- **Components.** A mask is a base shape plus refinements folded left to right by `MaskOp`
  (`MaskedAdjustment::refine`). Add is not commutative with Subtract or Intersect, so
  *(Gradient − Radial) ∪ Brush* and *(Gradient ∪ Brush) − Radial* differ.
- **Masks.** The tail builds one `MaskStage` per mask in list order (`pipeline/src/lib.rs`,
  `for (i, m) in edits.masks.iter().enumerate()`), so a later mask adjusts the output of the earlier
  ones: a luminance range after an exposure mask selects different pixels, and mask 2's curve bends
  mask 1's result.

Both lists are **append-only, delete-any** today.

## Decisions (the author's)

- **Masks and components**, one gesture for both (brainstorm question 1).
- **Any component may move anywhere, including the base, as long as the row that ends on top is an
  Add** (question 2). A drop that would put a Subtract or Intersect on top is not offered: either op
  onto nothing leaves nothing. Every order that changes the picture stays reachable, and nothing is
  reinterpreted.
- **Drag the whole row** — the Library sections' gesture — rather than a grip or Move up/down
  actions.

## Behaviour

- **Mask rows** drag up and down to change the order the masks apply in. **Component rows** (shown
  under the selected mask) drag to change the order its shapes combine in. A click still selects; a
  drag past egui's threshold moves.
- **While dragging:** a 2-pt accent line (`selection.bg_fill`) at the insertion slot, as in the
  Library rail. The slot is decided by the rows' **midpoints** — the upper half of a row is
  "before it", the lower half "after it" (the plan's refinement: the Library's past-the-top rule
  makes a drop on the row above a no-op at 30 pt). For components, a slot the top-row rule forbids
  draws **no line**, and releasing there does nothing.
- **The base is an Add onto nothing**, which is what makes base moves exact: when a refinement
  becomes the top row its `Add` is dropped (the op column goes blank), and the old base becomes an
  `Add` refinement in its new place (the op column shows `+`). Coverage is unchanged by that
  exchange itself; it changes only where the order genuinely changed.
- **One drop is one undo step** (`Edited::discrete`). A drop that leaves the order unchanged
  records nothing.
- **Selection follows the row.** The selected mask stays selected at its new index; the component
  being edited stays the one being edited, whichever row moved. `active_mask_handle` is cleared, as
  after a delete.
- **The overlay eye moves with its mask** (`Session::mask_shown` is permuted with `edits.masks`).
  `disabled` lives in the mask, so it moves anyway.
- **Names stay positional** — "Radial 2" moved to the top reads "Radial 1", as after a delete.
- **Hover:** each row gets one sentence under `every_hover_is_one_short_sentence`:
  - mask row: *"Selects this mask, and a drag moves it to change the order the masks apply in"*
  - component row: *"Selects this shape, and a drag moves it to change the order the shapes combine in"*
- **Cursor:** the arrow, per `DESIGN.md` §9.
- **No Settings binding.** A primary-button drag on a list row, as the Library sections are; not a
  canvas gesture, so `PanDrag`'s precedent does not apply.

## Placement

**Lib (no egui):**

- `panel_order::moved` becomes generic — `moved<T: Clone>(order: &[T], from, to) -> Vec<T>`. The
  one caller (`collections_panel`) and the existing tests are unchanged. `drop_index` is reused as
  is.
- New module **`mask_order.rs`**:
  - `index_after_move(i, from, to, len) -> usize` — where the item at `i` ends up after
    `moved(_, from, to)`. Used for both `selected_mask` and `selected_component`.
  - `component_drop_allowed(m: &MaskedAdjustment, from, to) -> bool` — true when the row that
    would end on top is the base or an `Add` refinement, and the move is in range.
  - `move_component(m: &mut MaskedAdjustment, selected: &mut usize, from, to) -> bool` — false
    when the drop is not allowed or changes nothing; otherwise the composite rebuilt in place from
    the new order (the top row's shape is `mask`, every other row a `Refinement` keeping its op, the
    old base as `Add`; `adjustments` and `disabled` untouched) and `selected` moved with its shape.
  - `move_mask(masks, shown, selected: &mut Option<usize>, from, to) -> bool` — the masks and the
    eye flags by one permutation, the selection with its mask (the plan's refinement: both moves
    update their selection themselves, so the bin cannot apply one and forget the other).

**Bin:**

- `mask_components::row` senses `click_and_drag` and reports `drag_started`, `dragging` and
  `released` in `RowOut`, plus the row's top edge; the hover sentence is a new `RowSpec` field
  (patch and preset rows that share `row` pass none and keep click-only sensing).
- `masks::list` collects each mask row's rect, paints the line from `panel_order::drop_index` over
  the rows' midpoints, and on release calls `mask_order::move_mask`.
- `mask_components::rows` does the same for components through `component_drop_allowed` /
  `move_component`.
- Drag state across frames is egui's own (`dragged()` on a stable row id), so no new `Session`
  field. ⚠ If a row's id turns out unstable across the drag, give the row an explicit id rather
  than adding a session field.

**Unchanged:** `edit-model`, the sidecar format, the pipeline, the GPU twins. The brush coverage
cache is keyed on content with **no component index** (`render_engine::BrushCache::key`, whose doc
records the sabotage that proved it), so a component move cannot serve a stale map; a gating
brush's base identity is `Edits` truncated to the masks before it, so a mask move misses rather
than hits stale.

⚠ `masks.rs` is 518 code lines and `mask_components.rs` 383; the added code in each is expected at
a few dozen lines. If either crosses ~600, report it.

## Error handling

Indices come from a drag and a pointer and are never trusted: `moved` already clamps, and
`move_component` / `index_after_move` return `None` / the input unchanged for anything out of
range. A mask list that changed under a live drag (an undo mid-drag) makes the drop a no-op, never
a panic in the paint loop.

## Accepted cost

Undoing a mask reorder restores the masks' order but not their eye flags — `mask_shown` is view
state outside the edits. Undoing a mask delete has the same flaw today. Left as is unless the
author asks.

## Tests

**`mask_order` (pure):**

- For a composite of base + Add/Subtract/Intersect refinements, over **every** `(from, to)`:
  a result is a permutation of the shapes; it exists exactly when the new top row is the base or an
  `Add`; every shape keeps its op except the base (→ `Add`) and the new top (op dropped, and it was
  `Add` or the base).
- ⚠ **Prove they differ first** (standing lesson (c)): with analytic radial/linear shapes, fold
  `analytic_coverage` through `MaskOp::combine` at sample points and assert a chosen reorder
  changes coverage by a clear margin **before** asserting anything else; and that exchanging the
  base with an adjacent `Add` changes nothing.
- `index_after_move` agrees with `moved` applied to `0..len` for every `(i, from, to)`.
- `panel_order::moved` keeps its existing tests on `PanelSection`.

**Panel harness (`develop/panel/masks.rs` tests, real pointer events):** a new
`drag_row(from_label, to_label)` helper pressing on one row's centre and releasing on another's.

- Dragging a component row reorders the composite, is **one** discrete edit, and the edited
  component follows it.
- Dragging a Subtract row onto the top changes nothing and records no edit.
- Dragging a mask row reorders `edits.masks` **and** `mask_shown`, and the selected mask follows.
- Dropping a row where it started records no edit.
- The row hovers pass `every_hover_is_one_short_sentence`.

**Sabotages** (each must turn a test red): skip `component_drop_allowed`; skip the selection
update; skip permuting `mask_shown`; make `moved` ignore the removal shift.
