//! The status row: under the Library's content and above the Develop filmstrip — `docs/DESIGN.md`
//! §6 and §7.12.
//!
//! ⚠⚠ **Why it exists.** The Library's one top bar carried navigation, search, filters, the sort,
//! the grid size, the format chips, the stack buttons, Reindex, the Immich buttons, the progress
//! of a running job and the status line — and at 1440 pt (a 13-inch MacBook Pro) they overprinted
//! each other: the count ran under the search field and Filter clipped to "Fi" (the 2026-09-29
//! survey). The top row now only navigates and narrows; this row **informs and sets the view**.
//!
//! Layout, both rows alike: the right-hand cluster is **planned first** (`bar_plan` — what shows,
//! what drops its label, what folds into ⋯) and drawn right to left; the left-hand text then gets
//! exactly what is left and truncates into it. Nothing is placed where it merely hopes to fit.

use crate::segmented;
use crate::theme::Hover as _;
use crate::theme::{self, tokens};
use crate::{LibrarySource, RustroomApp};
use eframe::egui;
use rustroom_app::bar_plan::{self, Form, Item};
use rustroom_app::format_filter;
use rustroom_app::icons;

/// The row's height.
pub const HEIGHT: f32 = 32.0;

/// Room the left-hand text keeps before anything on the right is allowed to push into it.
const STATUS_MIN: f32 = 200.0;

/// The frame both rows share — and a service view's Publish band — the panel surface, a hairline, 12 pt either side.
pub(crate) fn frame(style: &egui::Style) -> egui::Frame {
    egui::Frame::side_top_panel(style).inner_margin(egui::Margin {
        left: 12,
        right: 8,
        top: 0,
        bottom: 0,
    })
}

/// The Library's status row, as a bottom panel of the content column.
///
/// ⚠ Called **after** the collections panel and **before** the view's central panel: egui gives
/// side and bottom panels their space in call order, so this spans the content column only.
pub fn library(app: &mut RustroomApp, ui: &mut egui::Ui, ctx: &egui::Context) {
    egui::Panel::bottom("status")
        .exact_size(HEIGHT)
        .resizable(false)
        .frame(frame(ui.style()))
        .show(ui, |ui| library_row(app, ui, ctx));
}

/// The Develop status row, between the canvas and the filmstrip.
pub fn develop(app: &mut RustroomApp, ui: &mut egui::Ui) {
    egui::Panel::bottom("develop_status")
        .exact_size(HEIGHT)
        .resizable(false)
        .frame(frame(ui.style()))
        .show(ui, |ui| develop_row(app, ui));
}

/// Which right-hand items the Library row draws in the row itself; the rest go in ⋯.
#[derive(Clone, Copy, Debug)]
struct Shown {
    chips: bool,
    immich: bool,
    sort_compact: bool,
    size: bool,
}

/// The width of `text` in `font`.
fn text_w(ui: &egui::Ui, text: &str, font: &egui::FontId) -> f32 {
    ui.painter()
        .layout_no_wrap(text.to_owned(), font.clone(), egui::Color32::PLACEHOLDER)
        .size()
        .x
}

/// The Library row's right-hand items, measured and planned into `avail`.
fn plan_library(app: &RustroomApp, ui: &egui::Ui, avail: f32) -> Shown {
    let body = egui::TextStyle::Body.resolve(ui.style());
    let caption = egui::FontId::proportional(tokens::CAPTION);
    let pad = ui.spacing().button_padding.x * 2.0;
    let gap = ui.spacing().item_spacing.x;
    let chips: f32 = chip_labels(app)
        .iter()
        .map(|l| segmented::chip_width(ui, l, &caption) + gap)
        .sum();
    let immich = if immich_source(app) {
        text_w(ui, "Download", &body)
            + text_w(ui, "Remove downloads", &body)
            + text_w(ui, "Send marks", &body)
            + pad * 3.0
            + gap * 2.0
    } else {
        0.0
    };
    let sort_full = text_w(
        ui,
        &format!("{} {}", icons::SORT, app.settings.library_sort.label()),
        &body,
    ) + pad
        + 24.0;
    let items = [
        Item {
            priority: 40,
            full: chips,
            compact: None,
            hideable: true,
        },
        Item {
            priority: 50,
            full: immich,
            compact: None,
            hideable: true,
        },
        Item {
            priority: 80,
            full: sort_full,
            compact: Some(pad + 40.0),
            hideable: false,
        },
        Item {
            priority: 60,
            full: SIZE_SLIDER + gap + tokens::ICON,
            compact: None,
            hideable: true,
        },
        Item::fixed(99, tokens::CONTROL_ROW),
    ];
    let forms = bar_plan::plan(avail, gap, &items);
    Shown {
        chips: forms[0] == Form::Full && chips > 0.0,
        immich: forms[1] == Form::Full && immich > 0.0,
        sort_compact: forms[2] == Form::Compact,
        size: forms[3] == Form::Full,
    }
}

