//! The frame loop: the top toolbar, the Library/Develop dispatch, and the per-frame pumps.
//!
//! ⚠ This is a **trait impl moved to another module of the same crate**, which Rust allows —
//! so unlike every other move in this refactor, `self` stays `self` and the body is verbatim.

use crate::segmented::{self, PillItem, PillStyle};
use crate::theme;
use crate::{
    commands, develop, library, menu_bar, modals, multi, name_edit, AppView, LibraryMode,
    RustroomApp,
};
use eframe::egui;
use rustroom_app::format_filter;
use rustroom_app::icons;
use rustroom_app::menu::{self, Command};
use rustroom_app::photo_id::PhotoId;
use rustroom_app::render_engine::RegionRect;

impl eframe::App for RustroomApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();

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
        if let Some(cmd) = pressed_command(&ctx) {
            commands::execute(self, &ctx, cmd);
        }
        self.pump_export(&ctx);

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
                let img = egui::ColorImage::from_rgba_unmultiplied(
                    [frame.width, frame.height],
                    &frame.rgba,
                );
                s.texture = Some(ctx.load_texture("preview", img, egui::TextureOptions::LINEAR));
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
                if let Some(cmd) = menu_bar::show(ui) {
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
                crate::collections_panel::show(self, ui, &ctx);
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

        modals::draw(self, &ctx);
    }
}

impl RustroomApp {
    /// The bar's one row, at a **fixed** height in both views.
    ///
    /// ⚠⚠ `allocate_ui_with_layout` at `BAR_HEIGHT`, not `horizontal` with a minimum: the
    /// bar changed height between Library and Develop ("it feels like the height of the top
    /// bar also changes when switching", seen in use 2026-09-20), and a row allocated at an
    /// exact size cannot — `the_bar_is_the_same_height_in_both_views` measures it. Every
    /// control centres in the row.
    ///
    /// Returns the bar's file rename, if the centred name committed one, the command its
    /// Export button asked for, and the row's rect.
    /// The rename is applied by the caller after the panel closes: the commit writes the
    /// sidecar and rebuilds the Library with `&mut self`.
    fn top_bar(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
    ) -> (
        Option<(PhotoId, name_edit::Rename)>,
        Option<Command>,
        egui::Rect,
    ) {
        let mut rename = None;
        let mut command = None;
        let r = ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), BAR_HEIGHT),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                view_switch(ui, &mut self.view);
                if self.view == AppView::Develop && self.session.is_some() {
                    ui.separator();
                    let decoding = self.pending.is_some();
                    ui.add_enabled_ui(!decoding, |ui| develop::toolbar(self, ui, ctx));
                }
                // ⚠ On the LEFT, not in the right-aligned cluster below. A text field needs
                // ~180 px and that cluster already carries five controls plus the status line;
                // adding it there squeezes the status out, which is the exact bug the cluster was
                // created to fix. This space is where the Develop toolbar sits, and it is empty
                // in Library.
                if self.view == AppView::Library && self.library.is_some() {
                    ui.separator();
                    // ⚠ A visible control, not keys alone. `C` and `N` are the fast route and
                    // are what a photographer culling a shoot will use — but a feature reachable
                    // only by a key nothing on screen mentions is a feature nobody finds, and
                    // the Filter button's `Filter: ` prefix exists for the same reason.
                    mode_switch(ui, &mut self.library_mode);
                    ui.separator();
                    search_field(self, ui);
                    // ⚠ Beside the search field, not in the right-aligned cluster. They are one
                    // idea — "narrow what I am looking at" — and splitting them across the row put
                    // half of it next to the sort order and the grid size, which are not.
                    mark_filter(self, ui);
                }
                let row = ui.max_rect();
                let gap_left = ui.cursor().left();
                // ⚠ One right-aligned cluster for everything that belongs at the far end, laid
                // out **rightmost first**. Previously the develop toolbar right-aligned the
                // filename itself and then the status label was added after it — which meant
                // the status was pushed into a row that had no space left.
                let gap_right = ui
                    .with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // ⚠ Library only. The filename is **not** shown here: the Develop panel's
                        // footer already carries it, and the same name in two places at once is
                        // noise rather than reassurance.
                        if self.view == AppView::Library {
                            if self.library.is_some() {
                                more_menu(self, ui, ctx);
                            }
                            format_chips(self, ui);
                            if self.library.is_some() {
                                grid_size_slider(self, ui);
                                sort_combo(self, ui);
                                ui.separator();
                            }
                        }
                        // ⚠ Develop's one accent-filled control, at the far right — the same rule
                        // that gives the Library its accent-filled app mode: one per bar.
                        if self.view == AppView::Develop && self.session.is_some() {
                            if export_button(ui).clicked() {
                                command = Some(Command::Export);
                            }
                            ui.separator();
                        }
                        if !self.export_progress.is_empty() {
                            if ui.button("Cancel").clicked() {
                                self.cancel_export();
                            }
                            ui.label(&self.export_progress);
                        } else if !self.status.is_empty() {
                            ui.weak(&self.status);
                        }
                        // ⚠ Its own label, immediately left of the status/progress label, so neither
                        // is ever hidden. ⚠⚠ It must NOT become a second right-aligned region: this
                        // cluster's own comment above records that "the develop toolbar right-aligned
                        // the filename itself and then the status label was added after it — which
                        // meant the status was pushed into a row that had no space left." One
                        // cluster, laid out rightmost first.
                        //
                        // ⚠ Blank when the pointer is off the image, which `canvas::show` clears
                        // every frame — a stale number gets trusted.
                        //
                        // ⚠ Develop only. The Library grid and the Compare/Survey views render
                        // through other paths and have no rendered frame to read.
                        if self.view == AppView::Develop {
                            if let Some(s) = &self.session {
                                if let Some(rgb) = s.readout {
                                    // ⚠⚠ **The swatch is what replaces the `R`/`G`/`B` letters.** It
                                    // answers "which colour" instantly, which no labelling does, and
                                    // it costs less width than three prefixes in the one row this
                                    // project has already had to fix for running out of space.
                                    //
                                    // ⚠ Laid out **rightmost first** like everything in this cluster,
                                    // so the numbers are added before the swatch that precedes them.
                                    // ⚠⚠ **The label occupies a FIXED width, measured from the widest
                                    // label that can occur.** Padding each channel to three characters
                                    // and drawing it monospaced was the first attempt and was not
                                    // enough — reported in use as the swatch still moving when the
                                    // *red* channel changed digit count, which is where a width error
                                    // surfaces: this cluster is laid out right-to-left, so the label's
                                    // right edge is pinned and any discrepancy accumulates leftward,
                                    // into the swatch.
                                    //
                                    // ⚠ Reserving the width **removes the dependency on font metrics
                                    // altogether** rather than assuming padded spaces and digits
                                    // measure alike. The padding and the monospace both stay, because
                                    // together they keep the digits themselves from shifting *inside*
                                    // the reserved box.
                                    let font = egui::TextStyle::Monospace.resolve(ui.style());
                                    // ⚠ Measured through `Painter`, not `Ui::fonts`: the latter hands
                                    // out a shared `&Fonts` while laying text out needs a mutable one
                                    // to populate the glyph atlas. `Painter::layout_no_wrap` takes
                                    // `&self` and does that internally.
                                    let w = ui
                                        .painter()
                                        .layout_no_wrap(
                                            rustroom_app::color_readout::WIDEST_LABEL.to_owned(),
                                            font,
                                            egui::Color32::PLACEHOLDER,
                                        )
                                        .rect
                                        .width();
                                    ui.add_sized(
                                        egui::vec2(w, ui.spacing().interact_size.y),
                                        egui::Label::new(
                                            egui::RichText::new(
                                                rustroom_app::color_readout::label(rgb),
                                            )
                                            .monospace(),
                                        ),
                                    );
                                    let (patch, painter) = ui.allocate_painter(
                                        egui::vec2(READOUT_SWATCH, READOUT_SWATCH),
                                        egui::Sense::hover(),
                                    );
                                    let r = patch.rect;
                                    painter.rect_filled(
                                        r,
                                        egui::CornerRadius::ZERO,
                                        egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]),
                                    );
                                    // ⚠ A hairline border, or a near-black or near-white sample is
                                    // indistinguishable from the panel behind it and the swatch reads
                                    // as missing rather than as dark.
                                    painter.rect_stroke(
                                        r,
                                        egui::CornerRadius::ZERO,
                                        egui::Stroke::new(
                                            1.0,
                                            egui::Color32::from_black_alpha(120),
                                        ),
                                        egui::StrokeKind::Inside,
                                    );
                                }
                            }
                        }
                        // Where the cluster's leftmost control ends: the right edge of the gap.
                        let used = ui.min_rect().left();
                        if used.is_finite() {
                            used
                        } else {
                            row.right()
                        }
                    })
                    .inner;
                rename = centre_info(self, ui, row, egui::Rangef::new(gap_left, gap_right));
            },
        );
        (rename, command, r.response.rect)
    }
}

