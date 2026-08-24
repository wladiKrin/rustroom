# Layout and IA rework — a tool rail, a menu bar, and a panel that matches the edit groups

Status: **steps 1 and 2 implemented 2026-08-24** on branch `layout-ia-rework` (the panel, then the
rail); **step 3 (the menu bar) not yet planned**.

⚠ Two things in this document were **corrected by implementation** and the corrections are marked
inline where they apply: the rail's four glyphs (all of the originals were absent from egui's fonts)
and the drawer's exclusivity (reversed in use). Read those marks rather than the surrounding prose.

## Scope

The four problems confirmed by the user on 2026-08-19, all of them structural rather than cosmetic:

1. **The toolbar is ~20 controls on one row with a single divider.** Nothing distinguishes a *mode*
   (Geometry) from an *action* (Export) from a *panel toggle* (Presets, Filmstrip). Counting the
   app-level controls that share the same row — Open file, Open folder, Library/Develop, the format
   chips — it is worse than the "thirteen" recorded in CLAUDE.md.
2. **The right panel stacks two organising principles.** Thirteen always-open ungrouped sliders with
   *no header at all*, then seven `CollapsingHeader` sections. The top half cannot be collapsed.
3. **Masks sits last, below Vignette**, purely because it was added last, though it is now a primary
   tool with its own canvas interaction.
4. **Presets is both a toolbar toggle and a dedicated left panel** — the only feature with two homes.

The user chose the **full IA rework** over the two cheaper options, and then added the menu-bar idea
unprompted. Both are in scope. The 2026-08-13 sequencing rule still governs *within* the work:
reordering controls inside an existing container is cheap; changing what containers exist is what
carries risk — which is why the **Sequencing** section puts the container changes last.

## The shape

Three surfaces replace one crowded row.

```
┌ Rustroom  File  Edit  View  Photo  Help ────────── native on macOS ─┐
├──────────────────────────────────────────────────────────────────────┤
│ Library │Develop│ Undo Redo │ Fit Before/After          DSCF1042.RAF │  ← 6 controls
├───────────────────────────────────────┬──────────────────┬──────────┤
│                                       │ histogram        │  ☰ Edit  │
│                                       │ ──────────────── │  ▣ Crop  │
│              image                    │ White Balance    │  ◑ Masks │  ← rail
│                                       │ Tone             │  ★ Presets│
│                                       │ Color            │          │
│                                       │ Detail           │          │
│                                       │ Effects          │          │
│                                       │ [Reset all]      │          │
│                                       │  (Edit tool)     │          │
├───────────────────────────────────────┴──────────────────┴──────────┤
│ filmstrip                                                            │
└──────────────────────────────────────────────────────────────────────┘
```

- **The rail** is on the **right edge, outermost**, so it sits against the panel it drives. Lightroom
  CC's arrangement. A left rail was considered — it keeps the image visually centred, which is
  Photoshop's and Capture One's reason — and rejected because it puts the control as far as possible
  from its consequence.
- **⚠⚠ The panel is EXCLUSIVE — reversed 2026-08-24, in use.** Selecting a tool replaces the panel's
  contents, so the global adjustment sections are reachable **only under `Edit`**. The original
  choice was Lightroom Classic's shape — a drawer *above* a permanent adjustment stack, so the global
  sliders stayed reachable while painting a mask — and it was reversed after trying it: mixing a
  tool's own controls with the global stack made it ambiguous which of the two a given slider
  belonged to, and a mask already carries its own Temp…Vibrance so nothing in the Masks tool needed
  the global ones. This is Lightroom CC's shape, and it was the recommended option at the original
  brainstorm.
  - Consequence: **`Reset all` lives under `Edit`**, because it resets the *global* edits — under
    Masks it would read as "reset this mask", which it is not.
  - Consequence: the heading is a **plain heading, not a `CollapsingHeader`**. With the tool's
    controls the only thing in the panel, collapsing them would leave it empty.
- **Presets becomes a rail tool**, so the left panel disappears entirely and the image gets its
  width. That resolves problem 4 by giving the feature exactly one home.

