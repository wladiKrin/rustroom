//! Publish services in the app (M10.18d): opening a service's view, its members' states, and a Publish —
//! the export worker, then the folder's or the album's bookkeeping (`rustroom_app::publish_run`).
//!
//! ⚠ States derive once per change — the service, the catalog's count, the view's revision, the open photo's
//! edits — never per frame: a service can hold thousands of photos (CLAUDE.md, the rail's rule).

use crate::library_state::LibrarySource;
use crate::RustroomApp;
use catalog::db::{Member, PublishServiceRow, PublishTarget, Published};
use eframe::egui;
use rustroom_app::batch_export::{
    file_metadata, Decision, Destination, ExportEvent, ExportOptions, ExportReport, ExportWorker,
    UploadTarget,
};
use rustroom_app::photo_id::PhotoId;
use rustroom_app::publish_run::{self as run, AlbumSync, Key, Outcome, PublishPlan};
use rustroom_app::publish_state::{self as st, Counts, State};
use std::collections::HashMap;
use std::sync::mpsc::Receiver;
use std::time::Duration;

/// The publish half of the app's state.
#[derive(Default)]
pub(crate) struct PublishFlow {
    run: Option<Run>,
    /// The open service view's states.
    pub(crate) view: Option<ViewStates>,
    /// The New / Edit service dialog, while open.
    pub(crate) dialog: Option<Dialog>,
}

/// Which export settings a service takes, in its dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SettingsFrom {
    /// The service's own, unchanged — offered when editing.
    Keep,
    /// The export dialog's last-used settings.
    LastExport,
    /// A saved export preset, by name — copied, never linked.
    Preset(String),
}

/// The New / Edit service dialog's draft.
#[derive(Debug, Clone)]
pub(crate) struct Dialog {
    pub(crate) editing: Option<i64>,
    pub(crate) name: String,
    pub(crate) to_immich: bool,
    pub(crate) folder: Option<std::path::PathBuf>,
    /// An existing album `(id, name)`, or `None` for a new one named `new_album`.
    pub(crate) album: Option<(String, String)>,
    pub(crate) new_album: String,
    pub(crate) settings: SettingsFrom,
    pub(crate) error: Option<String>,
}

/// A running Publish.
struct Run {
    service: i64,
    target: PublishTarget,
    options_json: String,
    name: String,
    plan: PublishPlan,
    worker: Option<ExportWorker>,
    progress: String,
    /// The album step, on its own thread (it is network).
    album: Option<Receiver<Result<AlbumSync, String>>>,
    rows: Vec<(Key, Published)>,
    outcome: Outcome,
}

/// One service view's states and counts.
pub(crate) struct ViewStates {
    key: (i64, u64, u64, u64, u64),
    pub(crate) states: HashMap<PhotoId, State>,
    pub(crate) counts: Counts,
}

/// The service's export settings, pointed at its destination.
pub(crate) fn options_for(row: &PublishServiceRow) -> ExportOptions {
    let mut o: ExportOptions = serde_json::from_str(&row.options_json).unwrap_or_default();
    // ⚠ A service's files are named and placed by the service: no subfolder, nothing done afterwards.
    o.delivery = Default::default();
    match &row.target {
        PublishTarget::Folder { path } => {
            o.dest_dir = path.clone();
            o.destination = Destination::Files;
        }
        PublishTarget::Immich { .. } => {
            // ⚠ Staged: an unconfirmed upload is rescued into `dest_dir`, which must therefore be somewhere
            // the user can find it — the export settings' own folder.
            o.destination = Destination::Target(UploadTarget::Immich);
        }
    }
    o
}

fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

impl RustroomApp {
    /// The service `id`, from the catalog.
    pub(crate) fn publish_service(&self, id: i64) -> Option<PublishServiceRow> {
        self.catalog
            .as_ref()?
            .publish_services()
            .ok()?
            .into_iter()
            .find(|s| s.id == id)
    }

    /// The service the Library is showing, if it shows one.
    pub(crate) fn shown_service(&self) -> Option<i64> {
        match self.library.as_ref().map(|l| &l.source) {
            Some(LibrarySource::Publish { id, .. }) => Some(*id),
            _ => None,
        }
    }

