# Custom aspect ratios and an orientation swap (M5.5c) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this
> plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A `Custom` aspect chip with two W : H fields, and a `Swap` button that turns the crop
between landscape and portrait in place, inverting the lock.

**Architecture:** One pure function, `crop_tool::swap_orientation`, does the geometry; one pure
function, `geometry::chip_state`, decides which chip is lit; `Settings.custom_aspect: [u32; 2]`
remembers the last ratio. The panel gains a chip, two `DragValue`s and a text button, all through
the existing `Edited` accounting. Nothing enters `Edits`, the sidecar, or the pipeline.

**Tech Stack:** Rust, egui 0.35 headless harness (`test_ui`, the `click_in_panel` helper already in
`geometry.rs`), serde.

Spec: `docs/superpowers/specs/2026-09-08-custom-aspect-design.md`.

⚠⚠ **Standing conventions, and they override the harness's own default:** commit messages carry
**no `Co-Authored-By` trailer**; **never `git push`**. Work on branch `m5.5c-custom-aspect`, which
already exists and holds the spec.

⚠ **Every `cargo` command below assumes this was run first in the shell:**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

⚠ Two files this plan edits are already over the ~600-line mark and must be **named in the
report**: `crates/app/src/settings.rs` (~1,860 lines) and `crates/app/src/develop/panel/geometry.rs`
(~985). Neither is split here.

---

## File map

| file | responsibility in this plan |
|---|---|
| `crates/app/src/crop_tool.rs` | `swap_orientation` + tests |
| `crates/app/src/settings.rs` | `custom_aspect`, `CUSTOM_ASPECT_RANGE`, `custom_aspect_ratio()` + tests |
| `crates/app/src/develop/panel/geometry.rs` | `chip_state`, the chip, the fields, the button + tests |
| `crates/app/src/develop/panel/mod.rs` | pass the setting in; snapshot-and-save |
| docs | roadmap M5.5 ✅, capabilities line, `CLAUDE.md` |

---

### Task 1: `crop_tool::swap_orientation`

**Files:**
- Modify: `crates/app/src/crop_tool.rs` — new function after `snap_to_aspect` (~line 244), tests at the end of `mod tests`

- [ ] **Step 1: Write the failing tests**

Append inside `mod tests`, after the last existing test:

