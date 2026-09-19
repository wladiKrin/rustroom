# Develop panel restyle: surfaces, sliders, section chrome, icon rail

**Status:** spec, 2026-09-18. Approved as a direction on a design canvas (Rustroom Develop
Panel, five artboards beside the panel as shipped); this document is the written form.

## Why now

The visual identity pass (2026-08-13) settled the palette, the accent and the density, and
deliberately left the *structural* half alone: "whether buttons carry icons, whether sections
have headers, how a control row is composed." The Develop panel has since grown from six
sections of sliders to a panel that also carries a histogram, a tone curve, an HSL selector,
four grading wheels, a snapshots strip and four tool panels, all drawn with egui's stock widgets.
Three things the eye trips on, in the order they are met:

1. **The slider rail is egui's fixed 100 pt**, so a third of every row is the value box and the
   label, and the rail itself is short. The panel is 240 pt wide and the row spends 140 of it on
   chrome.
2. **The accent's trailing fill starts at the left edge**, so a bipolar slider at 0 (Contrast,
   Tint, every HSL band) shows half a bar of accent for "nothing applied." The accent was chosen
   to carry "where is this set" (`theme::tune`'s own comment) and on a bipolar control it says the
   opposite.
3. **Everything is one surface.** The panel, the canvas surround and the popups are all
   `gray(27)`, content sits flush against the panel edge with no inner margin, and the `Shad ·
   Mid · High · Global` chips are abbreviated because four `selectable_value`s do not fit.

Four options were drafted and compared beside the shipped panel; all four were approved, in
the order below, with two changes to D recorded in §5.

## Decisions taken

| Question | Decision |
|---|---|
| Order | **A** surfaces → **B** sliders → **C** section chrome → **D2** rail, each its own commit series on one branch |
| Rail | **Icon-only, 44 pt**, the tool's name as a hover tooltip. **Tint only** for the selected entry — the accent bar was rejected |
| Density | Rows stay at `interact_size.y` = 16. The compact decision stands; the mock's 18 pt row is not adopted |
| Light mode | Every colour is decided twice, as the visual identity spec requires. Dark values are from the canvas; light values are derived here and contrast-checked |
| Modals | Keep egui's stock `Slider`. They are dialogs with room, not the dense panel; the new widget is for the panel |
| The tone curve and colour wheel | Not redrawn. The curve gets the same framed well as the histogram and a ghost of the histogram behind it; the wheel is untouched |

## A · Surfaces and spacing

**Files:** `theme.rs` (the values, `tune`, `apply`), `develop/panel/mod.rs` (the panel's frame),
`develop/panel/snapshots.rs` (the strip's frame), `develop/canvas/mod.rs` (the canvas frame).
Everything else in the app inherits.

### A.1 Two surfaces, not one

The panel is a surface the image is judged *from*; the canvas is the surround the image is judged
*against*. Today both are `panel_fill = gray(27)`. The canvas becomes darker than the panel, in both themes:

| | dark | light |
|---|---|---|
| `visuals.panel_fill` (right panel, rail, filmstrip, menus) | `gray(32)` | `gray(248)` (unchanged) |
| `visuals.window_fill` (popups, dialogs) | `gray(32)` | unchanged |
| canvas surround (`theme::canvas_fill`) | `gray(22)` | `gray(222)` |

The canvas fill is applied by `develop::canvas::show` through
`CentralPanel::default().frame(theme::canvas_frame(ui.style()))`. ⚠ **Only the Develop canvas.**
The Library grid is a browsing surface where thumbnails carry their own captions and the darker
surround argument does not apply; it keeps `panel_fill`.

⚠ The visual identity spec's surround argument — low chroma **near the image** — is what this
serves: the darkest, most neutral surface is the one touching the photograph.

### A.2 Text and separators

| | dark now → after | light now → after |
|---|---|---|
| `widgets.noninteractive.fg_stroke.color` (body text, slider labels) | `gray(140)` → `gray(166)` | `gray(80)` (unchanged) |
| `widgets.noninteractive.bg_stroke.color` (separators, indents) | `gray(60)` → `gray(46)` | `gray(190)` → `gray(205)` |

Dark body text at `gray(140)` on `gray(27)` is 4.6:1, readable but dim beside the `gray(180)`
button text next to it; `gray(166)` on `gray(32)` is ~6.9:1. Light mode's `gray(80)` on
`gray(248)` is already 7.5:1 and stays. ⚠ Separators get *weaker*, not stronger: the section
headers in C carry the structure, so the hairlines become guides rather than walls.

### A.3 Radius and margin

- Every `widgets.*.corner_radius` becomes `CornerRadius::same(4)` in `tune` (theme-independent).
  Menus and windows keep egui's 6.
- The right panel and the snapshots strip get `Frame::side_top_panel(style).inner_margin(Margin
  { left: 10, right: 10, top: 6, bottom: 6 })`. Today the frame is egui's default, whose inner
  margin is what leaves "Camera profile" flush against the edge.

⚠ `DIALOG_MARGIN` is untouched; dialogs already solved this on 2026-08-27.

### A.4 What A does not do

No slider or widget anatomy changes. `slider_trailing_fill` stays on until B replaces the widget,
so the bipolar-fill fault survives A — B is what fixes it, and A is sequenced first only because
it is cheap and every later option is drawn on these surfaces.

### A.5 Tests

- The two existing contrast guards keep passing, **but `base_fill` in `theme`'s test module must
  read the tuned style's `panel_fill`, not `Visuals::dark().panel_fill`** — it documents itself as
  "the one place the base is defined", and after A egui's default is no longer the base. Assert
  through `apply` on a fresh `Context`.
- New: `body_text_is_readable_on_the_panel` — noninteractive `fg_stroke` against `panel_fill`,
  ≥ 4.5:1 (WCAG AA for text), both themes.
- New: `the_canvas_is_darker_than_the_panel_in_both_themes` — pins the
  direction per theme, so a swapped pair cannot ship.

## B · The slider row

**Files:** new bin module `develop/panel/slider.rs` (the widget); new lib module
`crates/app/src/slider_map.rs` (the pure mapping); `develop/panel/mod.rs` (`Edited::slider`
delegates); `develop/panel/adjustments.rs` (WB Temp); `develop/panel/retouch.rs` (Size, Feather,
Threshold). Four sites bypass `Edited::slider` today and all four move; nothing else in
`develop/` constructs an `egui::Slider` (verified by grep).

### B.1 Anatomy

One row, `interact_size.y` tall (16 pt), three columns:

```
Exposure   ────────────●─────────────   0.69
Contrast   ───────────┼●─────────────      0     (bipolar: fill from the centre, zero tick)
Temp       ██████████●──────────────── 4101 K    (unipolar: fill from the left)
```

- **Label**, `LABEL_WIDTH` = 78 pt, body text, left. A label wider than the column is truncated
  (`TextWrapMode::Truncate`) and the full text is the row's hover text.
- **Rail**, the remaining width, 3 pt tall, `widgets.inactive.bg_fill`. The fill is
  `selection.bg_fill` (the accent) and runs **from zero**: a range that straddles zero fills from
  the centre toward the handle and paints a 1 pt zero tick in the separator colour; any other
  range fills from the left. The handle is a 5 pt-radius circle in `strong_text_color`, ringed in
  the accent while hovered or dragged.
- **Value**, `VALUE_WIDTH` = 44 pt, right-aligned, tabular figures. It is an `egui::DragValue`
  drawn in a scope whose inactive and hovered `weak_bg_fill` are transparent, so it reads as plain
  text and keeps typing, drag-on-the-number, `custom_formatter` and `custom_parser` for free.
  When the range straddles zero and the value is exactly zero the number is drawn in
  `weak_text_color`, so untouched rows recede.

### B.2 Behaviour, and why it must match egui's

`Edited::slider` records `changed()` and `drag_stopped()` separately — one history entry per
settled drag — and four headless harnesses (`masks::drag_in_panel`, `mask_adjust::drag_slider`,
`geometry`, `adjustments`) locate a slider **by its AccessKit label**, press at its centre and
drag 40–120 pt to the right. So the widget must:

- allocate the **rail** with `Sense::click_and_drag()` and set `WidgetInfo::slider(enabled,
  value, label)` on *that* response, with `min`/`max` and the `SetValue`/`Increment`/`Decrement`
  actions exactly as `egui::Slider::add_contents` does — the label is what tests find, and the
  centre of the rail is where their press lands;
- set the value from the **pointer position** on every frame with an `interact_pointer_pos`,
  through `emath::smart_aim::best_in_range_f64` within `aim_radius`, so a drag produces the same
  "nice" numbers egui's does (0.69, not 0.6873);
- take focus on click and step by `ArrowLeft`/`ArrowRight` while focused, with egui's
  `EventFilter` so the arrows do not move focus;
- report `changed()` when the value moved and `dragged()`/`drag_stopped()` from the rail — the
  retouch panel reads `dragged()` to hold a patch suppressed mid-gesture.

⚠ **Not built:** double-click to reset, smart-aim's keyboard-precision modifier, vertical
orientation, `step`, `clamping` modes. Only what the four call sites use.

### B.3 The builder

```rust
pub struct Slider<'a> { /* value, range, label, log, formatter, parser */ }
impl<'a> Slider<'a> {
    pub fn new(value: &'a mut f32, range: RangeInclusive<f32>) -> Self;
    pub fn text(self, label: &'a str) -> Self;
    pub fn logarithmic(self, on: bool) -> Self;                  // retouch Size
    pub fn custom_formatter(self, f: impl Fn(f64) -> String) -> Self;   // Temp, Size
    pub fn custom_parser(self, f: impl Fn(&str) -> Option<f64>) -> Self; // Temp
}
impl egui::Widget for Slider<'_> { fn ui(self, ui: &mut egui::Ui) -> egui::Response; }
```

The default formatter is `emath::format_with_decimals_in_range(v, 0..=2)`, which is what the
stock `DragValue` shows today (Whites `100`, Blacks `-27.77`), so no displayed number changes.

### B.4 The pure half

`slider_map` (lib, no egui): `t_of(value, range, log) -> f32`, `value_of(t, range, log) -> f32`
and `fill_span(t, range) -> (f32, f32)`. The log mapping is egui's own (`remap` over `ln`,
which for a strictly positive range is what `Slider::logarithmic` does at the aim radius egui
uses). These are what the tests reach for the geometry; the widget only paints them.

### B.5 Tests

- `slider_map`: round trips at both ends and the middle, linear and log; `fill_span` is `(0, t)`
  for a unipolar range, `(min(t, ½), max(t, ½))` for a bipolar one — ⚠ probed at `t = 0.3` and
  `0.7`, never only at `0.5`, where a wrong branch is inert.
- The widget, headless through `test_ui`: it is found by its label; a drag from the centre changes
  the value and `drag_stopped` fires on release (`Edited` sees exactly one settle); a press with
  no motion leaves the value alone; a bipolar range at zero reports the value as zero after a
  press-release on the centre (the fill-from-centre geometry puts the handle there).
- Every existing panel harness passes **unchanged**. That is the acceptance test for B: the four
  drag helpers were written against egui's slider and must not need editing.
- `Edited`'s own tests are untouched.

## C · Section chrome, sub-labels, segmented controls, histogram

**Files:** `theme.rs` (three helpers: `section_header`, `sub_label`, `segmented`), `edit-model`
(`Edits::group_differs`), `develop/panel/adjustments.rs` (`section`, sub-labels, the grading
chips), `develop/panel/hsl_bands.rs`, `develop/panel/mask_adjust.rs`, `develop/panel/retouch.rs`
(the mode pair), `develop/panel/geometry.rs` (five sub-labels), `develop/panel/mod.rs`
(`draw_histogram`), `develop/panel/tone_curve.rs` (the well and the ghost). ⚠ `adjustments.rs`
is **1,165 lines** and `masks.rs` 834, both past the ~600 marker; C removes lines from the first
and adds none to the second, and the helpers go to `theme.rs` (503) rather than either.

### C.1 Section headers

`adjustments::section` stops using `CollapsingHeader` and draws its own header over
`CollapsingState`:

- a full-width row, 22 pt, `Sense::click()`, the section name **uppercased** in a new named text
  style `TextStyle::Name("section")` (proportional 11 pt, installed by `tune`), a 1 pt hairline
  under it in the separator colour, and 6 pt of air above;
- a chevron at the right, painted by hand from `openness` (two strokes, rotated 0 → 90°), so no
  glyph is needed and egui's default triangle is not drawn — `show_header` is not used, the body
  goes through `show_body_unindented`;
- a 5 pt accent dot after the name while `edits.group_differs(&Edits::default(), s.index())`,
  and a small `reset` text button at the right of the row in the same state, which does
  `edits.merge_from(&Edits::default(), EditGroups::only(s.index()))` and `ed.discrete()`. The
  hover text names the section, as "Reset adjustments" names all six.

⚠ **The body is unindented.** egui's `CollapsingHeader` indents the body by `indent`, which is
where the left-hand gap under every section comes from today; with a full-width header the
indent has nothing to align to.

⚠ The open state stays in egui's memory with the same id egui already uses (`s.name()`), so a
user's collapsed sections survive this change within a session as they do today; nothing is
persisted, as before.

### C.2 `Edits::group_differs`

```rust
/// Whether `self` and `other` differ in edit group `i`. ⚠ Defined *through* `merge_from`, so it
/// cannot disagree with the partition: merge `self`'s group into a clone of `other` and compare.
pub fn group_differs(&self, other: &Edits, i: usize) -> bool
```

Cost: one `Edits` clone per section per frame, six per frame. An `Edits` is a few kilobytes even
with brush strokes; ⚠ if a profile ever shows this, cache the six booleans on `ed.changed`
rather than inventing a second partition.

Tests, in `edit-model`: a non-neutral value in group `i` differs in `i` and in no other group,
swept over every group via `all_nonneutral`; the identity differs nowhere; ⚠ the sabotage
"always false" fails the first, "always true" fails the second.

### C.3 Sub-labels

`theme::sub_label(ui, "Presence")`: 4 pt of air, the text in the `section` style in
`weak_text_color`, no hairline. It replaces every plain `ui.label("…")` that names a block of
controls, and the bare `ui.separator()` above Texture, which gains the name **Presence** the
comment already gives it:

| file | today | after |
|---|---|---|
| `adjustments.rs` | separator → Texture | `sub_label("Presence")` |
| `adjustments.rs` | `label("Noise Reduction")`, `label("Sharpening")`, `label("Color Grading")` | `sub_label` |
| `mask_adjust.rs` | `label("Noise Reduction")`, `label("Sharpening")` | `sub_label` |
| `geometry.rs` | `label("Lens")`, `"Geometry"`, `"Composition"`, `"Perspective"`, `"Guides"` | `sub_label` |
| `tone_curve.rs` | `label("Tone Curve")` beside Reset | `sub_label` in the same row |

⚠ Not touched: `hint`, `caption`, the tool heading (`ui.heading(title)`), labels that are
*values* ("No presets yet.", the lens status line), and every `selectable_label`.

### C.4 One segmented control

`theme::segmented(ui, current: &mut T, items: &[(T, &str)]) -> Response` where `T: PartialEq +
Copy`: a full-width well 18 pt tall in `extreme_bg_color` with 2 pt padding, `items.len()` equal
columns, the selected column filled with `widgets.inactive.bg_fill` and its text in
`strong_text_color`, the rest in `text_color`. Each column is its own `ui.interact` with
`WidgetInfo::selected(WidgetType::SelectableLabel, enabled, selected, label)`, so
`test_ui::click(ctx, "Saturation", ..)` keeps working and the existing
`clicking_a_segment_switches_the_channel` test passes unchanged.

Sites: the HSL channel selector (`hsl_bands.rs`), both grading-range selectors (`adjustments.rs`,
`mask_adjust.rs`), and the retouch mode pair, which is already hand-rolled as a joined pair and
becomes the shared helper. **`GradeRange::label` returns the full words** — Shadows, Midtones,
Highlights, Global — since equal columns at 220 pt give each 55 pt and "Highlights" at 11 pt is
~52. ⚠ Two tests in `adjustments.rs` click chips by the abbreviated labels (`p.click(&ctx,
"Mid")` and `"High"`, in the grade-range view tests); both move to the full words in the same
commit as the rename. Verified by grep: no other test names them.

⚠ Not converted: the geometry aspect chips (they carry a Custom entry with an inline ratio
field), the composition-guide list, the Look and WB preset lists, the mask mode chips. Those are
lists of options with their own reasons for being `selectable_label`s; this control is for a
**view switch** over a fixed small set.

### C.5 The histogram

`draw_histogram` keeps its signature and paints:

- a well: 72 pt tall, `extreme_bg_color`, 4 pt radius;
- each channel as a filled area — one `egui::Mesh` of 255 quads per channel, in that channel's
  colour at alpha 140 — instead of 256 one-pixel lines, so overlaps mix rather than alternate;
- a clipping marker at each top corner, a 5 pt triangle in `strong_text_color`, when any
  channel's bin 0 (left) or bin 255 (right) holds more than **0.5 %** of the frame's samples.
  The threshold is a named constant with the reason: a marker that fires on the one specular
  pixel every frame has is a marker nobody reads.

The three channel colours stay hardcoded, as the visual identity spec's caveat 7 records for
everything painted by hand; they are lifted into named constants so the tone curve's ghost can
reuse the luma.

### C.6 The tone curve

`tone_curve::show` gains `hist: Option<&[[u32; 256]; 3]>`; `adjustments::tone` passes the
session's, `mask_adjust` passes `None`. Behind the grid it paints the per-bin **maximum** of the
three channels as one area in `weak_text_color` at alpha 40 — the shape of the tonal
distribution under the curve you are shaping, the convention of every curve editor. The well
takes the histogram's fill and 4 pt radius; the 1 pt outline goes. Grid, identity line, curve and
points are unchanged.

### C.7 Tests

- `theme`: `every_segment_is_reachable_by_its_label` (all columns found via `test_ui`);
  `clicking_a_segment_selects_it_and_only_it`; `the_section_style_is_installed_in_both_themes`.
- `adjustments`: `a_modified_section_shows_its_reset_and_a_neutral_one_does_not` (found by the
  `reset` label through `test_ui`, both halves asserted — a reset that always shows and one that
  never shows are equally wrong); `resetting_a_section_leaves_the_other_sections_alone`.
- `panel/mod.rs`: `the_clipping_marker_needs_more_than_a_single_pixel` on `clipped_edges`, the
  pure predicate the painter reads.
- The existing `clicking_a_segment_switches_the_channel`, the mask panel's chip tests and every
  drag harness pass unchanged after the label rename.

## D2 · The icon rail

**File:** `develop/rail.rs`.

- `RAIL_WIDTH` 88 → **44**; `LABEL_SIZE`, `GAP` and the label paint go; `GLYPH_SIZE` stays 22
  and `PAD` 6, so an entry is 34 pt tall over 44 wide.
- The selected entry is painted with **`selection.bg_fill.gamma_multiply(0.18)`** on a 4 pt-radius
  rect and its glyph in `selection.bg_fill`; hovered, unselected entries get
  `widgets.hovered.weak_bg_fill`; the rest are transparent with the glyph in `text_color`. ⚠ No
  bar at the edge — decided against on the canvas.
- Every entry gets `on_hover_text(t.label())` **and** `widget_info(WidgetInfo::labeled(
  WidgetType::Button, enabled, t.label()))`, so the name a sighted user gets on hover is the name
  a screen reader and `test_ui::find` get always.

⚠ The rail's `every_rail_glyph_is_renderable` and `clicking_a_rail_entry_selects_that_tool` are
untouched — the second calls `entry` directly and reads its rect, so it follows the width.

Tests: `every_rail_entry_is_labelled_for_accessibility` — each `Tool::label()` is found through
`test_ui::labelled_centres` after D2, which is the property the tooltip and the accessible name
both rest on; sabotage by dropping `widget_info` and watch it fail.

⚠ The rail is now narrower than the Presets panel it replaced was wide, and the labels the rail
spec (2026-08-24) chose deliberately — "sized so Presets fits on one line" — are gone. Recorded as
the trade taken, with the tooltip as the mitigation.

## What every option leaves alone

- `Edits`, the sidecar, presets, sync, the pipeline, every golden. A byte-identical golden set at
  the end is the proof nothing here reached a pixel.
- The Library, the filmstrip, the modals, the menu bar, the canvas overlays.
- The colour wheel, the mask component list, the snapshots list rows.
- The accent pairs and `Accent::ALL`.

## Sequencing and verification

One branch, `develop-panel-restyle`, four commit series in the order A → B → C → D2. Each series
ends with `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D warnings` and
`cargo test --all`, each exit code read from its own command, and the GUI launched for a look
before the next series starts — every panel-wiring defect this project has found was found by
use. The 74 goldens must be byte-identical at every step.

## Accepted caveats

1. **Two palettes to maintain** — every value in A is a pair, and the light values here were
   derived rather than seen; the light-mode look is checked by switching in the running app.
2. **Modals keep the stock slider**, so the app has two slider idioms. Deliberate: the panel's
   row is built for a 240 pt column of forty sliders, and a dialog's is not.
3. **A label wider than 78 pt truncates.** The widest today ("Radius (px)", "Smoothness") fit;
   the hover text is the mitigation for the next one.
4. **`group_differs` clones an `Edits` per section per frame.** Bounded and small; the cache is
   the fix if a profile ever disagrees.
5. **The histogram's channel colours and the ghost's alpha are hand-painted constants** outside
   the accent system, as everything hand-painted already is.
6. **The rail loses its labels.** The tooltip and the accessible name replace them.
7. **No slider double-click-to-reset**, no keyboard precision modifier. The per-section reset in
   C is the reset this pass ships.
