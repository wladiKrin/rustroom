# Develop Panel Restyle Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

> ⚠ **Conventions that are not negotiable:** commit messages carry **no `Co-Authored-By` trailer**;
> everything lands on the branch `develop-panel-restyle`; **never push** — the user pushes.
> Every gate reads its exit code from its own command, never through a pipe.

**Goal:** Restyle the Develop panel in four series — A surfaces, B slider rows, C section chrome, D2 icon rail — per `docs/superpowers/specs/2026-09-18-develop-panel-restyle-design.md`, with no change to any pixel the pipeline produces.

**Architecture:** Colour and metric changes go through `theme.rs`, the one place the look is defined. A new hand-painted slider widget (`develop/panel/slider.rs`, bin) sits on a pure mapping module (`slider_map.rs`, lib) and is reached through the existing `Edited::slider` funnel so every panel inherits it. Section headers become a custom row over egui's `CollapsingState`; a per-section "modified" signal comes from a new `Edits::group_differs` defined through `merge_from`. The rail loses its labels and gains hover tooltips plus accessible names.

**Tech Stack:** Rust, egui 0.35 (`CollapsingState`, `WidgetInfo`, `Mesh`), `emath::smart_aim`, AccessKit via `test_ui` for headless tests.

---

## Gate, run after every task that says "gate"

```bash
cd ~/Projects/rustroom
L=/private/tmp/claude-501/-Users-me-Projects-rustroom/f338e3ec-6858-46aa-8ee2-220e00ae7481/scratchpad
cargo fmt --all -- --check > $L/fmt.log 2>&1; echo FMT=$?
cargo clippy --all-targets --all-features -- -D warnings > $L/clippy.log 2>&1; echo CLIPPY=$?
cargo test --all > $L/test.log 2>&1; echo TEST=$?
grep -c "^test result: ok" $L/test.log; grep -E "^test result:.*FAILED|panicked" $L/test.log | head
```
Expected: `FMT=0`, `CLIPPY=0`, `TEST=0`, no FAILED lines. ⚠ Every golden test must still **run**: `grep -c "golden" $L/test.log` must not drop from its value on `main` (74 goldens).

## GUI launch, at the end of every series

```bash
cd ~/Projects/rustroom
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo run --release -p app
```
Run in the background and tell the user what to look at.

---

## File structure

| File | Series | Responsibility |
|---|---|---|
| `crates/app/src/theme.rs` | A, C | surfaces, radii, margins (`panel_frame`, `canvas_frame`), the `section` text style, `sub_label`, `segmented` |
| `crates/app/src/develop/panel/mod.rs` | A, B, C | panel frame; `Edited::slider` delegates; histogram painter; passes `hist` |
| `crates/app/src/develop/panel/snapshots.rs` | A | strip frame |
| `crates/app/src/develop/canvas/mod.rs` | A | canvas frame |
| `crates/app/src/slider_map.rs` (new, lib) | B | value ↔ rail position, fill span, zero tick |
| `crates/app/src/develop/panel/slider.rs` (new, bin) | B | the row widget |
| `crates/app/src/develop/panel/adjustments.rs` | B, C | Temp slider; `section`; sub-labels; grade selector; `SourceInfo.hist` |
| `crates/app/src/develop/panel/retouch.rs` | B | Size, Feather, Threshold sliders |
| `crates/edit-model/src/lib.rs` | C | `Edits::group_differs` |
| `crates/app/src/develop/panel/hsl_bands.rs`, `mask_adjust.rs`, `geometry.rs`, `color_wheel.rs`, `tone_curve.rs` | C | segmented selectors, sub-labels, full grade labels, curve well + ghost |
| `crates/app/src/histogram.rs` (lib) | C | `clipped_edges` |
| `crates/app/src/develop/rail.rs` | D2 | icon-only entries with tooltips |

⚠ `adjustments.rs` is 1,165 lines, `masks.rs` 834, `color_wheel.rs` 624 — past the ~600 marker. C removes lines from `adjustments.rs` and touches `masks.rs` not at all; say so in the report.

---

# Series A — surfaces and spacing

### Task A1: The values, in `theme.rs`

**Files:**
- Modify: `crates/app/src/theme.rs` — `apply`, `tune`, new `surfaces`, `canvas_fill`, `canvas_frame`, `panel_frame`; tests

- [ ] **Step 1: Write the failing tests** — add to `mod tests` in `theme.rs`, after `every_accent_differs_between_themes`:

```rust
    #[test]
    fn body_text_is_readable_on_the_panel() {
        // ⚠ 4.5:1 is WCAG AA for text, a stricter bar than the accents' 3:1: a slider label
        // is read, an accent is only noticed. Dark text sat at 4.6:1 before this pass.
        let ctx = egui::Context::default();
        apply(&ctx, &Settings::default());
        for (theme, dark) in [(egui::Theme::Dark, true), (egui::Theme::Light, false)] {
            let v = ctx.style_of(theme).visuals.clone();
            let c = contrast(v.widgets.noninteractive.fg_stroke.color, v.panel_fill);
            assert!(c >= 4.5, "{} body text on its panel: {c:.2}:1 is below 4.5:1", if dark { "dark" } else { "light" });
        }
    }

    #[test]
    fn the_canvas_is_darker_than_the_panel_in_dark_mode_and_lighter_in_light_mode() {
        // ⚠ Pins the *direction* per theme, so a swapped pair — the mistake the accent pairs
        // exist to prevent — cannot ship. The surround argument is the visual identity spec's:
        // the darkest, most neutral surface is the one touching the photograph.
        assert!(luminance(canvas_fill(true)) < luminance(base_fill(true)));
        assert!(luminance(canvas_fill(false)) > luminance(base_fill(false)));
    }

    #[test]
    fn the_panel_frame_has_an_inner_margin() {
        // A frame with egui's default margin is what left "Camera profile" flush against
        // the panel edge.
        let s = tuned();
        let m = panel_frame(&s).inner_margin;
        assert!(m.left >= 8 && m.right >= 8, "margin {m:?}");
    }

    #[test]
    fn every_widget_state_shares_one_corner_radius() {
        let s = tuned();
        let w = &s.visuals.widgets;
        for ws in [&w.noninteractive, &w.inactive, &w.hovered, &w.active, &w.open] {
            assert_eq!(ws.corner_radius, egui::CornerRadius::same(4));
        }
    }
```

- [ ] **Step 2: Replace `base_fill` in the test module** so it reads the *applied* style — after A the egui default is no longer the base:

```rust
    /// The panel fill each theme actually ends up with — what an accent is judged against.
    ///
    /// ⚠ Read through `apply`, not from `Visuals::dark()`: since the surfaces pass the panel
    /// is no longer egui's default, and checking accents against a base nobody paints would
    /// say nothing.
    fn base_fill(dark: bool) -> egui::Color32 {
        let ctx = egui::Context::default();
        apply(&ctx, &Settings::default());
        let theme = if dark { egui::Theme::Dark } else { egui::Theme::Light };
        ctx.style_of(theme).visuals.panel_fill
    }
```

- [ ] **Step 3: Run to verify failure**

Run: `cargo test -p app --bin Rustroom theme:: 2>&1 | tail -20`
Expected: compile error — `canvas_fill` and `panel_frame` not found.

- [ ] **Step 4: Implement.** In `apply`, replace the `ctx.style_mut_of` closure body:

```rust
        ctx.style_mut_of(theme, |style| {
            tune(style);
            surfaces(style, dark);
            style.visuals.selection.bg_fill = accent;
            style.visuals.widgets.hovered.bg_fill = accent;
            style.visuals.widgets.active.bg_fill = accent;
        });
```

Add after `apply`:

```rust
/// The theme's surfaces: the panel, the popups, body text and hairlines.
///
/// ⚠ **Per theme, never in [`tune`]** — every value here is a pair, the visual identity
/// spec's rule. Dark mode is the one that moves: egui's `gray(140)` body text on `gray(27)`
/// is 4.6:1, readable but dim beside the `gray(180)` button text next to it, and every surface
/// was the same `gray(27)` so nothing read as a panel. Light mode's `gray(80)` on `gray(248)`
/// is already 7.5:1 and keeps its text; only its hairlines soften.
///
/// ⚠ Separators get *weaker*, not stronger: section headers carry the structure now, so the
/// hairlines become guides rather than walls.
fn surfaces(style: &mut egui::Style, dark: bool) {
    let v = &mut style.visuals;
    if dark {
        let panel = egui::Color32::from_gray(32);
        v.panel_fill = panel;
        v.window_fill = panel;
        v.widgets.noninteractive.bg_fill = panel;
        v.widgets.noninteractive.weak_bg_fill = panel;
        v.widgets.noninteractive.fg_stroke.color = egui::Color32::from_gray(166);
        v.widgets.noninteractive.bg_stroke.color = egui::Color32::from_gray(46);
    } else {
        v.widgets.noninteractive.bg_stroke.color = egui::Color32::from_gray(205);
    }
}

/// The Develop canvas surround: darker than the panel in dark mode, lighter in light mode.
///
/// ⚠ The panel is a surface the image is judged *from*; the canvas is the surround it is
/// judged *against*, and the visual identity spec's argument — low chroma **near the image** —
/// is what this serves. ⚠ Only the Develop canvas takes it; the Library grid is a browsing
/// surface and keeps `panel_fill`.
pub fn canvas_fill(dark: bool) -> egui::Color32 {
    if dark {
        egui::Color32::from_gray(22)
    } else {
        egui::Color32::from_gray(222)
    }
}

/// The frame the Develop canvas is drawn in.
pub fn canvas_frame(style: &egui::Style) -> egui::Frame {
    egui::Frame::central_panel(style).fill(canvas_fill(style.visuals.dark_mode))
}

/// The inner margin of the right panel and the snapshots strip.
///
/// ⚠ egui's `side_top_panel` frame has a much smaller one, which is what left content flush
/// against the panel edge. Deliberately not applied through `tune`'s spacing, which every
/// menu and popup shares.
pub const PANEL_MARGIN: egui::Margin = egui::Margin {
    left: 10,
    right: 10,
    top: 6,
    bottom: 6,
};

/// The frame the right panel and the snapshots strip are drawn in.
pub fn panel_frame(style: &egui::Style) -> egui::Frame {
    egui::Frame::side_top_panel(style).inner_margin(PANEL_MARGIN)
}
```

In `tune`, after `style.spacing.slider_rail_height = 4.0;`:

```rust
    // One radius for every widget state. egui ships 2/2/3/2/2 — hovered widgets change shape
    // as well as colour, which reads as a flicker on a row of chips.
    let w = &mut style.visuals.widgets;
    for ws in [
        &mut w.noninteractive,
        &mut w.inactive,
        &mut w.hovered,
        &mut w.active,
        &mut w.open,
    ] {
        ws.corner_radius = egui::CornerRadius::same(4);
    }
```

⚠ `canvas_fill`, `canvas_frame`, `panel_frame` and `PANEL_MARGIN` are `pub` in a **binary** crate with no caller until A2 — the bin-crate dead-code trap. **A1 and A2 land as one commit.**

- [ ] **Step 5: Do not run the gate yet** — proceed to A2.

### Task A2: The frames

**Files:**
- Modify: `crates/app/src/develop/panel/mod.rs` (the `Panel::right("panel")` builder)
- Modify: `crates/app/src/develop/panel/snapshots.rs` (the `Panel::bottom(PANEL_ID)` builder)
- Modify: `crates/app/src/develop/canvas/mod.rs` (`CentralPanel::default().show`)

- [ ] **Step 1: Panel.** In `panel/mod.rs` change

```rust
    egui::Panel::right("panel")
        .default_size(240.0)
        .show(ui, |ui| {
```
to
```rust
    egui::Panel::right("panel")
        .default_size(240.0)
        .frame(theme::panel_frame(&ui.style()))
        .show(ui, |ui| {
```

- [ ] **Step 2: Strip.** In `snapshots.rs` change

```rust
    let panel = egui::Panel::bottom(PANEL_ID)
        .resizable(true)
```
to
```rust
    let panel = egui::Panel::bottom(PANEL_ID)
        .frame(theme::panel_frame(&ui.style()))
        .resizable(true)
```

- [ ] **Step 3: Canvas.** In `canvas/mod.rs` change `egui::CentralPanel::default().show(ui, |ui| {` to

```rust
    egui::CentralPanel::default()
        .frame(crate::theme::canvas_frame(&ui.style()))
        .show(ui, |ui| {
```

- [ ] **Step 4: Gate.** Expected all green; the four new theme tests pass; every existing theme test passes with the new `base_fill`.

- [ ] **Step 5: Commit**

```bash
git add crates/app/src/theme.rs crates/app/src/develop/panel/mod.rs crates/app/src/develop/panel/snapshots.rs crates/app/src/develop/canvas/mod.rs
git commit -m "Surfaces: a darker canvas, a lighter panel, readable body text, one radius, a panel margin"
```

- [ ] **Step 6: Launch the GUI.** Tell the user: the canvas is now darker than the panel; body text is brighter; the panel content has a margin; switch Settings → Appearance to Light to check the light pair. ⚠ The slider anatomy is unchanged until B.

---

# Series B — the slider row

### Task B1: `slider_map`, the pure half

**Files:**
- Create: `crates/app/src/slider_map.rs`
- Modify: `crates/app/src/lib.rs` (add `pub mod slider_map;` between `pub mod sidecar;` and `pub mod spot_source;`)

- [ ] **Step 1: Write the module with its tests**

```rust
//! The pure half of the panel slider: where a value sits on its rail, what the accent fill
//! spans, and where the zero tick goes.
//!
//! ⚠ No egui here — this is what the tests reach for the geometry, and the bin widget only
//! paints what these return. A rail position `t` is `0.0` at the range's start and `1.0` at
//! its end.

/// Whether a range has zero strictly inside it — the bipolar case, where the fill runs from
/// zero rather than from the left edge.
pub fn straddles_zero(min: f32, max: f32) -> bool {
    min < 0.0 && max > 0.0
}

/// Whether a log mapping is usable: both ends strictly positive.
fn log_ok(min: f32, max: f32) -> bool {
    min > 0.0 && max > 0.0 && min != max
}

/// The rail position of `value`, clamped to `0.0..=1.0`.
pub fn t_of(value: f32, min: f32, max: f32, log: bool) -> f32 {
    if min == max {
        return 0.0;
    }
    let t = if log && log_ok(min, max) {
        let v = value.max(min.min(max));
        (v.ln() - min.ln()) / (max.ln() - min.ln())
    } else {
        (value - min) / (max - min)
    };
    t.clamp(0.0, 1.0)
}

/// The value at rail position `t`, clamped to the range.
pub fn value_of(t: f32, min: f32, max: f32, log: bool) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let v = if log && log_ok(min, max) {
        (min.ln() + t * (max.ln() - min.ln())).exp()
    } else {
        min + t * (max - min)
    };
    v.clamp(min.min(max), min.max(max))
}

/// Where zero sits on a bipolar rail, `None` for a unipolar range.
pub fn zero_t(min: f32, max: f32) -> Option<f32> {
    straddles_zero(min, max).then(|| t_of(0.0, min, max, false))
}

/// The span of the accent fill: from zero to the handle on a bipolar rail, from the left
/// edge to the handle otherwise. Always `(lo, hi)` with `lo <= hi`.
pub fn fill_span(t: f32, min: f32, max: f32) -> (f32, f32) {
    match zero_t(min, max) {
        Some(z) => (t.min(z), t.max(z)),
        None => (0.0, t),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_linear_rail_round_trips_at_both_ends_and_the_middle() {
        for (t, want) in [(0.0, -100.0), (0.5, 0.0), (1.0, 100.0)] {
            assert_eq!(value_of(t, -100.0, 100.0, false), want);
            assert_eq!(t_of(want, -100.0, 100.0, false), t);
        }
    }

    #[test]
    fn a_log_rail_puts_the_geometric_middle_at_the_centre() {
        // 1..100: the centre is 10, not 50.5 — the whole point of the log mapping for a
        // radius whose useful zone sits at the bottom of a 50x range.
        assert!((value_of(0.5, 1.0, 100.0, true) - 10.0).abs() < 1e-4);
        assert!((t_of(10.0, 1.0, 100.0, true) - 0.5).abs() < 1e-6);
        assert_eq!(value_of(0.0, 1.0, 100.0, true), 1.0);
        assert_eq!(value_of(1.0, 1.0, 100.0, true), 100.0);
    }

    #[test]
    fn a_log_rail_with_a_non_positive_end_falls_back_to_linear() {
        // ln(0) is -inf; a caller asking for log on 0..100 must get something usable.
        assert_eq!(value_of(0.5, 0.0, 100.0, true), 50.0);
    }

    #[test]
    fn positions_are_clamped_to_the_rail() {
        assert_eq!(t_of(500.0, -100.0, 100.0, false), 1.0);
        assert_eq!(t_of(-500.0, -100.0, 100.0, false), 0.0);
        assert_eq!(value_of(2.0, 0.0, 10.0, false), 10.0);
    }

    #[test]
    fn the_fill_runs_from_zero_on_a_bipolar_rail_and_from_the_left_otherwise() {
        // ⚠ Probed at 0.3 and 0.7, never only at 0.5 where a wrong branch is inert.
        assert_eq!(fill_span(0.3, -100.0, 100.0), (0.3, 0.5));
        assert_eq!(fill_span(0.7, -100.0, 100.0), (0.5, 0.7));
        assert_eq!(fill_span(0.3, 0.0, 100.0), (0.0, 0.3));
        assert_eq!(fill_span(0.7, 0.0, 100.0), (0.0, 0.7));
    }

    #[test]
    fn the_zero_tick_follows_an_asymmetric_range() {
        // -5..15: zero is a quarter of the way along, not in the middle.
        assert_eq!(zero_t(-5.0, 15.0), Some(0.25));
        assert_eq!(zero_t(0.0, 100.0), None);
        assert_eq!(zero_t(0.5, 3.0), None);
    }
}
```