/// Turn this frame's key presses into a command, using the menu model as the single source of
/// truth for which chord means what.
///
/// ⚠ Gated on nothing having keyboard focus — the rule arrow navigation and the rail already
/// follow. Without it, typing "d" into the Save-preset name field jumps to Develop.
///
/// ⚠ **Not used for ⌘C / ⌘V.** `egui-winit` matches the clipboard chords first and pushes
/// `Event::Copy` / `Event::Paste` *instead of* a `Key` event, so `key_pressed(Key::C)` can
/// never fire — a trap this codebase has already paid for once. Those stay in
/// `handle_clipboard_keys`, which reads the semantic events.
/// The colour swatch beside the numeric readout, in points.
///
/// ⚠ Large enough to judge a colour against rather than merely notice — 11 pt read as a bullet.
const READOUT_SWATCH: f32 = 14.0;

fn pressed_command(ctx: &egui::Context) -> Option<Command> {
    if ctx.memory(|m| m.focused()).is_some() {
        return None;
    }
    for section in menu::model() {
        for item in section.items {
            let menu::Item::Action {
                command,
                shortcut: Some(sc),
                ..
            } = item
            else {
                continue;
            };
            if sc.cmd && matches!(sc.key, 'C' | 'V') {
                continue;
            }
            let Some(key) = egui_key(sc.key) else {
                continue;
            };
            let hit = ctx.input(|i| {
                i.key_pressed(key) && i.modifiers.command == sc.cmd && i.modifiers.shift == sc.shift
            });
            if hit {
                return Some(command);
            }
        }
    }
    None
}

