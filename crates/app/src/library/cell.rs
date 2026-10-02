//! One grid cell: the picture, the stack badge, the label badge, the caption, the marks and the
//! hover stars, and what a click, a drag or a right-click on it does.

use super::{cell_menu, menu_targets, GridOutcome};
use crate::interaction::apply_click;
use crate::theme::Hover as _;
use crate::{name_edit, thumb};
use eframe::egui;
use rustroom_app::photo_id::PhotoId;

/// The stack count badge's size.
///
/// ⚠ Small enough not to crowd a 10 pt thumbnail and large enough to hit — the grid marks settled at
/// 10–20 pt for the same reason.
const STACK_BADGE: egui::Vec2 = egui::vec2(20.0, 16.0);

/// How much larger the grid's marks are drawn than its filename caption.
///
/// ⚠ 1.6 (user's call, 2026-09-23): the stars, the reject cross and the colour chip were legible but
/// small. They are what a culling pass actually reads, so they are not tied to the caption's size.
/// ⚠ `thumb::marks_row` clamps the result, so this cannot make them absurd on a huge cell.
pub(super) const MARK_SCALE: f32 = 1.6;

/// What a cell reads besides the library.
pub(super) struct CellInputs<'a, 'b> {
    pub(super) rename_draft: &'b mut Option<(PhotoId, String)>,
    pub(super) label_draft: &'b mut Option<(PhotoId, String)>,
    pub(super) catalog: Option<&'a catalog::db::Catalog>,
    pub(super) live: &'a Option<(std::path::PathBuf, u32, edit_model::Edits)>,
    pub(super) downloaded: &'a [bool],
    pub(super) layout: rustroom_app::cell_layout::GridCell,
    pub(super) cell: f32,
    pub(super) name_size: f32,
    /// Immich's asset → stack map — the badge keys as the grouping did.
    pub(super) remote: &'a std::collections::HashMap<String, String>,
    /// What reorders an unfurled stack (Settings → Input).
    pub(super) stack_drag: rustroom_app::settings::StackDrag,
    /// How a collapsed stack is drawn (Settings → Appearance).
    pub(super) look: rustroom_app::stack::StackLook,
}

/// A drag that starts on a member of an **unfurled** stack moves it within the stack (M10.8c after
/// use) — Settings → Input says whether ⌥ must be held — and dropping it on another member takes that
/// member's place. From anywhere else a drag is the selection rectangle, which `interactions` starts.
fn reorder_gesture(
    ui: &mut egui::Ui,
    resp: &egui::Response,
    lib: &crate::Library,
    i: usize,
    stack_drag: rustroom_app::settings::StackDrag,
    out: &mut GridOutcome,
) {
    let here = &lib.entries[i];
    if !here.stack.is_some_and(|s| s.expanded) {
        return;
    }
    if resp.drag_started_by(egui::PointerButton::Primary)
        && stack_drag.reorders(ui.input(|inp| inp.modifiers.alt))
    {
        resp.dnd_set_drag_payload(StackMove(here.id.clone()));
    }
    if resp
        .dnd_hover_payload::<StackMove>()
        .is_some_and(|p| p.0 != here.id)
    {
        ui.painter().rect_stroke(
            resp.rect,
            3.0,
            egui::Stroke::new(2.0, ui.visuals().selection.bg_fill),
            egui::StrokeKind::Outside,
        );
    }
    if let Some(p) = resp.dnd_release_payload::<StackMove>() {
        if p.0 != here.id {
            out.stack_move = Some((p.0.clone(), here.id.clone()));
        }
    }
}

/// The drag payload of a photo being moved within its unfurled stack. ⚠ Its own type, so no other
/// drag in the app can be taken for one — `keyword_rows`' rule.
#[derive(Clone)]
struct StackMove(PhotoId);

