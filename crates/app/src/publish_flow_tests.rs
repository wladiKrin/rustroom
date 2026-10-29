//! The publish flow's tests (M10.18d), beside it so the flow stays readable.

use super::*;
use std::path::Path;
use std::time::Instant;

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

    let mut e = edit_model::Edits::default();
    e.exposure = 0.7;
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