/// The model's key character as an egui key.
///
/// ⚠ Returns `Option` rather than panicking: an unmappable chord should make its menu item
/// mouse-only, not take the app down. `every_shortcut_maps_to_an_egui_key` makes sure none of
/// the shipped ones actually do that.
fn egui_key(c: char) -> Option<egui::Key> {
    Some(match c {
        'O' => egui::Key::O,
        'E' => egui::Key::E,
        'Z' => egui::Key::Z,
        // ⚠ Mapped even though `pressed_command` skips ⌘C / ⌘V. This table is a *total*
        // mapping from the model's key characters to egui keys; whether a given chord is
        // consulted is a separate decision, made there and documented there. A hole here
        // would make the completeness test unable to say what it means.
        'C' => egui::Key::C,
        'V' => egui::Key::V,
        'G' => egui::Key::G,
        'D' => egui::Key::D,
        'N' => egui::Key::N,
        '+' => egui::Key::Plus,
        '-' => egui::Key::Minus,
        '0' => egui::Key::Num0,
        '1' => egui::Key::Num1,
        '2' => egui::Key::Num2,
        '3' => egui::Key::Num3,
        '4' => egui::Key::Num4,
        '5' => egui::Key::Num5,
        ',' => egui::Key::Comma,
        '[' => egui::Key::OpenBracket,
        ']' => egui::Key::CloseBracket,
        '\'' => egui::Key::Quote,
        '\\' => egui::Key::Backslash,
        _ => return None,
    })
}

/// The bar's minimum height in points: the app-mode pill and 4 pt of air.
const BAR_HEIGHT: f32 = 36.0;

/// The app-mode pill's height.
const VIEW_PILL: f32 = 32.0;
/// The Library layout pill's height — a step smaller than the app mode, which it sits inside.
const MODE_PILL: f32 = 28.0;

/// The Library / Develop switch: a 32 pt icon + label pill, its lit half filled with the accent.
///
/// ⚠⚠ **The one accent-filled control in the bar**, so it answers "where am I" at a glance —
/// which two selectable labels at the same weight as every other control did not (the canvas
/// "Rustroom Top Bar & Library", 2026-09-20). The accessible names stay "Library" / "Develop".
fn view_switch(ui: &mut egui::Ui, view: &mut AppView) {
    segmented::pill(
        ui,
        view,
        &[
            PillItem {
                value: AppView::Library,
                icon: icons::LIBRARY,
                label: "Library",
                hover: "Browse the catalog",
            },
            PillItem {
                value: AppView::Develop,
                icon: icons::DEVELOP,
                label: "Develop",
                hover: "Edit the open photo",
            },
        ],
        PillStyle {
            height: VIEW_PILL,
            font: egui::FontId::proportional(13.5),
            accent: true,
        },
    );
}

/// The Grid / Compare / Survey switch: a second pill, 28 pt, with an icon per layout.
///
/// ⚠ A *second* pill rather than three more cells on the first one: Library and Develop are
/// modules and these are layouts within one of them, and flattening the two would say that
/// leaving Compare for Develop is the same kind of move as leaving Compare for the grid. It is
/// shown only in Library, where the distinction is visible — and it takes the button fill, not
/// the accent, for the reason on [`view_switch`].
///
/// ⚠ The chord is in the hover text rather than the label: a segment reading "Grid (G)" is a
/// third of it punctuation, and the Keyboard-shortcuts dialog is generated from the menu model
/// so it already lists all three.
fn mode_switch(ui: &mut egui::Ui, mode: &mut LibraryMode) {
    segmented::pill(
        ui,
        mode,
        &[
            PillItem {
                value: LibraryMode::Grid,
                icon: icons::GRID,
                label: "Grid",
                hover: "Thumbnails (G)",
            },
            PillItem {
                value: LibraryMode::Compare,
                icon: icons::COMPARE,
                label: "Compare",
                hover: "Two photos side by side (C)",
            },
            PillItem {
                value: LibraryMode::Survey,
                icon: icons::SURVEY,
                label: "Survey",
                hover: "The whole selection at once (N)",
            },
        ],
        PillStyle {
            height: MODE_PILL,
            font: egui::FontId::proportional(12.5),
            accent: false,
        },
    );
}

/// The Develop toolbar's Export button: the bar's one accent-filled control.
fn export_button(ui: &mut egui::Ui) -> egui::Response {
    ui.scope(|ui| {
        ui.style_mut().spacing.button_padding.y = (MODE_PILL - 14.0) / 2.0;
        theme::primary_button(ui, &format!("{} Export…", icons::EXPORT), true)
    })
    .inner
    .on_hover_text("Export the selection (⌘E)")
}

/// The ⋯ menu at the bar's right edge: the Library actions that are neither a filter nor a
/// view — today, Reindex.
///
/// ⚠ Behind a menu rather than a button of its own: Reindex is used once per folder, and a
/// once-per-folder verb at the same weight as the sort order was the old bar's failure in
/// miniature.
fn more_menu(app: &mut RustroomApp, ui: &mut egui::Ui, ctx: &egui::Context) {
    let mut reindex = false;
    egui::containers::menu::MenuButton::new(
        egui::RichText::new(icons::MORE.to_string()).size(16.0),
    )
    .ui(ui, |ui| {
        if ui
            .button("Reindex")
            .on_hover_text(
                "Re-read the folder, sort it alphabetically, and close any gaps in the copy \
                 numbering.",
            )
            .clicked()
        {
            reindex = true;
            ui.close();
        }
    })
    .0
    .on_hover_text("More");
    if reindex {
        app.reindex(ctx);
    }
}

