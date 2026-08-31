//! A whole `RustroomApp`, built for tests.
//!
//! ⚠⚠ **This exists because the canvas is the least-tested code in the project and the barrier was
//! always construction, never intent.** Every untested drawing function takes `&mut RustroomApp`
//! and needs a live `Session`; a `Session` needs a render engine and textures. It turns out none
//! of that needs disk, libraw or a GPU — a synthetic `ImageBuf` and `ctx.load_texture` suffice.
//!
//! ⚠ The render engine's worker thread really does spawn. Tests must never wait on it: assert on
//! *requests and state*, never on rendered pixels.

use crate::*;
use eframe::egui;

/// A temp directory that removes itself.
///
/// ⚠ Uniqueness is **structural** — process id plus a counter — never the clock. Two tests
/// entering the same tick collide, `cargo test` runs them in parallel, and this project has paid
/// for that twice: once in `image-decode`, once in `app`, where it surfaced as an *unrelated
/// pre-existing test* failing during a sabotage run.
pub struct TestDir(pub std::path::PathBuf);

impl TestDir {
    pub fn new(tag: &str) -> Self {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "rustroom_testapp_{tag}_{}_{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
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

fn texture(ctx: &egui::Context, name: &str, w: usize, h: usize) -> egui::TextureHandle {
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
    let stub =
        |_: &std::path::Path| -> Result<(core_image::ImageBuf, String, f32, LensFacts), String> {
            Ok((
                core_image::ImageBuf::new(8, 4),
                "test".to_string(),
                1.0,
                LensFacts::default(),
            ))
        };
    RustroomApp {
        // ⚠ A real scanner, with a reader that never runs: nothing queues work in these tests, so
        // its workers park immediately and cost nothing.
        filter: Default::default(),
        metadata: rustroom_app::metadata_scanner::MetadataScanner::new(|| {}),
        camera_profiles_cache: Vec::new(),
        os_manages_colour: false,
        view: AppView::Develop,
        last_view: AppView::Develop,
        library: None,
        session: None,
        pending: None,
        decoder: DecodeWorker::new(stub, 2, || {}),
        full_decoder: DecodeWorker::new(stub, 1, || {}),
        full_pending: None,
        cache: BufferCache::new(BUFFER_CACHE_CAP),
        histories: HistoryStash::default(),
        status: String::new(),
        show_filmstrip: false,
        brush_size: 40.0,
        brush_hardness: 50.0,
        brush_flow: 100.0,
        brush_erase: false,
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
        export_draft: ExportOptions::default(),
        exporter: None,
        export_progress: String::new(),
        export_collision: None,
        export_apply_all: false,
        filmstrip_scrolled_to: None,
        export_preset_store: ExportPresetStore::new(dir.0.join("export_presets")),
        export_preset_list: Vec::new(),
        selected_export_preset: None,
        preset_store: PresetStore::new(dir.0.join("presets"), dir.0.join("trash")),
        preset_list: Vec::new(),
        settings_store: SettingsStore::new(dir.0.join("settings.json")),
        settings: Settings::default(),
        // ⚠ In memory, so tests never touch the user's real catalog.
        catalog: catalog::db::Catalog::open_in_memory().ok(),
        modal: Modal::None,
        actions: ActionLog::default(),
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
    let engine = RenderEngine::new(buf, 1.0, None, || {});
    app.session = Some(Session {
        path: std::path::PathBuf::from("/test/a.RAF"),
        copy: 0,
        name: None,
        snapshots: Vec::new(),
        engine,
        edits: Edits::default(),
        texture: Some(texture(ctx, "frame", w, h)),
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
        guides: Vec::new(),
        active_guide: None,
        show_guides: false,
        active_mask_handle: None,
        rendered_region: None,
        requested_region: None,
        fit_image_size: egui::vec2(w as f32, h as f32),
        rendered_long: w.max(h) as f32,
        buffer_long: w.max(h),
        buffer_is_full_res: true,
        opened_at: std::time::Instant::now(),
        render_rung: 0,
        display_space: color::OutputSpace::Srgb,
    });
}

/// A `Library` of `n` synthetic entries, with no files behind them.
///
/// ⚠ Exhaustive field literal on purpose, following `for_test`: a new field on `Library` is a
/// compile error here, which forces a decision about what a test should see.
pub fn library_of(n: usize) -> Library {
    let scan: Vec<catalog::CatalogEntry> = (0..n)
        .map(|i| catalog::CatalogEntry {
            path: std::path::PathBuf::from(format!("/nonexistent/rustroom_{i:05}.RAF")),
            file_name: format!("rustroom_{i:05}.RAF"),
            modified: std::time::SystemTime::UNIX_EPOCH,
            edited: None,
        })
        .collect();
    let entries =
        rustroom_app::entries::build(&scan, |_| rustroom_app::sidecar::Sidecar::default());
    let mut lib = Library {
        source: LibrarySource::Folder(std::path::PathBuf::from("/nonexistent")),
        scan,
        entries,
        cache: rustroom_app::thumb_cache::ThumbCache::with_default_budget(),
        last_want: Vec::new(),
        selection: Selection::default(),
        loader: ThumbLoader::new(color::OutputSpace::Srgb, || {}),
        visible: Vec::new(),
        marks: std::collections::HashMap::new(),
        captures: std::collections::HashMap::new(),
        band: None,
    };
    lib.refilter(&Default::default());
    lib
}
