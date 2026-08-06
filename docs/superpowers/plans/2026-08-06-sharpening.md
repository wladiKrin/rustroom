# Sharpening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add unsharp-mask sharpening (Amount / Radius / Detail / Masking) as the twelfth pipeline stage, on Rec.709 luma with a ratio-based apply-back, plus the `spatial::gaussian_blur` primitive it needs.

**Architecture:** A new `gaussian_blur` primitive lands **alongside** `box_blur` in `pipeline::spatial` (float sigma, `ceil(3σ)` kernel, replicated edges, both passes rayon-parallel over rows). `SharpenStage` computes `d = luma − blur(luma)`, shapes it with a Detail halo-suppression term and an optional Masking edge gate, and applies it back as an RGB gain so hue ratios are preserved and no Oklab round trip is needed. The stage joins the region-cropped **tail** after the vignette, which makes `REGION_MARGIN` vs. kernel radius a real invariant that gets asserted.

**Tech Stack:** Rust workspace, `rayon`, `egui`/`eframe` 0.35 for the sliders, the existing golden-PNG harness in `crates/pipeline/tests/golden.rs`.

**Spec:** `docs/superpowers/specs/2026-08-06-sharpening-design.md` (approved — do not re-derive).

---

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `crates/pipeline/src/spatial.rs` | `box_blur` + **new** `gaussian_blur`, `kernel_radius` | Modify |
| `crates/edit-model/src/lib.rs` | `Sharpening` params + `Edits.sharpening` | Modify |
| `crates/pipeline/src/stages/sharpening.rs` | `SharpenStage`, `RADIUS_RANGE`, `max_kernel_radius` | **Create** |
| `crates/pipeline/src/stages/mod.rs` | register the module | Modify |
| `crates/pipeline/src/lib.rs` | re-export, wire into the tail, extend `all_edits()` | Modify |
| `crates/pipeline/tests/golden.rs` | 3 new golden cases | Modify |
| `crates/pipeline/tests/golden/sharpen_*.png` | 3 new references | **Create** |
| `crates/app/src/view_region.rs` | owns `REGION_MARGIN`; asserts the margin invariant | Modify |
| `crates/app/src/main.rs` | use the moved constant; 4 new sliders | Modify |
| `crates/app/src/render_engine.rs` | extend the master invariant test | Modify |
| `crates/app/examples/bench_preview.rs` | 2 new perf cases | Modify |
| `CLAUDE.md` | status section | Modify |

Build/test commands throughout (macOS needs libraw on the pkg-config path):

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:$PKG_CONFIG_PATH"
```

---

## Task 1: `spatial::gaussian_blur`

**Files:**
- Modify: `crates/pipeline/src/spatial.rs`

- [ ] **Step 1: Write the failing tests**

Append these tests inside the existing `#[cfg(test)] mod tests` block at the bottom of `crates/pipeline/src/spatial.rs` (it already has `use super::*;`):

```rust
    #[test]
    fn kernel_radius_is_three_sigma_rounded_up() {
        assert_eq!(kernel_radius(1.0), 3);
        assert_eq!(kernel_radius(0.5), 2); // ceil(1.5)
        assert_eq!(kernel_radius(3.0), 9);
        // A non-positive or non-finite sigma has no kernel at all; gaussian_blur treats
        // that as the identity.
        assert_eq!(kernel_radius(0.0), 0);
        assert_eq!(kernel_radius(-1.0), 0);
        assert_eq!(kernel_radius(f32::NAN), 0);
    }

    #[test]
    fn non_positive_sigma_is_identity() {
        let p = vec![1.0, 2.0, 3.0, 4.0];
        assert_eq!(gaussian_blur(&p, 2, 2, 0.0), p);
        assert_eq!(gaussian_blur(&p, 2, 2, -2.0), p);
    }

    #[test]
    fn gaussian_constant_plane_stays_constant() {
        // Requires both halves: weights normalized to sum 1, *and* out-of-bounds reads
        // replicating the edge sample rather than contributing zero.
        let p = vec![0.375f32; 9 * 7];
        let out = gaussian_blur(&p, 9, 7, 1.5);
        for (i, v) in out.iter().enumerate() {
            assert!((v - 0.375).abs() < 1e-6, "sample {i} = {v}");
        }
    }

    #[test]
    fn gaussian_kernel_matches_the_analytic_weights() {
        // Independently recomputed from exp(-x^2 / 2*sigma^2), then normalized.
        let sigma = 1.0f32;
        let k = gaussian_kernel(sigma);
        assert_eq!(k.len(), 7, "radius 3 => 7 taps");
        let raw: Vec<f32> = (-3..=3)
            .map(|x| (-(x as f32 * x as f32) / (2.0 * sigma * sigma)).exp())
            .collect();
        let sum: f32 = raw.iter().sum();
        for (i, (&got, &r)) in k.iter().zip(raw.iter()).enumerate() {
            assert!((got - r / sum).abs() < 1e-6, "tap {i}: {got} vs {}", r / sum);
        }
        // A hard number, so the test is not just the implementation restated.
        assert!((k[3] - 0.399_05).abs() < 1e-4, "centre tap {}", k[3]);
        assert!((k.iter().sum::<f32>() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn impulse_response_is_symmetric_and_peaks_at_the_centre() {
        let (w, h) = (11usize, 11usize);
        let mut p = vec![0.0f32; w * h];
        p[5 * w + 5] = 1.0;
        let out = gaussian_blur(&p, w, h, 1.0);
        let centre = out[5 * w + 5];
        for y in 0..h {
            for x in 0..w {
                if (x, y) != (5, 5) {
                    assert!(out[y * w + x] < centre, "({x},{y}) exceeds the centre");
                }
                // Mirror symmetry in both axes.
                assert!((out[y * w + x] - out[y * w + (w - 1 - x)]).abs() < 1e-6);
                assert!((out[y * w + x] - out[(h - 1 - y) * w + x]).abs() < 1e-6);
            }
        }
        // Energy is conserved (edges replicate, so nothing leaks out of a centred impulse).
        assert!((out.iter().sum::<f32>() - 1.0).abs() < 1e-4);
    }

    #[test]
    fn larger_sigma_reduces_variance_more() {
        let (w, h) = (32usize, 32usize);
        let mut state: u32 = 0x1234_5678;
        let p: Vec<f32> = (0..w * h)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 17;
                state ^= state << 5;
                0.5 + (state as f32 / u32::MAX as f32 - 0.5) * 0.2
            })
            .collect();
        let var = |v: &[f32]| {
            let m = v.iter().sum::<f32>() / v.len() as f32;
            v.iter().map(|x| (x - m) * (x - m)).sum::<f32>() / v.len() as f32
        };
        let small = var(&gaussian_blur(&p, w, h, 0.5));
        let large = var(&gaussian_blur(&p, w, h, 2.0));
        assert!(large < small, "sigma 0.5 var {small}, sigma 2.0 var {large}");
        assert!(small < var(&p));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p pipeline spatial
```

Expected: FAIL to compile — `cannot find function 'gaussian_blur'` / `'kernel_radius'` / `'gaussian_kernel'`.

- [ ] **Step 3: Implement the primitive**

Insert this into `crates/pipeline/src/spatial.rs`, immediately **before** the `#[cfg(test)] mod tests` block (i.e. after `blur_cols`):

