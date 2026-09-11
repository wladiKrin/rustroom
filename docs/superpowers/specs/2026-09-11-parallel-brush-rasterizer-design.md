# M8.3i — the parallel brush rasterizer

**Status:** design approved 2026-09-11. Branch `m8.3i-mask-loop`.

> ⚠⚠ **THE NUMBERING SHIFTED AGAIN — the fifth time in Phase 8.** This milestone was scoped as
> "the mask loop on the GPU". The decomposition below moved that work out of it, so **M8.3i is the
> parallel brush rasterizer and the GPU mask loop becomes M8.3j.** Any text saying *"M8.3i — the
> mask loop"* is stale.

## 1. What the measurement found, and why it overturned the scope

M8.3i was scoped on the belief that a mask's cost is its sub-stages, and that porting them to the
GPU was the lever. `CLAUDE.md`'s own standing rule — **measure before theorising** — was applied
first, by instrumenting `MaskStage::apply` and `brush::raster::rasterize_in` with phase timers.

**Per mask, rung 3200, 3123×2085 (6.5 Mpx), Apple M2, release, best of 5:**

| mask | coverage | clone | sub-stages | blend | total |
|---|---|---|---|---|---|
| exposure, gradient | 0.00 | 2.06 | 3.8 | **11.1** | 17.0 |
| **HSL, gradient** | 0.00 | 2.06 | **68.8** | 11.1 | 82.0 |
| exposure+contrast+HSL, gradient | 0.00 | 2.1 | 67.5 | 11.5 | 81.2 |
| exposure, small radial | 0.00 | 2.2 | 4.0 | 15.3 | 21.5 |
| exposure, colour range | 0.00 | 2.3 | 4.0 | **61–120** | 67–128 |
| exposure, brush (plain, uncached) | 26.6 | 2.3 | 4.0 | 9.3 | 42.4 |
| exposure, auto-masked brush (synthetic) | 113 | 2.1 | 4.0 | 9.3 | 128 |
| **the catalogue's one real mask** | **228** | 2.1 | **3.8** | 9.7 | **236** |

**There is no dominant phase.** Which one dominates depends entirely on the mask kind and what it
carries: HSL → sub-stages (84%); colour range → the blend (91%); auto-masked brush → coverage
(97%); exposure under a gradient → the blend (65%).

Three consequences:

1. **The clone is never above 12% and is ~2 ms flat.** The scope note's worry about needing a
   full-RGB scratch copy on the GPU is a non-issue — it is not where the time goes.
2. **⚠⚠ On the only masked photo this catalogue has, the sub-stages are 3.8 ms of a 296.8 ms
   frame — 1.3%.** Porting them buys essentially nothing there.
3. **The dominant cost is brush coverage rasterisation**, and it is entirely on the CPU.

### 1.1 Three corrections to what was believed going in

- **⚠⚠ `CLAUDE.md`'s record of that sidecar is wrong.** It says the mask carries noise_reduction,
  exposure, sharpening and grading. Read directly from `DSCF6274.RAF.json`, **every field is
  neutral except `exposure: -3.6`**. NR is `luminance: 0, color: 0` (detail 50 / smoothness 50 are
  the *defaults*, and `is_active` is amount-only); sharpening amount 0; all four grade wheels 0;
  all eight HSL bands 0. **Correct that note when this milestone's record is written.**
- **That brush is 36 strokes and 1,387 points, 22 of the strokes auto-masked.** `BrushCache`
  declines any brush whose `needs_base()` is true, so it is rasterised from scratch on every
  render that misses the auto-mask coverage cache.
- **The rasterizer is the last fully sequential hot loop in the project** — `grep -c rayon`
  returns **0** for `brush/raster.rs`, where `map_pixels_xy`, `downscale_to`, the 8-bit pack and
  `box_blur`'s horizontal pass are all parallel.

### 1.2 The coverage build, decomposed

Real sidecar, whole-frame rasterisation, box 1510×2019 (3.05 Mpx, 47% of the frame), 36 strokes,
**3,645 dabs**:

| phase | ms |
|---|---|
| `base_lab` — one `prophoto_to_oklab` per pixel of the box | **55** |
| the per-stroke dab loop | **165** |
| total coverage | **~222** |

At radius ≈ 68 px a dab covers ~14,500 pixels, so 3,645 dabs is roughly **53 M pixel-writes**,
each a `sqrt`, a `dab_alpha` and possibly a gate. 165 ms for that is about 3 ns per write — an
ordinary rate for sequential scalar work. **This is not a structural bug; it is a large amount of
genuine work done on one core.**

### 1.3 What this does and does not fix, stated before the design

The auto-mask coverage cache (2026-09-09) already makes coverage free **on a hit** — its key is
`Edits` truncated to the masks before this one, plus the rung, so moving *this mask's own*
exposure slider is a hit. It structurally cannot help a **global** slider (which changes the base
the gate reads) or a **pan** at a native rung (where whole-frame coverage for a gating brush is
not merely uncached but unknowable). Those are the cases that pay the 236 ms, and they are what
this milestone targets.

## 2. Scope

`crates/pipeline/src/brush/raster.rs`, function `rasterize_in`, two phases. Nothing else.

**Explicitly out of scope:** the sub-stages (M8.3j), the blend (already rayon-parallel through
`map_pixels_xy` — a colour range's 61–120 ms is parallel and simply expensive), `MaskStage`'s
clone, and any change to `CoverageMap`'s shape or to what coverage *is*.

## 3. Change 1 — the Oklab conversion goes parallel

`base_lab` is a pure per-pixel map over the box: `prophoto_to_oklab(base.at(x, y))` for each pixel,
pushed in row-major order. It becomes a preallocated `Vec` filled through `par_chunks_exact_mut`
over rows — the shape `map_pixels_xy` and the 8-bit pack already use.

Bit-identical by construction: each output element is a pure function of one input pixel, and the
element order is unchanged.

⚠ `BasePixels::at` takes `&self` and `BaseView` holds `&ImageBuf`, so the trait object must be
`Sync` for the parallel fill. `&dyn BasePixels` becomes `&(dyn BasePixels + Sync)`.

## 4. Change 2 — the dab loop becomes a per-row gather

### 4.1 What it does today

Per stroke, three passes over the stroke's sub-rect:

1. clear the sub-rect of a reused `layer` buffer;
2. for each dab, for each pixel of the dab's box, `layer[i] = max(layer[i], gated_alpha)`;
3. for each pixel of the sub-rect, `cov[i] = op.combine(cov[i], layer[i] * flow)`.

Pass 2 is a **scatter**: dabs overlap, so two dabs write the same pixel and it cannot be
parallelised over dabs.

### 4.2 What it becomes

Per stroke, **one** pass, parallel over **rows of the stroke's sub-rect**. Each row:

- walks the dabs that touch it, **in stroke order**, maintaining a running per-pixel max in a
  small row-local scratch;
- composites that row straight into its own row of `cov`.

`layer` and the clear pass disappear; three passes become one.

### 4.3 Why it is bit-identical

- **`max` is exact and order-independent** — no float reassociation, which a sum would have.
- Each `cov` row is written by exactly one thread within a stroke, and strokes stay sequential,
  so the cross-stroke `MaskOp::combine` order is unchanged.
- The composite arithmetic per pixel is untouched.

### 4.4 ⚠⚠ The short-circuit is load-bearing and a naive rewrite destroys it

The inner loop today reads:

```rust
let slot = &mut layer[..];
if a <= *slot { continue; }          // positional alpha already loses the max
let a = match (&d.gate, &base_lab) { .. };   // the gate is evaluated ONLY past that point
*slot = slot.max(a);
```

That skip is sound because **the gate can only ever reduce an alpha**, so a dab whose *positional*
alpha already loses the running max cannot change the pixel however its gate resolves. Dabs are
laid `SPACING` (a quarter radius) apart and overlap heavily, so most of them lose — which is
precisely why the gate costs 165 ms rather than several times that.

**A gather that evaluated the gate for every dab touching a pixel would be correct and
dramatically slower.** The rewrite must keep the running max and the same skip, visiting dabs in
**stroke order** so the sequence of comparisons — and therefore which dabs are skipped — is
identical. That is also what makes the result bit-identical rather than merely equal in the
absence of the skip.

### 4.5 Finding which dabs touch a row

Two candidates, and the choice is **measured, not argued**:

- **Scan**: each row tests all the stroke's dabs. 2019 rows × 3,645 dabs = 7.4 M cheap `y`-range
  tests against 53 M pixel operations.
- **Bucket**: build a per-row dab index once before the parallel pass.

⚠ The 8-bit pack's lesson applies directly — *the obvious parallel spelling measured slower than
not threading at all* — so both are tried and the numbers decide.

## 5. Change 3 — convert only the gating strokes' box

`base_lab` today spans the union box of **every** stroke, and is built whenever *any* dab gates.
Fourteen of the real brush's thirty-six strokes are not auto-masked, and their pixels are never
read. The conversion box narrows to the union of the **gating** strokes' boxes, clamped into the
overall box so the existing index arithmetic still holds.

⚠ Measured rather than assumed: if the gating union is nearly the whole box on the real sidecar,
this buys little and is kept only because it costs a few lines. The measurement goes in the record
either way.

## 6. Granularity

Row chunk size is **swept, not chosen** — `PACK_BLOCK_PX`'s precedent, where per-item rayon
measured 1.64 ms against 1.13 ms sequential. Two things need a number:

- the **chunk size** (rows per rayon task);
- a **sequential floor** below which the whole thing stays single-threaded, because the canvas
  tint rasterises on every drag frame and every test uses a tiny fixture, where dispatch overhead
  would dominate.

Both are recorded as measured constants with the sweep in their doc comments.

## 7. Testing

- **Bit-identity against a sequential reference written inline in the test**, deliberately not
  shared with production — the rule `downscale_to`'s and the 8-bit pack's parallelisations each
  followed, because a reference that calls the code under test cannot witness a change to it.
  The fixture carries: several strokes, **overlapping dabs** (so the running max matters), an
  **erase** stroke (so `MaskOp::Subtract` and stroke order matter), an **auto-masked gating**
  stroke against a real base (so the short-circuit and the gate order matter), a stroke whose
  sub-rect is **one row tall**, and a stroke clipped by the region.
- **The 63 goldens must stay byte-identical.** `mask_brush_exposure`, `mask_brush_erase` and
  `mask_brush_flow_buildup` are what prove the two-level max-then-composite rule survived.
- ⚠ **The existing `brush::raster` tests pass unchanged** — that is the evidence behaviour did not
  move, and is the acceptance criterion rather than "still green".
- **A sabotage that visits a row's dabs out of stroke order** must fail the bit-identity test; if
  it does not, the fixture lacks overlapping dabs whose gates differ and must be strengthened.
- Before/after on the real sidecar at rung 3200, through `mask_breakdown`.

## 8. Kept

- **`RUSTROOM_DEBUG_MASK`** — the four phase timers in `MaskStage::apply` (coverage / clone /
  sub-stages / blend) and the two in `rasterize_in` (`base_lab` / dab loop) — kept permanently,
  the `RUSTROOM_DEBUG_BRUSH` and `RUSTROOM_DEBUG_RENDER` precedent. This instrumentation is what
  reframed the milestone; rebuilding it by hand next time is the alternative.
- **`crates/app/examples/mask_breakdown.rs`**, including the real-sidecar row, which is the only
  thing in the repo that prices a mask as a photographer actually has one.

## 9. What this does not do, stated rather than glossed

- **Nothing for masked HSL's 68.8 ms of sub-stages** — that is M8.3j, and it is the case reported
  in use as slow.
- **Nothing for a colour range's 61–120 ms blend**, which is already rayon-parallel and simply
  expensive. The range-mask coverage cache in `docs/BACKLOG.md` is the lever there.
- **Coverage remains the dominant term on the real photo even after a 3–4×** — roughly 60 ms of a
  ~130 ms frame.
- **It does not make an auto-masked brush cacheable across a global slider.** That blind spot is
  unchanged and is a property of the gate reading base pixels.
- It is one machine, one adapter-free CPU path, one rung, one photographer's single masked photo.
