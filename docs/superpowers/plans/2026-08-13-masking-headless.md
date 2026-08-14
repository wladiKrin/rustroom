# Masking Architecture — Headless Half (M3.1a, plan A of 2)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A linear-gradient mask carrying per-pixel adjustments renders correctly — authored as a JSON sidecar, verified by goldens and the CLI, with no GUI involved.

**Architecture:** `edit-model` gains the mask types and the coverage definition; `pipeline` gains a `MaskStage` that runs ordinary per-pixel stages on a copy and blends the result by coverage. Every existing stage is reused unchanged.

**Tech Stack:** Rust. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-08-13-masking-architecture-design.md`

**Build note:** every `cargo` command needs libraw and lcms2 on the pkg-config path. Export once per shell:

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

**Commit messages carry no `Co-Authored-By` trailer** (explicit user preference). Never `git push` — the user pushes themselves.

---

## Why this is plan A of two

The spec's §8 says build headless first, and that follows from rules already in force: `pipeline` has **zero GUI dependency** by the crate-layout rules, and the sequencing notes call the CLI "our permanent test harness — it makes the engine testable without the GUI".

So this plan ends with a masked edit **rendering correctly from a hand-written sidecar**. That is working, verifiable software. **Plan B — the canvas UI** is written after this lands, when the coverage signature and the history integration are facts rather than guesses, and when the mask list's provisional placement can be judged against something real.

It also means a canvas bug can never be confused with a model bug — the confusion that cost four diagnostic rounds during the progressive-sharpen work.

## File Structure

| File | Responsibility | Task |
|---|---|---|
| `crates/edit-model/src/lib.rs` | The mask types, the **coverage definition**, `Edits.masks`, the seventh `EditGroups` group, the `merge_from` arm. | 1, 3 |
| `crates/pipeline/src/lib.rs` | `per_pixel_stages` extracted (task 2), `MaskStage` wired into the tail (task 4). | 2, 4 |
| `crates/pipeline/src/stages/mask.rs` | **New.** `MaskStage`: run sub-stages on a copy, blend by coverage, region-aware. | 4 |
| `crates/pipeline/tests/golden.rs` | Two new golden cases. | 4 |

### ⚠ What this deliberately does *not* touch

- **No `PrefixStepKey` variant.** Masks live in the **tail**, so they are outside the prefix
  cache key entirely. A mask in the prefix would invalidate the noise-reduction checkpoint on
  every mouse move.
- **No change to `tail_kernel_radius`.** `MaskStage` reads only the pixel under consideration,
  so it adds no kernel and `view_region::REGION_MARGIN` needs no headroom for it. ⚠ That
  changes at 3.1c: a brush mask is still per-pixel, but if a future mask type ever blurs its
  own coverage, this is the line to revisit.

### ⚠ Two traps that do *not* apply here, and one that does

- **No dead-code trap.** `pipeline` and `edit-model` are **library** crates, so `pub` escapes and an item without an internal caller is fine. That trap is specific to the binary crates (`app`, `rustroom`) and has bitten three times this session — it does not apply to these two.
- **⚠ The orphan rule does apply.** `LinearGradient` is defined in `edit-model`, so `pipeline` cannot write an inherent `impl` for it. Coverage therefore lives in `edit-model` — which is also where it belongs on the merits: `CropRect::rotated_cw`, `CropRect::flipped_h` and `Orientation::flip_v` are already pure geometry on the edit types, so there is precedent, and coverage is a *definition of what the mask means*, not a rendering strategy.

---

## Task 1: The mask types in `edit-model`

**Files:**
- Modify: `crates/edit-model/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Add to `edit-model`'s `mod tests`:

