//! Naming faces in the People view (M10.18c): a group named at once, a suggestion answered Yes or No, a face
//! cleared or dismissed as "not a face" — each one undo step, catalog only.

use crate::RustroomApp;
use catalog::db::FaceState;
use rustroom_app::action_log::ActionKind;

impl RustroomApp {
    /// Write `after(before)` to every face in `faces`, and the "No"s in `reject`, as one undo step.
    fn change_faces(
        &mut self,
        faces: &[i64],
        after: impl Fn(FaceState) -> FaceState,
        reject: Option<i64>,
        status: String,
    ) {
        let Some(cat) = &self.catalog else { return };
        let Ok(before) = cat.face_states(faces) else {
            return;
        };
        let changes: Vec<(i64, FaceState, FaceState)> = before
            .into_iter()
            .map(|(id, b)| (id, b, after(b)))
            .filter(|(_, b, a)| b != a)
            .collect();
        let rejections: Vec<(i64, i64)> =
            reject.map_or_else(Vec::new, |p| faces.iter().map(|f| (*f, p)).collect());
        if changes.is_empty() && rejections.is_empty() {
            return;
        }
        let writes: Vec<(i64, FaceState)> = changes.iter().map(|(id, _, a)| (*id, *a)).collect();
        if let Err(e) = cat.set_face_states(&writes) {
            self.status = format!("Could not change the faces: {e}");
            return;
        }
        if let Some(p) = reject {
            let _ = cat.set_rejected(faces, p, true);
        }
        let open = self.open_photo();
        self.actions.push(
            ActionKind::FaceChange {
                changes,
                rejections,
                apart: Vec::new(),
            },
            open.as_ref(),
        );
        self.status = status;
    }

    /// Name every face in a group `name` — the person of that name, made if there is none.
    pub(crate) fn name_faces(&mut self, faces: &[i64], name: &str) {
        let name = name.trim();
        let Some(person) = self
            .catalog
            .as_ref()
            .and_then(|c| c.person_named(name).ok())
        else {
            return;
        };
        let n = rustroom_app::status_text::photos(faces.len()).replace("photo", "face");
        self.change_faces(
            faces,
            |(_, _, d)| (Some(person), true, d),
            None,
            format!("Named {n} as {name}"),
        );
    }

    /// "Yes, that is them": a suggested group named its person.
    pub(crate) fn confirm_faces(&mut self, faces: &[i64], person: i64) {
        self.change_faces(
            faces,
            |(_, _, d)| (Some(person), true, d),
            None,
            "Named".into(),
        );
    }

    /// "No": never offered as `person` again.
    pub(crate) fn reject_faces(&mut self, faces: &[i64], person: i64) {
        self.change_faces(faces, |s| s, Some(person), "Will not ask again".into());
    }

    /// "Not in this group": `face` kept apart from every other face of its group — it stands alone now, and no
    /// later grouping puts it back with them. One undo step.
    pub(crate) fn keep_apart(&mut self, face: i64, group: &[i64]) {
        let Some(cat) = &self.catalog else { return };
        let pairs: Vec<(i64, i64)> = group
            .iter()
            .filter(|f| **f != face)
            .map(|f| (face, *f))
            .collect();
        if pairs.is_empty() {
            return;
        }
        if let Err(e) = cat.set_apart(&pairs, true) {
            self.status = format!("Could not change the faces: {e}");
            return;
        }
        let open = self.open_photo();
        self.actions.push(
            ActionKind::FaceChange {
                changes: Vec::new(),
                rejections: Vec::new(),
                apart: pairs,
            },
            open.as_ref(),
        );
        self.status = "Took the face out of its group".into();
    }

    /// "Not a face": left out of every list, kept so the undo is a flip.
    pub(crate) fn dismiss_faces(&mut self, faces: &[i64]) {
        self.change_faces(faces, |(p, c, _)| (p, c, true), None, "Removed".into());
    }

    /// Undo (`forward: false`) or redo an [`ActionKind::FaceChange`].
    pub(crate) fn replay_face_change(&mut self, kind: &ActionKind, forward: bool) {
        let ActionKind::FaceChange {
            changes,
            rejections,
            apart,
        } = kind
        else {
            return;
        };
        if let Some(cat) = &self.catalog {
            let writes: Vec<(i64, FaceState)> = changes
                .iter()
                .map(|(id, b, a)| (*id, if forward { *a } else { *b }))
                .collect();
            let _ = cat.set_face_states(&writes);
            let _ = cat.set_apart(apart, forward);
            for (f, p) in rejections {
                let _ = cat.set_rejected(std::slice::from_ref(f), *p, forward);
            }
        }
        self.status = if forward { "Redone" } else { "Undone" }.to_string();
    }
}

