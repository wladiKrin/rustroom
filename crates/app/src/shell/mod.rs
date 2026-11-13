//! The frame loop: the top toolbar, the Library/Develop dispatch, and the per-frame pumps.
//!
//! ⚠ This is a **trait impl moved to another module of the same crate**, which Rust allows —
//! so unlike every other move in this refactor, `self` stays `self` and the body is verbatim.

mod centre_info;
mod filter;
pub(crate) mod keys;
mod landing;
pub(crate) mod top_bar;

use crate::{
    commands, develop, library, menu_bar, modals, multi, name_edit, AppView, LibraryMode,
    RustroomApp,
};
use eframe::egui;
use keys::pressed_command;
pub(crate) use landing::clipping_texture;
use rustroom_app::menu::Command;
use rustroom_app::photo_id::PhotoId;

impl eframe::App for RustroomApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.open_dropped_files(&ctx);

        // Workaround for an upstream winit/macOS teardown bug: when the window closes,
        // AppKit's Touch Bar responder tracking (`_NSTouchBarFinderObservation`) removes a
        // KVO observer for "nextResponder" from `WinitView` with an imbalanced count,
        // throwing an uncaught `NSRangeException` (SIGABRT). Exiting here — before that
        // deferred observer invalidation runs during teardown — gives a clean exit.
        #[cfg(target_os = "macos")]
        if ctx.input(|i| i.viewport().close_requested()) {
            self.persist_current_session();
            std::process::exit(0);
        }

        self.reset_view_scoped_state();
        self.pump_loaders(&ctx);
        self.handle_input(&ctx);
        // M11.4: reopen where you left off on the first frame, then follow the Library.
        self.pump_place(&ctx);
        self.pump_jobs(&ctx);

        // Pull any finished preview frame into a texture.
        self.pump_render_frame(&ctx);

        // Pull finished thumbnails into textures.
        self.pump_thumbnails(&ctx);

        // Pull finished capture metadata into the catalog.
        //
        // ⚠ Written as results arrive rather than batched at the end, so a folder closed half way
        // keeps what was read.
        self.pump_captures();
        self.pump_location_pass(&ctx);

        self.top_panel(ui, &ctx);
        self.view_body(ui, &ctx);

        develop::ambient::draw(self, &ctx);
        modals::draw(self, &ctx);

        // A keyword, collection, smart-collection or date view follows the catalog. ⚠ Last, after
        // everything this frame could write — the panels apply their clicks after drawing.
        self.pump_source_membership(&ctx);
    }
}

impl RustroomApp {
    /// What the menus' check marks reflect (M10.8b).
    pub(crate) fn menu_state(&self) -> rustroom_app::menu::MenuState {
        rustroom_app::menu::MenuState {
            filmstrip_shown: self.show_filmstrip,
        }
    }

    /// Open a folder or photos dropped on the window.
    fn open_dropped_files(&mut self, ctx: &egui::Context) {
        // ⚠ A folder or photos dropped on the window open, the way the empty Library promises.
        let dropped: Vec<std::path::PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        if !dropped.is_empty() {
            self.open_dropped(ctx, dropped);
        }
    }

    /// Drop what belongs to the view just left: its status line and its drafts.
    fn reset_view_scoped_state(&mut self) {
        // ⚠ The status line is per view. "Full resolution ready" or "Decoding …" following the
        // user into the Library is noise there — it describes work in a view they have left.
        if self.view != self.last_view {
            self.status.clear();
            // ⚠⚠ **The rename draft is per view too, and this is a real bug fix.** The Develop
            // panel draws its name field on *every* frame, so `get_or_insert_with` leaves a draft
            // behind for whatever photo was open. Harmless there — autofocus is deliberately off
            // in Develop — but the Library grid draws the field for whichever row owns the draft
            // **and autofocuses it**, so returning to the Library put you in a text box you never
            // asked for, with the keyboard captured.
            //
            // ⚠ Discarded rather than committed: renaming is explicit, and navigating away is not
            // a way to confirm a half-typed name.
            self.rename_draft = None;
            self.info_draft = None;
            self.snapshot_rename = None;
            self.last_view = self.view;
        }
    }

    /// Advance the decode, full-resolution and preview loaders.
    fn pump_loaders(&mut self, ctx: &egui::Context) {
        self.pump_decodes(ctx);
        self.maybe_request_full_res(ctx);
        self.pump_full_res(ctx);
        // M9.14: preview textures land, builds advance, and Develop pins its stand-in.
        self.pump_previews(ctx);
        self.pump_preview_builds(ctx);
        if self.view == AppView::Develop {
            self.sync_develop_previews();
        }
    }

