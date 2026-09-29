//! ⚠⚠ **Every clickable control says what it does** (M10.6b) — the check `DESIGN.md` §8 asks for,
//! made on the drawn app rather than on its sources.
//!
//! Each surface a harness can draw is drawn with its sections opened, and every AccessKit node that
//! offers a click must carry a description — the sentence `theme::Hover` records — which is one short
//! sentence by `theme::sentence_fault`. Reading what was **drawn** is what the source sweep cannot do:
//! a sentence built with `format!`, passed in a tuple, or simply missing is invisible to it.
//!
//! **Exempt, by the author's scope (2026-09-29):** section headers (they only open and close — marked
//! by their AccessKit *expanded* state), menu items (`Role::MenuItem`: their label and shortcut say it),
//! text fields (their placeholder names what goes in). Nodes with no declared role (`Role::Unknown`)
//! are internal hit areas — a canvas, a row's background — not controls. ⚠ **Labels are exempt too,
//! and not by oversight:** egui's labels are selectable text by design here (`theme::empty_state`
//! says why the global switch stays on), so every label reports a click it only uses for selecting —
//! 50-odd "controls" on the first run that were nothing of the kind.
//!
//! ⚠ **Not reachable by any harness, so not covered — said rather than claimed:** Compare, Survey,
//! the canvas overlays and the Import dialog.

use crate::test_ui::{self, Node};
use eframe::egui;
use egui::accesskit::Role;
use rustroom_app::tool::Tool;

/// Roles that never need a sentence of their own.
const EXEMPT: &[Role] = &[
    Role::Label,
    Role::TextInput,
    Role::MenuItem,
    Role::Unknown,
    Role::Window,
    Role::Pane,
    Role::ScrollBar,
    Role::Splitter,
];

#[derive(Default)]
struct Walk {
    faults: Vec<String>,
    /// Controls checked, for the sample assertion.
    seen: usize,
}

/// A control — it takes a click, or it is a slider, which egui's own slider does not report as
/// clickable (it only drags) and which the author's scope names.
fn is_control(n: &Node) -> bool {
    (n.clickable || n.role == Role::Slider) && !n.expandable && !EXEMPT.contains(&n.role)
}

/// ⚠ egui's **built-in** slider draws its number as a separate `DragValue` with an automatic id, so
/// no caller can give that number a description. It is covered by the described slider it belongs to
/// — one directly left of it on the same row, which is exactly how egui lays the two out. The app's
/// own slider describes its number itself (`develop/panel/slider.rs`).
fn number_of_a_described_slider(n: &Node, nodes: &[Node]) -> bool {
    n.role == Role::SpinButton
        && nodes.iter().any(|s| {
            s.role == Role::Slider
                && s.description.as_deref().is_some_and(|d| !d.is_empty())
                && s.rect.y_range().intersects(n.rect.y_range())
                && s.rect.right() <= n.rect.left() + 1.0
                && n.rect.left() - s.rect.right() < 40.0
        })
}

impl Walk {
    fn check(&mut self, surface: &str, nodes: &[Node]) {
        for n in nodes {
            if !is_control(n) || number_of_a_described_slider(n, nodes) {
                continue;
            }
            self.seen += 1;
            match n.description.as_deref() {
                None | Some("") => self.faults.push(format!(
                    "{surface}: {:?} ({:?}) has no sentence",
                    n.label, n.role
                )),
                Some(d) => {
                    if let Some(f) = crate::theme::sentence_fault(d) {
                        self.faults
                            .push(format!("{surface}: {:?}: {f}: {d:?}", n.label));
                    }
                }
            }
        }
    }
}

/// Draw `body` until no section is left closed, then report its nodes.
fn open_all(ctx: &egui::Context, mut body: impl FnMut(&mut egui::Ui)) -> Vec<Node> {
    for _ in 0..8 {
        let nodes = test_ui::nodes(ctx, &mut body);
        let closed: Vec<String> = nodes
            .iter()
            .filter(|n| n.expanded == Some(false))
            .map(|n| n.label.clone())
            .collect();
        if closed.is_empty() {
            return nodes;
        }
        for label in closed {
            test_ui::click(ctx, &label, &mut body);
        }
    }
    test_ui::nodes(ctx, &mut body)
}

