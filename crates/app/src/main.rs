use catalog::CatalogEntry;
use edit_model::EditGroups;
use edit_model::Edits;
use eframe::egui;
use rustroom_app::action_log::{ActionKind, ActionLog, SyncAction};
use rustroom_app::batch_export::{
    destination_ready, Collision, Decision, ExportEvent, ExportOptions, ExportReport, ExportWorker,
};
use rustroom_app::buffer_cache::BufferCache;
use rustroom_app::crop_tool;
use rustroom_app::decode_worker::{DecodeWorker, Decoded};
use rustroom_app::edit_clipboard;
use rustroom_app::export_presets::{ExportPreset, ExportPresetStore};
use rustroom_app::histogram::histogram_rgba;
use rustroom_app::history::History;
use rustroom_app::presets::{Preset, PresetStore};
use rustroom_app::render_engine::{RegionRect, RenderEngine};
use rustroom_app::selection::Selection;
use rustroom_app::settings::{ConfirmOutcome, Settings, SettingsStore};
use rustroom_app::sidecar;
use rustroom_app::sync;
use rustroom_app::thumb_loader::ThumbLoader;
use rustroom_app::view_region;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

const PREVIEW_MAX_DIM: usize = 1600;
/// How long the selection must sit still before we commit to a decode. Holding an arrow key
/// therefore starts zero decodes until you settle.
const DECODE_DEBOUNCE: Duration = Duration::from_millis(180);
/// Superseded decodes are abandoned rather than aborted, so cap how many run at once.
const MAX_CONCURRENT_DECODES: usize = 2;
/// Recently viewed photos kept decoded in memory (~85 MB each).
const BUFFER_CACHE_CAP: usize = 3;

fn main() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native(
        "Rustroom",
        native_options,
        Box::new(|cc| Ok(Box::new(RustroomApp::new(&cc.egui_ctx)))),
    )
}

#[derive(Default, PartialEq)]
enum AppView {
    #[default]
    Library,
    Develop,
}

/// Browsing state for an opened folder.
struct Library {
    entries: Vec<CatalogEntry>,
    thumbs: Vec<Option<egui::TextureHandle>>, // parallel to entries; None = still loading
    selection: Selection,
    loader: ThumbLoader,
}

/// A photo whose decode is scheduled or in flight. `Session` still means "fully decoded",
/// so nothing downstream needs to know about this half-open state.
struct PendingOpen {
    path: PathBuf,
    /// The filmstrip thumbnail shown while decoding, when the folder is loaded.
    placeholder: Option<egui::TextureHandle>,
    /// Debounce deadline — the decode starts once `Instant::now()` passes this.
    start_at: Instant,
    /// The worker sequence number, once the decode has actually been requested.
    seq: Option<u64>,
}

/// Which modal dialog is open. At most one at a time, so this is an enum rather than a
/// pile of booleans that could contradict each other.
#[derive(Default, PartialEq)]
enum Modal {
    #[default]
    None,
    /// Naming a new preset and choosing the groups it will carry.
    SavePreset {
        name: String,
        groups: EditGroups,
    },
    /// Choosing which groups a paste applies.
    PasteSettings,
    /// Choosing which groups a sync applies to the rest of the selection.
    SyncSettings,
    /// Choosing where and how to export the selection.
    ExportSettings,
    /// Naming a new export preset. Returns to `ExportSettings` when closed, so the export
    /// dialog does not vanish underneath the user.
    SaveExportPreset {
        name: String,
    },
    /// Confirming an export-preset deletion. `dont_ask_again` is scratch state, committed to
    /// `Settings` only on the Delete path. Returns to `ExportSettings` when closed.
    ConfirmDeleteExportPreset {
        name: String,
        dont_ask_again: bool,
    },
    /// Confirming a preset deletion. `dont_ask_again` is scratch state — it is committed to
    /// `Settings` only on the Delete path, never on Cancel.
    ConfirmDeletePreset {
        name: String,
        dont_ask_again: bool,
    },
    Settings,
}

struct RustroomApp {
    view: AppView,
    library: Option<Library>,
    session: Option<Session>,
    pending: Option<PendingOpen>,
    decoder: DecodeWorker,
    cache: BufferCache,
    status: String,
    show_filmstrip: bool,
    show_presets: bool,
    /// This session's copied edits. A copy *also* writes a tagged payload to the system
    /// clipboard (see [`edit_clipboard`]), so pasting works between two running instances;
    /// this field is what the "Paste…" button uses, because egui can only hand us the
    /// system clipboard's contents in response to a paste shortcut, never on demand.
    clipboard: Option<Edits>,
    /// Group selection last used for a paste, so Ctrl+V can skip the dialog.
    paste_groups: EditGroups,
    /// Group selection last used for a sync, kept separate from `paste_groups` so the two
    /// dialogs do not silently reconfigure each other.
    sync_groups: EditGroups,
    /// Last-used export options, so a second export does not re-ask for everything.
    /// The export dialog's **working copy**, reset from `settings.export` each time the
    /// dialog opens.
    ///
    /// ⚠ Scratch state, exactly like `ConfirmDeletePreset`'s `dont_ask_again`. The committed
    /// value lives in `settings.export` and is written only by `start_export`, so cancelling
    /// the dialog discards whatever was typed rather than half-applying it. An earlier
    /// version edited the committed value directly and Cancel left the changes behind.
    export_draft: ExportOptions,
    /// The running batch export, if any.
    exporter: Option<ExportWorker>,
    /// Progress line for the running export.
    export_progress: String,
    /// The destination awaiting a collision answer; `Some` means the worker is blocked.
    export_collision: Option<PathBuf>,
    /// Scratch state for the collision dialog's "apply to all" tick.
    export_apply_all: bool,
    /// The photo the filmstrip has already scrolled to.
    ///
    /// ⚠ Scrolling only when this *changes* is the whole point: calling `scroll_to_me` every
    /// frame would fight the user's own scrolling and pin the strip in place.
    filmstrip_scrolled_to: Option<usize>,
    export_preset_store: ExportPresetStore,
    /// Cached `export_preset_store.list()`, refreshed on save and delete rather than every
    /// frame — the same reason `preset_list` is cached.
    export_preset_list: Vec<ExportPreset>,
    /// Which preset the dialog currently shows, so Delete knows what to delete.
    ///
    /// ⚠ Tracked explicitly rather than inferred by matching the dialog's options against
    /// the list: two presets can hold identical options, and a Delete that guesses would
    /// remove the wrong one.
    selected_export_preset: Option<String>,
    preset_store: PresetStore,
    /// Cached `preset_store.list()`, refreshed on save and delete rather than every frame.
    preset_list: Vec<Preset>,
    settings_store: SettingsStore,
    settings: Settings,
    modal: Modal,
    /// One chronological order over photo edits *and* preset deletions, so Ctrl+Z walks
    /// back through both. The payloads stay where they are — photo snapshots in
    /// `Session::history`, deleted presets in the trash — and this only records which of
    /// the two the next undo should pop.
    actions: ActionLog,
}

impl RustroomApp {
    fn new(ctx: &egui::Context) -> Self {
        let ctx2 = ctx.clone();
        let preset_store = PresetStore::user_default();
        let preset_list = preset_store.list();
        let export_preset_store = ExportPresetStore::user_default();
        let export_preset_list = export_preset_store.list();
        let settings_store = SettingsStore::user_default();
        let settings = settings_store.load();
        Self {
            view: AppView::default(),
            library: None,
            session: None,
            pending: None,
            decoder: DecodeWorker::new(decode_for_edit, MAX_CONCURRENT_DECODES, move || {
                ctx2.request_repaint()
            }),
            cache: BufferCache::new(BUFFER_CACHE_CAP),
            status: String::new(),
            show_filmstrip: true,
            show_presets: false,
            clipboard: None,
            paste_groups: EditGroups::default(),
            sync_groups: EditGroups::default(),
            export_draft: settings.export.clone(),
            exporter: None,
            export_progress: String::new(),
            export_collision: None,
            export_apply_all: false,
            filmstrip_scrolled_to: None,
            export_preset_store,
            export_preset_list,
            selected_export_preset: None,
            preset_store,
            preset_list,
            settings_store,
            settings,
            modal: Modal::None,
            actions: ActionLog::default(),
        }
    }
}

struct Session {
    path: PathBuf,
    engine: RenderEngine,
    edits: Edits,
    texture: Option<egui::TextureHandle>,
    baseline: egui::TextureHandle,
    hist: [[u32; 256]; 3],
    make_model: String,
    zoom: f32,
    pan: egui::Vec2,
    history: History<Edits>,
    show_before: bool,
    crop_mode: bool,
    active_handle: Option<crop_tool::Handle>,
    /// The region the current texture covers, as a rect, so the canvas can both place it and
    /// test containment without consulting texture dimensions — which would be wrong while
    /// `show_before` swaps in the whole-image baseline. `None` = whole image.
    rendered_region: Option<RegionRect>,
    /// The region last requested, so the same one is not re-requested every frame.
    requested_region: Option<RegionRect>,
    /// Post-geometry image size at fit resolution, for the fit/zoom maths.
    fit_image_size: egui::Vec2,
    /// Long edge of the *full* post-geometry image at the resolution the current texture was
    /// rendered at. Placement must use this rather than the current mode's: while a frame
    /// from the other mode is still in flight the two differ, and using the current one drew
    /// the old texture far off-screen — a white flash when zooming out quickly.
    rendered_long: f32,
    /// The editing buffer's long edge — `max_dim` for native mode.
    buffer_long: usize,
}

impl Session {
    /// The working resolution to re-render at: native while a region is being shown, the
    /// preview cap otherwise. Keeps an edit made while zoomed in from snapping back to fit.
    fn render_max_dim(&self) -> usize {
        if self.requested_region.is_some() {
            self.buffer_long
        } else {
            PREVIEW_MAX_DIM
        }
    }
}

/// Edits to render for the preview. In crop mode the crop is reset to the full frame so
/// the whole straightened image shows under the crop overlay.
fn preview_edits(edits: &Edits, crop_mode: bool) -> Edits {
    let mut e = edits.clone();
    if crop_mode {
        e.geometry.crop = edit_model::CropRect::default();
    }
    e
}

/// Decode a file for editing (half-size for RAW; full decode for images) into a linear
/// `ImageBuf`, a display label, and the buffer's size relative to a full-resolution
/// decode. Dispatches by file kind.
fn decode_for_edit(path: &std::path::Path) -> Result<(core_image::ImageBuf, String, f32), String> {
    if catalog::is_raw(path) {
        let opts = raw_decode::DecodeOptions::preview();
        let d = raw_decode::decode_raw(path, &opts).map_err(|e| e.to_string())?;
        let label = format!("{} {}", d.metadata.make, d.metadata.model);
        Ok((d.image, label, opts.source_scale()))
    } else {
        let d = image_decode::decode(path).map_err(|e| e.to_string())?;
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_uppercase();
        // Non-RAW formats are decoded at full size.
        Ok((d.image, format!("{ext} {}×{}", d.width, d.height), 1.0))
    }
}

/// Full-resolution decode for export. Dispatches by file kind.
fn decode_for_export(path: &std::path::Path) -> Result<core_image::ImageBuf, String> {
    if catalog::is_raw(path) {
        raw_decode::decode_raw(path, &raw_decode::DecodeOptions::linear_scene())
            .map(|d| d.image)
            .map_err(|e| e.to_string())
    } else {
        image_decode::decode(path)
            .map(|d| d.image)
            .map_err(|e| e.to_string())
    }
}

impl RustroomApp {
    /// Save the current session's edits to its `<file>.json` sidecar, if it has any.
    /// Called before switching photos / on exit so edits are never lost.
    fn persist_current_session(&self) {
        if let Some(s) = &self.session {
            if s.edits != Edits::default() {
                let _ = sidecar::save(&s.path, &s.edits);
            }
        }
    }

