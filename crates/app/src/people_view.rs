//! Library → People (M10.18c): who is in the photos, and the faces still to name — the mockup's layout A
//! (`https://claude.ai/artifact/8veXQc9qb5x7ivD4GnM5UW`). Named people across the top; below, "Who is this?":
//! the shown photos' unnamed faces grouped by likeness, each group named at once or answered Yes / No.
//!
//! ⚠ The search runs in the background while the view is open ([`RustroomApp::pump_face_pass`], pumped every
//! frame from the shell), on local photos only — an Immich photo has Immich's faces. Face crops are cut from the
//! grid's thumbnails, so only what the Library shows is drawn, and only the faces in view ask for one.

use crate::theme::Hover as _;
use crate::{theme, RustroomApp};
use catalog::db::FaceRow;
use eframe::egui;
use rustroom_app::face_groups::{self, Group, GroupMemo};
use rustroom_app::face_pass::{FaceFinder, FacePass};
use rustroom_app::photo_id::PhotoId;
use std::collections::HashMap;
use std::sync::Arc;

/// A face crop's edge in the groups.
const FACE: f32 = 56.0;
/// How many faces a group shows.
const SHOWN: usize = 5;
/// A group card's width.
const CARD: f32 = 220.0;

/// The People view's state.
#[derive(Default)]
pub(crate) struct PeopleView {
    groups: GroupMemo,
    /// The faces the groups were made from, by id — read with them.
    faces: Arc<HashMap<i64, FaceRow>>,
    /// Each group's name field, by its first face.
    names: HashMap<i64, String>,
    pass: Option<FacePass>,
    /// The view the running or last pass was started for (`Library::visible_rev`).
    pass_for: Option<u64>,
    /// Tests' finder — never the networks. `None`: the real one.
    pub(crate) finder: Option<FaceFinder>,
    /// The models were offered this session.
    offered: bool,
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
    fn start_face_pass(&mut self) {
        let Some(lib) = &self.library else { return };
        if !self.settings.find_faces
            || self.people.pass.is_some()
            || self.people.pass_for == Some(lib.visible_rev)
        {
            return;
        }
        self.people.pass_for = Some(lib.visible_rev);
        let root = self.immich_cache_root();
        let local: Vec<std::path::PathBuf> = lib
            .visible
            .iter()
            .filter_map(|&i| lib.entries.get(i))
            .map(|e| e.id.path.clone())
            .filter(|p| {
                !root
                    .as_deref()
                    .is_some_and(|r| rustroom_app::immich_source::is_immich_path(r, p))
            })
            .collect();
        let mut local = local;
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
            None => {
                let models = self.face_models();
                let missing: Vec<ai_mask::ModelId> =
                    [ai_mask::ModelId::YunetFace, ai_mask::ModelId::SfaceFace]
                        .into_iter()
                        .filter(|m| {
                            !models
                                .as_deref()
                                .is_some_and(|d| ai_mask::download::present(&m.spec(), d))
                        })
                        .collect();
                if !missing.is_empty() {
                    // ⚠ Offered once a session; the view looks again when it next changes.
                    if !self.people.offered {
                        self.people.offered = true;
                        self.ai.proposed = missing;
                        self.modal = crate::modals::Modal::AiDownload;
                    }
                    self.people.pass_for = None;
                    return;
                }
                rustroom_app::face_pass::real_finder(models.expect("checked above"))
            }
        };
        self.people.pass = Some(FacePass::start(todo, finder));
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
            self.people.pass = None;
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(150));
        }
    }

    /// The status row's line while the search runs.
    pub(crate) fn face_pass_line(&self) -> Option<String> {
        let (done, total) = self.people.pass.as_ref()?.progress();
        Some(format!("Looking for faces · {done} of {total} photos"))
    }

    pub(crate) fn cancel_face_pass(&mut self) {
        if let Some(p) = self.people.pass.take() {
            p.cancel();
        }
    }
}

/// The shown photos' unnamed faces, grouped — memoised on the catalog and the view.
fn groups(app: &mut RustroomApp) -> (Arc<Vec<Group>>, Arc<HashMap<i64, FaceRow>>) {
    let (Some(cat), Some(lib)) = (&app.catalog, &app.library) else {
        return (Arc::default(), Arc::default());
    };
    let key = cat.instance() ^ cat.change_count().rotate_left(20) ^ lib.visible_rev.rotate_left(40);
    let shown: std::collections::HashSet<&std::path::Path> = lib
        .visible
        .iter()
        .filter_map(|&i| lib.entries.get(i).map(|e| e.id.path.as_path()))
        .collect();
    let mut faces = None;
    let groups = app.people.groups.get(key, || {
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
        let input: Vec<(i64, Vec<f32>)> = unnamed
            .iter()
            .filter_map(|f| Some((f.id, f.embedding.clone()?)))
            .collect();
        let rejected = cat.face_rejections().unwrap_or_default();
        faces = Some(Arc::new(
            unnamed
                .into_iter()
                .map(|f| (f.id, f))
                .collect::<HashMap<_, _>>(),
        ));
        face_groups::group(&input, &named, &rejected, face_groups::THRESHOLD)
    });
    if let Some(f) = faces {
        app.people.faces = f;
    }
    (groups, app.people.faces.clone())
}