- [ ] **Step 2: Run** `cargo test -p app --lib slider_map 2>&1 | tail -12`. Expected: `6 passed`. ⚠ Read the passed count — `--lib`, because this is a lib module.

- [ ] **Step 3: Commit**

```bash
git add crates/app/src/slider_map.rs crates/app/src/lib.rs
git commit -m "slider_map: the pure half of the panel slider"
```

### Task B2: The widget, wired through `Edited::slider`

**Files:**
- Create: `crates/app/src/develop/panel/slider.rs`
- Modify: `crates/app/src/develop/panel/mod.rs` — add `pub mod slider;` to the module list; `Edited::slider` delegates

- [ ] **Step 1: Write the widget**

```rust
//! The panel's slider row: label │ rail │ value, on one 16 pt line.
//!
//! ⚠⚠ **Why not `egui::Slider`.** Its rail is a fixed `spacing.slider_width` (100 pt), so a
//! third of every 240 pt row was the value box and the label. And its trailing fill starts at
//! the **left edge**, so a bipolar slider at 0 — Contrast, Tint, every HSL band — showed half a
//! bar of accent for "nothing applied", the opposite of the signal the accent was chosen to
//! carry. Here the fill runs from zero and the rail takes what the label and value leave.
//!
//! ⚠⚠ **The behaviour matches egui's on purpose.** Four headless harnesses locate a slider by
//! its AccessKit label, press at its centre and drag right; `Edited::slider` records
//! `changed()` and `drag_stopped()` separately for one history entry per settled drag. So the
//! rail carries `WidgetInfo::slider` with the label, the value is set from the pointer's
//! **position** (through `smart_aim`, so a drag lands on 0.69 rather than 0.6873), arrows step
//! it while focused, and the returned response is the union of rail and value exactly as
//! `egui::Slider::add_contents` builds it.
//!
//! ⚠ Not built, deliberately: double-click to reset, a precision modifier, vertical rails,
//! `step`, clamping modes. Only what the panel's call sites use.

use eframe::egui;
use rustroom_app::slider_map;
use std::ops::RangeInclusive;

/// The label column. The widest labels today ("Radius (px)", "Smoothness") fit; a wider one
/// truncates and shows in full on hover.
pub const LABEL_WIDTH: f32 = 78.0;
/// The value column, right-aligned.
pub const VALUE_WIDTH: f32 = 44.0;
const RAIL_HEIGHT: f32 = 3.0;
const HANDLE_RADIUS: f32 = 5.0;
const GAP: f32 = 8.0;
/// One arrow press moves the handle this fraction of the rail.
const KEY_STEP: f32 = 0.01;

type Formatter<'a> = Box<dyn Fn(f64) -> String + 'a>;
type Parser<'a> = Box<dyn Fn(&str) -> Option<f64> + 'a>;

pub struct Slider<'a> {
    value: &'a mut f32,
    range: RangeInclusive<f32>,
    label: &'a str,
    log: bool,
    formatter: Option<Formatter<'a>>,
    parser: Option<Parser<'a>>,
}

impl<'a> Slider<'a> {
    pub fn new(value: &'a mut f32, range: RangeInclusive<f32>) -> Self {
        Self {
            value,
            range,
            label: "",
            log: false,
            formatter: None,
            parser: None,
        }
    }

    pub fn text(mut self, label: &'a str) -> Self {
        self.label = label;
        self
    }

    /// A log rail, for a range whose useful zone sits at the bottom (the retouch Size).
    /// Falls back to linear when an end is not strictly positive.
    pub fn logarithmic(mut self, on: bool) -> Self {
        self.log = on;
        self
    }

    pub fn custom_formatter(mut self, f: impl Fn(f64) -> String + 'a) -> Self {
        self.formatter = Some(Box::new(f));
        self
    }

    pub fn custom_parser(mut self, f: impl Fn(&str) -> Option<f64> + 'a) -> Self {
        self.parser = Some(Box::new(f));
        self
    }
}

impl egui::Widget for Slider<'_> {
    fn ui(self, ui: &mut egui::Ui) -> egui::Response {
        let Self {
            value,
            range,
            label,
            log,
            formatter,
            parser,
        } = self;
        let (min, max) = (*range.start(), *range.end());
        let height = ui.spacing().interact_size.y;
        let old = *value;
        let bipolar = slider_map::straddles_zero(min, max);

        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = GAP;

            // ── label ──
            let (label_rect, label_resp) =
                ui.allocate_exact_size(egui::vec2(LABEL_WIDTH, height), egui::Sense::hover());
            let font = egui::TextStyle::Body.resolve(ui.style());
            let mut job = egui::text::LayoutJob::simple_singleline(
                label.to_owned(),
                font,
                ui.visuals().text_color(),
            );
            job.wrap.max_width = LABEL_WIDTH;
            job.wrap.max_rows = 1;
            job.wrap.break_anywhere = true;
            job.wrap.overflow_character = Some('…');
            let galley = ui.fonts_mut(|f| f.layout_job(job));
            let elided = galley.elided;
            ui.painter().galley(
                egui::pos2(
                    label_rect.left(),
                    label_rect.center().y - galley.size().y / 2.0,
                ),
                galley,
                ui.visuals().text_color(),
            );
            if elided {
                label_resp.on_hover_text(label);
            }

            // ── rail ──
            let rail_w = (ui.available_width() - VALUE_WIDTH - GAP).max(24.0);
            let (rect, mut resp) = ui.allocate_exact_size(
                egui::vec2(rail_w, height),
                egui::Sense::click_and_drag(),
            );
            let x_range = egui::Rangef::new(
                rect.left() + HANDLE_RADIUS,
                rect.right() - HANDLE_RADIUS,
            );
            if let Some(p) = resp.interact_pointer_pos() {
                // Position-based, through smart aim, exactly as egui's own slider does it.
                let aim = ui.input(|i| i.aim_radius());
                let at = |x: f32| {
                    slider_map::value_of(
                        egui::remap_clamp(x, x_range, 0.0..=1.0),
                        min,
                        max,
                        log,
                    ) as f64
                };
                let (a, b) = (at(p.x - aim), at(p.x + aim));
                *value = emath::smart_aim::best_in_range_f64(a.min(b), a.max(b)) as f32;
            }
            if resp.clicked() {
                resp.request_focus();
            }
            if resp.has_focus() {
                ui.memory_mut(|m| {
                    m.set_focus_lock_filter(
                        resp.id,
                        egui::EventFilter {
                            horizontal_arrows: true,
                            ..Default::default()
                        },
                    );
                });
                let steps = ui.input(|i| {
                    i.num_presses(egui::Key::ArrowRight) as i32
                        - i.num_presses(egui::Key::ArrowLeft) as i32
                });
                if steps != 0 {
                    let t = slider_map::t_of(*value, min, max, log) + steps as f32 * KEY_STEP;
                    *value = slider_map::value_of(t, min, max, log);
                }
            }
            if *value != old {
                resp.mark_changed();
            }
            let now = *value as f64;
            resp.widget_info(|| egui::WidgetInfo::slider(ui.is_enabled(), now, label));
            ui.ctx().accesskit_node_builder(resp.id, |b| {
                use accesskit::Action;
                b.set_min_numeric_value(min as f64);
                b.set_max_numeric_value(max as f64);
                b.add_action(Action::SetValue);
                if *value < max {
                    b.add_action(Action::Increment);
                }
                if *value > min {
                    b.add_action(Action::Decrement);
                }
            });

            if ui.is_rect_visible(rect) {
                let v = ui.visuals();
                let painter = ui.painter_at(rect);
                let rail = egui::Rect::from_x_y_ranges(
                    x_range,
                    egui::Rangef::new(
                        rect.center().y - RAIL_HEIGHT / 2.0,
                        rect.center().y + RAIL_HEIGHT / 2.0,
                    ),
                );
                painter.rect_filled(rail, 1.5, v.widgets.inactive.bg_fill);
                let t = slider_map::t_of(*value, min, max, log);
                let (lo, hi) = slider_map::fill_span(t, min, max);
                let x = |t: f32| egui::lerp(x_range, t);
                painter.rect_filled(
                    egui::Rect::from_x_y_ranges(x(lo)..=x(hi), rail.y_range()),
                    1.5,
                    v.selection.bg_fill,
                );
                if let Some(z) = slider_map::zero_t(min, max) {
                    painter.line_segment(
                        [
                            egui::pos2(x(z), rail.top() - 2.0),
                            egui::pos2(x(z), rail.bottom() + 2.0),
                        ],
                        egui::Stroke::new(1.0, v.widgets.noninteractive.bg_stroke.color),
                    );
                }
                let c = egui::pos2(x(t), rail.center().y);
                painter.circle_filled(c, HANDLE_RADIUS, v.strong_text_color());
                if resp.hovered() || resp.dragged() || resp.has_focus() {
                    painter.circle_stroke(
                        c,
                        HANDLE_RADIUS,
                        egui::Stroke::new(1.5, v.selection.bg_fill),
                    );
                }
            }

            // ── value ──
            // ⚠ A `DragValue` drawn frameless so it reads as text: typing, drag-on-the-number
            // and the formatter/parser come with it, and a zero on a bipolar row is dimmed so
            // untouched rows recede.
            let dim = bipolar && *value == 0.0;
            let value_resp = ui
                .allocate_ui_with_layout(
                    egui::vec2(VALUE_WIDTH, height),
                    egui::Layout::right_to_left(egui::Align::Center),
                    |ui| {
                        let vis = ui.visuals_mut();
                        vis.widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
                        vis.widgets.hovered.weak_bg_fill = egui::Color32::TRANSPARENT;
                        vis.widgets.inactive.bg_stroke = egui::Stroke::NONE;
                        vis.widgets.hovered.bg_stroke = egui::Stroke::NONE;
                        if dim {
                            let weak = vis.weak_text_color();
                            vis.widgets.inactive.fg_stroke.color = weak;
                        }
                        let mut dv = egui::DragValue::new(value)
                            .range(range.clone())
                            .speed(((max - min) / 200.0).max(1e-3));
                        dv = match formatter {
                            Some(f) => dv.custom_formatter(move |v, _| f(v)),
                            None => dv.min_decimals(0).max_decimals(2),
                        };
                        if let Some(p) = parser {
                            dv = dv.custom_parser(p);
                        }
                        ui.add(dv)
                    },
                )
                .inner;
            resp.union(value_resp)
        })
        .inner
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_ui;

    /// Press at `from`, move to `to`, release — five passes, `drag_through`'s count, because a
    /// harness that moves on the same pass as the press produces a gesture that grabs nothing
    /// while looking exactly like one that works.
    fn drag(ctx: &egui::Context, from: egui::Pos2, to: egui::Pos2, mut body: impl FnMut(&mut egui::Ui)) {
        for pass in 0..5 {
            let events = match pass {
                1 => vec![egui::Event::PointerMoved(from)],
                2 => vec![egui::Event::PointerButton {
                    pos: from,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                }],
                3 => vec![egui::Event::PointerMoved(to)],
                4 => vec![egui::Event::PointerButton {
                    pos: to,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                }],
                _ => Vec::new(),
            };
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(test_ui::screen()),
                    events,
                    ..Default::default()
                },
                &mut body,
            );
        }
    }

    #[test]
    fn the_row_is_found_by_its_label_and_a_drag_moves_and_settles_it() {
        let ctx = test_ui::ctx();
        let mut v = 0.0f32;
        let mut settles = 0;
        let mut changes = 0;
        let mut body = |ui: &mut egui::Ui| {
            let r = ui.add(Slider::new(&mut v, -100.0..=100.0).text("Contrast"));
            changes += r.changed() as u32;
            settles += r.drag_stopped() as u32;
        };
        let at = test_ui::find(&ctx, "Contrast", &mut body).expect("labelled by its text");
        drag(&ctx, at, at + egui::vec2(60.0, 0.0), &mut body);
        assert!(v > 0.0, "a drag to the right moved nothing: {v}");
        assert!(changes >= 1, "no frame reported a change");
        assert_eq!(settles, 1, "one release, one settle");
    }

    #[test]
    fn a_press_without_motion_at_the_centre_of_a_bipolar_rail_leaves_zero() {
        // ⚠ The fill-from-zero geometry puts the handle at the rail's centre for 0 on a
        // symmetric range; the rail's accesskit centre is that point, so a press there must
        // resolve to 0 — a slider that reset to some other value on a stray click would be
        // reported as "the panel changes my photo".
        let ctx = test_ui::ctx();
        let mut v = 0.0f32;
        let mut body = |ui: &mut egui::Ui| {
            let _ = ui.add(Slider::new(&mut v, -100.0..=100.0).text("Tint"));
        };
        let at = test_ui::find(&ctx, "Tint", &mut body).expect("drawn");
        drag(&ctx, at, at, &mut body);
        assert_eq!(v, 0.0);
    }

    #[test]
    fn a_log_rail_reports_its_value_through_the_formatter() {
        // The retouch Size row: log scale plus a "px" formatter, driven through the builder.
        let ctx = test_ui::ctx();
        let mut v = 10.0f32;
        let values = test_ui::numeric_values(&ctx, |ui| {
            let _ = ui.add(
                Slider::new(&mut v, 1.0..=100.0)
                    .text("Size")
                    .logarithmic(true)
                    .custom_formatter(|v| format!("{v:.0} px")),
            );
        });
        assert!(values.contains(&10.0), "the value never reached accessibility: {values:?}");
    }
}
```

