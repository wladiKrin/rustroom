use catalog::snapshot::CatalogSnapshot;
use edit_model::EditGroups;
use edit_model::Edits;
use eframe::egui;
use rustroom_app::action_log::{ActionKind, ActionLog, SyncAction};
use rustroom_app::batch_export::{
    ExportEvent, ExportJob, ExportOptions, ExportReport, ExportWorker,
};
use rustroom_app::buffer_cache::BufferCache;
use rustroom_app::crop_tool;
use rustroom_app::decode_worker::{DecodeWorker, Decoded, LensFacts};
use rustroom_app::edit_clipboard;
use rustroom_app::edit_history::HistoryStash;
use rustroom_app::entries::{self, PhotoEntry};
use rustroom_app::export_presets::{ExportPreset, ExportPresetStore};
use rustroom_app::file_rename;
use rustroom_app::format_filter;
use rustroom_app::full_res;
use rustroom_app::history::History;
use rustroom_app::mask_tool;
use rustroom_app::photo_id::PhotoId;
use rustroom_app::presets::{Preset, PresetStore};
use rustroom_app::render_engine::{FrameSpec, RegionRect, RenderEngine};
use rustroom_app::selection::Selection;
use rustroom_app::settings::{Accent, Settings, SettingsStore, ThemeChoice};
use rustroom_app::sidecar;
use rustroom_app::sync;
use rustroom_app::thumb_key::ThumbKey;
use rustroom_app::thumb_loader::{ThumbLoader, ThumbRequest};
use rustroom_app::tool::Tool;
use rustroom_app::view_region;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

mod catalog_ops;
mod collections_panel;
mod commands;
mod copies;
mod decode;
mod develop;
#[cfg(target_os = "macos")]
mod display_colorspace;
mod export_flow;
mod filmstrip;
mod interaction;
mod library;
mod menu_bar;
mod modals;
mod name_edit;
#[cfg(target_os = "macos")]
mod native_menu;
mod settings_apply;
mod shell;
#[cfg(test)]
mod test_app;
#[cfg(test)]
mod test_ui;
mod theme;
mod thumb;
mod transfer;
mod undo;

use decode::{decode_for_edit, decode_for_export, full_res_worker};
use modals::Modal;

const PREVIEW_MAX_DIM: usize = 1600;
/// How long the selection must sit still before we commit to a decode. Holding an arrow key
/// therefore starts zero decodes until you settle.
const DECODE_DEBOUNCE: Duration = Duration::from_millis(180);
/// Superseded decodes are abandoned rather than aborted, so cap how many run at once.
const MAX_CONCURRENT_DECODES: usize = 2;

/// One at a time: a full-resolution decode holds a 313 MB buffer, and two in flight would be
/// 626 MB of transient allocation for no gain.
const MAX_CONCURRENT_FULL_DECODES: usize = 1;
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
        Box::new(|cc| Ok(Box::new(RustroomApp::new(cc)))),
    )
}

// ⚠ `Copy` so the frame loop can snapshot the previous view for edge detection.
#[derive(Default, PartialEq, Eq, Clone, Copy)]
enum AppView {
    #[default]
    Library,
    Develop,
}

/// Browsing state for an opened folder.
/// What the Library is currently showing.
///
/// ⚠ A collection is a *source*, not a filter, so the format filter and the six sort orders
/// compose with it unchanged — they operate on whatever scan is loaded.
#[derive(Debug, Clone, PartialEq)]
enum LibrarySource {
    Folder(PathBuf),
    /// Every photo the catalog knows, from every folder ever opened.
    AllPhotos,
    /// The photos a saved rule matches, re-evaluated every time it is opened.
    Smart {
        id: i64,
        name: String,
    },
    /// Photos taken in any of these calendar periods.
    ///
    /// ⚠ A **set**, because ⌘-clicking several days is how a shoot spanning a weekend is browsed.
    /// A single click is the one-element case, so there is no second code path.
    Periods {
        periods: Vec<rustroom_app::date_tree::Period>,
        label: String,
    },
    Collection {
        id: i64,
        name: String,
    },
    Keyword {
        id: i64,
        name: String,
    },
}

struct Library {
    /// Where these photos came from.
    ///
    /// ⚠ Kept rather than derived from `scan[0].path.parent()`, which says nothing at all about
    /// an *empty* folder — and Reindex on an empty folder is exactly when you would want it,
    /// because the files may have arrived since.
    source: LibrarySource,
    /// The folder scan this Library was built from.
    ///
    /// ⚠ Kept so `entries` can be re-expanded when a copy is created or deleted, without a
    /// second directory read — and so the rebuild sees exactly the same files, rather than
    /// whatever the folder happens to hold at that moment.
    scan: Vec<catalog::CatalogEntry>,
    entries: Vec<PhotoEntry>,
    /// Loaded thumbnails, keyed by path and bounded by a byte budget.
    ///
    /// ⚠ Keyed by **path**, so copies of one file share a texture and a re-sort or a rebuild
    /// invalidates nothing. `entries` indices never reach it.
    cache: rustroom_app::thumb_cache::ThumbCache<egui::TextureHandle>,
    /// The last want-list handed to the loader.
    ///
    /// ⚠ Kept so `sync_thumb_window` can call `want` **only when the list changes**. Its body
    /// runs every frame; without the comparison it would rewrite the queue sixty times a second.
    /// The same guard `apply_view_demosaic` needs, for the same reason.
    last_want: Vec<ThumbRequest>,
    selection: Selection,
    loader: ThumbLoader,
    /// Indices into `entries` that pass `filter`, ascending. Derived — recomputed by
    /// `refilter`, never edited directly.
    visible: Vec<usize>,
    /// Marks for every marked photo on show, keyed by identity.
    ///
    /// ⚠⚠ Read **once per rebuild**, not per frame and never per cell. The grid draws sixty
    /// times a second over hundreds of thumbnails; a query per cell would be a database round
    /// trip per photo per frame. Only marked photos are in here, so an unmarked folder costs
    /// nothing.
    marks: std::collections::HashMap<PhotoId, catalog::db::Marks>,
    /// Capture metadata for everything on show, keyed by **path**.
    ///
    /// ⚠ By path, not `PhotoId`: capture is per *file* and every copy shares it, while `marks` is
    /// keyed by `PhotoId` for the opposite reason. Both carry a comment saying so, because it
    /// looks like an inconsistency and is not.
    ///
    /// ⚠⚠ Read **once per rebuild**, never per frame and never per cell — the rule `marks`
    /// already follows, and for the same reason: the grid draws sixty times a second.
    captures: std::collections::HashMap<PathBuf, core_image::Capture>,
    /// An in-progress rubber-band selection, as `(start, current)` in **screen** coordinates.
    ///
    /// ⚠ Screen coordinates rather than content coordinates, because the cell rects it is tested
    /// against come from `Response::rect`, which is also screen. Both are recomputed every frame,
    /// so scrolling mid-drag cannot leave them describing different things — the mistake the
    /// M3.1a mask overlay made by caching a derived screen position.
    band: Option<(egui::Pos2, egui::Pos2)>,
}

impl Library {
    /// Recompute `visible` and prune the selection to it.
    ///
    /// ⚠ Both halves, always. Recomputing the visible list without pruning leaves the
    /// selection holding photos that are not on screen, which a sync or a paste would then
    /// write to.
    fn refilter(&mut self, filter: &rustroom_app::library_filter::LibraryFilter) {
        // ⚠ One derived `visible` list, composed from one filter — **not** a second filtering
        // mechanism. Sparse-versus-dense index confusion is this project's third-most-repeated
        // bug, and `visible` staying the single derived list is the mitigation that has held.
        self.visible = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| {
                let m = self.marks.get(&e.id).copied().unwrap_or_default();
                let c = self.captures.get(&e.id.path).cloned().unwrap_or_default();
                filter.matches(e, m, &c)
            })
            .map(|(i, _)| i)
            .collect();
        self.selection.retain_visible(&self.visible);
    }

    /// The thumbnail request for row `i`, using `live` in place of the stored edits when that
    /// row is the photo currently open.
    ///
    /// ⚠ `live` is passed in rather than read from `app`: the callers hold `&mut Library` through
    /// a closure and cannot reach `app.session` from there. Hoisting it out is the same shape the
    /// grid's context menu already uses for the catalog.
    fn thumb_request(&self, i: usize, live: Option<&(PathBuf, u32, Edits)>) -> ThumbRequest {
        let e = &self.entries[i];
        let edits = match live {
            Some((p, c, ed)) if *p == e.id.path && *c == e.id.copy => ed.clone(),
            _ => e.edits.clone(),
        };
        ThumbRequest {
            id: e.id.clone(),
            edits,
        }
    }

    /// Tell the cache and the loader what is on screen.
    ///
    /// `window` is every thumbnail the view wants resident — the drawn cells plus the prefetch
    /// margin, one request per row rather than per file.
    /// Pins them in the cache, evicts down to the budget, then asks the loader for whatever is
    /// still missing.
    fn sync_thumb_window(&mut self, window: &[ThumbRequest]) {
        let keys: Vec<ThumbKey> = window.iter().map(|r| r.key()).collect();
        self.cache.retain_window(&keys);
        let missing: Vec<ThumbRequest> = window
            .iter()
            .filter(|r| !self.cache.contains(&r.key()))
            .cloned()
            .collect();
        // ⚠ Only when it changes. See `last_want`.
        if missing != self.last_want {
            self.loader.want(&missing);
            self.last_want = missing;
        }
    }
}

/// The modification time `collections::scan_of` used to read for itself.
///
/// ⚠ `UNIX_EPOCH` when the metadata cannot be read, so an unreadable file sorts to the oldest end
/// rather than failing the whole load — the rule `CatalogEntry::modified` already documents. It
/// lives here rather than in `scan_of` because **All Photos must not walk the filesystem**, and a
/// function that stats internally cannot offer that.
fn fs_modified(path: &std::path::Path) -> std::time::SystemTime {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
}

