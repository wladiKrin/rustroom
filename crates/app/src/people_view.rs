//! Library → People (M10.18c): who is in the photos, and the faces still to name — the mockup's layout A
//! (`https://claude.ai/artifact/8veXQc9qb5x7ivD4GnM5UW`). Named people across the top; below, "Who is this?":
//! the shown photos' unnamed faces grouped by likeness, each group named at once or answered Yes / No.
//!
//! ⚠⚠ Nothing here may grow with the catalog per frame (the M10.18c review): the groups are made OFF the UI thread
//! when the faces or the view change (`Catalog::faces_revision`, `Library::visible_rev`), at most every
//! [`REGROUP_EVERY`] while a search writes; the path index is rebuilt only with the view; the groups are drawn as
//! virtual rows, so only the cards on screen ask for a thumbnail. The search runs in the background while the view
//! is open ([`RustroomApp::pump_face_pass`]), on local photos only — an Immich photo has Immich's faces. A face's
//! crop is cut from its photo's UNEDITED thumbnail, the frame its box is measured in.

use crate::theme::Hover as _;
use crate::{theme, RustroomApp};
use catalog::db::FaceRow;
use eframe::egui;
use rustroom_app::face_groups::{self, Group};
use rustroom_app::face_pass::{FaceFinder, FacePass};
use rustroom_app::photo_id::PhotoId;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A face crop's edge in the groups.
const FACE: f32 = 56.0;
/// How many faces a group shows.
const SHOWN: usize = 5;
/// A group card's inner width and its row's height. ⚠ Wide enough for [`SHOWN`] crops and their gaps
/// (5 × 56 + 4 × 8 = 312): narrower, the crops pushed every card wider and the rows past the window (found in use).
const CARD: f32 = 320.0;
const ROW: f32 = 150.0;
/// How often the groups may be made again while a search keeps landing faces.
const REGROUP_EVERY: Duration = Duration::from_secs(2);

/// What a grouping is for: the catalog, its faces' revision, the view.
type Key = (u64, u64, u64);
type Grouped = (Key, Arc<Vec<Group>>, Arc<HashMap<i64, FaceRow>>);

/// The People view's state.
#[derive(Default)]
pub(crate) struct PeopleView {
    groups: Arc<Vec<Group>>,
    /// The faces the groups were made from, by id — read with them.
    faces: Arc<HashMap<i64, FaceRow>>,
    /// What the groups on show were made for, and what a running grouping is for.
    grouped: Option<Key>,
    grouping: Option<(Key, Receiver<Grouped>)>,
    grouped_at: Option<Instant>,
    /// Each entry's index by path, for the view `index_rev` names — rebuilt only when the view changes.
    index: Arc<HashMap<PathBuf, usize>>,
    index_rev: Option<u64>,
    /// Each group's name field, by its first face. Cleared with the view.
    names: HashMap<i64, String>,
    pass: Option<FacePass>,
    /// What the last look for unsearched photos was for: the view and the faces' revision. ⚠⚠ A look, never a
    /// verdict — it is repeated whenever either moves, so no early exit can leave a view unsearched for good
    /// (found in use, 2026-10-28: only Reindex made a folder's faces appear).
    pub(crate) looked: Option<(u64, u64)>,
    /// The view the running search is for.
    pass_rev: Option<u64>,
    /// Photos the search could not read this session — never retried in a loop.
    failed: std::collections::HashSet<PathBuf>,
    /// Waiting for the models: the offer was made, and the view looks again only when they arrive.
    waiting_models: bool,
    /// Tests' finder — never the networks. `None`: the real one.
    pub(crate) finder: Option<FaceFinder>,
    /// The models were offered this session.
    offered: bool,
    /// The last frame's crops: `(with their picture, still blank)` — what a test reads.
    pub(crate) crops: (usize, usize),
}

enum Act {
    Name(Vec<i64>, String),
    Yes(Vec<i64>, i64),
    No(Vec<i64>, i64),
    Open(PhotoId),
    Dismiss(Vec<i64>),
    Person(catalog::db::PersonRow),
}

impl RustroomApp {
    /// The face search's models — the AI flow's folder, or a test's.
    fn face_models(&self) -> Option<std::path::PathBuf> {
        self.people_models()
    }