```rust
/// Half-width of the kernel for `sigma`: `ceil(3σ)`, where 99.7% of the weight lies.
///
/// Zero for a non-positive or non-finite sigma, which [`gaussian_blur`] treats as the
/// identity. Public because callers need it to reason about how far a pixel's output
/// reaches — see the region margin invariant in the app.
pub fn kernel_radius(sigma: f32) -> usize {
    if sigma.is_finite() && sigma > 0.0 {
        (3.0 * sigma).ceil() as usize
    } else {
        0
    }
}

/// Normalized Gaussian weights of length `2·kernel_radius(sigma) + 1`, index 0 at `-r`.
fn gaussian_kernel(sigma: f32) -> Vec<f32> {
    let r = kernel_radius(sigma) as isize;
    let denom = 2.0 * sigma * sigma;
    let mut k: Vec<f32> = (-r..=r)
        .map(|x| (-((x * x) as f32) / denom).exp())
        .collect();
    let sum: f32 = k.iter().sum();
    for v in k.iter_mut() {
        *v /= sum;
    }
    k
}

/// Separable Gaussian blur of a single-channel plane, with a **float** sigma.
///
/// Sits *alongside* [`box_blur`] rather than replacing it — they serve opposite regimes.
/// `box_blur` is O(1) in its radius, which is what makes noise reduction's 6–32 px windows
/// affordable. Sharpening works at σ = 0.5–3 px, where a direct kernel is cheaper (7 taps at
/// σ=1) **and** where an integer box radius cannot represent a sub-pixel blur at all.
///
/// Kernel radius `ceil(3σ)`, weights normalized to sum 1, and out-of-bounds reads **replicate
/// the edge sample** — so a constant plane stays exactly constant. Both passes are parallel
/// over rows, which `box_blur`'s vertical pass could not be because it carries a running sum.
///
/// `sigma <= 0` (or non-finite) returns a copy.
///
/// # Panics
/// Panics if `plane.len() != w * h`.
pub fn gaussian_blur(plane: &[f32], w: usize, h: usize, sigma: f32) -> Vec<f32> {
    assert_eq!(plane.len(), w * h, "plane size mismatch");
    if kernel_radius(sigma) == 0 || w == 0 || h == 0 {
        return plane.to_vec();
    }
    let kernel = gaussian_kernel(sigma);
    let horizontal = gauss_rows(plane, w, h, &kernel);
    gauss_cols(&horizontal, w, h, &kernel)
}

/// Horizontal pass. Rows are independent, so rayon splits them directly.
fn gauss_rows(src: &[f32], w: usize, h: usize, k: &[f32]) -> Vec<f32> {
    let r = (k.len() - 1) / 2;
    let mut out = vec![0.0f32; w * h];
    out.par_chunks_exact_mut(w)
        .zip(src.par_chunks_exact(w))
        .for_each(|(orow, srow)| {
            for (x, o) in orow.iter_mut().enumerate() {
                let mut acc = 0.0f32;
                for (i, &kw) in k.iter().enumerate() {
                    // Clamp the *sample index* (replicate), not the window: `x + i - r`
                    // saturating at 0 on the left, at `w - 1` on the right.
                    let sx = (x + i).saturating_sub(r).min(w - 1);
                    acc += kw * srow[sx];
                }
                *o = acc;
            }
        });
    out
}

/// Vertical pass. Unlike `box_blur`'s, this carries no running state, so each output row
/// can be accumulated independently — and every read is still a contiguous row slice.
fn gauss_cols(src: &[f32], w: usize, h: usize, k: &[f32]) -> Vec<f32> {
    let r = (k.len() - 1) / 2;
    let mut out = vec![0.0f32; w * h];
    out.par_chunks_exact_mut(w)
        .enumerate()
        .for_each(|(y, orow)| {
            for (i, &kw) in k.iter().enumerate() {
                let sy = (y + i).saturating_sub(r).min(h - 1);
                let srow = &src[sy * w..(sy + 1) * w];
                for (o, s) in orow.iter_mut().zip(srow) {
                    *o += kw * s;
                }
            }
        });
    out
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p pipeline spatial
```

Expected: PASS, including the six pre-existing `box_blur` tests.

- [ ] **Step 5: Commit**

```bash
git add crates/pipeline/src/spatial.rs && git commit -m "feat(pipeline): add spatial::gaussian_blur with a float sigma"
```

---

## Task 2: `Sharpening` in the edit model

**Files:**
- Modify: `crates/edit-model/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Append inside `#[cfg(test)] mod tests` in `crates/edit-model/src/lib.rs`:

```rust
    #[test]
    fn sharpening_defaults_to_off() {
        let s = Sharpening::default();
        assert_eq!(s.amount, 0.0);
        assert_eq!(s.masking, 0.0);
        // Radius is a physical pixel quantity, not a 0..100 slider; 1.0 px is the usual
        // starting point. Detail sits at its midpoint like every other modifier.
        assert_eq!(s.radius, 1.0);
        assert_eq!(s.detail, 50.0);
        assert_eq!(Edits::default().sharpening, s);
    }

    #[test]
    fn sharpening_is_active_only_on_amount() {
        assert!(!Sharpening::default().is_active());
        // Same rule as NoiseReduction and Vignette: the modifiers shape an effect that is
        // off until amount is non-zero, so dragging one alone must not insert a stage.
        assert!(!Sharpening {
            radius: 3.0,
            detail: 0.0,
            masking: 90.0,
            ..Default::default()
        }
        .is_active());
        assert!(Sharpening {
            amount: 1.0,
            ..Default::default()
        }
        .is_active());
    }

    #[test]
    fn old_sidecar_without_sharpening_still_loads() {
        let e: Edits = serde_json::from_str(r#"{"exposure": 0.5}"#).unwrap();
        assert_eq!(e.sharpening, Sharpening::default());
    }

    #[test]
    fn partial_sharpening_json_keeps_modifier_defaults() {
        let e: Edits = serde_json::from_str(r#"{"sharpening": {"amount": 60.0}}"#).unwrap();
        assert_eq!(e.sharpening.amount, 60.0);
        assert_eq!(e.sharpening.radius, 1.0);
        assert_eq!(e.sharpening.detail, 50.0);
        assert_eq!(e.sharpening.masking, 0.0);
    }

    #[test]
    fn sharpening_roundtrips_json() {
        let e = Edits {
            sharpening: Sharpening {
                amount: 70.0,
                radius: 2.5,
                detail: 30.0,
                masking: 80.0,
            },
            ..Default::default()
        };
        let json = serde_json::to_string(&e).unwrap();
        let back: Edits = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p edit-model sharpening
```

Expected: FAIL to compile — `cannot find type 'Sharpening'`.

- [ ] **Step 3: Add the type and the field**

In `crates/edit-model/src/lib.rs`, insert immediately **after** the `impl Vignette { ... }` block (i.e. before the `/// All edits applied to one image.` doc comment):

```rust
/// Unsharp-mask sharpening. `amount` 0 = off; the other three only shape the effect.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sharpening {
    /// Strength, 0..100. 0 = off.
    pub amount: f32,
    /// Blur radius (sigma) in **full-resolution pixels**, ~0.5..3.0. Scaled to whatever
    /// resolution is actually being rendered, so a preview approximates the export.
    pub radius: f32,
    /// Halo suppression, 0..100. 100 is a pure unsharp mask; lower values hold back
    /// sharpening where the local contrast is already high. Inert while `amount` is 0.
    pub detail: f32,
    /// Edge gate, 0..100. Higher confines sharpening to edges, sparing flat noisy areas.
    /// 0 = no gate. Inert while `amount` is 0.
    pub masking: f32,
}

impl Default for Sharpening {
    fn default() -> Self {
        Self {
            amount: 0.0,
            radius: 1.0,
            detail: 50.0,
            masking: 0.0,
        }
    }
}

impl Sharpening {
    /// True when these settings actually change pixels. Amount-only, the same rule
    /// [`NoiseReduction::is_active`] and [`Vignette::is_active`] follow.
    pub fn is_active(&self) -> bool {
        self.amount > 0.0
    }
}
```

