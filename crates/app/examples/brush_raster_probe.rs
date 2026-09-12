//! Does the GPU brush rasteriser agree with the CPU one on a **real** brush, and is it faster?
//!
//! ⚠⚠ **A synthetic fixture cannot stand in for this one.** The catalogue's masked photograph
//! carries **36 strokes, 1,387 points, ~3,645 dabs, 22 of the strokes auto-masked**, over a box of
//! about 1510×2019 — 3.05 Mpx. Every test in `gpu-pipeline` runs on a 400×300 fixture, so this is
//! the only thing that exercises the tile scan at a scale where the shared list, the prefix sum
//! and the gate all matter at once.
//!
//! ⚠ **An example, not a test.** It needs a real RAW *and* a real sidecar, and a fixture-gated
//! test is no evidence in CI — the rule `RAW_FIXTURE` already follows. It also prints timings, and
//! a benchmark is not a test.
//!
//! ⚠ It lives in `app` rather than `gpu-pipeline` because that crate has no `raw-decode` and its
//! own header says nothing may depend on it; `app` already has the decoder and the sidecar reader,
//! and `mask_breakdown` and `bench_gpu_tail` both live here for the same reason.
//!
//! `cargo run --release -p app --example brush_raster_probe <raw>`

use core_image::ImageBuf;
use edit_model::{Brush, Geometry};
use pipeline::brush::{plan_dabs, rasterize_in, BasePixels, CoverageMap};
use std::time::Instant;

/// The rung every masked figure in `CLAUDE.md` is quoted at.
const RUNG: usize = 3200;

/// The decoded buffer seen as a base for the auto-mask gate.
///
/// ⚠ The CPU and the GPU are handed **the same plane** — this wrapper for one, `data()` for the
/// other — so any disagreement is the shader's and not the fixture's.
struct ImgBase<'a>(&'a ImageBuf);
impl BasePixels for ImgBase<'_> {
    fn at(&self, x: usize, y: usize) -> [f32; 3] {
        self.0.pixel(x, y)
    }
}

