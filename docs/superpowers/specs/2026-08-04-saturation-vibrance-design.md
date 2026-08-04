# Saturation & Vibrance — design

Date: 2026-08-04
Status: approved

## Goal

Add the first *color* adjustments to the develop pipeline: **saturation** (uniform chroma
scaling) and **vibrance** (chroma scaling weighted toward already-muted colors), exposed as
two sliders in the Develop right panel and persisted in the sidecar like every other edit.

This is the smallest useful step of "rounded-out Develop", and it exercises the pipeline
architecture exactly as intended: a new pure `PipelineStage`, a new `Edits` field pair, a
slider, a golden test — no changes to any existing stage.

## Non-goals

- HSL / per-hue controls, color grading, split toning (later, separate stages).
- Skin-tone protection in vibrance (Adobe weights hues; we do not).
- Adobe-identical response curves. This is a defensible MVP model, like the existing
  contrast / tone-regions models.

## Model

One stage handles both parameters, because both are chroma scaling around the same luma
axis; a single pass means one traversal and no ordering question between them.

Per pixel, in the linear scene-referred working space:

```
luma  = 0.2126*R + 0.7152*G + 0.0722*B          (Rec.709)
f_sat = 1 + saturation / 100                     // -100 -> 0 (gray), +100 -> 2x chroma
m     = max(R, G, B)
c     = clamp((m - min(R,G,B)) / max(m, EPS), 0, 1)   // current chroma, 0 for neutrals
f_vib = 1 + (vibrance / 100) * (1 - c)
f     = f_sat * f_vib
out   = luma + (rgb - luma) * f, each channel clamped at >= 0
```

Properties this gives us (and that the tests assert):

- `saturation == 0 && vibrance == 0` → `f == 1` → **exact identity**.
- A neutral pixel (`R == G == B`) is unchanged at any setting (its chroma is zero).
- `saturation == -100` → the pixel collapses to its luma (monochrome), luma preserved.
- Vibrance moves a muted pixel more than an already-saturated one (`c` close to 1 → `f_vib`
  close to 1).
- Negative channels are clamped to 0, matching `ContrastStage`'s convention.

### Accepted caveats

- **Negative vibrance uses the same `(1 - c)` weighting**, so it desaturates muted colors
  *more* than saturated ones. Symmetric and simple; Lightroom behaves differently. Documented,
  not fixed.
- Clamping at 0 on strong boosts can shift hue for near-clipped colors. Same trade-off the
  other linear-light stages already make.
- Luma is computed on linear sRGB primaries, which is what libraw hands us today.

## Pipeline position

`SaturationStage` is inserted **after `ToneCurveStage` and before `OutputTransformSrgb`**,
matching the documented canonical order (color comes after tone):

```
geometry → white balance → exposure → contrast → tone regions → tone curve →
saturation → output transform
```

`build_pipeline` adds it only when `saturation != 0.0 || vibrance != 0.0`.

## Surface changes

**`edit-model`** — two new fields on `Edits`:

```rust
/// Saturation, ~[-100, 100]. -100 = monochrome, 0 = no change, +100 = 2x chroma.
pub saturation: f32,
/// Vibrance, ~[-100, 100]. Like saturation but weighted toward already-muted colors.
pub vibrance: f32,
```

Both default to 0. `Edits` already carries struct-level `#[serde(default)]`, so **existing
sidecars keep loading unchanged** and newly written ones simply gain two keys.

**`pipeline`** — new file `crates/pipeline/src/stages/saturation.rs` exporting
`SaturationStage::new(saturation: f32, vibrance: f32)`; registered in `stages/mod.rs`,
re-exported from `lib.rs`, wired into `build_pipeline`.

**`app`** — two sliders in the Develop right panel, immediately after `Blacks`, range
`-100.0..=100.0`, added through the existing `slider` closure. Undo/redo (via
`drag_stopped`), live re-render, sidecar save/load, and export all come for free — no other
app change.

**CLI** — nothing to do; `rustroom render --edits` picks the fields up through `Edits`.

## Testing

**Unit tests** (in `saturation.rs`, no I/O, deterministic):

1. `zero_is_identity` — both params 0 leaves a colored pixel bit-close to unchanged.
2. `minus_100_is_monochrome` — `saturation = -100` gives `R == G == B == luma`.
3. `neutral_pixel_unchanged` — a gray pixel is untouched at `saturation = +80`.
4. `positive_saturation_widens_channel_spread` — max-min grows.
5. `vibrance_favours_muted_pixels` — with `vibrance = +50`, a muted pixel's chroma grows by a
   larger factor than a saturated pixel's.
6. `no_negative_samples` — a strong boost on a near-neutral dark pixel never emits < 0.

**Golden tests** (`crates/pipeline/tests/golden.rs`):

- `golden_saturation_plus50` — `saturation: 50.0` on the existing 4-band chart.
- `golden_vibrance_plus50` — `vibrance: 50.0` on a **new muted chart**. The existing chart's
  bands are pure primaries (chroma 1) and gray (chroma 0), on which vibrance is a no-op, so it
  would test nothing. The muted chart is a small set of desaturated color bands.

To support the second chart, `check_golden` gains a sibling
`check_golden_with(name, edits, chart)`; `check_golden` delegates to it with the existing
chart. **The 7 committed reference PNGs must stay byte-identical** — verified by running the
suite without `UPDATE_GOLDEN` after the refactor.

## Done when

- `cargo fmt --check`, `cargo clippy --all-targets`, `cargo test --all` all green on the
  workspace.
- The two new golden references are committed; the pre-existing ones are unchanged.
- The GUI (`cargo run --release -p app`) shows Saturation and Vibrance sliders that visibly
  and correctly change the image, participate in undo/redo, and round-trip through the sidecar.
- `CLAUDE.md` "Current status" and the crate-layout notes are updated in the same session.