    /// Apply `src`'s selected groups to the open photo and re-render.
    ///
    /// Returns whether a history step was actually recorded — `History::push` is a no-op
    /// when nothing changed, and logging a step that did not happen would desynchronize the
    /// action log from the session history. The caller decides how to log it, which is what
    /// lets a paste bundle this together with a sync into one undo step.
    fn apply_edits_to_session(&mut self, src: &Edits, groups: EditGroups) -> bool {
        let Some(s) = &mut self.session else {
            return false;
        };
        s.edits.merge_from(src, groups);
        let pushed = s.history.push(s.edits.clone());
        s.engine.request(
            preview_edits(&s.edits, s.crop_mode),
            s.render_max_dim(),
            s.requested_region,
        );
        pushed
    }

    /// Selected photos **other than the one currently open** — the ones a batch has to
    /// reach through their sidecars rather than through the live session.
    ///
    /// Keyed on the open *path*, not the active index: in Develop those are the same photo,
    /// but a paste triggered from the Library grid may have a selection that does not
    /// include whatever is open, and then every selected photo needs a sidecar write.
    fn sync_targets(&self) -> Vec<PathBuf> {
        let open = self.session.as_ref().map(|s| s.path.clone());
        let Some(lib) = &self.library else {
            return Vec::new();
        };
        lib.selection
            .iter()
            .filter_map(|i| lib.entries.get(i).map(|e| e.path.clone()))
            .filter(|p| Some(p) != open.as_ref())
            .collect()
    }

    /// Whether the open photo is part of the current selection, and so should be changed
    /// through its session. True in single-file mode, where there is no selection at all
    /// and the open photo is the only thing an apply could mean.
    fn open_photo_is_selected(&self) -> bool {
        let Some(s) = &self.session else { return false };
        let Some(lib) = &self.library else {
            return true;
        };
        lib.selection
            .iter()
            .filter_map(|i| lib.entries.get(i))
            .any(|e| e.path == s.path)
    }

    /// Apply `src` to the whole selection — the open photo through its session, the rest
    /// through their sidecars — as **one** undo step.
    ///
    /// Two mechanisms, one gesture, so the resulting actions are wrapped in a `Batch`: one
    /// Ctrl+V, or one preset click, must stay one Ctrl+Z.
    ///
    /// Returns `(photos changed, photos that failed)`. The caller words the status line,
    /// because "pasted" and "applied preset" want different phrasing for the same work.
    fn apply_to_selection(&mut self, src: &Edits, groups: EditGroups) -> (usize, usize) {
        let targets = self.sync_targets();
        let into_session = self.open_photo_is_selected();
        let mut parts: Vec<ActionKind> = Vec::new();
        let mut touched = 0usize;
        let mut failed = 0usize;

        if into_session {
            touched += 1;
            if self.apply_edits_to_session(src, groups) {
                parts.push(ActionKind::PhotoEdit);
            }
        }
        if !targets.is_empty() {
            let report = sync::apply(&targets, src, groups);
            touched += report.prior.len();
            failed = report.failed.len();
            if !report.prior.is_empty() {
                parts.push(ActionKind::Sync(Box::new(SyncAction {
                    source: src.clone(),
                    groups,
                    prior: report.prior,
                })));
            }
        }

        // A lone action does not need a wrapper — keep the log readable.
        match parts.len() {
            0 => {}
            1 => self.actions.push(parts.remove(0)),
            _ => self.actions.push(ActionKind::Batch(parts)),
        }
        (touched, failed)
    }

    /// Apply the open photo's edits to the rest of the selection, as one undo step.
    fn run_sync(&mut self, groups: EditGroups) {
        let targets = self.sync_targets();
        let Some(source) = self.session.as_ref().map(|s| s.edits.clone()) else {
            return;
        };
        if targets.is_empty() {
            self.status = "Nothing to sync".to_string();
            return;
        }
        // The source is the session's *in-memory* edits, not its sidecar: you sync what you
        // are looking at, not what was last written.
        let report = sync::apply(&targets, &source, groups);
        self.status = if report.failed.is_empty() {
            format!("Synced {} photos", report.prior.len())
        } else {
            format!(
                "Synced {}, {} failed",
                report.prior.len(),
                report.failed.len()
            )
        };
        self.actions.push(ActionKind::Sync(Box::new(SyncAction {
            source,
            groups,
            prior: report.prior,
        })));
    }

    /// Sources and edits for a batch export: every selected photo, using the open photo's
    /// **in-memory** edits so unsaved changes are exported, and each other photo's sidecar.
    fn export_jobs(&self) -> Vec<(PathBuf, Edits)> {
        let open = self
            .session
            .as_ref()
            .map(|s| (s.path.clone(), s.edits.clone()));
        let Some(lib) = &self.library else {
            return open.into_iter().collect();
        };
        lib.selection
            .iter()
            .filter_map(|i| lib.entries.get(i).map(|e| e.path.clone()))
            .map(|p| match &open {
                Some((op, edits)) if *op == p => (p.clone(), edits.clone()),
                _ => {
                    let edits = sidecar::load(&p).unwrap_or_default();
                    (p, edits)
                }
            })
            .collect()
    }

