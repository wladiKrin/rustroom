# M5.2 White Balance — Kelvin + Tint Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `WhiteBalance` becomes `{ kelvin: Option<f32>, tint: f32 }` — an absolute instrument with an eyedropper and presets — serving RAW and non-RAW through one code path where only the matrix differs.

**Architecture:** New `color::wb` holds the Planckian/daylight locus and the inverse solve (Kelvin+tint → camera-space gains). The source's matrix reaches the pipeline as a **per-render parameter**, exactly as `RenderScale` and `Resampler` do — defaulting to sRGB primaries, so `pipeline` stays source-agnostic and every golden is unaffected. `WhiteBalanceStage` keeps taking gains and learns nothing new.

**Tech Stack:** Rust. Spec: `docs/superpowers/specs/2026-09-01-white-balance-kelvin-design.md`.

**Build environment:**
```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

**⚠⚠ Verification — use UNIQUE log filenames.** Agents and the orchestrator share `/tmp`, and on 2026-09-01 that produced two false readings that each looked like a catastrophe (a "901 passed" and a "1495"). Also: **macOS `awk` stops early on the log's UTF-8** and has reported "1470 passed, 0 failed" for a run that exited 101 with a compile error. **The exit code is the authority**, and count with Python:
```bash
L=/tmp/m52-$$-test.log
cargo fmt --all --check > /tmp/m52-$$-f.log 2>&1; echo "FMT=$?"
cargo clippy --all-targets --all-features -- -D warnings > /tmp/m52-$$-c.log 2>&1; echo "CLIPPY=$?"
cargo test --all > "$L" 2>&1; echo "TEST=$?"
python3 -c "
import re
p=f=0
for l in open('$L',errors='replace'):
    m=re.match(r'test result: \w+\. (\d+) passed; (\d+) failed',l)
    if m: p+=int(m.group(1)); f+=int(m.group(2))