fn themed_ctx() -> egui::Context {
    let ctx = test_ui::ctx();
    crate::theme::apply(&ctx, &rustroom_app::settings::Settings::default());
    ctx
}

/// The rail with every section open, a catalogued folder, a collection, a keyword, a dated photo and
/// a configured Immich server with an album and a month — `every_count_in_the_rail_ends_at_one_x`'s
/// fixture.
fn library_rail(w: &mut Walk) {
    let ctx = themed_ctx();
    let mut app = crate::test_app::for_test();
    app.library = Some(crate::test_app::library_of(3));
    crate::test_app::catalog_library(&mut app);
    {
        let cat = app.catalog.as_ref().unwrap();
        let first = app.library.as_ref().unwrap().entries[0].id.path.clone();
        cat.set_capture(
            &first,
            &core_image::Capture {
                taken: Some(20_590 * 86_400),
                ..Default::default()
            },
        )
        .unwrap();
        cat.create_collection("Trips").unwrap();
        cat.create_keyword("Europe", None).unwrap();
    }
    let s = &mut app.settings.panel_sections;
    s.folders = true;
    s.collections = true;
    s.smart = true;
    s.keywords = true;
    s.dates = true;
    s.immich = true;
    app.settings.immich_enabled = true;
    app.settings.immich_url = "https://immich.example".to_string();
    app.settings.immich_key = "a-key".to_string();
    app.immich_albums = Some(Ok(vec![immich::AlbumSummary {
        id: "a1".to_string(),
        album_name: "Holiday".to_string(),
        asset_count: 1234,
        last_modified_asset_timestamp: None,
    }]));
    app.immich_buckets = Some(Ok(vec![immich::Bucket {
        time_bucket: "2025-03-01T00:00:00.000Z".to_string(),
        count: 57,
    }]));
    let nodes = open_all(&ctx, |ui| {
        crate::collections_panel::show(&mut app, ui, &ctx)
    });
    w.check("Library rail", &nodes);
}

/// The top bar in `view`, on a wide screen — at the panels' 400 pt it has no gap for its centred line.
fn top_bar(w: &mut Walk, surface: &str, view: crate::AppView) {
    let ctx = themed_ctx();
    let mut app = crate::test_app::for_test();
    crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
    app.library = Some(crate::test_app::library_of(3));
    app.view = view;
    let wide = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 200.0));
    let nodes = test_ui::nodes_on(&ctx, wide, |ui| {
        let ctx = ui.ctx().clone();
        let _ = app.top_bar(ui, &ctx);
    });
    w.check(surface, &nodes);
}

fn status_row(w: &mut Walk) {
    let ctx = themed_ctx();
    let mut app = crate::test_app::for_test();
    app.library = Some(crate::test_app::library_of(3));
    let nodes = test_ui::nodes(&ctx, |ui| {
        let ctx = ui.ctx().clone();
        crate::status_bar::library_row(&mut app, ui, &ctx);
    });
    w.check("Status row", &nodes);
}

/// Drawn directly: the app draws it only off macOS, and its exemption must be exercised here too,
/// not only on the Linux runner (standing lesson (e) — a test of the host).
fn menu_bar(w: &mut Walk) {
    let ctx = themed_ctx();
    let nodes = test_ui::nodes(&ctx, |ui| {
        let _ = crate::menu_bar::show(ui, &[]);
    });
    assert!(
        nodes.iter().any(|n| n.role == Role::MenuItem),
        "sample: the menu bar drew no menu"
    );
    w.check("Menu bar", &nodes);
}

