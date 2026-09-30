//! **M7.3a — MEASURE THREE RESAMPLING KERNELS. THIS ADOPTS NONE OF THEM.**
//!
//! `GeometryStage`'s straighten and keystone (one pass since M7.3b) resample through
//! [`sample_bilinear`](../../pipeline/src/stages/geometry.rs), and a single bilinear pass costs
//! roughly half the frame's acutance. This harness answers *what a better kernel would buy and
//! what it would cost*, on the user's own photographs, so the choice is made against numbers
//! rather than against reputation.
//!
//! ⚠⚠ **THE KERNELS LIVE HERE, NOT IN `pipeline`, ON PURPOSE.** Nothing in `crates/pipeline/`
//! is modified by this example — no golden can move and no existing caller's behaviour changes.
//! The rotation loop below is a *replica* of `rotate_bilinear`, and
//! `the_replica_agrees_with_the_production_stage` is what proves the replica measures the real
//! operation rather than something adjacent to it. If a kernel is adopted, it moves into
//! `pipeline::stages::geometry` and this example stays as the evidence.
//!
//! Usage: `cargo run --release -p app --example resample_kernels <raw> [out-dir]`
//!
//! ⚠ **`--release` is not optional.** A debug build rotates 26 Mpx sequentially and the timing
//! column becomes meaningless.

use core_image::ImageBuf;
use edit_model::{Edits, Geometry};
use pipeline::{PipelineStage, RenderScale};
use std::path::PathBuf;
use std::time::Instant;

/// The straighten angle every arm uses. Matches `persp_sharpness`, so the bilinear acutance here
/// is directly comparable to the 0.01661 baseline that motivated this task.
const ANGLE_DEG: f32 = 2.0;

/// Side of the square region the crops are taken from.
const CROP: usize = 220;

/// Nearest-neighbour magnification of the second crop written per kernel. Ringing is a one- to
/// two-pixel excursion; at 1:1 on a 6246-wide frame it is invisible on any screen.
const ZOOM: usize = 4;

/// Phases in the weight lookup table. 1/1024 of a pixel — far below the quantisation of an 8-bit
/// output, and the reason the LUT arm is a fair cost estimate rather than a cheat.
const LUT_PHASES: usize = 1024;

// ---------------------------------------------------------------------------------------------
// Kernels
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Kernel {
    /// What ships today.
    Bilinear,
    /// Mitchell–Netravali with B=0, C=0.5. Sharp, and the standard "no visible ringing" cubic.
    CatmullRom,
    /// Mitchell–Netravali with B=C=1/3. Softer than Catmull-Rom, less overshoot still.
    Mitchell,
    /// Windowed sinc, a=3. The sharpest of the four, and the only one expected to ring.
    Lanczos3,
}

impl Kernel {
    const ALL: [Kernel; 4] = [
        Kernel::Bilinear,
        Kernel::CatmullRom,
        Kernel::Mitchell,
        Kernel::Lanczos3,
    ];

    fn label(self) -> &'static str {
        match self {
            Kernel::Bilinear => "bilinear (shipping)",
            Kernel::CatmullRom => "bicubic Catmull-Rom",
            Kernel::Mitchell => "bicubic Mitchell",
            Kernel::Lanczos3 => "Lanczos3",
        }
    }

    fn slug(self) -> &'static str {
        match self {
            Kernel::Bilinear => "bilinear",
            Kernel::CatmullRom => "catmullrom",
            Kernel::Mitchell => "mitchell",
            Kernel::Lanczos3 => "lanczos3",
        }
    }

    /// Half-width of the support, in pixels. `2 * radius` taps per axis.
    fn radius(self) -> usize {
        match self {
            Kernel::Bilinear => 1,
            Kernel::CatmullRom | Kernel::Mitchell => 2,
            Kernel::Lanczos3 => 3,
        }
    }

    fn taps(self) -> usize {
        2 * self.radius()
    }

    fn weight(self, t: f32) -> f32 {
        let x = t.abs();
        match self {
            Kernel::Bilinear => {
                if x < 1.0 {
                    1.0 - x
                } else {
                    0.0
                }
            }
            Kernel::CatmullRom => mitchell_netravali(x, 0.0, 0.5),
            Kernel::Mitchell => mitchell_netravali(x, 1.0 / 3.0, 1.0 / 3.0),
            Kernel::Lanczos3 => {
                if x < 3.0 {
                    sinc(x) * sinc(x / 3.0)
                } else {
                    0.0
                }
            }
        }
    }
}

/// The Mitchell–Netravali cubic family. `B=0, C=0.5` is Catmull-Rom; `B=C=1/3` is Mitchell.
///
/// ⚠ Every member of this family is a partition of unity — the four taps sum to exactly 1 at any
/// phase — which is why normalising the weights below is a no-op for the two cubics and for
/// bilinear, and load-bearing only for Lanczos3.
fn mitchell_netravali(x: f32, b: f32, c: f32) -> f32 {
    let x2 = x * x;
    let x3 = x2 * x;
    if x < 1.0 {
        ((12.0 - 9.0 * b - 6.0 * c) * x3 + (-18.0 + 12.0 * b + 6.0 * c) * x2 + (6.0 - 2.0 * b))
            / 6.0
    } else if x < 2.0 {
        ((-b - 6.0 * c) * x3
            + (6.0 * b + 30.0 * c) * x2
            + (-12.0 * b - 48.0 * c) * x
            + (8.0 * b + 24.0 * c))
            / 6.0
    } else {
        0.0
    }
}

