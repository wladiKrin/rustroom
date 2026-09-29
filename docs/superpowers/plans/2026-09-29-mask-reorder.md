# Reordering masks and a mask's components — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:executing-plans. Steps use `- [ ]`.
> **Commits carry no `Co-Authored-By` trailer. Never push.**

**Goal:** drag a mask row to reorder the masks, and a component row to reorder its mask's shapes,
with the top component always an Add, one undo step per drop, and selection and the overlay eye
following the row.

**Architecture:** every decision a drop makes is a pure function in a new lib module
`mask_order.rs` over `edit_model` types and `panel_order`'s move rule; the bin's row widget senses
drag, and `masks::list` / `mask_components::rows` turn a release into one of those calls.

**Spec:** `docs/superpowers/specs/2026-09-29-mask-reorder-design.md`.

**Gate:** `cargo fmt --all --check` · `CARGO_TARGET_DIR=target/clippy cargo clippy --workspace
--all-targets -- -D warnings` · `cargo test --workspace` (filter by test **name**, never `-p`).

**Refinements to the spec, decided while planning:**

- Drop slots are computed from the rows' **midpoints**, not their tops: the upper half of a row
  means "before it", the lower half "after it". With the Library's past-the-top rule a 30-pt row
  dragged onto the centre of the row above lands where it started and nothing happens.
- `move_component` and `move_mask` mutate in place and update the selection themselves (returning
  whether anything changed), so the bin cannot apply a move and forget the selection.
- `panel_order` gains `move_in_place`, which `moved` calls — one rule, and no clone of every mask's
  stroke list on a mask drop.

---

## Task 1: `panel_order` — generic `moved`, and `move_in_place`

**Files:** Modify `crates/app/src/panel_order.rs`.

- [ ] **Step 1: Make the move rule generic and in-place**

Replace the `use` line and `moved` with:

```rust
/// The order after moving the item at `from` into position `to`.
///
/// ⚠ Both indices are clamped rather than trusted: `from` comes from a drag that may have started
/// before a re-sort, and `to` from a pointer position. An out-of-range index must not panic in a
/// paint loop.
pub fn moved<T: Clone>(order: &[T], from: usize, to: usize) -> Vec<T> {
    let mut out = order.to_vec();
    move_in_place(&mut out, from, to);
    out
}

/// [`moved`] without the copy — for lists whose items are expensive to clone (a mask's strokes).
///
/// ⚠ The one implementation of the rule; `moved` calls it, so the two cannot disagree.
pub fn move_in_place<T>(v: &mut Vec<T>, from: usize, to: usize) {
    if v.is_empty() || from >= v.len() {
        return;
    }
    let item = v.remove(from);
    // ⚠ `to` is an *insertion* index into the list **including** the item being moved, so removing
    // it first shifts every later slot down by one.
    let to = if to > from { to.saturating_sub(1) } else { to };
    v.insert(to.min(v.len()), item);
}
```

Move `use crate::settings::PanelSection;` into `mod tests` (it is now used only there). Change the
module doc's first line to *"Where a dragged row lands — a panel section, a mask, a mask's
component."*

- [ ] **Step 2: Run the existing tests** — `cargo test --workspace panel_order` → all pass
  (unchanged behaviour; the permutation property covers the extraction).

- [ ] **Step 3: Commit** — `refactor(app): panel_order's move rule is generic and has an in-place form`.

## Task 2: `mask_order` (lib, no egui)

**Files:** Create `crates/app/src/mask_order.rs`; modify `crates/app/src/lib.rs` (add
`pub mod mask_order;` after `pub mod mask_kind;`).

- [ ] **Step 1: Write the module with its tests**

