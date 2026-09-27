//! M9.12 probe: the import survey's fast path (`image_decode::quick_look`) against the full read
//! (`raw_decode::survey` / `image_decode::read_capture` + `thumbnail`), on real files.
//!
//! `cargo run --release -p app --example import_survey_probe -- <dir> [limit]`
//!
//! Prints how many files each path answered, whether the capture times agree, the thumbnail sizes,
//! the time per file, and the bytes each path reads — the number that decides a card's speed.

use std::path::PathBuf;
use std::time::Instant;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let dir = PathBuf::from(&a[1]);
    let limit: usize = a.get(2).and_then(|s| s.parse().ok()).unwrap_or(200);
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| catalog::is_supported(p))
        .collect();
    files.sort();
    files.truncate(limit);
    for kind in ["RAF", "JPG"] {
        let set: Vec<&PathBuf> = files
            .iter()
            .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case(kind)))
            .collect();
        if set.is_empty() {
            continue;
        }
        let (mut fast_ok, mut thumbs, mut agree, mut fast_bytes, mut full_bytes) =
            (0, 0, 0, 0u64, 0u64);
        let mut sizes = std::collections::BTreeMap::new();
        let t = Instant::now();
        let fast: Vec<_> = set
            .iter()
            .map(|p| image_decode::quick_look(p).ok().flatten())
            .collect();
        let fast_t = t.elapsed();
        let t = Instant::now();
        let full: Vec<Option<i64>> = set
            .iter()
            .map(|p| {
                if kind == "RAF" {
                    raw_decode::survey(p)
                        .ok()
                        .and_then(|(c, _)| c)
                        .and_then(|c| c.taken)
                } else {
                    let _ = image_decode::thumbnail(p);
                    image_decode::read_capture(p).ok().and_then(|c| c.taken)
                }
            })
            .collect();
        let full_t = t.elapsed();
        for ((p, f), taken) in set.iter().zip(&fast).zip(&full) {
            full_bytes += std::fs::metadata(p).unwrap().len();
            if let Some(q) = f {
                fast_ok += 1;
                agree += usize::from(q.capture.taken == *taken);
                if let Some(th) = &q.thumb {
                    thumbs += 1;
                    *sizes.entry((th.width, th.height)).or_insert(0) += 1;
                }
                // Bytes the fast path touched: the RAF header, then the JPEG's segment headers
                // and its EXIF block — bounded above by 148 + 4 + 64 KiB + a few headers.
                fast_bytes += 148 + 70_000;
            }
        }
        let n = set.len();
        println!(
            "{kind}: {n} files | fast path answered {fast_ok}, thumbnails {thumbs} {sizes:?}, \
             capture agrees with the full read {agree}/{fast_ok}"
        );
        println!(
            "   time/file: fast {:.2} ms, full {:.1} ms | bytes/file: fast ≤{:.0} KB, full {:.1} MB",
            fast_t.as_secs_f64() * 1e3 / n as f64,
            full_t.as_secs_f64() * 1e3 / n as f64,
            fast_bytes as f64 / fast_ok.max(1) as f64 / 1e3,
            full_bytes as f64 / n as f64 / 1e6
        );
    }
}
