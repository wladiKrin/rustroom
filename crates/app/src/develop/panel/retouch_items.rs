//! Retouch's one list (M10.19d): every removal, person, patch and eye as a row — grouped and numbered by
//! `rustroom_app::retouch_list` — with each row's eye, Hold out and ×, Reset and Show all; a picked row selects that
//! retouch alone and shows its brush; under the list, the selected retouch's own settings.
//!
//! ⚠ Three vectors, three owners of a row's flags: a patch's in `Session::spot_shown` / `spot_held`, a removal's in
//! `RemovalUi`; an eye has neither (as before the merge).

use super::{mask_components, retouch_brush, retouch_eyes, section, Edited};
use crate::theme;
use crate::theme::Hover as _;
use crate::Session;
use edit_model::SpotMode;
use eframe::egui;
use rustroom_app::eye_tool::EyeTool;
use rustroom_app::mask_tool;
use rustroom_app::remove_tool::RemoveTool;
use rustroom_app::retouch_list::{self, Item, Kind};
use rustroom_app::retouch_task::{RetouchState, Task};
use rustroom_app::slider_help::{self, Scope};
use rustroom_app::spot_tool::{self, SpotTool, MAX_RADIUS, MIN_RADIUS};

/// What a row's click asked for.
enum Act {
    Pick(Item),
    Eye(Item),
    Hold(Item),
    Delete(Item),
}

/// The "Retouches" card and, under it, the selected retouch's settings.
pub(super) fn show(
    ui: &mut egui::Ui,
    s: &mut Session,
    tool: &mut SpotTool,
    eyes: &mut EyeTool,
    remove: &mut RemoveTool,
    state: &mut RetouchState,
    ed: &mut Edited,
) {
    let count = retouch_list::summary(&s.edits);
    let mut out = section::header(
        ui,
        ui.make_persistent_id(("retouch", "Retouches")),
        true,
        section::Header::new("Retouches")
            .modified(retouch_list::count(&s.edits) > 0)
            .resettable(true)
            .trailing(&count),
    );
    if out.reset {
        reset(s);
        ed.discrete();
    }
    // ⚠ Read AFTER a reset: rows from before it index into the emptied vectors.
    let rows = retouch_list::rows(&s.edits);
    let selected = selection(s);
    let filled: Vec<bool> = {
        let maps = s.engine.ai_maps();
        let got = maps.read().ok();
        s.edits
            .removals
            .iter()
            .map(|r| got.as_ref().is_some_and(|m| m.fill(r).is_some()))
            .collect()
    };
    let long = s.full_long;
    let mut act = None;
    let mut flip_all = false;
    out.show_body(ui, |ui| {
        for row in &rows {
            let (glyph, meta, eye, hold, delete_hover, hover) = match (row.kind, row.item) {
                (Kind::Removal | Kind::Person, Item::Removal(i)) => (
                    mask_components::RowGlyph::Spot(SpotMode::Heal),
                    if filled.get(i).copied().unwrap_or(false) {
                        String::new()
                    } else {
                        "filling".to_owned()
                    },
                    Some(s.removal.is_shown(i)),
                    Some((
                        s.removal.is_held(i),
                        "Hold out",
                        "Hides this removal in the view only, so you can see what it covers",
                    )),
                    "Deletes this removal",
                    "Selects this removal, to change its Opacity or fill it again",
                ),
                (_, Item::Spot(i)) => (
                    mask_components::RowGlyph::Spot(s.edits.spots[i].mode),
                    format!(
                        "{:.0} px",
                        spot_tool::radius_px(s.edits.spots[i].radius, long)
                    ),
                    Some(s.spot_shown.get(i).copied().unwrap_or(true)),
                    Some((
                        s.spot_held.get(i).copied().unwrap_or(false),
                        "Hold out",
                        "Hides this patch in the view only, so you can see what it covers",
                    )),
                    "Deletes this patch",
                    "Selects this patch, so its settings edit it",
                ),
                (_, Item::Eye(i)) => (
                    mask_components::RowGlyph::Eye(s.edits.eyes[i].kind),
                    format!(
                        "{:.0} px",
                        spot_tool::radius_px(s.edits.eyes[i].radius, long)
                    ),
                    None,
                    None,
                    "Deletes this correction",
                    "Selects this correction, so its sliders edit it",
                ),
                (_, Item::Removal(_)) => unreachable!("a removal row is a Removal or a Person"),
            };
            let r = mask_components::row(
                ui,
                mask_components::RowSpec {
                    glyph,
                    op: None,
                    name: &row.label,
                    meta: &meta,
                    accessible: &row.label,
                    selected: selected == Some(row.item),
                    current: false,
                    deletable: true,
                    delete_hover,
                    indent: 0.0,
                    eye,
                    hold,
                    hover,
                    draggable: false,
                },
            );
            if r.delete {
                act = Some(Act::Delete(row.item));
            } else if r.eye {
                act = Some(Act::Eye(row.item));
            } else if r.hold {
                act = Some(Act::Hold(row.item));
            } else if r.clicked {
                act = Some(Act::Pick(row.item));
            }
        }
        flip_all = show_all_row(ui, s);
    });
    if flip_all {
        let on = !all_shown(s);
        s.set_all_spots_shown(on);
        let k = s.edits.removals.len();
        s.removal.set_all_shown(on, k);
    }
    match act {
        Some(Act::Pick(item)) if selected == Some(item) => deselect(s),
        Some(Act::Pick(item)) => {
            let Some(kind) = retouch_list::kind_of(&s.edits, item) else {
                return;
            };
            state.task = Task::Paint;
            retouch_brush::set(s, tool, eyes, remove, retouch_list::brush_of(kind));
            if let Item::Eye(i) = item {
                eyes.active = Some(s.edits.eyes[i].kind);
            }
            select(s, item);
        }
        Some(Act::Eye(Item::Spot(i))) => {
            if let Some(on) = s.spot_shown.get_mut(i) {
                *on = !*on;
            }
        }
        Some(Act::Eye(Item::Removal(i))) => s.removal.toggle_shown(i),
        // ⚠ A VIEW MODE, not an edit: it changes `view_edits`, so it re-renders, and pushes no history entry.
        Some(Act::Hold(Item::Spot(i))) => {
            if let Some(on) = s.spot_held.get_mut(i) {
                *on = !*on;
            }
            ed.changed = true;
        }
        Some(Act::Hold(Item::Removal(i))) => {
            s.removal.toggle_held(i);
            ed.changed = true;
        }
        Some(Act::Delete(item)) => {
            delete(s, item);
            ed.discrete();
        }
        Some(Act::Eye(Item::Eye(_))) | Some(Act::Hold(Item::Eye(_))) | None => {}
    }
    if let Some(item) = selection(s) {
        let label = retouch_list::rows(&s.edits)
            .into_iter()
            .find(|r| r.item == item)
            .map(|r| r.label)
            .unwrap_or_default();
        theme::sub_label(ui, &label);
        match item {
            Item::Removal(i) => removal_settings(ui, s, i, ed),
            Item::Spot(i) => patch_settings(ui, s, i, ed),
            Item::Eye(i) => retouch_eyes::selected_settings(ui, s, i, eyes, ed),
        }
    }
}