```rust
    fn close(a: CropRect, b: CropRect) -> bool {
        (a.left - b.left).abs() < 1e-6
            && (a.top - b.top).abs() < 1e-6
            && (a.right - b.right).abs() < 1e-6
            && (a.bottom - b.bottom).abs() < 1e-6
    }

    /// Pixel width / pixel height of a rect on an image of aspect `img_aspect`.
    fn pixel_ratio(c: &CropRect, img_aspect: f32) -> f32 {
        ratio(c) * img_aspect
    }

    #[test]
    fn on_a_square_image_a_swap_exchanges_width_and_height() {
        let out = swap_orientation(r(), 1.0);
        // `r()` is square, so the swap is the identity here — the discriminating case follows.
        assert!(close(out, r()), "{out:?}");
        let wide = CropRect {
            left: 0.1,
            top: 0.3,
            right: 0.7,
            bottom: 0.5,
        };
        let out = swap_orientation(wide, 1.0);
        assert!((out.right - out.left - 0.2).abs() < 1e-6, "{out:?}");
        assert!((out.bottom - out.top - 0.6).abs() < 1e-6, "{out:?}");
    }

    #[test]
    fn a_full_frame_crop_on_a_3_2_image_turns_into_the_centred_2_3() {
        // ⚠ The one case where "transpose in place" and "re-snap" agree, and it pins the
        // pixel-space meaning: a 3:2 frame turned is a 2:3 crop, not a 3:2 one drawn narrower.
        let a = 1.5;
        let out = swap_orientation(CropRect::default(), a);
        assert!(close(out, snap_to_aspect((1.0 / a) / a)), "{out:?}");
        assert!((pixel_ratio(&out, a) - 1.0 / a).abs() < 1e-5, "{out:?}");
    }

    #[test]
    fn an_off_centre_rect_keeps_its_centre() {
        let a = 1.5;
        let c = CropRect {
            left: 0.5,
            top: 0.2,
            right: 0.7,
            bottom: 0.4,
        };
        let out = swap_orientation(c, a);
        assert!(((out.left + out.right) - (c.left + c.right)).abs() < 1e-6, "{out:?}");
        assert!(((out.top + out.bottom) - (c.top + c.bottom)).abs() < 1e-6, "{out:?}");
        assert!((pixel_ratio(&out, a) * pixel_ratio(&c, a) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn swapping_twice_is_the_identity_when_both_fit() {
        let a = 1.5;
        // ⚠ Not a rect whose transposed shape equals its own — on a 3:2 image a 0.2 × 0.3
        // rect turns into a 0.2 × 0.3 rect, and the test would pass with the swap deleted.
        let c = CropRect {
            left: 0.4,
            top: 0.15,
            right: 0.6,
            bottom: 0.75,
        };
        let once = swap_orientation(c, a);
        assert!(!close(once, c), "fixture check: the swap must change this rect");
        let out = swap_orientation(once, a);
        assert!(close(out, c), "{out:?} vs {c:?}");
    }

    #[test]
    fn a_rect_that_would_overhang_shrinks_uniformly_and_fits() {
        // A wide 3:2 crop filling the width: turned, it is taller than the frame and must shrink
        // — keeping its transposed pixel ratio exactly, or it would no longer match a lock.
        let a = 1.5;
        let c = CropRect {
            left: 0.0,
            top: 0.2,
            right: 1.0,
            bottom: 0.8,
        };
        let out = swap_orientation(c, a);
        assert!(inside_frame(&out), "{out:?}");
        let want = pixel_ratio(&c, a).recip();
        assert!((pixel_ratio(&out, a) - want).abs() < 1e-5, "{out:?}");
        assert!(out.bottom - out.top > 0.99, "it should use the full height: {out:?}");
    }

    #[test]
    fn a_rect_near_an_edge_is_shifted_not_resized() {
        let a = 1.0;
        let c = CropRect {
            left: 0.7,
            top: 0.0,
            right: 1.0,
            bottom: 0.1,
        };
        let out = swap_orientation(c, a);
        assert!(inside_frame(&out), "{out:?}");
        assert!((out.right - out.left - 0.1).abs() < 1e-6, "{out:?}");
        assert!((out.bottom - out.top - 0.3).abs() < 1e-6, "{out:?}");
    }

    #[test]
    fn a_bad_aspect_or_an_empty_rect_comes_back_unchanged() {
        let empty = CropRect {
            left: 0.5,
            top: 0.2,
            right: 0.5,
            bottom: 0.4,
        };
        assert_eq!(swap_orientation(empty, 1.5), empty);
        for bad in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert_eq!(swap_orientation(r(), bad), r(), "aspect {bad}");
        }
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p app --lib crop_tool 2>&1 | grep -E "^error" | head -3`
Expected: `cannot find function swap_orientation`.

- [ ] **Step 3: Implement**

After `snap_to_aspect` (before `#[cfg(test)]`):

```rust
/// The crop turned between landscape and portrait: its pixel width and height exchanged, its
/// centre kept, shrunk **uniformly** only if the turned rect would leave the frame, then
/// shifted — never resized — back inside `[0, 1]`.
///
/// ⚠ In normalised units a `w × h` rect on an image of aspect `A` is `w·img_w × h·img_h`
/// pixels, so the transposed rect is `h / A` wide and `w · A` tall. The uniform shrink is what
/// keeps the transposed **pixel ratio** exact even when it had to fit, which is what keeps a
/// swapped crop consistent with the inverted lock the panel sets beside it.
///
/// ⚠ Deliberately not `snap_to_aspect(1/a)`: that discards where the crop was, and the point
/// of a swap is that the framing survives (spec §1). On a full-frame crop the two agree.
///
/// ⚠ A non-finite or non-positive `img_aspect`, or a zero-area rect, comes back unchanged —
/// `apply_drag`'s rule: a bad input must not produce NaN.
pub fn swap_orientation(r: CropRect, img_aspect: f32) -> CropRect {
    if !img_aspect.is_finite() || img_aspect <= 0.0 {
        return r;
    }
    let w = r.right - r.left;
    let h = r.bottom - r.top;
    if w <= 0.0 || h <= 0.0 {
        return r;
    }
    let cx = (r.left + r.right) / 2.0;
    let cy = (r.top + r.bottom) / 2.0;
    let mut nw = h / img_aspect;
    let mut nh = w * img_aspect;
    let s = (1.0 / nw).min(1.0 / nh).min(1.0);
    nw *= s;
    nh *= s;
    let mut left = cx - nw / 2.0;
    let mut top = cy - nh / 2.0;
    // Shift back inside the frame on either axis it overhangs; the size is already ≤ 1.
    left = left.clamp(0.0, 1.0 - nw);
    top = top.clamp(0.0, 1.0 - nh);
    CropRect {
        left,
        top,
        right: left + nw,
        bottom: top + nh,
    }
}
```

