# Custom aspect ratios and an orientation swap — design

Date: 2026-09-08
Status: **built 2026-09-08.** Plan `2026-09-08-custom-aspect.md`.

**M5.5c**, the last outstanding sub-slice of **M5.5 — Crop and straighten usability**, and the
roadmap inconsistency `CLAUDE.md` raised on 2026-09-07: Phase 5 is marked ✅ in both documents
while this was not built. Closing it makes the marker true.

## What it is

Two additions to the Geometry panel's aspect row:

1. **A `Custom` chip** that locks the crop to a ratio you type — two whole-number fields, W : H —
   so 5:4, 21:9, 65:24 and 2.35:1 (as 235:100) are all reachable. The last ratio entered persists
   in Settings.
2. **A `Swap` button** that turns the crop between landscape and portrait: the rectangle is
   transposed **in place** about its own centre, and a lock, if one is set, is inverted.

Nothing enters `Edits` or the sidecar. The lock and the custom ratio stay tool state, exactly as
the lock is today.

**Decisions taken with the user (2026-09-08):** swap transposes in place rather than re-snapping;
**button only**, no keyboard shortcut; **one** remembered custom ratio, not a list.

## What already exists

`develop/panel/geometry.rs` draws five `selectable_label` chips — Free, 1:1, 3:2, 4:3, 16:9 —
writing a **pixel** ratio into `RustroomApp.crop_aspect: Option<f32>`, and a chip click snaps the
crop centred and maximal through `crop_tool::snap_to_aspect`. `crop_tool::apply_drag` already
honours the lock on every handle, with its drive-axis rule and centre anchoring (2026-08-24). The
canvas normalises the lock per photo (`crop_aspect / img_aspect`) from the baseline texture, the
same source the panel uses. **None of that changes.** This feature adds one way to *set* the
lock and one way to *turn* it.

## The design

### 1. `crop_tool::swap_orientation` — a pure function

```rust
/// The crop turned between landscape and portrait: its pixel width and height exchanged, its
/// centre kept, shrunk uniformly only if the turned rect would leave the frame.
pub fn swap_orientation(r: CropRect, img_aspect: f32) -> CropRect;
```

In normalised units a rect `w × h` on an image of aspect `A = img_w / img_h` has pixel size
`w·img_w × h·img_h`. Exchanging those gives normalised `w' = h / A`, `h' = w · A`. Then:

- **Fit:** `s = min(1, 1/w', 1/h')`; both sides scale by `s`, so the transposed **pixel ratio is
  preserved exactly** even when the rect had to shrink. That is what keeps a swapped rect
  consistent with the inverted lock in §3.
- **Place:** centre unchanged, then the rect is shifted (never resized) back inside `[0, 1]` on
  either axis it overhangs.
- **Guards:** a non-finite or non-positive `img_aspect`, or a zero-area rect, returns `r`
  unchanged — the `apply_drag` rule: a bad input must not produce NaN.

⚠ Deliberately *not* `snap_to_aspect(1/a)`. That discards where the crop was, and the point of
the swap is that your framing survives — the user's call, and Lightroom's behaviour. On a
full-frame crop the two agree, which is one of the tests.

### 2. `Custom` chip and its two fields

`Settings` gains:

```rust
/// The last custom crop ratio entered, as whole numbers W : H. ⚠ `u32`, for the two reasons
/// `library_cell` carries: `Settings` derives `Eq`, and a NaN is unrepresentable rather than
/// guarded. Read through `custom_aspect_ratio()`, which repairs a hand-edited zero.
pub custom_aspect: [u32; 2],   // default [5, 4]
```

with `Settings::custom_aspect_ratio(&self) -> f32` clamping each side to `1..=1000` before
dividing. No field-level `#[serde(default)]` — the container-level one fills it, the
`snapshot_strip_height` rule, and `a_settings_file_from_before_custom_aspect_still_loads` guards
it.

