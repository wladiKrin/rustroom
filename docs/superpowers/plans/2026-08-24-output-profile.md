# Output profile choice at export — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let an export target sRGB, Display P3, Adobe RGB or ProPhoto instead of always sRGB, now that the working space is wide enough to hold the colour.

**Architecture:** `color` gains an `OutputSpace` enum carrying a matrix and a transfer curve per space. `pipeline`'s final stage takes one as a parameter, with every existing builder delegating with `Srgb` so nothing else changes. `export` builds a real ICC profile per space with lcms2. `ExportOptions` carries the choice, so export presets persist it, and the CLI gets `--space`.

**Tech Stack:** Rust, lcms2 6.1.1, `image` crate encoders.

**Spec:** `docs/superpowers/specs/2026-08-24-output-profile-design.md`

**Before anything:** every build command in this plan needs

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

**Branch first, per this project's standing convention.** Never push; the user pushes.

```bash
git checkout -b output-profile
```

---

## File Structure

| file | responsibility |
|---|---|
| `crates/color/src/spaces.rs` | *modify* — add the two new ProPhoto→ matrices beside the existing one |
| `crates/color/src/output_space.rs` | **create** — the `OutputSpace` enum: matrix, transfer curve, label, 8-bit warning |
| `crates/color/src/lib.rs` | *modify* — declare and re-export the module |
| `crates/pipeline/src/stages/output_transform.rs` | *modify* — `OutputTransformSrgb` → `OutputTransform { space }` |
| `crates/pipeline/src/lib.rs` | *modify* — thread `space` through the deep builder; add `build_pipeline_to` |
| `crates/pipeline/tests/golden.rs` | *modify* — one new `output_p3` case |
| `crates/export/Cargo.toml` | *modify* — depend on `color` |
| `crates/export/src/lib.rs` | *modify* — `srgb_icc()` → `icc_for(space)`; `save` takes a space |
| `crates/app/src/batch_export.rs` | *modify* — `ExportOptions.space`; pass it to the pipeline and to `save` |
| `crates/app/src/modals/export.rs` | *modify* — the dropdown and the banding hint |
| `crates/rustroom/src/space_arg.rs` | **create** — `--space` token parsing |
| `crates/rustroom/src/main.rs` | *modify* — the flag, and pass the space through |

⚠⚠ **Do NOT touch the existing `PROPHOTO_TO_SRGB` constant.** A fresh derivation from published
primaries gives `2.0343808` where the shipped value is `2.0342892` — a ~1e-4 difference from how the
ROMM primaries were rounded. That is *confirmation* the shipped matrix is right, not a correction to
apply. Changing it would move all 38 goldens for no reason.

---

### Task 1: `color::OutputSpace`

**Files:**
- Modify: `crates/color/src/spaces.rs`
- Create: `crates/color/src/output_space.rs`
- Modify: `crates/color/src/lib.rs`

- [ ] **Step 1: Add the two new matrices to `spaces.rs`**

Append after `SRGB_TO_PROPHOTO`:

```rust
/// Linear ProPhoto (ROMM, D50) → linear Display P3 (D65), Bradford-adapted.
///
/// ⚠ Display P3 differs from sRGB **only in its primaries**. Its transfer curve is the sRGB
/// one — see `OutputSpace::encode`.
pub const PROPHOTO_TO_DISPLAY_P3: [[f32; 3]; 3] = [
    [1.6325756087, -0.3797716185, -0.2528039902],
    [-0.1537004023, 1.1667025472, -0.0130021449],
    [0.0103931953, -0.0628073126, 1.0524141174],
];

/// Linear ProPhoto (ROMM, D50) → linear Adobe RGB (1998) (D65), Bradford-adapted.
pub const PROPHOTO_TO_ADOBE_RGB: [[f32; 3]; 3] = [
    [1.3896512482, -0.1694590769, -0.2201921712],
    [-0.2288257316, 1.2317425412, -0.0029168096],
    [-0.0176254437, -0.0962570231, 1.1138824667],
];
```

- [ ] **Step 2: Write the failing tests for the new module**

