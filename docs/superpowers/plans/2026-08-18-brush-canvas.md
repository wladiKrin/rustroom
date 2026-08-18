# M3.1c-B — Brush Canvas Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make brush strokes authorable on the canvas — painting, cursor, coverage tint, panel controls and a rasterization cache — closing M3.1.

**Architecture:** `FrameMap` gains an inverse so a pointer position becomes a source coordinate. The canvas builds that map at **screen** dimensions, which makes one map serve both painting and the tint and collapses the spec's separate radius conversion into a single division. Painting appends directly into `Edits`, so the render path needs no preview branch. A `BrushCache` beside `PrefixCache` keeps unrelated slider drags from re-rasterizing.

**Tech Stack:** Rust, eframe/egui 0.35, rayon.

**Spec:** `docs/superpowers/specs/2026-08-18-brush-canvas-design.md`

---

## Before you start

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

**Package is `app`** (`rustroom_app` is its lib target). **No `Co-Authored-By`. Never push.**
**Branch first** — three features in a row have, and the user merges.

⚠ **After writing a test, break the thing it names, confirm the sabotage COMPILED, and watch it
fail.** Eleven recorded cases in this project of a test that could not detect its subject, plus one
sabotage that failed to compile while looking exactly like a pass. Use this helper:

```bash
sab () {
  if cargo build -p "$3" --tests 2>&1 | grep -q '^error'; then echo "$1: DID NOT COMPILE — invalid"
  else echo "$1: failures=$(cargo test -p "$3" "$2" 2>&1 | grep -cE 'FAILED')"; fi; }
```

⚠ **The three ways those tests were blind** — check each new test against all three:
**no leverage**; **a sabotage that is a no-op on the chosen fixture**; **an assertion band wide
enough to contain the bug**.

⚠ Items in the `app` **binary** (`develop/*`, `theme.rs`) fail `clippy -D warnings` as dead code
without a caller — introduce each with its first caller. `edit-model`, `pipeline` and
`rustroom_app`'s lib are exempt.

---

## A simplification over the spec

The spec described an "inverse radius scale":
`radius_source = radius_screen / (screen_px_per_frame_px × radius_scale)`.

⚠ **That composition is unnecessary and is one more place to get a factor wrong.** The image is
drawn preserving aspect, so the on-screen rect has the frame's aspect ratio — and every step of
`FrameMap` except the final multiply is normalised. Building the map with the **screen rect's**
dimensions therefore yields a map whose outputs are in *screen* pixels, and whose `radius_scale` is
already screen-pixels-per-source-long-edge. The conversion collapses to:

```rust
let radius_source = radius_screen / map.radius_scale;
```

The same map then serves painting *and* the tint, so there is one construction and one set of
units to reason about rather than two.

---

## File structure

| File | Responsibility | Change |
|---|---|---|
| `crates/pipeline/src/brush/frame_map.rs` | `inverse_point` | Modify |
| `crates/pipeline/src/brush/raster.rs` | accept a prebuilt map | (unchanged; see Task 5) |
| `crates/app/src/brush_tool.rs` | Pure point-decimation and stroke assembly | Create |
| `crates/app/src/develop/canvas.rs` | Painting, tint, cursor | Modify |
| `crates/app/src/develop/panel.rs` | Add-brush button, Size/Hardness/Erase | Modify |
| `crates/app/src/render_engine.rs` | `BrushCache` | Modify |
| `crates/pipeline/src/stages/mask.rs` | Prebuilt coverage | Modify |
| `crates/pipeline/src/lib.rs` | `BrushCoverage`, the optional parameter | Modify |

⚠ `brush_tool.rs` is in the **lib**, beside `crop_tool` and `mask_tool`, for the same reason those
are: the decimation rule is arithmetic that must be testable without a window, and it is the one
part of painting that can be wrong silently.

---

## Task 1: `FrameMap::inverse_point`

**Files:**
- Modify: `crates/pipeline/src/brush/frame_map.rs`

- [ ] **Step 1: Write the failing tests**

Add to `crates/pipeline/src/brush/frame_map.rs`'s test module:

```rust
    /// A geometry with real leverage in every component.
    ///
    /// ⚠ 12° and an off-centre probe are not arbitrary. 3.1c-A's sabotage proved that a rotation
    /// sign error displaces a point *in proportion to its distance from the centre*: a 7° probe at
    /// 0.34 of the half-width moved only 1.34 px and passed against broken code.
    fn leveraged() -> Geometry {
        geo(
            Orientation {
                rotate: Quarter::Cw90,
                mirror: true,
            },
            12.0,
            CropRect {
                left: 0.1,
                top: 0.15,
                right: 0.85,
                bottom: 0.9,
            },
        )
    }

    #[test]
    fn inverse_point_round_trips_the_forward_map() {
        let g = leveraged();
        let map = FrameMap::new(&g, 120, 90);
        // Probes well away from the centre on both axes, where a sign or permutation error has
        // room to show.
        for p in [[0.15, 0.2], [0.85, 0.25], [0.2, 0.8], [0.9, 0.85], [0.5, 0.5]] {
            let (x, y) = map.point(p);
            let back = map.inverse_point(x, y);
            assert!(
                (back[0] - p[0]).abs() < 1e-4 && (back[1] - p[1]).abs() < 1e-4,
                "{p:?} -> ({x:.2}, {y:.2}) -> {back:?}"
            );
        }
    }

    #[test]
    fn inverse_point_agrees_with_geometry_stage() {
        // ⚠⚠ A round trip alone is NOT sufficient, and this test exists for exactly that reason:
        // a forward and an inverse that are wrong in mirrored ways round-trip perfectly. This
        // anchors the pair to the real transform — put a mark in a source image, run
        // `GeometryStage`, and ask the inverse where the mark's frame position came from.
        let g = leveraged();
        let (w, h) = (120usize, 90usize);
        let (sx, sy) = (34usize, 61usize);
        let (fw, fh, gx, gy) = run(&g, w, h, sx, sy);
        let map = FrameMap::new(&g, fw, fh);
        let back = map.inverse_point(gx, gy);
        let want = [(sx as f32 + 0.5) / w as f32, (sy as f32 + 0.5) / h as f32];
        // Loose because the brightest-pixel search quantises to whole frame pixels, and one frame
        // pixel is ~1/120 of the source width.
        assert!(
            (back[0] - want[0]).abs() < 0.02 && (back[1] - want[1]).abs() < 0.02,
            "inverse says {back:?}, source was {want:?}"
        );
    }

    #[test]
    fn the_radius_scale_is_screen_relative_when_built_at_screen_size() {
        // ⚠ The property the canvas relies on to convert a screen-point brush size into a source
        // radius with a single division. Built at the on-screen rect's size, `radius_scale` is
        // screen-pixels-per-source-long-edge.
        let map = FrameMap::new(&Geometry::default(), 700, 350);
        assert!((map.radius_scale - 700.0).abs() < 1e-3, "{}", map.radius_scale);
        // A 35 px brush is a twentieth of the long edge.
        assert!((35.0 / map.radius_scale - 0.05).abs() < 1e-6);
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p pipeline inverse_point 2>&1 | tail -8`
Expected: FAIL — `no method named inverse_point`.

