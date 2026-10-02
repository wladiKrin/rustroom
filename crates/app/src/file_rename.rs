//! Renaming a photo on disk.
//!
//! ⚠ The rules here are pure and tested without touching the filesystem, because a rename that
//! goes wrong loses a file rather than mis-rendering one. The IO is `photo_move`'s; every
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
    let stem = strip_typed_extension(stem);
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

/// Strip a trailing extension the user left on a typed name.
///
/// ⚠ Splits at the **last dot** rather than using `Path::file_stem`, which follows the dotfile
/// rule and reports `".RAF"` as its own stem — so deleting the name and leaving the extension
/// produced the nonsense `_RAF.RAF` instead of being refused.
///
/// ⚠ The split is unconditional, so `".RAF"` yields `""` and the caller refuses it. That is
/// deliberately *not* what `default_copy_name` does with the same input — a name someone typed
/// and a name derived from a real file are different problems, and sharing the rule made a blank
/// entry silently acceptable.
fn strip_typed_extension(name: &str) -> &str {
    match name.rsplit_once('.') {
        Some((before, _)) => before,
        None => name,
    }
}

/// What a copy is called when it has not been given a name of its own.
///
/// ⚠ **Derived, never stored.** Leaving an unnamed copy's name as `None` is what makes it follow
/// the file: rename `a.RAF` to `b.RAF` and its copies become `b_1.RAF` on their own. Storing the
/// default at creation would freeze them on the old stem, silently, and the export filenames with
/// them.
pub fn default_copy_name(file_name: &str, copy: u32) -> String {
    match file_name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => format!("{stem}_{copy}.{ext}"),
        _ => format!("{file_name}_{copy}"),
    }
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
}

#[cfg(test)]
mod copy_name_tests {
    use super::*;

    #[test]
    fn an_unnamed_copy_is_the_stem_the_number_and_the_extension() {
        // ⚠ It is a whole filename, not a bare label, because it is what the copy exports as —
        // the field shows exactly what you will get.
        assert_eq!(default_copy_name("DSCF6223.RAF", 1), "DSCF6223_1.RAF");
        assert_eq!(default_copy_name("DSCF6223.RAF", 12), "DSCF6223_12.RAF");
    }

    #[test]
    fn a_file_with_no_extension_still_gets_a_copy_name() {
        assert_eq!(default_copy_name("photo", 2), "photo_2");
    }

    #[test]
    fn a_dotfile_keeps_its_leading_dot() {
        // ⚠ `.RAF` has no stem before the dot, so treating the dot as a separator would give
        // `_1.RAF` — a name with nothing left of the original in it.
        assert_eq!(default_copy_name(".RAF", 1), ".RAF_1");
    }
}
