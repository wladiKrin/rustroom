//! The Masks panel: the mask list with its Add menu and Refine row, the Shape section, and the
//! selected mask's adjustments.
//!
//! ⚠ Shown only while the **Masks** tool is selected on the rail, and it is then the *whole*
//! panel — the global adjustment sliders are not visible alongside it. A mask carries its own
//! Temp…Sharpening, so nothing here needs them.
//!
//! ⚠ The layout is option A of the 2026-09-19 canvas ("menus, not modes"): one `Add mask`
//! menu, painted rows, a Refine row of op menus, then SHAPE and the four adjustment sections
//! the Edit panel also uses — one layout learned once.

use crate::develop::panel::{mask_components, section, Edited};
use crate::{theme, Session};
use eframe::egui;
use rustroom_app::mask_kind::MaskKind;
use rustroom_app::mask_tool;

/// Brush tool settings, which live on the app rather than on the mask.
///
/// ⚠ They are the *tool's* state and apply to the next stroke, exactly as a brush size does
/// in any editor. Each stroke records its own copy at the moment it is drawn.
///
/// ⚠ Seven `&mut` to *disjoint* fields of `RustroomApp`, which is what replaced the previous
/// copy-into-locals-and-write-back workaround: that existed only because a closure capturing
/// `app` while `s` also borrowed it is `E0499`. Explicit parameters have no such problem.
pub struct BrushTool<'a> {
    pub size: &'a mut f32,
    pub hardness: &'a mut f32,
    pub flow: &'a mut f32,
    pub erase: &'a mut bool,
    /// The overlay: tints where the selected mask reaches. Toggled in the MASKS header and
    /// from the View menu, the same flag.
    pub show_tint: &'a mut bool,
    pub auto_mask: &'a mut bool,
    pub tolerance: &'a mut f32,
}

/// The panel state a mask's controls need that lives on `RustroomApp` rather than on the mask.
///
/// ⚠ Two `&mut` to **disjoint** fields, which is what makes this legal where a closure capturing
/// `app` would be `E0499` — the reasoning `BrushTool` already carries.
pub struct MaskUi<'a> {
    pub hsl: &'a mut super::adjustments::HslChannel,
    pub grade: &'a mut super::color_wheel::GradeRange,
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
    } = ui_state;
    // Hoisted above every `&mut s.*` below: `Illuminant` is `Copy`, so reading it here costs
    // nothing and keeps the later mutable borrows disjoint.
    let baseline = s.wb_baseline;

    // ── MASKS ──
    // ⚠ `reset` here is the old "Remove all masks", and `overlay` the old "Show mask" checkbox;
    // both belong to the list as a whole, so they sit on its header.
    let mut out = section::header(
        ui,
        ui.make_persistent_id(("mask", "Masks")),
        true,
        section::Header::new("Masks")
            .modified(!s.edits.masks.is_empty())
            .resettable(true)
            .toggle("overlay", *brush.show_tint),
    );
    if out.toggled {
        *brush.show_tint = !*brush.show_tint;
    }
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
        ed.discrete();
    }
    out.state.show_body_unindented(ui, |ui| {
        add_row(ui, s, ed);
        list(ui, s, ed);
        if s.selected_mask.is_none() {
            theme::hint(
                ui,
                "Add a mask, then drag its handles on the image. A gradient's two outer lines \
                 mark where the effect starts and reaches full strength; a radial's ellipse marks \
                 where it falls to nothing.",
            );
        }
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
        out.state
            .show_body_unindented(ui, |ui| shape_block(ui, shape, &mut brush, ed));
    }

    super::mask_adjust::show(ui, &mut m.adjustments, baseline, mask_hsl, mask_grade, ed);
}

