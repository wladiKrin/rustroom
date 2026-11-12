//! What does a brush's GPU rasterisation cost end to end — upload, raster, readback — at the rungs a mask renders at?
//!
//! `cargo run --release -p gpu-pipeline --example bench_coverage_wire`
//!
//! ⚠ M10.14g's before/after: the readback used to carry one `u32` per sample and narrow on the CPU; it now carries
//! the bytes. Constructed: one long soft stroke across the frame, so the box is most of it. Best of 10 warm runs.

use edit_model::{Brush, BrushStroke, Geometry};
use std::time::Instant;

fn main() {
    let Some(ctx) = gpu_pipeline::shared() else {
        eprintln!("no GPU adapter");
        return;
    };
    let brush = Brush {
        strokes: vec![BrushStroke {
            points: (0..60).map(|i| [0.05 + i as f32 * 0.015, 0.5]).collect(),
            radius: 0.35,
            hardness: 20.0,
            ..Default::default()
        }],
        invert: false,
    };
    let raster = gpu_pipeline::GpuBrushRaster::new(ctx);
    println!("tile capacity {}", gpu_pipeline::max_tile_dabs());
    for (w, h) in [(1600, 1067), (3200, 2133), (6246, 4170)] {
        let plan = pipeline::brush::plan_dabs(&brush, &Geometry::default(), w, h, None, None)
            .expect("dabs");
        let mut best = f64::MAX;
        let mut px = 0;
        for r in 0..11 {
            let t = Instant::now();
            let m = raster.rasterize(&plan, false, None).expect("raster");
            let ms = t.elapsed().as_secs_f64() * 1e3;
            px = m.w * m.h;
            if r > 0 {
                best = best.min(ms);
            }
        }
        println!("{w}x{h}: box {:.2} Mpx, {best:.2} ms", px as f64 / 1e6);
    }
}
