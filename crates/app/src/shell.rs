//! The frame loop: the top toolbar, the Library/Develop dispatch, and the per-frame pumps.
//!
//! ⚠ This is a **trait impl moved to another module of the same crate**, which Rust allows —
//! so unlike every other move in this refactor, `self` stays `self` and the body is verbatim.

use crate::{commands, develop, library, menu_bar, modals, AppView, RustroomApp, PREVIEW_MAX_DIM};
use eframe::egui;
use rustroom_app::icons;
use rustroom_app::menu::{self, Command};
use rustroom_app::render_engine::RegionRect;
use rustroom_app::{format_filter, view_region};

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
                let img = egui::ColorImage::from_rgba_unmultiplied(
                    [frame.width, frame.height],
                    &frame.rgba,
                );
                s.texture = Some(ctx.load_texture("preview", img, egui::TextureOptions::LINEAR));
                s.rendered_region = frame.region.map(|r| RegionRect {
                    x: r.x0,
                    y: r.y0,
                    w: frame.width,
                    h: frame.height,
                });
                // A whole-image frame is by definition a fit-resolution render, and is the
                // only thing that can tell us the post-geometry size — which changes with
                // the crop and the straighten angle.
                // A whole-image frame is the only thing that can tell us the post-geometry
                // size, which changes with the crop and the straighten angle.
                //
                // ⚠ Scaled by the ratio of *caps*, not accepted only at the fit rung. Both are
                // pre-geometry caps, so the crop factor cancels and this is exact: a frame is
                // `max_dim x crop_factor` and the fit size is `fit_cap x crop_factor`. That
                // generalisation is what lets a whole-image frame at a *higher* rung — which
                // now happens whenever the window is larger than the fit texture — keep this
                // fresh, where the old equality test would silently have rejected it.
                //
                // It is still not derived from a rendered size feeding back: `fit_cap` and
                // `frame.max_dim` are both caps, and neither comes from `fit_image_size`.
                let fit_cap = view_region::fit_cap(s.buffer_long, PREVIEW_MAX_DIM);
                if frame.region.is_none() && frame.max_dim > 0 {
                    let ratio = fit_cap as f32 / frame.max_dim as f32;
                    s.fit_image_size =
                        egui::vec2(frame.width as f32 * ratio, frame.height as f32 * ratio);
                }
                // A region frame carries the full post-geometry dimensions it was cut from,
                // so this is exact either way.
                s.rendered_long = frame
                    .region
                    .map(|r| r.full_w.max(r.full_h))
                    .unwrap_or(frame.width.max(frame.height))
                    as f32;
            }
        }

        // Pull finished thumbnails into textures.
        if let Some(lib) = &mut self.library {
            for res in lib.loader.drain() {
                if let Some(t) = res.thumb {
                    let bytes = t.rgba.len();
                    let img =
                        egui::ColorImage::from_rgba_unmultiplied([t.width, t.height], &t.rgba);
                    // ⚠ The texture name is derived from the path, not from an index. Two
                    // different photos sharing a name would have egui serve one image for both.
                    let tex = ctx.load_texture(
                        format!("thumb:{}", res.path.display()),
                        img,
                        egui::TextureOptions::LINEAR,
                    );
                    lib.cache.insert(res.path, tex, bytes);
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
        egui::Panel::top("top").show(ui, |ui| {
            // ⚠ macOS gets the real system menu bar instead; drawing this one too would put a
            // redundant row under a nearly-empty system menu.
            if !cfg!(target_os = "macos") {
                if let Some(cmd) = menu_bar::show(ui) {
                    pending_command = Some(cmd);
                }
            }
            ui.horizontal(|ui| {
                view_switch(ui, &mut self.view);
                if self.view == AppView::Develop && self.session.is_some() {
                    ui.separator();
                    let decoding = self.pending.is_some();
                    ui.add_enabled_ui(!decoding, |ui| develop::toolbar(self, ui, &ctx));
                }
                // ⚠ On the LEFT, not in the right-aligned cluster below. A text field needs
                // ~180 px and that cluster already carries five controls plus the status line;
                // adding it there squeezes the status out, which is the exact bug the cluster was
                // created to fix. This space is where the Develop toolbar sits, and it is empty
                // in Library.
                if self.view == AppView::Library && self.library.is_some() {
                    ui.separator();
                    search_field(self, ui);
                    // ⚠ Beside the search field, not in the right-aligned cluster. They are one
                    // idea — "narrow what I am looking at" — and splitting them across the row put
                    // half of it next to the sort order and the grid size, which are not.
                    mark_filter(self, ui);
                }
                // ⚠ One right-aligned cluster for everything that belongs at the far end, laid
                // out **rightmost first**. Previously the develop toolbar right-aligned the
                // filename itself and then the status label was added after it — which meant
                // the status was pushed into a row that had no space left.
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // ⚠ Library only. The filename is **not** shown here: the Develop panel's
                    // footer already carries it, and the same name in two places at once is
                    // noise rather than reassurance.
                    if self.view == AppView::Library {
                        format_bullets(self, ui);
                        if self.library.is_some() {
                            grid_size_buttons(self, ui);
                            sort_combo(self, ui);
                            let r = ui.button("Reindex").on_hover_text(
                                "Re-read the folder, sort it alphabetically, and close any gaps \
                                 in the copy numbering.",
                            );
                            if r.clicked() {
                                self.reindex(&ctx);
                            }
                        }
                    }
                    if !self.export_progress.is_empty() {
                        if ui.button("Cancel").clicked() {
                            self.cancel_export();
                        }
                        ui.label(&self.export_progress);
                    } else if !self.status.is_empty() {
                        ui.label(&self.status);
                    }
                });
            });
        });

        if let Some(cmd) = pending_command {
            commands::execute(self, &ctx, cmd);
        }

        match self.view {
            AppView::Develop => develop::body(self, ui, &ctx),
            AppView::Library => {
                // ⚠ Panel order is load-bearing: egui allocates side panels in call order and
                // the central panel takes what is left, so the collections panel must be drawn
                // *before* the grid.
                crate::collections_panel::show(self, ui, &ctx);
                library::show(self, ui, &ctx);
            }
        }

        modals::draw(self, &ctx);
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
        '+' => egui::Key::Plus,
        '-' => egui::Key::Minus,
        '0' => egui::Key::Num0,
        '1' => egui::Key::Num1,
        '2' => egui::Key::Num2,
        '3' => egui::Key::Num3,
        '4' => egui::Key::Num4,
        ',' => egui::Key::Comma,
        '[' => egui::Key::OpenBracket,
        ']' => egui::Key::CloseBracket,
        '\'' => egui::Key::Quote,
        '\\' => egui::Key::Backslash,
        _ => return None,
    })
}