```rust
//! Reordering masks and a mask's components: which drops are offered, what the lists become, and
//! where a selection lands.
//!
//! ⚠ No egui. The rows' drag lives in `develop/panel/{masks,mask_components}.rs`; everything a drop
//! *decides* is here, where it can be tested without a window — `panel_order`'s rule.

use edit_model::{Mask, MaskOp, MaskedAdjustment, Refinement};

use crate::panel_order::{move_in_place, moved};

/// Where the item at `i` ends up after [`moved`]`(_, from, to)` on a list of `len`.
///
/// ⚠ Read off `moved` itself rather than re-derived by arithmetic: two rules for one move is how a
/// selection comes to follow a different row than the one that moved. An `i` outside the list is
/// returned unchanged.
pub fn index_after_move(i: usize, from: usize, to: usize, len: usize) -> usize {
    let order = moved(&(0..len).collect::<Vec<_>>(), from, to);
    order.iter().position(|&k| k == i).unwrap_or(i)
}

/// A mask's components in fold order: `None` for the base, the op for each refinement.
fn ops(m: &MaskedAdjustment) -> Vec<Option<MaskOp>> {
    std::iter::once(None)
        .chain(m.refine.iter().map(|r| Some(r.op)))
        .collect()
}

/// Whether dropping component `from` into slot `to` is offered.
///
/// ⚠⚠ **The row that ends on top must add** (the author's rule, 2026-09-29). It becomes the base,
/// and the base is an Add onto nothing; a Subtract or Intersect there would fold onto zero coverage
/// and leave nothing. Refusing the drop keeps every shape meaning what it meant — the other choice
/// was to drop its op and silently turn a hole into the shape itself.
pub fn component_drop_allowed(m: &MaskedAdjustment, from: usize, to: usize) -> bool {
    let ops = ops(m);
    if from >= ops.len() {
        return false;
    }
    matches!(moved(&ops, from, to)[0], None | Some(MaskOp::Add))
}

/// Move component `from` into slot `to`, keeping `selected` on the shape it was on.
///
/// The top row's shape becomes `mask`; every other row is a [`Refinement`] keeping its op, the old
/// base as `Add` — exact, because the base *is* an Add onto nothing. `adjustments` and `disabled`
/// are untouched.
///
/// Returns whether anything changed: false for a refused drop, an out-of-range `from`, or a drop
/// where the row already was — so the caller records an undo step only for a real move.
pub fn move_component(
    m: &mut MaskedAdjustment,
    selected: &mut usize,
    from: usize,
    to: usize,
) -> bool {
    let n = 1 + m.refine.len();
    let idx: Vec<usize> = (0..n).collect();
    let order = moved(&idx, from, to);
    if from >= n || order == idx || !component_drop_allowed(m, from, to) {
        return false;
    }
    let base = std::mem::take(&mut m.mask);
    let refine = std::mem::take(&mut m.refine);
    let mut comps: Vec<Option<(Option<MaskOp>, Mask)>> = std::iter::once(Some((None, base)))
        .chain(refine.into_iter().map(|r| Some((Some(r.op), r.mask))))
        .collect();
    let mut taken = order
        .iter()
        .map(|&k| comps[k].take().expect("a permutation names each component once"));
    // ⚠ Its op is `None` (the old base) or `Add` — `component_drop_allowed` said so above.
    let (_, top) = taken.next().expect("a mask has at least its base");
    m.mask = top;
    m.refine = taken
        .map(|(op, mask)| Refinement {
            op: op.unwrap_or(MaskOp::Add),
            mask,
        })
        .collect();
    *selected = index_after_move(*selected, from, to, n);
    true
}

/// Move mask `from` into slot `to`, carrying its overlay flag and keeping `selected` on the mask it
/// was on. Returns whether anything changed.
///
/// ⚠⚠ `shown` is index-aligned with `masks` (`Session::mask_shown`) and moves by the same
/// permutation — or after a move every eye would belong to a different mask. It is padded to the
/// masks' length first, `Session::sync_mask_shown`'s rule, so a short list cannot misalign it.
pub fn move_mask(
    masks: &mut Vec<MaskedAdjustment>,
    shown: &mut Vec<bool>,
    selected: &mut Option<usize>,
    from: usize,
    to: usize,
) -> bool {
    let n = masks.len();
    let idx: Vec<usize> = (0..n).collect();
    if from >= n || moved(&idx, from, to) == idx {
        return false;
    }
    shown.resize(n, false);
    move_in_place(masks, from, to);
    move_in_place(shown, from, to);
    if let Some(s) = selected.as_mut() {
        *s = index_after_move(*s, from, to, n);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use edit_model::RadialGradient;

    /// A hard-edged disc at `(x, 0.5)` — the x is each shape's identity in these tests.
    fn disc(x: f32, r: f32) -> Mask {
        Mask::Radial(RadialGradient {
            center: [x, 0.5],
            radius: [r, r],
            feather: 0.0,
            invert: false,
        })
    }

    fn x_of(m: &Mask) -> f32 {
        match m {
            Mask::Radial(r) => r.center[0],
            other => panic!("expected a disc, got {other:?}"),
        }
    }

    fn composite(base: Mask, refine: &[(MaskOp, Mask)]) -> MaskedAdjustment {
        MaskedAdjustment {
            mask: base,
            refine: refine
                .iter()
                .map(|(op, mask)| Refinement {
                    op: *op,
                    mask: mask.clone(),
                })
                .collect(),
            ..Default::default()
        }
    }

    /// The fold `MaskStage` performs, over analytic shapes only.
    fn coverage(m: &MaskedAdjustment, u: f32, v: f32) -> f32 {
        let at = |s: &Mask| s.analytic_coverage(u, v, 1.0).expect("an analytic shape");
        m.refine
            .iter()
            .fold(at(&m.mask), |c, r| r.op.combine(c, at(&r.mask)))
    }

    fn max_difference(a: &MaskedAdjustment, b: &MaskedAdjustment) -> f32 {
        let mut worst = 0.0f32;
        for i in 0..=40 {
            for j in 0..=40 {
                let (u, v) = (i as f32 / 40.0, j as f32 / 40.0);
                worst = worst.max((coverage(a, u, v) - coverage(b, u, v)).abs());
            }
        }
        worst
    }

    /// Base A, minus B, plus C — B and C overlap, A does not reach them.
    fn a_minus_b_plus_c() -> MaskedAdjustment {
        composite(
            disc(0.25, 0.2),
            &[
                (MaskOp::Subtract, disc(0.5, 0.15)),
                (MaskOp::Add, disc(0.55, 0.15)),
            ],
        )
    }

    /// ⚠⚠ Proven to differ FIRST (standing lesson (c)): a feature that reorders a fold is worth
    /// nothing if the orders it reaches all look alike, and every assertion below would pass on a
    /// fold that ignored order.
    #[test]
    fn a_reorder_that_changes_the_fold_changes_the_coverage() {
        let before = a_minus_b_plus_c();
        let mut after = before.clone();
        let mut sel = 0;
        assert!(move_component(&mut after, &mut sel, 2, 1), "the move was refused");
        assert_eq!(after.refine[0].op, MaskOp::Add, "fixture: C now before B");
        let d = max_difference(&before, &after);
        assert!(d > 0.9, "(A − B) ∪ C and (A ∪ C) − B must differ, got {d}");
    }

    #[test]
    fn exchanging_the_base_with_an_add_keeps_the_coverage() {
        // [A, +C, −B] → [C, +A, −B]: the base is an Add onto nothing, so the exchange is exact.
        let before = composite(
            disc(0.25, 0.2),
            &[
                (MaskOp::Add, disc(0.55, 0.15)),
                (MaskOp::Subtract, disc(0.5, 0.15)),
            ],
        );
        let mut after = before.clone();
        let mut sel = 0;
        assert!(move_component(&mut after, &mut sel, 1, 0));
        assert_eq!(x_of(&after.mask), 0.55, "C is the base now");
        assert_eq!(after.refine[0].op, MaskOp::Add, "and A adds");
        assert_eq!(x_of(&after.refine[0].mask), 0.25);
        let d = max_difference(&before, &after);
        assert!(d < 1e-6, "the exchange changed the coverage by {d}");
    }

    /// Base A, then −B, +C, ∩D, +E: every op, and two Adds so more than one shape may go on top.
    fn mixed() -> MaskedAdjustment {
        composite(
            disc(0.1, 0.1),
            &[
                (MaskOp::Subtract, disc(0.2, 0.1)),
                (MaskOp::Add, disc(0.3, 0.1)),
                (MaskOp::Intersect, disc(0.4, 0.1)),
                (MaskOp::Add, disc(0.5, 0.1)),
            ],
        )
    }

    /// Each shape's op keyed by its identity; the base reads as `Add`, which is what it is.
    fn op_by_shape(m: &MaskedAdjustment) -> Vec<(u32, MaskOp)> {
        let mut out: Vec<(u32, MaskOp)> = std::iter::once((x_of(&m.mask), MaskOp::Add))
            .chain(m.refine.iter().map(|r| (x_of(&r.mask), r.op)))
            .map(|(x, op)| ((x * 100.0).round() as u32, op))
            .collect();
        out.sort_by_key(|(k, _)| *k);
        out
    }

    /// ⚠ Over EVERY `(from, to)`, because an off-by-one only shows at particular indices.
    #[test]
    fn every_move_keeps_every_shape_and_its_op_and_is_offered_only_when_the_top_adds() {
        let m = mixed();
        let n = 1 + m.refine.len();
        let ops = ops(&m);
        let mut offered = 0;
        let mut refused = 0;
        for from in 0..n {
            for to in 0..=n {
                let top = moved(&ops, from, to)[0];
                let want = matches!(top, None | Some(MaskOp::Add));
                assert_eq!(
                    component_drop_allowed(&m, from, to),
                    want,
                    "{from} → {to}"
                );
                let mut got = m.clone();
                let mut sel = 0;
                let changed = move_component(&mut got, &mut sel, from, to);
                if !want {
                    refused += 1;
                    assert!(!changed, "{from} → {to} was refused but changed the mask");
                    assert_eq!(got, m, "{from} → {to} was refused but changed the mask");
                    continue;
                }
                offered += 1;
                assert_eq!(got.refine.len(), m.refine.len(), "{from} → {to} lost a shape");
                assert_eq!(
                    op_by_shape(&got),
                    op_by_shape(&m),
                    "{from} → {to} changed an op"
                );
            }
        }
        // ⚠ Assert the sample (standing lesson (b)): a rule that refused everything, or nothing,
        // would pass the loop above.
        assert!(offered > 10 && refused > 5, "offered {offered}, refused {refused}");
    }

    #[test]
    fn a_subtract_or_intersect_is_never_moved_to_the_top() {
        let m = mixed();
        let mut sel = 0;
        for (from, to) in [(1, 0), (3, 0), (0, 2)] {
            // (0, 2) moves the base below −B, which would put −B on top.
            let mut got = m.clone();
            assert!(!move_component(&mut got, &mut sel, from, to), "{from} → {to}");
            assert_eq!(got, m);
        }
    }

    #[test]
    fn the_selected_shape_stays_selected() {
        // Selected: +C at 2.
        for (from, to, want) in [
            (2, 1, 1), // C itself moves up
            (1, 3, 1), // −B moves below C, so C shifts up
            (4, 1, 3), // +E moves above C, so C shifts down
            (4, 5, 2), // a move below C leaves it alone
        ] {
            let mut m = mixed();
            let mut sel = 2;
            move_component(&mut m, &mut sel, from, to);
            assert_eq!(sel, want, "{from} → {to}");
            let x = if sel == 0 {
                x_of(&m.mask)
            } else {
                x_of(&m.refine[sel - 1].mask)
            };
            assert_eq!(x, 0.3, "{from} → {to}: the selection left +C");
        }
    }

    #[test]
    fn a_drop_where_the_row_already_is_changes_nothing() {
        let m = mixed();
        for i in 0..=m.refine.len() {
            for to in [i, i + 1] {
                let mut got = m.clone();
                let mut sel = 0;
                assert!(!move_component(&mut got, &mut sel, i, to), "{i} → {to}");
                assert_eq!(got, m);
            }
        }
    }

    #[test]
    fn out_of_range_indices_change_nothing() {
        let m = mixed();
        let mut got = m.clone();
        let mut sel = 7;
        assert!(!move_component(&mut got, &mut sel, 99, 0));
        assert_eq!((got, sel), (m, 7));
        let mut masks = vec![mixed()];
        let mut shown = vec![true];
        let mut selected = Some(0);
        assert!(!move_mask(&mut masks, &mut shown, &mut selected, 5, 0));
        assert_eq!(selected, Some(0));
    }

    fn one_shape(x: f32) -> MaskedAdjustment {
        composite(disc(x, 0.1), &[])
    }

    #[test]
    fn a_mask_moves_with_its_eye_and_the_selection_follows() {
        let mut masks = vec![one_shape(0.1), one_shape(0.2), one_shape(0.3)];
        let mut shown = vec![true, false, false];
        let mut selected = Some(0);
        assert!(move_mask(&mut masks, &mut shown, &mut selected, 0, 3));
        let xs: Vec<f32> = masks.iter().map(|m| x_of(&m.mask)).collect();
        assert_eq!(xs, [0.2, 0.3, 0.1]);
        assert_eq!(shown, [false, false, true], "the eye stayed behind");
        assert_eq!(selected, Some(2), "the selection stayed behind");
    }

    #[test]
    fn another_mask_moving_across_the_selection_shifts_it() {
        let mut masks = vec![one_shape(0.1), one_shape(0.2), one_shape(0.3)];
        let mut shown = vec![false; 3];
        let mut selected = Some(1);
        assert!(move_mask(&mut masks, &mut shown, &mut selected, 2, 0));
        assert_eq!(selected, Some(2));
        assert_eq!(x_of(&masks[2].mask), 0.2);
    }

    #[test]
    fn a_short_eye_list_is_padded_before_it_moves() {
        let mut masks = vec![one_shape(0.1), one_shape(0.2)];
        let mut shown = vec![true];
        let mut selected = None;
        assert!(move_mask(&mut masks, &mut shown, &mut selected, 0, 2));
        assert_eq!(shown, [false, true]);
        assert_eq!(selected, None);
    }

    #[test]
    fn a_mask_dropped_where_it_is_changes_nothing() {
        let mut masks = vec![one_shape(0.1), one_shape(0.2)];
        let mut shown = vec![true, false];
        let mut selected = Some(0);
        assert!(!move_mask(&mut masks, &mut shown, &mut selected, 0, 1));
        assert_eq!(shown, [true, false]);
    }
}
```

