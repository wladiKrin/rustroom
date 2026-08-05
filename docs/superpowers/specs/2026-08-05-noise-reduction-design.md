# Noise reduction — design

Date: 2026-08-05
Status: approved

## Goal

Luminance and color noise reduction: the `detail` stage the pipeline order has always named
but never had. This is the biggest remaining quality gap — it is what keeps per-hue luminance
and saturation edits clean on high-ISO files, and it attacks the chroma speckle that the HSL
stage documents (`CHROMA_FADE`) at its source rather than after the fact.

It is also the **first spatial stage** in Rustroom. Every stage so far is per-pixel
(`map_samples` / `map_pixels`), which is why this design spends as much effort on the
architecture as on the filter.

## Non-goals

- Sharpening. It shares the "detail" heading and would reuse the same box-blur primitive, but
  it belongs late in the pipeline while NR belongs early — a separate feature.
- Wavelet / multi-scale denoising (what Lightroom and darktable actually do). Better quality,
  substantially bigger build; the guided filter chosen here leaves that upgrade path open.
- Per-ISO automatic defaults. NR defaults to off.
- Adobe-identical response. As with every other stage: a defensible model, not a clone.

## The problem that shapes everything: scale

A spatial filter's result depends on the resolution it runs at. The GUI edits a preview (RAW
half-size decode, then `downscale_to(1600)`); export renders full-res. A radius-8 filter on a
1600 px preview is simply not the same edit as radius-8 on a 6000 px file, so the slider would
lie about the result.

Two consequences:

1. **The pipeline becomes scale-aware** (section 1). Spatial stages scale their pixel radii;
   per-pixel stages ignore it.
2. **Radius-dependent kernels are ruled out.** With a scale-aware radius, a radius that is 8 px
   on the preview is ~30 px at full res. A bilateral filter is O(r²) per pixel: 289 taps on the
   preview but **~3600 taps** on a 24 Mpx export, ≈ 90 G ops. The guided filter is O(1) per
   pixel *regardless of radius* — that, not filter quality, is the deciding argument.

## 1. The scale seam

```rust
// crates/pipeline/src/lib.rs

/// Ratio of the rendered image's dimensions to the full-resolution original.
/// 1.0 = full res. Spatial stages multiply their pixel radii by this; per-pixel
/// stages ignore it entirely.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderScale(f32);

impl RenderScale {
    pub const FULL: RenderScale = RenderScale(1.0);
    /// Clamped to (0, 1] — a scale above 1 or at/below 0 is a caller bug, and a
    /// silently clamped radius beats a panic in the render worker.
    pub fn new(s: f32) -> Self;
    pub fn get(self) -> f32;
}

pub fn build_pipeline(edits: &Edits, scale: RenderScale) -> Pipeline;
```

### Where the number comes from

`RawMetadata.width/height` reports the **processed** size, so on a `DecodeOptions::preview()`
decode it already reads half-size and cannot serve as the full-res reference. We do not need
libraw for it: `half_size` is *our own* flag, so the factor is exactly 0.5 by construction.

```
scale = decode_options.source_scale()          // 0.5 for a RAW half_size decode, else 1.0
      × rendered_long_edge / buffer_long_edge   // render_once's own downscale_to ratio
```

Both factors are exactly known at the point they are applied. Changes:

- `raw_decode::DecodeOptions::source_scale() -> f32` — new, three lines, unit-tested. It lives
  next to `half_size` so it stays truthful if the decode options change.
- `app::decode_worker`: the injected decode fn returns `(ImageBuf, String, f32)` instead of
  `(ImageBuf, String)`, and `Decoded` gains `source_scale: f32`. `decode_for_edit` supplies 0.5
  for RAW (via `source_scale()`) and 1.0 for JPEG/PNG/TIFF, which are decoded full-size. The
  worker's four unit tests inject a fake decode fn, so they gain the extra return value.
- `render_engine::render_once(buffer, edits, max_dim, source_scale)` computes the product and
  passes `RenderScale::new(..)` to `build_pipeline`. `RenderEngine::new(buffer, source_scale,
  on_done)` stores it for the worker loop.
- `decode_worker` also calls `render_once` for the neutral before/after baseline; it passes the
  same `source_scale` it just produced.
- Export (`main.rs`), the CLI (`crates/rustroom`), and the golden tests pass `RenderScale::FULL`.

Rounding in libraw's half-size output (w/2 rather than exactly half) is irrelevant: the scale
only multiplies a radius that is rounded to whole pixels anyway.

## 2. Stage placement

```
geometry → NOISE REDUCTION → white balance → exposure → contrast →
tone regions → tone curve → saturation → hsl → output transform
```

Early, immediately after geometry. Denoise the source before anything amplifies it — this is
what real RAW converters do, and it prevents the HSL chroma speckle instead of smoothing it
afterwards. Late placement would mean a Shadows lift has already multiplied the noise up, so
the same slider value would do less.

