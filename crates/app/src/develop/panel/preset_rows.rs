//! The Presets tool's *Yours* (M10.9a): the presets with no folder, then each folder — a collapsible
//! header with its count — and its presets below. A row applies on a click; a double-click or the
//! right-click's Rename renames it in place; it drags onto a folder (or onto *Yours*) to move.
//!
//! ⚠ Draws from plain data and returns one [`Act`] — the caller performs it — so no `RustroomApp`
//! borrow is held across the drawing (`develop::panel::presets` applies it).

use crate::develop::panel::{mask_components, section};
use crate::preset_ops::PresetRename;
use crate::theme::Hover as _;
use edit_model::Edits;
use eframe::egui;
use rustroom_app::presets::Preset;

/// How far a folder's presets sit in from its header.
const INDENT: f32 = 14.0;

/// What a click in *Yours* asked for. A preset is named by its folder and its name.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Act {
    Apply(Option<String>, String),
    Delete(Option<String>, String),
    Rename(Option<String>, String, String),
    Update(Option<String>, String),
    Move(Option<String>, String, Option<String>),
    MoveToNewFolder(Option<String>, String),
    Export(Option<String>, String),
    Reveal(Option<String>, String),
    RenameFolder(String, String),
    ImportInto(String),
    ExportFolder(String),
    DeleteFolder(String),
}

/// A preset being dragged: where it is and what it is called.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Dragged {
    pub folder: Option<String>,
    pub name: String,
}

/// What the rows need to know about the app.
pub(crate) struct Ctx<'a> {
    pub presets: &'a [Preset],
    pub folders: &'a [String],
    /// The open photo's edits, which light the rows it already has — `None` with no photo open.
    pub current: Option<&'a Edits>,
}

/// Draw *Yours*. `rename` is the in-place rename, owned by the app across frames.
pub(crate) fn yours(
    ui: &mut egui::Ui,
    c: &Ctx<'_>,
    rename: &mut Option<PresetRename>,
) -> Option<Act> {
    let mut act = None;
    let dragging = egui::DragAndDrop::payload::<Dragged>(ui.ctx()).map(|d| (*d).clone());

    // The caption is the drop target for "no folder" — lit while a preset from a folder hovers it.
    let caption = ui.label(egui::RichText::new("Yours").small().weak());
    if let Some(d) = caption.dnd_release_payload::<Dragged>() {
        act = Some(Act::Move(d.folder.clone(), d.name.clone(), None));
    }
    if dragging.as_ref().is_some_and(|d| d.folder.is_some()) {
        drop_hint(ui, &caption);
    }
    if c.presets.is_empty() && c.folders.is_empty() {
        ui.label(
            egui::RichText::new("None yet. Save current… keeps this photo's settings.").weak(),
        );
    }
    for p in c.presets.iter().filter(|p| p.folder.is_none()) {
        if let Some(a) = preset_row(ui, c, p, 0.0, rename) {
            act = Some(a);
        }
    }
    for f in c.folders {
        if let Some(a) = folder(ui, c, f, rename, dragging.as_ref()) {
            act = Some(a);
        }
    }
    act
}