/// Draw cell `i` and collect what its interactions asked for.
pub(super) fn draw(
    ui: &mut egui::Ui,
    lib: &mut crate::Library,
    i: usize,
    c: CellInputs,
    out: &mut GridOutcome,
) {
    let CellInputs {
        rename_draft,
        label_draft,
        catalog,
        live,
        downloaded,
        layout,
        cell,
        name_size,
        remote,
        stack_drag,
        look,
    } = c;
    // ⚠⚠ A stable id per photo, not egui's auto-id counter. Under
    // `show_rows` that counter shifts with the scroll position — it skips
    // `min_row` ids while each row holds `cols` widgets — which silently
    // kills the rename field's focus: the stale id keeps focus, so
    // `autofocus`'s `focused().is_none()` guard never fires and the box
    // goes dead. Keyed by `PhotoId`, so it is also stable across sorting
    // and filtering.
    // ⚠ Cloned rather than borrowed: the closure reads `*lib` whole (the
    // context menu needs the selection *and* the entries), so a live
    // borrow of `lib.entries` in the argument would conflict with it.
    let cell_id = lib.entries[i].id.clone();
    // ⚠ `caption()`, never `file_name` — the filmstrip's rule.
    let caption = lib.entries[i].caption();
    ui.push_id(&cell_id, |ui| {
        // The picture gets the cell's height minus the caption and the
        // marks row — `cell_layout::thumb_box`'s rule.
        let box_size = egui::vec2(layout.box_w, layout.box_h);
        let cell_size = egui::vec2(cell, layout.height);
        let resp = ui
            .allocate_ui(cell_size, |ui| {
                // Pin the cell to its full size. Without this
                // `allocate_ui` reports only the space the content used,
                // so cells varied with their thumbnail — the filmstrip's
                // reflow bug in its grid form.
                ui.set_min_size(cell_size);
                ui.vertical_centered(|ui| {
                    let marks = lib
                        .marks
                        .get(&lib.entries[i].id)
                        .copied()
                        .unwrap_or_default();
                    // M10.8c after use: a collapsed stack is drawn as a pile — the photos
                    // below peeking out, plain cards, or none at all (Settings → Appearance).
                    let pile: Vec<Option<&egui::TextureHandle>> = match lib.entries[i].stack {
                        Some(st) if !st.expanded && look.draws_pile() => lib
                            .below
                            .get(&lib.entries[i].id)
                            .into_iter()
                            .flatten()
                            .take(rustroom_app::cell_layout::PILE_LAYERS)
                            .map(|m| {
                                if !look.loads_below() {
                                    return None;
                                }
                                let want = rustroom_app::thumb_loader::ThumbRequest {
                                    id: m.id.clone(),
                                    edits: m.edits.clone(),
                                };
                                lib.cache
                                    .get(&want.key())
                                    .or_else(|| lib.cache.get_stale(&m.id))
                            })
                            .collect(),
                        _ => Vec::new(),
                    };
                    // ⚠ The marks go in a row under the picture, not over
                    // it — see `thumb::marks_row`.
                    let cell_resp = thumb::cell_with(
                        ui,
                        box_size,
                        // ⚠ The exact render if it is there, else this
                        // photo's previous one — never grey while a
                        // re-render is in flight.
                        lib.cache
                            .get(&lib.thumb_request(i, live.as_ref()).key())
                            .or_else(|| lib.cache.get_stale(&lib.entries[i].id)),
                        lib.entries[i].badge().as_deref(),
                        marks,
                        thumb::CellStyle {
                            overlay_marks: false,
                            downloaded: downloaded.get(i).copied().unwrap_or(false),
                            sit_on_bottom: true,
                        },
                        &pile,
                    );
                    let e = &lib.entries[i];
                    stack_badge(ui, e, &cell_resp, out, remote, &lib.hand_stacks);
                    // ⚠ The selected cell's name is drawn in the strong
                    // text colour: the border says "selected" at the edge
                    // of the eye, the name says it where you read.
                    if lib.selection.contains(i) {
                        let strong = ui.visuals().strong_text_color();
                        ui.visuals_mut().override_text_color = Some(strong);
                    }
                    label_badge(ui, e, out);
                    caption_field(ui, e, label_draft, rename_draft, name_size, out);
                    ui.visuals_mut().override_text_color = None;
                    // ⚠ The marks get their **own** size, not the filename's:
                    // they are what the grid is read for while culling, and the
                    // caption is not. Scales with the cell exactly as
                    // `name_size` does, then `marks_row` clamps it.
                    thumb::marks_row(ui, box_size.x, marks, name_size * MARK_SCALE);
                    hover_stars(ui, &cell_resp, i, marks, name_size, e, out);
                });
            })
            .response
            // ⚠ `click_and_drag`, so a drag starting on a thumbnail can
            // begin a rubber band. A plain click still reports
            // `clicked()`; egui distinguishes the two.
            .interact(egui::Sense::click_and_drag());
        // ⚠ Before `interactions`: a drag this starts is a reorder, and the band then stays down.
        reorder_gesture(ui, &resp, lib, i, stack_drag, out);
        interactions(ui, &resp, lib, i, &caption, catalog, out);
    });
}

