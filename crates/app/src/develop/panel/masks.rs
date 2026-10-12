//! The Masks panel: the mask list with its Add menu and Refine row, the Shape section, and the
//! selected mask's adjustments.
//!
//! ⚠ Shown only while the **Masks** tool is selected on the rail, and it is then the *whole*
//! panel — the global adjustment sliders are not visible alongside it. A mask carries its own
//! Temp…Sharpening, so nothing here needs them.
//!
//! ⚠ The layout is option C of the 2026-09-19 canvas ("icon rows, minimal change"): a
//! segmented mode control (New mask / Add / Subtract / Intersect), one row of kind icons,
//! painted rows, then SHAPE and the four adjustment sections the Edit panel also uses — one
//! layout learned once. Option A ("menus, not modes") was built first the same day and
//! replaced at the user's request: the mode is visible state here, and an armed Subtract
//! still adds a shape you did not mean, which the user accepted for the shorter panel.

use crate::develop::panel::{mask_components, section, Edited};
use crate::theme::Hover as _;
use crate::Session;
use eframe::egui;
use rustroom_app::mask_kind::MaskKind;
use rustroom_app::{mask_actions, mask_order, mask_tool};

/// Brush tool settings, which live on the app rather than on the mask.
///
/// ⚠ They are the *tool's* state and apply to the next stroke, exactly as a brush size does
/// in any editor. Each stroke records its own copy at the moment it is drawn.
///
/// ⚠ Six `&mut` to *disjoint* fields of `RustroomApp`, which is what replaced the previous
/// copy-into-locals-and-write-back workaround: that existed only because a closure capturing
/// `app` while `s` also borrowed it is `E0499`. Explicit parameters have no such problem.
pub struct BrushTool<'a> {
    pub size: &'a mut f32,
    pub hardness: &'a mut f32,
    pub flow: &'a mut f32,
    pub erase: &'a mut bool,
    pub auto_mask: &'a mut bool,
    pub tolerance: &'a mut f32,
}

/// The panel state a mask's controls need that lives on `RustroomApp` rather than on the mask.
///
/// ⚠ Three `&mut` to **disjoint** fields, which is what makes this legal where a closure capturing
/// `app` would be `E0499` — the reasoning `BrushTool` already carries.
pub struct MaskUi<'a> {
    pub hsl: &'a mut super::adjustments::HslChannel,
    pub grade: &'a mut super::color_wheel::GradeRange,
    /// `None` = New mask; `Some(op)` = the next kind button refines the selected mask.
    pub add_mode: &'a mut Option<edit_model::MaskOp>,
    /// Whether the mask's tone curve is expanded — `Settings::tone_curve_open`, saved by the caller.
    pub curve_open: &'a mut bool,
}

/// Draw the Masks tool's controls.
///
/// ⚠ No plain heading: the MASKS section header is the heading. `Tool::Masks.panel_heading()`
/// returns `None` for that reason.
pub fn show(
    ui: &mut egui::Ui,
    s: &mut Session,
    ui_state: MaskUi<'_>,
    mut brush: BrushTool<'_>,
    ed: &mut Edited,
) {
    let MaskUi {
        hsl: mask_hsl,
        grade: mask_grade,
        add_mode,
        curve_open,
    } = ui_state;
    // Hoisted above every `&mut s.*` below: `Illuminant` is `Copy`, so reading it here costs
    // nothing and keeps the later mutable borrows disjoint.
    let baseline = s.wb_baseline;
    // ⚠ Once per frame, before anything reads it: undo, paste and snapshots change the mask
    // count without passing through this panel.
    s.sync_mask_shown();

    // ── MASKS ──
    // ⚠ `reset` here is the old "Remove all masks": it belongs to the list as a whole, so it
    // sits on its header.
    let mut out = section::header(
        ui,
        ui.make_persistent_id(("mask", "Masks")),
        true,
        section::Header::new("Masks")
            .modified(!s.edits.masks.is_empty())
            .resettable(true),
    );
    if out.reset {
        s.edits.merge_from(
            &edit_model::Edits::default(),
            edit_model::EditGroups::only(edit_model::MASKS_GROUP),
        );
        // ⚠ Both must be cleared, for the reason `selection_after_delete` exists: the canvas
        // overlay returns early on a stale selection, which would leave a dead handle set.
        s.selected_mask = None;
        s.selected_component = 0;
        s.active_mask_handle = None;
        s.mask_shown.clear();
        ed.discrete();
    }
    out.show_body(ui, |ui| {
        add_controls(ui, s, add_mode, ed);
        list(ui, s, ed);
        show_all_row(ui, s);
    });

    let Some(i) = s.selected_mask else {
        return;
    };
    let j = s.component_index();
    let Some(m) = s.edits.masks.get_mut(i) else {
        return;
    };

    // ── SHAPE ──
    {
        let Some(shape) = mask_components::component_mut(m, j) else {
            return;
        };
        let kind = shape.label();
        let mut out = section::header(
            ui,
            ui.make_persistent_id(("mask", "Shape")),
            true,
            section::Header::new("Shape").trailing(kind),
        );
        out.show_body(ui, |ui| shape_block(ui, shape, &mut brush, ed));
    }

    super::mask_adjust::show(
        ui,
        &mut m.adjustments,
        baseline,
        mask_hsl,
        mask_grade,
        curve_open,
        ed,
    );
}

/// The mode control's height per row: 32-pt segments once the four wrap into two rows of two,
/// which they do at the panel's width with an icon before each label (the author's pick, option B
/// on the 2026-09-29 canvas — "same modes, larger").
const MODE_HEIGHT: f32 = 34.0;

