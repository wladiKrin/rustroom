# Menu bar Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give Rustroom a menu bar — native on macOS, in-window elsewhere — driven by one menu model and one command dispatcher, and slim the Develop toolbar from ~14 controls to six.

**Architecture:** `rustroom_app::menu` (lib, egui-free) holds the `Command` enum, a `Shortcut`, and the menu tree. One `commands::execute` dispatcher in the bin is fed by three sources: menu clicks, key presses, and the rail. `menu_bar.rs` renders the model with `egui::containers::menu::MenuBar` and **compiles on every platform**; `native_menu.rs` renders the same model as an NSMenu via `muda` and is the only `#[cfg]`-gated module.

**Tech Stack:** Rust, egui 0.35 (`containers::menu::MenuBar`), [`muda` 0.19](https://docs.rs/muda) on macOS.

**Spec:** `docs/superpowers/specs/2026-08-23-layout-ia-rework-design.md`. This is **step 3 of 3**; steps 1 (the panel) and 2 (the rail) landed 2026-08-24.

---

## ⚠⚠ Read first: two design decisions that differ from the spec

Both come from facts checked while writing this plan, not from preference.

### 1. Only *some* shortcuts become native accelerators — and no probe is needed

The spec worried that ⌘C could not be focus-gated on a native menu and proposed disabling items while a
text field has focus. **That machinery is not being built, because a simpler rule removes the hazard
entirely.**

An NSMenu accelerator is consumed before winit sees the key. So:

- **A bare-key shortcut must never be a native accelerator.** `G`, `D`, `\`, `1`–`4` would steal every
  such character typed into the Save-preset name field. This is not a judgement call; it is fatal.
- **A text-editing chord must never be a native accelerator.** ⌘Z, ⌘C, ⌘V, ⌘X, ⌘A all have meaning
  inside a `TextEdit`.

Everything left — ⌘O, ⇧⌘O, ⌘E, ⌘, , ⌘[ , ⌘] — has no text-editing meaning and is safe.

So `Shortcut::safe_as_native_accelerator()` decides, with tests. The unsafe ones stay handled by egui
exactly as today, **focus-gated by the code that already does it**, and the menu shows the chord *in
the item's label* so it stays discoverable. Cost, stated plainly: on macOS those items show their
shortcut as label text rather than right-aligned native accelerator text. That is cosmetic, and it
buys the removal of a whole class of bug.

⚠ **Why there is no probe task here, unlike the rail's glyph probe.** The open question — whether a
*disabled* NSMenuItem releases its key equivalent — can only be answered with a running native menu,
i.e. after Task 5. The glyph question could be answered in a unit test before any code existed. The
asymmetry is deliberate. If someone later wants ⌘Z as a real accelerator, that probe is the way in,
and `safe_as_native_accelerator` is the one place to change.

### 2. The egui menu bar compiles on every platform; only its *invocation* is gated

The spec implied `#[cfg]`-ing the egui renderer to non-macOS. Do not: the developer works on macOS, so
that renderer would never be compiled or tested locally, and Linux CI would never compile the muda
one — **both halves untested by whoever is looking**. Instead `menu_bar.rs` is built and tested
everywhere, and only the decision to *call* it is platform-dependent. `native_menu.rs` stays the sole
`#[cfg]`-gated module, and it is deliberately thin.

---

## File Structure

| file | responsibility |
|---|---|
| `crates/app/src/menu.rs` | **Create.** Lib. `Command`, `Shortcut`, `Item`, `Section`, `model()`. No egui, no muda. |
| `crates/app/src/commands.rs` | **Create.** Bin. `execute(app, ctx, cmd)` — one exhaustive match. |
| `crates/app/src/menu_bar.rs` | **Create.** Bin. Renders `model()` with `egui::containers::menu::MenuBar`. Compiled everywhere. |
| `crates/app/src/native_menu.rs` | **Create.** Bin, `#[cfg(target_os = "macos")]`. muda NSMenu. The only gated module. |
| `crates/app/src/develop/mod.rs` | Toolbar slims to six controls; Undo/Redo/Copy/Paste/Sync/Export/Settings leave. |
| `crates/app/src/shell.rs` | Shows the menu bar; drains menu events; routes keys through `commands`. |
| `crates/app/src/main.rs` | `handle_clipboard_keys` emits Commands; `preview_edits` untouched. |
| `crates/app/Cargo.toml` | `muda` as a macOS-only dependency. |

---

### Task 1: The menu model

⚠ Lib module, so — unlike the bin's `HslChannel` — it can be committed before it has a caller.

**Files:**
- Create: `crates/app/src/menu.rs`
- Modify: `crates/app/src/lib.rs`

- [ ] **Step 1: Write the model**

Create `crates/app/src/menu.rs`:

```rust
//! The menu model: what the menus contain, independent of how they are drawn.
//!
//! ⚠ **No egui and no muda here.** `lib.rs`'s rule keeps these units testable without a
//! windowing system, and this module has two renderers — an in-window `egui::MenuBar` and a
//! native macOS NSMenu — which is exactly why the model must not know about either.

use crate::tool::Tool;

/// Everything the app can be asked to do from a menu, a key, or the rail.
///
/// ⚠ One enum with **one exhaustive dispatcher**, so a command that nothing performs is a
/// compile error rather than a menu item that silently does nothing.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Command {
    OpenFile,
    OpenFolder,
    Export,
    Settings,
    Undo,
    Redo,
    CopySettings,
    PasteSettings,
    PasteSettingsWith,
    SyncSelection,
    ResetAdjustments,
    ShowLibrary,
    ShowDevelop,
    FitView,
    ToggleBeforeAfter,
    ToggleFilmstrip,
    ToggleMaskOverlay,
    SelectTool(Tool),
    RotateLeft,
    RotateRight,
    SavePreset,
    KeyboardShortcuts,
    About,
}

/// A keyboard chord, in the model's own terms so neither renderer's types leak in here.
///
/// `key` is the uppercase character or digit as printed on the key: `'O'`, `'Z'`, `'1'`,
/// `'\\'`, `'['`, `','`, `'0'`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Shortcut {
    /// Cmd on macOS, Ctrl elsewhere.
    pub cmd: bool,
    pub shift: bool,
    pub key: char,
}

impl Shortcut {
    pub const fn cmd(key: char) -> Self {
        Self { cmd: true, shift: false, key }
    }
    pub const fn cmd_shift(key: char) -> Self {
        Self { cmd: true, shift: true, key }
    }
    pub const fn bare(key: char) -> Self {
        Self { cmd: false, shift: false, key }
    }

    /// Whether this chord may be registered as a **native** menu accelerator.
    ///
    /// ⚠⚠ The rule that removes the whole text-field hazard, and neither half is optional.
    ///
    /// A native accelerator is consumed by the menu *before* the window sees the key. So a
    /// **bare key** would steal every `g`, `d` or `2` typed into the Save-preset name field,
    /// and a **text-editing chord** (⌘Z/C/V/X/A) would break editing inside it. Everything
    /// else — ⌘O, ⌘E, ⌘, , ⌘[ — has no meaning in a text field and is safe.
    ///
    /// The unsafe ones are still shown in the menu and still work: egui handles them, gated on
    /// `memory().focused()` exactly as it does today.
    pub fn safe_as_native_accelerator(self) -> bool {
        self.cmd && !matches!(self.key, 'Z' | 'C' | 'V' | 'X' | 'A')
    }

    /// How the chord is written for a human, e.g. `⌘O` on macOS, `Ctrl+O` elsewhere.
    pub fn label(self) -> String {
        let key = if self.key == '\\' { "\\".to_string() } else { self.key.to_string() };
        if cfg!(target_os = "macos") {
            format!(
                "{}{}{}",
                if self.shift { "⇧" } else { "" },
                if self.cmd { "⌘" } else { "" },
                key
            )
        } else {
            let mut s = String::new();
            if self.cmd {
                s.push_str("Ctrl+");
            }
            if self.shift {
                s.push_str("Shift+");
            }
            s.push_str(&key);
            s
        }
    }
}

/// One entry in a menu.
#[derive(Clone, Debug)]
pub enum Item {
    Action {
        label: &'static str,
        command: Command,
        shortcut: Option<Shortcut>,
    },
    Separator,
}

/// One top-level menu.
#[derive(Clone, Debug)]
pub struct Section {
    pub title: &'static str,
    pub items: Vec<Item>,
}

fn action(label: &'static str, command: Command, shortcut: Option<Shortcut>) -> Item {
    Item::Action { label, command, shortcut }
}

/// The menu bar's contents.
///
/// ⚠ The **Photo** section is what makes the rail keyboard-reachable, numbered 1–4 the way
/// Lightroom numbers its modules. Those are bare keys, so they never become native
/// accelerators — see [`Shortcut::safe_as_native_accelerator`].
pub fn model() -> Vec<Section> {
    vec![
        Section {
            title: "File",
            items: vec![
                action("Open file…", Command::OpenFile, Some(Shortcut::cmd('O'))),
                action("Open folder…", Command::OpenFolder, Some(Shortcut::cmd_shift('O'))),
                Item::Separator,
                action("Export…", Command::Export, Some(Shortcut::cmd('E'))),
                Item::Separator,
                action("Settings…", Command::Settings, Some(Shortcut::cmd(','))),
            ],
        },
        Section {
            title: "Edit",
            items: vec![
                action("Undo", Command::Undo, Some(Shortcut::cmd('Z'))),
                action("Redo", Command::Redo, Some(Shortcut::cmd_shift('Z'))),
                Item::Separator,
                action("Copy settings", Command::CopySettings, Some(Shortcut::cmd('C'))),
                action("Paste settings", Command::PasteSettings, Some(Shortcut::cmd('V'))),
                action("Paste settings…", Command::PasteSettingsWith, None),
                Item::Separator,
                action("Sync selection…", Command::SyncSelection, None),
                action("Reset adjustments", Command::ResetAdjustments, None),
            ],
        },
        Section {
            title: "View",
            items: vec![
                action("Library", Command::ShowLibrary, Some(Shortcut::bare('G'))),
                action("Develop", Command::ShowDevelop, Some(Shortcut::bare('D'))),
                Item::Separator,
                action("Fit", Command::FitView, Some(Shortcut::cmd('0'))),
                action("Before / After", Command::ToggleBeforeAfter, Some(Shortcut::bare('\\'))),
                Item::Separator,
                action("Filmstrip", Command::ToggleFilmstrip, None),
                action("Show mask overlay", Command::ToggleMaskOverlay, None),
            ],
        },
        Section {
            title: "Photo",
            items: vec![
                action("Edit", Command::SelectTool(Tool::Edit), Some(Shortcut::bare('1'))),
                action("Crop & Geometry", Command::SelectTool(Tool::Crop), Some(Shortcut::bare('2'))),
                action("Masks", Command::SelectTool(Tool::Masks), Some(Shortcut::bare('3'))),
                action("Presets", Command::SelectTool(Tool::Presets), Some(Shortcut::bare('4'))),
                Item::Separator,
                action("Rotate left", Command::RotateLeft, Some(Shortcut::cmd('['))),
                action("Rotate right", Command::RotateRight, Some(Shortcut::cmd(']'))),
                Item::Separator,
                action("Save as preset…", Command::SavePreset, None),
            ],
        },
        Section {
            title: "Help",
            items: vec![
                action("Keyboard shortcuts", Command::KeyboardShortcuts, None),
                action("About Rustroom", Command::About, None),
            ],
        },
    ]
}

/// Every command the model contains, in menu order.
pub fn commands() -> Vec<Command> {
    model()
        .into_iter()
        .flat_map(|s| s.items)
        .filter_map(|i| match i {
            Item::Action { command, .. } => Some(command),
            Item::Separator => None,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_two_items_share_a_shortcut() {
        // ⚠ Two items on one chord means one of them is unreachable, and which one wins
        // depends on iteration order — the kind of bug that looks like "the menu is flaky".
        let mut seen: Vec<Shortcut> = model()
            .into_iter()
            .flat_map(|s| s.items)
            .filter_map(|i| match i {
                Item::Action { shortcut, .. } => shortcut,
                Item::Separator => None,
            })
            .collect();
        let total = seen.len();
        seen.sort_by_key(|s| (s.cmd, s.shift, s.key));
        seen.dedup();
        assert_eq!(seen.len(), total, "two menu items share a shortcut");
    }

    #[test]
    fn every_command_appears_exactly_once() {
        // A command in two places is two entry points that will drift apart.
        let mut c = commands();
        let total = c.len();
        c.sort_by_key(|c| format!("{c:?}"));
        c.dedup();
        assert_eq!(c.len(), total, "a command appears twice in the menu");
    }

    #[test]
    fn every_tool_is_reachable_from_the_photo_menu() {
        // ⚠ Stated over `Tool::ALL`, so a fifth tool cannot ship with no way to select it by
        // keyboard — which would compile and look fine.
        let cmds = commands();
        for t in Tool::ALL {
            assert!(
                cmds.contains(&Command::SelectTool(t)),
                "{t:?} has no menu entry"
            );
        }
    }

    #[test]
    fn no_bare_key_or_text_chord_is_a_native_accelerator() {
        // ⚠⚠ The load-bearing safety rule. A bare key as a native accelerator steals every
        // such character typed into a text field; ⌘C/⌘V/⌘Z break editing inside one.
        for s in [
            Shortcut::bare('G'),
            Shortcut::bare('1'),
            Shortcut::bare('\\'),
            Shortcut::cmd('Z'),
            Shortcut::cmd('C'),
            Shortcut::cmd('V'),
            Shortcut::cmd_shift('Z'),
        ] {
            assert!(
                !s.safe_as_native_accelerator(),
                "{s:?} must not become a native accelerator"
            );
        }
        for s in [Shortcut::cmd('O'), Shortcut::cmd('E'), Shortcut::cmd(',')] {
            assert!(s.safe_as_native_accelerator(), "{s:?} is safe and should be native");
        }
    }

    #[test]
    fn every_section_has_a_title_and_at_least_one_action() {
        // An empty menu renders as a title that opens nothing.
        for s in model() {
            assert!(!s.title.is_empty());
            assert!(
                s.items.iter().any(|i| matches!(i, Item::Action { .. })),
                "{} has no actions",
                s.title
            );
        }
    }

    #[test]
    fn every_shortcut_has_a_label() {
        for s in model().into_iter().flat_map(|s| s.items) {
            if let Item::Action { shortcut: Some(sc), label, .. } = s {
                assert!(!sc.label().is_empty(), "{label} has an unprintable shortcut");
            }
        }
    }
}
```

Add `pub mod menu;` to `crates/app/src/lib.rs`, after `pub mod histogram;`.

- [ ] **Step 2: Run the tests**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo test -p app --lib menu:: 2>&1 | tail -15
```

Expected: 6 PASS.

- [ ] **Step 3: Sabotage — make a bare key look native-safe**

```rust
    pub fn safe_as_native_accelerator(self) -> bool {
        !matches!(self.key, 'Z' | 'C' | 'V' | 'X' | 'A')
    }
```

```bash
cargo test -p app --lib --no-run 2>&1 | grep -cE "^error" || echo "0 compile errors"
cargo test -p app --lib no_bare_key_or_text_chord 2>&1 | tail -8
```

Expected: zero compile errors, then FAIL naming a bare shortcut. Restore, then `touch crates/app/src/menu.rs`.

- [ ] **Step 4: Verify and commit**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -5
cargo test --all 2>&1 | grep -oE "^test result: ok\. [0-9]+" | awk '{s+=$4} END {print "TOTAL:", s}'
```

Expected: clean; **694** (688 + 6).

```bash
git add crates/app/src/menu.rs crates/app/src/lib.rs
git commit -m "feat(app): a menu model with one command enum"
```

---

### Task 2: The command dispatcher

Every existing entry point is rewritten to emit a `Command`, so there is exactly one implementation
of each action.

**Files:**
- Create: `crates/app/src/commands.rs`
- Modify: `crates/app/src/main.rs`, `crates/app/src/develop/mod.rs`

- [ ] **Step 1: Write the dispatcher**

Create `crates/app/src/commands.rs`:

```rust
//! The one place a `Command` is performed.
//!
//! ⚠ Three input sources feed this — menu clicks, key presses and the rail — and each has a
//! different reason to exist. Routing them all through one exhaustive `match` is what stops a
//! menu item and its shortcut drifting apart, which is precisely what happened with the old
//! Copy button and Ctrl+C before they were made to share a code path.

use crate::{modals::Modal, AppView, RustroomApp};
use edit_model::EditGroups;
use eframe::egui;
use rustroom_app::menu::Command;

pub fn execute(app: &mut RustroomApp, ctx: &egui::Context, cmd: Command) {
    match cmd {
        Command::OpenFile => app.open_dialog(ctx),
        Command::OpenFolder => app.open_folder(ctx),
        Command::Export => {
            if !app.export_jobs().is_empty() {
                // Reset the working copy from the committed value, so a previously cancelled
                // edit does not linger.
                app.export_draft = app.settings.export.clone();
                app.modal = Modal::ExportSettings;
            }
        }
        Command::Settings => app.modal = Modal::Settings,
        Command::Undo => app.undo_action(),
        Command::Redo => app.redo_action(),
        Command::CopySettings => app.copy_settings(ctx),
        Command::PasteSettings => app.paste_last_groups(),
        Command::PasteSettingsWith => {
            if app.clipboard.is_some() {
                app.modal = Modal::PasteSettings;
            }
        }
        Command::SyncSelection => {
            let selected = app.library.as_ref().map_or(0, |l| l.selection.len());
            if selected > 1 {
                app.modal = Modal::SyncSettings;
            }
        }
        Command::ResetAdjustments => app.reset_adjustments(),
        Command::ShowLibrary => app.view = AppView::Library,
        Command::ShowDevelop => app.view = AppView::Develop,
        Command::FitView => {
            if let Some(s) = &mut app.session {
                s.zoom = 1.0;
                s.pan = egui::Vec2::ZERO;
            }
        }
        Command::ToggleBeforeAfter => {
            if let Some(s) = &mut app.session {
                s.show_before = !s.show_before;
            }
        }
        Command::ToggleFilmstrip => app.show_filmstrip = !app.show_filmstrip,
        Command::ToggleMaskOverlay => app.show_mask_tint = !app.show_mask_tint,
        Command::SelectTool(t) => app.select_tool(t),
        Command::RotateLeft => app.rotate(false),
        Command::RotateRight => app.rotate(true),
        Command::SavePreset => {
            app.modal = Modal::SavePreset {
                name: String::new(),
                groups: EditGroups::default(),
            }
        }
        Command::KeyboardShortcuts => app.modal = Modal::KeyboardShortcuts,
        Command::About => app.modal = Modal::About,
    }
}
```

⚠ **No catch-all arm.** A new `Command` must fail to compile here rather than becoming a menu item
that silently does nothing — that is the whole reason this enum and this match exist.

⚠ `Command::SelectTool` delegates to `app.select_tool`, which owns the re-render decision. Doing it
in this arm instead would leave the rail's own path to duplicate it.

- [ ] **Step 2: Add the `RustroomApp` methods the dispatcher calls**

Several arms above call methods that do not exist yet, because the logic currently lives inline in
the toolbar. Extract each into `main.rs` **without changing its behaviour** — move the body, do not
retype it. Sources, against the tree at the start of this task:

- `copy_settings(&mut self, ctx: &egui::Context)` — from `develop/mod.rs`, the `if ui.button("Copy")`
  body (clipboard field, `ctx.copy_text(edit_clipboard::encode(..))`, status line). ⚠ `main.rs`'s
  `handle_clipboard_keys` (~line 917) duplicates it today; make that call this method so the two
  cannot diverge. Note the existing `self.session.as_ref().map(..)` spelling is load-bearing — the
  `if let Some(s) = &self.session` form holds an immutable borrow across the assignment and will not
  compile.
- `paste_last_groups(&mut self)` — the Ctrl+V path from `handle_clipboard_keys`: apply the clipboard
  with `self.paste_groups` to the selection.
- `reset_adjustments(&mut self)` — from `develop/panel/mod.rs`, the `Reset adjustments` button body:
  `s.edits.merge_from(&Edits::default(), adjustments::edit_groups())`, plus the render request and
  history push the surrounding `Edited` normally performs. ⚠ Then make that button call this, so the
  menu item and the button are one implementation rather than two that drift.
- `select_tool(&mut self, t: Tool)` — sets `self.tool` and, when it changed, issues
  `s.request(preview_edits(&s.edits, tool), s.render_max_dim(), s.requested_region)`. ⚠ This is the
  `let before = app.tool; … if app.tool != before` comparison currently inline in `develop::body`;
  move it here and have `body` call it after `rail::show`, or the menu's 1–4 will switch tools
  **without re-rendering** — the M3.1a `draw_mask_overlay` bug in a new place.
- `rotate(&mut self, clockwise: bool)` — the ⟲ / ⟳ bodies from `develop/panel/geometry.rs`,
  including the matching `CropRect` transform (`rotate_ccw` plus three `rotated_cw`, or `rotate_cw`
  plus one). ⚠ Keep the crop transform with the orientation change in the same function; separating
  them is exactly how a rotated crop lands on different pixels.

- [ ] **Step 3: Add the two new modals**

`Modal::KeyboardShortcuts` and `Modal::About` in `crates/app/src/modals/mod.rs`, each a plain
`egui::Modal` with a `theme::button_row(ui, "", false, "Close")`-style dismiss. The shortcuts dialog
lists the model:

```rust
for section in rustroom_app::menu::model() {
    ui.heading(section.title);
    for item in section.items {
        if let rustroom_app::menu::Item::Action { label, shortcut: Some(sc), .. } = item {
            ui.horizontal(|ui| {
                ui.label(label);
                theme::hint(ui, &sc.label());
            });
        }
    }
}
```

⚠ Generated from `model()`, never hand-written: a shortcuts dialog that lists a chord the app does
not have is worse than no dialog.

- [ ] **Step 4: Verify and commit**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -10
cargo test --all 2>&1 | grep -oE "^test result: ok\. [0-9]+" | awk '{s+=$4} END {print "TOTAL:", s}'
```

Expected: clean; **694** unchanged — this task moves logic, it does not add behaviour.

```bash
git add crates/app/src/
git commit -m "refactor(app): every action goes through one command dispatcher"
```

---

### Task 3: Keyboard shortcuts route through the dispatcher

**Files:**
- Modify: `crates/app/src/shell.rs`, `crates/app/src/main.rs`, `crates/app/src/develop/mod.rs`

- [ ] **Step 1: One key handler, driven by the model**

Add to `shell.rs`:

```rust
/// Turn this frame's key presses into commands, using the menu model as the single source of
/// truth for which chord means what.
///
/// ⚠ Gated on nothing having keyboard focus — the rule arrow navigation and the rail already
/// follow. Without it, typing "d" into the Save-preset name field jumps to Develop.
///
/// ⚠ Not used for ⌘C / ⌘V. `egui-winit` matches the clipboard chords first and pushes
/// `Event::Copy` / `Event::Paste` *instead of* a `Key` event, so `key_pressed(Key::C)` can
/// never fire — a trap this codebase has already paid for once. Those stay in
/// `handle_clipboard_keys`.
fn pressed_command(ctx: &egui::Context) -> Option<Command> {
    if ctx.memory(|m| m.focused()).is_some() {
        return None;
    }
    for section in menu::model() {
        for item in section.items {
            let menu::Item::Action { command, shortcut: Some(sc), .. } = item else {
                continue;
            };
            if matches!(sc.key, 'C' | 'V') && sc.cmd {
                continue;
            }
            let Some(key) = egui_key(sc.key) else { continue };
            let hit = ctx.input(|i| {
                i.key_pressed(key)
                    && i.modifiers.command == sc.cmd
                    && i.modifiers.shift == sc.shift
            });
            if hit {
                return Some(command);
            }
        }
    }
    None
}

/// The model's key character as an egui key.
///
/// ⚠ Returns `Option` rather than panicking: an unmappable chord should make its menu item
/// mouse-only, not take the app down. `every_shortcut_maps_to_an_egui_key` makes sure none of
/// the shipped ones actually do that.
fn egui_key(c: char) -> Option<egui::Key> {
    Some(match c {
        'O' => egui::Key::O,
        'E' => egui::Key::E,
        'Z' => egui::Key::Z,
        'G' => egui::Key::G,
        'D' => egui::Key::D,
        '0' => egui::Key::Num0,
        '1' => egui::Key::Num1,
        '2' => egui::Key::Num2,
        '3' => egui::Key::Num3,
        '4' => egui::Key::Num4,
        ',' => egui::Key::Comma,
        '[' => egui::Key::OpenBracket,
        ']' => egui::Key::CloseBracket,
        '\\' => egui::Key::Backslash,
        _ => return None,
    })
}
```

Call it once per frame in `ui()`, before the view dispatch:

```rust
        if let Some(cmd) = pressed_command(&ctx) {
            commands::execute(self, &ctx, cmd);
        }
```

- [ ] **Step 2: Delete the shortcut handling this replaces**

- `develop::toolbar`'s `ctx.input(|i| { let z = i.key_pressed(Key::Z) …})` block — gone, along with
  the `undo_click || kundo` folding; the buttons now just call `commands::execute`.
- `develop::body`'s 1–4 block from step 2 of the rail plan — gone.
- `handle_clipboard_keys` **stays**, for the reason in the doc comment above, but its two bodies now
  call `copy_settings` / `paste_last_groups`.

- [ ] **Step 3: Add the mapping test**

In `shell.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shortcut_maps_to_an_egui_key() {
        // ⚠ A chord in the model with no egui key is a menu item whose shortcut silently does
        // nothing on Windows and Linux — invisible on macOS, where the native menu handles it.
        for section in menu::model() {
            for item in section.items {
                if let menu::Item::Action { label, shortcut: Some(sc), .. } = item {
                    assert!(
                        egui_key(sc.key).is_some(),
                        "{label}'s shortcut key {:?} has no egui mapping",
                        sc.key
                    );
                }
            }
        }
    }
}
```

- [ ] **Step 4: Sabotage**

Remove the `'G' => egui::Key::G,` arm.

```bash
cargo test -p app --bin rustroom-gui --no-run 2>&1 | grep -cE "^error" || echo "0 compile errors"
cargo test -p app --bin rustroom-gui every_shortcut_maps 2>&1 | tail -8
```

Expected: zero compile errors, then FAIL naming Library. Restore and `touch`.

- [ ] **Step 5: Verify and commit**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -5
cargo test --all 2>&1 | grep -oE "^test result: ok\. [0-9]+" | awk '{s+=$4} END {print "TOTAL:", s}'
```

