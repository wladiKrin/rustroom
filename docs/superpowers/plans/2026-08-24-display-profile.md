# Display profile for the preview — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stop the preview oversaturating on a wide-gamut display, by converting the rendered frame into the display's primaries on its way to the GPU.

**Architecture:** `color` gains an sRGB→display transform. It is applied in `render_engine::pack`, **after** the pipeline and after nothing else — never as a pipeline stage, because the histogram is computed from the packed bytes and must stay image-referred. Thumbnails get the same treatment through an 8-bit helper. A `Settings` field drives it.

**Tech Stack:** Rust, existing `color` crate, egui/eframe.

**Spec:** `docs/superpowers/specs/2026-08-24-display-profile-design.md`

**Before anything:** every build command needs

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

Branch per feature; **never push**.

---

## File Structure

| file | responsibility |
|---|---|
| `crates/color/src/spaces.rs` | *modify* — `SRGB_TO_DISPLAY_P3`, `SRGB_TO_ADOBE_RGB` |
| `crates/color/src/output_space.rs` | *modify* — `DISPLAYS`, `from_srgb`, `convert_srgb8` |
| `crates/app/src/render_engine.rs` | *modify* — `FrameSpec`; the transform inside `pack` |
| `crates/app/src/decode_worker.rs` | *modify* — baseline render call site |
| `crates/app/src/settings.rs` | *modify* — `display_space` field |
| `crates/app/src/thumb_loader.rs` | *modify* — convert thumbnails at load |
| `crates/app/src/modals/settings.rs` | *modify* — the dropdown |
| `crates/app/src/main.rs` | *modify* — pass the setting; re-render and reload on change |

⚠ **`pipeline` is not in this table and must not enter it.** Putting the transform in the pipeline
is the one wrong turn this design exists to avoid — see Task 2's note.

---

### Task 1: `color` gains the sRGB → display transform ✅ landed as `34e851f`

**Files:** `crates/color/src/spaces.rs`, `crates/color/src/output_space.rs`

- [x] **Step 1: Add `SRGB_TO_DISPLAY_P3` and `SRGB_TO_ADOBE_RGB`**, derived from primaries with no
      chromatic adaptation (all three spaces are D65).

```rust
pub const SRGB_TO_DISPLAY_P3: [[f32; 3]; 3] = [
    [0.8224619687, 0.1775380313, 0.0000000000],
    [0.0331941989, 0.9668058011, 0.0000000000],
    [0.0170826307, 0.0723974407, 0.9105199286],
];

pub const SRGB_TO_ADOBE_RGB: [[f32; 3]; 3] = [
    [0.7151256069, 0.2848743931, 0.0000000000],
    [0.0000000000, 1.0000000000, 0.0000000000],
    [0.0000000000, 0.0411619485, 0.9588380515],
];
```

- [x] **Step 2: Add `OutputSpace::DISPLAYS`, `from_srgb`, `convert_srgb8`.** `DISPLAYS` excludes
      ProPhoto; `from_srgb` is the identity for `Srgb`; `convert_srgb8` is for thumbnails only.

- [x] **Step 3: Tests, then sabotage each one.**

⚠⚠ **The finding from this task, and it changed the test set.** A **row swap** in
`SRGB_TO_DISPLAY_P3` passed every test originally written — both rows sum to 1.0, so white still
maps to white, and "red needs less than full red" is still true of the wrong row. A *within-row*
swap then survived even a per-channel dominance check. The test that catches both is
**`the_display_transform_preserves_luminance`**: Y is physical, so expressing a colour in different
primaries cannot change it. Any permutation moves Y.

Sabotages verified to fail: sRGB routed through the P3 matrix; P3 rows swapped; a within-row swap;
Adobe green row `0.9`; `DISPLAYS` containing ProPhoto; `convert_srgb8` skipping the sRGB decode;
the transform inverted.

---

### Task 2: apply it when packing a frame ✅ landed as `c944998`

**Files:** `crates/app/src/render_engine.rs`, `crates/app/src/decode_worker.rs`, `crates/app/src/main.rs`, `crates/app/examples/bench_preview.rs`

- [x] **Step 1: Convert inside `pack`, in float, before quantizing.**

⚠ In float, not on the packed bytes: an 8-bit → 8-bit conversion compresses the range and costs
roughly 0.8 bits, which shows as banding on a smooth sky.

