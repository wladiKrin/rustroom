# M5.3b Plan 1 — Spot-removal placement

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `Spot`s placeable, selectable, movable and deletable on the Develop canvas, so M5.3a's headless spot removal is reachable without hand-editing a sidecar.

**Architecture:** A fifth rail tool (`Tool::Retouch`). The gesture *maths* lives in two new
egui-free lib modules — `spot_tool` (hit-test, place, drag) and `spot_source` (auto-pick over an
**injected sampler**) — so both are testable with no window. The egui lives in two new bin modules,
`develop/panel/retouch.rs` and `develop/canvas/spot_overlay.rs`. **No `Edits` field and no
`pipeline` stage is added**; M5.3a built that half.

**Tech Stack:** Rust workspace; egui/eframe 0.35; the existing `test_app` + `test_ui` headless
canvas fixture.

**Spec:** `docs/superpowers/specs/2026-09-03-spot-removal-canvas-design.md`

---

## Ground rules for every task

- Build/test with the project's env:
  ```bash
  export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
  ```
- Before every commit: `cargo fmt --all`, then **`cargo clippy --all --all-targets --all-features -- -D warnings`** and check its exit code. `cargo test` being green is not evidence about clippy — this project has shipped six commits with clippy failing.
- **All 53 goldens must stay byte-identical.** Check with
  `git status --porcelain crates/pipeline/tests/golden/ | wc -l` → `0`. Nothing here touches a stage.
- Commit messages carry **no `Co-Authored-By` trailer**.
- ⚠ **zsh does not word-split `$var`.** Any sabotage helper must use an array, or `cargo test $args`
  silently passes one malformed argument and every sabotage reports "DID NOT COMPILE".

---

## File structure

| File | Responsibility |
| --- | --- |
| `crates/app/src/icons.rs` | **modify** — add `RETOUCH` (`bandaids`, U+E0B2), extend `ALL` to 12 |
| `crates/app/src/tool.rs` | **modify** — `Tool::Retouch`, `ALL` to 5, label/glyph/heading arms |
| `crates/app/src/spot_tool.rs` | **create** — `Handle`, `hit_test`, `apply_drag`, `place`, `SpotTool` |
| `crates/app/src/spot_source.rs` | **create** — `pick` over an injected sampler |
| `crates/app/src/lib.rs` | **modify** — declare the two new modules |
| `crates/app/src/main.rs` | **modify** — `Session.selected_spot`, `RustroomApp.spot_tool` |
| `crates/app/src/develop/panel/retouch.rs` | **create** — Size, Feather, Clone/Heal, count, Clear all |
| `crates/app/src/develop/panel/mod.rs` | **modify** — dispatch `Tool::Retouch` |
| `crates/app/src/develop/canvas/spot_overlay.rs` | **create** — overlay + gestures |
| `crates/app/src/develop/canvas/mod.rs` | **modify** — dispatch `Tool::Retouch` |
| `crates/app/src/test_app.rs` | **modify** — the two new fields in the exhaustive literals |

---

## Task 1: The glyph

**Files:**
- Modify: `crates/app/src/icons.rs`

- [ ] **Step 1: Add the constant and extend `ALL`**

In `crates/app/src/icons.rs`, after the `PRESETS` constant:

```rust
/// `bandaids` — the Retouch tool.
///
/// ⚠ The plaster, which is Photoshop's healing-brush symbol and reads as "spot removal" at a
/// glance. Verified present in the bundled 2.1.2 TTF by parsing its cmap, not assumed: four
/// separate features in this project have picked a glyph that turned out to be absent and would
/// have shipped a tofu box.
pub const RETOUCH: char = '\u{e0b2}';
```

Change the `ALL` declaration from `[char; 11]` to `[char; 12]` and add `RETOUCH` after `PRESETS`:

```rust
pub const ALL: [char; 12] = [
    EDIT,
    CROP,
    MASKS,
    PRESETS,
    RETOUCH,
    UNDO,
    REDO,
    ZOOM_OUT,
    ZOOM_IN,
    ZOOM_FIT,
    ZOOM_ACTUAL,
    BEFORE_AFTER,
];
```

- [ ] **Step 2: Verify the glyph renders**

```bash
cargo test -p app --bin Rustroom renderable 2>&1 | grep -E "test result|panicked"
```
Expected: PASS. The existing `every_icon_is_in_the_private_use_area` and the coverage tests sweep
`icons::ALL`, so `RETOUCH` is now covered with no new test.

- [ ] **Step 3: Prove the guard would have caught a wrong codepoint**

