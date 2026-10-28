//! The Map view's right-click menu (M10.18b, asked for in use 2026-10-28): on a pin, what to do with its
//! photos; on the map, what to do there.

use crate::{AppView, LibraryMode, RustroomApp};
use eframe::egui;
use rustroom_app::map_state::MapCamera;
use rustroom_app::photo_id::PhotoId;
use std::sync::Arc;

/// What was right-clicked.
#[derive(Clone)]
pub(crate) enum MenuTarget {
    /// A pin: its entries and their positions.
    Pins {
        entries: Arc<Vec<usize>>,
        at: Arc<Vec<(f64, f64)>>,
    },
    /// The map itself, at this place.
    Place((f64, f64)),
}

/// What the menu asked for — applied after the frame's drawing, when `app` is free again.
pub(crate) enum MenuAction {
    Select(Arc<Vec<usize>>),
    ShowInGrid(Arc<Vec<usize>>),
    ZoomTo(Arc<Vec<(f64, f64)>>),
    Open(usize),
    Remove(Arc<Vec<usize>>),
    PlaceHere((f64, f64)),
    ZoomHere((f64, f64)),
    ShowAll,
}

/// The menu's items for `target`; `selected` photos can be placed from the map's own menu.
pub(crate) fn show(ui: &mut egui::Ui, target: &MenuTarget, selected: usize) -> Option<MenuAction> {
    let mut out = None;
    match target {
        MenuTarget::Pins { entries, at } => {
            let n = rustroom_app::status_text::photos(entries.len());
            if ui.button(format!("Select {n}")).clicked() {
                out = Some(MenuAction::Select(entries.clone()));
            }
            if ui.button("Show in the grid").clicked() {
                out = Some(MenuAction::ShowInGrid(entries.clone()));
            }
            if ui.button(format!("Zoom to {n}")).clicked() {
                out = Some(MenuAction::ZoomTo(at.clone()));
            }
            if entries.len() == 1 && ui.button("Open in Develop").clicked() {
                out = Some(MenuAction::Open(entries[0]));
            }
            ui.separator();
            if ui.button("Remove location").clicked() {
                out = Some(MenuAction::Remove(entries.clone()));
            }
        }
        MenuTarget::Place(at) => {
            let label = match selected {
                0 => "Place the selected photos here".to_string(),
                n => format!("Place {} here", rustroom_app::status_text::photos(n)),
            };
            if ui
                .add_enabled(selected > 0, egui::Button::new(label))
                .on_disabled_hover_text("Select photos in the strip or the grid first")
                .clicked()
            {
                out = Some(MenuAction::PlaceHere(*at));
            }
            if ui.button("Zoom in here").clicked() {
                out = Some(MenuAction::ZoomHere(*at));
            }
            if ui.button("Show all photos").clicked() {
                out = Some(MenuAction::ShowAll);
            }
        }
    }
    if out.is_some() {
        ui.close();
    }
    out
}

impl RustroomApp {
    /// Do what the Map's menu asked for.
    pub(crate) fn apply_map_action(
        &mut self,
        ctx: &egui::Context,
        a: MenuAction,
        size: (f32, f32),
    ) {
        let ids = |app: &RustroomApp, entries: &[usize]| -> Vec<PhotoId> {
            app.library.as_ref().map_or_else(Vec::new, |l| {
                entries
                    .iter()
                    .filter_map(|&i| l.entries.get(i).map(|e| e.id.clone()))
                    .collect()
            })
        };
        match a {
            MenuAction::Select(e) => {
                if let Some(l) = self.library.as_mut() {
                    l.selection.select_indices(&e);
                }
            }
            MenuAction::ShowInGrid(e) => {
                if let Some(l) = self.library.as_mut() {
                    l.selection.select_indices(&e);
                }
                self.view = AppView::Library;
                self.library_mode = LibraryMode::Grid;
            }
            MenuAction::ZoomTo(at) => self.map.set_camera(MapCamera::fit(&at, size)),
            MenuAction::Open(i) => self.open_entry(ctx, i),
            MenuAction::Remove(e) => {
                let ids = ids(self, &e);
                self.remove_location(&ids);
            }
            MenuAction::PlaceHere(at) => {
                let ids = self
                    .library
                    .as_ref()
                    .map_or_else(Vec::new, |l| l.selected_ids());
                self.place_at(&ids, at);
            }
            MenuAction::ZoomHere(at) => {
                let zoom = self.map.camera().map_or(12.0, |c| c.zoom + 2.0);
                self.map.set_camera(MapCamera { center: at, zoom });
            }
            MenuAction::ShowAll => self.map.refit_next(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The menu's actions on the Library and the catalog — select, show in the grid, place, remove.
    #[test]
    fn the_map_menu_selects_shows_places_and_removes() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        let d = test_dir::TempDir::new("map-menu");
        for n in ["A.JPG", "B.JPG", "C.JPG"] {
            std::fs::write(d.join(n), b"x").unwrap();
        }
        app.open_folder_at(&ctx, d.path().to_path_buf());
        let size = (800.0, 600.0);
        let path =
            |app: &RustroomApp, i: usize| app.library.as_ref().unwrap().entries[i].id.path.clone();
        let selected = |app: &RustroomApp| app.library.as_ref().unwrap().selection.len();

        app.apply_map_action(&ctx, MenuAction::Select(Arc::new(vec![0, 2])), size);
        assert_eq!(selected(&app), 2);
        app.library_mode = LibraryMode::Map;
        app.apply_map_action(&ctx, MenuAction::ShowInGrid(Arc::new(vec![1])), size);
        assert_eq!(selected(&app), 1);
        assert_eq!(app.library_mode, LibraryMode::Grid);

        app.apply_map_action(&ctx, MenuAction::PlaceHere((38.8, -9.4)), size);
        let at = |app: &RustroomApp, i| {
            app.catalog
                .as_ref()
                .unwrap()
                .location_of(&path(app, i))
                .unwrap()
        };
        assert_eq!(at(&app, 1), Some((38.8, -9.4)), "the selection was placed");
        assert_eq!(at(&app, 0), None);
        app.apply_map_action(&ctx, MenuAction::Remove(Arc::new(vec![1])), size);
        assert_eq!(at(&app, 1), None);

        app.apply_map_action(&ctx, MenuAction::ZoomHere((10.0, 20.0)), size);
        assert_eq!(app.map.camera().map(|c| c.center), Some((10.0, 20.0)));
    }
}
