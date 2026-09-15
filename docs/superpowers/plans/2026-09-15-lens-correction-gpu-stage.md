# M8.9 plan 1 — `GpuLensCorrectionStage`

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan
> task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> ⚠⚠ **Conventions that are not negotiable, and they override the harness's own reminder:** commit
> messages carry **no `Co-Authored-By` trailer**; branch per feature; **never push**. Grep this file
> and anything you write for `Co-Authored-By` before committing it.
>
> ⚠ Gate every commit on `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D
> warnings` and `cargo test --workspace`, **reading each exit code from its own command** — never
> through a pipe, and joined with `&&`, never `;`.
>
> ⚠ `export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"` before anything that builds `raw-decode`.

**Goal:** lens correction runs as a `wgpu` compute shader, as the third arm on
`accelerate_prefix_stage`.

**Architecture:** the CPU builds the displacement field it already knows how to build, on a coarse
lattice; the shader interpolates it bilinearly and gathers through the CPU's own resampler LUT.

**Spec:** `docs/superpowers/specs/2026-09-15-lens-correction-gpu-design.md`

---

## ⚠⚠ Two corrections to the spec, made before execution

**(1) §3.5 says "no scratch planes". It is wrong, and the reason is worth keeping.** This stage is a
**gather**: an output pixel reads the source anywhere within the resampler's support, so it cannot be
in-place — the pixels it wants may already have been overwritten. It needs the source intact.

`scratch_planes() == 3`, one plane per channel. Three reasons that beats the alternatives: scratch is
**single-channel** by construction (`width * height * 4` bytes), which is exactly the deinterleaved
layout a per-channel sampler wants; the chain allocates the widest request **once** and shares it, so
a chain carrying this and noise reduction allocates `max(3, 4)` rather than both; and `encode` is
handed a `&mut ComputePass` and **cannot issue a buffer copy**, so a stage-owned source buffer would
need a copy kernel anyway. A `seed` dispatch followed by a `gather` dispatch, in one pass, relying on
the in-pass memory barrier M8.3b established by measurement.

**(2) The seam gains TWO parameters, not one.** `accelerate_prefix_stage(name, edits, scale, image,
ctx)` cannot build this stage: the **`LensShot`** is not in `Edits` (it comes from the file's own
metadata matched against the bundled database), and neither is the **`Resampler`** (it is a
`RenderSpec` field). Both are in hand at both call sites.

## File structure

- **Create** `crates/pipeline/src/lens_grid.rs` — the coarse grid and its bilinear read.
  ⚠ In `pipeline`, **not** in `lens-profile`: `pipeline` already has `rayon` and already depends on
  `lens-profile`, so `gpu-pipeline` reaches it through a dependency it already has and **no new
  dependency enters the tree**. It also keeps `lens-profile/src/lib.rs` — **949 lines, already past
  the ~600 marker** — from growing.
- **Create** `crates/gpu-pipeline/src/lens_correction.rs` — the stage and its shader.
- **Modify** `crates/pipeline/src/lib.rs` — `pub mod lens_grid;`
- **Modify** `crates/pipeline/src/resample.rs` — `Weights::lut()` / `taps()` / `phases()`.
- **Modify** `crates/gpu-pipeline/src/lib.rs` — `mod lens_correction;` + the re-export.
- **Modify** `crates/gpu-pipeline/src/accelerate.rs` — the third arm, two new parameters.
- **Modify** `crates/gpu-pipeline/Cargo.toml` — `lens-profile`.
- **Modify** `crates/app/src/render_engine.rs` — the call site.

---

### Task 1: the coarse displacement grid

**Files:**
- Create: `crates/pipeline/src/lens_grid.rs`
- Modify: `crates/pipeline/src/lib.rs`

- [ ] **Step 1: write the module**