Temporarily set `RETOUCH` to `'\u{f8fe}'` (inside the PUA, outside Phosphor's block), run the same
command, and confirm the coverage test **fails**. Restore `e0b2` afterwards.

Expected: FAIL, then PASS again after restoring.

- [ ] **Step 4: Commit**

```bash
git add crates/app/src/icons.rs
git commit -m "feat(app): a Retouch glyph, verified against the bundled font"
```

---

## Task 2: `Tool::Retouch`

**Files:**
- Modify: `crates/app/src/tool.rs`

- [ ] **Step 1: Write the failing tests**

In `crates/app/src/tool.rs`, inside `mod tests`, change the two existing count assertions from `4`
to `5` in `labels_and_glyphs_are_all_distinct`, and add to
`the_digits_select_the_rail_in_order`:

```rust
        assert_eq!(Tool::from_digit(5), Some(Tool::Retouch));
        assert_eq!(Tool::from_digit(6), None);
```

⚠ Delete the old `assert_eq!(Tool::from_digit(5), None);` line — it now asserts the opposite of the
truth, and leaving it would be a test that is wrong about correct code.

Then add:

```rust
    #[test]
    fn retouch_is_not_a_geometry_tool_and_has_its_own_heading() {
        // ⚠ Retouch must NOT be a geometry tool: `is_geometry` pins the canvas to the fit rung
        // and makes `preview_edits` strip the crop, so a spot placed there would be positioned
        // against a frame the user never sees applied.
        assert!(!Tool::Retouch.is_geometry());
        assert_eq!(Tool::Retouch.panel_heading(), Some("Retouch"));
    }
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test -p app --lib tool 2>&1 | grep -E "test result|error\[E0"
```
Expected: FAIL to compile — `no variant named Retouch found for enum Tool`.

- [ ] **Step 3: Add the variant**

In `crates/app/src/tool.rs`:

```rust
pub enum Tool {
    #[default]
    Edit,
    Crop,
    Masks,
    Retouch,
    Presets,
}
```

⚠ **Retouch goes before Presets, not at the end.** The rail is ordered by what a photographer
reaches for while working on the picture; Presets is a library of saved looks and stays last. This
also keeps `4` on Presets — no, it does **not**: it moves Presets to `5`. That is deliberate and is
the one shipped shortcut this change moves; note it in the commit message.

Then:

```rust
    pub const ALL: [Tool; 5] = [
        Self::Edit,
        Self::Crop,
        Self::Masks,
        Self::Retouch,
        Self::Presets,
    ];
```

and add arms to the three matches:

```rust
            Self::Retouch => "Retouch",              // in label()
            Self::Retouch => crate::icons::RETOUCH,  // in glyph()
            Self::Retouch => Some("Retouch"),        // in panel_heading()
```

⚠ `is_geometry` needs **no** arm — it is `matches!(self, Self::Crop)`.

- [ ] **Step 4: Run the tests**

```bash
cargo test -p app --lib tool 2>&1 | grep -E "test result|panicked"
```
Expected: PASS.

- [ ] **Step 5: Fix the fallout the compiler names**

`cargo build --all --all-targets` will fail on every exhaustive `match tool` — expected, and it is
what those matches are for. Add `Tool::Retouch` to:
- `crates/app/src/develop/canvas/mod.rs:252` — for now put it in the do-nothing arm
  (`Tool::Edit | Tool::Presets | Tool::Retouch => {}`); Task 7 replaces this.
- `crates/app/src/develop/panel/mod.rs:233` — for now `Tool::Presets | Tool::Retouch => {}`;
  Task 6 replaces this.

Any other site the compiler names, handle the same way: no behaviour yet.

- [ ] **Step 6: Verify green and commit**

```bash
cargo fmt --all
cargo clippy --all --all-targets --all-features -- -D warnings; echo "CLIPPY=$?"
cargo test --all 2>&1 | grep -c "test result: FAILED"
git add -A
git commit -m "feat(app): a fifth rail tool, Retouch

⚠ Presets moves from shortcut 4 to 5. The rail is ordered by what a
photographer reaches for while working on the picture, and a library of saved
looks belongs after the tools that change it."
```

---

## Task 3: `spot_tool` — hit-test and drag maths

**Files:**
- Create: `crates/app/src/spot_tool.rs`
- Modify: `crates/app/src/lib.rs`

- [ ] **Step 1: Declare the module**

In `crates/app/src/lib.rs`, beside the other `pub mod` lines, add:

```rust
pub mod spot_tool;
```

- [ ] **Step 2: Write the failing tests**

Create `crates/app/src/spot_tool.rs` containing **only** the test module for now:

```rust
//! The retouch tool's gesture maths: which handle a point grabs, and what a drag does to it.
//!
//! ⚠ No egui. `mask_tool`'s rule, for its reason: the decisions are what is worth testing, and a
//! window is not needed to test them.

#[cfg(test)]
mod tests {
    use super::*;
    use edit_model::{Spot, SpotMode};

    fn spot(cx: f32, cy: f32, sx: f32, sy: f32, r: f32) -> Spot {
        Spot { centre: [cx, cy], source: [sx, sy], radius: r, feather: 50.0, mode: SpotMode::Heal }
    }

    /// ⚠⚠ The destination wins a tie. Both discs are grabbable, and where they overlap the
    /// destination is the one the user means — it is the thing being removed. A hit-test that
    /// preferred the source would make a spot whose source sits on its own destination
    /// impossible to move.
    #[test]
    fn the_destination_wins_when_both_discs_contain_the_point() {
        let s = [spot(0.5, 0.5, 0.52, 0.5, 0.05)];
        assert_eq!(hit_test(&s, [0.51, 0.5], 1.0), Some((0, Handle::Centre)));
    }

    #[test]
    fn the_source_is_grabbable_where_the_destination_is_not() {
        let s = [spot(0.2, 0.5, 0.8, 0.5, 0.05)];
        assert_eq!(hit_test(&s, [0.8, 0.5], 1.0), Some((0, Handle::Source)));
        assert_eq!(hit_test(&s, [0.2, 0.5], 1.0), Some((0, Handle::Centre)));
        assert_eq!(hit_test(&s, [0.5, 0.5], 1.0), None, "empty space grabs nothing");
    }

    /// ⚠ The later spot wins, because it is drawn on top and is what the user sees.
    #[test]
    fn the_last_spot_placed_is_the_one_grabbed() {
        let s = [spot(0.5, 0.5, 0.2, 0.2, 0.05), spot(0.5, 0.5, 0.8, 0.8, 0.05)];
        assert_eq!(hit_test(&s, [0.5, 0.5], 1.0), Some((1, Handle::Centre)));
    }

    /// ⚠⚠ The radius is a fraction of the **long edge**, so on a wide frame a disc spans less of
    /// the width than of the height. A hit-test that compared normalised distance directly would
    /// make every disc an ellipse to grab — the aspect trap `BrushStroke::radius` records.
    #[test]
    fn hit_testing_is_round_on_a_wide_frame() {
        let s = [spot(0.5, 0.5, 0.2, 0.2, 0.10)];
        // aspect 2.0: the frame is twice as wide as it is tall, so the long edge is the width.
        // A point 0.09 of the WIDTH to the right is inside; 0.09 of the HEIGHT above is not,
        // because 0.09 of the height is only 0.045 of the long edge... and is therefore inside
        // too. The discriminating probe is one that is inside on one axis and outside on the
        // other under the wrong maths.
        assert!(hit_test(&s, [0.59, 0.5], 2.0).is_some(), "on the long axis");
        assert!(hit_test(&s, [0.5, 0.71], 2.0).is_none(), "beyond the disc vertically");
        assert!(hit_test(&s, [0.5, 0.60], 2.0).is_some(), "inside the disc vertically");
    }

    #[test]
    fn dragging_the_centre_moves_only_the_destination() {
        let mut s = spot(0.5, 0.5, 0.2, 0.2, 0.05);
        apply_drag(&mut s, Handle::Centre, 0.1, -0.05);
        assert_eq!(s.centre, [0.6, 0.45]);
        assert_eq!(s.source, [0.2, 0.2], "the source moved with it");
    }

    #[test]
    fn dragging_the_source_moves_only_the_source() {
        let mut s = spot(0.5, 0.5, 0.2, 0.2, 0.05);
        apply_drag(&mut s, Handle::Source, -0.1, 0.05);
        assert!((s.source[0] - 0.1).abs() < 1e-6 && (s.source[1] - 0.25).abs() < 1e-6);
        assert_eq!(s.centre, [0.5, 0.5], "the destination moved with it");
    }

    /// ⚠ Clamped into the frame: a spot dragged off the edge is unreachable afterwards, which is
    /// the failure `mask_tool::apply_drag` clamps for.
    #[test]
    fn a_drag_cannot_push_a_handle_out_of_the_frame() {
        let mut s = spot(0.5, 0.5, 0.5, 0.5, 0.05);
        apply_drag(&mut s, Handle::Centre, 5.0, 5.0);
        assert!((0.0..=1.0).contains(&s.centre[0]) && (0.0..=1.0).contains(&s.centre[1]));
    }

    /// ⚠⚠ Sizing measures the drag against the **long edge**, the same unit the radius is stored
    /// in. Measuring it against the width would make the same gesture produce a different radius
    /// on a portrait frame than on a landscape one.
    #[test]
    fn drag_radius_is_the_distance_in_long_edge_units() {
        // aspect 2.0 → the long edge is the width, so dx is already in long-edge units.
        let r = drag_radius([0.5, 0.5], [0.62, 0.5], 2.0);
        assert!((r - 0.12).abs() < 1e-6, "got {r}");
        // The same *visual* distance vertically is half as many long-edge units.
        let r = drag_radius([0.5, 0.5], [0.5, 0.74], 2.0);
        assert!((r - 0.12).abs() < 1e-6, "got {r}");
    }

    /// ⚠ A radius below the floor is a spot that covers nothing and cannot be seen or grabbed —
    /// which is what a click with a shaky hand would otherwise place.
    #[test]
    fn the_radius_has_a_floor_and_a_ceiling() {
        assert_eq!(drag_radius([0.5, 0.5], [0.5, 0.5], 1.0), MIN_RADIUS);
        assert_eq!(drag_radius([0.5, 0.5], [9.0, 9.0], 1.0), MAX_RADIUS);
    }
}
```

- [ ] **Step 3: Run to verify they fail**

```bash
cargo test -p app --lib spot_tool 2>&1 | grep -E "error\[E0|test result"
```
Expected: FAIL to compile — `cannot find function hit_test in this scope`.

- [ ] **Step 4: Write the implementation**

Prepend to `crates/app/src/spot_tool.rs`, above the test module:

```rust
use edit_model::{Spot, SpotMode};

/// The smallest radius a gesture may produce, as a fraction of the long edge.
///
/// ⚠ A spot below this covers no pixels, draws as a dot, and cannot be grabbed again — so a click
/// with a shaky hand would leave litter the user can neither see nor remove.
pub const MIN_RADIUS: f32 = 0.004;
/// The largest. Beyond this a "spot" is a region and the wrong tool.
pub const MAX_RADIUS: f32 = 0.25;

/// Which end of a spot a gesture has hold of.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Handle {
    /// The destination — the thing being removed.
    Centre,
    /// The source — where the pixels are copied from.
    Source,
}

/// Distance between two normalised points measured in **long-edge units**.
///
/// ⚠⚠ Normalised coordinates are anisotropic: `0.1` of the width and `0.1` of the height are
/// different distances unless the frame is square. `Spot::radius` is a fraction of the long edge,
/// so every comparison against it must be in the same unit or a disc becomes an ellipse — the
/// trap `BrushStroke::radius` records and `mask_tool::hit_test` corrects for separately.
fn long_edge_distance(a: [f32; 2], b: [f32; 2], aspect: f32) -> f32 {
    let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
    // With aspect = w/h, one unit of normalised y is `1/aspect` units of normalised x when
    // aspect >= 1 (the width is the long edge), and the mirror when it is not.
    let (sx, sy) = if aspect >= 1.0 {
        (1.0, 1.0 / aspect)
    } else {
        (aspect, 1.0)
    };
    ((dx * sx).powi(2) + (dy * sy).powi(2)).sqrt()
}

/// The spot and handle a normalised point grabs, if any.
///
/// ⚠ Searched **last-first**, so the most recently placed spot — the one drawn on top — wins.
/// ⚠ The destination is tested before the source within one spot: where the two discs overlap,
/// the destination is what the user means, and preferring the source would make a spot whose
/// source sits on its own destination impossible to move.
pub fn hit_test(spots: &[Spot], p: [f32; 2], aspect: f32) -> Option<(usize, Handle)> {
    for (i, s) in spots.iter().enumerate().rev() {
        if long_edge_distance(s.centre, p, aspect) <= s.radius {
            return Some((i, Handle::Centre));
        }
        if long_edge_distance(s.source, p, aspect) <= s.radius {
            return Some((i, Handle::Source));
        }
    }
    None
}

/// Move one end of a spot by a normalised delta, clamped into the frame.
///
/// ⚠ Clamped because a handle dragged off the edge cannot be grabbed again — `mask_tool`'s rule.
pub fn apply_drag(s: &mut Spot, handle: Handle, dx: f32, dy: f32) {
    let end = match handle {
        Handle::Centre => &mut s.centre,
        Handle::Source => &mut s.source,
    };
    end[0] = (end[0] + dx).clamp(0.0, 1.0);
    end[1] = (end[1] + dy).clamp(0.0, 1.0);
}

/// The radius a size-drag from `from` to `to` produces, in long-edge units.
pub fn drag_radius(from: [f32; 2], to: [f32; 2], aspect: f32) -> f32 {
    long_edge_distance(from, to, aspect).clamp(MIN_RADIUS, MAX_RADIUS)
}

/// The retouch tool's live settings, held on `RustroomApp`.
///
/// ⚠ Session-lifetime, not persisted — the brush's rule. Unlike the brush's screen-point size this
/// one is frame-normalised and *could* persist; it does not, so that the two tools behave alike.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpotTool {
    /// Radius as a fraction of the long edge. Shared with the size-drag: one value, two
    /// affordances — the white-balance eyedropper-and-Kelvin idiom.
    pub radius: f32,
    pub feather: f32,
    pub mode: SpotMode,
}

impl Default for SpotTool {
    /// ⚠ Hand-written, not derived: a derived `Default` gives radius 0, which places spots that
    /// cover nothing. The default radius is generous on purpose — A's Wave 3 finding is that the
    /// radius must *exceed* the blemish, or heal measures a ring that is still on it.
    fn default() -> Self {
        Self { radius: 0.02, feather: 50.0, mode: SpotMode::Heal }
    }
}

/// A new spot at `centre` with `source`, taking the tool's current settings.
pub fn place(centre: [f32; 2], source: [f32; 2], tool: SpotTool) -> Spot {
    Spot {
        centre,
        source,
        radius: tool.radius.clamp(MIN_RADIUS, MAX_RADIUS),
        feather: tool.feather,
        mode: tool.mode,
    }
}
```

- [ ] **Step 5: Run the tests**

```bash
cargo test -p app --lib spot_tool 2>&1 | grep -E "test result|panicked|assertion"
```
Expected: PASS, 9 tests.

- [ ] **Step 6: Sabotage the aspect correction**

Change `long_edge_distance`'s `(sx, sy)` to `(1.0, 1.0)` and re-run.

Expected: `hit_testing_is_round_on_a_wide_frame` and `drag_radius_is_the_distance_in_long_edge_units`
**FAIL**. Restore.

- [ ] **Step 7: Sabotage the hit-test order**

Change `.rev()` to nothing (search first-first) and re-run.

Expected: `the_last_spot_placed_is_the_one_grabbed` **FAILS**. Restore.

- [ ] **Step 8: Commit**

```bash
cargo fmt --all
cargo clippy --all --all-targets --all-features -- -D warnings; echo "CLIPPY=$?"
git add crates/app/src/spot_tool.rs crates/app/src/lib.rs
git commit -m "feat(app): the retouch tool's gesture maths

Hit-test, drag and sizing as pure functions over plain types — mask_tool's
pattern, testable with no window.

⚠ Every distance is measured in LONG-EDGE units, because Spot::radius is a
fraction of the long edge while normalised coordinates are anisotropic.
Comparing a raw normalised distance against the radius makes every disc an
ellipse to grab, which is the trap BrushStroke::radius already records."
```

---

## Task 4: `spot_source` — the auto-pick

**Files:**
- Create: `crates/app/src/spot_source.rs`
- Modify: `crates/app/src/lib.rs`

- [ ] **Step 1: Declare the module**

In `crates/app/src/lib.rs`:

```rust
pub mod spot_source;
```

- [ ] **Step 2: Write the failing tests**

Create `crates/app/src/spot_source.rs` with only the test module:

```rust
//! Choosing a plausible source for a retouch spot.
//!
//! ⚠ No egui, and no `ImageBuf`: the search runs over an **injected sampler**, so it is testable
//! with a synthetic closure and has no dependency on `lens_profile` or `perspective`. The caller
//! composes the corrected → raw mapping; see the spec.

#[cfg(test)]
mod tests {
    use super::*;
    use edit_model::{Spot, SpotMode};

    /// A frame that is flat grey on the left half and flat red on the right half.
    fn split(p: [f32; 2]) -> [f32; 3] {
        if p[0] < 0.5 { [0.5, 0.5, 0.5] } else { [0.8, 0.1, 0.1] }
    }

    /// ⚠ The pick must land on the side of the split the destination is on: a source from the
    /// other half would paste red onto grey. This is the whole point of scoring against the ring.
    #[test]
    fn the_pick_stays_on_the_matching_side_of_an_edge() {
        let got = pick(split, [0.25, 0.5], 0.03, 1.0, &[]).expect("a source");
        assert!(got[0] < 0.5, "picked across the edge: {got:?}");
        let got = pick(split, [0.75, 0.5], 0.03, 1.0, &[]).expect("a source");
        assert!(got[0] > 0.5, "picked across the edge: {got:?}");
    }

    /// ⚠ Never overlapping the destination — a source disc that overlaps its own destination
    /// copies part of the blemish back onto itself.
    #[test]
    fn the_pick_never_overlaps_the_destination() {
        let dest = [0.5, 0.5];
        let r = 0.04;
        let got = pick(|_| [0.5, 0.5, 0.5], dest, r, 1.0, &[]).expect("a source");
        let d = ((got[0] - dest[0]).powi(2) + (got[1] - dest[1]).powi(2)).sqrt();
        assert!(d >= 2.0 * r, "source overlaps the destination: d={d}, r={r}");
    }

    /// ⚠ And never on an existing spot's destination, which is a blemish somebody is removing.
    #[test]
    fn the_pick_avoids_an_existing_spots_destination() {
        let existing = [Spot {
            centre: [0.5, 0.36],
            source: [0.1, 0.1],
            radius: 0.04,
            feather: 50.0,
            mode: SpotMode::Heal,
        }];
        let got = pick(|_| [0.5, 0.5, 0.5], [0.5, 0.5], 0.04, 1.0, &existing).expect("a source");
        let d = ((got[0] - 0.5f32).powi(2) + (got[1] - 0.36f32).powi(2)).sqrt();
        assert!(d >= 0.08, "picked onto an existing blemish: d={d}");
    }

    /// ⚠⚠ **A's Wave 3 finding, one layer up.** The ring must be sampled OUTSIDE the blemish. A
    /// ring flush with the rim of a too-small radius *is* the blemish, so the signature would be
    /// the blemish's colour and the search would hunt for a source that looks like the thing
    /// being removed. Here the blemish is a dark disc of radius 1.5r on a flat field.
    #[test]
    fn the_ring_is_sampled_outside_the_blemish() {
        let dest = [0.5, 0.5];
        let r = 0.02;
        let field = move |p: [f32; 2]| {
            let d = ((p[0] - dest[0]).powi(2) + (p[1] - dest[1]).powi(2)).sqrt();
            if d < 1.5 * r { [0.05, 0.05, 0.05] } else { [0.6, 0.6, 0.6] }
        };
        let got = pick(field, dest, r, 1.0, &[]).expect("a source");
        // The source must be a light patch, not another dark one.
        assert!(field(got)[0] > 0.5, "picked a source matching the blemish: {got:?}");
    }

    /// ⚠ A degenerate radius must not divide by zero or spin — it returns `None` and the caller
    /// falls back.
    #[test]
    fn a_degenerate_radius_yields_none() {
        assert_eq!(pick(|_| [0.5; 3], [0.5, 0.5], 0.0, 1.0, &[]), None);
        assert_eq!(pick(|_| [0.5; 3], [0.5, 0.5], -1.0, 1.0, &[]), None);
    }

    /// ⚠ A destination in the corner has few legal candidates; the search must still find one
    /// rather than returning `None` and making the tool feel broken at the edges.
    #[test]
    fn a_corner_destination_still_finds_a_source() {
        assert!(pick(|_| [0.5; 3], [0.03, 0.03], 0.02, 1.0, &[]).is_some());
    }
}
```

- [ ] **Step 3: Run to verify they fail**

```bash
cargo test -p app --lib spot_source 2>&1 | grep -E "error\[E0|test result"
```
Expected: FAIL to compile — `cannot find function pick`.

- [ ] **Step 4: Write the implementation**

Prepend to `crates/app/src/spot_source.rs`:

```rust
use edit_model::Spot;

/// Candidate directions and distances. 16 × 5 = 80 probes, which is a few milliseconds and only
/// on a click.
const ANGLES: usize = 16;
const DISTANCES: [f32; 5] = [2.0, 3.0, 4.0, 5.0, 6.0];

/// Where the destination's signature ring is sampled, as multiples of the radius.
///
/// ⚠⚠ **Outset from the rim, not flush with it, and this is A's Wave 3 finding arriving in the
/// search.** That finding — *the retouch radius must exceed the blemish, or heal measures a ring
/// that is still lying on the spot* — applies here identically: a flush ring on a too-small
/// radius **is** the blemish, so the signature becomes the blemish's own colour and the search
/// confidently returns a source that looks like the thing being removed.
const RING: [f32; 3] = [1.2, 1.4, 1.6];

/// How many points to sample around a ring, and across a disc.
const RING_SAMPLES: usize = 24;
/// ⚠ A disc is sampled on a coarse lattice rather than per pixel: a mean and a variance need
/// nothing finer, and it is what keeps a large-radius search in single-digit milliseconds.
const DISC_STEPS: i32 = 3;

fn mean(samples: &[[f32; 3]]) -> [f32; 3] {
    let n = samples.len().max(1) as f32;
    let mut m = [0.0; 3];
    for s in samples {
        for c in 0..3 {
            m[c] += s[c];
        }
    }
    m.map(|v| v / n)
}

fn distance2(a: [f32; 3], b: [f32; 3]) -> f32 {
    (0..3).map(|c| (a[c] - b[c]).powi(2)).sum()
}

/// The scaling that turns a long-edge radius into per-axis normalised offsets.
fn axes(aspect: f32) -> (f32, f32) {
    if aspect >= 1.0 {
        (1.0, aspect)
    } else {
        (1.0 / aspect, 1.0)
    }
}

fn sample_ring(sample: &impl Fn([f32; 2]) -> [f32; 3], c: [f32; 2], r: f32, aspect: f32) -> Vec<[f32; 3]> {
    let (ax, ay) = axes(aspect);
    let mut out = Vec::with_capacity(RING.len() * RING_SAMPLES);
    for m in RING {
        for k in 0..RING_SAMPLES {
            let t = k as f32 / RING_SAMPLES as f32 * std::f32::consts::TAU;
            let p = [
                (c[0] + m * r * t.cos() * ax).clamp(0.0, 1.0),
                (c[1] + m * r * t.sin() * ay).clamp(0.0, 1.0),
            ];
            out.push(sample(p));
        }
    }
    out
}

fn sample_disc(sample: &impl Fn([f32; 2]) -> [f32; 3], c: [f32; 2], r: f32, aspect: f32) -> Vec<[f32; 3]> {
    let (ax, ay) = axes(aspect);
    let mut out = Vec::new();
    for iy in -DISC_STEPS..=DISC_STEPS {
        for ix in -DISC_STEPS..=DISC_STEPS {
            let (fx, fy) = (ix as f32 / DISC_STEPS as f32, iy as f32 / DISC_STEPS as f32);
            if fx * fx + fy * fy > 1.0 {
                continue;
            }
            out.push(sample([
                (c[0] + fx * r * ax).clamp(0.0, 1.0),
                (c[1] + fy * r * ay).clamp(0.0, 1.0),
            ]));
        }
    }
    out
}

fn variance(samples: &[[f32; 3]], m: [f32; 3]) -> f32 {
    let n = samples.len().max(1) as f32;
    samples.iter().map(|s| distance2(*s, m)).sum::<f32>() / n
}

/// A plausible source for a spot at `dest` of `radius` (long-edge units).
///
/// `sample` returns the colour at a normalised point **in the spot-removal stage's input space**;
/// the caller is responsible for mapping that through the lens and perspective inverses. `None`
/// when nothing qualifies — the caller then places a fixed offset rather than refusing the click.
///
/// ⚠ Scored on the colour distance from the destination's ring **plus** the variance inside the
/// candidate: a flat patch copies more cleanly than an edge.
pub fn pick(
    sample: impl Fn([f32; 2]) -> [f32; 3],
    dest: [f32; 2],
    radius: f32,
    aspect: f32,
    existing: &[Spot],
) -> Option<[f32; 2]> {
    if !(radius.is_finite() && radius > 0.0) || !aspect.is_finite() || aspect <= 0.0 {
        return None;
    }
    let (ax, ay) = axes(aspect);
    let target = mean(&sample_ring(&sample, dest, radius, aspect));

    let mut best: Option<([f32; 2], f32)> = None;
    for k in 0..ANGLES {
        let t = k as f32 / ANGLES as f32 * std::f32::consts::TAU;
        for m in DISTANCES {
            let c = [
                dest[0] + m * radius * t.cos() * ax,
                dest[1] + m * radius * t.sin() * ay,
            ];
            // Wholly inside the frame, measured on each axis in its own normalised units.
            if c[0] - radius * ax < 0.0
                || c[0] + radius * ax > 1.0
                || c[1] - radius * ay < 0.0
                || c[1] + radius * ay > 1.0
            {
                continue;
            }
            // ⚠ Never overlapping the destination: a source disc that overlaps its own
            // destination copies part of the blemish back onto itself.
            if long_edge_gap(c, dest, aspect) < 2.0 * radius {
                continue;
            }
            // ⚠ Nor an existing spot's destination, which is a blemish somebody is removing.
            if existing
                .iter()
                .any(|s| long_edge_gap(c, s.centre, aspect) < radius + s.radius)
            {
                continue;
            }
            let disc = sample_disc(&sample, c, radius, aspect);
            let m_disc = mean(&disc);
            let score = distance2(m_disc, target) + variance(&disc, m_disc);
            if best.is_none_or(|(_, b)| score < b) {
                best = Some((c, score));
            }
        }
    }
    best.map(|(c, _)| c)
}

/// Distance between two normalised points in long-edge units. Mirrors
/// `spot_tool`'s private helper; kept separate so neither module depends on the other.
fn long_edge_gap(a: [f32; 2], b: [f32; 2], aspect: f32) -> f32 {
    let (sx, sy) = if aspect >= 1.0 {
        (1.0, 1.0 / aspect)
    } else {
        (aspect, 1.0)
    };
    (((b[0] - a[0]) * sx).powi(2) + ((b[1] - a[1]) * sy).powi(2)).sqrt()
}
```

- [ ] **Step 5: Run the tests**

```bash
cargo test -p app --lib spot_source 2>&1 | grep -E "test result|panicked|assertion"
```
Expected: PASS, 6 tests.

- [ ] **Step 6: Sabotage the ring outset**

Change `const RING: [f32; 3] = [1.2, 1.4, 1.6];` to `[0.9, 0.95, 1.0]` (flush with the rim) and
re-run.

Expected: `the_ring_is_sampled_outside_the_blemish` **FAILS**. Restore.

- [ ] **Step 7: Sabotage the overlap rejection**

Delete the `if long_edge_gap(c, dest, aspect) < 2.0 * radius { continue; }` block and re-run.

Expected: `the_pick_never_overlaps_the_destination` **FAILS**. Restore.

- [ ] **Step 8: Commit**

```bash
cargo fmt --all
cargo clippy --all --all-targets --all-features -- -D warnings; echo "CLIPPY=$?"
git add crates/app/src/spot_source.rs crates/app/src/lib.rs
git commit -m "feat(app): auto-pick a retouch source

Scores 16x5 candidates on colour distance from the destination's ring plus the
variance inside the candidate — a flat patch copies more cleanly than an edge.

⚠⚠ The ring is sampled at 1.2r-1.6r, OUTSET from the rim, and that is A's Wave 3
finding arriving one layer up: a ring flush with a too-small radius IS the
blemish, so the signature becomes the blemish's own colour and the search
confidently returns a source that looks like the thing being removed.

⚠ The sampler is injected rather than the search taking a buffer, so this stays
a pure function with no dependency on lens_profile or perspective, and its
tests pass a synthetic closure with no image at all."
```

---

## Task 5: The two state fields

**Files:**
- Modify: `crates/app/src/main.rs`
- Modify: `crates/app/src/test_app.rs`

- [ ] **Step 1: Add `Session.selected_spot`**

In `crates/app/src/main.rs`, beside `selected_mask: Option<usize>,`:

```rust
    /// Which spot the Retouch tool has selected, if any.
    ///
    /// ⚠ **View state, never `Edits`** — `selected_mask`'s rule: a selection is not part of the
    /// photograph's description, so it must not reach the sidecar or travel with a paste. It
    /// dies with the session, which is what makes a photo switch reset it for free.
    selected_spot: Option<usize>,
```

- [ ] **Step 2: Add `RustroomApp.spot_tool`**

In `crates/app/src/main.rs`, on `RustroomApp` beside the brush fields:

```rust
    /// The Retouch tool's live settings. Session-lifetime, not persisted — the brush's rule.
    spot_tool: rustroom_app::spot_tool::SpotTool,
```

- [ ] **Step 3: Fill in every construction site the compiler names**

```bash
cargo build --all --all-targets 2>&1 | grep -E "^error\[E0063\]" -A 4 | head -20
```
Add `selected_spot: None,` to each `Session { .. }` literal and
`spot_tool: Default::default(),` to each `RustroomApp { .. }` literal — including the exhaustive
ones in `crates/app/src/test_app.rs`, which exist precisely to force this decision.

- [ ] **Step 4: Clear the selection on a snapshot restore**

Find `restore_snapshot` (grep: `grep -rn "fn restore_snapshot" crates/app/src`). Beside the
existing `selected_mask = None`, add:

```rust
        // ⚠ Same reason as `selected_mask`: a restored state has a different spot list, so a
        // stale index would leave a spot selected that nobody selected.
        s.selected_spot = None;
```

- [ ] **Step 5: Verify and commit**

```bash
cargo fmt --all
cargo clippy --all --all-targets --all-features -- -D warnings; echo "CLIPPY=$?"
cargo test --all 2>&1 | grep -c "test result: FAILED"
git add -A
git commit -m "feat(app): retouch selection and tool state

Both view state, never Edits — selected_mask's rule. Cleared on a snapshot
restore, because a restored state has a different spot list and a stale index
would select a spot nobody selected."
```

---

## Task 6: The Retouch panel

**Files:**
- Create: `crates/app/src/develop/panel/retouch.rs`
- Modify: `crates/app/src/develop/panel/mod.rs`

- [ ] **Step 1: Write the panel module**

Create `crates/app/src/develop/panel/retouch.rs`:

```rust
//! The Retouch tool's controls: the patch's size and feather, its mode, and what to do with the
//! spots already placed.

use super::Edited;
use eframe::egui;
use rustroom_app::spot_tool::{SpotTool, MAX_RADIUS, MIN_RADIUS};
use edit_model::SpotMode;

/// Draw the Retouch controls. Returns `true` when the edits changed.
///
/// ⚠ `tool` is `&mut` to a field of `RustroomApp` and `spots` to one of `Session` — **disjoint**
/// borrows, which is what makes this legal where a closure capturing the app twice would be
/// `E0499`. The `masks::BrushTool` precedent.
pub(crate) fn show(
    ui: &mut egui::Ui,
    tool: &mut SpotTool,
    spots: &mut Vec<edit_model::Spot>,
    selected: &mut Option<usize>,
    ed: &mut Edited,
) {
    // ⚠ The Size slider shows and sets **the same value the canvas drag sets** — one value, two
    // affordances, the white-balance eyedropper-and-Kelvin idiom. It is not a second mechanism.
    ui.add(
        egui::Slider::new(&mut tool.radius, MIN_RADIUS..=MAX_RADIUS)
            .text("Size")
            .custom_formatter(|v, _| format!("{:.1}%", v * 100.0)),
    )
    .on_hover_text("The patch's radius, as a percentage of the long edge. Dragging on the canvas sets this too.");
    ui.add(egui::Slider::new(&mut tool.feather, 0.0..=100.0).text("Feather"))
        .on_hover_text("0 is a hard edge; 100 ramps from the centre to the rim.");

    ui.horizontal(|ui| {
        // ⚠ A segmented pair, matching the HSL channel selector and the grading wheels: two
        // mutually exclusive modes read better as one control than as two checkboxes.
        ui.spacing_mut().item_spacing.x = 0.0;
        for (m, label, hint) in [
            (SpotMode::Heal, "Heal", "Matches the source patch to the ring around the destination — what makes a spot on a sky disappear."),
            (SpotMode::Clone, "Clone", "A straight copy, blended by the feather."),
        ] {
            if ui
                .selectable_label(tool.mode == m, format!("  {label}  "))
                .on_hover_text(hint)
                .clicked()
            {
                tool.mode = m;
            }
        }
    });

    ui.separator();

    // ⚠ A count, not a list. Forty dust spots make a useless list, and Lightroom has none either.
    ui.horizontal(|ui| {
        super::super::theme::hint(
            ui,
            &match spots.len() {
                0 => "No spots".to_string(),
                1 => "1 spot".to_string(),
                n => format!("{n} spots"),
            },
        );
        if ui
            .add_enabled(!spots.is_empty(), egui::Button::new("Clear all"))
            .on_hover_text("Removes every retouch patch from this photo")
            .clicked()
        {
            spots.clear();
            *selected = None;
            // ⚠ `discrete`, not a slider's `changed`: a click is a complete act, so it is one
            // history entry immediately rather than waiting for a `drag_stopped` that never comes.
            ed.discrete();
        }
    });
}
```

⚠ Check `theme::hint`'s real path before compiling — `grep -n "pub fn hint" crates/app/src/theme.rs`
— and use whatever the other panel modules use to reach it.

- [ ] **Step 2: Declare and dispatch it**

In `crates/app/src/develop/panel/mod.rs`, add `mod retouch;` beside the other panel modules, and
replace the placeholder arm from Task 2:

```rust
                        Tool::Retouch => retouch::show(
                            ui,
                            spot_tool,
                            &mut s.edits.spots,
                            &mut s.selected_spot,
                            ed,
                        ),
                        Tool::Presets => {}
```

`spot_tool` is `&mut app.spot_tool`, hoisted above the session borrow exactly as
`app_brush_size` and friends are.

- [ ] **Step 3: Write the test**

Append to `crates/app/src/develop/panel/retouch.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn draw(spots: &mut Vec<edit_model::Spot>, f: impl Fn(&egui::Context)) -> (SpotTool, Edited) {
        let ctx = crate::test_ui::ctx();
        let mut tool = SpotTool::default();
        let mut sel = None;
        let mut ed = Edited::default();
        f(&ctx);
        crate::test_ui::click(&ctx, "Clear all", |ui| {
            ed = Edited::default();
            show(ui, &mut tool, spots, &mut sel, &mut ed);
        });
        (tool, ed)
    }

    fn a_spot() -> edit_model::Spot {
        edit_model::Spot {
            centre: [0.5, 0.5],
            source: [0.2, 0.2],
            radius: 0.02,
            feather: 50.0,
            mode: SpotMode::Heal,
        }
    }

    /// ⚠ One click, one history entry — and it must clear the selection too, or the next
    /// keystroke would act on an index into an empty list.
    #[test]
    fn clear_all_removes_every_spot_and_marks_one_edit() {
        let mut spots = vec![a_spot(), a_spot()];
        let (_, ed) = draw(&mut spots, |_| {});
        assert!(spots.is_empty(), "Clear all left spots behind");
        assert!(ed.changed && ed.settled, "one click, one history entry");
    }

    /// ⚠ The discriminating other half: merely drawing the panel must change nothing, or the
    /// button would fire sixty times a second.
    #[test]
    fn merely_drawing_the_panel_changes_nothing() {
        let ctx = crate::test_ui::ctx();
        let mut spots = vec![a_spot()];
        let mut tool = SpotTool::default();
        let mut sel = Some(0);
        for _ in 0..3 {
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(crate::test_ui::screen()),
                    ..Default::default()
                },
                |ui| {
                    let mut ed = Edited::default();
                    show(ui, &mut tool, &mut spots, &mut sel, &mut ed);
                    assert!(!ed.changed, "drawing the panel reported an edit");
                },
            );
        }
        assert_eq!(spots.len(), 1, "drawing the panel removed a spot");
    }

    /// ⚠ The default radius is generous on purpose: A's Wave 3 finding is that the radius must
    /// EXCEED the blemish, or heal measures a ring that is still lying on it.
    #[test]
    fn the_default_radius_is_usable_rather_than_zero() {
        let t = SpotTool::default();
        assert!(t.radius >= MIN_RADIUS * 4.0, "got {}", t.radius);
        assert_eq!(t.mode, SpotMode::Heal, "Heal is the useful default");
    }
}
```

- [ ] **Step 4: Run the tests**

```bash
cargo test -p app --bin Rustroom retouch 2>&1 | grep -E "test result|panicked|assertion"
```
Expected: PASS, 3 tests.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
cargo clippy --all --all-targets --all-features -- -D warnings; echo "CLIPPY=$?"
git add -A
git commit -m "feat(develop): the Retouch panel

Size, Feather, Heal/Clone, a count and Clear all.

⚠ The Size slider is not a second mechanism: it shows and sets the same value
the canvas drag sets, which is the eyedropper-and-Kelvin idiom M5.2 shipped.

⚠ A count, not a list — forty dust spots make a useless list, and Lightroom has
none either."
```

---

## Task 7: The canvas overlay and gestures

**Files:**
- Create: `crates/app/src/develop/canvas/spot_overlay.rs`
- Modify: `crates/app/src/develop/canvas/mod.rs`

- [ ] **Step 1: Write the overlay module**

Create `crates/app/src/develop/canvas/spot_overlay.rs`:

```rust
//! The retouch overlay: every spot's destination and source, and the gestures that place, move
//! and size them.
//!
//! ⚠⚠ **Every coordinate goes against `frame_rect`, never `img_rect`.** That is the M3.1a drift
//! bug: once a region render is on, `img_rect` covers only the rendered *slice*, so an overlay
//! placed against it follows the view while panning and takes the region's aspect rather than the
//! frame's. `img_rect` survives as the **clip** and nothing else.
//!
//! ⚠⚠ **Spots are stored pre-geometry and this draws on the post-geometry frame**, so each one
//! maps forward through `FrameMap::point`. Unlike the analytic masks, whose coordinates are
//! already frame-relative, that mapping is not optional: without it a cropped or straightened
//! photo draws every spot in the wrong place.

use super::navigation::{tool_drag, tool_drag_started};
use crate::develop::canvas_geom;
use crate::Session;
use eframe::egui;
use pipeline::brush::FrameMap;
use rustroom_app::spot_tool::{self, Handle, SpotTool};

/// Segments in a drawn circle. egui has no ellipse primitive; the radial mask uses the same count.
const CIRCLE_SEGMENTS: usize = 48;

/// A drag shorter than this (in screen points) is a click.
///
/// ⚠ Without it a click with a shaky hand places a spot of radius ~0, which covers nothing and
/// cannot be seen or grabbed again.
const CLICK_SLOP: f32 = 3.0;

/// What the pointer is currently doing.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum SpotDrag {
    None,
    /// Sizing a new spot placed at this normalised point.
    Sizing([f32; 2]),
    /// Choosing a source for a new spot at this normalised point (⌥-drag).
    Sourcing([f32; 2]),
    /// Moving one end of an existing spot.
    Moving(usize, Handle),
}

