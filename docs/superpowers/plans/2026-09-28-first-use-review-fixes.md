# First-use review fixes — plan

**Branch:** `first-use-fixes` from `main` `1abb9ff`. **Input:** the two reviews in `docs/reviews/`
(untracked, transient; ids A-nn and P-nn). Small items, knocked off in groups that share files so
each group is one gate run and one commit.

Status: ☐ open · ✅ done · ⛔ declined (reason) · ❓ waiting on the author.

## G1 · Strings and glyphs (no behaviour change)

| Id | Fix | Where | |
|---|---|---|---|
| A-01/P-01 | no glyph outside the bundled cmaps: Inter is the text face (it has ⇧ ⌘ → ←), Hack is the proportional fallback for ▸ ▾ ▴; the Compare hint reads "← →" again | `theme.rs` `install_fonts`, `multi/compare.rs` | ✅ |
| A-01 | `every_menu_string_is_renderable`: every menu label, shortcut label, tool name, hint string and shortcuts-dialog line through the installed fonts; proven to fail on ✕ | `theme.rs` tests | ✅ |
| A-07 | the RGB readout has hover text (the swatch stays; it replaced the letters on purpose) | `shell.rs` | ✅ |
| A-16 | "Copy 0" → "Version: Original" / "Copy n" | `modals/info.rs` | ✅ |
| A-21/P-17 | Compare/Survey hint lines at body size; both are named constants | `multi/compare.rs`, `multi/survey.rs` | ✅ |
| A-25 | profile note in plain words | `develop/panel/mod.rs` | ✅ |
| A-26 | "Edited file, otherwise RAW"; the stack badge already had hover text | `stack.rs`, `modals/settings.rs` | ✅ |
| A-15 | Immich tab leads with one sentence saying what Immich is; the on-by-default gate is pinned by an upgrade test and stays | `modals/settings.rs` | ✅ |

## G2 · Menus and keys

| Id | Fix | Where | |
|---|---|---|---|
| A-02 | `&` escaped for muda; "Crop & Geometry" survives | `native_menu.rs` `native_label` + test | ✅ |
| A-18 | one tool name everywhere: `Tool::Crop.label()` is "Crop & Geometry" | `tool.rs` + test | ✅ |
| A-03 | window tabbing off (no Show Tab Bar / Show All Tabs); AutoFill, Emoji & Symbols, Enter Full Screen stripped by title, repeated during the first 5 s of polling because AppKit adds them after install | `native_menu.rs` | ✅ verify in use |
| A-04 | bare-key chords shown inside labels | — | ⛔ a native accelerator without modifiers fires while typing; muda has no other right-aligned slot |
| A-25 | soft proof + rendering intents under one "Colour management" submenu — `Item::Submenu` in the model, `menu::walk` as the one traversal for both bars, the shortcut matcher, the shortcuts dialog and the glyph audit | `menu.rs`, `menu_bar.rs`, `native_menu.rs`, `modals/mod.rs` | ✅ |
| A-08 | "Open recent ▸" from `Settings::recent_folders` (last 8, remembered on every folder open; the native menu is rebuilt then) | `menu.rs`, `settings.rs`, `sources.rs`, `commands.rs` | ✅ |
| P-10/A-12 | digits, P, X, U rate in Develop too (`handle_develop_culling`, auto-advance honoured); tools moved to E R K Q T; `no_bare_shortcut_is_a_culling_key` guards it | `interaction.rs`, `menu.rs`, `shell.rs` | ✅ |
| P-27 | Tab hides the panels, the rail and the filmstrip (`Command::TogglePanels`) | `commands.rs`, `develop/mod.rs`, `shell.rs` | ✅ |
| A-27 | shortcuts dialog lists the rating keys and the slider reset | `modals/mod.rs` `EXTRA_KEYS` | ✅ |

## G3 · Develop panel

| Id | Fix | Where | |
|---|---|---|---|
| A-05/P-03 | `min_size(300)` on the panel — `default_size` is a wrapping width under egui 0.35 and the panel came up at ~214 pt | `develop/panel/mod.rs` | ✅ verify in use |
| A-20/P-02 | the wrapping segmented controls | — | ☐ re-check at the new floor in use |
| A-24 | `slider_help::text(label)`: one sentence per slider, on the label's hover, "Double-click resets it." appended once; coverage tests over the Edit and Geometry panels; the inline hovers moved into the table | new lib module, `slider.rs`, `develop/panel/mod.rs` | ✅ |
| P-15 | histogram 72 → 120 pt | `develop/panel/mod.rs` | ✅ |
| P-11/A-14 | Scale resets to `Perspective::default().scale`; `scale_resets_to_its_own_default_on_double_click` (the Edit-panel sweep never sees Geometry) | `geometry.rs` | ✅ |
| A-17 | Auto says what it set in the status bar (`Edited::note`) | `adjustments.rs`, `develop/panel/mod.rs` | ✅ |
| P-05 | JPEG quality hidden for lossless formats | `modals/export.rs` | ✅ |
| A-25 | Export: colour space and intent in a collapsed "Colour" section with its own trailer | `modals/export.rs`, `export_summary.rs` | ✅ |
| A-13 | Export defaults to the first photo's folder | `commands.rs` | ✅ |
| P-04 | Escape read before the dialog body draws; global shortcuts skipped while a modal is open | `modals/shell.rs`, `shell.rs` | ✅ |
| P-14 | Settings dialog size | — | ⛔ the 932×650 was a persisted resize; the default is 480×320 |