/// The `Add mask` menu, right-aligned in a row pinned to its width.
///
/// ⚠ `allocate_ui_with_layout`, never a bare `with_layout(right_to_left)`: inside this panel
/// that widened the panel every frame (2026-09-19, `theme::sub_label_with`'s note).
fn add_row(ui: &mut egui::Ui, s: &mut Session, ed: &mut Edited) {
    let mut added: Option<MaskKind> = None;
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), ui.spacing().interact_size.y + 4.0),
        egui::Layout::right_to_left(egui::Align::Center),
        |ui| {
            ui.menu_button("Add mask", |ui| {
                for k in MaskKind::ALL {
                    if ui.button(k.menu_label()).clicked() {
                        added = Some(k);
                        ui.close();
                    }
                }
            })
            .response
            .on_hover_text("A new mask of one of five kinds; the Refine row adds shapes to one");
        },
    );
    if let Some(k) = added {
        s.edits.masks.push(edit_model::MaskedAdjustment {
            mask: k.template(),
            refine: Vec::new(),
            adjustments: edit_model::LocalAdjustments::default(),
        });
        s.selected_mask = Some(s.edits.masks.len() - 1);
        s.selected_component = 0;
        s.active_mask_handle = None;
        ed.discrete();
    }
}

/// The mask rows, the selected mask's component rows and its Refine row.
fn list(ui: &mut egui::Ui, s: &mut Session, ed: &mut Edited) {
    let mut remove: Option<usize> = None;
    let mut pick: Option<usize> = None;
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
                kind,
                op: None,
                name: &name,
                accessible: &name,
                selected: picked,
                current: false,
                deletable: true,
                indent: 0.0,
            },
        );
        if out.delete {
            remove = Some(i);
        } else if out.clicked {
            pick = Some(i);
        }
        if picked {
            mask_components::rows(ui, s, i, ed);
            mask_components::refine_row(ui, s, i, ed);
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
        s.active_mask_handle = None;
        ed.discrete();
    }
}