- [ ] **Step 2: Add the module and delegate.** In `panel/mod.rs` add `pub mod slider;` after `pub mod retouch;`. Replace the body of `Edited::slider`:

```rust
        let r = ui.add(slider::Slider::new(value, range).text(label));
```

- [ ] **Step 3: Build and run the panel tests.** `cargo test -p app --bin Rustroom 2>&1 | tail -15`. Expected: compiles (⚠ if `accesskit` is not a direct dependency of `app`, replace the `accesskit_node_builder` block with nothing — `WidgetInfo::slider` alone carries label and value, which is what every harness reads — and note it in the commit), all three new tests pass, and **every existing panel harness passes unchanged**: `drag_in_panel`, `drag_slider`, the geometry and adjustments drags. That is B's acceptance test.

- [ ] **Step 4: Gate.**

- [ ] **Step 5: Commit**

```bash
git add crates/app/src/develop/panel/slider.rs crates/app/src/develop/panel/mod.rs
git commit -m "The panel slider row: label, a rail that fills from zero, the value as text"
```

### Task B3: The four sites that bypass `Edited::slider`

**Files:**
- Modify: `crates/app/src/develop/panel/adjustments.rs` (Temp, in `white_balance`)
- Modify: `crates/app/src/develop/panel/retouch.rs` (Size, Feather, Threshold)

- [ ] **Step 1: Temp.** Replace the `ui.add(egui::Slider::new(&mut mired, ...))` expression with

```rust
    let r = ui.add(
        super::slider::Slider::new(&mut mired, MIRED_MIN..=MIRED_MAX)
            .text("Temp")
            .custom_formatter(|m| format!("{:.0} K", 1.0e6 / m))
            .custom_parser(|s| {
                s.trim()
                    .trim_end_matches(['K', 'k', ' '])
                    .parse::<f64>()
                    .ok()
                    .filter(|k| *k > 0.0)
                    .map(|k| 1.0e6 / k)
            }),
    );
```

- [ ] **Step 2: Retouch.** Size:

```rust
    let r = ui
        .add(
            super::slider::Slider::new(
                &mut px,
                spot_tool::radius_px(MIN_RADIUS, long)..=spot_tool::radius_px(MAX_RADIUS, long),
            )
            .logarithmic(true)
            .text("Size")
            .custom_formatter(|v| format!("{v:.0} px")),
        )
        .on_hover_text(
            "The patch's radius in pixels of the full-resolution image. Dragging out on the \
             canvas sets this too.",
        );
```
Feather: `ui.add(super::slider::Slider::new(feather, 0.0..=100.0).text("Feather"))`. Threshold: `ui.add(super::slider::Slider::new(t, MIN..=MAX).text("Threshold"))` with the same range expression as today. Keep every comment and every `.on_hover_text` as it is.

- [ ] **Step 3: Verify nothing constructs `egui::Slider` under `develop/` any more**

Run: `grep -rn "egui::Slider" crates/app/src/develop` — Expected: no output.

- [ ] **Step 4: Gate.** ⚠ The retouch tests read `dragged()`; they must pass unchanged.

- [ ] **Step 5: Commit**

