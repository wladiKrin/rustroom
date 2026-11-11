//! Learning the camera's colour in the app (M10.13d): the pass over the catalog's Fuji RAFs, on its own thread,
//! and what it changes when it lands. The decisions are `rustroom_app::camera_look`'s.
//!
//! ⚠ What a look is fitted on is what this app renders before applying one: Camera rendering at the photo's
//! fresh edits, through the camera profile chosen for its body — else the look corrects the wrong picture.

use crate::RustroomApp;
use eframe::egui;
use rustroom_app::camera_look::learn::{self, Candidate, Rendered, EDGE};
use rustroom_app::camera_look::store::Store;
use rustroom_app::camera_look::{fuji, store};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

/// What the pass reports.
pub(crate) enum Progress {
    /// Fitting this look now — the `n`th of `of`.
    Learning { label: String, n: usize, of: usize },
    /// Done: how many looks were learned.
    Done { learned: usize },
}

/// The running pass, if any.
#[derive(Default)]
pub(crate) struct CameraLearning {
    rx: Option<Receiver<Progress>>,
    stop: Option<Arc<AtomicBool>>,
    line: String,
}

impl CameraLearning {
    /// The status row's progress while it runs.
    pub(crate) fn line(&self) -> Option<&str> {
        self.rx.as_ref().map(|_| self.line.as_str())
    }
}

/// The catalog's Fuji RAFs with what their JPEGs were rendered with — read from each file's EXIF block.
fn candidates(paths: &[PathBuf], stop: &AtomicBool) -> Vec<Candidate> {
    paths
        .iter()
        .take_while(|_| !stop.load(Ordering::Relaxed))
        .filter_map(|p| {
            let look = fuji::read(p)?;
            let body = image_decode::quick_look(p).ok()??.capture.camera?;
            Some(Candidate {
                path: p.clone(),
                body,
                look,
            })
        })
        .collect()
}

/// One photo as the app renders it before a camera look: its fresh edits under Camera rendering, through the
/// profile chosen for its body, at [`EDGE`].
fn render(
    path: &Path,
    profiles: &BTreeMap<String, PathBuf>,
    edits: &edit_model::Edits,
) -> Option<Rendered> {
    let opts = raw_decode::DecodeOptions::preview();
    let mut d = raw_decode::decode_raw(path, &opts).ok()?;
    let profile = profiles
        .get(&format!("{} {}", d.metadata.make, d.metadata.model))
        .cloned();
    if profile.is_some() {
        d = raw_decode::decode_raw(
            path,
            &raw_decode::DecodeOptions {
                camera_profile: profile.clone(),
                ..opts
            },
        )
        .ok()?;
    }
    let (curve, baseline_ev) = crate::decode::profile_rendering(&profile);
    let img = d.image.downscale_to(EDGE * 2);
    let scale = img.width() as f32 / d.image.width().max(1) as f32;
    let out = pipeline::build_pipeline(
        edits,
        pipeline::RenderScale::new(scale),
        pipeline::TailRendering {
            curve: curve.as_deref(),
            baseline_ev,
            ..pipeline::TailRendering::default()
        },
    )
    .run(img)
    .downscale_to(EDGE);
    let px = out
        .to_rgb8()
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| p.map(|v| f32::from(v) / 255.0))
        .collect();
    Some((px, out.width(), out.height()))
}

/// The pass: candidates, the plan against what is learned, then each due look fitted and saved.
fn run(
    paths: Vec<PathBuf>,
    profiles: BTreeMap<String, PathBuf>,
    edits: edit_model::Edits,
    store: Store,
    stop: Arc<AtomicBool>,
    tx: std::sync::mpsc::Sender<Progress>,
) {
    let cands = candidates(&paths, &stop);
    let groups = learn::plan(&cands, &store.manifest());
    let mut learned = 0;
    for (n, g) in groups.iter().enumerate() {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        let _ = tx.send(Progress::Learning {
            label: store::label(&g.body, &g.film),
            n: n + 1,
            of: groups.len(),
        });
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs() as i64);
        let r = learn::learn(
            g,
            &|p| {
                (!stop.load(Ordering::Relaxed))
                    .then(|| render(p, &profiles, &edits))
                    .flatten()
            },
            &learn::camera_jpeg,
            &store,
            now,
        );
        if matches!(r, Ok(Some(_))) && !stop.load(Ordering::Relaxed) {
            learned += 1;
        }
    }
    let _ = tx.send(Progress::Done { learned });
}

impl RustroomApp {
    /// Start the pass when Camera matching is on and none runs — at startup, and after Relearn.
    pub(crate) fn start_camera_learning(&mut self) {
        if !self.settings.camera_match || self.camera_learning.rx.is_some() {
            return;
        }
        let Some(cat) = &self.catalog else { return };
        let Some(config) = dirs::config_dir() else {
            return;
        };
        let paths: Vec<PathBuf> = cat
            .all_photos()
            .unwrap_or_default()
            .into_iter()
            .filter(|r| r.copy == 0)
            .map(|r| r.path)
            .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("raf")))
            .collect();
        if paths.is_empty() {
            return;
        }
        let edits = edit_model::Edits {
            rendering: Some(edit_model::Rendering::Camera),
            ..self.settings.initial_edits(None)
        };
        let profiles = self.settings.camera_profiles.clone();
        let (tx, rx) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let s = Arc::clone(&stop);
        let store = Store::user_default(&config);
        std::thread::spawn(move || run(paths, profiles, edits, store, s, tx));
        self.camera_learning = CameraLearning {
            rx: Some(rx),
            stop: Some(stop),
            line: "Looking for the camera's film simulations…".into(),
        };
    }

    /// Settings' Relearn: forget every learned look and learn them again.
    pub(crate) fn relearn_camera_looks(&mut self) {
        self.cancel_camera_learning();
        if let Some(config) = dirs::config_dir() {
            if let Err(e) = Store::user_default(&config).clear() {
                self.status = format!("Could not forget the learned looks: {e}");
                return;
            }
        }
        self.start_camera_learning();
    }

    pub(crate) fn cancel_camera_learning(&mut self) {
        if let Some(stop) = self.camera_learning.stop.take() {
            stop.store(true, Ordering::Relaxed);
        }
        self.camera_learning = CameraLearning::default();
    }

    /// Once per frame: the progress, and — when looks were learned — the open photo decoded again, so it
    /// shows its film's look.
    pub(crate) fn pump_camera_learning(&mut self, ctx: &egui::Context) {
        let Some(rx) = &self.camera_learning.rx else {
            return;
        };
        let mut done = None;
        while let Ok(p) = rx.try_recv() {
            match p {
                Progress::Learning { label, n, of } => {
                    self.camera_learning.line =
                        format!("Learning the camera's colour — {label} ({n} of {of})");
                }
                Progress::Done { learned } => done = Some(learned),
            }
        }
        match done {
            Some(learned) => {
                self.camera_learning = CameraLearning::default();
                if learned > 0 {
                    self.status = match learned {
                        1 => "Learned the camera's colour for 1 film simulation".into(),
                        n => format!("Learned the camera's colour for {n} film simulations"),
                    };
                    // ⚠ The open photo was decoded before its look existed: decode it again.
                    self.cache.clear();
                    if let Some(s) = self.session.as_ref() {
                        let id = rustroom_app::photo_id::PhotoId::new(s.path.clone(), s.copy);
                        self.open_path(ctx, id);
                    }
                }
            }
            None => ctx.request_repaint_after(std::time::Duration::from_millis(250)),
        }
    }
}
