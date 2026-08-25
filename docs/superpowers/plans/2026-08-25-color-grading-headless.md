# Colour grading (M3.3), plan 1 of 2 — headless

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Three-way colour grading — shadows / midtones / highlights wheels plus a Global wheel, Blending and Balance — renders correctly through the CLI and the golden harness, with no GUI code in existence.

**Architecture:** A new `ColorGrading` op-list type in `edit-model` (Color group), and one `ColorGradingStage` in the pipeline tail between `HslStage` and the masks loop. The stage converts to Oklab, adds an offset looked up from a 256-entry table indexed by Oklab *L*, and converts back. The table is built once per render, which is what collapses three tonal ranges into one stage.

**Tech Stack:** Rust, `serde`, existing `color::oklab` (`prophoto_to_oklab` / `oklab_to_prophoto`), existing golden-PNG harness.

**Spec:** `docs/superpowers/specs/2026-08-25-color-grading-design.md`

**Build environment** — every `cargo` command in this plan needs:

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

**Standing project rules that apply to every task here**

- Commit messages carry **no `Co-Authored-By` trailer**.
- **Never push.** The user pushes.
- After writing a test, **break the thing it names and watch it fail**. ⚠ A sabotage that fails to *compile* looks exactly like one that passes — always confirm the sabotage compiled before believing the result.
- `cargo clippy --all-targets --all-features -- -D warnings` must stay clean. ⚠ Never pipe clippy through `grep`; that reports grep's exit status.

---

### Task 1: The `ColorGrading` op-list types

