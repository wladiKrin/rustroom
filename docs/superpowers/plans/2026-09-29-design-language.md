# Design language (direction B) — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:executing-plans (the author runs every
> plan inline). Steps use checkbox (`- [x]`) syntax for tracking.

**Status (2026-09-29): ✅ built on `design-language`, gate green at 3230 tests, awaiting use.**
Deviations, each recorded in `docs/DESIGN.md`: the compact segmented control is 24 pt with caption
labels; the format chips sit with the view settings on the status row's right; Develop's status row
has no zoom percentage; hovers outside the sliders are still set at their call sites (tabling them
per panel is the open half of §8); Title and Display render Regular until Inter Medium is bundled.
`RUSTROOM_DEBUG_WINDOW` was added to check the 13-inch width.

**Goal:** make the running app follow `docs/DESIGN.md`: four text sizes and nothing below 11 pt,
the B surfaces and seven accents, one form per control, a top row plus a status row that never
overlap at 1440 pt, and hover text in place of on-screen hints.

**Architecture:** colour and size tokens live in one bin module (`theme/tokens.rs`) that every
hand-painter reads; what fits in a bar is decided by a pure lib module (`bar_plan`) and drawn by a
new bin module (`status_bar`) plus a slimmer `shell::top_bar`. Control restyles stay in their
existing modules (`segmented`, `list_row`, `develop/panel/section`, `develop/rail`,
`modals/shell`, `thumb`).

**Tech stack:** egui 0.35 / eframe, the existing `test_ui` headless harness.

**Branch:** `design-language` (already holds `docs/DESIGN.md`). Commit after each task; no
`Co-Authored-By` trailer; never push.

**Gate (one invocation mode for the whole session):**
```
cargo fmt --all -- --check
CARGO_TARGET_DIR=target/clippy cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

---

### Task 1: Tokens and accents

**Files:** create `crates/app/src/theme/tokens.rs`; modify `crates/app/src/theme.rs`,
`crates/app/src/settings.rs` (`Accent`).

- [x] `settings::Accent` gains `Orange`, `Sage`, `Rose`; `ALL` becomes seven in the order Steel
  blue, Teal, Violet, Orange, Sage, Rose, Neutral; labels "Steel blue", "Teal", "Violet",
  "Orange", "Sage", "Rose", "Neutral". Serde stays by variant name, so old files load.
- [x] `theme/tokens.rs`:
  ```rust
  pub struct Tokens { pub canvas, pub panel, pub card, pub raised, pub hover, pub line,
                      pub text, pub text2, pub text3, pub tooltip: egui::Color32 }
  pub const DARK: Tokens = /* #121214 #1b1b1e #232327 #303036 #3b3b42 #2b2b30 #e6e6e8 #a3a3a8 #727278 #0f0f11 */;
  pub const LIGHT: Tokens = /* #dcdcdf #f4f4f5 #ffffff #e6e6ea #dcdce1 #d9d9de #1d1d1f #5f5f66 #9a9aa0 #2a2a2e */;
  pub fn tokens(dark: bool) -> &'static Tokens;
  pub fn of(ui: &egui::Ui) -> &'static Tokens;       // by ui.visuals().dark_mode
  pub const TINT: f32 = 0.18;                         // open row, active tool
  pub const ON_ACCENT_DARK: Color32 = #0e1216;        // text on the dark-theme accent
  pub fn on_accent(dark: bool) -> Color32;            // dark text in dark, white in light
  ```
  plus the size tokens (Task 2) so there is one place.
- [x] `theme::accent_color` gets the three new pairs (Orange `#e39a5c`/`#b0521a`, Sage
  `#9cc48a`/`#3e7a37`, Rose `#dd8fa6`/`#a63d5f`).
- [x] `theme::surfaces` sets, per theme: `panel_fill`, `window_fill` = panel; `extreme_bg_color`
  = card (fields); `faint_bg_color` = card; `widgets.noninteractive.{bg_fill,weak_bg_fill}` =
  panel, `.fg_stroke` = text, `.bg_stroke` = line; `widgets.inactive.{bg_fill,weak_bg_fill}` =
  raised, `.fg_stroke` = text; `widgets.hovered.weak_bg_fill` = hover; `weak_text_color` =
  Some(text2); `window_stroke` = line. `canvas_fill` returns `tokens.canvas`.
