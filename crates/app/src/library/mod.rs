//! The Library grid: a page of thumbnails, filtered by format, with selection.

mod band;
mod cell;
mod cell_menu;
mod grid;
mod keys;

use crate::{name_edit, RustroomApp};
pub use cell_menu::{apply_cell_action, cell_menu, distinct_files, menu_targets, CellAction};
use eframe::egui;
use rustroom_app::photo_id::PhotoId;

/// What one frame of the grid collected, applied after the panel closes: every one of these needs
/// `&mut RustroomApp`, which the panel cannot have while it holds `&mut app.library`.
#[derive(Default)]
struct GridOutcome {
    open_index: Option<usize>,
    /// Delete or Backspace in the grid: the selection leaves the collection on screen, if one is.
    remove_from_collection: bool,
    /// ⚠ Declared out here like `open_index`: the start page's clicks need `&mut RustroomApp`, which the
    /// closure cannot have while it borrows fields of it (M11.4).
    start_pick: Option<crate::start_page::StartPick>,
    /// ⚠ Same shape: a star clicked on a thumbnail rates through `apply_mark_to`, which needs
    /// `&mut RustroomApp` once the closure has let go of the library.
    star_click: Option<(PhotoId, u8)>,
    /// ⚠ Collected and applied after the closure, the shape every action in this file uses: the
    /// closure holds `lib`, and all three of these need `&mut app`.
    menu: Option<(usize, CellAction)>,
    menu_select: Option<usize>,
    /// ⚠ Only the cells actually drawn this frame. The grid is virtualised, so a band can reach
    /// only what is on screen — which is also all it can cover, since it cannot leave the viewport.
    cell_rects: Vec<(usize, egui::Rect)>,
    band_started: Option<egui::Pos2>,
    scroll_now: Option<f32>,
    bg_clicked: bool,
    /// ⚠ Collected here and applied after the panel closes: committing needs `&mut RustroomApp`
    /// while the closure below still holds `&mut app.library`. Same shape `open_index` uses.
    rename: Option<(PhotoId, name_edit::Rename)>,
    /// ⚠ Collected inside the grid closure and applied after it: opening a draft writes
    /// `app.label_draft`, which the closure cannot reach while it holds `&mut app.library`.
    badge_clicked: Option<(PhotoId, String)>,
    /// The stack whose expanded state a click asked to flip.
    ///
    /// ⚠ Collected rather than applied: `lib` is borrowed through the grid closure, and flipping it
    /// must trigger a **rebuild** of `entries` rather than patch the list the closure is indexing.
    stack_toggle: Option<rustroom_app::stack::StackKey>,
    /// A photo dropped onto another member of its unfurled stack: `(dragged, onto)` (M10.8c).
    stack_move: Option<(PhotoId, PhotoId)>,
    /// ⚠ Declared out here, like `open_index` and `rename`: applying it needs `&mut RustroomApp`
    /// while the closure below still holds `&mut app.library`.
    /// ⚠⚠ **The targets travel WITH the action, and that is the whole safety of auto-advance.**
    /// The advance below moves the selection, and `apply_mark` used to read the targets out of the
    /// live selection *after* the frame — so rating a run of five frames would have rated the wrong
    /// photo, silently. Carrying the ids here makes "a mark without its targets" unrepresentable
    /// rather than something a comment asks the next reader to remember.
    mark: Option<(Vec<PhotoId>, rustroom_app::marks::MarkAction)>,
}

/// What the grid reads from `app` besides the library, borrowed before the panel takes `&mut app.library`.
struct GridInputs<'a> {
    /// ⚠ Read before `lib` takes a `&mut` borrow of `app`, so the grid can ask which assets are on
    /// disk without cloning the set into the draw closure.
    immich_present: &'a std::collections::HashSet<std::ffi::OsString>,
    rename_draft: &'a mut Option<(PhotoId, String)>,
    label_draft: &'a mut Option<(PhotoId, String)>,
    catalog: Option<&'a catalog::db::Catalog>,
    live: &'a Option<(std::path::PathBuf, u32, edit_model::Edits)>,
    cell: f32,
    auto_advance: bool,
    scroll_was: f32,
    /// The live filter, so an empty grid can say which part of it emptied it.
    filter: &'a rustroom_app::library_filter::LibraryFilter,
    /// Immich's asset → stack map, so a stack's badge keys as the grouping did (M10.8c).
    remote: &'a std::collections::HashMap<String, String>,
    /// What reorders an unfurled stack (Settings → Input).
    stack_drag: rustroom_app::settings::StackDrag,
    /// How a collapsed stack is drawn (Settings → Appearance).
    look: rustroom_app::stack::StackLook,
    /// The user's shortcuts (M10.9b) — the start page's hovers name them.
    keys: &'a [rustroom_app::shortcuts::Rebinding],
    /// The start page's Continue rows — empty while a source is open (M11.4).
    start_rows: &'a [rustroom_app::start::ContinueRow],
    /// Whether the start page shows the welcome.
    show_welcome: bool,
    /// A publish service view's states (M10.18d), for the cells' marks.
    publish: Option<
        &'a std::collections::HashMap<
            rustroom_app::photo_id::PhotoId,
            rustroom_app::publish_state::State,
        >,
    >,
}

