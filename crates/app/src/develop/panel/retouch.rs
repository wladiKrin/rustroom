//! The Retouch panel: the patch list first, the settings for the lit row under it, Find dust.
//!
//! ⚠ Option C of the 2026-09-20 canvas ("Rustroom Retouch Panel") plus the user's addition:
//! every patch is a row with an eye (its outline), a hold-out toggle (the patch held out of the
//! view so what it covers shows) and a `×`, mirroring the Masks list. A **Next patch** row is
//! lit whenever no patch is selected, so which patch the sliders edit is a row, not a sentence.

use super::retouch_eyes::{self, Mode};
use super::Edited;
use crate::develop::panel::{mask_components, section};
use crate::theme;
use crate::theme::Hover as _;
use crate::Session;
use edit_model::SpotMode;
use eframe::egui;
use rustroom_app::eye_tool::EyeTool;
use rustroom_app::mask_tool;
use rustroom_app::spot_tool::{self, SpotTool, MAX_RADIUS, MIN_RADIUS};

/// Draw the Retouch controls.
///
/// ⚠ `tool` is `&mut` to a field of `RustroomApp` while `s` borrows another — **disjoint** fields,
/// which is what makes both legal where a closure capturing the app twice would be `E0499`. The
/// `masks::BrushTool` precedent, which takes five of them.
pub(crate) fn show_with(
    ui: &mut egui::Ui,
    s: &mut Session,
    tool: &mut SpotTool,
    eyes: &mut EyeTool,
    ed: &mut Edited,
) {
    // ⚠ Once per frame, before anything reads them: undo, paste, snapshots and the canvas all
    // change the patch count without passing through this panel.
    s.sync_spot_flags();
    let n = s.edits.spots.len();
    let count = match n {
        0 => "no patches".to_owned(),
        1 => "1 patch".to_owned(),
        n => format!("{n} patches"),
    };
    // ⚠ `reset` is the old "Clear all": it belongs to the list as a whole, so it sits on its
    // header.
    let mut out = section::header(
        ui,
        ui.make_persistent_id(("retouch", "Patches")),
        true,
        section::Header::new("Patches")
            .modified(n > 0)
            .resettable(true)
            .trailing(&count),
    );
    if out.reset {
        s.edits.spots.clear();
        s.selected_spot = None;
        s.spot_shown.clear();
        s.spot_held.clear();
        ed.discrete();
    }
    let picked_before = s.selected_spot;
    out.show_body(ui, |ui| {
        list(ui, s, tool, ed);
        show_all_row(ui, s);
    });
    // ⚠ M9.11: picking a patch row leaves eye mode, or the next click would place an eye while a
    // patch's settings are lit.
    if s.selected_spot.is_some() && s.selected_spot != picked_before {
        eyes.active = None;
        s.selected_eye = None;
    }
    if !s.edits.eyes.is_empty() || eyes.active.is_some() {
        ui.add_space(6.0);
        retouch_eyes::list(ui, s, eyes, ed);
    }
    ui.add_space(6.0);
    settings(ui, s, tool, eyes, ed);
    find_dust(ui, s, ed);
}

/// ⚠ Test-only: the patch-mode panel, so the patch tests keep their four-argument call. The app
/// calls [`show_with`]; eye tests call it too.
#[cfg(test)]
pub(crate) fn show(ui: &mut egui::Ui, s: &mut Session, tool: &mut SpotTool, ed: &mut Edited) {
    show_with(ui, s, tool, &mut EyeTool::default(), ed);
}