```rust
//! The lens correction's displacement field, sampled on a coarse lattice.
//!
//! ⚠⚠ **The milestone's central mechanism, and it was settled by measurement rather than by
//! argument.** The alternative was reimplementing lensfun's distortion model in WGSL — a second
//! definition of the correction that can disagree with [`Corrector::source`], made harder by
//! `Modifier`'s `norm_scale`/`center_x`/`center_y` being private. Instead the CPU builds the field
//! it already knows how to build and the shader interpolates between grid points.
//!
//! Measured on a 6246x4170 frame with the real XF16-80 profile:
//!
//! | step | points | MiB | build | worst source error | worst gain error |
//! |---|---|---|---|---|---|
//! | per-pixel | 26.0 M | 695 | 38.3 ms | 0 | 0 |
//! | 8 | 408,146 | 10.9 | 2.6 ms | 0.0007 px | 0.000000 |
//! | **16** | **102,704** | **2.74** | **1.4 ms** | **0.0027 px** | **0.000000** |
//! | 32 | 25,905 | 0.69 | 0.8 ms | 0.0111 px | 0.000000 |
//! | 64 | 6,536 | 0.17 | 0.5 ms | 0.0348 px | 0.000000 |
//!
//! ⚠ And a second, unlooked-for win: building at step 16 costs **1.4 ms against the full field's
//! 38.3**, because it calls into the corrector only every 16th row.
//!
//! ⚠⚠ **It stores DISPLACEMENT, and the reason is PRECISION rather than interpolability.** Bilinear
//! interpolation is exact for the linear part of the field either way — a grid of absolute source
//! positions interpolates just as well. What differs is f32 resolution: a coordinate near 6246
//! resolves to ~0.0005 px, where a displacement near 50 resolves to ~4e-6. Subtracting the identity
//! leaves a small residual that keeps far more of its mantissa.

use lens_profile::Corrector;
use rayon::prelude::*;

/// Values per grid point: `[dxR, dyR, dxG, dyG, dxB, dyB, gain]`.
///
/// ⚠ The vignetting gain rides the **same** grid as a seventh component rather than getting one of
/// its own. Measured exact to six decimals at every step above: the gain field is far smoother than
/// the displacement field, so nothing is given up and there is one lattice rather than two that
/// could come to disagree about their own geometry.
pub const STRIDE: usize = 7;

/// The correction's field on a coarse lattice, in **displacements** from the pixel's own position.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplacementGrid {
    cols: usize,
    rows: usize,
    step: usize,
    data: Vec<f32>,
}

impl DisplacementGrid {
    /// Sample `corrector`'s field every `step` pixels, covering `w x h`.
    ///
    /// ⚠ `div_ceil(step) + 1` so the lattice always **contains** the frame: the last column sits at
    /// or past `w - 1`, which is what lets [`Self::at`] clamp its cell index without ever
    /// extrapolating.
    pub fn build(corrector: &Corrector, w: usize, h: usize, step: usize) -> Self {
        let step = step.max(1);
        let cols = w.div_ceil(step) + 1;
        let rows = h.div_ceil(step) + 1;
        let mut data = vec![0.0f32; cols * rows * STRIDE];
        data.par_chunks_exact_mut(cols * STRIDE)
            .enumerate()
            .for_each(|(gy, row)| {
                let y = (gy * step) as f32;
                for (gx, cell) in row.as_chunks_mut::<STRIDE>().0.iter_mut().enumerate() {
                    let x = (gx * step) as f32;
                    let s = corrector.source(x, y);
                    for ch in 0..3 {
                        cell[2 * ch] = s[ch][0] - x;
                        cell[2 * ch + 1] = s[ch][1] - y;
                    }
                    cell[6] = corrector.gain(x, y);
                }
            });
        Self {
            cols,
            rows,
            step,
            data,
        }
    }

    pub fn cols(&self) -> usize {
        self.cols
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn step(&self) -> usize {
        self.step
    }

    /// The raw lattice, row-major, [`STRIDE`] values per point — what the shader binds.
    pub fn data(&self) -> &[f32] {
        &self.data
    }

    /// Bilinearly interpolate the seven values at a pixel.
    ///
    /// ⚠ **The CPU twin of the shader's `grid_at`, and it exists so the accuracy sweep can run with
    /// no GPU at all.** Two implementations that must agree is how they come to disagree — so the
    /// shader is written from this, and the equivalence test is what holds them together.
    pub fn at(&self, x: f32, y: f32) -> [f32; STRIDE] {
        let fx = x / self.step as f32;
        let fy = y / self.step as f32;
        let gx = (fx.floor().max(0.0) as usize).min(self.cols - 2);
        let gy = (fy.floor().max(0.0) as usize).min(self.rows - 2);
        let tx = fx - gx as f32;
        let ty = fy - gy as f32;
        let idx = |cx: usize, cy: usize| (cy * self.cols + cx) * STRIDE;
        let (a, b, c, d) = (
            idx(gx, gy),
            idx(gx + 1, gy),
            idx(gx, gy + 1),
            idx(gx + 1, gy + 1),
        );
        let mut out = [0.0f32; STRIDE];
        for (k, o) in out.iter_mut().enumerate() {
            let top = self.data[a + k] * (1.0 - tx) + self.data[b + k] * tx;
            let bot = self.data[c + k] * (1.0 - tx) + self.data[d + k] * tx;
            *o = top * (1.0 - ty) + bot * ty;
        }
        out
    }

    /// The source point for channel `ch` at `(x, y)`, reconstructed from the displacement.
    pub fn source(&self, x: f32, y: f32, ch: usize) -> [f32; 2] {
        let v = self.at(x, y);
        [x + v[2 * ch], y + v[2 * ch + 1]]
    }

    /// The vignetting gain at `(x, y)`.
    pub fn gain(&self, x: f32, y: f32) -> f32 {
        self.at(x, y)[6]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lens_profile::{Catalogue, Corrections, LensShot, Shot};

    /// A synthetic lens with coefficients chosen here rather than read from the bundled database —
    /// the seam that makes lens correction testable in CI at all. Mirrors the stage's own fixture.
    fn shot() -> LensShot {
        let xml = r#"<lensdatabase version="1">
            <camera><maker>TestMaker</maker><model>TestCam</model>
                <mount>TestMount</mount><cropfactor>1.0</cropfactor></camera>
            <lens><maker>TestMaker</maker><model>TestLens 50mm</model>
                <mount>TestMount</mount><cropfactor>1.0</cropfactor>
                <calibration>
                    <distortion model="ptlens" focal="50" a="0.0" b="-0.05" c="0.0"/>
                    <tca model="poly3" focal="50" vr="1.004" vb="0.996"/>
                </calibration></lens>
        </lensdatabase>"#;
        let db = Catalogue::from_xml(xml).unwrap();
        let profile = db.find("TestMaker", "TestCam", "TestLens 50mm").unwrap();
        LensShot {
            profile,
            shot: Shot {
                focal: 50.0,
                aperture: 2.8,
            },
        }
    }

    fn corrector(w: usize, h: usize) -> Corrector {
        let lens = shot();
        Corrector::new(
            &lens.profile,
            &lens.shot,
            w,
            h,
            Corrections {
                distortion: true,
                tca: true,
                vignetting: true,
                scale: 0.0,
            },
        )
        .expect("fixture check: the synthetic profile must produce a corrector")
    }

    /// ⚠⚠ **The measurement the whole design rests on, as a test rather than a one-off.** Without
    /// it the grid step is a constant nobody can defend — and this crate has shipped two constants
    /// reasoned about instead of swept (`MIN_GPU_BOX_PX` low by ~8x, `MAX_TILE_DABS` past an 8.7x
    /// cliff). The bound is deliberately generous against the measured 0.0027 px at step 16 on a
    /// real lens; what it pins is the **shape** — error grows with step, and stays sub-pixel.
    #[test]
    fn the_grid_tracks_the_corrector_it_samples() {
        let (w, h) = (400, 300);
        let c = corrector(w, h);
        let mut prev = 0.0f32;
        for step in [8usize, 16, 32] {
            let g = DisplacementGrid::build(&c, w, h, step);
            let mut worst_src = 0.0f32;
            let mut worst_gain = 0.0f32;
            for y in 0..h {
                for x in 0..w {
                    let (xf, yf) = (x as f32, y as f32);
                    let truth = c.source(xf, yf);
                    for ch in 0..3 {
                        let got = g.source(xf, yf, ch);
                        worst_src = worst_src
                            .max((got[0] - truth[ch][0]).abs())
                            .max((got[1] - truth[ch][1]).abs());
                    }
                    worst_gain = worst_gain.max((g.gain(xf, yf) - c.gain(xf, yf)).abs());
                }
            }
            assert!(
                worst_src < 0.25,
                "step {step}: worst source error {worst_src} px is not sub-pixel"
            );
            assert!(
                worst_gain < 1e-4,
                "step {step}: worst gain error {worst_gain} is not negligible"
            );
            assert!(
                worst_src >= prev,
                "step {step}: error {worst_src} fell below step {}'s {prev} — \
                 a coarser lattice cannot be more accurate, so the sweep is measuring nothing"
            );
            prev = worst_src;
        }
    }

    /// ⚠ At a grid point the interpolation must be **exact**, not merely close: `tx` and `ty` are
    /// both 0 there, so any disagreement is a wrong index rather than a rounding.
    #[test]
    fn a_grid_point_reproduces_the_corrector_exactly() {
        let (w, h) = (400, 300);
        let c = corrector(w, h);
        let g = DisplacementGrid::build(&c, w, h, 16);
        for (x, y) in [(0.0f32, 0.0f32), (16.0, 32.0), (112.0, 208.0)] {
            let truth = c.source(x, y);
            for ch in 0..3 {
                let got = g.source(x, y, ch);
                assert!(
                    (got[0] - truth[ch][0]).abs() < 1e-3 && (got[1] - truth[ch][1]).abs() < 1e-3,
                    "grid point ({x}, {y}) ch {ch}: {got:?} against {:?}",
                    truth[ch]
                );
            }
        }
    }

    /// ⚠ The lattice must **contain** the frame, or the bottom-right pixels extrapolate off a cell
    /// that does not exist. A `div_ceil` without the `+ 1` passes every interior probe and fails
    /// exactly here.
    #[test]
    fn the_lattice_contains_the_frame() {
        for (w, h, step) in [(400usize, 300usize, 16usize), (401, 301, 16), (17, 3, 16)] {
            let g = DisplacementGrid::build(&corrector(w, h), w, h, step);
            assert!(
                (g.cols() - 1) * step >= w - 1 && (g.rows() - 1) * step >= h - 1,
                "{w}x{h} step {step}: lattice {}x{} does not reach the far edge",
                g.cols(),
                g.rows()
            );
        }
    }
}
```

