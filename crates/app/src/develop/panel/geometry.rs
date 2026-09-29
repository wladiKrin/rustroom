//! The Crop & Geometry panel: four sections — Crop, Straighten, Perspective, Lens.
//!
//! ⚠ Option A of the 2026-09-19 canvas ("Rustroom Geometry Panel"): the Edit and Masks panels'
//! chrome, the aspect as one segmented control that wraps rather than clips, the composition
//! guide as a dropdown, the level tool beside the angle, the guides on one row under the
//! perspective sliders they feed, and a dot and reset per section through
//! `edit_model::GeoGroup`.
//!
//! ⚠ Shown only while the **Crop** tool is selected on the rail. There is no stored
//! `geometry_mode` flag any more — it is derived from `Tool::is_geometry`, so the mode and the
//! selection cannot disagree.

use crate::develop::panel::{section, Edited};
use crate::theme;
use crate::theme::Hover as _;
use crate::Session;
use eframe::egui;
use rustroom_app::composition_guides::CompositionGuide;
use rustroom_app::crop_tool;

/// One entry of the aspect row.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Chip {
    Free,
    /// A preset, as a pixel ratio.
    Preset(f32),
    Custom,
}

/// The preset chips, in the order they are drawn.
const PRESETS: [(&str, f32); 4] = [
    ("1:1", 1.0),
    ("3:2", 1.5),
    ("4:3", 4.0 / 3.0),
    ("16:9", 16.0 / 9.0),
];

fn same_ratio(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

/// Whether `chip` is lit for the current `lock`, with `custom` the stored custom ratio.
///
/// ⚠ A ratio chip is lit by its ratio **or its inverse**, so 3:2 stays lit after a swap to 2:3
/// and `Swap` shows which way it faces. Pure, so the rule is testable without a window.
///
/// ⚠⚠ **One rule for all six chips, and NO tie-break between Custom and a preset** — the spec
/// first said a custom ratio equal to a preset lights the preset *instead*, and that was a dead
/// end found in use within the hour: the W:H fields draw only while Custom is lit, so a stored
/// 6:4 (which is 3:2) could never be edited again — every tap on Custom locked to 3:2 and lit
/// 3:2. Two chips lit says truthfully that two ratios agree, and the fields stay reachable.
fn chip_state(chip: Chip, lock: Option<f32>, custom: f32) -> bool {
    let matches =
        |ratio: f32| lock.is_some_and(|cur| same_ratio(ratio, cur) || same_ratio(1.0 / ratio, cur));
    match chip {
        Chip::Free => lock.is_none(),
        Chip::Preset(ratio) => matches(ratio),
        Chip::Custom => matches(custom),
    }
}

/// Lock to `ratio` (a pixel ratio) and snap the crop to it, centred and maximal — what a
/// preset chip and the Custom chip both do.
fn lock_to(
    ratio: f32,
    img_aspect: f32,
    s: &mut Session,
    crop_aspect: &mut Option<f32>,
    ed: &mut Edited,
) {
    *crop_aspect = Some(ratio);
    s.edits.geometry.crop = crop_tool::snap_to_aspect(ratio / img_aspect);
    ed.discrete();
}

/// The crop's aspect state — both halves tool state on `RustroomApp`, neither an edit.
///
/// ⚠ One struct rather than two parameters because `show` reached clippy's seven-argument
/// line, the same answer `FrameSpec` and `NameField` gave when they did. Both are `&mut` into
/// disjoint fields, so the caller builds it without cloning anything.
pub struct AspectLock<'a> {
    /// The pixel ratio the crop is locked to, or `None` for a free crop.
    pub lock: &'a mut Option<f32>,
    /// The stored custom ratio, W : H — persisted by the caller on change.
    pub custom: &'a mut [u32; 2],
}

/// Draw the geometry controls. The caller decides whether geometry mode is on.
pub fn show(
    ui: &mut egui::Ui,
    s: &mut Session,
    mut aspect: AspectLock<'_>,
    composition_guide: &mut CompositionGuide,
    lens_profile_found: bool,
    ed: &mut Edited,
    // Set when the Auto button was clicked. ⚠ Reported outward rather than handled here — see
    // the button itself for why.
    auto: &mut bool,
) {
    use edit_model::GeoGroup;

    // ── CROP ──
    let mut out = geo_header(ui, s, GeoGroup::Crop);
    out.show_body(ui, |ui| crop(ui, s, &mut aspect, composition_guide, ed));
    if out.reset {
        s.edits.reset_geo_group(GeoGroup::Crop);
        // ⚠ The lock is view state on the app, and a full-frame crop under a lock would snap
        // straight back; a reset crop is a free crop.
        *aspect.lock = None;
        ed.discrete();
    }

    // ── STRAIGHTEN ──
    let mut out = geo_header(ui, s, GeoGroup::Straighten);
    out.show_body(ui, |ui| straighten(ui, s, ed));
    if out.reset {
        s.edits.reset_geo_group(GeoGroup::Straighten);
        clear_guides(s);
        ed.discrete();
    }

    // ── PERSPECTIVE ──
    let mut out = geo_header(ui, s, GeoGroup::Perspective);
    out.show_body(ui, |ui| perspective(ui, s, ed, auto));
    if out.reset {
        s.edits.reset_geo_group(GeoGroup::Perspective);
        // ⚠ As editing the sliders by hand does: guides that solved to a correction that is
        // now gone would claim a tilt the picture no longer shows.
        clear_guides(s);
        ed.discrete();
    }

    // ── LENS ──
    // ⚠ Always drawn, with the lens in the header, so the section does not appear and vanish
    // with the profile; the body says why there is nothing to scale when there is not.
    let lens_label: String = match (&s.lens.name, lens_profile_found) {
        (Some(name), _) => name.clone(),
        (None, _) => "no lens recorded".to_owned(),
    };
    let mut out = section::header(
        ui,
        ui.make_persistent_id(("geo", "Lens")),
        true,
        section::Header::new("Lens").trailing(&lens_label),
    );
    out.show_body(ui, |ui| lens(ui, s, lens_profile_found, ed));
}

/// One of the three resettable headers, its dot from `GeoGroup`.
fn geo_header(ui: &mut egui::Ui, s: &Session, g: edit_model::GeoGroup) -> section::HeaderOut {
    section::header(
        ui,
        ui.make_persistent_id(("geo", g.name())),
        true,
        section::Header::new(g.name())
            .modified(s.edits.geo_group_differs(g))
            .resettable(true),
    )
}

fn clear_guides(s: &mut Session) {
    s.guides.clear();
    s.active_guide = None;
}

/// Height of the Crop section's button rows and its aspect control.
const CROP_ROW: f32 = 28.0;

