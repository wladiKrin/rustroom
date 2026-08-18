# M3.1c-A — Brush Mask (Headless) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `Mask::Brush` — vector strokes in pre-geometry source coordinates, rasterized into a bbox-bounded 8-bit coverage map — proved against goldens and a real RAW before any canvas code exists.

**Architecture:** Nine tasks, and the first four exist to make the fifth safe. `edit-model` gains `BrushStroke`/`Brush`; `Mask::coverage` becomes `analytic_coverage -> Option<f32>`; `mask_tool::apply_drag` becomes in-place — all as green refactors *before* the variant lands and `Copy` disappears. Then a pure coordinate map, the rasterizer, and a `CoverageSource` split inside `MaskStage`. The `build_pipeline` family keeps its signatures, so the CLI, export and all 35 goldens compile untouched.

**Tech Stack:** Rust, serde, rayon.

**Spec:** `docs/superpowers/specs/2026-08-18-brush-headless-design.md`

---

## Before you start

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

**The cargo package is `app`; `rustroom_app` is its lib target.** Use `cargo test -p app`.

**No `Co-Authored-By` trailer. Never `git push`.** Branch first — the last two features each had their own branch, merged by the user.

⚠ **After writing a test, break the thing it names and watch it fail.** This project has **eight** recorded cases of a test that could not detect its own subject. A sabotage that passes is a finding about the test — stop and strengthen it. Restore with `cp`, then `touch` (a `mv` keeps the old mtime and cargo reuses the sabotaged build).

⚠ Items in the `app` **binary** (`develop/*`, `theme.rs`) fail `clippy -D warnings` as dead code without a caller. Items in `edit-model`, `pipeline` and `rustroom_app`'s lib are fine.

---

## The transform, derived from `GeometryStage` rather than invented

`GeometryStage::apply` is `orient` → `rotate_bilinear` → `apply_crop`, and all three are written as
**inverse** maps (output pixel asks where it came from). The rasterizer needs the **forward** map.
Inverting each, with source normalised `(u, v)`:

**Orientation** — `orient` un-rotates and then un-mirrors in source space, so forward is mirror
first, then turn. Dimensionless:

| `rotate` | forward |
|---|---|
| `Zero` | `(u', v)` |
| `Cw90` | `(1 − v, u')` |
| `Cw180` | `(1 − u', 1 − v)` |
| `Cw270` | `(v, 1 − u')` |

where `u' = 1 − u` when `mirror`, else `u`.

**Rotation** — `rotate_bilinear`'s inverse is `sx = cx + dx·cos + dy·sin`,
`sy = cy − dx·sin + dy·cos`, i.e. `R(−θ)`. The forward is `R(θ)`:
`dx' = dx·cos − dy·sin`, `dy' = dx·sin + dy·cos`.

⚠ It mixes axes, so it needs an **isotropic** space. Working in units of the oriented image's
*height* (`X = ou · aspect_o`, `Y = ov`) makes it isotropic while needing only the **aspect**,
never absolute dimensions.

**Crop** — `(ru − left) / (right − left)`, normalised.

**Frame pixels** — `× frame_w`, `× frame_h`.

And the oriented pre-crop dimensions in render pixels are `ow = frame_w / (right − left)` and
`oh = frame_h / (bottom − top)`, so `aspect_o = ow / oh` and the radius scale is `max(ow, oh)` —
the source long edge, since orientation only permutes pixels.

⚠ **`rotate_bilinear` centres on `(w−1)/2`, and the map below centres on the normalised midpoint.**
That is a half-pixel difference, far below mask-edge precision, and it is why the agreement test
uses a ~1.5 px tolerance rather than exact equality.

---

## File structure

| File | Responsibility | Change |
|---|---|---|
| `crates/edit-model/src/lib.rs` | `BrushStroke`, `Brush`, the `Mask::Brush` variant, `analytic_coverage` | Modify |
| `crates/app/src/mask_tool.rs` | `apply_drag` in place; brush has no handles | Modify |
| `crates/app/src/develop/canvas.rs` | `Copy` removal fallout | Modify |
| `crates/pipeline/src/brush/mod.rs` | `CoverageMap`, `rasterize` | Create |
| `crates/pipeline/src/brush/frame_map.rs` | `FrameMap` — source normalised → frame pixels | Create |
| `crates/pipeline/src/stages/mask.rs` | `CoverageSource`, the bbox early-out | Modify |
| `crates/pipeline/tests/golden.rs` | Two brush cases | Modify |

⚠ `brush` is a **module directory with two files**, not one: the coordinate map is a
self-contained piece of geometry with its own tests, and the rasterizer is a self-contained piece
of drawing. Keeping them apart is what lets the agreement test in Task 5 be read on its own.

---

## Task 1: `BrushStroke` and `Brush` types

Additive and green — the `Mask` variant comes later.

