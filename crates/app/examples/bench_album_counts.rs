//! What one Library frame spends on the Immich rail's album counts, on a real server's stored lists
//! (2026-10-12). Every album row asks for its count on every frame.
//!
//! `cargo run --release -p app --example bench_album_counts -- <cache>/immich/<host>-lists`
//!
//! Three ways, each over every stored album list in the directory:
//! - **before**: the stored list read and parsed, catalog entries built (a sidecar `stat` per asset);
//! - **no stat**: the stored list read and parsed, the shown names derived and counted;
//! - **now**: the names derived once (`immich_album_names`), only looked up per frame.

use rustroom_app::immich_list_cache as lc;
use rustroom_app::immich_source as src;
use std::time::Instant;

fn main() {
    let dir = std::path::PathBuf::from(
        std::env::args()
            .nth(1)
            .expect("usage: bench_album_counts <lists dir>"),
    );
    let name = dir.file_name().unwrap().to_string_lossy().to_string();
    let root = dir.with_file_name(name.trim_end_matches("-lists"));
    let present = src::present_names(&root);

    // Every stored album, with the validator it was stored under — what a current rail would give.
    let mut albums = Vec::new();
    for e in std::fs::read_dir(&dir).expect("lists dir") {
        let p = e.unwrap().path();
        let f = p.file_name().unwrap().to_string_lossy().to_string();
        let Some(id) = f
            .strip_prefix("album-")
            .and_then(|s| s.strip_suffix(".json"))
        else {
            continue;
        };
        let v: serde_json::Value = serde_json::from_slice(&std::fs::read(&p).unwrap()).unwrap();
        let want: lc::Validator = serde_json::from_value(v["validator"].clone()).unwrap();
        albums.push((lc::Key::Album(id.to_string()), want));
    }
    let load = |(k, v): &(lc::Key, lc::Validator)| lc::load(&dir, k, v).expect("current");
    let assets: usize = albums.iter().map(|a| load(a).len()).sum();
    println!(
        "{} albums, {assets} assets, {} downloaded files",
        albums.len(),
        present.len()
    );

    let frames = 20;
    let time = |label: &str, f: &mut dyn FnMut() -> usize| {
        let t = Instant::now();
        let mut have = 0;
        for _ in 0..frames {
            have = f();
        }
        let ms = t.elapsed().as_secs_f64() * 1e3 / frames as f64;
        println!("{label:>9}: {ms:8.3} ms per frame ({have} downloaded)");
    };
    time("before", &mut || {
        albums
            .iter()
            .map(|a| {
                src::entries_from_assets(&load(a), &root)
                    .iter()
                    .filter(|e| e.path.file_name().is_some_and(|n| present.contains(n)))
                    .count()
            })
            .sum()
    });
    time("no stat", &mut || {
        albums
            .iter()
            .map(|a| src::count_present(&src::shown_names(&load(a)), &present).0)
            .sum()
    });
    let names: Vec<_> = albums.iter().map(|a| src::shown_names(&load(a))).collect();
    time("now", &mut || {
        names
            .iter()
            .map(|n| src::count_present(n, &present).0)
            .sum()
    });
}