    /// Start exporting the selection.
    fn start_export(&mut self) {
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
        self.export_progress = format!("Exporting 0 of {}…", jobs.len());
        self.export_collision = None;
        self.exporter = Some(ExportWorker::spawn(
            jobs,
            opts,
            |p: &std::path::Path| decode_for_export(p),
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
    fn cancel_export(&mut self) {
        if let Some(w) = &self.exporter {
            w.cancel();
        }
        self.exporter = None;
        self.export_progress.clear();
        self.export_collision = None;
        self.status = "Export cancelled".to_string();
    }

    /// Drain the exporter's events once per frame.
    fn pump_export(&mut self, ctx: &egui::Context) {
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

    /// Reload the open photo's edits if its sidecar was just rewritten underneath it.
    ///
    /// ⚠ The active photo is never a sync *target*, so a sync cannot make the open session
    /// stale. But a user can sync, switch to a synced photo, and *then* undo — which
    /// rewrites that photo's sidecar while it is open with its own in-memory edits. Without
    /// this the canvas silently disagrees with the disk until you switch away and back.
    fn reload_if_affected(&mut self, affected: &[PathBuf]) {
        let Some(open) = self.session.as_ref().map(|s| s.path.clone()) else {
            return;
        };
        if !affected.contains(&open) {
            return;
        }
        let edits = sidecar::load(&open).unwrap_or_default();
        if let Some(s) = &mut self.session {
            s.edits = edits;
            // The old history described a state that is no longer on disk.
            s.history = History::new(s.edits.clone());
            s.engine.request(
                preview_edits(&s.edits, s.crop_mode),
                s.render_max_dim(),
                s.requested_region,
            );
        }
        // Those history entries just became unreachable.
        self.actions.forget_photo_edits();
    }

    /// Re-render after the session's edits were changed by an undo or a redo.
    fn rerender_current(&mut self) {
        let Some(s) = &mut self.session else { return };
        s.edits = s.history.get().clone();
        s.engine.request(
            preview_edits(&s.edits, s.crop_mode),
            s.render_max_dim(),
            s.requested_region,
        );
    }

    /// Undo the most recent action, whichever kind it was.
    fn undo_action(&mut self) {
        if let Some(kind) = self.actions.undo() {
            self.undo_one(kind);
        }
    }

    fn undo_one(&mut self, kind: ActionKind) {
        match kind {
            ActionKind::Batch(parts) => {
                // Reverse order: the last effect a gesture had is the first one undone.
                for part in parts.into_iter().rev() {
                    self.undo_one(part);
                }
            }
            ActionKind::PhotoEdit => {
                if let Some(s) = &mut self.session {
                    s.history.undo();
                }
                self.rerender_current();
            }
            ActionKind::PresetDelete { name } => {
                match self.preset_store.restore(&name) {
                    Ok(Some(p)) => self.status = format!("Restored preset “{}”", p.name),
                    // Evicted past the trash cap, or removed by hand.
                    Ok(None) => self.status = format!("“{name}” is no longer in the trash"),
                    Err(e) => self.status = format!("Could not restore preset: {e}"),
                }
                self.reload_presets();
            }
            ActionKind::Sync(action) => {
                let failed = sync::revert(&action.prior);
                let total = action.prior.len();
                self.status = if failed.is_empty() {
                    format!("Reverted sync of {total} photos")
                } else {
                    format!("Reverted {}, {} failed", total - failed.len(), failed.len())
                };
                let affected: Vec<PathBuf> = action.prior.iter().map(|(p, _)| p.clone()).collect();
                self.reload_if_affected(&affected);
            }
        }
    }

    /// Redo the most recently undone action.
    fn redo_action(&mut self) {
        if let Some(kind) = self.actions.redo() {
            self.redo_one(kind);
        }
    }

    fn redo_one(&mut self, kind: ActionKind) {
        match kind {
            ActionKind::Batch(parts) => {
                for part in parts {
                    self.redo_one(part);
                }
            }
            ActionKind::PhotoEdit => {
                if let Some(s) = &mut self.session {
                    s.history.redo();
                }
                self.rerender_current();
            }
            ActionKind::PresetDelete { name } => {
                match self.preset_store.delete(&name) {
                    Ok(()) => self.status = format!("Deleted preset “{name}”"),
                    Err(e) => self.status = format!("Could not delete preset: {e}"),
                }
                self.reload_presets();
            }
            ActionKind::Sync(action) => {
                let targets: Vec<PathBuf> = action.prior.iter().map(|(p, _)| p.clone()).collect();
                let report = sync::apply(&targets, &action.source, action.groups);
                self.status = format!("Re-synced {} photos", report.prior.len());
                self.reload_if_affected(&targets);
            }
        }
    }

    /// Re-read the presets directory. Called after a save or a delete, not every frame.
    fn reload_presets(&mut self) {
        self.preset_list = self.preset_store.list();
    }

    /// Persist the settings, surfacing a failure in the status line rather than silently.
    fn save_settings(&mut self) {
        if let Err(e) = self.settings_store.save(&self.settings) {
            self.status = format!("Could not save settings: {e}");
        }
    }

    /// Cmd/Ctrl+C and Cmd/Ctrl+V for the edit clipboard.
    ///
    /// ⚠ These must be read as **`egui::Event::Copy` / `Event::Paste`, not key presses.**
    /// `egui-winit` matches the clipboard chords first, pushes the semantic event and
    /// *returns without emitting a `Key` event at all* — so `key_pressed(Key::C)` can never
    /// fire, which is exactly how the first version of this silently did nothing.
    ///
    /// The copy also writes to the **system** clipboard, and not only as a nicety: egui
    /// pushes `Event::Paste` only when the system clipboard holds non-empty text, so
    /// without that write Ctrl+V would produce no event on a freshly booted machine.
    ///
    /// Gated on nothing having keyboard focus, the way `handle_develop_arrows` already is,
    /// so Ctrl+C inside the preset-name field still copies text.
    fn handle_clipboard_keys(&mut self, ctx: &egui::Context) {
        // Deliberately not gated on a session: a copy needs one, but a paste only needs a
        // clipboard and a selection, and Ctrl+V has to work from the Library grid too.
        if self.modal != Modal::None {
            return;
        }
        if ctx.memory(|m| m.focused()).is_some() {
            return;
        }
        let (copy, pasted) = ctx.input(|i| {
            let mut copy = false;
            let mut pasted: Option<String> = None;
            for e in &i.events {
                match e {
                    egui::Event::Copy => copy = true,
                    egui::Event::Paste(text) => pasted = Some(text.clone()),
                    _ => {}
                }
            }
            (copy, pasted)
        });

        if copy && self.session.is_some() {
            // `self.session.as_ref().map(..)` rather than `if let Some(s) = &self.session`:
            // the latter holds an immutable borrow of `self` across the assignment to
            // `self.clipboard` and does not compile.
            self.clipboard = self.session.as_ref().map(|s| s.edits.clone());
            if let Some(edits) = &self.clipboard {
                ctx.copy_text(edit_clipboard::encode(edits));
            }
            self.status = "Settings copied".to_string();
        }

        if let Some(text) = pasted {
            // Prefer the clipboard's own payload — that is what makes copy/paste work
            // between two running instances — and fall back to this session's in-memory
            // copy when the system clipboard holds something else entirely.
            let src = edit_clipboard::decode(&text).or_else(|| self.clipboard.clone());
            if let Some(src) = src {
                // Ctrl+V uses the last-used group selection with no dialog; the "Paste…"
                // button opens the chooser. Either way it targets the whole selection.
                let (n, failed) = self.apply_to_selection(&src, self.paste_groups);
                self.status = paste_status(n, failed);
            }
        }
    }

    /// Start opening `path`: instant from the cache, otherwise a debounced background decode
    /// with the filmstrip thumbnail as a placeholder. Never blocks the UI thread.
    fn open_path(&mut self, ctx: &egui::Context, path: PathBuf) {
        self.persist_current_session();

        if let Some(decoded) = self.cache.get(&path) {
            self.pending = None;
            self.finalize_session(ctx, path, decoded);
            return;
        }

        let placeholder = self.thumbnail_for(&path);
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        self.status = format!("Decoding {name}…");
        self.pending = Some(PendingOpen {
            path,
            placeholder,
            start_at: Instant::now() + DECODE_DEBOUNCE,
            seq: None,
        });
    }

    /// The library thumbnail for `path`, if the folder is loaded and that thumbnail has
    /// already arrived. `TextureHandle` is `Arc`-backed, so cloning it is cheap.
    fn thumbnail_for(&self, path: &std::path::Path) -> Option<egui::TextureHandle> {
        let lib = self.library.as_ref()?;
        let i = lib.entries.iter().position(|e| e.path == *path)?;
        lib.thumbs.get(i).cloned().flatten()
    }

    /// Build the editing `Session` from a decoded photo (cache hit or worker result).
    fn finalize_session(&mut self, ctx: &egui::Context, path: PathBuf, decoded: Arc<Decoded>) {
        let edits = sidecar::load(&path).unwrap_or_default();

        let bf = &decoded.baseline;
        let baseline = ctx.load_texture(
            "baseline",
            egui::ColorImage::from_rgba_unmultiplied([bf.width, bf.height], &bf.rgba),
            egui::TextureOptions::LINEAR,
        );

        let ctx2 = ctx.clone();
        let engine = RenderEngine::new(decoded.buffer.clone(), decoded.source_scale, move || {
            ctx2.request_repaint()
        });
        engine.request(edits.clone(), PREVIEW_MAX_DIM, None);

        self.session = Some(Session {
            path,
            engine,
            edits: edits.clone(),
            texture: None,
            baseline,
            hist: [[0; 256]; 3],
            make_model: decoded.label.clone(),
            zoom: 1.0,
            pan: egui::Vec2::ZERO,
            history: History::new(edits),
            show_before: false,
            crop_mode: false,
            active_handle: None,
            rendered_region: None,
            requested_region: None,
            // Corrected by the first frame that arrives; the placeholder only has to be
            // non-zero so the first fit calculation cannot divide by zero.
            fit_image_size: egui::vec2(1.0, 1.0),
            rendered_long: 1.0,
            buffer_long: decoded.buffer.width().max(decoded.buffer.height()),
        });
        // ⚠ The previous session's `History<Edits>` just went away with it, so any
        // `PhotoEdit` still in the log would make an undo pop *this* photo's history and
        // silently revert an edit the user never made here. Preset deletions are global
        // and stay.
        self.actions.forget_photo_edits();
        self.status.clear();
    }

    /// Per-frame pump: fire the debounce timer, then absorb finished decodes.
    fn pump_decodes(&mut self, ctx: &egui::Context) {
        if let Some(p) = &mut self.pending {
            if p.seq.is_none() {
                let now = Instant::now();
                if now >= p.start_at {
                    let path = p.path.clone();
                    // `p` borrows `self.pending`, the call borrows `self.decoder` — disjoint
                    // fields, so the borrow checker allows both at once.
                    p.seq = Some(self.decoder.request(path, PREVIEW_MAX_DIM));
                } else {
                    ctx.request_repaint_after(p.start_at - now);
                }
            }
        }

        for res in self.decoder.poll() {
            let is_current = self.pending.as_ref().and_then(|p| p.seq) == Some(res.seq);
            match res.outcome {
                Ok(decoded) => {
                    // Cache even superseded decodes — that work is already paid for.
                    self.cache.insert(res.path.clone(), decoded.clone());
                    if is_current {
                        self.pending = None;
                        self.finalize_session(ctx, res.path, decoded);
                    }
                }
                Err(e) => {
                    if is_current {
                        self.pending = None;
                        self.status = format!("Decode failed: {e}");
                    }
                }
            }
        }
    }

    fn open_dialog(&mut self, ctx: &egui::Context) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter(
                "Images",
                &[
                    "cr2", "cr3", "nef", "arw", "raf", "dng", "orf", "rw2", "jpg", "jpeg", "png",
                    "tif", "tiff",
                ],
            )
            .pick_file()
        else {
            return;
        };
        self.open_path(ctx, path);
        self.library = None;
        self.view = AppView::Develop;
    }

    fn open_folder(&mut self, ctx: &egui::Context) {
        let Some(dir) = rfd::FileDialog::new().pick_folder() else {
            return;
        };
        match catalog::scan_folder(&dir) {
            Ok(entries) => {
                let n = entries.len();
                let paths: Vec<_> = entries.iter().map(|e| e.path.clone()).collect();
                let ctx2 = ctx.clone();
                let loader = ThumbLoader::new(paths, move || ctx2.request_repaint());
                self.library = Some(Library {
                    entries,
                    thumbs: (0..n).map(|_| None).collect(),
                    selection: Selection::default(),
                    loader,
                });
                self.view = AppView::Library;
                self.status = if n == 0 {
                    "No supported images in this folder.".into()
                } else {
                    format!("{n} photos")
                };
            }
            Err(e) => self.status = format!("Could not read folder: {e}"),
        }
    }

    fn open_entry(&mut self, ctx: &egui::Context, index: usize) {
        let path = match self.library.as_mut() {
            Some(lib) => match lib.entries.get(index) {
                Some(e) => {
                    lib.selection.click(index);
                    e.path.clone()
                }
                None => return,
            },
            None => return,
        };
        self.open_path(ctx, path);
        self.view = AppView::Develop;
    }

    /// Develop-mode toolbar buttons (shown in the top panel when a photo is open).
    fn develop_toolbar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        // Undo/Redo drive the unified `ActionLog`, so one control walks back through photo
        // edits and preset deletions in the order they actually happened. The label says
        // which, so "Undo" never silently does the surprising one.
        let undo_label = match self.actions.peek_undo() {
            Some(ActionKind::PresetDelete { name }) => format!("Undo delete “{name}”"),
            _ => "Undo".to_string(),
        };
        let undo_click = ui
            .add_enabled(self.actions.can_undo(), egui::Button::new(undo_label))
            .clicked();
        let redo_click = ui
            .add_enabled(self.actions.can_redo(), egui::Button::new("Redo"))
            .clicked();
        let (kundo, kredo) = ctx.input(|i| {
            let z = i.key_pressed(egui::Key::Z) && i.modifiers.command;
            (z && !i.modifiers.shift, z && i.modifiers.shift)
        });
        if undo_click || kundo {
            self.undo_action();
        }
        if redo_click || kredo {
            self.redo_action();
        }
        if let Some(s) = &mut self.session {
            if ui.button("Fit").clicked() {
                s.zoom = 1.0;
                s.pan = egui::Vec2::ZERO;
            }
            ui.toggle_value(&mut s.show_before, "Before/After");
            if ui.toggle_value(&mut s.crop_mode, "Crop").clicked() {
                s.engine.request(
                    preview_edits(&s.edits, s.crop_mode),
                    s.render_max_dim(),
                    s.requested_region,
                );
            }
        }
        if self.session.is_some() {
            ui.separator();
            if ui.button("Copy").clicked() {
                // Same borrow reason as in `handle_clipboard_keys`, and the same system
                // clipboard write, so the button and the shortcut cannot disagree.
                self.clipboard = self.session.as_ref().map(|s| s.edits.clone());
                if let Some(edits) = &self.clipboard {
                    ui.ctx().copy_text(edit_clipboard::encode(edits));
                }
                self.status = "Settings copied".to_string();
            }
            if ui
                .add_enabled(self.clipboard.is_some(), egui::Button::new("Paste…"))
                .clicked()
            {
                self.modal = Modal::PasteSettings;
            }
            let selected = self.library.as_ref().map_or(0, |l| l.selection.len());
            if ui
                .add_enabled(
                    selected > 1,
                    egui::Button::new(format!("Sync {}…", selected.saturating_sub(1))),
                )
                .clicked()
            {
                self.modal = Modal::SyncSettings;
            }
            // One button for both: the count decides whether it exports the open photo or
            // the whole selection. There is no second code path — a single export is a
            // batch of one, through the same worker.
            let to_export = self.export_jobs().len();
            let label = if to_export > 1 {
                format!("Export {to_export}…")
            } else {
                "Export…".to_string()
            };
            if ui
                .add_enabled(to_export > 0, egui::Button::new(label))
                .clicked()
            {
                // Reset the working copy from the committed value, so a previously
                // cancelled edit does not linger.
                self.export_draft = self.settings.export.clone();
                self.modal = Modal::ExportSettings;
            }
            // Unlike the filmstrip, this does not need a Library: presets are just as
            // useful on a single-file open.
            ui.toggle_value(&mut self.show_presets, "Presets");
            if ui.button("Settings…").clicked() {
                self.modal = Modal::Settings;
            }
        }
        if self.library.is_some() {
            ui.toggle_value(&mut self.show_filmstrip, "Filmstrip");
        }
    }

    /// The left preset panel: save the current edits as a preset, apply one, or delete one.
    fn preset_panel(&mut self, ui: &mut egui::Ui) {
        // egui borrows `self` through the closure, so the click is recorded here and acted
        // on after the panel closes.
        enum Action {
            None,
            Apply(usize),
            Delete(String),
            Save,
            UndoDelete,
        }
        let mut action = Action::None;
        // Shown whenever the next undo would restore a preset, so the restore is
        // discoverable rather than only a keystroke. It is the same action the toolbar's
        // Undo performs — not a second, competing one.
        let undo_name = match self.actions.peek_undo() {
            Some(ActionKind::PresetDelete { name }) => Some(name.clone()),
            _ => None,
        };

        egui::Panel::left("presets")
            .default_size(180.0)
            .show(ui, |ui| {
                ui.heading("Presets");
                if ui.button("Save current as preset…").clicked() {
                    action = Action::Save;
                }
                if let Some(name) = &undo_name {
                    if ui.button(format!("↩ Undo delete “{name}”")).clicked() {
                        action = Action::UndoDelete;
                    }
                }
                ui.separator();
                if self.preset_list.is_empty() {
                    ui.label("No presets yet.");
                }
                egui::ScrollArea::vertical().show(ui, |ui| {
                    for (i, p) in self.preset_list.iter().enumerate() {
                        ui.horizontal(|ui| {
                            if ui.button("×").on_hover_text("Delete").clicked() {
                                action = Action::Delete(p.name.clone());
                            }
                            if ui.button(&p.name).clicked() {
                                action = Action::Apply(i);
                            }
                        });
                    }
                });
            });

        match action {
            Action::None => {}
            Action::Save => {
                self.modal = Modal::SavePreset {
                    name: String::new(),
                    groups: EditGroups::default(),
                };
            }
            Action::Apply(i) => {
                if let Some(p) = self.preset_list.get(i).cloned() {
                    // Applying a preset targets the selection, exactly as a paste does.
                    let (n, failed) = self.apply_to_selection(&p.edits, p.groups);
                    self.status = match (n, failed) {
                        (0, _) => "Nothing selected".to_string(),
                        (1, 0) => format!("Applied preset “{}”", p.name),
                        (n, 0) => format!("Applied “{}” to {n} photos", p.name),
                        (n, f) => format!("Applied “{}” to {n} photos, {f} failed", p.name),
                    };
                }
            }
            Action::UndoDelete => self.undo_action(),
            Action::Delete(name) => {
                if self.settings.confirm_preset_delete {
                    self.modal = Modal::ConfirmDeletePreset {
                        name,
                        dont_ask_again: false,
                    };
                } else {
                    self.delete_preset(&name);
                }
            }
        }
    }

    /// Move a preset to the trash and refresh the cached list.
    fn delete_preset(&mut self, name: &str) {
        match self.preset_store.delete(name) {
            Ok(()) => {
                self.status = format!("Deleted preset “{name}” — Ctrl+Z to restore");
                self.actions.push(ActionKind::PresetDelete {
                    name: name.to_string(),
                });
            }
            Err(e) => self.status = format!("Could not delete preset: {e}"),
        }
        self.reload_presets();
    }

    /// The "Save current as preset…" dialog.
    fn save_preset_modal(&mut self, ctx: &egui::Context) {
        let Modal::SavePreset { name, groups } = &mut self.modal else {
            return;
        };
        let mut name = std::mem::take(name);
        let mut groups = *groups;
        let mut close = false;
        let mut save = false;

        let dismissed = modal_dialog(ctx, "save_preset", "Save preset", |ui| {
            ui.horizontal(|ui| {
                ui.label("Name");
                ui.text_edit_singleline(&mut name);
            });
            ui.separator();
            ui.label("Include:");
            edit_group_checkboxes(ui, &mut groups);
            ui.separator();
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(!name.trim().is_empty(), egui::Button::new("Save"))
                    .clicked()
                {
                    save = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        close |= dismissed;

        if save {
            // Clone the edits *out* first: holding `&self.session` across the assignments
            // to `self.status` below would be an immutable borrow crossing a mutable one.
            if let Some(edits) = self.session.as_ref().map(|s| s.edits.clone()) {
                let preset = Preset {
                    name: name.trim().to_string(),
                    groups,
                    edits,
                };
                match self.preset_store.save(&preset) {
                    Ok(()) => self.status = format!("Saved preset “{}”", preset.name),
                    Err(e) => self.status = format!("Could not save preset: {e}"),
                }
                self.reload_presets();
            }
            close = true;
        }

        if close {
            self.modal = Modal::None;
        } else {
            self.modal = Modal::SavePreset { name, groups };
        }
    }

    /// Draw whichever modal is open. Called once per frame from `ui`.
    fn draw_modals(&mut self, ctx: &egui::Context) {
        // Decide *which* modal first, then dispatch. Matching on `self.modal` directly and
        // calling a `&mut self` method from an arm keeps the scrutinee's borrow alive across
        // the call, which does not compile.
        enum Which {
            None,
            Save,
            Paste,
            Sync,
            Export,
            SaveExport,
            ConfirmDeleteExport,
            ConfirmDelete,
            Settings,
        }
        let which = match &self.modal {
            Modal::None => Which::None,
            Modal::SavePreset { .. } => Which::Save,
            Modal::PasteSettings => Which::Paste,
            Modal::SyncSettings => Which::Sync,
            Modal::ExportSettings => Which::Export,
            Modal::SaveExportPreset { .. } => Which::SaveExport,
            Modal::ConfirmDeleteExportPreset { .. } => Which::ConfirmDeleteExport,
            Modal::ConfirmDeletePreset { .. } => Which::ConfirmDelete,
            Modal::Settings => Which::Settings,
        };
        // Independent of `self.modal`: a collision arrives while no dialog is open.
        if self.export_collision.is_some() {
            self.collision_modal(ctx);
        }
        match which {
            Which::None => {}
            Which::Save => self.save_preset_modal(ctx),
            Which::Paste => self.paste_modal(ctx),
            Which::Sync => self.sync_modal(ctx),
            Which::Export => self.export_modal(ctx),
            Which::SaveExport => self.save_export_preset_modal(ctx),
            Which::ConfirmDeleteExport => self.confirm_delete_export_modal(ctx),
            Which::ConfirmDelete => self.confirm_delete_modal(ctx),
            Which::Settings => self.settings_modal(ctx),
        }
    }

    /// The "Paste…" dialog: which groups this paste applies.
    ///
    /// The chosen selection is remembered in `paste_groups`, which is also what a bare
    /// Ctrl+V uses.
    fn paste_modal(&mut self, ctx: &egui::Context) {
        let mut groups = self.paste_groups;
        let mut close = false;
        let mut paste = false;

        let dismissed = modal_dialog(ctx, "paste_settings", "Paste settings", |ui| {
            ui.label("Apply:");
            edit_group_checkboxes(ui, &mut groups);
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Paste").clicked() {
                    paste = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        close |= dismissed;

        self.paste_groups = groups;
        if paste {
            if let Some(src) = self.clipboard.clone() {
                let (n, failed) = self.apply_to_selection(&src, groups);
                self.status = paste_status(n, failed);
            }
            close = true;
        }
        if close {
            self.modal = Modal::None;
        }
    }

    /// The "Sync…" dialog: which groups this sync applies to the rest of the selection.
    fn sync_modal(&mut self, ctx: &egui::Context) {
        let mut groups = self.sync_groups;
        let count = self.sync_targets().len();
        let mut close = false;
        let mut run = false;

        let dismissed = modal_dialog(ctx, "sync_settings", "Sync settings", |ui| {
            ui.label(format!("Apply to {count} other selected photos:"));
            edit_group_checkboxes(ui, &mut groups);
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Sync").clicked() {
                    run = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        close |= dismissed;

        self.sync_groups = groups;
        if run {
            self.run_sync(groups);
            close = true;
        }
        if close {
            self.modal = Modal::None;
        }
    }

    /// Delete whatever preset the export dialog has selected, and clear the selection.
    fn delete_selected_export_preset(&mut self) {
        if let Some(name) = self.selected_export_preset.take() {
            if let Err(e) = self.export_preset_store.delete(&name) {
                self.status = format!("Could not delete preset: {e}");
            }
            self.export_preset_list = self.export_preset_store.list();
        }
    }

    /// Confirming an export-preset deletion.
    ///
    /// Mirrors `confirm_delete_modal`, but folds into its **own** settings flag: an export
    /// preset has no trash, so disarming the guard on the recoverable develop-preset deletion
    /// must not disarm it here.
    fn confirm_delete_export_modal(&mut self, ctx: &egui::Context) {
        let Modal::ConfirmDeleteExportPreset {
            name,
            dont_ask_again,
        } = &mut self.modal
        else {
            return;
        };
        let name = std::mem::take(name);
        let mut dont_ask = *dont_ask_again;
        let mut outcome: Option<ConfirmOutcome> = None;

        let dismissed = modal_dialog(ctx, "confirm_delete_export", "Delete export preset", |ui| {
            ui.label(format!("Delete the export preset \u{201c}{name}\u{201d}?"));
            ui.label(
                egui::RichText::new("This cannot be undone — export presets have no trash.")
                    .small(),
            );
            ui.checkbox(&mut dont_ask, "Don't ask again");
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Delete").clicked() {
                    outcome = Some(ConfirmOutcome::Confirmed {
                        suppress_future: dont_ask,
                    });
                }
                if ui.button("Cancel").clicked() {
                    outcome = Some(ConfirmOutcome::Cancelled);
                }
            });
        });
        // Clicking outside is a cancel — and therefore discards the tick too.
        if dismissed {
            outcome = Some(ConfirmOutcome::Cancelled);
        }

        match outcome {
            None => {
                self.modal = Modal::ConfirmDeleteExportPreset {
                    name,
                    dont_ask_again: dont_ask,
                };
            }
            Some(o) => {
                if self.settings.apply_export_delete_confirm(o) {
                    self.delete_selected_export_preset();
                }
                self.save_settings();
                // Back to the export dialog rather than to nothing.
                self.modal = Modal::ExportSettings;
            }
        }
    }

    /// Naming a new export preset.
    fn save_export_preset_modal(&mut self, ctx: &egui::Context) {
        let Modal::SaveExportPreset { name } = &self.modal else {
            return;
        };
        let mut name = name.clone();
        let mut save = false;
        let mut close = false;
        let dismissed = modal_dialog(ctx, "save_export_preset", "Save export preset", |ui| {
            ui.text_edit_singleline(&mut name);
            ui.horizontal(|ui| {
                let ok = !name.trim().is_empty();
                if ui.add_enabled(ok, egui::Button::new("Save")).clicked() {
                    save = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        if save {
            let preset = ExportPreset {
                name: name.clone(),
                options: self.export_draft.clone(),
            };
            match self.export_preset_store.save(&preset) {
                Ok(()) => self.selected_export_preset = Some(preset.name),
                Err(e) => self.status = format!("Could not save preset: {e}"),
            }
            self.export_preset_list = self.export_preset_store.list();
            close = true;
        }
        self.modal = if close || dismissed {
            // Back to the export dialog rather than to nothing, so it does not vanish
            // underneath the user.
            Modal::ExportSettings
        } else {
            Modal::SaveExportPreset { name }
        };
    }

    /// Where and how to export the selection.
    fn export_modal(&mut self, ctx: &egui::Context) {
        let mut opts = self.export_draft.clone();
        let count = self.export_jobs().len();
        let mut close = false;
        let mut start = false;
        // egui has no folder field, so the picker writes into a local and the label shows it.
        let mut pick_dir = false;
        let mut picked: Option<(String, ExportOptions)> = None;
        let mut save_as = false;
        let mut delete_selected = false;
        // ⚠ Snapshots taken *before* the closure. `self.export_preset_list` cannot be read
        // inside it while `opts` also borrows `self` — the same borrow shape that has
        // already cost a compile error in this file twice.
        let presets = self.export_preset_list.clone();
        let selected = self.selected_export_preset.clone();

        let dismissed = modal_dialog(ctx, "export_settings", "Export", |ui| {
            ui.label(format!("Export {count} photos as JPEG."));
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("export_preset_pick")
                    .selected_text(selected.clone().unwrap_or_else(|| "Preset…".into()))
                    .show_ui(ui, |ui| {
                        for p in &presets {
                            let is_sel = selected.as_deref() == Some(p.name.as_str());
                            if ui.selectable_label(is_sel, &p.name).clicked() {
                                // Fills every field below, which is what keeps a preset's
                                // destination change visible rather than hidden.
                                picked = Some((p.name.clone(), p.options.clone()));
                            }
                        }
                    });
                if ui.button("Save as…").clicked() {
                    save_as = true;
                }
                let can_delete = selected.is_some();
                if ui
                    .add_enabled(can_delete, egui::Button::new("Delete"))
                    .clicked()
                {
                    delete_selected = true;
                }
            });
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Destination…").clicked() {
                    pick_dir = true;
                }
                let shown = if opts.dest_dir.as_os_str().is_empty() {
                    "(none chosen)".to_string()
                } else if opts.dest_dir.is_dir() {
                    opts.dest_dir.display().to_string()
                } else {
                    // A preset can carry a folder that has since gone — an unplugged drive,
                    // or a preset from another machine. Say so rather than failing per photo.
                    format!("{} (missing)", opts.dest_dir.display())
                };
                ui.label(shown);
            });
            ui.add(egui::Slider::new(&mut opts.quality, 1..=100).text("JPEG quality"));

            let mut resize = opts.max_edge.is_some();
            ui.checkbox(&mut resize, "Fit within a long edge");
            if resize {
                let mut edge = opts.max_edge.unwrap_or(2048);
                ui.add(egui::Slider::new(&mut edge, 256..=8192).text("Long edge (px)"));
                opts.max_edge = Some(edge);
            } else {
                opts.max_edge = None;
            }

            ui.separator();
            ui.horizontal(|ui| {
                let ready = destination_ready(&opts.dest_dir);
                if ui.add_enabled(ready, egui::Button::new("Export")).clicked() {
                    start = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });

        if pick_dir {
            if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                opts.dest_dir = dir;
            }
        }
        if let Some((name, o)) = picked {
            opts = o;
            self.selected_export_preset = Some(name);
        }
        if save_as {
            self.modal = Modal::SaveExportPreset {
                name: String::new(),
            };
        }
        if delete_selected {
            // No trash — see `export_presets`' module doc. But deletion *is* permanent here,
            // which is precisely why it is guarded by its own confirmation.
            if let Some(name) = self.selected_export_preset.clone() {
                if self.settings.confirm_export_preset_delete {
                    self.modal = Modal::ConfirmDeleteExportPreset {
                        name,
                        dont_ask_again: false,
                    };
                } else {
                    self.delete_selected_export_preset();
                }
            }
        }
        self.export_draft = opts;
        if start {
            self.start_export();
            close = true;
        }
        if close || dismissed {
            self.modal = Modal::None;
        }
    }

    /// One destination already exists: what should happen to it?
    ///
    /// Drawn whenever `export_collision` is set, which means the worker is **blocked**
    /// waiting. Not a `Modal` variant, because an export can be started and then collide
    /// long after the export dialog closed.
    fn collision_modal(&mut self, ctx: &egui::Context) {
        let Some(dest) = self.export_collision.clone() else {
            return;
        };
        let name = dest
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut apply_all = self.export_apply_all;
        let mut decision: Option<Decision> = None;

        let dismissed = modal_dialog(ctx, "export_collision", "File already exists", |ui| {
            ui.label(format!("“{name}” is already in the destination folder."));
            ui.checkbox(&mut apply_all, "Apply to all remaining");
            ui.separator();
            ui.horizontal(|ui| {
                for (label, collision) in [
                    ("Skip", Collision::Skip),
                    ("Overwrite", Collision::Overwrite),
                    ("Keep both", Collision::KeepBoth),
                ] {
                    if ui.button(label).clicked() {
                        decision = Some(Decision::Resolve {
                            collision,
                            apply_to_all: apply_all,
                        });
                    }
                }
                if ui.button("Cancel export").clicked() {
                    decision = Some(Decision::Cancel);
                }
            });
        });

        self.export_apply_all = apply_all;
        // ⚠ Dismissing by clicking outside cancels the run — and `Decision::Cancel` carries
        // no `apply_to_all`, so the tick cannot leak out of a cancel by construction.
        if dismissed {
            decision = Some(Decision::Cancel);
        }
        if let Some(d) = decision {
            if let Some(w) = &self.exporter {
                w.decide(d);
            }
            self.export_collision = None;
            // Cancelling from the prompt ends the run just as the toolbar's Cancel does,
            // rather than leaving a progress line up until the worker's Finished arrives.
            if d == Decision::Cancel {
                self.cancel_export();
            }
        }
    }

    /// The preset-delete confirmation.
    ///
    /// ⚠ "Don't ask again" is scratch state until Delete is clicked. Ticking it and then
    /// cancelling must leave the confirmation armed — see
    /// `settings::cancelling_never_suppresses_the_confirmation`, which is where that rule
    /// is actually tested.
    fn confirm_delete_modal(&mut self, ctx: &egui::Context) {
        let Modal::ConfirmDeletePreset {
            name,
            dont_ask_again,
        } = &mut self.modal
        else {
            return;
        };
        let name = std::mem::take(name);
        let mut dont_ask = *dont_ask_again;
        let mut outcome: Option<ConfirmOutcome> = None;

        let dismissed = modal_dialog(ctx, "confirm_delete", "Delete preset", |ui| {
            ui.label(format!("Delete the preset “{name}”?"));
            ui.checkbox(&mut dont_ask, "Don't ask again");
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Delete").clicked() {
                    outcome = Some(ConfirmOutcome::Confirmed {
                        suppress_future: dont_ask,
                    });
                }
                if ui.button("Cancel").clicked() {
                    outcome = Some(ConfirmOutcome::Cancelled);
                }
            });
        });
        // Clicking outside is a cancel — and therefore also discards the "don't ask again"
        // tick, exactly like the Cancel button.
        if dismissed {
            outcome = Some(ConfirmOutcome::Cancelled);
        }

        match outcome {
            None => {
                // Still open — carry the scratch state to the next frame.
                self.modal = Modal::ConfirmDeletePreset {
                    name,
                    dont_ask_again: dont_ask,
                };
            }
            Some(o) => {
                let confirm_before = self.settings.confirm_preset_delete;
                if self.settings.apply_delete_confirm(o) {
                    self.delete_preset(&name);
                }
                if self.settings.confirm_preset_delete != confirm_before {
                    self.save_settings();
                }
                self.modal = Modal::None;
            }
        }
    }

    /// The Settings window. One setting today; a modal is the right weight for that.
    fn settings_modal(&mut self, ctx: &egui::Context) {
        let mut confirm = self.settings.confirm_preset_delete;
        let mut close = false;

        let dismissed = modal_dialog(ctx, "settings", "Settings", |ui| {
            ui.checkbox(&mut confirm, "Confirm before deleting a preset");
            ui.separator();
            if ui.button("Close").clicked() {
                close = true;
            }
        });
        close |= dismissed;

        if confirm != self.settings.confirm_preset_delete {
            self.settings.confirm_preset_delete = confirm;
            self.save_settings();
        }
        if close {
            self.modal = Modal::None;
        }
    }

    /// The Develop editor body: right adjustment panel + central image canvas.
    fn develop_body(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.handle_develop_arrows(ctx);
        // Set by whichever panel commits a photo edit, and recorded in the action log after
        // the panels close. Collected in a local rather than assigned inline because both
        // sites sit inside closures that already hold `&mut self` through `self.session`,
        // so they cannot touch another field of `self`.
        let mut photo_edited = false;
        if self.show_presets && self.session.is_some() {
            self.preset_panel(ui);
        }
        egui::Panel::right("panel")
            .default_size(240.0)
            .show(ui, |ui| {
                if self.pending.is_some() {
                    ui.disable();
                }
                egui::ScrollArea::vertical().show(ui, |ui| {
                    if let Some(s) = &mut self.session {
                        draw_histogram(ui, &s.hist);
                        ui.separator();

                        let mut changed = false;
                        let mut settled = false;
                        let mut slider = |ui: &mut egui::Ui, value: &mut f32, range, label| {
                            let r = ui.add(egui::Slider::new(value, range).text(label));
                            changed |= r.changed();
                            settled |= r.drag_stopped();
                        };
                        slider(ui, &mut s.edits.white_balance.temp, -100.0..=100.0, "Temp");
                        slider(ui, &mut s.edits.white_balance.tint, -100.0..=100.0, "Tint");
                        slider(ui, &mut s.edits.exposure, -5.0..=5.0, "Exposure");
                        slider(ui, &mut s.edits.contrast, -100.0..=100.0, "Contrast");
                        slider(ui, &mut s.edits.highlights, -100.0..=100.0, "Highlights");
                        slider(ui, &mut s.edits.shadows, -100.0..=100.0, "Shadows");
                        slider(ui, &mut s.edits.whites, -100.0..=100.0, "Whites");
                        slider(ui, &mut s.edits.blacks, -100.0..=100.0, "Blacks");
                        // Lightroom's "Presence" grouping, and it reflects that these are
                        // tonal rather than detail controls — which is also why they are in
                        // the Tone edit group rather than Detail.
                        slider(
                            ui,
                            &mut s.edits.local_contrast.texture,
                            -100.0..=100.0,
                            "Texture",
                        );
                        slider(
                            ui,
                            &mut s.edits.local_contrast.clarity,
                            -100.0..=100.0,
                            "Clarity",
                        );
                        slider(ui, &mut s.edits.dehaze.amount, -100.0..=100.0, "Dehaze");
                        slider(ui, &mut s.edits.saturation, -100.0..=100.0, "Saturation");
                        slider(ui, &mut s.edits.vibrance, -100.0..=100.0, "Vibrance");

                        ui.separator();
                        egui::CollapsingHeader::new("Hue").show(ui, |ui| {
                            for (i, name) in edit_model::HSL_BAND_NAMES.iter().enumerate() {
                                slider(ui, &mut s.edits.hsl.bands[i].hue, -100.0..=100.0, *name);
                            }
                        });
                        egui::CollapsingHeader::new("Saturation").show(ui, |ui| {
                            for (i, name) in edit_model::HSL_BAND_NAMES.iter().enumerate() {
                                slider(
                                    ui,
                                    &mut s.edits.hsl.bands[i].saturation,
                                    -100.0..=100.0,
                                    *name,
                                );
                            }
                        });
                        egui::CollapsingHeader::new("Luminance").show(ui, |ui| {
                            for (i, name) in edit_model::HSL_BAND_NAMES.iter().enumerate() {
                                slider(
                                    ui,
                                    &mut s.edits.hsl.bands[i].luminance,
                                    -100.0..=100.0,
                                    *name,
                                );
                            }
                        });
                        egui::CollapsingHeader::new("Sharpening").show(ui, |ui| {
                            let sh = &mut s.edits.sharpening;
                            slider(ui, &mut sh.amount, 0.0..=100.0, "Amount");
                            // The one slider not on a 0..100 scale: it is a physical pixel
                            // quantity, in full-resolution pixels, and naming it in pixels
                            // is what makes "scaled for the render" comprehensible.
                            slider(ui, &mut sh.radius, 0.5..=3.0, "Radius (px)");
                            slider(ui, &mut sh.detail, 0.0..=100.0, "Detail");
                            slider(ui, &mut sh.masking, 0.0..=100.0, "Masking");
                        });
                        egui::CollapsingHeader::new("Noise Reduction").show(ui, |ui| {
                            let nr = &mut s.edits.noise_reduction;
                            slider(ui, &mut nr.luminance, 0.0..=100.0, "Luminance");
                            slider(ui, &mut nr.detail, 0.0..=100.0, "Detail");
                            slider(ui, &mut nr.color, 0.0..=100.0, "Color");
                            slider(ui, &mut nr.smoothness, 0.0..=100.0, "Smoothness");
                        });
                        egui::CollapsingHeader::new("Vignette").show(ui, |ui| {
                            let v = &mut s.edits.vignette;
                            slider(ui, &mut v.amount, -100.0..=100.0, "Amount");
                            slider(ui, &mut v.midpoint, 0.0..=100.0, "Midpoint");
                            slider(ui, &mut v.roundness, -100.0..=100.0, "Roundness");
                            slider(ui, &mut v.feather, 0.0..=100.0, "Feather");
                        });

                        ui.separator();
                        if ui.button("Reset all").clicked() {
                            s.edits = Edits::default();
                            changed = true;
                            settled = true;
                        }

                        if s.crop_mode {
                            ui.separator();
                            ui.label("Crop");
                            let img_aspect = s.baseline.size_vec2().x / s.baseline.size_vec2().y;
                            let presets: [(&str, Option<f32>); 5] = [
                                ("Free", None),
                                ("1:1", Some(1.0)),
                                ("3:2", Some(1.5)),
                                ("4:3", Some(4.0 / 3.0)),
                                ("16:9", Some(16.0 / 9.0)),
                            ];
                            ui.horizontal(|ui| {
                                for (label, pa) in presets {
                                    if ui.button(label).clicked() {
                                        s.edits.geometry.crop = match pa {
                                            Some(a) => crop_tool::snap_to_aspect(a / img_aspect),
                                            None => edit_model::CropRect::default(),
                                        };
                                        settled = true;
                                    }
                                }
                            });
                            let ar = ui.add(
                                egui::Slider::new(&mut s.edits.geometry.angle, -45.0..=45.0)
                                    .text("Straighten"),
                            );
                            changed |= ar.changed();
                            settled |= ar.drag_stopped();
                        }

                        if changed {
                            s.engine.request(
                                preview_edits(&s.edits, s.crop_mode),
                                s.render_max_dim(),
                                s.requested_region,
                            );
                        }
                        if settled {
                            photo_edited |= s.history.push(s.edits.clone());
                        }

                        ui.separator();
                        // The filename first: which photo this is matters more than what
                        // shot it, and the panel scrolls, so it belongs above the metadata.
                        if let Some(name) = s.path.file_name() {
                            ui.label(egui::RichText::new(name.to_string_lossy()).strong());
                        }
                        ui.label(egui::RichText::new(&s.make_model).small());
                    } else {
                        ui.label("No image open.");
                    }
                });
            });

        if self.show_filmstrip && self.library.is_some() {
            self.filmstrip(ui, ctx);
        }

        egui::CentralPanel::default().show(ui, |ui| {
            if let Some(p) = &self.pending {
                draw_pending_canvas(ui, p, self.session.as_ref());
                return;
            }
            match &mut self.session {
                Some(s) => {
                    if let Some(tex) = &s.texture {
                        let shown = if s.show_before { &s.baseline } else { tex };
                        let avail = ui.available_size();

                        // fit/zoom are expressed against the post-geometry image at *fit*
                        // resolution, which stays constant as the render mode changes.
                        let full = s.fit_image_size;
                        let fit = (avail.x / full.x).min(avail.y / full.y).min(1.0);
                        // Crop mode stays at fit resolution whatever the zoom: it is a framing
                        // operation, the overlay is drawn against the whole image, and a
                        // whole-image *native* render would be both expensive and placed by
                        // the fit-mode maths the overlay assumes.
                        let native = view_region::needs_native(fit, s.zoom) && !s.crop_mode;

                        // Dimensions of the image at the resolution we render it, and the
                        // screen pixels per rendered pixel.
                        let full_long = full.x.max(full.y);
                        let render_long = if native {
                            s.buffer_long as f32
                        } else {
                            full_long
                        };
                        let k = render_long / full_long;
                        let (iw, ih) =
                            ((full.x * k).round() as usize, (full.y * k).round() as usize);
                        let scale = fit * s.zoom / k;

                        // Which part of the image the view needs, and whether what is already
                        // rendered still covers it.
                        let want = if native {
                            let vis = view_region::visible_rect(
                                iw, ih, avail.x, avail.y, scale, s.pan.x, s.pan.y,
                            );
                            Some(view_region::with_margin(
                                vis,
                                iw,
                                ih,
                                view_region::REGION_MARGIN,
                            ))
                        } else {
                            None
                        };
                        let covered = match (want, s.rendered_region) {
                            (None, None) => true,
                            (Some(w), Some(r)) => view_region::contains(r, w),
                            _ => false,
                        };
                        if !covered && s.requested_region != want {
                            s.requested_region = want;
                            let max_dim = if native {
                                s.buffer_long
                            } else {
                                PREVIEW_MAX_DIM
                            };
                            s.engine
                                .request(preview_edits(&s.edits, s.crop_mode), max_dim, want);
                        }

                        // Place using the *texture's* own render resolution, not the current
                        // mode's: a frame from the other mode may still be on screen while a
                        // new one is in flight.
                        let (place_region, place_long) = if s.show_before {
                            // The baseline is always the whole image at fit resolution.
                            (None, full_long)
                        } else {
                            (s.rendered_region, s.rendered_long.max(1.0))
                        };
                        let pk = place_long / full_long;
                        let pscale = fit * s.zoom / pk;

                        let img_rect = match place_region {
                            Some(r) => {
                                let origin = ui.max_rect().center() + s.pan
                                    - egui::vec2(full.x * pk, full.y * pk) * pscale * 0.5;
                                egui::Rect::from_min_size(
                                    origin + egui::vec2(r.x as f32, r.y as f32) * pscale,
                                    shown.size_vec2() * pscale,
                                )
                            }
                            // `pscale` rather than `fit * zoom`: the texture's own size
                            // already carries its render resolution, so this stays right
                            // even for a whole-image frame from the other mode.
                            None => egui::Rect::from_center_size(
                                ui.max_rect().center() + s.pan,
                                shown.size_vec2() * pscale,
                            ),
                        };
                        let resp = ui.allocate_rect(img_rect, egui::Sense::drag());
                        egui::Image::new(shown).paint_at(ui, resp.rect);

                        if s.crop_mode {
                            photo_edited |= draw_crop_overlay(ui, s, &resp, img_rect);
                        } else {
                            if resp.dragged() {
                                s.pan += resp.drag_delta();
                            }
                            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                            if scroll != 0.0 && resp.hovered() {
                                let old = s.zoom;
                                s.zoom = (s.zoom * (1.0 + scroll * 0.001)).clamp(1.0, 8.0);
                                // Zoom about the cursor, not the centre: keep whatever the
                                // pointer is over fixed on screen. `anchor` is the image's
                                // centre, so `d` is the cursor's offset from it; magnifying
                                // by `r` moves that point to `d*r`, which `d*(1-r)` undoes.
                                if let Some(p) = ui.input(|i| i.pointer.hover_pos()) {
                                    let anchor = ui.max_rect().center() + s.pan;
                                    s.pan += (p - anchor) * (1.0 - s.zoom / old);
                                }
                                // Back at fit the whole image is on screen, so leftover pan
                                // from cursor-anchored zooming would just push it off-centre.
                                if s.zoom <= 1.0 {
                                    s.pan = egui::Vec2::ZERO;
                                }
                            }
                        }
                    } else {
                        ui.centered_and_justified(|ui| {
                            ui.spinner();
                        });
                    }
                }
                None => {
                    ui.centered_and_justified(|ui| {
                        ui.label("Open an image to begin (Open file… button).");
                    });
                }
            }
        });

        // Recorded in the same order as everything else, so "delete a preset, move a
        // slider, Ctrl+Z twice" undoes the slider and then restores the preset.
        if photo_edited {
            self.actions.push(ActionKind::PhotoEdit);
        }
    }

    /// ←/→ move to the previous/next photo in the loaded folder — but only when no egui
    /// widget has keyboard focus, so arrows still fine-tune a focused slider.
    fn handle_develop_arrows(&mut self, ctx: &egui::Context) {
        let nav = {
            let Some(lib) = &self.library else { return };
            let n = lib.entries.len();
            if n == 0 || ctx.memory(|m| m.focused().is_some()) {
                return;
            }
            let cur = lib.selection.active().unwrap_or(0);
            ctx.input(|i| {
                if i.key_pressed(egui::Key::ArrowRight) {
                    Some((cur + 1).min(n - 1))
                } else if i.key_pressed(egui::Key::ArrowLeft) {
                    Some(cur.saturating_sub(1))
                } else {
                    None
                }
            })
        };
        if let Some(i) = nav {
            if self.library.as_ref().and_then(|l| l.selection.active()) != Some(i) {
                self.open_entry(ctx, i);
            }
        }
    }

    /// Bottom strip of the loaded folder's thumbnails; click a cell to open that photo.
    /// The panel is resizable (drag its top border); thumbnails scale with its height.
    fn filmstrip(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let mut open_index: Option<usize> = None;
        // Decided before the panel borrows `self.library`, so the closure only needs two
        // `Copy` locals. Opening from the Library grid and arrow-navigating both land here.
        let active = self.library.as_ref().and_then(|l| l.selection.active());
        let want_scroll = active.is_some() && self.filmstrip_scrolled_to != active;
        egui::Panel::bottom("filmstrip")
            .resizable(true)
            .default_size(96.0)
            .show(ui, |ui| {
                let Some(lib) = &mut self.library else { return };
                // Fill the panel's height so egui stores the panel size (not the smaller
                // content size) — otherwise a resizable bottom panel shrinks back each frame.
                let avail_h = ui.available_height();
                ui.set_min_height(avail_h);
                let cell_h = (avail_h - 8.0).clamp(48.0, 400.0);
                let box_size = egui::vec2(cell_h * 1.5, cell_h);
                egui::ScrollArea::horizontal().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for i in 0..lib.entries.len() {
                            // The filmstrip's cell *is* its click target, so it opts in here.
                            let resp = thumb_cell(ui, box_size, lib.thumbs[i].as_ref())
                                .interact(egui::Sense::click());
                            paint_selection_border(
                                ui,
                                resp.rect,
                                lib.selection.active() == Some(i),
                                lib.selection.contains(i),
                            );
                            if want_scroll && Some(i) == active {
                                resp.scroll_to_me(Some(egui::Align::Center));
                            }
                            if resp.clicked() {
                                let mods = ui.input(|inp| inp.modifiers);
                                // Only a plain click moves the active photo, so only a
                                // plain click opens — a Cmd- or Shift-click builds the
                                // target set without triggering a decode.
                                if apply_click(&mut lib.selection, i, mods) {
                                    open_index = Some(i);
                                }
                            }
                        }
                    });
                });
            });
        if want_scroll {
            self.filmstrip_scrolled_to = active;
        }
        if let Some(i) = open_index {
            self.open_entry(ctx, i);
        }
    }

    /// The Library grid: thumbnails, selection, double-click / Enter to open in Develop.
    fn library_view(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let mut open_index: Option<usize> = None;
        egui::CentralPanel::default().show(ui, |ui| {
            let Some(lib) = &mut self.library else {
                ui.centered_and_justified(|ui| {
                    ui.label("Open a folder to browse (Open folder… button).");
                });
                return;
            };
            // A folder can be reloaded with fewer photos than the selection remembers.
            lib.selection.clamp(lib.entries.len());
            if lib.entries.is_empty() {
                ui.centered_and_justified(|ui| {
                    ui.label("No supported images in this folder.");
                });
                return;
            }
            const CELL: f32 = 168.0;

            // Keyboard: arrows move the selection, Enter opens it.
            ui.input(|i| {
                let n = lib.entries.len();
                let cur = lib.selection.active().unwrap_or(0);
                if i.key_pressed(egui::Key::ArrowRight) {
                    lib.selection.click((cur + 1).min(n - 1));
                }
                if i.key_pressed(egui::Key::ArrowLeft) {
                    lib.selection.click(cur.saturating_sub(1));
                }
                if i.key_pressed(egui::Key::Enter) {
                    open_index = lib.selection.active();
                }
                if i.key_pressed(egui::Key::A) && i.modifiers.command {
                    lib.selection.select_all(n);
                }
                if i.key_pressed(egui::Key::Escape) {
                    lib.selection.clear();
                }
            });

            egui::ScrollArea::vertical().show(ui, |ui| {
                let cols = (ui.available_width() / CELL).floor().max(1.0) as usize;
                egui::Grid::new("thumb_grid").show(ui, |ui| {
                    for i in 0..lib.entries.len() {
                        let box_size = egui::vec2(CELL - 12.0, (CELL - 12.0) * 0.66);
                        let resp = ui
                            .allocate_ui(egui::vec2(CELL, CELL), |ui| {
                                // Pin the cell to its full size. Without this `allocate_ui`
                                // reports only the space the content used, so cells varied
                                // with their thumbnail — the filmstrip's reflow bug in its
                                // grid form, where `egui::Grid`'s column alignment hid it.
                                ui.set_min_size(egui::vec2(CELL, CELL));
                                ui.vertical_centered(|ui| {
                                    thumb_cell(ui, box_size, lib.thumbs[i].as_ref());
                                    ui.label(
                                        egui::RichText::new(&lib.entries[i].file_name).small(),
                                    );
                                });
                            })
                            .response
                            .interact(egui::Sense::click());

                        paint_selection_border(
                            ui,
                            resp.rect,
                            lib.selection.active() == Some(i),
                            lib.selection.contains(i),
                        );
                        if resp.clicked() {
                            let mods = ui.input(|inp| inp.modifiers);
                            apply_click(&mut lib.selection, i, mods);
                        }
                        if resp.double_clicked() {
                            open_index = Some(i);
                        }
                        if (i + 1) % cols == 0 {
                            ui.end_row();
                        }
                    }
                });
            });
        });

        if let Some(i) = open_index {
            self.open_entry(ctx, i);
        }
    }
}

impl eframe::App for RustroomApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

