use edit_model::Edits;
use eframe::egui;
use rustroom_app::histogram::histogram_rgba;
use rustroom_app::render_engine::RenderEngine;
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

#[derive(Default)]
struct RustroomApp {
    session: Option<Session>,
    status: String,
}

struct Session {
    path: PathBuf,
    engine: RenderEngine,
    edits: Edits,
    texture: Option<egui::TextureHandle>,
    hist: [[u32; 256]; 3],
    make_model: String,
    zoom: f32,
    pan: egui::Vec2,
}

impl RustroomApp {
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
        self.status = format!("Decoding {}…", path.display());
        // Half-size decode can take several seconds; inline for M1.4a (thread it later).
        match raw_decode::decode_raw(&path, &raw_decode::DecodeOptions::preview()) {
            Ok(decoded) => {
                let make_model = format!("{} {}", decoded.metadata.make, decoded.metadata.model);
                let buffer = Arc::new(decoded.image);
                let ctx2 = ctx.clone();
                let engine = RenderEngine::new(buffer, move || ctx2.request_repaint());
                let edits = Edits::default();
                engine.request(edits.clone(), PREVIEW_MAX_DIM);
                self.session = Some(Session {
                    path: path.clone(),
                    engine,
                    edits,
                    texture: None,
                    hist: [[0; 256]; 3],
                    make_model,
                    zoom: 1.0,
                    pan: egui::Vec2::ZERO,
                });
                self.status.clear();
            }
            Err(e) => self.status = format!("Decode failed: {e}"),
        }
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
                Ok(())
            })();
            if let Err(e) = result {
                eprintln!("export error: {e}");
            }
        });
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
            std::process::exit(0);
        }

        // Pull any finished frame into a texture.
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

        egui::Panel::top("top").show(ui, |ui| {
            ui.horizontal(|ui| {
                if ui.button("Open…").clicked() {
                    self.open_dialog(&ctx);
                }
                if self.session.is_some() && ui.button("Fit").clicked() {
                    if let Some(s) = &mut self.session {
                        s.zoom = 1.0;
                        s.pan = egui::Vec2::ZERO;
                    }
                }
                if self.session.is_some() && ui.button("Export…").clicked() {
                    self.export_dialog();
                }
                if !self.status.is_empty() {
                    ui.label(&self.status);
                }
            });
        });

        egui::Panel::right("panel")
            .default_size(240.0)
            .show(ui, |ui| {
                if let Some(s) = &mut self.session {
                    draw_histogram(ui, &s.hist);
                    ui.separator();
                    let mut changed = false;
                    changed |= ui
                        .add(
                            egui::Slider::new(&mut s.edits.white_balance.temp, -100.0..=100.0)
                                .text("Temp"),
                        )
                        .changed();
                    changed |= ui
                        .add(
                            egui::Slider::new(&mut s.edits.white_balance.tint, -100.0..=100.0)
                                .text("Tint"),
                        )
                        .changed();
                    changed |= ui
                        .add(egui::Slider::new(&mut s.edits.exposure, -5.0..=5.0).text("Exposure"))
                        .changed();
                    changed |= ui
                        .add(
                            egui::Slider::new(&mut s.edits.contrast, -100.0..=100.0)
                                .text("Contrast"),
                        )
                        .changed();
                    changed |= ui
                        .add(
                            egui::Slider::new(&mut s.edits.highlights, -100.0..=100.0)
                                .text("Highlights"),
                        )
                        .changed();
                    changed |= ui
                        .add(
                            egui::Slider::new(&mut s.edits.shadows, -100.0..=100.0).text("Shadows"),
                        )
                        .changed();
                    changed |= ui
                        .add(egui::Slider::new(&mut s.edits.whites, -100.0..=100.0).text("Whites"))
                        .changed();
                    changed |= ui
                        .add(egui::Slider::new(&mut s.edits.blacks, -100.0..=100.0).text("Blacks"))
                        .changed();
                    ui.separator();
                    if ui.button("Reset all").clicked() {
                        s.edits = Edits::default();
                        changed = true;
                    }
                    if changed {
                        s.engine.request(s.edits.clone(), PREVIEW_MAX_DIM);
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
                    let avail = ui.available_size();
                    let tex_size = tex.size_vec2();
                    let fit = (avail.x / tex_size.x).min(avail.y / tex_size.y).min(1.0);
                    let draw = tex_size * fit * s.zoom;
                    let center = ui.max_rect().center() + s.pan;
                    let resp = ui.allocate_rect(
                        egui::Rect::from_center_size(center, draw),
                        egui::Sense::drag(),
                    );
                    egui::Image::new(tex).paint_at(ui, resp.rect);
                    if resp.dragged() {
                        s.pan += resp.drag_delta();
                    }
                    let scroll = ui.input(|i| i.smooth_scroll_delta.y);
                    if scroll != 0.0 && resp.hovered() {
                        s.zoom = (s.zoom * (1.0 + scroll * 0.001)).clamp(1.0, 8.0);
                    }
                } else {
                    ui.centered_and_justified(|ui| {
                        ui.spinner();
                    });
                }
            }
            None => {
                ui.centered_and_justified(|ui| {
                    ui.label("Open a RAW to begin (Open… button).");
                });
            }
        });
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