- [ ] **Step 3: Write the implementation**

Add to `impl FrameMap` in `crates/pipeline/src/brush/frame_map.rs`:

```rust
    /// The inverse of [`FrameMap::point`]: frame pixels back to pre-geometry source normalised
    /// coordinates.
    ///
    /// ⚠ Painting needs this direction. `point` exists because splatting a dab goes source →
    /// frame; a pointer gives a frame position and the stroke must be *recorded* in source
    /// coordinates, which is the whole basis of the anchoring guarantee.
    ///
    /// ⚠ An error here lands strokes *near* the pointer rather than obviously wrong, so it is
    /// guarded twice: a round trip against `point`, and agreement with `GeometryStage`. The round
    /// trip alone would not do — a forward and inverse wrong in mirrored ways round-trip
    /// perfectly.
    pub fn inverse_point(&self, x: f32, y: f32) -> [f32; 2] {
        // 4'. Frame pixels to normalised, then un-crop.
        let cw = (self.crop.right - self.crop.left).max(CROP_EPS);
        let ch = (self.crop.bottom - self.crop.top).max(CROP_EPS);
        let ru = self.crop.left + (x / self.fw.max(CROP_EPS)) * cw;
        let rv = self.crop.top + (y / self.fh.max(CROP_EPS)) * ch;

        // 3'. Un-rotate, in units of the oriented image's height so the space is isotropic — the
        //     same requirement `point` has. `point` applies R(θ), so this is R(-θ).
        let (cx, cy) = (self.aspect_o * 0.5, 0.5);
        let (dx, dy) = (ru * self.aspect_o - cx, rv - cy);
        let ox = cx + dx * self.cos + dy * self.sin;
        let oy = cy - dx * self.sin + dy * self.cos;
        let (ou, ov) = (ox / self.aspect_o.max(CROP_EPS), oy);

        // 2'. Un-permute the orientation: the inverse of `point`'s table.
        let (mut u, v) = match self.orientation.rotate {
            Quarter::Zero => (ou, ov),
            Quarter::Cw90 => (ov, 1.0 - ou),
            Quarter::Cw180 => (1.0 - ou, 1.0 - ov),
            Quarter::Cw270 => (1.0 - ov, ou),
        };

        // 1'. Un-mirror.
        if self.orientation.mirror {
            u = 1.0 - u;
        }
        [u, v]
    }
```

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p pipeline inverse_point 2>&1 | tail -8`
Expected: PASS — 2 tests. Then
`cargo test -p pipeline the_radius_scale_is_screen_relative 2>&1 | tail -4` — PASS.

⚠ If the round trip fails, fix the **permutation table** first: the inverse of `Cw90 → (1−v, u)`
is `(v, 1−u)`, which is easy to write as `(1−v, u)` by copying. Do not widen the tolerance.

- [ ] **Step 5: Sabotage**

```bash
sab () {
  if cargo build -p "$3" --tests 2>&1 | grep -q '^error'; then echo "$1: DID NOT COMPILE — invalid"
  else echo "$1: failures=$(cargo test -p "$3" "$2" 2>&1 | grep -cE 'FAILED')"; fi; }
cp crates/pipeline/src/brush/frame_map.rs /tmp/fm.bak
```

Drop the un-mirror (`if self.orientation.mirror { u = 1.0 - u; }`), then
`sab "S1 no un-mirror" inverse_point pipeline` — expect **failures ≥ 1**. Restore with
`cp /tmp/fm.bak crates/pipeline/src/brush/frame_map.rs; touch crates/pipeline/src/brush/frame_map.rs`.

Flip the un-rotation sign (`ox = cx + dx*cos - dy*sin`), then
`sab "S2 rotation sign" inverse_point pipeline` — expect **failures ≥ 1**. Restore.

Swap the `Cw90` and `Cw270` inverse arms, then
`sab "S3 permutation" inverse_point pipeline` — expect **failures ≥ 1**. Restore.

⚠ **If `inverse_point_round_trips_the_forward_map` passes any of these while
`inverse_point_agrees_with_geometry_stage` fails, that is the mirrored-error case the spec
predicted — and it is evidence the second test earns its place, not a reason to relax it.**

- [ ] **Step 6: Commit**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings 2>&1 | grep -cE '^(error|warning)'
git add crates/pipeline/src/brush/frame_map.rs
git commit -F - <<'EOF'
feat(pipeline): FrameMap::inverse_point

Painting needs frame -> source; 3.1c-A built only source -> frame, because
that is the direction splatting a dab needs.

⚠ Guarded twice on purpose. A round trip against the forward map would pass
for a forward/inverse pair wrong in mirrored ways, so agreement with
GeometryStage is a separate test rather than a nicety.

⚠ The fixtures carry leverage — 12 deg, a mirrored quarter turn, an off-centre
crop and probes far from the centre — because 3.1c-A proved a rotation sign
error at 0.34 of the half-width moves a point only 1.34 px and passes.
EOF
```

