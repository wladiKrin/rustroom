//! The thin catalog: scan a folder for RAW files. In-memory only (no DB yet).

use std::path::{Path, PathBuf};

/// Known RAW file extensions (lowercase, without the dot).
pub const RAW_EXTENSIONS: &[&str] = &["cr2", "cr3", "nef", "arw", "raf", "dng", "orf", "rw2"];

/// One browsable photo in a folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEntry {
    pub path: PathBuf,
    pub file_name: String,
}

fn is_raw(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| RAW_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// Scan a single folder (non-recursive) for RAW files, sorted by file name.
pub fn scan_folder(dir: &Path) -> std::io::Result<Vec<CatalogEntry>> {
    let mut entries = Vec::new();
    for dirent in std::fs::read_dir(dir)? {
        let dirent = dirent?;
        let path = dirent.path();
        if path.is_file() && is_raw(&path) {
            let file_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_string();
            entries.push(CatalogEntry { path, file_name });
        }
    }
    entries.sort_by(|a, b| a.file_name.cmp(&b.file_name));
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Make a unique empty temp dir for the test.
    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rustroom_catalog_{tag}_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
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
}
