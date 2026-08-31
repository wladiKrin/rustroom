# Dialog sizing Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give all fourteen dialogs one sizing convention — three width classes, roomy padding, a resize handle whose result persists per dialog, a viewport-bounded height that scrolls, and aligned label columns.

**Architecture:** `modals::dialog` already funnels 13 of the 14 dialogs; it grows a `DialogSize` parameter and a `&mut BTreeMap` of persisted sizes, and owns the frame, the `egui::Resize`, the `ScrollArea` and the seed/measure round trip. `theme` owns the class and its numbers. `modals::draw` — the one place that dispatches every modal — gains the close detection that writes the settings file. `shortcuts` and `about` convert to the funnel so the convention is universal.

**Tech Stack:** Rust, egui 0.35 (`Modal`, `Resize`, `Grid`, `ScrollArea`), serde.

**Read first:** `docs/superpowers/specs/2026-08-27-dialog-sizing-design.md`, especially its seven "facts that shaped the design" — each was read out of the vendored egui source at
`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/egui-0.35.0`.

**Before you start:**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

⚠ **No `Co-Authored-By` trailer. Never push. Never merge unprompted.** You are on branch `dialog-sizing`.

⚠⚠ **`theme` and `modals` are BINARY modules.** A `pub` item with no caller is dead code and `clippy -D warnings` rejects it — this project has hit that nine times. Task 1's enum is used by Task 3, so **Tasks 1 and 3 must land together** unless Task 1's tests count as callers (they do not, for a `pub fn` in a bin crate — `#[cfg(test)]` uses exist only in the test target). **Check clippy's exit code before every commit**, not just `cargo test`'s; two commits went in this week with clippy red because a green test run was mistaken for evidence.

⚠ **Appearance is exactly what the headless harness cannot see.** Task 6 is the real acceptance test. Do not claim this works without running it.

---

### Task 1: `theme::DialogSize`

**Files:**
- Modify: `crates/app/src/theme.rs` (add near `button_row`, around line 184)

⚠ Committed together with Task 3 — see the dead-code warning above. Do the work now, commit at the end of Task 3.

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` at the bottom of `crates/app/src/theme.rs`:

```rust
    /// ⚠ A table test over **every** variant, so a fourth class cannot ship inconsistent.
    #[test]
    fn every_dialog_class_defaults_to_at_least_its_own_minimum() {
        for class in DialogSize::ALL {
            let (min_w, min_h) = class.min();
            let (def_w, def_h) = class.default_size();
            assert!(
                def_w >= min_w && def_h >= min_h,
                "{class:?} defaults to {def_w}x{def_h}, below its minimum {min_w}x{min_h}"
            );
        }
    }

    /// ⚠ The classes must actually be ordered, or naming one at a call site means nothing.
    #[test]
    fn the_dialog_classes_get_wider_in_order() {
        let w: Vec<f32> = DialogSize::ALL.iter().map(|c| c.min().0).collect();
        assert!(
            w.windows(2).all(|p| p[0] < p[1]),
            "minimum widths must strictly increase, got {w:?}"
        );
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p app --bin Rustroom every_dialog_class_defaults 2>&1 | tail -20`
Expected: FAIL to compile — `cannot find type DialogSize`.

- [ ] **Step 3: Write the implementation**

Add to `crates/app/src/theme.rs`, immediately above `pub fn button_row`:

```rust
/// How wide a dialog wants to be.
///
/// ⚠⚠ **`egui::Modal` has no width knob at all** — its only sizing surface is `frame` and `area`,
/// so width comes entirely from content. That is *why* every dialog in this app was a different
/// width. The class is the app's own answer, named at the call site and resolved here.
///
/// ⚠ Three rather than one, because a two-button "Delete preset?" at the rule editor's width reads
/// as padding rather than design. Three rather than two, because Settings and Information want to
/// sit between a confirmation and the widest thing in the app.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DialogSize {
    /// A confirmation: a sentence and two buttons.
    Compact,
    /// A form: a handful of labelled controls.
    Standard,
    /// Dense or long content — a rule, a shortcut list, a metadata table.
    Wide,
}

impl DialogSize {
    /// Every class, narrowest first. ⚠ The tests iterate this, so a variant missing from it is
    /// unguarded.
    pub const ALL: [DialogSize; 3] = [Self::Compact, Self::Standard, Self::Wide];

