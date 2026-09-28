//! Fetching a model file, and never keeping one whose SHA-256 does not match.

use crate::registry::ModelSpec;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum FetchError {
    #[error("download failed: {0}")]
    Http(String),
    #[error("could not write the model file: {0}")]
    Io(#[from] std::io::Error),
    #[error("the downloaded file does not match its pinned hash — it was deleted")]
    HashMismatch,
}

/// Whether `spec`'s file is already in `dir` at its pinned size. The hash was checked when it
/// arrived; re-hashing 224 MB on every mask would cost a second or more.
pub fn present(spec: &ModelSpec, dir: &Path) -> bool {
    std::fs::metadata(dir.join(spec.file)).is_ok_and(|m| m.len() == spec.bytes)
}

/// Download `spec` into `dir`. `progress` receives the bytes received so far.
pub fn fetch(
    spec: &ModelSpec,
    dir: &Path,
    progress: impl FnMut(u64),
) -> Result<PathBuf, FetchError> {
    let resp = ureq::get(spec.url)
        .call()
        .map_err(|e| FetchError::Http(e.to_string()))?;
    let reader = resp.into_body().into_reader();
    fetch_from(reader, spec, dir, progress)
}

/// Stream `reader` into `dir/<file>.partial`, hashing as it goes; rename into place only on a
/// matching hash. ⚠ Split from [`fetch`] so the whole rule is testable without a network.
pub fn fetch_from(
    mut reader: impl Read,
    spec: &ModelSpec,
    dir: &Path,
    mut progress: impl FnMut(u64),
) -> Result<PathBuf, FetchError> {
    std::fs::create_dir_all(dir)?;
    let dest = dir.join(spec.file);
    let partial = dir.join(format!("{}.partial", spec.file));
    let result = (|| {
        let mut out = std::fs::File::create(&partial)?;
        let mut hasher = Sha256::new();
        let mut buf = vec![0u8; 1 << 16];
        let mut total = 0u64;
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            out.write_all(&buf[..n])?;
            total += n as u64;
            progress(total);
        }
        out.flush()?;
        let got: String = hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if got != spec.sha256 {
            return Err(FetchError::HashMismatch);
        }
        std::fs::rename(&partial, &dest)?;
        Ok(dest.clone())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        static N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let p = std::env::temp_dir().join(format!(
            "rustroom_aimask_{tag}_{}_{}",
            std::process::id(),
            N.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn spec_for(bytes: &[u8], sha256: &'static str) -> ModelSpec {
        ModelSpec {
            file: "m.onnx",
            url: "https://github.com/x/m.onnx",
            bytes: bytes.len() as u64,
            sha256,
            licence: "MIT",
            licence_url: "https://github.com/x",
        }
    }

    /// SHA-256 of `b"model bytes"` — computed with `shasum`, never typed from memory.
    const GOOD: &str = "9cb7487000bc86ac36ce83c4acfabe8878552be99572a6770f65ab1d048a5c48";

    #[test]
    fn a_matching_download_lands_and_leaves_no_partial() {
        let d = temp_dir("ok");
        let body = b"model bytes";
        let actual: String = Sha256::digest(body)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(actual, GOOD, "fixture check: the pinned hash is the body's");
        let spec = spec_for(body, GOOD);
        let mut seen = 0;
        let p = fetch_from(&body[..], &spec, &d, |n| seen = n).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), body);
        assert_eq!(seen, body.len() as u64);
        assert!(!d.join("m.onnx.partial").exists());
        assert!(present(&spec, &d));
    }

    #[test]
    fn a_mismatched_download_is_deleted() {
        let d = temp_dir("bad");
        let spec = spec_for(b"model bytes", GOOD);
        let err = fetch_from(&b"tampered!!!"[..], &spec, &d, |_| {}).unwrap_err();
        assert!(matches!(err, FetchError::HashMismatch));
        assert!(!d.join("m.onnx").exists() && !d.join("m.onnx.partial").exists());
        assert!(!present(&spec, &d));
    }

    #[test]
    fn a_file_of_the_wrong_size_is_not_present() {
        let d = temp_dir("size");
        let spec = spec_for(b"model bytes", GOOD);
        std::fs::write(d.join("m.onnx"), b"short").unwrap();
        assert!(!present(&spec, &d));
    }
}
