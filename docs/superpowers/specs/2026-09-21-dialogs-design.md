# The dialogs in one language: title bar, tabs, pinned footer — design

**Date:** 2026-09-21. **Branch:** `dialogs-tabs`. **Status:** option B of the canvas "Rustroom
Dialogs" (https://claude.ai/artifact/Gmvn3eqBNgtWopBRG9vJCK), chosen in conversation. **Written to
be executed by a different agent**: everything it needs is in this file and the code it names.

## Read first

- `CLAUDE.md` — the conventions. The ones that bite here: **no `Co-Authored-By` trailer**; **never
  push**; every cargo command needs
  `export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"`;
  the gate is `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace`, **each exit code read from its own command, never two gates at once**
  (seven temp-dir test sites collide); every fresh test binary stalls ~22 s on this Mac, so a gate
  is 15–30 min — run it in the background with logs; launch the GUI after the work with
  `cargo run --release -p app` in the background; **egui lives only in the bin modules**; a file
  past ~600 lines is reported, not silently grown.
- `crates/app/src/modals/mod.rs` — `dialog(ctx, settings, id, title, DialogSize, content) -> bool`
  is the one shell every dialog goes through (the `bool` is "the backdrop was clicked"), and
  `draw` dispatches the fifteen. `crates/app/src/theme.rs` — `DialogSize`, `dialog_frame`,
  `button_row`. `crates/app/src/develop/panel/section.rs` — `header(ui, id, default_open,
  Header::new(name).trailing(..))` is the `› NAME` section chrome the panels use.
  `crates/app/src/segmented.rs` — `theme::segmented(ui, &mut current, &[(T, &str)])` is the
  full-width segmented control; `pill` the content-sized one. `crates/app/src/shell.rs`
  `export_button` paints the app's one accent-filled button by hand.
- `crates/app/src/test_ui.rs` — the AccessKit harness (`ctx`, `labelled_centres`, `find`,
  `click`); the modal tests in `modals/mod.rs` and `modals/export.rs` show how a dialog is
  driven headless (`test_app::for_test()`, then `modals::draw(&mut app, &ctx)` inside
  `labelled_centres`).

## Why

The fifteen dialogs share one shell, but the shell is a 17 pt heading, a hairline and a scroll
area. Settings is thirteen groups separated by nameless hairlines. Buttons sit at the bottom of
the scrolling content, left-aligned, confirm first, both plain — "Forget" reads the same as
"Cancel", and in a long dialog the buttons scroll out of view. Info uses bold labels for its
sections, Export collapsing headers, Shortcuts bold titles: three idioms for one idea.

## The shell

Every dialog is drawn by `modals::shell::dialog` (new file `crates/app/src/modals/shell.rs`,
because `modals/mod.rs` is 572 lines and `modals/export.rs` 696 — the shell, the footer and the
tabs move out of `mod.rs` into it, and `mod.rs` re-exports `dialog` so the dispatch reads as
before). Still `egui::Modal` + `egui::Resize` with the remembered size (`Settings::dialog_size`
/ `remember_dialog_size`, unchanged), three parts top to bottom:

1. **Title bar**, 36 pt: the title in the body face and the strong text colour at the left, a
   `×` at the right (accessible name "Close"; a click = dismissed, exactly like the backdrop). A
   hairline under it in `widgets.noninteractive.bg_stroke`. No more `ui.heading`.
2. **Body**: the caller's content in a vertical `ScrollArea` with `auto_shrink([false, false])`,
   inside the dialog's margin. Where the spec below says *tabs*, the body starts with a
   full-width `theme::segmented` over the tab names and shows only the current tab's content.
3. **Footer**, pinned under the scroll, never scrolling: a hairline above, then a row with an
   optional weak **note** at the left and the buttons right-aligned. Rightmost is the
   **primary** verb, accent-filled with white text; left of it the **dismiss** button, plain;
   left of that any **extra** buttons, plain, in the order given (the collision dialog's Skip /
   Overwrite / Keep both). A dialog with no primary (Settings, Info, Shortcuts, About, and the
   unsupported-rule case) has just a plain dismiss, and its label is "Done" or "Close" as the
   table says.

The shell's signature becomes:

```rust
pub struct Footer<'a> {
    /// The accent-filled verb, and whether it is enabled. `None` for an informational dialog.
    pub primary: Option<(&'a str, bool)>,
    /// "Cancel", "Close" or "Done" — what the plain button says.
    pub dismiss: &'a str,
    /// Plain buttons left of dismiss, in this order. Empty for every dialog but one.
    pub extra: &'a [&'a str],
    /// Weak text at the footer's left. Empty for none.
    pub note: &'a str,
}

pub struct DialogOut {
    /// The backdrop, the × or the dismiss button.
    pub dismissed: bool,
    /// The primary was clicked (never when disabled).
    pub primary: bool,
    /// Which extra button was clicked, as an index into `extra`.
    pub extra: Option<usize>,
}

pub fn dialog(
    ctx: &egui::Context,
    settings: &mut Settings,
    id: &str,
    title: &str,
    size: theme::DialogSize,
    footer: Footer<'_>,
    content: impl FnOnce(&mut egui::Ui),
) -> DialogOut
```

`theme::button_row` is deleted; its two remaining semantics (confirm, dismiss) are the footer's.
`theme::primary_button(ui, text, enabled) -> Response` is new — the accent fill with white text
that `shell.rs::export_button` paints today — and `export_button` calls it, so the bar and the
dialogs cannot drift apart. A disabled primary is drawn through `add_enabled` (greyed), never
hidden: the Save button with an empty name must still be seen.

**Tabs**: `shell::tabs(ui, id: egui::Id, names: &[&str]) -> usize` draws the segmented control
and returns the current index. The index lives in egui's temporary memory under `id`
(`ui.data_mut(|d| d.get_temp_mut_or_default::<usize>(id))`) — per dialog, per process; opening
the dialog again in the same session lands on the last tab, a restart on the first. Not in
`Settings`: a remembered tab is a convenience, and the settings file is for decisions.

**Sections** inside a tab or a body use `section::header` from the Develop panel, with
`show_body_unindented`, exactly as the panels do. Collapsible; default open. Their ids are
salted with the dialog id: `egui::Id::new((id, "Metadata"))`.

## Every dialog

Ids, titles and sizes are as today unless a cell says otherwise. "Groups" means the
`edit_group_checkboxes` block from `crate::transfer`.

