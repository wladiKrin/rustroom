//! A mask's components: the mode chips that decide whether a kind button creates a mask or
//! refines one, and the indented list of the shapes a composite is built from.
//!
//! ⚠ Its own module because `develop/panel/masks.rs` is past the ~600 marker before this
//! milestone adds anything to it.

use crate::develop::panel::Edited;
use crate::{theme, Session};
use eframe::egui;
use rustroom_app::mask_tool;

/// The mask at component `j` — `0` is the base, `j > 0` the `j-1`th refinement.
///
/// ⚠⚠ **One resolver, and the canvas will share it.** Two derivations of "which shape is
/// selected" is exactly how the sliders and the handles would come to move different things —
/// the class of disagreement `render_max_dim`, `fit_image_size` and `Session::geometry_mode` were
/// each fixed for by deriving rather than storing twice.
///
/// ⚠ The shared-borrow twin landed **with its caller**, the canvas: a `pub fn` with no production
/// caller in a *binary* crate is dead code under `-D warnings`.
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

/// Draw the mode chips. The chosen mode is `*mode`; `None` means "New mask".
///
/// ⚠ `selectable_label`s rather than buttons: a mode with no shown selection is **invisible
/// state**, which is what the crop aspect lock was fixed for.
pub fn chips(ui: &mut egui::Ui, s: &Session, mode: &mut Option<edit_model::MaskOp>) {
    ui.horizontal_wrapped(|ui| {
        if ui.selectable_label(mode.is_none(), "New mask").clicked() {
            *mode = None;
        }
        // ⚠⚠ **Disabled without a selection rather than falling back to New.** Refining needs
        // something to refine, and a control whose meaning depends on invisible selection state
        // is the class of thing this project's record says bites. The chips say which one is
        // armed; they never quietly do something else.
        ui.add_enabled_ui(s.selected_mask.is_some(), |ui| {
            for op in edit_model::MaskOp::ALL {
                if ui
                    .selectable_label(*mode == Some(op), op.chip_label())
                    .on_disabled_hover_text("Select a mask first")
                    .clicked()
                {
                    *mode = Some(op);
                }
            }
        });
    });
    theme::hint(
        ui,
        "New mask starts a fresh mask. The other three add a shape to the \
         selected mask instead: Add widens it, Subtract cuts out of it, \
         Intersect keeps only where both reach.",
    );
}

/// Draw the selected mask's component list.
///
/// ⚠ Indented under the mask row and drawn **only for the selected mask**: a composite is one
/// mask, so its parts belong inside it rather than beside it in the flat list.
pub fn rows(ui: &mut egui::Ui, s: &mut Session, i: usize, ed: &mut Edited) {
    let Some(m) = s.edits.masks.get(i) else {
        return;
    };
    // ⚠ Names built from `Mask::label` and `Refinement::label`, never written out here, so a row
    // cannot name a kind the mask does not have.
    let names: Vec<String> = std::iter::once(m.mask.label().to_string())
        .chain(m.refine.iter().map(|r| r.label()))
        .collect();
    if names.len() == 1 {
        // ⚠ A mask with no refinements is not a composite, and a one-row list saying so is chrome
        // rather than information — the reasoning that hides the format chips below two formats.
        return;
    }

    let selected = s.component_index();
    let mut remove: Option<usize> = None;
    let mut pick: Option<usize> = None;
    ui.indent("mask_components", |ui| {
        for (j, name) in names.iter().enumerate() {
            ui.horizontal(|ui| {
                // ⚠ The base has no `×`: deleting it is deleting the **mask**, which is the mask
                // row's own button. Two ways to delete one thing is how the two come to disagree.
                if j == 0 {
                    ui.add_space(BASE_ROW_INDENT);
                } else if ui.button("×").on_hover_text("Remove this shape").clicked() {
                    remove = Some(j);
                }
                if ui.selectable_label(selected == j, name).clicked() {
                    pick = Some(j);
                }
            });
        }
    });

    if let Some(j) = pick {
        s.selected_component = j;
        // ⚠ The handle was hit-tested against the component being left.
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

/// Space standing in for the `×` on the base's row, so its name lines up with the rows that
/// carry one.
///
/// ⚠ A constant rather than a magic number at the call site: it is a layout fact about this list,
/// and the next person to change the button will look for it here.
const BASE_ROW_INDENT: f32 = 22.0;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{test_app, test_ui};

    fn radial() -> edit_model::Mask {
        edit_model::Mask::Radial(edit_model::RadialGradient::default())
    }

    /// A session holding one mask: a gradient base plus two refinements.
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

    /// Every label one frame of `rows` draws.
    ///
    /// ⚠ `labelled_centres` is the real query — AccessKit only reports **interactive** widgets, so
    /// a plain `ui.label` is invisible here. Every row is a `selectable_label`, which is one.
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
        // ⚠ Both halves: a list that never draws and one that always draws are equally wrong, and
        // only the pair distinguishes them.
        let (ctx, mut app) = composite_session();
        app.session.as_mut().unwrap().edits.masks[0].refine.clear();
        let labels = row_labels(&ctx, &mut app);
        assert!(
            !labels.iter().any(|l| l == "Gradient"),
            "a single-shape mask is not a composite: {labels:?}"
        );
    }

    /// Click a component row by label and return the session it acted on.
    fn click_row(app: &mut crate::RustroomApp, ctx: &egui::Context, label: &str) {
        let s = app.session.as_mut().expect("session");
        test_ui::click(ctx, label, |ui| {
            let mut ed = Edited::default();
            rows(ui, s, 0, &mut ed);
        });
    }

    #[test]
    fn clicking_a_component_selects_it_and_drops_the_drag_handle() {
        // ⚠⚠ The handle must go. It was hit-tested against a different shape, so keeping it would
        // let the next drag frame move the component you just left — the failure
        // `selection_after_delete` already guards for a deleted *mask*, one level down.
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
        let (ctx, mut app) = composite_session();
        app.session.as_mut().unwrap().selected_component = 2;
        // The `×` on the middle row. ⚠ Both rows carry one, so this locates by position: the
        // first `×` drawn is the first refinement's.
        let s = app.session.as_mut().expect("session");
        let at = test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
            let mut ed = Edited::default();
            rows(ui, s, 0, &mut ed);
        })
        .into_iter()
        .filter(|(l, _)| l == "×")
        .map(|(_, p)| p)
        .next()
        .expect("a remove button was drawn");
        for pressed in [true, false] {
            let mut ed = Edited::default();
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(test_ui::screen()),
                    events: vec![
                        egui::Event::PointerMoved(at),
                        egui::Event::PointerButton {
                            pos: at,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: Default::default(),
                        },
                    ],
                    ..Default::default()
                },
                |ui| rows(ui, s, 0, &mut ed),
            );
        }
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
    fn every_chip_is_drawn_and_named_from_the_op() {
        // ⚠ Built from `MaskOp::ALL`, so a fourth op cannot ship with no chip — the rule
        // `EDIT_GROUP_NAMES` and `Tool::ALL` already follow.
        let (ctx, app) = composite_session();
        let mut mode = None;
        let s = app.session.as_ref().expect("session");
        let labels: Vec<String> =
            test_ui::labelled_centres(&ctx, Vec::new(), |ui| chips(ui, s, &mut mode))
                .into_iter()
                .map(|(l, _)| l)
                .collect();
        assert!(labels.iter().any(|l| l == "New mask"));
        for op in edit_model::MaskOp::ALL {
            assert!(
                labels.iter().any(|l| l == op.chip_label()),
                "missing {}: {labels:?}",
                op.chip_label()
            );
        }
    }
}
