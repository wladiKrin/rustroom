//! The new-version check's two ends in the app (piece 4): started at launch, its answer kept in the settings.
//! Every decision is `rustroom_app::update_check`'s.

use crate::RustroomApp;
use eframe::egui;
use rustroom_app::update_check;

/// Unix seconds now; a clock before 1970 reads 0, which is "due".
fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl RustroomApp {
    /// At launch: starts a check if this copy may check and one is due.
    pub(crate) fn start_update_check(&mut self, ctx: &egui::Context) {
        let ctx = ctx.clone();
        self.update_checker = update_check::start_if_due(
            &self.settings,
            now(),
            update_check::live_fetch,
            Box::new(move || ctx.request_repaint()),
        );
    }

    /// Each frame: once the check answers, keep what it found and drop it.
    pub(crate) fn pump_update_check(&mut self) {
        let Some(outcome) = self.update_checker.as_ref().and_then(|c| c.poll()) else {
            return;
        };
        self.update_checker = None;
        update_check::record(&mut self.settings, &outcome, now());
        self.save_settings();
    }
}
