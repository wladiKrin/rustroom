# W5 track C — colour range masks: Implementation Plan

> **For the agent executing this:** use `superpowers:executing-plans`, task by task. Spec:
> `docs/superpowers/specs/2026-09-04-colour-range-mask-design.md`. **Every decision in that spec is
> settled — none is yours to re-open.** If you meet a question it does not answer,
> `PARALLEL-WORK.md` §6 applies: **stop and report**, do not choose.

**Goal:** A fifth `Mask` variant selecting on Oklab hue — the companion to M7.1a's luminance range,
whose own doc comment already says it leaves room for one.

**Architecture:** `ColourRange` mirrors `LuminanceRange` field for field, with two deliberate
differences: **hue wraps**, and a **chroma gate** stops near-neutral pixels being assigned an
arbitrary hue.

**Tech Stack:** Rust; `edit-model`, `pipeline` (stage), `crates/app` (panel).

**Baseline:** `main` at **1896 tests, 53 goldens**, all gates green.

---

## ⚠⚠ Surfaces this track OWNS, and surfaces it must not touch

**Owns, exclusively this wave:** `Mask` variants, `MaskStage`'s analytic half,
`develop/panel/masks.rs`.

**Barred — another track owns these:**
- The **`Edits` field list and its five exhaustive sites** → **track A**. That means `merge_from`'s
  destructure, `EditGroups`, `pipeline::all_edits`, `edit_model::all_nonneutral` and
  `edit_clipboard`'s payload. ⚠⚠ **A `Mask` variant is not an `Edits` field** — you add a variant to
  an enum, not a field to `Edits`, so you should not need any of those five. **If you find yourself
  editing one, stop and report.**
- `pipeline/src/lib.rs` stage order, `PrefixStepKey`, `DisplayRenderingStage` → **track A**
- `render_engine::pack`, `commands.rs`, `menu.rs` → **track B**

⚠ **Goldens are shared with track A.** Rule 3: **append, never insert.**

---

## Build environment — every shell

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

**Conventions:** no `Co-Authored-By` trailer; **never push**; `fmt` + `clippy --all-targets
--all-features -- -D warnings` + `test --all` green before reporting done.

⚠ **Read every exit code from a file, never through a pipe** — `${PIPESTATUS[0]}` is bash, this shell
is zsh.

⚠⚠ **Sabotage discipline. Nine sabotages passed while proving nothing in the two sessions before
this wave, and THREE were the fixture's fault rather than the guard's.** Every time: grep the edit
landed; confirm a `test result:` line appeared; confirm the **named** test failed; check the target
(`-p edit-model --lib`, `-p pipeline --lib`, `-p app --bin Rustroom`); `cargo test` takes a
**substring, not a regex**. **If a sabotage does not fail, suspect the fixture before concluding the
guard is redundant.**

---

## File Structure

| File | Responsibility |
| --- | --- |
| **Modify** `crates/edit-model/src/lib.rs` | `ColourRange`, the `Mask::Colour` variant, coverage. |
| **Modify** `crates/pipeline/src/stages/mask.rs` | `CoverageSource`'s exhaustive match. |
| **Modify** `crates/app/src/develop/panel/masks.rs` | The controls, mirroring the luminance range's. |
| **Modify** `crates/app/src/develop/canvas/mask_overlay.rs` | One arm: draw nothing. |

---

## Task 1: `ColourRange` and its coverage

**Files:** `crates/edit-model/src/lib.rs`

⚠ **Read `LuminanceRange` and its `coverage` first** (around line 846) and mirror them. This track's
value is that it reuses something already learned.

- [ ] **Step 1: Write the failing tests**

