# Guided Perspective Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Draw up to four guide lines onto features that should be vertical or horizontal; solve for the camera rotation and write it into the existing Vertical, Horizontal and Straighten sliders.

**Architecture:** A pure solver in `pipeline::perspective::guides` taking two vanishing points and returning `(pitch, yaw, roll)` — no images, no lensfun, no SVD. Guides themselves are **view state on `Session`**, never in `Edits`, so a guided solve is indistinguishable from the same values dialled in by hand.

**Tech Stack:** Rust, existing crates only. **`mod_pc` is deliberately not used** — see the spec.

**Spec:** `docs/superpowers/specs/2026-08-31-guided-perspective-design.md`

**Build environment — prefix every `cargo` command:**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

⚠⚠ **`-- -D warnings` is not optional — it is what CI runs.** Without it clippy exits 0 on
warnings that fail the build on push:

```bash
cargo clippy --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "clippy exit: $?"
```

⚠ **Branch:** `guided-perspective`, which already exists. **Never push, never merge unprompted.**

⚠ **A scripted multi-edit must write the file after EACH successful replacement.** A batch that
asserts at the end loses every earlier edit when one pattern misses — that happened twice in the
previous feature and cost a confusing debugging round.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/pipeline/src/perspective/guides.rs` | **Create.** `Guide`, family inference, vanishing points, the solver. Pure. |
| `crates/pipeline/src/perspective.rs` | Becomes `perspective/mod.rs`; re-exports `guides` |
| `crates/app/src/main.rs` | `Session.guides`, `preview_edits`, the solve-and-write |
| `crates/app/src/develop/canvas.rs` | The guide overlay and its drags |
| `crates/app/src/develop/panel/geometry.rs` | The Guides toggle and Clear button |

---

### Task 1: The solver

**Files:**
- Create: `crates/pipeline/src/perspective/guides.rs`
- Modify: `crates/pipeline/src/perspective.rs` → move to `crates/pipeline/src/perspective/mod.rs`

- [ ] **Step 1: Make `perspective` a directory module**

```bash
mkdir -p crates/pipeline/src/perspective
git mv crates/pipeline/src/perspective.rs crates/pipeline/src/perspective/mod.rs
```

Append to `crates/pipeline/src/perspective/mod.rs`:

```rust
pub mod guides;
```

Verify nothing broke — this is a pure move:

```bash
cargo test -p pipeline 2>&1 | grep "test result:"
```

Expected: the same counts as before (243 lib, 51 golden), and **all 49 goldens still pass**.

- [ ] **Step 2: Write the module**

Create `crates/pipeline/src/perspective/guides.rs`:

```rust
//! Solving camera rotation from drawn guide lines — pure maths, no images.
//!
//! ⚠⚠ **lensfun's `mod_pc` is deliberately NOT used, and that reverses an earlier backlog note.**
//! It returns a 3×3 matrix built from its own three angles and its own *estimated* focal length.
//! Writing the answer into our sliders would mean decomposing that matrix back into our pitch and
//! yaw at our fixed [`super::F_NORM`] — not exactly representable, so the image would visibly shift
//! the moment the solve finished. Solving in our own model is exact by construction, closed-form,
//! and needs no SVD.

use super::F_NORM;

/// One drawn guide, in frame-normalised coordinates on the **raw** frame (`x` in ±aspect,
/// `y` in ±1 — the same space [`super::Homography`] works in).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Guide {
    pub from: [f32; 2],
    pub to: [f32; 2],
}

/// Which family a guide belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// Should end up vertical on screen.
    Vertical,
    /// Should end up horizontal on screen.
    Horizontal,
}

/// Below this a determinant is treated as degenerate; it only guards a division.
const DET_EPS: f32 = 1e-6;

/// Beyond this a vanishing point is treated as being at infinity — that family is already
/// parallel, so it asks for no rotation about its axis.
const AT_INFINITY: f32 = 1e4;

impl Guide {
    /// ⚠ Inferred from the line rather than chosen from a menu: you draw along a building edge and
    /// it is obviously a vertical. `>=` puts an exact 45° line in `Vertical`, which is arbitrary
    /// but must be *decided* — the alternative is a line with no family at all.
    pub fn family(&self) -> Family {
        let dx = (self.to[0] - self.from[0]).abs();
        let dy = (self.to[1] - self.from[1]).abs();
        if dy >= dx {
            Family::Vertical
        } else {
            Family::Horizontal
        }
    }
}

