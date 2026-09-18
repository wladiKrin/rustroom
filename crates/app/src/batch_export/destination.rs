//! Where an exported file goes, and what happens when something is already there.
//!
//! ⚠ Split out of `batch_export.rs` on 2026-09-18 as a **pure move** — the `prefix_cache.rs`
//! precedent, with the test count unchanged and no golden moved. Every `⚠` below is the record of
//! a decision or a bug; **do not reword one.**

use crate::filename_template::{NameFacts, Template};
use export::ExportFormat;
use std::path::{Path, PathBuf};

/// What to do about one destination that already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Collision {
    Skip,
    Overwrite,
    KeepBoth,
}

/// The user's answer to a collision prompt.
///
/// ⚠ `Cancel` carries **no** `apply_to_all`, on purpose: ticking "apply to all" and then
/// cancelling must arm nothing, and making that unrepresentable is better than enforcing it
/// with a rule someone can forget. Compare `Settings::apply_delete_confirm`, where the flag
/// and the outcome are separate values held together only by a test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Resolve {
        collision: Collision,
        apply_to_all: bool,
    },
    Cancel,
}

/// The destination for one source file: `<dir>/<stem>.jpg`, or `<dir>/<stem>-N.jpg`.
///
/// A source with no file stem falls back to `export`, so a pathological path still yields a
/// writable name rather than panicking.
/// Where one export lands.
///
/// ⚠ **Two numbers, two meanings, two separators.** `_N` is which virtual copy this is — a
/// property of the photo, stable forever, and only the *default* for an unnamed copy. `-N` is how
/// many files already wanted this name in this folder — a property of the destination, assigned at
/// export time. Merging them into one scheme would make `a-2.jpg` ambiguous between "copy 2" and
/// "the third file called a", and would silently shift a copy's number whenever its plain name
/// happened to be taken.
///
/// ⚠ Two copies can be given the *same* name; nothing prevents it, and `-N` is what stops them
/// overwriting each other. That is the second reason the schemes stay separate.
pub fn dest_for(
    dir: &Path,
    source: &Path,
    copy: u32,
    copy_name: Option<&str>,
    dedup: usize,
    format: ExportFormat,
) -> PathBuf {
    let stem = source
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("export");
    // ⚠ The default template, so this legacy entry point and the templated path cannot disagree
    // about what a copy is called. `export_stem` is gone.
    let base = Template::default().render(&NameFacts {
        file_stem: stem,
        copy,
        copy_label: copy_name,
        ..NameFacts::default()
    });
    dest_named(dir, &base, dedup, format)
}

/// Where an export lands, given the base name already chosen for it.
///
/// ⚠ This is the half that owns the **de-duplication** suffix and the extension, and nothing
/// else. Splitting it out is what lets a filename template decide the base without learning
/// about `-N` — which must stay separate from both `_N` (copy identity) and the template, for
/// the reasons [`dest_for`] gives.
pub fn dest_named(dir: &Path, base: &str, dedup: usize, format: ExportFormat) -> PathBuf {
    let ext = format.extension();
    let name = if dedup == 0 {
        format!("{base}.{ext}")
    } else {
        format!("{base}-{dedup}.{ext}")
    };
    dir.join(name)
}

/// Whether a destination directory can be exported into.
///
/// Empty means never chosen; anything else means chosen once and possibly gone since — an
/// unplugged drive, a deleted folder, or a preset carried from another machine.
///
/// Checked **before** a run rather than per photo. `run` already survives an unwritable
/// destination without panicking, but failing two hundred photos one at a time and reporting
/// afterwards is a poor trade when the answer is knowable up front.
pub fn destination_ready(dir: &Path) -> bool {
    !dir.as_os_str().is_empty() && dir.is_dir()
}

/// A short display name for the progress line.
pub(crate) fn file_label(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::batch_export::fixtures::*;

    #[test]
    fn the_destination_is_the_stem_plus_jpg() {
        let dir = PathBuf::from("/out");
        assert_eq!(
            dest_for(
                &dir,
                Path::new("/in/DSCF6509.RAF"),
                0,
                None,
                0,
                ExportFormat::Jpeg
            ),
            PathBuf::from("/out/DSCF6509.jpg")
        );
        assert_eq!(
            dest_for(
                &dir,
                Path::new("/in/holiday.jpeg"),
                0,
                None,
                0,
                ExportFormat::Jpeg
            ),
            PathBuf::from("/out/holiday.jpg")
        );
    }

    #[test]
    fn a_nonzero_index_adds_a_suffix() {
        // What "Keep both" uses: the first free name-1, name-2, ...
        let dir = PathBuf::from("/out");
        assert_eq!(
            dest_for(&dir, Path::new("/in/a.RAF"), 0, None, 1, ExportFormat::Jpeg),
            PathBuf::from("/out/a-1.jpg")
        );
        assert_eq!(
            dest_for(&dir, Path::new("/in/a.RAF"), 0, None, 7, ExportFormat::Jpeg),
            PathBuf::from("/out/a-7.jpg")
        );
    }

    #[test]
    fn destination_readiness_distinguishes_unset_missing_and_usable() {
        // ⚠ A pure function of the path rather than logic inside the egui closure, which is
        // what makes "Export is disabled when the folder is gone" testable at all rather
        // than only verifiable by clicking. Same extraction as
        // `Settings::apply_delete_confirm` and `view_region`'s maths.
        assert!(
            !destination_ready(Path::new("")),
            "an unset destination is not ready"
        );
        let d = TempDir::new("dest_ready");
        assert!(destination_ready(&d.0), "a real directory is ready");
        assert!(
            !destination_ready(&d.0.join("gone")),
            "a path that does not exist is not ready — the unplugged-drive case"
        );
        // A *file* is not a destination, and this is the case a plain `exists()` gets wrong.
        let file = d.0.join("a.txt");
        std::fs::write(&file, "x").unwrap();
        assert!(!destination_ready(&file), "a file is not a directory");
    }

    #[test]
    fn a_source_with_no_stem_still_produces_a_name() {
        // Defensive: `file_stem` is an Option, and a path ending in `..` has none.
        let dir = PathBuf::from("/out");
        let got = dest_for(&dir, Path::new("/in/.."), 0, None, 0, ExportFormat::Jpeg);
        assert!(got.starts_with("/out"));
        assert_eq!(got.extension().and_then(|e| e.to_str()), Some("jpg"));
    }
}
