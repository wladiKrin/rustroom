# Perspective Correction Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Four sliders — Vertical, Horizontal, Aspect, Scale — that build one perspective homography and apply it as its own pipeline stage between lens correction and geometry.

**Architecture:** A pure `Homography` value object in `pipeline::perspective` (build from angles, invert, map a point, compute autoscale) with **no image handling at all**, wrapped by a thin `PerspectiveStage` that inverse-maps and bilinearly samples. The stage lives in the prefix, so it always runs whole-buffer: no `Region`, no kernel margin, no `RenderScale`. `FrameMap` is untouched because the stage sits upstream of the brush's anchor space.

**Tech Stack:** Rust, existing crates only. **No new dependency** — `lensfun`'s `mod_pc` is a control-point API and is not used by this slice.

**Spec:** `docs/superpowers/specs/2026-08-31-perspective-correction-design.md`

**Build environment — every `cargo` command in this plan needs this first:**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

⚠ **Check clippy's exit code before every commit.** `cargo test` passing is not evidence about clippy; six commits this month went in with `clippy -D warnings` failing and had to be amended. Always:

```bash
cargo clippy --all-targets --all-features > /tmp/clippy.log 2>&1; echo "clippy exit: $?"
```

⚠ **Branch:** work happens on `perspective-correction`, which already exists. **Never push.**

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/edit-model/src/lib.rs` | `Perspective` struct, `Edits.perspective`, group in `merge_from` |
| `crates/pipeline/src/perspective.rs` | **Create.** `Homography` — pure maths, no `ImageBuf` |
| `crates/pipeline/src/stages/perspective.rs` | **Create.** `PerspectiveStage` — inverse-map and sample |
| `crates/pipeline/src/stages/geometry.rs` | `sample_bilinear` becomes `pub(crate)` so the new stage reuses it |
| `crates/pipeline/src/lib.rs` | `PrefixStepKey::Perspective`, `build_prefix_steps` |
| `crates/pipeline/tests/golden.rs` | `grid_chart()` and two goldens |
| `crates/app/src/develop/panel/geometry.rs` | The four sliders, and the reset button's text |
| `crates/app/src/tool.rs` | Rail label `Crop` → `Geometry` |

---

### Task 1: The `Perspective` edit model

**Files:**
- Modify: `crates/edit-model/src/lib.rs`
- Modify: `crates/pipeline/src/lib.rs` (the `all_edits` fixture)
- Modify: `crates/app/src/edit_clipboard.rs` (the payload fixture)

- [ ] **Step 1: Add the struct**

Put it immediately after `LensCorrection`'s `Default` impl in `crates/edit-model/src/lib.rs`.

```rust
/// Manual perspective correction — keystone about both axes, plus the two controls that make it
/// usable.
///
/// ⚠ Its own field on [`Edits`], **not** a member of [`Geometry`]. That is what buys the separate
/// cache checkpoint: sharing `PrefixStepKey::Geometry` would make every crop drag and every
/// Straighten frame re-run the perspective resample.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Perspective {
    /// Keystone about the horizontal axis (camera pitch), ~[-100, 100].
    pub vertical: f32,
    /// Keystone about the vertical axis (camera yaw), ~[-100, 100].
    pub horizontal: f32,
    /// Counteracts the stretch a keystone correction causes, ~[-100, 100].
    pub aspect: f32,
    /// How far to scale the corrected image, 0..100.
    ///
    /// ⚠ 0 leaves the empty corners the correction opens; 100 scales until they are gone. A
    /// keystone empties far more of the frame than pincushion distortion does, which is why this
    /// defaults to 100 where [`LensCorrection::scale`] could reasonably have defaulted to 0.
    pub scale: f32,
}

impl Default for Perspective {
    /// ⚠ Hand-written, **not** derived: `scale` is 100 at neutral, and the container-level
    /// `#[serde(default)]` fills a missing field from this. A derived `Default` would give every
    /// partial sidecar `scale: 0`, silently — the trap `LensCorrection::default`,
    /// `RadialGradient::default` and `ColorGrading::default` each carry.
    fn default() -> Self {
        Self {
            vertical: 0.0,
            horizontal: 0.0,
            aspect: 0.0,
            scale: 100.0,
        }
    }
}

impl Perspective {
    /// Whether this warrants a pipeline stage.
    ///
    /// ⚠ The three *corrections* only — `scale` is a modifier, and dragging it alone must not
    /// insert a stage that would resample the whole image for nothing. The rule
    /// `NoiseReduction`, `Vignette` and `Sharpening` already follow.
    pub fn is_active(&self) -> bool {
        self.vertical != 0.0 || self.horizontal != 0.0 || self.aspect != 0.0
    }
}
```

- [ ] **Step 2: Add the field to `Edits`**

In `pub struct Edits`, immediately **after** `pub lens_correction: LensCorrection,` and **before** `pub geometry: Geometry,` — matching pipeline order, which is how every other field in that struct is ordered:

```rust
    pub perspective: Perspective,
```

- [ ] **Step 3: Run the build and watch it fail in four places**

```bash
cargo build --all 2>&1 | grep -E "^error" -A 4 | head -30
```

Expected: `E0027` (pattern does not mention field `perspective`) in `merge_from`, and `E0063` (missing field) in `edit_model`'s `all_nonneutral`, `pipeline`'s `all_edits`, and `edit_clipboard`'s payload test. **That is those constructs doing their job** — each forces a decision rather than letting a new field silently never transfer.

- [ ] **Step 4: Group it**

In `merge_from`'s destructure, add `perspective,` after `lens_correction,`. Then extend the geometry arm:

```rust
        if groups.geometry {
            self.geometry = *geometry;
            // ⚠ Perspective is compositional, exactly as the crop is: a keystone framed for one
            // photograph is meaningless on another. So it rides the Geometry group, which
            // defaults **off** for presets, paste and sync.
            self.perspective = *perspective;
        }
```

- [ ] **Step 5: Fill the three fixtures**

`crates/edit-model/src/lib.rs`, in `all_nonneutral()`, after the `lens_correction` block:

```rust
            perspective: Perspective {
                vertical: 40.0,
                horizontal: -20.0,
                aspect: 15.0,
                scale: 80.0,
            },