    /// Show a service's members — the shape of `open_collection`.
    pub(crate) fn open_publish(&mut self, ctx: &egui::Context, id: i64, name: &str) {
        let source = LibrarySource::Publish {
            id,
            name: name.to_string(),
        };
        let members = match self.source_members(&source) {
            Some(Ok(m)) => m,
            Some(Err(e)) => {
                self.status = format!("Could not read the service: {e}");
                return;
            }
            None => {
                self.status = "No catalog".into();
                return;
            }
        };
        let mut scan = rustroom_app::collections::scan_of(
            &members,
            rustroom_app::sidecar::edited_at,
            crate::library_state::fs_modified,
        );
        catalog::sort(&mut scan, self.settings.library_sort);
        let copies = crate::source_refresh::copies_of(&source, &members);
        self.open_scan_with(ctx, source, scan, copies);
        self.publish.view = None;
    }

    /// What each non-removed member would be published as now, under `opts`.
    fn publish_prints(&self, opts: &ExportOptions, members: &[Member]) -> HashMap<Key, u64> {
        let ids: Vec<PhotoId> = members
            .iter()
            .filter(|m| !m.removed)
            .map(|m| PhotoId::new(m.path.clone(), m.copy))
            .collect();
        let embeds = opts.metadata.scope != export::MetadataScope::CopyrightOnly;
        self.export_jobs_for(&ids)
            .iter()
            .map(|job| {
                let meta = embeds
                    .then(|| export::FileMetadata::resolve(&opts.metadata, file_metadata(job)));
                (
                    (job.id.path.clone(), job.id.copy),
                    st::fingerprint(&job.edits, opts, meta.as_ref()),
                )
            })
            .collect()
    }

    /// The open service view's states — re-derived only when something they read has moved.
    pub(crate) fn publish_view_states(&mut self) -> Option<&ViewStates> {
        // ⚠ Dropped once the view shows something else, or its controls would linger in the status row.
        let Some(id) = self.shown_service() else {
            self.publish.view = None;
            return None;
        };
        let cat = self.catalog.as_ref()?;
        let open = self
            .session
            .as_ref()
            .map_or(0, |s| rustroom_app::thumb_key::edits_fingerprint(&s.edits));
        let rev = self.library.as_ref().map_or(0, |l| l.visible_rev);
        let key = (id, cat.instance(), cat.change_count(), rev, open);
        if self.publish.view.as_ref().map(|v| v.key) != Some(key) {
            let row = self.publish_service(id)?;
            let members = self.catalog.as_ref()?.publish_members(id).ok()?;
            let prints = self.publish_prints(&options_for(&row), &members);
            let mut counts = Counts::default();
            let states = members
                .iter()
                .map(|m| {
                    let s = match prints.get(&(m.path.clone(), m.copy)) {
                        Some(f) => st::state(m, *f),
                        None if m.removed => State::Removed,
                        None => State::New,
                    };
                    counts.add(s);
                    (PhotoId::new(m.path.clone(), m.copy), s)
                })
                .collect();
            self.publish.view = Some(ViewStates {
                key,
                states,
                counts,
            });
        }
        self.publish.view.as_ref()
    }

    /// Publish the service the Library shows.
    pub(crate) fn start_publish(&mut self) {
        if self.publish.run.is_some() {
            self.status = "A publish is already running".into();
            return;
        }
        let Some(row) = self.shown_service().and_then(|id| self.publish_service(id)) else {
            return;
        };
        let opts = options_for(&row);
        let upload = match &row.target {
            PublishTarget::Folder { .. } => None,
            PublishTarget::Immich { .. } => {
                match rustroom_app::immich_upload::config(&self.settings) {
                    Some(c) => Some(c),
                    None => {
                        self.status =
                            "Publishing to Immich needs the Write key — Settings → Immich".into();
                        return;
                    }
                }
            }
        };
        let Some(members) = self
            .catalog
            .as_ref()
            .and_then(|c| c.publish_members(row.id).ok())
        else {
            return;
        };
        let prints = self.publish_prints(&opts, &members);
        let plan = run::plan(&members, &prints);
        if plan.export.is_empty() && plan.clear.is_empty() {
            self.status = format!("“{}” is up to date", row.name);
            return;
        }
        let ids: Vec<PhotoId> = plan
            .export
            .iter()
            .map(|(p, c)| PhotoId::new(p.clone(), *c))
            .collect();
        let jobs = self.export_jobs_for(&ids);
        let worker = (!jobs.is_empty()).then(|| {
            let demosaic = opts.demosaic;
            let profile = self.camera_profile_path();
            let config = dirs::config_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
            ExportWorker::spawn(
                jobs,
                opts,
                move |p: &std::path::Path| {
                    crate::decode::decode_for_export(p, demosaic, profile.clone())
                },
                std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                rustroom_app::batch_export::ExportRender {
                    ai_subject: self.settings.ai_subject_model,
                    resampler: self.settings.resampler,
                    gpu_tail: self.settings.gpu_tail,
                    denoise: rustroom_app::denoised::DenoiseUse::Compute,
                    network: self.settings.denoise_network,
                },
                self.settings.export_width(),
                rustroom_app::batch_export::ExportDirs {
                    looks: Some(
                        rustroom_app::looks::LookStore::user_default(&config)
                            .dir()
                            .to_path_buf(),
                    ),
                    watermarks: Some(
                        rustroom_app::watermark_store::WatermarkStore::user_default(&config)
                            .dir()
                            .to_path_buf(),
                    ),
                },
                upload,
            )
        });
        let total = plan.export.len();
        self.publish.run = Some(Run {
            service: row.id,
            target: row.target.clone(),
            options_json: row.options_json.clone(),
            name: row.name.clone(),
            plan,
            worker,
            progress: format!("Publishing 0 of {total} to “{}”…", row.name),
            album: None,
            rows: Vec::new(),
            outcome: Outcome::default(),
        });
    }

