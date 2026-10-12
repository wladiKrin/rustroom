//! The adjustments a mask carries, under the same four sections as the Edit panel.
//!
//! ⚠ Its own module because `masks.rs` was 575 lines before this and these controls add ~120.
//! The rest of that file is the list, the shapes and the brush settings, which is a different
//! job.
//!
//! ⚠⚠ **The order mirrors the global panel**, which is both its rule and (mostly) the
//! pipeline's: White Balance, Tone, Colour, Detail. **With exactly two exceptions, and they are
//! deliberate: Dehaze and Noise Reduction.** Both run in the **prefix**, before everything else
//! here, so strict pipeline order would put them above White Balance. They are placed where the
//! *global panel* puts them instead — Dehaze with Texture and Clarity, noise reduction above
//! Sharpening — on the rule `CLAUDE.md` records for these two stages specifically: *"Panel
//! follows workflow, pipeline follows maths."* `LocalGroup` in `edit-model` partitions the
//! fields the same way, so the dot and the reset agree with the layout by construction.

use crate::develop::panel::{adjustments, color_wheel, hsl_bands, section, tone_curve, Edited};
use crate::theme;
use edit_model::{LocalAdjustments, LocalGroup};
use eframe::egui;

/// Draw every adjustment control for one mask, returning the tone-curve editor's own rect.
///
/// ⚠ The section ids are salted `("mask", name)`: `make_persistent_id("Tone")` here would share
/// its open state with the Edit panel's Tone, and collapsing one would collapse the other.
///
/// ⚠ White Balance and Tone start open, Color and Detail closed: a mask's section is already
/// the tallest thing in the panel, and the first thing reached for is Exposure.
///
/// ⚠ The returned rect is `Rect::NOTHING` while Tone is collapsed, because a closed section does
/// not run its body. Not decoration: a headless test has to aim a drag at a point on the curve,
/// and the alternative is hardcoding a screen coordinate — brittle in the worst way, because a
/// layout change then moves the target while the test keeps passing and grabbing nothing.
pub fn show(
    ui: &mut egui::Ui,
    a: &mut LocalAdjustments,
    baseline: color::wb::Illuminant,
    channel: &mut adjustments::HslChannel,
    grade: &mut color_wheel::GradeRange,
    // `Settings::tone_curve_open` — one choice for the global curve and a mask's.
    curve_open: &mut bool,
    ed: &mut Edited,
) -> egui::Rect {
    let mut curve_rect = egui::Rect::NOTHING;
    for g in LocalGroup::ALL {
        let open = matches!(g, LocalGroup::WhiteBalance | LocalGroup::Tone);
        let mut out = section::header(
            ui,
            ui.make_persistent_id(("mask", g.name())),
            open,
            section::Header::new(g.name())
                .modified(a.group_differs(g))
                .resettable(true),
        );
        out.show_body(ui, |ui| match g {
            LocalGroup::WhiteBalance => {
                // ⚠⚠ Shifts relative to the photo's white balance, Lightroom's local Temp and Tint
                // (M10.12b) — not the global sliders' Kelvin. A legacy mask shows its absolute
                // value as the shift it amounts to, and only a drag converts it: drawing it must
                // not rewrite an edit nobody touched.
                let (mut temp, mut tint) = rustroom_app::mask_wb::shown(a, baseline);
                ed.help_scope = rustroom_app::slider_help::Scope::MaskWhiteBalance;
                let t = ed.slider(ui, &mut temp, -100.0..=100.0, "Temp", 0.0);
                let n = ed.slider(ui, &mut tint, -100.0..=100.0, "Tint", 0.0);
                ed.help_scope = rustroom_app::slider_help::Scope::Global;
                if t.changed() || n.changed() {
                    rustroom_app::mask_wb::set(a, temp, tint);
                }
            }
            LocalGroup::Tone => {
                ed.slider(ui, &mut a.exposure, -5.0..=5.0, "Exposure", 0.0);
                ed.slider(ui, &mut a.contrast, -100.0..=100.0, "Contrast", 0.0);
                ed.slider(ui, &mut a.highlights, -100.0..=100.0, "Highlights", 0.0);
                ed.slider(ui, &mut a.shadows, -100.0..=100.0, "Shadows", 0.0);
                ed.slider(ui, &mut a.whites, -100.0..=100.0, "Whites", 0.0);
                ed.slider(ui, &mut a.blacks, -100.0..=100.0, "Blacks", 0.0);
                // ⚠ Inline under its sub-label, as in the Edit panel: no histogram ghost, since
                // a mask's curve acts on a region no histogram describes.
                // ⚠ `None`: a mask's curve is composite-only by decision, so it gets no channel
                // selector — see `build_pipeline`'s mask branch, which is the other half of that.
                // ⚠ Collapsible like the global curve, under the same setting (2026-09-29).
                let modified = !a.tone_curve.points.is_empty();
                if tone_curve::header(ui, curve_open, modified, |ui| {
                    tone_curve::reset_button(ui, &mut a.tone_curve, ed)
                }) {
                    curve_rect = tone_curve::show(ui, &mut a.tone_curve, ed, None, None, None);
                }
                theme::sub_label(ui, "Presence");
                ed.slider(
                    ui,
                    &mut a.local_contrast.texture,
                    -100.0..=100.0,
                    "Texture",
                    0.0,
                );
                ed.slider(
                    ui,
                    &mut a.local_contrast.clarity,
                    -100.0..=100.0,
                    "Clarity",
                    0.0,
                );
                ed.slider(ui, &mut a.dehaze.amount, -100.0..=100.0, "Dehaze", 0.0);
            }
            LocalGroup::Color => {
                ed.slider(ui, &mut a.vibrance, -100.0..=100.0, "Vibrance", 0.0);
                ed.slider(ui, &mut a.saturation, -100.0..=100.0, "Saturation", 0.0);
                ui.add_space(4.0);
                hsl_bands::show(ui, &mut a.hsl, channel, ed);
                theme::sub_label(ui, "Color grading");
                theme::segmented(
                    ui,
                    grade,
                    &color_wheel::GradeRange::ALL.map(|r| (r, r.label(), r.hover())),
                );
                let diameter = ui.available_width().min(188.0);
                {
                    color_wheel::show(ui, &mut a.color_grading, *grade, diameter, ed);
                    let w = grade.wheel_mut(&mut a.color_grading);
                    ed.help_scope = rustroom_app::slider_help::Scope::TintByTone;
                    ed.slider(ui, &mut w.hue, 0.0..=360.0, "Hue", 0.0);
                    // ⚠ `Strength` here too — the same instrument, see `adjustments`.
                    ed.slider(ui, &mut w.saturation, 0.0..=100.0, "Strength", 0.0);
                    ed.slider(ui, &mut w.luminance, -100.0..=100.0, "Luminance", 0.0);
                }
                ui.separator();
                let cg = &mut a.color_grading;
                ed.slider(
                    ui,
                    &mut cg.blending,
                    0.0..=100.0,
                    "Blending",
                    edit_model::ColorGrading::default().blending,
                );
                ed.slider(ui, &mut cg.balance, -100.0..=100.0, "Balance", 0.0);
                ed.help_scope = rustroom_app::slider_help::Scope::Global;
            }
            LocalGroup::Detail => {
                theme::sub_label(ui, "Noise reduction");
                ed.help_scope = rustroom_app::slider_help::Scope::NoiseReduction;
                ed.slider(
                    ui,
                    &mut a.noise_reduction.luminance,
                    0.0..=100.0,
                    "Luminance",
                    0.0,
                );
                ed.slider(
                    ui,
                    &mut a.noise_reduction.detail,
                    0.0..=100.0,
                    "Detail",
                    edit_model::NoiseReduction::default().detail,
                );
                ed.slider(ui, &mut a.noise_reduction.color, 0.0..=100.0, "Color", 0.0);
                ed.slider(
                    ui,
                    &mut a.noise_reduction.smoothness,
                    0.0..=100.0,
                    "Smoothness",
                    edit_model::NoiseReduction::default().smoothness,
                );
                theme::sub_label(ui, "Sharpening");
                ed.help_scope = rustroom_app::slider_help::Scope::Sharpening;
                ed.slider(ui, &mut a.sharpening.amount, 0.0..=100.0, "Amount", 0.0);
                ed.slider(
                    ui,
                    &mut a.sharpening.radius,
                    0.5..=3.0,
                    "Radius (px)",
                    edit_model::Sharpening::default().radius,
                );
                ed.slider(
                    ui,
                    &mut a.sharpening.detail,
                    0.0..=100.0,
                    "Detail",
                    edit_model::Sharpening::default().detail,
                );
                ed.slider(ui, &mut a.sharpening.masking, 0.0..=100.0, "Masking", 0.0);
                ed.help_scope = rustroom_app::slider_help::Scope::Global;
                // ⚠ M9.6, Lightroom's meaning: negative protects this area from the global
                // defringe (a real purple object), positive adds defringe here.
                theme::sub_label(ui, "Defringe");
                ed.slider(ui, &mut a.defringe, -100.0..=100.0, "Defringe", 0.0);
            }
        });
        if out.reset {
            a.reset_group(g);
            ed.discrete();
        }
    }
    curve_rect
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_ui;

    struct Harness {
        ctx: egui::Context,
        a: LocalAdjustments,
        ed: Edited,
        channel: adjustments::HslChannel,
        grade: color_wheel::GradeRange,
        rect: egui::Rect,
    }

    impl Harness {
        fn new() -> Self {
            let ctx = test_ui::ctx();
            // ⚠⚠ **Animation off, and it is load-bearing.** A section animates open, and with
            // the animation on the body is still zero-height on the frame after the click.
            for theme in [egui::Theme::Dark, egui::Theme::Light] {
                ctx.style_mut_of(theme, |s| s.animation_time = 0.0);
            }
            Self {
                ctx,
                a: LocalAdjustments::default(),
                ed: Edited::default(),
                channel: adjustments::HslChannel::default(),
                grade: color_wheel::GradeRange::default(),
                rect: egui::Rect::NOTHING,
            }
        }

        fn body(&mut self, ui: &mut egui::Ui) {
            let ed = &mut self.ed;
            self.rect = show(
                ui,
                &mut self.a,
                color::wb::Illuminant::default(),
                &mut self.channel,
                &mut self.grade,
                &mut true,
                ed,
            );
        }

        /// One frame, no input; the labels drawn.
        fn draw(&mut self) -> Vec<String> {
            let ctx = self.ctx.clone();
            test_ui::labelled_centres(&ctx, Vec::new(), |ui| self.body(ui))
                .into_iter()
                .map(|(l, _)| l)
                .collect()
        }

        fn click(&mut self, label: &str) {
            let ctx = self.ctx.clone();
            self.ed = Edited::default();
            test_ui::click(&ctx, label, |ui| self.body(ui));
        }

        fn drag_curve(&mut self, from: [f32; 2], to: [f32; 2]) {
            let at = |r: egui::Rect, n: [f32; 2]| {
                egui::pos2(r.left() + n[0] * r.width(), r.bottom() - n[1] * r.height())
            };
            let ctx = self.ctx.clone();
            for pass in 0..5 {
                let (p, q) = (at(self.rect, from), at(self.rect, to));
                let mut events = Vec::new();
                match pass {
                    1 => events.push(egui::Event::PointerMoved(p)),
                    2 => events.push(egui::Event::PointerButton {
                        pos: p,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: Default::default(),
                    }),
                    3 => events.push(egui::Event::PointerMoved(q)),
                    4 => events.push(egui::Event::PointerButton {
                        pos: q,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: Default::default(),
                    }),
                    _ => {}
                }
                let _ = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(test_ui::screen()),
                        events,
                        ..Default::default()
                    },
                    |ui| self.body(ui),
                );
            }
        }

        fn drag_slider(&mut self, label: &str) {
            let ctx = self.ctx.clone();
            let at = test_ui::labelled_centres(&ctx, Vec::new(), |ui| self.body(ui))
                .into_iter()
                .find(|(l, _)| l == label)
                .map(|(_, p)| p)
                .unwrap_or_else(|| panic!("no slider labelled {label:?} was drawn"));
            let to = at + egui::vec2(40.0, 0.0);
            for pass in 1..5 {
                let mut events = Vec::new();
                match pass {
                    1 => events.push(egui::Event::PointerMoved(at)),
                    2 => events.push(egui::Event::PointerButton {
                        pos: at,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: Default::default(),
                    }),
                    3 => events.push(egui::Event::PointerMoved(to)),
                    4 => events.push(egui::Event::PointerButton {
                        pos: to,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: Default::default(),
                    }),
                    _ => {}
                }
                let _ = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(test_ui::screen()),
                        events,
                        ..Default::default()
                    },
                    |ui| self.body(ui),
                );
            }
        }
    }

    #[test]
    fn the_capture_sliders_reach_the_masks_own_fields() {
        // ⚠ Dehaze sits in Tone (open); Smoothness and Masking in Detail, opened first through
        // its own header. "Color" and "Detail" are not used as slider labels here because each
        // now collides with a section header for the harness.
        type Case = (
            &'static str,
            Option<&'static str>,
            fn(&LocalAdjustments) -> f32,
        );
        let cases: [Case; 3] = [
            ("Dehaze", None, |a| a.dehaze.amount),
            ("Smoothness", Some("Detail"), |a| {
                a.noise_reduction.smoothness
            }),
            ("Masking", Some("Detail"), |a| a.sharpening.masking),
        ];
        for (label, section, read) in cases {
            let mut h = Harness::new();
            if let Some(sec) = section {
                h.click(sec);
            }
            let before = read(&h.a);
            h.drag_slider(label);
            assert_ne!(
                read(&h.a),
                before,
                "dragging {label} never reached the mask's own field"
            );
        }
    }

    /// A mask's Temp and Tint are shifts relative to the photo (M10.12b): a drag writes `temp` and
    /// `tint`, never the legacy absolute white balance.
    #[test]
    fn the_masks_temp_and_tint_are_relative_shifts() {
        for (label, read) in [
            ("Temp", (|a| a.temp) as fn(&LocalAdjustments) -> f32),
            ("Tint", |a| a.tint),
        ] {
            let mut h = Harness::new();
            h.drag_slider(label);
            assert!(
                read(&h.a) != 0.0,
                "dragging {label} never reached the shift"
            );
            assert_eq!(
                h.a.white_balance,
                edit_model::WhiteBalance::default(),
                "{label} wrote the legacy absolute value"
            );
            assert!(h.ed.changed && h.ed.settled, "{label}: one undo step");
        }
    }

    /// ⚠ Not 6500 K: the harness's baseline is `Illuminant::default()`, D65, so 6500 K would amount
    /// to no shift at all and the conversion would be invisible.
    fn legacy() -> LocalAdjustments {
        LocalAdjustments {
            white_balance: edit_model::WhiteBalance {
                kelvin: Some(4000.0),
                tint: 4.0,
            },
            ..Default::default()
        }
    }

    /// A legacy mask's Kelvin is shown as the shift it amounts to, and the first drag of EITHER
    /// slider converts it — Tint alone leaves Temp at exactly that shift, not at zero.
    #[test]
    fn a_legacy_mask_converts_on_its_first_drag_and_keeps_its_temperature() {
        let mut h = Harness::new();
        h.a = legacy();
        let (temp, _) = pipeline::mask_wb_shift(&h.a, color::wb::Illuminant::default());
        assert!(
            temp.abs() > 1.0,
            "fixture check: the legacy shift is not zero"
        );
        h.drag_slider("Tint");
        assert_eq!(h.a.white_balance, edit_model::WhiteBalance::default());
        assert_eq!(h.a.temp, temp, "Temp kept the converted legacy value");
        assert!(
            h.a.tint != 4.0,
            "the drag moved Tint from its converted value"
        );
    }

    /// Merely looking at a legacy mask converts nothing — only a drag does.
    #[test]
    fn drawing_a_legacy_mask_leaves_it_as_it_was() {
        let mut h = Harness::new();
        h.a = legacy();
        h.draw();
        assert_eq!(h.a, legacy());
        assert!(!h.ed.changed);
    }

    #[test]
    fn dragging_the_masked_curve_reaches_the_masks_own_tone_curve() {
        let mut h = Harness::new();
        h.draw();
        assert!(
            h.rect.is_positive(),
            "fixture check: the curve editor was never laid out, so the drag would prove nothing"
        );
        h.drag_curve([0.5, 0.5], [0.5, 0.8]);
        assert!(
            !h.a.tone_curve.points.is_empty(),
            "the drag never reached the mask's tone_curve"
        );
        assert!(h.ed.changed, "the preview must be re-requested");
        assert!(h.ed.settled, "and a settled drag is one undo step");
    }

    #[test]
    fn color_and_detail_start_collapsed_and_tone_open() {
        // ⚠ A mask's section is already the tallest thing in the panel; the two long sections
        // start closed. Their headers are drawn either way.
        let mut h = Harness::new();
        let labels = h.draw();
        let has = |l: &str| labels.iter().any(|n| n == l);
        assert!(
            has("Color") && has("Detail"),
            "the headers must be drawn even when closed"
        );
        assert!(
            !has("Blending"),
            "Color's contents must be hidden while it is closed"
        );
        assert!(
            !has("Masking"),
            "Detail's contents must be hidden while it is closed"
        );
        assert!(
            h.rect.is_positive(),
            "Tone is open, so the curve is laid out"
        );
    }

    #[test]
    fn drawing_the_panel_leaves_the_masks_own_grade_range_alone() {
        let mut h = Harness::new();
        h.grade = color_wheel::GradeRange::Highlights;
        h.click("Color");
        assert_eq!(
            h.grade,
            color_wheel::GradeRange::Highlights,
            "drawing the panel must not reset the mask's own selected wheel"
        );
    }

    #[test]
    fn a_sections_reset_clears_exactly_that_group() {
        // ⚠ Tone and Detail are both modified, so two resets are drawn and the harness clicks
        // whichever it finds first — the AccessKit tree does not promise layout order. The
        // property is scoping, not which: exactly one of the two groups goes back to neutral
        // and the other is untouched.
        let mut h = Harness::new();
        h.a.dehaze.amount = 20.0;
        h.a.sharpening.amount = 30.0;
        h.click("reset");
        let (d, s) = (h.a.dehaze.amount, h.a.sharpening.amount);
        assert!(
            (d == 0.0 && s == 30.0) || (d == 20.0 && s == 0.0),
            "a reset must clear exactly its own section: dehaze {d}, sharpening {s}"
        );
        assert!(h.ed.settled, "one click, one history entry");
    }
}