- [ ] **Step 4: Run**

Run: `cargo test -p app --lib crop_tool 2>&1 | grep -E "^test result|panicked"`
Expected: `test result: ok.` with 7 more tests than before.

- [ ] **Step 5: Commit**

```bash
git add crates/app/src/crop_tool.rs
git commit -m "crop_tool: swap_orientation, the crop turned in place"
```

---

### Task 2: `Settings.custom_aspect`

**Files:**
- Modify: `crates/app/src/settings.rs` — field after `survey_cell` (~line 283), constant beside `CELL_RANGE` (~line 389), method in `impl Settings` after `clamped_cell` (~line 421), `Default` literal (~line 574), the exhaustive test literal (~line 1074), tests after `a_settings_file_from_before_view_demosaic_still_loads`

- [ ] **Step 1: Write the failing tests**

After `a_settings_file_from_before_view_demosaic_still_loads` in `mod tests`:

```rust
    #[test]
    fn a_settings_file_from_before_custom_aspect_still_loads() {
        // ⚠ `load()` is infallible, so a field that fails to parse silently reverts *every*
        // setting. The container-level `#[serde(default)]` is what fills the new field.
        let f = TempFile::new("custom_aspect_old");
        std::fs::write(&f.0, r#"{"confirm_preset_delete": false}"#).unwrap();
        let loaded = SettingsStore::new(f.0.clone()).load();
        assert!(!loaded.confirm_preset_delete, "the old fields must survive");
        assert_eq!(loaded.custom_aspect, DEFAULT_CUSTOM_ASPECT);
    }

    #[test]
    fn a_hand_edited_custom_aspect_is_repaired_rather_than_dividing_by_zero() {
        let mut s = Settings::default();
        s.custom_aspect = [0, 7];
        let r = s.custom_aspect_ratio();
        assert!(r.is_finite() && r > 0.0, "{r}");
        assert!((r - 1.0 / 7.0).abs() < 1e-6, "a zero repairs to the minimum: {r}");
        s.custom_aspect = [5, 4];
        assert!((s.custom_aspect_ratio() - 1.25).abs() < 1e-6);
        s.custom_aspect = [50_000, 1];
        assert!((s.custom_aspect_ratio() - 1000.0).abs() < 1e-6, "clamped at the top");
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p app --lib custom_aspect 2>&1 | grep -E "^error" | head -3`
Expected: `no field custom_aspect`.

- [ ] **Step 3: Add the field, the constants, the method**

Field, directly after `pub survey_cell: u32,`:

```rust
    /// The last custom crop ratio entered in the Geometry panel, as whole numbers W : H.
    ///
    /// ⚠ `u32`, for the two reasons `library_cell` carries: `Settings` derives `Eq`, which no
    /// float satisfies, and a NaN ratio is **unrepresentable** rather than guarded. Whole
    /// numbers cover the decimal cases — 2.35:1 is 235:100.
    ///
    /// ⚠ Read through [`Settings::custom_aspect_ratio`], which repairs a hand-edited zero:
    /// the file is user-editable and a zero divides by zero.
    ///
    /// ⚠ No field-level `#[serde(default)]` — the container-level one fills a missing field
    /// from this struct's `Default`, the `snapshot_strip_height` rule.
    pub custom_aspect: [u32; 2],
```

Constants, after `pub const CELL_RANGE: (u32, u32) = (108, 320);`:

```rust
/// The custom crop ratio's default, 5:4 — a print ratio none of the five preset chips offers.
pub const DEFAULT_CUSTOM_ASPECT: [u32; 2] = [5, 4];
/// Each side of a custom ratio, inclusive. 1000 lets 2.35:1 be written as 235:100.
pub const CUSTOM_ASPECT_RANGE: (u32, u32) = (1, 1000);
```

Method, after `clamped_cell`:

```rust
    /// The custom crop ratio as a pixel width / height, guaranteed finite and positive.
    pub fn custom_aspect_ratio(&self) -> f32 {
        let [w, h] = self.custom_aspect;
        let w = w.clamp(CUSTOM_ASPECT_RANGE.0, CUSTOM_ASPECT_RANGE.1) as f32;
        let h = h.clamp(CUSTOM_ASPECT_RANGE.0, CUSTOM_ASPECT_RANGE.1) as f32;
        w / h
    }
```

`Default` literal (~line 574) and the exhaustive test literal (~line 1074): add
`custom_aspect: DEFAULT_CUSTOM_ASPECT,` directly after `survey_cell: DEFAULT_SURVEY_CELL,` in
**both**. ⚠ The compiler names any third site; add it there too.

- [ ] **Step 4: Run**

Run: `cargo test -p app --lib settings 2>&1 | grep -E "^test result|panicked|^error"`
Expected: `test result: ok.`

- [ ] **Step 5: Commit**

```bash
git add crates/app/src/settings.rs
git commit -m "Settings: the last custom crop ratio"
```

---

### Task 3: `geometry::chip_state`, the pure lit-chip rule

**Files:**
- Modify: `crates/app/src/develop/panel/geometry.rs` — new function above `pub fn show`, tests in `mod tests`

- [ ] **Step 1: Write the failing tests**

In `mod tests`, after `flipping_twice_is_the_identity`:

```rust
    #[test]
    fn a_ratio_chip_is_lit_by_its_ratio_and_by_its_inverse() {
        // ⚠ The inverse too: after a swap the lock is 2:3 and the 3:2 chip must stay lit, with
        // Swap showing which way it faces. A chip that went dark on swap would read as "no
        // preset", which is false.
        assert!(chip_state(Chip::Preset(1.5), Some(1.5), 1.25));
        assert!(chip_state(Chip::Preset(1.5), Some(1.0 / 1.5), 1.25));
        assert!(!chip_state(Chip::Preset(1.5), Some(1.0), 1.25));
        assert!(!chip_state(Chip::Preset(1.5), None, 1.25));
    }

    #[test]
    fn free_is_lit_only_with_no_lock() {
        assert!(chip_state(Chip::Free, None, 1.25));
        assert!(!chip_state(Chip::Free, Some(1.0), 1.25));
    }

    #[test]
    fn custom_is_lit_by_the_stored_ratio_unless_a_preset_already_matches() {
        assert!(chip_state(Chip::Custom, Some(1.25), 1.25));
        assert!(chip_state(Chip::Custom, Some(0.8), 1.25), "its inverse too");
        assert!(!chip_state(Chip::Custom, Some(1.5), 1.25));
        assert!(!chip_state(Chip::Custom, None, 1.25));
        // ⚠ The tie-break: a custom ratio equal to a preset lights the preset, not both.
        assert!(!chip_state(Chip::Custom, Some(1.5), 1.5));
        assert!(chip_state(Chip::Preset(1.5), Some(1.5), 1.5));
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p app --bin Rustroom chip_state 2>&1 | grep -E "^error" | head -3`
Expected: `cannot find type Chip`.

- [ ] **Step 3: Implement**

Above `pub fn show`:

```rust
/// One entry of the aspect row.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Chip {
    Free,
    /// A preset, as a pixel ratio.
    Preset(f32),
    Custom,
}

/// The five preset chips, in the order they are drawn.
const PRESETS: [(&str, f32); 4] = [
    ("1:1", 1.0),
    ("3:2", 1.5),
    ("4:3", 4.0 / 3.0),
    ("16:9", 16.0 / 9.0),
];

fn same_ratio(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

/// Whether `chip` is lit for the current `lock`, with `custom` the stored custom ratio.
///
/// ⚠ A ratio chip is lit by its ratio **or its inverse**, so 3:2 stays lit after a swap to 2:3
/// and `Swap` shows which way it faces. `Custom` is lit by the stored ratio or its inverse
/// **only when no preset matched** — a custom 3:2 lights 3:2, not both. Pure, so the rule is
/// testable without a window.
fn chip_state(chip: Chip, lock: Option<f32>, custom: f32) -> bool {
    let matches = |ratio: f32| {
        lock.is_some_and(|cur| same_ratio(ratio, cur) || same_ratio(1.0 / ratio, cur))
    };
    match chip {
        Chip::Free => lock.is_none(),
        Chip::Preset(ratio) => matches(ratio),
        Chip::Custom => matches(custom) && !PRESETS.iter().any(|(_, p)| matches(*p)),
    }
}
```

⚠ `PRESETS` and `Chip` have no production caller until Task 4 and this is a **binary** crate, so
`clippy -D warnings` will flag them dead until then. **Do not commit Task 3 alone** — run its
tests, then continue straight into Task 4 and commit both together.

- [ ] **Step 4: Run**

Run: `cargo test -p app --bin Rustroom chip_state 2>&1 | grep -E "^test result|panicked"`
Expected: `test result: ok. 3 passed`.

---

### Task 4: The chip, the fields, the button, and the wiring

**Files:**
- Modify: `crates/app/src/develop/panel/geometry.rs` — `show`'s signature (line 14), the presets block (lines 87–116), every test call of `show` (7 sites) and `click_in_panel_reporting`, new tests
- Modify: `crates/app/src/develop/panel/mod.rs` — the `Tool::Crop` call (~line 220), the snapshot-and-save (~lines 108 and 291)

- [ ] **Step 1: Write the failing tests**

In `geometry.rs`'s `mod tests`, after the `chip_state` tests. The harness gains a custom-ratio
parameter so a test can seed it; `click_in_panel` keeps its signature by passing the default.

Replace `click_in_panel_reporting` with:

```rust
    /// The one harness, with the Auto click it reports outward.
    ///
    /// ⚠ `click_in_panel` is a thin wrapper over this rather than a second copy: a helper
    /// duplicated per test is how two harnesses come to disagree about what a click is.
    fn click_in_panel_reporting(label: &str, s: &mut Session, lens: bool) -> (Edited, bool) {
        let mut aspect = None;
        let mut custom = rustroom_app::settings::DEFAULT_CUSTOM_ASPECT;
        click_in_panel_with(label, s, lens, &mut aspect, &mut custom)
    }

    /// The harness with the lock and the stored custom ratio exposed, for the aspect tests.
    fn click_in_panel_with(
        label: &str,
        s: &mut Session,
        lens: bool,
        aspect: &mut Option<f32>,
        custom: &mut [u32; 2],
    ) -> (Edited, bool) {
        let ctx = test_ui::ctx();
        let mut ed = Edited::default();
        let mut auto = false;
        test_ui::click(&ctx, label, |ui| {
            ed = Edited::default();
            auto = false;
            show(
                ui,
                s,
                aspect,
                custom,
                &mut CompositionGuide::Off,
                lens,
                &mut ed,
                &mut auto,
            );
        });
        (ed, auto)
    }
```

Then the tests:

```rust
    /// The image the harness opens is 80×40, aspect 2.0 — every crop below is on that.
    const IMG_ASPECT: f32 = 2.0;

    #[test]
    fn custom_and_swap_are_offered() {
        let ctx = test_ui::ctx();
        let mut s = session();
        let mut aspect = None;
        let mut custom = rustroom_app::settings::DEFAULT_CUSTOM_ASPECT;
        let mut ed = Edited::default();
        let drawn: Vec<String> = test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
            show(
                ui,
                &mut s,
                &mut aspect,
                &mut custom,
                &mut CompositionGuide::Off,
                false,
                &mut ed,
                &mut false,
            );
        })
        .into_iter()
        .map(|(l, _)| l)
        .collect();
        for want in ["Custom", "Swap"] {
            assert!(drawn.iter().any(|l| l == want), "{want} missing: {drawn:?}");
        }
    }

    #[test]
    fn swap_under_a_lock_inverts_it_and_transposes_the_crop() {
        let mut s = session();
        s.edits.geometry.crop = asymmetric();
        let mut aspect = Some(1.5);
        let mut custom = rustroom_app::settings::DEFAULT_CUSTOM_ASPECT;
        let (ed, _) = click_in_panel_with("Swap", &mut s, false, &mut aspect, &mut custom);
        assert!(ed.settled, "a swap is one undo step");
        assert!((aspect.unwrap() - 1.0 / 1.5).abs() < 1e-6, "{aspect:?}");
        assert_crop_close(
            s.edits.geometry.crop,
            crate::crop_tool::swap_orientation(asymmetric(), IMG_ASPECT),
        );
        assert_eq!(custom, rustroom_app::settings::DEFAULT_CUSTOM_ASPECT, "not a custom chip");
    }

    #[test]
    fn swap_on_a_free_crop_only_transposes() {
        let mut s = session();
        s.edits.geometry.crop = asymmetric();
        let mut aspect = None;
        let mut custom = rustroom_app::settings::DEFAULT_CUSTOM_ASPECT;
        let (ed, _) = click_in_panel_with("Swap", &mut s, false, &mut aspect, &mut custom);
        assert!(ed.settled);
        assert_eq!(aspect, None, "a free crop stays free");
        assert_crop_close(
            s.edits.geometry.crop,
            crate::crop_tool::swap_orientation(asymmetric(), IMG_ASPECT),
        );
    }

    #[test]
    fn clicking_custom_locks_to_the_stored_ratio_and_snaps() {
        let mut s = session();
        s.edits.geometry.crop = asymmetric();
        let mut aspect = None;
        let mut custom = [21, 9];
        let (ed, _) = click_in_panel_with("Custom", &mut s, false, &mut aspect, &mut custom);
        assert!(ed.settled);
        assert!((aspect.unwrap() - 21.0 / 9.0).abs() < 1e-6, "{aspect:?}");
        assert_crop_close(
            s.edits.geometry.crop,
            crate::crop_tool::snap_to_aspect((21.0 / 9.0) / IMG_ASPECT),
        );
    }

    #[test]
    fn swap_under_custom_exchanges_the_stored_pair() {
        let mut s = session();
        let mut aspect = Some(21.0 / 9.0);
        let mut custom = [21, 9];
        click_in_panel_with("Swap", &mut s, false, &mut aspect, &mut custom);
        assert_eq!(custom, [9, 21], "the numbers on screen must agree with the lock");
        assert!((aspect.unwrap() - 9.0 / 21.0).abs() < 1e-6);
    }

    #[test]
    fn the_ratio_fields_are_drawn_only_while_custom_is_lit() {
        let ctx = test_ui::ctx();
        let drawn_with = |lock: Option<f32>| {
            let mut s = session();
            let mut aspect = lock;
            let mut custom = [21, 9];
            let mut ed = Edited::default();
            test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
                show(
                    ui,
                    &mut s,
                    &mut aspect,
                    &mut custom,
                    &mut CompositionGuide::Off,
                    false,
                    &mut ed,
                    &mut false,
                );
            })
            .into_iter()
            .map(|(l, _)| l)
            .collect::<Vec<_>>()
        };
        // ⚠ A `DragValue` is a `TextEdit` under the hood and reports its *value* as its
        // AccessKit name, so "21" and "9" are what the harness sees.
        let lit = drawn_with(Some(21.0 / 9.0));
        assert!(lit.iter().any(|l| l == "21"), "W field missing: {lit:?}");
        assert!(lit.iter().any(|l| l == "9"), "H field missing: {lit:?}");
        let dark = drawn_with(Some(1.5));
        assert!(!dark.iter().any(|l| l == "21"), "fields drawn under a preset: {dark:?}");
        let free = drawn_with(None);
        assert!(!free.iter().any(|l| l == "21"), "fields drawn under Free: {free:?}");
    }
