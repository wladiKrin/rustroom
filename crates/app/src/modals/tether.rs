//! File → Tethered capture… (M10.18a): the camera found, where the shots go, and a preset for each.

use super::{dialog, Footer};
use crate::theme::Hover as _;
use crate::{theme, RustroomApp};
use eframe::egui;
use rustroom_app::tether_options;

pub fn show(app: &mut RustroomApp, ctx: &egui::Context) {
    let Some(d) = app.tether.dialog.as_mut() else {
        app.modal = super::Modal::None;
        return;
    };
    let ready = d.camera.is_some();
    let today = tether_options::today();
    let folder = d.draft.folder(&today);
    let mut pick = false;
    let out = dialog(
        ctx,
        &mut app.settings,
        "tether",
        "Tethered capture",
        theme::DialogSize::Standard,
        Footer::confirm(
            "Start",
            "Downloads each shot from the camera into this folder as it is taken",
            ready,
            "Cancel",
        ),
        |ui| {
            let d = app.tether.dialog.as_mut().expect("checked above");
            egui::Grid::new("tether_grid")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    ui.label("Camera").hover(
                        "A Fujifilm camera sends its shots only when set to USB tether shooting auto",
                    );
                    match (&d.camera, &d.trouble) {
                        (Some(c), _) => {
                            ui.label(c);
                        }
                        (None, Some(t)) => theme::error_hint(ui, t),
                        (None, None) => theme::hint(ui, "Looking for a camera"),
                    }
                    ui.end_row();
                    ui.label("Destination");
                    ui.horizontal(|ui| {
                        if ui
                            .button("Choose…")
                            .hover("Chooses the folder the sessions are made in")
                            .clicked()
                        {
                            pick = true;
                        }
                        theme::hint(ui, &d.draft.destination.display().to_string());
                    });
                    ui.end_row();
                    ui.label("Session");
                    ui.add(
                        egui::TextEdit::singleline(&mut d.draft.session)
                            .hint_text(&today)
                            .desired_width(f32::INFINITY),
                    )
                    .hover("Names the session's folder, the day's date when empty");
                    ui.end_row();
                    ui.label("Develop preset");
                    let shown = d.draft.preset.clone().unwrap_or_else(|| "None".into());
                    egui::ComboBox::from_id_salt("tether_preset")
                        .selected_text(shown)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut d.draft.preset, None, "None");
                            for n in &d.preset_names {
                                ui.selectable_value(&mut d.draft.preset, Some(n.clone()), n);
                            }
                        })
                        .response
                        .hover("Applies this preset to each shot as it arrives");
                    ui.end_row();
                });
            theme::hint(ui, &format!("Shots go to {}", folder.display()));
        },
    );
    if pick {
        if let Some(f) = rfd::FileDialog::new()
            .set_title("Choose a folder for tethered sessions")
            .pick_folder()
        {
            if let Some(d) = app.tether.dialog.as_mut() {
                d.draft.destination = f;
            }
        }
    }
    if out.primary && ready {
        app.start_tether(ctx);
    } else if out.dismissed {
        app.close_tether_dialog();
    }
}
