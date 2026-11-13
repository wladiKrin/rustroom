//! The People view's tests (M10.18c), kept beside it so the view stays readable.

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
    // ⚠ An injected finder, so the face models count as present wherever this runs: without it the view read this
    // machine's model folder, and on CI — no models downloaded — said they were missing instead (2026-11-13).
    app.people.finder = Some(Arc::new(|_| Ok(Vec::new())));
    // …and an EMPTY models folder, as on CI, so the injected finder is what makes the models present here too.
    app.ai.dirs = Some((d.path().join("empty"), d.path().join("empty")));
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

/// Asked for in use: a face dragged onto a person is named after them — and the release is not also a click
/// that opens their photos.
#[test]
fn a_face_dropped_on_a_person_is_named_after_them() {
    let ctx = crate::test_ui::ctx();
    let mut app = crate::test_app::for_test();
    app.settings.find_faces = false;
    let d = test_dir::TempDir::new("people-drop");
    for n in ["A.JPG", "B.JPG"] {
        std::fs::write(d.join(n), b"x").unwrap();
    }
    app.open_folder_at(&ctx, d.path().to_path_buf());
    let cat = app.catalog.as_ref().unwrap();
    for n in ["A.JPG", "B.JPG"] {
        let f = NewFace {
            bbox: [0.1, 0.1, 0.3, 0.3],
            embedding: vec![0.088; 128],
        };
        cat.set_local_faces(&d.join(n), &[f]).unwrap();
    }
    let anna = cat.person_named("Anna").unwrap();
    let named = cat.faces_of(&d.join("A.JPG")).unwrap()[0].id;
    cat.set_face_states(&[(named, (Some(anna), true, false))])
        .unwrap();
    let loose = cat.faces_of(&d.join("B.JPG")).unwrap()[0].id;
    app.rail_data.refresh(app.catalog.as_ref(), None);
    app.library_mode = crate::LibraryMode::People;
    egui::DragAndDrop::set_payload(&ctx, FaceDrag(loose));
    crate::test_ui::click(&ctx, "Anna · 1", |ui| {
        let c = ui.ctx().clone();
        show(&mut app, ui, &c);
    });
    let state = app.catalog.as_ref().unwrap().face_states(&[loose]).unwrap();
    assert_eq!(state[0].1 .0, Some(anna), "the dropped face was not named");
    assert!(
        app.library_mode == crate::LibraryMode::People,
        "the drop also opened Anna's photos"
    );
}