```

`crates/pipeline/src/lib.rs`, in `all_edits()`, after the `lens_correction` block:

```rust
            perspective: edit_model::Perspective {
                vertical: 30.0,
                horizontal: -15.0,
                ..Default::default()
            },
```

`crates/app/src/edit_clipboard.rs`, in the payload fixture, immediately before its `geometry:` line:

```rust
            perspective: edit_model::Perspective {
                vertical: 25.0,
                ..Default::default()
            },
```

- [ ] **Step 6: Write the model tests**

Append inside `mod tests` in `crates/edit-model/src/lib.rs`:

```rust
    /// ⚠ The value, not the mechanism. A derived `Default` would give `scale: 0`, and every
    /// sidecar written without the field would silently come back with the correction scaled
    /// away. Sabotage: `#[derive(Default)]` on `Perspective`.
    #[test]
    fn a_neutral_perspective_scales_fully() {
        assert_eq!(Perspective::default().scale, 100.0);
        assert!(!Perspective::default().is_active());
    }

    /// ⚠ `scale` is a modifier, so dragging it alone must not insert a stage.
    #[test]
    fn scale_alone_does_not_activate_the_perspective_stage() {
        let p = Perspective {
            scale: 0.0,
            ..Default::default()
        };
        assert!(!p.is_active(), "scale is a modifier, not a correction");
        for probe in [
            Perspective {
                vertical: 1.0,
                ..Default::default()
            },
            Perspective {
                horizontal: 1.0,
                ..Default::default()
            },
            Perspective {
                aspect: 1.0,
                ..Default::default()
            },
        ] {
            assert!(probe.is_active(), "{probe:?} must activate");
        }
    }

    /// ⚠ A sidecar written before this field existed must load with the *neutral* perspective,
    /// not a zeroed one. `#[serde(default)]` at the container level is what does it.
    #[test]
    fn a_sidecar_from_before_perspective_still_loads() {
        let json = r#"{"version":1,"exposure":0.5}"#;
        let e: Edits = serde_json::from_str(json).unwrap();
        assert_eq!(e.perspective, Perspective::default());
        assert_eq!(e.exposure, 0.5);
    }

    /// ⚠ Both directions. That it *travels* with Geometry proves the arm exists; that it does
    /// **not** travel with any other group proves it was not put in two.
    #[test]
    fn perspective_travels_on_the_geometry_group_and_no_other() {
        let src = all_nonneutral();
        let mut with = Edits::default();
        with.merge_from(&src, EditGroups::only(GEOMETRY_GROUP));
        assert_eq!(with.perspective, src.perspective);

        for g in 0..EDIT_GROUP_NAMES.len() {
            if g == GEOMETRY_GROUP {
                continue;
            }
            let mut without = Edits::default();
            without.merge_from(&src, EditGroups::only(g));
            assert_eq!(
                without.perspective,
                Perspective::default(),
                "group {} ({}) must not carry perspective",
                g,
                EDIT_GROUP_NAMES[g]
            );
        }
    }
```

- [ ] **Step 7: Green, clippy, commit**

```bash
cargo fmt
cargo clippy --all-targets --all-features > /tmp/clippy.log 2>&1; echo "clippy exit: $?"
cargo test --all 2>&1 | grep "test result:" | awk -F'[ ;]' '{p+=$4; f+=$6} END {print "passed="p, "failed="f}'
```

Expected: clippy exit 0, failed=0. Then:

```bash
git add -A
git commit -m "feat(edit-model): a Perspective edit, on the Geometry group"
```

---

### Task 2: The homography

**Files:**
- Create: `crates/pipeline/src/perspective.rs`
- Modify: `crates/pipeline/src/lib.rs` (add `pub mod perspective;`)

⚠ **This task adds no image handling whatsoever.** Everything here is testable with points and floats, which is the whole reason it is its own file: the M3.1c-A precedent, where the pure `FrameMap` carried ~30 tests and produced no defect while the canvas around it produced six.

- [ ] **Step 1: Write the module**

Create `crates/pipeline/src/perspective.rs`:

```rust
//! The perspective-correction homography — pure maths, no images.
//!
//! ⚠ Deliberately free of `ImageBuf`: every property here is testable with points and floats,
//! which is where this project's defect record says the tests actually pay.
//!
//! Coordinates are normalised about the frame centre with **half-height = 1**, so `y` runs
//! ±1 and `x` runs ±aspect. A point `(x, y)` is the camera ray `(x, y, f)`.

use edit_model::Perspective;

/// The assumed focal length, in units of the frame's half-height. 2.0 is about a 24 mm lens on
/// full frame.
///
/// ⚠⚠ **Fixed, not read from EXIF, and that is a decision rather than a shortcut.** For *any*
/// assumed `f` there is a `θ` that sends the vertical vanishing point to infinity —
/// `tan θ = −f / y_v` — so the verticals come out exactly parallel whichever value is chosen.
/// What differs is how much the rest of the frame stretches, and that is precisely what the
/// Aspect slider corrects.
///
/// ⚠ Reading `Capture.focal_len` instead would make the tool behave one way on a RAW and another
/// on a JPEG carrying no EXIF — two behaviours that differ invisibly, the failure
/// `render_max_dim` and `Session::geometry_mode` were each fixed for.
///
/// ⚠ The consequence, stated so it is not later filed as a bug: the same Vertical value is **not**
/// the same physical camera pitch on a 16 mm shot and a 200 mm one. It means "this much
/// keystone", judged by eye — which is how Lightroom's manual sliders behave too.
pub const F_NORM: f32 = 2.0;

/// What ±100 on Vertical or Horizontal means, in degrees. A tuning constant; the `hue` bands'
/// ±100 → ±30° precedent.
pub const MAX_ANGLE_DEG: f32 = 30.0;

/// What ±100 on Aspect means, as a stretch factor. `x` is multiplied by `s` and `y` by `1/s`, so
/// the transform is area-preserving to first order.
pub const MAX_ASPECT: f32 = 1.4;

/// Below this an autoscale probe is treated as being on the axis and skipped; it only guards a
/// division.
const PROBE_EPS: f32 = 1e-3;

/// A 3×3 projective transform on normalised frame coordinates, together with its inverse.
///
/// The forward direction maps **source → output**. Rendering inverse-maps, so [`Self::source_of`]
/// is what the stage calls.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Homography {
    fwd: [[f32; 3]; 3],
    inv: [[f32; 3]; 3],
}