**Files:**
- Modify: `crates/edit-model/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Add to `crates/edit-model/src/lib.rs`'s test module:

```rust
    #[test]
    fn a_brush_defaults_to_no_strokes() {
        // An empty brush must be inert rather than covering everything: a mask added but not
        // yet painted has to do nothing at all.
        assert!(Brush::default().strokes.is_empty());
    }

    #[test]
    fn a_brush_round_trips_through_json() {
        let src = Brush {
            invert: false,
            strokes: vec![
                BrushStroke {
                    points: vec![[0.1, 0.2], [0.3, 0.4]],
                    radius: 0.05,
                    hardness: 60.0,
                    erase: false,
                },
                BrushStroke {
                    points: vec![[0.5, 0.5]],
                    radius: 0.02,
                    hardness: 100.0,
                    erase: true,
                },
            ],
        };
        let back: Brush = serde_json::from_str(&serde_json::to_string(&src).unwrap()).unwrap();
        assert_eq!(back, src);
    }

    #[test]
    fn a_partial_stroke_json_gets_defaults() {
        let s: BrushStroke = serde_json::from_str(r#"{"points":[[0.5,0.5]]}"#).unwrap();
        assert_eq!(s.points, vec![[0.5, 0.5]]);
        assert!(!s.erase, "a stroke paints unless it says otherwise");
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p edit-model brush 2>&1 | tail -10`
Expected: FAIL — `cannot find struct Brush`.

- [ ] **Step 3: Write the implementation**

Insert into `crates/edit-model/src/lib.rs`, immediately before `/// The mask primitives.`:

```rust
/// One continuous drag of the brush.
///
/// ⚠ **Points are in pre-geometry SOURCE normalised coordinates**, unlike [`LinearGradient`] and
/// [`RadialGradient`], which are in *frame* coordinates. `MaskStage` runs after `GeometryStage`,
/// so a frame-anchored stroke would slide across the image content whenever the crop changed —
/// invisible for a gradient, ruinous for a stroke painted onto a subject. The asymmetry is
/// principled: a gradient is compositional, a stroke is content-referred.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct BrushStroke {
    pub points: Vec<[f32; 2]>,
    /// Radius as a fraction of the source image's **long edge**.
    ///
    /// ⚠ The long edge, not the width — `points` are normalised per axis and so live in an
    /// anisotropic space. A radius measured against width but compared against height would make
    /// every dab an ellipse. Choosing an isotropic unit avoids the correction that
    /// `LinearGradient::coverage` and `mask_tool::hit_test` both need.
    pub radius: f32,
    /// 0 = maximally soft, 100 = a hard edge.
    ///
    /// ⚠ Arithmetically this is `100 − feather` in [`RadialGradient`]'s maths. It keeps the brush
    /// word because that is what every brush calls it. **Do not "unify" the two** — the
    /// inversion would be silent, since both spellings look plausible.
    pub hardness: f32,
    /// Subtract from coverage rather than adding to it.
    pub erase: bool,
}

/// A painted mask: an ordered list of strokes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Brush {
    pub strokes: Vec<BrushStroke>,
    /// Flip coverage, as both analytic shapes do.
    ///
    /// ⚠ Present from the start rather than retrofitted when `Mask::invert_mut` needs it in
    /// Task 4 — adding a field to a struct whose literals are already written elsewhere in this
    /// plan would break them.
    pub invert: bool,
}
```

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p edit-model brush 2>&1 | tail -8`
Expected: PASS — 3 tests.

- [ ] **Step 5: Sabotage**

Remove `#[serde(default)]` from `BrushStroke`.
Run: `cargo test -p edit-model a_partial_stroke_json 2>&1 | tail -8`
Expected: **FAIL** — missing field `radius`.

Restore, then `touch crates/edit-model/src/lib.rs`.

- [ ] **Step 6: Commit**

```bash
git add crates/edit-model/src/lib.rs
git commit -F - <<'EOF'
feat(edit-model): BrushStroke and Brush types

Not yet reachable from `Mask` — that variant lands once its consumers are
ready, following the sequencing that made M3.1b's radial a one-arm change.

⚠ Points are in pre-geometry SOURCE coordinates, unlike the two analytic
masks. MaskStage runs after GeometryStage, so a frame-anchored stroke slides
across the image content whenever the crop changes.

⚠ Radius is a fraction of the long edge — the aspect trap in its fourth guise,
resolved by choosing an isotropic unit rather than by a correction.
EOF
```

---

## Task 2: `Mask::coverage` becomes `analytic_coverage`

A pure refactor while `Mask` still has two variants.

**Files:**
- Modify: `crates/edit-model/src/lib.rs`
- Modify: `crates/pipeline/src/stages/mask.rs`

- [ ] **Step 1: Write the failing test**

Add to `crates/edit-model/src/lib.rs`'s test module:

```rust
    #[test]
    fn the_analytic_masks_answer_coverage_directly() {
        // Both existing shapes are pure functions of position, so they answer `Some`. The brush
        // will not, which is what this method's `Option` exists to express.
        let lin = Mask::Linear(LinearGradient {
            from: [0.0, 0.0],
            to: [1.0, 0.0],
            invert: false,
        });
        assert_eq!(lin.analytic_coverage(0.5, 0.5, 1.0), Some(0.5));
        let rad = Mask::Radial(RadialGradient::default());
        assert_eq!(rad.analytic_coverage(0.5, 0.5, 1.0), Some(1.0));
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p edit-model analytic_coverage 2>&1 | tail -8`
Expected: FAIL — `no method named analytic_coverage`.

- [ ] **Step 3: Rewrite the method**

In `crates/edit-model/src/lib.rs`, replace `Mask::coverage` with:

```rust
    /// Coverage at `(u, v)` in normalised frame coordinates, for the **analytic** shapes.
    ///
    /// `None` means the shape is sampled data — a brush — and must be rasterized instead. It is
    /// an `Option` rather than a `0.0` fallback because a fallback compiles, runs, and silently
    /// produces a mask that covers nothing; the `Option` forces every caller to decide.
    ///
    /// `aspect` is the frame's width divided by its height. ⚠ Whether a variant *needs* it
    /// differs: see [`LinearGradient::coverage`] for why a projection does, and
    /// [`RadialGradient::coverage`] for why an axis-aligned ellipse does not.
    pub fn analytic_coverage(&self, u: f32, v: f32, aspect: f32) -> Option<f32> {
        match self {
            Mask::Linear(g) => Some(g.coverage(u, v, aspect)),
            Mask::Radial(r) => Some(r.coverage(u, v, aspect)),
        }
    }
```

- [ ] **Step 4: Update the one production caller**

In `crates/pipeline/src/stages/mask.rs`, replace:

```rust
            let c = self.masked.mask.coverage(u, v, aspect).clamp(0.0, 1.0);
```

with:

```rust
            // Task 7 replaces this with a `CoverageSource` resolved outside the loop; for now
            // the two analytic shapes are the only ones that exist.
            let c = self
                .masked
                .mask
                .analytic_coverage(u, v, aspect)
                .unwrap_or(0.0)
                .clamp(0.0, 1.0);
```

- [ ] **Step 5: Fix any remaining callers the compiler names**

Run: `cargo test --all 2>&1 | grep -E '^error' -A4 | head -20`
Expected: either nothing, or `no method named coverage` at test call sites in
`crates/edit-model/src/lib.rs` — those are `LinearGradient::coverage` /
`RadialGradient::coverage` (unchanged, still fine) or `Mask::coverage` (change to
`analytic_coverage(..).unwrap()`).

- [ ] **Step 6: Verify**

Run: `cargo test --all 2>&1 | grep -E '^test result' | grep -v 'ok\.'`
Expected: no output.

Run: `cargo test -p pipeline --test golden 2>&1 | grep 'test result'`
Expected: 35 passed — the goldens are the proof this refactor changed no pixels.

- [ ] **Step 7: Commit**

```bash
git add crates/edit-model/src/lib.rs crates/pipeline/src/stages/mask.rs
git commit -F - <<'EOF'
refactor(edit-model): Mask::coverage becomes analytic_coverage -> Option

Preparation for the brush, which is sampled data and cannot answer a pure
function of position. The `Option` is the point: a `Brush => 0.0` arm would
compile, run, and silently produce a mask covering nothing.

All 35 goldens byte-identical, which is the proof no pixel moved.
EOF
```

---

## Task 3: `mask_tool::apply_drag` in place

**Files:**
- Modify: `crates/app/src/mask_tool.rs`
- Modify: `crates/app/src/develop/canvas.rs`

- [ ] **Step 1: Change the signature**

In `crates/app/src/mask_tool.rs`, replace `apply_drag` with:

```rust
/// Apply a normalized drag `(dx, dy)` to `handle`, clamped to the frame.
///
/// ⚠ **In place, not `Mask -> Mask`.** A brush carries a `Vec<BrushStroke>`, so the by-value form
/// would clone an entire stroke list on every frame of every drag. Taking `&mut` also means the
/// mismatched-handle case is simply "do nothing" rather than "return the input".
///
/// ⚠ Clamping matters: an endpoint dragged off the canvas would be unreachable afterwards.
pub fn apply_drag(m: &mut Mask, handle: Handle, dx: f32, dy: f32) {
    let mv = |p: [f32; 2]| [(p[0] + dx).clamp(0.0, 1.0), (p[1] + dy).clamp(0.0, 1.0)];
    match (m, handle) {
        (Mask::Linear(g), Handle::From) => g.from = mv(g.from),
        (Mask::Linear(g), Handle::To) => g.to = mv(g.to),
        (Mask::Linear(g), Handle::Line) => {
            g.from = mv(g.from);
            g.to = mv(g.to);
        }
        (Mask::Radial(r), Handle::Center) => r.center = mv(r.center),
        (Mask::Radial(r), Handle::Edge(side)) => resize_radial(r, side, dx, dy),
        // ⚠ A handle belonging to another shape. Unreachable — there is no UI to change a mask's
        // kind, let alone mid-drag — and doing nothing is the right failure for a UI.
        _ => {}
    }
}

/// Resize about a **fixed centre**: each edge grows away from the middle, which is what a marked
/// centre implies and what Lightroom does.
fn resize_radial(r: &mut RadialGradient, side: Side, dx: f32, dy: f32) {
    let (mut rx, mut ry) = (r.radius[0], r.radius[1]);
    match side {
        Side::Right => rx += dx,
        Side::Left => rx -= dx,
        Side::Bottom => ry += dy,
        Side::Top => ry -= dy,
    }
    r.radius = [
        rx.clamp(RADIUS_MIN, RADIUS_MAX),
        ry.clamp(RADIUS_MIN, RADIUS_MAX),
    ];
}
```

- [ ] **Step 2: Update the tests**

Every `apply_drag(m, h, dx, dy)` in the test module becomes a mutate-then-inspect. Replace the two
unwrap helpers and use this shape throughout — for example
`dragging_an_endpoint_leaves_the_other_alone` becomes:

```rust
    #[test]
    fn dragging_an_endpoint_leaves_the_other_alone() {
        let mut m = horizontal();
        apply_drag(&mut m, Handle::From, 0.1, -0.2);
        let out = gradient_of(m);
        assert_eq!(out.from, [0.3, 0.3]);
        assert_eq!(out.to, horizontal_gradient().to, "the far end must not move");
    }
```

and `a_mismatched_handle_leaves_the_mask_unchanged` becomes:

```rust
    #[test]
    fn a_mismatched_handle_leaves_the_mask_unchanged() {
        // ⚠ Unreachable in the app — there is no UI to change a mask's kind, let alone mid-drag —
        // but the flat `Handle` enum admits the pair, so the degradation is worth pinning. Doing
        // nothing is the right failure for a UI; panicking is not.
        let mut radial = Mask::Radial(ellipse());
        let before = radial.clone();
        apply_drag(&mut radial, Handle::Line, 0.1, 0.1);
        assert_eq!(radial, before);

        let mut linear = horizontal();
        let before = linear.clone();
        apply_drag(&mut linear, Handle::Center, 0.1, 0.1);
        assert_eq!(linear, before);
    }
```

⚠ `gradient_of` and `radial_of` take `Mask` by value; that still works because `Mask` is `Clone`.
Apply the same mutate-then-inspect shape to the other seven tests that call `apply_drag`.

- [ ] **Step 3: Update the canvas call site**

In `crates/app/src/develop/canvas.rs`, replace:

```rust
                    m.mask = mask_tool::apply_drag(m.mask, h, dx, dy);
```

with:

```rust
                    mask_tool::apply_drag(&mut m.mask, h, dx, dy);
```

- [ ] **Step 4: Verify**

Run: `cargo clippy --all-targets --all-features -- -D warnings 2>&1 | grep -cE '^(error|warning)'`
Expected: `0`.

Run: `cargo test --all 2>&1 | grep -E '^test result' | grep -v 'ok\.'`
Expected: no output.

- [ ] **Step 5: Commit**

```bash
git add crates/app/src/mask_tool.rs crates/app/src/develop/canvas.rs
git commit -F - <<'EOF'
refactor(app): mask_tool::apply_drag mutates in place

Preparation for the brush. A `Vec<BrushStroke>` cannot be `Copy`, and the
by-value form would clone an entire stroke list on every frame of every drag.

Behaviour unchanged: every existing test passes with the same assertions,
restructured as mutate-then-inspect.
EOF
```

---

## Task 4: the `Mask::Brush` variant and the `Copy` removal

**Files:**
- Modify: `crates/edit-model/src/lib.rs`
- Modify: `crates/pipeline/src/stages/mask.rs`
- Modify: `crates/app/src/develop/canvas.rs`

- [ ] **Step 1: Write the failing tests**

Add to `crates/edit-model/src/lib.rs`'s test module:

```rust
    #[test]
    fn a_brush_has_no_analytic_coverage() {
        // ⚠ The whole reason `analytic_coverage` returns an Option. A `0.0` here would compile,
        // run, and produce a mask that silently covers nothing.
        let m = Mask::Brush(Brush::default());
        assert_eq!(m.analytic_coverage(0.5, 0.5, 1.0), None);
    }

    #[test]
    fn every_mask_variant_has_a_label_and_an_invert() {
        // A table over every variant, so a fourth cannot ship with an empty list row or an
        // `invert` the panel's checkbox cannot reach.
        for mut m in [
            Mask::Linear(LinearGradient::default()),
            Mask::Radial(RadialGradient::default()),
            Mask::Brush(Brush::default()),
        ] {
            assert!(!m.label().is_empty());
            *m.invert_mut() = true;
            assert!(*m.invert_mut(), "invert_mut must expose the real field");
        }
    }

    #[test]
    fn an_existing_sidecar_without_a_brush_still_loads() {
        // ⚠ `Mask` is externally tagged, so every sidecar written before M3.1c holds
        // `{"Linear": {...}}`. Adding a variant is backward compatible — but a switch to an
        // untagged representation would silently break every file on disk.
        //
        // ⚠ Assert the PAYLOAD, not the variant. Under `#[serde(untagged)]` serde reads
        // `LinearGradient` from the whole object, and its container-level `#[serde(default)]`
        // makes the unknown "Linear" key be ignored and every field default — so the variant
        // still matches with `from`/`to` silently `[0,0]`.
        let json = r#"{"masks":[{"mask":{"Linear":{"from":[0.0,0.0],"to":[1.0,0.0],
                      "invert":false}},"adjustments":{"exposure":1.0}}]}"#;
        let e: Edits = serde_json::from_str(json).unwrap();
        assert_eq!(e.masks.len(), 1);
        let Mask::Linear(g) = &e.masks[0].mask else {
            panic!("expected a linear gradient, got {:?}", e.masks[0].mask);
        };
        assert_eq!(g.to, [1.0, 0.0], "the endpoints must survive the round trip");
    }
