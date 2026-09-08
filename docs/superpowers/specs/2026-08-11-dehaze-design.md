# Dehaze — design

**Date:** 2026-08-11
**Status:** approved, ready to plan
**Scope:** `edit-model`, `pipeline` (one new stage, one new spatial primitive, one generalised), `app` (one slider)

Closes **M2.3**, the last third of the texture / clarity / dehaze line item.

## The finding that shaped this

The handoff note from the texture/clarity cycle said dehaze would force a new
"global statistics computed upstream" seam in the pipeline interface, because the
dark-channel model needs a **frame-global airlight** and no tail stage can compute one from a
cropped region without breaking `a_region_render_matches_the_whole_render_cropped` in the
*interior*.

That is true only if dehaze runs in the **tail**. Reading `render_engine::render_cached`:

```rust
let mid = /* prefix.run(buffer.downscale_to(max_dim)) — whole frame, cached */;
let rect = region.and_then(|r| r.clamped(pw, ph));   // the crop happens AFTER
tail.run(mid.crop(...))
```

The **prefix always runs whole-buffer**. Putting dehaze there dissolves the whole problem:
no new seam, no `Region` plumbing, no growth in `REGION_MARGIN` however large the patch gets,
and the master region invariant keeps passing untouched. It is the same property that makes
noise reduction incapable of producing a seam.

**So the architectural work this feature was expected to need does not exist.** What is left
is a stage, two spatial primitives, and a slider.

## The model

Standard haze model, per channel:

```
I = J·t + A·(1 − t)
```

`I` observed, `J` haze-free, `A` atmospheric light (global), `t ∈ (0,1]` transmission —
`t → 0` is fully hazed, `t = 1` is clear.

Dark-channel prior (He et al.): in a haze-free outdoor patch, some channel is near zero.

```
dark(x) = min over patch Ω(x), min over channels, of  I_c(y) / A_c
t̃(x)    = 1 − OMEGA · dark(x)                              OMEGA = 0.95
t(x)    = guided_filter(t̃, guide = luma(I), r_refine, eps) ← cross-guided, see below
```

`OMEGA < 1` deliberately leaves a little haze: removing all of it flattens aerial perspective
and looks wrong, and it also softens the failure case in caveat 1.

### Linear light is *more* correct here

The haze model is **radiometric** — a statement about light transport — so it holds exactly
in the linear working space and only approximately in the gamma-encoded space most published
implementations use. That is a point in our favour and worth stating.

The asymmetry worth writing down: the *prior* is an empirical statistic gathered on
gamma-encoded images. It is a claim about darkness, which linear encoding preserves, so it
should carry over — but it is an inherited assumption, not something this pipeline proves.

### One slider, and both directions share an expression

With `k = amount/100`:

```
t_k = 1 − |k|·(1 − t)                          the estimated haze, scaled by the slider

k > 0:  J = (I − A) / max(t_k, T_MIN) + A      invert the model — remove haze
k < 0:  J = I·t_k + A·(1 − t_k)                apply it forward — add haze
```

At `k = 0` both branches collapse to `J = I` **exactly**, which is what lets every existing
golden stay byte-identical.

At `+100` it removes the estimated haze. At `−100` it lays the same estimate back on — and
because `t` encodes depth, **negative dehaze is depth-aware**: distant regions get hazier and
near ones do not. That falls out free, since `t` is already computed, and is a much better
effect than the uniform veil a simpler model would give.

Guards: `T_MIN = 0.1` bounds the gain in dense haze; the result clamps at 0, because a pixel
darker than the airlight can otherwise be driven negative by the division.

### Constants

| name | value | meaning |
|---|---|---|
| `OMEGA` | 0.95 | haze deliberately retained, so aerial perspective survives |
| `T_MIN` | 0.1 | transmission floor; bounds the gain to ×10 in dense haze |
| `PATCH_RADIUS_FULL` | 20 px | dark-channel patch, in **full-resolution** pixels, scaled by `RenderScale` like NR's radii (`max(1, round(base·scale))`, never 0) |
| `REFINE_RADIUS_FULL` | 60 px | guided-filter radius for `t`, same scaling. Deliberately ≫ the patch: the refinement's job is to restore edges the patch min smeared |
| `REFINE_EPS` | 1e-4 | guided-filter regularisation, against **linear** luma variance |
| `AIRLIGHT_SAMPLE` | 256 px | fixed long edge for the airlight estimate |
| `AIRLIGHT_FRACTION` | 0.001 | top fraction of the dark channel used |
| `A_MIN` | 1e-3 | floor on each channel of `A` — load-bearing, `I_c/A_c` divides by it |

All five of the first are tuning constants (caveat 5). `REFINE_EPS` in particular is quoted in
the literature against gamma-encoded values; in linear light mid-grey is 0.18, so variances
are smaller and this may want to come down. Validate, do not trust.

## Airlight is estimated at a fixed resolution

