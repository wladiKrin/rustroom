//! Where does the GPU coverage rasteriser start beating the CPU one — **separately for a gating
//! and a non-gating brush**? The measurement behind `MIN_GPU_BOX_PX`, which `CLAUDE.md` records as
//! a per-gating follow-up.
//!
//! ⚠⚠ **`MIN_GPU_BOX_PX` was swept once, on a NON-GATING brush only** (`brush_raster.rs:526`), and
//! its own doc comment says so: *"Measured on a NON-GATING brush, which is the GPU's weakest case,
//! so this is conservative for a gating one … a per-gating floor is a recorded follow-up."* This
//! example runs the same sweep twice — once with `auto_mask: false` and no base, once with
//! `auto_mask: true` and the decoded photograph as the base — over a finer ladder of box sizes, so
//! the two crossovers can be read off side by side.
//!
//! ⚠ **This example changes nothing.** It reads the constant only to print it.
//!
//! ⚠ It goes through `GpuBrushRaster`'s **inherent** `rasterize`, not the `CoverageRasterizer`
//! trait — the trait applies the very floor the sweep exists to choose, so going through it would
//! measure the floor rather than the crossover it should sit at. Same rule
//! `brush_raster_probe`'s sweep follows.
//!
//! ⚠ **Bands, not figures.** Each configuration is timed in `BLOCKS` independent blocks; each
//! block discards a warm-up round and takes the best of `ROUNDS`. What is reported is the range
//! over the blocks' bests, and the per-block ratio range — never a single number. M8.3h had to
//! retract a headline for quoting the top of a band as the value.
//!
//! `cargo run --release -p app --example box_floor_sweep <raw>`

use core_image::ImageBuf;
use edit_model::{Brush, BrushStroke, Geometry};
use pipeline::brush::{plan_dabs, rasterize_in, BasePixels, BaseRef, CoverageMap};
use std::time::Instant;

/// The rung every masked figure in `CLAUDE.md` is quoted at, and the one `brush_raster_probe`
/// uses — so these numbers sit beside the ones already recorded.
const RUNG: usize = 3200;

/// Independent timing blocks per configuration. The band is taken over these.
const BLOCKS: usize = 4;
/// Timed rounds inside a block, after one discarded warm-up.
const ROUNDS: usize = 4;

struct ImgBase<'a>(&'a ImageBuf);
impl BasePixels for ImgBase<'_> {
    fn at(&self, x: usize, y: usize) -> [f32; 3] {
        self.0.pixel(x, y)
    }
}

/// One stroke, a horizontal bar of relative length `k` and radius `k/2`, so the box grows as
/// `k^2`. Identical geometry in both variants — only the gate differs.
fn sweep_brush(k: f32, gating: bool) -> Brush {
    let n = 12;
    Brush {
        strokes: vec![BrushStroke {
            points: (0..n)
                .map(|i| [0.5 - k / 2.0 + k * i as f32 / (n - 1) as f32, 0.5])
                .collect(),
            radius: (k / 2.0).max(0.002),
            hardness: 50.0,
            flow: 100.0,
            auto_mask: gating,
            tolerance: 25.0,
            erase: false,
        }],
        invert: false,
    }
}

fn worst_disagreement(a: &CoverageMap, b: &CoverageMap) -> (i32, usize) {
    let (mut worst, mut differing) = (0i32, 0usize);
    for i in 0..a.data.len().min(b.data.len()) {
        let d = (i32::from(a.data[i]) - i32::from(b.data[i])).abs();
        if d > 0 {
            differing += 1;
        }
        worst = worst.max(d);
    }
    (worst, differing)
}

fn band(v: &[f64]) -> (f64, f64) {
    let mut lo = f64::MAX;
    let mut hi = f64::MIN;
    for &x in v {
        lo = lo.min(x);
        hi = hi.max(x);
    }
    (lo, hi)
}

fn loadavg() -> String {
    std::process::Command::new("/usr/bin/uptime")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "?".into())
}

/// One row of the sweep, for one gating choice.
struct Row {
    area: usize,
    cpu: (f64, f64),
    gpu: (f64, f64),
    ratio: (f64, f64),
    worst: i32,
    differing: usize,
    total: usize,
}