print(f'passed={p} failed={f}')"
```
Baseline **1553 passed, 0 failed**, **51 goldens**.

**⚠ Sabotage discipline.** Break what a test names and watch it fail — and confirm it **ran** (`ran = passed + failed >= 1`), that the file **actually changed** (`cargo fmt` reflows code, so a text match can silently apply to nothing), and that it **compiled** (no `test result:` line means it did not).

**Conventions:** no `Co-Authored-By`; **never push**; commit per task.

---

## File Structure

| File | Responsibility |
| --- | --- |
| `crates/color/src/wb.rs` **(new)** | The locus and both directions. Pure, no I/O. |
| `crates/color/src/lib.rs` | `pub mod wb;` |
| `crates/edit-model/src/lib.rs` | `WhiteBalance { kelvin: Option<f32>, tint: f32 }` |
| `crates/pipeline/src/stages/white_balance.rs` | Takes gains; no longer computes them |
| `crates/pipeline/src/lib.rs` | `RenderSpec` params struct carrying the camera matrix |
| `crates/raw-decode/src/backend_rsraw.rs` | Expose `rgb_cam` as the shot's matrix |
| `crates/app/src/main.rs`, `session_flow.rs`, `decode.rs` | Carry the matrix decode → `Session` |
| `crates/app/src/develop/panel/adjustments.rs` | Kelvin slider, tint, presets |
| `crates/app/src/develop/canvas/mod.rs` | Eyedropper pick |
| `crates/pipeline/tests/golden.rs` | `warm_wb` re-authored |

---

## Task 1: The locus and the inverse solve

**Files:** Create `crates/color/src/wb.rs`; modify `crates/color/src/lib.rs`.

- [ ] **Step 1: Write the failing tests**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// ⚠⚠ **Cross-checked against PUBLISHED values, not against ourselves.** A self-consistent
    /// pair of wrong transforms passes a round trip happily — this is the only test here that
    /// can see a wrong locus. The CIE daylight locus at these temperatures is standard; D50 and
    /// D65 are the two everyone tabulates.
    #[test]
    fn the_locus_matches_published_chromaticities() {
        // D65: x = 0.31272, y = 0.32903. D50: x = 0.34567, y = 0.35850.
        let (x, y) = locus_xy(6504.0);
        assert!((x - 0.31272).abs() < 0.002, "D65 x was {x}");
        assert!((y - 0.32903).abs() < 0.002, "D65 y was {y}");
        let (x, y) = locus_xy(5003.0);
        assert!((x - 0.34567).abs() < 0.002, "D50 x was {x}");
        assert!((y - 0.35850).abs() < 0.002, "D50 y was {y}");
    }

    /// ⚠ Warmer must mean *more red*, and this is the direction that is easy to invert with
    /// nothing failing — every other test here is symmetric in it.
    #[test]
    fn a_lower_temperature_is_redder() {
        let (x_warm, _) = locus_xy(2800.0);
        let (x_cool, _) = locus_xy(9000.0);
        assert!(x_warm > x_cool, "2800K should sit redder than 9000K");
    }

    /// The identity that makes the whole feature work: asking for the temperature a source is
    /// already at must change nothing.
    #[test]
    fn the_baseline_temperature_gives_unit_gains() {
        let m = crate::spaces::SRGB_TO_XYZ;
        let g = gains_for(6504.0, 0.0, &m);
        for c in g {
            assert!((c - 1.0).abs() < 0.01, "gains were {g:?}");
        }
    }

    /// ⚠ Tint moves perpendicular to the locus, so at zero it must contribute nothing — or
    /// every Kelvin value would carry a hidden green cast.
    #[test]
    fn zero_tint_leaves_the_locus_untouched() {
        let (x, y) = with_tint(locus_xy(5500.0), 0.0);
        let (bx, by) = locus_xy(5500.0);
        assert!((x - bx).abs() < 1e-6 && (y - by).abs() < 1e-6);
    }

    /// ⚠ Gains are luminance-normalised, as today: a white-balance change moves colour, not
    /// brightness. Without this a warm setting also darkens the photograph.
    #[test]
    fn gains_preserve_luminance() {
        let m = crate::spaces::SRGB_TO_XYZ;
        for k in [2800.0, 5500.0, 9000.0] {
            let g = gains_for(k, 0.0, &m);
            let lum = crate::luminance(g);
            assert!((lum - 1.0).abs() < 0.01, "at {k}K luminance was {lum}");
        }
    }
}
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test -p color wb:: > /tmp/m52-$$-t1.log 2>&1; echo "EXIT=$?"
python3 -c "
import re
for l in open('/tmp/m52-$$-t1.log',errors='replace'):
    m=re.match(r'test result: \w+\. (\d+) passed; (\d+) failed',l)
    if m: print('ran=',int(m.group(1))+int(m.group(2)))"
```
Expected: compile errors — `wb` module not found.

- [ ] **Step 3: Write the module**

Create `crates/color/src/wb.rs`:

```rust
//! Kelvin + tint white balance: the daylight/Planckian locus, and the gains that reach it.
//!
//! ⚠⚠ **Two directions, and only one existed before this.** `camera::neutral_temperature` maps a
//! camera neutral **to** Kelvin — it is what gives a RAW its as-shot readout, and it works from
//! libraw's single `cam_xyz` by being passed the same matrix twice, since `blend(m, m, g)` is `m`
//! for any `g`. What a *slider* needs is the inverse, and that is this module.
//!
//! ⚠ The reference for a non-RAW file is its own white point. A JPEG has already been balanced by
//! the camera, so a neutral in it reads as equal RGB and its baseline **is** sRGB's D65. The same
//! code serves both; only the matrix differs.

/// Chromaticity of the CIE daylight locus at `kelvin`, clamped to the range the approximation is
/// defined over.
///
/// ⚠ The daylight locus, not the Planckian one. They diverge below ~4000 K, and every raw
/// converter's "temperature" slider follows daylight — matching the Planckian curve instead would
/// make our numbers disagree with every other tool at the tungsten end.
pub fn locus_xy(kelvin: f32) -> (f32, f32) {
    let t = kelvin.clamp(1667.0, 25000.0);
    let x = if t <= 7000.0 {
        -4.6070e9 / (t * t * t) + 2.9678e6 / (t * t) + 0.09911e3 / t + 0.244063
    } else {
        -2.0064e9 / (t * t * t) + 1.9018e6 / (t * t) + 0.24748e3 / t + 0.237040
    };
    let y = -3.000 * x * x + 2.870 * x - 0.275;
    (x, y)
}

/// Offset a chromaticity perpendicular to the locus. Positive `tint` is magenta, negative green.
///
/// ⚠ `TINT_SCALE` is the one tuning constant here. It sets what "+100 tint" means and is chosen so
/// the full range covers the casts real light actually produces without reaching absurdity.
pub fn with_tint((x, y): (f32, f32), tint: f32) -> (f32, f32) {
    const TINT_SCALE: f32 = 0.0006;
    // The locus slope at this point; the perpendicular is (-dy, dx) normalised.
    let dy_dx = -6.000 * x + 2.870;
    let n = (1.0 + dy_dx * dy_dx).sqrt();
    let (px, py) = (-dy_dx / n, 1.0 / n);
    (x + px * tint * TINT_SCALE, y + py * tint * TINT_SCALE)
}

/// Per-channel gains that make an illuminant of `kelvin`/`tint` render neutral, for a source whose
/// RGB→XYZ matrix is `rgb_to_xyz`.
///
/// ⚠ Luminance-normalised, as the previous relative model was: a white-balance change moves colour
/// and not brightness. `luminance` is called on the **gains as a pixel**, which is what it means.
pub fn gains_for(kelvin: f32, tint: f32, rgb_to_xyz: &[[f32; 3]; 3]) -> [f32; 3] {
    let (x, y) = with_tint(locus_xy(kelvin), tint);
    if y.abs() < 1e-6 {
        return [1.0, 1.0, 1.0];
    }
    // The illuminant as XYZ at unit luminance, then into the source's own RGB.
    let xyz = [x / y, 1.0, (1.0 - x - y) / y];
    let Some(inv) = crate::spaces::invert(rgb_to_xyz) else {
        return [1.0, 1.0, 1.0];
    };
    let rgb = crate::spaces::apply(&inv, xyz);
    // ⚠ A channel at or below zero means the illuminant is outside this source's gamut; leaving it
    // would produce a negative gain and invert that channel across the whole photograph.
    let mut gains = [0.0f32; 3];
    for i in 0..3 {
        gains[i] = if rgb[i] > 1e-6 { 1.0 / rgb[i] } else { 1.0 };
    }
    let lum = crate::luminance(gains);
    if lum > 1e-6 {
        for c in &mut gains {
            *c /= lum;
        }
    }
    gains
}
```

Add `pub mod wb;` to `crates/color/src/lib.rs`.

⚠⚠ **Two of the three helpers are not where you would expect, and I checked rather than assumed:**
`apply` is in `color::spaces`, but **`invert` is in `color::camera`** (line 116, `pub`, used by
`neutral_temperature`), and **`SRGB_TO_XYZ` does not exist at all** — `spaces` has
`XYZ_D50_TO_PROPHOTO` and the RGB↔RGB matrices, but no sRGB→XYZ.

So `wb.rs` uses `crate::camera::invert` and `crate::spaces::apply`, and **adds `SRGB_TO_XYZ` to
`spaces`**, derived from the sRGB primaries and the D65 white point:

```rust
/// Linear sRGB → XYZ (D65).
///
/// ⚠ Derived from the sRGB primaries and the D65 white point, then verified against the standard
/// values rather than transcribed — the same treatment the ProPhoto pair got, which is what caught
/// a 1e-4 rounding difference there and proved it was rounding rather than error.
pub const SRGB_TO_XYZ: [[f32; 3]; 3] = [
    [0.4123908, 0.3575843, 0.1804808],
    [0.2126390, 0.7151687, 0.0721923],
    [0.0193308, 0.1191948, 0.9505322],
];
```

with a test beside the other matrix tests:

