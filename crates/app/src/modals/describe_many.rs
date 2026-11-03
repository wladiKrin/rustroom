//! Several photos' title, caption, copyright and creator, edited at once (2026-11-03): a field shows
//! the value they share or reads "Mixed", and Save writes only the fields typed in, to every photo.
//!
//! ⚠ The rule lives in `rustroom_app::description::BatchDraft`; this is the dialog.

use super::{dialog, Footer, Modal};
use crate::theme::Hover as _;
use crate::{theme, RustroomApp};
use eframe::egui;
use rustroom_app::description::{commit_many, BatchDraft, Field};

/// The fields' hover — `info::SHARED_NOTE`'s, for several photos.
const NOTE: &str = "Applies to every selected file and is written into exports and the XMP sidecar";

/// Open the dialog on `photos`' files, reading their descriptions once.
pub fn open(app: &mut RustroomApp, photos: &[std::path::PathBuf]) {
    app.modal = Modal::DescribeMany {
        draft: BatchDraft::open(app.catalog.as_ref(), photos),
    };
}

pub fn show(app: &mut RustroomApp, ctx: &egui::Context) {
    let Modal::DescribeMany { draft } = &app.modal else {
        return;
    };
    let mut draft = draft.clone();
    let n = draft.photos.len();
    let out = dialog(
        ctx,
        &mut app.settings,
        "describe_many",
        &format!("Edit description of {n} photos"),
        theme::DialogSize::Standard,
        Footer::confirm(
            "Save",
            "Writes the fields you changed to every selected photo",
            draft.touched.iter().any(|t| *t),
            "Cancel",
        ),
        |ui| {
            egui::Grid::new("describe_many_grid")
                .num_columns(2)
                .spacing([12.0, 6.0])
                .show(ui, |ui| {
                    for f in Field::ALL {
                        let i = f as usize;
                        let mixed = draft.shared(f).is_none() && !draft.touched[i];
                        ui.label(egui::RichText::new(label(f)).weak()).hover(NOTE);
                        let mut edit = if f == Field::Caption {
                            egui::TextEdit::multiline(&mut draft.edited[i]).desired_rows(2)
                        } else {
                            egui::TextEdit::singleline(&mut draft.edited[i])
                        }
                        .desired_width(f32::INFINITY);
                        // ⚠ A mixed field is blank with a hint, and stays each photo's own until typed in.
                        if mixed {
                            edit = edit.hint_text("Mixed");
                        }
                        if ui.add(edit).changed() {
                            draft.touched[i] = true;
                        }
                        ui.end_row();
                    }
                });
            theme::hint(ui, &format!("Changes {n} photos."));
        },
    );
    if out.primary {
        if let Some(cat) = &app.catalog {
            let written = commit_many(cat, &draft);
            if !written.is_empty() {
                app.write_xmp_for(&written);
            }
            app.status = match written.len() {
                1 => "Changed the description of 1 photo".into(),
                w => format!("Changed the description of {w} photos"),
            };
        }
        app.modal = Modal::None;
    } else if out.dismissed {
        app.modal = Modal::None;
    } else {
        app.modal = Modal::DescribeMany { draft };
    }
}

fn label(f: Field) -> &'static str {
    match f {
        Field::Title => "Title",
        Field::Caption => "Caption",
        Field::Copyright => "Copyright",
        Field::Creator => "Creator",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modals::Modal;
    use rustroom_app::description::{BatchDraft, Field};

    fn app_with_three_catalogued_photos() -> (crate::RustroomApp, Vec<std::path::PathBuf>) {
        let mut app = crate::test_app::for_test();
        app.view = crate::AppView::Library;
        app.library = Some(crate::test_app::library_of(3));
        crate::test_app::catalog_library(&mut app);
        let paths = app
            .library
            .as_ref()
            .unwrap()
            .entries
            .iter()
            .map(|e| e.id.path.clone())
            .collect();
        (app, paths)
    }

    fn settle(ctx: &egui::Context, app: &mut crate::RustroomApp) {
        let _ = crate::test_ui::labelled_centres(ctx, Vec::new(), |_| show(app, ctx));
    }

    fn caption(app: &crate::RustroomApp, p: &std::path::Path) -> String {
        app.catalog
            .as_ref()
            .unwrap()
            .description_of(p)
            .unwrap()
            .unwrap_or_default()
            .caption
    }

    /// The menu's entry opens the dialog on every targeted photo.
    #[test]
    fn the_menu_entry_opens_the_dialog_on_the_selection() {
        let ctx = crate::test_ui::ctx();
        let (mut app, paths) = app_with_three_catalogued_photos();
        let lib = app.library.as_mut().unwrap();
        lib.selection.click(0);
        lib.selection.toggle(1);
        lib.selection.toggle(2);
        crate::library::apply_cell_action(
            &mut app,
            &ctx,
            0,
            crate::library::CellAction::DescribeMany,
        );
        let Modal::DescribeMany { draft } = &app.modal else {
            panic!("the dialog did not open");
        };
        assert_eq!(draft.photos, paths);
    }

    /// ⚠ Save is a real click: the commit is the wiring that would be forgotten.
    #[test]
    fn save_writes_the_typed_caption_to_every_photo() {
        let ctx = crate::test_ui::ctx();
        let (mut app, paths) = app_with_three_catalogued_photos();
        let mut draft = BatchDraft::open(app.catalog.as_ref(), &paths);
        draft.set(Field::Caption, "Boats returning");
        app.modal = Modal::DescribeMany { draft };
        settle(&ctx, &mut app);
        crate::test_ui::click(&ctx, "Save", |_| show(&mut app, &ctx));
        assert!(app.modal == Modal::None, "the dialog did not close");
        for p in &paths {
            assert_eq!(caption(&app, p), "Boats returning", "{p:?}");
        }
    }

    #[test]
    fn cancel_writes_nothing() {
        let ctx = crate::test_ui::ctx();
        let (mut app, paths) = app_with_three_catalogued_photos();
        let mut draft = BatchDraft::open(app.catalog.as_ref(), &paths);
        draft.set(Field::Caption, "Boats returning");
        app.modal = Modal::DescribeMany { draft };
        settle(&ctx, &mut app);
        crate::test_ui::click(&ctx, "Cancel", |_| show(&mut app, &ctx));
        assert!(app.modal == Modal::None, "the dialog did not close");
        for p in &paths {
            assert_eq!(caption(&app, p), "", "{p:?}");
        }
    }
}
