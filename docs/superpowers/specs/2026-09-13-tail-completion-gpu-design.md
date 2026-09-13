# M8.3p — the vignette and the creative look, and the tail becomes contiguous

**Date:** 2026-09-13
**Status:** design

---

## 1. Why this, and why now

Twelve of the fourteen tail stages run on the GPU. The two that do not — `vignette` and
`creative_look` — are not slow. They are **splitters**: each sits between ported stages, so a
photograph using either pays a whole extra round trip.

⚠⚠ **The number is the transfer, not the stage.** A vignette costs **+1.6 ms** of CPU work at
fit. Leaving it on the CPU costs **~27 ms**, because it cuts one chain into two and the second
pays `TRANSFER_COST` again. M8.3c measured exactly this shape and recorded it as the milestone's
own headline caveat: *"an active colour grade splits the run and costs almost the whole win —
1.52× → 1.04×"*. Grading was ported; these two were not.

**And the prize is an invariant rather than a speed-up.** With both ported, every stage
`build_pipeline_split_in_region` can place in a tail is replaceable except `MaskStage` — so a
tail with no mask is **always exactly one chain**, rather than usually one. That makes the
run-splitting path in `accelerate` unreachable for an unmasked photograph, which is the third
time this crate has retired a failure mode by completing a set (M8.3e's *"a tail with nothing
replaceable"*, M8.3h's *"a trailing run refused"*, M8.3n's *"a chain behind a CPU stage"*).

### ⚠⚠ The catalogue is not evidence and is not used here

Three milestones leaned on *"of the catalogue's 40 sidecars, N use this"*. The user has stated
plainly that he has not been using Rustroom, so those folders record nothing about what a
photographer does. **This milestone is scoped structurally: close the region, not the popular
row.** The vignette is 0 of 40 and the creative look 0 of 40, and neither fact is admitted as an
argument.

---

## 2. What is actually missing from the GPU pipeline

Recorded here because the answer reframes the rest of Phase 8. **21 stages; 12 on the GPU.**

| region | stage | GPU | measured |
| --- | --- | --- | --- |
| prefix | `lens_correction` | ✗ | **~210 ms** at rung 3200, **on by default for every RAW** |
| prefix | `geometry` | ✗ | ~26 ms crop-only; ~200 ms with a rotation |
| prefix | `perspective` | ✗ | a resample, when used |
| prefix | `spot_removal` | ✗ | per patch |
| prefix | `noise_reduction` | ✗ | **~84 ms** at 6.5 Mpx |
| prefix | `dehaze` | ✓ | 96 → 66 ms (M8.3o) |
| prefix | masked capture | ✗ | |
| tail | twelve stages | ✓ | |
| tail | **`vignette`** | ✗ | **this milestone** |
| tail | **`creative_look`** | ✗ | **this milestone** |
| mask | sub-stages (10/10), brush coverage | ✓ | |
| mask | analytic + range coverage | ✗ | a colour range's blend is 61–120 ms |
| mask | the blend | ✗ | 9–11 ms per mask |

⚠⚠ **Every GPU milestone so far has been on the cheap half.** Measured today at a native rung:
the **prefix is 238.6 ms against a 32.5 ms tail**, and the prefix is five-sixths CPU. Lens
correction alone is larger than everything Phase 8 has ported put together.

⚠ They are different complaints and both are real: the **tail** is paid on every frame of every
drag, so it decides whether dragging feels smooth; the **prefix** is paid when a prefix slider
moves or the rung changes, so it decides whether those moments stall. The tail is nearly closed,
which is why finishing it is cheap. The prefix is next, and it is where the time is.

---

## 3. The vignette

Position-dependent, per-pixel, no neighbours. Two shader branches matching the CPU's two.

### 3.1 ⚠⚠ It needs the frame, which the tail's seam does not currently carry

`VignetteStage` measures its falloff against the **whole frame** even when rendering a region, so
a cropped viewport is shaded exactly as that part of the whole would have been. It takes a
`Region { full_w, full_h, x0, y0 }` for that. `accelerate` has never needed one.

**`accelerate` gains an `Option<Region>` parameter.** One production call site
(`render_engine.rs:1261`), where `region` is already a local three lines above.

⚠ **Deliberately NOT a field of `RenderSpec`**, even though it is a render property and would be
`Copy + PartialEq`. `build_pipeline_split_in_region` takes `region` *beside* `render_spec`, so
putting it inside would give the region two homes in one call — the two-facts-that-can-disagree
shape `render_max_dim`, `fit_image_size` and `Session::geometry_mode` were each fixed for. A
separate parameter mirrors the CPU builder exactly.

### 3.2 ⚠⚠ One derivation, two callers — `pipeline::vignette_params`

`Shape::new`, the two `lerp`s and the `e0`/`e1`/`k` arithmetic are private to the CPU stage.
Transliterating them into a builder would be a second derivation of the same model, and the two
would disagree about `R_MID`, `FEATHER` or the superellipse the first time either moved —
**invisibly**, because both answers are plausible photographs.

So `pipeline` exposes:

```rust
/// Everything a vignette evaluation needs, derived once from the parameters and the frame.
pub struct VignetteParams {
    pub wu: f32, pub wv: f32, pub expo: f32, pub elliptical: bool,
    pub e0: f32, pub e1: f32, pub k: f32,
}
pub fn vignette_params(p: &Vignette, fw: usize, fh: usize) -> VignetteParams;
```

`VignetteStage::apply` is rewritten to call it, so the CPU path *is* the reference rather than
merely agreeing with it. **The 63 goldens staying byte-identical is the proof that extraction
moved nothing.** This is `white_balance_gains`' precedent exactly.

### 3.3 The shader

Uniforms: `x0, y0` (u32), `hw, hh` (f32 half-frame), and the seven `VignetteParams` fields.

```wgsl
let u = (f32(gid.x + p.x0) + 0.5) / p.hw - 1.0;
let v = (f32(gid.y + p.y0) + 0.5) / p.hh - 1.0;
let du = u * p.wu; let dv = v * p.wv;
var d: f32;
if (p.elliptical != 0u) { d = sqrt(du*du + dv*dv); }
else { d = pow(pow(abs(du), p.expo) + pow(abs(dv), p.expo), 1.0 / p.expo); }
let t = clamp((d - p.e0) / (p.e1 - p.e0), 0.0, 1.0);
let f = p.k * (t * t * (3.0 - 2.0 * t));
```

⚠ The smoothstep is written out rather than using WGSL's built-in `smoothstep`. The formula is
the same, but writing it matches the CPU's **expression shape**, so any divergence is the known
FMA class rather than a third thing to explain.

⚠ `pow` is safe here where it was not in `oklab_wgsl`: the base is `abs(du)`, never negative, so
the `sign(x) * pow(abs(x), e)` guard M8.3b needed does not apply. `pow(0.0, e)` for `e > 0` is 0.

### 3.4 ⚠⚠ The lift's `max(0.0)` needs a RAW-FLOAT test

```rust
let s = 1.0 - (1.0 - f) * (1.0 - f);
out = v + max(1.0 - v, 0.0) * s;
```

That `max` is what leaves a scene-referred sample **already above 1.0** alone rather than
dragging it back toward white. Drop it and a highlight at 1.4 gets pulled *down* — and
`compare` quantises to 8-bit levels, where everything above 1.0 is 255 and the bug is invisible.
This is M8.3e's output-transform clamp lesson and M8.3m's four repeats of it, in mirror form: the
clamp is at the **top** rather than at zero, and the same blindness applies.

`a_scene_referred_highlight_is_not_dragged_down` reads raw floats. It is the only test in the
workspace that can see it.

### 3.5 Equivalence, and what it can be

⚠ **Not bit-identical.** The smoothstep is a divide plus multiply-adds (FMA contraction, M8.3m)
and the superellipse is two `pow` calls. Within one 8-bit level, and the fixture must carry a
**negative roundness** so the `pow` branch is exercised — an elliptical-only fixture takes the
`sqrt` fast path and says nothing about the branch most likely to diverge.

⚠ The fixture must also be **non-uniform**, or a gain applied at the wrong coordinate produces
the same picture. `the_stage_really_vignettes_rather_than_copying` guards that the fixture is one
this stage moves — the rule M8.3c established and every port since has followed.

### 3.6 `cpu_cost`

Measured by `probe_stages` at 3200×2136, three runs, shipped at the **bottom** of its band.
Expected small — around 8–10 ms, well under `TRANSFER_COST`'s 27.0.

⚠⚠ **So on its own it will never chain, and that is correct.** Its value is entirely in *joining*
a run that already pays — which is the same reason `contrast` was worth porting at 1.2 ms:
**what a stage buys is not always its own cost.**

---

## 4. The creative look

### 4.1 It needs no new parameter

`ProfileRendering` already carries `look: Option<&Arc<Cube>>`, and `accelerate` already receives a
`ProfileRendering`. The builder reads `profile.look` exactly as `display_rendering` reads
`profile.curve`.

⚠ `None` means the look does not run — a preset naming a `.cube` absent from this machine must
render *without the look*. The builder returns no stage, matching the CPU's `if let Some(cube)`.

### 4.2 The cube is a storage buffer, not a 3-D texture

`Cube::entries` is `Vec<[f32; 3]>`, tightly packed, so it uploads as a flat `array<f32>` indexed
`3 * ((b * size + g) * size + r) + c` — **the CPU's own layout, byte for byte**. `Cube` gains
`pub fn entries(&self) -> &[[f32; 3]]` and accessors for `dmin`/`dmax`, so the table is uploaded
rather than rebuilt. `HueTable::entries` and `ColorGradingStage::lut()` are the precedent.

⚠⚠ **Hardware filtering is deliberately refused.** A `texture_3d` with a linear sampler would do
the trilinear interpolation for free — and `Rgba32Float` is not filterable on every backend, and
where it is, the sampler quantises the interpolation weight to a fixed sub-texel precision. The
CPU's `lookup` does its own trilinear with `a + (b - a) * t`, chosen so an identity cube is
**bit-exact at every grid point**. Matching that means doing the same arithmetic in the shader.

⚠ **The domain remap comes first, then the clamp** — `t = (v - dmin) / (dmax - dmin)` then
`clamp(t, 0, 1) * n`. `lut.rs` records that clamping the raw value first is "the old bug wearing
a different hat", and the shader must not reintroduce it. A real ACES LMT declares
`LUT_3D_INPUT_RANGE 0.0927061729 1.0196260353`, so this is reachable in practice.

### 4.3 The shaper

Three cases, selected by a `u32` tag:

- **`Linear` → no shaper at all.** ⚠⚠ That single branch is what makes a linear LUT a *rendering*
  rather than a grade. Getting it wrong applies a transfer the LUT then applies again, and the
  result is a plausible photograph.
- **`SrgbDisplay`** — `prophoto_to_srgb`, sRGB transfer, look up, sRGB inverse, `srgb_to_prophoto`.
- **`Rec709`** — the same matrices, BT.709's transfer.

⚠ **The matrices are substituted from `color::spaces::{PROPHOTO_TO_SRGB, SRGB_TO_PROPHOTO}`**,
which are already `pub const`. A substituted constant cannot disagree; a transcribed one is
merely checked for agreeing — the rule `luma_wgsl` records and `color`'s own doc justifies with
six copies of a Rec.709 triple that all went wrong on one day.

⚠ The **transfer constants** (12.92 / 0.0031308 / 1.055 / 0.055 for sRGB; 4.5 / 0.018 / 1.099 /
0.099 / 0.45 for Rec.709) are transcribed, because they are scalars inside private functions
rather than public constants. Guarded **behaviourally**, `oklab_wgsl`'s pattern: an **identity
cube** through each space must return the input, which is only true if encode and decode are a
matched pair *and* the matrices round-trip. `an_identity_cube_returns_the_input_in_every_space`.

### 4.4 The strength short-circuit is a correctness guard

```wgsl
if (p.blend == 0.0) { return; }   // leave the pixel exactly as it was
```

⚠ Not a speed path. The shaper **clamps out-of-gamut values**, so running it at strength 0 and
lerping back leaves a residue on saturated colours — a look dialled to zero that is not quite no
look. The CPU records this as `strength_zero_is_bit_exactly_the_input`; the GPU needs the same
guard and the same test.

### 4.5 ⚠ The upload is the one honest cost, and it is measured rather than assumed

A 33³ cube is 430 KB, a 65³ is 3.3 MB, and `Cube::MAX_SIZE` of 128 is **25 MB — uploaded on every
encode**, because M8.3f deliberately does not cache per-render buffers.

The plan **measures it** at 33³ and 65³ before deciding anything. If a 65³ upload is material,
the follow-up is to key it in `GpuCache` by the `Arc`'s pointer identity — the table is immutable
and shared across renders — and that is recorded rather than done here.

---

## 5. The invariant, and the test that keeps it

With both ported, **every stage the tail builder can place is replaceable except `MaskStage`**.

`every_tail_stage_is_replaceable_or_a_mask` builds a tail from the exhaustive `all_edits()`
fixture and asserts each stage's name is either in `REPLACEABLE` or `"mask"`.

⚠⚠ **That is what stops the tail silently un-completing.** A fifteenth tail stage added later
would split every chain again, and nothing else in the build would say so — the failure M8.3j's
stale hint string and the "Reset crop" hover text each shipped. This is the same class of guard
as `sub_edits`' exhaustive destructure, and it is the reason this milestone is worth more than
its milliseconds.

⚠ Stated honestly: **a masked photograph still pays two round trips**, because `MaskStage` is not
replaceable and splits the outer run. Closing that is the mask region's own work (§2).

---

## 6. Accepted caveats

1. **Neither stage is fast on its own** — the win is the round trip they stop costing, and only
   on photographs that use them.
2. **A masked photograph still splits into two chains.** Unchanged here.
3. **Not bit-identical** — `pow`, a divide and multiply-adds put both stages in the established
   divergence classes.
4. **The cube uploads per encode**, up to 25 MB at `MAX_SIZE`. Measured in the plan; a pointer-keyed
   cache is the recorded follow-up.
5. `accelerate` gains a parameter, which every call site and test must pass.
6. One machine, one adapter (Apple M2, Metal).
7. **`gpu_tail` is still off by default.**