| id | title | size | body | footer |
|---|---|---|---|---|
| `settings` | Settings | Standard | **Tabs** Behaviour · Appearance · Display · Performance · Input (contents below) | dismiss "Done"; note "Changes apply as you make them." |
| `export_settings` | Export | Standard | **Tabs** Output · Filename · Metadata · Watermark. Output: the destination, the format, colour space, size and quality, demosaic; Filename: the template field and its preview; Metadata: the Metadata group (copyright, creator, caption, include); Watermark: the watermark group. The current `CollapsingHeader`s go; their `(on)` suffix becomes the tab's hover text ("Metadata is on") | primary "Export" (`can_export`); dismiss "Cancel"; note = the job count the dialog already computes ("3 photos to export") |
| `photo_info` | the photo's caption (as today) | Wide | **Tabs** File · Image · Capture · Description · Marks — one per current bold section; the two-column grids stay | dismiss "Close" |
| `shortcuts` | Keyboard shortcuts | Wide | one `section::header` per menu section (the model's `section.title`), the rows under it as today | dismiss "Close" |
| `edit_rule` | Rule for “name” | Wide | as today; the unsupported case shows its hint and nothing else | primary "Save" (enabled unless unsupported); dismiss "Cancel" |
| `save_preset` | Save preset | Standard | Name field on its own row, then a section header **Include** with trailing text = `presets::groups_summary(groups)`, then the groups | primary "Save" (name not blank); dismiss "Cancel"; note "Saved presets appear in the Presets panel." |
| `save_export_preset` | Save export preset | Standard | the name field | primary "Save" (name not blank); dismiss "Cancel" |
| `paste_settings` | Paste settings | Standard | section header **Apply**, the groups | primary "Paste"; dismiss "Cancel" |
| `sync_settings` | Sync settings | Standard | section header **Apply** with trailing text "to n other selected photos", the groups | primary "Sync"; dismiss "Cancel" |
| `confirm_delete` | Delete preset? | Compact | the sentence, then the "Don't ask again" checkbox | primary "Delete"; dismiss "Cancel" |
| `confirm_delete_export` | Delete export preset? | Compact | same shape | primary "Delete"; dismiss "Cancel" |
| `confirm_forget_folder` | Forget folder? | Compact | "Forget “name” and its n photos?" in body text; the consequence ("Their ratings, labels and keywords go with it. The files stay on disk.") as a hint under it | primary "Forget"; dismiss "Cancel" |
| `confirm_delete_keyword` | Delete keyword? | Compact | the sentence and its counts as today, consequence as a hint | primary "Delete"; dismiss "Cancel" |
| `export_collision` | File already exists | Standard | the sentence, the "Apply to all remaining" checkbox | primary "Keep both"; extra ["Skip", "Overwrite"]; dismiss "Cancel export" (dismissing = cancel, as today) |
| `about` | Rustroom | Compact | the one line | dismiss "Close" |

**Settings' tabs**, from today's thirteen hairline groups in order:

- **Behaviour**: Confirm before deleting a preset · Open new photos with the camera rendering
  (+ hint) · Apply lens corrections to new photos (+ hint) · Write XMP sidecars beside RAW files
  (+ hint) · Move to the next photo after rating (+ hint).
- **Appearance**: the Theme / Accent grid and its hint.
- **Display**: the Display group (the "handled by macOS" row on macOS, the profile picker
  elsewhere) and its hint.
- **Performance**: 1:1 view quality · Rendering (Use the GPU) · Thumbnail cache · Export at once ·
  Resampling.
- **Input**: Pan with · Erase with · Preview while dragging.

Inside a tab the groups keep their current controls and hints; the hairlines between groups go,
and where a group had a label row ("Rendering", "Thumbnail cache") that label becomes a
`theme::sub_label` above its controls.

Every accessible label that exists today keeps its text: "Export", "Save", "Paste", "Sync",
"Delete", "Forget", "Cancel", "Close", "Skip", "Overwrite", "Keep both", "Cancel export", every
checkbox and every combo. The existing tests click them by name and must keep passing unchanged
except where a test asserted the old shell's shape.

## What does not change

- The `Modal` enum, `draw`'s dispatch, every dialog's *behaviour* on confirm and dismiss, the
  size classes and the remembered sizes, the Esc/backdrop dismissal, the collision dialog's
  `Decision` semantics, `Settings`' persistence.
- The dialogs stay resizable within the same minimum and maximum.

## Files

- **New** `crates/app/src/modals/shell.rs`: `dialog`, `Footer`, `DialogOut`, `tabs`; the size
  round-trip tests move here from `mod.rs` with the shell.
- `modals/mod.rs` (dispatch; the four small dialogs it holds), `modals/settings.rs`,
  `modals/export.rs` (696 lines — report it; the tab split may be the moment to move
  `collision` and `save_preset` into `modals/export_small.rs`), `modals/info.rs`,
  `modals/rule_edit.rs`, `modals/presets.rs`, `modals/edits.rs`.
- `theme.rs`: `primary_button` added, `button_row` removed. `shell.rs`: `export_button` calls
  `theme::primary_button`.
- `docs/JOURNAL.md` (a new section at the top of the list, newest first, with a Contents entry),
  `CLAUDE.md` (the live State block overwritten, never appended: the branch, the test count on
  its tip, "gated green, GUI launched, not yet confirmed by use").

## Testing

- `shell`: the footer's buttons are reachable by their labels; a disabled primary reports
  disabled and a click on it does nothing; the × is named "Close" and dismisses; the tab
  control switches which content is drawn (draw two tabs with one distinct label each, click
  the second tab's name, assert the first's label is gone and the second's present); the
  remembered tab survives a second draw in the same context and is 0 in a fresh one.
- `every_dialog_has_a_class_and_the_ids_are_unique` and the size round-trip tests keep passing.
- Settings: every one of today's checkbox labels is reachable across the five tabs (a sweep
  that clicks each tab and unions the labels); Export: "Export" is in the footer whichever tab
  is current; Info: the five tab names are present; collision: all four buttons are named.
- Gate as above; GUI in release, with a look at Settings, Export, Info and one confirm.
