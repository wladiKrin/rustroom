//! Times the grade range view's plane — the one cost the view adds, paid only while it is on.
//!
//! Run: `cargo run --release -p app --example bench_grade_view <raw>`
//!
//! Two buffers: the fit rung (1600 px preview) and a 2000×1400 crop of the half-size decode,
//! the shape of a native-rung region. The tap itself adds nothing — the halves are the tail —
//! so `coverage` is the whole cost.
use rustroom_app::grade_view::{coverage, GradeRangeView};
use std::path::PathBuf;
use std::time::Instant;

fn main() {
    let path = PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("usage: bench_grade_view <raw>"),
    );
    let decoded =
        raw_decode::decode_raw(&path, &raw_decode::DecodeOptions::preview()).expect("decode");
    let full = decoded.image;
    let fit = full.downscale_to(1600);
    let (cw, ch) = (2000.min(full.width()), 1400.min(full.height()));
    let region = full.crop((full.width() - cw) / 2, (full.height() - ch) / 2, cw, ch);
    for (name, buf) in [("fit", &fit), ("native region", &region)] {
        let mut best = f64::MAX;
        for _ in 0..5 {
            let t = Instant::now();
            let p = coverage(buf, GradeRangeView::Midtones, 50.0, 0.0);
            best = best.min(t.elapsed().as_secs_f64() * 1000.0);
            std::hint::black_box(p);
        }
        println!(
            "{name:>14} {}x{}: coverage {best:.2} ms (best of 5)",
            buf.width(),
            buf.height()
        );
    }
}