/// Draw the overlay and run the gestures. Returns `true` when the edits changed.
#[allow(clippy::too_many_arguments)]
pub(super) fn draw_spot_overlay(
    ui: &egui::Ui,
    s: &mut Session,
    resp: &egui::Response,
    img_rect: egui::Rect,
    frame_rect: egui::Rect,
    tool: &mut SpotTool,
    drag: &mut SpotDrag,
    sample: &dyn Fn([f32; 2]) -> [f32; 3],
) -> bool {
    if frame_rect.width() <= f32::EPSILON || frame_rect.height() <= f32::EPSILON {
        return false;
    }
    let aspect = canvas_geom::frame_aspect(frame_rect);
    let map = FrameMap::new(
        &s.edits.geometry,
        frame_rect.width() as usize,
        frame_rect.height() as usize,
    );
    let mut changed = false;

    // A canvas point → the stage's input space, which is where a spot lives.
    let to_source = |p: egui::Pos2| -> [f32; 2] {
        let (nx, ny) = canvas_geom::normalized_point(p, frame_rect);
        map.inverse_point(nx * frame_rect.width(), ny * frame_rect.height())
    };

    // ---- the click gesture ------------------------------------------------------------------
    // ⚠⚠ **A second, click-sensing interaction — the canvas's own `resp` is `Sense::drag()` and
    // never reports `clicked()`.** Making `resp` `click_and_drag` instead also changes when egui
    // promotes a press to a drag, which broke both mask-drag tests outright when the white-balance
    // eyedropper tried it. egui's hit test resolves click- and drag-sensing widgets **separately**,
    // so a click-only widget on top and the drag-sensing canvas beneath each get their own events.
    //
    // ⚠ Registered *after* `resp`, which is what makes it win the click: egui breaks a hit-test
    // tie by registration order, taking the last as the one on top.
    let click = ui
        .interact(img_rect, ui.id().with("retouch_click"), egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::Crosshair);
    if click.clicked() {
        if let Some(p) = click.interact_pointer_pos() {
            let at = to_source(p);
            // ⚠ Hit-test BEFORE placement: a click on an existing handle selects it rather than
            // stacking a new spot on top of it.
            match spot_tool::hit_test(&s.edits.spots, at, aspect) {
                Some((i, _)) => s.selected_spot = Some(i),
                None => {
                    let sp = place_with_source(at, *tool, &s.edits.spots, aspect, sample);
                    s.edits.spots.push(sp);
                    s.selected_spot = Some(s.edits.spots.len() - 1);
                    changed = true;
                }
            }
        }
    }

    // ---- the drag gestures ------------------------------------------------------------------
    if tool_drag_started(resp) {
        if let Some(p) = resp.interact_pointer_pos() {
            let at = to_source(p);
            *drag = match spot_tool::hit_test(&s.edits.spots, at, aspect) {
                Some((i, h)) => {
                    s.selected_spot = Some(i);
                    SpotDrag::Moving(i, h)
                }
                None if ui.input(|i| i.modifiers.alt) => SpotDrag::Sourcing(at),
                None => SpotDrag::Sizing(at),
            };
        }
    }

    if tool_drag(resp) {
        if let SpotDrag::Moving(i, h) = *drag {
            // ⚠ `drag_delta` is in **screen** points and a spot lives in source-normalised units,
            // so the delta is converted by mapping two points through the same inverse the press
            // used. Scaling by `frame_rect` alone would be wrong on a cropped or straightened
            // photo, where the map is not a plain scale.
            if let Some(p) = resp.interact_pointer_pos() {
                let now = to_source(p);
                let was = to_source(p - resp.drag_delta());
                if let Some(sp) = s.edits.spots.get_mut(i) {
                    spot_tool::apply_drag(sp, h, now[0] - was[0], now[1] - was[1]);
                    changed = true;
                }
            }
        }
    }

    if resp.drag_stopped() {
        let released = resp.interact_pointer_pos().map(to_source);
        let placed = match (*drag, released) {
            (SpotDrag::Sizing(centre), Some(to)) => {
                // ⚠⚠ **The gesture writes back into the tool**, which is what makes the Size
                // slider follow the drag. One value, two affordances — the eyedropper-and-Kelvin
                // idiom. Storing the radius only on the spot would leave the slider showing a
                // number that no longer describes what the next click will place.
                //
                // ⚠ A drag shorter than the slop is a click that wandered; keep the stored radius
                // rather than placing a spot of radius ~0 that covers nothing.
                if resp.drag_delta().length() > CLICK_SLOP {
                    tool.radius = spot_tool::drag_radius(centre, to, aspect);
                }
                Some(place_with_source(centre, *tool, &s.edits.spots, aspect, sample))
            }
            (SpotDrag::Sourcing(centre), Some(source)) => {
                Some(spot_tool::place(centre, source, *tool))
            }
            _ => None,
        };
        if let Some(sp) = placed {
            s.edits.spots.push(sp);
            s.selected_spot = Some(s.edits.spots.len() - 1);
            changed = true;
        }
        if *drag != SpotDrag::None {
            // ⚠⚠ A drag MUST request a render. `draw_crop_overlay` deliberately does not, because
            // `preview_edits` strips the crop in geometry mode so the overlay *is* the feedback —
            // copying that here is the exact false analogy that shipped the M3.1a mask bug, where
            // the image did not update until an unrelated slider was touched.
            s.request_current();
            *drag = SpotDrag::None;
        }
    }

    // ---- overlay --------------------------------------------------------------------------
    let painter = ui.painter_with_clip_rect(canvas_geom::overlay_clip(img_rect));
    for (i, sp) in s.edits.spots.iter().enumerate() {
        let selected = s.selected_spot == Some(i);
        let c = to_screen(&map, sp.centre, frame_rect);
        let src = to_screen(&map, sp.source, frame_rect);
        let r = sp.radius * frame_rect.width().max(frame_rect.height()) * map.scale_ratio().0;
        // ⚠ Doubled: a dark halo under a light mark. A thin white circle is invisible against a
        // bright sky — the fix the crop overlay needed after being reported.
        for (w, col) in [
            (3.0, egui::Color32::from_black_alpha(140)),
            (1.0, if selected { egui::Color32::WHITE } else { egui::Color32::from_white_alpha(190) }),
        ] {
            painter.line_segment([c, src], egui::Stroke::new(w, col));
            circle(&painter, c, r, egui::Stroke::new(w, col));
            circle(&painter, src, r, egui::Stroke::new(w, col));
        }
    }

    changed
}