- [ ] **Step 2: declare the module**

In `crates/pipeline/src/lib.rs`, beside the other `pub mod` declarations:

```rust
pub mod lens_grid;
```

- [ ] **Step 3: run the tests**

```bash
cargo test -p pipeline lens_grid
```

Expected: 3 passed.

- [ ] **Step 4: sabotage — drop the `+ 1` from `rows`**

Change `let rows = h.div_ceil(step) + 1;` to `let rows = h.div_ceil(step);` and re-run. Expected:
`the_lattice_contains_the_frame` fails (and `the_grid_tracks_the_corrector_it_samples` may panic on
an index). **Restore from a copy taken first — never `git checkout`:**

```bash
cp crates/pipeline/src/lens_grid.rs /tmp/rr-lens-grid.bak
# ... sabotage, run, then:
cp /tmp/rr-lens-grid.bak crates/pipeline/src/lens_grid.rs
```

- [ ] **Step 5: sabotage — store the absolute position rather than the displacement**

Change `cell[2 * ch] = s[ch][0] - x;` to `cell[2 * ch] = s[ch][0];` (and the `y` line likewise).
Expected: `a_grid_point_reproduces_the_corrector_exactly` fails, because `source` then adds `x`
twice. Restore from the copy.

- [ ] **Step 6: gate and commit**

```bash
cargo fmt --all
cargo fmt --all -- --check; echo "FMT=$?"
cargo clippy --all-targets --all-features -- -D warnings; echo "CLIPPY=$?"
cargo test --workspace; echo "TEST=$?"
```

All three must read 0. Then:

```bash
git add crates/pipeline/src/lens_grid.rs crates/pipeline/src/lib.rs
git commit -m "feat(pipeline): the lens correction's field on a coarse displacement grid"
```

---

### Task 2: expose the resampler's own LUT

**Files:**
- Modify: `crates/pipeline/src/resample.rs`

⚠⚠ **Substituted, never transcribed** — the rule `ToneCurveStage::lut` and `ColorGradingStage::lut`
already follow. A shader that rebuilt the kernel would be a second definition of a curve nobody
re-derives, and the two would disagree invisibly.

- [ ] **Step 1: write the failing test**

Append to `resample.rs`'s `mod tests`:

```rust
/// ⚠⚠ **`Resampler::Bilinear`'s table is deliberately EMPTY, and a consumer that does not branch
/// on it diverges on exactly one kernel while agreeing on every other.** The module's own comment
/// says why: the table quantises the phase, and computing bilinear exactly is both cheaper and
/// more accurate — `1.0 - (1.0 - f)` is not `f` in f32. Anything reading `lut()` must handle the
/// empty case, which is why this is asserted rather than left to be discovered.
#[test]
fn the_exposed_table_is_empty_for_bilinear_and_full_for_the_rest() {
    for k in [
        Resampler::Bilinear,
        Resampler::Mitchell,
        Resampler::CatmullRom,
        Resampler::Lanczos3,
    ] {
        let w = Weights::new(k);
        assert_eq!(w.taps(), k.taps(), "{k:?}: taps disagree");
        if k == Resampler::Bilinear {
            assert!(w.lut().is_empty(), "{k:?}: the table must stay empty");
        } else {
            assert_eq!(
                w.lut().len(),
                w.phases() * w.taps(),
                "{k:?}: the table is not phases x taps"
            );
        }
    }
}

/// ⚠ The exposed table must be the one `axis` reads, not a rebuild of it — otherwise a consumer
/// uploading it would resample with a kernel the CPU does not use.
#[test]
fn the_exposed_table_is_the_one_axis_reads() {
    let w = Weights::new(Resampler::CatmullRom);
    let mut got = [0.0f32; MAX_TAPS];
    for p in [0usize, 1, 511, 1023] {
        let f = p as f32 / w.phases() as f32;
        w.axis(f, &mut got);
        let row = &w.lut()[p * w.taps()..(p + 1) * w.taps()];
        assert_eq!(&got[..w.taps()], row, "phase {p} disagrees with the table");
    }
}
```

- [ ] **Step 2: run it to watch it fail**

```bash
cargo test -p pipeline the_exposed_table
```

Expected: FAIL — `no method named 'lut'`.

- [ ] **Step 3: add the accessors**

In `impl Weights`, beside `kernel()`:

```rust
    /// Taps per axis.
    pub fn taps(&self) -> usize {
        self.taps
    }

    /// How many phases the table resolves; the index is `min(f * phases, phases - 1)`.
    pub fn phases(&self) -> usize {
        LUT_PHASES
    }

    /// The table `axis` reads, row-major, `phases() * taps()` entries.
    ///
    /// ⚠⚠ **Empty for [`Resampler::Bilinear`]**, which is computed exactly — see the field's own
    /// comment. A consumer that does not branch on that diverges on one kernel and agrees on every
    /// other, which is the worst shape a divergence can have.
    ///
    /// ⚠ Exposed so a GPU twin can upload the CPU's own weights rather than rebuild them:
    /// substituted, never transcribed, the rule `ToneCurveStage::lut` already follows.
    pub fn lut(&self) -> &[f32] {
        &self.lut
    }
```

