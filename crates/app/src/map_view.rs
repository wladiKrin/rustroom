//! Library → Map (M10.18b): where the photos in view were taken, as clustered pins over OpenStreetMap
//! tiles; a strip of the clicked cluster's photos (or of the ones with no location); a drop of photos onto
//! the map, or the location field, places them by hand.
//!
//! ⚠⚠ Everything here runs every frame. The pins are derived once per catalog change and view
//! (`refresh_pins`, the catalog's half throttled while a background pass writes), the clusters once per pins and zoom
//! (`ClusterMemo`), and tiles come from the fetcher's disk cache or the network on its own threads.

use crate::theme::Hover as _;
use crate::{theme, RustroomApp};
use eframe::egui;
use rustroom_app::geo::{self, TileId};
use rustroom_app::map_cluster::ClusterMemo;
use rustroom_app::map_points::{self, Pins};
use rustroom_app::map_state::MapCamera;
use rustroom_app::photo_id::PhotoId;
use rustroom_app::place_search::{self, Place, Throttle};
use rustroom_app::tile_fetch::{live_fetch, TileFetcher};
use std::collections::HashMap;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The strip's height: a 64-pt thumbnail row and its title.
const STRIP_H: f32 = 96.0;
/// A strip thumbnail's edge.
const THUMB: f32 = 64.0;
/// The clustering cell, in screen points.
const CELL_PX: f64 = 56.0;
/// How often the pins may be re-derived while the catalog keeps changing (the startup pass).
const PINS_EVERY: Duration = Duration::from_secs(1);
/// Tile textures kept beyond those in view.
const TEXTURES_MAX: usize = 400;

/// Photos dragged from the strip onto the map.
#[derive(Clone)]
pub(crate) struct PhotoDrag(pub(crate) Vec<PhotoId>);

/// The Map view's state, kept across frames.
#[derive(Default)]
pub(crate) struct MapView {
    camera: Option<MapCamera>,
    fetcher: Option<TileFetcher>,
    textures: HashMap<TileId, egui::TextureHandle>,
    /// The catalog's locations, re-read at most every [`PINS_EVERY`] while the catalog keeps changing.
    located: Arc<map_points::Located>,
    located_key: Option<(u64, u64)>,
    located_at: Option<Instant>,
    /// The view the pins were made for (`Library::visible_rev`).
    view_rev: Option<u64>,
    pins: Arc<Pins>,
    /// Bumped whenever `pins` is rebuilt — the clusters' memo key.
    pins_gen: u64,
    clusters: ClusterMemo,
    /// The clicked cluster's entries; `None` lists the photos with no location. ⚠ Entry indices —
    /// cleared whenever the view changes, or they would name other photos (the review's #6).
    strip: Option<Arc<Vec<usize>>>,
    /// The location field's text.
    pub(crate) field: String,
    search: Option<Receiver<Result<Vec<Place>, String>>>,
    matches: Vec<Place>,
    throttle: Throttle,
    /// Tests' fetch — never the network, never the user's cache. `None`: the real one.
    offline: Option<rustroom_app::tile_fetch::FetchTile>,
}

impl MapView {
    /// A Map that fetches nothing — for tests, which must not reach OpenStreetMap.
    #[cfg(test)]
    pub(crate) fn offline() -> Self {
        Self {
            offline: Some(Arc::new(|_| Err("offline".to_string()))),
            ..Self::default()
        }
    }
}