    /// Start a search over the shown local photos never searched, when the view is open and the setting is on.
    ///
    /// ⚠ The models are checked FIRST and the view is marked as tried either way: with them missing and the offer
    /// declined, this used to re-read the catalog for every shown photo on every frame (the review's #4).
    fn start_face_pass(&mut self) {
        let Some(rev) = self.library.as_ref().map(|l| l.visible_rev) else {
            return;
        };
        let models = self.face_models();
        let missing: Vec<ai_mask::ModelId> = if self.people.finder.is_some() {
            Vec::new()
        } else {
            [ai_mask::ModelId::YunetFace, ai_mask::ModelId::SfaceFace]
                .into_iter()
                .filter(|m| {
                    !models
                        .as_deref()
                        .is_some_and(|d| ai_mask::download::present(&m.spec(), d))
                })
                .collect()
        };
        self.people.waiting_models = !missing.is_empty();
        // ⚠ Another view's search stops at once — what it found is kept, the rest stays unsearched for its next
        // visit — and this view's starts (asked for in use, 2026-10-28).
        if self.people.pass.is_some() && self.people.pass_rev != Some(rev) {
            self.stop_face_pass();
        }
        if !self.settings.find_faces || self.people.pass.is_some() {
            return;
        }
        if !missing.is_empty() {
            self.people.waiting_models = true;
            // ⚠ Offered once a session.
            if !self.people.offered {
                self.people.offered = true;
                self.ai.proposed = missing;
                self.modal = crate::modals::Modal::AiDownload;
            }
            return;
        }
        let faces_rev = self.catalog.as_ref().map_or(0, |c| c.faces_revision());
        if self.people.looked == Some((rev, faces_rev)) {
            return;
        }
        self.people.looked = Some((rev, faces_rev));
        let Some(lib) = &self.library else { return };
        let root = self.immich_cache_root();
        let mut local: Vec<PathBuf> = lib
            .visible
            .iter()
            .filter_map(|&i| lib.entries.get(i))
            .map(|e| e.id.path.clone())
            .filter(|p| {
                !root
                    .as_deref()
                    .is_some_and(|r| rustroom_app::immich_source::is_immich_path(r, p))
            })
            .filter(|p| !self.people.failed.contains(p))
            .collect();
        local.sort();
        local.dedup();
        let Some(todo) = self
            .catalog
            .as_ref()
            .and_then(|c| c.files_without_faces_read(&local).ok())
        else {
            return;
        };
        if todo.is_empty() {
            return;
        }
        let finder = match &self.people.finder {
            Some(f) => f.clone(),
            None => rustroom_app::face_pass::real_finder(models.expect("checked above")),
        };
        self.people.pass = Some(FacePass::start(todo, finder));
        self.people.pass_rev = Some(rev);
    }

    /// Stop the running search, keeping what it has found.
    fn stop_face_pass(&mut self) {
        let Some(p) = self.people.pass.take() else {
            return;
        };
        p.cancel();
        let (found, _) = p.drain();
        if let Some(cat) = &self.catalog {
            for (path, faces) in &found {
                if let Some(faces) = faces {
                    if let Err(e) = cat.set_local_faces(path, faces) {
                        eprintln!("catalog: {e}");
                    }
                }
            }
        }
        self.people.pass_rev = None;
    }

    /// Once a frame: the search's findings into the catalog. Every write moves `change_count`, which is what
    /// regroups the view.
    pub(crate) fn pump_face_pass(&mut self, ctx: &egui::Context) {
        let Some(pass) = &self.people.pass else {
            return;
        };
        let (found, done) = pass.drain();
        if let Some(cat) = &self.catalog {
            for (path, faces) in &found {
                if let Some(faces) = faces {
                    if let Err(e) = cat.set_local_faces(path, faces) {
                        eprintln!("catalog: {e}");
                    }
                }
            }
        }
        if done {
            if let Some(p) = self.people.pass.take() {
                self.people.failed.extend(p.failed_paths());
                let (n, why) = p.failures();
                let (_, total) = p.progress();
                self.status = match (n, why) {
                    (0, _) => "Looked for faces".to_string(),
                    (n, Some(why)) => format!(
                        "Could not look for faces in {} of {total}: {why}",
                        rustroom_app::status_text::photos(n)
                    ),
                    (n, None) => format!(
                        "Could not look for faces in {}",
                        rustroom_app::status_text::photos(n)
                    ),
                };
            }
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(150));
        }
    }

    /// The status row's line while the search runs.
    pub(crate) fn face_pass_line(&self) -> Option<String> {
        let (done, total) = self.people.pass.as_ref()?.progress();
        Some(format!("Looking for faces · {done} of {total} photos"))
    }

    /// The status row's Cancel — what was found is kept.
    pub(crate) fn cancel_face_pass(&mut self) {
        self.stop_face_pass();
        self.status = "Stopped looking for faces".into();
    }
}