```

⚠ If `the_ratio_fields_are_drawn_only_while_custom_is_lit` cannot find `"21"`, the AccessKit
name of a `DragValue` differs from its value in this egui version. Then assert on the *number of
interactive widgets* instead — lit must draw exactly two more than dark — and say so in the test's
comment rather than deleting it.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p app --bin Rustroom geometry 2>&1 | grep -E "^error" | head -3`
Expected: `show` takes 7 arguments but 8 were supplied.

- [ ] **Step 3: The signature and the row**

`show`'s signature gains, after `crop_aspect: &mut Option<f32>,`:

```rust
    /// The stored custom ratio, W : H — persisted by the caller on change.
    custom_aspect: &mut [u32; 2],
```

Replace the presets block (from `let img_aspect = …` through the closing `});` of the
`ui.horizontal` that draws the chips, lines ~87–116) with:

```rust
    let img_aspect = s.baseline.size_vec2().x / s.baseline.size_vec2().y;
    let custom_ratio = {
        let [w, h] = *custom_aspect;
        let r = rustroom_app::settings::CUSTOM_ASPECT_RANGE;
        w.clamp(r.0, r.1) as f32 / h.clamp(r.0, r.1) as f32
    };
    // ⚠ `selectable_label`, not `button`: the lock is otherwise **invisible state**. Without a
    // shown selection there is no way to tell a locked crop from a free one until you drag a
    // handle and it refuses to move the way you expected.
    //
    // ⚠ Which chip is lit is `chip_state`'s decision, pure and tested, including the rule that
    // a preset stays lit after a swap to its inverse.
    ui.horizontal(|ui| {
        if ui
            .selectable_label(chip_state(Chip::Free, *crop_aspect, custom_ratio), "Free")
            .clicked()
        {
            // ⚠ "Free" merely unlocks, leaving the crop where it is — it used to reset to the
            // full frame, which made unlocking destructive and is now "Reset geometry"'s job.
            *crop_aspect = None;
        }
        for (label, ratio) in PRESETS {
            if ui
                .selectable_label(chip_state(Chip::Preset(ratio), *crop_aspect, custom_ratio), label)
                .clicked()
            {
                lock_to(ratio, img_aspect, s, crop_aspect, ed);
            }
        }
        let custom_lit = chip_state(Chip::Custom, *crop_aspect, custom_ratio);
        if ui
            .selectable_label(custom_lit, "Custom")
            .on_hover_text("Lock to the ratio below")
            .clicked()
        {
            lock_to(custom_ratio, img_aspect, s, crop_aspect, ed);
        }
        // ⚠ A text label, not a glyph: `⬍`/`⬌` already mean *flip* two rows up, and a swap
        // that looks like a flip is the wrong model.
        if ui
            .button("Swap")
            .on_hover_text("Turn the crop between landscape and portrait")
            .clicked()
        {
            s.edits.geometry.crop =
                crop_tool::swap_orientation(s.edits.geometry.crop, img_aspect);
            // ⚠ A free crop stays free and merely turns; a lock inverts. The custom pair is
            // exchanged only when Custom is the lit chip, so the numbers on screen agree with
            // the lock — under a preset the chip keeps its label and stays lit by `chip_state`.
            *crop_aspect = crop_aspect.map(|a| 1.0 / a);
            if custom_lit {
                let [w, h] = *custom_aspect;
                *custom_aspect = [h, w];
            }
            ed.discrete();
        }
    });
    if chip_state(Chip::Custom, *crop_aspect, custom_ratio) {
        // ⚠ Drawn only while Custom is lit: two number fields under a preset would be state
        // that edits nothing, which is worse than absent.
        ui.horizontal(|ui| {
            let r = rustroom_app::settings::CUSTOM_ASPECT_RANGE;
            let mut field = |ui: &mut egui::Ui, v: &mut u32| -> egui::Response {
                ui.add(egui::DragValue::new(v).range(r.0..=r.1))
            };
            let [w, h] = custom_aspect;
            let rw = field(ui, w);
            ui.label(":");
            let rh = field(ui, h);
            // ⚠ Re-lock on any change so the crop follows as you drag or type. A drag settles
            // on release and a typed value settles on commit — `changed()` with no drag in
            // progress — so either way one edit is one Ctrl+Z. `ed.absorb` alone would never
            // settle a typed value, since a `TextEdit` commit stops no drag.
            let changed = rw.changed() || rh.changed();
            if changed {
                let ratio = {
                    let [w, h] = *custom_aspect;
                    w.clamp(r.0, r.1) as f32 / h.clamp(r.0, r.1) as f32
                };
                *crop_aspect = Some(ratio);
                s.edits.geometry.crop = crop_tool::snap_to_aspect(ratio / img_aspect);
                ed.changed = true;
            }
            let dragging = rw.dragged() || rh.dragged();
            if rw.drag_stopped() || rh.drag_stopped() || (changed && !dragging) {
                ed.settled = true;
            }
        });
    }
```

