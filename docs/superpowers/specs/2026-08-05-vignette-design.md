# Vignette — design

Date: 2026-08-05
Status: approved

## Goal

A creative post-crop vignette: darken or brighten the frame's edges, with control over where
the falloff sits, how abrupt it is, and what shape it takes. This is the last small stage of
the "rounded-out Develop" set, after saturation/vibrance, HSL and noise reduction.

It is also the first stage whose output depends on a pixel's **position** rather than only its
value, which is why it needs a new `core-image` primitive (§4).

## Non-goals

- **Lens vignetting correction.** That is a different feature driven by lens profile data and
  belongs with `lensfun` in a later phase. This stage is creative, applied after the crop.
- **Highlights protection and Style modes** (Lightroom's Highlight Priority / Color Priority /
  Paint Overlay). Style is three genuinely different rendering models, not one model with a
  parameter; deferred deliberately.
- Adobe-identical response. As with every other stage: a defensible model, not a clone.

## Two things that come for free

**Post-crop behaviour.** `GeometryStage` crops and runs first, so every later stage already
receives the cropped frame. Lightroom needs a separate "post-crop vignette" concept to get
this; here it is structural, and the stage simply reads the dimensions of the image handed to
it.

**Scale invariance.** The vignette is defined in normalized frame coordinates — the falloff is
a fraction of the frame, not a pixel radius — so `VignetteStage::new` takes **no**
`RenderScale`. Preview and export agree exactly. This is the noise-reduction seam working as
designed: spatial stages consume the scale, position-dependent-but-normalized stages do not.

## 1. Coordinates and shape

Normalized and frame-centred:

```
u = (x as f32 + 0.5) / w * 2.0 - 1.0     // -1 … 1
v = (y as f32 + 0.5) / h * 2.0 - 1.0
```

Because each axis is normalized by its *own* half-dimension, `sqrt(u² + v²) = 1` already traces
an ellipse inscribed in the frame — the Roundness-0 shape needs no special case. `d` is 0 at
the centre and 1 on the inscribed ellipse; at the **corners** it depends on the shape, reaching
`√2 ≈ 1.414` at Roundness 0, about `1.12` at Roundness −100, and considerably more than `√2` at
Roundness +100 on a wide frame (a circle inscribed in the short dimension leaves the corners far
outside it). The falloff constants below are chosen against the Roundness-0 range.

One metric covers all three shapes:

```
d = ( |u·wu|^p + |v·wv|^p ) ^ (1/p)
```

| Roundness | `p` | `wu`, `wv` | shape |
|---|---|---|---|
| 0 (default) | 2 | 1, 1 | ellipse inscribed in the frame |
| 0 → +100 | 2 | long axis lerps 1 → `max(w,h)/min(w,h)` | true circle in pixel space |
| 0 → −100 | lerps 2 → `P_MAX` (6) | 1, 1 | rounded rectangle hugging the edges |

The two halves of the slider move different variables, and never both at once. Concretely:

```rust
let aspect = w.max(h) as f32 / w.min(h) as f32;   // >= 1
let (mut wu, mut wv, mut p) = (1.0, 1.0, 2.0);
if roundness > 0.0 {
    let t = (roundness / 100.0).min(1.0);
    // Stretch the LONG axis so d = 1 traces a circle of radius min(w,h)/2 in pixels.
    if w >= h { wu = 1.0 + (aspect - 1.0) * t } else { wv = 1.0 + (aspect - 1.0) * t }
} else if roundness < 0.0 {
    let t = (-roundness / 100.0).min(1.0);
    p = 2.0 + (P_MAX - 2.0) * t;
}
```

`p == 2.0` takes a plain `sqrt` fast path, so the `powf` pair is paid **only** when Roundness is
negative. Both `wu`/`wv` and `p` are computed once per `apply`, not per pixel — but `wu`/`wv`
depend on the image's aspect, which is known only at `apply` time (it is post-crop), so they
cannot move into `new`.

## 2. Falloff and Amount

```
r_mid = lerp(R_MID_MIN, R_MID_MAX, midpoint/100)          // 0.2 … 1.4
width = lerp(FEATHER_MIN, FEATHER_MAX, feather/100)       // 0.0 … 1.0
        .max(WIDTH_EPS)                                   // 1e-4: feather 0 is a genuinely
                                                          //  hard edge, and this is the only
                                                          //  thing standing between it and a
                                                          //  division by zero
t     = smoothstep(r_mid - width/2, r_mid + width/2, d)   // 0 inside, 1 outside
f     = clamp(amount/100, -1, 1) * t                      // -1 … 1

f <= 0:  rgb *= (1 + f)^2                            // exactly ×0 (black) at f=-1
f >  0:  rgb += (1 - rgb).max(0) * (1 - (1 - f)^2)   // exactly 1.0 (white) at f=+1
```