Create `crates/color/src/output_space.rs` containing only the test module for now, so the tests
fail to compile against a type that does not exist yet:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_space_maps_white_to_white() {
        // ⚠ The check that each matrix carries its Bradford adaptation the right way round.
        // Without it, D50 white lands off-white — a warm cast on every image, subtle enough to
        // ship. ProPhoto's own D50 white point makes this a real check, not a formality.
        for s in OutputSpace::ALL {
            let w = s.from_working([1.0, 1.0, 1.0]);
            for (i, v) in w.iter().enumerate() {
                assert!(
                    (v - 1.0).abs() < 1e-3,
                    "{}: white channel {i} became {v}",
                    s.label()
                );
            }
        }
    }

    #[test]
    fn prophoto_is_bit_exactly_the_identity() {
        // ⚠ Not "close to" the identity. ProPhoto *is* the working space, so exporting it must
        // apply no matrix at all — it is the one lossless option. Routing it through a general
        // matrix path would leave it a ulp off and this assertion is what forbids that.
        for c in [[0.18, 0.5, 0.9], [0.0, 1.0, 0.0], [-0.2, 1.4, 0.03]] {
            assert_eq!(OutputSpace::ProPhoto.from_working(c), c);
        }
    }

    #[test]
    fn display_p3_holds_a_green_srgb_clips() {
        // ⚠ The capability the whole feature exists for, as a test. P3's own green primary is
        // outside sRGB, so converting it to sRGB yields negative components while converting it
        // to P3 gives back exactly the primary. If this fails the feature does nothing, however
        // well the UI is wired.
        let p3_green_in_working = [0.2139, 0.8859, 0.0508];
        let in_srgb = OutputSpace::Srgb.from_working(p3_green_in_working);
        assert!(
            in_srgb.iter().any(|v| *v < -0.05),
            "P3 green should fall outside sRGB, got {in_srgb:?}"
        );
        let in_p3 = OutputSpace::DisplayP3.from_working(p3_green_in_working);
        assert!(
            in_p3[0].abs() < 0.02 && (in_p3[1] - 1.0).abs() < 0.02 && in_p3[2].abs() < 0.02,
            "P3 green should be in gamut in P3, got {in_p3:?}"
        );
    }

    #[test]
    fn display_p3_uses_the_srgb_transfer_curve() {
        // ⚠ The error this exists to prevent is invisible: using gamma 2.2 for P3 gives an image
        // that is off by a few percent in the shadows, which reads as a contrast preference
        // rather than as a bug. Only the primaries differ from sRGB.
        for v in [0.0, 0.002, 0.18, 0.5, 1.0] {
            assert_eq!(
                OutputSpace::DisplayP3.encode(v),
                OutputSpace::Srgb.encode(v),
                "P3 and sRGB must share a curve, differed at {v}"
            );
        }
    }

    #[test]
    fn the_curves_match_their_specifications_at_mid_grey() {
        // Known values, computed from each specification rather than from a second copy of our
        // own implementation — re-deriving in the test proves only that the same mistake was
        // made twice.
        let want = [
            (OutputSpace::Srgb, 0.461356),
            (OutputSpace::DisplayP3, 0.461356),
            (OutputSpace::AdobeRgb, 0.458529),
            (OutputSpace::ProPhoto, 0.385711),
        ];
        for (s, w) in want {
            let got = s.encode(0.18);
            assert!(
                (got - w).abs() < 1e-4,
                "{}: encode(0.18) = {got}, want {w}",
                s.label()
            );
        }
    }

    #[test]
    fn the_prophoto_toe_meets_its_power_segment() {
        // ⚠ ROMM's linear toe below 1/512 and its 1.8 power segment are continuous at the join:
        // both give exactly 0.03125. A discontinuity here would be a visible step in deep
        // shadow, and getting the threshold or the slope wrong produces exactly that.
        let et = 1.0 / 512.0;
        let below = OutputSpace::ProPhoto.encode(et - 1e-6);
        let at = OutputSpace::ProPhoto.encode(et);
        assert!(
            (below - at).abs() < 1e-4 && (at - 0.03125).abs() < 1e-4,
            "toe {below} and power segment {at} should meet at 0.03125"
        );
    }

    #[test]
    fn every_curve_clamps_to_the_encodable_range() {
        for s in OutputSpace::ALL {
            assert_eq!(s.encode(-0.5), 0.0, "{} let a negative through", s.label());
            assert_eq!(s.encode(2.0), 1.0, "{} let a value above 1 through", s.label());
        }
    }

    #[test]
    fn only_prophoto_warns_in_eight_bit() {
        // ⚠ Deliberately narrow. 8-bit Display P3 is what every iPhone ships and 8-bit Adobe RGB
        // is the standard print-delivery format; warning on those is a false alarm, and false
        // alarms train people to dismiss the warning that matters.
        for s in OutputSpace::ALL {
            assert_eq!(
                s.warns_in_8_bit(),
                s == OutputSpace::ProPhoto,
                "{} warned unexpectedly",
                s.label()
            );
        }
    }

    #[test]
    fn every_space_has_a_distinct_label() {
        let mut seen = Vec::new();
        for s in OutputSpace::ALL {
            assert!(!seen.contains(&s.label()), "duplicate label {}", s.label());
            seen.push(s.label());
        }
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p color output_space`
Expected: FAIL — compile error, `cannot find type OutputSpace in this scope`.

- [ ] **Step 4: Write the implementation**

Prepend to `crates/color/src/output_space.rs`, above the test module:

```rust
//! The colour spaces an export can target.
//!
//! ⚠ Each space is a **pair**: a set of primaries (the matrix) and a transfer curve. Mixing one
//! space's primaries with another's curve produces a file that opens, has plausible colour, and
//! is quietly wrong — which is why they live together on one type rather than as two parallel
//! lookups a caller could combine incorrectly.

use crate::spaces::{apply, PROPHOTO_TO_ADOBE_RGB, PROPHOTO_TO_DISPLAY_P3, PROPHOTO_TO_SRGB};

/// Adobe RGB (1998)'s encoding gamma.
///
/// ⚠ **563/256, not 2.2.** The two differ by under 0.1%, but 2.2 is a value the specification
/// does not contain, and a profile claiming to be Adobe RGB should carry Adobe RGB's number.
const ADOBE_RGB_GAMMA: f32 = 563.0 / 256.0;

/// ROMM's linear-segment threshold, 1/512.
const PROPHOTO_TOE_END: f32 = 0.001_953_125;

/// A space an export can be written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum OutputSpace {
    /// The safe default: every browser, every screen, every recipient.
    #[default]
    Srgb,
    /// sRGB's curve with wider primaries. What modern phones and displays use.
    DisplayP3,
    /// The long-standing print-delivery space.
    AdobeRgb,
    /// The working space itself — the only lossless option.
    ProPhoto,
}

impl OutputSpace {
    pub const ALL: [OutputSpace; 4] = [
        OutputSpace::Srgb,
        OutputSpace::DisplayP3,
        OutputSpace::AdobeRgb,
        OutputSpace::ProPhoto,
    ];

    /// Linear ProPhoto working space → this space's linear primaries.
    ///
    /// ⚠ **ProPhoto is the identity**, returned without touching a matrix. It *is* the working
    /// space, so exporting it converts nothing and clips nothing.
    ///
    /// ⚠ Produces negative components for colour outside the target gamut. That is correct here;
    /// [`Self::encode`] is where clipping belongs.
    #[inline]
    pub fn from_working(self, c: [f32; 3]) -> [f32; 3] {
        match self {
            OutputSpace::Srgb => apply(&PROPHOTO_TO_SRGB, c),
            OutputSpace::DisplayP3 => apply(&PROPHOTO_TO_DISPLAY_P3, c),
            OutputSpace::AdobeRgb => apply(&PROPHOTO_TO_ADOBE_RGB, c),
            OutputSpace::ProPhoto => c,
        }
    }

