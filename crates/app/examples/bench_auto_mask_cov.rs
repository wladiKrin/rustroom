//! What does an auto-masked brush's coverage cost, and how much of a render is it?
//!
//! Run: `cargo run --release -p app --example bench_auto_mask_cov <raw>`
//!
//! ⚠⚠ **Measure before designing a cache for it.** `BrushCache` declines an auto-mask brush, so its
//! coverage is re-rasterised on every render — and `RUSTROOM_DEBUG_RENDER` shows a masked frame
//! costing 19-156 ms against ~21 ms unmasked. That says a mask is expensive; it does **not** say
//! how much of it is the coverage. `docs/BACKLOG.md` records a confidently-stated cost hypothesis
//! in this exact area that measurement overturned by 6x.
//!
//! ⚠ Clipped to a region, as the canvas renders at 1:1 — the unclipped number is a different
//! question and is measured separately, because whether the region belongs in the cache key is
//! precisely what a cache design has to decide.
use core_image::ImageBuf;
use edit_model::{Brush, BrushStroke, Edits};
use std::time::Instant;

const FIT_CAP: usize = 1600;
/// The reporter's canvas and zoom, from `RUSTROOM_DEBUG_PLACE`.
const AVAIL: (f32, f32) = (2167.2, 1229.5);
const FIT: f32 = 1.151217;
const ZOOM: f32 = 7.7168;

fn best_of<T>(n: usize, mut f: impl FnMut() -> T) -> (T, f64) {
    let mut best = f64::INFINITY;
    let mut out = None;
    for _ in 0..n {
        let t = Instant::now();
        let v = f();
        best = best.min(t.elapsed().as_secs_f64() * 1000.0);
        out = Some(v);
    }
    (out.unwrap(), best)
}

/// A stroke across the middle of the frame, in pre-geometry normalised coordinates.
fn stroke(auto_mask: bool) -> BrushStroke {
    BrushStroke {
        points: (0..40).map(|i| [0.3 + i as f32 * 0.01, 0.5]).collect(),
        radius: 0.04,
        hardness: 50.0,
        flow: 100.0,
        erase: false,
        auto_mask,
        tolerance: 35.0,
    }
}

/// The rendered image, as a `BasePixels` the gate can read.
struct Base<'a>(&'a ImageBuf);
impl pipeline::brush::BasePixels for Base<'_> {
    fn at(&self, x: usize, y: usize) -> [f32; 3] {
        if self.0.width() == 0 || self.0.height() == 0 {
            return [0.0; 3];
        }
        self.0
            .pixel(x.min(self.0.width() - 1), y.min(self.0.height() - 1))
    }
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("usage: bench_auto_mask_cov <raw>");
    let opts = raw_decode::DecodeOptions::linear_scene();
    let d = raw_decode::decode_raw(std::path::Path::new(&path), &opts).expect("decode");
    let buf = std::sync::Arc::new(d.image);
    let buffer_long = buf.width().max(buf.height());
    println!("buffer {}x{}", buf.width(), buf.height());

    let rung = buffer_long;
    let k = rung as f32 / FIT_CAP as f32;
    let px_per_rendered = FIT * ZOOM / k;
    let src = ImageBuf::clone(&buf).downscale_to(rung);
    let (fw, fh) = (src.width(), src.height());
    let rw = ((AVAIL.0 / px_per_rendered).ceil() as usize + 512).min(fw);
    let rh = ((AVAIL.1 / px_per_rendered).ceil() as usize + 512).min(fh);
    let (x0, y0) = ((fw - rw) / 2, (fh - rh) / 2);
    let clip = Some((x0, y0, rw, rh));
    println!(
        "rung {rung}, frame {fw}x{fh}, region {rw}x{rh} ({:.2} Mpx) — the canvas's own margin included\n",
        (rw * rh) as f64 / 1e6
    );

    let geometry = Edits::default().geometry;
    let base = Base(&src);

    let plain = Brush {
        strokes: vec![stroke(false)],
        invert: false,
    };
    let auto = Brush {
        strokes: vec![stroke(true)],
        invert: false,
    };

    let (_, plain_clipped) = best_of(5, || {
        pipeline::brush::rasterize_in(&plain, &geometry, fw, fh, clip, None)
    });
    let (_, auto_clipped) = best_of(5, || {
        pipeline::brush::rasterize_in(&auto, &geometry, fw, fh, clip, Some(&base))
    });
    let (_, plain_whole) = best_of(3, || {
        pipeline::brush::rasterize_in(&plain, &geometry, fw, fh, None, None)
    });
    let (_, auto_whole) = best_of(3, || {
        pipeline::brush::rasterize_in(&auto, &geometry, fw, fh, None, Some(&base))
    });

    println!(
        "{:<34}{:>12}{:>12}",
        "rasterize_in", "clipped", "whole frame"
    );
    println!(
        "{:<34}{plain_clipped:>10.2} ms{plain_whole:>10.2} ms",
        "plain brush"
    );
    println!(
        "{:<34}{auto_clipped:>10.2} ms{auto_whole:>10.2} ms",
        "auto-mask brush"
    );
    println!(
        "\nThe gate costs {:.2} ms clipped ({:.1}x the plain rasterisation).",
        auto_clipped - plain_clipped,
        auto_clipped / plain_clipped.max(1e-9)
    );
    println!(
        "⚠ Whole-frame is what a cache keyed WITHOUT the region would have to build: {:.1}x the\n\
         clipped cost, paid once instead of on every region change.",
        auto_whole / auto_clipped.max(1e-9)
    );
}
