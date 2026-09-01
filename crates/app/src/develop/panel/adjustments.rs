//! The five edit-group sections of the Develop panel.

use crate::develop::panel::color_wheel;
use crate::develop::panel::tone_curve;
use crate::develop::panel::Edited;
use edit_model::HslBand;
use eframe::egui;
use rustroom_app::wb_tool::{self, WbPreset};

/// Which of an [`HslBand`]'s three values the HSL selector is showing.
///
/// ⚠ View state, never part of `Edits` — it must not reach a sidecar, a paste or a preset,
/// for the same reason `Session::selected_mask` does not.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum HslChannel {
    #[default]
    Hue,
    Saturation,
    Luminance,
}

impl HslChannel {
    pub const ALL: [HslChannel; 3] = [Self::Hue, Self::Saturation, Self::Luminance];

    pub fn label(self) -> &'static str {
        match self {
            Self::Hue => "Hue",
            Self::Saturation => "Saturation",
            Self::Luminance => "Luminance",
        }
    }

    /// The band field this channel edits.
    pub fn value_mut(self, band: &mut HslBand) -> &mut f32 {
        match self {
            Self::Hue => &mut band.hue,
            Self::Saturation => &mut band.saturation,
            Self::Luminance => &mut band.luminance,
        }
    }
}

/// What the Detail section's lens line reads.
///
/// A free function rather than inline formatting so the wording is pinned by a test — the
/// no-match case is the one a user will actually hit, and it must not be silent. The commonest
/// reason the controls do nothing is an unlisted lens, and the only way to act on that is to be
/// told which name failed to match.
pub fn lens_status(lens: Option<&str>, profile_found: bool) -> String {
    match (lens, profile_found) {
        (None, _) => "No lens recorded in this file".to_string(),
        (Some(name), false) => format!("No profile for {name}"),
        (Some(name), true) => name.to_string(),
    }
}

/// A panel section, identified by its index into [`edit_model::EDIT_GROUP_NAMES`].
///
/// ⚠ The name is *read from* that table, never written here. The panel and the Paste / Sync
/// / Save-preset dialogs then cannot disagree about what a group is called, and renaming a
/// group updates the header for free.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Section(usize);

impl Section {
    pub fn name(self) -> &'static str {
        edit_model::EDIT_GROUP_NAMES[self.0]
    }

    /// This section's index into [`edit_model::EDIT_GROUP_NAMES`].
    pub fn index(self) -> usize {
        self.0
    }
}

/// The edit groups this panel shows — exactly the five in [`SECTIONS`].
///
/// ⚠ **Derived from `SECTIONS`, never listed a second time.** It is what "Reset adjustments"
/// resets, so deriving it makes the button provably reset what the panel shows and nothing
/// else. A hand-written second list would drift the moment a section moved.
pub fn edit_groups() -> edit_model::EditGroups {
    let mut g = edit_model::EditGroups::NONE;
    for s in SECTIONS {
        *g.get_mut(s.index()) = true;
    }
    g
}

/// The five adjustment sections, in the order the decisions are made in.
///
/// ⚠ **Deliberately not pipeline order.** The pipeline runs noise reduction and dehaze
/// *before* white balance, because that is where the maths wants them; NR is *judged* last,
/// at 1:1, once the tone is settled. The panel follows the workflow. Do not "fix" the
/// mismatch — Lightroom's panels diverge from its pipeline for the same reason.
///
/// Geometry and Masks are absent because they are rail tools — see `GEOMETRY_GROUP` and
/// `MASKS_GROUP` for their indices rather than writing numbers here.
pub const SECTIONS: [Section; 6] = [
    Section(0), // White Balance
    Section(1), // Tone
    Section(2), // Color
    Section(3), // Detail
    Section(4), // Lens
    Section(5), // Vignette
];

/// Draw the five adjustment sections.
///
/// ⚠ Takes `hsl_channel` as its own parameter rather than reaching through the app: the
/// caller holds `&mut app.session` at the same time, and Rust's disjoint field borrows make
/// two separate `&mut` parameters legal where a closure capturing `app` twice is `E0499`.
/// That is the second reason this file exists.
/// What the panel needs to know about the file behind the edits.
///
/// ⚠ A struct because `show` reached eight arguments and clippy's `too_many_arguments` objected —
/// the fourth time this project has answered that with a params struct, after `FrameSpec`,
/// `NameField` and `RenderSpec`. Every time the note was that clippy was right. It groups the
/// three things that are facts about the *source* rather than about the edits.
pub struct SourceInfo<'a> {
    /// The lens the shot was taken with, whether or not a profile matched it.
    pub lens: Option<&'a str>,
    pub lens_profile_found: bool,
    /// The illuminant the decode left the photo balanced to.
    pub wb_baseline: color::wb::Illuminant,
}

