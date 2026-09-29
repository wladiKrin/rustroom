//! The tone-curve editor.
//!
//! ⚠ egui has no curve widget, so this one is painted. Every rule about *what a gesture means*
//! lives in `rustroom_app::tone_curve_tool`, where it is testable with no window; this file
//! draws, and translates egui events into calls on it.
//!
//! ⚠⚠ The axes are **display-referred** — see `tone_curve_tool`'s module doc for why, and for
//! the cost. Nothing here converts: the tool's functions take and return display coordinates,
//! and the stored control points never pass through this file.

use crate::develop::panel::Edited;
use crate::theme;
use edit_model::ToneCurve;
use eframe::egui;
use rustroom_app::tone_curve_tool as tool;

/// Widest the editor is drawn, and it stays **fixed** while the panel grows. The widget is square —
/// a tone curve read against a non-square box misleads about its slope.
///
/// ⚠ The panel is 320 pt (2026-09-23). The curve is deliberately narrower and **centred**, the way the
/// colour wheel is; growing it to fill the panel would make that centring a no-op.
///
/// ⚠⚠ This doc said "the panel is 240 pt" until 2026-09-23 — stale since the panel went to 270 two
/// days earlier. **A size named in a comment is a size that goes stale**; the number above is a
/// cross-reference, not a fact this file can enforce.
const MAX_SIZE: f32 = 220.0;

/// How strongly the ghost histogram is painted behind the curve.
///
/// ⚠ 0.35, up from 0.15 (user's call, 2026-09-23): at 0.15 it was present but not readable. A number
/// to tune by looking, which is why it is named rather than sitting inline.
const GHOST_OPACITY: f32 = 0.35;

/// Points along the plotted curve. At 220 pt this is a sample every ~2 pt, well below the
/// point where the polyline reads as segments.
const SAMPLES: usize = 96;

/// How near, in display units, the pointer must come to grab a control point.
///
/// ⚠ The same tolerance decides whether a press *grabs* or *adds*, so it is what stops a
/// slightly-off click on an existing point silently creating a second one on top of it.
const GRAB_TOL: f32 = 0.045;

/// Quarter-tone grid divisions.
const GRID: usize = 4;

/// Draw the curve editor, reporting changes through `ed`.
///
/// Returns the editor's own rect. ⚠ Not decoration: a headless test has to aim a drag at a
/// point on the curve, and the alternative is hardcoding a screen coordinate — brittle in the
/// worst way, because a layout change then moves the target while the test keeps passing and
/// grabbing nothing. `test_ui`'s module doc records that fault happening twice.
/// Which curve the editor is showing.
///
/// ⚠⚠ **View state, never an edit.** It lives on the session beside `HslChannel` — `visualize`'s rule —
/// because switching channels must not push a history entry. A field in `Edits` would make "look at the
/// red curve" an undoable act and would travel in a preset.
#[derive(Default, PartialEq, Eq, Clone, Copy, Debug)]
pub enum CurveChannel {
    /// Lightroom's parametric curve (M5.1c): four region sliders over three splits. Its own view,
    /// `parametric_curve::show`, since it edits a different field than the four point curves.
    Parametric,
    #[default]
    Rgb,
    Red,
    Green,
    Blue,
}

impl CurveChannel {
    pub const ALL: [CurveChannel; 5] = [
        Self::Parametric,
        Self::Rgb,
        Self::Red,
        Self::Green,
        Self::Blue,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Parametric => "Parametric",
            Self::Rgb => "RGB",
            Self::Red => "Red",
            Self::Green => "Green",
            Self::Blue => "Blue",
        }
    }

    /// The channel's index into a per-channel histogram, or `None` for the composite.
    fn index(self) -> Option<usize> {
        match self {
            Self::Parametric | Self::Rgb => None,
            Self::Red => Some(0),
            Self::Green => Some(1),
            Self::Blue => Some(2),
        }
    }

    /// The colour the plotted line takes. ⚠ Hand-picked constants outside the accent system, as
    /// everything painted by hand is — and deliberately the *same* three as the histogram's channels,
    /// so the curve and the ghost under it read as the same channel.
    fn colour(self, v: &egui::Visuals) -> egui::Color32 {
        match self {
            Self::Parametric | Self::Rgb => v.strong_text_color(),
            Self::Red => egui::Color32::from_rgb(230, 105, 105),
            Self::Green => egui::Color32::from_rgb(110, 210, 110),
            Self::Blue => egui::Color32::from_rgb(120, 150, 240),
        }
    }
}

