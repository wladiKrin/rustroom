use catalog::CatalogEntry;
use edit_model::EditGroups;
use edit_model::Edits;
use eframe::egui;
use rustroom_app::action_log::{ActionKind, ActionLog, SyncAction};
use rustroom_app::batch_export::{ExportEvent, ExportOptions, ExportReport, ExportWorker};
use rustroom_app::buffer_cache::BufferCache;
use rustroom_app::crop_tool;
use rustroom_app::decode_worker::{DecodeWorker, Decoded};
use rustroom_app::edit_clipboard;
use rustroom_app::edit_history::HistoryStash;
use rustroom_app::export_presets::{ExportPreset, ExportPresetStore};
use rustroom_app::format_filter::{self, FormatFilter};
use rustroom_app::full_res;
use rustroom_app::history::History;
use rustroom_app::mask_tool;
use rustroom_app::presets::{Preset, PresetStore};
use rustroom_app::render_engine::{FrameSpec, RegionRect, RenderEngine};
use rustroom_app::selection::Selection;
use rustroom_app::settings::{Accent, Settings, SettingsStore, ThemeChoice};
use rustroom_app::sidecar;
use rustroom_app::sync;
use rustroom_app::thumb_loader::ThumbLoader;
use rustroom_app::tool::Tool;
use rustroom_app::view_region;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

mod commands;
mod develop;
#[cfg(target_os = "macos")]
mod display_colorspace;
mod filmstrip;
mod library;
mod menu_bar;
mod modals;
#[cfg(target_os = "macos")]
mod native_menu;
mod shell;
mod theme;
mod thumb;

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
struct Library {
    entries: Vec<CatalogEntry>,
    thumbs: Vec<Option<egui::TextureHandle>>, // parallel to entries; None = still loading
    selection: Selection,
    loader: ThumbLoader,
    /// Which formats the browser hides. Reset when a folder is loaded.
    filter: FormatFilter,
    /// Indices into `entries` that pass `filter`, ascending. Derived — recomputed by
    /// `refilter`, never edited directly.
    visible: Vec<usize>,
}