/// A stack's count badge, top-right of the picture; a click flips the stack.
fn stack_badge(
    ui: &mut egui::Ui,
    e: &rustroom_app::entries::PhotoEntry,
    cell_resp: &egui::Response,
    out: &mut GridOutcome,
    remote: &std::collections::HashMap<String, String>,
    hand: &rustroom_app::stack::HandStacks,
) {
    // ⚠⚠ Drawn **here** rather than inside `cell_with`, and for
    // two reasons: the count badge must be **clickable** while
    // `cell_with` allocates with `Sense::hover()`, and a click
    // in this app has to be *collected* and applied after the
    // closure releases `lib`. ⚠ Top-right, because the Immich
    // download badge already owns the top-left — and the
    // top-right **of the picture**, not of the cell, or a
    // portrait's badge floated in the empty corner beside it.
    if let Some(st) = e.stack {
        // ⚠⚠ `key_for`, the grouping's own key — `key_of` (the name alone) never matched a hand
        // stack's key nor an Immich server stack's, so their badges did nothing (M10.8c, after use).
        if let Some(k) = rustroom_app::stack::key_for(&e.id.path, remote, hand) {
            let full = thumb::image_rect(ui).unwrap_or(cell_resp.rect);
            let pill = egui::Rect::from_min_size(
                egui::pos2(full.right() - STACK_BADGE.x - 4.0, full.top() + 4.0),
                STACK_BADGE,
            );
            let r = ui.interact(pill, ui.id().with("stack_badge"), egui::Sense::click());
            // ⚠ A name for a screen reader and a test — the pill shows only a number.
            let name = if st.expanded {
                "Collapse the stack".to_string()
            } else {
                format!("Stack of {}", st.count)
            };
            r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &name));
            let v = ui.visuals();
            // ⚠ The thumbnail badge's colours (§7.10): white
            // on 55 % black at rest, the accent under the
            // pointer because it is a button.
            let hot = r.hovered();
            ui.painter().rect_filled(
                pill,
                4.0,
                if hot {
                    v.selection.bg_fill
                } else {
                    egui::Color32::from_black_alpha(140)
                },
            );
            ui.painter().text(
                pill.center(),
                egui::Align2::CENTER_CENTER,
                // ⚠ `count`, never "1": a lone file is
                // `stack: None`, so this is always ≥ 2.
                if st.expanded {
                    // ⚠ Not "×": on both children of an
                    // expanded stack it read as delete (A-11).
                    "▴".to_string()
                } else {
                    st.count.to_string()
                },
                egui::FontId::proportional(crate::theme::tokens::CAPTION),
                if hot {
                    crate::theme::on_accent(v.dark_mode)
                } else {
                    egui::Color32::WHITE
                },
            );
            let clicked = r.clicked();
            let _ = r.hover(if st.expanded {
                "Collapses this stack to one photo"
            } else {
                "Shows every file in this stack"
            });
            if clicked {
                out.stack_toggle = Some(k);
            }
        }
    }
}