- [ ] **Step 2: Run** — `cargo test --workspace mask_order` → all pass. (A pure module written with
  its tests; the red half is the sabotage step below.)

- [ ] **Step 3: Sabotage** — each must turn at least one test red, then revert:
  1. `component_drop_allowed` → `return true` at the top: `every_move…` and
     `a_subtract_or_intersect…` fail.
  2. drop `*selected = index_after_move(…)` in `move_component`: `the_selected_shape_stays_selected`.
  3. drop `move_in_place(shown, …)` in `move_mask`: `a_mask_moves_with_its_eye…`.

- [ ] **Step 4: Commit** — `feat(app): mask_order — which component drops are offered (the top row must add), the moves, and where a selection lands`.

## Task 3: the row widget senses drag

**Files:** Modify `crates/app/src/develop/panel/mask_components.rs`,
`develop/panel/{retouch.rs, retouch_eyes.rs, presets.rs}` (the new field, `None`).

- [ ] **Step 1: Hover constants, `RowSpec::reorder`, `RowOut`'s new fields, the drop helpers**

Beside `ROW_HEIGHT`:

```rust
/// A mask row's hover.
pub const MASK_ROW_HOVER: &str =
    "Selects this mask, and a drag moves it to change the order the masks apply in";
/// A mask's component row's hover.
pub const COMPONENT_ROW_HOVER: &str =
    "Selects this shape, and a drag moves it to change the order the shapes combine in";
```

