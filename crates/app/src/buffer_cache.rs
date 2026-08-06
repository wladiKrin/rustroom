//! A tiny LRU cache of decoded photos, so stepping back to a recently viewed photo is
//! instant instead of costing another ~1 s decode.

use crate::decode_worker::Decoded;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Least-recently-used cache of decoded photos, keyed by source path.
///
/// Values are `Arc<Decoded>`, so a cache hit shares the ~78 MB editing buffer with the live
/// session rather than copying it. `entries` is ordered least-recently-used first; at a
/// capacity of 3 a linear scan is cheaper than any index.
pub struct BufferCache {
    cap: usize,
    entries: Vec<(PathBuf, Arc<Decoded>)>,
}

impl BufferCache {
    pub fn new(cap: usize) -> Self {
        Self {
            cap: cap.max(1),
            entries: Vec::new(),
        }
    }

    /// Fetch a decoded photo, marking it most-recently-used.
    pub fn get(&mut self, path: &Path) -> Option<Arc<Decoded>> {
        let i = self.entries.iter().position(|(p, _)| p == path)?;
        let entry = self.entries.remove(i);
        let decoded = entry.1.clone();
        self.entries.push(entry);
        Some(decoded)
    }

    /// Insert (or replace) a photo, evicting the least recently used entry when full.
    pub fn insert(&mut self, path: PathBuf, decoded: Arc<Decoded>) {
        if let Some(i) = self.entries.iter().position(|(p, _)| *p == path) {
            self.entries.remove(i);
        }
        self.entries.push((path, decoded));
        while self.entries.len() > self.cap {
            self.entries.remove(0);
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_engine::RenderedFrame;
    use core_image::ImageBuf;

    fn decoded(label: &str) -> Arc<Decoded> {
        Arc::new(Decoded {
            buffer: Arc::new(ImageBuf::new(2, 2)),
            label: label.to_string(),
            baseline: RenderedFrame {
                width: 1,
                height: 1,
                rgba: vec![0, 0, 0, 255],
                region: None,
            },
            source_scale: 1.0,
        })
    }

    fn label_of(d: &Arc<Decoded>) -> String {
        d.label.clone()
    }

    #[test]
    fn miss_then_hit() {
        let mut c = BufferCache::new(3);
        assert!(c.get(Path::new("/a")).is_none());
        c.insert(PathBuf::from("/a"), decoded("a"));
        assert_eq!(
            c.get(Path::new("/a")).map(|d| label_of(&d)).as_deref(),
            Some("a")
        );
    }

    #[test]
    fn evicts_least_recently_used() {
        let mut c = BufferCache::new(2);
        c.insert(PathBuf::from("/a"), decoded("a"));
        c.insert(PathBuf::from("/b"), decoded("b"));
        // Touch /a so /b becomes the least recently used.
        assert!(c.get(Path::new("/a")).is_some());
        c.insert(PathBuf::from("/c"), decoded("c"));
        assert_eq!(c.len(), 2);
        assert!(
            c.get(Path::new("/b")).is_none(),
            "/b should have been evicted"
        );
        assert!(c.get(Path::new("/a")).is_some());
        assert!(c.get(Path::new("/c")).is_some());
    }

    #[test]
    fn reinserting_replaces_without_growing() {
        let mut c = BufferCache::new(2);
        c.insert(PathBuf::from("/a"), decoded("a"));
        c.insert(PathBuf::from("/a"), decoded("a2"));
        assert_eq!(c.len(), 1);
        assert_eq!(
            c.get(Path::new("/a")).map(|d| label_of(&d)).as_deref(),
            Some("a2")
        );
    }

    #[test]
    fn capacity_is_never_exceeded() {
        let mut c = BufferCache::new(3);
        for i in 0..10 {
            c.insert(PathBuf::from(format!("/{i}")), decoded("x"));
        }
        assert_eq!(c.len(), 3);
    }
}
