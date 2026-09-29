//! Everything Rustroom knows about one photo.
//!
//! ⚠ The formatting lives in pure functions below so it can be tested; the dialog itself is glue.

use crate::modals::shell::tabs;
use crate::modals::{dialog, Footer, Modal};
use crate::theme;
use crate::RustroomApp;
use eframe::egui;
use crate::theme::Hover as _;

/// The description fields' hover. ⚠ It was a paragraph under them (`docs/DESIGN.md` §8).
const SHARED_NOTE: &str =
    "Applies to every copy of this file and is written into exports and the XMP sidecar";

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

/// One `label: value` line, inside a [`grid`] block.
///
/// ⚠ Must be called inside `Grid::show` — that is what aligns the value column. The previous
/// version used a fixed 130 pt label, which aligns until a label is longer than that, and is a
/// magic number besides. `egui::Grid` sizes the column to the widest row instead.
fn row(ui: &mut egui::Ui, label: &str, value: &str) {
    ui.label(egui::RichText::new(label).weak());
    ui.label(value);
    ui.end_row();
}

/// A block of `label: value` rows with an aligned value column.
///
/// ⚠ Each block needs its **own** salt: `egui::Grid` sizes columns per id, so sharing one across
/// the File/Image/Capture/Marks blocks would make them share a width — and "Path" is far longer
/// than "ISO", so every block would inherit the widest label in the dialog.
fn grid(ui: &mut egui::Ui, salt: &str, rows: impl FnOnce(&mut egui::Ui)) {
    egui::Grid::new(salt)
        .num_columns(2)
        .spacing([12.0, 4.0])
        .show(ui, rows);
}