fn to_screen(map: &FrameMap, p: [f32; 2], frame: egui::Rect) -> egui::Pos2 {
    let (x, y) = map.point(p);
    frame.min + egui::vec2(x, y)
}

fn circle(painter: &egui::Painter, c: egui::Pos2, r: f32, stroke: egui::Stroke) {
    let pts: Vec<egui::Pos2> = (0..CIRCLE_SEGMENTS)
        .map(|k| {
            let t = k as f32 / CIRCLE_SEGMENTS as f32 * std::f32::consts::TAU;
            c + egui::vec2(r * t.cos(), r * t.sin())
        })
        .collect();
    painter.add(egui::Shape::closed_line(pts, stroke));
}

/// Place a spot, auto-picking the source and falling back to a fixed offset.
///
/// ⚠ The fallback is **3r straight up**, clamped into the frame: a spot the user can drag beats a
/// click that silently did nothing.
fn place_with_source(
    centre: [f32; 2],
    tool: SpotTool,
    existing: &[edit_model::Spot],
    aspect: f32,
    sample: &dyn Fn([f32; 2]) -> [f32; 3],
) -> edit_model::Spot {
    let source = rustroom_app::spot_source::pick(sample, centre, tool.radius, aspect, existing)
        .unwrap_or_else(|| {
            let up = (centre[1] - 3.0 * tool.radius).clamp(0.0, 1.0);
            let down = (centre[1] + 3.0 * tool.radius).clamp(0.0, 1.0);
            let y = if (up - centre[1]).abs() > (down - centre[1]).abs() { up } else { down };
            [centre[0], y]
        });
    spot_tool::place(centre, source, tool)
}
```

⚠ **Two things this task must verify against the real code rather than assume**, because the plan
cannot see them:
1. `FrameMap` may not expose `point` publicly from `pipeline::brush`, and it has no `scale_ratio`.
   Check with `grep -n "pub fn" crates/pipeline/src/brush/frame_map.rs`. `point` returns
   `(f32, f32)` in **frame pixels**, so `to_screen` is right and `scale_ratio` is **not needed** —
   delete it and derive the radius as `sp.radius * frame_long_edge_in_source_units`. If the maths
   is not obvious from `FrameMap`'s source, compute the screen radius by mapping a second point at
   `centre + radius` along the long axis and taking the distance. Prefer that: it cannot drift from
   the map.
2. `Session` may not have `request_current`. Use whatever the mask overlay calls after a drag —
   `grep -n "request" crates/app/src/develop/canvas/mask_overlay.rs`.

- [ ] **Step 2: Dispatch it from the canvas**

In `crates/app/src/develop/canvas/mod.rs`, add `mod spot_overlay;` and
`use spot_overlay::{draw_spot_overlay, SpotDrag};`, add a `spot_drag: SpotDrag` local beside
`guide_drag`, and replace the placeholder arm:

```rust
                        Tool::Retouch => {
                            photo_edited |= draw_spot_overlay(
                                ui,
                                s,
                                &resp,
                                img_rect,
                                frame_rect,
                                &mut app_spot_tool,
                                &mut spot_drag,
                                &sampler,
                            );
                        }
                        Tool::Edit | Tool::Presets => {}
