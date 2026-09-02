# M5.4b Display Rendering Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A RAW opens looking like the photograph the camera made, via one new pipeline stage that also bounds the data to `[0, 1]` — the seam creative LUTs and `ProfileLookTable` were blocked on.

**Architecture:** `DisplayRenderingStage` sits between Sharpening and `OutputTransform`. It applies a **monotone cubic spline over log-exposure**, per channel, compiled to a LUT. `Edits.rendering` defaults to `None` so every golden and the CLI are untouched; a `Settings` default turns it on for photos with no sidecar. RAW only — JPEG/PNG/TIFF and embedded thumbnails are already rendered.

**Tech Stack:** Rust. Spec: `docs/superpowers/specs/2026-09-02-display-rendering-design.md`.

**Build environment:**
```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

**Baseline: 1729 tests passed, 0 failed, 53 goldens.** Verify with unique log names, count with Python, trust the exit code:
```bash
L=/tmp/m54b-$$-test.log
cargo fmt --all --check > /tmp/m54b-$$-f.log 2>&1; echo "FMT=$?"
cargo clippy --all-targets --all-features -- -D warnings > /tmp/m54b-$$-c.log 2>&1; echo "CLIPPY=$?"
cargo test --all > "$L" 2>&1; echo "TEST=$?"
python3 -c "
import re
p=f=0
for l in open('$L',errors='replace'):
    m=re.match(r'test result: \w+\. (\d+) passed; (\d+) failed',l)
    if m: p+=int(m.group(1)); f+=int(m.group(2))
print(f'passed={p} failed={f}')"
```

**Sabotage discipline.** After each test, break what it names and watch it fail. Confirm it **ran** (`passed + failed >= 1`), **compiled** (a `test result:` line exists), and that the file **actually changed** (`cargo fmt` reflows, so a text match can apply to nothing).

**Conventions:** no `Co-Authored-By` trailer; **never push**; commit per task.

---

## ⚠⚠ The division of labour this plan rests on

**The camera's JPEG brightness includes its own per-frame metering decision.** A fixed tone curve
cannot encode that and must not try — **M5.4a Auto Tone already owns per-frame exposure.** So:

- **The rendering owns the tonal *shape*:** crush below ~0.01, lift through the midtones, roll off
  the highlights.
- **Auto Tone owns the *level*.**

⚠ This is why Task 8's acceptance is *"the binned ratio against the camera becomes approximately
**constant**"* rather than *"the mean gap reaches zero"*. A constant ratio means "same shape,
different exposure", which is the correct outcome. Chasing the absolute level would bake one
frame's metering into a curve every photograph pays.

---

## File Structure

| File | Responsibility |
| --- | --- |
| `crates/pipeline/src/spline.rs` **(new)** | The monotone Fritsch–Carlson spline, extracted from `tone_curve.rs` so two stages share one interpolant. |
| `crates/pipeline/src/stages/display_rendering.rs` **(new)** | The stage: log-exposure mapping, the built-in curve, the LUT, per-channel application. |
| `crates/pipeline/src/stages/tone_curve.rs` | Loses `Spline`; uses `crate::spline::Spline`. |
| `crates/pipeline/src/lib.rs` | `pub mod spline;`, the stage's insertion, the 7th parameter. |
| `crates/edit-model/src/lib.rs` | `Rendering`, `Edits.rendering`, the `Rendering` group. |
| `crates/camera-profile/src/lib.rs` | DNG tag 50940 → `Profile.tone_curve`. |
| `crates/app/src/decode_worker.rs` | `Decoded.profile_curve`, `Decoded.scene_referred`. |
| `crates/app/src/render_engine.rs` | Captures the profile curve, passes it to the builder. |
| `crates/app/src/thumb_loader.rs` | Thumbnails render with `Rendering::None`. |
| `crates/app/src/settings.rs` | `default_rendering`. |
| `crates/app/src/develop/panel/adjustments.rs` | The checkbox at the top of Tone. |

---

## Task 1: Extract the spline

**Files:** Create `crates/pipeline/src/spline.rs`; modify `crates/pipeline/src/stages/tone_curve.rs`, `crates/pipeline/src/lib.rs`.

- [ ] **Step 1: Move the type verbatim**

Cut `struct Spline`, its `impl`, the free `fn end_tangent`, and the consts `X_EPS` and `FLAT_EPS`
out of `tone_curve.rs` into a new `crates/pipeline/src/spline.rs`. Make `Spline`, `Spline::new`
and `Spline::eval` `pub(crate)`; leave `end_tangent`, `X_EPS` and `FLAT_EPS` private to the module.
Keep **every doc comment verbatim** — the Fritsch–Carlson rationale and the `X_EPS` NaN note are
load-bearing.

Head the new file with:

```rust
//! A **monotone** cubic interpolant, shared by every curve in the pipeline.
//!
//! ⚠ Extracted from `stages/tone_curve.rs` when the display rendering needed the same
//! interpolant. `tone_curve`'s own tests passed unchanged through the move, which is the
//! evidence it moved no behaviour.
//!
//! ⚠⚠ **Control points are clamped to `[0, 1]` in both axes.** A caller working in another
//! domain — log-exposure, say — must normalise into `[0, 1]` first, or every point collapses
//! onto the edges and the curve flattens to garbage. `DisplayRenderingStage` does exactly that.
```

- [ ] **Step 2: Wire it up**

In `crates/pipeline/src/lib.rs`, beside the other module declarations:

```rust
pub(crate) mod spline;
```

In `tone_curve.rs`, replace the removed items with:

```rust
use crate::spline::Spline;
```

- [ ] **Step 3: Verify nothing moved**

```bash
cargo test -p pipeline tone_curve 2>&1 | grep "test result"
```
Expected: the same count as before the move, **0 failed**, with **no test edited**. If any
assertion had to change, the extraction changed behaviour — stop and find out why.

- [ ] **Step 4: Commit**

```bash
git add -A crates/pipeline
git commit -m "refactor(pipeline): the monotone spline is its own module