```rust
    #[test]
    fn a_sidecar_from_before_masks_still_loads() {
        // `Edits` carries `#[serde(default)]`, so an older sidecar must load with no masks
        // rather than failing — the same forgiving-load property every added field needs.
        let e: Edits = serde_json::from_str(r#"{"exposure": 0.5}"#).unwrap();
        assert_eq!(e.exposure, 0.5);
        assert!(e.masks.is_empty());
    }

    #[test]
    fn a_masked_adjustment_round_trips_through_json() {
        let e = Edits {
            masks: vec![MaskedAdjustment {
                mask: Mask::Linear(LinearGradient {
                    from: [0.1, 0.2],
                    to: [0.8, 0.9],
                    invert: true,
                }),
                adjustments: LocalAdjustments {
                    exposure: -1.5,
                    ..Default::default()
                },
            }],
            ..Edits::default()
        };
        let text = serde_json::to_string(&e).unwrap();
        let back: Edits = serde_json::from_str(&text).unwrap();
        assert_eq!(back, e);
    }

    #[test]
    fn masks_do_not_transfer_by_default() {
        // ⚠ The same treatment geometry gets, for the same reason: a gradient positioned for
        // one composition is meaningless on another, so carrying it across photos by default
        // would be wrong more often than right.
        assert!(!EditGroups::default().masks);
        assert!(EditGroups::ALL.masks, "but it must be reachable when asked for");
    }

    #[test]
    fn the_masks_group_transfers_masks_and_nothing_else() {
        let src = all_nonneutral();
        let mut dst = Edits::default();
        dst.merge_from(&src, EditGroups::only(6));
        assert_eq!(dst.masks, src.masks);
        assert_eq!(dst.exposure, 0.0, "only the masks group was selected");
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p edit-model
```

Expected: **compile error**, `cannot find type MaskedAdjustment in this scope`, plus `no field masks on type Edits` and `no field masks on type EditGroups`.

- [ ] **Step 3: Write the types**

In `crates/edit-model/src/lib.rs`, above `struct Edits`:

```rust
/// A graduated filter. Coverage ramps from 0 at `from` to 1 at `to`, measured along the axis
/// between them and constant on lines perpendicular to it.
///
/// Endpoints are in **normalised frame coordinates**, as `CropRect` is — so a mask means the
/// same thing on the 1600 px preview and on the full-resolution export.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct LinearGradient {
    pub from: [f32; 2],
    pub to: [f32; 2],
    pub invert: bool,
}

/// The mask primitives. 3.1b adds `Radial`, 3.1c adds `Brush`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Mask {
    Linear(LinearGradient),
}

impl Default for Mask {
    fn default() -> Self {
        Mask::Linear(LinearGradient::default())
    }
}

/// What a mask can carry: the **per-pixel** adjustments, and only those.
///
/// ⚠ Not an arbitrary subset. It is exactly the set where "apply the stage, then blend the
/// result by coverage" is both correct and cheap, because each output pixel depends only on
/// its own input pixel. A spatial stage — noise reduction, sharpening, clarity, dehaze, HSL —
/// reads neighbours *outside* the mask, so "masked" needs a separate decision for each and is
/// out of scope here.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct LocalAdjustments {
    pub white_balance: WhiteBalance,
    pub exposure: f32,
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub whites: f32,
    pub blacks: f32,
    pub saturation: f32,
    pub vibrance: f32,
}

impl LocalAdjustments {
    /// Whether this would change any pixel. An inactive set inserts no stages at all.
    pub fn is_active(&self) -> bool {
        *self != Self::default()
    }
}

/// One mask and the adjustments it carries.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct MaskedAdjustment {
    pub mask: Mask,
    pub adjustments: LocalAdjustments,
}
```

- [ ] **Step 4: Add the field and the group**

In `struct Edits`, after `geometry`:

```rust
    /// Local adjustments, each confined to a mask. Applied in order, after the global
    /// per-pixel adjustments and before the vignette.
    pub masks: Vec<MaskedAdjustment>,
```

⚠ `Edits` derives `Default` and carries `#[serde(default)]`, so an empty `Vec` needs no further work — but `Edits` is no longer `Copy`-friendly in the way a flat struct was. It was never `Copy`; check nothing assumed otherwise when the compiler complains.

In `struct EditGroups`, after `geometry`:

```rust
    /// Masked local adjustments.
    pub masks: bool,
```

Extend `EDIT_GROUP_NAMES` to seven, appending `"Masks"`, and extend `get`/`get_mut`'s match arms with `6 => …masks`. In `EditGroups::ALL` set `masks: true`; in `Default` leave it **false**, beside `geometry`.

- [ ] **Step 5: Add the `merge_from` arm**

`Edits::merge_from` destructures `Edits` exhaustively, so it will not compile until `masks` is assigned a group. Add, beside the geometry arm:

```rust
        if groups.masks {
            self.masks = src.masks.clone();
        }
```

⚠ The exhaustive destructure is why this is a compile error rather than a silently untransferred field. That is what it is for.