```bash
git add crates/app/src/develop/panel/adjustments.rs crates/app/src/develop/panel/retouch.rs
git commit -m "Temp, Size, Feather and Threshold take the panel slider row"
```

- [ ] **Step 6: Launch the GUI.** Tell the user: every slider is label / rail / value; Contrast and Tint at 0 show no accent; drag Exposure and check the value lands on two decimals; click a value to type; the retouch Size rail is logarithmic.

---

# Series C — section chrome, sub-labels, segmented controls, histogram

### Task C1: `Edits::group_differs`

**Files:**
- Modify: `crates/edit-model/src/lib.rs` — after `merge_from` in the `impl Edits` block that holds it; a test beside `all_nonneutral`'s users

- [ ] **Step 1: Write the failing test** (in `mod tests`, after the last test that uses `all_nonneutral`):

```rust
    #[test]
    fn group_differs_names_exactly_the_group_that_moved() {
        // ⚠ Both halves: "always true" fails the identity row, "always false" fails every
        // moved row. Swept over every group via the exhaustive fixture, so a group whose
        // fields never differ from neutral would surface here as a fixture gap.
        let neutral = Edits::default();
        let full = all_nonneutral();
        let n = EDIT_GROUP_NAMES.len();
        for g in 0..n {
            let mut one = neutral.clone();
            one.merge_from(&full, EditGroups::only(g));
            for h in 0..n {
                assert_eq!(
                    one.group_differs(&neutral, h),
                    g == h,
                    "moved group {g} ({}), asked about {h} ({})",
                    EDIT_GROUP_NAMES[g],
                    EDIT_GROUP_NAMES[h]
                );
            }
        }
        for h in 0..n {
            assert!(!neutral.group_differs(&neutral, h), "identity differs in {h}");
        }
    }
```

- [ ] **Step 2: Run** `cargo test -p edit-model group_differs 2>&1 | tail -5` — Expected: compile error, no such method.

- [ ] **Step 3: Implement**, directly after `merge_from`'s closing brace:

```rust
    /// Whether `self` and `other` differ inside edit group `group`.
    ///
    /// ⚠ Defined *through* [`Self::merge_from`] rather than as a second field list, so it
    /// cannot disagree with the partition: merge `self`'s group into a clone of `other` and
    /// see whether anything moved. Costs one clone per call — the Develop panel asks six
    /// times a frame, which is a few kilobytes; if a profile ever shows it, cache the six
    /// answers on a change rather than inventing a second partition.
    pub fn group_differs(&self, other: &Edits, group: usize) -> bool {
        let mut probe = other.clone();
        probe.merge_from(self, EditGroups::only(group));
        probe != *other
    }
```

- [ ] **Step 4: Run** the same command — Expected: `1 passed`.

- [ ] **Step 5: Commit**

```bash
git add crates/edit-model/src/lib.rs
git commit -m "Edits::group_differs, defined through merge_from"
```

### Task C2: The section text style, `sub_label`, `segmented`

**Files:**
- Modify: `crates/app/src/theme.rs`

- [ ] **Step 1: Write the failing tests** in `theme`'s `mod tests`:

```rust
    #[test]
    fn the_section_style_is_installed_in_both_themes() {
        let ctx = egui::Context::default();
        apply(&ctx, &Settings::default());
        for theme in [egui::Theme::Dark, egui::Theme::Light] {
            let s = ctx.style_of(theme);
            let section = s.text_styles.get(&section_style()).map(|f| f.size);
            let body = s.text_styles[&egui::TextStyle::Body].size;
            assert!(section.is_some_and(|x| x < body), "{theme:?}: section {section:?} vs body {body}");
        }
    }

    #[test]
    fn every_segment_is_reachable_by_its_label_and_a_click_selects_only_it() {
        #[derive(Clone, Copy, PartialEq, Debug)]
        enum C {
            A,
            B,
            D,
        }
        let items = [(C::A, "Hue"), (C::B, "Saturation"), (C::D, "Luminance")];
        let ctx = crate::test_ui::ctx();
        let mut cur = C::A;
        let found = crate::test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
            segmented(ui, &mut cur, &items);
        });
        for (_, l) in items {
            assert!(found.iter().any(|(f, _)| f == l), "{l} not reachable: {found:?}");
        }
        crate::test_ui::click(&ctx, "Luminance", |ui| {
            segmented(ui, &mut cur, &items);
        });
        assert_eq!(cur, C::D);
        crate::test_ui::click(&ctx, "Saturation", |ui| {
            segmented(ui, &mut cur, &items);
        });
        assert_eq!(cur, C::B);
    }
```

- [ ] **Step 2: Run** `cargo test -p app --bin Rustroom theme:: 2>&1 | tail -6` — Expected: compile error.

- [ ] **Step 3: Implement.** In `tune`, after the radius loop:

```rust
    // The section header and sub-label face: smaller than body, uppercased by its callers.
    style
        .text_styles
        .insert(section_style(), egui::FontId::proportional(11.0));
```

After `caption`, add:

```rust
/// The named text style section headers and sub-labels use.
///
/// ⚠ A named style rather than `Small`: `Small` is the hint face at 9 pt, and a header at hint
/// size is not a header.
pub fn section_style() -> egui::TextStyle {
    egui::TextStyle::Name("section".into())
}

/// A sub-label: the name of a block of controls inside a section — Presence, Noise Reduction,
/// Color Grading. Uppercased, in the section face, in the weak text colour, with a little air
/// above and no hairline: the hairline was the old idiom and the name replaces it.
///
/// ⚠ Not for a value ("No presets yet.") and not for a control's own text; those stay
/// `label`s. Plain labels produce no AccessKit node, so nothing in the tests reaches these.
pub fn sub_label(ui: &mut egui::Ui, text: &str) {
    ui.add_space(4.0);
    let colour = ui.visuals().weak_text_color();
    ui.add(
        egui::Label::new(
            egui::RichText::new(text.to_uppercase())
                .text_style(section_style())
                .color(colour),
        )
        .selectable(false),
    );
}

/// A segmented control: one full-width well, `items.len()` equal columns, exactly one lit.
///
/// ⚠ For a **view switch** over a fixed small set — the HSL channel, the grading range — not
/// for a list of options with its own reasons to be `selectable_label`s (aspect presets, looks,
/// composition guides). Each column carries `WidgetInfo::selected` with its label, so
/// `test_ui::click(ctx, "Saturation", ..)` reaches it exactly as it reached the chips.
pub fn segmented<T: PartialEq + Copy>(
    ui: &mut egui::Ui,
    current: &mut T,
    items: &[(T, &str)],
) -> egui::Response {
    const HEIGHT: f32 = 18.0;
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), HEIGHT),
        egui::Sense::hover(),
    );
    let v = ui.visuals().clone();
    let font = section_style().resolve(ui.style());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, v.extreme_bg_color);
    let inner = rect.shrink(2.0);
    let w = inner.width() / items.len().max(1) as f32;
    let mut all = resp.clone();
    for (i, (item, label)) in items.iter().enumerate() {
        let cell = egui::Rect::from_min_size(
            egui::pos2(inner.left() + w * i as f32, inner.top()),
            egui::vec2(w, inner.height()),
        );
        let r = ui.interact(cell, resp.id.with(i), egui::Sense::click());
        let enabled = ui.is_enabled();
        let selected = *current == *item;
        r.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, enabled, selected, *label)
        });
        if r.clicked() {
            *current = *item;
        }
        let lit = *current == *item;
        if lit {
            painter.rect_filled(cell, 3.0, v.widgets.inactive.bg_fill);
        } else if r.hovered() {
            painter.rect_filled(cell, 3.0, v.widgets.hovered.weak_bg_fill);
        }
        painter.text(
            cell.center(),
            egui::Align2::CENTER_CENTER,
            *label,
            font.clone(),
            if lit {
                v.strong_text_color()
            } else {
                v.text_color()
            },
        );
        all = all.union(r);
    }
    all
}
```

⚠ `sub_label`, `segmented` and `section_style` are `pub` in a **binary** crate. `section_style` has a production caller in `tune`; the other two do not until C3–C5, and `clippy -D warnings` will reject them. **Land C2 together with C3 (which uses `section_style` and `sub_label`) and C5 (which uses `segmented`) as one commit**, or add the callers before running the gate.

### Task C3: The section header

**Files:**
- Modify: `crates/app/src/develop/panel/adjustments.rs` — `section`, every `section(..)` call in `show`, the tone Presence hairline; tests

- [ ] **Step 1: Write the failing tests** in `adjustments`'s `mod tests`, using the existing `RangePanel` harness (it draws the whole panel and clicks by label):