/// A photo whose decode is scheduled or in flight. `Session` still means "fully decoded",
/// so nothing downstream needs to know about this half-open state.
struct PendingOpen {
    id: PhotoId,
    /// The filmstrip thumbnail shown while decoding, when the folder is loaded.
    placeholder: Option<egui::TextureHandle>,
    /// Debounce deadline — the decode starts once `Instant::now()` passes this.
    start_at: Instant,
    /// The worker sequence number, once the decode has actually been requested.
    seq: Option<u64>,
}

struct RustroomApp {
    /// The camera profiles found on disk, read when a photo opens.
    ///
    /// ⚠ Cached rather than rescanned per frame: the panel body runs sixty times a second, and
    /// a directory scan there is the `apply_view_demosaic` mistake in a different costume.
    camera_profiles_cache: Vec<camera_profile::store::ProfileEntry>,
    /// Whether the OS is colour-matching the window for us, so we must not also convert.
    ///
    /// ⚠ Runtime, not `#[cfg]`: a macOS build where tagging failed has to take the manual path,
    /// and keeping it a value means both branches compile on every platform.
    os_manages_colour: bool,
    view: AppView,
    /// The view as of the previous frame, so a switch can be noticed.
    ///
    /// ⚠ An edge detector, not a second source of truth: it is only ever compared against
    /// `view` and then updated. The view itself is set from several places — the segmented
    /// switch, the View menu, the G/D keys — and this catches all of them without each having
    /// to remember to clear the status line.
    last_view: AppView,
    library: Option<Library>,
    session: Option<Session>,
    pending: Option<PendingOpen>,
    decoder: DecodeWorker,
    /// Decodes the full-resolution buffer for the true-1:1 tier, on demand.
    full_decoder: DecodeWorker,
    /// Path and sequence number of the full-resolution decode in flight, if any.
    full_pending: Option<(PathBuf, u64)>,
    cache: BufferCache,
    /// Undo stacks for photos that are not open, so switching away and back does not throw
    /// one away. Unlike `cache`, which is capped at 3 because it holds decoded buffers, this
    /// is unbounded — it holds `Edits`, not pixels.
    histories: HistoryStash,
    status: String,
    show_filmstrip: bool,
    /// Brush radius in **screen points**.
    ///
    /// ⚠ Not persisted, and not in `Settings`. A screen-point size is view-dependent, so
    /// restoring last session's value at a different window size or zoom restores a *different*
    /// brush. The other three could be persisted, but splitting one tool's settings across two
    /// lifetimes is worse than one honest rule.
    brush_size: f32,
    /// 0 = maximally soft, 100 = a hard edge. ⚠ Edge shape, **not** strength — see `brush_flow`.
    brush_hardness: f32,
    /// How much coverage one pass lays down, 0..=100 — the **strength** control.
    ///
    /// ⚠ Separate from hardness because they are genuinely different things, and conflating them
    /// is what made the first version's softest brush still paint at full strength.
    brush_flow: f32,
    /// Whether the next stroke subtracts.
    brush_erase: bool,
    /// Whether to tint the selected brush mask's coverage on the canvas.
    show_mask_tint: bool,
    /// Which HSL channel the Color section's selector is showing. View state, per app —
    /// like a brush size, it is a property of the tool and not of the photograph.
    hsl_channel: develop::panel::adjustments::HslChannel,
    /// Which colour-grading range the wheel is showing. View state, like `hsl_channel`.
    grade_range: develop::panel::color_wheel::GradeRange,
    /// Scratch text for the new-collection field.
    new_collection: String,
    /// Scratch name for the smart collection about to be saved.
    new_smart: String,
    /// The date row a Shift-range extends from.
    last_period: Option<rustroom_app::date_tree::Period>,
    /// Scratch text for the new-keyword field.
    new_keyword: String,
    /// The keyword a new one will be created under. `None` makes a root.
    new_keyword_parent: Option<i64>,
    /// Scratch text for the filename field, with **the photo it belongs to**.
    ///
    /// ⚠ A draft rather than writing through, so a half-typed name is never committed and a
    /// rename is one undo step rather than one per keystroke.
    ///
    /// ⚠⚠ The `PhotoId` is not decoration. As a bare `String` this draft outlived the photo it
    /// was typed for, so **every** photo switched to afterwards showed the last name typed —
    /// reported as "now every photo shows the name I changed". Pairing the text with its photo
    /// makes that unrepresentable rather than something to remember to clear.
    rename_draft: Option<(PhotoId, String)>,
    /// The snapshot row being renamed, and what has been typed into it.
    ///
    /// ⚠ A scratch draft, exactly like `rename_draft`: writing through on every keystroke would
    /// push one undo step per character and commit a half-typed name on focus loss. Cleared
    /// wherever `rename_draft` is, and for the same reason — a draft left behind belongs to a
    /// photo that is no longer open.
    snapshot_rename: Option<(u32, String)>,
    /// The Library panel section being dragged, if one is.
    ///
    /// ⚠ On the app rather than a local, because a drag spans frames and a `Response` only knows
    /// about the frame it was built in. Cleared on release, whatever the outcome.
    panel_drag: Option<rustroom_app::settings::PanelSection>,
    /// A grid-size pinch is under way and its result has not been written to disk yet.
    ///
    /// ⚠ Exists so the settings file is written once when the gesture settles rather than sixty
    /// times a second while it runs.
    cell_zoom_pending: bool,
    /// Which Develop tool the rail has selected.
    ///
    /// ⚠ On the app, not on `Session`, so it **survives a photo switch** — see `tool::Tool`.
    tool: Tool,
    /// The pixel aspect ratio the crop is locked to, or `None` for a free crop.
    ///
    /// ⚠ Stored as the **pixel** ratio (1.0, 1.5, …), not the normalized one. The normalized
    /// aspect is `pixel / image_aspect` and so differs per photo; storing that would silently
    /// change shape the moment you switched to a photo with different dimensions.
    ///
    /// ⚠ Tool state, not an edit: it must never reach the sidecar, a paste or a preset, for
    /// the same reason `Session::selected_mask` does not.
    crop_aspect: Option<f32>,
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
    /// The one catalog. `None` when it could not be opened.
    ///
    /// ⚠⚠ **Invisible by design.** Nothing in the UI mentions it; it exists so that collections
    /// — and later ratings and search — have somewhere to live. If you can tell it shipped,
    /// something is wrong.
    ///
    /// ⚠ `Option` rather than a hard failure: a catalog that will not open must not stop you
    /// editing photographs, because edits live in the sidecars and do not need it.
    catalog: Option<catalog::db::Catalog>,
    /// Reads capture metadata into the catalog in the background.
    ///
    /// ⚠ Its own pass rather than part of the thumbnail loader, which is **windowed** — you cannot
    /// filter by a date that was never read. Built once and kept for the life of the app, so a
    /// source switch never abandons a half-scanned folder.
    metadata: rustroom_app::metadata_scanner::MetadataScanner,
    /// Everything that narrows what the Library shows.
    ///
    /// ⚠⚠ On the **app**, not on `Library`, so it survives a source switch. Browsing *replaces*
    /// the source, so filter-then-pick-a-date is the only way to compose the two — and rebuilding
    /// the filter on every switch destroyed exactly that. ⚠ It is safe to persist because the
    /// Filter button reads `Filter: …` whenever it is active, so a filter that hides everything is
    /// visible and one click from fixed. That label did not exist when the reset was chosen.
    ///
    /// ⚠ Session-only. The *format* half is separately persisted in `Settings`, because a format
    /// preference is standing where a search string is about the question being asked now.
    filter: rustroom_app::library_filter::LibraryFilter,
    modal: Modal,
    /// One chronological order over photo edits *and* preset deletions, so Ctrl+Z walks
    /// back through both. The payloads stay where they are — photo snapshots in
    /// `Session::history`, deleted presets in the trash — and this only records which of
    /// the two the next undo should pop.
    actions: ActionLog,
}

impl RustroomApp {
    /// ⚠ Takes the whole `CreationContext`, not just the egui one, because the window handle is
    /// needed to reach the `CAMetalLayer` — see `display_colorspace`.
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let ctx = &cc.egui_ctx;
        // ⚠ Installed here rather than lazily per frame: `init_for_nsapp` must run once, on
        // the main thread, after NSApplication exists — which it does by the time eframe
        // builds the app.
        #[cfg(target_os = "macos")]
        native_menu::install(ctx);
        // ⚠ Once is enough: `wgpu_hal`'s `configure()` never touches `colorspace`, so the value
        // survives every resize and surface rebuild. The result decides whether *we* convert or
        // the OS does — never both, which would double-convert.
        #[cfg(target_os = "macos")]
        let os_manages_colour = {
            use raw_window_handle::HasWindowHandle;
            cc.window_handle()
                .ok()
                .map(|h| display_colorspace::tag_as_srgb(h.as_raw()))
                .unwrap_or(false)
        };
        #[cfg(not(target_os = "macos"))]
        let os_manages_colour = false;
        let ctx2 = ctx.clone();
        let preset_store = PresetStore::user_default();
        let preset_list = preset_store.list();
        let export_preset_store = ExportPresetStore::user_default();
        let export_preset_list = export_preset_store.list();
        let settings_store = SettingsStore::user_default();
        let settings = settings_store.load();
        // ⚠ Before any UI runs: `add_font` rebuilds the glyph atlas, so installing later would
        // throw away the first frame's cache for nothing. Unlike `apply`, this does not depend
        // on the settings, so it does not have to wait for the load.
        theme::install_fonts(ctx);
        // ⚠ After the load, not before: the style now depends on the persisted theme and
        // accent, so applying it earlier would style the app from the defaults and then
        // silently disagree with what the user chose.
        theme::apply(ctx, &settings);
        // ⚠ Opened once, at startup. A failure is reported and then ignored — see the field.
        let catalog = match catalog::db::Catalog::open(&catalog::db::Catalog::user_default_path()) {
            Ok(c) => Some(c),
            Err(e) => {
                eprintln!("catalog unavailable: {e}");
                None
            }
        };
        Self {
            filter: rustroom_app::library_filter::LibraryFilter::default(),
            metadata: rustroom_app::metadata_scanner::MetadataScanner::new({
                let ctx = ctx.clone();
                move || ctx.request_repaint()
            }),
            camera_profiles_cache: Vec::new(),
            os_manages_colour,
            view: AppView::default(),
            last_view: AppView::default(),
            library: None,
            session: None,
            pending: None,
            // ⚠ No profile at startup: nothing is open, so no camera is known. `open_path`
            // rebuilds this worker once the model is, which is the same rebuild-on-change shape
            // `apply_view_demosaic` uses.
            decoder: DecodeWorker::new(
                move |p| decode_for_edit(p, None),
                MAX_CONCURRENT_DECODES,
                move || ctx2.request_repaint(),
            ),
            full_decoder: full_res_worker(settings.view_demosaic, None, ctx),
            full_pending: None,
            cache: BufferCache::new(BUFFER_CACHE_CAP),
            histories: HistoryStash::default(),
            status: String::new(),
            show_filmstrip: true,
            brush_size: 40.0,
            brush_hardness: 50.0,
            brush_flow: 100.0,
            brush_erase: false,
            // On by default: a freshly added brush mask with no adjustment yet is otherwise
            // completely invisible, which is a poor first five minutes.
            show_mask_tint: true,
            hsl_channel: Default::default(),
            grade_range: Default::default(),
            new_collection: String::new(),
            new_smart: String::new(),
            last_period: None,
            new_keyword: String::new(),
            new_keyword_parent: None,
            rename_draft: None,
            snapshot_rename: None,
            panel_drag: None,
            cell_zoom_pending: false,
            tool: Default::default(),
            crop_aspect: None,
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
            catalog,
            modal: Modal::None,
            actions: ActionLog::default(),
        }
    }
}

