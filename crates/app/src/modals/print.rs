//! File → Print… (M10.18e): the page preview on the left, the layout, paper and destination on the right.
//!
//! ⚠ Every rule is `rustroom_app::print`'s; this paints a page from it and edits a `PrintSettings` draft.

use super::{dialog, Footer, Modal};
use crate::print_flow::Target;
use crate::theme::Hover as _;
use crate::{theme, RustroomApp};
use eframe::egui;
use rustroom_app::photo_id::PhotoId;
use rustroom_app::print::layout::{
    self, Fit, Layout, Orientation, Paper, PrintSettings, Sharpening, PPI_CHOICES,
};

/// The preview's box, in points on screen.
const PREVIEW: egui::Vec2 = egui::vec2(340.0, 440.0);

/// The dialog's state.
#[derive(PartialEq)]
pub struct PrintDraft {
    pub settings: PrintSettings,
    pub photos: Vec<(PhotoId, String)>,
    /// What `lpstat` listed when the dialog opened — empty without CUPS.
    pub printers: Vec<String>,
    /// The page the preview shows.
    pub page: usize,
}

impl PrintDraft {
    /// The printer the draft prints to — the remembered one only while it is still listed.
    pub fn printer(&self) -> Option<&str> {
        self.settings
            .printer
            .as_deref()
            .filter(|p| self.printers.iter().any(|q| q == p))
    }
}

/// Open the dialog on `photos`, the last settings and `printers`.
pub fn open(app: &mut RustroomApp, photos: Vec<(PhotoId, String)>, printers: Vec<String>) {
    app.modal = Modal::Print(Box::new(PrintDraft {
        settings: app.settings.print.clone(),
        photos,
        printers,
        page: 0,
    }));
}

pub fn show(app: &mut RustroomApp, ctx: &egui::Context) {
    let Modal::Print(draft) = &app.modal else {
        return;
    };
    let mut d = PrintDraft {
        settings: draft.settings.clone(),
        photos: draft.photos.clone(),
        printers: draft.printers.clone(),
        page: draft.page,
    };
    // The thumbnails, looked up once a frame before the dialog borrows the app.
    let thumbs: Vec<Option<egui::TextureHandle>> = d
        .photos
        .iter()
        .map(|(id, _)| {
            app.library
                .as_ref()
                .and_then(|l| l.cache.get_stale(id).cloned())
        })
        .collect();
    let n = d.photos.len();
    let printing = d.printer().is_some();
    let (primary, says) = if printing {
        ("Print", "Sends the pages to the chosen printer")
    } else {
        ("Save PDF…", "Writes the pages to a PDF file you choose")
    };
    let out = dialog(
        ctx,
        &mut app.settings,
        "print",
        &format!("Print {}", plural(n, "photo")),
        theme::DialogSize::Wide,
        Footer::confirm(primary, says, n > 0, "Cancel"),
        |ui| {
            ui.horizontal_top(|ui| {
                ui.vertical(|ui| preview(ui, &mut d, &thumbs));
                ui.add_space(16.0);
                ui.vertical(|ui| controls(ui, &mut d));
            });
        },
    );
    if out.primary {
        let target = match d.printer() {
            Some(p) => Some(Target::Printer(p.to_owned())),
            None => save_path().map(Target::Pdf),
        };
        if let Some(target) = target {
            app.modal = Modal::None;
            app.start_print(d.settings, d.photos, target);
        } else {
            app.modal = Modal::Print(Box::new(d));
        }
    } else if out.dismissed {
        app.modal = Modal::None;
    } else {
        app.modal = Modal::Print(Box::new(d));
    }
}

/// Where the PDF goes — asked of the system's save dialog.
fn save_path() -> Option<std::path::PathBuf> {
    rfd::FileDialog::new()
        .set_file_name("print.pdf")
        .add_filter("PDF", &["pdf"])
        .save_file()
        .map(|p| {
            if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")) {
                p
            } else {
                p.with_extension("pdf")
            }
        })
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

