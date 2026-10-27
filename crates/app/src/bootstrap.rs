//! Building the application at startup: every store opened, every worker started, every field
//! given its first value.

use crate::decode::{decode_for_edit, full_res_worker};
#[cfg(target_os = "macos")]
use crate::{display_colorspace, native_menu};
use crate::{
    theme, AppView, LibraryMode, Modal, RustroomApp, BUFFER_CACHE_CAP, MAX_CONCURRENT_DECODES,
    PREVIEW_TEXTURE_BUDGET,
};
use edit_model::EditGroups;
use rustroom_app::action_log::ActionLog;
use rustroom_app::buffer_cache::BufferCache;
use rustroom_app::decode_worker::DecodeWorker;
use rustroom_app::edit_history::HistoryStash;
use rustroom_app::export_presets::ExportPresetStore;
use rustroom_app::presets::PresetStore;
use rustroom_app::settings::SettingsStore;

impl RustroomApp {
    /// ⚠ Takes the whole `CreationContext`, not just the egui one, because the window handle is
    /// needed to reach the `CAMetalLayer` — see `display_colorspace`.
    pub(crate) fn new(cc: &eframe::CreationContext<'_>) -> Self {
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
        let preset_folders = preset_store.folders();
        let export_preset_store = ExportPresetStore::user_default();
        let export_preset_list = export_preset_store.list();
        let settings_store = SettingsStore::user_default();
        let settings = settings_store.load();
        let preview_store = rustroom_app::preview_store::PreviewStore::user_default(
            settings.preview_budget_mb as u64 * 1024 * 1024,
        );
        #[cfg(target_os = "macos")]
        native_menu::install(ctx, &settings.recent_folders, &settings.shortcuts, true);
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
            ghost_store: rustroom_app::thumb_store::ThumbStore::user_default(
                settings.thumb_budget_mb as u64 * 1024 * 1024,
            ),
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
            preview_misses: Default::default(),
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
            remove_tool: Default::default(),
            retouch: Default::default(),
            brush_size: rustroom_app::brush_tool::DEFAULTS.size,
            brush_hardness: rustroom_app::brush_tool::DEFAULTS.hardness,
            brush_flow: rustroom_app::brush_tool::DEFAULTS.flow,
            brush_erase: false,
            brush_auto_mask: false,
            brush_tolerance: rustroom_app::brush_tool::DEFAULTS.tolerance,
            hsl_channel: Default::default(),
            curve_mode: Default::default(),
            curve_channel: Default::default(),
            mask_hsl_channel: Default::default(),
            mask_grade_range: Default::default(),
            mask_add_mode: None,
            show_depth_map: false,
            show_lens_depth: false,
            grade_range: Default::default(),
            new_collection: String::new(),
            new_smart: String::new(),
            last_period: None,
            new_keyword: String::new(),
            new_keyword_parent: None,
            keyword_field: Default::default(),
            keyword_suggest: Default::default(),
            rail_data: Default::default(),
            rename_draft: None,
            label_draft: None,
            snapshot_rename: None,
            keyword_rename: None,
            folders_collapsed: Default::default(),
            folder_marks: Default::default(),
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
            hdr: Default::default(),
            merges: Default::default(),
            ai: Default::default(),
            denoise: Default::default(),
            removals: Default::default(),
            export_progress: String::new(),
            export_collision: None,
            export_apply_all: false,
            filmstrip_scrolled_to: None,
            export_preset_store,
            export_preset_list,
            selected_export_preset: None,
            preset_store,
            preset_list,
            preset_folders,
            preset_rename: None,
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
            immich_album_names: Default::default(),
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
            immich_originals: None,
            immich_send_selection: Vec::new(),
            immich_send_source: None,
            immich_send_pending: None,
            immich_last_send_count: None,
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
        // M10.15b: the soft limit's warning — one directory listing, ~3 ms for 1,000 photos.
        app.check_immich_cache_at_startup();
        // M10.16b: AI denoise's results, the same rule — after Immich's, never over its dialog.
        if app.modal == crate::Modal::None {
            app.check_denoise_cache_at_startup();
        }
        // Last, so no other startup message covers it.
        app.report_unread_keys();
        app
    }
}