/// The Library's format filter: which file formats the grid and filmstrip show.
///
/// ⚠ **Library only.** The filter decides what the browser contains, which is a browsing
/// decision — it has no meaning while a single photo is open, and it sat in the Develop
/// toolbar for no better reason than living in a shared row.
///
/// Rating, flag and colour-label filtering, in one popup.
///
/// ⚠ **One control rather than three.** The row it sits in is a right-to-left cluster already
/// carrying the format bullets, the grid-size buttons, the sort combo, Reindex and the status
/// line; three more controls would squeeze the status out — the exact bug that cluster was
/// created to fix.
///
/// ⚠ The button reports the active state, so a filter is never invisible. A control that hides
/// rows while looking idle is the "invisible state" problem the crop aspect lock was fixed for.
///
/// ⚠ Composes with the format filter and the search text through `Library::refilter`, the single
/// derived `visible` list every consumer iterates — not a second filtering mechanism.
fn mark_filter(app: &mut RustroomApp, ui: &mut egui::Ui) {
    if app.library.is_none() {
        return;
    }
    let before = app.filter.clone();
    let mut f = before.clone();

    // ⚠⚠ `CloseOnClickOutside`, not the default `CloseOnClick`. Every row here is a checkbox and
    // the whole point is ticking several; a popup that shut on the first tick made setting two
    // filters a four-click job of reopening it.
    let cfg = egui::containers::menu::MenuConfig::default()
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
    // ⚠ The funnel is prepended at the widget, not in `mark_filter_label`: the label stays a
    // pure, testable sentence and the glyph is its clothing.
    egui::containers::menu::MenuButton::new(format!("{} {}", icons::FILTER, mark_filter_label(&f)))
        .config(cfg)
        .ui(ui, |ui| {
            // ⚠ Tall enough to need scrolling now that capture values join the marks — the list
            // of lenses grows with the library, unlike the fixed mark sections.
            egui::ScrollArea::vertical()
                .max_height(420.0)
                .show(ui, |ui| {
                    // ⚠⚠ **One ordering across all three sections: the unset value first, then the
                    // values.** Flag used to read "Any, picked, rejected, unflagged" while colour read
                    // "Any, unlabelled, colours" — two orderings for one idea. There is no "Any" row any
                    // more either: an empty set *is* "any", which is the same convention in all three and
                    // one fewer thing to explain.
                    ui.label("Rating");
                    for v in 0..=rustroom_app::library_filter::RatingSet::MAX {
                        let text = if v == 0 {
                            "Unrated".to_string()
                        } else {
                            format!("{v}★")
                        };
                        let mut on = f.stars.contains(v);
                        if ui.checkbox(&mut on, text).changed() {
                            f.stars.toggle(v);
                        }
                    }
                    ui.separator();
                    ui.label("Flag");
                    // ⚠ Driven by `FlagSet::ALL`, so the ordering lives beside the type rather than being
                    // re-invented here — the rule `EDIT_GROUP_NAMES` and `Tool::ALL` already follow.
                    for flag in rustroom_app::library_filter::FlagSet::ALL {
                        let text = match flag {
                            catalog::db::Flag::None => "Unflagged",
                            catalog::db::Flag::Pick => "⚑ Picked",
                            catalog::db::Flag::Reject => "× Rejected",
                        };
                        let mut on = f.flags.contains(flag);
                        if ui.checkbox(&mut on, text).changed() {
                            f.flags.toggle(flag);
                        }
                    }
                    ui.separator();
                    ui.label("Colour");
                    for (n, name) in [
                        (0u8, "Unlabelled"),
                        (1, "Red"),
                        (2, "Blue"),
                        (3, "Yellow"),
                        (4, "Green"),
                    ] {
                        let mut on = f.labels.contains(n);
                        if ui.checkbox(&mut on, name).changed() {
                            f.labels.toggle(n);
                        }
                    }
                    ui.separator();
                    // ⚠⚠ Shown only with **more than one** value. Measured: this library has exactly one
                    // camera, so a Camera section would be a single checkbox that can only hide
                    // everything — the same reasoning that hides the format chips below two formats.
                    let cameras = app
                        .catalog
                        .as_ref()
                        .and_then(|c| c.distinct_cameras().ok())
                        .unwrap_or_default();
                    capture_section(ui, "Camera", &cameras, &mut f.cameras);
                    let lenses = app
                        .catalog
                        .as_ref()
                        .and_then(|c| c.distinct_lenses().ok())
                        .unwrap_or_default();
                    capture_section(ui, "Lens", &lenses, &mut f.lenses);
                    ui.separator();
                    if ui.button("Clear").clicked() {
                        f.clear_marks_and_text();
                    }
                });
        });

    // ⚠ Guarded on an actual change: this body runs every frame, and refiltering a folder of
    // hundreds per frame is the `apply_view_demosaic` mistake in another costume.
    if f != before {
        app.filter = f;
        // ⚠ The filter lives on the app now, so refiltering is a separate step rather than a
        // field write — that split is what lets it survive a source switch.
        let filter = app.filter.clone();
        if let Some(lib) = app.library.as_mut() {
            lib.refilter(&filter);
        }
    }
}

/// One multi-select section of open-ended string values.
///
/// ⚠ Drawn only when there is **more than one** value: a single checkbox can only hide everything,
/// which is not a filter. The same guard the format chips use.
fn capture_section(
    ui: &mut egui::Ui,
    title: &str,
    values: &[String],
    chosen: &mut std::collections::BTreeSet<String>,
) {
    if values.len() < 2 {
        return;
    }
    ui.label(title);
    for v in values {
        let mut on = chosen.contains(v);
        if ui.checkbox(&mut on, v).changed() {
            if on {
                chosen.insert(v.clone());
            } else {
                chosen.remove(v);
            }
        }
    }
    ui.separator();
}

