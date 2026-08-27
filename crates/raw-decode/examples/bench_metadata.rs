//! How long does reading a RAW's capture metadata take, without decoding it?
//!
//! The number that decides whether a folder scan can afford to read it inline.
//! `cargo run --release -p raw-decode --example bench_metadata <folder> [n]`
use std::time::Instant;

fn main() {
    let dir = std::env::args()
        .nth(1)
        .expect("usage: bench_metadata <folder> [n]");
    let n: usize = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(40);
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .expect("read_dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("raf")))
        .collect();
    paths.sort();
    paths.truncate(n);
    println!("{} RAW files", paths.len());

    let mut bytes_total = 0u64;
    let t_read = Instant::now();
    let bufs: Vec<Vec<u8>> = paths
        .iter()
        .map(|p| {
            let b = std::fs::read(p).unwrap();
            bytes_total += b.len() as u64;
            b
        })
        .collect();
    let read_ms = t_read.elapsed().as_secs_f64() * 1000.0;
    println!(
        "read       : {read_ms:8.0} ms  ({:.1} ms/file, {} MB total)",
        read_ms / paths.len() as f64,
        bytes_total >> 20
    );

    let t_open = Instant::now();
    let mut got = 0usize;
    for b in &bufs {
        if let Ok(raw) = rsraw::RawImage::open(b) {
            let _ = raw.datetime();
            let _ = raw.normalized_make().to_string();
            let _ = raw.normalized_model().to_string();
            let _ = raw.iso_speed();
            got += 1;
        }
    }
    let open_ms = t_open.elapsed().as_secs_f64() * 1000.0;
    println!(
        "libraw open: {open_ms:8.0} ms  ({:.1} ms/file), {got} parsed",
        open_ms / paths.len() as f64
    );
    let per = (read_ms + open_ms) / paths.len() as f64;
    println!(
        "TOTAL      : {per:.1} ms/file  =>  830 files ≈ {:.1} s",
        per * 830.0 / 1000.0
    );

    // Does libraw need the whole file, or only the header? The read dominates, so this is the
    // question that decides whether a scan can afford metadata inline.
    use std::io::Read;
    for cap in [64usize * 1024, 256 * 1024, 1024 * 1024, 4 * 1024 * 1024] {
        let t = Instant::now();
        let mut ok = 0usize;
        let mut dated = 0usize;
        for p in &paths {
            let mut buf = vec![0u8; cap];
            let mut f = std::fs::File::open(p).unwrap();
            let mut n = 0usize;
            while n < cap {
                match f.read(&mut buf[n..]) {
                    Ok(0) => break,
                    Ok(k) => n += k,
                    Err(_) => break,
                }
            }
            buf.truncate(n);
            if let Ok(raw) = rsraw::RawImage::open(&buf) {
                ok += 1;
                if raw.datetime().is_some() && !raw.normalized_model().is_empty() {
                    dated += 1;
                }
            }
        }
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        println!(
            "head {:5} KB: {ms:8.0} ms ({:.2} ms/file) — {ok}/{} opened, {dated} with date+model  => 830 ≈ {:.2} s",
            cap >> 10,
            ms / paths.len() as f64,
            paths.len(),
            ms / paths.len() as f64 * 830.0 / 1000.0
        );
    }
}
