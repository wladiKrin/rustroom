# Rotate and flip — implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Quarter turns and mirroring, so a frame shot in the wrong orientation can be fixed — plus renaming the Crop mode to Geometry, which is what it has become.

**Architecture:** `Orientation { rotate: Quarter, mirror: bool }` on `Geometry`, applied first inside `GeometryStage` as an index permutation. Composition lives in pure methods with a property test. Rotating also transforms the stored `CropRect` so a crop keeps its content.

**Tech Stack:** Rust, `serde`, `egui`, `cargo test` / `cargo clippy --all-targets --all-features` / `cargo fmt`.

**Spec:** `docs/superpowers/specs/2026-08-13-orientation-design.md`

**Build setup — every `cargo` command below assumes this has been exported:**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

---

## File structure

| File | Responsibility | Change |
|---|---|---|
| `crates/edit-model/src/lib.rs` | `Quarter`, `Orientation`, composition, `CropRect` transforms | Modify |
| `crates/pipeline/src/stages/geometry.rs` | apply the permutation before straighten | Modify |
| `crates/pipeline/src/lib.rs` | `all_edits()` fixture gains a non-neutral orientation | Modify |
| `crates/pipeline/tests/golden.rs` | two new goldens | Modify |
| `crates/app/src/main.rs` | four buttons, Crop → Geometry rename | Modify |
| `CLAUDE.md` | durable status | Modify |

---

## Task 1: `Orientation` and its composition

The whole feature's correctness risk is here, so it lands alone, with the property test.

**Files:**
- Modify: `crates/edit-model/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Add inside `mod tests` in `crates/edit-model/src/lib.rs`:

```rust
    /// Every state reachable from the identity by the four buttons.
    fn reachable() -> std::collections::BTreeSet<(u8, bool)> {
        let key = |o: &Orientation| (o.rotate as u8, o.mirror);
        let mut seen = std::collections::BTreeSet::new();
        let mut frontier = vec![Orientation::default()];
        seen.insert(key(&Orientation::default()));
        // ⚠ The annotation is required: each `fn` item has its own distinct type, so an
        // unannotated array of them does not infer a common element type.
        let acts: [fn(&mut Orientation); 4] = [
            Orientation::rotate_cw,
            Orientation::rotate_ccw,
            Orientation::flip_h,
            Orientation::flip_v,
        ];
        while let Some(o) = frontier.pop() {
            for act in acts {
                let mut next = o;
                act(&mut next);
                if seen.insert(key(&next)) {
                    frontier.push(next);
                }
            }
        }
        seen
    }

    #[test]
    fn the_generators_produce_exactly_eight_states() {
        // ⚠ The property that catches a wrong composition rule. A rectangle has exactly eight
        // rigid transforms; a rule that composes wrongly collapses the reachable set (states
        // merge) or inflates it (an unreachable eighth appears twice under different names).
        // A table of hand-written cases would probably encode the same mistake twice.
        assert_eq!(reachable().len(), 8);
    }

    #[test]
    fn each_generator_returns_to_identity() {
        let mut o = Orientation::default();
        for _ in 0..4 {
            o.rotate_cw();
        }
        assert_eq!(o, Orientation::default());

        let mut o = Orientation::default();
        for _ in 0..4 {
            o.rotate_ccw();
        }
        assert_eq!(o, Orientation::default());

        let flips: [fn(&mut Orientation); 2] = [Orientation::flip_h, Orientation::flip_v];
        for act in flips {
            let mut o = Orientation::default();
            act(&mut o);
            assert_ne!(o, Orientation::default(), "a flip must change something");
            act(&mut o);
            assert_eq!(o, Orientation::default());
        }
    }

    #[test]
    fn flipping_after_a_rotation_negates_it() {
        // ⚠ The non-abelian rule, stated directly so a failure names itself rather than only
        // showing up as a wrong state count. `M ∘ rot_r = rot_{-r} ∘ M`.
        let mut o = Orientation::default();
        o.rotate_cw(); // 90
        o.flip_h();
        assert_eq!(o.rotate, Quarter::Cw270, "90 must become 270 under a flip");
        assert!(o.mirror);
    }

    #[test]
    fn a_vertical_flip_is_a_horizontal_flip_and_a_half_turn() {
        // Keeps the UI's four buttons honest against the two stored fields.
        for start in [Quarter::Zero, Quarter::Cw90, Quarter::Cw180, Quarter::Cw270] {
            for mirror in [false, true] {
                let base = Orientation {
                    rotate: start,
                    mirror,
                };
                let mut via_v = base;
                via_v.flip_v();

                let mut via_h = base;
                via_h.flip_h();
                via_h.rotate_cw();
                via_h.rotate_cw();

                assert_eq!(via_v, via_h, "from {base:?}");
            }
        }
    }

    #[test]
    fn a_clockwise_turn_moves_the_left_half_to_the_top() {
        // ⚠ The crop must follow the rotation, or rotating a cropped photo lands the crop on
        // entirely different content. Checked by *content*, not by arithmetic.
        let left_half = CropRect {
            left: 0.0,
            top: 0.0,
            right: 0.5,
            bottom: 1.0,
        };
        let top_half = CropRect {
            left: 0.0,
            top: 0.0,
            right: 1.0,
            bottom: 0.5,
        };
        assert_eq!(left_half.rotated_cw(), top_half);
    }

    #[test]
    fn four_crop_rotations_restore_the_rect() {
        let r = CropRect {
            left: 0.1,
            top: 0.2,
            right: 0.7,
            bottom: 0.9,
        };
        let mut got = r;
        for _ in 0..4 {
            got = got.rotated_cw();
        }
        assert_eq!(got, r);

        // And a horizontal flip is its own inverse.
        assert_eq!(r.flipped_h().flipped_h(), r);
    }
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p edit-model the_generators 2>&1 | grep -E "^error" | head -3`

Expected: FAIL to compile — `cannot find type `Orientation` in this scope`.

- [ ] **Step 3: Implement**

In `crates/edit-model/src/lib.rs`, add above `pub struct Geometry`:

```rust
/// A quarter turn, clockwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Quarter {
    #[default]
    Zero,
    Cw90,
    Cw180,
    Cw270,
}