- [ ] **Step 6: Extend the exhaustive test fixture**

`all_nonneutral()` in `edit-model`'s tests lists every field explicitly so that adding one breaks compilation. Add:

```rust
        masks: vec![MaskedAdjustment {
            mask: Mask::Linear(LinearGradient {
                from: [0.0, 0.0],
                to: [1.0, 1.0],
                invert: false,
            }),
            adjustments: LocalAdjustments {
                exposure: 0.75,
                ..Default::default()
            },
        }],
```

- [ ] **Step 7: Run the tests to verify they pass**

```bash
cargo test -p edit-model
```

Expected: all pass, including the four new ones and the existing generic partition tests, which now cover seven groups without being edited.

- [ ] **Step 8: Run the full check suite**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
```

Expected: no warnings. ⚠ `pipeline::all_edits()` also lists every field explicitly and **will fail to compile** — add the same `masks` value there. Likewise `app::edit_clipboard`'s payload test if it constructs an exhaustive `Edits`.

- [ ] **Step 9: Commit**

```bash
git add crates/edit-model/src/lib.rs crates/pipeline/src/lib.rs crates/app/src/edit_clipboard.rs
git commit -m "feat(edit-model): mask types and a masks edit group"
```

---

## Task 2: Extract `per_pixel_stages` — a pure refactor

Nothing observable changes. The goldens are the proof.

**Files:**
- Modify: `crates/pipeline/src/lib.rs`

- [ ] **Step 1: Add the shared builder**

In `crates/pipeline/src/lib.rs`, above `build_pipeline_split_in_region`:

```rust
/// Build the per-pixel stages for one set of adjustments.
///
/// ⚠ **One list, two consumers**: the global tail calls it with the values from `Edits`, and
/// every mask calls it with its own `LocalAdjustments`. A stage added here reaches both — the
/// same guard `build_prefix_steps` provides for the prefix, and the reason `LocalAdjustments`
/// repeating field *names* from `Edits` is harmless: the duplication that would actually hurt
/// is this logic, and it exists once.
fn per_pixel_stages(a: &edit_model::LocalAdjustments) -> Vec<Box<dyn PipelineStage>> {
    let mut out: Vec<Box<dyn PipelineStage>> = Vec::new();
    if a.white_balance != edit_model::WhiteBalance::default() {
        out.push(Box::new(WhiteBalanceStage::new(&a.white_balance)));
    }
    if a.exposure != 0.0 {
        out.push(Box::new(ExposureStage::new(a.exposure)));
    }
    if a.contrast != 0.0 {
        out.push(Box::new(ContrastStage::new(a.contrast)));
    }
    if a.highlights != 0.0 || a.shadows != 0.0 || a.whites != 0.0 || a.blacks != 0.0 {
        out.push(Box::new(ToneRegionsStage::new(
            a.highlights,
            a.shadows,
            a.whites,
            a.blacks,
        )));
    }
    if a.saturation != 0.0 || a.vibrance != 0.0 {
        out.push(Box::new(SaturationStage::new(a.saturation, a.vibrance)));
    }
    out
}
```

- [ ] **Step 2: Call it from the global tail**

In `build_pipeline_split_in_region`, replace the individual `if edits.white_balance != …`,
`if edits.exposure != 0.0`, `if edits.contrast != 0.0` and `if edits.highlights != 0.0 || …`
pushes with one call, keeping every other tail stage exactly where it is:

```rust
    let mut tail: Vec<Box<dyn PipelineStage>> = per_pixel_stages(&edit_model::LocalAdjustments {
        white_balance: edits.white_balance,
        exposure: edits.exposure,
        contrast: edits.contrast,
        highlights: edits.highlights,
        shadows: edits.shadows,
        whites: edits.whites,
        blacks: edits.blacks,
        saturation: 0.0,
        vibrance: 0.0,
    });
```

⚠ **`saturation` and `vibrance` are passed as zero here, deliberately.** In the global chain
saturation runs *after* the tone curve and local contrast, not with the basic adjustments — so
it stays where it is, further down the existing tail. Passing the real values would silently
move it earlier and change every golden. Inside a mask there is no tone curve, so ordering
within `per_pixel_stages` is the whole story and saturation belongs in it.

- [ ] **Step 3: Verify nothing changed**

```bash
cargo test -p pipeline
```

Expected: **all 31 goldens byte-identical**, all tests pass. That is the entire point of this
task — if a golden moves, the extraction changed stage order.

- [ ] **Step 4: Run the full check suite**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
```

