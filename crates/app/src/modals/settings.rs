//! The Settings window.

use super::dialog;
use crate::{theme, view_quality_hint, Modal, RustroomApp};
use eframe::egui;
use rustroom_app::settings::{Accent, ThemeChoice};

/// The Settings window: preferences that survive a restart.
pub fn show(app: &mut RustroomApp, ctx: &egui::Context) {
    let mut confirm = app.settings.confirm_preset_delete;
    let mut tier = app.settings.view_demosaic;
    let mut theme_choice = app.settings.theme;
    let mut accent = app.settings.accent;
    let mut close = false;

    let dismissed = dialog(ctx, "settings", "Settings", |ui| {
        ui.checkbox(&mut confirm, "Confirm before deleting a preset");
        ui.separator();

        ui.horizontal(|ui| {
            ui.label("Theme");
            egui::ComboBox::from_id_salt("theme_pick")
                .selected_text(theme_choice.label())
                .show_ui(ui, |ui| {
                    for t in ThemeChoice::ALL {
                        ui.selectable_value(&mut theme_choice, t, t.label());
                    }
                });
        });
        ui.horizontal(|ui| {
            ui.label("Accent");
            egui::ComboBox::from_id_salt("accent_pick")
                .selected_text(accent.label())
                .show_ui(ui, |ui| {
                    for a in Accent::ALL {
                        ui.selectable_value(&mut accent, a, a.label());
                    }
                });
        });
        theme::hint(
            ui,
            "Dark is the default because a light surround changes how you judge exposure. \
             The accent marks active controls; Neutral turns it off.",
        );
        ui.separator();

        ui.horizontal(|ui| {
            ui.label("1:1 view quality");
            egui::ComboBox::from_id_salt("view_demosaic_pick")
                .selected_text(tier.label())
                .show_ui(ui, |ui| {
                    for d in raw_decode::Demosaic::ALL {
                        ui.selectable_value(&mut tier, d, d.label());
                    }
                });
            theme::hint(ui, view_quality_hint(tier));
        });
        // ⚠ Not decoration. Unlike the export setting, this cost is *recurring*: the
        // full-resolution decode is eager, so a slower tier is paid on every photo you
        // stop on. The dialog is the only place a user would find that out.
        theme::hint(
            ui,
            "RAW only, and separate from the export setting. Sets the demosaic used \
             for the zoomed view. A slower tier costs that much extra on every photo \
             you stop on, not just on export — the full-resolution decode starts on \
             its own. The measured difference is small, which is why the default is \
             Fast. Changing this re-decodes the open photo.",
        );

        ui.separator();
        if ui.button("Close").clicked() {
            close = true;
        }
    });
    close |= dismissed;

    if confirm != app.settings.confirm_preset_delete {
        app.settings.confirm_preset_delete = confirm;
        app.save_settings();
    }
    // ⚠ The inequality is what stops the worker being rebuilt and a decode fired on
    // *every frame* the dialog is open — `settings_modal` runs once per frame.
    // ⚠ The same one-frame rule: `show` runs once per frame the dialog is open, so an
    // unguarded re-apply would rebuild both themes' styles sixty times a second.
    if theme_choice != app.settings.theme || accent != app.settings.accent {
        app.apply_appearance(theme_choice, accent, ctx);
    }
    if tier != app.settings.view_demosaic {
        app.apply_view_demosaic(tier, ctx);
    }
    if close {
        app.modal = Modal::None;
    }
}