/// The Crop section: orientation, the aspect, the custom ratio, the composition guide.
fn crop(
    ui: &mut egui::Ui,
    s: &mut Session,
    aspect: &mut AspectLock<'_>,
    composition_guide: &mut CompositionGuide,
    ed: &mut Edited,
) {
    let img_aspect = s.baseline.size_vec2().x / s.baseline.size_vec2().y;
    let custom_ratio = {
        let [w, h] = *aspect.custom;
        let r = rustroom_app::settings::CUSTOM_ASPECT_RANGE;
        w.clamp(r.0, r.1) as f32 / h.clamp(r.0, r.1) as f32
    };
    let custom_lit = chip_state(Chip::Custom, *aspect.lock, custom_ratio);

    // ── orientation, and Swap at the right ──
    // ⚠ The glyphs the panel has always used: they are known to render in the bundled font,
    // and a codepoint from the icon font that is not is a tofu box. The AccessKit label is the
    // glyph too, which the tests click by.
    let glyph_button = |ui: &mut egui::Ui, glyph: &str, hover: &str| -> bool {
        ui.add(
            egui::Button::new(egui::RichText::new(glyph).size(crate::theme::tokens::ICON))
                .min_size(egui::vec2(CROP_ROW, CROP_ROW)),
        )
        .hover(hover)
        .clicked()
    };
    let mut swap = false;
    ui.horizontal(|ui| {
        let g = &mut s.edits.geometry;
        let mut turned = false;
        if glyph_button(ui, "⟲", "Rotates the photo 90° to the left") {
            g.orientation.rotate_ccw();
            for _ in 0..3 {
                g.crop = g.crop.rotated_cw();
            }
            turned = true;
        }
        if glyph_button(ui, "⟳", "Rotates the photo 90° to the right") {
            g.orientation.rotate_cw();
            g.crop = g.crop.rotated_cw();
            turned = true;
        }
        ui.add_space(6.0);
        if glyph_button(ui, "⬌", "Flips the photo horizontally") {
            g.orientation.flip_h();
            g.crop = g.crop.flipped_h();
            turned = true;
        }
        if glyph_button(ui, "⬍", "Flips the photo vertically") {
            g.orientation.flip_v();
            g.crop = g.crop.flipped_h();
            g.crop = g.crop.rotated_cw();
            g.crop = g.crop.rotated_cw();
            turned = true;
        }
        if turned {
            ed.discrete();
        }
        // ⚠ Pinned to its width, never a bare `with_layout(right_to_left)` — the 2026-09-19
        // growth bug.
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), CROP_ROW),
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                swap = ui
                    .add(egui::Button::new("Swap").min_size(egui::vec2(0.0, CROP_ROW)))
                    .hover("Switches the crop between landscape and portrait")
                    .clicked();
            },
        );
    });
    if swap {
        s.edits.geometry.crop = crop_tool::swap_orientation(s.edits.geometry.crop, img_aspect);
        *aspect.lock = aspect.lock.map(|a| 1.0 / a);
        if custom_lit {
            let [w, h] = *aspect.custom;
            *aspect.custom = [h, w];
        }
        ed.discrete();
    }

    // ── the aspect ──
    // ⚠ One segmented control, never a row of chips: seven chips clipped at the panel's width.
    // The lit segment is *derived* from the lock each frame, as the chips were; a lock that
    // matches nothing lights nothing.
    let mut items: Vec<(Option<Chip>, &str, &str)> = vec![(Some(Chip::Free), "Free", "")];
    for (label, ratio) in PRESETS {
        items.push((Some(Chip::Preset(ratio)), label, ""));
    }
    items.push((
        Some(Chip::Custom),
        "W:H",
        "Locks the crop to the ratio entered below",
    ));
    let mut cur: Option<Chip> = items
        .iter()
        .map(|(c, _, _)| c.expect("every item is Some"))
        .find(|c| chip_state(*c, *aspect.lock, custom_ratio));
    let before = cur;
    let font = egui::TextStyle::Body.resolve(ui.style());
    theme::segmented_sized(ui, &mut cur, &items, CROP_ROW, font);
    if cur != before {
        match cur {
            Some(Chip::Free) => *aspect.lock = None,
            Some(Chip::Preset(ratio)) => lock_to(ratio, img_aspect, s, aspect.lock, ed),
            Some(Chip::Custom) => lock_to(custom_ratio, img_aspect, s, aspect.lock, ed),
            None => {}
        }
    }

    // ── the custom ratio, only while W:H is lit ──
    if chip_state(Chip::Custom, *aspect.lock, custom_ratio) {
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), ui.spacing().interact_size.y + 4.0),
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                let r = rustroom_app::settings::CUSTOM_ASPECT_RANGE;
                let field = |ui: &mut egui::Ui, v: &mut u32| -> egui::Response {
                    ui.add(egui::DragValue::new(v).range(r.0..=r.1))
                };
                // ⚠ Right to left: the height field lands first, at the right edge.
                let [w, h] = &mut *aspect.custom;
                let rh = field(ui, h);
                ui.label(":");
                let rw = field(ui, w);
                ui.label(egui::RichText::new("Custom").weak());
                let changed = rw.changed() || rh.changed();
                if changed {
                    let ratio = {
                        let [w, h] = *aspect.custom;
                        w.clamp(r.0, r.1) as f32 / h.clamp(r.0, r.1) as f32
                    };
                    *aspect.lock = Some(ratio);
                    s.edits.geometry.crop = crop_tool::snap_to_aspect(ratio / img_aspect);
                    ed.changed = true;
                }
                let dragging = rw.dragged() || rh.dragged();
                if rw.drag_stopped() || rh.drag_stopped() || (changed && !dragging) {
                    ed.settled = true;
                }
            },
        );
    }

    // ── the composition guide ──
    // ⚠ `from_label`, not an id salt: the label is the combo's accessible name (an id salt
    // leaves it nameless, which the harness found first), and in a right-to-left row egui
    // draws it to the combo's left — "Composition guide  [Rule of thirds ▾]".
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), ui.spacing().interact_size.y + 4.0),
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                egui::ComboBox::from_label("Composition guide")
                    .selected_text(composition_guide.label())
                    .show_ui(ui, |ui| {
                        for g in CompositionGuide::ALL {
                            ui.selectable_value(composition_guide, g, g.label());
                        }
                    })
                    .response
                    .hover("Chooses the guide drawn inside the crop (O cycles through them)");
            },
        );
    });
}

/// The Straighten section: the angle, and the draw-a-line tool beside it.
fn straighten(ui: &mut egui::Ui, s: &mut Session, ed: &mut Edited) {
    ui.horizontal(|ui| {
        let w = ui.available_width() - CROP_ROW - ui.spacing().item_spacing.x;
        ui.allocate_ui_with_layout(
            egui::vec2(w.max(60.0), ui.spacing().interact_size.y),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ed.slider(ui, &mut s.edits.geometry.angle, -45.0..=45.0, "Angle", 0.0);
            },
        );
        if level_button(ui, s.straighten_armed).clicked() {
            s.straighten_armed = !s.straighten_armed;
            s.straighten_line = None;
        }
    });
}