`lock_to` is a free function placed above `show`, **not** a closure — a closure capturing `ed`
mutably while the `Swap` arm in the same `ui.horizontal` body also uses `ed` is the `E0499` this
panel has already paid for twice:

```rust
/// Lock to `ratio` (a pixel ratio) and snap the crop to it, centred and maximal — what a
/// preset chip and the Custom chip both do.
fn lock_to(
    ratio: f32,
    img_aspect: f32,
    s: &mut Session,
    crop_aspect: &mut Option<f32>,
    ed: &mut Edited,
) {
    *crop_aspect = Some(ratio);
    s.edits.geometry.crop = crop_tool::snap_to_aspect(ratio / img_aspect);
    ed.discrete();
}
```

- [ ] **Step 4: The test call sites**

Every remaining `show(` call in `mod tests` that passes `&mut aspect,` gains a line
`&mut rustroom_app::settings::DEFAULT_CUSTOM_ASPECT.clone(),` directly after it — or, cleaner, a
local `let mut custom = rustroom_app::settings::DEFAULT_CUSTOM_ASPECT;` above and `&mut custom,`.
Run `cargo test -p app --bin Rustroom geometry 2>&1 | grep -E "^error" -A 3` to find each.

- [ ] **Step 5: The wiring in `panel/mod.rs`**