/// The mode control and the row of kind buttons.
///
/// ⚠ The mode is **visible state** — a segmented control, never a hidden flag — and it is
/// inert without a selection: the three ops mean nothing with no mask to refine, so the control
/// is disabled and the mode forced back to New mask. A kind click in New mask (or with nothing
/// selected) adds a mask; in an op mode it appends a refinement to the selected mask and selects
/// the new shape, or the sliders below would still be pointing at whatever was selected before.
fn add_controls(
    ui: &mut egui::Ui,
    s: &mut Session,
    mode: &mut Option<edit_model::MaskOp>,
    ed: &mut Edited,
) {
    let can_refine = s.selected_mask.is_some_and(|i| i < s.edits.masks.len());
    if !can_refine {
        *mode = None;
    }
    // ⚠ The hint that used to sit under the buttons, split into a hover per mode (user's
    // call, 2026-09-19).
    use crate::segmented::{Mark, Segment};
    use rustroom_app::icons;
    let items = [
        Segment {
            value: None,
            label: "New mask",
            hover: "Starts a new mask with the next kind you pick",
            mark: Mark::Icon(icons::MASK_NEW),
        },
        Segment {
            value: Some(edit_model::MaskOp::Add),
            label: edit_model::MaskOp::Add.label(),
            hover: "Adds the next shape to the selected mask",
            mark: Mark::Icon(icons::MASK_ADD),
        },
        Segment {
            value: Some(edit_model::MaskOp::Subtract),
            label: edit_model::MaskOp::Subtract.label(),
            hover: "Cuts the next shape out of the selected mask",
            mark: Mark::Icon(icons::MASK_SUBTRACT),
        },
        Segment {
            value: Some(edit_model::MaskOp::Intersect),
            label: edit_model::MaskOp::Intersect.label(),
            hover: "Keeps only where the selected mask and the next shape overlap",
            mark: Mark::Icon(icons::MASK_INTERSECT),
        },
    ];
    // ⚠ Taller and in the body face: this is the panel's primary control, and the compact
    // selector the HSL bands use read too small here (user's call, 2026-09-19). ⚠ An icon before
    // each label since 2026-09-29, which is what makes the four wrap into a 2 × 2 block at the
    // panel's width — `segmented::plan` balances the rows, never three and one.
    let font = egui::TextStyle::Body.resolve(ui.style());
    ui.add_enabled_ui(can_refine, |ui| {
        crate::segmented::segmented_marked(ui, mode, &items, MODE_HEIGHT, font)
    })
    .response
    .on_disabled_hover_text("Select a mask to refine it");
    let mut added: Option<MaskKind> = None;
    ui.horizontal(|ui| {
        let n = MaskKind::ALL.len() as f32;
        let w = (ui.available_width() - ui.spacing().item_spacing.x * (n - 1.0)) / n;
        for k in MaskKind::ALL {
            if mask_components::kind_button(ui, k, w).clicked() {
                added = Some(k);
            }
        }
    });
    if let Some(k) = added {
        match (*mode, s.selected_mask) {
            (Some(op), Some(i)) if i < s.edits.masks.len() => {
                s.edits.masks[i].refine.push(edit_model::Refinement {
                    op,
                    mask: k.template(),
                });
                s.selected_component = s.edits.masks[i].refine.len();
            }
            _ => {
                s.edits.masks.push(edit_model::MaskedAdjustment {
                    mask: k.template(),
                    refine: Vec::new(),
                    adjustments: edit_model::LocalAdjustments::default(),
                    disabled: false,
                    invert: false,
                });
                s.selected_mask = Some(s.edits.masks.len() - 1);
                s.selected_component = 0;
                // ⚠ A mask you just added shows its overlay: a brush has no strokes yet, and
                // the tint is how you see where you are painting.
                s.sync_mask_shown();
                if let Some(last) = s.mask_shown.last_mut() {
                    *last = true;
                }
            }
        }
        s.active_mask_handle = None;
        ed.discrete();
    }
}

/// `Show all` / `Hide all`, right-aligned under the list: every eye at once.
///
/// ⚠ `allocate_ui_with_layout`, never a bare `with_layout(right_to_left)` — the 2026-09-19
/// growth bug.
fn show_all_row(ui: &mut egui::Ui, s: &mut Session) {
    if s.edits.masks.is_empty() {
        return;
    }
    let all = s.all_masks_shown();
    let label = if all { "Hide all" } else { "Show all" };
    let mut clicked = false;
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), ui.spacing().interact_size.y + 4.0),
        egui::Layout::right_to_left(egui::Align::Center),
        |ui| {
            clicked = ui
                .small_button(label)
                .hover("Shows or hides every mask's overlay at once")
                .clicked();
        },
    );
    if clicked {
        s.set_all_masks_shown(!all);
    }
}