---

## Task 2: Tool state and panel controls

**Files:**
- Modify: `crates/app/src/main.rs`
- Modify: `crates/app/src/develop/panel.rs`

- [ ] **Step 1: Add the tool state**

In `crates/app/src/main.rs`, add to `RustroomApp`'s fields, beside the other view state:

```rust
    /// Brush radius in **screen points**.
    ///
    /// ⚠ Not persisted, and not in `Settings`. A screen-point size is view-dependent, so
    /// restoring last session's value at a different window size or zoom restores a different
    /// brush. The other three could be persisted, but splitting one tool's settings across two
    /// lifetimes is worse than one honest rule.
    brush_size: f32,
    /// 0 = maximally soft, 100 = a hard edge.
    brush_hardness: f32,
    /// Whether the next stroke subtracts.
    brush_erase: bool,
    /// Whether to tint the selected brush mask's coverage on the canvas.
    show_mask_tint: bool,
```

and in its constructor, beside the other defaults:

```rust
            brush_size: 40.0,
            brush_hardness: 50.0,
            brush_erase: false,
            // On by default: a freshly added brush mask with no adjustment yet is otherwise
            // completely invisible, which is a poor first five minutes.
            show_mask_tint: true,
```

⚠ Find the constructor with `grep -n 'selected_mask: None' crates/app/src/main.rs` — the session
fields and the app fields are initialised in different places, and these are **app** fields.

- [ ] **Step 2: Add the third add-button**

In `crates/app/src/develop/panel.rs`, inside the `ui.horizontal` that holds the two existing add
buttons:

```rust
                            if ui.button("Add brush").clicked() {
                                added = Some(edit_model::Mask::Brush(edit_model::Brush::default()));
                            }
```

⚠ `Brush::default()` has **no strokes**, so the mask is inert until painted — which is exactly
why the tint defaults on.

- [ ] **Step 3: Add the brush controls to the shape-properties block**

In the same file, in the block that already holds Invert and the radial's Feather, after the
Feather line:

```rust
                                if matches!(m.mask, edit_model::Mask::Brush(_)) {
                                    // ⚠ Brush settings live on the app, not on the mask: they are
                                    // the *tool's* state and apply to the next stroke, exactly as
                                    // a brush size does in any editor. Each stroke records its own
                                    // copy at the moment it is drawn.
                                    slider(ui, &mut brush_size, 4.0..=300.0, "Size");
                                    slider(ui, &mut brush_hardness, 0.0..=100.0, "Hardness");
                                    ui.checkbox(&mut brush_erase, "Erase");
                                    ui.checkbox(&mut show_mask_tint, "Show mask");
                                    theme::hint(ui, "Drag on the image to paint.");
                                }
```

⚠ **`brush_size` and friends must be copied into locals before the `CollapsingHeader` closure and
written back after it**, exactly as `modals/settings.rs` does and for the same reason: the closure
already borrows `app` through `s`, and a second capture is `E0499`. Add before the closure:

```rust
                    let mut brush_size = app.brush_size;
                    let mut brush_hardness = app.brush_hardness;
                    let mut brush_erase = app.brush_erase;
                    let mut show_mask_tint = app.show_mask_tint;
```

and after it:

```rust
                    app.brush_size = brush_size;
                    app.brush_hardness = brush_hardness;
                    app.brush_erase = brush_erase;
                    app.show_mask_tint = show_mask_tint;
```

⚠ These are **not** folded into `changed`/`settled`: changing a tool setting does not alter the
photograph, so it must not push a history entry or request a render.

- [ ] **Step 4: Verify**

Run: `cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings 2>&1 | grep -cE '^(error|warning)'`
Expected: `0`.

Run: `cargo test --all 2>&1 | grep -E '^test result' | grep -v 'ok\.'`
Expected: no output.

- [ ] **Step 5: Commit**

```bash
git add crates/app/src/main.rs crates/app/src/develop/panel.rs
git commit -F - <<'EOF'
feat(app): brush tool state and panel controls

An `Add brush` button and Size / Hardness / Erase / Show mask, shown only for a
brush selection.

⚠ Tool state lives on RustroomApp and is not persisted. A screen-point size is
view-dependent, so restoring it at a different window size restores a different
brush — and splitting one tool's settings across two lifetimes is worse than
one honest rule.

⚠ Not folded into changed/settled: a tool setting does not alter the
photograph, so it must not push history or request a render.
EOF
```

---

## Task 3: Painting

**Files:**
- Create: `crates/app/src/brush_tool.rs`
- Modify: `crates/app/src/lib.rs`
- Modify: `crates/app/src/develop/canvas.rs`

- [ ] **Step 1: Write the failing decimation tests**

Create `crates/app/src/brush_tool.rs`:

```rust
//! Pure stroke-assembly maths for the brush.
//!
//! Deliberately free of `egui` types, like `crop_tool` and `mask_tool` beside it: the decimation
//! rule is the one part of painting that can be wrong silently, so it must be testable without a
//! window.

/// Dabs are splatted a quarter of a radius apart, so recording points more finely than that adds
/// sidecar bytes and nothing else.
pub const SPACING_FRACTION: f32 = 0.25;

/// Whether a new point is far enough from the last recorded one to be worth keeping.
///
/// ⚠ Not tidiness. A drag emits a point every ~16 ms, so a three-second stroke is ~180 points,
/// most a pixel apart, and every one of them is serialised into the sidecar as JSON.
pub fn should_record(last: [f32; 2], next: [f32; 2], radius: f32) -> bool {
    let min = (radius * SPACING_FRACTION).max(1e-5);
    let (dx, dy) = (next[0] - last[0], next[1] - last[1]);
    (dx * dx + dy * dy).sqrt() >= min
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_point_within_the_spacing_is_dropped() {
        // radius 0.1 -> minimum spacing 0.025.
        assert!(!should_record([0.5, 0.5], [0.51, 0.5], 0.1));
        assert!(!should_record([0.5, 0.5], [0.5, 0.512], 0.1));
    }

    #[test]
    fn a_point_beyond_the_spacing_is_kept() {
        assert!(should_record([0.5, 0.5], [0.53, 0.5], 0.1));
        assert!(should_record([0.5, 0.5], [0.5, 0.6], 0.1));
    }

    #[test]
    fn decimation_scales_with_the_radius() {
        // ⚠ The discriminating case: the SAME offset must be dropped by a fat brush and kept by a
        // thin one. A test using one radius could not tell a radius-scaled rule from a fixed one.
        let off = [0.53, 0.5];
        assert!(!should_record([0.5, 0.5], off, 0.5), "fat brush should drop");
        assert!(should_record([0.5, 0.5], off, 0.02), "thin brush should keep");
    }

    #[test]
    fn a_zero_radius_still_terminates() {
        // A degenerate radius must not make the minimum spacing zero and record every event.
        assert!(should_record([0.5, 0.5], [0.5001, 0.5], 0.0));
    }
}
```

Register it in `crates/app/src/lib.rs` beside the other modules:

```rust
pub mod brush_tool;
```

- [ ] **Step 2: Run to verify they pass**

Run: `cargo test -p app brush_tool 2>&1 | grep 'test result' | head -1`
Expected: PASS — 4 tests.

- [ ] **Step 3: Sabotage the decimation**

```bash
cp crates/app/src/brush_tool.rs /tmp/bt.bak
```

Change `should_record` to `true` unconditionally, then
`sab "S1 no decimation" brush_tool app` — expect **failures ≥ 1**. Restore and `touch`.

Change `let min = (radius * SPACING_FRACTION)` to a constant `0.025`, then
`sab "S2 fixed spacing" decimation_scales_with_the_radius app` — expect **failures = 1**.
Restore and `touch`.

- [ ] **Step 4: Wire painting into the canvas**

In `crates/app/src/develop/canvas.rs`, inside `draw_mask_overlay`, replace the handle-drag block —
that is, everything from `if tool_drag_started(resp) {` through the closing brace of
`if tool_drag(resp) { ... }` — with a branch on the mask's kind:

```rust
        if let edit_model::Mask::Brush(_) = &mask {
            // ⚠ No paint mode. Left-drag is free because the navigation rework moved panning
            // onto scroll and a drag button, which is the whole reason that landed first.
            let map = pipeline::brush::FrameMap::new(
                &s.edits.geometry,
                frame_rect.width() as usize,
                frame_rect.height() as usize,
            );
            let radius = (app_brush_size / map.radius_scale).max(1e-4);
            let src = map.inverse_point(p.x - frame_rect.left(), p.y - frame_rect.top());
            if tool_drag_started(resp) {
                if let Some(edit_model::Mask::Brush(b)) =
                    s.edits.masks.get_mut(i).map(|m| &mut m.mask)
                {
                    b.strokes.push(edit_model::BrushStroke {
                        points: vec![src],
                        radius,
                        hardness: app_brush_hardness,
                        erase: app_brush_erase,
                    });
                    moved = true;
                }
            } else if tool_drag(resp) {
                if let Some(edit_model::Mask::Brush(b)) =
                    s.edits.masks.get_mut(i).map(|m| &mut m.mask)
                {
                    if let Some(stroke) = b.strokes.last_mut() {
                        let keep = stroke
                            .points
                            .last()
                            .is_none_or(|last| brush_tool::should_record(*last, src, radius));
                        if keep {
                            stroke.points.push(src);
                            moved = true;
                        }
                    }
                }
            }
            if resp.drag_stopped() {
                s.history.push(s.edits.clone());
                return true;
            }
        } else {
            if tool_drag_started(resp) {
                s.active_mask_handle = mask_tool::hit_test(&mask, [nx, ny], aspect, 0.04);
            }
            if tool_drag(resp) {
                if let Some(h) = s.active_mask_handle {
                    let d = resp.drag_delta();
                    let dx = d.x / frame_rect.width();
                    let dy = d.y / frame_rect.height();
                    if let Some(m) = s.edits.masks.get_mut(i) {
                        mask_tool::apply_drag(&mut m.mask, h, dx, dy);
                        moved = true;
                    }
                }
            }
            if resp.drag_stopped() && s.active_mask_handle.take().is_some() {
                s.history.push(s.edits.clone());
                return true;
            }
        }
```

⚠ **`else if tool_drag(resp)` rather than a second `if`** — `tool_drag_started` and `tool_drag` are
both true on the frame a drag begins, and without the `else` that frame would record the first
point twice.

⚠ `app_brush_size`, `app_brush_hardness` and `app_brush_erase` must be copied out of `app` in
`canvas::show` **before** `match &mut app.session` takes its borrow, exactly as `app_pan_drag`
already is. Add beside it:

```rust
    let app_brush_size = app.brush_size;
    let app_brush_hardness = app.brush_hardness;
    let app_brush_erase = app.brush_erase;
    let app_show_mask_tint = app.show_mask_tint;
```

and thread the four into `draw_mask_overlay`'s signature. Add
`use rustroom_app::brush_tool;` to the file's imports.

⚠ `is_none_or` needs Rust 1.82+; the toolchain is 1.97. If clippy prefers `map_or(true, ..)`,
follow clippy.