/// The white-balance controls: a temperature, a tint, and the named illuminants.
///
/// ⚠⚠ **The Kelvin slider is only drawn once a temperature exists**, and its absence is the
/// control for "As Shot": a photo following its file has no number to show, and inventing one
/// would freeze the value the moment anybody touched the slider. The As Shot button is how you
/// get back.
///
/// ⚠ The slider is **linear in mireds**, not in Kelvin. Equal steps in mireds are equal
/// perceptual steps, so a Kelvin-linear slider spends most of its travel in the blues where
/// nothing changes and crams the whole tungsten range into its first few pixels.
pub(crate) fn white_balance(
    ui: &mut egui::Ui,
    wb: &mut edit_model::WhiteBalance,
    baseline: color::wb::Illuminant,
    // ⚠ `Option`, not a scratch `&mut false`: a mask has no eyedropper, because solving one
    // against a region is a different question from solving it against the frame. `None` says
    // that rather than drawing a control that quietly does nothing.
    picking: Option<&mut bool>,
    ed: &mut Edited,
) {
    // ⚠⚠ **The sliders are ALWAYS drawn, and Temp shows the baseline while `kelvin` is `None`.**
    // An earlier version hid the slider behind a "Set a temperature" button, on the reasoning that
    // a photo following its file has no number to show. In use that reads as the control being
    // absent rather than as a state. Showing the *effective* temperature is both honest — it is
    // what the photo is balanced to — and self-explanatory: drag it and it becomes yours.
    //
    // ⚠ `None` survives until the slider is actually moved, which is what keeps the photo
    // following its file. Merely *looking* at the panel must not freeze a number nobody chose.
    //
    // Mireds ascend as Kelvin descends, so the slider is drawn over the *reciprocal* and the range
    // reads warm-on-the-left the way every other temperature control does.
    let mut mired = 1.0e6 / wb_tool::effective_kelvin(wb, baseline);
    let r = ui.add(
        egui::Slider::new(&mut mired, MIRED_MIN..=MIRED_MAX)
            .custom_formatter(|m, _| format!("{:.0} K", 1.0e6 / m))
            .custom_parser(|s| {
                s.trim()
                    .trim_end_matches(['K', 'k', ' '])
                    .parse::<f64>()
                    .ok()
                    .filter(|k| *k > 0.0)
                    .map(|k| 1.0e6 / k)
            })
            .text("Temp"),
    );
    if r.changed() {
        wb.kelvin = Some(1.0e6 / mired);
    }
    ed.absorb(&r);
    ed.slider(ui, &mut wb.tint, -100.0..=100.0, "Tint");

    if let Some(picking) = picking {
        // ⚠ A toggle, not a button: an armed eyedropper changes what a click on the canvas does,
        // and a mode you cannot see is the complaint the crop aspect lock was fixed for. The
        // canvas puts a crosshair under the pointer while it is on, which is the other half of
        // saying so.
        if ui
            .selectable_label(*picking, "Pick a neutral")
            .on_hover_text("Click something in the photo that should be grey")
            .clicked()
        {
            *picking = !*picking;
        }
    }

    // ⚠⚠ **A menu, not a row of chips.** The presets used to sit above the sliders, which made
    // the *named* illuminants look like the primary control and the temperature like a detail —
    // backwards. They are shortcuts that move the slider, so they belong behind one word.
    ui.menu_button("Presets", |ui| {
        for p in WbPreset::ALL {
            // ⚠ `selectable_label`, not a button: which preset a photo is sitting on is real
            // state, and a menu that gives no sign of it makes you guess.
            //
            // ⚠ The match is on the **effective** temperature, so a slider dragged onto a
            // preset's value lights that preset up. That is honest — the two are the same edit.
            let on = match p.kelvin() {
                None => wb.kelvin.is_none(),
                Some(k) => wb.kelvin.is_some_and(|v| (v - k).abs() < 0.5),
            };
            if ui.selectable_label(on, p.label()).clicked() {
                wb_tool::apply_preset(wb, p);
                ed.mark_changed();
            }
        }
    })
    .response
    .on_hover_text("Named illuminants — each one just moves the Temp slider");
}