/// The current page, painted as it will print, and the page buttons under it.
fn preview(ui: &mut egui::Ui, d: &mut PrintDraft, thumbs: &[Option<egui::TextureHandle>]) {
    let s = &d.settings;
    let pages = layout::pages(d.photos.len(), s);
    d.page = d.page.min(pages.len().saturating_sub(1));
    let (pw, ph) = layout::page_size_pt(s);
    let (area, _) = ui.allocate_exact_size(PREVIEW, egui::Sense::hover());
    let k = (PREVIEW.x / pw).min(PREVIEW.y / ph);
    let sheet = egui::Rect::from_center_size(area.center(), egui::vec2(pw * k, ph * k));
    let to_screen = |r: layout::Rect| {
        egui::Rect::from_min_size(
            egui::pos2(sheet.left() + r.x * k, sheet.bottom() - (r.y + r.h) * k),
            egui::vec2(r.w * k, r.h * k),
        )
    };
    let painter = ui.painter_at(area);
    painter.rect_filled(sheet, 0.0, theme::paper());
    let fill = s.layout == Layout::Single && s.fit == Fit::Fill;
    for (slot, &i) in pages.get(d.page).into_iter().flatten().enumerate() {
        let cell = layout::cell(s, slot);
        // ⚠ A photo with no thumbnail yet stands in at 3:2 — its cell is still where it prints.
        let tex = thumbs.get(i).and_then(Option::as_ref);
        let px = tex.map_or((3, 2), |t| (t.size()[0], t.size()[1]));
        let placed = layout::place(px, cell, fill, s.rotate);
        let (img, clip) = (to_screen(placed.image), to_screen(placed.clip));
        let p = painter.with_clip_rect(clip.intersect(sheet));
        match tex {
            Some(t) => p.add(image_mesh(t.id(), img, placed.rotated)),
            None => p.rect_filled(img, 0.0, ui.visuals().faint_bg_color),
        };
        if let Some((cx, cy)) = layout::caption_at(s, slot) {
            let w = cell.w * k * 0.6;
            let y = sheet.bottom() - cy * k;
            painter.line_segment(
                [
                    egui::pos2(sheet.left() + cx * k - w / 2.0, y),
                    egui::pos2(sheet.left() + cx * k + w / 2.0, y),
                ],
                egui::Stroke::new(1.0, theme::paper_ink()),
            );
        }
    }
    ui.horizontal(|ui| {
        let last = pages.len().saturating_sub(1);
        if ui
            .add_enabled(d.page > 0, egui::Button::new("Previous"))
            .hover("Shows the previous page")
            .clicked()
        {
            d.page -= 1;
        }
        ui.label(format!("Page {} of {}", d.page + 1, pages.len().max(1)));
        if ui
            .add_enabled(d.page < last, egui::Button::new("Next"))
            .hover("Shows the next page")
            .clicked()
        {
            d.page += 1;
        }
    });
}

/// A thumbnail stretched over `r`, turned a quarter when `rotated` — as the PDF's matrix turns it: the
/// picture's top edge runs up the rectangle's left side.
fn image_mesh(tex: egui::TextureId, r: egui::Rect, rotated: bool) -> egui::Shape {
    let mut m = egui::Mesh::with_texture(tex);
    let white = egui::Color32::WHITE;
    let uv = |u: f32, v: f32| egui::pos2(u, v);
    let corners = if rotated {
        [uv(1.0, 0.0), uv(1.0, 1.0), uv(0.0, 1.0), uv(0.0, 0.0)]
    } else {
        [uv(0.0, 0.0), uv(1.0, 0.0), uv(1.0, 1.0), uv(0.0, 1.0)]
    };
    for (pos, uv) in [
        r.left_top(),
        r.right_top(),
        r.right_bottom(),
        r.left_bottom(),
    ]
    .into_iter()
    .zip(corners)
    {
        m.vertices.push(egui::epaint::Vertex {
            pos,
            uv,
            color: white,
        });
    }
    m.indices.extend_from_slice(&[0, 1, 2, 0, 2, 3]);
    egui::Shape::mesh(m)
}

