# M8.3e — the per-pixel tail on the GPU

**Status:** design, 2026-09-11. Branch `m8.3e-per-pixel-tail`, from `main` at `22f2fc6`.

## 1. What this is

Every **per-pixel** stage in the tail becomes a `GpuStage`, so that on an ordinary photograph the
whole tail is one chain and one round trip.

⚠⚠ **THE MILESTONE THIS REPLACES WAS PROPOSED AND MEASURED WRONG, AND THE MEASUREMENT IS THE
REASON THIS SPEC EXISTS.** After M8.3d I recommended porting `display_rendering` next, on the
strength of one number: it is active on **100% of this catalogue's 40 sidecars**, where `hsl` is
active on 0%, `color_grading` on 5% and `sharpening` on 2.5%. That was the right *diagnosis* and
the wrong *prescription*, and measuring before designing is what caught it.

| at 3200×2136 (6.8 Mpx, 82 MB) | |
|---|---|
| `display_rendering` on the CPU | **22.2 ms** |
| one GPU round trip (a one-stage chain of `GpuExposureStage`) | **34.0 ms** |

**Porting it alone makes the typical photograph ~10 ms per frame SLOWER.** That is M8.3a's finding
arriving exactly where it was predicted — *the transfer is everything* — and it means stage
popularity is the wrong selection criterion on its own. **The criterion is how much CPU work a
contiguous run can put across one bridge.**

## 2. The structural fact the whole design rests on

`build_pipeline_split_in_region` pushes a stage **only when `is_active()`**, so an untouched
adjustment is not a neutral stage in the list — it is *absent*. Printed for real edits
(`probe_tail`, run 2026-09-11):

```
neutral, no rendering        -> ["output_transform"]
neutral + camera rendering   -> ["display_rendering", "output_transform"]
TYPICAL of this catalogue    -> ["exposure", "tone_regions", "display_rendering", "output_transform"]
```

⚠⚠ **There is nothing between them to split a run.** The eleven stages this catalogue never
touches are not obstacles sitting in the middle of the tail; they do not exist. So porting the
per-pixel set does not merely add a stage to an existing chain — **it makes the ordinary
photograph's entire tail one chain**, which is the case M8.3c's run-merging was built for and has
never yet been able to serve.

Measured end to end on that tail:

| | CPU | one round trip |
|---|---|---|
| whole typical tail @1600×1068 | 19.38 ms | 8.71 ms |
| whole typical tail @3200×2136 | **74.92 ms** | **34.03 ms** |

So the target is **~2×, on 100% of the catalogue**, against M8.3d's 2.05× on 7.5% of it.

⚠ Quoted honestly: eight dispatches cost a few ms of genuine compute, and the transfer floor does
not move, so expect **~1.9–2.1×**, not 2.2×. The acceptance measurement is the bench, not this
paragraph.

## 3. Scope

Eight stages. **One of them already exists**: `GpuExposureStage` was written for M8.3a as the
seam's proof and never added to `REPLACEABLE`, so exposure costs one table row and no shader.

| stage | shader | CPU @6.8 Mpx | shape |
|---|---|---|---|
| `white_balance` | new | 4.07 ms | three multipliers, computed CPU-side |
| `exposure` | **exists** | 1.21 | one multiply |
| `contrast` | new | 1.31 | lerp about a pivot |
| `tone_regions` | new | 16.78 | **1-D LUT on luminance** (see §5) |
| `tone_curve` | new | 9.30 | 1-D LUT per channel |
| `saturation` | new | 7.16 | luminance + chroma scale |
| `display_rendering` | new | 25.70 | 1-D LUT per channel + chroma scale |
| `output_transform` | new | **28.63** | 3×3 matrix + transfer curve |

⚠⚠ **`output_transform` IS THE SECOND MOST EXPENSIVE STAGE IN THE TAIL, WHICH I HAD ASSUMED WAS
TRIVIAL.** It is three `powf` per pixel — twenty million transcendentals at a native rung. It is
also present on **every** photograph, neutral ones included. Leaving it on the CPU caps the
typical win at **1.2×** instead of 2.2×, so it is not optional; and putting it in is what forces
§4.

### Deliberately out of scope, each for its own reason

- **`local_contrast`** — spatial, and needs three blur kinds (`box_blur`, `wide_blur`, the guided
  filter) where sharpening needed one. Scoped out of M8.3c for the same reason.