/// Whether the Library shows an Immich source — the only one Download and Remove downloads mean
/// anything for.
fn immich_source(app: &RustroomApp) -> bool {
    matches!(
        app.library.as_ref().map(|l| &l.source),
        Some(LibrarySource::ImmichAlbum { .. })
            | Some(LibrarySource::ImmichBucket { .. })
            | Some(LibrarySource::ImmichPerson { .. })
    )
}

/// The format chips' labels, as `format_chips` draws them — for measuring.
fn chip_labels(app: &RustroomApp) -> Vec<String> {
    app.library
        .as_ref()
        .map(|lib| format_filter::formats_of(lib.scan.iter().map(|e| e.path.as_path())))
        .unwrap_or_default()
        .iter()
        .map(|(ext, count)| format!("{} {count}", ext.to_uppercase()))
        .collect()
}

/// The Library row. `pub(crate)` for the shell's tests, which draw it without the panel.
pub(crate) fn library_row(app: &mut RustroomApp, ui: &mut egui::Ui, ctx: &egui::Context) {
    let row = egui::Rect::from_min_size(ui.cursor().min, egui::vec2(ui.available_width(), HEIGHT));
    let shown = plan_library(app, ui, (row.width() - STATUS_MIN).max(0.0));
    let has_library = app.library.is_some();
    // ── the right-hand cluster, rightmost first ──
    let cluster = ui
        .scope_builder(
            egui::UiBuilder::new()
                .max_rect(row)
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
            |ui| {
                if has_library {
                    more_menu(app, ui, ctx, shown);
                    if shown.size {
                        grid_size_slider(app, ui);
                    }
                    sort_combo(app, ui, shown.sort_compact);
                    if shown.immich {
                        crate::status_immich::immich_buttons(app, ui);
                    }
                    if shown.chips {
                        format_chips(app, ui, true);
                    }
                }
                ui.min_rect().left()
            },
        )
        .inner;
    let right = if cluster.is_finite() {
        cluster - 12.0
    } else {
        row.right()
    };
    // ── the left-hand text: the running job or the status, truncated into what is left ──
    // ⚠ The selected photo's name is NOT here: it is centred in the top row, where Develop has
    // it too, so it does not move on a switch (the author's call, 2026-09-29).
    let free = (right - row.left()).max(0.0);
    let left_rect = egui::Rect::from_min_size(row.min, egui::vec2(free, row.height()));
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(left_rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
        |ui| job_or_status(app, ui),
    );
    ui.allocate_rect(row, egui::Sense::hover());
}

/// The Develop row: the colour under the pointer at the left, the running job or the status at
/// the right.
pub(crate) fn develop_row(app: &mut RustroomApp, ui: &mut egui::Ui) {
    let row = egui::Rect::from_min_size(ui.cursor().min, egui::vec2(ui.available_width(), HEIGHT));
    let readout_end = ui
        .scope_builder(
            egui::UiBuilder::new()
                .max_rect(row)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
            |ui| {
                readout(app, ui);
                ui.min_rect().right()
            },
        )
        .inner;
    let from = if readout_end.is_finite() {
        readout_end + 16.0
    } else {
        row.left()
    };
    let right_rect = egui::Rect::from_min_max(egui::pos2(from, row.top()), row.max);
    ui.scope_builder(
        egui::UiBuilder::new()
            .max_rect(right_rect)
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
        |ui| job_or_status(app, ui),
    );
    ui.allocate_rect(row, egui::Sense::hover());
}