Then add the field to `Edits`, on the line **after** `pub vignette: Vignette,`:

```rust
    pub sharpening: Sharpening,
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p edit-model
```

Expected: PASS (all pre-existing edit-model tests included).

- [ ] **Step 5: Commit**

```bash
git add crates/edit-model/src/lib.rs && git commit -m "feat(edit-model): add Sharpening params to Edits"
```

---

## Task 3: `SharpenStage` — the unsharp mask

Amount and Radius only; Detail and Masking come in Task 4 so each half gets its own red-green cycle.

**Files:**
- Create: `crates/pipeline/src/stages/sharpening.rs`
- Modify: `crates/pipeline/src/stages/mod.rs`
- Modify: `crates/pipeline/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/pipeline/src/stages/sharpening.rs` containing **only** this test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    /// A 64x64 image split by a hard vertical edge at x = 32.
    fn hard_edge() -> ImageBuf {
        let mut img = ImageBuf::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                img.set_pixel(x, y, if x < 32 { [0.10; 3] } else { [0.80; 3] });
            }
        }
        img
    }

    fn run(params: Sharpening, img: ImageBuf) -> ImageBuf {
        SharpenStage::new(&params, RenderScale::FULL).apply(img)
    }

    fn sharpen(amount: f32) -> Sharpening {
        Sharpening {
            amount,
            detail: 100.0,
            ..Default::default()
        }
    }

    #[test]
    fn amount_zero_is_an_exact_identity() {
        let src = hard_edge();
        assert_eq!(run(Sharpening::default(), src.clone()), src);
    }

    #[test]
    fn a_flat_region_is_untouched() {
        // d = 0 everywhere, so the gain is 1 and nothing moves.
        let mut src = ImageBuf::new(32, 32);
        for y in 0..32 {
            for x in 0..32 {
                src.set_pixel(x, y, [0.42, 0.30, 0.18]);
            }
        }
        let out = run(sharpen(100.0), src.clone());
        for (a, b) in out.data().iter().zip(src.data()) {
            assert!((a - b).abs() < 1e-6, "{a} vs {b}");
        }
    }

    #[test]
    fn a_hard_edge_gains_contrast() {
        // The entire point of the stage.
        let src = hard_edge();
        let out = run(sharpen(100.0), src.clone());
        let before = src.pixel(32, 32)[1] - src.pixel(31, 32)[1];
        let after = out.pixel(32, 32)[1] - out.pixel(31, 32)[1];
        assert!(after > before * 1.1, "edge contrast {before} -> {after}");
    }

    #[test]
    fn hue_ratios_are_preserved() {
        // Applying the correction as a scalar RGB gain is what buys this: no colour
        // fringing on a chromatic edge, and no Oklab round trip to pay for.
        let mut src = ImageBuf::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                let v = if x < 32 { 0.30 } else { 0.70 };
                src.set_pixel(x, y, [v, v * 0.6, v * 0.35]);
            }
        }
        let out = run(sharpen(60.0), src.clone());
        let mut moved = 0usize;
        for (o, s) in out.data().chunks_exact(3).zip(src.data().chunks_exact(3)) {
            if o[0] <= 0.0 {
                // A gain clamped to 0 is still a scalar multiply; there is simply no ratio
                // left to compare.
                continue;
            }
            assert!((o[1] / o[0] - s[1] / s[0]).abs() < 1e-4);
            assert!((o[2] / o[0] - s[2] / s[0]).abs() < 1e-4);
            if (o[0] - s[0]).abs() > 1e-6 {
                moved += 1;
            }
        }
        assert!(moved > 0, "nothing was sharpened, so nothing was tested");
    }

    #[test]
    fn radius_scales_with_the_render_scale() {
        // A full-resolution radius rendered at 1/4 scale is a quarter-size blur, so the
        // preview approximates the export rather than contradicting it.
        let p = Sharpening {
            amount: 100.0,
            radius: 2.0,
            ..Default::default()
        };
        assert_eq!(sigma_for(&p, RenderScale::FULL), 2.0);
        assert_eq!(sigma_for(&p, RenderScale::new(0.25)), 0.5);
        // Out-of-range radii are clamped rather than trusted.
        let wide = Sharpening {
            radius: 99.0,
            ..p
        };
        assert_eq!(sigma_for(&wide, RenderScale::FULL), RADIUS_RANGE.1);
    }

    #[test]
    fn never_emits_negative_or_nan_samples() {
        let src = hard_edge();
        let out = run(
            Sharpening {
                amount: 100.0,
                radius: 3.0,
                detail: 100.0,
                masking: 0.0,
            },
            src,
        );
        assert!(out.data().iter().all(|v| v.is_finite() && *v >= 0.0));
    }

    #[test]
    fn the_near_black_guard_bounds_the_gain() {
        // Without LUMA_EPS a near-black pixel beside a bright edge divides by ~0 and the
        // ratio explodes. GAIN_MAX then caps the halo at +2 stops.
        let mut src = ImageBuf::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                src.set_pixel(x, y, if x < 32 { [1e-6; 3] } else { [1.0; 3] });
            }
        }
        let out = run(sharpen(100.0), src.clone());
        for (o, s) in out.data().iter().zip(src.data()) {
            assert!(o.is_finite(), "non-finite sample {o}");
            assert!(*o <= s * GAIN_MAX + 1e-6, "{o} exceeds {s} * {GAIN_MAX}");
        }
    }
}
```

- [ ] **Step 2: Register the module so the tests compile**

In `crates/pipeline/src/stages/mod.rs`, add in alphabetical position (after `pub mod saturation;`):

```rust
pub mod sharpening;
```

In `crates/pipeline/src/lib.rs`, add after the `pub use stages::saturation::SaturationStage;` line:

```rust
pub use stages::sharpening::{max_kernel_radius, SharpenStage, RADIUS_RANGE};
```

- [ ] **Step 3: Run the tests to verify they fail**

```bash
cargo test -p pipeline sharpening
```

Expected: FAIL to compile — `cannot find type 'SharpenStage'` and friends.

- [ ] **Step 4: Implement the stage**

Prepend this to `crates/pipeline/src/stages/sharpening.rs`, above the test module written in Step 1:

```rust
use crate::spatial::{gaussian_blur, kernel_radius};
use crate::{PipelineStage, RenderScale};
use core_image::ImageBuf;
use edit_model::Sharpening;
use rayon::prelude::*;

/// Radius slider range, as sigma in **full-resolution pixels**.
///
/// Public because the app's region margin has to exceed the kernel this implies — see
/// [`max_kernel_radius`] and the invariant asserted in `app::view_region`.
pub const RADIUS_RANGE: (f32, f32) = (0.5, 3.0);

/// Sharpening strength at Amount 100.
const MAX_AMOUNT: f32 = 1.5;
/// Luma excursion above which Detail starts suppressing.
const HALO_KNEE: f32 = 0.05;
/// Sigma of the blur applied to `|d|` so the mask is smooth rather than speckled, in
/// full-resolution pixels. Scaled by `RenderScale` like the main sigma, or the mask would
/// change character between the fit and native render modes.
const MASK_SIGMA: f32 = 2.0;
/// Edge strength at which Masking 100 still sharpens fully.
const MASK_MAX: f32 = 0.08;
/// Near-black guard for the ratio. Load-bearing: without it a dark pixel beside a bright
/// edge has `luma ~ 0` and the gain explodes.
const LUMA_EPS: f32 = 1e-3;
/// Caps a sharpening halo in near-black regions at +2 stops.
const GAIN_MAX: f32 = 4.0;