Expected: no warnings, all tests pass.

- [ ] **Step 5: Commit**

```bash
git add crates/pipeline/src/lib.rs
git commit -m "refactor(pipeline): one builder for the per-pixel stages"
```

---

## Task 3: Coverage — the definition of what a mask means

**Files:**
- Modify: `crates/edit-model/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn coverage_ramps_from_zero_at_from_to_one_at_to() {
        let g = LinearGradient {
            from: [0.0, 0.0],
            to: [1.0, 0.0],
            invert: false,
        };
        assert!((g.coverage(0.0, 0.0, 1.0) - 0.0).abs() < 1e-6);
        assert!((g.coverage(1.0, 0.0, 1.0) - 1.0).abs() < 1e-6);
        assert!((g.coverage(0.5, 0.0, 1.0) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn coverage_clamps_outside_the_axis() {
        let g = LinearGradient {
            from: [0.25, 0.0],
            to: [0.75, 0.0],
            invert: false,
        };
        assert_eq!(g.coverage(0.0, 0.0, 1.0), 0.0);
        assert_eq!(g.coverage(1.0, 0.0, 1.0), 1.0);
    }

    #[test]
    fn coverage_is_constant_perpendicular_to_the_axis() {
        // A graduated filter's whole point: it varies along one direction only.
        let g = LinearGradient {
            from: [0.0, 0.0],
            to: [1.0, 0.0],
            invert: false,
        };
        let a = g.coverage(0.3, 0.0, 1.0);
        for v in [0.1, 0.5, 0.9] {
            assert!((g.coverage(0.3, v, 1.0) - a).abs() < 1e-6);
        }
    }

    #[test]
    fn invert_mirrors_the_ramp() {
        let g = LinearGradient {
            from: [0.0, 0.0],
            to: [1.0, 0.0],
            invert: true,
        };
        assert!((g.coverage(0.0, 0.0, 1.0) - 1.0).abs() < 1e-6);
        assert!((g.coverage(1.0, 0.0, 1.0) - 0.0).abs() < 1e-6);
    }

    #[test]
    fn a_diagonal_gradient_is_not_skewed_by_a_non_square_frame() {
        // ⚠⚠ The subtle one, and the reason `coverage` takes an aspect at all.
        //
        // A *line* through two points survives anisotropic scaling — but lines *perpendicular*
        // to it do not. Compute coverage in raw normalised space and a diagonal gradient's
        // iso-lines skew on screen, which reads as a rendering glitch rather than a
        // coordinate error. This is the vignette's `roundness` problem in a different hat.
        //
        // Test: on a 2:1 frame, take two points that are equidistant from the axis *in pixel
        // space* and assert they get the same coverage.
        let aspect = 2.0; // width / height
        let g = LinearGradient {
            from: [0.0, 0.0],
            to: [1.0, 1.0],
            invert: false,
        };
        // The axis in pixel space runs (0,0) -> (2,1) in units of the frame height.
        // Two points offset from the midpoint along the pixel-space perpendicular (1,-2)/√5.
        let mid = (0.5f32, 0.5f32);
        let (px, py) = (1.0f32 / 5.0f32.sqrt(), -2.0f32 / 5.0f32.sqrt());
        let d = 0.1f32;
        // Convert the pixel-space offset back to normalised: x is in units of width = aspect.
        let p1 = (mid.0 + d * px / aspect, mid.1 + d * py);
        let p2 = (mid.0 - d * px / aspect, mid.1 - d * py);
        let c1 = g.coverage(p1.0, p1.1, aspect);
        let c2 = g.coverage(p2.0, p2.1, aspect);
        assert!(
            (c1 - c2).abs() < 1e-5,
            "perpendicular offsets gave {c1} and {c2} — the gradient is skewed"
        );
    }

    #[test]
    fn a_degenerate_gradient_covers_everything() {
        // Both endpoints equal: no axis to project onto. Returns full coverage rather than
        // dividing by zero — the safe direction, since the alternative is NaN reaching the
        // blend and painting the frame with garbage.
        let g = LinearGradient {
            from: [0.5, 0.5],
            to: [0.5, 0.5],
            invert: false,
        };
        assert_eq!(g.coverage(0.1, 0.9, 1.5), 1.0);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p edit-model coverage
```