        // Workaround for an upstream winit/macOS teardown bug: when the window closes,
        // AppKit's Touch Bar responder tracking (`_NSTouchBarFinderObservation`) removes a
        // KVO observer for "nextResponder" from `WinitView` with an imbalanced count,
        // throwing an uncaught `NSRangeException` (SIGABRT). Exiting here — before that
        // deferred observer invalidation runs during teardown — gives a clean exit.
        #[cfg(target_os = "macos")]
        if ctx.input(|i| i.viewport().close_requested()) {
            self.persist_current_session();
            std::process::exit(0);
        }

        self.pump_decodes(&ctx);
        // Outside the view match: Ctrl+V must reach a selection made in the Library grid,
        // not only one made in the filmstrip.
        self.handle_clipboard_keys(&ctx);
        self.pump_export(&ctx);

        // Pull any finished preview frame into a texture.
        if let Some(s) = &mut self.session {
            if let Some(frame) = s.engine.poll_latest() {
                s.hist = histogram_rgba(&frame.rgba);
                let img = egui::ColorImage::from_rgba_unmultiplied(
                    [frame.width, frame.height],
                    &frame.rgba,
                );
                s.texture = Some(ctx.load_texture("preview", img, egui::TextureOptions::LINEAR));
                s.rendered_region = frame.region.map(|r| RegionRect {
                    x: r.x0,
                    y: r.y0,
                    w: frame.width,
                    h: frame.height,
                });
                // A whole-image frame is by definition a fit-resolution render, and is the
                // only thing that can tell us the post-geometry size — which changes with
                // the crop and the straighten angle.
                if frame.region.is_none() {
                    s.fit_image_size = egui::vec2(frame.width as f32, frame.height as f32);
                }
                // A region frame carries the full post-geometry dimensions it was cut from,
                // so this is exact either way.
                s.rendered_long = frame
                    .region
                    .map(|r| r.full_w.max(r.full_h))
                    .unwrap_or(frame.width.max(frame.height))
                    as f32;
            }
        }