- [ ] **Step 4: run the tests**

```bash
cargo test -p pipeline the_exposed_table
```

Expected: 2 passed.

- [ ] **Step 5: gate and commit**

```bash
cargo fmt --all
cargo fmt --all -- --check; echo "FMT=$?"
cargo clippy --all-targets --all-features -- -D warnings; echo "CLIPPY=$?"
cargo test --workspace; echo "TEST=$?"
git add crates/pipeline/src/resample.rs
git commit -m "feat(pipeline): expose the resampler's own weight table, bilinear's emptiness included"
```

---

### Task 3: `GpuLensCorrectionStage`

**Files:**
- Create: `crates/gpu-pipeline/src/lens_correction.rs`
- Modify: `crates/gpu-pipeline/src/lib.rs`, `crates/gpu-pipeline/Cargo.toml`

- [ ] **Step 1: add the dependency**

In `crates/gpu-pipeline/Cargo.toml`, under `[dependencies]`:

```toml
lens-profile = { path = "../lens-profile" }
```

⚠ No new crate enters the tree: `pipeline` already depends on it, and it is pure Rust with one
dependency of its own.

- [ ] **Step 2: write the stage**

```rust
//! Lens correction as a compute shader — the third stage to move outside the tail, and the first
//! one aimed at **export** rather than at the preview.
//!
//! ⚠⚠ **The field travels as a coarse displacement grid rather than as a WGSL reimplementation of
//! lensfun's distortion model.** See `pipeline::lens_grid` for the measurement that settled it. The
//! shader interpolates that lattice and gathers through the CPU's own resampler weights, so neither
//! the correction nor the kernel has a second definition here.
//!
//! ⚠⚠ **THREE SCRATCH PLANES, because a gather cannot be in place.** Every stage before this one
//! touched only its own pixel, so the chain's one-image contract held. An output pixel here reads
//! the source anywhere within the resampler's support, and those pixels may already have been
//! overwritten — so `seed` copies the image into the scratch planes and `gather` reads from them.
//! Single-channel planes are exactly the deinterleaved layout a per-channel sampler wants, and the
//! chain allocates the widest request once and shares it.
//!
//! ⚠ Two dispatches in one pass, relying on the in-pass memory barrier M8.3b **measured** rather
//! than read: three chained exposures equalled one, and gains multiply, so a missing barrier would
//! have given a wrong product rather than a plausible image.

use crate::dispatch::Dispatch;
use crate::{GpuContext, GpuImage, GpuStage};
use pipeline::lens_grid::{DisplacementGrid, STRIDE};
use pipeline::{PipelineStage, Resampler};
use std::sync::Arc;

const WORKGROUP: u32 = 64;

/// How many pixels apart the displacement field is sampled.
///
/// ⚠⚠ **Swept against OUTPUT 8-bit levels, never against the field's own pixel error** — see
/// `the_grid_step_is_invisible_in_the_output`. 0.0027 px of source error on a hard edge is roughly
/// 0.7 of a level; 0.0348 px would be ~9. Two constants in this crate were reasoned about instead
/// of swept (`MIN_GPU_BOX_PX` low by ~8x, `MAX_TILE_DABS` sitting past an 8.7x occupancy cliff),
/// which is why this one has a test that can move it.
pub const GRID_STEP: usize = 16;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    w: u32,
    h: u32,
    pixels: u32,
    row_groups: u32,
    cols: u32,
    rows: u32,
    step: f32,
    pad: f32,
}

pub struct GpuLensCorrectionStage {
    cpu: pipeline::LensCorrectionStage,
    grid: DisplacementGrid,
    weights: Vec<f32>,
    resampler: Resampler,
    w: usize,
    h: usize,
    ctx: Arc<GpuContext>,
}

impl GpuLensCorrectionStage {
    /// Build the GPU twin, or `None` when the profile yields no corrector.
    ///
    /// ⚠ The corrector is built from **this image's** dimensions, exactly as the CPU stage builds
    /// its own: a corrector is tied to a size, and the preview and the export render at different
    /// ones through the same `Edits`.
    pub fn new(
        lens: &lens_profile::LensShot,
        want: lens_profile::Corrections,
        resampler: Resampler,
        image: &core_image::ImageBuf,
        ctx: &Arc<GpuContext>,
    ) -> Option<Self> {
        let (w, h) = (image.width(), image.height());
        let corrector = lens_profile::Corrector::new(&lens.profile, &lens.shot, w, h, want)?;
        let grid = DisplacementGrid::build(&corrector, w, h, GRID_STEP);
        let weights = pipeline::Weights::new(resampler).lut().to_vec();
        Some(Self {
            cpu: pipeline::LensCorrectionStage::new(lens.clone(), want, resampler),
            grid,
            weights,
            resampler,
            w,
            h,
            ctx: ctx.clone(),
        })
    }

    /// The shader, with the tap count, the radius and the bilinear branch baked in.
    ///
    /// ⚠ Baked rather than uniform, for `box_wgsl`'s reason: the shader text is the program
    /// cache's key, and these change only with the resampler setting — a handful of values — where
    /// a literal lets the compiler see a constant trip count.
    fn shader(&self) -> String {
        let taps = self.resampler.taps();
        let radius = self.resampler.radius() as i32;
        // ⚠⚠ **The bilinear branch is not an optimisation.** `Weights`' table is deliberately empty
        // for that kernel because it is computed exactly; reading a table that does not exist would
        // resample with zeros, and materialising one would quantise the phase and diverge from the
        // CPU on the one kernel every other path agrees about.
        let axis_body = if self.resampler == Resampler::Bilinear {
            "    w[0] = 1.0 - f;\n    w[1] = f;".to_string()
        } else {
            format!(
                "    let p = min(u32(f * {phases}.0), {last}u);\n\
                 \x20   for (var i = 0u; i < {taps}u; i = i + 1u) {{ w[i] = lut[p * {taps}u + i]; }}",
                phases = pipeline::Weights::new(self.resampler).phases(),
                last = pipeline::Weights::new(self.resampler).phases() - 1,
            )
        };
        format!(
            r#"
struct Params {{
    w: u32,
    h: u32,
    pixels: u32,
    row_groups: u32,
    cols: u32,
    rows: u32,
    step: f32,
    pad: f32,
}};
@group(0) @binding(0) var<storage, read_write> img: array<f32>;
@group(0) @binding(1) var<uniform> params: Params;
@group(0) @binding(2) var<storage, read_write> p0: array<f32>;
@group(0) @binding(3) var<storage, read_write> p1: array<f32>;
@group(0) @binding(4) var<storage, read_write> p2: array<f32>;
@group(0) @binding(5) var<storage, read> grid: array<f32>;
@group(0) @binding(6) var<storage, read> lut: array<f32>;

fn index(id: vec3<u32>) -> u32 {{
    return id.y * (params.row_groups * 64u) + id.x;
}}

// The source planes, so the gather never reads a pixel it has already overwritten.
@compute @workgroup_size(64)
fn seed(@builtin(global_invocation_id) id: vec3<u32>) {{
    let i = index(id);
    if (i >= params.pixels) {{ return; }}
    let q = i * 3u;
    p0[i] = img[q];
    p1[i] = img[q + 1u];
    p2[i] = img[q + 2u];
}}

// The CPU twin of this is `DisplacementGrid::at`; the two are held together by the equivalence test.
fn grid_at(x: f32, y: f32, k: u32) -> f32 {{
    let fx = x / params.step;
    let fy = y / params.step;
    let gx = min(u32(max(floor(fx), 0.0)), params.cols - 2u);
    let gy = min(u32(max(floor(fy), 0.0)), params.rows - 2u);
    let tx = fx - f32(gx);
    let ty = fy - f32(gy);
    let a = (gy * params.cols + gx) * {stride}u + k;
    let c = ((gy + 1u) * params.cols + gx) * {stride}u + k;
    let top = grid[a] * (1.0 - tx) + grid[a + {stride}u] * tx;
    let bot = grid[c] * (1.0 - tx) + grid[c + {stride}u] * tx;
    return top * (1.0 - ty) + bot * ty;
}}

fn axis(f: f32) -> array<f32, {taps}u> {{
    var w: array<f32, {taps}u>;
{axis_body}
    return w;
}}

// ⚠⚠ A tap outside the frame contributes ZERO and the weights are NOT renormalised — which is what
// `resample::sample_channel` does, and the only thing it can do here: renormalising would brighten
// the border relative to the CPU.
fn sample_ch(ch: u32, sx: f32, sy: f32) -> f32 {{
    let bx = floor(sx);
    let by = floor(sy);
    let x0 = i32(bx) - {radius} + 1;
    let y0 = i32(by) - {radius} + 1;
    let wx = axis(sx - bx);
    let wy = axis(sy - by);
    var acc = 0.0;
    for (var j = 0u; j < {taps}u; j = j + 1u) {{
        let yi = y0 + i32(j);
        if (yi < 0 || yi >= i32(params.h)) {{ continue; }}
        var row = 0.0;
        for (var i = 0u; i < {taps}u; i = i + 1u) {{
            let xi = x0 + i32(i);
            if (xi < 0 || xi >= i32(params.w)) {{ continue; }}
            let o = u32(yi) * params.w + u32(xi);
            var v = 0.0;
            if (ch == 0u) {{ v = p0[o]; }}
            else if (ch == 1u) {{ v = p1[o]; }}
            else {{ v = p2[o]; }}
            row = row + wx[i] * v;
        }}
        acc = acc + wy[j] * row;
    }}
    return acc;
}}

@compute @workgroup_size(64)
fn gather(@builtin(global_invocation_id) id: vec3<u32>) {{
    let i = index(id);
    if (i >= params.pixels) {{ return; }}
    let x = f32(i % params.w);
    let y = f32(i / params.w);
    let gain = grid_at(x, y, 6u);
    let q = i * 3u;
    img[q] = sample_ch(0u, x + grid_at(x, y, 0u), y + grid_at(x, y, 1u)) * gain;
    img[q + 1u] = sample_ch(1u, x + grid_at(x, y, 2u), y + grid_at(x, y, 3u)) * gain;
    img[q + 2u] = sample_ch(2u, x + grid_at(x, y, 4u), y + grid_at(x, y, 5u)) * gain;
}}
"#,
            stride = STRIDE,
            taps = taps,
            radius = radius,
            axis_body = axis_body,
        )
    }
}

impl GpuStage for GpuLensCorrectionStage {
    fn name(&self) -> &'static str {
        "lens_correction"
    }

    /// ⚠ Measured by `probe_lens`, quoted at the **bottom** of its band — the conservative
    /// direction, because a lower number makes a run less likely to chain and the gate must fail
    /// toward the CPU. Filled in by Task 6; the placeholder below is the measured 6.5 Mpx figure
    /// scaled to 6.8.
    fn cpu_cost(&self) -> f32 {
        169.0
    }

    fn scratch_planes(&self) -> usize {
        3
    }

    fn encode(
        &self,
        ctx: &GpuContext,
        pass: &mut wgpu::ComputePass,
        img: &GpuImage,
        scratch: &[wgpu::Buffer],
    ) {
        let pixels = self.w * self.h;
        let groups = (pixels as u32).div_ceil(WORKGROUP);
        let row_groups = groups.min(crate::dispatch::MAX_GROUPS_PER_DIM);
        let params = Params {
            w: self.w as u32,
            h: self.h as u32,
            pixels: pixels as u32,
            row_groups,
            cols: self.grid.cols() as u32,
            rows: self.grid.rows() as u32,
            step: self.grid.step() as f32,
            pad: 0.0,
        };
        // ⚠ wgpu refuses a zero-sized buffer, and bilinear's table is legitimately empty — so a
        // 4-byte placeholder is bound and the shader's bilinear branch never reads it.
        let lut: &[f32] = if self.weights.is_empty() {
            &[0.0]
        } else {
            &self.weights
        };
        let mut d = Dispatch::new(ctx, &self.ctx, self.shader(), img);
        d.uniform(bytemuck::bytes_of(&params));
        d.plane(&scratch[0]);
        d.plane(&scratch[1]);
        d.plane(&scratch[2]);
        d.storage(bytemuck::cast_slice(self.grid.data()));
        d.storage(bytemuck::cast_slice(lut));
        d.run(pass, "seed", groups, row_groups);
        d.run(pass, "gather", groups, row_groups);
    }

    fn cpu(&self) -> &dyn PipelineStage {
        &self.cpu
    }
}
```

