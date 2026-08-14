# Visual Identity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give Rustroom a designed look — dark by default with a switchable accent, light mode as a setting, and a compact control density.

**Architecture:** Two plain serde enums join `Settings` in the egui-free lib; `theme.rs` in the binary maps them to `egui::Style`. `theme::apply` takes the settings and is re-invoked when either changes, exactly as `apply_view_demosaic` re-invokes the decode worker.

**Tech Stack:** Rust, egui 0.35. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-08-13-visual-identity-design.md`

**Build note:** every `cargo` command needs libraw and lcms2 on the pkg-config path. Export once per shell:

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

**Commit messages carry no `Co-Authored-By` trailer** (explicit user preference). Never `git push` — the user pushes themselves.

---

## File Structure

| File | Responsibility | Task |
|---|---|---|
| `crates/app/src/settings.rs` | **Lib.** Gains `ThemeChoice` and `Accent` enums plus two `Settings` fields. Knows *what the user picked*, never what it looks like. | 1 |
| `crates/app/src/theme.rs` | **Bin.** Maps the two enums to egui: the eight accent colours, the compact metrics, `apply(ctx, settings)`. | 2 |
| `crates/app/src/main.rs` | `RustroomApp::new` passes settings to `apply`; gains `apply_appearance`. | 3 |
| `crates/app/src/modals/settings.rs` | Gains an Appearance section: two combos with hints. | 3 |

### ⚠ Verified against egui 0.35 before writing this plan

The last two plans each guessed an API that did not exist. These were checked in the crate source:

- `Context::set_theme(impl Into<ThemePreference>)` — **exists**, and `egui::ThemePreference` already has exactly `Dark` / `Light` / `System`.
- `Context::style_mut_of(Theme, impl FnOnce(&mut Style))` — exists, already used by `theme::apply`.
- `Style::spacing.item_spacing: Vec2`, `spacing.interact_size: Vec2`, `spacing.slider_rail_height: f32` — all exist.
- `Visuals::selection.bg_fill: Color32`, `Visuals::widgets.{inactive,hovered,active}` — all exist.
- ⚠ `Accent` is defined in the **lib**, so Rust's orphan rule forbids an inherent `impl Accent` in the binary. The colour lookup is therefore a free function in `theme.rs`.
- **egui's real base colours** are `panel_fill = Color32::from_gray(27)` on dark and `from_gray(248)` on light. The spec's contrast table was computed against `#1c1c1c` / `#f2f2f2`; recomputed against the true bases every ratio is *better*, worst case **5.48:1** against a 3:1 threshold. No colour needs repicking.

---

## Task 1: The two choices, in the lib

`Settings` lives in `rustroom_app`, which is deliberately egui-free — so these are plain enums with no colour in them.

**Files:**
- Modify: `crates/app/src/settings.rs`

- [ ] **Step 1: Write the failing tests**

Add to `settings.rs`'s `mod tests`:

```rust
    #[test]
    fn a_settings_file_from_before_the_appearance_settings_still_loads() {
        // ⚠ The guard every new `Settings` field needs. `load()` is infallible, so a field
        // that fails to parse does not surface an error — it silently reverts *every* setting.
        let f = TempFile::new("appearance_old");
        std::fs::write(&f.0, r#"{"confirm_preset_delete": false}"#).unwrap();
        let loaded = SettingsStore::new(f.0.clone()).load();
        assert!(!loaded.confirm_preset_delete, "the old field must survive");
        assert_eq!(loaded.theme, ThemeChoice::Dark, "dark is the default");
        assert_eq!(loaded.accent, Accent::SteelBlue);
    }

    #[test]
    fn the_appearance_settings_round_trip() {
        let f = TempFile::new("appearance_rt");
        let store = SettingsStore::new(f.0.clone());
        let s = Settings {
            theme: ThemeChoice::Light,
            accent: Accent::Violet,
            ..Default::default()
        };
        store.save(&s).unwrap();
        let loaded = store.load();
        assert_eq!(loaded.theme, ThemeChoice::Light);
        assert_eq!(loaded.accent, Accent::Violet);
    }

    #[test]
    fn every_accent_and_theme_is_in_its_table() {
        // The dialog and the colour lookup both iterate `ALL`; a variant missing from it is
        // invisible in the UI rather than a compile error, so it is worth an explicit check.
        assert_eq!(Accent::ALL.len(), 4);
        assert_eq!(ThemeChoice::ALL.len(), 3);
        for a in Accent::ALL {
            assert!(!a.label().is_empty());
        }
        for t in ThemeChoice::ALL {
            assert!(!t.label().is_empty());
        }
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p app --lib settings
```