/// The Next patch row and one row per patch.
fn list(ui: &mut egui::Ui, s: &mut Session, tool: &SpotTool, ed: &mut Edited) {
    let long = s.full_long;
    let selected = s.selected_spot.filter(|&i| i < s.edits.spots.len());
    // ⚠ The Next patch row shows the *tool's* defaults, because that is what the sliders edit
    // while it is lit; a patch you add takes them.
    let next_meta = meta(tool.mode, tool.radius, long);
    let out = mask_components::row(
        ui,
        mask_components::RowSpec {
            glyph: mask_components::RowGlyph::Spot(tool.mode),
            op: None,
            name: "Next patch",
            meta: &next_meta,
            accessible: "Next patch",
            selected: false,
            current: selected.is_none(),
            deletable: false,
            indent: 0.0,
            eye: None,
            hold: None,
            hover: "Edits the settings the next patch you place will use",
            draggable: false,
        },
    );
    if out.clicked {
        s.selected_spot = None;
    }
    let mut remove: Option<usize> = None;
    let mut pick: Option<usize> = None;
    for i in 0..s.edits.spots.len() {
        let sp = s.edits.spots[i];
        let name = format!("Patch {}", i + 1);
        let m = meta(sp.mode, sp.radius, long);
        let out = mask_components::row(
            ui,
            mask_components::RowSpec {
                glyph: mask_components::RowGlyph::Spot(sp.mode),
                op: None,
                name: &name,
                meta: &m,
                accessible: &name,
                selected: selected == Some(i),
                current: false,
                deletable: true,
                indent: 0.0,
                eye: Some(s.spot_shown.get(i).copied().unwrap_or(true)),
                hold: Some((
                    s.spot_held.get(i).copied().unwrap_or(false),
                    "Hold out",
                    "Hides this patch in the view only, so you can see what it covers",
                )),
                hover: "Selects this patch, so the sliders edit it",
                draggable: false,
            },
        );
        if out.delete {
            remove = Some(i);
        } else if out.eye {
            if let Some(on) = s.spot_shown.get_mut(i) {
                *on = !*on;
            }
        } else if out.hold {
            if let Some(on) = s.spot_held.get_mut(i) {
                *on = !*on;
            }
            // ⚠ A VIEW MODE, not an edit: it changes `view_edits`, so it re-renders, and pushes
            // no history entry.
            ed.changed = true;
        } else if out.clicked {
            pick = Some(i);
        }
    }
    if let Some(i) = pick {
        s.selected_spot = Some(i);
    }
    if let Some(i) = remove {
        let before = s.edits.spots.len();
        s.edits.spots.remove(i);
        if i < s.spot_shown.len() {
            s.spot_shown.remove(i);
        }
        if i < s.spot_held.len() {
            s.spot_held.remove(i);
        }
        s.selected_spot = mask_tool::selection_after_delete(s.selected_spot, i, before);
        if s.suppressed_spot == Some(i) {
            s.suppressed_spot = None;
        }
        ed.discrete();
    }
}

fn meta(mode: SpotMode, radius: f32, long: usize) -> String {
    format!(
        "{} · {:.0} px",
        mode_label(mode),
        spot_tool::radius_px(radius, long)
    )
}

fn mode_label(mode: SpotMode) -> &'static str {
    match mode {
        SpotMode::Heal => "Heal",
        SpotMode::Clone => "Clone",
    }
}

/// `Show all` / `Hide all` under the list: every outline at once.
fn show_all_row(ui: &mut egui::Ui, s: &mut Session) {
    if s.edits.spots.is_empty() {
        return;
    }
    let all = s.all_spots_shown();
    let label = if all { "Hide all" } else { "Show all" };
    let mut clicked = false;
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), ui.spacing().interact_size.y + 4.0),
        egui::Layout::right_to_left(egui::Align::Center),
        |ui| {
            clicked = ui
                .small_button(label)
                .hover(if all {
                    "Hides every patch's outline, so you can see the photograph"
                } else {
                    "Shows every patch's outline"
                })
                .clicked();
        },
    );
    if clicked {
        s.set_all_spots_shown(!all);
    }
}

const HEAL_HINT: &str =
    "Removes a spot by blending in nearby texture that matches its surroundings";
const CLONE_HINT: &str = "Removes a spot by copying another area exactly";

