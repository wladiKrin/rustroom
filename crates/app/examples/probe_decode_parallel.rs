//! Is a batch export leaving the machine idle?
//!
//! ⚠⚠ **Written before any design, because the next milestone's whole value turns on it.** M8.9
//! measured a per-photo export as ~90% decode — 10.78 s of an 11.4 s export at the `Best` tier —
//! and `batch_export` runs **one photo at a time by design**, to bound peak memory at roughly one
//! 26 Mpx image. If libraw's decode is single-threaded, that design leaves seven of eight cores
//! idle for 90% of a batch, and decoding several photos at once is worth several times anything
//! left on the GPU. If it is already parallel, there is nothing there and the idea is dead.
//!
//! Run: `cargo run --release -p app --example probe_decode_parallel <raw>...`

use std::time::Instant;

fn main() {
    let files: Vec<std::path::PathBuf> = std::env::args().skip(1).map(Into::into).collect();
    assert!(
        files.len() >= 2,
        "usage: probe_decode_parallel <raw> <raw> [...]  (at least two distinct files)"
    );
    let opts = raw_decode::DecodeOptions::linear_scene();
    println!(
        "{} files, {} threads available, demosaic {:?}",
        files.len(),
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(0),
        opts.demosaic
    );

    // One file alone, so the rows below have a baseline that is not itself contended.
    let t = Instant::now();
    let d = raw_decode::decode_raw(&files[0], &opts).expect("decode");
    let one = t.elapsed().as_secs_f64();
    println!(
        "{:8.2} s   ONE decode alone ({}x{})",
        one,
        d.image.width(),
        d.image.height()
    );
    drop(d);

    // ⚠ Distinct files, so nothing is served from a warm page cache that the sequential row did
    // not also get — and so libraw cannot share any per-file state.
    let n = files.len();
    let t = Instant::now();
    for f in &files {
        let d = raw_decode::decode_raw(f, &opts).expect("decode");
        std::hint::black_box(&d.image);
    }
    let seq = t.elapsed().as_secs_f64();
    println!(
        "{seq:8.2} s   {n} decodes SEQUENTIALLY ({:.2} s each)",
        seq / n as f64
    );

    let t = Instant::now();
    std::thread::scope(|s| {
        for f in &files {
            s.spawn(|| {
                let d = raw_decode::decode_raw(f, &opts).expect("decode");
                std::hint::black_box(&d.image);
            });
        }
    });
    let par = t.elapsed().as_secs_f64();
    println!(
        "{par:8.2} s   {n} decodes CONCURRENTLY ({:.2} s each)",
        par / n as f64
    );
    println!(
        "\n         speed-up from running them at once: {:.2}x",
        seq / par
    );
    println!(
        "         peak buffers held at once: {} x {:.0} MiB",
        n,
        (6246.0 * 4170.0 * 3.0 * 4.0) / (1024.0 * 1024.0)
    );
}