In `RowSpec`, after `hold`:

```rust
    /// `Some(hover)` makes the row draggable to reorder and says so on hover; `None` keeps it
    /// click-only (patches, eyes, presets).
    pub reorder: Option<&'a str>,
```

In `RowOut`:

```rust
    /// The row's rect, for the drop slots.
    pub rect: egui::Rect,
    /// Being dragged this frame.
    pub dragging: bool,
    /// A drag of this row ended this frame.
    pub released: bool,
```

In `row`: sense `click_and_drag()` when `spec.reorder.is_some()`, else `click()`; after
`widget_info`, `let resp = match spec.reorder { Some(h) => resp.on_hover_text(h), None => resp };`;
return `rect`, `dragging: resp.dragged()`, `released: resp.drag_stopped()`.

After `row`:

```rust
/// The slot a row dragged to the pointer would land in, and the y of its insertion line.
///
/// ⚠ The boundaries are the rows' **midpoints**, not their tops: the upper half of a row is
/// "before it", the lower half "after it". The Library's past-the-top rule suits its tall section
/// headers; on a 30-pt row it made a drop on the row above land where the row started.
pub fn drop_slot(ui: &egui::Ui, rects: &[egui::Rect]) -> Option<(usize, f32)> {
    let y = ui.ctx().pointer_interact_pos()?.y;
    let mids: Vec<f32> = rects.iter().map(|r| r.center().y).collect();
    let at = rustroom_app::panel_order::drop_index(&mids, y);
    // ⚠ Past the last row the line sits at the cursor — below anything drawn under that row (a
    // selected mask's components), which is where such a drop lands.
    let line = rects.get(at).map_or(ui.cursor().top(), |r| r.top());
    Some((at, line))
}

/// The insertion line: the Library rail's, in the accent.
pub fn paint_drop_line(ui: &egui::Ui, x: egui::Rangef, y: f32) {
    ui.painter()
        .hline(x, y, egui::Stroke::new(2.0, ui.visuals().selection.bg_fill));
}
```