Two curves need it now. tone_curve's tests passed unchanged through the
move, which is the evidence it moved no behaviour."
```

---

## Task 2: The model

**Files:** `crates/edit-model/src/lib.rs`.

- [ ] **Step 1: Write the failing tests**

Add to `edit-model`'s test module:

```rust
#[test]
fn the_default_rendering_is_colorimetric_only() {
    // ⚠⚠ `None` by default is what keeps `Edits::default()` strictly neutral, and therefore
    // what keeps all 53 goldens and the CLI untouched. A default of `Camera` would move every
    // one of them.
    assert_eq!(Edits::default().rendering, Rendering::None);
}

#[test]
fn a_sidecar_written_before_the_rendering_still_loads() {
    let e: Edits = serde_json::from_str(r#"{"exposure":0.5}"#).expect("parses");
    assert_eq!(e.rendering, Rendering::None);
    assert_eq!(e.exposure, 0.5);
}

#[test]
fn the_rendering_travels_on_its_own_group_and_by_default() {
    // ⚠ Included in `EditGroups::default()`: a rendering is a *look*, so it should follow a
    // paste, a preset and a sync — unlike Geometry and Masks, which are placements.
    assert!(EditGroups::default().get(RENDERING_GROUP));

    let src = Edits { rendering: Rendering::Camera, exposure: 1.0, ..Default::default() };
    let mut dst = Edits::default();
    dst.merge_from(&src, EditGroups::only(RENDERING_GROUP));
    assert_eq!(dst.rendering, Rendering::Camera, "the rendering did not transfer");
    assert_eq!(dst.exposure, 0.0, "its group carried something else too");
}
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test -p edit-model rendering 2>&1 | grep -E "^error|test result"
```
Expected: `cannot find type Rendering`.

- [ ] **Step 3: Add the type and the field**

```rust
/// How a scene-referred photograph is mapped onto a display.
///
/// ⚠⚠ **A rendering is not an adjustment.** Every slider in this pipeline moves a photograph
/// that is already in a defined state; a rendering *establishes* that state — it turns
/// scene-referred light into something a screen can show. That is why it curves each channel
/// independently rather than preserving hue like every other tonal stage, and why "Reset
/// adjustments" does not touch it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Rendering {
    /// Colorimetric only — the primaries and the transfer curve, nothing else. What Rustroom
    /// did before this existed, and what every golden and the CLI still get.
    #[default]
    None,
    /// The camera's rendering: the profile's tone curve when it supplies a usable one, else the
    /// built-in filmic curve.
    Camera,
}
```

Add to `Edits`, beside the other fields:

```rust
    /// How the scene is mapped onto a display. See [`Rendering`].
    #[serde(default)]
    pub rendering: Rendering,
```

Append `"Rendering"` to `EDIT_GROUP_NAMES` (making it 9 entries), add the field to `EditGroups`
with its `get`/`get_mut`/`only` arms, and:

```rust
/// The `Rendering` group's index into [`EDIT_GROUP_NAMES`].
pub const RENDERING_GROUP: usize = 8;
```

- [ ] **Step 4: Follow the compiler through the exhaustive sites**

```bash
cargo build --all --all-targets 2>&1 | grep -E "^error" -A 6 | head -40
```

Fix each: `merge_from`'s destructure (add `rendering` to the `Rendering` group's arm),
`EditGroups`' `Default` (set it **true**), `pipeline::all_edits`, `edit_model::all_nonneutral`
(use `Rendering::Camera`), and `app::edit_clipboard`'s payload fixture. These are exhaustive on
purpose — do not weaken any of them.

⚠ `crates/app/src/develop/panel/adjustments.rs` has a test asserting which groups the panel does
not show. It becomes:

```rust
        assert_eq!(missing, vec!["Geometry", "Masks", "Rendering"]);
