//! One file's copy, never leaving a half-file under the real name.

use std::path::{Path, PathBuf};

/// Copy `src` to `dest`: write a sibling `.partial-<pid>`, check its size against the source's,
/// then rename it into place. On any error the partial is removed, so an interrupted import (a
/// pulled card, a full disk) leaves nothing that looks like a photo.
///
/// ⚠ `src` is only read.
pub fn copy_verified(src: &Path, dest: &Path) -> std::io::Result<()> {
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let partial = partial_path(dest);
    let result = (|| {
        let expected = std::fs::metadata(src)?.len();
        std::fs::copy(src, &partial)?;
        verify_size(expected, &partial)?;
        std::fs::rename(&partial, dest)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    result
}

fn partial_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".partial-{}", std::process::id()));
    dest.with_file_name(name)
}

/// An error unless `file` holds exactly `expected` bytes.
fn verify_size(expected: u64, file: &Path) -> std::io::Result<()> {
    let got = std::fs::metadata(file)?.len();
    if got == expected {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "copied {got} of {expected} bytes"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ingest::temp_dir;

    #[test]
    fn a_copy_is_byte_identical_and_the_source_is_untouched() {
        let d = temp_dir("copy");
        let src = d.join("card/A.RAF");
        std::fs::create_dir_all(src.parent().unwrap()).unwrap();
        std::fs::write(&src, b"raw bytes").unwrap();
        let before = std::fs::metadata(&src).unwrap().modified().unwrap();
        let dest = d.join("home/2026/2026-09-27/A.RAF");
        copy_verified(&src, &dest).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"raw bytes");
        assert_eq!(std::fs::read(&src).unwrap(), b"raw bytes");
        assert_eq!(std::fs::metadata(&src).unwrap().modified().unwrap(), before);
        assert!(!partial_path(&dest).exists());
    }

    #[test]
    fn a_failed_copy_leaves_no_partial_behind() {
        let d = temp_dir("fail");
        let src = d.join("A.RAF");
        std::fs::write(&src, b"x").unwrap();
        // The destination's parent is a FILE, so the directory cannot be made.
        std::fs::write(d.join("blocker"), b"").unwrap();
        let dest = d.join("blocker/A.RAF");
        assert!(copy_verified(&src, &dest).is_err());
        assert!(!dest.exists());
        // A missing source fails too, and leaves nothing.
        let dest2 = d.join("ok/B.RAF");
        assert!(copy_verified(&d.join("gone.RAF"), &dest2).is_err());
        assert!(!partial_path(&dest2).exists() && !dest2.exists());
    }

    #[test]
    fn a_size_mismatch_is_an_error() {
        let d = temp_dir("size");
        let f = d.join("f");
        std::fs::write(&f, b"abc").unwrap();
        assert!(verify_size(3, &f).is_ok());
        assert!(verify_size(4, &f).is_err());
    }
}
