//! The application's state and its bootstrap — and **nothing else**.
//!
//! ⚠⚠ `main.rs` reached 4,375 lines one reasonable-seeming addition at a time. The rule that
//! replaced it: **this file holds the type definitions, `RustroomApp::new` and `fn main`.**
//! Behaviour lives in a topic module beside it. An inherent impl only has to be in the same
//! *crate*, not the same module, so a method moves with `self` staying `self` and no call site
//! changes — which is why there is no excuse for adding one here.
//!
//! | Where | What |
//! |---|---|
//! | [`session_flow`] | opening a photo, the decode handover, the full-resolution upgrade, the per-frame pumps |
//! | [`sources`] | folders, All Photos, collections, keywords |
//! | [`smart_sources`] | the two query-driven sources: dates and smart collections |
//! | [`library_state`] | the loaded library's derived state, sorting and rebuilds |
//! | [`catalog_ops`] | keywords, collection membership, marks, rules, restoring a deletion |
//! | [`copies`] / [`snapshots`] | virtual copies and naming / named saved states |
//! | [`transfer`] | copy/paste, presets and sync |
//! | [`export_flow`] | driving the export worker |
//! | [`undo`] | walking the action log |
//! | [`settings_apply`] | applying a settings change to the running app |
//! | [`interaction`] | tool, rotate, arrow and guide gestures |
//! | [`decode`] | decoding a file, and the workers that do it off the UI thread |
//!
//! egui lives in the bin modules only — `theme`, `modals`, `develop`, `library`, `filmstrip`,
//! `thumb`, `shell`. Anything testable without a window belongs in the lib (`lib.rs`).

use edit_model::EditGroups;
use edit_model::Edits;
use eframe::egui;
use rustroom_app::action_log::ActionLog;
use rustroom_app::batch_export::{ExportOptions, ExportWorker};
use rustroom_app::buffer_cache::BufferCache;
use rustroom_app::crop_tool;
use rustroom_app::decode_worker::{DecodeWorker, LensFacts};
use rustroom_app::edit_history::HistoryStash;
use rustroom_app::entries::PhotoEntry;
use rustroom_app::export_presets::{ExportPreset, ExportPresetStore};
use rustroom_app::history::History;
use rustroom_app::mask_tool;
use rustroom_app::photo_id::PhotoId;
use rustroom_app::presets::{Preset, PresetStore};
use rustroom_app::render_engine::{RegionRect, RenderEngine};
use rustroom_app::selection::Selection;
use rustroom_app::settings::{Settings, SettingsStore};
use rustroom_app::thumb_loader::{ThumbLoader, ThumbRequest};
use rustroom_app::tool::Tool;
use std::path::PathBuf;
use std::time::{Duration, Instant};

mod ai_flow;
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
mod folder_rows;
mod immich_flow;
mod immich_panel;
mod import_flow;
mod interaction;
mod keyword_ops;
mod keyword_rows;
mod library;
mod library_state;
mod list_row;
mod menu_bar;
mod modals;
mod multi;
mod name_edit;
#[cfg(target_os = "macos")]
mod native_menu;
mod preview_flow;
mod segmented;
mod session_flow;
mod settings_apply;
mod shell;
mod smart_sources;
mod snapshots;
mod sources;
#[cfg(test)]
mod test_app;
#[cfg(test)]
mod test_ui;
mod theme;
mod thumb;
mod transfer;
mod undo;

use decode::{decode_for_edit, full_res_worker};
use modals::Modal;

pub(crate) use rustroom_app::view_region::PREVIEW_MAX_DIM;
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
/// GPU memory for preview textures (M9.14): three 26-Mpx RGBA pictures, the most the on-screen
/// window (Compare's two panes plus a neighbour) ever pins.
const PREVIEW_TEXTURE_BUDGET: usize = 192 * 1024 * 1024;

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