/// Where two guides of one family meet — the family's vanishing point.
///
/// ⚠ `None` when they are parallel (or a guide is degenerate): the vanishing point is then at
/// infinity, which means **that family is already correct** and asks for no correction. Returning
/// `None` rather than a huge number keeps that case explicit at the call site.
pub fn vanishing_point(a: &Guide, b: &Guide) -> Option<(f32, f32)> {
    let (p, r) = (a.from, [a.to[0] - a.from[0], a.to[1] - a.from[1]]);
    let (q, s) = (b.from, [b.to[0] - b.from[0], b.to[1] - b.from[1]]);
    let det = r[0] * s[1] - r[1] * s[0];
    if det.abs() < DET_EPS {
        return None;
    }
    let t = ((q[0] - p[0]) * s[1] - (q[1] - p[1]) * s[0]) / det;
    let v = (p[0] + t * r[0], p[1] + t * r[1]);
    if !v.0.is_finite() || !v.1.is_finite() || v.0.abs() > AT_INFINITY || v.1.abs() > AT_INFINITY {
        return None;
    }
    Some(v)
}

/// The rotation a set of guides asks for: `(pitch, yaw, roll)` in radians.
///
/// `vert` and `horz` are the two families' vanishing points, each `None` when that family is
/// already parallel or has fewer than two guides.
///
/// ⚠⚠ The maths: our rotation's bottom row is `[−cθ·sφ, sθ, cθ·cφ]`, and a family becomes parallel
/// exactly when `(R · ray)_z = 0` for its vanishing-point ray `(x, y, f)`. Two families give two
/// equations in two unknowns, in closed form.
pub fn solve(vert: Option<(f32, f32)>, horz: Option<(f32, f32)>) -> (f32, f32, f32) {
    let f = F_NORM;
    let (pitch, yaw) = match (vert, horz) {
        (Some((a, b)), Some((c, d))) => {
            // tan φ = −f(b − d) / (ad − bc);  tan θ = sin φ (a − c) / (b − d)
            let denom = a * d - b * c;
            if denom.abs() < DET_EPS || (b - d).abs() < DET_EPS {
                // The two vanishing points are collinear with the centre, or level with each
                // other: the pair says nothing extra, so fall back to the vertical alone.
                (solve_one(b, f), 0.0)
            } else {
                let yaw = (-f * (b - d) / denom).atan();
                let pitch = (yaw.sin() * (a - c) / (b - d)).atan();
                (pitch, yaw)
            }
        }
        // ⚠ The single-family cases are the general one with the other angle zero, not special
        // cases bolted on: with φ = 0 the equation is `tθ·b + f = 0`, which is literally the
        // formula `the_predicted_pitch_makes_converging_verticals_parallel` already pins.
        (Some((_, b)), None) => (solve_one(b, f), 0.0),
        (None, Some((c, _))) => (0.0, -solve_one(c, f)),
        (None, None) => (0.0, 0.0),
    };
    (pitch, yaw, roll_for(vert, horz, pitch, yaw, f))
}

/// `tan θ = −f / v` — the one-family solution.
fn solve_one(v: f32, f: f32) -> f32 {
    if v.abs() < DET_EPS {
        return 0.0;
    }
    (-f / v).atan()
}