⚠⚠ **`Dispatch`'s real API may differ from the sketch above.** Read
`crates/gpu-pipeline/src/dispatch.rs` and `noise_reduction.rs`'s `encode` and follow **those**
exactly — `noise_reduction` is the closest shape (a uniform plus four planes plus several entry
points in one pass). Adapt the binding helpers, the grid/row-group arithmetic and the entry-point
invocation to whatever that module actually exposes; do not invent methods. If `Dispatch` has no
read-only storage helper, add one beside its plane helper rather than building a bind group by hand
in this file.

⚠ `pipeline::LensCorrectionStage` and `pipeline::Weights` must be reachable from `pipeline`'s root.
Check `crates/pipeline/src/lib.rs`'s re-exports and add them if missing — additive, no call-site
churn.

- [ ] **Step 3: declare the module and re-export**

In `crates/gpu-pipeline/src/lib.rs`, beside `mod local_contrast;`:

```rust
mod lens_correction;
```

and beside the other re-exports:

```rust
pub use lens_correction::GpuLensCorrectionStage;
```

- [ ] **Step 4: build**

```bash
cargo clippy -p gpu-pipeline --all-targets -- -D warnings; echo "CLIPPY=$?"
```

Expected: 0. Fix any WGSL or API mismatch here rather than at test time.

