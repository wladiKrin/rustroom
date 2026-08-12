# A parallel 8-bit pack — design

Date: 2026-08-12
Status: approved

## Goal

`render_engine::pack` turns the pipeline's `f32` sRGB output into the RGBA8 bytes the canvas
uploads as a texture. It is a sequential `Vec::push` loop and, since the per-stage prefix
checkpoints landed, it is the largest single slice of a cached preview frame — **4.6 ms of
14.4**, on every frame including the ones that are now cheap.

Make it fast, and put the primitive where the buffer and the parallel machinery already live.

## What the measurements said — and they redirected the design

X-T4 `.RAF`, release, best of 9, 8 threads available.

| | fit (1600×1068, 1.7 Mpx) | native (3123×2085, 6.5 Mpx) |
|---|---|---|
| `pack` → RGBA8, today's `Vec::push` loop | **4.59 ms** | **17.81 ms** |
| `export::to_rgb8`, `.map().collect()` | **0.91 ms** | **3.41 ms** |
| `vec![0u8; n]` alone | 0.09 ms | 0.31 ms |

**⚠ The headline finding: this is not primarily a parallelism problem.** `to_rgb8` produces
**three quarters** as many output bytes as `pack` on the same buffer and does it in **a fifth**
of the time — 0.18 ns per output byte against 0.67. Per byte the pack is **3.7× more
expensive**, and no thread is involved in that gap. The cause is `Vec::push` in a loop, which
carries a capacity check per byte and does not vectorise, against a `collect` over an
exact-size iterator that allocates once.

So the fix is two independent multipliers, and the cheaper one comes first:

1. **Drop `Vec::push`** — preallocate and fill. Expected ~4.6 → ~1.2 ms at fit, from the
   `to_rgb8` rate scaled by 4/3. This is most of the win and it is not what the task was named
   after.
2. **Parallelise the fill** — expected ~1.2 → ~0.3 ms on 8 threads.

Both are worth having; the point of separating them is that the attribution gets **measured**
rather than assumed. The plan benches the intermediate state so the record says which multiplier
delivered what.

**`vec![0u8; n]` costs 2% of the current total**, so the preallocate-then-overwrite shape is
affordable and there is no case for `MaybeUninit` or unsafe. Measured rather than argued,
because "zeroing 6.8 MB before overwriting it is wasteful" is exactly the kind of true-sounding
claim that turns out not to matter.

## Non-goals

- **Changing what the bytes are.** Output must be bit-identical. This is a performance change
  and the 29 goldens must not move.
- **The GPU upload itself.** `egui`'s texture path is downstream and untouched.
- **Reworking the golden harness's own quantiser.** See §3.
- **Speeding up export encoding.** `export::to_rgb8` gets faster as a side effect, but on a
  ~13 s export its ~13 ms is noise; that is a bonus, not a justification.

## 1. The primitive moves to `core-image`

The same "clamp to 0..1, ×255, round" quantisation exists **three times** in the tree today:
`export::to_rgb8`, a private copy in `crates/pipeline/tests/golden.rs`, and inline in
`render_engine::pack`. Two of those are production code.

`ImageBuf` gains both conversions:

```rust
impl ImageBuf {
    /// Quantize to 8-bit RGB: clamp to 0..=1, scale by 255, round.
    pub fn to_rgb8(&self) -> Vec<u8>;

    /// As `to_rgb8`, interleaving an opaque alpha byte — the layout a GPU texture wants.
    pub fn to_rgba8(&self) -> Vec<u8>;
}
```

`core-image` is the right home on three counts: it owns `ImageBuf`, it already depends on
`rayon` (`map_pixels`, `map_pixels_xy`), and it already knows about 8-bit RGBA because
`Thumbnail` holds one. **`app` gains no `rayon` dependency**, which keeps intact the deliberate
separation recorded for `ThumbLoader` — that the app's own worker pool stays distinct from the
render pool.

`export::to_rgb8` **keeps its name, signature and tests** and delegates to
`image.to_rgb8()`. No call site changes, and its existing `to_rgb8_quantizes_and_clamps` is
then the guard that the move preserved behaviour exactly.

Rejected: **adding `rayon` to `app` and rewriting `pack` in place.** Smallest diff, but it puts
a dependency in the crate that was deliberately kept off the global pool, and leaves two
production copies of the quantisation side by side — the drift risk that pulled `safe_name` out
of `presets` and `guided_filter` out of `noise_reduction`.

## 2. Implementation shape

```rust
pub fn to_rgba8(&self) -> Vec<u8> {
    let mut out = vec![0u8; self.width * self.height * 4];
    out.par_chunks_exact_mut(4)
        .zip(self.data.par_chunks_exact(Self::CHANNELS))
        .for_each(|(dst, src)| {
            dst[0] = quantize(src[0]);
            dst[1] = quantize(src[1]);
            dst[2] = quantize(src[2]);
            dst[3] = 255;
        });
    out
}
```

Chunk sizes are non-zero constants, so unlike `map_pixels_xy` — where a zero-width image makes
`par_chunks_exact_mut(0)` panic — there is **no zero-size special case**. An empty image yields
an empty `Vec` and the parallel iterator does nothing. Tested anyway, because that reasoning is
worth pinning rather than trusting.

`quantize` is one shared private function, so the two public conversions cannot disagree about
rounding.

## 3. The golden harness keeps its own copy — deliberately

`crates/pipeline/tests/golden.rs` has a private `to_rgb8`. It stays.

A golden test that quantises through the production quantiser cannot independently witness a
change to the production quantiser. Keeping the test's own reference implementation separate is
what lets the checked-in PNGs act as a check on this change rather than moving with it. The
cost — a third copy — is confined to a test, where drift shows up as a visible failure rather
than as wrong pixels.

## 4. Testing

**The existing tests cannot prove the thing that matters.** `every_cached_frame_matches_a_cold_render`
compares two frames that would *both* go through the new pack, so it would pass just as happily
if the new one were wrong. Bit-identity needs a test that holds the old behaviour still.

| test | asserts |
|---|---|
| `to_rgba8_matches_a_sequential_reference` | byte for byte against a plain sequential loop, on a varied image including out-of-range and negative samples |
| `to_rgb8_matches_a_sequential_reference` | the same for the three-channel form |
| `to_rgba8_is_to_rgb8_with_an_opaque_alpha` | the two conversions agree on colour, so the shared `quantize` really is shared |
| `the_conversions_handle_an_empty_image` | no panic, empty output — the `par_chunks_exact_mut(0)` trap that does *not* apply here |
| `to_rgb8_quantizes_and_clamps` (existing, `export`) | unchanged, and now the guard that delegation preserved behaviour |
| the 29 goldens | byte-identical |

The reference loops are written inline in the tests rather than shared with production, for the
same reason §3 gives.

## 5. Bench

`bench_preview` gains a pack line, reporting fit and native. The measurement is taken at three
points so the attribution is on the record: today's `Vec::push` loop, the sequential
preallocate-and-fill, and the parallel version.

## Accepted caveats

1. **A third copy of the quantisation still exists**, in the golden harness. Deliberate, §3.
2. **`vec![0u8; n]` writes every byte twice** — 0.09 ms at fit, measured and accepted rather
   than fixed with unsafe.
3. **The pack now competes for the global rayon pool** with the pipeline stages. It runs on the
   render worker where those stages already run, so this adds no new contention with the UI or
   with `ThumbLoader`'s separate pool.
4. **Gains shrink at small sizes**, where rayon's per-call overhead is a larger fraction. The
   preview and native buffers are both megapixels, so this only affects tests.
