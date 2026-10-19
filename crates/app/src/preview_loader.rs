//! The preview worker (M9.14): one thread that answers a request with a rendered preview from
//! the store, else the camera's embedded JPEG for an untouched photo, else nothing — in that
//! order, so an edited photo never shows the camera's rendering of its unedited self.
//!
//! ⚠ One thread, not `thumb_loader::worker_count()`: a request is a 4 MB JPEG decode (~0.2 s)
//! or a store read, and the consumers want one or two at a time. A second thread would only
//! double the peak memory of two 52 MB pictures in flight.

use crate::photo_id::PhotoId;
use crate::preview_key::PreviewKey;
use crate::preview_store::PreviewStore;
use crate::thumb_key::ThumbKey;
use core_image::Thumbnail;
use edit_model::Edits;
use raw_decode::Demosaic;
use std::collections::{HashSet, VecDeque};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};

/// Where a preview came from — the canvas names it in its corner.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PreviewSource {
    /// The camera's own JPEG: its film simulation, sharpening and lens correction.
    Camera,
    /// Rustroom's rendering of the photo's edits, from the store.
    Rendered,
}

#[derive(Clone, PartialEq, Debug)]
pub struct PreviewRequest {
    pub id: PhotoId,
    pub edits: Edits,
    /// The tier a rendered preview would have been built at (`Settings::preview_tier`).
    pub tier: Demosaic,
    /// Decided by the caller, who has the settings: `preview_key::is_untouched`.
    pub untouched: bool,
    /// AI denoise's network (`Settings::denoise_network`) — part of the file's name when AI denoise is on.
    pub network: crate::settings::DenoiseNetwork,
}

impl PreviewRequest {
    /// The in-memory key: the photo and its edits. The tier is a property of the file on disk,
    /// not of the picture shown, so two tiers of one edit state share a texture slot.
    pub fn key(&self) -> ThumbKey {
        ThumbKey::new(self.id.clone(), &self.edits)
    }

    pub fn store_key(&self) -> PreviewKey {
        PreviewKey::new(self.id.clone(), &self.edits, self.tier, self.network)
    }
}

pub struct PreviewResult {
    pub key: ThumbKey,
    /// `None` is a miss: the caller keeps its thumbnail.
    pub preview: Option<(Thumbnail, PreviewSource)>,
}

pub type PreviewLoadFn =
    Arc<dyn Fn(&PreviewRequest) -> Option<(Thumbnail, PreviewSource)> + Send + Sync>;

/// The resolution order. `extract` is the camera-JPEG reader, injected so the order is testable
/// without a RAW.
pub fn resolve(
    req: &PreviewRequest,
    store: &PreviewStore,
    extract: &dyn Fn(&std::path::Path) -> Option<Thumbnail>,
) -> Option<(Thumbnail, PreviewSource)> {
    if let Some(t) = store.open(&req.store_key()) {
        return Some((t, PreviewSource::Rendered));
    }
    if req.untouched {
        return extract(&req.id.path).map(|t| (t, PreviewSource::Camera));
    }
    None
}

/// The camera's JPEG for a RAW; the file itself for anything else.
pub fn extract_camera(path: &std::path::Path) -> Option<Thumbnail> {
    if catalog::is_raw(path) {
        raw_decode::extract_preview(path).ok()
    } else {
        image_decode::preview(path).ok()
    }
}

/// Shrink to the GPU's texture limit, once, so the bin never sees an unpaintable picture.
///
/// ⚠ 8192 on this Metal device: an X-T4's 4416 px fits with room; a 61 Mpx sensor's 9504 px
/// full-size rendered preview does not, and would be an upload error rather than a picture.
pub fn cap_to(t: Thumbnail, max_side: usize) -> Thumbnail {
    let long = t.width.max(t.height);
    if long <= max_side || long == 0 {
        return t;
    }
    let k = max_side as f64 / long as f64;
    let w = ((t.width as f64 * k).round() as u32).max(1);
    let h = ((t.height as f64 * k).round() as u32).max(1);
    let Some(img) = image::RgbaImage::from_raw(t.width as u32, t.height as u32, t.rgba) else {
        // ⚠ An RGBA buffer of the wrong length cannot be resized; an empty picture is a miss
        // downstream rather than a panic on the worker.
        return Thumbnail {
            width: 0,
            height: 0,
            rgba: Vec::new(),
        };
    };
    let out = image::imageops::resize(&img, w, h, image::imageops::FilterType::Triangle);
    Thumbnail {
        width: w as usize,
        height: h as usize,
        rgba: out.into_raw(),
    }
}

#[derive(Default)]
struct Queue {
    wanted: VecDeque<PreviewRequest>,
    in_flight: HashSet<ThumbKey>,
    stop: bool,
}

pub struct PreviewLoader {
    rx: Receiver<PreviewResult>,
    queue: Arc<(Mutex<Queue>, Condvar)>,
    _handle: Option<JoinHandle<()>>,
}

impl PreviewLoader {
    pub fn new(
        store: PreviewStore,
        display_space: color::OutputSpace,
        max_side: usize,
        on_progress: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self::with_loader(
            Arc::new(move |r: &PreviewRequest| resolve(r, &store, &extract_camera)),
            display_space,
            max_side,
            on_progress,
        )
    }

