//! AI masks for the open photo (M9.13): making sure every prompt the edits ask has its map, and
//! downloading the models the first time one is needed.
//!
//! ⚠ The deciding is in the lib (`ai_worker`, `ai_cache`, `ai_coverage`); this is the wiring, and
//! it runs every frame from `pump_ai`, which is what makes it robust to the render engine being
//! **replaced** (the full-resolution handover, a photo switch): a fresh engine's store is empty, and
//! the next frame refills it from the disk cache in a few milliseconds.

use crate::RustroomApp;
use ai_mask::ModelId;
use edit_model::AiPrompt;
use eframe::egui;
use rustroom_app::ai_coverage::MapQuery;
use rustroom_app::ai_worker::{AiJob, AiRequest, AiWorker, TractRunner};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::Arc;

/// A model download in progress.
pub(crate) struct Download {
    pub received: Arc<AtomicU64>,
    pub total: u64,
    done: Receiver<Result<(), String>>,
    pub error: Option<String>,
}

#[derive(Default)]
pub(crate) struct AiState {
    worker: Option<AiWorker>,
    /// Requests in flight: cache key → (photo, query).
    pending: HashMap<String, (PathBuf, MapQuery)>,
    /// Cache keys that failed — not retried this run, or a broken model would loop forever.
    failed: HashSet<String>,
    /// The missing models the dialog was already offered for this run — it opens once, not every
    /// frame, and a Cancel is respected until something asks again.
    offered: HashSet<ModelId>,
    /// The last photo's model inputs, so a second click does not re-render one — keyed on the film
    /// negative too (M10.11a), since the model is shown the positive, and on the long side (M10.16a):
    /// Subject and Select read 1024 px, Depth the stored size of its recipe. Two at most.
    input: Vec<(
        PathBuf,
        u8,
        edit_model::Negative,
        usize,
        Arc<image::RgbImage>,
    )>,
    /// The photo and film negative the session's maps were found for (M10.11a). A pick or a
    /// re-measure on the same photo makes them another positive's, and they are dropped.
    maps_for: Option<(PathBuf, edit_model::Negative)>,
    pub download: Option<Download>,
    /// The models the download dialog proposes.
    pub proposed: Vec<ModelId>,
    /// `(cache, models)` directories — `None` means the real ones. ⚠ Set only by tests, which must
    /// never read or fill the user's own model and cache directories.
    pub dirs: Option<(PathBuf, PathBuf)>,
}

impl AiState {
    /// The map cache and the models directory.
    fn dirs(&self) -> Option<(PathBuf, PathBuf)> {
        self.dirs.clone().or_else(|| {
            Some((
                rustroom_app::ai_cache::dir()?,
                rustroom_app::ai_cache::models_dir()?,
            ))
        })
    }
}

/// The models `query` needs.
fn needed(query: &MapQuery, subject: ai_mask::SubjectModel) -> Vec<ModelId> {
    match query {
        MapQuery::Prompt(AiPrompt::Subject) => vec![subject.model()],
        MapQuery::Prompt(AiPrompt::Click { .. }) => ai_mask::registry::CLICK_MODELS.to_vec(),
        MapQuery::Depth => vec![ModelId::DepthAnythingV2Small],
    }
}

/// The long side of the image `query`'s network is handed.
fn input_long(query: &MapQuery) -> usize {
    match query {
        MapQuery::Prompt(_) => 1024,
        MapQuery::Depth => rustroom_app::depth_map::RECIPE.store_long as usize,
    }
}