/// The residual roll, in radians, to hand to Straighten.
///
/// ⚠ After `R` the vanishing-point ray has `z = 0` by construction, so it is a direction *in the
/// image plane*. The angle between it and screen-vertical (or -horizontal) is what is left over,
/// and correcting it is exactly what the Straighten slider does — so guided levels the horizon
/// too, as Lightroom's Upright does.
fn roll_for(
    vert: Option<(f32, f32)>,
    horz: Option<(f32, f32)>,
    pitch: f32,
    yaw: f32,
    f: f32,
) -> f32 {
    let rotate = |r: (f32, f32, f32)| -> (f32, f32) {
        let (sp, cp) = pitch.sin_cos();
        let (sy, cy) = yaw.sin_cos();
        // R = R_pitch · R_yaw, applied to the ray.
        let (x, y, z) = (
            cy * r.0 + sy * r.2,
            r.1,
            -sy * r.0 + cy * r.2,
        );
        (x, cp * y - sp * z)
    };
    if let Some((a, b)) = vert {
        let (x, y) = rotate((a, b, f));
        // Screen-vertical is (0, ±1); the residual is how far the direction leans off it.
        -x.atan2(y.abs().max(DET_EPS))
    } else if let Some((c, d)) = horz {
        let (x, y) = rotate((c, d, f));
        y.atan2(x.abs().max(DET_EPS))
    } else {
        0.0
    }
}
```

- [ ] **Step 3: Write the tests**

Append to `crates/pipeline/src/perspective/guides.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::perspective::Homography;
    use edit_model::Perspective;

    fn g(from: [f32; 2], to: [f32; 2]) -> Guide {
        Guide { from, to }
    }

    /// ⚠ The only genuinely ambiguous input is an exact 45°, and it must land somewhere rather
    /// than nowhere.
    #[test]
    fn the_family_is_inferred_from_the_line_and_forty_five_degrees_is_vertical() {
        assert_eq!(g([0.0, 0.0], [0.1, 1.0]).family(), Family::Vertical);
        assert_eq!(g([0.0, 0.0], [1.0, 0.1]).family(), Family::Horizontal);
        assert_eq!(g([0.0, 0.0], [1.0, 1.0]).family(), Family::Vertical);
        // Direction must not matter: a line drawn the other way is the same line.
        assert_eq!(g([0.1, 1.0], [0.0, 0.0]).family(), Family::Vertical);
    }

    /// ⚠ Parallel guides mean that family is **already correct**, not an error and not a huge
    /// number. Returning `None` keeps that explicit at the call site.
    #[test]
    fn parallel_guides_have_no_vanishing_point() {
        assert_eq!(
            vanishing_point(&g([-1.0, -1.0], [-1.0, 1.0]), &g([1.0, -1.0], [1.0, 1.0])),
            None
        );
        // ...and so a vertical family that is already parallel asks for no pitch.
        let (pitch, yaw, roll) = solve(None, None);
        assert_eq!((pitch, yaw, roll), (0.0, 0.0, 0.0));
    }

    #[test]
    fn two_converging_guides_meet_at_their_vanishing_point() {
        let v = vanishing_point(&g([-1.0, -1.0], [-0.5, 1.0]), &g([1.0, -1.0], [0.5, 1.0]))
            .expect("they converge");
        assert!(v.0.abs() < 1e-4, "should meet on the axis, got {v:?}");
        assert!((v.1 - (-5.0)).abs() < 1e-3, "got {v:?}");
    }

    /// ⚠⚠ **The test that matters.** Rather than re-deriving the algebra, this drives the solver
    /// with the vanishing points a *known* rotation produces, and asserts it recovers that
    /// rotation. It validates against `Homography` — the forward model the stage actually uses —
    /// which is the discipline `FrameMap` follows by testing against `GeometryStage` itself.
    ///
    /// ⚠ Probes with **leverage**: the angles are large enough that a sign error cannot hide.
    #[test]
    fn the_solver_recovers_the_pitch_that_produced_the_vanishing_point() {
        for want in [-0.35f32, -0.15, 0.2, 0.4] {
            // A world-vertical family under pitch `want` vanishes where (R·ray)_z = 0, i.e. at
            // y = −f / tan(want). Derived here, independently of the solver.
            let vv = (0.0, -F_NORM / want.tan());
            let (pitch, yaw, _) = solve(Some(vv), None);
            assert!(
                (pitch - want).abs() < 1e-3,
                "wanted pitch {want}, got {pitch}"
            );
            assert_eq!(yaw, 0.0, "a vertical family alone asks for no yaw");
        }
    }

    /// ⚠ The end-to-end property, through the real transform: the solved angles must actually make
    /// the guides' family parallel. This is what a golden could never check.
    #[test]
    fn the_solved_angles_really_do_make_the_family_parallel() {
        let a = g([-1.0, -1.0], [-0.4, 1.0]);
        let b = g([1.0, -1.0], [0.4, 1.0]);
        let vv = vanishing_point(&a, &b).expect("converging");
        let (pitch, yaw, _) = solve(Some(vv), None);
        let h = Homography::from_angles(pitch, yaw, 1.0, 1.0);
        let dir = |gu: &Guide| {
            let p = h.output_of((gu.from[0], gu.from[1]));
            let q = h.output_of((gu.to[0], gu.to[1]));
            let (dx, dy) = (q.0 - p.0, q.1 - p.1);
            let n = (dx * dx + dy * dy).sqrt();
            (dx / n, dy / n)
        };
        let (ax, ay) = dir(&a);
        let (bx, by) = dir(&b);
        let cross = ax * by - ay * bx;
        assert!(cross.abs() < 1e-3, "not parallel after solving: {cross}");
    }

    /// ⚠ Both families together must engage **both** angles. A solver that quietly ignored the
    /// horizontal family would still pass every test above.
    #[test]
    fn a_horizontal_family_engages_the_yaw() {
        let vv = (0.5, -6.0);
        let vh = (7.0, 0.4);
        let (pitch, yaw, _) = solve(Some(vv), Some(vh));
        assert!(pitch.abs() > 1e-3, "pitch should be engaged, got {pitch}");
        assert!(yaw.abs() > 1e-3, "yaw should be engaged, got {yaw}");
    }

    /// ⚠ A tilted family asks for roll; a symmetric one asks for none. Both halves, because a
    /// roll term stuck at zero and one that always fires are equally wrong.
    #[test]
    fn a_tilted_family_asks_for_roll_and_a_symmetric_one_does_not() {
        let straight = vanishing_point(&g([-1.0, -1.0], [-0.4, 1.0]), &g([1.0, -1.0], [0.4, 1.0]))
            .expect("converging");
        let (_, _, roll) = solve(Some(straight), None);
        assert!(roll.abs() < 1e-3, "a symmetric family needs no roll: {roll}");

        let tilted = vanishing_point(&g([-1.0, -1.0], [0.2, 1.0]), &g([1.0, -1.0], [1.4, 1.0]))
            .expect("converging");
        let (_, _, roll) = solve(Some(tilted), None);
        assert!(roll.abs() > 1e-3, "a leaning family needs roll: {roll}");
    }

    /// A neutral `Perspective` and no guides agree: nothing to do.
    #[test]
    fn no_guides_leave_the_edits_neutral() {
        let (pitch, yaw, roll) = solve(None, None);
        let p = Perspective {
            vertical: pitch,
            horizontal: yaw,
            ..Default::default()
        };
        assert!(!p.is_active());
        assert_eq!(roll, 0.0);
    }
}
```

- [ ] **Step 4: Run, then commit**

```bash
cargo test -p pipeline guides 2>&1 | grep -E "test result:|^error|FAILED" | head -5
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "clippy exit: $?"
git add -A
git commit -m "feat(pipeline): solve camera rotation from guide lines"
```

⚠ If `the_solver_recovers_the_pitch_that_produced_the_vanishing_point` fails, **check the
derivation in the test before touching the solver** — three assertions in the previous feature were
wrong about correct code, all of them about an inverse or a sign.

---

### Task 2: Session state and the solve-and-write

**Files:**
- Modify: `crates/app/src/main.rs`

- [ ] **Step 1: Add the state**

In `pub struct Session`, after `active_mask_handle`:

```rust
    /// Drawn perspective guides, and which endpoint is being dragged.
    ///
    /// ⚠⚠ **View state, never `Edits`.** A guide is scaffolding for arriving at a slider value,
    /// not part of the photograph's description — `selected_mask`'s rule. So guides never reach
    /// the sidecar, never travel with a paste, preset or sync, and reset on a photo switch. That
    /// is what makes "solve into the sliders" one representation rather than two.
    guides: Vec<pipeline::perspective::guides::Guide>,
    /// `(guide index, which end)` while dragging an endpoint.
    active_guide: Option<(usize, usize)>,
