//! Find on the canvas (M10.19d, grown from M10.16c3's people view): what a search found — each person shaded with
//! their outline, each spot of dust as a disc its patch would cover — the ones to remove in the accent, the others
//! faintly, each with a light border. The panel's **On the photo** says what the pointer does: in **Pick** a click picks
//! or unpicks one and a drag does nothing; in **Add more** a drawing, or a click, asks to look again there, closer
//! (the author's (a)). Before a review nothing is drawn and nothing reacts.
//!
//! ⚠ In Find this stands in for every Paint overlay (`tools.rs`): nothing here ever makes a retouch — Remove N is the
//! panel's step. The finds live in DECODE space, mapped as the removals are.

use super::navigation::{tool_drag, tool_drag_started};
use crate::Session;
use eframe::egui;
use rustroom_app::remove_tool::RemoveTool;
use rustroom_app::retouch_task::{FindKind, Gesture, RetouchState};
use rustroom_app::{dust_tool, people_tool};
use std::sync::Arc;

/// The people's outlines as textures — each uploaded on its first frame, dropped with its person — and the drawing
/// across frames.
#[derive(Default)]
pub(crate) struct FindView {
    textures: Vec<(usize, egui::TextureHandle)>,
    /// The drawing being made, decode-normalised.
    stroke: Vec<[f32; 2]>,
    /// Where the press began, on screen — what tells a drawing from a click.
    press: Option<egui::Pos2>,
    /// The last drawing looked under — shown while the look runs.
    looked: Vec<[f32; 2]>,
}