/// Which of the Library's three layouts is on screen.
///
/// ⚠⚠ **A mode within Library, not a fourth `AppView`.** Library and Develop are *modules* —
/// the segmented switch names them and every module-level control in the toolbar is gated on
/// them. Compare and Survey are ways of looking at the same browsing source, so they compose
/// with the search field, the filters and the sort order rather than replacing them, and `G`
/// returns to the grid without leaving the module. Lightroom's own G/E/C/N are modes of its
/// Library module for exactly this reason.
///
/// ⚠ `Copy`, like `AppView`, so the toolbar's segmented control can hold one by value.
#[derive(Default, PartialEq, Eq, Clone, Copy, Debug)]
enum LibraryMode {
    #[default]
    Grid,
    /// Two photos side by side: a reference and a candidate being judged against it.
    Compare,
    /// The whole selection at once, tiled to fill the view.
    Survey,
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
    /// One Immich album, its photos cached locally on demand.
    ///
    /// ⚠ The name is carried rather than looked up: the rail may not have fetched the album list
    /// when the status line wants to say where these photos came from.
    ImmichAlbum {
        id: String,
        name: String,
    },
    /// One month of the Immich timeline.
    ///
    /// ⚠ The bucket identifier the server gave, plus the label to show. The label is carried rather
    /// than formatted on demand so the status line and the rail always agree.
    ImmichBucket {
        bucket: String,
        label: String,
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
    /// Which stacks the user has expanded, by `stack::StackKey`.
    ///
    /// ⚠ **Session state, deliberately not persisted.** Expanding a pair is a momentary "let me see
    /// the JPEG", not a preference. ⚠ It lives here rather than on `RustroomApp` so opening another
    /// folder starts collapsed — `Library` is rebuilt, and the keys carry the folder anyway.
    ///
    /// ⚠⚠ Changing it must go through `rebuild_entries`, never patch `entries` in place: the grid,
    /// the filmstrip, `selection` and `view_region` all index into that one list.
    stack_expanded: std::collections::HashSet<rustroom_app::stack::StackKey>,
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
    /// What to ask the preview loader for while the decode runs (M9.14): the camera JPEG or a
    /// rendered preview stands in for the thumbnail placeholder the moment it lands.
    preview_request: rustroom_app::preview_loader::PreviewRequest,
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
    /// Which Library layout is on screen — the grid, Compare or Survey.
    library_mode: LibraryMode,
    /// The photo Compare shows beside the reference.
    ///
    /// ⚠⚠ A `PhotoId` rather than an index, and **resolved every frame** through
    /// `compare_pair::candidate` rather than trusted. Indices shift under a sort, a filter, a
    /// rebuild and a copy being created; an id that no longer names a visible row simply falls
    /// back to the next candidate. That is why no rebuild path has to remember to clear this —
    /// the alternative is an invalidation hook at every one of them, and the one that forgets
    /// shows the wrong photograph.
    compare_candidate: Option<PhotoId>,
    /// Which of Compare's two panes carries the border, and therefore what its keys act on.
    ///
    /// ⚠ 0 is the reference, 1 the candidate. A plain index rather than a `PhotoId`, because it
    /// is a fact about the *layout* — which side you are looking at — and must survive the
    /// candidate changing under the arrow keys.
    compare_focus: usize,
    /// Last frame's vertical scroll offset for the Library grid and for Survey.
    ///
    /// ⚠⚠ **Needed *before* the scroll area is built**, which is why it is remembered here rather
    /// than read from the one being drawn: `grid_nav::follow_offset` has to know where the view
    /// currently is to decide whether the selection is inside its band, and the answer must be
    /// available when the area is constructed. Written back from each frame's `ScrollAreaOutput`.
    grid_scroll: f32,
    survey_scroll: f32,
    /// Compare's two panes, and the zoom and pan they share.
    ///
    /// ⚠ Lives on the app rather than inside the view so it survives a frame — a pane owns a
    /// decoded buffer and a render worker, and rebuilding those per frame would decode the
    /// photograph sixty times a second.
    compare_view: multi::preview_pane::PaneSet,
    /// The view as of the previous frame, so a switch can be noticed.
    ///
    /// ⚠ An edge detector, not a second source of truth: it is only ever compared against
    /// `view` and then updated. The view itself is set from several places — the segmented
    /// switch, the View menu, the G/D keys — and this catches all of them without each having
    /// to remember to clear the status line.
    last_view: AppView,
    library: Option<Library>,
    /// The server's stacks, reduced to asset→stack. Empty until fetched, and empty is "no grouping".
    ///
    /// ⚠⚠ On the app rather than on `Library`, because it is a fact about the **server** and outlives
    /// any one album: opening five albums must not mean five 6.6 MB fetches, which is the mistake the
    /// asset-list cache was built to fix.
    ///
    /// ⚠ Loaded from `immich_stack_cache` at startup and refreshed only by ⋯ → Reindex — there is no
    /// validator to do better, which that module's doc explains.
    immich_stacks: rustroom_app::immich_stack_cache::Projection,
    session: Option<Session>,
    pending: Option<PendingOpen>,
    decoder: DecodeWorker,
    /// Decodes the full-resolution buffer for the true-1:1 tier, on demand.
    full_decoder: DecodeWorker,
    /// Path and sequence number of the full-resolution decode in flight, if any.
    full_pending: Option<(PathBuf, u64)>,
    cache: BufferCache,
    /// M9.14: the preview worker — the camera's JPEG or a rendered preview, one thread.
    previews: rustroom_app::preview_loader::PreviewLoader,
    /// Preview textures on the GPU, pinned by the on-screen window exactly as thumbnails are.
    /// ⚠ Three 26-Mpx RGBA pictures fit the budget; the window never holds more than Compare's two.
    preview_textures: rustroom_app::thumb_cache::ThumbCache<(
        egui::TextureHandle,
        rustroom_app::preview_loader::PreviewSource,
    )>,
    preview_store: rustroom_app::preview_store::PreviewStore,
    /// The last want-list handed to the preview loader, so it is re-sent only when it changes.
    preview_want: Vec<rustroom_app::preview_loader::PreviewRequest>,
    /// Edited photos left without a preview, built one at a time in the background.
    leave_queue: rustroom_app::preview_build::LeaveQueue,
    /// The build in flight, if any, and whether it was asked for or came from the leave queue.
    preview_builder: Option<(
        rustroom_app::batch_export::ExportWorker,
        preview_flow::BuildKind,
    )>,
    /// "Building previews 3 of 12…" while an on-demand build runs; empty otherwise.
    preview_progress: String,
    /// Undo stacks for photos that are not open, so switching away and back does not throw
    /// one away. Unlike `cache`, which is capped at 3 because it holds decoded buffers, this
    /// is unbounded — it holds `Edits`, not pixels.
    histories: HistoryStash,
    status: String,
    show_filmstrip: bool,
    /// Tab: the panels, the rail and the filmstrip out of the way, the picture alone.
    show_panels: bool,
    /// L: 0 normal, 1 the surround dimmed, 2 the surround black — the picture alone.
    lights_out: u8,
    /// I: the capture-info card over the picture's corner.
    show_info_overlay: bool,
    /// J: clipped highlights red and clipped shadows blue, over the picture.
    show_clipping: bool,
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
    /// Paint only pixels similar to the one under the brush centre — Lightroom's Auto Mask.
    ///
    /// ⚠ A **tool** setting, not an edit: it is baked into each stroke as that stroke is painted,
    /// so changing it later affects only the next one. `brush_size`/`brush_flow`'s rule.
    brush_auto_mask: bool,
    /// How close counts, 0..=100.
    brush_tolerance: f32,
    /// The Retouch tool's live settings.
    ///
    /// ⚠ On `RustroomApp` rather than `Session` and **not persisted** — the brush's rule. The
    /// canvas size-drag writes `radius` back here, which is what makes the panel's Size slider
    /// follow the gesture.
    spot_tool: rustroom_app::spot_tool::SpotTool,
    /// The red-eye tool's settings (M9.11) — `spot_tool`'s standing. Its `active` field is the
    /// Retouch tool's eye mode: `None` places patches, `Some(kind)` places eyes.
    eye_tool: rustroom_app::eye_tool::EyeTool,
    /// Which HSL channel the Color section's selector is showing. View state, per app —
    /// like a brush size, it is a property of the tool and not of the photograph.
    hsl_channel: develop::panel::adjustments::HslChannel,
    /// Which of the four tone curves the Tone section's editor is showing. View state, per app, for
    /// the same reason as `hsl_channel`.
    ///
    /// ⚠⚠ Deliberately **not** in `Edits`: switching to the red curve must not push a history entry,
    /// and must not travel in a preset or a copy-paste. `visualize`'s rule.
    curve_channel: develop::panel::tone_curve::CurveChannel,
    /// The channel a **mask's** HSL block is showing. View state, like `hsl_channel`.
    ///
    /// ⚠ Its own field rather than sharing `hsl_channel`: switching to a mask would otherwise
    /// silently change which channel the global panel shows when you switch back.
    mask_hsl_channel: develop::panel::adjustments::HslChannel,
    /// Which colour-grading range a **mask's** wheel is showing. View state, like
    /// `mask_hsl_channel`.
    ///
    /// ⚠ Its own field rather than sharing `grade_range`: switching to a mask would otherwise
    /// silently change which wheel the global panel shows when you switch back.
    mask_grade_range: develop::panel::color_wheel::GradeRange,
    /// `None` = New mask; `Some(op)` = the next kind button refines the selected mask.
    mask_add_mode: Option<edit_model::MaskOp>,
    /// Whether the next mask-kind button creates a mask or refines the selected one.
    ///
    /// ⚠ Tool state on the app rather than on the mask, the rule the brush settings already
    /// follow: it applies to the **next** thing added, exactly as a brush size applies to the
    /// next stroke.
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
    /// The label being typed, if any — a scratch draft exactly like `rename_draft`.
    ///
    /// ⚠⚠ **Separate from `rename_draft` on purpose.** One draft serving both jobs, with its
    /// meaning depending on whether the row was a copy, is the defect underneath every problem
    /// M6.0 fixes: a label and a filename are different strings with different rules, and copy 0
    /// carrying both leaves nothing for the old inference to key on.
    label_draft: Option<(PhotoId, String)>,
    /// The snapshot row being renamed, and what has been typed into it.
    ///
    /// ⚠ A scratch draft, exactly like `rename_draft`: writing through on every keystroke would
    /// push one undo step per character and commit a half-typed name on focus loss. Cleared
    /// wherever `rename_draft` is, and for the same reason — a draft left behind belongs to a
    /// photo that is no longer open.
    snapshot_rename: Option<(u32, String)>,
    /// The Keywords row being renamed in place, and its draft (M9.8). `snapshot_rename`'s shape.
    keyword_rename: Option<(i64, String)>,
    /// Folders collapsed in the Folders tree (M9.9), for this session.
    folders_collapsed: std::collections::HashSet<std::path::PathBuf>,
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
    /// The top bar's file-size lookup, one photo at a time.
    info_sizes: rustroom_app::photo_info::SizeCache,
    /// The top bar's file-rename draft — its own, never the grid's, see `shell::centre_info`.
    info_draft: Option<(PhotoId, String)>,
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
    /// The Import dialog and any import it is running (M9.12); `None` when neither is open.
    import: Option<import_flow::ImportState>,
    /// AI masks' worker, requests and model downloads (M9.13).
    ai: ai_flow::AiState,
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
    /// The one worker for everything the UI needs from an Immich server.
    immich: rustroom_app::immich_worker::ImmichWorker,
    /// The album list once fetched. `None` = never asked, `Some(Err)` = the failure to show.
    ///
    /// ⚠ Three states, not two: "not asked yet" has to be distinguishable from "asked and got
    /// nothing", or the section would refetch on every frame.
    immich_albums: Option<Result<Vec<immich::AlbumSummary>, String>>,
    /// The month buckets, once fetched. Same three states as `immich_albums`.
    immich_buckets: Option<Result<Vec<immich::Bucket>, String>>,
    /// What the Settings dialog's Immich **Test** button last reported.
    ///
    /// ⚠⚠ Its own field rather than `self.status`: the status line sits behind the modal, so the
    /// answer to a button you just pressed was invisible — reported in use, 2026-09-23.
    ///
    /// ⚠ Cleared whenever the URL or the key is edited, or a stale "Connected" would sit under
    /// credentials it was never tested against.
    immich_test: Option<Result<String, String>>,
    /// Which album or bucket the user has just clicked, while its assets are on the way.
    ///
    /// ⚠⚠ Clicking one is a **couple of seconds** — the server returns the whole album's metadata,
    /// ~900 KB for 872 assets. Without this the rail's highlight follows `library.source`, which does
    /// not change until the reply lands, so the click looked like it had done nothing. Reported in
    /// use, 2026-09-23.
    immich_opening: Option<String>,
    /// Asset lists already fetched this session, keyed by album id or bucket identifier.
    ///
    /// ⚠ In memory only, so switching **back** to something already opened is instant while a first
    /// visit still pays the round trip. ~260 KB per 872-asset album. Persisting it across restarts is
    /// a further change with a staleness rule to decide, not a side effect of this one.
    immich_assets: std::collections::HashMap<String, Vec<immich::Asset>>,
    /// The cache file names this server has on disk — the Folders rail's count and the grid's
    /// downloaded indicator both read it.
    ///
    /// ⚠⚠ Cached because the alternative is a stat per thumbnail per frame. Refreshed at startup, when
    /// an Immich library opens, and by one insert per completed download — never per frame.
    immich_present: std::collections::HashSet<std::ffi::OsString>,
    /// The asset ids a **bulk** download is still waiting on, and how many it asked for.
    ///
    /// ⚠⚠ A set rather than a counter, so opening a single photo while a bulk download runs cannot
    /// corrupt the progress: that open's completion is simply not in this set. A `done`/`total` pair
    /// would have counted it.
    immich_download_pending: std::collections::HashSet<String>,
    /// How many the current bulk download asked for; `0` when none is running.
    immich_download_total: usize,
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
    /// The thumbnail cache's size in bytes, read **once** when the Settings dialog opens.
    ///
    /// ⚠ `None` means "not yet read". Computing it walks the cache directory, and a dialog body
    /// runs sixty times a second — the `apply_view_demosaic` rule, cited more than a dozen times
    /// in CLAUDE.md. Cleared when the dialog closes, so the next open re-reads it rather than
    /// showing a stale figure.
    thumb_cache_bytes: Option<u64>,
    /// The Immich thumbnail cache's size, computed when the Settings dialog opens.
    ///
    /// ⚠ Its own field rather than a sum: the two caches have separate budgets, so a single number
    /// could not tell you which one is full.
    immich_thumb_cache_bytes: Option<u64>,
    /// The rendered-preview cache's size, computed when the Settings dialog opens (M9.14).
    preview_cache_bytes: Option<u64>,
    /// The Settings dialog's draft while it is open — applied on Apply, dropped on Cancel.
    settings_draft: Option<modals::settings::SettingsDraft>,
    /// One chronological order over photo edits *and* preset deletions, so Ctrl+Z walks
    /// back through both. The payloads stay where they are — photo snapshots in
    /// `Session::history`, deleted presets in the trash — and this only records which of
    /// the two the next undo should pop.
    actions: ActionLog,
    /// The creative looks found on disk, and whether the one this photo names actually loaded.
    ///
    /// ⚠ Cached rather than rescanned per frame, exactly like `camera_profiles_cache` and for the
    /// same reason. Refreshed when a photo opens.
    looks: rustroom_app::looks::LooksState,
}

impl RustroomApp {
    /// ⚠ Takes the whole `CreationContext`, not just the egui one, because the window handle is
    /// needed to reach the `CAMetalLayer` — see `display_colorspace`.
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let ctx = &cc.egui_ctx;
        // ⚠ Installed here rather than lazily per frame: `init_for_nsapp` must run once, on
        // the main thread, after NSApplication exists — which it does by the time eframe
        // builds the app.
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
        let preview_store = rustroom_app::preview_store::PreviewStore::user_default(
            settings.preview_budget_mb as u64 * 1024 * 1024,
        );
        #[cfg(target_os = "macos")]
        native_menu::install(ctx, &settings.recent_folders);
        // ⚠ Before any UI runs: `add_font` rebuilds the glyph atlas, so installing later would
        // throw away the first frame's cache for nothing. Unlike `apply`, this does not depend
        // on the settings, so it does not have to wait for the load.
        theme::install_fonts(ctx);
        // ⚠ After the load, not before: the style now depends on the persisted theme and
        // accent, so applying it earlier would style the app from the defaults and then
        // silently disagree with what the user chose.
        theme::apply(ctx, &settings);
        // ⚠⚠ Before the first `ThumbLoader` exists. The loader reads the connection out of a
        // shared cell, so a server configured in a previous session has to be in that cell by the
        // time the first album could be browsed — otherwise every thumbnail would decline until
        // Settings happened to be opened and applied.
        rustroom_app::immich_source::refresh_shared(&settings);
        // ⚠⚠ The thumbnail caches are swept **here**, once, rather than in `ThumbStore::new` —
        // which `ThumbLoader::new` calls on every library open, making it ~2,500 stats per album
        // click. This is the moment a store can be over budget with nothing having been written:
        // the budget was lowered while the app was closed.
        //
        // ⚠ Off the UI thread: it walks two directories, and nothing downstream waits on it.
        {
            let local = settings.thumb_budget_mb as u64 * 1024 * 1024;
            let immich = settings.immich_thumb_budget_mb as u64 * 1024 * 1024;
            let previews = settings.preview_budget_mb as u64 * 1024 * 1024;
            std::thread::spawn(move || {
                rustroom_app::thumb_store::ThumbStore::user_default(local).sweep();
                rustroom_app::thumb_store::ThumbStore::immich_default(immich).sweep();
                rustroom_app::preview_store::PreviewStore::user_default(previews).sweep();
            });
        }
        // ⚠ Opened once, at startup. A failure is reported and then ignored — see the field.
        let catalog = match catalog::db::Catalog::open(&catalog::db::Catalog::user_default_path()) {
            Ok(c) => Some(c),
            Err(e) => {
                eprintln!("catalog unavailable: {e}");
                None
            }
        };
        let mut app = Self {
            // ⚠ From the settings, not `default()`: the format filter is persisted and this is
            // where it is read back — see `Settings::initial_filter`.
            filter: settings.initial_filter(),
            metadata: rustroom_app::metadata_scanner::MetadataScanner::new({
                let ctx = ctx.clone();
                move || ctx.request_repaint()
            }),
            camera_profiles_cache: Vec::new(),
            os_manages_colour,
            view: AppView::default(),
            library_mode: LibraryMode::default(),
            compare_candidate: None,
            compare_focus: 1,
            grid_scroll: 0.0,
            survey_scroll: 0.0,
            compare_view: Default::default(),
            last_view: AppView::default(),
            library: None,
            immich_stacks: Default::default(),
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
            previews: {
                let ctx3 = ctx.clone();
                // ⚠ The device's own limit, not a constant: an X-T4's 4416 px fits any GPU this
                // targets, but a 61 Mpx sensor's rendered preview must be shrunk to what the
                // texture upload accepts, and only the device knows that number.
                let max_side = cc
                    .wgpu_render_state
                    .as_ref()
                    .map(|s| s.device.limits().max_texture_dimension_2d as usize)
                    .unwrap_or(8192);
                rustroom_app::preview_loader::PreviewLoader::new(
                    preview_store.clone(),
                    settings.effective_display_space(os_manages_colour),
                    max_side,
                    move || ctx3.request_repaint(),
                )
            },
            preview_textures: rustroom_app::thumb_cache::ThumbCache::new(PREVIEW_TEXTURE_BUDGET),
            preview_store,
            preview_want: Vec::new(),
            leave_queue: Default::default(),
            preview_builder: None,
            preview_progress: String::new(),
            histories: HistoryStash::default(),
            status: String::new(),
            show_filmstrip: true,
            show_panels: true,
            lights_out: 0,
            show_info_overlay: false,
            show_clipping: false,
            spot_tool: Default::default(),
            eye_tool: Default::default(),
            brush_size: rustroom_app::brush_tool::DEFAULTS.size,
            brush_hardness: rustroom_app::brush_tool::DEFAULTS.hardness,
            brush_flow: rustroom_app::brush_tool::DEFAULTS.flow,
            brush_erase: false,
            brush_auto_mask: false,
            brush_tolerance: rustroom_app::brush_tool::DEFAULTS.tolerance,
            // On by default: a freshly added brush mask with no adjustment yet is otherwise
            // completely invisible, which is a poor first five minutes.
            hsl_channel: Default::default(),
            curve_channel: Default::default(),
            mask_hsl_channel: Default::default(),
            mask_grade_range: Default::default(),
            mask_add_mode: None,
            grade_range: Default::default(),
            new_collection: String::new(),
            new_smart: String::new(),
            last_period: None,
            new_keyword: String::new(),
            new_keyword_parent: None,
            rename_draft: None,
            label_draft: None,
            snapshot_rename: None,
            keyword_rename: None,
            folders_collapsed: Default::default(),
            panel_drag: None,
            cell_zoom_pending: false,
            info_sizes: Default::default(),
            info_draft: None,
            tool: Default::default(),
            crop_aspect: None,
            clipboard: None,
            paste_groups: EditGroups::default(),
            sync_groups: EditGroups::default(),
            export_draft: settings.export.clone(),
            exporter: None,
            import: None,
            ai: Default::default(),
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
            // ⚠⚠ The worker is built from the settings **before** the `settings` field moves into
            // the struct, which is why the fetcher is constructed here rather than after. A key
            // pasted later reaches it through `settings_apply`, which rebuilds the fetcher.
            immich: rustroom_app::immich_worker::ImmichWorker::new(
                rustroom_app::immich_worker::live_fetcher(rustroom_app::immich_source::connection(
                    &settings,
                )),
                {
                    let ctx = ctx.clone();
                    move || ctx.request_repaint()
                },
            ),
            immich_albums: None,
            immich_buckets: None,
            immich_test: None,
            immich_opening: None,
            immich_assets: Default::default(),
            // ⚠ Read at startup so the Folders rail's count is right before any album is opened.
            immich_present: rustroom_app::immich_source::connection(&settings)
                .map(|c| {
                    rustroom_app::immich_source::present_names(
                        &rustroom_app::immich_source::cache_root(&c.url),
                    )
                })
                .unwrap_or_default(),
            immich_download_pending: Default::default(),
            immich_download_total: 0,
            settings,
            catalog,
            modal: Modal::None,
            thumb_cache_bytes: None,
            immich_thumb_cache_bytes: None,
            preview_cache_bytes: None,
            settings_draft: None,
            actions: ActionLog::default(),
            // ⚠ Empty until a photo opens: the looks directory is scanned once per photo in
            // `session_flow`, never at startup and never per frame.
            looks: Default::default(),
        };
        // ⚠⚠ After construction rather than in the literal: it reads the settings the struct now
        // holds, and it may **request a fetch** on the worker the struct now owns. ⚠ Cheap when the
        // file exists — a 350 KB read — and when it does not, the ~2.9 s happens on the worker while
        // the grid shows both halves.
        app.load_immich_stacks();
        app
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
    /// The editing buffer, shared with the render worker.
    ///
    /// ⚠ Kept **only** so the white-balance eyedropper can read a pixel *before* any stage has
    /// touched it: sampling the rendered texture would chase its own correction, since that
    /// texture already has the white balance applied. An `Arc` clone costs nothing — the worker
    /// owns the same allocation.
    ///
    /// ⚠ It must be replaced at the full-resolution handover along with `engine`, or the
    /// eyedropper would read the half-size buffer at full-resolution coordinates.
    buffer: std::sync::Arc<core_image::ImageBuf>,
    /// The eyedropper is armed: the next click on the canvas sets the white balance.
    ///
    /// ⚠ View state on the session, never in `Edits` — arming a tool is not part of the
    /// photograph's description, so it must not reach the sidecar or travel with a paste. The
    /// rule `selected_mask` and `show_guides` already follow.
    picking_wb: bool,
    /// The defringe eyedropper is armed: the next click on the canvas sets a fringe's hue range
    /// (M9.6). View state, `picking_wb`'s rule.
    picking_fringe: bool,
    /// The illuminant the decode left this photo balanced to — the Kelvin slider's zero point.
    ///
    /// ⚠⚠ Kept on the session for the same reason `lens` is: it is a fact about the file that
    /// cannot change within one session, and **it must survive the full-resolution handover** or
    /// the white balance would silently shift the moment the upgrade lands. It also has to reach
    /// export, since a Kelvin in the sidecar means nothing without the baseline it was set
    /// against.
    wb_baseline: color::wb::Illuminant,
    /// The camera profile's `BaselineExposure` in stops, when it declares one.
    ///
    /// ⚠ Kept beside `wb_baseline` and for its reason: a fact about the file that cannot change
    /// within one session. The **panel** needs it, not only the renderer — Auto Tone has to aim
    /// the scene median at a target divided by the rendering's gain, or the two both place the
    /// level and stack.
    baseline_ev: Option<f32>,
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
    /// Which **component** of the selected mask the shape controls and the canvas handles act on.
    /// `0` is the mask's base shape; `j > 0` is the `j-1`th refinement.
    ///
    /// ⚠ **View state, never `Edits`** — `selected_mask`'s rule, for its reason.
    ///
    /// ⚠ Read through [`Session::component_index`], never directly: that clamps against the
    /// selected mask's real component count, so a site that forgets to reset this degrades to
    /// "the base is selected" rather than leaving the panel's sliders on one shape and the
    /// canvas's handles on another.
    selected_component: usize,
    /// Which retouch spot is selected, if any.
    ///
    /// ⚠ **View state, never `Edits`** — `selected_mask`'s rule, for its reason: a selection is
    /// not part of the photograph's description, so it must not reach the sidecar or travel with
    /// a paste. `Session` is per-photo, so this resets on a switch for free.
    selected_spot: Option<usize>,
    /// The mask handle this drag is moving, if the drag started on one.
    ///
    /// ⚠ Load-bearing for panning: a drag that did *not* start on a handle must still pan the
    /// image, so this records whether the mask claimed the gesture.
    active_mask_handle: Option<mask_tool::Handle>,
    /// A spot the *view* leaves out while it is being adjusted.
    ///
    /// ⚠⚠ **Why: while you drag a patch, its old effect is still on screen, hiding the very
    /// blemish you are trying to line up with.** Reported as "it shows the previous retouching
    /// there, I think it should just disappear, so I can once again better align the positions".
    /// Suppressing it reveals the untouched pixels underneath for the duration of the gesture.
    ///
    /// ⚠ **View state, never `Edits`** — the same standing as `selected_spot`, and applied in
    /// `view_edits`, which is where `preview_edits` already strips the crop in geometry mode. The
    /// sidecar, an export and the CLI never see it.
    ///
    /// ⚠ Two renders per gesture, not one per frame: one when the suppression begins and one when
    /// it ends. `SpotRemovalStage` is in the prefix, so each costs a whole-buffer noise-reduction
    /// and dehaze pass — which is exactly why the per-frame version was reported as laggy.
    suppressed_spot: Option<usize>,
    /// Per mask, whether its overlay is tinted on the canvas — the row's eye. View state, never
    /// part of `Edits`; `sync_mask_shown` keeps it the masks' length, and a mask that arrives by
    /// undo, paste or a snapshot starts with its overlay off.
    ///
    /// ⚠ Since 2026-09-20 the eye is the overlay **only**; switching a mask off is
    /// `MaskedAdjustment::disabled` in the edits — its own toggle beside the eye — because the
    /// user wants an off mask out of the export too.
    mask_shown: Vec<bool>,
    /// Per patch, whether its outline is drawn on the canvas — the old "Show patches", per
    /// patch (2026-09-20). View state; `sync_spot_flags` keeps it the patches' length, and a
    /// patch that arrives by any route starts shown, as outlines always did.
    spot_shown: Vec<bool>,
    /// Per patch, whether it is held out of the view so that what it covers shows — the
    /// per-patch twin of a mask's eye. View state; `view_edits` removes held patches together
    /// with the drag-suppressed one.
    spot_held: Vec<bool>,
    /// Set for exactly one frame after a suppression change, so the canvas lets the deliberately
    /// softer render stand instead of sharpening it away on the same frame.
    ///
    /// ⚠⚠ **A baton passed across one frame, not a second owner of a fact.** `develop::body` runs
    /// the panels before the canvas, so without it the canvas sees `max_dim != s.render_rung` in
    /// the very frame the drop was made and requests full quality immediately — the soft render
    /// is superseded before it can be shown, and the toggle takes the full 95-300 ms after all.
    /// That is not the spot-removal shape: nothing else decides this, and it is consumed by the
    /// one reader.
    soft_next: bool,
    /// The last **whole-image** texture, kept as a backdrop under a region render, with the
    /// `fit_size` it was rendered at.
    ///
    /// ⚠⚠ **This is what stops the edges of a pan being BLACK.** A region render paints only
    /// `img_rect`; everything outside it was simply not painted, so revealing new area while
    /// panning showed black rectangles that snapped to image when the next region landed.
    /// Reported as making panning at 1:1 "very stuttery" and "very unpleasant".
    ///
    /// ⚠ The alternative — render the whole image at the native rung — was **measured and
    /// rejected**: 26 Mpx against a region's 0.5, about 300 ms per render, and paid on every
    /// slider move as well as every pan. This costs one texture and one `paint_at`, and no
    /// rendering at all.
    ///
    /// ⚠ Keyed on the `fit_size` it was rendered at, and dropped when that no longer matches.
    /// A crop or an orientation change moves the frame, and a backdrop from before it would be
    /// painted at the wrong scale — visibly, since it is drawn across the whole frame rect.
    backdrop: Option<(egui::TextureHandle, (f32, f32))>,
    /// What the retouch gesture is doing, across frames.
    ///
    /// ⚠⚠ **On the session, not a local in `canvas::show`** — `active_mask_handle`'s home, for a
    /// reason found the hard way: `show` runs fresh every frame, so a local resets between the
    /// press and the release and the release then sees no gesture at all. The drag placed nothing
    /// and the lifecycle looked perfect from every angle until it was instrumented.
    spot_drag: crate::develop::canvas::spot_overlay::SpotDrag,
    /// Which eye is selected (M9.11). View state, never `Edits` — `selected_spot`'s rule.
    /// ⚠ Selecting an eye clears `selected_spot` and vice versa, so the panel's sliders always
    /// edit exactly one thing.
    selected_eye: Option<usize>,
    /// An eye the view leaves out while it is moved or resized — `suppressed_spot`'s twin, so the
    /// red pupil underneath is visible to aim at. Applied in `view_edits`.
    suppressed_eye: Option<usize>,
    /// The eye gesture across frames — on the session for `spot_drag`'s reason.
    eye_drag: crate::develop::canvas::eye_overlay::EyeDrag,
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
    /// The full post-geometry frame's size in **rendered** pixels — what a region was cut from,
    /// or the frame's own dimensions when it arrived whole.
    ///
    /// ⚠ [`Self::rendered_long`] is derived from this at the single write site rather than being
    /// read from the frame separately: two spellings of one fact that are computed independently
    /// is the shape `render_max_dim` and `fit_image_size` were each fixed for.
    rendered_full: (usize, usize),
    rendered_long: f32,
    /// The editing buffer's long edge — the top rung of the render ladder.
    buffer_long: usize,
    /// The long edge of a **full-resolution** decode of this file, in pixels.
    ///
    /// ⚠⚠ Derived from the buffer and its `source_scale` **once**, so it is the same number
    /// before and after the full-resolution handover. `buffer_long` is not usable for this: a RAW
    /// opens at half size, so a size shown in pixels against it would silently double a few
    /// seconds after the photo appeared.
    full_long: usize,
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
    /// The canvas's rendering intent, copied from `Settings` when the session opens — held here
    /// for `display_space`'s reason. `apply_canvas_intent` writes it on the open session.
    canvas_intent: color::RenderingIntent,
    /// The resampling kernel, copied from `Settings` when the session opens.
    ///
    /// ⚠ Held here for the same reason `display_space` is: `Session::request` is the one funnel
    /// every render goes through and it cannot reach `RustroomApp`. `apply_resampler` writes it
    /// on the open session so a change takes effect without reopening the photo.
    resampler: pipeline::Resampler,
    /// Run the **global** HSL stage on the GPU, copied from `Settings` when the session opens.
    ///
    /// ⚠ Held here for `resampler`'s reason directly above: `Session::request` is the one funnel
    /// every render goes through and it cannot reach `RustroomApp`. `apply_gpu_tail` writes it on
    /// the open session so a change takes effect without reopening the photo.
    gpu_tail: bool,
    /// The Visualize Spots threshold, or `None` when the view is off.
    ///
    /// ⚠ On `Session`, not `RustroomApp`, for the reason `display_space` gives: `Session::request`
    /// is the one funnel every render goes through and has no access to the app, so the render
    /// reads it from here. A second copy on the app would be two facts that can disagree — the
    /// shape `render_max_dim` and `geometry_mode` were each fixed for. The cost is that it resets
    /// on a photo switch, like `selected_spot`; `select_tool` also clears it, since a canvas
    /// stuck in the grey edge view after switching to Edit reads as a broken render.
    visualize: Option<f32>,
    /// The "Show range" checkbox in the Color Grading block.
    ///
    /// ⚠ View state on `Session`, beside `visualize` and for its reason: `Session::request` is
    /// the one funnel every render goes through and cannot reach the app. Resets per photo and
    /// clears on a tool switch. ⚠ Never `Edits` — it must not reach the sidecar or a paste.
    show_grade_range: bool,
    /// What the request carries: the checkbox AND a non-Global selection, or `None`.
    ///
    /// ⚠ Two fields, one derivation site. The value depends on the checkbox and on
    /// `app.grade_range`, which lives on the app and which `request` cannot see; the panel
    /// derives this with a change guard in the one place both are in scope. Under Global it is
    /// `None` while the box stays ticked — one `Option` would untick the box the moment Global
    /// was selected.
    grade_view: Option<rustroom_app::grade_view::GradeRangeView>,
    /// The view's plane as a texture, set where the frame lands and dropped by the same
    /// assignment when a frame arrives without one.
    grade_view_texture: Option<egui::TextureHandle>,
    /// The straighten-line gesture is armed: the next drag under `Tool::Crop` draws a line
    /// instead of moving the crop rectangle.
    ///
    /// ⚠ View state on the session, never in `Edits` — the rule `picking_wb` states: "arming a
    /// tool is not part of the photograph's description, so it must not reach the sidecar or
    /// travel with a paste." `show_guides` and `selected_mask` follow it too. `Session` is
    /// per-photo, so this resets on a switch for free.
    straighten_armed: bool,
    /// The line being drawn, in **raw**-frame normalised coordinates, while the drag is live.
    ///
    /// ⚠⚠ **On `Session`, never a local in `canvas::show`.** M5.3b records the exact failure: the
    /// spot drag state "was a **local** in `canvas::show` at first — reset every frame, so the
    /// release saw no gesture", and "one `eprintln` found it after the lifecycle had read
    /// perfectly three times." It is `Session.spot_drag` now, and this follows it.
    straighten_drag: Option<([f32; 2], [f32; 2])>,
    /// The last straighten line, on the **raw** frame, drawn mapped through the correction so it
    /// reads level — the feedback that says the gesture worked.
    ///
    /// ⚠ View state, never `Edits`, for `straighten_armed`'s reason. Cleared when the mode is
    /// re-armed, the tool changes, or the photo changes (the last is free — `Session` is
    /// per-photo).
    straighten_line: Option<([f32; 2], [f32; 2])>,
    /// The rendered frame's sRGB bytes, kept for the colour readout, with the dimensions they
    /// describe.
    ///
    /// ⚠⚠ **Not the texture.** `texture` is an `egui::TextureHandle`, i.e. GPU-side, and cannot
    /// be sampled CPU-side — which is exactly why `guide_overlay::draw_loupe` magnifies it by
    /// *drawing* it with UVs. These bytes are `pack`'s `srgb8`, the same ones `hist` is bucketed
    /// from, and deliberately **not** `RenderedFrame.rgba`, which is in the display's primaries
    /// and would give a readout that changes with the monitor.
    ///
    /// ⚠ The dimensions travel **with** the bytes rather than being re-derived from
    /// `rendered_region` or the texture. Two facts that can disagree is the shape
    /// `render_max_dim`, `fit_image_size` and `geometry_mode` were each fixed for.
    ///
    /// ⚠ An `Arc`, so retaining it costs nothing beyond the memory the frame already held.
    readout_bytes: Option<(std::sync::Arc<Vec<u8>>, usize, usize)>,
    /// The clipping overlay for the frame `readout_bytes` describes, when `J` is on and
    /// something clips; painted at the preview's own rectangle.
    clip_texture: Option<egui::TextureHandle>,
    /// Where the whole photo sat on screen last frame — what lights-out cuts its hole around
    /// and the info card anchors to.
    canvas_frame_rect: Option<egui::Rect>,
    /// The sampled value under the cursor, or `None`.
    ///
    /// ⚠ The **value**, not a formatted string: the swatch and the numbers are two renderings of
    /// one fact, and storing the string would leave the swatch deriving a colour from text. One
    /// fact, formatted at the edge that draws it.
    ///
    /// ⚠ Cleared by `canvas::show` on every frame and set again only by an actual hover over the
    /// image. A stale number left on screen after the pointer leaves is worse than none, because
    /// it gets trusted.
    readout: Option<[u8; 3]>,
    /// The output space the canvas is being proofed into, or `None`.
    ///
    /// ⚠ On `Session`, not `RustroomApp`: `Session::request` is the one funnel every render goes
    /// through and it has no app access. That is why `display_space` and `visualize` live here —
    /// and, as there, the cost is that it resets on a photo switch.
    ///
    /// ⚠⚠ **View state, never `Edits`.** A proof is a property of the *view*, exactly like
    /// `display_space`: it must not reach the sidecar, must not travel with a paste, a preset or
    /// a sync, and must not appear in any golden.
    soft_proof: Option<color::OutputSpace>,
    /// Whether out-of-gamut pixels are marked while proofing.
    ///
    /// ⚠ A separate toggle from the proof itself: proofing without the warning is the common case
    /// — you want to see the picture — and a warning you cannot switch off is chrome. It has no
    /// effect at all while `soft_proof` is `None`, which is why `Session::request` folds the two
    /// into one `Proof` rather than sending them separately.
    gamut_warning: bool,
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