```

⚠ `app_spot_tool` is **`&mut app.spot_tool`**, not a copy: the size-drag writes the radius back
so the panel's Size slider follows the gesture. `app.spot_tool` and `app.session` are **disjoint
fields**, so both `&mut` borrows are legal — the `masks::BrushTool` precedent, which takes five of
them. Hoist it above the session borrow the way the brush fields are.

The `sampler` closure composes the corrected → raw mapping:

```rust
    // ⚠⚠ `FrameMap::inverse_point` returns POST-lens, POST-perspective coordinates while
    // `s.buffer` is the raw decode. Indexing one with the other is wrong by the distortion —
    // ~3.5% at the corners, ~220 px on a 6246 px frame, larger than a spot radius. The search
    // would sample a different piece of the picture and return a confidently wrong source,
    // silently. Both stages expose the inverse, which is exactly the direction needed.
    let sampler = |p: [f32; 2]| -> [f32; 3] { /* see Step 3 */ };
```

- [ ] **Step 3: Write the sampler**

Add to `crates/app/src/develop/canvas/spot_overlay.rs`:

```rust
/// A sampler over the spot-removal stage's input space, built from the session.
///
/// ⚠ Missing corrections are the identity, not an error: a JPEG has no lens profile and a neutral
/// perspective is the identity homography, so this degrades to a plain buffer read.
pub(super) fn sampler_for(s: &Session) -> impl Fn([f32; 2]) -> [f32; 3] + '_ {
    let (w, h) = (s.buffer.width(), s.buffer.height());
    let persp = pipeline::perspective::Perspective::from_edits(&s.edits.perspective, w as f32 / h as f32);
    let corr = s.lens.shot.as_ref().map(|shot| {
        lens_profile::Corrector::new(shot, w, h)
    });
    move |p: [f32; 2]| {
        // stage input -> perspective input (normalised both sides)
        let (px, py) = persp.source_of((p[0], p[1]));
        // -> lens-correction input, in pixels, per channel for TCA
        let (mut x, mut y) = (px * w as f32, py * h as f32);
        let mut out = [0.0f32; 3];
        if let Some(c) = &corr {
            let per = c.source(x, y);
            for ch in 0..3 {
                out[ch] = read(&s.buffer, per[ch][0], per[ch][1])[ch];
            }
            return out;
        }
        x = x.clamp(0.0, w as f32 - 1.0);
        y = y.clamp(0.0, h as f32 - 1.0);
        read(&s.buffer, x, y)
    }
}