## The panel: sections *are* the edit groups

`EDIT_GROUP_NAMES` is `[White Balance, Tone, Color, Detail, Effects, Geometry, Masks]`. Geometry and
Masks become rail tools, leaving exactly five for the panel:

| section | contents |
|---|---|
| **White Balance** | Temp, Tint |
| **Tone** | Exposure, Contrast, Highlights, Shadows, Whites, Blacks · *hairline* · Texture, Clarity, Dehaze |
| **Color** | Vibrance, Saturation · *hairline* · `[Hue│Sat│Lum]` selector + 8 band sliders |
| **Detail** | Noise Reduction, then Sharpening |
| **Effects** | Vignette |

⚠ **This is the point of the choice, not a coincidence.** The panel then teaches the same five words
the Paste, Sync and Save-preset dialogs already use, and `merge_from`'s exhaustive destructure keeps
forcing *one* decision per new `Edits` field rather than two. The Lightroom-style `Basic` grouping
was rejected for exactly this: "Basic" spans three edit groups, so the panel and the Sync dialog
would name the same controls differently — the ambiguity being removed everywhere else.

Its honest cost: a two-slider White Balance section and a nine-slider Tone one. Lightroom groups
those together because they are used together. Accepted; collapsing is available.

White Balance and Tone open by default, the rest collapsed.

### Ordering — what stays, and why

Three things are already right and are recorded here so nobody later "fixes" them.

- **Group order stays White Balance → Tone → Color → Detail → Effects.** It is the order the
  decisions are actually made in — white balance is the foundation, tone rests on it, colour on tone,
  detail and effects are judged last — and it is already the order of `EDIT_GROUP_NAMES`, so panel
  and dialogs agree for free.
- **⚠ Panel order deliberately does NOT follow pipeline order.** The pipeline runs noise reduction
  and dehaze *early*, before white balance, because that is where the maths wants them. NR is
  *judged* last, at 1:1, once the tone is settled. The panel follows the workflow; the pipeline
  follows the maths. Lightroom's panels diverge from its pipeline for the same reason.
- **Exposure · Contrast · Highlights · Shadows · Whites · Blacks keeps Adobe's order.** No
  alternative is better enough to pay for relearning.

### Ordering — the five agreed changes

1. **HSL becomes a `[Hue│Saturation│Luminance]` segmented selector** showing 8 band sliders, instead
   of three nested `CollapsingHeader`s. Removes a third nesting level *and* the current oddity of a
   *Saturation* header sitting under a *Saturation* slider that means something else. This is
   Lightroom's own HSL panel. The strongest of the five.
2. **Vibrance above Saturation** — Adobe's order, and the gentler, more-often-correct control is
   offered first. Rustroom currently has them the other way round.
3. **Noise Reduction above Sharpening** inside Detail. You must denoise before you can judge
   sharpening, and it is the pipeline's own order. ⚠ A deliberate divergence from Lightroom, which
   puts Sharpening first — recorded as a choice, not an accident.
4. **A hairline (not a header) before Texture / Clarity / Dehaze** inside Tone. They are
   local-contrast tools rather than tonal ones, but they belong to the `Tone` edit group and must not
   become a section of their own.
5. **A mask's adjustments mirror the global order** — Temp, Tint, Exposure…Blacks, Vibrance,
   Saturation. One layout learned once.

⚠ **One collision left unresolved, deliberately.** Both Sharpening and Noise Reduction have a slider
named *Detail*, inside a section now also called *Detail*. Renaming either invents vocabulary
Lightroom users do not have, so the slider names stay and the sub-headers disambiguate.

## Architecture

| unit | crate / kind | job |
|---|---|---|
| `tool.rs` | `app` **lib** | `Tool { Edit, Crop, Masks, Presets }` — `ALL`, `label()`, `glyph()`, index for the 1–4 keys. Plain data, no egui. |
| `menu.rs` | `app` **lib** | The menu *model*: the `Command` enum plus a tree of sections, items and shortcuts. No egui, no muda. |
| `develop/rail.rs` | `app` **bin** | Draws the rail; returns the clicked `Tool`. |
| `menu_bar.rs` | `app` **bin** | Renders the model with `egui::containers::menu::MenuBar`. |
| `native_menu.rs` | `app` **bin**, `#[cfg(target_os = "macos")]` | Renders the same model as an NSMenu via `muda`. |