/// What the Filter button says.
///
/// ⚠ Pure, so "an active filter is visible" is testable without a window.
fn mark_filter_label(f: &rustroom_app::library_filter::LibraryFilter) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !f.stars.is_empty() {
        let list: Vec<String> = f
            .stars
            .ratings()
            .into_iter()
            .map(|r| if r == 0 { "0".into() } else { r.to_string() })
            .collect();
        parts.push(format!("{}★", list.join(",")));
    }
    for flag in f.flags.flags() {
        parts.push(
            match flag {
                catalog::db::Flag::None => "unflagged",
                catalog::db::Flag::Pick => "⚑",
                catalog::db::Flag::Reject => "×",
            }
            .to_string(),
        );
    }
    for c in &f.cameras {
        parts.push(c.clone());
    }
    for l in &f.lenses {
        parts.push(l.clone());
    }
    if !f.labels.is_empty() {
        let names = ["unlabelled", "red", "blue", "yellow", "green"];
        let list: Vec<&str> = f
            .labels
            .labels()
            .into_iter()
            .map(|l| names.get(l as usize).copied().unwrap_or("?"))
            .collect();
        parts.push(list.join(","));
    }
    // ⚠⚠ **Always prefixed.** It read as `3★+` alone when active, so the button stopped naming
    // itself and looked like a different control depending on what was set — reported in use.
    // The prefix is what keeps it recognisably the same button.
    if parts.is_empty() {
        "Filter".to_string()
    } else {
        format!("Filter: {}", parts.join(" "))
    }
}

/// Narrow the Library by name.
///
/// ⚠ Matches `entries::shown_name`, so what you search is what the caption shows — including a
/// copy's own name.
///
/// ⚠ **Needs no new keyboard work, and that is checked rather than assumed.** The Library's bare
/// culling keys (`0`–`9`, `p`, `x`, `u`), its arrow navigation and `pressed_command` are all
/// already gated on `memory().focused()`, which is exactly the gate a text field needs. Escape is
/// added so the keyboard is recoverable without the mouse.
fn search_field(app: &mut RustroomApp, ui: &mut egui::Ui) {
    if app.library.is_none() {
        return;
    }
    let before = app.filter.text.clone();
    let mut text = before.clone();
    // ⚠ The magnifying glass is in the hint, so it disappears with it once you type — it
    // names the field, it is not a button.
    let r = ui.add(
        egui::TextEdit::singleline(&mut text)
            .hint_text(format!("{} Search photos", icons::ZOOM_ACTUAL))
            .desired_width(200.0),
    );
    // ⚠ Read the key press into a local *before* touching `r`. `Response` methods take the context
    // lock internally, and holding one across `ui.input(..)` panics inside epaint's mutex — the
    // re-entrant lock `name_edit::field` already paid for.
    let escaped = ui.input(|i| i.key_pressed(egui::Key::Escape));
    if escaped && r.has_focus() {
        r.surrender_focus();
    }
    // ⚠ Guarded on a change: the body runs every frame.
    if text != before {
        app.filter.text = text;
        let filter = app.filter.clone();
        if let Some(lib) = app.library.as_mut() {
            lib.refilter(&filter);
        }
    }
}

/// The Library cell size, as a short slider with the `arrows-out-simple` glyph.
///
/// ⚠ A slider at a **fixed** 80 pt, where the two buttons it replaces were chosen because "a
/// slider needs a fixed width there whether or not the space exists": the row's right cluster
/// has since lost the format checkboxes' text and gained a ⋯ menu, so the width exists now.
///
/// ⚠ Saved when the drag **settles**, never per frame — the `cell_zoom_pending` rule the pinch
/// gesture follows. A click on the rail (no drag) saves at once.
fn grid_size_slider(app: &mut RustroomApp, ui: &mut egui::Ui) {
    let (lo, hi) = rustroom_app::settings::CELL_RANGE;
    let mut cell = app.settings.clamped_cell() as u32;
    ui.spacing_mut().slider_width = 80.0;
    let r = ui
        .add(egui::Slider::new(&mut cell, lo..=hi).show_value(false))
        .on_hover_text("Thumbnail size");
    if r.changed() {
        app.settings.library_cell = cell;
        app.cell_zoom_pending = true;
    }
    if app.cell_zoom_pending && !r.dragged() {
        app.cell_zoom_pending = false;
        app.save_settings();
    }
    ui.label(egui::RichText::new(icons::THUMB_SIZE.to_string()).size(15.0))
        .on_hover_text("Thumbnail size");
}

/// How the folder is ordered. Drives the filmstrip too — both read one `entries` list.
fn sort_combo(app: &mut RustroomApp, ui: &mut egui::Ui) {
    let mut order = app.settings.library_sort;
    egui::ComboBox::from_id_salt("library_sort")
        .selected_text(format!("{} {}", icons::SORT, order.label()))
        .show_ui(ui, |ui| {
            for o in catalog::SortOrder::ALL {
                ui.selectable_value(&mut order, o, o.label());
            }
        });
    // ⚠ `apply_sort` guards on an actual change itself, since this body runs every frame and
    // re-sorting a folder of hundreds per frame is not free.
    app.apply_sort(order);
}