/// The selected retouch, clamped to its vector — at most one of the three is set.
fn selection(s: &Session) -> Option<Item> {
    retouch_list::selected(
        s.selected_spot.filter(|&i| i < s.edits.spots.len()),
        s.removal.selected.filter(|&i| i < s.edits.removals.len()),
        s.selected_eye.filter(|&i| i < s.edits.eyes.len()),
    )
}

fn deselect(s: &mut Session) {
    s.selected_spot = None;
    s.removal.selected = None;
    s.selected_eye = None;
}

fn select(s: &mut Session, item: Item) {
    deselect(s);
    match item {
        Item::Spot(i) => s.selected_spot = Some(i),
        Item::Removal(i) => s.removal.selected = Some(i),
        Item::Eye(i) => s.selected_eye = Some(i),
    }
}

/// Every retouch gone, every selection and flag with it.
fn reset(s: &mut Session) {
    s.edits.spots.clear();
    s.edits.removals.clear();
    s.edits.eyes.clear();
    deselect(s);
    s.spot_shown.clear();
    s.spot_held.clear();
    s.suppressed_spot = None;
    s.suppressed_eye = None;
    s.removal.sync(0);
}

/// One retouch deleted: its flags go with it, a later selection follows it down.
fn delete(s: &mut Session, item: Item) {
    match item {
        Item::Spot(i) => {
            let before = s.edits.spots.len();
            s.edits.spots.remove(i);
            if i < s.spot_shown.len() {
                s.spot_shown.remove(i);
            }
            if i < s.spot_held.len() {
                s.spot_held.remove(i);
            }
            s.selected_spot = mask_tool::selection_after_delete(s.selected_spot, i, before);
            if s.suppressed_spot == Some(i) {
                s.suppressed_spot = None;
            }
        }
        Item::Removal(i) => {
            s.edits.removals.remove(i);
            s.removal.forget(i);
            s.removal.selected = match s.removal.selected {
                Some(j) if j == i => None,
                Some(j) if j > i => Some(j - 1),
                other => other,
            };
        }
        Item::Eye(i) => {
            let before = s.edits.eyes.len();
            s.edits.eyes.remove(i);
            s.selected_eye = mask_tool::selection_after_delete(s.selected_eye, i, before);
            if s.suppressed_eye == Some(i) {
                s.suppressed_eye = None;
            }
        }
    }
}

