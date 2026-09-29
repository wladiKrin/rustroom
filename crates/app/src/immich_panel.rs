//! The Immich section of the left rail — a header at the same level as Folders and Collections.
//!
//! ⚠ **A bin module**: egui lives here, never in `immich_source` or `immich_worker`.
//!
//! ⚠⚠ Nothing here calls the server. A click returns an [`Action`] the panel applies, which
//! enqueues a job on the worker; the result lands on a later frame. A blocking HTTP call inside a
//! frame is a frozen window.

use crate::list_row::{self, RowSpec};
use crate::RustroomApp;
use eframe::egui;

/// What a click in this section asked for.
/// The row actions an Immich album or month row offers, given what is already downloaded.
///
/// ⚠⚠ **Only the applicable ones.** Nothing to download when every photo is here; nothing to remove when
/// none is (user's call, 2026-09-24) — a button that cannot do anything is worse than no button.
///
/// ⚠ `downloaded` is `None` when the asset list is not in hand, and then **both** are offered: the rail
/// genuinely does not know, and pressing one loads the list and says to press again. Guessing "neither"
/// would make an album unreachable.
///
/// ⚠⚠ **Both glyphs are verified against the bundled fonts, by parsing their cmaps.** `\u{2b07}` is in
/// NotoEmoji **and** emoji-icon-font, both of which egui puts in the `Proportional` family, and NotoEmoji
/// is monochrome so it takes the painter's colour. `\u{d7}` is in Ubuntu-Light and is what the Folders
/// rail's Forget already uses. ⚠ The first attempt used `\u{2193}` and `\u{2715}`: the first is in **Hack
/// only**, which is the *Monospace* family, and the second is in **no bundled font at all** — so both
/// shipped as grey squares, reported immediately. `icons.rs`'s doc warns about exactly this, and four
/// features had already paid for it.
fn row_actions(downloaded: Option<(usize, usize)>) -> Vec<list_row::RowAction<'static>> {
    let download = list_row::RowAction {
        glyph: '\u{2b07}',
        verb: "Download",
        hover: "Downloads every photo here for offline use",
        enabled: true,
    };
    let remove = list_row::RowAction {
        glyph: '\u{d7}',
        verb: "Remove downloads",
        hover: "Deletes the local copies only, keeping the photos on your Immich server",
        enabled: true,
    };
    match downloaded {
        None => vec![download, remove],
        Some((0, _)) => vec![download],
        Some((have, total)) if have >= total => vec![remove],
        Some(_) => vec![download, remove],
    }
}

pub enum Action {
    None,
    /// Ask the server for its albums.
    Refresh,
    /// Open this album.
    Open {
        id: String,
        name: String,
    },
    /// Predownload every photo in this album.
    DownloadAlbum {
        id: String,
        name: String,
    },
    /// Delete the local copies of this album's photos. ⚠ Never touches the server.
    RemoveAlbum {
        id: String,
        name: String,
    },
    /// Ask the server for its month buckets.
    RefreshBuckets,
    /// Open one month of the timeline.
    OpenBucket {
        bucket: String,
        label: String,
    },
}

/// A month's name, for the timeline's rows.
///
/// ⚠ `"?"` rather than a panic for an impossible month: `group_buckets` already refuses one, so this
/// is unreachable — and a rail that panics is worse than a rail with a question mark in it.
fn month_name(month: u32) -> &'static str {
    match month {
        1 => "January",
        2 => "February",
        3 => "March",
        4 => "April",
        5 => "May",
        6 => "June",
        7 => "July",
        8 => "August",
        9 => "September",
        10 => "October",
        11 => "November",
        12 => "December",
        _ => "?",
    }
}

/// Draw the section's body. Returns what was clicked.
pub fn body(app: &RustroomApp, ui: &mut egui::Ui) -> Action {
    let mut action = Action::None;

    if rustroom_app::immich_source::connection(&app.settings).is_none() {
        // ⚠ Quiet, not an error: an unconfigured server is the normal state for everyone who does
        // not use Immich, and this section is drawn for them too.
        ui.label(
            egui::RichText::new("No server configured — add one in Settings")
                .weak()
                .small(),
        );
        return action;
    }

    // ⚠⚠ The Timeline first (the author's to-do, M10.6a): it is how the photos in **no album** are
    // reached — most of them, for most people. Then the albums.
    timeline(app, ui, &mut action);
    albums(app, ui, &mut action);
    action
}

