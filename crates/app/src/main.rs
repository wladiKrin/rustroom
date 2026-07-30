use catalog::CatalogEntry;
use edit_model::Edits;
use eframe::egui;
use rustroom_app::crop_tool;
use rustroom_app::histogram::histogram_rgba;
use rustroom_app::history::History;
use rustroom_app::render_engine::{render_once, RenderEngine};
use rustroom_app::thumb_loader::ThumbLoader;
use std::path::PathBuf;
use std::sync::Arc;

const PREVIEW_MAX_DIM: usize = 1600;

fn main() -> eframe::Result<()> {
    let native_options = eframe::NativeOptions {
        renderer: eframe::Renderer::Wgpu,
        ..Default::default()
    };
    eframe::run_native(
        "Rustroom",
        native_options,
        Box::new(|_cc| Ok(Box::new(RustroomApp::default()))),
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

#[derive(Default)]
struct RustroomApp {
    view: AppView,
    library: Option<Library>,
    session: Option<Session>,
    status: String,
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

impl RustroomApp {
    /// Save the current session's edits to its `<file>.json` sidecar, if it has any.
    /// Called before switching photos / on exit so edits are never lost.
    fn persist_current_session(&self) {
        if let Some(s) = &self.session {
            if s.edits != Edits::default() {
                let _ = s.edits.to_json_file(&s.path.with_extension("json"));
            }
        }
    }

    /// Decode `path` (half-size) and set it up as the current editing `Session`.
    fn open_path(&mut self, ctx: &egui::Context, path: PathBuf) {
        self.persist_current_session();
        self.status = format!("Decoding {}…", path.display());
        match raw_decode::decode_raw(&path, &raw_decode::DecodeOptions::preview()) {
            Ok(decoded) => {
                let make_model = format!("{} {}", decoded.metadata.make, decoded.metadata.model);
                let buffer = Arc::new(decoded.image);

                // Load a sidecar (`<file>.json`) if present.
                let sidecar = path.with_extension("json");
                let edits = if sidecar.exists() {
                    edit_model::Edits::from_json_file(&sidecar).unwrap_or_default()
                } else {
                    Edits::default()
                };

                // Baseline render (neutral edits) for before/after — before `buffer` moves.
                let bf = render_once(&buffer, &Edits::default(), PREVIEW_MAX_DIM);
                let baseline = ctx.load_texture(
                    "baseline",
                    egui::ColorImage::from_rgba_unmultiplied([bf.width, bf.height], &bf.rgba),
                    egui::TextureOptions::LINEAR,
                );

                let ctx2 = ctx.clone();
                let engine = RenderEngine::new(buffer, move || ctx2.request_repaint());
                engine.request(edits.clone(), PREVIEW_MAX_DIM);

                self.session = Some(Session {
                    path,
                    engine,
                    edits: edits.clone(),
                    texture: None,
                    baseline,
                    hist: [[0; 256]; 3],
                    make_model,
                    zoom: 1.0,
                    pan: egui::Vec2::ZERO,
                    history: History::new(edits),
                    show_before: false,
                    crop_mode: false,
                    active_handle: None,
                });
                self.status.clear();
            }
            Err(e) => self.status = format!("Decode failed: {e}"),
        }
    }

    fn open_dialog(&mut self, ctx: &egui::Context) {
        let Some(path) = rfd::FileDialog::new()
            .add_filter(
                "RAW",
                &["cr2", "cr3", "nef", "arw", "raf", "dng", "orf", "rw2"],
            )
            .pick_file()
        else {
            return;
        };
        self.open_path(ctx, path);
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
                    "No RAW files in this folder.".into()
                } else {
                    format!("{n} photos")
                };
            }
            Err(e) => self.status = format!("Could not read folder: {e}"),
        }
    }

    fn open_entry(&mut self, ctx: &egui::Context, index: usize) {
        let Some(lib) = &self.library else { return };
        let Some(entry) = lib.entries.get(index) else {
            return;
        };
        let path = entry.path.clone();
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
                let decoded =
                    raw_decode::decode_raw(&src, &raw_decode::DecodeOptions::linear_scene())?;
                let rendered = pipeline::build_pipeline(&edits).run(decoded.image);
                export::save_jpeg(&rendered, &out, 92)?;
                edits.to_json_file(&out.with_extension("json"))?;
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
                s.engine
                    .request(preview_edits(&s.edits, s.crop_mode), PREVIEW_MAX_DIM);
            }
            if (redo_click || kredo) && s.history.redo() {
                s.edits = s.history.get().clone();
                s.engine
                    .request(preview_edits(&s.edits, s.crop_mode), PREVIEW_MAX_DIM);
            }
            ui.toggle_value(&mut s.show_before, "Before/After");
            if ui.toggle_value(&mut s.crop_mode, "Crop").clicked() {
                s.engine
                    .request(preview_edits(&s.edits, s.crop_mode), PREVIEW_MAX_DIM);
            }
        }
    }

    /// The Develop editor body: right adjustment panel + central image canvas.
    fn develop_body(&mut self, ui: &mut egui::Ui) {
        egui::Panel::right("panel")
            .default_size(240.0)
            .show(ui, |ui| {
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
                        s.engine
                            .request(preview_edits(&s.edits, s.crop_mode), PREVIEW_MAX_DIM);
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

        egui::CentralPanel::default().show(ui, |ui| match &mut self.session {
            Some(s) => {
                if let Some(tex) = &s.texture {
                    let shown = if s.show_before { &s.baseline } else { tex };
                    let avail = ui.available_size();
                    let tex_size = shown.size_vec2();
                    let fit = (avail.x / tex_size.x).min(avail.y / tex_size.y).min(1.0);
                    let draw = tex_size * fit * s.zoom;
                    let img_rect =
                        egui::Rect::from_center_size(ui.max_rect().center() + s.pan, draw);
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
                            s.zoom = (s.zoom * (1.0 + scroll * 0.001)).clamp(1.0, 8.0);
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
                    ui.label("Open a RAW to begin (Open file… button).");
                });
            }
        });
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
                    ui.label("No RAW files in this folder.");
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

        // Pull any finished preview frame into a texture.
        if let Some(s) = &mut self.session {
            if let Some(frame) = s.engine.poll_latest() {
                s.hist = histogram_rgba(&frame.rgba);
                let img = egui::ColorImage::from_rgba_unmultiplied(
                    [frame.width, frame.height],
                    &frame.rgba,
                );
                s.texture = Some(ctx.load_texture("preview", img, egui::TextureOptions::LINEAR));
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
                    self.develop_toolbar(ui, &ctx);
                }
                if !self.status.is_empty() {
                    ui.label(&self.status);
                }
            });
        });

        match self.view {
            AppView::Develop => self.develop_body(ui),
            AppView::Library => self.library_view(ui, &ctx),
        }
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