/// One folder: its header (or the rename field), then its presets when open.
fn folder(
    ui: &mut egui::Ui,
    c: &Ctx<'_>,
    name: &str,
    rename: &mut Option<PresetRename>,
    dragging: Option<&Dragged>,
) -> Option<Act> {
    let mut act = None;
    if let Some(PresetRename::Folder { name: n, text }) = rename.as_mut() {
        if n == name {
            let n = n.clone();
            match field(ui, text, 0.0) {
                Field::Commit(t) => {
                    *rename = None;
                    return Some(Act::RenameFolder(n, t));
                }
                Field::Cancel => *rename = None,
                Field::Typing => return None,
            }
        }
    }
    let inside: Vec<&Preset> = c
        .presets
        .iter()
        .filter(|p| p.folder.as_deref() == Some(name))
        .collect();
    let count = inside.len().to_string();
    let mut h = section::header(
        ui,
        ui.make_persistent_id(("preset_folder", name)),
        true,
        section::Header::new(name).trailing(&count).verbatim(true),
    );
    let r = h.response.clone();
    if let Some(d) = r.dnd_release_payload::<Dragged>() {
        act = Some(Act::Move(
            d.folder.clone(),
            d.name.clone(),
            Some(name.into()),
        ));
    }
    if dragging.is_some_and(|d| d.folder.as_deref() != Some(name)) {
        drop_hint(ui, &r);
    }
    if r.double_clicked() {
        *rename = Some(PresetRename::Folder {
            name: name.into(),
            text: name.into(),
        });
    }
    r.context_menu(|ui| {
        if ui.button("Rename").clicked() {
            *rename = Some(PresetRename::Folder {
                name: name.into(),
                text: name.into(),
            });
            ui.close();
        }
        if ui.button("Import presets here…").clicked() {
            act = Some(Act::ImportInto(name.into()));
            ui.close();
        }
        if ui.button("Export folder…").clicked() {
            act = Some(Act::ExportFolder(name.into()));
            ui.close();
        }
        ui.separator();
        if ui.button("Delete folder…").clicked() {
            act = Some(Act::DeleteFolder(name.into()));
            ui.close();
        }
    });
    h.state.show_body_unindented(ui, |ui| {
        if inside.is_empty() {
            ui.label(
                egui::RichText::new("Empty — drag a preset here")
                    .small()
                    .weak(),
            );
        }
        for p in inside {
            if let Some(a) = preset_row(ui, c, p, INDENT, rename) {
                act = Some(a);
            }
        }
    });
    act
}

/// One preset: the row (or the rename field), its right-click menu, its drag.
fn preset_row(
    ui: &mut egui::Ui,
    c: &Ctx<'_>,
    p: &Preset,
    indent: f32,
    rename: &mut Option<PresetRename>,
) -> Option<Act> {
    let folder = p.folder.clone();
    if let Some(PresetRename::Preset {
        folder: f,
        name: n,
        text,
    }) = rename.as_mut()
    {
        if *f == folder && *n == p.name {
            match field(ui, text, indent) {
                Field::Commit(t) => {
                    *rename = None;
                    return Some(Act::Rename(folder, p.name.clone(), t));
                }
                Field::Cancel => *rename = None,
                Field::Typing => return None,
            }
        }
    }
    let meta = rustroom_app::presets::groups_summary(p.groups);
    let has = c.current.is_some_and(|e| p.matches(e));
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
            delete_hover: "Deletes this preset",
            indent,
            eye: None,
            hold: None,
            hover: "Applies this preset to the photo",
            draggable: true,
        },
    );
    let r = &out.response;
    r.dnd_set_drag_payload(Dragged {
        folder: folder.clone(),
        name: p.name.clone(),
    });
    let mut act = None;
    if out.delete {
        act = Some(Act::Delete(folder.clone(), p.name.clone()));
    } else if r.double_clicked() {
        *rename = Some(PresetRename::Preset {
            folder: folder.clone(),
            name: p.name.clone(),
            text: p.name.clone(),
        });
    } else if out.clicked {
        act = Some(Act::Apply(folder.clone(), p.name.clone()));
    }
    r.context_menu(|ui| {
        let me = |a: fn(Option<String>, String) -> Act| a(folder.clone(), p.name.clone());
        if ui.button("Rename").clicked() {
            *rename = Some(PresetRename::Preset {
                folder: folder.clone(),
                name: p.name.clone(),
                text: p.name.clone(),
            });
            ui.close();
        }
        if ui
            .add_enabled(
                c.current.is_some(),
                egui::Button::new("Update with current settings"),
            )
            .clicked()
        {
            act = Some(me(Act::Update));
            ui.close();
        }
        ui.menu_button("Move to", |ui| {
            if folder.is_some() && ui.button("No folder").clicked() {
                act = Some(Act::Move(folder.clone(), p.name.clone(), None));
                ui.close();
            }
            for f in c.folders.iter().filter(|f| Some(*f) != folder.as_ref()) {
                if ui.button(f).clicked() {
                    act = Some(Act::Move(folder.clone(), p.name.clone(), Some(f.clone())));
                    ui.close();
                }
            }
            ui.separator();
            if ui.button("New folder…").clicked() {
                act = Some(me(Act::MoveToNewFolder));
                ui.close();
            }
        });
        if ui.button("Export…").clicked() {
            act = Some(me(Act::Export));
            ui.close();
        }
        if ui.button("Show in Finder").clicked() {
            act = Some(me(Act::Reveal));
            ui.close();
        }
        ui.separator();
        if ui.button("Delete").clicked() {
            act = Some(me(Act::Delete));
            ui.close();
        }
    });
    act
}