impl FindView {
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

/// Whether `kind`'s review is on.
fn reviewing(s: &Session, kind: FindKind) -> bool {
    match kind {
        FindKind::People => s.people.active,
        FindKind::Dust => s.dust.active,
    }
}

/// Whether `kind` is searching — a look there or a whole search.
fn searching(s: &Session, kind: FindKind) -> bool {
    match kind {
        FindKind::People => s.people.search.is_some(),
        FindKind::Dust => s.dust.search.is_some(),
    }
}

/// The find of `kind` at decode-normalised `at`.
fn hit(s: &Session, kind: FindKind, at: [f32; 2]) -> Option<usize> {
    match kind {
        FindKind::People => people_tool::hit(&s.people.people, at),
        FindKind::Dust => dust_tool::hit(&s.dust.finds, at, s.buffer.width(), s.buffer.height()),
    }
}

/// Ask `kind`'s flow to look again under `points` — one look at a time, so none while one runs.
fn ask(s: &mut Session, kind: FindKind, points: Vec<[f32; 2]>) {
    if points.is_empty() || searching(s, kind) {
        return;
    }
    s.find_view.looked = points.clone();
    match kind {
        FindKind::People => s.people.look = Some(points),
        FindKind::Dust => s.dust.look = Some(points),
    }
}

/// The finds of the review in `state.find`, shaded and bordered, and the pointer as `state.gesture` says. It edits
/// nothing.
pub(super) fn draw(
    ui: &egui::Ui,
    s: &mut Session,
    resp: &egui::Response,
    img_rect: egui::Rect,
    frame_rect: egui::Rect,
    tool: &RemoveTool,
    state: &RetouchState,
) {
    let kind = state.find;
    if !reviewing(s, kind)
        || frame_rect.width() <= f32::EPSILON
        || frame_rect.height() <= f32::EPSILON
    {
        s.find_view.stroke.clear();
        s.find_view.press = None;
        return;
    }
    let dm = super::remove_overlay::decode_map(s, frame_rect);
    let to_decode =
        |p: egui::Pos2| dm.frame_to_decode(p.x - frame_rect.left(), p.y - frame_rect.top());
    let to_screen = |d: [f32; 2]| {
        let (x, y) = dm.decode_to_frame(d);
        egui::pos2(frame_rect.left() + x, frame_rect.top() + y)
    };
    let adding = state.gesture == Gesture::Add;
    let click = ui.interact(img_rect, ui.id().with("find_click"), egui::Sense::click());
    // In Pick a find under the pointer is what a click would pick; in Add more nothing is.
    let hovered = if adding {
        None
    } else {
        click.hover_pos().and_then(|p| hit(s, kind, to_decode(p)))
    };
    if hovered.is_some() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let (dw, dh) = (s.buffer.width(), s.buffer.height());
    if adding {
        // The author's (a): a drawing over something it missed — the Remove brush's size — asks to look again there.
        if tool_drag_started(resp) {
            s.find_view.stroke.clear();
            s.find_view.press = resp.interact_pointer_pos();
        }
        if tool_drag(resp) {
            if let Some(p) = resp.interact_pointer_pos() {
                let at = to_decode(p);
                rustroom_app::remove_tool::add_point(
                    &mut s.find_view.stroke,
                    at,
                    tool.radius,
                    dw,
                    dh,
                );
            }
        }
        if resp.drag_stopped() {
            let points = std::mem::take(&mut s.find_view.stroke);
            let moved = match (s.find_view.press.take(), resp.interact_pointer_pos()) {
                (Some(a), Some(b)) => (b - a).length() > super::remove_overlay::CLICK_SLOP,
                (Some(_), None) => points.len() > 1,
                _ => false,
            };
            if moved {
                ask(s, kind, points);
            }
        }
        // A click is a drawing of one point.
        if click.clicked() {
            if let Some(p) = click.interact_pointer_pos() {
                ask(s, kind, vec![to_decode(p)]);
            }
        }
    } else if click.clicked() {
        if let Some(p) = click.interact_pointer_pos() {
            if let Some(i) = hit(s, kind, to_decode(p)) {
                let ticked = match kind {
                    FindKind::People => s.people.ticked.get_mut(i),
                    FindKind::Dust => s.dust.ticked.get_mut(i),
                };
                if let Some(t) = ticked {
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
    s.find_view.textures.retain(|(k, _)| live.contains(k));
    let painter = ui.painter_at(crate::develop::canvas_geom::overlay_clip(img_rect));
    // ⚠ The accent for the ones to remove — set state, `docs/DESIGN.md`'s one use of it here.
    let accent = ui.visuals().selection.bg_fill.gamma_multiply(0.45);
    let faint = egui::Color32::from_white_alpha(45);
    // A decode radius (a share of the long edge) on screen, measured at `at`.
    let screen_radius = |at: [f32; 2], radius: f32| {
        let step = radius * dw.max(dh) as f32 / dw.max(1) as f32;
        (to_screen([at[0] + step, at[1]]) - to_screen(at)).length()
    };
    // The drawing being made, and the last one while its look runs.
    let looking_there = match kind {
        FindKind::People => s.people.there,
        FindKind::Dust => s.dust.there.is_some(),
    } && searching(s, kind);
    let drawing = if !s.find_view.stroke.is_empty() {
        Some(&s.find_view.stroke)
    } else if looking_there {
        Some(&s.find_view.looked)
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
    // The author (2026-10-21): a light white border, so one knows a find can be clicked — brighter under the pointer,
    // over a faint dark halo so it shows on a bright sky.
    let border = |i: usize| {
        let (halo, mark) = if hovered == Some(i) {
            (120, 255)
        } else {
            (60, 170)
        };
        [
            (3.0, egui::Color32::from_black_alpha(halo)),
            (1.0, egui::Color32::from_white_alpha(mark)),
        ]
    };
    match kind {
        FindKind::People => {
            for i in 0..s.people.people.len() {
                let area = Arc::clone(&s.people.people[i].area);
                let tex = s.find_view.texture(ui.ctx(), &area);
                let tint = if s.people.ticked.get(i).copied().unwrap_or(false) {
                    accent
                } else {
                    faint
                };
                painter.add(super::remove_overlay::area_mesh(
                    tex, area.rect, &to_screen, tint,
                ));
                // The border follows the tinted outline ("a tighter border"); a person without one keeps their box.
                let p = &s.people.people[i];
                let b = p.bbox;
                let boxed = [vec![[b[0], b[1]], [b[2], b[1]], [b[2], b[3]], [b[0], b[3]]]];
                let loops: &[Vec<[f32; 2]>] = if p.outline.is_empty() {
                    &boxed
                } else {
                    &p.outline
                };
                for l in loops {
                    let pts: Vec<egui::Pos2> = l.iter().map(|&d| to_screen(d)).collect();
                    for (w, col) in border(i) {
                        painter.add(egui::Shape::closed_line(
                            pts.clone(),
                            egui::Stroke::new(w, col),
                        ));
                    }
                }
            }
        }
        // Each spot as the disc its patch would cover — what a click inside picks (`dust_tool::hit`).
        FindKind::Dust => {
            for (i, f) in s.dust.finds.iter().enumerate() {
                let c = to_screen(f.centre);
                let rad = screen_radius(f.centre, dust_tool::SPOT_SCALE * f.radius);
                let tint = if s.dust.ticked.get(i).copied().unwrap_or(false) {
                    accent
                } else {
                    faint
                };
                painter.circle_filled(c, rad, tint);
                for (w, col) in border(i) {
                    painter.circle_stroke(c, rad, egui::Stroke::new(w, col));
                }
            }
        }
    }
    // In Add more, the brush under the pointer — what a drawing would cover.
    if adding {
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
    use rustroom_app::dust_tool::Find;
    use rustroom_app::people_tool::Person;
    use rustroom_app::retouch_task::{FindKind, Gesture, Task};
    use std::sync::Arc;

    /// Retouch in Find on People — Pick, the default gesture — with one person found around the photo's middle.
    fn app_with_a_person(ctx: &egui::Context) -> crate::RustroomApp {
        let mut app = retouch_app(ctx);
        app.remove_tool = Default::default();
        app.retouch.task = Task::Find;
        app.retouch.find = FindKind::People;
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

    /// Retouch in Find on Dust — Pick — with one spot found at the photo's middle.
    fn app_with_dust(ctx: &egui::Context) -> crate::RustroomApp {
        let mut app = retouch_app(ctx);
        app.remove_tool = Default::default();
        app.retouch.task = Task::Find;
        app.retouch.find = FindKind::Dust;
        let s = app.session.as_mut().unwrap();
        s.dust.finds = vec![Find {
            centre: [0.5, 0.5],
            radius: 0.05,
            contrast: 0.3,
        }];
        s.dust.ticked = vec![false];
        s.dust.active = true;
        app
    }

    /// Every shape one frame of the canvas paints, flattened.
    fn shapes(app: &mut crate::RustroomApp, ctx: &egui::Context) -> Vec<egui::Shape> {
        fn flat(s: egui::Shape, out: &mut Vec<egui::Shape>) {
            match s {
                egui::Shape::Vec(v) => v.into_iter().for_each(|s| flat(s, out)),
                s => out.push(s),
            }
        }
        let c = ctx.clone();
        let frame = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(crate::develop::canvas::test_support::screen()),
                ..Default::default()
            },
            |ui| {
                crate::develop::canvas::show(app, ui, &c);
            },
        );
        let mut out = Vec::new();
        frame
            .shapes
            .into_iter()
            .for_each(|s| flat(s.shape, &mut out));
        out
    }

    /// The closed outlines' horizontal extents on screen.
    fn border_widths(app: &mut crate::RustroomApp, ctx: &egui::Context) -> Vec<f32> {
        shapes(app, ctx)
            .iter()
            .filter_map(|s| match s {
                egui::Shape::Path(p) if p.closed => {
                    let (lo, hi) = p
                        .points
                        .iter()
                        .fold((f32::MAX, f32::MIN), |(a, b), q| (a.min(q.x), b.max(q.x)));
                    Some(hi - lo)
                }
                _ => None,
            })
            .collect()
    }

    /// The shaded discs — dust found.
    fn discs(app: &mut crate::RustroomApp, ctx: &egui::Context) -> usize {
        shapes(app, ctx)
            .iter()
            .filter(|s| matches!(s, egui::Shape::Circle(c) if c.fill != egui::Color32::TRANSPARENT))
            .count()
    }

    fn look(app: &crate::RustroomApp, kind: FindKind) -> Option<Vec<[f32; 2]>> {
        let s = app.session.as_ref().unwrap();
        match kind {
            FindKind::People => s.people.look.clone(),
            FindKind::Dust => s.dust.look.clone(),
        }
    }

    fn retouches(app: &crate::RustroomApp) -> usize {
        rustroom_app::retouch_list::count(&app.session.as_ref().unwrap().edits)
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
        let one = border_widths(&mut app, &ctx).len();
        assert!(one >= 1, "a border around the one person");
        let mut second = app.session.as_ref().unwrap().people.people[0].clone();
        second.bbox = [0.05, 0.05, 0.15, 0.25];
        let s = app.session.as_mut().unwrap();
        s.people.people.push(second);
        s.people.ticked.push(true);
        assert_eq!(
            border_widths(&mut app, &ctx).len(),
            2 * one,
            "one border each"
        );
    }

    /// Each spot of dust found is shaded on the photo, as a person is.
    #[test]
    fn each_dust_find_is_shaded() {
        let ctx = egui::Context::default();
        let mut app = app_with_dust(&ctx);
        assert_eq!(discs(&mut app, &ctx), 1);
        let s = app.session.as_mut().unwrap();
        s.dust.finds.push(Find {
            centre: [0.2, 0.2],
            radius: 0.03,
            contrast: 0.3,
        });
        s.dust.ticked.push(true);
        assert_eq!(discs(&mut app, &ctx), 2);
    }

    /// Review Focus 5: in Pick, a click inside a person toggles them and paints no removal.
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
    fn a_click_in_pick_toggles_a_find() {
        let ctx = egui::Context::default();
        let mut app = app_with_dust(&ctx);
        let c = canvas_centre(&mut app, &ctx);
        click_through(&mut app, &ctx, c);
        assert_eq!(app.session.as_ref().unwrap().dust.ticked, vec![true]);
        assert_eq!(retouches(&app), 0, "a pick made a patch");
        click_through(&mut app, &ctx, c);
        assert_eq!(app.session.as_ref().unwrap().dust.ticked, vec![false]);
    }

    /// Review Focus 3: in Find a drag never paints; in Pick it looks nowhere, in Add more it looks there — people and
    /// dust alike.
    #[test]
    fn a_drag_in_find_paints_nothing_and_looks_only_in_add_more() {
        for kind in [FindKind::People, FindKind::Dust] {
            let ctx = egui::Context::default();
            let mut app = match kind {
                FindKind::People => app_with_a_person(&ctx),
                FindKind::Dust => app_with_dust(&ctx),
            };
            let c = canvas_centre(&mut app, &ctx);
            drag_through(&mut app, &ctx, c, c + egui::vec2(60.0, 10.0));
            assert_eq!(retouches(&app), 0, "{kind:?}: a drag in Pick painted");
            assert!(
                look(&app, kind).is_none(),
                "{kind:?}: a drag in Pick looked"
            );
            app.retouch.gesture = Gesture::Add;
            drag_through(&mut app, &ctx, c, c + egui::vec2(60.0, 10.0));
            assert_eq!(retouches(&app), 0, "{kind:?}: a drag in Add more painted");
            let l = look(&app, kind).unwrap_or_else(|| panic!("{kind:?}: no look there"));
            assert!(l.len() >= 2, "{kind:?}: {l:?}");
        }
    }

    /// In Add more a click is a one-point drawing — it looks there and picks nobody.
    #[test]
    fn a_click_in_add_more_looks_there() {
        let ctx = egui::Context::default();
        let mut app = app_with_a_person(&ctx);
        app.retouch.gesture = Gesture::Add;
        let c = canvas_centre(&mut app, &ctx);
        click_through(&mut app, &ctx, c);
        let s = app.session.as_ref().unwrap();
        assert_eq!(s.people.ticked, vec![false], "a click in Add more picked");
        assert_eq!(s.people.look.as_ref().map(Vec::len), Some(1));
    }

    /// The review on with nobody found: a drawing still looks there — that is what the empty review is for.
    #[test]
    fn a_drawing_in_an_empty_review_looks_there() {
        let ctx = egui::Context::default();
        let mut app = app_with_a_person(&ctx);
        app.retouch.gesture = Gesture::Add;
        app.session.as_mut().unwrap().people.people.clear();
        app.session.as_mut().unwrap().people.ticked.clear();
        let c = canvas_centre(&mut app, &ctx);
        drag_through(&mut app, &ctx, c, c + egui::vec2(60.0, 10.0));
        assert_eq!(retouches(&app), 0);
        assert!(app.session.as_ref().unwrap().people.look.is_some());
    }

    /// One look at a time: a drawing while it looks asks for nothing.
    #[test]
    fn a_drawing_while_it_looks_asks_for_nothing() {
        let ctx = egui::Context::default();
        let mut app = app_with_a_person(&ctx);
        app.retouch.gesture = Gesture::Add;
        let (search, _answer) = rustroom_app::people_tool::Search::answered_by(0.3);
        app.session.as_mut().unwrap().people.search = Some(search);
        let c = canvas_centre(&mut app, &ctx);
        drag_through(&mut app, &ctx, c, c + egui::vec2(60.0, 10.0));
        assert!(app.session.as_ref().unwrap().people.look.is_none());
    }

    /// Before a review there is nothing to pick or add to: Find draws nothing and a drag does nothing — in either
    /// gesture.
    #[test]
    fn before_a_review_find_draws_nothing_and_does_nothing() {
        let ctx = egui::Context::default();
        let mut app = app_with_a_person(&ctx);
        let s = app.session.as_mut().unwrap();
        s.people.people.clear();
        s.people.ticked.clear();
        s.people.active = false;
        assert!(
            border_widths(&mut app, &ctx).is_empty(),
            "an outline before a review"
        );
        let c = canvas_centre(&mut app, &ctx);
        for gesture in [Gesture::Pick, Gesture::Add] {
            app.retouch.gesture = gesture;
            drag_through(&mut app, &ctx, c, c + egui::vec2(60.0, 10.0));
            assert_eq!(retouches(&app), 0, "{gesture:?}: Find painted");
            assert!(
                look(&app, FindKind::People).is_none(),
                "{gesture:?}: Find looked"
            );
        }
    }

    /// In Paint the brush paints as before — even with a review left on in Find.
    #[test]
    fn in_paint_a_drag_paints_as_before() {
        let ctx = egui::Context::default();
        let mut app = app_with_a_person(&ctx);
        app.retouch.task = Task::Paint;
        let c = canvas_centre(&mut app, &ctx);
        drag_through(&mut app, &ctx, c, c + egui::vec2(60.0, 10.0));
        let s = app.session.as_ref().unwrap();
        assert_eq!(
            s.edits.removals.len(),
            1,
            "Paint's Remove brush painted nothing"
        );
        assert!(s.people.look.is_none(), "Paint looked there");
        assert_eq!(s.people.ticked, vec![false], "Paint picked");
    }
}