    /// This space's transfer curve. Input is clamped to `[0, 1]`.
    ///
    /// ⚠ Display P3 shares sRGB's curve — only the primaries differ. Using gamma 2.2 there is a
    /// common error and an invisible one.
    #[inline]
    pub fn encode(self, v: f32) -> f32 {
        match self {
            OutputSpace::Srgb | OutputSpace::DisplayP3 => crate::linear_to_srgb(v),
            OutputSpace::AdobeRgb => v.clamp(0.0, 1.0).powf(1.0 / ADOBE_RGB_GAMMA),
            OutputSpace::ProPhoto => {
                let v = v.clamp(0.0, 1.0);
                if v < PROPHOTO_TOE_END {
                    v * 16.0
                } else {
                    v.powf(1.0 / 1.8)
                }
            }
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            OutputSpace::Srgb => "sRGB",
            OutputSpace::DisplayP3 => "Display P3",
            OutputSpace::AdobeRgb => "Adobe RGB",
            OutputSpace::ProPhoto => "ProPhoto",
        }
    }

    /// Whether 8-bit output in this space is worth warning about.
    ///
    /// ⚠⚠ **True for ProPhoto only, and that narrowness is deliberate — do not "complete" this
    /// table.** Banding does scale with gamut volume, but 8-bit Display P3 is what every iPhone
    /// ships by default and 8-bit Adobe RGB has been the standard print-delivery format for
    /// twenty years. A warning that fires on ordinary practice is a false alarm, and false
    /// alarms train people to dismiss the warning that matters. 8-bit ProPhoto is the case
    /// Adobe itself warns against.
    pub fn warns_in_8_bit(self) -> bool {
        matches!(self, OutputSpace::ProPhoto)
    }
}
```

- [ ] **Step 5: Declare the module**

In `crates/color/src/lib.rs`, beside the existing `pub mod spaces;` line:

```rust
pub mod output_space;
```

and beside the existing `pub use spaces::{...};`:

```rust
pub use output_space::OutputSpace;
```

`color` needs `serde` for the derive. Check `crates/color/Cargo.toml` — if `serde` is not listed,
add it under `[dependencies]`:

```toml
serde = { workspace = true }
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p color`
Expected: PASS, all tests including the eight new ones.

- [ ] **Step 7: Sabotage each new test and watch it fail**

⚠ This project's standing rule, and it has caught eleven tests that named a bug they could not
detect. **Confirm each sabotage compiles before believing a pass** — an edit that fails to compile
looks exactly like one that passes when you grep for a test result.

Run each of these, confirm `cargo test -p color` **fails on the named test**, then revert:

| sabotage | must fail |
|---|---|
| `OutputSpace::ProPhoto => apply(&PROPHOTO_TO_SRGB, c)` in `from_working` | `prophoto_is_bit_exactly_the_identity` |
| swap `PROPHOTO_TO_DISPLAY_P3`'s first two rows | `every_space_maps_white_to_white` |
| `DisplayP3 => v.clamp(0.0,1.0).powf(1.0/2.2)` in `encode` | `display_p3_uses_the_srgb_transfer_curve` **and** `the_curves_match_their_specifications_at_mid_grey` |
| `ADOBE_RGB_GAMMA = 2.2` | `the_curves_match_their_specifications_at_mid_grey` |
| `PROPHOTO_TOE_END = 0.01` | `the_prophoto_toe_meets_its_power_segment` |
| `warns_in_8_bit` → `!matches!(self, OutputSpace::Srgb)` | `only_prophoto_warns_in_eight_bit` |

- [ ] **Step 8: Commit**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings
git add crates/color
git commit -m "feat(color): OutputSpace — primaries and transfer curve per export space"
```

---

### Task 2: the output transform takes a space

**Files:**
- Modify: `crates/pipeline/src/stages/output_transform.rs`
- Modify: `crates/pipeline/src/lib.rs`
- Modify: `crates/pipeline/tests/golden.rs`
- Modify: `crates/app/src/render_engine.rs:543`

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `crates/pipeline/src/stages/output_transform.rs`:

```rust
#[test]
fn the_space_decides_the_output_and_srgb_is_unchanged() {
    // ⚠ Two claims in one test, and both matter. The sRGB half is the regression guard: it is
    // the default, so if it moved, all 38 goldens would be wrong in the same invisible way. The
    // P3 half is the capability: a saturated green must come out *differently*, or the parameter
    // is being ignored and every space silently writes sRGB.
    let mut img = ImageBuf::new(1, 1);
    img.set_pixel(0, 0, color::srgb_to_prophoto([0.1, 0.9, 0.2]));

    let srgb = OutputTransform::new(color::OutputSpace::Srgb)
        .apply(img.clone())
        .pixel(0, 0);
    for (i, want) in [0.1_f32, 0.9, 0.2].iter().enumerate() {
        let want = color::linear_to_srgb(*want);
        assert!(
            (srgb[i] - want).abs() < 2e-3,
            "sRGB channel {i}: got {}, want {want}",
            srgb[i]
        );
    }

    let p3 = OutputTransform::new(color::OutputSpace::DisplayP3)
        .apply(img)
        .pixel(0, 0);
    assert!(
        (p3[1] - srgb[1]).abs() > 1e-3,
        "P3 output should differ from sRGB, got {p3:?} vs {srgb:?}"
    );
}

#[test]
fn a_prophoto_export_clips_nothing_in_gamut() {
    // ⚠ ProPhoto is the identity in the matrix, so a working-space value inside [0,1] must reach
    // the encoder untouched — nothing is out of gamut in its own space. Any negative or clipped
    // channel here means a matrix crept back in.
    let mut img = ImageBuf::new(1, 1);
    img.set_pixel(0, 0, [0.05, 0.8, 0.4]);
    let got = OutputTransform::new(color::OutputSpace::ProPhoto)
        .apply(img)
        .pixel(0, 0);
    for (i, v) in [0.05_f32, 0.8, 0.4].iter().enumerate() {
        let want = color::OutputSpace::ProPhoto.encode(*v);
        assert!(
            (got[i] - want).abs() < 1e-6,
            "channel {i}: got {}, want {want}",
            got[i]
        );
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p pipeline output_transform`
Expected: FAIL — `cannot find function new` / `OutputTransform` not found.