- [ ] **Step 5: write the equivalence tests**

Append to `lens_correction.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use core_image::ImageBuf;
    use lens_profile::{Catalogue, Corrections, LensShot, Shot};

    fn ctx() -> Option<Arc<GpuContext>> {
        match crate::shared() {
            Some(c) => Some(c.clone()),
            None => {
                eprintln!("SKIPPED: no GPU adapter on this machine");
                None
            }
        }
    }

    /// The synthetic lens the CPU stage's own tests use, so the two are comparable.
    ///
    /// ⚠ `b` is **negative** — a barrel correction — deliberately: that is the sign whose extreme
    /// sits at an edge midpoint rather than at a corner, and the one the CPU stage's region tests
    /// established as the harder case.
    fn shot() -> LensShot {
        let xml = r#"<lensdatabase version="1">
            <camera><maker>TestMaker</maker><model>TestCam</model>
                <mount>TestMount</mount><cropfactor>1.0</cropfactor></camera>
            <lens><maker>TestMaker</maker><model>TestLens 50mm</model>
                <mount>TestMount</mount><cropfactor>1.0</cropfactor>
                <calibration>
                    <distortion model="ptlens" focal="50" a="0.0" b="-0.05" c="0.0"/>
                    <tca model="poly3" focal="50" vr="1.004" vb="0.996"/>
                </calibration></lens>
        </lensdatabase>"#;
        let db = Catalogue::from_xml(xml).unwrap();
        LensShot {
            profile: db.find("TestMaker", "TestCam", "TestLens 50mm").unwrap(),
            shot: Shot {
                focal: 50.0,
                aperture: 2.8,
            },
        }
    }

    fn want() -> Corrections {
        Corrections {
            distortion: true,
            tca: true,
            vignetting: true,
            scale: 0.0,
        }
    }

    /// ⚠⚠ **A hard grid, not a smooth ramp** — the CPU stage's own fixture, and for its reason:
    /// every kernel reproduces a linear ramp almost exactly, so a ramp cannot tell two samplers
    /// apart and a sub-pixel source error would be invisible on one.
    fn grid_chart(w: usize, h: usize) -> ImageBuf {
        let mut img = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let v = if x % 8 < 2 || y % 8 < 2 { 0.9 } else { 0.1 };
                img.set_pixel(x, y, [v, v * 0.6, v * 0.3]);
            }
        }
        img
    }

    fn on_gpu(ctx: &Arc<GpuContext>, r: Resampler, img: ImageBuf) -> ImageBuf {
        let stage = GpuLensCorrectionStage::new(&shot(), want(), r, &img, ctx)
            .expect("fixture check: the synthetic profile must produce a corrector");
        crate::GpuChain::new(ctx.clone(), vec![Box::new(stage)]).apply(img)
    }

    fn on_cpu(r: Resampler, img: ImageBuf) -> ImageBuf {
        pipeline::LensCorrectionStage::new(shot(), want(), r).apply(img)
    }

    /// ⚠⚠ **Without this every assertion below is vacuous** — a stage that did nothing would match
    /// a CPU stage that also did nothing. The W5 failure mode.
    #[test]
    fn the_fixture_is_one_this_stage_actually_moves() {
        let img = grid_chart(200, 160);
        let out = on_cpu(Resampler::CatmullRom, img.clone());
        let moved = img
            .data()
            .iter()
            .zip(out.data())
            .filter(|(a, b)| a != b)
            .count();
        assert!(
            moved > img.data().len() / 2,
            "fixture check: the correction must move most of the fixture, moved {moved}"
        );
    }

    /// The milestone's acceptance test: the GPU renders the photograph the CPU does.
    ///
    /// ⚠ Every kernel, because **bilinear takes a different branch** — its table is empty by
    /// design, so a shader that only read the table would agree on three kernels and produce
    /// black on the fourth.
    #[test]
    fn every_kernel_renders_the_same_picture_as_the_cpu() {
        let Some(ctx) = ctx() else { return };
        for r in [
            Resampler::Bilinear,
            Resampler::Mitchell,
            Resampler::CatmullRom,
            Resampler::Lanczos3,
        ] {
            let img = grid_chart(200, 160);
            let d = crate::equivalence::compare(&on_cpu(r, img.clone()), &on_gpu(&ctx, r, img));
            assert!(
                d.worst_levels < 1.5,
                "{r:?}: worst {} levels over {:.3}% of samples",
                d.worst_levels,
                d.differing_percent()
            );
        }
    }

    /// ⚠⚠ **The grid step is judged in OUTPUT LEVELS, which is the only judgement that matters.**
    /// A source error in pixels says nothing on its own: 0.0027 px across a hard edge is a
    /// fraction of a level, where 0.0348 px is several. This is what makes `GRID_STEP` a swept
    /// constant rather than a chosen one.
    #[test]
    fn the_grid_step_is_invisible_in_the_output() {
        let Some(ctx) = ctx() else { return };
        let img = grid_chart(200, 160);
        let reference = on_cpu(Resampler::CatmullRom, img.clone());
        let d = crate::equivalence::compare(&reference, &on_gpu(&ctx, Resampler::CatmullRom, img));
        eprintln!(
            "GRID_STEP {GRID_STEP}: worst {} levels, {:.3}% of samples differ",
            d.worst_levels,
            d.differing_percent()
        );
        assert!(
            d.worst_levels < 1.5,
            "GRID_STEP {GRID_STEP} costs {} levels — sweep it again",
            d.worst_levels
        );
    }

    /// ⚠ The vignetting gain rides the grid as a seventh component. Without this, a shader reading
    /// six components and ignoring the seventh renders a plausible photograph that is simply not
    /// corrected for falloff — and on a synthetic profile with mild vignetting the equivalence
    /// bound above might not see it.
    #[test]
    fn the_vignetting_gain_reaches_the_output() {
        let flat = {
            let mut img = ImageBuf::new(200, 160);
            for y in 0..160 {
                for x in 0..200 {
                    img.set_pixel(x, y, [0.5, 0.5, 0.5]);
                }
            }
            img
        };
        let with = on_cpu(Resampler::CatmullRom, flat.clone());
        let mut gains: Vec<f32> = Vec::new();
        for (a, b) in flat.data().iter().zip(with.data()) {
            if *a > 0.0 {
                gains.push(b / a);
            }
        }
        let spread = gains.iter().cloned().fold(0.0f32, f32::max)
            - gains.iter().cloned().fold(f32::MAX, f32::min);
        assert!(
            spread > 1e-3,
            "fixture check: this profile's vignetting must vary across the frame, spread {spread}"
        );
    }
}
```