/// Pinch, or ctrl/cmd+scroll, resizes the grid; the size is saved once the gesture settles.
fn apply_pinch(app: &mut RustroomApp, ui: &egui::Ui) {
    // Pinch on a trackpad, or ctrl/cmd+scroll, resizes the grid — the same gesture the develop
    // canvas uses to zoom the photo.
    //
    // ⚠ `zoom_delta` folds trackpad pinch *and* ctrl+scroll into one **multiplicative factor
    // that is 1.0 when idle**, which is why plain scrolling still scrolls the grid rather than
    // resizing it.
    // ⚠ Not while a dialog is open: the HDR merge preview zooms with the same gesture, and the grid
    // behind it resized too (reported 2026-10-27). The rule the keyboard shortcuts already follow.
    let zoom = if matches!(app.modal, crate::modals::Modal::None) {
        ui.ctx().input(|i| i.zoom_delta())
    } else {
        1.0
    };
    if zoom != 1.0 {
        app.settings.library_cell = app.settings.zoomed_cell(zoom);
        app.cell_zoom_pending = true;
    } else if app.cell_zoom_pending {
        // ⚠ Saved when the gesture *settles*, not per frame: a pinch runs at 60 Hz and writing
        // the settings file that often is the `apply_view_demosaic` mistake in a new costume.
        app.cell_zoom_pending = false;
        app.save_settings();
    }
}