pub struct Session {
    path: PathBuf,
    /// Which copy of `path` this session is editing. `0` is the original.
    ///
    /// ⚠ Added beside `path` rather than replacing it with a `PhotoId`: `path` has many call
    /// sites that genuinely want the **file** — the decode, the buffer cache, the thumbnail.
    /// Only the sidecar, the undo scope and the label want the version.
    copy: u32,
    /// This copy's own name, when it has one. Always `None` on the original, whose name is the
    /// filename on disk.
    name: Option<String>,
    /// This copy's snapshots, cached from the sidecar when the photo opened.
    ///
    /// ⚠⚠ **Cached rather than read where it is drawn.** The strip lives in the panel body, which
    /// runs every frame; a `load_all` there would read the file sixty times a second — the
    /// `apply_view_demosaic` rule. `name` above is cached from the same envelope for the same
    /// reason.
    ///
    /// ⚠ Kept in step by the `RustroomApp::*_snapshot` methods, which are the only writers. A
    /// second place mutating the sidecar's snapshots without updating this is two facts that can
    /// disagree.
    snapshots: Vec<rustroom_app::sidecar::Snapshot>,
    engine: RenderEngine,
    edits: Edits,
    texture: Option<egui::TextureHandle>,
    baseline: egui::TextureHandle,
    hist: [[u32; 256]; 3],
    make_model: String,
    /// The lens the shot was taken with, and the profile matched to it.
    ///
    /// ⚠ Kept on the session so the panel can name a lens it could *not* match. A miss is the
    /// commonest reason the controls do nothing, and the only way to act on it is to be told
    /// which name failed.
    lens: LensFacts,
    zoom: f32,
    /// The scale that fits `fit_image_size` into the canvas, as of the last frame.
    ///
    /// ⚠ Written by the canvas, which is the only thing that knows the viewport. "Zoom to 1:1"
    /// needs it and cannot compute it — the same reason `fit_image_size` is stored rather than
    /// derived. It is a cache of per-frame view geometry, never an edit.
    fit_scale: f32,
    pan: egui::Vec2,
    history: History<Edits>,
    show_before: bool,
    active_handle: Option<crop_tool::Handle>,
    /// Which mask the canvas is editing, if any. Index into `edits.masks`.
    ///
    /// ⚠ **View state, not an edit.** It must not reach the sidecar, transfer on a paste or
    /// appear in a preset — `Edits` is the file format, and a selection is not part of the
    /// photograph's description. `Session` is per-photo, so this resets on a switch for free.
    selected_mask: Option<usize>,
    /// The mask handle this drag is moving, if the drag started on one.
    ///
    /// ⚠ Load-bearing for panning: a drag that did *not* start on a handle must still pan the
    /// image, so this records whether the mask claimed the gesture.
    active_mask_handle: Option<mask_tool::Handle>,
    /// Drawn perspective guides.
    ///
    /// ⚠⚠ **View state, never `Edits`.** A guide is scaffolding for arriving at a slider value,
    /// not part of the photograph's description — `selected_mask`'s rule. So guides never reach
    /// the sidecar, never travel with a paste, preset or sync, and reset on a photo switch. That
    /// is what makes "solve into the sliders" one representation rather than two.
    guides: Vec<pipeline::perspective::guides::Guide>,
    /// The perspective and straighten the **visible** texture was rendered with, and the pair the
    /// newest request will produce.
    ///
    /// ⚠⚠ Two, not one: a render is asynchronous, so "what was asked for" and "what is showing"
    /// genuinely differ for as long as it takes. The guide overlay is drawn against `shown_geo`,
    /// so a guide stays on the feature it was traced onto until the image actually catches up —
    /// which at native zoom is hundreds of milliseconds, because perspective is a *prefix* stage
    /// and changing it drops the prefix cache.
    shown_geo: (edit_model::Perspective, f32),
    /// ⚠ A `Cell` so `request` can stay `&self`. It is called from places already holding a
    /// shared borrow of the session, and `&mut self` there is an `E0502`.
    pending_geo: std::cell::Cell<(edit_model::Perspective, f32)>,
    /// Whether the Guides sub-mode of the Geometry tool is on.
    ///
    /// ⚠ On `Session` rather than `RustroomApp`, beside the guides themselves — which is what
    /// lets `view_edits` read it directly and leaves all seven of its call sites untouched.
    show_guides: bool,
    /// `(guide index, 0 = from | 1 = to)` while an endpoint is being dragged.
    active_guide: Option<(usize, usize)>,
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
    /// The editing buffer's long edge — the top rung of the render ladder.
    buffer_long: usize,
    /// Whether `engine`'s buffer is the full-resolution decode.
    ///
    /// True from the start for JPEG/PNG/TIFF, which are decoded at full size already.
    buffer_is_full_res: bool,
    /// When this session was created — the dwell the upgrade trigger waits on.
    opened_at: Instant,
    /// The rung the canvas last chose, so an edit made from a panel re-renders at the tier
    /// currently on screen rather than snapping back to fit. Read through
    /// [`Session::render_max_dim`], never directly.
    render_rung: usize,
    /// The display's primaries, copied from `Settings` when the session opens.
    ///
    /// ⚠ Held here rather than reached through the app because `Session::request` — which every
    /// render goes through — has no access to `RustroomApp`. `apply_display_space` writes it on
    /// the open session so a change takes effect without reopening the photo.
    display_space: color::OutputSpace,
}

impl Session {
    /// The working resolution for a re-render driven from a panel rather than the canvas.
    ///
    /// ⚠ **Derived from `requested_region`, not stored beside it.** A stored rung and a stored
    /// region are two facts that can disagree, and when they did — `pump_full_res` clears the
    /// region but the canvas had left a native rung — a panel edit issued a *whole-image
    /// render at a native rung*. That poisoned the cached fit size and made the photo open
    /// zoomed absurdly far out, unrecoverably. Deriving one from the other makes the
    /// disagreement unrepresentable.
    fn render_max_dim(&self) -> usize {
        if self.requested_region.is_some() {
            self.render_rung
        } else {
            self.fit_cap()
        }
    }

    /// The ladder's base rung for this session's buffer.
    ///
    /// ⚠ Every whole-image request must use this, not `PREVIEW_MAX_DIM`. They differ for an
    /// image smaller than the cap, and the canvas identifies a fit frame by comparing the
    /// frame's own `max_dim` against this — so a request at the wrong value produces a frame
    /// that is silently never accepted as the fit size.
    fn fit_cap(&self) -> usize {
        view_region::fit_cap(self.buffer_long, PREVIEW_MAX_DIM)
    }

    /// Send a render request. A one-line delegate: it no longer routes anywhere, but "every
    /// render request goes through one place" is worth keeping.
    fn request(&self, edits: Edits, max_dim: usize, region: Option<RegionRect>) {
        // ⚠⚠ Recorded so the guide overlay can be drawn against what is **on screen** rather than
        // what has just been asked for. Perspective is a *prefix* stage, so changing it drops the
        // prefix cache — at native zoom the re-render costs hundreds of milliseconds, during which
        // the guides would sit at their new positions over the previous image.
        self.pending_geo
            .set((edits.perspective, edits.geometry.angle));
        self.engine.request(
            edits,
            FrameSpec {
                max_dim,
                region,
                display_space: self.display_space,
            },
        );
    }
}

/// Edits to render for the preview. In Geometry mode the crop is reset to the full frame so
/// the whole straightened image shows under the crop overlay.
///
/// ⚠ **Only the crop.** The orientation must survive, or the preview would show a
/// differently-oriented image from the one being edited — and the crop overlay would then be
/// drawn against a frame that does not match it.
fn preview_edits(edits: &Edits, tool: Tool) -> Edits {
    let mut e = edits.clone();
    if tool.is_geometry() {
        e.geometry.crop = edit_model::CropRect::default();
    }
    e
}

