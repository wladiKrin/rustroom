# A one-format folder shows its one format

**Status:** spec · 2026-09-17
**Found by:** using it. *"In library mode I turned off the view for jpg files. Then I opened a
folder which only had jpegs, and it didn't show anything, and there was also no option to turn on
the view for jpegs."*

## The defect

`FormatFilter` is **global and persisted** (`Settings::hidden_formats`), but the chips that control
it are drawn only when the folder has **two or more** formats — `shell.rs::format_bullets`:

```rust
if formats.len() < 2 {
    return;
}
```

So hiding `jpg` and then opening a JPEG-only folder yields **an empty grid and no control to undo
it.** The folder is unrecoverable from its own screen; the only exits are opening a different
folder or editing `settings.json`.

⚠⚠ **And the comment immediately below that early return argues the case the return defeats:**

> *"Every box stays enabled, including the last ticked one. … Unticking everything is recoverable in
> one click **because the boxes are still there**."*

They are not there in the `< 2` case. **Two individually reasonable decisions — a global filter and
a chip row that hides itself when pointless — produce an unrecoverable state where they meet.**

## The behaviour, the user's call

**A folder whose entries have fewer than two formats always shows them all.** The stored hidden-set
is **overridden for that folder**, not edited. Returning to a mixed folder uses the remembered
setting again, unchanged.

No toggle is offered in such a folder — there is nothing to choose between.

## Goal

**One predicate, consulted by both the filtering and the UI, so they cannot disagree.**

Today two places encode "this folder has fewer than two formats": the early return above, and
(after this change) the filtering. If they ever drift, the bug returns in the opposite direction —
a chip that does nothing, or filtering with no chip. ⚠ This is the *guard that enumerates a list*
lesson in a second form: **a rule written twice is a rule that can disagree with itself.**

## Non-goals

- **The stored set is never edited.** Overriding by *reading* keeps "hidden-by-exception" true;
  editing it would silently unhide the format everywhere, which nobody asked for.
- **No new UI.** The user asked for no toggle in a one-format folder, and `format_bullets` already
  draws none. It changes only *where its condition comes from*.
- Not the rating / flag / label / camera / lens filters. A one-format folder can still be filtered
  to empty by those, and that is recoverable because their controls are always present.

## Design

```
format_filter::folder_overrides_hidden(entries) -> bool     // the ONE predicate
LibraryFilter::effective_for(&self, entries)    -> LibraryFilter
```

`effective_for` clones `self` and clears `format` when the predicate holds.

**`LibraryState::refilter` applies it.** That is the single choke point: `refilter` has
`self.entries` in hand and **12 call sites** across `shell`, `library`, `library_state`, `sources`,
`multi::compare` and `test_app`. Putting the override at the call sites would be twelve chances to
forget it; putting it inside `refilter` is one place that cannot be bypassed — the same argument its
own comment already makes for `visible` being one derived list.

**`format_bullets` early-returns on the same predicate** instead of its own `formats.len() < 2`.

⚠ **Placement.** `format_filter` and `library_filter` are **lib** modules — egui-free and unit
testable, which is where this logic belongs by this repo's own rule. `library_state` and `shell` are
**bin** modules and get a one-line change each.

## ⚠ A premise that turned out to be false, recorded so it is not re-derived

Before writing this, the question *"should the **Filter** button still report an active filter while
the format half is overridden?"* was put to the user, who chose **no — report inactive while nothing
is hidden.**

**That symptom does not currently manifest.** `mark_filter_label` builds its label from stars,
flags, cameras, labels and lenses and **never reads the format filter**; and
`LibraryFilter::is_active()` has **no production caller at all** — every reference outside its own
definition is a test.

The chosen semantics are still right, and they come **for free**: anything asking
`effective_for(entries).is_active()` gets `false` when the override applies, because `format` was
cleared. No separate code, and correct for whatever becomes its first caller. ⚠ **Nothing in this
milestone may be reported as fixing a visible Filter-button symptom.**

## Done when

1. `folder_overrides_hidden` exists and is the only place the rule is written.
2. `effective_for` clears the format half when it holds, and leaves the stored set untouched.
3. `refilter` applies it, so all 12 call sites are covered without changing any of them.
4. `format_bullets` early-returns on the same predicate.
5. Tests, in the lib modules: a JPEG-only folder with `jpg` hidden shows **every** entry; the stored
   set is **unchanged** afterwards; a mixed folder still hides `jpg`; `effective_for(...).is_active()`
   is false when overridden and true when not.
6. **A sabotage step:** making `folder_overrides_hidden` return `false` unconditionally must make
   those tests FAIL. A test that passes with the override disabled is testing nothing.
7. `fmt` / `clippy --all-targets --all-features -D warnings` / `test --workspace` green together,
   **each exit code read from its own command**, and **74 goldens ran**.
8. ⚠ Confirmed **by use**: hide JPG in a mixed folder, open a JPEG-only folder and see the photos
   with no chips; go back to the mixed folder and see JPG still hidden.

## Risk

Low, and the shape is bounded: the override only ever *widens* what is shown. The failure mode
worth naming is the opposite one — **a mixed folder forgetting the setting**, which item 5's
third test pins.
