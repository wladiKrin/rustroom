//! M9.14: previews in the app — shaping requests from the settings, the texture window, the
//! pumps, the leave hook and the two kinds of build.
//!
//! ⚠ Logic that needs no egui lives in the lib: `preview_loader` (what a request resolves to),
//! `preview_build` (the queue and the job shape), `preview_key` (untouched, the file name) and
//! `stand_in` (when Develop shows a preview). This module owns the textures and the workers.

use crate::decode::decode_for_export;
use crate::RustroomApp;
use edit_model::Edits;
use eframe::egui;
use rustroom_app::batch_export::{Collision, Decision, ExportEvent, ExportJob, ExportWorker};
use rustroom_app::photo_id::PhotoId;
use rustroom_app::preview_build::{build_job, build_options};
use rustroom_app::preview_key::{is_untouched, PreviewKey};
use rustroom_app::preview_loader::PreviewRequest;
use rustroom_app::thumb_key::ThumbKey;

/// Where a build came from: the leave queue runs one photo at a time and says nothing; an
/// on-demand build reports its progress and its result.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum BuildKind {
    Leave,
    OnDemand,
}

impl RustroomApp {
    /// A request shaped by the settings: the build tier and whether the photo is untouched.
    pub(crate) fn preview_request(&self, id: PhotoId, edits: Edits) -> PreviewRequest {
        let fresh = self.settings.initial_edits(None);
        PreviewRequest {
            untouched: is_untouched(&edits, &fresh),
            tier: self.settings.preview_tier(),
            id,
            edits,
        }
    }

    /// The previews on screen now: pin them, ask the loader for the missing ones — once per
    /// change, `Library::sync_thumb_window`'s rule.
    pub(crate) fn sync_preview_window(&mut self, window: &[PreviewRequest]) {
        let keys: Vec<ThumbKey> = window.iter().map(|r| r.key()).collect();
        self.preview_textures.retain_window(&keys);
        let missing: Vec<PreviewRequest> = window
            .iter()
            .filter(|r| !self.preview_textures.contains(&r.key()))
            .cloned()
            .collect();
        if missing != self.preview_want {
            self.previews.want(&missing);
            self.preview_want = missing;
        }
    }

    /// Per frame: finished previews become textures. A miss leaves the thumbnail in place.
    pub(crate) fn pump_previews(&mut self, ctx: &egui::Context) {
        for res in self.previews.drain() {
            let Some((t, source)) = res.preview else {
                continue;
            };
            if t.width == 0 || t.height == 0 {
                continue;
            }
            let bytes = t.rgba.len();
            let img = egui::ColorImage::from_rgba_unmultiplied([t.width, t.height], &t.rgba);
            let tex = ctx.load_texture(
                format!(
                    "preview:{}#{}:{}",
                    res.key.id.path.display(),
                    res.key.id.copy,
                    res.key.edits
                ),
                img,
                egui::TextureOptions::LINEAR,
            );
            self.preview_textures.insert(res.key, (tex, source), bytes);
            if std::env::var_os("RUSTROOM_DEBUG_RENDER").is_some() {
                eprintln!(
                    "open  {:8.1} ms  preview landed {}x{} {source:?}",
                    crate::debug_ms(),
                    t.width,
                    t.height
                );
            }
            ctx.request_repaint();
        }
    }

    /// Develop's window: the pending photo's preview, or the open one's until the full decode
    /// lands — after that the live buffer has every pixel and a stand-in would only cost memory.
    pub(crate) fn sync_develop_previews(&mut self) {
        let window: Vec<PreviewRequest> = if let Some(p) = &self.pending {
            vec![p.preview_request.clone()]
        } else if let Some(s) = &self.session {
            if s.buffer_is_full_res {
                Vec::new()
            } else {
                vec![self.preview_request(PhotoId::new(s.path.clone(), s.copy), s.edits.clone())]
            }
        } else {
            Vec::new()
        };
        self.sync_preview_window(&window);
    }

    /// Leaving an edited photo: queue its preview unless the store already has it.
    pub(crate) fn queue_leave_build(&mut self, id: PhotoId, edits: Edits) {
        let req = self.preview_request(id.clone(), edits.clone());
        if req.untouched || self.preview_store.has(&req.store_key()) {
            return;
        }
        self.leave_queue.push(id, edits);
    }

    fn spawn_build(&mut self, jobs: Vec<ExportJob>, width: usize, kind: BuildKind) {
        let tier = self.settings.preview_tier();
        let profile = self.camera_profile_path();
        // ⚠ The worker writes into an existing directory only.
        self.preview_store.ensure_dir();
        let worker = ExportWorker::spawn(
            jobs,
            build_options(self.preview_store.dir().to_path_buf(), tier),
            move |p: &std::path::Path| decode_for_export(p, tier, profile.clone()),
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            rustroom_app::batch_export::ExportRender {
                ai_subject: self.settings.ai_subject_model,
                resampler: self.settings.resampler,
                gpu_tail: self.settings.gpu_tail,
            },
            width,
            Some(
                rustroom_app::looks::LookStore::user_default(
                    &dirs::config_dir().unwrap_or_else(|| std::path::PathBuf::from(".")),
                )
                .dir()
                .to_path_buf(),
            ),
            None,
        );
        self.preview_builder = Some((worker, kind));
    }

