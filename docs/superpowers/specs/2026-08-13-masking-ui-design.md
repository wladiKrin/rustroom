# M3.1a plan B — the canvas UI for masks

**Status:** spec, 2026-08-13.

## Scope

The authoring half of M3.1a. The headless half landed already: a linear-gradient mask carrying
per-pixel adjustments renders correctly, proved against goldens and a real `.RAF` rendered from
a hand-written sidecar. This spec adds the way to author one without editing JSON.

⚠ **M3.1 does not close here.** 3.1b adds radial, 3.1c adds brush, and the milestone's own
"Done when" names dodge/burn with a brush.

## Decisions taken

| Question | Decision |
|---|---|
| Mask editing vs Geometry mode | **No new mode.** Handles appear when a mask is selected; `geometry_mode` wins while it is on. |
| Coverage display | **Three guide lines**, not a translucent tint. |
| Where selection state lives | **`Session`**, not `Edits`. |

### Why no third mode

The mask list *is* the mode indicator: select a mask and its handles appear, deselect and they
go. That matches how the same tool behaves elsewhere and adds nothing to explain.

A third exclusive toggle beside Geometry was rejected on two counts: it would mean entering a
mode to adjust a mask already visible on screen, and it would grow the toolbar — which is
precisely the change parked until after M3.1. Hanging the behaviour off the Masks panel
section's expanded state was rejected too: a collapsible section is for tidying a long panel,
not for changing what the canvas does, and overloading one is hidden coupling.

⚠ The cost is that two things can claim a drag, so the precedence rule must be **explicit**:

```rust
if s.geometry_mode {
    // crop overlay owns the pointer
} else if let Some(i) = s.selected_mask {
    // the selected mask owns it
}
```

### Why guide lines rather than a tint

The spec for the headless half proposed a translucent coverage tint. Reconsidered:

1. **It obscures the photograph you are judging** — the adjustment's effect is the thing to
   look at, and a wash over it competes.
2. **Lines are exact; a tint would be approximated.** Coverage is clamped at both ends, so a
   faithful tint needs three quads — flat, ramp, flat — not one interpolated gradient.
   Guide lines at `t = 0` and `t = 1` state the same information exactly.
3. It is what a graduated filter conventionally shows.

A tint behind a toggle remains available later; this is a deferral, not a rejection.

## Design

### 1. `app::mask_tool` — pure, mirroring `crop_tool`

```rust
/// What a drag is moving.
pub enum Handle {
    From,
    To,
    /// The whole gradient — both endpoints move together.
    Line,
}

/// Which handle is under `p` (normalised frame coordinates), within `tol`.
///
/// ⚠ `aspect` is required for the same reason `LinearGradient::coverage` needs it: distance
/// from the pointer to the axis must be measured in **screen** space. Without it, grabbing the
/// line is off on any non-square image, and worse near the ends.
pub fn hit_test(g: &LinearGradient, p: [f32; 2], aspect: f32, tol: f32) -> Option<Handle>;

/// Apply a drag delta in normalised units.
pub fn apply_drag(g: LinearGradient, h: Handle, dx: f32, dy: f32) -> LinearGradient;
```

Endpoints are tested before the line, so grabbing near an end moves that end rather than the
whole gradient.

This mirrors `crop_tool` deliberately: one pattern for canvas dragging, and the pure half is
testable without egui — which is where the arithmetic that actually goes wrong lives.

### 2. Selection state on `Session`

```rust
/// Which mask the canvas is editing, if any. Index into `edits.masks`.
selected_mask: Option<usize>,
```

⚠ **View state, not an edit.** It must not reach the sidecar, transfer on a paste, or appear in
a preset — `Edits` is the file format, and a selection is not part of the photograph's
description. `Session` is per-photo, so it resets on a photo switch for free.

⚠ It must be **clamped when a mask is deleted**, or it indexes past the end. The same class of
bug `Selection::clamp` already handles for the Library.

### 3. Canvas — `draw_mask_overlay`

Mirrors `draw_crop_overlay` in structure: paint, then handle the drag through
`drag_started` / `dragged` / `drag_stopped`, returning whether a history entry was recorded.

Painted:

- the **axis line** from `from` to `to`,
- a **boundary line** through each endpoint, perpendicular to the axis *in screen space*,
- a filled dot at each endpoint.

⚠ The perpendicular is computed in screen space, not normalised space — the same trap as
`coverage` and `hit_test`, showing up a third time. On a 3:2 frame a normalised-space
perpendicular is visibly not perpendicular.

The drag pattern is the established one:

```rust
if resp.drag_started() { s.active_mask_handle = mask_tool::hit_test(..); }
if resp.dragged() { /* apply the delta in normalised units */ }
if resp.drag_stopped() { s.active_mask_handle = None; s.history.push(..); return true; }
```

### 4. Panel — a Masks section

At the **bottom** of the right adjustments panel, after the existing sections:

- **"Add graduated filter"** — appends a `MaskedAdjustment` with a sensible default gradient
  (top to bottom across the middle third) and selects it.
- **The list** — one row per mask, showing its index and a summary of what it carries; clicking
  selects, a delete button removes.
- **The selected mask's sliders** — the nine `LocalAdjustments` fields, using the same
  `changed` / `drag_stopped` closure the global panel already uses.

⚠ **Placement is provisional**, as the headless spec required. This is the concept that caused
the layout work to be parked; it goes somewhere plausible now and gets its real home in the
layout pass after M3.1.

### 5. History

One entry per gesture, through the existing `photo_edited` return-value path so that one
gesture stays one Ctrl+Z:

- a settled endpoint or line drag — on `drag_stopped`,
- a settled slider — on `drag_stopped`, as every other slider does,
- adding or deleting a mask — immediately, since neither is a drag.

## Testing

- **`mask_tool` is pure and gets real tests**: an endpoint is grabbed in preference to the
  line; the line is grabbed only within tolerance; ⚠ **hit-testing the line is aspect-correct**
  — the same non-square property that `coverage` needed, watched failing without it; dragging
  `Line` moves both endpoints by the same delta; dragging `From` leaves `To` alone.
- **Selection clamping**: deleting the selected mask, and deleting one before it, both leave
  `selected_mask` valid or `None`.
- The overlay and the panel are egui glue, verified by running the app.

## Accepted caveats

1. **Linear gradients only** — the list can hold one kind of mask until 3.1b.
2. **No coverage tint**, by the reasoning above; deferred behind a future toggle.
3. **The panel's placement is provisional.**
4. **No mask reordering or renaming.** Masks are identified by position; both are obvious
   follow-ups and neither is needed to author a gradient.
5. **No keyboard nudging of handles.** Mouse only, as the crop tool is today.
6. **M3.1 does not close here** — 3.1b radial and 3.1c brush remain.
