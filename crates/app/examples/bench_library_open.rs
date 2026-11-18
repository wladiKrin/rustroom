//! How much does a folder cost to browse — thumbnails resident, and time to a usable grid?
//!
//! The companion to `bench_thumbs`. Run it when touching `ThumbLoader`, `ThumbCache` or the
//! grid's windowing: `cargo run --release -p app --example bench_library_open <folder>`.
use rustroom_app::thumb_cache::{ThumbCache, THUMB_BUDGET_BYTES};
use rustroom_app::thumb_loader::ThumbLoader;
use std::path::PathBuf;
use std::time::Instant;

fn main() {
    let dir = std::env::args()
        .nth(1)
        .expect("usage: bench_library_open <folder>");
    let entries = catalog::scan_folder(std::path::Path::new(&dir)).expect("scan");
    let paths: Vec<PathBuf> = entries.iter().map(|e| e.path.clone()).collect();
    println!(
        "{} entries; budget {} MB",
        paths.len(),
        THUMB_BUDGET_BYTES >> 20
    );

    // What a first screenful costs — the number that decides whether a folder feels instant —
    // against what loading the whole folder costs, which is what the browser used to do on open.
    for window in [24usize, 120, paths.len()] {
        let window = window.min(paths.len());
        let wanted: Vec<rustroom_app::thumb_loader::ThumbRequest> = paths[..window]
            .iter()
            .map(|p| rustroom_app::thumb_loader::ThumbRequest {
                id: rustroom_app::photo_id::PhotoId::original(p.clone()),
                edits: Default::default(),
            })
            .collect();
        let mut cache: ThumbCache<usize> = ThumbCache::with_default_budget();
        let t = Instant::now();
        let loader = ThumbLoader::new(color::Display::SRGB, u64::MAX, u64::MAX, || {});
        loader.want(&wanted);
        let mut got = 0usize;
        while got < window && t.elapsed().as_secs() < 300 {
            for r in loader.drain() {
                if let Some(th) = r.thumb {
                    let bytes = th.rgba.len();
                    cache.insert(r.key, bytes, bytes);
                }
                got += 1;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let keys: Vec<_> = wanted.iter().map(|r| r.key()).collect();
        cache.retain_window(&keys);
        println!(
            "window {window:5}: {:6.2} s, {:5} MB resident",
            t.elapsed().as_secs_f64(),
            cache.bytes() >> 20
        );
    }
}
