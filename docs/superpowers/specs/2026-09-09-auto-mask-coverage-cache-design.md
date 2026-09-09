# A coverage cache for the auto-mask brush

**Date:** 2026-09-09
**Branch:** `m7.6b-auto-mask-coverage-cache`
**Status:** design, approved

## 1. What this is

`BrushCache` deliberately declines a brush whose strokes gate on the image
(`Mask::Brush(b) if b.needs_base() => None`), so an auto-masked brush's coverage is re-rasterised
from scratch on **every render**. This widens the key with an identity of the base buffer, exactly
as that code's own note proposes — *"the answer is to widen the key with a cheap identity of the
base buffer, never to reintroduce the unkeyed map."*

## 2. The measurement, and what it does and does not promise

⚠⚠ **Measured before designing**, because `docs/BACKLOG.md` records a confidently-stated cost
hypothesis in this exact area that measurement overturned by 6×.

`bench_auto_mask_cov` (kept), X-T4 `DSCF6223.RAF`, buffer 6246×4170, rung 6246, region 1465×1053
(1.54 Mpx — the canvas's own margin included), best of 5:

| `rasterize_in` | clipped to the region | whole frame |
|---|---|---|
| plain brush | 12.32 ms | 21.62 ms |
| **auto-mask brush** | **42.55 ms** | **80.81 ms** |

**The gate costs 30.24 ms on top of the rasterisation — 3.5×.** Against a region tail measured at
~23.5 ms for the same region, **coverage is about 65% of a masked render at 1:1**. That matches
what `RUSTROOM_DEBUG_RENDER` shows in the running app: a 1.7 Mpx region costs ~21 ms with no mask
and **19–156 ms with one**.

⚠⚠ **What it promises is narrower than it first appears, and an earlier framing of mine overstated
it.** A **global** slider drag genuinely changes the base — the masks loop runs after the entire
global tail — so its coverage must be rebuilt and no cache can help. `docs/BACKLOG.md` already
records this as a correction to an earlier claim: *"cacheable across a drag of the mask's own
sliders; invalidated every frame by a global slider."*

**What it does buy:**

- **Dragging a mask's own adjustment sliders at 1:1** — the case a mask exists for. Mask *i*'s base
  is the output after masks *0..i-1*, so changing mask *i*'s own exposure does not change its base:
  42.55 ms per frame becomes zero.
- Any repeat render at an unchanged region and unchanged upstream state.

**What it does not buy:**

- **Panning.** A pan changes the region, and a changed region needs new coverage regardless.
- **Global slider drags**, as above.

## 3. Why the region stays in the key — a structural limit, not an omission

⚠⚠ **Whole-frame coverage for a gating brush is not merely uncached, it is unknowable during a
region render.** The gate compares each pixel's colour against the reference under its dab's
centre, so it reads base pixels across the whole stroke; a region render computes only the region,
so the base outside it does not exist. Building whole-frame coverage would mean rendering the whole
frame's tail purely to feed the gate — 26 Mpx against a region's 1.5.

This is provable rather than empirical, in the same way `docs/BACKLOG.md`'s `chroma ≤ K · spread`
argument is: it follows from what the pipeline computes, not from a measurement that might change.

⚠ It is also why the whole-frame column above is recorded and then **not used**. It is the number a
region-free cache would need, and it is 1.9× the clipped cost for a build that cannot be performed.

## 4. The design

### 4.0 ⚠⚠ CORRECTED AT PLAN TIME: the cache cannot BUILD a gating brush's coverage

**This section replaces an assumption §4 was written on.** `BrushCache` lives in `render_engine`
and is handed `(edits, fw, fh, clip)` — **it has no base buffer**. A gating brush's coverage is
built inside `MaskStage::apply` (`stages/mask.rs:265`), the only place that holds the image, and
`render_cached` cannot interpose there: the masks loop is inside the tail it runs as one unit.

So the cache can never build this coverage. It can only **store one the stage built**.

**The channel:** `BrushCoverage` — already a positional `Vec<Option<Arc<CoverageMap>>>` passed into
the tail — gains a write side, `built: Vec<Mutex<Option<Arc<CoverageMap>>>>`, and a `deposit(i, map)`.
`MaskStage` writes what it rasterised; `render_cached` reads the deposits after the run and stores
them under the key below.

⚠ **The lookup stays in `app`, where the key lives.** The stage does no lookup and holds no cache:
it receives a `prebuilt` map when one is available — through the parameter that **already exists**
— and otherwise builds and reports. That keeps stages free of cache logic and keeps "what
identifies a base" in one place, beside `PrefixKey`.

⚠ `Mutex` rather than `RefCell` because `BrushCoverage` crosses into stages that must stay `Sync`.
It is uncontended: `apply` runs sequentially, once per mask per tail.

⚠ Two alternatives were rejected. **A `CoverageStore` trait handed to `MaskStage`** puts cache
lookup inside a stage and makes `pipeline` carry a trait it needs for nothing else. **Splitting the
tail at every mask boundary**, as `build_pipeline_split_tapped` does for the grade view, keeps
stages pure but needs one tap per mask and multiplies with the proof path's second tail.

### 4.1 Per-mask entries, not one entry per render

⚠⚠ **This is the restructure the win depends on.** `BrushCache` currently stores one
`Arc<BrushCoverage>` per key, holding every mask's map together. With a single key, changing mask
0's adjustment would invalidate the whole entry — **including mask 0's own coverage**, which is
exactly the case this exists to make free. Entries become per mask, and `BrushCoverage` is
assembled from per-mask lookups.

### 4.2 The base identity

For a **gating** brush at index `i`, the key gains:

```
base_id(i) = (edits with `masks` truncated to `..i`, max_dim)
```

⚠ **Sound because everything upstream of mask `i`'s base is in it**: the prefix stages (geometry,
lens correction, noise reduction, dehaze all live in `Edits`), the entire global tail, and every
earlier mask's shape *and* adjustments. `max_dim` is there because the base is a different image at
a different rung.