fn read(buf: &core_image::ImageBuf, x: f32, y: f32) -> [f32; 3] {
    let xi = (x.round() as i64).clamp(0, buf.width() as i64 - 1) as usize;
    let yi = (y.round() as i64).clamp(0, buf.height() as i64 - 1) as usize;
    buf.pixel(xi, yi)
}
```

⚠ **Verify these three APIs before compiling**, because the plan cannot:
`Perspective::from_edits`'s real name and signature (`grep -n "pub fn" crates/pipeline/src/perspective/mod.rs`),
`Corrector::new`'s (`grep -n "pub fn new\|pub fn scaled" crates/lens-profile/src/lib.rs`),
and whether `Session` exposes `lens`. Adjust the construction to match; the *composition order* —
perspective inverse, then lens inverse, then read — is the part that must not change.

- [ ] **Step 4: Write the canvas tests**

Append to `crates/app/src/develop/canvas/spot_overlay.rs`:

These go in `crates/app/src/develop/canvas/mod.rs`'s existing `mod tests`, beside the mask-drag
tests, because that is where `test_support` is reachable from.

```rust
    fn retouch_app(ctx: &egui::Context) -> crate::RustroomApp {
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, ctx, 80, 60);
        app.tool = rustroom_app::tool::Tool::Retouch;
        app
    }

    fn spots(app: &crate::RustroomApp) -> &[edit_model::Spot] {
        &app.session.as_ref().unwrap().edits.spots
    }

    /// ⚠⚠ Sampled ON THE DRAG FRAME (`DRAG_PASS`), not after the gesture. A later frame issues a
    /// render anyway, and in the real app that frame never comes because egui idles after a drag —
    /// which is exactly how the M3.1a mask bug hid for weeks.
    #[test]
    fn a_retouch_drag_places_a_sized_spot_and_asks_for_a_render() {
        let ctx = crate::test_ui::ctx();
        let mut app = retouch_app(&ctx);
        let c = test_support::canvas_centre(&mut app, &ctx);
        let counts = test_support::drag_through(&mut app, &ctx, c, c + egui::vec2(30.0, 0.0));
        assert_eq!(spots(&app).len(), 1, "the drag did not place exactly one spot");
        assert!(
            spots(&app)[0].radius > rustroom_app::spot_tool::MIN_RADIUS,
            "the drag did not size the spot: {}",
            spots(&app)[0].radius
        );
        assert!(
            counts[test_support::DRAG_PASS] > counts[test_support::DRAG_PASS - 1],
            "no render was requested on the drag frame: {counts:?}"
        );
    }

    /// ⚠ A click places one spot at the tool's stored radius, and the auto-source must put the
    /// source somewhere else — a source equal to the destination copies the blemish onto itself.
    #[test]
    fn a_retouch_click_places_one_spot_with_a_source_elsewhere() {
        let ctx = crate::test_ui::ctx();
        let mut app = retouch_app(&ctx);
        let c = test_support::canvas_centre(&mut app, &ctx);
        test_support::click_through(&mut app, &ctx, c);
        assert_eq!(spots(&app).len(), 1, "the click did not place exactly one spot");
        let s = spots(&app)[0];
        assert_ne!(s.source, s.centre, "the source sits on the destination");
    }

    /// ⚠ A second click on a placed spot **selects** it rather than stacking another on top —
    /// the hit-test-before-placement rule, and the thing that makes a crowded frame editable.
    #[test]
    fn clicking_a_placed_spot_selects_it_rather_than_stacking() {
        let ctx = crate::test_ui::ctx();
        let mut app = retouch_app(&ctx);
        let c = test_support::canvas_centre(&mut app, &ctx);
        test_support::click_through(&mut app, &ctx, c);
        test_support::click_through(&mut app, &ctx, c);
        assert_eq!(spots(&app).len(), 1, "the second click stacked a spot");
        assert_eq!(app.session.as_ref().unwrap().selected_spot, Some(0));
    }

    /// ⚠⚠ The guard on the guard. Without it, "a spot appeared after a drag" means nothing —
    /// it would pass against a version that placed spots under every tool.
    #[test]
    fn a_drag_under_another_tool_places_nothing() {
        let ctx = crate::test_ui::ctx();
        let mut app = retouch_app(&ctx);
        app.tool = rustroom_app::tool::Tool::Edit;
        let c = test_support::canvas_centre(&mut app, &ctx);
        test_support::drag_through(&mut app, &ctx, c, c + egui::vec2(30.0, 0.0));
        assert!(spots(&app).is_empty(), "Edit placed a retouch spot");
    }
