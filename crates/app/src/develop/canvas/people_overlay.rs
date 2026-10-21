//! Find people on the canvas (M10.16c3): every person found shaded with their outline — the ones to remove in the
//! accent, the others faintly — and a click on one picking or unpicking them.
//!
//! ⚠ While people are shown it stands in for Remove's overlay (`tools.rs`): no stroke, no click-removal, so a click
//! on the photo means one thing. The people live in DECODE space, mapped as the removals are.

use crate::Session;
use eframe::egui;
use std::sync::Arc;

/// The people's outlines as textures — each uploaded on its first frame, dropped with its person.
#[derive(Default)]
pub(crate) struct PeopleView {
    textures: Vec<(usize, egui::TextureHandle)>,
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

/// The people shaded, and a click picking one. It edits nothing — Remove N people is the panel's step.
pub(super) fn draw(ui: &egui::Ui, s: &mut Session, img_rect: egui::Rect, frame_rect: egui::Rect) {
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
        }];
        s.people.ticked = vec![false];
        app
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
        assert!(app.session.as_ref().unwrap().edits.removals.is_empty());
    }

    /// Without people shown, Remove paints as before — the guard above is not just a dead canvas.
    #[test]
    fn without_people_a_drag_still_paints() {
        let ctx = egui::Context::default();
        let mut app = app_with_a_person(&ctx);
        app.session.as_mut().unwrap().people.people.clear();
        app.session.as_mut().unwrap().people.ticked.clear();
        let c = canvas_centre(&mut app, &ctx);
        drag_through(&mut app, &ctx, c, c + egui::vec2(60.0, 10.0));
        assert_eq!(app.session.as_ref().unwrap().edits.removals.len(), 1);
    }
}
