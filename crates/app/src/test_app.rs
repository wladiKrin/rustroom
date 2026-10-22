//! A whole `RustroomApp`, built for tests.
//!
//! ⚠⚠ **This exists because the canvas is the least-tested code in the project and the barrier was
//! always construction, never intent.** Every untested drawing function takes `&mut RustroomApp`
//! and needs a live `Session`; a `Session` needs a render engine and textures. It turns out none
//! of that needs disk, libraw or a GPU — a synthetic `ImageBuf` and `ctx.load_texture` suffice.
//!
//! ⚠ The render engine's worker thread really does spawn. Tests must never wait on it: assert on
//! *requests and state*, never on rendered pixels.

use crate::modals::Modal;
use crate::{AppView, Library, LibraryMode, LibrarySource, RustroomApp, Session, BUFFER_CACHE_CAP};
use edit_model::EditGroups;
use edit_model::Edits;
use eframe::egui;
use rustroom_app::action_log::ActionLog;
use rustroom_app::batch_export::ExportOptions;
use rustroom_app::buffer_cache::BufferCache;
use rustroom_app::decode_worker::{DecodeWorker, Decoded, LensFacts};
use rustroom_app::edit_history::HistoryStash;
use rustroom_app::export_presets::ExportPresetStore;
use rustroom_app::history::History;
use rustroom_app::presets::PresetStore;
use rustroom_app::render_engine::RenderEngine;
use rustroom_app::selection::Selection;
use rustroom_app::settings::{Settings, SettingsStore};
use rustroom_app::thumb_loader::ThumbLoader;

/// A temp directory that removes itself.
///
/// ⚠ Uniqueness is **structural** — process id plus a counter — never the clock. Two tests
/// entering the same tick collide, `cargo test` runs them in parallel, and this project has paid
/// for that twice: once in `image-decode`, once in `app`, where it surfaced as an *unrelated
/// pre-existing test* failing during a sabotage run.
pub struct TestDir(pub std::path::PathBuf);

