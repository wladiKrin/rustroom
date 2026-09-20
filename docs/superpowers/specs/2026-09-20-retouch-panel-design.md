# The Retouch panel: list first — design

**Date:** 2026-09-20. **Branch:** `retouch-panel`. **Status:** option C of the canvas "Rustroom
Retouch Panel" (https://claude.ai/artifact/QZJjScKCCWxdZPbmzgss92), plus the user's addition:
the show and visualize functions move onto each patch's row, next to its `×`, mirroring the
Masks list.

## Why

The panel is a plain heading, a line of small print saying whether the sliders edit the next
patch or the selected one, Heal and Clone as loose chips under the sliders, two checkboxes, a
count with Clear all beside it, and a line of instructions. Patches can only be seen, selected
or deleted on the canvas.

## The panel, top to bottom

```
› PATCHES •                                  3 patches   reset
  ◎ Next patch   Heal · 24 px           (lit whenever no patch is selected)
  ◎ Patch 1      Heal · 24 px                   ◐  👁  ×
  ◎ Patch 2      Heal · 31 px   (selected)      ◐  👁  ×
  ◎ Patch 3      Clone · 60 px                  ◐  👁  ×
                                       [Show all] [Hide all]
  [ Heal | Clone ]
  Size ──────●───── 24 px
  Feather ────●──── 50
  [Find dust]   Threshold ────●──── (when on)
```

### The list

- `Tool::Retouch.panel_heading()` returns `None`; the PATCHES section header is the heading:
  `modified` = any patches, `reset` = the old "Clear all" (same clearing, one history entry),
  trailing text = the count ("3 patches", "1 patch", "no patches").
- **The Next patch row** is always first. It is lit (the weak "current" fill) whenever no real
  patch is selected; clicking it deselects the selected patch. Its meta shows the tool's
  defaults (mode and size), because that is what the sliders below edit then. It has no
  toggles and no `×`.
- **One row per patch**: a mode glyph (Heal: a cross over a dot; Clone: two circles), the name
  "Patch n", a weak meta "Heal · 24 px" (`spot_tool::radius_px`), and three cells at the right,
  in this order from the `×` inward: `×` delete, an **eye** for the outline, and a **hold-out**
  toggle. Clicking the row selects the patch (`selected_spot`), as clicking it on the canvas
  does; the selected row is tinted like the masks. Accessible names: the row is "Patch n",
  the `×` "Delete Patch n", the eye "Show Patch n", the hold-out "Hold out Patch n".
- **The eye** is today's "Show patches" per patch: on = the outline is drawn on the canvas.
  New patches start shown. `Session::spot_shown: Vec<bool>`, resynced every frame like
  `mask_shown` (a patch that arrives by undo, paste or a snapshot starts shown, since outlines
  default on today). The app-level `show_spots` flag goes; the canvas takes the vector.
- **Hold out** is the per-patch analogue of the masks' eye: on = the patch is held out of the
  view so you see what it covers, its outline still drawn. This is the suppression a slider drag
  already uses, made a toggle. `Session::spot_held: Vec<bool>`, resynced every frame, default
  off; `view_edits` removes every held patch plus the drag-suppressed one, highest index
  first. The drag suppression (`suppressed_spot`) is unchanged and still owned by whoever
  started it.
- **Show all / Hide all** under the list: every eye at once, labelled by what it will do;
  hidden with no patches.

### The patch settings

Under the list, editing the lit row: the mode as `theme::segmented_sized` at 28 pt (Heal,
Clone, with the two hovers), then Size (logarithmic, in px) and Feather, exactly as today
including the drag suppression and the "tool defaults follow the patch" rule. The Size hover
carries the placement instructions ("Click on the image to place a patch; drag out to size it;
⌥-drag to choose the source yourself. Delete removes the selected patch.").

### Find dust

Today's "Visualize spots" is a whole-image high-contrast view, so it cannot be a row toggle;
it stays as a toggle button labelled **Find dust** under the sliders, with the Threshold slider
beside it while on. Same session field (`visualize`), same "view mode, never an edit" rule.

## Follow-up, same day

The user asked for the same hold-out on the Masks list, beside its eye; the masks spec's item 9
records it. With it, hold-outs of both kinds are released on a tool change (`select_tool`), as
Visualize is.

## Files

- `develop/panel/retouch.rs` (rewrite), `develop/panel/mask_components.rs` (the row gains a
  glyph enum, a meta text and a second toggle), `develop/panel/mask_glyph.rs` (spot glyphs),
  `develop/canvas/spot_overlay.rs` and `canvas/mod.rs` (outline per patch), `session_flow.rs`
  (`view_edits`, the sync/toggle helpers), `main.rs` + `test_app.rs` (fields), `tool.rs`
- `docs/JOURNAL.md`, `CLAUDE.md` state

## Testing

- `session_flow`: held patches leave the view along with the drag-suppressed one; the flags
  follow the count.
- `retouch`: the reset clears every patch and the selection; a row click selects; Next patch
  deselects; the eye and hold-out flip their flags and edit nothing; Show all / Hide all;
  deleting a patch drops its flags with it; the existing slider, suppression, Find dust and
  Threshold tests re-targeted.
- `spot_overlay`: a hidden patch draws no outline.
- `tool`: the unheaded tools are Edit, Crop, Masks, Retouch.
- Gate as usual; GUI in release.