⚠⚠ **In `pack`, not in the pipeline.** `shell.rs` computes the histogram from `frame.rgba`, so a
pipeline stage would silently make the histogram display-referred — a histogram that shifts when
you change monitors is useless as a clipping instrument.

- [x] **Step 2: `FrameSpec { max_dim, region, display_space }`.**

⚠ Threading the three individually pushed `render_cached` to 8 arguments and clippy's
`too_many_arguments` fired. That was a fair complaint, not a lint to silence. `render_cached`
destructures the spec on its first line, so both function bodies read exactly as before.

- [x] **Step 3: `the_display_space_changes_the_packed_pixels_and_srgb_does_not`**, sabotaged two
      ways (pack ignoring the space; the transform inverted) and watched to fail both times.

⚠ The test asserts P3 red is **below** sRGB red, not merely different — an inverted transform also
differs, and would reproduce the very bug this fixes while passing an `assert_ne!`.

---

### Task 3: the setting, the dropdown, and the thumbnails

**Files:**
- Modify: `crates/app/src/settings.rs`
- Modify: `crates/app/src/thumb_loader.rs`
- Modify: `crates/app/src/modals/settings.rs`
- Modify: `crates/app/src/main.rs`

- [ ] **Step 1: Write the failing tests**

In `crates/app/src/settings.rs`'s test module:

```rust
#[test]
fn a_settings_file_from_before_the_display_space_still_loads() {
    // ⚠ `SettingsStore::load()` is infallible, so a field that fails to parse silently reverts
    // *every* setting. What this actually discriminates is the **value**: the default must be
    // sRGB, because a wide space arriving by default would change what every existing user sees
    // without them asking for it.
    let json = r#"{"confirm_preset_delete":true}"#;
    let s: Settings = serde_json::from_str(json).expect("older settings should load");
    assert_eq!(s.display_space, color::OutputSpace::Srgb);
}
```

In `crates/app/src/thumb_loader.rs`'s test module:

```rust
#[test]
fn a_thumbnail_is_converted_into_the_display_space() {
    // ⚠ Not optional polish: the filmstrip sits directly beside the canvas showing the same
    // photo, so converting only the canvas leaves a visibly more saturated strip next to a
    // corrected image — a mismatch more noticeable than the original error.
    let red = [255u8, 0, 0];
    let srgb = color::OutputSpace::Srgb.convert_srgb8(red);
    let p3 = color::OutputSpace::DisplayP3.convert_srgb8(red);
    assert_eq!(srgb, red, "sRGB must be the identity");
    assert!(p3[0] < red[0], "P3 red {} should be below sRGB red 255", p3[0]);
    assert!(p3[1] > 0 && p3[2] > 0, "P3 red needs some green and blue: {p3:?}");
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p app --lib settings thumb_loader`
Expected: FAIL — no field `display_space`.

- [ ] **Step 3: Add the field**

In `crates/app/src/settings.rs`, add to `Settings` (check whether the struct carries a
**container-level** `#[serde(default)]` first — if it does, do **not** add a field-level one, which
would be redundant; that exact redundancy was found and removed in piece B):

```rust
    /// The primaries of the display Rustroom is being viewed on.
    ///
    /// ⚠ Applied to the preview and to thumbnails only, never to an export, and never inside the
    /// pipeline — see `render_engine::pack`.
    pub display_space: color::OutputSpace,
```

and to its `Default`:

```rust
            display_space: color::OutputSpace::default(),
```

- [ ] **Step 4: Convert thumbnails at load**

`ThumbLoader`'s load function produces a `Thumbnail { width, height, rgba }`. Apply the space to
its RGBA in place, leaving alpha untouched:

```rust
/// Convert a thumbnail's pixels into the display's primaries.
///
/// ⚠ 8-bit → 8-bit here, unlike the canvas: a thumbnail is a camera JPEG with no float behind
/// it, so there is no precision to preserve.
fn convert_thumb(thumb: &mut Thumbnail, space: color::OutputSpace) {
    if space == color::OutputSpace::Srgb {
        return;
    }
    for px in thumb.rgba.chunks_exact_mut(4) {
        let out = space.convert_srgb8([px[0], px[1], px[2]]);
        px[..3].copy_from_slice(&out);
    }
}
```