    /// Open the New service dialog.
    pub(crate) fn open_new_service(&mut self) {
        self.publish.dialog = Some(Dialog {
            editing: None,
            name: String::new(),
            to_immich: false,
            folder: None,
            album: None,
            new_album: String::new(),
            settings: SettingsFrom::LastExport,
            error: None,
        });
        self.modal = crate::modals::Modal::PublishService;
    }

    /// Open the Edit dialog for service `id`.
    pub(crate) fn open_edit_service(&mut self, id: i64) {
        let Some(row) = self.publish_service(id) else {
            return;
        };
        let (to_immich, folder, album, new_album) = match &row.target {
            PublishTarget::Folder { path } => (false, Some(path.clone()), None, String::new()),
            PublishTarget::Immich {
                album_id: Some(a),
                album_name,
            } => (
                true,
                None,
                Some((a.clone(), album_name.clone())),
                String::new(),
            ),
            PublishTarget::Immich {
                album_id: None,
                album_name,
            } => (true, None, None, album_name.clone()),
        };
        self.publish.dialog = Some(Dialog {
            editing: Some(id),
            name: row.name,
            to_immich,
            folder,
            album,
            new_album,
            settings: SettingsFrom::Keep,
            error: None,
        });
        self.modal = crate::modals::Modal::PublishService;
    }

    /// Save the dialog's service; `false` (with the dialog's error set) when it cannot be saved.
    pub(crate) fn save_service_dialog(&mut self) -> bool {
        let Some(d) = self.publish.dialog.clone() else {
            return false;
        };
        let fail = |app: &mut Self, e: &str| {
            if let Some(d) = app.publish.dialog.as_mut() {
                d.error = Some(e.to_string());
            }
            false
        };
        let name = d.name.trim().to_string();
        if name.is_empty() {
            return fail(self, "Name the service");
        }
        let target = if d.to_immich {
            match &d.album {
                Some((id, album_name)) => PublishTarget::Immich {
                    album_id: Some(id.clone()),
                    album_name: album_name.clone(),
                },
                None if !d.new_album.trim().is_empty() => PublishTarget::Immich {
                    album_id: None,
                    album_name: d.new_album.trim().to_string(),
                },
                None => return fail(self, "Choose an album or name a new one"),
            }
        } else {
            match &d.folder {
                Some(path) => PublishTarget::Folder { path: path.clone() },
                None => return fail(self, "Choose a folder"),
            }
        };
        let kept = d
            .editing
            .and_then(|id| self.publish_service(id))
            .map(|r| r.options_json);
        let options_json = match (&d.settings, kept) {
            (SettingsFrom::Keep, Some(json)) => json,
            (SettingsFrom::Preset(p), _) => match self
                .export_preset_store
                .list()
                .into_iter()
                .find(|x| x.name == *p)
            {
                Some(x) => serde_json::to_string(&x.options).unwrap_or_default(),
                None => return fail(self, "That export preset is gone"),
            },
            _ => serde_json::to_string(&self.settings.export).unwrap_or_default(),
        };
        let Some(cat) = &self.catalog else {
            return fail(self, "No catalog");
        };
        let saved = match d.editing {
            Some(id) => cat.update_publish_service(id, &name, &target, &options_json),
            None => cat
                .create_publish_service(&name, &target, &options_json)
                .map(|_| ()),
        };
        if saved.is_err() {
            return fail(self, "A service of that name already exists");
        }
        self.publish.dialog = None;
        self.publish.view = None;
        // A renamed service's open view keeps its title in step.
        if let (Some(id), Some(lib)) = (d.editing, self.library.as_mut()) {
            if let LibrarySource::Publish { id: shown, name: n } = &mut lib.source {
                if *shown == id {
                    *n = name.clone();
                }
            }
        }
        true
    }