    /// Photo → Build previews: the selection, or the open photo, skipping what the store has.
    pub(crate) fn start_preview_build(&mut self) {
        let tier = self.settings.preview_tier();
        let jobs: Vec<ExportJob> = self
            .export_jobs()
            .into_iter()
            .filter(|j| {
                !self
                    .preview_store
                    .has(&PreviewKey::new(j.id.clone(), &j.edits, tier))
            })
            .map(|j| build_job(j.id, j.edits, tier))
            .collect();
        if jobs.is_empty() {
            self.status =
                "Nothing to build — select photos, or every selected photo already has a preview"
                    .to_string();
            return;
        }
        // ⚠ On demand goes first: a leave build in flight is cancelled and its photo re-queued
        // by the next leave, while the user's explicit request runs at full width.
        if let Some((w, _)) = &self.preview_builder {
            w.cancel();
        }
        self.preview_progress = format!("Building previews 0 of {}…", jobs.len());
        let width = self.settings.export_width();
        self.spawn_build(jobs, width, BuildKind::OnDemand);
    }

    pub(crate) fn cancel_preview_build(&mut self) {
        if let Some((w, _)) = &self.preview_builder {
            w.cancel();
        }
        self.preview_builder = None;
        self.preview_progress.clear();
        self.status = "Preview build cancelled".to_string();
    }

    /// Per frame: start the next leave build when nothing runs, and absorb the events of the
    /// one in flight.
    pub(crate) fn pump_preview_builds(&mut self, ctx: &egui::Context) {
        if self.preview_builder.is_none() {
            if let Some((id, edits)) = self.leave_queue.pop() {
                let tier = self.settings.preview_tier();
                self.spawn_build(vec![build_job(id, edits, tier)], 1, BuildKind::Leave);
            }
        }
        let Some((worker, kind)) = &self.preview_builder else {
            return;
        };
        let kind = *kind;
        let mut finished = None;
        while let Ok(ev) = worker.events().try_recv() {
            match ev {
                ExportEvent::Item { done, total, .. } if kind == BuildKind::OnDemand => {
                    self.preview_progress = format!("Building previews {done} of {total}…");
                }
                // ⚠ A key that already has a file is skipped before the job is made, so a
                // collision is a race with another instance writing the same JPEG: overwrite.
                ExportEvent::Collision { .. } => worker.decide(Decision::Resolve {
                    collision: Collision::Overwrite,
                    apply_to_all: true,
                }),
                ExportEvent::Finished(r) => finished = Some(r),
                _ => {}
            }
        }
        if let Some(r) = finished {
            self.preview_builder = None;
            self.preview_progress.clear();
            // ⚠ An estimate per file feeds the store's amortised sweep; the sweep itself measures.
            self.preview_store
                .note_written(r.exported as u64 * 8 * 1024 * 1024);
            // ⚠ Dropped rather than patched: the next want reloads each as Rendered.
            self.preview_textures.clear();
            self.preview_want.clear();
            self.preview_cache_bytes = None;
            if kind == BuildKind::OnDemand {
                let mut parts = vec![format!("Built {} previews", r.exported)];
                if !r.failed.is_empty() {
                    parts.push(format!("{} failed", r.failed.len()));
                }
                if r.cancelled {
                    parts.push("cancelled".to_string());
                }
                self.status = parts.join(", ");
            }
        }
        if self.preview_builder.is_some() || !self.leave_queue.is_empty() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaving_an_edited_photo_queues_its_preview_and_an_untouched_one_does_not() {
        let ctx = crate::test_ui::ctx();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 8, 4);
        let fresh = app.settings.initial_edits(None);
        let id = PhotoId::new(std::path::PathBuf::from("/test/a.RAF"), 0);
        app.queue_leave_build(id.clone(), fresh.clone());
        assert!(
            app.leave_queue.is_empty(),
            "an untouched photo queued a build"
        );
        let mut edited = fresh;
        edited.exposure = 0.5;
        app.queue_leave_build(id, edited);
        assert_eq!(app.leave_queue.len(), 1);
    }

    #[test]
    fn develop_asks_for_the_open_photos_preview_until_the_full_decode_lands() {
        let ctx = crate::test_ui::ctx();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 8, 4);
        app.session.as_mut().unwrap().buffer_is_full_res = false;
        app.sync_develop_previews();
        assert_eq!(app.preview_want.len(), 1, "no request for the open photo");
        app.session.as_mut().unwrap().buffer_is_full_res = true;
        app.sync_develop_previews();
        assert!(
            app.preview_want.is_empty(),
            "a full-resolution buffer needs no stand-in"
        );
    }

    #[test]
    fn a_request_is_untouched_exactly_when_its_edits_are_a_fresh_opens() {
        let app = crate::test_app::for_test();
        let id = PhotoId::new(std::path::PathBuf::from("/test/a.RAF"), 0);
        let fresh = app.settings.initial_edits(None);
        assert!(app.preview_request(id.clone(), fresh.clone()).untouched);
        assert!(
            app.preview_request(id.clone(), Edits::default()).untouched,
            "a never-opened photo's catalog edits are the bare defaults, and it is untouched"
        );
        let mut edited = fresh;
        edited.contrast = 5.0;
        assert!(!app.preview_request(id, edited).untouched);
    }
}