impl Session {
    /// The edits to render for the current view.
    ///
    /// ⚠ **Every render request goes through this**, so nothing can forget the before/after
    /// state. That is the point: `show_before` used to be handled by swapping in a texture the
    /// decode worker had rendered once, at preview resolution, which looked correct at fit and
    /// visibly pixelated at any zoom.
    ///
    /// ⚠ **"Before" keeps the geometry and drops the adjustments**, rather than being fully
    /// neutral. A fully neutral render is *uncropped*, so it has different dimensions from the
    /// edited one — the region and placement maths would describe different content, and
    /// toggling while zoomed would jump to somewhere else in the photo. Holding the framing
    /// fixed is what makes a zoomed comparison mean anything.
    fn view_edits(&self, tool: Tool) -> Edits {
        let edits = if self.show_before {
            before_edits(&self.edits)
        } else {
            self.edits.clone()
        };
        preview_edits(&edits, tool)
    }
}

/// The photo as it was before any adjustment, at the **current framing**.
///
/// ⚠ Geometry survives on purpose — see [`Session::view_edits`]. A free function rather than a
/// method so it can be tested without building a `Session`, which needs a decoded buffer and a
/// texture.
fn before_edits(edits: &Edits) -> Edits {
    Edits {
        geometry: edits.geometry,
        ..Default::default()
    }
}

impl RustroomApp {
    /// Save the current session's edits to its `<file>.json` sidecar, if it has any.
    /// Called before switching photos / on exit so edits are never lost.
    /// The open photo, which is what the action log's photo-edit entries must match to be
    /// undoable right now.
    ///
    /// ⚠ Returns an owned `PathBuf` rather than a borrow: every caller passes it *into* a
    /// `&mut self.actions` method, and a borrow of `self` living across that call does not
    /// compile. One clone per user gesture is not worth fighting the borrow checker over.
    /// Which photo the undo log should scope to.
    ///
    /// Every site that scopes undo goes through here, which is what keeps "which photo is this?"
    /// a single answer rather than one per call site.
    fn open_photo(&self) -> Option<PhotoId> {
        self.session
            .as_ref()
            .map(|s| PhotoId::new(s.path.clone(), s.copy))
    }

    fn persist_current_session(&self) {
        if let Some(s) = &self.session {
            // `persist`, not `save`: neutral edits *delete* the sidecar. Skipping the write
            // would leave a stale one behind, so "Reset all" survived only until the next
            // switch away and back.
            // ⚠ `persist_copy`, not `persist`: two copies of one file share a sidecar, and
            // whole-file writes would make the last-saved copy silently overwrite the other.
            let _ = sidecar::persist_copy(&s.path, s.copy, &s.edits);
        }
    }

    /// Apply `src`'s selected groups to the open photo and re-render.
    ///
    /// Returns whether a history step was actually recorded — `History::push` is a no-op
    /// when nothing changed, and logging a step that did not happen would desynchronize the
    /// action log from the session history. The caller decides how to log it, which is what
    /// lets a paste bundle this together with a sync into one undo step.
    fn apply_edits_to_session(&mut self, src: &Edits, groups: EditGroups) -> bool {
        // ⚠ Hoisted before the session borrow: `self.tool` is an immutable borrow of `self`
        // and `&mut self.session` is a mutable one, so reading it afterwards is E0502.
        // `Tool` is `Copy`, so the hoist is free.
        let tool = self.tool;
        let Some(s) = &mut self.session else {
            return false;
        };
        s.edits.merge_from(src, groups);
        let pushed = s.history.push(s.edits.clone());
        s.request(s.view_edits(tool), s.render_max_dim(), s.requested_region);
        pushed
    }

    /// Re-render after the session's edits were changed by an undo or a redo.
    fn rerender_current(&mut self) {
        let tool = self.tool;
        let Some(s) = &mut self.session else { return };
        s.edits = s.history.get().clone();
        s.request(s.view_edits(tool), s.render_max_dim(), s.requested_region);
    }

    /// Start opening `path`: instant from the cache, otherwise a debounced background decode
    /// with the filmstrip thumbnail as a placeholder. Never blocks the UI thread.
    fn open_path(&mut self, ctx: &egui::Context, id: PhotoId) {
        self.persist_current_session();

        // ⚠ The cache is keyed by **path**, so opening a copy of an already-decoded file is a
        // hit — which is exactly M3.4's "no storage cost".
        if let Some(decoded) = self.cache.get(&id.path) {
            self.pending = None;
            self.finalize_session(ctx, id, decoded);
            return;
        }

        let placeholder = self.thumbnail_for(&id);
        let path = id.path.clone();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        self.status = format!("Decoding {name}…");
        self.pending = Some(PendingOpen {
            id,
            placeholder,
            start_at: Instant::now() + DECODE_DEBOUNCE,
            seq: None,
        });
    }

    /// The library thumbnail for `path`, if the folder is loaded and that thumbnail has
    /// already arrived. `TextureHandle` is `Arc`-backed, so cloning it is cheap.
    fn thumbnail_for(&self, id: &PhotoId) -> Option<egui::TextureHandle> {
        // ⚠ `get_stale`: the best thumbnail available for this photo, even if its edits have
        // since changed. The decode placeholder and the grid then show the previous render rather
        // than grey while a re-render is in flight.
        self.library.as_ref()?.cache.get_stale(id).cloned()
    }

    /// The edits a photo's thumbnail should be rendered with.
    ///
    /// ⚠⚠ **Two sources, deliberately.** For the **open** photo the edits live in `Session` and
    /// the sidecar is stale — `persist_current_session` writes only on a photo switch or on quit,
    /// which is exactly the period during which the filmstrip most needs to update. For every
    /// other photo the entry's copy, read from the sidecar when the row was built, is correct.
    fn live_edits(&self) -> Option<(PathBuf, u32, Edits)> {
        let s = self.session.as_ref()?;
        Some((s.path.clone(), s.copy, s.edits.clone()))
    }

    /// Build the editing `Session` from a decoded photo (cache hit or worker result).
    fn finalize_session(&mut self, ctx: &egui::Context, id: PhotoId, decoded: Arc<Decoded>) {
        let path = id.path.clone();
        // ⚠ The only place `self.session` is ever assigned, and so the only choke point the
        // stash needs: the outgoing history is put away here whether the switch came from a
        // cache hit or from a finished background decode.
        if let Some(old) = self.session.take() {
            self.histories
                .stash(PhotoId::new(old.path, old.copy), old.history);
        }
        // ⚠ Scanned here, once per photo, rather than in the panel body which runs every frame.
        // Cheap enough per photo; ruinous sixty times a second.
        self.camera_profiles_cache = camera_profile::store::ProfileStore::user_default(
            &dirs::config_dir().unwrap_or_else(|| std::path::PathBuf::from(".")),
        )
        .list_for(&decoded.label);

        // ⚠ The preference applies only to a photo with **no sidecar**. A photo already edited
        // keeps exactly what it had, which is what lets `Edits::default()` stay strictly neutral
        // while fresh photos still open corrected — see `Settings::initial_lens_correction`.
        let stored = sidecar::load_copy(&path, id.copy);
        let mut edits = stored.clone().unwrap_or_default();
        edits.lens_correction = self.settings.initial_lens_correction(stored.is_some());
        let edits = edits;
        let history = match self.histories.take_matching(&id, &edits) {
            Some(h) => h,
            None => {
                // Either this photo has no stack, or the one it had describes a state no
                // longer on disk — a sync or a paste rewrote its sidecar while it was closed.
                // Any log entry pointing at that stack would now pop steps it does not hold.
                self.actions.forget_edits_of(&id);
                History::new(edits.clone())
            }
        };

        // ⚠ One read for both the name and the snapshots, rather than a `load_all` per field:
        // they come from the same envelope, and a second read is a second chance to disagree.
        let envelope = sidecar::load_all(&id.path);

        let bf = &decoded.baseline;
        let baseline = ctx.load_texture(
            "baseline",
            egui::ColorImage::from_rgba_unmultiplied([bf.width, bf.height], &bf.rgba),
            egui::TextureOptions::LINEAR,
        );

        let ctx2 = ctx.clone();
        let engine = RenderEngine::new(
            decoded.buffer.clone(),
            decoded.source_scale,
            decoded.lens.shot.clone(),
            move || ctx2.request_repaint(),
        );
        let buffer_long = decoded.buffer.width().max(decoded.buffer.height());
        // `fit_cap`, not `PREVIEW_MAX_DIM`: they differ for an image smaller than the cap, and
        // the canvas only accepts a frame as the fit size when its `max_dim` matches.
        let fit_cap = view_region::fit_cap(buffer_long, PREVIEW_MAX_DIM);
        // ⚠ The display space must be passed here too. This call used `FrameSpec::fit`, which
        // hardcodes sRGB, so **every photo opened uncorrected** and stayed that way until some
        // unrelated edit forced a re-render — which made the whole setting look inert. Found by
        // instrumenting `pack`, not by reading the code, which looks right at every call site.
        // `FrameSpec::fit` is now `#[cfg(test)]` so this cannot recur.
        engine.request(
            edits.clone(),
            FrameSpec {
                max_dim: fit_cap,
                region: None,
                display_space: self
                    .settings
                    .effective_display_space(self.os_manages_colour),
            },
        );
        // Non-RAW formats are decoded at full size, so their buffer is already the real thing.
        let buffer_is_full_res = !catalog::is_raw(&path);

        self.session = Some(Session {
            display_space: self
                .settings
                .effective_display_space(self.os_manages_colour),
            path,
            copy: id.copy,
            // ⚠ Read from the sidecar rather than carried from the Library entry, so a session
            // opened without a Library (single-file mode) still knows its own name.
            name: envelope.name_of(id.copy).map(str::to_string),
            snapshots: envelope.snapshots_of(id.copy).to_vec(),
            engine,
            edits,
            texture: None,
            baseline,
            hist: [[0; 256]; 3],
            make_model: decoded.label.clone(),
            lens: decoded.lens.clone(),
            zoom: 1.0,
            fit_scale: 1.0,
            pan: egui::Vec2::ZERO,
            history,
            show_before: false,
            active_handle: None,
            selected_mask: None,
            active_mask_handle: None,
            guides: Vec::new(),
            active_guide: None,
            shown_geo: (edit_model::Perspective::default(), 0.0),
            pending_geo: std::cell::Cell::new((edit_model::Perspective::default(), 0.0)),
            show_guides: false,
            rendered_region: None,
            requested_region: None,
            // Corrected by the first frame that arrives; the placeholder only has to be
            // non-zero so the first fit calculation cannot divide by zero.
            fit_image_size: egui::vec2(1.0, 1.0),
            rendered_long: 1.0,
            buffer_long,
            buffer_is_full_res,
            opened_at: Instant::now(),
            render_rung: PREVIEW_MAX_DIM,
        });
        self.status.clear();
    }

