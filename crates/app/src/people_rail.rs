//! The rail's People section (M10.18c): everyone, by name, with the photos the catalog has of them. A click
//! opens their photos — an Immich person's from the server, a local one's from the catalog.

use crate::list_row::{self, RowSpec};
use crate::RustroomApp;
use catalog::db::PersonRow;
use eframe::egui;

/// The section's body; the person clicked, if any.
pub(crate) fn body(app: &mut RustroomApp, ui: &mut egui::Ui) -> Option<PersonRow> {
    let rows = std::sync::Arc::clone(&app.rail_data.people);
    if rows.is_empty() {
        crate::theme::hint(ui, "No people yet — name faces in the People view.");
        return None;
    }
    let mut clicked = None;
    for p in rows.iter() {
        let selected = app.library.as_ref().is_some_and(|l| match &l.source {
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
                hover: "Shows the photos this person is in",
                actions: &[],
            },
        );
        if r.clicked {
            clicked = Some(p.clone());
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
}