impl MapView {
    /// Re-derive the pins when the catalog or the view changed: the catalog's locations at most once
    /// every [`PINS_EVERY`] while it keeps changing (a background pass writes every frame), the pins at
    /// once when the view did. Returns `(changed, deferred)` — `deferred` when a catalog change waits.
    fn refresh_pins(
        &mut self,
        cat: Option<&catalog::db::Catalog>,
        lib: &crate::library_state::Library,
    ) -> (bool, bool) {
        let key = (
            cat.map_or(0, |c| c.instance()),
            cat.map_or(0, |c| c.change_count()),
        );
        let mut changed = false;
        let stale = self.located_key != Some(key);
        let may = self.located_key.map(|k| k.0) != Some(key.0)
            || self.located_at.is_none_or(|t| t.elapsed() >= PINS_EVERY);
        if stale && may {
            let (at, removed) = cat
                .and_then(|c| c.all_locations_and_removals().ok())
                .unwrap_or_default();
            self.located = Arc::new(map_points::Located::new(at, removed));
            self.located_key = Some(key);
            self.located_at = Some(Instant::now());
            changed = true;
        }
        if self.view_rev != Some(lib.visible_rev) {
            self.view_rev = Some(lib.visible_rev);
            self.strip = None;
            changed = true;
        }
        if changed {
            let visible = lib
                .visible
                .iter()
                .filter_map(|&i| lib.entries.get(i).map(|e| (i, e.id.path.as_path())));
            self.pins = Arc::new(map_points::pins(visible, &self.located));
            self.pins_gen += 1;
        }
        (changed, stale && !may)
    }
}