- [ ] **Step 2: `rows` — the component drag**

In `rows`, pass `reorder: Some(COMPONENT_ROW_HOVER)`, collect `rects` and
`drag: Option<(usize, bool)>` (`(j, out.released)` when `out.dragging || out.released`), and after
the `remove` block:

```rust
    if let (Some((from, released)), None, None) = (drag, pick, remove) {
        if let Some((to, line)) = drop_slot(ui, &rects) {
            // ⚠ A refused slot draws no line, and a release there does nothing — the only signal,
            // and enough: the line is what says where the row would go.
            let offered = s
                .edits
                .masks
                .get(i)
                .is_some_and(|m| mask_order::component_drop_allowed(m, from, to));
            if offered {
                if released {
                    let mut sel = s.component_index();
                    if mask_order::move_component(&mut s.edits.masks[i], &mut sel, from, to) {
                        s.selected_component = sel;
                        s.active_mask_handle = None;
                        ed.discrete();
                    }
                } else {
                    let r = ui.max_rect();
                    paint_drop_line(ui, egui::Rangef::new(r.left() + INDENT, r.right()), line);
                }
            }
        }
    }
```

`pick` and `remove` are consumed by the `if let Some(..)` blocks above it, so copy them out
(`let (picked, removed) = (pick, remove);` — both are `Option<usize>`, `Copy`) before those blocks
and match on the copies. Import `rustroom_app::mask_order`.