Expected: **compile error**, `cannot find type ThemeChoice in this scope` and the same for `Accent`.

- [ ] **Step 3: Write the enums**

In `crates/app/src/settings.rs`, above `struct Settings`:

```rust
/// Which theme to render in.
///
/// ⚠ A mirror of `egui::ThemePreference`, which `CLAUDE.md` warns against on the grounds that
/// "a mapping that exists twice is one that will disagree" — the reason `Demosaic` lives in
/// `raw-decode` rather than being duplicated. Accepted here because the direction is forced:
/// this crate's **lib** must not depend on egui, since that separation is what keeps
/// `history`, `selection` and the rest testable without a windowing system. The mitigation is
/// that `theme.rs` maps it with three exhaustive match arms, so a new variant is a compile
/// error rather than a silent gap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ThemeChoice {
    #[default]
    Dark,
    Light,
    System,
}

impl ThemeChoice {
    pub const ALL: [ThemeChoice; 3] = [
        ThemeChoice::Dark,
        ThemeChoice::Light,
        ThemeChoice::System,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ThemeChoice::Dark => "Dark",
            ThemeChoice::Light => "Light",
            ThemeChoice::System => "Follow system",
        }
    }
}

/// The colour used for *active* state — slider fill and handle, the open section, the primary
/// button.
///
/// ⚠ `Neutral` is not an absence: it is a grey, so "no accent" is a real choice rather than a
/// discarded one. Its cost is that active state falls back to lightness alone, which is
/// genuinely harder to read.
///
/// ⚠ **Amber is deliberately absent.** The objection is functional, not aesthetic: it sits on
/// the skin-tone and colour-temperature axis, which is precisely the judgement Temp and Tint
/// exist to serve, so a warm cast in the image becomes harder to see against it. It also
/// collides with amber's conventional meaning as a warning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Accent {
    Neutral,
    #[default]
    SteelBlue,
    Teal,
    Violet,
}

impl Accent {
    pub const ALL: [Accent; 4] = [
        Accent::Neutral,
        Accent::SteelBlue,
        Accent::Teal,
        Accent::Violet,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Accent::Neutral => "Neutral (no accent)",
            Accent::SteelBlue => "Steel blue",
            Accent::Teal => "Teal",
            Accent::Violet => "Violet",
        }
    }
}
```

- [ ] **Step 4: Add the fields**

In `struct Settings`, after `view_demosaic`:

```rust
    /// Which theme to render in. Dark by default: a light surround measurably changes how you
    /// judge exposure, so a photo editor defaults to dark and offers the rest.
    pub theme: ThemeChoice,
    /// The accent colour for active state.
    pub accent: Accent,
```

and in `impl Default for Settings`:

```rust
            theme: ThemeChoice::default(),
            accent: Accent::default(),
```

⚠ The exhaustive `Settings { .. }` literal in `settings_round_trip_carries_the_export_options` will now fail to compile. That is deliberate — it is what forces a decision about each new field. Add both explicitly there, keeping the literal exhaustive:

```rust
            theme: ThemeChoice::System,
            accent: Accent::Teal,
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cargo test -p app --lib settings
```

Expected: `test result: ok. 24 passed`.

