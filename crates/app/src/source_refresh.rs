//! A catalog-backed view follows the catalog: a photo untagged in a keyword's view, rated out of a
//! smart collection or taken out of a collection leaves it at once, and one tagged or rated in joins
//! it — the author's report, 2026-10-01: *"it stays until the view is updated by e.g. switching to a
//! different folder"*.
//!
//! ⚠⚠ **Driven by the catalog's change count, not by a list of the places that write.** Every write,
//! from any route present or future, moves it — the rule `keyword_suggest::Cache` already follows —
//! so no route can be the one that forgets to refresh. One pump at the end of the frame re-reads the
//! source's members when the count has moved and settled, and rebuilds the rows only when the member
//! files actually differ.
//!
//! ⚠ Only the sources that ARE a catalog query: keywords, collections, smart collections, dates. A
//! folder is its directory (a click on its row re-scans it); All Photos changes only when a folder is
//! opened, forgotten or imported, and re-reading every photo on each rating would be paid by every
//! keystroke of a cull; an Immich view is the server's.

use crate::library_state::fs_modified;
use crate::{LibrarySource, RustroomApp};
use eframe::egui;
use rustroom_app::sidecar;
use std::path::PathBuf;

impl RustroomApp {
    /// What `source` holds now, as (file, copy) members — `None` without a catalog, or for a source
    /// that is not a catalog query.
    ///
    /// ⚠⚠ The one definition: opening a source and refreshing it both read it here, so the view you
    /// open and the view it refreshes into cannot disagree.
    pub(crate) fn source_members(
        &self,
        source: &LibrarySource,
    ) -> Option<Result<Vec<(PathBuf, u32)>, String>> {
        let cat = self.catalog.as_ref()?;
        let read = match source {
            LibrarySource::Keyword { id, .. } => cat.keyword_members(*id),
            LibrarySource::Collection { id, .. } => cat.collection_members(*id),
            LibrarySource::Periods { periods, .. } => cat.photos_in_periods(periods),
            LibrarySource::Smart { id, .. } => {
                return Some(self.smart_members(*id).map(|found| found.members));
            }
            LibrarySource::Folder(_)
            | LibrarySource::AllPhotos
            | LibrarySource::ImmichAlbum { .. }
            | LibrarySource::ImmichBucket { .. } => return None,
        };
        Some(read.map_err(|e| e.to_string()))
    }

    /// Once a frame, after everything that can write: re-read the source when the catalog has
    /// changed since its members were read and the change has settled.
    ///
    /// ⚠ Settled means the same count on two frames running. A metadata scan writes a capture on
    /// every frame while it runs, and a smart collection re-evaluated per write would cost ~60 ms a
    /// frame at 10,000 photos (`open_smart`'s measurement); a single tagging still lands one frame on.
    pub(crate) fn pump_source_membership(&mut self, ctx: &egui::Context) {
        let Some(now) = self.catalog.as_ref().map(|c| c.change_count()) else {
            return;
        };
        let Some(lib) = self.library.as_mut() else {
            return;
        };
        if lib.members_read_at == now {
            return;
        }
        if lib.members_settling != Some(now) {
            lib.members_settling = Some(now);
            // ⚠ egui paints on input only: without this the second frame waits for the mouse.
            ctx.request_repaint();
            return;
        }
        lib.members_settling = None;
        lib.members_read_at = now;
        if self.refresh_source() {
            ctx.request_repaint();
        }
    }