- [x] `primary_button` and the accent pill paint text in `on_accent(dark)`.
- [x] Tests (in `theme.rs`): `every_accent_has_usable_contrast` stays (≥ 3:1 over seven);
  new `text_on_every_accent_is_readable` (≥ 4.5:1 for `on_accent` on every accent, both themes);
  new `the_text_greys_are_readable_where_they_are_used` (text ≥ 4.5 and text2 ≥ 4.5 on panel
  *and* card, both themes); new `the_surfaces_step_up_in_order` (dark: canvas < panel < card <
  raised < hover by luminance; light: canvas darkest, card lightest).
- [x] Gate, commit `feat(app): design tokens — B surfaces and text greys, seven accents, dark
  text on the dark-theme accent`.

### Task 2: Type scale and metrics

**Files:** `theme.rs` (`tune`), and every literal-size site: `thumb.rs`, `shell.rs`,
`library.rs`, `segmented.rs`, `list_row.rs`, `develop/panel/snapshots.rs`,
`develop/panel/mask_components.rs`, `develop/panel/geometry.rs`, `develop/mod.rs`
(`ICON_SIZE`), `develop/rail.rs`, `multi/mod.rs`, `multi/survey.rs`, `modals/shell.rs`.

- [x] `tune` sets `text_styles`: `Small` 11, `Body` 13, `Button` 13, `Monospace` 13 (Hack),
  `Heading` 20, `section` 11, and a named `title` 15. `theme::title_font(style)` resolves it
  with a fallback (the `section_font` pattern). Size tokens in `tokens.rs`: `CAPTION 11.0`,
  `BODY 13.0`, `TITLE 15.0`, `DISPLAY 20.0`, `ICON_INLINE 14.0`, `ICON 16.0`, `ICON_LARGE 24.0`.
- [x] Metrics in `tune`: `interact_size.y` 28, `item_spacing` (8, 2), `button_padding` (10, 4),
  `slider_rail_height` 4, one `corner_radius` 6 for every widget state, `menu_corner_radius` and
  `window_corner_radius` 12, `tooltip_width` 280, `interaction.tooltip_delay` 0.4.
- [x] Replace every literal: badge 9 → `CAPTION`; captions/counts/notes 11 → `Small`; 12, 12.5,
  13, 13.5 → `Body`; 14 (switch label) → `TITLE`; 15 (empty-state button) → `title_font`;
  glyphs 15, 17, 18 → `ICON`; rail glyph 26 → `ICON_LARGE`; `develop::ICON_SIZE` 17 → `ICON`.
- [x] Tests: `hint_text_is_smaller_than_body_text` becomes `the_type_scale_is_four_sizes`
  (collect every installed style's size; assert the set is exactly {11, 13, 15, 20}; assert
  nothing below 11). `the_compact_metrics_are_tighter_than_the_defaults` becomes
  `the_metrics_follow_the_design_language` (control row 28, radius 6). The radius test reads the
  token. A source sweep `no_literal_font_size_outside_theme`: walk `crates/app/src/**/*.rs`
  except `theme*`, strip `#[cfg(test)]` tails, and fail on `FontId::proportional(<digit>`,
  `FontId::new(<digit>` or `.size(<digit>` — sample first (the sweep must find the theme files'
  own uses when pointed at them, the positive control).
- [x] Gate, commit `feat(app): four text sizes — the type scale in tune, every literal size
  replaced, a sweep that keeps it so`.

### Task 3: One form per control

**Files:** `segmented.rs`, `list_row.rs`, `develop/panel/section.rs`, `theme.rs`
(`sub_label*`), `develop/panel/adjustments.rs` + `develop/panel/mod.rs` + masks callers
(section cards), `develop/rail.rs`, `modals/shell.rs`, `thumb.rs`, `library.rs`,
`collections_panel.rs`, `develop/panel/mod.rs` (panel width).

- [x] **Segmented** (`segmented_sized`): the well is `tokens.canvas`, full radius; cells full
  radius; the lit cell `raised` with `text`, unlit `text2`, hover `hover`; compact height
  `SEGMENTED_HEIGHT` 24 (panel view switches, Caption face), primary 28 (Body face).
  **Pill**: same well and cells; height 28; label Body; icon `ICON`; accent-lit text
  `on_accent`. **Chip**: 20 tall, Caption, `raised`. **Switch**: label `TITLE`.
