//! Settings → Performance's AI denoise block (M10.16b): the results' total with **Clear…**, the soft limit
//! and its startup warning — Settings → Immich's downloaded-originals block, again.
//!
//! ⚠ Its own module: `modals/settings.rs` stands near the 1,000-line ceiling (M10.15b's precedent).

use super::settings::SettingsDraft;
use crate::theme;
use crate::theme::Hover as _;
use eframe::egui;
use rustroom_app::denoise_cache::{self, Usage};

/// What the block reads and writes besides the draft.
pub(super) struct Block<'a> {
    /// Where the results are; `None` with no cache folder.
    pub dir: Option<std::path::PathBuf>,
    /// The measured total — once per opening of Settings, never per frame.
    pub usage: &'a mut Option<Usage>,
    /// For **Clear…**, which opens the Clear dialog in this window's place.
    pub modal: &'a mut crate::Modal,
}

pub(super) fn show(ui: &mut egui::Ui, d: &mut SettingsDraft, b: Block<'_>) {
    theme::sub_label(ui, "AI denoise results");
    let usage = match &b.dir {
        Some(dir) => *b.usage.get_or_insert_with(|| denoise_cache::usage(dir)),
        None => Usage::default(),
    };
    ui.horizontal(|ui| {
        ui.label(denoise_cache::size_line(usage));
        // ⚠ Asks first: a result costs minutes of network to get back.
        if ui
            .add_enabled(usage.photos > 0, egui::Button::new("Clear…"))
            .hover("Asks before deleting the denoised results, which recompute when a photo opens")
            .clicked()
        {
            if let Some(dir) = b.dir.clone() {
                *b.modal = crate::Modal::DenoiseCache {
                    startup: false,
                    dir,
                    usage,
                };
            }
        }
    });
    ui.add(
        egui::Slider::new(&mut d.denoise_cache_limit_gb, 1..=1000)
            .logarithmic(true)
            .text("GB limit"),
    )
    .hover("Sets the results' size past which Rustroom warns at startup");
    ui.checkbox(
        &mut d.denoise_cache_warn,
        "Warn at startup when results exceed the limit",
    )
    .hover("Offers to clear the denoised results at startup once they pass the limit");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{test_app, test_ui, Modal};

    fn draw(
        ui: &mut egui::Ui,
        d: &mut SettingsDraft,
        dir: &std::path::Path,
        modal: &mut Modal,
        usage: &mut Option<Usage>,
    ) {
        show(
            ui,
            d,
            Block {
                dir: Some(dir.to_path_buf()),
                usage,
                modal,
            },
        );
    }

    #[test]
    fn clear_opens_the_dialog_and_deletes_nothing() {
        let dir = test_dir::TempDir::new("settings-denoise-clear");
        let img = core_image::ImageBuf::from_vec(4, 2, vec![0.5; 4 * 2 * 3]);
        denoise_cache::store(dir.path(), "k", &img).unwrap();
        let ctx = test_ui::ctx();
        let app = test_app::for_test();
        let mut d = SettingsDraft::from_settings(&app.settings);
        let (mut modal, mut usage) = (Modal::None, None);
        test_ui::click(&ctx, "Clear…", |ui| {
            draw(ui, &mut d, dir.path(), &mut modal, &mut usage)
        });
        assert!(matches!(
            &modal,
            Modal::DenoiseCache { startup: false, usage, .. } if usage.photos == 1
        ));
        assert_eq!(
            denoise_cache::usage(dir.path()).photos,
            1,
            "Clear… must ask"
        );
    }

    #[test]
    fn with_no_results_clear_opens_nothing() {
        let dir = test_dir::TempDir::new("settings-denoise-empty");
        let ctx = test_ui::ctx();
        let app = test_app::for_test();
        let mut d = SettingsDraft::from_settings(&app.settings);
        let (mut modal, mut usage) = (Modal::None, None);
        test_ui::click(&ctx, "Clear…", |ui| {
            draw(ui, &mut d, dir.path(), &mut modal, &mut usage)
        });
        assert!(matches!(modal, Modal::None));
        assert_eq!(usage, Some(Usage::default()));
    }
}