impl Quarter {
    /// Quarter turns clockwise, 0..=3 — the arithmetic form, so composition is modular
    /// addition rather than a match table nobody can check.
    fn turns(self) -> u8 {
        match self {
            Quarter::Zero => 0,
            Quarter::Cw90 => 1,
            Quarter::Cw180 => 2,
            Quarter::Cw270 => 3,
        }
    }

    fn from_turns(t: u8) -> Self {
        match t % 4 {
            0 => Quarter::Zero,
            1 => Quarter::Cw90,
            2 => Quarter::Cw180,
            _ => Quarter::Cw270,
        }
    }

    /// Whether this turn swaps width and height.
    pub fn swaps_axes(self) -> bool {
        matches!(self, Quarter::Cw90 | Quarter::Cw270)
    }
}

/// One of the eight rigid transforms of a rectangle: mirror about the vertical axis if
/// `mirror`, **then** turn by `rotate`.
///
/// ⚠ `mirror` is one flag, not two. A vertical flip is a horizontal flip plus a half turn, so
/// storing `flip_h` and `flip_v` separately would make four of the eight states representable
/// two ways. Flip-vertical is a derived *action* — see [`Orientation::flip_v`] — not a field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Orientation {
    pub rotate: Quarter,
    pub mirror: bool,
}

impl Orientation {
    pub fn is_identity(self) -> bool {
        self == Orientation::default()
    }

    pub fn rotate_cw(&mut self) {
        self.rotate = Quarter::from_turns(self.rotate.turns() + 1);
    }

    pub fn rotate_ccw(&mut self) {
        self.rotate = Quarter::from_turns(self.rotate.turns() + 3);
    }

