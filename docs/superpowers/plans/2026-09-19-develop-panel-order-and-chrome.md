# Develop panel order and chrome — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this
> plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Smooth the wheel rim, move the section chevron left of the name so `reset` stands alone
on the right, gather the three rendering controls into a Profile block above White Balance, and
re-order White Balance, Tone and Color as the spec agrees.

**Architecture:** One new bin module holds the section header (`section.rs`), one holds the
rendering controls (`profile.rs`). `adjustments::section` and the Profile block in `panel/mod.rs`
both call `section::header`. Nothing in `edit-model` or the pipeline changes.

**Tech Stack:** Rust, egui 0.35, the AccessKit harness in `crate::test_ui`.

**Spec:** `docs/superpowers/specs/2026-09-19-develop-panel-order-and-chrome-design.md`.

**Branch:** stays on `develop-panel-restyle`. No `Co-Authored-By` trailer. Never push.

**Gate recipe** (each exit code read from its own command, logs in the scratchpad):
```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo fmt --all -- --check; echo FMT=$?
cargo clippy --workspace --all-targets -- -D warnings; echo CLIPPY=$?
cargo test --workspace; echo TEST=$?
```
⚠ Every fresh test binary stalls ~22 s before `main` on this Mac (macOS launch assessment). Budget
for it; it is not a hang.

---

### Task 1: The wheel's feather ring

**Files:** modify `crates/app/src/develop/panel/color_wheel.rs`

- [ ] **Step 1: the failing test** (in `mod tests`):

```rust
/// The wheel mesh as drawn: its vertices, read from the frame's shapes.
fn wheel_vertices() -> Vec<egui::epaint::Vertex> {
    let ctx = egui::Context::default();
    let mut grade = ColorGrading::default();
    let mut ed = Edited::default();
    let out = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(300.0, 400.0))),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                show(ui, &mut grade, GradeRange::Shadows, 200.0, &mut ed);
            });
        },
    );
    out.shapes
        .into_iter()
        .find_map(|c| match c.shape {
            egui::Shape::Mesh(m) if m.vertices.len() > 100 => Some(m.vertices.clone()),
            _ => None,
        })
        .expect("the wheel mesh was not drawn")
}

#[test]
fn the_rim_is_feathered_by_a_transparent_ring_outside_the_colour() {
    // ⚠ A mesh gets no anti-aliasing from egui, so without this ring the rim is a hard
    // 180-gon. Sabotage: drop the ring and the outermost vertices are opaque.
    let verts = wheel_vertices();
    let hub = verts[0].pos;
    let far = verts.iter().map(|v| hub.distance(v.pos)).fold(0.0, f32::max);
    let outer: Vec<_> = verts.iter().filter(|v| hub.distance(v.pos) > far - 0.1).collect();
    assert!(outer.len() >= SEGMENTS, "no ring at the outer radius");
    assert!(outer.iter().all(|v| v.color.a() == 0), "the outer ring is not transparent");
    let rim: Vec<_> = verts.iter().filter(|v| v.color.a() == 255 && hub.distance(v.pos) > 1.0).collect();
    assert!(rim.len() >= SEGMENTS, "the coloured rim is missing");
    let rim_r = rim.iter().map(|v| hub.distance(v.pos)).fold(0.0, f32::max);
    assert!(far > rim_r && far - rim_r < 2.0, "the ring is not one pixel outside the rim: {far} vs {rim_r}");
}
```

