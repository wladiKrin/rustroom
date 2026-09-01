//! Applying a settings change to the running app.
//!
//! Split out of `main.rs` unchanged. ⚠ Every function here is called from a dialog body,
//! which runs **every frame it is open** — so each one is guarded on an actual change by
//! its caller. Writing the settings file per frame is the `apply_view_demosaic` mistake.

use crate::decode::{decode_for_edit, full_res_worker};
use crate::theme;
use crate::{RustroomApp, MAX_CONCURRENT_DECODES};
use eframe::egui;
use rustroom_app::decode_worker::DecodeWorker;
use rustroom_app::photo_id::PhotoId;
use rustroom_app::settings::{Accent, ThemeChoice};
use rustroom_app::thumb_loader::ThumbLoader;

impl RustroomApp {
    /// Persist the settings, surfacing a failure in the status line rather than silently.
    pub(crate) fn save_settings(&mut self) {
        if let Err(e) = self.settings_store.save(&self.settings) {
            self.status = format!("Could not save settings: {e}");
        }
    }

    /// The Settings window. One setting today; a modal is the right weight for that.
    /// Adopt new appearance settings: persist them and re-style immediately, so the change is
    /// visible rather than deferred to the next launch.
    ///
    /// ⚠ Cheap enough to do inline — unlike `apply_view_demosaic`, nothing is decoded. It is
    /// still guarded by a change comparison at the call site, because the Settings dialog body
    /// runs every frame it is open.
    pub(crate) fn apply_appearance(
        &mut self,
        theme_choice: ThemeChoice,
        accent: Accent,
        ctx: &egui::Context,
    ) {
        self.settings.theme = theme_choice;
        self.settings.accent = accent;
        self.save_settings();
        theme::apply(ctx, &self.settings);
    }

    /// Adopt a new display space: persist it, reload the thumbnails so the filmstrip agrees
    /// with the canvas, and re-render the open photo so the change is visible immediately.
    ///
    /// ⚠ Thumbnails are converted **at load**, so the loader has to be rebuilt and the cached
    /// thumbnails dropped. They reload in parallel and cost about 41 ms each, which is why this
    /// is affordable to do eagerly rather than deferring to the next folder open.
    pub(crate) fn apply_display_space(&mut self, space: color::OutputSpace, ctx: &egui::Context) {
        self.settings.display_space = space;
        self.save_settings();
        // ⚠ The *effective* value from here on: if the OS is managing colour this is `Srgb`
        // whatever the dropdown said, so the two can never both convert.
        let space = self
            .settings
            .effective_display_space(self.os_manages_colour);
        if let Some(lib) = self.library.as_mut() {
            let ctx2 = ctx.clone();
            lib.loader = ThumbLoader::new(space, move || ctx2.request_repaint());
            // ⚠ Every cached thumbnail was converted at load, so they all describe the old space.
            lib.cache.clear();
            // ⚠ And the want-list, or `sync_thumb_window` compares against a list that is now
            // satisfied by nothing and never re-asks.
            lib.last_want.clear();
        }
        // The canvas is a plain re-render: nothing is decoded. `Session::request` reads the
        // new value on its way through, so it has to be written on the session first.
        let tool = self.tool;
        if let Some(s) = self.session.as_mut() {
            s.display_space = space;
            let edits = s.view_edits(tool);
            let (max_dim, region) = (s.render_max_dim(), s.requested_region);
            s.request(edits, max_dim, region);
        }
    }

    /// The profile chosen for the open photo's camera, if any.
    ///
    /// ⚠ Keyed by `Session::make_model` — libraw's make and model joined, e.g. `"Fujifilm X-T4"`.
    /// That is deliberately the same shape a profile's own `ProfileName` has (`"FUJIFILM X-T4"`),
    /// so matching compares like with like and the make disambiguates two bodies that share a
    /// model string.
    pub(crate) fn camera_profile_path(&self) -> Option<std::path::PathBuf> {
        let s = self.session.as_ref()?;
        self.settings.camera_profiles.get(&s.make_model).cloned()
    }

    /// Adopt a camera profile for the open photo's camera.
    ///
    /// Mirrors `apply_view_demosaic`: persist, rebuild both workers, and re-decode.
    ///
    /// ⚠ Clearing `full_pending` is load-bearing for the same reason it is there — the sequence
    /// number belongs to the worker being dropped, and leaving it blocks `maybe_request_full_res`
    /// forever.
    ///
    /// ⚠ **The buffer cache must be cleared too.** Without that, arrowing back to a photo serves
    /// the buffer decoded with the *old* profile, which looks exactly like the setting not
    /// working.
    pub(crate) fn apply_camera_profile(
        &mut self,
        make_model: String,
        profile: Option<std::path::PathBuf>,
        ctx: &egui::Context,
    ) {
        match &profile {
            Some(p) => self.settings.camera_profiles.insert(make_model, p.clone()),
            None => self.settings.camera_profiles.remove(&make_model),
        };
        self.save_settings();

        let ctx2 = ctx.clone();
        let for_edit = profile.clone();
        self.decoder = DecodeWorker::new(
            move |p| decode_for_edit(p, for_edit.clone()),
            MAX_CONCURRENT_DECODES,
            move || ctx2.request_repaint(),
        );
        self.full_decoder = full_res_worker(self.settings.view_demosaic, profile, ctx);
        self.full_pending = None;
        self.cache.clear();

        if let Some(s) = self.session.as_ref() {
            let id = PhotoId::new(s.path.clone(), s.copy);
            self.open_path(ctx, id);
        }
    }

    /// Adopt a new 1:1 view demosaic tier: persist it, rebuild the worker, and re-decode the
    /// open photo so the change is visible rather than silently deferred to the next photo.
    ///
    /// ⚠ Clearing `full_pending` is load-bearing: that sequence number belongs to the worker
    /// being dropped, and leaving it would make `pump_full_res` compare against a number no
    /// live worker will ever produce — blocking `maybe_request_full_res`, which refuses while
    /// a decode is in flight.
    pub(crate) fn apply_view_demosaic(&mut self, tier: raw_decode::Demosaic, ctx: &egui::Context) {
        self.settings.view_demosaic = tier;
        self.save_settings();
        self.full_decoder = full_res_worker(tier, self.camera_profile_path(), ctx);
        self.full_pending = None;
        // RAW only. For JPEG/PNG/TIFF the buffer really is the whole file, so clearing this
        // would be a lie — and `maybe_request_full_res` refuses on its own `is_raw` check
        // anyway. The old buffer stays on screen until the new one lands, so nothing flickers.
        if let Some(s) = self.session.as_mut() {
            if catalog::is_raw(&s.path) {
                s.buffer_is_full_res = false;
            }
        }
    }
}