/// The level tool's toggle: a hand-painted level glyph, lit while the tool is armed.
///
/// ⚠ The accessible name is the old button's text, so a screen reader — and the harness — still
/// gets a sentence rather than a picture.
fn level_button(ui: &mut egui::Ui, armed: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(CROP_ROW, CROP_ROW), egui::Sense::click());
    let enabled = ui.is_enabled();
    resp.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::Checkbox,
            enabled,
            armed,
            "Straighten by drawing a line",
        )
    });
    let resp = resp
        .hover("Straightens the photo along a line you drag over something that should be level");
    if ui.is_rect_visible(rect) {
        let v = ui.visuals();
        let accent = v.selection.bg_fill;
        let (fill, colour) = if armed {
            (accent.gamma_multiply(0.18), accent)
        } else if resp.hovered() {
            (v.widgets.hovered.weak_bg_fill, v.strong_text_color())
        } else {
            (
                v.widgets.inactive.weak_bg_fill,
                v.widgets.inactive.fg_stroke.color,
            )
        };
        let painter = ui.painter();
        painter.rect_filled(rect, 4.0, fill);
        let r = rect.shrink(7.0);
        let stroke = egui::Stroke::new(1.6, colour);
        painter.line_segment(
            [
                egui::pos2(r.left(), r.bottom() - r.height() * 0.2),
                egui::pos2(r.right(), r.top() + r.height() * 0.2),
            ],
            stroke,
        );
        painter.circle_filled(
            egui::pos2(r.left() + r.width() * 0.2, r.bottom() - r.height() * 0.28),
            2.0,
            colour,
        );
        painter.circle_filled(
            egui::pos2(r.right() - r.width() * 0.2, r.top() + r.height() * 0.28),
            2.0,
            colour,
        );
    }
    resp
}

/// The Perspective section: the four sliders, then the guides on one row.
fn perspective(ui: &mut egui::Ui, s: &mut Session, ed: &mut Edited, auto: &mut bool) {
    let solved_before = (
        s.edits.perspective.vertical,
        s.edits.perspective.horizontal,
        s.edits.geometry.angle,
    );
    let p = &mut s.edits.perspective;
    ed.slider(ui, &mut p.vertical, -100.0..=100.0, "Vertical", 0.0);
    ed.slider(ui, &mut p.horizontal, -100.0..=100.0, "Horizontal", 0.0);
    ed.slider(ui, &mut p.aspect, -100.0..=100.0, "Aspect ⬍ ⬌", 0.0);
    // ⚠ The field's own default, −100, not 0: a double-click used to land on the raw,
    // clipping view (P-11). Its help text lives in `slider_help` with every other slider's.
    ed.slider(
        ui,
        &mut p.scale,
        -100.0..=100.0,
        "Scale",
        edit_model::Perspective::default().scale,
    );
    if (
        s.edits.perspective.vertical,
        s.edits.perspective.horizontal,
        s.edits.geometry.angle,
    ) != solved_before
        && !s.guides.is_empty()
    {
        clear_guides(s);
    }

    // ── the guides, one row ──
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        if ui
            .toggle_value(&mut s.show_guides, "Guide mode")
            .hover(
                "Corrects perspective from lines you draw along edges that should be vertical or horizontal",
            )
            .changed()
        {
            ed.discrete();
        }
        let refining = !s.guides.is_empty();
        let label = if refining { "Refine guides" } else { "Auto" };
        let hint = if refining {
            "Move each guide you drew onto the nearest strong edge. A guide with no edge near it \
             is left exactly where you put it."
        } else {
            "Find two vertical edges in the photograph and solve the tilt from them. The guides \
             it infers are left on screen so you can nudge, refine or delete them."
        };
        if ui.button(label).hover(hint).clicked() {
            *auto = true;
        }
        if ui
            .add_enabled(refining, egui::Button::new("Clear"))
            .hover("Removes the guide lines, keeping the corrections they made")
            .clicked()
        {
            clear_guides(s);
        }
    });
}