**Revised three times, every time from actually using it.** Originally `gain =
exp2(amount/100 · 2·t)`, i.e. ±2 EV.

1. **Endpoints too weak.** A multiplicative gain is strictly positive, so it can never reach
   black however large `MAX_EV` grows. Interpolating toward the endpoint is the only model
   that hits pure black and pure white exactly. The linear form `(1 + f)` was chosen first
   because it left the mid-range identical to the old model.
2. **Mid-range then too weak** relative to the new extremes. Both curves are now **squared
   about their own endpoint**, roughly doubling the effect at every point.

| Amount | original (`exp2`, ±2 EV) | linear `(1+f)` | **current `(1+f)²`** |
|---|---|---|---|
| −25 | ×0.707 | ×0.75 | ×0.563 (−0.83 EV) |
| −50 | ×0.5 (−1 EV) | ×0.5 (−1 EV) | ×0.25 (−2 EV) |
| −75 | ×0.354 | ×0.25 (−2 EV) | ×0.063 (−4 EV) |
| −100 | ×0.25 | ×0 | **×0 — black** |

3. **The halves were then mirrored.** Squaring only the darkening side left equal |amount|
   moving a pixel much further toward black than toward white (at ±25: 44% vs 25%).
   Brightening now uses `1 − (1 − f)²`, so the two are mirror images —
   `darkening_and_brightening_are_mirror_images` asserts it across ±25/50/75. This changed
   no golden, since all three use a negative amount, which is what confirmed the edit stayed
   inside the brightening branch.

The response is not symmetric in log space either: past −50 each further step is worth
progressively more. That is inherent to reaching true black at all, and is accepted.

The `max(0.0)` on the brightening branch leaves scene-referred samples already above 1.0
alone rather than dragging them back down toward white.

`smoothstep(e0, e1, x)` clamps `(x - e0) / (e1 - e0)` to `[0, 1]` then applies `t·t·(3 - 2t)`,
matching the inline form already used in `stages/hsl.rs`.

### Two properties worth naming

- **Neither branch can produce a negative sample** (`1 + f ≥ 0`, and the lift only ever raises
  a value), so this stage needs no clamp, unlike `HslStage` and `NoiseReductionStage`.
- **Darkening is a scalar multiply**, so `r:g:b` ratios are untouched and hue and saturation
  survive exactly. Brightening lerps toward white and therefore desaturates — that is what
  "toward white" means, not a defect. Both are asserted by tests rather than left as comments.

## 3. Pipeline position

```
geometry → noise reduction → white balance → exposure → contrast → tone regions →
tone curve → saturation → hsl → VIGNETTE → output transform
```

Last before the output transform: a creative effect laid over the finished grade. Eleven
stages. `build_pipeline` passes no scale to it.

## 4. New `core-image` primitive

The stage needs a pixel's coordinates, which `map_pixels` does not supply.

```rust
/// Like `map_pixels`, but `f` also receives the pixel's `(x, y)`. For effects whose result
/// depends on position — vignette today; graduated and radial filters, and lens corrections,
/// later.
pub fn map_pixels_xy<F: Fn(usize, usize, [f32; 3]) -> [f32; 3] + Sync + Send>(
    &self,
    f: F,
) -> ImageBuf;
```

Rayon-parallel **over rows** (`par_chunks_exact_mut(w * 3)` zipped with the source rows and
`enumerate()` for `y`), so each task owns a contiguous row and the result is bit-identical to a
sequential run — the same property `map_pixels` documents.

Adding this to `core-image` rather than looping inside the stage keeps the stage about the
vignette math, and the next position-dependent effect gets it for free.

## 5. Edit model

```rust
/// Post-crop vignette. `amount` 0 = off; the other three are modifiers.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Vignette {
    /// -100 darkens the edges by 2 EV, +100 brightens by 2 EV. 0 = off.
    pub amount: f32,
    /// Where the falloff sits, 0..100. Higher pushes it outward.
    pub midpoint: f32,
    /// -100 rounded rectangle … 0 frame ellipse … +100 circle.
    pub roundness: f32,
    /// Transition width, 0..100. 0 is a hard edge.
    pub feather: f32,
}
// Manual Default (like CropRect and NoiseReduction):
// amount 0, midpoint 50, roundness 0, feather 50.

impl Vignette {
    /// True when this actually changes pixels. Amount-only on purpose: midpoint, roundness
    /// and feather only shape an effect that is off until `amount` is non-zero.
    pub fn is_active(&self) -> bool { self.amount != 0.0 }
}
```

`Edits` gains `pub vignette: Vignette`; struct-level `#[serde(default)]` keeps older sidecars
loading, and a partial `{"vignette": {"amount": -40}}` fills the three modifiers with their
defaults.

