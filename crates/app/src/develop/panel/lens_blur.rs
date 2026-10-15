//! The Lens Blur section's body (M10.16d): on, the amount, what is in focus, the focal range, the
//! bokeh and its boost — and the line that says while a map computes, or that no subject was found.
//!
//! ⚠ The bin half only: what is asked for, the focus and the waiting line are
//! `rustroom_app::lens_blur_tool`'s; the click that picks a focus is the canvas's.

use crate::develop::panel::Edited;
use crate::theme;
use crate::theme::Hover as _;
use edit_model::{Bokeh, LensBlur, LensFocus};
use eframe::egui;
use rustroom_app::slider_help::Scope;

/// What the section needs besides the settings. `None` for a toggle draws none — a test's "no
/// session".
#[derive(Default)]
pub struct LensBlurUi<'a> {
    /// The Pick focus crosshair — armed, a click on the photo sets a Point's focus.
    pub picking: Option<&'a mut bool>,
    /// The depth view with the sharp zone tinted — view state, never `Edits`.
    pub visualize: Option<&'a mut bool>,
    /// `lens_blur_tool::status` — a map computing, or no subject found.
    pub status: Option<&'static str>,
}

/// Focus as the segmented control holds it — Point's coordinates are the click's, not the control's.
#[derive(Clone, Copy, PartialEq)]
enum Focus {
    Subject,
    Point,
}