pub fn show(app: &mut RustroomApp, ui: &mut egui::Ui, ctx: &egui::Context) {
    // ── the pins and the status line ──────────────────────────────────────────────────────────
    let Some(lib) = app.library.as_ref() else {
        egui::CentralPanel::default().show(ui, |ui| {
            ui.centered_and_justified(|ui| {
                theme::empty_state(ui, "Open a folder to see where its photos were taken.");
            });
        });
        return;
    };
    let (changed, deferred) = app.map.refresh_pins(app.catalog.as_ref(), lib);
    let pins = app.map.pins.clone();
    if changed {
        app.status = map_points::status(&pins);
    }
    if deferred {
        ctx.request_repaint_after(PINS_EVERY);
    }
    pump_search(app);

    // ── the tiles' source ─────────────────────────────────────────────────────────────────────
    let template = app.settings.map_tile_url.clone();
    if app
        .map
        .fetcher
        .as_ref()
        .is_none_or(|f| f.template() != template)
    {
        let repaint = ctx.clone();
        let (root, fetch) = match &app.map.offline {
            Some(f) => (None, f.clone()),
            None => (geo::tile_cache_root(&template), live_fetch()),
        };
        app.map.fetcher = Some(TileFetcher::new(
            root,
            template,
            fetch,
            Arc::new(move || repaint.request_repaint()),
        ));
        app.map.textures.clear();
    }

    let mut drop_at: Option<(Vec<PhotoId>, (f64, f64))> = None;
    let mut open_entry: Option<usize> = None;
    let mut place: Option<(f64, f64)> = None;
    let mut remove = false;
    egui::CentralPanel::default()
        .frame(egui::Frame::NONE.fill(theme::tokens::of(ui).canvas))
        .show(ui, |ui| {
            let full = ui.max_rect();
            let map_rect =
                egui::Rect::from_min_max(full.min, egui::pos2(full.max.x, full.max.y - STRIP_H));
            let strip_rect =
                egui::Rect::from_min_max(egui::pos2(full.min.x, map_rect.max.y), full.max);
            let size = (map_rect.width(), map_rect.height());

            // First show: fit the located photos, or centre the one selected.
            if app.map.camera.is_none() && size.0 > 0.0 {
                let lib = app.library.as_ref().expect("checked above");
                let selected: Vec<(f64, f64)> = pins
                    .points
                    .iter()
                    .filter(|(i, _)| lib.selection.contains(*i))
                    .map(|(_, at)| *at)
                    .collect();
                let all: Vec<(f64, f64)> = pins.points.iter().map(|(_, at)| *at).collect();
                app.map.camera = Some(if selected.len() == 1 {
                    MapCamera {
                        center: selected[0],
                        zoom: 14.0,
                    }
                } else {
                    MapCamera::fit(&all, size)
                });
            }
            let mut cam = app.map.camera.unwrap_or_default();

            // ── gestures: drag pans, scroll and pinch zoom about the pointer ──────────────────
            let resp = ui.interact(map_rect, ui.id().with("map"), egui::Sense::click_and_drag());
            let dialog_open = !matches!(app.modal, crate::modals::Modal::None);
            if resp.dragged() && !egui::DragAndDrop::has_any_payload(ctx) {
                let d = resp.drag_delta();
                cam.pan((d.x, d.y), size);
            }
            if resp.hovered() && !dialog_open {
                let (scroll, pinch, pointer) = ui.input(|i| {
                    (
                        i.smooth_scroll_delta.y,
                        i.zoom_delta(),
                        i.pointer.hover_pos(),
                    )
                });
                let steps = f64::from(scroll) / 240.0 + f64::from(pinch).log2();
                if steps != 0.0 {
                    if let Some(p) = pointer {
                        let rel = p - map_rect.min;
                        cam.zoom_about(steps, (rel.x, rel.y), size);
                    }
                }
            }

            // ── tiles ─────────────────────────────────────────────────────────────────────────
            let painter = ui.painter_at(map_rect);
            let z = cam.zoom.round().clamp(0.0, 19.0) as u8;
            let scale = (cam.zoom - f64::from(z)).exp2() as f32;
            let centre_z = geo::to_world(cam.center.0, cam.center.1, f64::from(z));
            let tiles = geo::tiles_for(centre_z, z, (size.0 / scale, size.1 / scale));
            let now = Instant::now();
            let fetcher = app.map.fetcher.as_mut().expect("made above");
            for (t, bytes) in fetcher.poll(now) {
                if let Ok(b) = &bytes {
                    if let Ok(img) = image::load_from_memory(b) {
                        let rgba = img.to_rgba8();
                        let ci = egui::ColorImage::from_rgba_unmultiplied(
                            [rgba.width() as usize, rgba.height() as usize],
                            &rgba,
                        );
                        let tex = ctx.load_texture(
                            format!("tile-{}-{}-{}", t.z, t.x, t.y),
                            ci,
                            egui::TextureOptions::LINEAR,
                        );
                        app.map.textures.insert(t, tex);
                        continue;
                    }
                }
                // ⚠ Not a picture, or not fetched: the fetcher waits before asking again (#7).
                fetcher.fail(t, now);
            }
            let wanted: Vec<TileId> = tiles.iter().map(|(t, _)| *t).collect();
            for (t, at) in &tiles {
                let r = egui::Rect::from_min_size(
                    map_rect.min + egui::vec2(at.0, at.1) * scale,
                    egui::Vec2::splat(geo::TILE as f32 * scale),
                );
                match app.map.textures.get(t) {
                    Some(tex) => {
                        let uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0));
                        painter.image(tex.id(), r, uv, egui::Color32::WHITE);
                    }
                    None => fetcher.request(*t, now),
                }
            }
            fetcher.retain_only(&wanted);
            if app.map.textures.len() > TEXTURES_MAX {
                app.map.textures.retain(|t, _| wanted.contains(t));
            }

            // ── the pins ──────────────────────────────────────────────────────────────────────
            // ⚠ In the tile zoom's pixels, so the memo key needs no fractional scale (the review's #11).
            let gen = app.map.pins_gen;
            let clusters = app.map.clusters.get(gen, z, CELL_PX, || {
                pins.points.iter().map(|(_, at)| *at).collect()
            });
            let accent = ui.visuals().selection.bg_fill;
            let on_accent = theme::tokens::on_accent(ui.visuals().dark_mode);
            let ring = theme::tokens::of(ui).text;
            let lib = app.library.as_ref().expect("checked above");
            let mut hit: Option<(Vec<usize>, egui::Pos2)> = None;
            let pointer = resp.interact_pointer_pos().or(resp.hover_pos());
            for c in clusters.iter() {
                let p = map_rect.center()
                    + egui::vec2(
                        ((c.at.0 - centre_z.0) as f32) * scale,
                        ((c.at.1 - centre_z.1) as f32) * scale,
                    );
                if !map_rect.expand(24.0).contains(p) {
                    continue;
                }
                let n = c.members.len();
                let r = 9.0 + (n as f32).log2().min(6.0) * 2.0;
                let entries: Vec<usize> = c.members.iter().map(|&m| pins.points[m].0).collect();
                if entries.iter().any(|&i| lib.selection.contains(i)) {
                    painter.circle_stroke(p, r + 3.0, egui::Stroke::new(2.0, ring));
                }
                painter.circle_filled(p, r, accent);
                if n > 1 {
                    painter.text(
                        p,
                        egui::Align2::CENTER_CENTER,
                        n.to_string(),
                        egui::FontId::proportional(theme::tokens::CAPTION),
                        on_accent,
                    );
                }
                if pointer.is_some_and(|q| q.distance(p) <= r + 2.0) {
                    hit = Some((entries, p));
                }
            }
            if resp.clicked() {
                app.map.strip = hit.as_ref().map(|(e, _)| Arc::new(e.clone()));
            }
            if resp.double_clicked() {
                if let Some((_, p)) = &hit {
                    let rel = *p - map_rect.min;
                    cam.zoom_about(2.0, (rel.x, rel.y), size);
                }
            }
            resp.clone()
                .hover("Drag to move the map, scroll to zoom, click a pin to list its photos");

            // ── a drop of photos places them where it lands ───────────────────────────────────
            if let Some(dragged) = resp.dnd_release_payload::<PhotoDrag>() {
                if let Some(q) = ui.input(|i| i.pointer.hover_pos()) {
                    let rel = q - map_rect.min;
                    drop_at = Some((dragged.0.clone(), cam.from_view((rel.x, rel.y), size)));
                }
            }

            // ── the controls over the map ─────────────────────────────────────────────────────
            overlay(app, ui, map_rect, &mut cam, size, &mut place, &mut remove);
            app.map.camera = Some(cam);

            // ── the strip ─────────────────────────────────────────────────────────────────────
            strip(app, ui, strip_rect, &pins, &mut open_entry);
        });

    if let Some((ids, at)) = drop_at {
        app.place_at(&ids, at);
    }
    if let Some(at) = place {
        let ids = selected_ids(app);
        app.place_at(&ids, at);
    }
    if remove {
        let ids = selected_ids(app);
        app.remove_location(&ids);
    }
    if let Some(i) = open_entry {
        app.open_entry(ctx, i);
    }
    if app.map.fetcher.as_ref().is_some_and(|f| f.busy() > 0) || app.map.search.is_some() {
        ctx.request_repaint_after(Duration::from_millis(100));
    }
}

