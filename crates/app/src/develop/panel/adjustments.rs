//! The five edit-group sections of the Develop panel.

use crate::develop::panel::color_wheel;
use crate::develop::panel::Edited;
use edit_model::HslBand;
use eframe::egui;

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
/// Geometry (index 5) and Masks (index 6) are absent because they are rail tools.
pub const SECTIONS: [Section; 5] = [
    Section(0), // White Balance
    Section(1), // Tone
    Section(2), // Color
    Section(3), // Detail
    Section(4), // Effects
];

/// Draw the five adjustment sections.
///
/// ⚠ Takes `hsl_channel` as its own parameter rather than reaching through the app: the
/// caller holds `&mut app.session` at the same time, and Rust's disjoint field borrows make
/// two separate `&mut` parameters legal where a closure capturing `app` twice is `E0499`.
/// That is the second reason this file exists.
pub fn show(
    ui: &mut egui::Ui,
    edits: &mut edit_model::Edits,
    hsl_channel: &mut HslChannel,
    grade_range: &mut color_wheel::GradeRange,
    lens: Option<&str>,
    lens_profile_found: bool,
    ed: &mut Edited,
) {
    // White Balance and Tone open by default; the rest start collapsed, because a panel
    // that opens with 30 sliders showing is the problem this section structure removes.
    section(ui, SECTIONS[0], true, |ui| {
        ed.slider(ui, &mut edits.white_balance.temp, -100.0..=100.0, "Temp");
        ed.slider(ui, &mut edits.white_balance.tint, -100.0..=100.0, "Tint");
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
        // ⚠ First in Detail because it is first in the pipeline: a capture correction to get out
        // of the way before judging noise or sharpening. Detail is this codebase's
        // capture-specific group — "noise, lens, output size".
        ui.label("Lens Corrections");
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
            // pre-crop; that one is creative and post-crop. The enclosing "Lens Corrections"
            // heading is what keeps them apart.
            if ui.checkbox(&mut l.vignetting, "Vignetting").changed() {
                ed.discrete();
            }
            // ⚠ "Scale", not "Crop": only *pincushion* correction empties the frame, and barrel
            // correction's autoscale is exactly 1 — so on many photos this slider does nothing
            // at all, which is correct and would read as a broken crop control.
            ed.slider(ui, &mut l.scale, 0.0..=100.0, "Scale");
        });
        ui.separator();

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

    section(ui, SECTIONS[4], false, |ui| {
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
            vec!["White Balance", "Tone", "Color", "Detail", "Effects"]
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