    /// `ThumbLoader::with_loader`'s loop with one thread; each answer is capped to the texture
    /// limit and converted to the display space before it is sent, so it is stored the way it
    /// will be shown — the thumbnails' rule, for the same reason.
    pub fn with_loader(
        load: PreviewLoadFn,
        display_space: color::OutputSpace,
        max_side: usize,
        on_progress: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        let (tx, rx) = channel::<PreviewResult>();
        let queue = Arc::new((Mutex::new(Queue::default()), Condvar::new()));
        let worker_queue = queue.clone();
        let handle = thread::spawn(move || {
            let (lock, cv) = &*worker_queue;
            loop {
                let req = loop {
                    let mut q = lock.lock().unwrap();
                    if q.stop {
                        return;
                    }
                    if let Some(r) = q.wanted.pop_front() {
                        q.in_flight.insert(r.key());
                        break r;
                    }
                    drop(cv.wait(q).unwrap());
                };
                let preview = load(&req).map(|(t, source)| {
                    let mut t = cap_to(t, max_side);
                    crate::thumb_loader::convert_thumb(&mut t, display_space);
                    (t, source)
                });
                let key = req.key();
                lock.lock().unwrap().in_flight.remove(&key);
                if tx.send(PreviewResult { key, preview }).is_err() {
                    return;
                }
                on_progress();
            }
        });
        Self {
            rx,
            queue,
            _handle: Some(handle),
        }
    }

    /// Replace the wanted list — what is on screen now — skipping requests already in flight.
    pub fn want(&self, wanted: &[PreviewRequest]) {
        let (lock, cv) = &*self.queue;
        let mut q = lock.lock().unwrap();
        q.wanted.clear();
        for r in wanted {
            if !q.in_flight.contains(&r.key()) {
                q.wanted.push_back(r.clone());
            }
        }
        drop(q);
        cv.notify_all();
    }

    pub fn drain(&self) -> Vec<PreviewResult> {
        self.rx.try_iter().collect()
    }
}

impl Drop for PreviewLoader {
    fn drop(&mut self) {
        let (lock, cv) = &*self.queue;
        lock.lock().unwrap().stop = true;
        cv.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            Self(test_dir::path(&format!("preview-loader-{tag}")))
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn req(dir: &std::path::Path, untouched: bool) -> PreviewRequest {
        let p = dir.join("a.RAF");
        std::fs::write(&p, b"raw").unwrap();
        PreviewRequest {
            network: Default::default(),
            id: PhotoId::original(p),
            edits: Edits::default(),
            tier: Demosaic::Better,
            untouched,
        }
    }

    fn pixels(w: usize, h: usize) -> Thumbnail {
        Thumbnail {
            width: w,
            height: h,
            rgba: vec![7; w * h * 4],
        }
    }

    #[test]
    fn a_rendered_preview_beats_the_camera_one_and_never_extracts() {
        let d = TempDir::new("rendered");
        let store = PreviewStore::new(d.0.join("previews"), u64::MAX);
        let r = req(&d.0, true);
        std::fs::create_dir_all(store.dir()).unwrap();
        image::RgbImage::from_pixel(8, 4, image::Rgb([1, 2, 3]))
            .save(store.path_for(&r.store_key()))
            .unwrap();
        let calls = AtomicUsize::new(0);
        let got = resolve(&r, &store, &|_| {
            calls.fetch_add(1, Ordering::Relaxed);
            Some(pixels(2, 2))
        });
        let (t, src) = got.expect("a preview");
        assert_eq!(src, PreviewSource::Rendered);
        assert_eq!((t.width, t.height), (8, 4));
        assert_eq!(
            calls.load(Ordering::Relaxed),
            0,
            "the camera JPEG was read for nothing"
        );
    }

    #[test]
    fn an_untouched_photo_without_a_rendered_preview_gets_the_camera_jpeg() {
        let d = TempDir::new("camera");
        let store = PreviewStore::new(d.0.join("previews"), u64::MAX);
        let (t, src) =
            resolve(&req(&d.0, true), &store, &|_| Some(pixels(3, 3))).expect("a preview");
        assert_eq!(src, PreviewSource::Camera);
        assert_eq!(t.width, 3);
    }

    #[test]
    fn an_edited_photo_without_a_rendered_preview_gets_nothing() {
        let d = TempDir::new("edited");
        let store = PreviewStore::new(d.0.join("previews"), u64::MAX);
        let calls = AtomicUsize::new(0);
        let got = resolve(&req(&d.0, false), &store, &|_| {
            calls.fetch_add(1, Ordering::Relaxed);
            Some(pixels(3, 3))
        });
        assert!(got.is_none());
        assert_eq!(calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn a_preview_wider_than_the_texture_limit_is_capped_and_a_smaller_one_untouched() {
        let big = cap_to(pixels(1000, 500), 400);
        assert_eq!((big.width, big.height), (400, 200));
        assert_eq!(big.rgba.len(), 400 * 200 * 4);
        let small = cap_to(pixels(300, 100), 400);
        assert_eq!((small.width, small.height), (300, 100));
    }

    #[test]
    fn the_loader_answers_what_it_was_asked_with_the_request_key() {
        let d = TempDir::new("loader");
        let r = req(&d.0, true);
        let loader = PreviewLoader::with_loader(
            Arc::new(|_r: &PreviewRequest| Some((pixels(5, 5), PreviewSource::Camera))),
            color::OutputSpace::Srgb,
            8192,
            || {},
        );
        loader.want(std::slice::from_ref(&r));
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut got = Vec::new();
        while got.is_empty() && std::time::Instant::now() < deadline {
            got = loader.drain();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].key, r.key());
        assert_eq!(
            got[0].preview.as_ref().map(|(t, s)| (t.width, *s)),
            Some((5, PreviewSource::Camera))
        );
    }
}
