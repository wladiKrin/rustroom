# Tool rail Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the Geometry and Presets toolbar toggles with a four-entry tool rail on the right edge — Edit, Crop, Masks, Presets — whose selection opens a drawer above the permanent adjustment stack, and make `geometry_mode` a derived property of the selected tool rather than a stored field.

**Architecture:** `Tool` is a plain enum in the **lib** (`rustroom_app::tool`), egui-free and unit-tested there. `develop/rail.rs` renders it as an outermost `Panel::right`. The drawers already exist as files from step 1 (`panel/masks.rs`, `panel/geometry.rs`); this plan adds `panel/presets.rs` and puts all three behind one `match app.tool` in `panel/mod.rs`. `Session::geometry_mode` is **deleted**; every reader derives it from `app.tool == Tool::Crop`.

**Tech Stack:** Rust, egui 0.35 (`Panel::right`, `selectable_label`, `FontId`, `Context::fonts_mut`), headless `Context::run_ui` + `RawInput` for interaction tests.

**Spec:** `docs/superpowers/specs/2026-08-23-layout-ia-rework-design.md`. This is **step 2 of 3**; step 1 (the panel) landed 2026-08-24, step 3 (the menu bar) is separate.

---

## ⚠⚠ Read first: the spec's glyphs are wrong, and this is now settled by measurement

The spec's mockup used **▦ ⧉ ◐ ≡**. A probe against egui 0.35's bundled fonts
(Hack, Ubuntu-Light, NotoEmoji, `emoji-icon-font`) found that **all four fail to render** and would
have shipped as tofu boxes. This is precisely the risk flagged when the glyph option was chosen, and
it was real.

Measured, `has_glyph` returns **true** for: `☂ ★ ☆ ☰ ◑ ◔ ◕ ■ ▣ ○ ◎ ⌘ ✚ ⚙` — and **false** for
`▦ ⧉ ◐ ≡ ⌗ ⬚ ● ✎ ✂ ◧ ▤`.

**The chosen set, all four verified present and visually distinct:**

| tool | glyph | codepoint | why |
|---|---|---|---|
| Edit | `☰` | U+2630 | a stack of horizontal lines — the adjustment sliders |
| Crop | `▣` | U+25A3 | a frame within a frame |
| Masks | `◑` | U+25D1 | half-covered — partial coverage is exactly what a mask is |
| Presets | `★` | U+2605 | a saved, recallable thing |

⚠ **Do not substitute a glyph without re-running the coverage test in Task 2.** The four above are
the only reason choice B is safe, and "it looks fine in my terminal" proves nothing — the terminal
uses system fonts, egui uses its own bundled ones.

---

## File Structure

| file | responsibility |
|---|---|
| `crates/app/src/tool.rs` | **Create.** `Tool` enum: `ALL`, `label`, `glyph`, `drawer_label`, `is_geometry`, `from_digit`. Lib module, egui-free. |
| `crates/app/src/develop/rail.rs` | **Create.** Renders the rail as an outermost `Panel::right`; owns the glyph-coverage test. |
| `crates/app/src/develop/panel/presets.rs` | **Create.** The Presets drawer, moved out of `develop/mod.rs`'s left panel. |
| `crates/app/src/develop/panel/mod.rs` | Gains the `match app.tool` drawer dispatch. |
| `crates/app/src/develop/mod.rs` | Loses `preset_panel` and the Geometry/Presets toolbar toggles; `body` gains the rail in the correct panel order. |
| `crates/app/src/develop/canvas.rs` | Overlay choice becomes a `match` on the tool rather than `if s.geometry_mode`. |
| `crates/app/src/main.rs` | `RustroomApp` gains `tool`, loses `show_presets`; `Session` loses `geometry_mode`; `preview_edits` takes a `Tool`. |
| `crates/app/src/lib.rs` | Add `pub mod tool;`. |

---

### Task 1: `Tool` in the lib

⚠ **Unlike step 1's `HslChannel`, this can be committed on its own.** `tool.rs` is a **lib** module,
and a `pub` item in a lib is public API — `dead_code` does not fire. That is the exact contrast with
the binary-crate trap that forced step 1's Tasks 3 and 4 to merge.

**Files:**
- Create: `crates/app/src/tool.rs`
- Modify: `crates/app/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/app/src/tool.rs`:

```rust
//! Which Develop tool the rail has selected.
//!
//! ⚠ Lives on `RustroomApp`, not on `Session`, so the selection **survives a photo switch** —
//! today's `geometry_mode` resets when you arrow to the next photo, and with a rail it should
//! not. Lightroom does not reset it either.
//!
//! ⚠ No egui here: `lib.rs`'s rule is what keeps these units testable without a windowing
//! system. The rendering lives in the bin, in `develop/rail.rs`.

/// A Develop tool. `Edit` is the default and opens no drawer.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Tool {
    #[default]
    Edit,
    Crop,
    Masks,
    Presets,
}

impl Tool {
    /// Every tool, in rail order. The rail iterates this, so a variant missing from it is
    /// unreachable in the UI while compiling perfectly.
    pub const ALL: [Tool; 4] = [Self::Edit, Self::Crop, Self::Masks, Self::Presets];

    /// The rail's text label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Edit => "Edit",
            Self::Crop => "Crop",
            Self::Masks => "Masks",
            Self::Presets => "Presets",
        }
    }

    /// The rail's glyph.
    ///
    /// ⚠ **Every one of these is verified present in egui's bundled fonts** by
    /// `every_rail_glyph_is_renderable` in `develop/rail.rs`. The obvious choices — ▦ for a
    /// grid, ⧉ for a crop, ◐ for a half-covered disc, ≡ for a list — are all **absent** and
    /// render as tofu boxes. Do not substitute one without re-running that test.
    pub fn glyph(self) -> char {
        match self {
            Self::Edit => '☰',
            Self::Crop => '▣',
            Self::Masks => '◑',
            Self::Presets => '★',
        }
    }

    /// The header of this tool's drawer, or `None` when it opens none.
    ///
    /// ⚠ `Edit` has no drawer *by design*: it is the resting state, where the panel is exactly
    /// the five adjustment sections and nothing else.
    pub fn drawer_label(self) -> Option<&'static str> {
        match self {
            Self::Edit => None,
            Self::Crop => Some("Crop & Geometry"),
            Self::Masks => Some("Masks"),
            Self::Presets => Some("Presets"),
        }
    }

    /// Whether this tool puts the canvas in geometry mode.
    ///
    /// ⚠ This **replaces the stored `Session::geometry_mode` field**. Two facts that could
    /// disagree became one — the same correction `render_max_dim` needed when it was a field
    /// beside `requested_region`.
    pub fn is_geometry(self) -> bool {
        matches!(self, Self::Crop)
    }

    /// The tool for a 1–4 keypress, if any.
    pub fn from_digit(d: u32) -> Option<Tool> {
        Self::ALL.get(d.checked_sub(1)? as usize).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_is_the_default_and_the_only_tool_without_a_drawer() {
        // ⚠ Both halves matter. Edit being default is what makes the panel at rest just the
        // five adjustment sections; Edit being the *only* drawerless tool is what stops a
        // future tool silently opening nothing.
        assert_eq!(Tool::default(), Tool::Edit);
        let drawerless: Vec<Tool> = Tool::ALL
            .into_iter()
            .filter(|t| t.drawer_label().is_none())
            .collect();
        assert_eq!(drawerless, vec![Tool::Edit]);
    }

    #[test]
    fn only_crop_is_a_geometry_tool() {
        // ⚠ Pins the derived property that replaced the stored field. Stated over ALL rather
        // than as `assert!(Tool::Crop.is_geometry())`, so a second tool accidentally
        // returning true cannot pass.
        let geo: Vec<Tool> = Tool::ALL.into_iter().filter(|t| t.is_geometry()).collect();
        assert_eq!(geo, vec![Tool::Crop]);
    }

    #[test]
    fn labels_and_glyphs_are_all_distinct() {
        // Two tools sharing either would give the rail two identical-looking entries.
        let mut labels: Vec<&str> = Tool::ALL.iter().map(|t| t.label()).collect();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), 4, "two tools share a label");
        let mut glyphs: Vec<char> = Tool::ALL.iter().map(|t| t.glyph()).collect();
        glyphs.sort_unstable();
        glyphs.dedup();
        assert_eq!(glyphs.len(), 4, "two tools share a glyph");
    }

    #[test]
    fn the_digits_select_the_rail_in_order() {
        // ⚠ 1-based, matching the labels a user sees. Off-by-one here means every shortcut
        // picks the neighbouring tool — plausible-looking and completely wrong.
        assert_eq!(Tool::from_digit(1), Some(Tool::Edit));
        assert_eq!(Tool::from_digit(2), Some(Tool::Crop));
        assert_eq!(Tool::from_digit(3), Some(Tool::Masks));
        assert_eq!(Tool::from_digit(4), Some(Tool::Presets));
        assert_eq!(Tool::from_digit(0), None, "there is no tool 0");
        assert_eq!(Tool::from_digit(5), None);
    }
}
```

Add to `crates/app/src/lib.rs`, in alphabetical position (after `pub mod thumb_loader;`):

```rust
pub mod tool;
```