## G4 · Canvas

| Id | Fix | Where | |
|---|---|---|---|
| A-06 | left-drag pans in the Edit tool when zoomed and no eyedropper is armed; grab cursor; the 1:1 tooltip says so; `a_left_drag_pans_only_when_the_edit_tool_allows_it` | `canvas/navigation.rs`, `canvas/mod.rs`, `develop/mod.rs` | ✅ |
| P-08 | Before keeps `rendering` and `lens_correction` | `session_flow.rs` + test | ✅ |
| P-09 | Compare's border is the accent; Survey's green and red are the Pick/Reject colours (semantic, kept) | `multi/mod.rs` | ✅ |
| P-06 | composition guide only in Geometry by default | `canvas/mod.rs` | ❓ reverses a recorded decision (guides under every tool so `O` works everywhere); off by default on a fresh profile either way |
| A-19/P-07 | crop frame colour and handles | `crop_overlay.rs` | ❓ red is a recorded decision (legible on any photo, independent of the accent); corner brackets already exist |

## G5 · Library

| Id | Fix | Where | |
|---|---|---|---|
| A-08 | empty-state card: "No photos yet", two 240-pt buttons, one sentence; a dropped folder or photo opens | `library.rs` `empty_library`, `shell.rs`, `sources.rs` `open_dropped` | ✅ verify in use |
| A-09 | `DEFAULT_CELL` 168 → 224 | `settings.rs` | ✅ |
| A-10 | "9 photos · 18 files" when stacks hide files | `sources.rs` | ✅ |
| A-11 | expanded-stack children show ▴, not × | `library.rs` | ✅ |
| A-12/P-16 | context menu: Rating ▸ Flag ▸ Colour ▸, Export…, Create virtual copy, Show in Finder (`reveal.rs`) | `library.rs` | ✅ verify in use |
| P-13 | row names elide with "…"; row actions drawn only under the pointer (still present for a screen reader) | `list_row.rs` | ✅ |
| P-12 | an AI mask whose map has not landed shows a spinner row and keeps its sliders disabled | `develop/panel/masks.rs` | ✅ |

## G6 · Type and scale

| Id | Fix | Where | |
|---|---|---|---|
| P-18 | Inter 4.1 Regular bundled as the proportional face (OFL, licence beside it) | `theme.rs`, `assets/` | ✅ verify in use |
| A-22 | Settings → Appearance → Interface size 100 / 125 / 150 % (`UiScale`, egui zoom factor) | `settings.rs`, `theme.rs`, `modals/settings.rs`, `settings_apply.rs` | ✅ |
| P-31 | the light theme, looked at | — | ☐ |

## G7 · Starter presets

| Id | Fix | Where | |
|---|---|---|---|
| A-23/P-22 | nine built-in presets (Natural, Punchy, Soft, Warm, Cool, Black & white, Matte, High key, Faded) carrying tone + colour, listed above the user's under "Built-in", not deletable; `the_starter_presets_each_do_something_different` | `presets.rs` `builtin`, `develop/panel/presets.rs` | ✅ values are the author's to retune |

⚠ A preset replaces the groups it carries: a starter that sets contrast also sets exposure back
to 0, as every user preset does. Thumbnails of the current photo (A-23's ask) are not in this pass.

## Not in this pass

P-20 previews · P-21 catalog scale · P-22 camera profiles/LUTs · P-23 AI denoise · P-24 HDR/pano ·
P-25/P-26/P-28 scope · P-29/P-30 decisions · P-27's J/I overlays and lights-out · A-23's preset
thumbnails · A-12's hover stars on thumbnails (the context menu and keys cover it).

## Gate

`cargo fmt --all --check` · `CARGO_TARGET_DIR=target/clippy cargo clippy --workspace --all-targets -- -D warnings` ·
`cargo test --workspace` (3173 on 2026-09-28) · the release GUI driven for screenshots of every changed place.
