# M3.1b — Radial Gradient Mask Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `Mask::Radial` — an axis-aligned elliptical mask with a feathered edge — as the second of M3.1's three mask primitives, and wire up the `invert` flag that 3.1a left with no UI.

**Architecture:** `RadialGradient` joins `LinearGradient` in `edit-model` as a second variant of the `Mask` enum. Coverage is `smoothstep` over the normalised elliptical distance, and — unlike every other mask function — needs **no aspect correction**, because anisotropic scaling maps an axis-aligned ellipse to an axis-aligned ellipse. `mask_tool` widens its `Handle` enum and changes `hit_test`/`apply_drag` to take `Mask` rather than `LinearGradient`, so the canvas's drag plumbing stays shape-agnostic. `pipeline`, `EditGroups`, `merge_from` and the sidecar format are untouched.

**Tech Stack:** Rust (Cargo workspace), serde, egui/eframe 0.35, rayon. Tests are `#[cfg(test)]` modules plus the checked-in golden PNGs in `crates/pipeline/tests/`.

**Spec:** `docs/superpowers/specs/2026-08-18-radial-mask-design.md`

---

## Before you start

Every `cargo` command in this plan needs libraw and lcms2 on the pkg-config path. Run this once per shell:

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

**Commit messages carry no `Co-Authored-By` trailer** — this is an explicit user preference.

**Never `git push`.** The user pushes to `origin` themselves.

**⚠ The standing rule of this codebase: after writing a test, break the thing it names and watch it fail.** Three recorded bugs in this project were tests that named something they could not detect. Every task below has an explicit sabotage step. When a sabotage step does *not* produce a failure, that is a finding about the test — stop and fix the test, do not wave it through.

**⚠ `mv file.bak file` fools cargo.** `mv` preserves the backup's older mtime, so cargo reuses the artifact built from the sabotaged source. Always `touch` the file after restoring it.

---

## File structure

| File | Responsibility | Change |
|---|---|---|
| `crates/edit-model/src/lib.rs` | The `RadialGradient` shape, its coverage function, the `Mask` variant and its accessors | Modify |
| `crates/app/src/mask_tool.rs` | Pure hit-testing and drag math for every mask shape | Modify |
| `crates/app/src/develop/canvas.rs` | Painting the overlay and routing drags | Modify |
| `crates/app/src/develop/panel.rs` | The Masks panel section | Modify |
| `crates/pipeline/tests/golden.rs` | Golden regression cases | Modify |
| `crates/pipeline/tests/golden/mask_radial_exposure.png` | New reference image | Create |
| `crates/pipeline/tests/golden/mask_radial_inverted_saturation.png` | New reference image | Create |

No new files. `crates/pipeline/src/stages/mask.rs` and `crates/pipeline/src/lib.rs` are **deliberately untouched** — that is the evidence the architecture held.

---

## Task 1: `RadialGradient` and its coverage function

Standalone: the type is added but not yet reachable from `Mask`, so the workspace stays green throughout.

**Files:**
- Modify: `crates/edit-model/src/lib.rs` (insert after the `impl LinearGradient` block, before `pub enum Mask`)

- [ ] **Step 1: Write the failing tests**

Add to the `#[cfg(test)] mod tests` block at the bottom of `crates/edit-model/src/lib.rs`:

```rust
    /// An ellipse in the middle of the frame, half as wide as the frame and 60% as tall.
    fn radial() -> RadialGradient {
        RadialGradient {
            center: [0.5, 0.5],
            radius: [0.25, 0.3],
            feather: 50.0,
            invert: false,
        }
    }

    #[test]
    fn radial_coverage_is_one_at_the_centre_and_zero_outside() {
        let r = radial();
        assert!((r.coverage(0.5, 0.5, 1.0) - 1.0).abs() < 1e-6);
        // Well outside on each axis.
        assert_eq!(r.coverage(0.95, 0.5, 1.0), 0.0);
        assert_eq!(r.coverage(0.05, 0.5, 1.0), 0.0);
        assert_eq!(r.coverage(0.5, 0.95, 1.0), 0.0);
        assert_eq!(r.coverage(0.5, 0.05, 1.0), 0.0);
        // Exactly on the boundary: the ramp has reached zero.
        assert!(r.coverage(0.75, 0.5, 1.0).abs() < 1e-6);
    }

    #[test]
    fn radial_coverage_is_independent_of_aspect() {
        // ⚠⚠ The load-bearing test of this whole feature, and the only thing standing between
        // this design and a future reader "fixing" the missing aspect correction that three
        // sibling functions all genuinely need.
        //
        // An axis-aligned ellipse is immune to the anisotropic scaling between normalised and
        // screen space, because that scaling maps an axis-aligned ellipse to an axis-aligned
        // ellipse. `LinearGradient::coverage` needs the correction only because it projects
        // onto an axis, and *perpendicularity* is what anisotropic scaling destroys.
        //
        // ⚠ The probe sits off-centre on BOTH axes. A point on either axis would be
        // insensitive to a correction applied to the other one — which is the precise shape of
        // the mistake `hit_testing_the_line_is_aspect_correct` made in 3.1a.
        let r = radial();
        let (u, v) = (0.62, 0.58);
        let base = r.coverage(u, v, 1.0);
        assert!(base > 0.0 && base < 1.0, "probe must land on the ramp: {base}");
        for aspect in [0.33, 1.5, 3.0, 10.0] {
            assert!(
                (r.coverage(u, v, aspect) - base).abs() < 1e-6,
                "aspect {aspect} changed coverage: {} vs {base}",
                r.coverage(u, v, aspect)
            );
        }
    }

    #[test]
    fn feather_zero_is_a_hard_edge() {
        let r = RadialGradient {
            feather: 0.0,
            ..radial()
        };
        // Just inside and just outside the boundary at u = 0.75.
        assert_eq!(r.coverage(0.74, 0.5, 1.0), 1.0);
        assert_eq!(r.coverage(0.76, 0.5, 1.0), 0.0);
        // ⚠ And no NaN escapes the division that feather 0 would otherwise perform.
        assert!(!r.coverage(0.74, 0.5, 1.0).is_nan());
    }

    #[test]
    fn feather_one_hundred_ramps_from_the_centre() {
        let r = RadialGradient {
            feather: 100.0,
            ..radial()
        };
        // At the centre, full; halfway to the boundary, strictly between; at it, zero.
        assert!((r.coverage(0.5, 0.5, 1.0) - 1.0).abs() < 1e-6);
        let mid = r.coverage(0.625, 0.5, 1.0);
        assert!(mid > 0.0 && mid < 1.0, "{mid}");
        assert!(r.coverage(0.75, 0.5, 1.0).abs() < 1e-6);
        // Contrast with feather 50, where the inner half is still flat at full coverage.
        assert!((radial().coverage(0.625, 0.5, 1.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn radial_invert_flips_coverage() {
        let r = RadialGradient {
            invert: true,
            ..radial()
        };
        assert!(r.coverage(0.5, 0.5, 1.0).abs() < 1e-6);
        assert!((r.coverage(0.95, 0.5, 1.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_zero_radius_radial_is_inert_not_nan() {
        // ⚠ Reachable only from a hand-edited sidecar — `Default` and the panel both build a
        // usable ellipse — but a NaN here would reach the blend and paint the frame with
        // garbage, exactly as `LinearGradient`'s zero-length guard prevents.
        //
        // The guard yields pre-invert coverage 0.0 and then falls through the invert, so an
        // inverted point-ellipse covers everything. That is the limit of the surrounding
        // behaviour rather than a special case: as the radius shrinks, an inverted mask covers
        // all but a vanishing point.
        let r = RadialGradient {
            radius: [0.0, 0.0],
            ..radial()
        };
        assert_eq!(r.coverage(0.5, 0.5, 1.0), 0.0);
        assert_eq!(r.coverage(0.1, 0.9, 2.0), 0.0);
        assert!(!r.coverage(0.5, 0.5, 1.0).is_nan());
        let inv = RadialGradient { invert: true, ..r };
        assert_eq!(inv.coverage(0.5, 0.5, 1.0), 1.0);
    }

    #[test]
    fn a_radial_default_is_a_usable_ellipse() {
        // ⚠ NOT a derived `Default`. Container-level `#[serde(default)]` fills missing fields
        // from the struct's `Default`, so a derived one would give every partial sidecar a
        // zero-radius point-ellipse — and `feather: 50` cannot come from `f32::default()`
        // under any arrangement.
        let d = RadialGradient::default();
        assert_eq!(d.center, [0.5, 0.5]);
        assert!(d.radius[0] > 0.0 && d.radius[1] > 0.0);
        assert_eq!(d.feather, 50.0);
        assert!(!d.invert);
        // It covers its own centre, which a derived default would not.
        assert!((d.coverage(0.5, 0.5, 1.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_partial_radial_json_gets_the_hand_written_defaults() {
        let r: RadialGradient = serde_json::from_str(r#"{"center":[0.25,0.75]}"#).unwrap();
        assert_eq!(r.center, [0.25, 0.75]);
        assert_eq!(r.radius, RadialGradient::default().radius);
        assert_eq!(r.feather, 50.0);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p edit-model radial 2>&1 | tail -20`
Expected: FAIL to compile — `cannot find struct RadialGradient in this scope`.

- [ ] **Step 3: Write the implementation**

Insert into `crates/edit-model/src/lib.rs`, immediately after the closing brace of `impl LinearGradient` and before `/// The mask primitives.`:

```rust
/// A radial filter. Coverage is 1 inside the ellipse and ramps to 0 at its boundary.
///
/// `center` is in **normalised frame coordinates**, as [`CropRect`] is. `radius[0]` is a
/// fraction of the frame's *width* and `radius[1]` a fraction of its *height* — so a mask means
/// the same thing on the 1600 px preview and on the full-resolution export, and the default
/// radius renders as an ellipse *similar to the frame*.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RadialGradient {
    pub center: [f32; 2],
    pub radius: [f32; 2],
    /// 0 = a hard edge at the ellipse, 100 = a ramp all the way from the centre.
    pub feather: f32,
    pub invert: bool,
}

/// ⚠ **Hand-written, not derived** — unlike [`LinearGradient`]. Container-level
/// `#[serde(default)]` fills a missing field from the *struct's* `Default`, so a derived one
/// would give every partial sidecar a zero-radius point-ellipse. `feather: 50` cannot come
/// from `f32::default()` in any case.
impl Default for RadialGradient {
    fn default() -> Self {
        Self {
            center: [0.5, 0.5],
            radius: [0.25, 0.25],
            feather: 50.0,
            invert: false,
        }
    }
}

impl RadialGradient {
    /// Below this the ramp is treated as a hard edge. The only thing between feather 0 and a
    /// division by zero — the same role `WIDTH_EPS` plays in the vignette.
    const FEATHER_EPS: f32 = 1e-4;

    /// Coverage at `(u, v)` in normalised frame coordinates, in `0.0..=1.0`.
    ///
    /// ⚠⚠ **`aspect` is deliberately unused, and this is not an oversight.** Every sibling
    /// mask function needs it: [`LinearGradient::coverage`] projects onto an axis and
    /// `mask_tool::hit_test` measures a screen-space distance, and the anisotropic scaling
    /// between normalised and screen space destroys *perpendicularity* and *distance*
    /// respectively. It does **not** destroy axis-alignment: an axis-aligned ellipse maps to an
    /// axis-aligned ellipse, so the expression below is already a correct on-screen ellipse.
    ///
    /// Rotation is precisely the feature that would bring the correction back, which is the
    /// main reason this shape is axis-aligned. `radial_coverage_is_independent_of_aspect` is
    /// the guard against someone "fixing" the omission.
    ///
    /// The parameter stays in the signature so [`Mask::coverage`] has one shape across
    /// variants.
    pub fn coverage(&self, u: f32, v: f32, _aspect: f32) -> f32 {
        let (rx, ry) = (self.radius[0], self.radius[1]);
        let c = if rx <= f32::EPSILON || ry <= f32::EPSILON {
            // A degenerate ellipse honestly covers nothing. Unlike `LinearGradient`'s guard,
            // which chooses 1.0 only because its alternative was NaN, 0.0 is the true answer
            // here and cannot paint garbage either.
            0.0
        } else {
            // Normalised elliptical distance: exactly 1.0 on the boundary.
            let dx = (u - self.center[0]) / rx;
            let dy = (v - self.center[1]) / ry;
            let d = (dx * dx + dy * dy).sqrt();
            let f = (self.feather / 100.0).clamp(0.0, 1.0);
            if f <= Self::FEATHER_EPS {
                if d <= 1.0 {
                    1.0
                } else {
                    0.0
                }
            } else {
                // Smoothstep rather than a linear ramp: a linear ramp in radial distance
                // leaves a visible crease ring where the derivative jumps. The vignette's
                // feather made the same choice for the same reason.
                let t = ((1.0 - d) / f).clamp(0.0, 1.0);
                t * t * (3.0 - 2.0 * t)
            }
        };
        if self.invert {
            1.0 - c
        } else {
            c
        }
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p edit-model radial 2>&1 | tail -20`
Expected: PASS — 7 tests (`radial_coverage_is_one_at_the_centre_and_zero_outside`, `radial_coverage_is_independent_of_aspect`, `feather_zero_is_a_hard_edge`, `feather_one_hundred_ramps_from_the_centre`, `radial_invert_flips_coverage`, `a_zero_radius_radial_is_inert_not_nan`, `a_radial_default_is_a_usable_ellipse`) plus `a_partial_radial_json_gets_the_hand_written_defaults`.

- [ ] **Step 5: Sabotage — the aspect test**

Change the first two lines of the `else` branch in `coverage` to apply a correction that is not needed:

```rust
            let dx = (u * _aspect - self.center[0] * _aspect) / rx;
```

Run: `cargo test -p edit-model radial_coverage_is_independent_of_aspect 2>&1 | tail -20`
Expected: **FAIL** with "aspect 0.33 changed coverage". Then revert the line, and run `touch crates/edit-model/src/lib.rs`.

- [ ] **Step 6: Sabotage — the feather guard and the radius guard**

Delete the `if f <= Self::FEATHER_EPS { ... } else` branch, keeping only the smoothstep body.
Run: `cargo test -p edit-model feather_zero_is_a_hard_edge 2>&1 | tail -20`
Expected: **FAIL** — `(1.0 - d) / 0.0` yields infinity, so the clamp gives 1.0 outside the ellipse where 0.0 is required.

Restore it, then change the radius guard's `0.0` to `1.0`.
Run: `cargo test -p edit-model a_zero_radius_radial_is_inert_not_nan 2>&1 | tail -20`
Expected: **FAIL** — `left: 1.0, right: 0.0`.

Restore, then swap the hand-written `Default` for `#[derive(Default)]` on the struct.
Run: `cargo test -p edit-model a_radial_default 2>&1 | tail -20`
Expected: **FAIL** on `d.radius[0] > 0.0`.

Restore all three, then `touch crates/edit-model/src/lib.rs`.

- [ ] **Step 7: Verify the whole crate is green**

Run: `cargo test -p edit-model 2>&1 | tail -5`
Expected: PASS, all tests.

- [ ] **Step 8: Commit**

```bash
git add crates/edit-model/src/lib.rs
git commit -F - <<'EOF'
feat(edit-model): a radial gradient shape

Centre plus two radii, axis-aligned, with a smoothstep feather. Not yet
reachable from `Mask` — that variant lands once its consumers are ready.

⚠ `coverage` takes `aspect` and ignores it, which is the interesting part.
Anisotropic scaling between normalised and screen space destroys
perpendicularity (which is why the linear gradient needs the correction) but
not axis-alignment, so an axis-aligned ellipse needs no correction at all.
`radial_coverage_is_independent_of_aspect` guards against someone adding the
"missing" one later.

`Default` is hand-written rather than derived, because container-level
`#[serde(default)]` fills missing fields from the struct's `Default` and a
derived one would give every partial sidecar a zero-radius point-ellipse.
EOF
```

---

## Task 2: Retarget `mask_tool` at `Mask` rather than `LinearGradient`

A pure refactor performed **before** the variant exists, so the diff is small and the behaviour is provably unchanged. `Mask` still has one variant here.

**Files:**
- Modify: `crates/app/src/mask_tool.rs`
- Modify: `crates/app/src/develop/canvas.rs` (the two call sites)

- [ ] **Step 1: Change the signatures and add the dispatch**

In `crates/app/src/mask_tool.rs`, change the import line:

```rust
use edit_model::{LinearGradient, Mask};
```

Replace the `pub fn hit_test` signature and body's opening with a dispatcher plus the existing body moved into a private function. The full replacement for the `hit_test` item (keep its entire existing doc comment, adding the two new paragraphs shown):

```rust
/// Which handle (if any) is under normalized point `p`, within `tol`.
///
/// `aspect` is the frame's width divided by its height.
///
/// ⚠ **`tol` is in units of frame height, not normalized units** — unlike
/// [`crate::crop_tool::hit_test`], whose `tol` is normalized. The two are siblings with the
/// same shape and different units, so a tolerance copied from one to the other is wrong by a
/// factor of the aspect ratio on the horizontal axis. It has to be this way here: the whole
/// point of the aspect correction below is that distances are isotropic, and a tolerance
/// compared against them must share their metric.
///
/// ⚠ **Distances are measured in screen space, not normalized space** — the same requirement
/// `LinearGradient::coverage` has, for the same reason. Without it the grab region is
/// anisotropic: on a 3:1 frame you could grab the line from three times further away
/// horizontally than vertically.
///
/// ⚠ Note this is the *opposite* of `RadialGradient::coverage`, which takes an aspect and
/// ignores it. Coverage of an axis-aligned ellipse is immune to the anisotropy; grabbing a
/// handle is a distance test and is not. Both facts are load-bearing and each is guarded by
/// its own test.
///
/// Takes a `Mask` rather than a shape so the canvas's drag plumbing stays shape-agnostic and
/// is written once.
pub fn hit_test(m: &Mask, p: [f32; 2], aspect: f32, tol: f32) -> Option<Handle> {
    match m {
        Mask::Linear(g) => hit_test_linear(g, p, aspect, tol),
    }
}

/// Endpoints are tested before the line, so aiming at an end moves that end rather than the
/// whole gradient.
fn hit_test_linear(g: &LinearGradient, p: [f32; 2], aspect: f32, tol: f32) -> Option<Handle> {
    // Everything below is in units of frame height.
    let pt = (p[0] * aspect, p[1]);
    let a = (g.from[0] * aspect, g.from[1]);
    let b = (g.to[0] * aspect, g.to[1]);
    let dist = |q: (f32, f32)| ((pt.0 - q.0).powi(2) + (pt.1 - q.1).powi(2)).sqrt();

    if dist(a) <= tol {
        return Some(Handle::From);
    }
    if dist(b) <= tol {
        return Some(Handle::To);
    }

    // Distance to the segment, not the infinite line: past either end it is the endpoint
    // distance, which the two checks above already rejected.
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    if len2 <= f32::EPSILON {
        return None;
    }
    let t = (((pt.0 - a.0) * dx + (pt.1 - a.1) * dy) / len2).clamp(0.0, 1.0);
    let closest = (a.0 + t * dx, a.1 + t * dy);
    if dist(closest) <= tol {
        return Some(Handle::Line);
    }
    None
}
```

Replace the whole `pub fn apply_drag` item with:

```rust
/// Apply a normalized drag `(dx, dy)` to `handle`, clamped to the frame.
///
/// ⚠ Clamping matters: an endpoint dragged off the canvas would be unreachable afterwards.
pub fn apply_drag(m: Mask, handle: Handle, dx: f32, dy: f32) -> Mask {
    let mv = |p: [f32; 2]| [(p[0] + dx).clamp(0.0, 1.0), (p[1] + dy).clamp(0.0, 1.0)];
    match (m, handle) {
        (Mask::Linear(g), Handle::From) => Mask::Linear(LinearGradient {
            from: mv(g.from),
            ..g
        }),
        (Mask::Linear(g), Handle::To) => Mask::Linear(LinearGradient { to: mv(g.to), ..g }),
        (Mask::Linear(g), Handle::Line) => Mask::Linear(LinearGradient {
            from: mv(g.from),
            to: mv(g.to),
            ..g
        }),
    }
}
```

- [ ] **Step 2: Update the existing tests to go through `Mask`**

In `crates/app/src/mask_tool.rs`'s test module, change `horizontal()` to return a `Mask` and adjust the six tests that use it. Replace the helper and the six tests as follows (leave `deleting_a_mask_keeps_the_selection_valid` and `deleting_the_last_mask_clears_the_selection` untouched):

```rust
    /// A gradient running left to right across the middle of a square frame.
    fn horizontal_gradient() -> LinearGradient {
        LinearGradient {
            from: [0.2, 0.5],
            to: [0.8, 0.5],
            invert: false,
        }
    }

    fn horizontal() -> Mask {
        Mask::Linear(horizontal_gradient())
    }

    /// The gradient inside a mask, for tests that assert on endpoints.
    fn gradient_of(m: Mask) -> LinearGradient {
        match m {
            Mask::Linear(g) => g,
        }
    }

    #[test]
    fn an_endpoint_is_grabbed_in_preference_to_the_line() {
        // The pointer is on the axis *and* on an endpoint. Moving the whole gradient when the
        // user aimed at an end would be the more annoying of the two mistakes.
        let g = horizontal();
        assert_eq!(hit_test(&g, [0.2, 0.5], 1.0, 0.04), Some(Handle::From));
        assert_eq!(hit_test(&g, [0.8, 0.5], 1.0, 0.04), Some(Handle::To));
    }

    #[test]
    fn the_line_is_grabbed_between_the_endpoints() {
        let g = horizontal();
        assert_eq!(hit_test(&g, [0.5, 0.5], 1.0, 0.04), Some(Handle::Line));
    }

    #[test]
    fn nothing_is_grabbed_far_from_the_gradient() {
        let g = horizontal();
        assert_eq!(hit_test(&g, [0.5, 0.9], 1.0, 0.04), None);
        assert_eq!(hit_test(&g, [0.5, 0.1], 1.0, 0.04), None);
    }

    #[test]
    fn hit_testing_the_line_is_aspect_correct() {
        // ⚠ The same non-square trap `LinearGradient::coverage` needed, in its third guise:
        // distance to the axis must be measured in screen space, or the grab region is
        // anisotropic — on a 3:1 frame you could grab the line from three times further away
        // horizontally than vertically.
        //
        // ⚠⚠ The axis here is **vertical** and the probe points are offset **horizontally**,
        // and that is the whole point. An earlier version used a horizontal axis with the
        // point directly below it — where the perpendicular distance is purely vertical, so
        // scaling x cannot change the answer. It passed with the correction removed, i.e. it
        // could not detect the very bug it names. A test is only as good as the property it
        // actually exercises.
        let g = Mask::Linear(LinearGradient {
            from: [0.5, 0.2],
            to: [0.5, 0.8],
            invert: false,
        });
        let aspect = 3.0;
        // 0.03 normalised to the side of a vertical axis is 0.09 frame *heights* away on a
        // 3:1 frame — outside a 0.04 tolerance. Without the correction it measures 0.03 and
        // is wrongly grabbed.
        assert_eq!(hit_test(&g, [0.53, 0.5], aspect, 0.04), None);
        // 0.01 normalised = 0.03 heights: inside the tolerance either way.
        assert_eq!(hit_test(&g, [0.51, 0.5], aspect, 0.04), Some(Handle::Line));
    }

    #[test]
    fn dragging_an_endpoint_leaves_the_other_alone() {
        let g = horizontal();
        let out = gradient_of(apply_drag(g, Handle::From, 0.1, -0.2));
        assert_eq!(out.from, [0.3, 0.3]);
        assert_eq!(out.to, horizontal_gradient().to, "the far end must not move");
    }

    #[test]
    fn dragging_the_line_moves_both_ends_equally() {
        // The gradient translates: its direction and length are unchanged, which is what
        // makes "reposition without reshaping" possible.
        //
        // ⚠ Tolerance, not `assert_eq!`: 0.8f32 + 0.1f32 is 0.9000000358, one ULP off the
        // 0.9f32 literal (0.8999999762) — the same class of trap the orientation work hit
        // ("1.0 - (1.0 - 0.1) is 0.100000024"). `crop_tool`'s own arithmetic-result tests use
        // the same 1e-6 tolerance for the same reason.
        let g = horizontal();
        let out = gradient_of(apply_drag(g, Handle::Line, 0.1, 0.05));
        let close =
            |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6;
        assert!(close(out.from, [0.3, 0.55]), "{:?}", out.from);
        assert!(close(out.to, [0.9, 0.55]), "{:?}", out.to);
    }

    #[test]
    fn dragging_clamps_to_the_frame() {
        // An endpoint dragged off the canvas would be unreachable afterwards.
        let g = horizontal();
        let out = gradient_of(apply_drag(g, Handle::To, 5.0, 5.0));
        assert_eq!(out.to, [1.0, 1.0]);
    }

    #[test]
    fn invert_survives_a_drag() {
        // It is not a positional property, so no drag should disturb it.
        let g = Mask::Linear(LinearGradient {
            invert: true,
            ..horizontal_gradient()
        });
        assert!(gradient_of(apply_drag(g, Handle::Line, 0.1, 0.1)).invert);
        assert!(gradient_of(apply_drag(g, Handle::From, 0.1, 0.1)).invert);
    }

    #[test]
    fn a_zero_length_gradient_grabs_nothing_away_from_its_point() {
        // ⚠ Both endpoints equal: there is no axis to project onto, and `len2` is exactly
        // zero. Correct by inspection — the endpoint checks catch anything near the shared
        // point and the guard returns `None` rather than dividing — but "correct by
        // inspection" is what a test is for. A NaN escaping here would reach the canvas as a
        // handle that can never be grabbed again.
        let g = Mask::Linear(LinearGradient {
            from: [0.5, 0.5],
            to: [0.5, 0.5],
            invert: false,
        });
        assert_eq!(hit_test(&g, [0.1, 0.1], 1.0, 0.04), None);
        assert_eq!(hit_test(&g, [0.5, 0.5], 1.0, 0.04), Some(Handle::From));
    }
```

- [ ] **Step 3: Update the two canvas call sites**

In `crates/app/src/develop/canvas.rs`, inside `draw_mask_overlay`, replace this block:

```rust
        if resp.drag_started() {
            s.active_mask_handle = mask_tool::hit_test(&g, [nx, ny], aspect, 0.04);
        }
        if resp.dragged() {
            if let Some(h) = s.active_mask_handle {
                let d = resp.drag_delta();
                let dx = d.x / frame_rect.width();
                let dy = d.y / frame_rect.height();
                if let Some(m) = s.edits.masks.get_mut(i) {
                    let edit_model::Mask::Linear(cur) = m.mask;
                    m.mask = edit_model::Mask::Linear(mask_tool::apply_drag(cur, h, dx, dy));
                    moved = true;
                }
            }
        }
```

with:

```rust
        if resp.drag_started() {
            s.active_mask_handle = mask_tool::hit_test(&mask, [nx, ny], aspect, 0.04);
        }
        if resp.dragged() {
            if let Some(h) = s.active_mask_handle {
                let d = resp.drag_delta();
                let dx = d.x / frame_rect.width();
                let dy = d.y / frame_rect.height();
                if let Some(m) = s.edits.masks.get_mut(i) {
                    m.mask = mask_tool::apply_drag(m.mask, h, dx, dy);
                    moved = true;
                }
            }
        }
```

And change the binding near the top of the same function from:

```rust
    let edit_model::Mask::Linear(g) = m.mask;
```

to:

```rust
    let mask = m.mask;
    let edit_model::Mask::Linear(g) = mask;
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p app 2>&1 | tail -10`
Expected: PASS — every `mask_tool` test still passes with its assertions unchanged in meaning. That is the regression argument for this refactor.

- [ ] **Step 5: Check the workspace builds and lints**

Run: `cargo clippy --all-targets --all-features 2>&1 | tail -10`
Expected: no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/app/src/mask_tool.rs crates/app/src/develop/canvas.rs
git commit -F - <<'EOF'
refactor(app): mask_tool takes a Mask, not a LinearGradient

Preparation for the radial variant. `Session.active_mask_handle` is a single
field that must hold either shape's handle, so `hit_test`/`apply_drag` are
better expressed over the sum type — which also keeps the canvas's drag
plumbing shape-agnostic and written once.

Behaviour is unchanged: every existing test passes with its assertions
unchanged in meaning, only wrapped in `Mask::Linear`.
EOF
```

---

## Task 3: Split the canvas overlay into drag plumbing and a painter

Also a pure refactor with one variant, so the seam exists before the variant needs it.

**Files:**
- Modify: `crates/app/src/develop/canvas.rs`

- [ ] **Step 1: Extract `paint_mask`**

In `crates/app/src/develop/canvas.rs`, inside `draw_mask_overlay`, delete this block (from the `to_screen` closure down to and including the endpoint-dot loop):

```rust
    let to_screen = |p: [f32; 2]| {
        egui::pos2(
            frame_rect.left() + p[0] * frame_rect.width(),
            frame_rect.top() + p[1] * frame_rect.height(),
        )
    };
    let (a, b) = (to_screen(g.from), to_screen(g.to));

    let painter = ui.painter_at(img_rect);
    let stroke = egui::Stroke::new(1.5, egui::Color32::WHITE);
    painter.line_segment([a, b], stroke);

    // ⚠ The perpendicular is computed **in screen space**, from the on-screen axis — the same
    // trap as `coverage` and `hit_test`, in its third guise. A normalised-space perpendicular
    // is visibly not perpendicular on a 3:2 frame.
    let axis = b - a;
    let len = axis.length();
    if len > f32::EPSILON {
        let perp = egui::vec2(-axis.y, axis.x) / len;
        // Long enough to cross the clip region from any point inside it — measured on
        // `img_rect`, since that is what bounds what will actually be drawn.
        let reach = img_rect.width() + img_rect.height();
        for end in [a, b] {
            painter.line_segment(
                [end - perp * reach, end + perp * reach],
                egui::Stroke::new(1.0, egui::Color32::from_white_alpha(120)),
            );
        }
    }
    for end in [a, b] {
        painter.circle_filled(end, 5.0, egui::Color32::WHITE);
    }
```

and replace it with a single call:

```rust
    paint_mask(&ui.painter_at(img_rect), mask, frame_rect, img_rect);
```

Then delete the now-unused line `let edit_model::Mask::Linear(g) = mask;` that Task 2 added.

Add this new function immediately after `draw_mask_overlay`:

```rust
/// Paint one mask's handles and guides.
///
/// ⚠ **Split from `draw_mask_overlay` deliberately, on painting rather than on the whole
/// function.** The drag plumbing — hit-testing, applying the drag, pushing history and calling
/// `s.request` — is shared by every shape and must be written once. Duplicating it per shape is
/// exactly how the two would drift, and it is where both of 3.1a's real-use bugs lived.
///
/// ⚠ `frame_rect` places everything; `clip` only bounds what is drawn. They differ once a
/// region render is on, and using `clip` for placement is the bug that made the overlay drift
/// with the view while panning at high zoom.
fn paint_mask(
    painter: &egui::Painter,
    mask: edit_model::Mask,
    frame_rect: egui::Rect,
    clip: egui::Rect,
) {
    let to_screen = |p: [f32; 2]| {
        egui::pos2(
            frame_rect.left() + p[0] * frame_rect.width(),
            frame_rect.top() + p[1] * frame_rect.height(),
        )
    };
    let stroke = egui::Stroke::new(1.5, egui::Color32::WHITE);

    match mask {
        edit_model::Mask::Linear(g) => {
            let (a, b) = (to_screen(g.from), to_screen(g.to));
            painter.line_segment([a, b], stroke);

            // ⚠ The perpendicular is computed **in screen space**, from the on-screen axis —
            // the same trap as `coverage` and `hit_test`, in its third guise. A
            // normalised-space perpendicular is visibly not perpendicular on a 3:2 frame.
            let axis = b - a;
            let len = axis.length();
            if len > f32::EPSILON {
                let perp = egui::vec2(-axis.y, axis.x) / len;
                // Long enough to cross the clip region from any point inside it.
                let reach = clip.width() + clip.height();
                for end in [a, b] {
                    painter.line_segment(
                        [end - perp * reach, end + perp * reach],
                        egui::Stroke::new(1.0, egui::Color32::from_white_alpha(120)),
                    );
                }
            }
            for end in [a, b] {
                painter.circle_filled(end, 5.0, egui::Color32::WHITE);
            }
        }
    }
}
```

- [ ] **Step 2: Build and lint**

Run: `cargo clippy --all-targets --all-features 2>&1 | tail -10`
Expected: no warnings, no errors.

- [ ] **Step 3: Run the tests**

Run: `cargo test --all 2>&1 | tail -5`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/app/src/develop/canvas.rs
git commit -F - <<'EOF'
refactor(app): split mask painting out of the overlay's drag plumbing

`paint_mask` matches on the shape; `draw_mask_overlay` keeps the hit-testing,
drag application, history push and re-render request that every shape shares.

Splitting on painting rather than on the whole function is what makes the
radial variant inherit both of 3.1a's real-use bug fixes for free: the
`s.request` call after a drag, and placement against `frame_rect` rather than
`img_rect`.
EOF
```

---

## Task 4: Add the `Mask::Radial` variant and fill every arm

The three predicted compile errors arrive here, with their seams already prepared.

**Files:**
- Modify: `crates/edit-model/src/lib.rs`
- Modify: `crates/app/src/mask_tool.rs`
- Modify: `crates/app/src/develop/canvas.rs`

- [ ] **Step 1: Write the failing `mask_tool` tests**

Add to `crates/app/src/mask_tool.rs`'s test module:

```rust
    /// An ellipse in the middle of a square frame.
    fn ellipse() -> RadialGradient {
        RadialGradient {
            center: [0.5, 0.5],
            radius: [0.25, 0.2],
            feather: 50.0,
            invert: false,
        }
    }

    fn radial_of(m: Mask) -> RadialGradient {
        match m {
            Mask::Radial(r) => r,
            other => panic!("expected a radial, got {other:?}"),
        }
    }

    #[test]
    fn the_centre_is_grabbed_in_preference_to_an_edge() {
        // Mirrors "endpoints before the line": aiming at the middle should move the whole
        // shape, not resize it. Here the two are far apart, so this pins the ordering only
        // insofar as the centre is found at all — the discriminating case is below.
        let m = Mask::Radial(ellipse());
        assert_eq!(hit_test(&m, [0.5, 0.5], 1.0, 0.04), Some(Handle::Center));
        assert_eq!(
            hit_test(&m, [0.75, 0.5], 1.0, 0.04),
            Some(Handle::Edge(Side::Right))
        );
        assert_eq!(
            hit_test(&m, [0.25, 0.5], 1.0, 0.04),
            Some(Handle::Edge(Side::Left))
        );
        assert_eq!(
            hit_test(&m, [0.5, 0.3], 1.0, 0.04),
            Some(Handle::Edge(Side::Top))
        );
        assert_eq!(
            hit_test(&m, [0.5, 0.7], 1.0, 0.04),
            Some(Handle::Edge(Side::Bottom))
        );
        assert_eq!(hit_test(&m, [0.9, 0.9], 1.0, 0.04), None);
    }

    #[test]
    fn a_tiny_ellipse_gives_the_centre_to_the_pointer_over_its_edges() {
        // ⚠ The discriminating case for the ordering above: with a radius inside the
        // tolerance, the centre and all four edge handles are simultaneously in range.
        // Reordering the checks changes the answer here and nowhere else.
        let m = Mask::Radial(RadialGradient {
            radius: [0.01, 0.01],
            ..ellipse()
        });
        assert_eq!(hit_test(&m, [0.5, 0.5], 1.0, 0.04), Some(Handle::Center));
    }

    #[test]
    fn hit_testing_an_edge_handle_is_aspect_correct() {
        // ⚠⚠ Designed against the 3.1a lesson. The probe is offset along the axis where the
        // scaling actually changes the answer: the **top** handle, probed **horizontally**, on
        // a 3:1 frame. A vertical offset from the same handle would be untouched by the
        // scaling and would pass with the correction deleted — the exact mistake
        // `hit_testing_the_line_is_aspect_correct` originally made.
        let m = Mask::Radial(RadialGradient {
            center: [0.5, 0.5],
            radius: [0.1, 0.2],
            ..ellipse()
        });
        let aspect = 3.0;
        // The top handle sits at normalised (0.5, 0.3). A 0.03 normalised offset in x is 0.09
        // frame *heights* on a 3:1 frame — outside a 0.04 tolerance. Without the correction it
        // measures 0.03 and is wrongly grabbed.
        assert_eq!(hit_test(&m, [0.53, 0.3], aspect, 0.04), None);
        // 0.01 normalised = 0.03 heights: inside the tolerance either way.
        assert_eq!(
            hit_test(&m, [0.51, 0.3], aspect, 0.04),
            Some(Handle::Edge(Side::Top))
        );
    }

    #[test]
    fn dragging_an_edge_resizes_about_a_fixed_centre() {
        // Lightroom's behaviour, and what a marked centre implies. Dragging the right edge
        // outward widens the ellipse; dragging the left edge outward (negative dx) does the
        // same, because each side grows away from the centre.
        let m = Mask::Radial(ellipse());
        let close = |a: f32, b: f32| (a - b).abs() < 1e-6;

        let right = radial_of(apply_drag(m, Handle::Edge(Side::Right), 0.1, 0.0));
        assert!(close(right.radius[0], 0.35), "{:?}", right.radius);
        assert!(close(right.radius[1], 0.2), "the other radius must not move");
        assert_eq!(right.center, [0.5, 0.5], "the centre must not move");

        let left = radial_of(apply_drag(m, Handle::Edge(Side::Left), -0.1, 0.0));
        assert!(close(left.radius[0], 0.35), "{:?}", left.radius);
        assert_eq!(left.center, [0.5, 0.5]);

        let bottom = radial_of(apply_drag(m, Handle::Edge(Side::Bottom), 0.0, 0.05));
        assert!(close(bottom.radius[1], 0.25), "{:?}", bottom.radius);
        assert!(close(bottom.radius[0], 0.25), "the other radius must not move");

        let top = radial_of(apply_drag(m, Handle::Edge(Side::Top), 0.0, -0.05));
        assert!(close(top.radius[1], 0.25), "{:?}", top.radius);
    }

    #[test]
    fn dragging_the_centre_leaves_the_radii_alone() {
        let m = Mask::Radial(ellipse());
        let out = radial_of(apply_drag(m, Handle::Center, 0.1, -0.2));
        let close = |a: [f32; 2], b: [f32; 2]| {
            (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6
        };
        assert!(close(out.center, [0.6, 0.3]), "{:?}", out.center);
        assert!(close(out.radius, ellipse().radius), "{:?}", out.radius);
    }

    #[test]
    fn the_radial_centre_clamps_to_the_frame() {
        // A centre dragged off the canvas would be unreachable afterwards — the same rule the
        // linear gradient's endpoints follow.
        let m = Mask::Radial(ellipse());
        assert_eq!(
            radial_of(apply_drag(m, Handle::Center, 5.0, 5.0)).center,
            [1.0, 1.0]
        );
        assert_eq!(
            radial_of(apply_drag(m, Handle::Center, -5.0, -5.0)).center,
            [0.0, 0.0]
        );
    }

    #[test]
    fn radii_clamp_to_a_usable_range() {
        // ⚠ The minimum is what keeps the ellipse grabbable: a zero-radius shape has all five
        // handles stacked on one point and is inert. The maximum stops a fast drag producing
        // an absurd value while still allowing an ellipse twice the frame, a legitimate look.
        let m = Mask::Radial(ellipse());
        let shrunk = radial_of(apply_drag(m, Handle::Edge(Side::Right), -5.0, 0.0));
        assert_eq!(shrunk.radius[0], RADIUS_MIN);
        let grown = radial_of(apply_drag(m, Handle::Edge(Side::Bottom), 0.0, 50.0));
        assert_eq!(grown.radius[1], RADIUS_MAX);
    }

    #[test]
    fn radial_invert_survives_a_drag() {
        let m = Mask::Radial(RadialGradient {
            invert: true,
            ..ellipse()
        });
        assert!(radial_of(apply_drag(m, Handle::Center, 0.1, 0.1)).invert);
        assert!(radial_of(apply_drag(m, Handle::Edge(Side::Right), 0.1, 0.0)).invert);
    }

    #[test]
    fn radial_feather_survives_a_drag() {
        // Like `invert`, not a positional property.
        let m = Mask::Radial(RadialGradient {
            feather: 17.0,
            ..ellipse()
        });
        assert_eq!(radial_of(apply_drag(m, Handle::Center, 0.1, 0.1)).feather, 17.0);
    }

    #[test]
    fn a_mismatched_handle_leaves_the_mask_unchanged() {
        // ⚠ Unreachable in the app — there is no UI to change a mask's kind, let alone
        // mid-drag — but the flat `Handle` enum admits the pair, so the degradation is worth
        // pinning. Doing nothing is the right failure for a UI; panicking is not.
        let radial = Mask::Radial(ellipse());
        assert_eq!(apply_drag(radial, Handle::Line, 0.1, 0.1), radial);
        let linear = horizontal();
        assert_eq!(apply_drag(linear, Handle::Center, 0.1, 0.1), linear);
    }
```

⚠ **Do not add an import to the test module.** `use super::*` already picks up the parent
module's own `use` declarations, which is how the existing tests reach `LinearGradient`. Step 4
adds `RadialGradient` to that parent import and the tests see it automatically; adding a second
import here would be a duplicate and would fail `clippy -D warnings`.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p app radial 2>&1 | tail -20`
Expected: FAIL to compile — `no variant named Radial found for enum Mask`, `cannot find type Side`.

- [ ] **Step 3: Add the variant and its accessors**

In `crates/edit-model/src/lib.rs`, replace the `Mask` enum and its `impl` block with:

```rust
/// The mask primitives. 3.1c adds `Brush`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Mask {
    Linear(LinearGradient),
    Radial(RadialGradient),
}

impl Default for Mask {
    fn default() -> Self {
        Mask::Linear(LinearGradient::default())
    }
}

impl Mask {
    /// Coverage at `(u, v)` in normalised frame coordinates, in `0.0..=1.0`.
    ///
    /// `aspect` is the frame's width divided by its height. ⚠ Whether a variant *needs* it
    /// differs: see [`LinearGradient::coverage`] for why a projection does, and
    /// [`RadialGradient::coverage`] for why an axis-aligned ellipse does not.
    pub fn coverage(&self, u: f32, v: f32, aspect: f32) -> f32 {
        match self {
            Mask::Linear(g) => g.coverage(u, v, aspect),
            Mask::Radial(r) => r.coverage(u, v, aspect),
        }
    }

    /// The shape's name, for the mask list in the UI.
    pub fn label(&self) -> &'static str {
        match self {
            Mask::Linear(_) => "Gradient",
            Mask::Radial(_) => "Radial",
        }
    }

    /// The `invert` flag, wherever it lives on this shape.
    ///
    /// Every shape carries one and none of them means anything shape-specific by it, so the
    /// panel offers a single checkbox rather than a match. Returning `&mut bool` rather than a
    /// getter/setter pair is what lets `ui.checkbox` drive it directly.
    pub fn invert_mut(&mut self) -> &mut bool {
        match self {
            Mask::Linear(g) => &mut g.invert,
            Mask::Radial(r) => &mut r.invert,
        }
    }
}
```

Then add these tests to `crates/edit-model/src/lib.rs`'s test module, pinning that the variant
is additive at the *format* level:

```rust
    #[test]
    fn an_existing_linear_sidecar_still_loads() {
        // ⚠ `Mask` is externally tagged, so every sidecar written before M3.1b holds
        // `{"Linear": {...}}`. Adding a variant to an externally tagged enum is backward
        // compatible — but "is" is doing a lot of work in that sentence, and a switch to an
        // internally tagged or untagged representation would silently break every file on
        // disk while every other test kept passing.
        let json = r#"{"masks":[{"mask":{"Linear":{"from":[0.0,0.0],"to":[1.0,0.0],
                      "invert":false}},"adjustments":{"exposure":1.0}}]}"#;
        let e: Edits = serde_json::from_str(json).unwrap();
        assert_eq!(e.masks.len(), 1);
        assert!(matches!(e.masks[0].mask, Mask::Linear(_)));
        assert_eq!(e.masks[0].adjustments.exposure, 1.0);
    }

    #[test]
    fn a_radial_round_trips_through_json() {
        let src = Mask::Radial(RadialGradient {
            center: [0.25, 0.75],
            radius: [0.4, 0.1],
            feather: 12.5,
            invert: true,
        });
        let back: Mask = serde_json::from_str(&serde_json::to_string(&src).unwrap()).unwrap();
        assert_eq!(back, src);
    }

    #[test]
    fn every_mask_variant_has_a_label_and_an_invert() {
        // A table over both variants, so a third one cannot ship with an empty list row or an
        // `invert` the panel's checkbox cannot reach. 3.1c's brush will land here.
        for mut m in [
            Mask::Linear(LinearGradient::default()),
            Mask::Radial(RadialGradient::default()),
        ] {
            assert!(!m.label().is_empty());
            *m.invert_mut() = true;
            assert!(*m.invert_mut(), "invert_mut must expose the real field");
        }
    }
```

- [ ] **Step 4: Fill the `mask_tool` arms**

In `crates/app/src/mask_tool.rs`, change the import:

```rust
use edit_model::{LinearGradient, Mask, RadialGradient};
```

Add the `Side` enum and the clamp constants above `pub enum Handle`:

```rust
/// Which edge of a radial mask's ellipse a handle sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
    Top,
    Bottom,
}

/// The smallest a radius may be dragged. Below this the five handles stack on one point and
/// the shape becomes ungrabbable — the same class of trap the linear gradient's frame clamp
/// avoids.
pub const RADIUS_MIN: f32 = 0.01;
/// The largest. An ellipse twice the frame is a legitimate look; a fast drag producing 40 is
/// not.
pub const RADIUS_MAX: f32 = 2.0;
```

Extend `Handle`:

```rust
/// What a drag is moving.
///
/// ⚠ **One flat enum across every shape**, rather than `Handle { Linear(..), Radial(..) }`.
/// `Session.active_mask_handle` is a single field and must hold either, and nesting would not
/// make a mismatched (mask, handle) pair unrepresentable — it would only add a level. So the
/// simplest spelling wins, and `apply_drag`'s catch-all arm handles the pair that cannot occur.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    // Linear.
    From,
    To,
    /// The whole gradient — both endpoints move together.
    Line,
    // Radial.
    Center,
    /// One edge of the ellipse. Dragging it resizes about a fixed centre.
    Edge(Side),
}
```

Add the radial arm to `hit_test`:

```rust
pub fn hit_test(m: &Mask, p: [f32; 2], aspect: f32, tol: f32) -> Option<Handle> {
    match m {
        Mask::Linear(g) => hit_test_linear(g, p, aspect, tol),
        Mask::Radial(r) => hit_test_radial(r, p, aspect, tol),
    }
}
```

and the function itself, after `hit_test_linear`:

```rust
/// The centre is tested before the edges, so aiming at the middle of a small ellipse moves the
/// whole shape rather than resizing it — the same preference `hit_test_linear` gives endpoints
/// over the line.
fn hit_test_radial(r: &RadialGradient, p: [f32; 2], aspect: f32, tol: f32) -> Option<Handle> {
    // ⚠ In units of frame height, like the linear case and for the same reason: a tolerance
    // must share the metric of the distances it is compared against. Note this is the opposite
    // of `RadialGradient::coverage`, which needs no correction — coverage of an axis-aligned
    // ellipse survives the anisotropy, but a distance does not.
    let pt = (p[0] * aspect, p[1]);
    let c = (r.center[0] * aspect, r.center[1]);
    let dist = |q: (f32, f32)| ((pt.0 - q.0).powi(2) + (pt.1 - q.1).powi(2)).sqrt();

    if dist(c) <= tol {
        return Some(Handle::Center);
    }
    let (rx, ry) = (r.radius[0] * aspect, r.radius[1]);
    for (side, q) in [
        (Side::Left, (c.0 - rx, c.1)),
        (Side::Right, (c.0 + rx, c.1)),
        (Side::Top, (c.0, c.1 - ry)),
        (Side::Bottom, (c.0, c.1 + ry)),
    ] {
        if dist(q) <= tol {
            return Some(Handle::Edge(side));
        }
    }
    None
}
```

Extend `apply_drag`:

```rust
pub fn apply_drag(m: Mask, handle: Handle, dx: f32, dy: f32) -> Mask {
    let mv = |p: [f32; 2]| [(p[0] + dx).clamp(0.0, 1.0), (p[1] + dy).clamp(0.0, 1.0)];
    match (m, handle) {
        (Mask::Linear(g), Handle::From) => Mask::Linear(LinearGradient {
            from: mv(g.from),
            ..g
        }),
        (Mask::Linear(g), Handle::To) => Mask::Linear(LinearGradient { to: mv(g.to), ..g }),
        (Mask::Linear(g), Handle::Line) => Mask::Linear(LinearGradient {
            from: mv(g.from),
            to: mv(g.to),
            ..g
        }),
        (Mask::Radial(r), Handle::Center) => Mask::Radial(RadialGradient {
            center: mv(r.center),
            ..r
        }),
        (Mask::Radial(r), Handle::Edge(side)) => Mask::Radial(resize_radial(r, side, dx, dy)),
        // ⚠ A handle belonging to the other shape. Unreachable — there is no UI to change a
        // mask's kind, let alone mid-drag — and doing nothing is the right failure for a UI.
        _ => m,
    }
}

/// Resize about a **fixed centre**: each edge grows away from the middle, which is what a
/// marked centre implies and what Lightroom does.
fn resize_radial(r: RadialGradient, side: Side, dx: f32, dy: f32) -> RadialGradient {
    let (mut rx, mut ry) = (r.radius[0], r.radius[1]);
    match side {
        Side::Right => rx += dx,
        Side::Left => rx -= dx,
        Side::Bottom => ry += dy,
        Side::Top => ry -= dy,
    }
    RadialGradient {
        radius: [
            rx.clamp(RADIUS_MIN, RADIUS_MAX),
            ry.clamp(RADIUS_MIN, RADIUS_MAX),
        ],
        ..r
    }
}
```

- [ ] **Step 5: Fill the canvas painting arm**

In `crates/app/src/develop/canvas.rs`, add above `fn paint_mask`:

```rust
/// Points used to approximate a mask ellipse. egui has no ellipse primitive, so it is drawn as
/// a closed polyline; 64 is smooth at any zoom this canvas reaches.
const ELLIPSE_SEGMENTS: usize = 64;
```

and add the second arm inside `paint_mask`'s match:

```rust
        edit_model::Mask::Radial(r) => {
            let points: Vec<egui::Pos2> = (0..ELLIPSE_SEGMENTS)
                .map(|i| {
                    let th = i as f32 / ELLIPSE_SEGMENTS as f32 * std::f32::consts::TAU;
                    to_screen([
                        r.center[0] + r.radius[0] * th.cos(),
                        r.center[1] + r.radius[1] * th.sin(),
                    ])
                })
                .collect();
            painter.add(egui::Shape::closed_line(points, stroke));
            // Centre plus the four edge handles, in the order `mask_tool::hit_test_radial`
            // tests them.
            let handles = [
                r.center,
                [r.center[0] - r.radius[0], r.center[1]],
                [r.center[0] + r.radius[0], r.center[1]],
                [r.center[0], r.center[1] - r.radius[1]],
                [r.center[0], r.center[1] + r.radius[1]],
            ];
            for h in handles {
                painter.circle_filled(to_screen(h), 5.0, egui::Color32::WHITE);
            }
        }
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test --all 2>&1 | tail -10`
Expected: PASS. The golden tests still pass because no mask is added to any existing case.

- [ ] **Step 7: Sabotage — the aspect and ordering tests**

Delete the `* aspect` from `pt` and `c` in `hit_test_radial` (four occurrences of the multiplication across the two tuples and `rx`).
Run: `cargo test -p app hit_testing_an_edge_handle_is_aspect_correct 2>&1 | tail -20`
Expected: **FAIL** — the first assertion gets `Some(Edge(Top))` where `None` is required.

Restore. Then move the centre check below the edge loop in `hit_test_radial`.
Run: `cargo test -p app a_tiny_ellipse 2>&1 | tail -20`
Expected: **FAIL** — `Some(Edge(Left))` where `Some(Center)` is required.

Restore. Then break the axis independence in `resize_radial` by changing its `Side::Right` arm to:

```rust
        Side::Right => {
            rx += dx;
            ry += dy;
        }
```

Run: `cargo test -p app dragging_an_edge_resizes_about_a_fixed_centre 2>&1 | tail -20`
Expected: **FAIL** on "the other radius must not move".

Restore. Then change `_ => m` in `apply_drag` to `_ => unreachable!()`.
Run: `cargo test -p app a_mismatched_handle 2>&1 | tail -20`
Expected: **FAIL** with a panic.

Restore. Then change `Mask`'s derive to add `#[serde(untagged)]` above the enum.
Run: `cargo test -p edit-model an_existing_linear_sidecar_still_loads 2>&1 | tail -20`
Expected: **FAIL** — the untagged representation cannot read `{"Linear": {...}}`.

Restore all five, then:
```bash
touch crates/app/src/mask_tool.rs crates/edit-model/src/lib.rs
```

- [ ] **Step 8: Verify the whole workspace**

Run: `cargo clippy --all-targets --all-features 2>&1 | tail -10`
Expected: no warnings.

Run: `cargo test --all 2>&1 | tail -5`
Expected: PASS.

- [ ] **Step 9: Commit**

```bash
git add crates/edit-model/src/lib.rs crates/app/src/mask_tool.rs crates/app/src/develop/canvas.rs
git commit -F - <<'EOF'
feat: Mask::Radial — an axis-aligned elliptical mask

Fills the four arms the variant demands: coverage, hit_test, apply_drag and
the canvas painter. Five handles — centre plus one per edge — with an edge
drag resizing about a fixed centre.

`Mask` also gains `label()` for the list row and `invert_mut()` so one
checkbox can serve both shapes.

The ellipse is drawn as a 64-point closed polyline; egui has no ellipse
primitive.
EOF
```

---

## Task 5: The panel — two buttons, the shape block, and the invert checkbox

**Files:**
- Modify: `crates/app/src/develop/panel.rs`

- [ ] **Step 1: Rename the local first**

⚠ Do this before the edits below, which all write to the new name — otherwise the file does not
compile between steps.

Rename `mask_list_changed` to `mask_changed` at all four of its occurrences (the declaration,
the delete branch, and the `if mask_list_changed {` fold after the closure), and update the
comment above the declaration:

```rust
                    // ⚠ `mask_changed` is a local, not a direct write to `changed`/`settled`
                    // from inside this closure: the closure also calls `slider`, which already
                    // holds its own mutable capture of those two, and a second direct capture
                    // of the same variables from this closure would be two simultaneous
                    // mutable borrows — a compile error. Folding the add/delete/invert events
                    // into `changed`/`settled` happens after the closure returns instead, the
                    // same restructuring `develop_body` already uses elsewhere in this
                    // codebase for the same reason.
                    let mut mask_changed = false;
```

- [ ] **Step 2: Replace the add button with two**

In `crates/app/src/develop/panel.rs`, replace:

```rust
                        if ui.button("Add graduated filter").clicked() {
                            s.edits.masks.push(edit_model::MaskedAdjustment {
                                mask: edit_model::Mask::Linear(edit_model::LinearGradient {
                                    // A visible default: top to bottom across the middle
                                    // third, so the handles land on screen and the ramp is
                                    // immediately obvious.
                                    from: [0.5, 0.33],
                                    to: [0.5, 0.67],
                                    invert: false,
                                }),
                                adjustments: edit_model::LocalAdjustments::default(),
                            });
                            s.selected_mask = Some(s.edits.masks.len() - 1);
                            mask_list_changed = true;
                        }
```

with:

```rust
                        let mut added: Option<edit_model::Mask> = None;
                        ui.horizontal(|ui| {
                            if ui.button("Add graduated filter").clicked() {
                                added = Some(edit_model::Mask::Linear(
                                    edit_model::LinearGradient {
                                        // A visible default: top to bottom across the middle
                                        // third, so the handles land on screen and the ramp is
                                        // immediately obvious.
                                        from: [0.5, 0.33],
                                        to: [0.5, 0.67],
                                        invert: false,
                                    },
                                ));
                            }
                            if ui.button("Add radial filter").clicked() {
                                // ⚠ The hand-written `Default`: centred, a quarter of each
                                // axis, feather 50. Both radii being fractions of their own
                                // axis makes the default ellipse *similar to the frame*, which
                                // is usually the shape being reached for.
                                added = Some(edit_model::Mask::Radial(
                                    edit_model::RadialGradient::default(),
                                ));
                            }
                        });
                        if let Some(mask) = added {
                            s.edits.masks.push(edit_model::MaskedAdjustment {
                                mask,
                                adjustments: edit_model::LocalAdjustments::default(),
                            });
                            s.selected_mask = Some(s.edits.masks.len() - 1);
                            mask_changed = true;
                        }
```

- [ ] **Step 3: Use the shape's label in the list**

In the list loop, replace the label:

```rust
                                let picked = s.selected_mask == Some(i);
                                let name = format!("{} {}", s.edits.masks[i].mask.label(), i + 1);
                                if ui.selectable_label(picked, name).clicked() {
                                    s.selected_mask = if picked { None } else { Some(i) };
                                }
```

- [ ] **Step 4: Add the shape-properties block**

Replace the selected-mask block:

```rust
                        if let Some(i) = s.selected_mask {
                            if let Some(m) = s.edits.masks.get_mut(i) {
                                ui.separator();
                                let a = &mut m.adjustments;
```

with:

```rust
                        if let Some(i) = s.selected_mask {
                            if let Some(m) = s.edits.masks.get_mut(i) {
                                ui.separator();
                                // Shape properties, kept apart from the adjustments below:
                                // these describe the mask, not what it carries.
                                //
                                // ⚠ `invert` had no UI at all until now. Survivable for a
                                // linear gradient, which can be inverted by dragging its
                                // endpoints past each other; not survivable for a radial,
                                // where the subject vignette — darken everything except the
                                // ellipse — depends on it and no drag substitutes.
                                if ui.checkbox(m.mask.invert_mut(), "Invert").changed() {
                                    mask_changed = true;
                                }
                                if let edit_model::Mask::Radial(r) = &mut m.mask {
                                    slider(ui, &mut r.feather, 0.0..=100.0, "Feather");
                                }
                                ui.separator();
                                let a = &mut m.adjustments;
```

- [ ] **Step 5: Update the hint**

Replace the hint text:

```rust
                        theme::hint(
                            ui,
                            "Select a mask to drag its handles on the image. A gradient's two \
                             outer lines mark where the effect starts and reaches full \
                             strength; a radial's ellipse marks where it falls to nothing.",
                        );
```

- [ ] **Step 6: Build and lint**

Run: `cargo clippy --all-targets --all-features 2>&1 | tail -10`
Expected: no warnings.

Run: `cargo fmt --all && cargo fmt --all --check`
Expected: no output.

- [ ] **Step 7: Run the tests**

Run: `cargo test --all 2>&1 | tail -5`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add crates/app/src/develop/panel.rs
git commit -F - <<'EOF'
feat(app): a Masks panel that can add a radial, and an Invert checkbox

Two add buttons, the list row labelled by shape, and a shape-properties block
between the list and the adjustments — Feather for a radial, Invert for both.

⚠ `invert` has existed on `LinearGradient` since 3.1a with no control anywhere;
it was reachable only by hand-editing a sidecar. That was survivable for a
gradient, whose endpoints can be dragged past each other, and is not for a
radial, where the subject vignette depends on it.
EOF
```

---

## Task 6: Goldens

**Files:**
- Modify: `crates/pipeline/tests/golden.rs`
- Create: `crates/pipeline/tests/golden/mask_radial_exposure.png`
- Create: `crates/pipeline/tests/golden/mask_radial_inverted_saturation.png`

- [ ] **Step 1: Add the two cases**

Append to `crates/pipeline/tests/golden.rs`, after `golden_mask_linear_inverted_saturation`:

```rust
#[test]
fn golden_mask_radial_exposure() {
    // An elliptical exposure lift in the middle of the 64x16 chart: full strength at the
    // centre, falling to nothing at the boundary. The radii are fractions of their own axis,
    // so on this 4:1 chart the ellipse is 45 px wide and 14 px tall — wide enough to sample
    // the ramp across the bands' 0..1 gradient.
    check_golden(
        "mask_radial_exposure",
        &Edits {
            masks: vec![edit_model::MaskedAdjustment {
                mask: edit_model::Mask::Radial(edit_model::RadialGradient {
                    center: [0.5, 0.5],
                    radius: [0.35, 0.45],
                    feather: 50.0,
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
fn golden_mask_radial_inverted_saturation() {
    // The subject vignette: everything *except* the ellipse is desaturated and darkened. This
    // is the case that `invert` exists for, and the one that had no UI until M3.1b.
    //
    // Off-centre and with a tighter feather, so the reference covers a different part of the
    // parameter space from the case above.
    check_golden(
        "mask_radial_inverted_saturation",
        &Edits {
            masks: vec![edit_model::MaskedAdjustment {
                mask: edit_model::Mask::Radial(edit_model::RadialGradient {
                    center: [0.3, 0.5],
                    radius: [0.25, 0.6],
                    feather: 25.0,
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

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p pipeline --test golden mask_radial 2>&1 | tail -20`
Expected: FAIL — "missing golden mask_radial_exposure; run with UPDATE_GOLDEN=1".

- [ ] **Step 3: Generate the references**

Run: `UPDATE_GOLDEN=1 cargo test -p pipeline --test golden mask_radial 2>&1 | tail -5`
Expected: PASS (the harness writes rather than compares).

- [ ] **Step 4: Look at the generated PNGs**

Run: `open crates/pipeline/tests/golden/mask_radial_exposure.png crates/pipeline/tests/golden/mask_radial_inverted_saturation.png`

⚠ **This step is not optional.** A golden nobody has looked at is just a hash — that lesson cost a whole chart redesign during the noise-reduction work. Check by eye:
- `mask_radial_exposure`: a brightened elliptical blob centred in the frame, fading smoothly outward, with the four colour bands still visible through it.
- `mask_radial_inverted_saturation`: the reverse — a left-of-centre ellipse keeps its colour while everything around it is grey and darker.

If either looks wrong, the bug is in the implementation, not the reference. Do not commit a reference you cannot explain.

- [ ] **Step 5: Verify the existing 33 are byte-identical**

⚠ **This is the actual acceptance criterion for the whole feature** — it is the proof the variant is additive and that nothing in the shared path shifted.

Run:
```bash
git status --short crates/pipeline/tests/golden/
```
Expected: exactly two lines, both `??` (untracked), for the two new PNGs. **No `M` lines.** If any existing golden shows as modified, stop — something in the shared path changed and that must be understood before going further.

Run: `cargo test -p pipeline --test golden 2>&1 | tail -5`
Expected: PASS, 35 tests.

- [ ] **Step 6: Commit**

```bash
git add crates/pipeline/tests/golden.rs crates/pipeline/tests/golden/mask_radial_exposure.png crates/pipeline/tests/golden/mask_radial_inverted_saturation.png
git commit -F - <<'EOF'
test(pipeline): goldens for the radial mask

Two cases — a centred exposure lift, and the inverted subject vignette that
`invert` exists for.

The existing 33 references came out byte-identical, which is the proof the new
variant is additive and that nothing in the shared path shifted.
EOF
```

---

## Task 7: End-to-end validation and documentation

**Files:**
- Modify: `CLAUDE.md`
- Modify: `docs/superpowers/specs/2026-08-18-radial-mask-design.md` (fill in the verdict section)

- [ ] **Step 1: Full workspace verification**

Run each and confirm before moving on:

```bash
cargo fmt --all --check
```
Expected: no output.

```bash
cargo clippy --all-targets --all-features 2>&1 | tail -5
```
Expected: no warnings.

```bash
cargo test --all 2>&1 | grep -E '^test result' 
```
Expected: every line `ok`, and the total test count has risen from 559 to about 582.

- [ ] **Step 2: Validate on a real RAW through the CLI**

⚠ Validate against a real photograph, not only synthetic charts — every mask feature in this project has been checked this way, and the headless half of 3.1a was validated by measuring rather than squinting.

Find a RAW to work with:

```bash
ls ~/Pictures/**/*.RAF 2>/dev/null | head -3
```

If none is found, ask the user for a path to a `.RAF` or other RAW file before continuing.

Write a sidecar with an inverted radial that darkens everything but a centre ellipse, then render it, substituting your RAW's path:

```bash
cat > /tmp/radial.json <<'EOF'
{
  "masks": [
    {
      "mask": { "Radial": { "center": [0.5, 0.5], "radius": [0.3, 0.3], "feather": 60.0, "invert": true } },
      "adjustments": { "exposure": -1.5 }
    }
  ]
}
EOF
```

```bash
cargo run --release -p rustroom -- render <YOUR.RAF> /tmp/radial.jpg --edits /tmp/radial.json
```

Expected: it writes `/tmp/radial.jpg`. Open it and confirm the centre is at its original brightness while the corners are visibly darkened, with a smooth falloff between.

⚠ **Measure rather than squint**, as the 3.1a validation did. Compare a centre pixel against a corner pixel:

```bash
python3 -c "
from PIL import Image
im = Image.open('/tmp/radial.jpg').convert('L')
w, h = im.size
print('centre', im.getpixel((w//2, h//2)))
print('corner', im.getpixel((w//20, h//20)))
"
```
Expected: the corner is substantially darker than the centre. If they are close, the mask is not reaching the render.

- [ ] **Step 3: Launch the GUI**

⚠ The user wants the GUI started after every big update, without being asked.

```bash
cargo run --release -p app
```

Run it in the background and tell the user what to look at:
- The Masks section now has **two** add buttons.
- Add a radial filter: an ellipse with five handles appears centred on the photo.
- Drag the centre to move it; drag an edge handle to resize about that centre.
- The **Invert** checkbox is new and applies to gradients too.
- Feather appears only for a radial.

- [ ] **Step 4: Fill in the spec's verdict section**

Replace the "To be filled in on completion, but the prediction from the design work:" line in `docs/superpowers/specs/2026-08-18-radial-mask-design.md` with what actually happened, correcting any item where the prediction was wrong. ⚠ Where the prediction was wrong, say so explicitly — a spec that quietly matches its own prediction teaches nothing to 3.1c.

- [ ] **Step 5: Update CLAUDE.md**

In the RESUME HERE block:
- Change **▶ NEXT ACTION** to **M3.1c — the brush**, noting that it is what closes M3.1 and that the parked button/layout work resumes after it.
- Add a section for M3.1b following the house style of the existing sections: what landed, the ⚠ items worth reading, and the accepted caveats from the spec.
- The ⚠ items that must appear: (1) **the aspect correction does not come back for an axis-aligned ellipse, and rotation is precisely what would bring it back**; (2) `hit_test` still needs it, so the two functions differ deliberately; (3) `invert` had no UI since 3.1a; (4) the hand-written `Default` and why `#[serde(default)]` forces it; (5) the verdict on whether the architecture held, and the note that the brush will test the `Handle` decision much harder.
- Update the test count and add the new spec and plan filenames to the docs list at the bottom.
- Update the "Push state" line by running `git log origin/main..main --oneline | wc -l`.

- [ ] **Step 6: Commit the documentation**

```bash
git add CLAUDE.md docs/superpowers/specs/2026-08-18-radial-mask-design.md docs/superpowers/plans/2026-08-18-radial-mask.md
git commit -F - <<'EOF'
docs: record M3.1b, and the verdict on 3.1a's architecture

The load-bearing finding: an axis-aligned ellipse needs no aspect correction,
because anisotropic scaling preserves axis-alignment. Rotation is precisely
what would bring the correction back, which is the main argument for leaving
it out rather than the extra handle.
EOF
```

- [ ] **Step 7: Report, do not push**

⚠ **Never `git push`.** Report the unpushed count and let the user push:

```bash
git log origin/main..main --oneline | wc -l
```