    /// Delete service `id` — its rows only; nothing it published is touched.
    pub(crate) fn delete_service(&mut self, id: i64) {
        if let Some(cat) = &self.catalog {
            let _ = cat.delete_publish_service(id);
        }
        if self.shown_service() == Some(id) {
            self.publish.view = None;
        }
    }

    /// Add the selection to service `id`, or take it out; the status says how many.
    pub(crate) fn service_selection(&mut self, id: i64, add: bool) {
        let ids: Vec<PhotoId> = self
            .library
            .as_ref()
            .map(|l| {
                l.selection
                    .iter()
                    .filter_map(|i| l.entries.get(i))
                    .map(|e| e.id.clone())
                    .collect()
            })
            .unwrap_or_default();
        let Some(cat) = &self.catalog else {
            return;
        };
        let n = ids
            .iter()
            .filter(|p| {
                if add {
                    cat.add_to_service(id, &p.path, p.copy).unwrap_or(false)
                } else {
                    cat.remove_from_service(id, &p.path, p.copy)
                        .unwrap_or(false)
                }
            })
            .count();
        let photos = if n == 1 { "photo" } else { "photos" };
        self.status = if add {
            format!("Added {n} {photos} to publish")
        } else {
            format!("Took {n} {photos} out of the service")
        };
    }

    /// The status row's line while a publish runs.
    pub(crate) fn publish_line(&self) -> Option<String> {
        self.publish.run.as_ref().map(|r| r.progress.clone())
    }

    /// Stop the running publish: what was already delivered is NOT recorded, so it is published again next
    /// time (a duplicate upload, an overwritten own file) — never lost.
    pub(crate) fn cancel_publish(&mut self) {
        if let Some(w) = self.publish.run.as_ref().and_then(|r| r.worker.as_ref()) {
            w.cancel();
        }
        self.publish.run = None;
        self.publish.view = None;
        self.status = "Publish cancelled".into();
    }

    /// Once a frame: the worker's events, then the album step, then the record.
    pub(crate) fn pump_publish(&mut self, ctx: &egui::Context) {
        let Some(r) = self.publish.run.as_mut() else {
            return;
        };
        ctx.request_repaint_after(Duration::from_millis(100));
        let mut report: Option<ExportReport> = None;
        if let Some(w) = &r.worker {
            while let Ok(ev) = w.events().try_recv() {
                match ev {
                    ExportEvent::Item { done, total, name } => {
                        r.progress =
                            format!("Publishing {done} of {total} to “{}” — {name}", r.name);
                    }
                    // ⚠ Answered here, never asked: the service's own file is overwritten, anything else kept.
                    ExportEvent::Collision { dest } => w.decide(Decision::Resolve {
                        collision: run::collision_answer(&dest, &r.plan.replacing),
                        apply_to_all: false,
                    }),
                    ExportEvent::Finished(rep) => report = Some(rep),
                    _ => {}
                }
            }
        } else if r.album.is_none() && r.rows.is_empty() {
            // Nothing to export — only removals to clear.
            report = Some(ExportReport::default());
        }
        if let Some(rep) = report {
            r.worker = None;
            r.outcome.failed = rep.failed.len() + rep.unconfirmed.len();
            r.outcome.cancelled = rep.cancelled;
            r.outcome.notes = rep.notes.clone();
            r.rows = run::delivered(&r.plan, &rep, now());
            match &r.target {
                PublishTarget::Folder { .. } => self.finish_folder(),
                PublishTarget::Immich {
                    album_id,
                    album_name,
                } => {
                    let (album_id, album_name) = (album_id.clone(), album_name.clone());
                    self.start_album_step(album_id, album_name);
                }
            }
            return;
        }
        let landed = r.album.as_ref().and_then(|rx| rx.try_recv().ok());
        if let Some(result) = landed {
            self.finish_album(result);
        }
    }