impl RustroomApp {
    /// Once per frame, beside `pump_export`.
    pub(crate) fn pump_ai(&mut self, ctx: &egui::Context) {
        self.pump_ai_download();
        self.pump_ai_results();
        self.ensure_ai_maps(ctx);
        if self.ai.worker.is_some() && !self.ai.pending.is_empty() || self.ai.download.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    fn pump_ai_results(&mut self) {
        let Some(w) = &self.ai.worker else { return };
        for r in w.poll() {
            let Some((photo, query)) = self.ai.pending.remove(&r.key) else {
                continue;
            };
            match r.map {
                Ok(map) => {
                    self.status = match query {
                        MapQuery::Prompt(AiPrompt::Subject) => "Subject found".to_string(),
                        MapQuery::Prompt(AiPrompt::Click { .. }) => "Selection found".to_string(),
                        MapQuery::Depth => "Depth estimated".to_string(),
                    };
                    if let Some(s) = &self.session {
                        // ⚠ M10.11a: and only if it was found for the negative the photo has now —
                        // a pick while the model ran makes it another positive's map.
                        let current = rustroom_app::ai_cache::key(
                            &photo,
                            rustroom_app::ai_coverage::model_for(
                                &query,
                                self.settings.ai_subject_model,
                            ),
                            &query,
                            &s.edits.negative,
                        );
                        if s.path == photo && r.key == current {
                            s.engine
                                .ai_maps()
                                .write()
                                .expect("ai maps")
                                .insert(query, map);
                            s.request(
                                s.view_edits(self.tool),
                                s.render_max_dim(),
                                s.requested_region,
                            );
                        }
                    }
                }
                Err(e) => {
                    self.status = format!("AI mask failed: {e}");
                    self.ai.failed.insert(r.key);
                }
            }
        }
    }

    /// Every map the open photo's edits read must be there — from the store, the disk cache, or the
    /// worker. A model that is missing opens the download dialog, once.
    fn ensure_ai_maps(&mut self, ctx: &egui::Context) {
        let Some(s) = &self.session else { return };
        let queries = rustroom_app::ai_coverage::queries(&s.edits);
        if queries.is_empty() {
            return;
        }
        let Some((cache, models)) = self.ai.dirs() else {
            return;
        };
        // ⚠⚠ M10.11a: maps found for this photo under another film negative are another positive's.
        let now = (s.path.clone(), s.edits.negative);
        if self.ai.maps_for.as_ref() != Some(&now) {
            if self.ai.maps_for.as_ref().is_some_and(|(p, _)| *p == now.0) {
                s.engine.ai_maps().write().expect("ai maps").clear();
            }
            self.ai.maps_for = Some(now);
        }
        let subject = self.settings.ai_subject_model;
        let photo = s.path.clone();
        let store = s.engine.ai_maps();
        let mut rendered_new = false;
        let mut to_request: Vec<(String, MapQuery)> = Vec::new();
        let mut missing_models: Vec<ModelId> = Vec::new();
        for p in queries {
            if store.read().expect("ai maps").get(&p).is_some() {
                continue;
            }
            let key = rustroom_app::ai_cache::key(
                &photo,
                rustroom_app::ai_coverage::model_for(&p, subject),
                &p,
                &s.edits.negative,
            );
            if self.ai.pending.contains_key(&key) || self.ai.failed.contains(&key) {
                continue;
            }
            if let Some(m) = rustroom_app::ai_cache::load(&cache, &key) {
                store.write().expect("ai maps").insert(p, Arc::new(m));
                rendered_new = true;
                continue;
            }
            let need = needed(&p, subject);
            let absent: Vec<ModelId> = need
                .into_iter()
                .filter(|m| !ai_mask::download::present(&m.spec(), &models))
                .collect();
            if absent.is_empty() {
                to_request.push((key, p));
            } else {
                missing_models.extend(absent);
            }
        }
        if rendered_new {
            s.request(
                s.view_edits(self.tool),
                s.render_max_dim(),
                s.requested_region,
            );
        }
        missing_models.sort_by_key(|m| *m as u8);
        missing_models.dedup();
        let fresh: Vec<ModelId> = missing_models
            .iter()
            .copied()
            .filter(|m| !self.ai.offered.contains(m))
            .collect();
        if !fresh.is_empty()
            && self.ai.download.is_none()
            && self.modal == crate::modals::Modal::None
        {
            self.ai.offered.extend(fresh);
            self.ai.proposed = missing_models;
            self.modal = crate::modals::Modal::AiDownload;
        }
        // ⚠⚠ Never from a ghost session's buffer — the 512-px thumbnail (M9.14) — whose map would be
        // cached under the FILE's key for good: a soft Subject, a depth map stretched from 512 to 1536.
        // Cached maps were loaded above; the request waits for the real decode, a frame or two away.
        if to_request.is_empty() || s.buffer_is_ghost {
            return;
        }
        let turns = s.edits.geometry.orientation.rotate.turns();
        if self.ai.worker.is_none() {
            let repaint = ctx.clone();
            self.ai.worker = Some(AiWorker::spawn(
                Box::new(TractRunner::new(models)),
                Some(cache),
                move || repaint.request_repaint(),
            ));
        }
        for (key, p) in to_request {
            let long = input_long(&p);
            let held = self.ai.input.iter().find(|(ph, t, n, l, _)| {
                *ph == photo && *t == turns && *n == s.edits.negative && *l == long
            });
            let input = match held {
                Some((.., img)) => Arc::clone(img),
                None => {
                    let img = Arc::new(rustroom_app::ai_worker::model_input(
                        &s.buffer,
                        long,
                        turns,
                        &s.edits.negative,
                    ));
                    self.ai.input.insert(
                        0,
                        (
                            photo.clone(),
                            turns,
                            s.edits.negative,
                            long,
                            Arc::clone(&img),
                        ),
                    );
                    self.ai.input.truncate(2);
                    img
                }
            };
            self.status = match p {
                MapQuery::Prompt(AiPrompt::Subject) => "Finding the subject…".to_string(),
                MapQuery::Prompt(AiPrompt::Click { .. }) => "Selecting…".to_string(),
                MapQuery::Depth => "Estimating depth…".to_string(),
            };
            let w = self.ai.worker.as_ref().expect("just spawned");
            w.request(AiRequest {
                key: key.clone(),
                photo: photo.clone(),
                image: input,
                turns,
                job: match p {
                    MapQuery::Prompt(AiPrompt::Subject) => AiJob::Subject(subject),
                    MapQuery::Prompt(AiPrompt::Click { point }) => AiJob::Click(point),
                    MapQuery::Depth => AiJob::Depth,
                },
            });
            self.ai.pending.insert(key, (photo.clone(), p));
        }
    }

    /// Start fetching `self.ai.proposed` on a background thread.
    pub(crate) fn start_ai_download(&mut self) {
        let Some((_, dir)) = self.ai.dirs() else {
            return;
        };
        let models = self.ai.proposed.clone();
        let total = models.iter().map(|m| m.spec().bytes).sum();
        let received = Arc::new(AtomicU64::new(0));
        let (tx, done) = std::sync::mpsc::channel();
        let counter = Arc::clone(&received);
        let list = models.clone();
        std::thread::spawn(move || {
            let mut base = 0u64;
            for m in list {
                let spec = m.spec();
                if !ai_mask::download::present(&spec, &dir) {
                    if let Err(e) = ai_mask::download::fetch(&spec, &dir, |n| {
                        counter.store(base + n, Ordering::Relaxed)
                    }) {
                        let _ = tx.send(Err(format!("{}: {e}", spec.file)));
                        return;
                    }
                }
                base += spec.bytes;
                counter.store(base, Ordering::Relaxed);
            }
            let _ = tx.send(Ok(()));
        });
        self.ai.download = Some(Download {
            received,
            total,
            done,
            error: None,
        });
    }

    fn pump_ai_download(&mut self) {
        let Some(d) = &mut self.ai.download else {
            return;
        };
        match d.done.try_recv() {
            Ok(Ok(())) => {
                self.ai.download = None;
                self.status = "AI models downloaded".to_string();
                if self.modal == crate::modals::Modal::AiDownload {
                    self.modal = crate::modals::Modal::None;
                }
            }
            Ok(Err(e)) => d.error = Some(e),
            Err(_) => {}
        }
    }

    /// Close the download dialog (Cancel, or Close after an error).
    pub(crate) fn close_ai_download(&mut self) {
        self.ai.download = None;
        self.modal = crate::modals::Modal::None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        test_dir::path(&format!("aiflow-{tag}"))
    }

    /// An app with a photo open, a Subject mask on it, and its own empty cache and models dirs.
    fn app_with_subject(tag: &str) -> (RustroomApp, egui::Context, PathBuf) {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 40, 20);
        let d = temp_dir(tag);
        let (cache, models) = (d.join("cache"), d.join("models"));
        app.ai.dirs = Some((cache.clone(), models));
        let s = app.session.as_mut().unwrap();
        s.edits.masks.push(edit_model::MaskedAdjustment {
            mask: edit_model::Mask::Ai(edit_model::AiMask::default()),
            ..Default::default()
        });
        (app, ctx, cache)
    }

