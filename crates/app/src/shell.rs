//! The frame loop: the top toolbar, the Library/Develop dispatch, and the per-frame pumps.
//!
//! ⚠ This is a **trait impl moved to another module of the same crate**, which Rust allows —
//! so unlike every other move in this refactor, `self` stays `self` and the body is verbatim.

use crate::{commands, develop, library, menu_bar, modals, AppView, RustroomApp, PREVIEW_MAX_DIM};
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
        // ⚠ Before the key handling, so a menu activation and a keypress in the same frame
        // are applied in the order they arrived.
        #[cfg(target_os = "macos")]
        for cmd in crate::native_menu::poll() {
            commands::execute(self, &ctx, cmd);
        }
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

        // ⚠ Applied after the panel closure closes: `commands::execute` takes `&mut self`
        // and the closure already holds it. The same deferred-action shape the preset list
        // uses.
        let mut pending_command: Option<Command> = None;
        egui::Panel::top("top").show(ui, |ui| {
            // ⚠ macOS gets the real system menu bar instead; drawing this one too would put a
            // redundant row under a nearly-empty system menu.
            if !cfg!(target_os = "macos") {
                if let Some(cmd) = menu_bar::show(ui) {
                    pending_command = Some(cmd);
                }
            }
            ui.horizontal(|ui| {
                view_switch(ui, &mut self.view);
                if self.view == AppView::Develop && self.session.is_some() {
                    ui.separator();
                    let decoding = self.pending.is_some();
                    ui.add_enabled_ui(!decoding, |ui| develop::toolbar(self, ui, &ctx));
                }
                // ⚠ One right-aligned cluster for everything that belongs at the far end, laid
                // out **rightmost first**. Previously the develop toolbar right-aligned the
                // filename itself and then the status label was added after it — which meant
                // the status was pushed into a row that had no space left.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    match self.view {
                        AppView::Library => format_bullets(self, ui),
                        AppView::Develop => {
                            if let Some(s) = &self.session {
                                if let Some(name) = s.path.file_name() {
                                    crate::theme::caption(ui, &name.to_string_lossy());
                                }
                            }
                        }
                    }
                    if !self.export_progress.is_empty() {
                        if ui.button("Cancel").clicked() {
                            self.cancel_export();
                        }
                        ui.label(&self.export_progress);
                    } else if !self.status.is_empty() {
                        ui.label(&self.status);
                    }
                });
            });
        });

        if let Some(cmd) = pending_command {
            commands::execute(self, &ctx, cmd);
        }

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
        '+' => egui::Key::Plus,
        '-' => egui::Key::Minus,
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

/// The Library / Develop switch, as one segmented control.
///
/// ⚠ Zero spacing between the two halves is the whole of the effect: as separate buttons they
/// read as two unrelated actions, and abutted they read as one control with two states — which
/// is what a module switch is. The padded labels give it the proportions of a segment rather
/// than of a word.
fn view_switch(ui: &mut egui::Ui, view: &mut AppView) {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.selectable_value(view, AppView::Library, "  Library  ");
        ui.selectable_value(view, AppView::Develop, "  Develop  ");
    });
}

/// The Library's format filter, as a single choice: All, or one format.
///
/// ⚠ **Library only.** The filter decides what the grid and the filmstrip contain, which is a
/// browsing decision — it has no meaning while a single photo is open, and it sat in the
/// Develop toolbar for no better reason than living in a shared row.
///
/// ⚠ Shown only at two or more formats, as the chips were: a lone control here could only ever
/// blank the grid, which is a footgun with no upside.
///
/// ⚠ Radios rather than the previous per-format toggles, so the selection is legible at a
/// glance. The cost, stated: two of three formats can no longer be shown together — "All" or
/// one. The underlying hidden-set still supports it, so restoring that is a UI change only.
fn format_bullets(app: &mut RustroomApp, ui: &mut egui::Ui) {
    let formats = app
        .library
        .as_ref()
        .map(|lib| format_filter::formats_present(&lib.entries))
        .unwrap_or_default();
    if formats.len() < 2 {
        return;
    }
    let present: Vec<String> = formats.iter().map(|(e, _)| e.clone()).collect();
    let Some(lib) = &mut app.library else { return };
    // ⚠ Built rightmost-first, because the enclosing layout runs right to left; reversing here
    // is what puts All on the left of the group, where a reader starts.
    let mut pick: Option<Option<String>> = None;
    for (ext, count) in formats.iter().rev() {
        let selected = lib.filter.only_shown(&present) == Some(ext.as_str());
        let label = format!("{} ({count})", ext.to_uppercase());
        if ui.radio(selected, label).clicked() {
            pick = Some(Some(ext.clone()));
        }
    }
    if ui
        .radio(lib.filter.only_shown(&present).is_none(), "All")
        .clicked()
    {
        pick = Some(None);
    }
    if let Some(choice) = pick {
        lib.filter.show_only(choice.as_deref(), &present);
        lib.refilter();
    }
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
