//! AI denoise for the open photo (M10.16b): its result loaded or computed in the background and held for
//! the session. The wiring only — every decision is `rustroom_app::denoise_tool`'s.
//!
//! ⚠ Runs every frame from `pump_jobs`, which is what survives the render engine being REPLACED (the
//! full-resolution handover): a fresh engine's store is empty, and the next frame re-inserts the result
//! held here — never re-reading 135 MB from disk.

use crate::RustroomApp;
use eframe::egui;
use raw_decode::Demosaic;
use rustroom_app::denoise_tool::{self, Facts, Next};
use rustroom_app::denoise_worker::{DenoiseRequest, DenoiseResult, DenoiseWorker};
use rustroom_app::denoised::{self, Denoised};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Default)]
pub(crate) struct DenoiseState {
    worker: Option<DenoiseWorker>,
    /// The open photo's key, memoised on its path and demosaic quality — a stat and a digest, once.
    key: Option<(PathBuf, Demosaic, String)>,
    /// The open photo's result, by key — re-inserted into a replaced engine's store.
    held: Option<(String, Arc<Denoised>)>,
    /// The key a job runs for.
    pending: Option<String>,
    /// Keys that failed — not retried this run, or a broken model would loop forever.
    failed: HashSet<String>,
    /// The download was offered this run — once, and a Cancel is respected.
    offered: bool,
    /// `(cache, models)` directories — `None` means the real ones. ⚠ Set only by tests.
    pub dirs: Option<(PathBuf, PathBuf)>,
    /// The results' total for Settings → Performance — measured once per opening, `None` after a clear.
    pub usage: Option<rustroom_app::denoise_cache::Usage>,
}

impl DenoiseState {
    fn dirs(&self) -> Option<(PathBuf, PathBuf)> {
        self.dirs.clone().or_else(|| {
            Some((
                rustroom_app::denoise_cache::dir()?,
                rustroom_app::ai_cache::models_dir()?,
            ))
        })
    }

    /// Where the results are — the Settings block's and the Clear dialog's folder.
    pub(crate) fn results_dir(&self) -> Option<PathBuf> {
        self.dirs().map(|(cache, _)| cache)
    }

    /// The open photo's facts this frame, `key` its key.
    fn facts(&self, s: &crate::Session, key: &str, cache: &Path, models: &Path) -> Facts {
        Facts {
            active: s.edits.ai_denoise.is_active(),
            has_result: self.held.as_ref().is_some_and(|(k, _)| k == key),
            cached: rustroom_app::denoise_cache::exists(cache, key),
            full_decode: s.buffer_is_full_res && !s.buffer_is_ghost,
            model_present: ai_mask::download::present(
                &ai_mask::ModelId::DrunetColor.spec(),
                models,
            ),
            running: self.pending.as_deref() == Some(key),
            failed: self.failed.contains(key),
        }
    }

    fn key_for(&mut self, photo: &Path, tier: Demosaic) -> String {
        let tier = denoised::tier_for(photo, tier);
        match &self.key {
            Some((p, t, k)) if p == photo && *t == tier => k.clone(),
            _ => {
                let k = rustroom_app::denoise_cache::key(photo, tier);
                self.key = Some((photo.to_path_buf(), tier, k.clone()));
                k
            }
        }
    }

    /// The tiles done of total of the open photo's job, while it runs — the Detail block's line.
    pub(crate) fn progress(&self) -> Option<(usize, usize)> {
        let key = self.pending.as_ref()?;
        self.worker.as_ref()?.progress(key)
    }
}

/// The Detail block's line for the open photo — `denoise_tool::status` on this frame's facts, read with
/// the key `pump_denoise` memoised this frame.
pub(crate) fn status(st: &DenoiseState, s: &crate::Session) -> Option<String> {
    let (photo, _, key) = st.key.as_ref()?;
    if *photo != s.path {
        return None;
    }
    let (cache, models) = st.dirs()?;
    denoise_tool::status(&st.facts(s, key, &cache, &models), st.progress())
}