- [ ] **Step 6: Run the full check suite**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
```

Expected: no warnings, 530 tests pass (527 + the three added here).

- [ ] **Step 7: Commit**

```bash
git add crates/app/src/settings.rs
git commit -m "feat(app): persist a theme and accent choice"
```

---

## Task 2: The colours and the metrics, in `theme.rs`

**Files:**
- Modify: `crates/app/src/theme.rs`

- [ ] **Step 1: Write the failing tests**

Add to `theme.rs`'s `mod tests`:

```rust
    /// WCAG relative luminance.
    fn luminance(c: egui::Color32) -> f64 {
        fn ch(v: u8) -> f64 {
            let v = v as f64 / 255.0;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * ch(c.r()) + 0.7152 * ch(c.g()) + 0.0722 * ch(c.b())
    }

    /// WCAG contrast ratio, 1.0..=21.0.
    fn contrast(a: egui::Color32, b: egui::Color32) -> f64 {
        let (x, y) = (luminance(a), luminance(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn every_accent_has_usable_contrast() {
        // ⚠ The load-bearing test. An accent tuned for near-black washes out on near-white —
        // that is why each is a pair — and 3:1 is the WCAG threshold for non-text UI
        // components. This is a guard for the *next* accent someone adds: the four here were
        // computed against it before being written down, worst case 5.48:1.
        for accent in Accent::ALL {
            for dark in [true, false] {
                let base = base_fill(dark);
                let c = contrast(accent_color(accent, dark), base);
                assert!(
                    c >= 3.0,
                    "{accent:?} on {} base: {c:.2}:1 is below 3:1",
                    if dark { "dark" } else { "light" }
                );
            }
        }
    }

    #[test]
    fn every_accent_differs_between_themes() {
        // A pair whose halves are equal means someone added a variant and filled in one value.
        // ⚠ Neutral is included deliberately — it is a grey, not an absence, so it needs two
        // values like everything else.
        for accent in Accent::ALL {
            assert_ne!(
                accent_color(accent, true),
                accent_color(accent, false),
                "{accent:?} uses the same colour for both themes"
            );
        }
    }

    #[test]
    fn the_compact_metrics_are_tighter_than_the_defaults() {
        // Pins the direction, not the values: a later tweak may change how much tighter, but
        // `tune` accidentally becoming a no-op again should fail.
        let mut s = egui::Style::default();
        let before = (s.spacing.item_spacing.y, s.spacing.interact_size.y);
        tune(&mut s);
        assert!(s.spacing.item_spacing.y < before.0);
        assert!(s.spacing.interact_size.y < before.1);
    }
```

⚠ Add `use rustroom_app::settings::Accent;` to the test module's imports if `super::*` does not already bring it in.

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p app --bin rustroom-gui theme
```

Expected: **compile error**, `cannot find function accent_color` and `cannot find function base_fill`.

- [ ] **Step 3: Write the colour table**

In `crates/app/src/theme.rs`, below the imports:

```rust
/// egui's own panel fill, which is what an accent is judged against.
///
/// ⚠ Read from `Visuals`, not hardcoded: if egui changes its base in a future release the
/// contrast test notices, rather than silently checking against a number that is no longer
/// true.
fn base_fill(dark: bool) -> egui::Color32 {
    if dark {
        egui::Visuals::dark().panel_fill
    } else {
        egui::Visuals::light().panel_fill
    }
}

/// The accent colours, one pair per variant.
///
/// ⚠ **Two values per accent, not one.** A colour tuned for near-black washes out on
/// near-white — `#7fb0d8` on a light base is low contrast and fails white text on it. Every
/// pair here was contrast-checked against egui's real bases before being written down; worst
/// case is steel blue on light at 5.48:1, against a 3:1 threshold.
///
/// ⚠ A free function, not `impl Accent`. `Accent` is defined in the **lib** crate, and Rust's
/// orphan rule allows inherent impls only in the crate that defines the type — so this could
/// only be an extension trait or a free function, and a trait carrying one method used in one
/// file is machinery for its own sake.
fn accent_color(accent: Accent, dark: bool) -> egui::Color32 {
    let (r, g, b) = match (accent, dark) {
        (Accent::Neutral, true) => (0x9a, 0x9a, 0x9a),
        (Accent::Neutral, false) => (0x5a, 0x5a, 0x5a),
        (Accent::SteelBlue, true) => (0x7f, 0xb0, 0xd8),
        (Accent::SteelBlue, false) => (0x2f, 0x6a, 0x94),
        (Accent::Teal, true) => (0x5f, 0xb3, 0xac),
        (Accent::Teal, false) => (0x2c, 0x6d, 0x68),
        (Accent::Violet, true) => (0x9a, 0x8f, 0xd0),
        (Accent::Violet, false) => (0x54, 0x4a, 0x8c),
    };
    egui::Color32::from_rgb(r, g, b)
}
```

⚠ Add `use rustroom_app::settings::{Accent, Settings, ThemeChoice};` to `theme.rs`'s imports.

- [ ] **Step 4: Fill in `tune`**

Replace the empty `tune` with:

```rust
/// The theme-independent tweaks: the compact density.
///
/// ⚠ **Only theme-independent things belong here.** Anything colour-related goes in that
/// theme's own `visuals` — see [`apply`] — or light mode inherits dark colours.
///
/// Compact because the Develop panel stacks 20+ sliders across six collapsible sections, and
/// every Phase 3 tool makes that worse. It costs some trackpad precision on fine drags, which
/// is the trade taken knowingly.
fn tune(style: &mut egui::Style) {
    style.spacing.item_spacing.y = 4.0;
    style.spacing.interact_size.y = 18.0;
    style.spacing.slider_rail_height = 4.0;
}
```

- [ ] **Step 5: Rewrite `apply` to take the settings**

```rust
/// Apply Rustroom's style, at startup and whenever the appearance settings change.
///
/// ⚠ **egui 0.35 keeps a separate `Style` per theme** — dark and light — so there is no single
/// style to set, and `Context::set_style` does not exist. Anything theme-independent goes
/// through [`tune`] for both; the accent is resolved *per theme*, because a colour tuned for
/// near-black washes out on near-white.
///
/// `style_mut_of` rather than `set_style_of`: mutating in place preserves each theme's own
/// visuals, where handing over a whole `Style` would overwrite them.
pub fn apply(ctx: &egui::Context, settings: &Settings) {
    ctx.set_theme(match settings.theme {
        ThemeChoice::Dark => egui::ThemePreference::Dark,
        ThemeChoice::Light => egui::ThemePreference::Light,
        ThemeChoice::System => egui::ThemePreference::System,
    });
    for (theme, dark) in [(egui::Theme::Dark, true), (egui::Theme::Light, false)] {
        let accent = accent_color(settings.accent, dark);
        ctx.style_mut_of(theme, |style| {
            tune(style);
            style.visuals.selection.bg_fill = accent;
            style.visuals.widgets.hovered.bg_fill = accent;
            style.visuals.widgets.active.bg_fill = accent;
        });
    }
}
```

- [ ] **Step 6: Run the tests to verify they pass**

```bash
cargo test -p app --bin rustroom-gui theme
```

Expected: `test result: ok. 6 passed` — the three original plus the three added.

⚠ `apply` now takes two arguments, so `main.rs` will not compile until task 3. `cargo test --bin` builds the whole binary, so **expect the run above to fail on `main.rs` first**; fix that by doing task 3 step 1 now if you prefer a green intermediate state. Do **not** leave a `Settings::default()` call in `apply` to paper over it.

- [ ] **Step 7: Prove the contrast test can fail**

Temporarily change the light steel blue to the dark one — `(Accent::SteelBlue, false) => (0x7f, 0xb0, 0xd8),` — then run:

```bash
cargo test -p app --bin rustroom-gui theme
```

Expected: **both** `every_accent_has_usable_contrast` (`SteelBlue on light base: 1.86:1 is below 3:1`) and `every_accent_differs_between_themes` fail. Restore the value and re-run to confirm green.

⚠ If restoring from a copy rather than by editing, `touch crates/app/src/theme.rs` afterwards — `cp -p` preserves the older mtime and cargo reuses the sabotaged artifact.

---

## Task 3: Wire it in, and add the Settings control

**Files:**
- Modify: `crates/app/src/main.rs`
- Modify: `crates/app/src/modals/settings.rs`

- [ ] **Step 1: Pass the settings at startup**

In `RustroomApp::new` in `crates/app/src/main.rs`, the first statement is currently `theme::apply(ctx);`. `settings` is loaded a few lines below it, so **move the call to after `let settings = settings_store.load();`**:

```rust
        let settings = settings_store.load();
        theme::apply(ctx, &settings);
```

- [ ] **Step 2: Add the re-apply method**

In `impl RustroomApp` in `main.rs`, directly above `apply_view_demosaic`:

```rust
    /// Adopt new appearance settings: persist them and re-style immediately, so the change is
    /// visible rather than deferred to the next launch.
    ///
    /// ⚠ Cheap enough to do inline — unlike `apply_view_demosaic`, nothing is decoded. It is
    /// still guarded by a change comparison at the call site, because the Settings dialog body
    /// runs every frame it is open.
    fn apply_appearance(&mut self, theme_choice: ThemeChoice, accent: Accent, ctx: &egui::Context) {
        self.settings.theme = theme_choice;
        self.settings.accent = accent;
        self.save_settings();
        theme::apply(ctx, &self.settings);
    }
```

⚠ Add `ThemeChoice` and `Accent` to `main.rs`'s existing settings import:

```rust
use rustroom_app::settings::{Accent, Settings, SettingsStore, ThemeChoice};
```

- [ ] **Step 3: Add the Appearance section to the Settings dialog**

In `crates/app/src/modals/settings.rs`, inside the `dialog(..)` closure and **above** the existing "1:1 view quality" row, add:

```rust
            ui.horizontal(|ui| {
                ui.label("Theme");
                egui::ComboBox::from_id_salt("theme_pick")
                    .selected_text(theme_choice.label())
                    .show_ui(ui, |ui| {
                        for t in ThemeChoice::ALL {
                            ui.selectable_value(&mut theme_choice, t, t.label());
                        }
                    });
            });
            ui.horizontal(|ui| {
                ui.label("Accent");
                egui::ComboBox::from_id_salt("accent_pick")
                    .selected_text(accent.label())
                    .show_ui(ui, |ui| {
                        for a in Accent::ALL {
                            ui.selectable_value(&mut accent, a, a.label());
                        }
                    });
            });
            theme::hint(
                ui,
                "Dark is the default because a light surround changes how you judge \
                 exposure. The accent marks active controls; Neutral turns it off.",
            );
            ui.separator();
```

Declare the two locals at the top of `show`, beside `let mut tier = app.settings.view_demosaic;`:

```rust
    let mut theme_choice = app.settings.theme;
    let mut accent = app.settings.accent;
```

and after the dialog closes, beside the existing `tier` guard:

```rust
    // ⚠ The inequality is what stops the style being rebuilt on *every frame* the dialog is
    // open — `show` runs once per frame.
    if theme_choice != app.settings.theme || accent != app.settings.accent {
        app.apply_appearance(theme_choice, accent, ctx);
    }
```

⚠ Add to `modals/settings.rs`'s imports:

```rust
use rustroom_app::settings::{Accent, ThemeChoice};
```

- [ ] **Step 4: Run the full check suite**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
```

Expected: no warnings, **533 tests pass** — 527 before, plus three in task 1 and three in task 2.

- [ ] **Step 5: Commit**

```bash
git add crates/app/src/theme.rs crates/app/src/main.rs crates/app/src/modals/settings.rs
git commit -m "feat(app): a dark default, a switchable accent, and compact controls"
```

---

## Task 4: Verify and document

**Files:** `CLAUDE.md`

- [ ] **Step 1: Launch the GUI**

```bash
cargo run --release -p app
```

- [ ] **Step 2: Walk the appearance settings**

1. **On launch** the app is dark, regardless of the system setting, and the Develop panel is visibly tighter than before — more sliders visible without scrolling.
2. **Settings → Accent**, step through all four. Slider fills and handles, the open section header and the primary button change immediately. **Neutral** should read as genuinely no-accent: active state is lightness only.
3. **Settings → Theme → Light.** The whole app switches, and the accent stays legible rather than washing out — that is the pair working.
4. **Theme → Follow system**, then change the system appearance; the app follows.
5. **Restart.** Both choices survive.
6. **⚠ Check what the accent does *not* reach**: the crop overlay, the histogram and the thumbnail selection border are drawn by hand with hardcoded colours and will not follow the accent. Confirm they still look right in **light** mode especially, since they were only ever seen on dark.

- [ ] **Step 3: Update `CLAUDE.md`**

Add a section in the "Current status" block, house style: what landed; the lib/bin split of choice-versus-colour and why `Accent::color` is an extension trait; that `ThemeChoice` is a mirror enum accepted against the codebase's own rule, with the reason; the two-values-per-accent finding and the contrast test that guards it; why amber is excluded; the compact metrics and their trackpad cost; and the caveats — two palettes to maintain, `Neutral` being harder to read, and the hand-drawn colours the accent does not reach.

Then set the **▶ NEXT ACTION** to **Phase 3**, noting it is a phase rather than a feature and needs decomposing into sub-projects during brainstorming. Add this spec and plan to the docs list at the bottom.

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md docs/superpowers/plans/2026-08-13-visual-identity.md
git commit -m "docs: record the visual identity work"
```
