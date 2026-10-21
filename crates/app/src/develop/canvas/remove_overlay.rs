//! Retouch's Remove mode on the canvas (M10.16c1): the brush under the pointer, a stroke painted over
//! what to remove, a click to pick one, and every removal drawn — the selected one lit, the ones still
//! being filled shaded until their fill lands.
//!
//! ⚠ Removals live in DECODE space (`edit_model::Removal`), so the screen maps through
//! `ai_space::DecodeMap` both ways: `frame_to_decode` for the pointer, `decode_to_frame` for drawing.

use super::navigation::{tool_drag, tool_drag_started};
use crate::Session;
use eframe::egui;
use rustroom_app::ai_space::DecodeMap;
use rustroom_app::remove_tool::{self, RemoveTool};
use rustroom_app::tool::Tool;
use std::sync::Arc;

/// A press that moved less than this many points is a click, left to the click path — the eye
/// overlay's rule, for its reason: egui runs the whole drag lifecycle for a plain click.
const CLICK_SLOP: f32 = 3.0;

/// What a screen ↔ decode map is built from: the geometry, the perspective, the frame's size, the lens
/// (`ai_space::lens_key`) and the decode's size.
type MapKey = (
    edit_model::Geometry,
    edit_model::Perspective,
    usize,
    usize,
    u64,
    usize,
    usize,
);

/// The Remove gesture across frames — on the session for `spot_drag`'s reason: `show` runs fresh every
/// frame. And which removal is selected — view state, never `Edits`.
#[derive(Default)]
pub(crate) struct RemovalUi {
    pub(crate) selected: Option<usize>,
    /// The stroke being painted, decode-normalised.
    pub(crate) stroke: Vec<[f32; 2]>,
    /// Where the press began, on screen — what tells a stroke from a click.
    pub(crate) press: Option<egui::Pos2>,
    /// The screen ↔ decode map and what it was built from. ⚠ Memoised: it builds a lens corrector, which
    /// samples its whole displacement field — not a per-frame cost while the pointer moves.
    map: Option<(MapKey, Arc<DecodeMap>)>,
    /// Each removal's outline shown — the row's eye; view state, `spot_shown`'s.
    shown: Vec<bool>,
    /// Each removal held out of the view — the row's Hold out; view state, `spot_held`'s.
    held: Vec<bool>,
    /// The areas fills are being made for (M10.16c1b), by removal — `removal_flow` sets them each frame.
    areas: Vec<(edit_model::Removal, Arc<rustroom_app::remove_tool::Area>)>,
    /// Each area's texture, by the area's address — uploaded once, dropped with its area.
    textures: Vec<(usize, egui::TextureHandle)>,
}

impl RemovalUi {
    /// The areas being filled now.
    pub(crate) fn set_areas(
        &mut self,
        areas: Vec<(edit_model::Removal, Arc<rustroom_app::remove_tool::Area>)>,
    ) {
        self.areas = areas;
        let live: Vec<usize> = self
            .areas
            .iter()
            .map(|(_, a)| Arc::as_ptr(a) as usize)
            .collect();
        self.textures.retain(|(k, _)| live.contains(k));
    }