pub fn show(app: &mut RustroomApp, ctx: &egui::Context) {
    let Modal::PhotoInfo {
        id,
        dims,
        description,
    } = &app.modal
    else {
        return;
    };
    let id = id.clone();
    // ⚠⚠ Read **once**, when the dialog opens, and cached in the modal's own state. A dialog body
    // runs sixty times a second, and a whole-file read there would be the `apply_view_demosaic`
    // mistake in its most expensive costume yet — ~15 ms per frame. The description draft follows
    // the same rule for the same reason, one catalog query instead of one file read. Both are
    // written back into the modal at the end of this function.
    let dims = match *dims {
        Some(d) => d,
        None => {
            let d = if catalog::is_raw(&id.path) {
                raw_decode::dimensions(&id.path)
            } else {
                image_decode::dimensions(&id.path)
            };
            d.unwrap_or((0, 0))
        }
    };
    let mut draft = match description {
        Some(d) => d.clone(),
        None => rustroom_app::description::DescriptionDraft::open(app.catalog.as_ref(), &id.path),
    };

    let entry = app
        .library
        .as_ref()
        .and_then(|l| l.entries.iter().find(|e| e.id == id).cloned());
    let shown = entry.as_ref().map(|e| e.caption()).unwrap_or_else(|| {
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

    let out = dialog(
        ctx,
        &mut app.settings,
        "photo_info",
        &shown,
        theme::DialogSize::Wide,
        Footer::close("Close"),
        |ui| {
            let tab = tabs(
                ui,
                egui::Id::new("info_tab"),
                &["File", "Image", "Capture", "Description", "Marks"],
            );
            match tab {
                0 => grid(ui, "info_file", |ui| {
                    row(ui, "Name", &shown);
                    row(ui, "Path", &id.path.to_string_lossy());
                    row(
                        ui,
                        "Version",
                        &if id.copy == 0 {
                            "Original".to_string()
                        } else {
                            format!("Copy {}", id.copy)
                        },
                    );
                    row(
                        ui,
                        "Size",
                        &size.map(size_text).unwrap_or_else(|| "—".into()),
                    );
                }),
                1 => grid(ui, "info_image", |ui| {
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
                }),
                2 => grid(ui, "info_capture", |ui| {
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
                }),
                3 => {
                    grid(ui, "info_description", |ui| {
                        ui.label(egui::RichText::new("Title").weak())
                            .hover(SHARED_NOTE);
                        ui.text_edit_singleline(&mut draft.edited.title);
                        ui.end_row();
                        ui.label(egui::RichText::new("Caption").weak())
                            .hover(SHARED_NOTE);
                        ui.add(
                            egui::TextEdit::multiline(&mut draft.edited.caption)
                                .desired_rows(2)
                                .desired_width(f32::INFINITY),
                        );
                        ui.end_row();
                        ui.label(egui::RichText::new("Copyright").weak())
                            .hover(SHARED_NOTE);
                        ui.text_edit_singleline(&mut draft.edited.copyright);
                        ui.end_row();
                        ui.label(egui::RichText::new("Creator").weak())
                            .hover(SHARED_NOTE);
                        ui.text_edit_singleline(&mut draft.edited.creator);
                        ui.end_row();
                    });
                }
                _ => grid(ui, "info_marks", |ui| {
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
                }),
            }
        },
    );
    if out.dismissed {
        // ⚠ Commit on close, whichever way it closed — and only when changed, or the sidecar
        // rewrite below would move its mtime for nothing (`description::commit`).
        if let Some(cat) = &app.catalog {
            if rustroom_app::description::commit(cat, &id.path, &draft) {
                // ⚠ Not gated on `is_original()` the way `apply_mark_to` is: a description is
                // per **file**, so editing it from a copy's dialog changes what the sidecar says.
                app.write_xmp_for(std::slice::from_ref(&id.path));
            }
        }
        app.modal = Modal::None;
    } else {
        app.modal = Modal::PhotoInfo {
            id,
            dims: Some(dims),
            description: Some(draft),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modals::Modal;
    use catalog::db::Description;
    use rustroom_app::description::DescriptionDraft;

    fn app_with_one_catalogued_photo() -> (crate::RustroomApp, crate::PhotoId) {
        let mut app = crate::test_app::for_test();
        app.view = crate::AppView::Library;
        app.library = Some(crate::test_app::library_of(1));
        // ⚠ Without this `set_description` is a no-op on an uncatalogued photo and every
        // assertion below fails against correct code — see `catalog_library`.
        crate::test_app::catalog_library(&mut app);
        let id = app.library.as_ref().unwrap().entries[0].id.clone();
        (app, id)
    }

    /// One layout frame with no input, so the dialog's resize and scroll area settle.
    ///
    /// ⚠⚠ Load-bearing, found by a failing test: on the very first frame the modal measures its
    /// content, and the Close button's reported centre is not where it is drawn from the second
    /// frame on. `test_ui::click` locates on its own first frame, so without this warm-up the
    /// press lands beside the button and the dialog silently stays open.
    fn settle(ctx: &egui::Context, app: &mut crate::RustroomApp) {
        let _ = crate::test_ui::labelled_centres(ctx, Vec::new(), |_| show(app, ctx));
    }

    /// ⚠⚠ The dialog-to-catalog carrier: every M5.3b defect was in dialog and panel wiring and
    /// none was caught by a test before it was reported. The draft is set directly — typing into
    /// an egui `TextEdit` is egui's to prove — and the **close** is a real click, because the
    /// commit-on-close is the wiring that would be forgotten.
    #[test]
    fn closing_the_dialog_commits_a_changed_description() {
        let ctx = crate::test_ui::ctx();
        let (mut app, id) = app_with_one_catalogued_photo();
        app.modal = Modal::PhotoInfo {
            id: id.clone(),
            dims: Some((0, 0)),
            description: Some(DescriptionDraft {
                original: Description::default(),
                edited: Description {
                    caption: "Dusk over the harbour".into(),
                    ..Default::default()
                },
            }),
        };
        settle(&ctx, &mut app);
        crate::test_ui::click(&ctx, "Close", |_| show(&mut app, &ctx));
        assert!(app.modal == Modal::None, "the dialog did not close");
        let got = app
            .catalog
            .as_ref()
            .unwrap()
            .description_of(&id.path)
            .unwrap()
            .expect("the caption never reached the catalog");
        assert_eq!(got.caption, "Dusk over the harbour");
    }

    #[test]
    fn closing_an_untouched_dialog_writes_nothing() {
        let ctx = crate::test_ui::ctx();
        let (mut app, id) = app_with_one_catalogued_photo();
        app.modal = Modal::PhotoInfo {
            id: id.clone(),
            dims: Some((0, 0)),
            description: None,
        };
        // The warm-up frame fills the draft from the catalog (empty); the click then closes it.
        settle(&ctx, &mut app);
        crate::test_ui::click(&ctx, "Close", |_| show(&mut app, &ctx));
        assert!(app.modal == Modal::None);
        assert_eq!(
            app.catalog
                .as_ref()
                .unwrap()
                .description_of(&id.path)
                .unwrap(),
            None,
            "an unchanged draft produced a row"
        );
    }

    #[test]
    fn the_description_fields_are_offered() {
        let ctx = crate::test_ui::ctx();
        let (mut app, id) = app_with_one_catalogued_photo();
        app.modal = Modal::PhotoInfo {
            id,
            dims: Some((0, 0)),
            description: None,
        };
        let drawn: Vec<String> =
            crate::test_ui::labelled_centres(&ctx, Vec::new(), |_| show(&mut app, &ctx))
                .into_iter()
                .map(|(l, _)| l)
                .collect();
        // ⚠ A `TextEdit` produces an AccessKit node; its *label* is drawn as a plain label,
        // which does not. So what is checked is that the dialog still offers Close and has
        // grown, not the field captions themselves — stated rather than papered over.
        assert!(drawn.iter().any(|l| l == "Close"), "{drawn:?}");
    }

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
