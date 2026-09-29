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

/// The trailing count for the section header: how many albums, once known.
pub fn header_count(app: &RustroomApp) -> String {
    app.immich_albums
        .as_ref()
        .and_then(|r| r.as_ref().ok())
        .map(|v| v.len().to_string())
        .unwrap_or_default()
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

    match &app.immich_albums {
        None => {
            if app.immich.is_listing() {
                ui.label(egui::RichText::new("Loading albums…").weak().small());
            } else {
                // ⚠⚠ Asking happens **here**, on the first frame the section is drawn open, rather
                // than when the header is clicked: the header's open state is persisted, so a
                // restart reopens it with nothing having been clicked at all.
                action = Action::Refresh;
            }
        }
        Some(Err(e)) => {
            // ⚠ The server's own message, which distinguishes a rejected key from an unreachable
            // host. That distinction is the whole reason `immich::Error` has variants.
            ui.label(egui::RichText::new(e).weak().small());
            if ui.small_button("Retry").clicked() {
                action = Action::Refresh;
            }
        }
        Some(Ok(albums)) => {
            if albums.is_empty() {
                ui.label(
                    egui::RichText::new("No albums on the server")
                        .weak()
                        .small(),
                );
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
                        indent: 0.0,
                        hover: "Shows these photos, downloading each one when you open it",
                        actions: &acts,
                    },
                );
                if r.clicked {
                    action = Action::Open {
                        id: a.id.clone(),
                        name: a.album_name.clone(),
                    };
                }
                // ⚠⚠ Read by **verb**, not by index. The slice's length now varies with what is
                // downloaded, so a hard-coded `0`/`1` would download when you meant to remove as soon as
                // one of the two was absent — which is the whole point of them being absent.
                match r.action.and_then(|i| acts.get(i)).map(|a| a.verb) {
                    Some("Download") => {
                        action = Action::DownloadAlbum {
                            id: a.id.clone(),
                            name: a.album_name.clone(),
                        }
                    }
                    Some("Remove downloads") => {
                        action = Action::RemoveAlbum {
                            id: a.id.clone(),
                            name: a.album_name.clone(),
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    // ⚠⚠ The timeline is how the photos in **no album** are reached — most of them, for most
    // people. Albums above; the whole library by month here.
    ui.add_space(4.0);
    egui::CollapsingHeader::new("Timeline")
        .id_salt("immich_timeline")
        .show(ui, |ui| match &app.immich_buckets {
            None => {
                if app.immich.is_listing() {
                    ui.label(egui::RichText::new("Loading…").weak().small());
                } else {
                    action = Action::RefreshBuckets;
                }
            }
            Some(Err(e)) => {
                ui.label(egui::RichText::new(e).weak().small());
            }
            Some(Ok(buckets)) => {
                let years = rustroom_app::immich_source::group_buckets(buckets);
                if years.is_empty() {
                    ui.label(egui::RichText::new("Nothing on the server").weak().small());
                }
                for y in years {
                    // ⚠ A row with its count in the rail's column, not a collapsing header whose
                    // label carried it — `date_rows` says why. The id is the header's old one, so an
                    // open year stays open; salted, because egui's auto-ids shift with what was
                    // drawn before them and two years would otherwise collapse together.
                    let year_id = ui.make_persistent_id(("immich_year", y.year));
                    if crate::date_rows::branch(ui, year_id, &y.year.to_string(), y.count as i64, 0)
                    {
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
                                    indent: list_row::tree_indent(1),
                                    hover:
                                        "Shows these photos, downloading each one when you open it",
                                    actions: &[],
                                },
                            );
                            if r.clicked {
                                action = Action::OpenBucket {
                                    bucket: m.bucket.clone(),
                                    label,
                                };
                            }
                        }
                    }
                }
            }
        });

    action
}
