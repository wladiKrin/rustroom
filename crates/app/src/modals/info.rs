//! Everything Rustroom knows about one photo.
//!
//! ⚠ The formatting lives in pure functions below so it can be tested; the dialog itself is glue.

use crate::modals::{dialog, Modal};
use crate::RustroomApp;
use eframe::egui;

/// A shutter speed as a photographer writes it.
///
/// ⚠ The stored value is `0.0199999995529652`; showing that is technically honest and practically
/// useless.
pub fn shutter_text(secs: f32) -> String {
    if secs <= 0.0 || !secs.is_finite() {
        return "—".to_string();
    }
    if secs < 1.0 {
        format!("1/{} s", (1.0 / secs).round() as i64)
    } else {
        format!("{} s", (secs * 10.0).round() / 10.0)
    }
}

/// A file size in units a person reads.
pub fn size_text(bytes: i64) -> String {
    const MB: f64 = 1024.0 * 1024.0;
    const KB: f64 = 1024.0;
    let b = bytes as f64;
    if b >= MB {
        format!("{:.1} MB", b / MB)
    } else if b >= KB {
        format!("{} KB", (b / KB).round() as i64)
    } else {
        format!("{bytes} B")
    }
}

/// ⚠ An absent value shows as `—`, never as a blank row. A missing field is *information*: it says
/// this file carries no EXIF, which is exactly the case two of the author's exported JPEGs are in.
pub fn opt_text<T: std::fmt::Display>(v: Option<T>) -> String {
    v.map(|v| v.to_string()).unwrap_or_else(|| "—".to_string())
}

/// A wall-clock unix timestamp, formatted.
///
/// ⚠ Read back with the same convention it was stored under — see `Capture::taken`.
pub fn taken_text(secs: Option<i64>) -> String {
    let Some(s) = secs else {
        return "—".to_string();
    };
    chrono::DateTime::from_timestamp(s, 0)
        .map(|d| d.format("%Y-%m-%d %H:%M:%S").to_string())
        .unwrap_or_else(|| "—".to_string())
}

fn row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.horizontal(|ui| {
        ui.add_sized(
            [130.0, ui.spacing().interact_size.y],
            egui::Label::new(egui::RichText::new(label).weak()),
        );
        ui.label(value);
    });
}