/// The mode, Size and Feather of the lit row: the selected patch, or the tool's defaults.
fn settings(
    ui: &mut egui::Ui,
    s: &mut Session,
    tool: &mut SpotTool,
    eyes: &mut EyeTool,
    ed: &mut Edited,
) {
    // ⚠ M9.11: four modes, one selector. The patch modes keep their old behaviour exactly — a
    // selected patch's mode is edited, else the tool's; the eye modes hand over to
    // `retouch_eyes`.
    let spot_mode = s
        .selected_spot
        .and_then(|i| s.edits.spots.get(i))
        .map_or(tool.mode, |sp| sp.mode);
    let was = Mode::current(spot_mode, eyes.active);
    let mut m = was;
    let font = egui::TextStyle::Body.resolve(ui.style());
    theme::segmented_sized(
        ui,
        &mut m,
        &[
            (Mode::Heal, "Heal", HEAL_HINT),
            (Mode::Clone, "Clone", CLONE_HINT),
            (Mode::RedEye, "Red eye", retouch_eyes::RED_EYE_HINT),
            (Mode::PetEye, "Pet eye", retouch_eyes::PET_EYE_HINT),
        ],
        28.0,
        font,
    );
    let chosen = if m != was {
        retouch_eyes::apply_mode(s, eyes, m, ed)
    } else {
        None
    };
    if eyes.active.is_some() {
        retouch_eyes::settings(ui, s, eyes, ed);
        return;
    }

    let selected = s.selected_spot.filter(|&i| i < s.edits.spots.len());
    let mut live = selected.map(|i| s.edits.spots[i]);
    let (mut touched, mut settled, mut dragging) = (false, false, false);

    if let Some(new) = chosen {
        let mode = live.as_mut().map_or(&mut tool.mode, |sp| &mut sp.mode);
        if new != *mode {
            *mode = new;
            touched = true;
            settled = true;
        }
    }

    let radius = live.as_mut().map_or(&mut tool.radius, |sp| &mut sp.radius);
    let long = s.full_long;
    let mut px = spot_tool::radius_px(*radius, long);
    let r = ui.add(
        super::slider::Slider::new(
            &mut px,
            spot_tool::radius_px(MIN_RADIUS, long)..=spot_tool::radius_px(MAX_RADIUS, long),
        )
        .logarithmic(true)
        .text("Size")
        .help(rustroom_app::slider_help::text(
            rustroom_app::slider_help::Scope::Patch,
            "Size",
        ))
        .custom_formatter(|v| format!("{v:.0} px")),
    );
    if r.changed() {
        *radius = spot_tool::radius_from_px(px, long);
    }
    touched |= r.changed();
    settled |= super::settles(&r);
    dragging |= r.dragged();

    let feather = live
        .as_mut()
        .map_or(&mut tool.feather, |sp| &mut sp.feather);
    let r = ui.add(
        super::slider::Slider::new(feather, 0.0..=100.0)
            .text("Feather")
            .help(rustroom_app::slider_help::text(
                rustroom_app::slider_help::Scope::Patch,
                "Feather",
            ))
            .reset_to(rustroom_app::spot_tool::SpotTool::default().feather),
    );
    touched |= r.changed();
    settled |= super::settles(&r);
    dragging |= r.dragged();

    if let (Some(i), Some(sp)) = (selected, live) {
        if touched {
            s.edits.spots[i] = sp;
            tool.radius = sp.radius;
            tool.feather = sp.feather;
            tool.mode = sp.mode;
        }
        if dragging {
            if s.suppressed_spot != Some(i) {
                s.suppressed_spot = Some(i);
                ed.changed = true;
            }
        } else if settled {
            s.suppressed_spot = None;
            ed.changed = true;
        }
        ed.settled |= settled;
    }
}