/// Whether every patch's and removal's outline is shown.
fn all_shown(s: &Session) -> bool {
    let (p, r) = (s.edits.spots.len(), s.edits.removals.len());
    (p > 0 || r > 0) && (p == 0 || s.all_spots_shown()) && (r == 0 || s.removal.all_shown(r))
}

/// `Show all` / `Hide all` under the list, over every kind that has an outline. `true` when clicked.
fn show_all_row(ui: &mut egui::Ui, s: &Session) -> bool {
    if s.edits.spots.is_empty() && s.edits.removals.is_empty() {
        return false;
    }
    let all = all_shown(s);
    let label = if all { "Hide all" } else { "Show all" };
    let mut clicked = false;
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), ui.spacing().interact_size.y + 4.0),
        egui::Layout::right_to_left(egui::Align::Center),
        |ui| {
            clicked = ui
                .small_button(label)
                .hover(if all {
                    "Hides every retouch's outline, so you can see the photograph"
                } else {
                    "Shows every retouch's outline"
                })
                .clicked();
        },
    );
    clicked
}

/// A selected removal's Opacity and Try another fill.
fn removal_settings(ui: &mut egui::Ui, s: &mut Session, i: usize, ed: &mut Edited) {
    let mut opacity = s.edits.removals[i].opacity;
    let r = ui.add(
        super::slider::Slider::new(&mut opacity, 0.0..=100.0)
            .text("Opacity")
            .help(slider_help::text(Scope::Remove, "Opacity"))
            .reset_to(100.0),
    );
    if r.changed() {
        s.edits.removals[i].opacity = opacity;
    }
    ed.absorb(&r);
    if ui
        .button("Try another fill")
        .hover("Fills this removal again from another part of its surroundings")
        .clicked()
    {
        s.edits.removals[i].variant += 1;
        ed.discrete();
    }
}

/// A selected patch's Heal · Clone, Size and Feather — that patch only. A slider drag takes it out of the view and
/// renders once, on release, as moving one does.
fn patch_settings(ui: &mut egui::Ui, s: &mut Session, i: usize, ed: &mut Edited) {
    let mut sp = s.edits.spots[i];
    let (mut touched, mut settled, mut dragging) = (false, false, false);
    let mut mode = sp.mode;
    theme::segmented(
        ui,
        &mut mode,
        &[
            (SpotMode::Heal, "Heal", retouch_brush::HEAL_HINT),
            (SpotMode::Clone, "Clone", retouch_brush::CLONE_HINT),
        ],
    );
    if mode != sp.mode {
        sp.mode = mode;
        touched = true;
        settled = true;
    }
    let long = s.full_long;
    let mut px = spot_tool::radius_px(sp.radius, long);
    let r = ui.add(
        super::slider::Slider::new(
            &mut px,
            spot_tool::radius_px(MIN_RADIUS, long)..=spot_tool::radius_px(MAX_RADIUS, long),
        )
        .logarithmic(true)
        .text("Size")
        .help(slider_help::text(Scope::Patch, "Size"))
        .custom_formatter(|v| format!("{v:.0} px")),
    );
    if r.changed() {
        sp.radius = spot_tool::radius_from_px(px, long);
    }
    touched |= r.changed();
    settled |= super::settles(&r);
    dragging |= r.dragged();
    let r = ui.add(
        super::slider::Slider::new(&mut sp.feather, 0.0..=100.0)
            .text("Feather")
            .help(slider_help::text(Scope::Patch, "Feather"))
            .reset_to(SpotTool::default().feather),
    );
    touched |= r.changed();
    settled |= super::settles(&r);
    dragging |= r.dragged();
    if touched {
        s.edits.spots[i] = sp;
    }
    if dragging {
        if s.suppressed_spot != Some(i) {
            s.suppressed_spot = Some(i);
            ed.changed = true;
        }
    } else if settled {
        s.suppressed_spot = None;
        ed.changed = true;
    }
    ed.settled |= settled;
}