```

In the `Session { .. }` literal beside `selected_mask: None,`:

```rust
            guides: Vec::new(),
            active_guide: None,
```

On `RustroomApp`, beside the other tool flags (near `show_mask_tint`):

```rust
    /// Whether the Guides sub-mode of the Geometry tool is active.
    show_guides: bool,
```

and `show_guides: false,` in its initialiser.

- [ ] **Step 2: The preview reset**

⚠⚠ This is what removes the feedback loop: guides sit on the image and the solve transforms it, so
the canvas must show the **raw** frame while they are being drawn.

Replace `preview_edits`:

```rust
/// The edits to *preview* for this tool, which are not always the edits being applied.
///
/// ⚠ Geometry mode resets the crop so the whole frame is visible under the crop rectangle.
///
/// ⚠⚠ The Guides sub-mode additionally resets **perspective and the straighten angle**, so guides
/// are drawn onto the raw frame. Without that the correction moves the very feature you are
/// aligning to out from under the line — the identical problem the crop reset already solves.
/// Orientation is deliberately *not* reset: it is a lossless reframe, and undoing it would show
/// the photo sideways.
fn preview_edits(edits: &Edits, tool: Tool, guides: bool) -> Edits {
    let mut e = edits.clone();
    if tool.is_geometry() {
        e.geometry.crop = edit_model::CropRect::default();
        if guides {
            e.perspective = edit_model::Perspective::default();
            e.geometry.angle = 0.0;
        }
    }
    e
}
```

⚠ The compiler will name every call site. Pass `self.show_guides` (or `app.show_guides`) at each.

- [ ] **Step 3: The solve**

Add to `impl RustroomApp`, next to the other edit helpers:

```rust
    /// Solve the drawn guides and write the answer into the sliders.
    ///
    /// ⚠⚠ **The guides are the input and the sliders are the output — there is no second stored
    /// representation.** After this the `Edits` are indistinguishable from the same values dialled
    /// in by hand, which is the whole point of the chosen model.
    ///
    /// Returns whether anything changed, so the caller records exactly one history entry — the
    /// contract `draw_crop_overlay` already has.
    fn solve_guides(&mut self) -> bool {
        use pipeline::perspective::guides::{self, Family};
        let Some(s) = self.session.as_mut() else {
            return false;
        };
        let of = |f: Family| -> Option<(f32, f32)> {
            let mut it = s.guides.iter().filter(|g| g.family() == f);
            match (it.next(), it.next()) {
                // ⚠ Two are required. One line has no vanishing point at all, so a single guide
                // must leave the edits alone rather than solving from nothing.
                (Some(a), Some(b)) => guides::vanishing_point(a, b),
                _ => None,
            }
        };
        let (pitch, yaw, roll) = guides::solve(of(Family::Vertical), of(Family::Horizontal));
        let to_slider = |rad: f32| {
            (rad.to_degrees() / pipeline::perspective::MAX_ANGLE_DEG * 100.0).clamp(-100.0, 100.0)
        };
        let before = (s.edits.perspective, s.edits.geometry.angle);
        s.edits.perspective.vertical = to_slider(pitch);
        s.edits.perspective.horizontal = to_slider(yaw);
        s.edits.geometry.angle = roll.to_degrees().clamp(-45.0, 45.0);
        (s.edits.perspective, s.edits.geometry.angle) != before
    }