`develop/panel.rs` (392 lines) splits into `develop/panel/{mod,adjustments,drawers}.rs`. It would
otherwise hold five adjustment sections *and* four tool drawers in one file, which is the size signal
CLAUDE.md already names.

⚠ **`tool.rs` and `menu.rs` are lib modules and must stay egui-free.** `lib.rs`'s rule is what keeps
`history`, `selection` and `action_log` testable without a windowing system. Consequence: shortcuts
are a plain `Shortcut { command: bool, shift: bool, key: char }`, not `egui::KeyboardShortcut`, and
each renderer maps it.

### The load-bearing seam: one `Command`, one dispatcher, three input sources

```
rail click  ─┐
menu click  ─┼─→  Command  ─→  execute(app, cmd)   ← one exhaustive match
key press   ─┘
```

An exhaustive `match` in the dispatcher makes a forgotten command a **compile error** — the same
upgrade `merge_from`'s exhaustive destructure and `build_prefix_steps`'s key derivation already make
over a test.

### One field disappears

`geometry_mode` stops being stored on `Session` and becomes **derived**: `app.tool == Tool::Crop`.
Two facts that can disagree become one — the correction `render_max_dim` needed when it was a field
sitting beside `requested_region`.

⚠ **Consequence, and it is a real behaviour change:** the tool lives on `RustroomApp`, not `Session`,
so it **survives a photo switch**. Today Geometry resets when you arrow to the next photo. With a
rail it should not, and Lightroom does not. `selected_mask` stays per-photo, unchanged.

New view state: `app.hsl_channel` for the segmented selector. Like `selected_mask` it is view state
and must never reach `Edits`, a sidecar, or a paste.

## The menu bar

### Contents

| File | Edit | View | Photo | Help |
|---|---|---|---|---|
| Open file… ⌘O | Undo ⌘Z | Library G | Edit 1 | Keyboard shortcuts |
| Open folder… ⇧⌘O | Redo ⇧⌘Z | Develop D | Crop & Geometry 2 | About Rustroom |
| — | — | — | Masks 3 | |
| Export… ⌘E | Copy settings ⌘C | Fit ⌘0 | Presets 4 | |
| — | Paste settings ⌘V | Before / After \\ | — | |
| Settings… ⌘, | Paste settings… | — | Rotate left ⌘[ | |
| | — | Filmstrip | Rotate right ⌘] | |
| | Sync selection… | Show mask overlay¹ | — | |
| | Reset all | | Save as preset… | |

The **Photo** menu is what makes the rail keyboard-reachable, numbered 1–4 the way Lightroom numbers
its modules.

¹ the existing `app.show_mask_tint` flag, which today is a checkbox buried inside a selected
brush mask's controls and applies to every mask.

### What the toolbar keeps, and what duplicates on purpose

The toolbar keeps exactly six: **Library · Develop │ Undo · Redo │ Fit · Before/After**, with the
filename and camera model right-aligned where the develop panel's footer used to carry them.

⚠ **Every one of those six also appears in a menu, and that is deliberate**, not an oversight to be
tidied away later. They are the highest-frequency controls in the app; a menu is for reach and
discovery, a button is for speed. Everything *not* in that list leaves the toolbar entirely.

Three consequences that have to be decided rather than left to fall out:

- **`Reset all` moves from the bottom of the panel into the Edit menu.** It is a destructive whole-
  photo action sitting under a stack of sliders, which is the wrong altitude for it; it is already
  undoable, so the menu is a sufficient home.
- **Rotate left / right stay as buttons in the Crop drawer *and* gain menu items.** The buttons are
  contextual to the tool and need to stay next to the crop rectangle they move; the menu entries
  exist to carry the ⌘[ / ⌘] shortcuts, which is the same duplicate-on-purpose rule as the six above.
