//! Opening an Immich album, and draining the worker once a frame.
//!
//! ⚠ A separate bin module rather than more of [`crate::sources`], which is 571 lines and whose
//! subject is the local sources. Everything Immich-specific about *showing* a library lives here;
//! the reusable logic is in [`rustroom_app::immich_source`] and the worker in
//! [`rustroom_app::immich_worker`], neither of which knows about egui.

use crate::{AppView, LibrarySource, RustroomApp};
use eframe::egui;
use rustroom_app::immich_source;
use rustroom_app::immich_worker::{Job, Outcome};
use rustroom_app::photo_id::PhotoId;

impl RustroomApp {
    /// Show one Immich album's photos, downloading nothing.
    ///
    /// ⚠⚠ Goes through `open_scan`, the one place a `Library` is constructed, exactly like a
    /// folder — which is the entire point of shape A. The rows name cache paths that mostly do not
    /// exist yet, and `open_scan`'s own callees have to tolerate that.
    pub(crate) fn open_immich_album(
        &mut self,
        ctx: &egui::Context,
        id: String,
        name: String,
        assets: &[immich::Asset],
    ) -> usize {
        let Some(conn) = immich_source::connection(&self.settings) else {
            self.status = "No Immich server is configured — set one in Settings".to_string();
            return 0;
        };
        let root = immich_source::cache_root(&conn.url);
        let scan = immich_source::entries_from_album(assets, &root);
        let n = self.open_scan(
            ctx,
            LibrarySource::ImmichAlbum {
                id,
                name: name.clone(),
            },
            scan,
        );
        // ⚠ Said after `open_scan`, which sets its own status for a local scan.
        self.status = if n == assets.len() {
            format!("{n} photos in “{name}”")
        } else {
            // ⚠ The difference is worth saying out loud: videos and RAW formats this build cannot
            // decode are skipped, and a silently shorter grid reads as photos having gone missing.
            format!(
                "{n} of {} photos in “{name}” — the rest are videos or formats Rustroom cannot open",
                assets.len()
            )
        };
        n
    }

    /// Drain the Immich worker. Called once a frame from [`crate::shell`].
    pub(crate) fn pump_immich(&mut self, ctx: &egui::Context) {
        for outcome in self.immich.poll() {
            match outcome {
                Outcome::Albums(result) => self.immich_albums = Some(result),
                Outcome::Album { id, name, result } => match result {
                    Ok(assets) => {
                        self.open_immich_album(ctx, id, name, &assets);
                    }
                    Err(e) => self.status = format!("Could not open “{name}”: {e}"),
                },
                Outcome::About(Ok(v)) => self.status = format!("Connected to Immich {v}"),
                Outcome::About(Err(e)) => self.status = format!("Immich: {e}"),
                Outcome::Original { dest, result } => match result {
                    // ⚠⚠ The file is now where the `PhotoId` always said it was, so the ordinary
                    // open path applies with no special case — and the view switch mirrors
                    // `open_entry`, which is the shape that already exists.
                    Ok(()) => {
                        self.open_path(ctx, PhotoId::original(dest));
                        self.view = AppView::Develop;
                    }
                    Err(e) => self.status = format!("Download failed: {e}"),
                },
            }
        }
    }

    /// Ask the server for its albums, unless a list request is already in flight.
    pub(crate) fn request_immich_albums(&mut self) {
        self.immich.request(Job::Albums);
    }
}
