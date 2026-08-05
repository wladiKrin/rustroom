# HSL (per-hue color adjustments) — design

Date: 2026-08-05
Status: approved

## Goal

Per-hue Hue / Saturation / Luminance control, Lightroom-style: 8 color bands × 3 sliders.
This is the last of the "core Develop" color tools and the first stage that needs a
perceptual color space, so it also lands **Oklab** in the `color` crate for later reuse
(color grading, a better vibrance).

## Non-goals

- Targeted-adjustment ("click a pixel and drag") — a later UI feature.
- Color grading / split toning wheels.
- Adobe-identical band shapes or response curves. As with the other stages, this is a
  defensible model, not a clone.

## Color space: Oklab

New module `crates/color/src/oklab.rs`:

```rust
pub fn linear_srgb_to_oklab(rgb: [f32; 3]) -> [f32; 3];   // -> [L, a, b]
pub fn oklab_to_linear_srgb(lab: [f32; 3]) -> [f32; 3];
```

Ottosson's matrices with a cube-root non-linearity, ~40 lines and no new dependency. Inputs
may exceed 1.0 (scene-referred) — Oklab extends fine. `f32::cbrt` handles negatives, so
slightly negative samples survive the round trip instead of producing NaN.

Why not HSV-on-linear-RGB: hue bands there are perceptually uneven (an enormous "green", a
sliver of "orange") and a luminance change visibly drags hue with it. Per-hue work is exactly
where that breaks down.

**Tests:** round-trip `linear -> Oklab -> linear` within 1e-4 for a spread of colors including
neutrals, primaries, and an over-1.0 highlight; neutral colors give `a ≈ b ≈ 0`; `L` increases
monotonically with luminance.

## The stage

`crates/pipeline/src/stages/hsl.rs`, `HslStage::new(&Hsl)`.

Per pixel, in the linear working space:

1. `[L, a, b] = linear_srgb_to_oklab(rgb)`; `C = hypot(a, b)`; `h = atan2(b, a)` in degrees,
   normalized to `[0, 360)`.
2. If `C < EPS` the pixel is neutral — return it untouched (no hue to adjust).
3. **Band weights.** The two band centers bracketing `h` receive a smoothstep blend of the
   position between them; every other band gets 0. Weights therefore always sum to 1, which
   gives the property that setting the *same* saturation on all 8 bands equals a uniform
   global saturation change (asserted in a test).
4. Accumulate: `Δh = Σ wᵢ·hueᵢ·0.3` (±100 → ±30°), `f_sat = 1 + Σ wᵢ·satᵢ/100`
   (±100 → chroma ×0…×2), `f_lum = 1 + Σ wᵢ·lumᵢ/200` (±100 → L ×0.5…×1.5).
5. `h' = h + Δh`, `C' = max(C · f_sat, 0)`, `L' = max(L · f_lum, 0)`;
   `a' = C'·cos h'`, `b' = C'·sin h'`.
6. `oklab_to_linear_srgb([L', a', b'])`, each channel clamped at 0 (out-of-gamut results).

### Band centers

The 8 bands are red, orange, yellow, green, aqua, blue, purple, magenta. Their centers are
Oklab hue angles, stored as a constant table — **computed with our own conversion** from
canonical sRGB colors (`#FF0000`, `#FF8000`, `#FFFF00`, `#00FF00`, `#00FFFF`, `#0000FF`,
`#8000FF`, `#FF00FF`) rather than copied from an external source. A unit test recomputes each
angle and asserts it matches the stored constant within 1°, so the table cannot drift away
from the conversion it describes.

### Accepted caveats

- Strong boosts push colors outside sRGB; clamping at 0 shifts hue there. Same trade-off the
  other stages make.
- Band weighting is positional, so a hue shift can move a color into a neighbouring band's
  influence. This is inherent to the model and matches how Lightroom behaves in practice.
- `L · f_lum` is a multiplicative lightness change, not a tone-mapped one; extreme luminance
  settings on already-bright colors will clip.

## Edit model

```rust
/// One hue band's adjustments; all ~[-100, 100], 0 = neutral.
pub struct HslBand { pub hue: f32, pub saturation: f32, pub luminance: f32 }

/// The 8 bands, in `HSL_BAND_NAMES` order.
pub struct Hsl { pub bands: [HslBand; 8] }

pub const HSL_BAND_NAMES: [&str; 8] =
    ["Red", "Orange", "Yellow", "Green", "Aqua", "Blue", "Purple", "Magenta"];
```

`Edits` gains `pub hsl: Hsl`. Struct-level `#[serde(default)]` keeps older sidecars loading.
The sidecar representation is a positional array rather than named keys — the order is fixed
and documented by `HSL_BAND_NAMES`; the payoff is that the stage and the UI are both simple
loops.

## Pipeline position

After `SaturationStage`, before `OutputTransformSrgb`:

```
geometry → white balance → exposure → contrast → tone regions → tone curve →
saturation → hsl → output transform
```

`build_pipeline` adds it only when `edits.hsl != Hsl::default()`, so users who never touch HSL
pay nothing for the Oklab round trip.

## UI

Three `egui::CollapsingHeader`s in the Develop right panel — **Hue**, **Saturation**,
**Luminance** — each holding 8 sliders labelled by `HSL_BAND_NAMES`, all collapsed by default.

The right panel must gain an `egui::ScrollArea::vertical()`: 24 extra sliders do not fit
otherwise. That is a small change to existing panel code and is in scope here.

Sliders bind to `Edits` through the existing `slider` closure, so undo/redo, live re-render,
sidecar persistence and export need no changes.

## Testing

**`color::oklab` unit tests** — as listed above (round trip, neutral axis, monotone L).

**`HslStage` unit tests:**

1. Default `Hsl` is an exact identity.
2. A neutral (gray) pixel is unchanged even with every band cranked.
3. Band centers match recomputed Oklab hues within 1°.
4. Weights sum to 1 for a sweep of hues across the full circle.
5. Red-band saturation +100 raises a red pixel's chroma while a blue pixel is untouched.
6. The same saturation on all 8 bands matches a uniform chroma scale (within tolerance).
7. A hue shift of +100 on one band rotates that band's hue by ≈30°.
8. No negative samples out.

**Golden tests** on a new **hue-sweep chart** (x sweeps hue 0–360 at fixed saturation and
value, converted from sRGB to linear): `hsl_red_desaturated`, `hsl_blue_hue_shift`,
`hsl_green_luminance`. The existing charts and all 11 committed references stay untouched.

**Perf check:** time the 1600 px preview render with and without a non-neutral `Hsl` and
record the numbers in `CLAUDE.md`. Expectation is ~28 ms → 60–80 ms; if it lands materially
worse, say so rather than bury it (a LUT over hue is the obvious optimization, deliberately
not done now).

## Done when

- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --all` green.
- Three new golden references committed; the 11 existing ones unchanged.
- In the GUI: the three collapsible groups work, the panel scrolls, sliders participate in
  undo/redo, and values round-trip through the sidecar.
- `CLAUDE.md` updated: `color` gains Oklab, pipeline is 9 stages, measured perf recorded.