```

with the comment extended: `// ⚠ "Rendering" has no section on purpose — see `RENDERING_GROUP`:
a group with no matching section is not touched by "Reset adjustments", which is what stops that
button flattening the photograph.`

- [ ] **Step 5: Run, sabotage, commit**

Sabotage: set `Rendering`'s `#[default]` to `Camera` — `the_default_rendering_is_colorimetric_only`
must fail **and** goldens must fail. Restore.

```bash
git add -A crates
git commit -m "feat(edit-model): a Rendering field, defaulting to colorimetric only

None by default is what keeps Edits::default() neutral and therefore all
53 goldens and the CLI untouched. Its own group, included in the default
transfer set because a rendering is a look rather than a placement, and
with no matching panel section so Reset adjustments cannot flatten it."
```

---

## Task 3: The stage

**Files:** Create `crates/pipeline/src/stages/display_rendering.rs`; modify `crates/pipeline/src/stages/mod.rs`.

- [ ] **Step 1: Write the stage**

```rust
//! Scene-referred light onto a display.
//!
//! ⚠⚠ **A sigmoid in log-exposure, and the measurement is why.** The camera's own transfer was
//! measured by pairing a scene-linear decode against the embedded JPEG of the same frame
//! (`curve_probe`): it **crushes** below ~0.01 (0.19-0.33x) and **lifts** hard above (1.5-2.3x).
//! That is a true S-curve, and it rules out the compressive family this was going to use — a
//! curve like `x^c/(x^c + k)` has a *monotone* log-log slope, so matching the 0.19x toe forces
//! `c ~ 1.67`, which then gives 1.18x at 0.09 where the camera gives 2.28x. It can crush or
//! lift; it cannot do both.
//!
//! ⚠ The stage owns the **shape** and not the **level**. A camera's JPEG brightness includes its
//! own per-frame metering decision, which no fixed curve can encode — that is Auto Tone's job
//! (M5.4a). Chasing it here would bake one frame's metering into every photograph.

use crate::spline::Spline;
use crate::PipelineStage;
use core_image::ImageBuf;

/// Middle grey in the scene-referred working space.
const GREY: f32 = 0.18;
/// The log-exposure window the curve is defined over, in stops relative to [`GREY`].
///
/// ⚠ Everything below `T_MIN` renders black and everything above `T_MAX` renders white, so the
/// window has to cover more dynamic range than any photograph carries. 13 stops does.
const T_MIN: f32 = -8.0;
const T_MAX: f32 = 5.0;

const LUT_SIZE: usize = 1024;
/// The largest scene value the LUT resolves; above it the curve is flat at 1 anyway.
const LUT_MAX: f32 = GREY * 32.0;

/// The built-in filmic curve, as `(u, y)` where `u` is the normalised log-exposure
/// `(t - T_MIN) / (T_MAX - T_MIN)` and `y` is display-linear output.
///
/// ⚠⚠ **Normalised into `[0, 1]` because [`Spline`] clamps its control points there.** Feeding it
/// raw log-exposure would collapse every point onto the edges and flatten the curve to garbage.
///
/// ⚠ **Provisional until Task 8's fit replaces them.** They carry the measured *shape* — a toe
/// that crushes, a steep midtone, a shoulder — with middle grey anchored at `0.18 -> 0.18`, the
/// standard convention. The fit adjusts them against pooled real frames.
const BUILT_IN: [[f32; 2]; 9] = [
    [0.000, 0.000],
    [0.154, 0.0015],
    [0.277, 0.006],
    [0.408, 0.035],
    [0.538, 0.105],
    [0.615, 0.180], // middle grey
    [0.731, 0.450],
    [0.846, 0.750],
    [1.000, 1.000],
];

pub struct DisplayRenderingStage {
    lut: Vec<f32>,
}

impl DisplayRenderingStage {
    /// The built-in filmic rendering.
    pub fn built_in() -> Self {
        Self::from_normalised(&BUILT_IN)
    }

    /// A profile's `ProfileToneCurve`, which is defined on `[0, 1]` of *linear* input rather than
    /// on log-exposure.
    ///
    /// ⚠ Its input is **clamped** to `[0, 1]`, so scene values above 1.0 render white. The
    /// built-in curve does not clamp — it is total on `[0, inf)`. That difference is a caveat of
    /// using a profile's curve, not a bug.
    pub fn from_profile(points: &[[f32; 2]]) -> Self {
        let spline = Spline::new(points);
        let lut = (0..LUT_SIZE)
            .map(|i| {
                let x = i as f32 / (LUT_SIZE - 1) as f32 * LUT_MAX;
                spline.eval(x.clamp(0.0, 1.0)).clamp(0.0, 1.0)
            })
            .collect();
        Self { lut }
    }

    fn from_normalised(points: &[[f32; 2]]) -> Self {
        let spline = Spline::new(points);
        let lut = (0..LUT_SIZE)
            .map(|i| {
                let x = i as f32 / (LUT_SIZE - 1) as f32 * LUT_MAX;
                spline.eval(normalised_log(x)).clamp(0.0, 1.0)
            })
            .collect();
        Self { lut }
    }

    /// The curve's output for one scene value.
    ///
    /// ⚠ Total on `[0, inf)`: a non-finite or negative input reads as 0, and anything past
    /// [`LUT_MAX`] takes the last entry, which the curve has already flattened to 1.
    pub fn map(&self, x: f32) -> f32 {
        if !x.is_finite() || x <= 0.0 {
            return self.lut[0];
        }
        let pos = (x / LUT_MAX).min(1.0) * (LUT_SIZE - 1) as f32;
        let i = pos.floor() as usize;
        if i >= LUT_SIZE - 1 {
            return self.lut[LUT_SIZE - 1];
        }
        let frac = pos - i as f32;
        self.lut[i] + frac * (self.lut[i + 1] - self.lut[i])
    }
}

/// Scene value to normalised log-exposure, clamped into the spline's domain.
fn normalised_log(x: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    let t = (x / GREY).log2();
    ((t - T_MIN) / (T_MAX - T_MIN)).clamp(0.0, 1.0)
}

impl PipelineStage for DisplayRenderingStage {
    fn name(&self) -> &'static str {
        "display_rendering"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        // ⚠⚠ **Per channel, not on luminance with a scalar gain.** Every other tonal stage here
        // preserves hue deliberately; this one must not. Curving the channels independently is
        // where a camera rendering's shadow saturation and its roll toward white come from, and
        // it is what the DNG spec does with `ProfileToneCurve`. A neutral grey still stays
        // exactly neutral, because all three channels take the same curve.
        image.map_pixels(|[r, g, b]| [self.map(r), self.map(g), self.map(b)])
    }
}
```

