//! Settings → Immich: the connection, the thumbnail cache and the downloaded originals.
//!
//! ⚠ Moved out of `settings.rs` (M10.15b), which was at 867 lines of code and doc against the
//! 1,000-line ceiling — the `settings_keyboard` / `settings_layout` shape. The draft's Immich fields
//! are `pub(super)` for it.

use super::settings::{SettingsDraft, GAP};
use crate::theme;
use crate::theme::Hover as _;
use eframe::egui;

/// What the tab reads and writes besides the draft — separate borrows, because the dialog holds
/// `&mut app.settings` while the tab draws.
pub(super) struct Tab<'a> {
    pub immich: &'a mut rustroom_app::immich_worker::ImmichWorker,
    pub test: &'a mut Option<Result<String, String>>,
    pub key_home: rustroom_app::secrets::KeyHome,
    pub thumb_bytes: &'a mut Option<u64>,
    /// The downloaded originals, measured once per opening (M10.15b).
    pub originals: &'a mut Option<rustroom_app::immich_cache::Usage>,
    /// Where this server's originals are cached; `None` with no server URL.
    pub root: Option<std::path::PathBuf>,
    /// For **Clear…**, which opens the Clear dialog in this window's place.
    pub modal: &'a mut crate::Modal,
}

/// The tab's body.
pub(super) fn show(ui: &mut egui::Ui, d: &mut SettingsDraft, t: Tab<'_>) {
    // ⚠⚠ **The master toggle, above everything and outside the grid.** This tab is the
    // one place Immich is named when the integration is off — hiding the tab too would
    // leave no way back on. Everything below is hidden while it is unticked, so the
    // tab holds a switch and nothing else.
    // ⚠ A switch rather than a tick box: enabling or disabling a whole integration
    // is a decision, not one preference among many (user's call, 2026-09-24).
    // ⚠ What Immich is, and what switching it off does, are the switch's hover
    // (`docs/DESIGN.md` §8) — they were a paragraph above it.
    crate::segmented::switch(ui, &mut d.immich_enabled, "Enable Immich")
        .hover("Shows your Immich server's albums in the Library and lets exports upload to it");
    // ⚠ An `if` rather than an early `return`, so anything a later change adds after this block
    // still runs with Immich off.
    if d.immich_enabled {
        ui.add_space(GAP);
        egui::Grid::new("settings_immich")
            .num_columns(2)
            .spacing([12.0, GAP])
            .show(ui, |ui| {
                // ⚠ Editing either field clears the last answer: a stale "Connected"
                // sitting under credentials it was never tested against is worse than no
                // message at all.
                let before = (d.immich_url.clone(), d.immich_key.clone());

                ui.label("Server");
                ui.add(
                    egui::TextEdit::singleline(&mut d.immich_url)
                        .hint_text("https://photos.example.com")
                        .desired_width(260.0),
                )
                .hover("The address of your Immich server");
                ui.end_row();

                ui.label("API key");
                ui.add(
                    egui::TextEdit::singleline(&mut d.immich_key)
                        .password(true)
                        .desired_width(260.0),
                )
                // ⚠ Says where the key is kept — the keychain, or the file where none works
                // (M10.15a); the sentences are `secrets::key_hover`'s, tested there.
                .hover(rustroom_app::secrets::key_hover(
                    rustroom_app::secrets::KeyKind::Browse,
                    t.key_home,
                ));
                ui.end_row();

                // ⚠⚠ Its own field on purpose. Browsing, thumbnails and the rail run
                // constantly and unattended; giving that traffic a write-capable key
                // would make "browsing cannot write" depend on our code being right
                // rather than on the credential. See `Settings::immich_upload_key`.
                // M10.15d: the Write key — upload, and with `asset.update`, Send marks.
                ui.label("Write key");
                ui.add(
                    egui::TextEdit::singleline(&mut d.immich_upload_key)
                        .password(true)
                        .desired_width(260.0),
                )
                .hover(rustroom_app::secrets::key_hover(
                    rustroom_app::secrets::KeyKind::Upload,
                    t.key_home,
                ));
                ui.end_row();

                if before != (d.immich_url.clone(), d.immich_key.clone()) {
                    *t.test = None;
                }

                ui.label("");
                ui.horizontal(|ui| {
                    // ⚠⚠ Tests what is **typed**, not what is saved — the whole point of
                    // a test button is checking a key before committing to it, so the job
                    // carries its own connection rather than reading `app.settings`.
                    if ui
                        .button("Test")
                        .hover("Checks the connection by asking the server for its version")
                        .clicked()
                    {
                        // ⚠ Cleared first, so the previous answer does not sit there
                        // looking like this one while the request is in flight.
                        *t.test = None;
                        let typed = (!d.immich_url.trim().is_empty()
                            && !d.immich_key.trim().is_empty())
                        .then(|| rustroom_app::immich_source::Connection {
                            url: d.immich_url.trim().to_string(),
                            key: d.immich_key.trim().to_string(),
                        });
                        t.immich
                            .request(rustroom_app::immich_worker::Job::About { conn: typed });
                    }
                    // ⚠⚠ **Beside the button, not in the status line** — the status line
                    // is behind the modal, so the answer to a button you just pressed was
                    // invisible. Reported in use.
                    match &*t.test {
                        Some(Ok(v)) => {
                            ui.colored_label(
                                ui.visuals().selection.bg_fill,
                                format!("Connected to Immich {v}"),
                            );
                        }
                        Some(Err(e)) => {
                            ui.colored_label(ui.visuals().error_fg_color, e.as_str())
                                .hover(e.as_str());
                        }
                        None => {}
                    }
                });
                ui.end_row();
            });
        ui.add_space(GAP);
        theme::sub_label(ui, "Thumbnail cache");
        ui.horizontal(|ui| {
            let store = rustroom_app::thumb_store::ThumbStore::immich_default(
                d.immich_thumb_budget_mb as u64 * 1024 * 1024,
            );
            // ⚠ `size_bytes()` walks the directory, so it is computed when the dialog
            // opens and cached — never in a body, which runs sixty times a second.
            let bytes = *t.thumb_bytes.get_or_insert_with(|| store.size_bytes());
            ui.label(format!("{:.1} MB", bytes as f64 / 1024.0 / 1024.0));
            if ui
                .button("Clear")
                .hover("Deletes the cached Immich thumbnails, which are fetched again when needed")
                .clicked()
            {
                store.clear();
                *t.thumb_bytes = Some(0);
            }
        });
        ui.add(
            egui::Slider::new(&mut d.immich_thumb_budget_mb, 512..=51200)
                .step_by(512.0)
                .text("MB budget"),
        )
        .hover("Sets how much disk space Immich thumbnails may use");
        ui.add_space(GAP);
        theme::sub_label(ui, "Downloaded originals");
        // ⚠ Measured once per opening, never in the body's sixty-a-second redraw — the thumbnail
        // row's rule.
        let usage = match &t.root {
            Some(r) => *t
                .originals
                .get_or_insert_with(|| rustroom_app::immich_cache::usage(r)),
            None => rustroom_app::immich_cache::Usage::default(),
        };
        ui.horizontal(|ui| {
            ui.label(rustroom_app::immich_cache::size_line(usage));
            // ⚠ Asks first, unlike the thumbnails' Clear: an original costs a real download to get
            // back.
            if ui
                .add_enabled(usage.photos > 0, egui::Button::new("Clear…"))
                .hover("Asks before deleting the downloaded originals, which download again when opened")
                .clicked()
            {
                if let Some(root) = t.root.clone() {
                    *t.modal = crate::Modal::ImmichCache {
                        startup: false,
                        root,
                        usage,
                    };
                }
            }
        });
        ui.add(
            egui::Slider::new(&mut d.immich_cache_limit_gb, 1..=1000)
                .logarithmic(true)
                .text("GB limit"),
        )
        .hover("Sets the download size past which Rustroom warns at startup");
        ui.checkbox(
            &mut d.immich_cache_warn,
            "Warn at startup when downloads exceed the limit",
        )
        .hover("Offers to clear the downloads at startup once they pass the limit");
        ui.add_space(GAP);
        theme::sub_label(ui, "Merged photos");
        ui.horizontal(|ui| {
            match &d.immich_merge_folder {
                Some(f) => ui.label(f.display().to_string()),
                None => ui.label("Asked at the first merge"),
            }
            .hover(
                "Where HDR and panorama merges of Immich photos are saved, outside the downloads",
            );
            if ui
                .button("Change…")
                .hover("Picks the folder merges of Immich photos are saved in")
                .clicked()
            {
                let picked = rfd::FileDialog::new()
                    .set_title("Choose a folder for photos merged from Immich")
                    .pick_folder();
                // ⚠ Never inside the downloads, which Clear… empties.
                if let Some(f) =
                    picked.filter(|f| !t.root.as_deref().is_some_and(|r| f.starts_with(r)))
                {
                    d.immich_merge_folder = Some(f);
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{test_app, test_ui, Modal};
    use rustroom_app::immich_cache::Usage;

    fn draw(
        ui: &mut egui::Ui,
        app: &mut crate::RustroomApp,
        d: &mut SettingsDraft,
        root: &std::path::Path,
        modal: &mut Modal,
        originals: &mut Option<Usage>,
    ) {
        show(
            ui,
            d,
            Tab {
                immich: &mut app.immich,
                test: &mut app.immich_test,
                key_home: app.settings_store.key_home(),
                thumb_bytes: &mut app.immich_thumb_cache_bytes,
                originals,
                root: Some(root.to_path_buf()),
                modal,
            },
        );
    }

    #[test]
    fn clear_opens_the_dialog_with_what_was_measured() {
        let dir = test_dir::TempDir::new("settings-immich-clear");
        std::fs::write(dir.path().join("a.jpg"), b"jpeg").unwrap();
        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();
        // ⚠ Pre-filled, so the thumbnail row does not size the real Immich thumbnail cache.
        app.immich_thumb_cache_bytes = Some(0);
        let mut d = SettingsDraft::from_settings(&app.settings);
        assert!(
            d.immich_enabled,
            "fixture: the section shows only when Immich is on"
        );
        let (mut modal, mut originals) = (Modal::None, None);
        test_ui::click(&ctx, "Clear…", |ui| {
            draw(ui, &mut app, &mut d, dir.path(), &mut modal, &mut originals)
        });
        assert!(matches!(
            &modal,
            Modal::ImmichCache { startup: false, usage, .. } if usage.photos == 1
        ));
        assert!(
            dir.path().join("a.jpg").exists(),
            "Clear… must ask, not delete"
        );
    }

    #[test]
    fn with_nothing_downloaded_clear_opens_nothing() {
        let dir = test_dir::TempDir::new("settings-immich-empty");
        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();
        app.immich_thumb_cache_bytes = Some(0);
        let mut d = SettingsDraft::from_settings(&app.settings);
        let (mut modal, mut originals) = (Modal::None, None);
        test_ui::click(&ctx, "Clear…", |ui| {
            draw(ui, &mut app, &mut d, dir.path(), &mut modal, &mut originals)
        });
        assert!(matches!(modal, Modal::None));
        assert_eq!(originals, Some(Usage::default()));
    }
}