pub fn show(
    ui: &mut egui::Ui,
    curve: &mut ToneCurve,
    ed: &mut Edited,
    hist: Option<&[[u32; 256]; 3]>,
    // ⚠ `None` for a **mask's** curve, which has no per-channel curves by decision and therefore no
    // selector — see `build_pipeline`'s mask branch.
    channel: Option<&mut CurveChannel>,
    // ⚠ M5.1c: the global parametric curve, so the composite view can draw the combined curve.
    // `None` for a mask, which has none.
    parametric: Option<&edit_model::ParametricCurve>,
) -> egui::Rect {
    // ⚠ Through `sub_label_with`, never a hand-rolled `horizontal` around a sub-label: that
    // nesting is what widened the panel every frame on 2026-09-19 — see `theme::sub_label_with`.
    theme::sub_label_with(ui, "Tone curve", |ui| {
        // ⚠ Disabled rather than hidden on a neutral curve: a control that appears and
        // disappears is harder to learn than one that greys out.
        let r = ui
            .add_enabled(!curve.points.is_empty(), egui::Button::new("Reset"))
            .on_hover_text("Return the curve to a straight line");
        if r.clicked() {
            // ⚠ Cleared to *empty*, not to the two identity endpoints: empty is what keeps
            // `ToneCurveStage` out of the pipeline entirely.
            curve.points.clear();
            ed.discrete();
        }
    });

    // ⚠ The `hsl_bands` idiom — a selector rather than four nested headers — so one layout is learned
    // once. ⚠ `theme::segmented` never touches `ed`, which is what keeps switching channels out of the
    // history.
    let selected = match &channel {
        Some(c) => **c,
        None => CurveChannel::Rgb,
    };
    if let Some(c) = channel {
        theme::segmented(ui, c, &CurveChannel::ALL.map(|c| (c, c.label())));
    }

    let full = ui.available_width();
    let size = full.min(MAX_SIZE);
    // ⚠⚠ **Centred, the same way the colour wheel is** (`color_wheel.rs`, user's call 2026-09-02):
    // `allocate_exact_size` in a vertical layout starts at the left edge, so in a panel wider than
    // `MAX_SIZE` the curve sat left while everything around it spanned the full width.
    //
    // ⚠ The pad is allocated **beside** the widget rather than by widening its rect, so the click
    // target stays tight to the curve — a full-width response would let a drag grab a control point
    // from anywhere on the row.
    let pad = ((full - size) * 0.5).max(0.0);
    let (rect, resp) = {
        let mut allocated = None;
        ui.horizontal(|ui| {
            ui.add_space(pad);
            allocated =
                Some(ui.allocate_exact_size(egui::vec2(size, size), egui::Sense::click_and_drag()));
        });
        allocated.expect("the horizontal closure always runs")
    };
    // ⚠ How to work it is the hover, never a standing line (`docs/DESIGN.md` §8).
    let resp = resp.on_hover_text(GESTURE_HELP);

    // ⚠ Which point a drag grabbed has to survive between frames, or re-hit-testing each frame
    // would let a fast drag jump to whichever point the pointer happened to pass over. egui's
    // own temporary memory rather than a field on `RustroomApp`: it is per-widget transient
    // state with no meaning outside the gesture, and the app struct is a contended surface.
    let drag_id = resp.id.with("grabbed");
    let mut grabbed: Option<usize> = ui.data_mut(|d| d.get_temp(drag_id)).flatten();

    let mut pts = tool::editable(&curve.points);
    let mut changed = false;

    // Screen → display space. ⚠ Screen `y` runs **down** while a curve's output runs up, so it
    // is inverted here. Getting this wrong mirrors the curve and every drag reads backwards.
    let to_norm = |p: egui::Pos2| {
        [
            ((p.x - rect.left()) / rect.width()).clamp(0.0, 1.0),
            (1.0 - (p.y - rect.top()) / rect.height()).clamp(0.0, 1.0),
        ]
    };
    let at = |n: [f32; 2]| {
        egui::pos2(
            rect.left() + n[0] * rect.width(),
            rect.bottom() - n[1] * rect.height(),
        )
    };

    // ⚠⚠ **Where the press BEGAN, not where the pointer is now** — and this was a real bug the
    // headless drag test caught. egui only decides a press has become a drag once the pointer
    // has left it by a few points, so on the frame `drag_started` fires,
    // `interact_pointer_pos` is *already off the control point that was grabbed*. Hit-testing
    // there grabs nothing, and the drag silently does nothing at all. Read once and hoisted:
    // ⚠ never hand an `InputState` to something that also takes a `Response`, which re-enters
    // the context lock and panics.
    let press_origin = ui.input(|i| i.pointer.press_origin());

    // A secondary click removes the point under the pointer. `remove` refuses the endpoints.
    if resp.secondary_clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            if let Some(i) = tool::hit(&pts, to_norm(p), GRAB_TOL) {
                if tool::remove(&mut pts, i) {
                    changed = true;
                    ed.discrete();
                    grabbed = None;
                }
            }
        }
    }

    // ⚠ `_by(Primary)` throughout, never a bare `dragged()`/`clicked()`. egui reports a drag
    // for **every** button, so a bare check would let the middle-drag pan binding rewrite the
    // curve as the pointer crossed it.
    if resp.drag_started_by(egui::PointerButton::Primary) {
        if let Some(p) = press_origin.or_else(|| resp.interact_pointer_pos()) {
            let n = to_norm(p);
            // Grab what is there, or add a point and start dragging that — so one gesture both
            // places a point and positions it, which is what every curve editor does.
            grabbed = match tool::hit(&pts, n, GRAB_TOL) {
                Some(i) => Some(i),
                None => {
                    let i = tool::insert(&mut pts, n);
                    changed |= i.is_some();
                    i
                }
            };
        }
    }
    if resp.dragged_by(egui::PointerButton::Primary) {
        if let (Some(i), Some(p)) = (grabbed, resp.interact_pointer_pos()) {
            tool::drag(&mut pts, i, to_norm(p));
            changed = true;
        }
    }
    if resp.drag_stopped_by(egui::PointerButton::Primary) {
        grabbed = None;
        // ⚠ One history entry per settled drag, not per changed frame — the rule
        // `Edited::slider` follows, for the same reason.
        ed.settled = true;
    }
    // A press-release that never became a drag. ⚠ egui reports `clicked()` only for exactly
    // that, so this and the drag above are mutually exclusive by construction.
    if resp.clicked_by(egui::PointerButton::Primary) {
        if let Some(p) = resp.interact_pointer_pos() {
            let n = to_norm(p);
            if tool::hit(&pts, n, GRAB_TOL).is_none() && tool::insert(&mut pts, n).is_some() {
                changed = true;
                ed.discrete();
            }
        }
    }

    if changed {
        // ⚠ An untouched-back-to-straight curve is stored as *empty*, so the stage leaves the
        // pipeline again rather than running as an expensive identity.
        curve.points = if tool::is_identity(&pts) {
            Vec::new()
        } else {
            pts.clone()
        };
        ed.changed = true;
    }
    ui.data_mut(|d| d.insert_temp(drag_id, grabbed));

    // Whichever point the readout should describe: the one being dragged, else the one hovered.
    let described = grabbed.or_else(|| {
        resp.hover_pos()
            .and_then(|p| tool::hit(&pts, to_norm(p), GRAB_TOL))
    });

    if ui.is_rect_visible(rect) {
        let v = ui.visuals();
        let painter = ui.painter_at(rect);
        backdrop(&painter, rect, v, hist, selected);
        // ⚠ M5.1c: with a parametric curve active, the composite view also draws the COMBINED curve
        // faintly — the function actually applied — so the points are never read as the whole story.
        if let Some(p) = parametric.filter(|p| p.is_active() && selected == CurveChannel::Rgb) {
            let combined: Vec<egui::Pos2> = tool::combined_points(&pts, p, SAMPLES)
                .into_iter()
                .map(at)
                .collect();
            painter.add(egui::Shape::line(
                combined,
                egui::Stroke::new(1.0, v.strong_text_color().gamma_multiply(0.45)),
            ));
        }

        let curve_color = selected.colour(v);
        let plotted: Vec<egui::Pos2> = tool::curve_points(&pts, SAMPLES)
            .into_iter()
            .map(at)
            .collect();
        painter.add(egui::Shape::line(
            plotted,
            egui::Stroke::new(1.5, curve_color),
        ));

        for (i, p) in pts.iter().enumerate() {
            let c = at([tool::to_display(p[0]), tool::to_display(p[1])]);
            let active = described == Some(i);
            let r = if active { 5.0 } else { 3.5 };
            painter.circle_filled(c, r, curve_color);
            painter.circle_filled(
                c,
                r - 1.5,
                if active {
                    v.selection.bg_fill
                } else {
                    v.extreme_bg_color
                },
            );
        }
    }

    // ⚠ One line either way — the values, or an empty line — so the panel below it does not
    // jump as the pointer crosses a control point. The values are display percentages, matching
    // the axes.
    theme::hint(ui, &readout(&pts, described));
    rect
}

