# M8.3m — the rest of the per-pixel tail: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:executing-plans.
>
> ⚠⚠ **Not negotiable:** branch per feature; commit messages carry **no `Co-Authored-By`
> trailer**; **never push** — the user pushes to `github.com/wladiKrin/rustroom` himself.

**Goal:** white balance, contrast, the tone curve and saturation as compute shaders, so the
per-pixel tail is contiguous and a mask carries 9 of 10 sub-stages on the GPU.

**Architecture:** four `Dispatch` clients in `gpu-pipeline`, each per-pixel, in-place,
`scratch_planes = 0` — the `GpuExposureStage` shape. Four new `REPLACEABLE` rows.

**Spec:** `docs/superpowers/specs/2026-09-13-per-pixel-tail-plan-2-design.md`

**Branch:** `m8.3m-per-pixel-tail-2`

---

## Facts established before writing this plan — do not re-derive

- Measured costs at 3200×2136, three runs of `pipeline`'s `probe_stages`: wb **3.77–4.33**,
  contrast **0.97–1.97**, tone_curve **8.07–9.27**, saturation **7.19–9.39**. Ship the **bottom**
  of each band: **4.0 / 1.2 / 8.0 / 7.2**.
- `RenderSpec.baseline` already exists — M8.3e added it for this. No signature changes.
- `luma_wgsl()` already substitutes `color::PROPHOTO_LUMA`; saturation reuses it.
- `gpu-pipeline` is a **library**, so `pub` items are public API and land without a caller. Each
  stage can therefore be its own commit.
- The CPU bodies to match exactly:
  - wb: `[r*gr, g*gg, b*gb]`
  - contrast: `(PIVOT + (s - PIVOT) * slope).max(0.0)`, `PIVOT = 0.18`
  - tone_curve: clamp `v` to `[0,1]`, `pos = v*1023`, `i = floor(pos)`, `i >= 1023 → lut[1023]`,
    else `lut[i] + (pos - i) * (lut[i+1] - lut[i])`
  - saturation: `luma = prophoto_luminance(rgb)`, `chroma = ((max-min)/max(max, EPS)).clamp(0,1)`,
    `f = f_sat * (1 + vib*(1-chroma))`, each channel `(luma + (c-luma)*f).max(0.0)`, `EPS = 1e-6`

---

### Task 1: prepare the seams in `pipeline`

**Files:** `crates/pipeline/src/stages/{contrast,saturation,tone_curve}.rs`, `crates/pipeline/src/lib.rs`

- [ ] **Step 1: make the two constants public, with the reason at each**

`contrast.rs`:
```rust
/// Linear middle grey — the value contrast pivots around.
///
/// ⚠ `pub` so `gpu-pipeline`'s shader can **substitute** it rather than transcribe it. A
/// transcribed constant is checked for agreeing; a substituted one cannot disagree. `luma_wgsl`
/// records why that distinction is worth a `pub`.
pub const PIVOT: f32 = 0.18;
```

`saturation.rs`:
```rust
/// Guard against dividing by zero when computing chroma of a black pixel.
///
/// ⚠ `pub` for the shader to substitute — see [`crate::stages::contrast::PIVOT`].
pub const EPS: f32 = 1e-6;
```

⚠ Check how `stages` re-exports things (`grep -n "pub use stages::" crates/pipeline/src/lib.rs`)
and add whatever re-export the existing constants use. If `stages` is private, re-export as
`pub use stages::contrast::PIVOT as CONTRAST_PIVOT;` and `saturation::EPS as SATURATION_EPS;`
beside the existing stage re-exports.

- [ ] **Step 2: expose the tone curve's LUT**

In `tone_curve.rs`, beside `sample`:
```rust
    /// The compiled curve, for a consumer that samples it itself.
    ///
    /// ⚠⚠ **`gpu-pipeline` uploads THIS table rather than rebuilding one**, the rule
    /// [`crate::ColorGradingStage::lut`] and `HueTable::entries` already follow: two constructions
    /// of one curve is how a preview stops being evidence about the render.
    pub fn lut(&self) -> &[f32] {
        &self.lut
    }

    /// Entries in [`Self::lut`].
    pub const LUT_LEN: usize = LUT_SIZE;
```

