# Rustroom design language

**Status:** the rulebook, chosen by the author on 2026-09-29 from three directions
(canvas: <https://claude.ai/artifact/JK6A3tSpaXPo427cHZTZ7h>, direction **B · Layered**).
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
| Body | **13** | Regular | `Body`, `Button`, `Monospace` | labels, controls, list rows, menus, tooltips, slider values |
| Title | **15** | Medium | named `"title"` | dialog titles, the open photo's name in the Develop bar, empty-state buttons |
| Display | **20** | Medium | `Heading` | empty-state headings, nothing else |

Rules:

- **Set sizes only through `TextStyle`.** A literal `FontId::proportional(n)` outside `theme.rs`
  is a defect. `RichText::small()` is Caption; `.heading()` is Display. The named `"title"` style
  is resolved through a `theme::title_font()` helper with a fallback, the `section_font` pattern.
- **Weight needs a face.** Only Inter Regular is bundled today; Title and Display need
  **Inter Medium** bundled and subset the same way (OFL, licence beside it). Until it is, Title
  and Display render Regular — do not fake weight with a larger size.
- **Icons are not text sizes.** Inline in text 14 · toolbar and buttons 16 · tool rail and empty
  states 24. Glyphs come from `icons.rs`, verified against the cmap (its warning stands).
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
| List row (rail, mask list, snapshots, presets) | **30** |
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
│ Library: [Library|Develop] | [Grid|Compare|Survey] | search 240 | Filter │
│ Develop: [Library|Develop] | ↶ ↷ | [− fit 1:1 +] | Before/After | name · camera line | Export │
├────────────┬──────────────────────────────────────────────┬──────┬──────┤
│ rail 240   │ content                                      │panel │ tool │
│ (Library)  │                                              │ 316  │ rail │
│            │                                              │(Dev.)│  48  │
│            ├ status row 32 ───────────────────────────────┤      │      │
│            │ Library: count · chips        | sort · size · ⋯     │      │
│            │ Develop: zoom · readout       | progress / ready    │      │
│            ├ filmstrip 92 (Develop only) ──────────────────┤      │      │
└────────────┴──────────────────────────────────────────────┴──────┴──────┘
```

- **The top row navigates and narrows.** Nothing else may be added to it. Its priority, highest
  first: app switch · mode switch · search · filter (Library); app switch · undo/redo · zoom ·
  Before/After · name and camera line · Export (Develop). When a lower item does not fit, first
  its labels drop to icons (mode switch, Filter, Before/After), then it folds into ⋯. Search
  shrinks from 240 to 120 before anything folds.
- **The status row informs and sets the view.** Library: photo and file count (or the running
  export/preview/download with its Cancel) and the format chips at the left; sort, thumbnail
  size and ⋯ (Stack all, Expand all, Reindex, Show in Finder) at the right. Develop: zoom
  percentage and the readout swatch with its numbers at the left; "Full resolution ready" or the
  running job at the right. Priority: count · sort · size · chips; chips fold first, then size.
- **A bar is laid out by a plan, not by hope.** A pure function (the `segmented::plan` shape,
  in the lib crate, tested at 1440 / 1000 / 800) decides what shows; the bin module only draws
  the result. A centred label is drawn only if the plan measured room for it after both ends.
- **Rail and panel are cards on the panel surface** with 12 padding; the rail's groups are
  collapsible cards, the panel's sections are cards (§7.5).

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
A pill well on the `canvas` colour, 28 tall, 2 px inset, segments full-radius; the lit segment
is `raised` with a 1 px shadow and `text`; unlit segments `text2`. **The app switch is the only
segment lit with the accent.** Icon + label when there is room, icon only (with tooltip) when
there is not; labels shrink 15 % before wrapping (`segmented::plan`). Used for: app switch,
mode switch, zoom group, curve channels, Retouch modes, mask modes, Interface size, dialog tabs.

### 7.3 Field and combo
A **field** (you type) sits on `card` with a 1 px `line` border, placeholder in `text3`, an
icon 14 at the left when it is a search, a kbd chip at the right when it has a shortcut.
A **combo** (you pick) sits on `raised` with a chevron 14 at the right and no border. The two
must never look alike.

### 7.4 Slider row
Three columns: label 78 · rail · value 44, right-aligned, 10 gap, 28 tall. Rail 4 px on `line`;
the trailing fill in the accent runs **from the default value to the knob** (from the centre on
a bipolar slider, from the left on Temp); knob 14 px, `#f2f2f4`, soft shadow. Double-click
resets (existing). Every slider has a hover sentence from the `slider_help` table (existing,
with coverage tests) — that table is the pattern for every other control (§8).

### 7.5 Section (card) and header
A section is a `card` with 8 × 10 padding and 10 radius, separated by 8. Its header is 26 tall:
chevron 12 (left), name in Body **Medium**, sentence case; an accent dot when the group differs
from neutral; trailing text in Caption `text2` (the lens name, "9 built-in"); `reset` in Caption
quiet at the right while modified. The body is unindented (existing). Sub-labels inside a card
(Tone curve, Presence, Noise reduction) are Caption `text2` Medium, sentence case, 8 above.
The Library rail's group names (Folders, Collections…) use the same header but **uppercase
Caption**, so the rail reads as a table of contents and the panel as controls.

### 7.6 List row
30 tall, 6 radius, name in Body, count or meta in Caption `text3` at the right; indent 14 per
level; disclosure chevron 12. **Open** = accent 18 % tint, count in `text2`. **Hover** = `hover`
fill and the row's actions appear at the right edge as icon buttons (download, remove).

### 7.7 Chip
20 tall, full radius, Caption text on `raised`; a 6 px dot in the accent for a format that is
shown, `text3` for one that is hidden. Chips are filters and states, never buttons.

### 7.8 Dialog
`panel` surface, 12 radius, hairline border, soft shadow, backdrop 60 % `panel`. Head 44: Title
left, × icon button right, hairline under. Body padding 16, rows on a 130-pt label column.
Foot 52 with a hairline above: extras left, then **Cancel, then the primary verb at the far
right**; no note text. Escape cancels, Return presses the primary (existing). Width by class
(Compact 340, Standard 520, Wide 680); a remembered resize applies to Wide only.
Tabs inside a dialog are a segmented control (§7.2) under the head.

### 7.9 Tooltip
The only carrier of explanation. `tooltip` fill, 6 radius, hairline, Body text, 280 max width,
appears after 400 ms, one sentence in sentence case without a full stop, the shortcut appended
as a kbd chip ("Crop & Geometry  R"). Every control has one (§8).

### 7.10 Thumbnail cell
8 radius, letterboxed on `raised`. **Selected** = 2 px accent outline, 2 px offset. **Hover** =
the star row at the bottom left (existing). Badge: Caption white on 55 % black, top right.
Caption under the cell in Caption `text3`, elided at the cell width. In the filmstrip the same
cell at 70 tall without a caption.

### 7.11 Tool rail
48 wide, 32 px buttons, 24 glyph in `text2`; the active tool is the accent glyph on an 18 %
accent tint; name on hover.

### 7.12 Status row
32 tall, hairline above, on `panel`. Left: information in Caption `text2`; a running job
replaces it with a quiet Cancel and the progress line. Right: view settings as combos, a size
slider (110 wide) and ⋯. It exists in Library and Develop at the same place.

### 7.13 Menus
`panel` with 12 radius and shadow, rows 28, Body, shortcut in Caption `text2` at the right,
separators hairline, submenu chevron 12. Context menus and the ⋯ menu use the same.

### 7.14 Empty state
Centred: Display heading, one Body sentence in `text2`, up to two buttons (one primary). The
one place explanatory prose is allowed on screen, because there is nothing else there.

### 7.15 Histogram
92 tall on a near-black surface, 6 radius, the three channel fills at 45–55 % opacity as today.

### 7.16 Progress
Never a bar of its own: the status row's left side (§7.12), with Cancel. The same idiom for
export, preview builds and Immich downloads (existing rule).

## 8. Hints and help

- **Delete every persistent hint.** `theme::hint` under a control is retired: the camera-profile
  note, the curve's "click to add a point", the Compare and Survey key lines, dialog footer
  notes, the Settings explanations. `theme::error_hint` stays, shown only while the field is
  invalid, in `error_fg_color`.
- **Every control gets one hover sentence**, held in one table per panel or dialog with a
  coverage test that fails when a control is added without one — the `slider_help` shape.
  Buttons, sliders, combos, checkboxes, chips, rail entries, list-row actions, tool-rail entries,
  segments.
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

## 10. Accessibility floors

Text ≥ 4.5:1 including Caption; non-text ≥ 3:1; targets ≥ 28 pt tall; every icon-only control
has an accessibility label (`widget_info`) *and* a tooltip; Escape / Return work in every
dialog; nothing conveyed by colour alone (the modified dot sits beside a name; a selected
segment is also lighter).

## 11. Where the tokens live

| Rule | Code |
|---|---|
| Text styles (§3) | `theme::tune`: `Small` 11, `Body`/`Button`/`Monospace` 13, `Heading` 20, named `"title"` 15; `theme::title_font` |
| Surfaces, text greys, tooltip (§4.1) | `theme::surfaces`, per theme; a `Tokens` struct so hand-painters (`thumb`, `list_row`, `rail`, `section`, `segmented`, `modals::shell`) read the same values |
| Accent pairs, on-accent text (§4.2) | `settings::Accent` + `theme::accent_color`; a new `theme::on_accent(dark)`; `every_accent_has_usable_contrast` gains the text check |
| Spacing, row heights, radii (§5) | `theme::tune`: `item_spacing` 8 × 6, `interact_size.y` 28, `slider_rail_height` 4, `corner_radius` 6; the row constants move from `list_row`, `section`, `rail`, `shell::BAR_HEIGHT` into `theme` |
| Two rows and the plan (§6) | `shell::top_bar` slims down; a new bin module `status_bar` draws; a new lib module `bar_plan` (pure, tested) decides what fits |
| Segmented control (§7.2) | `segmented.rs`: pill well, lit segment raised, accent only on the app switch |
| Section card (§7.5) | `develop/panel/section.rs` header 26 and the card frame; `theme::sub_label` sentence case |
| Dialog anatomy (§7.8) | `modals/shell.rs`: title in `"title"`, × as an icon button, no footer note |
| Tooltips (§7.9, §8) | `theme::help(ui, resp, text)` as the one entry point; per-panel help tables with coverage tests |
| Thumbnails (§7.10) | `thumb.rs` badge and caption at Caption; `library.rs` gap 12 |
| Fonts (§3) | bundle Inter Medium beside Regular, subset the same way; `no_bundled_font_shadows_an_icon` covers it |

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