/// ⚠ Shown only at two or more formats: a lone control here could only ever blank the grid,
/// which is a footgun with no upside.
///
/// ⚠ **Multi-select, so there is no "All" entry** — showing everything is simply every box
/// ticked, which is both the default and self-evident. An "All" radio alongside per-format
/// radios would additionally make "RAF and JPG but not PNG" unreachable, which is a real
/// combination on a mixed folder.
fn format_chips(app: &mut RustroomApp, ui: &mut egui::Ui) {
    let formats = app
        .library
        .as_ref()
        .map(|lib| format_filter::formats_present(&lib.entries))
        .unwrap_or_default();
    // ⚠⚠ The SAME predicate the filtering uses, not a second copy of the rule. When these were
    // two independent conditions, hiding `jpg` and opening a JPEG-only folder drew no chips AND
    // hid every photo — an empty grid with no way back, reported in use 2026-09-17.
    //
    // ⚠ A single-format folder still draws its chip (user's call, 2026-09-20: the format and
    // its count should always be visible) — lit, since the override shows everything, and
    // **not clickable**, since a click could only hide the whole grid or do nothing.
    let single = app
        .library
        .as_ref()
        .is_some_and(|lib| format_filter::folder_overrides_hidden(&lib.entries));
    // ⚠ Built rightmost-first, because the enclosing layout runs right to left; reversing here
    // is what keeps the on-screen order the same as `formats_present` returns.
    //
    // ⚠ **Every box stays enabled, including the last ticked one.** Disabling it to prevent an
    // empty grid greyed out whichever format was still shown — which reads, from the other
    // side, as "the ones I did not click went dark". Unticking everything is recoverable in
    // one click because the boxes are still there, so the guard cost more than it bought.
    let mut toggled: Option<String> = None;
    for (ext, count) in formats.iter().rev() {
        let mut shown = single || !app.filter.format.is_hidden(ext);
        let label = format!("{} {count}", ext.to_uppercase());
        // ⚠ A chip with a dot rather than a checkbox: the dot carries the state in colour, the
        // semantics are the checkbox's exactly (`segmented::chip` reports as one).
        let r = ui
            .add_enabled_ui(!single, |ui| segmented::chip(ui, &mut shown, &label))
            .inner;
        if single {
            r.on_hover_text("The only format in this folder");
        } else if r.changed() {
            toggled = Some(ext.clone());
        }
    }
    if let Some(ext) = toggled {
        app.filter.format.toggle(&ext);
        let filter = app.filter.clone();
        if let Some(lib) = app.library.as_mut() {
            lib.refilter(&filter);
        }
        // ⚠ Persisted, so it survives a restart as well as a source switch. Written only on an
        // actual toggle — this body runs every frame, and saving unconditionally would rewrite the
        // settings file sixty times a second, the `apply_view_demosaic` rule.
        app.settings.hidden_formats = app.filter.format.clone();
        app.save_settings();
    }
}

/// The centred line naming the current photo: its name, then in weak small text its camera,
/// decoded size and file size.
///
/// ⚠ A child `Ui` placed in the gap, not a third laid-out cluster: the bar's left cluster and
/// its right-to-left cluster own the row's allocation, and a third would be exactly the "the
/// status was pushed into a row that had no space left" bug the right cluster was created to
/// fix. `new_child` places without advancing the parent. The child is centred on the bar and
/// pulled into the gap when the centre is not in it.
///
/// ⚠ The name is the Library caption's widget: a click opens the file-rename field, Enter
/// commits, clicking away abandons (user's call, 2026-09-20 — the Develop panel's footer, which
/// carried the filename and the copy-label field, is gone). The draft is the bar's own, never
/// the grid's: a photo selected in the grid would otherwise open two fields on one string.
///
/// ⚠ Two faces on purpose: the name in the body face and the strong colour, the rest in the
/// 11 pt weak face — as one run of body text "the name and file format and resolution and size
/// all read the same" (seen in use, 2026-09-20).
fn centre_info(
    app: &mut RustroomApp,
    ui: &mut egui::Ui,
    row: egui::Rect,
    gap: egui::Rangef,
) -> Option<(PhotoId, name_edit::Rename)> {
    // ⚠⚠ **Which photo** depends on the view; **what is said about it** does not. The facts
    // come from one rule (`photo_info::facts`) fed the same two sources in both views — the
    // catalog's capture scan and the open session — so switching views with a photo selected
    // never changes the line ("the content of the name should not change when switching",
    // seen in use 2026-09-20: Develop read the camera off the decode and showed the pixel size,
    // the Library read the catalog and showed neither).
    let (path, copy) = match app.view {
        AppView::Develop => {
            let s = app.session.as_ref()?;
            (s.path.clone(), s.copy)
        }
        AppView::Library => {
            let lib = app.library.as_ref()?;
            let e = lib.entries.get(lib.selection.active()?)?;
            (e.id.path.clone(), e.id.copy)
        }
    };
    let catalog_camera = app
        .library
        .as_ref()
        .and_then(|lib| lib.captures.get(&path))
        .and_then(|c| c.camera.clone());
    let session = app.session.as_ref().map(|s| {
        (
            s.path.clone(),
            s.make_model.clone(),
            (s.buffer.width(), s.buffer.height()),
        )
    });
    let facts = rustroom_app::photo_info::facts(
        &path,
        catalog_camera.as_deref(),
        session
            .as_ref()
            .map(|(p, m, d)| (p.as_path(), m.as_str(), *d)),
    );
    let id = PhotoId::new(path.clone(), copy);
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let bytes = app.info_sizes.bytes_of(&path);
    let meta = rustroom_app::photo_info::meta(facts.camera, facts.dims, bytes);

    const PAD: f32 = 12.0;
    const BETWEEN: f32 = 10.0;
    let avail = (gap.max - gap.min - PAD * 2.0).max(0.0);
    if avail < 60.0 {
        return None;
    }
    let body = egui::TextStyle::Body.resolve(ui.style());
    let small = egui::FontId::proportional(11.0);
    let editing = app.info_draft.as_ref().is_some_and(|(d, _)| *d == id);
    let (name_w, meta_w) = ui.fonts_mut(|f| {
        let shown = rustroom_app::entries::caption(&file_name, copy);
        (
            f.layout_no_wrap(shown, body.clone(), egui::Color32::WHITE)
                .size()
                .x,
            f.layout_no_wrap(meta.clone(), small.clone(), egui::Color32::WHITE)
                .size()
                .x,
        )
    });
    // The field wants the whole gap; the caption wants its own width.
    let want = if editing {
        avail
    } else if meta.is_empty() {
        name_w + 2.0
    } else {
        name_w + BETWEEN + meta_w + 2.0
    };
    let w = want.min(avail);
    let x = (row.center().x - w / 2.0)
        .max(gap.min + PAD)
        .min(gap.max - PAD - w);
    let rect = egui::Rect::from_min_size(egui::pos2(x, row.top()), egui::vec2(w, row.height()));
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center))
            .id_salt("centre_info"),
    );
    child.spacing_mut().item_spacing.x = BETWEEN;
    let strong = child.visuals().strong_text_color();
    child.visuals_mut().override_text_color = Some(strong);
    let rename = name_edit::caption_or_field(
        &mut child,
        &id,
        &file_name,
        None,
        &mut app.info_draft,
        body.size,
    );
    child.visuals_mut().override_text_color = None;
    if !editing && !meta.is_empty() {
        let weak = child.visuals().weak_text_color();
        child.add(
            egui::Label::new(egui::RichText::new(meta).font(small).color(weak))
                .selectable(false)
                .truncate(),
        );
    }
    rename.map(|r| (id, r))
}