    /// The texture `area` is drawn with — uploaded on its first frame, then the held one.
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
            "remove_area",
            egui::ColorImage::new([area.side, area.side], pixels),
            egui::TextureOptions::LINEAR,
        );
        let id = t.id();
        self.textures.push((key, t));
        id
    }

    /// The area a fill is being made for `r`, whatever `r`'s Opacity.
    pub(crate) fn area_of(
        &self,
        r: &edit_model::Removal,
    ) -> Option<&Arc<rustroom_app::remove_tool::Area>> {
        self.areas
            .iter()
            .find(|(k, _)| rustroom_app::remove_tool::same_fill(k, r))
            .map(|(_, a)| a)
    }

    /// One flag of each per removal — undo, paste and the canvas change the count without the panel.
    pub(crate) fn sync(&mut self, n: usize) {
        self.shown.resize(n, true);
        self.held.resize(n, false);
    }

    pub(crate) fn is_shown(&self, i: usize) -> bool {
        self.shown.get(i).copied().unwrap_or(true)
    }

    pub(crate) fn is_held(&self, i: usize) -> bool {
        self.held.get(i).copied().unwrap_or(false)
    }

    pub(crate) fn toggle_shown(&mut self, i: usize) {
        if let Some(on) = self.shown.get_mut(i) {
            *on = !*on;
        }
    }

    pub(crate) fn toggle_held(&mut self, i: usize) {
        if let Some(on) = self.held.get_mut(i) {
            *on = !*on;
        }
    }

    /// Removal `i` was deleted: its flags go with it, so the next ones keep theirs.
    pub(crate) fn forget(&mut self, i: usize) {
        for flags in [&mut self.shown, &mut self.held] {
            if i < flags.len() {
                flags.remove(i);
            }
        }
    }

    /// The removals the view shows: the held ones left out, so what they cover shows.
    pub(crate) fn hold_out(&self, removals: &mut Vec<edit_model::Removal>) {
        let mut i = 0;
        removals.retain(|_| {
            let keep = !self.is_held(i);
            i += 1;
            keep
        });
    }

    /// The held map's address — the memo's test.
    #[cfg(test)]
    pub(crate) fn map_ptr(&self) -> Option<*const DecodeMap> {
        self.map.as_ref().map(|(_, m)| Arc::as_ptr(m))
    }
}

/// The screen ↔ decode mapping for this view — the held one while nothing it is built from has changed.
fn decode_map(s: &mut Session, frame_rect: egui::Rect) -> Arc<DecodeMap> {
    let (dw, dh) = (s.buffer.width(), s.buffer.height());
    let (fw, fh) = (frame_rect.width() as usize, frame_rect.height() as usize);
    let lens = rustroom_app::ai_space::lens_key(s.lens.shot.as_ref(), &s.edits, dw, dh);
    let key: MapKey = (s.edits.geometry, s.edits.perspective, fw, fh, lens, dw, dh);
    if let Some((k, m)) = &s.removal.map {
        if *k == key {
            return Arc::clone(m);
        }
    }
    let corr = s.lens_memo.get(s.lens.shot.as_ref(), &s.edits, dw, dh);
    let m = Arc::new(DecodeMap::with_shared(
        &s.edits.geometry,
        s.edits.perspective,
        fw,
        fh,
        corr,
        dw,
        dh,
    ));
    s.removal.map = Some((key, Arc::clone(&m)));
    m
}

/// One removal as the panel and the canvas place it: the tool's brush and Detect, full Opacity.
fn removal(points: Vec<[f32; 2]>, tool: &RemoveTool) -> edit_model::Removal {
    edit_model::Removal {
        points,
        radius: tool.radius,
        detect: tool.detect,
        variant: 0,
        opacity: 100.0,
        person: None,
    }
}