```rust
    #[test]
    fn a_modified_section_shows_its_reset_and_a_neutral_one_does_not() {
        // ⚠ Both halves: a reset that always shows and one that never shows are equally wrong,
        // and only the pair distinguishes them. Found by the `reset` label through AccessKit.
        let ctx = instant_ctx();
        let mut p = RangePanel::new(false);
        let drawn = |p: &mut RangePanel, ctx: &egui::Context| {
            let mut n = 0;
            p.draw(ctx, |labels| n = labels.iter().filter(|(l, _)| l == "reset").count());
            n
        };
        assert_eq!(drawn(&mut p, &ctx), 0, "a neutral panel offered a reset");
        p.edits.contrast = 30.0;
        assert_eq!(drawn(&mut p, &ctx), 1, "exactly the Tone section should offer a reset");
    }

    #[test]
    fn resetting_a_section_leaves_the_other_sections_alone() {
        let ctx = instant_ctx();
        let mut p = RangePanel::new(false);
        p.edits.contrast = 30.0;
        p.edits.vibrance = 20.0;
        p.click(&ctx, "reset"); // the only reset drawn is Tone's: Color is collapsed by default
        assert_eq!(p.edits.contrast, 0.0, "Tone was not reset");
        assert_eq!(p.edits.vibrance, 20.0, "Color was reset too");
        assert!(p.ed.settled, "one click, one history entry");
    }
```

Add to `impl RangePanel` a `draw` helper beside `click`, which lays the panel out once and hands the labelled centres to `f`:

```rust
        /// Lay the panel out once; `f` sees every AccessKit label drawn.
        fn draw(&mut self, ctx: &egui::Context, f: impl FnOnce(&[(String, egui::Pos2)])) {
            let Self { edits, channel, grade, show_range, view, ed, buf, looks } = self;
            let labels = crate::test_ui::labelled_centres(ctx, Vec::new(), |ui| {
                *ed = Edited::default();
                show(
                    ui,
                    edits,
                    channel,
                    GradeUi { range: grade, show_range, view },
                    SourceInfo {
                        lens: None,
                        lens_profile_found: false,
                        wb_baseline: color::wb::Illuminant::new(5000.0, 0.0),
                        baseline_ev: None,
                        buffer: buf,
                        looks,
                    },
                    None,
                    ed,
                );
            });
            f(&labels);
        }
```
⚠ Copy the `SourceInfo { .. }` literal from `RangePanel::click` verbatim — the fields above are from memory of that harness; the harness is the source of truth.

- [ ] **Step 2: Run** `cargo test -p app --bin Rustroom adjustments::tests::a_modified 2>&1 | tail -5` — Expected: fails (no `reset` label is drawn; first assertion passes, second fails with `left: 0, right: 1`).

- [ ] **Step 3: Implement `section`.** Replace it with:

```rust
/// One collapsible section, headed by its edit group's name.
///
/// ⚠ A hand-drawn header over `CollapsingState`, not a `CollapsingHeader`: that widget draws
/// egui's triangle and indents its body, and has no slot for the two things this row carries —
/// an accent dot while the group differs from neutral, and a `reset` for exactly that group.
/// The body goes through `show_body_unindented`, so the left-hand gap under every section is
/// gone with the indent that made it.
///
/// ⚠ The AccessKit label is `s.name()` as written, not the uppercased text the row paints:
/// the harnesses click sections by name ("Color"), and the name is what a screen reader wants.
///
/// ⚠ `edits` and `ed` are passed *through* to the body rather than captured by it, because
/// the header reads `edits` for the dot before the body runs and writes it for the reset after
/// — a closure capturing `&mut edits` for its whole lifetime would be `E0499` against both.
fn section(
    ui: &mut egui::Ui,
    s: Section,
    open_by_default: bool,
    edits: &mut edit_model::Edits,
    ed: &mut Edited,
    body: impl FnOnce(&mut egui::Ui, &mut edit_model::Edits, &mut Edited),
) {
    const HEIGHT: f32 = 22.0;
    const CHEVRON_W: f32 = 14.0;
    let id = ui.make_persistent_id(s.name());
    let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
        ui.ctx(),
        id,
        open_by_default,
    );
    let modified = edits.group_differs(&edit_model::Edits::default(), s.index());

    ui.add_space(6.0);
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), HEIGHT),
        egui::Sense::click(),
    );
    let enabled = ui.is_enabled();
    resp.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::CollapsingHeader, enabled, s.name())
    });

    // ⚠ Registered after the row, so it wins the hit-test tie — egui takes the last widget
    // registered at a point, the rule the Library's rubber band relies on.
    let mut reset = false;
    let mut right = rect.right() - CHEVRON_W;
    let font = theme::section_style().resolve(ui.style());
    if modified {
        let weak = ui.visuals().weak_text_color();
        let galley = ui
            .painter()
            .layout_no_wrap("reset".to_owned(), font.clone(), weak);
        let cell = egui::Rect::from_min_max(
            egui::pos2(right - galley.size().x - 8.0, rect.top()),
            egui::pos2(right, rect.bottom()),
        );
        let r = ui
            .interact(cell, id.with("reset"), egui::Sense::click())
            .on_hover_text(format!("Reset {} and nothing else", s.name()));
        r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, "reset"));
        reset = r.clicked();
        let colour = if r.hovered() {
            ui.visuals().strong_text_color()
        } else {
            weak
        };
        ui.painter().galley(
            egui::pos2(cell.left() + 4.0, cell.center().y - galley.size().y / 2.0),
            galley,
            colour,
        );
        right = cell.left() - 6.0;
    }
    if resp.clicked() && !reset {
        state.toggle(ui);
    }

    if ui.is_rect_visible(rect) {
        let v = ui.visuals().clone();
        let painter = ui.painter_at(rect);
        let text_rect = painter.text(
            egui::pos2(rect.left(), rect.center().y),
            egui::Align2::LEFT_CENTER,
            s.name().to_uppercase(),
            font,
            v.strong_text_color(),
        );
        if modified {
            painter.circle_filled(
                egui::pos2(text_rect.right() + 8.0, rect.center().y),
                2.5,
                v.selection.bg_fill,
            );
        }
        // The chevron: '›' closed, rotated to '⌄' as the section opens.
        let openness = state.openness(ui.ctx());
        let rot = egui::emath::Rot2::from_angle(std::f32::consts::FRAC_PI_2 * openness);
        let c = egui::pos2(right + CHEVRON_W / 2.0, rect.center().y);
        let pts = [
            egui::vec2(-2.0, -3.5),
            egui::vec2(1.5, 0.0),
            egui::vec2(-2.0, 3.5),
        ]
        .map(|p| c + rot * p)
        .to_vec();
        painter.add(egui::Shape::line(
            pts,
            egui::Stroke::new(1.4, v.weak_text_color()),
        ));
        painter.line_segment(
            [rect.left_bottom(), rect.right_bottom()],
            egui::Stroke::new(1.0, v.widgets.noninteractive.bg_stroke.color),
        );
    }

    state.show_body_unindented(ui, |ui| body(ui, edits, ed));

    if reset {
        edits.merge_from(&edit_model::Edits::default(), edit_model::EditGroups::only(s.index()));
        ed.discrete();
    }
}
```

- [ ] **Step 4: Update every `section(..)` call in `show`.** Each becomes `section(ui, SECTIONS[n], open, edits, ed, |ui, edits, ed| { ...same body... })`. The bodies already refer to `edits` and `ed` by those names, so the closure parameters shadow the captures and the bodies stay textually identical. `picking` (moved into the WB body) and the `SourceInfo` fields are captured as before. ⚠ `ui.separator()` calls *between* sections in `show` (there is one before the Reset adjustments button in `mod.rs`, none between sections) stay.