/// The Library's selected photos.
fn selected_ids(app: &RustroomApp) -> Vec<PhotoId> {
    app.library.as_ref().map_or_else(Vec::new, |l| {
        l.selection
            .iter()
            .filter_map(|i| l.entries.get(i).map(|e| e.id.clone()))
            .collect()
    })
}

/// The location field and its matches at the top left, the zoom buttons at the top right, the credit
/// at the bottom right.
fn overlay(
    app: &mut RustroomApp,
    ui: &mut egui::Ui,
    map_rect: egui::Rect,
    cam: &mut MapCamera,
    size: (f32, f32),
    place: &mut Option<(f64, f64)>,
    remove: &mut bool,
) {
    let selected = app.library.as_ref().map_or(0, |l| l.selection.len());
    let field_rect = egui::Rect::from_min_size(
        map_rect.min + egui::vec2(12.0, 12.0),
        egui::vec2(300.0, 200.0),
    );
    ui.scope_builder(egui::UiBuilder::new().max_rect(field_rect), |ui| {
        egui::Frame::popup(ui.style()).show(ui, |ui| {
            ui.horizontal(|ui| {
                let r = ui
                    .add(
                        egui::TextEdit::singleline(&mut app.map.field)
                            .hint_text("Place or 38.80, -9.38")
                            .desired_width(190.0),
                    )
                    .hover("Type a place or coordinates and press Enter to put the selected photos there");
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if enter && selected > 0 {
                    let q = app.map.field.trim().to_string();
                    if let Some(at) = place_search::parse_coordinates(&q) {
                        *place = Some(at);
                        cam.center = at;
                    } else if !q.is_empty() && app.map.throttle.may_send(Instant::now()) {
                        app.map.matches.clear();
                        app.map.search = Some(start_search(q));
                    }
                }
                if ui
                    .add_enabled(selected > 0, egui::Button::new("Remove"))
                    .hover("Removes the location of the selected photos, whatever the file says")
                    .clicked()
                {
                    *remove = true;
                }
            });
            let matches = std::mem::take(&mut app.map.matches);
            let mut keep = true;
            for m in &matches {
                if ui
                    .selectable_label(false, &m.name)
                    .hover("Puts the selected photos here")
                    .clicked()
                {
                    *place = Some(m.at);
                    cam.center = m.at;
                    cam.zoom = cam.zoom.max(12.0);
                    keep = false;
                }
            }
            if keep {
                app.map.matches = matches;
            }
        });
    });

    let zoom_rect = egui::Rect::from_min_size(
        egui::pos2(map_rect.max.x - 12.0 - 28.0, map_rect.min.y + 12.0),
        egui::vec2(28.0, 64.0),
    );
    ui.scope_builder(egui::UiBuilder::new().max_rect(zoom_rect), |ui| {
        let mid = (size.0 / 2.0, size.1 / 2.0);
        if ui.button("+").hover("Zooms in").clicked() {
            cam.zoom_about(1.0, mid, size);
        }
        if ui.button("−").hover("Zooms out").clicked() {
            cam.zoom_about(-1.0, mid, size);
        }
    });

    let credit = "© OpenStreetMap contributors";
    let t = theme::tokens::of(ui);
    let galley = ui.painter().layout_no_wrap(
        credit.to_string(),
        egui::FontId::proportional(theme::tokens::CAPTION),
        t.text2,
    );
    let r = egui::Rect::from_min_size(
        map_rect.max - galley.size() - egui::vec2(10.0, 6.0),
        galley.size() + egui::vec2(8.0, 4.0),
    );
    ui.painter().rect_filled(r, 4.0, t.panel);
    ui.painter()
        .galley(r.min + egui::vec2(4.0, 2.0), galley, t.text2);
    ui.interact(r, ui.id().with("map-credit"), egui::Sense::hover())
        .hover("The map's data and tiles are OpenStreetMap's, under the ODbL");
}