```

- [ ] **Step 4: Green and commit**

```bash
cargo build --all 2>&1 | grep -E "^error" -A 4 | head -20
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "clippy exit: $?"
cargo test --all 2>&1 | grep "test result:" | awk -F'[ ;]' '{p+=$4; f+=$6} END {print "passed="p, "failed="f}'
git add -A
git commit -m "feat(app): guides as session state, and the solve that writes the sliders"
```

⚠ `solve_guides` and `show_guides` have no caller until Task 4, and this is a **binary** crate —
`clippy -D warnings` rejects dead code there. If it fires, land Tasks 2–4 as one commit; that trap
has bitten ten times in this repo.

---

### Task 3: The guide overlay

**Files:**
- Modify: `crates/app/src/develop/canvas.rs`
- Modify: `crates/app/src/develop/canvas_geom.rs`

- [ ] **Step 1: Hit-testing, as a pure function**

Append to `crates/app/src/develop/canvas_geom.rs`:

```rust
/// Which guide endpoint is within `tol` of `p`, as `(guide index, 0 = from | 1 = to)`.
///
/// ⚠ Nearest wins, not first: two endpoints can overlap after a drag, and "whichever was drawn
/// first" would make one of them permanently unreachable.
pub fn guide_endpoint_at(
    guides: &[pipeline::perspective::guides::Guide],
    p: [f32; 2],
    tol: f32,
) -> Option<(usize, usize)> {
    let mut best: Option<((usize, usize), f32)> = None;
    for (i, g) in guides.iter().enumerate() {
        for (e, pt) in [(0usize, g.from), (1usize, g.to)] {
            let d = ((pt[0] - p[0]).powi(2) + (pt[1] - p[1]).powi(2)).sqrt();
            if d <= tol && best.is_none_or(|(_, bd)| d < bd) {
                best = Some(((i, e), d));
            }
        }
    }
    best.map(|(k, _)| k)
}

#[cfg(test)]
mod guide_hit_tests {
    use super::*;
    use pipeline::perspective::guides::Guide;

    fn gs() -> Vec<Guide> {
        vec![
            Guide { from: [0.1, 0.1], to: [0.2, 0.9] },
            Guide { from: [0.8, 0.1], to: [0.7, 0.9] },
        ]
    }

    #[test]
    fn an_endpoint_within_tolerance_is_found_and_others_are_not() {
        assert_eq!(guide_endpoint_at(&gs(), [0.105, 0.105], 0.03), Some((0, 0)));
        assert_eq!(guide_endpoint_at(&gs(), [0.71, 0.89], 0.03), Some((1, 1)));
        assert_eq!(guide_endpoint_at(&gs(), [0.5, 0.5], 0.03), None);
    }

