//! Driving [`ExportWorker`]: building the job list, starting a run, cancelling it, and
//! pumping its events each frame.
//!
//! Split out of `main.rs` unchanged.

use crate::*;

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
            // to make a copy either, so the open photo is always the original.
            return open
                .into_iter()
                .map(|(id, edits)| ExportJob {
                    id,
                    name: None,
                    edits,
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
                let name = e.name.clone();
                let edits = match &open {
                    Some((oid, edits)) if *oid == id => edits.clone(),
                    // ⚠ `load_copy`, not `load`: two copies of one file have different edits, and
                    // loading the file would export both of them identically.
                    _ => sidecar::load_copy(&id.path, id.copy).unwrap_or_default(),
                };
                ExportJob { id, name, edits }
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