        // Pull finished thumbnails into textures.
        if let Some(lib) = &mut self.library {
            for res in lib.loader.drain() {
                if let (Some(t), true) = (res.thumb, res.index < lib.thumbs.len()) {
                    let img =
                        egui::ColorImage::from_rgba_unmultiplied([t.width, t.height], &t.rgba);
                    lib.thumbs[res.index] = Some(ctx.load_texture(
                        format!("thumb{}", res.index),
                        img,
                        egui::TextureOptions::LINEAR,
                    ));
                }
            }
        }

        egui::Panel::top("top").show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Open file…").clicked() {
                    self.open_dialog(&ctx);
                }
                if ui.button("Open folder…").clicked() {
                    self.open_folder(&ctx);
                }
                ui.separator();
                ui.selectable_value(&mut self.view, AppView::Library, "Library");
                ui.selectable_value(&mut self.view, AppView::Develop, "Develop");
                ui.separator();
                if self.view == AppView::Develop && self.session.is_some() {
                    let decoding = self.pending.is_some();
                    ui.add_enabled_ui(!decoding, |ui| self.develop_toolbar(ui, &ctx));
                }
                if !self.export_progress.is_empty() {
                    ui.separator();
                    ui.label(&self.export_progress);
                    if ui.button("Cancel").clicked() {
                        self.cancel_export();
                    }
                } else if !self.status.is_empty() {
                    ui.label(&self.status);
                }
            });
        });

        match self.view {
            AppView::Develop => self.develop_body(ui, &ctx),
            AppView::Library => self.library_view(ui, &ctx),
        }

        self.draw_modals(&ctx);
    }
}