pub fn show(app: &mut RustroomApp, ctx: &egui::Context) {
    let Modal::PhotoInfo { id, dims } = &app.modal else {
        return;
    };
    let id = id.clone();
    // ⚠⚠ Read **once**, when the dialog opens, and cached in the modal's own state. A dialog body
    // runs sixty times a second, and a whole-file read there would be the `apply_view_demosaic`
    // mistake in its most expensive costume yet — ~15 ms per frame.
    let dims = match *dims {
        Some(d) => d,
        None => {
            let d = if catalog::is_raw(&id.path) {
                raw_decode::dimensions(&id.path)
            } else {
                image_decode::dimensions(&id.path)
            };
            let d = d.unwrap_or((0, 0));
            app.modal = Modal::PhotoInfo {
                id: id.clone(),
                dims: Some(d),
            };
            d
        }
    };

    let entry = app
        .library
        .as_ref()
        .and_then(|l| l.entries.iter().find(|e| e.id == id).cloned());
    let shown = entry.as_ref().map(|e| e.label()).unwrap_or_else(|| {
        id.path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string()
    });
    let capture = app
        .library
        .as_ref()
        .and_then(|l| l.captures.get(&id.path).cloned())
        .unwrap_or_default();
    let marks = app
        .library
        .as_ref()
        .and_then(|l| l.marks.get(&id).copied())
        .unwrap_or_default();
    let size = std::fs::metadata(&id.path).ok().map(|m| m.len() as i64);

    let mut close = false;
    let dismissed = dialog(ctx, "photo_info", "Information", |ui| {
        egui::ScrollArea::vertical()
            .max_height(460.0)
            .show(ui, |ui| {
                ui.label(egui::RichText::new("File").strong());
                row(ui, "Name", &shown);
                row(ui, "Path", &id.path.to_string_lossy());
                row(ui, "Copy", &id.copy.to_string());
                row(
                    ui,
                    "Size",
                    &size.map(size_text).unwrap_or_else(|| "—".into()),
                );
                ui.separator();

                ui.label(egui::RichText::new("Image").strong());
                if dims == (0, 0) {
                    row(ui, "Dimensions", "—");
                } else {
                    row(ui, "Dimensions", &format!("{} × {}", dims.0, dims.1));
                    let mp = dims.0 as f64 * dims.1 as f64 / 1_000_000.0;
                    row(ui, "Megapixels", &format!("{mp:.1} MP"));
                    let orient = if dims.0 > dims.1 {
                        "Landscape"
                    } else if dims.1 > dims.0 {
                        "Portrait"
                    } else {
                        "Square"
                    };
                    row(ui, "Orientation", orient);
                }
                ui.separator();

                ui.label(egui::RichText::new("Capture").strong());
                row(ui, "Taken", &taken_text(capture.taken));
                row(ui, "Camera", &opt_text(capture.camera.as_deref()));
                row(ui, "Lens", &opt_text(capture.lens.as_deref()));
                row(ui, "ISO", &opt_text(capture.iso));
                row(
                    ui,
                    "Shutter",
                    &capture
                        .shutter
                        .map(shutter_text)
                        .unwrap_or_else(|| "—".into()),
                );
                row(
                    ui,
                    "Aperture",
                    &capture
                        .aperture
                        .map(|a| format!("f/{a}"))
                        .unwrap_or_else(|| "—".into()),
                );
                row(
                    ui,
                    "Focal length",
                    &capture
                        .focal_len
                        .map(|f| format!("{f} mm"))
                        .unwrap_or_else(|| "—".into()),
                );
                ui.separator();

                ui.label(egui::RichText::new("Marks").strong());
                row(
                    ui,
                    "Rating",
                    &if marks.rating == 0 {
                        "—".to_string()
                    } else {
                        "★".repeat(marks.rating as usize)
                    },
                );
                row(
                    ui,
                    "Flag",
                    match marks.flag {
                        catalog::db::Flag::Pick => "⚑ Picked",
                        catalog::db::Flag::Reject => "× Rejected",
                        catalog::db::Flag::None => "—",
                    },
                );
                row(
                    ui,
                    "Colour",
                    &if marks.label == 0 {
                        "—".to_string()
                    } else {
                        ["", "Red", "Blue", "Yellow", "Green"]
                            .get(marks.label as usize)
                            .unwrap_or(&"—")
                            .to_string()
                    },
                );
            });
        ui.separator();
        if ui.button("Close").clicked() {
            close = true;
        }
    });

    if close || dismissed {
        app.modal = Modal::None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fast_shutter_shows_as_a_fraction_and_a_slow_one_as_seconds() {
        assert_eq!(shutter_text(0.02), "1/50 s");
        assert_eq!(shutter_text(0.5), "1/2 s");
        assert_eq!(shutter_text(2.0), "2 s");
        assert_eq!(shutter_text(0.0), "—");
        assert_eq!(shutter_text(f32::NAN), "—");
    }

    #[test]
    fn an_absent_value_shows_as_a_dash_rather_than_a_blank() {
        // ⚠ A missing field is information — it says the file carries no EXIF.
        assert_eq!(opt_text(None::<u32>), "—");
        assert_eq!(opt_text(Some(1600u32)), "1600");
        assert_eq!(taken_text(None), "—");
    }

    #[test]
    fn a_file_size_is_shown_in_units_a_person_reads() {
        assert_eq!(size_text(505_252), "493 KB");
        assert_eq!(size_text(76_802_608), "73.2 MB");
        assert_eq!(size_text(512), "512 B");
    }

    #[test]
    fn a_wall_clock_timestamp_reads_back_as_it_was_stored() {
        // ⚠ Same convention both readers write under — no timezone step in either direction.
        assert_eq!(taken_text(Some(1_779_046_001)), "2026-05-17 19:26:41");
    }
}