- [ ] **Step 3: extract the white-balance gain expression**

⚠⚠ It currently lives only inside `adjustment_stages`. A GPU builder recomputing it would be a
second copy of "what as-shot means". In `lib.rs`, beside `adjustment_stages`:

```rust
/// The gains [`WhiteBalanceStage`] is built from, for a given white balance and baseline.
///
/// ⚠⚠ **One expression, two consumers** — `adjustment_stages` and `gpu-pipeline`'s builder. The
/// second was about to recompute it, which is how the CPU and GPU stages would come to disagree
/// about what "as shot" means: a wrong picture, not an error.
///
/// ⚠ `kelvin: None` is "as shot", which must resolve to the **baseline's** temperature so the
/// arithmetic lands on `[1, 1, 1]` — tint alone still needs a temperature to sit at.
pub fn white_balance_gains(
    wb: &edit_model::WhiteBalance,
    baseline: color::wb::Illuminant,
) -> [f32; 3] {
    let declared = color::wb::Illuminant::new(wb.kelvin.unwrap_or(baseline.kelvin), wb.tint);
    color::wb::adaptation_gains(declared, baseline, &color::spaces::PROPHOTO_TO_XYZ)
}
```

and rewrite the call site in `adjustment_stages` to use it. ⚠ Check the real field path for the
white balance — `grep -n "a.white_balance\|wb\." crates/pipeline/src/lib.rs | head` — and match it.

- [ ] **Step 4: verify the extraction moved no behaviour**

```bash
cargo test -p pipeline 2>&1 | grep -E "^test result"
```
Expected: **523 passed, 0 failed**, including 63 goldens. That the goldens are byte-identical is
the proof the extraction changed nothing.

- [ ] **Step 5: commit**

```bash
cargo fmt --all
```
```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings; echo "CLIPPY=$?"
```
```bash
git add -A && git commit -F - <<'EOF'
refactor(pipeline): expose what the GPU shaders must not transcribe

PIVOT and EPS become pub so their shaders can substitute them rather than
transcribe them — a substituted constant cannot disagree where a transcribed one
is merely checked for agreeing, which is why luma_wgsl is stronger than
oklab_wgsl. ToneCurveStage::lut exposes the compiled curve so gpu-pipeline
uploads the CPU stage's own table, the rule ColorGradingStage::lut already
follows. And white_balance_gains extracts the as-shot expression that lived only
inside adjustment_stages, before a GPU builder could recompute it and let the two
disagree about what "as shot" means.

Behaviour-preserving: 63 goldens byte-identical.
EOF
```

---

### Task 2: `GpuWhiteBalanceStage`

**Files:** Create `crates/gpu-pipeline/src/white_balance.rs`; modify `lib.rs`, `accelerate.rs`

- [ ] **Step 1: the stage**