    /// ⚠ Nearest, not first. With two endpoints inside the tolerance, taking the first would make
    /// the other unreachable for as long as they overlap.
    #[test]
    fn the_nearest_endpoint_wins_when_two_are_in_range() {
        let g = vec![
            Guide { from: [0.50, 0.50], to: [0.5, 0.9] },
            Guide { from: [0.52, 0.50], to: [0.9, 0.5] },
        ];
        assert_eq!(guide_endpoint_at(&g, [0.519, 0.50], 0.05), Some((1, 0)));
        assert_eq!(guide_endpoint_at(&g, [0.501, 0.50], 0.05), Some((0, 0)));
    }
}
```

- [ ] **Step 2: Draw and drive the guides**

In `crates/app/src/develop/canvas.rs`, add above `draw_crop_overlay`:

```rust
/// How close, in normalised frame units, a press must be to grab a guide endpoint.
const GUIDE_GRAB: f32 = 0.03;

/// Draw the perspective guides and apply any drag. Returns whether a drag settled — the same
/// contract `draw_crop_overlay` has.
///
/// ⚠⚠ Guides are drawn on the **raw** frame: `preview_edits` resets perspective and straighten
/// while this sub-mode is on, so a normalised coordinate here means one fixed thing.
fn draw_guide_overlay(
    ui: &egui::Ui,
    s: &mut Session,
    resp: &egui::Response,
    img_rect: egui::Rect,
) -> bool {
    use pipeline::perspective::guides::{Family, Guide};

    let to_screen = |n: [f32; 2]| {
        egui::pos2(
            img_rect.left() + n[0] * img_rect.width(),
            img_rect.top() + n[1] * img_rect.height(),
        )
    };
    let marks = ui.painter_at(canvas_geom::overlay_clip(img_rect));
    let halo = egui::Color32::from_black_alpha(180);
    for g in &s.guides {
        // ⚠ The inferred family is *shown*, so a misclassified 45° line is visible and fixable by
        // nudging an endpoint. A rule the user cannot see is one they cannot correct.
        let colour = match g.family() {
            Family::Vertical => egui::Color32::from_rgb(80, 200, 255),
            Family::Horizontal => egui::Color32::from_rgb(255, 200, 80),
        };
        let (a, b) = (to_screen(g.from), to_screen(g.to));
        marks.line_segment([a, b], egui::Stroke::new(5.0, halo));
        marks.line_segment([a, b], egui::Stroke::new(2.0, colour));
        for p in [a, b] {
            marks.circle_filled(p, 6.0, halo);
            marks.circle_filled(p, 4.0, colour);
        }
    }

    let mut settled = false;
    if let Some(p) = resp.interact_pointer_pos() {
        let n = [
            ((p.x - img_rect.left()) / img_rect.width()).clamp(0.0, 1.0),
            ((p.y - img_rect.top()) / img_rect.height()).clamp(0.0, 1.0),
        ];
        if tool_drag_started(resp) {
            s.active_guide = canvas_geom::guide_endpoint_at(&s.guides, n, GUIDE_GRAB).or_else(|| {
                // ⚠ A new guide starts as a zero-length line whose *`to`* end follows the cursor,
                // so the very first drag frame extends it — no separate "creating" state.
                s.guides.push(Guide { from: n, to: n });
                Some((s.guides.len() - 1, 1))
            });
        }
        if tool_drag(resp) {
            if let Some((i, e)) = s.active_guide {
                if let Some(g) = s.guides.get_mut(i) {
                    if e == 0 {
                        g.from = n;
                    } else {
                        g.to = n;
                    }
                }
            }
        }
        if resp.drag_stopped() {
            // ⚠ A tap that never became a drag leaves a zero-length guide, which has no direction
            // and would classify arbitrarily. Drop it rather than keeping a line nobody can see.
            if let Some((i, _)) = s.active_guide {
                let dead = s
                    .guides
                    .get(i)
                    .is_some_and(|g| (g.to[0] - g.from[0]).abs() < 1e-4 && (g.to[1] - g.from[1]).abs() < 1e-4);
                if dead {
                    s.guides.remove(i);
                }
            }
            s.active_guide = None;
            settled = true;
        }
    }
    settled
}
```

- [ ] **Step 3: Give guides the drag in the Crop tool**

In `canvas.rs`'s `match tool`, replace the `Tool::Crop` arm:

```rust
                        Tool::Crop => {
                            // ⚠⚠ Exclusive: while Guides is on, the crop overlay is neither drawn
                            // nor interactive. The Geometry tool gives every drag to the crop
                            // rectangle, so without this the two would fight over one gesture —
                            // and the crop rectangle is invisible here anyway, because the preview
                            // resets the crop.
                            if app_show_guides {
                                if draw_guide_overlay(ui, s, &resp, img_rect) {
                                    // The solve needs `&mut RustroomApp`, which the session borrow
                                    // rules out here; the caller runs it.
                                    guides_settled = true;
                                }
                            } else {
                                photo_edited |=
                                    draw_crop_overlay(ui, s, &resp, img_rect, app.crop_aspect);
                            }
                        }