```

⚠ There is already an `an_existing_linear_sidecar_still_loads` from M3.1b asserting the same
thing. **Delete the old one** rather than keeping both — one guard, one place.

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p edit-model brush 2>&1 | tail -8`
Expected: FAIL — `no variant named Brush`.

- [ ] **Step 3: Add the variant and drop `Copy`**

In `crates/edit-model/src/lib.rs`:

```rust
/// The mask primitives.
///
/// ⚠ **Not `Copy`** — `Mask::Brush` carries a `Vec<BrushStroke>`. `MaskedAdjustment` therefore
/// is not `Copy` either. Both are still `Clone`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Mask {
    Linear(LinearGradient),
    Radial(RadialGradient),
    Brush(Brush),
}
```

Add the arms:

```rust
    pub fn analytic_coverage(&self, u: f32, v: f32, aspect: f32) -> Option<f32> {
        match self {
            Mask::Linear(g) => Some(g.coverage(u, v, aspect)),
            Mask::Radial(r) => Some(r.coverage(u, v, aspect)),
            // Sampled data — `pipeline::brush::rasterize` answers for this one.
            Mask::Brush(_) => None,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Mask::Linear(_) => "Gradient",
            Mask::Radial(_) => "Radial",
            Mask::Brush(_) => "Brush",
        }
    }
```

⚠ `invert_mut` needs a brush arm: `Mask::Brush(b) => &mut b.invert`. The field already exists —
Task 1 added it.

Then remove `Copy` from `MaskedAdjustment`'s derive (keep `Clone`).

- [ ] **Step 4: Fix the cascade the compiler names**

Expect exactly these, and fix each as shown:

- `crates/pipeline/src/stages/mask.rs:35` — `masked: *masked` → `masked: masked.clone()`.
- `crates/app/src/develop/canvas.rs` — `let mask = m.mask;` → `let mask = m.mask.clone();`
- `crates/app/src/develop/canvas.rs` — `paint_mask(&ui.painter_at(img_rect), mask, ..)` →
  pass `&mask` and change `paint_mask`'s parameter to `mask: &edit_model::Mask`, matching on
  `mask` with `edit_model::Mask::Linear(g)` binding by reference.
- `paint_mask` needs a `Brush` arm. **Draw nothing** for now:

```rust
        // 3.1c-B paints the coverage tint and the brush cursor. There are no handles to draw:
        // a stroke is not a shape with grab points, which is the observation that made
        // `mask_tool::hit_test` return `None` for it.
        edit_model::Mask::Brush(_) => {}
```

- `mask_tool::hit_test` needs a brush arm returning `None`, and `apply_drag`'s `_ => {}`
  already covers the brush.

- [ ] **Step 5: Run and verify**

Run: `cargo clippy --all-targets --all-features -- -D warnings 2>&1 | grep -E '^(error|warning)' -A4 | head -20`
Expected: no output.

Run: `cargo test --all 2>&1 | grep -E '^test result' | grep -v 'ok\.'`
Expected: no output.

Run: `git status --short crates/pipeline/tests/golden/`
Expected: no output — no golden may move.

- [ ] **Step 6: Sabotage**

Change the brush arm of `analytic_coverage` to `Some(0.0)`.
Run: `cargo test -p edit-model a_brush_has_no_analytic_coverage 2>&1 | tail -8`
Expected: **FAIL**.

Restore, then add `#[serde(untagged)]` above `pub enum Mask`.
Run: `cargo test -p edit-model an_existing_sidecar_without_a_brush 2>&1 | tail -8`
Expected: **FAIL** on "the endpoints must survive the round trip".

Restore both, then `touch crates/edit-model/src/lib.rs`.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -F - <<'EOF'
feat(edit-model): Mask::Brush, and Mask is no longer Copy

The variant plus its label, invert and `analytic_coverage => None` arms.
`Mask` and `MaskedAdjustment` lose `Copy` because a stroke list cannot be
copied; both stay `Clone`.

`mask_tool::hit_test` returns None for a brush and `paint_mask` draws nothing
yet: a stroke is not a shape with grab points, which is the observation that
shapes 3.1c-B's canvas work.

All 35 goldens byte-identical.
EOF
```

---

## Task 5: `FrameMap` — source normalised to frame pixels

**Files:**
- Create: `crates/pipeline/src/brush/mod.rs`
- Create: `crates/pipeline/src/brush/frame_map.rs`
- Modify: `crates/pipeline/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/pipeline/src/brush/frame_map.rs` with only its test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use core_image::ImageBuf;
    use edit_model::{CropRect, Geometry, Orientation, Quarter};

    /// Where `GeometryStage` actually puts a marked source pixel.
    ///
    /// ⚠⚠ **This is the whole point of the test suite below: the map is checked against the real
    /// transform, not against a re-derivation of it.** Re-deriving the maths in a test proves
    /// only that the same mistake was made twice — and this map was derived by inverting three
    /// separate inverse maps, which is exactly the kind of derivation that goes wrong quietly.
    fn where_geometry_puts(g: &Geometry, w: usize, h: usize, sx: usize, sy: usize) -> (f32, f32) {
        let mut img = ImageBuf::new(w, h);
        img.set_pixel(sx, sy, [1.0, 1.0, 1.0]);
        let out = crate::stages::geometry::GeometryStage::new(g).apply(img);
        let (mut best, mut bx, mut by) = (0.0f32, 0usize, 0usize);
        for y in 0..out.height() {
            for x in 0..out.width() {
                let v = out.pixel(x, y)[0];
                if v > best {
                    best = v;
                    bx = x;
                    by = y;
                }
            }
        }
        assert!(best > 0.05, "the marked pixel was cropped away or lost");
        (bx as f32, by as f32)
    }

    fn check(g: Geometry, w: usize, h: usize, sx: usize, sy: usize) {
        let (fx, fy) = where_geometry_puts(&g, w, h, sx, sy);
        // The frame's dimensions are what `MaskStage` sees, so derive them the same way.
        let mut img = ImageBuf::new(w, h);
        img.set_pixel(sx, sy, [1.0, 1.0, 1.0]);
        let out = crate::stages::geometry::GeometryStage::new(&g).apply(img);
        let map = FrameMap::new(&g, out.width(), out.height());
        let (mx, my) = map.point([
            (sx as f32 + 0.5) / w as f32,
            (sy as f32 + 0.5) / h as f32,
        ]);
        // ⚠ ~1.5 px: `rotate_bilinear` centres on `(w-1)/2` while the map centres on the
        // normalised midpoint, and the brightest-pixel search quantises to whole pixels.
        assert!(
            (mx - fx).abs() < 1.5 && (my - fy).abs() < 1.5,
            "map says ({mx:.2}, {my:.2}), geometry says ({fx}, {fy})"
        );
    }

    fn geo(orientation: Orientation, angle: f32, crop: CropRect) -> Geometry {
        Geometry {
            orientation,
            angle,
            crop,
        }
    }

    #[test]
    fn the_identity_geometry_is_the_identity_map() {
        let g = Geometry::default();
        let map = FrameMap::new(&g, 80, 40);
        let (x, y) = map.point([0.25, 0.75]);
        assert!((x - 20.0).abs() < 1e-3, "{x}");
        assert!((y - 30.0).abs() < 1e-3, "{y}");
    }

    #[test]
    fn frame_map_agrees_with_geometry_stage_on_a_crop() {
        check(
            geo(
                Orientation::default(),
                0.0,
                CropRect {
                    left: 0.2,
                    top: 0.1,
                    right: 0.9,
                    bottom: 0.8,
                },
            ),
            80,
            40,
            30,
            22,
        );
    }

    #[test]
    fn frame_map_agrees_with_geometry_stage_on_a_quarter_turn() {
        check(
            geo(
                Orientation {
                    rotate: Quarter::Cw90,
                    mirror: false,
                },
                0.0,
                CropRect::default(),
            ),
            80,
            40,
            30,
            12,
        );
    }

    #[test]
    fn frame_map_agrees_with_geometry_stage_on_a_mirror() {
        check(
            geo(
                Orientation {
                    rotate: Quarter::Zero,
                    mirror: true,
                },
                0.0,
                CropRect::default(),
            ),
            80,
            40,
            30,
            12,
        );
    }

    #[test]
    fn frame_map_agrees_with_geometry_stage_on_a_mirrored_turn() {
        // ⚠ The discriminating case for the non-abelian composition the orientation work
        // recorded: mirror-then-turn is not turn-then-mirror, and only a combined case can tell
        // them apart.
        check(
            geo(
                Orientation {
                    rotate: Quarter::Cw270,
                    mirror: true,
                },
                0.0,
                CropRect::default(),
            ),
            80,
            40,
            22,
            9,
        );
    }

    #[test]
    fn frame_map_agrees_with_geometry_stage_on_a_straighten_angle() {
        check(geo(Orientation::default(), 7.0, CropRect::default()), 80, 40, 26, 14);
    }

    #[test]
    fn frame_map_agrees_with_geometry_stage_on_everything_at_once() {
        check(
            geo(
                Orientation {
                    rotate: Quarter::Cw90,
                    mirror: true,
                },
                5.0,
                CropRect {
                    left: 0.1,
                    top: 0.15,
                    right: 0.85,
                    bottom: 0.9,
                },
            ),
            80,
            40,
            34,
            18,
        );
    }

    #[test]
    fn the_radius_scale_is_the_source_long_edge_in_frame_pixels() {
        // A half-crop doubles the pre-crop size, so a radius of 0.1 of the long edge covers
        // twice as many frame pixels as it would uncropped.
        let full = FrameMap::new(&Geometry::default(), 80, 40);
        assert!((full.radius_scale - 80.0).abs() < 1e-3, "{}", full.radius_scale);
        let half = FrameMap::new(
            &geo(
                Orientation::default(),
                0.0,
                CropRect {
                    left: 0.25,
                    top: 0.25,
                    right: 0.75,
                    bottom: 0.75,
                },
            ),
            40,
            20,
        );
        assert!((half.radius_scale - 80.0).abs() < 1e-3, "{}", half.radius_scale);
    }
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p pipeline frame_map 2>&1 | tail -10`
Expected: FAIL — `cannot find type FrameMap` / `file not found for module brush`.

