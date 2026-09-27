//! The Keywords section's rows: open, delete, tag and untag as before, and — M9.8 — rename in place
//! and drag to move.
//!
//! ⚠ Its own module: `collections_panel.rs` was already ~860 lines of code, and this is where all
//! the new behaviour of M9.8's panel half lives. The rules (valid names, allowed drops) are in the
//! lib's `keyword_tool`; this is the gesture.

use crate::list_row::{self, RowAction, RowSpec};
use catalog::db::KeywordRow;
use eframe::egui;
use rustroom_app::keyword_tool;

/// What a row asked for, applied after the panel closes.
pub(crate) enum Out {
    Open(i64, String),
    Delete(i64, String),
    Tag(i64),
    Untag(i64),
    Rename(i64, String),
    Move(i64, Option<i64>),
}

/// The drag payload: a keyword id. ⚠ Its own type, so no other drag in the app — a panel reorder,
/// say — can be mistaken for a keyword being dropped.
#[derive(Clone, Copy)]
struct Dragged(i64);

/// The indent per tree level, in points.
const INDENT: f32 = 12.0;

pub(crate) fn rows(
    ui: &mut egui::Ui,
    kws: &[KeywordRow],
    picked: usize,
    selected: Option<i64>,
    rename: &mut Option<(i64, String)>,
) -> Option<Out> {
    let mut out = None;
    let dragging = egui::DragAndDrop::payload::<Dragged>(ui.ctx()).map(|d| d.0);

    let tag_hover = format!("Tag {picked} selected");
    let untag_hover = format!("Remove from {picked} selected");
    for k in kws {
        // While this row owns the draft, it is an editable field — the snapshot strip's shape.
        if let Some((id, text)) = rename.as_mut().filter(|(id, _)| *id == k.id) {
            let id = *id;
            let resp = ui
                .horizontal(|ui| {
                    ui.add_space(k.depth as f32 * INDENT);
                    ui.add(egui::TextEdit::singleline(text).desired_width(f32::INFINITY))
                })
                .inner;
            if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                *rename = None;
            } else if crate::name_edit::commit_gesture(ui, &resp, true) {
                out = Some(Out::Rename(id, text.clone()));
                *rename = None;
            }
            continue;
        }
        // ⚠ Indented by depth — the only thing that makes the tree readable, since the rows are a
        // flat depth-first list.
        //
        // ⚠ `−` completes the existing mechanism rather than adding a second one:
        // `unassign_keyword` already existed and was tested. Both go through one pair of functions
        // the photo context menu also calls. (Moved here from `collections_panel` with the rows.)
        let r = list_row::row_draggable(
            ui,
            RowSpec {
                name: &k.name,
                count: Some(k.count),
                meta: "",
                selected: selected == Some(k.id),
                indent: k.depth as f32 * INDENT,
                hover: "Double-click to rename; drag onto another keyword to move it there",
                actions: &[
                    RowAction {
                        glyph: '×',
                        verb: "Delete",
                        hover: "Delete, with everything under it",
                        enabled: true,
                    },
                    RowAction {
                        glyph: '+',
                        verb: "Tag",
                        hover: &tag_hover,
                        enabled: picked > 0,
                    },
                    RowAction {
                        glyph: '−',
                        verb: "Untag",
                        hover: &untag_hover,
                        enabled: picked > 0,
                    },
                ],
            },
        );
        // ⚠ The row senses the drag itself — see `list_row::row_draggable` for the layer that
        // swallowed its clicks.
        let drag = &r.response;
        if drag.drag_started() {
            drag.dnd_set_drag_payload(Dragged(k.id));
        }
        if let Some(p) = drag.dnd_hover_payload::<Dragged>() {
            if p.0 != k.id {
                let ok = keyword_tool::can_drop(kws, p.0, Some(k.id));
                let v = ui.visuals();
                let colour = if ok {
                    v.selection.bg_fill
                } else {
                    v.error_fg_color
                };
                ui.painter().rect_stroke(
                    r.rect,
                    3.0,
                    egui::Stroke::new(1.5, colour),
                    egui::StrokeKind::Inside,
                );
            }
        }
        if let Some(p) = drag.dnd_release_payload::<Dragged>() {
            if keyword_tool::can_drop(kws, p.0, Some(k.id)) {
                out = Some(Out::Move(p.0, Some(k.id)));
            }
        }
        match r.action {
            Some(0) => out = Some(Out::Delete(k.id, k.name.clone())),
            Some(1) => out = Some(Out::Tag(k.id)),
            Some(_) => out = Some(Out::Untag(k.id)),
            None if r.double_clicked => *rename = Some((k.id, k.name.clone())),
            None if r.clicked => out = Some(Out::Open(k.id, k.name.clone())),
            None => {}
        }
    }
    // ⚠ The "Top level" target exists only while a keyword is being dragged — a drop row that is
    // always there would be a row that does nothing. ⚠⚠ And it is drawn BELOW the rows: drawn above
    // them it appeared the moment a drag began and pushed every row down one line, so the row under
    // the pointer changed mid-drag — found by `dragging_a_keyword_onto_another_moves_it_there`.
    if let Some(d) = dragging {
        let ok = keyword_tool::can_drop(kws, d, None);
        let (_, dropped) = ui.dnd_drop_zone::<Dragged, _>(egui::Frame::NONE, |ui| {
            let text = egui::RichText::new("⤒ Top level").weak();
            ui.add_enabled(ok, egui::Label::new(text));
        });
        if let Some(p) = dropped {
            if keyword_tool::can_drop(kws, p.0, None) {
                out = Some(Out::Move(p.0, None));
            }
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_ui;

    fn row(id: i64, name: &str, parent: Option<i64>, depth: usize) -> KeywordRow {
        KeywordRow {
            id,
            name: name.to_string(),
            parent_id: parent,
            depth,
            count: 0,
        }
    }

    /// Europe(1) > Pariss(2); Asia(3).
    fn tree() -> Vec<KeywordRow> {
        vec![
            row(1, "Europe", None, 0),
            row(2, "Pariss", Some(1), 1),
            row(3, "Asia", None, 0),
        ]
    }

    /// Runs frames of `events`, collecting what the rows asked for.
    struct Harness {
        ctx: egui::Context,
        rename: Option<(i64, String)>,
        outs: Vec<String>,
    }

    impl Harness {
        fn new() -> Self {
            Self {
                ctx: test_ui::ctx(),
                rename: None,
                outs: Vec::new(),
            }
        }

        fn frame(&mut self, events: Vec<egui::Event>, t: f64) {
            let kws = tree();
            let (rename, outs) = (&mut self.rename, &mut self.outs);
            let _ = self.ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(test_ui::screen()),
                    time: Some(t),
                    events,
                    ..Default::default()
                },
                |ui| {
                    if let Some(o) = rows(ui, &kws, 0, None, rename) {
                        outs.push(match o {
                            Out::Rename(id, s) => format!("rename {id} {s}"),
                            Out::Move(id, p) => format!("move {id} {p:?}"),
                            Out::Open(id, _) => format!("open {id}"),
                            _ => "other".to_string(),
                        });
                    }
                },
            );
        }

        fn at(&mut self, label: &str) -> egui::Pos2 {
            let kws = tree();
            let mut rename = self.rename.clone();
            test_ui::find(&self.ctx, label, |ui| {
                let _ = rows(ui, &kws, 0, None, &mut rename);
            })
            .unwrap_or_else(|| panic!("no row {label}"))
        }
    }

    fn button(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }
    }

    fn key(k: egui::Key) -> egui::Event {
        egui::Event::Key {
            key: k,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        }
    }

    fn double_click(h: &mut Harness, p: egui::Pos2) {
        let t = 10.0;
        h.frame(vec![egui::Event::PointerMoved(p)], t);
        h.frame(vec![button(p, true)], t + 0.02);
        h.frame(vec![button(p, false)], t + 0.04);
        h.frame(vec![button(p, true)], t + 0.06);
        h.frame(vec![button(p, false)], t + 0.08);
        h.frame(vec![], t + 0.1);
    }

    #[test]
    fn a_double_click_opens_the_field_and_enter_renames() {
        let mut h = Harness::new();
        let p = h.at("Pariss");
        double_click(&mut h, p);
        assert_eq!(
            h.rename.as_ref().map(|r| r.0),
            Some(2),
            "the field must open on row 2"
        );
        // Replace the draft and commit with Enter.
        h.rename = Some((2, "Paris".to_string()));
        h.frame(vec![], 11.0);
        h.frame(vec![key(egui::Key::Enter)], 11.1);
        h.frame(vec![], 11.2);
        assert!(
            h.outs.contains(&"rename 2 Paris".to_string()),
            "{:?}",
            h.outs
        );
        assert!(h.rename.is_none(), "the field closes");
    }

    #[test]
    fn escape_cancels_a_rename() {
        let mut h = Harness::new();
        h.rename = Some((2, "Paris".to_string()));
        h.frame(vec![], 1.0);
        h.frame(vec![key(egui::Key::Escape)], 1.1);
        assert!(h.rename.is_none());
        assert!(
            !h.outs.iter().any(|o| o.starts_with("rename")),
            "{:?}",
            h.outs
        );
    }

    /// Drag Pariss onto Asia: a move under Asia. ⚠ Moved in steps, past egui's drag threshold, or
    /// the gesture is a click and moves nothing — the harness trap `drag_through` records.
    fn drag(h: &mut Harness, from: egui::Pos2, to: egui::Pos2) {
        let mut t = 20.0;
        h.frame(vec![egui::Event::PointerMoved(from)], t);
        t += 0.02;
        h.frame(vec![button(from, true)], t);
        for k in 1..=6 {
            t += 0.02;
            let p = from + (to - from) * (k as f32 / 6.0);
            h.frame(vec![egui::Event::PointerMoved(p)], t);
        }
        t += 0.02;
        h.frame(vec![button(to, false)], t);
        h.frame(vec![], t + 0.02);
    }

    #[test]
    fn dragging_a_keyword_onto_another_moves_it_there() {
        let mut h = Harness::new();
        let (from, to) = (h.at("Pariss"), h.at("Asia"));
        drag(&mut h, from, to);
        assert!(
            h.outs.contains(&"move 2 Some(3)".to_string()),
            "{:?}",
            h.outs
        );
    }

    /// The "Top level" row, which appears below the list only while dragging: dropping Pariss there
    /// makes it a root. Its position is found DURING the drag, since it does not exist before.
    #[test]
    fn dropping_on_top_level_makes_a_root() {
        let mut h = Harness::new();
        let from = h.at("Pariss");
        let asia = h.at("Asia");
        let mut t = 30.0;
        h.frame(vec![egui::Event::PointerMoved(from)], t);
        t += 0.02;
        h.frame(vec![button(from, true)], t);
        for k in 1..=4 {
            t += 0.02;
            let p = from + (asia - from) * (k as f32 / 4.0);
            h.frame(vec![egui::Event::PointerMoved(p)], t);
        }
        // Mid-drag, ask where the drop row landed.
        let kws = tree();
        let mut rename = None;
        let top = test_ui::labelled_centres(&h.ctx, vec![egui::Event::PointerMoved(asia)], |ui| {
            let _ = rows(ui, &kws, 0, None, &mut rename);
        })
        .into_iter()
        .find(|(l, _)| l.contains("Top level"))
        .map(|(_, p)| p)
        .expect("the drop row must appear while dragging");
        for k in 1..=3 {
            t += 0.02;
            let p = asia + (top - asia) * (k as f32 / 3.0);
            h.frame(vec![egui::Event::PointerMoved(p)], t);
        }
        t += 0.02;
        h.frame(vec![button(top, false)], t);
        h.frame(vec![], t + 0.02);
        assert!(h.outs.contains(&"move 2 None".to_string()), "{:?}", h.outs);
    }

    /// ⚠ And a refused drop does nothing: Europe onto its own child.
    #[test]
    fn a_drop_onto_its_own_child_is_refused() {
        let mut h = Harness::new();
        let (from, to) = (h.at("Europe"), h.at("Pariss"));
        drag(&mut h, from, to);
        assert!(
            !h.outs.iter().any(|o| o.starts_with("move")),
            "{:?}",
            h.outs
        );
    }
}