fn worst_disagreement(a: &CoverageMap, b: &CoverageMap) -> (i32, usize, (usize, usize)) {
    assert_eq!(
        (a.x0, a.y0, a.w, a.h),
        (b.x0, b.y0, b.w, b.h),
        "boxes differ"
    );
    let (mut worst, mut differing, mut at) = (0i32, 0usize, (0usize, 0usize));
    for y in 0..a.h {
        for x in 0..a.w {
            let i = y * a.w + x;
            let d = (i32::from(a.data[i]) - i32::from(b.data[i])).abs();
            if d > 0 {
                differing += 1;
            }
            if d > worst {
                worst = d;
                at = (x, y);
            }
        }
    }
    (worst, differing, at)
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: brush_raster_probe <raw>");
    let path = std::path::Path::new(&path);

    let side = path.with_file_name(format!(
        "{}.json",
        path.file_name().unwrap().to_string_lossy()
    ));
    let text = std::fs::read_to_string(&side).expect("no sidecar beside that RAW");
    let v: serde_json::Value = serde_json::from_str(&text).expect("sidecar is not JSON");
    let edits = v
        .get("copies")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("edits"))
        .expect("sidecar has no copy 0");
    let edits: edit_model::Edits =
        serde_json::from_value(edits.clone()).expect("edits do not parse");

    let brush: Brush = edits
        .masks
        .iter()
        .find_map(|m| match &m.mask {
            edit_model::Mask::Brush(b) => Some(b.clone()),
            _ => None,
        })
        .expect("that sidecar carries no brush mask — this probe needs one");
    let geometry: Geometry = edits.geometry;

    let opts = raw_decode::DecodeOptions::preview();
    let d = raw_decode::decode_raw(path, &opts).expect("decode");
    // ⚠ Downscaled to the rung, because a dab's radius is a fraction of the **frame's** long edge:
    // rasterising at the buffer's own size would measure a different box and a different dab count.
    let base = d.image.downscale_to(RUNG);
    let (w, h) = (base.width(), base.height());

    let ctx = match gpu_pipeline::shared() {
        Some(c) => c.clone(),
        None => {
            println!("no GPU adapter — nothing to compare");
            return;
        }
    };
    println!("frame {w}x{h} at rung {RUNG}");
    println!("adapter: {} ({})", ctx.adapter_name(), ctx.backend());
    println!(
        "brush: {} strokes, {} points, {} auto-masked",
        brush.strokes.len(),
        brush.strokes.iter().map(|s| s.points.len()).sum::<usize>(),
        brush.strokes.iter().filter(|s| s.auto_mask).count()
    );

    let b = ImgBase(&base);
    let plan = plan_dabs(&brush, &geometry, w, h, None, Some(&b)).expect("the brush plans no dabs");
    println!(
        "plan:  box {}x{} at ({},{}), {} dabs, gating rows {:?}\n",
        plan.w,
        plan.h,
        plan.x0,
        plan.y0,
        plan.strokes.iter().map(|s| s.dabs.len()).sum::<usize>(),
        plan.gating_rows.map(|(a, z)| z - a),
    );

    let raster = gpu_pipeline::GpuBrushRaster::new(&ctx);

    // ⚠ Best of 5 **warm** rounds, round 0 discarded — the first GPU encode compiles the shader,
    // and the CPU's first pass warms its own caches. The same rule `bench_gpu_tail` follows.
    let mut cpu_best = f64::MAX;
    let mut gpu_best = f64::MAX;
    let mut cpu_map = None;
    let mut gpu_map = None;
    let mut occupancy = 0u32;
    for round in 0..5 {
        let t = Instant::now();
        let cpu = rasterize_in(&brush, &geometry, w, h, None, Some(&b));
        let cpu_ms = t.elapsed().as_secs_f64() * 1e3;

        let t = Instant::now();
        let (gpu, occ) = raster
            .rasterize_with_occupancy(
                &plan,
                brush.invert,
                Some(pipeline::brush::BaseRef {
                    plane: base.data(),
                    w,
                    h,
                    x0: 0,
                    y0: 0,
                }),
            )
            .expect("the GPU raster failed");
        let gpu_ms = t.elapsed().as_secs_f64() * 1e3;
        occupancy = occupancy.max(occ);

        if round > 0 {
            cpu_best = cpu_best.min(cpu_ms);
            gpu_best = gpu_best.min(gpu_ms);
        }
        cpu_map = Some(cpu);
        gpu_map = Some(gpu);
    }
    let (cpu, gpu) = (cpu_map.unwrap(), gpu_map.unwrap());

    let (worst, differing, at) = worst_disagreement(&gpu, &cpu);
    let total = cpu.w * cpu.h;
    println!("agreement over all {total} samples of the box:");
    println!("  worst {worst} level(s) at {at:?}");
    println!(
        "  {differing} samples differ ({:.4}%)",
        differing as f64 / total as f64 * 100.0
    );
    println!();
    println!(
        "busiest tile held {occupancy} dabs, against a capacity of {} — {:.0}x headroom",
        gpu_pipeline::max_tile_dabs(),
        f64::from(gpu_pipeline::max_tile_dabs()) / f64::from(occupancy.max(1)),
    );
    // ⚠⚠ **Isolating what plan 1 pays and plan 2 would not: the base upload.** The same brush with
    // its gates off plans the same dabs over the same box, but uploads no base plane and evaluates
    // no gate — so the difference bounds what sharing M8.3j's already-resident buffer could
    // recover. It is an upper bound rather than the figure, because it also drops the gate itself.
    let mut ungated = brush.clone();
    for s in &mut ungated.strokes {
        s.auto_mask = false;
    }
    let flat = plan_dabs(&ungated, &geometry, w, h, None, None).expect("no dabs");
    let mut flat_best = f64::MAX;
    for round in 0..5 {
        let t = Instant::now();
        raster
            .rasterize(&flat, ungated.invert, None)
            .expect("positional raster failed");
        let ms = t.elapsed().as_secs_f64() * 1e3;
        if round > 0 {
            flat_best = flat_best.min(ms);
        }
    }

    println!();
    println!("  CPU  {cpu_best:7.2} ms");
    println!(
        "  GPU, no base upload and no gate  {flat_best:7.2} ms           (so upload + gate is about {:.1} ms of the GPU figure below)",
        gpu_best - flat_best
    );
    println!("  GPU  {gpu_best:7.2} ms   {:.2}x", cpu_best / gpu_best);
    println!();
    println!(
        "⚠ Quote a BAND over several runs, not one figure — M8.3h had to retract a headline for\n\
         ⚠ quoting the top of a band as the value. ⚠ A worst of 0 or 1 level is agreement; a\n\
         ⚠ systematic difference would show as a large `differing` share, so read both."
    );
}
