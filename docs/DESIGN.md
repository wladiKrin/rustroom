# Rustroom design language

**Status:** the rulebook, chosen by the author on 2026-09-29 from three directions
(canvas: <https://claude.ai/artifact/JK6A3tSpaXPo427cHZTZ7h>, direction **B · Layered**), and
**implemented and merged the same day** (branch `design-language`) — what follows describes the app as built;
where the build settled something the first draft left open, the text says so.
**Read this before adding or changing anything the user can see.** It is the answer to "what
should this look like"; `docs/JOURNAL.md` holds why each earlier decision was taken.

> ⚠ **Keep this file the live state.** When a rule changes, overwrite it here and record the
> old rule and its date in the decisions log at the end. Never append a superseded block.

## 1. What it is for

Rustroom's look must read as **one program**: the same four text sizes, the same greys, one
accent doing two jobs, one shape for each kind of control, and bars that hold together on a
13-inch MacBook Pro (1440 pt wide) as well as on a 5K display (2560 pt wide). The 2026-09-29
survey found ten text sizes in use (9, 11, 12, 12.5, 13, 13.5, 14, 15, 17, 18), four ways of
showing "selected", three segmented-control looks and a Library bar whose controls overprinted
each other at 1440 pt. This document is what stops that coming back.

**The one-line test for any new element:** *could it have been copied from the Kit board of the
canvas?* If not, extend this document first, then build it.

## 2. Principles

1. **The photo is the content; chrome is the surround.** Low chroma near the image, no accent on
   the canvas, the darkest surface under the photo.
2. **One of each.** One button, one segmented control, one field, one slider row, one section
   header, one list row, one dialog anatomy, one tooltip. A second variant needs a reason in §12.
3. **Explanation lives on hover, never on screen.** A control gets one hover sentence. No hint
   text sits under a control in a panel or a dialog. Empty states are the exception.
4. **Two rows, with priority.** The top row navigates and narrows; the status row informs and sets
   the view. Each row has a priority order and never wraps or overlaps at any width.
5. **State is the accent's job.** Selected, open, set: the accent. Buttons, chrome, canvas: grey.
6. **Nothing below 11 pt.** Four text sizes. If a label does not fit at 11, the layout is wrong,
   not the size.

## 3. Type

Inter is the text face (bundled, subset without its private-use block — `theme::PROPORTIONAL`).
Phosphor is the icon face. Hack is the monospace fallback and is never used for UI text.

| Role | Size | Weight | egui `TextStyle` | Used for |
|---|---|---|---|---|
| Caption | **11** | Regular | `Small` | section names (uppercase in the Library rail only, see §7.5), counts, thumbnail captions and badges, chips, kbd chips, footer status |
| Body | **13** | Regular | `Body`, `Button`, `Monospace` | labels, controls, list rows, menus, tooltips, slider values, the photo's name in the top row (in `strong`) |
| Title | **15** | Medium | named `"title"` | dialog titles |
| Display | **20** | Medium | `Heading` | empty-state headings, nothing else |

Rules:

- **Set sizes only through `TextStyle`.** A literal `FontId::proportional(n)` outside `theme.rs`
  is a defect. `RichText::small()` is Caption; `.heading()` is Display. The named `"title"` style
  is resolved through a `theme::title_font()` helper with a fallback, the `section_font` pattern.
- **Weight needs a face.** Inter **Regular** and **Medium** are both bundled
  (`crates/app/assets/`, one OFL licence beside them), subset the same way so their coverage
  matches: `pyftsubset --unicodes="U+0000-DFFF,U+F900-10FFFF" --layout-features='*'`. Medium is
  the named family `theme::MEDIUM` and is used for exactly four things: Title, Display, panel
  section names (§7.5) and sub-labels. Reach it through `theme::medium_font(ctx, size)`, which
  falls back to Regular where the bundled fonts are not installed (a bare harness context) rather
  than panicking; `titles_are_medium_where_the_fonts_are_installed_and_regular_where_not` pins
  both halves, and `no_bundled_font_shadows_an_icon` covers the Medium face too. Never fake
  weight with a larger size.
- **Icons are not text sizes.** Inline in text 14 · toolbar and buttons 16 · tool rail and empty
  states 24. Glyphs come from `icons.rs`, verified against the cmap (its warning stands). ⚠ One
  exception, drawn rather than a font glyph: the mask kinds' painted glyphs are 18 in their 36-pt
  buttons (option B of the 2026-09-29 canvas, as the author picked it).
- **Numbers that change while you watch** (readout, slider values, counts) are laid out at a
  fixed width measured from the widest value — the readout's precedent — never left to jump.
- **Case:** sentence case everywhere (buttons, menu items, section names, dialog titles). The
  only uppercase text is the Library rail's group names. British spelling in UI strings
  ("Colour"), as the menus already have; code identifiers stay as they are.
- **Interface size** (Settings, 100 / 125 / 150 %) is the per-machine knob. The sizes above are
  at 100 % and are what the author reads on a 13-inch display; on the 5K he may run 125 %.
  Never compensate for a display by picking a bigger size in code.

## 4. Colour

Two themes, every value a pair. Dark is the default. Numbers are WCAG contrast ratios against
the surface named; **text must be ≥ 4.5:1, non-text UI ≥ 3:1**, checked by test
(`every_accent_has_usable_contrast` is the shape).

### 4.1 Surfaces and text

| Token | Dark | Light | Role |
|---|---|---|---|
| `canvas` | `#121214` | `#dcdcdf` | the surround the photo is judged against; Develop canvas, Compare/Survey ground |
| `panel` | `#1b1b1e` | `#f4f4f5` | bars, rails, the Develop panel, dialogs, menus |
| `card` | `#232327` | `#ffffff` | a section of controls, a field, a list card in the rail |
| `raised` | `#303036` | `#e6e6ea` | buttons, combos, the lit segment, chips |
| `hover` | `#3b3b42` | `#dcdce1` | any raised or quiet control under the pointer |
| `line` | `#2b2b30` | `#d9d9de` | hairlines: bar edges, dialog head/foot, row separators |
| `text` | `#e6e6e8` (13.8:1) | `#1d1d1f` | body, titles, values |
| `text2` | `#a3a3a8` (6.8:1) | `#5f5f66` | captions, counts, secondary labels, quiet buttons at rest |
| `text3` | `#727278` (3.6:1) | `#8a8a90` (3.1:1) | decoration only: disabled, chevrons, thumbnail captions — never text that must be read |
| `tooltip` | `#0f0f11` | `#2a2a2e` | tooltip fill; text is `text` of the *dark* theme in both |

Elevation is the only depth: canvas < panel < card < raised < hover. No borders on controls
except fields (1 px `line`) and the outline button; no shadows except the lit segment (subtle)
and dialogs and menus (soft, large). The light-theme values are a first cut and must pass the
same contrast test before shipping.

### 4.2 The accent

One accent, chosen in Settings, doing exactly two jobs: **selection** (the selected photo's
outline, the open row's tint, the active tool) and **set state** (the slider's trailing fill,
the modified dot on a section, the app switch's lit segment, the one primary button). Never a
decoration, never on the canvas, never on the histogram.

| Accent | Dark theme | on-accent text | Light theme | on-accent text |
|---|---|---|---|---|
| Steel blue (default) | `#7fb0d8` (7.5:1) | `#0e1216` (8.2:1) | `#2f6a94` (5.3:1) | white (5.8:1) |
| Teal | `#5fb3ac` (7.0) | `#0e1216` (7.6) | `#2c6d68` (5.5) | white (6.0) |
| Violet | `#9a8fd0` (5.9) | `#0e1216` (6.5) | `#544a8c` (7.0) | white (7.7) |
| Orange | `#e39a5c` (7.4) | `#0e1216` (8.1) | `#b0521a` (4.7) | white (5.2) |
| Sage | `#9cc48a` (8.7) | `#0e1216` (9.6) | `#3e7a37` (4.7) | white (5.2) |
| Rose | `#dd8fa6` (7.0) | `#0e1216` (7.7) | `#a63d5f` (5.5) | white (6.1) |
| Neutral | `#9a9a9a` (6.1) | `#0e1216` (6.7) | `#5a5a5a` (6.3) | white (6.9) |

- **Text on the accent is dark in the dark theme and white in the light theme.** The current
  `primary_button` paints white on `#7fb0d8` (2.4:1); that changes.
- **Orange and Sage were deliberately absent** from the 2026-08-13 accent list: amber sits on the
  skin-tone and colour-temperature axis that Temp and Tint exist to judge, and green is the
  complement the eye adapts against. They are offered because the author asked (2026-09-29); the
  default stays Steel blue and the Settings hover for Orange says why a warm accent can mislead a
  white-balance judgement.
- Tints: an open row or active tool is the accent at **18 %** over its surface (`list_row::TINT`,
  `rail::TINT` — one constant, in `theme`).
- Hand-painted colours that stay outside the token set, listed so nobody hunts for them: the
  histogram's three channel fills, the crop frame's light neutral and halo, the mask overlay
  red, the clipping overlay's red/blue, the red-eye circle. Everything else goes through `theme`.

## 5. Spacing, sizes, shape

A 4-pt grid. Distances: **4** inside a control · **8** between controls · **12** between rows
of controls · **16** panel and dialog padding · **24** between sections in a dialog.

| Element | Size |
|---|---|
| Control row (buttons, fields, combos, slider rows, checkboxes) | **28** tall |
| List row (rail, mask list, snapshots, presets) | **30**; action cells **20** wide |
| Section header | **26** |
| Top bar | **44** |
| Status row | **32** |
| Menu row | **28**; menu padding 8 |
| Filmstrip | **92**; cells 70 tall |
| Tool rail | **48** wide; 32 px buttons; 24 px glyph |
| Library rail | **240** wide, resizable, floor 200 |
| Develop panel | **316** wide, resizable, floor 300 |
| Grid cell | 224 default, 96 – 400 by the size slider; gap 12; caption row 26 |
| Dialog | three widths, **340 / 520 / 680**; head 44; foot 52; body padding 16 |
| Tooltip | max width 280; padding 5 × 9 |
| Minimum window | **800 × 600** |

Corner radii: controls **6** · chips and pill segments **full** · cards **10** · thumbnails
**8** · dialogs and menus **12** · tooltips **6**. One radius per element in every state; a
control never changes shape on hover (the existing rule).

## 6. The shell

```
┌ top bar 44 ──────────────────────────────────────────────────────────────┐
│ Library: [Library|Develop] | [Grid|Compare|Survey] |   name · facts   | search 240 | Filter │
│ Develop: [Library|Develop] | ↶ ↷ | [− fit 1:1 +] | Before/After |   name · facts   | Export │
├────────────┬──────────────────────────────────────────────┬──────┬──────┤
│ rail 240   │ content                                      │panel │ tool │
│ (Library)  │                                              │ 316  │ rail │
│            │                                              │(Dev.)│  48  │
│            ├ status row 32 ───────────────────────────────┤      │      │
│            │ Library: count / job           chips · sort · size · ⋯ │   │
│            │ Develop: readout               progress / ready      │      │
│            ├ filmstrip 92 (Develop only) ──────────────────┤      │      │
└────────────┴──────────────────────────────────────────────┴──────┴──────┘
```

- **The top row navigates and narrows.** Nothing else may be added to it. Its priority, highest
  first: app switch · mode switch · search · filter (Library); app switch · undo/redo · zoom ·
  Before/After · name and camera line · Export (Develop). When a lower item does not fit, first
  its labels drop to icons (mode switch, Filter, Before/After), then it folds into ⋯. Search
  shrinks from 240 to 120 before anything folds.
- **The photo's name sits at the same x in both views** (`shell::centre_info`). Both views put
  what narrows or finishes at the right edge — the Library's search and Filter (Filter
  rightmost), Develop's Export — so the name is centred on the row and clamped into the gap
  **both** views leave (`shell::shared_gap` records each view's edges; Develop's are measured by
  `develop::toolbar_width` until it has been drawn). ⚠⚠ **The name is centred alone, never with
  its facts**: the Library knows no pixel size before a decode, so a centred name-and-facts block
  put the name 27 pt further left in the Library than in Develop. The facts follow the name and
  truncate into what is left. `the_centred_name_sits_at_the_same_x_in_both_views` and
  `the_name_does_not_move_with_its_facts` (sabotage-checked). Click the name to rename.
- **A publish service's view has a band above its grid** (`publish_panel::band`, M10.18d): **Publish N**
  as the primary button at the left end, the service's name and what is waiting ("3 new ·
  1 modified · 1 removed") at the right end, the status row's height and frame. Only in a service's view;
  the running publish and its Cancel stay in the status row's job slot. A view's own action is not
  the top row's (which only navigates and narrows) and was too hidden in the status row.
- **The status row informs and sets the view** (`status_bar.rs`). Library: at the left the photo
  and file count, or the running export / preview build / Immich download with its Cancel, or the
  last status message — truncated into whatever is left, never pushing; at the right the
  format chips, Download / Remove downloads (Immich sources with a selection), the sort, the size
  slider and ⋯ (Reindex, Expand all / Stack all, and anything folded). Develop: the readout swatch
  and numbers at the left; "Full resolution ready" or the running job at the right. Folding
  order: chips first, then the Immich buttons, then the size slider; the sort drops its label and
  narrows to the glyph and its arrow. The size slider is 20 pt thick, its knob smaller than a
  control row's (the author, 2026-11-24).
  ⚠ The first draft put the chips at the left and a zoom percentage in Develop's row; the chips
  sit with the view settings they are, and the zoom percentage was not built. ⚠ The first build
  also named the photo in the Library's status row; the name moved back to the top row so it does
  not jump when the view switches.
- **A bar is laid out by a plan, not by hope.** `rustroom_app::bar_plan` (pure, tested with a
  sweep from 100 to 2000 pt) decides what shows, what drops its label and what folds; the bin
  measures with the widgets' own fonts (`segmented::pill_width`, `chip_width`) and draws the
  result. Every row draws its right-hand cluster first and gives the left-hand text what is left.
  `nothing_in_either_library_row_overlaps` draws both rows at 1440, 1000 and 800 with a status
  longer than any row and fails on any two intersecting widgets.
- **The panels move** (M10.8b, the author's choices): Settings → Appearance sets the Library rail left or right, the
  edit panel left or right — its tool rail always on the panel's **outer** edge — and the filmstrip at
  the bottom, left or right, each remembered on its own. ⚠ Not in the View menu — they were, for one
  build, and the author moved them: where a panel sits is a setting, not a working toggle. ⚠ A strip on the edit panel's side sits at
  the **window edge**, so the sliders stay beside the photo; a bottom strip spans only the photo
  column, as the status row does. On a side the strip is a column, 150 wide by default and resizable,
  its cells 3:2 at the column's width. The diagram above is the default layout.
  `layout::develop_order` is the allocation order; `every_develop_layout_puts_each_piece_where_it_says`
  draws all six.
- **The panel's sections are cards on the panel surface** (§7.5); the Library rail's groups
  are uppercase headers without cards, so the rail reads as a list and the panel as controls.

## 7. Components

Each has one form. The Kit board of the canvas shows every state side by side.

### 7.1 Button
28 tall, 6 radius, Body text, 10 px side padding, icon 16 before the label with a 6 gap.
**Default** `raised`; **Primary** accent fill with on-accent text — one per bar or dialog (the
Export button; a dialog's verb); **Quiet** no fill, `text2`, `hover` fill under the pointer —
toolbar tools and dialog Cancel; **Icon** 28 × 28, quiet, *always* with a tooltip and an
accessibility label; **Disabled** 40 % opacity, never hidden. No outline variant in the app
(the kit's outline is for documentation).

### 7.2 Segmented control
A pill well on the `canvas` colour, 2 px inset, segments full-radius; the lit segment is `raised`
with a 1 px shadow and `text`; unlit segments `text2`. **Two sizes of one shape:** 28 tall with
Body labels for a primary switch (app, mode, mask mode, the tone curve's two rows, dialog tabs), 24
with Caption labels for a compact view switch in the panel (HSL, grading range). **The app switch
is the only segment lit with the accent.** A segment may carry a **mark** before its label
(`segmented::Mark`): an icon (the mask modes) or a dot in its own colour (the curve's Red, Green and
Blue); the mark counts in the width a cell must hold and never shrinks. Used for: app switch, mode
switch, zoom group, the curve's mode and channels, Retouch's Paint · Find (with icons), its brush and
its On the photo: Pick · Add more (with icons), mask modes, Interface size, dialog tabs.

**When it does not fit:** labels shrink 15 % — **never below Caption** (`segmented::shrunk_size`) —
and if that is not enough the items wrap into **balanced** rows: four that would fit three to a row
go two and two, never three and one (`segmented::plan`). ⚠ Until 2026-09-29 the shrink was
unconditional and the tone curve's five channels rendered at 9.35 pt in 20-pt cells, "all very
small"; the author then picked a new setup for the curve and the mask modes from the options at
<https://claude.ai/artifact/E5FSgBTobVtqkKvxvTNpL8>:

- **The tone curve (option A):** two rows at the control height — *Point curve · Parametric*, then
  *RGB · Red · Green · Blue* with channel dots. Parametric is a different kind of curve, not a
  fifth channel; under it the channel row is greyed and keeps the channel you left
  (`tone_curve::selectors`).
- **The mask modes (option B):** *New mask · Add · Subtract · Intersect* with an icon each, which
  wraps them into a 2 × 2 block of 32-pt segments at the panel's width; the kind buttons under them
  are 36 tall.

### 7.3 Field and combo
A **field** (you type) sits on `card` with a 1 px `line` border, placeholder in `text3`, an
icon 14 at the left when it is a search, a kbd chip at the right when it has a shortcut.
A **combo** (you pick) sits on `raised` with a chevron 14 at the right and no border. The two
must never look alike.

### 7.4 Slider row
Three columns: label 78 · rail · value 44, right-aligned, 10 gap, 28 tall. Rail 4 px on `line`;
the trailing fill in the accent runs **from the default value to the knob** (from the centre on
a bipolar slider, from the left on Temp); knob 14 px, `#f2f2f4`, soft shadow. Double-click
resets (existing) — and the hover does **not** say so; that is a property of every slider, taught
once in Help, not repeated on each. Every slider has a hover sentence from `slider_help` (§8).

### 7.5 Section (card) and header
A section is a `card` with 8 × 10 padding and 10 radius, separated by 8. Its header is 26 tall:
chevron 12 (left), name in Body **Medium**, sentence case; an accent dot when the group differs
from neutral; trailing text in Caption `text2` (the lens name, "9 built-in"); `reset` in Caption
quiet at the right while modified. The body is unindented (existing). Sub-labels inside a card
(Tone curve, Presence, Noise reduction) are Caption `text2` Medium, sentence case, 8 above.
The Library rail's group names (Folders, Collections…) use the same header but **uppercase
Caption** (`Header::rail`), so the rail reads as a table of contents and the panel as controls.
A card is painted **behind** its header and body (`HeaderOut::show_body`) — never for a body that
fills its container, whose card gap would grow a resizable panel every frame (the snapshot strip
did). Names are sentence-cased when painted (`header_text`), not renamed: they are also the
accessible labels and the Sync dialog's rows.

**A collapsible block inside a card** (`section::sub_header`) — the tone curve, globally and in a
mask: the section chevron, the name in the sub-label face, an accent dot while the block differs
from neutral, and its own Reset at the right while it is open. Collapsed, only that row remains.
Its open state is a **setting** (`Settings::tone_curve_open`, one for both curves) because egui's
memory does not survive a restart here, and a block the author collapsed "because it's quite large
and may be in the way" must stay collapsed.

⚠⚠ **A row that asks for more width than the rail has grows the rail every frame.** egui's side
panels widen to their content, so a row laid out wider than the panel pushes it out a little on
each repaint — seen in use as the Library rail "expanding indefinitely" while the pointer moved.
The rail's new-item rows (collection, smart collection, keyword) lay out at exactly the width
they are given, `+` first from the right (`collections_panel::new_item_row`);
`the_rail_keeps_its_width_while_the_pointer_moves` draws the rail across many frames and fails on
any growth.

### 7.6 List row
30 tall, 6 radius, name in Body. **The count ends 8 pt from the right edge**
(`tokens::TRAILING_INSET`), in Caption `text3` — the same x as a section header's count, so every count in the
Library rail stands in one column (the author's rule, 2026-09-29: "all the numbers should be to the
most right, with the buttons to the left of them"). A count column is at least four digits wide,
so the actions beside it line up too; a meta text (a snapshot's time) takes the same place at its
own width. **Hover** = `hover` fill. The row's actions are icon cells **left of the count**
(download, remove), **20 wide** (`tokens::ROW_ACTION`) and the row's height tall, and they are **always
drawn**: `text3` at 55 % at rest (`tokens::ACTION_REST` — faint by the author's choice, below §10's
non-text floor on purpose), `text` while the pointer is anywhere on the row, and the `raised` face under the
one the pointer is on (not `hover` — the row already wears it). A disabled action stays fainter still
(60 % of that), never pops and senses the pointer but not a click, so a click there reaches the row. (The author,
2026-09-29, reversing the first-use review's P-13, which had hidden them until hover.) **Open** = accent 18 % tint, count in `text2`. **Trees** (folders, dates, the Immich timeline)
indent 12 per level and carry a painted disclosure triangle in a 12-pt gutter left of the name
(`list_row::disclosure`); a count never goes inside a label — the Dates' "2024 (523)" was the
counter-example. `collections_panel`'s `every_count_in_the_rail_ends_at_one_x` checks the painted
text.

### 7.7 Chip
20 tall, full radius, Caption text on `raised`; a 6 px dot in the accent for a format that is
shown, `text3` for one that is hidden. Chips are filters and states, never buttons.

### 7.8 Dialog
`panel` surface, 12 radius, hairline border, soft shadow, backdrop 60 % `panel`. Head 44: Title
left, × icon button right, hairline under. Body padding 16, rows on a 130-pt label column.
Foot 52 with a hairline above: extras left, then **Cancel, then the primary verb at the far
right**; no note text. Escape cancels, Return presses the primary (existing). Width by class
(Compact 340, Standard 520, Wide 680); a dialog the user resized opens at that size next time.
Tabs inside a dialog are a segmented control (§7.2) under the head.

### 7.9 Tooltip
The only carrier of explanation. `tooltip` fill, 6 radius, hairline, Body text, 280 max width,
appears after 400 ms, one sentence in sentence case without a full stop, the shortcut appended
as a kbd chip ("Crop & Geometry  R"). Every control has one (§8).

### 7.10 Thumbnail cell
8 radius, letterboxed on `raised`. **Selected** = 2 px accent outline, 2 px offset. **Hover** =
the star row at the bottom left (existing). Badges are Caption white on 55 % black: a copy's label
at the picture's top left, the stack count at the **picture's** top right (`thumb::image_rect`) —
never the cell's corner, where a portrait's badge floated beside it.
Caption under the cell in Caption `text3`, elided at the cell width. In the filmstrip the same
cell at 70 tall without a caption. **Resting on a photo in the filmstrip shows its name**, which it
draws no caption for; in the grid the caption says it, so there is no tooltip (the author,
2026-09-30, after trying one). In both, the name is the cell's accessible name and description.

### 7.11 Tool rail
48 wide, 32 px buttons, 24 glyph in `text2`; the active tool is the accent glyph on an 18 %
accent tint; name on hover.

### 7.12 Status row
32 tall, hairline above, on `panel`. Left: information in Caption `text2`; a running job
replaces it with a quiet Cancel and the progress line. Right: view settings as combos, a size
slider (110 wide) and ⋯. It exists in Library and Develop at the same place.
When no job runs and a newer Rustroom is known, the left side leads with **the update notice**: "Rustroom 0.2.0 is
available" in Caption `text` (one step brighter than the information beside it — not the accent, which is for state)
with the hand cursor, opening the release page, then two small buttons in Caption (the Cancel's Body-sized form read
too large beside the row's Caption text — the author, 2026-11-24): **Hide** (until Rustroom next starts, nothing
saved) and **Don't show again** (turns Settings → Behaviour's switch off). It reads line, Hide, Don't show again in
either row, and gives way rather than push: it is drawn only where the information keeps its own width beside it, at
most 120 pt (`update_flow::STATUS_BESIDE`). While it is pending the Library row plans its right-hand side around it,
so the format chips go into ⋯ first (the author's pick, 2026-11-24).

### 7.13 Menus
`panel` with 12 radius and shadow, rows 28, Body, shortcut in Caption `text2` at the right,
separators hairline, submenu chevron 12. Context menus and the ⋯ menu use the same. A toggle or a choice
in a menu carries a **check mark** (`menu::checked`): a checkbox or a radio in the in-window bar, a
native check on macOS (M10.8b).

### 7.14 Empty state
Centred: Display heading, one Body sentence in `text2`, up to two buttons (one primary). The
one place explanatory prose is allowed on screen, because there is nothing else there.

**The Library's empty state is the start page** (M11.4, 2026-11-13, mockups at
<https://claude.ai/artifact/Hnj6DR4oeAh21SR1KGYjge>): a column of at most 620, the Display heading and its sentence,
the two Title-size buttons (Open a folder… primary), a **Continue** card (Caption sub-label, list rows with the counts
in the count column — All Photos, then up to five recent folders with their parent in `text3`), and beneath it the
**welcome** card until "Don't show this again": "What's where", two numbered maps of the window (Library, Develop) in
the surface tokens with accent discs, each with its key. The page scrolls when both are taller than the window. ⚠ No
drop hint, by the author's call. The heading follows the welcome: "Welcome to Rustroom" while it shows, "No photos
open" after.

### 7.15 Histogram
92 tall on a near-black surface, 6 radius, the three channel fills at 45–55 % opacity as today.

### 7.16 Progress
Never a bar of its own: the status row's left side (§7.12), with Cancel. The same idiom for
export, preview builds and Immich downloads (existing rule). **One exception (the author,
2026-10-18):** AI denoise's whole-photo job draws a quiet line under its Amount — "Full denoise" in
Caption `text2` over a 3 px line across the section, the done part `text3` on `line`, never the
accent, no percentage — because the view is done long before the photo is. **A second, the same
line (2026-10-20, asked for by the author: a press of Remove dust "often" showed nothing):** Retouch's
Remove dust drew it under its button while it looked — "Looking for dust" — and then left what it
did there as a readout until the next press; Find people (M10.16c3) did the same. **Since M10.19d
both are Retouch's Find card** (§13, 2026-10-22): the line runs under Find people / Find dust —
"Looking for people", "Looking for dust" — then gives way to the review's readout ("12 people · 9 to
remove", "3 spots · 3 to remove", or "No people found" / "No dust found"); a look again there keeps
the review and draws "Looking there" under it. All draw `develop/panel/progress_line.rs`.

## 8. Hints and help

- **Delete every persistent hint.** `theme::hint` under a control is retired: the camera-profile
  note, the curve's "click to add a point", the Compare and Survey key lines, dialog footer
  notes, the Settings explanations. `theme::error_hint` stays, shown only while the field is
  invalid, in `error_fg_color`.
- **Every control gets one hover sentence: what it does, here, exactly.** No second sentence,
  no "see also", no mention of a same-named control elsewhere, no "double-click to reset", no
  full stop, no ⚠. `theme::every_hover_is_one_short_sentence` sweeps every `on_hover_text("…")`,
  every `hover: "…"` and every `*_HINT` / `*_HELP` / `*_NOTE` / `HOVER` constant and fails on
  text over 110 characters, a second sentence, a semicolon, a newline, a trailing full stop, ⚠ or
  the reset phrase.
- **A slider's sentence depends on where it is** (`slider_help::text(scope, label)`). "Detail"
  in Noise reduction and "Detail" in Sharpening are different controls, so each **scope** —
  global, parametric curve, each HSL tab, tint by tone, look, noise reduction, sharpening,
  vignette, lens, each mask kind, patch, eye — has its own table, and a label that means
  different things in two places has **no** global entry, so a panel that forgets its scope gets
  no sentence rather than the wrong one. The panel sets `Edited::help_scope`; a slider without a
  sentence is recorded in `Edited::missing_help`, which both coverage tests
  (`every_edit_slider_has_hover_help`, `every_geometry_slider_has_hover_help`) require to be
  empty. `a_shared_label_says_what_it_does_where_it_is` and `an_ambiguous_label_needs_its_scope`
  pin the rule. **This is the shape to copy** for any other control that wants a table. Every
  other control carries its sentence at the call site.
- **Every clickable control has its sentence, and it is checked on the drawn app (M10.6b).** A
  sentence goes through `theme::Hover` — `.hover(sentence)` — which shows it and makes it the
  control's **AccessKit description**; `on_hover_text` is called nowhere else
  (`no_bare_on_hover_text_outside_theme`). The shared widgets take the sentence as a **required**
  argument — a dialog footer's primary (`Footer::confirm`), dialog tabs, segmented controls, chips,
  the mask/patch/preset row — so a new one cannot ship mute. `hover_coverage` draws every surface a
  harness reaches and fails on a clickable control (sliders included) with no description, or one that
  `slider_help::sentence_fault` rejects. **Exempt:** section headers (they carry an *expanded* state),
  menu items, text fields, and labels (selectable text, not controls). **Not reached, and said so:**
  Compare, Survey, the canvas overlays, the Import dialog.
- **What moved on 2026-09-29:** the camera-profile note (the combo), the curve's gestures (the
  curve; a value still shows while a point is in play), the parametric and defringe instructions,
  the Settings tier / resampler / Immich paragraphs (their controls, the chosen option's line
  first), the Import and Export token lines (`filename_template::token_hover`), the DNG note (the
  Format combo), the Information note (its field labels), and Compare's and Survey's key lines
  (Help → Keyboard shortcuts, in their own Compare and Survey sections — `modals::EXTRA_KEYS`). What stayed, at Caption: values and readouts, chosen paths, ticked counts,
  empty-list notes, transient states (an AI mask waiting), warnings, and dialog body sentences.
- **Keyboard shortcuts are discoverable three ways and no more:** the menu bar, the tooltip's
  kbd chip, and Help → Keyboard shortcuts. Never as prose in a bar.
- **Placeholders are not hints.** A field's placeholder names what goes in it ("Search photos",
  "New keyword") in `text3`.

## 9. Interaction and motion

- Hover: fill changes only (`hover`), never shape, never size. Pressed: `raised` one step
  darker. Focus ring: 2 px accent outline on the focused control for keyboard users.
- No animation of panels, sections or dialogs. Collapsing is instant.
- Cursor: the arrow everywhere except the I-beam in fields, the hand over links, the tool
  cursors on the canvas. Labels are not selectable text (the `empty_state` lesson).
- Double-click resets a slider; a click on the rail is an undo step (existing).
- Reorder by dragging the row itself (Library sections, masks, a mask's components): a 2-pt accent
  line marks the slot, and a slot that is not allowed shows no line. For list rows the slot is
  decided by the rows' midpoints — upper half before, lower half after. Where the order is an
  edit (masks), one drop is one undo step.

## 10. Accessibility floors

Text ≥ 4.5:1 including Caption; non-text ≥ 3:1 (one deliberate exception: a list row's action glyph
*at rest*, §7.6, which the pointer on the row brings to full contrast); targets ≥ 28 pt tall; every icon-only control
has an accessibility label (`widget_info`) *and* a tooltip — and every control's hover sentence is
also its AccessKit description, which a screen reader reads (§8); Escape / Return work in every
dialog; nothing conveyed by colour alone (the modified dot sits beside a name; a selected
segment is also lighter).

## 11. Where the tokens live

| Rule | Code |
|---|---|
| Text styles (§3) | `theme::tune`: `Small` 11, `Body`/`Button`/`Monospace` 13, `Heading` 20, named `"section"` 11 and `"title"` 15; `theme::title_font`, `theme::medium_font`; `no_literal_font_size_outside_theme` sweeps the sources |
| Hover sentences (§8) | `slider_help.rs` (lib: `Scope`, `HslTab`, `text`, **`sentence_fault`** — the one rule); `Edited::help_scope` / `missing_help`; **`theme::Hover`** (`.hover`, `.hover_titled`, `describe`); `hover_coverage.rs` walks the drawn app; `theme::every_hover_is_one_short_sentence` sweeps the sources |
| Surfaces, text greys, sizes, radii (§4.1, §5) | `theme::tokens` (`tokens::of(ui)` for a painter); `theme::surfaces` maps them onto egui's slots, per theme |
| Accent pairs, text on the accent (§4.2) | `settings::Accent` + `theme::accent_color`; `theme::on_accent`; `text_on_every_accent_is_readable` |
| Metrics (§5) | `theme::tune`: `interact_size.y` 28, `item_spacing` 8 × 2, `button_padding` 10 × 4, radius 6, dialogs and menus 12, `tooltip_delay` 0.4, `tooltip_width` 280 |
| Two rows and the plan (§6) | `shell::top_bar` + `plan_library_bar`, `shared_gap`, `centre_info`; `status_bar.rs` (bin); `bar_plan.rs` (lib, pure) |
| Segmented, pill, chip, swatches (§7.2, §7.7) | `segmented.rs`: the well, `pill` / `pill_width` / `icons_only`, `chip` / `chip_width`, `swatches` |
| Section card (§7.5) | `develop/panel/section.rs`: `Header::rail`, `HeaderOut::show_body`, `header_text` |
| Dialog anatomy (§7.8) | `modals/shell.rs`: head 44 with the title in `title_font`, foot 52; `theme::DialogSize` |
| Thumbnails (§7.10) | `thumb.rs` (`image_rect`, badge, radius 8); `library.rs` gap 12 and the stack badge |
| Checking a width | `RUSTROOM_DEBUG_WINDOW=1440x875` opens the window at that size (the author runs AeroSpace, which tiles windows — float the window first; see the drive memory) |

**Zero-GUI rule still applies:** plans, priority orders, help tables and token values are plain
data in the lib crate and testable headless; only the drawing lives in the bin modules.

## 12. Adding a new element — the checklist

1. **Which component is it** (§7)? If none fits, extend §7 first and add it to the Kit board.
2. **Text**: one of the four sizes, through `TextStyle`; no literal size.
3. **Colour**: a token; no literal `Color32` unless it is on the §4.2 hand-painted list.
4. **Size**: the row height from §5; spacing on the 4-pt grid.
5. **Help**: one hover sentence in the panel's help table; no text on screen.
6. **Place**: top row, status row, panel, rail or dialog by §6's rule — and its priority in that
   row, checked at 800 pt.
7. **State**: selection or set state through the accent; nothing else coloured.
8. **Both themes**, both contrast floors.
9. **Look at it** at 1440 pt and at 2560 pt (`drive` with a scratch profile; the memory note
   says how) before calling it done.
10. If you had to break a rule, change this document in the same commit and log it in §13.

## 13. Decisions log

| Date | Decision | Supersedes |
|---|---|---|
| 2026-08-13 | Dark, near-neutral, one accent; accents as dark/light pairs; compact density; amber excluded | egui defaults |
| 2026-08-24 | Phosphor as the icon font | text-only buttons |
| 2026-09-19 | Icon-only tool rail at 26; chevron left, actions right on section headers | labelled rail, chevron right |
| 2026-09-28 | Inter Regular as the text face; panel floor 300; segments shrink before wrapping; Interface size setting | Ubuntu Light |
| 2026-09-29 | **Direction B · Layered** chosen from three; four text sizes, nothing below 11; a status row and the two-row priority rule; hover text replaces every on-screen hint; seven accents including Orange and Sage at the author's request (default stays Steel blue); dark text on the dark-theme accent | ten sizes; one overloaded bar; persistent hints; four accents; white on accent |
| 2026-09-29 | The tone curve as two rows (Point curve · Parametric, then the channels with dots), collapsible, its state saved; the mask modes as a 2 × 2 block with icons and 36-pt kind buttons; a segmented label never below Caption, wrapped rows balanced | the five-channel compact selector at 9.35 pt; a 28-pt mask mode row; an unconditional shrink |
| 2026-09-29 | Counts in one column at the rail's right edge, actions left of them; Dates and the Immich timeline as tree rows; a default lens correction is the photo's starting point, not an edit; the filmstrip keeps the selected photo in place through a resize | counts left of the actions; counts inside labels |
| 2026-09-29 | **Inter Medium** bundled for Title, Display, section names and sub-labels; the photo's name back in the top row, centred alone at the same x in both views, with the Library's search and Filter at the right edge; one scoped hover sentence per control, no reset phrase | Regular-only titles; the name in the Library's status row; hovers that explained the same-named control elsewhere |
| 2026-09-29 | **Built.** Reindex moved back behind ⋯ with Expand all / Stack all — the 2026-09-23 call took it out of a one-item menu and set the condition "if a second Library action ever wants this corner, reconsider the menu then"; that happened | Reindex as a bar button |
| 2026-09-29 | A list row's actions **always drawn**, `text3` at rest and `text` under the pointer on the row, a `raised` face under the pointed one; action cells **20** wide (`tokens::ROW_ACTION`) — the author, after trying M10.6a | actions drawn only under the pointer (P-13); 30-wide cells |
| 2026-09-29 | The list-row action glyph at rest **fainter**: `text3` at 55 % (`tokens::ACTION_REST`), below the 3:1 floor by choice | `text3` at rest |
| 2026-09-30 | **Every clickable control carries one hover sentence, as its AccessKit description too** — one helper, required sentences on the shared widgets, and a walk over the drawn app (537 controls) | sentences at call sites under a source sweep only |
| 2026-10-01 | **The panels move** (M10.8b): the Library rail, the edit panel with its tool rail, and the filmstrip each on a side set in **Settings → Appearance** (first built as View items, moved there by the author); a strip on the panel's side at the window edge; a vertical filmstrip; check marks in menus, on the *Filmstrip* toggle | fixed sides; a bottom-only filmstrip; marks-less toggles |
| 2026-10-22 | **The Retouch panel as two tasks, Paint · Find** (M10.19d, the author's option B of three, from <https://claude.ai/artifact/7ACHJJ2zFpWPJ3WspdRYcC>): **Paint · Find** at the top decides what the photo does. Paint: one brush, **Remove · Heal · Clone · Eye**, whose settings take NEW retouches only. Find: **People · Dust** found and reviewed alike — shaded and bordered on the photo, **On the photo: Pick · Add more** (a click picks; a drawing looks again there, closer), Everyone, **Remove N** as the card's one primary, Cancel; Remove N returns to Paint with the brush that made them. Under both, **one Retouches list** of every kind (Removal, Person, Heal, Clone, Dust, Red eye, Pet eye, each numbered apart); a picked row selects it alone, switches to its brush, and shows its own settings under the list | five modes (Remove · Heal · Clone · Red eye · Pet eye); Detect objects, Refresh, Find people and Remove dust as buttons beside the brush; dust healed at once, unreviewed; three lists (removals, patches, eyes) and a Next patch row; a drawing over the people view that looked again with nothing on screen saying so |
| 2026-10-26 | The **HDR merge preview** (M10.17a): a Wide dialog whose body is a preview well on the `canvas` surface — the canvas's own idiom inside it (a spinner before the first picture, the last one under `theme::stale_dim()` while the next is made) — beside a 220-pt options column; its merge runs in the status row's job slot with Cancel, as an export | — |
| 2026-11-02 | A **publish service's band** above its grid (M10.18d, the author's pick of three): the name, the counts and **Publish N**; the New dot and Modified ring over a thumbnail in a dark rim (`theme::mark_rim`) | Publish N and the counts in the status row; the marks without a rim |
| 2026-11-03 | **Print** as a dialog (M10.18e, my call — the author skipped the questions): the page preview left (white paper, `theme::paper`; a caption shown as a grey line, `theme::paper_ink`), the settings right in the order a print is thought about — layout, paper, quality, destination; the footer reads **Print** with a listed printer chosen, else **Save PDF…** | A third module beside Library and Develop, Lightroom's shape |
| 2026-11-23 | **The update notice** in the status row (piece 4, my call within the author's "on, with a switch to turn it off"): Caption `text`, a click opening the release page, Hide for this run and Don't show again as small Caption buttons (the author's split, 2026-11-24); the switch **Check for new versions** in Settings → Behaviour, with the two launch switches | the accent for the line; a dialog or a toast at launch; Body-sized buttons (too large beside Caption, the author); a saved Hide per version |