    /// M10.11a: a map cached for the photo as a negative is not loaded onto its positive.
    #[test]
    fn a_map_found_on_the_negative_is_not_the_positives() {
        let (mut app, ctx, cache) = app_with_subject("negative_key");
        let photo = app.session.as_ref().unwrap().path.clone();
        let off = edit_model::Negative::default();
        let key = rustroom_app::ai_cache::key(
            &photo,
            ModelId::IsNet,
            &MapQuery::Prompt(AiPrompt::Subject),
            &off,
        );
        let map = ai_mask::ProbMap::from_fn(4, 2, |x, _| x as f32 / 3.0);
        rustroom_app::ai_cache::store(&cache, &key, &map).unwrap();
        app.session.as_mut().unwrap().edits.negative = edit_model::Negative {
            on: true,
            base: [0.85, 0.55, 0.3],
            ..off
        };
        app.pump_ai(&ctx);
        let s = app.session.as_ref().unwrap();
        let got = s
            .engine
            .ai_maps()
            .read()
            .unwrap()
            .get(&MapQuery::Prompt(AiPrompt::Subject))
            .cloned();
        assert!(
            got.is_none(),
            "the negative's map was shown on the positive"
        );
    }

    /// M10.11a: a map the session already holds, found before the base was picked, is dropped when
    /// the photo becomes a positive — it outlines the negative's subject.
    #[test]
    fn picking_the_film_base_drops_the_maps_found_on_the_negative() {
        let (mut app, ctx, _) = app_with_subject("negative_store");
        let map = std::sync::Arc::new(ai_mask::ProbMap::from_fn(4, 2, |x, _| x as f32 / 3.0));
        app.pump_ai(&ctx);
        app.session
            .as_ref()
            .unwrap()
            .engine
            .ai_maps()
            .write()
            .unwrap()
            .insert(MapQuery::Prompt(AiPrompt::Subject), map);
        app.pump_ai(&ctx);
        let held = |app: &RustroomApp| {
            let s = app.session.as_ref().unwrap();
            let maps = s.engine.ai_maps();
            let got = maps
                .read()
                .unwrap()
                .get(&MapQuery::Prompt(AiPrompt::Subject))
                .is_some();
            got
        };
        assert!(held(&app), "the control: an unchanged photo keeps its map");
        app.session.as_mut().unwrap().edits.negative = edit_model::Negative {
            on: true,
            base: [0.85, 0.55, 0.3],
            ..edit_model::Negative::default()
        };
        app.pump_ai(&ctx);
        assert!(!held(&app), "the negative's map was kept on the positive");
    }