`A` is taken per channel as the **mean of the brightest `AIRLIGHT_FRACTION` of pixels by dark
channel** — the mean rather than the paper's single brightest pixel, so one specular highlight
or hot pixel cannot set the airlight for the whole frame. At least one pixel is always taken,
and each channel is floored at `A_MIN` because `I_c / A_c` divides by it.

**It is computed on a fixed 256 px downscale of the stage input, not at the render
resolution.**

⚠ This is load-bearing. An order statistic over the brightest pixels moves when you
downscale — averaging pulls extremes toward the mean — so estimating `A` at the render scale
would give the fit preview, the 1:1 view and the export three different airlights, and the
image would visibly shift when zooming. Pinning the estimate to a fixed size makes `A`
render-scale-independent **by construction** rather than by tuning. It is also nearly free,
and cheaper than the alternative at export.

This is the principle `RenderScale` already serves — the preview must approximate the export
— applied to a *statistic* instead of a radius.

The patch min used for `t` still runs at the render resolution with a radius scaled by
`RenderScale`, exactly as noise reduction's radii are.

## Machinery

### `spatial::min_filter(plane, w, h, r)` — new

Separable min over a `(2r+1)²` window, **O(1) per pixel regardless of `r`**, via a monotonic
deque (indices held in increasing order of value; push at the back popping any element that
is not smaller, pop the front once it leaves the window).

The naive version costs `2(2r+1)` comparisons per pixel: at a 20 px full-resolution patch
that is ~2 G comparisons on a 26 Mpx export, against ~78 M here. Same reason `box_blur` keeps
running sums.

A deque rather than van Herk / Gil-Werman specifically because **the window clamps at the
borders** — it shrinks rather than reading outside — which a deque handles naturally and the
block-decomposition of van Herk does not. Clamping the window is the same border rule
`box_blur` uses, and it keeps a constant plane exactly constant.

### `spatial::guided_filter(p, guide, r, eps)` — generalised from `noise_reduction`

The refinement step needs a **cross**-guided filter: filter `t̃`, guided by the image.
`guided_self_filter` in `stages/noise_reduction.rs` is the special case `p == guide`. It moves
to `spatial` in general form and noise reduction calls it with `p == guide`:

```
mean_I, mean_p, corr_I = box(guide²), corr_Ip = box(guide·p)
var_I  = corr_I  − mean_I²
cov_Ip = corr_Ip − mean_I·mean_p
slope  = cov_Ip / (var_I + eps)
offset = mean_p − slope·mean_I
out    = box(slope)·guide + box(offset)
```

With `p == guide` this reduces algebraically to exactly today's `slope = var/(var+eps)`,
`offset = (1−slope)·mean`.

⚠ **Algebraically identical is not bit-identical.** Today's code computes `offset` as
`(1.0 − s) * m`; the general form computes `m − s * m`. Those agree in real arithmetic and
may differ in the last `f32` ulp. So the acceptance criterion is stated honestly:

- The three noise-reduction goldens must **pass** (tolerance 2 per channel), and
- a direct test must show the generalised filter matching the current implementation on
  random data to within `1e-6`, and
- byte-identity of the reference PNGs will be **checked and reported**. If a last-bit
  difference moves a rounded 8-bit value, that is stated explicitly and the reference
  regenerated with the reason recorded — never waved through as "close enough".

Without the guided refinement `t` is blocky, and blurring it instead produces halos at depth
discontinuities — sky against mountain, which is exactly where dehaze gets used. That is what
the refinement exists to prevent.

### `DehazeStage`, in the **prefix**

```
geometry → noise_reduction → dehaze │ white_balance → … → local_contrast → … → sharpening → output
```

After noise reduction because dehaze is a contrast stretch that amplifies noise hard;
denoising first means the noise it multiplies has already been reduced. Same argument that
put NR ahead of Exposure and the tone curve.

**`PrefixKey` gains `dehaze`.** The tripwire test
`the_prefix_holds_only_geometry_and_noise_reduction` will fail until it does — that is the
test doing its designed job — and gets renamed. `prefix_key_ignores_every_downstream_field`
does **not** gain a dehaze case (dehaze is not downstream); the case belongs in
`prefix_key_changes_with_prefix_fields_and_scale` instead.

## Model and UI

```rust
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Dehaze {
    /// ~[-100, 100]. Positive removes haze, negative adds it. 0 = off.
    pub amount: f32,
}
impl Dehaze { pub fn is_active(&self) -> bool { self.amount != 0.0 } }
```

`Edits` gains `dehaze: Dehaze`. **Four exhaustive fixtures will refuse to compile until it is
given a home** — `pipeline::tests::all_edits`, `edit_model::tests::all_nonneutral`,
`Edits::merge_from`'s destructure, and `app::edit_clipboard`'s payload test. That is the
designed behaviour, not friction.

**Group: Tone**, with Texture and Clarity.

⚠ There is a real tension here worth naming rather than hiding: dehaze sits beside noise
reduction in the *pipeline*, and noise reduction is in the **Detail** group. Those answer
different questions — pipeline order is about mathematical correctness, groups are about what
a person wants to carry between photos — and dehaze is something you would copy along with a
look, not along with a lens profile.