/// The shown photos' unnamed faces, grouped — made on a thread of its own when the faces or the view change, the
/// last answer shown meanwhile.
fn groups(
    app: &mut RustroomApp,
    ctx: &egui::Context,
) -> (Arc<Vec<Group>>, Arc<HashMap<i64, FaceRow>>) {
    // A grouping that finished.
    if let Some((_, rx)) = &app.people.grouping {
        if let Ok((key, g, f)) = rx.try_recv() {
            app.people.groups = g;
            app.people.faces = f;
            app.people.grouped = Some(key);
            app.people.grouping = None;
        } else {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
    let (Some(cat), Some(lib)) = (&app.catalog, &app.library) else {
        return (Arc::default(), Arc::default());
    };
    let key: Key = (cat.instance(), cat.faces_revision(), lib.visible_rev);
    let view_changed = app
        .people
        .grouped
        .is_none_or(|k| (k.0, k.2) != (key.0, key.2));
    let stale = app.people.grouped != Some(key)
        && app.people.grouping.as_ref().is_none_or(|(k, _)| *k != key);
    let throttled = !view_changed
        && app.people.pass.is_some()
        && app
            .people
            .grouped_at
            .is_some_and(|t| t.elapsed() < REGROUP_EVERY);
    if stale && app.people.grouping.is_none() && !throttled {
        // The inputs are read here, once per change; the grouping itself — O(faces × groups) — is the thread's.
        let shown: std::collections::HashSet<&std::path::Path> = lib
            .visible
            .iter()
            .filter_map(|&i| lib.entries.get(i).map(|e| e.id.path.as_path()))
            .collect();
        let unnamed: Vec<FaceRow> = cat
            .unnamed_local_faces()
            .unwrap_or_default()
            .into_iter()
            .filter(|f| shown.contains(f.photo.as_path()))
            .collect();
        let named: Vec<(i64, Vec<f32>)> = cat
            .named_local_faces()
            .unwrap_or_default()
            .into_iter()
            .filter(|f| f.confirmed)
            .filter_map(|f| Some((f.person?, f.embedding?)))
            .collect();
        let rejected = cat.face_rejections().unwrap_or_default();
        let (tx, rx) = std::sync::mpsc::channel();
        let repaint = ctx.clone();
        std::thread::Builder::new()
            .name("face-groups".into())
            .spawn(move || {
                let input: Vec<(i64, Vec<f32>)> = unnamed
                    .iter()
                    .filter_map(|f| Some((f.id, f.embedding.clone()?)))
                    .collect();
                let g = face_groups::group(&input, &named, &rejected, face_groups::THRESHOLD);
                let f: HashMap<i64, FaceRow> = unnamed.into_iter().map(|f| (f.id, f)).collect();
                let _ = tx.send((key, Arc::new(g), Arc::new(f)));
                repaint.request_repaint();
            })
            .expect("spawn the face grouping");
        app.people.grouping = Some((key, rx));
        app.people.grouped_at = Some(Instant::now());
    }
    // The path index, with the view.
    if app.people.index_rev != Some(lib.visible_rev) {
        app.people.index = Arc::new(
            lib.entries
                .iter()
                .enumerate()
                .filter(|(_, e)| e.id.copy == 0)
                .map(|(i, e)| (e.id.path.clone(), i))
                .collect(),
        );
        app.people.index_rev = Some(lib.visible_rev);
        app.people.names.clear();
    }
    (app.people.groups.clone(), app.people.faces.clone())
}

pub fn show(app: &mut RustroomApp, ui: &mut egui::Ui, ctx: &egui::Context) {
    app.ask_immich_people();
    app.start_face_pass();
    let (groups, faces) = groups(app, ctx);
    let people = app.rail_data.people.clone();
    let index = app.people.index.clone();
    let searching = app.people.pass.as_ref().map(|p| p.progress());
    let (find_faces, waiting_models) = (app.settings.find_faces, app.people.waiting_models);
    let mut download = false;
    let mut act: Option<Act> = None;
    egui::CentralPanel::default().show(ui, |ui| {
        if app.library.is_none() {
            ui.centered_and_justified(|ui| {
                theme::empty_state(ui, "Open a folder to see who is in its photos.");
            });
            return;
        }
        let names = &mut app.people.names;
        let Some(lib) = app.library.as_mut() else {
            return;
        };
        let mut window = Vec::new();
        let mut crops = (0, 0);
        theme::sub_label(ui, &format!("People · {}", people.len()));
        ui.horizontal_wrapped(|ui| {
            for p in people.iter() {
                let label = if p.immich_id.is_some() && p.photos == 0 {
                    p.name.clone()
                } else {
                    format!("{} · {}", p.name, p.photos)
                };
                if ui
                    .button(label)
                    .hover("Shows the photos this person is in")
                    .clicked()
                {
                    act = Some(Act::Person(p.clone()));
                }
            }
        });
        ui.add_space(12.0);
        let n = groups.len();
        theme::sub_label(ui, &format!("Who is this? · {n} groups"));
        if n == 0 {
            // ⚠ Say what is going on — a blank view read as "nothing happens" (found in use, 2026-10-28).
            let why = if let Some((done, total)) = searching {
                format!("Looking for faces · {done} of {total} photos")
            } else if !find_faces {
                "Finding faces is off — Settings → Behaviour".to_string()
            } else if waiting_models {
                "The face models are not downloaded yet".to_string()
            } else {
                "No unnamed faces in these photos".to_string()
            };
            theme::hint(ui, &why);
            if waiting_models
                && ui
                    .button("Download the face models…")
                    .hover("Downloads the two networks that find and tell apart faces")
                    .clicked()
            {
                download = true;
            }
        }
        // ⚠ Virtual rows of cards: only the rows on screen lay out and ask for thumbnails (the review's #3).
        // ⚠ From the card's OUTER width — the group frame's margins and stroke — and the gap between cards.
        let gap = ui.spacing().item_spacing.x;
        let frame = egui::Frame::group(ui.style());
        let outer = CARD + frame.inner_margin.sum().x + 2.0 * frame.stroke.width;
        let per_row = (((ui.available_width() + gap) / (outer + gap)).floor() as usize).max(1);
        let rows = n.div_ceil(per_row);
        egui::ScrollArea::vertical().show_rows(ui, ROW, rows, |ui, range| {
            for row in range {
                ui.horizontal(|ui| {
                    for g in groups.iter().skip(row * per_row).take(per_row) {
                        card(
                            ui,
                            g,
                            &faces,
                            &people,
                            &index,
                            lib,
                            names,
                            &mut window,
                            &mut act,
                            &mut crops,
                        );
                    }
                });
            }
        });
        // ⚠⚠ Everything drawn, or `retain_window` leaves these thumbnails evictable (Survey's rule).
        lib.sync_thumb_window(&window);
        app.people.crops = crops;
    });
    if download {
        app.ai.proposed = vec![ai_mask::ModelId::YunetFace, ai_mask::ModelId::SfaceFace];
        app.modal = crate::modals::Modal::AiDownload;
    }
    match act {
        Some(Act::Name(f, n)) => app.name_faces(&f, &n),
        Some(Act::Yes(f, p)) => app.confirm_faces(&f, p),
        Some(Act::No(f, p)) => app.reject_faces(&f, p),
        Some(Act::Dismiss(f)) => app.dismiss_faces(&f),
        Some(Act::Open(id)) => {
            let i = app
                .library
                .as_ref()
                .and_then(|l| l.entries.iter().position(|e| e.id == id));
            if let Some(i) = i {
                app.open_entry(ctx, i);
            }
        }
        Some(Act::Person(p)) => app.open_person(ctx, &p),
        None => {}
    }
}

/// One group's card: its faces, its suggestion's Yes / No, its name field.
#[allow(clippy::too_many_arguments)]
fn card(
    ui: &mut egui::Ui,
    g: &Group,
    faces: &HashMap<i64, FaceRow>,
    people: &[catalog::db::PersonRow],
    index: &HashMap<PathBuf, usize>,
    lib: &mut crate::library_state::Library,
    names: &mut HashMap<i64, String>,
    window: &mut Vec<rustroom_app::thumb_loader::ThumbRequest>,
    act: &mut Option<Act>,
    crops: &mut (usize, usize),
) {
    let first = g.faces[0];
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(CARD);
        ui.set_height(ROW - 24.0);
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                for f in g.faces.iter().take(SHOWN).filter_map(|id| faces.get(id)) {
                    let (r, resp) =
                        ui.allocate_exact_size(egui::Vec2::splat(FACE), egui::Sense::click());
                    if index.contains_key(&f.photo) {
                        // ⚠ The UNEDITED picture: the box is measured on the upright original, and an edited
                        // thumbnail's crop or rotation would put a wall where the face is (the review's #5).
                        let req = rustroom_app::thumb_loader::ThumbRequest {
                            id: PhotoId::original(f.photo.clone()),
                            edits: edit_model::Edits::default(),
                        };
                        let tex = lib.cache.get(&req.key());
                        window.push(req);
                        match tex {
                            Some(t) => {
                                crops.0 += 1;
                                let uv = egui::Rect::from_min_max(
                                    egui::pos2(f.bbox[0], f.bbox[1]),
                                    egui::pos2(f.bbox[2], f.bbox[3]),
                                );
                                ui.painter().image(t.id(), r, uv, egui::Color32::WHITE);
                            }
                            None => {
                                crops.1 += 1;
                                ui.painter()
                                    .rect_filled(r, FACE / 2.0, theme::tokens::of(ui).card);
                            }
                        }
                    }
                    let resp = resp.hover("Right-click for this face's photo, or to remove it");
                    let (id, photo) = (f.id, f.photo.clone());
                    resp.context_menu(|ui| {
                        if ui.button("Open photo").clicked() {
                            *act = Some(Act::Open(PhotoId::original(photo.clone())));
                            ui.close();
                        }
                        if ui.button("Not a face").clicked() {
                            *act = Some(Act::Dismiss(vec![id]));
                            ui.close();
                        }
                    });
                }
            });
            let count = rustroom_app::status_text::photos(g.faces.len()).replace("photo", "face");
            match g.suggest.and_then(|s| people.iter().find(|p| p.id == s)) {
                Some(p) => {
                    ui.label(format!("{}? · {count}", p.name));
                    ui.horizontal(|ui| {
                        if ui
                            .button("Yes")
                            .hover("Names these faces after this person")
                            .clicked()
                        {
                            *act = Some(Act::Yes(g.faces.clone(), p.id));
                        }
                        if ui
                            .button("No")
                            .hover("Never offers these faces as this person again")
                            .clicked()
                        {
                            *act = Some(Act::No(g.faces.clone(), p.id));
                        }
                    });
                }
                None => theme::hint(ui, &count),
            }
            let field = names.entry(first).or_default();
            let r = ui
                .add(
                    egui::TextEdit::singleline(field)
                        .hint_text("Name")
                        .desired_width(CARD - 16.0),
                )
                .hover("Type a name and press Enter to name every face in this group");
            if r.lost_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter))
                && !field.trim().is_empty()
            {
                *act = Some(Act::Name(g.faces.clone(), std::mem::take(field)));
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use catalog::db::NewFace;
    use std::time::{Duration, Instant};

    /// The view starts a search on the shown local photos with an injected finder, and its faces land.
    #[test]
    fn the_view_starts_a_search_with_an_injected_finder_and_lands_faces() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        let d = test_dir::TempDir::new("people-view");
        // Real (tiny) JPEGs, so the pass has a picture to hand the finder.
        for n in ["A.JPG", "B.JPG"] {
            image::RgbImage::from_pixel(64, 48, image::Rgb([120, 90, 70]))
                .save(d.join(n))
                .unwrap();
        }
        app.open_folder_at(&ctx, d.path().to_path_buf());
        app.people.finder = Some(Arc::new(|_| {
            Ok(vec![NewFace {
                bbox: [0.1, 0.1, 0.3, 0.3],
                embedding: vec![0.1; 128],
            }])
        }));
        app.start_face_pass();
        assert!(app.face_pass_line().is_some(), "no search started");
        let t = Instant::now();
        while app.people.pass.is_some() {
            assert!(t.elapsed() < Duration::from_secs(120), "the search hung");
            app.pump_face_pass(&ctx);
            std::thread::sleep(Duration::from_millis(2));
        }
        let cat = app.catalog.as_ref().unwrap();
        assert_eq!(
            cat.unnamed_local_faces().unwrap().len(),
            2,
            "a face for each photo"
        );
        app.start_face_pass();
        assert!(
            app.people.pass.is_none(),
            "the same view was searched again"
        );
    }

    /// The review's #3 and #5: 400 groups lay out a screenful, and each crop asks for the UNEDITED thumbnail.
    #[test]
    fn only_the_groups_on_screen_ask_for_a_thumbnail_and_they_are_unedited() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        app.settings.find_faces = false;
        let d = test_dir::TempDir::new("people-many");
        let names: Vec<String> = (0..400).map(|i| format!("P{i:03}.JPG")).collect();
        for n in &names {
            std::fs::write(d.join(n), b"x").unwrap();
        }
        app.open_folder_at(&ctx, d.path().to_path_buf());
        let cat = app.catalog.as_ref().unwrap();
        // Unlike one another: a pseudo-random unit vector each, whose cosines sit near 0 — far below 0.363.
        let mut seed = 7u64;
        for n in &names {
            let mut e: Vec<f32> = (0..128)
                .map(|_| {
                    seed = seed
                        .wrapping_mul(6364136223846793005)
                        .wrapping_add(1442695040888963407);
                    ((seed >> 33) as f32 / (1u64 << 31) as f32) - 0.5
                })
                .collect();
            let l = e.iter().map(|x| x * x).sum::<f32>().sqrt();
            e.iter_mut().for_each(|x| *x /= l);
            cat.set_local_faces(
                &d.join(n),
                &[NewFace {
                    bbox: [0.1, 0.1, 0.3, 0.3],
                    embedding: e,
                }],
            )
            .unwrap();
        }
        // The grouping lands off the UI thread: draw until it has.
        let t = Instant::now();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        while app.people.groups.len() < 300 {
            assert!(
                t.elapsed() < Duration::from_secs(120),
                "the grouping never landed"
            );
            let _ = crate::test_ui::nodes_on(&ctx, screen, |ui| {
                let c = ui.ctx().clone();
                show(&mut app, ui, &c);
            });
            std::thread::sleep(Duration::from_millis(5));
        }
        let _ = crate::test_ui::nodes_on(&ctx, screen, |ui| {
            let c = ui.ctx().clone();
            show(&mut app, ui, &c);
        });
        let want = &app.library.as_ref().unwrap().last_want;
        assert!(
            !want.is_empty(),
            "fixture check: nothing asked for a thumbnail"
        );
        assert!(
            want.len() < 100,
            "{} thumbnails asked for 400 groups",
            want.len()
        );
        assert!(
            want.iter().all(|r| r.edits == edit_model::Edits::default()),
            "an edited thumbnail was asked for"
        );
    }

    /// The review's #4: with the models missing, the offer is made once and the view is not searched again every
    /// frame; when they arrive (here: a finder handed in) the view is searched.
    #[test]
    fn missing_models_are_offered_once_and_the_view_waits_for_them() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        let d = test_dir::TempDir::new("people-wait");
        image::RgbImage::from_pixel(64, 48, image::Rgb([120, 90, 70]))
            .save(d.join("A.JPG"))
            .unwrap();
        let models = test_dir::TempDir::new("people-wait-models");
        app.ai.dirs = Some((models.join("cache"), models.join("models")));
        app.open_folder_at(&ctx, d.path().to_path_buf());
        app.start_face_pass();
        assert!(app.modal == crate::modals::Modal::AiDownload);
        app.modal = crate::modals::Modal::None;
        app.start_face_pass();
        assert!(app.modal == crate::modals::Modal::None, "offered again");
        assert!(app.people.waiting_models && app.people.pass.is_none());
        app.people.finder = Some(Arc::new(|_| Ok(Vec::new())));
        app.start_face_pass();
        assert!(
            app.people.pass.is_some(),
            "the models arrived and nothing was searched"
        );
    }

    /// Found in use (2026-10-28, "there are faces, but when I click on People, nothing shows up"): the whole path
    /// with the REAL networks — a folder with the crew portrait (a real photo), the People view, its search, its
    /// groups. ⚠ Needs the models in the app's folder: `cargo test --workspace --release a_real_folder -- --ignored`.
    #[test]
    #[ignore]
    fn a_real_folder_shows_its_faces_as_groups() {
        let ctx = crate::test_ui::ctx();
        let mut app = crate::test_app::for_test();
        let d = test_dir::TempDir::new("people-real");
        std::fs::copy(
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../ai-mask/tests/fixtures/apollo11_crew.jpg"
            ),
            d.join("crew.jpg"),
        )
        .unwrap();
        app.ai.dirs = rustroom_app::ai_cache::models_dir().map(|m| (m.clone(), m));
        app.open_folder_at(&ctx, d.path().to_path_buf());
        app.library_mode = crate::LibraryMode::People;
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let t = Instant::now();
        loop {
            let labels: Vec<String> = crate::test_ui::nodes_on(&ctx, screen, |ui| {
                let c = ui.ctx().clone();
                show(&mut app, ui, &c);
            })
            .into_iter()
            .map(|n| n.label)
            .collect();
            app.pump_face_pass(&ctx);
            if app.people.groups.len() == 3 {
                break;
            }
            assert!(
                t.elapsed() < Duration::from_secs(120),
                "no groups — status {:?}, labels {labels:?}",
                app.status
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Found in use (2026-10-28: "it searches it till the end, even when clicking on a different one … it should
    /// stop, save the progress, and start searching within the new folder"): a new folder stops the old search at
    /// once and starts its own; what the old one found is kept, the rest stays unsearched for its next visit.
    #[test]
    fn a_new_folder_stops_the_old_search_and_starts_its_own() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        let (a, b) = (
            test_dir::TempDir::new("people-first"),
            test_dir::TempDir::new("people-second"),
        );
        for d in [&a, &b] {
            for n in ["P1.JPG", "P2.JPG", "P3.JPG"] {
                image::RgbImage::from_pixel(64, 48, image::Rgb([120, 90, 70]))
                    .save(d.join(n))
                    .unwrap();
            }
        }
        // The first photo answers at once; every later one waits behind a gate the test holds.
        let gate = Arc::new(std::sync::Mutex::new(()));
        let held = gate.lock().unwrap();
        let (g, seen) = (
            gate.clone(),
            Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        );
        let s2 = seen.clone();
        app.people.finder = Some(Arc::new(move |_| {
            if s2.fetch_add(1, std::sync::atomic::Ordering::SeqCst) > 0 {
                drop(g.lock().unwrap());
            }
            Ok(vec![NewFace {
                bbox: [0.1, 0.1, 0.3, 0.3],
                embedding: vec![0.1; 128],
            }])
        }));
        app.open_folder_at(&ctx, a.path().to_path_buf());
        app.start_face_pass();
        let t = Instant::now();
        let unread = |app: &RustroomApp, d: &test_dir::TempDir| {
            let files: Vec<PathBuf> = ["P1.JPG", "P2.JPG", "P3.JPG"]
                .iter()
                .map(|n| d.join(n))
                .collect();
            app.catalog
                .as_ref()
                .unwrap()
                .files_without_faces_read(&files)
                .unwrap()
                .len()
        };
        while unread(&app, &a) > 2 {
            app.pump_face_pass(&ctx);
            assert!(
                t.elapsed() < Duration::from_secs(120),
                "the first photo never landed"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        // The second folder, while the first's search waits on its second photo.
        app.open_folder_at(&ctx, b.path().to_path_buf());
        app.start_face_pass();
        let line = app.face_pass_line().unwrap_or_default();
        assert!(
            line.ends_with("of 3 photos"),
            "the new folder's search did not start at once: {line:?}"
        );
        drop(held);
        while app.people.pass.is_some() {
            app.pump_face_pass(&ctx);
            assert!(
                t.elapsed() < Duration::from_secs(120),
                "the second search hung"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(unread(&app, &b), 0, "the new folder was not searched");
        assert_eq!(
            unread(&app, &a),
            2,
            "the old search's found photo was lost, or it ran on"
        );
    }

    /// Found in use (2026-10-28: "after clicking back and forth a couple of times, no images appear in the face
    /// card"): after each switch between two folders, every crop gets its picture.
    #[test]
    fn crops_get_their_pictures_after_switching_folders() {
        let ctx = crate::test_ui::ctx();
        let mut app = crate::test_app::for_test();
        app.settings.find_faces = false;
        let (a, b) = (
            test_dir::TempDir::new("people-switch-a"),
            test_dir::TempDir::new("people-switch-b"),
        );
        let crew = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../ai-mask/tests/fixtures/apollo11_crew.jpg"
        );
        for d in [&a, &b] {
            for n in ["one.jpg", "two.jpg"] {
                std::fs::copy(crew, d.join(n)).unwrap();
            }
        }
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let draw = |app: &mut RustroomApp| {
            app.pump_thumbnails(&ctx);
            let _ = crate::test_ui::nodes_on(&ctx, screen, |ui| {
                let c = ui.ctx().clone();
                show(app, ui, &c);
            });
        };
        for (round, d) in [&a, &b, &a, &b, &a].into_iter().enumerate() {
            app.open_folder_at(&ctx, d.path().to_path_buf());
            if round < 2 {
                let cat = app.catalog.as_ref().unwrap();
                for n in ["one.jpg", "two.jpg"] {
                    let f = NewFace {
                        bbox: [0.2, 0.2, 0.4, 0.5],
                        embedding: vec![0.088; 128],
                    };
                    cat.set_local_faces(&d.join(n), &[f]).unwrap();
                }
            }
            let t = Instant::now();
            loop {
                draw(&mut app);
                let (drawn, blank) = app.people.crops;
                if drawn > 0 && blank == 0 {
                    break;
                }
                assert!(
                    t.elapsed() < Duration::from_secs(30),
                    "round {round}: {drawn} crops drawn, {blank} blank, {} groups, grouped {:?}, grouping {:?}, \
                     rev {:?}, faces {:?}, in index {:?}",
                    app.people.groups.len(),
                    app.people.grouped,
                    app.people.grouping.as_ref().map(|g| g.0),
                    app.library.as_ref().map(|l| l.visible_rev),
                    app.people.faces.values().map(|f| f.photo.clone()).collect::<Vec<_>>(),
                    app.people.index.keys().collect::<Vec<_>>(),
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }

    /// Found in use (2026-10-28: "these cards go beyond the border and are partly cut off"): nothing the view draws
    /// reaches past its right edge, at a narrow and a wide width.
    #[test]
    fn the_cards_stay_inside_the_view() {
        let ctx = crate::test_ui::ctx();
        let mut app = crate::test_app::for_test();
        app.settings.find_faces = false;
        let d = test_dir::TempDir::new("people-fit");
        for i in 0..12 {
            std::fs::write(d.join(format!("P{i:02}.JPG")), b"x").unwrap();
        }
        app.open_folder_at(&ctx, d.path().to_path_buf());
        let cat = app.catalog.as_ref().unwrap();
        for i in 0..12 {
            // Twelve people, six faces each photo — every card full.
            let mut e = vec![0.0f32; 128];
            e[i] = 1.0;
            let faces: Vec<NewFace> = (0..6)
                .map(|k| NewFace {
                    bbox: [0.1 * k as f32, 0.1, 0.1 * k as f32 + 0.08, 0.2],
                    embedding: e.clone(),
                })
                .collect();
            cat.set_local_faces(&d.join(format!("P{i:02}.JPG")), &faces)
                .unwrap();
        }
        for width in [700.0, 1300.0] {
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 800.0));
            let t = Instant::now();
            let nodes = loop {
                let nodes = crate::test_ui::nodes_on(&ctx, screen, |ui| {
                    let c = ui.ctx().clone();
                    show(&mut app, ui, &c);
                });
                if app.people.groups.len() >= 12 {
                    break nodes;
                }
                assert!(
                    t.elapsed() < Duration::from_secs(30),
                    "the groups never landed"
                );
                std::thread::sleep(Duration::from_millis(5));
            };
            let fields = nodes
                .iter()
                .filter(|n| n.role == egui::accesskit::Role::TextInput)
                .count();
            assert!(fields >= 2, "fixture check: {fields} cards drawn");
            for n in &nodes {
                assert!(
                    n.rect.right() <= width + 0.5,
                    "{:?} reaches {} past {width}",
                    n.label,
                    n.rect.right()
                );
            }
        }
    }

    /// Found in use: a view with nothing to show said nothing. It says why.
    #[test]
    fn an_empty_view_says_why() {
        let ctx = crate::test_ui::ctx();
        let mut app = crate::test_app::for_test();
        let d = test_dir::TempDir::new("people-empty");
        std::fs::write(d.join("A.JPG"), b"x").unwrap();
        app.settings.find_faces = false;
        app.open_folder_at(&ctx, d.path().to_path_buf());
        let text = |app: &mut RustroomApp| {
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
            crate::test_ui::nodes_on(&ctx, screen, |ui| {
                let c = ui.ctx().clone();
                show(app, ui, &c);
            })
            .into_iter()
            .map(|n| n.label)
            .collect::<Vec<_>>()
        };
        assert!(
            text(&mut app)
                .iter()
                .any(|l| l.starts_with("Finding faces is off")),
            "{:?}",
            text(&mut app)
        );
        app.settings.find_faces = true;
        let rev = app.library.as_ref().map(|l| l.visible_rev).unwrap();
        app.people.looked = Some((rev, app.catalog.as_ref().unwrap().faces_revision()));
        assert!(text(&mut app)
            .iter()
            .any(|l| l == "No unnamed faces in these photos"));
    }

    #[test]
    fn missing_models_offer_the_download() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        let d = test_dir::TempDir::new("people-models");
        std::fs::write(d.join("A.JPG"), b"x").unwrap();
        let models = test_dir::TempDir::new("people-models-dir");
        app.ai.dirs = Some((models.join("cache"), models.join("models")));
        app.open_folder_at(&ctx, d.path().to_path_buf());
        app.start_face_pass();
        assert!(
            app.modal == crate::modals::Modal::AiDownload,
            "the models were not offered"
        );
        assert!(app.ai.proposed.contains(&ai_mask::ModelId::YunetFace));
    }
}