/// The Library grid: thumbnails, selection, double-click / Enter to open in Develop.
pub fn show(app: &mut RustroomApp, ui: &mut egui::Ui, ctx: &egui::Context) {
    let mut out = GridOutcome::default();
    // Where the grid was scrolled to, read out here so it survives the closure that borrows
    // `app.library` and can be written back after it.
    let scroll_was = app.grid_scroll;
    // ⚠ Hoisted before the closure borrows `app.library`, like `scroll_was`: reading
    // `app.settings` inside would be a second borrow of `app`.
    let auto_advance = app.settings.auto_advance;
    apply_pinch(app, ui);

    // ⚠ Hoisted out of `app` before the closure borrows `app.library`: the open photo's edits
    // live in `Session` and the sidecar is stale while it is being edited, so the thumbnail must
    // use these rather than the row's stored copy.
    //
    // ⚠ **Before any `&mut app.*` binding below**: `live_edits` takes `&self`, so it borrows the
    // whole of `app` and cannot follow a mutable borrow of one of its fields.
    // M10.18d: derived here, once per change, before the grid borrows the library.
    app.publish_view_states();
    // ⚠ Before the central panel, which takes what is left: the band spans the grid's column only.
    crate::publish_panel::band(app, ui);
    let live = app.live_edits();
    // ⚠ Borrowed out of `app` up front, because the grid closure already holds `&mut app.library`
    // and the two are disjoint fields — which the borrow checker only accepts when it can see so.
    let rename_draft = &mut app.rename_draft;
    let label_draft = &mut app.label_draft;
    // ⚠ Same reason: the context menu reads the catalog while the closure holds `&mut
    // app.library`, and the borrow checker only accepts that when the two fields are visibly
    // disjoint.
    // M11.4: the start page's rows, only while it shows — at most a handful of `stat`s, never per photo.
    let start_rows = if app.library.is_none() {
        let all = app.rail_data.all(&app.immich_present);
        rustroom_app::start::continue_rows(&app.settings.recent_folders, &app.rail_data, all, |p| {
            p.is_dir()
        })
    } else {
        Vec::new()
    };
    let catalog = app.catalog.as_ref();
    // ⚠ From the settings, not a constant. Everything below is expressed as a fraction of it so
    // the thumbnail, the cell and the name all grow together — a fixed caption beside a scaling
    // thumbnail is what makes a size control look broken at the extremes.
    let cell = app.settings.clamped_cell();
    let inp = GridInputs {
        immich_present: &app.immich_present,
        rename_draft,
        label_draft,
        catalog,
        live: &live,
        cell,
        auto_advance,
        scroll_was,
        filter: &app.filter,
        remote: &app.immich_stacks.of_asset,
        stack_drag: app.settings.stack_drag,
        look: app.settings.stacking.look,
        keys: &app.settings.shortcuts,
        start_rows: &start_rows,
        show_welcome: app.settings.show_welcome,
        publish: app.publish.view.as_ref().map(|v| &v.states),
    };
    egui::CentralPanel::default().show(ui, |ui| grid::panel(ui, &mut app.library, inp, &mut out));

    if let Some((targets, a)) = out.mark {
        app.apply_mark_to(&targets, a);
    }
    if out.remove_from_collection {
        app.remove_selection_from_viewed_collection();
    }
    // ⚠⚠ Applied by **rebuilding** the entry list, never by patching it: the grid, the filmstrip,
    // `selection` and `view_region` all index into `lib.entries`, and `entries`' own doc records
    // three sparse-index bugs on exactly that. A rebuild is measured at 4.6 ms for 830 files.
    if let Some((from, onto)) = out.stack_move.take() {
        app.move_in_stack(&from, &onto);
    }
    if let Some(k) = out.stack_toggle {
        if let Some(lib) = app.library.as_mut() {
            if !lib.stack_expanded.remove(&k) {
                lib.stack_expanded.insert(k);
            }
        }
        app.rebuild_entries(None);
    }
    if let Some((id, text)) = out.badge_clicked {
        // ⚠ Opening the label draft closes any file-rename draft: two open fields in one cell
        // would both claim the same space, and the second would be invisible.
        app.rename_draft = None;
        app.label_draft = Some((id, text));
    }
    if let Some((id, r)) = out.rename {
        name_edit::commit(app, ctx, &id, r);
    }
    // ── Rubber-band selection ────────────────────────────────────────────────────────────────
    //
    // ⚠ Handled after the grid closure, where `lib` is free again and the painter can draw over
    // the cells. The rects were collected inside it.
    band::rubber_band(
        ui,
        app.library.as_mut(),
        &out.cell_rects,
        out.band_started,
        out.bg_clicked,
    );

    if let Some(y) = out.scroll_now {
        app.grid_scroll = y;
    }
    if let Some(i) = out.menu_select {
        if let Some(lib) = app.library.as_mut() {
            lib.selection.click(i);
        }
    }
    if let Some((i, action)) = out.menu {
        apply_cell_action(app, ctx, i, action);
    }
    if let Some(pick) = out.start_pick {
        apply_start_pick(app, ctx, pick);
    }
    if let Some((id, stars)) = out.star_click {
        app.apply_mark_to(&[id], rustroom_app::marks::MarkAction::Rating(stars));
    }
    if let Some(i) = out.open_index {
        app.open_entry(ctx, i);
    }
}

/// What a click on the start page does (M11.4).
pub(crate) fn apply_start_pick(
    app: &mut RustroomApp,
    ctx: &egui::Context,
    pick: crate::start_page::StartPick,
) {
    use crate::start_page::StartPick;
    match pick {
        StartPick::Command(cmd) => crate::commands::execute(app, ctx, cmd),
        StartPick::AllPhotos => app.open_all_photos(ctx),
        StartPick::Folder(dir) => app.open_folder_at(ctx, dir),
        StartPick::HideWelcome => {
            app.settings.show_welcome = false;
            app.save_settings();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pinch(app: &mut RustroomApp) {
        let ctx = crate::test_ui::ctx();
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(crate::test_ui::screen()),
                events: vec![egui::Event::Zoom(1.5)],
                ..Default::default()
            },
            |ui| apply_pinch(app, ui),
        );
    }

    /// The control: with nothing open, a pinch resizes the grid.
    #[test]
    fn a_pinch_resizes_the_grid() {
        let mut app = crate::test_app::for_test();
        let before = app.settings.library_cell;
        pinch(&mut app);
        assert!(app.settings.library_cell > before);
    }

    /// A pinch meant for a dialog — the HDR merge preview zooms with it — leaves the grid alone
    /// (reported 2026-10-27: zooming the preview resized the icons behind it).
    #[test]
    fn a_pinch_with_a_dialog_open_leaves_the_grid_alone() {
        let mut app = crate::test_app::for_test();
        app.modal = crate::modals::Modal::HdrMerge;
        let before = app.settings.library_cell;
        pinch(&mut app);
        assert_eq!(app.settings.library_cell, before);
    }
}
