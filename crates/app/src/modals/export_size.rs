//! The export dialog's Size section (M10.10b): Lightroom's *Resize to fit* in px, cm or in, a
//! resolution, and Don't enlarge.
//!
//! ⚠ Its own file because `modals/export.rs` held 787 lines of code when this arrived — the
//! watermark section's precedent.

use crate::theme;
use crate::theme::Hover as _;
use eframe::egui;
use rustroom_app::batch_export::{ExportOptions, FitMode, LengthUnit, OutputSize};
use rustroom_app::export_summary;

/// The section's body.
pub(super) fn body(ui: &mut egui::Ui, opts: &mut ExportOptions) {
    let s = &mut opts.size;
    egui::Grid::new("export_size_grid")
        .num_columns(2)
        .spacing([12.0, 4.0])
        .show(ui, |ui| {
            ui.label("Resize to fit");
            egui::ComboBox::from_id_salt("size_fit")
                .selected_text(s.fit.label())
                .show_ui(ui, |ui| {
                    for f in FitMode::ALL {
                        ui.selectable_value(&mut s.fit, f, f.label());
                    }
                })
                .response
                .hover("Chooses what sets the size of each exported photo");
            ui.end_row();
            let unit = s.unit;
            match s.fit {
                FitMode::Original => {}
                FitMode::WidthHeight | FitMode::Dimensions => {
                    ui.label("Width");
                    length(ui, &mut s.width, unit);
                    ui.end_row();
                    ui.label("Height");
                    length(ui, &mut s.height, unit);
                    ui.end_row();
                }
                FitMode::LongEdge => {
                    ui.label("Long edge");
                    length(ui, &mut s.long_edge, unit);
                    ui.end_row();
                }
                FitMode::ShortEdge => {
                    ui.label("Short edge");
                    length(ui, &mut s.short_edge, unit);
                    ui.end_row();
                }
                FitMode::Megapixels => {
                    ui.label("Megapixels");
                    ui.add(
                        egui::DragValue::new(&mut s.megapixels)
                            .range(0.1..=500.0)
                            .speed(0.1)
                            .max_decimals(1)
                            .suffix(" MP"),
                    )
                    .hover("Sets how many million pixels each photo has");
                    ui.end_row();
                }
                FitMode::Percentage => {
                    ui.label("Percentage");
                    ui.add(
                        egui::DragValue::new(&mut s.percent)
                            .range(1.0..=400.0)
                            .speed(1.0)
                            .max_decimals(0)
                            .suffix(" %"),
                    )
                    .hover("Sets each photo's size as a share of its own");
                    ui.end_row();
                }
            }
            if s.fit.uses_length() {
                ui.label("Unit");
                unit_menu(ui, s);
                ui.end_row();
            }
            ui.label("Resolution");
            let (lo, hi) = OutputSize::PPI_RANGE;
            ui.add(
                egui::DragValue::new(&mut s.ppi)
                    .range(lo..=hi)
                    .suffix(" PPI"),
            )
            .hover("Sets the pixels per inch each file says it prints at");
            ui.end_row();
        });
    if s.fit != FitMode::Original {
        ui.checkbox(&mut s.dont_enlarge, "Don't enlarge")
            .hover("Keeps a photo at its own pixels when the size asks for more");
    }
    if let Some(note) = export_summary::size_note(opts) {
        theme::hint(ui, &note);
    }
}

/// One length's box, ranged and stepped for its unit.
fn length(ui: &mut egui::Ui, v: &mut f32, unit: LengthUnit) {
    let (range, speed, decimals) = match unit {
        LengthUnit::Px => (1.0..=65_000.0, 1.0, 0),
        LengthUnit::Cm => (0.1..=500.0, 0.1, 2),
        LengthUnit::In => (0.1..=200.0, 0.05, 2),
    };
    ui.add(
        egui::DragValue::new(v)
            .range(range)
            .speed(speed)
            .max_decimals(decimals)
            .suffix(format!(" {}", unit.label())),
    )
    .hover("Sets the size in the unit chosen below");
}

/// The unit menu. ⚠ A change converts the lengths through the PPI, so the size stays the same.
fn unit_menu(ui: &mut egui::Ui, s: &mut OutputSize) {
    let mut unit = s.unit;
    egui::ComboBox::from_id_salt("size_unit")
        .selected_text(unit.label())
        .show_ui(ui, |ui| {
            for u in LengthUnit::ALL {
                ui.selectable_value(&mut unit, u, u.label());
            }
        })
        .response
        .hover("Chooses pixels, centimetres or inches");
    if unit != s.unit {
        s.set_unit(unit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_ui;

    fn numbers(ctx: &egui::Context, o: &mut ExportOptions) -> usize {
        test_ui::numeric_values(ctx, |ui| body(ui, o)).len()
    }

    fn labels(ctx: &egui::Context, o: &mut ExportOptions) -> Vec<String> {
        test_ui::nodes(ctx, |ui| body(ui, o))
            .into_iter()
            .map(|n| n.label)
            .collect()
    }

    #[test]
    fn each_mode_shows_its_own_boxes() {
        let ctx = test_ui::ctx();
        let mut o = ExportOptions::default();
        assert_eq!(numbers(&ctx, &mut o), 1, "Original: the resolution alone");
        assert!(!labels(&ctx, &mut o).iter().any(|l| l == "Don't enlarge"));
        for (fit, n) in [
            (FitMode::LongEdge, 2),
            (FitMode::ShortEdge, 2),
            (FitMode::WidthHeight, 3),
            (FitMode::Dimensions, 3),
            (FitMode::Megapixels, 2),
            (FitMode::Percentage, 2),
        ] {
            o.size.fit = fit;
            assert_eq!(numbers(&ctx, &mut o), n, "{fit:?}");
            let drawn = labels(&ctx, &mut o);
            assert!(
                drawn.iter().any(|l| l == "Resolution"),
                "{fit:?}: {drawn:?}"
            );
            assert!(drawn.iter().any(|l| l == "Don't enlarge"), "{fit:?}");
            assert_eq!(
                drawn.iter().any(|l| l == "Unit"),
                fit.uses_length(),
                "{fit:?}: a unit only for lengths"
            );
        }
        o.size.fit = FitMode::WidthHeight;
        let drawn = labels(&ctx, &mut o);
        assert!(drawn.iter().any(|l| l == "Width") && drawn.iter().any(|l| l == "Height"));
    }
}
