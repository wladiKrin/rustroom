//! A mask's rows and its kind buttons: the painted row the mask list and the component list
//! share, the component rows under a refined mask, and the icon button for one mask kind.
//!
//! ⚠ Its own module because `develop/panel/masks.rs` is past the ~600 marker.

use crate::develop::panel::{mask_glyph, Edited};
use crate::theme::Hover as _;
use crate::Session;
use eframe::egui;
use rustroom_app::mask_kind::MaskKind;
use rustroom_app::{mask_actions, mask_order, mask_tool};

/// The mask at component `j` — `0` is the base, `j > 0` the `j-1`th refinement.
///
/// ⚠⚠ **One resolver, and the canvas shares it.** Two derivations of "which shape is
/// selected" is exactly how the sliders and the handles would come to move different things —
/// the class of disagreement `render_max_dim`, `fit_image_size` and `Session::geometry_mode` were
/// each fixed for by deriving rather than storing twice.
pub fn component(m: &edit_model::MaskedAdjustment, j: usize) -> Option<&edit_model::Mask> {
    match j.checked_sub(1) {
        None => Some(&m.mask),
        Some(k) => m.refine.get(k).map(|r| &r.mask),
    }
}

/// The mutable twin. ⚠ Both exist because the canvas reads and the panel writes.
pub fn component_mut(
    m: &mut edit_model::MaskedAdjustment,
    j: usize,
) -> Option<&mut edit_model::Mask> {
    match j.checked_sub(1) {
        None => Some(&mut m.mask),
        Some(k) => m.refine.get_mut(k).map(|r| &mut r.mask),
    }
}

/// Row height in points.
pub const ROW_HEIGHT: f32 = crate::theme::tokens::LIST_ROW;
/// A mask row's hover.
pub const MASK_ROW_HOVER: &str =
    "Selects this mask, and a drag moves it to change the order the masks apply in";
/// A mask's component row's hover.
pub const COMPONENT_ROW_HOVER: &str =
    "Selects this shape, and a drag moves it to change the order the shapes combine in";
/// "Invert mask"'s hover.
pub const WHOLE_INVERT_HOVER: &str =
    "Inverts the whole mask so it covers what its shapes leave out";
/// How far a component row sits in from its mask.
pub const INDENT: f32 = 18.0;
const GLYPH: f32 = crate::theme::tokens::ICON_INLINE;
/// How much of the accent the selected mask's tint carries — the rail's value.
const TINT: f32 = crate::theme::tokens::TINT;

/// The glyph a refinement's op shows in the row's first column.
pub fn op_glyph(op: edit_model::MaskOp) -> &'static str {
    match op {
        edit_model::MaskOp::Add => "+",
        edit_model::MaskOp::Subtract => "−",
        edit_model::MaskOp::Intersect => "∩",
    }
}

/// What a row's glyph shows: a mask's kind, or a retouch patch's mode.
#[derive(Clone, Copy)]
pub enum RowGlyph {
    Mask(MaskKind),
    Spot(edit_model::SpotMode),
    /// A red- or pet-eye correction (M9.11).
    Eye(edit_model::EyeKind),
    Preset,
}

/// One painted row: an op column, a glyph, a name and its meta, then the action cells.
pub struct RowSpec<'a> {
    pub glyph: RowGlyph,
    /// The op column: `None` for a mask row (no column), `Some("")` for the base component.
    pub op: Option<&'a str>,
    /// The painted name.
    pub name: &'a str,
    /// Weak text after the name — a patch's mode and size; empty for a mask.
    pub meta: &'a str,
    /// The AccessKit name — what a screen reader says and the harnesses click.
    pub accessible: &'a str,
    /// The accent tint: the selected mask.
    pub selected: bool,
    /// The weak fill: the component being edited.
    pub current: bool,
    pub deletable: bool,
    /// What the `×` does, in one sentence — the row's own kind (M10.9a: every row's said "Deletes
    /// this mask", a preset's and a patch's included).
    pub delete_hover: &'a str,
    pub indent: f32,
    /// The eye: `Some(on)` draws it left of the `×`; a mask row has one, a component row none.
    pub eye: Option<bool>,
    /// The second toggle, left of the eye: `Some((on, verb, hover))`. A mask's is "Disable"
    /// (an edit: off in the export too); a patch's is "Hold out" (view only).
    pub hold: Option<(bool, &'a str, &'a str)>,
    /// What a click on the row does, in one sentence (M10.6b) — required, a row is a control.
    pub hover: &'a str,
    /// Draggable to reorder (masks and their components); patches, eyes and presets are not.
    pub draggable: bool,
}