**Files:**
- Modify: `crates/edit-model/src/lib.rs` (add types near `Vignette`, around line 230; tests in the existing `#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing tests**

Add to the test module at the bottom of `crates/edit-model/src/lib.rs`:

```rust
#[test]
fn a_sidecar_without_blending_still_gets_fifty() {
    // ⚠ `blending` defaults to 50, not 0, so `ColorGrading::default` must be hand-written.
    // A derived `Default` would give every partial sidecar hard-edged ranges, silently —
    // the same trap `RadialGradient::default` records.
    let e: Edits =
        serde_json::from_str(r#"{"color_grading":{"shadows":{"saturation":40.0}}}"#).unwrap();
    assert_eq!(e.color_grading.blending, 50.0);
    assert_eq!(e.color_grading.shadows.saturation, 40.0);
}

#[test]
fn a_hue_with_no_saturation_is_inert() {
    // A hue is a direction, not a strength. Rotating the wheel with the saturation at zero
    // must not insert a stage.
    let g = ColorGrading {
        shadows: GradeWheel {
            hue: 210.0,
            saturation: 0.0,
            luminance: 0.0,
        },
        ..ColorGrading::default()
    };
    assert!(!g.is_active());
}

#[test]
fn blending_and_balance_alone_are_inert() {
    // The rule `NoiseReduction::is_active` and `Vignette::is_active` already follow: a
    // modifier shapes an effect that is off until a wheel is turned up.
    let g = ColorGrading {
        blending: 100.0,
        balance: -80.0,
        ..ColorGrading::default()
    };
    assert!(!g.is_active());
}

#[test]
fn any_wheel_with_saturation_or_luminance_is_active() {
    for g in [
        ColorGrading {
            shadows: GradeWheel {
                saturation: 30.0,
                ..GradeWheel::default()
            },
            ..ColorGrading::default()
        },
        ColorGrading {
            midtones: GradeWheel {
                luminance: -20.0,
                ..GradeWheel::default()
            },
            ..ColorGrading::default()
        },
        ColorGrading {
            highlights: GradeWheel {
                saturation: 5.0,
                ..GradeWheel::default()
            },
            ..ColorGrading::default()
        },
        ColorGrading {
            global: GradeWheel {
                luminance: 1.0,
                ..GradeWheel::default()
            },
            ..ColorGrading::default()
        },
    ] {
        assert!(g.is_active(), "{g:?} should be active");
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p edit-model 2>&1 | tail -20
```

Expected: compile error, `cannot find type ColorGrading in this scope`.

- [ ] **Step 3: Add the types**

Insert into `crates/edit-model/src/lib.rs`, immediately after the `impl Vignette { .. }` block (around line 265):

```rust
/// One colour-grading wheel: a hue/saturation tint direction plus a lightness offset,
/// applied to one tonal range.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct GradeWheel {
    /// Oklab hue angle in degrees, 0..360, measured from the +a axis counterclockwise —
    /// i.e. exactly `atan2(b, a)`, with no offset to get wrong.
    ///
    /// ⚠ This is the **same angle** `BAND_HUES` uses in the HSL stage, so "red" means the
    /// same thing in both halves of the Color section. It would not if one were an sRGB hue.
    pub hue: f32,
    /// Tint strength, 0..100. 0 = no tint, whatever the hue.
    pub saturation: f32,
    /// Lightness offset for this range, -100..100.
    pub luminance: f32,
}

/// Three-way colour grading: a tint per tonal range, plus one applied everywhere.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorGrading {
    pub shadows: GradeWheel,
    pub midtones: GradeWheel,
    pub highlights: GradeWheel,
    /// Applied uniformly at every lightness, on top of the weighted three.
    pub global: GradeWheel,
    /// How far the three ranges overlap, 0..100. At 0 the nearest range wins outright; at
    /// 100 all three weigh 1/3 everywhere and the grade becomes a uniform tint.
    pub blending: f32,
    /// Slides all three range centres, -100..100. Positive lets the highlight tint reach
    /// further down.
    pub balance: f32,
}

impl Default for ColorGrading {
    /// ⚠ Hand-written, **not** derived: `blending` is 50 at neutral. The container-level
    /// `#[serde(default)]` fills a missing field from this, so a derived `Default` would give
    /// every partial sidecar `blending: 0` — hard-edged ranges — with nothing to notice.
    fn default() -> Self {
        Self {
            shadows: GradeWheel::default(),
            midtones: GradeWheel::default(),
            highlights: GradeWheel::default(),
            global: GradeWheel::default(),
            blending: 50.0,
            balance: 0.0,
        }
    }
}

impl ColorGrading {
    /// True when this actually changes pixels.
    ///
    /// Wheels only, the rule [`NoiseReduction::is_active`] and [`Vignette::is_active`] follow:
    /// `blending` and `balance` shape ranges that tint nothing until a wheel is turned up, and
    /// a `hue` with zero `saturation` is a direction with no magnitude.
    pub fn is_active(&self) -> bool {
        [self.shadows, self.midtones, self.highlights, self.global]
            .iter()
            .any(|w| w.saturation != 0.0 || w.luminance != 0.0)
    }
}
```

- [ ] **Step 4: Add the field to `Edits`**

In `crates/edit-model/src/lib.rs`, in `pub struct Edits`, immediately after `pub hsl: Hsl,`:

```rust
    pub color_grading: ColorGrading,
```

- [ ] **Step 5: Run the tests**

```bash
cargo test -p edit-model 2>&1 | tail -30
```

Expected: the four new tests pass. ⚠ `merge_from` will now **fail to compile** — that is the exhaustive destructure doing its job. Fix it in Step 6 rather than working around it.

- [ ] **Step 6: Assign the group in `merge_from`**

In `Edits::merge_from`, add `color_grading,` to the destructuring pattern immediately after `hsl,`, and add the assignment to the Color block (around line 865):

```rust
        if groups.color {
            self.saturation = *saturation;
            self.vibrance = *vibrance;
            self.hsl = *hsl;
            self.color_grading = *color_grading;
        }
```

- [ ] **Step 7: Extend the exhaustive fixture in this crate**

In `crates/edit-model/src/lib.rs`, in `fn all_nonneutral()`, add after the `hsl,` line:

```rust
            color_grading: ColorGrading {
                shadows: GradeWheel {
                    hue: 220.0,
                    saturation: 35.0,
                    luminance: -10.0,
                },
                highlights: GradeWheel {
                    hue: 40.0,
                    saturation: 30.0,
                    luminance: 8.0,
                },
                blending: 70.0,
                balance: 20.0,
                ..ColorGrading::default()
            },
```

- [ ] **Step 8: Run the whole crate's tests**

```bash
cargo test -p edit-model 2>&1 | tail -20
```

Expected: all pass. The group round-trip and disjointness tests now cover `color_grading` for free, because they are written generically over the partition.

- [ ] **Step 9: Sabotage — confirm the default test discriminates**

Temporarily add `Default` to `ColorGrading`'s derive list and delete the hand-written `impl Default`. Confirm it **compiles**, then:

```bash
cargo test -p edit-model a_sidecar_without_blending 2>&1 | tail -10
```

Expected: FAIL, `assertion `left == right` failed: left: 0.0, right: 50.0`. Restore the hand-written impl and re-run to confirm PASS.

- [ ] **Step 10: Commit**

```bash
git add crates/edit-model/src/lib.rs
git commit -m "feat(edit-model): colour-grading wheels in the op-list"
```

---

### Task 2: Repair the two exhaustive fixtures outside `edit-model`

Adding a field to `Edits` breaks two more explicitly-listed fixtures. They are deliberately exhaustive so that this happens; the fix is to list the new field, never to switch them to `..Default::default()`.

**Files:**
- Modify: `crates/pipeline/src/lib.rs` — `fn all_edits()`, around line 478
- Modify: `crates/app/src/edit_clipboard.rs` — the payload fixture, around line 95

- [ ] **Step 1: See the breakage**

```bash
cargo test --all 2>&1 | grep -E "^error" | head
```

Expected: `error[E0063]: missing field \`color_grading\` in initializer of \`Edits\`` in both files.

- [ ] **Step 2: Fix `all_edits()`**

In `crates/pipeline/src/lib.rs`, in `fn all_edits()`, add after the `hsl,` line:

```rust
            color_grading: edit_model::ColorGrading {
                shadows: edit_model::GradeWheel {
                    hue: 240.0,
                    saturation: 40.0,
                    luminance: -15.0,
                },
                highlights: edit_model::GradeWheel {
                    hue: 45.0,
                    saturation: 35.0,
                    luminance: 10.0,
                },
                blending: 60.0,
                balance: 15.0,
                ..Default::default()
            },
```

- [ ] **Step 3: Fix the clipboard fixture**

In `crates/app/src/edit_clipboard.rs`, add after the `hsl,` line in the same fixture:

```rust
            color_grading: edit_model::ColorGrading {
                midtones: edit_model::GradeWheel {
                    hue: 160.0,
                    saturation: 25.0,
                    luminance: 5.0,
                },
                blending: 40.0,
                balance: -10.0,
                ..Default::default()
            },
```

- [ ] **Step 4: Run the whole suite**

```bash
cargo test --all 2>&1 | tail -25
```

Expected: everything compiles and passes. ⚠ `all_edits()` now carries an active grade but no stage exists yet, so the pipeline is unchanged — `the_prefix_holds_geometry_noise_reduction_and_dehaze` still passes because grading is a *tail* concept.

- [ ] **Step 5: Commit**

```bash
git add crates/pipeline/src/lib.rs crates/app/src/edit_clipboard.rs
git commit -m "test: carry colour grading in the exhaustive edit fixtures"
```

---

### Task 3: The range weights

**Files:**
- Create: `crates/pipeline/src/stages/color_grading.rs`
- Modify: `crates/pipeline/src/stages/mod.rs`

- [ ] **Step 1: Create the module with only the weight function's signature and constants**

Create `crates/pipeline/src/stages/color_grading.rs`:

```rust
//! Three-way colour grading: a tint per tonal range, added in Oklab.

/// Narrowest range width, at Blending 0 — near-hard selection.
const SIGMA_MIN: f32 = 0.08;
/// Widest, at Blending 100 — wide enough that all three weights sit within a few percent
/// of 1/3 across the whole lightness range, i.e. a uniform tint.
const SIGMA_MAX: f32 = 3.0;
/// How far Balance ±100 slides the three range centres, in Oklab L.
const BALANCE_SHIFT: f32 = 0.25;

/// Range centres in Oklab L before Balance slides them: black, mid, white.
///
/// ⚠ The ranges are selected on **Oklab L, not physical luminance**. We are already in Oklab
/// for the tint, so L is free — and mid-grey sits near 0.6 in L but at 0.18 in Y. Selecting
/// on Y would put "midtones" where most people would say highlights.
const CENTRES: [f32; 3] = [0.0, 0.5, 1.0];

/// The shadow / midtone / highlight weights at lightness `l`.
///
/// Three Gaussians, normalised — so they **sum to exactly 1 at every lightness**. That is what
/// makes three equal wheels reproduce that tint uniformly with no seams, and it is why the
/// Global wheel is a plain fourth constant rather than a fourth range.
pub(crate) fn range_weights(l: f32, blending: f32, balance: f32) -> [f32; 3] {
    let sigma = SIGMA_MIN * (SIGMA_MAX / SIGMA_MIN).powf(blending.clamp(0.0, 100.0) / 100.0);
    let shift = -(balance.clamp(-100.0, 100.0) / 100.0) * BALANCE_SHIFT;

    let mut e = [0.0f32; 3];
    for i in 0..3 {
        let d = (l - (CENTRES[i] + shift)) / sigma;
        e[i] = -(d * d);
    }

    // ⚠ Subtracting the maximum exponent before exponentiating is load-bearing, not tidiness.
    // Without it, an `l` far from every centre at small sigma underflows all three
    // exponentials to zero and the normalisation divides by zero. With it the largest raw
    // weight is exactly 1, so the sum is always >= 1.
    let m = e[0].max(e[1]).max(e[2]);
    let raw = [(e[0] - m).exp(), (e[1] - m).exp(), (e[2] - m).exp()];
    let sum = raw[0] + raw[1] + raw[2];
    [raw[0] / sum, raw[1] / sum, raw[2] / sum]
}
```

Add to `crates/pipeline/src/stages/mod.rs`, keeping the list alphabetical — between `pub mod contrast;` and `pub mod dehaze;`:

```rust
pub mod color_grading;
```

- [ ] **Step 2: Write the failing tests**

Append to `crates/pipeline/src/stages/color_grading.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const BLENDINGS: [f32; 5] = [0.0, 25.0, 50.0, 75.0, 100.0];
    const BALANCES: [f32; 5] = [-100.0, -50.0, 0.0, 50.0, 100.0];

    #[test]
    fn the_weights_sum_to_one_at_every_lightness() {
        for &blending in &BLENDINGS {
            for &balance in &BALANCES {
                for i in 0..=100 {
                    let l = i as f32 / 100.0;
                    let w = range_weights(l, blending, balance);
                    let sum = w[0] + w[1] + w[2];
                    assert!(
                        (sum - 1.0).abs() < 1e-5,
                        "l={l} blending={blending} balance={balance} sum={sum}"
                    );
                    assert!(w.iter().all(|&x| x >= 0.0), "negative weight in {w:?}");
                }
            }
        }
    }

    #[test]
    fn blending_zero_selects_one_range_almost_exclusively() {
        assert!(range_weights(0.0, 0.0, 0.0)[0] > 0.99);
        assert!(range_weights(0.5, 0.0, 0.0)[1] > 0.99);
        assert!(range_weights(1.0, 0.0, 0.0)[2] > 0.99);
    }

    #[test]
    fn blending_one_hundred_weighs_the_ranges_almost_equally() {
        for i in 0..=10 {
            let l = i as f32 / 10.0;
            let w = range_weights(l, 100.0, 0.0);
            for &x in &w {
                assert!((x - 1.0 / 3.0).abs() < 0.03, "l={l} w={w:?}");
            }
        }
    }

    #[test]
    fn positive_balance_lets_the_highlight_tint_reach_lower() {
        let l = 0.4;
        let neutral = range_weights(l, 50.0, 0.0)[2];
        let shifted = range_weights(l, 50.0, 80.0)[2];
        assert!(
            shifted > neutral,
            "highlight weight at l={l}: balance 0 gave {neutral}, +80 gave {shifted}"
        );
    }

    #[test]
    fn an_extreme_lightness_at_the_narrowest_width_still_normalises() {
        // The case the max-subtraction exists for: every exponent hugely negative at once.
        for &balance in &BALANCES {
            for &l in &[-2.0f32, 0.0, 1.0, 3.0] {
                let w = range_weights(l, 0.0, balance);
                let sum = w[0] + w[1] + w[2];
                assert!(
                    (sum - 1.0).abs() < 1e-5 && sum.is_finite(),
                    "l={l} balance={balance} sum={sum} w={w:?}"
                );
            }
        }
    }
}
```

- [ ] **Step 3: Run them**

```bash
cargo test -p pipeline color_grading 2>&1 | tail -20
```

Expected: all five PASS (the implementation from Step 1 is already complete — these tests exist to pin it, and the sabotages below are what prove they can see anything).

- [ ] **Step 4: Sabotage 1 — delete the normalisation**

Change the last line of `range_weights` to `[raw[0], raw[1], raw[2]]`. Confirm it **compiles**, then:

```bash
cargo test -p pipeline color_grading 2>&1 | tail -20
```

Expected: `the_weights_sum_to_one_at_every_lightness` and `blending_one_hundred_weighs_the_ranges_almost_equally` FAIL. Restore.

- [ ] **Step 5: Sabotage 2 — flip the balance sign**

Change `let shift = -(balance...` to `let shift = (balance.clamp(-100.0, 100.0) / 100.0) * BALANCE_SHIFT;`. Confirm it compiles, then run.

Expected: `positive_balance_lets_the_highlight_tint_reach_lower` FAILS. Restore.

- [ ] **Step 6: Sabotage 3 — delete the max subtraction**

Change to `let raw = [e[0].exp(), e[1].exp(), e[2].exp()];`. Confirm it compiles, then run.

Expected: `an_extreme_lightness_at_the_narrowest_width_still_normalises` FAILS (`sum=NaN`). Restore and re-run to confirm all five pass.

- [ ] **Step 7: Commit**

```bash
git add crates/pipeline/src/stages/color_grading.rs crates/pipeline/src/stages/mod.rs
git commit -m "feat(pipeline): tonal range weights for colour grading"
```

---

### Task 4: The stage

**Files:**
- Modify: `crates/pipeline/src/stages/color_grading.rs`

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `crates/pipeline/src/stages/color_grading.rs`:

```rust
    use core_image::ImageBuf;
    use edit_model::{ColorGrading, GradeWheel};

    fn apply_one(g: &ColorGrading, px: [f32; 3]) -> [f32; 3] {
        let mut img = ImageBuf::new(1, 1);
        img.set_pixel(0, 0, px);
        ColorGradingStage::new(g).apply(img).pixel(0, 0)
    }

    /// The grade at `l`, computed straight from the definition with no table involved —
    /// the independent reference the LUT is checked against.
    fn direct_grade(g: &ColorGrading, l: f32) -> [f32; 3] {
        let w = range_weights(l.clamp(0.0, 1.0), g.blending, g.balance);
        let wheels = [g.shadows, g.midtones, g.highlights];
        let mut o = wheel_offset(&g.global);
        for k in 0..3 {
            let c = wheel_offset(&wheels[k]);
            o[0] += w[k] * c[0];
            o[1] += w[k] * c[1];
            o[2] += w[k] * c[2];
        }
        o
    }

    fn split_tone() -> ColorGrading {
        ColorGrading {
            shadows: GradeWheel {
                hue: 240.0,
                saturation: 60.0,
                luminance: -20.0,
            },
            highlights: GradeWheel {
                hue: 45.0,
                saturation: 60.0,
                luminance: 15.0,
            },
            ..ColorGrading::default()
        }
    }

    #[test]
    fn a_neutral_grade_returns_the_pixel_unchanged() {
        // ⚠ Within rounding, not bit-exact: a neutral grade still round-trips through Oklab.
        // The stage is never *inserted* at neutral — `build_pipeline` skips it — which is
        // what keeps the existing goldens byte-identical.
        let px = [0.4, 0.25, 0.15];
        let out = apply_one(&ColorGrading::default(), px);
        for k in 0..3 {
            assert!((out[k] - px[k]).abs() < 1e-5, "{out:?} vs {px:?}");
        }
    }

    #[test]
    fn the_table_agrees_with_direct_evaluation() {
        let g = split_tone();
        let stage = ColorGradingStage::new(&g);
        for i in 0..=200 {
            let l = i as f32 / 200.0;
            let (got, want) = (stage.grade_at(l), direct_grade(&g, l));
            for k in 0..3 {
                assert!(
                    (got[k] - want[k]).abs() < 1e-3,
                    "l={l} component {k}: table {got:?} vs direct {want:?}"
                );
            }
        }
    }

    #[test]
    fn three_wheels_alike_reproduce_a_uniform_tint() {
        // The payoff of sum-to-one: the same tint at every lightness, with no seams.
        let w = GradeWheel {
            hue: 200.0,
            saturation: 70.0,
            luminance: 30.0,
        };
        let g = ColorGrading {
            shadows: w,
            midtones: w,
            highlights: w,
            ..ColorGrading::default()
        };
        let stage = ColorGradingStage::new(&g);
        let want = wheel_offset(&w);
        for i in 0..=100 {
            let l = i as f32 / 100.0;
            let got = stage.grade_at(l);
            for k in 0..3 {
                assert!(
                    (got[k] - want[k]).abs() < 1e-5,
                    "l={l} component {k}: {got:?} vs {want:?}"
                );
            }
        }
    }

    #[test]
    fn the_global_wheel_adds_on_top_of_the_weighted_ranges() {
        let base = split_tone();
        let extra = GradeWheel {
            hue: 90.0,
            saturation: 40.0,
            luminance: -25.0,
        };
        let with_global = ColorGrading {
            global: extra,
            ..base
        };
        let (a, b) = (
            ColorGradingStage::new(&base),
            ColorGradingStage::new(&with_global),
        );
        let want = wheel_offset(&extra);
        for i in 0..=50 {
            let l = i as f32 / 50.0;
            let (x, y) = (a.grade_at(l), b.grade_at(l));
            for k in 0..3 {
                assert!(
                    ((y[k] - x[k]) - want[k]).abs() < 1e-5,
                    "l={l} component {k}: delta {} vs {}",
                    y[k] - x[k],
                    want[k]
                );
            }
        }
    }

    #[test]
    fn a_grey_pixel_is_tinted() {
        // Unlike HSL, grading is deliberately **not** chroma-gated: it *assigns* a hue rather
        // than reading one, so there is no noisy hue to fade out — and tinting neutrals is
        // the entire point of shadow toning.
        let g = ColorGrading {
            shadows: GradeWheel {
                hue: 240.0,
                saturation: 80.0,
                luminance: 0.0,
            },
            ..ColorGrading::default()
        };
        let out = apply_one(&g, [0.05, 0.05, 0.05]);
        let [_, a, b] = color::oklab::prophoto_to_oklab(out);
        assert!(a.hypot(b) > 0.01, "grey stayed neutral: {out:?}");
    }

    #[test]
    fn the_tint_lands_at_the_hue_the_wheel_names() {
        // Pins the convention: a wheel's `hue` **is** the Oklab hue angle of the tint it
        // applies — the same angle `BAND_HUES` uses in the HSL stage.
        //
        // ⚠ This cannot be left to `the_table_agrees_with_direct_evaluation`, which calls
        // `wheel_offset` on *both* sides and so is blind to anything wrong inside it; nor to a
        // golden, which only ever catches a *change* and would happily bake a wrong convention
        // into its reference on the first run. This test derives the answer independently, by
        // measuring the chroma the stage actually produced.
        for &hue in &[0.0f32, 45.0, 90.0, 180.0, 240.0, 330.0] {
            let g = ColorGrading {
                global: GradeWheel {
                    hue,
                    saturation: 40.0,
                    luminance: 0.0,
                },
                ..ColorGrading::default()
            };
            // A neutral pixel carries no chroma of its own, so the output's chroma is the tint.
            let out = apply_one(&g, [0.4, 0.4, 0.4]);
            let [_, a, b] = color::oklab::prophoto_to_oklab(out);
            let got = b.atan2(a).to_degrees().rem_euclid(360.0);
            let err = (got - hue + 540.0).rem_euclid(360.0) - 180.0;
            assert!(err.abs() < 1.0, "hue {hue}: tint landed at {got}");
        }
    }

    #[test]
    fn a_pixel_above_white_keeps_its_headroom() {
        // ⚠ The **index** clamps to [0, 1]; the pixel's own L must not. Clamping the value
        // would drag every scene-referred highlight back to white, destroying exactly the
        // headroom the wide working space exists to preserve. Same trap as HueSatMap's `v`.
        let px = [2.0, 2.0, 2.0];
        let before = color::oklab::prophoto_to_oklab(px)[0];
        assert!(before > 1.2, "fixture is not above white: L={before}");

        let g = ColorGrading {
            highlights: GradeWheel {
                hue: 45.0,
                saturation: 60.0,
                luminance: 0.0,
            },
            ..ColorGrading::default()
        };
        let after = color::oklab::prophoto_to_oklab(apply_one(&g, px))[0];
        assert!(
            (after - before).abs() < 1e-4,
            "lightness moved: {before} -> {after}"
        );
    }

    #[test]
    fn the_output_is_clamped_at_zero_but_not_at_one() {
        // Matches `SaturationStage` and `HslStage`. Clipping at the top belongs in the output
        // transform and nowhere else.
        let g = ColorGrading {
            global: GradeWheel {
                hue: 30.0,
                saturation: 100.0,
                luminance: 100.0,
            },
            ..ColorGrading::default()
        };
        let out = apply_one(&g, [1.6, 1.6, 1.6]);
        assert!(out.iter().all(|&v| v >= 0.0), "negative sample in {out:?}");
        assert!(
            out.iter().any(|&v| v > 1.0),
            "headroom was clipped: {out:?}"
        );
    }
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test -p pipeline color_grading 2>&1 | tail -20
```

Expected: compile error, `cannot find function wheel_offset` / `cannot find type ColorGradingStage`.

- [ ] **Step 3: Implement the stage**

Insert into `crates/pipeline/src/stages/color_grading.rs`, between the `range_weights` function and the `#[cfg(test)]` module. Also add the imports directly below the `//!` module doc comment and above the constants:

```rust
use crate::PipelineStage;
// ⚠ The **ProPhoto** pair, not the sRGB one: this stage converts the working space in and out
// again, and mixing the two directions produces a plausible-looking colour shift that is very
// hard to attribute later.
use color::oklab::{oklab_to_prophoto, prophoto_to_oklab};
use core_image::ImageBuf;
use edit_model::{ColorGrading, GradeWheel};
```

and the body:

```rust
/// Oklab chroma offset at Saturation 100. For scale, a vivid sRGB red has a chroma near 0.26.
const SAT_MAX: f32 = 0.15;
/// Oklab L offset at Luminance ±100.
const LUM_MAX: f32 = 0.15;
/// Entries in the lightness table. The weight curves are smooth, so 256 with linear
/// interpolation is well inside the tolerance the table is checked against.
const LUT_SIZE: usize = 256;

/// One wheel's contribution as an Oklab offset `(da, db, dl)`.
pub(crate) fn wheel_offset(w: &GradeWheel) -> [f32; 3] {
    let h = w.hue.to_radians();
    let s = (w.saturation / 100.0) * SAT_MAX;
    [
        s * h.cos(),
        s * h.sin(),
        (w.luminance / 100.0) * LUM_MAX,
    ]
}

/// A tint per tonal range, added in Oklab.
///
/// ⚠ **One stage, not three.** The tint at a pixel depends on nothing but its lightness and on
/// constants, so the whole three-wheel grade collapses to a 1-D table built once per render.
/// Only one stage can do that; three sequential stages would mean three Oklab round trips with
/// each range re-measuring a lightness the previous one had already moved — the interaction
/// `LocalContrastStage` sums deltas against one source specifically to avoid.
pub struct ColorGradingStage {
    /// `(da, db, dl)` per lightness, indexed by `l * (LUT_SIZE - 1)` and interpolated.
    lut: Vec<[f32; 3]>,
}

impl ColorGradingStage {
    pub fn new(g: &ColorGrading) -> Self {
        let wheels = [
            wheel_offset(&g.shadows),
            wheel_offset(&g.midtones),
            wheel_offset(&g.highlights),
        ];
        let global = wheel_offset(&g.global);

        let lut = (0..LUT_SIZE)
            .map(|i| {
                let l = i as f32 / (LUT_SIZE - 1) as f32;
                let w = range_weights(l, g.blending, g.balance);
                let mut o = global;
                for k in 0..3 {
                    o[0] += w[k] * wheels[k][0];
                    o[1] += w[k] * wheels[k][1];
                    o[2] += w[k] * wheels[k][2];
                }
                o
            })
            .collect();

        Self { lut }
    }

    /// The grade at lightness `l`, linearly interpolated between table entries.
    ///
    /// ⚠ The **index** clamps to `[0, 1]`; the caller's `l` does not. Scene-referred values
    /// above white have `L > 1`, and clamping the value rather than the index would drag every
    /// highlight back to white — destroying exactly the headroom the wide working space exists
    /// to preserve. Identical trap to `ProfileHueSatMap`'s `v`.
    fn grade_at(&self, l: f32) -> [f32; 3] {
        let t = l.clamp(0.0, 1.0) * (LUT_SIZE - 1) as f32;
        let i = (t.floor() as usize).min(LUT_SIZE - 1);
        let j = (i + 1).min(LUT_SIZE - 1);
        let f = t - i as f32;
        let (a, b) = (self.lut[i], self.lut[j]);
        [
            a[0] + (b[0] - a[0]) * f,
            a[1] + (b[1] - a[1]) * f,
            a[2] + (b[2] - a[2]) * f,
        ]
    }
}

impl PipelineStage for ColorGradingStage {
    fn name(&self) -> &'static str {
        "color_grading"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        image.map_pixels(|rgb| {
            let [l, a, b] = prophoto_to_oklab(rgb);
            let [da, db, dl] = self.grade_at(l);
            let out = oklab_to_prophoto([l + dl, a + da, b + db]);
            // Clamped at 0 only, matching `SaturationStage` and `HslStage`. Never at 1: the
            // output transform is the only place clipping belongs.
            [out[0].max(0.0), out[1].max(0.0), out[2].max(0.0)]
        })
    }
}
```

- [ ] **Step 4: Run the tests**

```bash
cargo test -p pipeline color_grading 2>&1 | tail -25
```

Expected: all thirteen PASS (five from Task 3, eight new).

- [ ] **Step 5: Sabotage 1 — clamp the value instead of the index**

In `apply`, change `let [da, db, dl] = self.grade_at(l);` and the line after it to:

```rust
            let l = l.clamp(0.0, 1.0);
            let [da, db, dl] = self.grade_at(l);
```

Confirm it compiles, then run.

Expected: `a_pixel_above_white_keeps_its_headroom` FAILS (`lightness moved: 1.26… -> 1.0`). Restore.

- [ ] **Step 6: Sabotage 2 — swap the offset's `a` and `b` components**

In `wheel_offset`, swap the first two array entries to `[s * h.sin(), s * h.cos(), ..]`. Confirm it compiles, then run.

Expected: **only** `the_tint_lands_at_the_hue_the_wheel_names` FAILS, at hue 45 and beyond. ⚠ Note which tests stay green and why: `the_table_agrees_with_direct_evaluation` and `three_wheels_alike_reproduce_a_uniform_tint` both call `wheel_offset` on each side, so neither can see a fault *inside* it; `a_grey_pixel_is_tinted` checks a chroma magnitude, which a swap preserves. That is the whole reason the hue-convention test exists. Restore.

⚠ If a hue in that test fails because a channel clamped at 0 rather than because the angle is wrong, **lower the test's `saturation` constant** (documenting why) rather than widening the tolerance — a wide tolerance on an angle is what would let a real convention error through.

- [ ] **Step 7: Sabotage 3 — make the LUT 4 entries**

Change `const LUT_SIZE: usize = 256;` to `4`. Confirm it compiles, then run.

Expected: `the_table_agrees_with_direct_evaluation` FAILS. Restore and re-run to confirm all pass.

- [ ] **Step 8: Commit**

```bash
git add crates/pipeline/src/stages/color_grading.rs
git commit -m "feat(pipeline): the colour-grading stage"
```

---

### Task 5: Wire it into the pipeline

**Files:**
- Modify: `crates/pipeline/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `crates/pipeline/src/lib.rs`:

```rust
    #[test]
    fn colour_grading_runs_after_hsl_and_before_the_masks() {
        // After HSL because HSL *corrects* colours that are in the photograph while a grade is
        // a look laid over the top — reversed, an HSL band tweak would act on the tint just
        // added. Before the masks because a mask "should act on the global look rather than be
        // re-mapped by it", and a grade is global look.
        let mut hsl = edit_model::Hsl::default();
        hsl.bands[0].saturation = 20.0;
        let edits = Edits {
            hsl,
            color_grading: edit_model::ColorGrading {
                shadows: edit_model::GradeWheel {
                    hue: 240.0,
                    saturation: 40.0,
                    luminance: 0.0,
                },
                ..Default::default()
            },
            masks: vec![edit_model::MaskedAdjustment {
                mask: edit_model::Mask::Linear(edit_model::LinearGradient {
                    from: [0.0, 0.0],
                    to: [1.0, 1.0],
                    invert: false,
                }),
                adjustments: edit_model::LocalAdjustments {
                    exposure: 0.75,
                    ..Default::default()
                },
            }],
            vignette: edit_model::Vignette {
                amount: -40.0,
                ..Default::default()
            },
            ..Edits::default()
        };
        assert_eq!(
            build_pipeline(&edits, RenderScale::FULL).stage_names(),
            vec!["hsl", "color_grading", "mask", "vignette", "output_transform"]
        );
    }

    #[test]
    fn colour_grading_modifiers_alone_add_no_stage() {
        let edits = Edits {
            color_grading: edit_model::ColorGrading {
                blending: 100.0,
                balance: -60.0,
                ..Default::default()
            },
            ..Edits::default()
        };
        assert_eq!(build_pipeline(&edits, RenderScale::FULL).len(), 1);
    }
```

And add a `color_grading` entry to the `downstream` array in `prefix_key_ignores_every_downstream_field`, immediately after the `hsl` entry:

```rust
            Edits {
                color_grading: edit_model::ColorGrading {
                    highlights: edit_model::GradeWheel {
                        hue: 30.0,
                        saturation: 50.0,
                        luminance: 20.0,
                    },
                    blending: 10.0,
                    balance: 60.0,
                    ..Default::default()
                },
                ..base.clone()
            },
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test -p pipeline colour_grading 2>&1 | tail -20
```

Expected: `colour_grading_runs_after_hsl_and_before_the_masks` FAILS — the stage list is missing `"color_grading"`.

- [ ] **Step 3: Export the stage**

In `crates/pipeline/src/lib.rs`, add to the `pub use` block, keeping it alphabetical (between `ContrastStage` and `DehazeStage`):

```rust
pub use stages::color_grading::ColorGradingStage;
```

- [ ] **Step 4: Insert the stage into the tail**

In `build_pipeline_split_in_region_with_brush`, immediately after the `if edits.hsl != edit_model::Hsl::default() { .. }` block and before the masks loop:

```rust
    // A look laid over the corrected photograph. After HSL because HSL corrects colours that
    // are *in* the picture while a grade is applied on top — reversed, an HSL band tweak would
    // act on the tint just added. After the tone curve and local contrast is forced rather than
    // chosen: the ranges are selected on lightness, so anything remapping lightness later would
    // slide pixels between ranges. Before the masks, which act on the global look.
    //
    // Per-pixel and position-independent, so it needs no `Region`, no `RenderScale` and no
    // kernel margin — and being in the tail, no `PrefixStepKey`.
    if edits.color_grading.is_active() {
        tail.push(Box::new(ColorGradingStage::new(&edits.color_grading)));
    }
```

- [ ] **Step 5: Run the whole suite**

```bash
cargo test --all 2>&1 | tail -25
```

Expected: everything passes, **including all 39 goldens byte-identical** — the stage is inert at its default, which is the acceptance criterion for this half.

- [ ] **Step 6: Sabotage — move the stage after the masks loop**

Move the block to just after the `for (i, m) in edits.masks.iter().enumerate() { .. }` loop. Confirm it compiles, then run.

Expected: `colour_grading_runs_after_hsl_and_before_the_masks` FAILS with the order `["hsl", "mask", "color_grading", ..]`. Restore and re-run.

- [ ] **Step 7: Lint and commit**

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
echo "clippy exit: $?"
```

Expected: `clippy exit: 0` (check the exit code, not grep output).

```bash
git add crates/pipeline/src/lib.rs
git commit -m "feat(pipeline): run colour grading after HSL, before the masks"
```

---

### Task 6: Goldens

**Files:**
- Modify: `crates/pipeline/tests/golden.rs`
- Create: `crates/pipeline/tests/golden/grade_split_tone.png`, `grade_luminance.png`, `grade_blending_wide.png`, `grade_balance_high.png`

- [ ] **Step 1: Add the chart and the four golden tests**

Add the chart alongside the other chart functions in `crates/pipeline/tests/golden.rs` (after `hazy_chart`):

```rust
/// A lightness ramp for the colour-grading goldens: a neutral ramp above a muted warm one, so a
/// grade can be seen acting on both neutral and chromatic pixels.
///
/// ⚠ The ramp runs **0.02..0.85, not 0..1**. Adding chroma near white clips, and a clipped
/// golden cannot tell one tint from another — the failure the exposure golden recorded, where
/// 15 of 64 samples pinned at 255. Step 3 below measures that no sample reaches 255.
fn grade_ramp_chart() -> ImageBuf {
    const GW: usize = 64;
    const GH: usize = 16;
    let mut img = ImageBuf::new(GW, GH);
    for y in 0..GH {
        for x in 0..GW {
            let v = 0.02 + (x as f32 / (GW - 1) as f32) * 0.83;
            let px = if y < GH / 2 {
                [v, v, v]
            } else {
                [v, v * 0.82, v * 0.66]
            };
            set_srgb(&mut img, x, y, px);
        }
    }
    img
}

/// A blue-shadow / warm-highlight split tone, the canonical grade.
fn split_tone() -> ColorGrading {
    ColorGrading {
        shadows: GradeWheel {
            hue: 240.0,
            saturation: 60.0,
            luminance: 0.0,
        },
        highlights: GradeWheel {
            hue: 45.0,
            saturation: 60.0,
            luminance: 0.0,
        },
        ..ColorGrading::default()
    }
}
```

Add `ColorGrading, GradeWheel` to the `use edit_model::{..}` list at the top of the file.

Then the four tests, at the end of the file:

```rust
#[test]
fn golden_grade_split_tone() {
    check_golden_with(
        "grade_split_tone",
        &Edits {
            color_grading: split_tone(),
            ..Edits::default()
        },
        grade_ramp_chart(),
    );
}

#[test]
fn golden_grade_luminance() {
    check_golden_with(
        "grade_luminance",
        &Edits {
            color_grading: ColorGrading {
                shadows: GradeWheel {
                    hue: 0.0,
                    saturation: 0.0,
                    luminance: -70.0,
                },
                highlights: GradeWheel {
                    hue: 0.0,
                    saturation: 0.0,
                    luminance: 70.0,
                },
                ..ColorGrading::default()
            },
            ..Edits::default()
        },
        grade_ramp_chart(),
    );
}

#[test]
fn golden_grade_blending_wide() {
    check_golden_with(
        "grade_blending_wide",
        &Edits {
            color_grading: ColorGrading {
                blending: 100.0,
                ..split_tone()
            },
            ..Edits::default()
        },
        grade_ramp_chart(),
    );
}

#[test]
fn golden_grade_balance_high() {
    check_golden_with(
        "grade_balance_high",
        &Edits {
            color_grading: ColorGrading {
                balance: 80.0,
                ..split_tone()
            },
            ..Edits::default()
        },
        grade_ramp_chart(),
    );
}
```

- [ ] **Step 2: Generate the references**

```bash
UPDATE_GOLDEN=1 cargo test -p pipeline --test golden 2>&1 | tail -5
ls crates/pipeline/tests/golden/ | wc -l
```

Expected: 43.

- [ ] **Step 3: Measure the goldens for clipping — do not merely look at them**

```bash
python3 - <<'PY'
from PIL import Image
for n in ("grade_split_tone","grade_luminance","grade_blending_wide","grade_balance_high"):
    px = list(Image.open(f"crates/pipeline/tests/golden/{n}.png").convert("RGB").getdata())
    flat = [c for p in px for c in p]
    hi = sum(1 for c in flat if c >= 255)
    lo = sum(1 for c in flat if c <= 0)
    print(f"{n}: max={max(flat)} min={min(flat)} clipped_high={hi} clipped_low={lo} n={len(flat)}")
PY
```

Expected: `clipped_high` and `clipped_low` are **0** for all four, and `max` is comfortably below 255. If any golden clips, lower the ramp's top from `0.83` and regenerate — a clipped golden cannot discriminate.

- [ ] **Step 4: Confirm the four goldens actually differ from one another**

A grade whose parameters do nothing would still produce four valid-looking PNGs.

```bash
cd crates/pipeline/tests/golden && \
  for a in grade_split_tone grade_luminance grade_blending_wide grade_balance_high; do \
    for b in grade_split_tone grade_luminance grade_blending_wide grade_balance_high; do \
      [ "$a" \< "$b" ] && { cmp -s $a.png $b.png && echo "IDENTICAL: $a $b" || echo "differ: $a $b"; }; \
    done; done; cd -
```

Expected: six `differ:` lines and no `IDENTICAL:`.

- [ ] **Step 5: Run the full golden suite against the committed references**

```bash
cargo test -p pipeline --test golden 2>&1 | tail -5
```

Expected: 43 passed. ⚠ The 39 pre-existing references must be **byte-identical**; confirm with `git status` showing only the four new PNGs as additions and no modifications.

```bash
git status --short crates/pipeline/tests/golden/
```

Expected: exactly four `??` lines, no ` M ` lines.

- [ ] **Step 6: Commit**

```bash
git add crates/pipeline/tests/golden.rs crates/pipeline/tests/golden/grade_*.png
git commit -m "test(pipeline): goldens for colour grading"
```

---

### Task 7: Validate on a real photograph, and measure

**Files:**
- No source changes expected. Create scratch files under the scratchpad directory only.

- [ ] **Step 1: Confirm a RAW fixture exists**

```bash
ls -la ~/Desktop/fuji/*.RAF 2>/dev/null | head -3
```

If nothing is there, ask the user for a path and use it below. This step cannot be skipped — a synthetic chart cannot falsify how the grade reads on a photograph.

- [ ] **Step 2: Write a grading sidecar**

```bash
SP=/private/tmp/claude-501/-Users-me-Projects-rustroom/58ca40ba-db49-4da9-800f-b23dd64a2ed6/scratchpad
mkdir -p "$SP"
cat > "$SP/grade.json" <<'JSON'
{
  "color_grading": {
    "shadows":    { "hue": 240.0, "saturation": 70.0, "luminance": 0.0 },
    "highlights": { "hue": 45.0,  "saturation": 70.0, "luminance": 0.0 },
    "blending": 50.0,
    "balance": 0.0
  }
}
JSON
cat > "$SP/neutral.json" <<'JSON'
{}
JSON
```

- [ ] **Step 3: Render both**

```bash
SP=/private/tmp/claude-501/-Users-me-Projects-rustroom/58ca40ba-db49-4da9-800f-b23dd64a2ed6/scratchpad
RAW=$(ls ~/Desktop/fuji/*.RAF | head -1)
cargo run --release -p rustroom -- render "$RAW" "$SP/neutral.jpg" --edits "$SP/neutral.json"
cargo run --release -p rustroom -- render "$RAW" "$SP/graded.jpg" --edits "$SP/grade.json"
```

Expected: both write without error.

- [ ] **Step 4: Measure the effect, split by lightness**

⚠ A whole-image mean is the wrong statistic here and will understate the effect — the same mistake the HueSatMap work nearly drew a wrong conclusion from. Bucket by lightness instead: shadows must move blue, highlights warm, and midtones barely at all.

```bash
SP=/private/tmp/claude-501/-Users-me-Projects-rustroom/58ca40ba-db49-4da9-800f-b23dd64a2ed6/scratchpad
python3 - <<PY
from PIL import Image
a = Image.open("$SP/neutral.jpg").convert("RGB")
b = Image.open("$SP/graded.jpg").convert("RGB")
a = a.resize((400, int(400*a.height/a.width)))
b = b.resize(a.size)
buckets = {"shadow": [], "mid": [], "high": []}
for (r0,g0,b0),(r1,g1,b1) in zip(a.getdata(), b.getdata()):
    l = (r0+g0+b0)/3
    k = "shadow" if l < 70 else ("high" if l > 170 else "mid")
    buckets[k].append(((r1-r0)-(b1-b0), l))
for k,v in buckets.items():
    if not v: print(f"{k}: empty"); continue
    warm = sum(x for x,_ in v)/len(v)   # (R-B) shift: positive = warmer
    print(f"{k}: n={len(v)} mean_warm_shift={warm:+.2f}")
PY
```

Expected: `shadow` clearly **negative** (pushed blue), `high` clearly **positive** (pushed warm), `mid` near zero. If the signs are reversed, the hue convention is wrong — fix `wheel_offset`, not the test.

- [ ] **Step 5: Measure the cost**

```bash
RAW=$(ls ~/Desktop/fuji/*.RAF | head -1)
cargo run --release -p app --example bench_preview "$RAW" 2>&1 | tail -20
```

Record the neutral and grading numbers. If `bench_preview` has no colour-grading case, add one alongside the existing HSL case, following that file's pattern, and re-run. The number goes into the CLAUDE.md entry — CLAUDE.md's standing rule is that a cost is measured, not estimated.

- [ ] **Step 6: Full green check**

```bash
cargo fmt --all --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all 2>&1 | tail -15
echo "exit: $?"
```

Expected: `exit: 0`, all tests passing, golden count 43.

- [ ] **Step 7: Commit any bench addition**

```bash
git add -A
git commit -m "test(app): bench case for colour grading"
```

(Skip if nothing changed.)

---

## Definition of done for plan 1

- `ColorGrading` is in the op-list, in the **Color** group, transferring through paste / preset / sync.
- One `ColorGradingStage` runs between `hsl` and `mask`.
- 43 goldens; the 39 pre-existing ones **byte-identical**.
- `cargo fmt --check`, `cargo clippy --all-targets --all-features -D warnings`, `cargo test --all` all green.
- The grade validated on a real `.RAF`, **measured by lightness bucket** rather than eyeballed.
- The preview cost measured and recorded.
- `LocalAdjustments` is **untouched**, so masks carry no grading — a grade is global look, and
  `per_pixel_stages` is shared between the global tail and every mask.
- No GUI code exists yet — that is plan 2.
