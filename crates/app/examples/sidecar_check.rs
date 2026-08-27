//! Read every sidecar in a folder and report what the current code makes of it.
//!
//! ⚠ A backward-compatibility check against **real** files, which no unit test fixture can be:
//! these were written by earlier builds and are the only evidence that adding a field to
//! `CopyEntry` did not silently discard anyone's edits.
//!
//! `cargo run --release -p app --example sidecar_check <folder>`
fn main() {
    let dir = std::env::args()
        .nth(1)
        .expect("usage: sidecar_check <folder>");
    let mut files = 0;
    let mut copies = 0;
    let mut nonneutral = 0;
    let mut snapshots = 0;
    for e in std::fs::read_dir(&dir).expect("read_dir").flatten() {
        let p = e.path();
        if p.extension().is_none_or(|x| x != "json") {
            continue;
        }
        // The sidecar's own path is `<photo>.json`, so the photo is the stem.
        let photo = p.with_extension("");
        let s = rustroom_app::sidecar::load_all(&photo);
        files += 1;
        if s.copies.is_empty() {
            println!("EMPTY (would be data loss): {}", p.display());
            continue;
        }
        for c in &s.copies {
            copies += 1;
            if c.edits != edit_model::Edits::default() {
                nonneutral += 1;
            }
            snapshots += c.snapshots.len();
        }
    }
    println!(
        "{files} sidecars, {copies} copies, {nonneutral} with real edits, {snapshots} snapshots"
    );
    assert!(files > 0, "no sidecars found — the check proved nothing");
    assert_eq!(
        nonneutral.min(1),
        1,
        "every sidecar read as neutral, which is what data loss looks like"
    );
}