- [ ] **Step 3: The other callers** — add `reorder: None,` to the `RowSpec` in `retouch.rs` (×2),
  `retouch_eyes.rs` and `presets.rs`.

- [ ] **Step 4: Build** — `cargo test --workspace mask_components` → existing tests pass.

## Task 4: `masks::list` — the mask drag

**Files:** Modify `crates/app/src/develop/panel/masks.rs`.

- [ ] **Step 1: The drag** — in `list`, pass `reorder: Some(mask_components::MASK_ROW_HOVER)`,
collect `rects` and `drag` exactly as in `rows`, and after the `remove` block:

```rust
    if let (Some((from, released)), None, None) = (drag, pick, remove) {
        if let Some((to, line)) = mask_components::drop_slot(ui, &rects) {
            if released {
                if mask_order::move_mask(
                    &mut s.edits.masks,
                    &mut s.mask_shown,
                    &mut s.selected_mask,
                    from,
                    to,
                ) {
                    s.active_mask_handle = None;
                    ed.discrete();
                }
            } else {
                mask_components::paint_drop_line(ui, ui.max_rect().x_range(), line);
            }
        }
    }
```

(`pick` and `remove` are consumed by `if let Some(..)` above — bind them with `.is_none()` into
locals before those blocks.)

- [ ] **Step 2: The harness helper** — in `mod tests`'s `impl Panel`:

```rust
        /// Press on the row labelled `from` and release on the upper (`after == false`) or lower
        /// half of the row labelled `to` — "before it" and "after it".
        fn drag_row(&mut self, from: &str, to: &str, after: bool) -> Edited {
            let ctx = self.ctx.clone();
            let centres = test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
                self.body(ui, &mut Edited::default())
            });
            let at = |label: &str| {
                centres
                    .iter()
                    .find(|(l, _)| l == label)
                    .map(|(_, p)| *p)
                    .unwrap_or_else(|| panic!("no row labelled {label:?}: {centres:?}"))
            };
            let start = at(from);
            let quarter = mask_components::ROW_HEIGHT / 4.0;
            let end = at(to) + egui::vec2(0.0, if after { quarter } else { -quarter });
            self.pointer_path(start, end)
        }
```

and factor the event loop of the existing `drag` into `fn pointer_path(&mut self, from, to) -> Edited`
(the five frames: move, press, move, release, idle), which `drag` then calls with
`from + egui::vec2(120.0, 0.0)`.

- [ ] **Step 3: The panel tests** (in `masks.rs`'s `mod tests`):

```rust
    /// Gradient, then −Radial, then +Radial — the last selected.
    fn composite_panel() -> Panel {
        let mut p = Panel::new();
        p.add(MaskKind::Gradient);
        p.refine(edit_model::MaskOp::Subtract, MaskKind::Radial);
        p.refine(edit_model::MaskOp::Add, MaskKind::Radial);
        assert_eq!(p.s.selected_component, 2, "fixture: +Radial is being edited");
        p
    }

    #[test]
    fn dragging_a_component_reorders_the_composite_as_one_edit() {
        let mut p = composite_panel();
        let ed = p.drag_row("Add Radial", "Subtract Radial", false);
        let m = &p.s.edits.masks[0];
        assert_eq!(m.refine[0].op, edit_model::MaskOp::Add, "+Radial moved up");
        assert_eq!(m.refine[1].op, edit_model::MaskOp::Subtract);
        assert_eq!(p.s.selected_component, 1, "the edited shape followed its row");
        assert!(ed.changed && ed.settled, "one drop, one history entry");
    }

    /// ⚠ Slot 0 is reached through the MASK row's lower half: "Gradient" names both the base
    /// component row and the kind button, so the harness cannot aim at it by name.
    #[test]
    fn a_subtract_is_never_dropped_on_top() {
        let mut p = composite_panel();
        let before = p.s.edits.masks.clone();
        let ed = p.drag_row("Subtract Radial", "Gradient 1", true);
        assert_eq!(p.s.edits.masks, before, "a Subtract reached the top");
        assert!(!ed.settled, "a refused drop is no history entry");
    }

    #[test]
    fn an_add_dropped_on_top_becomes_the_base() {
        let mut p = composite_panel();
        let ed = p.drag_row("Add Radial", "Gradient 1", true);
        let m = &p.s.edits.masks[0];
        assert!(matches!(m.mask, edit_model::Mask::Radial(_)), "the Add is the base");
        assert!(matches!(m.refine[0].mask, edit_model::Mask::Linear(_)));
        assert_eq!(m.refine[0].op, edit_model::MaskOp::Add, "the old base adds");
        assert_eq!(m.refine[1].op, edit_model::MaskOp::Subtract);
        assert_eq!(p.s.selected_component, 0);
        assert!(ed.settled);
    }

    #[test]
    fn dragging_a_mask_reorders_the_masks_and_their_eyes() {
        let mut p = Panel::new();
        p.add(MaskKind::Gradient);
        p.add(MaskKind::Radial);
        p.s.mask_shown = vec![true, false];
        assert_eq!(p.s.selected_mask, Some(1), "fixture: Radial 2 selected");
        let ed = p.drag_row("Radial 2", "Gradient 1", false);
        assert!(matches!(p.s.edits.masks[0].mask, edit_model::Mask::Radial(_)));
        assert!(matches!(p.s.edits.masks[1].mask, edit_model::Mask::Linear(_)));
        assert_eq!(p.s.mask_shown, [false, true], "the eyes stayed behind");
        assert_eq!(p.s.selected_mask, Some(0), "the selection stayed behind");
        assert!(ed.changed && ed.settled);
        let labels = p.labels();
        assert!(labels.iter().any(|l| l == "Radial 1"), "{labels:?}");
    }

    #[test]
    fn a_mask_dropped_where_it_started_is_no_edit() {
        let mut p = Panel::new();
        p.add(MaskKind::Gradient);
        p.add(MaskKind::Radial);
        let before = p.s.edits.masks.clone();
        let ed = p.drag_row("Gradient 1", "Radial 2", false);
        assert_eq!(p.s.edits.masks, before);
        assert!(!ed.settled);
    }
```

- [ ] **Step 4: Run** — `cargo test --workspace masks` → pass.

- [ ] **Step 5: Sabotage**, each red then reverted:
  1. in `rows`, drop the `component_drop_allowed` guard → still refused by `move_component` itself
     (belt and braces; expect GREEN — record it), then also make `component_drop_allowed` return
     `true` → `a_subtract_is_never_dropped_on_top` red.
  2. in `list`, pass `&mut Vec::new()` instead of `&mut s.mask_shown` →
     `dragging_a_mask_reorders_the_masks_and_their_eyes` red.
  3. `MASK_ROW_HOVER` ending in a full stop → `every_hover_is_one_short_sentence` red (proves the
     sweep reads the new constants).

- [ ] **Step 6: Commit** — `feat(app): drag a mask or a mask's component to reorder — an accent line marks the slot, a Subtract or Intersect is never offered the top, one undo step per drop, selection and the eye follow (sabotage-checked)`.

## Task 5: gate, docs, GUI

- [ ] **Step 1: Gate** — fmt, clippy (own target dir), `cargo test --workspace`; each exit code read
  from its own command. Measure code lines of `masks.rs` and `mask_components.rs`
  (`awk '/#\[cfg\(test\)\]/{exit} {n++} END{print n}'`); report either over ~600.
- [ ] **Step 2: Docs** — `docs/JOURNAL.md` entry (newest first, plus the Contents line);
  `docs/BACKLOG.md`: the reorder entry and its table row → ✅ with a pointer; `docs/DESIGN.md` §9:
  *"Reorder: drag the row; a 2-pt accent line marks the slot, midpoints decide before/after"*;
  `CLAUDE.md`: test count, `lib.rs` module count 73 → 74, *Next action* rewritten to the agreed order
  (open items chronologically, Phase 10 last); Obsidian `03-plan-of-action.md` if it names the item.
  Grep every changed doc for `Co-Authored-By` before committing.
- [ ] **Step 3: Commit** — `docs: reordering masks and components built — journal, backlog, design §9, state`.
- [ ] **Step 4: GUI** — `cargo run --release -p app` in the background with `PKG_CONFIG_PATH` set;
  tell the author what to try.