- [ ] **Step 3: Replace the stage**

In `crates/pipeline/src/stages/output_transform.rs`, replace the struct and impl (keep the
existing module doc comment, extending it as shown):

```rust
/// Final stage: convert the linear **ProPhoto** working space to the chosen output space and
/// encode it.
///
/// ⚠ This is where the pipeline's gamut is decided. Everything upstream works in ProPhoto, which
/// holds far more saturated colour than sRGB can express; the clipping happens here, once, at the
/// last possible moment — rather than silently in the decode, which is what the old sRGB working
/// space did.
///
/// ⚠ **The preview always passes `Srgb`.** The display is assumed sRGB until a display profile
/// exists, so letting the canvas follow the export choice would make it lie about what is being
/// edited.
pub struct OutputTransform {
    space: color::OutputSpace,
}

impl OutputTransform {
    pub fn new(space: color::OutputSpace) -> Self {
        Self { space }
    }
}

impl PipelineStage for OutputTransform {
    fn name(&self) -> &'static str {
        "output_transform"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        // ⚠ Two steps, and the order is not negotiable: primaries first, in linear light, then
        // the transfer curve. Encoding before converting would apply a matrix to gamma-encoded
        // values, which is meaningless and does not fail loudly.
        //
        // ⚠ Out-of-gamut colour arrives here with negative components — that is the point of a
        // wide working space, not a bug. `encode` clamps them.
        let space = self.space;
        image
            .map_pixels(move |c| space.from_working(c))
            .map_samples(move |v| space.encode(v))
    }
}
```

Then update the two pre-existing tests in that file: replace every `OutputTransformSrgb.apply(` with
`OutputTransform::new(color::OutputSpace::Srgb).apply(`.

⚠ Their assertions stay **verbatim** — unchanged assertions passing is the evidence the sRGB path
did not move.

- [ ] **Step 4: Thread the space through the builders**

In `crates/pipeline/src/lib.rs`:

Change the re-export on line 35:

```rust
pub use stages::output_transform::OutputTransform;
```

Add a fifth parameter to `build_pipeline_split_in_region_with_brush`:

```rust
pub fn build_pipeline_split_in_region_with_brush(
    edits: &Edits,
    scale: RenderScale,
    region: Option<Region>,
    brush: Option<&BrushCoverage>,
    space: color::OutputSpace,
) -> (Pipeline, Pipeline) {
```

and change its last tail push from `tail.push(Box::new(OutputTransformSrgb));` to:

```rust
    tail.push(Box::new(OutputTransform::new(space)));
```

Update its delegating wrapper to pass `Srgb`:

```rust
pub fn build_pipeline_split_in_region(
    edits: &Edits,
    scale: RenderScale,
    region: Option<Region>,
) -> (Pipeline, Pipeline) {
    build_pipeline_split_in_region_with_brush(edits, scale, region, None, color::OutputSpace::Srgb)
}
```

Add the new public builder immediately after `build_pipeline`:

```rust
/// As [`build_pipeline`], writing to `space` rather than sRGB.
///
/// ⚠ Additive with a delegating wrapper — the `Region` precedent, which gave that work zero
/// call-site churn. Only export and `rustroom render` call this; the preview goes through
/// [`build_pipeline`] and stays sRGB, because the display is assumed sRGB until a display
/// profile exists.
pub fn build_pipeline_to(
    edits: &Edits,
    scale: RenderScale,
    space: color::OutputSpace,
) -> Pipeline {
    let (prefix, tail) = build_pipeline_split_in_region_with_brush(edits, scale, None, None, space);
    let mut stages = prefix.into_stages();
    stages.extend(tail.into_stages());
    Pipeline::new(stages)
}
```

- [ ] **Step 5: Fix the one external caller of the deep builder**

`crates/app/src/render_engine.rs:543` calls
`build_pipeline_split_in_region_with_brush(edits, scale, region, Some(&coverage))`. Add the space:

```rust
        pipeline::build_pipeline_split_in_region_with_brush(
            edits,
            scale,
            region,
            Some(&coverage),
            color::OutputSpace::Srgb, // the preview is always sRGB — see `OutputTransform`
        );
```

If `color` is not already a dependency of `app`, add `color = { path = "../color" }` to
`crates/app/Cargo.toml`.

- [ ] **Step 6: Run the whole suite — every golden must be byte-identical**

Run: `cargo test --all`
Expected: PASS, all 714 tests. ⚠ **Every one of the 38 goldens must pass without regeneration.**
If any golden moved, the sRGB path changed and something above is wrong — do **not** run
`UPDATE_GOLDEN=1` to make it green.

- [ ] **Step 7: Add the `output_p3` golden**

In `crates/pipeline/tests/golden.rs`, `check_golden` renders with `build_pipeline`. Add a
space-aware sibling beside it — find the helper at line ~246 and add:

```rust
/// As `check_golden`, rendering to a non-sRGB output space.
///
/// ⚠ The non-default output path has no reference otherwise: every other golden goes through
/// sRGB, so a mistake in the P3 matrix or in the space parameter's plumbing would be invisible.
fn check_golden_in_space(name: &str, edits: &edit_model::Edits, space: color::OutputSpace) {
    let input = chart();
    let rendered = pipeline::build_pipeline_to(edits, RenderScale::FULL, space).run(input);
    compare_or_update(name, &rendered);
}
```

⚠ Match the existing helper's final call exactly — read how `check_golden` writes and compares its
PNG at line ~246 and reuse that same code path rather than inventing a second one.

Add the test case:

```rust
#[test]
fn golden_output_p3() {
    check_golden_in_space(
        "output_p3",
        &edit_model::Edits::default(),
        color::OutputSpace::DisplayP3,
    );
}
```

Add `color = { path = "../color" }` to `crates/pipeline/Cargo.toml`'s `[dev-dependencies]` if the
test target cannot see it (it is already a normal dependency of `pipeline`, so this is likely
unnecessary — check before adding).

- [ ] **Step 8: Generate the golden and check it is not a duplicate**