- **⚠ `Export…` and `Sync…` lose their dynamic counts.** Today they read `Export 3…` and `Sync 2…`,
  which is how the user learns the action applies to a *selection*. A menu item cannot carry a label
  that changes under the pointer as usefully. The count moves **into the dialogs**, which already
  name their targets, and the menu items read plainly. This is a small regression in discoverability,
  taken knowingly; if it bites, a count in the Library's status area is the contained fix.

### Why two renderers, and why Linux decides it

- **eframe has no native menubar.** [emilk/egui#3411](https://github.com/emilk/egui/issues/3411) is
  still open.
- **`muda`'s Linux backend is GTK-only**, and eframe/winit on Linux is X11/Wayland. muda's Linux path
  is therefore unusable for us, and this is a **Linux-first project**.
- **macOS is the easy case**: `Menu::init_for_nsapp()` installs into the global NSApplication menu
  and needs no window handle, so the eframe integration is build-once-at-startup plus polling
  `MenuEvent` per frame.
- **egui 0.35 ships a real `MenuBar`** (`egui::containers::menu`), which is the conventional place
  for a menu on Windows and Linux anyway.

So: native NSMenu on macOS, `egui::MenuBar` in the top panel everywhere else, both driven by the one
model. Shipping only the in-window bar first was considered and rejected — it would temporarily make
the user's own machine *worse* than today, an extra row beneath a nearly-empty system bar.

### ⚠⚠ The finding that shapes the shortcut design

**On macOS `NSMenu` consumes a key equivalent before winit ever sees it.** That is good — there is no
double-firing — but it has two consequences that must be built for, not discovered:

1. On macOS every shortcut arrives as a **muda `MenuEvent` → `Command`**. Everywhere else it arrives
   as an **egui key press → the same `Command`**.
2. The existing ad-hoc handling — `handle_clipboard_keys`, and the Ctrl+Z block inside
   `develop::toolbar` — **must be refactored to emit Commands rather than act directly**, or macOS
   silently loses them.

⚠ **And a genuine hazard.** `Ctrl/⌘+C` is currently gated on `ctx.memory(|m| m.focused())` so it does
not hijack typing in the preset-name field. A native menu cannot be gated that way. So **`Copy
settings` and `Paste settings` must be disabled on the native menu while a text field has focus** —
muda supports per-item enable/disable, and it has to be driven every frame the focus changes. Without
it, typing a preset name copies settings instead of text.

⚠ Note this interacts with an already-recorded trap: `egui-winit` matches the clipboard chords first
and pushes `Event::Copy`/`Event::Paste` *instead of* a `Key` event, and `Event::Paste` only fires
when the system clipboard already holds text. The Command path must not reintroduce a dependency on
either.

## The rail's appearance

**Glyph above text**, right edge, outermost. The user chose this over text-only knowing it
**reverses the recorded "no icons — everything stays text" decision**, which CLAUDE.md holds as a
decision taken rather than an omission.

⚠⚠ **The candidate glyphs named in this spec (▦ ⧉ ◐ ≡) are ALL ABSENT from egui's bundled fonts** —
measured, not guessed. The shipped set is **Edit ☰ · Crop ▣ · Masks ◑ · Presets ★**, every one
verified by `every_rail_glyph_is_renderable`. Do not reinstate the originals from this document.

⚠ **Rail typography, settled in use (2026-08-24).** The rail is **88 pt** wide with a **22 pt**
glyph over a **13 pt** label, and the entry is built from a `LayoutJob` with `halign: Center`. The
first version used a `"{glyph}\n{label}"` string at the default compact size in a 64 pt rail: the two
rows were left-aligned against each other so the glyph sat hard left of the word, the label was too
small, and "Presets" wrapped onto a second line. `halign` centres each row against the widest; the
surrounding `vertical_centered_justified` layout centres the block in the rail.

⚠ **Glyph coverage is not guaranteed and must not be assumed.** egui bundles Hack, Ubuntu-Light,
NotoEmoji and `emoji-icon-font`; nothing promises an arbitrary Geometric-Shapes character is in any
of them, and a missing one renders as a tofu box. The Geometry buttons prove *some* glyphs work
(⟲ ⟳ ⬌ ⬍), which is evidence for feasibility and not for any particular character.

**This is resolved by a test, not by looking once.** `epaint::Fonts::has_glyph` is reachable as
`ctx.fonts_mut(|f| f.has_glyph(&font_id, c))`, so `every_rail_glyph_is_renderable` sweeps `Tool::ALL`
and fails the build on a glyph that would ship as a box. Candidate characters (▦ ⧉ ◐ ≡) are
**provisional** — whichever set passes the test wins, and the test is written before the characters
are chosen.

## Testing

The brush canvas work produced **six bugs, every one found by using the app and none by a test**,
while the pure functions beside it carried ~30 tests and produced no defect at all. All six were
canvas wiring. This feature is entirely canvas and panel wiring, so the testing plan is the part of
this spec least open to trimming.

**Headless `RawInput` interaction tests for the rail.** The seam already exists and works —
`clicking_a_library_grid_thumbnail_opens_it`, `a_left_drag_does_not_pan`, `tool_drag_for` — it has
simply never been pointed at the canvas or the panel.

- `clicking_a_rail_entry_selects_that_tool`
- `selecting_crop_enters_geometry_mode` — pins the derived field
- `only_the_selected_tools_drawer_is_shown`
- `the_tool_survives_a_photo_switch` — pins the "One field disappears" behaviour change

**Model tests, all pure:**

- `every_rail_glyph_is_renderable` — the guarantee that makes the glyph choice safe
- `every_command_has_a_menu_home` — no command reachable only from a button
- `no_two_menu_items_share_a_shortcut`
- the dispatcher's exhaustive `match` — a compile error, not a test

⚠ **Each test gets a planned sabotage**, and the sabotage is **run through a helper that confirms it
compiled first**. That is the process finding from M3.1b: a sabotage that fails to compile looks
exactly like one that passes, because grepping for a failure finds nothing either way.

⚠ **The standing lesson applies with force here**: eleven recorded instances of a test naming a bug
it cannot detect, all sharing one shape — asserting a property at inputs where the bug is inert. For
each interaction test, the question to answer *while writing it* is which input makes the wrong
behaviour observable, not whether the assertion reads plausibly.

## Sequencing — one spec, three plans

Too large for a single plan, and the order puts the platform-specific risk last so it cannot block
anything else. Each step ships something usable on its own.

1. **The panel.** Five sections, the five ordering changes, the HSL segmented selector, the mask
   mirror, the `develop/panel/` split. No new containers, so this is the cheap contained half.
2. **The rail.** Rail and drawers; Crop, Masks and Presets become tools; the left preset panel and
   the Geometry toggle are removed; `geometry_mode` becomes derived; the tool moves to app level.
3. **The menu bar.** The model, the `Command` dispatcher, both renderers, the shortcut refactor, and
   the toolbar slimming to six controls.

## Accepted caveats

1. **A new dependency (`muda`), and the macOS renderer is `#[cfg]`-gated so Linux CI never compiles
   it.** It can rot without anything failing. Stated here rather than discovered later.
2. **The "no icons" decision is reversed.** Glyphs become a thing to maintain, and every future rail
   entry needs one that passes the coverage test.
3. **Tool selection survives a photo switch**, where Geometry used to reset. Deliberate.
4. **`show_presets` and its toolbar toggle are removed** from `Settings`, and the left panel with
   them.
5. **Native-menu items are only as gated as we remember to disable them.** The text-field hazard is
   mitigated by disabling Copy/Paste settings on focus, not eliminated.
6. **Section open/closed state is egui memory, not `Settings`** — it does not survive a restart.
   Persisting it is a contained follow-up if it turns out to be wanted.
7. **No rail reordering or customisation**, and no user-defined shortcuts.
8. **The Library view's toolbar is out of scope** beyond losing the controls that move into menus;
   the format chips stay where they are.