fn sinc(x: f32) -> f32 {
    if x.abs() < 1e-8 {
        1.0
    } else {
        let p = std::f32::consts::PI * x;
        p.sin() / p
    }
}

/// Weights for one axis at fractional offset `f`, written into `out[..taps]`, normalised.
///
/// Tap `i` sits at source index `floor(coord) - radius + 1 + i`, so its distance from the sample
/// point is `f + (radius - 1 - i)`.
///
/// ⚠ The normalisation divides by the sum over **all** taps including those that will read
/// out of bounds — which is what reproduces `sample_bilinear`'s black-outside darkening at the
/// frame edge instead of quietly renormalising it away.
fn axis_weights(k: Kernel, f: f32, out: &mut [f32; 6]) {
    let r = k.radius() as f32;
    let n = k.taps();
    let mut sum = 0.0;
    for (i, w) in out.iter_mut().enumerate().take(n) {
        *w = k.weight(f + (r - 1.0 - i as f32));
        sum += *w;
    }
    if sum != 0.0 {
        for w in out.iter_mut().take(n) {
            *w /= sum;
        }
    }
}

/// Precomputed axis weights, quantised to [`LUT_PHASES`] sub-pixel phases.
///
/// This exists because a naive Lanczos3 evaluates 24 `sin` calls per output pixel, and reporting
/// *that* as the cost of adopting Lanczos3 would overstate it by several times — nobody ships it
/// that way. The LUT is what adoption would actually pay.
struct WeightLut {
    kernel: Kernel,
    rows: Vec<f32>,
}

impl WeightLut {
    fn new(kernel: Kernel) -> Self {
        let n = kernel.taps();
        let mut rows = vec![0.0; LUT_PHASES * n];
        let mut buf = [0.0f32; 6];
        for p in 0..LUT_PHASES {
            axis_weights(kernel, p as f32 / LUT_PHASES as f32, &mut buf);
            rows[p * n..p * n + n].copy_from_slice(&buf[..n]);
        }
        Self { kernel, rows }
    }

    fn row(&self, f: f32) -> &[f32] {
        let n = self.kernel.taps();
        let p = ((f * LUT_PHASES as f32) as usize).min(LUT_PHASES - 1);
        &self.rows[p * n..p * n + n]
    }
}

/// Sample `src` at fractional `(sx, sy)` with `k`; out-of-bounds reads as black.
///
/// ⚠ Black-outside matches `sample_bilinear` exactly. It is also an adoption cost worth naming:
/// a 6-tap kernel bleeds black **3 px** into the frame where bilinear bleeds 1, so a straighten
/// crop has to discard a slightly wider border.
fn sample(src: &ImageBuf, sx: f32, sy: f32, k: Kernel) -> [f32; 3] {
    let mut wx = [0.0f32; 6];
    let mut wy = [0.0f32; 6];
    axis_weights(k, sx - sx.floor(), &mut wx);
    axis_weights(k, sy - sy.floor(), &mut wy);
    accumulate(src, sx, sy, k, &wx, &wy)
}

fn accumulate(src: &ImageBuf, sx: f32, sy: f32, k: Kernel, wx: &[f32], wy: &[f32]) -> [f32; 3] {
    let w = src.width() as i32;
    let h = src.height() as i32;
    let r = k.radius() as i32;
    let n = k.taps();
    let x0 = sx.floor() as i32 - r + 1;
    let y0 = sy.floor() as i32 - r + 1;
    let mut out = [0.0f32; 3];
    for (j, wyj) in wy.iter().enumerate().take(n) {
        let yi = y0 + j as i32;
        if yi < 0 || yi >= h {
            continue;
        }
        let mut row = [0.0f32; 3];
        for (i, wxi) in wx.iter().enumerate().take(n) {
            let xi = x0 + i as i32;
            if xi < 0 || xi >= w {
                continue;
            }
            let p = src.pixel(xi as usize, yi as usize);
            row[0] += wxi * p[0];
            row[1] += wxi * p[1];
            row[2] += wxi * p[2];
        }
        out[0] += wyj * row[0];
        out[1] += wyj * row[1];
        out[2] += wyj * row[2];
    }
    out
}

// ---------------------------------------------------------------------------------------------
// The rotation, replicating `rotate_bilinear`'s inverse map exactly
// ---------------------------------------------------------------------------------------------

/// Inverse map used by `rotate_bilinear`: where in the source does output `(x, y)` come from?
fn inverse_map(x: usize, y: usize, cx: f32, cy: f32, sin: f32, cos: f32) -> (f32, f32) {
    let dx = x as f32 - cx;
    let dy = y as f32 - cy;
    (cx + dx * cos + dy * sin, cy - dx * sin + dy * cos)
}