/// Ask for a list unless something this frame already asked for something — `body` returns one
/// `Action`, and a click must never lose to a background request (which is made again next frame).
fn ask(action: &mut Action, request: Action) {
    if matches!(action, Action::None) {
        *action = request;
    }
}

/// A quiet line under an open row: loading, empty, or the server's own error.
fn note(ui: &mut egui::Ui, text: &str) {
    ui.horizontal(|ui| {
        ui.add_space(list_row::tree_indent(1));
        ui.label(egui::RichText::new(text).weak().small());
    });
}

/// The Timeline row's id — the old `CollapsingHeader`'s exactly, so an open Timeline stays open.
///
/// ⚠⚠ **Two things make it the header's id, and both were found by the test, not by reading.**
/// (1) `IdSalt::new`, not the bare string: the header hashed its salt into an `IdSalt` before
/// `make_persistent_id`. (2) It must be read inside a `ui.vertical` — the header drew itself in one
/// (`CollapsingHeader::show_dyn`), whose child `Ui` adds a level to the id. [`timeline`] does that.
/// `the_timeline_keeps_the_old_headers_open_state` holds both to the real header.
fn timeline_id(ui: &egui::Ui) -> egui::Id {
    ui.make_persistent_id(egui::IdSalt::new("immich_timeline"))
}

/// The Timeline row and, open, its years and months — inside a `ui.vertical`, as the old header
/// was, which is half of what keeps its open state (see [`timeline_id`]).
fn timeline(app: &RustroomApp, ui: &mut egui::Ui, action: &mut Action) {
    ui.vertical(|ui| timeline_rows(app, ui, action));
}

fn timeline_rows(app: &RustroomApp, ui: &mut egui::Ui, action: &mut Action) {
    // ⚠ Asked for with the section, not with the row, so the closed row shows its count.
    if app.immich_buckets.is_none() && !app.immich.is_listing_buckets() {
        ask(action, Action::RefreshBuckets);
    }
    let total = match &app.immich_buckets {
        Some(Ok(b)) => Some(rustroom_app::immich_source::photo_total(b) as i64),
        _ => None,
    };
    let open = crate::date_rows::branch_with(
        ui,
        timeline_id(ui),
        crate::date_rows::BranchSpec {
            name: "Timeline",
            count: total,
            depth: 0,
            default_open: false,
            hover: "Shows the server's whole library by year and month",
        },
    );
    if !open {
        return;
    }
    match &app.immich_buckets {
        None => note(ui, "Loading…"),
        Some(Err(e)) => note(ui, e),
        Some(Ok(buckets)) => {
            let years = rustroom_app::immich_source::group_buckets(buckets);
            if years.is_empty() {
                note(ui, "Nothing on the server");
            }
            for y in years {
                // ⚠ A row with its count in the rail's column, not a collapsing header whose
                // label carried it — `date_rows` says why. Salted, because egui's auto-ids shift
                // with what was drawn before them and two years would otherwise collapse together.
                let year_id = ui.make_persistent_id(("immich_year", y.year));
                if crate::date_rows::branch(ui, year_id, &y.year.to_string(), y.count as i64, 1) {
                    for m in &y.months {
                        let label = format!("{} {}", month_name(m.month), y.year);
                        let selected = app.immich_opening.as_deref() == Some(m.bucket.as_str())
                            || matches!(
                                app.library.as_ref().map(|l| &l.source),
                                Some(crate::LibrarySource::ImmichBucket { bucket, .. })
                                    if bucket == &m.bucket
                            );
                        let r = list_row::row(
                            ui,
                            RowSpec {
                                name: &label,
                                count: Some(m.count as i64),
                                meta: "",
                                selected,
                                indent: list_row::tree_indent(2),
                                hover: "Shows these photos, downloading each one when you open it",
                                actions: &[],
                            },
                        );
                        if r.clicked {
                            *action = Action::OpenBucket {
                                bucket: m.bucket.clone(),
                                label,
                            };
                        }
                    }
                }
            }
        }
    }
}

