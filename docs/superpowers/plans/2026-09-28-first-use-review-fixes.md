# First-use review fixes — plan

**Branch:** `first-use-fixes` from `main` `1abb9ff`. **Input:** the two reviews in `docs/reviews/`
(untracked, transient; ids A-nn and P-nn). Small items, knocked off in groups that share files so
each group is one gate run and one commit.

Status: ☐ open · ✅ done · ⛔ declined (reason) · ❓ waiting on the author.

## G1 · Strings and glyphs (no behaviour change)

| Id | Fix | Where | |
|---|---|---|---|
| A-01/P-01 | no glyph outside the bundled cmaps: the empty-state arrow, the shift chord, the Compare "<- ->", the Export date-row box | `library.rs:319`, `menu.rs` `Shortcut::label`, `multi/compare.rs:55`, `modals/export.rs` ~339 | ☐ |
| A-01 | a test that every menu label, shortcut label and known dialog string is covered by the fonts | new, lib | ☐ |
| A-07 | label the RGB readout and give it hover text | Develop top bar | ☐ |
| A-16 | "Copy 0" → "Original" | `modals/info.rs:152` | ☐ |
| A-21/P-17 | Compare/Survey hint lines at body size | `multi/compare.rs:285`, `multi/survey.rs:174` | ☐ |
| A-25 | profile note in plain words | `develop/panel/mod.rs:494` | ☐ |
| A-26 | reword "Show: Edited, else RAW"; hover text on the stack badge and the format chips | `modals/settings.rs`, `library.rs` | ☐ |
| A-15 | Immich tab leads with one sentence saying what Immich is (the on-by-default gate is pinned by an upgrade test and stays) | `modals/settings.rs` | ☐ |

## G2 · Menus and keys