- [ ] **Step 2:** run `cargo test -p app --bin Rustroom the_rim_is_feathered` → FAIL ("the outer
  ring is not transparent").
- [ ] **Step 3:** implement: `SEGMENTS = 180`, doc comment "~3.3 px", and in `show` after the fan:

```rust
let feather = 1.0 / ui.pixels_per_point();
let first_outer = mesh.vertices.len() as u32;
for i in 0..=SEGMENTS {
    let deg = i as f32 / SEGMENTS as f32 * 360.0;
    mesh.colored_vertex(centre + offset(deg, 1.0, radius + feather), egui::Color32::TRANSPARENT);
}
for i in 0..SEGMENTS as u32 {
    let (a, b) = (1 + i, 2 + i);
    let (c, d) = (first_outer + i, first_outer + i + 1);
    mesh.add_triangle(a, c, b);
    mesh.add_triangle(b, c, d);
}
```
- [ ] **Step 4:** test passes; `cargo test -p app --bin Rustroom color_wheel` all green.
- [ ] **Step 5:** commit `The colour wheel's rim is feathered, and drawn at 180 segments`.

### Task 2: `section.rs` — the header with the chevron on the left

**Files:** create `crates/app/src/develop/panel/section.rs`; modify `adjustments.rs`, `panel/mod.rs`
(add `pub mod section;`).

- [ ] **Step 1:** write `section.rs`:

```rust
//! The section header: `› NAME •  …  reset`.
use crate::theme;
use eframe::egui;

const HEIGHT: f32 = 22.0;
const CHEVRON_W: f32 = 14.0;

pub struct Header<'a> {
    pub name: &'a str,
    /// Draw the accent dot after the name.
    pub modified: bool,
    /// Offer a `reset` at the right edge. ⚠ The Profile block passes `false`.
    pub resettable: bool,
}

/// Returns the collapsing state (call `show_body_unindented` on it) and whether `reset` was clicked.
pub fn header(ui: &mut egui::Ui, id: egui::Id, default_open: bool, h: Header<'_>) -> (egui::collapsing_header::CollapsingState, bool) { … }
```
Body: as today's `adjustments::section` minus the `modified` computation and the reset apply, with
the chevron centre at `rect.left() + CHEVRON_W / 2.0`, the name at `rect.left() + CHEVRON_W + 2.0`,
the reset cell against `rect.right()`. Test in the module: `the_profile_header_offers_no_reset` —
`labelled_centres` over a `header` with `resettable: false, modified: true` contains the name and
not "reset"; and with `resettable: true` contains "reset".

- [ ] **Step 2:** `adjustments::section` becomes:

```rust
fn section(ui, s, open_by_default, edits, ed, body) {
    let modified = edits.group_differs(&edit_model::Edits::default(), s.index());
    let (state, reset) = super::section::header(ui, ui.make_persistent_id(s.name()), open_by_default,
        super::section::Header { name: s.name(), modified, resettable: true });
    state.show_body_unindented(ui, |ui| body(ui, edits, ed));
    if reset { edits.merge_from(&Edits::default(), EditGroups::only(s.index())); ed.discrete(); }
}
```
- [ ] **Step 3:** `cargo test -p app --bin Rustroom adjustments` green (the two reset tests).
- [ ] **Step 4:** commit `The section header: chevron on the left, reset alone on the right, in its own module`.

### Task 3: `profile.rs` and the Profile block

**Files:** create `crates/app/src/develop/panel/profile.rs`; modify `adjustments.rs` (`tone` loses
the checkbox, the look, and its `looks` parameter; tests move), `panel/mod.rs` (block + hoisted
`Edited`), `edit-model/src/lib.rs` (`RENDERING_GROUP` doc).

- [ ] **Step 1:** `profile.rs`: `pub fn rendering(ui, edits, looks, ed)` = the checkbox block and
  `look::section` call moved verbatim from `tone`; `mod tests` = `the_rendering_checkbox_switches_both_ways…`,
  `unticking_the_rendering_decides_off…`, `two_looks`, `look_labels`,
  `the_strength_slider_appears_only_when_a_look_is_chosen`, all calling `rendering` (no buffer).
- [ ] **Step 2:** `tone(ui, edits, buffer, baseline_ev, hist, ed)`; fix its three test callers and
  the doc comment.
- [ ] **Step 3:** `panel/mod.rs`: hoist `let mut ed = Edited::default();` above the Profile block;
  under `if tool == Tool::Edit` replace `camera_profile_row(app, ui)` with:

```rust
let (state, _) = section::header(ui, ui.make_persistent_id("profile-block"), true,
    section::Header { name: "Profile", modified: false, resettable: false });
state.show_body_unindented(ui, |ui| {
    camera_profile_row(app, ui);
    if let Some(s) = &mut app.session {
        profile::rendering(ui, &mut s.edits, &app.looks, &mut ed);
    }
});
```
- [ ] **Step 4:** run `cargo test -p app --bin Rustroom` → green; fix the `edit-model` doc.
- [ ] **Step 5:** commit `A Profile block above White Balance: camera profile, camera rendering, look`.

### Task 4: White Balance row, Tone order, Color row, the global reset

**Files:** `adjustments.rs`, `theme.rs`, `panel/mod.rs`.

- [ ] **Step 1:** `theme::sub_label_with(ui, text, trailing: impl FnOnce(&mut egui::Ui))`: the
  `add_space`, then a `horizontal` with the label and `with_layout(right_to_left(Center), trailing)`.
  `sub_label` calls it with `|_| {}`.
- [ ] **Step 2:** WB: wrap the eyedropper toggle and the Presets menu in `ui.horizontal`.
- [ ] **Step 3:** Tone: move `tone_curve::show` to directly after Blacks; Presence after it.
- [ ] **Step 4:** Color: `theme::sub_label_with(ui, "Color Grading", |ui| { checkbox "Show range" })`
  replacing the bottom checkbox; delete one of the two consecutive `ui.separator()`.
- [ ] **Step 5:** `panel/mod.rs`: the heading row becomes a `horizontal` with the heading and, for
  Edit with a session, a right-aligned small `Reset adjustments` button (same tooltip, same
  `merge_from`, `ed.discrete()`); delete the bottom button and its separator. Test in `mod tests`:
  `reset_adjustments_sits_in_the_heading_row_and_resets_the_sliders` — open a test photo, set
  exposure 2.0, `click(&ctx, "Reset adjustments", |ui| show(&mut app, ui))`, exposure is 0.
- [ ] **Step 6:** `cargo test -p app` green.
- [ ] **Step 7:** commit `White Balance shortcuts on one row, the curve before Presence, Show range beside its selector, the reset in the heading`.

### Task 5: Gate, GUI, docs

- [ ] **Step 1:** the gate recipe; all three zero.
- [ ] **Step 2:** launch the GUI in the background (`cargo run --release -p app`).
- [ ] **Step 3:** journal section under the restyle's, CLAUDE.md state (commit count, test count);
  commit `docs: record the panel order and chrome corrections`.
- [ ] **Step 4:** report: what landed, sizes over the marker, the branch unmerged and unpushed.
