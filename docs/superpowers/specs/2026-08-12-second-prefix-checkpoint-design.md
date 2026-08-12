# Per-stage prefix checkpoints — design

Date: 2026-08-12
Status: approved

## Goal

The prefix — `downscale → geometry → noise_reduction → dehaze` — is cached as **one** unit,
keyed by `PrefixKey { geometry, noise_reduction, dehaze, scale }`. Change any of those three
and all three re-run. Dragging Dehaze, the *last* prefix stage, therefore re-runs the crop and
the denoise that did not change.

Cache an intermediate at **every** prefix stage boundary, so a change re-runs only what is
downstream of it.

## What the measurements said

X-T4 `.RAF`, 3123×2085 half-size buffer, release, best of 5. Two render modes: fit (1600 px
long edge, 21 MB per buffer) and native (3123 px, 78 MB per buffer).

| step | fit | native |
|---|---|---|
| `downscale_to` | **9.0 ms** | **2.2 ms** (already small enough — it is a clone) |
| geometry, angle 3° | **13.6 ms** | **52.2 ms** |
| geometry, crop only | 1.0 ms | 3.9 ms |
| noise reduction 50/50 | 31.4 ms | 117.4 ms |
| dehaze 60 | 32.3 ms | 115.6 ms |
| deep clone of one checkpoint | 0.5 ms | 2.1 ms |

Whole-render figures for reference (fit): any tail slider **13.7 ms**; Dehaze with NR on
**86.8 ms**; Dehaze with NR off 55.3 ms; NR both 50 52.4 ms; native prefix one-off 172 ms.

Two corrections to what `CLAUDE.md` assumed, and both redirect the design:

1. **The post-geometry checkpoint is worth far more than "the ~9 ms downscale".** That note
   reasoned about *changing* Straighten, where it is correct that no checkpoint can help —
   geometry is first, so changing it invalidates everything by definition. But the checkpoint's
   real job is the opposite direction: with an angle set, every NR or Dehaze drag currently
   re-runs a **13.6 ms / 52.2 ms** rotation. That is on par with noise reduction itself at fit
   resolution. Note the asymmetry within geometry: a crop alone is ~1 ms, so essentially all of
   this is the rotation.
2. **The post-downscale checkpoint is a bad deal at native — but it can be free instead.** It
   saves 2.2 ms for 78 MB, because `downscale_to` returns a clone when the image is already
   small enough. Since the engine already holds the editing buffer as `Arc<ImageBuf>`, the
   chain can *seed* from that Arc: zero copy, zero time, no cache entry. At fit it is a genuine
   21 MB / 9.0 ms checkpoint and is cached normally.

Expected outcome, fit resolution: Dehaze with NR on **86.8 ms → ~47 ms** (dehaze ~33 + tail
~14). NR with a straighten angle set loses the 13.6 ms rotation. Every other slider is
unchanged at 13.7 ms, since it was already served from the final checkpoint.

## Non-goals

- **Making Straighten cheap.** Impossible by construction, see above. Its only win is the seed.
- **Caching tail stages.** The tail is 13.7 ms whole; there is nothing to reclaim, and tail
  stages run on a *cropped* region during a 1:1 render, which the whole-buffer checkpoint
  scheme does not model.
- **A full-resolution editing buffer.** Separately valuable (it is what would make 1:1 honest),
  separately scoped, and it only raises the value of this change. See "Interaction with a
  full-resolution buffer" below.
- **Parallelising the RGBA8 pack.** Still the next standing perf item; not in this change.
- **Changing `build_pipeline`, the CLI, export or any stage.** This is a caching change. All 29
  goldens must stay byte-identical, and that is the acceptance criterion.

## 1. The prefix becomes a list of steps

The load-bearing risk in this feature is **silent staleness**: a prefix stage missing from a
cache key means the cache serves wrong pixels with nothing failing. Today one tripwire test
(`the_prefix_holds_geometry_noise_reduction_and_dehaze`) guards a single key. Splitting into
three keys would mean three chances to forget, which is strictly worse.

So the keys are **derived from the stage list** rather than written alongside it:

```rust
/// One prefix stage's parameters, identifying its output for caching.
///
/// ⚠ Adding a prefix stage means adding a variant here. `build_prefix_steps` returns
/// (key, stage) pairs, so a stage with no key does not compile — the cache cannot be
/// left behind by a new stage.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PrefixStepKey {
    Geometry(edit_model::Geometry),
    NoiseReduction(edit_model::NoiseReduction),
    Dehaze(edit_model::Dehaze),
}

pub struct PrefixStep {
    pub key: PrefixStepKey,
    pub stage: Box<dyn PipelineStage>,
}

/// The prefix as an ordered list of (key, stage) pairs. Inactive stages are absent, exactly
/// as they are absent from the pipeline today.
pub fn build_prefix_steps(edits: &Edits, scale: RenderScale) -> Vec<PrefixStep>;
```

`PrefixKey` becomes the cumulative key and is **defined through** `build_prefix_steps`:

```rust
#[derive(Debug, Clone, PartialEq)]
pub struct PrefixKey {
    steps: Vec<PrefixStepKey>,
    /// Still here: it sets the noise-reduction and dehaze radii.
    scale: f32,
}

impl PrefixKey {
    /// Unchanged signature. Now equal to `upto(&build_prefix_steps(..), n_all, scale)`.
    pub fn new(edits: &Edits, scale: RenderScale) -> Self;

    /// The key for the checkpoint after the first `n` prefix stages have run.
    /// `n == 0` is the post-downscale seed.
    pub fn upto(steps: &[PrefixStep], n: usize, scale: RenderScale) -> Self;
}
```

`build_pipeline_split_in_region` is redefined as `build_prefix_steps(..)` mapped to its stages,
plus the unchanged tail — the same anti-drift move `build_pipeline = prefix ++ tail` already
makes, one level down.

Two properties fall out for free:

- **An inactive stage costs nothing.** It produces no step, so the checkpoint "after" it has
  the same key as the one before it. No special-casing, no duplicate image. Chain length is
  `active prefix stages + 1`.
- **`CacheKey = (PrefixKey, max_dim)` keeps its exact current shape.**

`PrefixKey` loses `Copy` and allocates a ≤3-element `Vec` per render. Accepted: a render costs
at minimum 13.7 ms.

### Rejected alternatives

- **Hand-written per-checkpoint key structs** (`Ck0Key`/`Ck1Key`/`Ck2Key`, or
  `PrefixKey::truncated(n)` over fixed fields). Keeps `Copy` and allocates nothing, but it is
  three structs that must each be updated when a prefix stage is added, with nothing forcing
  it. Three chances to serve stale pixels where there is one today.
- **Keying by `(PrefixKey, stages_completed)`.** Wrong, not merely worse: two edit sets that
  share a geometry get different keys at checkpoint 1 even though the pixels there are
  identical, so a Dehaze change would still miss the post-NR checkpoint. It defeats the
  feature.

## 2. The render becomes a chain walk

`render_cached` replaces "get the prefix output or compute it" with:

```
steps = build_prefix_steps(edits, scale)

// Checkpoint 0 — the seed.
seed = if max_dim >= buffer_long { Arc::clone(buffer) }   // free; never cached
       else { cache.get(key.upto(0)) ?? Arc::new(buffer.downscale_to(max_dim)) }

// Resume from the deepest checkpoint we still hold.
start = the largest i in 0..=steps.len() with a cache hit at key.upto(i)   (0 = the seed)

for i in start..steps.len() {
    img = Arc::new(steps[i].stage.apply((*img).clone()));
    cache.insert(key.upto(i + 1), img);
}
```

Everything downstream is untouched: the region crop still happens on the final checkpoint's
output, the tail still runs on the crop, `pack` is unchanged. `render_once` stays pure and
uncached and remains the reference the cached path is tested against.

**Clones.** `PipelineStage::apply` consumes its input — the ownership that keeps the GPU-port
seam clean — so each step that runs clones its input checkpoint. A cold chain now pays one
clone per active stage instead of one total: at worst +1.0 ms at fit and +4.2 ms at native, on
renders that cost 87 ms and 172 ms. Against savings of 13–117 ms per avoided stage, this does
not eat the win; it is measured rather than assumed, and re-measured after implementation.

## 3. The cache becomes two tiers

Each checkpoint is a whole `ImageBuf`: 21 MB at fit, 78 MB at native. A naive chain in both
modes is ~318 MB, on top of the ~234 MB `BufferCache`. So the eviction policy is designed
first, per the standing warning.

```rust
struct PrefixCache {
    /// Complete prefix outputs. CAP 2 — one per render mode, byte for byte today's policy.
    finals: Vec<(CacheKey, Arc<ImageBuf>)>,
    /// Intermediates, byte-budgeted LRU. MRU first.
    mids: Vec<(CacheKey, Arc<ImageBuf>)>,
}

const MID_BUDGET_BYTES: usize = 256 << 20;
```

**The two tiers cannot evict each other**, and that is the point. `finals` is exactly today's
cache with today's cap, so the zoom guarantee — fit and native both stay warm, no re-render on
zoom in or out — is preserved unconditionally. A native render can never evict the fit final in
order to make room for its own intermediates.

Retention within `mids` is **deepest-first**: when the budget bites, the surviving checkpoint is
the one that saves the most upstream work. Accounting is `data().len() * 4` bytes; entries are
distinct images by construction (distinct keys differ by step count), so there is no double
counting, and the free native seed is never stored.

An entry at index `steps.len()` goes to `finals`; anything shallower goes to `mids`. When the
prefix is empty, checkpoint 0 *is* the final and goes to `finals`.

**What the budget actually does on this hardware — stated plainly, because it is less than it
sounds.** Worst case with all three stages active: fit mids are the seed plus post-geometry plus
post-NR, 3 × 21 = 63 MB; native mids are post-geometry plus post-NR (the seed is free), 2 × 78 =
156 MB. Total 219 MB, **under** the 256 MB budget. So on the X-T4 the ceiling never bites, the
full chain stays warm in both modes, and the footprint is the same ~318 MB an unbudgeted chain
would use — against ~99 MB today.