⚠ **And the truncation is what makes the win possible**: dropping mask `i` drops its own
adjustments with it, so moving that mask's exposure leaves `base_id` unchanged. Masks *after* `i`
are dropped too, correctly — they cannot affect a base computed before them.

⚠ **A non-gating brush keeps its present key and gains no `base_id`.** Its coverage is a pure
function of `(strokes, geometry, frame dims, region)`, so adding a base identity would make it
invalidate on edits that never mattered to it — a regression dressed as consistency.

### 4.3 Failure direction

⚠⚠ **The key fails toward a miss**, `PrefixKey`'s rule. Anything not modelled makes the key differ
and the coverage rebuild. The dangerous direction is a **stale hit** — coverage rendered
positionally while the canvas tint shows it stopping at an edge, which the declining note calls
*"a silent divergence between preview and export, the failure mode this project has recorded more
than any other"*. The truncation makes that unrepresentable rather than unlikely: a change anywhere
upstream is a change to the truncated `Edits`.

### 4.4 Capacity

`CAP` becomes `2 * masks.len().max(1)` rather than a flat 2, recomputed per build. ⚠ The reason for
2 is unchanged and is written at the constant: two rungs are live at once, so a single entry per
mask would be evicted on every zoom change and rebuilt coming back. Per mask, the same property
needs two entries per mask.

⚠ Memory: a `CoverageMap` is 8-bit and **bbox-bounded**, so a stroke covering a quarter of a
1.5 Mpx region is ~390 KB. Four masks at two rungs is a few megabytes against `PrefixCache`'s
512 MB budget.

## 5. Testing

⚠⚠ **All 60 goldens must stay byte-identical.** This changes when coverage is computed, never what
it is.

- **The load-bearing pair, and neither half alone means anything.** Changing a **global** slider
  must **miss**; changing the **mask's own** adjustment must **hit**. A test of the first alone
  passes against a cache that never hits; a test of the second alone passes against one that never
  misses.
- **A stale hit must show as wrong pixels, not a wrong count.**
  `every_cached_frame_matches_a_cold_render`'s sequence gains an auto-masked brush, so a key that
  admits a stale base fails on the image rather than on a counter.
- **The existing four `BrushCache` tests must pass unchanged** — `the_brush_cache_hits_and_keeps_
  both_zoom_levels`, `changing_a_stroke_misses_the_brush_cache`, `the_brush_cache_misses_when_the_
  region_moves`, `the_brush_cache_still_hits_when_only_a_slider_moves`. They describe a plain
  brush's behaviour, which this must not move; their passing unchanged is the evidence it did not.
- **`the_brush_cache_declines_a_brush_that_needs_the_image` is inverted, not deleted**, and its
  replacement says what it used to assert. An inversion that changes only assertions leaves a name
  and a comment that lie — recorded when the spot-removal render rule was inverted.
- Every new test is sabotage-verified, and a sabotage is believed only after a `test result:` line
  appears.

## 6. Accepted caveats

1. **Panning is not helped**, for the structural reason in §3. It is the case that prompted this,
   and saying so plainly is better than implying otherwise.
2. **A global slider drag is not helped**, and cannot be by any cache.
3. **The key over-invalidates**: a change to a *later* mask leaves `base_id` alone, correctly, but a
   change to any earlier mask's adjustments invalidates every mask after it — sound, and the
   cheapest sound rule available.
4. **Comparing the key costs an `Edits` clone and comparison per mask per render.** `Edits` is
   already cloned into every request, so this is not a new order of cost; if it ever shows up, the
   answer is a hash of the truncated `Edits`, not a looser key.
5. **The whole-frame numbers are recorded and unused**, deliberately — they are what a region-free
   cache would need, and that cache cannot be built.