fn mul(a: &[[f32; 3]; 3], b: &[[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let mut m = [[0.0f32; 3]; 3];
    for (i, row) in m.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            *cell = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    m
}

/// The adjugate over the determinant. A projective matrix is only defined up to scale, so the
/// determinant division is cosmetic — but keeping it makes `fwd · inv == I` testable exactly.
fn invert(m: &[[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let c = |r: usize, col: usize| -> f32 {
        let rs: Vec<usize> = (0..3).filter(|&i| i != r).collect();
        let cs: Vec<usize> = (0..3).filter(|&i| i != col).collect();
        let d = m[rs[0]][cs[0]] * m[rs[1]][cs[1]] - m[rs[0]][cs[1]] * m[rs[1]][cs[0]];
        if (r + col) % 2 == 0 {
            d
        } else {
            -d
        }
    };
    let det = m[0][0] * c(0, 0) + m[0][1] * c(0, 1) + m[0][2] * c(0, 2);
    // A degenerate matrix cannot arise from the bounded angles this builds, but dividing by zero
    // would give a frame of NaN rather than an error, so fall back to the identity.
    if det.abs() < f32::EPSILON {
        return [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    }
    let mut out = [[0.0f32; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, cell) in row.iter_mut().enumerate() {
            // Transposed: the inverse is the adjugate, i.e. the transpose of the cofactors.
            *cell = c(j, i) / det;
        }
    }
    out
}

fn apply(m: &[[f32; 3]; 3], p: (f32, f32)) -> (f32, f32) {
    let x = m[0][0] * p.0 + m[0][1] * p.1 + m[0][2];
    let y = m[1][0] * p.0 + m[1][1] * p.1 + m[1][2];
    let w = m[2][0] * p.0 + m[2][1] * p.1 + m[2][2];
    if w.abs() < f32::EPSILON {
        // The point is on the horizon and has no finite image. Push it far away rather than
        // returning NaN, so the sampler simply reads outside the frame and gets black.
        return (x * 1e6, y * 1e6);
    }
    (x / w, y / w)
}

impl Homography {
    /// Build from angles directly, in radians.
    ///
    /// ⚠ Separate from [`Self::from_edits`] on purpose: it lets the geometric tests state their
    /// own angles and stay independent of `MAX_ANGLE_DEG`, so re-tuning a slider range cannot
    /// break a test about the maths.
    pub fn from_angles(pitch: f32, yaw: f32, aspect_s: f32, scale_k: f32) -> Self {
        let f = F_NORM;
        // (x, y, 1) → the ray (x, y, f).
        let k = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, f]];
        // (X, Y, Z) → (fX, fY, Z), which after the homogeneous divide is (fX/Z, fY/Z).
        let k_out = [[f, 0.0, 0.0], [0.0, f, 0.0], [0.0, 0.0, 1.0]];
        let (sp, cp) = pitch.sin_cos();
        let r_pitch = [[1.0, 0.0, 0.0], [0.0, cp, -sp], [0.0, sp, cp]];
        let (sy, cy) = yaw.sin_cos();
        let r_yaw = [[cy, 0.0, sy], [0.0, 1.0, 0.0], [-sy, 0.0, cy]];
        let stretch = [
            [aspect_s, 0.0, 0.0],
            [0.0, 1.0 / aspect_s, 0.0],
            [0.0, 0.0, 1.0],
        ];
        let zoom = [[scale_k, 0.0, 0.0], [0.0, scale_k, 0.0], [0.0, 0.0, 1.0]];

        let r = mul(&r_pitch, &r_yaw);
        let fwd = mul(&zoom, &mul(&stretch, &mul(&k_out, &mul(&r, &k))));
        Self {
            inv: invert(&fwd),
            fwd,
        }
    }

    /// Build from the sliders, **without** the autoscale — `scale_k` is 1.0.
    ///
    /// ⚠ Two steps rather than one because the autoscale is computed *from* the un-zoomed
    /// transform: see [`Self::autoscale`].
    pub fn unscaled(p: &Perspective) -> Self {
        let to_rad = |v: f32| (v / 100.0) * MAX_ANGLE_DEG.to_radians();
        Self::from_angles(
            to_rad(p.vertical),
            to_rad(p.horizontal),
            MAX_ASPECT.powf(p.aspect / 100.0),
            1.0,
        )
    }

    /// The full transform for these sliders at this frame aspect, autoscale included.
    pub fn from_edits(p: &Perspective, aspect: f32) -> Self {
        let base = Self::unscaled(p);
        let full = base.autoscale(aspect);
        // 0..100 interpolates 1.0 → autoscale, exactly as `LensCorrection::scale` does.
        let k = 1.0 + (full - 1.0) * (p.scale.clamp(0.0, 100.0) / 100.0);
        let to_rad = |v: f32| (v / 100.0) * MAX_ANGLE_DEG.to_radians();
        Self::from_angles(
            to_rad(p.vertical),
            to_rad(p.horizontal),
            MAX_ASPECT.powf(p.aspect / 100.0),
            k,
        )
    }

    /// Where in the source an output point comes from. Normalised coordinates both sides.
    pub fn source_of(&self, p: (f32, f32)) -> (f32, f32) {
        apply(&self.inv, p)
    }

    /// Where an output point lands. Used by the tests and by the autoscale.
    pub fn output_of(&self, p: (f32, f32)) -> (f32, f32) {
        apply(&self.fwd, p)
    }

    /// The scale that just fills the frame with real image data.
    ///
    /// ⚠ Mirrors `Corrector::compute_autoscale` rather than inventing a second approach: probe
    /// the four corners and four edge midpoints, ask how far outside the frame each one's source
    /// lands, and take the largest ratio.
    pub fn autoscale(&self, aspect: f32) -> f32 {
        let (ax, ay) = (aspect, 1.0);
        let probes = [
            (-ax, -ay),
            (ax, -ay),
            (-ax, ay),
            (ax, ay),
            (0.0, -ay),
            (0.0, ay),
            (-ax, 0.0),
            (ax, 0.0),
        ];
        let mut scale = 1.0f32;
        for p in probes {
            let s = self.source_of(p);
            if p.0.abs() > PROBE_EPS {
                scale = scale.max((s.0 / p.0).abs());
            }
            if p.1.abs() > PROBE_EPS {
                scale = scale.max((s.1 / p.1).abs());
            }
        }
        // A wild angle can drive this arbitrarily high; past 8× there is nothing usable left.
        scale.clamp(1.0, 8.0)
    }
}
```

Then add to `crates/pipeline/src/lib.rs`, beside the other `pub mod` lines:

```rust
pub mod perspective;
```

- [ ] **Step 2: Write the tests**

Append to `crates/pipeline/src/perspective.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const ASPECT: f32 = 1.5;

    /// ⚠ Probes with **leverage**. A projective error displaces a point in proportion to its
    /// distance from the axis, and M3.1c-A recorded a rotation sign error that survived because
    /// its probe sat at 0.34 of the half-width. These sit at the corners.
    #[test]
    fn the_homography_round_trips_at_the_corners() {
        let h = Homography::from_angles(0.3, -0.2, 1.2, 1.1);
        for p in [(-1.5, -1.0), (1.5, -1.0), (-1.5, 1.0), (1.5, 1.0)] {
            let back = h.source_of(h.output_of(p));
            assert!(
                (back.0 - p.0).abs() < 1e-4 && (back.1 - p.1).abs() < 1e-4,
                "{p:?} round-tripped to {back:?}"
            );
        }
    }

    /// ⚠⚠ **The test that matters, and it must not be a golden.** A golden only ever catches a
    /// *change*, and would bake a wrong convention into its reference on the first run — the
    /// lesson `the_tint_lands_at_the_hue_the_wheel_names` was added for.
    ///
    /// Two world-vertical lines converge to a vanishing point at `(0, y_v)`. The pitch that
    /// straightens them satisfies `tan θ = −f / y_v`, derived independently here rather than
    /// read back out of the implementation. After correction the two lines must be parallel.
    #[test]
    fn the_predicted_pitch_makes_converging_verticals_parallel() {
        let y_v = 4.0f32;
        let theta = (-F_NORM / y_v).atan();
        let h = Homography::from_angles(theta, 0.0, 1.0, 1.0);

        // Two lines, each from a base point toward the vanishing point.
        let vp = (0.0f32, y_v);
        let line = |base: (f32, f32), t: f32| {
            (
                base.0 + (vp.0 - base.0) * t,
                base.1 + (vp.1 - base.1) * t,
            )
        };
        let dir = |base: (f32, f32)| {
            let a = h.output_of(line(base, 0.0));
            let b = h.output_of(line(base, 0.4));
            (b.0 - a.0, b.1 - a.1)
        };
        let (ax, ay) = dir((-1.0, -1.0));
        let (bx, by) = dir((1.0, -1.0));
        // Parallel ⟺ the 2-D cross product vanishes. Normalise so the tolerance means an angle.
        let na = (ax * ax + ay * ay).sqrt();
        let nb = (bx * bx + by * by).sqrt();
        let cross = (ax / na) * (by / nb) - (ay / na) * (bx / nb);
        assert!(
            cross.abs() < 1e-3,
            "corrected verticals are not parallel: cross = {cross}"
        );
    }

    /// ⚠ The complement: without the correction those same lines genuinely do converge, or the
    /// test above is measuring nothing.
    #[test]
    fn the_uncorrected_lines_really_do_converge() {
        let h = Homography::from_angles(0.0, 0.0, 1.0, 1.0);
        let vp = (0.0f32, 4.0);
        let dir = |base: (f32, f32)| {
            let a = h.output_of(base);
            let b = h.output_of((
                base.0 + (vp.0 - base.0) * 0.4,
                base.1 + (vp.1 - base.1) * 0.4,
            ));
            (b.0 - a.0, b.1 - a.1)
        };
        let (ax, ay) = dir((-1.0, -1.0));
        let (bx, by) = dir((1.0, -1.0));
        let na = (ax * ax + ay * ay).sqrt();
        let nb = (bx * bx + by * by).sqrt();
        let cross = (ax / na) * (by / nb) - (ay / na) * (bx / nb);
        assert!(cross.abs() > 0.1, "fixture check: the lines must converge");
    }

    /// ⚠ Sign conventions, one assertion per axis. A test that only checks "something moved"
    /// cannot tell a sign error from a correct implementation.
    ///
    /// A positive pitch must move the **top** of the frame outward relative to the bottom — that
    /// is what un-leans a building photographed from below.
    #[test]
    fn a_positive_pitch_widens_the_top_relative_to_the_bottom() {
        let h = Homography::from_angles(0.3, 0.0, 1.0, 1.0);
        let top = h.output_of((1.0, -1.0)).0;
        let bottom = h.output_of((1.0, 1.0)).0;
        assert!(
            top > bottom,
            "top half-width {top} should exceed bottom {bottom}"
        );
    }

    /// The yaw equivalent, about the other axis.
    #[test]
    fn a_positive_yaw_heightens_the_left_relative_to_the_right() {
        let h = Homography::from_angles(0.0, 0.3, 1.0, 1.0);
        let left = h.output_of((-1.0, 1.0)).1;
        let right = h.output_of((1.0, 1.0)).1;
        assert!(
            left > right,
            "left half-height {left} should exceed right {right}"
        );
    }

    /// ⚠ `diag(s, 1/s)` claims to be area-preserving; assert it rather than trusting the shape.
    #[test]
    fn aspect_stretches_one_axis_and_shrinks_the_other() {
        let h = Homography::from_angles(0.0, 0.0, 1.25, 1.0);
        let p = h.output_of((1.0, 1.0));
        assert!((p.0 - 1.25).abs() < 1e-5, "x stretched to {}", p.0);
        assert!((p.1 - 0.8).abs() < 1e-5, "y shrank to {}", p.1);
    }

    /// ⚠⚠ **Both halves.** That the autoscale bounds the frame is only meaningful alongside the
    /// fact that without it the frame really is empty at the corners.
    #[test]
    fn the_autoscale_pulls_every_probe_back_inside() {
        let p = Perspective {
            vertical: 60.0,
            ..Default::default()
        };
        let bare = Homography::unscaled(&p);
        let k = bare.autoscale(ASPECT);
        assert!(k > 1.0, "a strong keystone must need scaling, got {k}");

        let outside = |h: &Homography| {
            [
                (-ASPECT, -1.0),
                (ASPECT, -1.0),
                (-ASPECT, 1.0),
                (ASPECT, 1.0),
            ]
            .iter()
            .any(|&q| {
                let s = h.source_of(q);
                s.0.abs() > ASPECT + 1e-3 || s.1.abs() > 1.0 + 1e-3
            })
        };
        assert!(outside(&bare), "fixture check: uncorrected corners are empty");
        assert!(
            !outside(&Homography::from_edits(&p, ASPECT)),
            "at scale 100 no corner may read outside the source"
        );
    }

    /// ⚠ Scale 0 must leave the gaps, or the slider's low end does nothing.
    #[test]
    fn scale_zero_leaves_the_corners_empty() {
        let p = Perspective {
            vertical: 60.0,
            scale: 0.0,
            ..Default::default()
        };
        let h = Homography::from_edits(&p, ASPECT);
        let s = h.source_of((ASPECT, -1.0));
        assert!(
            s.0.abs() > ASPECT + 1e-3 || s.1.abs() > 1.0 + 1e-3,
            "at scale 0 a corner should still read outside, got {s:?}"
        );
    }

    /// A neutral `Perspective` must be the identity exactly, not merely nearly — that is what
    /// lets the stage be skipped and the 47 goldens stay byte-identical.
    #[test]
    fn a_neutral_perspective_is_the_identity() {
        let h = Homography::from_edits(&Perspective::default(), ASPECT);
        for p in [(0.0, 0.0), (ASPECT, 1.0), (-ASPECT, -1.0), (0.7, -0.3)] {
            let q = h.source_of(p);
            assert!(
                (q.0 - p.0).abs() < 1e-5 && (q.1 - p.1).abs() < 1e-5,
                "{p:?} moved to {q:?}"
            );
        }
    }
}
```

- [ ] **Step 3: Run them**

```bash
cargo test -p pipeline perspective 2>&1 | grep -E "test result:|^error" | head -3
```

Expected: `test result: ok. 9 passed`.

⚠ If `a_positive_pitch_widens_the_top_relative_to_the_bottom` fails, **the sign convention is the thing to change, not the assertion** — negate `pitch` in `from_angles`. The test states what the slider must do; the maths must meet it.

- [ ] **Step 4: Clippy and commit**

```bash
cargo fmt
cargo clippy --all-targets --all-features > /tmp/clippy.log 2>&1; echo "clippy exit: $?"
git add -A
git commit -m "feat(pipeline): the perspective homography, with no image handling"
```

---

### Task 3: `PerspectiveStage`

**Files:**
- Create: `crates/pipeline/src/stages/perspective.rs`
- Modify: `crates/pipeline/src/stages/geometry.rs` (one visibility change)
- Modify: `crates/pipeline/src/lib.rs`

- [ ] **Step 1: Share the sampler**

In `crates/pipeline/src/stages/geometry.rs`, change

```rust
fn sample_bilinear(src: &ImageBuf, sx: f32, sy: f32) -> [f32; 3] {
```

to

```rust
pub(crate) fn sample_bilinear(src: &ImageBuf, sx: f32, sy: f32) -> [f32; 3] {
```

⚠ Reused rather than copied. `lens_correction.rs` already has its own `sample_channel` because TCA needs a *per-channel* source point; this stage does not, so a third copy would be duplication with no reason behind it.

- [ ] **Step 2: Write the stage**

Create `crates/pipeline/src/stages/perspective.rs`:

```rust
//! Manual perspective correction — the keystone transform.
//!
//! ⚠ Runs in the **prefix**, between `LensCorrectionStage` and `GeometryStage`. After lens
//! correction because a calibration is defined in the uncropped sensor frame and describes the
//! optics rather than the pose; before geometry because you correct the keystone, level what is
//! left, then crop away the corners the correction emptied. Running it after the crop would move
//! content inside a rectangle already chosen, leaving gaps *within* the crop.
//!
//! ⚠ Being in the prefix, it always runs whole-buffer — so no `Region` and no kernel margin,
//! despite being a spatial resample. And being normalised, it takes **no `RenderScale`**: the
//! preview and the export agree exactly, the creative vignette's property.
//!
//! ⚠⚠ It sits **upstream of the brush's anchor space**, which is `GeometryStage`'s input. That is
//! what means `FrameMap` needs no change at all. The cost is the caveat lens correction already
//! documents: toggling perspective after painting shifts strokes.

use crate::perspective::Homography;
use crate::stages::geometry::sample_bilinear;
use crate::PipelineStage;
use core_image::ImageBuf;
use edit_model::Perspective;

pub struct PerspectiveStage {
    params: Perspective,
}

impl PerspectiveStage {
    pub fn new(params: Perspective) -> Self {
        Self { params }
    }
}

impl PipelineStage for PerspectiveStage {
    fn name(&self) -> &'static str {
        "perspective"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        let (w, h) = (image.width(), image.height());
        if w < 2 || h < 2 {
            return image;
        }
        let cx = (w as f32 - 1.0) / 2.0;
        let cy = (h as f32 - 1.0) / 2.0;
        // Half-height is the unit, so `x` runs ±aspect and `y` runs ±1.
        let aspect = cx / cy;
        let hom = Homography::from_edits(&self.params, aspect);

        let mut out = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let n = ((x as f32 - cx) / cy, (y as f32 - cy) / cy);
                let s = hom.source_of(n);
                out.set_pixel(x, y, sample_bilinear(&image, cx + s.0 * cy, cy + s.1 * cy));
            }
        }
        out
    }
}
```

⚠ Sequential, mirroring `rotate_bilinear` in the sibling stage rather than reaching for rayon. Geometry's whole-buffer rotation measures 203 ms at full resolution and nobody has complained; matching it keeps the two readable side by side. If it ever needs to be faster, both should move together.

- [ ] **Step 3: Register the stage and the key**

In `crates/pipeline/src/lib.rs`, add to the `stages` module declaration list (beside `pub mod geometry;` etc.):

```rust
pub mod perspective;
```

⚠ That goes in the **`stages`** module, not the crate root — the crate root already has `pub mod perspective;` for the maths from Task 2, and the two are different modules with the same leaf name. Import the stage as `stages::perspective::PerspectiveStage`.

Add the key variant:

```rust
pub enum PrefixStepKey {
    LensCorrection(edit_model::LensCorrection),
    Perspective(edit_model::Perspective),
    Geometry(edit_model::Geometry),
    NoiseReduction(edit_model::NoiseReduction),
    Dehaze(edit_model::Dehaze),
}
```

And in `build_prefix_steps`, **between** the lens-correction block and the geometry block:

```rust
    // ⚠ Between lens correction and geometry: after the optics, before the crop. See the stage's
    // own module docs for why that order is forced rather than chosen.
    if edits.perspective.is_active() {
        steps.push(PrefixStep {
            key: PrefixStepKey::Perspective(edits.perspective),
            stage: Box::new(stages::perspective::PerspectiveStage::new(
                edits.perspective,
            )),
        });
    }
```

- [ ] **Step 4: Extend the prefix-composition tripwire**

Find the test named `the_prefix_holds_geometry_noise_reduction_and_dehaze` in `crates/pipeline/src/lib.rs` (it may have been renamed to mention lens correction). Add `"perspective"` to its expected name list, in position after `lens_correction` and before `geometry`.

⚠ That test pins *composition and order*, which the compiler cannot. Keep it.

- [ ] **Step 5: Write the stage tests**

Append to `crates/pipeline/src/stages/perspective.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::PipelineStage;

    fn ramp(w: usize, h: usize) -> ImageBuf {
        let mut img = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let v = x as f32 / (w - 1) as f32;
                img.set_pixel(x, y, [v, 1.0 - v, 0.5]);
            }
        }
        img
    }

    /// ⚠ Bit-identical, not merely close. A neutral stage that *nearly* reproduced its input
    /// would move all 47 goldens and the failure would read as a maths bug somewhere else.
    #[test]
    fn a_neutral_perspective_reproduces_the_image_exactly() {
        let src = ramp(37, 23);
        let out = PerspectiveStage::new(Perspective::default()).apply(src.clone());
        assert_eq!(out, src);
    }

    /// ⚠ That the stage actually reaches the pixels. The obvious sabotage — returning the input
    /// untouched — leaves everything else in this file green.
    #[test]
    fn a_keystone_really_moves_the_pixels() {
        let src = ramp(64, 48);
        let out = PerspectiveStage::new(Perspective {
            vertical: 50.0,
            ..Default::default()
        })
        .apply(src.clone());
        assert_ne!(out, src);
        assert_eq!((out.width(), out.height()), (src.width(), src.height()));
    }

    /// The canvas size is preserved, which is what lets `FrameMap` stay untouched: `GeometryStage`
    /// receives exactly the dimensions it would have without this stage.
    #[test]
    fn the_canvas_size_is_unchanged_by_a_strong_correction() {
        for (w, h) in [(64, 48), (48, 64), (2, 2)] {
            let out = PerspectiveStage::new(Perspective {
                vertical: 100.0,
                horizontal: -100.0,
                aspect: 80.0,
                scale: 0.0,
            })
            .apply(ramp(w, h));
            assert_eq!((out.width(), out.height()), (w, h));
        }
    }

    /// A degenerate image must pass through rather than divide by zero.
    #[test]
    fn a_one_pixel_image_passes_through() {
        let src = ImageBuf::new(1, 1);
        let out = PerspectiveStage::new(Perspective {
            vertical: 50.0,
            ..Default::default()
        })
        .apply(src.clone());
        assert_eq!(out, src);
    }
}
```

- [ ] **Step 6: The goldens must not have moved**

```bash
cargo test -p pipeline 2>&1 | grep -E "test result:|^error" | head -3
cargo test --all 2>&1 | grep "test result:" | awk -F'[ ;]' '{p+=$4; f+=$6} END {print "passed="p, "failed="f}'
```

Expected: failed=0, **all 47 goldens still passing without regeneration**. ⚠ If any golden moved, `is_active` or the neutral identity is wrong — **do not regenerate**, find it.

- [ ] **Step 7: Clippy and commit**

```bash
cargo fmt
cargo clippy --all-targets --all-features > /tmp/clippy.log 2>&1; echo "clippy exit: $?"
git add -A
git commit -m "feat(pipeline): PerspectiveStage, between lens correction and geometry"
```

---

### Task 4: Goldens

**Files:**
- Modify: `crates/pipeline/tests/golden.rs`
- Create: two PNGs under the goldens directory (generated)

- [ ] **Step 1: Add a chart with straight-line structure**

The existing `chart()` is 64×16 of horizontal ramps — a perspective transform on it is nearly unreadable. Append to `crates/pipeline/tests/golden.rs`, after `chart()`:

```rust
/// A grid, for the geometric stages. ⚠ Straight lines are the only structure a perspective
/// transform can be *read* from; the ramp chart shows a keystone as a vague smear.
///
/// ⚠ Deliberately dim (0.55 on 0.12): a golden that clips cannot discriminate the transform from
/// any stronger one, which is the weakness measured and fixed in the colour-grading goldens.
fn grid_chart() -> ImageBuf {
    const GW: usize = 64;
    const GH: usize = 48;
    let mut img = ImageBuf::new(GW, GH);
    for y in 0..GH {
        for x in 0..GW {
            let line = x % 8 == 0 || y % 8 == 0;
            let v = if line { 0.55 } else { 0.12 };
            set_srgb(&mut img, x, y, [v, v, v]);
        }
    }
    img
}
```

- [ ] **Step 2: Add the two goldens**

```rust
#[test]
fn golden_perspective_vertical() {
    check_golden_with(
        "perspective_vertical",
        &Edits {
            perspective: edit_model::Perspective {
                vertical: 55.0,
                ..Default::default()
            },
            ..Default::default()
        },
        grid_chart(),
    );
}