/// 2000 K – 25000 K, as mireds. ⚠ Stored the way the slider draws them, so the two cannot
/// disagree about which end is warm.
const MIRED_MIN: f32 = 1.0e6 / 25000.0;
const MIRED_MAX: f32 = 1.0e6 / 2000.0;

pub fn show(
    ui: &mut egui::Ui,
    edits: &mut edit_model::Edits,
    hsl_channel: &mut HslChannel,
    grade_range: &mut color_wheel::GradeRange,
    source: SourceInfo<'_>,
    picking: Option<&mut bool>,
    ed: &mut Edited,
) {
    let SourceInfo {
        lens,
        lens_profile_found,
        wb_baseline: baseline,
    } = source;
    // White Balance and Tone open by default; the rest start collapsed, because a panel
    // that opens with 30 sliders showing is the problem this section structure removes.
    section(ui, SECTIONS[0], true, |ui| {
        white_balance(ui, &mut edits.white_balance, baseline, picking, ed);
    });

    section(ui, SECTIONS[1], true, |ui| {
        ed.slider(ui, &mut edits.exposure, -5.0..=5.0, "Exposure");
        ed.slider(ui, &mut edits.contrast, -100.0..=100.0, "Contrast");
        ed.slider(ui, &mut edits.highlights, -100.0..=100.0, "Highlights");
        ed.slider(ui, &mut edits.shadows, -100.0..=100.0, "Shadows");
        ed.slider(ui, &mut edits.whites, -100.0..=100.0, "Whites");
        ed.slider(ui, &mut edits.blacks, -100.0..=100.0, "Blacks");
        // ⚠ A hairline, not a header. These are Lightroom's "Presence" block and they are
        // local-contrast tools rather than tonal ones — but they belong to the `Tone` edit
        // group, and a header here would put the panel and the Sync dialog out of step.
        ui.separator();
        ed.slider(
            ui,
            &mut edits.local_contrast.texture,
            -100.0..=100.0,
            "Texture",
        );
        ed.slider(
            ui,
            &mut edits.local_contrast.clarity,
            -100.0..=100.0,
            "Clarity",
        );
        ed.slider(ui, &mut edits.dehaze.amount, -100.0..=100.0, "Dehaze");
        // ⚠ Last in the section, and a hairline rather than a header — `tone_curve` belongs to
        // the `Tone` edit group, and a header here would put the panel and the Sync dialog out
        // of step, which is the ambiguity this section structure exists to remove. It comes
        // after the sliders because that is the order the decisions are made in: set the tone
        // broadly, then refine its shape.
        ui.separator();
        tone_curve::show(ui, &mut edits.tone_curve, ed);
    });

    section(ui, SECTIONS[2], false, |ui| {
        // ⚠ Vibrance first: Adobe's order, and the gentler control. Saturation clips
        // already-saturated colour far more readily, so it is the one offered second.
        ed.slider(ui, &mut edits.vibrance, -100.0..=100.0, "Vibrance");
        ed.slider(ui, &mut edits.saturation, -100.0..=100.0, "Saturation");
        ui.separator();
        // ⚠ A selector, not three nested CollapsingHeaders. Nesting cost a third level *and*
        // put a "Saturation" header under a "Saturation" slider meaning something else.
        ui.horizontal(|ui| {
            for c in HslChannel::ALL {
                ui.selectable_value(hsl_channel, c, c.label());
            }
        });
        for (i, name) in edit_model::HSL_BAND_NAMES.iter().enumerate() {
            ed.slider(
                ui,
                hsl_channel.value_mut(&mut edits.hsl.bands[i]),
                -100.0..=100.0,
                name,
            );
        }

        // ⚠ A hairline and a plain label, not a nested CollapsingHeader — the idiom already
        // used for the Presence block inside Tone. Colour grading belongs to the `Color` edit
        // group, and a header here would put the panel and the Sync dialog out of step.
        ui.separator();
        ui.label("Color Grading");
        ui.horizontal(|ui| {
            for r in color_wheel::GradeRange::ALL {
                ui.selectable_value(grade_range, r, r.label());
            }
        });
        let diameter = ui.available_width().min(188.0);
        {
            // Scoped so the borrow of `color_grading` ends before Blending and Balance,
            // which need the struct itself.
            let w = grade_range.wheel_mut(&mut edits.color_grading);
            color_wheel::show(ui, w, diameter, ed);
            ed.slider(ui, &mut w.hue, 0.0..=360.0, "Hue");
            ed.slider(ui, &mut w.saturation, 0.0..=100.0, "Saturation");
            ed.slider(ui, &mut w.luminance, -100.0..=100.0, "Luminance");
        }
        ui.separator();
        let g = &mut edits.color_grading;
        ed.slider(ui, &mut g.blending, 0.0..=100.0, "Blending");
        ed.slider(ui, &mut g.balance, -100.0..=100.0, "Balance");
    });

    section(ui, SECTIONS[3], false, |ui| {
        // ⚠ Noise Reduction above Sharpening: you must denoise before you can judge
        // sharpening, and it is the pipeline's own order. A deliberate divergence from
        // Lightroom, which puts Sharpening first.
        ui.label("Noise Reduction");
        let nr = &mut edits.noise_reduction;
        ed.slider(ui, &mut nr.luminance, 0.0..=100.0, "Luminance");
        ed.slider(ui, &mut nr.detail, 0.0..=100.0, "Detail");
        ed.slider(ui, &mut nr.color, 0.0..=100.0, "Color");
        ed.slider(ui, &mut nr.smoothness, 0.0..=100.0, "Smoothness");
        ui.separator();
        ui.label("Sharpening");
        let sh = &mut edits.sharpening;
        ed.slider(ui, &mut sh.amount, 0.0..=100.0, "Amount");
        // The one slider not on a 0..100 scale: a physical quantity in full-resolution
        // pixels, and naming it in pixels is what makes "scaled for the render" legible.
        ed.slider(ui, &mut sh.radius, 0.5..=3.0, "Radius (px)");
        ed.slider(ui, &mut sh.detail, 0.0..=100.0, "Detail");
        ed.slider(ui, &mut sh.masking, 0.0..=100.0, "Masking");
    });

    // ⚠ Its own section rather than a block inside Detail, and its own edit group with it. They
    // were bundled at first on the grounds that Detail is the capture-specific group — true, but
    // it meant syncing noise reduction also synced lens corrections. Separating them makes
    // "copy this photo's lens corrections" its own choice everywhere `EditGroups` is offered.
    //
    // ⚠ Placed after Detail, not first, despite running first in the pipeline: it is on by
    // default and visited only to turn something *off*, so depth costs nothing. Panel follows
    // workflow, pipeline follows maths — the rule `SECTIONS`' own doc comment states.
    section(ui, SECTIONS[4], false, |ui| {
        ui.label(
            egui::RichText::new(lens_status(lens, lens_profile_found))
                .small()
                .weak(),
        );
        // ⚠ Disabled rather than hidden: a greyed row that names the unmatched lens explains
        // itself, and an absent row does not.
        ui.add_enabled_ui(lens_profile_found, |ui| {
            let l = &mut edits.lens_correction;
            if ui.checkbox(&mut l.distortion, "Distortion").changed() {
                ed.discrete();
            }
            if ui.checkbox(&mut l.tca, "Chromatic aberration").changed() {
                ed.discrete();
            }
            // ⚠ Not the creative Vignette, which stays in Effects. This one is corrective and
            // pre-crop; that one is creative and post-crop. The section heading keeps them apart.
            if ui.checkbox(&mut l.vignetting, "Vignetting").changed() {
                ed.discrete();
            }
            // ⚠ Also surfaced in the **Crop** tool, and that is not duplication for its own sake:
            // Scale is only *judgeable* against the uncropped frame, which is what crop mode
            // shows. Here it is often invisible — if you have cropped at all, the empty corners
            // it removes are already outside your crop.
            //
            // ⚠ "Scale", not "Crop": only *pincushion* correction empties the frame, and barrel
            // correction's autoscale is exactly 1, so on many photos this does nothing at all.
            ed.slider(ui, &mut l.scale, 0.0..=100.0, "Scale");
        });
    });

    section(ui, SECTIONS[5], false, |ui| {
        let v = &mut edits.vignette;
        ed.slider(ui, &mut v.amount, -100.0..=100.0, "Amount");
        ed.slider(ui, &mut v.midpoint, 0.0..=100.0, "Midpoint");
        ed.slider(ui, &mut v.roundness, -100.0..=100.0, "Roundness");
        ed.slider(ui, &mut v.feather, 0.0..=100.0, "Feather");
    });
}

