//! How long does reading every sidecar in a folder take?
//!
//! ⚠ M3.4 makes folder-open read one sidecar per file, where it previously read none. The design
//! committed to measuring that rather than assuming it was free.
//!
//! `cargo run --release -p app --example sidecar_scan <folder>`
fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).expect("usage: sidecar_scan <dir>"));
    let entries = catalog::scan_folder(&dir).expect("scan");

    let t = std::time::Instant::now();
    let mut copies = 0usize;
    for e in &entries {
        copies += rustroom_app::sidecar::load_all(&e.path).copies.len();
    }
    let ms = t.elapsed().as_secs_f64() * 1000.0;

    println!(
        "{} files, {copies} stored copies, {ms:.1} ms ({:.3} ms/file)",
        entries.len(),
        ms / entries.len().max(1) as f64
    );
}