```rust
/// ⚠ White is the one input that catches a transposed or mis-scaled matrix, because every row
/// contributes. D65's XYZ is (0.95047, 1.0, 1.08883).
#[test]
fn srgb_white_maps_to_d65() {
    let w = apply(&SRGB_TO_XYZ, [1.0, 1.0, 1.0]);
    assert!((w[0] - 0.95047).abs() < 1e-3, "X was {}", w[0]);
    assert!((w[1] - 1.0).abs() < 1e-3, "Y was {}", w[1]);
    assert!((w[2] - 1.08883).abs() < 1e-3, "Z was {}", w[2]);
}
```

⚠ Its middle row **is** Rec.709's luminance weights, which is not a coincidence and is why
`gains_preserve_luminance` can use `crate::luminance` as its check.

- [ ] **Step 4: Run the tests**

Expected: PASS, 5 new tests.

- [ ] **Step 5: Sabotage**

Change `locus_xy`'s `y` polynomial constant `2.870` to `2.0`. Expected: `the_locus_matches_published_chromaticities` **fails**. Confirm it ran, and that the file changed. Restore.

- [ ] **Step 6: Commit**

```bash
git add crates/color
git commit -m "feat(color): the daylight locus and Kelvin-to-gains

The forward direction existed -- camera::neutral_temperature, built for
DCP interpolation, which works from libraw's single matrix by being passed
the same one twice. The inverse is what a slider needs and existed
nowhere. Cross-checked against published D50 and D65 chromaticities rather
than only against itself: a self-consistent pair of wrong transforms
passes a round trip happily."
```

---

## Task 2: The model, and the stage takes gains

**Files:** `crates/edit-model/src/lib.rs`; `crates/pipeline/src/stages/white_balance.rs`.

- [ ] **Step 1: Write the failing tests**

In `edit-model`:

```rust
/// ⚠⚠ `None` means the file's own baseline — the as-shot temperature for a RAW, D65 for a JPEG.
/// That is what lets `Edits::default()` stay neutral **without knowing which kind of file it
/// describes**, which is the rule the entire golden harness rests on.
#[test]
fn the_default_white_balance_is_the_files_own_baseline() {
    let wb = WhiteBalance::default();
    assert_eq!(wb.kelvin, None);
    assert_eq!(wb.tint, 0.0);
    assert!(!wb.is_active());
}

/// ⚠ A legacy sidecar carries `temp`, which serde ignores as an unknown field. All 49 real copies
/// store 0.0, so nothing is lost; a non-zero one is dropped, and that is the honest cost of a
/// value that never had a Kelvin meaning.
#[test]
fn a_sidecar_written_before_kelvin_still_loads_as_the_baseline() {
    let wb: WhiteBalance = serde_json::from_str(r#"{"temp":0.0,"tint":0.0}"#).expect("parses");
    assert_eq!(wb.kelvin, None);
    assert_eq!(wb.tint, 0.0);
}
```

In `pipeline/src/stages/white_balance.rs`:

```rust
/// ⚠ Unit gains must be **bit-exactly** the identity, or a neutral photo is not neutral and every
/// golden moves.
#[test]
fn unit_gains_are_exactly_the_identity() {
    let s = WhiteBalanceStage::new([1.0, 1.0, 1.0]);
    let img = ImageBuf::from_fn(2, 2, |x, y| [0.25 + x as f32, 0.5, 0.75 + y as f32]);
    let out = s.apply(img.clone());
    assert_eq!(out.pixels(), img.pixels());
}
```

- [ ] **Step 2: Run to verify they fail**

Expected: `no field kelvin`, and `WhiteBalanceStage::new` takes a `&WhiteBalance`.

- [ ] **Step 3: Change the model**

In `crates/edit-model/src/lib.rs`, replace `WhiteBalance`:

```rust
/// White balance as an absolute instrument: the illuminant declared for the scene.
///
/// ⚠⚠ `kelvin: None` is **the file's own baseline** — the as-shot temperature for a RAW, D65 for a
/// JPEG, which has already been balanced by the camera. That is what keeps `Edits::default()`
/// neutral without this type knowing which kind of file it describes.
///
/// ⚠ This replaces a relative `temp`/`tint` pair whose coefficients were chosen by hand, and which
/// therefore described a *shift* rather than a *target* — so a white balance copied between two
/// photos meant nothing. A legacy `temp` key is ignored by serde; every real sidecar stored 0.0.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct WhiteBalance {
    /// Kelvin. `None` = the file's baseline.
    pub kelvin: Option<f32>,
    /// Green–magenta, perpendicular to the locus. 0 = none.
    pub tint: f32,
}

impl WhiteBalance {
    /// True when this changes pixels. The rule `NoiseReduction::is_active` established.
    pub fn is_active(&self) -> bool {
        self.kelvin.is_some() || self.tint != 0.0
    }
}
```

- [ ] **Step 4: The stage takes gains**

Replace `WhiteBalanceStage::new`:

```rust
impl WhiteBalanceStage {
    /// ⚠⚠ Takes **gains**, not a `WhiteBalance`. Converting Kelvin to gains needs the source's
    /// RGB→XYZ matrix, and `pipeline` is source-agnostic by rule — a JPEG has no camera matrix,
    /// and the decoder is the only thing that knows which camera took the picture. The conversion
    /// therefore happens in `color::wb`, called by whoever builds the pipeline.
    pub fn new(gains: [f32; 3]) -> Self {
        Self { gains }
    }
}
```

⚠ The luminance normalisation moves into `color::wb::gains_for` (Task 1) — it must not happen twice.

- [ ] **Step 5: Fix the fallout with the compiler**

```bash
cargo build --all 2>&1 | grep -E "^error" -A 3 | head -40
```
Fix each site the compiler names. ⚠ Several test fixtures construct `WhiteBalance { temp, tint }`; those are `kelvin: Some(..)` now, and **choose a Kelvin deliberately** rather than translating the old number, which had no Kelvin meaning.

- [ ] **Step 6: Run the tests**

⚠ `golden_warm_wb` **will fail**. That is expected and is Task 7; leave it failing for now and note it.

- [ ] **Step 7: Commit**

```bash
git add -A crates
git commit -m "feat(edit-model): white balance is Kelvin and tint

kelvin: None means the file's own baseline, which is what keeps
Edits::default() neutral without the type knowing whether it describes a
RAW or a JPEG. The stage takes gains now, because converting Kelvin to
gains needs the source's matrix and pipeline is source-agnostic by rule."
```

---

## Task 3: `RenderSpec`, and the matrix reaches the pipeline

**Files:** `crates/pipeline/src/lib.rs`.

- [ ] **Step 1: Write the failing test**

```rust
/// ⚠ The sRGB default must leave every existing render untouched — that is what proves a new
/// per-render parameter is inert when nobody passes one.
#[test]
fn the_default_render_spec_uses_srgb_primaries() {
    let s = RenderSpec::default();
    assert_eq!(s.camera, color::spaces::SRGB_TO_XYZ);
}

/// ⚠⚠ The same Kelvin gives *different* gains on a camera matrix than on sRGB. This is the caveat
/// in the spec asserted rather than discovered — and it is the thing most likely to be "fixed" by
/// someone later who thinks the two should agree.
#[test]
fn the_same_kelvin_differs_between_a_camera_and_srgb() {
    let cam = [[0.6, 0.3, 0.1], [0.2, 0.7, 0.1], [0.0, 0.1, 0.9]];
    let a = color::wb::gains_for(5000.0, 0.0, &color::spaces::SRGB_TO_XYZ);
    let b = color::wb::gains_for(5000.0, 0.0, &cam);
    assert!(
        (a[0] - b[0]).abs() > 0.01,
        "a camera matrix must not give sRGB's gains: {a:?} vs {b:?}"
    );
}
```

- [ ] **Step 2: Run to verify it fails**

Expected: `RenderSpec` not found.

- [ ] **Step 3: Introduce the struct**

⚠⚠ `build_pipeline_split_in_region_with_brush` already takes **seven** arguments, so an eighth trips clippy's `too_many_arguments`. This project has met that twice — `FrameSpec` for `render_cached`, `NameField` for `name_edit::field` — and both times recorded that **clippy was right, not a lint to silence**. This is the third instance.