/// A running job — an export, an HDR merge, a preview build, an Immich download — with its Cancel, or else the
/// status line. One idiom for every long background job (the rule the top bar had).
///
/// ⚠ The text truncates into the width it is given, so a long status cannot push anything.
fn job_or_status(app: &mut RustroomApp, ui: &mut egui::Ui) {
    let immich_progress = app.immich_download_progress();
    let (text, cancel): (String, Option<fn(&mut RustroomApp)>) = if !app.export_progress.is_empty()
    {
        (
            app.export_progress.clone(),
            Some(RustroomApp::cancel_export),
        )
    } else if let Some(line) = app.camera_learning.line() {
        (line.to_owned(), Some(RustroomApp::cancel_camera_learning))
    } else if let Some(line) = app.print.line() {
        (line, Some(RustroomApp::cancel_print))
    } else if let Some(line) = app.publish_line() {
        (line, Some(RustroomApp::cancel_publish))
    } else if !app.merges.progress.is_empty() {
        (
            app.merges.progress.clone(),
            Some(RustroomApp::cancel_merges),
        )
    } else if !app.preview_progress.is_empty() {
        (
            app.preview_progress.clone(),
            Some(RustroomApp::cancel_preview_build),
        )
    } else if !immich_progress.is_empty() {
        (immich_progress, Some(RustroomApp::cancel_immich_downloads))
    } else if let Some(line) = app.face_pass_line() {
        (line, Some(RustroomApp::cancel_face_pass))
    } else if let Some(line) = app.tether_line() {
        // ⚠ Tethering is a session, not a job: Shoot and Stop instead of Cancel (M10.18a). Behind
        // every job above, whose Cancel matters more for its minutes; F12 still shoots then.
        tether_controls(app, ui, &line);
        return;
    } else {
        // Piece 4: a newer version's line, only when nothing runs.
        crate::update_flow::notice(app, ui);
        (app.status.clone(), None)
    };
    // ⚠ Cancel first in either direction: in the Library's left-to-right text it leads the line,
    // in Develop's right-to-left corner it lands at the far edge.
    let mut cancel_clicked = false;
    if cancel.is_some() && ui.button("Cancel").clicked() {
        cancel_clicked = true;
    }
    if !text.is_empty() {
        let colour = theme::tokens::of(ui).text2;
        ui.add(
            egui::Label::new(
                egui::RichText::new(&text)
                    .size(tokens::CAPTION)
                    .color(colour),
            )
            .truncate()
            .selectable(false),
        )
        .hover(&text);
    }
    if cancel_clicked {
        if let Some(f) = cancel {
            f(app);
        }
    }
}

/// A tethered session's Stop and Shoot, then its line — the job idiom's order (§7.12).
fn tether_controls(app: &mut RustroomApp, ui: &mut egui::Ui, line: &str) {
    let stopping = app.tether.run.as_ref().is_some_and(|r| r.stopping);
    let stop = ui
        .add_enabled(!stopping, egui::Button::new("Stop"))
        .hover("Ends the tethered session and leaves the camera's card as it is")
        .clicked();
    let shoot = ui
        .add_enabled(!stopping, egui::Button::new("Shoot"))
        .hover("Fires the camera's shutter")
        .clicked();
    let colour = theme::tokens::of(ui).text2;
    ui.add(
        egui::Label::new(
            egui::RichText::new(line)
                .size(tokens::CAPTION)
                .color(colour),
        )
        .truncate()
        .selectable(false),
    )
    .hover(line);
    if stop {
        app.stop_tether();
    }
    if shoot {
        app.shoot_tether();
    }
}