/// The dust view: a whole-image toggle, so not a row's business.
fn find_dust(ui: &mut egui::Ui, s: &mut Session, ed: &mut Edited) {
    ui.add_space(6.0);
    let mut on = s.visualize.is_some();
    if ui
        .toggle_value(&mut on, "Find dust")
        .hover("Shows a high-contrast view that makes sensor dust stand out, in the view only")
        .changed()
    {
        s.visualize = on.then_some(rustroom_app::visualize::DEFAULT_THRESHOLD);
        ed.changed = true;
    }
    if let Some(t) = s.visualize.as_mut() {
        let r = ui.add(
            super::slider::Slider::new(
                t,
                rustroom_app::visualize::MIN_THRESHOLD..=rustroom_app::visualize::MAX_THRESHOLD,
            )
            .text("Threshold")
            // ⚠ The slider's own help, not a hover on its response: that way the rail and the
            // number carry the sentence too (M10.6b).
            .help(Some(
                "Raises the view's gain, so fainter dust shows".to_string(),
            )),
        );
        ed.changed |= r.changed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn a_spot() -> edit_model::Spot {
        edit_model::Spot {
            centre: [0.5, 0.5],
            source: [0.2, 0.2],
            radius: 0.02,
            feather: 50.0,
            mode: SpotMode::Heal,
        }
    }

    /// ⚠ One click, one history entry — and it must clear the selection too, or the next
    /// keystroke would act on an index into an empty list.
    #[test]
    fn the_reset_removes_every_spot_and_marks_one_edit() {
        let ctx = crate::test_ui::ctx();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 64, 48);
        {
            let s = app.session.as_mut().expect("session");
            s.edits.spots = vec![a_spot(), a_spot()];
            s.selected_spot = Some(1);
        }
        let mut seen = Edited::default();
        crate::test_ui::click(&ctx, "reset", |ui| {
            let mut ed = Edited::default();
            let mut tool = app.spot_tool;
            let s = app.session.as_mut().expect("session");
            show(ui, s, &mut tool, &mut ed);
            if ed.changed {
                seen = ed;
            }
        });
        let s = app.session.as_ref().expect("session");
        assert!(s.edits.spots.is_empty(), "the reset left spots behind");
        assert_eq!(s.selected_spot, None, "the selection outlived the spots");
        assert!(
            seen.changed && seen.settled,
            "one click must be one history entry"
        );
    }

    /// ⚠ The discriminating other half: merely drawing the panel must change nothing, or the
    /// button would fire sixty times a second — the `apply_view_demosaic` rule.
    #[test]
    fn merely_drawing_the_panel_changes_nothing() {
        let ctx = crate::test_ui::ctx();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 64, 48);
        app.session.as_mut().unwrap().edits.spots = vec![a_spot()];
        for _ in 0..3 {
            let mut tool = app.spot_tool;
            let s = app.session.as_mut().expect("session");
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(crate::test_ui::screen()),
                    ..Default::default()
                },
                |ui| {
                    let mut ed = Edited::default();
                    show(ui, s, &mut tool, &mut ed);
                    assert!(!ed.changed, "drawing the panel reported an edit");
                },
            );
        }
        assert_eq!(
            app.session.as_ref().unwrap().edits.spots.len(),
            1,
            "drawing the panel removed a spot"
        );
    }

    /// Drive one panel frame with a synthetic drag on the Size slider.
    fn drag_size(app: &mut crate::RustroomApp, ctx: &egui::Context, to: f32) -> Edited {
        let mut out = Edited::default();
        // Find the slider, then drag its handle to the far right.
        let at = crate::test_ui::find(ctx, "Size", |ui| {
            let mut ed = Edited::default();
            let mut tool = app.spot_tool;
            let s = app.session.as_mut().expect("session");
            show(ui, s, &mut tool, &mut ed);
        })
        .expect("no Size slider was drawn");
        let target = egui::pos2(at.x + to, at.y);
        for pass in 0..4 {
            let pos = if pass <= 1 { at } else { target };
            let mut events = vec![egui::Event::PointerMoved(pos)];
            if pass == 1 || pass == 3 {
                events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: pass == 1,
                    modifiers: Default::default(),
                });
            }
            let mut tool = app.spot_tool;
            let mut ed = Edited::default();
            {
                let s = app.session.as_mut().expect("session");
                let _ = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(crate::test_ui::screen()),
                        events,
                        ..Default::default()
                    },
                    |ui| show(ui, s, &mut tool, &mut ed),
                );
            }
            app.spot_tool = tool;
            if ed.changed {
                out.changed = true;
            }
            if ed.settled {
                out.settled = true;
            }
        }
        out
    }

    /// ⚠⚠ **The reported bug: selecting a patch and moving Size did nothing**, because the
    /// sliders only ever wrote the tool's defaults — which decide the *next* placement and change
    /// nothing on screen. With a patch selected they must edit that patch.
    #[test]
    fn the_sliders_edit_the_selected_patch() {
        let ctx = crate::test_ui::ctx();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 64, 48);
        {
            let s = app.session.as_mut().expect("session");
            s.edits.spots = vec![a_spot()];
            s.selected_spot = Some(0);
        }
        let before = app.session.as_ref().unwrap().edits.spots[0].radius;
        let ed = drag_size(&mut app, &ctx, 200.0);
        let after = app.session.as_ref().unwrap().edits.spots[0].radius;
        assert_ne!(after, before, "the slider did not reach the selected patch");
        assert!(ed.changed, "editing a patch must request a render");
        // ⚠ And the defaults follow it, so the next click inherits what was just dialled in.
        assert_eq!(
            app.spot_tool.radius, after,
            "the tool default did not follow the patch"
        );
    }

    /// ⚠⚠ The discriminating other half: with **nothing** selected the same slider must move only
    /// the default and leave every existing patch alone — and must not report a photo edit, since
    /// it changes no pixels. Without this, "the slider edits the patch" would pass against a
    /// version that edited a patch whether or not one was selected.
    #[test]
    fn with_nothing_selected_the_sliders_leave_the_patches_alone() {
        let ctx = crate::test_ui::ctx();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 64, 48);
        {
            let s = app.session.as_mut().expect("session");
            s.edits.spots = vec![a_spot()];
            s.selected_spot = None;
        }
        let before = app.session.as_ref().unwrap().edits.spots[0];
        let ed = drag_size(&mut app, &ctx, 200.0);
        let after = app.session.as_ref().unwrap().edits.spots[0];
        assert_eq!(after, before, "an unselected patch was edited");
        assert_ne!(
            app.spot_tool.radius, before.radius,
            "the default did not move either, so the drag did nothing at all"
        );
        assert!(
            !ed.settled,
            "changing a default pushed a history entry for a photo that did not change"
        );
    }

    /// ⚠⚠ **A slider drag must not re-render the pixels, and must hide the patch while it runs —
    /// the same rule moving one follows.** Reported as "changing the size or feathering does the
    /// live update as before with moving it; it should follow the same guidelines". The reason is
    /// the same too: `SpotRemovalStage` is in the prefix, so every frame would re-run noise
    /// reduction and dehaze over the whole buffer.
    ///
    /// ⚠ Three properties, and each is a separate way to get this wrong: the stored spot follows
    /// the slider (so the outline moves), the patch leaves the view while dragging, and it comes
    /// back with exactly one settled edit on release.
    #[test]
    fn a_slider_drag_hides_the_patch_and_renders_only_on_release() {
        let ctx = crate::test_ui::ctx();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 64, 48);
        {
            let s = app.session.as_mut().expect("session");
            s.edits.spots = vec![a_spot()];
            s.selected_spot = Some(0);
        }
        let before = app.session.as_ref().unwrap().edits.spots[0].radius;

        let at = crate::test_ui::find(&ctx, "Size", |ui| {
            let mut ed = Edited::default();
            let mut tool = app.spot_tool;
            let s = app.session.as_mut().expect("session");
            show(ui, s, &mut tool, &mut ed);
        })
        .expect("no Size slider was drawn");

        let mut mid = None;
        let mut released = Edited::default();
        for pass in 0..4 {
            let pos = if pass <= 1 {
                at
            } else {
                egui::pos2(at.x + 200.0, at.y)
            };
            let mut events = vec![egui::Event::PointerMoved(pos)];
            if pass == 1 || pass == 3 {
                events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: pass == 1,
                    modifiers: Default::default(),
                });
            }
            let mut tool = app.spot_tool;
            let mut ed = Edited::default();
            {
                let s = app.session.as_mut().expect("session");
                let _ = ctx.run_ui(
                    egui::RawInput {
                        screen_rect: Some(crate::test_ui::screen()),
                        events,
                        ..Default::default()
                    },
                    |ui| show(ui, s, &mut tool, &mut ed),
                );
            }
            app.spot_tool = tool;
            if pass == 2 {
                mid = Some((
                    app.session.as_ref().unwrap().suppressed_spot,
                    app.session.as_ref().unwrap().edits.spots[0].radius,
                ));
            }
            if ed.settled {
                released = ed;
            }
        }

        let (suppressed, radius_mid) = mid.expect("the drag never reached its middle");
        assert_ne!(
            radius_mid, before,
            "the stored spot did not follow the slider, so the outline would not move"
        );
        assert_eq!(
            suppressed,
            Some(0),
            "the patch stayed in the view while the slider was dragged"
        );
        assert_eq!(
            app.session.as_ref().unwrap().suppressed_spot,
            None,
            "the patch never came back after the drag"
        );
        assert!(
            released.settled,
            "the drag pushed no history entry on release"
        );
    }

    /// ⚠ The toggle is a plain view flag, and turning it off must not touch a single patch —
    /// hiding the outlines is for *looking*, not for editing.
    /// The panel with one context for its whole life, so the section's open state and the
    /// flags survive between clicks.
    struct Panel {
        ctx: egui::Context,
        app: crate::RustroomApp,
    }

    impl Panel {
        fn with_spots(spots: Vec<edit_model::Spot>) -> Self {
            let ctx = crate::test_ui::ctx();
            let mut app = crate::test_app::for_test();
            crate::test_app::open_test_photo(&mut app, &ctx, 64, 48);
            app.session.as_mut().unwrap().edits.spots = spots;
            Self { ctx, app }
        }

        fn click(&mut self, label: &str) -> Edited {
            let ctx = self.ctx.clone();
            let mut seen = Edited::default();
            let mut tool = self.app.spot_tool;
            crate::test_ui::click(&ctx, label, |ui| {
                let mut ed = Edited::default();
                let s = self.app.session.as_mut().expect("session");
                show(ui, s, &mut tool, &mut ed);
                if ed.changed || ed.settled {
                    seen = ed;
                }
            });
            self.app.spot_tool = tool;
            seen
        }

        fn s(&self) -> &Session {
            self.app.session.as_ref().expect("session")
        }

        fn with_eyes(eyes: Vec<edit_model::Eye>) -> Self {
            let mut p = Self::with_spots(Vec::new());
            p.app.session.as_mut().unwrap().edits.eyes = eyes;
            p
        }

        /// As [`Self::click`], through the real entry point with the app's eye tool.
        fn click_with_eyes(&mut self, label: &str) -> Edited {
            let ctx = self.ctx.clone();
            let mut seen = Edited::default();
            let (mut tool, mut eyes) = (self.app.spot_tool, self.app.eye_tool);
            crate::test_ui::click(&ctx, label, |ui| {
                let mut ed = Edited::default();
                let s = self.app.session.as_mut().expect("session");
                show_with(ui, s, &mut tool, &mut eyes, &mut ed);
                if ed.changed || ed.settled {
                    seen = ed;
                }
            });
            self.app.spot_tool = tool;
            self.app.eye_tool = eyes;
            seen
        }
    }

    fn an_eye() -> edit_model::Eye {
        edit_model::Eye {
            centre: [0.5, 0.5],
            radius: 0.02,
            kind: edit_model::EyeKind::Red,
            pupil_size: 50.0,
            darken: 50.0,
            catchlight: None,
        }
    }

    #[test]
    fn choosing_red_eye_switches_the_tool_to_eyes_and_deselects_the_patch() {
        let mut p = Panel::with_spots(vec![a_spot()]);
        p.app.session.as_mut().unwrap().selected_spot = Some(0);
        let before = p.s().edits.clone();
        p.click_with_eyes("Red eye");
        assert_eq!(p.app.eye_tool.active, Some(edit_model::EyeKind::Red));
        assert_eq!(p.s().selected_spot, None);
        assert_eq!(p.s().edits, before, "switching mode edited the photograph");
    }

    #[test]
    fn pet_eye_on_a_selected_red_eye_changes_its_kind_as_one_step() {
        let mut p = Panel::with_eyes(vec![an_eye()]);
        p.app.eye_tool.active = Some(edit_model::EyeKind::Red);
        p.app.session.as_mut().unwrap().selected_eye = Some(0);
        let ed = p.click_with_eyes("Pet eye");
        let e = p.s().edits.eyes[0];
        assert_eq!(e.kind, edit_model::EyeKind::Pet);
        assert!(
            e.catchlight.is_some(),
            "the default catchlight did not come with it"
        );
        assert!(ed.settled, "not an undo step");
    }

    #[test]
    fn heal_from_eye_mode_returns_to_patches() {
        let mut p = Panel::with_eyes(vec![an_eye()]);
        p.app.eye_tool.active = Some(edit_model::EyeKind::Red);
        p.app.session.as_mut().unwrap().selected_eye = Some(0);
        p.click_with_eyes("Heal");
        assert_eq!(p.app.eye_tool.active, None);
        assert_eq!(p.s().selected_eye, None);
        assert_eq!(
            p.s().edits.eyes[0].kind,
            edit_model::EyeKind::Red,
            "the eye was edited"
        );
    }

    #[test]
    fn an_eye_row_selects_it_and_its_delete_removes_it_as_one_step() {
        let mut p = Panel::with_eyes(vec![an_eye(), an_eye()]);
        p.click_with_eyes("Eye 2");
        assert_eq!(p.s().selected_eye, Some(1));
        assert_eq!(p.app.eye_tool.active, Some(edit_model::EyeKind::Red));
        let ed = p.click_with_eyes("Delete Eye 1");
        assert_eq!(p.s().edits.eyes.len(), 1);
        assert_eq!(
            p.s().selected_eye,
            Some(0),
            "the selection followed the survivor"
        );
        assert!(ed.settled, "a delete is one history entry");
    }

    /// The guard on the guard: with no eyes and patch mode, there is no Eyes list to click.
    #[test]
    fn the_eyes_list_is_absent_until_there_is_an_eye_or_an_eye_mode() {
        let ctx = crate::test_ui::ctx();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 64, 48);
        let found = crate::test_ui::find(&ctx, "Eyes", |ui| {
            let (mut t, mut e, mut ed) = (app.spot_tool, app.eye_tool, Edited::default());
            show_with(ui, app.session.as_mut().unwrap(), &mut t, &mut e, &mut ed);
        });
        assert!(
            found.is_none(),
            "an empty Eyes list was drawn in patch mode"
        );
        app.eye_tool.active = Some(edit_model::EyeKind::Red);
        let found = crate::test_ui::find(&ctx, "Eyes", |ui| {
            let (mut t, mut e, mut ed) = (app.spot_tool, app.eye_tool, Edited::default());
            show_with(ui, app.session.as_mut().unwrap(), &mut t, &mut e, &mut ed);
        });
        assert!(found.is_some(), "no Eyes list in an eye mode");
    }

    #[test]
    fn the_eye_hides_a_patchs_outline_and_edits_nothing() {
        let mut p = Panel::with_spots(vec![a_spot()]);
        let before = p.s().edits.spots.clone();
        let ed = p.click("Show Patch 1");
        assert_eq!(
            p.s().spot_shown,
            vec![false],
            "the eye did not hide the outline"
        );
        assert_eq!(
            p.s().edits.spots,
            before,
            "hiding an outline edited a patch"
        );
        assert!(!ed.settled, "a view toggle pushed a history entry");
        p.click("Show Patch 1");
        assert_eq!(p.s().spot_shown, vec![true], "nor showed it again");
    }

    #[test]
    fn holding_a_patch_out_re_renders_and_edits_nothing() {
        let mut p = Panel::with_spots(vec![a_spot()]);
        let before = p.s().edits.clone();
        let ed = p.click("Hold out Patch 1");
        assert_eq!(
            p.s().spot_held,
            vec![true],
            "the toggle did not hold the patch out"
        );
        assert!(
            ed.changed,
            "holding a patch out changes the picture and asked for no render"
        );
        assert!(!ed.settled, "a view toggle pushed a history entry");
        assert_eq!(p.s().edits, before, "a view toggle edited the photograph");
        assert!(
            p.s()
                .view_edits(rustroom_app::tool::Tool::Retouch)
                .spots
                .is_empty(),
            "the held patch is still in the view"
        );
    }

    #[test]
    fn show_all_and_hide_all_flip_every_eye() {
        let mut p = Panel::with_spots(vec![a_spot(), a_spot()]);
        p.click("Show Patch 1");
        assert_eq!(p.s().spot_shown, vec![false, true]);
        p.click("Show all");
        assert_eq!(p.s().spot_shown, vec![true, true]);
        p.click("Hide all");
        assert_eq!(p.s().spot_shown, vec![false, false]);
    }

    #[test]
    fn a_row_selects_its_patch_and_next_patch_deselects() {
        let mut p = Panel::with_spots(vec![a_spot(), a_spot()]);
        assert_eq!(p.s().selected_spot, None);
        p.click("Patch 2");
        assert_eq!(
            p.s().selected_spot,
            Some(1),
            "the row did not select its patch"
        );
        p.click("Next patch");
        assert_eq!(p.s().selected_spot, None, "Next patch did not deselect");
    }

    #[test]
    fn deleting_a_patch_from_its_row_drops_its_flags_with_it() {
        let mut p = Panel::with_spots(vec![a_spot(), a_spot()]);
        p.click("Show Patch 1"); // hidden, shown
        p.click("Hold out Patch 2"); // -, held
        p.app.session.as_mut().unwrap().selected_spot = Some(1);
        let ed = p.click("Delete Patch 1");
        let s = p.s();
        assert_eq!(s.edits.spots.len(), 1);
        assert_eq!(s.spot_shown, vec![true], "the survivor keeps its own eye");
        assert_eq!(s.spot_held, vec![true], "and its own hold-out");
        assert_eq!(
            s.selected_spot,
            Some(0),
            "the selection followed the survivor"
        );
        assert!(ed.settled, "a delete is one history entry");
    }

    #[test]
    fn ticking_visualize_re_renders_without_editing_anything() {
        let ctx = crate::test_ui::ctx();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 64, 48);
        app.session.as_mut().unwrap().edits.spots = vec![a_spot()];
        let before = app.session.as_ref().unwrap().edits.clone();
        let mut ed = Edited::default();
        crate::test_ui::click(&ctx, "Find dust", |ui| {
            ed = Edited::default();
            let mut tool = app.spot_tool;
            let s = app.session.as_mut().expect("session");
            show(ui, s, &mut tool, &mut ed);
        });
        let s = app.session.as_ref().unwrap();
        assert_eq!(
            s.visualize,
            Some(rustroom_app::visualize::DEFAULT_THRESHOLD),
            "the toggle did not turn the view on"
        );
        assert!(
            ed.changed,
            "the view mode asked for no render, so the canvas would not change"
        );
        assert!(!ed.settled, "a view toggle pushed a history entry");
        assert_eq!(s.edits, before, "a view toggle edited the photograph");
    }

    /// ⚠⚠ A suppression the CANVAS owns must survive the panel being drawn. Moving a patch selects
    /// it, and a clear keyed on "the selected spot is suppressed" fired here on every frame of a
    /// canvas move — the heal flickered away for one frame and came straight back. Reported as
    /// such, after the previous round had fixed exactly this for the sliders.
    ///
    /// ⚠ Both halves: the suppression stays, AND no render is requested — a request alone would
    /// re-render the (still suppressed) view for nothing on every frame of the move.
    #[test]
    fn the_panel_leaves_a_suppression_it_did_not_start_alone() {
        let ctx = crate::test_ui::ctx();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 64, 48);
        {
            let s = app.session.as_mut().unwrap();
            s.edits.spots = vec![a_spot()];
            // What the canvas does on the first frame of a move.
            s.selected_spot = Some(0);
            s.suppressed_spot = Some(0);
        }
        for _ in 0..3 {
            let mut tool = app.spot_tool;
            let s = app.session.as_mut().expect("session");
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(crate::test_ui::screen()),
                    ..Default::default()
                },
                |ui| {
                    let mut ed = Edited::default();
                    show(ui, s, &mut tool, &mut ed);
                    assert!(
                        !ed.changed,
                        "the panel requested a render during a canvas move"
                    );
                },
            );
        }
        assert_eq!(
            app.session.as_ref().unwrap().suppressed_spot,
            Some(0),
            "the panel released a suppression the canvas owns"
        );
    }

    /// ⚠ The threshold is drawn only while the view is on: its absence is the off state.
    #[test]
    fn the_threshold_appears_only_while_visualize_is_on() {
        let ctx = crate::test_ui::ctx();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 64, 48);
        for (visualize, expect) in [(None, false), (Some(12.0), true)] {
            app.session.as_mut().unwrap().visualize = visualize;
            let found = crate::test_ui::find(&ctx, "Threshold", |ui| {
                let mut tool = app.spot_tool;
                let mut ed = Edited::default();
                let s = app.session.as_mut().expect("session");
                show(ui, s, &mut tool, &mut ed);
            })
            .is_some();
            assert_eq!(found, expect, "visualize={visualize:?}");
        }
    }

    /// ⚠⚠ It must not survive leaving the tool: a canvas stuck in the grey edge view after
    /// switching to Edit reads as a broken render, and it is the single most likely thing here to
    /// be reported as a bug.
    ///
    /// ⚠ There is deliberately no "staying in Retouch keeps it" half. A first version had one and
    /// it was vacuous: `select_tool` returns early when the tool is unchanged, so the line under
    /// test is never reached and the assertion passes against anything. The view can only be
    /// switched on from this panel, so every real tool change is a change away from Retouch.
    #[test]
    fn visualize_turns_itself_off_when_the_tool_changes() {
        let ctx = crate::test_ui::ctx();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 64, 48);
        app.select_tool(rustroom_app::tool::Tool::Retouch);
        app.session.as_mut().unwrap().visualize = Some(12.0);
        app.select_tool(rustroom_app::tool::Tool::Edit);
        assert_eq!(
            app.session.as_ref().unwrap().visualize,
            None,
            "the view mode outlived its tool"
        );
    }

    /// ⚠ The default radius is generous on purpose: A's Wave 3 finding is that the radius must
    /// **exceed** the blemish, or heal measures a ring that is still lying on it.
    #[test]
    fn the_default_tool_is_usable() {
        let t = SpotTool::default();
        assert!(t.radius >= MIN_RADIUS * 4.0, "got {}", t.radius);
        assert_eq!(t.mode, SpotMode::Heal, "Heal is the useful default");
    }
}