pub struct RowOut {
    pub clicked: bool,
    pub delete: bool,
    /// The eye was clicked.
    pub eye: bool,
    /// The hold-out toggle was clicked.
    pub hold: bool,
    /// The row's rect, for the drop slots.
    pub rect: egui::Rect,
    /// Being dragged this frame.
    pub dragging: bool,
    /// A drag of this row ended this frame.
    pub released: bool,
    /// The row's own response — a right-click menu, a double-click and a drag are read from it
    /// (M10.9a, the presets).
    pub response: egui::Response,
}

/// The least share of the text's room the name keeps when the meta wants more.
const NAME_SHARE: f32 = 0.55;
/// The gap between the name and the meta.
const META_GAP: f32 = 8.0;
/// Narrower than this, the meta is not drawn at all — a sliver of it reads as a glitch.
const MIN_META: f32 = 16.0;

/// The meta between `left` and `right`: clipped there, and scrolled sideways under the pointer (a
/// trackpad swipe, Shift and the wheel) when it does not fit — the author's choice over cutting it.
///
/// ⚠ Hover-only text in a scroll area that does not drag: a click or a drag on it is still the
/// row's — applying a preset, reordering a mask.
fn meta(
    ui: &mut egui::Ui,
    spec: &RowSpec<'_>,
    id: egui::Id,
    left: f32,
    right: f32,
    row: egui::Rect,
    font: egui::FontId,
) {
    let area =
        egui::Rect::from_min_max(egui::pos2(left, row.top()), egui::pos2(right, row.bottom()));
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(area)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    egui::ScrollArea::horizontal()
        .id_salt(id.with("meta"))
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .scroll_source(egui::scroll_area::ScrollSource::MOUSE_WHEEL)
        .max_width(area.width())
        .show(&mut child, |ui| {
            ui.set_min_height(area.height());
            ui.add(
                egui::Label::new(
                    egui::RichText::new(spec.meta)
                        .font(font)
                        .color(ui.visuals().weak_text_color()),
                )
                .selectable(false)
                .sense(egui::Sense::hover())
                .wrap_mode(egui::TextWrapMode::Extend),
            );
        });
}