/// One collapsible section, headed by its edit group's name.
fn section(ui: &mut egui::Ui, s: Section, open_by_default: bool, body: impl FnOnce(&mut egui::Ui)) {
    egui::CollapsingHeader::new(s.name())
        .default_open(open_by_default)
        .show(ui, body);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⚠⚠ **Drawing the panel must not freeze a temperature.** The slider is always visible and
    /// shows the baseline while `kelvin` is `None`, so the obvious implementation — write the
    /// slider's value back every frame — would turn "this photo follows its file" into a hard
    /// number the moment anybody opened the panel, on every photo, with nothing to notice it.
    /// Only `changed()` may commit.
    #[test]
    fn merely_drawing_the_panel_leaves_the_temperature_following_the_file() {
        let ctx = crate::test_ui::ctx();
        let mut wb = edit_model::WhiteBalance::default();
        let base = color::wb::Illuminant::new(4200.0, 0.0);
        for _ in 0..3 {
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(crate::test_ui::screen()),
                    ..Default::default()
                },
                |ui| {
                    let mut ed = Edited::default();
                    white_balance(ui, &mut wb, base, None, &mut ed);
                    assert!(!ed.changed, "drawing the panel reported an edit");
                },
            );
        }
        assert_eq!(wb.kelvin, None, "the panel froze a temperature nobody set");
    }

    #[test]
    fn each_channel_edits_its_own_field() {
        // ⚠ The load-bearing test of the whole selector. Every value is distinct, and each
        // is written through `value_mut` and then read from the *other two* fields as well —
        // so a swapped match arm cannot pass by coincidence, which asserting one field at a
        // time would allow.
        let mut b = HslBand::default();
        *HslChannel::Hue.value_mut(&mut b) = 11.0;
        *HslChannel::Saturation.value_mut(&mut b) = 22.0;
        *HslChannel::Luminance.value_mut(&mut b) = 33.0;
        assert_eq!(b.hue, 11.0, "Hue wrote the wrong field");
        assert_eq!(b.saturation, 22.0, "Saturation wrote the wrong field");
        assert_eq!(b.luminance, 33.0, "Luminance wrote the wrong field");
    }

    #[test]
    fn every_channel_has_a_distinct_label() {
        // A table over ALL, so a fourth channel cannot ship sharing a label with another —
        // the selector would then have two identical-looking segments.
        let mut seen: Vec<&str> = HslChannel::ALL.iter().map(|c| c.label()).collect();
        assert_eq!(seen.len(), 3);
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), 3, "two channels share a label");
    }

    #[test]
    fn all_lists_every_variant() {
        // ⚠ `ALL` is what the selector iterates. A variant missing from it is unreachable in
        // the UI while compiling perfectly — exactly the kind of silent gap that ships.
        for c in HslChannel::ALL {
            // A match with no wildcard: adding a variant breaks this, forcing a decision.
            match c {
                HslChannel::Hue | HslChannel::Saturation | HslChannel::Luminance => {}
            }
        }
        assert_eq!(HslChannel::ALL.len(), 3);
    }

    #[test]
    fn the_panel_sections_are_the_edit_groups() {
        // ⚠ The point of the whole restructure: the panel teaches the same five words the
        // Paste, Sync and Save-preset dialogs already use. Reading them out of
        // EDIT_GROUP_NAMES rather than hardcoding strings is what makes that true by
        // construction — rename a group and the header follows, instead of drifting.
        let names: Vec<&str> = SECTIONS.iter().map(|s| s.name()).collect();
        assert_eq!(
            names,
            vec![
                "White Balance",
                "Tone",
                "Color",
                "Detail",
                "Lens",
                "Vignette"
            ]
        );
    }

    #[test]
    fn geometry_and_masks_are_the_two_groups_the_panel_does_not_show() {
        // They are rail tools, not adjustment sections. Stated as a property over the full
        // group list so that a *seventh* group added later cannot silently go missing from
        // the panel with nothing failing.
        let shown: Vec<&str> = SECTIONS.iter().map(|s| s.name()).collect();
        let missing: Vec<&str> = edit_model::EDIT_GROUP_NAMES
            .iter()
            .copied()
            .filter(|n| !shown.contains(n))
            .collect();
        assert_eq!(missing, vec!["Geometry", "Masks"]);
    }

    /// Click the segment labelled `want` and report the channel afterwards.
    ///
    /// Three passes: lay out with the pointer in place, press, release — egui resolves a
    /// click against the widgets registered on the *previous* pass, which is why a
    /// single-pass harness reads zeros and passes vacuously. That exact harness bug made
    /// `a_left_drag_does_not_pan` inert once already.
    fn click_segment(want: HslChannel) -> HslChannel {
        let ctx = egui::Context::default();
        let mut channel = HslChannel::Hue;
        let mut target = egui::Pos2::ZERO;
        for pass in 0..3 {
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 400.0),
                )),
                ..Default::default()
            };
            input.events.push(egui::Event::PointerMoved(target));
            if pass == 1 || pass == 2 {
                input.events.push(egui::Event::PointerButton {
                    pos: target,
                    button: egui::PointerButton::Primary,
                    pressed: pass == 1,
                    modifiers: Default::default(),
                });
            }
            let _ = ctx.run_ui(input, |ui| {
                ui.horizontal(|ui| {
                    for c in HslChannel::ALL {
                        let r = ui.selectable_value(&mut channel, c, c.label());
                        if c == want && pass == 0 {
                            target = r.rect.center();
                        }
                    }
                });
            });
        }
        channel
    }

    #[test]
    fn clicking_a_segment_switches_the_channel() {
        // ⚠ `value_mut` being right proves nothing about the segments being wired to it.
        // Every non-default channel is exercised, because a selector stuck on its default
        // would pass a test that only ever clicked Hue.
        assert_eq!(
            click_segment(HslChannel::Saturation),
            HslChannel::Saturation
        );
        assert_eq!(click_segment(HslChannel::Luminance), HslChannel::Luminance);
        assert_eq!(click_segment(HslChannel::Hue), HslChannel::Hue);
    }

    #[test]
    fn resetting_the_adjustments_leaves_geometry_and_masks_alone() {
        // ⚠ The property that makes a *scoped* reset safe: under an exclusive panel, each tool
        // owns its own domain, so the Edit tool's reset must not throw away a crop or a
        // painted mask the user cannot even see from here.
        let g = edit_groups();
        for s in SECTIONS {
            assert!(
                g.get(s.index()),
                "{} is shown but would not reset",
                s.name()
            );
        }
        let names = edit_model::EDIT_GROUP_NAMES;
        let untouched: Vec<&str> = (0..names.len())
            .filter(|i| !g.get(*i))
            .map(|i| names[i])
            .collect();
        assert_eq!(untouched, vec!["Geometry", "Masks"]);
    }

    #[test]
    fn a_scoped_reset_really_keeps_the_crop() {
        // ⚠ Asserts through `merge_from` itself rather than re-deriving what it should do —
        // the same reason `FrameMap` is tested against `GeometryStage` and not a re-derivation.
        let mut e = edit_model::Edits {
            exposure: 2.0,
            ..Default::default()
        };
        e.geometry.crop.left = 0.25;
        e.masks.push(edit_model::MaskedAdjustment {
            mask: edit_model::Mask::Brush(edit_model::Brush::default()),
            adjustments: edit_model::LocalAdjustments::default(),
        });
        e.merge_from(&edit_model::Edits::default(), edit_groups());
        assert_eq!(e.exposure, 0.0, "the adjustments should be reset");
        assert_eq!(e.geometry.crop.left, 0.25, "the crop must survive");
        assert_eq!(e.masks.len(), 1, "the masks must survive");
    }

    #[test]
    fn the_lens_line_names_what_it_could_not_find() {
        assert_eq!(lens_status(None, false), "No lens recorded in this file");
        assert_eq!(
            lens_status(Some("XF8mmF3.5 R WR"), false),
            "No profile for XF8mmF3.5 R WR"
        );
        assert_eq!(lens_status(Some("XF23mmF2 R WR"), true), "XF23mmF2 R WR");
    }
}
