//! The per-user cache folder (2026-11-18): where Immich's downloads and lists and the map's tiles live — the
//! system's own (`dirs::cache_dir`: `%LOCALAPPDATA%`, `~/Library/Caches`, `~/.cache`), or a folder of the tests' own
//! in a test process.
//!
//! ⚠⚠ Found on Windows: the app's tests derived Immich paths from a server address and the worker created their
//! folders, so `<cache>/rustroom/immich/https___example_invalid` sat in the author's own cache — on every system,
//! unnoticed because it stayed empty. Every per-user cache path goes through [`base`], and a test process points it
//! away once ([`redirect_for_tests`]; this library's own tests do so by themselves).

use std::path::PathBuf;
use std::sync::OnceLock;

static REDIRECT: OnceLock<PathBuf> = OnceLock::new();

/// The folder per-user caches live under, or `None` where the system has none.
pub fn base() -> Option<PathBuf> {
    match REDIRECT.get() {
        Some(dir) => Some(dir.clone()),
        None => default_base(),
    }
}

/// For the tests outside this library — the app binary's `test_app` — send [`base`] to `dir()` for the rest of the
/// process. ⚠ The first call wins, so one folder serves the whole run; nothing in production calls it.
pub fn redirect_for_tests(dir: impl FnOnce() -> PathBuf) {
    REDIRECT.get_or_init(dir);
}

#[cfg(not(test))]
fn default_base() -> Option<PathBuf> {
    dirs::cache_dir()
}

/// ⚠ This library's own tests: a test folder from the start, never the user's cache.
#[cfg(test)]
fn default_base() -> Option<PathBuf> {
    Some(
        REDIRECT
            .get_or_init(|| test_dir::path("user-cache"))
            .clone(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The library's own tests derive their cache paths inside the run's test folder — `test_dir`'s, asserted on
    /// the path the code derives (`AGENTS.md`: never an absolute path of the host's).
    #[test]
    fn this_librarys_tests_cache_inside_the_test_folder() {
        let run = test_dir::path("probe");
        let run = run
            .parent()
            .expect("a test folder sits in its run's folder");
        let root = crate::immich_source::cache_root("https://example.invalid");
        assert!(
            root.starts_with(run),
            "{} is outside {}",
            root.display(),
            run.display()
        );
        assert_eq!(base(), base(), "one folder for the whole run");
    }
}
