//! The creative-look controls, at the top of the Tone section beside the camera rendering.
//!
//! ⚠ Its own module rather than more lines in `adjustments.rs`, which is already 836 — the
//! standing instruction in `AGENTS.md`, *Where new code goes*. Every oversized file in this repo got that way
//! one reasonable-seeming addition at a time.

use crate::develop::panel::Edited;
use crate::theme::Hover as _;
use eframe::egui;
use rustroom_app::looks::LooksState;

/// The look dropdown, its space, and Strength.
///
/// ⚠⚠ **It must say when the camera rendering is not in force.** Selecting a `Linear` look means
/// `DisplayRenderingStage` does not run, because that LUT *is* the rendering — and two renderings
/// apparently both switched on with one silently ignored is the invisible-state failure this
/// project has recorded repeatedly.
pub fn section(
    ui: &mut egui::Ui,
    edits: &mut edit_model::Edits,
    looks: &LooksState,
    ed: &mut Edited,
) {
    // ⚠ Nothing at all when no `.cube` is installed AND this photo names none. A dropdown with
    // one entry reading "None" is a control that can only disappoint; the status line below
    // still appears if a preset brought a look this machine does not have.
    if looks.entries.is_empty() && edits.look.file.is_none() {
        return;
    }

    let chosen = edits.look.file.clone();
    let label = chosen
        .as_deref()
        .map(|f| {
            looks
                .entries
                .iter()
                .find(|e| e.file == f)
                .map(|e| e.name.clone())
                // ⚠ A look this machine does not have still shows its file name rather than
                // reading "None" — otherwise a preset's look would look like no look at all.
                .unwrap_or_else(|| f.to_owned())
        })
        .unwrap_or_else(|| "None".to_owned());

    egui::ComboBox::from_label("Look")
        .selected_text(label)
        .show_ui(ui, |ui| {
            if ui
                .selectable_label(edits.look.file.is_none(), "None")
                .clicked()
                && edits.look.file.is_some()
            {
                edits.look.file = None;
                ed.mark_changed();
            }
            let mut learned_heading = false;
            for e in &looks.entries {
                // M10.13d: the films learned from the camera, in their own group after the user's looks.
                if e.learned && !learned_heading {
                    learned_heading = true;
                    ui.separator();
                    ui.label(egui::RichText::new("Learned from the camera").weak());
                }
                let on = chosen.as_deref() == Some(e.file.as_str());
                if ui.selectable_label(on, &e.name).clicked() && !on {
                    choose(edits, e);
                    ed.mark_changed();
                }
            }
        });

    if edits.look.file.is_none() {
        // ⚠ The status still shows: a preset may have named a look that is not here, in which
        // case `file` is `Some` — but if it is `None` there is nothing more to say.
        return;
    }

    // ⚠ A learned look's space is known — it was fitted in sRGB display — so there is nothing to choose.
    let learned = edits
        .look
        .file
        .as_deref()
        .is_some_and(rustroom_app::looks::is_learned);
    if !learned {
        look_space(ui, edits, ed);
    }

    ed.help_scope = rustroom_app::slider_help::Scope::Look;
    ed.slider(
        ui,
        &mut edits.look.strength,
        0.0..=100.0,
        "Strength",
        edit_model::Look::default().strength,
    );
    ed.help_scope = rustroom_app::slider_help::Scope::Global;

    // ⚠ What to say is decided in the lib, where it is a pure function a test can reach; this
    // body only draws it. See `looks::notes`.
    for note in rustroom_app::looks::notes(edits, looks) {
        ui.colored_label(ui.visuals().warn_fg_color, note);
    }
}

/// Make `e` the photo's look. ⚠ A learned look is an sRGB display table, whatever the last look's space was.
pub(crate) fn choose(edits: &mut edit_model::Edits, e: &rustroom_app::looks::LookEntry) {
    edits.look.file = Some(e.file.clone());
    if e.learned {
        edits.look.space = edit_model::LutSpace::SrgbDisplay;
    }
}

/// The look's colour space — for a look the user added, whose file does not say.
fn look_space(ui: &mut egui::Ui, edits: &mut edit_model::Edits, ed: &mut Edited) {
    egui::ComboBox::from_label("Look space")
        .selected_text(edits.look.space.label())
        .show_ui(ui, |ui| {
            for s in edit_model::LutSpace::ALL {
                let on = edits.look.space == s;
                if ui.selectable_label(on, s.label()).clicked() && !on {
                    edits.look.space = s;
                    ed.mark_changed();
                }
            }
        })
        .response
        .hover(
            "Sets the colour space the .cube file was made for, which the file itself does not record",
        );
}
