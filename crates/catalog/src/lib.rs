//! The thin catalog: scan a folder for RAW files. In-memory only (no DB yet).

pub mod db;
pub mod snapshot;

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Known RAW file extensions (lowercase, without the dot).
pub const RAW_EXTENSIONS: &[&str] = &["cr2", "cr3", "nef", "arw", "raf", "dng", "orf", "rw2"];

/// Non-RAW image extensions Rustroom can edit (lowercase, without the dot). HEIC/HEIF since M10.19a — an
/// iPhone's `.heic`, the generic `.heif`, a camera's `.hif` — read through libheif (`image_decode::heif`).
pub const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "tif", "tiff", "heic", "heif", "hif"];

/// One browsable photo in a folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEntry {
    pub path: PathBuf,
    /// The name to **show** for this photo, which is not necessarily the name on disk.
    ///
    /// ⚠⚠ **Not the catalog's key.** The catalog keys a row on `path`'s own file name, because every
    /// read side derives its key from a `Path`. For a local file the two are the same; an Immich
    /// asset's cache path is `<uuid>.<ext>` while this carries the camera's name, and storing *this*
    /// as the key wrote rows that could never be found again — every rating on an Immich photo was
    /// silently dropped until 2026-09-23.
    pub file_name: String,
    /// When the file was last written, read once at scan time.
    ///
    /// ⚠ Captured during the scan rather than asked for while sorting: a sort touching the
    /// filesystem once per comparison is O(n log n) stat calls on a folder of hundreds.
    ///
    /// ⚠ `UNIX_EPOCH` when the metadata cannot be read, so an unreadable file sorts to the
    /// oldest end rather than failing the whole scan.
    pub modified: SystemTime,
    /// When this photo's edits were last written, or `None` if it has never been edited.
    ///
    /// ⚠⚠ **Filled in by the caller, never by `scan_folder`.** What counts as "edited" is the
    /// sidecar's modification time, and the sidecar naming convention belongs to `app` — this
    /// crate would have to learn it to read this itself. Carrying the field while staying
    /// ignorant of where it comes from is what keeps the layering intact.
    ///
    /// ⚠ `None` sorts as the oldest possible time, so a never-edited photo lands at the far end
    /// of "Last edited first" rather than jumbled in among the edited ones.
    pub edited: Option<SystemTime>,
}

/// How the browser orders a folder.
///
/// ⚠ Lives here, beside the comparator, for the reason `Demosaic` lives in `raw-decode`: the
/// ordering rule then exists **once**. A mirror enum in `app` would mean a second copy of it, and
/// a rule that exists twice is one that will disagree. The cost is `serde` as this crate's first
/// dependency, so the choice can be persisted in `Settings`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum SortOrder {
    #[default]
    NameAsc,
    NameDesc,
    ModifiedDesc,
    ModifiedAsc,
    EditedDesc,
    EditedAsc,
}

impl SortOrder {
    pub const ALL: [SortOrder; 6] = [
        SortOrder::NameAsc,
        SortOrder::NameDesc,
        SortOrder::ModifiedDesc,
        SortOrder::ModifiedAsc,
        SortOrder::EditedDesc,
        SortOrder::EditedAsc,
    ];

    /// Whether this order needs [`CatalogEntry::edited`] filled in.
    ///
    /// ⚠ Lets the caller skip a stat per file for the four orders that cannot use it.
    pub fn needs_edited(self) -> bool {
        matches!(self, SortOrder::EditedDesc | SortOrder::EditedAsc)
    }

    pub fn label(self) -> &'static str {
        match self {
            SortOrder::NameAsc => "Name (A–Z)",
            SortOrder::NameDesc => "Name (Z–A)",
            SortOrder::ModifiedDesc => "Newest first",
            SortOrder::ModifiedAsc => "Oldest first",
            SortOrder::EditedDesc => "Last edited first",
            SortOrder::EditedAsc => "Least recently edited",
        }
    }
}