- [ ] **Step 5: Verify the render request survives**

The existing `if moved { s.request(...) }` at the end of `draw_mask_overlay` now covers painting
too. ⚠ **Do not remove or bypass it** — it is the fix for 3.1a's first real-use bug, where the
overlay mutated `Edits` and never asked for a frame, so the image showed the previous state until
some later edit happened to request one.

Run: `cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings 2>&1 | grep -E '^(error|warning)' -A5 | head -14`
Expected: no output.

Run: `cargo test --all 2>&1 | grep -E '^test result' | grep -v 'ok\.'`
Expected: no output.

- [ ] **Step 6: Commit**

```bash
git add crates/app/src/brush_tool.rs crates/app/src/lib.rs crates/app/src/develop/canvas.rs
git commit -F - <<'EOF'
feat(app): paint brush strokes on the canvas

A drag on a selected brush mask appends a stroke, decimated to the same
quarter-radius spacing the rasterizer splats at, so the recorded path is
exactly as dense as splatting needs and no denser.

⚠ No paint mode, which is the payoff of doing the navigation rework first:
left-drag was deliberately freed.

⚠ `else if tool_drag` rather than a second `if` — both predicates are true on
the frame a drag begins, and without the else that frame records its first
point twice.

⚠ The stroke is appended straight into `Edits`, so it renders as you draw with
no preview path — which is exactly why the existing `s.request` call must
survive. That call is the fix for 3.1a's first real-use bug.
EOF
```

---

## Task 4: The tint and the cursor

**Files:**
- Modify: `crates/app/src/develop/canvas.rs`

- [ ] **Step 1: Fill `paint_mask`'s brush arm**

Replace the empty brush arm in `paint_mask` with:

```rust
        edit_model::Mask::Brush(b) => {
            // ⚠ The canvas rasterizes its OWN coverage at screen resolution rather than reusing
            // the render worker's. That is a deliberate second rasterization: having the worker
            // return its map would give one source of truth but make the tint lag the stroke by a
            // full render, and for a brush, seeing the mark appear under the cursor *is* the
            // experience.
            //
            // ~1400x900 and sub-millisecond for any plausible stroke.
            if show_tint && !b.strokes.is_empty() {
                let (w, h) = (frame_rect.width() as usize, frame_rect.height() as usize);
                let cov = pipeline::brush::rasterize(b, geometry, w, h);
                if !cov.is_empty() {
                    // ⚠ Red because it is conventional and because none of the four accents is
                    // red, so the tint can never be confused with active state.
                    let mut mesh = egui::Mesh::default();
                    for y in 0..cov.h {
                        for x in 0..cov.w {
                            let a = cov.at(cov.x0 + x, cov.y0 + y);
                            if a == 0 {
                                continue;
                            }
                            let px = egui::Rect::from_min_size(
                                egui::pos2(
                                    frame_rect.left() + (cov.x0 + x) as f32,
                                    frame_rect.top() + (cov.y0 + y) as f32,
                                ),
                                egui::vec2(1.0, 1.0),
                            );
                            mesh.add_colored_rect(
                                px,
                                egui::Color32::from_rgba_unmultiplied(255, 40, 40, a / 2),
                            );
                        }
                    }
                    painter.add(egui::Shape::mesh(mesh));
                }
            }
        }
```

⚠ `paint_mask` needs two new parameters — `show_tint: bool` and `geometry: &edit_model::Geometry`
— and its caller passes `app_show_mask_tint` and `&s.edits.geometry`.

⚠ **If the per-pixel mesh proves slow in the GUI check (Task 7), that is the thing to replace**,
with a texture uploaded once per change rather than a quad per pixel. Building the texture path up
front would be optimising before measuring, which this project has been burned by; building the
mesh first keeps the change small enough to judge.

- [ ] **Step 2: Draw the brush cursor**

At the end of `draw_mask_overlay`, before the `if moved` block:

```rust
    // The brush cursor: a circle at the pointer at the current screen radius. ⚠ Without it the
    // Size slider is guesswork, because a screen-point size has no other on-screen
    // representation.
    if matches!(mask, edit_model::Mask::Brush(_)) {
        if let Some(p) = resp.hover_pos() {
            ui.painter_at(img_rect).circle_stroke(
                p,
                app_brush_size,
                egui::Stroke::new(1.0, egui::Color32::from_white_alpha(180)),
            );
        }
    }
```

⚠ `hover_pos`, not `interact_pointer_pos`: the cursor must show while merely hovering, before any
drag begins.

- [ ] **Step 3: Verify — and add no tests here, deliberately**

⚠ **This task ships with no new tests, and that is a decision rather than an omission.** The tint
and the cursor are egui painting with no branching logic worth pinning: their correctness is "it
looks right", which is what Task 7's GUI check is for. The rasterization they call is already
covered by 14 tests in `pipeline::brush::raster`. Padding the suite with a test that asserts a mesh
was constructed would add maintenance and catch nothing — this codebase has explicitly declined to
pad tests before, in the view-demosaic work.

Run: `cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings 2>&1 | grep -cE '^(error|warning)'`
Expected: `0`.

Run: `cargo test --all 2>&1 | grep -E '^test result' | grep -v 'ok\.'`
Expected: no output.

- [ ] **Step 4: Commit**

```bash
git add crates/app/src/develop/canvas.rs
git commit -F - <<'EOF'
feat(app): the brush coverage tint and cursor

⚠ The canvas rasterizes its own coverage at screen resolution rather than
reusing the worker's map. A deliberate second rasterization: reusing the
worker's would give one source of truth but make the tint lag the stroke by a
full render, and for a brush, seeing the mark appear under the cursor is the
experience.

Red at 50%, because none of the four accents is red, so it can never be
confused with active state.
EOF
```

---