| Id | Fix | Where | |
|---|---|---|---|
| A-02 | escape `&` for muda so "Crop & Geometry" survives | `native_menu.rs:139` | ☐ |
| A-18 | one tool name everywhere: rail tooltip, menu, shortcuts | `tool.rs:37` + tests | ☐ |
| A-03 | remove AppKit's injected items after install (Emoji & Symbols, AutoFill, Show Tab Bar, Show All Tabs, Enter Full Screen) | `native_menu.rs` | ☐ |
| A-04 | bare-key chords shown inside labels | — | ⛔ a native accelerator without modifiers fires while typing; muda has no other right-aligned slot |
| A-25 | soft proof + rendering intents under one "Colour management" submenu — needs `Item::Submenu` in the model, the egui bar and the native bar | `menu.rs`, `menu_bar.rs`, `native_menu.rs`, shortcuts dialog | ☐ |
| A-08 | "Open recent ▸" (last folders, from the catalog's folder list) | same seam as above | ☐ |
| P-10/A-12 | digits rate in Develop too; tools move to letters (E, R, K, Q, T) | `menu.rs`, `shell.rs` `egui_key`, `shell.rs` frame loop | ☐ |
| P-27 | Tab hides the panels and filmstrip | new `Command::TogglePanels` | ☐ |
| A-27 | shortcuts dialog mentions double-click-to-reset and the culling keys | `modals/mod.rs` | ☐ |

## G3 · Develop panel

| Id | Fix | Where | |
|---|---|---|---|
| A-05/P-03 | panel minimum width (the 320 default is a *wrapping* width, so the panel shrank to ~214) | `develop/panel/mod.rs:140` | ☐ |
| A-20/P-02 | re-check the four wrapping segmented controls at the new floor; shrink labels if any still wrap | `segmented.rs` | ☐ |
| A-24 | one-sentence hover text on every slider label, from a lib table with a test that no slider lacks one; every entry ends "Double-click resets." | new lib module + `Edited::slider` | ☐ |
| P-15 | histogram height doubled | `develop/panel/mod.rs` `HIST_HEIGHT` | ☐ |
| P-11/A-14 | Scale resets to its default (−100); find why the sweep missed it | `geometry.rs:429`, `adjustments.rs:1079` | ☐ |
| A-17 | Auto reports what it did in the status line | `adjustments.rs:300` | ☐ |
| P-05 | JPEG quality hidden for lossless formats | `modals/export.rs:479` | ☐ |
| A-25 | Export: colour space and intent move to a collapsed "Colour" section | `modals/export.rs` | ☐ |
| A-13 | Export defaults to the photo's folder; the button is never disabled on open | `modals/export.rs`, `export_summary.rs` | ☐ |
| P-04 | Escape closes a dialog even with a text field focused; global shortcuts swallowed while a modal is open | `modals/mod.rs`, `shell.rs` | ☐ |
| P-14 | Settings dialog sized to content | `modals/settings.rs:273` | ☐ |

## G4 · Canvas

| Id | Fix | Where | |
|---|---|---|---|
| A-06 | left-drag pans in the Edit tool when zoomed; grab cursor; the zoom tooltip says so | `canvas/navigation.rs`, `canvas/mod.rs`, `develop/mod.rs` | ☐ |
| P-08 | Before keeps the profile rendering, lens correction and look | `session_flow.rs:30` | ☐ |
| P-09 | Compare/Survey borders use the accent | `multi/mod.rs:201`, `multi/survey.rs:324` | ☐ |
| P-06 | composition guide only in Geometry by default | `canvas/mod.rs` | ❓ reverses a recorded decision (guides under every tool so `O` works everywhere) |
| A-19/P-07 | crop frame neutral, with handles | `crop_overlay.rs:153` | ❓ red is a recorded decision (legible on any photo, independent of the accent) — handles yes, colour is the author's call |

## G5 · Library

| Id | Fix | Where | |
|---|---|---|---|
| A-08 | empty-state card with two buttons; a dropped folder or files open | `library.rs:319`, `shell.rs` | ☐ |
| A-09 | larger default cell | `settings.rs:645` `DEFAULT_CELL` | ☐ |
| A-10 | "9 photos · 18 files" when stacks hide files | `library.rs` toolbar | ☐ |
| A-11 | expanded-stack children get a collapse chevron with hover text, one colour | `library.rs:589` | ☐ |
| A-12/P-16 | context menu: Rating ▸ Flag ▸ Colour ▸ Export… Create virtual copy Rotate Reveal in Finder | `library.rs:110` | ☐ |
| P-13 | folder names elide with "…" (full path on hover); × only on hover | `folder_rows.rs`, `list_row.rs` | ☐ |
| P-12 | a spinner on an AI mask's row while its map is pending; its sliders disabled | `develop/panel/masks.rs`, `ai_flow.rs` | ☐ |

## G6 · Type and scale

| Id | Fix | Where | |
|---|---|---|---|
| P-18 | bundle Inter (OFL) as the proportional face; check its cmap covers the glyphs G1 needs | `theme.rs` `install_fonts` | ☐ |
| A-22 | Appearance → Interface size 100/125/150 % (egui zoom factor) | `settings.rs`, `modals/settings.rs`, `settings_apply.rs` | ☐ |
| P-31 | run the light theme for a look; fix what is plainly wrong | `theme.rs` | ☐ |

## G7 · Starter presets

| Id | Fix | Where | |
|---|---|---|---|
| A-23/P-22 | nine built-in presets (Natural, Punchy, Soft, Warm, Cool, Black & white, Matte, High key, Faded), embedded, listed under "Built-in", not deletable; the author retunes the values | `presets.rs`, `develop/panel/presets.rs` | ☐ |

## Not in this pass

P-20 previews · P-21 catalog scale · P-22 camera profiles/LUTs · P-23 AI denoise · P-24 HDR/pano ·
P-25/P-26/P-28 scope · P-29/P-30 decisions · P-27's J/I overlays and lights-out · A-23's preset
thumbnails · A-12's hover stars on thumbnails (the context menu and keys cover it).

## Gate

`cargo fmt --all --check` · `CARGO_TARGET_DIR=target/clippy cargo clippy --workspace --all-targets -- -D warnings` ·
`cargo test --workspace` · the release GUI driven for screenshots of every changed place.
