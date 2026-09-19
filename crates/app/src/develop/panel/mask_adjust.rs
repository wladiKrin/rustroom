//! The adjustments a mask carries — the per-pixel set it has always had, plus the tail-native
//! stages M7.2 added, plus the tone curve and colour grade of Part A.
//!
//! ⚠ Its own module because `masks.rs` was 575 lines before this and these controls add ~120.
//! The rest of that file is shape properties and brush settings, which is a different job.

use crate::develop::panel::{adjustments, color_wheel, hsl_bands, tone_curve, Edited};
use crate::theme;
use edit_model::LocalAdjustments;
use eframe::egui;

/// Draw every adjustment control for one mask, returning the tone-curve editor's own rect.
///
/// ⚠⚠ **The order mirrors the global chain**, which is both the panel's existing rule and the
/// pipeline's: White Balance, Tone, Colour, then the spatial stages — the order
/// `adjustment_stages` inserts them in.
///
/// ⚠⚠ **With exactly two exceptions, and they are deliberate: Dehaze and Noise Reduction.** Both
/// run in the **prefix**, before everything else here, so strict pipeline order would put them
/// above White Balance. They are placed where the *global panel* puts them instead — Dehaze with
/// Texture and Clarity, noise reduction above Sharpening — on the rule `CLAUDE.md` records for
/// these two stages specifically: *"Panel follows workflow, pipeline follows maths."* NR is
/// judged last, at 1:1, once tone is settled. "One layout learned once" is what decides it: the
/// cost of the exception is smaller than the cost of the two panels disagreeing.
///
/// ⚠ The returned rect is `Rect::NOTHING` while the Tone Curve block is collapsed, because a
/// closed `CollapsingHeader` does not run its body. Not decoration: a headless test has to aim a
/// drag at a point on the curve, and the alternative is hardcoding a screen coordinate — brittle
/// in the worst way, because a layout change then moves the target while the test keeps passing
/// and grabbing nothing. `test_ui`'s module doc records that fault happening twice.
pub fn show(
    ui: &mut egui::Ui,
    a: &mut LocalAdjustments,
    baseline: color::wb::Illuminant,
    channel: &mut adjustments::HslChannel,
    grade: &mut color_wheel::GradeRange,
    ed: &mut Edited,
) -> egui::Rect {
    // ⚠ The same control as the global one, and the same baseline: a mask declares the
    // light *this region* was under, measured against what the whole photo is balanced
    // to. Two different meanings for one number is how they drift.
    adjustments::white_balance(ui, &mut a.white_balance, baseline, None, ed);
    ed.slider(ui, &mut a.exposure, -5.0..=5.0, "Exposure");
    ed.slider(ui, &mut a.contrast, -100.0..=100.0, "Contrast");
    ed.slider(ui, &mut a.highlights, -100.0..=100.0, "Highlights");
    ed.slider(ui, &mut a.shadows, -100.0..=100.0, "Shadows");
    ed.slider(ui, &mut a.whites, -100.0..=100.0, "Whites");
    ed.slider(ui, &mut a.blacks, -100.0..=100.0, "Blacks");

    // ⚠⚠ **Collapsed, closed by default.** A mask's section is already the tallest thing in the
    // panel and a curve editor is a ~220 pt square rather than a row of sliders. It sits here,
    // between the tone sliders and the colour ones, because that is where `adjustment_stages`
    // runs it — after the tone regions, before local contrast.
    //
    // ⚠ The open/closed state lives in egui's memory and its id path is the same for every mask,
    // so it is **shared across masks**. Deliberate rather than accidental: you want the block to
    // stay open as you click through masks comparing them.
    let mut curve_rect = egui::Rect::NOTHING;
    egui::CollapsingHeader::new("Tone Curve")
        .id_salt("mask_tone_curve")
        .default_open(false)
        .show(ui, |ui| {
            // The same widget the global panel draws, unchanged — `hsl_bands`' rule, and
            // `adjustments.rs`' own "One layout learned once".
            curve_rect = tone_curve::show(ui, &mut a.tone_curve, ed, None);
        });

    // ⚠ The same order as the global panel: White Balance, then Tone, then
    // Color with Vibrance first. One layout learned once, and it keeps
    // holding as either list changes.
    ed.slider(ui, &mut a.vibrance, -100.0..=100.0, "Vibrance");
    ed.slider(ui, &mut a.saturation, -100.0..=100.0, "Saturation");

    ui.separator();
    // ⚠ Negative Texture is the skin-softening tool, and the reason deferring masked noise
    // reduction costs nothing: it removes the fine band and softens edges within it.
    ed.slider(ui, &mut a.local_contrast.texture, -100.0..=100.0, "Texture");
    ed.slider(ui, &mut a.local_contrast.clarity, -100.0..=100.0, "Clarity");
    // ⚠⚠ **Panel order deliberately does NOT follow pipeline order here, and this and the noise
    // reduction block below are the only two places in this file where that is true.** Both run
    // in the **prefix**, before everything else in this section — but `CLAUDE.md` records the
    // rule for exactly these two stages: *"The pipeline runs NR and dehaze before white balance
    // because that is where the maths wants them; NR is judged last, at 1:1, once tone is
    // settled. Panel follows workflow, pipeline follows maths."* They sit where the **global
    // panel** puts them, so "one layout learned once" keeps holding across the two panels.
    ed.slider(ui, &mut a.dehaze.amount, -100.0..=100.0, "Dehaze");

    ui.separator();
    // ⚠ The mask's own channel, never the global panel's — sharing one would silently change
    // which channel the global panel shows when you switch back to it.
    hsl_bands::show(ui, &mut a.hsl, channel, ed);

    ui.separator();
    // ⚠ Collapsed for the curve's reason: a wheel is a ~188 pt square plus five sliders. After
    // HSL because that is where `adjustment_stages` runs it — HSL corrects colours that are *in*
    // the picture while a grade is laid on top.
    egui::CollapsingHeader::new("Color Grading")
        .id_salt("mask_color_grading")
        .default_open(false)
        .show(ui, |ui| {
            theme::segmented(
                ui,
                grade,
                &color_wheel::GradeRange::ALL.map(|r| (r, r.label())),
            );
            let diameter = ui.available_width().min(188.0);
            {
                // Scoped so the borrow of `color_grading` ends before Blending and Balance,
                // which need the struct itself.
                color_wheel::show(ui, &mut a.color_grading, *grade, diameter, ed);
                let w = grade.wheel_mut(&mut a.color_grading);
                ed.slider(ui, &mut w.hue, 0.0..=360.0, "Hue");
                ed.slider(ui, &mut w.saturation, 0.0..=100.0, "Saturation");
                ed.slider(ui, &mut w.luminance, -100.0..=100.0, "Luminance");
            }
            ui.separator();
            let g = &mut a.color_grading;
            // ⚠⚠ **Blending and Balance are carried inside a mask, and that is parity rather
            // than padding.** They are properties of the *range decomposition* shared by all
            // four wheels, so omitting them would make a masked grade a different control from
            // the global one wearing the same name — the failure the Whites slider already cost
            // this project once.
            ed.slider(ui, &mut g.blending, 0.0..=100.0, "Blending");
            ed.slider(ui, &mut g.balance, -100.0..=100.0, "Balance");
        });

    ui.separator();
    // ⚠ Above Sharpening, as in the global panel: you must denoise before you can judge
    // sharpening. A deliberate divergence from Lightroom, which puts Sharpening first.
    //
    // ⚠ This block and Sharpening below it each carry a slider labelled **"Detail"**, which is
    // the same adjacency the global Detail section already ships — the sub-labels are what tell
    // them apart. Parity with the global panel wins over uniqueness here; the alternative would
    // be naming the same control differently in two places.
    theme::sub_label(ui, "Noise Reduction");
    ed.slider(
        ui,
        &mut a.noise_reduction.luminance,
        0.0..=100.0,
        "Luminance",
    );
    ed.slider(ui, &mut a.noise_reduction.detail, 0.0..=100.0, "Detail");
    ed.slider(ui, &mut a.noise_reduction.color, 0.0..=100.0, "Color");
    ed.slider(
        ui,
        &mut a.noise_reduction.smoothness,
        0.0..=100.0,
        "Smoothness",
    );

    theme::sub_label(ui, "Sharpening");
    // ⚠⚠ Full parity with the global control, `Masking` included — which here is a second gate
    // inside a gate, since the mask has already answered *where*. Kept deliberately (2026-09-04)
    // rather than trimmed: parity costs nothing, and an edge gate is defensible for a loose mask
    // over mixed content.
    ed.slider(ui, &mut a.sharpening.amount, 0.0..=100.0, "Amount");
    ed.slider(ui, &mut a.sharpening.radius, 0.5..=3.0, "Radius");
    ed.slider(ui, &mut a.sharpening.detail, 0.0..=100.0, "Detail");
    ed.slider(ui, &mut a.sharpening.masking, 0.0..=100.0, "Masking");

    curve_rect
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_ui;

    /// Draw the panel as it ships, into a scratch set of adjustments.
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
            // ⚠⚠ **Animation off, and it is load-bearing.** A `CollapsingHeader` animates its
            // body open over `animation_time`, driven by wall-clock `stable_dt` — and a headless
            // pass advances almost no time at all. Left on, the body is still at zero height
            // when the drag lands, so the press hits nothing and the test passes for the wrong
            // reason: exactly the failure mode `a_left_drag_does_not_pan` had.
            // ⚠ `style_mut_of`, per theme — `Context::style_mut` does not exist in egui 0.35,
            // which `theme::apply` already had to learn. Both themes, so the harness cannot
            // depend on which one a default `Context` happens to start in.
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

        /// Open a collapsed block by clicking its header, the way a user does.
        ///
        /// ⚠ `test_ui::click` panics when the label was never drawn, so this also asserts the
        /// block is really wired into `show` rather than merely existing somewhere.
        fn open(&mut self, label: &str) {
            let (a, ed, ch, gr, rect) = (
                &mut self.a,
                &mut self.ed,
                &mut self.channel,
                &mut self.grade,
                &mut self.rect,
            );
            test_ui::click(&self.ctx, label, |ui| {
                *rect = show(ui, a, color::wb::Illuminant::default(), ch, gr, ed);
            });
        }

        /// Drag across the tone-curve editor, in the editor's own normalised display space.
        ///
        /// ⚠⚠ **Five passes, and the split between them is what makes this harness honest.**
        /// Lay out; move to `from`; press *while still there*; move to `to`; release. egui
        /// resolves interaction against the widgets registered on the **previous** pass, so a
        /// harness that moves and presses on the same pass lands its press away from the target.
        ///
        /// ⚠ No screen coordinate is written down: the rect comes back from `show`.
        fn drag_curve(&mut self, from: [f32; 2], to: [f32; 2]) {
            let at = |r: egui::Rect, n: [f32; 2]| {
                egui::pos2(r.left() + n[0] * r.width(), r.bottom() - n[1] * r.height())
            };
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
                let (a, ed, ch, gr, rect) = (
                    &mut self.a,
                    &mut self.ed,
                    &mut self.channel,
                    &mut self.grade,
                    &mut self.rect,
                );
                let _ = self.ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(test_ui::screen()),
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        *rect = show(ui, a, color::wb::Illuminant::default(), ch, gr, ed);
                    },
                );
            }
        }
        /// Drag a slider right by a quarter of its width, located by label.
        ///
        /// ⚠⚠ **Five passes, for `drag_curve`'s reason**: egui resolves interaction against the
        /// widgets registered on the *previous* pass, so a harness that moves and presses on one
        /// pass lands its press away from the target and the test then passes for the wrong
        /// reason.
        ///
        /// ⚠ The target is located by label rather than by coordinate, so a layout change moves
        /// it with the widget instead of silently missing it. Panics if it was never drawn,
        /// which is what makes "the slider exists at all" part of the assertion.
        fn drag_slider(&mut self, label: &str) {
            let mut at = egui::Pos2::ZERO;
            for pass in 0..5 {
                let mut events = Vec::new();
                let to = at + egui::vec2(40.0, 0.0);
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
                let (a, ed, ch, gr, rect) = (
                    &mut self.a,
                    &mut self.ed,
                    &mut self.channel,
                    &mut self.grade,
                    &mut self.rect,
                );
                let out = self.ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(test_ui::screen()),
                        events,
                        ..Default::default()
                    },
                    |ui| {
                        *rect = show(ui, a, color::wb::Illuminant::default(), ch, gr, ed);
                    },
                );
                if pass == 0 {
                    let found = out
                        .platform_output
                        .accesskit_update
                        .as_ref()
                        .and_then(|u| {
                            u.nodes.iter().find_map(|(_, n)| {
                                let b = n.bounds()?;
                                (n.label()? == label).then(|| {
                                    egui::pos2(
                                        (b.x0 + b.x1) as f32 / 2.0,
                                        (b.y0 + b.y1) as f32 / 2.0,
                                    )
                                })
                            })
                        })
                        .unwrap_or_else(|| panic!("no slider labelled {label:?} was drawn"));
                    at = found;
                }
            }
        }
    }

    /// ⚠⚠ **The carrier test for the capture corrections.** The pipeline half is proven by the
    /// prefix-order tests; nothing there proves the panel's sliders reach `LocalAdjustments`.
    /// W5 shipped a milestone whose maths was fully proven and which was 100% non-functional for
    /// exactly that reason.
    ///
    /// ⚠ **"Dehaze", "Color" and "Smoothness" are named deliberately, because they are unique in
    /// this panel.** "Detail" appears twice (noise reduction's and Sharpening's) and "Luminance"
    /// twice (noise reduction's and the colour grade's), so a test naming either could pass
    /// against the wrong slider — `test_ui` takes the first match.
    #[test]
    fn the_capture_sliders_reach_the_masks_own_fields() {
        /// A slider's label and the field it must reach.
        type Case = (&'static str, fn(&LocalAdjustments) -> f32);
        let cases: [Case; 3] = [
            ("Dehaze", |a| a.dehaze.amount),
            ("Color", |a| a.noise_reduction.color),
            ("Smoothness", |a| a.noise_reduction.smoothness),
        ];
        for (label, read) in cases {
            let mut h = Harness::new();
            let before = read(&h.a);
            h.drag_slider(label);
            assert_ne!(
                read(&h.a),
                before,
                "dragging {label} never reached the mask's own field"
            );
        }
    }

    /// ⚠⚠ **The carrier test, and it is the one that matters.** The pipeline half of this
    /// milestone is proven by `a_masked_tone_curve_runs_after_the_tone_regions_and_before_local_contrast`;
    /// nothing there proves the panel's value ever reaches `LocalAdjustments`. W5 shipped a
    /// milestone whose maths was fully proven and which was 100% non-functional for exactly that
    /// reason — four passing pipeline tests, and nothing carrying the data to them.
    #[test]
    fn dragging_the_masked_curve_reaches_the_masks_own_tone_curve() {
        let mut h = Harness::new();
        h.open("Tone Curve");
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

    /// ⚠ Both blocks start closed. A mask's section is already the tallest thing in the panel,
    /// and this is the whole reason they are collapsible rather than inline.
    #[test]
    fn both_blocks_start_collapsed() {
        let mut h = Harness::new();
        let (a, ed, ch, gr, rect) = (
            &mut h.a,
            &mut h.ed,
            &mut h.channel,
            &mut h.grade,
            &mut h.rect,
        );
        let labels = test_ui::labelled_centres(&h.ctx, Vec::new(), |ui| {
            *rect = show(ui, a, color::wb::Illuminant::default(), ch, gr, ed);
        });
        let has = |l: &str| labels.iter().any(|(n, _)| n == l);
        assert!(
            has("Tone Curve"),
            "the header must be drawn even when closed"
        );
        assert!(has("Color Grading"), "likewise");
        assert!(
            !h.rect.is_positive(),
            "a closed block must not lay its body out"
        );
        // ⚠ Named against a control that only exists *inside* the grading block, so this cannot
        // be satisfied by the sliders the mask draws unconditionally.
        assert!(
            !has("Blending"),
            "the grading block's contents must be hidden while it is closed"
        );
    }

    /// ⚠ The mask's grade range must be its own. Sharing the global panel's would silently change
    /// which wheel the global panel shows when you switch back to it — the reason
    /// `mask_hsl_channel` exists, applied to the second piece of view state a mask now carries.
    #[test]
    fn drawing_the_panel_leaves_the_masks_own_grade_range_alone() {
        let mut h = Harness::new();
        h.grade = color_wheel::GradeRange::Highlights;
        h.open("Color Grading");
        assert_eq!(
            h.grade,
            color_wheel::GradeRange::Highlights,
            "drawing the panel must not reset the mask's own selected wheel"
        );
    }
}
