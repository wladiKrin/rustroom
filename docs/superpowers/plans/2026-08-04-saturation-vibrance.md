# Saturation & Vibrance Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add saturation and vibrance to the develop pipeline — one new `PipelineStage`, two new `Edits` fields, two sliders in the GUI, unit + golden test coverage.

**Architecture:** Both parameters are chroma scaling around the same Rec.709 luma axis in the linear working space, so a single `SaturationStage` computes one combined per-pixel factor `f = f_sat * f_vib` and applies `out = luma + (rgb - luma) * f`. The stage sits after `ToneCurveStage` and before `OutputTransformSrgb`, and `build_pipeline` only adds it when either parameter is non-zero. Nothing in any existing stage changes.

**Tech Stack:** Rust workspace; `core-image` (`ImageBuf::map_pixels`), `edit-model` (serde), `pipeline` (`PipelineStage`), `app` (egui 0.35). Tests: `cargo test` unit tests plus the committed-PNG golden harness in `crates/pipeline/tests/golden.rs`.

**Spec:** `docs/superpowers/specs/2026-08-04-saturation-vibrance-design.md`

**Build environment (macOS — needed for any command that touches `raw-decode`, i.e. all `--all` builds):**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:$PKG_CONFIG_PATH"
```

---

## File Structure

| File | Change | Responsibility |
|---|---|---|
| `crates/edit-model/src/lib.rs` | Modify | Add `saturation` / `vibrance` to `Edits` (serde-defaulted, neutral at 0) |
| `crates/pipeline/src/stages/saturation.rs` | Create | The `SaturationStage` math + its unit tests |
| `crates/pipeline/src/stages/mod.rs` | Modify | Register the module |
| `crates/pipeline/src/lib.rs` | Modify | Re-export `SaturationStage`; insert it in `build_pipeline` |
| `crates/pipeline/tests/golden.rs` | Modify | `check_golden_with` + `muted_chart` + two new golden cases |
| `crates/pipeline/tests/golden/saturation_plus50.png` | Create | Golden reference (generated) |
| `crates/pipeline/tests/golden/vibrance_plus50.png` | Create | Golden reference (generated) |
| `crates/app/src/main.rs` | Modify | Two sliders in the Develop right panel |
| `CLAUDE.md` | Modify | Status / crate-layout notes |

---

### Task 1: `Edits` gains `saturation` and `vibrance`

**Files:**
- Modify: `crates/edit-model/src/lib.rs` (the `Edits` struct, and its `mod tests`)

- [ ] **Step 1: Write the failing test**

Add to `mod tests` at the bottom of `crates/edit-model/src/lib.rs`:

```rust
    #[test]
    fn color_fields_default_to_neutral() {
        let e = Edits::default();
        assert_eq!(e.saturation, 0.0);
        assert_eq!(e.vibrance, 0.0);
    }

    #[test]
    fn old_sidecar_without_color_fields_still_loads() {
        let e: Edits = serde_json::from_str(r#"{"exposure": 0.5, "contrast": 10.0}"#).unwrap();
        assert_eq!(e.exposure, 0.5);
        assert_eq!(e.saturation, 0.0);
        assert_eq!(e.vibrance, 0.0);
    }

    #[test]
    fn color_fields_roundtrip_json() {
        let e = Edits {
            saturation: 25.0,
            vibrance: -40.0,
            ..Default::default()
        };
        let json = serde_json::to_string(&e).unwrap();
        let back: Edits = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p edit-model`
Expected: FAIL — compile error, `struct Edits has no field named saturation`.

- [ ] **Step 3: Add the fields**

In `crates/edit-model/src/lib.rs`, inside `pub struct Edits`, insert these two fields **after `pub tone_curve: ToneCurve,` and before `pub geometry: Geometry,`** (mirrors pipeline order — color comes after tone):

```rust
    /// Saturation, ~[-100, 100]. -100 = monochrome, 0 = no change, +100 = 2x chroma.
    pub saturation: f32,
    /// Vibrance, ~[-100, 100]. Like saturation, but weighted toward already-muted colors.
    pub vibrance: f32,
```

No `#[serde(...)]` attributes are needed on the fields: `Edits` already carries struct-level `#[serde(default)]`, and `f32`'s default is `0.0`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p edit-model`
Expected: PASS — all tests in the crate green, including the pre-existing ones.

- [ ] **Step 5: Commit**

```bash
git add crates/edit-model/src/lib.rs
git commit -m "feat(edit-model): saturation + vibrance fields on Edits"
```

---

### Task 2: `SaturationStage`

**Files:**
- Create: `crates/pipeline/src/stages/saturation.rs`
- Modify: `crates/pipeline/src/stages/mod.rs`

**Reference for style:** `crates/pipeline/src/stages/contrast.rs` — same shape (constructor converts the ~[-100,100] UI amount into math coefficients; `apply` is one `map_*` call; unit tests live in the same file).

- [ ] **Step 1: Write the failing test**

Create `crates/pipeline/src/stages/saturation.rs` containing **only** the test module and the imports, so the tests compile against a not-yet-written type:

```rust
use crate::PipelineStage;
use core_image::ImageBuf;

#[cfg(test)]
mod tests {
    use super::*;

    /// Rec.709 luma of a linear RGB triple — the value chroma is scaled around.
    fn luma(px: [f32; 3]) -> f32 {
        0.2126 * px[0] + 0.7152 * px[1] + 0.0722 * px[2]
    }

    fn chroma(px: [f32; 3]) -> f32 {
        px[0].max(px[1]).max(px[2]) - px[0].min(px[1]).min(px[2])
    }

    fn apply_one(sat: f32, vib: f32, px: [f32; 3]) -> [f32; 3] {
        let mut img = ImageBuf::new(1, 1);
        img.set_pixel(0, 0, px);
        SaturationStage::new(sat, vib).apply(img).pixel(0, 0)
    }

    #[test]
    fn zero_is_identity() {
        let px = [0.4, 0.2, 0.1];
        let out = apply_one(0.0, 0.0, px);
        for i in 0..3 {
            assert!((out[i] - px[i]).abs() < 1e-6, "channel {i}: {out:?} vs {px:?}");
        }
    }

    #[test]
    fn minus_100_is_monochrome_preserving_luma() {
        let px = [0.4, 0.2, 0.1];
        let out = apply_one(-100.0, 0.0, px);
        let l = luma(px);
        for i in 0..3 {
            assert!((out[i] - l).abs() < 1e-6, "channel {i}: {out:?} vs luma {l}");
        }
    }

    #[test]
    fn neutral_pixel_is_unchanged() {
        let px = [0.3, 0.3, 0.3];
        let out = apply_one(80.0, 60.0, px);
        for i in 0..3 {
            assert!((out[i] - px[i]).abs() < 1e-6, "channel {i}: {out:?}");
        }
    }

    #[test]
    fn positive_saturation_widens_chroma() {
        let px = [0.4, 0.2, 0.1];
        let out = apply_one(50.0, 0.0, px);
        assert!(chroma(out) > chroma(px) * 1.4, "{out:?}");
    }

    #[test]
    fn vibrance_favours_muted_pixels() {
        // Muted: chroma/max = 0.2. Saturated: chroma/max = 1.0 (vibrance is a no-op there).
        let muted = [0.5, 0.45, 0.4];
        let saturated = [0.5, 0.05, 0.0];
        let muted_gain = chroma(apply_one(0.0, 50.0, muted)) / chroma(muted);
        let saturated_gain = chroma(apply_one(0.0, 50.0, saturated)) / chroma(saturated);
        assert!(
            muted_gain > saturated_gain + 0.2,
            "muted {muted_gain} should beat saturated {saturated_gain}"
        );
    }

    #[test]
    fn never_emits_negative_samples() {
        let out = apply_one(100.0, 100.0, [0.02, 0.01, 0.0]);
        assert!(out.iter().all(|&v| v >= 0.0), "{out:?}");
    }
}
```

Also register the module — in `crates/pipeline/src/stages/mod.rs`, add this line so the list stays alphabetical (after `pub mod output_transform;`):

```rust
pub mod saturation;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p pipeline saturation`
Expected: FAIL — compile error, `cannot find type SaturationStage in this scope`.

- [ ] **Step 3: Write the implementation**

In `crates/pipeline/src/stages/saturation.rs`, insert this **above** the `#[cfg(test)] mod tests` block (keeping the two existing `use` lines at the top of the file):

```rust
/// Rec.709 luma weights, applied to linear RGB.
const LUMA_R: f32 = 0.2126;
const LUMA_G: f32 = 0.7152;
const LUMA_B: f32 = 0.0722;

/// Guards the chroma ratio against division by zero on black pixels.
const EPS: f32 = 1e-6;

/// Chroma scaling around the luma axis: a uniform part (`saturation`) times a part
/// weighted toward already-muted pixels (`vibrance`).
///
/// `out = luma + (rgb - luma) * f_sat * f_vib`, clamped at 0.
pub struct SaturationStage {
    /// 1 + saturation/100: 0 = monochrome, 1 = neutral, 2 = double chroma.
    f_sat: f32,
    /// vibrance/100, applied as `1 + vib * (1 - chroma)`.
    vib: f32,
}

impl SaturationStage {
    /// Both amounts ~[-100, 100]; `(0, 0)` is the identity.
    pub fn new(saturation: f32, vibrance: f32) -> Self {
        Self {
            f_sat: 1.0 + saturation / 100.0,
            vib: vibrance / 100.0,
        }
    }
}

impl PipelineStage for SaturationStage {
    fn name(&self) -> &'static str {
        "saturation"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        let (f_sat, vib) = (self.f_sat, self.vib);
        image.map_pixels(|[r, g, b]| {
            let luma = LUMA_R * r + LUMA_G * g + LUMA_B * b;
            let max = r.max(g).max(b);
            let min = r.min(g).min(b);
            // Current chroma in [0, 1]; 0 for neutrals, 1 for a fully saturated color.
            let chroma = ((max - min) / max.max(EPS)).clamp(0.0, 1.0);
            let f = f_sat * (1.0 + vib * (1.0 - chroma));
            [
                (luma + (r - luma) * f).max(0.0),
                (luma + (g - luma) * f).max(0.0),
                (luma + (b - luma) * f).max(0.0),
            ]
        })
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p pipeline saturation`
Expected: PASS — 6 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/pipeline/src/stages/saturation.rs crates/pipeline/src/stages/mod.rs
git commit -m "feat(pipeline): SaturationStage — luma-weighted saturation + vibrance"
```

---

### Task 3: Wire the stage into `build_pipeline`

**Files:**
- Modify: `crates/pipeline/src/lib.rs` (re-export list, `build_pipeline`, `mod tests`)

- [ ] **Step 1: Write the failing test**

Add to `mod tests` in `crates/pipeline/src/lib.rs`:

```rust
    #[test]
    fn saturation_adds_one_stage() {
        let edits = Edits {
            saturation: 20.0,
            ..Edits::default()
        };
        assert_eq!(build_pipeline(&edits).len(), 2);
    }

    #[test]
    fn vibrance_alone_adds_one_stage() {
        let edits = Edits {
            vibrance: -30.0,
            ..Edits::default()
        };
        assert_eq!(build_pipeline(&edits).len(), 2);
    }

    #[test]
    fn neutral_color_adds_no_stage() {
        let edits = Edits {
            exposure: 1.0,
            ..Edits::default()
        };
        assert_eq!(build_pipeline(&edits).len(), 2);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p pipeline --lib`
Expected: FAIL — `saturation_adds_one_stage` and `vibrance_alone_adds_one_stage` fail with `assertion left == right failed: 1 != 2` (the stage isn't built yet). `neutral_color_adds_no_stage` passes already; it is the guard that the new stage stays opt-in.

- [ ] **Step 3: Write the implementation**

In `crates/pipeline/src/lib.rs`:

1. Add the re-export next to the others (keeps the list alphabetical, after the `output_transform` line):

```rust
pub use stages::saturation::SaturationStage;
```

2. In `build_pipeline`, insert this block **after** the `tone_curve` block and **before** `stages.push(Box::new(OutputTransformSrgb));`:

```rust
    if edits.saturation != 0.0 || edits.vibrance != 0.0 {
        stages.push(Box::new(SaturationStage::new(
            edits.saturation,
            edits.vibrance,
        )));
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p pipeline --lib`
Expected: PASS — all lib tests green.

- [ ] **Step 5: Commit**

```bash
git add crates/pipeline/src/lib.rs
git commit -m "feat(pipeline): build saturation stage after tone curve when non-neutral"
```

---

### Task 4: Golden coverage (existing references must stay byte-identical)

**Files:**
- Modify: `crates/pipeline/tests/golden.rs`
- Create: `crates/pipeline/tests/golden/saturation_plus50.png`, `crates/pipeline/tests/golden/vibrance_plus50.png` (generated, then committed)

- [ ] **Step 1: Refactor the harness to accept a chart, and add the muted chart + the two cases**

In `crates/pipeline/tests/golden.rs`:

1. Add this function right after the existing `chart()` function:

```rust
/// Deterministic linear chart of *muted* colors: 4 bands, each a 0..1 ramp with a weak
/// color cast (chroma/max = 0.2), so vibrance actually does something. The main `chart()`
/// is all pure primaries (chroma 1) and neutral gray (chroma 0), where vibrance is a no-op.
fn muted_chart() -> ImageBuf {
    let mut img = ImageBuf::new(W, H);
    for y in 0..H {
        let band = y * 4 / H; // 0..3
        let tint = match band {
            0 => [1.0, 0.9, 0.8],  // warm
            1 => [0.8, 1.0, 0.9],  // green
            2 => [0.8, 0.9, 1.0],  // cool
            _ => [1.0, 0.8, 0.95], // magenta
        };
        for x in 0..W {
            let v = x as f32 / (W - 1) as f32;
            img.set_pixel(x, y, [v * tint[0], v * tint[1], v * tint[2]]);
        }
    }
    img
}
```

2. Replace the existing `check_golden` function with this pair. The body is the old body verbatim, except that the chart comes in as an argument:

```rust
fn check_golden(name: &str, edits: &Edits) {
    check_golden_with(name, edits, chart());
}

fn check_golden_with(name: &str, edits: &Edits, input: ImageBuf) {
    let rendered = build_pipeline(edits).run(input);
    let (rw, rh) = (rendered.width() as u32, rendered.height() as u32);
    let bytes = to_rgb8(&rendered);
    let path = golden_path(name);

    if std::env::var("UPDATE_GOLDEN").is_ok() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        image::save_buffer(&path, &bytes, rw, rh, image::ColorType::Rgb8).unwrap();
        return;
    }

    let golden = image::open(&path)
        .unwrap_or_else(|_| panic!("missing golden {name}; run with UPDATE_GOLDEN=1"))
        .to_rgb8();
    assert_eq!(golden.dimensions(), (rw, rh), "{name}: size");
    let gbytes = golden.into_raw();
    let max_diff = bytes
        .iter()
        .zip(&gbytes)
        .map(|(&a, &b)| (a as i32 - b as i32).abs())
        .max()
        .unwrap_or(0);
    assert!(
        max_diff <= TOL,
        "{name}: max per-channel diff {max_diff} > {TOL}"
    );
}
```

3. Append the two new cases at the end of the file:

```rust
#[test]
fn golden_saturation_plus50() {
    check_golden(
        "saturation_plus50",
        &Edits {
            saturation: 50.0,
            ..Edits::default()
        },
    )
}

#[test]
fn golden_vibrance_plus50() {
    check_golden_with(
        "vibrance_plus50",
        &Edits {
            vibrance: 50.0,
            ..Edits::default()
        },
        muted_chart(),
    )
}
```

- [ ] **Step 2: Run the golden tests to verify the two new ones fail and the old ones still pass**

Run: `cargo test -p pipeline --test golden`
Expected: the 9 pre-existing `golden_*` tests PASS (proving the harness refactor changed nothing), and `golden_saturation_plus50` + `golden_vibrance_plus50` FAIL with `missing golden ...; run with UPDATE_GOLDEN=1`.

- [ ] **Step 3: Generate only the two new references**

Run: `UPDATE_GOLDEN=1 cargo test -p pipeline --test golden golden_saturation_plus50 golden_vibrance_plus50`

Note: `cargo test` takes a single filter string, so run the two separately if the combined form matches nothing:

```bash
UPDATE_GOLDEN=1 cargo test -p pipeline --test golden golden_saturation_plus50
UPDATE_GOLDEN=1 cargo test -p pipeline --test golden golden_vibrance_plus50
```

- [ ] **Step 4: Verify the whole golden suite passes and no existing PNG changed**

Run:

```bash
cargo test -p pipeline --test golden
git status --porcelain crates/pipeline/tests/golden
```

Expected: 11 tests pass. `git status` lists **exactly two untracked files** — `saturation_plus50.png` and `vibrance_plus50.png`. If any pre-existing PNG shows as modified, the harness refactor changed behavior — stop and fix that before committing.

- [ ] **Step 5: Eyeball the new references**

Open `crates/pipeline/tests/golden/saturation_plus50.png` and confirm the R/G/B bands are more vivid than in `neutral.png` while the gray band is unchanged. Open `vibrance_plus50.png` and confirm the muted bands gained color.

- [ ] **Step 6: Commit**

```bash
git add crates/pipeline/tests/golden.rs crates/pipeline/tests/golden/saturation_plus50.png crates/pipeline/tests/golden/vibrance_plus50.png
git commit -m "test(pipeline): golden coverage for saturation and vibrance"
```

---

### Task 5: Saturation & Vibrance sliders in the GUI

**Files:**
- Modify: `crates/app/src/main.rs:343` (the slider block in `develop_body`)

There is no unit test here — egui view code isn't unit-testable in this project (see CLAUDE.md); the verification is compile + clippy + a manual GUI check. Everything else the sliders need (undo/redo via `drag_stopped`, re-render on `changed`, sidecar persistence, export) is already wired to `s.edits` generically.

- [ ] **Step 1: Add the sliders**

In `crates/app/src/main.rs`, directly after this line:

```rust
                    slider(ui, &mut s.edits.blacks, -100.0..=100.0, "Blacks");
```

insert:

```rust
                    slider(ui, &mut s.edits.saturation, -100.0..=100.0, "Saturation");
                    slider(ui, &mut s.edits.vibrance, -100.0..=100.0, "Vibrance");
```

- [ ] **Step 2: Verify it builds clean**

Run:

```bash
cargo clippy -p app --all-targets -- -D warnings
```

Expected: no warnings, no errors.

- [ ] **Step 3: Manual GUI check**

Run:

```bash
cargo run --release -p app
```

Open a photo, then confirm all four:
1. Dragging **Saturation** right makes colors more vivid, left toward −100 makes the image gray.
2. Dragging **Vibrance** right boosts muted areas noticeably more than already-saturated ones.
3. **Ctrl+Z** undoes a settled slider drag; Ctrl+Shift+Z redoes it.
4. Close the window, reopen the same photo — the sidecar restores both values (check `<file>.<ext>.json` contains `"saturation"` and `"vibrance"`).

- [ ] **Step 4: Commit**

```bash
git add crates/app/src/main.rs
git commit -m "feat(app): Saturation and Vibrance sliders in the Develop panel"
```

---

### Task 6: Full verification and docs

**Files:**
- Modify: `CLAUDE.md`

- [ ] **Step 1: Run the whole workspace green**

Run:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test --all
```

Expected: all three succeed with no output beyond test summaries. Fix anything that fails before continuing (`cargo fmt --all` without `--check` to fix formatting).

- [ ] **Step 2: Verify the CLI path end-to-end**

Write `/tmp/sat.json`:

```json
{ "saturation": 60.0, "vibrance": 30.0 }
```

Run (substituting a real RAW path):

```bash
cargo run --release -p rustroom -- render <photo.RAF> --edits /tmp/sat.json /tmp/sat_out.jpg
```

Check the exact flag order against `cargo run -p rustroom -- render --help` if it errors. Expected: the output JPEG is visibly more colorful than the same render with an empty `{}` edits file.

- [ ] **Step 3: Update CLAUDE.md**

Make these edits:
1. In the crate-layout block, extend the `pipeline/` line to mention the saturation stage (it currently says "7 stages (incl. geometry)" → "8 stages (incl. geometry, saturation)").
2. In **Current status**, add a short "Saturation & vibrance (done)" paragraph at the top of the status body: the single `SaturationStage` (luma-weighted chroma scaling, vibrance weighted by `1 - chroma`), its position after the tone curve, the two `Edits` fields, the two sliders, and the two new goldens (11 total) — plus the documented caveat that negative vibrance uses the same `(1 - c)` weighting and so is not Adobe-identical.
3. Rewrite the **▶ RESUME HERE** block: saturation/vibrance is done; the remaining candidates are (a) snappy RAW switching (thread the `open_path` decode + thumbnail placeholder — still the top perf item) and (b) the next color adjustments (HSL / vignette).
4. Update the **Next step** section and add this plan + spec to the `docs/superpowers/` file lists.

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: record saturation + vibrance"
```

- [ ] **Step 5: Final check**

Run:

```bash
git status --porcelain
```

Expected: empty (clean tree). Do **not** push — the user pushes to GitHub themselves.

---

## Notes for the implementer

- **Never regenerate all goldens.** `UPDATE_GOLDEN=1 cargo test -p pipeline --test golden` with no filter would rewrite every reference and silently destroy the regression net. Always pass a test-name filter, as Task 4 does.
- The `-100` saturation case relies on `f_sat` being exactly `0.0`; don't "improve" the constructor with a clamp that makes it `0.001`.
- `map_pixels` takes `Fn([f32; 3]) -> [f32; 3]` and is already used by other stages — no `core-image` change is needed for this feature.