The `Tool::Crop` arm becomes:

```rust
                        Tool::Crop => {
                            let found = s.lens.shot.is_some();
                            geometry::show(
                                ui,
                                s,
                                &mut app.crop_aspect,
                                &mut app.settings.custom_aspect,
                                &mut app.settings.composition_guide,
                                found,
                                &mut ed,
                                &mut auto,
                            )
                        }
```

Beside `let guide_before = app.settings.composition_guide;` add:

```rust
    let custom_before = app.settings.custom_aspect;
```

and the save guard becomes:

```rust
    // ⚠ Guarded on an actual change, and outside the closure — see `guide_before`. The custom
    // crop ratio is the second setting this panel edits in place.
    if app.settings.composition_guide != guide_before
        || app.settings.custom_aspect != custom_before
    {
        app.save_settings();
    }
```

- [ ] **Step 6: Run**

Run: `cargo test -p app --bin Rustroom 2>&1 | grep -E "^test result|panicked|^error" | head`
Expected: `test result: ok.`; the 12 pre-existing geometry tests unchanged.

- [ ] **Step 7: Sabotage check**

Change `*crop_aspect = crop_aspect.map(|a| 1.0 / a);` to `let _ = crop_aspect;` and run
`cargo test -p app --bin Rustroom swap_under 2>&1 | grep -E "^test result|panicked"`: expected
two failures naming the lock. Revert. Change `chip_state`'s inverse check to only the direct
ratio and run `a_ratio_chip_is_lit`: expected FAIL. Revert.