```rust
/// The per-render parameters that are not part of `Edits`.
///
/// ⚠ A struct because the function reached eight arguments and clippy objected — the same answer
/// `FrameSpec` and `NameField` gave. The body destructures it on its first line, so it reads as
/// before.
///
/// ⚠⚠ `camera` is the **source's** RGB→XYZ matrix, and its default of sRGB primaries is what keeps
/// `pipeline` source-agnostic: a JPEG genuinely *is* sRGB, so the default is not a fallback but the
/// right answer for every non-RAW file.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderSpec {
    pub space: color::OutputSpace,
    pub resampler: Resampler,
    pub camera: [[f32; 3]; 3],
}

impl Default for RenderSpec {
    fn default() -> Self {
        Self {
            space: color::OutputSpace::Srgb,
            resampler: Resampler::default(),
            camera: color::spaces::SRGB_TO_XYZ,
        }
    }
}
```

Replace `space` and `resampler` in `build_pipeline_split_in_region_with_brush` with `spec: RenderSpec`, keeping `lens` separate (it is `Option` and shot-specific). Build the stage with:

```rust
    if edits.white_balance.is_active() {
        let wb = &edits.white_balance;
        // ⚠ `None` is the baseline, which for any source is "no change" — so no stage at all.
        if let Some(k) = wb.kelvin {
            tail.push(Box::new(WhiteBalanceStage::new(color::wb::gains_for(
                k,
                wb.tint,
                &spec.camera,
            ))));
        } else if wb.tint != 0.0 {
            // Tint alone still needs a reference temperature: the source's baseline.
            tail.push(Box::new(WhiteBalanceStage::new(color::wb::gains_for(
                BASELINE_KELVIN,
                wb.tint,
                &spec.camera,
            ))));
        }
    }
```

with `const BASELINE_KELVIN: f32 = 6504.0;` and a comment that it is D65, the white point of both sRGB and the working space.

- [ ] **Step 4: Update the five call sites**

`crates/pipeline/tests/golden.rs:310` and `:1352`, `crates/rustroom/src/main.rs:213`, `crates/app/src/batch_export.rs:336`, `crates/app/src/render_engine.rs:678`. ⚠ Goldens and the CLI pass `RenderSpec::default()` with only `space`/`resampler` overridden — **they must not pass a camera matrix**, or the goldens move.

- [ ] **Step 5: Run the tests**

Expected: all pass except `golden_warm_wb` (Task 7). **Every other golden must be byte-identical** — that is the proof the parameter is inert.

- [ ] **Step 6: Sabotage**

Change `RenderSpec::default`'s `camera` to the synthetic matrix from Step 1. Expected: **many goldens fail**. Restore.

- [ ] **Step 7: Commit**

```bash
git add -A crates
git commit -m "feat(pipeline): RenderSpec carries the source's matrix

An eighth argument tripped too_many_arguments, and clippy was right --
the third time this project has answered that with a params struct, after
FrameSpec and NameField.

The camera matrix defaults to sRGB primaries, which keeps pipeline
source-agnostic: a JPEG genuinely is sRGB, so the default is the right
answer rather than a fallback."
```

---

## Task 4: Carry the camera matrix from the decode

**Files:** `crates/raw-decode/src/backend_rsraw.rs`, `crates/raw-decode/src/lib.rs`, `crates/app/src/decode.rs`, `crates/app/src/main.rs`, `crates/app/src/session_flow.rs`, `crates/app/src/render_engine.rs`.

- [ ] **Step 1: Write the failing test**

In `raw-decode`, gated on the fixture as its siblings are:

```rust
/// ⚠ The matrix must be the one libraw actually used, read **after** `dcraw_process` — it is
/// computed there, so asking earlier returns nothing useful. That is the same ordering
/// `camera_native` already depends on.
#[test]
fn a_raw_decode_reports_its_camera_matrix() {
    let Some(p) = fixture() else { return };
    let d = decode(&p, DecodeOptions::linear_scene()).expect("decodes");
    let m = d.camera_rgb_to_xyz.expect("a RAW knows its matrix");
    // Every row of a sane RGB->XYZ matrix sums to something positive and finite.
    for row in m {
        let s: f32 = row.iter().sum();
        assert!(s.is_finite() && s > 0.0, "bad row {row:?}");
    }
}
```

