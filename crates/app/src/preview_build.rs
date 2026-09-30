//! Scheduling rendered previews (M9.14): the queue of edited photos left behind, and the shape
//! of a build — which **is an export job** into the store, so one render path serves both and
//! progress and cancel come for free.

use crate::batch_export::{ExportJob, ExportOptions};
use crate::photo_id::PhotoId;
use crate::preview_key::PreviewKey;
use edit_model::Edits;
use export::ExportFormat;
use raw_decode::Demosaic;

/// JPEG quality for a rendered preview. Like the thumbnail store's, a tunable with its number
/// here: about 6–10 MB for a 26 Mpx frame.
pub const PREVIEW_QUALITY: u8 = 90;

/// Edited photos that were left without a preview, oldest first.
#[derive(Default, Debug)]
pub struct LeaveQueue {
    entries: Vec<(PhotoId, Edits)>,
}

impl LeaveQueue {
    /// A photo already queued is replaced **in place** — edited again before its build ran, it
    /// is built once, with the latest edits, and keeps its turn.
    pub fn push(&mut self, id: PhotoId, edits: Edits) {
        match self.entries.iter_mut().find(|(i, _)| *i == id) {
            Some(e) => e.1 = edits,
            None => self.entries.push((id, edits)),
        }
    }

    pub fn pop(&mut self) -> Option<(PhotoId, Edits)> {
        if self.entries.is_empty() {
            None
        } else {
            Some(self.entries.remove(0))
        }
    }

    pub fn remove(&mut self, id: &PhotoId) {
        self.entries.retain(|(i, _)| i != id);
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// The export options of a preview build: a full-size sRGB JPEG at `tier`, into the store.
///
/// ⚠ `{copy}` as the template renders the job's `name` when one is set — [`build_job`] puts
/// the key's hash there, so the file lands exactly where `PreviewStore::path_for` looks.
pub fn build_options(store_dir: std::path::PathBuf, tier: Demosaic) -> ExportOptions {
    ExportOptions {
        dest_dir: store_dir,
        quality: PREVIEW_QUALITY,
        max_edge: None,
        demosaic: tier,
        format: ExportFormat::Jpeg,
        space: color::OutputSpace::Srgb,
        filename_template: "{copy}".to_string(),
        ..ExportOptions::default()
    }
}

/// One build, named by its key. The metadata fields stay empty: nothing worth embedding.
pub fn build_job(id: PhotoId, edits: Edits, tier: Demosaic) -> ExportJob {
    let name = PreviewKey::new(id.clone(), &edits, tier).file_stem();
    ExportJob {
        id,
        name: Some(name),
        edits,
        capture: Default::default(),
        description: Default::default(),
        marks: Default::default(),
        keywords: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::batch_export::{ExportEvent, ExportRender, ExportWorker};

    struct TempDir(std::path::PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            Self(test_dir::path(&format!("preview-build-{tag}")))
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn id(dir: &std::path::Path, name: &str) -> PhotoId {
        let p = dir.join(name);
        std::fs::write(&p, b"raw").unwrap();
        PhotoId::original(p)
    }

    #[test]
    fn a_re_edited_photo_is_queued_once_with_its_latest_edits_and_order_is_kept() {
        let d = TempDir::new("queue");
        let (a, b) = (id(&d.0, "a.RAF"), id(&d.0, "b.RAF"));
        let mut q = LeaveQueue::default();
        assert!(q.is_empty());
        q.push(a.clone(), Edits::default());
        q.push(b.clone(), Edits::default());
        let e = Edits {
            exposure: 1.0,
            ..Default::default()
        };
        q.push(a.clone(), e.clone());
        assert_eq!(q.len(), 2);
        let first = q.pop().expect("a job");
        assert_eq!(first.0, a, "the replaced entry keeps its place at the head");
        assert_eq!(first.1.exposure, 1.0, "with the latest edits");
        q.remove(&b);
        assert!(q.is_empty());
        assert!(q.pop().is_none());
    }

    #[test]
    fn build_options_are_a_full_size_srgb_jpeg_at_the_tier_named_by_the_key() {
        let d = TempDir::new("opts");
        let o = build_options(d.0.clone(), Demosaic::Fast);
        assert_eq!(o.format, ExportFormat::Jpeg);
        assert_eq!(o.quality, PREVIEW_QUALITY);
        assert_eq!(o.max_edge, None);
        assert_eq!(o.space, color::OutputSpace::Srgb);
        assert_eq!(o.demosaic, Demosaic::Fast);
        assert_eq!(o.dest_dir, d.0);
        let j = build_job(id(&d.0, "a.RAF"), Edits::default(), Demosaic::Fast);
        let k = PreviewKey::new(j.id.clone(), &j.edits, Demosaic::Fast);
        assert_eq!(j.name.as_deref(), Some(k.file_stem().as_str()));
    }

    #[test]
    fn a_build_lands_in_the_store_under_its_key() {
        let d = TempDir::new("build");
        let store = crate::preview_store::PreviewStore::new(d.0.join("previews"), u64::MAX);
        let photo = id(&d.0, "a.RAF");
        let key = PreviewKey::new(photo.clone(), &Edits::default(), Demosaic::Better);
        // ⚠ The worker writes into an existing directory only; the app calls this before a build.
        store.ensure_dir();
        let worker = ExportWorker::spawn(
            vec![build_job(photo, Edits::default(), Demosaic::Better)],
            build_options(store.dir().to_path_buf(), Demosaic::Better),
            crate::batch_export::fixtures::ok_decode(),
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            ExportRender {
                resampler: Default::default(),
                gpu_tail: false,
                ai_subject: Default::default(),
            },
            1,
            None,
            None,
        );
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let mut finished = false;
        while !finished && std::time::Instant::now() < deadline {
            while let Ok(ev) = worker.events().try_recv() {
                match ev {
                    ExportEvent::Finished(r) => {
                        assert_eq!(r.exported, 1, "{r:?}");
                        finished = true;
                    }
                    ExportEvent::Collision { dest } => {
                        panic!("a fresh directory collided at {}", dest.display())
                    }
                    _ => {}
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(finished, "the build never finished");
        assert!(
            store.has(&key),
            "no file at {}",
            store.path_for(&key).display()
        );
        let t = store
            .open(&key)
            .expect("the file is not a JPEG the store can open");
        assert_eq!((t.width, t.height), (8, 4), "the fixture decode is 8×4");
    }
}