    /// ⚠ Not just `mirror = !mirror`. The group is non-abelian: with `mirror` applied *first*,
    /// post-composing a horizontal flip gives `M ∘ rot_r = rot_{-r} ∘ M`, so the rotation
    /// negates too. Without this the buttons are each correct in isolation and wrong in
    /// sequence — see `the_generators_produce_exactly_eight_states`.
    pub fn flip_h(&mut self) {
        self.rotate = Quarter::from_turns(4 - self.rotate.turns());
        self.mirror = !self.mirror;
    }

    /// A horizontal flip and a half turn. Derived rather than stored, per the type's note.
    pub fn flip_v(&mut self) {
        self.flip_h();
        self.rotate = Quarter::from_turns(self.rotate.turns() + 2);
    }
}
```

Add to `Geometry`:

```rust
    /// Quarter turns and mirroring, applied before the straighten angle.
    #[serde(default)]
    pub orientation: Orientation,
```

And add to `impl CropRect`:

```rust
    /// The rect covering the same content after the frame turns clockwise.
    ///
    /// ⚠ Without this, rotating a cropped photo leaves the crop on entirely different pixels.
    /// In the unit square a clockwise turn maps `(x, y) -> (1 - y, x)`.
    pub fn rotated_cw(self) -> Self {
        Self {
            left: 1.0 - self.bottom,
            top: self.left,
            right: 1.0 - self.top,
            bottom: self.right,
        }
    }

    /// The rect covering the same content after the frame mirrors horizontally.
    pub fn flipped_h(self) -> Self {
        Self {
            left: 1.0 - self.right,
            top: self.top,
            right: 1.0 - self.left,
            bottom: self.bottom,
        }
    }
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p edit-model 2>&1 | grep -E "^test result|FAILED" | head -3`

Expected: PASS, including every existing `edit-model` test — `Geometry::default()` must still
be neutral, or `build_prefix_steps` would insert a geometry stage for every photo.

- [ ] **Step 5: Verify the property test can fail**

Temporarily break the composition rule: in `flip_h`, replace the rotation line with
`self.rotate = self.rotate;` (i.e. flip the mirror only).

Run: `cargo test -p edit-model the_generators 2>&1 | grep -E "left|right|^test result" | head -3`

Expected: FAIL — the reachable set is no longer 8. Revert and re-run to confirm PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/edit-model/src/lib.rs
git commit -m "feat(edit-model): orientation as quarter turns plus a mirror"
```

---

## Task 2: Apply it in `GeometryStage`

**Files:**
- Modify: `crates/pipeline/src/stages/geometry.rs`
- Modify: `crates/pipeline/src/lib.rs` (`all_edits` fixture)

- [ ] **Step 1: Write the failing tests**