/// The settings, in the order a print is thought about: layout, paper, quality, destination.
fn controls(ui: &mut egui::Ui, d: &mut PrintDraft) {
    let s = &mut d.settings;
    crate::segmented::segmented(
        ui,
        &mut s.layout,
        &[
            (Layout::Single, "Single", "Prints one photo on each page"),
            (
                Layout::ContactSheet,
                "Contact sheet",
                "Prints the photos in a grid, several to a page",
            ),
        ],
    );
    ui.add_space(8.0);
    egui::Grid::new("print_grid")
        .num_columns(2)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            if s.layout == Layout::Single {
                ui.label("Photo");
                crate::segmented::segmented(
                    ui,
                    &mut s.fit,
                    &[
                        (Fit::Fit, "Fit", "Shows the whole photo inside the margins"),
                        (
                            Fit::Fill,
                            "Fill",
                            "Covers the page inside the margins, cropping the photo's edges",
                        ),
                    ],
                );
                ui.end_row();
            } else {
                ui.label("Columns");
                ui.add(egui::DragValue::new(&mut s.columns).range(1..=8))
                    .hover("Sets how many photos sit side by side");
                ui.end_row();
                ui.label("Rows");
                ui.add(egui::DragValue::new(&mut s.rows).range(1..=10))
                    .hover("Sets how many rows of photos fit on a page");
                ui.end_row();
                ui.label("Gap");
                ui.add(
                    egui::DragValue::new(&mut s.gap_mm)
                        .range(0..=30)
                        .suffix(" mm"),
                )
                .hover("Sets the space between photos");
                ui.end_row();
                ui.label("");
                ui.checkbox(&mut s.captions, "File names")
                    .hover("Prints each photo's file name under it");
                ui.end_row();
            }
            ui.label("");
            ui.checkbox(&mut s.rotate, "Rotate to fit")
                .hover("Turns a photo on the page when that prints it larger");
            ui.end_row();

            ui.label("Paper");
            egui::ComboBox::from_id_salt("print_paper")
                .selected_text(s.paper.label())
                .show_ui(ui, |ui| {
                    for p in Paper::ALL {
                        ui.selectable_value(&mut s.paper, p, p.label());
                    }
                })
                .response
                .hover("Chooses the paper size the pages are laid out for");
            ui.end_row();
            ui.label("");
            crate::segmented::segmented(
                ui,
                &mut s.orientation,
                &[
                    (
                        Orientation::Portrait,
                        "Portrait",
                        "Lays the page out upright",
                    ),
                    (
                        Orientation::Landscape,
                        "Landscape",
                        "Lays the page out on its side",
                    ),
                ],
            );
            ui.end_row();
            ui.label("Margin");
            ui.add(
                egui::DragValue::new(&mut s.margin_mm)
                    .range(0..=50)
                    .suffix(" mm"),
            )
            .hover("Sets the white border on every side of the page");
            ui.end_row();

            ui.label("Resolution");
            egui::ComboBox::from_id_salt("print_ppi")
                .selected_text(format!("{} PPI", s.ppi))
                .show_ui(ui, |ui| {
                    for p in PPI_CHOICES {
                        ui.selectable_value(&mut s.ppi, p, format!("{p} PPI"));
                    }
                })
                .response
                .hover("Sets how many pixels each inch of the print is rendered with");
            ui.end_row();
            ui.label("Sharpening");
            egui::ComboBox::from_id_salt("print_sharpen")
                .selected_text(s.sharpening.label())
                .show_ui(ui, |ui| {
                    for v in Sharpening::ALL {
                        ui.selectable_value(&mut s.sharpening, v, v.label());
                    }
                })
                .response
                .hover("Sharpens for paper, which softens a photo more than a screen does");
            ui.end_row();

            ui.label("Destination");
            let shown = match s
                .printer
                .as_deref()
                .filter(|p| d.printers.iter().any(|q| q == p))
            {
                Some(p) => p.to_owned(),
                None => "PDF file".to_owned(),
            };
            let combo = ui.add_enabled_ui(!d.printers.is_empty(), |ui| {
                egui::ComboBox::from_id_salt("print_to")
                    .selected_text(shown)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut s.printer, None, "PDF file");
                        for p in &d.printers {
                            ui.selectable_value(&mut s.printer, Some(p.clone()), p);
                        }
                    })
            });
            if d.printers.is_empty() {
                combo
                    .response
                    .hover("Saves a PDF, since the system lists no printer");
            } else {
                combo
                    .response
                    .hover("Chooses a printer, or a PDF file to print later or send to a lab");
            }
            ui.end_row();
            if s.printer
                .as_deref()
                .is_some_and(|p| d.printers.iter().any(|q| q == p))
            {
                ui.label("Copies");
                ui.add(egui::DragValue::new(&mut s.copies).range(1..=99))
                    .hover("Sets how many copies of every page the printer makes");
                ui.end_row();
            }
        });
    let pages = layout::pages(d.photos.len(), s).len();
    theme::hint(
        ui,
        &format!(
            "{} on {}.",
            plural(d.photos.len(), "photo"),
            plural(pages, "page")
        ),
    );
}