```rust
//! White balance as a compute shader: three multiplies per pixel.

use crate::{GpuContext, GpuImage};
use core_image::ImageBuf;
use pipeline::PipelineStage;
use std::sync::Arc;

/// ⚠ The gains arrive as uniforms, computed CPU-side by `pipeline::white_balance_gains`. The
/// shader does three multiplies and no colour maths at all, which is what makes the result
/// bit-identical and a divergence here mean *plumbing* rather than float drift —
/// `GpuExposureStage`'s reasoning exactly.
pub(crate) const SHADER: &str = r#"
struct Params {
    gr: f32,
    gg: f32,
    gb: f32,
    pixels: u32,
    row_groups: u32,
    pad: vec3<u32>,
};
@group(0) @binding(0) var<storage, read_write> data: array<f32>;
@group(0) @binding(1) var<uniform> params: Params;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let pixel = id.y * (params.row_groups * 64u) + id.x;
    if (pixel >= params.pixels) { return; }
    let p = pixel * 3u;
    data[p] = data[p] * params.gr;
    data[p + 1u] = data[p + 1u] * params.gg;
    data[p + 2u] = data[p + 2u] * params.gb;
}
"#;

pub struct GpuWhiteBalanceStage {
    gains: [f32; 3],
    cpu: pipeline::WhiteBalanceStage,
    ctx: Arc<GpuContext>,
}

impl GpuWhiteBalanceStage {
    pub fn new(gains: [f32; 3], ctx: Arc<GpuContext>) -> Self {
        Self {
            gains,
            cpu: pipeline::WhiteBalanceStage::new(gains),
            ctx,
        }
    }

    fn clone_stage(&self) -> Self {
        Self::new(self.gains, self.ctx.clone())
    }
}

impl crate::GpuStage for GpuWhiteBalanceStage {
    /// Measured at 3200x2136 by `pipeline`'s `probe_stages`: 3.77-4.33 ms over three runs.
    ///
    /// ⚠ The **bottom** of the band, which is the conservative direction for a cost: a lower
    /// number makes a run less likely to chain, and the gate must fail toward the CPU.
    /// `TRANSFER_COST` is rounded up from the top of its own band for the mirror reason.
    fn cpu_cost(&self) -> f32 {
        4.0
    }

    fn name(&self) -> &'static str {
        "white_balance"
    }

    fn encode(
        &self,
        ctx: &GpuContext,
        pass: &mut wgpu::ComputePass,
        img: &GpuImage,
        _scratch: &[wgpu::Buffer],
    ) {
        let (row_groups, _) = crate::dispatch::Dispatch::grid(img.pixels());
        let params: [u32; 8] = [
            self.gains[0].to_bits(),
            self.gains[1].to_bits(),
            self.gains[2].to_bits(),
            img.pixels(),
            row_groups,
            0,
            0,
            0,
        ];
        crate::dispatch::Dispatch {
            label: "white_balance",
            shader: SHADER.to_string(),
            params: bytemuck::cast_slice(&params).to_vec(),
            storages: &[],
        }
        .encode(ctx, pass, img);
    }

    fn cpu(&self) -> &dyn PipelineStage {
        &self.cpu
    }
}

impl PipelineStage for GpuWhiteBalanceStage {
    /// ⚠ The **same** name as the CPU stage — six order-asserting tests match on it.
    fn name(&self) -> &'static str {
        "white_balance"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        crate::GpuChain::new(self.ctx.clone(), vec![Box::new(self.clone_stage())]).apply(image)
    }
}
```

⚠ Check `Dispatch`'s uniform alignment rules first — `sed -n '1,80p' crates/gpu-pipeline/src/dispatch.rs`.
A WGSL uniform struct is 16-byte aligned; the `[u32; 8]` above is 32 bytes and `pad: vec3<u32>`
rounds the struct to 32. If `Dispatch` already pads, simplify to `[u32; 5]` plus its padding and
drop `pad` from the struct — **match whatever `tone_regions.rs` does**, which is the closest
existing multi-field client.

- [ ] **Step 2: the tests** (in the same file)

Reuse `exposure.rs`'s `ctx()`, `awkward()` and `assert_same()` helpers — ⚠ **copy them into this
module rather than sharing**, because that is what every other stage file in this crate does and a
shared test helper would be the only one of its kind. If a shared module already exists
(`grep -n "mod test_support\|fn assert_same" crates/gpu-pipeline/src/*.rs`), use it instead.