Expected: clean; **695**.

```bash
git add crates/app/src/
git commit -m "refactor(app): shortcuts come from the menu model"
```

---

### Task 4: The in-window menu bar

**Files:**
- Create: `crates/app/src/menu_bar.rs`
- Modify: `crates/app/src/shell.rs`, `crates/app/src/main.rs`

- [ ] **Step 1: Write the renderer**

```rust
//! The in-window menu bar, drawn with egui.
//!
//! ⚠ **Compiled on every platform, invoked only where there is no native menu.** Gating the
//! compilation would mean the developer (on macOS) never builds or tests it and Linux CI never
//! builds the native one — both halves unverified by whoever is looking. Only the call is
//! platform-dependent; see `shell::ui`.

use eframe::egui;
use rustroom_app::menu::{self, Command, Item};

/// Draw the bar. Returns the command the user picked, if any.
pub fn show(ui: &mut egui::Ui) -> Option<Command> {
    let mut picked = None;
    egui::containers::menu::MenuBar::new().ui(ui, |ui| {
        for section in menu::model() {
            ui.menu_button(section.title, |ui| {
                for item in section.items {
                    match item {
                        Item::Separator => {
                            ui.separator();
                        }
                        Item::Action { label, command, shortcut } => {
                            // ⚠ The chord is shown as part of the label rather than as egui
                            // shortcut text, so the in-window bar and the native one read the
                            // same way — the native bar cannot use a real accelerator for the
                            // unsafe chords either. One presentation, both platforms.
                            let text = match shortcut {
                                Some(sc) => format!("{label}\t{}", sc.label()),
                                None => label.to_string(),
                            };
                            if ui.button(text).clicked() {
                                picked = Some(command);
                                ui.close();
                            }
                        }
                    }
                }
            });
        }
    });
    picked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bar_renders_every_section_without_panicking() {
        // ⚠ Thin but not worthless: this is the only thing that executes the renderer at all,
        // and egui panics on a duplicate widget id or a malformed menu — neither of which the
        // model tests can see.
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            let picked = show(ui);
            assert!(picked.is_none(), "nothing was clicked");
        });
    }
}
```