⚠ Take the space as a parameter on `ThumbLoader::new` (captured like `DecodeWorker`'s decode fn),
not per request — the caller of a per-thumbnail request has no business knowing about it.

- [ ] **Step 5: Pass the setting from `main.rs`**

Every `FrameSpec` built in `main.rs` takes `display_space: self.settings.display_space` instead of
`color::OutputSpace::Srgb`. `FrameSpec::fit` stays sRGB and is used only by tests and reference
paths.

⚠ The before/after baseline in `decode_worker.rs` stays `Srgb` and must not change: it is never
drawn (`show_before` is an ordinary render), and survives only for its dimensions.

- [ ] **Step 6: The dropdown**

In `crates/app/src/modals/settings.rs`:

```rust
        ui.horizontal(|ui| {
            ui.label("Display");
            egui::ComboBox::from_id_salt("display_space_pick")
                .selected_text(app.settings.display_space.label())
                .show_ui(ui, |ui| {
                    for s in color::OutputSpace::DISPLAYS {
                        ui.selectable_value(&mut app.settings.display_space, s, s.label());
                    }
                });
        });
        theme::hint(
            ui,
            "The primaries of your monitor. Most monitors are sRGB; recent Apple displays \
             and many laptops are Display P3. Affects only what you see, never an export.",
        );
```

- [ ] **Step 7: Re-render and reload thumbnails on a change**

⚠ **Guarded by a change comparison** — the Settings dialog body runs every frame, the rule
`apply_view_demosaic` already follows. On change: persist, rebuild the `ThumbLoader` with the new
space, clear loaded thumbnails so they reload, and request a fresh render of the open photo.

- [ ] **Step 8: Run the tests to verify they pass**

Run: `cargo test --all`
Expected: PASS. ⚠ **All 39 goldens byte-identical** — `pipeline` was not touched, and if a golden
moved something went into the pipeline that should not have.

- [ ] **Step 9: Sabotage**

| sabotage | must fail |
|---|---|
| `Default` gives `DisplayP3` | `a_settings_file_from_before_the_display_space_still_loads` |
| `convert_thumb` returns early always | `a_thumbnail_is_converted_into_the_display_space` |

Confirm each compiles before believing the result.

- [ ] **Step 10: Commit**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
git add -A
git commit -m "feat(app): a display-space setting for the preview and thumbnails"
```

⚠ Check clippy's **exit code**, not grep output. A `cargo clippy … | grep` pipeline reports grep's
status, which let a failing clippy through once in this feature.

---

### Task 4: verify in the app, then update the docs

- [ ] **Step 1: Full green check**

```bash
cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
```

Expected: PASS, 39 goldens byte-identical.

- [ ] **Step 2: Launch the GUI and look**

```bash
cargo run --release -p app
```

1. Settings shows a **Display** dropdown with three entries and no ProPhoto.
2. Switching to **Display P3** visibly desaturates the canvas — on a P3 panel this is the
   correction; strongly saturated reds and greens move most.
3. The filmstrip thumbnails move **with** the canvas, not against it.
4. ⚠ **The histogram does not move at all.** That is the property the whole design protects; if it
   shifts, the transform has leaked into the pipeline.
5. Export is unaffected — export a JPEG and confirm it is byte-identical to one exported with the
   setting on sRGB.

- [ ] **Step 3: Update `CLAUDE.md`**

A section in the house style: what landed, the ⚠ findings (the transform being a post-step and
why; the luminance test and the two permutations that survived weaker tests; `FrameSpec` and the
clippy limit; ProPhoto excluded from `DISPLAYS`), the test count, and the caveats from the spec —
no soft-proofing, no calibration, no auto-detection. Update the **▶ NEXT ACTION** block: D is done,
remaining are **C** (camera input profiles), **M3.2** lens corrections, **M3.4** virtual copies,
plus the named macOS follow-up.

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: record the display profile for the preview"
```

⚠ **Do not push.**

---

## Self-Review

**Spec coverage:** matrices and `from_srgb` (Task 1) ✓; `DISPLAYS` excluding ProPhoto (Task 1) ✓;
transform in `pack`, in float, outside the pipeline (Task 2) ✓; histogram untouched (Task 2 note,
Task 4 check) ✓; thumbnails converted (Task 3) ✓; setting with sRGB default (Task 3) ✓; dropdown
(Task 3) ✓; change-guarded re-render and reload (Task 3) ✓; caveats recorded (Task 4) ✓; the macOS
follow-up named (Task 4) ✓.

**Type consistency:** `OutputSpace::{from_srgb, convert_srgb8, DISPLAYS, label}` are used with
those names throughout. `FrameSpec { max_dim, region, display_space }` and `FrameSpec::fit` match
their definition in Task 2 and their uses in Task 3.