```rust
    /// ⚠⚠ **Hue WRAPS and lightness does not — the one place this must not mirror
    /// `LuminanceRange`.** sRGB red sits at 29.23° in Oklab and magenta at 328.36°, both in
    /// `BAND_HUES`, so without wrapping *the single most commonly masked colour is the one the
    /// control cannot express*.
    ///
    /// ⚠ Asserts BOTH halves: that red is covered and that green is not. A wrap bug that selects
    /// everything passes a coverage-only assertion.
    #[test]
    fn a_range_across_zero_selects_red_and_not_everything_else() {
        let r = ColourRange { low: 340.0, high: 20.0, feather: 0.0, min_chroma: 0.01, invert: false };
        assert!(r.coverage_at(350.0, 0.1) > 0.9, "350° is inside a 340→20 band");
        assert!(r.coverage_at(10.0, 0.1) > 0.9, "10° is inside it too");
        assert!(r.coverage_at(150.0, 0.1) < 0.1, "green must NOT be selected");
    }

    /// ⚠⚠ **The chroma gate is load-bearing, not a refinement.** `HslStage` measured **37.8× noise
    /// amplification** on a near-neutral patch without one: a near-neutral pixel's hue is sensor
    /// noise, so neighbouring pixels land in different hues and get different treatment.
    ///
    /// ⚠ The fixture is a NOISY near-neutral, not a perfect grey — a perfect grey has an undefined
    /// hue and passes against a broken gate too.
    #[test]
    fn a_near_neutral_pixel_is_not_selected_however_well_its_hue_matches() {
        let r = ColourRange { low: 0.0, high: 60.0, feather: 0.0, min_chroma: 0.02, invert: false };
        // Hue dead-centre in the band, chroma at sensor-noise level.
        assert!(r.coverage_at(30.0, 0.003) < 0.1, "noise-level chroma was selected");
        assert!(r.coverage_at(30.0, 0.06) > 0.9, "a real colour must still be selected");
    }

    /// ⚠⚠ Probe **ON** the boundary, not either side of it. `RadialGradient`'s
    /// `feather_zero_is_a_hard_edge` probed either side and never on it, and so PASSED with its
    /// guard deleted, because IEEE infinity clamped to the right answer.
    #[test]
    fn feather_zero_is_a_hard_edge_on_the_boundary_itself() {
        let r = ColourRange { low: 0.0, high: 60.0, feather: 0.0, min_chroma: 0.0, invert: false };
        let on = r.coverage_at(60.0, 0.1);
        assert!(on.is_finite(), "coverage on the boundary was {on}");
        assert!(r.coverage_at(59.0, 0.1) > 0.9 && r.coverage_at(61.0, 0.1) < 0.1);
    }

    /// ⚠ Invert flips coverage, as every other shape does.
    #[test]
    fn invert_flips_the_band() {
        let base = ColourRange { low: 0.0, high: 60.0, feather: 0.0, min_chroma: 0.0, invert: false };
        let inv = ColourRange { invert: true, ..base };
        assert!((base.coverage_at(30.0, 0.1) + inv.coverage_at(30.0, 0.1) - 1.0).abs() < 1e-6);
    }

    /// ⚠⚠ Hand-written `Default`, not derived. `LuminanceRange::default`'s comment records why: the
    /// container-level `#[serde(default)]` fills a missing field from the struct's `Default`, so a
    /// derived one gives every partial sidecar `feather: 0`, `min_chroma: 0` and a band of zero
    /// width at 0° — a mask selecting nothing, silently.
    #[test]
    fn the_default_selects_a_recognisable_band_with_a_soft_shoulder() {
        let d = ColourRange::default();
        assert!(d.high > d.low, "a zero-width band selects nothing: {d:?}");
        assert!(d.feather > 0.0);
        assert!(d.min_chroma > 0.0);
    }
```

⚠ `coverage_at(hue_degrees, chroma)` is a test-facing helper on `ColourRange` so the band logic is
testable without building an Oklab pixel. Keep the pixel-facing `coverage` a thin wrapper over it, so
there is one rule rather than two.

- [ ] **Step 2: Run, expect a compile failure naming `ColourRange`.**

```bash
cargo test -p edit-model --lib colour > /tmp/w5c-t1.log 2>&1; echo "EXIT=$?"
grep -E "^error" -A4 /tmp/w5c-t1.log | head -8
```

- [ ] **Step 3: Implement**

```rust
/// A mask selecting on **hue**, the companion to [`LuminanceRange`].
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ColourRange {
    /// Lower edge of the fully-covered band, in Oklab hue degrees 0..=360.
    pub low: f32,
    /// Upper edge. ⚠⚠ May be **less** than `low`, which means the band crosses 0° — see
    /// `a_range_across_zero_selects_red_and_not_everything_else`.
    pub high: f32,
    /// 0 = a hard edge at each end, 100 = the widest ramp.
    ///
    /// ⚠ Named and scaled to match [`LuminanceRange::feather`], [`RadialGradient::feather`] and the
    /// vignette's, so the app keeps one feather idiom rather than four. Smoothstep, as there: a
    /// linear ramp leaves a visible crease where its derivative jumps.
    pub feather: f32,
    /// Below this chroma a pixel is not selected at all.
    ///
    /// ⚠⚠ **Load-bearing, not a refinement.** A near-neutral pixel's hue is sensor noise: `HslStage`
    /// measured chroma ~0.003 at ±0.6% noise against ~0.06 for a real colour, and **37.8× noise
    /// amplification** without a taper. Exposed as a control rather than fixed because what counts
    /// as "coloured enough" genuinely differs between a pastel skin tone and a saturated sign —
    /// which is exactly the judgement HSL's fixed `CHROMA_FADE` cannot make.
    pub min_chroma: f32,
    /// Flip coverage, as every other shape does.
    pub invert: bool,
}
```

⚠ The wrap: normalise `(hue - low).rem_euclid(360)` against `(high - low).rem_euclid(360)`. That one
expression handles both the wrapped and unwrapped cases with no branch — a branch on `low > high` is
a second rule that can disagree with the first.

⚠ `Default` hand-written, per the test above.

- [ ] **Step 4: Run — all five pass.**

- [ ] **Step 5: Sabotage**

| # | Sabotage | Must fail |
| --- | --- | --- |
| 1 | Clamp instead of wrap (`low.min(high)..low.max(high)`) | `a_range_across_zero_selects_red_and_not_everything_else` |
| 2 | Ignore `min_chroma` | `a_near_neutral_pixel_is_not_selected_however_well_its_hue_matches` |
| 3 | Derive `Default` | `the_default_selects_a_recognisable_band_with_a_soft_shoulder` |
| 4 | Ignore `invert` | `invert_flips_the_band` |

- [ ] **Step 6: Commit.**

---

## Task 2: the `Mask` variant

**Files:** `crates/edit-model/src/lib.rs`, `crates/pipeline/src/stages/mask.rs`

- [ ] **Step 1: Append the variant** — `PARALLEL-WORK.md` rule 3, at the **end** of the enum:

```rust
    /// ⚠ A function of the pixel's **hue**, not of its position — see [`ColourRange`]. The
    /// companion `Luminance` anticipated: "leaving room for a colour range beside it".
    Colour(ColourRange),
