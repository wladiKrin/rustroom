//! The five edit-group sections of the Develop panel.

use crate::develop::panel::color_wheel;
use crate::develop::panel::hsl_bands;
use crate::develop::panel::tone_curve;
use crate::develop::panel::Edited;
use crate::theme;
use crate::theme::Hover as _;
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

    /// The selector's hover sentence (M10.6b), beside the label so the two cannot part.
    pub fn hover(self) -> &'static str {
        match self {
            Self::Hue => "Shows the sliders that shift each colour's hue",
            Self::Saturation => "Shows the sliders that change each colour's saturation",
            Self::Luminance => "Shows the sliders that brighten or darken each colour",
        }
    }

    /// The tab the band sliders' hover sentences describe.
    pub fn help_tab(self) -> rustroom_app::slider_help::HslTab {
        use rustroom_app::slider_help::HslTab;
        match self {
            Self::Hue => HslTab::Hue,
            Self::Saturation => HslTab::Saturation,
            Self::Luminance => HslTab::Luminance,
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
    /// The rendered frame's histogram, for the ghost behind the tone curve. `None` in a
    /// harness that has no render.
    pub hist: Option<&'a [[u32; 256]; 3]>,
    /// What this photo would open as if it had never been edited — `Settings::initial_edits(None)`.
    /// **The sections' neutral**: a section is modified when it differs from this, and its reset
    /// returns to it.
    ///
    /// ⚠⚠ **Never `Edits::default()`** (reported in use 2026-09-29): with "Apply lens correction to
    /// new photos" ticked every fresh photo opens with the corrections on, and measured against
    /// all-off the Lens section showed its dot and a reset on a photo nobody had touched — and
    /// the reset, like "Reset adjustments", turned the default correction off.
    pub fresh: &'a edit_model::Edits,
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
            .help(rustroom_app::slider_help::text(
                rustroom_app::slider_help::Scope::Global,
                "Temp",
            ))
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
    // ⚠ A double-click resets Temp to AS SHOT — `kelvin: None`, the photo following its file —
    // rather than to a number: no fixed temperature is this slider's default. After the line
    // above, so a first click that moved the value cannot win.
    if r.double_clicked() {
        wb.kelvin = None;
        ed.discrete();
    }
    ed.absorb(&r);
    ed.slider(ui, &mut wb.tint, -100.0..=100.0, "Tint", 0.0);

    // ⚠ One row for the two shortcuts (2026-09-19): both move the sliders above, and two
    // full-width rows of buttons under two sliders was heavy for what they are. A mask has
    // no eyedropper, and then the row holds Presets alone.
    ui.horizontal(|ui| {
        if let Some(picking) = picking {
            // ⚠ A toggle, not a button: an armed eyedropper changes what a click on the canvas does,
            // and a mode you cannot see is the complaint the crop aspect lock was fixed for. The
            // canvas puts a crosshair under the pointer while it is on, which is the other half of
            // saying so.
            if ui
                .selectable_label(*picking, "Pick a neutral")
                .hover("Sets white balance from a spot you click that should be neutral grey")
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
        .hover("Sets the temperature for a common light source such as daylight or tungsten");
    });
}

/// The `Advanced color` block's persistent id.
///
/// ⚠ Named rather than a bare literal, so the block's identity is one thing to find. ⚠ The tests open it
/// by **clicking its header**, not by this id: `ui.make_persistent_id` is relative to the Ui's id stack,
/// and this header is created inside the section's nested body, so an id computed anywhere else is a
/// different id — which is how a first attempt at those tests passed while opening nothing.
const ADVANCED_ID: &str = "color_advanced";

/// 1667 K – 25000 K, as mireds. ⚠ Stored the way the slider draws them, so the two cannot
/// disagree about which end is warm.
///
/// ⚠⚠ **The warm end is 1667 K to match `color::wb::solve_xy`, which clamps an as-shot temperature
/// to exactly `1667..=25000`.** It was 2000 K until 2026-09-23, and that two-file mismatch is what
/// the "opening a photo sets Temp to 2000 K" report was: the panel shows the *effective* temperature
/// while `kelvin` is `None`, so a warm as-shot solving below 2000 K was handed to a control that
/// could not represent it. ⚠ Pinned by
/// `the_sliders_range_covers_everything_the_solve_can_return` — **if the solve's clamp moves, that
/// test fails rather than this silently breaking again.**
const MIRED_MIN: f32 = 1.0e6 / 25000.0;
const MIRED_MAX: f32 = 1.0e6 / 1667.0;

/// The point curve a channel edits.
fn point_curve(
    edits: &mut edit_model::Edits,
    channel: tone_curve::CurveChannel,
) -> &mut edit_model::ToneCurve {
    match channel {
        tone_curve::CurveChannel::Rgb => &mut edits.tone_curve,
        tone_curve::CurveChannel::Red => &mut edits.tone_curve_red,
        tone_curve::CurveChannel::Green => &mut edits.tone_curve_green,
        tone_curve::CurveChannel::Blue => &mut edits.tone_curve_blue,
    }
}

/// The Tone section: Auto, the six tonal sliders, the curve, then the Presence block.
///
/// ⚠ The camera rendering and the look sat on top of this section until 2026-09-19; they are
/// the Profile block now — see `profile::rendering`.
///
/// ⚠ Its own function rather than a closure inside [`show`], which had reached 605 lines. The
/// Auto button needs the buffer threaded to it, and threading a parameter through a closure that
/// already captures half the panel is how that file grew in the first place.
pub(crate) fn tone(
    ui: &mut egui::Ui,
    edits: &mut edit_model::Edits,
    buffer: &core_image::ImageBuf,
    baseline_ev: Option<f32>,
    hist: Option<&[[u32; 256]; 3]>,
    // ⚠ Its own parameter rather than reached through the app, for the same reason `hsl_channel` is:
    // the caller holds `&mut app.session` at the same time, and two separate `&mut` parameters are
    // legal where a closure capturing `app` twice is `E0499`.
    curves: CurveUi<'_>,
    ed: &mut Edited,
) {
    let CurveUi {
        mode: curve_mode,
        channel: curve_channel,
        open: curve_open,
        parametric_drag,
    } = curves;
    // ⚠⚠ **The analysis runs on the click only.** This body runs sixty times a second, and a
    // percentile sort of the buffer per frame is the `apply_view_demosaic` mistake in its most
    // expensive costume yet.
    //
    // ⚠ Its own row rather than squeezed beside the Exposure slider: the panel is 240 pt and a
    // slider there already spends all of it on the rail, its label and its value. Lightroom puts
    // its Auto on its own row in the Tone panel for the same reason.
    if ui
        .button("Auto")
        .hover("Sets Exposure, Whites and Blacks automatically from the histogram")
        .clicked()
    {
        rustroom_app::auto_tone_tool::apply(edits, buffer, baseline_ev);
        // Said out loud in the status bar: two sliders pinned at +100 read as a fault to a
        // first-time user unless the app says it did that on purpose (A-17).
        ed.note = Some(format!(
            "Auto set exposure {:+.2}, whites {:+.0} and blacks {:+.0}",
            edits.exposure, edits.whites, edits.blacks
        ));
        // ⚠ `mark_changed`, not a slider's `changed()`: a click is a complete act, so it is one
        // history entry immediately rather than waiting for a `drag_stopped` that never comes.
        // That is what makes Auto exactly one Ctrl+Z — the model the WB presets already follow.
        ed.mark_changed();
    }
    ed.slider(ui, &mut edits.exposure, -5.0..=5.0, "Exposure", 0.0);
    ed.slider(ui, &mut edits.contrast, -100.0..=100.0, "Contrast", 0.0);
    ed.slider(ui, &mut edits.highlights, -100.0..=100.0, "Highlights", 0.0);
    ed.slider(ui, &mut edits.shadows, -100.0..=100.0, "Shadows", 0.0);
    ed.slider(ui, &mut edits.whites, -100.0..=100.0, "Whites", 0.0);
    ed.slider(ui, &mut edits.blacks, -100.0..=100.0, "Blacks", 0.0);
    // ⚠ The curve directly after the six sliders, because it refines the tone they set; a
    // sub-label rather than a header, since `tone_curve` belongs to the `Tone` edit group and a
    // header here would put the panel and the Sync dialog out of step. The curve draws its own
    // sub-label row. (It closed the section until 2026-09-19.)
    //
    // ⚠⚠ **Collapsible since 2026-09-29** ("it's quite large and may be in the way", the author): the
    // header stays, with a dot while any of the five curves is not straight, so a closed curve still
    // says it is doing something.
    let modified = [
        &edits.tone_curve,
        &edits.tone_curve_red,
        &edits.tone_curve_green,
        &edits.tone_curve_blue,
    ]
    .iter()
    .any(|c| !c.points.is_empty())
        || edits.tone_curve_parametric != edit_model::ParametricCurve::default();
    // ⚠ Read *by value* first, which is what keeps `edits` and the selections from being borrowed at
    // once below.
    let (mode, channel) = (*curve_mode, *curve_channel);
    let open = tone_curve::header(ui, curve_open, modified, |ui| match mode {
        tone_curve::CurveMode::Parametric => {
            super::parametric_curve::reset_button(ui, &mut edits.tone_curve_parametric, ed)
        }
        tone_curve::CurveMode::Point => {
            tone_curve::reset_button(ui, point_curve(edits, channel), ed)
        }
    });
    if open {
        tone_curve::selectors(ui, curve_mode, curve_channel);
        // ⚠ M5.1c: the Parametric view edits a different field, so it is its own widget. The point
        // views get the parametric curve read-only, to draw the combined curve.
        let param = edits.tone_curve_parametric;
        match *curve_mode {
            tone_curve::CurveMode::Parametric => {
                let _ = super::parametric_curve::show(
                    ui,
                    &mut edits.tone_curve_parametric,
                    &edits.tone_curve.points,
                    ed,
                    hist,
                    parametric_drag,
                );
            }
            tone_curve::CurveMode::Point => {
                let c = *curve_channel;
                tone_curve::show(ui, point_curve(edits, c), ed, hist, Some(c), Some(&param));
            }
        }
    }
    // ⚠ A sub-label, not a header. These are Lightroom's "Presence" block and they are
    // local-contrast tools rather than tonal ones — a different kind of decision, which is why
    // they close the section — but they belong to the `Tone` edit group, and a header here would
    // put the panel and the Sync dialog out of step.
    theme::sub_label(ui, "Presence");
    ed.slider(
        ui,
        &mut edits.local_contrast.texture,
        -100.0..=100.0,
        "Texture",
        0.0,
    );
    ed.slider(
        ui,
        &mut edits.local_contrast.clarity,
        -100.0..=100.0,
        "Clarity",
        0.0,
    );
    ed.slider(ui, &mut edits.dehaze.amount, -100.0..=100.0, "Dehaze", 0.0);
}

/// The colour-grading block's view state: the `Shad │ Mid │ High │ Global` selector and the
/// grade range view's two session fields.
///
/// The tone curve's UI inputs: which curve and channel are shown, whether the block is open, and
/// what a drag does in the Parametric view. Grouped when the drag binding arrived (M5.1c), because
/// `too_many_arguments` fired at eight.
pub(crate) struct CurveUi<'a> {
    pub mode: &'a mut tone_curve::CurveMode,
    pub channel: &'a mut tone_curve::CurveChannel,
    /// `Settings::tone_curve_open` — the caller saves it when it changes.
    pub open: &'a mut bool,
    pub parametric_drag: rustroom_app::settings::ParametricDrag,
}

/// Which view each of the panel's selectors is showing, and whether the tone curve is open.
///
/// ⚠ A params struct because clippy counted **eight** arguments on [`show`], and as `SourceInfo`'s own
/// doc records, *"every time the note was that clippy was right"* — the fifth time this project has
/// answered `too_many_arguments` this way, after `FrameSpec`, `NameField`, `RenderSpec` and
/// `SourceInfo`.
///
/// ⚠⚠ All are **view state**, never `Edits`: switching a selector must not push a history entry or
/// travel in a preset. That is why they arrive as their own `&mut`s rather than being reached through
/// the app — the caller holds `&mut app.session` at the same time.
pub struct Channels<'a> {
    pub hsl: &'a mut HslChannel,
    pub curve_mode: &'a mut tone_curve::CurveMode,
    pub curve: &'a mut tone_curve::CurveChannel,
    /// Whether the tone curve is expanded — `Settings::tone_curve_open`, saved by the caller.
    pub curve_open: &'a mut bool,
    /// What a drag does in the curve's Parametric view — a `Settings` binding, carried with the
    /// channel selections because it is read in the same place.
    pub parametric_drag: rustroom_app::settings::ParametricDrag,
}

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

