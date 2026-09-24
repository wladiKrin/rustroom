//! Driving [`ExportWorker`]: building the job list, starting a run, cancelling it, and
//! pumping its events each frame.
//!
//! Split out of `main.rs` unchanged.

use crate::decode::decode_for_export;
use crate::RustroomApp;
use eframe::egui;
use rustroom_app::batch_export::{ExportEvent, ExportJob, ExportReport, ExportWorker};
use rustroom_app::photo_id::PhotoId;
use rustroom_app::sidecar;
use std::time::Duration;

impl RustroomApp {
    /// Sources and edits for a batch export: every selected photo, using the open photo's
    /// **in-memory** edits so unsaved changes are exported, and each other photo's sidecar.
    pub(crate) fn export_jobs(&self) -> Vec<ExportJob> {
        let open = self
            .session
            .as_ref()
            .map(|s| (PhotoId::new(s.path.clone(), s.copy), s.edits.clone()));
        let Some(lib) = &self.library else {
            // ⚠ Single-file mode has no Library, hence no entry to read a name from — and no way
            // to make a copy either, so the open photo is always the original. ⚠ It has no
            // catalog either, so `{date}` and `{camera}` are absent and collapse away.
            return open
                .into_iter()
                .map(|(id, edits)| ExportJob {
                    id,
                    name: None,
                    edits,
                    capture: core_image::Capture::default(),
                    description: Default::default(),
                    marks: Default::default(),
                    keywords: Vec::new(),
                })
                .collect();
        };
        lib.selection
            .iter()
            .filter_map(|i| lib.entries.get(i))
            .map(|e| {
                let id = e.id.clone();
                // ⚠ The name comes from the *entry*, so it is whatever the Library is showing —
                // including a rename made this session that has not been re-scanned.
                let name = e.label.clone();
                let edits = match &open {
                    Some((oid, edits)) if *oid == id => edits.clone(),
                    // ⚠ `load_copy`, not `load`: two copies of one file have different edits, and
                    // loading the file would export both of them identically.
                    _ => sidecar::load_copy(&id.path, id.copy).unwrap_or_default(),
                };
                // ⚠ Keyed by **path**, because capture metadata describes the exposure and every
                // copy of a file shares it — the split `Library.captures` already documents. An
                // uncatalogued or EXIF-less file gets the default, which the template collapses.
                let capture = lib.captures.get(&id.path).cloned().unwrap_or_default();
                // ⚠ Per **file**, like `capture`, and read here rather than held on `Library`:
                // nothing reads descriptions per frame, so a per-folder map would be machinery
                // for a consumer that does not exist (spec §1). One query per selected photo.
                let description = self
                    .catalog
                    .as_ref()
                    .and_then(|c| c.description_of(&id.path).ok().flatten())
                    .unwrap_or_default();
                // ⚠ Per **copy** — the rating and label of *this* version.
                let marks = lib.marks.get(&id).copied().unwrap_or_default();
                let keywords = self
                    .catalog
                    .as_ref()
                    .map(|c| rustroom_app::xmp_sync::keyword_paths(c, &id.path))
                    .unwrap_or_default();
                ExportJob {
                    id,
                    name,
                    edits,
                    capture,
                    description,
                    marks,
                    keywords,
                }
            })
            .collect()
    }

    /// Start exporting the selection.
    pub(crate) fn start_export(&mut self) {
        let jobs = self.export_jobs();
        if jobs.is_empty() {
            self.status = "Nothing selected to export".to_string();
            return;
        }
        // ⚠ Persisted here rather than as the sliders move: these are the *last-used*
        // options, so tweaking the dialog and then cancelling must leave the remembered
        // settings alone. That rule is structural — this is the only write to
        // `settings.export`, so Cancel cannot persist anything because no path would. Same
        // shape `Settings::apply_delete_confirm` uses for the "don't ask again" tick, and it
        // keeps the write count to one per export rather than one per slider frame.
        if self.settings.export != self.export_draft {
            self.settings.export = self.export_draft.clone();
            self.save_settings();
        }
        let opts = self.export_draft.clone();
        let demosaic = opts.demosaic;
        // ⚠ Read **before** `opts` is moved into `spawn`.
        let opts_upload_wanted = opts.destination.target().is_some();
        let export_profile = self.camera_profile_path();
        self.export_progress = format!("Exporting 0 of {}…", jobs.len());
        self.export_collision = None;
        self.exporter = Some(ExportWorker::spawn(
            jobs,
            opts,
            // ⚠ Captured from `opts` — the committed options — not from `export_draft`, so a
            // cancelled edit cannot leak into a running export.
            //
            // ⚠ The camera profile is captured too, from the *open* photo's camera. A batch
            // export of one camera's files is the case that matters; a mixed-camera selection
            // would want per-file lookup, which is noted as a limitation rather than guessed at.
            move |p: &std::path::Path| decode_for_export(p, demosaic, export_profile.clone()),
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            // ⚠ From `Settings`, never from `opts`: both are choices shared with the preview, so
            // an export preset must not be able to carry a different one.
            //
            // ⚠⚠ `gpu_tail` is named for the *tail* and now decides three things — the tail, the
            // preview's prefix seam, and this. Reusing it was chosen over a new `ExportOptions`
            // field (user's call, 2026-09-15) precisely so there is one setting rather than two
            // that could disagree; the naming hazard is carried by the Settings hint instead.
            rustroom_app::batch_export::ExportRender {
                resampler: self.settings.resampler,
                gpu_tail: self.settings.gpu_tail,
            },
            // ⚠ Read afresh when the run begins, and already repaired — a hand-edited 0 becomes 1
            // rather than spawning nothing, and a 64 becomes the core count rather than asking for
            // nineteen gigabytes of decode buffers.
            self.settings.export_width(),
            // ⚠⚠ **Derived the same way `open_photo` derives it**, rather than read off the open
            // photo's `ProfileParts`: an export can run over a selection that includes photos
            // never opened, so "where looks live" has to be a session fact here too. ⚠ It is the
            // *directory*, not a loaded table — which look each photo wants is in its own `Edits`,
            // and the worker resolves every distinct name once.
            Some(
                rustroom_app::looks::LookStore::user_default(
                    &dirs::config_dir().unwrap_or_else(|| std::path::PathBuf::from(".")),
                )
                .dir()
                .to_path_buf(),
            ),
            // ⚠⚠ From `Settings`, and `None` unless the *upload* key is set — the browse key is
            // never used here, which is the point of it being a second field. ⚠ Read when the run
            // begins, so a key pasted mid-session applies to the next export without a restart.
            //
            // ⚠ `opts.upload_target` decides whether uploading was *asked for*; this decides
            // whether it is *possible*. Both must hold, and the dialog cannot offer the checkbox
            // when it is not.
            if opts_upload_wanted {
                rustroom_app::immich_upload::config(&self.settings)
            } else {
                None
            },
        ));
    }

