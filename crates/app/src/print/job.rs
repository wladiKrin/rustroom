//! One print: the export it renders through, the pages assembled from what that export wrote, and the
//! sentence the status line ends with.

use super::layout::{self, Fit, Layout, PrintSettings};
use super::pdf::{self, Caption, PageContent, PageImage};
use crate::batch_export::{Destination, ExportOptions, OutputSharpening};
use crate::photo_id::PhotoId;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// The staged files' names — numbered, so two photos of one stem never collide (and never ask).
const STAGED_NAME: &str = "print-{seq:4}";

/// The export a print renders through: JPEG 95 in sRGB into `staging`, at the layout's size, with the print's
/// sharpening and no watermark. `demosaic` is the export's own setting.
pub fn export_options(
    s: &PrintSettings,
    staging: &Path,
    demosaic: raw_decode::Demosaic,
) -> ExportOptions {
    ExportOptions {
        dest_dir: staging.to_path_buf(),
        quality: 95,
        size: layout::export_size(s),
        demosaic,
        format: export::ExportFormat::default(),
        space: color::OutputSpace::Srgb,
        filename_template: STAGED_NAME.into(),
        output_sharpening: OutputSharpening {
            amount: s.sharpening.amount(),
        },
        destination: Destination::Files,
        ..ExportOptions::default()
    }
}

/// The document, and how many photos and pages it holds.
pub struct Assembled {
    pub pdf: Vec<u8>,
    pub photos: usize,
    pub pages: usize,
}

/// The pages of `photos` (each with its caption) from the files the export `written`, in order. A photo the
/// export did not write is left out and the pages close up — the outcome names the failure.
pub fn assemble(
    s: &PrintSettings,
    photos: &[(PhotoId, String)],
    written: &HashMap<PhotoId, PathBuf>,
) -> Result<Assembled, String> {
    let fill = s.layout == Layout::Single && s.fit == Fit::Fill;
    let mut rendered = Vec::new();
    for (id, caption) in photos {
        let Some(file) = written.get(id) else {
            continue;
        };
        let jpeg =
            std::fs::read(file).map_err(|e| format!("could not read a rendered photo: {e}"))?;
        let (w, h) = image::ImageReader::new(std::io::Cursor::new(&jpeg))
            .with_guessed_format()
            .ok()
            .and_then(|r| r.into_dimensions().ok())
            .ok_or("a rendered photo is not a JPEG")?;
        rendered.push((jpeg, (w as usize, h as usize), caption.clone()));
    }
    if rendered.is_empty() {
        return Err("no photo rendered".into());
    }
    let size = layout::page_size_pt(s);
    let pages: Vec<PageContent> = layout::pages(rendered.len(), s)
        .into_iter()
        .map(|on_page| {
            let mut page = PageContent {
                size,
                images: Vec::new(),
                captions: Vec::new(),
            };
            for (slot, i) in on_page.into_iter().enumerate() {
                let (jpeg, px, caption) = &rendered[i];
                let cell = layout::cell(s, slot);
                page.images.push(PageImage {
                    jpeg: jpeg.clone(),
                    px: *px,
                    placed: layout::place(*px, cell, fill, s.rotate),
                });
                if let Some((x, y)) = layout::caption_at(s, slot) {
                    page.captions.push(Caption {
                        x,
                        y,
                        max_w: cell.w,
                        text: caption.clone(),
                    });
                }
            }
            page
        })
        .collect();
    Ok(Assembled {
        pdf: pdf::write(&pages),
        photos: rendered.len(),
        pages: pages.len(),
    })
}

/// Where a print went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delivered {
    Printed { printer: String },
    Saved { file: PathBuf },
}

