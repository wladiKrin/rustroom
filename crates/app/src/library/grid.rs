//! The grid's geometry: how many columns fit, and which positions each row holds.

use super::cell::MARK_SCALE;
use super::{empty_library, GridInputs, GridOutcome};
use crate::thumb;
use eframe::egui;
use rustroom_app::thumb_loader::ThumbRequest;

/// The positions in `visible` that belong to grid row `row`.
///
/// ⚠ Positions, never entry indices. Once a format or star filter is on, `visible` is *sparse*;
/// the grid's geometry is a function of the *position* in that list, and conflating the two is
/// the bug this project has now made three times.
pub(super) fn cells_in_row(row: usize, cols: usize, n_visible: usize) -> std::ops::Range<usize> {
    let start = row.saturating_mul(cols).min(n_visible);
    let end = start.saturating_add(cols).min(n_visible);
    start..end
}

/// How many `cell`-wide columns fit in `avail`, given egui's inter-column `spacing`.
///
/// ⚠ `n` cells need `n*cell + (n-1)*spacing`, **not** `n*cell`. Ignoring the gaps overflows
/// whenever the leftover width is smaller than the gaps the columns need — and that gets *more*
/// likely as the window widens, because the gap total grows with the column count while the
/// leftover stays uniform in `[0, cell)`. Which is exactly why it showed on a large monitor and
/// not a small one: at 14 columns the gaps want 104 px of slack, at 7 only 48.
pub(super) fn grid_columns(avail: f32, cell: f32, spacing: f32) -> usize {
    if cell <= 0.0 {
        return 1;
    }
    (((avail + spacing) / (cell + spacing)).floor() as usize).max(1)
}

