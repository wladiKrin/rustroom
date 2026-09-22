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
pub enum Action {
    None,
    /// Ask the server for its albums.
    Refresh,
    /// Open this album.
    Open { id: String, name: String },
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
                ui.label(egui::RichText::new("No albums on the server").weak().small());
            }
            for a in albums {
                let selected = matches!(
                    app.library.as_ref().map(|l| &l.source),
                    Some(crate::LibrarySource::ImmichAlbum { id, .. }) if id == &a.id
                );
                let r = list_row::row(
                    ui,
                    RowSpec {
                        name: &a.album_name,
                        // ⚠ `i64`, not `usize` — see `list_row::RowSpec::count`.
                        count: Some(a.asset_count as i64),
                        meta: "",
                        selected,
                        indent: 0.0,
                        hover: "Photos are downloaded when you open them",
                        actions: &[],
                    },
                );
                if r.clicked {
                    action = Action::Open {
                        id: a.id.clone(),
                        name: a.album_name.clone(),
                    };
                }
            }
        }
    }

    action
}
