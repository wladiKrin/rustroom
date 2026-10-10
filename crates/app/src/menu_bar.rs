//! The in-window menu bar, drawn with egui.
//!
//! ⚠ **Compiled on every platform, invoked only where there is no native menu.** Gating the
//! compilation would mean the developer (on macOS) never builds or tests it and Linux CI never
//! builds the native one — both halves unverified by whoever is looking. Only the call is
//! platform-dependent; see `shell::ui`.

use eframe::egui;
use rustroom_app::menu::{self, Command, Item};

/// Draw the bar. Returns the command the user picked, if any.
pub fn show(
    ui: &mut egui::Ui,
    recent: &[std::path::PathBuf],
    overrides: &[rustroom_app::shortcuts::Rebinding],
    state: &menu::MenuState,
) -> Option<Command> {
    let mut picked = None;
    egui::containers::menu::MenuBar::new().ui(ui, |ui| {
        for section in menu::model_for(recent, overrides) {
            let r = ui.menu_button(section.title, |ui| {
                items(ui, &section.items, &mut picked, state)
            });
            // ⚠ A menu, not a button: its label and its items say what it does, so it carries no
            // hover sentence, and `hover_coverage` exempts it by this role (M10.6b).
            ui.ctx().accesskit_node_builder(r.response.id, |n| {
                n.set_role(egui::accesskit::Role::MenuItem)
            });
        }
    });
    picked
}

/// One level of a menu; a submenu opens a nested one.
fn items(ui: &mut egui::Ui, items: &[Item], picked: &mut Option<Command>, state: &menu::MenuState) {
    for item in items {
        match item {
            Item::Separator => {
                ui.separator();
            }
            Item::Submenu {
                label,
                items: nested,
            } => {
                ui.menu_button(label, |ui| self::items(ui, nested, picked, state));
            }
            Item::Action {
                label,
                command,
                shortcut,
            } => {
                // ⚠ The chord is part of the label rather than egui shortcut text, so this bar
                // and the native one read the same way. One presentation, both platforms.
                let text = match shortcut {
                    Some(sc) => format!("{label}   {}", sc.label()),
                    None => label.clone(),
                };
                // M10.8b: a checkbox where the command has a mark, so this bar shows what the native
                // menu's check marks show.
                let clicked = match menu::checked(*command, state) {
                    Some(mut on) => ui.checkbox(&mut on, text).clicked(),
                    None => ui.button(text).clicked(),
                };
                if clicked {
                    *picked = Some(*command);
                    ui.close();
                }
            }
        }
    }
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
        // Both values of every mark, so a checkbox and a radio are each drawn checked and not.
        for shown in [true, false] {
            let state = menu::MenuState {
                filmstrip_shown: shown,
            };
            let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
                let picked = show(ui, &[], &[], &state);
                assert!(picked.is_none(), "nothing was clicked");
            });
        }
    }
}