/// The Albums row and, open, the album rows.
fn albums(app: &RustroomApp, ui: &mut egui::Ui, action: &mut Action) {
    // ⚠⚠ Asked for on the first frame the section is drawn open rather than when anything is
    // clicked: the section's open state is persisted, so a restart reopens it with nothing clicked.
    if app.immich_albums.is_none() && !app.immich.is_listing_albums() {
        ask(action, Action::Refresh);
    }
    let n = match &app.immich_albums {
        Some(Ok(a)) => Some(a.len() as i64),
        _ => None,
    };
    // ⚠ Open by default: the albums were always in view, and stay there one row lower.
    let open = crate::date_rows::branch_with(
        ui,
        ui.make_persistent_id("immich_albums"),
        crate::date_rows::BranchSpec {
            name: "Albums",
            count: n,
            depth: 0,
            default_open: true,
            hover: "Shows the server's albums",
        },
    );
    if !open {
        return;
    }
    match &app.immich_albums {
        None => note(ui, "Loading albums…"),
        Some(Err(e)) => {
            // ⚠ The server's own message, which distinguishes a rejected key from an unreachable
            // host. That distinction is the whole reason `immich::Error` has variants.
            note(ui, e);
            ui.horizontal(|ui| {
                ui.add_space(list_row::tree_indent(1));
                if ui.small_button("Retry").clicked() {
                    *action = Action::Refresh;
                }
            });
        }
        Some(Ok(albums)) => {
            if albums.is_empty() {
                note(ui, "No albums on the server");
            }
            for a in albums {
                // ⚠⚠ Lit while its assets are still on the way, too. The server takes a couple of
                // seconds over a large album, and without this the row you clicked stays dark the
                // whole time — indistinguishable from a click that did nothing.
                let selected = app.immich_opening.as_deref() == Some(a.id.as_str())
                    || matches!(
                        app.library.as_ref().map(|l| &l.source),
                        Some(crate::LibrarySource::ImmichAlbum { id, .. }) if id == &a.id
                    );
                // ⚠ `None` when the asset list is not in hand — see `row_actions`.
                let acts = row_actions(app.immich_downloaded_of(&a.id));
                let r = list_row::row(
                    ui,
                    RowSpec {
                        name: &a.album_name,
                        // ⚠ `i64`, not `usize` — see `list_row::RowSpec::count`.
                        count: Some(a.asset_count as i64),
                        meta: "",
                        selected,
                        indent: list_row::tree_indent(1),
                        hover: "Shows these photos, downloading each one when you open it",
                        actions: &acts,
                    },
                );
                if r.clicked {
                    *action = Action::Open {
                        id: a.id.clone(),
                        name: a.album_name.clone(),
                    };
                }
                // ⚠⚠ Read by **verb**, not by index. The slice's length now varies with what is
                // downloaded, so a hard-coded `0`/`1` would download when you meant to remove as soon as
                // one of the two was absent — which is the whole point of them being absent.
                match r.action.and_then(|i| acts.get(i)).map(|a| a.verb) {
                    Some("Download") => {
                        *action = Action::DownloadAlbum {
                            id: a.id.clone(),
                            name: a.album_name.clone(),
                        }
                    }
                    Some("Remove downloads") => {
                        *action = Action::RemoveAlbum {
                            id: a.id.clone(),
                            name: a.album_name.clone(),
                        }
                    }
                    _ => {}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_ui;

    fn app(buckets: Option<Vec<u32>>) -> RustroomApp {
        let mut app = crate::test_app::for_test();
        app.settings.immich_enabled = true;
        app.settings.immich_url = "https://immich.example".to_string();
        app.settings.immich_key = "a-key".to_string();
        app.immich_albums = Some(Ok(vec![immich::AlbumSummary {
            id: "a1".to_string(),
            album_name: "Holiday".to_string(),
            asset_count: 1234,
            last_modified_asset_timestamp: None,
        }]));
        app.immich_buckets = buckets.map(|counts| {
            Ok(counts
                .into_iter()
                .enumerate()
                .map(|(i, count)| immich::Bucket {
                    time_bucket: format!("2025-{:02}-01T00:00:00.000Z", i + 1),
                    count,
                })
                .collect())
        });
        app
    }

    fn labels(ctx: &egui::Context, app: &RustroomApp) -> Vec<(String, egui::Pos2)> {
        test_ui::labelled_centres(ctx, Vec::new(), |ui| {
            let _ = body(app, ui);
        })
    }

    fn y_of(all: &[(String, egui::Pos2)], label: &str) -> f32 {
        all.iter()
            .find(|(l, _)| l == label)
            .map(|(_, p)| p.y)
            .unwrap_or_else(|| panic!("no {label:?} drawn: {all:?}"))
    }

    /// Every all-digit text painted, i.e. the counts.
    fn painted_counts(ctx: &egui::Context, app: &RustroomApp) -> Vec<String> {
        let out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(test_ui::screen()),
                ..Default::default()
            },
            |ui| {
                let _ = body(app, ui);
            },
        );
        fn walk(shape: &egui::Shape, found: &mut Vec<String>) {
            match shape {
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, found)),
                egui::Shape::Text(t) => {
                    let text = t.galley.text();
                    if !text.is_empty() && text.chars().all(|c| c.is_ascii_digit()) {
                        found.push(text.to_owned());
                    }
                }
                _ => {}
            }
        }
        let mut found = Vec::new();
        for c in &out.shapes {
            walk(&c.shape, &mut found);
        }
        found.sort();
        found
    }

    #[test]
    fn the_timeline_is_drawn_above_the_albums() {
        let ctx = test_ui::ctx();
        let all = labels(&ctx, &app(Some(vec![57])));
        assert!(y_of(&all, "Timeline") < y_of(&all, "Albums"), "{all:?}");
        assert!(y_of(&all, "Albums") < y_of(&all, "Holiday"), "{all:?}");
    }

    /// Both halves on one fresh context: the albums show with no click, the years do not — and
    /// opening the Timeline shows them, so their absence means something.
    #[test]
    fn albums_start_open_and_the_timeline_closed() {
        let ctx = test_ui::ctx();
        let a = app(Some(vec![57]));
        let all = labels(&ctx, &a);
        assert!(all.iter().any(|(l, _)| l == "Holiday"), "albums closed: {all:?}");
        assert!(!all.iter().any(|(l, _)| l == "2025"), "timeline open: {all:?}");
        test_ui::click(&ctx, "Timeline", |ui| {
            let _ = body(&a, ui);
        });
        let all = labels(&ctx, &a);
        assert!(all.iter().any(|(l, _)| l == "2025"), "{all:?}");
    }

    #[test]
    fn the_timeline_counts_every_photo_and_nothing_before_the_months_arrive() {
        let ctx = test_ui::ctx();
        // 57 + 43: the Timeline's 100, beside the album count 1 and the album's 1234.
        assert_eq!(
            painted_counts(&ctx, &app(Some(vec![57, 43]))),
            ["1", "100", "1234"]
        );
        // ⚠ Exactly these, so a `Some(0)` for an unknown total — a "0" — fails too.
        assert_eq!(painted_counts(&test_ui::ctx(), &app(None)), ["1", "1234"]);
    }

    #[test]
    fn a_closed_timeline_still_asks_for_its_months() {
        let ctx = test_ui::ctx();
        let a = app(None);
        let mut got = Action::None;
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(test_ui::screen()),
                ..Default::default()
            },
            |ui| got = body(&a, ui),
        );
        assert!(matches!(got, Action::RefreshBuckets));
    }

    /// ⚠⚠ Against the REAL old header, and through `body` — not `timeline_id` against itself, which
    /// could not catch either of the two ways the id differed (an `IdSalt`, and the header's own
    /// `ui.vertical`), both of which this test found. `albums_start_open_and_the_timeline_closed`
    /// is its control: on a fresh context the years are absent.
    #[test]
    fn the_timeline_keeps_the_old_headers_open_state() {
        let ctx = test_ui::ctx();
        test_ui::click(&ctx, "Timeline", |ui| {
            egui::CollapsingHeader::new("Timeline")
                .id_salt("immich_timeline")
                .show(ui, |ui| {
                    ui.label("body");
                });
        });
        let all = labels(&ctx, &app(Some(vec![57])));
        assert!(
            all.iter().any(|(l, _)| l == "2025"),
            "the old header's open state did not reach the new row: {all:?}"
        );
    }
}