/// The swatch and the three numbers of the colour under the pointer, at a fixed width.
///
/// ⚠⚠ **The label occupies a FIXED width, measured from the widest label that can occur**, so the
/// swatch never moves as a channel changes digit count — reported in use when the readout sat in
/// the top bar's right-to-left cluster, and kept here for the same reason: the numbers must not
/// jump while the pointer moves.
///
/// ⚠ Blank when the pointer is off the image (`canvas::show` clears it every frame) — a stale
/// number gets trusted. Develop only: the Library and Compare/Survey render through other paths.
fn readout(app: &RustroomApp, ui: &mut egui::Ui) {
    let Some(rgb) = app.session.as_ref().and_then(|s| s.readout) else {
        return;
    };
    let (patch, painter) = ui.allocate_painter(
        egui::vec2(READOUT_SWATCH, READOUT_SWATCH),
        egui::Sense::hover(),
    );
    let r = patch.rect;
    painter.rect_filled(
        r,
        egui::CornerRadius::same(2),
        egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]),
    );
    // ⚠ A hairline border, or a near-black or near-white sample is indistinguishable from the
    // panel behind it and the swatch reads as missing rather than as dark.
    painter.rect_stroke(
        r,
        egui::CornerRadius::same(2),
        egui::Stroke::new(1.0, egui::Color32::from_black_alpha(120)),
        egui::StrokeKind::Inside,
    );
    let font = egui::TextStyle::Monospace.resolve(ui.style());
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
            egui::RichText::new(rustroom_app::color_readout::label(rgb))
                .monospace()
                .color(theme::tokens::of(ui).text2),
        ),
    )
    .hover("The colour under the pointer, as red, green and blue from 0 to 255");
}

/// The colour swatch beside the numeric readout, in points.
///
/// ⚠ Large enough to judge a colour against rather than merely notice — 11 pt read as a bullet.
const READOUT_SWATCH: f32 = 14.0;

/// The size slider's rail.
const SIZE_SLIDER: f32 = 80.0;

/// ⋯ — the Library row's rare actions and whatever the plan folded out of the row.
///
/// ⚠⚠ **A menu again, and on purpose** (Reindex was taken out of one on 2026-09-23 because it
/// held a single item). It now holds Reindex, Expand all / Stack all, and whatever did not fit —
/// the rule that doc left for exactly this case: "if a second Library action ever wants this
/// corner, reconsider the menu then". Stack all / Expand all and Reindex were among the controls
/// that overprinted the bar at 1440 pt.
///
/// ⚠ `CloseOnClickOutside`, so a folded size slider can be dragged; the buttons close it
/// themselves.
fn more_menu(app: &mut RustroomApp, ui: &mut egui::Ui, ctx: &egui::Context, shown: Shown) {
    let cfg = egui::containers::menu::MenuConfig::default()
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
    let r = egui::containers::menu::MenuButton::new(
        egui::RichText::new(icons::MORE.to_string()).size(tokens::ICON),
    )
    .config(cfg)
    .ui(ui, |ui| {
        ui.set_min_width(200.0);
        if !shown.size {
            ui.horizontal(|ui| grid_size_slider(app, ui));
        }
        if !shown.chips && !chip_labels(app).is_empty() {
            ui.horizontal_wrapped(|ui| format_chips(app, ui, false));
        }
        if !shown.immich && immich_source(app) {
            crate::status_immich::immich_buttons(app, ui);
        }
        if !shown.size || !shown.chips {
            ui.separator();
        }
        stack_buttons(app, ui);
        // M10.15d: present only while a last send exists — read once per catalog change.
        if app.can_undo_send()
            && ui
                .button("Undo last send")
                .hover("Puts back what the last send of marks replaced in Immich")
                .clicked()
        {
            app.begin_undo_send();
            ui.close();
        }
        let hover = reindex_hover(app);
        if ui.button("Reindex").hover(hover).clicked() {
            app.reindex(ctx);
            ui.close();
        }
    });
    r.0.hover("Shows more actions, such as Reindex and the stack controls");
}

/// What Reindex will do, in the words of whatever is open.
///
/// ⚠ The label stays "Reindex" for every source — one verb, one place. A second name for the same
/// button would be worse than a hover that adapts, and the Immich branch does the same two things:
/// re-read the list, then close gaps in the copy numbering.
fn reindex_hover(app: &RustroomApp) -> &'static str {
    match app.library.as_ref().map(|l| &l.source) {
        Some(LibrarySource::ImmichAlbum { .. }) => {
            "Fetches this album again from Immich and renumbers its virtual copies"
        }
        Some(LibrarySource::ImmichBucket { .. }) => {
            "Fetches this month again from Immich and renumbers its virtual copies"
        }
        _ => "Re-reads the folder and renumbers its virtual copies without gaps",
    }
}

