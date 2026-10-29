//! The People view's background search (M10.18c): each local photo's faces found and told apart, on a thread of
//! its own. No egui.
//!
//! ⚠ The picture is the camera's embedded JPEG for a RAW, at its own size (4,416 px on an X-T4), and any other file's
//! own pixels at 2,048 px — never the 512-px thumbnail, where a group shot's faces are 15–25 px. A file not on disk
//! is skipped, never answered: an undownloaded Immich row has no file, and its faces are Immich's anyway.

use catalog::db::NewFace;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

/// A non-RAW file's long side for the search.
pub const OTHER_LONG: usize = 2048;

/// Finds and embeds the faces in one upright picture.
pub type FaceFinder = Arc<dyn Fn(&image::RgbImage) -> Result<Vec<NewFace>, String> + Send + Sync>;

/// The real finder: YuNet and SFace from `models`, loaded once on the pass's thread.
pub fn real_finder(models: PathBuf) -> FaceFinder {
    let loaded = std::sync::OnceLock::new();
    Arc::new(move |img| {
        let faces = loaded
            .get_or_init(|| {
                ai_mask::faces::Faces::load(
                    &models.join(ai_mask::ModelId::YunetFace.spec().file),
                    &models.join(ai_mask::ModelId::SfaceFace.spec().file),
                )
                .map_err(|e| e.to_string())
            })
            .as_ref()
            .map_err(Clone::clone)?;
        let found = faces.find(img).map_err(|e| e.to_string())?;
        found
            .iter()
            .map(|f| {
                Ok(NewFace {
                    bbox: f.bbox,
                    embedding: faces.embed(img, f).map_err(|e| e.to_string())?.to_vec(),
                })
            })
            .collect()
    })
}

/// The picture faces are looked for in.
fn picture(path: &std::path::Path) -> Option<image::RgbImage> {
    let t = crate::preview_loader::extract_camera(path)?;
    let t = if catalog::is_raw(path) {
        t
    } else {
        crate::preview_loader::cap_to(t, OTHER_LONG)
    };
    let rgba = image::RgbaImage::from_raw(t.width as u32, t.height as u32, t.rgba)?;
    Some(image::DynamicImage::ImageRgba8(rgba).to_rgb8())
}

/// One photo's answer: its faces, or `None` when it could not be read (it stays unread, to try again).
pub type Found = (PathBuf, Option<Vec<NewFace>>);

/// A running search.
pub struct FacePass {
    rx: Receiver<Found>,
    done: Arc<AtomicUsize>,
    total: usize,
    cancel: Arc<AtomicBool>,
    /// Photos that could not be searched, and the first reason — said to the user, never swallowed.
    failed: Arc<std::sync::Mutex<(usize, Option<String>)>>,
    failed_paths: Arc<std::sync::Mutex<Vec<PathBuf>>>,
}

impl FacePass {
    pub fn start(files: Vec<PathBuf>, finder: FaceFinder) -> Self {
        Self::with_picture(files, finder, picture)
    }

    /// [`FacePass::start`] with the picture reader handed in, for tests.
    pub fn with_picture(
        files: Vec<PathBuf>,
        finder: FaceFinder,
        picture: fn(&std::path::Path) -> Option<image::RgbImage>,
    ) -> Self {
        let (tx, rx) = channel();
        let done = Arc::new(AtomicUsize::new(0));
        let cancel = Arc::new(AtomicBool::new(false));
        let total = files.len();
        let failed = Arc::new(std::sync::Mutex::new((0usize, None::<String>)));
        let failed_paths = Arc::new(std::sync::Mutex::new(Vec::new()));
        let (d, c, fl, fp) = (
            done.clone(),
            cancel.clone(),
            failed.clone(),
            failed_paths.clone(),
        );
        std::thread::Builder::new()
            .name("face-pass".into())
            .spawn(move || {
                for f in files {
                    if c.load(Ordering::Relaxed) {
                        return;
                    }
                    let answer = if f.is_file() {
                        let r = picture(&f)
                            .ok_or_else(|| "the photo could not be read".to_string())
                            .and_then(|img| finder(&img));
                        match r {
                            Ok(faces) => Some(faces),
                            Err(e) => {
                                let mut g = fl.lock().unwrap_or_else(|p| p.into_inner());
                                g.0 += 1;
                                g.1.get_or_insert(e);
                                fp.lock().unwrap_or_else(|p| p.into_inner()).push(f.clone());
                                None
                            }
                        }
                    } else {
                        None
                    };
                    d.fetch_add(1, Ordering::Relaxed);
                    if answer.is_some() && tx.send((f, answer)).is_err() {
                        return;
                    }
                }
            })
            .expect("spawn the face pass");
        Self {
            rx,
            done,
            total,
            cancel,
            failed,
            failed_paths,
        }
    }