/// How the point editor is worked — its hover, and the only place the gestures are named.
const GESTURE_HELP: &str = "Click to add a point · drag one to move it · right-click removes it";

/// The line under the editor: a control point's input and output when one is in play, else
/// nothing — how to work the editor is its hover ([`GESTURE_HELP`]).
///
/// A free function so the wording is pinned by a test — the gestures are not discoverable from
/// the widget, so this line *is* the affordance.
fn readout(points: &[[f32; 2]], described: Option<usize>) -> String {
    match described.and_then(|i| points.get(i)) {
        Some(p) => format!(
            "in {:.0}%  →  out {:.0}%",
            tool::to_display(p[0]) * 100.0,
            tool::to_display(p[1]) * 100.0
        ),
        None => String::new(),
    }
}

/// The widget's backdrop — background, histogram, grid and the identity diagonal — shared by the
/// point view and the Parametric view (M5.1c), so the two cannot draw it differently.
pub(super) fn backdrop(
    painter: &egui::Painter,
    rect: egui::Rect,
    v: &egui::Visuals,
    hist: Option<&[[u32; 256]; 3]>,
    selected: CurveChannel,
) {
    let at = |n: [f32; 2]| {
        egui::pos2(
            rect.left() + n[0] * rect.width(),
            rect.bottom() - n[1] * rect.height(),
        )
    };
    painter.rect_filled(rect, 4.0, v.extreme_bg_color);
    // The ghost: the tonal distribution under the curve you are shaping, the convention
    // of every curve editor. The per-bin maximum over the channels, so a saturated
    // colour is not hidden by its dark siblings.
    if let Some(h) = hist {
        // ⚠ The **selected** channel's distribution, so the ghost describes the curve above it.
        // Both already exist and already share one scale, so nothing new is computed.
        let bins = match selected.index() {
            None => rustroom_app::histogram::envelope(h),
            Some(c) => rustroom_app::histogram::normalised(h)[c],
        };
        painter.add(egui::Shape::mesh(super::area_mesh(
            rect,
            &bins,
            v.weak_text_color().linear_multiply(GHOST_OPACITY),
        )));
    }

    let faint = v.weak_text_color().gamma_multiply(0.4);
    for i in 1..GRID {
        let t = i as f32 / GRID as f32;
        painter.line_segment([at([t, 0.0]), at([t, 1.0])], egui::Stroke::new(1.0, faint));
        painter.line_segment([at([0.0, t]), at([1.0, t])], egui::Stroke::new(1.0, faint));
    }
    // The identity, so a curve's departure from it is readable at a glance.
    painter.line_segment(
        [at([0.0, 0.0]), at([1.0, 1.0])],
        egui::Stroke::new(1.0, v.weak_text_color().gamma_multiply(0.7)),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drive one pointer gesture over the editor and report the curve it left behind.
    ///
    /// ⚠⚠ **Five passes, and the split between them is what makes this harness honest.** Lay
    /// out; move to `from`; press *while still there*; move to `to`; release. egui resolves
    /// interaction against the widgets registered on the **previous** pass, so a harness that
    /// moves and presses on the same pass lands its press away from the target — and the test
    /// then passes for the wrong reason, which is exactly how `a_left_drag_does_not_pan` went
    /// inert once already.
    ///
    /// `from`/`to` are in the editor's own normalised display space, so no screen coordinate
    /// is ever written down: the rect comes back from `show`.
    fn gesture(
        initial: Vec<[f32; 2]>,
        from: [f32; 2],
        to: [f32; 2],
        button: egui::PointerButton,
    ) -> (ToneCurve, Edited) {
        let ctx = egui::Context::default();
        let mut curve = ToneCurve { points: initial };
        let mut ed = Edited::default();
        let mut rect = egui::Rect::NOTHING;
        // Where a normalised point lands on screen, given the rect the last pass reported.
        let at = |r: egui::Rect, n: [f32; 2]| {
            egui::pos2(r.left() + n[0] * r.width(), r.bottom() - n[1] * r.height())
        };

        for pass in 0..5 {
            let (a, b) = (at(rect, from), at(rect, to));
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 600.0),
                )),
                ..Default::default()
            };
            match pass {
                1 => input.events.push(egui::Event::PointerMoved(a)),
                2 => input.events.push(egui::Event::PointerButton {
                    pos: a,
                    button,
                    pressed: true,
                    modifiers: Default::default(),
                }),
                3 => {
                    if from != to {
                        input.events.push(egui::Event::PointerMoved(b));
                    }
                }
                4 => input.events.push(egui::Event::PointerButton {
                    pos: b,
                    button,
                    pressed: false,
                    modifiers: Default::default(),
                }),
                _ => {}
            }
            // ⚠ One `Edited` across every pass: the flags only ever go true, and the question
            // a test asks is "did *any* frame of this gesture report it", which is what a
            // production caller reads too.
            let _ = ctx.run_ui(input, |ui| {
                rect = show(ui, &mut curve, &mut ed, None, None, None);
            });
        }
        (curve, ed)
    }

    /// A three-point curve whose middle point sits at display (0.5, 0.5) and whose endpoints
    /// are deliberately **not** the identity, so removing the middle leaves a stored curve
    /// rather than an empty list — a test that could not tell "removed one point" from
    /// "cleared everything" would be no test at all.
    fn three_points() -> Vec<[f32; 2]> {
        vec![
            [0.0, 0.0],
            [tool::from_display(0.5), tool::from_display(0.5)],
            [1.0, tool::from_display(0.9)],
        ]
    }

    #[test]
    fn a_click_on_empty_space_adds_a_control_point() {
        let (curve, ed) = gesture(
            Vec::new(),
            [0.5, 0.6],
            [0.5, 0.6],
            egui::PointerButton::Primary,
        );
        assert_eq!(curve.points.len(), 3, "{:?}", curve.points);
        assert!((tool::to_display(curve.points[1][0]) - 0.5).abs() < 0.02);
        assert!((tool::to_display(curve.points[1][1]) - 0.6).abs() < 0.02);
        assert!(ed.changed && ed.settled, "a click is one history entry");
    }

    #[test]
    fn dragging_a_point_upward_raises_its_output_and_downward_lowers_it() {
        // ⚠ The screen-y inversion, which is the single easiest thing to ship backwards here
        // and reads as the curve fighting the pointer. Asserting *both* directions is what
        // makes a flipped sign impossible to pass.
        let (up, ed) = gesture(
            three_points(),
            [0.5, 0.5],
            [0.5, 0.8],
            egui::PointerButton::Primary,
        );
        assert!(
            tool::to_display(up.points[1][1]) > 0.7,
            "dragging up gave output {}",
            tool::to_display(up.points[1][1])
        );
        assert!(ed.settled, "a settled drag is one history entry");

        let (down, _) = gesture(
            three_points(),
            [0.5, 0.5],
            [0.5, 0.2],
            egui::PointerButton::Primary,
        );
        assert!(
            tool::to_display(down.points[1][1]) < 0.3,
            "dragging down gave output {}",
            tool::to_display(down.points[1][1])
        );
    }

    #[test]
    fn a_right_click_removes_the_point_under_the_pointer() {
        // ⚠⚠ **Two interior points, not one, and that is the whole of the test.** With a single
        // interior point every "removes the wrong one" implementation is inert — index 1, the
        // last removable index and "whichever the hit test returned" are all the same point,
        // so the assertion could not see a removal that ignored where the click landed.
        let four = vec![
            [0.0, 0.0],
            [tool::from_display(0.3), tool::from_display(0.2)],
            [tool::from_display(0.7), tool::from_display(0.8)],
            [1.0, 1.0],
        ];
        let (curve, ed) = gesture(
            four.clone(),
            [0.3, 0.2],
            [0.3, 0.2],
            egui::PointerButton::Secondary,
        );
        assert_eq!(curve.points.len(), 3, "{:?}", curve.points);
        assert!(
            (tool::to_display(curve.points[1][0]) - 0.7).abs() < 0.02,
            "the wrong point was removed: {:?}",
            curve.points
        );
        assert!(ed.settled);
    }

    #[test]
    fn a_right_click_on_empty_space_removes_nothing() {
        // The other half: a removal that fires wherever you right-click would delete a point
        // you were nowhere near, and only this can see it.
        let (curve, ed) = gesture(
            three_points(),
            [0.85, 0.15],
            [0.85, 0.15],
            egui::PointerButton::Secondary,
        );
        assert_eq!(curve.points, three_points());
        assert!(!ed.changed && !ed.settled);
    }

    #[test]
    fn a_middle_drag_leaves_the_curve_alone() {
        // ⚠ egui reports `dragged()` for **every** button, so without the `_by(Primary)`
        // gating the middle-drag pan binding would rewrite the curve whenever the pointer
        // crossed the editor. The trap the canvas-navigation work recorded.
        let (curve, ed) = gesture(
            three_points(),
            [0.5, 0.5],
            [0.5, 0.9],
            egui::PointerButton::Middle,
        );
        assert_eq!(curve.points, three_points());
        assert!(!ed.changed && !ed.settled);
    }

    #[test]
    fn a_click_outside_the_endpoints_adds_nothing() {
        // There is no segment to insert into beyond the last point, and the curve is flat
        // before the first — a control point there would do nothing and could not be dragged
        // anywhere useful.
        let (curve, ed) = gesture(
            Vec::new(),
            [0.002, 0.5],
            [0.002, 0.5],
            egui::PointerButton::Primary,
        );
        assert!(curve.points.is_empty(), "{:?}", curve.points);
        assert!(!ed.changed);
    }

    #[test]
    fn an_untouched_editor_reports_nothing() {
        // ⚠ The guard on every other test here: "the curve changed after a gesture" means
        // nothing unless it *stays put* when there is no gesture. The panel draws this widget
        // sixty times a second on a photo nobody is editing.
        let (curve, ed) = gesture(
            three_points(),
            [0.5, 0.5],
            [0.5, 0.5],
            egui::PointerButton::Extra1,
        );
        assert_eq!(curve.points, three_points());
        assert!(!ed.changed && !ed.settled);
    }

    #[test]
    fn the_reset_button_clears_the_curve_and_is_disabled_on_a_neutral_one() {
        // Found by label rather than by coordinate, so a layout change cannot leave this
        // clicking empty space and still passing.
        let ctx = crate::test_ui::ctx();
        let mut curve = ToneCurve {
            points: three_points(),
        };
        let mut ed = Edited::default();
        crate::test_ui::click(&ctx, "Reset", |ui| {
            show(ui, &mut curve, &mut ed, None, None, None);
        });
        assert!(curve.points.is_empty(), "{:?}", curve.points);
        assert!(ed.settled);

        // ⚠ And the other half: a disabled button must not fire. Without this the test could
        // not tell a working Reset from one that clears on every frame.
        let mut ed2 = Edited::default();
        crate::test_ui::click(&ctx, "Reset", |ui| {
            show(ui, &mut curve, &mut ed2, None, None, None);
        });
        assert!(!ed2.changed && !ed2.settled, "a neutral curve was 'reset'");
    }

    /// ⚠⚠ **Switching channels must push NO history entry.** It is view state — `visualize`'s rule —
    /// and a selector that reported an edit would put "I looked at the red curve" in the undo stack and
    /// in every preset.
    #[test]
    fn switching_channels_is_not_an_edit() {
        let ctx = crate::test_ui::ctx();
        let mut curve = ToneCurve {
            points: three_points(),
        };
        let mut channel = CurveChannel::Rgb;
        let mut ed = Edited::default();
        crate::test_ui::click(&ctx, "Blue", |ui| {
            ed = Edited::default();
            show(ui, &mut curve, &mut ed, None, Some(&mut channel), None);
        });
        assert_eq!(
            channel,
            CurveChannel::Blue,
            "the click must select the channel"
        );
        assert!(
            !ed.changed && !ed.settled,
            "selecting a channel reported an edit"
        );
        assert_eq!(
            curve.points,
            three_points(),
            "and it must not touch the curve"
        );
    }

    /// ⚠ All four are offered, and each label is distinct — a selector with two "Red"s would be
    /// unusable and the harness would click whichever came first.
    #[test]
    fn every_channel_is_offered_once_under_a_distinct_label() {
        let mut seen: Vec<&str> = CurveChannel::ALL.iter().map(|c| c.label()).collect();
        // ⚠ Five since M5.1c (Parametric). Written out rather than read from `ALL.len()`, so adding a
        // mode is a deliberate edit here and not a silent pass.
        assert_eq!(seen.len(), 5);
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), 5, "two channels share a label");
        assert_eq!(
            CurveChannel::default(),
            CurveChannel::Rgb,
            "RGB is the default"
        );
    }

    /// ⚠⚠ **A mask's curve gets no selector**, because masks have no per-channel curves — the other
    /// half of `build_pipeline`'s mask branch. Passing `None` must draw nothing clickable, or the
    /// selector would offer channels that go nowhere.
    #[test]
    fn a_mask_curve_is_offered_no_channel_selector() {
        let ctx = crate::test_ui::ctx();
        let mut curve = ToneCurve {
            points: three_points(),
        };
        let labels = crate::test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
            show(ui, &mut curve, &mut Edited::default(), None, None, None);
        });
        for c in CurveChannel::ALL {
            assert!(
                !labels.iter().any(|(l, _)| l == c.label()),
                "a mask's curve offered the {} channel: {labels:?}",
                c.label()
            );
        }
        // ⚠ And the sample must be non-empty, or this passes against a widget that drew nothing.
        assert!(
            labels.iter().any(|(l, _)| l == "Reset"),
            "the widget did not draw, so this would guard nothing: {labels:?}"
        );
    }

    #[test]
    fn the_readout_describes_a_point_in_display_percentages() {
        // ⚠ Display, not the stored linear value: the axes are warped, so a point drawn at the
        // middle of the widget must read as 50% rather than as 21%.
        let pts = vec![[0.0, 0.0], [tool::from_display(0.25), 1.0], [1.0, 1.0]];
        assert_eq!(readout(&pts, Some(1)), "in 25%  →  out 100%");
    }

    #[test]
    fn the_gestures_are_named_on_hover_and_the_idle_readout_is_empty() {
        // The gestures are invisible otherwise, so the hover must name them; since 2026-09-29 it is
        // the hover and not a standing line of 9-pt text (`docs/DESIGN.md` §8).
        assert!(GESTURE_HELP.contains("add"), "{GESTURE_HELP}");
        assert!(GESTURE_HELP.contains("remove"), "{GESTURE_HELP}");
        let pts = tool::IDENTITY.to_vec();
        assert_eq!(readout(&pts, None), "");
        // An index that no longer exists must not panic — a point can be removed on the same
        // frame the pointer is still over it.
        assert_eq!(readout(&pts, Some(9)), "");
    }
}