/// How the folder is ordered. Drives the filmstrip too — both read one `entries` list.
///
/// ⚠ `compact`: the glyph alone, its order named on hover — the status row's narrow form.
fn sort_combo(app: &mut RustroomApp, ui: &mut egui::Ui, compact: bool) {
    let mut order = app.settings.library_sort;
    let shown = if compact {
        icons::SORT.to_string()
    } else {
        format!("{} {}", icons::SORT, order.label())
    };
    egui::ComboBox::from_id_salt("library_sort")
        .selected_text(shown)
        .show_ui(ui, |ui| {
            for o in catalog::SortOrder::ALL {
                ui.selectable_value(&mut order, o, o.label());
            }
        })
        .response
        // ⚠ The compact form is the glyph alone, and the promise above that it names its order on
        // hover had no hover behind it until M10.6b's walk found the combo mute.
        .hover(format!(
            "Sorts the photos by {}",
            order.label().to_lowercase()
        ));
    // ⚠ `apply_sort` guards on an actual change itself, since this body runs every frame and
    // re-sorting a folder of hundreds per frame is not free.
    app.apply_sort(order);
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
        .hover("Sets the thumbnail size");
    if r.changed() {
        app.settings.library_cell = cell;
        app.cell_zoom_pending = true;
    }
    if app.cell_zoom_pending && !r.dragged() {
        app.cell_zoom_pending = false;
        app.save_settings();
    }
    ui.label(egui::RichText::new(icons::THUMB_SIZE.to_string()).size(crate::theme::tokens::ICON))
        .hover("Sets the thumbnail size");
}

/// Collapse or expand every stack in the current folder.
///
/// ⚠⚠ **Library only, never Develop** — and `format_chips` above records why, having been moved out
/// of the Develop toolbar for it: "The filter decides what the browser contains, which is a browsing
/// decision — it has no meaning while a single photo is open, and it sat in the Develop toolbar for
/// no better reason than living in a shared row." Stacking is the same kind of decision, so putting
/// these in a shared row would repeat a mistake already corrected once.
///
/// ⚠ Drawn only when stacking is on **and** the folder actually holds a stack, the same rule that
/// hides the format chips in a folder with one format.
///
/// ⚠ **"Expand all", not "Unstack all"** (user's call, of three offered). Stacking here is
/// rule-based rather than manual, so there is nothing to permanently unstack — "Unstack all" could
/// only mean "expand" while reading like "turn the feature off", which Settings also does. The
/// ambiguity was on one label only, so only that label changed.
fn stack_buttons(app: &mut RustroomApp, ui: &mut egui::Ui) {
    if !app.settings.stacking.enabled {
        return;
    }
    let Some(lib) = app.library.as_ref() else {
        return;
    };
    // ⚠ Read off the **already grouped** entries rather than recomputed: `stack::group` has just
    // decided what a stack is, and asking twice invites the two answers to differ.
    if !lib.entries.iter().any(|e| e.stack.is_some()) {
        return;
    }
    // ⚠ The same map the grouping used, or the keys it derives would not match the ones the grid
    // carries — and "Expand all" would silently do nothing, which is the bug this shape already had.
    let all_keys = rustroom_app::stack::stacked_keys_with(
        &lib.entries,
        &app.immich_stacks.of_asset,
        &lib.hand_stacks,
    );
    // ⚠ Symmetrical, so each button is lit exactly when it has something to do — a button that greys
    // out says "already done", where a live one that changes nothing says nothing at all.
    let state = |want: bool| {
        lib.entries
            .iter()
            .any(|e| e.stack.map(|s| s.expanded == want).unwrap_or(false))
    };
    let (any_collapsed, any_expanded) = (state(false), state(true));
    let mut collapse = false;
    let mut expand = false;
    if ui
        .add_enabled(any_collapsed, egui::Button::new("Expand all"))
        .hover("Shows every file in every stack")
        .clicked()
    {
        expand = true;
    }
    if ui
        .add_enabled(any_expanded, egui::Button::new("Stack all"))
        .hover("Collapses every stack to a single photo")
        .clicked()
    {
        collapse = true;
    }
    if collapse || expand {
        if let Some(lib) = app.library.as_mut() {
            lib.stack_expanded.clear();
            if expand {
                lib.stack_expanded = all_keys;
            }
        }
        // ⚠ A rebuild, never a patch — `lib.entries` is what every consumer indexes into.
        app.rebuild_entries(None);
    }
}