- [ ] **Step 2: Run the tests**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo test -p app --lib tool:: 2>&1 | tail -12
```

Expected: 4 PASS.

- [ ] **Step 3: Sabotage — make Masks a geometry tool too**

```rust
    pub fn is_geometry(self) -> bool {
        matches!(self, Self::Crop | Self::Masks)
    }
```

```bash
cargo test -p app --lib --no-run 2>&1 | grep -cE "^error" || echo "0 compile errors"
cargo test -p app --lib only_crop_is_a_geometry_tool 2>&1 | tail -8
```

Expected: **zero compile errors first**, then FAIL. Restore, then `touch crates/app/src/tool.rs`.

- [ ] **Step 4: Verify and commit**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -5
cargo test --all 2>&1 | grep -oE "^test result: ok\. [0-9]+" | awk '{s+=$4} END {print "TOTAL:", s}'
```

Expected: clean; **676** (672 + 4).

```bash
git add crates/app/src/tool.rs crates/app/src/lib.rs
git commit -m "feat(app): a Tool enum for the develop rail"
```

---

### Task 2: The rail widget, and the glyph guarantee

**Files:**
- Create: `crates/app/src/develop/rail.rs`
- Modify: `crates/app/src/develop/mod.rs`
- Modify: `crates/app/src/main.rs`

- [ ] **Step 1: Add the `tool` field to `RustroomApp`**

In `crates/app/src/main.rs`, beside `hsl_channel` (added in step 1 of this rework):

```rust
    /// Which Develop tool the rail has selected.
    ///
    /// ⚠ On the app, not on `Session`, so it **survives a photo switch** — see `tool::Tool`.
    tool: rustroom_app::tool::Tool,
```

and in the initialiser, beside `hsl_channel: Default::default(),`:

```rust
            tool: Default::default(),
```

- [ ] **Step 2: Write `develop/rail.rs`**

```rust
//! The Develop tool rail: Edit, Crop, Masks, Presets.
//!
//! ⚠ **Outermost on the right.** egui allocates side panels in call order, so this must be
//! shown *before* the adjustments panel or it lands between the panel and the canvas. The
//! ordering rule is documented on `develop::body` and is load-bearing.

use eframe::egui;
use rustroom_app::tool::Tool;

/// Rail width in points. Wide enough for "Presets" at the small text size.
const RAIL_WIDTH: f32 = 64.0;

/// Draw the rail.
///
/// ⚠ Returns nothing on purpose. `body` detects a tool change by comparing against the
/// previous value, which covers the keyboard shortcuts too — a return value here would catch
/// only clicks, and the 1–4 keys would then switch tools without re-rendering.
pub fn show(ui: &mut egui::Ui, tool: &mut Tool) {
    egui::Panel::right("rail")
        .resizable(false)
        .default_size(RAIL_WIDTH)
        .show(ui, |ui| {
            ui.vertical_centered_justified(|ui| {
                for t in Tool::ALL {
                    let picked = *tool == t;
                    let text = format!("{}\n{}", t.glyph(), t.label());
                    if ui.selectable_label(picked, text).clicked() {
                        *tool = t;
                    }
                }
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_rail_glyph_is_renderable() {
        // ⚠⚠ The test that makes glyphs safe at all, and it is not hypothetical: the design
        // spec's own candidates — ▦ ⧉ ◐ ≡ — are ALL absent from egui's bundled fonts and
        // would have shipped as tofu boxes. egui bundles Hack, Ubuntu-Light, NotoEmoji and
        // emoji-icon-font; nothing guarantees an arbitrary Geometric-Shapes character is in
        // any of them, and a missing glyph is invisible to every other test in this suite.
        //
        // ⚠ A frame must run before the fonts exist — `Context::default()` has not built its
        // atlas yet, so asking a fresh context would answer about nothing.
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(egui::RawInput::default(), |_ui| {});
        let id = egui::FontId::proportional(14.0);
        for t in Tool::ALL {
            let c = t.glyph();
            assert!(
                ctx.fonts_mut(|f| f.has_glyph(&id, c)),
                "{:?}'s glyph {c} (U+{:04X}) is not in any bundled font and would render as \
                 a tofu box",
                t,
                c as u32
            );
        }
    }

    /// Click the rail entry for `want` and report the selection afterwards.
    ///
    /// Three passes: lay out with the pointer in place, press, release — egui resolves a
    /// click against the widgets registered on the *previous* pass. A single-pass harness
    /// reads zeros and passes vacuously, which is exactly how `a_left_drag_does_not_pan` was
    /// once inert.
    fn click_rail(want: Tool) -> Tool {
        let ctx = egui::Context::default();
        let mut tool = Tool::Edit;
        let mut target = egui::Pos2::ZERO;
        for pass in 0..3 {
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 400.0),
                )),
                ..Default::default()
            };
            input.events.push(egui::Event::PointerMoved(target));
            if pass == 1 || pass == 2 {
                input.events.push(egui::Event::PointerButton {
                    pos: target,
                    button: egui::PointerButton::Primary,
                    pressed: pass == 1,
                    modifiers: Default::default(),
                });
            }
            let _ = ctx.run_ui(input, |ui| {
                egui::Panel::right("rail").show(ui, |ui| {
                    ui.vertical_centered_justified(|ui| {
                        for t in Tool::ALL {
                            let picked = tool == t;
                            let text = format!("{}\n{}", t.glyph(), t.label());
                            let r = ui.selectable_label(picked, text);
                            if t == want && pass == 0 {
                                target = r.rect.center();
                            }
                            if r.clicked() && !picked {
                                tool = t;
                            }
                        }
                    });
                });
            });
        }
        tool
    }

    #[test]
    fn clicking_a_rail_entry_selects_that_tool() {
        // ⚠ Canvas and panel wiring is the area this suite does not cover — six bugs in the
        // brush work, every one there, none caught by ~30 tests on the pure functions beside
        // it. Every non-default tool is exercised, because a rail stuck on its default would
        // pass a test that only ever clicked Edit.
        assert_eq!(click_rail(Tool::Crop), Tool::Crop);
        assert_eq!(click_rail(Tool::Masks), Tool::Masks);
        assert_eq!(click_rail(Tool::Presets), Tool::Presets);
    }
}
```