/// The Remove gesture and drawing. Returns whether a removal was added and recorded as one undo step.
pub(super) fn draw_remove_overlay(
    ui: &egui::Ui,
    s: &mut Session,
    resp: &egui::Response,
    img_rect: egui::Rect,
    frame_rect: egui::Rect,
    tool: &RemoveTool,
) -> bool {
    if frame_rect.width() <= f32::EPSILON || frame_rect.height() <= f32::EPSILON {
        return false;
    }
    let (dw, dh) = (s.buffer.width(), s.buffer.height());
    let dm = decode_map(s, frame_rect);
    let to_decode =
        |p: egui::Pos2| dm.frame_to_decode(p.x - frame_rect.left(), p.y - frame_rect.top());
    let to_screen = |d: [f32; 2]| {
        let (x, y) = dm.decode_to_frame(d);
        egui::pos2(frame_rect.left() + x, frame_rect.top() + y)
    };
    // A decode radius on screen, measured at `at` — the lens and the zoom both scale it.
    let screen_radius = |at: [f32; 2], radius: f32| {
        let step = radius * dw.max(dh) as f32 / dw.max(1) as f32;
        (to_screen([at[0] + step, at[1]]) - to_screen(at)).length()
    };
    let mut changed = false;

    // ⚠ A click-sensing interaction over the drag-sensing canvas — the spot overlay's arrangement and
    // its reason: the canvas's own `resp` never reports `clicked()`.
    let click = ui
        .interact(img_rect, ui.id().with("remove_click"), egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::Crosshair);
    if click.clicked() {
        if let Some(p) = click.interact_pointer_pos() {
            let at = to_decode(p);
            match remove_tool::hit_test(&s.edits.removals, at, dw, dh) {
                Some(i) => s.removal.selected = Some(i),
                None if at.iter().all(|v| (0.0..=1.0).contains(v)) => {
                    s.edits.removals.push(removal(vec![at], tool));
                    s.removal.selected = Some(s.edits.removals.len() - 1);
                    changed = true;
                }
                None => {}
            }
            s.selected_spot = None;
            s.selected_eye = None;
        }
    }

    if tool_drag_started(resp) {
        s.removal.stroke.clear();
        s.removal.press = resp.interact_pointer_pos();
    }
    if tool_drag(resp) {
        if let Some(p) = resp.interact_pointer_pos() {
            let at = to_decode(p);
            remove_tool::add_point(&mut s.removal.stroke, at, tool.radius, dw, dh);
        }
    }
    if resp.drag_stopped() {
        let points = std::mem::take(&mut s.removal.stroke);
        let moved = match (s.removal.press.take(), resp.interact_pointer_pos()) {
            (Some(a), Some(b)) => (b - a).length() > CLICK_SLOP,
            (Some(_), None) => points.len() > 1,
            _ => false,
        };
        // ⚠ A press that never moved is the click path's; anything further is one stroke, one removal.
        if moved && !points.is_empty() {
            s.edits.removals.push(removal(points, tool));
            s.removal.selected = Some(s.edits.removals.len() - 1);
            s.selected_spot = None;
            s.selected_eye = None;
            changed = true;
        }
    }
    if changed {
        s.request(
            s.view_edits(Tool::Retouch),
            s.render_max_dim(),
            s.requested_region,
        );
    }
    // ⚠ The overlay records its own history step and reports the edit only if one was recorded — the
    // spot overlay's rule.
    let changed = changed && s.history.push(s.edits.clone());

    // ---- the overlay ------------------------------------------------------------------------
    let painter = ui.painter_at(crate::develop::canvas_geom::overlay_clip(img_rect));
    let filled: Vec<bool> = {
        let maps = s.engine.ai_maps();
        let maps = maps.read().expect("ai maps");
        s.edits
            .removals
            .iter()
            .map(|r| maps.fill(r).is_some())
            .collect()
    };
    let selected = s.removal.selected.filter(|&i| i < s.edits.removals.len());
    // ⚠ M10.16c1b, the author's call: a removal being filled shades the AREA it will replace once the job has
    // sent it — Detect objects' whole object, not only the stroke — through a grid that follows crop, rotation
    // and lens. Each area's texture is uploaded once.
    let mut shades: Vec<Option<(egui::TextureId, [f32; 4])>> = vec![None; s.edits.removals.len()];
    for (i, shade) in shades.iter_mut().enumerate() {
        if filled[i] {
            continue;
        }
        let Some(area) = s.removal.area_of(&s.edits.removals[i]).cloned() else {
            continue;
        };
        *shade = Some((s.removal.texture(ui.ctx(), &area), area.rect));
    }
    for (i, r) in s.edits.removals.iter().enumerate() {
        let Some(&first) = r.points.first() else {
            continue;
        };
        // The row's eye: a hidden outline is not drawn at all.
        if !s.removal.is_shown(i) {
            continue;
        }
        let rad = screen_radius(first, r.radius);
        let pts: Vec<egui::Pos2> = r.points.iter().map(|&p| to_screen(p)).collect();
        // ⚠ A removal still being filled is shaded — the object is still in the picture under it; a
        // filled one is only outlined, faintly unless selected (the spot overlay's dimming).
        if let Some((tex, rect)) = shades[i] {
            painter.add(area_mesh(tex, rect, &to_screen));
        }
        let fill = if shades[i].is_some() {
            None
        } else if !filled[i] {
            Some(egui::Color32::from_white_alpha(70))
        } else if selected == Some(i) {
            Some(egui::Color32::from_white_alpha(35))
        } else {
            None
        };
        band(&painter, &pts, rad, fill, selected == Some(i));
    }
    if !s.removal.stroke.is_empty() {
        let pts: Vec<egui::Pos2> = s.removal.stroke.iter().map(|&p| to_screen(p)).collect();
        let rad = screen_radius(s.removal.stroke[0], tool.radius);
        band(
            &painter,
            &pts,
            rad,
            Some(egui::Color32::from_white_alpha(70)),
            true,
        );
    }
    // The brush under the pointer.
    if let Some(p) = click.hover_pos().or_else(|| resp.hover_pos()) {
        let rad = screen_radius(to_decode(p), tool.radius);
        for (w, col) in [
            (3.0, egui::Color32::from_black_alpha(140)),
            (1.0, egui::Color32::from_white_alpha(255)),
        ] {
            painter.circle_stroke(p, rad, egui::Stroke::new(w, col));
        }
    }
    changed
}