/// The Library / Develop switch, as one segmented control.
///
/// ⚠ Zero spacing between the two halves is the whole of the effect: as separate buttons they
/// read as two unrelated actions, and abutted they read as one control with two states — which
/// is what a module switch is. The padded labels give it the proportions of a segment rather
/// than of a word.
fn view_switch(ui: &mut egui::Ui, view: &mut AppView) {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.selectable_value(view, AppView::Library, "  Library  ");
        ui.selectable_value(view, AppView::Develop, "  Develop  ");
    });
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
    egui::containers::menu::MenuButton::new(mark_filter_label(&f))
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
    let r = ui.add(
        egui::TextEdit::singleline(&mut text)
            .hint_text("Search")
            .desired_width(170.0),
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

/// Bigger / smaller Library cells.
///
/// ⚠ Buttons rather than a slider: this row is right-aligned and already carries the sort combo,
/// the format bullets and the status line, and a slider needs a fixed width there whether or not
/// the space exists. Two icons cost one line each.
///
/// ⚠ Built rightmost-first, because the enclosing layout runs right to left.
fn grid_size_buttons(app: &mut RustroomApp, ui: &mut egui::Ui) {
    let now = app.settings.clamped_cell() as u32;
    let mut next = None;
    if ui
        .add_enabled(
            now < rustroom_app::settings::CELL_RANGE.1,
            egui::Button::new(icons::ZOOM_IN.to_string()),
        )
        .on_hover_text("Bigger thumbnails")
        .clicked()
    {
        next = Some(app.settings.stepped_cell(true));
    }
    if ui
        .add_enabled(
            now > rustroom_app::settings::CELL_RANGE.0,
            egui::Button::new(icons::ZOOM_OUT.to_string()),
        )
        .on_hover_text("Smaller thumbnails")
        .clicked()
    {
        next = Some(app.settings.stepped_cell(false));
    }
    // ⚠ Saved only on an actual press. This body runs every frame, so writing unconditionally
    // would rewrite the settings file sixty times a second — the `apply_view_demosaic` rule.
    if let Some(cell) = next {
        app.settings.library_cell = cell;
        app.save_settings();
    }
}

/// How the folder is ordered. Drives the filmstrip too — both read one `entries` list.
fn sort_combo(app: &mut RustroomApp, ui: &mut egui::Ui) {
    let mut order = app.settings.library_sort;
    egui::ComboBox::from_id_salt("library_sort")
        .selected_text(order.label())
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
fn format_bullets(app: &mut RustroomApp, ui: &mut egui::Ui) {
    let formats = app
        .library
        .as_ref()
        .map(|lib| format_filter::formats_present(&lib.entries))
        .unwrap_or_default();
    if formats.len() < 2 {
        return;
    }
    // ⚠ Built rightmost-first, because the enclosing layout runs right to left; reversing here
    // is what keeps the on-screen order the same as `formats_present` returns.
    //
    // ⚠ **Every box stays enabled, including the last ticked one.** Disabling it to prevent an
    // empty grid greyed out whichever format was still shown — which reads, from the other
    // side, as "the ones I did not click went dark". Unticking everything is recoverable in
    // one click because the boxes are still there, so the guard cost more than it bought.
    let mut toggled: Option<String> = None;
    for (ext, count) in formats.iter().rev() {
        let mut shown = !app.filter.format.is_hidden(ext);
        let label = format!("{} ({count})", ext.to_uppercase());
        if ui.checkbox(&mut shown, label).changed() {
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

#[cfg(test)]
mod tests {

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
}