/// The place search, on a thread of its own.
fn start_search(q: String) -> Receiver<Result<Vec<Place>, String>> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .user_agent(place_search::USER_AGENT)
            .timeout_global(Some(Duration::from_secs(15)))
            .build()
            .into();
        let r = agent
            .get(&place_search::query_url(&q))
            .call()
            .map_err(|e| e.to_string())
            .and_then(|mut r| r.body_mut().read_to_string().map_err(|e| e.to_string()))
            .map(|body| place_search::parse(&body));
        let _ = tx.send(r);
    });
    rx
}

fn pump_search(app: &mut RustroomApp) {
    let Some(rx) = &app.map.search else { return };
    match rx.try_recv() {
        Ok(Ok(found)) => {
            app.status = if found.is_empty() {
                "No place found by that name".to_string()
            } else {
                String::new()
            };
            app.map.matches = found;
            app.map.search = None;
        }
        Ok(Err(e)) => {
            app.status = format!("The place search failed: {e}");
            app.map.search = None;
        }
        Err(std::sync::mpsc::TryRecvError::Empty) => {}
        Err(std::sync::mpsc::TryRecvError::Disconnected) => app.map.search = None,
    }
}

/// The strip: the clicked cluster's photos, or the ones with no location. A click selects, a
/// double-click opens in Develop, a drag carries the selection onto the map.
fn strip(
    app: &mut RustroomApp,
    ui: &mut egui::Ui,
    rect: egui::Rect,
    pins: &Pins,
    open_entry: &mut Option<usize>,
) {
    let t = theme::tokens::of(ui);
    ui.painter().rect_filled(rect, 0.0, t.panel);
    ui.painter()
        .hline(rect.x_range(), rect.min.y, egui::Stroke::new(1.0, t.line));
    // ⚠ Borrowed through the Arcs, never cloned per frame — a "No location" strip may be 100k long.
    let clicked = app.map.strip.clone();
    let rows: &[usize] = match &clicked {
        Some(e) => e,
        None => &pins.unplaced,
    };
    let title = match &app.map.strip {
        Some(e) => rustroom_app::status_text::photos(e.len()),
        None => format!("No location · {}", rows.len()),
    };
    let live = app.live_edits();
    let Some(lib) = app.library.as_mut() else {
        return;
    };
    let inner = rect.shrink2(egui::vec2(12.0, 6.0));
    ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(&title)
                    .size(theme::tokens::CAPTION)
                    .color(t.text2),
            )
            .hover("Drag photos from here onto the map to place them there");
        });
        let mut window = Vec::new();
        // ⚠⚠ Virtual: only the cells in view are laid out and ask for a thumbnail (the review's #1) —
        // every thumbnail asked for is pinned in the cache by `sync_thumb_window`.
        let cell = THUMB + ui.spacing().item_spacing.x;
        egui::ScrollArea::horizontal().show_viewport(ui, |ui, viewport| {
            let origin = ui.max_rect().min;
            ui.allocate_space(egui::vec2(cell * rows.len() as f32, THUMB));
            {
                for k in
                    map_points::visible_cells(rows.len(), cell, viewport.min.x, viewport.width())
                {
                    let i = rows[k];
                    let Some(e) = lib.entries.get(i) else {
                        continue;
                    };
                    let id = e.id.clone();
                    let req = lib.thumb_request(i, live.as_ref());
                    let tex = lib
                        .cache
                        .get(&req.key())
                        .or_else(|| lib.cache.get_stale(&id));
                    window.push(req);
                    let r = egui::Rect::from_min_size(
                        origin + egui::vec2(cell * k as f32, 0.0),
                        egui::Vec2::splat(THUMB),
                    );
                    let resp = ui.interact(
                        r,
                        ui.id().with(("strip", &id)),
                        egui::Sense::click_and_drag(),
                    );
                    match tex {
                        Some(tex) => {
                            let s = tex.size_vec2();
                            let img = egui::Rect::from_center_size(
                                r.center(),
                                s * (THUMB / s.x.max(s.y)),
                            );
                            ui.painter().image(
                                tex.id(),
                                img,
                                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                                egui::Color32::WHITE,
                            );
                        }
                        None => {
                            ui.painter().rect_filled(r, 8.0, t.card);
                        }
                    }
                    if lib.selection.contains(i) {
                        ui.painter().rect_stroke(
                            r.expand(2.0),
                            8.0,
                            egui::Stroke::new(2.0, ui.visuals().selection.bg_fill),
                            egui::StrokeKind::Outside,
                        );
                    }
                    let resp = resp.hover(&e.file_name);
                    if resp.drag_started() {
                        if !lib.selection.contains(i) {
                            lib.selection.click(i);
                        }
                        let ids: Vec<PhotoId> = lib
                            .selection
                            .iter()
                            .filter_map(|j| lib.entries.get(j).map(|e| e.id.clone()))
                            .collect();
                        resp.dnd_set_drag_payload(PhotoDrag(ids));
                    }
                    if resp.double_clicked() {
                        *open_entry = Some(i);
                    } else if resp.clicked() {
                        let mods = ui.input(|m| m.modifiers);
                        if mods.command {
                            lib.selection.toggle(i);
                        } else if mods.shift {
                            lib.selection.extend_to(i);
                        } else {
                            lib.selection.click(i);
                        }
                    }
                }
            }
        });
        // ⚠⚠ Everything drawn, or `retain_window` leaves these thumbnails evictable (Survey's rule).
        lib.sync_thumb_window(&window);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The review's #6: the strip holds entry indices, so a new view drops it — or it would list, and
    /// a drag would place, whatever photos now sit at those indices.
    #[test]
    fn a_new_view_clears_the_strip() {
        let mut app = crate::test_app::for_test();
        app.library = Some(crate::test_app::library_of(4));
        let mut map = MapView::offline();
        let lib = app.library.as_ref().unwrap();
        map.refresh_pins(app.catalog.as_ref(), lib);
        map.strip = Some(Arc::new(vec![1, 2]));
        map.refresh_pins(app.catalog.as_ref(), lib);
        assert!(map.strip.is_some(), "an unchanged view dropped the strip");
        app.rebuild_entries(None);
        map.refresh_pins(app.catalog.as_ref(), app.library.as_ref().unwrap());
        assert!(map.strip.is_none(), "the strip outlived its view");
    }
}