Add a `mod tests` at the end of `crates/pipeline/src/stages/geometry.rs` (or extend the
existing one):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use edit_model::{Orientation, Quarter};

    /// A 3x2 image whose every pixel is distinguishable, so a permutation is checked exactly
    /// rather than by a summary statistic.
    fn ramp() -> ImageBuf {
        let mut img = ImageBuf::new(3, 2);
        for y in 0..2 {
            for x in 0..3 {
                img.set_pixel(x, y, [x as f32, y as f32, 0.0]);
            }
        }
        img
    }

    fn oriented(img: ImageBuf, rotate: Quarter, mirror: bool) -> ImageBuf {
        let g = Geometry {
            orientation: Orientation { rotate, mirror },
            ..Default::default()
        };
        GeometryStage::new(&g).apply(img)
    }

    #[test]
    fn orienting_is_an_index_permutation() {
        // Quarter turns swap the axes; every source pixel appears exactly once, unresampled.
        let out = oriented(ramp(), Quarter::Cw90, false);
        assert_eq!((out.width(), out.height()), (2, 3));
        // Clockwise: the source's bottom-left (0,1) becomes the output's top-left.
        assert_eq!(out.pixel(0, 0), [0.0, 1.0, 0.0]);
        assert_eq!(out.pixel(1, 0), [0.0, 0.0, 0.0]);
        assert_eq!(out.pixel(1, 2), [2.0, 0.0, 0.0]);

        let out = oriented(ramp(), Quarter::Zero, true);
        assert_eq!((out.width(), out.height()), (3, 2));
        assert_eq!(out.pixel(0, 0), [2.0, 0.0, 0.0], "mirrored about x");
        assert_eq!(out.pixel(2, 0), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn orienting_twice_by_180_is_the_identity() {
        let src = ramp();
        let once = oriented(src.clone(), Quarter::Cw180, false);
        let twice = oriented(once, Quarter::Cw180, false);
        assert_eq!(twice.data(), src.data(), "bit-identical, no resampling");
    }

    #[test]
    fn the_identity_orientation_changes_nothing() {
        // What keeps all 29 goldens byte-identical.
        let src = ramp();
        let out = oriented(src.clone(), Quarter::Zero, false);
        assert_eq!(out.data(), src.data());
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p pipeline orienting_is_an_index 2>&1 | grep -E "^error" | head -3`

Expected: FAIL to compile — `Geometry` has no field `orientation` in this crate's view until
Task 1 is in, and `GeometryStage` does not apply it.

- [ ] **Step 3: Implement**

In `crates/pipeline/src/stages/geometry.rs`, add the field and the transform:

```rust
pub struct GeometryStage {
    orientation: Orientation,
    angle_rad: f32,
    crop: CropRect,
}
```

with `orientation: g.orientation` in `new`, and in `apply`:

```rust
    fn apply(&self, image: ImageBuf) -> ImageBuf {
        // Orientation first: a lossless whole-image reframe, so the straighten angle and the
        // crop are both expressed in the oriented frame.
        let image = if self.orientation.is_identity() {
            image
        } else {
            orient(&image, self.orientation)
        };
        let rotated = if self.angle_rad != 0.0 {
            rotate_bilinear(&image, self.angle_rad)
        } else {
            image
        };
        apply_crop(rotated, &self.crop)
    }
```

And the permutation itself:

```rust
/// Apply a quarter turn and optional mirror. An index permutation — no resampling, so this is
/// lossless and exact.
///
/// Follows `Orientation`'s convention: mirror about the vertical axis first, then turn.
fn orient(src: &ImageBuf, o: Orientation) -> ImageBuf {
    let (w, h) = (src.width(), src.height());
    let (ow, oh) = if o.rotate.swaps_axes() { (h, w) } else { (w, h) };
    let mut out = ImageBuf::new(ow, oh);
    for oy in 0..oh {
        for ox in 0..ow {
            // Inverse map: where in the source does this output pixel come from?
            let (mut sx, sy) = match o.rotate {
                Quarter::Zero => (ox, oy),
                Quarter::Cw90 => (oy, h - 1 - ox),
                Quarter::Cw180 => (w - 1 - ox, h - 1 - oy),
                Quarter::Cw270 => (w - 1 - oy, ox),
            };
            // Un-mirror in *source* space, after un-rotating — the inverse of "mirror first,
            // then turn", so `w` is the source width whatever the turn.
            if o.mirror {
                sx = w - 1 - sx;
            }
            out.set_pixel(ox, oy, src.pixel(sx, sy));
        }
    }
    out
}
```

⚠ Note the inverse mapping: `Cw90` reads the source at `(oy, h - 1 - ox)`. Deriving it forwards
and inverting it by hand is where a transposed-instead-of-rotated bug comes from, which is what
`orienting_is_an_index_permutation` checks by naming specific pixels.

Update the imports at the top of the file to include `Orientation` and `Quarter`.

- [ ] **Step 4: Make the fixture non-neutral**

In `crates/pipeline/src/lib.rs`, `all_edits()`, give the geometry a non-identity orientation so
the split and prefix-key tests exercise it:

```rust
            geometry: edit_model::Geometry {
                angle: 2.0,
                orientation: edit_model::Orientation {
                    rotate: edit_model::Quarter::Cw90,
                    mirror: true,
                },
                ..Default::default()
            },
```

- [ ] **Step 5: Run everything**

Run: `cargo test --all 2>&1 | grep -E "^error|FAILED" | head; cargo test -p pipeline --test golden 2>&1 | tail -2`

Expected: all pass, and **the 29 goldens byte-identical** — the stage is inert at its default,
which is the proof this change is additive.

- [ ] **Step 6: Commit**

```bash
git add crates/pipeline/src/stages/geometry.rs crates/pipeline/src/lib.rs
git commit -m "feat(pipeline): apply orientation before straighten and crop"
```

---

## Task 3: Two goldens

**Files:**
- Modify: `crates/pipeline/tests/golden.rs`

- [ ] **Step 1: Add the cases**

Add to `crates/pipeline/tests/golden.rs`, following the existing pattern:

```rust
#[test]
fn golden_orient_cw90() {
    check_golden(
        "orient_cw90",
        &Edits {
            geometry: edit_model::Geometry {
                orientation: edit_model::Orientation {
                    rotate: edit_model::Quarter::Cw90,
                    mirror: false,
                },
                ..Default::default()
            },
            ..Edits::default()
        },
    );
}

#[test]
fn golden_orient_mirror() {
    check_golden(
        "orient_mirror",
        &Edits {
            geometry: edit_model::Geometry {
                orientation: edit_model::Orientation {
                    rotate: edit_model::Quarter::Cw180,
                    mirror: true,
                },
                ..Default::default()
            },
            ..Edits::default()
        },
    );
}
```

- [ ] **Step 2: Generate and *look at* the references**

Run: `UPDATE_GOLDEN=1 cargo test -p pipeline --test golden golden_orient 2>&1 | tail -3`

Then open both new PNGs and confirm by eye that one is turned a quarter clockwise and the other
is turned upside down *and* mirrored. ⚠ A golden nobody has looked at is just a hash — this is
the step that caught the noise-chart bleeding when NR landed.

- [ ] **Step 3: Confirm the other 29 did not move**

Run: `git status --short crates/pipeline/tests/golden/`

Expected: exactly two new files, no modifications.

- [ ] **Step 4: Commit**

```bash
git add crates/pipeline/tests/golden.rs crates/pipeline/tests/golden/
git commit -m "test(pipeline): goldens for a quarter turn and a mirror"
```

---

## Task 4: The Geometry panel

**Files:**
- Modify: `crates/app/src/main.rs`

- [ ] **Step 1: Rename Crop to Geometry**

Mechanical, 15 sites. The field, the parameter, the toggle label and the section header:

```bash
# The field and every use of it.
perl -pi -e 's/\bcrop_mode\b/geometry_mode/g' crates/app/src/main.rs
```

Then the two user-facing strings by hand: the toggle at what was line 1135
(`ui.toggle_value(&mut s.geometry_mode, "Crop")` → `"Geometry"`) and the section header at what
was line 1993 (`ui.label("Crop")` → `ui.label("Geometry")`).

⚠ `preview_edits`'s parameter renames with it, and its **behaviour must not change**: it keeps
resetting the crop so the whole frame shows under the overlay, and must **not** reset the
orientation — resetting it would show a differently-oriented preview from the one being edited.
Update its doc comment to say so.

- [ ] **Step 2: Add the buttons**

Beside the Straighten slider (search for `.text("Straighten")`), add above it:

```rust
                            ui.horizontal(|ui| {
                                let mut turned = false;
                                if ui.button("⟲").on_hover_text("Rotate left").clicked() {
                                    s.edits.geometry.orientation.rotate_ccw();
                                    s.edits.geometry.crop = s.edits.geometry.crop.rotated_cw();
                                    s.edits.geometry.crop = s.edits.geometry.crop.rotated_cw();
                                    s.edits.geometry.crop = s.edits.geometry.crop.rotated_cw();
                                    turned = true;
                                }
                                if ui.button("⟳").on_hover_text("Rotate right").clicked() {
                                    s.edits.geometry.orientation.rotate_cw();
                                    s.edits.geometry.crop = s.edits.geometry.crop.rotated_cw();
                                    turned = true;
                                }
                                if ui.button("⇄").on_hover_text("Flip horizontal").clicked() {
                                    s.edits.geometry.orientation.flip_h();
                                    s.edits.geometry.crop = s.edits.geometry.crop.flipped_h();
                                    turned = true;
                                }
                                if ui.button("⇅").on_hover_text("Flip vertical").clicked() {
                                    s.edits.geometry.orientation.flip_v();
                                    s.edits.geometry.crop = s.edits.geometry.crop.flipped_h();
                                    s.edits.geometry.crop = s.edits.geometry.crop.rotated_cw();
                                    s.edits.geometry.crop = s.edits.geometry.crop.rotated_cw();
                                    turned = true;
                                }
                                if turned {
                                    changed = true;
                                    settled = true;
                                }
                            });
```

⚠ `settled = true` as well as `changed`: a button press is a discrete act, so it is **one**
`history.push` and one Ctrl+Z — unlike a slider, which pushes only on `drag_stopped`.

⚠ The crop transform mirrors the orientation action exactly, three clockwise turns standing in
for a counter-clockwise one. If that repetition grates, give `CropRect` a `rotated_ccw` — but do
not let the two fall out of step, or a rotation will move the crop off its content.

- [ ] **Step 3: Build and check**

Run: `cargo fmt --all && cargo clippy --all-targets --all-features 2>&1 | grep -E "^warning|^error" | head -10`

Expected: clean.

- [ ] **Step 4: Full suite**

Run: `cargo test --all 2>&1 | grep -E "^error|FAILED" | head; cargo test --all 2>&1 | grep -c "test result: ok"`

Expected: no failures, 21 groups.

- [ ] **Step 5: Commit**

```bash
git add crates/app/src/main.rs
git commit -m "feat(app): rotate and flip buttons, and Crop becomes Geometry"
```

---

## Task 5: Verify and document

- [ ] **Step 1: Launch**

Run: `cargo run --release -p app`

- [ ] **Step 2: Check by hand**

Open a RAW, enter **Geometry** mode.

1. **Rotate right four times** — back where it started, and the aspect flips between landscape
   and portrait on the way.
2. **Rotate right, then flip horizontal, then rotate right** — the non-abelian case. It should
   behave like a physical print: no surprises, no double-mirroring.
3. **Flip vertical twice** — identity.
4. **Crop to a distinctive corner, leave Geometry mode, re-enter, rotate right** — the crop
   should still be on the *same content*, now in a different corner of the frame.
5. **Ctrl+Z after each button** — one undo per press.
6. **Rotate, then export** — the exported JPEG carries the rotation.
7. **Rotate, switch photo, come back** — the sidecar round-trips it.
8. **Open a photo edited before today** — unchanged, identity orientation.

- [ ] **Step 3: Fix or continue**

If anything misbehaves, reproduce it with a test before fixing.

- [ ] **Step 4: Update `CLAUDE.md`**

Add a feature section in the house style covering:

- ⚠ Two fields for eight states, and why `flip_v` is a derived action rather than a stored flag.
- ⚠ The non-abelian composition rule, and that it is guarded by a **property** test (exactly 8
  reachable states) rather than a case table that would encode the same mistake twice.
- ⚠ The crop following the rotation, and what breaks without it.
- That the dimension swap needed no work elsewhere, with the four reasons from the spec's §5 —
  worth recording because it is the kind of thing that looks like it should have.
- The Crop → Geometry rename, and that `preview_edits` resets the crop but *not* the orientation.
- Orientation living inside `GeometryStage` rather than earning its own checkpoint.
- Goldens 29 → 31, with the 29 byte-identical.
- Accepted caveats from the spec.

Update the `edit-model` crate-layout line, and add both docs to the index.

- [ ] **Step 5: Final check and commit**

Run: `cargo fmt --all --check && cargo clippy --all-targets --all-features 2>&1 | grep -E "^warning|^error" | head; cargo test --all 2>&1 | grep -E "^error|FAILED" | head`

```bash
git add CLAUDE.md
git commit -m "docs: record rotate and flip"
```
