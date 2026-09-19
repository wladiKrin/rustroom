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
pub const ROW_HEIGHT: f32 = 22.0;
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

/// One painted row: an op column, a kind glyph, a name, a `×`.
pub struct RowSpec<'a> {
    pub kind: MaskKind,
    /// The op column: `None` for a mask row (no column), `Some("")` for the base component.
    pub op: Option<&'a str>,
    /// The painted name.
    pub name: &'a str,
    /// The AccessKit name — what a screen reader says and the harnesses click.
    pub accessible: &'a str,
    /// The accent tint: the selected mask.
    pub selected: bool,
    /// The weak fill: the component being edited.
    pub current: bool,
    pub deletable: bool,
    pub indent: f32,
}

pub struct RowOut {
    pub clicked: bool,
    pub delete: bool,
}

/// Draw one row.
///
/// ⚠ The `×` is registered **after** the row so it wins the hit-test tie, and a click on it is
/// not a click on the row.
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
        let r = ui
            .interact(cell, resp.id.with("delete"), egui::Sense::click())
            .on_hover_text("Delete");
        r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, "×"));
        delete = r.clicked();
        if r.hovered() {
            x_colour = v.strong_text_color();
        }
        right = cell.left();
    }
    let clicked = resp.clicked() && !delete;

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
        let text = if lit {
            v.strong_text_color()
        } else {
            v.text_color()
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
        mask_glyph::paint(&painter, glyph, spec.kind, text);
        x += GLYPH + 6.0;
        painter.text(
            egui::pos2(x, rect.center().y),
            egui::Align2::LEFT_CENTER,
            spec.name,
            font.clone(),
            text,
        );
        if spec.deletable {
            painter.text(
                egui::pos2(right + ROW_HEIGHT / 2.0, rect.center().y),
                egui::Align2::CENTER_CENTER,
                "×",
                font,
                x_colour,
            );
        }
    }
    RowOut { clicked, delete }
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
                kind: *kind,
                op: Some(op),
                name,
                accessible,
                selected: false,
                current: selected == j,
                deletable: j > 0,
                indent: INDENT,
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
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, 24.0), egui::Sense::click());
    let enabled = ui.is_enabled();
    resp.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, kind.menu_label())
    });
    let resp = resp.on_hover_text(kind.menu_label());
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
        // ⚠ The base row has no `×`, so the first one drawn belongs to the first refinement.
        let (ctx, mut app) = composite_session();
        app.session.as_mut().unwrap().selected_component = 2;
        let s = app.session.as_mut().expect("session");
        test_ui::click(&ctx, "×", |ui| {
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