- [ ] **Step 2: Run to verify it fails**

⚠ Requires `RAW_FIXTURE`; without it the test **skips** and proves nothing. Set it:
```bash
export RAW_FIXTURE=~/Desktop/fuji/DSCF6509.RAF
```

- [ ] **Step 3: Expose it**

Add `camera_rgb_to_xyz: Option<[[f32; 3]; 3]>` to `raw-decode`'s decoded output, filled from libraw's `rgb_cam` composed with sRGB→XYZ (C1 established that `rgb_cam` is camera → linear **sRGB**, already row-normalised). `image-decode` leaves it `None`.

- [ ] **Step 4: Thread it to `Session`**

`Decoded` gains the field; `Session` stores it; `Session::render_spec()` returns a `RenderSpec` with `camera` set from it or `SRGB_TO_XYZ` when `None`. ⚠ Every render request goes through `Session::request`, so this is **one** place — the rule `view_edits` already follows.

- [ ] **Step 5: Run the tests, then commit**

```bash
git add -A crates
git commit -m "feat(raw-decode): a decode reports its camera matrix

Threaded to Session so every render request carries it through one place,
the rule view_edits already follows. image-decode leaves it None and the
sRGB default applies, which is the right answer for a JPEG rather than a
fallback."
```

---

## Task 5: The panel

**Files:** `crates/app/src/develop/panel/adjustments.rs`.

- [ ] **Step 1: Write the failing test**

```rust
/// ⚠ "As Shot" must store `None`, never the numeric baseline. `None` keeps the photo *following*
/// its file; freezing the number would strand it if the profile or decode ever changed — the rule
/// an unlabelled virtual copy already follows.
#[test]
fn as_shot_stores_none_rather_than_the_baseline_number() {
    let mut wb = edit_model::WhiteBalance { kelvin: Some(4200.0), tint: 5.0 };
    apply_wb_preset(&mut wb, WbPreset::AsShot);
    assert_eq!(wb.kelvin, None);
    assert_eq!(wb.tint, 0.0, "As Shot clears the tint too");
}

/// ⚠ Every preset must name a temperature, or one silently does nothing.
#[test]
fn every_preset_sets_a_temperature_except_as_shot() {
    for p in WbPreset::ALL {
        let mut wb = edit_model::WhiteBalance::default();
        apply_wb_preset(&mut wb, p);
        match p {
            WbPreset::AsShot => assert_eq!(wb.kelvin, None),
            _ => assert!(wb.kelvin.is_some(), "{p:?} set no temperature"),
        }
    }
}
```

- [ ] **Step 2: Run to verify it fails, then implement**

`WbPreset { AsShot, Daylight, Cloudy, Shade, Tungsten, Fluorescent, Flash }` with `ALL`, and `apply_wb_preset`. Conventional values: Daylight 5500, Cloudy 6500, Shade 7500, Tungsten 2850, Fluorescent 3800, Flash 5500.

⚠ **"Auto" is deliberately omitted** — the spec marks it as the one item that can be cut, because it is a real analysis pass and not a preset value.

Draw a Kelvin slider (2000–12000, logarithmic feel via a mired-linear mapping if it reads better) shown only when `kelvin.is_some()`, a tint slider, and the preset buttons.

- [ ] **Step 3: Run, sabotage (make `AsShot` store `Some(6504.0)` — the first test must fail), commit**

```bash
git add -A crates/app
git commit -m "feat(develop): a Kelvin white balance in the panel

As Shot stores None rather than the baseline number, so the photo keeps
following its file rather than freezing a value that would strand it."
```

---

## Task 6: The eyedropper

**Files:** `crates/app/src/develop/canvas/mod.rs`, plus a lib module for the solve.

- [ ] **Step 1: Write the failing test**