Add `pub mod display_rendering;` to `crates/pipeline/src/stages/mod.rs` and re-export in
`lib.rs` beside the others:

```rust
pub use stages::display_rendering::DisplayRenderingStage;
```

- [ ] **Step 2: Write the tests**

Append to `display_rendering.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// ⚠ Total and monotone over everything a scene-referred buffer can hold, including values
    /// far above 1.0 and the degenerate ones.
    #[test]
    fn the_curve_is_total_and_monotone() {
        let s = DisplayRenderingStage::built_in();
        let mut last = -1.0;
        for i in 0..=2000 {
            let x = i as f32 / 2000.0 * 64.0;
            let y = s.map(x);
            assert!((0.0..=1.0).contains(&y), "x={x} left the range: {y}");
            assert!(y >= last - 1e-6, "x={x} went backwards: {y} after {last}");
            last = y;
        }
        for x in [0.0, -1.0, f32::NAN, f32::INFINITY, f32::MAX] {
            let y = s.map(x);
            assert!(y.is_finite() && (0.0..=1.0).contains(&y), "{x} gave {y}");
        }
    }

    /// ⚠⚠ **The shape, and the only test here that can see a curve fitted with the wrong sign
    /// or built in linear instead of log space.** Total-and-monotone passes against a plain
    /// ramp; this does not. The numbers are the measured camera behaviour: crush below ~0.01,
    /// lift above.
    #[test]
    fn the_curve_crushes_the_deep_shadows_and_lifts_the_midtones() {
        let s = DisplayRenderingStage::built_in();
        assert!(s.map(0.005) / 0.005 < 1.0, "the toe does not crush");
        assert!(s.map(0.05) / 0.05 > 1.0, "the midtones are not lifted");
        // And it is a *curve*, not a gain: the two ratios differ by a wide margin.
        assert!((s.map(0.05) / 0.05) > 2.0 * (s.map(0.005) / 0.005));
    }

    /// ⚠ Middle grey is the anchor the whole design hangs on — the rendering owns the shape and
    /// Auto Tone owns the level, which only works if grey is a fixed point.
    #[test]
    fn middle_grey_is_preserved() {
        let s = DisplayRenderingStage::built_in();
        assert!((s.map(GREY) - GREY).abs() < 0.01, "grey moved to {}", s.map(GREY));
    }

    /// ⚠⚠ **Per channel, and both halves matter.** A neutral-only assertion passes against a
    /// luminance-and-scalar-gain model, which is what every other stage here uses.
    #[test]
    fn a_neutral_stays_neutral_and_a_colour_shifts() {
        let s = DisplayRenderingStage::built_in();
        let mut img = ImageBuf::new(2, 1);
        img.set_pixel(0, 0, [0.2, 0.2, 0.2]);
        img.set_pixel(1, 0, [0.30, 0.10, 0.02]);
        let out = s.apply(img);

        let n = out.pixel(0, 0);
        assert!((n[0] - n[1]).abs() < 1e-6 && (n[1] - n[2]).abs() < 1e-6, "grey shifted: {n:?}");

        let c = out.pixel(1, 0);
        let before = 0.30 / 0.10;
        let after = c[0] / c[1];
        assert!(
            (after - before).abs() > 0.05,
            "the channel ratio did not change, so this is a luminance model: {before} -> {after}"
        );
    }

    /// ⚠ A profile's curve is defined on `[0, 1]`, so it clamps above 1.0 where the built-in
    /// does not. Asserting both is what keeps the difference deliberate.
    #[test]
    fn a_profile_curve_clamps_above_one_and_the_built_in_does_not() {
        let profile = DisplayRenderingStage::from_profile(&[[0.0, 0.0], [0.5, 0.6], [1.0, 1.0]]);
        assert!((profile.map(2.0) - profile.map(1.0)).abs() < 1e-6, "the profile curve did not clamp");
        let built_in = DisplayRenderingStage::built_in();
        assert!(built_in.map(4.0) > built_in.map(1.0), "the built-in clamped at 1.0");
    }
}
```