Expected: **compile error**, `no method named coverage found for struct LinearGradient`.

- [ ] **Step 3: Write the implementation**

```rust
impl LinearGradient {
    /// Coverage at `(u, v)` in normalised frame coordinates, in `0.0..=1.0`.
    ///
    /// `aspect` is the frame's width divided by its height.
    ///
    /// ⚠ **The aspect is not decoration.** Coverage is the projection onto the axis, and a
    /// projection needs a metric. In raw normalised space the metric is anisotropic, so lines
    /// perpendicular to the axis *in that space* are not perpendicular on screen — a diagonal
    /// gradient's iso-lines would visibly skew. Scaling `u` by `aspect` restores an isotropic
    /// metric before projecting. Same species of problem as the vignette's `roundness`.
    pub fn coverage(&self, u: f32, v: f32, aspect: f32) -> f32 {
        // Work in units of frame height, where both axes have the same pixel scale.
        let (ax, ay) = (self.from[0] * aspect, self.from[1]);
        let (bx, by) = (self.to[0] * aspect, self.to[1]);
        let (dx, dy) = (bx - ax, by - ay);
        let len2 = dx * dx + dy * dy;
        if len2 <= f32::EPSILON {
            // No axis to project onto. Full coverage rather than NaN — see the test.
            return 1.0;
        }
        let t = ((u * aspect - ax) * dx + (v - ay) * dy) / len2;
        let t = t.clamp(0.0, 1.0);
        if self.invert {
            1.0 - t
        } else {
            t
        }
    }
}

impl Mask {
    /// Coverage at `(u, v)` in normalised frame coordinates, in `0.0..=1.0`.
    pub fn coverage(&self, u: f32, v: f32, aspect: f32) -> f32 {
        match self {
            Mask::Linear(g) => g.coverage(u, v, aspect),
        }
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p edit-model coverage
```

Expected: all six pass.

- [ ] **Step 5: Prove the aspect test can fail**

Temporarily drop the correction — change both `* aspect` occurrences in the `let (ax, ay)` /
`let t` lines to `* 1.0` — then run:

```bash
cargo test -p edit-model coverage
```

Expected: `a_diagonal_gradient_is_not_skewed_by_a_non_square_frame` **fails**, reporting two
different coverages. Restore and re-run.

⚠ If restoring from a copy rather than by editing, `touch crates/edit-model/src/lib.rs`
afterwards — `cp -p` preserves the older mtime and cargo reuses the sabotaged artifact.

- [ ] **Step 6: Commit**

```bash
git add crates/edit-model/src/lib.rs
git commit -m "feat(edit-model): linear gradient coverage, corrected for frame aspect"
```

---

## Task 4: `MaskStage` — run the sub-stages, blend by coverage