⚠ The harness deliberately **re-declares** the rail body rather than calling `show`, because
`show` takes `&mut Tool` and the harness needs to observe the click on a specific pass. Keep the two
in step: if `show`'s widget changes from `selectable_label`, the harness must change with it, or the
test stops describing the shipped widget.

- [ ] **Step 3: Run the tests**

```bash
cargo test -p app --bin Rustroom rail:: 2>&1 | tail -12
```

Expected: 2 PASS. **If `every_rail_glyph_is_renderable` fails, do not change the test** — pick a
different character from the measured-present set (`☂ ★ ☆ ☰ ◑ ◔ ◕ ■ ▣ ○ ◎ ⌘ ✚ ⚙`) and re-run.

- [ ] **Step 4: Sabotage — substitute a known-absent glyph**

In `crates/app/src/tool.rs`, change Masks' glyph to the spec's original:

```rust
            Self::Masks => '◐',
```

```bash
cargo test -p app --bin Rustroom --no-run 2>&1 | grep -cE "^error" || echo "0 compile errors"
cargo test -p app --bin Rustroom every_rail_glyph_is_renderable 2>&1 | tail -8
```

Expected: zero compile errors, then FAIL naming `Masks` and `U+25D0`. Restore `'◑'`, then
`touch crates/app/src/tool.rs`.

- [ ] **Step 5: Show the rail, in the right panel order**

In `crates/app/src/develop/mod.rs`, add `pub mod rail;` beside `pub mod canvas;`, and in `body`
replace the preset-panel block with the rail — it must come **first**, before `panel::show`:

```rust
pub fn body(app: &mut RustroomApp, ui: &mut egui::Ui, ctx: &egui::Context) {
    app.handle_develop_arrows(ctx);
    let mut photo_edited = false;
    if app.session.is_some() {
        rail::show(ui, &mut app.tool);
    }
    photo_edited |= panel::show(app, ui);
    if app.show_filmstrip && app.library.is_some() {
        filmstrip::show(app, ui, ctx);
    }
    photo_edited |= canvas::show(app, ui, ctx);
```

⚠ Leave `preset_panel` and the `show_presets` toggle alone for now — Task 5 removes them. This
commit shows the rail while Presets still has its old left panel, which is briefly redundant and
entirely harmless.

Update `body`'s doc comment, which currently names the old order:

```rust
/// ⚠ **Panel order is load-bearing.** egui allocates side panels in call order and the central
/// panel takes what is left, so rail (right, outermost) → adjustments (right) → filmstrip
/// (bottom) → canvas (central) must be preserved exactly. The rail coming *first* is what puts
/// it against the window edge rather than between the panel and the image.
```

