use super::*;
use crate::test_ui;

/// Constructed rows: All Photos and one recent folder.
fn rows() -> Vec<ContinueRow> {
    vec![
        ContinueRow {
            kind: ContinueKind::AllPhotos,
            name: "All Photos".to_string(),
            parent: None,
            count: Some(2914),
        },
        ContinueRow {
            kind: ContinueKind::Folder(PathBuf::from("/photos/2026-11-08")),
            name: "2026-11-08".to_string(),
            parent: Some("/photos".to_string()),
            count: Some(212),
        },
    ]
}

/// One frame of the page on a window-sized harness, returning every painted label's rect, and what was picked.
fn draw(rows: &[ContinueRow], welcome: bool) -> Vec<(String, egui::Rect)> {
    let ctx = test_ui::ctx();
    crate::theme::install_fonts(&ctx);
    test_ui::labelled_rects_on(&ctx, test_ui::screen(), |ui| {
        let _ = show(
            ui,
            StartInputs {
                rows,
                show_welcome: welcome,
                keys: &[],
            },
        );
    })
}

fn has(labels: &[(String, egui::Rect)], text: &str) -> Option<egui::Rect> {
    labels.iter().find(|(l, _)| l == text).map(|(_, r)| *r)
}

#[test]
fn the_welcome_shows_beneath_continue() {
    let labels = draw(&rows(), true);
    assert!(has(&labels, WELCOME).is_some(), "{labels:?}");
    let cont = has(&labels, CONTINUE).expect("the Continue card");
    let last = labels
        .iter()
        .filter(|(l, _)| l.starts_with("2026-11-08"))
        .map(|(_, r)| *r)
        .next()
        .expect("the folder row");
    let welcome = has(&labels, WHATS_WHERE).expect("the welcome card");
    assert!(has(&labels, DONT_SHOW).is_some());
    assert!(cont.top() < last.top());
    assert!(
        welcome.top() > last.bottom(),
        "the welcome sits below Continue's last row: {welcome:?} vs {last:?}"
    );
}

#[test]
fn with_nothing_to_continue_only_the_welcome_shows() {
    let labels = draw(&[], true);
    assert!(has(&labels, CONTINUE).is_none());
    assert!(has(&labels, WHATS_WHERE).is_some());
}

#[test]
fn without_the_welcome_the_heading_is_no_photos_open() {
    let labels = draw(&rows(), false);
    assert!(has(&labels, NOTHING_OPEN).is_some(), "{labels:?}");
    assert!(has(&labels, WELCOME).is_none());
    assert!(has(&labels, WHATS_WHERE).is_none());
    assert!(has(&labels, CONTINUE).is_some());
}

/// Click `label` on the page and return what it picked.
fn pick_of(label: &str, rows: &[ContinueRow]) -> Option<StartPick> {
    let ctx = test_ui::ctx();
    crate::theme::install_fonts(&ctx);
    let mut got = None;
    test_ui::click(&ctx, label, |ui| {
        if let Some(p) = show(
            ui,
            StartInputs {
                rows,
                show_welcome: true,
                keys: &[],
            },
        ) {
            got = Some(p);
        }
    });
    got
}

#[test]
fn ticking_the_box_hides_the_welcome() {
    assert_eq!(pick_of(DONT_SHOW, &rows()), Some(StartPick::HideWelcome));
    let mut app = crate::test_app::for_test();
    assert!(
        app.settings.show_welcome,
        "fixture check: the welcome is on"
    );
    crate::library::apply_start_pick(&mut app, &egui::Context::default(), StartPick::HideWelcome);
    assert!(!app.settings.show_welcome);
}

#[test]
fn a_continue_row_opens_its_folder() {
    let rows = rows();
    let label = test_ui::labelled_centres(&test_ui::ctx(), Vec::new(), |ui| {
        let _ = show(
            ui,
            StartInputs {
                rows: &rows,
                show_welcome: false,
                keys: &[],
            },
        );
    })
    .into_iter()
    .map(|(l, _)| l)
    .find(|l| l.starts_with("2026-11-08"))
    .expect("the folder row is labelled");
    assert_eq!(
        pick_of(&label, &rows),
        Some(StartPick::Folder(PathBuf::from("/photos/2026-11-08")))
    );
    assert_eq!(pick_of("All Photos", &rows), Some(StartPick::AllPhotos));
    assert_eq!(
        pick_of("Open a folder…", &rows),
        Some(StartPick::Command(Command::OpenFolder))
    );
}

#[test]
fn there_is_no_drop_hint() {
    let labels = draw(&rows(), true);
    assert!(
        labels.len() > 12,
        "fixture check: the page was drawn, {labels:?}"
    );
    assert!(
        labels
            .iter()
            .all(|(l, _)| !l.to_lowercase().contains("drop")),
        "{labels:?}"
    );
}

/// The pointer on a key's line lights its part of the map: the line reports its number, and the map paints that
/// part lighter (the author, 2026-11-13).
#[test]
fn hovering_a_key_line_lights_its_part() {
    let ctx = test_ui::ctx();
    crate::theme::install_fonts(&ctx);
    let rows = rows();
    let mut body = |ui: &mut egui::Ui| {
        let _ = show(
            ui,
            StartInputs {
                rows: &rows,
                show_welcome: true,
                keys: &[],
            },
        );
    };
    let rects = test_ui::labelled_rects_on(&ctx, test_ui::screen(), &mut body);
    let at = rects
        .iter()
        .find(|(l, _)| l.starts_with("Rail"))
        .map(|(_, r)| r.center())
        .expect("the Rail key line is drawn");
    assert_eq!(lit(&ctx), None, "nothing lit at rest");
    for _ in 0..2 {
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(test_ui::screen()),
                events: vec![egui::Event::PointerMoved(at)],
                ..Default::default()
            },
            &mut body,
        );
    }
    assert_eq!(lit(&ctx), Some(2));
}