    /// Per-frame: start the full-resolution decode once the session has settled.
    ///
    /// ⚠ It must schedule its own repaint while waiting. egui does not redraw an idle window,
    /// so without this the dwell could expire with nothing to notice, and the decode would
    /// start only when the user next moved the mouse.
    fn maybe_request_full_res(&mut self, ctx: &egui::Context) {
        let in_flight = self.full_pending.is_some();
        let Some(s) = self.session.as_ref() else {
            return;
        };
        if !catalog::is_raw(&s.path) || s.buffer_is_full_res || in_flight {
            return;
        }
        let age = s.opened_at.elapsed();
        if !full_res::wants_upgrade(true, false, false, age) {
            ctx.request_repaint_after(full_res::SETTLE - age);
            return;
        }
        let path = s.path.clone();
        let seq = self.full_decoder.request(path.clone(), PREVIEW_MAX_DIM);
        self.full_pending = Some((path, seq));
        self.status = "Loading full resolution…".to_string();
    }

    /// Per-frame: absorb a finished full-resolution decode into the live session.
    fn pump_full_res(&mut self, ctx: &egui::Context) {
        let tool = self.tool;
        for res in self.full_decoder.poll() {
            let is_current = self.full_pending.as_ref().map(|(_, seq)| *seq) == Some(res.seq);
            if is_current {
                self.full_pending = None;
            }
            // A result for a photo we have since navigated away from is dropped: the buffer is
            // per-session and dies with it, so there is nothing to keep it in.
            let Some(s) = self.session.as_mut() else {
                continue;
            };
            if !is_current || s.path != res.path {
                continue;
            }
            match res.outcome {
                Ok(decoded) => {
                    // ⚠ The baseline `DecodeWorker` rendered is discarded on purpose: the
                    // existing preview-resolution one stays, so before/after does not change
                    // under the user. The waste is one ~60 ms render on a background thread
                    // during a 3 s decode.
                    // ⚠ The old engine is *replaced*, not kept alongside: once every rung
                    // renders from the full buffer the half-size one is dead weight. Dropping
                    // it disconnects its channel, so its worker thread returns on its own. Its
                    // prefix cache goes too — a one-off cost at the moment of upgrade.
                    let ctx2 = ctx.clone();
                    s.buffer_long = decoded.buffer.width().max(decoded.buffer.height());
                    s.engine = RenderEngine::new(
                        decoded.buffer.clone(),
                        decoded.source_scale,
                        // ⚠ The profile must be carried across the full-resolution handover, or
                        // corrections would silently switch off the moment the upgrade lands.
                        s.lens.shot.clone(),
                        move || ctx2.request_repaint(),
                    );
                    s.buffer_is_full_res = true;
                    // ⚠ Load-bearing. Region rects are in *rendered* pixels, and those
                    // coordinates change with the buffer — reusing them would describe the
                    // wrong part of the image. Clearing both forces the next frame to
                    // re-request against the new resolution.
                    s.rendered_region = None;
                    s.requested_region = None;
                    s.render_rung = PREVIEW_MAX_DIM;
                    // ⚠ Ask for a frame explicitly. At the fit rung the canvas would otherwise
                    // see an unchanged rung and a covered region and request nothing at all —
                    // leaving the old buffer's texture on screen, so the upgrade would be
                    // invisible until something else happened to trigger a render.
                    s.request(s.view_edits(tool), s.fit_cap(), None);
                    // ⚠ Only worth saying where it is visible. A full-resolution decode can
                    // finish while the user is browsing the Library, where the message means
                    // nothing and merely displaces something that does.
                    if self.view == AppView::Develop {
                        self.status = "Full resolution ready".to_string();
                    }
                }
                Err(e) => {
                    // The ladder simply tops out at the half-size buffer, i.e. the behaviour
                    // before this feature existed.
                    self.status = format!("Full-resolution decode failed: {e}");
                }
            }
        }
    }