/// The grid panel: the empty states, the keyboard, the rows of cells, and the thumbnail window.
pub(super) fn panel(
    ui: &mut egui::Ui,
    library: &mut Option<crate::Library>,
    inp: GridInputs,
    out: &mut GridOutcome,
) {
    let GridInputs {
        immich_present,
        rename_draft,
        label_draft,
        catalog,
        live,
        cell,
        auto_advance,
        scroll_was,
        filter,
        remote,
        stack_drag,
        look,
        keys,
        publish,
    } = inp;
    // ⚠ The grid's own gap, 12 each way (`docs/DESIGN.md` §5) — scoped to this panel, because
    // `item_spacing` is also every control row's air and 12 there would double the panel.
    ui.spacing_mut().item_spacing = egui::vec2(12.0, 12.0);
    let Some(lib) = library else {
        out.empty_pick = empty_library(ui, keys);
        return;
    };
    // A folder can be reloaded with fewer photos than the selection remembers. This
    // guards a *different* failure from filtering, so it stays.
    lib.selection.clamp(lib.entries.len());
    if lib.visible.is_empty() {
        // ⚠ The reason, not a guess: a search, a mark filter and the format chips each empty the
        // grid differently, and each needs a different thing undone (reported 2026-09-30).
        let sentence = lib.why_empty(filter).sentence(&filter.text);
        ui.centered_and_justified(|ui| {
            crate::theme::empty_state(ui, &sentence);
        });
        return;
    }

    // Keyboard: arrows move the selection, Enter opens it.
    //
    // ⚠⚠ **Suppressed while anything has keyboard focus.** With a name being edited, arrow
    // keys must move the cursor inside the field and Enter must commit — not step to the next
    // photo and open it. This is the same gate the filmstrip's arrow navigation already uses,
    // and it was simply missing here.
    let typing = ui.ctx().memory(|m| m.focused().is_some());
    // ⚠ Hoisted above the keyboard block because a vertical arrow steps by a *row*, and a row
    // is `cols` wide. It was computed further down, next to the layout that also needs it —
    // one value, read twice, rather than two derivations that could disagree about the shape
    // of the grid the arrows are moving through.
    let cols = grid_columns(ui.available_width(), cell, ui.spacing().item_spacing.x);
    let mut centre_on: Option<usize> = None;
    ui.input(|i| {
        if typing {
            return;
        }
        centre_on = super::keys::grid_keys(i, lib, cols, auto_advance, out);
    });

    // ⚠ How many rows of prefetch to keep resident beyond the drawn ones. Latency, not
    // correctness: at 0 the grid is still right, it just shows grey while you scroll. No test
    // covers this number for that reason.
    const MARGIN_ROWS: usize = 2;

    // Cloned so the loop body can mutate `lib.selection`. A few hundred `usize`
    // per frame is nothing against the thumbnails being drawn.
    let visible = lib.visible.clone();
    let rows = visible.len().div_ceil(cols);
    // ⚠ The caption scales *relative to the theme's own* Small size rather than from a
    // literal, so a later change to the type scale carries through here for free.
    let name_size = ui.style().text_styles[&egui::TextStyle::Small].size * cell
        / rustroom_app::settings::DEFAULT_CELL as f32;

    background(ui, out);

    // The cell's geometry: a fixed 3:2 picture box and a height that is exactly the box,
    // the caption and the marks row — `cell_layout::grid_cell`'s rule.
    let layout = {
        let spacing_y = ui.spacing().item_spacing.y;
        let caption_h = ui.fonts_mut(|f| f.row_height(&egui::FontId::proportional(name_size)));
        rustroom_app::cell_layout::grid_cell(
            cell,
            caption_h,
            thumb::marks_row_height(name_size * MARK_SCALE),
            spacing_y,
        )
    };
    // ⚠ `show_rows` adds `item_spacing.y` itself, so the height passed is the bare cell —
    // and the *pitch* the centring works in is therefore the cell plus that gap.
    let pitch = layout.height + ui.spacing().item_spacing.y;
    let viewport_h = ui.available_height();
    let mut area = egui::ScrollArea::vertical()
        // ⚠ The scrollbar belongs at the window's right edge, not hugging the rightmost
        // thumbnail. With `auto_shrink` egui puts the leftover width *outside* the scroll
        // area, so the bar tracked the grid and moved whenever the column count or cell size
        // changed. `false` puts that space inside instead, pinning the bar.
        .auto_shrink([false, false]);
    // ⚠⚠ **Arithmetic, not `scroll_to_me`.** egui computes a `scroll_to_me` target offset
    // **once** and never recomputes it, which is the drift bug the filmstrip already paid
    // for: the centred photo slid away as thumbnails ahead of it loaded and reflowed the
    // content. A grid of fixed-height rows can be placed exactly instead.
    if let Some(offset) = centre_on.and_then(|pos| {
        rustroom_app::grid_nav::follow_offset(pos, cols, pitch, viewport_h, rows, scroll_was)
    }) {
        area = area.vertical_scroll_offset(offset);
    }
    let downloaded = downloaded_flags(lib, immich_present);
    let drawn = area.show_rows(ui, layout.height, rows, |ui, row_range| {
        for row in row_range.clone() {
            ui.horizontal(|ui| {
                for pos in cells_in_row(row, cols, visible.len()) {
                    let i = visible[pos];
                    super::cell::draw(
                        ui,
                        lib,
                        i,
                        super::cell::CellInputs {
                            rename_draft: &mut *rename_draft,
                            label_draft: &mut *label_draft,
                            catalog,
                            live,
                            downloaded: &downloaded,
                            layout,
                            cell,
                            name_size,
                            remote,
                            stack_drag,
                            look,
                            publish,
                        },
                        out,
                    );
                }
            });
        }
        row_range
    });
    // ⚠ Remembered for the *next* frame's follow decision: `follow_offset` has to know where
    // the view is before the scroll area is built, and by then this frame's output is gone.
    out.scroll_now = Some(drawn.state.offset.y);
    let drawn = drawn.inner;

    // ⚠ The window is the drawn rows widened by the prefetch margin, mapped through
    // positions to entry indices to paths. Built after drawing so it reflects what egui
    // actually laid out this frame.
    let first = drawn.start.saturating_sub(MARGIN_ROWS);
    let last = drawn.end.saturating_add(MARGIN_ROWS).min(rows);
    let mut window: Vec<ThumbRequest> = Vec::new();
    for row in first..last {
        for pos in cells_in_row(row, cols, visible.len()) {
            // ⚠ **No longer de-duplicated by path.** A file's copies were separate rows
            // sharing one cache entry while a thumbnail was the camera's JPEG; now each copy
            // renders its own edits, so each is its own request.
            window.push(lib.thumb_request(visible[pos], live.as_ref()));
            // ⚠ M10.8c after use: a collapsed stack's pile shows the photos below it — their
            // thumbnails too, up to the pile's depth, but only when peeking: cards and None need
            // none.
            if look.loads_below() {
                let e = &lib.entries[visible[pos]];
                if e.stack.is_some_and(|s| !s.expanded) {
                    for m in lib
                        .below
                        .get(&e.id)
                        .into_iter()
                        .flatten()
                        .take(rustroom_app::cell_layout::PILE_LAYERS)
                    {
                        window.push(ThumbRequest {
                            id: m.id.clone(),
                            edits: m.edits.clone(),
                        });
                    }
                }
            }
        }
    }
    lib.sync_thumb_window(&window);
}

