//! The Library status row's Immich buttons — Download, Remove downloads and Send marks.
//!
//! ⚠ Moved out of `status_bar.rs` (M10.15d), which was at 627 lines of code and doc, past the 600
//! this project flags, when Send marks joined them.

use crate::theme::Hover as _;
use crate::{AppView, LibrarySource, RustroomApp};
use eframe::egui;

/// Download, Remove downloads and Send marks, for the current selection.
///
/// ⚠ **Library only, Immich sources only, and only with a selection.** A local photo is always
/// "downloaded", so these are meaningless on a folder — and drawing a disabled pair on every folder is
/// worse than drawing none.
///
/// ⚠⚠ **"Remove downloads", never "Discard" or "Delete".** The action *cannot* reach the server — the API
/// key is read-only by the Settings help text's own instruction — and "discard" beside a photograph reads
/// as throwing the photograph away, with a hover as the only correction (user's call, 2026-09-24).
pub(crate) fn immich_buttons(app: &mut RustroomApp, ui: &mut egui::Ui) {
    if app.view != AppView::Library {
        return;
    }
    let immich = matches!(
        app.library.as_ref().map(|l| &l.source),
        Some(LibrarySource::ImmichAlbum { .. })
            | Some(LibrarySource::ImmichBucket { .. })
            | Some(LibrarySource::ImmichPerson { .. })
    );
    if !immich {
        return;
    }
    // ⚠ From the ordinary selection — the same question Export asks, so "these twelve" means one thing.
    let picked: Vec<std::path::PathBuf> = app
        .library
        .as_ref()
        .map(|l| {
            l.selection
                .iter()
                .filter_map(|i| l.entries.get(i))
                .map(|e| e.id.path.clone())
                .collect()
        })
        .unwrap_or_default();
    let any = !picked.is_empty();

    if ui
        .add_enabled(any, egui::Button::new("Download"))
        .hover("Downloads the selected photos for offline use")
        .clicked()
    {
        app.download_assets(&picked);
    }
    if ui
        .add_enabled(any, egui::Button::new("Remove downloads"))
        .hover(
            "Deletes the local copies of the selected photos, keeping them on your Immich server",
        )
        .clicked()
    {
        app.remove_downloaded(&picked);
    }
    // M10.15d: never sends by itself — it checks the server and opens the confirmation.
    if ui
        .add_enabled(any, egui::Button::new("Send marks"))
        .hover("Writes the selected photos' marks to Immich after showing every change")
        .clicked()
    {
        app.begin_send_marks();
    }
}