impl TestDir {
    pub fn new(tag: &str) -> Self {
        Self(test_dir::path(&format!("testapp-{tag}")))
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A flat mid-grey buffer.
pub fn buffer(w: usize, h: usize) -> std::sync::Arc<core_image::ImageBuf> {
    let mut b = core_image::ImageBuf::new(w, h);
    for v in b.data_mut().iter_mut() {
        *v = 0.5;
    }
    std::sync::Arc::new(b)
}

pub fn texture(ctx: &egui::Context, name: &str, w: usize, h: usize) -> egui::TextureHandle {
    let img = egui::ColorImage::new([w, h], vec![egui::Color32::GRAY; w * h]);
    ctx.load_texture(name, img, egui::TextureOptions::default())
}

/// A `RustroomApp` with no photo open.
///
/// ⚠ Every field is listed explicitly rather than through a `Default`. Following this project's
/// exhaustive-`Settings`-literal precedent, that is deliberate: a new field on `RustroomApp`
/// becomes a **compile error here**, forcing a decision about what a test should see.
///
/// ⚠ The returned app owns no `TestDir`, so the temp directories are removed as soon as this
/// returns. That is intentional — the stores are only ever *read* by these tests, and a missing
/// directory reads as an empty store, which is what a test wants.
///
/// ⚠ Takes no `Context`: nothing here needs one. `open_test_photo` does, for the textures.
pub fn for_test() -> RustroomApp {
    let dir = TestDir::new("app");
    let stub = |_: &std::path::Path| -> Result<
        (
            core_image::ImageBuf,
            rustroom_app::decode_worker::SourceFacts,
        ),
        String,
    > {
        Ok((
            core_image::ImageBuf::new(8, 4),
            rustroom_app::decode_worker::SourceFacts {
                label: "test".to_string(),
                source_scale: 1.0,
                ..Default::default()
            },
        ))
    };
    RustroomApp {
        // ⚠ A real scanner, with a reader that never runs: nothing queues work in these tests, so
        // its workers park immediately and cost nothing.
        filter: Default::default(),
        // ⚠ A worker whose fetcher never runs — nothing in these tests enqueues a job, and a
        // fetcher that panics is the right thing to hand it: if one ever did, the panic names the
        // test rather than letting it reach the network.
        immich: rustroom_app::immich_worker::ImmichWorker::new(
            std::sync::Arc::new(|_| panic!("a test must not talk to an Immich server")),
            || {},
        ),
        immich_albums: None,
        immich_buckets: None,
        immich_test: None,
        immich_opening: None,
        immich_assets: Default::default(),
        immich_present: Default::default(),
        immich_album_names: Default::default(),
        immich_download_pending: Default::default(),
        immich_download_total: 0,
        metadata: rustroom_app::metadata_scanner::MetadataScanner::new(|| {}),
        camera_profiles_cache: Vec::new(),
        compare_view: Default::default(),
        compare_focus: 1,
        grid_scroll: 0.0,
        survey_scroll: 0.0,
        os_manages_colour: false,
        view: AppView::Develop,
        // ⚠ The grid, not `Default::default()` spelled out — a test that has not said otherwise
        // is looking at the ordinary Library.
        library_mode: LibraryMode::Grid,
        compare_candidate: None,
        last_view: AppView::Develop,
        library: None,
        immich_stacks: Default::default(),
        session: None,
        pending: None,
        decoder: DecodeWorker::new(stub, 2, || {}),
        full_decoder: DecodeWorker::new(stub, 1, || {}),
        full_pending: None,
        cache: BufferCache::new(BUFFER_CACHE_CAP),
        ghost_store: rustroom_app::thumb_store::ThumbStore::new(dir.0.join("thumbs"), u64::MAX),
        previews: rustroom_app::preview_loader::PreviewLoader::with_loader(
            std::sync::Arc::new(|_: &rustroom_app::preview_loader::PreviewRequest| None),
            color::OutputSpace::Srgb,
            8192,
            || {},
        ),
        preview_textures: rustroom_app::thumb_cache::ThumbCache::new(crate::PREVIEW_TEXTURE_BUDGET),
        preview_store: rustroom_app::preview_store::PreviewStore::new(
            dir.0.join("previews"),
            u64::MAX,
        ),
        preview_want: Vec::new(),
        preview_misses: Default::default(),
        leave_queue: Default::default(),
        preview_builder: None,
        preview_progress: String::new(),
        histories: HistoryStash::default(),
        status: String::new(),
        show_filmstrip: false,
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
        export_draft: ExportOptions::default(),
        exporter: None,
        import: None,
        ai: Default::default(),
        denoise: Default::default(),
        removals: Default::default(),
        export_progress: String::new(),
        export_collision: None,
        export_apply_all: false,
        filmstrip_scrolled_to: None,
        export_preset_store: ExportPresetStore::new(dir.0.join("export_presets")),
        export_preset_list: Vec::new(),
        selected_export_preset: None,
        preset_store: PresetStore::new(dir.0.join("presets"), dir.0.join("trash")),
        preset_list: Vec::new(),
        preset_folders: Vec::new(),
        preset_rename: None,
        settings_store: SettingsStore::new(dir.0.join("settings.json")),
        settings: Settings::default(),
        // ⚠ In memory, so tests never touch the user's real catalog.
        catalog: catalog::db::Catalog::open_in_memory().ok(),
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
        // ⚠ Empty, and deliberately not pointed at the real looks directory: a test that found
        // the developer's own `.cube` files would pass or fail depending on whose machine it ran
        // on. Panel tests that need a look install one into their own temp directory.
        looks: Default::default(),
    }
}

/// Open a synthetic photo, so the canvas has something to draw.
///
/// ⚠ `fit_scale` and `fit_image_size` are set as a real session would have them after one frame.
/// The canvas derives its placement from them, and a zero would make every rect degenerate — the
/// overlay would then be drawn into an empty rect and every hit test would miss, which looks like
/// a logic bug and is a fixture bug.
pub fn open_test_photo(app: &mut RustroomApp, ctx: &egui::Context, w: usize, h: usize) {
    let buf = buffer(w, h);
    let engine = RenderEngine::new(
        buf.clone(),
        1.0,
        None,
        color::wb::Illuminant::default(),
        Default::default(),
        true,
        || {},
    );
    app.session = Some(Session {
        wb_baseline: color::wb::Illuminant::default(),
        baseline_ev: None,
        buffer: buf,
        picking_wb: false,
        picking_fringe: false,
        picking_negative: false,
        picking_focus: false,
        path: std::path::PathBuf::from("/test/a.RAF"),
        copy: 0,
        name: None,
        snapshots: Vec::new(),
        engine,
        edits: Edits::default(),
        texture: Some(texture(ctx, "frame", w, h)),
        stale: None,
        baseline: texture(ctx, "baseline", w, h),
        hist: [[0; 256]; 3],
        make_model: "Test Camera".to_string(),
        lens: LensFacts::default(),
        zoom: 1.0,
        fit_scale: 1.0,
        pan: egui::Vec2::ZERO,
        history: History::new(Edits::default()),
        show_before: false,
        active_handle: None,
        selected_mask: None,
        selected_component: 0,
        selected_spot: None,
        suppressed_spot: None,
        mask_shown: Vec::new(),
        spot_shown: Vec::new(),
        spot_held: Vec::new(),
        soft_next: false,
        backdrop: None,
        spot_drag: Default::default(),
        selected_eye: None,
        suppressed_eye: None,
        eye_drag: Default::default(),
        removal: Default::default(),
        lens_memo: Default::default(),
        dust: Default::default(),
        people: Default::default(),
        find_view: Default::default(),
        guides: Vec::new(),
        active_guide: None,
        shown_geo: (edit_model::Perspective::default(), 0.0),
        pending_geo: std::cell::Cell::new((edit_model::Perspective::default(), 0.0)),
        show_guides: false,
        active_mask_handle: None,
        rendered_region: None,
        requested_region: None,
        fit_image_size: egui::vec2(w as f32, h as f32),
        rendered_full: (w, h),
        rendered_long: w.max(h) as f32,
        buffer_long: w.max(h),
        full_long: w.max(h),
        buffer_is_full_res: true,
        buffer_is_ghost: false,
        opened_at: std::time::Instant::now(),
        // ⚠ The fit cap, as a real session starts. `render_max_dim` returns this now, so a
        // zero here would make every panel-driven render ask for nothing.
        render_rung: w.max(h).min(crate::PREVIEW_MAX_DIM),
        display_space: color::OutputSpace::Srgb,
        canvas_intent: color::RenderingIntent::default(),
        resampler: pipeline::Resampler::default(),
        gpu_tail: false,
        visualize: None,
        show_grade_range: false,
        grade_view: None,
        grade_view_texture: None,
        straighten_armed: false,
        straighten_drag: None,
        straighten_line: None,
        readout_bytes: None,
        clip_texture: None,
        canvas_frame_rect: None,
        canvas_view_centre: None,
        readout: None,
        soft_proof: None,
        gamut_warning: false,
    });
}

/// A `Library` of `n` synthetic entries, with no files behind them.
///
/// ⚠ Exhaustive field literal on purpose, following `for_test`: a new field on `Library` is a
/// compile error here, which forces a decision about what a test should see.
pub fn library_of(n: usize) -> Library {
    let names: Vec<String> = (0..n).map(|i| format!("rustroom_{i:05}.RAF")).collect();
    library_of_names(&names.iter().map(String::as_str).collect::<Vec<_>>())
}

/// A `Library` of exactly these file names, for tests that care which formats are present —
/// stacking, above all, where `a.RAF` beside `a.JPG` is the whole point.
///
/// ⚠ The entries are grouped by `stack::group` with stacking **on**, which is the default, so a
/// fixture naming a pair behaves as the real Library does.
pub fn library_of_names(names: &[&str]) -> Library {
    let scan: Vec<catalog::CatalogEntry> = names
        .iter()
        .map(|n| catalog::CatalogEntry {
            path: std::path::PathBuf::from(format!("/nonexistent/{n}")),
            file_name: (*n).to_string(),
            modified: std::time::SystemTime::UNIX_EPOCH,
            edited: None,
        })
        .collect();
    let entries = rustroom_app::stack::group(
        rustroom_app::entries::build(&scan, |_| rustroom_app::sidecar::Sidecar::default()),
        &rustroom_app::stack::Stacking {
            enabled: true,
            ..Default::default()
        },
        &Default::default(),
        &Default::default(),
    );
    let mut lib = Library {
        source: LibrarySource::Folder(std::path::PathBuf::from("/nonexistent")),
        scan,
        entries,
        stack_expanded: Default::default(),
        cache: rustroom_app::thumb_cache::ThumbCache::with_default_budget(),
        last_want: Vec::new(),
        selection: Selection::default(),
        // ⚠ No limit: a test must never evict, or its own fixtures could vanish mid-run.
        loader: ThumbLoader::new(color::OutputSpace::Srgb, u64::MAX, u64::MAX, || {}),
        visible: Vec::new(),
        marks: std::collections::HashMap::new(),
        captures: std::collections::HashMap::new(),
        band: None,
        copies: None,
        hand_stacks: Default::default(),
        below: Default::default(),
        members_read_at: 0,
        members_settling: None,
    };
    lib.refilter(&Default::default());
    lib
}

/// Register the loaded library's folder and photos in the app's catalog.
///
/// ⚠⚠ **A test that rates a synthetic library and reads the marks back needs this, or it cannot
/// see what it names.** `Catalog::set_marks` looks the photo up first and *returns early* when
/// there is no row — so on an uncatalogued library it silently does nothing, and `reload_marks`
/// then reads nothing back. The assertion fails against perfectly correct rating code, which is
/// exactly the wrong place to be looking.
pub fn catalog_library(app: &mut RustroomApp) {
    let Some(lib) = app.library.as_ref() else {
        return;
    };
    let Some(dir) = lib.entries.first().and_then(|e| e.id.path.parent()) else {
        return;
    };
    if let Some(cat) = &app.catalog {
        // ⚠ The files do not exist, and that is fine: `sync_folder` records whatever the
        // filesystem reports and never refuses a missing one — the same "absence is not
        // deletion" rule it follows in production.
        let _ = cat.sync_folder(dir, &lib.scan);
    }
}

/// A decoded buffer with no libraw, no disk and no GPU behind it.
///
/// Lives here rather than in one module's `mod tests` because `session_flow` and `snapshots`
/// both drive the real `finalize_session` with it.
pub fn fake_decode() -> std::sync::Arc<Decoded> {
    std::sync::Arc::new(Decoded {
        buffer: buffer(8, 4),
        label: "Test Camera".to_string(),
        baseline: rustroom_app::render_engine::RenderedFrame {
            width: 8,
            height: 4,
            rgba: std::sync::Arc::new(vec![0; 8 * 4 * 4]),
            srgb: std::sync::Arc::new(vec![0; 8 * 4 * 4]),
            region: None,
            hist: [[0; 256]; 3],
            max_dim: 8,
            fit_size: (8.0, 4.0),
            grade_view: None,
        },
        source_scale: 1.0,
        lens: LensFacts::default(),
        wb_baseline: color::wb::Illuminant::default(),
        profile: Default::default(),
        scene_referred: true,
    })
}