fn has_ext(path: &Path, exts: &[&str]) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| exts.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// True if `path` is a camera RAW (needs the libraw decoder).
pub fn is_raw(path: &Path) -> bool {
    has_ext(path, RAW_EXTENSIONS)
}

/// True if Rustroom can open `path` (RAW or supported non-RAW image).
pub fn is_supported(path: &Path) -> bool {
    has_ext(path, RAW_EXTENSIONS) || has_ext(path, IMAGE_EXTENSIONS)
}

/// Scan a single folder (non-recursive) for supported images, sorted by file name.
///
/// ⚠ **Case-insensitively.** A plain byte comparison puts every capital before every lowercase
/// letter — `Zebra.RAF` ahead of `apple.RAF` — because `'A'` is 65 and `'a'` is 97. On a folder
/// mixing the two that reads as no ordering at all.
///
/// ⚠ Ties break on the exact name, so two files differing only in case have a **stable** order
/// rather than whichever the filesystem happened to hand back. Case-insensitive alone is not a
/// total order.
pub fn scan_folder(dir: &Path) -> std::io::Result<Vec<CatalogEntry>> {
    let mut entries = Vec::new();
    for dirent in std::fs::read_dir(dir)? {
        let dirent = dirent?;
        let path = dirent.path();
        if path.is_file() && is_supported(&path) {
            let file_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_string();
            let modified = dirent
                .metadata()
                .and_then(|m| m.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            entries.push(CatalogEntry {
                path,
                file_name,
                modified,
                // ⚠ Left for the caller — see the field's doc comment.
                edited: None,
            });
        }
    }
    sort(&mut entries, SortOrder::default());
    Ok(entries)
}

/// Scan `dir` and every folder beneath it (M9.9): `(folder, its entries)` for each folder holding a
/// supported image, `dir` itself first even when empty, the rest in path order.
///
/// ⚠ **Hidden folders are skipped** — `.git`, `.thumbnails`, an app's cache — which hold no
/// photographs anyone means. ⚠⚠ **So are symlinked folders**: a link to an ancestor would recurse
/// forever, and one elsewhere would put the same photos in the catalog under two paths.
///
/// ⚠ An unreadable subfolder is skipped, not an error: one permission-denied directory must not
/// hide the rest of the tree. Only `dir` itself failing to read is an error, as for `scan_folder`.
pub fn scan_tree(dir: &Path) -> std::io::Result<Vec<(std::path::PathBuf, Vec<CatalogEntry>)>> {
    let mut out = vec![(dir.to_path_buf(), scan_folder(dir)?)];
    let mut pending = subfolders(dir);
    while let Some(d) = pending.pop() {
        if let Ok(entries) = scan_folder(&d) {
            pending.extend(subfolders(&d));
            if !entries.is_empty() {
                out.push((d, entries));
            }
        }
    }
    out[1..].sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

/// The visible, non-symlinked subfolders of `dir`.
fn subfolders(dir: &Path) -> Vec<std::path::PathBuf> {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    rd.flatten()
        .filter(|e| {
            let hidden = e.file_name().to_string_lossy().starts_with('.');
            // `file_type` does not follow a symlink, which is exactly the test wanted.
            let real_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
            !hidden && real_dir
        })
        .map(|e| e.path())
        .collect()
}

/// Order a scan.
///
/// ⚠ Separated from `scan_folder` so the rules are testable **without a filesystem**, which here
/// is not merely convenient: macOS is case-insensitive, so a test that writes `a.RAF` and `A.RAF`
/// gets *one* file and cannot say anything about how two such names order.
///
/// ⚠⚠ **Every order ends in the same name tie-break, and that is what makes the grid stable.**
/// Two files written in the same second are common — a burst, a copy, an unzip — and without it
/// they would swap places between one folder open and the next, for no visible reason.
pub fn sort(entries: &mut [CatalogEntry], order: SortOrder) {
    fn edited(e: &CatalogEntry) -> SystemTime {
        e.edited.unwrap_or(SystemTime::UNIX_EPOCH)
    }
    entries.sort_by(|a, b| {
        let by_name = a
            .file_name
            .to_lowercase()
            .cmp(&b.file_name.to_lowercase())
            // ⚠ Case-insensitive comparison alone is not a total order, so two names differing
            // only in case would otherwise keep whatever order they arrived in.
            .then_with(|| a.file_name.cmp(&b.file_name));
        match order {
            SortOrder::NameAsc => by_name,
            SortOrder::NameDesc => by_name.reverse(),
            SortOrder::ModifiedDesc => b.modified.cmp(&a.modified).then(by_name),
            SortOrder::ModifiedAsc => a.modified.cmp(&b.modified).then(by_name),
            // ⚠ `None` is treated as the oldest possible time rather than skipped, so a photo
            // that has never been edited sorts to the far end instead of jumbling in among the
            // edited ones.
            SortOrder::EditedDesc => edited(b).cmp(&edited(a)).then(by_name),
            SortOrder::EditedAsc => edited(a).cmp(&edited(b)).then(by_name),
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh directory of its own — `test_dir` makes it unique per test and per run.
    fn temp_dir(tag: &str) -> PathBuf {
        test_dir::path(&format!("catalog-{tag}"))
    }

    fn touch(dir: &Path, name: &str) {
        std::fs::write(dir.join(name), b"x").unwrap();
    }

    #[test]
    fn finds_raws_sorted_ignores_others() {
        let dir = temp_dir("basic");
        touch(&dir, "b.CR2");
        touch(&dir, "a.raf");
        touch(&dir, "notes.txt");
        touch(&dir, "c.NEF");
        std::fs::create_dir_all(dir.join("sub")).unwrap();

        let got = scan_folder(&dir).unwrap();
        let names: Vec<_> = got.iter().map(|e| e.file_name.as_str()).collect();
        assert_eq!(names, vec!["a.raf", "b.CR2", "c.NEF"]);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_dir_is_err() {
        assert!(scan_folder(Path::new("/no/such/rustroom/dir")).is_err());
    }

    #[test]
    fn empty_folder_is_empty_vec() {
        let dir = temp_dir("empty");
        assert!(scan_folder(&dir).unwrap().is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn scan_includes_images_and_raws() {
        let dir = temp_dir("mixed");
        touch(&dir, "a.raf");
        touch(&dir, "b.JPG");
        touch(&dir, "c.png");
        touch(&dir, "d.tiff");
        touch(&dir, "notes.txt");
        let names: Vec<_> = scan_folder(&dir)
            .unwrap()
            .into_iter()
            .map(|e| e.file_name)
            .collect();
        assert_eq!(names, vec!["a.raf", "b.JPG", "c.png", "d.tiff"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn classifiers() {
        assert!(is_raw(Path::new("x.CR2")));
        assert!(!is_raw(Path::new("x.jpg")));
        assert!(is_supported(Path::new("x.jpg")) && is_supported(Path::new("x.NEF")));
        assert!(!is_supported(Path::new("x.txt")));
        // M10.19a: an iPhone's, the generic and a camera's spelling of HEIF, in any case — never RAW.
        for p in ["IMG_1.HEIC", "x.heif", "DSCF1.HIF"] {
            assert!(is_supported(Path::new(p)) && !is_raw(Path::new(p)), "{p}");
        }
    }

    fn entry(name: &str, secs: u64) -> CatalogEntry {
        CatalogEntry {
            path: PathBuf::from(format!("/x/{name}")),
            file_name: name.to_string(),
            modified: SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs),
            edited: None,
        }
    }

    fn edited_entry(name: &str, edited: Option<u64>) -> CatalogEntry {
        CatalogEntry {
            edited: edited.map(|s| SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(s)),
            ..entry(name, 0)
        }
    }

    fn sorted(names: &[&str]) -> Vec<String> {
        ordered(
            &names.iter().map(|n| (*n, 0)).collect::<Vec<_>>(),
            SortOrder::NameAsc,
        )
    }

    fn ordered(files: &[(&str, u64)], order: SortOrder) -> Vec<String> {
        let mut v: Vec<CatalogEntry> = files.iter().map(|(n, t)| entry(n, *t)).collect();
        sort(&mut v, order);
        v.into_iter().map(|e| e.file_name).collect()
    }

    #[test]
    fn sorting_ignores_case() {
        // ⚠ Byte order puts every capital ahead of every lowercase letter — `'A'` is 65 and
        // `'a'` is 97 — so `Zebra` would come before `apple`. On a folder mixing the two that
        // reads as no ordering at all.
        assert_eq!(
            sorted(&["beta.RAF", "Alpha.RAF", "alpha2.RAF", "Beta2.RAF"]),
            ["Alpha.RAF", "alpha2.RAF", "beta.RAF", "Beta2.RAF"]
        );
        assert_eq!(
            sorted(&["apple.RAF", "Zebra.RAF"]),
            ["apple.RAF", "Zebra.RAF"]
        );
    }

    #[test]
    fn names_differing_only_in_case_have_a_stable_order() {
        // ⚠ Case-insensitive comparison alone is not a total order, so without the tie-break two
        // such names would keep whatever order they arrived in — which for a directory read is
        // not defined. Both input orders must give the same output.
        assert_eq!(sorted(&["a.RAF", "A.RAF"]), ["A.RAF", "a.RAF"]);
        assert_eq!(sorted(&["A.RAF", "a.RAF"]), ["A.RAF", "a.RAF"]);
    }

    #[test]
    fn every_order_reaches_a_different_arrangement() {
        // ⚠ Options that quietly produce the same list would look like working controls and do
        // nothing. Each is checked against every other, not merely against itself.
        //
        // ⚠⚠ The timestamps are chosen so all six orders land on six *different* permutations.
        // Three files have exactly six permutations, in three reverse-pairs — and each
        // ascending/descending pair of orders is one such pair — so the fixture must hand a
        // different pair to name, to modified and to edited. A first attempt let name-ascending
        // and last-edited-first coincide, and the pairwise check below is what caught it: two
        // orders indistinguishable on the data means the test cannot tell them apart, however
        // right the code is.
        let files = [
            ("a.RAF", 30, Some(20)),
            ("b.RAF", 10, Some(30)),
            ("c.RAF", 20, Some(10)),
        ];
        let build = |o| {
            let mut v: Vec<CatalogEntry> = files
                .iter()
                .map(|(n, m, e)| CatalogEntry {
                    edited: e.map(|s| SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(s)),
                    ..entry(n, *m)
                })
                .collect();
            sort(&mut v, o);
            v.into_iter().map(|e| e.file_name).collect::<Vec<_>>()
        };
        let got: Vec<Vec<String>> = SortOrder::ALL.iter().map(|&o| build(o)).collect();
        assert_eq!(got[0], ["a.RAF", "b.RAF", "c.RAF"], "name ascending");
        assert_eq!(got[1], ["c.RAF", "b.RAF", "a.RAF"], "name descending");
        assert_eq!(got[2], ["a.RAF", "c.RAF", "b.RAF"], "newest first");
        assert_eq!(got[3], ["b.RAF", "c.RAF", "a.RAF"], "oldest first");
        assert_eq!(got[4], ["b.RAF", "a.RAF", "c.RAF"], "last edited first");
        assert_eq!(got[5], ["c.RAF", "a.RAF", "b.RAF"], "least recently edited");
        for (i, a) in got.iter().enumerate() {
            for b in got.iter().skip(i + 1) {
                assert_ne!(a, b, "two orders produced the same list");
            }
        }
    }

    #[test]
    fn files_written_at_the_same_time_keep_a_stable_order() {
        // ⚠⚠ Two files sharing an mtime is ordinary — a burst, a copy, an unzip. Without the name
        // tie-break their order would depend on however the directory read returned them, so the
        // grid would reshuffle between one folder open and the next for no visible reason.
        let one = [("b.RAF", 5), ("a.RAF", 5), ("c.RAF", 5)];
        let other = [("c.RAF", 5), ("b.RAF", 5), ("a.RAF", 5)];
        for order in [SortOrder::ModifiedAsc, SortOrder::ModifiedDesc] {
            assert_eq!(
                ordered(&one, order),
                ordered(&other, order),
                "{} depends on the input order",
                order.label()
            );
        }
    }

    #[test]
    fn every_order_has_its_own_label() {
        let mut seen: Vec<&str> = Vec::new();
        for o in SortOrder::ALL {
            assert!(!o.label().is_empty(), "{o:?} has no label");
            assert!(!seen.contains(&o.label()), "duplicate label {}", o.label());
            seen.push(o.label());
        }
    }

    #[test]
    fn a_scan_reads_the_modification_time() {
        // ⚠ Without a real timestamp every file would sort as `UNIX_EPOCH` and the two date
        // orders would silently degrade into the name tie-break — a control that appears to work.
        let dir = temp_dir("mtime");
        std::fs::write(dir.join("a.RAF"), b"x").unwrap();
        let got = scan_folder(&dir).unwrap();
        assert!(
            got[0].modified > SystemTime::UNIX_EPOCH,
            "the scan did not read a modification time"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_never_edited_photo_sorts_to_the_far_end() {
        // ⚠ `None` means "never edited", not "unknown, put it anywhere". Under *last edited
        // first* it belongs last; under *least recently edited* it belongs first.
        let files = [("b.RAF", Some(20)), ("a.RAF", None), ("c.RAF", Some(10))];
        let build = |o| {
            let mut v: Vec<CatalogEntry> = files.iter().map(|(n, e)| edited_entry(n, *e)).collect();
            sort(&mut v, o);
            v.into_iter().map(|e| e.file_name).collect::<Vec<_>>()
        };
        assert_eq!(build(SortOrder::EditedDesc), ["b.RAF", "c.RAF", "a.RAF"]);
        assert_eq!(build(SortOrder::EditedAsc), ["a.RAF", "c.RAF", "b.RAF"]);
    }

    #[test]
    fn only_the_edited_orders_need_the_edited_time() {
        // ⚠ This is what lets the caller skip a stat per file for the other four.
        for o in SortOrder::ALL {
            assert_eq!(
                o.needs_edited(),
                matches!(o, SortOrder::EditedDesc | SortOrder::EditedAsc),
                "{} disagrees about needing the edited time",
                o.label()
            );
        }
    }

    /// M9.9: the tree, with a hidden folder and a symlinked one that must both be skipped — the
    /// symlink pointing BACK at the root, where following it would never end.
    #[test]
    fn scan_tree_recurses_and_skips_hidden_and_symlinked_folders() {
        let root = test_dir::path("scantree");
        for d in ["a/b", ".hidden", "empty"] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        for f in ["top.jpg", "a/one.RAF", "a/b/two.jpg", ".hidden/no.jpg"] {
            std::fs::write(root.join(f), b"x").unwrap();
        }
        #[cfg(unix)]
        std::os::unix::fs::symlink(&root, root.join("a/loop")).unwrap();

        let tree = scan_tree(&root).unwrap();
        let _ = std::fs::remove_dir_all(&root);
        let folders: Vec<String> = tree
            .iter()
            .map(|(d, e)| format!("{}:{}", d.strip_prefix(&root).unwrap().display(), e.len()))
            .collect();
        assert_eq!(folders, vec![":1", "a:1", "a/b:1"], "{folders:?}");
    }
}