#[cfg(test)]
mod tests {
    /// The bar with a Library of three, the second selected, and that same photo open.
    fn app_with_photo() -> (egui::Context, RustroomApp) {
        let ctx = crate::test_ui::ctx();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        let mut lib = crate::test_app::library_of(3);
        lib.selection.click(1);
        let path = lib.entries[1].id.path.clone();
        if let Some(s) = app.session.as_mut() {
            s.path = path;
        }
        app.library = Some(lib);
        (ctx, app)
    }

    /// Draw the bar in `view` and return the row's height and every accessible label in it.
    fn bar(ctx: &egui::Context, app: &mut RustroomApp, view: AppView) -> (f32, Vec<String>) {
        app.view = view;
        let mut height = 0.0;
        // ⚠ A wide screen: at the panels' 400 pt the bar has no gap for its centred line.
        let wide = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 200.0));
        let labels: Vec<String> =
            crate::test_ui::labelled_centres_on(ctx, wide, Vec::new(), |ui| {
                let ctx = ui.ctx().clone();
                let (_, _, rect) = app.top_bar(ui, &ctx);
                height = rect.height();
            })
            .into_iter()
            .map(|(l, _)| l)
            .collect();
        (height, labels)
    }

    #[test]
    fn the_bar_is_the_same_height_in_both_views() {
        // ⚠ Seen in use: the bar changed height on a view switch. The row is allocated at an
        // exact height now, and this measures it with the real controls of both views.
        let (ctx, mut app) = app_with_photo();
        let (lib_h, _) = bar(&ctx, &mut app, AppView::Library);
        let (dev_h, _) = bar(&ctx, &mut app, AppView::Develop);
        assert_eq!(lib_h, BAR_HEIGHT, "the Library bar grew past its height");
        assert_eq!(dev_h, BAR_HEIGHT, "the Develop bar grew past its height");
    }

    #[test]
    fn the_centred_name_is_the_same_in_both_views() {
        // ⚠ Seen in use: the line said different things about one photo depending on the
        // view. The name is the caption widget's label; it must be there, and be the same.
        let (ctx, mut app) = app_with_photo();
        let name = app.library.as_ref().unwrap().entries[1].caption();
        let (_, lib) = bar(&ctx, &mut app, AppView::Library);
        let (_, dev) = bar(&ctx, &mut app, AppView::Develop);
        assert!(lib.contains(&name), "Library bar lacks the name: {lib:?}");
        assert!(dev.contains(&name), "Develop bar lacks the name: {dev:?}");
    }

    #[test]
    fn the_filter_button_reports_an_active_filter() {
        // ⚠ A control that hides rows while looking idle is invisible state — the problem the crop
        // aspect lock was fixed for. This label is the only thing that says a filter is on.
        use rustroom_app::library_filter::LibraryFilter;
        assert_eq!(mark_filter_label(&LibraryFilter::default()), "Filter");
        let mut three = rustroom_app::library_filter::RatingSet::default();
        three.toggle(3);
        let mut unflagged = rustroom_app::library_filter::FlagSet::default();
        unflagged.toggle(catalog::db::Flag::None);
        let mut unlabelled = rustroom_app::library_filter::LabelSet::default();
        unlabelled.toggle(0);
        for f in [
            LibraryFilter {
                stars: three,
                ..Default::default()
            },
            LibraryFilter {
                flags: unflagged,
                ..Default::default()
            },
            LibraryFilter {
                labels: unlabelled,
                ..Default::default()
            },
        ] {
            let label = mark_filter_label(&f);
            assert_ne!(label, "Filter", "an active filter looked idle: {f:?}");
            // ⚠⚠ Always prefixed. It read as `3★+` alone before, so the button stopped naming
            // itself and looked like a different control depending on what was set.
            assert!(
                label.starts_with("Filter: "),
                "the button stopped calling itself Filter: {label:?}"
            );
        }
    }

    #[test]
    fn every_filter_glyph_is_renderable() {
        // ⚠⚠ M4.2a found `✓` and `✕` are in NO bundled font and would have shipped as tofu boxes,
        // and the rail's four candidates were all absent too. Probe, never assume.
        //
        // ⚠ A frame must run before the fonts exist, and `install_fonts` is what makes this ask
        // about the stack production uses rather than egui's defaults.
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        let _ = ctx.run_ui(egui::RawInput::default(), |_ui| {});
        let id = egui::FontId::proportional(14.0);
        for c in ['★', '⚑', '×'] {
            assert!(
                ctx.fonts_mut(|f| f.has_glyph(&id, c)),
                "{c} (U+{:04X}) is not in any bundled font and would render as a tofu box",
                c as u32
            );
        }
    }
    use super::*;

    /// One press of a bare, unmodified key.
    fn bare_key(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    #[test]
    fn the_composition_guide_key_is_gated_on_nothing_having_focus() {
        // ⚠⚠ **Both halves, and neither is optional.** A gate that never lets the key through
        // and one that never blocks it are equally wrong, and only the pair can tell them
        // apart — the rule `a_bare_keyword_is_deleted_without_a_dialog_and_a_parent_is_not`
        // already follows. `O` is bare, so a text field must swallow it: the Save-preset name
        // field and the Library search box are both one keystroke away.
        let ctx = egui::Context::default();

        let mut unfocused = None;
        let _ = ctx.run_ui(
            egui::RawInput {
                events: vec![bare_key(egui::Key::O)],
                ..Default::default()
            },
            |_ui| unfocused = pressed_command(&ctx),
        );
        assert_eq!(
            unfocused,
            Some(Command::CycleCompositionGuide),
            "with nothing focused, O must reach the command"
        );

        // Give a text field the focus, then press the same key again.
        let mut text = String::new();
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.add(egui::TextEdit::singleline(&mut text))
                .request_focus();
        });
        let mut focused = Some(Command::CycleCompositionGuide);
        let _ = ctx.run_ui(
            egui::RawInput {
                events: vec![bare_key(egui::Key::O)],
                ..Default::default()
            },
            |ui| {
                // ⚠ Read **before** the field is drawn, deliberately. A `TextEdit` consumes the
                // key events it handles, so reading afterwards would pass even with the focus
                // gate deleted — it would be testing egui's consumption, not our guard. The
                // field is still added, so egui does not drop its focus at the end of the pass.
                focused = pressed_command(&ctx);
                ui.add(egui::TextEdit::singleline(&mut text));
            },
        );
        assert_eq!(
            focused, None,
            "a key typed into a text field must not reach a command"
        );
    }

    #[test]
    fn every_shortcut_maps_to_an_egui_key() {
        // ⚠ A chord in the model with no egui key is a menu item whose shortcut silently does
        // nothing on Windows and Linux — and invisible on macOS, where the native menu handles
        // the safe ones anyway.
        for section in menu::model() {
            for item in section.items {
                if let menu::Item::Action {
                    label,
                    shortcut: Some(sc),
                    ..
                } = item
                {
                    assert!(
                        egui_key(sc.key).is_some(),
                        "{label}'s shortcut key {:?} has no egui mapping",
                        sc.key
                    );
                }
            }
        }
    }

    /// Press one bare key and return what `pressed_command` made of it.
    fn bare(k: egui::Key) -> Option<menu::Command> {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            events: vec![egui::Event::Key {
                key: k,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        };
        let mut got = None;
        let _ = ctx.run_ui(input, |ui| {
            got = pressed_command(ui.ctx());
        });
        got
    }

    #[test]
    fn a_bare_c_really_reaches_the_dispatcher() {
        // ⚠⚠ **The one fact this feature could not be built on faith.** `egui-winit` matches the
        // clipboard chords *first* and pushes `Event::Copy` **instead of** a `Key` event, so
        // `key_pressed(Key::C)` can never see ⌘C — a trap this codebase has already paid for.
        // Reading its source settles the other half: `is_copy_command` is
        // `modifiers.command && key == C`, so a **bare** C is untouched. This asserts our end of
        // it — that the bare chord is not skipped by `pressed_command`'s own ⌘C/⌘V exclusion,
        // which tests `sc.cmd` and would swallow both if it tested only the character.
        assert_eq!(bare(egui::Key::C), Some(menu::Command::ShowCompare));
        assert_eq!(bare(egui::Key::N), Some(menu::Command::ShowSurvey));
    }

    #[test]
    fn a_bare_key_is_ignored_while_a_text_field_has_focus() {
        // ⚠ The guard on the guard: `C` and `N` are ordinary characters, so without this the
        // Library's search field would jump to Compare on every "c" typed into it. Asserted
        // through the *same* function, or it would be a test about a different code path.
        let ctx = egui::Context::default();
        ctx.memory_mut(|m| m.request_focus(egui::Id::new("a text field")));
        let input = egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::C,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        };
        let mut got = None;
        let _ = ctx.run_ui(input, |ui| {
            got = pressed_command(ui.ctx());
        });
        assert_eq!(got, None, "a bare C typed into a field switched view");
    }
}
