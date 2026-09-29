//! The Presets tool: a sectioned list — save the current edits as a preset, apply one by
//! clicking its row, delete one by its `×`. A row is tinted when the open photo already has
//! that preset, and says in weak text what the preset carries (option A of the 2026-09-20
//! canvas).
//!
//! ⚠ Was a dedicated **left panel** plus a toolbar toggle — the only feature in the app with
//! two homes. It is a rail tool now, with exactly one.
//!
//! ⚠ Drawn *outside* the panel's mutable `Session` borrow, because it is the one tool that
//! edits neither `Session` nor `Edits`: applying a preset targets the whole *selection*
//! through `app.apply_to_selection`. That is why it needs `&mut RustroomApp` and cannot sit
//! inside the tool `match` with the others.

use crate::develop::panel::{mask_components, section};
use crate::{Modal, RustroomApp};
use edit_model::EditGroups;
use eframe::egui;
use rustroom_app::action_log::ActionKind;
use rustroom_app::thumb_loader::ThumbRequest;
use crate::theme::Hover as _;

pub fn show(app: &mut RustroomApp, ui: &mut egui::Ui, ctx: &egui::Context) {
    enum Action {
        None,
        Apply(usize),
        ApplyBuiltin(usize),
        Delete(String),
        Save,
        UndoDelete,
    }
    let mut action = Action::None;
    let open = app.open_photo();
    let undo_name = match app.actions.peek_undo(open.as_ref()) {
        Some(ActionKind::PresetDelete { name }) => Some(name.clone()),
        _ => None,
    };

    let n = app.preset_list.len();
    let starters = rustroom_app::presets::builtin();
    let count = match n {
        0 => format!("{} built-in", starters.len()),
        1 => format!("1 of yours · {} built-in", starters.len()),
        n => format!("{n} of yours · {} built-in", starters.len()),
    };
    let mut out = section::header(
        ui,
        ui.make_persistent_id(("presets", "Presets")),
        true,
        section::Header::new("Presets").trailing(&count),
    );
    out.show_body(ui, |ui| {
        // ⚠ Pinned to its width, never a bare `with_layout(right_to_left)` — the 2026-09-19
        // growth bug.
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), ui.spacing().interact_size.y + 4.0),
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                let r = ui
                    .button("+ Save current…")
                    .hover("Saves this photo's settings as a new preset");
                // ⚠ The accessible name is the old sentence: the harness and a screen reader
                // get what the button does, not its short form.
                let enabled = ui.is_enabled();
                r.widget_info(|| {
                    egui::WidgetInfo::labeled(
                        egui::WidgetType::Button,
                        enabled,
                        "Save current as preset…",
                    )
                });
                if r.clicked() {
                    action = Action::Save;
                }
            },
        );

        // ⚠ Computed once per frame against the open photo's edits: `matches` clones the edits,
        // and a mask's brush strokes are not free. A handful of presets is fine; a hundred would
        // want a cache keyed on the edits.
        let current = app.session.as_ref().map(|s| s.edits.clone());
        // ⚠ The starters first, under their own caption, and never deletable: an amateur's first
        // question of any editor is "can it make this look nice in one click", and an empty
        // panel answered no (A-23).
        ui.label(egui::RichText::new("Built-in").small().weak());
        // ⚠ A preview of the open photo through each starter, rendered by the thumbnail loader
        // the grid and the filmstrip already use — a preset is chosen by eye, not by name
        // (A-23). `also_want` adds to the loader's list rather than replacing the grid's.
        let previews: Option<(&crate::Library, Vec<ThumbRequest>)> =
            match (&app.library, &open, &current) {
                (Some(lib), Some(id), Some(cur)) => Some((
                    lib,
                    starters
                        .iter()
                        .map(|p| {
                            let mut edits = cur.clone();
                            edits.merge_from(&p.edits, p.groups);
                            ThumbRequest {
                                id: id.clone(),
                                edits,
                            }
                        })
                        .collect(),
                )),
                _ => None,
            };
        if let Some((lib, reqs)) = &previews {
            let missing: Vec<ThumbRequest> = reqs
                .iter()
                .filter(|r| lib.cache.get(&r.key()).is_none())
                .cloned()
                .collect();
            if !missing.is_empty() {
                lib.loader.also_want(&missing);
            }
        }
        for (i, p) in starters.iter().enumerate() {
            let has = current.as_ref().is_some_and(|e| p.matches(e));
            let tex = previews
                .as_ref()
                .and_then(|(lib, reqs)| lib.cache.get(&reqs[i].key()));
            if starter_row(ui, &p.name, has, tex) {
                action = Action::ApplyBuiltin(i);
            }
        }
        ui.add_space(4.0);
        ui.label(egui::RichText::new("Yours").small().weak());
        if n == 0 {
            ui.label(
                egui::RichText::new("None yet. Save current… keeps this photo's settings.").weak(),
            );
        }
        for (i, p) in app.preset_list.iter().enumerate() {
            let meta = rustroom_app::presets::groups_summary(p.groups);
            let has = current.as_ref().is_some_and(|e| p.matches(e));
            let out = mask_components::row(
                ui,
                mask_components::RowSpec {
                    glyph: mask_components::RowGlyph::Preset,
                    op: None,
                    name: &p.name,
                    meta: &meta,
                    accessible: &p.name,
                    selected: has,
                    current: false,
                    deletable: true,
                    indent: 0.0,
                    eye: None,
                    hold: None,
                    reorder: None,
                },
            );
            if out.delete {
                action = Action::Delete(p.name.clone());
            } else if out.clicked {
                action = Action::Apply(i);
            }
        }
        if let Some(name) = &undo_name {
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), ui.spacing().interact_size.y + 4.0),
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| {
                    if ui.small_button(format!("↩ Undo delete “{name}”")).clicked() {
                        action = Action::UndoDelete;
                    }
                },
            );
        }
    });

    match action {
        Action::None => {}
        Action::Save => {
            app.modal = Modal::SavePreset {
                name: String::new(),
                groups: EditGroups::default(),
            };
        }
        Action::ApplyBuiltin(i) => {
            if let Some(p) = rustroom_app::presets::builtin().get(i).cloned() {
                let (n, failed) = app.apply_to_selection(&p.edits, p.groups);
                app.status = match (n, failed) {
                    (0, _) => "Nothing selected".to_string(),
                    (1, 0) => format!("Applied “{}”", p.name),
                    (n, 0) => format!("Applied “{}” to {n} photos", p.name),
                    (n, f) => format!("Applied “{}” to {n} photos, {f} failed", p.name),
                };
            }
        }
        Action::Apply(i) => {
            if let Some(p) = app.preset_list.get(i).cloned() {
                let (n, failed) = app.apply_to_selection(&p.edits, p.groups);
                app.status = match (n, failed) {
                    (0, _) => "Nothing selected".to_string(),
                    (1, 0) => format!("Applied preset “{}”", p.name),
                    (n, 0) => format!("Applied “{}” to {n} photos", p.name),
                    (n, f) => format!("Applied “{}” to {n} photos, {f} failed", p.name),
                };
            }
        }
        Action::UndoDelete => app.undo_action(ctx),
        Action::Delete(name) => {
            if app.settings.confirm_preset_delete {
                app.modal = Modal::ConfirmDeletePreset {
                    name,
                    dont_ask_again: false,
                };
            } else {
                app.delete_preset(&name);
            }
        }
    }
}

