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

/// A fresh directory of its own — `test_dir` makes it unique per test and per run.
#[cfg(test)]
pub(crate) fn temp_dir(tag: &str) -> std::path::PathBuf {
    test_dir::path(&format!("ingest-{tag}"))
}