- [ ] **Step 8: Commit (Tasks 3 and 4 together)**

```bash
git add crates/app/src/develop/panel/geometry.rs crates/app/src/develop/panel/mod.rs
git commit -m "Geometry panel: a Custom aspect chip and a Swap button"
```

---

### Task 5: Docs, gates, and the GUI

**Files:**
- Modify: `~/Documents/obsidian/private/opensource_lightroom/03-plan-of-action.md` (M5.5 heading ~line 378)
- Modify: `~/Documents/obsidian/private/opensource_lightroom/01-lightroom-capabilities.md` (line 59)
- Modify: `docs/superpowers/specs/2026-09-08-custom-aspect-design.md` (status)
- Modify: `CLAUDE.md` (resume block: the inconsistency paragraph and next action; a short section)

- [ ] **Step 1: The gates, each from its own exit code**

```bash
cargo fmt --all -- --check; echo "fmt exit $?"
```
```bash
cargo clippy --all-targets --all-features -- -D warnings > /tmp/m55c_clippy.txt 2>&1; echo "clippy exit $?"
```
```bash
cargo test --all > /tmp/m55c_tests.txt 2>&1; echo "test exit $?"; grep -E "^test result" /tmp/m55c_tests.txt | awk '{p+=$4; f+=$6} END {print "passed", p, "failed", f}'
```
```bash
echo "goldens: $(( $(grep -cE '^test .*golden.* ok$' /tmp/m55c_tests.txt) - 7 ))"
```