/// Draw one row.
///
/// ⚠ The `×` and the eye are registered **after** the row so they win the hit-test tie, and a
/// click on either is not a click on the row.
pub fn row(ui: &mut egui::Ui, spec: RowSpec<'_>) -> RowOut {
    // ⚠ Drag is sensed only where a reorder exists: egui separates a click from a drag by its
    // threshold, so a reorderable row still selects on a click.
    let sense = if spec.draggable {
        egui::Sense::click_and_drag()
    } else {
        egui::Sense::click()
    };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW_HEIGHT), sense);
    let enabled = ui.is_enabled();
    let lit = spec.selected || spec.current;
    resp.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            enabled,
            lit,
            spec.accessible,
        )
    });
    let v = ui.visuals().clone();
    let mut delete = false;
    let mut right = rect.right();
    let mut x_colour = v.weak_text_color();
    if spec.deletable {
        let cell =
            egui::Rect::from_min_max(egui::pos2(rect.right() - ROW_HEIGHT, rect.top()), rect.max);
        // ⚠ Named after its row for a screen reader — and for the harness, which cannot tell
        // one "×" from another (a test deleted the wrong mask before this).
        let name = format!("Delete {}", spec.accessible);
        let r = ui
            .interact(cell, resp.id.with("delete"), egui::Sense::click())
            .hover(spec.delete_hover);
        r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, &name));
        delete = r.clicked();
        if r.hovered() {
            x_colour = v.strong_text_color();
        }
        right = cell.left();
    }
    let mut eye = false;
    let mut eye_colour = v.weak_text_color();
    if let Some(on) = spec.eye {
        let cell = egui::Rect::from_min_max(
            egui::pos2(right - ROW_HEIGHT, rect.top()),
            egui::pos2(right, rect.bottom()),
        );
        let name = format!("Show {}", spec.accessible);
        let r = ui
            .interact(cell, resp.id.with("eye"), egui::Sense::click())
            .hover("Shows this mask's overlay on the photo");
        r.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::Checkbox, enabled, on, &name)
        });
        eye = r.clicked();
        eye_colour = if on {
            v.selection.bg_fill
        } else if r.hovered() {
            v.strong_text_color()
        } else {
            v.weak_text_color()
        };
        // ⚠ Advance, or the next cell lands on this one's hit area and wins the tie — which is
        // exactly what happened when this line was dropped as "never read" (2026-09-20).
        right = cell.left();
    }
    let mut hold = false;
    let mut hold_colour = v.weak_text_color();
    if let Some((on, verb, hover)) = spec.hold {
        let cell = egui::Rect::from_min_max(
            egui::pos2(right - ROW_HEIGHT, rect.top()),
            egui::pos2(right, rect.bottom()),
        );
        let name = format!("{verb} {}", spec.accessible);
        let r = ui
            .interact(cell, resp.id.with("hold"), egui::Sense::click())
            .hover(hover);
        r.widget_info(|| {
            egui::WidgetInfo::selected(egui::WidgetType::Checkbox, enabled, on, &name)
        });
        hold = r.clicked();
        hold_colour = if on {
            v.selection.bg_fill
        } else if r.hovered() {
            v.strong_text_color()
        } else {
            v.weak_text_color()
        };
        right = cell.left();
    }
    // ⚠⚠ The text's room ends at the first action cell — never under it (M10.9a after use: a
    // preset's list of groups ran under its `×`). The name is elided with "…" and shows in full on
    // hover; the meta takes what is left and scrolls sideways when it does not fit.
    let text = if lit {
        v.strong_text_color()
    } else {
        v.widgets.inactive.fg_stroke.color
    };
    let font = egui::TextStyle::Body.resolve(ui.style());
    let meta_font = egui::FontId::proportional(crate::theme::tokens::CAPTION);
    let glyph_left = rect.left() + spec.indent + 6.0 + if spec.op.is_some() { 14.0 } else { 0.0 };
    let text_left = glyph_left + GLYPH + 6.0;
    let text_right = right - 6.0;
    let span = (text_right - text_left).max(0.0);
    let meta_w = if spec.meta.is_empty() {
        0.0
    } else {
        let job = egui::text::LayoutJob::simple_singleline(
            spec.meta.to_owned(),
            meta_font.clone(),
            v.weak_text_color(),
        );
        ui.fonts_mut(|f| f.layout_job(job)).size().x + META_GAP
    };
    // The name gets all the room the meta does not need — but at least `NAME_SHARE` of it.
    let name_room = (span - meta_w).max(span * NAME_SHARE).min(span);
    let mut job =
        egui::text::LayoutJob::simple_singleline(spec.name.to_owned(), font.clone(), text);
    job.wrap.max_width = name_room;
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    let name_galley = ui.fonts_mut(|f| f.layout_job(job));
    let resp = if name_galley.elided {
        resp.hover_titled(spec.name, spec.hover)
    } else {
        resp.hover(spec.hover)
    };
    let clicked = resp.clicked() && !delete && !eye && !hold;

    if ui.is_rect_visible(rect) {
        let painter = ui.painter_at(rect);
        let fill = if spec.selected {
            v.selection.bg_fill.gamma_multiply(TINT)
        } else if spec.current {
            v.widgets.inactive.weak_bg_fill
        } else if resp.hovered() {
            v.widgets.hovered.weak_bg_fill
        } else {
            egui::Color32::TRANSPARENT
        };
        if fill != egui::Color32::TRANSPARENT {
            painter.rect_filled(rect, 4.0, fill);
        }
        // ⚠ The button-text colour, not body text: the rows are controls, and the mockup's
        // brighter names were what the user asked for (2026-09-19) — `text`, above.
        let mut x = rect.left() + spec.indent + 6.0;
        if let Some(op) = spec.op {
            painter.text(
                egui::pos2(x + 5.0, rect.center().y),
                egui::Align2::CENTER_CENTER,
                op,
                font.clone(),
                v.weak_text_color(),
            );
            x += 14.0;
        }
        let glyph = egui::Rect::from_center_size(
            egui::pos2(x + GLYPH / 2.0, rect.center().y),
            egui::vec2(GLYPH, GLYPH),
        );
        match spec.glyph {
            RowGlyph::Mask(k) => mask_glyph::paint(&painter, glyph, k, text),
            RowGlyph::Spot(m) => mask_glyph::paint_spot(&painter, glyph, m, text),
            RowGlyph::Eye(k) => mask_glyph::paint_pupil(&painter, glyph, k, text),
            RowGlyph::Preset => mask_glyph::paint_star(&painter, glyph, text),
        }
        debug_assert!(
            (x + GLYPH + 6.0 - text_left).abs() < 0.01,
            "one text origin"
        );
        let name_right = text_left + name_galley.size().x;
        painter.galley(
            egui::pos2(text_left, rect.center().y - name_galley.size().y / 2.0),
            name_galley,
            text,
        );
        let meta_left = name_right + META_GAP;
        if !spec.meta.is_empty() && text_right - meta_left > MIN_META {
            meta(ui, &spec, resp.id, meta_left, text_right, rect, meta_font);
        }
        // The action cells, from the right edge in: `×`, then the eye.
        let mut cell_x = rect.right() - ROW_HEIGHT / 2.0;
        if spec.deletable {
            painter.text(
                egui::pos2(cell_x, rect.center().y),
                egui::Align2::CENTER_CENTER,
                "×",
                egui::FontId::proportional(crate::theme::tokens::ICON),
                x_colour,
            );
            cell_x -= ROW_HEIGHT;
        }
        if let Some(on) = spec.eye {
            let eye_rect = egui::Rect::from_center_size(
                egui::pos2(cell_x, rect.center().y),
                egui::vec2(16.0, 16.0),
            );
            mask_glyph::paint_eye(&painter, eye_rect, on, eye_colour);
            cell_x -= ROW_HEIGHT;
        }
        if let Some((on, _, _)) = spec.hold {
            let hold_rect = egui::Rect::from_center_size(
                egui::pos2(cell_x, rect.center().y),
                egui::vec2(16.0, 16.0),
            );
            mask_glyph::paint_hold(&painter, hold_rect, on, hold_colour);
        }
    }
    RowOut {
        clicked,
        delete,
        eye,
        hold,
        rect,
        dragging: resp.dragged(),
        released: resp.drag_stopped(),
        response: resp,
    }
}

