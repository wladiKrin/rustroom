//! A path-keyed, byte-budgeted cache of loaded thumbnails.
//!
//! ⚠⚠ **Keyed by `PathBuf`, not by an `entries` index**, and three things fall out of that:
//! copies of one file share one texture (a Library thumbnail is the camera's embedded JPEG, so
//! every copy's is pixel-identical); a re-sort or `rebuild_entries` invalidates nothing; and
//! indices never leave the UI, which removes this project's third-most-repeated bug family —
//! sparse-versus-dense index confusion — at the boundary rather than guarding against it.
//!
//! ⚠ Generic over the value so it can live in this lib, which must not depend on egui. The bin
//! instantiates `ThumbCache<egui::TextureHandle>`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// How much memory thumbnails may occupy.
///
/// ~770 thumbnails at the measured ~0.67 MB each, so any folder up to roughly that size is
/// browsed end-to-end and back with no eviction at all, while a 10,000-photo source is bounded
/// here instead of at ~7 GB.
///
/// ⚠⚠ **It was 256 MB and that was too small, found by using it.** A 415-file folder measures
/// **276 MB** — 20 MB over — so scrolling to the end and back evicted the top ~30 thumbnails and
/// they visibly reloaded. The boundary was landing on ordinary folders. 512 MB also matches this
/// project's own sizing precedent: `MID_BUDGET_BYTES` is 512 MB, and a *single* full-resolution
/// prefix checkpoint is ~313 MB — larger than this whole cache was.
pub const THUMB_BUDGET_BYTES: usize = 512 * 1024 * 1024;

/// A typical thumbnail, measured: 415 RAF previews came to 276 MB, and 830 mixed RAF+JPEG ones
/// to 552 MB — 0.665 MB apiece both times.
const TYPICAL_THUMB_BYTES: usize = 700 * 1024;

// ⚠ **Module-level** compile-time guards, not associated ones. An associated `const` inside an
// `impl` is only evaluated where it is used, so `PrefixCache`'s first attempt at exactly this
// compiled happily with a budget of zero — a guard that did not guard.
const _: () = assert!(THUMB_BUDGET_BYTES > 0);
// ⚠ The *reasoning* for the number, not merely the number. Lowering the budget below a folder of
// ~700 photos brings back the eviction-on-round-trip this was raised to fix, and a comment alone
// would not stop it.
const _: () = assert!(THUMB_BUDGET_BYTES / TYPICAL_THUMB_BYTES >= 700);

struct Entry<T> {
    value: T,
    bytes: usize,
    /// Monotonic recency stamp. Set when the entry is inserted and refreshed by `retain_window`.
    seq: u64,
}

pub struct ThumbCache<T> {
    map: HashMap<PathBuf, Entry<T>>,
    budget: usize,
    bytes: usize,
    clock: u64,
}

impl<T> ThumbCache<T> {
    pub fn new(budget: usize) -> Self {
        Self {
            map: HashMap::new(),
            budget,
            bytes: 0,
            clock: 0,
        }
    }

    pub fn with_default_budget() -> Self {
        Self::new(THUMB_BUDGET_BYTES)
    }

    /// ⚠ Takes `&self`, deliberately. Recency is set by `retain_window` instead, for two reasons:
    /// what makes a thumbnail recent is *being on screen*, which is exactly what `retain_window`
    /// is told; and a `&mut self` accessor called from inside the draw loop would fight the
    /// shared borrows of `entries` and `visible` that the loop already holds.
    pub fn get(&self, path: &Path) -> Option<&T> {
        self.map.get(path).map(|e| &e.value)
    }

    pub fn contains(&self, path: &Path) -> bool {
        self.map.contains_key(path)
    }

    /// Store `value` for `path`, replacing whatever was there.
    pub fn insert(&mut self, path: PathBuf, value: T, bytes: usize) {
        self.clock += 1;
        let seq = self.clock;
        if let Some(old) = self.map.insert(path, Entry { value, bytes, seq }) {
            // ⚠ Subtract the replaced entry's bytes, or a re-inserted path double-counts and the
            // budget drifts upward until eviction starts throwing away live thumbnails.
            self.bytes = self.bytes.saturating_sub(old.bytes);
        }
        self.bytes += bytes;
    }

