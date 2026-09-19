# Develop panel: order of controls, header chrome, wheel rim — design

**Date:** 2026-09-19. **Branch:** `develop-panel-restyle` (a correction to the restyle that is
unmerged on it; not its own branch). **Status:** approved in conversation, 2026-09-19.

Three corrections the user asked for after looking at the restyled panel in use:

1. the colour wheel's rim is visibly stair-stepped;
2. the per-section `reset` sits in the same corner as the chevron and the two read as one smudge;
3. the order of controls in the Edit panel puts a rendering decision (the "Camera rendering"
   checkbox) on top of the Tone sliders, and more generally was never re-thought as a whole.

## 1. The wheel rim

`color_wheel::show` paints a triangle fan with a per-vertex colour. An `egui::Mesh` gets no
anti-aliasing, so the outer edge is a hard 96-gon.

- Add a **feather ring**: a second ring of vertices one physical pixel outside the rim
  (`1.0 / pixels_per_point` points), coloured `Color32::TRANSPARENT`, joined to the rim by two
  triangles per segment. This is egui's own tessellator technique; premultiplied alpha makes the
  transparent vertex interpolate correctly.
- `SEGMENTS` 96 → **180**, so the arc between two vertices is ~3.3 px at the panel's 188 px.
- Nothing else about the wheel changes: hub colour, rim chroma, handle, hit-testing, the strip.

**Test:** capture the frame's shapes, find the wheel mesh, and assert that its outermost vertices
are fully transparent and lie outside the vertices at the rim, which are opaque. Removing the ring
fails it.

## 2. The section header

Today: `NAME •  …  reset ›`, both actions at the right edge.

New layout, Lightroom's: **`› NAME •  …  reset`**.

- The chevron moves to the **left** of the name, 14 pt wide, rotated from `›` to `⌄` by the
  section's openness as before.
- The modified dot stays 8 pt after the name.
- `reset` sits **alone at the right edge**, weak text, strong on hover, with its tooltip. It is
  still registered after the row so it wins the hit-test tie.
- The header is extracted into its own bin module, `develop/panel/section.rs`, as
  `Header { name, modified, resettable }` and `header(ui, id, default_open, Header) ->
  (CollapsingState, bool)` where the bool is "reset was clicked". `adjustments::section` becomes a
  thin wrapper that computes `modified`, calls `header`, shows the body, and applies the reset.
  The Profile block (§3) is the second caller and the reason for the extraction; it also takes
  ~90 lines out of `adjustments.rs`, which is over the 600-line marker.

Accessible names are unchanged: the row is a `CollapsingHeader` named after the group, the
reset a `Button` named "reset". The existing tests keep passing.

## 3. The order of controls in Edit

### Today

```
Histogram · "Edit" heading · Camera profile (RAW only)
WHITE BALANCE   Temp, Tint, [Pick a neutral], [Presets ▾]
TONE            ☐ Camera rendering, Look ▾, [Auto], Exposure … Blacks,
                Presence (Texture, Clarity, Dehaze), Tone Curve
COLOR           Vibrance, Saturation, HSL, Color Grading (range, wheel, H/S/L),
                Blending, Balance, ☐ Show range
DETAIL / LENS / VIGNETTE
[Reset adjustments]
```

### Agreed

```
Histogram ……………………………………………………… [Reset adjustments]
› PROFILE       Camera profile ▾ (RAW only) · ☐ Camera rendering · Look ▾ (when any)
› WHITE BALANCE Temp, Tint · one row: [Pick a neutral] [Presets ▾]
› TONE          [Auto] · Exposure, Contrast, Highlights, Shadows, Whites, Blacks
                Tone Curve
                Presence: Texture, Clarity, Dehaze
› COLOR         Vibrance, Saturation · HSL · Color Grading ……… ☐ Show range
                range, wheel, Hue/Sat/Lum · Blending, Balance
› DETAIL        unchanged
› LENS          unchanged
› VIGNETTE      unchanged
```

**The Profile block.** Three controls decide how the raw data becomes a picture: the camera
profile, the camera rendering, and a look. They are gathered under one header, `PROFILE`, above
White Balance, because they establish the starting picture and everything below adjusts it.

- It is **not** a `Section`: "Reset adjustments" derives its group set from `SECTIONS`, and a
  reset here would flatten the photo to a colorimetric render (`RENDERING_GROUP`'s doc). So the
  header is drawn with `resettable: false` and no dot; it collapses like the others and opens by
  default.
- It is drawn in `panel/mod.rs`, where the camera profile row already lives, because that row
  needs `&mut RustroomApp` for the settings and the re-decode while the rendering checkbox and
  the look need `&mut Session`. The body borrows `app` first, then `app.session`.
- The checkbox and the look move out of `adjustments::tone` into a new bin module
  `develop/panel/profile.rs` as `profile::rendering(ui, edits, looks, ed)`, with their three
  tests and the look-labels test. `tone` loses its `looks` parameter.
- For a JPEG the block shows only the checkbox (and a look if one is installed or named); the
  profile row already returns early for non-RAW files.

**White Balance.** The eyedropper toggle and the Presets menu share one row under the sliders.
When there is no eyedropper (a mask), the row holds Presets alone.

**Tone.** Auto stays its own row. The curve follows the six sliders directly, because it refines
the tone they set; Presence closes the section as a different kind of decision. The curve draws
its own sub-label.

**Color.** "Show range" moves onto the "Color Grading" sub-label row, right-aligned, because it
is a view toggle for exactly the range selector below it. `theme::sub_label` gains a variant
taking trailing content, `sub_label_with`, and `sub_label` becomes a call to it with nothing
trailing. The doubled separator before Blending is removed. The state derivation (checkbox +
selector → `grade_view`) is unchanged.

**Detail, Lens, Vignette.** Unchanged. NR before Sharpening is a documented divergence.

**Reset adjustments.** Moves from the bottom of the stack to a right-aligned row at the top of
it, under the histogram, as a small button, so it is reachable without scrolling past forty
sliders. (The Edit tool has no heading by design, so the row is its own.) Its group set and tooltip
are unchanged. The bottom button and its separator go.

## Out of scope

Auto in the Tone header; dropping the global reset; renaming Vignette to Effects; anything in the
Crop, Masks, Retouch or Presets panels.

## Files

- new `crates/app/src/develop/panel/section.rs` (header), `profile.rs` (rendering + look)
- `adjustments.rs`: `section` wrapper, `tone` order and signature, WB row, Color changes
- `panel/mod.rs`: the reset row, the Profile block, hoisted `Edited`
- `color_wheel.rs`: feather ring, `SEGMENTS`
- `theme.rs`: `sub_label_with`
- `edit-model/src/lib.rs`: `RENDERING_GROUP` doc names the Profile block
- `docs/JOURNAL.md`, `CLAUDE.md` state

## Testing

- wheel: the feather-ring shape test above
- header: the existing `a_modified_section_shows_its_reset…` and `resetting_a_section…` tests, now
  through `section::header`; a new test that the Profile header offers no reset
- profile: the three rendering tests and the look-labels test, moved
- tone: the Auto test unchanged apart from the signature
- panel: a test that "Reset adjustments" is on screen with a session open and resets exposure
- Gate: `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace`, each exit code read from its own command