/// The Develop panel under `tool`, after `setup` has shaped the session.
fn develop(w: &mut Walk, surface: &str, tool: Tool, setup: impl FnOnce(&mut crate::Session)) {
    let ctx = themed_ctx();
    let mut app = crate::test_app::for_test();
    crate::test_app::open_test_photo(&mut app, &ctx, 200, 100);
    app.tool = tool;
    if let Some(s) = app.session.as_mut() {
        setup(s);
        s.sync_mask_shown();
    }
    let nodes = open_all(&ctx, |ui| {
        egui::ScrollArea::vertical().show(ui, |ui| {
            let _ = crate::develop::panel::show(&mut app, ui);
        });
    });
    w.check(surface, &nodes);
}

fn settings(w: &mut Walk) {
    let ctx = themed_ctx();
    let mut app = crate::test_app::for_test();
    app.modal = crate::modals::Modal::Settings;
    let mut body = |_: &mut egui::Ui| crate::modals::settings::show(&mut app, &ctx);
    let _ = test_ui::nodes(&ctx, &mut body);
    for tab in [
        "Behaviour",
        "Appearance",
        "Display",
        "Performance",
        "Input",
        "Immich",
    ] {
        test_ui::click(&ctx, tab, &mut body);
        let nodes = open_all(&ctx, &mut body);
        w.check(&format!("Settings ({tab})"), &nodes);
    }
}

fn export(w: &mut Walk) {
    let ctx = themed_ctx();
    let mut app = crate::test_app::for_test();
    app.library = Some(crate::test_app::library_of(2));
    app.library.as_mut().unwrap().selection.select_indices(&[0]);
    let nodes = open_all(&ctx, |_| crate::modals::export::settings(&mut app, &ctx));
    w.check("Export", &nodes);
}

fn info(w: &mut Walk) {
    let ctx = themed_ctx();
    let mut app = crate::test_app::for_test();
    app.view = crate::AppView::Library;
    app.library = Some(crate::test_app::library_of(1));
    crate::test_app::catalog_library(&mut app);
    let id = app.library.as_ref().unwrap().entries[0].id.clone();
    app.modal = crate::modals::Modal::PhotoInfo {
        id,
        dims: Some((0, 0)),
        description: None,
    };
    let nodes = open_all(&ctx, |_| crate::modals::info::show(&mut app, &ctx));
    w.check("Info", &nodes);
}

#[test]
fn every_clickable_control_says_what_it_does() {
    let mut w = Walk::default();
    library_rail(&mut w);
    top_bar(&mut w, "Top bar (Library)", crate::AppView::Library);
    top_bar(&mut w, "Top bar (Develop)", crate::AppView::Develop);
    status_row(&mut w);
    menu_bar(&mut w);
    develop(&mut w, "Develop (Edit)", Tool::Edit, |_| {});
    develop(&mut w, "Develop (Crop)", Tool::Crop, |_| {});
    for kind in rustroom_app::mask_kind::MaskKind::ALL {
        develop(
            &mut w,
            &format!("Develop (Masks: {kind:?})"),
            Tool::Masks,
            |s| {
                s.edits.masks = vec![edit_model::MaskedAdjustment {
                    mask: kind.template(),
                    ..Default::default()
                }];
                s.selected_mask = Some(0);
            },
        );
    }
    develop(&mut w, "Develop (Retouch)", Tool::Retouch, |s| {
        s.edits.spots = vec![edit_model::Spot {
            centre: [0.5, 0.5],
            source: [0.3, 0.3],
            radius: 0.05,
            feather: 0.5,
            mode: edit_model::SpotMode::Heal,
        }];
        s.selected_spot = Some(0);
        s.sync_spot_flags();
    });
    develop(&mut w, "Develop (Presets)", Tool::Presets, |_| {});
    settings(&mut w);
    export(&mut w);
    info(&mut w);
    // ⚠ The sample first (standing lesson (b)): a walk that reached nothing would pass below.
    // ⚠ 537 on 2026-09-30; the floor sits below it so a surface that stops drawing fails here
    // rather than quietly shrinking the walk.
    assert!(w.seen > 400, "sample: only {} controls checked", w.seen);
    assert!(
        w.faults.is_empty(),
        "{} controls without one short sentence:\n{}",
        w.faults.len(),
        w.faults.join("\n")
    );
}
