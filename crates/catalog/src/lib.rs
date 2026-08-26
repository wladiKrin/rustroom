//! The thin catalog: scan a folder for RAW files. In-memory only (no DB yet).

use std::path::{Path, PathBuf};

/// Known RAW file extensions (lowercase, without the dot).
pub const RAW_EXTENSIONS: &[&str] = &["cr2", "cr3", "nef", "arw", "raf", "dng", "orf", "rw2"];

/// Non-RAW image extensions Rustroom can edit (lowercase, without the dot).
pub const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "tif", "tiff"];

/// One browsable photo in a folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEntry {
    pub path: PathBuf,
    pub file_name: String,
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
            entries.push(CatalogEntry { path, file_name });
        }
    }
    sort_by_name(&mut entries);
    Ok(entries)
}

/// Order a scan by file name, case-insensitively.
///
/// ⚠ Separated from `scan_folder` so the rule is testable **without a filesystem**, which here is
/// not merely convenient: macOS is case-insensitive, so a test that writes `a.RAF` and `A.RAF`
/// gets *one* file and cannot say anything about how two such names order.
pub fn sort_by_name(entries: &mut [CatalogEntry]) {
    entries.sort_by(|a, b| {
        a.file_name
            .to_lowercase()
            .cmp(&b.file_name.to_lowercase())
            // ⚠ Ties break on the exact name: case-insensitive comparison alone is not a total
            // order, so without this two names differing only in case would sort by whatever
            // order they arrived in.
            .then_with(|| a.file_name.cmp(&b.file_name))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Make a unique empty temp dir for the test.
    ///
    /// ⚠ Uniqueness is **structural** — process id plus a counter — never the clock. Two tests
    /// entering the same tick collide, and `cargo test` runs them in parallel; that cost this
    /// project a one-in-five flake in `image-decode` and again in `app`.
    fn temp_dir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "rustroom_catalog_{tag}_{}_{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
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
    }

    fn sorted(names: &[&str]) -> Vec<String> {
        let mut v: Vec<CatalogEntry> = names
            .iter()
            .map(|n| CatalogEntry {
                path: PathBuf::from(format!("/x/{n}")),
                file_name: n.to_string(),
            })
            .collect();
        sort_by_name(&mut v);
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
}