⚠ `MenuBar::new().ui(..)` and `ui.close()` are egui 0.35 spellings — if either does not compile,
check `egui-0.35.0/src/containers/menu.rs` rather than guessing; that file is the authority.

- [ ] **Step 2: Invoke it**

In `shell.rs`, inside the existing `egui::Panel::top("top")`, above the current toolbar row:

```rust
            // ⚠ macOS gets the real system menu bar instead; drawing this one too would put a
            // redundant row under a nearly-empty system menu.
            if !cfg!(target_os = "macos") {
                if let Some(cmd) = menu_bar::show(ui) {
                    pending_command = Some(cmd);
                }
            }
```

⚠ `pending_command` is a local applied **after** the panel closure closes — `commands::execute`
takes `&mut self` and the closure already holds it. This is the same deferred-action shape
`preset_panel` uses.

- [ ] **Step 3: Verify and commit**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -5
cargo test --all 2>&1 | grep -oE "^test result: ok\. [0-9]+" | awk '{s+=$4} END {print "TOTAL:", s}'
```

Expected: clean; **696**.

```bash
git add crates/app/src/
git commit -m "feat(app): an in-window menu bar"
```

---

### Task 5: The native macOS menu

**Files:**
- Create: `crates/app/src/native_menu.rs`
- Modify: `crates/app/Cargo.toml`, `crates/app/src/main.rs`, `crates/app/src/shell.rs`

- [ ] **Step 1: Add the dependency, macOS only**

In `crates/app/Cargo.toml`:

```toml
[target.'cfg(target_os = "macos")'.dependencies]
muda = "0.19"
```

⚠ A target-specific dependency, so Linux CI does not build muda at all — which is the point, since
its Linux backend needs GTK that eframe/winit does not provide.

- [ ] **Step 2: Write the native menu**

```rust
//! The macOS system menu bar, built from the same model the in-window bar uses.
//!
//! ⚠ **The only `#[cfg]`-gated module in the app, and deliberately thin.** Linux CI never
//! compiles this, so every line here is unverified by CI — keep the logic in `menu.rs` and
//! `commands.rs`, which are built everywhere.