#[test]
fn golden_perspective_aspect() {
    check_golden_with(
        "perspective_aspect",
        &Edits {
            perspective: edit_model::Perspective {
                horizontal: -40.0,
                aspect: 60.0,
                ..Default::default()
            },
            ..Default::default()
        },
        grid_chart(),
    );
}
```

- [ ] **Step 3: Generate them, then MEASURE rather than eyeball**

```bash
UPDATE_GOLDEN=1 cargo test -p pipeline --test golden golden_perspective 2>&1 | grep "test result:"
```

Then check both properties the spec requires — no clipping, and the two differ:

```bash
cd crates/pipeline/tests && python3 - <<'PY'
import struct, zlib, glob, sys
def read_png(p):
    d = open(p,'rb').read(); i = 8; w=h=None; idat=b''
    while i < len(d):
        ln = struct.unpack('>I', d[i:i+4])[0]; typ = d[i+4:i+8]
        if typ == b'IHDR': w,h = struct.unpack('>II', d[i+8:i+16])
        if typ == b'IDAT': idat += d[i+8:i+8+ln]
        i += 12 + ln
    raw = zlib.decompress(idat); out=[]; stride = w*3; prev = bytearray(stride)
    pos = 0
    for _ in range(h):
        f = raw[pos]; pos += 1; line = bytearray(raw[pos:pos+stride]); pos += stride
        for x in range(stride):
            a = line[x-3] if x >= 3 else 0; b = prev[x]
            c = prev[x-3] if x >= 3 else 0
            if f==1: line[x]=(line[x]+a)&255
            elif f==2: line[x]=(line[x]+b)&255
            elif f==3: line[x]=(line[x]+(a+b)//2)&255
            elif f==4:
                pa=abs(b-c); pb=abs(a-c); pc=abs(a+b-2*c)
                pr=a if (pa<=pb and pa<=pc) else (b if pb<=pc else c)
                line[x]=(line[x]+pr)&255
        out += list(line); prev = line
    return out
for name in ["perspective_vertical","perspective_aspect"]:
    px = read_png(glob.glob(f"**/{name}.png", recursive=True)[0])
    print(name, "max =", max(px), "clipped =", sum(1 for v in px if v >= 255))
a = read_png(glob.glob("**/perspective_vertical.png", recursive=True)[0])
b = read_png(glob.glob("**/perspective_aspect.png", recursive=True)[0])
print("the two goldens differ:", a != b)
PY
cd ../../..
```

Expected: `clipped = 0` for both, and `the two goldens differ: True`.

⚠ If either clips, lower the `0.55` in `grid_chart()` and regenerate. ⚠ If they do not differ, one of the two axes is inert — find it rather than accepting the pair.

- [ ] **Step 4: Full green and commit**

```bash
cargo fmt
cargo clippy --all-targets --all-features > /tmp/clippy.log 2>&1; echo "clippy exit: $?"
cargo test --all 2>&1 | grep "test result:" | awk -F'[ ;]' '{p+=$4; f+=$6} END {print "passed="p, "failed="f}'
git add -A
git commit -m "test(pipeline): two perspective goldens on a grid chart"
```

---

### Task 5: The panel, and the rail rename

**Files:**
- Modify: `crates/app/src/develop/panel/geometry.rs`
- Modify: `crates/app/src/tool.rs`

- [ ] **Step 1: Rename the rail entry**

In `crates/app/src/tool.rs`, in `fn label`:

```rust
            Self::Crop => "Geometry",
```

⚠ The enum variant stays `Tool::Crop` — renaming it would churn every call site for no gain, and the hover text already reads *"Crop & Geometry"*. Only the rail's visible label changes.

- [ ] **Step 2: Add the sliders**

In `crates/app/src/develop/panel/geometry.rs`, immediately after the Straighten slider line

```rust
    ed.slider(ui, &mut s.edits.geometry.angle, -45.0..=45.0, "Straighten");
```

insert:

```rust

    ui.separator();
    // ⚠ Here rather than in the Edit panel because this is the only view that can judge it: crop
    // mode previews the *uncropped* frame, which is where the empty corners a keystone opens are
    // visible at all. The Lens `Scale` slider above is here for the identical reason.
    ui.label("Perspective");
    let p = &mut s.edits.perspective;
    ed.slider(ui, &mut p.vertical, -100.0..=100.0, "Vertical");
    ed.slider(ui, &mut p.horizontal, -100.0..=100.0, "Horizontal");
    ed.slider(ui, &mut p.aspect, -100.0..=100.0, "Aspect");
    ed.slider(ui, &mut p.scale, 0.0..=100.0, "Scale");
```

- [ ] **Step 3: Fix the reset button's now-stale text**

⚠⚠ Adding perspective to `GEOMETRY_GROUP` makes the existing reset button silently reset a fifth thing it does not mention — the same failure as the "Reset adjustments" hover text that enumerated five panel sections and went wrong when a sixth arrived.

Change the button from `"Reset crop"` to `"Reset geometry"`, and its hover text to:

```rust
        .on_hover_text("Resets the crop, straighten, rotation, flip and perspective. Leaves the adjustments and any masks alone.")
```

Add above it:

```rust
    // ⚠ Scoped to `GEOMETRY_GROUP`, so it resets whatever is in that group — including anything
    // added later. **Anything added to that group must be named in the hover text above**, or
    // this button silently resets something it does not mention.
```

- [ ] **Step 4: Add the two headless panel tests**

Append to `mod tests` in `crates/app/src/develop/panel/geometry.rs`. It already has a `session()`
fixture and a `click_in_panel` helper; these reuse `session()` and call `show` with the same five
arguments `click_in_panel` does.

```rust
    /// ⚠ That the sliders are actually drawn. A section added to the wrong branch compiles, runs,
    /// and is simply never on screen — which no other test in this file would notice.
    #[test]
    fn the_perspective_sliders_are_drawn() {
        let ctx = test_ui::ctx();
        let mut s = session();
        let mut aspect = None;
        let mut ed = Edited::default();
        let found = test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
            show(ui, &mut s, &mut aspect, false, &mut ed);
        });
        for want in ["Vertical", "Horizontal", "Aspect", "Scale"] {
            assert!(
                found.iter().any(|(label, _)| label.contains(want)),
                "no “{want}” slider in the Geometry tool; found {:?}",
                found.iter().map(|(l, _)| l).collect::<Vec<_>>()
            );
        }
    }

    /// ⚠ The rail must say what the tool now does. Asserted on `Tool::label` rather than by
    /// finding the widget, because the rail is drawn by a different module entirely.
    #[test]
    fn the_rail_calls_the_crop_tool_geometry() {
        assert_eq!(rustroom_app::tool::Tool::Crop.label(), "Geometry");
    }