    /// Stop the running export, immediately.
    ///
    /// ⚠ "Abandon, don't abort", the same trade `DecodeWorker` makes for superseded previews.
    /// libraw's `unpack` cannot be interrupted, so the decode in flight runs to completion on
    /// its thread — but the flag makes the worker discard that result instead of writing it,
    /// and dropping the `ExportWorker` here means the UI does not wait for any of that. From
    /// the user's side the cancel is instant and the photo in flight is *not* exported.
    ///
    /// Dropping the worker also drops the decision channel, so a worker blocked on a
    /// collision prompt sees `Err` and exits rather than hanging.
    pub(crate) fn cancel_export(&mut self) {
        if let Some(w) = &self.exporter {
            w.cancel();
        }
        self.exporter = None;
        self.export_progress.clear();
        self.export_collision = None;
        self.status = "Export cancelled".to_string();
    }

    /// Drain the exporter's events once per frame.
    pub(crate) fn pump_export(&mut self, ctx: &egui::Context) {
        let Some(worker) = &self.exporter else { return };
        let mut finished: Option<ExportReport> = None;
        while let Ok(event) = worker.events().try_recv() {
            match event {
                ExportEvent::Started { total } => {
                    self.export_progress = format!("Exporting 0 of {total}…");
                }
                ExportEvent::Item { done, total, name } => {
                    self.export_progress = format!("Exporting {done} of {total} — {name}");
                }
                ExportEvent::Collision { dest } => {
                    self.export_collision = Some(dest);
                    self.export_apply_all = false;
                }
                ExportEvent::UploadUnconfirmed { at, why } => {
                    // ⚠⚠ Raised **during** the run, deliberately: a file whose upload did not
                    // confirm is important enough to say at once rather than at the end. The export
                    // keeps going. ⚠ No dialog can be open here — every collision is settled up
                    // front in `plan`, before anything is rendered or uploaded.
                    self.modal = crate::modals::Modal::UploadUnconfirmed { at, why };
                }
                ExportEvent::Finished(report) => finished = Some(report),
            }
        }
        if let Some(r) = finished {
            self.exporter = None;
            self.export_progress.clear();
            self.export_collision = None;
            let mut parts = vec![format!("Exported {}", r.exported)];
            if r.skipped > 0 {
                parts.push(format!("{} skipped", r.skipped));
            }
            if !r.failed.is_empty() {
                parts.push(format!("{} failed", r.failed.len()));
            }
            if r.cancelled {
                parts.push("cancelled".to_string());
            }
            self.status = parts.join(", ");
        }
        // The worker runs off the UI thread, so nothing else would wake the app up.
        if self.exporter.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
}

#[cfg(test)]
mod tests {
    /// ⚠⚠ The half of the filename template no unit test in `batch_export` can see: the tokens
    /// being right says nothing about anything **reaching** for the catalog's answer. Stubbing
    /// this to `Capture::default()` leaves every `filename_template` test green while `{date}`
    /// and `{camera}` silently collapse away on every photo — the feature absent, not wrong.
    #[test]
    fn a_jobs_capture_comes_from_the_library() {
        let mut app = crate::test_app::for_test();
        let mut lib = crate::test_app::library_of(2);
        let path = lib.entries[1].id.path.clone();
        lib.captures.insert(
            path.clone(),
            core_image::Capture {
                taken: Some(1_787_824_800),
                camera: Some("FUJIFILM X-T4".to_string()),
                ..Default::default()
            },
        );
        lib.selection.select_indices(&[0, 1]);
        app.library = Some(lib);

        let jobs = app.export_jobs();

        assert_eq!(jobs.len(), 2);
        assert_eq!(
            jobs[1].capture.camera.as_deref(),
            Some("FUJIFILM X-T4"),
            "the catalog's camera must reach the worker"
        );
        assert_eq!(jobs[1].capture.taken, Some(1_787_824_800));
        // ⚠ The other half: a photo the catalog knows nothing about is the *ordinary* case, and
        // must be an empty capture rather than the previous photo's.
        assert_eq!(jobs[0].capture, core_image::Capture::default());
    }
}