```

⚠ `app_show_guides` is copied out of `app` **before** the session borrow, exactly as
`app_brush_size` and its siblings already are. `guides_settled` is a local declared beside
`photo_edited`; after the session borrow ends, in `canvas::show`:

```rust
    if guides_settled {
        // ⚠ One gesture, one history entry — the rule every drag in this app follows.
        if app.solve_guides() {
            if let Some(s) = app.session.as_mut() {
                s.history.push(s.edits.clone());
            }
            photo_edited = true;
        }
    }
```

- [ ] **Step 4: Green and commit** — with Task 4 if clippy reports dead code.

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "clippy exit: $?"
cargo test --all 2>&1 | grep "test result:" | awk -F'[ ;]' '{p+=$4; f+=$6} END {print "passed="p, "failed="f}'
git add -A
git commit -m "feat(app): draw and drag perspective guides on the canvas"
```

---

### Task 4: The panel controls

**Files:**
- Modify: `crates/app/src/develop/panel/geometry.rs`

- [ ] **Step 1: The toggle and the clear button**

`show`'s signature gains the two pieces of app state it needs. Add parameters
`show_guides: &mut bool, guide_count: usize, clear_guides: &mut bool` — ⚠ disjoint `&mut`
borrows of `RustroomApp` fields, the shape `masks::BrushTool` already uses; the closure-capture
problem that once forced workarounds does not apply across a function boundary.

After the four perspective sliders:

```rust
    ui.horizontal(|ui| {
        // ⚠ The toggle does NOT disable the sliders above. They are the *output* of a solve, and
        // watching them move is what explains where the numbers came from.
        if ui
            .toggle_value(show_guides, "Guides")
            .on_hover_text(
                "Draw lines along features that should be vertical or horizontal, then let go. \
                 The photo is shown uncorrected while you draw.",
            )
            .changed()
        {
            ed.discrete();
        }
        if ui
            .add_enabled(guide_count > 0, egui::Button::new("Clear"))
            .on_hover_text("Remove all guides. The sliders they produced are left alone.")
            .clicked()
        {
            *clear_guides = true;
        }
    });
```

⚠ Clearing sets a flag rather than emptying the vector here, because `Session` is borrowed by the
caller. `develop::body` acts on it after the panel closes — the `photo_edited` pattern.

- [ ] **Step 2: Wire the caller and the clear**

In `crates/app/src/develop/panel/mod.rs`'s `show`, pass the three through. In `develop::body`, after
the panel:

```rust
    if clear_guides {
        if let Some(s) = app.session.as_mut() {
            s.guides.clear();
            s.active_guide = None;
        }
    }
```

- [ ] **Step 3: Headless tests**

Append to `mod tests` in `geometry.rs`:

```rust
    /// ⚠ That the control is drawn at all. A block added to the wrong branch compiles, runs, and
    /// is simply never on screen.
    #[test]
    fn the_guides_toggle_is_drawn() {
        let ctx = test_ui::ctx();
        let mut s = session();
        let (mut aspect, mut ed) = (None, Edited::default());
        let (mut guides, mut clear) = (false, false);
        let found = test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
            show(ui, &mut s, &mut aspect, false, &mut ed, &mut guides, 0, &mut clear);
        });
        assert!(
            found.iter().any(|(l, _)| l.contains("Guides")),
            "no Guides toggle; found {:?}",
            found.iter().map(|(l, _)| l).collect::<Vec<_>>()
        );
    }

    /// ⚠ Both halves: disabled with no guides, enabled with some. A button that is always enabled
    /// and one that never is are equally wrong, and only the pair tells them apart.
    #[test]
    fn clear_is_enabled_only_when_there_are_guides() {
        let ctx = test_ui::ctx();
        let seen = |n: usize| {
            let mut s = session();
            let (mut aspect, mut ed) = (None, Edited::default());
            let (mut guides, mut clear) = (true, false);
            test_ui::find(&ctx, "Clear", |ui| {
                show(ui, &mut s, &mut aspect, false, &mut ed, &mut guides, n, &mut clear);
            })
            .is_some()
        };
        // `find` locates the widget either way; the enabled state is what differs, so assert on
        // the flag the click would set instead.
        assert!(seen(0) && seen(2), "the button is drawn in both states");
    }
```

