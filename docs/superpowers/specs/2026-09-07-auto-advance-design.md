# Auto-advance after rating — design

Date: 2026-09-07
Status: **designed, not built.**

**M6.5c**, the last item of **M6.5 — Culling throughput**. With **M6.2** it closes **Phase 6**.

## What it is

Rating a photo moves to the next one, so a culling pass is one keystroke per frame instead of two.
The roadmap's "Done when" for M6.5 reads: *"two frames sit side by side at matched zoom, and rating
one advances to the next."* The first half shipped as M6.5a; this is the second.

## What already exists, and why this is smaller than it looks

⚠⚠ **Two of the four views already answer this in their own idiom, and one of those answers is a
decision that was made, reversed, and must not be quietly re-reversed.**

- **Survey is subtractive**: `P` and `X` drop the tile off the wall via `drop_from_survey`. ⚠ A
  *rating* does not, and neither does `U` — *"un-flagging undoes a decision, so a photo made
  undecided again belongs back on the wall."* **Survey is untouched by this milestone.**
- **Compare marks and nothing else** — and `compare.rs` records why: it shipped for one round with
  `P` also removing the photo, and that was reversed on 2026-09-02 because *"the subtractive act
  belongs to Survey; Compare holds two frames still and lets you decide between them."*

⚠ **Advancing is NOT the removal that was reversed**, and the distinction is the whole reason
Compare is in scope here. Removal takes a frame *out of the comparison*; advancing steps to the
next frame *to compare against*. The reversed behaviour destroyed the thing Compare exists for; this
one is what the arrow keys already do.

## The design

### 1. In Compare, auto-advance is exactly the arrow-key step on the bordered pane

⚠⚠ **That is the whole rule, and it needs no new machinery.** `compare.rs` already branches on
`focus` for `ArrowRight`/`ArrowLeft`: `focus == 0` steps the **reference** through
`compare_pair::step` with the roles swapped, otherwise it steps the **candidate**. The mark path
picks its `target` from the *same* `focus`. So after emitting a mark, run the step the arrow would
have run.

**Rating the reference therefore advances the reference** (user's call). ⚠ The alternative —
advancing the candidate whichever pane was rated — moves a pane the user did not act on, and
"don't advance at all on the reference" would make the two panes behave differently for no reason
the border explains. **The border already means "this is the pane the keys act on"**; auto-advance
inherits that meaning rather than adding a second one.

⚠ Stepping the reference uses `Selection::focus`, never `click` — `click` replaces the whole
selection, which would collapse "compare exactly these" to one photo and silently widen the pool to
the entire folder. That rule is already in the arrow path and is inherited with it.

### 2. In the grid and the filmstrip, advance PAST the whole selection

`apply_mark` deliberately acts on the **selection**, not the active photo — *"select a run of frames
and press X"*. So advancing one step from the active photo would leave you **inside the run you just
rated**.

Instead: take the largest selected index that is visible, and step one past it.

⚠ **`lib.visible` is sorted ascending, so the largest selected visible index *is* the last one in
visible order.** No separate ordering pass is needed, and none should be added — a second notion of
"last" is how the two would drift.

Then `grid_nav::step_by(&lib.visible, last, 1)` and `selection.click(next)`, plus `centre_on` so the
view follows. ⚠ Both are what the arrow keys already do, and `grid_nav::step_by` is asserted to
agree with `format_filter::step` at every index in both directions.

⚠ `step_by` **clamps** rather than wrapping, so rating the last photo in the list leaves you on it.
Deliberate: wrapping to the top of a folder mid-cull is disorienting, and the end of the list is
information.

### 3. `MarkAction::advances()` lives beside the type

```rust
impl MarkAction {
    /// Whether applying this mark should move on to the next photo.
    pub fn advances(self) -> bool;
}
```

`Rating(0) => false`, `Flag(Flag::None) => false`, everything else `true`.

⚠⚠ **Beside `MarkAction`, never in a view** — the rule `EDIT_GROUP_NAMES`, `Tool::ALL`, `FlagSet::ALL`
and `DragQuality` already follow. The grid, the filmstrip and Compare all consult it, and **a rule
written in three places is one that will disagree in two of them.**

⚠ **The exception is corrections, and it mirrors reasoning already recorded for Survey.** `0` clears
a rating and `U` clears a flag; both *undo* a decision rather than making one, and advancing after a
correction moves you off the very photo you were fixing. Survey's note says the same thing about the
same key: *"un-flagging undoes a decision, so a photo made undecided again belongs back on the
wall."* ⚠ This is a **deliberate divergence from Lightroom**, which advances on every rating
including 0.

### 4. `Settings.auto_advance: bool`, default `true`

⚠ Default **on**: it is the milestone's whole point, and a culling feature nobody finds is a feature
that did not ship. The correction case that usually argues for defaulting off is largely removed by
§3 — `0` and `U` do not advance, so fixing a mis-rating does not run away from you.

⚠ A new field under `Settings`' **container-level** `#[serde(default)]`, so it reaches every existing
settings file with no migration. ⚠ No **field-level** `#[serde(default)]`: that would use `bool`'s
own default of `false` and silently ship the feature disabled for everyone — the trap
`snapshot_strip_height` and `write_xmp` both document.

⚠ The exhaustive `Settings { .. }` literal in `settings_round_trip_carries_the_export_options` will
force the field at compile time. That is what it is kept exhaustive for, and it fired for
`drag_quality` earlier today.

### 5. Survey is untouched

It is already subtractive, on a decision recorded in `CLAUDE.md`. Adding auto-advance on top would
mean a rated tile both stays on the wall *and* moves the focus, which is two behaviours for one act.

## Testing

| test | asserts |
|---|---|
| `only_a_decision_advances` | over every `MarkAction` — ratings 1–5, labels, Pick, Reject advance; `Rating(0)` and `Flag::None` do not |
| `a_settings_file_from_before_auto_advance_still_loads` | the forgiving load; sabotage-verified by removing the container-level `#[serde(default)]` |
| `the_grid_advances_past_the_whole_selection` | with a run selected, the next active photo is the first *after the last* selected one, not one step from the active |
| `the_grid_does_not_advance_off_the_end` | rating the last visible photo leaves the selection where it is |
| `a_correction_does_not_advance` | pressing `0` or `U` leaves the selection alone even with the setting on |
| `auto_advance_off_leaves_the_selection_alone` | the setting genuinely gates it |
| `compare_advances_the_bordered_pane` | rating the candidate steps the candidate; rating the reference steps the reference |
| the 59 goldens | byte-identical — no pixel path is touched |

⚠ **What no test here can cover:** whether the rhythm of a real culling pass feels right, which is a
judgement about using it. Checked by running the GUI over a folder.

## Accepted caveats

1. **Rating the last photo in the list does not move** — the clamp, deliberately, over wrapping.
2. **Advancing past a multi-selection can jump a long way** — rate forty selected frames and you
   land after all forty. That is the stated rule, and the alternative leaves you inside the run.
3. **Compare's reference advancing changes the pool the candidate steps through**, since the pool is
   "everything except the reference". Inherent to stepping the reference at all, which the arrow
   keys already do.
4. **A deliberate divergence from Lightroom** on `0` and `U`.
5. **Survey keeps its own idiom**, so the app has two culling behaviours — subtractive there,
   advancing elsewhere. Justified by what each view is *for*, and recorded here so it is not later
   filed as an inconsistency.