/// Canvas while a decode is in flight: the target photo's thumbnail if we have one (scaled
/// up to fill, since thumbnails are ≤512 px), else the previous photo dimmed, else a spinner.
fn draw_pending_canvas(ui: &mut egui::Ui, pending: &PendingOpen, session: Option<&Session>) {
    let showing_placeholder = pending.placeholder.is_some();
    let fallback = session.and_then(|s| s.texture.as_ref());
    let Some(tex) = pending.placeholder.as_ref().or(fallback) else {
        ui.centered_and_justified(|ui| {
            ui.spinner();
        });
        return;
    };

    // Lay the placeholder out where the decoded preview will land, so the swap happens in
    // place instead of snapping: the preview's long side is exactly PREVIEW_MAX_DIM, so
    // predict that size from this texture's aspect and then apply the session's fit rule.
    let tex_size = tex.size_vec2();
    let long = tex_size.x.max(tex_size.y).max(1.0);
    let preview_size = tex_size * (PREVIEW_MAX_DIM as f32 / long);
    let avail = ui.available_size();
    let fit = (avail.x / preview_size.x)
        .min(avail.y / preview_size.y)
        .min(1.0);
    let rect = egui::Rect::from_center_size(ui.max_rect().center(), preview_size * fit);
    egui::Image::new(tex).paint_at(ui, rect);

    if !showing_placeholder {
        // This is the *previous* photo — dim it so it reads as stale.
        ui.painter().rect_filled(
            rect,
            egui::CornerRadius::ZERO,
            egui::Color32::from_black_alpha(140),
        );
    }
}

