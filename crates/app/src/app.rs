//! The application's state: the one `RustroomApp`, and the view and Library mode it is in.

use crate::{
    ai_flow, develop, import_flow, modals, multi, preview_flow, Library, Modal, PendingOpen,
    Session,
};
use edit_model::{EditGroups, Edits};
use eframe::egui;
use rustroom_app::action_log::ActionLog;
use rustroom_app::batch_export::{ExportOptions, ExportWorker};
use rustroom_app::buffer_cache::BufferCache;
use rustroom_app::decode_worker::DecodeWorker;
use rustroom_app::edit_history::HistoryStash;
use rustroom_app::export_presets::{ExportPreset, ExportPresetStore};
use rustroom_app::photo_id::PhotoId;
use rustroom_app::presets::{Preset, PresetStore};
use rustroom_app::settings::{Settings, SettingsStore};
use rustroom_app::tool::Tool;
use std::path::PathBuf;

// ⚠ `Copy` so the frame loop can snapshot the previous view for edge detection.
#[derive(Default, PartialEq, Eq, Clone, Copy)]
pub(crate) enum AppView {
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
pub(crate) enum LibraryMode {
    #[default]
    Grid,
    /// Two photos side by side: a reference and a candidate being judged against it.
    Compare,
    /// The whole selection at once, tiled to fill the view.
    Survey,
}

pub(crate) struct RustroomApp {
    /// The camera profiles found on disk, read when a photo opens.
    ///
    /// ⚠ Cached rather than rescanned per frame: the panel body runs sixty times a second, and
    /// a directory scan there is the `apply_view_demosaic` mistake in a different costume.
    pub(crate) camera_profiles_cache: Vec<camera_profile::store::ProfileEntry>,
    /// Whether the OS is colour-matching the window for us, so we must not also convert.
    ///
    /// ⚠ Runtime, not `#[cfg]`: a macOS build where tagging failed has to take the manual path,
    /// and keeping it a value means both branches compile on every platform.
    pub(crate) os_manages_colour: bool,
    pub(crate) view: AppView,
    /// Which Library layout is on screen — the grid, Compare or Survey.
    pub(crate) library_mode: LibraryMode,
    /// The photo Compare shows beside the reference.
    ///
    /// ⚠⚠ A `PhotoId` rather than an index, and **resolved every frame** through
    /// `compare_pair::candidate` rather than trusted. Indices shift under a sort, a filter, a
    /// rebuild and a copy being created; an id that no longer names a visible row simply falls
    /// back to the next candidate. That is why no rebuild path has to remember to clear this —
    /// the alternative is an invalidation hook at every one of them, and the one that forgets
    /// shows the wrong photograph.
    pub(crate) compare_candidate: Option<PhotoId>,
    /// Which of Compare's two panes carries the border, and therefore what its keys act on.
    ///
    /// ⚠ 0 is the reference, 1 the candidate. A plain index rather than a `PhotoId`, because it
    /// is a fact about the *layout* — which side you are looking at — and must survive the
    /// candidate changing under the arrow keys.
    pub(crate) compare_focus: usize,
    /// Last frame's vertical scroll offset for the Library grid and for Survey.
    ///
    /// ⚠⚠ **Needed *before* the scroll area is built**, which is why it is remembered here rather
    /// than read from the one being drawn: `grid_nav::follow_offset` has to know where the view
    /// currently is to decide whether the selection is inside its band, and the answer must be
    /// available when the area is constructed. Written back from each frame's `ScrollAreaOutput`.
    pub(crate) grid_scroll: f32,
    pub(crate) survey_scroll: f32,
    /// Compare's two panes, and the zoom and pan they share.
    ///
    /// ⚠ Lives on the app rather than inside the view so it survives a frame — a pane owns a
    /// decoded buffer and a render worker, and rebuilding those per frame would decode the
    /// photograph sixty times a second.
    pub(crate) compare_view: multi::preview_pane::PaneSet,
    /// The view as of the previous frame, so a switch can be noticed.
    ///
    /// ⚠ An edge detector, not a second source of truth: it is only ever compared against
    /// `view` and then updated. The view itself is set from several places — the segmented
    /// switch, the View menu, the G/D keys — and this catches all of them without each having
    /// to remember to clear the status line.
    pub(crate) last_view: AppView,
    pub(crate) library: Option<Library>,
    /// The server's stacks, reduced to asset→stack. Empty until fetched, and empty is "no grouping".
    ///
    /// ⚠⚠ On the app rather than on `Library`, because it is a fact about the **server** and outlives
    /// any one album: opening five albums must not mean five 6.6 MB fetches, which is the mistake the
    /// asset-list cache was built to fix.
    ///
    /// ⚠ Loaded from `immich_stack_cache` at startup and refreshed only by ⋯ → Reindex — there is no
    /// validator to do better, which that module's doc explains.
    pub(crate) immich_stacks: rustroom_app::immich_stack_cache::Projection,
    pub(crate) session: Option<Session>,
    pub(crate) pending: Option<PendingOpen>,
    pub(crate) decoder: DecodeWorker,
    /// Decodes the full-resolution buffer for the true-1:1 tier, on demand.
    pub(crate) full_decoder: DecodeWorker,
    /// Path and sequence number of the full-resolution decode in flight, if any.
    pub(crate) full_pending: Option<(PathBuf, u64)>,
    pub(crate) cache: BufferCache,
    /// The on-disk thumbnail cache, read at the click for the ghost session's buffer.
    pub(crate) ghost_store: rustroom_app::thumb_store::ThumbStore,
    /// M9.14: the preview worker — the camera's JPEG or a rendered preview, one thread.
    pub(crate) previews: rustroom_app::preview_loader::PreviewLoader,
    /// Preview textures on the GPU, pinned by the on-screen window exactly as thumbnails are.
    /// ⚠ Three 26-Mpx RGBA pictures fit the budget; the window never holds more than Compare's two.
    pub(crate) preview_textures: rustroom_app::thumb_cache::ThumbCache<(
        egui::TextureHandle,
        rustroom_app::preview_loader::PreviewSource,
    )>,
    pub(crate) preview_store: rustroom_app::preview_store::PreviewStore,
    /// The last want-list handed to the preview loader, so it is re-sent only when it changes.
    pub(crate) preview_want: Vec<rustroom_app::preview_loader::PreviewRequest>,
    /// Keys the loader answered with **nothing** — an edited photo with no rendered preview yet.
    /// Only then does a view fall back to the 512-px thumbnail; while an answer is pending it
    /// shows nothing low-resolution at all (the author's call, 2026-09-29).
    pub(crate) preview_misses: std::collections::HashSet<rustroom_app::thumb_key::ThumbKey>,
    /// Edited photos left without a preview, built one at a time in the background.
    pub(crate) leave_queue: rustroom_app::preview_build::LeaveQueue,
    /// The build in flight, if any, and whether it was asked for or came from the leave queue.
    pub(crate) preview_builder: Option<(
        rustroom_app::batch_export::ExportWorker,
        preview_flow::BuildKind,
    )>,
    /// "Building previews 3 of 12…" while an on-demand build runs; empty otherwise.
    pub(crate) preview_progress: String,
    /// Undo stacks for photos that are not open, so switching away and back does not throw
    /// one away. Unlike `cache`, which is capped at 3 because it holds decoded buffers, this
    /// is unbounded — it holds `Edits`, not pixels.
    pub(crate) histories: HistoryStash,
    pub(crate) status: String,
    pub(crate) show_filmstrip: bool,
    /// Tab: the panels, the rail and the filmstrip out of the way, the picture alone.
    pub(crate) show_panels: bool,
    /// L: 0 normal, 1 the surround dimmed, 2 the surround black — the picture alone.
    pub(crate) lights_out: u8,
    /// I: the capture-info card over the picture's corner.
    pub(crate) show_info_overlay: bool,
    /// J: clipped highlights red and clipped shadows blue, over the picture.
    pub(crate) show_clipping: bool,
    /// Brush radius in **screen points**.
    ///
    /// ⚠ Not persisted, and not in `Settings`. A screen-point size is view-dependent, so
    /// restoring last session's value at a different window size or zoom restores a *different*
    /// brush. The other three could be persisted, but splitting one tool's settings across two
    /// lifetimes is worse than one honest rule.
    pub(crate) brush_size: f32,
    /// 0 = maximally soft, 100 = a hard edge. ⚠ Edge shape, **not** strength — see `brush_flow`.
    pub(crate) brush_hardness: f32,
    /// How much coverage one pass lays down, 0..=100 — the **strength** control.
    ///
    /// ⚠ Separate from hardness because they are genuinely different things, and conflating them
    /// is what made the first version's softest brush still paint at full strength.
    pub(crate) brush_flow: f32,
    /// Whether the next stroke subtracts.
    pub(crate) brush_erase: bool,
    /// Paint only pixels similar to the one under the brush centre — Lightroom's Auto Mask.
    ///
    /// ⚠ A **tool** setting, not an edit: it is baked into each stroke as that stroke is painted,
    /// so changing it later affects only the next one. `brush_size`/`brush_flow`'s rule.
    pub(crate) brush_auto_mask: bool,
    /// How close counts, 0..=100.
    pub(crate) brush_tolerance: f32,
    /// The Retouch tool's live settings.
    ///
    /// ⚠ On `RustroomApp` rather than `Session` and **not persisted** — the brush's rule. The
    /// canvas size-drag writes `radius` back here, which is what makes the panel's Size slider
    /// follow the gesture.
    pub(crate) spot_tool: rustroom_app::spot_tool::SpotTool,
    /// The red-eye tool's settings (M9.11) — `spot_tool`'s standing. Its `active` field is the
    /// Retouch tool's eye mode: `None` places patches, `Some(kind)` places eyes.
    pub(crate) eye_tool: rustroom_app::eye_tool::EyeTool,
    /// Which HSL channel the Color section's selector is showing. View state, per app —
    /// like a brush size, it is a property of the tool and not of the photograph.
    pub(crate) hsl_channel: develop::panel::adjustments::HslChannel,
    /// Point curve or Parametric — view state beside `curve_channel`, never an edit.
    pub(crate) curve_mode: develop::panel::tone_curve::CurveMode,
    /// Which of the four tone curves the Tone section's editor is showing. View state, per app, for
    /// the same reason as `hsl_channel`.
    ///
    /// ⚠⚠ Deliberately **not** in `Edits`: switching to the red curve must not push a history entry,
    /// and must not travel in a preset or a copy-paste. `visualize`'s rule.
    pub(crate) curve_channel: develop::panel::tone_curve::CurveChannel,
    /// The channel a **mask's** HSL block is showing. View state, like `hsl_channel`.
    ///
    /// ⚠ Its own field rather than sharing `hsl_channel`: switching to a mask would otherwise
    /// silently change which channel the global panel shows when you switch back.
    pub(crate) mask_hsl_channel: develop::panel::adjustments::HslChannel,
    /// Which colour-grading range a **mask's** wheel is showing. View state, like
    /// `mask_hsl_channel`.
    ///
    /// ⚠ Its own field rather than sharing `grade_range`: switching to a mask would otherwise
    /// silently change which wheel the global panel shows when you switch back.
    pub(crate) mask_grade_range: develop::panel::color_wheel::GradeRange,
    /// Whether the next mask-kind button creates a mask or refines the selected one.
    ///
    /// ⚠ Tool state on the app rather than on the mask, the rule the brush settings already
    /// follow: it applies to the **next** thing added, exactly as a brush size applies to the
    /// next stroke.
    ///
    /// `None` = New mask; `Some(op)` = the next kind button refines the selected mask.
    pub(crate) mask_add_mode: Option<edit_model::MaskOp>,
    /// Which colour-grading range the wheel is showing. View state, like `hsl_channel`.
    pub(crate) grade_range: develop::panel::color_wheel::GradeRange,
    /// Scratch text for the new-collection field.
    pub(crate) new_collection: String,
    /// Scratch name for the smart collection about to be saved.
    pub(crate) new_smart: String,
    /// The date row a Shift-range extends from.
    pub(crate) last_period: Option<rustroom_app::date_tree::Period>,
    /// Scratch text for the new-keyword field.
    pub(crate) new_keyword: String,
    /// The keyword a new one will be created under. `None` makes a root.
    pub(crate) new_keyword_parent: Option<i64>,
    /// The Keywords field's highlighted row (M10.8a).
    pub(crate) keyword_field: crate::keyword_field::FieldState,
    /// The selection's keyword suggestions, remembered until the selection or the catalog changes.
    pub(crate) keyword_suggest: rustroom_app::keyword_suggest::Cache,
    /// Scratch text for the filename field, with **the photo it belongs to**.
    ///
    /// ⚠ A draft rather than writing through, so a half-typed name is never committed and a
    /// rename is one undo step rather than one per keystroke.
    ///
    /// ⚠⚠ The `PhotoId` is not decoration. As a bare `String` this draft outlived the photo it
    /// was typed for, so **every** photo switched to afterwards showed the last name typed —
    /// reported as "now every photo shows the name I changed". Pairing the text with its photo
    /// makes that unrepresentable rather than something to remember to clear.
    pub(crate) rename_draft: Option<(PhotoId, String)>,
    /// The label being typed, if any — a scratch draft exactly like `rename_draft`.
    ///
    /// ⚠⚠ **Separate from `rename_draft` on purpose.** One draft serving both jobs, with its
    /// meaning depending on whether the row was a copy, is the defect underneath every problem
    /// M6.0 fixes: a label and a filename are different strings with different rules, and copy 0
    /// carrying both leaves nothing for the old inference to key on.
    pub(crate) label_draft: Option<(PhotoId, String)>,
    /// The snapshot row being renamed, and what has been typed into it.
    ///
    /// ⚠ A scratch draft, exactly like `rename_draft`: writing through on every keystroke would
    /// push one undo step per character and commit a half-typed name on focus loss. Cleared
    /// wherever `rename_draft` is, and for the same reason — a draft left behind belongs to a
    /// photo that is no longer open.
    pub(crate) snapshot_rename: Option<(u32, String)>,
    /// The Keywords row being renamed in place, and its draft (M9.8). `snapshot_rename`'s shape.
    pub(crate) keyword_rename: Option<(i64, String)>,
    /// Folders collapsed in the Folders tree (M9.9), for this session.
    pub(crate) folders_collapsed: std::collections::HashSet<std::path::PathBuf>,
    /// The Folders rail's marked folders — view state like `folders_collapsed` (M10.19c).
    pub(crate) folder_marks: rustroom_app::folder_marks::FolderMarks,
    /// The Library panel section being dragged, if one is.
    ///
    /// ⚠ On the app rather than a local, because a drag spans frames and a `Response` only knows
    /// about the frame it was built in. Cleared on release, whatever the outcome.
    pub(crate) panel_drag: Option<rustroom_app::settings::PanelSection>,
    /// A grid-size pinch is under way and its result has not been written to disk yet.
    ///
    /// ⚠ Exists so the settings file is written once when the gesture settles rather than sixty
    /// times a second while it runs.
    pub(crate) cell_zoom_pending: bool,
    /// The top bar's file-size lookup, one photo at a time.
    pub(crate) info_sizes: rustroom_app::photo_info::SizeCache,
    /// The top bar's file-rename draft — its own, never the grid's, see `shell::centre_info`.
    pub(crate) info_draft: Option<(PhotoId, String)>,
    /// Which Develop tool the rail has selected.
    ///
    /// ⚠ On the app, not on `Session`, so it **survives a photo switch** — see `tool::Tool`.
    pub(crate) tool: Tool,
    /// The pixel aspect ratio the crop is locked to, or `None` for a free crop.
    ///
    /// ⚠ Stored as the **pixel** ratio (1.0, 1.5, …), not the normalized one. The normalized
    /// aspect is `pixel / image_aspect` and so differs per photo; storing that would silently
    /// change shape the moment you switched to a photo with different dimensions.
    ///
    /// ⚠ Tool state, not an edit: it must never reach the sidecar, a paste or a preset, for
    /// the same reason `Session::selected_mask` does not.
    pub(crate) crop_aspect: Option<f32>,
    /// This session's copied edits. A copy *also* writes a tagged payload to the system
    /// clipboard (see [`edit_clipboard`]), so pasting works between two running instances;
    /// this field is what the "Paste…" button uses, because egui can only hand us the
    /// system clipboard's contents in response to a paste shortcut, never on demand.
    pub(crate) clipboard: Option<Edits>,
    /// Group selection last used for a paste, so Ctrl+V can skip the dialog.
    pub(crate) paste_groups: EditGroups,
    /// Group selection last used for a sync, kept separate from `paste_groups` so the two
    /// dialogs do not silently reconfigure each other.
    pub(crate) sync_groups: EditGroups,
    /// The export dialog's **working copy**, reset from `settings.export` each time the
    /// dialog opens.
    ///
    /// ⚠ Scratch state, exactly like `ConfirmDeletePreset`'s `dont_ask_again`. The committed
    /// value lives in `settings.export` and is written only by `start_export`, so cancelling
    /// the dialog discards whatever was typed rather than half-applying it. An earlier
    /// version edited the committed value directly and Cancel left the changes behind.
    pub(crate) export_draft: ExportOptions,
    /// The running batch export, if any.
    pub(crate) exporter: Option<ExportWorker>,
    /// Progress line for the running export.
    pub(crate) export_progress: String,
    /// The Import dialog and any import it is running (M9.12); `None` when neither is open.
    pub(crate) import: Option<import_flow::ImportState>,
    /// AI masks' worker, requests and model downloads (M9.13).
    pub(crate) ai: ai_flow::AiState,
    /// The destination awaiting a collision answer; `Some` means the worker is blocked.
    pub(crate) export_collision: Option<PathBuf>,
    /// Scratch state for the collision dialog's "apply to all" tick.
    pub(crate) export_apply_all: bool,
    /// The photo the filmstrip has already scrolled to.
    ///
    /// ⚠ Scrolling only when this *changes* is the whole point: calling `scroll_to_me` every
    /// frame would fight the user's own scrolling and pin the strip in place.
    pub(crate) filmstrip_scrolled_to: Option<usize>,
    pub(crate) export_preset_store: ExportPresetStore,
    /// Cached `export_preset_store.list()`, refreshed on save and delete rather than every
    /// frame — the same reason `preset_list` is cached.
    pub(crate) export_preset_list: Vec<ExportPreset>,
    /// Which preset the dialog currently shows, so Delete knows what to delete.
    ///
    /// ⚠ Tracked explicitly rather than inferred by matching the dialog's options against
    /// the list: two presets can hold identical options, and a Delete that guesses would
    /// remove the wrong one.
    pub(crate) selected_export_preset: Option<String>,
    pub(crate) preset_store: PresetStore,
    /// Cached `preset_store.list()`, refreshed on save and delete rather than every frame.
    pub(crate) preset_list: Vec<Preset>,
    /// Cached `preset_store.folders()`, refreshed with `preset_list` (M10.9a).
    pub(crate) preset_folders: Vec<String>,
    /// The preset or folder being renamed in place, with what has been typed (M10.9a).
    pub(crate) preset_rename: Option<crate::preset_ops::PresetRename>,
    pub(crate) settings_store: SettingsStore,
    pub(crate) settings: Settings,
    /// The one worker for everything the UI needs from an Immich server.
    pub(crate) immich: rustroom_app::immich_worker::ImmichWorker,
    /// The album list once fetched. `None` = never asked, `Some(Err)` = the failure to show.
    ///
    /// ⚠ Three states, not two: "not asked yet" has to be distinguishable from "asked and got
    /// nothing", or the section would refetch on every frame.
    pub(crate) immich_albums: Option<Result<Vec<immich::AlbumSummary>, String>>,
    /// The month buckets, once fetched. Same three states as `immich_albums`.
    pub(crate) immich_buckets: Option<Result<Vec<immich::Bucket>, String>>,
    /// What the Settings dialog's Immich **Test** button last reported.
    ///
    /// ⚠⚠ Its own field rather than `self.status`: the status line sits behind the modal, so the
    /// answer to a button you just pressed was invisible — reported in use, 2026-09-23.
    ///
    /// ⚠ Cleared whenever the URL or the key is edited, or a stale "Connected" would sit under
    /// credentials it was never tested against.
    pub(crate) immich_test: Option<Result<String, String>>,
    /// Which album or bucket the user has just clicked, while its assets are on the way.
    ///
    /// ⚠⚠ Clicking one is a **couple of seconds** — the server returns the whole album's metadata,
    /// ~900 KB for 872 assets. Without this the rail's highlight follows `library.source`, which does
    /// not change until the reply lands, so the click looked like it had done nothing. Reported in
    /// use, 2026-09-23.
    pub(crate) immich_opening: Option<String>,
    /// Asset lists already fetched this session, keyed by album id or bucket identifier.
    ///
    /// ⚠ In memory only, so switching **back** to something already opened is instant while a first
    /// visit still pays the round trip. ~260 KB per 872-asset album. Persisting it across restarts is
    /// a further change with a staleness rule to decide, not a side effect of this one.
    pub(crate) immich_assets: std::collections::HashMap<String, Vec<immich::Asset>>,
    /// The cache file names this server has on disk — the Folders rail's count and the grid's
    /// downloaded indicator both read it.
    ///
    /// ⚠⚠ Cached because the alternative is a stat per thumbnail per frame. Refreshed at startup, when
    /// an Immich library opens, and by one insert per completed download — never per frame.
    pub(crate) immich_present: std::collections::HashSet<std::ffi::OsString>,
    /// Each album's cache file names, derived once per list — what an album row's download count
    /// looks up in `immich_present` on every frame (`immich_downloaded_of`, 2026-10-12).
    ///
    /// ⚠ A `RefCell` because the rail draws from `&RustroomApp`. Only names are kept: what is
    /// downloaded is read live, so no writer of `immich_present` has to invalidate anything.
    pub(crate) immich_album_names: std::cell::RefCell<rustroom_app::immich_album_names::AlbumNames>,
    /// The asset ids a **bulk** download is still waiting on, and how many it asked for.
    ///
    /// ⚠⚠ A set rather than a counter, so opening a single photo while a bulk download runs cannot
    /// corrupt the progress: that open's completion is simply not in this set. A `done`/`total` pair
    /// would have counted it.
    pub(crate) immich_download_pending: std::collections::HashSet<String>,
    /// How many the current bulk download asked for; `0` when none is running.
    pub(crate) immich_download_total: usize,
    /// The one catalog. `None` when it could not be opened.
    ///
    /// ⚠⚠ **Invisible by design.** Nothing in the UI mentions it; it exists so that collections
    /// — and later ratings and search — have somewhere to live. If you can tell it shipped,
    /// something is wrong.
    ///
    /// ⚠ `Option` rather than a hard failure: a catalog that will not open must not stop you
    /// editing photographs, because edits live in the sidecars and do not need it.
    pub(crate) catalog: Option<catalog::db::Catalog>,
    /// Reads capture metadata into the catalog in the background.
    ///
    /// ⚠ Its own pass rather than part of the thumbnail loader, which is **windowed** — you cannot
    /// filter by a date that was never read. Built once and kept for the life of the app, so a
    /// source switch never abandons a half-scanned folder.
    pub(crate) metadata: rustroom_app::metadata_scanner::MetadataScanner,
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
    pub(crate) filter: rustroom_app::library_filter::LibraryFilter,
    pub(crate) modal: Modal,
    /// The thumbnail cache's size in bytes, read **once** when the Settings dialog opens.
    ///
    /// ⚠ `None` means "not yet read". Computing it walks the cache directory, and a dialog body
    /// runs sixty times a second — the `apply_view_demosaic` rule, cited more than a dozen times
    /// in CLAUDE.md. Cleared when the dialog closes, so the next open re-reads it rather than
    /// showing a stale figure.
    pub(crate) thumb_cache_bytes: Option<u64>,
    /// The Immich thumbnail cache's size, computed when the Settings dialog opens.
    ///
    /// ⚠ Its own field rather than a sum: the two caches have separate budgets, so a single number
    /// could not tell you which one is full.
    pub(crate) immich_thumb_cache_bytes: Option<u64>,
    /// The rendered-preview cache's size, computed when the Settings dialog opens (M9.14).
    pub(crate) preview_cache_bytes: Option<u64>,
    /// The Settings dialog's draft while it is open — applied on Apply, dropped on Cancel.
    pub(crate) settings_draft: Option<modals::settings::SettingsDraft>,
    /// One chronological order over photo edits *and* preset deletions, so Ctrl+Z walks
    /// back through both. The payloads stay where they are — photo snapshots in
    /// `Session::history`, deleted presets in the trash — and this only records which of
    /// the two the next undo should pop.
    pub(crate) actions: ActionLog,
    /// The creative looks found on disk, and whether the one this photo names actually loaded.
    ///
    /// ⚠ Cached rather than rescanned per frame, exactly like `camera_profiles_cache` and for the
    /// same reason. Refreshed when a photo opens.
    pub(crate) looks: rustroom_app::looks::LooksState,
}