- [ ] **Step 3: Write the implementation**

Put this **above** the test module in `crates/pipeline/src/brush/frame_map.rs`:

```rust
//! The forward map from pre-geometry source coordinates into frame pixels.
//!
//! ⚠ `GeometryStage` is written entirely as **inverse** maps — every output pixel asks where it
//! came from — because that is what resampling needs. Splatting a brush dab needs the opposite
//! direction, so all three steps are inverted here. The tests check this against
//! `GeometryStage` itself rather than against a second derivation.

use edit_model::{CropRect, Geometry, Quarter};

/// Maps a point in pre-geometry **source** normalised coordinates to **frame** pixels.
pub struct FrameMap {
    orientation: edit_model::Orientation,
    sin: f32,
    cos: f32,
    crop: CropRect,
    /// Width over height of the oriented, pre-crop image.
    aspect_o: f32,
    fw: f32,
    fh: f32,
    /// Multiplies a radius given as a fraction of the source long edge into frame pixels.
    pub radius_scale: f32,
}

/// Below this a crop side is treated as degenerate; it only guards a division.
const CROP_EPS: f32 = 1e-6;

impl FrameMap {
    pub fn new(g: &Geometry, frame_w: usize, frame_h: usize) -> Self {
        let cw = (g.crop.right - g.crop.left).max(CROP_EPS);
        let ch = (g.crop.bottom - g.crop.top).max(CROP_EPS);
        // The oriented, pre-crop dimensions at this render scale. ⚠ Derived rather than passed
        // in: the pipeline is built before the image flows through it, so nothing upstream knows
        // them at construction time.
        let ow = frame_w as f32 / cw;
        let oh = frame_h as f32 / ch;
        let (sin, cos) = g.angle.to_radians().sin_cos();
        Self {
            orientation: g.orientation,
            sin,
            cos,
            crop: g.crop,
            aspect_o: ow / oh.max(CROP_EPS),
            fw: frame_w as f32,
            fh: frame_h as f32,
            // Orientation only permutes pixels, so the source long edge is the oriented one.
            radius_scale: ow.max(oh),
        }
    }

    /// `(x, y)` in frame pixels. May fall outside the frame — the caller clips.
    pub fn point(&self, p: [f32; 2]) -> (f32, f32) {
        // 1. Orientation. Mirror first, then turn — the forward of `orient`, which un-turns and
        //    then un-mirrors in source space. Dimensionless.
        let u = if self.orientation.mirror {
            1.0 - p[0]
        } else {
            p[0]
        };
        let v = p[1];
        let (ou, ov) = match self.orientation.rotate {
            Quarter::Zero => (u, v),
            Quarter::Cw90 => (1.0 - v, u),
            Quarter::Cw180 => (1.0 - u, 1.0 - v),
            Quarter::Cw270 => (v, 1.0 - u),
        };

        // 2. Straighten. ⚠ Rotation mixes the axes, so it needs an isotropic space: working in
        //    units of the oriented image's *height* gives one while needing only the aspect,
        //    never absolute dimensions. `GeometryStage`'s inverse is R(-θ), so this is R(θ).
        let (cx, cy) = (self.aspect_o * 0.5, 0.5);
        let (dx, dy) = (ou * self.aspect_o - cx, ov - cy);
        let rx = cx + dx * self.cos - dy * self.sin;
        let ry = cy + dx * self.sin + dy * self.cos;
        let (ru, rv) = (rx / self.aspect_o.max(CROP_EPS), ry);

        // 3. Crop, and 4. into frame pixels.
        let cw = (self.crop.right - self.crop.left).max(CROP_EPS);
        let ch = (self.crop.bottom - self.crop.top).max(CROP_EPS);
        (
            (ru - self.crop.left) / cw * self.fw,
            (rv - self.crop.top) / ch * self.fh,
        )
    }
}
```

Create `crates/pipeline/src/brush/mod.rs`:

```rust
//! Rasterizing a painted mask into a coverage map.

mod frame_map;

pub use frame_map::FrameMap;
```

And add to `crates/pipeline/src/lib.rs`, beside the other module declarations:

```rust
pub mod brush;
```

⚠ `stages::geometry` must be reachable from the test. Check with
`grep -n 'mod geometry\|pub mod stages' crates/pipeline/src/lib.rs crates/pipeline/src/stages/mod.rs`
and make `GeometryStage` and the `stages` module visible to sibling modules — `pub(crate)` is
enough and is preferable to widening the public API for a test.

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p pipeline frame_map 2>&1 | tail -14`
Expected: PASS — 8 tests.

⚠ If a `check` case fails by a large margin (tens of pixels) the derivation is wrong, not the
tolerance. **Do not widen the tolerance to make it pass** — that would convert the strongest test
in this plan into decoration. Print both coordinates and find the disagreeing step.

- [ ] **Step 5: Sabotage**

In `point`, swap the `Cw90` and `Cw270` arms.
Run: `cargo test -p pipeline frame_map_agrees 2>&1 | grep -cE 'FAILED'`
Expected: at least 2.

Restore, then flip the rotation sign: `rx = cx + dx*cos + dy*sin`.
Run: `cargo test -p pipeline frame_map_agrees_with_geometry_stage_on_a_straighten 2>&1 | tail -6`
Expected: **FAIL**.

Restore, then drop the mirror: `let u = p[0];`.
Run: `cargo test -p pipeline frame_map_agrees_with_geometry_stage_on_a_mirror 2>&1 | tail -6`
Expected: **FAIL**.

Restore all three, then `touch crates/pipeline/src/brush/frame_map.rs`.

- [ ] **Step 6: Commit**

```bash
git add crates/pipeline/src/brush crates/pipeline/src/lib.rs
git commit -F - <<'EOF'
feat(pipeline): FrameMap — source coordinates into frame pixels

GeometryStage is written entirely as inverse maps, because that is what
resampling needs; splatting a brush dab needs the forward direction, so all
three steps are inverted here.

⚠ The tests check the map against GeometryStage itself — mark a source pixel,
run the real stage, find where it landed — rather than against a second
derivation of the same maths. Re-deriving in a test proves only that the same
mistake was made twice, and this map inverts three separate inverse maps.

