//! Find people on the canvas (M10.16c3): every person found shaded with their outline — the ones to remove in the
//! accent, the others faintly — and a click on one picking or unpicking them.
//!
//! ⚠ While the people view is on it stands in for Remove's overlay (`tools.rs`): a click picks a person, and a
//! drawing means "look again there" (the author's (a)) — never a removal. The people live in DECODE space, mapped
//! as the removals are.

use super::navigation::{tool_drag, tool_drag_started};
use crate::Session;
use eframe::egui;
use rustroom_app::remove_tool::RemoveTool;
use std::sync::Arc;

/// The people's outlines as textures — each uploaded on its first frame, dropped with its person — and the drawing
/// across frames.
#[derive(Default)]
pub(crate) struct PeopleView {
    textures: Vec<(usize, egui::TextureHandle)>,
    /// The drawing being made, decode-normalised.
    stroke: Vec<[f32; 2]>,
    /// Where the press began, on screen — what tells a drawing from a click.
    press: Option<egui::Pos2>,
    /// The last drawing looked under — shown while the look runs.
    looked: Vec<[f32; 2]>,
}

impl PeopleView {
    fn texture(
        &mut self,
        ctx: &egui::Context,
        area: &Arc<rustroom_app::remove_tool::Area>,
    ) -> egui::TextureId {
        let key = Arc::as_ptr(area) as usize;
        if let Some((_, t)) = self.textures.iter().find(|(k, _)| *k == key) {
            return t.id();
        }
        let pixels = area
            .alpha
            .iter()
            .map(|&a| egui::Color32::from_white_alpha((a.clamp(0.0, 1.0) * 255.0).round() as u8))
            .collect();
        let t = ctx.load_texture(
            "person_area",
            egui::ColorImage::new([area.side, area.side], pixels),
            egui::TextureOptions::LINEAR,
        );
        let id = t.id();
        self.textures.push((key, t));
        id
    }
}