- [ ] **Step 3: Run, sabotage, commit**

```bash
cargo test -p pipeline display_rendering 2>&1 | grep -E "test result|panicked"
```

Sabotage each: replace `normalised_log`'s body with `x.clamp(0.0, 1.0)` (the shape test must
fail); make `apply` use `color::luminance` and a scalar gain (the per-channel test must fail);
drop the `.clamp(0.0, 1.0)` in `from_profile` (the clamp test must fail).

```bash
git add -A crates/pipeline
git commit -m "feat(pipeline): a display rendering stage

A sigmoid in log-exposure, measured rather than assumed: the camera
crushes below 0.01 and lifts 1.5-2.3x above, which a compressive curve
cannot do -- its log-log slope is monotone, so it crushes or lifts, never
both. Per channel, because that is where a rendering's shadow saturation
and roll toward white come from."
```

---

## Task 4: Insert it into the pipeline

**Files:** `crates/pipeline/src/lib.rs`.

- [ ] **Step 1: Write the failing test**

In `pipeline`'s test module:

```rust
/// ⚠⚠ `Rendering::None` must insert **no stage at all**, not a curve that happens to be the
/// identity. Bit-exactness is the rule the entire golden harness rests on.
#[test]
fn the_default_rendering_inserts_no_stage() {
    let names: Vec<&str> = build_pipeline(&Edits::default(), RenderScale::new(1.0))
        .stage_names();
    assert!(
        !names.contains(&"display_rendering"),
        "a neutral photo got a rendering stage: {names:?}"
    );
}

/// ⚠ And it lands between sharpening and the output transform — after every capture stage so
/// their linear tuning survives, before the transform so the histogram sees the rendered image.
#[test]
fn the_rendering_runs_last_before_the_output_transform() {
    let edits = Edits { rendering: Rendering::Camera, ..Default::default() };
    let names: Vec<&str> = build_pipeline(&edits, RenderScale::new(1.0)).stage_names();
    let r = names.iter().position(|n| *n == "display_rendering").expect("no rendering stage");
    let o = names.iter().position(|n| *n == "output_transform").expect("no output transform");
    assert_eq!(o, r + 1, "something got between the rendering and the transform: {names:?}");
}
```

⚠ If `Pipeline` has no `stage_names()`, add one returning `Vec<&'static str>` from each stage's
`name()`, and note in its doc that it exists so ordering is asserted rather than assumed.

- [ ] **Step 2: Run to verify it fails**

Expected: `no rendering stage`.

- [ ] **Step 3: Insert the stage**

In `build_pipeline_split_in_region_with_brush`, add a **seventh** parameter after `lens`:

```rust
    profile_curve: Option<&[[f32; 2]]>,
```

⚠ Not a `RenderSpec` field: `RenderSpec` is `Copy` and a curve is not, which is exactly why
`lens` sits outside it too. An eighth parameter would trip `too_many_arguments` and want a
struct; seven does not.

Immediately **before** the `OutputTransform` push:

```rust
    // ⚠⚠ Last before the output transform. Every capture stage upstream keeps its scene-linear
    // tuning, and this is what bounds the data to [0, 1] — which is the seam creative LUTs and
    // ProfileLookTable were blocked on.
    match edits.rendering {
        edit_model::Rendering::None => {}
        edit_model::Rendering::Camera => {
            // ⚠⚠ An **identity** profile curve is ignored. The X-T4's ProfileToneCurve is
            // literally [[0,0],[1,1]], so honouring it would produce no rendering at all —
            // indistinguishable from `None`, and the setting would silently do nothing on the
            // author's own camera.
            let stage = match profile_curve.filter(|p| !is_identity(p)) {
                Some(p) => DisplayRenderingStage::from_profile(p),
                None => DisplayRenderingStage::built_in(),
            };
            tail.push(Box::new(stage));
        }
    }
```

And beside it:

```rust
/// Whether a profile's tone curve is the identity, and therefore no rendering at all.
fn is_identity(points: &[[f32; 2]]) -> bool {
    points.len() < 2 || points.iter().all(|p| (p[0] - p[1]).abs() < 1e-4)
}
```

Update the delegating wrappers (`build_pipeline`, `build_pipeline_split`, …) to pass `None`.

- [ ] **Step 4: Fix the call sites**

```bash
cargo build --all --all-targets 2>&1 | grep -E "^error" -A 4 | head -20
```
`golden.rs`, `crates/rustroom/src/main.rs`, `crates/app/src/batch_export.rs` and
`crates/app/src/render_engine.rs` all pass `None` for now; Task 6 gives the engine a real one.

- [ ] **Step 5: Run everything — the goldens are the point**

```bash
cargo test --all 2>&1 | grep -E "test result: FAILED|^failures:" -A 5
git status --porcelain crates/pipeline/tests/golden/
```
Expected: **0 failed and no PNG modified.** A moved golden means the stage is running when it
should not.

- [ ] **Step 6: Sabotage and commit**

Sabotage: make the `None` arm push `built_in()` too — many goldens must fail. Restore.

```bash
git add -A crates
git commit -m "feat(pipeline): the rendering runs last before the output transform

An identity profile curve is ignored rather than honoured: the X-T4's is
literally [[0,0],[1,1]], so applying it would give no rendering at all and
the setting would silently do nothing on real hardware."
```

---

## Task 5: Parse `ProfileToneCurve`

**Files:** `crates/camera-profile/src/lib.rs`.

- [ ] **Step 1: Write the failing tests**

```rust
/// ⚠ Built by hand rather than read from a real `.dcp`, so it runs in CI: the fixture that made
/// the matrices testable at all.
#[test]
fn a_profile_tone_curve_is_read() {
    let dcp = synthetic_dcp_with_tone_curve(&[[0.0, 0.0], [0.5, 0.7], [1.0, 1.0]]);
    let p = Profile::parse(&dcp).expect("parses");
    let c = p.tone_curve.expect("the curve was not read");
    assert_eq!(c.len(), 3);
    assert!((c[1][1] - 0.7).abs() < 1e-5, "wrong values: {c:?}");
}

#[test]
fn a_profile_without_a_tone_curve_reports_none() {
    let db = synthetic_dcp_without_tone_curve();
    assert!(Profile::parse(&db).expect("parses").tone_curve.is_none());
}
```

Write `synthetic_dcp_with_tone_curve` / `synthetic_dcp_without_tone_curve` beside the existing
synthetic-profile helpers in that file's test module, following whatever helper already builds a
DCP there.

- [ ] **Step 2: Run to verify they fail**

Expected: `no field tone_curve`.

- [ ] **Step 3: Parse the tag**

Add to `Profile`:

```rust
    /// The profile's `ProfileToneCurve` (DNG tag 50940), as `(x, y)` control points on `[0, 1]`.
    ///
    /// ⚠ Stored as read. Whether it is *used* is the pipeline's decision — an identity curve is
    /// ignored there, because the X-T4's is exactly that and honouring it would mean no
    /// rendering at all.
    pub tone_curve: Option<Vec<[f32; 2]>>,
```

Parse tag **50940**: a `FLOAT` array of `2n` values, `x0, y0, x1, y1, …`. Reject an odd length as
`None` rather than erroring — a malformed curve should cost the rendering, not the profile.

- [ ] **Step 4: Run, sabotage, commit**

Sabotage: parse the pairs as `(y, x)` — `a_profile_tone_curve_is_read` must fail on the value.

```bash
git add -A crates/camera-profile
git commit -m "feat(camera-profile): read ProfileToneCurve (tag 50940)"
```

---

## Task 6: Thread it through the app

**Files:** `crates/app/src/decode_worker.rs`, `crates/app/src/decode.rs`, `crates/app/src/main.rs`, `crates/app/src/session_flow.rs`, `crates/app/src/render_engine.rs`, `crates/app/src/thumb_loader.rs`.

- [ ] **Step 1: Write the failing tests**

In `thumb_loader.rs`:

```rust
/// ⚠⚠ **A thumbnail is the camera's embedded JPEG, which is already rendered.** Running our
/// rendering over it would render twice — and it is also what closes the strip/canvas gap
/// rather than widening it: the strip keeps the camera's rendering and the canvas gains ours.
#[test]
fn a_thumbnail_is_never_rendered_twice() {
    let edits = Edits { rendering: Rendering::Camera, exposure: 0.5, ..Default::default() };
    assert_eq!(
        thumbnail_edits(&edits).rendering,
        Rendering::None,
        "the thumbnail path kept the rendering"
    );
    assert_eq!(thumbnail_edits(&edits).exposure, 0.5, "it dropped the other edits too");
}
```

- [ ] **Step 2: Implement**

In `thumb_loader.rs`:

```rust
/// The edits a *thumbnail* renders through: the photo's, with the display rendering removed.
///
/// ⚠ See `a_thumbnail_is_never_rendered_twice`. Everything else is kept, because an
/// edit-accurate thumbnail is the whole point of rendering one at all.
fn thumbnail_edits(edits: &Edits) -> Edits {
    Edits { rendering: Rendering::None, ..edits.clone() }
}
```

and call it in `render_edits`'s caller so the pipeline is built from it.

Add to `Decoded`:

```rust
    /// The camera profile's tone curve, when it has one.
    pub profile_curve: Option<Vec<[f32; 2]>>,
    /// Whether `buffer` holds **scene-referred** light. False for JPEG/PNG/TIFF, which have
    /// already been rendered by whatever produced them.
    ///
    /// ⚠⚠ The display rendering is skipped entirely when this is false — applying ours on top of
    /// a camera's would render twice. The decoder is the only component that knows, so the flag
    /// rides with the decode.
    pub scene_referred: bool,
```

Set `scene_referred: true` in the RAW path and `false` in the `image-decode` path; fill
`profile_curve` from the resolved camera profile in the RAW path and `None` otherwise.

Carry both onto `Session` in `finalize_session`, pass `profile_curve` to `RenderEngine::new`
(captured, like `lens`, because it cannot change without a re-decode), and have the engine pass
`profile_curve.as_deref()` to the builder. In the engine's render call, when
`!scene_referred`, build from `thumbnail_edits`-style stripped edits so a JPEG never renders
through the stage.

- [ ] **Step 3: Run, sabotage, commit**

Sabotage: make `thumbnail_edits` return `edits.clone()` — the test must fail.

```bash
git add -A crates/app
git commit -m "feat(app): carry the profile curve, and never render a thumbnail twice

The rendering is scene-referred only. A JPEG and an embedded thumbnail have
already been rendered by whatever produced them, so the flag rides with the
decode -- the decoder is the only thing that knows."
```

---

## Task 7: The setting and the control