- [ ] **Step 6: Verify and commit**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -5
cargo test --all 2>&1 | grep -oE "^test result: ok\. [0-9]+" | awk '{s+=$4} END {print "TOTAL:", s}'
```

Expected: clean; **678** (676 + 2).

```bash
git add crates/app/src/develop/rail.rs crates/app/src/develop/mod.rs crates/app/src/main.rs
git commit -m "feat(app): a tool rail on the develop canvas"
```

---

### Task 3: `geometry_mode` becomes derived

Deletes the stored field. **Ten read sites**, all listed below — do not enumerate them from memory.

**Files:**
- Modify: `crates/app/src/main.rs` (field, initialiser, `preview_edits`, 4 call sites)
- Modify: `crates/app/src/develop/mod.rs` (the Geometry toggle)
- Modify: `crates/app/src/develop/canvas.rs` (5 sites)
- Modify: `crates/app/src/develop/panel/mod.rs` (2 sites)

- [ ] **Step 1: Change `preview_edits` to take a `Tool`**

In `main.rs`:

```rust
fn preview_edits(edits: &Edits, tool: Tool) -> Edits {
    let mut e = edits.clone();
    if tool.is_geometry() {
        e.geometry.crop = edit_model::CropRect::default();
    }
    e
}
```

⚠ Taking the `Tool` rather than a `bool` is the point: a bare `bool` at a call site is exactly the
kind of parameter that gets passed inverted with nothing to notice. Add `use rustroom_app::tool::Tool;`
to `main.rs`.

- [ ] **Step 2: Delete the field**

Remove `geometry_mode: bool,` from `Session` (`main.rs:272`) and `geometry_mode: false,` from its
initialiser (`main.rs:1008`).

- [ ] **Step 3: Fix every reader**

The compiler will name all of them. The rule at each site: **hoist the tool into a local before
borrowing the session**, because `self.tool` is an immutable borrow of `self` and `&mut self.session`
is a mutable one — reading it afterwards is `E0502`. `Tool` is `Copy`, so the hoist is free.

In `main.rs`, at each of the four sites (lines ~494, ~748, ~763, ~1094), the shape is:

```rust
        let tool = self.tool;
        if let Some(s) = &mut self.session {
            s.request(preview_edits(&s.edits, tool), /* unchanged args */);
        }
```

In `develop/canvas.rs`, hoist once at the top of `show`, before `s` is bound:

```rust
    let tool = app.tool;
```

then `s.geometry_mode` → `tool.is_geometry()` at lines ~119 and ~214, and
`preview_edits(&s.edits, s.geometry_mode)` → `preview_edits(&s.edits, tool)` at ~168.

⚠ The two sites at ~551 and ~579 are inside helper functions that receive `s` but not `app`. **Add a
`tool: Tool` parameter to each** rather than reaching for a global — the same explicit-parameter
move that dissolved step 1's borrow problems.

In `develop/panel/mod.rs`, `show` already has `app`; hoist `let tool = app.tool;` above the
`if let Some(s) = &mut app.session` and use `tool.is_geometry()` at ~97 and `tool` at ~104.

- [ ] **Step 4: Point the toolbar's Geometry toggle at the tool**

In `develop/mod.rs`'s `toolbar`, replace the toggle:

```rust
        if ui.toggle_value(&mut s.geometry_mode, "Geometry").clicked() {
```

⚠ This whole block is **deleted** in Task 4, when the rail owns the mode. For this commit, keep the
button working so the tree is never in a state where geometry is unreachable:

```rust
    }
    if app.session.is_some() {
        // ⚠ Temporary shim: the rail owns this from Task 4. It exists only so this commit
        // leaves geometry mode reachable.
        let mut geo = app.tool.is_geometry();
        if ui.toggle_value(&mut geo, "Geometry").clicked() {
            app.tool = if geo { Tool::Crop } else { Tool::Edit };
        }
    }
```

Place it after the `if let Some(s) = &mut app.session { .. }` block that previously held it, and
remove the toggle from inside that block. ⚠ **Keep the render request** — `preview_edits` strips the
crop in geometry mode, so without it this commit would leave the crop overlay drawn over a cropped
preview. An intermediate commit may be redundant; it must not be broken:

```rust
        let mut geo = app.tool.is_geometry();
        if ui.toggle_value(&mut geo, "Geometry").clicked() {
            app.tool = if geo { Tool::Crop } else { Tool::Edit };
            let tool = app.tool;
            if let Some(s) = &mut app.session {
                s.request(
                    preview_edits(&s.edits, tool),
                    s.render_max_dim(),
                    s.requested_region,
                );
            }
        }