```

⚠ **`Scale` appears twice when a lens profile is present** — the Lens block draws one too. The
test passes `lens = false`, so only the perspective one is drawn and the assertion is unambiguous.
That duplication is deliberate and pre-existing; do not "fix" it by renaming either slider.

- [ ] **Step 5: Green, clippy, commit**

```bash
cargo fmt
cargo clippy --all-targets --all-features > /tmp/clippy.log 2>&1; echo "clippy exit: $?"
cargo test --all 2>&1 | grep "test result:" | awk -F'[ ;]' '{p+=$4; f+=$6} END {print "passed="p, "failed="f}'
git add -A
git commit -m "feat(app): perspective sliders in the Geometry tool, and the rail renamed"
```

---

### Task 6: Sabotage, hand verification, documentation

**Files:**
- Modify: `CLAUDE.md`
- Modify: `docs/BACKLOG.md`

- [ ] **Step 1: Run the sabotages**

⚠⚠ **Confirm each one COMPILED before believing it.** A sabotage that fails to build looks exactly like one that passes, and a harness that greps for the wrong string reports a universal false alarm — both variants are recorded in `CLAUDE.md`. The robust check is that a `test result:` line appears at all.

Back up first, and verify each restore landed:

```bash
cp crates/pipeline/src/perspective.rs /tmp/persp.bak
cp crates/edit-model/src/lib.rs /tmp/em.bak
```

Run these five, one at a time, restoring between each:

| # | Sabotage | Expected to fail |
|---|---|---|
| 1 | In `from_angles`, swap `r_pitch` and `r_yaw`'s axes (make pitch rotate about y) | `a_positive_pitch_widens_the_top_relative_to_the_bottom` |
| 2 | Negate **only** `sp` in `r_pitch` (leave the matrix otherwise) | `the_predicted_pitch_makes_converging_verticals_parallel` |
| 3 | In `Perspective::is_active`, add `\|\| self.scale != 100.0` | `scale_alone_does_not_activate_the_perspective_stage` |
| 4 | Replace `Perspective`'s hand-written `Default` with `#[derive(Default)]` | `a_neutral_perspective_scales_fully` |
| 5 | In `from_edits`, use `let k = full;` (ignore the slider) | `scale_zero_leaves_the_corners_empty` |