/// The Lens section: the lens scale, or why there is nothing to scale.
///
/// ⚠ The lens **Scale** appears here as well as in the Edit panel's Lens section, and this is
/// the one place it can actually be judged: crop mode previews the *uncropped* frame, which is
/// where the empty corners that pincushion correction leaves are visible at all. In the Edit
/// panel the crop is applied, so if you have cropped even slightly the gaps are already outside
/// it and the slider looks inert. One field, relevant to two decisions, edited from whichever
/// view can see its effect.
///
/// ⚠ Labelled "Lens scale", not "Scale": Perspective has a Scale, and two controls on one panel
/// must not share a name.
fn lens(ui: &mut egui::Ui, s: &mut Session, lens_profile_found: bool, ed: &mut Edited) {
    let distortion = s.edits.applied_lens_correction().distortion;
    if lens_profile_found && distortion {
        if let Some(l) = &mut s.edits.lens_correction {
            ed.slider(
                ui,
                &mut l.scale,
                0.0..=100.0,
                "Lens scale",
                edit_model::LensCorrection::default().scale,
            );
        }
    } else if !lens_profile_found {
        ui.label(
            egui::RichText::new("No profile for this lens, so there is nothing to scale.").weak(),
        );
    } else {
        ui.label(egui::RichText::new("Distortion is off in the Edit panel's Lens section.").weak());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_ui;

    fn session() -> Session {
        let ctx = test_ui::ctx();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        app.session.take().unwrap()
    }

    /// An off-centre, non-square crop — so every rigid transform of it is distinguishable.
    ///
    /// ⚠ A centred or full-frame crop is invariant under most of these, which would make the
    /// tests below pass with the crop left alone entirely.
    fn asymmetric() -> edit_model::CropRect {
        edit_model::CropRect {
            left: 0.1,
            top: 0.2,
            right: 0.6,
            bottom: 0.5,
        }
    }

    /// ⚠ Approximate, always. `1.0 - (1.0 - 0.1)` is `0.100000024`, so a crop put through any
    /// rigid transform and back does not compare exactly — an earlier version of
    /// `flipping_twice_is_the_identity` demanded `assert_eq!` and failed against correct code. A
    /// millionth of the frame is sub-pixel on any image.
    fn assert_crop_close(a: edit_model::CropRect, b: edit_model::CropRect) {
        for (x, y) in [
            (a.left, b.left),
            (a.top, b.top),
            (a.right, b.right),
            (a.bottom, b.bottom),
        ] {
            assert!((x - y).abs() < 1e-6, "crop drifted: {a:?} vs {b:?}");
        }
    }

    fn click_in_panel(label: &str, s: &mut Session, lens: bool) -> Edited {
        click_in_panel_reporting(label, s, lens).0
    }

    /// ⚠⚠ **The label carries the state.** One button doing two things with nothing on screen
    /// saying which is the shape that produced this project's greyed-out straighten-button
    /// confusion, whose lesson is "a rule the user cannot see is one they cannot correct."
    ///
    /// ⚠ Asserts **both** labels, and that each is ABSENT in the other state. A test that only
    /// looked for "Refine guides" would pass against a panel drawing both buttons at once.
    #[test]
    fn the_auto_button_says_which_of_its_two_jobs_it_will_do() {
        let ctx = test_ui::ctx();
        let seen = |label: &str, with_guide: bool| {
            let mut s = session();
            if with_guide {
                s.guides.push(pipeline::perspective::guides::Guide {
                    from: [0.2, 0.2],
                    to: [0.2, 0.8],
                });
            }
            let mut aspect = None;
            let mut custom = rustroom_app::settings::DEFAULT_CUSTOM_ASPECT;
            let mut ed = Edited::default();
            test_ui::find(&ctx, label, |ui| {
                show(
                    ui,
                    &mut s,
                    AspectLock {
                        lock: &mut aspect,
                        custom: &mut custom,
                    },
                    &mut CompositionGuide::Off,
                    false,
                    &mut ed,
                    &mut false,
                );
            })
            .is_some()
        };
        assert!(
            seen("Auto", false),
            "with no guides it should offer to find them"
        );
        assert!(!seen("Refine guides", false), "nothing to refine yet");
        assert!(
            seen("Refine guides", true),
            "with a guide it should offer to refine"
        );
        assert!(!seen("Auto", true), "the two labels must not both be drawn");
    }

    /// The one harness, with the Auto click it reports outward.
    ///
    /// ⚠ `click_in_panel` is a thin wrapper over this rather than a second copy: a helper
    /// duplicated per test is how two harnesses come to disagree about what a click is.
    fn click_in_panel_reporting(label: &str, s: &mut Session, lens: bool) -> (Edited, bool) {
        let mut aspect = None;
        let mut custom = rustroom_app::settings::DEFAULT_CUSTOM_ASPECT;
        click_in_panel_with(label, s, lens, &mut aspect, &mut custom)
    }

    /// The harness with the lock and the stored custom ratio exposed, for the aspect tests.
    fn click_in_panel_with(
        label: &str,
        s: &mut Session,
        lens: bool,
        aspect: &mut Option<f32>,
        custom: &mut [u32; 2],
    ) -> (Edited, bool) {
        let ctx = test_ui::ctx();
        let mut ed = Edited::default();
        let mut auto = false;
        test_ui::click(&ctx, label, |ui| {
            ed = Edited::default();
            auto = false;
            show(
                ui,
                s,
                AspectLock {
                    lock: &mut *aspect,
                    custom: &mut *custom,
                },
                &mut CompositionGuide::Off,
                lens,
                &mut ed,
                &mut auto,
            );
        });
        (ed, auto)
    }

    /// ⚠⚠ **The button exists and is reachable.** A test on `auto_guides` alone cannot see that
    /// nothing calls it — the defect shape this project's records name most often, and the half
    /// it has repeatedly shipped broken.
    #[test]
    fn the_auto_button_is_drawn_in_the_guides_block() {
        let mut s = session();
        let (_, auto) = click_in_panel_reporting("Auto", &mut s, false);
        assert!(auto, "clicking Auto reported nothing to the caller");
    }

    /// ⚠ The discriminating other half: a panel that reported `true` unconditionally would pass
    /// the test above. Clicking a *different* control must leave it alone.
    #[test]
    fn clicking_something_else_does_not_report_an_auto() {
        let mut s = session();
        let (_, auto) = click_in_panel_reporting("Clear", &mut s, false);
        assert!(!auto, "clicking Clear reported an Auto");
    }

    /// Draw the panel with one shared context, with `guide` as the composition guide.
    fn composition_labels(
        ctx: &egui::Context,
        s: &mut Session,
        guide: &mut CompositionGuide,
    ) -> Vec<String> {
        let mut aspect = None;
        let mut custom = rustroom_app::settings::DEFAULT_CUSTOM_ASPECT;
        test_ui::labelled_centres(ctx, Vec::new(), |ui| {
            let mut ed = Edited::default();
            show(
                ui,
                s,
                AspectLock {
                    lock: &mut aspect,
                    custom: &mut custom,
                },
                guide,
                false,
                &mut ed,
                &mut false,
            );
        })
        .into_iter()
        .map(|(l, _)| l)
        .collect()
    }

    fn click_composition(
        ctx: &egui::Context,
        s: &mut Session,
        guide: &mut CompositionGuide,
        label: &str,
    ) {
        let mut aspect = None;
        let mut custom = rustroom_app::settings::DEFAULT_CUSTOM_ASPECT;
        test_ui::click(ctx, label, |ui| {
            let mut ed = Edited::default();
            show(
                ui,
                s,
                AspectLock {
                    lock: &mut aspect,
                    custom: &mut custom,
                },
                guide,
                false,
                &mut ed,
                &mut false,
            );
        });
    }

    #[test]
    fn every_composition_guide_is_offered_by_the_dropdown() {
        // ⚠ Through the shipped combo: its button is reachable by the current guide's name
        // (egui gives a combo only its selected text, as a value — `test_ui` falls back to it),
        // and the items appear once it is open. One context across the frames, or the popup is
        // gone.
        let ctx = test_ui::ctx();
        let mut s = session();
        let mut guide = CompositionGuide::Off;
        click_composition(&ctx, &mut s, &mut guide, CompositionGuide::Off.label());
        let labels = composition_labels(&ctx, &mut s, &mut guide);
        for g in CompositionGuide::ALL {
            assert!(
                labels.iter().any(|l| l == g.label()),
                "{g:?} is not offered; drawn: {labels:?}"
            );
        }
    }

    #[test]
    fn picking_a_composition_guide_from_the_dropdown_selects_it() {
        let ctx = test_ui::ctx();
        let mut s = session();
        let mut guide = CompositionGuide::Off;
        click_composition(&ctx, &mut s, &mut guide, CompositionGuide::Off.label());
        click_composition(
            &ctx,
            &mut s,
            &mut guide,
            CompositionGuide::Diagonals.label(),
        );
        assert_eq!(guide, CompositionGuide::Diagonals);
    }

    #[test]
    fn drawing_the_composition_control_is_not_a_photo_edit() {
        // ⚠ The guide is a view preference, so merely drawing its control must leave `Edited`
        // alone — otherwise the panel would re-render the frame and push a history entry on
        // every pass, which reads as the photo changing by itself.
        let ctx = test_ui::ctx();
        let mut s = session();
        let mut aspect = None;
        let mut custom = rustroom_app::settings::DEFAULT_CUSTOM_ASPECT;
        let mut guide = CompositionGuide::Grid;
        let mut ed = Edited::default();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            ed = Edited::default();
            show(
                ui,
                &mut s,
                AspectLock {
                    lock: &mut aspect,
                    custom: &mut custom,
                },
                &mut guide,
                false,
                &mut ed,
                &mut false,
            );
        });
        assert!(!ed.changed && !ed.settled, "the guide is not an edit");
    }

    #[test]
    fn every_orientation_button_moves_the_crop_with_it() {
        // ⚠⚠ The invariant this panel's own comment names: each button must move the crop by the
        // *same* transform it applies to the orientation, or the crop lands on different content.
        // A button that turns the image and leaves the crop behind looks right on an uncropped
        // photo and is silently wrong on every cropped one — the same shape as the crop-factor
        // bug that was invisible on 400 files and total on the one that was cropped.
        for label in ["⟲", "⟳", "⬌", "⬍"] {
            let mut s = session();
            s.edits.geometry.crop = asymmetric();
            let before = s.edits.geometry;

            click_in_panel(label, &mut s, false);

            let after = s.edits.geometry;
            assert_ne!(
                before.orientation, after.orientation,
                "{label} did not change the orientation"
            );
            assert_ne!(
                before.crop, after.crop,
                "{label} changed the orientation but left the crop behind"
            );
        }
    }

    #[test]
    fn turning_left_then_right_returns_both_to_where_they_started() {
        // ⚠ Rotating left is implemented as *three* clockwise crop turns, which is the fiddly
        // half. A wrong count leaves the crop somewhere else while the orientation is back.
        let mut s = session();
        s.edits.geometry.crop = asymmetric();
        let before = s.edits.geometry;

        click_in_panel("⟳", &mut s, false);
        click_in_panel("⟲", &mut s, false);

        assert_eq!(before.orientation, s.edits.geometry.orientation);
        assert_crop_close(before.crop, s.edits.geometry.crop);
    }

    #[test]
    fn flipping_twice_is_the_identity() {
        for label in ["⬌", "⬍"] {
            let mut s = session();
            s.edits.geometry.crop = asymmetric();
            let before = s.edits.geometry;
            click_in_panel(label, &mut s, false);
            click_in_panel(label, &mut s, false);
            assert_eq!(
                before.orientation, s.edits.geometry.orientation,
                "{label} twice should undo itself"
            );
            assert_crop_close(before.crop, s.edits.geometry.crop);
        }
    }

    #[test]
    fn a_ratio_chip_is_lit_by_its_ratio_and_by_its_inverse() {
        // ⚠ The inverse too: after a swap the lock is 2:3 and the 3:2 chip must stay lit, with
        // Swap showing which way it faces. A chip that went dark on swap would read as "no
        // preset", which is false.
        assert!(chip_state(Chip::Preset(1.5), Some(1.5), 1.25));
        assert!(chip_state(Chip::Preset(1.5), Some(1.0 / 1.5), 1.25));
        assert!(!chip_state(Chip::Preset(1.5), Some(1.0), 1.25));
        assert!(!chip_state(Chip::Preset(1.5), None, 1.25));
    }

    #[test]
    fn free_is_lit_only_with_no_lock() {
        assert!(chip_state(Chip::Free, None, 1.25));
        assert!(!chip_state(Chip::Free, Some(1.0), 1.25));
    }

    #[test]
    fn custom_is_lit_by_the_stored_ratio_even_when_a_preset_matches_too() {
        assert!(chip_state(Chip::Custom, Some(1.25), 1.25));
        assert!(chip_state(Chip::Custom, Some(0.8), 1.25), "its inverse too");
        assert!(!chip_state(Chip::Custom, Some(1.5), 1.25));
        assert!(!chip_state(Chip::Custom, None, 1.25));
        // ⚠⚠ NO tie-break, and this reverses the spec's first rule — found in use within the
        // hour. A custom ratio equal to a preset (6:4 is 3:2) used to light the preset *instead*,
        // and since the W:H fields draw only while Custom is lit, the custom ratio became
        // uneditable for good: every tap on Custom locked to 3:2 and lit 3:2. Two chips lit says
        // truthfully that two ratios agree, and the fields stay reachable.
        assert!(chip_state(Chip::Custom, Some(1.5), 1.5));
        assert!(chip_state(Chip::Preset(1.5), Some(1.5), 1.5));
    }

    /// ⚠⚠ The report, verbatim: *"custom button worked the first time, but now by tipping on it
    /// it goes to 3:2 all the time."* The settings file held `[6, 4]`.
    #[test]
    fn a_custom_ratio_equal_to_a_preset_stays_editable() {
        let ctx = test_ui::ctx();
        let mut s = session();
        let mut aspect = None;
        let mut custom = [6, 4];
        click_in_panel_with("W:H", &mut s, false, &mut aspect, &mut custom);
        assert!((aspect.unwrap() - 1.5).abs() < 1e-6);
        let mut ed = Edited::default();
        let values = test_ui::numeric_values(&ctx, |ui| {
            show(
                ui,
                &mut s,
                AspectLock {
                    lock: &mut aspect,
                    custom: &mut custom,
                },
                &mut CompositionGuide::Off,
                false,
                &mut ed,
                &mut false,
            );
        });
        let has = |v: f64| values.iter().any(|x| (x - v).abs() < 1e-9);
        assert!(
            has(6.0) && has(4.0),
            "the W:H fields vanished, so 6:4 can never be edited again: {values:?}"
        );
    }

    /// The image the harness opens is 80×40, aspect 2.0 — every crop below is on that.
    const IMG_ASPECT: f32 = 2.0;

    #[test]
    fn custom_and_swap_are_offered() {
        let ctx = test_ui::ctx();
        let mut s = session();
        let mut aspect = None;
        let mut custom = rustroom_app::settings::DEFAULT_CUSTOM_ASPECT;
        let mut ed = Edited::default();
        let drawn: Vec<String> = test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
            show(
                ui,
                &mut s,
                AspectLock {
                    lock: &mut aspect,
                    custom: &mut custom,
                },
                &mut CompositionGuide::Off,
                false,
                &mut ed,
                &mut false,
            );
        })
        .into_iter()
        .map(|(l, _)| l)
        .collect();
        for want in ["W:H", "Swap"] {
            assert!(drawn.iter().any(|l| l == want), "{want} missing: {drawn:?}");
        }
    }

    #[test]
    fn swap_under_a_lock_inverts_it_and_transposes_the_crop() {
        let mut s = session();
        s.edits.geometry.crop = asymmetric();
        let mut aspect = Some(1.5);
        let mut custom = rustroom_app::settings::DEFAULT_CUSTOM_ASPECT;
        let (ed, _) = click_in_panel_with("Swap", &mut s, false, &mut aspect, &mut custom);
        assert!(ed.settled, "a swap is one undo step");
        assert!((aspect.unwrap() - 1.0 / 1.5).abs() < 1e-6, "{aspect:?}");
        assert_crop_close(
            s.edits.geometry.crop,
            crate::crop_tool::swap_orientation(asymmetric(), IMG_ASPECT),
        );
        assert_eq!(
            custom,
            rustroom_app::settings::DEFAULT_CUSTOM_ASPECT,
            "not a custom chip"
        );
    }

    #[test]
    fn swap_on_a_free_crop_only_transposes() {
        let mut s = session();
        s.edits.geometry.crop = asymmetric();
        let mut aspect = None;
        let mut custom = rustroom_app::settings::DEFAULT_CUSTOM_ASPECT;
        let (ed, _) = click_in_panel_with("Swap", &mut s, false, &mut aspect, &mut custom);
        assert!(ed.settled);
        assert_eq!(aspect, None, "a free crop stays free");
        assert_crop_close(
            s.edits.geometry.crop,
            crate::crop_tool::swap_orientation(asymmetric(), IMG_ASPECT),
        );
    }

    #[test]
    fn clicking_custom_locks_to_the_stored_ratio_and_snaps() {
        let mut s = session();
        s.edits.geometry.crop = asymmetric();
        let mut aspect = None;
        let mut custom = [21, 9];
        let (ed, _) = click_in_panel_with("W:H", &mut s, false, &mut aspect, &mut custom);
        assert!(ed.settled);
        assert!((aspect.unwrap() - 21.0 / 9.0).abs() < 1e-6, "{aspect:?}");
        assert_crop_close(
            s.edits.geometry.crop,
            crate::crop_tool::snap_to_aspect((21.0 / 9.0) / IMG_ASPECT),
        );
    }

    #[test]
    fn swap_under_custom_exchanges_the_stored_pair() {
        let mut s = session();
        let mut aspect = Some(21.0 / 9.0);
        let mut custom = [21, 9];
        click_in_panel_with("Swap", &mut s, false, &mut aspect, &mut custom);
        assert_eq!(
            custom,
            [9, 21],
            "the numbers on screen must agree with the lock"
        );
        assert!((aspect.unwrap() - 9.0 / 21.0).abs() < 1e-6);
    }

    #[test]
    fn the_ratio_fields_are_drawn_only_while_custom_is_lit() {
        let ctx = test_ui::ctx();
        let values_with = |lock: Option<f32>| {
            let mut s = session();
            let mut aspect = lock;
            let mut custom = [21, 9];
            let mut ed = Edited::default();
            test_ui::numeric_values(&ctx, |ui| {
                show(
                    ui,
                    &mut s,
                    AspectLock {
                        lock: &mut aspect,
                        custom: &mut custom,
                    },
                    &mut CompositionGuide::Off,
                    false,
                    &mut ed,
                    &mut false,
                );
            })
        };
        // ⚠ By numeric value, not by label: a `DragValue` has no AccessKit label at all, so
        // `labelled_centres` returns the whole panel without it — see `test_ui::numeric_values`.
        // 21 and 9 are values no other control in this panel sits at.
        let has = |vs: &[f64], v: f64| vs.iter().any(|x| (x - v).abs() < 1e-9);
        let lit = values_with(Some(21.0 / 9.0));
        assert!(has(&lit, 21.0) && has(&lit, 9.0), "fields missing: {lit:?}");
        let dark = values_with(Some(1.5));
        assert!(!has(&dark, 21.0), "fields drawn under a preset: {dark:?}");
        let free = values_with(None);
        assert!(!has(&free, 21.0), "fields drawn under Free: {free:?}");
    }

    #[test]
    fn the_lens_scale_appears_only_when_a_profile_matched_and_distortion_is_on() {
        // ⚠ The slider is here *as well as* in the Lens section because this is the one view
        // where its effect is visible — crop mode previews the uncropped frame. Showing it with
        // no profile would offer a control that cannot do anything.
        let ctx = test_ui::ctx();
        let seen = |lens: bool, distortion: bool| {
            let mut s = session();
            s.edits.lens_correction = Some(edit_model::LensCorrection {
                distortion,
                ..Default::default()
            });
            let mut aspect = None;
            let mut custom = rustroom_app::settings::DEFAULT_CUSTOM_ASPECT;
            let mut ed = Edited::default();
            test_ui::find(&ctx, "Lens scale", |ui| {
                show(
                    ui,
                    &mut s,
                    AspectLock {
                        lock: &mut aspect,
                        custom: &mut custom,
                    },
                    &mut CompositionGuide::Off,
                    lens,
                    &mut ed,
                    &mut false,
                );
            })
            .is_some()
        };
        assert!(
            seen(true, true),
            "a matched profile with distortion on shows it"
        );
        assert!(!seen(false, true), "no profile, no slider");
        assert!(!seen(true, false), "distortion off, no slider");
    }

    /// ⚠⚠ The two Scale sliders must stay distinguishable. They are different corrections with
    /// different autoscales, and once the "Lens" and "Perspective" headings have scrolled out of
    /// view two sliders both called "Scale" say nothing about which is which.
    #[test]
    fn the_lens_scale_and_the_perspective_scale_have_different_labels() {
        let ctx = test_ui::ctx();
        let mut s = session();
        s.edits.lens_correction = Some(edit_model::LensCorrection {
            distortion: true,
            ..Default::default()
        });
        let mut aspect = None;
        let mut custom = rustroom_app::settings::DEFAULT_CUSTOM_ASPECT;
        let mut ed = Edited::default();
        let found = test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
            show(
                ui,
                &mut s,
                AspectLock {
                    lock: &mut aspect,
                    custom: &mut custom,
                },
                &mut CompositionGuide::Off,
                true,
                &mut ed,
                &mut false,
            );
        });
        let scales: Vec<&String> = found
            .iter()
            .map(|(l, _)| l)
            .filter(|l| l.contains("cale"))
            .collect();
        assert_eq!(scales.len(), 2, "expected exactly two, got {scales:?}");
        assert_ne!(scales[0], scales[1], "both are called the same thing");
    }

    /// ⚠ That the sliders are actually drawn. A section added to the wrong branch compiles, runs,
    /// and is simply never on screen — which no other test in this file would notice.
    /// ⚠ Scale is the one slider whose neutral is not zero: −100 fits the corrected image. A
    /// double-click used to send it to 0, the raw clipping view (P-11); the Edit panel's sweep
    /// never saw it because Geometry is another tool.
    #[test]
    fn scale_resets_to_its_own_default_on_double_click() {
        let ctx = test_ui::ctx();
        for theme in [egui::Theme::Dark, egui::Theme::Light] {
            ctx.style_mut_of(theme, |s| s.animation_time = 0.0);
        }
        let mut s = session();
        let mut aspect = None;
        let mut custom = rustroom_app::settings::DEFAULT_CUSTOM_ASPECT;
        let mut ed = Edited::default();
        // The body owns `s` for as long as it lives, so the value is read out through a cell.
        let scale = std::cell::Cell::new(f32::NAN);
        let mut body = |ui: &mut egui::Ui| {
            show(
                ui,
                &mut s,
                AspectLock {
                    lock: &mut aspect,
                    custom: &mut custom,
                },
                &mut CompositionGuide::Off,
                false,
                &mut ed,
                &mut false,
            );
            scale.set(s.edits.perspective.scale);
        };
        let sliders = test_ui::slider_centres(&ctx, &mut body);
        let (_, at) = sliders
            .iter()
            .find(|(l, _)| l == "Scale")
            .expect("a Scale slider");
        let off = *at + egui::vec2(40.0, 0.0);
        test_ui::click_at(&ctx, off, false, &mut body);
        assert_ne!(scale.get(), -100.0, "the click must move it first");
        test_ui::click_at(&ctx, off, true, &mut body);
        assert_eq!(scale.get(), -100.0);
    }

    /// Every slider the Geometry tool draws has a help sentence — see the Edit panel's twin.
    #[test]
    fn every_geometry_slider_has_hover_help() {
        let ctx = test_ui::ctx();
        let mut s = session();
        let mut aspect = None;
        let mut custom = rustroom_app::settings::DEFAULT_CUSTOM_ASPECT;
        let mut ed = Edited::default();
        let labels = test_ui::slider_labels(&ctx, |ui| {
            show(
                ui,
                &mut s,
                AspectLock {
                    lock: &mut aspect,
                    custom: &mut custom,
                },
                &mut CompositionGuide::Off,
                false,
                &mut ed,
                &mut false,
            );
        });
        assert!(labels.len() >= 5, "only {} sliders found", labels.len());
        assert!(
            ed.missing_help.is_empty(),
            "sliders without help: {:?}",
            ed.missing_help
        );
    }

    #[test]
    fn the_perspective_sliders_are_drawn() {
        let ctx = test_ui::ctx();
        let mut s = session();
        let mut aspect = None;
        let mut custom = rustroom_app::settings::DEFAULT_CUSTOM_ASPECT;
        let mut ed = Edited::default();
        let found = test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
            show(
                ui,
                &mut s,
                AspectLock {
                    lock: &mut aspect,
                    custom: &mut custom,
                },
                &mut CompositionGuide::Off,
                false,
                &mut ed,
                &mut false,
            );
        });
        for want in ["Vertical", "Horizontal", "Aspect", "Scale"] {
            assert!(
                found.iter().any(|(label, _)| label.contains(want)),
                "no “{want}” slider in the Geometry tool; found {:?}",
                found.iter().map(|(l, _)| l).collect::<Vec<_>>()
            );
        }
    }

    /// ⚠ The rail must say what the tool now does. Asserted on `Tool::label` rather than by
    /// finding the widget, because the rail is drawn by a different module entirely.
    #[test]
    fn the_rail_calls_the_crop_tool_crop_and_geometry() {
        assert_eq!(rustroom_app::tool::Tool::Crop.label(), "Crop & Geometry");
    }

    /// ⚠⚠ The reset button covers the whole Geometry group, perspective included. Without this
    /// the button's promise is only checked by reading its hover text.
    #[test]
    fn a_perspective_reset_clears_it_and_leaves_the_crop_alone() {
        // ⚠ Only Perspective is modified, so exactly one `reset` is drawn — the harness cannot
        // tell two apart.
        let mut s = session();
        s.edits.perspective = edit_model::Perspective {
            vertical: 40.0,
            ..Default::default()
        };
        s.guides.push(pipeline::perspective::guides::Guide {
            from: [0.2, 0.2],
            to: [0.2, 0.8],
        });
        let ed = click_in_panel("reset", &mut s, false);
        assert_eq!(s.edits.perspective, edit_model::Perspective::default());
        assert!(s.guides.is_empty(), "a reset perspective keeps no guides");
        assert!(ed.settled, "one click, one history entry");
    }

    #[test]
    fn a_crop_reset_leaves_the_angle_and_the_perspective_alone() {
        let mut s = session();
        s.edits.geometry.crop = asymmetric();
        let mut aspect = Some(1.5);
        let mut custom = rustroom_app::settings::DEFAULT_CUSTOM_ASPECT;
        click_in_panel_with("reset", &mut s, false, &mut aspect, &mut custom);
        assert_eq!(s.edits.geometry.crop, edit_model::CropRect::default());
        assert_eq!(aspect, None, "a reset crop is a free crop");
    }

    #[test]
    fn the_level_toggle_arms_the_line_tool_by_its_accessible_name() {
        let mut s = session();
        assert!(!s.straighten_armed);
        click_in_panel("Straighten by drawing a line", &mut s, false);
        assert!(s.straighten_armed, "the toggle did not arm the tool");
        click_in_panel("Straighten by drawing a line", &mut s, false);
        assert!(!s.straighten_armed, "nor disarm it");
    }

    /// ⚠⚠ `↔` and `↕` — the obvious choice — are **absent from every bundled font** and would
    /// have shipped as tofu boxes, invisible to every other test. Probed before choosing, which
    /// is the fourth time that has paid off here after the rail's four glyphs, the mark badges
    /// and the keyword checkmarks.
    #[test]
    fn every_perspective_glyph_is_renderable() {
        let ctx = test_ui::ctx();
        // ⚠ Both lines are load-bearing, and leaving them out is why this test failed first.
        // Without `install_fonts` it asks about egui's *defaults*, which production no longer
        // uses; and `has_glyph` needs a frame to have run, because a fresh context has not built
        // its atlas and answers about nothing.
        crate::theme::install_fonts(&ctx);
        let _ = ctx.run_ui(egui::RawInput::default(), |_ui| {});
        let id = egui::FontId::proportional(14.0);
        for g in ['⬍', '⬌'] {
            assert!(
                ctx.fonts_mut(|f| f.has_glyph(&id, g)),
                "{g} (U+{:04X}) is not in any bundled font and would render as a tofu box",
                g as u32
            );
        }
    }

    /// ⚠ The Aspect label must name both directions. "Aspect" alone was reported as confusing
    /// precisely because it says nothing about which end does what.
    #[test]
    fn the_aspect_slider_names_both_directions() {
        let ctx = test_ui::ctx();
        let mut s = session();
        let mut aspect = None;
        let mut custom = rustroom_app::settings::DEFAULT_CUSTOM_ASPECT;
        let mut ed = Edited::default();
        let found = test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
            show(
                ui,
                &mut s,
                AspectLock {
                    lock: &mut aspect,
                    custom: &mut custom,
                },
                &mut CompositionGuide::Off,
                false,
                &mut ed,
                &mut false,
            );
        });
        let label = found
            .iter()
            .map(|(l, _)| l)
            .find(|l| l.contains("Aspect"))
            .expect("no Aspect slider");
        assert!(
            label.contains('⬍') && label.contains('⬌'),
            "the Aspect label names neither direction: {label:?}"
        );
    }

    /// ⚠ That the control is drawn at all. A block added to the wrong branch compiles, runs, and
    /// is simply never on screen — which no other test in this file would notice.
    #[test]
    fn the_guides_toggle_and_clear_are_drawn() {
        let ctx = test_ui::ctx();
        let mut s = session();
        let (mut aspect, mut ed) = (None, Edited::default());
        let mut custom = rustroom_app::settings::DEFAULT_CUSTOM_ASPECT;
        let found = test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
            show(
                ui,
                &mut s,
                AspectLock {
                    lock: &mut aspect,
                    custom: &mut custom,
                },
                &mut CompositionGuide::Off,
                false,
                &mut ed,
                &mut false,
            );
        });
        for want in ["Guide mode", "Clear"] {
            assert!(
                found.iter().any(|(l, _)| l.contains(want)),
                "no “{want}” control; found {:?}",
                found.iter().map(|(l, _)| l).collect::<Vec<_>>()
            );
        }
    }

    /// ⚠⚠ Clearing must leave the **sliders** alone. The guides are the input and the sliders are
    /// the output, so throwing the scaffolding away cannot undo the correction it produced — that
    /// is the whole point of solving *into* the sliders rather than storing the guides.
    #[test]
    fn clearing_the_guides_leaves_the_correction_alone() {
        let mut s = session();
        s.show_guides = true;
        s.edits.perspective.vertical = 42.0;
        s.guides.push(pipeline::perspective::guides::Guide {
            from: [0.1, 0.1],
            to: [0.2, 0.9],
        });
        click_in_panel("Clear", &mut s, false);
        assert!(s.guides.is_empty(), "the guides should be gone");
        assert_eq!(s.edits.perspective.vertical, 42.0, "the solve must survive");
    }

    /// ⚠⚠ **The correction stays visible while guides are drawn — the live solve depends on it.**
    /// An earlier version reset perspective and straighten here so guides sat on the raw frame,
    /// which removed a feedback loop but made the sub-mode feel inert: you drew, let go, and had
    /// to toggle out to see anything. The loop is handled by *mapping the guides* through the
    /// live transform instead, so both move together.
    #[test]
    fn the_guides_preview_keeps_the_correction_and_only_drops_the_crop() {
        let mut e = edit_model::Edits::default();
        e.perspective.vertical = 40.0;
        e.geometry.angle = 3.0;
        e.geometry.crop = asymmetric();

        let p = crate::preview_edits(&e, rustroom_app::tool::Tool::Crop);
        assert_eq!(
            p.perspective.vertical, 40.0,
            "the correction must stay visible"
        );
        assert_eq!(p.geometry.angle, 3.0);
        assert_eq!(
            p.geometry.crop,
            edit_model::CropRect::default(),
            "only the crop is reset, so the whole frame shows under the overlay"
        );
    }

    /// ⚠⚠ **Editing by hand throws the guides away**, because the solve no longer describes what
    /// is on screen. Driven through the real panel rather than by poking the flag, so the
    /// before/after capture is what is under test.
    #[test]
    fn editing_the_solved_sliders_by_hand_clears_the_guides() {
        let ctx = test_ui::ctx();
        let mut s = session();
        s.guides.push(pipeline::perspective::guides::Guide {
            from: [0.1, 0.1],
            to: [0.2, 0.9],
        });
        // One frame with nothing touched: the guides must survive it.
        let (mut aspect, mut ed) = (None, Edited::default());
        let mut custom = rustroom_app::settings::DEFAULT_CUSTOM_ASPECT;
        let _ = test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
            show(
                ui,
                &mut s,
                AspectLock {
                    lock: &mut aspect,
                    custom: &mut custom,
                },
                &mut CompositionGuide::Off,
                false,
                &mut ed,
                &mut false,
            );
        });
        assert_eq!(s.guides.len(), 1, "an untouched frame must not clear them");

        // ⚠ The edit has to happen **inside** the panel body, through the real widget. Setting
        // the field beforehand does not clear — and must not: the solve itself writes that field
        // from outside, as does a paste, and neither is someone overriding the answer.
        s.edits.perspective.vertical = 60.0;
        // Clicking a slider's centre sets it to the middle of its range, i.e. 0 here.
        click_in_panel("Vertical", &mut s, false);
        assert_ne!(s.edits.perspective.vertical, 60.0, "the click must move it");
        assert!(s.guides.is_empty(), "a hand edit must clear them");
    }

    /// ⚠⚠ **This test used to assert the opposite**, and the inversion is the point. The toggle was
    /// gated on `!s.show_guides && s.guides.is_empty()`, which was reported in use as the button
    /// being mysteriously greyed out after any guide work. Guides are no longer a reason to refuse
    /// the gesture: `straighten_overlay::settle` clears them, the way a hand edit to Straighten
    /// already does. Kept rather than deleted so the removal is visible to the next reader.
    #[test]
    fn the_straighten_line_toggle_arms_even_with_guides_in_force() {
        let mut s = session();
        s.show_guides = true;
        s.guides.push(pipeline::perspective::guides::Guide {
            from: [0.2, 0.2],
            to: [0.2, 0.8],
        });
        assert!(!s.straighten_armed, "fixture check: it starts unarmed");
        let _ = click_in_panel("Straighten by drawing a line", &mut s, false);
        assert!(
            s.straighten_armed,
            "the toggle refused to arm while guides existed"
        );
    }

    /// And it is reachable with no guides, or the test above would pass against a toggle that is
    /// disabled always — including one that is missing entirely.
    #[test]
    fn the_straighten_line_toggle_arms_with_no_guides() {
        let mut s = session();
        assert!(!s.straighten_armed, "fixture check: it starts unarmed");
        let _ = click_in_panel("Straighten by drawing a line", &mut s, false);
        assert!(s.straighten_armed, "the toggle did not arm");
    }

    /// ⚠ Re-arming discards the previous line, which described a correction that is no longer the
    /// one being drawn. Without this the old line hangs on screen through the next gesture.
    #[test]
    fn re_arming_the_toggle_discards_the_previous_line() {
        let mut s = session();
        s.straighten_line = Some(([0.2, 0.4], [0.8, 0.6]));
        let _ = click_in_panel("Straighten by drawing a line", &mut s, false);
        assert_eq!(s.straighten_line, None, "the old line survived a re-arm");
    }

    /// ⚠⚠ It must not survive leaving the tool. `visualize_turns_itself_off_when_the_tool_changes`
    /// is the precedent, and its note applies here too: there is deliberately **no** "staying in
    /// Crop keeps it" half, because `select_tool` returns early when the tool is unchanged, so the
    /// line under test would never be reached and the assertion would pass against anything.
    #[test]
    fn the_straighten_line_does_not_survive_a_tool_change() {
        let ctx = test_ui::ctx();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 64, 48);
        app.select_tool(rustroom_app::tool::Tool::Crop);
        {
            let s = app.session.as_mut().unwrap();
            s.straighten_line = Some(([0.2, 0.4], [0.8, 0.6]));
            s.straighten_drag = Some(([0.2, 0.4], [0.8, 0.6]));
        }
        app.select_tool(rustroom_app::tool::Tool::Edit);
        let s = app.session.as_ref().unwrap();
        assert_eq!(s.straighten_line, None, "the line outlived its tool");
        assert_eq!(
            s.straighten_drag, None,
            "an interrupted drag outlived its tool"
        );
    }

    /// ⚠ The other half: Aspect and Scale are **not** written by a solve, so adjusting them is
    /// "you framed it differently", not "you overrode the answer". A rule that cleared on any
    /// perspective change would fail this.
    #[test]
    fn adjusting_aspect_or_scale_keeps_the_guides() {
        let ctx = test_ui::ctx();
        let mut s = session();
        s.guides.push(pipeline::perspective::guides::Guide {
            from: [0.1, 0.1],
            to: [0.2, 0.9],
        });
        let (mut aspect, mut ed) = (None, Edited::default());
        let mut custom = rustroom_app::settings::DEFAULT_CUSTOM_ASPECT;
        for change in [10.0f32, -40.0] {
            s.edits.perspective.aspect = change;
            s.edits.perspective.scale = change;
            let _ = test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
                show(
                    ui,
                    &mut s,
                    AspectLock {
                        lock: &mut aspect,
                        custom: &mut custom,
                    },
                    &mut CompositionGuide::Off,
                    false,
                    &mut ed,
                    &mut false,
                );
            });
        }
        assert_eq!(s.guides.len(), 1, "Aspect and Scale must not clear them");
    }
}