    /// Per-frame pump: fire the debounce timer, then absorb finished decodes.
    fn pump_decodes(&mut self, ctx: &egui::Context) {
        if let Some(p) = &mut self.pending {
            if p.seq.is_none() {
                let now = Instant::now();
                if now >= p.start_at {
                    let path = p.id.path.clone();
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
                        // ⚠ The **pending** id, not `res.path`: the decode is keyed by file, but
                        // which *copy* was being opened is only known here.
                        let id = self
                            .pending
                            .take()
                            .map(|p| p.id)
                            .unwrap_or_else(|| PhotoId::original(res.path.clone()));
                        self.finalize_session(ctx, id, decoded);
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
        // ⚠ Single-file mode has no Library, so no copies are listed — open the original.
        // Its sidecar may still hold copies; they simply are not browsable until a folder is
        // opened, which is the same limitation single-file mode already has for the filmstrip.
        self.open_path(ctx, PhotoId::original(path));
        self.library = None;
        self.view = AppView::Develop;
    }

    fn open_folder(&mut self, ctx: &egui::Context) {
        let Some(dir) = rfd::FileDialog::new().pick_folder() else {
            return;
        };
        self.open_folder_at(ctx, dir);
    }

    /// Scan `dir` and show it. The half of `open_folder` that is not the file dialog.
    fn open_folder_at(&mut self, ctx: &egui::Context, dir: PathBuf) {
        match catalog::scan_folder(&dir) {
            Ok(mut scan) => {
                // ⚠ Ordered **before** expanding into entries. Sorting the scan is what keeps a
                // file's copies adjacent to it and ascending under every ordering — sorting
                // `entries` instead would scatter them.
                if self.settings.library_sort.needs_edited() {
                    for e in scan.iter_mut() {
                        e.edited = sidecar::edited_at(&e.path);
                    }
                }
                catalog::sort(&mut scan, self.settings.library_sort);
                // ⚠ The catalog records the *files*; copies are added when they are created
                // rather than scanned, because they live in the sidecar and not on disk.
                if let Some(cat) = &self.catalog {
                    if let Err(e) = cat.sync_folder(&dir, &scan) {
                        eprintln!("catalog: {e}");
                    }
                }
                let n = self.open_scan(ctx, LibrarySource::Folder(dir.clone()), scan);
                self.status = if n == 0 {
                    "No supported images in this folder.".into()
                } else {
                    format!("{n} photos")
                };
            }
            Err(e) => self.status = format!("Could not read folder: {e}"),
        }
    }

    /// Show every photo the catalog knows, from every folder ever opened.
    ///
    /// ⚠⚠ **This does not walk the filesystem.** The modification times come from the catalog,
    /// recorded when each folder was last scanned — which is what makes the view affordable at
    /// 10,000 photos, and is what `mtime` was captured for in M4.1 (nothing had read it until
    /// now). Consequence: a file changed outside Rustroom since its folder was last scanned shows
    /// a stale time here. Clicking that folder in the panel re-scans it and refreshes them.
    fn open_all_photos(&mut self, ctx: &egui::Context) {
        let Some(cat) = &self.catalog else {
            self.status = "No catalog".into();
            return;
        };
        let rows = match cat.all_photos() {
            Ok(r) => r,
            Err(e) => {
                self.status = format!("Could not read the catalog: {e}");
                return;
            }
        };
        // ⚠ A lookup built once, rather than a stat per photo. `s.max(0)` guards a negative unix
        // time, which `SystemTime` addition cannot represent — it would panic rather than sort
        // oddly.
        let mtimes: std::collections::HashMap<PathBuf, std::time::SystemTime> = rows
            .iter()
            .filter_map(|r| {
                r.mtime.map(|s| {
                    (
                        r.path.clone(),
                        std::time::SystemTime::UNIX_EPOCH
                            + std::time::Duration::from_secs(s.max(0) as u64),
                    )
                })
            })
            .collect();
        let members: Vec<(PathBuf, u32)> = rows.into_iter().map(|r| (r.path, r.copy)).collect();
        let mut scan = rustroom_app::collections::scan_of(&members, sidecar::edited_at, |p| {
            mtimes
                .get(p)
                .copied()
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        });
        catalog::sort(&mut scan, self.settings.library_sort);
        let n = self.open_scan(ctx, LibrarySource::AllPhotos, scan);
        self.status = if n == 0 {
            "The catalog is empty — open a folder to add photos.".into()
        } else {
            format!("All photos — {n}")
        };
    }

    /// Handle a click on a date row, honouring the modifiers.
    ///
    /// ⚠⚠ **The same three rules `apply_click` uses for photos**, so the app has *one*
    /// multi-select idiom rather than two: a plain click replaces, ⌘/Ctrl toggles, Shift extends
    /// from the last click.
    ///
    /// ⚠ Shift extends **in tree order**, which `date_tree::flatten` defines — so "everything
    /// between" means what the eye sees rather than what a tuple comparison would say.
    fn click_period(
        &mut self,
        ctx: &egui::Context,
        period: rustroom_app::date_tree::Period,
        mods: egui::Modifiers,
    ) {
        use rustroom_app::date_tree;
        let current: Vec<date_tree::Period> = match self.library.as_ref().map(|l| &l.source) {
            Some(LibrarySource::Periods { periods, .. }) => periods.clone(),
            _ => Vec::new(),
        };
        let next = if mods.command {
            let mut v = current.clone();
            if let Some(i) = v.iter().position(|p| *p == period) {
                v.remove(i);
            } else {
                v.push(period);
            }
            v
        } else if mods.shift {
            let days = self
                .catalog
                .as_ref()
                .and_then(|c| c.capture_days().ok())
                .unwrap_or_default();
            let order = date_tree::flatten(&date_tree::group(&days));
            // ⚠ Anchored on the *last* click, which is the previous selection's tail — the same
            // thing `Selection::extend_to` anchors on.
            let anchor = self.last_period.unwrap_or(period);
            date_tree::range(&order, anchor, period)
        } else {
            vec![period]
        };

        // ⚠ Only a plain or ⌘ click moves the anchor; a Shift-click extends from where it was, or
        // shift-clicking twice would walk the range along instead of growing it.
        if !mods.shift {
            self.last_period = Some(period);
        }

        if next.is_empty() {
            // ⚠ Deselecting the last date closes the view rather than showing an empty grid with
            // no way back — `photos_in_periods(&[])` matches nothing by design.
            self.library = None;
            self.status = "No dates selected".into();
            return;
        }
        let label = date_tree::label_for(&next);
        self.open_periods(ctx, &next, &label);
    }

    fn open_periods(
        &mut self,
        ctx: &egui::Context,
        periods: &[rustroom_app::date_tree::Period],
        label: &str,
    ) {
        let Some(cat) = &self.catalog else {
            self.status = "No catalog".into();
            return;
        };
        let members = match cat.photos_in_periods(periods) {
            Ok(m) => m,
            Err(e) => {
                self.status = format!("Could not read the catalog: {e}");
                return;
            }
        };
        let mut scan =
            rustroom_app::collections::scan_of(&members, sidecar::edited_at, fs_modified);
        catalog::sort(&mut scan, self.settings.library_sort);
        let n = self.open_scan(
            ctx,
            LibrarySource::Periods {
                periods: periods.to_vec(),
                label: label.to_string(),
            },
            scan,
        );
        self.status = if n == 0 {
            format!("Nothing taken in {label}")
        } else {
            format!("{label} — {n} photos")
        };
    }

    /// Collection ids per photo, and keyword ids per **file**.
    ///
    /// ⚠ One query per collection and per keyword, never one per photo: a catalog has a handful
    /// of each and may have ten thousand photos.
    ///
    /// ⚠⚠ Two maps keyed differently, and that is M4.2b's asymmetry rather than an oversight:
    /// **collections are per (file, copy)** — a rating describes the image you made — while
    /// **keywords are per file**, because a keyword describes what is *in* the picture, which
    /// every copy shares. **Do not unify them.**
    ///
    /// ⚠ `keyword_members` already expands *descendants* through its recursive CTE, so inserting
    /// the queried keyword's own id for each member is exactly the "matches an ancestor"
    /// semantics `Criterion::Keyword` documents.
    #[allow(clippy::type_complexity)]
    fn memberships(
        &self,
    ) -> (
        std::collections::HashMap<PhotoId, Vec<i64>>,
        std::collections::HashMap<PathBuf, Vec<i64>>,
    ) {
        let mut colls: std::collections::HashMap<PhotoId, Vec<i64>> = Default::default();
        let mut keys: std::collections::HashMap<PathBuf, Vec<i64>> = Default::default();
        let Some(cat) = &self.catalog else {
            return (colls, keys);
        };
        for c in cat.collections().unwrap_or_default() {
            for (path, copy) in cat.collection_members(c.id).unwrap_or_default() {
                colls
                    .entry(PhotoId::new(path, copy))
                    .or_default()
                    .push(c.id);
            }
        }
        for k in cat.keywords().unwrap_or_default() {
            for (path, _copy) in cat.keyword_members(k.id).unwrap_or_default() {
                let e = keys.entry(path).or_default();
                if !e.contains(&k.id) {
                    e.push(k.id);
                }
            }
        }
        (colls, keys)
    }

    /// Show the photos a saved rule matches.
    ///
    /// ⚠⚠ **Re-evaluated on every open, never cached as a member list.** A photo rated *into* the
    /// rule must appear without a restart — the same reason `open_collection` re-reads its members
    /// rather than caching them.
    ///
    /// ⚠ Evaluated in memory over every photo the catalog knows, which is what the `Text`
    /// criterion forces: it reads a copy's own name from the **sidecar**, which SQL cannot see.
    /// Measured at 4.6 ms per 830 sidecars, so ~60 ms at 10,000.
    fn open_smart(&mut self, ctx: &egui::Context, id: i64, name: &str) {
        let Some(cat) = &self.catalog else {
            self.status = "No catalog".into();
            return;
        };
        let Some(row) = cat
            .smart_collections()
            .unwrap_or_default()
            .into_iter()
            .find(|r| r.id == id)
        else {
            self.status = format!("“{name}” is gone");
            return;
        };
        // ⚠ A rule that will not parse yields an **empty** collection and a message, never a
        // panic: the text is user-editable, in a file, by hand.
        let rule: rustroom_app::rule::Expr = match serde_json::from_str(&row.rule) {
            Ok(r) => r,
            Err(e) => {
                self.status = format!("“{name}” has an unreadable rule: {e}");
                return;
            }
        };

        let rows = cat.all_photos().unwrap_or_default();
        let mtimes: std::collections::HashMap<PathBuf, std::time::SystemTime> = rows
            .iter()
            .filter_map(|r| {
                r.mtime.map(|s| {
                    (
                        r.path.clone(),
                        std::time::SystemTime::UNIX_EPOCH
                            + std::time::Duration::from_secs(s.max(0) as u64),
                    )
                })
            })
            .collect();
        let sizes: std::collections::HashMap<PathBuf, i64> = Default::default();
        let members: Vec<(PathBuf, u32)> = rows.into_iter().map(|r| (r.path, r.copy)).collect();
        let scan = rustroom_app::collections::scan_of(&members, sidecar::edited_at, |p| {
            mtimes
                .get(p)
                .copied()
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        });
        let all = entries::build(&scan, sidecar::load_all);

        // Facts the evaluator needs, gathered once rather than per entry.
        let (colls, keys) = self.memberships();
        let Some(cat) = &self.catalog else { return };
        let mut marks: std::collections::HashMap<PhotoId, catalog::db::Marks> = Default::default();
        let mut captures: std::collections::HashMap<PathBuf, core_image::Capture> =
            Default::default();
        let mut folders: Vec<PathBuf> = Vec::new();
        for e in &all {
            if let Some(dir) = e.id.path.parent() {
                if !folders.iter().any(|f| f == dir) {
                    folders.push(dir.to_path_buf());
                }
            }
        }
        for dir in &folders {
            for (path, copy, m) in cat.marks_in(dir).unwrap_or_default() {
                marks.insert(PhotoId::new(path, copy), m);
            }
            for (path, c) in cat.captures_in(dir).unwrap_or_default() {
                captures.insert(path, c);
            }
        }

        let empty: Vec<i64> = Vec::new();
        let keep: Vec<PathBuf> = all
            .iter()
            .filter(|e| {
                let capture = captures.get(&e.id.path).cloned().unwrap_or_default();
                let facts = rustroom_app::rule::Facts {
                    entry: e,
                    marks: marks.get(&e.id).copied().unwrap_or_default(),
                    capture: &capture,
                    collections: colls.get(&e.id).unwrap_or(&empty),
                    keywords: keys.get(&e.id.path).unwrap_or(&empty),
                    size: sizes.get(&e.id.path).copied(),
                };
                rule.matches(&facts)
            })
            .map(|e| e.id.path.clone())
            .collect();

        // ⚠ One entry per **file**: `entries::build` expands copies itself, so passing copies
        // through `scan_of` would double every one of them.
        let mut kept: Vec<(PathBuf, u32)> = Vec::new();
        for p in keep {
            if !kept.iter().any(|(q, _)| q == &p) {
                kept.push((p, 0));
            }
        }
        let mut scan = rustroom_app::collections::scan_of(&kept, sidecar::edited_at, |p| {
            mtimes
                .get(p)
                .copied()
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        });
        catalog::sort(&mut scan, self.settings.library_sort);
        let n = self.open_scan(
            ctx,
            LibrarySource::Smart {
                id,
                name: name.to_string(),
            },
            scan,
        );
        self.status = if n == 0 {
            format!("“{name}” matches nothing")
        } else {
            format!("“{name}” — {n} photos")
        };
    }

    /// Open a folder named by the catalog, re-scanning it.
    ///
    /// ⚠⚠ **A folder row re-scans; it does not browse from the catalog.** "One idiom for the whole
    /// panel" was the first instinct and it is wrong here: collections and keywords *span* folders
    /// and genuinely cannot scan, while a folder row names one real directory and can. So there is
    /// no such thing as a stale panel-browsed folder, refreshing needs no new button (click the
    /// row again), and `Reindex` keeps working because the source is a `Folder`.
    ///
    /// ⚠ An unreadable directory falls back to the catalog's last-seen rows rather than showing an
    /// empty folder. An unplugged drive must not look like the folder emptied itself — the rule
    /// `sync_folder` already follows in never deleting a photo row.
    fn open_catalog_folder(&mut self, ctx: &egui::Context, dir: &std::path::Path) {
        if catalog::scan_folder(dir).is_ok() {
            self.open_folder_at(ctx, dir.to_path_buf());
            return;
        }
        let Some(cat) = &self.catalog else {
            self.status = "No catalog".into();
            return;
        };
        let names = cat.photos_in(dir).unwrap_or_default();
        let members: Vec<(PathBuf, u32)> = names.into_iter().map(|n| (dir.join(n), 0)).collect();
        let mut scan =
            rustroom_app::collections::scan_of(&members, sidecar::edited_at, fs_modified);
        catalog::sort(&mut scan, self.settings.library_sort);
        let n = self.open_scan(ctx, LibrarySource::Folder(dir.to_path_buf()), scan);
        self.status = format!(
            "{} is unreadable — showing {n} photos last seen",
            dir.display()
        );
    }

    /// Remove a folder from the catalog.
    ///
    /// ⚠ Deletes catalog rows only — the photos, their sidecars and every edit in them are
    /// untouched. Re-opening the folder catalogues it again, minus the library metadata.
    fn forget_folder(&mut self, ctx: &egui::Context, id: i64, path: &std::path::Path) {
        if let Some(cat) = &self.catalog {
            match cat.forget_folder(id) {
                // ⚠ Pushed so the deletion is undoable. This is the destructive one — it takes
                // the ratings, flags, labels and keyword assignments with it.
                Ok(snap) => {
                    let open = self.open_photo();
                    self.actions.push(
                        ActionKind::CatalogDelete {
                            snapshot: Box::new(snap),
                        },
                        open.as_ref(),
                    );
                }
                Err(e) => {
                    self.status = format!("Could not forget the folder: {e}");
                    return;
                }
            }
        }
        // ⚠ What is on screen may now be rows that exist nowhere. Two cases, and both need
        // handling: the folder itself, and All Photos, which was showing some of its photos.
        match self.library.as_ref().map(|l| l.source.clone()) {
            Some(LibrarySource::Folder(p)) if p == path => {
                // ⚠ Closed rather than left showing rows the catalog no longer has — the rule the
                // collection and keyword deletions already follow.
                self.library = None;
            }
            Some(LibrarySource::AllPhotos) => self.open_all_photos(ctx),
            _ => {}
        }
        self.status = format!("Forgot “{}”", path.display());
    }

    /// Build and show a `Library` from an already-sorted scan, returning how many rows it holds.
    ///
    /// ⚠⚠ **The one place a `Library` is constructed.** Three loaders had grown three copies of
    /// these fifteen lines between them, and a fourth was the point at which they became one — a
    /// new field on `Library` had to be added in three places, which is how two of them drift.
    ///
    /// ⚠ One row per (file, copy). Reading every sidecar costs 4.6 ms for 830 files, measured —
    /// small enough to stay inline rather than moving to the loader thread.
    fn open_scan(
        &mut self,
        ctx: &egui::Context,
        source: LibrarySource,
        scan: Vec<catalog::CatalogEntry>,
    ) -> usize {
        let entries = entries::build(&scan, sidecar::load_all);
        let n = entries.len();
        let ctx2 = ctx.clone();
        let space = self
            .settings
            .effective_display_space(self.os_manages_colour);
        let loader = ThumbLoader::new(space, move || ctx2.request_repaint());
        self.library = Some(Library {
            source,
            scan,
            entries,
            cache: rustroom_app::thumb_cache::ThumbCache::with_default_budget(),
            last_want: Vec::new(),
            selection: Selection::default(),
            loader,
            visible: (0..n).collect(),
            marks: std::collections::HashMap::new(),
            captures: std::collections::HashMap::new(),
            band: None,
        });
        // ⚠ Applied explicitly rather than left to `reload_marks`, which early-returns when there
        // is no catalog — the filter must hold across a source switch whether or not one is open.
        let filter = self.filter.clone();
        if let Some(lib) = self.library.as_mut() {
            lib.refilter(&filter);
        }
        self.reload_marks();
        self.reload_captures();
        self.queue_metadata();
        self.view = AppView::Library;
        n
    }

    /// Queue whatever this source's folders have not had their capture metadata read yet.
    ///
    /// ⚠ Called on load only, never per frame — the `apply_view_demosaic` rule. The scanner
    /// de-duplicates anyway, but the query would not.
    ///
    /// ⚠ One query per **folder**, because a collection or All Photos spans several — the same
    /// shape `reload_marks` uses, and for the same reason.
    fn queue_metadata(&mut self) {
        let Some(cat) = &self.catalog else { return };
        let Some(lib) = self.library.as_ref() else {
            return;
        };
        let mut folders: Vec<PathBuf> = Vec::new();
        for e in &lib.entries {
            if let Some(dir) = e.id.path.parent() {
                if !folders.iter().any(|f| f == dir) {
                    folders.push(dir.to_path_buf());
                }
            }
        }
        let mut want: Vec<PathBuf> = Vec::new();
        for dir in &folders {
            if let Ok(names) = cat.files_without_capture(dir) {
                want.extend(names.into_iter().map(|n| dir.join(n)));
            }
        }
        if !want.is_empty() {
            self.metadata.scan(&want);
        }
    }

    fn open_entry(&mut self, ctx: &egui::Context, index: usize) {
        let id = match self.library.as_mut() {
            Some(lib) => match lib.entries.get(index) {
                Some(e) => {
                    lib.selection.click(index);
                    e.id.clone()
                }
                None => return,
            },
            None => return,
        };
        self.open_path(ctx, id);
        self.view = AppView::Develop;
    }

    /// Change how the browser orders a folder, keeping the photo you were looking at selected.
    ///
    /// ⚠ Guarded on an actual change, because the combo's body runs every frame — the
    /// `apply_view_demosaic` rule. Without it a folder of hundreds would be re-sorted and its
    /// entries rebuilt sixty times a second.
    fn apply_sort(&mut self, order: catalog::SortOrder) {
        if self.settings.library_sort == order {
            return;
        }
        self.settings.library_sort = order;
        self.save_settings();
        // ⚠ Sorting renumbers every index, and `Selection` holds indices. `rebuild_entries`
        // re-derives the selection from a `PhotoId`, which survives a reorder; keeping the old
        // indices would leave a different photo selected.
        let keep = self.open_photo().or_else(|| {
            let lib = self.library.as_ref()?;
            lib.entries
                .get(lib.selection.active()?)
                .map(|e| e.id.clone())
        });
        self.rebuild_entries(keep.as_ref());
    }

    /// Show a collection's photos, from wherever they live.
    ///
    /// ⚠⚠ The members are re-read from the catalog every time rather than cached: a photo added
    /// from another view must appear here without a restart.
    fn open_collection(&mut self, ctx: &egui::Context, id: i64, name: &str) {
        let Some(cat) = &self.catalog else {
            self.status = "No catalog".into();
            return;
        };
        let members = match cat.collection_members(id) {
            Ok(m) => m,
            Err(e) => {
                self.status = format!("Could not read the collection: {e}");
                return;
            }
        };
        let mut scan =
            rustroom_app::collections::scan_of(&members, sidecar::edited_at, fs_modified);
        catalog::sort(&mut scan, self.settings.library_sort);
        let n = self.open_scan(
            ctx,
            LibrarySource::Collection {
                id,
                name: name.to_string(),
            },
            scan,
        );
        self.status = if n == 0 {
            format!("“{name}” is empty")
        } else {
            format!("“{name}” — {n} photos")
        };
    }

    /// Show every photo carrying a keyword or any of its descendants.
    ///
    /// ⚠ The same shape as `open_collection`, deliberately: both are "a set of photos from
    /// anywhere", and giving them two different loaders is how two idioms grow for one concept.
    fn open_keyword(&mut self, ctx: &egui::Context, id: i64, name: &str) {
        let Some(cat) = &self.catalog else {
            self.status = "No catalog".into();
            return;
        };
        let members = match cat.keyword_members(id) {
            Ok(m) => m,
            Err(e) => {
                self.status = format!("Could not read the keyword: {e}");
                return;
            }
        };
        let mut scan =
            rustroom_app::collections::scan_of(&members, sidecar::edited_at, fs_modified);
        catalog::sort(&mut scan, self.settings.library_sort);
        let n = self.open_scan(
            ctx,
            LibrarySource::Keyword {
                id,
                name: name.to_string(),
            },
            scan,
        );
        self.status = if n == 0 {
            format!("Nothing tagged “{name}”")
        } else {
            format!("“{name}” — {n} photos")
        };
    }

    /// Which keyword the Library is showing, if it is showing one.
    fn selected_keyword(&self) -> Option<i64> {
        match self.library.as_ref().map(|l| &l.source) {
            Some(LibrarySource::Keyword { id, .. }) => Some(*id),
            _ => None,
        }
    }

    /// Which collection the Library is showing, if it is showing one.
    fn selected_collection(&self) -> Option<i64> {
        match self.library.as_ref().map(|l| &l.source) {
            Some(LibrarySource::Collection { id, .. }) => Some(*id),
            _ => None,
        }
    }

    /// Re-read the marks for whatever the Library is showing.
    ///
    /// ⚠ Reads the marks of every *folder* represented, because a collection spans folders. One
    /// query per folder, never per photo.
    fn reload_marks(&mut self) {
        let Some(cat) = &self.catalog else { return };
        let Some(lib) = self.library.as_ref() else {
            return;
        };
        let mut folders: Vec<PathBuf> = Vec::new();
        for e in &lib.entries {
            if let Some(dir) = e.id.path.parent() {
                if !folders.iter().any(|f| f == dir) {
                    folders.push(dir.to_path_buf());
                }
            }
        }
        let mut map = std::collections::HashMap::new();
        for dir in &folders {
            if let Ok(rows) = cat.marks_in(dir) {
                for (path, copy, m) in rows {
                    map.insert(PhotoId::new(path, copy), m);
                }
            }
        }
        let filter = self.filter.clone();
        if let Some(lib) = self.library.as_mut() {
            lib.marks = map;
            // ⚠ Refilter here, or a newly-rated photo would not appear or disappear under an
            // active star filter until some unrelated action happened to rebuild.
            lib.refilter(&filter);
        }
    }

    /// Re-read the capture metadata for whatever the Library is showing.
    ///
    /// ⚠ One query per **folder** represented, never one per photo — the shape `reload_marks`
    /// uses, and for the same reason.
    ///
    /// ⚠⚠ Must run after the metadata scanner drains as well as wherever `reload_marks` does, or
    /// a newly-scanned folder does not become filterable until something unrelated rebuilds. The
    /// `reload_marks` refilter rule, one layer along.
    fn reload_captures(&mut self) {
        let Some(cat) = &self.catalog else { return };
        let Some(lib) = self.library.as_ref() else {
            return;
        };
        let mut folders: Vec<PathBuf> = Vec::new();
        for e in &lib.entries {
            if let Some(dir) = e.id.path.parent() {
                if !folders.iter().any(|f| f == dir) {
                    folders.push(dir.to_path_buf());
                }
            }
        }
        let mut map = std::collections::HashMap::new();
        for dir in &folders {
            if let Ok(rows) = cat.captures_in(dir) {
                for (path, c) in rows {
                    map.insert(path, c);
                }
            }
        }
        let filter = self.filter.clone();
        if let Some(lib) = self.library.as_mut() {
            lib.captures = map;
            lib.refilter(&filter);
        }
    }

    /// Re-expand the Library's rows after a copy was added or removed.
    ///
    /// ⚠ Touches neither the thumbnail cache nor the loader: both are keyed by **path**, so a
    /// rebuild that only shifts indices invalidates nothing. This used to have to carry the
    /// textures across by hand, and getting it wrong greyed out the whole folder.
    fn rebuild_entries(&mut self, select: Option<&PhotoId>) {
        // ⚠ Read before the borrow: `self.settings` and `self.library` are disjoint fields, but
        // the compiler only sees that when the read happens first.
        let sort = self.settings.library_sort;
        // ⚠ Read before the borrow: `self.filter` and `self.library` are disjoint fields, but the
        // compiler only sees that when the read happens first — the rule this file already states
        // for `self.settings`.
        let filter = self.filter.clone();
        let Some(lib) = self.library.as_mut() else {
            return;
        };
        // ⚠ Re-sorted every rebuild rather than only when the order changes: a rename moves a
        // photo's place in a name ordering, and a rebuild is exactly when that has to be seen.
        //
        // ⚠ The edit times are refreshed here too, and **only for the orders that use them** —
        // the other four would be paying a stat per file for nothing.
        if sort.needs_edited() {
            for e in lib.scan.iter_mut() {
                e.edited = sidecar::edited_at(&e.path);
            }
        }
        catalog::sort(&mut lib.scan, sort);
        lib.entries = entries::build(&lib.scan, sidecar::load_all);

        // ⚠ The selection is by *index*, and the indices have just shifted — so it is re-derived
        // from the photo we mean rather than kept. Keeping it would silently select whichever
        // row happened to land on the old number.
        lib.selection.clear();
        if let Some(id) = select {
            if let Some(i) = lib.entries.iter().position(|e| &e.id == id) {
                lib.selection.click(i);
            }
        }
        lib.refilter(&filter);
        // ⚠ The indices have just shifted, so the marks map must be re-read against the new
        // entries — and `reload_marks` refilters, which the rebuild needs anyway.
        self.reload_marks();
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_filter_survives_a_source_switch() {
        // ⚠⚠ Browsing **replaces** the source, so filter-then-pick-a-date is the only way to say
        // "five-star picks from the 9th". Rebuilding the filter on every switch destroyed exactly
        // the composition that browse-replaces-source otherwise denies — which is why the original
        // reset, argued for on the grounds that a stale filter "reads as breakage", was wrong in
        // use.
        let dir = test_app::TestDir::new("filterkeep");
        std::fs::write(dir.0.join("a.RAF"), b"x").unwrap();
        std::fs::write(dir.0.join("b.RAF"), b"x").unwrap();
        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();

        app.open_catalog_folder(&ctx, &dir.0);
        assert_eq!(app.library.as_ref().unwrap().visible.len(), 2);

        app.filter.text = "a.RAF".into();
        let f = app.filter.clone();
        app.library.as_mut().unwrap().refilter(&f);
        assert_eq!(app.library.as_ref().unwrap().visible.len(), 1);

        // Re-open the same folder — a source switch by any other name.
        app.open_catalog_folder(&ctx, &dir.0);
        assert_eq!(
            app.filter.text, "a.RAF",
            "the search text was reset by a source switch"
        );
        assert_eq!(
            app.library.as_ref().unwrap().visible.len(),
            1,
            "the filter was not applied to the new source"
        );
    }

    #[test]
    fn a_catalog_folder_row_rescans_the_directory() {
        // ⚠⚠ The whole reason a folder row differs from a collection or keyword row, and the
        // reason there is no "Refresh" button: browsing a folder from the panel *is* scanning it.
        // Without this the row would show whatever the catalog last recorded, and a file added
        // outside Rustroom would never appear — which reads as a bug.
        //
        // ⚠ Written because the sabotage step found nothing covered it: forcing
        // `open_catalog_folder` down its catalog-fallback path left the whole suite green.
        let dir = test_app::TestDir::new("rescan");
        std::fs::write(dir.0.join("a.RAF"), b"x").unwrap();
        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();

        app.open_catalog_folder(&ctx, &dir.0);
        assert_eq!(
            app.library.as_ref().unwrap().entries.len(),
            1,
            "the folder did not load"
        );

        std::fs::write(dir.0.join("b.RAF"), b"x").unwrap();
        app.open_catalog_folder(&ctx, &dir.0);
        assert_eq!(
            app.library.as_ref().unwrap().entries.len(),
            2,
            "the row did not re-scan — a file added outside Rustroom stayed invisible"
        );
        // ⚠ And the source must stay a `Folder`, or `Reindex` would start refusing on it.
        assert!(matches!(
            app.library.as_ref().unwrap().source,
            LibrarySource::Folder(_)
        ));
    }

    #[test]
    fn the_before_view_keeps_the_geometry_and_drops_the_adjustments() {
        // ⚠⚠ The property the whole before/after rework rests on. A *fully* neutral render is
        // uncropped, so it has different dimensions from the edited one — the region and
        // placement maths would then describe different content and toggling while zoomed
        // would jump elsewhere in the photo. Keeping the framing is what makes the comparison
        // a comparison.
        let mut edits = Edits {
            exposure: 1.5,
            ..Default::default()
        };
        edits.geometry.crop.left = 0.25;
        edits.geometry.angle = 3.0;
        edits.vignette.amount = -40.0;

        let before = super::before_edits(&edits);
        assert_eq!(before.geometry.crop.left, 0.25, "the crop must survive");
        assert_eq!(before.geometry.angle, 3.0, "the straighten must survive");
        assert_eq!(before.exposure, 0.0, "the adjustments must not");
        assert_eq!(before.vignette.amount, 0.0);
    }

    /// A `Decoded` with nothing behind it, so `finalize_session` can be driven directly.
    /// ⚠⚠ Drives the **real** `finalize_session`, not a hand-assigned field. Read once when the
    /// photo opens and never in a panel body — the `apply_view_demosaic` rule; `Session::name`
    /// caches the copy name from the same envelope for the same reason.
    #[test]
    fn opening_a_photo_caches_its_snapshots_and_its_name() {
        let dir = test_app::TestDir::new("sessionsnaps");
        let photo = dir.0.join("a.RAF");
        std::fs::write(&photo, b"not a real raw").unwrap();

        let mut sc = rustroom_app::sidecar::Sidecar::default();
        sc.set_edits(1, Edits::default());
        sc.set_name(1, Some("Warm".to_string()));
        sc.add_snapshot(
            1,
            rustroom_app::sidecar::Snapshot {
                n: 1,
                name: Some("Before the crop".to_string()),
                taken: 0,
                edits: Edits::default(),
            },
        );
        rustroom_app::sidecar::save_all(&photo, &sc).unwrap();

        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();
        app.finalize_session(
            &ctx,
            PhotoId::new(photo.clone(), 1),
            test_app::fake_decode(),
        );

        let s = app.session.as_ref().expect("the session must exist");
        assert_eq!(s.name.as_deref(), Some("Warm"), "the name still caches");
        assert_eq!(s.snapshots.len(), 1, "and so do the snapshots");
        assert_eq!(s.snapshots[0].label(), "Before the crop");
    }

    /// ⚠ Snapshots are **per copy**: opening copy 0 must not show copy 1's.
    #[test]
    fn a_session_caches_only_its_own_copys_snapshots() {
        let dir = test_app::TestDir::new("percopysnaps");
        let photo = dir.0.join("a.RAF");
        std::fs::write(&photo, b"not a real raw").unwrap();

        let mut sc = rustroom_app::sidecar::Sidecar::default();
        sc.set_edits(0, Edits::default());
        sc.add_snapshot(
            1,
            rustroom_app::sidecar::Snapshot {
                n: 1,
                name: None,
                taken: 0,
                edits: Edits::default(),
            },
        );
        rustroom_app::sidecar::save_all(&photo, &sc).unwrap();

        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();
        app.finalize_session(
            &ctx,
            PhotoId::original(photo.clone()),
            test_app::fake_decode(),
        );

        assert!(
            app.session.as_ref().unwrap().snapshots.is_empty(),
            "copy 0 has none of its own"
        );
    }

    /// ⚠⚠ **Requesting a render must not move the guide overlay.** `shown_geo` describes the
    /// texture on screen and may only advance when a frame actually lands; `pending_geo` is what
    /// was asked for. Collapsing the two puts guides at their new positions over the previous
    /// image — reported as the line jumping on release when fully zoomed in, where a perspective
    /// change drops the prefix cache and the re-render takes hundreds of milliseconds.
    #[test]
    fn a_render_request_records_what_was_asked_for_without_moving_the_overlay() {
        let ctx = crate::test_ui::ctx();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 60);
        let s = app.session.as_mut().unwrap();
        s.shown_geo = (edit_model::Perspective::default(), 0.0);

        let mut e = s.edits.clone();
        e.perspective.vertical = 40.0;
        e.geometry.angle = 2.5;
        s.request(e, 512, None);

        assert_eq!(
            s.pending_geo.get(),
            (
                edit_model::Perspective {
                    vertical: 40.0,
                    ..Default::default()
                },
                2.5
            ),
            "the request must be recorded"
        );
        assert_eq!(
            s.shown_geo,
            (edit_model::Perspective::default(), 0.0),
            "but what is on screen has not changed yet"
        );
    }
}