/// The copy's label badge; a click opens its label draft.
fn label_badge(ui: &mut egui::Ui, e: &rustroom_app::entries::PhotoEntry, out: &mut GridOutcome) {
    // ⚠⚠ Clicking the **badge** opens the label draft,
    // clicking the **caption** opens the file-rename
    // draft: each affordance edits the string drawn at
    // that spot. That is M6.0's rule, and it is what
    // replaced one field whose meaning depended on
    // whether the row happened to be a copy.
    if let Some(br) = thumb::badge_rect(ui) {
        let bresp = ui.interact(br, ui.id().with("badge_click"), egui::Sense::click());
        if bresp.clicked() {
            out.badge_clicked = Some((e.id.clone(), e.label.clone().unwrap_or_default()));
        }
        if bresp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Text);
        }
    }
}

/// The caption, or the name field when a draft is open for this photo.
fn caption_field(
    ui: &mut egui::Ui,
    e: &rustroom_app::entries::PhotoEntry,
    label_draft: &mut Option<(PhotoId, String)>,
    rename_draft: &mut Option<(PhotoId, String)>,
    name_size: f32,
    out: &mut GridOutcome,
) {
    // ⚠ The caption is the file's name.
    //
    // ⚠ Only the **active** row is editable. A text field
    // in every cell would fight the click that selects a
    // photo, and would put hundreds of them on screen.
    // ⚠⚠ One field, and which string it edits depends on
    // which affordance opened it — the badge or the
    // caption. Two fields in a cell would fight for the
    // same space and the same click.
    let editing_label = label_draft.as_ref().is_some_and(|(d, _)| *d == e.id);
    let r = if editing_label {
        name_edit::field(
            ui,
            name_edit::NameField {
                id: &e.id,
                file_name: &e.file_name,
                label: e.label.as_deref(),
                mode: name_edit::Editing::Label,
            },
            label_draft,
            egui::FontId::proportional(name_size),
            true,
        )
    } else {
        name_edit::caption_or_field(
            ui,
            &e.id,
            &e.file_name,
            e.label.as_deref(),
            rename_draft,
            name_size,
        )
    };
    if let Some(r) = r {
        out.rename = Some((e.id.clone(), r));
    }
}

/// Five stars over the thumbnail while the pointer is on it; a click rates the photo.
fn hover_stars(
    ui: &mut egui::Ui,
    cell_resp: &egui::Response,
    i: usize,
    marks: catalog::db::Marks,
    name_size: f32,
    e: &rustroom_app::entries::PhotoEntry,
    out: &mut GridOutcome,
) {
    // ⚠ Stars under the pointer, on the thumbnail itself:
    // rating from the mouse where the eye already is (A-12).
    // Clicking the current rating clears it, like `0`.
    let thumb_rect = cell_resp.rect;
    if ui.rect_contains_pointer(thumb_rect) {
        let size = (name_size * MARK_SCALE).clamp(12.0, 18.0);
        let base = egui::pos2(thumb_rect.left() + 6.0, thumb_rect.bottom() - size * 0.75);
        let star_hit = |k: u8| {
            egui::Rect::from_center_size(
                egui::pos2(base.x + (k as f32 - 0.5) * size, base.y),
                egui::vec2(size, size),
            )
        };
        // ⚠ The pointer over star k previews a rating of k,
        // so stars 1..=k light together — a rating is a
        // count, not a position (reported 2026-09-28).
        let previewed = (1..=5u8).find(|k| ui.rect_contains_pointer(star_hit(*k)));
        for k in 1..=5u8 {
            let hit = star_hit(k);
            let c = hit.center();
            let r = ui.interact(hit, ui.id().with(("star", i, k)), egui::Sense::click());
            let lit = match previewed {
                Some(h) => k <= h,
                None => k <= marks.rating,
            };
            ui.painter().text(
                c,
                egui::Align2::CENTER_CENTER,
                if lit { "★" } else { "☆" },
                egui::FontId::proportional(size),
                if lit {
                    egui::Color32::WHITE
                } else {
                    egui::Color32::from_white_alpha(170)
                },
            );
            if r.clicked() {
                let stars = if k == marks.rating { 0 } else { k };
                out.star_click = Some((e.id.clone(), stars));
            }
        }
    }
}

