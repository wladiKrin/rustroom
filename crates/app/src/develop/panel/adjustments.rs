//! The five edit-group sections of the Develop panel.

use crate::develop::panel::color_wheel;
use crate::develop::panel::hsl_bands;
use crate::develop::panel::tone_curve;
use crate::develop::panel::Edited;
use crate::theme;
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
    /// The camera profile's `BaselineExposure` in stops, when it declares one.
    ///
    /// ⚠ Auto Tone needs it: with the display rendering on, the scene is lifted by this before
    /// the curve, so a solve aiming the *scene* median at `MEDIAN_TARGET` would stack the two
    /// and open every photograph about a stop too bright.
    pub baseline_ev: Option<f32>,
    /// The decoded editing buffer, for Auto Tone's analysis.
    ///
    /// ⚠⚠ **The decode, never the rendered texture.** That texture already carries the current
    /// exposure and tone, so analysing it would make the auto chase its own correction — press
    /// Auto twice and it would keep moving. The white-balance eyedropper reads this same `Arc`
    /// for the same reason, which is why `Session` keeps it at all.
    pub buffer: &'a core_image::ImageBuf,
    /// The looks installed on this machine, and whether this photo's named one loaded.
    ///
    /// ⚠ Read-only here: discovery happens once per photo in `session_flow`, never in this body,
    /// which runs sixty times a second.
    pub looks: &'a rustroom_app::looks::LooksState,
    /// The rendered frame's histogram, for the ghost behind the tone curve. `None` in a
    /// harness that has no render.
    pub hist: Option<&'a [[u32; 256]; 3]>,
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
        super::slider::Slider::new(&mut mired, MIRED_MIN..=MIRED_MAX)
            .text("Temp")
            .custom_formatter(|m| format!("{:.0} K", 1.0e6 / m))
            .custom_parser(|s| {
                s.trim()
                    .trim_end_matches(['K', 'k', ' '])
                    .parse::<f64>()
                    .ok()
                    .filter(|k| *k > 0.0)
                    .map(|k| 1.0e6 / k)
            }),
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

/// The Tone section: Auto, the six tonal sliders, the Presence block and the curve.
///
/// ⚠ Its own function rather than a closure inside [`show`], which had reached 605 lines. The
/// Auto button needs the buffer threaded to it, and threading a parameter through a closure that
/// already captures half the panel is how that file grew in the first place.
pub(crate) fn tone(
    ui: &mut egui::Ui,
    edits: &mut edit_model::Edits,
    buffer: &core_image::ImageBuf,
    baseline_ev: Option<f32>,
    looks: &rustroom_app::looks::LooksState,
    hist: Option<&[[u32; 256]; 3]>,
    ed: &mut Edited,
) {
    // ⚠⚠ **The analysis runs on the click only.** This body runs sixty times a second, and a
    // percentile sort of the buffer per frame is the `apply_view_demosaic` mistake in its most
    // expensive costume yet.
    //
    // ⚠ Its own row rather than squeezed beside the Exposure slider: the panel is 240 pt and a
    // slider there already spends all of it on the rail, its label and its value. Lightroom puts
    // its Auto on its own row in the Tone panel for the same reason.
    // ⚠⚠ **A rendering is not an adjustment, and it sits above the sliders to say so.** Every
    // control below moves a photograph already in a defined state; this one *establishes* that
    // state, turning scene-referred light into something a screen can show. It is also the first
    // thing anyone reaches for when a RAW opens looking flat, which is the other reason it is
    // first.
    let mut rendered = edits.applied_rendering() == edit_model::Rendering::Camera;
    if ui
        .checkbox(&mut rendered, "Camera rendering")
        .on_hover_text(
            "Maps the scene's light onto the display the way the camera would. \
             Off is colorimetric only, which looks flat and is what the CLI renders.",
        )
        .changed()
    {
        // ⚠ Always `Some`: a click is a decision, and *decided off* is what survives a photo
        // switch — `None` would read as never-asked and the preference would switch it back on.
        edits.rendering = Some(if rendered {
            edit_model::Rendering::Camera
        } else {
            edit_model::Rendering::None
        });
        // ⚠ `mark_changed`, not a slider's `changed()`: a click is a complete act, so it is one
        // history entry immediately rather than waiting for a `drag_stopped` that never comes.
        ed.mark_changed();
    }
    // ⚠ Directly under the camera-rendering checkbox, because a `Linear` look *replaces* it:
    // the two controls decide one thing between them and must be read together.
    crate::develop::panel::look::section(ui, edits, looks, ed);
    if ui
        .button("Auto")
        .on_hover_text("Sets Exposure, Whites and Blacks from the histogram")
        .clicked()
    {
        rustroom_app::auto_tone_tool::apply(edits, buffer, baseline_ev);
        // ⚠ `mark_changed`, not a slider's `changed()`: a click is a complete act, so it is one
        // history entry immediately rather than waiting for a `drag_stopped` that never comes.
        // That is what makes Auto exactly one Ctrl+Z — the model the WB presets already follow.
        ed.mark_changed();
    }
    ed.slider(ui, &mut edits.exposure, -5.0..=5.0, "Exposure");
    ed.slider(ui, &mut edits.contrast, -100.0..=100.0, "Contrast");
    ed.slider(ui, &mut edits.highlights, -100.0..=100.0, "Highlights");
    ed.slider(ui, &mut edits.shadows, -100.0..=100.0, "Shadows");
    ed.slider(ui, &mut edits.whites, -100.0..=100.0, "Whites");
    ed.slider(ui, &mut edits.blacks, -100.0..=100.0, "Blacks");
    // ⚠ A sub-label, not a header. These are Lightroom's "Presence" block and they are
    // local-contrast tools rather than tonal ones — but they belong to the `Tone` edit
    // group, and a header here would put the panel and the Sync dialog out of step.
    theme::sub_label(ui, "Presence");
    ed.slider(
        ui,
        &mut edits.local_contrast.texture,
        -100.0..=100.0,
        "Texture",
    )
    .on_hover_text("Fine surface detail. Negative smooths texture while keeping edges sharp; positive crisps detail along edges too.");
    ed.slider(
        ui,
        &mut edits.local_contrast.clarity,
        -100.0..=100.0,
        "Clarity",
    );
    ed.slider(ui, &mut edits.dehaze.amount, -100.0..=100.0, "Dehaze");
    // ⚠ Last in the section, and a sub-label rather than a header — `tone_curve` belongs to
    // the `Tone` edit group, and a header here would put the panel and the Sync dialog out
    // of step, which is the ambiguity this section structure exists to remove. It comes
    // after the sliders because that is the order the decisions are made in: set the tone
    // broadly, then refine its shape. The curve draws its own sub-label row.
    tone_curve::show(ui, &mut edits.tone_curve, ed, hist);
}