impl RustroomApp {
    /// Once per frame, beside `pump_ai`.
    pub(crate) fn pump_denoise(&mut self, ctx: &egui::Context) {
        self.pump_denoise_results();
        let Some(s) = &self.session else {
            self.denoise.held = None;
            return;
        };
        let key = self.denoise.key_for(&s.path, self.settings.view_demosaic);
        if self.denoise.held.as_ref().is_some_and(|(k, _)| *k != key) {
            self.denoise.held = None;
        }
        if !s.edits.ai_denoise.is_active() {
            return;
        }
        let Some((cache, models)) = self.denoise.dirs() else {
            return;
        };
        let full_decode = s.buffer_is_full_res && !s.buffer_is_ghost;
        // ⚠⚠ Into the store only over the decode it was computed on — never the half-size one or a ghost,
        // whose pixels it would be blended with out of place.
        if let Some((k, d)) = &self.denoise.held {
            let id = denoised::id_of(k);
            let store = s.engine.ai_maps();
            let fits = d.size() == (s.buffer.width(), s.buffer.height());
            let shown = store.read().expect("ai maps").denoised().map(|(_, i)| *i) == Some(id);
            if full_decode && fits && !shown {
                store
                    .write()
                    .expect("ai maps")
                    .set_denoised(Some((Arc::clone(d), id)));
                s.request(
                    s.view_edits(self.tool),
                    s.render_max_dim(),
                    s.requested_region,
                );
            }
        }
        let facts = self.denoise.facts(s, &key, &cache, &models);
        let image = match denoise_tool::next(&facts) {
            Next::Nothing | Next::WaitForDecode => None,
            Next::Load => Some(None),
            Next::Run => Some(Some(Arc::clone(&s.buffer))),
            Next::NeedModel => {
                if !self.denoise.offered
                    && self.ai.download.is_none()
                    && self.modal == crate::modals::Modal::None
                {
                    self.denoise.offered = true;
                    self.ai.proposed = vec![ai_mask::ModelId::DrunetColor];
                    self.modal = crate::modals::Modal::AiDownload;
                }
                None
            }
        };
        if let Some(image) = image {
            let worker = self.denoise.worker.get_or_insert_with(|| {
                let repaint = ctx.clone();
                DenoiseWorker::spawn(
                    Box::new(move || denoised::drunet(&models)),
                    Some(cache),
                    move || repaint.request_repaint(),
                )
            });
            worker.request(DenoiseRequest {
                key: key.clone(),
                image,
            });
            self.denoise.pending = Some(key);
        }
        if self.denoise.pending.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }

    fn pump_denoise_results(&mut self) {
        let Some(w) = &self.denoise.worker else {
            return;
        };
        let open = self.denoise.key.as_ref().map(|(_, _, k)| k.clone());
        for r in w.poll() {
            let key = match &r {
                DenoiseResult::Done { key, .. } | DenoiseResult::Failed { key, .. } => key.clone(),
            };
            if self.denoise.pending.as_ref() == Some(&key) {
                self.denoise.pending = None;
            }
            match r {
                // ⚠ Only the open photo's: a job finished for the photo before is in the cache already.
                DenoiseResult::Done { image, .. } if open.as_ref() == Some(&key) => {
                    self.denoise.held = Some((key, Arc::new(Denoised::new(image))));
                    self.status = "Denoised".to_string();
                }
                DenoiseResult::Done { .. } => {}
                DenoiseResult::Failed { error, .. } => {
                    self.status = format!("AI denoise failed: {error}");
                    self.denoise.failed.insert(key);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An app with a photo open at full resolution, AI denoise at `amount`, its own empty directories.
    fn app_with_denoise(tag: &str, amount: f32) -> (RustroomApp, egui::Context, PathBuf) {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 40, 20);
        let d = test_dir::path(&format!("denoiseflow-{tag}"));
        let cache = d.join("cache");
        app.denoise.dirs = Some((cache.clone(), d.join("models")));
        let s = app.session.as_mut().unwrap();
        s.edits.ai_denoise.amount = amount;
        (app, ctx, cache)
    }

    fn shown(app: &RustroomApp) -> Option<(usize, usize)> {
        let s = app.session.as_ref().unwrap();
        let maps = s.engine.ai_maps();
        let got = maps.read().unwrap().denoised().map(|(d, _)| d.size());
        got
    }

    /// A result in the cache is loaded on open — no model needed — and shown over the full decode.
    #[test]
    fn a_cached_result_is_loaded_on_open() {
        let (mut app, ctx, cache) = app_with_denoise("cached", 100.0);
        let (photo, w, h) = {
            let s = app.session.as_ref().unwrap();
            (s.path.clone(), s.buffer.width(), s.buffer.height())
        };
        assert!(
            app.session.as_ref().unwrap().buffer_is_full_res,
            "the fixture"
        );
        let key = rustroom_app::denoise_cache::key(
            &photo,
            denoised::tier_for(&photo, app.settings.view_demosaic),
        );
        let img = core_image::ImageBuf::from_vec(w, h, vec![0.3; w * h * 3]);
        rustroom_app::denoise_cache::store(&cache, &key, &img).unwrap();
        for _ in 0..200 {
            app.pump_denoise(&ctx);
            if shown(&app).is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(shown(&app), Some((w, h)));
        assert!(
            app.modal == crate::modals::Modal::None,
            "no download offered"
        );
    }

    /// Review Focus 1: never over the half-size decode — held, and shown once the full one is adopted.
    #[test]
    fn a_result_waits_for_the_full_decode() {
        let (mut app, ctx, cache) = app_with_denoise("half", 100.0);
        let (photo, w, h) = {
            let s = app.session.as_mut().unwrap();
            s.buffer_is_full_res = false;
            (s.path.clone(), s.buffer.width(), s.buffer.height())
        };
        let key = rustroom_app::denoise_cache::key(
            &photo,
            denoised::tier_for(&photo, app.settings.view_demosaic),
        );
        let img = core_image::ImageBuf::from_vec(w, h, vec![0.3; w * h * 3]);
        rustroom_app::denoise_cache::store(&cache, &key, &img).unwrap();
        for _ in 0..200 {
            app.pump_denoise(&ctx);
            if app.denoise.held.is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(app.denoise.held.is_some(), "loaded");
        app.pump_denoise(&ctx);
        assert_eq!(shown(&app), None, "shown over the half decode");
        app.session.as_mut().unwrap().buffer_is_full_res = true;
        app.pump_denoise(&ctx);
        assert_eq!(shown(&app), Some((w, h)));
    }

    /// Without a result or a model the download is offered, once.
    #[test]
    fn a_missing_model_is_offered_once() {
        let (mut app, ctx, _) = app_with_denoise("model", 50.0);
        app.pump_denoise(&ctx);
        assert!(app.modal == crate::modals::Modal::AiDownload, "not offered");
        assert_eq!(app.ai.proposed, vec![ai_mask::ModelId::DrunetColor]);
        app.modal = crate::modals::Modal::None;
        app.pump_denoise(&ctx);
        assert!(app.modal == crate::modals::Modal::None, "offered again");
    }

    /// The Detail line follows the facts — waiting for the decode, then nothing once the result is in.
    #[test]
    fn the_line_says_why_nothing_shows_yet() {
        let (mut app, ctx, _) = app_with_denoise("line", 100.0);
        let models = app.denoise.dirs.as_ref().unwrap().1.clone();
        let spec = ai_mask::ModelId::DrunetColor.spec();
        std::fs::create_dir_all(&models).unwrap();
        std::fs::File::create(models.join(spec.file))
            .unwrap()
            .set_len(spec.bytes)
            .unwrap();
        app.session.as_mut().unwrap().buffer_is_full_res = false;
        app.pump_denoise(&ctx);
        let line = status(&app.denoise, app.session.as_ref().unwrap());
        assert_eq!(
            line.as_deref(),
            Some("Waiting for the full-resolution decode…")
        );
        app.session.as_mut().unwrap().edits.ai_denoise.amount = 0.0;
        assert_eq!(status(&app.denoise, app.session.as_ref().unwrap()), None);
    }

    /// At Amount 0 nothing is asked for.
    #[test]
    fn off_asks_for_nothing() {
        let (mut app, ctx, _) = app_with_denoise("off", 0.0);
        app.pump_denoise(&ctx);
        assert!(app.modal == crate::modals::Modal::None, "a dialog opened");
        assert!(app.denoise.worker.is_none());
    }
}