```bash
UPDATE_GOLDEN=1 cargo test -p pipeline golden_output_p3
cargo test -p pipeline golden_output_p3
cmp crates/pipeline/tests/golden/output_p3.png crates/pipeline/tests/golden/neutral.png && echo "IDENTICAL — BUG" || echo "differs, good"
```

Expected: the generate passes, the re-run passes, and the `cmp` reports **differs**.
⚠ If the two files are identical the space parameter is not reaching the stage, and the golden
would be guarding nothing.

- [ ] **Step 9: Commit**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
git add crates/pipeline crates/app/src/render_engine.rs crates/app/Cargo.toml
git commit -m "feat(pipeline): the output transform takes an output space"
```

---

### Task 3: `export` builds a profile per space

**Files:**
- Modify: `crates/export/Cargo.toml`
- Modify: `crates/export/src/lib.rs`
- Modify: `crates/app/src/batch_export.rs` (caller, passes `Srgb`)
- Modify: `crates/rustroom/src/main.rs` (caller, passes `Srgb`)

⚠ `save`'s signature changes, so its callers must be updated **in the same commit** or the
workspace does not build. They pass `Srgb` here; Tasks 4 and 5 make them a real choice.

- [ ] **Step 1: Add the dependency**

In `crates/export/Cargo.toml`, under `[dependencies]`:

```toml
color = { path = "../color" }
```

- [ ] **Step 2: Write the failing tests**

Add to the `tests` module in `crates/export/src/lib.rs`:

```rust
#[test]
fn every_space_produces_a_distinct_non_empty_profile() {
    // ⚠ `icc_for` falls back to an empty slice if lcms2 fails, so "non-empty" is a real check
    // rather than a formality — a silent failure would ship untagged files that look fine until
    // someone opens them on a wide-gamut display.
    let mut seen: Vec<Vec<u8>> = Vec::new();
    for s in color::OutputSpace::ALL {
        let icc = icc_for(s);
        assert!(!icc.is_empty(), "{} produced no profile", s.label());
        assert!(
            !seen.contains(&icc.to_vec()),
            "{} produced a duplicate profile",
            s.label()
        );
        seen.push(icc.to_vec());
    }
}

#[test]
fn the_profile_agrees_with_our_own_maths() {
    // ⚠⚠ The load-bearing test of this task. It transforms a colour through **lcms2**, from the
    // profile we emit to lcms2's own sRGB, and compares against the result of our own matrix and
    // curve. Two independent implementations of the same transform must agree.
    //
    // This is what catches an inverted parametric tone curve — the trap here, because an ICC
    // curve describes device→linear, the inverse of what `encode` applies. Get it backwards and
    // the file opens, has the right primaries, and is washed out in a way easily mistaken for a
    // rendering preference. No other test here can see it.
    let srgb_profile = lcms2::Profile::new_srgb();
    // A mid-tone with real chroma: a neutral would pass under a wrong matrix, since every sane
    // matrix preserves grey.
    let working = color::srgb_to_prophoto([0.35, 0.55, 0.20]);

    for s in color::OutputSpace::ALL {
        let ours: [f32; 3] = {
            let lin = s.from_working(working);
            [s.encode(lin[0]), s.encode(lin[1]), s.encode(lin[2])]
        };
        let profile = lcms2::Profile::new_icc(icc_for(s)).expect("our profile should parse");
        let t: lcms2::Transform<[f32; 3], [f32; 3]> = lcms2::Transform::new(
            &profile,
            lcms2::PixelFormat::RGB_FLT,
            &srgb_profile,
            lcms2::PixelFormat::RGB_FLT,
            lcms2::Intent::RelativeColorimetric,
        )
        .expect("transform should build");
        let mut through_lcms = [[0.0f32; 3]];
        t.transform_pixels(&[ours], &mut through_lcms);

        let want_srgb = {
            let lin = color::OutputSpace::Srgb.from_working(working);
            [
                color::linear_to_srgb(lin[0]),
                color::linear_to_srgb(lin[1]),
                color::linear_to_srgb(lin[2]),
            ]
        };
        for i in 0..3 {
            assert!(
                (through_lcms[0][i] - want_srgb[i]).abs() < 5e-3,
                "{}: lcms gave {} for channel {i}, our own maths gives {}",
                s.label(),
                through_lcms[0][i],
                want_srgb[i]
            );
        }
    }
}
```

- [ ] **Step 3: Run them to verify they fail**

Run: `cargo test -p export`
Expected: FAIL — `cannot find function icc_for`.

- [ ] **Step 4: Replace `srgb_icc` with `icc_for`**

In `crates/export/src/lib.rs`, replace the `srgb_icc` function with:

```rust
/// The ICC profile for an output space, serialized once per space.
///
/// ⚠ sRGB uses lcms2's own built-in profile, which is canonical; the other three are constructed
/// from primaries, a white point and a transfer curve.
///
/// ⚠⚠ **An ICC tone curve runs device→linear**, which is the *inverse* of what
/// `OutputSpace::encode` applies — hence `1.0 / gamma` and the inverse-form parametric
/// parameters below. Getting this backwards produces a self-consistently wrong file:
/// right primaries, opens fine, washed out. `the_profile_agrees_with_our_own_maths` is the guard.
///
/// Falls back to an empty slice if lcms2 ever fails, because an export that writes an untagged
/// file is far better than one that refuses to write at all.
pub fn icc_for(space: color::OutputSpace) -> &'static [u8] {
    use color::OutputSpace as S;
    static SRGB: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    static P3: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    static ADOBE: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    static PROPHOTO: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();

    match space {
        S::Srgb => SRGB.get_or_init(|| lcms2::Profile::new_srgb().icc().unwrap_or_default()),
        S::DisplayP3 => P3.get_or_init(|| {
            build_icc(
                D65,
                [(0.680, 0.320), (0.265, 0.690), (0.150, 0.060)],
                srgb_curve(),
            )
        }),
        S::AdobeRgb => ADOBE.get_or_init(|| {
            build_icc(
                D65,
                [(0.640, 0.330), (0.210, 0.710), (0.150, 0.060)],
                lcms2::ToneCurve::new(563.0 / 256.0),
            )
        }),
        S::ProPhoto => PROPHOTO.get_or_init(|| {
            build_icc(
                // ⚠ ProPhoto is a **D50** space. The other three are D65; this is not an
                // inconsistency to unify.
                (0.3457, 0.3585),
                [
                    (0.734699, 0.265301),
                    (0.159597, 0.840403),
                    (0.036598, 0.000105),
                ],
                prophoto_curve(),
            )
        }),
    }
    .as_slice()
}