/// The slot a row dragged to the pointer would land in, and the y of its insertion line.
///
/// ⚠ The boundaries are the rows' **midpoints**, not their tops: the upper half of a row is
/// "before it", the lower half "after it". The Library's past-the-top rule suits its tall section
/// headers; on a 30-pt row it made a drop on the row above land where the row started.
pub fn drop_slot(ui: &egui::Ui, rects: &[egui::Rect]) -> Option<(usize, f32)> {
    let y = ui.ctx().pointer_interact_pos()?.y;
    let mids: Vec<f32> = rects.iter().map(|r| r.center().y).collect();
    let at = rustroom_app::panel_order::drop_index(&mids, y);
    // ⚠ Past the last row the line sits at the cursor — below anything drawn under that row (a
    // selected mask's components), which is where such a drop lands.
    let line = rects.get(at).map_or(ui.cursor().top(), |r| r.top());
    Some((at, line))
}

/// The insertion line: the Library rail's, in the accent.
pub fn paint_drop_line(ui: &egui::Ui, x: egui::Rangef, y: f32) {
    ui.painter()
        .hline(x, y, egui::Stroke::new(2.0, ui.visuals().selection.bg_fill));
}

/// The component rows of mask `i`, if it has any refinements.
///
/// ⚠ A single-shape mask draws nothing here: its one row above is the shape, and a second row
/// saying "Gradient" under "Gradient 1" would be noise.
pub fn rows(ui: &mut egui::Ui, s: &mut Session, i: usize, ed: &mut Edited) {
    let Some(m) = s.edits.masks.get(i) else {
        return;
    };
    if m.refine.is_empty() {
        return;
    }
    let names: Vec<(MaskKind, &'static str, String, &'static str)> = std::iter::once((
        MaskKind::of(&m.mask),
        "",
        m.mask.label().to_string(),
        m.mask.label(),
    ))
    .chain(m.refine.iter().map(|r| {
        (
            MaskKind::of(&r.mask),
            op_glyph(r.op),
            r.label(),
            r.mask.label(),
        )
    }))
    .collect();
    let selected = s.component_index();
    let mut remove: Option<usize> = None;
    let mut pick: Option<usize> = None;
    let mut rects = Vec::with_capacity(names.len());
    // The row being dragged, and whether it was released this frame.
    let mut drag: Option<(usize, bool)> = None;
    // A row's op chosen from its right-click menu (M10.12a).
    let mut set_op: Option<(usize, edit_model::MaskOp)> = None;
    for (j, (kind, op, accessible, name)) in names.iter().enumerate() {
        let out = row(
            ui,
            RowSpec {
                glyph: RowGlyph::Mask(*kind),
                op: Some(op),
                name,
                meta: "",
                accessible,
                selected: false,
                current: selected == j,
                deletable: j > 0,
                delete_hover: "Deletes this part of the mask",
                indent: INDENT,
                eye: None,
                hold: None,
                hover: COMPONENT_ROW_HOVER,
                draggable: true,
            },
        );
        // ⚠ The base row has no op (M10.12a): it is the Add every other row folds onto.
        if j > 0 {
            let current = s.edits.masks[i].refine[j - 1].op;
            out.response.context_menu(|ui| {
                for op in edit_model::MaskOp::ALL {
                    if ui.radio(op == current, op.label()).clicked() {
                        set_op = Some((j, op));
                        ui.close();
                    }
                }
            });
        }
        rects.push(out.rect);
        if out.dragging || out.released {
            drag = Some((j, out.released));
        }
        if out.delete {
            remove = Some(j);
        } else if out.clicked {
            pick = Some(j);
        }
    }
    if let Some((j, op)) = set_op {
        if mask_actions::set_op(&mut s.edits.masks[i], j, op) {
            ed.discrete();
        }
        return;
    }
    if let (Some((from, released)), None, None) = (drag, pick, remove) {
        if let Some((to, line)) = drop_slot(ui, &rects) {
            // ⚠ A refused slot draws no line, and a release there does nothing — the only signal,
            // and enough: the line is what says where the row would go.
            let offered = s
                .edits
                .masks
                .get(i)
                .is_some_and(|m| mask_order::component_drop_allowed(m, from, to));
            if offered {
                if released {
                    let mut sel = s.component_index();
                    if mask_order::move_component(&mut s.edits.masks[i], &mut sel, from, to) {
                        s.selected_component = sel;
                        s.active_mask_handle = None;
                        ed.discrete();
                    }
                } else {
                    let r = ui.max_rect();
                    paint_drop_line(ui, egui::Rangef::new(r.left() + INDENT, r.right()), line);
                }
            }
        }
    }
    if let Some(j) = pick {
        s.selected_component = j;
        s.active_mask_handle = None;
    }
    if let Some(j) = remove {
        let before = names.len();
        s.edits.masks[i].refine.remove(j - 1);
        s.selected_component = mask_tool::component_after_delete(s.selected_component, j, before);
        s.active_mask_handle = None;
        ed.discrete();
    }
}