## Task 5: The coverage cache

**Files:**
- Modify: `crates/pipeline/src/stages/mask.rs`
- Modify: `crates/pipeline/src/lib.rs`
- Modify: `crates/app/src/render_engine.rs`

- [ ] **Step 1: Let `MaskStage` accept a prebuilt map**

In `crates/pipeline/src/stages/mask.rs`, change the `Painted` variant and the constructor:

```rust
    /// Brush. ⚠ Kept unrasterized when no prebuilt map is supplied: the frame's dimensions are
    /// not known until `apply`, because with no region the frame *is* the image.
    Painted {
        brush: edit_model::Brush,
        geometry: Geometry,
        /// A map the caller already has, from `render_engine`'s cache. `None` on the CLI and
        /// export paths, which render once and have nothing to reuse.
        prebuilt: Option<std::sync::Arc<crate::brush::CoverageMap>>,
    },
```

`new_in_region` gains a fifth parameter `prebuilt: Option<Arc<CoverageMap>>` and stores it;
`new` passes `None`. In `apply`:

```rust
        let coverage = match &self.source {
            CoverageSource::Analytic(_) => None,
            CoverageSource::Painted {
                prebuilt: Some(c), ..
            } => Some(std::sync::Arc::clone(c)),
            CoverageSource::Painted { brush, geometry, .. } => Some(std::sync::Arc::new(
                crate::brush::rasterize(brush, geometry, fw, fh),
            )),
        };
```

⚠ **Five existing call sites will break** — `MaskStage::new` and `new_in_region` are called by
`build_pipeline_split_in_region` and by four tests in this file's own test module. `new` keeps its
arity by passing `None` internally, so only the `new_in_region` sites need the extra argument.

⚠ A prebuilt map **must** have been rasterized at the same `fw`/`fh`. The cache key includes them,
so this holds by construction; a mismatch would silently sample the wrong pixels rather than
erroring, which is why the key must never lose them.

- [ ] **Step 2: Add the pipeline seam**

In `crates/pipeline/src/lib.rs`:

```rust
/// Pre-rasterized coverage for a photo's brush masks, indexed by position in `Edits::masks`.
///
/// ⚠ Optional and additive, following the `Region` precedent: `build_pipeline` and
/// `build_pipeline_split` delegate with `None`, so the CLI, export and every golden compile
/// untouched.
#[derive(Default)]
pub struct BrushCoverage {
    pub maps: Vec<Option<std::sync::Arc<brush::CoverageMap>>>,
}

impl BrushCoverage {
    pub fn get(&self, i: usize) -> Option<std::sync::Arc<brush::CoverageMap>> {
        self.maps.get(i).and_then(|m| m.clone())
    }
}
```

Rename the existing three-argument `build_pipeline_split_in_region` body into a four-argument
`build_pipeline_split_in_region_with_brush(edits, scale, region, brush: Option<&BrushCoverage>)`,
and make the old name delegate with `None`. Inside, the mask loop becomes:

```rust
    for (i, m) in edits.masks.iter().enumerate() {
        if m.adjustments.is_active() {
            tail.push(Box::new(MaskStage::new_in_region(
                m,
                scale,
                region,
                &edits.geometry,
                brush.and_then(|b| b.get(i)),
            )));
        }
    }
```

⚠ **The index must be the position in `edits.masks`, not a counter over *active* masks.** An
inactive mask is skipped here but still occupies a slot in `BrushCoverage`, so a separate counter
would misalign every map after the first inactive one — the same sparse-versus-dense index bug this
codebase has now hit three times (the crop feedback loop, `render_max_dim`, the Library grid).

- [ ] **Step 3: Add the cache**

In `crates/app/src/render_engine.rs`, beside `PrefixCache`:

```rust
/// What a cached set of brush coverage maps was built from.
#[derive(PartialEq)]
struct BrushKey {
    /// Every mask's brush, by position — `None` for the analytic shapes.
    brushes: Vec<Option<edit_model::Brush>>,
    geometry: edit_model::Geometry,
    fw: usize,
    fh: usize,
}

/// Rasterized brush coverage, so an unrelated slider drag does not re-rasterize.
///
/// ⚠ **The region is not in the key**, exactly as `PrefixCache`'s is not: rasterize whole-frame
/// and sample the sub-rect, so panning never invalidates.
///
/// ⚠ It misses on every frame *while painting*, and that is correct — the strokes really did
/// change. Its job is that dragging Exposure with a brush mask present does not re-rasterize.
struct BrushCache {
    entries: Vec<(BrushKey, std::sync::Arc<pipeline::BrushCoverage>)>,
}

impl BrushCache {
    /// ⚠ Two, not one, and for the reason `PrefixCache::FINALS_CAP` is two: the frame dimensions
    /// are in the key, so a single entry would be evicted on every zoom change and re-rasterized
    /// coming back — reintroducing at the brush layer the cost that decision removed at the
    /// prefix layer.
    const CAP: usize = 2;
}
```

```rust
impl Default for BrushCache {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
        }
    }
}

impl BrushCache {
    /// ⚠ The key carries **all** masks (`None` for the analytic ones), not just the brushes, so
    /// the positional index cannot drift when a mask is added or removed.
    fn key(edits: &Edits, fw: usize, fh: usize) -> BrushKey {
        BrushKey {
            brushes: edits
                .masks
                .iter()
                .map(|m| match &m.mask {
                    edit_model::Mask::Brush(b) => Some(b.clone()),
                    _ => None,
                })
                .collect(),
            geometry: edits.geometry,
            fw,
            fh,
        }
    }

    fn get_or_build(
        &mut self,
        edits: &Edits,
        fw: usize,
        fh: usize,
    ) -> std::sync::Arc<pipeline::BrushCoverage> {
        let key = Self::key(edits, fw, fh);
        if let Some(i) = self.entries.iter().position(|(k, _)| *k == key) {
            // Promote the hit so the least recently used entry is the one evicted.
            let hit = self.entries.remove(i);
            let out = std::sync::Arc::clone(&hit.1);
            self.entries.insert(0, hit);
            return out;
        }
        let maps = edits
            .masks
            .iter()
            .map(|m| match &m.mask {
                edit_model::Mask::Brush(b) => Some(std::sync::Arc::new(
                    pipeline::brush::rasterize(b, &edits.geometry, fw, fh),
                )),
                _ => None,
            })
            .collect();
        let built = std::sync::Arc::new(pipeline::BrushCoverage { maps });
        self.entries.insert(0, (key, std::sync::Arc::clone(&built)));
        self.entries.truncate(Self::CAP);
        built
    }
}
```