    /// Promote everything on screen to most-recent, then evict down to the budget.
    ///
    /// ⚠⚠ **The window is pinned, not merely preferred.** Nothing in `window` is ever evicted,
    /// however far the budget falls, so the cache may legitimately exceed its budget while a very
    /// large window is on screen.
    pub fn retain_window(&mut self, window: &[PathBuf]) {
        let pinned: HashSet<&Path> = window.iter().map(PathBuf::as_path).collect();
        for path in window {
            self.clock += 1;
            let seq = self.clock;
            if let Some(e) = self.map.get_mut(path) {
                e.seq = seq;
            }
        }
        while self.bytes > self.budget {
            let victim = self
                .map
                .iter()
                .filter(|(path, _)| !pinned.contains(path.as_path()))
                .min_by_key(|(_, e)| e.seq)
                .map(|(path, _)| path.clone());
            // `None` means everything left is pinned — stop rather than blank the screen.
            let Some(victim) = victim else { break };
            if let Some(e) = self.map.remove(&victim) {
                self.bytes = self.bytes.saturating_sub(e.bytes);
            }
        }
    }

    pub fn clear(&mut self) {
        self.map.clear();
        self.bytes = 0;
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(name: &str) -> PathBuf {
        PathBuf::from(format!("/x/{name}.RAF"))
    }

    /// Five 10-byte entries, `a`..`e`, inserted in order.
    fn cache_of_five(budget: usize) -> ThumbCache<String> {
        let mut c = ThumbCache::new(budget);
        for name in ["a", "b", "c", "d", "e"] {
            c.insert(p(name), name.to_string(), 10);
        }
        c
    }

    #[test]
    fn a_value_survives_a_round_trip() {
        let c = cache_of_five(1000);
        assert_eq!(c.get(&p("c")).map(String::as_str), Some("c"));
        assert!(c.contains(&p("a")));
        assert!(!c.contains(&p("zz")));
        assert_eq!(c.len(), 5);
        assert_eq!(c.bytes(), 50);
    }

    #[test]
    fn two_copies_of_one_file_are_one_entry() {
        // ⚠ The whole reason for path-keying. `entries` repeats a path once per copy, so an
        // index-keyed cache holds three pixel-identical textures for a file with three copies.
        let mut c = ThumbCache::new(1000);
        c.insert(p("a"), "first".into(), 10);
        c.insert(p("a"), "second".into(), 10);
        assert_eq!(c.len(), 1);
        assert_eq!(
            c.bytes(),
            10,
            "a replacement must not double-count its bytes"
        );
        assert_eq!(c.get(&p("a")).map(String::as_str), Some("second"));
    }

    #[test]
    fn nothing_is_evicted_while_under_budget() {
        let mut c = cache_of_five(1000);
        c.retain_window(&[p("a")]);
        assert_eq!(c.len(), 5);
    }

    #[test]
    fn a_path_outside_the_window_is_evicted_when_over_budget() {
        // Budget 30 holds three of the five.
        let mut c = cache_of_five(30);
        c.retain_window(&[p("e")]);
        assert!(c.bytes() <= 30, "over budget: {}", c.bytes());
        assert!(c.contains(&p("e")), "the window must survive");
    }

    #[test]
    fn eviction_takes_the_least_recently_windowed_first() {
        // ⚠ Recency is set by `retain_window`, not by `get`: what makes a thumbnail recent is
        // having been on screen. Here `a` is re-windowed, so `b` becomes the oldest and goes.
        let mut c = cache_of_five(1000);
        c.retain_window(&[p("a")]);
        c.budget = 40;
        c.retain_window(&[p("e")]);
        assert!(c.contains(&p("a")), "a was re-windowed and must outlive b");
        assert!(
            !c.contains(&p("b")),
            "b was the oldest and should have gone"
        );
        assert!(c.contains(&p("e")));
    }

    #[test]
    fn the_window_is_pinned_even_when_it_exceeds_the_budget() {
        // ⚠⚠ The load-bearing property. Eviction may never touch what is on screen, however far
        // the budget falls — otherwise a large window silently blanks the very cells being drawn,
        // and the failure looks like a slow loader rather than a bug.
        let mut c = cache_of_five(10);
        let window: Vec<PathBuf> = ["a", "b", "c", "d", "e"].iter().map(|n| p(n)).collect();
        c.retain_window(&window);
        assert_eq!(c.len(), 5, "an on-screen thumbnail was evicted");
        assert!(c.bytes() > c.budget, "the test is not exercising the pin");
    }

    #[test]
    fn a_window_naming_absent_paths_is_harmless() {
        // The window is whatever is on screen; most of it may not be loaded yet.
        let mut c = cache_of_five(1000);
        c.retain_window(&[p("zz"), p("a")]);
        assert_eq!(c.len(), 5);
    }

    #[test]
    fn clear_empties_it() {
        let mut c = cache_of_five(1000);
        c.clear();
        assert!(c.is_empty());
        assert_eq!(c.bytes(), 0);
    }
}
