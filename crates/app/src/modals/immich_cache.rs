//! Clearing the downloaded Immich originals (M10.15b): one dialog, opened at startup when the
//! downloads exceed the soft limit, or from Settings → Immich — which it returns to.
//!
//! ⛔ Nothing here runs by itself: a clear is always this dialog's primary button.

use super::{dialog, Footer, Modal};
use crate::{theme, RustroomApp};
use eframe::egui;
use rustroom_app::immich_cache;

pub fn show(app: &mut RustroomApp, ctx: &egui::Context) {
    let Modal::ImmichCache {
        startup,
        root,
        usage,
    } = &app.modal
    else {
        return;
    };
    let (startup, root, usage) = (*startup, root.clone(), *usage);
    let title = immich_cache::dialog_title(usage, startup);
    let body = immich_cache::dialog_body(usage, app.settings.immich_cache_limit_gb, startup);
    let extra: &[(&str, &str)] = if startup {
        &[(
            "Don't warn again",
            "Turns this startup warning off, which the Immich settings can turn back on",
        )]
    } else {
        &[]
    };
    let out = dialog(
        ctx,
        &mut app.settings,
        "immich_cache",
        &title,
        theme::DialogSize::Standard,
        Footer::confirm(
            "Clear cache",
            "Deletes the downloaded originals and keeps every edit, mark and thumbnail",
            usage.photos > 0,
            if startup { "Not now" } else { "Cancel" },
        )
        .extra(extra),
        |ui| {
            ui.label(&body);
        },
    );
    if out.primary {
        app.clear_immich_cache(&root);
    }
    if out.extra == Some(0) {
        app.settings.immich_cache_warn = false;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{test_app, test_ui};

    /// A cache root of the test's own with `name` downloaded (4 bytes) and its sidecar.
    fn root_with(tag: &str, name: &str) -> (test_dir::TempDir, std::path::PathBuf) {
        let dir = test_dir::TempDir::new(tag);
        let root = dir.path().to_path_buf();
        std::fs::write(root.join(name), b"jpeg").unwrap();
        std::fs::write(rustroom_app::sidecar::path(&root.join(name)), b"{}").unwrap();
        (dir, root)
    }

    fn settle(ctx: &egui::Context, app: &mut RustroomApp) {
        let _ = test_ui::labelled_centres(ctx, Vec::new(), |_| show(app, ctx));
    }

    #[test]
    fn clear_cache_removes_every_original_and_keeps_every_sidecar() {
        let (_d, root) = root_with("clear-all", "a.jpg");
        std::fs::write(root.join("b.RAF"), b"raw").unwrap();
        let mut app = test_app::for_test();
        app.immich_originals = Some(rustroom_app::immich_cache::Usage {
            bytes: 7,
            photos: 2,
        });
        app.clear_immich_cache(&root);
        assert!(!root.join("a.jpg").exists() && !root.join("b.RAF").exists());
        assert_eq!(
            app.immich_originals, None,
            "Settings would show the old size"
        );
        assert!(
            rustroom_app::sidecar::path(&root.join("a.jpg")).exists(),
            "the edits went too"
        );
    }

    #[test]
    fn clearing_the_open_photo_empties_develop_and_keeps_its_edits() {
        let (_d, root) = root_with("clear-open", "a.jpg");
        let car = rustroom_app::sidecar::path(&root.join("a.jpg"));
        std::fs::remove_file(&car).unwrap();
        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();
        test_app::open_test_photo(&mut app, &ctx, 200, 100);
        let s = app.session.as_mut().unwrap();
        s.path = root.join("a.jpg");
        s.edits.exposure = 1.0;
        app.view = crate::AppView::Develop;
        app.clear_immich_cache(&root);
        assert!(app.session.is_none(), "Develop still holds a removed photo");
        assert!(matches!(app.view, crate::AppView::Library));
        assert!(car.exists(), "the open photo's edits were not saved");
    }

    #[test]
    fn startup_opens_the_dialog_only_when_connected_warned_and_over() {
        let (_d, root) = root_with("startup", "a.jpg");
        let mut app = test_app::for_test();
        app.settings.immich_cache_limit_gb = 0; // 4 bytes is over a 0 GB limit
        app.check_immich_cache_in(false, root.clone());
        assert!(matches!(app.modal, Modal::None), "Immich is not connected");
        app.settings.immich_cache_warn = false;
        app.check_immich_cache_in(true, root.clone());
        assert!(matches!(app.modal, Modal::None), "the warning is off");
        app.settings.immich_cache_warn = true;
        app.settings.immich_cache_limit_gb = 1;
        app.check_immich_cache_in(true, root.clone());
        assert!(matches!(app.modal, Modal::None), "under the limit");
        app.settings.immich_cache_limit_gb = 0;
        app.check_immich_cache_in(true, root.clone());
        assert!(matches!(
            &app.modal,
            Modal::ImmichCache { startup: true, usage, .. } if usage.photos == 1
        ));
    }

    fn at_startup(root: &std::path::Path) -> (egui::Context, RustroomApp) {
        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();
        app.modal = Modal::ImmichCache {
            startup: true,
            root: root.to_path_buf(),
            usage: rustroom_app::immich_cache::usage(root),
        };
        settle(&ctx, &mut app);
        (ctx, app)
    }

    #[test]
    fn the_dialogs_clear_cache_deletes() {
        let (_d, root) = root_with("dialog-clear", "a.jpg");
        let (ctx, mut app) = at_startup(&root);
        test_ui::click(&ctx, "Clear cache", |_| show(&mut app, &ctx));
        assert!(!root.join("a.jpg").exists());
        assert!(matches!(app.modal, Modal::None));
    }

    #[test]
    fn not_now_deletes_nothing() {
        let (_d, root) = root_with("dialog-not-now", "a.jpg");
        let (ctx, mut app) = at_startup(&root);
        test_ui::click(&ctx, "Not now", |_| show(&mut app, &ctx));
        assert!(root.join("a.jpg").exists());
        assert!(matches!(app.modal, Modal::None));
        assert!(app.settings.immich_cache_warn);
    }

    #[test]
    fn dont_warn_again_unticks_and_saves_the_warning() {
        let (_d, root) = root_with("dialog-dont-warn", "a.jpg");
        let (ctx, mut app) = at_startup(&root);
        test_ui::click(&ctx, "Don't warn again", |_| show(&mut app, &ctx));
        assert!(!app.settings.immich_cache_warn);
        assert!(
            !app.settings_store.load().immich_cache_warn,
            "not saved — it returns next launch"
        );
        assert!(
            root.join("a.jpg").exists(),
            "Don't warn again cleared the cache"
        );
        assert!(matches!(app.modal, Modal::None));
    }

    #[test]
    fn asked_from_settings_closing_returns_to_settings_with_its_draft() {
        let (_d, root) = root_with("dialog-settings", "a.jpg");
        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();
        app.modal = Modal::Settings;
        let _ = test_ui::labelled_centres(&ctx, Vec::new(), |_| {
            super::super::settings::show(&mut app, &ctx)
        });
        assert!(
            app.settings_draft.is_some(),
            "fixture: Settings keeps a draft"
        );
        app.modal = Modal::ImmichCache {
            startup: false,
            root: root.clone(),
            usage: rustroom_app::immich_cache::usage(&root),
        };
        settle(&ctx, &mut app);
        test_ui::click(&ctx, "Cancel", |_| show(&mut app, &ctx));
        assert!(matches!(app.modal, Modal::Settings));
        assert!(app.settings_draft.is_some(), "the Settings draft was lost");
    }
}