pub(super) fn body(ui: &mut egui::Ui, lb: &mut LensBlur, view: LensBlurUi<'_>, ed: &mut Edited) {
    let LensBlurUi {
        picking,
        visualize,
        status,
    } = view;
    let mut on = lb.on;
    if ui
        .checkbox(&mut on, "Lens blur")
        .hover("Blurs the photo by its estimated depth, as a wide aperture would")
        .changed()
    {
        lb.on = on;
        ed.discrete();
    }
    if let Some(text) = status {
        theme::hint(ui, text);
    }
    let d = LensBlur::default();
    ed.help_scope = Scope::LensBlur;
    ed.slider(ui, &mut lb.amount, 0.0..=100.0, "Amount", d.amount);

    theme::sub_label(ui, "Focus");
    let was = match lb.focus {
        LensFocus::Subject => Focus::Subject,
        LensFocus::Point { .. } => Focus::Point,
    };
    let mut focus = was;
    crate::segmented::segmented(
        ui,
        &mut focus,
        &[
            (
                Focus::Subject,
                "Subject",
                "Keeps the detected subject sharp",
            ),
            (
                Focus::Point,
                "Point",
                "Keeps sharp the depth of a point you pick on the photo",
            ),
        ],
    );
    if focus != was {
        // ⚠ Point starts at the centre; Pick focus moves it — choosing Point arms nothing.
        lb.focus = match focus {
            Focus::Subject => LensFocus::Subject,
            Focus::Point => LensFocus::Point { point: [0.5, 0.5] },
        };
        ed.discrete();
    }
    if let Some(p) = picking {
        if matches!(lb.focus, LensFocus::Point { .. }) {
            // ⚠ The white-balance eyedropper's shape: a toggle that arms one canvas click.
            if ui
                .selectable_label(*p, "Pick focus")
                .hover("Sets the focus to the depth of a point you click on the photo")
                .clicked()
            {
                *p = !*p;
            }
        } else {
            // A Subject focus has nothing to pick.
            *p = false;
        }
    }
    ed.slider(
        ui,
        &mut lb.focal_range,
        0.0..=100.0,
        "Focal range",
        d.focal_range,
    );

    theme::sub_label(ui, "Bokeh");
    let mut bokeh = lb.bokeh;
    crate::segmented::segmented(
        ui,
        &mut bokeh,
        &[
            (
                Bokeh::Circle,
                "Circle",
                "Makes round discs, as a lens wide open",
            ),
            (
                Bokeh::Blades,
                "Blades",
                "Makes six-sided discs, as a stopped-down aperture",
            ),
            (
                Bokeh::Ring,
                "Ring",
                "Makes rings bright at the rim, as a mirror lens",
            ),
            (
                Bokeh::CatEye,
                "Cat-eye",
                "Narrows the discs toward the frame's edges, as a fast lens",
            ),
        ],
    );
    if bokeh != lb.bokeh {
        lb.bokeh = bokeh;
        ed.discrete();
    }
    ed.slider(ui, &mut lb.boost, 0.0..=100.0, "Boost", d.boost);
    ed.help_scope = Scope::Global;

    if let Some(v) = visualize {
        ui.checkbox(v, "Visualize depth")
            .hover("Shows the depth map with the sharp zone tinted");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_ui;

    #[derive(Default)]
    struct State {
        lb: LensBlur,
        picking: bool,
        visualize: bool,
        status: Option<&'static str>,
        /// Frames whose change settled — one undo step each.
        settles: usize,
        missing_help: Vec<String>,
    }

    fn draw(st: &mut State) -> impl FnMut(&mut egui::Ui) + '_ {
        move |ui| {
            let mut ed = Edited::default();
            body(
                ui,
                &mut st.lb,
                LensBlurUi {
                    picking: Some(&mut st.picking),
                    visualize: Some(&mut st.visualize),
                    status: st.status,
                },
                &mut ed,
            );
            st.settles += ed.settled as usize;
            st.missing_help.extend(ed.missing_help);
        }
    }

    fn labels(st: &mut State) -> Vec<String> {
        let ctx = test_ui::ctx();
        test_ui::nodes(&ctx, draw(st))
            .into_iter()
            .map(|n| n.label)
            .collect()
    }

    /// Every control, each with its hover sentence; Pick focus only in Point mode.
    #[test]
    fn the_section_holds_its_controls() {
        let mut st = State::default();
        let shown = labels(&mut st);
        for want in [
            "Lens blur",
            "Amount",
            "Subject",
            "Point",
            "Focal range",
            "Circle",
            "Blades",
            "Ring",
            "Cat-eye",
            "Boost",
            "Visualize depth",
        ] {
            assert!(shown.iter().any(|l| l == want), "{want}: {shown:?}");
        }
        assert!(!shown.iter().any(|l| l == "Pick focus"), "Subject mode");
        assert!(st.missing_help.is_empty(), "{:?}", st.missing_help);
        st.lb.focus = LensFocus::Point { point: [0.5, 0.5] };
        assert!(labels(&mut st).iter().any(|l| l == "Pick focus"));
    }

    #[test]
    fn switching_on_is_one_edit() {
        let ctx = test_ui::ctx();
        let mut st = State::default();
        test_ui::click(&ctx, "Lens blur", draw(&mut st));
        assert!(st.lb.on);
        assert_eq!(st.settles, 1, "one undo step");
    }

    #[test]
    fn the_sliders_are_wired() {
        let ctx = test_ui::ctx();
        let mut st = State::default();
        let sliders = test_ui::slider_centres(&ctx, draw(&mut st));
        assert_eq!(sliders.len(), 3, "{sliders:?}");
        for (label, at) in sliders {
            let before = st.lb;
            test_ui::click_at(&ctx, at + egui::vec2(40.0, 0.0), false, draw(&mut st));
            assert_ne!(st.lb, before, "{label} moved nothing");
        }
        let d = LensBlur::default();
        assert!(st.lb.amount != d.amount && st.lb.focal_range != d.focal_range);
        assert_ne!(st.lb.boost, d.boost);
    }

    /// Choosing Point focuses on the centre and arms nothing; Pick focus arms the crosshair; going back
    /// to Subject disarms it.
    #[test]
    fn choosing_point_arms_nothing_until_pick_focus() {
        let ctx = test_ui::ctx();
        let mut st = State::default();
        test_ui::click(&ctx, "Point", draw(&mut st));
        assert_eq!(st.lb.focus, LensFocus::Point { point: [0.5, 0.5] });
        assert!(!st.picking);
        assert_eq!(st.settles, 1);
        test_ui::click(&ctx, "Pick focus", draw(&mut st));
        assert!(st.picking);
        test_ui::click(&ctx, "Subject", draw(&mut st));
        assert_eq!(st.lb.focus, LensFocus::Subject);
        assert!(!st.picking, "a Subject focus has nothing to pick");
    }

    #[test]
    fn a_bokeh_and_visualize_are_wired() {
        let ctx = test_ui::ctx();
        let mut st = State::default();
        test_ui::click(&ctx, "Cat-eye", draw(&mut st));
        assert_eq!(st.lb.bokeh, Bokeh::CatEye);
        assert_eq!(st.settles, 1);
        test_ui::click(&ctx, "Visualize depth", draw(&mut st));
        assert!(st.visualize);
        assert_eq!(st.settles, 1, "a view, not an edit");
    }

    #[test]
    fn the_status_line_shows_when_there_is_one() {
        let mut st = State {
            status: Some("Estimating depth…"),
            ..State::default()
        };
        assert!(labels(&mut st).iter().any(|l| l == "Estimating depth…"));
        st.status = None;
        assert!(!labels(&mut st).iter().any(|l| l == "Estimating depth…"));
    }
}
