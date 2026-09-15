//! The lens correction's displacement field, sampled on a coarse lattice.
//!
//! ⚠⚠ **The milestone's central mechanism, and it was settled by measurement rather than by
//! argument.** The alternative was reimplementing lensfun's distortion model in WGSL — a second
//! definition of the correction that can disagree with [`Corrector::source`], made harder by
//! `Modifier`'s `norm_scale`/`center_x`/`center_y` being private. Instead the CPU builds the field
//! it already knows how to build and a shader interpolates between grid points.
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
//!
//! ⚠ It lives in `pipeline` rather than in `lens-profile` for two reasons that both happen to point
//! the same way: this crate already has `rayon` and already depends on `lens-profile`, so a GPU twin
//! reaches it through a dependency it already has and **no new dependency enters the tree**; and
//! `lens-profile/src/lib.rs` is past the ~600-line marker and does not need to grow.

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

    /// The raw lattice, row-major, [`STRIDE`] values per point — what a shader binds.
    pub fn data(&self) -> &[f32] {
        &self.data
    }

    /// Bilinearly interpolate the seven values at a pixel.
    ///
    /// ⚠⚠ **The CPU twin of a shader's `grid_at`, and it exists so the accuracy sweep can run with
    /// no GPU at all.** Two implementations that must agree is how they come to disagree — so the
    /// shader is written from this, and an equivalence test is what holds them together.
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
                    <vignetting model="pa" focal="50" aperture="2.8" distance="10"
                        k1="-0.3" k2="0.0" k3="0.0"/>
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
    /// occupancy cliff).
    ///
    /// Measured here, on the synthetic `b = -0.05` at 400x300:
    ///
    /// | step | worst source error | worst gain error |
    /// |---|---|---|
    /// | 8 | 0.028 px | 0.00055 |
    /// | **16** | **0.111 px** | **0.0021** |
    /// | 32 | 0.459 px | 0.0091 |
    /// | 64 | 1.546 px | 0.0323 |
    ///
    /// Both columns are **quadratic in the step** to within a few percent, which is what bilinear
    /// interpolation of a smooth field must be — so the sweep is measuring the mechanism rather
    /// than noise.
    ///
    /// ⚠⚠ **Step 16 reads 40x the real XF16-80's 0.0027 px, and the fixture is why rather than the
    /// code.** The error goes as `step^2 * max_displacement / frame^2`, and this synthetic lens
    /// distorts far harder relative to a 400x300 frame than a real zoom does across 6246x4170 — and
    /// its `k1 = -0.3` vignetting is likewise far steeper than the real lens, whose gain the same
    /// sweep found exact to six decimals. So this fixture is the **pessimistic** end, which is the
    /// right side to test on; it is also why 32 is not sub-pixel here while it is on a photograph.
    ///
    /// What is asserted is therefore two things, and neither is a number tuned until it passed:
    /// sub-pixel **at the step that ships and finer**, and that the error **grows** with the step.
    /// A sweep whose error fell as the lattice coarsened would be measuring nothing at all.
    #[test]
    fn the_grid_tracks_the_corrector_it_samples() {
        /// The step `gpu-pipeline`'s `GRID_STEP` ships. Named here rather than imported: this
        /// crate must not depend on that one, and the direction of the dependency is the point.
        const SHIPPED: usize = 16;
        let (w, h) = (400, 300);
        let c = corrector(w, h);
        let mut prev = 0.0f32;
        let mut prev_gain = 0.0f32;
        for step in [8usize, 16, 32, 64] {
            let g = DisplacementGrid::build(&c, w, h, step);
            let mut worst_src = 0.0f32;
            let mut worst_gain = 0.0f32;
            for y in 0..h {
                for x in 0..w {
                    let (xf, yf) = (x as f32, y as f32);
                    let truth = c.source(xf, yf);
                    for (ch, t) in truth.iter().enumerate() {
                        let got = g.source(xf, yf, ch);
                        worst_src = worst_src
                            .max((got[0] - t[0]).abs())
                            .max((got[1] - t[1]).abs());
                    }
                    worst_gain = worst_gain.max((g.gain(xf, yf) - c.gain(xf, yf)).abs());
                }
            }
            if step <= SHIPPED {
                assert!(
                    worst_src < 0.25,
                    "step {step}: worst source error {worst_src} px is not sub-pixel"
                );
            }
            // ⚠ The gain is the seventh component, and this is what justifies one lattice
            // rather than two: at the shipped step it costs about half of an 8-bit level on a
            // mid-tone, against a displacement error the resampler then spreads over several.
            if step <= SHIPPED {
                assert!(
                    worst_gain < 5e-3,
                    "step {step}: worst gain error {worst_gain} is too large"
                );
            }
            assert!(
                worst_gain > prev_gain,
                "step {step}: gain error {worst_gain} did not exceed the previous step's \
                 {prev_gain} — a coarser lattice cannot be more accurate"
            );
            assert!(
                worst_src > prev,
                "step {step}: error {worst_src} did not exceed the previous step's {prev} — \
                 a coarser lattice cannot be more accurate, so the sweep is measuring nothing"
            );
            prev = worst_src;
            prev_gain = worst_gain;
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
            for (ch, t) in truth.iter().enumerate() {
                let got = g.source(x, y, ch);
                assert!(
                    (got[0] - t[0]).abs() < 1e-3 && (got[1] - t[1]).abs() < 1e-3,
                    "grid point ({x}, {y}) ch {ch}: {got:?} against {t:?}"
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