    /// Re-read the source's members and, when the member files changed, rebuild the rows. Returns
    /// whether they changed.
    ///
    /// ⚠ The selection follows `Selection::retain_visible`'s rule: the photos still here stay
    /// selected, and if the active one left, nothing is — moving it silently would change which photo
    /// a sync copies from. ⚠ Never a re-open: `open_scan` switches to the Library, and a photo
    /// untagged from Develop's filmstrip must not throw you out of Develop.
    pub(crate) fn refresh_source(&mut self) -> bool {
        let Some(source) = self.library.as_ref().map(|l| l.source.clone()) else {
            return false;
        };
        // ⚠ A read that fails keeps the rows on screen rather than emptying the view.
        let Some(Ok(members)) = self.source_members(&source) else {
            return false;
        };
        let Some(lib) = self.library.as_mut() else {
            return false;
        };
        let Some(scan) = rustroom_app::collections::refreshed_scan(
            &lib.scan,
            &members,
            sidecar::edited_at,
            fs_modified,
        ) else {
            return false;
        };
        lib.scan = scan;
        // Re-sorts, rebuilds the rows, keeps the selection by id, refilters, re-reads the marks.
        self.rebuild_entries(None);
        // A photo that joined from a folder no other row is in needs its captures, and maybe a scan.
        self.reload_captures();
        self.queue_metadata();
        true
    }
}

#[cfg(test)]
mod tests {
    use crate::test_app;
    use crate::AppView;
    use std::path::PathBuf;

    /// An app whose catalog knows `n` photos, and their paths.
    fn app_of(n: usize) -> (crate::RustroomApp, Vec<PathBuf>) {
        let mut app = test_app::for_test();
        app.library = Some(test_app::library_of(n));
        test_app::catalog_library(&mut app);
        let paths = app
            .library
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .map(|e| e.id.path.clone())
            .collect();
        (app, paths)
    }

    fn shown(app: &crate::RustroomApp) -> Vec<PathBuf> {
        app.library
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .map(|e| e.id.path.clone())
            .collect()
    }

    fn selected(app: &crate::RustroomApp) -> Vec<PathBuf> {
        let lib = app.library.as_ref().unwrap();
        lib.selected_ids().into_iter().map(|id| id.path).collect()
    }

    /// Two frames: the first sees the change, the second finds it settled and re-reads.
    fn settle(app: &mut crate::RustroomApp, ctx: &egui::Context) {
        app.pump_source_membership(ctx);
        app.pump_source_membership(ctx);
    }

    /// A view of `Paris`, tagged on all three photos, with the first and the last selected.
    fn paris_view() -> (crate::RustroomApp, Vec<PathBuf>, i64, egui::Context) {
        let (mut app, p) = app_of(3);
        let cat = app.catalog.as_ref().unwrap();
        let paris = cat.create_keyword("Paris", None).unwrap();
        for f in &p {
            cat.assign_keyword(paris, f).unwrap();
        }
        let ctx = egui::Context::default();
        app.open_keyword(&ctx, paris, "Paris");
        let lib = app.library.as_mut().unwrap();
        lib.selection.click(0);
        lib.selection.toggle(2);
        (app, p, paris, ctx)
    }

    /// ⚠⚠ The report: untagging a photo in a keyword's view takes it out of the view — once the
    /// change settles, and not before (the write alone changes nothing on screen, which is the bug).
    #[test]
    fn an_untagged_photo_leaves_the_keywords_view() {
        let (mut app, p, paris, ctx) = paris_view();
        assert_eq!(shown(&app), p, "the sample: all three are shown");
        app.tag_files(paris, false, std::slice::from_ref(&p[1]));
        assert_eq!(shown(&app), p, "the write alone rebuilds nothing");
        app.pump_source_membership(&ctx);
        assert_eq!(shown(&app), p, "the first frame only notices");
        app.pump_source_membership(&ctx);
        assert_eq!(shown(&app), vec![p[0].clone(), p[2].clone()]);
        assert_eq!(
            selected(&app),
            vec![p[0].clone(), p[2].clone()],
            "both stay selected"
        );
    }

    #[test]
    fn a_tagged_photo_joins_the_keywords_view() {
        let (mut app, p) = app_of(3);
        let cat = app.catalog.as_ref().unwrap();
        let paris = cat.create_keyword("Paris", None).unwrap();
        cat.assign_keyword(paris, &p[0]).unwrap();
        let ctx = egui::Context::default();
        app.open_keyword(&ctx, paris, "Paris");
        assert_eq!(shown(&app), vec![p[0].clone()]);
        app.tag_files(paris, true, std::slice::from_ref(&p[2]));
        settle(&mut app, &ctx);
        assert_eq!(shown(&app), vec![p[0].clone(), p[2].clone()]);
    }

