//! The frame loop: the top toolbar, the Library/Develop dispatch, and the per-frame pumps.
//!
//! ⚠ This is a **trait impl moved to another module of the same crate**, which Rust allows —
//! so unlike every other move in this refactor, `self` stays `self` and the body is verbatim.

mod centre_info;
mod filter;
mod keys;
mod landing;
mod top_bar;

use crate::{
    commands, develop, library, menu_bar, modals, multi, name_edit, AppView, LibraryMode,
    RustroomApp,
};
use eframe::egui;
use keys::pressed_command;
pub(crate) use landing::clipping_texture;
use rustroom_app::menu::Command;
use rustroom_app::photo_id::PhotoId;
use rustroom_app::render_engine::RegionRect;

impl eframe::App for RustroomApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        // ⚠ A folder or photos dropped on the window open, the way the empty Library promises.
        let dropped: Vec<std::path::PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .filter_map(|f| f.path.clone())
                .collect()
        });
        if !dropped.is_empty() {
            self.open_dropped(&ctx, dropped);
        }

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
        self.pump_decodes(&ctx);
        self.maybe_request_full_res(&ctx);
        self.pump_full_res(&ctx);
        // M9.14: preview textures land, builds advance, and Develop pins its stand-in.
        self.pump_previews(&ctx);
        self.pump_preview_builds(&ctx);
        if self.view == AppView::Develop {
            self.sync_develop_previews();
        }
        // Outside the view match: Ctrl+V must reach a selection made in the Library grid,
        // not only one made in the filmstrip.
        // ⚠ Before the key handling, so a menu activation and a keypress in the same frame
        // are applied in the order they arrived.
        #[cfg(target_os = "macos")]
        for cmd in crate::native_menu::poll() {
            commands::execute(self, &ctx, cmd);
        }
        self.handle_clipboard_keys(&ctx);
        // ⚠ After `handle_clipboard_keys`, which owns ⌘C / ⌘V for the reason in
        // `pressed_command`'s doc comment.
        // ⚠ Not while a dialog is open: ⌘1 zoomed the canvas and `\` toggled Before/After behind
        // the Export dialog (P-04). The dialog owns the keyboard until it closes.
        if matches!(self.modal, crate::modals::Modal::None) {
            if let Some(cmd) = pressed_command(&ctx) {
                commands::execute(self, &ctx, cmd);
            }
        }
        // ⚠ After the shortcuts, so a digit some menu item claimed would win — none does now that
        // the tools sit on letters, which is the point: rating in Develop.
        self.handle_develop_culling(&ctx);
        self.pump_export(&ctx);
        self.pump_import(&ctx);
        self.pump_ai(&ctx);
        // ⚠ Beside the other pumps, so an album list or a finished download lands on the next
        // frame rather than waiting for whatever the user does next.
        self.pump_immich(&ctx);

        // Pull any finished preview frame into a texture.
        if let Some(s) = &mut self.session {
            if let Some(frame) = s.engine.poll_latest() {
                // ⚠ Taken from the frame, never recomputed from `frame.rgba` — those bytes are in
                // the display's primaries. See `RenderedFrame::hist`.
                s.hist = frame.hist;
                // ⚠⚠ Kept rather than dropped, and it is the **sRGB** channel, not `rgba`. See
                // `RenderedFrame::srgb`: `rgba` is in the display's primaries, so a readout built
                // on it would report different numbers on a different monitor — the same bug the
                // histogram's ordering already exists to prevent. The dimensions travel with the
                // bytes so nothing has to re-derive them.
                //
                // ⚠ An `Arc::clone`, so this retains no memory the frame did not already hold.
                s.readout_bytes = Some((
                    std::sync::Arc::clone(&frame.srgb),
                    frame.width,
                    frame.height,
                ));
                s.clip_texture = if self.show_clipping {
                    clipping_texture(&ctx, &frame.srgb, frame.width, frame.height)
                } else {
                    None
                };
                // ⚠ Timed under `RUSTROOM_DEBUG_RENDER` (2026-09-26): the worker's `render:` line
                // cannot see what the UI thread spends turning a frame into a texture, and the
                // in-app drag ran ~11 ms slower than the headless probe with no line saying where.
                let t_tex = std::time::Instant::now();
                // ⚠ PREMULTIPLIED, because every frame is opaque (the GPU quantiser ORs in
                // 0xff000000; `to_rgba8` is pinned opaque by `to_rgba8_is_to_rgb8_with_an_opaque_alpha`),
                // and at alpha 255 the two are the same pixels — without the per-pixel conversion
                // that cost 5.5 ms of the UI thread per 6.5 Mpx drag frame.
                let img = egui::ColorImage::from_rgba_premultiplied(
                    [frame.width, frame.height],
                    &frame.rgba,
                );
                let t_load = std::time::Instant::now();
                // ⚠⚠ **Updated in place when a whole frame follows a whole frame of the same size**
                // (2026-09-26): a NEW 27 MB texture per drag frame is an allocation plus an upload
                // on the window's own GPU device, competing with the render pipeline's. ⚠ Only
                // whole-after-whole: `backdrop` is a clone of this very handle, and a region frame
                // written into it would turn the backdrop into the slice it exists to surround — so
                // a region frame (or the first whole frame after one) still gets a fresh texture.
                let reuse = frame.region.is_none()
                    && s.rendered_region.is_none()
                    && s.texture
                        .as_ref()
                        .is_some_and(|t| t.size() == [frame.width, frame.height]);
                // ⚠ MEASUREMENT SWITCH (2026-09-26): with `RUSTROOM_DEBUG_NO_UPLOAD` set, a new frame
                // never reaches the window's GPU once a picture is on screen — the window keeps
                // drawing the old one. It exists to answer ONE question: does the window's handling
                // of new frames cost the render pipeline GPU time (the ~12 ms app-vs-probe gap)?
                let frozen =
                    s.texture.is_some() && std::env::var_os("RUSTROOM_DEBUG_NO_UPLOAD").is_some();
                match (&mut s.texture, reuse) {
                    _ if frozen => {}
                    (Some(t), true) => t.set(img, egui::TextureOptions::LINEAR),
                    _ => {
                        s.texture =
                            Some(ctx.load_texture("preview", img, egui::TextureOptions::LINEAR));
                    }
                }
                if std::env::var_os("RUSTROOM_DEBUG_RENDER").is_some() {
                    eprintln!(
                        "  UI texture: convert {:5.1} ms  load {:5.1} ms  {}x{}",
                        (t_load - t_tex).as_secs_f64() * 1e3,
                        t_load.elapsed().as_secs_f64() * 1e3,
                        frame.width,
                        frame.height
                    );
                }
                // ⚠ A whole-image frame is kept as the backdrop a region render is painted over.
                // ⚠ `Some(..)` only — a region frame must NOT replace it, or the backdrop would
                // become the very slice it exists to surround and the black would come back.
                if frame.region.is_none() {
                    s.backdrop = s.texture.clone().map(|t| (t, frame.fit_size));
                }
                // ⚠ The grade range view's plane becomes a texture in the same place the frame
                // does, so the two can never describe different frames — and a frame that
                // arrives without a plane (box unticked, Global, a tool switch) drops the
                // texture in this same assignment. No separate clear.
                s.grade_view_texture = frame.grade_view.as_ref().map(|plane| {
                    let img = egui::ColorImage::from_rgba_unmultiplied(
                        [frame.width, frame.height],
                        &rustroom_app::grade_view::tint_rgba(plane),
                    );
                    ctx.load_texture("grade_view", img, egui::TextureOptions::LINEAR)
                });
                s.rendered_region = frame.region.map(|r| RegionRect {
                    x: r.x0,
                    y: r.y0,
                    w: frame.width,
                    h: frame.height,
                });
                // ⚠⚠ **Read from the frame, not recomputed here.** This used to be
                // `frame_dims x fit_cap / frame.max_dim`. A frame's dimensions come from
                // `downscale_to`, which rounds, so that form amplified the rounding by
                // `rung / fit_cap` and made the placement depend on which rung last landed —
                // measured on an uncropped X-T4 frame at 1068.000 against a true 1068.204.
                // `RenderedFrame::fit_size` is computed from the buffer and the view geometry, so
                // it is exact and identical at every rung.
                //
                // ⚠ It is carried on the frame rather than derived here from `s.edits`, which
                // would also be exact and would be worse: it would describe a crop the frame on
                // screen does not have until the next one lands, misplacing the texture for a
                // frame or two after every crop change. Travelling with the pixels it describes
                // is what makes that unrepresentable.
                //
                // ⚠ The finding that made the old code take region frames as well as whole-image
                // ones still holds and is now structural: **every** frame carries this, so the
                // distinction has stopped existing here. The symptom it fixed is worth keeping —
                // entering geometry mode with a crop applied resets the crop for the preview, so
                // the frame grows; with only region frames arriving, a whole-image-only rule left
                // `fit_image_size` describing the *cropped* frame, `frame_rect` was the wrong size
                // and the guide overlay sat slightly off, correcting itself only when you zoomed
                // out far enough for a whole-image frame to land.
                s.fit_image_size = egui::vec2(frame.fit_size.0, frame.fit_size.1);
                // A region frame carries the full post-geometry dimensions it was cut from,
                // so this is exact either way.
                s.rendered_full = frame
                    .region
                    .map(|r| (r.full_w, r.full_h))
                    .unwrap_or((frame.width, frame.height));
                s.rendered_long = s.rendered_full.0.max(s.rendered_full.1) as f32;
                // ⚠ The overlay may now describe this frame. The engine coalesces superseded
                // requests, so the frame that lands is the newest one asked for.
                s.shown_geo = s.pending_geo.get();
            }
        }

        // Pull finished thumbnails into textures.
        if let Some(lib) = &mut self.library {
            for res in lib.loader.drain() {
                if let Some(t) = res.thumb {
                    let bytes = t.rgba.len();
                    let img =
                        egui::ColorImage::from_rgba_unmultiplied([t.width, t.height], &t.rgba);
                    // ⚠ The texture name is derived from the **key**, not from an index: two
                    // different photos sharing a name would have egui serve one image for both,
                    // and now two renders of one photo with different edits would too.
                    let tex = ctx.load_texture(
                        format!(
                            "thumb:{}#{}:{}",
                            res.key.id.path.display(),
                            res.key.id.copy,
                            res.key.edits
                        ),
                        img,
                        egui::TextureOptions::LINEAR,
                    );
                    lib.cache.insert(res.key, tex, bytes);
                }
            }
        }

        // Pull finished capture metadata into the catalog.
        //
        // ⚠ Written as results arrive rather than batched at the end, so a folder closed half way
        // keeps what was read.
        {
            let done = self.metadata.drain();
            if let Some(cat) = &self.catalog {
                for r in &done {
                    if let Err(e) = cat.set_capture(&r.path, &r.capture) {
                        eprintln!("catalog: {e}");
                    }
                }
            }
            // ⚠⚠ Refresh the Library's capture map, or a folder that has just been scanned stays
            // unfilterable until something unrelated rebuilds — the `reload_marks` refilter rule,
            // one layer along.
            //
            // ⚠ Guarded on there being results: this runs every frame, and re-querying every
            // folder sixty times a second is the `apply_view_demosaic` mistake in yet another
            // costume.
            if !done.is_empty() {
                self.reload_captures();
            }
        }

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
                if let Some(cmd) = menu_bar::show(ui, &self.settings.recent_folders) {
                    pending_command = Some(cmd);
                }
            }
            let (row_rename, row_command, _) = self.top_bar(ui, &ctx);
            pending_rename = row_rename;
            if row_command.is_some() {
                pending_command = row_command;
            }
        });

        if let Some(cmd) = pending_command {
            commands::execute(self, &ctx, cmd);
        }
        if let Some((id, r)) = pending_rename {
            name_edit::commit(self, &ctx, &id, r);
        }

        match self.view {
            AppView::Develop => develop::body(self, ui, &ctx),
            AppView::Library => {
                // ⚠ Panel order is load-bearing: egui allocates side panels in call order and
                // the central panel takes what is left, so the collections panel must be drawn
                // *before* the grid.
                if self.show_panels {
                    crate::collections_panel::show(self, ui, &ctx);
                }
                // ⚠ After the rail and before the view, so it spans the content column — the
                // status row informs and sets the view (`docs/DESIGN.md` §6).
                crate::status_bar::library(self, ui, &ctx);
                // ⚠ Exhaustive, so a fourth Library layout is a compile error here rather than
                // a mode that silently shows the grid — the rule `commands::execute`'s match
                // already follows.
                self.release_compare_panes();
                match self.library_mode {
                    LibraryMode::Grid => library::show(self, ui, &ctx),
                    LibraryMode::Compare => multi::compare::show(self, ui, &ctx),
                    LibraryMode::Survey => multi::survey::show(self, ui, &ctx),
                }
            }
        }

        develop::ambient::draw(self, &ctx);
        modals::draw(self, &ctx);
    }
}