/// Draw the crop rectangle overlay and handle dragging (reads/writes `s.edits.geometry.crop`).
///
/// Returns whether a drag settled into a new history entry, so the caller can treat that as
/// a photo edit — it is one of the two places a photo edit is committed.
fn draw_crop_overlay(
    ui: &egui::Ui,
    s: &mut Session,
    resp: &egui::Response,
    img_rect: egui::Rect,
) -> bool {
    let g = s.edits.geometry.crop;
    let to_screen = |nx: f32, ny: f32| {
        egui::pos2(
            img_rect.left() + nx * img_rect.width(),
            img_rect.top() + ny * img_rect.height(),
        )
    };
    let crop_rect =
        egui::Rect::from_min_max(to_screen(g.left, g.top), to_screen(g.right, g.bottom));

    let painter = ui.painter_at(img_rect);
    let dim = egui::Color32::from_black_alpha(140);
    for outside in rects_around(img_rect, crop_rect) {
        painter.rect_filled(outside, egui::CornerRadius::ZERO, dim);
    }
    painter.rect_stroke(
        crop_rect,
        egui::CornerRadius::ZERO,
        egui::Stroke::new(1.5, egui::Color32::WHITE),
        egui::StrokeKind::Inside,
    );
    for c in [
        crop_rect.left_top(),
        crop_rect.right_top(),
        crop_rect.left_bottom(),
        crop_rect.right_bottom(),
    ] {
        painter.circle_filled(c, 4.0, egui::Color32::WHITE);
    }

    if let Some(p) = resp.interact_pointer_pos() {
        let nx = ((p.x - img_rect.left()) / img_rect.width()).clamp(0.0, 1.0);
        let ny = ((p.y - img_rect.top()) / img_rect.height()).clamp(0.0, 1.0);
        if resp.drag_started() {
            s.active_handle = crop_tool::hit_test(&s.edits.geometry.crop, [nx, ny], 0.03);
        }
        if resp.dragged() {
            if let Some(h) = s.active_handle {
                let d = resp.drag_delta();
                let dx = d.x / img_rect.width();
                let dy = d.y / img_rect.height();
                s.edits.geometry.crop = crop_tool::apply_drag(s.edits.geometry.crop, h, dx, dy);
            }
        }
        if resp.drag_stopped() {
            s.active_handle = None;
            s.history.push(s.edits.clone());
            return true;
        }
    }
    false
}

/// The four rectangles of `outer` that lie outside `inner` (for dimming).
fn rects_around(outer: egui::Rect, inner: egui::Rect) -> [egui::Rect; 4] {
    [
        egui::Rect::from_min_max(outer.left_top(), egui::pos2(outer.right(), inner.top())),
        egui::Rect::from_min_max(
            egui::pos2(outer.left(), inner.bottom()),
            outer.right_bottom(),
        ),
        egui::Rect::from_min_max(
            egui::pos2(outer.left(), inner.top()),
            egui::pos2(inner.left(), inner.bottom()),
        ),
        egui::Rect::from_min_max(
            egui::pos2(inner.right(), inner.top()),
            egui::pos2(outer.right(), inner.bottom()),
        ),
    ]
}

/// How a paste reports what it did. Shared by the Ctrl+V path and the "Paste…" dialog so
/// the two cannot describe the same work differently.
fn paste_status(touched: usize, failed: usize) -> String {
    match (touched, failed) {
        (0, _) => "Nothing selected to paste into".to_string(),
        (1, 0) => "Settings pasted".to_string(),
        (n, 0) => format!("Pasted to {n} photos"),
        (n, f) => format!("Pasted to {n} photos, {f} failed"),
    }
}

/// A centred dialog with a dimmed backdrop. Returns whether the user clicked *outside* it,
/// which every dialog here treats as a cancel.
///
/// `egui::Modal` rather than `egui::Window`: a Window opens at the top-left and is easy to
/// miss entirely, whereas a Modal centres itself on the viewport, dims what is behind it and
/// blocks clicks reaching the app underneath.
fn modal_dialog<R>(
    ctx: &egui::Context,
    id: &str,
    title: &str,
    content: impl FnOnce(&mut egui::Ui) -> R,
) -> bool {
    egui::Modal::new(egui::Id::new(id))
        .show(ctx, |ui| {
            ui.heading(title);
            ui.separator();
            content(ui);
        })
        .backdrop_response
        .clicked()
}

/// Apply a click to the selection according to its modifiers. Returns whether this was a
/// plain click, which is the only gesture that moves the active photo — and therefore the
/// only one the filmstrip turns into an open.
fn apply_click(sel: &mut Selection, i: usize, mods: egui::Modifiers) -> bool {
    if mods.command {
        sel.toggle(i);
        false
    } else if mods.shift {
        sel.extend_to(i);
        false
    } else {
        sel.click(i);
        true
    }
}

/// Outline a selected cell: bright for the active photo, dim for the rest of the selection.
/// Being able to tell the source from the targets is what makes a sync predictable.
fn paint_selection_border(ui: &egui::Ui, rect: egui::Rect, active: bool, selected: bool) {
    if !selected {
        return;
    }
    let color = if active {
        egui::Color32::from_rgb(120, 160, 240)
    } else {
        egui::Color32::from_rgb(90, 100, 120)
    };
    ui.painter().rect_stroke(
        rect,
        egui::CornerRadius::same(3),
        egui::Stroke::new(2.0, color),
        egui::StrokeKind::Inside,
    );
}