/// The two canvas eyedroppers the Edit panel can arm. `None` draws no picker — how a test says
/// "no session" rather than handing in a scratch flag that quietly does nothing.
///
/// ⚠ A struct because `show` sits at clippy's argument limit — this project's standing answer to
/// that lint, the `CurveUi` / `GradeUi` precedent.
#[derive(Default)]
pub struct Pickers<'a> {
    pub wb: Option<&'a mut bool>,
    /// The defringe eyedropper (M9.6).
    pub fringe: Option<&'a mut bool>,
}

pub fn show(
    ui: &mut egui::Ui,
    edits: &mut edit_model::Edits,
    channels: Channels<'_>,
    grade: GradeUi<'_>,
    source: SourceInfo<'_>,
    pickers: Pickers<'_>,
    ed: &mut Edited,
) {
    let Pickers {
        wb: picking,
        fringe: picking_fringe,
    } = pickers;
    let Channels {
        hsl: hsl_channel,
        curve_mode,
        curve: curve_channel,
        curve_open,
        parametric_drag,
    } = channels;
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
        hist,
        fresh,
    } = source;
    // White Balance and Tone open by default; the rest start collapsed, because a panel
    // that opens with 30 sliders showing is the problem this section structure removes.
    section(ui, SECTIONS[0], true, fresh, edits, ed, |ui, edits, ed| {
        white_balance(ui, &mut edits.white_balance, baseline, picking, ed);
    });

    section(ui, SECTIONS[1], true, fresh, edits, ed, |ui, edits, ed| {
        tone(
            ui,
            edits,
            buffer,
            baseline_ev,
            hist,
            CurveUi {
                mode: curve_mode,
                channel: curve_channel,
                open: curve_open,
                parametric_drag,
            },
            ed,
        );
    });

    section(ui, SECTIONS[2], false, fresh, edits, ed, |ui, edits, ed| {
        // ⚠ Vibrance first: Adobe's order, and the gentler control. Saturation clips
        // already-saturated colour far more readily, so it is the one offered second.
        ed.slider(ui, &mut edits.vibrance, -100.0..=100.0, "Vibrance", 0.0);
        ed.slider(ui, &mut edits.saturation, -100.0..=100.0, "Saturation", 0.0);

        // ⚠⚠ **Closed by default, and that is the change.** A newcomer opening Color met ~40 values
        // behind 15 sliders, two segmented selectors and a wheel; they now meet **two sliders**, with
        // everything else one click away behind a name that says what it is for (user's call,
        // 2026-09-24, after an amateur photographer's review).
        //
        // ⚠ Not `resettable`: only the section's own header resets the `Color` group, or two reset
        // affordances would claim to reset different things — and this one would reset less than it
        // appeared to.
        //
        // ⚠⚠ **Not a `Section`.** `Section::name()` is `EDIT_GROUP_NAMES[i]` and `edit_groups()` is
        // *derived* from `SECTIONS`, so a real section here would claim an edit group that does not
        // exist and put the panel and the Sync dialog out of step — the hazard both this file and
        // `hsl_bands.rs` already warn about. Splitting the `Color` group into three is its own change,
        // with a version bump and three Sync checkboxes where there is one.
        let mut adv = super::section::header(
            ui,
            ui.make_persistent_id(ADVANCED_ID),
            false,
            super::section::Header::new("Advanced color"),
        );
        adv.show_body(ui, |ui| {
            theme::sub_label_hover(
                ui,
                "Adjust individual colors",
                "Adjusts the hue, saturation and brightness of each colour separately (HSL)",
            );
            // ⚠ Shared with a mask's own HSL block — see `hsl_bands`. One layout learned once.
            hsl_bands::show(ui, &mut edits.hsl, hsl_channel, ed);

            // ⚠ A sub-label rather than a header of its own: colour grading belongs to the `Color`
            // edit group, and a *second* header claiming a group would put the panel and the Sync
            // dialog out of step. ⚠ It now sits inside `Advanced color`, which is a collapsible that
            // deliberately claims **no** group — see its own note above. Two levels, not three, which
            // is the line `hsl_bands.rs` records.
            // ⚠⚠ "Show range" is A VIEW MODE, not an edit — `visualize`'s rule. It writes two
            // session fields and never `Edits`, so it re-renders (`ed.changed`) without a history
            // entry. On the sub-label row, right-aligned, because it is a toggle for exactly the
            // range selector under it (2026-09-19; it sat below Balance before, next to nothing it
            // belonged to).
            // ⚠ Plain language visible, the jargon in the hover on the block above it. `sub_label_with`
            // has no hover parameter and the trailing closure owns the right of the row, so "color
            // grading" is carried by the checkbox's hover rather than half-added here.
            theme::sub_label_with(ui, "Tint by tone", |ui| {
                ui.checkbox(show_grade_range, "Show range").hover(
                    "Tints the preview where the selected range applies, without changing the photo",
                );
            });
            theme::segmented(
                ui,
                grade_range,
                &color_wheel::GradeRange::ALL.map(|r| (r, r.label(), r.hover())),
            );
            let diameter = ui.available_width().min(188.0);
            {
                // Scoped so the borrow of `color_grading` ends before Blending and Balance,
                // which need the struct itself.
                color_wheel::show(ui, &mut edits.color_grading, *grade_range, diameter, ed);
                let w = grade_range.wheel_mut(&mut edits.color_grading);
                ed.help_scope = rustroom_app::slider_help::Scope::TintByTone;
                ed.slider(ui, &mut w.hue, 0.0..=360.0, "Hue", 0.0);
                // ⚠⚠ `Strength`, not `Saturation`: the global Saturation slider and HSL's Saturation channel both
                // mean **the image's chroma**, while this means **how much tint is applied** — the same word
                // for the opposite subject, and a named cause of the "the Color section is confusing" review
                // (2026-09-24). ⚠ `Hue` and `Luminance` are deliberately left alone: they repeat by word but
                // not by meaning, and Lightroom labels its wheels the same way.
                ed.slider(ui, &mut w.saturation, 0.0..=100.0, "Strength", 0.0);
                ed.slider(ui, &mut w.luminance, -100.0..=100.0, "Luminance", 0.0);
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
            ed.slider(
                ui,
                &mut g.blending,
                0.0..=100.0,
                "Blending",
                edit_model::ColorGrading::default().blending,
            );
            ed.slider(ui, &mut g.balance, -100.0..=100.0, "Balance", 0.0);
            ed.help_scope = rustroom_app::slider_help::Scope::Global;
            // ⚠ Two inputs, one value on the wire, derived here because this is the one place the
            // checkbox and the selector are both in scope. Change-guarded: this body runs every
            // frame, and an unguarded write would request a render sixty times a second.
            let wanted = show_grade_range.then(|| grade_range.view()).flatten();
            if *grade_view != wanted {
                *grade_view = wanted;
                ed.changed = true;
            }
        });
    });

    section(ui, SECTIONS[3], false, fresh, edits, ed, |ui, edits, ed| {
        // ⚠ Noise Reduction above Sharpening: you must denoise before you can judge
        // sharpening, and it is the pipeline's own order. A deliberate divergence from
        // Lightroom, which puts Sharpening first.
        theme::sub_label(ui, "Noise reduction");
        ed.help_scope = rustroom_app::slider_help::Scope::NoiseReduction;
        let nr = &mut edits.noise_reduction;
        ed.slider(ui, &mut nr.luminance, 0.0..=100.0, "Luminance", 0.0);
        ed.slider(
            ui,
            &mut nr.detail,
            0.0..=100.0,
            "Detail",
            edit_model::NoiseReduction::default().detail,
        );
        ed.slider(ui, &mut nr.color, 0.0..=100.0, "Color", 0.0);
        ed.slider(
            ui,
            &mut nr.smoothness,
            0.0..=100.0,
            "Smoothness",
            edit_model::NoiseReduction::default().smoothness,
        );
        theme::sub_label(ui, "Sharpening");
        ed.help_scope = rustroom_app::slider_help::Scope::Sharpening;
        let sh = &mut edits.sharpening;
        ed.slider(ui, &mut sh.amount, 0.0..=100.0, "Amount", 0.0);
        // The one slider not on a 0..100 scale: a physical quantity in full-resolution
        // pixels, and naming it in pixels is what makes "scaled for the render" legible.
        ed.slider(
            ui,
            &mut sh.radius,
            0.5..=3.0,
            "Radius (px)",
            edit_model::Sharpening::default().radius,
        );
        ed.slider(
            ui,
            &mut sh.detail,
            0.0..=100.0,
            "Detail",
            edit_model::Sharpening::default().detail,
        );
        ed.slider(ui, &mut sh.masking, 0.0..=100.0, "Masking", 0.0);
        ed.help_scope = rustroom_app::slider_help::Scope::Global;
    });

    // ⚠ Its own section rather than a block inside Detail, and its own edit group with it. They
    // were bundled at first on the grounds that Detail is the capture-specific group — true, but
    // it meant syncing noise reduction also synced lens corrections. Separating them makes
    // "copy this photo's lens corrections" its own choice everywhere `EditGroups` is offered.
    //
    // ⚠ Placed after Detail, not first, despite running first in the pipeline: it is on by
    // default and visited only to turn something *off*, so depth costs nothing. Panel follows
    // workflow, pipeline follows maths — the rule `SECTIONS`' own doc comment states.
    section(ui, SECTIONS[4], false, fresh, edits, ed, |ui, edits, ed| {
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
            touched |= ui
                .checkbox(&mut l.distortion, "Distortion")
                .hover("Straightens the lens's barrel or pincushion bending")
                .changed();
            touched |= ui
                .checkbox(&mut l.tca, "Chromatic aberration")
                .hover("Removes the colour fringes the lens adds toward the edges")
                .changed();
            // ⚠ Not the creative Vignette, which stays in Effects. This one is corrective and
            // pre-crop; that one is creative and post-crop. The section heading keeps them apart.
            touched |= ui
                .checkbox(&mut l.vignetting, "Vignetting")
                .hover("Brightens the corners the lens darkens")
                .changed();
            // ⚠ Also surfaced in the **Crop** tool, and that is not duplication for its own sake:
            // Scale is only *judgeable* against the uncropped frame, which is what crop mode
            // shows. Here it is often invisible — if you have cropped at all, the empty corners
            // it removes are already outside your crop.
            //
            // ⚠ "Scale", not "Crop": only *pincushion* correction empties the frame, and barrel
            // correction's autoscale is exactly 1, so on many photos this does nothing at all.
            ed.help_scope = rustroom_app::slider_help::Scope::Lens;
            let scaled = ed
                .slider(
                    ui,
                    &mut l.scale,
                    0.0..=100.0,
                    "Scale",
                    edit_model::LensCorrection::default().scale,
                )
                .changed();
            ed.help_scope = rustroom_app::slider_help::Scope::Global;
            if touched || scaled {
                edits.lens_correction = Some(l);
            }
            if touched {
                ed.discrete();
            }
        });
        // ⚠ OUTSIDE the profile-gated block: a defringe needs no lens profile — it exists for
        // exactly the photos whose profile did not fix the fringe (M9.6).
        ui.separator();
        super::defringe::show(ui, &mut edits.defringe, picking_fringe, ed);
    });

    section(ui, SECTIONS[5], false, fresh, edits, ed, |ui, edits, ed| {
        let v = &mut edits.vignette;
        ed.help_scope = rustroom_app::slider_help::Scope::Vignette;
        ed.slider(ui, &mut v.amount, -100.0..=100.0, "Amount", 0.0);
        ed.slider(
            ui,
            &mut v.midpoint,
            0.0..=100.0,
            "Midpoint",
            edit_model::Vignette::default().midpoint,
        );
        ed.slider(ui, &mut v.roundness, -100.0..=100.0, "Roundness", 0.0);
        ed.slider(
            ui,
            &mut v.feather,
            0.0..=100.0,
            "Feather",
            edit_model::Vignette::default().feather,
        );
        ed.help_scope = rustroom_app::slider_help::Scope::Global;
    });
}

