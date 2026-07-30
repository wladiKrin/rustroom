//! Background worker that extracts embedded thumbnails off the UI thread and streams
//! them back keyed by their index in the folder listing.

use raw_decode::Thumbnail;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::thread::{self, JoinHandle};

/// One finished thumbnail (or `None` if that file had no usable preview).
pub struct ThumbResult {
    pub index: usize,
    pub thumb: Option<Thumbnail>,
}

/// Owns a worker thread that loads a fixed list of paths, in order.
pub struct ThumbLoader {
    rx: Receiver<ThumbResult>,
    _handle: JoinHandle<()>,
}

impl ThumbLoader {
    /// `on_progress` is called after each thumbnail (e.g. `ctx.request_repaint`).
    pub fn new(paths: Vec<PathBuf>, on_progress: impl Fn() + Send + 'static) -> Self {
        let (tx, rx) = channel::<ThumbResult>();
        let handle = thread::spawn(move || {
            for (index, path) in paths.into_iter().enumerate() {
                let thumb = raw_decode::extract_thumbnail(&path).ok();
                if tx.send(ThumbResult { index, thumb }).is_err() {
                    return;
                }
                on_progress();
            }
        });
        Self {
            rx,
            _handle: handle,
        }
    }

    /// Take all thumbnails finished since the last call.
    pub fn drain(&self) -> Vec<ThumbResult> {
        self.rx.try_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streams_results_keyed_by_index_even_on_failure() {
        // Non-existent files fail extraction -> thumb is None, but a result still streams.
        let paths = vec![
            PathBuf::from("/nonexistent/rustroom_a.raf"),
            PathBuf::from("/nonexistent/rustroom_b.raf"),
        ];
        let loader = ThumbLoader::new(paths, || {});
        let mut results = Vec::new();
        for _ in 0..200 {
            results.extend(loader.drain());
            if results.len() >= 2 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        results.sort_by_key(|r| r.index);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].index, 0);
        assert_eq!(results[1].index, 1);
        assert!(results[0].thumb.is_none() && results[1].thumb.is_none());
    }
}