/// The selected component's own controls: Invert, its ranges, or the brush tool's settings.
fn shape_block(
    ui: &mut egui::Ui,
    shape: &mut edit_model::Mask,
    brush: &mut BrushTool<'_>,
    ed: &mut Edited,
) {
    if ui.checkbox(shape.invert_mut(), "Invert").changed() {
        ed.discrete();
    }
    if let edit_model::Mask::Radial(r) = shape {
        ed.slider(ui, &mut r.feather, 0.0..=100.0, "Feather");
    }
    if let edit_model::Mask::Luminance(r) = shape {
        ed.slider(ui, &mut r.low, 0.0..=100.0, "Range low");
        ed.slider(ui, &mut r.high, 0.0..=100.0, "Range high");
        ed.slider(ui, &mut r.feather, 0.0..=100.0, "Feather");
        theme::hint(
            ui,
            "Selects tones by brightness — 0 is black, 100 is white, the same scale as the \
             histogram. Feather widens the range with a soft shoulder either side.",
        );
    }
    if let edit_model::Mask::Colour(r) = shape {
        ed.slider(ui, &mut r.low, 0.0..=360.0, "Hue low");
        ed.slider(ui, &mut r.high, 0.0..=360.0, "Hue high");
        ed.slider(ui, &mut r.feather, 0.0..=100.0, "Feather");
        ed.slider(ui, &mut r.min_chroma, 0.0..=0.15, "Min chroma");
        theme::hint(
            ui,
            "Selects colours by hue, in degrees around the wheel — and the band wraps, so 340 \
             to 20 selects the reds either side of zero. Min chroma is what stops a near-neutral \
             sky being selected: below it a pixel has no reliable hue, only sensor noise.",
        );
    }
    if matches!(shape, edit_model::Mask::Brush(_)) {
        ed.slider(ui, &mut *brush.size, 4.0..=300.0, "Size");
        ed.slider(ui, &mut *brush.hardness, 0.0..=100.0, "Hardness");
        ed.slider(ui, &mut *brush.flow, 1.0..=100.0, "Flow");
        ui.checkbox(&mut *brush.erase, "Erase");
        ui.checkbox(&mut *brush.auto_mask, "Auto mask");
        ui.add_enabled_ui(*brush.auto_mask, |ui| {
            ed.slider(ui, &mut *brush.tolerance, 0.0..=100.0, "Tolerance");
        });
        theme::hint(
            ui,
            "Drag to paint. Hardness shapes the edge; Flow is strength, and repeated passes \
             build up. Auto mask paints only what matches the pixel under the brush centre, and \
             applies to new strokes.",
        );
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
        tint: bool,
        auto_mask: bool,
        tolerance: f32,
        channel: crate::develop::panel::adjustments::HslChannel,
        grade: crate::develop::panel::color_wheel::GradeRange,
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
                tint: true,
                auto_mask: false,
                tolerance: 35.0,
                channel: Default::default(),
                grade: Default::default(),
            }
        }

        fn body(&mut self, ui: &mut egui::Ui, ed: &mut Edited) {
            show(
                ui,
                &mut self.s,
                MaskUi {
                    hsl: &mut self.channel,
                    grade: &mut self.grade,
                },
                BrushTool {
                    size: &mut self.size,
                    hardness: &mut self.hardness,
                    flow: &mut self.flow,
                    erase: &mut self.erase,
                    show_tint: &mut self.tint,
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

        /// Add a mask through the menu: `Add mask`, then the kind's item.
        fn add(&mut self, kind: MaskKind) -> Edited {
            self.click("Add mask");
            self.click(kind.menu_label())
        }

        /// Refine the selected mask through an op's menu.
        fn refine(&mut self, op: edit_model::MaskOp, kind: MaskKind) -> Edited {
            self.click(op.label());
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
            let ctx = self.ctx.clone();
            let from = test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
                self.body(ui, &mut Edited::default())
            })
            .into_iter()
            .find(|(l, _)| l == label)
            .map(|(_, p)| p)
            .unwrap_or_else(|| panic!("no widget labelled {label:?} was drawn"));
            let to = from + egui::vec2(120.0, 0.0);
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
    }

    #[test]
    fn the_add_menu_adds_a_mask_and_an_op_menu_refines_the_selected_one() {
        // ⚠⚠ **Both halves, and the pair is what makes this mean anything.** A version that
        // always appended a component would pass a "does Subtract refine?" test on its own, and
        // a version that always added a mask would pass the Add-mask half.
        let mut p = Panel::new();
        p.add(MaskKind::Gradient);
        assert_eq!(p.s.edits.masks.len(), 1, "fixture: one mask to refine");
        p.add(MaskKind::Radial);
        assert_eq!(p.s.edits.masks.len(), 2, "Add mask adds a mask");
        assert!(p.s.edits.masks[1].refine.is_empty());

        p.refine(edit_model::MaskOp::Subtract, MaskKind::Radial);
        assert_eq!(p.s.edits.masks.len(), 2, "a refinement adds no mask");
        assert_eq!(p.s.edits.masks[1].refine.len(), 1);
        assert_eq!(
            p.s.edits.masks[1].refine[0].op,
            edit_model::MaskOp::Subtract,
            "the menu's op is what the refinement carries"
        );
        assert_eq!(
            p.s.selected_component, 1,
            "the new shape becomes the one the sliders edit"
        );
    }

    #[test]
    fn the_refine_row_exists_only_for_a_selected_mask() {
        // ⚠ Replaces the disabled chips: with nothing selected there is nothing to refine, and
        // the row is absent rather than greyed.
        let mut p = Panel::new();
        assert!(
            !p.labels().iter().any(|l| l == "Subtract"),
            "a Refine row with no mask"
        );
        p.add(MaskKind::Gradient);
        assert!(p.labels().iter().any(|l| l == "Subtract"));
        p.click("Gradient 1"); // deselect
        assert_eq!(p.s.selected_mask, None);
        assert!(
            !p.labels().iter().any(|l| l == "Subtract"),
            "a Refine row with nothing selected"
        );
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
        let ed = p.click("×");
        assert!(p.s.edits.masks.is_empty(), "the mask should be gone");
        assert!(ed.settled, "a delete is one history entry");
        assert_eq!(p.s.selected_mask, None, "and nothing should be selected");
        assert!(
            p.s.active_mask_handle.is_none(),
            "and no drag handle left behind"
        );
    }

    #[test]
    fn the_overlay_toggle_flips_show_tint() {
        // ⚠ Both directions: a toggle stuck on would pass a one-way test.
        let mut p = Panel::new();
        assert!(p.tint);
        p.click("overlay");
        assert!(!p.tint, "the header toggle did not turn the overlay off");
        p.click("overlay");
        assert!(p.tint, "nor back on");
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
}