/// Rec.709 luma, in linear light.
fn luma(px: &[f32]) -> f32 {
    0.2126 * px[0] + 0.7152 * px[1] + 0.0722 * px[2]
}

/// Clamped Hermite smoothstep. Same form as the one in `stages/vignette.rs`.
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The blur sigma actually used, in rendered pixels.
fn sigma_for(params: &Sharpening, scale: RenderScale) -> f32 {
    params.radius.clamp(RADIUS_RANGE.0, RADIUS_RANGE.1) * scale.get()
}

/// The widest neighbourhood one output pixel can depend on, in **rendered** pixels.
///
/// The two Gaussians compose: `d` reaches `kernel_radius(sigma)`, and the mask blurs `|d|`
/// a further `kernel_radius(MASK_SIGMA · scale)` on top of that. Any renderer that crops a
/// region before running this stage must keep at least this many pixels of margin outside
/// what it displays, or the region's edge band is computed from neighbours that do not
/// exist. `app::view_region` asserts exactly that against `REGION_MARGIN`.
pub fn max_kernel_radius(radius_px: f32, scale: RenderScale) -> usize {
    let s = scale.get();
    kernel_radius(radius_px.clamp(RADIUS_RANGE.0, RADIUS_RANGE.1) * s)
        + kernel_radius(MASK_SIGMA * s)
}

/// Unsharp-mask sharpening on Rec.709 luma, applied back as an RGB gain.
///
/// Working on luma and re-applying as a ratio means no colour fringing on a chromatic edge,
/// hue and saturation ratios preserved exactly, and no Oklab round trip — the cost that
/// dominates `HslStage` and `NoiseReductionStage`.
///
/// **Detail** suppresses halos where the local contrast is already high: at 100 `supp = 1`
/// and this is a pure unsharp mask; at 0, `supp -> 1` for small `|d|` (fine texture keeps
/// full sharpening) and `-> 0` for large `|d|`. Our own model, not Adobe's — defensible and
/// monotone rather than a clone, and genuinely distinct from Masking, which gates on *where*
/// rather than *how strong*.
///
/// Runs after the vignette, so darkened corners sharpen proportionally less in absolute
/// terms — correct for a ratio-based model.
pub struct SharpenStage {
    params: Sharpening,
    scale: RenderScale,
}

impl SharpenStage {
    pub fn new(params: &Sharpening, scale: RenderScale) -> Self {
        Self {
            params: *params,
            scale,
        }
    }
}

impl PipelineStage for SharpenStage {
    fn name(&self) -> &'static str {
        "sharpening"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        let (w, h) = (image.width(), image.height());
        if w == 0 || h == 0 || !self.params.is_active() {
            return image;
        }
        let n = w * h;

        let l: Vec<f32> = image.data().par_chunks_exact(3).map(luma).collect();
        let blurred = gaussian_blur(&l, w, h, sigma_for(&self.params, self.scale));
        let d: Vec<f32> = l.iter().zip(&blurred).map(|(a, b)| a - b).collect();

        let s = (self.params.detail / 100.0).clamp(0.0, 1.0);
        let k = (self.params.amount / 100.0).clamp(0.0, 1.0) * MAX_AMOUNT;

        let mut data = vec![0.0f32; n * 3];
        data.par_chunks_exact_mut(3)
            .enumerate()
            .zip(image.data().par_chunks_exact(3))
            .for_each(|((i, o), px)| {
                let dv = d[i];
                let supp = s + (1.0 - s) / (1.0 + (dv.abs() / HALO_KNEE).powi(2));
                let delta = k * dv * supp;
                let gain = (1.0 + delta / l[i].max(LUMA_EPS)).clamp(0.0, GAIN_MAX);
                o[0] = px[0] * gain;
                o[1] = px[1] * gain;
                o[2] = px[2] * gain;
            });
        ImageBuf::from_vec(w, h, data)
    }
}
```

`smoothstep`, `MASK_SIGMA` and `MASK_MAX` are wired up in Task 4; until then the compiler
will warn they are unused. To keep `clippy -D warnings` green *within this task*, temporarily
add `#[allow(dead_code)]` above `smoothstep`, `MASK_SIGMA` and `MASK_MAX`, and remove all
three attributes in Task 4 Step 3.

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cargo test -p pipeline sharpening
```

Expected: PASS — 7 tests.

- [ ] **Step 6: Commit**

```bash
git add crates/pipeline/src/stages/sharpening.rs crates/pipeline/src/stages/mod.rs crates/pipeline/src/lib.rs
git commit -m "feat(pipeline): add SharpenStage (unsharp mask on Rec.709 luma)"
```

---

## Task 4: Detail and Masking

**Files:**
- Modify: `crates/pipeline/src/stages/sharpening.rs`

- [ ] **Step 1: Write the failing tests**

Append inside the `#[cfg(test)] mod tests` block of `crates/pipeline/src/stages/sharpening.rs`:

```rust
    /// A 64x64 image of fine, low-amplitude texture: a 4 px-period sine on a mid gray.
    fn fine_texture() -> ImageBuf {
        let mut img = ImageBuf::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                let v = 0.40 + 0.008 * (x as f32 / 4.0 * std::f32::consts::TAU).sin();
                img.set_pixel(x, y, [v; 3]);
            }
        }
        img
    }

    /// A 64x64 flat gray patch under low-amplitude per-channel noise.
    fn noisy_flat() -> ImageBuf {
        let mut state: u32 = 0x9E37_79B9;
        let mut noise = move || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            ((state as f32 / u32::MAX as f32) * 2.0 - 1.0) * 0.01
        };
        let mut img = ImageBuf::new(64, 64);
        for y in 0..64 {
            for x in 0..64 {
                let v = 0.40 + noise();
                img.set_pixel(x, y, [v; 3]);
            }
        }
        img
    }

    /// Mean absolute change over the image interior (avoiding the replicated border).
    fn mean_abs_change(a: &ImageBuf, b: &ImageBuf) -> f32 {
        let (mut sum, mut count) = (0.0f32, 0usize);
        for y in 12..a.height() - 12 {
            for x in 12..a.width() - 12 {
                sum += (a.pixel(x, y)[1] - b.pixel(x, y)[1]).abs();
                count += 1;
            }
        }
        sum / count as f32
    }

    #[test]
    fn detail_zero_suppresses_a_strong_edge_far_more_than_fine_texture() {
        // The claim that makes Detail a distinct control: it gates on *how strong* the
        // local excursion is, so halos around a hard edge go while texture survives.
        // Sampled at x = 32, the pixel *on* the edge, where |d| is ~0.15 in luma — three
        // times HALO_KNEE. One pixel further out |d| has already collapsed to ~0.03 and
        // Detail correctly barely engages, so the sample point matters.
        let strong = |detail: f32| {
            let src = hard_edge();
            let out = run(
                Sharpening {
                    amount: 100.0,
                    detail,
                    ..Default::default()
                },
                src.clone(),
            );
            (out.pixel(32, 32)[1] - src.pixel(32, 32)[1]).abs()
        };
        let texture = |detail: f32| {
            let src = fine_texture();
            let out = run(
                Sharpening {
                    amount: 100.0,
                    detail,
                    ..Default::default()
                },
                src.clone(),
            );
            mean_abs_change(&out, &src)
        };
        let edge_ratio = strong(0.0) / strong(100.0);
        let texture_ratio = texture(0.0) / texture(100.0);
        assert!(edge_ratio < 0.2, "edge kept {edge_ratio} of its sharpening");
        assert!(
            texture_ratio > 0.8,
            "texture kept only {texture_ratio} of its sharpening"
        );
    }

    #[test]
    fn detail_100_is_a_pure_unsharp_mask() {
        // supp = 1 identically, so the result must not depend on HALO_KNEE at all.
        let src = hard_edge();
        let p = Sharpening {
            amount: 80.0,
            detail: 100.0,
            ..Default::default()
        };
        let out = run(p, src.clone());
        let l: Vec<f32> = src.data().chunks_exact(3).map(luma).collect();
        let blur = gaussian_blur(&l, 64, 64, sigma_for(&p, RenderScale::FULL));
        let (x, y) = (32usize, 32usize);
        let i = y * 64 + x;
        let expect_luma = l[i] + 0.8 * MAX_AMOUNT * (l[i] - blur[i]);
        let got_luma = luma(&out.pixel(x, y));
        assert!(
            (got_luma - expect_luma).abs() < 1e-4,
            "{got_luma} vs {expect_luma}"
        );
    }

    #[test]
    fn masking_spares_a_noisy_flat_patch_while_an_edge_still_sharpens() {
        // The pairing that justifies the control at all: one number must do both.
        let masked = |img: ImageBuf, masking: f32| {
            let out = run(
                Sharpening {
                    amount: 100.0,
                    detail: 100.0,
                    masking,
                    ..Default::default()
                },
                img.clone(),
            );
            (out, img)
        };

        let (flat_on, flat_src) = masked(noisy_flat(), 80.0);
        let (flat_off, _) = masked(noisy_flat(), 0.0);
        let kept_in_flat =
            mean_abs_change(&flat_on, &flat_src) / mean_abs_change(&flat_off, &flat_src);
        assert!(kept_in_flat < 0.2, "masking left {kept_in_flat} of the noise sharpening");

        // On the edge pixel the blurred |d| reaches ~0.066, just past Masking 80's
        // threshold of 0.064, so the gate is effectively fully open there.
        let (edge_on, edge_src) = masked(hard_edge(), 80.0);
        let (edge_off, _) = masked(hard_edge(), 0.0);
        let d_on = (edge_on.pixel(32, 32)[1] - edge_src.pixel(32, 32)[1]).abs();
        let d_off = (edge_off.pixel(32, 32)[1] - edge_src.pixel(32, 32)[1]).abs();
        assert!(d_on > d_off * 0.7, "the edge lost sharpening too: {d_on} vs {d_off}");
    }

    #[test]
    fn masking_zero_skips_the_gate_entirely() {
        // Not just "mask == 1 numerically": smoothstep(0, 0, ..) is degenerate, so the
        // branch has to be skipped rather than evaluated with a zero-width ramp.
        let src = hard_edge();
        let base = Sharpening {
            amount: 70.0,
            masking: 0.0,
            ..Default::default()
        };
        let out = run(base, src.clone());
        assert!(out.data().iter().all(|v| v.is_finite()));
        assert_ne!(out, src);
    }

    #[test]
    fn max_kernel_radius_covers_both_gaussians() {
        // radius 3 -> ceil(9) = 9, plus the mask's ceil(3 * 2) = 6.
        assert_eq!(max_kernel_radius(RADIUS_RANGE.1, RenderScale::FULL), 15);
        // Halving the render scale halves both sigmas.
        assert_eq!(max_kernel_radius(RADIUS_RANGE.1, RenderScale::new(0.5)), 8);
        // Monotone in the render scale, so a smaller preview never needs more margin.
        assert!(
            max_kernel_radius(RADIUS_RANGE.1, RenderScale::new(0.25))
                <= max_kernel_radius(RADIUS_RANGE.1, RenderScale::FULL)
        );
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p pipeline sharpening
```

Expected: FAIL — `masking_spares_a_noisy_flat_patch_while_an_edge_still_sharpens` fails its
first assertion (masking currently does nothing, so `kept_in_flat` is ~1.0).

- [ ] **Step 3: Implement Masking**

Remove the three `#[allow(dead_code)]` attributes added in Task 3 Step 4.

In `SharpenStage::apply`, insert this **between** the `let d: Vec<f32> = ...;` line and the
`let s = (self.params.detail / 100.0)...;` line:

```rust
        // Masking gates on *where* to sharpen. At 0 the whole branch is skipped: that is
        // both the cheap path and what avoids `smoothstep(0, 0, ..)` being degenerate.
        let masking = self.params.masking.clamp(0.0, 100.0);
        let mask: Option<Vec<f32>> = (masking > 0.0).then(|| {
            let magnitude: Vec<f32> = d.iter().map(|v| v.abs()).collect();
            let edge = gaussian_blur(&magnitude, w, h, MASK_SIGMA * self.scale.get());
            let hi = MASK_MAX * masking / 100.0;
            edge.iter().map(|&e| smoothstep(0.0, hi, e)).collect()
        });
```

Then change the `delta` line inside the `for_each` from

```rust
                let delta = k * dv * supp;
```

to

```rust
                let m = mask.as_ref().map_or(1.0, |mk| mk[i]);
                let delta = k * dv * supp * m;
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p pipeline
cargo clippy -p pipeline --all-targets -- -D warnings
```

Expected: PASS, no warnings.

- [ ] **Step 5: Commit**

```bash
git add crates/pipeline/src/stages/sharpening.rs
git commit -m "feat(pipeline): add Detail halo suppression and the Masking edge gate"
```

---

## Task 5: Wire sharpening into the pipeline tail

**Files:**
- Modify: `crates/pipeline/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

In `crates/pipeline/src/lib.rs`, first extend the test fixture. In `all_edits()`, add this
field immediately after the `vignette: ...` entry (the explicit field list is deliberate —
it fails to compile until the new field is placed, forcing the prefix/tail decision):

```rust
            sharpening: edit_model::Sharpening {
                amount: 60.0,
                masking: 40.0,
                ..Default::default()
            },
```

In `prefix_key_ignores_every_downstream_field`, add this entry to the `downstream` array,
after the `vignette` entry:

```rust
            Edits {
                sharpening: edit_model::Sharpening {
                    amount: 80.0,
                    ..Default::default()
                },
                ..base.clone()
            },
