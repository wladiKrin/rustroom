//! A mask's rows and its kind buttons: the painted row the mask list and the component list
//! share, the component rows under a refined mask, and the icon button for one mask kind.
//!
//! ⚠ Its own module because `develop/panel/masks.rs` is past the ~600 marker.

use crate::develop::panel::{mask_glyph, Edited};
use crate::Session;
use eframe::egui;
use rustroom_app::mask_kind::MaskKind;
use rustroom_app::mask_tool;

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
pub const ROW_HEIGHT: f32 = 26.0;
/// How far a component row sits in from its mask.
pub const INDENT: f32 = 18.0;
const GLYPH: f32 = 14.0;
/// How much of the accent the selected mask's tint carries — the rail's value.
const TINT: f32 = 0.18;

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
    pub indent: f32,
    /// The eye: `Some(on)` draws it left of the `×`; a mask row has one, a component row none.
    pub eye: Option<bool>,
    /// The second toggle, left of the eye: `Some((on, verb, hover))`. A mask's is "Disable"
    /// (an edit: off in the export too); a patch's is "Hold out" (view only).
    pub hold: Option<(bool, &'a str, &'a str)>,
}

pub struct RowOut {
    pub clicked: bool,
    pub delete: bool,
    /// The eye was clicked.
    pub eye: bool,
    /// The hold-out toggle was clicked.
    pub hold: bool,
}

/// Draw one row.
///
/// ⚠ The `×` and the eye are registered **after** the row so they win the hit-test tie, and a
/// click on either is not a click on the row.
pub fn row(ui: &mut egui::Ui, spec: RowSpec<'_>) -> RowOut {
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ROW_HEIGHT),
        egui::Sense::click(),
    );
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
            .on_hover_text("Delete");
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
            .on_hover_text(
                "Shows the overlay on the image, so you can see what it covers. Any number can \
                 be shown at once.",
            );
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
            .on_hover_text(hover);
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
    }
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
        // brighter names were what the user asked for (2026-09-19).
        let text = if lit {
            v.strong_text_color()
        } else {
            v.widgets.inactive.fg_stroke.color
        };
        let font = egui::TextStyle::Body.resolve(ui.style());
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
        x += GLYPH + 6.0;
        let name_rect = painter.text(
            egui::pos2(x, rect.center().y),
            egui::Align2::LEFT_CENTER,
            spec.name,
            font.clone(),
            text,
        );
        if !spec.meta.is_empty() {
            painter.text(
                egui::pos2(name_rect.right() + 8.0, rect.center().y),
                egui::Align2::LEFT_CENTER,
                spec.meta,
                egui::FontId::proportional(11.0),
                v.weak_text_color(),
            );
        }
        // The action cells, from the right edge in: `×`, then the eye.
        let mut cell_x = rect.right() - ROW_HEIGHT / 2.0;
        if spec.deletable {
            painter.text(
                egui::pos2(cell_x, rect.center().y),
                egui::Align2::CENTER_CENTER,
                "×",
                egui::FontId::proportional(17.0),
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
    }
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
                indent: INDENT,
                eye: None,
                hold: None,
            },
        );
        if out.delete {
            remove = Some(j);
        } else if out.clicked {
            pick = Some(j);
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

/// One kind as an icon button `width` wide: the glyph, and the kind's name on hover and as its
/// accessible name.
///
/// ⚠ Painted rather than a `Button` with text: five names do not fit the panel's ~240 pt on one
/// row, and the old "Add graduated filter" buttons wrapped over three. The name is one hover
/// away and every screen reader gets it.
pub fn kind_button(ui: &mut egui::Ui, kind: MaskKind, width: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, 28.0), egui::Sense::click());
    let enabled = ui.is_enabled();
    resp.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, kind.menu_label())
    });
    let resp = resp.on_hover_text(kind.hover());
    if ui.is_rect_visible(rect) {
        let v = ui.visuals();
        let wv = if resp.hovered() {
            &v.widgets.hovered
        } else {
            &v.widgets.inactive
        };
        let painter = ui.painter();
        painter.rect_filled(rect, 4.0, wv.weak_bg_fill);
        let glyph = egui::Rect::from_center_size(rect.center(), egui::vec2(15.0, 15.0));
        mask_glyph::paint(painter, glyph, kind, wv.fg_stroke.color);
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;
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
