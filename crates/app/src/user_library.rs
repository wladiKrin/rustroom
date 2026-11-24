//! The per-user library folder (2026-11-24): where the thumbnails and rendered previews live beside the catalog —
//! `catalog::db::Catalog::library_dir` (Local AppData on Windows, the config folder elsewhere), or a folder of the
//! tests' own in a test process. [`crate::user_cache`]'s shape, for its reason.
//!
//! ⚠⚠ Found on Windows: every full `cargo test --workspace` left six new `.rthm` files in the author's own
//! `%LOCALAPPDATA%\rustroom\thumbs` — a test that opens a folder of real photos builds a `ThumbLoader`, whose store
//! was `ThumbStore::user_default`, so their thumbnails landed in the real cache; on every system, since the
//! macOS and Linux library folder is derived the same way. The 2026-11-18 fix had moved only [`crate::user_cache`].
//! Every store under the library folder goes through [`dir`], and a test process points it away once
//! ([`redirect_for_tests`]; this library's own tests do so by themselves).

use std::path::PathBuf;
use std::sync::OnceLock;

static REDIRECT: OnceLock<PathBuf> = OnceLock::new();

/// The folder the thumbnail and preview stores live under.
pub fn dir() -> PathBuf {
    match REDIRECT.get() {
        Some(dir) => dir.clone(),
        None => default_dir(),
    }
}

/// For the tests outside this library — the app binary's `test_app` — send [`dir`] to `dir()` for the rest of the
/// process. ⚠ The first call wins, so one folder serves the whole run; nothing in production calls it.
pub fn redirect_for_tests(dir: impl FnOnce() -> PathBuf) {
    REDIRECT.get_or_init(dir);
}

#[cfg(not(test))]
fn default_dir() -> PathBuf {
    catalog::db::Catalog::library_dir()
}

/// ⚠ This library's own tests: a test folder from the start, never the user's library.
#[cfg(test)]
fn default_dir() -> PathBuf {
    REDIRECT
        .get_or_init(|| test_dir::path("user-library"))
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The library's own tests derive the thumbnail and preview folders inside the run's test folder — `test_dir`'s,
    /// asserted on the paths the stores derive (`AGENTS.md`: never an absolute path of the host's).
    #[test]
    fn this_librarys_tests_keep_thumbnails_and_previews_inside_the_test_folder() {
        let run = test_dir::path("probe");
        let run = run
            .parent()
            .expect("a test folder sits in its run's folder");
        let stores = [
            crate::thumb_store::ThumbStore::user_default(u64::MAX)
                .dir()
                .to_path_buf(),
            crate::thumb_store::ThumbStore::immich_default(u64::MAX)
                .dir()
                .to_path_buf(),
            crate::preview_store::PreviewStore::user_default(u64::MAX)
                .dir()
                .to_path_buf(),
        ];
        for store in &stores {
            assert!(
                store.starts_with(run),
                "{} is outside {}",
                store.display(),
                run.display()
            );
        }
        assert_eq!(dir(), dir(), "one folder for the whole run");
    }
}