⚠ It needs no absolute pre-geometry dimensions: the rotation step needs only
the aspect, which derives exactly from the frame dims and the crop rect.
EOF
```

---

## Task 6: the rasterizer

**Files:**
- Create: `crates/pipeline/src/brush/raster.rs`
- Modify: `crates/pipeline/src/brush/mod.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/pipeline/src/brush/raster.rs` with its test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use edit_model::{Brush, BrushStroke, CropRect, Geometry};

    fn dot(x: f32, y: f32, radius: f32, hardness: f32, erase: bool) -> BrushStroke {
        BrushStroke {
            points: vec![[x, y]],
            radius,
            hardness,
            erase,
        }
    }

    fn brush(strokes: Vec<BrushStroke>) -> Brush {
        Brush {
            strokes,
            invert: false,
        }
    }

    #[test]
    fn an_empty_brush_covers_nothing() {
        // A mask added but not yet painted must be inert.
        let m = rasterize(&brush(vec![]), &Geometry::default(), 80, 40);
        assert_eq!(m.w * m.h, 0);
        assert_eq!(m.at(10, 10), 0);
    }

    #[test]
    fn a_single_point_stroke_paints_one_dab() {
        // ⚠ A click without a drag is one point. Interpolation must not require two.
        let m = rasterize(&brush(vec![dot(0.5, 0.5, 0.1, 100.0, false)]), &Geometry::default(), 80, 40);
        assert_eq!(m.at(40, 20), 255, "the centre must be fully covered");
        assert_eq!(m.at(2, 2), 0, "far away must be untouched");
    }

    #[test]
    fn a_dab_is_round_on_a_wide_frame() {
        // ⚠⚠ The aspect trap in its fourth guise. On an 80x40 frame a radius of 0.125 of the
        // long edge is 10 px in BOTH directions. Measured against width it would be 10 px
        // across and 10/40*80 = 20 px down — an ellipse.
        let m = rasterize(&brush(vec![dot(0.5, 0.5, 0.125, 100.0, false)]), &Geometry::default(), 80, 40);
        // 8 px out on each axis is inside a 10 px radius; 12 px out is outside.
        assert_eq!(m.at(48, 20), 255, "8 px right must be covered");
        assert_eq!(m.at(40, 28), 255, "8 px down must be covered");
        assert_eq!(m.at(52, 20), 0, "12 px right must be outside");
        assert_eq!(m.at(40, 32), 0, "12 px down must be outside");
    }

    #[test]
    fn a_soft_dab_ramps_from_the_centre_outward() {
        let m = rasterize(&brush(vec![dot(0.5, 0.5, 0.25, 0.0, false)]), &Geometry::default(), 80, 40);
        let centre = m.at(40, 20);
        let mid = m.at(50, 20);
        let edge = m.at(59, 20);
        assert_eq!(centre, 255);
        assert!(mid > 0 && mid < 255, "mid = {mid}");
        assert!(edge < mid, "edge {edge} must be below mid {mid}");
    }

    #[test]
    fn a_hard_brush_edge_is_not_nan() {
        // ⚠⚠ The guard is only reachable at exactly `d == r`: on either side IEEE arithmetic
        // gives the right answer anyway (+inf and -inf both clamp correctly). Probing either
        // side of the boundary would pass with the guard deleted — the exact mistake M3.1b's
        // `feather_zero_is_a_hard_edge` made and had to be corrected for.
        //
        // A radius of 0.125 of an 80 px long edge is exactly 10 px, so (50, 20) is exactly on
        // the boundary from a centre at (40, 20).
        let m = rasterize(&brush(vec![dot(0.5, 0.5, 0.125, 100.0, false)]), &Geometry::default(), 80, 40);
        let on_boundary = m.at(50, 20);
        assert!(
            on_boundary == 255 || on_boundary == 0,
            "the boundary produced {on_boundary}, which is neither covered nor clear — a NaN \
             reached the quantizer"
        );
    }

    #[test]
    fn erase_after_paint_removes_coverage() {
        let m = rasterize(
            &brush(vec![
                dot(0.5, 0.5, 0.2, 100.0, false),
                dot(0.5, 0.5, 0.1, 100.0, true),
            ]),
            &Geometry::default(),
            80,
            40,
        );
        assert_eq!(m.at(40, 20), 0, "the erased core must be clear");
        assert_eq!(m.at(52, 20), 255, "outside the eraser must survive");
    }

    #[test]
    fn paint_after_erase_restores_coverage() {
        // Order matters, so an eraser is not terminal.
        let m = rasterize(
            &brush(vec![
                dot(0.5, 0.5, 0.2, 100.0, false),
                dot(0.5, 0.5, 0.1, 100.0, true),
                dot(0.5, 0.5, 0.05, 100.0, false),
            ]),
            &Geometry::default(),
            80,
            40,
        );
        assert_eq!(m.at(40, 20), 255);
    }

    #[test]
    fn overlapping_dabs_in_one_stroke_do_not_double_up() {
        // ⚠ `max`, not accumulation. Without it a slow stroke would be darker than a fast one
        // over the same path, which is the stroke-overlap problem that made flow a later
        // milestone.
        let long = BrushStroke {
            points: vec![[0.3, 0.5], [0.7, 0.5]],
            radius: 0.1,
            hardness: 0.0,
            erase: false,
        };
        let m = rasterize(&brush(vec![long]), &Geometry::default(), 80, 40);
        assert!(m.at(40, 20) <= 255);
        // A point midway along the path is covered by many dabs and must still be exactly full.
        assert_eq!(m.at(40, 20), 255);
    }

    #[test]
    fn dabs_do_not_gap_on_a_fast_stroke() {
        // Two points far apart, interpolated. Every pixel along the segment must be covered.
        let s = BrushStroke {
            points: vec![[0.1, 0.5], [0.9, 0.5]],
            radius: 0.05,
            hardness: 100.0,
            erase: false,
        };
        let m = rasterize(&brush(vec![s]), &Geometry::default(), 80, 40);
        for x in 12..68 {
            assert_eq!(m.at(x, 20), 255, "gap at x = {x}");
        }
    }

    #[test]
    fn the_bbox_bounds_the_painted_region() {
        // ⚠ The representation, not an optimisation: a full-frame map at full resolution is
        // 26 MB per mask.
        let m = rasterize(&brush(vec![dot(0.5, 0.5, 0.05, 100.0, false)]), &Geometry::default(), 80, 40);
        assert!(m.w < 20 && m.h < 20, "bbox {}x{} is not bounded", m.w, m.h);
        assert!(m.x0 > 20 && m.y0 > 5, "bbox origin ({}, {}) is wrong", m.x0, m.y0);
    }

    #[test]
    fn coverage_outside_the_bbox_reads_zero() {
        let m = rasterize(&brush(vec![dot(0.5, 0.5, 0.05, 100.0, false)]), &Geometry::default(), 80, 40);
        assert_eq!(m.at(0, 0), 0);
        assert_eq!(m.at(79, 39), 0);
        assert_eq!(m.at(1000, 1000), 0, "far out of range must not panic");
    }

    #[test]
    fn a_stroke_near_the_edge_is_clipped_not_wrapped() {
        let m = rasterize(&brush(vec![dot(0.02, 0.02, 0.1, 100.0, false)]), &Geometry::default(), 80, 40);
        assert_eq!(m.at(1, 1), 255);
        assert_eq!(m.at(79, 39), 0, "the far corner must not be painted");
    }

    #[test]
    fn a_stroke_survives_a_crop() {
        // ⚠⚠ The load-bearing test of the whole spec, and the natural wrong version of it fails
        // against a CORRECT implementation. Asserting "the bbox is unchanged" would be wrong —
        // the frame moved, that is the point. Assert instead that the dab lands on the SAME
        // IMAGE CONTENT: a stroke at source (0.5, 0.5) is the middle of the photo, so after a
        // crop it must sit at the point of the frame that corresponds to source (0.5, 0.5).
        let stroke = brush(vec![dot(0.5, 0.5, 0.05, 100.0, false)]);
        let crop = CropRect {
            left: 0.25,
            top: 0.25,
            right: 0.75,
            bottom: 0.75,
        };
        let g = Geometry {
            crop,
            ..Geometry::default()
        };
        // The cropped frame is half the size in each axis, and source (0.5, 0.5) is its centre.
        let m = rasterize(&stroke, &g, 40, 20);
        assert_eq!(m.at(20, 10), 255, "the stroke must still be on the same content");
        // And with no crop, the same source point is the centre of the full frame.
        let full = rasterize(&stroke, &Geometry::default(), 80, 40);
        assert_eq!(full.at(40, 20), 255);
    }

    #[test]
    fn an_eight_bit_edge_has_no_visible_banding() {
        // ⚠ Measured rather than assumed. The worst case is the softest possible edge across
        // the widest possible radius: consecutive samples must never differ by more than one
        // step of 1/255, which is below the visibility threshold for a smooth gradient.
        let m = rasterize(&brush(vec![dot(0.5, 0.5, 0.4, 0.0, false)]), &Geometry::default(), 80, 40);
        let mut worst = 0i32;
        for x in 40..72 {
            let a = m.at(x, 20) as i32;
            let b = m.at(x + 1, 20) as i32;
            worst = worst.max((a - b).abs());
        }
        assert!(worst <= 8, "worst adjacent step was {worst}/255");
    }
}
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p pipeline raster 2>&1 | tail -10`
Expected: FAIL — `cannot find function rasterize`.

- [ ] **Step 3: Write the implementation**

Put this above the test module in `crates/pipeline/src/brush/raster.rs`:

```rust
//! Splatting strokes into a coverage map.

use super::FrameMap;
use edit_model::{Brush, BrushStroke, Geometry};

/// A rasterized brush mask: 8-bit coverage over a bounding box in **frame** pixels.
///
/// ⚠ Bounded by what was actually painted, and that is the representation rather than an
/// optimisation bolted on. A full-frame map at full resolution is 6246x4170 = 26 MB per mask,
/// and several masks would run to hundreds. Bounding makes memory proportional to painted area
/// *and* gives `MaskStage` an early-out for every pixel outside the box.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CoverageMap {
    pub x0: usize,
    pub y0: usize,
    pub w: usize,
    pub h: usize,
    /// `w * h` samples.
    pub data: Vec<u8>,
}

impl CoverageMap {
    /// Coverage at a **frame** pixel. Everything outside the box reads 0.
    pub fn at(&self, x: usize, y: usize) -> u8 {
        if x < self.x0 || y < self.y0 || x >= self.x0 + self.w || y >= self.y0 + self.h {
            return 0;
        }
        self.data[(y - self.y0) * self.w + (x - self.x0)]
    }

    /// Whether this map covers nothing at all.
    pub fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }
}

/// Dabs are laid this fraction of a radius apart along a stroke, so a fast drag leaves no gaps.
const SPACING: f32 = 0.25;
/// Below this a hardness is treated as a hard edge; the only thing between hardness 100 and a
/// division by zero.
const HARDNESS_EPS: f32 = 1e-4;
/// A radius below this in frame pixels cannot paint anything visible.
const MIN_RADIUS_PX: f32 = 0.5;

/// Rasterize a brush into a coverage map over the frame.
pub fn rasterize(brush: &Brush, geometry: &Geometry, frame_w: usize, frame_h: usize) -> CoverageMap {
    if brush.strokes.is_empty() || frame_w == 0 || frame_h == 0 {
        return CoverageMap::default();
    }
    let map = FrameMap::new(geometry, frame_w, frame_h);

    // Every dab, in frame pixels, so the bounding box can be computed before allocating.
    let mut dabs: Vec<(f32, f32, f32, f32, bool)> = Vec::new();
    for s in &brush.strokes {
        let r = (s.radius * map.radius_scale).max(MIN_RADIUS_PX);
        let hard = (s.hardness / 100.0).clamp(0.0, 1.0);
        let pts: Vec<(f32, f32)> = s.points.iter().map(|p| map.point(*p)).collect();
        if pts.is_empty() {
            continue;
        }
        // ⚠ A single point still paints one dab: a click without a drag is one point, so
        // interpolation must not require two.
        dabs.push((pts[0].0, pts[0].1, r, hard, s.erase));
        for pair in pts.windows(2) {
            let (a, b) = (pair[0], pair[1]);
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let len = (dx * dx + dy * dy).sqrt();
            let step = (r * SPACING).max(0.5);
            let n = (len / step).ceil() as usize;
            for i in 1..=n {
                let t = i as f32 / n as f32;
                dabs.push((a.0 + dx * t, a.1 + dy * t, r, hard, s.erase));
            }
        }
    }
    if dabs.is_empty() {
        return CoverageMap::default();
    }

    // Bounding box, clipped to the frame.
    let (mut lx, mut ly, mut hx, mut hy) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
    for (x, y, r, _, _) in &dabs {
        lx = lx.min(x - r);
        ly = ly.min(y - r);
        hx = hx.max(x + r);
        hy = hy.max(y + r);
    }
    let x0 = lx.floor().max(0.0) as usize;
    let y0 = ly.floor().max(0.0) as usize;
    let x1 = (hx.ceil().max(0.0) as usize + 1).min(frame_w);
    let y1 = (hy.ceil().max(0.0) as usize + 1).min(frame_h);
    if x1 <= x0 || y1 <= y0 {
        return CoverageMap::default();
    }
    let (w, h) = (x1 - x0, y1 - y0);

    let mut cov = vec![0.0f32; w * h];
    for (cx, cy, r, hard, erase) in dabs {
        // Only the rows and columns this dab can reach.
        let dx0 = ((cx - r).floor().max(x0 as f32) as usize).min(x1);
        let dy0 = ((cy - r).floor().max(y0 as f32) as usize).min(y1);
        let dx1 = ((cx + r).ceil().max(0.0) as usize + 1).min(x1);
        let dy1 = ((cy + r).ceil().max(0.0) as usize + 1).min(y1);
        for y in dy0..dy1 {
            for x in dx0..dx1 {
                let d = (((x as f32 + 0.5) - cx).powi(2) + ((y as f32 + 0.5) - cy).powi(2)).sqrt();
                let a = dab_alpha(d, r, hard);
                if a <= 0.0 {
                    continue;
                }
                let slot = &mut cov[(y - y0) * w + (x - x0)];
                // ⚠ `max`, not accumulation. Accumulation would make a slow stroke darker than
                // a fast one over the same path, because dabs overlap — the stroke-overlap
                // problem that made flow a later milestone rather than part of this one.
                if erase {
                    *slot = slot.min(1.0 - a);
                } else {
                    *slot = slot.max(a);
                }
            }
        }
    }

    let invert = brush.invert;
    let data = cov
        .iter()
        .map(|c| {
            let c = if invert { 1.0 - c } else { *c };
            // ⚠ NaN reaches 0 through the cast, not the clamp: `f32::clamp` propagates NaN,
            // while Rust's float-to-int casts saturate. The same note `ImageBuf::quantize`
            // carries.
            (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
        })
        .collect();

    CoverageMap {
        x0,
        y0,
        w,
        h,
        data,
    }
}

/// A dab's alpha at distance `d` from its centre.
///
/// Smoothstep from 1 at `d/r = hard` to 0 at `d/r = 1` — the mirror of `RadialGradient`'s
/// feather, with `hard = 1 - feather`.
fn dab_alpha(d: f32, r: f32, hard: f32) -> f32 {
    if r <= 0.0 {
        return 0.0;
    }
    let q = d / r;
    if q >= 1.0 {
        return 0.0;
    }
    // ⚠ The guard is only reachable at exactly `q == hard == 1`, where the expression below is
    // `0.0 / 0.0`. On either side IEEE arithmetic already gives the right answer, which is why
    // `a_hard_brush_edge_is_not_nan` must probe the boundary itself.
    if hard >= 1.0 - HARDNESS_EPS {
        return 1.0;
    }
    let t = ((1.0 - q) / (1.0 - hard)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
```

Extend `crates/pipeline/src/brush/mod.rs`:

```rust
//! Rasterizing a painted mask into a coverage map.

mod frame_map;
mod raster;

pub use frame_map::FrameMap;
pub use raster::{rasterize, CoverageMap};
```

⚠ `Brush` gained an `invert` field in Task 4, so the test helper `brush()` sets it. If Task 4 put
`invert` elsewhere, match the real shape.

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p pipeline raster 2>&1 | tail -16`
Expected: PASS — 14 tests.

⚠ If `a_dab_is_round_on_a_wide_frame` or `a_stroke_survives_a_crop` fails, **fix the code, not the
expected coordinates.** Those two encode the spec's two central claims, and adjusting their
numbers to match the output would erase exactly the property they exist to protect.

- [ ] **Step 5: Sabotage**

Change `let r = (s.radius * map.radius_scale)` to use `frame_w as f32` instead of
`map.radius_scale`.
Run: `cargo test -p pipeline a_dab_is_round 2>&1 | tail -8`
Expected: **FAIL** on the vertical assertion.

Restore, then change `*slot = slot.max(a)` to `*slot += a`.
Run: `cargo test -p pipeline an_eight_bit_edge 2>&1 | tail -8`
Expected: **FAIL** — accumulation saturates the soft ramp into a hard step.

Restore, then remove the `hard >= 1.0 - HARDNESS_EPS` branch.
Run: `cargo test -p pipeline a_hard_brush_edge_is_not_nan 2>&1 | tail -8`
Expected: **FAIL** — the boundary produces a value that is neither 0 nor 255.

Restore, then raise `SPACING` to `4.0`.
Run: `cargo test -p pipeline dabs_do_not_gap 2>&1 | tail -8`
Expected: **FAIL** with a gap.

Restore, then delete the single-point push (`dabs.push((pts[0].0, ...))`).
Run: `cargo test -p pipeline a_single_point_stroke 2>&1 | tail -8`
Expected: **FAIL**.

Restore all five, then `touch crates/pipeline/src/brush/raster.rs`.

- [ ] **Step 6: Commit**

```bash
git add crates/pipeline/src/brush
git commit -F - <<'EOF'
feat(pipeline): the brush rasterizer

Dabs splatted along each polyline at quarter-radius spacing, combined with
`max` within and across strokes, into an 8-bit coverage map bounded by what
was actually painted.

⚠ The bounding box is the representation rather than an optimisation: a
full-frame map at full resolution is 26 MB per mask, and bounding it also
gives MaskStage the early-out deferred at 3.1a and again at 3.1b.

⚠ `max` rather than accumulation is why flow is a later milestone. Dabs
overlap, so accumulating would make a slow stroke darker than a fast one over
the same path — the bookkeeping that buys nothing until flow exists.

⚠ The hard-edge guard is reachable only at exactly d == r, so its test probes
the boundary itself. Probing either side passes with the guard deleted, which
is the mistake M3.1b's feather test made.
EOF
```

---

## Task 7: `CoverageSource` in `MaskStage`

**Files:**
- Modify: `crates/pipeline/src/stages/mask.rs`
- Modify: `crates/pipeline/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Add a test module to `crates/pipeline/src/stages/mask.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use edit_model::{Brush, BrushStroke, Edits, LocalAdjustments, Mask, MaskedAdjustment};

    fn painted() -> MaskedAdjustment {
        MaskedAdjustment {
            mask: Mask::Brush(Brush {
                strokes: vec![BrushStroke {
                    points: vec![[0.5, 0.5]],
                    radius: 0.2,
                    hardness: 100.0,
                    erase: false,
                }],
                invert: false,
            }),
            adjustments: LocalAdjustments {
                exposure: 1.0,
                ..Default::default()
            },
        }
    }

    fn flat(w: usize, h: usize) -> ImageBuf {
        let mut img = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                img.set_pixel(x, y, [0.25, 0.25, 0.25]);
            }
        }
        img
    }

    #[test]
    fn a_brush_mask_brightens_only_where_it_was_painted() {
        let stage = MaskStage::new(&painted(), RenderScale::FULL, &edit_model::Geometry::default());
        let out = stage.apply(flat(80, 40));
        assert!(out.pixel(40, 20)[0] > 0.4, "the painted centre must be lifted");
        assert!(
            (out.pixel(2, 2)[0] - 0.25).abs() < 1e-6,
            "the unpainted corner must be untouched"
        );
    }

    #[test]
    fn an_unpainted_brush_mask_changes_nothing() {
        // ⚠ A mask added but not yet painted must be exactly inert — not merely close.
        let m = MaskedAdjustment {
            mask: Mask::Brush(Brush::default()),
            ..painted()
        };
        let stage = MaskStage::new(&m, RenderScale::FULL, &edit_model::Geometry::default());
        let src = flat(80, 40);
        let out = stage.apply(src.clone());
        assert_eq!(out, src);
    }

    #[test]
    fn a_brush_mask_in_a_region_matches_the_same_part_of_the_whole() {
        // The region path must agree with the whole-frame path pixel for pixel, which is what
        // makes zooming safe. Same property `a_region_render_matches_the_whole_render_cropped`
        // pins for the pipeline at large.
        let whole = MaskStage::new(&painted(), RenderScale::FULL, &edit_model::Geometry::default())
            .apply(flat(80, 40));
        let region = Region {
            full_w: 80,
            full_h: 40,
            x0: 30,
            y0: 10,
        };
        let mut sub = ImageBuf::new(20, 20);
        for y in 0..20 {
            for x in 0..20 {
                sub.set_pixel(x, y, [0.25, 0.25, 0.25]);
            }
        }
        let got = MaskStage::new_in_region(
            &painted(),
            RenderScale::FULL,
            Some(region),
            &edit_model::Geometry::default(),
        )
        .apply(sub);
        for y in 0..20 {
            for x in 0..20 {
                assert_eq!(
                    got.pixel(x, y),
                    whole.pixel(30 + x, 10 + y),
                    "at ({x}, {y})"
                );
            }
        }
    }

    #[test]
    fn the_analytic_masks_are_unaffected() {
        // The refactor must not disturb the two existing shapes.
        let m = MaskedAdjustment {
            mask: Mask::Radial(edit_model::RadialGradient::default()),
            adjustments: LocalAdjustments {
                exposure: 1.0,
                ..Default::default()
            },
        };
        let out = MaskStage::new(&m, RenderScale::FULL, &edit_model::Geometry::default())
            .apply(flat(80, 40));
        assert!(out.pixel(40, 20)[0] > 0.4, "the ellipse centre must be lifted");
        assert!((out.pixel(1, 1)[0] - 0.25).abs() < 1e-6, "the corner is outside");
    }

    #[test]
    fn an_unused_edits_field_is_not_silently_dropped() {
        // A tripwire: `Edits` must still carry masks after all this restructuring.
        assert!(Edits::default().masks.is_empty());
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p pipeline mask:: 2>&1 | tail -10`
Expected: FAIL — `MaskStage::new` takes 2 arguments.

