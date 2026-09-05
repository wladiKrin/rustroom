//! The Masks section: add, select, delete, shape properties, brush settings, adjustments.
//!
//! ⚠ Shown only while the **Masks** tool is selected on the rail, and it is then the *whole*
//! panel — the global adjustment sliders are not visible alongside it. A mask carries its own
//! Temp…Vibrance, so nothing here needs them.

use crate::develop::panel::Edited;
use crate::{theme, Session};
use eframe::egui;
use rustroom_app::mask_tool;

/// Brush tool settings, which live on the app rather than on the mask.
///
/// ⚠ They are the *tool's* state and apply to the next stroke, exactly as a brush size does
/// in any editor. Each stroke records its own copy at the moment it is drawn.
///
/// ⚠ Five `&mut` to *disjoint* fields of `RustroomApp`, which is what replaced the previous
/// copy-into-locals-and-write-back workaround: that existed only because a closure capturing
/// `app` while `s` also borrowed it is `E0499`. Explicit parameters have no such problem.
pub struct BrushTool<'a> {
    pub size: &'a mut f32,
    pub hardness: &'a mut f32,
    pub flow: &'a mut f32,
    pub erase: &'a mut bool,
    pub show_tint: &'a mut bool,
}

/// Draw the Masks tool's controls.
///
/// ⚠ No `CollapsingHeader` of its own — the panel supplies the heading. A header inside a
/// header is a nesting level this rework exists to remove.
pub fn show(
    ui: &mut egui::Ui,
    s: &mut Session,
    mask_hsl: &mut super::adjustments::HslChannel,
    mask_grade: &mut super::color_wheel::GradeRange,
    brush: BrushTool<'_>,
    ed: &mut Edited,
) {
    // Hoisted above every `&mut s.*` below: `Illuminant` is `Copy`, so reading it here costs
    // nothing and keeps the later mutable borrows disjoint.
    let baseline = s.wb_baseline;
    let mut added: Option<edit_model::Mask> = None;
    // ⚠ `horizontal_wrapped`, not `horizontal`: a fourth button does not fit the panel's ~240 pt
    // and a plain `horizontal` row overflows rather than wrapping, so the last one is clipped
    // where nothing announces it is there.
    ui.horizontal_wrapped(|ui| {
        if ui.button("Add graduated filter").clicked() {
            added = Some(edit_model::Mask::Linear(edit_model::LinearGradient {
                // A visible default: top to bottom across the middle
                // third, so the handles land on screen and the ramp is
                // immediately obvious.
                from: [0.5, 0.33],
                to: [0.5, 0.67],
                invert: false,
            }));
        }
        if ui.button("Add brush").clicked() {
            // ⚠ `Brush::default()` has no strokes, so the mask is inert until
            // painted — which is exactly why the tint defaults on.
            added = Some(edit_model::Mask::Brush(edit_model::Brush::default()));
        }
        if ui.button("Add radial filter").clicked() {
            // ⚠ The hand-written `Default`: centred, a quarter of each
            // axis, feather 50. Both radii being fractions of their own
            // axis makes the default ellipse *similar to the frame*, which
            // is usually the shape being reached for.
            added = Some(edit_model::Mask::Radial(
                edit_model::RadialGradient::default(),
            ));
        }
        if ui.button("Add luminance range").clicked() {
            // ⚠ The hand-written `Default`: the darker half with a soft shoulder. Visible
            // immediately and one click of Invert from the highlights, where a full-range
            // default would be indistinguishable from a global adjustment.
            added = Some(edit_model::Mask::Luminance(
                edit_model::LuminanceRange::default(),
            ));
        }
        // ⚠ Appended, per PARALLEL-WORK.md rule 3 — the mask-kind row gains one entry at the end
        // rather than being reordered around it.
        if ui.button("Add colour range").clicked() {
            // ⚠ The hand-written `Default`: the warm half of the wheel with a soft shoulder and
            // the chroma gate already at HSL's measured value, so the mask is doing something
            // recognisable before any slider is touched.
            added = Some(edit_model::Mask::Colour(edit_model::ColourRange::default()));
        }
    });
    if let Some(mask) = added {
        s.edits.masks.push(edit_model::MaskedAdjustment {
            mask,
            adjustments: edit_model::LocalAdjustments::default(),
        });
        s.selected_mask = Some(s.edits.masks.len() - 1);
        ed.discrete();
    }

    let mut remove: Option<usize> = None;
    for i in 0..s.edits.masks.len() {
        ui.horizontal(|ui| {
            // ⚠ The same "× then name" row the Presets tool uses. Two delete idioms in one
            // app is how a design system loses the conventions it exists to hold.
            if ui.button("×").on_hover_text("Delete").clicked() {
                remove = Some(i);
            }
            let picked = s.selected_mask == Some(i);
            let name = format!("{} {}", s.edits.masks[i].mask.label(), i + 1);
            if ui.selectable_label(picked, name).clicked() {
                s.selected_mask = if picked { None } else { Some(i) };
            }
        });
    }
    if let Some(i) = remove {
        s.selected_mask =
            mask_tool::selection_after_delete(s.selected_mask, i, s.edits.masks.len());
        s.edits.masks.remove(i);
        // ⚠ Clear any in-flight drag handle too: the canvas overlay
        // returns early when the selection is gone, which would
        // otherwise leave a stale handle set and panning dead.
        s.active_mask_handle = None;
        ed.discrete();
    }

    if let Some(i) = s.selected_mask {
        if let Some(m) = s.edits.masks.get_mut(i) {
            ui.separator();
            // Shape properties, kept apart from the adjustments below:
            // these describe the mask, not what it carries.
            //
            // ⚠ `invert` had no UI at all until now. Survivable for a
            // linear gradient, which can be inverted by dragging its
            // endpoints past each other; not survivable for a radial,
            // where the subject vignette — darken everything except the
            // ellipse — depends on it and no drag substitutes.
            if ui.checkbox(m.mask.invert_mut(), "Invert").changed() {
                ed.discrete();
            }
            if let edit_model::Mask::Radial(r) = &mut m.mask {
                ed.slider(ui, &mut r.feather, 0.0..=100.0, "Feather");
            }
            if let edit_model::Mask::Luminance(r) = &mut m.mask {
                // ⚠⚠ **These sliders are the whole of this mask's interaction — there is nothing
                // on the canvas.** A range mask is not a shape: it has no position, so no handle
                // and no boundary can be drawn for it. See `mask_overlay::paint_mask` for why a
                // coverage tint would be misleading rather than merely absent.
                ed.slider(ui, &mut r.low, 0.0..=100.0, "Range low");
                ed.slider(ui, &mut r.high, 0.0..=100.0, "Range high");
                // ⚠ Named "Feather" like the radial's and the vignette's, and a smoothstep like
                // both, so the app has one idiom for "soften this edge" rather than three.
                ed.slider(ui, &mut r.feather, 0.0..=100.0, "Feather");
                theme::hint(
                    ui,
                    "Selects tones by brightness — 0 is black, 100 is white, the \
                     same scale as the histogram. Feather widens the range with a \
                     soft shoulder either side.",
                );
            }
            if let edit_model::Mask::Colour(r) = &mut m.mask {
                // ⚠⚠ **These sliders are the whole of this mask's interaction — there is nothing
                // on the canvas**, for the luminance range's reason and one more: a colour range
                // has no position at all, so no handle and no boundary can be drawn for it.
                //
                // ⚠ Named "Hue low"/"Hue high" rather than the luminance range's "Range
                // low"/"Range high" deliberately. They are never on screen together, so this is
                // not the two-sliders-called-Scale ambiguity the perspective work fixed — it is
                // that the label should say *what* is being ranged, and a test asserting the
                // colour block appears could not otherwise tell it from the luminance one being
                // drawn for every mask.
                ed.slider(ui, &mut r.low, 0.0..=360.0, "Hue low");
                ed.slider(ui, &mut r.high, 0.0..=360.0, "Hue high");
                // ⚠ Named "Feather" like the luminance range's, the radial's and the vignette's,
                // and a smoothstep like all three, so the app keeps one idiom for "soften this
                // edge" rather than four.
                ed.slider(ui, &mut r.feather, 0.0..=100.0, "Feather");
                // ⚠ The top of the range is 0.15, not 1.0. Oklab chroma is not a percentage:
                // sensor noise measures ~0.003, an ordinary real colour ~0.06, and a fully
                // saturated sRGB primary reaches ~0.26 — so a gate above 0.15 excludes nearly
                // every colour in a photograph and the rest of the slider's travel would do
                // nothing but turn the mask off.
                ed.slider(ui, &mut r.min_chroma, 0.0..=0.15, "Min chroma");
                theme::hint(
                    ui,
                    "Selects colours by hue, in degrees around the wheel — and the band \
                     wraps, so 340 to 20 selects the reds either side of zero. Min chroma \
                     is what stops a near-neutral sky being selected: below it a pixel has \
                     no reliable hue, only sensor noise.",
                );
            }
            if matches!(m.mask, edit_model::Mask::Brush(_)) {
                // ⚠ Brush settings live on the app, not on the mask: they are
                // the *tool's* state and apply to the next stroke, exactly as
                // a brush size does in any editor. Each stroke records its own
                // copy at the moment it is drawn.
                ed.slider(ui, &mut *brush.size, 4.0..=300.0, "Size");
                ed.slider(ui, &mut *brush.hardness, 0.0..=100.0, "Hardness");
                ed.slider(ui, &mut *brush.flow, 1.0..=100.0, "Flow");
                ui.checkbox(&mut *brush.erase, "Erase");
                ui.checkbox(&mut *brush.show_tint, "Show mask");
                theme::hint(
                    ui,
                    "Drag to paint. Hardness shapes the edge; Flow is \
                     strength, and repeated passes build up.",
                );
            }
            ui.separator();
            super::mask_adjust::show(ui, &mut m.adjustments, baseline, mask_hsl, mask_grade, ed);
        }
    }

    theme::hint(
        ui,
        "Select a mask to drag its handles on the image. A gradient's two \
         outer lines mark where the effect starts and reaches full \
         strength; a radial's ellipse marks where it falls to nothing.",
    );

    ui.separator();
    // ⚠ Scoped to the Masks group, the same rule the Edit and Crop tools follow. Disabled when
    // there is nothing to remove, so the button never looks like it did nothing.
    if ui
        .add_enabled(
            !s.edits.masks.is_empty(),
            egui::Button::new("Remove all masks"),
        )
        .on_hover_text("Deletes every mask. Leaves the adjustments and the crop alone.")
        .clicked()
    {
        s.edits.merge_from(
            &edit_model::Edits::default(),
            edit_model::EditGroups::only(edit_model::MASKS_GROUP),
        );
        // ⚠ Both must be cleared, for the reason `selection_after_delete` exists: the canvas
        // overlay returns early on a stale selection, which would leave a dead handle set.
        s.selected_mask = None;
        s.active_mask_handle = None;
        ed.discrete();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_ui;

    /// Drive the real `masks::show` against a scratch session, clicking a widget by label.
    ///
    /// ⚠ The panel is driven **as it ships** — no re-declared widgets. A harness that redraws its
    /// own buttons tests the harness; this project already has two such tests (`cell_widths`,
    /// `grid_columns`) and they are documented as pinning a *rule*, not a call site.
    fn click_in_panel(label: &str, s: &mut Session) -> Edited {
        let ctx = test_ui::ctx();
        let mut ed = Edited::default();
        let (mut size, mut hardness, mut flow) = (40.0_f32, 50.0_f32, 100.0_f32);
        let (mut erase, mut tint) = (false, true);
        // ⚠ A local: a test must not be able to assert about the *global* panel's channel.
        let mut channel = crate::develop::panel::adjustments::HslChannel::default();
        // ⚠ A local for the same reason, covering the mask's own selected grading wheel.
        let mut grade = crate::develop::panel::color_wheel::GradeRange::default();
        test_ui::click(&ctx, label, |ui| {
            ed = Edited::default();
            show(
                ui,
                s,
                &mut channel,
                &mut grade,
                BrushTool {
                    size: &mut size,
                    hardness: &mut hardness,
                    flow: &mut flow,
                    erase: &mut erase,
                    show_tint: &mut tint,
                },
                &mut ed,
            );
        });
        ed
    }

    fn session() -> Session {
        let ctx = test_ui::ctx();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        app.session.take().unwrap()
    }

    #[test]
    fn each_add_button_adds_its_own_kind_of_mask() {
        // ⚠ All three, not just one. Three buttons wired to the same constructor would pass a
        // test that only ever clicked "Add graduated filter".
        for (label, want) in [
            ("Add graduated filter", "Linear"),
            ("Add brush", "Brush"),
            ("Add radial filter", "Radial"),
            ("Add luminance range", "Luminance"),
            ("Add colour range", "Colour"),
        ] {
            let mut s = session();
            click_in_panel(label, &mut s);
            assert_eq!(s.edits.masks.len(), 1, "{label} added no mask");
            let got = match s.edits.masks[0].mask {
                edit_model::Mask::Linear(_) => "Linear",
                edit_model::Mask::Radial(_) => "Radial",
                edit_model::Mask::Brush(_) => "Brush",
                edit_model::Mask::Luminance(_) => "Luminance",
                edit_model::Mask::Colour(_) => "Colour",
            };
            assert_eq!(got, want, "{label} added the wrong kind");
        }
    }

    #[test]
    fn adding_a_mask_selects_it_and_counts_as_an_edit() {
        // ⚠ Selecting it is what puts its handles on the canvas. A mask added without being
        // selected is invisible and unmovable, which reads as "the button did nothing".
        let mut s = session();
        let ed = click_in_panel("Add graduated filter", &mut s);
        assert_eq!(s.selected_mask, Some(0));
        assert!(ed.changed, "the preview must be re-requested");
        assert!(ed.settled, "and it must be one undo step");
    }

    #[test]
    fn a_second_mask_is_selected_over_the_first() {
        let mut s = session();
        click_in_panel("Add graduated filter", &mut s);
        click_in_panel("Add radial filter", &mut s);
        assert_eq!(s.edits.masks.len(), 2);
        assert_eq!(
            s.selected_mask,
            Some(1),
            "the newest mask is the selected one"
        );
    }

    #[test]
    fn deleting_the_selected_mask_clears_the_selection_and_the_drag_handle() {
        // ⚠⚠ Two things at once, and both were bugs waiting to happen. Deleting the selected
        // mask must **clear** the selection rather than move it to a neighbour — silently
        // selecting a different mask means the next slider drag edits something the user never
        // chose, the rule `Selection::clamp` already follows. And a stale `active_mask_handle`
        // leaves the canvas returning early with panning dead.
        let mut s = session();
        click_in_panel("Add graduated filter", &mut s);
        s.active_mask_handle = Some(crate::mask_tool::Handle::From);
        assert_eq!(s.selected_mask, Some(0));

        click_in_panel("×", &mut s);

        assert!(s.edits.masks.is_empty(), "the mask should be gone");
        assert_eq!(s.selected_mask, None, "and nothing should be selected");
        assert!(
            s.active_mask_handle.is_none(),
            "and no drag handle left behind"
        );
    }

    /// Drag the widget labelled `label` to the right, and report what the panel recorded.
    ///
    /// ⚠⚠ **A drag, not a click, and that was found the hard way.** `test_ui::click` presses and
    /// releases at one point; an `egui::Slider` built with `.text(..)` spans a rail, a numeric
    /// box and the caption, so its accesskit centre need not land on the rail at all and the
    /// value does not move. A drag engages it wherever it starts.
    ///
    /// ⚠ Five passes, the count `test_support::drag_through` established: `Response` state is
    /// per-frame, and a harness that moves the pointer on the same pass as the press produces a
    /// gesture that grabs nothing while looking exactly like one that works.
    fn drag_in_panel(label: &str, s: &mut Session) -> Edited {
        let ctx = test_ui::ctx();
        let mut ed = Edited::default();
        let (mut size, mut hardness, mut flow) = (40.0_f32, 50.0_f32, 100.0_f32);
        let (mut erase, mut tint) = (false, true);
        // ⚠ A local: a test must not be able to assert about the *global* panel's channel.
        let mut channel = crate::develop::panel::adjustments::HslChannel::default();
        // ⚠ A local for the same reason, covering the mask's own selected grading wheel.
        let mut grade = crate::develop::panel::color_wheel::GradeRange::default();
        let mut body = |ui: &mut egui::Ui, ed: &mut Edited| {
            show(
                ui,
                s,
                &mut channel,
                &mut grade,
                BrushTool {
                    size: &mut size,
                    hardness: &mut hardness,
                    flow: &mut flow,
                    erase: &mut erase,
                    show_tint: &mut tint,
                },
                ed,
            );
        };
        let from =
            test_ui::labelled_centres(&ctx, Vec::new(), |ui| body(ui, &mut Edited::default()))
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
                |ui| body(ui, &mut pass),
            );
            ed.changed |= pass.changed;
            ed.settled |= pass.settled;
        }
        ed
    }

    /// Every widget label the panel draws for `s`, laid out once.
    fn labels_in_panel(s: &mut Session) -> Vec<String> {
        let ctx = test_ui::ctx();
        let (mut size, mut hardness, mut flow) = (40.0_f32, 50.0_f32, 100.0_f32);
        let (mut erase, mut tint) = (false, true);
        // ⚠ A local: a test must not be able to assert about the *global* panel's channel.
        let mut channel = crate::develop::panel::adjustments::HslChannel::default();
        // ⚠ A local for the same reason, covering the mask's own selected grading wheel.
        let mut grade = crate::develop::panel::color_wheel::GradeRange::default();
        test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
            let mut ed = Edited::default();
            show(
                ui,
                s,
                &mut channel,
                &mut grade,
                BrushTool {
                    size: &mut size,
                    hardness: &mut hardness,
                    flow: &mut flow,
                    erase: &mut erase,
                    show_tint: &mut tint,
                },
                &mut ed,
            );
        })
        .into_iter()
        .map(|(l, _)| l)
        .collect()
    }

    /// Every tail-native slider is present on a selected mask **and wired to that mask's field**.
    ///
    /// ⚠⚠ That a slider *exists* proves nothing about what it edits. This project has shipped
    /// exactly that bug — the Library grid drew a name field bound to the wrong string for a
    /// week — so each drag is asserted against the mask's own value, not merely against `Edited`.
    ///
    /// ⚠ All four in one test on purpose: they arrived together and share one wiring path, so a
    /// per-slider test would be four copies of one assertion. If they ever stop sharing it, this
    /// is the test that should be split.
    #[test]
    fn the_tail_native_sliders_edit_the_selected_mask() {
        for (label, read) in [
            (
                "Texture",
                (|a: &edit_model::LocalAdjustments| a.local_contrast.texture)
                    as fn(&edit_model::LocalAdjustments) -> f32,
            ),
            ("Clarity", |a| a.local_contrast.clarity),
            ("Amount", |a| a.sharpening.amount),
            // ⚠ The HSL block's first band. It reaches through the *shared* `hsl_bands` widget,
            // so this is also what proves the extraction is wired to the mask rather than to
            // the global panel's HSL.
            ("Red", |a| a.hsl.bands[0].hue),
        ] {
            let mut s = session();
            click_in_panel("Add graduated filter", &mut s);
            let before = read(&s.edits.masks[0].adjustments);

            let ed = drag_in_panel(label, &mut s);
            let after = read(&s.edits.masks[0].adjustments);

            assert!(ed.changed, "dragging {label} reported no edit");
            assert_ne!(
                after, before,
                "the {label} slider is not wired to the selected mask"
            );
        }
    }

    #[test]
    fn a_selected_luminance_mask_shows_its_range_and_a_radial_does_not() {
        // ⚠⚠ **Both halves, and the pair is what makes the test mean anything.** Sliders that
        // never appear and sliders that always appear are equally wrong, and a test asserting
        // only the first passes against a panel that draws them for every mask — where "Range
        // low" beside a radial's ellipse controls nothing at all.
        //
        // ⚠ These three sliders are the *whole* interaction for this mask: there is nothing on
        // the canvas to drag, so a panel that failed to draw them would leave it unusable rather
        // than merely inconvenient.
        let mut s = session();
        click_in_panel("Add luminance range", &mut s);
        let shown = labels_in_panel(&mut s);
        for want in ["Range low", "Range high", "Feather"] {
            assert!(
                shown.iter().any(|l| l == want),
                "a selected luminance mask must offer {want}; got {shown:?}"
            );
        }

        let mut s = session();
        click_in_panel("Add radial filter", &mut s);
        let shown = labels_in_panel(&mut s);
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
        // ⚠⚠ **Added because the sabotage step showed the test above could not see this.**
        // Pointing the sliders at a clone (`&mut m.mask.clone()`) leaves every label drawn and
        // every assertion about *presence* green, while the panel edits a temporary and the mask
        // never moves — the exact "a test names a bug it cannot detect" shape, and here it would
        // ship as three controls that visibly do nothing.
        let mut s = session();
        click_in_panel("Add luminance range", &mut s);
        let before = match s.edits.masks[0].mask {
            edit_model::Mask::Luminance(r) => r,
            ref other => panic!("expected a luminance mask, got {other:?}"),
        };
        let ed = drag_in_panel("Range high", &mut s);
        let after = match s.edits.masks[0].mask {
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
        // ⚠⚠ **Both halves, the pair `a_selected_luminance_mask_shows_its_range_and_a_radial_
        // does_not` established.** Controls that never appear and controls that appear for every
        // mask are equally wrong, and only the pair distinguishes them — "Min chroma" beside a
        // luminance range's tone band would control nothing at all.
        //
        // ⚠ These four sliders are the *whole* interaction for this mask: there is nothing on the
        // canvas to drag, so a panel that failed to draw them would leave it unusable rather than
        // merely inconvenient.
        let mut s = session();
        click_in_panel("Add colour range", &mut s);
        let shown = labels_in_panel(&mut s);
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

        let mut s = session();
        click_in_panel("Add luminance range", &mut s);
        let shown = labels_in_panel(&mut s);
        assert!(
            !shown.iter().any(|l| l == "Hue low" || l == "Min chroma"),
            "a luminance mask must not offer a hue band; got {shown:?}"
        );
    }

    #[test]
    fn the_hue_sliders_are_wired_to_the_mask_and_settle_into_one_edit() {
        // ⚠⚠ **Presence is not wiring, and the sabotage step proved that on the luminance range's
        // own sliders.** Pointing these at a clone leaves every label drawn and every assertion
        // about presence green while the panel edits a temporary and the mask never moves — four
        // controls that visibly do nothing.
        let mut s = session();
        click_in_panel("Add colour range", &mut s);
        let before = match s.edits.masks[0].mask {
            edit_model::Mask::Colour(r) => r,
            ref other => panic!("expected a colour mask, got {other:?}"),
        };
        let ed = drag_in_panel("Hue high", &mut s);
        let after = match s.edits.masks[0].mask {
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
        let mut s = session();
        click_in_panel("Add graduated filter", &mut s);
        let name = format!("{} 1", s.edits.masks[0].mask.label());
        click_in_panel(&name, &mut s);
        assert_eq!(s.selected_mask, None);
        assert_eq!(s.edits.masks.len(), 1, "deselecting must not delete it");
    }
}