**Files:**
- Create: `crates/pipeline/src/stages/mask.rs`
- Modify: `crates/pipeline/src/lib.rs`, `crates/pipeline/src/stages/mod.rs`
- Modify: `crates/pipeline/tests/golden.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/pipeline/src/stages/mask.rs` with only this test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use edit_model::{LinearGradient, LocalAdjustments, Mask};

    /// A flat mid-grey image.
    fn flat(w: usize, h: usize) -> ImageBuf {
        let mut img = ImageBuf::new(w, h);
        for s in img.data_mut() {
            *s = 0.5;
        }
        img
    }

    fn exposure_mask(from: [f32; 2], to: [f32; 2]) -> MaskStage {
        MaskStage::new(
            &edit_model::MaskedAdjustment {
                mask: Mask::Linear(LinearGradient {
                    from,
                    to,
                    invert: false,
                }),
                adjustments: LocalAdjustments {
                    exposure: 1.0,
                    ..Default::default()
                },
            },
            crate::RenderScale::FULL,
        )
    }

    #[test]
    fn coverage_zero_leaves_the_pixel_untouched_and_one_applies_fully() {
        // The two endpoints of the blend, checked against the unmasked stage's own answer —
        // so this also proves `per_pixel_stages` is what the mask runs.
        let img = exposure_mask([0.0, 0.0], [1.0, 0.0]).apply(flat(64, 16));
        let left = img.pixel(0, 8)[0];
        let right = img.pixel(63, 8)[0];
        assert!((left - 0.5).abs() < 1e-4, "left edge moved: {left}");
        assert!((right - 1.0).abs() < 1e-3, "right edge should be +1 EV: {right}");
    }

    #[test]
    fn an_inactive_adjustment_set_is_a_no_op() {
        // No stages to run means no clone and no blend — and byte-identical output, which is
        // what keeps the existing goldens unchanged when `masks` is empty.
        let stage = MaskStage::new(
            &edit_model::MaskedAdjustment::default(),
            crate::RenderScale::FULL,
        );
        let before = flat(8, 8);
        let after = stage.apply(before.clone());
        assert_eq!(after, before);
    }

    #[test]
    fn a_region_render_matches_the_whole_frame_cropped() {
        // ⚠ The load-bearing test, and a direct descendant of the vignette's. A mask is
        // defined in *frame* coordinates while the 1:1 view renders a crop, so the region's
        // own dimensions must never be mistaken for the frame's. Get this wrong and masks
        // slide as you zoom.
        let (fw, fh) = (64usize, 32usize);
        let (x0, y0, rw, rh) = (10usize, 6usize, 20usize, 12usize);

        let whole = exposure_mask([0.0, 0.0], [1.0, 1.0]).apply(flat(fw, fh));

        let region = crate::Region {
            full_w: fw,
            full_h: fh,
            x0,
            y0,
        };
        let mut sub = ImageBuf::new(rw, rh);
        for s in sub.data_mut() {
            *s = 0.5;
        }
        let part = MaskStage::new_in_region(
            &edit_model::MaskedAdjustment {
                mask: Mask::Linear(LinearGradient {
                    from: [0.0, 0.0],
                    to: [1.0, 1.0],
                    invert: false,
                }),
                adjustments: LocalAdjustments {
                    exposure: 1.0,
                    ..Default::default()
                },
            },
            crate::RenderScale::FULL,
            Some(region),
        )
        .apply(sub);

        for y in 0..rh {
            for x in 0..rw {
                assert_eq!(
                    part.pixel(x, y),
                    whole.pixel(x0 + x, y0 + y),
                    "region pixel ({x},{y}) differs from the whole frame's ({},{})",
                    x0 + x,
                    y0 + y
                );
            }
        }
    }
}
```

- [ ] **Step 2: Register the module and run the tests to verify they fail**

Add to `crates/pipeline/src/stages/mod.rs`, in alphabetical position:

```rust
pub mod mask;
```

```bash
cargo test -p pipeline mask
```

Expected: **compile error**, `cannot find type MaskStage in this scope`.

- [ ] **Step 3: Write the implementation**

Insert above the test module in `crates/pipeline/src/stages/mask.rs`:

```rust
//! A masked group of per-pixel adjustments.
//!
//! Runs ordinary stages on a copy of the image and blends the result back by the mask's
//! coverage. ⚠ **Every existing stage is reused completely unchanged** — nothing else in the
//! pipeline learns that masking exists, which is what keeps the `PipelineStage` seam opaque
//! for the eventual GPU port.

use crate::{per_pixel_stages, PipelineStage, Region, RenderScale};
use core_image::ImageBuf;
use edit_model::MaskedAdjustment;

pub struct MaskStage {
    masked: MaskedAdjustment,
    stages: Vec<Box<dyn PipelineStage>>,
    region: Option<Region>,
}

impl MaskStage {
    /// For a render covering the whole frame.
    pub fn new(masked: &MaskedAdjustment, scale: RenderScale) -> Self {
        Self::new_in_region(masked, scale, None)
    }

    /// For a render covering only part of the frame — coverage is still measured against the
    /// full frame, so a cropped region is masked exactly as that part of the whole would be.
    pub fn new_in_region(
        masked: &MaskedAdjustment,
        scale: RenderScale,
        region: Option<Region>,
    ) -> Self {
        let _ = scale; // no spatial sub-stage yet; 3.1c's brush will need it
        Self {
            masked: *masked,
            stages: per_pixel_stages(&masked.adjustments),
            region,
        }
    }
}

impl PipelineStage for MaskStage {
    fn name(&self) -> &'static str {
        "mask"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        let (w, h) = (image.width(), image.height());
        if w == 0 || h == 0 || self.stages.is_empty() {
            return image;
        }