Note `roundness` defaults to **0** while `midpoint` and `feather` default to **50** — 0 is
roundness's neutral (frame ellipse), and the midpoint of its ±100 range.

## 6. UI

An `egui::CollapsingHeader("Vignette")` in the Develop right panel, after Noise Reduction:
Amount (−100..=100), Midpoint (0..=100), Roundness (−100..=100), Feather (0..=100), through
the existing `slider` closure — so undo/redo, sidecar persistence and export need no changes.

## 7. Testing

**`map_pixels_xy` unit tests**

1. Coordinates arrive correctly: writing `[x, y, 0]` into every pixel reproduces the grid.
2. Dimensions preserved, and the identity closure returns an equal image.

**`VignetteStage` unit tests**

1. `amount = 0` is an exact identity whatever the other three say.
2. The centre pixel is untouched at the default midpoint/feather (`d = 0` ⇒ `t = 0` ⇒ gain 1).
3. A corner is darker than the centre at negative amount, brighter at positive.
4. `amount = -100` gives **exactly black** and `+100` **exactly white** where `t == 1`
   (midpoint and feather set low so the corner is unambiguously past the falloff). This is the
   whole point of the endpoint-interpolation model.
4b. `amount = -50` scales by exactly 0.25 (two stops), pinning the squared darkening curve so
   a future retune cannot silently shift the range people build a feel for.
5. Four-fold mirror symmetry: gain at `(x, y)` equals gain at the horizontally, vertically and
   doubly mirrored positions.
6. **Roundness +100 on a 128×64 frame gives equal gain at equal *pixel* distance** — `+32 px`
   horizontally vs `+32 px` vertically from the centre.
7. **Roundness 0 on that same frame gives equal gain at equal *fractional* distance** — half
   way to the edge horizontally vs vertically.
   Tests 6 and 7 together are what actually pin down the aspect handling; either alone passes
   for the wrong reasons on a square image.
8. Roundness −100 leaves a **corner pixel less darkened** than Roundness 0 does. Raising `p`
   *lowers* `d` off the axes — at `(u,v) = (1,1)`, `d` goes from `√2 ≈ 1.414` at `p = 2` to
   `2^(1/6) ≈ 1.122` at `p = 6` — so the untouched interior becomes a rounded rectangle and
   the darkening concentrates along the edges instead of pooling in the corners. Getting the
   direction of this backwards is the easy mistake, which is why it is asserted.
9. Hue is preserved: `r:g:b` ratios unchanged for a coloured pixel.
10. No negative samples, and no NaN, at extreme settings.
11. `feather = 0` does not divide by zero and yields a hard-edged transition.

**Scale invariance** — the gain at a given `(u, v)` is asserted resolution-independent
directly: the same normalized position on a 64×64 and a 256×256 frame gets the same gain.
This is the property `RenderScale` exists for, obtained here for free.

**Golden tests** — three on the existing 64×16 `chart()`, whose 4:1 aspect makes the shape
differences plain: `vignette_darken` (amount −60), `vignette_circle` (amount −60, roundness
+100), `vignette_square` (amount −60, roundness −100). **The 17 existing references must stay
byte-identical.**

**Perf** — add a vignette case to `crates/app/examples/bench_preview.rs` and record the number
in `CLAUDE.md`. Expect single digits of ms (one `sqrt`, one `exp2`, three multiplies per
pixel), i.e. far below HSL and NR, which both pay an Oklab round trip. If it lands materially
worse, say so rather than bury it.

## 8. Accepted caveats

1. **The vignette follows the crop**, because it reads the post-geometry frame. Recropping
   reshapes it. Correct, and what Lightroom does — but worth knowing if a crop is changed late.
2. **In crop mode the preview shows the uncropped vignette**, since `preview_edits` resets the
   crop to the full frame; it snaps when crop mode is left. The same trade-off already accepted
   for every other stage.
3. **No highlight protection.** A strong negative Amount pulls bright corners down with
   everything else; Lightroom's Highlight Priority exists precisely to avoid that. Deferred.
4. Positive Amount lifts toward white and stops there; it does not push already-clipped
   highlights further. Samples already above 1.0 are left alone.
5. An extreme Midpoint/Feather combination can put `r_mid - width/2` below zero, which
   darkens the centre too. That is a legitimate look, not a bug, so it is not clamped — but it
   means "the centre is always untouched" holds only for sane settings, and test 2 asserts it
   at the defaults rather than universally.

## Done when

- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --all` green.
- Three new golden references committed; the 17 existing ones byte-identical.
- In the GUI: the four sliders work, participate in undo/redo, and round-trip through the
  sidecar.
- `CLAUDE.md` updated: pipeline is 11 stages, `map_pixels_xy` noted in the `core-image` row,
  the "vignette needs no RenderScale" point recorded, and the measured perf added.