```

⚠ Adjust `spots()` and the `selected_spot` read to whatever visibility the fields actually have —
they are private to `main.rs`, and the canvas test module already reaches `app.session` this way for
the mask tests, so copy that. ⚠ `test_support` may need `pub(super) use` of `DRAG_PASS` and
`click_through` if they are not already visible from `mod.rs`'s tests; they are declared
`pub(super)`, so they are.

- [ ] **Step 5: Run everything**

```bash
cargo fmt --all
cargo clippy --all --all-targets --all-features -- -D warnings; echo "CLIPPY=$?"
cargo test --all 2>&1 | grep -E "test result: FAILED" | wc -l
git status --porcelain crates/pipeline/tests/golden/ | wc -l
```
Expected: clippy 0, zero FAILED lines, zero dirty goldens.

- [ ] **Step 6: Sabotage — `img_rect` for `frame_rect`**

In `draw_spot_overlay`, replace both `frame_rect` uses in the coordinate maths with `img_rect` and
re-run the canvas tests.

Expected: the placement test **FAILS** (the spot lands elsewhere). Restore.

- [ ] **Step 6b: Sabotage — skip the forward map**

In `to_screen`, return `frame.min + egui::vec2(p[0] * frame.width(), p[1] * frame.height())`
(treating the stored pre-geometry point as if it were frame-relative) and in `to_source` return
`[nx, ny]` without `map.inverse_point`. Then run the canvas tests **with a crop applied** — add a
non-default `edits.geometry.crop` to `retouch_app` for this run.

Expected: `a_retouch_click_places_one_spot_with_a_source_elsewhere` or
`clicking_a_placed_spot_selects_it_rather_than_stacking` **FAILS**, because the round trip no longer
lands where it started. Restore.

⚠ If nothing fails, the tests are not exercising a crop and cannot see this class of bug — add a
cropped case before moving on. This is the mapping that makes a spot land where you clicked on a
cropped photo, and it is invisible on an uncropped one.

- [ ] **Step 7: Sabotage — drop the render request**

Delete the `s.request_current();` line and re-run.

Expected: `a_drag_places_a_spot_and_asks_for_a_render` **FAILS**. Restore.

- [ ] **Step 8: Sabotage — a bare `dragged()`**

Change `tool_drag(resp)` to `resp.dragged()` and `tool_drag_started` likewise, then run
`cargo test -p app` in full.

Expected: at least one test **FAILS**. Restore. ⚠ If nothing fails, that is a finding about the
tests, not a formality — add a middle-drag test before moving on.

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "feat(develop): place and edit retouch spots on the canvas

Click places with an auto-picked source; drag sizes; ⌥-drag chooses the source.
Hit-test runs before placement, so a press on an existing handle grabs it
rather than stacking a new spot on top.

⚠ Every coordinate goes against frame_rect, never img_rect — the M3.1a drift
bug, where img_rect covers only the rendered slice once a region render is on.

⚠ Spots are stored pre-geometry and drawn on the post-geometry frame, so each
maps forward through FrameMap::point. Unlike the analytic masks that mapping is
not optional: without it a cropped photo draws every spot in the wrong place.

⚠ A drag requests a render. draw_crop_overlay deliberately does not, because
preview_edits strips the crop in geometry mode so the overlay IS the feedback —
copying that here is the false analogy that shipped the M3.1a mask bug."
```