/// The drag-sensing background, registered before the cells.
fn background(ui: &mut egui::Ui, out: &mut GridOutcome) {
    // ⚠⚠ **A drag-sensing background, registered BEFORE the cells so the cells win the
    // hit-test.** egui breaks a tie by registration order — `hit_test.rs`: "In case of a tie,
    // take the last one = the one on top" — so this must come first, or it would swallow every
    // click meant for a thumbnail. It is what lets a rubber band start in the gaps between
    // cells and in the empty area below the last row.
    //
    // ⚠ The scrollbar is drawn by the `ScrollArea` afterwards and is therefore on top, so
    // dragging it still scrolls rather than banding.
    let bg_rect = ui.available_rect_before_wrap();
    let bg = ui.interact(
        bg_rect,
        ui.id().with("grid_background"),
        egui::Sense::click_and_drag(),
    );
    if bg.drag_started_by(egui::PointerButton::Primary) {
        out.band_started = bg.interact_pointer_pos();
    }
    // ⚠ A *click* on empty space clears the selection. egui fires `clicked()` only for a
    // press-and-release that did **not** become a drag, so this and the rubber band above
    // cannot both happen from one gesture.
    out.bg_clicked = bg.clicked();
}

/// One bool per entry: whether its Immich original is already on disk.
fn downloaded_flags(
    lib: &crate::Library,
    immich_present: &std::collections::HashSet<std::ffi::OsString>,
) -> Vec<bool> {
    // ⚠ Computed once for the whole grid, not per cell: `source` does not change mid-draw, and
    // `present` is the pre-read set rather than a stat. See `immich_catalog::shows_download_badge`.
    // ⚠⚠ **One bool per entry, decided before the closure — not a set cloned into it.** `lib` is a
    // `&mut` borrow of `app`, so the closure cannot also read `app.immich_present`; cloning the set
    // would work and would allocate a thousand short strings **every frame**, which is the
    // per-frame-work mistake this file has paid for twice. A `Vec<bool>` is one pass and one
    // kilobyte, and `shows_download_badge` stays the only place that decides.
    let immich_grid = matches!(
        lib.source,
        crate::LibrarySource::ImmichAlbum { .. }
            | crate::LibrarySource::ImmichBucket { .. }
            | crate::LibrarySource::ImmichPerson { .. }
    );
    let downloaded: Vec<bool> = if immich_grid {
        lib.entries
            .iter()
            .map(|e| {
                rustroom_app::immich_catalog::shows_download_badge(true, &e.id.path, immich_present)
            })
            .collect()
    } else {
        Vec::new()
    };
    downloaded
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::show;
    use eframe::egui;

    #[test]
    fn grid_columns_leaves_room_for_the_gaps() {
        // ⚠ The property, over a sweep of window widths: the chosen columns must *fit* with
        // their gaps, and one more must not — so it is neither overflowing nor wasteful.
        const CELL: f32 = 168.0;
        const GAP: f32 = 8.0;
        let width_of = |n: f32| n * CELL + (n - 1.0) * GAP;
        for avail in [300.0f32, 700.0, 1200.0, 1850.0, 2400.0, 3000.0] {
            let n = grid_columns(avail, CELL, GAP) as f32;
            assert!(
                width_of(n) <= avail,
                "{n} columns need {} > {avail}",
                width_of(n)
            );
            assert!(
                width_of(n + 1.0) > avail,
                "{} columns would also have fit in {avail}",
                n + 1.0
            );
        }

        // The naive `avail / cell` overflows precisely where this does not — the bug, pinned.
        let naive = (2400.0f32 / CELL).floor();
        assert!(
            width_of(naive) > 2400.0,
            "the naive formula should overflow"
        );
        assert!((grid_columns(2400.0, CELL, GAP) as f32) < naive);

        // Degenerate inputs still give a usable answer rather than 0 columns or a panic.
        assert_eq!(grid_columns(0.0, CELL, GAP), 1);
        assert_eq!(grid_columns(-50.0, CELL, GAP), 1);
        assert_eq!(grid_columns(500.0, 0.0, GAP), 1);
    }

    #[test]
    fn rows_consume_the_visible_positions_densely_and_in_order() {
        // ⚠ Replaces `the_grid_breaks_rows_by_position_not_by_entry_index`. That bug — counting
        // rows by a *sparse* entry index — is now structurally impossible, because a row is
        // computed from the position directly. What still has to hold is that the rows together
        // cover every position exactly once, in order, with only the last one partial.
        const COLS: usize = 4;
        for n in [0usize, 1, 3, 4, 5, 11, 12] {
            let rows = n.div_ceil(COLS);
            let mut seen = Vec::new();
            for row in 0..rows {
                let span = cells_in_row(row, COLS, n);
                assert!(
                    span.len() <= COLS,
                    "row {row} of {n} holds {} cells",
                    span.len()
                );
                seen.extend(span);
            }
            assert_eq!(seen, (0..n).collect::<Vec<_>>(), "n = {n}");
        }
    }

    #[test]
    fn a_row_past_the_end_is_empty() {
        // A stale scroll offset can name a row that no longer exists after a filter change.
        assert!(cells_in_row(99, 4, 10).is_empty());
    }

    #[test]
    fn only_the_visible_rows_are_asked_for() {
        // ⚠⚠ The point of the whole change, asserted where it is observable: with 2,000 entries
        // and a viewport a few rows tall, the want-list must be a few dozen paths, not 2,000.
        //
        // ⚠ Asserted on the *want-list*, not on the drawn cell count: drawing fewer cells is
        // egui's `show_rows` doing its job, while asking for fewer thumbnails is ours. The
        // memory and the 34-second fill are both consequences of the ask.
        let ctx = crate::test_ui::ctx();
        let mut app = crate::test_app::for_test();
        app.view = crate::AppView::Library;
        app.library = Some(crate::test_app::library_of(2000));

        let input = egui::RawInput {
            screen_rect: Some(crate::test_ui::screen()),
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| {
            let ctx = ui.ctx().clone();
            show(&mut app, ui, &ctx);
        });

        let asked = app.library.as_ref().unwrap().last_want.len();
        assert!(asked > 0, "nothing was asked for at all");
        assert!(
            asked < 300,
            "asked for {asked} thumbnails — the grid is not windowed"
        );
    }

    /// ⚠⚠ **Reported in use (2026-09-30):** gibberish in the search bar emptied the grid, and the grid
    /// said "Every format is hidden" — the one reason it knew besides an empty folder.
    #[test]
    fn a_search_that_matches_nothing_says_so() {
        let ctx = crate::test_ui::ctx();
        let mut app = crate::test_app::for_test();
        app.library = Some(crate::test_app::library_of(4));
        app.filter.text = "qwxz".into();
        let filter = app.filter.clone();
        app.library.as_mut().unwrap().refilter(&filter);
        assert!(
            app.library.as_ref().unwrap().visible.is_empty(),
            "the search hid nothing"
        );
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0));
        let mut body = |ui: &mut egui::Ui| {
            let c = ui.ctx().clone();
            show(&mut app, ui, &c);
        };
        let labels: Vec<String> = crate::test_ui::nodes_on(&ctx, screen, &mut body)
            .into_iter()
            .map(|n| n.label)
            .collect();
        assert!(
            labels
                .iter()
                .any(|l| l == "No photo's name contains “qwxz”."),
            "{labels:?}"
        );
        assert!(
            !labels.iter().any(|l| l.contains("Every format is hidden")),
            "{labels:?}"
        );
    }
}
