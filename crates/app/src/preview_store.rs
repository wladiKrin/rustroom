//! Rendered previews on disk: `<hash>.jpg` under `<config>/rustroom/previews`, written by the
//! export worker, named by `PreviewKey`, evicted oldest-first when over budget (M9.14).
//!
//! ⚠ Plain JPEGs, not `ThumbStore`'s headered entries: the key already carries the identity,
//! the edits and the tier, so a different photo or edit state is a different file name and
//! there is nothing to verify inside the file. The export worker writes the file; this store
//! only names, opens, counts and sweeps.
//!
//! ⚠ Holds a `PathBuf`, a budget and a written-bytes counter — `ThumbStore`'s shape, for its
//! reason: no in-process index to lock, **the filesystem is the index**.

use crate::preview_key::PreviewKey;
use core_image::Thumbnail;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct PreviewStore {
    dir: PathBuf,
    /// Bytes this directory may hold. `u64::MAX` means "no limit", which is what tests pass.
    budget: u64,
    /// Bytes written since the last sweep — `ThumbStore::written`'s amortisation.
    written: Arc<AtomicU64>,
}

impl PreviewStore {
    /// The directory is injected so tests run against a temp dir and never touch the real
    /// home directory. ⚠ Does not sweep; [`Self::sweep`] runs at startup and on a budget change.
    pub fn new(dir: PathBuf, budget_bytes: u64) -> Self {
        Self {
            dir,
            budget: budget_bytes,
            written: Arc::new(AtomicU64::new(0)),
        }
    }