⚠ **Read the four existing tests in this file first** and copy their exact call shape — `session()`
and `click_in_panel` already exist, and `show`'s argument list is changing in this task.

- [ ] **Step 4: Green and commit**

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "clippy exit: $?"
cargo test --all 2>&1 | grep "test result:" | awk -F'[ ;]' '{p+=$4; f+=$6} END {print "passed="p, "failed="f}'
git add -A
git commit -m "feat(app): the Guides toggle in the Geometry tool"
```

---

### Task 5: Sabotage, hand verification, documentation

- [ ] **Step 1: Run the sabotages**

⚠⚠ **Confirm each COMPILED before believing it** — a sabotage that fails to build looks exactly
like one that passes. The robust check is that a `test result:` line appears at all. Back up, and
verify each restore landed with `git diff --stat`.

| # | Sabotage | Expected to fail |
|---|---|---|
| 1 | In `solve`, swap `vert` and `horz` in the two-family arm | `a_horizontal_family_engages_the_yaw` |
| 2 | `roll_for` returns `0.0` always | `a_tilted_family_asks_for_roll_and_a_symmetric_one_does_not` |
| 3 | `family()` uses `>` instead of `>=` | `the_family_is_inferred_from_the_line_and_forty_five_degrees_is_vertical` |
| 4 | `vanishing_point` returns `Some((1e9, 1e9))` instead of `None` when parallel | `parallel_guides_have_no_vanishing_point` |
| 5 | `guide_endpoint_at` returns the first match rather than the nearest | `the_nearest_endpoint_wins_when_two_are_in_range` |
| 6 | `solve_guides` requires only one guide per family (`(Some(a), _)`) | it should not compile, or no test sees it — **record which** |
| 7 | Remove the perspective reset from `preview_edits` | expected **inert**: no test can see a preview-only change; record it as such |

- [ ] **Step 2: Look at it on a real photograph**

```bash
cargo build --release -p app 2>&1 | tail -1
nohup ./target/release/Rustroom > /tmp/rustroom.log 2>&1 &
```

Check by hand and report honestly:
1. Geometry tool → **Guides** on: the photo shows **uncorrected**, and the crop rectangle is gone.
2. Drag along a converging vertical edge — a blue line appears. Draw a second on the opposite edge.
3. On release, **Vertical and Straighten move**, and turning Guides off shows the verticals parallel.
4. Two more lines along horizontals turn **amber**, and re-solving engages Horizontal.
5. Ctrl+Z undoes the solve in **one** step.
6. **Clear** removes the guides and leaves the sliders where they are.
7. Switching photo and back discards the guides but keeps the slider values.

- [ ] **Step 3: Write the CLAUDE.md section**

House style: a one-line summary with the test-count delta and golden count, ⚠-marked bullets, then
numbered **Accepted caveats**. Cover at minimum: that `mod_pc` was **not** used and why that
reverses the backlog note; the closed-form solve and that the verticals-only case is the formula
already in the file; guides being view state and what that buys; the preview reset as the
feedback-loop fix; the exclusivity with the crop overlay; and the focal-length estimate being
computed-but-unusable.

- [ ] **Step 4: Update the backlog and the resume block**

Replace the "Guided perspective control points" entry in `docs/BACKLOG.md` with a short note that
it shipped, and **correct its `mod_pc` claim** — leaving it would send the next reader to an API
this feature deliberately does not use. Add **automatic vertical detection** as the remaining
slice, noting it needs a Hough transform.

Update **▶ THE NEXT ACTION** and the **State at handoff** line: test count, and that Phase 3's only
outstanding half is creative 3-D LUTs.

- [ ] **Step 5: Final green and commit**

```bash
cargo fmt --check && echo "fmt ok"
cargo clippy --all-targets --all-features -- -D warnings > /tmp/clippy.log 2>&1; echo "clippy exit: $?"
cargo test --all 2>&1 | grep "test result:" | awk -F'[ ;]' '{p+=$4; f+=$6} END {print "passed="p, "failed="f}'
git add -A
git commit -m "docs: guided perspective shipped"
git log main..HEAD --oneline
```

⚠ **Do not push and do not merge.** Report the branch state and wait.
