//! Import / ingest (M9.12): copying photos off a card into dated folders, with a backup copy and
//! metadata and a develop preset applied on entry.
//!
//! ⚠ No egui anywhere here — the dialog and the flow live in the bin (`modals::import`,
//! `import_flow`). The card is only ever READ: nothing in this module opens a source for writing.

pub mod copy;
pub mod entry;
pub mod options;
pub mod plan;
pub mod source;
pub mod worker;

/// A per-test temp directory: process id plus a counter, never the clock or the pid alone
/// (`thumb_store::TempDir`'s lesson).
#[cfg(test)]
pub(crate) fn temp_dir(tag: &str) -> std::path::PathBuf {
    static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let p = std::env::temp_dir().join(format!(
        "rustroom_ingest_{tag}_{}_{}",
        std::process::id(),
        N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}