The budget therefore earns its place on two other grounds, not on saving memory here: it is a
hard ceiling on files bigger than this sensor (a 100 Mpx file's native mid is ~300 MB, so
exactly zero survive and native degrades to today's behaviour), and it makes the degradation
*designed* — deepest-first — rather than whatever eviction order happens to fall out. If ~318 MB
on an X-T4 turns out to be too much in use, the constant is the one thing to turn down.

## 4. Testing

`every_cached_frame_matches_a_cold_render` stays the master test: it pushes a sequence through
one engine and asserts every frame equals a cold `render_once`. Its sequence extends to exercise
each new checkpoint — a dehaze-only change (resumes at post-NR), an NR-only change (resumes at
post-geometry), a geometry change (resumes at the seed), and returns to earlier states — and the
`assert_ne!` guards that stop it passing trivially are kept and extended.

That test proves correctness but **not** that anything is reused; it would still pass with the
cache deleted. So it is backed by a direct measurement of the feature: a count of prefix stages
actually applied per render, exposed for tests.

| test | asserts |
|---|---|
| `every_cached_frame_matches_a_cold_render` | extended sequence, every frame bit-identical to cold |
| `a_change_reruns_only_what_is_downstream_of_it` | with all three prefix stages active: a tail change runs **0** stages, a dehaze change **1**, an NR change **2**, a geometry change **3** |
| `the_native_seed_shares_the_buffer_rather_than_copying_it` | at `max_dim >= buffer_long` the seed is the buffer's own `Arc` and no checkpoint 0 is stored |
| `a_shrinking_prefix_does_not_serve_the_wrong_checkpoint` | turning the last prefix stage off promotes its checkpoint into `finals` rather than leaving the zoom guarantee in the budgeted tier |
| `the_mid_budget_evicts_the_least_recently_used` | with a tiny budget the shallow mids go and the deepest survives |
| `the_finals_tier_is_untouched_by_mid_pressure` | mid eviction never drops a final; both render modes stay warm |
| `the_finals_tier_still_holds_exactly_two` | today's cap, unchanged |
| `the_default_budget_is_the_real_one` | a derived `Default` would give budget 0 and leave the feature inert with every correctness test still green |
| `the_prefix_holds_geometry_noise_reduction_and_dehaze` | kept — composition and order, which the compiler cannot pin |
| `the_prefix_steps_are_the_prefix_pipeline` | the step list and the built prefix cannot drift |
| `every_prefix_step_has_its_own_key` | changing one stage's params changes that step's key and no other's |
| `a_truncated_prefix_key_ignores_the_stages_below_it` | two edit sets agreeing on the first *n* stages share checkpoint *n* |
| `prefix_key_ignores_every_downstream_field` | kept, unchanged |
| `prefix_key_changes_with_prefix_fields_and_scale` | kept, unchanged |
| `a_region_render_matches_the_whole_render_cropped` | kept, unchanged — must pass untouched |
| the 29 goldens | byte-identical |

The budget needs to be reachable from tests without being a production knob, so the const is
overridable through a constructor (`PrefixCache::with_mid_budget`), matching the injection
pattern `DecodeWorker` and both preset stores already use. The stage count is a plain return
value from `render_cached` that the worker discards.

**Deepest-first needs no rule of its own.** Within a render, checkpoints are inserted shallow to
deep, so the deepest is the most recently used; plain LRU eviction therefore drops the
shallowest first, which is the order wanted. It is worth stating because it looks like an
unimplemented requirement otherwise.

## 5. Bench

`bench_preview` gains the cases that name the win, so the table in `CLAUDE.md` can be
reproduced: a Dehaze drag with NR warm, an NR drag with a straighten angle warm, and a tail
slider for the unchanged baseline.

## Interaction with a full-resolution editing buffer

The roadmap's background full-res decode would make the prefix ~4× more expensive (NR ~470 ms,
dehaze ~460 ms at 26 Mpx), so this change gets *more* valuable, not less — the two stack.

One number would need revisiting: a full-resolution checkpoint is ~312 MB, so under a 256 MB
budget none would survive at 1:1 and native would fall back to today's behaviour. The structure
is right either way; only the constant is tuned. Recorded here so it is a decision rather than a
surprise.

## Accepted caveats

1. **~318 MB of prefix cache on this hardware, against ~99 MB today.** Bounded and deliberate,
   but it is real memory, and on this sensor the 256 MB budget does not reduce it — see §3.
2. **Straighten still costs full price** when you drag *it*. Structural, not fixable.
3. **A crop-only geometry checkpoint buys ~1 ms** and costs a whole buffer. It is cached anyway,
   because keying on "is the geometry expensive" would make the cache's contents depend on a
   heuristic about cost rather than on correctness.
4. **More clones per cold render** (+1.0 ms fit, +4.2 ms native), the price of the ownership
   model that keeps the GPU seam clean.
5. **`PrefixKey` is no longer `Copy`** and allocates per render.
6. **Nothing here helps the first render after opening a photo.** The cache is cold on open;
   this speeds up *editing*.
