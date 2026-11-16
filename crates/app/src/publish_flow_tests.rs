//! The publish flow's tests (M10.18d), beside it so the flow stays readable.

use super::*;
use std::path::Path;
use std::time::Instant;

/// Holds a photo's export decode until the test releases it — so a test that asserts an in-between state
/// (one photo written, the other not) can look before the run moves on (AGENTS.md's held-job rule).
///
/// ⚠ Keyed on the file, which lives in the test's own directory: tests run in parallel and must not hold
/// each other's photos.
pub(crate) mod hold {
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    static HELD: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

    pub(crate) fn hold(p: &Path) {
        HELD.lock().unwrap().push(p.to_path_buf());
    }

    pub(crate) fn release(p: &Path) {
        HELD.lock().unwrap().retain(|h| h != p);
    }

    /// Wait while `p` is held, under the 120 s ceiling against a test that never releases it.
    pub(crate) fn wait(p: &Path) {
        let t = Instant::now();
        while HELD.lock().unwrap().iter().any(|h| h == p) && t.elapsed() < Duration::from_secs(120)
        {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

/// A folder of two real (tiny) JPEGs, open in the app; a folder service holding both; its view open.
fn two_photo_service(app: &mut RustroomApp, ctx: &egui::Context, d: &Path, out: &Path) -> i64 {
    for n in ["a.jpg", "b.jpg"] {
        image::RgbImage::from_pixel(64, 48, image::Rgb([120, 90, 70]))
            .save(d.join(n))
            .unwrap();
    }
    std::fs::create_dir_all(out).unwrap();
    app.open_folder_at(ctx, d.to_path_buf());
    let cat = app.catalog.as_ref().unwrap();
    let s = cat
        .create_publish_service("Phone", &PublishTarget::Folder { path: out.into() }, "{}")
        .unwrap();
    for n in ["a.jpg", "b.jpg"] {
        assert!(cat.add_to_service(s, &d.join(n), 0).unwrap());
    }
    app.open_publish(ctx, s, "Phone");
    s
}

fn publish_and_wait(app: &mut RustroomApp, ctx: &egui::Context) {
    app.start_publish();
    assert!(
        app.publish_line().is_some(),
        "no publish started: {}",
        app.status
    );
    let t = Instant::now();
    while app.publish_line().is_some() {
        assert!(t.elapsed() < Duration::from_secs(120), "the publish hung");
        app.pump_publish(ctx);
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn files_in(d: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(d)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

fn counts(app: &mut RustroomApp) -> Counts {
    app.publish_view_states()
        .expect("the service's states")
        .counts
}

#[test]
fn publishing_a_folder_service_writes_and_records_then_a_reedit_is_modified() {
    let ctx = egui::Context::default();
    let mut app = crate::test_app::for_test();
    let (d, o) = (
        test_dir::TempDir::new("publish-src"),
        test_dir::TempDir::new("publish-out"),
    );
    two_photo_service(&mut app, &ctx, d.path(), o.path());
    assert_eq!(counts(&mut app).new, 2);

    publish_and_wait(&mut app, &ctx);
    assert_eq!(app.status, "Published 2");
    assert_eq!(files_in(o.path()), ["a.jpg", "b.jpg"]);
    assert_eq!(counts(&mut app).published, 2);

    let e = edit_model::Edits {
        exposure: 0.7,
        ..Default::default()
    };
    rustroom_app::sidecar::save(&d.join("a.jpg"), &e).unwrap();
    // ⚠ A sidecar written behind the app's back moves no revision of its own; the view is re-read as a
    // return from Develop would re-read it.
    app.publish.view = None;
    let c = counts(&mut app);
    assert_eq!((c.modified, c.published), (1, 1), "{c:?}");

    publish_and_wait(&mut app, &ctx);
    assert_eq!(app.status, "Published 1");
    assert_eq!(
        files_in(o.path()),
        ["a.jpg", "b.jpg"],
        "a re-publish made a second file instead of replacing its own"
    );
    assert_eq!(counts(&mut app).published, 2);
}

#[test]
fn a_removed_member_is_cleared_on_publish() {
    let ctx = egui::Context::default();
    let mut app = crate::test_app::for_test();
    let (d, o) = (
        test_dir::TempDir::new("publish-src-rm"),
        test_dir::TempDir::new("publish-out-rm"),
    );
    let s = two_photo_service(&mut app, &ctx, d.path(), o.path());
    publish_and_wait(&mut app, &ctx);
    app.catalog
        .as_ref()
        .unwrap()
        .remove_from_service(s, &d.join("b.jpg"), 0)
        .unwrap();
    assert_eq!(counts(&mut app).removed, 1);
    publish_and_wait(&mut app, &ctx);
    assert_eq!(app.status, "Published 0, removed 1");
    assert_eq!(files_in(o.path()), ["a.jpg"]);
    let c = counts(&mut app);
    assert_eq!((c.published, c.removed), (1, 0));
}

#[test]
fn an_up_to_date_service_says_so() {
    let ctx = egui::Context::default();
    let mut app = crate::test_app::for_test();
    let (d, o) = (
        test_dir::TempDir::new("publish-src-same"),
        test_dir::TempDir::new("publish-out-same"),
    );
    two_photo_service(&mut app, &ctx, d.path(), o.path());
    publish_and_wait(&mut app, &ctx);
    app.start_publish();
    assert!(app.publish_line().is_none());
    assert_eq!(app.status, "“Phone” is up to date");
}

/// Leaving a service's view drops its states, so its Publish controls go with it.
#[test]
fn leaving_the_view_drops_its_states() {
    let ctx = egui::Context::default();
    let mut app = crate::test_app::for_test();
    let (d, o) = (
        test_dir::TempDir::new("publish-src-leave"),
        test_dir::TempDir::new("publish-out-leave"),
    );
    two_photo_service(&mut app, &ctx, d.path(), o.path());
    assert!(app.publish_view_states().is_some());
    app.open_folder_at(&ctx, d.path().to_path_buf());
    assert!(app.publish_view_states().is_none());
    assert!(app.publish.view.is_none(), "the old view's states stayed");
}

#[test]
fn a_new_service_is_saved_from_its_dialog_with_the_last_export_settings() {
    let mut app = crate::test_app::for_test();
    app.settings.export.quality = 61;
    app.open_new_service();
    assert!(!app.save_service_dialog(), "an unnamed service was saved");
    let d = app.publish.dialog.as_mut().unwrap();
    assert_eq!(d.error.as_deref(), Some("Name the service"));
    d.name = "Phone".into();
    assert!(
        !app.save_service_dialog(),
        "a service with no folder was saved"
    );
    app.publish.dialog.as_mut().unwrap().folder = Some("/out".into());
    assert!(app.save_service_dialog());
    let rows = app.catalog.as_ref().unwrap().publish_services().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(options_for(&rows[0]).quality, 61);
    assert_eq!(
        options_for(&rows[0]).dest_dir,
        std::path::PathBuf::from("/out")
    );

    app.open_new_service();
    let d = app.publish.dialog.as_mut().unwrap();
    d.name = "phone".into();
    d.folder = Some("/elsewhere".into());
    assert!(!app.save_service_dialog());
    assert_eq!(
        app.publish.dialog.as_ref().unwrap().error.as_deref(),
        Some("A service of that name already exists")
    );
}

#[test]
fn editing_a_service_keeps_its_settings_unless_asked() {
    let mut app = crate::test_app::for_test();
    app.settings.export.quality = 61;
    app.open_new_service();
    let d = app.publish.dialog.as_mut().unwrap();
    d.name = "Phone".into();
    d.folder = Some("/out".into());
    assert!(app.save_service_dialog());
    let id = app.catalog.as_ref().unwrap().publish_services().unwrap()[0].id;
    app.settings.export.quality = 90;
    app.open_edit_service(id);
    app.publish.dialog.as_mut().unwrap().name = "Tablet".into();
    assert!(app.save_service_dialog());
    let row = &app.catalog.as_ref().unwrap().publish_services().unwrap()[0];
    assert_eq!(
        (row.name.as_str(), options_for(row).quality),
        ("Tablet", 61)
    );
}

#[test]
fn the_selection_goes_in_and_comes_out() {
    let ctx = egui::Context::default();
    let mut app = crate::test_app::for_test();
    let (d, o) = (
        test_dir::TempDir::new("publish-src-sel"),
        test_dir::TempDir::new("publish-out-sel"),
    );
    let s = two_photo_service(&mut app, &ctx, d.path(), o.path());
    let cat = app.catalog.as_ref().unwrap();
    for n in ["a.jpg", "b.jpg"] {
        cat.remove_from_service(s, &d.join(n), 0).unwrap();
    }
    app.open_folder_at(&ctx, d.path().to_path_buf());
    app.library
        .as_mut()
        .unwrap()
        .selection
        .select_indices(&[0, 1]);
    app.service_selection(s, true);
    assert_eq!(app.status, "Added 2 photos to publish");
    app.service_selection(s, false);
    assert_eq!(app.status, "Took 2 photos out of the service");
    assert!(app
        .catalog
        .as_ref()
        .unwrap()
        .publish_members(s)
        .unwrap()
        .is_empty());
}

/// Two folders' photos of one name in one service — the review's C1, through a real run each time.
fn same_name_service(app: &mut RustroomApp, ctx: &egui::Context, d: &Path, out: &Path) -> i64 {
    for sub in ["one", "two"] {
        std::fs::create_dir_all(d.join(sub)).unwrap();
        image::RgbImage::from_pixel(64, 48, image::Rgb([120, 90, 70]))
            .save(d.join(sub).join("a.jpg"))
            .unwrap();
        app.open_folder_at(ctx, d.join(sub));
    }
    std::fs::create_dir_all(out).unwrap();
    let cat = app.catalog.as_ref().unwrap();
    let s = cat
        .create_publish_service("Same", &PublishTarget::Folder { path: out.into() }, "{}")
        .unwrap();
    for sub in ["one", "two"] {
        assert!(cat
            .add_to_service(s, &d.join(sub).join("a.jpg"), 0)
            .unwrap());
    }
    app.open_publish(ctx, s, "Same");
    s
}

fn recorded_files(app: &RustroomApp, s: i64) -> Vec<std::path::PathBuf> {
    let mut v: Vec<_> = app
        .catalog
        .as_ref()
        .unwrap()
        .publish_members(s)
        .unwrap()
        .into_iter()
        .filter_map(|m| match m.published?.place {
            catalog::db::Place::File { path, .. } => Some(path),
            _ => None,
        })
        .collect();
    v.sort();
    v
}

#[test]
fn two_photos_of_one_name_keep_their_own_files_across_a_settings_change() {
    let ctx = egui::Context::default();
    let mut app = crate::test_app::for_test();
    let (d, o) = (
        test_dir::TempDir::new("publish-same-src"),
        test_dir::TempDir::new("publish-same-out"),
    );
    let s = same_name_service(&mut app, &ctx, d.path(), o.path());
    publish_and_wait(&mut app, &ctx);
    assert_eq!(files_in(o.path()), ["a-1.jpg", "a.jpg"]);
    let first = recorded_files(&app, s);
    assert_eq!(first.len(), 2);
    // Every member Modified at once — the case that made both write one file.
    let cat = app.catalog.as_ref().unwrap();
    cat.update_publish_service(
        s,
        "Same",
        &PublishTarget::Folder {
            path: o.path().into(),
        },
        r#"{"quality":50}"#,
    )
    .unwrap();
    app.publish.view = None;
    assert_eq!(counts(&mut app).modified, 2);
    publish_and_wait(&mut app, &ctx);
    assert_eq!(app.status, "Published 2");
    assert_eq!(
        files_in(o.path()),
        ["a-1.jpg", "a.jpg"],
        "a file was shared, or one leaked"
    );
    assert_eq!(recorded_files(&app, s), first, "a member's file moved");
}

/// The review's I4: a file retouched by hand at the destination is never written over.
#[test]
fn a_published_file_changed_by_hand_is_not_overwritten() {
    let ctx = egui::Context::default();
    let mut app = crate::test_app::for_test();
    let (d, o) = (
        test_dir::TempDir::new("publish-hand-src"),
        test_dir::TempDir::new("publish-hand-out"),
    );
    two_photo_service(&mut app, &ctx, d.path(), o.path());
    publish_and_wait(&mut app, &ctx);
    std::fs::write(o.join("a.jpg"), b"retouched by hand").unwrap();
    let e = edit_model::Edits {
        exposure: 0.7,
        ..Default::default()
    };
    rustroom_app::sidecar::save(&d.join("a.jpg"), &e).unwrap();
    app.publish.view = None;
    publish_and_wait(&mut app, &ctx);
    assert_eq!(
        std::fs::read(o.join("a.jpg")).unwrap(),
        b"retouched by hand"
    );
    assert_eq!(files_in(o.path()), ["a-1.jpg", "a.jpg", "b.jpg"]);
    assert_eq!(
        app.status,
        "Published 1, left 1 file changed since it was published"
    );
}

/// The review's I2: a cancelled publish records what it delivered, so nothing it wrote is left untracked.
#[test]
fn a_cancelled_publish_records_what_it_wrote() {
    let ctx = egui::Context::default();
    let mut app = crate::test_app::for_test();
    let (d, o) = (
        test_dir::TempDir::new("publish-cancel-src"),
        test_dir::TempDir::new("publish-cancel-out"),
    );
    let s = two_photo_service(&mut app, &ctx, d.path(), o.path());
    // ⚠ b.jpg is held, or both photos can land between two polls and "1 of" is never seen (it was,
    // on the merged main, every time).
    let b = d.path().join("b.jpg");
    hold::hold(&b);
    app.start_publish();
    // ⚠ Cancelled once a photo is WRITTEN — cancelled before, nothing is on disk and the check below would
    // pass for any code.
    let t = Instant::now();
    while !app
        .publish_line()
        .is_some_and(|l| l.starts_with("Publishing 1 of"))
    {
        assert!(
            t.elapsed() < Duration::from_secs(120),
            "no photo was written"
        );
        app.pump_publish(&ctx);
        std::thread::sleep(Duration::from_millis(1));
    }
    app.cancel_publish();
    hold::release(&b);
    let t = Instant::now();
    while app.publish_line().is_some() {
        assert!(t.elapsed() < Duration::from_secs(120), "the cancel hung");
        app.pump_publish(&ctx);
        std::thread::sleep(Duration::from_millis(2));
    }
    let on_disk: Vec<std::path::PathBuf> = files_in(o.path()).iter().map(|n| o.join(n)).collect();
    assert_eq!(
        recorded_files(&app, s),
        on_disk,
        "a written file is untracked"
    );
}

/// The review's I5: a new destination starts every member New — the old folder keeps what it has.
#[test]
fn a_new_destination_makes_every_photo_new() {
    let ctx = egui::Context::default();
    let mut app = crate::test_app::for_test();
    let (d, o, o2) = (
        test_dir::TempDir::new("publish-move-src"),
        test_dir::TempDir::new("publish-move-out"),
        test_dir::TempDir::new("publish-move-out2"),
    );
    let s = two_photo_service(&mut app, &ctx, d.path(), o.path());
    publish_and_wait(&mut app, &ctx);
    app.open_edit_service(s);
    app.publish.dialog.as_mut().unwrap().folder = Some(o2.path().into());
    assert!(app.save_service_dialog());
    assert_eq!(counts(&mut app).new, 2);
    assert_eq!(
        files_in(o.path()),
        ["a.jpg", "b.jpg"],
        "the old folder was touched"
    );
}

/// The review's I8: a catalog write alone re-derives the states at most every RECOMPUTE.
#[test]
fn a_catalog_write_rederives_the_states_at_most_every_recompute() {
    let ctx = egui::Context::default();
    let mut app = crate::test_app::for_test();
    let (d, o) = (
        test_dir::TempDir::new("publish-throttle-src"),
        test_dir::TempDir::new("publish-throttle-out"),
    );
    two_photo_service(&mut app, &ctx, d.path(), o.path());
    let at = app.publish_view_states().unwrap().at;
    let cat = app.catalog.as_ref().unwrap();
    cat.create_collection("A write").unwrap();
    assert_eq!(
        app.publish_view_states().unwrap().at,
        at,
        "re-derived on a write at once"
    );
}

/// A service's view draws its Publish button in a band above the grid, and the status row no longer does.
#[test]
fn a_service_view_shows_publish_in_a_band_above_the_grid() {
    let ctx = crate::test_ui::ctx();
    let mut app = crate::test_app::for_test();
    let (d, o) = (
        test_dir::TempDir::new("publish-src-band"),
        test_dir::TempDir::new("publish-out-band"),
    );
    two_photo_service(&mut app, &ctx, d.path(), o.path());
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
    let nodes = crate::test_ui::nodes_on(&ctx, screen, |ui| {
        let c = ui.ctx().clone();
        crate::library::show(&mut app, ui, &c);
    });
    let publish = nodes
        .iter()
        .find(|n| n.label == "Publish 2")
        .unwrap_or_else(|| {
            panic!(
                "no Publish 2 in the grid's view: {:?}",
                nodes.iter().map(|n| &n.label).collect::<Vec<_>>()
            )
        });
    assert!(
        publish.rect.top() < 40.0,
        "the button sits at {:?}",
        publish.rect
    );
    // The button at the band's left end, the name and counts at its right (the author, 2026-11-02).
    assert!(
        publish.rect.left() < 40.0,
        "the button is not at the left end: {:?}",
        publish.rect
    );
    let counts = nodes
        .iter()
        .find(|n| n.label == "2 new")
        .expect("the counts");
    assert!(
        counts.rect.right() > 1000.0,
        "the counts are not at the right end: {:?}",
        counts.rect
    );
    assert!(
        nodes.iter().any(|n| n.label == "2 new"),
        "the counts are beside it"
    );
    let row = crate::test_ui::nodes_on(&ctx, screen, |ui| {
        let c = ui.ctx().clone();
        crate::status_bar::library_row(&mut app, ui, &c);
    });
    assert!(
        row.iter().any(|n| !n.label.is_empty()),
        "the status row drew nothing at all"
    );
    assert!(
        !row.iter().any(|n| n.label.starts_with("Publish")),
        "the status row still holds Publish"
    );
}

/// Outside a service's view there is no band.
#[test]
fn a_folder_view_has_no_publish_band() {
    let ctx = crate::test_ui::ctx();
    let mut app = crate::test_app::for_test();
    app.library = Some(crate::test_app::library_of(3));
    let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
    let nodes = crate::test_ui::nodes_on(&ctx, screen, |ui| {
        let c = ui.ctx().clone();
        crate::library::show(&mut app, ui, &c);
    });
    assert!(
        nodes.iter().any(|n| !n.label.is_empty()),
        "the grid drew nothing at all"
    );
    assert!(!nodes.iter().any(|n| n.label.starts_with("Publish")));
}

/// One finished Immich run: two uploaded photos waiting for the album step.
fn uploaded_run() -> Run {
    let asset = |p: &str, id: &str| {
        (
            (std::path::PathBuf::from(p), 0),
            Published {
                fingerprint: 1,
                place: catalog::db::Place::Asset { id: id.into() },
                at: 0,
            },
        )
    };
    Run {
        service: 1,
        target: PublishTarget::Immich {
            album_id: Some("al1".into()),
            album_name: "analog".into(),
        },
        options_json: String::new(),
        name: "analog".into(),
        plan: run::PublishPlan::default(),
        worker: None,
        progress: String::new(),
        album: None,
        rows: vec![asset("/a.jpg", "n1"), asset("/b.jpg", "n2")],
        outcome: Outcome::default(),
    }
}

/// The reported bug (2026-11-16): every photo uploaded, the album step was refused, and the status line read
/// "Published 0, 75 failed". Photos the album step could not place are said as uploaded, with Immich's reason.
#[test]
fn a_refused_album_step_says_the_photos_were_uploaded() {
    let mut app = crate::test_app::for_test();
    app.publish.run = Some(uploaded_run());
    app.finish_album(Err(run::AlbumError {
        album: Some("al1".into()),
        why: "Immich refused: Missing required permission: albumAsset.create".into(),
    }));
    assert_eq!(
        app.status,
        "2 uploaded but not added to the album, \
         Immich refused: Missing required permission: albumAsset.create"
    );
}

/// An add the album answered "no" for, per photo, is uploaded-not-added too — never "failed".
#[test]
fn a_photo_the_album_turned_down_is_uploaded_not_failed() {
    let mut app = crate::test_app::for_test();
    app.publish.run = Some(uploaded_run());
    app.finish_album(Ok(AlbumSync {
        album: "al1".into(),
        added: ["n1".to_string()].into_iter().collect(),
        ..Default::default()
    }));
    assert!(
        app.status.contains("1 uploaded but not added to the album")
            && !app.status.contains("failed"),
        "{}",
        app.status
    );
}
