//! The Library grid: a page of thumbnails, filtered by format, with selection.

mod band;
mod cell;
mod cell_menu;
mod grid;
mod keys;

use crate::theme::Hover as _;
use crate::{name_edit, RustroomApp};
pub use cell_menu::{apply_cell_action, cell_menu, distinct_files, menu_targets, CellAction};
use eframe::egui;
use rustroom_app::photo_id::PhotoId;

/// What an empty Library says. Also swept by `theme::every_menu_string_is_renderable`.
pub(crate) const EMPTY_LIBRARY_TEXT: &str =
    "You can also drop a folder or some photos onto this window. Nothing is copied or changed: \
     Rustroom reads your photos where they are and keeps its edits beside them.";

/// The empty Library: two real buttons and one sentence, where the first-use review found a
/// footnote in a void (A-08).
fn empty_library(ui: &mut egui::Ui) -> Option<rustroom_app::menu::Command> {
    let mut picked: Option<rustroom_app::menu::Command> = None;
    ui.vertical_centered(|ui| {
        ui.add_space((ui.available_height() * 0.35).max(24.0));
        ui.label(egui::RichText::new("No photos yet").heading());
        ui.add_space(12.0);
        let big = |text: &str| {
            egui::Button::new(egui::RichText::new(text).size(crate::theme::tokens::TITLE))
                .min_size(egui::vec2(240.0, 36.0))
        };
        if ui
            .add(big("Open a folder…"))
            .hover("Opens a folder of photos (⇧⌘O)")
            .clicked()
        {
            picked = Some(rustroom_app::menu::Command::OpenFolder);
        }
        ui.add_space(6.0);
        if ui
            .add(big("Import from a card…"))
            .hover("Copies new photos from a memory card into dated folders (⇧⌘I)")
            .clicked()
        {
            picked = Some(rustroom_app::menu::Command::Import);
        }
        ui.add_space(14.0);
        ui.set_max_width(460.0);
        crate::theme::hint(ui, EMPTY_LIBRARY_TEXT);
    });
    picked
}

/// What one frame of the grid collected, applied after the panel closes: every one of these needs
/// `&mut RustroomApp`, which the panel cannot have while it holds `&mut app.library`.
#[derive(Default)]
struct GridOutcome {
    open_index: Option<usize>,
    /// Delete or Backspace in the grid: the selection leaves the collection on screen, if one is.
    remove_from_collection: bool,
    /// ⚠ Declared out here like `open_index`: the empty state's buttons run a command that needs
    /// `&mut RustroomApp`, which the closure cannot have while it borrows fields of it.
    empty_pick: Option<rustroom_app::menu::Command>,
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
}

/// Pinch, or ctrl/cmd+scroll, resizes the grid; the size is saved once the gesture settles.
fn apply_pinch(app: &mut RustroomApp, ui: &egui::Ui) {
    // Pinch on a trackpad, or ctrl/cmd+scroll, resizes the grid — the same gesture the develop
    // canvas uses to zoom the photo.
    //
    // ⚠ `zoom_delta` folds trackpad pinch *and* ctrl+scroll into one **multiplicative factor
    // that is 1.0 when idle**, which is why plain scrolling still scrolls the grid rather than
    // resizing it.
    let zoom = ui.ctx().input(|i| i.zoom_delta());
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
    let live = app.live_edits();
    // ⚠ Borrowed out of `app` up front, because the grid closure already holds `&mut app.library`
    // and the two are disjoint fields — which the borrow checker only accepts when it can see so.
    let rename_draft = &mut app.rename_draft;
    let label_draft = &mut app.label_draft;
    // ⚠ Same reason: the context menu reads the catalog while the closure holds `&mut
    // app.library`, and the borrow checker only accepts that when the two fields are visibly
    // disjoint.
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
    if let Some(cmd) = out.empty_pick {
        crate::commands::execute(app, ctx, cmd);
    }
    if let Some((id, stars)) = out.star_click {
        app.apply_mark_to(&[id], rustroom_app::marks::MarkAction::Rating(stars));
    }
    if let Some(i) = out.open_index {
        app.open_entry(ctx, i);
    }
}