impl Library {
    /// Recompute `visible` and prune the selection to it.
    ///
    /// ⚠ Both halves, always. Recomputing the visible list without pruning leaves the
    /// selection holding photos that are not on screen, which a sync or a paste would then
    /// write to.
    fn refilter(&mut self) {
        self.visible = self.filter.visible_indices(&self.entries);
        self.selection.retain_visible(&self.visible);
    }
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
        Self {
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

/// Decode a file for editing (half-size for RAW; full decode for images) into a linear
/// `ImageBuf`, a display label, and the buffer's size relative to a full-resolution
/// decode. Dispatches by file kind.
fn decode_for_edit(
    path: &std::path::Path,
    profile: Option<std::path::PathBuf>,
) -> Result<(core_image::ImageBuf, String, f32), String> {
    if catalog::is_raw(path) {
        let opts = raw_decode::DecodeOptions {
            camera_profile: profile,
            ..raw_decode::DecodeOptions::preview()
        };
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

/// What choosing `tier` for the 1:1 view costs, relative to the default.
///
/// ⚠ A **multiplier, not seconds.** The export dialog says "roughly 1.2 s to 11 s per photo",
/// which are X-T4 numbers written into prose and wrong on any other body. Ratios are the
/// stable part, and they matter more here because this cost is *recurring*: the
/// full-resolution decode is eager, so a slower tier is paid on every photo you stop on,
/// not once per export.
///
/// Derived from the measured end-to-end decodes — 1.25 / 2.89 / 6.9 / 11.42 s — recorded in
/// `docs/superpowers/specs/2026-08-11-demosaic-backend-design.md`.
///
/// Lives here rather than on `raw_decode::Demosaic` because a cost measured on one camera is
/// presentation, not a property of the algorithm; `raw-decode` exposes the neutral facts.
fn view_quality_hint(tier: raw_decode::Demosaic) -> &'static str {
    match tier {
        raw_decode::Demosaic::Fastest => "about 0.4× the decode time of Fast",
        raw_decode::Demosaic::Fast => "the default",
        raw_decode::Demosaic::Better => "about 2.4× the decode time of Fast",
        raw_decode::Demosaic::Best => "about 4× the decode time of Fast",
    }
}

/// Decode a RAW at **full** resolution, for the true-1:1 view, at `tier`.
///
/// The tier comes from `Settings::view_demosaic`, not from any export preset — see that
/// field's doc comment for why mirroring an export preset was rejected.
fn decode_full_res(
    path: &std::path::Path,
    tier: raw_decode::Demosaic,
    profile: Option<std::path::PathBuf>,
) -> Result<(core_image::ImageBuf, String, f32), String> {
    let opts = raw_decode::DecodeOptions {
        demosaic: tier,
        camera_profile: profile,
        ..raw_decode::DecodeOptions::linear_scene()
    };
    let d = raw_decode::decode_raw(path, &opts).map_err(|e| e.to_string())?;
    let label = format!("{} {}", d.metadata.make, d.metadata.model);
    Ok((d.image, label, opts.source_scale()))
}

/// Build the full-resolution decode worker for `tier`.
///
/// ⚠ The tier is **captured** here rather than passed per request. `DecodeWorker` is shared
/// with the preview decode, where demosaic is inert — libraw skips it entirely for a
/// half-size decode — so putting a tier on `DecodeWorker::request` would impose a parameter
/// on a caller that cannot act on it. Changing the setting therefore rebuilds this worker;
/// dropping the old one disconnects its channel, so a decode in flight finishes and discards
/// its own result, the same abandon-don't-abort trade the full-resolution handover makes.
/// ⚠ The tier and the profile are **captured**, not passed per request: `DecodeWorker`'s callers
/// have no business knowing about either, and rebuilding the worker is what makes a change take
/// effect. Dropping the old one disconnects its channel, so an in-flight decode discards its own
/// result — the abandon-don't-abort trade used at handover.
fn full_res_worker(
    tier: raw_decode::Demosaic,
    profile: Option<std::path::PathBuf>,
    ctx: &egui::Context,
) -> DecodeWorker {
    let ctx = ctx.clone();
    DecodeWorker::new(
        move |p| decode_full_res(p, tier, profile.clone()),
        MAX_CONCURRENT_FULL_DECODES,
        move || ctx.request_repaint(),
    )
}

/// Full-resolution decode for export. Dispatches by file kind.
fn decode_for_export(
    path: &std::path::Path,
    demosaic: raw_decode::Demosaic,
    profile: Option<std::path::PathBuf>,
) -> Result<core_image::ImageBuf, String> {
    if catalog::is_raw(path) {
        raw_decode::decode_raw(
            path,
            &raw_decode::DecodeOptions {
                demosaic,
                camera_profile: profile,
                ..raw_decode::DecodeOptions::linear_scene()
            },
        )
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
    /// The open photo, which is what the action log's photo-edit entries must match to be
    /// undoable right now.
    ///
    /// ⚠ Returns an owned `PathBuf` rather than a borrow: every caller passes it *into* a
    /// `&mut self.actions` method, and a borrow of `self` living across that call does not
    /// compile. One clone per user gesture is not worth fighting the borrow checker over.
    fn open_photo(&self) -> Option<PathBuf> {
        self.session.as_ref().map(|s| s.path.clone())
    }

    fn persist_current_session(&self) {
        if let Some(s) = &self.session {
            // `persist`, not `save`: neutral edits *delete* the sidecar. Skipping the write
            // would leave a stale one behind, so "Reset all" survived only until the next
            // switch away and back.
            let _ = sidecar::persist(&s.path, &s.edits);
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
        let open = self.session.as_ref().map(|s| s.path.clone());
        let mut parts: Vec<ActionKind> = Vec::new();
        let mut touched = 0usize;
        let mut failed = 0usize;

        if into_session {
            touched += 1;
            if self.apply_edits_to_session(src, groups) {
                // `into_session` is only true when a session exists, so this `if let` always
                // takes its branch — it is here because the compiler cannot know that.
                if let Some(path) = open.clone() {
                    parts.push(ActionKind::PhotoEdit { path });
                }
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
            1 => self.actions.push(parts.remove(0), open.as_deref()),
            _ => self.actions.push(ActionKind::Batch(parts), open.as_deref()),
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
        let open = self.open_photo();
        self.actions.push(
            ActionKind::Sync(Box::new(SyncAction {
                source,
                groups,
                prior: report.prior,
            })),
            open.as_deref(),
        );
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
        let tool = self.tool;
        if let Some(s) = &mut self.session {
            s.edits = edits;
            // The old history described a state that is no longer on disk.
            s.history = History::new(s.edits.clone());
            s.request(s.view_edits(tool), s.render_max_dim(), s.requested_region);
        }
        // That photo's history was just replaced, so its entries point at steps it no longer
        // holds. Every other photo's stack is untouched and stays.
        self.actions.forget_edits_of(&open);
    }

    /// Re-render after the session's edits were changed by an undo or a redo.
    fn rerender_current(&mut self) {
        let tool = self.tool;
        let Some(s) = &mut self.session else { return };
        s.edits = s.history.get().clone();
        s.request(s.view_edits(tool), s.render_max_dim(), s.requested_region);
    }

    /// Undo the most recent action, whichever kind it was.
    fn undo_action(&mut self) {
        let open = self.open_photo();
        if let Some(kind) = self.actions.undo(open.as_deref()) {
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
            ActionKind::PhotoEdit { .. } => {
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
        let open = self.open_photo();
        if let Some(kind) = self.actions.redo(open.as_deref()) {
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
            ActionKind::PhotoEdit { .. } => {
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

        if copy {
            self.copy_settings(ctx);
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

    /// Copy the open photo's edits to the clipboard, in memory and on the system clipboard.
    ///
    /// ⚠ One implementation for the menu item, the toolbar button and Ctrl+C. They used to be
    /// two, which is how a button and its shortcut drift apart.
    ///
    /// ⚠ `self.session.as_ref().map(..)` rather than `if let Some(s) = &self.session`: the
    /// latter holds an immutable borrow of `self` across the assignment and does not compile.
    fn copy_settings(&mut self, ctx: &egui::Context) {
        if self.session.is_none() {
            return;
        }
        self.clipboard = self.session.as_ref().map(|s| s.edits.clone());
        if let Some(edits) = &self.clipboard {
            ctx.copy_text(edit_clipboard::encode(edits));
        }
        self.status = "Settings copied".to_string();
    }

    /// Apply the in-memory clipboard to the selection with the last-used groups, no dialog.
    fn paste_last_groups(&mut self) {
        let Some(src) = self.clipboard.clone() else {
            return;
        };
        let (n, failed) = self.apply_to_selection(&src, self.paste_groups);
        self.status = paste_status(n, failed);
    }

    /// Reset the five adjustment groups the Edit tool shows, leaving geometry and masks alone.
    ///
    /// ⚠ Scoped, for the reason the button's own comment gives: under an exclusive panel each
    /// tool resets its own domain and must not discard a crop or a mask the user cannot see.
    fn reset_adjustments(&mut self) {
        let tool = self.tool;
        let Some(s) = &mut self.session else { return };
        s.edits.merge_from(
            &Edits::default(),
            develop::panel::adjustments::edit_groups(),
        );
        s.request(s.view_edits(tool), s.render_max_dim(), s.requested_region);
        if s.history.push(s.edits.clone()) {
            if let Some(path) = self.open_photo() {
                self.actions.push(
                    ActionKind::PhotoEdit { path: path.clone() },
                    Some(path.as_path()),
                );
            }
        }
    }

    /// Select a rail tool, re-rendering when it actually changed.
    ///
    /// ⚠ The re-render is not optional and it lives here rather than at each call site:
    /// `preview_edits` strips the crop in geometry mode, so entering or leaving Crop without a
    /// request leaves the overlay drawn over a *cropped* preview — the M3.1a
    /// `draw_mask_overlay` bug. The rail, the 1–4 keys and the Photo menu all come through
    /// here, so none of them can forget it.
    fn select_tool(&mut self, t: Tool) {
        if self.tool == t {
            return;
        }
        self.tool = t;
        let tool = self.tool;
        if let Some(s) = &mut self.session {
            s.request(s.view_edits(tool), s.render_max_dim(), s.requested_region);
        }
    }

    /// Turn the photo a quarter turn, moving the crop with it.
    ///
    /// ⚠ The `CropRect` transform stays in the same function as the orientation change. They
    /// are one act: separating them is exactly how a rotated crop lands on different pixels.
    fn rotate(&mut self, clockwise: bool) {
        let tool = self.tool;
        let Some(s) = &mut self.session else { return };
        let g = &mut s.edits.geometry;
        if clockwise {
            g.orientation.rotate_cw();
            g.crop = g.crop.rotated_cw();
        } else {
            g.orientation.rotate_ccw();
            for _ in 0..3 {
                g.crop = g.crop.rotated_cw();
            }
        }
        s.request(s.view_edits(tool), s.render_max_dim(), s.requested_region);
        if s.history.push(s.edits.clone()) {
            if let Some(path) = self.open_photo() {
                self.actions.push(
                    ActionKind::PhotoEdit { path: path.clone() },
                    Some(path.as_path()),
                );
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
        // ⚠ The only place `self.session` is ever assigned, and so the only choke point the
        // stash needs: the outgoing history is put away here whether the switch came from a
        // cache hit or from a finished background decode.
        if let Some(old) = self.session.take() {
            self.histories.stash(old.path, old.history);
        }
        // ⚠ Scanned here, once per photo, rather than in the panel body which runs every frame.
        // Cheap enough per photo; ruinous sixty times a second.
        self.camera_profiles_cache = camera_profile::store::ProfileStore::user_default(
            &dirs::config_dir().unwrap_or_else(|| std::path::PathBuf::from(".")),
        )
        .list_for(&decoded.label);

        let edits = sidecar::load(&path).unwrap_or_default();
        let history = match self.histories.take_matching(&path, &edits) {
            Some(h) => h,
            None => {
                // Either this photo has no stack, or the one it had describes a state no
                // longer on disk — a sync or a paste rewrote its sidecar while it was closed.
                // Any log entry pointing at that stack would now pop steps it does not hold.
                self.actions.forget_edits_of(&path);
                History::new(edits.clone())
            }
        };

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
            engine,
            edits,
            texture: None,
            baseline,
            hist: [[0; 256]; 3],
            make_model: decoded.label.clone(),
            zoom: 1.0,
            fit_scale: 1.0,
            pan: egui::Vec2::ZERO,
            history,
            show_before: false,
            active_handle: None,
            selected_mask: None,
            active_mask_handle: None,
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
                let space = self
                    .settings
                    .effective_display_space(self.os_manages_colour);
                let loader = ThumbLoader::new(paths, space, move || ctx2.request_repaint());
                self.library = Some(Library {
                    entries,
                    thumbs: (0..n).map(|_| None).collect(),
                    selection: Selection::default(),
                    loader,
                    // A fresh `Library` per folder load, so the filter resets by construction.
                    filter: FormatFilter::default(),
                    visible: (0..n).collect(),
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

    /// Move a preset to the trash and refresh the cached list.
    fn delete_preset(&mut self, name: &str) {
        match self.preset_store.delete(name) {
            Ok(()) => {
                self.status = format!("Deleted preset “{name}” — Ctrl+Z to restore");
                let open = self.open_photo();
                self.actions.push(
                    ActionKind::PresetDelete {
                        name: name.to_string(),
                    },
                    open.as_deref(),
                );
            }
            Err(e) => self.status = format!("Could not delete preset: {e}"),
        }
        self.reload_presets();
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

    /// The Settings window. One setting today; a modal is the right weight for that.
    /// Adopt new appearance settings: persist them and re-style immediately, so the change is
    /// visible rather than deferred to the next launch.
    ///
    /// ⚠ Cheap enough to do inline — unlike `apply_view_demosaic`, nothing is decoded. It is
    /// still guarded by a change comparison at the call site, because the Settings dialog body
    /// runs every frame it is open.
    fn apply_appearance(&mut self, theme_choice: ThemeChoice, accent: Accent, ctx: &egui::Context) {
        self.settings.theme = theme_choice;
        self.settings.accent = accent;
        self.save_settings();
        theme::apply(ctx, &self.settings);
    }

    /// Adopt a new display space: persist it, reload the thumbnails so the filmstrip agrees
    /// with the canvas, and re-render the open photo so the change is visible immediately.
    ///
    /// ⚠ Thumbnails are converted **at load**, so the loader has to be rebuilt and the cached
    /// thumbnails dropped. They reload in parallel and cost about 41 ms each, which is why this
    /// is affordable to do eagerly rather than deferring to the next folder open.
    fn apply_display_space(&mut self, space: color::OutputSpace, ctx: &egui::Context) {
        self.settings.display_space = space;
        self.save_settings();
        // ⚠ The *effective* value from here on: if the OS is managing colour this is `Srgb`
        // whatever the dropdown said, so the two can never both convert.
        let space = self
            .settings
            .effective_display_space(self.os_manages_colour);
        if let Some(lib) = self.library.as_mut() {
            let paths: Vec<_> = lib.entries.iter().map(|e| e.path.clone()).collect();
            let n = paths.len();
            let ctx2 = ctx.clone();
            lib.loader = ThumbLoader::new(paths, space, move || ctx2.request_repaint());
            lib.thumbs = (0..n).map(|_| None).collect();
        }
        // The canvas is a plain re-render: nothing is decoded. `Session::request` reads the
        // new value on its way through, so it has to be written on the session first.
        let tool = self.tool;
        if let Some(s) = self.session.as_mut() {
            s.display_space = space;
            let edits = s.view_edits(tool);
            let (max_dim, region) = (s.render_max_dim(), s.requested_region);
            s.request(edits, max_dim, region);
        }
    }
    /// The profile chosen for the open photo's camera, if any.
    ///
    /// ⚠ Keyed by `Session::make_model` — libraw's make and model joined, e.g. `"Fujifilm X-T4"`.
    /// That is deliberately the same shape a profile's own `ProfileName` has (`"FUJIFILM X-T4"`),
    /// so matching compares like with like and the make disambiguates two bodies that share a
    /// model string.
    fn camera_profile_path(&self) -> Option<std::path::PathBuf> {
        let s = self.session.as_ref()?;
        self.settings.camera_profiles.get(&s.make_model).cloned()
    }

    /// Adopt a camera profile for the open photo's camera.
    ///
    /// Mirrors `apply_view_demosaic`: persist, rebuild both workers, and re-decode.
    ///
    /// ⚠ Clearing `full_pending` is load-bearing for the same reason it is there — the sequence
    /// number belongs to the worker being dropped, and leaving it blocks `maybe_request_full_res`
    /// forever.
    ///
    /// ⚠ **The buffer cache must be cleared too.** Without that, arrowing back to a photo serves
    /// the buffer decoded with the *old* profile, which looks exactly like the setting not
    /// working.
    fn apply_camera_profile(
        &mut self,
        make_model: String,
        profile: Option<std::path::PathBuf>,
        ctx: &egui::Context,
    ) {
        match &profile {
            Some(p) => self.settings.camera_profiles.insert(make_model, p.clone()),
            None => self.settings.camera_profiles.remove(&make_model),
        };
        self.save_settings();

        let ctx2 = ctx.clone();
        let for_edit = profile.clone();
        self.decoder = DecodeWorker::new(
            move |p| decode_for_edit(p, for_edit.clone()),
            MAX_CONCURRENT_DECODES,
            move || ctx2.request_repaint(),
        );
        self.full_decoder = full_res_worker(self.settings.view_demosaic, profile, ctx);
        self.full_pending = None;
        self.cache.clear();

        if let Some(s) = self.session.as_ref() {
            let path = s.path.clone();
            self.open_path(ctx, path);
        }
    }

    /// Adopt a new 1:1 view demosaic tier: persist it, rebuild the worker, and re-decode the
    /// open photo so the change is visible rather than silently deferred to the next photo.
    ///
    /// ⚠ Clearing `full_pending` is load-bearing: that sequence number belongs to the worker
    /// being dropped, and leaving it would make `pump_full_res` compare against a number no
    /// live worker will ever produce — blocking `maybe_request_full_res`, which refuses while
    /// a decode is in flight.
    fn apply_view_demosaic(&mut self, tier: raw_decode::Demosaic, ctx: &egui::Context) {
        self.settings.view_demosaic = tier;
        self.save_settings();
        self.full_decoder = full_res_worker(tier, self.camera_profile_path(), ctx);
        self.full_pending = None;
        // RAW only. For JPEG/PNG/TIFF the buffer really is the whole file, so clearing this
        // would be a lie — and `maybe_request_full_res` refuses on its own `is_raw` check
        // anyway. The old buffer stays on screen until the new one lands, so nothing flickers.
        if let Some(s) = self.session.as_mut() {
            if catalog::is_raw(&s.path) {
                s.buffer_is_full_res = false;
            }
        }
    }

    /// ←/→ move to the previous/next photo in the loaded folder — but only when no egui
    /// widget has keyboard focus, so arrows still fine-tune a focused slider.
    fn handle_develop_arrows(&mut self, ctx: &egui::Context) {
        let nav = {
            let Some(lib) = &self.library else { return };
            if lib.visible.is_empty() || ctx.memory(|m| m.focused().is_some()) {
                return;
            }
            // ⚠ `cur` may not itself be visible: hiding the open photo's format does not close
            // it, so `step` has to cope with starting from a hidden index.
            let cur = lib.selection.active().unwrap_or(0);
            ctx.input(|i| {
                if i.key_pressed(egui::Key::ArrowRight) {
                    format_filter::step(&lib.visible, cur, true)
                } else if i.key_pressed(egui::Key::ArrowLeft) {
                    format_filter::step(&lib.visible, cur, false)
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

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn every_demosaic_tier_has_a_cost_hint() {
        // ⚠ A table test rather than four assertions: a fifth tier added to `Demosaic::ALL`
        // must not ship with an empty hint in the Settings dialog, and nothing else would
        // catch that.
        for d in raw_decode::Demosaic::ALL {
            let hint = view_quality_hint(d);
            assert!(!hint.is_empty(), "{d:?} has no cost hint");
        }
    }

    #[test]
    fn the_default_tier_is_described_as_the_default() {
        // The hint is the only place the dialog says which one costs nothing extra.
        assert!(view_quality_hint(raw_decode::Demosaic::default()).contains("default"));
    }

    #[test]
    fn a_full_size_decode_is_twice_the_half_size_one() {
        // ⚠ Without this, a typo in `decode_full_res` would leave the whole feature inert but
        // entirely plausible-looking: a second engine, a second cache, a status line, and the
        // same binned pixels. Gated like the other real-decode tests; run with --release.
        let Ok(path) = std::env::var("RAW_FIXTURE") else {
            eprintln!("skipping: set RAW_FIXTURE=/abs/path.raw to run this test");
            return;
        };
        let p = std::path::Path::new(&path);
        let (half, _, half_scale) = decode_for_edit(p, None).expect("half decode");
        let (full, _, full_scale) =
            decode_full_res(p, raw_decode::Demosaic::default(), None).expect("full decode");
        assert_eq!(half_scale, 0.5);
        assert_eq!(full_scale, 1.0);
        // libraw's half-size path halves each dimension, rounding down.
        assert!(
            full.width() >= half.width() * 2 - 2 && full.width() <= half.width() * 2 + 2,
            "full {} vs half {}",
            full.width(),
            half.width()
        );
    }
}