- **`vignette`** — per-pixel but **position-dependent**, so it needs the frame's dimensions and
  `Region` threaded into a shader. A different mechanism, not a bigger version of this one.
- **`creative_look`** — a 3-D LUT, so it needs a 3-D texture upload rather than a storage buffer.
- **masks** — `MaskStage` is one opaque stage to `accelerate` by construction; that is M8.3f.

## 4. The one new mechanism: a cost gate

⚠⚠ **Making `output_transform` replaceable creates a regression that does not exist today.** A
neutral frame's tail is `["output_transform"]` alone: 28.63 ms of CPU work against a 34.03 ms
round trip, so `accelerate` would build a one-stage chain and **lose ~5 ms on the cheapest frame
the app renders**. Browsing an unedited folder is exactly when that is felt.

**A stage count is the wrong gate.** One *expensive* stage pays easily — `hsl` alone is a measured
1.56× — while three cheap ones may not. The quantity that decides it is **work**, so that is what
the gate measures.

⚠ **It forces a shape change in `accelerate`.** The run currently holds `Box<dyn GpuStage>` and
has already *dropped* the CPU stage it replaced — so a gate that refuses a run would have nothing
to put back, and `GpuStage::cpu()` hands out a borrow that cannot be pushed into the output. The
run therefore carries **both** — the built GPU stage and the original — and `flush` chooses. Found
while planning rather than by the compiler, which would have accepted a `cpu()`-shaped design and
failed only at the borrow.

```rust
/// This stage's CPU cost, in milliseconds per 6.8 megapixels.
///
/// ⚠⚠ Used ONLY to decide whether a run is worth a round trip. It fails toward "run on the
/// CPU", which is always correct — a wrong number costs milliseconds, never pixels.
fn cpu_cost(&self) -> f32;
```

⚠⚠ **THE COMPARISON IS SCALE-FREE, WHICH IS WHAT MAKES THIS ONE CONSTANT PER STAGE RATHER THAN A
TABLE PER RUNG.** Both sides are linear in pixel count — the transfer moves `w·h·12` bytes and a
per-pixel stage does `w·h` work — so the ratio does not depend on resolution. Confirmed rather
than assumed: 4× the pixels gave **3.9×** the round trip (8.71 → 34.03 ms) and **3.3×** the
display rendering (6.73 → 22.24 ms). The gate is therefore `run_cost > TRANSFER_COST`, with
`TRANSFER_COST = 34.0` in the same units, and no rung enters it.

⚠ The units are deliberately *measured milliseconds at a named size* rather than abstract points,
because a number nobody can re-derive is a number nobody can check. `probe_stages` and `probe_rt`
are kept in the repo for exactly that.

⚠ **This is a performance switch, not a correctness one**, which is what distinguishes it from the
cost heuristic `PrefixCache` deliberately refused: there, a wrong guess would have changed *what
is cached* and so what is rendered; here the worst case is that a frame runs on the CPU exactly as
it does today.

## 5. Per-stage notes

**Three of the eight collapse to a 1-D LUT**, and in every case the CPU stage already owns that
table — so the GPU stage **uploads the CPU stage's own**, never rebuilds it. That is the rule
`HueTable::entries` set and `ColorGradingStage::lut()` followed in M8.3d: two constructions of one
table is how the two come to disagree about something nobody re-derives, and **a wrong table
renders a plausible photograph rather than an error.**

- **`tone_regions`** — ⚠ its output is a **scalar gain that is a pure function of luminance**
  (`gain_for_luma`), so the whole four-slider model is one table indexed by luminance. ⚠⚠ And the
  weights are shaped on **display-encoded** luminance, not linear — that asymmetry is the fix for
  "the Whites slider did nothing on most photographs" and must be inside the table, not
  re-derived in WGSL.
  - ⚠⚠ **This one table does not exist on the CPU yet, so building it introduces a quantization
    the CPU stage does not have** — unlike `tone_curve` and `display_rendering`, which are already
    tables. That is the deliberate trade: one implementation of the weight shaping with a bounded
    table error, against two implementations that cannot drift on a number nobody re-derives.
    ⚠ The size is **measured, not chosen** — the recipe `DisplayRenderingStage`'s own 4096 came
    from, sweeping until the drift falls below a tenth of an 8-bit level — and the table is built
    **by the CPU stage**, so there is still only one `gain_for_luma`.