---

## Task 8: Delete, and the keyboard

**Files:**
- Modify: `crates/app/src/develop/canvas/spot_overlay.rs` or the Develop key handler
  (find it: `grep -rn "Key::Delete\|Key::Backspace" crates/app/src`)

- [ ] **Step 1: Write the failing test**

In `crates/app/src/spot_tool.rs`'s test module:

```rust
    /// ⚠ Deleting clears the selection rather than moving it to a neighbour — `Selection::clamp`'s
    /// rule: silently selecting a *different* spot means the next Delete removes something the
    /// user never chose.
    #[test]
    fn deleting_clears_the_selection() {
        let mut spots = vec![spot(0.2, 0.2, 0.4, 0.4, 0.02), spot(0.6, 0.6, 0.8, 0.8, 0.02)];
        let mut sel = Some(0);
        assert!(delete_selected(&mut spots, &mut sel));
        assert_eq!(spots.len(), 1);
        assert_eq!(sel, None, "a neighbour was selected");
        assert!(!delete_selected(&mut spots, &mut None), "nothing selected, nothing deleted");
    }

    /// ⚠ An out-of-range index must not panic: a snapshot restore can leave one behind.
    #[test]
    fn deleting_a_stale_index_is_a_no_op() {
        let mut spots = vec![spot(0.2, 0.2, 0.4, 0.4, 0.02)];
        let mut sel = Some(9);
        assert!(!delete_selected(&mut spots, &mut sel));
        assert_eq!(spots.len(), 1);
        assert_eq!(sel, None, "the stale index was left in place");
    }
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test -p app --lib spot_tool 2>&1 | grep -E "error\[E0|test result"
```
Expected: FAIL to compile — `cannot find function delete_selected`.

- [ ] **Step 3: Implement**

In `crates/app/src/spot_tool.rs`:

```rust
/// Remove the selected spot. Returns `true` when something was removed.
///
/// ⚠ Clears the selection rather than moving it to a neighbour — `Selection::clamp`'s rule, for
/// its reason: silently selecting a different spot means the next Delete removes one the user
/// never chose.
pub fn delete_selected(spots: &mut Vec<Spot>, selected: &mut Option<usize>) -> bool {
    let Some(i) = *selected else { return false };
    *selected = None;
    if i >= spots.len() {
        return false;
    }
    spots.remove(i);
    true
}
```

- [ ] **Step 4: Wire the keys**

In the Develop key handler, alongside the existing shortcuts, gated on
`ctx.memory(|m| m.focused()).is_none()` (a bare key must not fire while a text field has focus —
the rule every bare key in this app follows) **and** on `app.tool == Tool::Retouch`:

```rust
        if app.tool == Tool::Retouch
            && ctx.memory(|m| m.focused()).is_none()
            && ctx.input(|i| i.key_pressed(egui::Key::Delete) || i.key_pressed(egui::Key::Backspace))
        {
            if let Some(s) = app.session.as_mut() {
                if rustroom_app::spot_tool::delete_selected(&mut s.edits.spots, &mut s.selected_spot) {
                    photo_edited = true;
                }
            }
        }
```

⚠ Match the surrounding code's shape for reporting an edit — find how the existing shortcuts push
a history entry and follow it exactly.

- [ ] **Step 5: Run and commit**

```bash
cargo fmt --all
cargo clippy --all --all-targets --all-features -- -D warnings; echo "CLIPPY=$?"
cargo test --all 2>&1 | grep -c "test result: FAILED"
git add -A
git commit -m "feat(develop): Delete removes the selected spot

⚠ Clears the selection rather than moving it to a neighbour — Selection::clamp's
rule: silently selecting a different spot means the next Delete removes one the
user never chose.

⚠ Gated on no text field having focus, the rule every bare key in this app
follows."
```

---

## Task 9: Verify the whole thing by hand

- [ ] **Step 1: Full green**

```bash
cargo fmt --all --check; echo "FMT=$?"
cargo clippy --all --all-targets --all-features -- -D warnings; echo "CLIPPY=$?"
cargo test --all 2>&1 | grep -E "test result:" | awk -F'[ ;]' '{p+=$4; f+=$6} END {print "passed="p" failed="f}'
git status --porcelain crates/pipeline/tests/golden/ | wc -l
```
Expected: FMT=0, CLIPPY=0, failed=0, goldens dirty=0.

- [ ] **Step 2: Run the GUI and check it by eye**

```bash
cargo run --release -p app
```
Open a RAW, press `4` for Retouch, then check:
1. A click places a circle pair with a line between them, and the render updates.
2. Dragging out from a point sizes the patch, and the panel's Size slider moves with it.
3. ⌥-dragging puts the source where you released.
4. Clicking a placed spot selects it (brighter) and dragging either circle moves that end.
5. Delete removes it; Clear all empties the panel count.
6. **Crop the photo, then place a spot** — it must land where you clicked, which is what
   `FrameMap::point` buys.
7. Switch photos and back: the spots persist (they are in `Edits`), the selection does not.

- [ ] **Step 3: Report**

Report each of the seven checks as pass or fail with what was seen. Do **not** claim the feature
works without having run it — this project has recorded eleven separate occasions where every
canvas defect was found by using the app and none by a test.