/// The cell's accessible name, its selection border, and what a click, a double-click, a drag and a
/// right-click on it do.
fn interactions(
    ui: &mut egui::Ui,
    resp: &egui::Response,
    lib: &mut crate::Library,
    i: usize,
    caption: &str,
    catalog: Option<&catalog::db::Catalog>,
    out: &mut GridOutcome,
) {
    // ⚠ The name as the cell's accessible name and description, but **no
    // tooltip**: the caption under the cell already says it (the author,
    // 2026-09-30, after trying one). The filmstrip, which draws no
    // caption, keeps its tooltip.
    crate::theme::describe(resp, caption);
    let lit = lib.selection.contains(i);
    resp.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, lit, caption)
    });
    out.cell_rects.push((i, resp.rect));

    thumb::selection_border(
        ui,
        resp.rect,
        lib.selection.active() == Some(i),
        lib.selection.contains(i),
    );
    if resp.clicked() {
        let mods = ui.input(|inp| inp.modifiers);
        apply_click(&mut lib.selection, i, mods);
    }
    if resp.double_clicked() {
        out.open_index = Some(i);
    }
    // ⚠ `drag_started_by(Primary)`, never a bare `drag_started()`:
    // egui reports a drag for **every** button, so the pan binding
    // would otherwise rubber-band as it passed over the grid — the
    // trap the canvas-navigation work recorded.
    // ⚠ Not when `reorder_gesture` has just made this drag a reorder (M10.8c after use).
    if resp.drag_started_by(egui::PointerButton::Primary)
        && !egui::DragAndDrop::has_any_payload(ui.ctx())
    {
        out.band_started = resp.interact_pointer_pos();
    }
    // ⚠⚠ A secondary click does **not** fire `clicked()`, so the
    // photo is selected here explicitly — otherwise Edit and Rename
    // would act on whatever was selected before rather than on what
    // was right-clicked.
    resp.context_menu(|ui| {
        if !lib.selection.contains(i) {
            out.menu_select = Some(i);
        }
        let targets = menu_targets(lib, i);
        if let Some(a) = cell_menu(ui, catalog, &lib.source, &lib.hand_stacks, &targets) {
            out.menu = Some((i, a));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⚠ The author's call (2026-09-30, after trying it): the grid shows a photo's name in its
    /// caption, so resting on the photo shows **no** tooltip — while the name stays the cell's
    /// accessible name and description. The filmstrip's own test is the positive twin: the same
    /// rest there does show a tooltip, so this absence is not a helper that never fires.
    #[test]
    fn the_grid_names_a_photo_without_a_tooltip() {
        let ctx = crate::test_ui::ctx();
        let mut app = crate::test_app::for_test();
        let lib = crate::test_app::library_of(5);
        let name = lib.entries[1].caption();
        app.library = Some(lib);
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let mut body = |ui: &mut egui::Ui| {
            let c = ui.ctx().clone();
            crate::library::show(&mut app, ui, &c);
        };
        let nodes = crate::test_ui::nodes_on(&ctx, screen, &mut body);
        // ⚠ The cell, not its caption text, which carries the same words as a Label.
        let cell = nodes
            .iter()
            .find(|n| n.label == name && n.role == egui::accesskit::Role::Button)
            .expect("the photo's cell is drawn and named");
        assert_eq!(cell.description.as_deref(), Some(name.as_str()));
        // Near the top of the cell: the picture, not the caption row under it.
        let on_picture = egui::pos2(cell.rect.center().x, cell.rect.top() + 20.0);
        assert!(
            !crate::test_ui::tooltip_after_rest(&ctx, screen, on_picture, &mut body),
            "a tooltip over a photo whose caption already names it"
        );
    }
}