The chip row becomes **Free · 1:1 · 3:2 · 4:3 · 16:9 · Custom**, then the `Swap` button. Clicking
`Custom` locks to the stored ratio and snaps the crop, exactly as a preset chip does. **While
`Custom` is lit, a second row appears**: `W` `DragValue`(1..=1000) `:` `H` `DragValue`(1..=1000).
Editing either re-locks to the new ratio and re-snaps; a drag settles on release and a typed value
settles on commit, so either is one Ctrl+Z (`ed.changed` on `changed()`, `ed.settled` on
`drag_stopped()` or on a change with no drag in progress).

**Which chip is lit** is a pure function, `chip_state(preset: Option<f32>, lock: Option<f32>)`,
extracted from the inline match so it can be tested: a ratio chip is lit when the lock equals it
**or its inverse** (so 3:2 stays lit after a swap to 2:3, with `Swap` showing which way it faces);
`Free` when there is no lock; **`Custom` when the lock matches the stored ratio or its inverse and
no preset chip matched** — a custom 3:2 lights 3:2, not both.

The custom ratio persists the way `composition_guide` does from this same panel: `panel/mod.rs`
snapshots it before drawing and calls `save_settings` on a change, never per frame.

### 3. `Swap`

A text button, hover *"Turn the crop between landscape and portrait"*. On click:

- `s.edits.geometry.crop = crop_tool::swap_orientation(crop, img_aspect)`;
- `crop_aspect = crop_aspect.map(|a| 1.0 / a)` — a free crop stays free and merely turns;
- the custom fields exchange (`[w, h] → [h, w]`) **only when `Custom` is the lit chip**, so the
  numbers on screen agree with the lock; a preset chip keeps its label and stays lit by the rule
  in §2;
- `ed.discrete()` — one history entry, one Ctrl+Z, like the rotate buttons.

⚠ A text label rather than a glyph. `⬍`/`⬌` already mean *flip* on the buttons two rows up, and
a swap that looks like a flip is the wrong model; no other suitable glyph is proven renderable,
and probing one is not worth it for a word that says what it does.

### 4. Not in scope

A shortcut key (user's call — the menu model and shortcuts dialog stay untouched); a list of
recent custom ratios; decimal ratios (whole numbers cover them, 235:100); the `Free` chip's
behaviour, the presets, and `apply_drag`.

## Testing

- **`crop_tool`**, pure: on a square image a transpose is an exact exchange of width and height;
  on a 3:2 image a full-frame crop turns into a centred 2:3 that fits, i.e. equals
  `snap_to_aspect(1/A)`; an off-centre rect keeps its centre; swapping twice is the identity to
  1e-6 when both fit; a rect that would overhang shrinks **uniformly** (its transposed pixel ratio
  is preserved) and lands inside the frame; a rect near an edge is shifted, not resized; a
  non-finite aspect and a zero-area rect come back unchanged.
- **`chip_state`**, pure: each of the five rules above, including the "custom equal to a preset
  lights the preset" tie-break.
- **`Settings`**: an older file loads with `[5, 4]`; a hand-edited `[0, 7]` repairs rather than
  divides by zero.
- **Panel, headless** (the `click_in_panel` harness already there): `Custom` and `Swap` are drawn;
  clicking `Swap` under a 3:2 lock leaves the lock at 2:3 and the crop at the transposed rect;
  clicking `Swap` on a free crop leaves the lock `None` and transposes; clicking `Custom` locks
  to the stored ratio and snaps; the W/H fields are drawn only while `Custom` is lit; a swap
  under `Custom` exchanges the stored pair. ⚠ Every click test asserts `ed.settled`, the
  wiring that would be forgotten.
- **No golden moves** and none is added — no pipeline code is touched; the crop rect is data the
  goldens already exercise.

## Files

| file | change |
|---|---|
| `crates/app/src/crop_tool.rs` | `swap_orientation` + tests (414 lines today) |
| `crates/app/src/settings.rs` | `custom_aspect`, `custom_aspect_ratio()` + tests. ⚠ Already 1,859 lines; say so. |
| `crates/app/src/develop/panel/geometry.rs` | the chip, the fields, the button, `chip_state` + tests. ⚠ Already 984 lines; say so. |
| `crates/app/src/develop/panel/mod.rs` | pass `&mut app.settings.custom_aspect`; snapshot-and-save |
| roadmap, capabilities note, `CLAUDE.md` | M5.5 ✅, the inconsistency closed |