/// The mask rows, and the selected mask's component rows.
fn list(ui: &mut egui::Ui, s: &mut Session, ed: &mut Edited) {
    let mut remove: Option<usize> = None;
    let mut pick: Option<usize> = None;
    let mut rects = Vec::with_capacity(s.edits.masks.len());
    // The mask row being dragged, and whether it was released this frame.
    let mut drag: Option<(usize, bool)> = None;
    // A mask to duplicate from its row's right-click menu, and whether inverted (M10.12a).
    let mut dup: Option<(usize, bool)> = None;
    for i in 0..s.edits.masks.len() {
        let (kind, name) = {
            let m = &s.edits.masks[i];
            (
                MaskKind::of(&m.mask),
                format!("{} {}", m.mask.label(), i + 1),
            )
        };
        let picked = s.selected_mask == Some(i);
        let out = mask_components::row(
            ui,
            mask_components::RowSpec {
                glyph: mask_components::RowGlyph::Mask(kind),
                op: None,
                name: &name,
                meta: "",
                accessible: &name,
                selected: picked,
                current: false,
                deletable: true,
                delete_hover: "Deletes this mask",
                indent: 0.0,
                eye: Some(s.mask_shown.get(i).copied().unwrap_or(false)),
                hold: Some((
                    s.edits.masks[i].disabled,
                    "Disable",
                    "Switches this mask off everywhere, the export included, until you switch it \
                     back on",
                )),
                hover: mask_components::MASK_ROW_HOVER,
                draggable: true,
            },
        );
        out.response.context_menu(|ui| {
            if ui.button("Duplicate").clicked() {
                dup = Some((i, false));
                ui.close();
            }
            if ui.button("Duplicate and invert").clicked() {
                dup = Some((i, true));
                ui.close();
            }
        });
        rects.push(out.rect);
        if out.dragging || out.released {
            drag = Some((i, out.released));
        }
        if out.delete {
            remove = Some(i);
        } else if out.eye {
            if let Some(on) = s.mask_shown.get_mut(i) {
                *on = !*on;
            }
        } else if out.hold {
            // ⚠ AN EDIT, not a view mode (2026-09-20, user's call): off in the export too, so
            // it lives in the edits and is one history entry.
            s.edits.masks[i].disabled = !s.edits.masks[i].disabled;
            ed.discrete();
        } else if out.clicked {
            pick = Some(i);
        }
        // ⚠ An AI mask whose map has not landed yet says so on its own row, where the user is
        // looking, and keeps its sliders quiet until it has — the top bar's "Finding the
        // subject…" was thirteen seconds away from where the click happened (P-12).
        let waiting = match &s.edits.masks[i].mask {
            edit_model::Mask::Ai(a) => {
                let store = s.engine.ai_maps();
                let missing = store.read().expect("ai maps").get(&a.prompt).is_none();
                missing.then_some(match a.prompt {
                    edit_model::AiPrompt::Subject => "Finding the subject…",
                    edit_model::AiPrompt::Click { .. } => "Outlining what you clicked…",
                })
            }
            _ => None,
        };
        if let Some(text) = waiting {
            ui.horizontal(|ui| {
                ui.add_space(18.0);
                ui.spinner();
                crate::theme::hint(ui, text);
            });
        }
        if picked {
            ui.add_enabled_ui(waiting.is_none(), |ui| {
                mask_components::rows(ui, s, i, ed);
                mask_components::whole_invert(ui, s, i, ed);
            });
        }
    }
    // ⚠ Returns, so a same-frame pick, drag or delete cannot apply against shifted indices.
    if let Some((i, invert)) = dup {
        if mask_actions::duplicate(
            &mut s.edits.masks,
            &mut s.mask_shown,
            &mut s.selected_mask,
            i,
            invert,
        ) {
            s.selected_component = 0;
            s.active_mask_handle = None;
            ed.discrete();
        }
        return;
    }
    if let (Some((from, released)), None, None) = (drag, pick, remove) {
        if let Some((to, line)) = mask_components::drop_slot(ui, &rects) {
            if released {
                if mask_order::move_mask(
                    &mut s.edits.masks,
                    &mut s.mask_shown,
                    &mut s.selected_mask,
                    from,
                    to,
                ) {
                    s.active_mask_handle = None;
                    ed.discrete();
                }
            } else {
                mask_components::paint_drop_line(ui, ui.max_rect().x_range(), line);
            }
        }
    }
    if let Some(i) = pick {
        // Clicking the selected mask deselects it.
        s.selected_mask = if s.selected_mask == Some(i) {
            None
        } else {
            Some(i)
        };
        s.selected_component = 0;
        s.active_mask_handle = None;
    }
    if let Some(i) = remove {
        s.selected_mask =
            mask_tool::selection_after_delete(s.selected_mask, i, s.edits.masks.len());
        s.edits.masks.remove(i);
        if i < s.mask_shown.len() {
            s.mask_shown.remove(i);
        }
        s.active_mask_handle = None;
        ed.discrete();
    }
}

/// The selected component's own controls: its ranges, or the brush tool's settings, and
/// Invert.
///
/// ⚠ For a brush the sliders come first and the three checkboxes sit together, Tolerance
/// straight under the Auto mask it belongs to (user's call, 2026-09-19: a checkbox, three
/// sliders and two more checkboxes read as a jumble). Every explanation is a hover on its own
/// control, never a line of small print under the block.
fn shape_block(
    ui: &mut egui::Ui,
    shape: &mut edit_model::Mask,
    brush: &mut BrushTool<'_>,
    ed: &mut Edited,
) {
    // ⚠ The shape decides what "Size" and "Feather" mean in the hover (`slider_help::Scope`).
    ed.help_scope = match shape {
        edit_model::Mask::Brush(_) => rustroom_app::slider_help::Scope::Brush,
        edit_model::Mask::Radial(_) => rustroom_app::slider_help::Scope::Radial,
        edit_model::Mask::Luminance(_) => rustroom_app::slider_help::Scope::LuminanceRange,
        edit_model::Mask::Colour(_) => rustroom_app::slider_help::Scope::ColourRange,
        _ => rustroom_app::slider_help::Scope::Global,
    };
    shape_controls(ui, shape, brush, ed);
    ed.help_scope = rustroom_app::slider_help::Scope::Global;
}