        // ⚠ Without a region the buffer *is* the frame — byte for byte the whole-frame path.
        let (fw, fh, x0, y0) = match self.region {
            Some(r) => (r.full_w, r.full_h, r.x0, r.y0),
            None => (w, h, 0, 0),
        };
        if fw == 0 || fh == 0 {
            return image;
        }
        let aspect = fw as f32 / fh as f32;

        // Run the sub-stages on a copy. The clone is the price of reusing every stage
        // unchanged; a bounding-box optimisation is deferred to 3.1b/c, where masks are
        // small enough for it to pay.
        let mut adjusted = image.clone();
        for stage in &self.stages {
            adjusted = stage.apply(adjusted);
        }

        // ⚠ `map_pixels_xy` rather than a hand-written loop: it is rayon-parallel over rows
        // and is the idiom `VignetteStage` already uses for position-dependent work. The
        // closure captures `&adjusted`, which is `Sync` because `ImageBuf` is a `Vec<f32>`.
        image.map_pixels_xy(|x, y, base| {
            // Pixel position in the *frame's* normalised coordinates.
            let u = (x0 + x) as f32 / fw as f32;
            let v = (y0 + y) as f32 / fh as f32;
            let c = self.masked.mask.coverage(u, v, aspect).clamp(0.0, 1.0);
            let adj = adjusted.pixel(x, y);
            [
                base[0] + (adj[0] - base[0]) * c,
                base[1] + (adj[1] - base[1]) * c,
                base[2] + (adj[2] - base[2]) * c,
            ]
        })
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p pipeline mask
```

Expected: `test result: ok. 3 passed`.

- [ ] **Step 5: Prove the region test can fail**

Temporarily use the region's own dimensions instead of the frame's — change
`Some(r) => (r.full_w, r.full_h, r.x0, r.y0)` to `Some(_) => (w, h, 0, 0)` — then run:

```bash
cargo test -p pipeline mask
```

Expected: `a_region_render_matches_the_whole_frame_cropped` **fails**. Restore and re-run.

- [ ] **Step 6: Wire it into the tail**

In `build_pipeline_split_in_region` in `crates/pipeline/src/lib.rs`, after the HSL stage is
pushed and **before** the vignette:

```rust
    // ⚠ After the global per-pixel block because a mask is the same kind of operation,
    // localised — it should act on the global look rather than be re-mapped by it. Before the
    // vignette and sharpening because both have earned their end-of-chain positions: the
    // vignette's "+100 = exactly white" endpoint depends on running after the tone curve, and
    // sharpening must not amplify anything a later stage would then stretch.
    for m in &edits.masks {
        if m.adjustments.is_active() {
            tail.push(Box::new(MaskStage::new_in_region(m, scale, region)));
        }
    }
```

Add `use crate::stages::mask::MaskStage;` to the imports.

- [ ] **Step 7: Add two goldens**

In `crates/pipeline/tests/golden.rs`, add:

```rust
#[test]
fn golden_mask_linear_exposure() {
    // A graduated exposure lift across the chart: the left edge untouched, the right at +1 EV.
    check_golden(
        "mask_linear_exposure",
        &Edits {
            masks: vec![edit_model::MaskedAdjustment {
                mask: edit_model::Mask::Linear(edit_model::LinearGradient {
                    from: [0.0, 0.0],
                    to: [1.0, 0.0],
                    invert: false,
                }),
                adjustments: edit_model::LocalAdjustments {
                    exposure: 1.0,
                    ..Default::default()
                },
            }],
            ..Edits::default()
        },
    );
}

#[test]
fn golden_mask_linear_inverted_saturation() {
    // A second mask type of payload, inverted, so the reference covers more than one stage
    // inside the mask and the `invert` flag reaches a real image.
    check_golden(
        "mask_linear_inverted_saturation",
        &Edits {
            masks: vec![edit_model::MaskedAdjustment {
                mask: edit_model::Mask::Linear(edit_model::LinearGradient {
                    from: [0.2, 0.0],
                    to: [0.8, 1.0],
                    invert: true,
                }),
                adjustments: edit_model::LocalAdjustments {
                    saturation: -100.0,
                    exposure: -0.5,
                    ..Default::default()
                },
            }],
            ..Edits::default()
        },
    );
}
```

Generate the references:

```bash
UPDATE_GOLDEN=1 cargo test -p pipeline golden_mask
```

⚠ **Then look at the two PNGs** in `crates/pipeline/tests/golden/`. A golden nobody has looked
at is just a hash — that lesson is recorded from the noise-reduction chart, where an unexamined
reference encoded chroma bleeding rather than the property under test. The first should show a
smooth left-to-right brightening; the second a diagonal ramp into desaturation, densest at the
*lower left* because it is inverted.

- [ ] **Step 8: Confirm the other goldens did not move**

```bash
cargo test -p pipeline
```

Expected: all pass, and **the 31 pre-existing references byte-identical** — the proof
`MaskStage` is inert when `masks` is empty.

- [ ] **Step 9: Extend the app-level master invariant**

In `crates/app/src/render_engine.rs`, add a mask to the edit set in
`a_region_render_matches_the_whole_render_cropped`:

```rust
            masks: vec![edit_model::MaskedAdjustment {
                mask: edit_model::Mask::Linear(edit_model::LinearGradient {
                    from: [0.1, 0.1],
                    to: [0.9, 0.8],
                    invert: false,
                }),
                adjustments: edit_model::LocalAdjustments {
                    exposure: 0.6,
                    ..Default::default()
                },
            }],
```

⚠ That test validates the region maths, the vignette correction **and** the prefix cache at
once. Adding a mask to its edit set is what proves masks survive the cached render path, not
just a bare stage call.

- [ ] **Step 10: Run the full check suite**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
```

Expected: no warnings, all tests pass.

- [ ] **Step 11: Commit**

```bash
git add crates/pipeline crates/app/src/render_engine.rs
git commit -m "feat(pipeline): masked local adjustments, blended by coverage"
```

---

## Task 5: Verify on a real photograph, and document

**Files:** `CLAUDE.md`

- [ ] **Step 1: Write a mask sidecar by hand**

This is the point of building headless first — the whole feature is exercisable with no GUI.

```bash
cat > /tmp/mask.json <<'JSON'
{
  "exposure": 0.0,
  "masks": [
    {
      "mask": { "Linear": { "from": [0.0, 0.0], "to": [0.0, 1.0], "invert": false } },
      "adjustments": { "exposure": -1.5 }
    }
  ]
}
JSON
```

- [ ] **Step 2: Render it**

Use a RAW on this machine; `~/Desktop/fuji/DSCF6509.RAF` is the fixture every other
feature was validated against.

```bash
cargo run --release -p rustroom -- render ~/Desktop/fuji/DSCF6509.RAF /tmp/mask-test.jpg --edits /tmp/mask.json
```

Expected: `Wrote /tmp/mask-test.jpg`, and the image is **progressively darker toward the
bottom** — a graduated filter running top to bottom, untouched at the top edge and −1.5 EV at
the bottom.

⚠ Open it. This is the first end-to-end evidence that normalised coordinates, the aspect
correction and the blend all agree on a real, non-square photograph — which no unit test
covers, because they all use synthetic buffers.

- [ ] **Step 3: Render the same mask inverted, as a control**

```bash
sed 's/"invert": false/"invert": true/' /tmp/mask.json > /tmp/mask-inv.json
cargo run --release -p rustroom -- render ~/Desktop/fuji/DSCF6509.RAF /tmp/mask-inv.jpg --edits /tmp/mask-inv.json
```

Expected: the darkening is at the **top** instead. If both images look the same, `invert` is
not reaching the render.

- [ ] **Step 4: Update `CLAUDE.md`**

Add a section in the "Current status" block, house style: that M3.1a landed and what it is; the
Phase 3 decomposition and why M3.1 comes first; the per-pixel-only rule and *why* it is not an
arbitrary subset; `per_pixel_stages` as one list with two consumers; the aspect correction and
the skew it prevents; that masks default to not transferring; and the caveats — the clone per
mask, no bbox optimisation yet, no mask intersection or range masks, and that **M3.1 does not
close until 3.1c's brush**.

Then set the **▶ NEXT ACTION** to **M3.1a plan B — the canvas UI**: mask handles on the canvas,
a mask list, history integration. Note that the list's placement is provisional and that the
parked button/layout work resumes once M3.1 lands. Add this spec and plan to the docs list.

- [ ] **Step 5: Commit**

```bash
git add CLAUDE.md docs/superpowers/plans/2026-08-13-masking-headless.md
git commit -m "docs: record the headless masking architecture"
```