fn sweep(
    raster: &gpu_pipeline::GpuBrushRaster,
    base: &ImageBuf,
    geometry: &Geometry,
    gating: bool,
    ks: &[f32],
) -> Vec<Row> {
    let (w, h) = (base.width(), base.height());
    let b = ImgBase(base);
    let base_ref = BaseRef {
        plane: base.data(),
        w,
        h,
        x0: 0,
        y0: 0,
    };
    let mut rows = Vec::new();
    for &k in ks {
        let brush = sweep_brush(k, gating);
        let bp: Option<&dyn BasePixels> = if gating { Some(&b) } else { None };
        let Some(plan) = plan_dabs(&brush, geometry, w, h, None, bp) else {
            continue;
        };
        // ⚠ A gating brush that planned no gating rows would be silently measuring the
        // non-gating case — the fixture hazard `CLAUDE.md` names. Say so rather than average it in.
        if gating && plan.gating_rows.is_none() {
            println!("  k={k}: gating brush planned NO gating rows — skipped, it would be vacuous");
            continue;
        }
        let gpu_base = if gating { Some(base_ref) } else { None };

        let mut cpu_bests = Vec::new();
        let mut gpu_bests = Vec::new();
        let mut ratios = Vec::new();
        let mut agree = (0i32, 0usize, 0usize);
        for _block in 0..BLOCKS {
            let mut c = f64::MAX;
            let mut g = f64::MAX;
            for round in 0..=ROUNDS {
                let t = Instant::now();
                let cm = rasterize_in(&brush, geometry, w, h, None, bp);
                let cms = t.elapsed().as_secs_f64() * 1e3;

                let t = Instant::now();
                let gm = raster.rasterize(&plan, brush.invert, gpu_base);
                let gms = t.elapsed().as_secs_f64() * 1e3;

                if let Some(gm) = &gm {
                    let (wst, dif) = worst_disagreement(gm, &cm);
                    agree = (agree.0.max(wst), agree.1.max(dif), cm.data.len());
                } else {
                    println!("  k={k}: the GPU raster returned None — not comparable");
                }
                if round > 0 {
                    c = c.min(cms);
                    g = g.min(gms);
                }
            }
            cpu_bests.push(c);
            gpu_bests.push(g);
            ratios.push(c / g);
        }
        rows.push(Row {
            area: plan.w * plan.h,
            cpu: band(&cpu_bests),
            gpu: band(&gpu_bests),
            ratio: band(&ratios),
            worst: agree.0,
            differing: agree.1,
            total: agree.2,
        });
    }
    rows
}

fn print_rows(title: &str, rows: &[Row]) {
    println!("\n{title}");
    println!(
        "  {:>10}  {:>17}  {:>17}  {:>15}   agreement",
        "box px", "CPU ms (band)", "GPU ms (band)", "ratio (band)"
    );
    for r in rows {
        println!(
            "  {:>10}  {:>7.3}–{:<9.3}  {:>7.3}–{:<9.3}  {:>6.2}x–{:<7.2}x   worst {} lvl, {:.4}% differ",
            r.area,
            r.cpu.0,
            r.cpu.1,
            r.gpu.0,
            r.gpu.1,
            r.ratio.0,
            r.ratio.1,
            r.worst,
            if r.total == 0 {
                0.0
            } else {
                r.differing as f64 / r.total as f64 * 100.0
            },
        );
    }
    // The crossover: the smallest box whose whole ratio band sits at or above parity.
    let cross = rows.iter().find(|r| r.ratio.0 >= 1.0);
    let straddle: Vec<usize> = rows
        .iter()
        .filter(|r| r.ratio.0 < 1.0 && r.ratio.1 >= 1.0)
        .map(|r| r.area)
        .collect();
    match cross {
        Some(r) => println!(
            "  → whole band above parity from {} px ({:.2}x–{:.2}x)",
            r.area, r.ratio.0, r.ratio.1
        ),
        None => println!("  → no row's whole band reached parity in this ladder"),
    }
    if !straddle.is_empty() {
        println!("  → bands straddling parity at: {straddle:?}");
    }
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: box_floor_sweep <raw>");
    let path = std::path::Path::new(&path);

    println!("load before decode: {}", loadavg());

    let opts = raw_decode::DecodeOptions::preview();
    let d = raw_decode::decode_raw(path, &opts).expect("decode");
    let base = d.image.downscale_to(RUNG);
    let (w, h) = (base.width(), base.height());

    let ctx = match gpu_pipeline::shared() {
        Some(c) => c.clone(),
        None => {
            println!("no GPU adapter — nothing to compare");
            return;
        }
    };
    println!("frame {w}x{h} at rung {RUNG}  ({} px)", w * h);
    println!("adapter: {} ({})", ctx.adapter_name(), ctx.backend());
    println!(
        "MIN_GPU_BOX_PX as shipped: 524288 px  (read from brush_raster.rs, unchanged by this example)"
    );
    println!("blocks {BLOCKS} x (1 warm-up + best of {ROUNDS})");

    let raster = gpu_pipeline::GpuBrushRaster::new(&ctx);
    let geometry = Geometry::default();

    // A ladder that brackets the shipped floor from well below to well above.
    let ks: Vec<f32> = vec![
        0.03, 0.05, 0.07, 0.09, 0.11, 0.13, 0.16, 0.20, 0.24, 0.30, 0.40, 0.50,
    ];

    println!("\nload before sweep: {}", loadavg());
    let non_gating = sweep(&raster, &base, &geometry, false, &ks);
    println!("load between sweeps: {}", loadavg());
    let gating = sweep(&raster, &base, &geometry, true, &ks);
    println!("load after sweeps: {}", loadavg());

    print_rows(
        "NON-GATING brush (auto_mask off, no base upload):",
        &non_gating,
    );
    print_rows(
        "GATING brush (auto_mask on, base uploaded and gate evaluated):",
        &gating,
    );

    println!("\n⚠ Quote the BAND. A single figure from this example is unverified.");
}