    /// The smallest this class may be dragged to.
    ///
    /// ⚠ A floor, not a target: the width is what the class actually asserts, and the height only
    /// stops a dialog being dragged shut.
    pub fn min(self) -> (f32, f32) {
        match self {
            Self::Compact => (300.0, 80.0),
            Self::Standard => (400.0, 120.0),
            Self::Wide => (520.0, 200.0),
        }
    }

    /// The size a dialog of this class opens at when it has never been resized.
    pub fn default_size(self) -> (f32, f32) {
        match self {
            Self::Compact => (340.0, 140.0),
            Self::Standard => (480.0, 320.0),
            Self::Wide => (640.0, 480.0),
        }
    }
}

/// The inner margin every dialog's frame gets.
///
/// ⚠⚠ **`egui::Modal`'s default frame is `Frame::popup`, whose inner margin is
/// `style.spacing.menu_margin` — `Margin::same(6)`.** That is egui's default, not a choice this
/// project made, and it is the bulk of the reported "very small and cramped".
///
/// ⚠ Applied per modal rather than by raising `menu_margin` in [`tune`], which is shared with
/// every real context menu and would fatten those too.
pub const DIALOG_MARGIN: i8 = 14;

/// The frame a dialog is drawn in.
pub fn dialog_frame(ctx: &egui::Context) -> egui::Frame {
    egui::Frame::popup(&ctx.style()).inner_margin(DIALOG_MARGIN)
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p app --bin Rustroom dialog_class 2>&1 | tail -5`
Expected: PASS, 2 tests.

⚠ Do **not** commit yet — `DialogSize` and `dialog_frame` have no production caller until Task 3, and `clippy -D warnings` will reject them as dead code.

---

### Task 2: `Settings.dialog_sizes`

**Files:**
- Modify: `crates/app/src/settings.rs` (the `Settings` struct, `impl Settings`, `impl Default`, and the exhaustive literal in `settings_round_trip_carries_the_export_options`)

⚠ This one **can** commit alone: `settings` is a **lib** module, where `pub` is public API and does not trip `dead_code`. That is the same contrast `tool` (lib) and `HslChannel` (bin) already illustrate.

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `crates/app/src/settings.rs`:

```rust
    /// ⚠ `SettingsStore::load` is infallible, so one unparseable field silently reverts **every**
    /// setting. The sibling of `a_settings_file_from_before_the_snapshot_strip_still_loads`.
    #[test]
    fn a_settings_file_from_before_dialog_sizes_still_loads() {
        let f = TempFile::new("predialogsizes");
        std::fs::write(&f.0, r#"{"library_cell":200}"#).unwrap();
        let got = SettingsStore::new(f.0.clone()).load();
        assert_eq!(got.library_cell, 200, "the stored field must survive");
        assert!(
            got.dialog_sizes.is_empty(),
            "and an absent map must read as empty, not fail the whole parse"
        );
    }

    /// ⚠ The file is user-editable and a zero width would make a dialog unreachable, so a stored
    /// size is clamped rather than trusted — `clamped_cell`'s rule.
    #[test]
    fn a_nonsense_stored_dialog_size_is_clamped_into_the_classs_range() {
        let mut s = Settings::default();
        s.dialog_sizes.insert("x".into(), [0, 0]);
        let (w, h) = s.dialog_size("x", (400.0, 120.0), (480.0, 320.0));
        assert_eq!((w, h), (400.0, 120.0), "clamped up to the class minimum");

        s.dialog_sizes.insert("x".into(), [99_999, 99_999]);
        let (w, h) = s.dialog_size("x", (400.0, 120.0), (480.0, 320.0));
        assert!(w <= MAX_DIALOG_EDGE && h <= MAX_DIALOG_EDGE, "clamped down");
    }

    /// An unresized dialog gets its class default, not zero.
    #[test]
    fn an_unresized_dialog_gets_its_class_default() {
        let s = Settings::default();
        assert_eq!(s.dialog_size("never", (400.0, 120.0), (480.0, 320.0)), (480.0, 320.0));
    }

    /// A remembered size is returned as stored.
    #[test]
    fn a_remembered_dialog_size_comes_back() {
        let mut s = Settings::default();
        s.remember_dialog_size("x", (612.0, 400.0));
        assert_eq!(s.dialog_size("x", (400.0, 120.0), (480.0, 320.0)), (612.0, 400.0));
    }
```

⚠ Check `TempFile`'s exact construction in that module — read
`a_settings_file_from_before_the_snapshot_strip_still_loads` and copy it rather than assuming.

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p app --lib a_settings_file_from_before_dialog_sizes 2>&1 | tail -20`
Expected: FAIL to compile — `no field dialog_sizes`.

- [ ] **Step 3: Write the implementation**

Add the field to `Settings` in `crates/app/src/settings.rs`, after `snapshot_strip_height`:

```rust
    /// Sizes the user has dragged dialogs to, keyed by the dialog's id.
    ///
    /// ⚠ `[u32; 2]`, not floats: `Settings` derives `Eq`, which no float satisfies — the reasoning
    /// `library_cell` and `snapshot_strip_height` already carry.
    ///
    /// ⚠ `BTreeMap` so the file has a stable order and a diff of it is readable — the reason
    /// `camera_profiles` is one.
    ///
    /// ⚠ Persisted at all because **egui's own `Resize` state dies with the process**: this app
    /// implements no `eframe::App::save`. The panel-sections work established that a control whose
    /// state does not survive a restart is worth very little.
    ///
    /// ⚠ No field-level `#[serde(default)]`: the struct carries a container-level one, so a
    /// missing field comes from `Settings::default()` — an empty map.
    pub dialog_sizes: std::collections::BTreeMap<String, [u32; 2]>,
```

Add the constant beside `STRIP_HEIGHT_RANGE`:

```rust
/// The largest a dialog may be stored as, in points.
///
/// ⚠ A ceiling on the *stored* value only. The dialog additionally clamps itself to 80% of the
/// live viewport, so a size chosen on a large monitor cannot strand it off-screen on a small one.
pub const MAX_DIALOG_EDGE: f32 = 2000.0;
```

Add to `impl Settings`, beside `clamped_strip_height`:

```rust
    /// The size dialog `id` should open at: what it was dragged to, else `default`, clamped.
    ///
    /// ⚠ Clamped rather than trusted — the settings file is user-editable, and a zero width would
    /// make a dialog unreachable. `clamped_cell`'s rule.
    pub fn dialog_size(&self, id: &str, min: (f32, f32), default: (f32, f32)) -> (f32, f32) {
        let (w, h) = match self.dialog_sizes.get(id) {
            Some([w, h]) => (*w as f32, *h as f32),
            None => default,
        };
        (
            w.clamp(min.0, MAX_DIALOG_EDGE),
            h.clamp(min.1, MAX_DIALOG_EDGE),
        )
    }

    /// Record the size dialog `id` is currently at.
    ///
    /// ⚠ In memory only — the **file** is written when the dialog closes, by `modals::draw`. A
    /// resize runs at 60 Hz and the settings file is not a place to write sixty times a second:
    /// the `apply_view_demosaic` rule.
    pub fn remember_dialog_size(&mut self, id: &str, size: (f32, f32)) {
        let v = [size.0.round().max(0.0) as u32, size.1.round().max(0.0) as u32];
        // ⚠ Only on a real change, so a dialog merely being open does not dirty the settings and
        // cause a needless file write when it closes.
        if self.dialog_sizes.get(id) != Some(&v) {
            self.dialog_sizes.insert(id.to_string(), v);
        }
    }
```

Add to `impl Default for Settings`, after `snapshot_strip_height`:

```rust
            dialog_sizes: std::collections::BTreeMap::new(),
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p app --lib settings:: 2>&1 | tail -5`
Expected: PASS.

Run: `cargo build --workspace --all-targets 2>&1 | tail -20`
Expected: **failure** in `settings_round_trip_carries_the_export_options`, whose `Settings { .. }`
literal is exhaustive **on purpose** — it exists so a new field is a compile error there. Add a
non-default value so the round trip actually carries it:

```rust
            // Non-default so a field re-defaulted on load cannot pass.
            dialog_sizes: std::collections::BTreeMap::from([("x".to_string(), [500u32, 300u32])]),
```

- [ ] **Step 5: Sabotage**

Add a field-level `#[serde(default)]` to `dialog_sizes`.
Run: `cargo test -p app --lib a_settings_file_from_before_dialog_sizes 2>&1 | tail -20`
Expected: **PASS** — ⚠ this sabotage is **inert**, because `BTreeMap`'s own default is also empty.
Record it as inert rather than inventing a test that appears to cover it. Restore the code anyway
(the container-level route is what every other field uses).

Then a discriminating one: make `dialog_size` ignore `min` (`(w, h)` instead of the clamp).
Run: `cargo test -p app --lib a_nonsense_stored_dialog_size 2>&1 | tail -20`
Expected: **FAIL**. ⚠ Confirm a `test result:` line appears — a sabotage that fails to compile
reads exactly like one that passes. Restore and re-run to confirm green.

- [ ] **Step 6: Commit**

```bash
cargo fmt && cargo clippy --all-targets --all-features -- -D warnings; echo "clippy exit: $?"
git add crates/app/src/settings.rs
git commit -m "feat(settings): remember the size each dialog was dragged to"
```

---

### Task 3: `dialog()` owns the frame, the size and the scroll

**Files:**
- Modify: `crates/app/src/modals/mod.rs` (`dialog`, `draw`, `shortcuts`, `about`, `confirm_forget_folder`)
- Modify: `crates/app/src/modals/{edits,export,info,presets,rule_edit,settings}.rs` (the call sites)
- Modify: `crates/app/src/theme.rs` (Task 1's code — commit it here)

⚠⚠ **One commit with Task 1.** `DialogSize` and `dialog_frame` are `pub` in a **binary** module and have no production caller until this task.

- [ ] **Step 1: Rewrite `dialog`**

⚠ `crates/app/src/modals/mod.rs` imports only `crate::RustroomApp`, `edit_model::EditGroups` and
`eframe::egui` — **add `use crate::theme;`** or every `theme::` path below fails to resolve.

⚠ `Settings` is in scope at the bin root as `rustroom_app::settings::Settings` (see `main.rs:24`);
the signature below spells it out rather than relying on a re-export that does not exist.

Replace `modals::dialog` in `crates/app/src/modals/mod.rs`:

```rust
/// Show a centred modal with a heading and a separator. Returns whether the backdrop was
/// clicked, which every dialog treats as a dismissal.
///
/// ⚠ `egui::Modal`, not `egui::Window`: a Window opens at the top-left and is genuinely easy to
/// miss, while a Modal centres on the viewport, dims the backdrop and blocks clicks behind it.
///
/// ⚠⚠ **This is the single place dialog sizing lives.** Fourteen dialogs each choosing their own
/// width is how they became inconsistent; a call site names a [`theme::DialogSize`] and nothing
/// else.
///
/// ⚠⚠ **The dialog stays centred as it grows**, because `Modal` anchors `Align2::CENTER_CENTER`.
/// The corner handle therefore drifts from the pointer at half rate. That is inherent to a centred
/// modal and cannot be fixed without giving up the centring, which was the deliberate choice over
/// `egui::Window`.
///
/// ⚠ The size is measured as `ui.available_size()` on the **first line** of the content closure —
/// the `Resize`'s own `content_ui`, whose extent is the same quantity `default_size` sets. Feeding
/// back the same quantity is what makes the round trip unable to drift. It is measured here rather
/// than read from `egui::Resize::State`, whose `desired_size` is `pub(crate)`.
pub fn dialog<R>(
    ctx: &egui::Context,
    settings: &mut rustroom_app::settings::Settings,
    id: &str,
    title: &str,
    size: theme::DialogSize,
    content: impl FnOnce(&mut egui::Ui) -> R,
) -> bool {
    let min = size.min();
    let seed = settings.dialog_size(id, min, size.default_size());
    // ⚠ Bounded by the live viewport, so a size chosen on a large monitor is clamped rather than
    // stranding the dialog off-screen on a small one.
    let vp = ctx.screen_rect().size();
    let max = (vp.x * 0.8, vp.y * 0.8);
    let mut measured = seed;

    let backdrop = egui::Modal::new(egui::Id::new(id))
        .frame(theme::dialog_frame(ctx))
        .show(ctx, |ui| {
            egui::Resize::default()
                .id_salt(id)
                .default_size(seed)
                .min_size(min)
                .max_size(max)
                .resizable(true)
                .show(ui, |ui| {
                    measured = (ui.available_width(), ui.available_height());
                    ui.heading(title);
                    ui.separator();
                    // ⚠ The content scrolls, so a long rule or preset list can never push the
                    // dialog past the bottom of the screen. ⚠ The buttons scroll **with** it:
                    // `dialog` does not own the button row, and pinning it means restructuring
                    // every call site. They stay reachable, which is the bug being fixed.
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            content(ui);
                        });
                });
        })
        .backdrop_response
        .clicked();

    settings.remember_dialog_size(id, measured);
    backdrop
}
```

- [ ] **Step 2: Update every call site**

Each becomes `dialog(ctx, &mut app.settings, "<id>", "<title>", theme::DialogSize::<Class>, |ui| …)`.

⚠ Every closure already mutates **locals only** — that is this module's documented rule — so the
extra `&mut app.settings` borrow is disjoint. ⚠ `settings::show` reads `app.settings` into locals
*before* the closure; keep it that way or the borrow will not compile.

| file:line | id | class |
|---|---|---|
| `presets.rs:19` | `save_preset` | `Standard` |
| `presets.rs:78` | `confirm_delete` | `Compact` |
| `edits.rs:16` | `paste_settings` | `Standard` |
| `edits.rs:46` | `sync_settings` | `Standard` |
| `export.rs:33` | `export_settings` | `Standard` |
| `export.rs:191` | `save_export_preset` | `Standard` |
| `export.rs:236` | `confirm_delete_export` | `Compact` |
| `export.rs:290` | `export_collision` | `Standard` |
| `mod.rs:212` | `confirm_forget_folder` | `Compact` |
| `info.rs:115` | `photo_info` | `Wide` |
| `rule_edit.rs:309` | `edit_rule` | `Wide` |
| `settings.rs:20` | `settings` | `Standard` |

⚠ Add `use crate::theme;` where a file does not already import it.

- [ ] **Step 3: Remove the three inner scroll areas**

⚠⚠ **Nested vertical scroll areas fight over the wheel** — the inner one under the pointer eats it,
so the dialog could not be scrolled past whichever list you happened to be over. That is the exact
bug the Library panel's three inner scroll areas were removed for on 2026-08-27.

- `crates/app/src/modals/info.rs:116` — delete the `egui::ScrollArea::vertical().max_height(460.0).show(ui, |ui| { … })` wrapper, keeping its body.
- `crates/app/src/modals/rule_edit.rs:339` — same.
- `crates/app/src/modals/mod.rs:246` (`shortcuts`) — same; it is replaced by the shared one in Step 4.

- [ ] **Step 4: Convert `shortcuts` and `about` to the funnel**

Replace both in `crates/app/src/modals/mod.rs`:

```rust
/// The keyboard-shortcut reference.
///
/// ⚠ **Generated from `menu::model()`, never hand-written.** A shortcuts dialog listing a chord
/// the app does not have is worse than no dialog at all, and a hand-written one drifts the first
/// time a shortcut moves.
fn shortcuts(app: &mut RustroomApp, ctx: &egui::Context) {
    let mut close = false;
    let dismissed = dialog(
        ctx,
        &mut app.settings,
        "shortcuts",
        "Keyboard shortcuts",
        theme::DialogSize::Wide,
        |ui| {
            for section in rustroom_app::menu::model() {
                ui.add_space(4.0);
                ui.label(egui::RichText::new(section.title).strong());
                for item in section.items {
                    if let rustroom_app::menu::Item::Action { label, shortcut, .. } = item {
                        if let Some(s) = shortcut {
                            ui.horizontal(|ui| {
                                ui.label(label);
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| theme::caption(ui, &s.label()),
                                );
                            });
                        }
                    }
                }
            }
            ui.separator();
            if ui.button("Close").clicked() {
                close = true;
            }
        },
    );
    if close || dismissed {
        app.modal = Modal::None;
    }
}

/// The about box.
fn about(app: &mut RustroomApp, ctx: &egui::Context) {
    let mut close = false;
    let dismissed = dialog(
        ctx,
        &mut app.settings,
        "about",
        "Rustroom",
        // ⚠ `Compact` despite being a "form-ish" dialog: it is two lines and a Close button, and
        // `Standard` would be mostly empty.
        theme::DialogSize::Compact,
        |ui| {
            theme::hint(ui, "An open-source RAW photo editor.");
            ui.separator();
            if ui.button("Close").clicked() {
                close = true;
            }
        },
    );
    if close || dismissed {
        app.modal = Modal::None;
    }
}
```

⚠ **Read the existing `shortcuts` body before replacing it** and preserve exactly what it renders —
the loop above is written from `menu::model()`'s shape and the real one may differ in field names.
The point of this step is the *funnel*, not a rewrite of what it shows.

- [ ] **Step 5: Build and look for the borrow errors**

Run: `cargo build --workspace --all-targets 2>&1 | tail -30`
Expected: clean. If a call site fails with `E0499`/`E0502`, the closure is touching `app` —
hoist that read into a local **before** the `dialog(..)` call, which is this module's documented
rule anyway.

- [ ] **Step 6: Verify the classes are complete**

Add to `mod tests` in `crates/app/src/modals/mod.rs` (create the module if absent):

```rust
#[cfg(test)]
mod tests {
    use crate::theme::DialogSize;

    /// ⚠ Every dialog id in the app, with the class it must use. A completeness list in the
    /// spirit of `every_rail_glyph_is_renderable`: a dialog added without a class is a dialog
    /// that will be sized by accident, and this is the only place that would notice.
    #[test]
    fn every_dialog_has_a_class_and_the_count_is_fourteen() {
        let dialogs: [(&str, DialogSize); 14] = [
            ("save_preset", DialogSize::Standard),
            ("confirm_delete", DialogSize::Compact),
            ("paste_settings", DialogSize::Standard),
            ("sync_settings", DialogSize::Standard),
            ("export_settings", DialogSize::Standard),
            ("save_export_preset", DialogSize::Standard),
            ("confirm_delete_export", DialogSize::Compact),
            ("export_collision", DialogSize::Standard),
            ("confirm_forget_folder", DialogSize::Compact),
            ("photo_info", DialogSize::Wide),
            ("edit_rule", DialogSize::Wide),
            ("settings", DialogSize::Standard),
            ("shortcuts", DialogSize::Wide),
            ("about", DialogSize::Compact),
        ];
        let mut ids: Vec<&str> = dialogs.iter().map(|(i, _)| *i).collect();
        let n = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), n, "every dialog id must be unique");
    }
}
```

⚠ This is a **documentation** test — it pins the roster and the ids' uniqueness, not the wiring.
Say so in its comment rather than overselling it; the wiring is verified by hand in Task 6.

- [ ] **Step 7: Full green, then commit Tasks 1 and 3 together**

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings; echo "clippy exit: $?"
cargo test --all 2>&1 | grep "test result:" | awk -F'[ ;]' '{p+=$4; f+=$6} END {print "passed="p, "failed="f}'
```
Expected: `clippy exit: 0`, no failures.

```bash
git add -A
git commit -m "feat(modals): one sizing convention for every dialog"
```

---

### Task 4: Write the settings file when a dialog closes

**Files:**
- Modify: `crates/app/src/modals/mod.rs` (`draw`)

⚠⚠ **One hook, not fourteen.** Each dialog sets `app.modal = Modal::None` from its own close path —
button, backdrop or Escape — so persisting at each would be fourteen edits and a fifteenth waiting
to be forgotten. `draw` is the one place that already dispatches every modal.

- [ ] **Step 1: Add the close detection**

In `crates/app/src/modals/mod.rs`, wrap the body of `draw`:

```rust
pub fn draw(app: &mut RustroomApp, ctx: &egui::Context) {
    // ⚠⚠ **The one place a dialog's size is written to disk.** `dialog` keeps `Settings` current
    // in memory every frame, which is cheap; the *file* is written only here, on the frame a
    // dialog closes. A resize runs at 60 Hz and the settings file is not a place to write sixty
    // times a second — the `apply_view_demosaic` rule.
    //
    // ⚠ Detected centrally rather than in each dialog's close path: there are fourteen of those,
    // and every one of them would have to remember.
    let was_open = !matches!(app.modal, Modal::None) || app.export_collision.is_some();

    // ---- everything already in `draw` stays exactly as it is: the `Which` enum,
    // ---- the `match &app.modal`, the `export_collision` call and the dispatch `match`. ----

    let still_open = !matches!(app.modal, Modal::None) || app.export_collision.is_some();
    if was_open && !still_open {
        app.save_settings();
    }
}
```

- [ ] **Step 2: Build and run the suite**

Run: `cargo build --workspace --all-targets 2>&1 | tail -10`
Expected: clean.

Run: `cargo test --all 2>&1 | grep "test result:" | awk -F'[ ;]' '{p+=$4; f+=$6} END {print "passed="p, "failed="f}'`
Expected: no failures.

- [ ] **Step 3: Record the honest gap**

⚠ There is **no headless test for this hook**, and there should not be a fake one: it needs a live
`egui::Context`, a real modal frame and a real file. Task 6 step 4 is its verification. Write that
into the comment above so the next reader knows it is deliberate, not an oversight.

- [ ] **Step 4: Commit**

```bash
cargo fmt && cargo clippy --all-targets --all-features -- -D warnings; echo "clippy exit: $?"
git add crates/app/src/modals/mod.rs
git commit -m "feat(modals): persist a dialog's size when it closes"
```

---

### Task 5: Aligned label columns

**Files:**
- Modify: `crates/app/src/modals/info.rs` (`row`, around line 246)
- Modify: `crates/app/src/modals/settings.rs` (the Theme / Accent / Display rows, around lines 24–60)
- Modify: `crates/app/src/modals/export.rs` (the Format / Quality / Long edge / Space rows)

- [ ] **Step 1: Convert `info::row` to a Grid**

⚠ Information is **already half-solved**, which is worth knowing so this is not oversold: `row`
uses `add_sized([130.0, ..])`, a fixed-width label. That aligns but truncates a long label and is a
magic number. `egui::Grid` sizes to the widest row instead.

In `crates/app/src/modals/info.rs`, replace `row` and wrap each run of rows in a grid. The rows are
drawn in groups separated by `ui.separator()`; each group becomes one grid with its **own** id:

```rust
/// One `label: value` line inside a [`grid`] block.
///
/// ⚠ Must be called inside `Grid::show`, which is what aligns the value column. The previous
/// version used a fixed 130 pt label, which aligns until a label is longer than that.
fn row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.label(egui::RichText::new(label).weak());
    ui.label(value);
    ui.end_row();
}

/// A block of `label: value` rows with an aligned value column.
///
/// ⚠ Each block needs its **own** salt: `egui::Grid` sizes columns per id, so sharing one across
/// the File/Image/Capture blocks would make them share a width — which is *sometimes* what you
/// want and is not what these want, since "Path" is far longer than "ISO".
fn grid(ui: &mut egui::Ui, salt: &str, rows: impl FnOnce(&mut egui::Ui)) {
    egui::Grid::new(salt)
        .num_columns(2)
        .spacing([12.0, 4.0])
        .show(ui, rows);
}
```

Then wrap each block, e.g.:

```rust
                ui.label(egui::RichText::new("File").strong());
                grid(ui, "info_file", |ui| {
                    row(ui, "Name", &shown);
                    row(ui, "Path", &id.path.to_string_lossy());
                    row(ui, "Copy", &id.copy.to_string());
                    row(ui, "Size", &size.map(size_text).unwrap_or_else(|| "—".into()));
                });
                ui.separator();
```

⚠ Read the file and convert **every** block the same way — do not leave some rows outside a grid,
or they will be the ragged ones.

- [ ] **Step 2: Convert the Settings rows**

In `crates/app/src/modals/settings.rs`, replace the bare
`ui.horizontal(|ui| { ui.label("Theme"); ComboBox … })` rows with one grid:

```rust
        egui::Grid::new("settings_appearance")
            .num_columns(2)
            .spacing([12.0, 4.0])
            .show(ui, |ui| {
                ui.label("Theme");
                egui::ComboBox::from_id_salt("theme_pick")
                    .selected_text(theme_choice.label())
                    .show_ui(ui, |ui| {
                        for t in ThemeChoice::ALL {
                            ui.selectable_value(&mut theme_choice, t, t.label());
                        }
                    });
                ui.end_row();

                ui.label("Accent");
                egui::ComboBox::from_id_salt("accent_pick")
                    .selected_text(accent.label())
                    .show_ui(ui, |ui| {
                        for a in Accent::ALL {
                            ui.selectable_value(&mut accent, a, a.label());
                        }
                    });
                ui.end_row();
            });
```

⚠ The Display row sits in a **runtime** `if os_manages_colour` branch. Put it in its own grid with
its own salt rather than trying to make one grid span a conditional — a `Grid` whose row count
changes between frames is an id-and-column-width problem for no gain.

- [ ] **Step 3: Convert the Export rows**

⚠⚠ **Export has exactly THREE labelled control rows, and they are NOT contiguous.** Read before
converting: `Format` (line 76) and `Colour space` (line 86) are adjacent; `Demosaic` (line 120) is
separated from them by the banding hint, the JPEG-quality slider, the "Fit within a long edge"
checkbox and the long-edge slider.

⚠⚠ **Putting all three in one grid would reorder the dialog** — it would move Demosaic up above the
quality slider and detach the banding hint from the two controls it is about, whose placement
`export.rs` has a comment explaining (*"Placed under the two controls it is about, so the fix
('use 16-bit') is one glance from the Format dropdown"*). Alignment is not worth silently
rearranging a dialog someone reasoned about.

So: **one grid, covering `Format` and `Colour space` only.** Replace those two `ui.horizontal`
blocks with:

```rust
        egui::Grid::new("export_output")
            .num_columns(2)
            .spacing([12.0, 4.0])
            .show(ui, |ui| {
                ui.label("Format");
                egui::ComboBox::from_id_salt("format_pick")
                    .selected_text(opts.format.label())
                    .show_ui(ui, |ui| {
                        for f in export::ExportFormat::ALL {
                            ui.selectable_value(&mut opts.format, f, f.label());
                        }
                    });
                ui.end_row();

                ui.label("Colour space");
                egui::ComboBox::from_id_salt("space_pick")
                    .selected_text(opts.space.label())
                    .show_ui(ui, |ui| {
                        for s in color::OutputSpace::ALL {
                            ui.selectable_value(&mut opts.space, s, s.label());
                        }
                    });
                ui.end_row();
            });
```

⚠ **Leave `Demosaic` as a `ui.horizontal`.** A grid of one row aligns nothing against anything, and
moving it to join the others is the reordering ruled out above.

⚠ **Leave the two sliders alone.** `Slider::text("JPEG quality")` and `Slider::text("Long edge
(px)")` carry their own labels inside the widget — they are not `label: control` rows and do not
belong in a column.

- [ ] **Step 4: Build and test**

Run: `cargo build --workspace --all-targets 2>&1 | tail -10`
Run: `cargo test --all 2>&1 | grep "test result:" | awk -F'[ ;]' '{p+=$4; f+=$6} END {print "passed="p, "failed="f}'`
Expected: clean, no failures.

⚠ There is **no test for alignment** — it is appearance, which the harness cannot see. Task 6
step 6 is its verification.

- [ ] **Step 5: Commit**

```bash
cargo fmt && cargo clippy --all-targets --all-features -- -D warnings; echo "clippy exit: $?"
git add -A
git commit -m "feat(modals): align the label columns in Information, Settings and Export"
```

---

### Task 6: Look at it — the real acceptance test

⚠⚠ **This task is not optional and cannot be replaced by the suite.** Appearance is the one thing
the headless harness explicitly cannot see, and this project has recorded twelve GUI defects found
by using the app and none by a test.

- [ ] **Step 1: Launch**

```bash
cargo run --release -p app 2>&1 | tail -5 &
```

⚠ Always `--release`, in the background.

- [ ] **Step 2: Open all fourteen dialogs**

File ▸ Export…; File ▸ Settings…; Photo ▸ Take snapshot then the Presets rail ▸ Save as preset…;
Edit ▸ Paste settings…; Edit ▸ Sync selection…; Help ▸ Keyboard shortcuts; Help ▸ About; the
Library context menu ▸ Information; a smart collection's Edit rule; a preset's Delete; an export
preset's Delete; a folder's Forget; and an export into a folder that already has the file.

For each: **is it a sensible width for its class, and is it no longer cramped against its frame?**

- [ ] **Step 3: The centred-resize feel**

Drag a dialog's bottom-right corner. ⚠ **The handle drifts from the pointer at half rate** because
the modal stays centred. Judge whether that is acceptable. **If it is not, the fallback is to drop
`.resizable(true)` and keep the three classes** — say so rather than trying to defeat the centring,
which was a deliberate choice over `egui::Window`.

- [ ] **Step 4: The persistence round trip — the mechanism that could not be verified from source**

Resize the rule editor. Close it. Reopen it: **the size must have come back.** Repeat three times
and confirm it does **not** creep larger or smaller each cycle.

Then quit the app, relaunch, and reopen it: the size must still be there. Check the file:

```bash
python3 -c "import json;print(json.load(open('$HOME/Library/Application Support/rustroom/settings.json')).get('dialog_sizes'))"
```

⚠ If it creeps, `ui.available_size()` is not the quantity `default_size` sets. The fix is to feed
back whatever *is* — instrument it with one `eprintln` printing seed and measured rather than
guessing, which is the lesson this project has now recorded three times.

- [ ] **Step 5: Long content**

Open a smart collection's rule editor with several rows, and Keyboard shortcuts. **Neither may
extend past the bottom of the screen**, and the buttons must be reachable by scrolling.

- [ ] **Step 6: Alignment**

Settings, Export and Information: **the value column lines up.**

- [ ] **Step 7: Light mode**

Settings ▸ Theme ▸ Light. ⚠ Light mode has barely been looked at in this project; check the frame
and the resize handle are visible against it.

- [ ] **Step 8: Update the docs**

Add a section to `CLAUDE.md`'s "Current status" in the house style — what shipped, the ⚠ findings
(above all the centred drift and whatever step 4 revealed), the test count, and the accepted
caveats from the spec. Remove the **Dialog sizing** entry from `docs/BACKLOG.md`. ⚠ Update the
`▶ NEXT ACTION` to **edit-accurate thumbnails**, which the user has already chosen as the next
piece of work.

```bash
git add -A
git commit -m "docs: dialog sizing shipped"
```

⚠ **Do not push and do not merge.** Report the branch state and let the user try it.