- **`tone_curve`** — a 1024-entry LUT already built by the CPU stage, applied per channel.
- **`display_rendering`** — a **4096**-entry LUT (the size is measured: 1024 drifts 0.61 of an
  8-bit level) applied per channel, then `chroma_scale(y)` about the pixel's own luminance. ⚠ Its
  per-channel curve is the one tonal stage in the pipeline that **deliberately does not preserve
  hue**; a shader that "fixed" that by curving luminance and scaling RGB would be a different
  stage wearing the same name.
- **`output_transform`** — a 3×3 matrix as a uniform, then one of **three** transfer curves
  (sRGB and Display P3 share one; Adobe RGB is gamma 563/256; ProPhoto has a toe below
  `PROPHOTO_TOE_END`). ProPhoto's matrix is the identity and must stay exactly so.
  - ⚠⚠ **`encode` CLAMPS TO [0,1] BEFORE THE `powf`, AND THAT CLAMP IS WHAT MAKES THE SHADER
    SAFE.** WGSL's `pow` returns **NaN for a negative base**, and out-of-gamut colour reaches this
    stage with negative components *by design* — that is the point of a wide working space. The
    CPU clamps first, so the GPU must clamp first too. This is the `cbrt_signed` trap in a new
    costume, and it gets a named test with an out-of-gamut fixture, because a fixture that is
    entirely in gamut cannot see it.
- **`white_balance`** — the illuminant solve stays on the CPU; the shader receives three gains.
- **`saturation`** — luminance, then `chroma = (max−min)/max`, then one scale. ⚠ `EPS` guards the
  division and must be carried over verbatim.
- **`contrast`** — `PIVOT + (s − PIVOT)·slope`, floored at 0.

## 6. Testing

The M8.3b/c/d playbook, unchanged, because it has caught real bugs three times:

1. **One equivalence test per stage**, GPU against its own CPU twin, over a fixture that is
   **scene-referred** — negatives and values above 1.0 — since that is where the two diverge and
   an in-gamut fixture is blind to it.
2. **A "the stage really does something" guard per stage.** ⚠⚠ An equivalence test compares GPU
   against CPU, so **a sabotage deleting both agrees perfectly**. This is the guard that exists
   because the goldens cannot see this crate at all.
3. **A carrier test for the whole run** — the eight-stage chain renders the same picture as the
   eight CPU stages. ⚠ Its bound is a **measured property of composition** and will be larger than
   any single stage's: M8.3d found three transcendental stages each under 0.01 levels composing to
   0.0115, and the fix was the fixture and an honest bound, never a looser assertion on an easy
   fixture.
4. **The gate's own tests**, both directions: a lone `output_transform` builds **no** chain, and a
   lone `hsl` still does.
5. **63 goldens must not move.** They run on the CPU and this touches no CPU pixel path.

## 7. Two plans

- **Plan 1 — the four the typical photograph runs**, plus the gate: `exposure` (a table row),
  `tone_regions`, `display_rendering`, `output_transform`. ⚠ This is the entire measured win;
  everything after it is coverage.
- **Plan 2 — the remaining four**: `white_balance`, `contrast`, `tone_curve`, `saturation`.

⚠ Split this way because plan 1 is independently shippable and independently *measurable* — the
bench's TYPICAL row moves or it does not — while plan 2 changes that row by nothing at all and
must be judged on a different case.

## 8. Accepted caveats

1. **A truly neutral frame is not accelerated**, by design (§4). The gate keeps it on the CPU
   rather than losing 5 ms.
2. **Nothing here helps export.** `batch_export` builds its pipeline directly and never calls
   `accelerate`; and ~93% of an export is libraw's demosaic upstream of every stage `wgpu` could
   ever touch.
3. **`gpu_tail` is still off by default**, so a fresh install sees no change.
4. **The shader and pipelines are still rebuilt on every encode** — eight stages make that eight
   rebuilds rather than three. It was measured as not the bottleneck at three; ⚠ **at eight that
   should be re-measured rather than assumed**, and a pipeline cache is the standing answer.
5. **The 8-bit readback is deliberately not taken.** With `output_transform` ending the chain the
   result is display-referred `[0,1]` and could be downloaded as RGBA8 — **4× less readback, which
   is the larger half of the transfer**. It changes `GpuChain`'s contract and `pack`'s role, so it
   is its own slice. Recorded here because this port is what makes it possible.
6. The cost constants are measured on one machine (Apple M2) with one buffer size. They are a
   *ratio* against a transfer measured the same way, which is what makes them portable-ish; a
   machine with a very different memory topology would want its own numbers.
