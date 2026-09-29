//! The Dates section's tree rows: each year, its months, their days — every one a `list_row` with
//! its count in the rail's count column and a disclosure triangle for the level beneath.
//!
//! ⚠ Its own module for `folder_rows`' reason: `collections_panel.rs` is past 850 lines of code.
//! The grouping is `rustroom_app::date_tree`, pure and tested; this draws it.
//!
//! ⚠⚠ **Rows, not `egui::CollapsingHeader`s** (2026-09-29): a collapsing header can carry its
//! count only inside its label — "2024 (523)" — so the Dates counts stood wherever each label
//! happened to end, never in the column every other count in the rail shares ("all the numbers
//! should be to the most right", the author). The interaction is the header's: a click on a year or
//! a month opens and closes it, and "All of 2024" opens the period. The open state is kept under the
//! headers' old ids, so a tree left open stays open across the change.

use crate::list_row::{self, RowSpec};
use catalog::db::DayRow;
use eframe::egui;
use egui::collapsing_header::CollapsingState;
use rustroom_app::date_tree::{self, Period};

/// Draw the tree; returns the period a row asked to open, with the click's modifiers — a Cmd- or
/// Shift-click adds to the selection rather than replacing it.
pub(crate) fn rows(
    ui: &mut egui::Ui,
    days: &[DayRow],
    open_periods: &[Period],
) -> Option<(Period, egui::Modifiers)> {
    let mut out = None;
    for y in date_tree::group(days) {
        // ⚠ The id the year's `CollapsingHeader` used (`id_salt(("year", y.year))`): egui's
        // auto-ids shift with what was drawn before them, so every level is salted explicitly.
        let year_id = ui.make_persistent_id(("year", y.year));
        if !branch(ui, year_id, &y.year.to_string(), y.count, 0) {
            continue;
        }
        let all_year: Period = (y.year, None, None);
        if leaf(
            ui,
            &format!("All of {}", y.year),
            None,
            open_periods.contains(&all_year),
            1,
        ) {
            out = Some((all_year, ui.input(|i| i.modifiers)));
        }
        for m in &y.months {
            let name = date_tree::month_name(m.month);
            let month_id = ui.make_persistent_id(("month", y.year, m.month));
            if !branch(ui, month_id, name, m.count, 1) {
                continue;
            }
            let all_month: Period = (y.year, Some(m.month), None);
            if leaf(
                ui,
                &format!("All of {name}"),
                None,
                open_periods.contains(&all_month),
                2,
            ) {
                out = Some((all_month, ui.input(|i| i.modifiers)));
            }
            for d in &m.days {
                let day: Period = (y.year, Some(m.month), Some(d.day));
                if leaf(
                    ui,
                    &format!("{} {name}", d.day),
                    Some(d.count),
                    open_periods.contains(&day),
                    2,
                ) {
                    out = Some((day, ui.input(|i| i.modifiers)));
                }
            }
        }
    }
    out
}

/// A year or a month: a row that opens and closes the level beneath, by a click anywhere on it or
/// on its triangle. Returns whether it is open now.
pub(crate) fn branch(
    ui: &mut egui::Ui,
    id: egui::Id,
    name: &str,
    count: i64,
    depth: usize,
) -> bool {
    let mut state = CollapsingState::load_with_default_open(ui.ctx(), id, false);
    let open = state.is_open();
    let indent = list_row::tree_indent(depth);
    let r = list_row::row(
        ui,
        RowSpec {
            name,
            count: Some(count),
            meta: "",
            selected: false,
            indent,
            hover: "",
            actions: &[],
        },
    );
    let toggled = list_row::disclosure(ui, r.rect, indent, open, id.with("disclosure"));
    if r.clicked || toggled {
        state.set_open(!open);
        state.store(ui.ctx());
    }
    state.is_open()
}

/// A period to open: returns whether it was clicked.
fn leaf(ui: &mut egui::Ui, name: &str, count: Option<i64>, selected: bool, depth: usize) -> bool {
    list_row::row(
        ui,
        RowSpec {
            name,
            count,
            meta: "",
            selected,
            indent: list_row::tree_indent(depth),
            hover: "",
            actions: &[],
        },
    )
    .clicked
}
