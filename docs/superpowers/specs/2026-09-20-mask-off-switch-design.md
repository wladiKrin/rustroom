# A switched-off mask stays off in the export — design

**Date:** 2026-09-20. **Branch:** `mask-disable` (stacked on `presets-panel`). **Status:** the
user's request after seeing the masks' hold-out toggle: a mask turned off by it must not appear
in the exported file either.

## The decision

The hold-out was view state (`Session::mask_held`, held out of the view only, released on a tool
change). "Not in the export" makes it an **edit**: it has to travel with the mask through the
sidecar, a paste, a sync, a preset, undo — and be seen by every render, the export's included.

- `MaskedAdjustment` gains `disabled: bool`, `#[serde(default, skip_serializing_if = Not::not)]`:
  an enabled mask writes the bytes it always did, so every sidecar on disk is unchanged, and a
  sidecar from before the field reads as enabled. `disabled` rather than `enabled` so the
  derived `Default` (false) is the applied state and no hand-written `Default` is needed.
- **The pipeline is the one gate**: a disabled mask inserts no tail stage, no prefix step for
  its noise reduction or dehaze, and counts nothing toward the region reach. It keeps its slot
  in the coverage tables, as an inactive mask already does. The export builds the same
  pipeline as the view, so the two cannot disagree.
- The panel's toggle beside the eye flips `disabled` and is one history entry ("Disable {name}"
  for a screen reader). The masks' eye is unchanged: the overlay only.
- `Session::mask_held`, `Session::suppressed_masks`, `held_masks`, the derivation block in
  `develop::body` (with its soft-first re-render) and `view_edits`'s mask removal all go: the
  pipeline does the work now. `release_hold_outs` releases patch hold-outs only — a switched-off
  mask survives a tool change, which is the point of it.
- The brush's "a stroke on a suppressed mask asks for no render" and the canvas's view-only
  gesture guard ask `disabled` on the selected mask instead.
- The patch hold-out stays view state, deliberately: the user asked about masks, and a patch is
  a repair rather than a look.

## Files

- `edit-model/src/lib.rs` (the field; 63 struct literals across the workspace gain
  `disabled: false`), `pipeline/src/lib.rs` (three gates), `app`: `main.rs`, `test_app.rs`,
  `session_flow.rs`, `develop/mod.rs`, `develop/canvas/{mod,mask_overlay}.rs`,
  `develop/panel/{masks,mask_components,retouch}.rs`, `interaction.rs`
- `docs/JOURNAL.md`, `CLAUDE.md`

## Testing

- edit-model: a switched-off mask round-trips; an enabled one serialises without the field; a
  pre-field sidecar reads as enabled.
- pipeline: a switched-off mask builds the same pipeline as no mask, against a fixture with a
  tail adjustment and a prefix one.
- app: the toggle is an edit both ways; an off mask survives a tool change while a patch
  hold-out is released; a stroke on an off mask asks for no render.
- Goldens unmoved (every fixture mask is enabled).
