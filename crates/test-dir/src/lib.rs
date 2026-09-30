//! A temp directory of its own for every test (M10.7c) — the only place in the workspace's tests
//! that calls `std::env::temp_dir()`; `every_test_temp_path_goes_through_this_crate` holds that.
//!
//! **Layout:** `<temp>/rustroom-tests/<pid>/<tag>-<n>`. The process id keeps two concurrent runs apart
//! (two gates at once); the counter keeps two tests of one run apart (they are threads of one
//! process). Before this crate, some tests used a fixed name, which collided across concurrent gates,
//! and some the process id alone, which collided between two tests of one run — and nothing was ever
//! removed: 3,128 directories, 72 MB, had piled up in a week.
//!
//! **Cleanup:** a [`TempDir`] deletes its directory when dropped. What is kept — [`path`], or a run
//! that crashed — goes at the next run: each run holds a lock on `<pid>/.lock` for its whole life, and
//! the first directory a run makes deletes every other run's root whose lock it can take, because that
//! run has ended. A concurrent gate still holds its lock, so its directories are never touched.

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

/// Where every run's root lives.
fn base() -> PathBuf {
    std::env::temp_dir().join("rustroom-tests")
}

/// This run's root, created — with its lock taken and ended runs swept — on first use.
fn run_root() -> &'static Path {
    static ROOT: OnceLock<(PathBuf, File)> = OnceLock::new();
    let (root, _held) = ROOT.get_or_init(|| {
        let base = base();
        let root = base.join(std::process::id().to_string());
        std::fs::create_dir_all(&root).expect("create the run's test root");
        let lock = File::create(root.join(".lock")).expect("create the run's lock file");
        // ⚠ Held for the life of the process: the `File` lives in this static, so the lock goes
        // only when the run ends — which is what tells the next run it may clean up.
        lock.try_lock().expect("take this run's own lock");
        sweep_ended_runs(&base, &root);
        (root, lock)
    });
    root
}

/// Delete every run root under `base` except `keep` whose run has ended: its lock can be taken.
///
/// ⚠ A root with no lock file is left alone unless it is over a day old — it may be a run between
/// creating its directory and its lock. Errors are ignored: cleanup must never fail a test.
pub fn sweep_ended_runs(base: &Path, keep: &Path) {
    let Ok(entries) = std::fs::read_dir(base) else {
        return;
    };
    for e in entries.flatten() {
        let root = e.path();
        if root == keep || !root.is_dir() {
            continue;
        }
        let ended = match File::options().write(true).open(root.join(".lock")) {
            // The lock is released as this `File` drops, just before the directory goes.
            Ok(f) => f.try_lock().is_ok(),
            Err(_) => e
                .metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age.as_secs() > 24 * 60 * 60),
        };
        if ended {
            let _ = std::fs::remove_dir_all(&root);
        }
    }
}

/// A test's own directory, deleted when dropped.
#[derive(Debug)]
pub struct TempDir {
    path: PathBuf,
    keep: bool,
}

impl TempDir {
    /// A fresh, empty directory named after `tag`, unique in this run and across runs.
    pub fn new(tag: &str) -> Self {
        static N: AtomicU64 = AtomicU64::new(0);
        let path = run_root().join(format!("{tag}-{}", N.fetch_add(1, Ordering::Relaxed)));
        // ⚠ Removed first: a pid is reused across runs, and a crashed run with the same pid could
        // have left this exact name behind before the sweep saw it.
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create a test's temp directory");
        Self { path, keep: false }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// A path inside the directory.
    pub fn join(&self, p: impl AsRef<Path>) -> PathBuf {
        self.path.join(p)
    }

    /// Hand the directory over without deleting it on drop — for a fixture that must return a plain
    /// path. The next run's sweep removes it once this run has ended.
    pub fn keep(mut self) -> PathBuf {
        self.keep = true;
        self.path.clone()
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }
}

/// A fresh directory for `tag` that outlives the call — [`TempDir::keep`] in one word.
pub fn path(tag: &str) -> PathBuf {
    TempDir::new(tag).keep()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_directories_with_one_tag_are_two_directories() {
        let a = TempDir::new("same");
        let b = TempDir::new("same");
        assert_ne!(a.path(), b.path());
        assert!(a.path().is_dir() && b.path().is_dir());
        // ⚠ Across threads too: tests are threads of one process.
        let paths: Vec<PathBuf> = (0..8)
            .map(|_| std::thread::spawn(|| path("threads")))
            .map(|h| h.join().unwrap())
            .collect();
        let mut unique = paths.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(unique.len(), paths.len(), "{paths:?}");
    }

    #[test]
    fn a_dropped_directory_is_gone_and_a_kept_one_is_not() {
        let d = TempDir::new("drop");
        let p = d.path().to_path_buf();
        std::fs::write(d.join("f"), b"x").unwrap();
        drop(d);
        assert!(!p.exists(), "dropping did not delete {p:?}");
        let kept = path("kept");
        assert!(kept.is_dir(), "a kept directory was deleted");
    }

    /// ⚠ Both halves, in a base of the test's own: a root whose run has ended (its lock is free) goes;
    /// one whose run is alive (its lock is held) and a young one with no lock at all stay.
    #[test]
    fn the_sweep_removes_ended_runs_and_nothing_else() {
        let base = TempDir::new("sweep-base");
        let root = |name: &str| {
            let r = base.join(name);
            std::fs::create_dir_all(&r).unwrap();
            r
        };
        let ended = root("111");
        File::create(ended.join(".lock")).unwrap();
        let alive = root("222");
        let held = File::create(alive.join(".lock")).unwrap();
        held.try_lock().unwrap();
        let young = root("333");
        let this = root("444");
        sweep_ended_runs(base.path(), &this);
        assert!(!ended.exists(), "an ended run's root survived");
        assert!(alive.exists(), "a live run's root was deleted");
        assert!(young.exists(), "a young root with no lock was deleted");
        assert!(this.exists(), "this run's own root was deleted");
        drop(held);
    }

    #[test]
    fn this_run_holds_its_own_lock() {
        let _ = TempDir::new("lock");
        let other = File::options()
            .write(true)
            .open(run_root().join(".lock"))
            .unwrap();
        assert!(other.try_lock().is_err(), "this run's lock is not held");
    }
}