**UI:** one slider, ±100, immediately after Clarity. Lightroom's Presence order is Texture,
Clarity, Dehaze, and ours now matches.

## Tests

### The one that carries the weight

**A ground-truth round trip.** Take a known image `J`, apply the forward haze model with a
*known* `A` and a synthetic depth map to produce `I`, then assert dehaze recovers `J` within
tolerance. That is an actual inverse-problem check against a known answer — not "the output
differs from the input", which almost anything would satisfy.

### `dehaze.rs`

- `amount_zero_is_an_exact_identity` — every existing golden depends on it.
- `synthetic_haze_is_removed` — the round trip above.
- `plus_then_minus_approximately_round_trips` — modulo the `T_MIN` clamp.
- `negative_dehaze_is_depth_aware` — a far (low-`t`) region gains more haze than a near one.
- `a_clear_image_is_barely_changed` — dark channel ≈ 0, so `t ≈ 1`.
- `the_airlight_is_independent_of_the_render_scale` — ⚠ the fixed-256 px decision, asserted
  directly: the same frame at two render scales must yield the same `A`.
- `the_airlight_ignores_a_single_hot_pixel` — the mean-of-top-fraction choice, pinned: one
  blown speculum must not set `A` for the frame.
- `a_black_frame_does_not_divide_by_zero` — the `A_MIN` floor, on an all-zero image.
- `the_transmission_floor_bounds_the_gain`, `never_emits_negative_or_nan_samples`,
  `the_patch_radius_scales_with_the_render_scale`.

### `spatial`

- `min_filter`: constant plane stays constant; hand-computed 1-D and 2-D cases; `r = 0` is the
  identity; window clamps at borders; and the decisive one — **agreement with a naive
  reference implementation on random data**, because a monotonic deque is easy to get subtly
  wrong and every other test would still pass.
- `guided_filter`: matches the current self-guided implementation to `1e-6` when
  `p == guide`; a cross-guided case where `p` and `guide` genuinely differ.

### `pipeline`

- `the_prefix_holds_geometry_noise_reduction_and_dehaze` (renamed tripwire).
- `dehaze_runs_after_noise_reduction`.
- `neutral_dehaze_adds_no_stage`.
- `prefix_key_changes_with_prefix_fields_and_scale` gains a dehaze case.

### Goldens 27 → 29

A new chart built by **applying** the haze model to a synthetic scene with depth increasing
across the frame, so the reference encodes a known correct answer rather than whatever the
code happened to produce. Cases: `dehaze_plus`, `dehaze_minus`.

**All 27 existing references must stay byte-identical** — that is simultaneously the proof
that the stage is inert at its default *and* that the guided-filter refactor did not move
noise reduction. It is the check that matters most in this feature.

## Validation on real photographs

Non-optional. Two frames specifically:

1. **A hazy landscape**, for the effect itself — does `+50` read as removing atmosphere
   rather than as a contrast slider, and are there halos along a sky/ridge boundary?
2. **The X-T4 kitten against a white wall** — the adversarial case for caveat 1 below. A
   large bright achromatic region is precisely what the dark-channel prior mis-reads, and this
   frame is full of one.

Then re-measure the preview and prefix cost against the table in `CLAUDE.md`.

## Accepted caveats

1. **⚠ Bright achromatic objects violate the prior.** Snow, white walls, a white cat: their
   dark channel is high, so they are read as haze and over-corrected — darkened and often
   colour-shifted. This is *the* classic dark-channel-prior artifact, inherent to the model
   rather than to this implementation. `OMEGA < 1` and the guided refinement soften it; a
   proper fix needs large-bright-region or sky detection and is its own feature.
   ✅ **Softened 2026-09-08** by an airlight tolerance (`AIRLIGHT_TOLERANCE` 0.1): a pixel within
   0.1 of the airlight in every channel is an object, not haze. The white-wall case is closed
   (18.7% → 3.4% darkening at +100); cloudy skies and a bright object whose frame's airlight is
   elsewhere remain — see `2026-09-08-dehaze-bright-tolerance-design.md`, which also records that
   region detection was rejected because the catalogue holds no genuinely hazy frame to test it on.
2. **Dragging Dehaze re-runs noise reduction** — ~29 ms at preview, ~141 ms for a native
   prefix. The cost accepted in exchange for needing no new architecture. Every other slider
   still hits the cache. A second cache checkpoint is already a standing perf item and would
   remove this.
3. **`A` is estimated per photo**, so the same Dehaze value pasted or synced onto another
   frame can look different. Inherent to a content-adaptive correction; Lightroom behaves the
   same way.
4. **A single global `A`**, so a scene with two lighting regions — bright sun and deep shade —
   gets one compromise value.
5. `OMEGA`, `T_MIN`, the patch radius and the refinement radius/eps are **tuning constants**.
   They will be settled by looking at photographs, as the vignette's were, not by this
   document.
