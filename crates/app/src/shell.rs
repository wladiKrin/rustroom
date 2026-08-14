//! The frame loop: the top toolbar, the Library/Develop dispatch, and the per-frame pumps.
//!
//! ⚠ This is a **trait impl moved to another module of the same crate**, which Rust allows —
//! so unlike every other move in this refactor, `self` stays `self` and the body is verbatim.

use crate::{develop, library, modals, AppView, RustroomApp, PREVIEW_MAX_DIM};
use eframe::egui;
use rustroom_app::histogram::histogram_rgba;
use rustroom_app::render_engine::RegionRect;
use rustroom_app::{format_filter, view_region};

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
        self.maybe_request_full_res(&ctx);
        self.pump_full_res(&ctx);
        // Outside the view match: Ctrl+V must reach a selection made in the Library grid,
        // not only one made in the filmstrip.
        self.handle_clipboard_keys(&ctx);
        self.pump_export(&ctx);

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
                // A whole-image frame is the only thing that can tell us the post-geometry
                // size, which changes with the crop and the straighten angle.
                //
                // ⚠ Scaled by the ratio of *caps*, not accepted only at the fit rung. Both are
                // pre-geometry caps, so the crop factor cancels and this is exact: a frame is
                // `max_dim x crop_factor` and the fit size is `fit_cap x crop_factor`. That
                // generalisation is what lets a whole-image frame at a *higher* rung — which
                // now happens whenever the window is larger than the fit texture — keep this
                // fresh, where the old equality test would silently have rejected it.
                //
                // It is still not derived from a rendered size feeding back: `fit_cap` and
                // `frame.max_dim` are both caps, and neither comes from `fit_image_size`.
                let fit_cap = view_region::fit_cap(s.buffer_long, PREVIEW_MAX_DIM);
                if frame.region.is_none() && frame.max_dim > 0 {
                    let ratio = fit_cap as f32 / frame.max_dim as f32;
                    s.fit_image_size =
                        egui::vec2(frame.width as f32 * ratio, frame.height as f32 * ratio);
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
                // Format chips, only when there is a choice to make: a lone toggle can only
                // ever blank the grid, which is a footgun with no upside.
                let formats = self
                    .library
                    .as_ref()
                    .map(|lib| format_filter::formats_present(&lib.entries))
                    .unwrap_or_default();
                if formats.len() >= 2 {
                    if let Some(lib) = &mut self.library {
                        for (ext, count) in &formats {
                            let mut shown = !lib.filter.is_hidden(ext);
                            let label = format!("{} ({count})", ext.to_uppercase());
                            if ui.toggle_value(&mut shown, label).changed() {
                                lib.filter.toggle(ext);
                                lib.refilter();
                            }
                        }
                        ui.separator();
                    }
                }
                if self.view == AppView::Develop && self.session.is_some() {
                    let decoding = self.pending.is_some();
                    ui.add_enabled_ui(!decoding, |ui| develop::toolbar(self, ui, &ctx));
                }
                if !self.export_progress.is_empty() {
                    ui.separator();
                    ui.label(&self.export_progress);
                    if ui.button("Cancel").clicked() {
                        self.cancel_export();
                    }
                } else if !self.status.is_empty() {
                    ui.label(&self.status);
                }
            });
        });

        match self.view {
            AppView::Develop => develop::body(self, ui, &ctx),
            AppView::Library => library::show(self, ui, &ctx),
        }

        modals::draw(self, &ctx);
    }
}