    /// ⚠⚠ M10.16a's review: a ghost session's buffer is the 512-px thumbnail (M9.14), and a map asked
    /// of it would be cached under the FILE's key for good — a soft Subject, a depth map stretched from
    /// 512 to 1536. Nothing is asked until the real decode lands; the model is made "present" (a
    /// sparse file at its pinned size) so only the ghost can be what holds the request back.
    #[test]
    fn a_ghost_session_asks_the_network_for_nothing() {
        let (mut app, ctx, _) = app_with_subject("ghost");
        let (_, models) = app.ai.dirs.clone().unwrap();
        std::fs::create_dir_all(&models).unwrap();
        let spec = ModelId::IsNet.spec();
        std::fs::File::create(models.join(spec.file))
            .unwrap()
            .set_len(spec.bytes)
            .unwrap();
        app.session.as_mut().unwrap().buffer_is_ghost = true;
        app.pump_ai(&ctx);
        assert!(
            app.ai.pending.is_empty(),
            "a map was asked of the 512-px ghost"
        );
        // The control: the real decode landed — now the map is asked for.
        app.session.as_mut().unwrap().buffer_is_ghost = false;
        app.pump_ai(&ctx);
        assert!(
            !app.ai.pending.is_empty(),
            "with the real decode nothing was asked"
        );
    }

    #[test]
    fn a_missing_model_offers_the_download_once() {
        let (mut app, ctx, _) = app_with_subject("offer");
        app.pump_ai(&ctx);
        assert!(app.modal == crate::modals::Modal::AiDownload);
        assert_eq!(app.ai.proposed, vec![ModelId::IsNet]);
        assert!(app.ai.worker.is_none(), "nothing may run without its model");
        // Not now: the dialog does not come back on the next frame.
        app.close_ai_download();
        app.pump_ai(&ctx);
        assert!(app.modal == crate::modals::Modal::None);
    }

    #[test]
    fn a_cached_map_is_loaded_without_a_model_or_a_worker() {
        let (mut app, ctx, cache) = app_with_subject("cached");
        let photo = app.session.as_ref().unwrap().path.clone();
        let key = rustroom_app::ai_cache::key(
            &photo,
            ModelId::IsNet,
            &MapQuery::Prompt(AiPrompt::Subject),
            &edit_model::Negative::default(),
        );
        let map = ai_mask::ProbMap::from_fn(4, 2, |x, _| x as f32 / 3.0);
        rustroom_app::ai_cache::store(&cache, &key, &map).unwrap();
        app.pump_ai(&ctx);
        let s = app.session.as_ref().unwrap();
        let store = s.engine.ai_maps();
        let got = store
            .read()
            .unwrap()
            .get(&MapQuery::Prompt(AiPrompt::Subject))
            .cloned();
        assert_eq!(got.as_deref(), Some(&map));
        assert!(
            app.modal == crate::modals::Modal::None,
            "a cached map needs no model"
        );
        assert!(app.ai.worker.is_none());
    }
}