    /// The photos that could not be searched.
    pub fn failed_paths(&self) -> Vec<PathBuf> {
        self.failed_paths
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// How many photos could not be searched, and the first reason.
    pub fn failures(&self) -> (usize, Option<String>) {
        self.failed
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Everything found since the last call, and whether the pass is over.
    pub fn drain(&self) -> (Vec<Found>, bool) {
        let mut found = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok(f) => found.push(f),
                Err(std::sync::mpsc::TryRecvError::Empty) => return (found, false),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return (found, true),
            }
        }
    }

    /// `(photos looked at, photos in the pass)`.
    pub fn progress(&self) -> (usize, usize) {
        (self.done.load(Ordering::Relaxed), self.total)
    }

    /// Stop after the photo in hand.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn all(p: &FacePass) -> Vec<Found> {
        let t = Instant::now();
        let mut out = Vec::new();
        loop {
            let (f, done) = p.drain();
            out.extend(f);
            if done {
                return out;
            }
            assert!(t.elapsed() < Duration::from_secs(120), "the pass hung");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn one_face() -> FaceFinder {
        Arc::new(|_| {
            Ok(vec![NewFace {
                bbox: [0.1, 0.1, 0.2, 0.2],
                embedding: vec![0.0; 128],
            }])
        })
    }

    fn any_picture(_: &std::path::Path) -> Option<image::RgbImage> {
        Some(image::RgbImage::new(4, 4))
    }

    /// Review focus 3: a file not on disk — an undownloaded Immich row — is never answered.
    #[test]
    fn the_face_pass_skips_absent_files() {
        let d = test_dir::TempDir::new("face-pass");
        let here = d.join("a.JPG");
        std::fs::write(&here, b"x").unwrap();
        let p = FacePass::with_picture(
            vec![d.join("gone.RAF"), here.clone()],
            one_face(),
            any_picture,
        );
        let found = all(&p);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, here);
        assert_eq!(found[0].1.as_ref().map(Vec::len), Some(1));
        assert_eq!(p.progress(), (2, 2));
    }

    #[test]
    fn an_unreadable_picture_stays_unread() {
        let d = test_dir::TempDir::new("face-pass-unreadable");
        let here = d.join("a.JPG");
        std::fs::write(&here, b"not a jpeg").unwrap();
        let p = FacePass::start(vec![here], one_face());
        assert!(
            all(&p).is_empty(),
            "a photo that could not be read was marked read"
        );
        assert_eq!(
            p.failures(),
            (1, Some("the photo could not be read".into()))
        );
    }

    /// Found in use: a finder's error (a model that will not load) was swallowed and nothing appeared.
    #[test]
    fn a_finders_error_is_kept() {
        let d = test_dir::TempDir::new("face-pass-error");
        let here = d.join("a.JPG");
        std::fs::write(&here, b"x").unwrap();
        let broken: FaceFinder = Arc::new(|_| Err("model: bad file".into()));
        let p = FacePass::with_picture(vec![here], broken, any_picture);
        all(&p);
        assert_eq!(p.failures(), (1, Some("model: bad file".into())));
    }

    /// The real path end to end — the picture reader and both networks — on the crew portrait (a real photo,
    /// public domain). ⚠ Needs the models in the app's folder, so ignored:
    /// `cargo test --workspace --release the_real_pass -- --ignored`.
    #[test]
    #[ignore]
    fn the_real_pass_finds_the_crews_faces() {
        let d = test_dir::TempDir::new("face-pass-real");
        let photo = d.join("crew.jpg");
        std::fs::copy(
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../ai-mask/tests/fixtures/apollo11_crew.jpg"
            ),
            &photo,
        )
        .unwrap();
        let models = crate::ai_cache::models_dir().expect("a models folder");
        let p = FacePass::start(vec![photo.clone()], real_finder(models));
        let found = all(&p);
        println!("{found:?}");
        assert_eq!(found.len(), 1, "the photo was not answered");
        assert_eq!(found[0].1.as_ref().map(Vec::len), Some(3));
    }

    #[test]
    fn cancel_stops_it() {
        let d = test_dir::TempDir::new("face-pass-cancel");
        let files: Vec<PathBuf> = (0..50)
            .map(|i| {
                let p = d.join(format!("{i}.JPG"));
                std::fs::write(&p, b"x").unwrap();
                p
            })
            .collect();
        let slow: FaceFinder = Arc::new(|_| {
            std::thread::sleep(Duration::from_millis(20));
            Ok(Vec::new())
        });
        let p = FacePass::with_picture(files, slow, any_picture);
        p.cancel();
        let found = all(&p);
        assert!(found.len() < 50, "cancel did nothing: {}", found.len());
    }
}