⚠ Sabotage 2 negates **one** rotation only. Negating a matrix and its inverse together stays self-consistent and the round trip survives — a trap hit twice in the colour-management work.

For each: apply, run `cargo test --all 2>&1 | grep -E "test result:|FAILED"`, confirm a `test result:` line **appeared** and the named test is in the failures, then restore and verify:

```bash
cp /tmp/persp.bak crates/pipeline/src/perspective.rs
cp /tmp/em.bak crates/edit-model/src/lib.rs
git diff --stat   # must be empty for these two files
```

- [ ] **Step 2: Look at it on a real photograph**

```bash
cargo build --release -p app 2>&1 | tail -1
nohup ./target/release/Rustroom > /tmp/rustroom.log 2>&1 &
```

Check by hand, and report honestly what is seen:
1. The rail's second entry reads **Geometry**, and `2` still selects it.
2. Open a photo with converging verticals, enter the Geometry tool, drag **Vertical** — the verticals straighten, and the frame does **not** show empty corners because Scale defaults to 100.
3. Drag **Scale** to 0 — the empty corners appear. That is the pair that proves both ends work.
4. **Aspect** stretches and un-stretches.
5. Ctrl+Z undoes a slider drag as one step.
6. "Reset geometry" clears the perspective along with the crop and straighten.
7. Switch to another photo and back — the perspective persists via the sidecar.