    fn finish_folder(&mut self) {
        let Some(mut r) = self.publish.run.take() else {
            return;
        };
        if let Some(cat) = &self.catalog {
            r.outcome.published = run::record(cat, r.service, &r.rows);
            let (cleared, left) = run::clear_folder(cat, r.service, &r.plan.clear);
            r.outcome.cleared = cleared;
            r.outcome.left = left;
        }
        self.status = r.outcome.summary();
        self.publish.view = None;
    }

    /// The album step on its own thread: create the album if it is new, add what was uploaded, take out
    /// what was replaced or removed.
    fn start_album_step(&mut self, album_id: Option<String>, album_name: String) {
        let Some(cfg) = rustroom_app::immich_upload::config(&self.settings) else {
            self.publish.run = None;
            self.status = "Publishing to Immich needs the Write key — Settings → Immich".into();
            return;
        };
        let Some(r) = self.publish.run.as_mut() else {
            return;
        };
        let new_of: HashMap<&Key, &str> = r
            .rows
            .iter()
            .filter_map(|(k, p)| match &p.place {
                catalog::db::Place::Asset { id } => Some((k, id.as_str())),
                _ => None,
            })
            .collect();
        let add: Vec<String> = new_of.values().map(|s| s.to_string()).collect();
        let replaced: HashMap<String, String> = r
            .plan
            .old_assets
            .iter()
            .filter_map(|(k, old)| new_of.get(k).map(|new| (old.clone(), new.to_string())))
            .collect();
        let drop: Vec<String> = r
            .plan
            .clear
            .iter()
            .filter_map(|m| match m.published.as_ref().map(|p| &p.place) {
                Some(catalog::db::Place::Asset { id }) => Some(id.clone()),
                _ => None,
            })
            .collect();
        if add.is_empty() && drop.is_empty() && album_id.is_some() {
            self.finish_album(Ok(AlbumSync {
                album: album_id.unwrap_or_default(),
                ..Default::default()
            }));
            return;
        }
        r.progress = format!("Updating the album “{album_name}”…");
        let (tx, rx) = std::sync::mpsc::channel();
        r.album = Some(rx);
        std::thread::Builder::new()
            .name("publish-album".into())
            .spawn(move || {
                let result = immich::Client::new(&cfg.base_url, &cfg.key)
                    .map_err(|e| format!("Immich: {e}"))
                    .and_then(|c| {
                        run::sync_album(
                            &c,
                            album_id.as_deref(),
                            &album_name,
                            &add,
                            &replaced,
                            &drop,
                        )
                    });
                let _ = tx.send(result);
            })
            .expect("spawn the album step");
    }

    fn finish_album(&mut self, result: Result<AlbumSync, String>) {
        let Some(mut r) = self.publish.run.take() else {
            return;
        };
        match result {
            Ok(sync) => {
                if let (
                    Some(cat),
                    PublishTarget::Immich {
                        album_id: None,
                        album_name,
                    },
                ) = (&self.catalog, &r.target)
                {
                    let target = PublishTarget::Immich {
                        album_id: Some(sync.album.clone()),
                        album_name: album_name.clone(),
                    };
                    let _ =
                        cat.update_publish_service(r.service, &r.name, &target, &r.options_json);
                }
                // ⚠ Recorded only once in the album (`publish_run`'s rule).
                let rows: Vec<(Key, Published)> = r
                    .rows
                    .iter()
                    .filter(|(_, p)| {
                        matches!(&p.place, catalog::db::Place::Asset { id } if sync.added.contains(id))
                    })
                    .cloned()
                    .collect();
                r.outcome.failed += r.rows.len() - rows.len();
                if let Some(cat) = &self.catalog {
                    r.outcome.published = run::record(cat, r.service, &rows);
                    for m in &r.plan.clear {
                        let gone = match m.published.as_ref().map(|p| &p.place) {
                            Some(catalog::db::Place::Asset { id }) => sync.removed.contains(id),
                            _ => true,
                        };
                        if gone && cat.clear_member(r.service, &m.path, m.copy).is_ok() {
                            r.outcome.cleared += 1;
                        }
                    }
                }
            }
            Err(e) => {
                r.outcome.failed += r.rows.len();
                r.outcome.notes.push(e);
            }
        }
        self.status = r.outcome.summary();
        self.publish.view = None;
    }
}

#[cfg(test)]
#[path = "publish_flow_tests.rs"]
mod tests;