Expected: all three exits 0, failed 0, goldens **59** (the 66-line count minus the seven tests
that merely contain the word, as M6.2 reconciled).

- [ ] **Step 2: Docs**

Roadmap M5.5 heading: `🟡` → `✅`, and its annotation gains
`· c custom ratios + swap ✅ 2026-09-08`. Capabilities line 59: `aspect lock ✅` →
`aspect lock ✅ (presets, custom W:H, orientation swap)`. Spec status →
`**built 2026-09-08.** Plan \`2026-09-08-custom-aspect.md\`.`

`CLAUDE.md` resume block: replace the paragraph beginning `⚠⚠ **AND ONE ROADMAP INCONSISTENCY IS
OPEN AND UNRESOLVED` with one sentence saying M5.5c is built (2026-09-08) and the Phase 5 marker
is now true, keeping the note that the roadmap prose still shows three stale 🟡 markers (M3.2,
M3.3, M3.4). Update the **▶ NEXT ACTION** to drop M5.5c from its list, and the handoff test count.
Add a short **"M5.5c — custom aspect ratios and a swap"** section above the M6.2 one carrying: the
three user decisions; that `swap_orientation` preserves the transposed pixel ratio under a shrink
and why that matters for the lock; the `chip_state` inverse rule and tie-break; the text-label
decision for Swap; the settle rule for a typed `DragValue`; and the two oversized files.

- [ ] **Step 3: Launch the GUI**

`cargo run --release -p app` in the background. What to look at: Geometry tool → Custom → type
21 : 9 → the crop snaps to a wide band; Swap → it turns tall, the fields read 9 : 21; 3:2 chip →
Swap → the chip stays lit and the crop is 2:3; Free → Swap → the crop turns, nothing locks.

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md docs/superpowers/specs/2026-09-08-custom-aspect-design.md
git commit -m "Record M5.5c; Phase 5's marker is true"
```

- [ ] **Step 5: Report**

Test delta, 59 goldens ran and none moved, the branch not merged and not pushed, the two
oversized files, and what the GUI check is for.