/// The status line: what went where, then what fell short.
pub fn outcome(to: &Delivered, a: &Assembled, failed: usize, below_ppi: usize, ppi: u16) -> String {
    let photos = plural(a.photos, "photo");
    let pages = plural(a.pages, "page");
    let mut s = match to {
        Delivered::Printed { printer } => format!("Printed {photos} on {pages} to {printer}"),
        Delivered::Saved { file } => format!(
            "Saved {pages} to {}",
            file.file_name().map_or_else(
                || file.display().to_string(),
                |n| n.to_string_lossy().into_owned()
            )
        ),
    };
    if failed > 0 {
        s.push_str(&format!(
            ", {} could not be rendered",
            plural(failed, "photo")
        ));
    }
    if below_ppi > 0 {
        s.push_str(&format!(
            ", {} print below {ppi} PPI",
            plural(below_ppi, "photo")
        ));
    }
    s
}

fn plural(n: usize, word: &str) -> String {
    if n == 1 {
        format!("1 {word}")
    } else {
        format!("{n} {word}s")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::print::pdf::tests::jpeg;

    fn staged(
        dir: &Path,
        names: &[(&str, u32, u32)],
    ) -> (Vec<(PhotoId, String)>, HashMap<PhotoId, PathBuf>) {
        let mut photos = Vec::new();
        let mut written = HashMap::new();
        for (n, w, h) in names {
            let id = PhotoId::original(dir.join(n));
            let file = dir.join(format!("{n}.jpg"));
            std::fs::write(&file, jpeg(*w, *h)).unwrap();
            photos.push((id.clone(), (*n).to_string()));
            written.insert(id, file);
        }
        (photos, written)
    }

    #[test]
    fn the_export_renders_jpegs_in_srgb_at_the_cells_size_with_the_prints_sharpening() {
        let s = PrintSettings::default();
        let o = export_options(&s, Path::new("/stage"), raw_decode::Demosaic::default());
        assert_eq!(o.dest_dir, Path::new("/stage"));
        assert_eq!((o.quality, o.space), (95, color::OutputSpace::Srgb));
        assert_eq!(o.size, layout::export_size(&s));
        assert_eq!(o.output_sharpening.amount, 50);
        assert_eq!(o.destination, Destination::Files);
        assert!(o.filename_template.contains("{seq"));
    }

    #[test]
    fn a_sheet_of_five_on_a_two_by_two_grid_is_two_pages_and_a_missing_photo_closes_up() {
        let dir = test_dir::TempDir::new("print-assemble");
        // Constructed JPEGs.
        let (photos, mut written) = staged(
            dir.path(),
            &[
                ("a", 60, 40),
                ("b", 40, 60),
                ("c", 50, 50),
                ("d", 60, 40),
                ("e", 60, 40),
            ],
        );
        let s = PrintSettings {
            layout: Layout::ContactSheet,
            columns: 2,
            rows: 2,
            ..PrintSettings::default()
        };
        let a = assemble(&s, &photos, &written).unwrap();
        assert_eq!((a.photos, a.pages), (5, 2));
        written.remove(&photos[4].0);
        let a = assemble(&s, &photos, &written).unwrap();
        assert_eq!((a.photos, a.pages), (4, 1));
        let t = String::from_utf8_lossy(&a.pdf);
        assert!(t.contains("(a) Tj") && t.contains("(d) Tj") && !t.contains("(e) Tj"));
        assert!(t.contains("/Width 40 /Height 60"));
    }

    #[test]
    fn nothing_rendered_is_an_error() {
        let s = PrintSettings::default();
        let photos = vec![(PhotoId::original(PathBuf::from("/x/a.raf")), "a".into())];
        assert!(assemble(&s, &photos, &HashMap::new()).is_err());
    }

    #[test]
    fn the_outcome_names_where_it_went_and_what_fell_short() {
        let a = Assembled {
            pdf: Vec::new(),
            photos: 12,
            pages: 3,
        };
        let printed = Delivered::Printed {
            printer: "Canon_G620".into(),
        };
        assert_eq!(
            outcome(&printed, &a, 0, 0, 300),
            "Printed 12 photos on 3 pages to Canon_G620"
        );
        let saved = Delivered::Saved {
            file: PathBuf::from("/home/x/contact.pdf"),
        };
        assert_eq!(
            outcome(&saved, &a, 1, 2, 300),
            "Saved 3 pages to contact.pdf, 1 photo could not be rendered, 2 photos print below 300 PPI"
        );
    }
}
