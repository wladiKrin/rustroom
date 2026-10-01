//! A side panel on the side the layout says (M10.8b).
//!
//! ⚠ One function for every movable panel — the Library rail, the edit panel, the tool rail and a
//! side filmstrip — so "left or right" is decided in one place.

use eframe::egui;
use rustroom_app::layout::Side;

/// `egui::Panel::left` or `right`, by `side`.
pub(crate) fn side(side: Side, id: impl Into<egui::Id>) -> egui::Panel {
    match side {
        Side::Left => egui::Panel::left(id),
        Side::Right => egui::Panel::right(id),
    }
}

#[cfg(test)]
mod tests {
    use crate::{test_app, test_ui, AppView, RustroomApp};
    use eframe::egui;
    use rustroom_app::layout::{Side, StripPlace};
    use rustroom_app::menu::Command;

    /// The rectangle egui stored for the panel `id` — what the panel actually occupied.
    fn panel_rect(ctx: &egui::Context, id: &str) -> Option<egui::Rect> {
        ctx.data_mut(|d| d.get_persisted::<egui::containers::panel::PanelState>(egui::Id::new(id)))
            .map(|s| s.outer_rect)
    }

    fn screen() -> egui::Rect {
        egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1440.0, 900.0))
    }

    /// A frame of Develop in this layout: the rail's, the panel's and the strip's rectangles.
    fn develop(panel: Side, strip: StripPlace) -> (egui::Rect, egui::Rect, egui::Rect) {
        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();
        app.settings.layout.edit_panel = panel;
        app.settings.layout.filmstrip = strip;
        let mut lib = test_app::library_of(30);
        lib.selection.click(0);
        app.library = Some(lib);
        test_app::open_test_photo(&mut app, &ctx, 200, 100);
        app.view = AppView::Develop;
        app.show_filmstrip = true;
        for _ in 0..2 {
            let _ = test_ui::nodes_on(&ctx, screen(), |ui| {
                let c = ui.ctx().clone();
                crate::develop::body(&mut app, ui, &c);
            });
        }
        let strip_id = match strip {
            StripPlace::Bottom => "filmstrip",
            _ => "filmstrip_side",
        };
        (
            panel_rect(&ctx, "rail").expect("the tool rail was drawn"),
            panel_rect(&ctx, "panel").expect("the edit panel was drawn"),
            panel_rect(&ctx, strip_id).expect("the filmstrip was drawn"),
        )
    }

    /// ⚠⚠ All six Develop layouts, drawn: the tool rail on the panel's outer edge, a strip on the
    /// panel's side at the window edge, a bottom strip under the photo column — and nothing on top of
    /// anything else.
    #[test]
    fn every_develop_layout_puts_each_piece_where_it_says() {
        for panel in [Side::Left, Side::Right] {
            for strip in [StripPlace::Bottom, StripPlace::Left, StripPlace::Right] {
                let (rail, edit, film) = develop(panel, strip);
                let at =
                    format!("{panel:?} {strip:?}: rail {rail:?} panel {edit:?} strip {film:?}");
                let (r, e, f) = (rail.shrink(1.0), edit.shrink(1.0), film.shrink(1.0));
                assert!(
                    !r.intersects(e) && !r.intersects(f) && !e.intersects(f),
                    "overlap — {at}"
                );
                match panel {
                    Side::Right => {
                        assert!(rail.center().x > edit.center().x, "rail outside — {at}")
                    }
                    Side::Left => assert!(rail.center().x < edit.center().x, "rail outside — {at}"),
                }
                match (strip.side(), panel) {
                    (None, side) => {
                        assert!(film.min.y > 450.0, "at the bottom — {at}");
                        // Under the photo column only: clear of the panel and its rail.
                        match side {
                            Side::Right => assert!(
                                film.min.x < 1.0 && film.max.x <= edit.min.x.min(rail.min.x) + 1.0,
                                "the photo column — {at}"
                            ),
                            Side::Left => assert!(
                                film.min.x >= edit.max.x.max(rail.max.x) - 1.0
                                    && film.max.x > 1439.0,
                                "the photo column — {at}"
                            ),
                        }
                    }
                    (Some(Side::Right), Side::Right) => {
                        assert!(film.min.x >= rail.max.x - 1.0, "outermost — {at}")
                    }
                    (Some(Side::Left), Side::Left) => {
                        assert!(film.max.x <= rail.min.x + 1.0, "outermost — {at}")
                    }
                    (Some(Side::Left), Side::Right) => {
                        assert!(
                            film.max.x < edit.min.x && film.min.x < 1.0,
                            "left edge — {at}"
                        )
                    }
                    (Some(Side::Right), Side::Left) => {
                        assert!(
                            film.min.x > edit.max.x && film.max.x > 1439.0,
                            "right edge — {at}"
                        )
                    }
                }
            }
        }
    }

    /// The Library rail on either side, the grid beside it.
    #[test]
    fn the_library_rail_sits_on_its_side_of_the_grid() {
        for side in [Side::Left, Side::Right] {
            let ctx = test_ui::ctx();
            let mut app = test_app::for_test();
            app.settings.layout.library_rail = side;
            app.library = Some(test_app::library_of(6));
            let mut cells = Vec::new();
            for _ in 0..2 {
                cells = test_ui::labelled_rects_on(&ctx, screen(), |ui| {
                    let c = ui.ctx().clone();
                    crate::collections_panel::show(&mut app, ui, &c);
                    crate::library::show(&mut app, ui, &c);
                });
            }
            let rail = panel_rect(&ctx, "collections").expect("the rail was drawn");
            let cell = cells
                .iter()
                .find(|(l, _)| l.starts_with("rustroom_"))
                .map(|(_, r)| *r)
                .expect("the sample: a grid cell");
            match side {
                Side::Left => assert!(cell.min.x >= rail.max.x - 1.0, "{rail:?} {cell:?}"),
                Side::Right => assert!(cell.max.x <= rail.min.x + 1.0, "{rail:?} {cell:?}"),
            }
        }
    }

    fn run(app: &mut RustroomApp, cmd: Command) {
        crate::commands::execute(app, &egui::Context::default(), cmd);
    }

    /// The View commands flip and set the layout, and choosing a place shows a hidden strip.
    #[test]
    fn the_view_commands_move_the_panels() {
        let mut app = test_app::for_test();
        run(&mut app, Command::ToggleLibraryRailSide);
        run(&mut app, Command::ToggleEditPanelSide);
        assert_eq!(app.settings.layout.library_rail, Side::Right);
        assert_eq!(app.settings.layout.edit_panel, Side::Left);
        run(&mut app, Command::ToggleEditPanelSide);
        assert_eq!(app.settings.layout.edit_panel, Side::Right, "a toggle");
        app.show_filmstrip = false;
        run(&mut app, Command::FilmstripAt(StripPlace::Right));
        assert_eq!(app.settings.layout.filmstrip, StripPlace::Right);
        assert!(app.show_filmstrip, "choosing a place shows the strip");
    }
}
