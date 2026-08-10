//! How long does a folder of thumbnails take, one thread vs the shipped worker count?
//!
//! The companion to `bench_preview`. Run it when touching `ThumbLoader` or `worker_count`:
//! `cargo run --release -p app --example bench_thumbs <folder>`.
use rustroom_app::thumb_loader::{worker_count, ThumbLoader};
use std::time::Instant;

fn main() {
    let dir = std::env::args()
        .nth(1)
        .expect("usage: bench_thumbs <folder>");
    let entries = catalog::scan_folder(std::path::Path::new(&dir)).expect("scan");
    let paths: Vec<_> = entries.iter().map(|e| e.path.clone()).take(120).collect();
    let n = paths.len();
    println!("{n} entries, worker_count() = {}", worker_count());

    for threads in [1, worker_count()] {
        let t = Instant::now();
        let loader = ThumbLoader::with_loader(
            paths.clone(),
            |p| {
                if catalog::is_raw(p) {
                    raw_decode::extract_thumbnail(p).ok()
                } else {
                    image_decode::thumbnail(p).ok()
                }
            },
            threads,
            || {},
        );
        let mut got = 0;
        while got < n {
            got += loader.drain().len();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        println!(
            "{threads:2} thread(s): {ms:8.0} ms  ({:.1} ms/thumb)",
            ms / n as f64
        );
    }
}