- [ ] **Step 4: Use it in `render_cached`**

`render_cached` gains a `brush_cache: &mut BrushCache` parameter beside `cache: &mut PrefixCache`,
and the worker that owns the `PrefixCache` owns this one too.

⚠ **The frame dimensions must be the PREFIX OUTPUT's, not the region's.** `render_cached` already
computes the whole-buffer prefix result before cropping to the region — that image *is* the frame.
Passing the region's dimensions would rasterize at the wrong size and, because the dims are in the
key, would also thrash the cache on every pan.

Where the tail is built, replace the `build_pipeline_split_in_region` call with:

```rust
    let (fw, fh) = (mid.width(), mid.height());
    let coverage = brush_cache.get_or_build(edits, fw, fh);
    let (_, tail) = build_pipeline_split_in_region_with_brush(edits, scale, region, Some(&coverage));
```

⚠ Check the local name for the prefix output with
`grep -n 'let mid' crates/app/src/render_engine.rs` and use whatever it actually is; the point is
that it is the whole-frame image, before the region crop.

- [ ] **Step 5: Verify**

Run: `cargo test --all 2>&1 | grep -E '^test result' | grep -v 'ok\.'`
Expected: no output.

Run: `git status --short crates/pipeline/tests/golden/`
Expected: **no output.** No golden may move — the cache is a caching change and nothing else.

Run: `cargo clippy --all-targets --all-features -- -D warnings 2>&1 | grep -cE '^(error|warning)'`
Expected: `0`.

- [ ] **Step 6: Add the cache-correctness test**

In `crates/app/src/render_engine.rs`'s test module:

```rust
    #[test]
    fn a_cached_brush_frame_matches_an_uncached_one() {
        // ⚠ Every other test here would pass with the cache deleted. This one pushes the same
        // edits twice and asserts the second frame — served from the cache — is byte-identical to
        // the first. The same shape `every_cached_frame_matches_a_cold_render` has for the prefix.
        let mut cache = BrushCache::default();
        let edits = brush_edits();
        let a = cache.get_or_build(&edits, 200, 100);
        let b = cache.get_or_build(&edits, 200, 100);
        assert!(std::sync::Arc::ptr_eq(&a, &b), "the second call must hit");
        let c = cache.get_or_build(&edits, 400, 200);
        assert!(!std::sync::Arc::ptr_eq(&a, &c), "different dims must miss");
        let d = cache.get_or_build(&edits, 200, 100);
        assert!(
            std::sync::Arc::ptr_eq(&a, &d),
            "CAP=2 must keep both zoom levels warm"
        );
    }
```

with the fixture:

```rust
    fn brush_edits() -> Edits {
        Edits {
            masks: vec![edit_model::MaskedAdjustment {
                mask: edit_model::Mask::Brush(edit_model::Brush {
                    strokes: vec![edit_model::BrushStroke {
                        points: vec![[0.3, 0.5], [0.7, 0.5]],
                        radius: 0.1,
                        hardness: 50.0,
                        erase: false,
                    }],
                    invert: false,
                }),
                adjustments: edit_model::LocalAdjustments {
                    exposure: 1.0,
                    ..Default::default()
                },
            }],
            ..Edits::default()
        }
    }
```

⚠ The last assertion is the one that pins `CAP = 2` rather than 1. With a single entry the
400×200 build would have evicted the 200×100 one and this would fail — which is exactly the zoom
thrash the capacity exists to prevent.

- [ ] **Step 7: Sabotage**

Set `CAP` to 1, then `sab "S1 cap of one" a_cached_brush_frame app` — expect **failures = 1**.
Restore.

Remove `fw`/`fh` from `BrushKey`, then `sab "S2 dims out of key" a_cached_brush_frame app` —
expect **failures = 1**. Restore and `touch`.

- [ ] **Step 8: Commit**

```bash
git add crates/pipeline/src crates/app/src/render_engine.rs
git commit -F - <<'EOF'
feat: cache rasterized brush coverage

A BrushCache beside PrefixCache, keyed by (brushes, geometry, frame dims).

⚠ The region is not in the key, exactly as PrefixCache's is not, so panning
never invalidates. And CAP is 2 rather than 1 for the reason FINALS_CAP is:
frame dims ARE in the key, so one entry would be evicted on every zoom change.

⚠ The coverage index is the position in `edits.masks`, not a counter over
active masks. An inactive mask is skipped when building stages but still owns a
slot, so a separate counter would misalign every map after it — the fourth
sparse-versus-dense index bug this codebase would have had.

The build_pipeline family keeps its signatures; the CLI, export and all 37
goldens compile untouched and stayed byte-identical.
EOF
```

---

## Task 6: Measure, then decide

**Files:**
- Modify: `crates/app/examples/bench_preview.rs`

- [ ] **Step 1: Add a brush case to the bench**

Add a case to `crates/app/examples/bench_preview.rs`. Read the file first — each case is an
`(label, Edits)` pair fed through the same timing loop — and add:

```rust
        (
            "brush 50 dabs",
            Edits {
                masks: vec![edit_model::MaskedAdjustment {
                    mask: edit_model::Mask::Brush(edit_model::Brush {
                        // ⚠ Two endpoints, not 50 points: the rasterizer interpolates dabs at a
                        // quarter-radius spacing, so a stroke spanning 0.6 of a 6246 px frame at
                        // radius 0.02 yields roughly 50 dabs — which is what costs time. Recording
                        // 50 literal points would measure something the app never produces,
                        // because painting decimates to exactly this spacing.
                        strokes: vec![edit_model::BrushStroke {
                            points: vec![[0.2, 0.5], [0.8, 0.5]],
                            radius: 0.02,
                            hardness: 50.0,
                            erase: false,
                        }],
                        invert: false,
                    }),
                    adjustments: edit_model::LocalAdjustments {
                        exposure: 0.5,
                        ..Default::default()
                    },
                }],
                ..Edits::default()
            },
        ),
```

Report it at both the fit cap and the buffer's native long edge, as the file's existing cases are.

- [ ] **Step 2: Run it**

```bash
cargo run --release -p app --example bench_preview <YOUR.RAF>
```

Record the brush row at both resolutions.

- [ ] **Step 3: Decide against the stated threshold**

⚠ **The threshold was fixed in the spec before any code was written, so the decision is made
against a number rather than an impression.**

- **Under ~30 ms at 1:1** → ship the simple version. Record the measurement in the spec's Outcome
  section and **stop here**.
- **Over ~30 ms** → build incremental splatting: the cache holds the map for committed strokes and
  only the in-progress stroke's new dabs are splatted onto a copy. Add it as its own commit with
  the measurement in the message.

⚠ This ordering is not caution for its own sake. The progressive-sharpen feature was built on
reasoning, shipped, debugged three times and removed; the prefix cache's measurement redirected its
design before any code was written. **Do not skip the measurement and build the optimisation
anyway** — that is precisely the mistake.

- [ ] **Step 4: Commit**

```bash
git add crates/app/examples/bench_preview.rs
git commit -F - <<'EOF'
bench(app): a brush case in bench_preview

Measures rasterization at fit and at 1:1 so the incremental-splatting decision
is made against the threshold the spec fixed in advance.
EOF
```

---

## Task 7: GUI validation and documentation

**Files:**
- Modify: `CLAUDE.md`
- Modify: `docs/superpowers/specs/2026-08-18-brush-canvas-design.md`

- [ ] **Step 1: Full verification**

```bash
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | grep -cE '^(error|warning)'
cargo test --all 2>&1 | grep -oE '[0-9]+ passed' | awk '{s+=$1} END {print s}'
git status --short crates/pipeline/tests/golden/
```
Expected: no fmt output, `0` warnings, roughly 650 tests, and **no golden moved**.

- [ ] **Step 2: Launch the GUI**

⚠ This is a feel change and no test can judge it.

```bash
cargo run --release -p app
```

Run it in the background and ask the user to check, in this order:

1. **Add brush, then drag on the photo.** The stroke should appear under the cursor as you draw.
2. **The tint** — red, following the stroke, and toggled by `Show mask`.
3. **The cursor circle** tracks the Size slider.
4. **Erase** removes; unticking it and painting over restores.
5. **One Ctrl+Z per stroke**, not per point.
6. **⚠ The one that matters most: paint on the subject, then enter Geometry and change the crop.**
   The stroke must stay on the subject. That is the entire claim of 3.1c-A's anchoring, and this
   is the first time it is visible interactively.
7. **Zoom to 1:1 and paint.** If it feels detached from the pointer, say so — that is the
   incremental-splatting trigger, and it should agree with Task 6's number.
8. **The tint's cost:** if painting a large area makes the canvas sluggish while the render itself
   is fine, the per-pixel mesh is the suspect, not the rasterizer.

- [ ] **Step 3: Fill in the spec's Outcome**

Add an "Outcome" section to `docs/superpowers/specs/2026-08-18-brush-canvas-design.md` recording:
the measured brush rasterization times and which branch of the threshold was taken; any sabotage
that did not fail first time and how the test was strengthened; whether the round trip alone ever
caught something the `GeometryStage` agreement did not, or vice versa; and whether the per-pixel
tint mesh needed replacing.

⚠ Where the spec was wrong, say so explicitly.

- [ ] **Step 4: Update `CLAUDE.md`**

- **M3.1 IS NOW CLOSED.** Say so prominently, and change **▶ NEXT ACTION** to the **parked
  button/layout work**, which masking was blocking — see the "Deferred, user's call 2026-08-13"
  note, which should now be marked live rather than parked.
- Add a section for 3.1c-B in the house style. The ⚠ items that must appear: (1) **no paint mode**,
  and that this is the payoff of sequencing the navigation rework first; (2) **`inverse_point` is
  guarded twice**, and why a round trip alone is insufficient; (3) **the tint is a deliberate
  second rasterization**, traded for responsiveness; (4) **the cache's region-free key and
  `CAP = 2`**; (5) **the coverage index is positional over all masks**, not a counter over active
  ones; (6) the measured numbers and which branch was taken.
- Note what M3.1 did **not** deliver: flow, auto-mask, brush-size shortcuts, mask
  intersection/subtraction — and that the roadmap file in the Obsidian vault still needs the flow
  note.
- Update the test count, add both filenames to the docs list, refresh the push-state line.

- [ ] **Step 5: Commit and report**

```bash
git add CLAUDE.md docs/superpowers/specs/2026-08-18-brush-canvas-design.md docs/superpowers/plans/2026-08-18-brush-canvas.md
git commit -F - <<'EOF'
docs: record M3.1c-B — M3.1 is closed

Three mask primitives, all authorable on the canvas. The parked button/layout
work is unblocked.
EOF
git log origin/main..HEAD --oneline | wc -l
```

⚠ **Do not push.**