```rust
    #[test]
    fn the_gpu_matches_the_cpu_bit_for_bit() {
        let Some(ctx) = ctx() else { return };
        // ⚠ Both sides of neutral, and an asymmetric triple: equal gains would pass against a
        // shader that used gr for every channel, which is precisely the plumbing bug to catch.
        for gains in [[1.3f32, 1.0, 0.8], [0.7, 1.0, 1.4]] {
            let img = awkward(64, 48);
            let cpu = pipeline::WhiteBalanceStage::new(gains).apply(img.clone());
            let gpu = GpuWhiteBalanceStage::new(gains, ctx.clone()).apply(img);
            assert_same(&gpu, &cpu, &format!("white balance at {gains:?}"));
        }
    }

    #[test]
    fn the_stage_is_named_like_the_cpu_one() {
        let Some(ctx) = ctx() else { return };
        assert_eq!(
            GpuWhiteBalanceStage::new([1.0; 3], ctx).name(),
            pipeline::WhiteBalanceStage::new([1.0; 3]).name()
        );
    }
```

- [ ] **Step 3: export it and add the `REPLACEABLE` row**

`lib.rs`: `mod white_balance;` and `pub use white_balance::GpuWhiteBalanceStage;`, both in
alphabetical position.

`accelerate.rs`, in `REPLACEABLE`:
```rust
    (
        "white_balance",
        &|e, _scale, _profile, spec, ctx| {
            Box::new(crate::GpuWhiteBalanceStage::new(
                pipeline::white_balance_gains(&e.white_balance, spec.baseline),
                ctx.clone(),
            ))
        },
    ),
```
⚠ Match the existing rows' exact closure signature — `grep -n "REPLACEABLE" -A 40 crates/gpu-pipeline/src/accelerate.rs`
— the parameter names and order are fixed by the table's type.

- [ ] **Step 4: run and gate**

```bash
cargo test --release -p gpu-pipeline white_balance 2>&1 | grep -E "^test |test result"
```
Expected: 2 passed.

```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings; echo "CLIPPY=$?"
```

- [ ] **Step 5: sabotage**

Take a `cp` backup first — **never `git checkout`**, which this project has twice used to destroy
uncommitted work. Change the shader's `params.gg` to `params.gr`; the bit-identity test must fail
with a `test result:` line present. Restore.

- [ ] **Step 6: commit**

```bash
git add -A && git commit -F - <<'EOF'
feat(gpu): white balance as a compute shader

Three multiplies per pixel, gains as uniforms computed CPU-side by
pipeline::white_balance_gains — so the shader does no colour maths, the result
is bit-identical, and a divergence means plumbing rather than float drift.

cpu_cost 4.0, the bottom of a measured 3.77-4.33 ms band at 3200x2136. The
bottom because a lower cost makes a run less likely to chain and the gate must
fail toward the CPU.
EOF
```

---

### Task 3: `GpuContrastStage`

Same shape. Shader body — ⚠ **per SAMPLE, not per pixel**, because `ContrastStage` uses
`map_samples`:

```wgsl
    let i = id.y * (params.row_groups * 64u) + id.x;
    if (i >= params.samples) { return; }
    data[i] = max({PIVOT}f + (data[i] - {PIVOT}f) * params.slope, 0.0);
```

⚠ `{PIVOT}` is **substituted** with `pipeline::CONTRAST_PIVOT` via `format!`, following
`luma_wgsl`. The shader is therefore a `fn shader() -> String`, not a `const`.

⚠ The dispatch is over **samples** (`pixels * 3`), so pass `img.pixels() * 3` and use
`Dispatch::grid` on that. Check `Dispatch::grid` accepts it —
`grep -n "fn grid" -A 12 crates/gpu-pipeline/src/dispatch.rs`.

`cpu_cost` **1.2** (band 0.97–1.97). Tests: bit-identity at `[+40.0, -40.0]` amounts; the name.
Sabotage: substitute `0.5` for the pivot → the bit-identity test must fail.

Commit: `feat(gpu): contrast as a compute shader`.

---

### Task 4: `GpuToneCurveStage`

- [ ] **The LUT is a storage buffer**, uploaded from `pipeline::ToneCurveStage::lut()`. See
`tone_regions.rs` for the existing LUT-upload client and copy its `storages` usage exactly.

Shader body, ⚠ **per sample**, and it must reproduce `sample()` including the clamp:

```wgsl
    let i = id.y * (params.row_groups * 64u) + id.x;
    if (i >= params.samples) { return; }
    // ⚠⚠ The input is CLAMPED to [0,1] before indexing, which pulls a scene-referred highlight
    // above 1.0 down to the curve's top entry. That is `ToneCurveStage::sample`'s behaviour, so
    // parity requires it — and it is the OPPOSITE of the rule ProfileHueSatMap's `v` and
    // ColorGradingStage's index follow, where the index clamps and the value keeps its headroom.
    // A shader that "fixed" it would be a different stage wearing the same name.
    let v = clamp(data[i], 0.0, 1.0);
    let pos = v * f32(params.last);
    let lo = u32(floor(pos));
    if (lo >= params.last) { data[i] = lut[params.last]; return; }
    let frac = pos - f32(lo);
    data[i] = lut[lo] + frac * (lut[lo + 1u] - lut[lo]);
```

`params.last` is `LUT_LEN - 1` = 1023, passed as a uniform rather than written as a literal.

`cpu_cost` **8.0** (band 8.07–9.27).

Tests:
- bit-identity on a curve with real curvature (e.g. `[[0,0],[0.25,0.35],[0.75,0.65],[1,1]]`) **and**
  on a two-point identity curve.
- ⚠ **`a_sample_above_one_takes_the_curves_top_entry`** — the fixture must carry values above 1.0
  or the clamp is untested. Assert the GPU and CPU agree *and* that the result equals `lut[1023]`.

Sabotage: drop the `clamp` → the above-one test must fail. Sabotage 2: rebuild the LUT in the GPU
stage instead of uploading the CPU's → should still pass (same spline), so **record it as inert**
rather than counting it, and note that what the upload buys is that the two *cannot* diverge if the
spline ever changes.

Commit: `feat(gpu): the tone curve as a compute shader`.

---

### Task 5: `GpuSaturationStage`

Shader, ⚠ **per pixel**, prefixed with `crate::luma_wgsl::luma_wgsl()`:

```wgsl
    let pixel = id.y * (params.row_groups * 64u) + id.x;
    if (pixel >= params.pixels) { return; }
    let p = pixel * 3u;
    let r = data[p]; let g = data[p + 1u]; let b = data[p + 2u];
    let luma = prophoto_luminance(r, g, b);
    let mx = max(r, max(g, b));
    let mn = min(r, min(g, b));
    let chroma = clamp((mx - mn) / max(mx, {EPS}f), 0.0, 1.0);
    let f = params.f_sat * (1.0 + params.vib * (1.0 - chroma));
    data[p] = max(luma + (r - luma) * f, 0.0);
    data[p + 1u] = max(luma + (g - luma) * f, 0.0);
    data[p + 2u] = max(luma + (b - luma) * f, 0.0);
```

⚠ `{EPS}` substituted from `pipeline::SATURATION_EPS`; the luma function comes from `luma_wgsl()`
so this stage cannot invent a second luminance.

`cpu_cost` **7.2** (band 7.19–9.39).

Tests:
- ⚠⚠ **The fixture must be COLOURED.** A neutral pixel has zero chroma and is untouched at any
  setting, so a grey fixture cannot see this stage at all — it would pass against a shader that
  did nothing. Assert up front that the CPU stage actually moves the fixture.
- Bit-identity across `(sat, vib)` of `(60, 0)`, `(-60, 0)`, `(0, 60)`, `(0, -60)` — both
  parameters, both directions.

Sabotage: use `max(mx, mn)` for the chroma denominator → bit-identity must fail. Sabotage 2: drop
the `.max(0.0)` → must fail on the negative samples in the fixture.

Commit: `feat(gpu): saturation as a compute shader`.

---

### Task 6: the decision table

⚠⚠ **M8.3h's lesson: state which decisions change and pin it, rather than believing the runs
merged.** In `accelerate.rs`'s tests:

```rust
    /// ⚠⚠ **What M8.3m actually buys, as a test.** `contrast` and `tone_curve` sit BETWEEN ported
    /// stages, so before this milestone a photo using them split the per-pixel tail into pieces
    /// too small to pay: `[exposure]` at 1.3 ms and `[tone_regions]` at 18.0 were each refused by
    /// the gate, stranding 19.3 ms of already-ported work on the CPU for no reason but a
    /// neighbour. This asserts the whole per-pixel head is now ONE run.
    #[test]
    fn the_per_pixel_head_is_one_run_once_every_member_is_ported() {
        let Some(ctx) = ctx() else { return };
        let mut e = Edits::default();
        e.white_balance.tint = 8.0;
        e.exposure = 0.4;
        e.contrast = 25.0;
        e.highlights = -30.0;
        e.tone_curve.points = vec![[0.0, 0.0], [0.3, 0.4], [1.0, 1.0]];
        e.saturation = 20.0;

        let before = tail_of(&e).stage_names();
        for s in ["white_balance", "contrast", "tone_curve", "saturation"] {
            assert!(before.contains(&s), "fixture check: {s} must be in the tail, got {before:?}");
        }
        let names = accel(
            tail_of(&e), &e, SCALE, ProfileRendering::default(), RenderSpec::default(), &ctx,
        ).stage_names();
        assert!(
            !names.iter().any(|n| ["white_balance", "exposure", "contrast", "tone_regions",
                                   "tone_curve", "saturation"].contains(n)),
            "every per-pixel head stage must have been absorbed into a chain, got {names:?}"
        );
    }
```

⚠ Check `Pipeline::stage_names` exists and returns `Vec<&'static str>` before writing this.

Commit: `test(gpu): pin what the four new rows change`.

---

### Task 7: measure, gate, record

- [ ] **Step 1: the bench**, three runs, and ⚠ **add a row that can see this** — the existing rows
use stages this catalogue does not have. Add to `bench_gpu_tail`:

```rust
    // ⚠⚠ **The row M8.3m exists for, and it is deliberately NOT a catalogue row.** 0 of 40
    // sidecars use white balance, contrast, the tone curve or saturation globally — that was
    // measured before M8.3k and is why plan 2 waited three milestones. What this row prices is
    // the *shape*: a photo whose per-pixel head is complete, where before this milestone
    // `[exposure]` and `[tone_regions]` were each refused by the gate and stranded on the CPU.
    let full_head = Edits { /* wb tint, exposure, contrast, tone regions, curve, saturation */ };
```

- [ ] **Step 2: the full gate**, each exit code from its own command

```bash
cargo fmt --all --check; echo "FMT=$?"
```
```bash
cargo clippy --workspace --all-targets --all-features -- -D warnings; echo "CLIPPY=$?"
```
```bash
cargo test --workspace > /tmp/m83m.log 2>&1; echo "TEST=$?"
```
⚠ Assert the count is **2358 + exactly the tests added**, not "still green". Confirm **63 goldens
ran and none moved** and **0 GPU skips**.

⚠ If the suite hangs with tests in libtest's "over 60 seconds" state at ~0% CPU, that is the Metal
adapter wedging — seen on 2026-09-13. Kill **only the test binary**, never a mid-build `cargo`
(which orphans rustc children and deadlocks the jobserver), and retry.

- [ ] **Step 3: record in `CLAUDE.md`** as **M8.3m**, above M8.3l's section. Must state:
  - ⚠⚠ **the honest headline: this does not move the catalogue**, and why it is worth doing anyway
    — the run merging, 9 of 10 mask sub-stages, and the M8.3l shared-upload unlock.
  - the measured bands and why each cost sits at the bottom of its band.
  - that `local_contrast` is now the **only** unported mask sub-stage.
  - Accepted caveats: nothing for this catalogue's globals; `local_contrast` still splits a mask;
    the vignette and creative look still split the tail; `gpu_tail` still off by default.
  - Update the **▶ NEXT ACTION** and **State at handoff** blocks.

- [ ] **Step 4: launch the GUI** and say what to look at.