/// One collapsible section, headed by its edit group's name — see `section::header` for the
/// row itself.
///
/// ⚠ `edits` and `ed` are passed *through* to the body rather than captured by it, because
/// the header reads `edits` for the dot before the body runs and writes it for the reset after
/// — a closure capturing `&mut edits` for its whole lifetime would be `E0499` against both.
fn section(
    ui: &mut egui::Ui,
    s: Section,
    open_by_default: bool,
    fresh: &edit_model::Edits,
    edits: &mut edit_model::Edits,
    ed: &mut Edited,
    body: impl FnOnce(&mut egui::Ui, &mut edit_model::Edits, &mut Edited),
) {
    // ⚠ Against the photo's own starting point, never all-off — `SourceInfo::fresh`.
    let modified = edits.group_differs(fresh, s.index());
    let mut out = super::section::header(
        ui,
        ui.make_persistent_id(s.name()),
        open_by_default,
        super::section::Header::new(s.name())
            .modified(modified)
            .resettable(true),
    );
    out.show_body(ui, |ui| body(ui, edits, ed));
    if out.reset {
        edits.merge_from(fresh, edit_model::EditGroups::only(s.index()));
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

    /// ⚠⚠ **Reported in use 2026-09-23: "some photos, when opening, automatically set temp to
    /// 2000K, even though the neutral is as shot".**
    ///
    /// The mechanism, and it is a **range mismatch between two files**: `color::wb::solve_xy` clamps
    /// an as-shot temperature to **1667–25000 K**, while this panel's slider ran **2000–25000 K**. A
    /// warm photo solving below 2000 K was handed to a `DragValue` whose `range()` clamps *existing*
    /// values by default and writes the clamp back through its `&mut` — reported as `changed()`, so
    /// the panel committed `Some(2000.0)` on the first frame it drew. The photo stopped following its
    /// file without anyone touching a control.
    ///
    /// ⚠ Not from the 2026-09-19 slider restyle: `egui::Slider`'s own default is
    /// `SliderClamping::Always`, so the version before it clamped too. Possible since 2026-09-01.
    ///
    /// ⚠⚠ **The guard beside this one was blind to it** because it only ever passed 4200 K — a
    /// baseline comfortably inside the range. A guard that names one value does not guard the others.
    #[test]
    fn a_baseline_below_the_sliders_range_does_not_freeze_a_temperature() {
        let ctx = crate::test_ui::ctx();
        let mut wb = edit_model::WhiteBalance::default();
        // ⚠ 1800 K: what `solve_xy` can genuinely return, being above its own 1667 K floor.
        let base = color::wb::Illuminant::new(1800.0, 0.0);
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
        assert_eq!(
            wb.kelvin, None,
            "the panel froze a temperature nobody set, from a baseline it could not display"
        );
    }

    /// ⚠ The other end, for the same reason: nothing about this is specific to the warm side.
    #[test]
    fn a_baseline_above_the_sliders_range_does_not_freeze_a_temperature_either() {
        let ctx = crate::test_ui::ctx();
        let mut wb = edit_model::WhiteBalance::default();
        let base = color::wb::Illuminant::new(40000.0, 0.0);
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(crate::test_ui::screen()),
                ..Default::default()
            },
            |ui| {
                let mut ed = Edited::default();
                white_balance(ui, &mut wb, base, None, &mut ed);
            },
        );
        assert_eq!(wb.kelvin, None);
    }

    /// ⚠⚠ **The mismatch itself, pinned so it cannot come back.** The slider must be able to show
    /// every temperature the solve can produce; otherwise the value handed to it is out of range and
    /// the whole class above reopens.
    #[test]
    fn the_sliders_range_covers_everything_the_solve_can_return() {
        // `color::wb::solve_xy` clamps to 1667..=25000 K.
        let (solve_min_k, solve_max_k) = (1667.0f32, 25000.0f32);
        let slider_warmest_k = 1.0e6 / MIRED_MAX;
        let slider_coolest_k = 1.0e6 / MIRED_MIN;
        assert!(
            slider_warmest_k <= solve_min_k,
            "the slider stops at {slider_warmest_k:.0} K but the solve can return {solve_min_k:.0} K"
        );
        assert!(
            slider_coolest_k >= solve_max_k,
            "the slider stops at {slider_coolest_k:.0} K but the solve can return {solve_max_k:.0} K"
        );
    }

    /// ⚠⚠ **The review, as a test.** An amateur photographer found the Color section confusing, and a
    /// concrete cause was that **"Saturation" named three different things in it** — the global slider,
    /// HSL's channel and the grading wheel's slider. `hsl_bands.rs` had already recorded hitting the
    /// same hazard from the other side.
    ///
    /// ⚠ A **role filter** over whatever sliders exist, never a list of expected labels: a second
    /// "Saturation" added tomorrow fails this with nothing to maintain.
    #[test]
    fn no_two_sliders_in_the_adjustments_panel_share_a_name() {
        let ctx = instant_ctx();
        let mut p = RangePanel::new(false);
        p.click(&ctx, "Color");
        p.click(&ctx, "Advanced color");
        let labels = p.slider_labels(&ctx);
        assert!(
            labels.len() > 12,
            "the advanced block did not open, so this would guard nothing: {labels:?}"
        );
        let mut seen = std::collections::HashMap::<&str, usize>::new();
        for l in &labels {
            *seen.entry(l.as_str()).or_default() += 1;
        }
        let mut dupes: Vec<&str> = seen
            .iter()
            .filter(|(_, &n)| n > 1)
            .map(|(l, _)| *l)
            .collect();
        dupes.sort_unstable();
        assert!(
            dupes.is_empty(),
            "these slider names appear more than once: {dupes:?} (all: {labels:?})"
        );
        assert!(
            labels.iter().any(|l| l == "Strength"),
            "the grading wheel's amount should read Strength: {labels:?}"
        );
    }

    /// ⚠ Closed by default, which is the change: a newcomer meets the two intensity sliders and not the
    /// forty values behind HSL and grading.
    #[test]
    fn the_advanced_color_block_is_closed_by_default() {
        let ctx = instant_ctx();
        let mut p = RangePanel::new(false);
        p.click(&ctx, "Color");
        let labels = p.slider_labels(&ctx);
        assert!(
            labels.iter().any(|l| l == "Vibrance"),
            "Vibrance must stay visible: {labels:?}"
        );
        assert!(
            !labels.iter().any(|l| l == "Blending"),
            "grading must start hidden: {labels:?}"
        );
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
                None,
                CurveUi {
                    mode: &mut tone_curve::CurveMode::default(),
                    channel: &mut tone_curve::CurveChannel::default(),
                    open: &mut true,
                    parametric_drag: Default::default(),
                },
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
                Channels {
                    hsl: &mut channel,
                    curve_mode: &mut tone_curve::CurveMode::default(),
                    curve: &mut tone_curve::CurveChannel::default(),
                    curve_open: &mut true,
                    parametric_drag: Default::default(),
                },
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
                    hist: None,
                    fresh: &edit_model::Edits::default(),
                },
                Pickers::default(),
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

    /// M9.6: the Lens section's Defringe block works with NO lens profile — the photos it exists
    /// for — and its picker arms the canvas eyedropper. ⚠ `lens_profile_found: false`, or a block
    /// drawn inside the profile-gated `add_enabled_ui` would pass.
    #[test]
    fn the_defringe_block_works_without_a_lens_profile() {
        let ctx = crate::test_ui::ctx();
        for theme in [egui::Theme::Dark, egui::Theme::Light] {
            ctx.style_mut_of(theme, |s| s.animation_time = 0.0);
        }
        let buf = tone_fixture();
        let mut edits = edit_model::Edits::default();
        let mut channel = HslChannel::default();
        let mut grade = color_wheel::GradeRange::default();
        let (mut show_range, mut view) = (false, None);
        let mut ed = Edited::default();
        let mut picking = false;
        let mut body = |ui: &mut egui::Ui| {
            ed = Edited::default();
            show(
                ui,
                &mut edits,
                Channels {
                    hsl: &mut channel,
                    curve_mode: &mut tone_curve::CurveMode::default(),
                    curve: &mut tone_curve::CurveChannel::default(),
                    curve_open: &mut true,
                    parametric_drag: Default::default(),
                },
                GradeUi {
                    range: &mut grade,
                    show_range: &mut show_range,
                    view: &mut view,
                },
                SourceInfo {
                    lens: None,
                    lens_profile_found: false,
                    wb_baseline: color::wb::Illuminant::default(),
                    baseline_ev: None,
                    buffer: &buf,
                    hist: None,
                    fresh: &edit_model::Edits::default(),
                },
                Pickers {
                    wb: None,
                    fringe: Some(&mut picking),
                },
                &mut ed,
            );
        };
        crate::test_ui::click(&ctx, "Lens", &mut body);
        let labels = crate::test_ui::slider_labels(&ctx, &mut body);
        assert!(
            labels.iter().any(|l| l == "Purple amount")
                && labels.iter().any(|l| l == "Green amount"),
            "{labels:?}"
        );
        crate::test_ui::click(&ctx, "Pick a fringe", &mut body);
        assert!(picking, "the picker did not arm");
    }

    /// ⚠⚠ **Every slider in the Edit panel resets to its field's DEFAULT on a double-click** —
    /// the guard behind `Edited::slider`'s required `default`. Not a list of fields: the sliders
    /// are found by ROLE, so one added tomorrow is covered. Each is clicked off its default, then
    /// double-clicked; afterwards the photo must be a fresh photo again, and anything left over
    /// names the slider whose default is wrong. ⚠ The fixture is asserted to have moved things
    /// first, or a harness whose clicks landed nowhere would pass.
    #[test]
    fn every_slider_resets_to_its_fields_default() {
        let ctx = crate::test_ui::ctx();
        for theme in [egui::Theme::Dark, egui::Theme::Light] {
            ctx.style_mut_of(theme, |s| s.animation_time = 0.0);
        }
        let buf = tone_fixture();
        let mut edits = edit_model::Edits::default();
        let mut channel = HslChannel::default();
        let mut curve = tone_curve::CurveChannel::default();
        // The Parametric view, so its sliders are in the sweep; the curve block open.
        let mut mode = tone_curve::CurveMode::Parametric;
        let mut open = true;
        let mut grade = color_wheel::GradeRange::default();
        let (mut show_range, mut view) = (false, None);
        let mut ed = Edited::default();
        // ⚠ The closure holds `edits` mutably, so the test reads this mirror between clicks.
        let seen = std::cell::RefCell::new(edit_model::Edits::default());
        let mut body = |ui: &mut egui::Ui| {
            ed = Edited::default();
            egui::ScrollArea::vertical().show(ui, |ui| {
                show(
                    ui,
                    &mut edits,
                    Channels {
                        hsl: &mut channel,
                        curve_mode: &mut mode,
                        curve: &mut curve,
                        curve_open: &mut open,
                        parametric_drag: Default::default(),
                    },
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
                        hist: None,
                        fresh: &edit_model::Edits::default(),
                    },
                    Pickers::default(),
                    &mut ed,
                )
            });
            *seen.borrow_mut() = edits.clone();
        };
        for h in ["Color", "Detail", "Lens", "Vignette"] {
            crate::test_ui::click(&ctx, h, &mut body);
        }
        crate::test_ui::click(&ctx, "Advanced color", &mut body);
        let sliders = crate::test_ui::slider_centres(&ctx, &mut body);
        assert!(sliders.len() > 30, "only {} sliders found", sliders.len());

        let mut moved = 0;
        for (label, at) in &sliders {
            // A third of the way from the rail's centre toward its right end: off any default
            // at the centre or an end, and away from the handle so the click moves it.
            let off = *at + egui::vec2(40.0, 0.0);
            let before = seen.borrow().clone();
            crate::test_ui::click_at(&ctx, off, false, &mut body);
            moved += (*seen.borrow() != before) as usize;
            crate::test_ui::click_at(&ctx, off, true, &mut body);
            let mut e = seen.borrow().clone();
            // ⚠ Touching Lens Scale DECIDES the lens corrections (`Some`), by design; decided
            // at the default is the fresh photo's corrections.
            if e.lens_correction == Some(edit_model::LensCorrection::default()) {
                e.lens_correction = None;
            }
            assert!(
                e == edit_model::Edits::default(),
                "{label:?} did not reset to its default: {} parameter(s) differ",
                e.changed_parameters(&edit_model::Edits::default())
            );
        }
        assert!(
            moved > 25,
            "only {moved} of {} single clicks moved anything",
            sliders.len()
        );
    }

    /// The Edit panel with the state the range view reads and writes, held together so a
    /// click's borrows end when the click does.
    /// ⚠⚠ Every slider the Edit panel draws explains itself on hover, from `slider_help`. A
    /// slider whose label the table does not know draws no help at all — which is how the
    /// first-use review found forty mute sliders next to three explained buttons (A-24).
    /// The collapsed sections are opened first, the way the reset sweep does it, so the
    /// count covers the whole panel and not the eleven sliders that show by default.
    #[test]
    fn every_edit_slider_has_hover_help() {
        let ctx = crate::test_ui::ctx();
        let buf = tone_fixture();
        let mut edits = edit_model::Edits::default();
        let mut channel = HslChannel::default();
        let mut curve = tone_curve::CurveChannel::default();
        // The Parametric view, so its sliders are in the sweep; the curve block open.
        let mut mode = tone_curve::CurveMode::Parametric;
        let mut open = true;
        let mut grade = color_wheel::GradeRange::default();
        let (mut show_range, mut view) = (true, None);
        let mut ed = Edited::default();
        let mut body = |ui: &mut egui::Ui| {
            ed = Edited::default();
            egui::ScrollArea::vertical().show(ui, |ui| {
                show(
                    ui,
                    &mut edits,
                    Channels {
                        hsl: &mut channel,
                        curve_mode: &mut mode,
                        curve: &mut curve,
                        curve_open: &mut open,
                        parametric_drag: Default::default(),
                    },
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
                        hist: None,
                        fresh: &edit_model::Edits::default(),
                    },
                    Pickers::default(),
                    &mut ed,
                );
            });
        };
        for h in ["Color", "Detail", "Lens", "Vignette"] {
            crate::test_ui::click(&ctx, h, &mut body);
        }
        crate::test_ui::click(&ctx, "Advanced color", &mut body);
        let labels = crate::test_ui::slider_labels(&ctx, &mut body);
        assert!(labels.len() > 30, "only {} sliders found", labels.len());
        // ⚠ What the sliders actually looked up — scope and label — not the labels alone: "Detail"
        // is two different sentences, and only the drawn pair says which one a slider got.
        assert!(
            ed.missing_help.is_empty(),
            "sliders without help: {:?}",
            ed.missing_help
        );
    }

    struct RangePanel {
        edits: edit_model::Edits,
        channel: HslChannel,
        grade: color_wheel::GradeRange,
        show_range: bool,
        view: Option<rustroom_app::grade_view::GradeRangeView>,
        ed: Edited,
        buf: core_image::ImageBuf,
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
            }
        }

        /// What the panel calls its sliders, as laid out right now.
        ///
        /// ⚠ Through the same harness the clicks go through, so "open the section, then open the block"
        /// is expressed the way a user does it. An earlier version forced the collapsing state by id and
        /// silently missed: `ui.make_persistent_id` is **relative to the Ui's id stack**, and the block's
        /// header is created inside the section's nested body, so the id computed at the top level was
        /// never the block's.
        fn slider_labels(&mut self, ctx: &egui::Context) -> Vec<String> {
            let Self {
                edits,
                channel,
                grade,
                show_range,
                view,
                ed,
                buf,
            } = self;
            crate::test_ui::slider_labels(ctx, |ui| {
                show(
                    ui,
                    edits,
                    Channels {
                        hsl: channel,
                        curve_mode: &mut tone_curve::CurveMode::default(),
                        curve: &mut tone_curve::CurveChannel::default(),
                        curve_open: &mut true,
                        parametric_drag: Default::default(),
                    },
                    GradeUi {
                        range: grade,
                        show_range,
                        view,
                    },
                    SourceInfo {
                        lens: None,
                        lens_profile_found: false,
                        wb_baseline: color::wb::Illuminant::new(5000.0, 0.0),
                        baseline_ev: None,
                        buffer: buf,
                        hist: None,
                        fresh: &edit_model::Edits::default(),
                    },
                    Pickers::default(),
                    ed,
                );
            })
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
            } = self;
            crate::test_ui::click(ctx, label, |ui| {
                *ed = Edited::default();
                show(
                    ui,
                    edits,
                    Channels {
                        hsl: channel,
                        curve_mode: &mut tone_curve::CurveMode::default(),
                        curve: &mut tone_curve::CurveChannel::default(),
                        curve_open: &mut true,
                        parametric_drag: Default::default(),
                    },
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
                        hist: None,
                        fresh: &edit_model::Edits::default(),
                    },
                    Pickers::default(),
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
            } = self;
            let labels = crate::test_ui::labelled_centres(ctx, Vec::new(), |ui| {
                *ed = Edited::default();
                show(
                    ui,
                    edits,
                    Channels {
                        hsl: channel,
                        curve_mode: &mut tone_curve::CurveMode::default(),
                        curve: &mut tone_curve::CurveChannel::default(),
                        curve_open: &mut true,
                        parametric_drag: Default::default(),
                    },
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
                        hist: None,
                        fresh: &edit_model::Edits::default(),
                    },
                    Pickers::default(),
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
        // ⚠ Grading moved inside `Advanced color` (2026-09-24); without this the harness finds
        // nothing to click and the test fails on a missing widget rather than on its subject.
        p.click(&ctx, "Advanced color");
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
        // ⚠ Grading moved inside `Advanced color` (2026-09-24); without this the harness finds
        // nothing to click and the test fails on a missing widget rather than on its subject.
        p.click(&ctx, "Advanced color");
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
                    None,
                    CurveUi {
                        mode: &mut tone_curve::CurveMode::default(),
                        channel: &mut tone_curve::CurveChannel::default(),
                        open: &mut true,
                        parametric_drag: Default::default(),
                    },
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
                        None,
                        CurveUi {
                            mode: &mut tone_curve::CurveMode::default(),
                            channel: &mut tone_curve::CurveChannel::default(),
                            open: &mut true,
                            parametric_drag: Default::default(),
                        },
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
            disabled: false,
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
