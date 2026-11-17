//! The rail's People section (M10.18c): everyone, by name, with the photos the catalog has of them. A click
//! opens their photos — an Immich person's from the server, a local one's from the catalog.

use crate::list_row::{self, RowSpec};
use crate::RustroomApp;
use catalog::db::PersonRow;
use eframe::egui;

/// The section's body; the person clicked, if any, and whether ⌘ (Ctrl) was held — which narrows the current view
/// to them instead of opening all their photos (asked for in use, 2026-10-28).
pub(crate) fn body(app: &mut RustroomApp, ui: &mut egui::Ui) -> Option<(PersonRow, bool)> {
    let rows = std::sync::Arc::clone(&app.rail_data.people);
    if rows.is_empty() {
        crate::theme::hint(ui, "No people yet — name faces in the People view.");
        return None;
    }
    let mut clicked = None;
    for p in rows.iter() {
        let selected = app.filter.people.contains_key(&p.id)
            || app.library.as_ref().is_some_and(|l| match &l.source {
                crate::LibrarySource::Person { id, .. } => *id == p.id,
                crate::LibrarySource::ImmichPerson { person, .. } => *person == p.id,
                _ => false,
            });
        // ⚠ An Immich person's own photos are the server's, so a count of the catalog's would mislead.
        let count = (p.immich_id.is_none() || p.photos > 0).then_some(p.photos);
        let r = list_row::row(
            ui,
            RowSpec {
                name: &p.name,
                count: if p.immich_id.is_some() { None } else { count },
                meta: if p.immich_id.is_some() { "Immich" } else { "" },
                selected,
                indent: 0.0,
                hover: if cfg!(target_os = "macos") {
                    "Shows this person's photos, or with ⌘ held narrows the current view to them"
                } else {
                    "Shows this person's photos, or with Ctrl held narrows the current view to them"
                },
                actions: &[],
            },
        );
        if r.clicked {
            clicked = Some((p.clone(), ui.input(|i| i.modifiers.command)));
        }
    }
    clicked
}

impl RustroomApp {
    /// Open `p`'s photos: an Immich person's from the server, merged with the local ones (M10.18c); a local
    /// person's from the catalog, following it as a keyword view does.
    pub(crate) fn open_person(&mut self, ctx: &egui::Context, p: &PersonRow) {
        // ⚠ Their photos, in the grid: left in the People view, a click showed only their unnamed faces — usually
        // none, so it looked as if nothing happened (found in use, 2026-10-28).
        self.view = crate::AppView::Library;
        self.library_mode = crate::LibraryMode::Grid;
        if let Some(id) = &p.immich_id {
            if rustroom_app::immich_source::connection(&self.settings).is_some() {
                self.immich_opening = Some(id.clone());
                self.immich
                    .request(rustroom_app::immich_worker::Job::Person {
                        id: id.clone(),
                        name: p.name.clone(),
                    });
                self.status = format!("Fetching {}'s photos from Immich…", p.name);
                return;
            }
        }
        let source = crate::LibrarySource::Person {
            id: p.id,
            name: p.name.clone(),
        };
        let members = match self.source_members(&source) {
            Some(Ok(m)) => m,
            Some(Err(e)) => {
                self.status = format!("Could not read {}'s photos: {e}", p.name);
                return;
            }
            None => return,
        };
        let mut scan = rustroom_app::collections::scan_of(
            &members,
            rustroom_app::sidecar::edited_at,
            crate::library_state::fs_modified,
        );
        catalog::sort(&mut scan, self.settings.library_sort);
        let n = self.open_scan(ctx, source, scan);
        self.status = format!("{n} photos of {}", p.name);
    }

    /// Once a frame: the people filter's facts, re-read when the faces changed while it asks for someone, and the
    /// view refiltered with them.
    pub(crate) fn pump_people_filter(&mut self) {
        if self.filter.people.is_empty() {
            return;
        }
        let (Some(cat), Some(lib)) = (&self.catalog, self.library.as_mut()) else {
            return;
        };
        let rev = cat.faces_revision();
        if lib.people_rev == Some(rev) {
            return;
        }
        lib.people_of = cat.people_by_photo().unwrap_or_default();
        lib.people_rev = Some(rev);
        let filter = self.filter.clone();
        lib.refilter(&filter);
    }

    /// ⌘-click (Ctrl on Linux) a person in the rail: add or take them off the current view's filter.
    pub(crate) fn toggle_person_filter(&mut self, p: &PersonRow) {
        if self.filter.people.remove(&p.id).is_none() {
            self.filter.people.insert(p.id, p.name.clone());
        }
        if let Some(lib) = self.library.as_mut() {
            // ⚠ Forced re-read on the next frame, so a first person's facts are there.
            lib.people_rev = None;
        }
        self.pump_people_filter();
        let filter = self.filter.clone();
        if let Some(lib) = self.library.as_mut() {
            lib.refilter(&filter);
        }
    }

    /// Ask Immich for its people once a session, when a server is configured (M10.18c). ⚠ Once — a key
    /// without `person.read` fails every time, and a retry per frame would hammer the server.
    pub(crate) fn ask_immich_people(&mut self) {
        if self.immich_people_asked
            || rustroom_app::immich_source::connection(&self.settings).is_none()
        {
            return;
        }
        self.immich_people_asked = true;
        self.immich
            .request(rustroom_app::immich_worker::Job::People);
    }
}