pub fn show(app: &mut RustroomApp, ui: &mut egui::Ui, _ctx: &egui::Context) {
    app.ask_immich_people();
    app.start_face_pass();
    let (groups, faces) = groups(app);
    let people = app.rail_data.people.clone();
    let mut act: Option<Act> = None;
    egui::CentralPanel::default().show(ui, |ui| {
        if app.library.is_none() {
            ui.centered_and_justified(|ui| {
                theme::empty_state(ui, "Open a folder to see who is in its photos.");
            });
            return;
        }
        let live = app.live_edits();
        let names = &mut app.people.names;
        let Some(lib) = app.library.as_mut() else {
            return;
        };
        let index: HashMap<&std::path::Path, usize> = lib
            .entries
            .iter()
            .enumerate()
            .map(|(i, e)| (e.id.path.as_path(), i))
            .collect();
        let mut window = Vec::new();
        egui::ScrollArea::vertical().show(ui, |ui| {
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
                theme::hint(ui, "Every face found in these photos has a name.");
            }
            ui.horizontal_wrapped(|ui| {
                for g in groups.iter() {
                    let first = g.faces[0];
                    egui::Frame::group(ui.style()).show(ui, |ui| {
                        ui.set_width(CARD);
                        ui.horizontal(|ui| {
                            for f in g.faces.iter().take(SHOWN).filter_map(|id| faces.get(id)) {
                                let (r, resp) = ui.allocate_exact_size(
                                    egui::Vec2::splat(FACE),
                                    egui::Sense::click(),
                                );
                                if let Some(&i) = index.get(f.photo.as_path()) {
                                    let req = lib.thumb_request(i, live.as_ref());
                                    let tex = lib
                                        .cache
                                        .get(&req.key())
                                        .or_else(|| lib.cache.get_stale(&lib.entries[i].id));
                                    window.push(req);
                                    match tex {
                                        Some(t) => {
                                            let uv = egui::Rect::from_min_max(
                                                egui::pos2(f.bbox[0], f.bbox[1]),
                                                egui::pos2(f.bbox[2], f.bbox[3]),
                                            );
                                            ui.painter().image(t.id(), r, uv, egui::Color32::WHITE);
                                        }
                                        None => {
                                            ui.painter().rect_filled(
                                                r,
                                                FACE / 2.0,
                                                theme::tokens::of(ui).card,
                                            );
                                        }
                                    }
                                }
                                let resp = resp
                                    .hover("Right-click for this face's photo, or to remove it");
                                let id = f.id;
                                let photo = f.photo.clone();
                                resp.context_menu(|ui| {
                                    if ui.button("Open photo").clicked() {
                                        act = Some(Act::Open(PhotoId::original(photo.clone())));
                                        ui.close();
                                    }
                                    if ui.button("Not a face").clicked() {
                                        act = Some(Act::Dismiss(vec![id]));
                                        ui.close();
                                    }
                                });
                            }
                        });
                        let count = rustroom_app::status_text::photos(g.faces.len())
                            .replace("photo", "face");
                        let suggest = g.suggest.and_then(|s| people.iter().find(|p| p.id == s));
                        match suggest {
                            Some(p) => {
                                ui.label(format!("{}? · {count}", p.name));
                                ui.horizontal(|ui| {
                                    if ui
                                        .button("Yes")
                                        .hover("Names these faces after this person")
                                        .clicked()
                                    {
                                        act = Some(Act::Yes(g.faces.clone(), p.id));
                                    }
                                    if ui
                                        .button("No")
                                        .hover("Never offers these faces as this person again")
                                        .clicked()
                                    {
                                        act = Some(Act::No(g.faces.clone(), p.id));
                                    }
                                });
                            }
                            None => {
                                theme::hint(ui, &count);
                            }
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
                            act = Some(Act::Name(g.faces.clone(), std::mem::take(field)));
                        }
                    });
                }
            });
        });
        // ⚠⚠ Everything drawn, or `retain_window` leaves these thumbnails evictable (Survey's rule).
        lib.sync_thumb_window(&window);
    });
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
                app.open_entry(_ctx, i);
            }
        }
        Some(Act::Person(p)) => app.open_person(_ctx, &p),
        None => {}
    }
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