**Files:** `crates/app/src/settings.rs`, `crates/app/src/develop/panel/adjustments.rs`, and wherever a photo with no sidecar gets its starting `Edits` (the same place `auto_lens_correction` is applied).

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_settings_file_from_before_the_rendering_still_loads() {
    // ⚠ `SettingsStore::load` is infallible, so one unparseable field silently reverts *every*
    // setting — which is why every new field gets this test.
    let s: Settings = serde_json::from_str(r#"{"library_cell":200}"#).expect("parses");
    assert_eq!(s.default_rendering, Rendering::Camera);
    assert_eq!(s.library_cell, 200);
}
```

and beside `auto_lens_correction`'s test:

```rust
/// ⚠ A photo with **no sidecar** gets the setting; one that already has edits keeps them, or
/// opening an old photo would silently change how it renders.
#[test]
fn only_a_photo_without_a_sidecar_takes_the_default_rendering() {
    let fresh = starting_edits(None, Rendering::Camera);
    assert_eq!(fresh.rendering, Rendering::Camera);

    let stored = Edits { rendering: Rendering::None, exposure: 1.0, ..Default::default() };
    let kept = starting_edits(Some(stored), Rendering::Camera);
    assert_eq!(kept.rendering, Rendering::None, "an existing photo's rendering was overwritten");
}
```

⚠ If no such seam exists, add `starting_edits(stored: Option<Edits>, default_rendering: Rendering)
-> Edits` next to the `auto_lens_correction` logic and route both through it.

- [ ] **Step 2: Implement**

`Settings.default_rendering: Rendering`, default `Rendering::Camera`, with the container-level
`#[serde(default)]` already covering a missing key. Apply it in `starting_edits`.

In `adjustments.rs`'s `tone`, **above** the Auto button:

```rust
    // ⚠ At the top of Tone because it is the baseline the tonal sliders sit on, not another
    // adjustment. A checkbox rather than a combo because `Rendering` has two variants; a third
    // makes it a combo.
    let mut on = edits.rendering == edit_model::Rendering::Camera;
    if ui
        .checkbox(&mut on, "Camera rendering")
        .on_hover_text("Map the scene onto the display the way the camera would. RAW only.")
        .changed()
    {
        edits.rendering = if on {
            edit_model::Rendering::Camera
        } else {
            edit_model::Rendering::None
        };
        ed.mark_changed();
    }
    ui.separator();
```

- [ ] **Step 3: Run, sabotage, commit**

Sabotage: apply the default unconditionally in `starting_edits` — the "keeps them" half must fail.

```bash
git add -A crates/app
git commit -m "feat(develop): a Camera rendering checkbox and its default

The setting applies only to a photo with no sidecar yet, the
auto_lens_correction rule -- opening an old photo must not silently change
how it renders."
```

---

## Task 8: Fit the curve, then hand the photographs over

**Files:** `crates/pipeline/src/stages/display_rendering.rs` (the `BUILT_IN` points), `crates/app/examples/curve_probe.rs`.

- [ ] **Step 1: Gather the data**

Extend `curve_probe` to take several RAWs, pool their (scene, camera) pairs into the same bins,
and print one table. Run it over **at least four frames spanning dark to bright**, including
`~/Desktop/fuji/DSCF6489.RAF` (darkest), `DSCF6509.RAF` and `DSCF6383.RAF` (brightest):

```bash
cargo run --release -p app --example curve_probe ~/Desktop/fuji/DSCF64*.RAF ~/Desktop/fuji/DSCF65*.RAF
```

- [ ] **Step 2: Check whether one curve fits all of them**

⚠⚠ **Print the per-frame tables as well as the pooled one, and compare.** If the frames disagree
materially, the difference is the camera's **per-frame metering**, which no fixed curve can encode
and which Auto Tone already owns. In that case fit the **shape** — normalise each frame's pairs so
its own middle grey lands at 0.18 before pooling — rather than the absolute levels. Report which
case held; it is the single most useful thing this task produces.

- [ ] **Step 3: Fit and replace**

Choose the nine `BUILT_IN` control points so the modelled curve tracks the pooled data, keeping
`[0.615, 0.180]` (middle grey) fixed. Replace the array and change its doc comment from
"Provisional until Task 8's fit replaces them" to a note recording the frames fitted against and
the date.

- [ ] **Step 4: Measure the result**

```bash
for f in DSCF6509 DSCF6383 DSCF6489; do
  cargo run --release -p app --example strip_vs_canvas ~/Desktop/fuji/$f.RAF
done
```

Baseline was **+0.39, +0.40, +0.11 EV**. Report the new figures, and re-run `curve_probe` to show
the binned ratio is now approximately **constant** — that is the acceptance criterion, not a zero
mean, because the residual level is the camera's exposure decision and belongs to Auto Tone.

- [ ] **Step 5: Produce the evidence and STOP**

Write before/after JPEGs for three frames to `/tmp/m54b-rendering/` and report. **Do not tune
further on your own judgement** — the "done when" is a judgement about a photograph, and per
`docs/PARALLEL-WORK.md` §6 that is the user's to make.

- [ ] **Step 6: Full gate and commit**

```bash
cargo fmt --all --check; cargo clippy --all-targets --all-features -- -D warnings; cargo test --all
git status --porcelain crates/pipeline/tests/golden/
```
Expected: all exit 0, **no golden modified** (the default is still `Rendering::None`).

```bash
git add -A crates
git commit -m "feat(pipeline): fit the built-in curve to measured camera data

Fitted against pooled frames spanning dark to bright rather than chosen by
eye. The acceptance is that the binned ratio against the camera becomes
approximately constant -- a constant ratio means the same shape at a
different exposure, and the exposure is Auto Tone's job."
```

---

## Self-Review

**Spec coverage.** §1 measurement → Task 8. §2 placement → Task 4. §3 curve + spline reuse →
Tasks 1 and 3. §4 model → Task 2 and Task 7. §5 profile curve → Tasks 5 and 6, identity rule in
Task 4. §6 RAW only → Task 6. §7 tests → distributed, each named. §8 acceptance → Task 8.
§9 caveats → no code.

**Placeholder scan.** None: every step carries its code or its exact command. The two "if it does
not exist" clauses (Task 4's `stage_names`, Task 7's `starting_edits`) each say precisely what to
add and why.

**Type consistency.** `Rendering::{None, Camera}`; `RENDERING_GROUP`; `DisplayRenderingStage::
{built_in, from_profile, map}`; `Profile.tone_curve: Option<Vec<[f32; 2]>>`; `Decoded.
{profile_curve, scene_referred}`; `Settings.default_rendering`; `thumbnail_edits`;
`starting_edits`. Consistent across tasks.

**Counts.** 1729 → roughly 1745. **No golden may change.**
