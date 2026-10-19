//! Clearing AI denoise's results (M10.16b): one dialog, opened at startup when the results exceed the
//! soft limit, or from Settings → Performance — which it returns to. M10.15b's Immich dialog, again.
//!
//! ⛔ Nothing here runs by itself: a clear is always this dialog's primary button.

use super::{dialog, Footer, Modal};
use crate::{theme, RustroomApp};
use eframe::egui;
use rustroom_app::denoise_cache;

pub fn show(app: &mut RustroomApp, ctx: &egui::Context) {
    let Modal::DenoiseCache {
        startup,
        dir,
        usage,
    } = &app.modal
    else {
        return;
    };
    let (startup, dir, usage) = (*startup, dir.clone(), *usage);
    let title = denoise_cache::dialog_title(usage, startup);
    let body = denoise_cache::dialog_body(usage, app.settings.denoise_cache_limit_gb, startup);
    let extra: &[(&str, &str)] = if startup {
        &[(
            "Don't warn again",
            "Turns this startup warning off, which the Performance settings can turn back on",
        )]
    } else {
        &[]
    };
    let out = dialog(
        ctx,
        &mut app.settings,
        "denoise_cache",
        &title,
        theme::DialogSize::Standard,
        Footer::confirm(
            "Clear results",
            "Deletes the denoised results and keeps every edit, which recomputes when opened",
            usage.photos > 0,
            if startup { "Not now" } else { "Cancel" },
        )
        .extra(extra),
        |ui| {
            ui.label(&body);
        },
    );
    if out.primary {
        app.clear_denoise_cache(&dir);
    }
    if out.extra == Some(0) {
        app.settings.denoise_cache_warn = false;
        // ⚠ Saved here: at startup no Settings window is behind this to save it on Apply.
        app.save_settings();
    }
    if out.primary || out.dismissed || out.extra.is_some() {
        app.modal = if startup {
            Modal::None
        } else {
            Modal::Settings
        };
    }
}

impl RustroomApp {
    /// Delete every denoised result in `dir` — the dialog's primary button and nothing else.
    pub(crate) fn clear_denoise_cache(&mut self, dir: &std::path::Path) {
        denoise_cache::remove_all(dir);
        self.denoise.usage = None;
    }

    /// Once, after bootstrap: offer to clear when the warning is on and the results are over the limit.
    pub(crate) fn check_denoise_cache_at_startup(&mut self) {
        if let Some(dir) = denoise_cache::dir() {
            self.check_denoise_cache_in(dir);
        }
    }

    /// [`Self::check_denoise_cache_at_startup`] with the folder given — what the tests drive.
    pub(crate) fn check_denoise_cache_in(&mut self, dir: std::path::PathBuf) {
        // ⚠ Not even measured with the warning off — a startup's `read_dir` it has no use for.
        if !self.settings.denoise_cache_warn {
            return;
        }
        let usage = denoise_cache::usage(&dir);
        if denoise_cache::warn_at_startup(true, self.settings.denoise_cache_limit_gb, usage) {
            self.modal = Modal::DenoiseCache {
                startup: true,
                dir,
                usage,
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{test_app, test_ui};

    /// A results folder of the test's own holding one result.
    fn dir_with_one(tag: &str) -> test_dir::TempDir {
        let d = test_dir::TempDir::new(tag);
        let img = core_image::ImageBuf::from_vec(4, 2, vec![0.5; 4 * 2 * 3]);
        denoise_cache::store(d.path(), "k", &img).unwrap();
        d
    }

    fn settle(ctx: &egui::Context, app: &mut RustroomApp) {
        let _ = test_ui::labelled_centres(ctx, Vec::new(), |_| show(app, ctx));
    }

    #[test]
    fn clear_results_empties_the_folder_and_returns_to_settings() {
        let d = dir_with_one("denoise-clear");
        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();
        let usage = denoise_cache::usage(d.path());
        assert_eq!(usage.photos, 1, "the fixture");
        app.denoise.usage = Some(usage);
        app.modal = Modal::DenoiseCache {
            startup: false,
            dir: d.path().to_path_buf(),
            usage,
        };
        settle(&ctx, &mut app);
        test_ui::click(&ctx, "Clear results", |_| show(&mut app, &ctx));
        assert_eq!(denoise_cache::usage(d.path()).photos, 0, "still there");
        assert_eq!(app.denoise.usage, None, "Settings would show the old size");
        assert!(matches!(app.modal, Modal::Settings));
    }

    #[test]
    fn startup_warns_only_when_on_and_over() {
        let d = dir_with_one("denoise-startup");
        let mut app = test_app::for_test();
        app.settings.denoise_cache_limit_gb = 0; // a few bytes are over a 0 GB limit
        app.settings.denoise_cache_warn = false;
        app.check_denoise_cache_in(d.path().to_path_buf());
        assert!(matches!(app.modal, Modal::None), "the warning is off");
        app.settings.denoise_cache_warn = true;
        app.settings.denoise_cache_limit_gb = 1;
        app.check_denoise_cache_in(d.path().to_path_buf());
        assert!(matches!(app.modal, Modal::None), "under the limit");
        app.settings.denoise_cache_limit_gb = 0;
        app.check_denoise_cache_in(d.path().to_path_buf());
        assert!(matches!(
            &app.modal,
            Modal::DenoiseCache { startup: true, usage, .. } if usage.photos == 1
        ));
    }

    #[test]
    fn dont_warn_again_turns_the_warning_off_and_keeps_the_results() {
        let d = dir_with_one("denoise-dont-warn");
        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();
        app.settings.denoise_cache_limit_gb = 0;
        app.check_denoise_cache_in(d.path().to_path_buf());
        settle(&ctx, &mut app);
        test_ui::click(&ctx, "Don't warn again", |_| show(&mut app, &ctx));
        assert!(!app.settings.denoise_cache_warn);
        assert!(!app.settings_store.load().denoise_cache_warn, "not saved");
        assert_eq!(denoise_cache::usage(d.path()).photos, 1, "cleared");
        assert!(matches!(app.modal, Modal::None));
    }
}