- [ ] **Step 5: Verify and commit**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -5
cargo test --all 2>&1 | grep -oE "^test result: ok\. [0-9]+" | awk '{s+=$4} END {print "TOTAL:", s}'
grep -rn "geometry_mode" crates/app/src/ | grep -v "^.*://" | grep -v "is_geometry"
```

Expected: clean; **678** unchanged; the grep returns **only doc-comment mentions**, no field access.

```bash
git add crates/app/src/
git commit -m "refactor(app): geometry mode is derived from the selected tool"
```

---

### Task 4: The drawers

**Files:**
- Modify: `crates/app/src/develop/panel/mod.rs`
- Modify: `crates/app/src/develop/mod.rs`

- [ ] **Step 1: Put the drawers behind the tool**

In `panel/mod.rs`'s `show`, the masks call currently runs unconditionally and the geometry block sits
lower down. Replace both with one dispatch, placed immediately after the histogram's `ui.separator()`
and **above** `adjustments::show`:

```rust
                    // The tool's drawer, above the permanent adjustment stack — the global
                    // sliders stay reachable while a mask is being painted. ⚠ `Tool::Edit`
                    // opens nothing, which is what makes the panel at rest exactly the five
                    // adjustment sections.
                    if let Some(title) = tool.drawer_label() {
                        egui::CollapsingHeader::new(title)
                            .default_open(true)
                            .show(ui, |ui| match tool {
                                Tool::Crop => geometry::show(ui, s, &mut ed),
                                Tool::Masks => masks::show(
                                    ui,
                                    s,
                                    masks::BrushTool {
                                        size: &mut app.brush_size,
                                        hardness: &mut app.brush_hardness,
                                        flow: &mut app.brush_flow,
                                        erase: &mut app.brush_erase,
                                        show_tint: &mut app.show_mask_tint,
                                    },
                                    &mut ed,
                                ),
                                Tool::Presets => {}
                                Tool::Edit => {}
                            });
                        ui.separator();
                    }
```

⚠ `Tool::Presets => {}` is a deliberate one-commit gap that Task 5 fills. It is a **match arm, not a
wildcard**, so adding a fifth tool is still a compile error.

Delete the old `if tool.is_geometry() { ui.separator(); geometry::show(..) }` block further down, and
the unconditional `masks::show(..)` call.

⚠ `masks::show` currently draws its own `CollapsingHeader::new("Masks")` internally. Remove that
header from `masks.rs` — the drawer supplies it now, and a header inside a header is a nesting level
this rework exists to remove. The body's contents stay exactly as they are.

- [ ] **Step 2: Delete the toolbar's Geometry shim**

Remove the temporary block added in Task 3 Step 4 from `develop/mod.rs`'s `toolbar`. The rail is now
the only way into geometry mode, which is the intent.

- [ ] **Step 3: Request a render when the tool changes**

`preview_edits` strips the crop in geometry mode, so entering or leaving Crop must re-render. In
`develop/mod.rs`'s `body`, wrap the rail call:

```rust
    let before = app.tool;
    if app.session.is_some() {
        rail::show(ui, &mut app.tool);
    }
    if app.tool != before {
        let tool = app.tool;
        if let Some(s) = &mut app.session {
            s.request(
                preview_edits(&s.edits, tool),
                s.render_max_dim(),
                s.requested_region,
            );
        }
    }
```

⚠ **A before/after comparison, not a return value from `rail::show`.** Task 6 adds 1–4 keyboard
shortcuts that also set `app.tool`; a click-only signal would leave those switching tools *without*
re-rendering. One mechanism covers both.

⚠ Without this the crop overlay draws over a *cropped* preview until some unrelated edit happens to
request a frame — structurally the same bug as `draw_mask_overlay` not calling `s.request`, found in
real use in M3.1a and recorded in CLAUDE.md. `preview_edits` is already imported in `develop/mod.rs`.

- [ ] **Step 4: Verify and commit**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -5
cargo test --all 2>&1 | grep -oE "^test result: ok\. [0-9]+" | awk '{s+=$4} END {print "TOTAL:", s}'
```

Expected: clean; **678**.

```bash
git add crates/app/src/develop/
git commit -m "feat(app): the rail's selection opens a drawer above the adjustments"
```

---

### Task 5: Presets becomes a drawer

**Files:**
- Create: `crates/app/src/develop/panel/presets.rs`
- Modify: `crates/app/src/develop/panel/mod.rs`, `crates/app/src/develop/mod.rs`, `crates/app/src/main.rs`

- [ ] **Step 1: Move `preset_panel` into a drawer**

Move the whole body of `develop/mod.rs`'s `preset_panel` into
`crates/app/src/develop/panel/presets.rs`, with these changes:

- Drop the `egui::Panel::left("presets").default_size(180.0).show(ui, |ui| { .. })` wrapper and the
  `ui.heading("Presets")` line — the drawer's `CollapsingHeader` supplies both the container and the
  title now.
- Keep the `Action` enum, the deferred-action pattern and the `match action` block **exactly** as
  they are. ⚠ That indirection is not incidental: egui borrows `app` through the closure, so the
  click must be recorded and acted on after the panel closes.
- Signature: `pub fn show(app: &mut RustroomApp, ui: &mut egui::Ui)`.

Header:

```rust
//! The Presets drawer: save the current edits as a preset, apply one, or delete one.
//!
//! ⚠ Was a dedicated **left panel** plus a toolbar toggle — the only feature in the app with
//! two homes. It is a rail tool now, with exactly one.
//!
//! ⚠ Applying a preset targets the whole *selection*, exactly as a paste does, which is why
//! this is not simply "edit the open photo".
```

- [ ] **Step 2: Draw the Presets drawer outside the session borrow**

⚠ **`Tool::Presets => presets::show(app, ui)` inside the drawer `match` cannot borrow-check**, and
this is worth understanding before writing it: that `match` sits inside a closure already holding
`s`, a `&mut app.session` borrow, while `presets::show` wants all of `&mut app`. Those are not
disjoint fields — one is the whole struct.

The resolution: **Presets is the only drawer that touches neither `Session` nor `Edits`**, so it does
not need to be inside the session borrow at all. Restructure the top of `panel::show`'s scroll area
so the histogram takes a *shared* borrow that ends, then Presets runs, then the session is borrowed
mutably for everything else:

```rust
            egui::ScrollArea::vertical().show(ui, |ui| {
                if app.session.is_none() {
                    ui.label("No image open.");
                    return;
                }
                // ⚠ A shared borrow that ends on this line, so `presets::show` below can take
                // `&mut app`. The histogram only reads.
                if let Some(s) = &app.session {
                    draw_histogram(ui, &s.hist);
                }
                ui.separator();

                // ⚠ Presets is drawn here, outside the mutable session borrow, because it is
                // the one drawer that edits neither `Session` nor `Edits` — it applies to the
                // whole selection through `app.apply_to_selection`.
                if app.tool == Tool::Presets {
                    egui::CollapsingHeader::new("Presets")
                        .default_open(true)
                        .show(ui, |ui| presets::show(app, ui));
                    ui.separator();
                }

                let tool = app.tool;
                if let Some(s) = &mut app.session {
                    // ... the drawer match (Crop / Masks only), adjustments, Reset all, footer
                }
            });
```

and in the drawer `match` from Task 4, the Presets arm becomes a no-op with a comment saying where it
is actually drawn:

```rust
                                // Drawn above, outside the session borrow — see the comment there.
                                Tool::Presets => {}
                                Tool::Edit => {}
```

⚠ The early `return` for "no session" replaces the old trailing `else { ui.label("No image open.") }`.
Keep that label — losing it means an empty panel with no explanation.

- [ ] **Step 3: Delete `show_presets` and the old panel**

- `main.rs`: remove the `show_presets: bool` field (~line 125) and its initialiser (~line 229).
- `develop/mod.rs`: remove the whole `preset_panel` function, the `if app.show_presets && ..` block
  in `body`, and `ui.toggle_value(&mut app.show_presets, "Presets");` from `toolbar`.
- Add `pub mod presets;` beside the other panel submodules.

- [ ] **Step 4: Verify and commit**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -5
cargo test --all 2>&1 | grep -oE "^test result: ok\. [0-9]+" | awk '{s+=$4} END {print "TOTAL:", s}'
grep -rn "show_presets\|preset_panel" crates/app/src/
```

Expected: clean; **678**; the grep returns **nothing**.

```bash
git add crates/app/src/
git commit -m "feat(app): Presets is a rail tool with one home"
```

---

### Task 6: The canvas follows the tool, and 1–4 select it

**Files:**
- Modify: `crates/app/src/develop/canvas.rs`
- Modify: `crates/app/src/develop/mod.rs`

- [ ] **Step 1: Gate the overlays on the tool**

In `canvas.rs` at ~line 214, replace:

```rust
                    if tool.is_geometry() {
                        photo_edited |= draw_crop_overlay(ui, s, &resp, img_rect);
                    } else {
                        photo_edited |= draw_mask_overlay(/* .. */);
                    }
```

with:

```rust
                    // ⚠ A behaviour change, and an intended one: mask handles and brush
                    // painting now belong to the Masks tool rather than to "any mode that is
                    // not Geometry". A stray drag in Edit can no longer paint a stroke.
                    match tool {
                        Tool::Crop => photo_edited |= draw_crop_overlay(ui, s, &resp, img_rect),
                        Tool::Masks => photo_edited |= draw_mask_overlay(/* unchanged args */),
                        Tool::Edit | Tool::Presets => {}
                    }