⚠ If `the_vignetting_gain_reaches_the_output`'s fixture check fails, the synthetic profile carries
no vignetting model — add a `<vignetting model="pa" focal="50" aperture="2.8" distance="10"
k1="-0.3" k2="0.0" k3="0.0"/>` line to the calibration block and re-run. **Fix the fixture, never
the assertion.**

- [ ] **Step 6: run the tests**

```bash
cargo test --release -p gpu-pipeline lens_correction
```

⚠ `--release`: acquiring a GPU adapter costs 44–75 s in a debug build against 21 ms in release.

Expected: 4 passed (or skipped with a printed note if no adapter).

- [ ] **Step 7: gate and commit**

```bash
cargo fmt --all
cargo fmt --all -- --check; echo "FMT=$?"
cargo clippy --all-targets --all-features -- -D warnings; echo "CLIPPY=$?"
cargo test --workspace; echo "TEST=$?"
git add crates/gpu-pipeline/
git commit -m "feat(gpu): lens correction as a compute shader over a coarse displacement grid"
```

---

### Task 4: the third arm on the seam

**Files:**
- Modify: `crates/gpu-pipeline/src/accelerate.rs`
- Modify: `crates/app/src/render_engine.rs`

- [ ] **Step 1: change the signature and add the arm**

In `accelerate_prefix_stage`:

```rust
pub fn accelerate_prefix_stage(
    name: &str,
    edits: &Edits,
    scale: RenderScale,
    image: &core_image::ImageBuf,
    // ⚠⚠ **Two facts `Edits` cannot carry, and a stage that needed either could not be built from
    // the seam's old signature.** The `LensShot` comes from the file's own metadata matched against
    // the bundled database; the `Resampler` is a `RenderSpec` field, i.e. a property of the render
    // rather than of the photograph. Both are in hand at every call site.
    lens: Option<&lens_profile::LensShot>,
    resampler: pipeline::Resampler,
    ctx: &Arc<GpuContext>,
) -> Option<Box<dyn PipelineStage>> {
    let gpu: Box<dyn GpuStage> = match name {
        "dehaze" => Box::new(crate::GpuDehazeStage::new(&edits.dehaze, scale, image, ctx)),
        "noise_reduction" => Box::new(crate::GpuNoiseReductionStage::new(
            &edits.noise_reduction,
            scale,
            ctx,
        )),
        // ⚠ The only arm that can DECLINE: a profile that yields no corrector has nothing to
        // accelerate, and `None` here leaves the CPU stage in place — the seam's own rule, fail
        // toward the whole-buffer path.
        "lens_correction" => {
            let c = edits.applied_lens_correction();
            let lens = lens?;
            Box::new(crate::GpuLensCorrectionStage::new(
                lens,
                lens_profile::Corrections {
                    distortion: c.distortion,
                    tca: c.tca,
                    vignetting: c.vignetting,
                    scale: c.scale,
                },
                resampler,
                image,
                ctx,
            )?)
        }
        _ => return None,
    };
    if !worth_a_chain(&[gpu.cpu_cost()]) {
        return None;
    }
    Some(Box::new(GpuChain::new(ctx.clone(), vec![gpu])))
}
```

- [ ] **Step 2: update the forcing function**

⚠⚠ **`every_other_prefix_stage_is_left_alone` now fails, and that is what makes adding this arm a
deliberate act rather than a silent one** — exactly as it fired when noise reduction landed. Narrow
it to the two analytic resamplers and say why:

```rust
    /// ⚠⚠ **A FORCING FUNCTION, not a coverage test.** Naming the stages with no GPU twin is what
    /// makes adding one a deliberate act: this test breaks, and whoever breaks it has to say why.
    ///
    /// ⚠ What is left are the two **analytic** resamplers. They are not a third instance of this
    /// milestone's mechanism: perspective is a homography and geometry a rotation, a few arithmetic
    /// operations per pixel wanting no field and no texture at all — and M8.5 took both from
    /// ~160 ms to ~34 by giving them eight cores rather than a GPU.
    #[test]
    fn every_other_prefix_stage_is_left_alone() {
        let Some(ctx) = ctx() else { return };
        let img = hazy(64, 64);
        for name in ["geometry", "perspective"] {
            assert!(
                accelerate_prefix_stage(
                    name,
                    &hazed_edits(60.0),
                    SCALE,
                    &img,
                    None,
                    pipeline::Resampler::Bilinear,
                    &ctx
                )
                .is_none(),
                "`{name}` has no GPU twin and must be left alone"
            );
        }
    }
```

⚠ Every other call of `accelerate_prefix_stage` in that file's tests needs the two new arguments
(`None, pipeline::Resampler::Bilinear`). The compiler names them all.

- [ ] **Step 3: add a test that the new arm both fires and declines**

