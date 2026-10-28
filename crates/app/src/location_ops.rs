//! Placing photos on the Map by hand (M10.18b): a drop or the location field sets the selection's
//! location, Remove location hides it, and each is one undo step. ⚠ Catalog only — never a file or a
//! sidecar (the author's call, 2026-10-28).

use crate::RustroomApp;
use catalog::db::Hand;
use rustroom_app::action_log::ActionKind;
use rustroom_app::photo_id::PhotoId;
use std::path::PathBuf;

impl RustroomApp {
    /// Put `ids` at `at` — one undo step. ⚠ By FILE: a virtual copy places its original too.
    pub(crate) fn place_at(&mut self, ids: &[PhotoId], at: (f64, f64)) {
        self.set_hand_for(ids, Hand::At(at.0, at.1));
    }

    /// Hide `ids`' location, whatever the file or Immich says — one undo step.
    pub(crate) fn remove_location(&mut self, ids: &[PhotoId]) {
        self.set_hand_for(ids, Hand::Removed);
    }

    fn set_hand_for(&mut self, ids: &[PhotoId], hand: Hand) {
        let Some(cat) = &self.catalog else { return };
        let mut files: Vec<PathBuf> = ids.iter().map(|i| i.path.clone()).collect();
        files.sort();
        files.dedup();
        let changes: Vec<(PathBuf, Hand, Hand)> = files
            .iter()
            .filter_map(|p| {
                let before = cat.hand_location(p).ok()?;
                (before != hand).then(|| (p.clone(), before, hand))
            })
            .collect();
        if changes.is_empty() {
            return;
        }
        let touched: Vec<PathBuf> = changes.iter().map(|c| c.0.clone()).collect();
        if let Err(e) = cat.set_hand(&touched, hand) {
            self.status = format!("Could not set the location: {e}");
            return;
        }
        let n = touched.len();
        let open = self.open_photo();
        self.actions
            .push(ActionKind::LocationChange { changes }, open.as_ref());
        let photos = rustroom_app::status_text::photos(n);
        self.status = match hand {
            Hand::Removed => format!("Removed the location of {photos}"),
            _ => format!("Placed {photos}"),
        };
    }

    /// Undo (`forward: false`) or redo an [`ActionKind::LocationChange`].
    pub(crate) fn replay_location_change(&mut self, kind: &ActionKind, forward: bool) {
        let ActionKind::LocationChange { changes } = kind else {
            return;
        };
        if let Some(cat) = &self.catalog {
            for (p, before, after) in changes {
                let h = if forward { *after } else { *before };
                if let Err(e) = cat.set_hand(std::slice::from_ref(p), h) {
                    eprintln!("catalog: {e}");
                }
            }
        }
        self.status = if forward { "Redone" } else { "Undone" }.to_string();
    }
}

#[cfg(test)]
mod tests {
    use catalog::db::Hand;
    use rustroom_app::photo_id::PhotoId;
    use std::path::{Path, PathBuf};

    /// A catalogued folder of `names` on disk.
    fn app_with(
        tag: &str,
        names: &[&str],
    ) -> (crate::RustroomApp, test_dir::TempDir, Vec<PathBuf>) {
        let mut app = crate::test_app::for_test();
        let d = test_dir::TempDir::new(tag);
        let paths: Vec<PathBuf> = names.iter().map(|n| d.join(n)).collect();
        for p in &paths {
            std::fs::write(p, b"x").unwrap();
        }
        app.catalog = Some(catalog::db::Catalog::open_in_memory().unwrap());
        let scan = catalog::scan_folder(d.path()).unwrap();
        app.catalog
            .as_ref()
            .unwrap()
            .sync_folder(d.path(), &scan)
            .unwrap();
        (app, d, paths)
    }

    fn ids(p: &[PathBuf]) -> Vec<PhotoId> {
        p.iter().cloned().map(PhotoId::original).collect()
    }

    fn at(app: &crate::RustroomApp, p: &Path) -> Option<(f64, f64)> {
        app.catalog.as_ref().unwrap().location_of(p).unwrap()
    }

    #[test]
    fn dropping_three_photos_places_them_in_one_undo_step() {
        let (mut app, d, p) = app_with("place-three", &["A.RAF", "B.RAF", "C.RAF"]);
        app.place_at(&ids(&p), (38.80, -9.38));
        assert!(p.iter().all(|x| at(&app, x) == Some((38.80, -9.38))));
        assert_eq!(app.status, "Placed 3 photos");
        app.undo_action(&eframe::egui::Context::default());
        assert!(
            p.iter().all(|x| at(&app, x).is_none()),
            "one undo restores all three"
        );
        app.redo_action(&eframe::egui::Context::default());
        assert!(p.iter().all(|x| at(&app, x) == Some((38.80, -9.38))));
        let xmp = std::fs::read_dir(d.path())
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .path()
                    .extension()
                    .is_some_and(|x| x == "xmp")
            })
            .count();
        assert_eq!(xmp, 0, "a sidecar was written");
    }

    /// Review focus 2: removing hides the file's own position; Undo brings it back.
    #[test]
    fn remove_then_undo_restores_the_files_location() {
        let (mut app, _d, p) = app_with("remove-undo", &["A.RAF"]);
        let cat = app.catalog.as_ref().unwrap();
        cat.set_file_location(&p[0], Some((38.7, -9.1))).unwrap();
        app.remove_location(&ids(&p));
        assert_eq!(at(&app, &p[0]), None);
        app.undo_action(&eframe::egui::Context::default());
        assert_eq!(at(&app, &p[0]), Some((38.7, -9.1)));
        assert_eq!(
            app.catalog.as_ref().unwrap().hand_location(&p[0]).unwrap(),
            Hand::Unset
        );
    }

    /// Copies share the file's location — the row is keyed by file, and one change is recorded once.
    #[test]
    fn a_virtual_copy_places_its_file() {
        let (mut app, _d, p) = app_with("place-copy", &["A.RAF"]);
        let both = [
            PhotoId::original(p[0].clone()),
            PhotoId::new(p[0].clone(), 1),
        ];
        app.place_at(&both, (1.0, 2.0));
        assert_eq!(at(&app, &p[0]), Some((1.0, 2.0)));
        assert_eq!(app.status, "Placed 1 photo");
    }
}