/// [`shape_block`]'s body, inside its scope.
fn shape_controls(
    ui: &mut egui::Ui,
    shape: &mut edit_model::Mask,
    brush: &mut BrushTool<'_>,
    ed: &mut Edited,
) {
    if matches!(shape, edit_model::Mask::Brush(_)) {
        ed.slider(
            ui,
            &mut *brush.size,
            4.0..=300.0,
            "Size",
            rustroom_app::brush_tool::DEFAULTS.size,
        );
        ed.slider(
            ui,
            &mut *brush.hardness,
            0.0..=100.0,
            "Hardness",
            rustroom_app::brush_tool::DEFAULTS.hardness,
        );
        ed.slider(
            ui,
            &mut *brush.flow,
            1.0..=100.0,
            "Flow",
            rustroom_app::brush_tool::DEFAULTS.flow,
        );
        invert_box(ui, shape, ed);
        ui.checkbox(&mut *brush.erase, "Erase")
            .hover("Paints away from the mask instead of adding to it");
        ui.checkbox(&mut *brush.auto_mask, "Auto mask")
            .hover("Paints only where the colour matches the one under the brush's centre");
        ui.add_enabled_ui(*brush.auto_mask, |ui| {
            ed.slider(
                ui,
                &mut *brush.tolerance,
                0.0..=100.0,
                "Tolerance",
                rustroom_app::brush_tool::DEFAULTS.tolerance,
            );
        });
        return;
    }
    invert_box(ui, shape, ed);
    if let edit_model::Mask::Radial(r) = shape {
        ed.slider(
            ui,
            &mut r.feather,
            0.0..=100.0,
            "Feather",
            edit_model::RadialGradient::default().feather,
        );
    }
    if let edit_model::Mask::Luminance(r) = shape {
        ed.slider(
            ui,
            &mut r.low,
            0.0..=100.0,
            "Range low",
            edit_model::LuminanceRange::default().low,
        );
        ed.slider(
            ui,
            &mut r.high,
            0.0..=100.0,
            "Range high",
            edit_model::LuminanceRange::default().high,
        );
        ed.slider(
            ui,
            &mut r.feather,
            0.0..=100.0,
            "Feather",
            edit_model::LuminanceRange::default().feather,
        );
    }
    if let edit_model::Mask::Colour(r) = shape {
        ed.slider(
            ui,
            &mut r.low,
            0.0..=360.0,
            "Hue low",
            edit_model::ColourRange::default().low,
        );
        ed.slider(
            ui,
            &mut r.high,
            0.0..=360.0,
            "Hue high",
            edit_model::ColourRange::default().high,
        );
        ed.slider(
            ui,
            &mut r.feather,
            0.0..=100.0,
            "Feather",
            edit_model::ColourRange::default().feather,
        );
        ed.slider(
            ui,
            &mut r.min_chroma,
            0.0..=0.15,
            "Min chroma",
            edit_model::ColourRange::default().min_chroma,
        );
    }
    // M9.13: an AI mask's one control. The network's soft map is thresholded; Refine moves the
    // threshold, so the edge grows or shrinks without computing anything again.
    if let edit_model::Mask::Ai(a) = shape {
        ed.slider(ui, &mut a.threshold, -100.0..=100.0, "Refine", 0.0);
        if let edit_model::AiPrompt::Click { .. } = a.prompt {
            crate::theme::hint(ui, "Click the photo to choose what to select.");
        }
    }
}