```rust
    /// ⚠⚠ **Both halves, because either alone passes against a bug.** An arm that never fires and
    /// one that fires unconditionally are equally wrong, and only the pair distinguishes them —
    /// the shape `a_bare_keyword_is_deleted_without_a_dialog_and_a_parent_is_not` already uses.
    #[test]
    fn lens_correction_is_accelerated_only_when_a_profile_is_in_hand() {
        let Some(ctx) = ctx() else { return };
        let img = hazy(64, 64);
        let mut e = Edits::default();
        e.lens_correction = Some(edit_model::LensCorrection {
            distortion: true,
            tca: true,
            vignetting: true,
            scale: 0.0,
        });
        assert!(
            accelerate_prefix_stage(
                "lens_correction",
                &e,
                SCALE,
                &img,
                None,
                pipeline::Resampler::CatmullRom,
                &ctx
            )
            .is_none(),
            "with no profile there is nothing to accelerate"
        );
        let lens = super::tests::lens_fixture();
        assert!(
            accelerate_prefix_stage(
                "lens_correction",
                &e,
                SCALE,
                &img,
                Some(&lens),
                pipeline::Resampler::CatmullRom,
                &ctx
            )
            .is_some(),
            "with a profile the seam must fire"
        );
    }
```

⚠ `lens_fixture()` is the synthetic `LensShot` from Task 3's tests. Make it `pub(crate)` in a shared
test helper, or copy the eight-line builder into this module — copying is acceptable here and the
two are compared by no test, but say which you did.

- [ ] **Step 4: update the one production call site**

In `crates/app/src/render_engine.rs`, around line 1225:

```rust
            let accelerated = if gpu_tail {
                gpu_pipeline::shared().and_then(|ctx| {
                    gpu_pipeline::accelerate_prefix_stage(
                        step.stage.name(),
                        edits,
                        scale,
                        &img,
                        lens.as_ref(),
                        resampler,
                        ctx,
                    )
                })
            } else {
                None
            };
```

⚠ `lens` and `resampler` are already parameters of that function — check their exact names in scope
rather than assuming; `lens` is `Option<lens_profile::LensShot>` at line 1031.

- [ ] **Step 5: build and test**

```bash
cargo test --release -p gpu-pipeline accelerate
cargo clippy --all-targets --all-features -- -D warnings; echo "CLIPPY=$?"
```

- [ ] **Step 6: sabotage — make the arm unreachable**

Change `"lens_correction" =>` to `"lens_correction_x" =>` and re-run. Expected:
`lens_correction_is_accelerated_only_when_a_profile_is_in_hand` fails on its second assertion, and
nothing else does. Restore from a copy.

- [ ] **Step 7: gate and commit**

```bash
cargo fmt --all
cargo fmt --all -- --check; echo "FMT=$?"
cargo clippy --all-targets --all-features -- -D warnings; echo "CLIPPY=$?"
cargo test --workspace; echo "TEST=$?"
git add -A
git commit -m "feat(gpu): lens correction joins the prefix seam, which gains the lens and the kernel"
```

---

### Task 5: sweep `GRID_STEP`, and the remaining sabotages

**Files:**
- Modify: `crates/gpu-pipeline/src/lens_correction.rs` (the constant and its doc, if the sweep moves it)

- [ ] **Step 1: sweep**

Temporarily parameterise the stage on the step (a `#[cfg(test)]` constructor taking it, the
`Weights::with_table_size` precedent), and run the output-level comparison at 8 / 16 / 32 / 64 on the
`grid_chart` fixture. Record the worst levels at each. Keep the smallest step whose cost is
indistinguishable from the next finer one — and **write the table into `GRID_STEP`'s doc comment**,
so the next reader can check it rather than trust it.

⚠ If 16 measures worse than ~1 level on the synthetic profile, that is a finding about the fixture as
much as the constant: the synthetic `b = -0.05` is a stronger correction than the real XF16-80's.
Report both numbers rather than tuning the constant to the fixture.

- [ ] **Step 2: the remaining sabotages**

Each one, restoring from a `cp` copy named per file, and confirming a `test result:` line appears
before believing anything — **a sabotage that fails to compile reads exactly like one that passes.**

| sabotage | expected |
|---|---|
| `gather` reads `img` instead of `p0`/`p1`/`p2` | `every_kernel_renders_the_same_picture_as_the_cpu` fails — the gather reads pixels it has overwritten |
| drop the `* gain` | the equivalence test fails |
| use channel 0's displacement for all three | the equivalence test fails (TCA collapses) |
| the bilinear branch reads the table | **bilinear alone** fails, the other three pass |
| `grid_at` takes `a + 1u` rather than `a + {stride}u` | the equivalence test fails |
| the two dispatches run in **two passes** rather than one | ⚠ predicted **inert** — M8.3a measured the alternative as harmless, so record it as inert rather than counting it |
| drop the out-of-bounds `continue` in `sample_ch` | expected to fail at the frame's border; if it does **not**, say so — the margin may make it unreachable on this fixture |

- [ ] **Step 3: gate and commit**

```bash
cargo fmt --all && cargo fmt --all -- --check; echo "FMT=$?"
cargo clippy --all-targets --all-features -- -D warnings; echo "CLIPPY=$?"
cargo test --workspace; echo "TEST=$?"
git add -A
git commit -m "test(gpu): GRID_STEP swept against output levels, with the table in its own doc"
```

---

### Task 6: measure the real thing

⚠⚠ **This task is the reason the spec projects nothing.** Composing component measurements has missed
in this crate three times; the plan's last task measures what shipped.

- [ ] **Step 1: price the stage**

Extend `crates/app/examples/probe_lens.rs` with a row comparing `LensCorrectionStage::apply` against
the same stage offered to `accelerate_prefix_stage`, on a real `.RAF` at full resolution:

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo run --release -p app --example probe_lens ~/Pictures/fuji/DSCF6296.RAF
```

Record three runs, report a **band** rather than a figure, and note the machine's load — this file's
own record had to retract a headline for quoting a point reading.

- [ ] **Step 2: set `cpu_cost` from the measurement**

The **bottom** of the measured band, scaled to ms per 6.8 Mpx. Conservative is the safe direction:
a lower number makes a run less likely to chain, and the gate must fail toward the CPU.

- [ ] **Step 3: gate and commit**

```bash
git add -A
git commit -m "perf(gpu): lens correction measured through the shipped path, cpu_cost set from it"
```

- [ ] **Step 4: launch the GUI**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo run --release -p app
```

Say what to look at: **Settings → Rendering → "Use the GPU"** on, then a photo with a matched lens
profile at the **fit** rung (the unscoped path, where the seam fires). ⚠⚠ **The picture must not
change.** ⚠ And **start at 1:1**, because a GPU prefix stage is the only thing that allocates a
full-resolution image — 298 MiB in and 298 MiB out — and that is the configuration that killed the
render worker when dehaze landed.
