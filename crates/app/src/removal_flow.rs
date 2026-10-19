//! The Remove brush's fills for the open photo (M10.16c1): loaded or computed in the background and held
//! for the session. The wiring only — every decision is `rustroom_app::removal`'s.
//!
//! ⚠ Runs every frame from `pump_jobs`, which is what survives the render engine being REPLACED (the
//! full-resolution handover): a fresh engine's maps are empty, and the next frame re-inserts the fills
//! held here.

use crate::RustroomApp;
use eframe::egui;
use rustroom_app::denoise_tool::Next;
use rustroom_app::removal::{self, Facts};
use rustroom_app::removal_worker::{RemovalRequest, RemovalResult, RemovalWorker};
use rustroom_app::{removal_cache, remove_tool};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Default)]
pub(crate) struct RemovalState {
    worker: Option<RemovalWorker>,
    /// The open photo and its key, memoised — a stat and a digest, once.
    photo: Option<(PathBuf, String)>,
    /// The open photo's fills by key — re-inserted into a replaced engine's maps.
    held: HashMap<String, Arc<pipeline::Fill>>,
    /// Keys a job runs for.
    pending: HashSet<String>,
    /// Keys that failed — not retried this run, or a broken model would loop forever.
    failed: HashSet<String>,
    /// The download was offered this run — once, and a Cancel is respected.
    offered: bool,
    /// `(cache, models)` directories — `None` means the real ones. ⚠ Set only by tests.
    pub dirs: Option<(PathBuf, PathBuf)>,
}

impl RemovalState {
    fn dirs(&self) -> Option<(PathBuf, PathBuf)> {
        self.dirs
            .clone()
            .or_else(|| Some((removal_cache::dir()?, rustroom_app::ai_cache::models_dir()?)))
    }
}

impl RustroomApp {
    /// Once per frame, beside `pump_denoise`.
    pub(crate) fn pump_removals(&mut self, ctx: &egui::Context) {
        let results = self
            .removals
            .worker
            .as_ref()
            .map(|w| w.poll())
            .unwrap_or_default();
        self.absorb_removals(results);
        let Some(s) = &self.session else {
            self.removals.held.clear();
            return;
        };
        if self.removals.photo.as_ref().map(|(p, _)| p) != Some(&s.path) {
            self.removals.held.clear();
            self.removals.photo = Some((s.path.clone(), removal::photo_key(&s.path)));
        }
        let Some((cache, models)) = self.removals.dirs() else {
            return;
        };
        let photo = self
            .removals
            .photo
            .as_ref()
            .map(|(_, k)| k.clone())
            .unwrap_or_default();
        let keys: Vec<String> = s
            .edits
            .removals
            .iter()
            .map(|r| remove_tool::id(&photo, r, removal::MODEL))
            .collect();
        self.removals.held.retain(|k, _| keys.contains(k));
        let store = s.engine.ai_maps();
        // The held fills into the maps — only what is missing there, so a frame with nothing new takes no
        // write lock.
        let missing: Vec<(edit_model::Removal, Arc<pipeline::Fill>)> = {
            let maps = store.read().expect("ai maps");
            s.edits
                .removals
                .iter()
                .zip(&keys)
                .filter_map(|(r, k)| {
                    let f = self.removals.held.get(k)?;
                    let shown = maps.fill(r).is_some_and(|(g, _)| Arc::ptr_eq(&g, f));
                    (!shown).then(|| (r.clone(), Arc::clone(f)))
                })
                .collect()
        };
        if !missing.is_empty() {
            let mut maps = store.write().expect("ai maps");
            maps.retain_fills(&s.edits.removals);
            for (r, f) in missing {
                maps.insert_fill(&r, f);
            }
            drop(maps);
            s.request(
                s.view_edits(self.tool),
                s.render_max_dim(),
                s.requested_region,
            );
        }
        let need = removal::missing_models(&s.edits, &models);
        let full_decode = s.buffer_is_full_res && !s.buffer_is_ghost;
        let mut requests = Vec::new();
        let mut need_model = false;
        for (r, key) in s.edits.removals.iter().zip(&keys) {
            let models_present = !need.contains(&removal::MODEL)
                && !(r.detect && need.iter().any(|m| *m != removal::MODEL));
            let facts = Facts {
                has_fill: self.removals.held.contains_key(key),
                cached: removal_cache::exists(&cache, key),
                models_present,
                full_decode,
                running: self.removals.pending.contains(key),
                failed: self.removals.failed.contains(key),
            };
            let image = match removal::next(&facts) {
                Next::Load => None,
                Next::Run => Some(Arc::clone(&s.buffer)),
                Next::NeedModel => {
                    need_model = true;
                    continue;
                }
                Next::Nothing | Next::WaitForDecode => continue,
            };
            requests.push(RemovalRequest {
                key: key.clone(),
                image,
                removal: r.clone(),
                negative: s.edits.negative,
            });
        }
        if need_model
            && !self.removals.offered
            && self.ai.download.is_none()
            && self.modal == crate::modals::Modal::None
        {
            self.removals.offered = true;
            self.ai.proposed = need;
            self.modal = crate::modals::Modal::AiDownload;
        }
        if !requests.is_empty() {
            let worker = self.removals.worker.get_or_insert_with(|| {
                let repaint = ctx.clone();
                let (m1, m2) = (models.clone(), models.clone());
                RemovalWorker::spawn(
                    Box::new(move || removal::lama(&m1)),
                    Box::new(move || removal::sam(&m2)),
                    Some(cache),
                    move || repaint.request_repaint(),
                )
            });
            for req in requests {
                self.removals.pending.insert(req.key.clone());
                worker.request(req);
            }
        }
        if !self.removals.pending.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }

    /// What the job sent: a fill held, a failure remembered, a job's end cleared — testable without a
    /// worker.
    pub(crate) fn absorb_removals(&mut self, results: Vec<RemovalResult>) {
        for r in results {
            match r {
                RemovalResult::Done { key, fill } => {
                    self.removals.pending.remove(&key);
                    self.removals.held.insert(key, fill);
                }
                RemovalResult::Missing { key } => {
                    self.removals.pending.remove(&key);
                }
                RemovalResult::Failed { key, error } => {
                    self.removals.pending.remove(&key);
                    self.status = format!("Remove failed: {error}");
                    self.removals.failed.insert(key);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An app with a photo open at full resolution, one removal on it, its own empty directories.
    fn app_with_removal(tag: &str) -> (RustroomApp, egui::Context, edit_model::Removal) {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 40, 20);
        let d = test_dir::path(&format!("removalflow-{tag}"));
        app.removals.dirs = Some((d.join("cache"), d.join("models")));
        let r = edit_model::Removal {
            points: vec![[0.5, 0.5]],
            radius: 0.1,
            detect: false,
            variant: 0,
            opacity: 100.0,
        };
        app.session.as_mut().unwrap().edits.removals = vec![r.clone()];
        (app, ctx, r)
    }

    fn shown(app: &RustroomApp, r: &edit_model::Removal) -> bool {
        let s = app.session.as_ref().unwrap();
        let maps = s.engine.ai_maps();
        let got = maps.read().unwrap().fill(r).is_some();
        got
    }

    /// A finished fill lands in the maps the render reads — and leaves them with its removal.
    #[test]
    fn a_result_lands_in_the_maps_and_renders() {
        let (mut app, ctx, r) = app_with_removal("lands");
        app.pump_removals(&ctx);
        assert!(!shown(&app, &r), "nothing yet");
        let key = remove_tool::id(
            app.removals
                .photo
                .as_ref()
                .map(|(_, k)| k.as_str())
                .expect("seen"),
            &r,
            removal::MODEL,
        );
        let fill = Arc::new(pipeline::Fill {
            rect: [0.25, 0.25, 0.75, 0.75],
            w: 2,
            h: 2,
            rgb: vec![0.5; 12],
            alpha: vec![1.0; 4],
        });
        app.absorb_removals(vec![RemovalResult::Done { key, fill }]);
        app.pump_removals(&ctx);
        assert!(shown(&app, &r), "the fill is in the maps");
        let faint = edit_model::Removal {
            opacity: 40.0,
            ..r.clone()
        };
        app.session.as_mut().unwrap().edits.removals = vec![faint.clone()];
        app.pump_removals(&ctx);
        assert!(shown(&app, &faint), "Opacity keeps the fill");
        app.session.as_mut().unwrap().edits.removals.clear();
        app.pump_removals(&ctx);
        assert!(!app.removals.held.contains_key(&remove_tool::id(
            app.removals
                .photo
                .as_ref()
                .map(|(_, k)| k.as_str())
                .unwrap(),
            &r,
            removal::MODEL
        )));
    }

    /// No model: the download is offered, once, naming the inpainting network.
    #[test]
    fn the_offer_proposes_the_inpainting_model() {
        let (mut app, ctx, _) = app_with_removal("offer");
        app.pump_removals(&ctx);
        assert!(app.modal == crate::modals::Modal::AiDownload, "the offer");
        assert_eq!(app.ai.proposed, vec![removal::MODEL]);
        app.modal = crate::modals::Modal::None;
        app.pump_removals(&ctx);
        assert!(
            app.modal == crate::modals::Modal::None,
            "a Cancel is respected"
        );
    }

    /// A failed fill says why and is not asked for again.
    #[test]
    fn a_failure_is_reported_and_not_retried() {
        let (mut app, ctx, r) = app_with_removal("failed");
        app.pump_removals(&ctx);
        let key = remove_tool::id(
            app.removals
                .photo
                .as_ref()
                .map(|(_, k)| k.as_str())
                .unwrap(),
            &r,
            removal::MODEL,
        );
        app.removals.pending.insert(key.clone());
        app.absorb_removals(vec![RemovalResult::Failed {
            key: key.clone(),
            error: "no LaMa".into(),
        }]);
        assert!(app.status.contains("no LaMa"), "{}", app.status);
        assert!(!app.removals.pending.contains(&key));
        assert!(app.removals.failed.contains(&key));
    }
}
