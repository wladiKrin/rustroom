# 1:1 region rendering — design

Date: 2026-08-06
Status: approved

## Goal

Make zoom actually re-render. Today `zoom` only magnifies the 1600 px preview texture, so
zooming in shows bigger blurry pixels rather than more detail. Above a threshold, render the
**visible region at the editing buffer's native resolution** instead.

This exists so that detail-level edits can be judged in the app at all.

## Why this comes before sharpening

Sharpening was designed first and then parked, because it cannot be judged on a fit-to-screen
preview. At a preview scale of 0.256, a full-resolution radius of σ=1.0 becomes σ=0.256 — a
Gaussian whose kernel is `[0.0005, 0.999, 0.0005]`, i.e. the identity. Sharpening would do
**nothing** visible while editing.

That is not a bug: the 1600 px preview no longer contains the detail sharpening acts on, and a
viewer looking at the exported JPEG scaled to fit their screen would not see it either. The
only honest fix is to show more pixels, which is this feature. Building sharpening first would
have meant tuning a control by exporting after every change.

**Sharpening's design decisions are settled and should not be re-derived** when it resumes:

- Controls: Amount, Radius, Detail, Masking (Lightroom's set).
- Operates on **Rec.709 luma**, applied back as a ratio `rgb · (luma'/luma)` with a near-black
  guard — no colour fringing, hue preserved, no Oklab round trip. The weights currently live
  privately in `stages/saturation.rs` and want extracting.
- Needs a new **float-sigma Gaussian** in `spatial`, *not* `box_blur`: sharpening radii are
  0.5–3 px, where integer box radii cannot represent sub-pixel blur at all, and a direct
  kernel is both cheaper (7 taps) and more accurate. `box_blur` stays for NR's 6–32 px windows,
  where O(1)-in-radius is what makes it affordable.
- Detail is our own model: `supp = s + (1−s)/(1 + (|d|/HALO_KNEE)²)` where `s = detail/100`, so
  100 is a pure unsharp mask and 0 suppresses halos at strong edges while keeping fine texture.
- Position in the pipeline: after vignette, before the output transform — in the cache's tail.

Once this feature lands, sharpening needs **no `SIGMA_MIN` clamp**: at buffer-native the scale
is 0.5, so σ=1.0 becomes σ=0.5, a real blur.

## What the measurements said

Full-resolution decode was assumed too expensive. It is not, and the assumption was wrong for
an instructive reason. Measured on the X-T4 `.RAF` (6246×4170), unpack a flat ~850 ms:

| `user_qual` | process | total |
|---|---|---|
| 0 bilinear | 405 ms | 1.26 s |
| 1 VNG | 2,310 ms | 3.16 s |
| 2 PPG | 6,782 ms | 7.64 s |
| **3 AHD** — libraw's default, which we never overrode | **10,148 ms** | 10.97 s |

So ~93% of a full-res decode is demosaic, selected by a default nobody chose. **That is tracked
separately** (see the spawned task on setting `user_qual` explicitly); it also means every
export currently pays ~11 s for AHD.

It does *not* change this design: the editing buffer is already decoded and in memory at
3123×2085 — **2× the linear detail of the preview, for free**. A background full-resolution
upgrade is a contained follow-up that reuses all the machinery here, changing only the source
buffer.

**The planned Phase 5 `wgpu` port would not help here.** That port moves *our* `PipelineStage`
implementations to compute shaders; those 10 seconds are inside libraw's `dcraw_process`,
entirely upstream of our pipeline. GPU demosaic would mean replacing libraw's — and on X-Trans
that needs Markesteijn/AMaZE-class work, whose errors show up precisely as the fine-detail
artifacts one zooms in to inspect. A separate project, not a side-effect.

## Non-goals

- True full-resolution 1:1 (needs the background decode above).
- Tiled rendering for images larger than memory.
- Fixing the before/after baseline, which stays at preview resolution.
- Sharpening itself — this unblocks it.

## 1. Two render modes

A continuously-varying `max_dim` would rebuild the cache on every scroll tick, so there are two
quantized modes:

| mode | `max_dim` | engaged when |
|---|---|---|
| **Fit** | 1600 (`PREVIEW_MAX_DIM`) | `fit · zoom ≤ 1.0` |
| **Native** | buffer long edge (3123 here) | `fit · zoom > 1.0` |

The threshold falls out of the existing canvas code: `draw = tex_size · fit · zoom` where
`tex_size` is the 1600 px preview, so the texture is magnified past its own pixels exactly when
`fit · zoom > 1`.

Native mode renders native pixels even when the on-screen scale is between 0.51 and 1.0, which
is slightly more than displayed. Accepted: a third mode buys little and costs another cache
entry.

**Fit mode is unchanged in every respect** — it renders the whole image with `region = None`,
which is exactly today's behaviour and the same code path. Everything in this design is
additive to a mode that already works.

## 2. Region rendering, between prefix and tail

```
buffer → [prefix: geometry, NR]  → cached at native (~78 MB)
       → crop to the needed region
       → [tail: WB … vignette, output] → pack → texture
```

Two properties make this the right cut:

- **No inverse geometry.** The prefix output *is* the post-geometry image, which is exactly
  what the viewport displays, so screen→region is a plain scale and offset.
- **No seams, ever.** NR runs over the whole buffer before the crop, so no region boundary can
  produce an edge artifact however the user pans. This was the deciding argument against
  cropping first and giving each spatial stage a margin.

### Panning without re-rendering constantly

The rendered region is the visible rect **plus `REGION_MARGIN = 128` px** on each side, clamped
to the image. A new render is requested only when the visible rect is no longer contained in
the rendered rect, so small pans reuse the existing texture.

`RenderedFrame` gains the region it represents, because the canvas must know where to draw it:

```rust
pub struct RenderedFrame {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
    /// Where this frame sits in the post-geometry image. `None` = the whole image.
    pub region: Option<Region>,
}
```

## 3. `Region`, added additively

Cropping before the tail breaks `VignetteStage`, the only position-dependent stage: it derives
normalized frame coordinates from the dimensions of the image it receives, so given a crop it
would centre the vignette on the crop.

`Region` is therefore **optional**, which keeps the change additive:

```rust
/// Where a cropped render sits inside the full frame. `None` means "this buffer *is* the
/// full frame" — what every existing caller implicitly passes, and what position-dependent
/// stages already assume.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Region {
    pub full_w: usize,
    pub full_h: usize,
    pub x0: usize,
    pub y0: usize,
}

pub fn build_pipeline_split_in_region(
    edits: &Edits, scale: RenderScale, region: Option<Region>,
) -> (Pipeline, Pipeline);

// Unchanged, delegating with `None`:
pub fn build_pipeline_split(edits: &Edits, scale: RenderScale) -> (Pipeline, Pipeline);
pub fn build_pipeline(edits: &Edits, scale: RenderScale) -> Pipeline;
```

`VignetteStage::new_in_region(params, Option<Region>)`; in `apply`, frame dimensions and the
pixel offset come from the region when present and from the image itself when absent — so the
existing behaviour *is* the `None` branch rather than a reimplementation of it.

**Zero call-site churn**: the CLI, export and all 20 goldens keep compiling untouched. The cost
is two near-duplicate builder functions, which is honest about doing two different things.

Rejected alternatives, for the record:

- **Moving vignette into the prefix** (no seam needed at all): it would then run *before* the
  tone curve, so the curve would re-map it, and its "+100 reaches exactly white" property
  depends on 1.0 meaning display white. It would also make every vignette slider invalidate the
  NR cache.
- **`ImageBuf` carrying `origin`/`full_size`**: no API change anywhere, but it puts a rendering
  concern in the core type, obliges every future buffer-producing operation to maintain it or
  be silently wrong, and changes `ImageBuf`'s `PartialEq`, which existing tests compare on.
- **Folding `RenderScale` and `Region` into a `RenderContext`**: the cleanest long-term shape
  and the right call once graduated/radial filters arrive, but it changes `build_pipeline`'s
  signature a third time and churns every call site for a capability only the zoom path uses.

## 4. Cache: two entries

Fit and Native are different `max_dim`s and so different keys. With today's single entry, every
zoom in or out would recompute the prefix (~123 ms with NR on). Two entries — one per mode —
make mode switches instant.

Memory: ~20 MB (fit) + ~78 MB (native).

**The cache key does not gain the region**, and that is worth stating because it is the point
of putting the crop where it is: the prefix always runs whole-buffer, so panning changes only
the tail's input and never invalidates the cache. `CacheKey` stays `(PrefixKey, max_dim)`.
Panning across an entire photo re-runs nothing but the tail.

## 5. App wiring

- `Session` tracks the currently rendered region so the canvas can place the texture.
- The canvas computes the needed region from `pan`/`zoom`/viewport, requests a render when it
  escapes the rendered rect, and draws the region texture into its corresponding screen rect.
- Crop mode continues to reset the crop to the full frame; the crop-overlay maths must be
  checked against the region rather than assuming a whole-image texture.

## 6. Testing

**The master invariant** — one property validates the region maths, the vignette correction and
the cache at once:

```
render_region(buffer, edits, region) == crop(render_whole(buffer, edits), region)
```

Bit-identical, across edit sets that include a vignette (the stage that would break) and a
geometry angle (which changes the post-geometry dimensions the region indexes into).

**`pipeline`** — `VignetteStage` with a `Region` equals the whole-image vignette cropped to it;
`build_pipeline_split_in_region(.., None)` is identical to `build_pipeline_split`.

**`app`** — the viewport→region mapping is a pure function and tested directly (centre, corners,
clamping at image edges, margin applied, margin clipped at edges); a needed rect inside the
rendered rect triggers **no** re-render, one outside it does; mode selection flips at
`fit · zoom > 1`.

**Perf** — a `bench_preview` case for a region tail render; expect ~10 ms, versus ~123 ms for
the one-off native prefix.

## 7. Accepted caveats

1. **Buffer-native is half of true full-res.** Detail judged here corresponds to ~2× the radius
   it will have in the export. Much better than "invisible", not exact.
2. **The buffer's demosaic differs from the export's.** The half-size decode bins 2×2 and skips
   demosaic entirely, while export runs AHD — so a 1:1 view differs from the export in *kind*,
   not only resolution. Independent of this feature, but it bounds how far 1:1 judgement can be
   trusted.
3. ~78 MB extra while zoomed.
4. The before/after baseline stays at preview resolution, so toggling it while zoomed shows a
   softer image.
5. Native mode renders slightly more than is displayed between 0.51× and 1.0× on-screen scale.

## Done when

- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --all` green.
- All 20 golden references byte-identical — no existing render path changes behaviour.
- The master invariant holds bit-identically.
- Zooming past fit shows genuinely more detail; panning is smooth; zoom in/out does not
  recompute the prefix.
- `CLAUDE.md` records the two modes, the optional `Region`, the two-entry cache, the demosaic
  measurements, and that the `wgpu` port would not address them.
