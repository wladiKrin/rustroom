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
    /// The open photo's result — re-inserted into a replaced engine's store.
    held: Option<Held>,
    /// The view's centre the job was last aimed at: the frame's rectangle and the centre on screen, and
    /// the decode point they map to — re-mapped only when they move (it builds a lens corrector).
    focus: Option<(egui::Rect, egui::Pos2, [f32; 2])>,
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

/// The open photo's result: the finished one, or the job's latest partial one (M10.16b4) — each its own id,
/// so a new partial re-renders.
pub(crate) struct Held {
    key: String,
    image: Arc<Denoised>,
    id: u64,
    complete: bool,
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
            // ⚠ A partial result is not a finished one: a job dropped mid-way runs again (Review Focus 1).
            has_result: self
                .held
                .as_ref()
                .is_some_and(|h| h.key == key && h.complete),
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
        if self.denoise.held.as_ref().is_some_and(|h| h.key != key) {
            self.denoise.held = None;
        }
        if !s.edits.ai_denoise.is_active() {
            // ⚠ "Not now" holds while Amount stays up; setting it again asks again.
            self.denoise.offered = false;
            return;
        }
        let Some((cache, models)) = self.denoise.dirs() else {
            return;
        };
        let full_decode = s.buffer_is_full_res && !s.buffer_is_ghost;
        // ⚠⚠ Into the store only over the decode it was computed on — never the half-size one or a ghost,
        // whose pixels it would be blended with out of place.
        if let Some(Held { image: d, id, .. }) = &self.denoise.held {
            let id = *id;
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
            // ⚠ Read when the worker starts: a change in Settings reaches the next session's job.
            let gpu = self.settings.gpu_tail;
            let worker = self.denoise.worker.get_or_insert_with(|| {
                let repaint = ctx.clone();
                DenoiseWorker::spawn(
                    Box::new(move || denoised::network(&models, gpu)),
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
        self.aim_denoise();
        if self.denoise.pending.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(250));
        }
    }

    /// While the open photo's job runs, aim it at the view's centre (M10.16b4) — re-mapped to decode space
    /// only when the frame or the centre moved.
    fn aim_denoise(&mut self) {
        let (Some(s), Some(w)) = (&self.session, &self.denoise.worker) else {
            return;
        };
        let running = self.denoise.key.as_ref().map(|(_, _, k)| k) == self.denoise.pending.as_ref();
        let (Some(frame), Some(centre)) = (s.canvas_frame_rect, s.canvas_view_centre) else {
            return;
        };
        if !running
            || self
                .denoise
                .focus
                .is_some_and(|(f, c, _)| f == frame && c == centre)
        {
            return;
        }
        let p = crate::develop::canvas::view_focus(s, frame, centre);
        let p = [p[0].clamp(0.0, 1.0), p[1].clamp(0.0, 1.0)];
        w.set_focus(p);
        self.denoise.focus = Some((frame, centre, p));
    }

    fn pump_denoise_results(&mut self) {
        let results = match &self.denoise.worker {
            Some(w) => w.poll(),
            None => return,
        };
        self.absorb_denoise(results);
    }

    /// What the job sent: a finished result or the latest partial one for the open photo held, a failure
    /// remembered, a job's end cleared — testable without a worker.
    pub(crate) fn absorb_denoise(&mut self, results: Vec<DenoiseResult>) {
        let open = self.denoise.key.as_ref().map(|(_, _, k)| k.clone());
        for r in results {
            let key = match &r {
                DenoiseResult::Done { key, .. }
                | DenoiseResult::Partial { key, .. }
                | DenoiseResult::Failed { key, .. }
                | DenoiseResult::Missing { key } => key.clone(),
            };
            // ⚠ A partial result is not the job's end: the job still runs.
            let ended = !matches!(r, DenoiseResult::Partial { .. });
            if ended && self.denoise.pending.as_ref() == Some(&key) {
                self.denoise.pending = None;
            }
            match r {
                // ⚠ Only the open photo's: a job finished for the photo before is in the cache already.
                DenoiseResult::Done { image, .. } if open.as_ref() == Some(&key) => {
                    self.denoise.held = Some(Held {
                        id: denoised::id_of(&key),
                        key,
                        image: Arc::new(Denoised::new(image)),
                        complete: true,
                    });
                    self.status = "Denoised".to_string();
                }
                DenoiseResult::Partial { image, done, .. } if open.as_ref() == Some(&key) => {
                    self.denoise.held = Some(Held {
                        id: denoised::id_of(&format!("{key}#{done}")),
                        key,
                        image: Arc::new(Denoised::new(image)),
                        complete: false,
                    });
                }
                DenoiseResult::Done { .. }
                | DenoiseResult::Partial { .. }
                | DenoiseResult::Missing { .. } => {}
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

    /// "Not now" is not for good: once Amount goes back to 0 and up again, the download is offered again.
    #[test]
    fn a_dismissed_offer_returns_when_amount_is_set_again() {
        let (mut app, ctx, _) = app_with_denoise("reoffer", 50.0);
        app.pump_denoise(&ctx);
        assert!(app.modal == crate::modals::Modal::AiDownload, "not offered");
        app.modal = crate::modals::Modal::None;
        app.session.as_mut().unwrap().edits.ai_denoise.amount = 0.0;
        app.pump_denoise(&ctx);
        app.session.as_mut().unwrap().edits.ai_denoise.amount = 40.0;
        app.pump_denoise(&ctx);
        assert!(
            app.modal == crate::modals::Modal::AiDownload,
            "not offered again"
        );
    }

    /// Review Focus 5: a corrupt result on disk is removed, and the photo goes the normal way — here, to
    /// the model's offer — rather than failing for the run.
    #[test]
    fn a_corrupt_cached_result_goes_the_normal_way() {
        let (mut app, ctx, cache) = app_with_denoise("corrupt", 100.0);
        let photo = app.session.as_ref().unwrap().path.clone();
        let key = rustroom_app::denoise_cache::key(
            &photo,
            denoised::tier_for(&photo, app.settings.view_demosaic),
        );
        std::fs::create_dir_all(&cache).unwrap();
        std::fs::write(cache.join(format!("{key}.rrdn")), b"RRDN garbage").unwrap();
        for _ in 0..200 {
            app.pump_denoise(&ctx);
            if app.modal == crate::modals::Modal::AiDownload {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(app.modal == crate::modals::Modal::AiDownload, "stuck");
        assert!(!app.denoise.failed.contains(&key), "failed for the run");
    }

    /// The open photo's memoised key, after one pump.
    fn open_key(app: &mut RustroomApp, ctx: &egui::Context) -> String {
        app.pump_denoise(ctx);
        app.denoise.key.as_ref().expect("a key").2.clone()
    }

    fn shown_id(app: &RustroomApp) -> Option<u64> {
        let s = app.session.as_ref().unwrap();
        let maps = s.engine.ai_maps();
        let id = maps.read().unwrap().denoised().map(|(_, id)| *id);
        id
    }

    /// M10.16b4: a partial result is shown over the full decode, then the finished one replaces it — each
    /// with its own id.
    #[test]
    fn a_partial_result_is_shown_then_replaced() {
        let (mut app, ctx, _) = app_with_denoise("partial", 100.0);
        let (w, h) = {
            let s = app.session.as_ref().unwrap();
            (s.buffer.width(), s.buffer.height())
        };
        let key = open_key(&mut app, &ctx);
        let img = |v: f32| Arc::new(core_image::ImageBuf::from_vec(w, h, vec![v; w * h * 3]));
        app.absorb_denoise(vec![DenoiseResult::Partial {
            key: key.clone(),
            image: img(0.2),
            done: 1,
        }]);
        app.pump_denoise(&ctx);
        let first = shown_id(&app).expect("the partial is shown");
        app.absorb_denoise(vec![DenoiseResult::Done {
            key: key.clone(),
            image: img(0.3),
        }]);
        app.pump_denoise(&ctx);
        let second = shown_id(&app).expect("the finished one is shown");
        assert_ne!(first, second);
        assert_eq!(second, denoised::id_of(&key));
    }

    /// Review Focus 1: a job dropped after a partial — the partial does not count as finished, so the
    /// photo's job is asked for again.
    #[test]
    fn a_partial_result_is_not_a_finished_one() {
        let (mut app, ctx, _) = app_with_denoise("cancelled", 100.0);
        let (w, h) = {
            let s = app.session.as_ref().unwrap();
            (s.buffer.width(), s.buffer.height())
        };
        // ⚠ The key first, with no model — so no real job starts on the stand-in file and fails.
        let key = open_key(&mut app, &ctx);
        app.modal = crate::modals::Modal::None;
        let models = app.denoise.dirs.as_ref().unwrap().1.clone();
        let spec = ai_mask::ModelId::DrunetColor.spec();
        std::fs::create_dir_all(&models).unwrap();
        std::fs::File::create(models.join(spec.file))
            .unwrap()
            .set_len(spec.bytes)
            .unwrap();
        let img = Arc::new(core_image::ImageBuf::from_vec(w, h, vec![0.2; w * h * 3]));
        app.absorb_denoise(vec![DenoiseResult::Partial {
            key: key.clone(),
            image: img,
            done: 1,
        }]);
        app.denoise.pending = None; // the job was dropped for another photo
        app.pump_denoise(&ctx);
        assert_eq!(
            app.denoise.pending.as_deref(),
            Some(key.as_str()),
            "not asked again"
        );
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