- [x] **List row**: `ROW_HEIGHT` 30, radius 6, name Body, count Caption `text3` (open row
  `text2`), open tint `TINT`, actions at `ICON`.
- [x] **Section**: header `HEIGHT` 26. `Header::rail(bool)`: rail headers keep the uppercase
  Caption name in `text2`; panel headers show the name as written in Body, `text`, sentence
  case, no hairline. New `section::card(ui, |ui| ..)` = `Frame` with `card` fill, radius 10,
  margin (10, 10, 2, 8); `adjustments::section`, the Profile block and the Masks panel's
  sections draw inside it, 8 apart. `theme::sub_label*` stop uppercasing (Caption, `text2`).
  Collections panel headers pass `.rail(true)`.
- [x] **Rail**: 32-pt buttons (`PAD` 4), glyph `ICON_LARGE`, tint `TINT`, radius 6.
- [x] **Dialog chrome** (`modals/shell.rs`): `TITLE_BAR` 44 with the title in `title_font`,
  × at `ICON`; `FOOTER` 52; footer note at `Small`. `DialogSize` default widths 340 / 520 / 680.
- [x] **Thumbnails**: `thumb::CORNER` 8; placeholder `raised`; badge Caption, 16 tall, white on
  55 % black; selection border 2 px accent, radius 8, drawn 2 px outside the image rect; the
  grid's gap 12 (`item_spacing` override inside the grid only); the rubber band in the accent.
- [x] **Panels**: Develop panel `default_size` 316 (floor 300 stays); collections panel
  `default_size` 240, `min_size` 200.
- [x] Tests: existing segmented/pill/section/list-row tests must stay green (they reach
  controls by label); update pinned numbers only where a test asserts a size this task changes,
  and say so in the commit. Add `a_rail_header_is_uppercase_and_a_panel_header_is_not` in
  `section.rs` (paint both, read the painted galley text from `ui.painter()` shapes — or assert
  on the `Header` → text mapping function `header_text(name, rail) -> String`, pure).
- [x] Gate, launch, look at Develop and Library, commit `feat(app): one form per control —
  pill segments, section cards, rail headers, dialog chrome, thumbnail cells`.

### Task 4: `bar_plan` — what fits in a bar (lib, pure)

**Files:** create `crates/app/src/bar_plan.rs`; modify `crates/app/src/lib.rs`.

```rust
//! What a bar shows at a given width. Pure: the bin measures, this decides, the bin draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Form { Full, Compact, Hidden }

#[derive(Clone, Copy, Debug)]
pub struct Item {
    /// Higher stays longer.
    pub priority: u8,
    pub full: f32,
    /// Width with its label dropped (an icon), when it has such a form.
    pub compact: Option<f32>,
    /// Whether it may fold away (into ⋯, or simply not be drawn).
    pub hideable: bool,
}

/// Step items down until the row fits: first every compactable item from the lowest priority
/// up, then every hideable item from the lowest priority up. `gap` sits between shown items.
pub fn plan(avail: f32, gap: f32, items: &[Item]) -> Vec<Form>;
pub fn width(items: &[Item], forms: &[Form], gap: f32) -> f32;
```

- [x] Tests first: `everything_shows_when_there_is_room`; `labels_drop_before_anything_hides`;
  `the_lowest_priority_hides_first`; `a_fixed_item_never_changes` (no compact, not hideable);
  `the_result_fits_whenever_the_fixed_items_do` (sweep avail 200..2000 step 10 over the real
  Library status-row items, assert `width <= avail` or every item is at its minimum);
  `gaps_are_counted_only_between_shown_items`.
- [x] Gate, commit `feat(app): bar_plan — which bar items show, drop their labels or fold, by
  priority; pure and tested`.

### Task 5: The status row and a slim top row

**Files:** create `crates/app/src/status_bar.rs` (bin); modify `main.rs` (mod), `shell.rs`
(`top_bar`, move `sort_combo`, `grid_size_slider`, `stack_buttons`, `format_chips`,
`reindex_button`, `reindex_hover`, `immich_cache_buttons`, the readout into `status_bar`),
`develop/mod.rs` (`body` draws the status row between the filmstrip and the canvas),
`shell.rs` `ui()` (Library draws the status row after the collections panel, before the view),
`multi/compare.rs`, `multi/survey.rs` (instruction line removed — Task 6).