#[cfg(test)]
mod tests {
    use catalog::db::NewFace;

    /// Three photos with one unnamed face each.
    fn app_with_faces() -> (crate::RustroomApp, test_dir::TempDir, Vec<i64>) {
        let mut app = crate::test_app::for_test();
        let d = test_dir::TempDir::new("face-ops");
        let cat = catalog::db::Catalog::open_in_memory().unwrap();
        for n in ["A.JPG", "B.JPG", "C.JPG"] {
            std::fs::write(d.join(n), b"x").unwrap();
        }
        let scan = catalog::scan_folder(d.path()).unwrap();
        cat.sync_folder(d.path(), &scan).unwrap();
        let mut ids = Vec::new();
        for n in ["A.JPG", "B.JPG", "C.JPG"] {
            let p = d.join(n);
            cat.set_local_faces(
                &p,
                &[NewFace {
                    bbox: [0.1, 0.1, 0.2, 0.2],
                    embedding: vec![0.1; 128],
                }],
            )
            .unwrap();
            ids.push(cat.faces_of(&p).unwrap()[0].id);
        }
        app.catalog = Some(cat);
        (app, d, ids)
    }

    fn ctx() -> eframe::egui::Context {
        eframe::egui::Context::default()
    }

    #[test]
    fn naming_a_group_of_three_is_one_undo_step_and_shows_in_the_rail() {
        let (mut app, _d, ids) = app_with_faces();
        app.name_faces(&ids, "Anna");
        app.rail_data.refresh(app.catalog.as_ref(), None);
        assert_eq!(app.rail_data.people[0].name, "Anna");
        assert_eq!(app.rail_data.people[0].photos, 3);
        assert_eq!(app.status, "Named 3 faces as Anna");
        app.undo_action(&ctx());
        assert!(
            app.catalog.as_ref().unwrap().people().unwrap().is_empty(),
            "one undo unnamed all three"
        );
        app.redo_action(&ctx());
        assert_eq!(app.catalog.as_ref().unwrap().people().unwrap()[0].photos, 3);
    }

    #[test]
    fn yes_names_the_group_and_no_is_final() {
        let (mut app, _d, ids) = app_with_faces();
        let ben = app.catalog.as_ref().unwrap().person_named("Ben").unwrap();
        app.reject_faces(&ids[..2], ben);
        let rejected = app.catalog.as_ref().unwrap().face_rejections().unwrap();
        assert!(rejected.contains(&(ids[0], ben)) && rejected.contains(&(ids[1], ben)));
        app.undo_action(&ctx());
        assert!(app
            .catalog
            .as_ref()
            .unwrap()
            .face_rejections()
            .unwrap()
            .is_empty());
        app.confirm_faces(&ids[2..], ben);
        assert_eq!(
            app.catalog
                .as_ref()
                .unwrap()
                .person_members(ben)
                .unwrap()
                .len(),
            1
        );
    }

    /// Asked for in use: one face out of its group, one undo step.
    #[test]
    fn not_in_this_group_and_its_undo() {
        let (mut app, _d, ids) = app_with_faces();
        app.keep_apart(ids[0], &ids);
        let apart = app.catalog.as_ref().unwrap().faces_apart().unwrap();
        assert_eq!(apart.len(), 2, "{apart:?}");
        app.undo_action(&ctx());
        assert!(app
            .catalog
            .as_ref()
            .unwrap()
            .faces_apart()
            .unwrap()
            .is_empty());
        app.redo_action(&ctx());
        assert_eq!(
            app.catalog.as_ref().unwrap().faces_apart().unwrap().len(),
            2
        );
    }

    #[test]
    fn not_a_face_and_its_undo() {
        let (mut app, d, ids) = app_with_faces();
        app.dismiss_faces(&ids[..1]);
        assert!(app
            .catalog
            .as_ref()
            .unwrap()
            .faces_of(&d.join("A.JPG"))
            .unwrap()
            .is_empty());
        app.undo_action(&ctx());
        assert_eq!(
            app.catalog
                .as_ref()
                .unwrap()
                .faces_of(&d.join("A.JPG"))
                .unwrap()
                .len(),
            1
        );
    }
}