```

⚠ An exhaustive `match`, not `_ => {}`: a fifth tool must force a decision about what the canvas does
under it.

- [ ] **Step 2: Add the 1–4 shortcuts**

In `develop/mod.rs`'s `body`, before the rail is shown:

```rust
    // ⚠ Gated on nothing having keyboard focus, the same rule arrow navigation follows — or
    // typing "2" into the preset-name field would jump to the Crop tool.
    // ⚠ Step 3 of this rework routes these through the menu's `Command` dispatcher; until
    // then they are handled here.
    if app.session.is_some() && ctx.memory(|m| m.focused().is_none()) {
        let digit = ctx.input(|i| {
            [
                egui::Key::Num1,
                egui::Key::Num2,
                egui::Key::Num3,
                egui::Key::Num4,
            ]
            .iter()
            .position(|k| i.key_pressed(*k))
            .map(|p| p as u32 + 1)
        });
        if let Some(t) = digit.and_then(Tool::from_digit) {
            app.tool = t;
        }
    }
```

⚠ This block must sit **after `let before = app.tool;` and before `rail::show`**, so that the rail
paints the new selection on the same frame *and* Task 4's before/after comparison sees the change and
requests the render. Nothing about Task 4's code needs altering — that is why it was written as a
comparison rather than as a click signal.

- [ ] **Step 3: Verify and commit**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -5
cargo test --all 2>&1 | grep -oE "^test result: ok\. [0-9]+" | awk '{s+=$4} END {print "TOTAL:", s}'
```

Expected: clean; **678**. ⚠ The existing canvas tests (`a_left_drag_does_not_pan`, the brush drag
tests) must all still pass — if one fails, the tool gating has changed a path they exercise, and that
needs understanding rather than adjusting the test.

```bash
git add crates/app/src/develop/
git commit -m "feat(app): the canvas overlay follows the selected tool"
```

---

### Task 7: Look at it, then record it

- [ ] **Step 1: Full green**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo fmt --all --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all 2>&1 | tail -5
```

Expected: all green, **678 tests**, and **no golden moved** — this plan touches no pipeline code.

- [ ] **Step 2: Run the GUI and check what tests cannot see**

```bash
cargo run --release -p app
```

1. The rail sits **against the right window edge**, outside the adjustments panel — not between the panel and the image.
2. All four glyphs render as **shapes, not empty boxes**. This is the one that would have shipped broken.
3. **Edit** opens no drawer: the panel is the histogram, then the five sections.
4. **Crop** shows the crop overlay *and* an uncropped preview, and the drawer holds rotate/flip, aspect presets and Straighten. Switching to Crop re-renders **immediately**, without needing another edit to trigger it.
5. **Masks** shows mask handles; painting a brush stroke still works; the drawer holds the mask list and its adjustments, with **no doubled "Masks" header**.
6. **Presets** shows the preset list, and applying one still targets the whole selection.
7. Pressing **1 2 3 4** switches tools; typing a digit into the Save-preset name field does **not**.
8. The tool **stays selected when you arrow to the next photo** — the deliberate behaviour change.
9. The toolbar no longer has Geometry or Presets buttons.

- [ ] **Step 3: Update CLAUDE.md**

Record: the rail and its four tools; **the glyph finding** — that the spec's four candidates were all
absent and the coverage test is what caught it; `geometry_mode` becoming derived and the tool moving
to app level (behaviour change); Presets losing its second home; the canvas overlay gating change;
and that this is **step 2 of 3**, with the menu bar remaining.

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: record the tool rail"
```

---

## Self-review notes

Checked against the spec's **rail** sections:

- Rail on the right edge, outermost → Task 2 Step 5, with the panel-order rule documented. ✅
- Four tools, Edit default → Task 1. ✅
- Non-exclusive drawer above a permanent adjustment stack → Task 4 Step 1. ✅
- Glyph above text → Task 2, with the characters **replaced by measured ones**. ✅
- `geometry_mode` derived → Task 3. ✅
- Tool survives a photo switch → Task 2 Step 1 (field on `RustroomApp`). ✅
- Presets one home → Task 5. ✅
- 1–4 keyboard → Task 6 Step 2, flagged as moving into the `Command` dispatcher in step 3. ✅
- Headless `RawInput` rail test + glyph coverage test → Task 2. ✅
- Sabotage-with-compile-check on each new test → Tasks 1 and 2. ✅

**Deliberately not tested, and why.** The spec lists `the_tool_survives_a_photo_switch`. It is **not**
in this plan: the property holds because `tool` is a field of `RustroomApp` and `Session` has no such
field, so a test would be asserting a fact the compiler already guarantees, and constructing a real
photo switch headlessly needs a decoded buffer. Writing it would add a test that cannot fail — the
standing lesson in CLAUDE.md, eleven instances deep, is precisely about tests that assert at inputs
where the bug is inert.

**Out of scope, deferred to step 3:** the menu bar, the `Command` dispatcher, the shortcut refactor,
`Reset all` moving into the Edit menu, and the toolbar slimming to six controls. The toolbar loses
Geometry and Presets here and keeps everything else.
