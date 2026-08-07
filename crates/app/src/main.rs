use catalog::CatalogEntry;
use edit_model::EditGroups;
use edit_model::Edits;
use eframe::egui;
use rustroom_app::buffer_cache::BufferCache;
use rustroom_app::crop_tool;
use rustroom_app::decode_worker::{DecodeWorker, Decoded};
use rustroom_app::histogram::histogram_rgba;
use rustroom_app::history::History;
use rustroom_app::presets::{Preset, PresetStore};
use rustroom_app::render_engine::{RegionRect, RenderEngine};
use rustroom_app::settings::{ConfirmOutcome, Settings, SettingsStore};
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
    selected: Option<usize>,
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
    /// In-memory and per-session: the clipboard does not survive a restart and does not
    /// touch the OS clipboard.
    clipboard: Option<Edits>,
    /// Group selection last used for a paste, so Ctrl+V can skip the dialog.
    paste_groups: EditGroups,
    preset_store: PresetStore,
    /// Cached `preset_store.list()`, refreshed on save and delete rather than every frame.
    preset_list: Vec<Preset>,
    settings_store: SettingsStore,
    settings: Settings,
    modal: Modal,
}

impl RustroomApp {
    fn new(ctx: &egui::Context) -> Self {
        let ctx2 = ctx.clone();
        let preset_store = PresetStore::user_default();
        let preset_list = preset_store.list();
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
            preset_store,
            preset_list,
            settings_store,
            settings,
            modal: Modal::None,
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

/// The edit sidecar path for a source file: the full filename with `.json` appended
/// (e.g. `photo.RAF` → `photo.RAF.json`), so a RAW and a JPEG that share a basename get
/// distinct sidecars.
fn sidecar_path(path: &std::path::Path) -> std::path::PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(".json");
    std::path::PathBuf::from(s)
}

impl RustroomApp {
    /// Save the current session's edits to its `<file>.json` sidecar, if it has any.
    /// Called before switching photos / on exit so edits are never lost.
    fn persist_current_session(&self) {
        if let Some(s) = &self.session {
            if s.edits != Edits::default() {
                let _ = s.edits.to_json_file(&sidecar_path(&s.path));
            }
        }
    }

