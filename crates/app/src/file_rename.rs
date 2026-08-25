//! Renaming a photo on disk.
//!
//! ⚠ The rules here are pure and tested without touching the filesystem, because a rename that
//! goes wrong loses a file rather than mis-rendering one. The IO lives in `main.rs`; every
//! decision about *what* the new path should be lives here.

use crate::safe_name;
use std::path::{Path, PathBuf};

/// Why a rename was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The typed name was empty, or sanitised away to nothing.
    Empty,
    /// A file of that name is already there. ⚠ Never silently overwritten — that would destroy
    /// a photo, which no undo can recover.
    Taken(PathBuf),
}

/// Where `photo` would move to if its name became `typed`.
///
/// ⚠ **The original extension is always preserved**, whatever the user types. The field shows
/// the whole filename, so it is easy to delete or mangle `.RAF` by accident — and a RAW file
/// without its extension stops being openable by this app or any other. Renaming is not the
/// place to let someone change a file's type.
pub fn target(photo: &Path, typed: &str) -> Result<PathBuf, Refusal> {
    let stem = typed.trim();
    // Strip an extension the user left on, so "DSCF1.RAF" does not become "DSCF1.RAF.RAF".
    //
    // ⚠ Split at the **last dot** rather than using `Path::file_stem`, which follows the dotfile
    // rule and reports `".RAF"` as its own stem — so deleting the name and leaving the extension
    // produced the nonsense `_RAF.RAF` instead of being refused.
    let stem = match stem.rsplit_once('.') {
        Some((before, _)) => before,
        None => stem,
    };
    // ⚠ Through `safe_name`, the same guard the preset store uses: this string comes from a text
    // field and becomes a path, so `../../evil` must not escape the folder.
    let stem = safe_name::sanitize(stem);
    if stem.is_empty() {
        return Err(Refusal::Empty);
    }

    let mut name = stem;
    if let Some(ext) = photo.extension().and_then(|e| e.to_str()) {
        name.push('.');
        name.push_str(ext);
    }
    Ok(photo.with_file_name(name))
}

/// The sidecar that must move with `photo`.
///
/// ⚠ Renaming the photo without its sidecar orphans every edit ever made to it — the file would
/// open neutral and the old sidecar would linger as litter.
pub fn sidecar_of(photo: &Path) -> PathBuf {
    crate::sidecar::path(photo)
}

/// What the name field should contain for `photo`: the whole filename, extension included.
pub fn field_text(photo: &Path) -> String {
    photo
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn the_extension_is_preserved_whatever_is_typed() {
        // ⚠ The field shows the whole filename, so deleting `.RAF` is one keystroke away — and a
        // RAW without its extension is unopenable by this app and every other.
        assert_eq!(
            target(&p("/x/a.RAF"), "holiday").unwrap(),
            p("/x/holiday.RAF")
        );
        assert_eq!(
            target(&p("/x/a.RAF"), "holiday.RAF").unwrap(),
            p("/x/holiday.RAF"),
            "an extension left on is not doubled"
        );
        assert_eq!(
            target(&p("/x/a.RAF"), "holiday.jpg").unwrap(),
            p("/x/holiday.RAF"),
            "a *different* extension is ignored — renaming cannot change a file's type"
        );
    }

    #[test]
    fn a_name_that_would_escape_the_folder_is_sanitised() {
        // ⚠ The one security-relevant line: this text comes from a field and becomes a path.
        let got = target(&p("/x/a.RAF"), "../../evil").unwrap();
        assert_eq!(got.parent(), Some(Path::new("/x")), "must stay put");
        assert!(!got.to_string_lossy().contains(".."));
    }

    #[test]
    fn an_empty_or_blank_name_is_refused() {
        assert_eq!(target(&p("/x/a.RAF"), ""), Err(Refusal::Empty));
        assert_eq!(target(&p("/x/a.RAF"), "   "), Err(Refusal::Empty));
        assert_eq!(target(&p("/x/a.RAF"), ".RAF"), Err(Refusal::Empty));
    }

    #[test]
    fn a_file_with_no_extension_keeps_having_none() {
        assert_eq!(target(&p("/x/plain"), "other").unwrap(), p("/x/other"));
    }

    #[test]
    fn the_field_shows_the_whole_filename() {
        assert_eq!(field_text(&p("/x/DSCF6223.RAF")), "DSCF6223.RAF");
    }

    #[test]
    fn the_sidecar_moves_with_the_photo() {
        // ⚠ Not renaming it orphans every edit: the photo would reopen neutral and the old
        // sidecar would linger.
        assert_eq!(sidecar_of(&p("/x/a.RAF")), p("/x/a.RAF.json"));
    }
}