/// The Invert checkbox, one for every kind.
fn invert_box(ui: &mut egui::Ui, shape: &mut edit_model::Mask, ed: &mut Edited) {
    if ui
        .checkbox(shape.invert_mut(), "Invert")
        .hover("Inverts the mask so it covers everything it did not")
        .changed()
    {
        ed.discrete();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_ui;

    /// The shipped `masks::show` against a scratch session, with the tool state the app holds.
    ///
    /// ⚠ One `egui::Context` for the harness's whole life, with animation off: the sections'
    /// open state lives in the context, so a test that opens Detail and then drags a slider in
    /// it needs both frames to see the same memory. The panel is driven **as it ships** — no
    /// re-declared widgets.
    struct Panel {
        ctx: egui::Context,
        s: Session,
        size: f32,
        hardness: f32,
        flow: f32,
        erase: bool,
        auto_mask: bool,
        tolerance: f32,
        channel: crate::develop::panel::adjustments::HslChannel,
        grade: crate::develop::panel::color_wheel::GradeRange,
        mode: Option<edit_model::MaskOp>,
    }

    impl Panel {
        fn new() -> Self {
            let ctx = test_ui::ctx();
            for theme in [egui::Theme::Dark, egui::Theme::Light] {
                ctx.style_mut_of(theme, |s| s.animation_time = 0.0);
            }
            let mut app = crate::test_app::for_test();
            crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
            Self {
                ctx,
                s: app.session.take().unwrap(),
                size: 40.0,
                hardness: 50.0,
                flow: 100.0,
                erase: false,
                auto_mask: false,
                tolerance: 35.0,
                channel: Default::default(),
                grade: Default::default(),
                mode: None,
            }
        }

        fn body(&mut self, ui: &mut egui::Ui, ed: &mut Edited) {
            show(
                ui,
                &mut self.s,
                MaskUi {
                    hsl: &mut self.channel,
                    grade: &mut self.grade,
                    add_mode: &mut self.mode,
                    curve_open: &mut true,
                },
                BrushTool {
                    size: &mut self.size,
                    hardness: &mut self.hardness,
                    flow: &mut self.flow,
                    erase: &mut self.erase,
                    auto_mask: &mut self.auto_mask,
                    tolerance: &mut self.tolerance,
                },
                ed,
            );
        }

        /// Click `label`; `ed` reports the frame the click landed in.
        fn click(&mut self, label: &str) -> Edited {
            let ctx = self.ctx.clone();
            let mut ed = Edited::default();
            test_ui::click(&ctx, label, |ui| {
                ed = Edited::default();
                self.body(ui, &mut ed);
            });
            ed
        }

        /// Add a mask: New mask mode, then the kind's button.
        fn add(&mut self, kind: MaskKind) -> Edited {
            self.mode = None;
            self.click(kind.menu_label())
        }

        /// Refine the selected mask: the op's segment, then the kind's button.
        fn refine(&mut self, op: edit_model::MaskOp, kind: MaskKind) -> Edited {
            self.click(op.label());
            assert_eq!(self.mode, Some(op), "the segment did not arm {op:?}");
            self.click(kind.menu_label())
        }

        fn labels(&mut self) -> Vec<String> {
            let ctx = self.ctx.clone();
            test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
                self.body(ui, &mut Edited::default());
            })
            .into_iter()
            .map(|(l, _)| l)
            .collect()
        }

        /// Drag the slider labelled `label` 120 pt to the right.
        fn drag(&mut self, label: &str) -> Edited {
            let from = self.centre(label);
            self.pointer_path(from, from + egui::vec2(120.0, 0.0))
        }

        /// Press on the row labelled `from` and release on the upper (`after == false`) or lower
        /// half of the row labelled `to` — "before it" and "after it".
        fn drag_row(&mut self, from: &str, to: &str, after: bool) -> Edited {
            let start = self.centre(from);
            let quarter = mask_components::ROW_HEIGHT / 4.0;
            let end = self.centre(to) + egui::vec2(0.0, if after { quarter } else { -quarter });
            self.pointer_path(start, end)
        }

        /// The centre of the widget labelled `label`.
        fn centre(&mut self, label: &str) -> egui::Pos2 {
            let ctx = self.ctx.clone();
            let all = test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
                self.body(ui, &mut Edited::default())
            });
            all.iter()
                .find(|(l, _)| l == label)
                .map(|(_, p)| *p)
                .unwrap_or_else(|| panic!("no widget labelled {label:?} was drawn: {all:?}"))
        }

        /// Press at `from`, move to `to`, release there: five frames, `ed` folded over them.
        fn pointer_path(&mut self, from: egui::Pos2, to: egui::Pos2) -> Edited {
            let ctx = self.ctx.clone();
            let press = |pos, pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            };
            let mut ed = Edited::default();
            for events in [
                vec![egui::Event::PointerMoved(from)],
                vec![press(from, true)],
                vec![egui::Event::PointerMoved(to)],
                vec![press(to, false)],
                Vec::new(),
            ] {
                let mut pass = Edited::default();
                let _ = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(test_ui::screen()),
                        events,
                        ..Default::default()
                    },
                    |ui| self.body(ui, &mut pass),
                );
                ed.changed |= pass.changed;
                ed.settled |= pass.settled;
            }
            ed
        }

        /// Every centre of a widget labelled `label`.
        fn centres(&mut self, label: &str) -> Vec<egui::Pos2> {
            let ctx = self.ctx.clone();
            test_ui::labelled_centres(&ctx, Vec::new(), |ui| self.body(ui, &mut Edited::default()))
                .into_iter()
                .filter(|(l, _)| l == label)
                .map(|(_, p)| p)
                .collect()
        }

        /// Right-click at `at`; `ed` folded over the frames.
        fn right_click_at(&mut self, at: egui::Pos2) -> Edited {
            let ctx = self.ctx.clone();
            let mut ed = Edited::default();
            test_ui::right_click_at(&ctx, at, |ui| {
                let mut pass = Edited::default();
                self.body(ui, &mut pass);
                ed.changed |= pass.changed;
                ed.settled |= pass.settled;
            });
            ed
        }

        /// Right-click the row labelled `row`, then click its menu's `item`.
        ///
        /// ⚠ The op items share their labels with the mode chips ("Add", "Subtract", "Intersect"),
        /// so the item is the match the menu ADDED — a centre not drawn before the right-click.
        fn menu(&mut self, row: &str, item: &str) -> Edited {
            let before = self.centres(item);
            let at = self.centre(row);
            self.right_click_at(at);
            let at = self
                .centres(item)
                .into_iter()
                .find(|p| !before.contains(p))
                .unwrap_or_else(|| panic!("right-clicking {row:?} drew no {item:?}"));
            let ctx = self.ctx.clone();
            let mut ed = Edited::default();
            test_ui::click_at(&ctx, at, false, |ui| {
                let mut pass = Edited::default();
                self.body(ui, &mut pass);
                ed.changed |= pass.changed;
                ed.settled |= pass.settled;
            });
            ed
        }
    }

    #[test]
    fn a_kind_button_adds_a_mask_in_new_mode_and_a_component_in_an_op_mode() {
        // ⚠⚠ **Both halves, and the pair is what makes this mean anything.** A version that
        // always appended a component would pass a "does Subtract refine?" test on its own, and
        // a version that always added a mask would pass the New-mask half.
        let mut p = Panel::new();
        p.add(MaskKind::Gradient);
        assert_eq!(p.s.edits.masks.len(), 1, "fixture: one mask to refine");
        p.add(MaskKind::Radial);
        assert_eq!(p.s.edits.masks.len(), 2, "New mask mode adds a mask");
        assert!(p.s.edits.masks[1].refine.is_empty());

        p.refine(edit_model::MaskOp::Subtract, MaskKind::Radial);
        assert_eq!(p.s.edits.masks.len(), 2, "a refinement adds no mask");
        assert_eq!(p.s.edits.masks[1].refine.len(), 1);
        assert_eq!(
            p.s.edits.masks[1].refine[0].op,
            edit_model::MaskOp::Subtract,
            "the armed op is what the refinement carries"
        );
        assert_eq!(
            p.s.selected_component, 1,
            "the new shape becomes the one the sliders edit"
        );
    }

    #[test]
    fn the_op_modes_are_inert_without_a_selection() {
        // ⚠ The belt to the disabled control's braces: with nothing selected an op cannot be
        // armed, and a kind click still adds a mask — recoverable in one click — rather than
        // dropping the click. And deselecting disarms: a mode that outlives its selection is
        // the invisible state option C was allowed to keep only while it stays visible.
        let mut p = Panel::new();
        p.click("Subtract");
        assert_eq!(p.mode, None, "an op was armed with nothing to refine");
        p.click("Radial");
        assert_eq!(p.s.edits.masks.len(), 1, "the click must still add a mask");
        assert!(p.s.edits.masks[0].refine.is_empty());

        p.click("Subtract");
        assert_eq!(p.mode, Some(edit_model::MaskOp::Subtract));
        p.click("Radial 1"); // deselect
        assert_eq!(p.s.selected_mask, None);
        p.labels();
        assert_eq!(p.mode, None, "deselecting must disarm the op");
    }

    #[test]
    fn the_shape_controls_edit_the_selected_component_not_the_base() {
        // ⚠ The property the whole composite UI rests on: the Feather slider follows
        // `component_index`, not the base.
        let mut p = Panel::new();
        p.add(MaskKind::Radial);
        p.refine(edit_model::MaskOp::Subtract, MaskKind::Radial);
        let base_before = match &p.s.edits.masks[0].mask {
            edit_model::Mask::Radial(r) => r.feather,
            other => panic!("expected radial, got {other:?}"),
        };
        p.drag("Feather");
        let base_after = match &p.s.edits.masks[0].mask {
            edit_model::Mask::Radial(r) => r.feather,
            other => panic!("expected radial, got {other:?}"),
        };
        let comp = match &p.s.edits.masks[0].refine[0].mask {
            edit_model::Mask::Radial(r) => r.feather,
            other => panic!("expected radial, got {other:?}"),
        };
        assert_eq!(base_after, base_before, "the base must be untouched");
        assert_ne!(comp, 50.0, "the refinement's feather must have moved");
    }

    #[test]
    fn each_menu_item_adds_its_own_kind_of_mask() {
        // ⚠ Every kind, because a menu wired to one template would pass a test that only ever
        // picked Gradient.
        for k in MaskKind::ALL {
            let mut p = Panel::new();
            p.add(k);
            assert_eq!(p.s.edits.masks.len(), 1, "{k:?} added no mask");
            assert_eq!(MaskKind::of(&p.s.edits.masks[0].mask), k, "the wrong kind");
        }
    }

    #[test]
    fn adding_a_mask_selects_it_and_counts_as_an_edit() {
        let mut p = Panel::new();
        let ed = p.add(MaskKind::Gradient);
        assert_eq!(p.s.selected_mask, Some(0));
        assert_eq!(p.s.selected_component, 0);
        assert!(ed.changed && ed.settled, "one pick, one history entry");
    }

    #[test]
    fn a_second_mask_is_selected_over_the_first() {
        let mut p = Panel::new();
        p.add(MaskKind::Gradient);
        p.add(MaskKind::Radial);
        assert_eq!(p.s.selected_mask, Some(1));
    }

    #[test]
    fn deleting_the_selected_mask_clears_the_selection_and_the_drag_handle() {
        let mut p = Panel::new();
        p.add(MaskKind::Gradient);
        p.s.active_mask_handle = Some(mask_tool::Handle::Center);
        let ed = p.click("Delete Gradient 1");
        assert!(p.s.edits.masks.is_empty(), "the mask should be gone");
        assert!(ed.settled, "a delete is one history entry");
        assert_eq!(p.s.selected_mask, None, "and nothing should be selected");
        assert!(
            p.s.active_mask_handle.is_none(),
            "and no drag handle left behind"
        );
    }

    #[test]
    fn the_eye_shows_one_masks_overlay_and_show_all_flips_every_eye() {
        // ⚠ Both directions for the eye, and both labels for the row button: a toggle stuck
        // on, or a button that only ever showed, would each pass a one-sided test.
        let mut p = Panel::new();
        p.add(MaskKind::Gradient);
        assert_eq!(
            p.s.mask_shown,
            vec![true],
            "a mask you add shows its overlay"
        );
        p.click("Show Gradient 1");
        assert_eq!(p.s.mask_shown, vec![false], "the eye did not hide it");
        p.click("Show Gradient 1");
        assert_eq!(p.s.mask_shown, vec![true], "nor show it again");
        p.add(MaskKind::Radial);
        p.click("Show Gradient 1");
        assert_eq!(p.s.mask_shown, vec![false, true]);
        p.click("Show all");
        assert_eq!(p.s.mask_shown, vec![true, true]);
        p.click("Hide all");
        assert_eq!(p.s.mask_shown, vec![false, false]);
    }

    #[test]
    fn deleting_a_mask_drops_its_eye_with_it() {
        let mut p = Panel::new();
        p.add(MaskKind::Gradient);
        p.add(MaskKind::Radial);
        p.click("Show Gradient 1"); // off, on
        assert_eq!(p.s.mask_shown, vec![false, true]);
        p.click("Delete Gradient 1");
        assert_eq!(p.s.edits.masks.len(), 1);
        assert_eq!(p.s.mask_shown, vec![true], "the survivor keeps its own eye");
    }

    #[test]
    fn the_disable_toggle_is_an_edit_that_switches_the_mask_off_and_on() {
        // ⚠ Both directions, and it is an edit: one history entry each way, and the flag lives
        // in the edits so the export and the sidecar see it.
        let mut p = Panel::new();
        p.add(MaskKind::Gradient);
        let ed = p.click("Disable Gradient 1");
        assert!(
            p.s.edits.masks[0].disabled,
            "the toggle did not switch the mask off"
        );
        assert!(ed.settled, "switching a mask off is one history entry");
        let ed = p.click("Disable Gradient 1");
        assert!(!p.s.edits.masks[0].disabled, "nor back on");
        assert!(ed.settled);
    }

    #[test]
    fn the_masks_reset_removes_every_mask_and_clears_the_selection() {
        let mut p = Panel::new();
        p.add(MaskKind::Gradient);
        p.add(MaskKind::Brush);
        p.s.edits.exposure = 1.5;
        // ⚠ The MASKS header's reset is the only one drawn: no adjustment section is modified.
        let ed = p.click("reset");
        assert!(p.s.edits.masks.is_empty(), "the masks survived the reset");
        assert!(p.s.mask_shown.is_empty(), "the eyes survived the reset");
        assert_eq!(p.s.selected_mask, None);
        assert_eq!(
            p.s.edits.exposure, 1.5,
            "the global adjustments must survive"
        );
        assert!(ed.settled);
    }

    #[test]
    fn the_tail_native_sliders_edit_the_selected_mask() {
        // ⚠ Texture and Clarity sit in Tone (open); Amount in Detail and Red in Color start
        // collapsed and are opened first, through their own headers.
        for (label, section, read) in [
            (
                "Texture",
                None,
                (|a: &edit_model::LocalAdjustments| a.local_contrast.texture)
                    as fn(&edit_model::LocalAdjustments) -> f32,
            ),
            ("Clarity", None, |a| a.local_contrast.clarity),
            ("Amount", Some("Detail"), |a| a.sharpening.amount),
            ("Red", Some("Color"), |a| a.hsl.bands[0].hue),
        ] {
            let mut p = Panel::new();
            p.add(MaskKind::Gradient);
            if let Some(sec) = section {
                p.click(sec);
            }
            let before = read(&p.s.edits.masks[0].adjustments);
            let ed = p.drag(label);
            let after = read(&p.s.edits.masks[0].adjustments);
            assert!(ed.changed, "dragging {label} reported no edit");
            assert_ne!(
                after, before,
                "the {label} slider is not wired to the selected mask"
            );
        }
    }

    #[test]
    fn a_selected_luminance_mask_shows_its_range_and_a_radial_does_not() {
        let mut p = Panel::new();
        p.add(MaskKind::Luminance);
        let shown = p.labels();
        for want in ["Range low", "Range high", "Feather"] {
            assert!(
                shown.iter().any(|l| l == want),
                "a selected luminance mask must offer {want}; got {shown:?}"
            );
        }

        let mut p = Panel::new();
        p.add(MaskKind::Radial);
        let shown = p.labels();
        assert!(
            !shown.iter().any(|l| l == "Range low"),
            "a radial mask must not offer a luminance range; got {shown:?}"
        );
        assert!(
            shown.iter().any(|l| l == "Feather"),
            "but it keeps its own feather; got {shown:?}"
        );
    }

    #[test]
    fn the_range_sliders_are_wired_to_the_mask() {
        let mut p = Panel::new();
        p.add(MaskKind::Luminance);
        let before = match p.s.edits.masks[0].mask {
            edit_model::Mask::Luminance(r) => r,
            ref other => panic!("expected a luminance mask, got {other:?}"),
        };
        let ed = p.drag("Range high");
        let after = match p.s.edits.masks[0].mask {
            edit_model::Mask::Luminance(r) => r,
            ref other => panic!("expected a luminance mask, got {other:?}"),
        };
        assert_ne!(
            after.high, before.high,
            "the slider must edit the mask itself, not a copy of it"
        );
        assert_eq!(after.low, before.low, "and only the one it names");
        assert!(ed.changed, "and it must re-request the preview");
    }

    #[test]
    fn a_selected_colour_mask_shows_its_hue_band_and_a_luminance_one_does_not() {
        let mut p = Panel::new();
        p.add(MaskKind::Colour);
        let shown = p.labels();
        for want in ["Hue low", "Hue high", "Feather", "Min chroma"] {
            assert!(
                shown.iter().any(|l| l == want),
                "a selected colour mask must offer {want}; got {shown:?}"
            );
        }
        assert!(
            !shown.iter().any(|l| l == "Range low"),
            "and not the luminance range's tone band; got {shown:?}"
        );

        let mut p = Panel::new();
        p.add(MaskKind::Luminance);
        let shown = p.labels();
        assert!(
            !shown.iter().any(|l| l == "Hue low" || l == "Min chroma"),
            "a luminance mask must not offer a hue band; got {shown:?}"
        );
    }

    #[test]
    fn the_hue_sliders_are_wired_to_the_mask_and_settle_into_one_edit() {
        let mut p = Panel::new();
        p.add(MaskKind::Colour);
        let before = match p.s.edits.masks[0].mask {
            edit_model::Mask::Colour(r) => r,
            ref other => panic!("expected a colour mask, got {other:?}"),
        };
        let ed = p.drag("Hue high");
        let after = match p.s.edits.masks[0].mask {
            edit_model::Mask::Colour(r) => r,
            ref other => panic!("expected a colour mask, got {other:?}"),
        };
        assert_ne!(
            after.high, before.high,
            "the slider must edit the mask itself, not a copy of it"
        );
        assert_eq!(after.low, before.low, "and only the one it names");
        assert!(ed.changed, "and it must re-request the preview");
        assert!(ed.settled, "one drag is one undo step");
    }

    #[test]
    fn clicking_the_selected_mask_deselects_it() {
        let mut p = Panel::new();
        p.add(MaskKind::Gradient);
        let name = format!("{} 1", p.s.edits.masks[0].mask.label());
        p.click(&name);
        assert_eq!(p.s.selected_mask, None);
        assert_eq!(p.s.edits.masks.len(), 1, "deselecting must not delete it");
    }

    /// Gradient, then −Radial, then +Radial — the last selected.
    fn composite_panel() -> Panel {
        let mut p = Panel::new();
        p.add(MaskKind::Gradient);
        p.refine(edit_model::MaskOp::Subtract, MaskKind::Radial);
        p.refine(edit_model::MaskOp::Add, MaskKind::Radial);
        assert_eq!(
            p.s.selected_component, 2,
            "fixture: +Radial is being edited"
        );
        p
    }

    #[test]
    fn dragging_a_component_reorders_the_composite_as_one_edit() {
        let mut p = composite_panel();
        let ed = p.drag_row("Add Radial", "Subtract Radial", false);
        let m = &p.s.edits.masks[0];
        assert_eq!(m.refine[0].op, edit_model::MaskOp::Add, "+Radial moved up");
        assert_eq!(m.refine[1].op, edit_model::MaskOp::Subtract);
        assert_eq!(
            p.s.selected_component, 1,
            "the edited shape followed its row"
        );
        assert!(ed.changed && ed.settled, "one drop, one history entry");
    }

    /// ⚠ Slot 0 is reached through the MASK row's lower half: "Gradient" names both the base
    /// component row and the kind button, so the harness cannot aim at it by name.
    ///
    /// ⚠ A "nothing changed" test also passes when the drag went nowhere — so its twin,
    /// `an_add_dropped_on_top_becomes_the_base`, aims at the same point and must change the mask.
    #[test]
    fn a_subtract_is_never_dropped_on_top() {
        let mut p = composite_panel();
        let before = p.s.edits.masks.clone();
        let ed = p.drag_row("Subtract Radial", "Gradient 1", true);
        assert_eq!(p.s.edits.masks, before, "a Subtract reached the top");
        assert!(!ed.settled, "a refused drop is no history entry");
    }

    #[test]
    fn an_add_dropped_on_top_becomes_the_base() {
        let mut p = composite_panel();
        let ed = p.drag_row("Add Radial", "Gradient 1", true);
        let m = &p.s.edits.masks[0];
        assert!(
            matches!(m.mask, edit_model::Mask::Radial(_)),
            "the Add is the base"
        );
        assert!(matches!(m.refine[0].mask, edit_model::Mask::Linear(_)));
        assert_eq!(m.refine[0].op, edit_model::MaskOp::Add, "the old base adds");
        assert_eq!(m.refine[1].op, edit_model::MaskOp::Subtract);
        assert_eq!(p.s.selected_component, 0);
        assert!(ed.settled);
    }

    #[test]
    fn dragging_a_mask_reorders_the_masks_and_their_eyes() {
        let mut p = Panel::new();
        p.add(MaskKind::Gradient);
        p.add(MaskKind::Radial);
        p.s.mask_shown = vec![true, false];
        assert_eq!(p.s.selected_mask, Some(1), "fixture: Radial 2 selected");
        let ed = p.drag_row("Radial 2", "Gradient 1", false);
        assert!(matches!(
            p.s.edits.masks[0].mask,
            edit_model::Mask::Radial(_)
        ));
        assert!(matches!(
            p.s.edits.masks[1].mask,
            edit_model::Mask::Linear(_)
        ));
        assert_eq!(p.s.mask_shown, [false, true], "the eyes stayed behind");
        assert_eq!(p.s.selected_mask, Some(0), "the selection stayed behind");
        assert!(ed.changed && ed.settled);
        let labels = p.labels();
        assert!(labels.iter().any(|l| l == "Radial 1"), "{labels:?}");
    }

    #[test]
    fn a_mask_dropped_where_it_started_is_no_edit() {
        let mut p = Panel::new();
        p.add(MaskKind::Gradient);
        p.add(MaskKind::Radial);
        let before = p.s.edits.masks.clone();
        let ed = p.drag_row("Gradient 1", "Radial 2", false);
        assert_eq!(p.s.edits.masks, before);
        assert!(!ed.settled);
    }

    #[test]
    fn a_component_rows_menu_changes_its_op_as_one_edit() {
        let mut p = composite_panel();
        let ed = p.menu("Subtract Radial", "Intersect");
        assert!(ed.settled, "an op change is one history entry");
        assert_eq!(
            p.s.edits.masks[0].refine[0].op,
            edit_model::MaskOp::Intersect
        );
        assert_eq!(
            p.s.edits.masks[0].refine[1].op,
            edit_model::MaskOp::Add,
            "only that row"
        );
        assert!(
            p.labels().contains(&"Intersect Radial".to_string()),
            "the row renamed itself"
        );
    }

    #[test]
    fn choosing_the_current_op_is_no_edit() {
        let mut p = composite_panel();
        let ed = p.menu("Subtract Radial", "Subtract");
        assert!(!ed.settled, "nothing changed, so no history entry");
        assert_eq!(
            p.s.edits.masks[0].refine[0].op,
            edit_model::MaskOp::Subtract
        );
    }

    /// Review Focus 2: the base is the Add everything folds onto, so its row offers no op.
    #[test]
    fn the_base_row_offers_no_op() {
        let mut p = composite_panel();
        let chips = p.centres("Intersect").len();
        // ⚠ "Gradient" names both the base row and the kind button; the row is the LOWER one.
        let base = p
            .centres("Gradient")
            .into_iter()
            .max_by(|a, b| a.y.total_cmp(&b.y))
            .expect("the base row is drawn");
        p.right_click_at(base);
        assert_eq!(
            p.centres("Intersect").len(),
            chips,
            "the base row opened an op menu"
        );
        // Positive control: the same probe on a refinement row DOES add the item.
        let at = p.centre("Subtract Radial");
        p.right_click_at(at);
        assert_eq!(
            p.centres("Intersect").len(),
            chips + 1,
            "fixture: a refinement's menu opens"
        );
    }

    #[test]
    fn duplicate_puts_a_selected_copy_below_as_one_edit() {
        let mut p = composite_panel();
        let ed = p.menu("Gradient 1", "Duplicate");
        assert!(ed.settled);
        assert_eq!(p.s.edits.masks.len(), 2);
        assert_eq!(p.s.edits.masks[1], p.s.edits.masks[0]);
        assert_eq!(p.s.selected_mask, Some(1));
        assert_eq!(p.s.selected_component, 0, "the copy's first row");
        assert_eq!(
            p.s.mask_shown.get(1),
            Some(&true),
            "the copy shows its overlay"
        );
    }

    #[test]
    fn duplicate_and_invert_inverts_the_copy_only() {
        let mut p = composite_panel();
        p.menu("Gradient 1", "Duplicate and invert");
        assert!(!p.s.edits.masks[0].invert, "the original is untouched");
        assert!(
            p.s.edits.masks[1].invert,
            "a composite's copy is inverted as a whole"
        );
    }

    /// Review Focus 1: a right-click is not a click — no selection change, no drag, no edit.
    #[test]
    fn a_right_click_opens_the_menu_without_selecting() {
        let mut p = Panel::new();
        p.add(MaskKind::Radial);
        p.click("Radial 1");
        assert_eq!(p.s.selected_mask, None, "fixture: nothing selected");
        let at = p.centre("Radial 1");
        let ed = p.right_click_at(at);
        assert_eq!(p.s.selected_mask, None, "a right-click selected the mask");
        assert!(!ed.changed, "a right-click edited something");
        assert!(
            !p.centres("Duplicate").is_empty(),
            "fixture: the menu opened"
        );
    }

    #[test]
    fn invert_mask_is_drawn_for_a_composite_only_and_is_one_edit() {
        let mut p = Panel::new();
        p.add(MaskKind::Radial);
        assert!(
            !p.labels().contains(&"Invert mask".to_string()),
            "a one-shape mask has only its shape's Invert"
        );
        p.refine(edit_model::MaskOp::Subtract, MaskKind::Radial);
        let ed = p.click("Invert mask");
        assert!(ed.settled);
        assert!(p.s.edits.masks[0].invert);
    }
}