    /// `<platform config dir>/rustroom/previews/`, beside `thumbs`.
    pub fn user_default(budget_bytes: u64) -> Self {
        Self::new(
            dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("rustroom")
                .join("previews"),
            budget_bytes,
        )
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Where `key`'s preview lives, whether or not it exists — the export worker's destination.
    pub fn path_for(&self, key: &PreviewKey) -> PathBuf {
        self.dir.join(key.file_name())
    }

    pub fn has(&self, key: &PreviewKey) -> bool {
        self.path_for(key).is_file()
    }

    /// Make the directory exist, for a build about to write into it.
    ///
    /// ⚠ The export worker does **not** create its destination — the Export dialog refuses a
    /// directory that is not there — so a build into a store that has never been written would
    /// fail with "No such file or directory" at the encode. Best-effort: a store that cannot be
    /// made simply never gets a preview.
    pub fn ensure_dir(&self) {
        let _ = std::fs::create_dir_all(&self.dir);
    }

    /// Decode the stored JPEG. Every failure — missing, unreadable, not a JPEG — is a miss.
    ///
    /// ⚠⚠ Touches the file's mtime so eviction is LRU rather than FIFO — `ThumbStore::get`'s
    /// one syscall, for its reason. Best-effort: a read-only directory must still serve hits.
    pub fn open(&self, key: &PreviewKey) -> Option<Thumbnail> {
        let path = self.path_for(key);
        let bytes = std::fs::read(&path).ok()?;
        let img = image::load_from_memory_with_format(&bytes, image::ImageFormat::Jpeg)
            .ok()?
            .to_rgba8();
        if let Ok(f) = std::fs::File::options().write(true).open(&path) {
            let _ =
                f.set_times(std::fs::FileTimes::new().set_modified(std::time::SystemTime::now()));
        }
        Some(Thumbnail {
            width: img.width() as usize,
            height: img.height() as usize,
            rgba: img.into_raw(),
        })
    }

    /// The export worker wrote `bytes` into the directory: sweep once a tenth of the budget has
    /// accumulated, so the overshoot is bounded without a directory scan per file.
    pub fn note_written(&self, bytes: u64) {
        if self.budget == u64::MAX {
            return;
        }
        let before = self.written.fetch_add(bytes, Ordering::Relaxed);
        let slack = (self.budget / 10).max(1);
        if before + bytes >= slack {
            self.written.store(0, Ordering::Relaxed);
            self.sweep();
        }
    }

    /// Delete oldest-first until the directory is within budget. Best-effort throughout.
    pub fn sweep(&self) {
        if self.budget == u64::MAX {
            return;
        }
        let Ok(d) = std::fs::read_dir(&self.dir) else {
            return;
        };
        let mut entries: Vec<(PathBuf, std::time::SystemTime, u64)> = Vec::new();
        let mut total: u64 = 0;
        for e in d.flatten() {
            let p = e.path();
            let Ok(m) = e.metadata() else { continue };
            if !m.is_file() {
                continue;
            }
            // ⚠ Litter from an interrupted write, not an entry: removed rather than counted.
            if p.extension().is_some_and(|x| x == "tmp") {
                let _ = std::fs::remove_file(&p);
                continue;
            }
            let when = m.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            total += m.len();
            entries.push((p, when, m.len()));
        }
        if total <= self.budget {
            return;
        }
        // ⚠ Oldest first, and mtime is the **touch** time rather than the write time — `open`
        // updates it, which is what makes this LRU instead of FIFO.
        entries.sort_by_key(|(_, when, _)| *when);
        for (p, _, len) in entries {
            if total <= self.budget {
                break;
            }
            if std::fs::remove_file(&p).is_ok() {
                total = total.saturating_sub(len);
            }
        }
    }

    /// Total bytes on disk. ⚠ Walks the directory: call it when the Settings dialog opens,
    /// never in a dialog body. An absent directory is `0`.
    pub fn size_bytes(&self) -> u64 {
        let Ok(d) = std::fs::read_dir(&self.dir) else {
            return 0;
        };
        d.flatten()
            .filter_map(|e| e.metadata().ok())
            .filter(|m| m.is_file())
            .map(|m| m.len())
            .sum()
    }

    /// Delete every entry, keeping the directory. Destroys nothing the app cannot rebuild.
    pub fn clear(&self) {
        let Ok(d) = std::fs::read_dir(&self.dir) else {
            return;
        };
        for e in d.flatten() {
            if e.metadata().map(|m| m.is_file()).unwrap_or(false) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::photo_id::PhotoId;
    use edit_model::Edits;
    use raw_decode::Demosaic;

    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static N: AtomicU64 = AtomicU64::new(0);
            let p = std::env::temp_dir().join(format!(
                "rustroom_preview_store_{tag}_{}_{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn key(dir: &Path, n: u32) -> PreviewKey {
        let p = dir.join(format!("{n}.RAF"));
        if !p.exists() {
            std::fs::write(&p, b"raw").unwrap();
        }
        PreviewKey::new(PhotoId::original(p), &Edits::default(), Demosaic::Better)
    }

    fn jpeg_at(path: &Path, w: u32, h: u32) -> u64 {
        let img = image::RgbImage::from_pixel(w, h, image::Rgb([10, 200, 30]));
        img.save(path).unwrap();
        std::fs::metadata(path).unwrap().len()
    }

    fn set_mtime(path: &Path, when: std::time::SystemTime) {
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(when))
            .unwrap();
    }

    #[test]
    fn opening_a_stored_preview_returns_its_pixels_and_touches_it() {
        let d = TempDir::new("open");
        let store = PreviewStore::new(d.0.join("previews"), u64::MAX);
        let k = key(&d.0, 1);
        assert!(!store.has(&k));
        assert!(store.open(&k).is_none(), "a missing file is a miss");
        std::fs::create_dir_all(store.dir()).unwrap();
        jpeg_at(&store.path_for(&k), 6, 4);
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
        set_mtime(&store.path_for(&k), old);
        assert!(store.has(&k));
        let t = store.open(&k).expect("a preview");
        assert_eq!((t.width, t.height), (6, 4));
        assert_eq!(t.rgba.len(), 6 * 4 * 4);
        assert!(
            t.rgba[1] > 150,
            "green survived the JPEG: {:?}",
            &t.rgba[..4]
        );
        let touched = std::fs::metadata(store.path_for(&k))
            .unwrap()
            .modified()
            .unwrap();
        assert!(
            touched > old + std::time::Duration::from_secs(1800),
            "open did not touch the file"
        );
    }

    #[test]
    fn a_sweep_deletes_oldest_first_until_under_budget() {
        let d = TempDir::new("sweep");
        let unbounded = PreviewStore::new(d.0.join("previews"), u64::MAX);
        std::fs::create_dir_all(unbounded.dir()).unwrap();
        let mut sizes = Vec::new();
        for n in 0..4u32 {
            let k = key(&d.0, n);
            sizes.push(jpeg_at(&unbounded.path_for(&k), 4, 4));
            let when = std::time::SystemTime::UNIX_EPOCH
                + std::time::Duration::from_secs(1_700_000_000 + n as u64 * 60);
            set_mtime(&unbounded.path_for(&k), when);
        }
        let total: u64 = sizes.iter().sum();
        let store = PreviewStore::new(d.0.join("previews"), total - 1);
        assert_eq!(store.size_bytes(), total, "fixture check");
        store.sweep();
        assert!(store.size_bytes() <= total - 1);
        assert!(!store.has(&key(&d.0, 0)), "the oldest went first");
        assert!(store.has(&key(&d.0, 3)), "the newest stayed");
        assert!(store.has(&key(&d.0, 2)), "one deletion was enough");
    }

    #[test]
    fn note_written_sweeps_once_a_tenth_of_the_budget_has_been_written() {
        let d = TempDir::new("written");
        let store = PreviewStore::new(d.0.join("previews"), 1000);
        std::fs::create_dir_all(store.dir()).unwrap();
        for n in 0..3u32 {
            jpeg_at(&store.path_for(&key(&d.0, n)), 32, 32);
        }
        let before = store.size_bytes();
        assert!(
            before > 1000,
            "fixture check: {before} bytes must be over budget"
        );
        store.note_written(50);
        assert_eq!(store.size_bytes(), before, "under a tenth: no sweep yet");
        store.note_written(60);
        assert!(store.size_bytes() <= 1000, "a tenth written: swept");
    }

    #[test]
    fn clear_removes_the_files_and_keeps_the_directory() {
        let d = TempDir::new("clear");
        let store = PreviewStore::new(d.0.join("previews"), u64::MAX);
        std::fs::create_dir_all(store.dir()).unwrap();
        jpeg_at(&store.path_for(&key(&d.0, 1)), 4, 4);
        assert!(store.size_bytes() > 0, "fixture check");
        store.clear();
        assert_eq!(store.size_bytes(), 0);
        assert!(store.dir().is_dir());
    }
}