#[cfg(test)]
mod tests {
    use catalog::db::NewFace;

    fn app_with_named(names: &[&str]) -> (crate::RustroomApp, test_dir::TempDir, i64) {
        let mut app = crate::test_app::for_test();
        let d = test_dir::TempDir::new("people-rail");
        for n in names {
            std::fs::write(d.join(n), b"x").unwrap();
        }
        let cat = catalog::db::Catalog::open_in_memory().unwrap();
        let scan = catalog::scan_folder(d.path()).unwrap();
        cat.sync_folder(d.path(), &scan).unwrap();
        let anna = cat.person_named("Anna").unwrap();
        for n in names {
            let p = d.join(n);
            cat.set_local_faces(
                &p,
                &[NewFace {
                    bbox: [0.1, 0.1, 0.2, 0.2],
                    embedding: vec![0.0; 128],
                }],
            )
            .unwrap();
            let id = cat.faces_of(&p).unwrap()[0].id;
            cat.set_face_states(&[(id, (Some(anna), true, false))])
                .unwrap();
        }
        app.catalog = Some(cat);
        (app, d, anna)
    }

    #[test]
    fn the_rail_lists_people_with_counts() {
        let (mut app, _d, _) = app_with_named(&["A.JPG", "B.JPG"]);
        app.rail_data.refresh(app.catalog.as_ref(), None);
        let p = &app.rail_data.people;
        assert_eq!(p.len(), 1);
        assert_eq!((p[0].name.as_str(), p[0].photos), ("Anna", 2));
    }

    #[test]
    fn a_local_person_opens_their_photos() {
        let ctx = eframe::egui::Context::default();
        let (mut app, _d, anna) = app_with_named(&["A.JPG", "B.JPG"]);
        app.rail_data.refresh(app.catalog.as_ref(), None);
        let p = app.rail_data.people[0].clone();
        app.library_mode = crate::LibraryMode::People;
        app.open_person(&ctx, &p);
        let lib = app.library.as_ref().expect("a person opened");
        assert_eq!(lib.entries.len(), 2);
        // Found in use: the click left the People view showing only unnamed faces — nothing seemed to happen.
        assert!(
            app.library_mode == crate::LibraryMode::Grid && app.view == crate::AppView::Library
        );
        assert!(matches!(&lib.source, crate::LibrarySource::Person { id, .. } if *id == anna));
    }

    /// Asked for in use: ⌘-click Anna, then Ben, and the open folder shows only the photos of the two together;
    /// ⌘-click Ben again and Anna's are back. A plain click still opens all of a person's photos.
    #[test]
    fn command_click_narrows_the_open_folder_to_everyone_ticked() {
        let ctx = eframe::egui::Context::default();
        let mut app = crate::test_app::for_test();
        let d = test_dir::TempDir::new("people-filter");
        for n in ["A.JPG", "B.JPG", "C.JPG"] {
            std::fs::write(d.join(n), b"x").unwrap();
        }
        app.open_folder_at(&ctx, d.path().to_path_buf());
        let cat = app.catalog.as_ref().unwrap();
        let (anna, ben) = (
            cat.person_named("Anna").unwrap(),
            cat.person_named("Ben").unwrap(),
        );
        let name = |photo: &str, people: &[i64]| {
            let p = d.join(photo);
            let faces: Vec<NewFace> = people
                .iter()
                .map(|_| NewFace {
                    bbox: [0.1, 0.1, 0.2, 0.2],
                    embedding: vec![0.0; 128],
                })
                .collect();
            cat.set_local_faces(&p, &faces).unwrap();
            let ids: Vec<i64> = cat.faces_of(&p).unwrap().iter().map(|f| f.id).collect();
            let states: Vec<_> = ids
                .iter()
                .zip(people)
                .map(|(f, who)| (*f, (Some(*who), true, false)))
                .collect();
            cat.set_face_states(&states).unwrap();
        };
        name("A.JPG", &[anna]);
        name("B.JPG", &[anna, ben]);
        name("C.JPG", &[ben]);
        app.rail_data.refresh(app.catalog.as_ref(), None);
        let row = |id: i64| {
            app.rail_data
                .people
                .iter()
                .find(|p| p.id == id)
                .unwrap()
                .clone()
        };
        let (a, b) = (row(anna), row(ben));
        let shown = |app: &crate::RustroomApp| {
            let lib = app.library.as_ref().unwrap();
            let mut v: Vec<String> = lib
                .visible
                .iter()
                .map(|&i| lib.entries[i].file_name.clone())
                .collect();
            v.sort();
            v
        };
        assert_eq!(shown(&app).len(), 3, "fixture check");
        app.toggle_person_filter(&a);
        assert_eq!(shown(&app), ["A.JPG", "B.JPG"]);
        app.toggle_person_filter(&b);
        assert_eq!(shown(&app), ["B.JPG"], "Anna and Ben together");
        app.toggle_person_filter(&b);
        assert_eq!(shown(&app), ["A.JPG", "B.JPG"]);
        assert!(
            matches!(
                app.library.as_ref().unwrap().source,
                crate::LibrarySource::Folder(_)
            ),
            "it left the folder"
        );
    }
}