    /// The active photo left: nothing is selected — `retain_visible`'s rule, not a silent move.
    #[test]
    fn when_the_active_photo_leaves_nothing_is_selected() {
        let (mut app, p, paris, ctx) = paris_view();
        app.tag_files(paris, false, std::slice::from_ref(&p[0]));
        settle(&mut app, &ctx);
        assert_eq!(shown(&app), vec![p[1].clone(), p[2].clone()]);
        assert!(selected(&app).is_empty(), "{:?}", selected(&app));
    }

    /// ⚠ Never a re-open: `open_scan` switches to the Library.
    #[test]
    fn a_refresh_leaves_develop_in_develop() {
        let (mut app, p, paris, ctx) = paris_view();
        app.view = AppView::Develop;
        app.tag_files(paris, false, std::slice::from_ref(&p[1]));
        settle(&mut app, &ctx);
        assert_eq!(shown(&app).len(), 2, "the sample: it did refresh");
        assert!(app.view == AppView::Develop, "a refresh switched the view");
    }

    #[test]
    fn a_collection_follows_what_is_added_and_removed() {
        let (mut app, p) = app_of(3);
        let cat = app.catalog.as_ref().unwrap();
        let trips = cat.create_collection("Trips").unwrap();
        cat.add_to_collection(trips, &p[0], 0).unwrap();
        let ctx = egui::Context::default();
        app.open_collection(&ctx, trips, "Trips");
        assert_eq!(shown(&app), vec![p[0].clone()]);
        let cat = app.catalog.as_ref().unwrap();
        cat.add_to_collection(trips, &p[2], 0).unwrap();
        settle(&mut app, &ctx);
        assert_eq!(shown(&app), vec![p[0].clone(), p[2].clone()]);
        let cat = app.catalog.as_ref().unwrap();
        cat.remove_from_collection(trips, &p[0], 0).unwrap();
        settle(&mut app, &ctx);
        assert_eq!(shown(&app), vec![p[2].clone()]);
    }

    /// A smart collection of three stars and up: rating a photo in or out moves it.
    #[test]
    fn a_smart_collection_follows_a_rating() {
        use rustroom_app::rule::{Cmp, Criterion, Expr};
        let (mut app, p) = app_of(3);
        let cat = app.catalog.as_ref().unwrap();
        let rule = Expr::Is(Criterion::Rating {
            op: Cmp::AtLeast,
            value: 3,
        });
        let good = cat
            .create_smart("Good", &serde_json::to_string(&rule).unwrap())
            .unwrap();
        let rate = |app: &crate::RustroomApp, f: &PathBuf, rating: u8| {
            let marks = catalog::db::Marks {
                rating,
                ..Default::default()
            };
            app.catalog
                .as_ref()
                .unwrap()
                .set_marks(f, 0, marks)
                .unwrap();
        };
        rate(&app, &p[0], 4);
        let ctx = egui::Context::default();
        app.open_smart(&ctx, good, "Good");
        assert_eq!(shown(&app), vec![p[0].clone()]);
        rate(&app, &p[1], 5);
        settle(&mut app, &ctx);
        assert_eq!(shown(&app), vec![p[0].clone(), p[1].clone()]);
        rate(&app, &p[0], 1);
        settle(&mut app, &ctx);
        assert_eq!(shown(&app), vec![p[1].clone()]);
    }

    /// A folder is its directory: a catalog write leaves its rows alone.
    #[test]
    fn a_folder_view_is_left_alone() {
        let (mut app, p) = app_of(3);
        let paris = app
            .catalog
            .as_ref()
            .unwrap()
            .create_keyword("Paris", None)
            .unwrap();
        app.tag_files(paris, true, std::slice::from_ref(&p[1]));
        settle(&mut app, &egui::Context::default());
        assert_eq!(shown(&app), p);
    }
}