- [ ] **Step 3: Rewrite `MaskStage`**

⚠ **The brush is stored unrasterized and rasterized in `apply`.** With no region the frame *is*
the image, and its dimensions are not known at construction — the pipeline is built before the
image flows through it. Rasterizing eagerly would need a size nobody can supply.

In `crates/pipeline/src/stages/mask.rs`, replace the struct and both constructors:

```rust
/// Where a mask's coverage comes from.
///
/// ⚠ Resolved **once at construction**, so `apply` matches on it outside the pixel loop. This is
/// the shape a brush forces: `Mask::analytic_coverage` is a pure function of position and a
/// brush is sampled data, so the two cannot share one code path without one of them lying.
enum CoverageSource {
    /// Linear and radial.
    Analytic(edit_model::Mask),
    /// Brush. ⚠ Kept unrasterized: the frame's dimensions are not known until `apply`, because
    /// with no region the frame is the image itself. Rasterizing on every render is correct and
    /// slow — 3.1c-B's cache is what makes it cheap, and the CLI walks this path once.
    Painted(edit_model::Brush, Geometry),
}

pub struct MaskStage {
    masked: MaskedAdjustment,
    source: CoverageSource,
    stages: Vec<Box<dyn PipelineStage>>,
    region: Option<Region>,
}

impl MaskStage {
    /// For a render covering the whole frame.
    pub fn new(masked: &MaskedAdjustment, scale: RenderScale, geometry: &Geometry) -> Self {
        Self::new_in_region(masked, scale, None, geometry)
    }

    /// For a render covering only part of the frame — coverage is still measured against the
    /// full frame, so a cropped region is masked exactly as that part of the whole would be.
    ///
    /// ⚠ `geometry` is needed only by a brush, whose strokes are in **pre-geometry source**
    /// coordinates and must be mapped forward into frame pixels. The analytic shapes ignore it.
    pub fn new_in_region(
        masked: &MaskedAdjustment,
        scale: RenderScale,
        region: Option<Region>,
        geometry: &Geometry,
    ) -> Self {
        // No spatial sub-stage yet: every adjustment a mask can carry is per-pixel.
        let _ = scale;
        let source = match &masked.mask {
            edit_model::Mask::Brush(b) => CoverageSource::Painted(b.clone(), *geometry),
            other => CoverageSource::Analytic(other.clone()),
        };
        Self {
            masked: masked.clone(),
            source,
            stages: per_pixel_stages(&masked.adjustments),
            region,
        }
    }
}
```

⚠ `Geometry` is `Copy`, so `*geometry` is a copy. Add `use edit_model::Geometry;` to the file's
imports.

- [ ] **Step 4: Rasterize and sample in `apply`**

In `apply`, after `fw`, `fh`, `x0`, `y0` and `aspect` are computed and before the `image.clone()`:

```rust
        // ⚠ Whole-frame, never region-sized. Coverage is defined over the frame, so a
        // region-sized raster would move under panning — the same class of bug the mask overlay
        // hit in 3.1a.
        let coverage = match &self.source {
            CoverageSource::Analytic(_) => None,
            CoverageSource::Painted(b, g) => Some(crate::brush::rasterize(b, g, fw, fh)),
        };
        // An unpainted brush covers nothing, so the stage cannot change a pixel. The early
        // return skips a full-image clone and a full run of every sub-stage.
        if coverage.as_ref().is_some_and(|c| c.is_empty()) {
            return image;
        }
```

and replace the per-pixel coverage lookup:

```rust
        image.map_pixels_xy(|x, y, base| {
            let c = match (&self.source, &coverage) {
                (CoverageSource::Analytic(m), _) => {
                    // Pixel position in the *frame's* normalised coordinates.
                    let u = (x0 + x) as f32 / fw as f32;
                    let v = (y0 + y) as f32 / fh as f32;
                    m.analytic_coverage(u, v, aspect).unwrap_or(0.0)
                }
                // ⚠ Indexed by FRAME pixel, not by the region-relative `(x, y)`.
                (CoverageSource::Painted(..), Some(cov)) => cov.at(x0 + x, y0 + y) as f32 / 255.0,
                (CoverageSource::Painted(..), None) => 0.0,
            }
            .clamp(0.0, 1.0);
            // ... the existing blend, unchanged ...
```

- [ ] **Step 5: Update `build_pipeline_split_in_region`**

In `crates/pipeline/src/lib.rs`, the `MaskStage` construction becomes:

```rust
    for m in &edits.masks {
        if m.adjustments.is_active() {
            tail.push(Box::new(MaskStage::new_in_region(
                m,
                scale,
                region,
                &edits.geometry,
            )));
        }
    }
```

⚠ **`build_pipeline`, `build_pipeline_split` and `build_pipeline_split_in_region` keep their own
signatures.** That is the property worth protecting — the CLI, export and all 35 goldens must
compile untouched.

- [ ] **Step 6: Verify**

Run: `cargo test -p pipeline mask:: 2>&1 | tail -10`
Expected: PASS — 5 tests.

Run: `cargo clippy --all-targets --all-features -- -D warnings 2>&1 | grep -cE '^(error|warning)'`
Expected: `0`.

Run: `git status --short crates/pipeline/tests/golden/`
Expected: no output.

Run: `cargo test --all 2>&1 | grep -E '^test result' | grep -v 'ok\.'`
Expected: no output.

- [ ] **Step 7: Sabotage**

In `apply`, change the painted branch to `cov.at(x, y)` (region-relative rather than
frame-relative).
Run: `cargo test -p pipeline a_brush_mask_in_a_region 2>&1 | tail -8`
Expected: **FAIL**.

Restore, then delete the `c.is_empty()` early return.
Run: `cargo test -p pipeline an_unpainted_brush_mask 2>&1 | tail -8`
Expected: this may still pass, since an empty map reads 0 everywhere and a zero blend is the
identity. ⚠ **If it passes, that is a finding about the early return, not the test**: the return
is then a pure optimisation rather than a correctness guard, so **say so in its comment** and keep
the test as a guard on inertness rather than on the branch.

Restore, then `touch crates/pipeline/src/stages/mask.rs`.

- [ ] **Step 8: Commit**

```bash
git add crates/pipeline/src
git commit -F - <<'EOF'
feat(pipeline): MaskStage resolves a coverage source

Analytic masks answer a pure function of position; a brush is rasterized. The
match happens outside the pixel loop.

⚠ The brush is rasterized in `apply`, not at construction: with no region the
frame *is* the image, and the pipeline is built before the image flows through
it. Correct and slow — 3.1c-B's cache is what makes it cheap, and the CLI
walks this path once.

⚠ The coverage lookup is frame-relative, never region-relative. A
region-relative one would move under panning, which is the same class of bug
the mask overlay hit in 3.1a.

The build_pipeline family keeps its signatures, so the CLI, export and all 35
goldens compile untouched — and all 35 stayed byte-identical.
EOF
```

---

## Task 8: Goldens

**Files:**
- Modify: `crates/pipeline/tests/golden.rs`
- Create: `crates/pipeline/tests/golden/mask_brush_exposure.png`
- Create: `crates/pipeline/tests/golden/mask_brush_erase.png`

- [ ] **Step 1: Add the two cases**

Append to `crates/pipeline/tests/golden.rs`:

```rust
#[test]
fn golden_mask_brush_exposure() {
    // A horizontal stroke across the middle of the 64x16 chart, soft-edged.
    //
    // ⚠ +0.55 EV rather than a larger lift, for the reason M3.1b's radial golden recorded: at
    // +1.0 EV a large share of samples pin at 255 and the reference can no longer discriminate
    // the stroke's peak coverage from any greater value.
    check_golden(
        "mask_brush_exposure",
        &Edits {
            masks: vec![edit_model::MaskedAdjustment {
                mask: edit_model::Mask::Brush(edit_model::Brush {
                    strokes: vec![edit_model::BrushStroke {
                        points: vec![[0.2, 0.5], [0.8, 0.5]],
                        radius: 0.12,
                        hardness: 40.0,
                        erase: false,
                    }],
                    invert: false,
                }),
                adjustments: edit_model::LocalAdjustments {
                    exposure: 0.55,
                    ..Default::default()
                },
            }],
            ..Edits::default()
        },
    );
}

#[test]
fn golden_mask_brush_erase() {
    // A broad stroke with a narrower eraser through it: the reference shows a painted band with
    // a clear channel down its middle, which is the ordering rule made visible.
    check_golden(
        "mask_brush_erase",
        &Edits {
            masks: vec![edit_model::MaskedAdjustment {
                mask: edit_model::Mask::Brush(edit_model::Brush {
                    strokes: vec![
                        edit_model::BrushStroke {
                            points: vec![[0.1, 0.5], [0.9, 0.5]],
                            radius: 0.2,
                            hardness: 60.0,
                            erase: false,
                        },
                        edit_model::BrushStroke {
                            points: vec![[0.3, 0.5], [0.7, 0.5]],
                            radius: 0.08,
                            hardness: 100.0,
                            erase: true,
                        },
                    ],
                    invert: false,
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

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p pipeline --test golden mask_brush 2>&1 | grep -E 'missing golden|test result'`
Expected: "missing golden mask_brush_exposure".

- [ ] **Step 3: Generate**

Run: `UPDATE_GOLDEN=1 cargo test -p pipeline --test golden mask_brush 2>&1 | grep 'test result'`
Expected: PASS.

- [ ] **Step 4: Look at them, and measure**

⚠ **Not optional.** A golden nobody has looked at is just a hash — that lesson cost a whole chart
redesign during the noise-reduction work, and M3.1b's radial golden had to be regenerated because
looking was not enough.

```bash
SP=$(mktemp -d)
for n in mask_brush_exposure mask_brush_erase; do
  sips -s format png -Z 512 "crates/pipeline/tests/golden/$n.png" --out "$SP/${n}_big.png" >/dev/null 2>&1
done
open "$SP"/*_big.png
```

Expect: `mask_brush_exposure` shows a brightened horizontal band across the middle two colour
bands, fading softly at its ends and edges. `mask_brush_erase` shows a grey, darkened band with a
clear stripe of original colour running down its centre.

Then **measure the clipping**, exactly as the radial golden had to:

```bash
python3 -c "
import zlib,struct
d=open('crates/pipeline/tests/golden/mask_brush_exposure.png','rb').read()
i,idat,w,h=8,b'',None,None
while i<len(d):
    ln=struct.unpack('>I',d[i:i+4])[0]; t=d[i+4:i+8]; b=d[i+8:i+8+ln]
    if t==b'IHDR': w,h=struct.unpack('>II',b[:8])
    elif t==b'IDAT': idat+=b
    i+=12+ln
raw=zlib.decompress(idat); st=w*3; prev=bytearray(st); rows=[]; p=0
for y in range(h):
    f=raw[p]; p+=1; ln=bytearray(raw[p:p+st]); p+=st
    if f==1:
        for x in range(3,st): ln[x]=(ln[x]+ln[x-3])&255
    elif f==2:
        for x in range(st): ln[x]=(ln[x]+prev[x])&255
    elif f==3:
        for x in range(st):
            a=ln[x-3] if x>=3 else 0
            ln[x]=(ln[x]+((a+prev[x])>>1))&255
    elif f==4:
        for x in range(st):
            a=ln[x-3] if x>=3 else 0; bb=prev[x]; c=prev[x-3] if x>=3 else 0
            pp=a+bb-c; pa,pb,pc=abs(pp-a),abs(pp-bb),abs(pp-c)
            pr=a if (pa<=pb and pa<=pc) else (bb if pb<=pc else c)
            ln[x]=(ln[x]+pr)&255
    rows.append(bytes(ln)); prev=ln
clip=sum(1 for x in range(w) if rows[10][x*3+1]==255)
print('clipped samples in the green band:', clip, 'of', w)
"
```

⚠ If more than a handful clip, **lower the exposure and regenerate**. A golden that pins at 255
cannot discriminate the stroke's peak coverage from any larger value.

- [ ] **Step 5: Verify the existing 35 are byte-identical**

Run: `git status --short crates/pipeline/tests/golden/`
Expected: **exactly two `??` lines** and no `M` lines. If any existing golden shows modified,
**stop** — something in the shared path changed.

Run: `cargo test -p pipeline --test golden 2>&1 | grep 'test result'`
Expected: 37 passed.

- [ ] **Step 6: Commit**

```bash
git add crates/pipeline/tests/golden.rs crates/pipeline/tests/golden/mask_brush_exposure.png crates/pipeline/tests/golden/mask_brush_erase.png
git commit -F - <<'EOF'
test(pipeline): goldens for the brush mask

A soft stroke lifting exposure, and a broad stroke with an eraser through it
so the ordering rule is visible in the reference.

The existing 35 came out byte-identical, which is the proof the variant is
additive and nothing in the shared path shifted.
EOF
```

---

## Task 9: Real-RAW validation and documentation

**Files:**
- Modify: `CLAUDE.md`
- Modify: `docs/superpowers/specs/2026-08-18-brush-headless-design.md`

- [ ] **Step 1: Full verification**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | grep -cE '^(error|warning)'
cargo test --all 2>&1 | grep -oE '[0-9]+ passed' | awk '{s+=$1} END {print s}'
```
Expected: no fmt output, `0` clippy warnings, roughly 640 tests.

- [ ] **Step 2: Validate the anchoring claim on a real RAW**

⚠ **The RAW case must be cropped, because pre-geometry anchoring is the entire claim of this spec
and only a geometry change can falsify it.** A stroke on an uncropped photo proves nothing — the
same shape as the crop-factor bug that was invisible on 400 uncropped files and total on the one
cropped one.

Find a RAW:

```bash
find ~/Desktop ~/Pictures -maxdepth 4 -iname '*.RAF' 2>/dev/null | head -3
```

Write two sidecars with **the same stroke** and different crops:

```bash
SP=$(mktemp -d); echo "$SP"
cat > "$SP/nocrop.json" <<'EOF'
{
  "masks": [
    {
      "mask": { "Brush": { "strokes": [ { "points": [[0.35,0.45],[0.55,0.55]],
        "radius": 0.08, "hardness": 50.0, "erase": false } ], "invert": false } },
      "adjustments": { "exposure": -2.0 }
    }
  ]
}
EOF
sed 's/"masks"/"geometry": { "crop": { "left": 0.2, "top": 0.1, "right": 0.9, "bottom": 0.85 } }, "masks"/' \
  "$SP/nocrop.json" > "$SP/cropped.json"
```

Render both, substituting your RAW:

```bash
cargo run --release -q -p rustroom -- render <YOUR.RAF> "$SP/nocrop.jpg" --edits "$SP/nocrop.json"
cargo run --release -q -p rustroom -- render <YOUR.RAF> "$SP/cropped.jpg" --edits "$SP/cropped.json"
```

Open both. **The darkened stroke must sit on the same subject in each**, at a different place in
the frame. If it sits at the same *frame position* in both, the anchoring is frame-relative and
the central claim of this spec is unimplemented.

- [ ] **Step 3: Fill in the spec's outcome**

Add an "Outcome" section to
`docs/superpowers/specs/2026-08-18-brush-headless-design.md` recording: the real test count, any
sabotage step that did not fail first time and how the test was strengthened, whether the
`c.is_empty()` early return turned out to be a correctness guard or an optimisation, and the
result of the cropped-RAW comparison.

⚠ Where the spec was wrong, say so explicitly.

- [ ] **Step 4: Update `CLAUDE.md`**

- **▶ NEXT ACTION** becomes **M3.1c-B — the brush canvas**, which closes M3.1.
- Replace the parked "M3.1c decisions" block with a section for what actually landed. The ⚠ items
  that must appear: (1) **a brush is not a fourth analytic shape** and why `analytic_coverage`
  returns an `Option`; (2) **strokes are pre-geometry, the analytic masks are frame-relative**,
  and why the asymmetry is principled; (3) **radius is a fraction of the long edge** — the aspect
  trap's fourth guise, resolved by unit choice; (4) **the bbox is the representation**, closing
  the memory question and the twice-deferred early-out together; (5) **`FrameMap` is tested
  against `GeometryStage` itself**, not against a re-derivation; (6) **`Mask` is no longer
  `Copy`**; (7) **`max` not accumulation**, and that this is why flow waits.
- Note that 3.1c-B still owes: painting, brush cursor, tint overlay, panel controls, and the
  coverage cache (whose key must not include the region).
- Update the test count, add both filenames to the docs list, refresh the push-state line.

- [ ] **Step 5: Commit and report**

```bash
git add CLAUDE.md docs/superpowers/specs/2026-08-18-brush-headless-design.md docs/superpowers/plans/2026-08-18-brush-headless.md
git commit -F - <<'EOF'
docs: record M3.1c-A, the headless brush

⚠ The structural finding stands: a brush is not a fourth analytic shape.
`Mask::coverage` is a pure function of position and a brush is sampled data,
so MaskStage resolves a coverage *source* and `analytic_coverage` returns an
Option rather than a stub that would silently cover nothing.
EOF
git log origin/main..HEAD --oneline | wc -l
```

⚠ **Do not push.**