Geometry stays first, so NR runs on the cropped pixels (fewer of them) and on data that
straighten's bilinear resample has already touched slightly.

**`CLAUDE.md`'s canonical pipeline-order line is corrected accordingly:** NR early, sharpening
stays late.

## 3. Working space

One Oklab round trip serves both halves, reusing `color::linear_srgb_to_oklab` /
`oklab_to_linear_srgb` from the HSL work. `L` is denoised by the guided filter, `a`/`b` by the
chroma blur, then a single conversion back with each channel clamped at 0 — the same
out-of-gamut convention `HslStage` uses.

Separating luminance from chroma is the point: chroma noise wants a wide, cheap blur (chroma is
genuinely low-frequency in real images) while luminance noise needs an edge-aware filter,
because that is where detail lives.

## 4. Shared primitive: `crates/pipeline/src/spatial.rs`

```rust
/// Separable running-sum box blur of a single-channel plane. O(1) per pixel in `r`.
/// Clamp-at-edge: the window shrinks at the borders and is normalized by its actual
/// sample count, so a constant plane stays exactly constant.
pub fn box_blur(plane: &[f32], w: usize, h: usize, r: usize) -> Vec<f32>;
```

Implementation: a horizontal running-sum pass parallelized over rows with rayon, then transpose,
then the same pass again, then transpose back. One tested code path serves both axes and both
are cache-linear. `r == 0` returns a copy.

Everything in section 5 is built from this one function. Both halves of NR need it, and a
future sharpening stage will too.

## 5. The filters

### Luminance — self-guided filter (He et al.)

Guide and signal are both `L`, which turns the guided filter into an edge-preserving smoother.
Four box blurs:

```
mI  = box(L, r)
mII = box(L², r)
var = max(mII − mI², 0)          // clamped: cancellation can push it slightly negative

slope  = var / (var + eps)
offset = (1 − slope) · mI

q  = box(slope, r) · L + box(offset, r)
L' = L + (q − L) · luminance/100
```

(The literature calls these coefficients `a` and `b`; this spec renames them **`slope`** and
**`offset`** because `a` and `b` are already the Oklab chroma channels two paragraphs down.
The implementation should use these names for the same reason.)

The behaviour is entirely in `slope`: where local variance greatly exceeds `eps` (an edge)
`slope → 1` and the pixel passes through untouched; where it is far below `eps` (noise)
`slope → 0` and the pixel becomes its local mean. `eps` is therefore literally "the variance
below which a neighbourhood is noise rather than structure", which is what the Detail slider
exposes.

### Color — multi-pass chroma blur

```
a' = a + (box²(a, r_col) − a) · color/100      // and the same for b
```

Two box passes approximate a Gaussian well enough to avoid box blur's cross-shaped artifacts,
and cost nothing extra asymptotically. Four box blurs total for the pair.

### Parameter mapping

Base radii are defined in **full-resolution pixels** (noise grain is a full-res phenomenon),
then `r = max(1, round(base · scale))`.

| slider | range | maps to |
|---|---|---|
| **Luminance** | 0–100, default 0 | blend strength on `L`; 0 = identity |
| **Detail** | 0–100, default 50 | `eps = 1e-3 · (1e-2)^(detail/100)` → 1e-3 (smooth) … 1e-5 (preserve). 50 → 1e-4, i.e. σ_L ≈ 0.01 |
| **Color** | 0–100, default 0 | blend strength on `a`,`b`; 0 = identity |
| **Smoothness** | 0–100, default 50 | `r_col = lerp(8, 32)` full-res px. 50 → 20 px |

The luminance radius is fixed at **6 full-res px** — it is the noise-grain scale, and with
Detail already exposing the quality trade-off there is nothing for a user to gain from a second
knob there.

The eps mapping is geometric (log-interpolated between the endpoints) because the quantity is a
variance spanning two decades; a linear slider over that range would be unusable at one end.

## 6. Edit model

```rust
/// Noise reduction strengths, all 0..100. Strengths default to 0 (off); the two
/// quality modifiers default to 50.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NoiseReduction {
    pub luminance: f32,
    pub detail: f32,
    pub color: f32,
    pub smoothness: f32,
}
// Manual Default (like CropRect): luminance 0, detail 50, color 0, smoothness 50.
```

`Edits` gains `pub noise_reduction: NoiseReduction`. Struct-level `#[serde(default)]` keeps
older sidecars loading; a partial `{"noise_reduction": {"color": 40}}` fills detail/smoothness
with 50.

Ranges are 0–100 rather than ±100 — negative noise reduction is not a thing.

### The neutrality test

`build_pipeline` adds the stage when **`luminance > 0.0 || color > 0.0`**, *not* when
`noise_reduction != NoiseReduction::default()`. Detail and Smoothness are modifiers: dragging
one on its own must not insert a stage that does nothing but pay for an Oklab round trip.
Inside the stage, each half is skipped independently when its own strength is 0, so
"Color only" never runs the guided filter.