/// Forward map, source → output. Only used to place the crops.
fn forward_map(sx: f32, sy: f32, cx: f32, cy: f32, sin: f32, cos: f32) -> (f32, f32) {
    let dx = sx - cx;
    let dy = sy - cy;
    (cx + dx * cos - dy * sin, cy + dx * sin + dy * cos)
}

fn centre(src: &ImageBuf) -> (f32, f32) {
    (
        (src.width() as f32 - 1.0) / 2.0,
        (src.height() as f32 - 1.0) / 2.0,
    )
}

/// Rotate about the centre, keeping the canvas size — `rotate_bilinear` with a choice of kernel.
///
/// ⚠ **Sequential, deliberately.** The production `rotate_bilinear` is a plain nested loop, and
/// the 203 ms figure this is measured against is sequential. Threading it would measure rayon
/// rather than the kernel. (It parallelises trivially over rows, which is an adoption note, not
/// something to fold into the comparison.)
fn rotate(src: &ImageBuf, angle_rad: f32, k: Kernel, lut: Option<&WeightLut>) -> ImageBuf {
    let (w, h) = (src.width(), src.height());
    let (cx, cy) = centre(src);
    let (sin, cos) = angle_rad.sin_cos();
    let mut out = ImageBuf::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let (sx, sy) = inverse_map(x, y, cx, cy, sin, cos);
            let px = match lut {
                None => sample(src, sx, sy, k),
                Some(t) => {
                    let wx = t.row(sx - sx.floor());
                    let wy = t.row(sy - sy.floor());
                    accumulate(src, sx, sy, k, wx, wy)
                }
            };
            out.set_pixel(x, y, px);
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Measurements
// ---------------------------------------------------------------------------------------------

fn luma(p: [f32; 3]) -> f64 {
    (0.2126 * p[0] + 0.7152 * p[1] + 0.0722 * p[2]) as f64
}

/// Mean absolute Laplacian — the same acutance proxy `persp_sharpness` uses, so the numbers are
/// directly comparable. Higher is crisper.
fn acutance(img: &ImageBuf) -> f64 {
    acutance_in(img, 1, 1, img.width() - 2, img.height() - 2)
}

/// Acutance over one rectangle.
///
/// ⚠⚠ **Whole-frame acutance cannot tell detail from grain, and that biases it toward the
/// sharper kernels for the wrong reason.** A resampler that smooths sensor noise loses acutance
/// even where it preserved every real edge. So it is also measured over the hottest tile (mostly
/// real structure) and the flattest one (almost entirely noise); the *ratio* of the two is the
/// statistic that says whether a kernel kept detail or merely kept grain.
fn acutance_in(img: &ImageBuf, x0: usize, y0: usize, w: usize, h: usize) -> f64 {
    let l = |x: usize, y: usize| luma(img.pixel(x, y));
    let mut sum = 0.0;
    let mut n = 0u64;
    for y in y0..y0 + h {
        for x in x0..x0 + w {
            sum += (4.0 * l(x, y) - l(x - 1, y) - l(x + 1, y) - l(x, y - 1) - l(x, y + 1)).abs();
            n += 1;
        }
    }
    sum / n.max(1) as f64
}

/// The rectangle a tile centre names, clamped inside the frame with a one-pixel Laplacian border.
fn tile_rect(img: &ImageBuf, c: (usize, usize), side: usize) -> (usize, usize, usize, usize) {
    let x0 =
        c.0.saturating_sub(side / 2)
            .clamp(1, img.width() - side - 1);
    let y0 =
        c.1.saturating_sub(side / 2)
            .clamp(1, img.height() - side - 1);
    (x0, y0, side, side)
}

/// Apply the neutral pipeline — ProPhoto → sRGB, then encode. Consumes, like every stage.
fn encoded(img: ImageBuf) -> ImageBuf {
    pipeline::build_pipeline(
        &Edits::default(),
        RenderScale::FULL,
        pipeline::TailRendering::colorimetric(),
    )
    .run(img)
}

/// 8-bit level (0..255) of one encoded sample.
fn level(v: f32) -> f32 {
    v.clamp(0.0, 1.0) * 255.0
}

/// Overshoot statistics for one kernel, measured against the bracket the sample point sits in.
#[derive(Default)]
struct Ringing {
    /// Largest excursion beyond the bracket, in 8-bit levels after the sRGB output transform.
    max_levels: f32,
    /// The span of the bracket that worst pixel sat in.
    ///
    /// ⚠ Reported instead of a ratio, because dividing the peak excursion by *its own* span is
    /// meaningless when that span is a fraction of a level — the first version of this printed
    /// "716%" for Mitchell off a 0.5-level bracket, which reads as catastrophic ringing and is
    /// in fact a 3.8-level wobble in a flat area. `max_frac` below is the guarded ratio.
    max_levels_span: f32,
    /// Largest excursion as a fraction of the bracket span, over brackets spanning >8 levels.
    max_frac: f32,
    /// Ringing **density** per [`TILE`]-sized cell: how many sampled pixels in each cell overshoot
    /// by more than [`VISIBLE_RING`] levels.
    ///
    /// ⚠⚠ **DENSITY, NOT THE SINGLE WORST PIXEL, AND THAT WAS A CORRECTION.** The first version
    /// centred the ringing crop on the frame's maximum excursion — which on this frame is about
    /// **five pixels out of 26 million**. A crop of a 1-in-26M outlier says nothing about whether
    /// anyone would ever see the artefact, and a sub-sampled scout could not even find it
    /// reliably (stride 3 reported 1.2 levels where the full pass found 27.5). The cell with the
    /// most ringing is both robust to sub-sampling and the honest answer to "where would this
    /// show".
    density: Vec<u32>,
    density_cols: usize,
    over_1: u64,
    over_4: u64,
    over_16: u64,
    considered: u64,
}

impl Ringing {
    /// Centre of the cell with the most ringing, in output pixels, and that cell's count.
    fn worst_cell(&self) -> ((usize, usize), u32) {
        let (i, n) = self
            .density
            .iter()
            .enumerate()
            .max_by_key(|(_, n)| **n)
            .map(|(i, n)| (i, *n))
            .unwrap_or((0, 0));
        let cols = self.density_cols.max(1);
        (
            ((i % cols) * TILE + TILE / 2, (i / cols) * TILE + TILE / 2),
            n,
        )
    }
}

/// An overshoot has to reach this many 8-bit levels before it counts toward ringing density.
/// One level is below the threshold of visibility; four is where a halo starts to read.
const VISIBLE_RING: f32 = 4.0;

/// ⚠⚠ **THE DEFINITION, BECAUSE IT IS THE WHOLE POINT OF THE TASK.**
///
/// For every output pixel the sample point `(sx, sy)` lies inside one source cell. The four
/// pixels bounding that cell are the *bracket*: a monotone interpolant cannot leave the interval
/// they span, and bilinear provably never does. Overshoot is how far a kernel leaves it.
///
/// Measured per channel in **linear light** — that is where the interpolation happens and where
/// the bracket is exactly defined — and then reported in **8-bit sRGB levels**, because that is
/// the unit in which a halo is or is not visible. The sRGB encode is monotone per channel, so
/// converting the endpoints and the output separately is exact rather than an approximation.
///
/// ⚠ This matters here specifically because `SharpenStage` already carries a `HALO_KNEE` constant
/// to suppress halos. A ringing resampler puts a halo *into* the buffer that a downstream stage
/// then spends effort trying to take out.
/// ⚠ It takes **no `Kernel`**, and that absence is the point: every kernel is measured over the
/// identical set of pixels, chosen by `MAX_RADIUS` below. Sizing the margin per kernel would let
/// bilinear count pixels Lanczos3 skipped, and the four columns would stop being comparable.
fn ringing(
    src: &ImageBuf,
    out: &ImageBuf,
    angle_rad: f32,
    space: color::OutputSpace,
    stride: usize,
) -> Ringing {
    let (w, h) = (out.width(), out.height());
    let (cx, cy) = centre(src);
    let (sin, cos) = angle_rad.sin_cos();
    let sw = src.width() as i32;
    let sh = src.height() as i32;
    // ⚠⚠ **THE FIRST VERSION OF THIS COUNTED THE FRAME BORDER AS RINGING, AND THE CROPS ARE WHAT
    // EXPOSED IT.** It skipped a pixel only when the 2×2 *bracket* fell outside the source — but
    // a rotation sweeps a black wedge deep into the frame (at 2° on a 6246 px edge the corners
    // move ~109 px), and just inside that wedge the bracket is in bounds while a **wider
    // kernel's support is not**. Lanczos3 then reads black from beyond the edge and undershoots,
    // which is real arithmetic and is *not* edge ringing on picture content. The densest-ringing
    // cell came back at x = 64, i.e. the border itself.
    //
    // ⚠ The support test uses `MAX_RADIUS`, not the kernel's own, so **all four kernels are
    // measured over exactly the same set of pixels**. Per-kernel margins would let bilinear
    // count pixels Lanczos3 skipped and make the comparison meaningless.
    const MAX_RADIUS: usize = 3;
    let margin = MAX_RADIUS + 2;
    let mut r = Ringing {
        density_cols: w.div_ceil(TILE),
        density: vec![0; w.div_ceil(TILE) * h.div_ceil(TILE)],
        ..Default::default()
    };
    let enc = |c: [f32; 3]| {
        let p = space.from_working(c);
        [
            level(space.encode(p[0])),
            level(space.encode(p[1])),
            level(space.encode(p[2])),
        ]
    };
    for y in (margin..h.saturating_sub(margin)).step_by(stride) {
        for x in (margin..w.saturating_sub(margin)).step_by(stride) {
            let (sx, sy) = inverse_map(x, y, cx, cy, sin, cos);
            let x0 = sx.floor() as i32;
            let y0 = sy.floor() as i32;
            let rad = MAX_RADIUS as i32;
            if x0 - rad + 1 < 0 || y0 - rad + 1 < 0 || x0 + rad >= sw || y0 + rad >= sh {
                continue;
            }
            let a = src.pixel(x0 as usize, y0 as usize);
            let b = src.pixel(x0 as usize + 1, y0 as usize);
            let c = src.pixel(x0 as usize, y0 as usize + 1);
            let d = src.pixel(x0 as usize + 1, y0 as usize + 1);
            let o = out.pixel(x, y);
            let (ea, eb, ec, ed, eo) = (enc(a), enc(b), enc(c), enc(d), enc(o));
            r.considered += 1;
            let mut worst = 0.0f32;
            let mut worst_span = 0.0f32;
            for ch in 0..3 {
                let lo = ea[ch].min(eb[ch]).min(ec[ch]).min(ed[ch]);
                let hi = ea[ch].max(eb[ch]).max(ec[ch]).max(ed[ch]);
                let over = (eo[ch] - hi).max(lo - eo[ch]).max(0.0);
                if over > worst {
                    worst = over;
                    worst_span = hi - lo;
                }
            }
            if worst > r.max_levels {
                r.max_levels = worst;
                r.max_levels_span = worst_span;
            }
            // ⚠ Only brackets with a real span get a ratio. Dividing by the span of four nearly
            // equal pixels turns sensor noise into a spectacular-looking percentage.
            if worst_span > 8.0 {
                r.max_frac = r.max_frac.max(worst / worst_span);
            }
            if worst > VISIBLE_RING {
                r.density[(y / TILE) * r.density_cols + (x / TILE)] += 1;
            }
            if worst > 1.0 {
                r.over_1 += 1;
            }
            if worst > 4.0 {
                r.over_4 += 1;
            }
            if worst > 16.0 {
                r.over_16 += 1;
            }
        }
    }
    r
}

/// Side of the tiles scored by [`survey_tiles`].
const TILE: usize = 128;

/// Where the hardest edges are, and where there are none.
struct Tiles {
    /// Centre of the tile holding the most hard edges, and its strongest local step in levels.
    hot: (usize, usize),
    hot_step: f32,
    /// Centre of the flattest tile with usable brightness — the noise reference.
    flat: (usize, usize),
}

/// Rank 128-px tiles on the *encoded* source, so the "hard edge" threshold is in visible units.
///
/// ⚠ The flat tile skips anything very dark or blown: a black frame corner or a clipped sky has
/// no noise to measure either, and would win "flattest" while saying nothing about grain.
fn survey_tiles(enc_src: &ImageBuf) -> Tiles {
    let (w, h) = (enc_src.width(), enc_src.height());
    let l = |x: usize, y: usize| level(enc_src.pixel(x, y)[1]);
    let mut hot = ((w / 2, h / 2), 0.0f32, 0u64);
    let mut flat = ((w / 2, h / 2), f32::MAX);
    let mut ty = 1;
    while ty + TILE < h {
        let mut tx = 1;
        while tx + TILE < w {
            let mut hard = 0u64;
            let mut peak = 0.0f32;
            let mut range_sum = 0.0f64;
            let mut mean = 0.0f64;
            let mut n = 0u64;
            // Every 2nd pixel: enough to rank tiles, four times cheaper.
            for y in (ty..ty + TILE).step_by(2) {
                for x in (tx..tx + TILE).step_by(2) {
                    let c = l(x, y);
                    let range = (l(x - 1, y) - c)
                        .abs()
                        .max((l(x + 1, y) - c).abs())
                        .max((l(x, y - 1) - c).abs())
                        .max((l(x, y + 1) - c).abs());
                    if range > 100.0 {
                        hard += 1;
                    }
                    peak = peak.max(range);
                    range_sum += range as f64;
                    mean += c as f64;
                    n += 1;
                }
            }
            let centre = (tx + TILE / 2, ty + TILE / 2);
            if hard > hot.2 {
                hot = (centre, peak, hard);
            }
            let mean = mean / n as f64;
            let avg_range = (range_sum / n as f64) as f32;
            if (24.0..=224.0).contains(&mean) && avg_range < flat.1 {
                flat = (centre, avg_range);
            }
            tx += TILE;
        }
        ty += TILE;
    }
    Tiles {
        hot: hot.0,
        hot_step: hot.1,
        flat: flat.0,
    }
}

// ---------------------------------------------------------------------------------------------
// Crops
// ---------------------------------------------------------------------------------------------

fn save_crop(enc: &ImageBuf, centre: (usize, usize), dir: &std::path::Path, name: &str) {
    let (w, h) = (enc.width(), enc.height());
    let x0 = centre
        .0
        .saturating_sub(CROP / 2)
        .min(w.saturating_sub(CROP));
    let y0 = centre
        .1
        .saturating_sub(CROP / 2)
        .min(h.saturating_sub(CROP));
    let cw = CROP.min(w);
    let ch = CROP.min(h);
    let mut flat = Vec::with_capacity(cw * ch * 3);
    let mut zoom = Vec::with_capacity(cw * ZOOM * ch * ZOOM * 3);
    for y in 0..ch {
        for _ in 0..ZOOM {
            for x in 0..cw {
                let p = enc.pixel(x0 + x, y0 + y);
                let b = [
                    level(p[0]).round() as u8,
                    level(p[1]).round() as u8,
                    level(p[2]).round() as u8,
                ];
                for _ in 0..ZOOM {
                    zoom.extend_from_slice(&b);
                }
            }
        }
        for x in 0..cw {
            let p = enc.pixel(x0 + x, y0 + y);
            flat.extend_from_slice(&[
                level(p[0]).round() as u8,
                level(p[1]).round() as u8,
                level(p[2]).round() as u8,
            ]);
        }
    }
    let one = dir.join(format!("{name}.png"));
    let big = dir.join(format!("{name}_{ZOOM}x.png"));
    image::save_buffer(&one, &flat, cw as u32, ch as u32, image::ColorType::Rgb8).expect("write");
    image::save_buffer(
        &big,
        &zoom,
        (cw * ZOOM) as u32,
        (ch * ZOOM) as u32,
        image::ColorType::Rgb8,
    )
    .expect("write");
}

// ---------------------------------------------------------------------------------------------

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args
        .next()
        .expect("usage: resample_kernels <raw> [out-dir]");
    let dir = PathBuf::from(args.next().unwrap_or_else(|| ".".into()));
    std::fs::create_dir_all(&dir).expect("out-dir");

    let opts = raw_decode::DecodeOptions::linear_scene();
    let t = Instant::now();
    let dec = raw_decode::decode_raw(std::path::Path::new(&path), &opts).expect("decode");
    let src = dec.image;
    println!(
        "source {}x{} ({:.1} Mpx), decoded in {} ms",
        src.width(),
        src.height(),
        (src.width() * src.height()) as f64 / 1e6,
        t.elapsed().as_millis()
    );
    println!("straighten angle {ANGLE_DEG}°, sequential, best of 3\n");

    let angle = ANGLE_DEG.to_radians();
    let space = color::OutputSpace::Srgb;

    // ⚠ The replica has to be checked against the real stage before any number it produces is
    // worth reading. Run on a crop, because the full frame is not needed to prove agreement.
    let probe = src.crop(1200, 800, 512, 512);
    let mine = rotate(&probe, angle, Kernel::Bilinear, None);
    let theirs = pipeline::GeometryStage::new(
        &Geometry {
            angle: ANGLE_DEG,
            ..Default::default()
        },
        // ⚠ Bilinear explicitly: the replica above is being checked against the *shipping*
        // sampler this example measured, not against whatever the default has since become.
        pipeline::Resampler::Bilinear,
    )
    .apply(probe.clone());
    let mut worst = 0.0f32;
    for y in 0..mine.height() {
        for x in 0..mine.width() {
            let (a, b) = (mine.pixel(x, y), theirs.pixel(x, y));
            for c in 0..3 {
                worst = worst.max((a[c] - b[c]).abs());
            }
        }
    }
    println!("replica check vs GeometryStage: max |diff| {worst:.3e} (float association only)\n");
    drop(probe);
    drop(mine);
    drop(theirs);

    let enc_src = encoded(src.clone());
    let neutral = acutance(&enc_src);
    let tiles = survey_tiles(&enc_src);
    let (cx, cy) = centre(&src);
    let (sin, cos) = angle.sin_cos();
    // The tiles are located in source coordinates; every rotated arm has to be measured over the
    // *same content*, which after a 2° rotation is a couple of hundred pixels away.
    let map_tile = |c: (usize, usize)| {
        let p = forward_map(c.0 as f32, c.1 as f32, cx, cy, sin, cos);
        (p.0.max(0.0) as usize, p.1.max(0.0) as usize)
    };
    let hot_out = map_tile(tiles.hot);
    let flat_out = map_tile(tiles.flat);
    let hr = tile_rect(&enc_src, tiles.hot, TILE);
    let fr = tile_rect(&enc_src, tiles.flat, TILE);
    let neutral_hot = acutance_in(&enc_src, hr.0, hr.1, hr.2, hr.3);
    let neutral_flat = acutance_in(&enc_src, fr.0, fr.1, fr.2, fr.3);
    println!(
        "hot tile at ({}, {}) — strongest local step {:.0}/255;  flat (noise) tile at ({}, {})\n",
        tiles.hot.0, tiles.hot.1, tiles.hot_step, tiles.flat.0, tiles.flat.1
    );
    save_crop(&enc_src, tiles.hot, &dir, "detail_00_source_unrotated");
    drop(enc_src);

    // ⚠⚠ A SCOUT PASS, AND IT IS WHAT MAKES THE RINGING CROPS WORTH LOOKING AT. The hot tile
    // says where edges are *dense*; it does not say where a kernel misbehaves most, and those
    // are different places. Lanczos3 rings hardest, so the cell where *it* rings most densely is
    // the frame's worst case — and every kernel is then cropped at that same spot, which is the
    // only way the four crops are a comparison rather than four unrelated pictures.
    let scout = rotate(
        &src,
        angle,
        Kernel::Lanczos3,
        Some(&WeightLut::new(Kernel::Lanczos3)),
    );
    let scout_r = ringing(&src, &scout, angle, space, 2);
    drop(scout);
    let (ring_at, ring_n) = scout_r.worst_cell();
    println!(
        "densest Lanczos3 ringing in the {TILE}px cell at ({}, {}) — {ring_n} sampled pixels over \
         {VISIBLE_RING:.0} levels\n",
        ring_at.0, ring_at.1
    );

    println!(
        "{:<22} {:>9} {:>7} {:>9} {:>9} {:>8} {:>8} {:>7}",
        "kernel", "acutance", "% of", "naive ms", "LUT ms", "hot", "noise", "h/n"
    );
    println!(
        "{:<22} {:>9.5} {:>6.0}% {:>9} {:>9} {:>8.4} {:>8.4} {:>7.2}",
        "no resample",
        neutral,
        100.0,
        "-",
        "-",
        neutral_hot,
        neutral_flat,
        neutral_hot / neutral_flat
    );

    for k in Kernel::ALL {
        let lut = WeightLut::new(k);

        let mut naive = u128::MAX;
        let mut out = ImageBuf::new(1, 1);
        for _ in 0..3 {
            let t = Instant::now();
            out = rotate(&src, angle, k, None);
            naive = naive.min(t.elapsed().as_millis());
        }
        let mut lut_ms = u128::MAX;
        let mut out_lut = ImageBuf::new(1, 1);
        for _ in 0..3 {
            let t = Instant::now();
            out_lut = rotate(&src, angle, k, Some(&lut));
            lut_ms = lut_ms.min(t.elapsed().as_millis());
        }
        // How much accuracy the LUT costs. If this is not tiny the LUT timing is not honest.
        let mut lut_err = 0.0f32;
        for y in (0..out.height()).step_by(7) {
            for x in (0..out.width()).step_by(7) {
                let (a, b) = (out.pixel(x, y), out_lut.pixel(x, y));
                for c in 0..3 {
                    lut_err = lut_err.max((a[c] - b[c]).abs());
                }
            }
        }
        drop(out_lut);

        let r = ringing(&src, &out, angle, space, 1);
        let enc = encoded(out);
        let acut = acutance(&enc);
        let hr = tile_rect(&enc, hot_out, TILE);
        let fr = tile_rect(&enc, flat_out, TILE);
        let a_hot = acutance_in(&enc, hr.0, hr.1, hr.2, hr.3);
        let a_flat = acutance_in(&enc, fr.0, fr.1, fr.2, fr.3);
        save_crop(&enc, hot_out, &dir, &format!("detail_{}", k.slug()));
        save_crop(&enc, ring_at, &dir, &format!("ringing_{}", k.slug()));
        drop(enc);

        println!(
            "{:<22} {:>9.5} {:>6.1}% {:>9} {:>9} {:>8.4} {:>8.4} {:>7.2}",
            k.label(),
            acut,
            100.0 * acut / neutral,
            naive,
            lut_ms,
            a_hot,
            a_flat,
            a_hot / a_flat
        );
        println!(
            "    ringing: max {:.1} levels beyond a bracket spanning {:.1} | \
             >1 lvl {:.3}%  >4 {:.4}%  >16 {:.5}%  worst ratio {:.0}% | LUT err {:.1e}",
            r.max_levels,
            r.max_levels_span,
            100.0 * r.over_1 as f64 / r.considered as f64,
            100.0 * r.over_4 as f64 / r.considered as f64,
            100.0 * r.over_16 as f64 / r.considered as f64,
            100.0 * r.max_frac,
            lut_err,
        );
    }

    println!("\ncrops written to {}", dir.display());
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⚠ Run with `cargo test --example resample_kernels` — an example's `#[test]`s are not part
    /// of `cargo test --all` unless `test = true` is set for the target in `Cargo.toml`.
    /// Deliberately left off so this task's footprint is one new file and the suite count is
    /// unchanged.
    ///
    /// ⚠⚠ **THIS TEST FIRST ASSERTED SOMETHING FALSE ABOUT CORRECT CODE, and the false half is a
    /// finding worth carrying into the decision.** It demanded that *every* kernel reproduce the
    /// source exactly at an integer coordinate. Bilinear, Catmull-Rom and Lanczos3 do —
    /// they are *interpolating* filters. **Mitchell is not.** `B = 1/3` puts weight `0.0556` on
    /// each neighbour at zero phase, so a Mitchell resample softens the image even where nothing
    /// moved. That is inherent to the family (any `B > 0` is approximating, not interpolating)
    /// and is the real reason Mitchell reads as the soft option.
    #[test]
    fn three_of_the_four_are_interpolating_and_mitchell_is_not() {
        for k in Kernel::ALL {
            let mut w = [0.0f32; 6];
            axis_weights(k, 0.0, &mut w);
            let centre_tap = k.radius() - 1;
            if k == Kernel::Mitchell {
                assert!(
                    w[centre_tap] < 0.9,
                    "Mitchell should not be interpolating, centre tap is {}",
                    w[centre_tap]
                );
                continue;
            }
            for (i, wi) in w.iter().enumerate().take(k.taps()) {
                let want = if i == centre_tap { 1.0 } else { 0.0 };
                assert!(
                    (wi - want).abs() < 1e-6,
                    "{k:?} tap {i} is {wi}, want {want}"
                );
            }
        }
    }

    /// A kernel whose weights do not sum to 1 modulates brightness with sub-pixel phase — a
    /// moiré that looks like a sensor problem rather than a resampler one.
    #[test]
    fn every_kernel_sums_to_one_at_every_phase() {
        for k in Kernel::ALL {
            for p in 0..64 {
                let f = p as f32 / 64.0;
                let mut w = [0.0f32; 6];
                axis_weights(k, f, &mut w);
                let sum: f32 = w.iter().take(k.taps()).sum();
                assert!((sum - 1.0).abs() < 1e-5, "{k:?} at phase {f} sums to {sum}");
            }
        }
    }

    /// A step edge from 0 to 1 at x = 6, wide enough that Lanczos3's 6 taps stay in bounds for
    /// every probe below — so nothing here is measuring the black-outside border instead.
    fn step_edge() -> ImageBuf {
        let mut img = ImageBuf::new(12, 1);
        for x in 0..12 {
            img.set_pixel(x, 0, if x < 6 { [0.0; 3] } else { [1.0; 3] });
        }
        img
    }

    /// Worst excursion outside [0, 1] anywhere along the edge.
    ///
    /// ⚠⚠ **THE SWEEP RANGE IS THE WHOLE TEST, AND THE FIRST VERSION GOT IT WRONG.** It probed
    /// only *within* the transition cell, where a symmetric cubic's two negative lobes land on
    /// one source pixel of each value and cancel — Catmull-Rom measured **exactly zero** there
    /// and the test declared it non-ringing. The overshoot lives on the **shoulders**, a pixel
    /// past the edge, where the negative lobe reaches back across the step and the pixels it
    /// pulls against are all on the far side. Sweeping [4, 8) covers both shoulders.
    fn peak_overshoot(img: &ImageBuf, k: Kernel) -> f32 {
        let mut worst: f32 = 0.0;
        for step in 0..128 {
            let sx = 4.0 + step as f32 / 32.0;
            let v = sample(img, sx, 0.0, k)[0];
            worst = worst.max((v - 1.0).max(-v).max(0.0));
        }
        worst
    }

    /// ⚠ The discriminating test of the whole exercise: bilinear provably cannot leave the
    /// bracket, and the other three provably can. A harness that reported no overshoot for
    /// Lanczos3 would be measuring nothing — this is what says the measurement can see it.
    #[test]
    fn only_bilinear_stays_inside_the_bracket() {
        let img = step_edge();
        for k in Kernel::ALL {
            let worst = peak_overshoot(&img, k);
            if k == Kernel::Bilinear {
                assert!(worst < 1e-6, "bilinear overshot by {worst}");
            } else {
                assert!(worst > 1e-4, "{k:?} did not overshoot at all ({worst})");
            }
        }
    }

    /// The ordering the whole comparison rests on. Lanczos3 rings hardest, Mitchell least of the
    /// three; if that inverts, a coefficient is wrong.
    #[test]
    fn overshoot_is_ordered_mitchell_under_catmull_under_lanczos() {
        let img = step_edge();
        let (m, c, l) = (
            peak_overshoot(&img, Kernel::Mitchell),
            peak_overshoot(&img, Kernel::CatmullRom),
            peak_overshoot(&img, Kernel::Lanczos3),
        );
        assert!(m < c, "Mitchell {m} should ring less than Catmull-Rom {c}");
        assert!(c < l, "Catmull-Rom {c} should ring less than Lanczos3 {l}");
    }

    /// The Catmull-Rom and Mitchell coefficients, against values that can be checked by hand.
    /// `mitchell_netravali(0, B, C) = (6 - 2B)/6`, so 1.0 for B=0 and 8/9 for B=1/3.
    #[test]
    fn the_cubic_coefficients_are_the_published_ones() {
        assert!((mitchell_netravali(0.0, 0.0, 0.5) - 1.0).abs() < 1e-6);
        assert!((mitchell_netravali(0.0, 1.0 / 3.0, 1.0 / 3.0) - 8.0 / 9.0).abs() < 1e-6);
        // Catmull-Rom interpolates: it is exactly 0 at every other integer.
        assert!(mitchell_netravali(1.0, 0.0, 0.5).abs() < 1e-6);
        // Mitchell does not interpolate — it is a genuine approximation, which is why it is
        // softer. A test asserting it hits 0 here would be asserting something false.
        assert!(mitchell_netravali(1.0, 1.0 / 3.0, 1.0 / 3.0) > 0.05);
        // Both vanish outside their support.
        assert_eq!(mitchell_netravali(2.0, 0.0, 0.5), 0.0);
        assert_eq!(Kernel::Lanczos3.weight(3.5), 0.0);
    }

    /// Lanczos3 must be a windowed sinc, not a bare one: the window is what bounds the ringing.
    #[test]
    fn lanczos_is_windowed() {
        // sinc(2.5) alone is about -0.127; windowed by sinc(2.5/3) it is markedly smaller.
        let bare = sinc(2.5).abs();
        let windowed = Kernel::Lanczos3.weight(2.5).abs();
        assert!(windowed < bare * 0.6, "bare {bare}, windowed {windowed}");
    }
}