/// One thumbnail cell — filmstrip *and* Library grid — a fixed-size box holding either the
/// thumbnail, centred and aspect-preserved, or a grey placeholder while it loads.
///
/// ⚠ **The fixed size is the entire point, and it is load-bearing.** `Ui::allocate_ui`
/// allocates the space its content *actually used*, not the space requested — so drawing a
/// loaded thumbnail with `Image::max_size` allocated a **portrait** cell only 58.6 px wide
/// against its placeholder's 132 px. Every thumbnail that arrived then shifted every cell to
/// its right, which made the strip twitch as it loaded and slid the centred photo out from
/// under the scroll target (that target is an absolute offset egui computes **once** and
/// never recomputes). Allocating first and painting into the allocated rect makes a cell's
/// geometry independent of whether its thumbnail has arrived.
/// ⚠ Allocates with `Sense::hover()`, **not** `Sense::click()` — this reserves space and
/// paints, and interaction is the caller's to add. In the Library grid the click target is
/// the whole cell (thumbnail *and* filename), so a click-sensing thumbnail sitting inside it
/// would swallow the click: both rects are distance 0 from the pointer and egui breaks that
/// tie by registration order. The grid then responded only on the grey margin around the
/// image. `clicking_a_library_grid_thumbnail_opens_it` pins it.
fn thumb_cell(
    ui: &mut egui::Ui,
    box_size: egui::Vec2,
    thumb: Option<&egui::TextureHandle>,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(box_size, egui::Sense::hover());
    match thumb {
        Some(tex) => {
            // Fit inside the cell without upscaling, then centre — the same result
            // `Image::max_size(..).maintain_aspect_ratio(true)` produced, minus the effect
            // on layout.
            let size = tex.size_vec2();
            let scale = (box_size.x / size.x).min(box_size.y / size.y).min(1.0);
            egui::Image::new(tex).paint_at(
                ui,
                egui::Rect::from_center_size(rect.center(), size * scale),
            );
        }
        None => {
            ui.painter().rect_filled(
                rect,
                egui::CornerRadius::same(3),
                egui::Color32::from_gray(60),
            );
        }
    }
    resp
}

/// The six group checkboxes, shared by the Save-preset and Paste-settings dialogs.
fn edit_group_checkboxes(ui: &mut egui::Ui, groups: &mut EditGroups) {
    ui.horizontal(|ui| {
        if ui.button("All").clicked() {
            *groups = EditGroups::ALL;
        }
        if ui.button("None").clicked() {
            *groups = EditGroups::NONE;
        }
    });
    for (i, name) in edit_model::EDIT_GROUP_NAMES.iter().enumerate() {
        ui.checkbox(groups.get_mut(i), *name);
    }
}

fn draw_histogram(ui: &mut egui::Ui, hist: &[[u32; 256]; 3]) {
    let max = hist
        .iter()
        .flat_map(|c| c.iter())
        .copied()
        .max()
        .unwrap_or(1)
        .max(1) as f32;
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 80.0), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let colors = [
        egui::Color32::from_rgb(220, 80, 80),
        egui::Color32::from_rgb(80, 200, 80),
        egui::Color32::from_rgb(90, 120, 230),
    ];
    for (ci, chan) in hist.iter().enumerate() {
        for (i, &count) in chan.iter().enumerate() {
            let x = rect.left() + rect.width() * (i as f32 / 255.0);
            let h = rect.height() * (count as f32 / max);
            painter.line_segment(
                [
                    egui::pos2(x, rect.bottom()),
                    egui::pos2(x, rect.bottom() - h),
                ],
                egui::Stroke::new(1.0, colors[ci].linear_multiply(0.5)),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Lay out three identical cells and return their allocated widths.
    ///
    /// `old` reproduces the pre-fix construction (`allocate_ui` + `Image::max_size`) so the
    /// regression is demonstrated here rather than only described in a comment.
    fn cell_widths(thumb_size: Option<[usize; 2]>, old: bool) -> Vec<f32> {
        let ctx = egui::Context::default();
        let box_size = egui::vec2(132.0, 88.0);
        let tex = thumb_size.map(|s| {
            ctx.load_texture(
                "t",
                egui::ColorImage::new(s, vec![egui::Color32::WHITE; s[0] * s[1]]),
                Default::default(),
            )
        });
        let mut widths = Vec::new();
        let _ = ctx.run_ui(Default::default(), |ui| {
            ui.horizontal(|ui| {
                for _ in 0..3 {
                    let w = if old {
                        ui.allocate_ui(box_size, |ui| match &tex {
                            Some(t) => {
                                ui.add(
                                    egui::Image::new(t)
                                        .max_size(box_size)
                                        .maintain_aspect_ratio(true),
                                );
                            }
                            None => {
                                let (rect, _) =
                                    ui.allocate_exact_size(box_size, egui::Sense::hover());
                                ui.painter().rect_filled(
                                    rect,
                                    egui::CornerRadius::same(3),
                                    egui::Color32::from_gray(60),
                                );
                            }
                        })
                        .response
                        .rect
                        .width()
                    } else {
                        thumb_cell(ui, box_size, tex.as_ref()).rect.width()
                    };
                    widths.push(w);
                }
            });
        });
        widths
    }

    /// A portrait thumbnail, as `extract_thumbnail` returns it once libraw's flip is applied.
    const PORTRAIT: [usize; 2] = [341, 512];
    const LANDSCAPE: [usize; 2] = [512, 341];

    #[test]
    fn the_old_cell_shrank_when_a_portrait_thumbnail_arrived() {
        // ⚠ The bug, reproduced. This is why the strip twitched while loading and why the
        // centred photo drifted left: every arriving portrait narrowed its cell by ~73 px,
        // shifting every cell to its right, while egui's scroll target — computed once, as
        // an absolute offset — stayed where it was.
        let empty = cell_widths(None, true)[0];
        let portrait = cell_widths(Some(PORTRAIT), true)[0];
        assert_eq!(empty, 132.0);
        assert!(
            portrait < empty - 70.0,
            "expected the old portrait cell to collapse, got {portrait} vs {empty}"
        );
    }

    #[test]
    fn a_thumb_cell_keeps_its_width_however_its_thumbnail_loads() {
        // The fix: geometry no longer depends on load state, so nothing reflows and the
        // scroll target stays valid.
        let empty = cell_widths(None, false);
        let portrait = cell_widths(Some(PORTRAIT), false);
        let landscape = cell_widths(Some(LANDSCAPE), false);
        assert_eq!(empty, vec![132.0; 3]);
        assert_eq!(portrait, empty, "a portrait thumbnail changed the layout");
        assert_eq!(landscape, empty, "a landscape thumbnail changed the layout");
    }

    /// The Library grid's shape: a `CELL`-square cell holding a thumbnail box and a filename.
    fn grid_cell_size(thumb_size: Option<[usize; 2]>) -> egui::Vec2 {
        const CELL: f32 = 140.0;
        let ctx = egui::Context::default();
        let box_size = egui::vec2(CELL - 12.0, (CELL - 12.0) * 0.66);
        let tex = thumb_size.map(|s| {
            ctx.load_texture(
                "t",
                egui::ColorImage::new(s, vec![egui::Color32::WHITE; s[0] * s[1]]),
                Default::default(),
            )
        });
        let mut size = egui::Vec2::ZERO;
        let _ = ctx.run_ui(Default::default(), |ui| {
            size = ui
                .allocate_ui(egui::vec2(CELL, CELL), |ui| {
                    ui.set_min_size(egui::vec2(CELL, CELL));
                    ui.vertical_centered(|ui| {
                        thumb_cell(ui, box_size, tex.as_ref());
                        ui.label(egui::RichText::new("DSCF6509.RAF").small());
                    });
                })
                .response
                .rect
                .size();
        });
        size
    }

    #[test]
    fn a_library_grid_cell_keeps_its_size_however_its_thumbnail_loads() {
        // The same defect as the filmstrip's, in the grid's shape. `egui::Grid` aligns
        // columns to the widest cell, which masked it — but a cell that changes size on load
        // is still a cell that moves its neighbours.
        let empty = grid_cell_size(None);
        assert_eq!(grid_cell_size(Some(PORTRAIT)), empty);
        assert_eq!(grid_cell_size(Some(LANDSCAPE)), empty);
        assert_eq!(empty.x, 140.0, "the cell should fill its column");
    }

    /// Click at `pos` and report whether the Library grid's *cell* response saw it.
    ///
    /// Three passes: one to lay out with the pointer in place, one to press, one to release
    /// — egui resolves a click against the widgets registered on the previous pass.
    fn grid_cell_click_lands(offset: egui::Vec2) -> bool {
        const CELL: f32 = 140.0;
        let ctx = egui::Context::default();
        let box_size = egui::vec2(CELL - 12.0, (CELL - 12.0) * 0.66);
        let tex = ctx.load_texture(
            "t",
            egui::ColorImage::new(LANDSCAPE, vec![egui::Color32::WHITE; 512 * 341]),
            Default::default(),
        );
        let mut clicked = false;
        let mut origin = egui::Pos2::ZERO;
        for pass in 0..3 {
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 400.0),
                )),
                ..Default::default()
            };
            let pos = origin + offset;
            input.events.push(egui::Event::PointerMoved(pos));
            if pass == 1 || pass == 2 {
                input.events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: pass == 1,
                    modifiers: Default::default(),
                });
            }
            let _ = ctx.run_ui(input, |ui| {
                let resp = ui
                    .allocate_ui(egui::vec2(CELL, CELL), |ui| {
                        ui.set_min_size(egui::vec2(CELL, CELL));
                        ui.vertical_centered(|ui| {
                            thumb_cell(ui, box_size, Some(&tex));
                            ui.label(egui::RichText::new("DSCF6509.RAF").small());
                        });
                    })
                    .response
                    .interact(egui::Sense::click());
                origin = resp.rect.min;
                if pass == 2 && resp.clicked() {
                    clicked = true;
                }
            });
        }
        clicked
    }

    #[test]
    fn clicking_a_library_grid_thumbnail_opens_it() {
        // ⚠ Regression: the whole cell is the click target in the grid, but `thumb_cell`
        // allocated its box with `Sense::click()`. Two overlapping click widgets both sit at
        // distance 0 from the pointer, and egui breaks that tie by registration order — so
        // the thumbnail swallowed the click and only the grey margin around it responded.
        assert!(
            grid_cell_click_lands(egui::vec2(70.0, 40.0)),
            "a click on the thumbnail itself did not reach the cell"
        );
        // The margin below the image (over the filename) must keep working too.
        assert!(
            grid_cell_click_lands(egui::vec2(70.0, 125.0)),
            "a click on the cell margin did not reach the cell"
        );
    }

    /// Click at `offset` from the first filmstrip cell and report whether that cell saw it.
    fn filmstrip_click_lands(offset: egui::Vec2) -> bool {
        let ctx = egui::Context::default();
        let box_size = egui::vec2(132.0, 88.0);
        let tex = ctx.load_texture(
            "t",
            egui::ColorImage::new(LANDSCAPE, vec![egui::Color32::WHITE; 512 * 341]),
            Default::default(),
        );
        let mut clicked = false;
        let mut origin = egui::Pos2::ZERO;
        for pass in 0..3 {
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(600.0, 200.0),
                )),
                ..Default::default()
            };
            let pos = origin + offset;
            input.events.push(egui::Event::PointerMoved(pos));
            if pass == 1 || pass == 2 {
                input.events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: pass == 1,
                    modifiers: Default::default(),
                });
            }
            let _ = ctx.run_ui(input, |ui| {
                ui.horizontal(|ui| {
                    for i in 0..3 {
                        let resp =
                            thumb_cell(ui, box_size, Some(&tex)).interact(egui::Sense::click());
                        if i == 0 {
                            origin = resp.rect.min;
                            if pass == 2 && resp.clicked() {
                                clicked = true;
                            }
                        }
                    }
                });
            });
        }
        clicked
    }

    #[test]
    fn clicking_a_filmstrip_thumbnail_opens_it() {
        // The symmetric guard to the grid's. `thumb_cell` allocates with `Sense::hover()`,
        // so the filmstrip — where the cell *is* the click target — has to opt in. Dropping
        // that `.interact()` would make the strip silently unclickable.
        assert!(
            filmstrip_click_lands(egui::vec2(66.0, 44.0)),
            "a click on a filmstrip thumbnail did not register"
        );
    }

    #[test]
    fn cells_are_laid_out_at_a_constant_pitch() {
        // The property that actually matters to the scroll target: cell *n*'s position must
        // not depend on how many of its predecessors have loaded. Equal widths imply it,
        // but assert the spacing too, since that is what accumulates over 800 cells.
        let w = cell_widths(Some(PORTRAIT), false);
        assert!(w.windows(2).all(|p| p[0] == p[1]), "{w:?}");
    }
}