- [ ] **Step 5: Run** the two new tests and the whole `adjustments` module: `cargo test -p app --bin Rustroom adjustments:: 2>&1 | tail -8`. Expected: all pass, including `p.click(&ctx, "Color")` in the range-view tests (the header's AccessKit label is `"Color"`).

- [ ] **Step 6: Commit** together with C2:

```bash
git add crates/app/src/theme.rs crates/app/src/develop/panel/adjustments.rs
git commit -m "Section headers: uppercase, a chevron, a modified dot and a per-section reset"
```

### Task C4: Sub-labels

**Files:**
- Modify: `adjustments.rs`, `mask_adjust.rs`, `geometry.rs`, `tone_curve.rs`

- [ ] **Step 1: `adjustments.rs`.** In `tone`: replace the `ui.separator();` that precedes the Texture slider with `theme::sub_label(ui, "Presence");` (keep the comment above it, reworded: "A sub-label, not a header…"). Remove the `ui.separator();` before `tone_curve::show` — the curve now draws its own sub-label row. In the Color section: replace `ui.separator();\n        ui.label("Color Grading");` with `theme::sub_label(ui, "Color Grading");`. In Detail: `ui.label("Noise Reduction")` → `theme::sub_label(ui, "Noise Reduction")`; `ui.separator();\n        ui.label("Sharpening");` → `theme::sub_label(ui, "Sharpening");`. Add `use crate::theme;` if the file lacks it.

- [ ] **Step 2: `mask_adjust.rs`.** `ui.label("Noise Reduction")` → `theme::sub_label(ui, "Noise Reduction")`; `ui.separator();\n    ui.label("Sharpening");` → `theme::sub_label(ui, "Sharpening");`. Add `use crate::theme;` if missing.

- [ ] **Step 3: `geometry.rs`.** `ui.label("Lens")`, `"Geometry"`, `"Composition"`, `"Perspective"`, `"Guides"` → `theme::sub_label(ui, ..)` each; keep every separator. ⚠ `ui.label(":")` at line ~222 is the custom-aspect field's separator and stays.

- [ ] **Step 4: `tone_curve.rs`.** In `show`'s header row, `ui.label("Tone Curve");` → `theme::sub_label(ui, "Tone Curve");`. Add `use crate::theme;` if missing.

- [ ] **Step 5: Gate.** Expected green; no test names these labels (plain labels have no AccessKit node).

- [ ] **Step 6: Commit**

```bash
git add crates/app/src/develop/panel/adjustments.rs crates/app/src/develop/panel/mask_adjust.rs crates/app/src/develop/panel/geometry.rs crates/app/src/develop/panel/tone_curve.rs
git commit -m "Sub-labels name every block of controls, Presence included"
```

### Task C5: Segmented selectors and full grade labels

**Files:**
- Modify: `hsl_bands.rs`, `adjustments.rs` (grade selector + two tests + `click_segment`), `mask_adjust.rs`, `color_wheel.rs`

- [ ] **Step 1: Labels.** In `color_wheel.rs` `GradeRange::label`: `"Shad"` → `"Shadows"`, `"Mid"` → `"Midtones"`, `"High"` → `"Highlights"`. In `adjustments.rs` tests: `p.click(&ctx, "Mid")` → `"Midtones"`, `p.click(&ctx, "High")` → `"Highlights"`.

- [ ] **Step 2: Run** `cargo test -p app --bin Rustroom the_selector_drives 2>&1 | tail -4` — Expected: pass (labels renamed together).

- [ ] **Step 3: `hsl_bands.rs`.** Replace the `ui.horizontal(|ui| { for c in HslChannel::ALL { ui.selectable_value(..) } })` block with

```rust
    theme::segmented(ui, channel, &HslChannel::ALL.map(|c| (c, c.label())));
```
Add `use crate::theme;`.

- [ ] **Step 4: Grade selectors.** In `adjustments.rs`'s Color section and in `mask_adjust.rs`'s grading block, replace the `ui.horizontal(|ui| { for r in color_wheel::GradeRange::ALL { ui.selectable_value(..) } })` block with

```rust
            theme::segmented(ui, grade_range, &color_wheel::GradeRange::ALL.map(|r| (r, r.label())));
```
(`grade` instead of `grade_range` in `mask_adjust.rs`). ⚠ In `adjustments.rs` read the lines after the loop before editing: if the surrounding code compares the range before and after the selector to drive the grade-range view, keep that logic and replace only the widget.

- [ ] **Step 5: Rewrite `click_segment`** in `adjustments`' tests so it drives the shipped selector rather than re-declaring `selectable_value`:

```rust
    /// Click `want`'s segment in the real HSL selector and report the channel afterwards.
    fn click_segment(want: HslChannel) -> HslChannel {
        let ctx = crate::test_ui::ctx();
        let mut channel = HslChannel::Hue;
        let mut hsl = edit_model::Hsl::default();
        let mut ed = Edited::default();
        crate::test_ui::click(&ctx, want.label(), |ui| {
            hsl_bands::show(ui, &mut hsl, &mut channel, &mut ed);
        });
        channel
    }
```
Keep `clicking_a_segment_switches_the_channel` as it is; its doc comment's "re-declares the loop" sentence goes.

- [ ] **Step 6: Gate.** Expected green, including the mask panel's chip tests.

- [ ] **Step 7: Commit**

```bash
git add crates/app/src/develop/panel/hsl_bands.rs crates/app/src/develop/panel/adjustments.rs crates/app/src/develop/panel/mask_adjust.rs crates/app/src/develop/panel/color_wheel.rs
git commit -m "One segmented control for the HSL channel and the grading range, with full names"
```

### Task C6: The histogram well and the tone curve's ghost

**Files:**
- Modify: `crates/app/src/histogram.rs` (lib) — `clipped_edges`
- Modify: `crates/app/src/develop/panel/mod.rs` — `draw_histogram`, pass `hist`
- Modify: `crates/app/src/develop/panel/adjustments.rs` — `SourceInfo.hist`, `tone` passes it
- Modify: `crates/app/src/develop/panel/tone_curve.rs` — `show(.., hist)`, the well, the ghost
- Modify: `crates/app/src/develop/panel/mask_adjust.rs` — passes `None`

- [ ] **Step 1: Failing test** in `histogram.rs`:

```rust
    #[test]
    fn the_clipping_marker_needs_more_than_a_single_pixel() {
        // ⚠ A marker that fires on the one specular every frame has is a marker nobody reads;
        // one that never fires on a blown sky is worse. 0.5% of the frame is the line.
        let mut h = [[0u32; 256]; 3];
        h[0][128] = 1000;
        h[1][128] = 1000;
        h[2][128] = 1000;
        h[0][255] = 1; // one hot pixel in red
        assert_eq!(clipped_edges(&h), (false, false));
        h[2][0] = 10; // 1% of blue at black
        assert_eq!(clipped_edges(&h), (true, false));
        h[0][255] = 10;
        assert_eq!(clipped_edges(&h), (true, true));
        assert_eq!(clipped_edges(&[[0; 256]; 3]), (false, false), "an empty frame clips nothing");
    }
```

- [ ] **Step 2: Implement** after `histogram_rgba`:

```rust
/// The share of a frame's samples one edge bin must hold for the panel to mark clipping.
pub const CLIP_FRACTION: f64 = 0.005;

/// Whether the shadows (bin 0) and the highlights (bin 255) are clipped in any channel:
/// `(left, right)`.
pub fn clipped_edges(hist: &[[u32; 256]; 3]) -> (bool, bool) {
    let total: u64 = hist[0].iter().map(|&c| c as u64).sum();
    if total == 0 {
        return (false, false);
    }
    let limit = (total as f64 * CLIP_FRACTION) as u64;
    let left = hist.iter().any(|c| c[0] as u64 > limit);
    let right = hist.iter().any(|c| c[255] as u64 > limit);
    (left, right)
}
```

- [ ] **Step 3: Run** `cargo test -p app --lib histogram 2>&1 | tail -4` — Expected: pass.

- [ ] **Step 4: The painter.** Replace `draw_histogram` in `panel/mod.rs`:

```rust
/// The histogram's channel colours. ⚠ Hand-painted constants outside the accent system, as
/// everything painted by hand is (visual identity spec, caveat 7); named so the tone curve's
/// ghost can share the idea without a second copy of the numbers.
pub(crate) const HIST_HEIGHT: f32 = 72.0;
const CHANNEL_COLOURS: [egui::Color32; 3] = [
    egui::Color32::from_rgb(220, 80, 80),
    egui::Color32::from_rgb(80, 200, 80),
    egui::Color32::from_rgb(90, 120, 230),
];

/// A filled area under a 256-bin curve, as one mesh of 255 quads.
pub(crate) fn area_mesh(rect: egui::Rect, bins: &[f32; 256], colour: egui::Color32) -> egui::Mesh {
    let mut mesh = egui::Mesh::default();
    let x = |i: usize| rect.left() + rect.width() * (i as f32 / 255.0);
    let y = |v: f32| rect.bottom() - rect.height() * v.clamp(0.0, 1.0);
    for i in 0..255 {
        let base = mesh.vertices.len() as u32;
        mesh.colored_vertex(egui::pos2(x(i), rect.bottom()), colour);
        mesh.colored_vertex(egui::pos2(x(i), y(bins[i])), colour);
        mesh.colored_vertex(egui::pos2(x(i + 1), rect.bottom()), colour);
        mesh.colored_vertex(egui::pos2(x(i + 1), y(bins[i + 1])), colour);
        mesh.add_triangle(base, base + 1, base + 2);
        mesh.add_triangle(base + 1, base + 3, base + 2);
    }
    mesh
}

fn draw_histogram(ui: &mut egui::Ui, hist: &[[u32; 256]; 3]) {
    let max = hist
        .iter()
        .flat_map(|c| c.iter())
        .copied()
        .max()
        .unwrap_or(1)
        .max(1) as f32;
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), HIST_HEIGHT),
        egui::Sense::hover(),
    );
    if !ui.is_rect_visible(rect) {
        return;
    }
    let v = ui.visuals().clone();
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, v.extreme_bg_color);
    let plot = rect.shrink2(egui::vec2(0.0, 4.0));
    for (ci, chan) in hist.iter().enumerate() {
        let bins: [f32; 256] = std::array::from_fn(|i| chan[i] as f32 / max);
        painter.add(egui::Shape::mesh(area_mesh(
            plot,
            &bins,
            CHANNEL_COLOURS[ci].linear_multiply(0.55),
        )));
    }
    let (left, right) = rustroom_app::histogram::clipped_edges(hist);
    let mark = |corner: egui::Pos2, dir: f32| {
        painter.add(egui::Shape::convex_polygon(
            vec![
                corner,
                corner + egui::vec2(6.0 * dir, 0.0),
                corner + egui::vec2(0.0, 6.0),
            ],
            v.strong_text_color(),
            egui::Stroke::NONE,
        ));
    };
    if left {
        mark(rect.left_top() + egui::vec2(1.0, 1.0), 1.0);
    }
    if right {
        mark(rect.right_top() + egui::vec2(-1.0, 1.0), -1.0);
    }
}
```
⚠ If `egui::Shape::mesh` wants an `Arc<Mesh>` in this egui, wrap: `egui::Shape::mesh(std::sync::Arc::new(..))`. Check the compiler, not memory.

- [ ] **Step 5: Thread `hist` to the tone curve.** In `adjustments.rs` add to `SourceInfo`:

```rust
    /// The rendered frame's histogram, for the ghost behind the tone curve. `None` in a
    /// harness that has no render.
    pub hist: Option<&'a [[u32; 256]; 3]>,
```
`tone` gains a parameter `hist: Option<&[[u32; 256]; 3]>` after `looks`, and passes it: `tone_curve::show(ui, &mut edits.tone_curve, ed, hist);`. `show` destructures it from `source` and passes it into the Tone section's body. In `panel/mod.rs` the `SourceInfo { .. }` literal gains `hist: Some(&s.hist),` (⚠ `s.hist` is a field of `Session` — a shared borrow disjoint from `&mut s.edits`, like `buffer`). Every other `SourceInfo { .. }` literal — `grep -rn "SourceInfo {" crates/app/src` — gains `hist: None,`.

In `tone_curve.rs`: `pub fn show(ui, curve, ed, hist: Option<&[[u32; 256]; 3]>) -> egui::Rect`; `mask_adjust.rs` passes `None`. In the paint block:

```rust
        painter.rect_filled(rect, 4.0, v.extreme_bg_color);
        if let Some(h) = hist {
            let max = h.iter().flat_map(|c| c.iter()).copied().max().unwrap_or(1).max(1) as f32;
            let bins: [f32; 256] =
                std::array::from_fn(|i| h.iter().map(|c| c[i]).max().unwrap_or(0) as f32 / max);
            painter.add(egui::Shape::mesh(super::area_mesh(
                rect,
                &bins,
                v.weak_text_color().linear_multiply(0.15),
            )));
        }
```
and delete the closing `painter.rect_stroke(rect, CornerRadius::ZERO, ..)` outline.

- [ ] **Step 6: Gate.**

- [ ] **Step 7: Commit**

```bash
git add crates/app/src/histogram.rs crates/app/src/develop/panel/mod.rs crates/app/src/develop/panel/adjustments.rs crates/app/src/develop/panel/tone_curve.rs crates/app/src/develop/panel/mask_adjust.rs
git commit -m "A framed histogram with clipping marks, and its ghost behind the tone curve"
```

- [ ] **Step 8: Launch the GUI.** Tell the user: section headers are uppercase with a chevron; edit Contrast and watch Tone gain a dot and a `reset`; Presence, Noise Reduction, Sharpening and Color Grading are sub-labels; the HSL and grading selectors are one full-width control with full names; the histogram is a filled well and the curve carries its ghost; a blown sky puts a marker in the histogram's top-right corner.

---

# Series D2 — the icon rail

### Task D1: Icon-only entries with tooltips

**Files:**
- Modify: `crates/app/src/develop/rail.rs`

- [ ] **Step 1: Failing test** in `rail`'s `mod tests`:

```rust
    #[test]
    fn every_rail_entry_is_labelled_for_accessibility() {
        // ⚠ The rail paints no text any more; the tool's name reaches a sighted user as a
        // tooltip and everyone else — a screen reader, this harness — as the accessible name.
        // Sabotage: drop `widget_info` and every label vanishes.
        let ctx = crate::test_ui::ctx();
        let found = crate::test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
            for t in Tool::ALL {
                let _ = entry(ui, t, false);
            }
        });
        for t in Tool::ALL {
            assert!(
                found.iter().any(|(l, _)| l == t.label()),
                "{} has no accessible name: {found:?}",
                t.label()
            );
        }
    }
```

- [ ] **Step 2: Run** `cargo test -p app --bin Rustroom rail:: 2>&1 | tail -5` — Expected: the new test fails (hand-painted text has no AccessKit node).

- [ ] **Step 3: Implement.** Replace the constants and `entry`:

```rust
/// Rail width in points. ⚠ Icon-only since 2026-09-19: the labels the first rail was sized for
/// ("Presets fits on one line" at 88 pt) are the tooltip now, and the width is what the glyph
/// and its padding need.
const RAIL_WIDTH: f32 = 44.0;

/// The glyph's point size.
const GLYPH_SIZE: f32 = 22.0;

/// Padding around the glyph.
const PAD: f32 = 6.0;

/// How much of the accent the selected entry's tint carries. ⚠ A tint, not a solid block and
/// not a bar: the accent should not shout beside the image, the visual identity spec's "low
/// chroma near the image".
const TINT: f32 = 0.18;
```

```rust
/// One rail entry: the glyph centred in a full-width clickable frame, its name on hover.
///
/// ⚠ The name is set **twice**, and both matter: `on_hover_text` is what a sighted user gets,
/// `widget_info` is what a screen reader and `test_ui` get. Dropping either leaves one of them
/// with an unlabelled button.
fn entry(ui: &mut egui::Ui, t: Tool, picked: bool) -> egui::Response {
    let height = PAD * 2.0 + GLYPH_SIZE;
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::click(),
    );
    let resp = resp.on_hover_text(t.label());
    let enabled = ui.is_enabled();
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, t.label()));

    let v = ui.visuals();
    let accent = v.selection.bg_fill;
    let (fill, colour) = if picked {
        (accent.gamma_multiply(TINT), accent)
    } else if resp.hovered() {
        (v.widgets.hovered.weak_bg_fill, v.text_color())
    } else {
        (egui::Color32::TRANSPARENT, v.text_color())
    };
    let painter = ui.painter();
    if fill != egui::Color32::TRANSPARENT {
        painter.rect_filled(rect.shrink(2.0), 4.0, fill);
    }
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        t.glyph(),
        egui::FontId::proportional(GLYPH_SIZE),
        colour,
    );
    resp
}
```
Delete `LABEL_SIZE` and `GAP`. In `show`, give the panel a tight frame so the tint rect is not squeezed by egui's default side-panel margin:

```rust
    egui::Panel::right("rail")
        .resizable(false)
        .default_size(RAIL_WIDTH)
        .frame(egui::Frame::side_top_panel(&ui.style()).inner_margin(4))
        .show(ui, |ui| {
```
Update the module doc's first line and the `entry` doc (the two-line-label story) to say what is there now; keep the two failed-attempt paragraphs only if they still describe something in the code (they do not — remove them).

- [ ] **Step 4: Gate.** Expected green; `clicking_a_rail_entry_selects_that_tool` passes untouched.

- [ ] **Step 5: Commit**

```bash
git add crates/app/src/develop/rail.rs
git commit -m "The rail is icon-only: 44 pt, a tint for the selected tool, its name on hover"
```

- [ ] **Step 6: Launch the GUI.** Tell the user: the rail is icons only; hover shows the name; the selected tool is tinted, no bar; the canvas gained 44 pt.

---

# Close

- [ ] Run the gate once more on the branch tip (⚠ *after* the last commit — a gate run before it describes a tree nobody shipped). Confirm `grep -c golden $L/test.log` matches `main`'s.
- [ ] `git log main..HEAD --oneline` — expect the spec, the plan, and eight to nine code commits; `git status` clean.
- [ ] Report: what landed, the file sizes (`adjustments.rs`, `masks.rs`, `color_wheel.rs` over 600), the two caveats to check by use (light mode; the retouch `dragged()` behaviour), and that the branch is unmerged and unpushed.
