//! The Settings window.

use super::dialog;
use crate::{theme, view_quality_hint, Modal, RustroomApp};
use eframe::egui;
use rustroom_app::settings::{Accent, PanDrag, ThemeChoice};

/// The Settings window: preferences that survive a restart.
pub fn show(app: &mut RustroomApp, ctx: &egui::Context) {
    let mut confirm = app.settings.confirm_preset_delete;
    let mut tier = app.settings.view_demosaic;
    let mut theme_choice = app.settings.theme;
    let mut accent = app.settings.accent;
    let mut pan_drag = app.settings.pan_drag;
    let mut display_space = app.settings.display_space;
    // Copied out before the closure borrows `app`.
    let os_manages_colour = app.os_manages_colour;
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

        // ⚠ A **runtime** branch, not `#[cfg]`, so both halves compile on every platform. The
        // menu-bar work shipped a `#[cfg]`-gated UI branch that the developer never compiled and
        // Linux CI never exercised, leaving both halves unverified by whoever was looking.
        if os_manages_colour {
            // ⚠ A note, not a disabled dropdown. There is nothing to choose: the OS is using the
            // display's calibrated profile and following the window between monitors, which no
            // value here could improve on.
            ui.horizontal(|ui| {
                ui.label("Display");
                ui.label("handled by macOS");
            });
            theme::hint(
                ui,
                "Your display's own colour profile is applied by the system, including any \
                 calibration, and it follows the window if you move it to another monitor.",
            );
        } else {
            ui.horizontal(|ui| {
                ui.label("Display");
                egui::ComboBox::from_id_salt("display_space_pick")
                    .selected_text(display_space.label())
                    .show_ui(ui, |ui| {
                        // ⚠ `DISPLAYS`, not `ALL`: no ProPhoto monitor exists, so offering it is
                        // a pure footgun.
                        for s in color::OutputSpace::DISPLAYS {
                            ui.selectable_value(&mut display_space, s, s.label());
                        }
                    });
            });
            theme::hint(
                ui,
                "The primaries of your monitor. Most are sRGB; recent Apple displays and many \
                 laptops are Display P3. Affects only what you see — never an export, and never \
                 the histogram.",
            );
        }
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

        ui.horizontal(|ui| {
            ui.label("Pan with");
            egui::ComboBox::from_id_salt("pan_drag_pick")
                .selected_text(pan_drag.label())
                .show_ui(ui, |ui| {
                    for b in PanDrag::ALL {
                        ui.selectable_value(&mut pan_drag, b, b.label());
                    }
                });
        });
        theme::hint(
            ui,
            "Scrolling and two-finger swipe always pan — this is the extra drag gesture, \
             for when a middle button is missing or broken.",
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
    // ⚠ Guarded like its neighbours: `show` runs every frame the dialog is open, and this one
    // rebuilds the thumbnail loader and re-renders, so an unguarded call would reload the whole
    // folder's thumbnails sixty times a second.
    if display_space != app.settings.display_space {
        app.apply_display_space(display_space, ctx);
    }
    // ⚠ A change guard like its neighbours, though for a weaker reason: this has no side
    // effect beyond the write, because the canvas reads `settings.pan_drag` afresh every
    // frame — no worker rebuild, no re-decode. The guard is here so the settings file is not
    // rewritten on every frame the dialog is open.
    if pan_drag != app.settings.pan_drag {
        app.settings.pan_drag = pan_drag;
        app.save_settings();
    }
    if close {
        app.modal = Modal::None;
    }
}
