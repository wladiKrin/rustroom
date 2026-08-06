# Preview prefix cache — design

Date: 2026-08-06
Status: approved

## Goal

Make editing feel fast. Today every slider move re-renders from the full editing buffer: it
re-downscales to preview size and re-runs every stage, including ones whose parameters did not
change. With noise reduction enabled — and NR runs *first* — moving the Exposure slider re-runs
~29 ms of denoising for nothing.

Cache one intermediate image at a fixed point in the pipeline, keyed by exactly the parameters
that point depends on.

## What the measurements actually said

Measured on the X-T4 `.RAF` (3123×2085 half-size buffer → 1600 px preview, release, best of 5)
with `cargo run --release -p app --example bench_preview`. The neutral 21.1 ms render breaks
down as:

| step | ms |
|---|---|
| `downscale_to` | **9.0** |
| pipeline, output transform only | 7.5 |
| RGBA8 pack | 4.4 |

Two corrections to prior assumptions came out of this, and they redirected the design:

1. **`CLAUDE.md`'s "~14 ms" for `downscale_to` was stale.** It is 9.0 ms.
2. **The downscale is not the main waste.** It is 43% of a *neutral* render but only 18% of an
   NR-on render (50.3 ms). Caching it alone — the original scope in `CLAUDE.md` — would have
   left the dominant cost untouched.

So the checkpoint goes *after* noise reduction, not merely after the downscale, which subsumes
the downscale win rather than competing with it.

## Non-goals

- **Per-stage caching.** Each preview-sized intermediate is ~20 MB; caching all eleven is not
  warranted. One checkpoint.
- **Making crop/straighten cheap.** Geometry is upstream of the checkpoint, so Straighten stays
  full cost. A second checkpoint would fix it and is deliberately deferred.
- **Speeding up photo opening.** The cache is cold on open; this makes *editing* fast.
- Parallelizing the 4.4 ms RGBA8 pack. Worth doing later; not in this change.

## 1. Where the pipeline splits

The split must be structural rather than a second stage list that can silently drift from the
first, so `build_pipeline` is redefined in terms of it:

```rust
/// The pipeline in two halves. The prefix's output is what gets cached: it depends only on
/// geometry and noise reduction, so every adjustment downstream can reuse it.
pub fn build_pipeline_split(edits: &Edits, scale: RenderScale) -> (Pipeline, Pipeline);

/// Unchanged signature and behaviour for the CLI, export and goldens — now defined as
/// `prefix ++ tail`, so the two constructions cannot diverge.
pub fn build_pipeline(edits: &Edits, scale: RenderScale) -> Pipeline;
```

- **prefix**: `geometry` → `noise_reduction`
- **tail**: white balance → exposure → contrast → tone regions → tone curve → saturation →
  hsl → vignette → output transform

`Pipeline` gains `into_stages(self) -> Vec<Box<dyn PipelineStage>>` so `build_pipeline` can
concatenate the two halves.

## 2. The key

Co-located with `build_pipeline_split` in the `pipeline` crate, because the whole risk is these
two drifting apart:

```rust
/// Everything the pipeline prefix depends on. If you add a stage to the prefix, add its
/// parameters here — the tests in this module fail until you do.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PrefixKey {
    geometry: Geometry,
    noise_reduction: NoiseReduction,
    scale: f32,
}

impl PrefixKey {
    pub fn new(edits: &Edits, scale: RenderScale) -> Self;
}
```

Compared with `==`, not hashed — no collision risk. `Geometry` and `NoiseReduction` are already
`Copy + PartialEq`. `scale` is included because it changes the noise-reduction radii. An `f32`
in a `PartialEq` struct is fine here: the only pathological value is `NaN`, which compares
unequal to itself and so fails *toward* a cache miss.

The app wraps it with the working resolution:

```rust
// render_engine.rs
type CacheKey = (PrefixKey, usize);   // (prefix params, max_dim)
```

`max_dim` is strictly redundant today — the buffer is fixed per `RenderEngine`, so `scale` is a
pure function of `max_dim` — but stating it costs nothing and removes the need to reason about
that coupling.

### Guarding against silent staleness

The dangerous failure is not a crash; it is someone later inserting a stage into the prefix,
forgetting the key, and the cache quietly serving stale pixels. Two tests stand in the way:

1. **Prefix composition** — for an `Edits` with *every* field non-neutral, the prefix's
   `stage_names()` is exactly `["geometry", "noise_reduction"]` and the tail contains neither.
   Inserting a stage into the prefix region fails this immediately.
2. **Key completeness** — for each downstream field (`white_balance`, `exposure`, `contrast`,
   `highlights`, `shadows`, `whites`, `blacks`, `tone_curve`, `saturation`, `vibrance`, `hsl`,
   `vignette`), mutating it leaves `PrefixKey` **equal**; mutating `geometry` or
   `noise_reduction` makes it **differ**. Every field is therefore provably either in the key
   or downstream of the checkpoint.

## 3. Where the cache lives

In the `RenderEngine` worker thread. It is the only thing that renders, and it already owns its
state, so there is no sharing and no locking:

```rust
// worker loop, after coalescing
let key = (PrefixKey::new(&req.edits, scale), req.max_dim);
let (prefix, tail) = build_pipeline_split(&req.edits, scale);
let mid: Arc<ImageBuf> = match &cache {
    Some((k, img)) if *k == key => img.clone(),        // Arc clone, free
    _ => {
        let pre = buffer.downscale_to(req.max_dim);
        let img = Arc::new(prefix.run(pre));
        cache = Some((key, img.clone()));
        img
    }
};
let out = tail.run((*mid).clone());
```

Building the stage objects each frame is a handful of `Box` allocations and is not worth
caching; only the *image* is.

### The scale has to be computed without downscaling

`render_once` currently derives the render scale from the *downscaled* image's dimensions:
`render_scale(buffer_long, preview.width().max(preview.height()), source_scale)`. That does not
survive the cache — on a hit there is no `preview` to measure, yet `scale` is still needed, both
for `PrefixKey` and to build the tail.

It must therefore be derived from `max_dim` directly. `downscale_to(max_dim)` never upscales and
caps the long edge, so:

```rust
let rendered_long = buffer_long.min(max_dim);
```

is exact — when scaling down, `dw = round(width · max_dim / long)` is exactly `max_dim` for the
long edge; when the image is already small enough, `downscale_to` returns a clone. A test pins
this against `downscale_to`'s actual output for both cases, because the whole cache keys off it.

**The deep clone is unavoidable.** `PipelineStage::apply(ImageBuf) -> ImageBuf` consumes its
input — that ownership is what keeps the future GPU-port seam clean — so a cached image has to
be copied before the tail can consume it. **Measured at 0.6 ms** against the 9.0 ms it saves,
so it stays rather than reshaping the trait.

`render_once` keeps its current pure, uncached signature. `decode_worker`'s one-off neutral
baseline render keeps calling it unchanged.

The cache holds one entry, replaced on miss. A new photo builds a new `RenderEngine`, so the
cache is naturally per-session with no invalidation logic of its own.

## 4. Expected effect

| scenario | now | cached |
|---|---|---|
| neutral, any adjustment slider | 21.1 ms | **~12 ms** |
| NR on, moving an adjustment slider | 50.3 ms | **~13 ms** |
| moving an NR slider, or Straighten | 50.3 ms | 50.3 ms — a correct miss |
| dragging the crop rectangle | 21.1 ms | ~12 ms |

That last row is not an accident worth glossing over: in crop mode `preview_edits` resets the
crop to the full frame, so dragging the rectangle never changes the geometry the pipeline sees
— only the overlay moves. Crop dragging therefore hits the cache every frame.

Memory: one extra ~20 MB preview-sized buffer per session, against the 78 MB decode buffers
already held three deep by `BufferCache`.

## 5. Testing

**`pipeline` crate**

1. `build_pipeline` equals `prefix ++ tail` by `stage_names()`, across a spread of edit sets
   including neutral, all-fields-set, and prefix-only.
2. Running `prefix` then `tail` gives **bit-identical** output to running `build_pipeline`, on
   a real chart — the numerical counterpart to test 1.
3. The two staleness guards from §2.

**`app` crate**

4. **A cache hit is bit-identical to a cold render.** The strongest formulation, and the one
   that would catch a stale cache: push a *sequence* of edit sets through one `RenderEngine`,
   waiting for each frame, and assert every frame equals `render_once` computed cold for that
   same edit set. The sequence deliberately covers: same edits twice (pure hit), a downstream
   change (hit), an NR change (miss), back to the first edits (miss, since one entry), and a
   geometry change (miss).
5. **The predicted rendered long edge matches `downscale_to`** for both the scaling-down and
   the already-small-enough cases (§3), since the cache keys off that prediction.
6. The existing `RenderEngine` tests keep passing unchanged.

**Perf** — re-run `bench_preview` and record the cached numbers in `CLAUDE.md`. The example
renders through `render_once`, which stays uncached, so a new case is added that drives a
`RenderEngine` through repeated requests to measure the hit path.

## 6. Accepted caveats

1. **Cold on open** — the first render of a photo is exactly as slow as today.
2. **Straighten stays full cost**, since geometry is upstream of the checkpoint. Crop does not,
   for the reason in §4.
3. **One entry, no LRU.** Alternating between two heavy NR settings misses every time. Real
   editing moves one slider at a time, so a second entry is not worth the memory.
4. **The 0.6 ms clone is paid on every frame**, hit or miss — the cost of the consuming stage
   trait, and 15× cheaper than what it saves.
5. The RGBA8 pack (4.4 ms) remains sequential, so it becomes a larger share of a cached frame
   (~35%). That makes it the obvious next target, not a problem with this change.

## Done when

- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --all` green.
- All 20 golden references byte-identical — `build_pipeline`'s behaviour must not change.
- A cache hit is proven bit-identical to a cold render by test 4.
- `CLAUDE.md` updated: the split, the key and its staleness guards, the corrected 9.0 ms
  `downscale_to` figure, and the measured cached timings.