const D65: (f64, f64) = (0.3127, 0.3290);

/// The sRGB transfer curve, in ICC's device→linear direction.
///
/// ICC parametric type 4 is `Y = (aX + b)^g` for `X >= d`, `Y = cX` below.
fn srgb_curve() -> lcms2::ToneCurve {
    lcms2::ToneCurve::new_parametric(
        4,
        &[2.4, 1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.04045],
    )
    .expect("sRGB parametric curve")
}

/// ROMM's transfer curve — gamma 1.8 with a linear toe — in ICC's device→linear direction.
fn prophoto_curve() -> lcms2::ToneCurve {
    // The encoded threshold is 16 × (1/512) = 0.03125, and the toe's device→linear slope is 1/16.
    lcms2::ToneCurve::new_parametric(4, &[1.8, 1.0, 0.0, 1.0 / 16.0, 0.03125])
        .expect("ProPhoto parametric curve")
}

fn build_icc(
    white: (f64, f64),
    primaries: [(f64, f64); 3],
    curve: lcms2::ToneCurve,
) -> Vec<u8> {
    let xy = |(x, y): (f64, f64)| lcms2::CIExyY { x, y, Y: 1.0 };
    let prim = lcms2::CIExyYTRIPLE {
        Red: xy(primaries[0]),
        Green: xy(primaries[1]),
        Blue: xy(primaries[2]),
    };
    lcms2::Profile::new_rgb(&xy(white), &prim, &[&curve, &curve, &curve])
        .and_then(|p| p.icc())
        .unwrap_or_default()
}
```

- [ ] **Step 5: Give `save` a space**

Change `save`'s signature and every `srgb_icc()` call inside it:

```rust
pub fn save(
    image: &ImageBuf,
    path: &Path,
    format: ExportFormat,
    quality: u8,
    space: color::OutputSpace,
) -> Result<(), ExportError> {
```

and replace all five occurrences of `enc.set_icc_profile(srgb_icc().to_vec())` with
`enc.set_icc_profile(icc_for(space).to_vec())`.

- [ ] **Step 6: Update the two callers to pass `Srgb`**

`crates/app/src/batch_export.rs` — find the `export::save(` call and add
`color::OutputSpace::Srgb` as the final argument.

`crates/rustroom/src/main.rs:148` — change

```rust
    export::save(&rendered, output, format, quality)
```

to

```rust
    export::save(&rendered, output, format, quality, color::OutputSpace::Srgb)
```

Add `color = { path = "../color" }` to `crates/rustroom/Cargo.toml` if it is not already there.

Also fix any existing `export` test that calls `save` — grep for `save(` in
`crates/export/src/lib.rs`'s test module and add `color::OutputSpace::Srgb` to each.

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test --all`
Expected: PASS. ⚠ All 38 goldens still byte-identical — nothing about rendering changed here.

⚠ If `the_profile_agrees_with_our_own_maths` fails for ProPhoto or sRGB but not Adobe RGB, the
parametric curve direction is inverted. Try the forward form (`g` instead of `1/g` conceptually —
i.e. swap whether the exponent is the encode or the decode gamma) and re-run; the test tells you
which is right, which is exactly why it exists.

- [ ] **Step 8: Sabotage and watch it fail**

| sabotage | must fail |
|---|---|
| P3's primaries → sRGB's `(0.640,0.330),(0.300,0.600),(0.150,0.060)` | `the_profile_agrees_with_our_own_maths` (and `every_space_produces_a_distinct_non_empty_profile`) |
| ProPhoto's white → `D65` | `the_profile_agrees_with_our_own_maths` |
| `lcms2::ToneCurve::new(256.0 / 563.0)` for Adobe RGB | `the_profile_agrees_with_our_own_maths` |

Confirm each compiles before trusting the result, then revert.

- [ ] **Step 9: Commit**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
git add -A
git commit -m "feat(export): a real ICC profile per output space"
```

---

### Task 4: the choice reaches the GUI

**Files:**
- Modify: `crates/app/src/batch_export.rs`
- Modify: `crates/app/src/modals/export.rs`

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module in `crates/app/src/batch_export.rs`:

```rust
#[test]
fn an_options_file_from_before_the_space_still_loads() {
    // ⚠ `SettingsStore::load()` is infallible, so a field that fails to parse silently reverts
    // *every* setting. `#[serde(default)]` on the new field is what prevents that, and this is
    // the guard — the same one `view_demosaic` and `export` each needed.
    let json = r#"{"dest_dir":"/tmp","quality":90,"max_edge":null,
                   "demosaic":"Fast","format":"Jpeg"}"#;
    let opts: ExportOptions = serde_json::from_str(json).expect("older options should load");
    assert_eq!(opts.space, color::OutputSpace::Srgb);
    assert_eq!(opts.quality, 90);
}

#[test]
fn the_banding_hint_appears_only_for_prophoto_in_eight_bit() {
    // ⚠ The condition is a pure function so it is testable without a dialog. It fires on
    // ProPhoto alone — see `OutputSpace::warns_in_8_bit` for why widening it would be wrong.
    use export::ExportFormat as F;
    use color::OutputSpace as S;
    assert!(banding_warning(S::ProPhoto, F::Jpeg).is_some());
    assert!(banding_warning(S::ProPhoto, F::Png8).is_some());
    assert!(banding_warning(S::ProPhoto, F::Tiff8).is_some());
    assert!(banding_warning(S::ProPhoto, F::Tiff16).is_none());
    assert!(banding_warning(S::ProPhoto, F::Png16).is_none());
    for f in F::ALL {
        for s in [S::Srgb, S::DisplayP3, S::AdobeRgb] {
            assert!(
                banding_warning(s, f).is_none(),
                "{} + {:?} should not warn",
                s.label(),
                f
            );
        }
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p rustroom_app batch_export`
Expected: FAIL — no field `space`, no function `banding_warning`.

(If the package name differs, use `cargo test -p app batch_export` — check `crates/app/Cargo.toml`.)

- [ ] **Step 3: Add the field and the warning**

In `crates/app/src/batch_export.rs`, add to `ExportOptions` after `format`:

```rust
    /// The colour space the file is written in.
    ///
    /// ⚠ Export-only. The preview stays sRGB, because the display is assumed sRGB until a
    /// display profile exists — see `pipeline::OutputTransform`.
    #[serde(default)]
    pub space: color::OutputSpace,
```

and to its `Default` impl:

```rust
            space: color::OutputSpace::default(),
```

Add the pure condition beside it:

```rust
/// The banding warning for a space/format pair, or `None` when there is nothing to say.
///
/// ⚠ **Warn, never block.** A blocked combination reads as "the thing I didn't click went dark",
/// which this project has already shipped once and taken back out. The state this warns about is
/// one click from being fixed.
pub fn banding_warning(
    space: color::OutputSpace,
    format: export::ExportFormat,
) -> Option<&'static str> {
    let eight_bit = matches!(
        format,
        export::ExportFormat::Jpeg | export::ExportFormat::Png8 | export::ExportFormat::Tiff8
    );
    (space.warns_in_8_bit() && eight_bit).then_some(
        "ProPhoto in 8-bit will band on smooth gradients — use 16-bit TIFF or PNG.",
    )
}
```

- [ ] **Step 4: Pass the space to the render and the encoder**

In the same file, `export_one` (around line 173) renders with
`pipeline::build_pipeline(edits, scale)`. Change it to:

```rust
    let rendered = pipeline::build_pipeline_to(edits, scale, opts.space).run(scaled);
```

⚠ Check the surrounding function actually has `opts` in scope under that name; if it takes the
fields individually, thread `space: color::OutputSpace` in as a parameter and pass `opts.space` at
the call site. Then change its `export::save(..., color::OutputSpace::Srgb)` to `..., opts.space)`.

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p rustroom_app batch_export`
Expected: PASS.

- [ ] **Step 6: Add the dropdown and the hint to the dialog**

In `crates/app/src/modals/export.rs`, immediately after the Format `ui.horizontal` block and
before the quality slider, insert:

```rust
        ui.horizontal(|ui| {
            ui.label("Colour space");
            egui::ComboBox::from_id_salt("space_pick")
                .selected_text(opts.space.label())
                .show_ui(ui, |ui| {
                    for s in color::OutputSpace::ALL {
                        ui.selectable_value(&mut opts.space, s, s.label());
                    }
                });
        });
        // ⚠ Warn, never block — see `banding_warning`. Placed under the two controls it is
        // about, so the fix ("use 16-bit") is one glance from the Format dropdown.
        if let Some(msg) = crate::batch_export::banding_warning(opts.space, opts.format) {
            theme::hint(ui, msg);
        }
```

- [ ] **Step 7: Verify it builds and the suite is green**

Run: `cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all`
Expected: PASS, all goldens byte-identical.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat(app): choose the export colour space, with an 8-bit banding warning"
```

---

### Task 5: `rustroom render --space`

**Files:**
- Create: `crates/rustroom/src/space_arg.rs`
- Modify: `crates/rustroom/src/main.rs`

⚠ **`rustroom` is a binary crate**, so a `pub fn` with no production caller is dead code that
fails `clippy -D warnings`. **The parsing and its wiring land in one commit** — this trap has bitten
this project five times.

- [ ] **Step 1: Write the failing tests**

Create `crates/rustroom/src/space_arg.rs`:

```rust
//! Parsing `--space` for `rustroom render`.
//!
//! A plain string plus a lookup rather than a `clap::ValueEnum` derive, so `color` does not gain
//! a `clap` dependency for a CLI concern — the same trade `--format` and `--demosaic` make. The
//! error lists the valid values, which is what the derive would have bought.

use anyhow::{bail, Result};
use color::OutputSpace;

/// The tokens `--space` accepts, in the order the error message lists them.
const TOKENS: [(&str, OutputSpace); 4] = [
    ("srgb", OutputSpace::Srgb),
    ("p3", OutputSpace::DisplayP3),
    ("adobergb", OutputSpace::AdobeRgb),
    ("prophoto", OutputSpace::ProPhoto),
];

/// `srgb|p3|adobergb|prophoto`, for an error message.
///
/// Built from `TOKENS` rather than written out, so a space added to one is never missing from
/// the other.
fn token_list() -> String {
    TOKENS.iter().map(|(t, _)| *t).collect::<Vec<_>>().join("|")
}

/// Parse a `--space` token. Case-insensitive.
pub fn parse(token: &str) -> Result<OutputSpace> {
    let lower = token.to_ascii_lowercase();
    match TOKENS.iter().find(|(t, _)| *t == lower) {
        Some((_, s)) => Ok(*s),
        None => bail!("unknown space {token:?} ({})", token_list()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_space_has_exactly_one_token() {
        // ⚠ A table over `OutputSpace::ALL`, not a list of four asserts: a fifth space added
        // later must be unreachable from the CLI *loudly*, by failing here, rather than silently.
        for s in OutputSpace::ALL {
            let hits: Vec<_> = TOKENS.iter().filter(|(_, t)| *t == s).collect();
            assert_eq!(hits.len(), 1, "{} has {} tokens", s.label(), hits.len());
        }
    }

    #[test]
    fn tokens_parse_and_are_case_insensitive() {
        assert_eq!(parse("srgb").unwrap(), OutputSpace::Srgb);
        assert_eq!(parse("P3").unwrap(), OutputSpace::DisplayP3);
        assert_eq!(parse("AdobeRGB").unwrap(), OutputSpace::AdobeRgb);
        assert_eq!(parse("prophoto").unwrap(), OutputSpace::ProPhoto);
    }

    #[test]
    fn an_unknown_token_is_refused_and_lists_the_valid_ones() {
        let err = parse("rec2020").unwrap_err().to_string();
        assert!(err.contains("rec2020"), "error should quote the input: {err}");
        for (t, _) in TOKENS {
            assert!(err.contains(t), "error should list {t}: {err}");
        }
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p rustroom space_arg`
Expected: FAIL — `file not found for module space_arg` (it is not declared yet).

- [ ] **Step 3: Declare and wire it**

In `crates/rustroom/src/main.rs`, beside `mod format_arg;`:

```rust
mod space_arg;
```

Add the flag to the `render` subcommand's `clap` definition, beside `--format` and `--quality`
(match the surrounding style exactly — read the existing `format` field's attribute and mirror it):

```rust
        /// Output colour space: srgb|p3|adobergb|prophoto
        #[arg(long)]
        space: Option<String>,
```

Thread it into `render`'s signature and body:

```rust
fn render(
    input: &Path,
    output: &Path,
    edits_path: Option<&Path>,
    format: Option<&str>,
    quality: Option<u8>,
    demosaic: &str,
    space: Option<&str>,
) -> Result<()> {
```

Right after the existing `let format = format_arg::resolve(output, format)?;` line — so a bad token
still fails in milliseconds rather than after a multi-second decode:

```rust
    let space = match space {
        Some(s) => space_arg::parse(s)?,
        None => color::OutputSpace::Srgb,
    };
    if space.warns_in_8_bit() && !matches!(format, export::ExportFormat::Png16 | export::ExportFormat::Tiff16) {
        eprintln!(
            "warning: {} in 8-bit will band on smooth gradients — use png16 or tiff16",
            space.label()
        );
    }
```

Change the render and save calls:

```rust
    let rendered =
        pipeline::build_pipeline_to(&edits, pipeline::RenderScale::FULL, space).run(decoded.image);

    export::save(&rendered, output, format, quality, space)
```

And update the call site in `main()` to pass `space.as_deref()`.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p rustroom`
Expected: PASS.

- [ ] **Step 5: Verify against a real file**

```bash
cargo run --release -p rustroom -- render <path-to.RAF> /tmp/p3.tif --format tiff16 --space p3
sips -g profile -g bitsPerSample /tmp/p3.tif
cargo run --release -p rustroom -- render <path-to.RAF> /tmp/srgb.tif --format tiff16 --space srgb
sips -g profile /tmp/srgb.tif
cargo run --release -p rustroom -- render <path-to.RAF> /tmp/bad.jpg --space prophoto
```

Expected: the P3 file reports a P3-ish profile name and `bitsPerSample: 16`; the sRGB file reports
sRGB; the third prints the banding warning to stderr and still writes the file.
⚠ If `sips` reports the same profile for both, the space is not reaching the encoder.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
git add -A
git commit -m "feat(cli): rustroom render --space"
```

---

### Task 6: verify in the app, then update the docs

- [ ] **Step 1: Full green check**

```bash
cargo fmt --all -- --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
```

Expected: PASS. ⚠ **All 38 pre-existing goldens byte-identical, plus `output_p3` = 39 total.**

- [ ] **Step 2: Launch the GUI and check it by hand**

```bash
cargo run --release -p app
```

Check, in order:
1. Export dialog shows a **Colour space** dropdown with four entries.
2. Selecting **ProPhoto** with JPEG shows the banding hint; switching format to **TIFF 16-bit**
   makes it disappear; selecting **Display P3** with JPEG shows nothing.
3. Export the same photo twice, once sRGB and once Display P3, both TIFF 16-bit. Run
   `sips -g profile` on each — the profiles must differ.
4. Save an export preset with Display P3 selected, switch to sRGB, reload the preset — it comes
   back as Display P3.

- [ ] **Step 3: Compare the two exports numerically**

```bash
sips -g profile /path/to/srgb.tif /path/to/p3.tif
```

⚠ Do not judge this by eye on an sRGB display. The P3 file's *numbers* are lower-saturation for the
same colour (a wider container needs smaller coordinates); it only looks right in a colour-managed
viewer. Trust the profile tag and the byte difference, not the appearance.

- [ ] **Step 4: Update `CLAUDE.md`**

Add a section for this feature under "Current status", following the house style of the sections
around it: what landed, the ⚠ findings (Display P3 sharing sRGB's curve; Adobe RGB's 563/256; the
ICC device→linear curve direction and the lcms cross-check that guards it; the deliberately narrow
banding warning), the test count, and the accepted caveats from the spec's "What this does not do".
Update the **▶ NEXT ACTION** block: B is done, remaining are **D** (display profile),
**C** (camera input profiles), **M3.2** lens corrections, **M3.4** virtual copies.

- [ ] **Step 5: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: record the output profile choice at export"
```

⚠ **Do not push.** The user pushes.

---

## Self-Review

**Spec coverage:** four spaces (Task 1) ✓; ProPhoto identity (Task 1, `assert_eq!`) ✓; P3 sRGB
curve and Adobe 563/256 (Task 1) ✓; `OutputSpace` in `color` with `export` depending on it (Tasks
1, 3) ✓; output transform parameterised with delegating wrappers and preview staying sRGB (Task 2)
✓; ICC built per space with the lcms cross-check (Task 3) ✓; `ExportOptions.space` with serde
default, persisted by presets (Task 4) ✓; dropdown and ProPhoto-only banding warning (Task 4) ✓;
`--space` (Task 5) ✓; `output_p3` golden and 38 unchanged (Tasks 2, 6) ✓; caveats recorded (Task 6)
✓.

**Type consistency:** `OutputSpace::{from_working, encode, label, warns_in_8_bit, ALL}` are used
with those exact names in every later task. `OutputTransform::new(space)` matches its use in
`build_pipeline_split_in_region_with_brush` and the tests. `icc_for(space)` matches its five uses
in `save`. `banding_warning(space, format)` matches its use in the dialog. `build_pipeline_to` is
called in Tasks 4 and 5 exactly as defined in Task 2.