Defaults stay at 0 even though Lightroom ships every RAW with Color = 25. A non-zero default
would break `Edits::default() == neutral`, the `neutral` golden, and the "add a stage only when
non-neutral" rule that the whole pipeline is built on. Not worth it.

## 7. UI

An `egui::CollapsingHeader("Noise Reduction")` in the Develop right panel, after the HSL groups,
holding four sliders over `0.0..=100.0`: Luminance, Detail, Color, Smoothness. They bind to
`Edits` through the existing `slider` closure, so undo/redo, live re-render, sidecar persistence
and export need no changes. The panel already has a `ScrollArea` from the HSL work.

## 8. Testing

**`spatial::box_blur` unit tests**

1. `r == 0` is an identity.
2. A constant plane stays exactly constant — the classic edge-normalization bug.
3. Known 1-D case: `[0,0,1,0,0]` with `r = 1` → `[0, ⅓, ⅓, ⅓, 0]`.
4. A radius larger than the image makes every pixel the global mean.
5. Blurring is symmetric: a horizontally mirrored input gives a mirrored output.

**`NoiseReductionStage` unit tests**

1. Zero strengths are an exact identity (whatever Detail/Smoothness say).
2. Luminance 100 substantially cuts the sample sd of a noisy flat patch. The xorshift
   `noisy_patch`/`sd` helpers are **copied into this stage's test module**, not extracted —
   a 20-line deterministic test fixture is not worth a shared test-support crate, and the two
   copies are free to diverge as each stage's tests need.
3. **Edge preservation:** a hard black/white edge keeps its contrast at Luminance 100 while the
   flat noisy patch smooths. This is the guided filter's entire claim and the test that would
   catch a botched `a` term.
4. Detail 100 leaves more residual sd than Detail 0 (monotone in the right direction).
5. Color 100 collapses chroma sd on a noisy colored patch while its mean hue survives.
6. Higher Smoothness leaves less residual chroma on a large-scale chroma blotch.
7. A neutral gray patch stays neutral (`a ≈ b ≈ 0` out).
8. No negative samples out.

**`RenderScale` tests**

- `RenderScale::FULL.get() == 1.0`; `new` clamps out-of-range input.
- A pipeline of per-pixel stages only gives bit-identical output at scale 0.25 and at 1.0.
- The radius and eps mappings are asserted **directly** rather than through an end-to-end
  scale comparison, which would be flaky for no added confidence.
- `DecodeOptions::source_scale()` is 0.5 for `preview()` and 1.0 for `linear_scene()`/`display()`.

**Golden tests**

New `noise_chart()`: 128×64, deterministic xorshift noise over three regions — flat gray, flat
colored, and a hard vertical black/white edge — so a single reference image covers smoothing,
chroma handling and edge preservation at once. Cases: `nr_luminance`, `nr_color`, `nr_both`.

**All 14 existing golden references must stay byte-identical.** Adding the `RenderScale`
argument changes nothing at 1.0, because no existing stage is spatial.

**Perf** — time the 1600 px preview render with NR off / color only / luminance only / both,
and record the numbers in `CLAUDE.md` alongside the HSL table. Expect the Oklab round trip to
dominate as it does for HSL, with the eight box blurs adding on top.

## 9. Accepted caveats

1. **Preview NR is approximate.** `downscale_to` has already averaged much of the noise away, so
   a given setting looks stronger on the preview than in the export even with the radius scaled.
   Inherent to editing a preview — Lightroom has the same caveat and answers it with "judge NR at
   1:1". Ours is judged at export, since our "100%" zoom is 100% of the *half-size* buffer (a
   pre-existing limitation, not introduced here).
2. **Chroma bleed** across strong color edges at high Color/Smoothness, worst where two colors
   share a luminance. Upgrade path: guide the chroma blur with `L`.
3. **Single-scale.** Luminance noise blotches larger than the radius survive untouched. Wavelets
   are the fix, deliberately deferred.
4. **NR runs first and re-runs on every slider move**, so with NR on, *every* adjustment gets
   slower — there is no per-stage caching. Mitigation is the already-planned preview/stage
   caching, out of scope here.
5. The guided filter's `eps` is not scale-corrected. Downscaling lowers noise variance but
   barely touches structure variance, so the preview smooths a little more eagerly than the
   export. Modelling that properly is not worth the complexity; caveat 1 already covers how to
   judge the result.

## Done when

- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --all` green.
- Three new golden references committed; the 14 existing ones byte-identical.
- `build_pipeline` takes a `RenderScale`, and all four call sites (render engine, export, CLI,
  goldens) pass the right one.
- In the GUI: the four sliders work, participate in undo/redo, and round-trip through the sidecar.
- `CLAUDE.md` updated — pipeline is 10 stages, the order line corrected (NR early), the scale
  seam described, and the measured perf recorded.
