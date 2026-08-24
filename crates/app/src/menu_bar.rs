//! The in-window menu bar, drawn with egui.
//!
//! ⚠ **Compiled on every platform, invoked only where there is no native menu.** Gating the
//! compilation would mean the developer (on macOS) never builds or tests it and Linux CI never
//! builds the native one — both halves unverified by whoever is looking. Only the call is
//! platform-dependent; see `shell::ui`.

use eframe::egui;
use rustroom_app::menu::{self, Command, Item};

/// Draw the bar. Returns the command the user picked, if any.
pub fn show(ui: &mut egui::Ui) -> Option<Command> {
    let mut picked = None;
    egui::containers::menu::MenuBar::new().ui(ui, |ui| {
        for section in menu::model() {
            ui.menu_button(section.title, |ui| {
                for item in section.items {
                    match item {
                        Item::Separator => {
                            ui.separator();
                        }
                        Item::Action {
                            label,
                            command,
                            shortcut,
                        } => {
                            // ⚠ The chord is part of the label rather than egui shortcut text,
                            // so this bar and the native one read the same way — the native bar
                            // cannot use a real accelerator for the unsafe chords either. One
                            // presentation, both platforms.
                            let text = match shortcut {
                                Some(sc) => format!("{label}   {}", sc.label()),
                                None => label.to_string(),
                            };
                            if ui.button(text).clicked() {
                                picked = Some(command);
                                ui.close();
                            }
                        }
                    }
                }
            });
        }
    });
    picked
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_bar_renders_every_section_without_panicking() {
        // ⚠ Thin but not worthless: this is the only thing that executes the renderer at all,
        // and egui panics on a duplicate widget id or a malformed menu — neither of which the
        // model's own tests can see. It also runs on macOS, where the bar is never shown, so
        // the renderer cannot rot unnoticed.
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            let picked = show(ui);
            assert!(picked.is_none(), "nothing was clicked");
        });
    }
}