/// The Library's format filter: which file formats the grid and filmstrip show.
///
/// ⚠ **Library only.** The filter decides what the browser contains, which is a browsing
/// decision — it has no meaning while a single photo is open, and it sat in the Develop
/// toolbar for no better reason than living in a shared row.
///
/// ⚠ Shown only at two or more formats: a lone control here could only ever blank the grid,
/// which is a footgun with no upside.
///
/// ⚠ **Multi-select, so there is no "All" entry** — showing everything is simply every box
/// ticked, which is both the default and self-evident. An "All" radio alongside per-format
/// radios would additionally make "RAF and JPG but not PNG" unreachable, which is a real
/// combination on a mixed folder.
///
/// ⚠ `rtl`: laid out right to left (the status row) the chips are added in reverse, so the
/// on-screen order stays the order `formats_of` returns; in the ⋯ menu they go top to bottom.
fn format_chips(app: &mut RustroomApp, ui: &mut egui::Ui, rtl: bool) {
    let formats = app
        .library
        .as_ref()
        // ⚠⚠ From the **scan**, never the grouped entries: a collapsed pair leaves no JPEG row, so
        // reading `entries` made the JPEG chip vanish and reappear as stacks were toggled. The
        // folder still holds those JPEGs, so the control that hides them stays put.
        .map(|lib| format_filter::formats_of(lib.scan.iter().map(|e| e.path.as_path())))
        .unwrap_or_default();
    // ⚠⚠ The SAME predicate the filtering uses, not a second copy of the rule. When these were
    // two independent conditions, hiding `jpg` and opening a JPEG-only folder drew no chips AND
    // hid every photo — an empty grid with no way back, reported in use 2026-09-17.
    //
    // ⚠ A single-format folder still draws its chip (user's call, 2026-09-20: the format and
    // its count should always be visible) — lit, since the override shows everything, and
    // **not clickable**, since a click could only hide the whole grid or do nothing.
    // ⚠ The same scan-derived census as the chips, not the grouped entries — see `formats` above.
    let single = format_filter::folder_overrides_hidden(&formats);
    // ⚠ Built rightmost-first, because the enclosing layout runs right to left; reversing here
    // is what keeps the on-screen order the same as `formats_present` returns.
    //
    // ⚠ **Every box stays enabled, including the last ticked one.** Disabling it to prevent an
    // empty grid greyed out whichever format was still shown — which reads, from the other
    // side, as "the ones I did not click went dark". Unticking everything is recoverable in
    // one click because the boxes are still there, so the guard cost more than it bought.
    let mut toggled: Option<String> = None;
    let ordered: Vec<&(String, usize)> = if rtl {
        formats.iter().rev().collect()
    } else {
        formats.iter().collect()
    };
    for (ext, count) in ordered {
        let mut shown = single || !app.filter.format.is_hidden(ext);
        let label = format!("{} {count}", ext.to_uppercase());
        // ⚠ A chip with a dot rather than a checkbox: the dot carries the state in colour, the
        // semantics are the checkbox's exactly (`segmented::chip` reports as one).
        let says = if single {
            "The only format in this folder".to_string()
        } else {
            format!("Shows or hides the {} files", ext.to_uppercase())
        };
        let r = ui
            .add_enabled_ui(!single, |ui| segmented::chip(ui, &mut shown, &label, &says))
            .inner;
        if !single && r.changed() {
            toggled = Some(ext.clone());
        }
    }
    if let Some(ext) = toggled {
        app.toggle_format(&ext);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⚠⚠ **All three states, because an absence alone passes against a toolbar that drew nothing.**
    /// The buttons appear only when stacking is on *and* the folder holds a stack — the same rule
    /// that hides the format chips in a folder with one format.
    #[test]
    fn the_stack_buttons_appear_only_when_there_is_something_to_act_on() {
        let labels = |stacking: bool, paired: bool| -> Vec<String> {
            let ctx = crate::test_ui::ctx();
            let mut app = crate::test_app::for_test();
            app.settings.stacking.enabled = stacking;
            let names: &[&str] = if paired {
                &["a.RAF", "a.JPG", "b.RAF"]
            } else {
                &["a.RAF", "b.RAF"]
            };
            app.library = Some(crate::test_app::library_of_names(names));
            crate::test_ui::labelled_centres_on(
                &ctx,
                egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1400.0, 900.0)),
                Vec::new(),
                |ui| {
                    stack_buttons(&mut app, ui);
                },
            )
            .into_iter()
            .map(|(l, _)| l)
            .collect()
        };

        let on_paired = labels(true, true);
        assert!(
            on_paired.iter().any(|l| l == "Expand all"),
            "a folder with a stack must offer them: {on_paired:?}"
        );
        assert!(on_paired.iter().any(|l| l == "Stack all"), "{on_paired:?}");

        assert!(
            labels(true, false).is_empty(),
            "a folder with no stack must offer nothing"
        );
        assert!(
            labels(false, true).is_empty(),
            "stacking off must offer nothing even when the folder has pairs"
        );
    }

    /// A Library of `n` with a status line, drawn through both rows at `width`.
    fn both_rows(width: f32, n: usize, status: &str) -> Vec<(String, egui::Rect)> {
        let ctx = crate::test_ui::ctx();
        crate::theme::apply(&ctx, &rustroom_app::settings::Settings::default());
        let mut app = crate::test_app::for_test();
        app.library = Some(crate::test_app::library_of(n));
        app.status = status.to_owned();
        crate::test_ui::labelled_rects_on(
            &ctx,
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(width, 400.0)),
            |ui| {
                let ctx = ui.ctx().clone();
                let _ = app.top_bar(ui, &ctx);
                ui.add_space(40.0);
                library_row(&mut app, ui, &ctx);
            },
        )
    }

    #[test]
    fn the_status_row_holds_the_count_and_the_view_settings() {
        let found = both_rows(1440.0, 3, "3 photos");
        let labels: Vec<&str> = found.iter().map(|(l, _)| l.as_str()).collect();
        assert!(labels.contains(&"3 photos"), "{labels:?}");
        assert!(
            labels.iter().any(|l| l.contains("Name")),
            "the sort combo is missing: {labels:?}"
        );
        // ⚠ And the top row no longer carries them: Reindex lives in ⋯ now.
        assert!(!labels.contains(&"Reindex"), "{labels:?}");
    }

    #[test]
    fn nothing_in_either_library_row_overlaps() {
        // ⚠⚠ The bug the two rows exist to fix: at 1440 pt the count ran under the search field
        // and Filter clipped to "Fi". Every labelled widget's rect, both rows, at the 13-inch
        // width and at the floor; no two may intersect. A long status line is the adversary —
        // it must truncate, never push.
        for width in [1440.0, 1000.0, 800.0] {
            // ⚠ Longer than any row at any width tested — the first version of this adversary fit
            // beside the cluster at 800 and so passed against a status that was never truncated.
            let long =
                "12 photos · 24 files — a status line long enough to reach across ".repeat(6);
            let found = both_rows(width, 12, &long);
            assert!(found.len() > 6, "sample at {width}: {found:?}");
            for (i, (a, ra)) in found.iter().enumerate() {
                for (b, rb) in found.iter().skip(i + 1) {
                    // ⚠ One widget can report two nodes with one label (a menu button and its
                    // text); two nodes of the same name are the same control, not a collision.
                    if a == b {
                        continue;
                    }
                    let overlap = ra.intersect(*rb);
                    assert!(
                        overlap.width() <= 0.5 || overlap.height() <= 0.5,
                        "at {width}: {a:?} {ra:?} overlaps {b:?} {rb:?}"
                    );
                }
            }
        }
    }
}