```rust
/// ⚠⚠ The round trip that makes the eyedropper meaningful: picking a pixel that is *already*
/// neutral must return the source's baseline, so clicking a grey card on a correctly balanced
/// photo changes nothing.
#[test]
fn picking_an_already_neutral_pixel_returns_the_baseline() {
    let m = color::spaces::SRGB_TO_XYZ;
    let (k, t) = solve_pick([0.5, 0.5, 0.5], &m);
    assert!((k - 6504.0).abs() < 300.0, "got {k}K");
    assert!(t.abs() < 5.0, "got tint {t}");
}
```

- [ ] **Step 2: Implement**

`solve_pick(rgb, matrix) -> (kelvin, tint)`: convert the picked RGB to XYZ, take its chromaticity, find the nearest point on the locus (the Kelvin), and the perpendicular distance (the tint). ⚠ Read the **pre-white-balance** pixel, or the eyedropper chases its own correction — the same trap the luminance range mask keys on `base` to avoid.

- [ ] **Step 3: Wire a canvas pick**, run, sabotage (return the picked chromaticity rather than the nearest locus point — the test must fail), commit.

---

## Task 7: Re-author `golden_warm_wb`

**Files:** `crates/pipeline/tests/golden.rs`, `crates/pipeline/tests/golden/warm_wb.png`.

- [ ] **Step 1: Choose a Kelvin deliberately**

⚠⚠ **Re-author, do not regenerate.** Its `temp: 50` has no Kelvin equivalent, so regenerating would bake whatever the new code happens to do into the reference. Set `kelvin: Some(3200.0)` — clearly warm against the 6504 K baseline, and inside the range a photograph is actually shot at.

- [ ] **Step 2: Regenerate only that one, and measure it**

```bash
UPDATE_GOLDEN=1 cargo test -p pipeline --test golden golden_warm_wb > /tmp/m52-$$-g.log 2>&1; echo "EXIT=$?"
git status --porcelain crates/pipeline/tests/golden/
```
Expected: **exactly one** file changed. ⚠ If any other golden moved, the `RenderSpec` default is not inert — stop and find out why.

- [ ] **Step 3: Check it discriminates**

⚠ A golden that clips cannot tell a strong effect from a stronger one — `CLAUDE.md` records a mask golden that pinned 15 of 64 samples at 255 and was blind. Count pinned samples; if any channel clips, lower the effect until none does.

- [ ] **Step 4: Full gate and commit**

Run the full verification block from the header. Expected `FMT=0 CLIPPY=0 TEST=0`, and **exactly one** golden changed since the start of this plan.

```bash
git add -A crates/pipeline
git commit -m "test(golden): warm_wb re-authored at 3200K

Its temp: 50 had no Kelvin equivalent, so regenerating would have baked
whatever the new code happens to do into the reference. 3200K is clearly
warm against the 6504K baseline and inside the range photographs are
actually shot at."
```

---

## Self-Review

**Spec coverage:** §1 model → Task 2. §1 non-RAW reference → Task 3's sRGB default. §2 maths → Task 1. §3 where the conversion lives → Tasks 3 and 4. §4 UI → Tasks 5 and 6; **"Auto" is deliberately dropped**, as §4 permits. §5 migration → Task 2 Step 1's legacy test; `warm_wb` → Task 7. §6 tests 1–5 → Task 1 (locus vs published, round trip), Task 2 (`None` exactly neutral), Task 3 (sRGB default inert, RAW vs sRGB differ). §7 caveats → no code.

**Placeholder scan:** none. The one conditional in an earlier draft — "`SRGB_TO_XYZ` may not exist" — was **checked and resolved**: it does not exist, `invert` is in `camera` rather than `spaces`, and Task 1 Step 3 now carries the actual constant and its test rather than an instruction to go and find out.

**Type consistency:** `WhiteBalance { kelvin: Option<f32>, tint: f32 }`; `color::wb::{locus_xy, with_tint, gains_for}`; `RenderSpec { space, resampler, camera }`; `WhiteBalanceStage::new([f32; 3])`; `WbPreset::ALL` + `apply_wb_preset`; `solve_pick`. Consistent across tasks.

**Counts:** 1553 → roughly 1570; the exact number depends on how many fixture sites Task 2 Step 5 touches. **Exactly one golden may change.**
