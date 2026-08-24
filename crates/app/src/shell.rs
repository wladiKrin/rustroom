//! The frame loop: the top toolbar, the Library/Develop dispatch, and the per-frame pumps.
//!
//! ⚠ This is a **trait impl moved to another module of the same crate**, which Rust allows —
//! so unlike every other move in this refactor, `self` stays `self` and the body is verbatim.

use crate::{commands, develop, library, modals, AppView, RustroomApp, PREVIEW_MAX_DIM};
use eframe::egui;
use rustroom_app::histogram::histogram_rgba;
use rustroom_app::menu::{self, Command};
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
        // ⚠ After `handle_clipboard_keys`, which owns ⌘C / ⌘V for the reason in
        // `pressed_command`'s doc comment.
        if let Some(cmd) = pressed_command(&ctx) {
            commands::execute(self, &ctx, cmd);
        }
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

/// Turn this frame's key presses into a command, using the menu model as the single source of
/// truth for which chord means what.
///
/// ⚠ Gated on nothing having keyboard focus — the rule arrow navigation and the rail already
/// follow. Without it, typing "d" into the Save-preset name field jumps to Develop.
///
/// ⚠ **Not used for ⌘C / ⌘V.** `egui-winit` matches the clipboard chords first and pushes
/// `Event::Copy` / `Event::Paste` *instead of* a `Key` event, so `key_pressed(Key::C)` can
/// never fire — a trap this codebase has already paid for once. Those stay in
/// `handle_clipboard_keys`, which reads the semantic events.
fn pressed_command(ctx: &egui::Context) -> Option<Command> {
    if ctx.memory(|m| m.focused()).is_some() {
        return None;
    }
    for section in menu::model() {
        for item in section.items {
            let menu::Item::Action {
                command,
                shortcut: Some(sc),
                ..
            } = item
            else {
                continue;
            };
            if sc.cmd && matches!(sc.key, 'C' | 'V') {
                continue;
            }
            let Some(key) = egui_key(sc.key) else {
                continue;
            };
            let hit = ctx.input(|i| {
                i.key_pressed(key) && i.modifiers.command == sc.cmd && i.modifiers.shift == sc.shift
            });
            if hit {
                return Some(command);
            }
        }
    }
    None
}

/// The model's key character as an egui key.
///
/// ⚠ Returns `Option` rather than panicking: an unmappable chord should make its menu item
/// mouse-only, not take the app down. `every_shortcut_maps_to_an_egui_key` makes sure none of
/// the shipped ones actually do that.
fn egui_key(c: char) -> Option<egui::Key> {
    Some(match c {
        'O' => egui::Key::O,
        'E' => egui::Key::E,
        'Z' => egui::Key::Z,
        // ⚠ Mapped even though `pressed_command` skips ⌘C / ⌘V. This table is a *total*
        // mapping from the model's key characters to egui keys; whether a given chord is
        // consulted is a separate decision, made there and documented there. A hole here
        // would make the completeness test unable to say what it means.
        'C' => egui::Key::C,
        'V' => egui::Key::V,
        'G' => egui::Key::G,
        'D' => egui::Key::D,
        '0' => egui::Key::Num0,
        '1' => egui::Key::Num1,
        '2' => egui::Key::Num2,
        '3' => egui::Key::Num3,
        '4' => egui::Key::Num4,
        ',' => egui::Key::Comma,
        '[' => egui::Key::OpenBracket,
        ']' => egui::Key::CloseBracket,
        '\\' => egui::Key::Backslash,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shortcut_maps_to_an_egui_key() {
        // ⚠ A chord in the model with no egui key is a menu item whose shortcut silently does
        // nothing on Windows and Linux — and invisible on macOS, where the native menu handles
        // the safe ones anyway.
        for section in menu::model() {
            for item in section.items {
                if let menu::Item::Action {
                    label,
                    shortcut: Some(sc),
                    ..
                } = item
                {
                    assert!(
                        egui_key(sc.key).is_some(),
                        "{label}'s shortcut key {:?} has no egui mapping",
                        sc.key
                    );
                }
            }
        }
    }
}