- [ ] **Step 3: Measure the softness the spec deferred a decision on**

The spec left fusing perspective into `GeometryStage` on the backlog **conditional on measuring visible softness**, rather than assuming it away. At 1:1 on a real photo, compare a strong Vertical correction against the same photo with no correction, and say plainly whether the extra resample is visible. Record the answer in the CLAUDE.md section either way — "not visible at 1:1" is a result, not a non-answer.

- [ ] **Step 4: Write the CLAUDE.md section**

Add a section immediately above the most recent one, following the house style: a one-line summary with the test count delta and golden count, then ⚠-marked bullets for each non-obvious decision, then **Accepted caveats** as a numbered list. Cover at minimum:

- The `mod_pc` finding — it is a control-point API, so the slider half cannot use it; and the two facts the guided slice will need (`build_perspective_state` is a free function needing no `Lens`; at 8 points lensfun estimates the focal length itself).
- Why `F_NORM` is fixed rather than read from EXIF, including the `tan θ = −f / y_v` argument and the consequence that the same slider value is not the same physical pitch on different lenses.
- Why the stage sits upstream of `GeometryStage` and what that buys: no `FrameMap` change, no `Region`, no `RenderScale`, its own cache checkpoint.
- The `Default` and `is_active` traps.
- The stale reset-button text, and the rule now written at that call site.
- The result of the softness measurement from Step 3.

Update the **▶ THE NEXT ACTION** block: M3.2's automatic half plus perspective means **M3.2 is now complete apart from nothing** — state Phase 3's remaining gap as creative 3-D LUTs alone.

- [ ] **Step 5: Update the backlog**

Remove the "Perspective correction (M3.2's remaining slice)" bullet from `docs/BACKLOG.md`. **Add two entries in its place**:

1. **Guided perspective control points** — the more powerful half. Record the two `mod_pc` facts so the next person does not re-derive them.
2. **Fuse perspective into `GeometryStage`** — one resample instead of two, conditional on the Step 3 measurement, with the `FrameMap` cost stated.

- [ ] **Step 6: Final green and commit**

```bash
cargo fmt --check && echo "fmt ok"
cargo clippy --all-targets --all-features > /tmp/clippy.log 2>&1; echo "clippy exit: $?"
cargo test --all 2>&1 | grep "test result:" | awk -F'[ ;]' '{p+=$4; f+=$6} END {print "passed="p, "failed="f}'
git add -A
git commit -m "docs: perspective correction shipped; M3.2's manual half closed"
git log main..HEAD --oneline
```

⚠ **Do not push and do not merge.** Report the branch state and wait.
