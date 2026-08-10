# Texture & Clarity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add bidirectional Texture (fine band) and Clarity (coarse band) local-contrast controls as one pipeline stage, without changing any existing rendered pixel.

**Architecture:** One `LocalContrastStage` in the pipeline *tail*, after the tone curve and before saturation. It is the same unsharp-mask shape as `SharpenStage` — `d = luma − blur(luma)` on Rec.709 luma in linear light, re-applied as a scalar RGB gain — with two blurs at two radii, both measured against the **same** source luma and summed so the sliders are exactly additive. Clarity's 40 px radius needs a new O(1) `spatial::wide_blur` (three iterated box passes) and pushes the widest tail kernel from 15 px to 120 px, which forces `REGION_MARGIN` up and replaces the two ad-hoc uses of sharpening's `max_kernel_radius` with a pair of pipeline-level functions.

**Tech Stack:** Rust, `rayon`, the existing `PipelineStage` / `RenderScale` / `Region` seams, `egui` for the two sliders.

**Spec:** `docs/superpowers/specs/2026-08-10-texture-clarity-design.md`

---

## Build environment

Every `cargo` command that touches `crates/app` needs libraw on the pkg-config path. Run this once per shell:

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:$PKG_CONFIG_PATH"
```

`core-image`, `edit-model` and `pipeline` build without it.

## File structure

| File | Change | Responsibility |
|---|---|---|
| `crates/pipeline/src/spatial.rs` | modify | gains `wide_blur`, `wide_blur_radius`, `wide_blur_reach` — the O(1) large-σ blur, alongside the existing `box_blur` and `gaussian_blur` |
| `crates/edit-model/src/lib.rs` | modify | gains `LocalContrast`, the `Edits::local_contrast` field, and its `merge_from` group (Tone) |
| `crates/pipeline/src/stages/local_contrast.rs` | **create** | the stage: two bands, one gain, plus `kernel_reach` |
| `crates/pipeline/src/stages/mod.rs` | modify | declare the module |
| `crates/pipeline/src/lib.rs` | modify | insert the stage in the tail; add `tail_kernel_radius` / `max_tail_kernel_radius` |
| `crates/app/src/view_region.rs` | modify | `REGION_MARGIN` 128 → 192; invariant test uses `max_tail_kernel_radius` |
| `crates/app/src/render_engine.rs` | modify | region-equality test: inset from `tail_kernel_radius`, edit set gains local contrast, buffer sized from the inset |
| `crates/pipeline/tests/golden.rs` | modify | new 256×256 chart + 4 cases |
| `crates/pipeline/tests/golden/*.png` | create ×4 | the new references |
| `crates/app/src/main.rs` | modify | two sliders |
| `CLAUDE.md` | modify | status section |

---

## Task 1: `spatial::wide_blur`

Clarity's σ=40 makes a Gaussian unaffordable (241 taps × 2 passes × 26 Mpx ≈ 13 G ops per export). Three iterated box passes are O(1) in σ and converge to a Gaussian.

**Files:**
- Modify: `crates/pipeline/src/spatial.rs`

- [ ] **Step 1: Write the failing tests**

Append inside `mod tests` in `crates/pipeline/src/spatial.rs`:

```rust
    #[test]
    fn wide_blur_radius_solves_the_three_pass_variance() {
        // Three box passes of radius r have variance 3*((2r+1)^2 - 1)/12 = ((2r+1)^2 - 1)/4,
        // so matching a target sigma means (2r+1)^2 = 1 + 4*sigma^2.
        assert_eq!(wide_blur_radius(40.0), 40); // (sqrt(6401) - 1)/2 = 39.5 -> 40
        assert_eq!(wide_blur_reach(40.0), 120);
        // Below sigma ~= 0.87 an integer box radius cannot represent the blur at all, so
        // there is deliberately no `.max(1)`: clamping up would render sigma 0.1 as an
        // effective sigma of 0.82, eight times too wide.
        assert_eq!(wide_blur_radius(0.1), 0);
        assert_eq!(wide_blur_radius(0.0), 0);
        assert_eq!(wide_blur_radius(-3.0), 0);
        assert_eq!(wide_blur_radius(f32::NAN), 0);
    }

    #[test]
    fn wide_blur_below_the_representable_sigma_is_identity() {
        let p = vec![1.0, 2.0, 3.0, 4.0];
        assert_eq!(wide_blur(&p, 2, 2, 0.1), p);
        assert_eq!(wide_blur(&p, 2, 2, 0.0), p);
    }

    #[test]
    fn wide_blur_constant_plane_stays_constant() {
        // Inherited from box_blur: the *window* clamps at the border, not the samples.
        let p = vec![0.375f32; 41 * 33];
        let out = wide_blur(&p, 41, 33, 4.0);
        for (i, v) in out.iter().enumerate() {
            assert!((v - 0.375).abs() < 1e-6, "sample {i} = {v}");
        }
    }

    #[test]
    fn wide_blur_matches_the_requested_sigma() {
        // A separable filter's centre row is proportional to its 1D kernel, so normalizing
        // it and taking the second moment recovers the realised sigma directly. This is what
        // makes "approximates a Gaussian" a measured claim rather than a comment.
        const N: usize = 121;
        const SIGMA: f32 = 8.0;
        let mut p = vec![0.0f32; N * N];
        p[(N / 2) * N + N / 2] = 1.0;
        let out = wide_blur(&p, N, N, SIGMA);

        let row = &out[(N / 2) * N..(N / 2 + 1) * N];
        let sum: f32 = row.iter().sum();
        let var: f32 = row
            .iter()
            .enumerate()
            .map(|(x, &w)| {
                let d = x as f32 - (N / 2) as f32;
                w / sum * d * d
            })
            .sum();
        let realised = var.sqrt();

        // Compare against the sigma the *rounded* radius actually encodes, not the requested
        // one — sqrt(((2r+1)^2 - 1)/4). At sigma 8 the radius rounds 7.52 up to 8, which is
        // a realised sigma of 8.49; asserting 8.0 to within a few percent would fail for a
        // correct implementation.
        let r = wide_blur_radius(SIGMA);
        let modelled = (((2 * r + 1).pow(2) - 1) as f32 / 4.0).sqrt();
        assert!(
            (realised - modelled).abs() / modelled < 0.03,
            "realised sigma {realised} vs modelled {modelled}"
        );
        // Rounding the radius to an integer is the only source of error, and it is bounded.
        assert!(
            (realised - SIGMA).abs() / SIGMA < 0.10,
            "realised sigma {realised} is too far from the requested {SIGMA}"
        );
    }

    #[test]
    fn wide_blur_impulse_response_is_a_symmetric_bell() {
        // Rules out an off-by-one in the iteration, which a variance check alone would miss.
        const N: usize = 81;
        let mut p = vec![0.0f32; N * N];
        p[(N / 2) * N + N / 2] = 1.0;
        let out = wide_blur(&p, N, N, 6.0);
        let c = N / 2;
        let peak = out[c * N + c];
        for y in 0..N {
            for x in 0..N {
                if (x, y) != (c, c) {
                    assert!(out[y * N + x] < peak, "({x},{y}) exceeds the centre");
                }
                assert!((out[y * N + x] - out[y * N + (N - 1 - x)]).abs() < 1e-6);
                assert!((out[y * N + x] - out[(N - 1 - y) * N + x]).abs() < 1e-6);
            }
        }
        // Monotone away from the centre along the centre row.
        for x in c..N - 1 {
            assert!(out[c * N + x] >= out[c * N + x + 1] - 1e-7, "not monotone at {x}");
        }
    }

    #[test]
    fn three_box_passes_are_much_closer_to_a_gaussian_than_one() {
        // Pins the *number* of iterations, not just the radius. One box pass is a
        // flat-topped kernel with hard shoulders; three passes are a quadratic B-spline,
        // within a few percent of a Gaussian everywhere.
        //
        // A direct peak-height comparison against a Gaussian at the *requested* sigma would
        // be wrong — the rounded radius makes wide_blur's realised sigma 6% wider, which is
        // a 12% lower peak in 2D. Comparing normalized profiles against the same reference
        // sidesteps that entirely.
        const N: usize = 121;
        const SIGMA: f32 = 8.0;
        let mut p = vec![0.0f32; N * N];
        p[(N / 2) * N + N / 2] = 1.0;

        let gauss = gaussian_blur(&p, N, N, SIGMA);
        let wide = wide_blur(&p, N, N, SIGMA);
        let once = box_blur(&p, N, N, wide_blur_radius(SIGMA));

        // A separable filter's centre row is proportional to its 1D kernel.
        let profile = |v: &[f32]| {
            let row = &v[(N / 2) * N..(N / 2 + 1) * N];
            let s: f32 = row.iter().sum();
            row.iter().map(|w| w / s).collect::<Vec<f32>>()
        };
        let g = profile(&gauss);
        let dev = |a: &[f32]| {
            a.iter()
                .zip(&g)
                .map(|(x, y)| (x - y).abs())
                .fold(0.0f32, f32::max)
        };
        let d1 = dev(&profile(&once));
        let d3 = dev(&profile(&wide));
        assert!(d3 * 2.0 < d1, "three passes {d3} vs one pass {d1}");

        let peak = g.iter().fold(0.0f32, |a, &b| a.max(b));
        assert!(
            d3 < 0.15 * peak,
            "three passes deviate {d3} from a Gaussian whose peak is {peak}"
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p pipeline --lib spatial 2>&1 | tail -20
```

Expected: compile error, `cannot find function 'wide_blur' in this scope` (and `wide_blur_radius`, `wide_blur_reach`).

- [ ] **Step 3: Implement**

Append to `crates/pipeline/src/spatial.rs`, after `gauss_cols` and before `mod tests`:

```rust
/// Number of box-blur passes [`wide_blur`] uses. Three is where the iterated box response
/// gets within a few percent of a true Gaussian; the cost is linear in this and the accuracy
/// gain past three is not worth a fourth pass over the data.
const WIDE_PASSES: usize = 3;

/// Box radius whose [`WIDE_PASSES`]-fold iteration has variance `sigma^2`.
///
/// Each pass of radius `r` has variance `((2r+1)^2 - 1)/12`, so three of them give
/// `((2r+1)^2 - 1)/4 = sigma^2`, i.e. `r = (sqrt(1 + 4*sigma^2) - 1)/2`.
///
/// Returns 0 — meaning "identity", exactly as [`kernel_radius`] does — for a non-positive,
/// non-finite, or **too small** sigma. There is deliberately no `.max(1)`: a box radius of 1
/// is an effective sigma of 0.82, so clamping up would render `sigma = 0.1` eight times too
/// wide. Below sigma ~= 0.87 an integer box radius cannot represent the blur at all, which
/// is precisely why [`gaussian_blur`] exists for the small-sigma regime.
pub fn wide_blur_radius(sigma: f32) -> usize {
    if !sigma.is_finite() || sigma <= 0.0 {
        return 0;
    }
    let r = ((1.0 + 4.0 * sigma * sigma).sqrt() - 1.0) / 2.0;
    r.round().max(0.0) as usize
}

/// The furthest an output sample of [`wide_blur`] can reach into its input.
///
/// Public because callers must reason about it: a stage using this in the pipeline *tail*
/// runs on a cropped region, so the renderer has to keep at least this much margin outside
/// what it displays. See `pipeline::tail_kernel_radius`.
pub fn wide_blur_reach(sigma: f32) -> usize {
    WIDE_PASSES * wide_blur_radius(sigma)
}

/// Gaussian-like blur for **large** sigma, built from iterated [`box_blur`] passes.
///
/// The third primitive in this module, and the three cover three regimes:
///
/// | primitive | regime | why |
/// |---|---|---|
/// | [`gaussian_blur`] | sub-pixel to a few px | a direct kernel is cheap there (7 taps at σ=1), and an integer box radius cannot express a sub-pixel blur at all |
/// | [`wide_blur`] | tens of px | O(1) in sigma, and at that scale the box-iteration error is invisible |
/// | [`box_blur`] | any, when a genuine box is wanted | the guided filter in noise reduction needs box means, not Gaussian ones |
///
/// A Gaussian at sigma = 40 would need 241 taps per pass — about 13 G operations on a
/// 26 Mpx export. This is three passes of running sums regardless of sigma.
///
/// Inherits `box_blur`'s border behaviour (the *window* clamps, not the samples), so a
/// constant plane stays exactly constant. Returns a copy when the sigma is too small to
/// represent — see [`wide_blur_radius`].
///
/// # Panics
/// Panics if `plane.len() != w * h`.
pub fn wide_blur(plane: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    assert_eq!(plane.len(), w * h, "plane size mismatch");
    let r = wide_blur_radius(sigma);
    if r == 0 || w == 0 || h == 0 {
        return plane.to_vec();
    }
    let mut out = box_blur(plane, w, h, r);
    for _ in 1..WIDE_PASSES {
        out = box_blur(&out, w, h, r);
    }
    out
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p pipeline --lib spatial 2>&1 | tail -20
```

Expected: `test result: ok.` with all `wide_blur_*` tests passing alongside the existing `box_blur`/`gaussian_blur` ones.

- [ ] **Step 5: Commit**

```bash
git add crates/pipeline/src/spatial.rs
git commit -m "feat(pipeline): add spatial::wide_blur, an O(1) large-sigma blur"
```

---

## Task 2: `LocalContrast` in `edit-model`

**Files:**
- Modify: `crates/edit-model/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Append inside `mod tests` in `crates/edit-model/src/lib.rs`:

```rust
    #[test]
    fn local_contrast_defaults_to_off() {
        let lc = LocalContrast::default();
        assert_eq!(lc.texture, 0.0);
        assert_eq!(lc.clarity, 0.0);
        assert_eq!(Edits::default().local_contrast, lc);
    }

    #[test]
    fn local_contrast_is_active_when_either_slider_moves() {
        assert!(!LocalContrast::default().is_active());
        assert!(LocalContrast { texture: 1.0, clarity: 0.0 }.is_active());
        assert!(LocalContrast { texture: 0.0, clarity: -1.0 }.is_active());
    }

    #[test]
    fn old_sidecar_without_local_contrast_still_loads() {
        let e: Edits = serde_json::from_str(r#"{"exposure": 0.5}"#).unwrap();
        assert_eq!(e.local_contrast, LocalContrast::default());
    }

    #[test]
    fn local_contrast_roundtrips_json() {
        let e = Edits {
            local_contrast: LocalContrast {
                texture: 35.0,
                clarity: -60.0,
            },
            ..Default::default()
        };
        let json = serde_json::to_string(&e).unwrap();
        let back: Edits = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
    }

    #[test]
    fn local_contrast_travels_with_the_tone_group() {
        // Texture and Clarity are *look*, which is what a preset carries; Detail is
        // capture-specific (noise, lens, output size). The generic partition tests above
        // prove disjointness and coverage — this pins which group it landed in.
        let src = all_nonneutral();
        let mut got = Edits::default();
        got.merge_from(&src, EditGroups::only(1)); // 1 == "Tone"
        assert_eq!(got.local_contrast, src.local_contrast);
        let mut not_detail = Edits::default();
        not_detail.merge_from(&src, EditGroups::only(3)); // 3 == "Detail"
        assert_eq!(not_detail.local_contrast, LocalContrast::default());
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p edit-model 2>&1 | tail -20
```

Expected: compile error, `cannot find type 'LocalContrast' in this scope`.

- [ ] **Step 3: Add the type**

Insert into `crates/edit-model/src/lib.rs` immediately after the `impl Sharpening { ... }` block (before the `EDIT_GROUP_NAMES` const):

```rust
/// Texture and Clarity: local contrast in two frequency bands. Both 0 = off.
///
/// Same operation as [`Sharpening`] at a different radius, which is why they share a stage
/// shape but not a struct: these are *look* controls that belong with tone, while sharpening
/// is capture- and output-specific.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct LocalContrast {
    /// Fine-band local contrast, ~[-100, 100]. Positive brings out surface detail, negative
    /// smooths it. 0 = off.
    pub texture: f32,
    /// Coarse-band local contrast, ~[-100, 100]. Positive is punchy midtone contrast,
    /// negative is a glow. 0 = off.
    pub clarity: f32,
}

impl LocalContrast {
    /// True when these settings actually change pixels.
    ///
    /// There are no modifier fields here, so this could have been `!= default()`. Written
    /// the same way [`NoiseReduction::is_active`], [`Vignette::is_active`] and
    /// [`Sharpening::is_active`] are, so the four read alike and so adding a modifier later
    /// does not change the rule.
    pub fn is_active(&self) -> bool {
        self.texture != 0.0 || self.clarity != 0.0
    }
}
```

- [ ] **Step 4: Add the field to `Edits`**

In `crates/edit-model/src/lib.rs`, in `pub struct Edits`, insert after the `pub hsl: Hsl,` line:

```rust
    pub local_contrast: LocalContrast,
```

- [ ] **Step 5: Give it a group in `merge_from`**

In `Edits::merge_from`, add `local_contrast,` to the destructuring pattern immediately after `hsl,`:

```rust
            hsl,
            local_contrast,
            noise_reduction,
```

and add it to the `groups.tone` block, after the `tone_curve` line:

```rust
        if groups.tone {
            self.exposure = *exposure;
            self.contrast = *contrast;
            self.highlights = *highlights;
            self.shadows = *shadows;
            self.whites = *whites;
            self.blacks = *blacks;
            self.tone_curve = tone_curve.clone();
            self.local_contrast = *local_contrast;
        }
```

- [ ] **Step 6: Update the group doc comment**

In `pub struct EditGroups`, change the `tone` doc comment:

```rust
    /// Exposure, contrast, highlights, shadows, whites, blacks, tone curve, texture, clarity.
    pub tone: bool,
```

- [ ] **Step 7: Add the field to the `all_nonneutral()` test fixture**

In `mod tests`, in `fn all_nonneutral()`, insert after the `hsl,` line:

```rust
            local_contrast: LocalContrast {
                texture: 40.0,
                clarity: -25.0,
            },
```

- [ ] **Step 8: Run the tests to verify they pass**

```bash
cargo test -p edit-model 2>&1 | tail -20
```

Expected: `test result: ok.` — including the pre-existing generic partition tests `merging_all_groups_reproduces_the_source`, `the_groups_are_disjoint` and `clearing_then_restoring_a_group_is_a_round_trip`, which now cover the new field with no changes of their own. That is the payoff of the exhaustive destructure.

- [ ] **Step 9: Commit**

```bash
git add crates/edit-model/src/lib.rs
git commit -m "feat(edit-model): add LocalContrast (texture, clarity) to the Tone group"
```

---

## Task 3: `LocalContrastStage`

**Files:**
- Create: `crates/pipeline/src/stages/local_contrast.rs`
- Modify: `crates/pipeline/src/stages/mod.rs`

- [ ] **Step 1: Declare the module**

In `crates/pipeline/src/stages/mod.rs`, add in alphabetical position (after `hsl`):

```rust
pub mod local_contrast;
```

- [ ] **Step 2: Write the stage with its failing tests**

Create `crates/pipeline/src/stages/local_contrast.rs` containing **only** the tests below plus `use super::*;` — the implementation comes in Step 4. Write the whole file now with the implementation section left out:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// All fixtures are 256 px wide on purpose: Clarity's kernel reaches 120 px at
    /// `RenderScale::FULL`, so on a 128 px image its blur is essentially the global mean and
    /// every measurement below would be about the border handling instead of about the
    /// filter.
    const S: usize = 256;

    /// A coarse 128 px-period sine (Clarity's band) with a fine 6 px-period sine on top
    /// (Texture's band), tinted so the hue-ratio property is exercised.
    fn two_band() -> ImageBuf {
        let mut img = ImageBuf::new(S, S);
        for y in 0..S {
            for x in 0..S {
                let coarse = 0.40 + 0.10 * (x as f32 / 128.0 * std::f32::consts::TAU).sin();
                let fine = 0.02 * (x as f32 / 6.0 * std::f32::consts::TAU).sin();
                let v = coarse + fine;
                img.set_pixel(x, y, [v, v * 0.8, v * 0.6]);
            }
        }
        img
    }

    /// Fine detail only: a 6 px-period sine on a flat mid gray.
    fn fine_only() -> ImageBuf {
        let mut img = ImageBuf::new(S, S);
        for y in 0..S {
            for x in 0..S {
                let v = 0.40 + 0.02 * (x as f32 / 6.0 * std::f32::consts::TAU).sin();
                img.set_pixel(x, y, [v; 3]);
            }
        }
        img
    }

    /// Coarse structure only: a 128 px-period sine, with nothing in the fine band.
    ///
    /// A *linear* ramp would be a poor fixture here — it is a fixed point of any symmetric
    /// blur, so Texture would be exactly inert on it and the test could not tell "Texture
    /// ignores coarse structure" apart from "Texture does nothing".
    fn coarse_only() -> ImageBuf {
        let mut img = ImageBuf::new(S, S);
        for y in 0..S {
            for x in 0..S {
                let v = 0.40 + 0.10 * (x as f32 / 128.0 * std::f32::consts::TAU).sin();
                img.set_pixel(x, y, [v; 3]);
            }
        }
        img
    }

    fn run(params: LocalContrast, img: ImageBuf) -> ImageBuf {
        LocalContrastStage::new(&params, RenderScale::FULL).apply(img)
    }

    /// Mean absolute luma change over the interior, avoiding the border band where the
    /// blur's edge handling dominates.
    fn mean_abs_change(a: &ImageBuf, b: &ImageBuf, border: usize) -> f32 {
        let (mut sum, mut count) = (0.0f32, 0usize);
        for y in border..a.height() - border {
            for x in border..a.width() - border {
                sum += (a.pixel(x, y)[1] - b.pixel(x, y)[1]).abs();
                count += 1;
            }
        }
        sum / count as f32
    }

    #[test]
    fn both_zero_is_an_exact_identity() {
        let src = two_band();
        assert_eq!(run(LocalContrast::default(), src.clone()), src);
    }

    #[test]
    fn a_flat_region_is_untouched() {
        // d = 0 in both bands, so the gain is exactly 1.
        let mut src = ImageBuf::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                src.set_pixel(x, y, [0.42, 0.30, 0.18]);
            }
        }
        let out = run(
            LocalContrast {
                texture: 100.0,
                clarity: 100.0,
            },
            src.clone(),
        );
        for (a, b) in out.data().iter().zip(src.data()) {
            assert!((a - b).abs() < 1e-6, "{a} vs {b}");
        }
    }

    #[test]
    fn texture_and_clarity_are_exactly_additive() {
        // ⚠ The entire justification for one stage rather than two. Both bands are measured
        // against the *same* source luma and summed, so the sliders cannot interact. Two
        // sequential stages would make Clarity operate on Texture's output instead, and
        // neither slider would be independently predictable.
        //
        // gain = 1 + (dt + dc)/L, so out(t,c) = out(t,0) + out(0,c) - src, exactly — as
        // long as amounts stay mild enough that GAIN_MAX never engages.
        let src = two_band();
        let t = run(
            LocalContrast {
                texture: 40.0,
                clarity: 0.0,
            },
            src.clone(),
        );
        let c = run(
            LocalContrast {
                texture: 0.0,
                clarity: 40.0,
            },
            src.clone(),
        );
        let both = run(
            LocalContrast {
                texture: 40.0,
                clarity: 40.0,
            },
            src.clone(),
        );
        let mut moved = 0usize;
        for (i, (((&bo, &to), &co), &so)) in both
            .data()
            .iter()
            .zip(t.data())
            .zip(c.data())
            .zip(src.data())
            .enumerate()
        {
            let expect = to + co - so;
            assert!((bo - expect).abs() < 1e-5, "sample {i}: {bo} vs {expect}");
            if (bo - so).abs() > 1e-6 {
                moved += 1;
            }
        }
        assert!(moved > 0, "nothing changed, so additivity was not tested");
    }

    #[test]
    fn texture_selects_the_fine_band_while_clarity_reaches_the_coarse_one() {
        // ⚠ Without this, a wrong sigma passes silently and every other test still goes
        // green.
        //
        // Note what is *not* asserted: that Texture beats Clarity on fine detail. Both are
        // high-pass, and Clarity's passband strictly *contains* Texture's — a 40 px blur of
        // a 6 px sine is the mean, so Clarity sees the fine band at nearly full strength too.
        // (Measured: ~0.011 for each.) An earlier draft of this test asserted the opposite
        // and would have failed against a correct implementation.
        //
        // The real discriminator is the other direction: Texture is band-*selective* and
        // barely sees coarse structure, while Clarity sees it strongly. Expected magnitudes
        // on these fixtures: coarse_t ~ 3e-4, fine_t ~ 1.1e-2, coarse_c ~ 4.7e-2.
        let tex = LocalContrast {
            texture: 100.0,
            clarity: 0.0,
        };
        let cla = LocalContrast {
            texture: 0.0,
            clarity: 100.0,
        };

        let fine = fine_only();
        let coarse = coarse_only();
        let fine_t = mean_abs_change(&run(tex, fine.clone()), &fine, 30);
        let coarse_t = mean_abs_change(&run(tex, coarse.clone()), &coarse, 30);
        let coarse_c = mean_abs_change(&run(cla, coarse.clone()), &coarse, 30);

        assert!(
            fine_t > coarse_t * 10.0,
            "texture is not fine-selective: fine {fine_t} vs coarse {coarse_t}"
        );
        assert!(
            coarse_c > coarse_t * 20.0,
            "clarity does not reach the coarse band: {coarse_c} vs texture's {coarse_t}"
        );
    }

    #[test]
    /// Peak-to-trough of the fine sine across the interior of a central row.
    fn fine_span(img: &ImageBuf) -> f32 {
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for x in 30..S - 30 {
            let v = img.pixel(x, S / 2)[1];
            lo = lo.min(v);
            hi = hi.max(v);
        }
        hi - lo
    }

    #[test]
    fn texture_minus_100_removes_the_fine_band() {
        // The endpoint claim: k = -1 subtracts the whole band, leaving the blurred base.
        //
        // The residual is not slop — it is exactly what a sigma-2 Gaussian retains of a 6 px
        // sine, about 11%. If this starts failing, check TEXTURE_SIGMA against the fixture's
        // 6 px period before touching the tolerance.
        let src = fine_only();
        let out = run(
            LocalContrast {
                texture: -100.0,
                clarity: 0.0,
            },
            src.clone(),
        );
        let (before, after) = (fine_span(&src), fine_span(&out));
        assert!(
            after < before * 0.2,
            "fine band went from {before} to {after}, not removed"
        );
    }

    #[test]
    fn texture_plus_100_roughly_doubles_the_fine_band() {
        // The other endpoint. About 1.89x rather than exactly 2, for the same reason:
        // the sigma-2 base retains ~11% of a 6 px sine, so `d` is 89% of the band.
        let src = fine_only();
        let out = run(
            LocalContrast {
                texture: 100.0,
                clarity: 0.0,
            },
            src.clone(),
        );
        let ratio = fine_span(&out) / fine_span(&src);
        assert!(
            (1.7..=2.1).contains(&ratio),
            "fine band scaled by {ratio}, expected about 1.9"
        );
    }

    #[test]
    fn midtone_weighting_protects_the_extremes() {
        // ⚠ Clarity only. Without it, +Clarity at a 40 px radius clips skies and blocks up
        // shadows — the standard complaint about the tool.
        //
        // The same coarse structure at three luma levels. Its amplitude is *proportional* to
        // the base, so `d` scales with the base too and the relative change below isolates
        // w(l) exactly. An absolute amplitude would go negative at base 0.01 and need
        // clamping, which would bias the comparison toward passing.
        //
        // Expected, from w = 4t(1-t) with t = l/(l+0.18): w(0.18) = 1.00, w(0.95) = 0.54,
        // w(0.01) = 0.20 — so mid/bright ~ 1.9 and mid/dark ~ 5.0.
        let ramp_at = |base: f32| {
            let mut img = ImageBuf::new(S, S);
            for y in 0..S {
                for x in 0..S {
                    let v = base
                        * (1.0 + 0.25 * (x as f32 / 128.0 * std::f32::consts::TAU).sin());
                    img.set_pixel(x, y, [v; 3]);
                }
            }
            img
        };
        let cla = LocalContrast {
            texture: 0.0,
            clarity: 100.0,
        };
        // Relative change, so the comparison is about w(l) and not about absolute level.
        let relative = |base: f32| {
            let src = ramp_at(base);
            let out = run(cla, src.clone());
            mean_abs_change(&out, &src, 30) / base
        };
        let mid = relative(0.18);
        let bright = relative(0.95);
        let dark = relative(0.01);
        assert!(
            mid > bright * 1.5,
            "highlights not protected: {mid} vs {bright}"
        );
        assert!(mid > dark * 1.5, "shadows not protected: {mid} vs {dark}");
    }

    #[test]
    fn texture_is_not_midtone_weighted() {
        // The asymmetry is deliberate and justified by amplitude: the fine band cannot
        // reach the clip on its own, so weighting it would cost work for no protection.
        let sine_at = |base: f32| {
            let mut img = ImageBuf::new(S, S);
            for y in 0..S {
                for x in 0..S {
                    let v = base * (1.0 + 0.05 * (x as f32 / 6.0 * std::f32::consts::TAU).sin());
                    img.set_pixel(x, y, [v; 3]);
                }
            }
            img
        };
        let tex = LocalContrast {
            texture: 100.0,
            clarity: 0.0,
        };
        let relative = |base: f32| {
            let src = sine_at(base);
            mean_abs_change(&run(tex, src.clone()), &src, 30) / base
        };
        let mid = relative(0.18);
        let bright = relative(0.95);
        assert!(
            (mid / bright - 1.0).abs() < 0.05,
            "texture is level-dependent: {mid} vs {bright}"
        );
    }

    #[test]
    fn hue_ratios_are_preserved() {
        // Applying the correction as a scalar RGB gain is what buys this: no colour
        // fringing, and no Oklab round trip to pay for.
        let src = two_band();
        let out = run(
            LocalContrast {
                texture: 60.0,
                clarity: 40.0,
            },
            src.clone(),
        );
        let mut moved = 0usize;
        for (o, s) in out.data().chunks_exact(3).zip(src.data().chunks_exact(3)) {
            if o[0] <= 0.0 {
                continue;
            }
            assert!((o[1] / o[0] - s[1] / s[0]).abs() < 1e-4);
            assert!((o[2] / o[0] - s[2] / s[0]).abs() < 1e-4);
            if (o[0] - s[0]).abs() > 1e-6 {
                moved += 1;
            }
        }
        assert!(moved > 0, "nothing changed, so nothing was tested");
    }

    #[test]
    fn never_emits_negative_or_nan_samples() {
        // Includes a near-black band, where the LUMA_EPS guard and GAIN_MAX have to hold.
        let mut src = ImageBuf::new(S, S);
        for y in 0..S {
            for x in 0..S {
                let v = if x < S / 2 { 1e-6 } else { 1.0 };
                src.set_pixel(x, y, [v; 3]);
            }
        }
        let out = run(
            LocalContrast {
                texture: -100.0,
                clarity: 100.0,
            },
            src.clone(),
        );
        assert!(out.data().iter().all(|v| v.is_finite() && *v >= 0.0));
        for (o, s) in out.data().iter().zip(src.data()) {
            assert!(*o <= s * GAIN_MAX + 1e-6, "{o} exceeds {s} * {GAIN_MAX}");
        }
    }

    #[test]
    fn radii_scale_with_the_render_scale() {
        // A full-resolution radius rendered at 1/4 scale is a quarter-size blur, so the
        // preview approximates the export rather than contradicting it.
        let both = LocalContrast {
            texture: 50.0,
            clarity: 50.0,
        };
        let full = kernel_reach(&both, RenderScale::FULL);
        let quarter = kernel_reach(&both, RenderScale::new(0.25));
        assert_eq!(full, 120); // wide_blur_reach(40.0)
        assert!(quarter < full, "reach {quarter} did not shrink from {full}");
        // Two different render scales must actually produce different pixels, or the scale
        // is being ignored somewhere inside `apply`.
        let src = two_band();
        let a = LocalContrastStage::new(&both, RenderScale::FULL).apply(src.clone());
        let b = LocalContrastStage::new(&both, RenderScale::new(0.25)).apply(src);
        assert_ne!(a, b);
    }

    #[test]
    fn kernel_reach_is_the_max_of_the_two_bands_and_zero_when_inactive() {
        // ⚠ max, not sum: the two bands are measured against the same source luma and
        // summed, so neither composes with the other — unlike sharpening's two Gaussians,
        // where the mask blurs |d| on top of the first blur and the radii genuinely add.
        let tex = LocalContrast {
            texture: 50.0,
            clarity: 0.0,
        };
        let cla = LocalContrast {
            texture: 0.0,
            clarity: 50.0,
        };
        let both = LocalContrast {
            texture: 50.0,
            clarity: 50.0,
        };
        assert_eq!(kernel_reach(&tex, RenderScale::FULL), 6); // ceil(3 * 2.0)
        assert_eq!(kernel_reach(&cla, RenderScale::FULL), 120);
        assert_eq!(kernel_reach(&both, RenderScale::FULL), 120);
        assert_eq!(kernel_reach(&LocalContrast::default(), RenderScale::FULL), 0);
    }
}
```

- [ ] **Step 3: Run the tests to verify they fail**

```bash
cargo test -p pipeline --lib local_contrast 2>&1 | tail -20
```

Expected: compile error, `cannot find type 'LocalContrastStage' in this scope` (and `kernel_reach`, `GAIN_MAX`, `ImageBuf`, `RenderScale`).

- [ ] **Step 4: Write the implementation**

Prepend to `crates/pipeline/src/stages/local_contrast.rs`, above the `#[cfg(test)] mod tests`:

```rust
use crate::spatial::{gaussian_blur, kernel_radius, wide_blur, wide_blur_reach};
use crate::{PipelineStage, RenderScale};
use core_image::ImageBuf;
use edit_model::LocalContrast;
use rayon::prelude::*;

/// Texture's blur sigma, in **full-resolution pixels**. Distinctly coarser than sharpening's
/// default of 1.0, so the two own different bands. A tuning constant — see the spec.
pub const TEXTURE_SIGMA: f32 = 2.0;
/// Clarity's blur sigma, in **full-resolution pixels**. Large enough to read as local rather
/// than global contrast, and small enough that the reach it implies (120 px) stays under
/// `app::view_region::REGION_MARGIN`. A tuning constant — see the spec.
pub const CLARITY_SIGMA: f32 = 40.0;

/// Luma that the midtone weight peaks at: mid-gray.
const MID_GRAY: f32 = 0.18;
/// Near-black guard for the ratio. Load-bearing, exactly as in `stages/sharpening.rs`:
/// without it a dark pixel beside a bright edge divides by ~0 and the gain explodes.
const LUMA_EPS: f32 = 1e-3;
/// Caps a halo in near-black regions at +2 stops.
const GAIN_MAX: f32 = 4.0;

/// Rec.709 luma, in linear light. Same as `stages/sharpening.rs`.
fn luma(px: &[f32]) -> f32 {
    0.2126 * px[0] + 0.7152 * px[1] + 0.0722 * px[2]
}

/// Midtone weight for Clarity: 1 at mid-gray, falling smoothly to 0 at black and at white.
///
/// `t = l / (l + MID_GRAY)` maps mid-gray to exactly 0.5 and needs no `powf`, which is why
/// it is preferred to an `l^(1/2.2)` proxy.
///
/// Applied to Clarity **only**. Without it, +Clarity at a 40 px radius clips skies and blocks
/// up shadows. Texture's band is low-amplitude and cannot reach the clip on its own, so
/// weighting it would cost work for no protection — the asymmetry is justified by amplitude,
/// not taste.
fn midtone_weight(l: f32) -> f32 {
    let t = l / (l + MID_GRAY);
    4.0 * t * (1.0 - t)
}

/// The widest neighbourhood one output pixel can depend on, in **rendered** pixels.
///
/// The **max** of the two bands, not their sum: both are measured against the same source
/// luma and their deltas are added, so neither composes with the other. (Contrast
/// `sharpening::max_kernel_radius`, where the mask blurs `|d|` on top of the first blur and
/// the two radii genuinely add.) An inactive band contributes nothing.
///
/// Any renderer that crops a region before running this stage must keep at least this much
/// margin outside what it displays — see `crate::tail_kernel_radius`.
pub fn kernel_reach(params: &LocalContrast, scale: RenderScale) -> usize {
    let s = scale.get();
    let t = if params.texture != 0.0 {
        kernel_radius(TEXTURE_SIGMA * s)
    } else {
        0
    };
    let c = if params.clarity != 0.0 {
        wide_blur_reach(CLARITY_SIGMA * s)
    } else {
        0
    };
    t.max(c)
}

/// Texture and Clarity: local contrast in a fine and a coarse band.
///
/// The same operation as `SharpenStage` — `d = luma - blur(luma)` on Rec.709 luma in linear
/// light, re-applied as a scalar RGB gain — at two much different radii. Working on luma and
/// re-applying as a ratio means no colour fringing on a chromatic edge, hue and saturation
/// ratios preserved exactly, and **no Oklab round trip**.
///
/// **One stage, not two.** Both bands are measured against the *same* source luma and their
/// deltas summed, so the sliders are exactly additive. Two sequential stages would make
/// Clarity operate on Texture's output, and neither slider would be independently
/// predictable.
///
/// `k = amount/100` with no extra gain constant, so the endpoints mean something: **-100
/// removes the band and +100 doubles it** (for Clarity, in the midtones, where the weight is
/// 1).
pub struct LocalContrastStage {
    params: LocalContrast,
    scale: RenderScale,
}

impl LocalContrastStage {
    pub fn new(params: &LocalContrast, scale: RenderScale) -> Self {
        Self {
            params: *params,
            scale,
        }
    }
}

impl PipelineStage for LocalContrastStage {
    fn name(&self) -> &'static str {
        "local_contrast"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        let (w, h) = (image.width(), image.height());
        if w == 0 || h == 0 || !self.params.is_active() {
            return image;
        }
        let n = w * h;
        let s = self.scale.get();

        let l: Vec<f32> = image.data().par_chunks_exact(3).map(luma).collect();

        let k_t = (self.params.texture / 100.0).clamp(-1.0, 1.0);
        let k_c = (self.params.clarity / 100.0).clamp(-1.0, 1.0);

        // Both bands read `l`, never each other's output. That is what makes the two
        // sliders additive rather than sequential.
        let mut delta = vec![0.0f32; n];
        if k_t != 0.0 {
            // Small sigma: a direct Gaussian kernel, which is cheap here and is the only
            // thing that can express a sub-pixel blur once RenderScale shrinks it.
            let base = gaussian_blur(&l, w, h, TEXTURE_SIGMA * s);
            delta
                .par_iter_mut()
                .zip(&l)
                .zip(&base)
                .for_each(|((d, &lv), &b)| *d += k_t * (lv - b));
        }
        if k_c != 0.0 {
            // Large sigma: iterated box passes, O(1) in the radius. A true Gaussian here
            // would be ~13 G operations on a 26 Mpx export.
            let base = wide_blur(&l, w, h, CLARITY_SIGMA * s);
            delta
                .par_iter_mut()
                .zip(&l)
                .zip(&base)
                .for_each(|((d, &lv), &b)| *d += k_c * (lv - b) * midtone_weight(lv));
        }

        let mut data = vec![0.0f32; n * 3];
        data.par_chunks_exact_mut(3)
            .zip(image.data().par_chunks_exact(3))
            .zip(&delta)
            .zip(&l)
            .for_each(|(((o, px), &d), &lv)| {
                // Clamped at 0 below, so the stage can never emit a negative sample and
                // needs no separate clamp — the same structural property VignetteStage has.
                let gain = (1.0 + d / lv.max(LUMA_EPS)).clamp(0.0, GAIN_MAX);
                o[0] = px[0] * gain;
                o[1] = px[1] * gain;
                o[2] = px[2] * gain;
            });
        ImageBuf::from_vec(w, h, data)
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cargo test -p pipeline --lib local_contrast 2>&1 | tail -30
```

Expected: `test result: ok.` with 12 tests.

If either endpoint test fails narrowly, do **not** widen the tolerance without first checking `TEXTURE_SIGMA` against the fixture's 6 px sine period — a σ of 2.0 retains about 11% of a 6 px sine, which is exactly why the endpoints measure 0.11 and 1.89 rather than 0 and 2. If the period and σ ever drift apart, fix the fixture, not the assertion.

- [ ] **Step 6: Commit**

```bash
git add crates/pipeline/src/stages/local_contrast.rs crates/pipeline/src/stages/mod.rs
git commit -m "feat(pipeline): add LocalContrastStage (texture + clarity)"
```

---

## Task 4: Wire the stage into the pipeline

**Files:**
- Modify: `crates/pipeline/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Append inside `mod tests` in `crates/pipeline/src/lib.rs`:

```rust
    #[test]
    fn local_contrast_runs_after_the_tone_curve_and_before_saturation() {
        // After the tone curve is mandatory: a local contrast computed before it would then
        // be re-mapped by it — the same argument that kept the vignette out of the prefix.
        // Before the colour stages so they see final tonality.
        let edits = Edits {
            exposure: 1.0,
            tone_curve: edit_model::ToneCurve {
                points: vec![[0.0, 0.0], [0.5, 0.6], [1.0, 1.0]],
            },
            local_contrast: edit_model::LocalContrast {
                texture: 40.0,
                clarity: 20.0,
            },
            saturation: 20.0,
            ..Edits::default()
        };
        assert_eq!(
            build_pipeline(&edits, RenderScale::FULL).stage_names(),
            vec![
                "exposure",
                "tone_curve",
                "local_contrast",
                "saturation",
                "output_transform_srgb"
            ]
        );
    }

    #[test]
    fn neutral_local_contrast_adds_no_stage() {
        let edits = Edits {
            exposure: 1.0,
            local_contrast: edit_model::LocalContrast::default(),
            ..Edits::default()
        };
        assert_eq!(build_pipeline(&edits, RenderScale::FULL).len(), 2);
    }

    #[test]
    fn local_contrast_is_in_the_tail() {
        // It must not join the prefix: it has to be downstream of the tone curve, and
        // putting it upstream would also make every Texture drag invalidate the noise
        // reduction cache.
        let (prefix, tail) = build_pipeline_split(&all_edits(), RenderScale::FULL);
        assert!(!prefix.stage_names().contains(&"local_contrast"));
        assert!(tail.stage_names().contains(&"local_contrast"));
    }

    #[test]
    fn tail_kernel_radius_covers_every_spatial_tail_stage() {
        let neutral = Edits::default();
        assert_eq!(tail_kernel_radius(&neutral, RenderScale::FULL), 0);

        // Sharpening alone at its default radius: ceil(3*1.0) + ceil(3*2.0) = 9.
        let sharp = Edits {
            sharpening: edit_model::Sharpening {
                amount: 50.0,
                ..Default::default()
            },
            ..Edits::default()
        };
        assert_eq!(tail_kernel_radius(&sharp, RenderScale::FULL), 9);

        // Clarity alone: wide_blur_reach(40.0) = 120, far wider than sharpening ever gets.
        let clarity = Edits {
            local_contrast: edit_model::LocalContrast {
                texture: 0.0,
                clarity: 50.0,
            },
            ..Edits::default()
        };
        assert_eq!(tail_kernel_radius(&clarity, RenderScale::FULL), 120);

        // Both: the max, because the two stages are sequential in the pipeline but each
        // reads its own neighbourhood of that stage's input.
        let both = Edits {
            sharpening: sharp.sharpening,
            local_contrast: clarity.local_contrast,
            ..Edits::default()
        };
        assert_eq!(tail_kernel_radius(&both, RenderScale::FULL), 120);

        // The worst case must bound every representable setting.
        let worst = max_tail_kernel_radius(RenderScale::FULL);
        for e in [&neutral, &sharp, &clarity, &both, &all_edits()] {
            assert!(
                tail_kernel_radius(e, RenderScale::FULL) <= worst,
                "an edit set exceeded max_tail_kernel_radius {worst}"
            );
        }
        // And it must be tighter at a smaller render scale, or a preview would over-reserve.
        assert!(max_tail_kernel_radius(RenderScale::new(0.25)) < worst);
    }
```

Also add a `local_contrast` case to the existing `prefix_key_ignores_every_downstream_field` test's `downstream` array, after the `hsl` entry:

```rust
            Edits {
                local_contrast: edit_model::LocalContrast {
                    texture: 60.0,
                    clarity: -40.0,
                },
                ..base.clone()
            },
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p pipeline --lib 2>&1 | tail -20
```

Expected: compile error — `all_edits()` is missing the `local_contrast` field (`missing field 'local_contrast' in initializer of 'Edits'`), and `tail_kernel_radius` is not found. **The `all_edits()` compile error is by design** — it is written out field by field precisely so that adding a field forces a decision about which half of the split it belongs to.

- [ ] **Step 3: Add the field to `all_edits()`**

In `crates/pipeline/src/lib.rs`, in `mod tests`, in `fn all_edits()`, insert after the `hsl,` line:

```rust
            local_contrast: edit_model::LocalContrast {
                texture: 40.0,
                clarity: -25.0,
            },
```

- [ ] **Step 4: Insert the stage into the tail**

In `crates/pipeline/src/lib.rs`, in `build_pipeline_split_in_region`, insert **between** the `tone_curve` block and the `saturation` block:

```rust
    // Local contrast, in two bands. After the tone curve is mandatory: a local contrast
    // measured before it would then be re-mapped by it. Before the colour stages so they
    // see final tonality — numerically near-irrelevant, since the gain is scalar and
    // preserves r:g:b ratios, but the order should say what we mean.
    if edits.local_contrast.is_active() {
        tail.push(Box::new(LocalContrastStage::new(
            &edits.local_contrast,
            scale,
        )));
    }
```

- [ ] **Step 5: Add the export and the two radius functions**

In `crates/pipeline/src/lib.rs`, add to the `pub use` block (after the `hsl` line):

```rust
pub use stages::local_contrast::LocalContrastStage;
```

and add, after the `build_pipeline` function:

```rust
/// The widest neighbourhood any **tail** stage reads, in rendered pixels, for these edits.
///
/// The tail is the half that runs on a cropped region during a 1:1 render, so at the
/// region's edge a spatial tail stage reads neighbours that do not exist. This is the width
/// of that wrong band, and therefore the margin such a render must keep outside what it
/// displays. Prefix stages are exempt: the prefix always runs whole-buffer, which is why
/// noise reduction can never produce a seam however you pan.
///
/// An inactive stage contributes 0.
pub fn tail_kernel_radius(edits: &Edits, scale: RenderScale) -> usize {
    let sharp = if edits.sharpening.is_active() {
        stages::sharpening::max_kernel_radius(edits.sharpening.radius, scale)
    } else {
        0
    };
    let local = stages::local_contrast::kernel_reach(&edits.local_contrast, scale);
    sharp.max(local)
}

/// [`tail_kernel_radius`] maximised over every representable setting — what a fixed render
/// margin has to exceed.
///
/// Defined *through* `tail_kernel_radius` rather than duplicating the maxima, so the two
/// cannot drift apart when a stage is added.
pub fn max_tail_kernel_radius(scale: RenderScale) -> usize {
    let worst = Edits {
        sharpening: edit_model::Sharpening {
            amount: 100.0,
            radius: RADIUS_RANGE.1,
            ..Default::default()
        },
        local_contrast: edit_model::LocalContrast {
            texture: 100.0,
            clarity: 100.0,
        },
        ..Edits::default()
    };
    tail_kernel_radius(&worst, scale)
}
```

- [ ] **Step 6: Run the tests to verify they pass**

```bash
cargo test -p pipeline 2>&1 | tail -30
```

Expected: all lib tests pass. **The golden tests must also still pass, all 23 of them** — the stage is inert at its default, so not one reference byte may change. If a golden fails here, the stage is being inserted when it should not be.

- [ ] **Step 7: Commit**

```bash
git add crates/pipeline/src/lib.rs
git commit -m "feat(pipeline): run local contrast after the tone curve; add tail_kernel_radius"
```

---

## Task 5: The region margin

Clarity's 120 px reach against a `REGION_MARGIN` of 128 leaves 8 px of slack, which is not slack — it is a trap for whoever touches this next.

**Files:**
- Modify: `crates/app/src/view_region.rs`
- Modify: `crates/app/src/render_engine.rs:337-430`

- [ ] **Step 1: Update the invariant test and the constant**

In `crates/app/src/view_region.rs`, change `REGION_MARGIN` and its doc comment:

```rust
/// Rendered beyond the visible rect so small pans reuse the existing texture — and, since
/// the spatial tail stages run on the cropped region, so their edge band falls outside what
/// is displayed. See `the_region_margin_exceeds_every_tail_kernel_radius`.
///
/// 192 rather than a tighter fit over Clarity's 120 px reach: 64 px of headroom costs about
/// 20% more pixels in the region tail (~1.5 ms) and buys room for the next spatial stage.
pub const REGION_MARGIN: usize = 192;
```

and replace the body of `the_region_margin_exceeds_every_tail_kernel_radius`:

```rust
    #[test]
    fn the_region_margin_exceeds_every_tail_kernel_radius() {
        // ⚠ The invariant that keeps 1:1 panning seam-free.
        //
        // Noise reduction cannot produce a seam because it lives in the *prefix*, which
        // always runs whole-buffer. Sharpening and local contrast live in the **tail**,
        // which runs on the cropped region — so at the region's edge they read neighbours
        // that do not exist, producing a band of wrong output. `with_margin` renders that
        // much extra on every side, so the band is never displayed.
        //
        // Widening a slider range, or adding another spatial stage to the tail, must fail
        // here rather than produce seams that only show up while panning at 1:1.
        let worst = pipeline::max_tail_kernel_radius(RenderScale::FULL);
        assert!(
            REGION_MARGIN > worst,
            "REGION_MARGIN {REGION_MARGIN} must exceed the tail's kernel radius {worst}"
        );
    }
```

- [ ] **Step 2: Update the region-equality test**

In `crates/app/src/render_engine.rs`, in `a_region_render_matches_the_whole_render_cropped`:

Replace the comment block's last paragraph and the `const N` / `edits` / geometry section. The edit set gains local contrast, and the buffer and region are **derived from the inset** rather than hard-coded — the inset jumps from 9 px to 120 px and the old 64×64 buffer cannot hold a region wider than twice that.

```rust
        // ⚠ Sharpening and local contrast *weaken* it, and that has to be said out loud
        // rather than left to an edit set that happens to omit them. A spatial stage in the
        // tail reads neighbours the cropped region genuinely does not have, so equality can
        // only hold in the interior. The excluded band is `tail_kernel_radius` wide —
        // precisely what `view_region::REGION_MARGIN` keeps off screen — and both the inset
        // and the buffer size are derived from it, so they track any future change to a
        // radius or a sigma.
        let edits = Edits {
            exposure: 0.4,
            vignette: edit_model::Vignette {
                amount: -80.0,
                roundness: 50.0,
                ..Default::default()
            },
            geometry: edit_model::Geometry {
                angle: 3.0,
                ..Default::default()
            },
            noise_reduction: edit_model::NoiseReduction {
                luminance: 40.0,
                color: 40.0,
                ..Default::default()
            },
            local_contrast: edit_model::LocalContrast {
                texture: 60.0,
                clarity: 70.0,
            },
            sharpening: edit_model::Sharpening {
                amount: 90.0,
                radius: 1.0,
                detail: 50.0,
                masking: 60.0,
            },
            ..Edits::default()
        };

        let inset = pipeline::tail_kernel_radius(&edits, RenderScale::FULL);
        let (x, y, w, h) = (12, 10, 2 * inset + 40, 2 * inset + 36);
        let n = 2 * inset + 80;
        let buf = varied_sized(n);
```

Then replace the `max_dim` argument `N` with `n` in the three `render_cached` calls, and replace the assertion block that used `N`:

```rust
        // max_dim = n is native for this n x n buffer, i.e. no downscale, so the render
        // scale is exactly 1.0 and the inset above is the full-resolution kernel.
        let whole = render_cached(&buf, &edits, n, 1.0, None, &mut PrefixCache::default());
```

```rust
        let unsharpened = render_cached(
            &buf,
            &Edits {
                sharpening: edit_model::Sharpening::default(),
                local_contrast: edit_model::LocalContrast::default(),
                ..edits.clone()
            },
            n,
            1.0,
            None,
            &mut PrefixCache::default(),
        );
        assert_ne!(
            whole.rgba, unsharpened.rgba,
            "the spatial tail stages changed nothing"
        );
```

```rust
        assert!(
            x + w <= whole.width && y + h <= whole.height,
            "region must fit"
        );
        assert!(
            w > 2 * inset && h > 2 * inset,
            "region must be wider than twice the kernel ({inset})"
        );

        let got = render_cached(
            &buf,
            &edits,
            n,
            1.0,
            Some(RegionRect { x, y, w, h }),
            &mut PrefixCache::default(),
        );
```

Delete the now-unused `const N: usize = 64;` line and the old `let buf = varied_sized(N);`.

- [ ] **Step 3: Run the app tests**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo test -p app --lib 2>&1 | tail -30
```

Expected: `test result: ok.` — in particular `the_region_margin_exceeds_every_tail_kernel_radius` and `a_region_render_matches_the_whole_render_cropped`.

- [ ] **Step 4: Prove the invariant test actually bites**

Temporarily set `REGION_MARGIN` to `100` and re-run:

```bash
cargo test -p app --lib the_region_margin 2>&1 | tail -10
```

Expected: FAIL with `REGION_MARGIN 100 must exceed the tail's kernel radius 120`. **Then set it back to 192 and re-run to confirm green.** A guard nobody has watched fail is not a guard.

- [ ] **Step 5: Prove the interior comparison actually bites**

Temporarily set `let inset = 0;` in `a_region_render_matches_the_whole_render_cropped` and re-run:

```bash
cargo test -p app --lib a_region_render 2>&1 | tail -10
```

Expected: FAIL on the interior byte comparison. **Then restore the derived `inset` and re-run to confirm green.**

- [ ] **Step 6: Commit**

```bash
git add crates/app/src/view_region.rs crates/app/src/render_engine.rs
git commit -m "fix(app): raise REGION_MARGIN to 192 for clarity's 120 px reach"
```

---

## Task 6: Golden references

**Files:**
- Modify: `crates/pipeline/tests/golden.rs`
- Create: `crates/pipeline/tests/golden/texture_plus.png`, `texture_minus.png`, `clarity_plus.png`, `clarity_minus.png`

- [ ] **Step 1: Add the chart and the four cases**

In `crates/pipeline/tests/golden.rs`, add `LocalContrast` to the existing `edit_model` import (alphabetically, after `Edits`). The line goes over 100 columns, so let `cargo fmt` wrap it rather than hand-formatting:

```bash
cargo fmt -p pipeline
```

Add the chart after `noise_chart()`:

```rust
/// Deterministic chart for the local-contrast cases: a coarse horizontal ramp (what
/// Clarity's 40 px radius sees), a fine 6 px-period sine on top of it (what Texture's 2 px
/// radius sees), and a hard horizontal edge three quarters of the way down (where a halo
/// would appear). Tinted rather than neutral, so the "hue ratios are preserved" property is
/// visible in the reference rather than only in a unit test.
///
/// 256x256, far larger than the other charts and deliberately so: Clarity's kernel reaches
/// 120 px, so on a 64- or 128-px chart the reference would record the global mean rather
/// than local contrast.
fn local_contrast_chart() -> ImageBuf {
    const S: usize = 256;
    let mut img = ImageBuf::new(S, S);
    for y in 0..S {
        for x in 0..S {
            let coarse = 0.15 + 0.5 * (x as f32 / S as f32);
            let fine = 0.03 * (x as f32 / 6.0 * std::f32::consts::TAU).sin();
            let step = if y >= 3 * S / 4 { 0.25 } else { 0.0 };
            let v = (coarse + fine + step).clamp(0.0, 1.0);
            img.set_pixel(x, y, [v, v * 0.85, v * 0.7]);
        }
    }
    img
}
```

Add the four cases at the end of the file:

```rust
#[test]
fn golden_texture_plus() {
    check_golden_with(
        "texture_plus",
        &Edits {
            local_contrast: LocalContrast {
                texture: 80.0,
                clarity: 0.0,
            },
            ..Edits::default()
        },
        local_contrast_chart(),
    )
}

#[test]
fn golden_texture_minus() {
    check_golden_with(
        "texture_minus",
        &Edits {
            local_contrast: LocalContrast {
                texture: -80.0,
                clarity: 0.0,
            },
            ..Edits::default()
        },
        local_contrast_chart(),
    )
}

#[test]
fn golden_clarity_plus() {
    check_golden_with(
        "clarity_plus",
        &Edits {
            local_contrast: LocalContrast {
                texture: 0.0,
                clarity: 60.0,
            },
            ..Edits::default()
        },
        local_contrast_chart(),
    )
}

#[test]
fn golden_clarity_minus() {
    check_golden_with(
        "clarity_minus",
        &Edits {
            local_contrast: LocalContrast {
                texture: 0.0,
                clarity: -60.0,
            },
            ..Edits::default()
        },
        local_contrast_chart(),
    )
}
```

- [ ] **Step 2: Run to verify the four new tests fail and the 23 existing ones pass**

```bash
cargo test -p pipeline --test golden 2>&1 | tail -20
```

Expected: the four new tests FAIL with `missing golden texture_plus; run with UPDATE_GOLDEN=1`; **all 23 existing golden tests PASS**. That second half is the important one — it is the proof the new stage is inert at its default.

- [ ] **Step 3: Generate the references**

```bash
UPDATE_GOLDEN=1 cargo test -p pipeline --test golden 2>&1 | tail -5
git status --short crates/pipeline/tests/golden/
```

Expected: exactly four new untracked PNGs, and **no modifications to the 23 existing ones**. If `git status` shows any existing golden as modified, stop — something leaked, and finding out what is more important than the new feature.

- [ ] **Step 4: Look at the four PNGs**

```bash
open crates/pipeline/tests/golden/texture_plus.png crates/pipeline/tests/golden/texture_minus.png crates/pipeline/tests/golden/clarity_plus.png crates/pipeline/tests/golden/clarity_minus.png
```

Check by eye, and do not skip this — a golden nobody has looked at is just a hash:

- `texture_plus` — the fine vertical striping is visibly stronger than in `texture_minus`, where it should be nearly gone.
- `clarity_plus` — the ramp has a visible "shoulder" near the horizontal edge and the edge itself has gained local contrast; the fine striping is essentially unchanged from the source.
- `clarity_minus` — flatter, glowing, with the edge softened.
- All four keep the warm tint — no colour shift, no channel fringing.

If `texture_plus` and `clarity_plus` look like the same image, the two sigmas are not separated and `clarity_acts_at_a_coarser_scale_than_texture` is passing on a technicality. Investigate before committing.

- [ ] **Step 5: Re-run to confirm they now pass**

```bash
cargo test -p pipeline --test golden 2>&1 | tail -5
```

Expected: 27 tests, `test result: ok.`

- [ ] **Step 6: Commit**

```bash
git add crates/pipeline/tests/golden.rs crates/pipeline/tests/golden/
git commit -m "test(pipeline): golden references for texture and clarity"
```

---

## Task 7: The sliders

**Files:**
- Modify: `crates/app/src/main.rs:1478-1479`

- [ ] **Step 1: Add the two sliders**

In `crates/app/src/main.rs`, in `develop_body`'s right panel, **replace** the existing Blacks and Saturation slider lines with this block (the two outer lines are unchanged — they are shown so the insertion point is unambiguous):

```rust
                        slider(ui, &mut s.edits.blacks, -100.0..=100.0, "Blacks");
                        // Lightroom's "Presence" grouping, and it reflects that these are
                        // tonal rather than detail controls — which is also why they are in
                        // the Tone edit group rather than Detail.
                        slider(
                            ui,
                            &mut s.edits.local_contrast.texture,
                            -100.0..=100.0,
                            "Texture",
                        );
                        slider(
                            ui,
                            &mut s.edits.local_contrast.clarity,
                            -100.0..=100.0,
                            "Clarity",
                        );
                        slider(ui, &mut s.edits.saturation, -100.0..=100.0, "Saturation");
```

- [ ] **Step 2: Verify the whole workspace builds clean**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --all 2>&1 | tail -30
```

Expected: no fmt diff, no clippy warnings, every test green across the workspace.

- [ ] **Step 3: Commit**

```bash
git add crates/app/src/main.rs
git commit -m "feat(app): Texture and Clarity sliders in the Develop panel"
```

---

## Task 8: Validate on a real photograph and measure

Non-optional, and it is where the tuning constants get settled. Precedent: the vignette's amount model was revised three times, every time from looking at an actual image, and the spec could not have predicted any of the three.

**Files:**
- Possibly modify: `crates/pipeline/src/stages/local_contrast.rs` (the sigmas, `midtone_weight`)
- Modify: `CLAUDE.md`

- [ ] **Step 1: Launch the GUI**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo run --release -p app
```

Always `--release`. Tell the user what to look at:

- **Clarity at fit zoom.** It should read as *local* contrast — midtones gaining punch — not as a global contrast slider and not as an obvious halo along a sky/mountain boundary.
- **Texture at 1:1 zoom** (scroll past `fit · zoom = 1.0` so native mode engages). At fit it will look nearly inert, and **that is correct** — at the X-T4 fit scale of 0.256 a σ of 2.0 becomes 0.51, and the detail Texture acts on has already been averaged out of the preview.
- **Negative Texture on a face**, for mush: it is not edge-aware, by design.
- **Both at once**, checking that moving one does not visibly change what the other was doing.

- [ ] **Step 2: Judge the three tuning constants**

Nothing here is settled by the spec:

- `TEXTURE_SIGMA = 2.0` — is this a band genuinely distinct from Sharpening's default σ of 1.0, or do the two sliders feel like duplicates?
- `CLARITY_SIGMA = 40.0` — local contrast, or global contrast, or haloed? **If it needs to grow past ~62, `REGION_MARGIN` must grow with it** — `max_tail_kernel_radius` and the invariant test will say so rather than letting seams appear.
- `midtone_weight` — are highlights protected enough at +100 Clarity on a bright sky?

Adjust, re-run `cargo test -p pipeline` (the unit tests assert *relationships*, not the constants themselves, so they should survive a retune; the **goldens will change** and must be regenerated with `UPDATE_GOLDEN=1` and looked at again), and commit any change separately with the reasoning in the message.

- [ ] **Step 3: Measure the preview cost**

```bash
cargo run --release -p app --example bench_preview <path-to-raw>
```

Compare against the table in `CLAUDE.md` (neutral 21.5 ms, sharpening 70 → 26.6 ms, NR both 50 → 45.3 ms). Record Texture alone, Clarity alone, and both.

If Clarity is unexpectedly expensive, check that `wide_blur` is being reached rather than a Gaussian — three box passes should cost roughly what one NR chroma blur costs.

- [ ] **Step 4: Update `CLAUDE.md`**

Rewrite the **RESUME HERE** block and the crate-layout line for `pipeline`:

- `pipeline` line: add `local_contrast` to the stage list and `spatial::wide_blur` to the primitives.
- Add a "**Texture & Clarity done**" section in the established style, covering: one stage not two and why (exact additivity); `wide_blur` as the third spatial primitive and the three regimes; the pipeline position and why after the tone curve; **`REGION_MARGIN` 128 → 192** and the `tail_kernel_radius` / `max_tail_kernel_radius` split; midtone weighting on Clarity only; the measured numbers from Step 3; goldens 23 → 27 with the 23 byte-identical; and the accepted caveats from the spec — especially **"judge Texture at 1:1, Clarity at fit"**, which is the one most likely to be reported as a bug.
- Update the Phase 2 status: **M2.3 is still 🟡** — texture and clarity are done, dehaze is not. Do not claim M2.3 closed.
- Set the **NEXT ACTION** to dehaze, carrying forward the two findings from this spec: it needs a **global airlight statistic**, which no tail stage can compute from a cropped region without breaking `a_region_render_matches_the_whole_render_cropped` in the interior; and its radii are larger still, so it will reopen `REGION_MARGIN`. Note that `NoiseReductionStage` already contains a guided filter, which is what a dark-channel transmission map needs for refinement.
- Add the spec and plan paths to the docs list at the bottom.

- [ ] **Step 5: Final full check and commit**

```bash
cargo fmt --all --check && cargo clippy --all-targets -- -D warnings && cargo test --all 2>&1 | tail -15
git add -A
git commit -m "docs: record texture & clarity; next action is dehaze"
```

Expected: everything green, working tree clean.

**Do not push.** The user pushes to `github.com/wladiKrin/rustroom` themselves.

---

## Definition of done

- [ ] `cargo fmt --all --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --all` all green
- [ ] All 23 pre-existing golden references **byte-identical**; 4 new ones added and looked at
- [ ] `the_region_margin_exceeds_every_tail_kernel_radius` watched to fail at `REGION_MARGIN = 100`, then restored
- [ ] `a_region_render_matches_the_whole_render_cropped` watched to fail at `inset = 0`, then restored
- [ ] Texture and Clarity validated on a real RAW in the GUI, at 1:1 and at fit respectively
- [ ] Preview cost measured and recorded in `CLAUDE.md`
- [ ] `CLAUDE.md` says M2.3 is still open pending dehaze