```

Then append these two tests to the same `mod tests`:

```rust
    #[test]
    fn sharpening_runs_after_the_vignette_and_before_the_output_transform() {
        // Sharpening a vignetted frame is correct for a ratio-based model: darkened
        // corners sharpen proportionally less in absolute terms.
        let edits = Edits {
            exposure: 1.0,
            vignette: edit_model::Vignette {
                amount: -40.0,
                ..Default::default()
            },
            sharpening: edit_model::Sharpening {
                amount: 50.0,
                ..Default::default()
            },
            ..Edits::default()
        };
        assert_eq!(
            build_pipeline(&edits, RenderScale::FULL).stage_names(),
            vec![
                "exposure",
                "vignette",
                "sharpening",
                "output_transform_srgb"
            ]
        );
    }

    #[test]
    fn sharpening_modifiers_alone_add_no_stage() {
        let edits = Edits {
            sharpening: edit_model::Sharpening {
                amount: 0.0,
                radius: 3.0,
                detail: 0.0,
                masking: 90.0,
            },
            ..Edits::default()
        };
        assert_eq!(build_pipeline(&edits, RenderScale::FULL).len(), 1);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p pipeline --lib
```

Expected: FAIL — `sharpening_runs_after_the_vignette_and_before_the_output_transform` gets
`["exposure", "vignette", "output_transform_srgb"]`.

- [ ] **Step 3: Add the stage to the tail**

In `build_pipeline_split_in_region`, insert between the `if edits.vignette.is_active() { ... }`
block and `tail.push(Box::new(OutputTransformSrgb));`:

```rust
    // The last spatial stage, and the only one in the tail. It therefore sets the margin a
    // region render must keep outside the visible rect — see `max_kernel_radius`. Placed
    // after the vignette so a darkened corner sharpens proportionally less in absolute
    // terms, which is what a ratio-based model should do.
    if edits.sharpening.is_active() {
        tail.push(Box::new(SharpenStage::new(&edits.sharpening, scale)));
    }
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p pipeline --lib
```

Expected: PASS. `the_prefix_holds_only_geometry_and_noise_reduction` must still pass —
sharpening is in the tail, so `PrefixKey` is deliberately untouched.

- [ ] **Step 5: Confirm the 20 existing goldens are byte-identical**

```bash
cargo test -p pipeline --test golden
git status --short crates/pipeline/tests/golden
```

Expected: 20 tests PASS and `git status` reports **no** modified reference PNGs — proof the
new stage is inert at its default.

- [ ] **Step 6: Commit**

```bash
git add crates/pipeline/src/lib.rs && git commit -m "feat(pipeline): run sharpening after the vignette in the tail"
```

---

## Task 6: Golden references

**Files:**
- Modify: `crates/pipeline/tests/golden.rs`
- Create: `crates/pipeline/tests/golden/sharpen_basic.png`, `sharpen_masked.png`, `sharpen_detail_low.png`

- [ ] **Step 1: Add the three cases**

In `crates/pipeline/tests/golden.rs`, change the import line

```rust
use edit_model::{Edits, NoiseReduction, ToneCurve, Vignette, WhiteBalance};
```

to

```rust
use edit_model::{Edits, NoiseReduction, Sharpening, ToneCurve, Vignette, WhiteBalance};
```

and append at the end of the file:

```rust
/// Sharpening cases run on `noise_chart()`, whose hard edge *and* noise are exactly what
/// sharpening interacts with — one chart covers halos, texture and the masking gate.
/// Amount and Radius are identical in all three, so the references differ only by the
/// control under test.
fn sharpen(detail: f32, masking: f32) -> Edits {
    Edits {
        sharpening: Sharpening {
            amount: 70.0,
            radius: 1.0,
            detail,
            masking,
        },
        ..Edits::default()
    }
}

#[test]
fn golden_sharpen_basic() {
    check_golden_with("sharpen_basic", &sharpen(50.0, 0.0), noise_chart());
}

#[test]
fn golden_sharpen_masked() {
    // Masking 80 should visibly spare the flat noisy bands while the edge still sharpens.
    check_golden_with("sharpen_masked", &sharpen(50.0, 80.0), noise_chart());
}

#[test]
fn golden_sharpen_detail_low() {
    check_golden_with("sharpen_detail_low", &sharpen(0.0, 0.0), noise_chart());
}
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test -p pipeline --test golden
```

Expected: the three new tests FAIL with `missing golden sharpen_*; run with UPDATE_GOLDEN=1`.

- [ ] **Step 3: Generate the references**

```bash
UPDATE_GOLDEN=1 cargo test -p pipeline --test golden
```

- [ ] **Step 4: Look at them, then verify**

A golden nobody has looked at is just a hash. Open all three and confirm by eye that the
edge in the bottom third is crisper than in `nr_luminance.png`'s source, that
`sharpen_masked.png` shows visibly less grain enhancement in the top two bands than
`sharpen_basic.png`, and that `sharpen_detail_low.png` has a weaker edge halo than
`sharpen_basic.png`. Then confirm mechanically that the three genuinely differ:

```bash
cd crates/pipeline/tests/golden && cmp sharpen_basic.png sharpen_masked.png; cmp sharpen_basic.png sharpen_detail_low.png
```

Expected: `cmp` reports a difference for both pairs (a silently untaken branch would make
them identical). Then:

```bash
cargo test -p pipeline --test golden
git status --short crates/pipeline/tests/golden
```

Expected: 23 tests PASS; `git status` shows exactly **three new** untracked PNGs and **no**
modifications to the 20 existing ones.

- [ ] **Step 5: Commit**

```bash
git add crates/pipeline/tests/golden.rs crates/pipeline/tests/golden
git commit -m "test(pipeline): add three sharpening goldens on the noise chart"
```

---

## Task 7: The margin invariant

`REGION_MARGIN` currently lives in the binary `main.rs`, where a test cannot reach it. Move it
into the lib beside the viewport maths it belongs to, and assert it there.

**Files:**
- Modify: `crates/app/src/view_region.rs`
- Modify: `crates/app/src/main.rs`

- [ ] **Step 1: Write the failing test**

Append inside `#[cfg(test)] mod tests` in `crates/app/src/view_region.rs`:

```rust
    #[test]
    fn the_region_margin_exceeds_every_tail_kernel_radius() {
        // ⚠ The invariant that keeps 1:1 panning seam-free.
        //
        // Noise reduction cannot produce a seam because it lives in the *prefix*, which
        // always runs whole-buffer. Sharpening lives in the *tail*, which runs on the
        // cropped region — so at the region's edge it reads neighbours that do not exist,
        // producing a band of wrong output `max_kernel_radius` px wide. `with_margin`
        // renders that much extra on every side, so the band is never displayed.
        //
        // Raising the Radius range, or adding another spatial stage to the tail, must fail
        // here rather than produce seams that only show up while panning at 1:1.
        let worst = pipeline::max_kernel_radius(pipeline::RADIUS_RANGE.1, RenderScale::FULL);
        assert!(
            REGION_MARGIN > worst,
            "REGION_MARGIN {REGION_MARGIN} must exceed the tail's kernel radius {worst}"
        );
    }
```

and add to the `use` line at the top of that test module:

```rust
    use pipeline::RenderScale;
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cargo test -p rustroom-app view_region
```

Expected: FAIL to compile — `cannot find value 'REGION_MARGIN' in this scope`.

- [ ] **Step 3: Move the constant into the lib**

In `crates/app/src/view_region.rs`, add just below the `pub use crate::render_engine::RegionRect;` line:

```rust
/// Rendered beyond the visible rect so small pans reuse the existing texture — and, since
/// sharpening runs on the cropped region, so its edge band falls outside what is displayed.
/// See `the_region_margin_exceeds_every_tail_kernel_radius`.
pub const REGION_MARGIN: usize = 128;
```

In `crates/app/src/main.rs`, delete these two lines (currently `main.rs:24-25`):

```rust
/// Rendered beyond the visible rect so small pans reuse the existing texture.
const REGION_MARGIN: usize = 128;
```

and change the one use site (`main.rs:630`) from

```rust
                            Some(view_region::with_margin(vis, iw, ih, REGION_MARGIN))
```

to

```rust
                            Some(view_region::with_margin(
                                vis,
                                iw,
                                ih,
                                view_region::REGION_MARGIN,
                            ))
```

- [ ] **Step 4: Run to verify it passes**

```bash
cargo test -p rustroom-app view_region
cargo build -p app
```

Expected: PASS; the binary builds.

- [ ] **Step 5: Commit**

```bash
git add crates/app/src/view_region.rs crates/app/src/main.rs
git commit -m "test(app): assert REGION_MARGIN exceeds the tail's spatial kernel radius"
```

---

## Task 8: Extend the master invariant

`a_region_render_matches_the_whole_render_cropped` would keep passing while silently no
longer covering its case, because its edit set has no sharpening. Add sharpening to it and
inset the comparison by the kernel radius.

**Files:**
- Modify: `crates/app/src/render_engine.rs`

- [ ] **Step 1: Rewrite the test**

In `crates/app/src/render_engine.rs`, generalize the existing 16x16 fixture. Replace

```rust
    fn varied() -> Arc<ImageBuf> {
        let mut img = ImageBuf::new(16, 16);
        for y in 0..16 {
            for x in 0..16 {
                img.set_pixel(
                    x,
                    y,
                    [x as f32 / 16.0, y as f32 / 16.0, ((x + y) % 5) as f32 / 5.0],
                );
            }
        }
        Arc::new(img)
    }
```

with

```rust
    fn varied_sized(n: usize) -> Arc<ImageBuf> {
        let mut img = ImageBuf::new(n, n);
        for y in 0..n {
            for x in 0..n {
                img.set_pixel(
                    x,
                    y,
                    [
                        x as f32 / n as f32,
                        y as f32 / n as f32,
                        ((x + y) % 5) as f32 / 5.0,
                    ],
                );
            }
        }
        Arc::new(img)
    }

    fn varied() -> Arc<ImageBuf> {
        varied_sized(16)
    }
```

Then replace the whole body of `a_region_render_matches_the_whole_render_cropped` with:

```rust
    #[test]
    fn a_region_render_matches_the_whole_render_cropped() {
        // The master invariant. It validates the region maths, the vignette correction and
        // the cache at once — if any of the three is wrong, this fails.
        //
        // ⚠ Sharpening *weakens* it, and that has to be said out loud rather than left to
        // an edit set that happens to omit sharpening. A spatial stage in the tail reads
        // neighbours the cropped region genuinely does not have, so equality can only hold
        // in the interior. The excluded band is `max_kernel_radius` wide — precisely what
        // `view_region::REGION_MARGIN` keeps off screen — and the inset is derived from the
        // radius rather than hard-coded, so it tracks any future change.
        const N: usize = 64;
        let buf = varied_sized(N);
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
            sharpening: edit_model::Sharpening {
                amount: 90.0,
                radius: 1.0,
                detail: 50.0,
                masking: 60.0,
            },
            ..Edits::default()
        };

        // max_dim = N is native for this NxN buffer, i.e. no downscale, so the render scale
        // is exactly 1.0 and the inset below is the full-resolution kernel.
        let whole = render_cached(&buf, &edits, N, 1.0, None, &mut PrefixCache::default());

        // Guard against the test passing trivially: sharpening must actually move pixels
        // here, or the "weakened invariant" it exists to cover is not being exercised.
        let unsharpened = render_cached(
            &buf,
            &Edits {
                sharpening: edit_model::Sharpening::default(),
                ..edits.clone()
            },
            N,
            1.0,
            None,
            &mut PrefixCache::default(),
        );
        assert_ne!(whole.rgba, unsharpened.rgba, "sharpening changed nothing");

        let inset = pipeline::max_kernel_radius(edits.sharpening.radius, RenderScale::FULL);
        let (x, y, w, h) = (12, 10, 40, 36);
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
            N,
            1.0,
            Some(RegionRect { x, y, w, h }),
            &mut PrefixCache::default(),
        );

        assert_eq!((got.width, got.height), (w, h));
        let (iw, ih) = (w - 2 * inset, h - 2 * inset);
        assert_eq!(
            crop_rgba(&got, inset, inset, iw, ih),
            crop_rgba(&whole, x + inset, y + inset, iw, ih),
            "the region's interior must match the whole render bit for bit"
        );
        assert_eq!(
            got.region,
            Some(pipeline::Region {
                full_w: whole.width,
                full_h: whole.height,
                x0: x,
                y0: y
            })
        );
    }
```

Add `RenderScale` to the test module's imports if it is not already in scope — the module's
`use super::*;` brings it in via the file's top-level `use pipeline::{... RenderScale}`, so no
change should be needed; if the compiler disagrees, add `use pipeline::RenderScale;` inside
`mod tests`.

- [ ] **Step 2: Run to verify it passes**

```bash
cargo test -p rustroom-app render_engine
```

Expected: PASS, including `every_cached_frame_matches_a_cold_render` and
`the_cache_keeps_one_entry_per_render_mode` (both still use the 16x16 `varied()`).

- [ ] **Step 3: Deliberately break it, to prove it bites**

Temporarily change `let inset = ...` to `let inset = 0;` and re-run:

```bash
cargo test -p rustroom-app a_region_render_matches
```

Expected: FAIL on the interior comparison — the edge band really does differ, so the inset
is doing work rather than decorating. Restore `inset` to `pipeline::max_kernel_radius(...)`
and re-run to confirm PASS.

- [ ] **Step 4: Commit**

```bash
git add crates/app/src/render_engine.rs
git commit -m "test(app): extend the region invariant to cover a spatial tail stage"
```

---

## Task 9: GUI sliders

**Files:**
- Modify: `crates/app/src/main.rs`

- [ ] **Step 1: Add the panel section**

In `crates/app/src/main.rs`, insert this immediately **before** the
`egui::CollapsingHeader::new("Noise Reduction")` block (currently `main.rs:515`) — Lightroom's
Detail panel puts sharpening above noise reduction, and the two are the halves of one idea:

```rust
                        egui::CollapsingHeader::new("Sharpening").show(ui, |ui| {
                            let sh = &mut s.edits.sharpening;
                            slider(ui, &mut sh.amount, 0.0..=100.0, "Amount");
                            // The one slider not on a 0..100 scale: it is a physical pixel
                            // quantity, in full-resolution pixels, and naming it in pixels
                            // is what makes "scaled for the render" comprehensible.
                            slider(ui, &mut sh.radius, 0.5..=3.0, "Radius (px)");
                            slider(ui, &mut sh.detail, 0.0..=100.0, "Detail");
                            slider(ui, &mut sh.masking, 0.0..=100.0, "Masking");
                        });
```

- [ ] **Step 2: Build and lint**

```bash
cargo build -p app --release
cargo clippy -p app --all-targets -- -D warnings
```

Expected: builds clean, no warnings. (egui view code is not unit-testable; it is verified by
running the app in Task 10.)

- [ ] **Step 3: Commit**

```bash
git add crates/app/src/main.rs && git commit -m "feat(app): add the four sharpening sliders to the Develop panel"
```

---

## Task 10: Measure, then look at it

**Files:**
- Modify: `crates/app/examples/bench_preview.rs`

- [ ] **Step 1: Add the perf cases**

In `crates/app/examples/bench_preview.rs`, add these two entries to the `cases` vector,
after the `"vignette -60"` entry:

```rust
        (
            "sharpening 70 (amount/radius/detail)",
            Edits {
                sharpening: edit_model::Sharpening {
                    amount: 70.0,
                    ..Default::default()
                },
                ..Edits::default()
            },
        ),
        (
            "sharpening 70 + masking 60 (second gaussian)",
            Edits {
                sharpening: edit_model::Sharpening {
                    amount: 70.0,
                    masking: 60.0,
                    ..Default::default()
                },
                ..Edits::default()
            },
        ),
```

- [ ] **Step 2: Run the benchmark**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo run --release -p app --example bench_preview <path-to-a-raw-file>
```

Record the numbers for both new cases and the `neutral` baseline — they go into `CLAUDE.md`
in Task 11. Ask the user for a RAW path if none is at hand.

- [ ] **Step 3: Full workspace verification**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --all
```

Expected: all three green, 23 goldens passing.

- [ ] **Step 4: Launch the GUI and check it by eye**

```bash
cargo run --release -p app
```

Confirm, and report to the user what to look at:
1. The **Sharpening** section shows four sliders; Radius reads 0.5–3.0 px.
2. At **fit** view the effect is nearly invisible — correct, not a bug (§7.1 of the spec):
   the 1600 px preview no longer contains the detail sharpening acts on.
3. Zoom past 1:1 (scroll in until the render switches to native) and Amount is clearly
   visible on fine texture.
4. Pan around at 1:1 with Amount 100 / Radius 3 — **no seams** at the texture boundaries as
   new regions are rendered. That is the margin invariant paying off in practice.
5. Masking 80 on a noisy sky spares the flat area while edges stay sharp.
6. Undo/redo (Ctrl+Z) steps through slider changes; export and reopen round-trips the
   values through the sidecar.

- [ ] **Step 5: Commit**

```bash
git add crates/app/examples/bench_preview.rs
git commit -m "bench(app): time sharpening in the preview path"
```

---

## Task 11: Update `CLAUDE.md`

**Files:**
- Modify: `CLAUDE.md`

- [ ] **Step 1: Update the pipeline order and crate map**

In the "Core architecture" section's pipeline diagram, the `sharpen` entry already appears —
verify it reads correctly relative to `vignette` and `geometry`; the implemented order is
`… → hsl → vignette → sharpen → output transform`, with geometry **first**, not last. Correct
the diagram to match what the code does.

In the crate-layout block, change the `pipeline/` line to say **12 stages** and
`spatial::box_blur + spatial::gaussian_blur`.

- [ ] **Step 2: Rewrite the RESUME HERE block**

Replace the "▶ NEXT ACTION" sentence with the remaining Phase 2 gaps (presets + copy/paste +
sync; batch export; texture/clarity/dehaze) and the standing perf items (parallelize
`render_engine::pack`; a second cache checkpoint so Straighten stops costing full price; a
background full-res decode; setting libraw's `user_qual`).

- [ ] **Step 3: Add the sharpening status section**

Insert a new section immediately above **"Vignette done"**, following the house style of the
sections around it (what landed, the ⚠ traps, measured numbers, accepted caveats):

```markdown
**Sharpening done — the "detail" pair is complete.** Twelve stages.

- **`spatial::gaussian_blur` is a *new* primitive alongside `box_blur`, not a replacement.**
  They serve opposite regimes: `box_blur` is O(1) in its radius, which is what makes NR's
  6–32 px windows affordable; sharpening works at σ 0.5–3 px, where a direct kernel is
  cheaper (7 taps at σ=1) **and** where an integer box radius cannot express a sub-pixel blur
  at all. Kernel radius `ceil(3σ)`, weights normalized, **edges replicate** (so a constant
  plane stays exactly constant), and *both* passes are rayon-parallel over rows — which
  `box_blur`'s vertical pass could not be, because it carries a running sum.
- **`SharpenStage`** — `d = luma − gaussian_blur(luma, σ)` on **Rec.709 luma in linear
  light**, applied back as a scalar RGB gain. That choice buys three things at once: no
  colour fringing on a chromatic edge, hue/saturation ratios preserved exactly, and **no
  Oklab round trip** — the cost that dominates HSL and NR.
- **Detail is our own model**, not Adobe's: `supp = s + (1−s)/(1 + (|d|/HALO_KNEE)²)`. At 100
  it is a pure unsharp mask; at 0 it suppresses halos where local contrast is already high
  while leaving fine texture at nearly full strength. Genuinely distinct from **Masking**,
  which gates on *where* (a blurred `|d|` through a smoothstep) rather than *how strong*.
  Masking 0 **skips the branch entirely** — both the cheap path and what avoids
  `smoothstep(0, 0, ..)` being degenerate.
- **The near-black guard is load-bearing.** `gain = 1 + Δ/max(luma, LUMA_EPS)` with
  `LUMA_EPS = 1e-3`; without it a dark pixel beside a bright edge divides by ~0. `GAIN_MAX
  = 4.0` then bounds a halo in near-black regions to +2 stops.
- **Radius is the one slider not on a 0–100 scale**, deliberately: it is σ in
  **full-resolution pixels**, multiplied by `RenderScale` — the same seam NR uses.
  `Sharpening::is_active()` is **amount-only**, the rule `NoiseReduction` and `Vignette`
  already follow.
- **⚠ The margin invariant is now real, and asserted.** NR can never seam because it lives in
  the *prefix*, which always runs whole-buffer. Sharpening lives in the **tail**, which runs
  on the *cropped* region — so at the region's edge it reads neighbours that do not exist.
  That band is `max_kernel_radius` wide (**15 px** at Radius 3 full-res: `ceil(3σ)` = 9 plus
  the mask's `ceil(3·2)` = 6), and `REGION_MARGIN` is 128, so it never reaches the screen.
  `view_region::the_region_margin_exceeds_every_tail_kernel_radius` fails loudly if the
  Radius range grows or another spatial stage joins the tail. `REGION_MARGIN` moved from
  `main.rs` into `view_region` precisely so a test could reach it.
- **⚠ The master invariant was *weakened*, and the test says so.**
  `a_region_render_matches_the_whole_render_cropped` can no longer be bit-identical at the
  region's edge — and never can be. It would have kept passing silently, because its edit set
  had no sharpening. So sharpening was **added** to that edit set and the comparison **inset
  by `max_kernel_radius`**, derived rather than hard-coded; an `assert_ne!` up front stops it
  passing trivially. The interior is still compared bit for bit.
- **Goldens now 23** — `sharpen_basic` / `sharpen_masked` (80) / `sharpen_detail_low` on the
  existing `noise_chart()`, whose hard edge *and* noise are exactly what sharpening interacts
  with. The 20 existing references stayed **byte-identical**.
- **Measured** (X-T4, 1600 px preview, release, best of 5): _fill in from Task 10_.
- **Accepted caveats:** (1) **judge sharpening at 1:1, not at fit** — at fit scale σ shrinks
  to ~0.26 and the kernel is effectively the identity, which is *correct*: neither the preview
  nor a fit-view of the export contains that detail. This is why 1:1 region rendering was
  built first. (2) Even at 1:1 the editing buffer is a **half-size** decode, so what you see
  corresponds to ~2× the radius the export applies, and the half-size path skips demosaic
  entirely while export runs AHD — bounded judgement, not exactness. (3) No output/print
  sharpening for resized exports; separate feature. (4) Detail is our own model. (5)
  Sharpening after the vignette means darkened corners sharpen less in absolute terms.
```

Fill the "Measured" line with the Task 10 Step 2 numbers.

- [ ] **Step 4: Add the plan to the docs index**

In the `Docs (docs/superpowers/)` paragraph, mark the sharpening spec as no longer
"plan not yet", and add `2026-08-06-sharpening.md` to the `plans/` list.

- [ ] **Step 5: Final verification**

```bash
cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --all
```

Expected: all green.

- [ ] **Step 6: Commit**

```bash
git add CLAUDE.md docs/superpowers/plans/2026-08-06-sharpening.md
git commit -m "docs: record the sharpening implementation"
```

---

## Done when

- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --all` green.
- Three new golden references committed; the 20 existing ones byte-identical.
- The margin invariant asserted by `the_region_margin_exceeds_every_tail_kernel_radius`.
- `a_region_render_matches_the_whole_render_cropped` includes sharpening and insets by
  `max_kernel_radius`.
- In the GUI: the four sliders work, participate in undo/redo, round-trip through the
  sidecar, and the effect is visible when zoomed to 1:1 with no seams while panning.
- `CLAUDE.md` updated: 12 stages, `gaussian_blur` alongside `box_blur` and why, the margin
  invariant, and measured perf.