- [x] **Top row, Library:** app switch · mode switch · search (240 → 120) · Filter, planned by
  `bar_plan` (app switch fixed; mode switch compact = icons only; search compact = 120; Filter
  compact = funnel only). Nothing on the right.
- [x] **Top row, Develop:** app switch · undo/redo · zoom group · Before/After (compact = icon)
  · centred name and camera line · Export. The readout leaves the top row.
- [x] **Status row, Library** (32 tall, hairline above, panel fill): left — the running job
  (Cancel + progress) or `app.status`; the format chips; centre — the selected photo's name
  and meta (`centre_info`, moved, still click-to-rename); right — Immich Download / Remove
  downloads (Immich source with a selection), sort combo, size slider, ⋯. ⋯ holds Expand all /
  Stack all (when the folder has stacks), Reindex, and whatever the plan folded (the size slider,
  the format chips as checkboxes). Plan priorities: status 90 (never hidden, truncates), sort 80,
  size 60, chips 40, Immich buttons 50.
- [x] **Status row, Develop:** left — the readout (swatch + numbers, fixed width); right — the
  running job or `app.status` ("Full resolution ready").
- [x] Tests in `status_bar.rs`: `the_status_row_holds_the_count_and_the_view_settings` (labels
  include the status text, "Reindex" reachable through ⋯ is not asserted, the sort combo and
  the size slider are there at 1440); `nothing_in_either_row_overlaps_at_the_floor_width`
  (draw both rows at 800 and 1440 wide; collect every labelled widget rect; assert no two
  intersect — sample first: > 4 rects); shell's `the_centred_name_is_the_same_in_both_views`
  reads the Library name from the status row. `the_bar_is_the_same_height_in_both_views` keeps
  its assertion with `BAR_HEIGHT` 44.
- [x] Gate, launch at a 1400 × 859 window and at Interface size 125 %, commit
  `feat(app): two rows — a slim top row that navigates and narrows, a status row that informs
  and sets the view, both planned by width`.

### Task 6: Hints become hover text

**Files:** `develop/panel/mod.rs` (profile note), `develop/panel/tone_curve.rs` (idle hint),
`develop/panel/parametric_curve.rs`, `develop/panel/defringe.rs`, `modals/settings.rs`
(view quality, resampler, Immich intro and footer paragraph, accent swatches),
`modals/import.rs` (tokens, "An empty field changes nothing", no-card explanation),
`modals/export.rs` (tokens, DNG note), `modals/info.rs` (shared-metadata note),
`multi/compare.rs` + `multi/survey.rs` (instruction lines), `shell.rs` (mode-switch hovers).

- [x] Each explanatory hint moves onto the hover of the control it explains; the line is
  deleted. Kept on screen, at Caption: values and live readouts (the curve point readout while a
  point is in play, the defringe range while dragging, "First file: …", ticked bytes, the chosen
  paths), empty-list notes ("No keywords yet."), transient states (the AI mask's waiting line,
  "Click the photo to choose what to select."), warnings (banding), and dialog body sentences.
- [x] Compare's and Survey's key lines become the hover of their mode-switch segment; the
  constants stay (the glyph sweep reads them).
- [x] The Settings accent control becomes a row of seven swatches (22-pt circles, the chosen one
  ringed in its own colour, the name on hover); Orange's hover says a warm accent can colour a
  white-balance judgement.
- [x] Tests: `the_accent_swatches_select_by_name` (click "Orange" through `test_ui`, the draft
  changes); keep every existing hover-coverage test green.
- [x] Gate, commit `feat(app): hover text instead of on-screen hints; accent swatches`.

### Task 7: Look, record, hand over

- [x] Launch the release build from a scratch profile (`DRIVE_PID` scoped), shoot Library,
  Develop (Edit, Masks, Crop), Settings, Export and Import at 1400 × 859, 125 % and the light
  theme; fix what the shots show; quit and prove it with `pgrep`.
- [x] Update `docs/DESIGN.md` where the build settled a value the document left open
  (compact segmented height 24, the status row's contents), `docs/JOURNAL.md` (a dated section),
  `CLAUDE.md` state (one line + grep pointer), plan statuses.
- [x] Re-gate the tip; commit; launch the app for the author and say what to look at.