/// The colour-grading block's view state: the `Shad │ Mid │ High │ Global` selector and the
/// grade range view's two session fields.
///
/// ⚠ Grouped because the three travel together and `show` was two arguments past clippy's
/// limit with them loose — a fair complaint rather than a lint to silence, the reason
/// `FrameSpec` exists. None of these is an edit: they never reach `Edits` or a sidecar.
pub struct GradeUi<'a> {
    pub range: &'a mut color_wheel::GradeRange,
    /// The "Show range" checkbox.
    pub show_range: &'a mut bool,
    /// What the render request carries — the box AND a non-Global selection, or `None`.
    pub view: &'a mut Option<rustroom_app::grade_view::GradeRangeView>,
}

pub fn show(
    ui: &mut egui::Ui,
    edits: &mut edit_model::Edits,
    hsl_channel: &mut HslChannel,
    grade: GradeUi<'_>,
    source: SourceInfo<'_>,
    picking: Option<&mut bool>,
    ed: &mut Edited,
) {
    let GradeUi {
        range: grade_range,
        show_range: show_grade_range,
        view: grade_view,
    } = grade;
    let SourceInfo {
        lens,
        lens_profile_found,
        wb_baseline: baseline,
        baseline_ev,
        buffer,
        looks,
        hist,
    } = source;
    // White Balance and Tone open by default; the rest start collapsed, because a panel
    // that opens with 30 sliders showing is the problem this section structure removes.
    section(ui, SECTIONS[0], true, edits, ed, |ui, edits, ed| {
        white_balance(ui, &mut edits.white_balance, baseline, picking, ed);
    });

    section(ui, SECTIONS[1], true, edits, ed, |ui, edits, ed| {
        tone(ui, edits, buffer, baseline_ev, looks, hist, ed);
    });

    section(ui, SECTIONS[2], false, edits, ed, |ui, edits, ed| {
        // ⚠ Vibrance first: Adobe's order, and the gentler control. Saturation clips
        // already-saturated colour far more readily, so it is the one offered second.
        ed.slider(ui, &mut edits.vibrance, -100.0..=100.0, "Vibrance");
        ed.slider(ui, &mut edits.saturation, -100.0..=100.0, "Saturation");
        ui.separator();
        // ⚠ Shared with a mask's own HSL block — see `hsl_bands`. One layout learned once.
        hsl_bands::show(ui, &mut edits.hsl, hsl_channel, ed);

        // ⚠ A sub-label, not a nested CollapsingHeader — the idiom already used for the
        // Presence block inside Tone. Colour grading belongs to the `Color` edit group, and a
        // header here would put the panel and the Sync dialog out of step.
        theme::sub_label(ui, "Color Grading");
        theme::segmented(
            ui,
            grade_range,
            &color_wheel::GradeRange::ALL.map(|r| (r, r.label())),
        );
        let diameter = ui.available_width().min(188.0);
        {
            // Scoped so the borrow of `color_grading` ends before Blending and Balance,
            // which need the struct itself.
            color_wheel::show(ui, &mut edits.color_grading, *grade_range, diameter, ed);
            let w = grade_range.wheel_mut(&mut edits.color_grading);
            ed.slider(ui, &mut w.hue, 0.0..=360.0, "Hue");
            ed.slider(ui, &mut w.saturation, 0.0..=100.0, "Saturation");
            ed.slider(ui, &mut w.luminance, -100.0..=100.0, "Luminance");
        }
        ui.separator();
        let g = &mut edits.color_grading;
        // ⚠⚠ **Blending and Balance are shared by all four wheels, and that is correct rather
        // than an oversight** — they are properties of the *range decomposition*, not of any one
        // wheel: Blending sets how far the shadow, midtone and highlight weights overlap and
        // Balance slides all three centres together. Lightroom places them the same way, below
        // the wheels rather than inside one.
        //
        // ⚠ Separated because without it they sit flush against Hue/Saturation/Luminance, which
        // *are* per-wheel. A caption saying so was tried and removed (user's call, 2026-09-02) —
        // the hairline carries it.
        ui.separator();
        ed.slider(ui, &mut g.blending, 0.0..=100.0, "Blending");
        ed.slider(ui, &mut g.balance, -100.0..=100.0, "Balance");
        // ⚠⚠ A VIEW MODE, not an edit — `visualize`'s rule. It writes two session fields and
        // never `Edits`, so it re-renders (`ed.changed`) without a history entry.
        ui.checkbox(show_grade_range, "Show range").on_hover_text(
            "Tints the photograph where the selected range reaches. Affects only what you see — \
             never the render, the export or the histogram.",
        );
        // ⚠ Two inputs, one value on the wire, derived here because this is the one place the
        // checkbox and the selector are both in scope. Change-guarded: this body runs every
        // frame, and an unguarded write would request a render sixty times a second.
        let wanted = show_grade_range.then(|| grade_range.view()).flatten();
        if *grade_view != wanted {
            *grade_view = wanted;
            ed.changed = true;
        }
    });

    section(ui, SECTIONS[3], false, edits, ed, |ui, edits, ed| {
        // ⚠ Noise Reduction above Sharpening: you must denoise before you can judge
        // sharpening, and it is the pipeline's own order. A deliberate divergence from
        // Lightroom, which puts Sharpening first.
        theme::sub_label(ui, "Noise Reduction");
        let nr = &mut edits.noise_reduction;
        ed.slider(ui, &mut nr.luminance, 0.0..=100.0, "Luminance");
        ed.slider(ui, &mut nr.detail, 0.0..=100.0, "Detail");
        ed.slider(ui, &mut nr.color, 0.0..=100.0, "Color");
        ed.slider(ui, &mut nr.smoothness, 0.0..=100.0, "Smoothness");
        theme::sub_label(ui, "Sharpening");
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
    section(ui, SECTIONS[4], false, edits, ed, |ui, edits, ed| {
        ui.label(
            egui::RichText::new(lens_status(lens, lens_profile_found))
                .small()
                .weak(),
        );
        // ⚠ Disabled rather than hidden: a greyed row that names the unmatched lens explains
        // itself, and an absent row does not.
        ui.add_enabled_ui(lens_profile_found, |ui| {
            // ⚠ Read the applied value into a local and write the whole struct back as `Some`
            // on any change: a click decides, and decided-off must not collapse back to
            // undecided. Binding the boxes to an `Option`'s inside would need it to be `Some`
            // first, which would turn merely *looking* at the panel into a decision.
            let mut l = edits.applied_lens_correction();
            let mut touched = false;
            touched |= ui.checkbox(&mut l.distortion, "Distortion").changed();
            touched |= ui.checkbox(&mut l.tca, "Chromatic aberration").changed();
            // ⚠ Not the creative Vignette, which stays in Effects. This one is corrective and
            // pre-crop; that one is creative and post-crop. The section heading keeps them apart.
            touched |= ui.checkbox(&mut l.vignetting, "Vignetting").changed();
            // ⚠ Also surfaced in the **Crop** tool, and that is not duplication for its own sake:
            // Scale is only *judgeable* against the uncropped frame, which is what crop mode
            // shows. Here it is often invisible — if you have cropped at all, the empty corners
            // it removes are already outside your crop.
            //
            // ⚠ "Scale", not "Crop": only *pincushion* correction empties the frame, and barrel
            // correction's autoscale is exactly 1, so on many photos this does nothing at all.
            let scaled = ed.slider(ui, &mut l.scale, 0.0..=100.0, "Scale").changed();
            if touched || scaled {
                edits.lens_correction = Some(l);
            }
            if touched {
                ed.discrete();
            }
        });
    });

    section(ui, SECTIONS[5], false, edits, ed, |ui, edits, ed| {
        let v = &mut edits.vignette;
        ed.slider(ui, &mut v.amount, -100.0..=100.0, "Amount");
        ed.slider(ui, &mut v.midpoint, 0.0..=100.0, "Midpoint");
        ed.slider(ui, &mut v.roundness, -100.0..=100.0, "Roundness");
        ed.slider(ui, &mut v.feather, 0.0..=100.0, "Feather");
    });
}

/// One collapsible section, headed by its edit group's name.
///
/// ⚠ A hand-drawn header over `CollapsingState`, not a `CollapsingHeader`: that widget draws
/// egui's triangle and indents its body, and has no slot for the two things this row carries —
/// an accent dot while the group differs from neutral, and a `reset` for exactly that group.
/// The body goes through `show_body_unindented`, so the left-hand gap under every section is
/// gone with the indent that made it.
///
/// ⚠ The AccessKit label is `s.name()` as written, not the uppercased text the row paints:
/// the harnesses click sections by name ("Color"), and the name is what a screen reader wants.
///
/// ⚠ `edits` and `ed` are passed *through* to the body rather than captured by it, because
/// the header reads `edits` for the dot before the body runs and writes it for the reset after
/// — a closure capturing `&mut edits` for its whole lifetime would be `E0499` against both.
fn section(
    ui: &mut egui::Ui,
    s: Section,
    open_by_default: bool,
    edits: &mut edit_model::Edits,
    ed: &mut Edited,
    body: impl FnOnce(&mut egui::Ui, &mut edit_model::Edits, &mut Edited),
) {
    const HEIGHT: f32 = 22.0;
    const CHEVRON_W: f32 = 14.0;
    let id = ui.make_persistent_id(s.name());
    let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
        ui.ctx(),
        id,
        open_by_default,
    );
    let modified = edits.group_differs(&edit_model::Edits::default(), s.index());

    ui.add_space(6.0);
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), HEIGHT),
        egui::Sense::click(),
    );
    let enabled = ui.is_enabled();
    resp.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::CollapsingHeader, enabled, s.name())
    });

    // ⚠ Registered after the row, so it wins the hit-test tie — egui takes the last widget
    // registered at a point, the rule the Library's rubber band relies on.
    let mut reset = false;
    let mut right = rect.right() - CHEVRON_W;
    let font = theme::section_font(ui.style());
    if modified {
        let weak = ui.visuals().weak_text_color();
        let galley = ui
            .painter()
            .layout_no_wrap("reset".to_owned(), font.clone(), weak);
        let cell = egui::Rect::from_min_max(
            egui::pos2(right - galley.size().x - 8.0, rect.top()),
            egui::pos2(right, rect.bottom()),
        );
        let r = ui
            .interact(cell, id.with("reset"), egui::Sense::click())
            .on_hover_text(format!("Reset {} and nothing else", s.name()));
        r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, "reset"));
        reset = r.clicked();
        let colour = if r.hovered() {
            ui.visuals().strong_text_color()
        } else {
            weak
        };
        ui.painter().galley(
            egui::pos2(cell.left() + 4.0, cell.center().y - galley.size().y / 2.0),
            galley,
            colour,
        );
        right = cell.left() - 6.0;
    }
    if resp.clicked() && !reset {
        state.toggle(ui);
    }

    if ui.is_rect_visible(rect) {
        let v = ui.visuals().clone();
        let painter = ui.painter_at(rect);
        let text_rect = painter.text(
            egui::pos2(rect.left(), rect.center().y),
            egui::Align2::LEFT_CENTER,
            s.name().to_uppercase(),
            font,
            v.strong_text_color(),
        );
        if modified {
            painter.circle_filled(
                egui::pos2(text_rect.right() + 8.0, rect.center().y),
                2.5,
                v.selection.bg_fill,
            );
        }
        // The chevron: '›' closed, rotated to '⌄' as the section opens.
        let openness = state.openness(ui.ctx());
        let rot = egui::emath::Rot2::from_angle(std::f32::consts::FRAC_PI_2 * openness);
        let c = egui::pos2(right + CHEVRON_W / 2.0, rect.center().y);
        let pts = [
            egui::vec2(-2.0, -3.5),
            egui::vec2(1.5, 0.0),
            egui::vec2(-2.0, 3.5),
        ]
        .map(|p| c + rot * p)
        .to_vec();
        painter.add(egui::Shape::line(
            pts,
            egui::Stroke::new(1.4, v.weak_text_color()),
        ));
        painter.line_segment(
            [rect.left_bottom(), rect.right_bottom()],
            egui::Stroke::new(1.0, v.widgets.noninteractive.bg_stroke.color),
        );
    }

    state.show_body_unindented(ui, |ui| body(ui, edits, ed));

    if reset {
        edits.merge_from(
            &edit_model::Edits::default(),
            edit_model::EditGroups::only(s.index()),
        );
        ed.discrete();
    }
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

    /// A buffer with real tails, so the solve engages every field it can.
    fn tone_fixture() -> core_image::ImageBuf {
        let (w, h) = (200usize, 100usize);
        let n = w * h;
        let mut data = Vec::with_capacity(n * 3);
        for i in 0..n {
            let v = 0.05 + 0.25 * (i as f32 / (n - 1) as f32);
            data.extend_from_slice(&[v, v, v]);
        }
        let pin = n / 500;
        for k in 0..pin {
            let dark = (k * 997) % n;
            let bright = (k * 991 + 13) % n;
            for c in 0..3 {
                data[dark * 3 + c] = 0.014;
                data[bright * 3 + c] = 0.93;
            }
        }
        core_image::ImageBuf::from_vec(w, h, data)
    }

    /// ⚠⚠ The load-bearing test of the button: it must write **all three** fields and mark the
    /// edit **once**. An assertion on the exposure alone would pass against a button wired to a
    /// third of the feature, which is exactly the shape M5.4a could ship broken in.
    #[test]
    fn the_auto_button_writes_all_three_and_marks_the_edit_once() {
        let ctx = crate::test_ui::ctx();
        let buf = tone_fixture();
        let mut edits = edit_model::Edits::default();
        let mut ed = Edited::default();
        crate::test_ui::click(&ctx, "Auto", |ui| {
            ed = Edited::default();
            tone(
                ui,
                &mut edits,
                &buf,
                None,
                &Default::default(),
                None,
                &mut ed,
            );
        });
        assert!(ed.changed, "the click must request a render");
        assert!(ed.settled, "the click must push exactly one history entry");
        assert_ne!(edits.exposure, 0.0, "Auto left the exposure alone");
        assert_ne!(edits.whites, 0.0, "Auto left the whites alone");
        assert_ne!(edits.blacks, 0.0, "Auto left the blacks alone");
        // ⚠ And nothing beyond its three fields: Contrast, Highlights and Shadows are a taste
        // nobody asked for, and a wildcard write would look identical in any screenshot.
        assert_eq!(edits.contrast, 0.0);
        assert_eq!(edits.highlights, 0.0);
        assert_eq!(edits.shadows, 0.0);
    }

    /// ⚠⚠ **Both directions, because a checkbox stuck on would pass a one-way test.** The
    /// control writes an enum from a `bool`, so the mapping is the thing that can be wrong, and
    /// only clicking it twice can see it.
    #[test]
    fn the_rendering_checkbox_switches_both_ways_and_marks_the_edit() {
        let ctx = crate::test_ui::ctx();
        let buf = tone_fixture();
        let mut edits = edit_model::Edits::default();
        let mut ed = Edited::default();

        crate::test_ui::click(&ctx, "Camera rendering", |ui| {
            ed = Edited::default();
            tone(
                ui,
                &mut edits,
                &buf,
                None,
                &Default::default(),
                None,
                &mut ed,
            );
        });
        assert_eq!(
            edits.rendering,
            Some(edit_model::Rendering::Camera),
            "ticking it did not turn the rendering on"
        );
        assert!(ed.changed && ed.settled, "one click, one history entry");

        crate::test_ui::click(&ctx, "Camera rendering", |ui| {
            ed = Edited::default();
            tone(
                ui,
                &mut edits,
                &buf,
                None,
                &Default::default(),
                None,
                &mut ed,
            );
        });
        assert_eq!(
            edits.rendering,
            Some(edit_model::Rendering::None),
            "unticking it did not turn the rendering off"
        );
    }

    /// ⚠ The checkbox writes `Some` in both directions. `None` on untick would read as
    /// never-asked, and the preference would switch the rendering back on at the next open —
    /// the exact bug the `Option` exists to end.
    #[test]
    fn unticking_the_rendering_decides_off_rather_than_forgetting() {
        let ctx = crate::test_ui::ctx();
        let buf = tone_fixture();
        let mut edits = edit_model::Edits {
            rendering: Some(edit_model::Rendering::Camera),
            ..Default::default()
        };
        let mut ed = Edited::default();
        crate::test_ui::click(&ctx, "Camera rendering", |ui| {
            ed = Edited::default();
            tone(
                ui,
                &mut edits,
                &buf,
                None,
                &Default::default(),
                None,
                &mut ed,
            );
        });
        assert_eq!(edits.rendering, Some(edit_model::Rendering::None));
        assert_ne!(
            edits,
            edit_model::Edits::default(),
            "decided off must not be neutral"
        );
    }

    /// Same for the lens boxes: unticking the last one leaves `Some(off)`, not `None`.
    ///
    /// ⚠ Drives the whole panel, because the Lens section is drawn inline in `show` and starts
    /// collapsed — so the header is clicked first, with animation off (the `mask_adjust`
    /// harness's rule: left on, the body is still zero-height when the click lands and the
    /// test passes for the wrong reason).
    #[test]
    fn unticking_the_last_lens_correction_decides_off() {
        let ctx = crate::test_ui::ctx();
        for theme in [egui::Theme::Dark, egui::Theme::Light] {
            ctx.style_mut_of(theme, |s| s.animation_time = 0.0);
        }
        let buf = tone_fixture();
        let looks = rustroom_app::looks::LooksState::default();
        let mut edits = edit_model::Edits {
            lens_correction: Some(edit_model::LensCorrection {
                distortion: true,
                ..Default::default()
            }),
            ..Default::default()
        };
        let mut channel = HslChannel::default();
        let mut grade = color_wheel::GradeRange::default();
        let (mut show_range, mut view) = (false, None);
        let mut ed = Edited::default();
        let mut body = |ui: &mut egui::Ui| {
            ed = Edited::default();
            show(
                ui,
                &mut edits,
                &mut channel,
                GradeUi {
                    range: &mut grade,
                    show_range: &mut show_range,
                    view: &mut view,
                },
                SourceInfo {
                    lens: Some("XF23mmF2 R WR"),
                    lens_profile_found: true,
                    wb_baseline: color::wb::Illuminant::default(),
                    baseline_ev: None,
                    buffer: &buf,
                    looks: &looks,
                    hist: None,
                },
                None,
                &mut ed,
            );
        };
        crate::test_ui::click(&ctx, "Lens", &mut body);
        crate::test_ui::click(&ctx, "Distortion", &mut body);
        assert_eq!(
            edits.lens_correction,
            Some(edit_model::LensCorrection::default()),
            "the last untick collapsed to undecided"
        );
        assert!(ed.settled, "one click, one history entry");
    }

    /// The Edit panel with the state the range view reads and writes, held together so a
    /// click's borrows end when the click does.
    struct RangePanel {
        edits: edit_model::Edits,
        channel: HslChannel,
        grade: color_wheel::GradeRange,
        show_range: bool,
        view: Option<rustroom_app::grade_view::GradeRangeView>,
        ed: Edited,
        buf: core_image::ImageBuf,
        looks: rustroom_app::looks::LooksState,
    }

    impl RangePanel {
        fn new(show_range: bool) -> Self {
            Self {
                edits: edit_model::Edits::default(),
                channel: HslChannel::default(),
                grade: color_wheel::GradeRange::default(),
                show_range,
                view: None,
                ed: Edited::default(),
                buf: tone_fixture(),
                looks: rustroom_app::looks::LooksState::default(),
            }
        }

        /// Click `label` in the panel; `ed` reports that one frame.
        fn click(&mut self, ctx: &egui::Context, label: &str) {
            let Self {
                edits,
                channel,
                grade,
                show_range,
                view,
                ed,
                buf,
                looks,
            } = self;
            crate::test_ui::click(ctx, label, |ui| {
                *ed = Edited::default();
                show(
                    ui,
                    edits,
                    channel,
                    GradeUi {
                        range: grade,
                        show_range,
                        view,
                    },
                    SourceInfo {
                        lens: None,
                        lens_profile_found: false,
                        wb_baseline: color::wb::Illuminant::default(),
                        baseline_ev: None,
                        buffer: buf,
                        looks,
                        hist: None,
                    },
                    None,
                    ed,
                );
            });
        }

        /// Lay the panel out once; `f` sees every AccessKit label drawn.
        fn draw(&mut self, ctx: &egui::Context, f: impl FnOnce(&[(String, egui::Pos2)])) {
            let Self {
                edits,
                channel,
                grade,
                show_range,
                view,
                ed,
                buf,
                looks,
            } = self;
            let labels = crate::test_ui::labelled_centres(ctx, Vec::new(), |ui| {
                *ed = Edited::default();
                show(
                    ui,
                    edits,
                    channel,
                    GradeUi {
                        range: grade,
                        show_range,
                        view,
                    },
                    SourceInfo {
                        lens: None,
                        lens_profile_found: false,
                        wb_baseline: color::wb::Illuminant::default(),
                        baseline_ev: None,
                        buffer: buf,
                        looks,
                        hist: None,
                    },
                    None,
                    ed,
                );
            });
            f(&labels);
        }
    }

    #[test]
    fn a_modified_section_shows_its_reset_and_a_neutral_one_does_not() {
        // ⚠ Both halves: a reset that always shows and one that never shows are equally wrong,
        // and only the pair distinguishes them. Found by the `reset` label through AccessKit.
        let ctx = instant_ctx();
        let mut p = RangePanel::new(false);
        let resets = |p: &mut RangePanel, ctx: &egui::Context| {
            let mut n = 0;
            p.draw(ctx, |labels| {
                n = labels.iter().filter(|(l, _)| l == "reset").count()
            });
            n
        };
        assert_eq!(resets(&mut p, &ctx), 0, "a neutral panel offered a reset");
        p.edits.contrast = 30.0;
        assert_eq!(
            resets(&mut p, &ctx),
            1,
            "exactly the Tone section should offer a reset"
        );
    }

    #[test]
    fn resetting_a_section_leaves_the_other_sections_alone() {
        let ctx = instant_ctx();
        let mut p = RangePanel::new(false);
        p.edits.contrast = 30.0;
        p.edits.vibrance = 20.0;
        // ⚠ Both Tone and Color are modified, so two `reset`s are drawn and the harness clicks
        // whichever it finds first. The property is scoping, not which: exactly one of the two
        // groups goes back to neutral and the other is untouched.
        p.click(&ctx, "reset");
        let (c, v) = (p.edits.contrast, p.edits.vibrance);
        assert!(
            (c == 0.0 && v == 20.0) || (c == 30.0 && v == 0.0),
            "a reset must clear exactly its own section: contrast {c}, vibrance {v}"
        );
        assert!(p.ed.settled, "one click, one history entry");
    }

    /// A context whose collapsing headers open on the click, not over an animation.
    fn instant_ctx() -> egui::Context {
        let ctx = crate::test_ui::ctx();
        for theme in [egui::Theme::Dark, egui::Theme::Light] {
            ctx.style_mut_of(theme, |s| s.animation_time = 0.0);
        }
        ctx
    }

    /// ⚠⚠ "Show range" is a VIEW MODE, not an edit — `visualize`'s test, re-run here: it asks
    /// for a render (`ed.changed`), pushes no history entry, and leaves `Edits` untouched.
    #[test]
    fn ticking_show_range_requests_the_selected_range_without_editing_anything() {
        let ctx = instant_ctx();
        let mut p = RangePanel::new(false);
        let before = p.edits.clone();
        p.click(&ctx, "Color");
        p.click(&ctx, "Show range");
        assert!(p.show_range, "the checkbox did not take");
        assert_eq!(
            p.view,
            Some(rustroom_app::grade_view::GradeRangeView::Shadows),
            "the default selection is Shadows, and the view must carry it"
        );
        assert!(
            p.ed.changed,
            "the view asked for no render, so the canvas would not change"
        );
        assert!(!p.ed.settled, "a view toggle pushed a history entry");
        assert_eq!(p.edits, before, "a view toggle edited the photograph");
    }

    /// ⚠ The derivation is change-guarded: this body runs every frame, and with the view on
    /// and nothing touched it must ask for nothing — or the canvas re-renders at 60 Hz for as
    /// long as the box is ticked. Sabotage: an unguarded write fails it.
    #[test]
    fn drawing_the_panel_with_the_view_on_requests_nothing() {
        let ctx = instant_ctx();
        let mut p = RangePanel::new(true);
        p.view = Some(rustroom_app::grade_view::GradeRangeView::Shadows);
        p.click(&ctx, "Color");
        assert!(!p.ed.changed, "drawing the panel asked for a render");
        assert!(!p.ed.settled, "drawing the panel pushed a history entry");
    }

    /// ⚠ Two inputs, one value on the wire: switching the selector while the box is ticked
    /// re-renders with the new range; Global carries `None` and leaves the box ticked, so
    /// stepping back to a range brings the view straight back.
    #[test]
    fn the_selector_drives_the_view_and_global_carries_none() {
        let ctx = instant_ctx();
        let mut p = RangePanel::new(true);
        // ⚠ Tone is collapsed first: its Highlights slider carries the same label as the
        // grading chip below it, and the harness clicks the first match it finds. A
        // sighted user tells the two apart by where they sit; the label query cannot.
        p.click(&ctx, "Tone");
        p.click(&ctx, "Color");
        p.click(&ctx, "Midtones");
        assert_eq!(
            p.view,
            Some(rustroom_app::grade_view::GradeRangeView::Midtones),
            "the selector did not reach the view"
        );
        assert!(p.ed.changed, "a new range asked for no render");
        p.click(&ctx, "Global");
        assert_eq!(p.view, None, "Global must carry no view");
        assert!(p.ed.changed, "leaving a range asked for no render");
        p.click(&ctx, "Highlights");
        assert!(p.show_range, "Global unticked the box");
        assert_eq!(
            p.view,
            Some(rustroom_app::grade_view::GradeRangeView::Highlights),
            "the view did not come back after Global"
        );
    }

    /// ⚠⚠ **The panel must forward the session's baseline into the solve, and nothing else can
    /// see it drop.** Auto Tone and the display rendering both place the level: with the
    /// rendering on, the scene is lifted by a baseline exposure before the curve, so a solve
    /// still aiming the scene median at `MEDIAN_TARGET` opens every photograph about a stop too
    /// bright. The correction lives in `pipeline` and is tested there — but passing `None` here
    /// would leave it present and **unreachable from the UI**, which is exactly what a sabotage
    /// found.
    #[test]
    fn the_auto_button_carries_the_baseline_into_the_solve() {
        let ctx = crate::test_ui::ctx();
        let buf = tone_fixture();
        let mut exposures = Vec::new();
        for baseline in [None, Some(2.0)] {
            let mut edits = edit_model::Edits {
                // ⚠ The correction applies only under the camera rendering; with `None` the two
                // do not stack and the target is unchanged, so this test could not see anything.
                rendering: Some(edit_model::Rendering::Camera),
                ..Default::default()
            };
            let mut ed = Edited::default();
            crate::test_ui::click(&ctx, "Auto", |ui| {
                ed = Edited::default();
                tone(
                    ui,
                    &mut edits,
                    &buf,
                    baseline,
                    &Default::default(),
                    None,
                    &mut ed,
                );
            });
            assert!(ed.changed, "the click did not take");
            exposures.push(edits.exposure);
        }
        assert!(
            (exposures[0] - exposures[1]).abs() > 0.1,
            "the baseline never reached the solve: {exposures:?}"
        );
    }

    /// ⚠ The discriminating other half: a button that fired every frame would pass the test
    /// above and be a catastrophe — an analysis and a history entry per frame, at 60 Hz.
    #[test]
    fn merely_drawing_the_tone_section_changes_nothing() {
        let ctx = crate::test_ui::ctx();
        let buf = tone_fixture();
        let mut edits = edit_model::Edits::default();
        for _ in 0..3 {
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(crate::test_ui::screen()),
                    ..Default::default()
                },
                |ui| {
                    let mut ed = Edited::default();
                    tone(
                        ui,
                        &mut edits,
                        &buf,
                        None,
                        &Default::default(),
                        None,
                        &mut ed,
                    );
                    assert!(!ed.changed, "drawing the panel reported an edit");
                    assert!(!ed.settled, "drawing the panel pushed a history entry");
                },
            );
        }
        assert_eq!(
            edits,
            edit_model::Edits::default(),
            "drawing changed the edits"
        );
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
    fn geometry_masks_and_retouch_are_the_groups_the_panel_does_not_show() {
        // They are rail tools, not adjustment sections. Stated as a property over the full
        // group list so that a group added later cannot silently go missing from the panel
        // with nothing failing — which is exactly what it did when Retouch (spot removal)
        // landed, and Retouch belongs here because its UI is a canvas tool (M5.3b), not a
        // slider section.
        let shown: Vec<&str> = SECTIONS.iter().map(|s| s.name()).collect();
        let missing: Vec<&str> = edit_model::EDIT_GROUP_NAMES
            .iter()
            .copied()
            .filter(|n| !shown.contains(n))
            .collect();
        assert_eq!(missing, vec!["Geometry", "Masks", "Retouch", "Rendering"]);
    }

    /// Click the segment labelled `want` and report the channel afterwards.
    ///
    /// Three passes: lay out with the pointer in place, press, release — egui resolves a
    /// click against the widgets registered on the *previous* pass, which is why a
    /// single-pass harness reads zeros and passes vacuously. That exact harness bug made
    /// `a_left_drag_does_not_pan` inert once already.
    fn click_segment(want: HslChannel) -> HslChannel {
        // ⚠ Drives the shipped selector through `hsl_bands::show`, not a re-declared loop —
        // a harness that rebuilds the widget tests egui, not the panel.
        let ctx = crate::test_ui::ctx();
        let mut channel = HslChannel::Hue;
        let mut hsl = edit_model::Hsl::default();
        let mut ed = Edited::default();
        crate::test_ui::click(&ctx, want.label(), |ui| {
            hsl_bands::show(ui, &mut hsl, &mut channel, &mut ed);
        });
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
        // ⚠ Retouch joins Geometry and Masks here: "Reset adjustments" must not wipe the
        // spots, for the reason it must not wipe a crop or a painted mask — they belong to a
        // tool the user cannot see from the Edit panel.
        //
        // ⚠⚠ And Rendering joins them for a *different* reason: it has no section at all. The
        // reset derives its group set from `SECTIONS`, so a group with no matching section is
        // never touched — which is precisely what stops this button flattening the photograph
        // back to a colorimetric render. See `edit_model::RENDERING_GROUP`.
        assert_eq!(untouched, vec!["Geometry", "Masks", "Retouch", "Rendering"]);
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
            refine: Vec::new(),
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

    /// A `LooksState` with two installed looks, built without touching disk.
    fn two_looks() -> rustroom_app::looks::LooksState {
        rustroom_app::looks::LooksState::from_entries(vec![
            rustroom_app::looks::LookEntry {
                file: "portra.cube".into(),
                name: "portra".into(),
            },
            rustroom_app::looks::LookEntry {
                file: "teal.cube".into(),
                name: "teal".into(),
            },
        ])
    }

    fn look_labels(
        edits: &mut edit_model::Edits,
        looks: &rustroom_app::looks::LooksState,
    ) -> Vec<String> {
        let ctx = crate::test_ui::ctx();
        let buf = tone_fixture();
        let mut ed = Edited::default();
        crate::test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
            ed = Edited::default();
            tone(ui, edits, &buf, None, looks, None, &mut ed);
        })
        .into_iter()
        .map(|(l, _)| l)
        .collect()
    }

    /// ⚠ The controls appear only once there is something to choose, and the Strength slider
    /// only once a look is chosen — a dropdown whose sole entry is "None" can only disappoint.
    ///
    /// ⚠⚠ **Asserted on Strength, not on the dropdown or the warnings.** The harness reads the
    /// accessibility tree, which carries interactive widgets and not plain labels or combo box
    /// headings — so what the panel *says* is guarded in `looks::notes`, where it is a pure
    /// function, and only what the panel *offers* is guarded here.
    #[test]
    fn the_strength_slider_appears_only_when_a_look_is_chosen() {
        let looks = two_looks();
        let mut none = edit_model::Edits::default();
        assert!(
            !look_labels(&mut none, &looks)
                .iter()
                .any(|l| l == "Strength"),
            "Strength was offered with no look chosen"
        );

        let mut chosen = edit_model::Edits {
            look: edit_model::Look {
                file: Some("portra.cube".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(
            look_labels(&mut chosen, &looks)
                .iter()
                .any(|l| l == "Strength"),
            "a chosen look offered no Strength"
        );

        // ⚠ And nothing at all when none are installed and none is named.
        let empty = rustroom_app::looks::LooksState::default();
        let mut e = edit_model::Edits::default();
        assert!(
            !look_labels(&mut e, &empty).iter().any(|l| l == "Strength"),
            "the look controls appeared with no looks installed"
        );
    }
}