    /// Apply `src`'s selected groups to the current photo: one undo step, one re-render.
    ///
    /// The single point of application for both paste and presets, so the two cannot drift.
    fn apply_edits_from(&mut self, src: &Edits, groups: EditGroups) {
        let Some(s) = &mut self.session else { return };
        s.edits.merge_from(src, groups);
        s.history.push(s.edits.clone());
        s.engine.request(
            preview_edits(&s.edits, s.crop_mode),
            s.render_max_dim(),
            s.requested_region,
        );
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

    /// Ctrl+C / Ctrl+V for the edit clipboard.
    ///
    /// Gated on nothing having keyboard focus, the way `handle_develop_arrows` already is,
    /// so Ctrl+C inside the preset-name field is not hijacked.
    fn handle_clipboard_keys(&mut self, ctx: &egui::Context) {
        if self.session.is_none() || self.modal != Modal::None {
            return;
        }
        if ctx.memory(|m| m.focused()).is_some() {
            return;
        }
        let (copy, paste) = ctx.input(|i| {
            (
                i.key_pressed(egui::Key::C) && i.modifiers.command,
                i.key_pressed(egui::Key::V) && i.modifiers.command,
            )
        });
        if copy {
            // `self.session.as_ref().map(..)` rather than `if let Some(s) = &self.session`:
            // the latter holds an immutable borrow of `self` across the assignment to
            // `self.clipboard` and does not compile.
            self.clipboard = self.session.as_ref().map(|s| s.edits.clone());
            self.status = "Settings copied".to_string();
        }
        if paste {
            // Ctrl+V uses the last-used group selection with no dialog; the "Paste…"
            // button opens the chooser.
            if let Some(src) = self.clipboard.clone() {
                self.apply_edits_from(&src, self.paste_groups);
                self.status = "Settings pasted".to_string();
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
        let sidecar = sidecar_path(&path);
        let edits = if sidecar.exists() {
            edit_model::Edits::from_json_file(&sidecar).unwrap_or_default()
        } else {
            Edits::default()
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
                    selected: None,
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
                    lib.selected = Some(index);
                    e.path.clone()
                }
                None => return,
            },
            None => return,
        };
        self.open_path(ctx, path);
        self.view = AppView::Develop;
    }

    fn export_dialog(&mut self) {
        let Some(s) = &self.session else { return };
        let Some(out) = rfd::FileDialog::new()
            .add_filter("JPEG", &["jpg", "jpeg"])
            .set_file_name("export.jpg")
            .save_file()
        else {
            return;
        };
        let src = s.path.clone();
        let edits = s.edits.clone();
        self.status = "Exporting (full-res decode)…".into();
        // Full decode + render + encode is slow; run detached so the UI stays responsive.
        std::thread::spawn(move || {
            let result = (|| -> anyhow::Result<()> {
                let image = decode_for_export(&src).map_err(|e| anyhow::anyhow!(e))?;
                let rendered =
                    pipeline::build_pipeline(&edits, pipeline::RenderScale::FULL).run(image);
                export::save_jpeg(&rendered, &out, 92)?;
                edits.to_json_file(&sidecar_path(&out))?;
                Ok(())
            })();
            if let Err(e) = result {
                eprintln!("export error: {e}");
            }
        });
    }

    /// Develop-mode toolbar buttons (shown in the top panel when a photo is open).
    fn develop_toolbar(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        if ui.button("Export…").clicked() {
            self.export_dialog();
        }
        if let Some(s) = &mut self.session {
            if ui.button("Fit").clicked() {
                s.zoom = 1.0;
                s.pan = egui::Vec2::ZERO;
            }
            let undo_click = ui
                .add_enabled(s.history.can_undo(), egui::Button::new("Undo"))
                .clicked();
            let redo_click = ui
                .add_enabled(s.history.can_redo(), egui::Button::new("Redo"))
                .clicked();
            let (kundo, kredo) = ctx.input(|i| {
                let z = i.key_pressed(egui::Key::Z) && i.modifiers.command;
                (z && !i.modifiers.shift, z && i.modifiers.shift)
            });
            if (undo_click || kundo) && s.history.undo() {
                s.edits = s.history.get().clone();
                s.engine.request(
                    preview_edits(&s.edits, s.crop_mode),
                    s.render_max_dim(),
                    s.requested_region,
                );
            }
            if (redo_click || kredo) && s.history.redo() {
                s.edits = s.history.get().clone();
                s.engine.request(
                    preview_edits(&s.edits, s.crop_mode),
                    s.render_max_dim(),
                    s.requested_region,
                );
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
                // Same borrow reason as in `handle_clipboard_keys`.
                self.clipboard = self.session.as_ref().map(|s| s.edits.clone());
                self.status = "Settings copied".to_string();
            }
            if ui
                .add_enabled(self.clipboard.is_some(), egui::Button::new("Paste…"))
                .clicked()
            {
                self.modal = Modal::PasteSettings;
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

    /// The Develop editor body: right adjustment panel + central image canvas.
    fn develop_body(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        self.handle_develop_arrows(ctx);
        self.handle_clipboard_keys(ctx);
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
                            s.history.push(s.edits.clone());
                        }

                        ui.separator();
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
                            draw_crop_overlay(ui, s, &resp, img_rect);
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
            let cur = lib.selected.unwrap_or(0);
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
            if self.library.as_ref().and_then(|l| l.selected) != Some(i) {
                self.open_entry(ctx, i);
            }
        }
    }

    /// Bottom strip of the loaded folder's thumbnails; click a cell to open that photo.
    /// The panel is resizable (drag its top border); thumbnails scale with its height.
    fn filmstrip(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        let mut open_index: Option<usize> = None;
        egui::Panel::bottom("filmstrip")
            .resizable(true)
            .default_size(96.0)
            .show(ui, |ui| {
                let Some(lib) = &self.library else { return };
                // Fill the panel's height so egui stores the panel size (not the smaller
                // content size) — otherwise a resizable bottom panel shrinks back each frame.
                let avail_h = ui.available_height();
                ui.set_min_height(avail_h);
                let cell_h = (avail_h - 8.0).clamp(48.0, 400.0);
                let box_size = egui::vec2(cell_h * 1.5, cell_h);
                egui::ScrollArea::horizontal().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for i in 0..lib.entries.len() {
                            let resp = ui
                                .allocate_ui(box_size, |ui| match &lib.thumbs[i] {
                                    Some(tex) => {
                                        ui.add(
                                            egui::Image::new(tex)
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
                                .interact(egui::Sense::click());
                            if lib.selected == Some(i) {
                                ui.painter().rect_stroke(
                                    resp.rect,
                                    egui::CornerRadius::same(3),
                                    egui::Stroke::new(2.0, egui::Color32::from_rgb(120, 160, 240)),
                                    egui::StrokeKind::Inside,
                                );
                            }
                            if resp.clicked() {
                                open_index = Some(i);
                            }
                        }
                    });
                });
            });
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
                let cur = lib.selected.unwrap_or(0);
                if i.key_pressed(egui::Key::ArrowRight) {
                    lib.selected = Some((cur + 1).min(n - 1));
                }
                if i.key_pressed(egui::Key::ArrowLeft) {
                    lib.selected = Some(cur.saturating_sub(1));
                }
                if i.key_pressed(egui::Key::Enter) {
                    open_index = lib.selected;
                }
            });

            egui::ScrollArea::vertical().show(ui, |ui| {
                let cols = (ui.available_width() / CELL).floor().max(1.0) as usize;
                egui::Grid::new("thumb_grid").show(ui, |ui| {
                    for i in 0..lib.entries.len() {
                        let box_size = egui::vec2(CELL - 12.0, (CELL - 12.0) * 0.66);
                        let resp = ui
                            .allocate_ui(egui::vec2(CELL, CELL), |ui| {
                                ui.vertical_centered(|ui| {
                                    match &lib.thumbs[i] {
                                        Some(tex) => {
                                            ui.add(
                                                egui::Image::new(tex)
                                                    .max_size(box_size)
                                                    .maintain_aspect_ratio(true),
                                            );
                                        }
                                        None => {
                                            let (rect, _) = ui.allocate_exact_size(
                                                box_size,
                                                egui::Sense::hover(),
                                            );
                                            ui.painter().rect_filled(
                                                rect,
                                                egui::CornerRadius::same(3),
                                                egui::Color32::from_gray(60),
                                            );
                                        }
                                    }
                                    ui.label(
                                        egui::RichText::new(&lib.entries[i].file_name).small(),
                                    );
                                });
                            })
                            .response
                            .interact(egui::Sense::click());

                        if lib.selected == Some(i) {
                            ui.painter().rect_stroke(
                                resp.rect,
                                egui::CornerRadius::same(3),
                                egui::Stroke::new(2.0, egui::Color32::from_rgb(120, 160, 240)),
                                egui::StrokeKind::Inside,
                            );
                        }
                        if resp.clicked() {
                            lib.selected = Some(i);
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
                if !self.status.is_empty() {
                    ui.label(&self.status);
                }
            });
        });

        match self.view {
            AppView::Develop => self.develop_body(ui, &ctx),
            AppView::Library => self.library_view(ui, &ctx),
        }
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
fn draw_crop_overlay(ui: &egui::Ui, s: &mut Session, resp: &egui::Response, img_rect: egui::Rect) {
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