enum Field {
    Typing,
    Commit(String),
    Cancel,
}

/// The in-place rename field — the keywords' shape: Enter or a click away commits, Escape cancels.
fn field(ui: &mut egui::Ui, text: &mut String, indent: f32) -> Field {
    let resp = ui
        .horizontal(|ui| {
            ui.add_space(indent);
            ui.add(egui::TextEdit::singleline(text).desired_width(f32::INFINITY))
                .hover("Type the new name and press Return")
        })
        .inner;
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        Field::Cancel
    } else if crate::name_edit::commit_gesture(ui, &resp, true) {
        Field::Commit(text.clone())
    } else {
        Field::Typing
    }
}

/// A drop target lit while something droppable hovers it.
fn drop_hint(ui: &egui::Ui, r: &egui::Response) {
    if r.contains_pointer() {
        ui.painter().rect_stroke(
            r.rect,
            4.0,
            egui::Stroke::new(2.0, ui.visuals().selection.bg_fill),
            egui::StrokeKind::Inside,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_ui;
    use edit_model::EditGroups;

    fn preset(folder: Option<&str>, name: &str) -> Preset {
        Preset {
            name: name.to_string(),
            groups: EditGroups::default(),
            edits: Edits::default(),
            folder: folder.map(str::to_string),
        }
    }

    /// `Mine` with no folder; `Film` holding `Look`; `Empty` holding nothing.
    fn data() -> (Vec<Preset>, Vec<String>) {
        (
            vec![preset(None, "Mine"), preset(Some("Film"), "Look")],
            vec!["Empty".to_string(), "Film".to_string()],
        )
    }

    /// Frames of `yours`, collecting what it asked for.
    struct Harness {
        ctx: egui::Context,
        rename: Option<PresetRename>,
        acts: Vec<Act>,
        t: f64,
    }

    impl Harness {
        fn new() -> Self {
            Self {
                ctx: test_ui::ctx(),
                rename: None,
                acts: Vec::new(),
                t: 10.0,
            }
        }

        /// One frame; returns every AccessKit label drawn.
        fn frame(&mut self, events: Vec<egui::Event>) -> Vec<String> {
            let (presets, folders) = data();
            let c = Ctx {
                presets: &presets,
                folders: &folders,
                current: None,
            };
            self.t += 0.05;
            let (rename, acts) = (&mut self.rename, &mut self.acts);
            let out = self.ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(test_ui::screen()),
                    time: Some(self.t),
                    events,
                    ..Default::default()
                },
                |ui| {
                    if let Some(a) = yours(ui, &c, rename) {
                        acts.push(a);
                    }
                },
            );
            out.platform_output
                .accesskit_update
                .map(|u| {
                    u.nodes
                        .iter()
                        .filter_map(|(_, n)| n.label().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default()
        }

        fn at(&mut self, label: &str) -> egui::Pos2 {
            let (presets, folders) = data();
            let c = Ctx {
                presets: &presets,
                folders: &folders,
                current: None,
            };
            let mut rename = self.rename.clone();
            test_ui::find(&self.ctx, label, |ui| {
                let _ = yours(ui, &c, &mut rename);
            })
            .unwrap_or_else(|| panic!("no {label}"))
        }

        fn press(&mut self, p: egui::Pos2, button: egui::PointerButton, pressed: bool) {
            self.frame(vec![egui::Event::PointerButton {
                pos: p,
                button,
                pressed,
                modifiers: Default::default(),
            }]);
        }

        fn click(&mut self, p: egui::Pos2, button: egui::PointerButton) {
            self.frame(vec![egui::Event::PointerMoved(p)]);
            self.press(p, button, true);
            self.press(p, button, false);
        }

        /// The labels drawn once a right-click on `label` has opened its menu.
        fn menu_of(&mut self, label: &str) -> Vec<String> {
            let p = self.at(label);
            self.click(p, egui::PointerButton::Secondary);
            self.frame(Vec::new())
        }

        fn drag(&mut self, from: egui::Pos2, to: egui::Pos2) {
            self.frame(vec![egui::Event::PointerMoved(from)]);
            self.press(from, egui::PointerButton::Primary, true);
            for k in 1..=6 {
                let p = from + (to - from) * (k as f32 / 6.0);
                self.frame(vec![egui::Event::PointerMoved(p)]);
            }
            self.press(to, egui::PointerButton::Primary, false);
            self.frame(Vec::new());
        }
    }

    fn has(labels: &[String], want: &[&str]) {
        for w in want {
            assert!(labels.iter().any(|l| l == w), "no {w:?} in {labels:?}");
        }
    }

    #[test]
    fn folders_and_their_presets_are_drawn() {
        let mut h = Harness::new();
        let drawn = h.frame(Vec::new());
        has(&drawn, &["Mine", "Film", "Look", "Empty"]);
    }

    #[test]
    fn a_click_applies() {
        let mut h = Harness::new();
        let p = h.at("Look");
        h.click(p, egui::PointerButton::Primary);
        assert_eq!(h.acts, [Act::Apply(Some("Film".into()), "Look".into())]);
    }

    /// The author's full set on a preset, and a folder's own four.
    #[test]
    fn the_right_click_menus_offer_their_entries() {
        let mut h = Harness::new();
        let menu = h.menu_of("Look");
        has(
            &menu,
            &[
                "Rename",
                "Update with current settings",
                // egui marks a submenu with its own arrow.
                "Move to ⏵",
                "Export…",
                "Show in Finder",
                "Delete",
            ],
        );
        let mut h = Harness::new();
        let menu = h.menu_of("Film");
        has(
            &menu,
            &[
                "Rename",
                "Import presets here…",
                "Export folder…",
                "Delete folder…",
            ],
        );
    }

    /// A double-click opens the field; Enter renames; Escape would have cancelled.
    #[test]
    fn a_double_click_renames_in_place() {
        let mut h = Harness::new();
        let p = h.at("Mine");
        // ⚠ Through the harness's own frames, 50 ms apart, so `yours` sees both clicks.
        h.frame(vec![egui::Event::PointerMoved(p)]);
        for pressed in [true, false, true, false] {
            h.press(p, egui::PointerButton::Primary, pressed);
        }
        assert!(
            matches!(&h.rename, Some(PresetRename::Preset { name, .. }) if name == "Mine"),
            "the field opens: {:?}",
            h.rename
        );
        h.rename = Some(PresetRename::Preset {
            folder: None,
            name: "Mine".into(),
            text: "Ours".into(),
        });
        h.frame(Vec::new());
        h.frame(vec![egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        }]);
        assert!(
            h.acts
                .contains(&Act::Rename(None, "Mine".into(), "Ours".into())),
            "{:?}",
            h.acts
        );
        assert!(h.rename.is_none(), "the field closes");
    }

    /// ⚠⚠ A preset dragged onto a folder's header moves there; one dragged onto *Yours* leaves its
    /// folder.
    #[test]
    fn a_drag_moves_a_preset_between_folders() {
        let mut h = Harness::new();
        let (from, to) = (h.at("Mine"), h.at("Film"));
        h.drag(from, to);
        assert!(
            h.acts
                .contains(&Act::Move(None, "Mine".into(), Some("Film".into()))),
            "{:?}",
            h.acts
        );
        let mut h = Harness::new();
        let (from, to) = (h.at("Look"), h.at("Yours"));
        h.drag(from, to);
        assert!(
            h.acts
                .contains(&Act::Move(Some("Film".into()), "Look".into(), None)),
            "{:?}",
            h.acts
        );
    }
}