    /// Menu activations, then the clipboard, then shortcuts, then culling — in that order.
    fn handle_input(&mut self, ctx: &egui::Context) {
        // Outside the view match: Ctrl+V must reach a selection made in the Library grid,
        // not only one made in the filmstrip.
        // ⚠ Before the key handling, so a menu activation and a keypress in the same frame
        // are applied in the order they arrived.
        #[cfg(target_os = "macos")]
        for cmd in crate::native_menu::poll() {
            commands::execute(self, ctx, cmd);
        }
        // M10.8b: the system menu's check marks follow the state — AppKit is touched only on a change.
        #[cfg(target_os = "macos")]
        crate::native_menu::sync(&self.menu_state());
        self.handle_clipboard_keys(ctx);
        // ⚠ After `handle_clipboard_keys`, which owns ⌘C / ⌘V for the reason in
        // `pressed_command`'s doc comment.
        // ⚠ Not while a dialog is open: ⌘1 zoomed the canvas and `\` toggled Before/After behind
        // the Export dialog (P-04). The dialog owns the keyboard until it closes.
        if matches!(self.modal, crate::modals::Modal::None) {
            if let Some(cmd) = pressed_command(ctx, &self.settings.shortcuts) {
                commands::execute(self, ctx, cmd);
            }
        }
        // ⚠ After the shortcuts, so a digit some menu item claimed would win — none does now that
        // the tools sit on letters, which is the point: rating in Develop.
        self.handle_develop_culling(ctx);
    }

    /// Advance the export, import, AI and Immich jobs.
    fn pump_jobs(&mut self, ctx: &egui::Context) {
        self.pump_export(ctx);
        self.pump_print(ctx);
        self.pump_camera_learning(ctx);
        self.pump_publish(ctx);
        self.pump_import(ctx);
        self.pump_hdr(ctx);
        self.pump_pano(ctx);
        self.pump_merges(ctx);
        self.pump_tether(ctx);
        self.pump_face_pass(ctx);
        self.pump_people_filter();
        self.pump_ai(ctx);
        self.pump_denoise(ctx);
        self.pump_removals(ctx);
        // ⚠ Beside the other pumps, so an album list or a finished download lands on the next
        // frame rather than waiting for whatever the user does next.
        self.pump_immich(ctx);
    }

    /// The top panel: the in-window menu bar and the top bar, with their command and rename applied
    /// after the panel closes.
    fn top_panel(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        // ⚠ Applied after the panel closure closes: `commands::execute` takes `&mut self`
        // and the closure already holds it. The same deferred-action shape the preset list
        // uses.
        let mut pending_command: Option<Command> = None;
        // ⚠ The bar's rename, committed after the panel closes for the reason above: the
        // commit writes the sidecar and rebuilds the Library with `&mut self`.
        let mut pending_rename: Option<(PhotoId, name_edit::Rename)> = None;
        egui::Panel::top("top").show(ui, |ui| {
            // ⚠ macOS gets the real system menu bar instead; drawing this one too would put a
            // redundant row under a nearly-empty system menu.
            if !cfg!(target_os = "macos") {
                let state = self.menu_state();
                if let Some(cmd) = menu_bar::show(
                    ui,
                    &self.settings.recent_folders,
                    &self.settings.shortcuts,
                    &state,
                ) {
                    pending_command = Some(cmd);
                }
            }
            let (row_rename, row_command, _) = self.top_bar(ui, ctx);
            pending_rename = row_rename;
            if row_command.is_some() {
                pending_command = row_command;
            }
        });

        if let Some(cmd) = pending_command {
            commands::execute(self, ctx, cmd);
        }
        if let Some((id, r)) = pending_rename {
            name_edit::commit(self, ctx, &id, r);
        }
    }

    /// The view's body: Develop, or the Library's rail, status row and current layout.
    fn view_body(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        match self.view {
            AppView::Develop => develop::body(self, ui, ctx),
            AppView::Library => {
                // ⚠ Panel order is load-bearing: egui allocates side panels in call order and
                // the central panel takes what is left, so the collections panel must be drawn
                // *before* the grid.
                if self.show_panels {
                    crate::collections_panel::show(self, ui, ctx);
                }
                // ⚠ After the rail and before the view, so it spans the content column — the
                // status row informs and sets the view (`docs/DESIGN.md` §6).
                crate::status_bar::library(self, ui, ctx);
                // ⚠ Exhaustive, so a fourth Library layout is a compile error here rather than
                // a mode that silently shows the grid — the rule `commands::execute`'s match
                // already follows.
                self.release_compare_panes();
                match self.library_mode {
                    LibraryMode::Grid => library::show(self, ui, ctx),
                    LibraryMode::Compare => multi::compare::show(self, ui, ctx),
                    LibraryMode::Survey => multi::survey::show(self, ui, ctx),
                    LibraryMode::Map => crate::map_view::show(self, ui, ctx),
                    LibraryMode::People => crate::people_view::show(self, ui, ctx),
                }
            }
        }
    }
}