use eframe::egui;
use muda::{accelerator::Accelerator, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use rustroom_app::menu::{self, Command, Item};
use std::cell::RefCell;
use std::collections::HashMap;

thread_local! {
    /// The live menu and its id → command map.
    ///
    /// ⚠ Thread-local rather than a field on `RustroomApp`, for a concrete reason: muda's
    /// types are `!Send`/`!Sync` and macOS menus panic if touched off the main thread. Keeping
    /// them here means `RustroomApp` stays plain and no `#[cfg]` leaks into its definition.
    static MENU: RefCell<Option<Installed>> = const { RefCell::new(None) };
}

struct Installed {
    _menu: Menu,
    by_id: HashMap<MenuId, Command>,
}

/// Build and install the menu. Call once, from the eframe creation callback.
pub fn install(ctx: &egui::Context) {
    let menu = Menu::new();
    let mut by_id = HashMap::new();

    for section in menu::model() {
        let sub = Submenu::new(section.title, true);
        for item in section.items {
            match item {
                Item::Separator => {
                    let _ = sub.append(&PredefinedMenuItem::separator());
                }
                Item::Action { label, command, shortcut } => {
                    // ⚠ Only *safe* chords become real accelerators. A bare key would steal
                    // every such character typed into a text field, and ⌘C/⌘V/⌘Z would break
                    // editing inside one — see `Shortcut::safe_as_native_accelerator`. The
                    // rest are shown in the label and handled by egui, focus-gated.
                    let (text, accel) = match shortcut {
                        Some(sc) if sc.safe_as_native_accelerator() => {
                            (label.to_string(), accelerator(sc))
                        }
                        Some(sc) => (format!("{label}   {}", sc.label()), None),
                        None => (label.to_string(), None),
                    };
                    let mi = MenuItem::new(text, true, accel);
                    by_id.insert(mi.id().clone(), command);
                    let _ = sub.append(&mi);
                }
            }
        }
        let _ = menu.append(&sub);
    }

    menu.init_for_nsapp();

    // ⚠ egui does not redraw an idle window, and a menu click does not generate a winit event
    // — so without an explicit repaint the command would sit in the channel until the user
    // happened to move the mouse. Same class of bug as the full-res trigger needing
    // `request_repaint_after`.
    let ctx = ctx.clone();
    MenuEvent::set_event_handler(Some(move |_event: MenuEvent| {
        ctx.request_repaint();
    }));

    MENU.with(|m| *m.borrow_mut() = Some(Installed { _menu: menu, by_id }));
}

/// Drain any menu activations since the last frame.
pub fn poll() -> Vec<Command> {
    let mut out = Vec::new();
    while let Ok(event) = MenuEvent::receiver().try_recv() {
        MENU.with(|m| {
            if let Some(installed) = m.borrow().as_ref() {
                if let Some(cmd) = installed.by_id.get(&event.id) {
                    out.push(*cmd);
                }
            }
        });
    }
    out
}

fn accelerator(sc: menu::Shortcut) -> Option<Accelerator> {
    use muda::accelerator::{Code, Modifiers};
    let code = match sc.key {
        'O' => Code::KeyO,
        'E' => Code::KeyE,
        ',' => Code::Comma,
        '[' => Code::BracketLeft,
        ']' => Code::BracketRight,
        _ => return None,
    };
    let mut mods = Modifiers::empty();
    if sc.cmd {
        mods |= Modifiers::CMD_OR_CTRL;
    }
    if sc.shift {
        mods |= Modifiers::SHIFT;
    }
    Some(Accelerator::new(Some(mods), code))
}
```

⚠ **`set_event_handler` replaces the default channel delivery in some muda versions.** If `poll()`
returns nothing after this is wired up, the handler is swallowing events — send them through your own
`std::sync::mpsc` channel from inside the handler instead of calling `try_recv` here, and keep the
`request_repaint`. Check by clicking *Open file…* in the native menu; if the dialog does not appear,
this is why.

- [ ] **Step 3: Install and poll**

In `main.rs`'s `RustroomApp::new(ctx)`, at the end:

```rust
        #[cfg(target_os = "macos")]
        crate::native_menu::install(ctx);
```

In `shell.rs`'s `ui()`, near the other pumps:

```rust
        #[cfg(target_os = "macos")]
        for cmd in native_menu::poll() {
            commands::execute(self, &ctx, cmd);
        }
```

- [ ] **Step 4: Verify, then check the things only a running app can show**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -5
cargo test --all 2>&1 | grep -oE "^test result: ok\. [0-9]+" | awk '{s+=$4} END {print "TOTAL:", s}'
cargo run --release -p app
```

1. The macOS menu bar shows **Rustroom · File · Edit · View · Photo · Help**, and there is **no
   in-window menu row**.
2. *File → Open file…* opens the dialog **on the first click**, without needing to jiggle the mouse.
   If it needs a mouse move, the repaint handler is not wired — see the warning in Step 2.
3. ⌘O, ⌘E, ⌘, all work as native accelerators.
4. ⌘Z, ⌘C, ⌘V still work **and** still type normally inside the Save-preset name field. That is the
   whole point of `safe_as_native_accelerator`; if a chord misbehaves here, the rule is wrong, not
   the field.
5. Typing "2" or "d" into the Save-preset name field does **not** switch tool or view.

- [ ] **Step 5: Commit**

```bash
git add crates/app/
git commit -m "feat(app): a native menu bar on macOS"
```

---

### Task 6: The toolbar slims to six

**Files:**
- Modify: `crates/app/src/develop/mod.rs`, `crates/app/src/shell.rs`

- [ ] **Step 1: Cut it down**

`develop::toolbar` keeps only **Undo · Redo │ Fit · Before/After** — Library/Develop already live in
`shell.rs`'s row. Delete the Copy, Paste…, Sync…, Export…, Settings… and Filmstrip controls and the
pan hint; every one is now a menu item. Each remaining button calls `commands::execute`.

⚠ **Keep the Undo label's "Undo delete '<name>'" wording.** It is what stops Ctrl+Z silently doing
the surprising one, and it is the only place that distinction is visible.

⚠ **`Export…` and `Sync…` lose their dynamic counts**, which the spec records as a knowing
regression: a menu item cannot carry a label that changes usefully. The count moves into the dialogs,
which already name their targets.

- [ ] **Step 2: Move the filename to the right of the toolbar**

The develop panel's footer already shows filename and camera model. Add the filename to the toolbar's
right edge with `ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), ..)`, so the row
that used to be crowded now carries the one piece of context worth always seeing.

- [ ] **Step 3: Verify and commit**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -5
cargo test --all 2>&1 | grep -oE "^test result: ok\. [0-9]+" | awk '{s+=$4} END {print "TOTAL:", s}'
```

Expected: clean; **696**.

```bash
git add crates/app/src/
git commit -m "feat(app): the develop toolbar is six controls"
```

---

### Task 7: Look at it, then record it

- [ ] **Step 1: Full green**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo fmt --all --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all 2>&1 | tail -5
```

Expected: green, **696**, and **no golden moved**.

- [ ] **Step 2: Check the whole rework end to end**

```bash
cargo run --release -p app
```

Every menu item does what it says; every shortcut in Help → Keyboard shortcuts works; the toolbar is
six controls; the rail still switches tools; Crop, Masks and Presets all still work.

⚠ **Also check the in-window bar**, which macOS never shows. There is no Linux machine here, so the
honest check is the unit test plus a temporary flip of the `!cfg!(target_os = "macos")` guard to
`true` — look at it once, then put it back. Note in CLAUDE.md that it has never run on Linux.

- [ ] **Step 3: Update CLAUDE.md**

Record: the model/dispatcher seam; **`safe_as_native_accelerator` and why bare keys and ⌘C/⌘V/⌘Z are
excluded**; that the egui bar compiles everywhere and only its invocation is gated; the muda
thread-local and the repaint handler; the toolbar reaching six; the dynamic-count regression; and
that **the layout/IA rework is complete** — all three steps.

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: record the menu bar, and close the layout rework"
```

---

## Self-review notes

Checked against the spec's **menu bar** sections:

- One model, two renderers → Tasks 1, 4, 5. ✅
- Native on macOS via `init_for_nsapp` → Task 5. ✅
- `egui::MenuBar` elsewhere → Task 4. ✅
- The five menus with the listed contents → Task 1. ✅
- Photo menu numbering the rail 1–4 → Task 1, `every_tool_is_reachable_from_the_photo_menu`. ✅
- `Command` dispatcher with an exhaustive match → Task 2. ✅
- Existing ad-hoc key handling refactored to emit Commands → Tasks 2, 3. ✅
- Toolbar to six controls → Task 6. ✅
- `Reset all` into the Edit menu → Task 2 (`Command::ResetAdjustments`). ✅
- Dynamic counts lost → Task 6, as the spec records. ✅

**Where this plan overrides the spec, and why.** The spec's "disable Copy/Paste while a text field has
focus" is **not built**: `safe_as_native_accelerator` removes the conflict at the source instead, which
is less machinery and cannot be forgotten on a frame. The spec's implied `#[cfg]`-ing of the egui
renderer is **not done**, because it would leave whichever renderer the developer is not on
uncompiled and untested.

**Known risk, stated rather than hidden.** `native_menu.rs` is never compiled by Linux CI, so it can
rot silently. Mitigations: it is thin, all decisions live in `menu.rs`/`commands.rs` which are built
everywhere, and Task 7 records the gap in CLAUDE.md.
