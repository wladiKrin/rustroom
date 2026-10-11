//! The export dialog's Watermark section: Text · Image, sharing the corner and the opacity
//! (M10.10a).
//!
//! ⚠ Split out of `modals/export.rs`, which held ~796 lines of code when the image half arrived.
//! The picker runs in the dialog, after it has drawn — `body` only asks for it, the destination
//! folder's pattern.

use crate::theme::Hover as _;
use eframe::egui;
use rustroom_app::batch_export::{ExportOptions, Watermark, WatermarkCorner, WatermarkKind};

/// The text mark's slider.
const SIZE: &str = "Size (% of long edge)";
/// The image mark's slider.
const WIDTH: &str = "Width (% of long edge)";

/// The section's body. Sets `pick` when *Choose image…* is clicked.
pub(super) fn body(ui: &mut egui::Ui, opts: &mut ExportOptions, pick: &mut bool) {
    let mark = &mut opts.watermark;
    egui::Grid::new("export_watermark_grid")
        .num_columns(2)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            ui.label("Style");
            ui.horizontal(|ui| {
                for k in WatermarkKind::ALL {
                    let hover = match k {
                        WatermarkKind::Text => "Lays a line of text over each export",
                        WatermarkKind::Image => "Lays a picture, such as a logo, over each export",
                    };
                    ui.selectable_value(&mut mark.kind, k, k.label())
                        .hover(hover);
                }
            });
            ui.end_row();
            match mark.kind {
                WatermarkKind::Text => {
                    ui.label("Text");
                    ui.text_edit_singleline(&mut mark.text)
                        .hover("Sets the watermark text, which scales with the exported image");
                }
                WatermarkKind::Image => {
                    ui.label("File");
                    ui.horizontal(|ui| {
                        if ui
                            .button("Choose image…")
                            .hover("Chooses a picture to lay over each export, kept as a copy")
                            .clicked()
                        {
                            *pick = true;
                        }
                        let name = mark.image.trim();
                        ui.label(if name.is_empty() {
                            "(none chosen)"
                        } else {
                            name
                        });
                    });
                }
            }
            ui.end_row();
            ui.label("Corner");
            egui::ComboBox::from_id_salt("watermark_corner")
                .selected_text(mark.corner.label())
                .show_ui(ui, |ui| {
                    for c in WatermarkCorner::ALL {
                        ui.selectable_value(&mut mark.corner, c, c.label());
                    }
                });
            ui.end_row();
        });
    match mark.kind {
        WatermarkKind::Text => {
            let (lo, hi) = Watermark::SIZE_RANGE;
            ui.add(egui::Slider::new(&mut mark.size, lo..=hi).text(SIZE));
        }
        WatermarkKind::Image => {
            let (lo, hi) = Watermark::IMAGE_WIDTH_RANGE;
            ui.add(egui::Slider::new(&mut mark.image_width, lo..=hi).text(WIDTH));
        }
    }
    ui.add(egui::Slider::new(&mut mark.opacity, 0..=100).text("Opacity"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_ui;
    use rustroom_app::batch_export::WatermarkKind;

    fn sliders(ctx: &egui::Context, opts: &mut ExportOptions) -> Vec<String> {
        let mut pick = false;
        test_ui::slider_labels(ctx, |ui| body(ui, opts, &mut pick))
    }

    #[test]
    fn text_shows_its_size_and_image_its_width() {
        let ctx = test_ui::ctx();
        let mut opts = ExportOptions::default();
        let drawn = sliders(&ctx, &mut opts);
        assert!(drawn.iter().any(|l| l == SIZE), "{drawn:?}");
        assert!(!drawn.iter().any(|l| l == WIDTH), "{drawn:?}");
        assert!(drawn.iter().any(|l| l == "Opacity"), "{drawn:?}");

        let mut pick = false;
        test_ui::click(&ctx, "Image", |ui| body(ui, &mut opts, &mut pick));
        assert_eq!(opts.watermark.kind, WatermarkKind::Image);
        let drawn = sliders(&ctx, &mut opts);
        assert!(drawn.iter().any(|l| l == WIDTH), "{drawn:?}");
        assert!(!drawn.iter().any(|l| l == SIZE), "{drawn:?}");
        assert!(drawn.iter().any(|l| l == "Opacity"), "shared: {drawn:?}");
        assert!(!pick, "switching the kind opens no picker");
    }

    #[test]
    fn choose_image_asks_for_the_picker_and_the_choice_is_named() {
        let ctx = test_ui::ctx();
        let mut opts = ExportOptions::default();
        opts.watermark.kind = WatermarkKind::Image;
        opts.watermark.image = "logo.png".into();
        let mut pick = false;
        let nodes = test_ui::nodes(&ctx, |ui| body(ui, &mut opts, &mut pick));
        assert!(
            nodes.iter().any(|n| n.label == "logo.png"),
            "the chosen name"
        );
        let button = nodes
            .iter()
            .find(|n| n.label == "Choose image…")
            .expect("the button");
        assert!(button.description.is_some(), "the button says what it does");
        test_ui::click(&ctx, "Choose image…", |ui| body(ui, &mut opts, &mut pick));
        assert!(pick, "the click asks for the picker");
    }
}