/// The people shaded and bordered, a click picking one, a drawing asking to look there. It edits nothing — Remove N
/// people is the panel's step.
pub(super) fn draw(
    ui: &egui::Ui,
    s: &mut Session,
    resp: &egui::Response,
    img_rect: egui::Rect,
    frame_rect: egui::Rect,
    tool: &RemoveTool,
) {
    if frame_rect.width() <= f32::EPSILON || frame_rect.height() <= f32::EPSILON {
        return;
    }
    let dm = super::remove_overlay::decode_map(s, frame_rect);
    let to_decode =
        |p: egui::Pos2| dm.frame_to_decode(p.x - frame_rect.left(), p.y - frame_rect.top());
    let to_screen = |d: [f32; 2]| {
        let (x, y) = dm.decode_to_frame(d);
        egui::pos2(frame_rect.left() + x, frame_rect.top() + y)
    };
    let click = ui.interact(img_rect, ui.id().with("people_click"), egui::Sense::click());
    let hovered = click
        .hover_pos()
        .and_then(|p| rustroom_app::people_tool::hit(&s.people.people, to_decode(p)));
    if hovered.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    // The author's (a): a drawing over someone it missed — the Remove brush's size — asks to look again there.
    let (dw, dh) = (s.buffer.width(), s.buffer.height());
    if tool_drag_started(resp) {
        s.people_view.stroke.clear();
        s.people_view.press = resp.interact_pointer_pos();
    }
    if tool_drag(resp) {
        if let Some(p) = resp.interact_pointer_pos() {
            let at = to_decode(p);
            rustroom_app::remove_tool::add_point(
                &mut s.people_view.stroke,
                at,
                tool.radius,
                dw,
                dh,
            );
        }
    }
    if resp.drag_stopped() {
        let points = std::mem::take(&mut s.people_view.stroke);
        let moved = match (s.people_view.press.take(), resp.interact_pointer_pos()) {
            (Some(a), Some(b)) => (b - a).length() > super::remove_overlay::CLICK_SLOP,
            (Some(_), None) => points.len() > 1,
            _ => false,
        };
        // ⚠ One look at a time: a drawing while one runs asks for nothing.
        if moved && !points.is_empty() && s.people.search.is_none() {
            s.people_view.looked = points.clone();
            s.people.look = Some(points);
        }
    }
    if click.clicked() {
        if let Some(p) = click.interact_pointer_pos() {
            if let Some(i) = rustroom_app::people_tool::hit(&s.people.people, to_decode(p)) {
                if let Some(t) = s.people.ticked.get_mut(i) {
                    *t = !*t;
                }
            }
        }
    }
    let live: Vec<usize> = s
        .people
        .people
        .iter()
        .map(|p| Arc::as_ptr(&p.area) as usize)
        .collect();
    s.people_view.textures.retain(|(k, _)| live.contains(k));
    let painter = ui.painter_at(crate::develop::canvas_geom::overlay_clip(img_rect));
    // ⚠ The accent for the ones to remove — set state, `docs/DESIGN.md`'s one use of it here.
    let accent = ui.visuals().selection.bg_fill.gamma_multiply(0.45);
    let faint = egui::Color32::from_white_alpha(45);
    // A decode radius on screen, measured at `at`.
    let screen_radius = |at: [f32; 2], radius: f32| {
        let step = radius * dw.max(dh) as f32 / dw.max(1) as f32;
        (to_screen([at[0] + step, at[1]]) - to_screen(at)).length()
    };
    // The drawing being made, and the last one while its look runs.
    let drawing = if !s.people_view.stroke.is_empty() {
        Some(&s.people_view.stroke)
    } else if s.people.there && s.people.search.is_some() {
        Some(&s.people_view.looked)
    } else {
        None
    };
    if let Some(stroke) = drawing.filter(|d| !d.is_empty()) {
        let pts: Vec<egui::Pos2> = stroke.iter().map(|&p| to_screen(p)).collect();
        let rad = screen_radius(stroke[0], tool.radius);
        super::remove_overlay::band(
            &painter,
            &pts,
            rad,
            Some(egui::Color32::from_white_alpha(70)),
            true,
        );
    }
    for i in 0..s.people.people.len() {
        let area = Arc::clone(&s.people.people[i].area);
        let tex = s.people_view.texture(ui.ctx(), &area);
        let tint = if s.people.ticked.get(i).copied().unwrap_or(false) {
            accent
        } else {
            faint
        };
        painter.add(super::remove_overlay::area_mesh(
            tex, area.rect, &to_screen, tint,
        ));
        // The author (2026-10-21): a light white border, so one knows a person can be clicked — following the tinted
        // outline ("a tighter border"), brighter under the pointer, over a faint dark halo so it shows on a bright
        // sky. A person without an outline keeps their box.
        let p = &s.people.people[i];
        let b = p.bbox;
        let boxed = [vec![[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]]]];
        let loops: &[Vec<[f32; 2]>] = if p.outline.is_empty() {
            &boxed
        } else {
            &p.outline
        };
        let (halo, mark) = if hovered == Some(i) {
            (120, 255)
        } else {
            (60, 170)
        };
        for l in loops {
            let pts: Vec<egui::Pos2> = l.iter().map(|&d| to_screen(d)).collect();
            for (w, col) in [
                (3.0, egui::Color32::from_black_alpha(halo)),
                (1.0, egui::Color32::from_white_alpha(mark)),
            ] {
                painter.add(egui::Shape::closed_line(
                    pts.clone(),
                    egui::Stroke::new(w, col),
                ));
            }
        }
    }
    // The brush under the pointer, away from a person — what a drawing would cover.
    if hovered.is_none() {
        if let Some(p) = click.hover_pos().or_else(|| resp.hover_pos()) {
            let rad = screen_radius(to_decode(p), tool.radius);
            for (w, col) in [
                (3.0, egui::Color32::from_black_alpha(140)),
                (1.0, egui::Color32::from_white_alpha(255)),
            ] {
                painter.circle_stroke(p, rad, egui::Stroke::new(w, col));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::develop::canvas::test_support::{
        canvas_centre, click_through, drag_through, retouch_app,
    };
    use eframe::egui;
    use rustroom_app::people_tool::Person;
    use std::sync::Arc;

    /// Retouch in Remove with one person found around the photo's middle.
    fn app_with_a_person(ctx: &egui::Context) -> crate::RustroomApp {
        let mut app = retouch_app(ctx);
        app.remove_tool = Default::default();
        let s = app.session.as_mut().unwrap();
        s.people.people = vec![Person {
            bbox: [0.3, 0.3, 0.7, 0.7],
            score: 0.9,
            tall: 0.4,
            area: Arc::new(rustroom_app::remove_tool::Area {
                rect: [0.25, 0.25, 0.75, 0.75],
                side: 4,
                alpha: vec![1.0; 16],
            }),
            // A diamond inside the box — the border follows it, not the box.
            outline: vec![vec![[0.5, 0.4], [0.6, 0.5], [0.5, 0.6], [0.4, 0.5]]],
        }];
        s.people.ticked = vec![false];
        s.people.active = true;
        app
    }

    /// The closed outlines the canvas draws.
    fn closed_paths(app: &mut crate::RustroomApp, ctx: &egui::Context) -> usize {
        fn count(s: &egui::Shape) -> usize {
            match s {
                egui::Shape::Path(p) if p.closed => 1,
                egui::Shape::Vec(v) => v.iter().map(count).sum(),
                _ => 0,
            }
        }
        let c = ctx.clone();
        let out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(crate::develop::canvas::test_support::screen()),
                ..Default::default()
            },
            |ui| {
                crate::develop::canvas::show(app, ui, &c);
            },
        );
        out.shapes.iter().map(|s| count(&s.shape)).sum()
    }

    /// The closed outlines' horizontal extents on screen.
    fn border_widths(app: &mut crate::RustroomApp, ctx: &egui::Context) -> Vec<f32> {
        fn collect(s: &egui::Shape, out: &mut Vec<f32>) {
            match s {
                egui::Shape::Path(p) if p.closed => {
                    let (lo, hi) = p
                        .points
                        .iter()
                        .fold((f32::MAX, f32::MIN), |(a, b), q| (a.min(q.x), b.max(q.x)));
                    out.push(hi - lo);
                }
                egui::Shape::Vec(v) => v.iter().for_each(|s| collect(s, out)),
                _ => {}
            }
        }
        let c = ctx.clone();
        let out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(crate::develop::canvas::test_support::screen()),
                ..Default::default()
            },
            |ui| {
                crate::develop::canvas::show(app, ui, &c);
            },
        );
        let mut w = Vec::new();
        out.shapes.iter().for_each(|s| collect(&s.shape, &mut w));
        w
    }

    /// The author (2026-10-21): "a tighter border, following the border of the tinted area" — the outline's
    /// width (a fifth of the frame), not the box's (two fifths).
    #[test]
    fn the_border_follows_the_outline_not_the_box() {
        let ctx = egui::Context::default();
        let mut app = app_with_a_person(&ctx);
        let tight = border_widths(&mut app, &ctx);
        assert!(!tight.is_empty(), "a border drawn");
        let b = app.session.as_ref().unwrap().people.people[0].bbox;
        app.session.as_mut().unwrap().people.people[0].outline =
            vec![vec![[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]]]];
        let boxed = border_widths(&mut app, &ctx);
        let ratio = tight[0] / boxed[0];
        assert!(
            (ratio - 0.5).abs() < 0.05,
            "the border is the outline's: {ratio}"
        );
    }

    /// The author (2026-10-21): each person found has a light white border, so one knows they can be clicked.
    #[test]
    fn each_person_found_has_a_border() {
        let ctx = egui::Context::default();
        let mut app = app_with_a_person(&ctx);
        let one = closed_paths(&mut app, &ctx);
        assert!(one >= 1, "a border around the one person");
        let mut second = app.session.as_ref().unwrap().people.people[0].clone();
        second.bbox = [0.05, 0.05, 0.15, 0.25];
        let s = app.session.as_mut().unwrap();
        s.people.people.push(second);
        s.people.ticked.push(true);
        assert_eq!(closed_paths(&mut app, &ctx), 2 * one, "one border each");
    }

    /// Review Focus 5: with people shown, a click inside one toggles them and paints no removal.
    #[test]
    fn a_click_on_a_person_toggles_them_and_paints_nothing() {
        let ctx = egui::Context::default();
        let mut app = app_with_a_person(&ctx);
        let c = canvas_centre(&mut app, &ctx);
        let depth = app.session.as_ref().unwrap().history.undo_depth();
        click_through(&mut app, &ctx, c);
        let s = app.session.as_ref().unwrap();
        assert_eq!(s.people.ticked, vec![true]);
        assert!(s.edits.removals.is_empty(), "no removal painted");
        assert_eq!(s.history.undo_depth(), depth, "no history step");
        click_through(&mut app, &ctx, c);
        assert_eq!(app.session.as_ref().unwrap().people.ticked, vec![false]);
    }

    #[test]
    fn a_drag_while_people_are_shown_paints_no_stroke() {
        let ctx = egui::Context::default();
        let mut app = app_with_a_person(&ctx);
        let c = canvas_centre(&mut app, &ctx);
        drag_through(&mut app, &ctx, c, c + egui::vec2(60.0, 10.0));
        let s = app.session.as_ref().unwrap();
        assert!(s.edits.removals.is_empty(), "no removal painted");
        // The author's (a): the drawing is where to look again.
        let look = s.people.look.as_ref().expect("a look there asked for");
        assert!(look.len() >= 2, "{look:?}");
    }

    /// The view on with nobody found: a drawing still looks there — that is what the empty view is for.
    #[test]
    fn a_drawing_in_an_empty_view_looks_there() {
        let ctx = egui::Context::default();
        let mut app = app_with_a_person(&ctx);
        app.session.as_mut().unwrap().people.people.clear();
        app.session.as_mut().unwrap().people.ticked.clear();
        let c = canvas_centre(&mut app, &ctx);
        drag_through(&mut app, &ctx, c, c + egui::vec2(60.0, 10.0));
        let s = app.session.as_ref().unwrap();
        assert!(s.edits.removals.is_empty());
        assert!(s.people.look.is_some());
    }

    /// One look at a time: a drawing while it looks asks for nothing.
    #[test]
    fn a_drawing_while_it_looks_asks_for_nothing() {
        let ctx = egui::Context::default();
        let mut app = app_with_a_person(&ctx);
        let (search, _answer) = rustroom_app::people_tool::Search::answered_by(0.3);
        app.session.as_mut().unwrap().people.search = Some(search);
        let c = canvas_centre(&mut app, &ctx);
        drag_through(&mut app, &ctx, c, c + egui::vec2(60.0, 10.0));
        assert!(app.session.as_ref().unwrap().people.look.is_none());
    }

    /// Without people shown, Remove paints as before — the guard above is not just a dead canvas.
    #[test]
    fn without_people_a_drag_still_paints() {
        let ctx = egui::Context::default();
        let mut app = app_with_a_person(&ctx);
        let s = app.session.as_mut().unwrap();
        s.people.people.clear();
        s.people.ticked.clear();
        s.people.active = false;
        let c = canvas_centre(&mut app, &ctx);
        drag_through(&mut app, &ctx, c, c + egui::vec2(60.0, 10.0));
        assert_eq!(app.session.as_ref().unwrap().edits.removals.len(), 1);
    }
}