/// "Invert mask" under mask `i`'s rows, where [`mask_actions::whole_invert_drawn`] says — a
/// composite, or a mask whose flag is still on (M10.12a).
pub fn whole_invert(ui: &mut egui::Ui, s: &mut Session, i: usize, ed: &mut Edited) {
    let Some(m) = s.edits.masks.get_mut(i) else {
        return;
    };
    if !mask_actions::whole_invert_drawn(m) {
        return;
    }
    ui.horizontal(|ui| {
        ui.add_space(INDENT);
        if ui
            .checkbox(&mut m.invert, "Invert mask")
            .hover(WHOLE_INVERT_HOVER)
            .changed()
        {
            ed.discrete();
        }
    });
}

/// A kind button's height: 36, up from 28 with option B of the 2026-09-29 canvas ("same modes,
/// larger") — the buttons that make a mask read as the panel's primary controls.
const KIND_HEIGHT: f32 = 36.0;
/// The painted kind glyph's box, up from 15 with [`KIND_HEIGHT`].
const KIND_GLYPH: f32 = 18.0;

/// One kind as an icon button `width` wide: the glyph, and the kind's name on hover and as its
/// accessible name.
///
/// ⚠ Painted rather than a `Button` with text: five names do not fit the panel's ~240 pt on one
/// row, and the old "Add graduated filter" buttons wrapped over three. The name is one hover
/// away and every screen reader gets it.
pub fn kind_button(ui: &mut egui::Ui, kind: MaskKind, width: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, KIND_HEIGHT), egui::Sense::click());
    let enabled = ui.is_enabled();
    resp.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, kind.menu_label())
    });
    let resp = resp.hover(kind.hover());
    if ui.is_rect_visible(rect) {
        let v = ui.visuals();
        let wv = if resp.hovered() {
            &v.widgets.hovered
        } else {
            &v.widgets.inactive
        };
        let painter = ui.painter();
        painter.rect_filled(rect, crate::theme::tokens::RADIUS, wv.weak_bg_fill);
        let glyph = egui::Rect::from_center_size(rect.center(), egui::vec2(KIND_GLYPH, KIND_GLYPH));
        // ⚠ In `text`, not the widget grey: at this size the grey read as disabled beside the lit
        // mode block above (the 2026-09-29 canvas drew them bright).
        let colour = crate::theme::tokens::of(ui).text;
        mask_glyph::paint(painter, glyph, kind, colour);
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⚠⚠ M10.9a after use: no text runs under the `×` — a long name is elided, a long meta
    /// clipped before the cell (a preset's list of groups ran under it).
    #[test]
    fn the_text_stops_before_the_delete_cell() {
        let ctx = egui::Context::default();
        let mut row_rect = egui::Rect::NOTHING;
        let out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(220.0, 200.0),
                )),
                ..Default::default()
            },
            |ui| {
                let o = row(
                    ui,
                    RowSpec {
                        glyph: RowGlyph::Preset,
                        op: None,
                        name: "A very long preset name indeed",
                        meta: "White balance · Tone · Color · Detail · Lens",
                        accessible: "x",
                        selected: false,
                        current: false,
                        deletable: true,
                        delete_hover: "Deletes this preset",
                        indent: 0.0,
                        eye: None,
                        hold: None,
                        hover: "Applies this preset to the photo",
                        draggable: false,
                    },
                );
                row_rect = o.rect;
            },
        );
        let cell_left = row_rect.right() - ROW_HEIGHT;
        let mut seen_texts = Vec::new();
        for c in &out.shapes {
            if let egui::Shape::Text(t) = &c.shape {
                if t.galley.text() == "×" {
                    continue;
                }
                let seen = t.visual_bounding_rect().intersect(c.clip_rect);
                if seen.is_positive() {
                    assert!(
                        seen.right() <= cell_left + 0.5,
                        "{:?} reaches {} past the × at {}",
                        t.galley.text(),
                        seen.right(),
                        cell_left
                    );
                    // ⚠ An elided galley still reports its whole text — the flag says it was cut.
                    seen_texts.push((t.galley.text().to_string(), t.galley.elided));
                }
            }
        }
        assert!(
            seen_texts
                .iter()
                .any(|(t, cut)| t.starts_with("A very") && *cut)
                && seen_texts
                    .iter()
                    .any(|(t, _)| t.starts_with("White balance")),
            "the sample: an elided name and the meta were drawn — {seen_texts:?}"
        );
    }
    use crate::{test_app, test_ui};

    fn radial() -> edit_model::Mask {
        edit_model::Mask::Radial(edit_model::RadialGradient::default())
    }

    fn composite_session() -> (egui::Context, crate::RustroomApp) {
        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();
        test_app::open_test_photo(&mut app, &ctx, 80, 40);
        {
            let s = app.session.as_mut().expect("session");
            s.edits.masks = vec![edit_model::MaskedAdjustment {
                mask: edit_model::Mask::Linear(edit_model::LinearGradient::default()),
                refine: vec![
                    edit_model::Refinement {
                        op: edit_model::MaskOp::Subtract,
                        mask: radial(),
                    },
                    edit_model::Refinement {
                        op: edit_model::MaskOp::Intersect,
                        mask: edit_model::Mask::Luminance(edit_model::LuminanceRange::default()),
                    },
                ],
                adjustments: Default::default(),
                disabled: false,
                invert: false,
            }];
            s.selected_mask = Some(0);
        }
        (ctx, app)
    }

    /// ⚠ Every row is a `SelectableLabel`, so it has an AccessKit node; a plain label would be
    /// invisible here.
    fn row_labels(ctx: &egui::Context, app: &mut crate::RustroomApp) -> Vec<String> {
        let s = app.session.as_mut().expect("session");
        test_ui::labelled_centres(ctx, Vec::new(), |ui| {
            let mut ed = Edited::default();
            rows(ui, s, 0, &mut ed);
        })
        .into_iter()
        .map(|(l, _)| l)
        .collect()
    }

    #[test]
    fn the_components_are_listed_under_their_mask() {
        // ⚠ The accessible names are the *op and kind*, not the painted kind name alone: a
        // screen reader (and this harness) must be able to tell "Subtract Radial" from a
        // second radial added with Intersect.
        let (ctx, mut app) = composite_session();
        let labels = row_labels(&ctx, &mut app);
        for want in ["Gradient", "Subtract Radial", "Intersect Luminance"] {
            assert!(
                labels.iter().any(|l| l == want),
                "missing {want}: {labels:?}"
            );
        }
    }

    #[test]
    fn a_mask_with_no_refinements_draws_no_list() {
        let (ctx, mut app) = composite_session();
        app.session.as_mut().unwrap().edits.masks[0].refine.clear();
        let labels = row_labels(&ctx, &mut app);
        assert!(
            !labels.iter().any(|l| l == "Gradient"),
            "a single-shape mask is not a composite: {labels:?}"
        );
    }

    fn click_row(app: &mut crate::RustroomApp, ctx: &egui::Context, label: &str) {
        let s = app.session.as_mut().expect("session");
        test_ui::click(ctx, label, |ui| {
            let mut ed = Edited::default();
            rows(ui, s, 0, &mut ed);
        });
    }

    #[test]
    fn clicking_a_component_selects_it_and_drops_the_drag_handle() {
        let (ctx, mut app) = composite_session();
        app.session.as_mut().unwrap().active_mask_handle =
            Some(rustroom_app::mask_tool::Handle::Center);
        click_row(&mut app, &ctx, "Intersect Luminance");
        let s = app.session.as_ref().unwrap();
        assert_eq!(
            s.component_index(),
            2,
            "the clicked row becomes the selection"
        );
        assert!(
            s.active_mask_handle.is_none(),
            "a handle hit-tested against another shape must not survive"
        );
    }

    #[test]
    fn deleting_a_component_removes_it_and_clamps_the_selection() {
        // ⚠ The base row has no `×`; the refinement's is named after it.
        let (ctx, mut app) = composite_session();
        app.session.as_mut().unwrap().selected_component = 2;
        let s = app.session.as_mut().expect("session");
        test_ui::click(&ctx, "Delete Subtract Radial", |ui| {
            let mut ed = Edited::default();
            rows(ui, s, 0, &mut ed);
        });
        assert_eq!(
            s.edits.masks[0].refine.len(),
            1,
            "one refinement was removed"
        );
        assert_eq!(
            s.component_index(),
            1,
            "the selection stepped back onto a shape that still exists"
        );
    }

    #[test]
    fn every_kind_button_is_named_for_accessibility() {
        // ⚠ The buttons paint a glyph and no text; the kind's name reaches a sighted user as a
        // tooltip and everyone else — a screen reader, this harness — as the accessible name.
        // Sabotage: drop `widget_info` and every label vanishes.
        let ctx = test_ui::ctx();
        let labels: Vec<String> = test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
            for k in MaskKind::ALL {
                let _ = kind_button(ui, k, 40.0);
            }
        })
        .into_iter()
        .map(|(l, _)| l)
        .collect();
        for k in MaskKind::ALL {
            assert!(
                labels.iter().any(|l| l == k.menu_label()),
                "{} has no accessible name: {labels:?}",
                k.menu_label()
            );
        }
    }
}