/// An area's shading: its texture laid through [`rustroom_app::remove_tool::area_grid`], each vertex mapped
/// decode → screen, at the pending stroke's tint.
fn area_mesh(
    tex: egui::TextureId,
    rect: [f32; 4],
    to_screen: &dyn Fn([f32; 2]) -> egui::Pos2,
) -> egui::Mesh {
    const N: usize = 24;
    let mut mesh = egui::Mesh::with_texture(tex);
    for (d, uv) in rustroom_app::remove_tool::area_grid(rect, N) {
        mesh.vertices.push(egui::epaint::Vertex {
            pos: to_screen(d),
            uv: egui::pos2(uv[0], uv[1]),
            color: egui::Color32::from_white_alpha(70),
        });
    }
    let row = (N + 1) as u32;
    for gy in 0..N as u32 {
        for gx in 0..N as u32 {
            let a = gy * row + gx;
            mesh.add_triangle(a, a + 1, a + row);
            mesh.add_triangle(a + 1, a + row + 1, a + row);
        }
    }
    mesh
}

/// A stroke's band on screen: `fill` laid along it at its radius, and its outline — bright when lit.
fn band(
    painter: &egui::Painter,
    pts: &[egui::Pos2],
    rad: f32,
    fill: Option<egui::Color32>,
    lit: bool,
) {
    if let Some(col) = fill {
        if pts.len() == 1 {
            painter.circle_filled(pts[0], rad, col);
        } else {
            painter.add(egui::Shape::line(
                pts.to_vec(),
                egui::Stroke::new(2.0 * rad, col),
            ));
        }
    }
    let (halo, mark) = if lit { (140, 255) } else { (70, 90) };
    for (w, col) in [
        (3.0, egui::Color32::from_black_alpha(halo)),
        (1.0, egui::Color32::from_white_alpha(mark)),
    ] {
        let stroke = egui::Stroke::new(w, col);
        if let (Some(a), Some(b)) = (pts.first(), pts.last()) {
            painter.circle_stroke(*a, rad, stroke);
            if pts.len() > 1 {
                painter.circle_stroke(*b, rad, stroke);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::develop::canvas::test_support::{canvas_centre, drag_through, retouch_app};
    use eframe::egui;
    use std::sync::Arc;

    /// Retouch open in Remove, its default mode.
    fn remove_app(ctx: &egui::Context) -> crate::RustroomApp {
        let mut app = retouch_app(ctx);
        app.remove_tool = Default::default();
        assert!(app.remove_tool.active, "Remove is the default mode");
        app
    }

    /// One drag across the photo: one removal, its points along the drag, one undo step, selected.
    #[test]
    fn a_stroke_makes_one_removal_and_one_undo_step() {
        let ctx = egui::Context::default();
        let mut app = remove_app(&ctx);
        app.remove_tool.radius = 0.02;
        let c = canvas_centre(&mut app, &ctx);
        let depth = app.session.as_ref().unwrap().history.undo_depth();
        drag_through(&mut app, &ctx, c, c + egui::vec2(60.0, 10.0));
        let s = app.session.as_ref().unwrap();
        assert_eq!(s.edits.removals.len(), 1, "one stroke, one removal");
        let r = &s.edits.removals[0];
        assert!(
            r.points.len() >= 2,
            "the stroke followed the drag: {:?}",
            r.points
        );
        assert!(
            r.points[1][0] > r.points[0][0],
            "left to right: {:?}",
            r.points
        );
        assert_eq!(r.radius, 0.02);
        assert_eq!(s.history.undo_depth(), depth + 1, "one undo step");
        assert_eq!(s.removal.selected, Some(0));
        assert!(s.edits.spots.is_empty(), "no patch placed");
    }

    /// ⚠ Painting changes no pixel until the release, so the view must not drop to the drag rung while a
    /// stroke is painted — it would blur under the brush for nothing. Read on the DRAG frame: the release
    /// asks for its own render either way.
    ///
    /// ⚠⚠ The fixture is `view.rs`'s pan test's, and for its reason: the test photo's own ladder has one
    /// rung, so without `buffer_long` 3200, a zoom and a seeded region a drop is unrepresentable — the
    /// first version of this test passed with nothing exempting the stroke.
    #[test]
    fn a_stroke_keeps_the_view_sharp_while_it_is_painted() {
        let ctx = egui::Context::default();
        let mut app = remove_app(&ctx);
        app.settings.drag_quality = rustroom_app::settings::DragQuality::Faster;
        {
            let s = app.session.as_mut().unwrap();
            s.buffer_long = 3200;
            s.zoom = 4.0;
        }
        let c = canvas_centre(&mut app, &ctx);
        let _ = canvas_centre(&mut app, &ctx);
        {
            let s = app.session.as_mut().unwrap();
            s.rendered_region = Some(rustroom_app::render_engine::RegionRect {
                x: 0,
                y: 0,
                w: 160,
                h: 120,
            });
        }
        assert_eq!(
            app.session.as_ref().unwrap().render_rung,
            3200,
            "fixture check: settled on the top rung, so a drop is representable"
        );
        let counts = drag_through(&mut app, &ctx, c, c + egui::vec2(6.0, 4.0));
        assert_eq!(
            counts[2],
            counts[1],
            "the drag frame asked for {} render(s): {counts:?}",
            counts[2] - counts[1]
        );
        assert_eq!(
            app.session.as_ref().unwrap().edits.removals.len(),
            1,
            "the stroke landed"
        );
    }

    /// The screen ↔ decode map builds a lens corrector, so it is made once per view and reused while the
    /// pointer moves — and made again when the geometry changes.
    #[test]
    fn the_decode_map_is_built_once_per_view() {
        let ctx = egui::Context::default();
        let mut app = remove_app(&ctx);
        let _ = canvas_centre(&mut app, &ctx);
        let first = app
            .session
            .as_ref()
            .unwrap()
            .removal
            .map_ptr()
            .expect("built");
        let _ = canvas_centre(&mut app, &ctx);
        assert_eq!(
            app.session.as_ref().unwrap().removal.map_ptr(),
            Some(first),
            "rebuilt for nothing"
        );
        app.session.as_mut().unwrap().edits.geometry.angle = 3.0;
        let _ = canvas_centre(&mut app, &ctx);
        assert_ne!(
            app.session.as_ref().unwrap().removal.map_ptr(),
            Some(first),
            "a rotation kept the old map"
        );
    }

    /// ⚠⚠ The author's report (2026-10-19): with a lens profile, moving over the photo in Retouch crawled
    /// — every frame built a lens corrector (290–370 ms on an X-T4). Two frames, Heal and Remove alike,
    /// must use the one the session holds.
    #[test]
    fn a_retouch_frame_reuses_the_held_lens_corrector() {
        let ctx = egui::Context::default();
        let mut app = remove_app(&ctx);
        let db = lens_profile::Catalogue::shared().expect("the bundled lens database");
        let profile = db
            .find("Fujifilm", "X-T4", "XF23mmF2 R WR")
            .or_else(|| db.find("Fujifilm", "X-T4", "XF16-80mmF4 R OIS WR"))
            .expect("an X-T4 lens in the bundled database");
        {
            let s = app.session.as_mut().unwrap();
            s.lens.shot = Some(lens_profile::LensShot {
                profile,
                shot: lens_profile::Shot {
                    focal: 23.0,
                    aperture: 4.0,
                },
            });
            s.edits.lens_correction = Some(edit_model::LensCorrection {
                distortion: true,
                ..Default::default()
            });
        }
        for remove in [false, true] {
            app.remove_tool.active = remove;
            let _ = canvas_centre(&mut app, &ctx);
            let held = |app: &crate::RustroomApp| {
                app.session
                    .as_ref()
                    .unwrap()
                    .lens_memo
                    .held()
                    .map(std::sync::Arc::as_ptr)
            };
            let first = held(&app).expect("the frame mapped through the session's corrector");
            let _ = canvas_centre(&mut app, &ctx);
            assert_eq!(held(&app), Some(first), "built again (Remove {remove})");
        }
    }

    /// The meshes a frame paints with a texture of their own — the photo, and each area being shaded.
    fn textured_meshes(app: &mut crate::RustroomApp, ctx: &egui::Context) -> usize {
        fn count(s: &egui::Shape) -> usize {
            match s {
                egui::Shape::Mesh(m) if m.texture_id != egui::TextureId::default() => 1,
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

    /// M10.16c1b, the author's call: while a fill is made, the canvas shades the area it will replace —
    /// Detect objects' whole object, not just the stroke — and stops once the fill is in.
    #[test]
    fn a_removal_being_filled_shades_its_area() {
        let ctx = egui::Context::default();
        let mut app = remove_app(&ctx);
        let r = edit_model::Removal {
            points: vec![[0.5, 0.5]],
            radius: 0.05,
            detect: true,
            variant: 0,
            opacity: 100.0,
            person: None,
        };
        app.session.as_mut().unwrap().edits.removals = vec![r.clone()];
        let plain = textured_meshes(&mut app, &ctx);
        let area = Arc::new(rustroom_app::remove_tool::Area {
            rect: [0.3, 0.3, 0.7, 0.7],
            side: 4,
            alpha: vec![1.0; 16],
        });
        app.session
            .as_mut()
            .unwrap()
            .removal
            .set_areas(vec![(r.clone(), area)]);
        assert_eq!(
            textured_meshes(&mut app, &ctx),
            plain + 1,
            "the area was not shaded"
        );
        app.session.as_mut().unwrap().removal.set_areas(Vec::new());
        assert_eq!(
            textured_meshes(&mut app, &ctx),
            plain,
            "its area outlived it"
        );
    }

    /// A drag in Heal mode places no removal — the mode decides.
    #[test]
    fn a_drag_in_heal_places_no_removal() {
        let ctx = egui::Context::default();
        let mut app = remove_app(&ctx);
        app.remove_tool.active = false;
        let c = canvas_centre(&mut app, &ctx);
        drag_through(&mut app, &ctx, c, c + egui::vec2(60.0, 10.0));
        assert!(app.session.as_ref().unwrap().edits.removals.is_empty());
    }
}