```

- [ ] **Step 2: Follow the compiler.** It will name every exhaustive match:

- `Mask::analytic_coverage` → return `Some(..)`. ⚠⚠ **This function returns `Option` precisely so a
  new variant cannot silently cover nothing**: a `Mask::Colour => 0.0` arm would compile, run, and
  produce a mask covering nothing.
- `Mask::label`, `Mask::invert_mut`
- `CoverageSource` in `stages/mask.rs`. ⚠⚠ **That match is exhaustive on purpose.** A catch-all arm
  was a live trap this project already removed once: `other => Analytic(other.clone())` sent a
  variant whose `analytic_coverage` returns `None` into `unwrap_or(0.0)` — *a mask covering nothing,
  silently.* **Do not add a catch-all.**
- `mask_overlay.rs` → one arm drawing nothing, beside `Mask::Luminance(_) => {}`.

- [ ] **Step 3: Write the test that the variant reaches the pixels**

```rust
    /// ⚠ The wiring test. `ColourRange` has its own unit tests, but nothing else can see that a
    /// colour mask reaches `MaskStage` and moves pixels — the half this project has repeatedly
    /// shipped broken.
    #[test]
    fn a_colour_range_mask_really_reaches_the_pixels() {
        /* a two-patch image, one red one green; a mask on red with a strong exposure lift;
           assert the red patch moved and the green did not */
    }
```

- [ ] **Step 4: Run the whole suite. ⚠ Existing mask tests must pass VERBATIM** — that is the
  evidence the variant changed nothing — and **all 53 goldens byte-identical**, since
  `Edits::default()` has no masks.

- [ ] **Step 5: Sabotage** — return `None` from `analytic_coverage` for the new variant; the wiring
  test must fail. Revert.

- [ ] **Step 6: Commit.**

---

## Task 3: the panel

**Files:** `crates/app/src/develop/panel/masks.rs`

- [ ] **Step 1:** Add the controls, **mirroring the luminance range's**: Low, High, Feather,
  Min chroma, Invert. Read that block first and follow it.

⚠ The mask-kind selector gains one entry. Append it.

- [ ] **Step 2:** Test through the panel harness that selecting the colour kind draws its controls
  and that a slider settles into one history entry, following `panel/masks.rs`'s existing tests.

- [ ] **Step 3: Commit.**

---

## Task 4: goldens

- [ ] **Step 1:** Append **one** golden: a colour range mask with an exposure lift, on the existing
  `hue_sweep_chart()` — the chart HSL already uses, which is built to exercise hue.

⚠⚠ **Append, never insert** — track A adds goldens this wave too.

⚠⚠ **Measure the generated PNG, do not just look at it.** This project's records: *"a golden nobody
has looked at is just a hash"*, and an exposure golden shipped that **clipped 6 of 20 samples**, so
it could not discriminate the effect it existed for. Assert nothing is pinned at 255 and that a
useful number of samples differ.

- [ ] **Step 2: Commit.**

---

## Task 5: verify

- [ ] `fmt --check`, `clippy --all-targets --all-features -D warnings`, `test --all` — **all three,
  each exit code read from a file.** `cargo test` being green is not evidence about clippy; that has
  caught this project out eight times.
- [ ] `git diff --stat main -- crates/pipeline/src/lib.rs` **empty**, and no diff to `merge_from`,
  `EditGroups`, `all_edits`, `all_nonneutral` or `edit_clipboard`. ⚠ This is the wave-discipline
  check: any diff there means this track entered track A's surface.
- [ ] 53 pre-existing goldens byte-identical; one appended.
- [ ] State the test arithmetic.
- [ ] Report any sabotage that did not fail, **named as such**, and any file over ~600 lines you
  added to.