/// One starter: a small preview of the open photo through it, then its name. The whole row
/// is the button. Without a preview yet, a quiet box of the same size holds the place.
fn starter_row(
    ui: &mut egui::Ui,
    name: &str,
    has: bool,
    tex: Option<&egui::TextureHandle>,
) -> bool {
    const BOX: egui::Vec2 = egui::vec2(56.0, 38.0);
    let mut clicked = false;
    ui.horizontal(|ui| {
        let (rect, r) = ui.allocate_exact_size(BOX, egui::Sense::click());
        clicked |= r.clicked();
        match tex {
            Some(t) => {
                let s = t.size_vec2();
                let fit = (BOX.x / s.x).min(BOX.y / s.y);
                let shown = egui::Rect::from_center_size(rect.center(), s * fit);
                egui::Image::new(t).paint_at(ui, shown);
            }
            None => {
                ui.painter()
                    .rect_filled(rect, 3.0, ui.visuals().extreme_bg_color);
            }
        }
        let label = ui.selectable_label(has, name);
        clicked |= label.clicked();
    });
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_ui;

    fn app_with(presets: &[&str]) -> RustroomApp {
        let mut app = crate::test_app::for_test();
        app.preset_list = presets
            .iter()
            .map(|n| rustroom_app::presets::Preset {
                name: n.to_string(),
                groups: EditGroups::default(),
                edits: edit_model::Edits::default(),
            })
            .collect();
        app
    }

    fn click_in_panel(label: &str, app: &mut RustroomApp) {
        let ctx = test_ui::ctx();
        let c = ctx.clone();
        test_ui::click(&ctx, label, |ui| show(app, ui, &c));
    }

    /// The starters are on the panel before the user has saved anything, and clicking one goes
    /// through the same apply path as a saved preset.
    #[test]
    fn the_starter_presets_are_listed_and_clickable() {
        let mut app = app_with(&[]);
        let ctx = test_ui::ctx();
        let c = ctx.clone();
        let found = test_ui::labelled_centres(&ctx, Vec::new(), |ui| show(&mut app, ui, &c));
        for name in ["Natural", "Punchy", "Black & white"] {
            assert!(
                found.iter().any(|(l, _)| l == name),
                "{name} is not on the panel: {:?}",
                found.iter().map(|(l, _)| l).collect::<Vec<_>>()
            );
        }
        click_in_panel("Punchy", &mut app);
        assert_eq!(app.status, "Nothing selected");
    }

    #[test]
    fn saving_opens_the_naming_dialog_rather_than_writing_straight_away() {
        // ⚠ A preset needs a name and a group choice before it can be written, so the button
        // opens a modal. One that saved immediately would produce unnamed presets carrying
        // every group — the two decisions the dialog exists to ask.
        let mut app = app_with(&[]);
        assert!(matches!(app.modal, Modal::None));
        click_in_panel("Save current as preset…", &mut app);
        assert!(
            matches!(app.modal, Modal::SavePreset { .. }),
            "expected the save dialog, got {:?}",
            std::mem::discriminant(&app.modal)
        );
    }

    #[test]
    fn deleting_asks_first_while_the_guard_is_armed() {
        // ⚠⚠ Deleting a preset is the *recoverable* one — it goes to a trash and Ctrl+Z restores
        // it — and it still confirms while the setting says to. The guard and the deletion are
        // separate flags on purpose: disarming the confirm on this one must not disarm it on the
        // irrecoverable export-preset deletion.
        let mut app = app_with(&["Warm"]);
        app.settings.confirm_preset_delete = true;
        click_in_panel("Delete Warm", &mut app);
        assert!(
            matches!(app.modal, Modal::ConfirmDeletePreset { .. }),
            "expected the confirm dialog"
        );
        assert_eq!(app.preset_list.len(), 1, "and nothing deleted yet");
    }

    #[test]
    fn deleting_with_the_guard_disarmed_does_not_ask() {
        let mut app = app_with(&["Warm"]);
        app.settings.confirm_preset_delete = false;
        click_in_panel("Delete Warm", &mut app);
        assert!(
            matches!(app.modal, Modal::None),
            "no dialog when the guard is off"
        );
    }

    #[test]
    fn applying_a_preset_with_nothing_selected_says_so() {
        // ⚠ A preset applies to the *selection*, exactly as a paste does. With nothing selected
        // the honest outcome is a status line, not a silent no-op.
        let mut app = app_with(&["Warm"]);
        click_in_panel("Warm", &mut app);
        assert_eq!(app.status, "Nothing selected");
    }

    #[test]
    fn an_empty_list_offers_nothing_to_delete_or_apply() {
        // ⚠ Asserted on the *absence* of the row buttons, not on the "No presets yet." label:
        // plain `ui.label` text produces no AccessKit node, so `test_ui` cannot see it. Only
        // interactive widgets are findable — the module's stated limit, and the point at which
        // `egui_kittest` would be the thing to adopt.
        let ctx = test_ui::ctx();
        let mut app = app_with(&[]);
        let c = ctx.clone();
        let labels: Vec<String> =
            test_ui::labelled_centres(&ctx, Vec::new(), |ui| show(&mut app, ui, &c))
                .into_iter()
                .map(|(l, _)| l)
                .collect();
        assert!(
            !labels.iter().any(|l| l.starts_with("Delete ")),
            "an empty list must offer nothing to delete, got {labels:?}"
        );
        assert!(
            labels.iter().any(|l| l == "Save current as preset…"),
            "but saving must still be offered, got {labels:?}"
        );
    }

    #[test]
    fn the_undo_delete_button_appears_only_when_that_is_what_undo_would_do() {
        // ⚠ It is the *same* action the toolbar's Undo performs, surfaced so the restore is
        // discoverable rather than only a keystroke — not a second, competing one. Showing it
        // when undo would do something else would make it lie about what it does.
        let ctx = test_ui::ctx();
        let shown = |app: &mut RustroomApp| {
            let c = ctx.clone();
            test_ui::labelled_centres(&ctx, Vec::new(), |ui| show(app, ui, &c))
                .into_iter()
                .any(|(l, _)| l.starts_with("↩ Undo delete"))
        };

        let mut app = app_with(&["Warm"]);
        assert!(!shown(&mut app), "nothing deleted yet");

        app.actions.push(
            ActionKind::PresetDelete {
                name: "Warm".to_string(),
            },
            None,
        );
        assert!(
            shown(&mut app),
            "a preset deletion should surface the restore"
        );
    }

    #[test]
    fn a_preset_the_photo_already_has_lights_its_row() {
        // ⚠ Both rows, so a `matches` wired to "always" or "never" fails: the preset equal to
        // the open photo's edits reports selected, the other does not.
        let ctx = test_ui::ctx();
        let mut app = app_with(&["Same", "Other"]);
        crate::test_app::open_test_photo(&mut app, &ctx, 40, 20);
        app.session.as_mut().unwrap().edits.exposure = 1.0;
        app.preset_list[0].edits.exposure = 1.0;
        app.preset_list[1].edits.exposure = 2.0;
        let c = ctx.clone();
        let out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(test_ui::screen()),
                ..Default::default()
            },
            |ui| show(&mut app, ui, &c),
        );
        let update = out
            .platform_output
            .accesskit_update
            .expect("accesskit is on");
        let selected = |name: &str| -> Option<bool> {
            update
                .nodes
                .iter()
                .find(|(_, n)| n.label() == Some(name))
                // ⚠ egui reports a `WidgetInfo::selected` state as the node's *toggled* value
                // (`response.rs`), not its selected flag.
                .map(|(_, n)| n.toggled() == Some(egui::accesskit::Toggled::True))
        };
        assert_eq!(
            selected("Same"),
            Some(true),
            "the matching preset is not lit"
        );
        assert_eq!(selected("Other"), Some(false), "a differing preset is lit");
    }
}
