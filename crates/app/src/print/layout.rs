//! A print's pages: the paper, the cells on it, and where a photo lands in its cell. In PDF points, the
//! origin at the page's bottom-left.

use export::sizing::{FitMode, LengthUnit, OutputSize};
use serde::{Deserialize, Serialize};

/// Points per millimetre.
const PT_PER_MM: f32 = 72.0 / 25.4;

/// A caption's size on paper, in points — small print under a contact sheet's cell.
pub const CAPTION_PT: f32 = 7.0;

/// The band under a contact sheet's photo its caption takes.
const CAPTION_BAND: f32 = CAPTION_PT * 2.0;

/// The paper sizes offered.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Paper {
    A3,
    #[default]
    A4,
    A5,
    Letter,
    In4x6,
    In5x7,
    In8x10,
}

impl Paper {
    pub const ALL: [Paper; 7] = [
        Paper::A3,
        Paper::A4,
        Paper::A5,
        Paper::Letter,
        Paper::In4x6,
        Paper::In5x7,
        Paper::In8x10,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Paper::A3 => "A3",
            Paper::A4 => "A4",
            Paper::A5 => "A5",
            Paper::Letter => "Letter",
            Paper::In4x6 => "4 × 6 in",
            Paper::In5x7 => "5 × 7 in",
            Paper::In8x10 => "8 × 10 in",
        }
    }

    /// Width and height upright, in millimetres.
    pub fn size_mm(self) -> (f32, f32) {
        match self {
            Paper::A3 => (297.0, 420.0),
            Paper::A4 => (210.0, 297.0),
            Paper::A5 => (148.0, 210.0),
            Paper::Letter => (215.9, 279.4),
            Paper::In4x6 => (101.6, 152.4),
            Paper::In5x7 => (127.0, 177.8),
            Paper::In8x10 => (203.2, 254.0),
        }
    }

    /// The name CUPS's `-o media=` takes — PWG's self-describing names for the photo sizes.
    pub fn media(self) -> &'static str {
        match self {
            Paper::A3 => "A3",
            Paper::A4 => "A4",
            Paper::A5 => "A5",
            Paper::Letter => "Letter",
            Paper::In4x6 => "na_index-4x6_4x6in",
            Paper::In5x7 => "na_5x7_5x7in",
            Paper::In8x10 => "na_govt-letter_8x10in",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Orientation {
    #[default]
    Portrait,
    Landscape,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Layout {
    /// One photo per page.
    #[default]
    Single,
    /// A grid of photos, each fitted in its cell.
    ContactSheet,
}

/// How a single photo meets its page.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Fit {
    /// The whole photo inside the margins.
    #[default]
    Fit,
    /// The area inside the margins covered, the excess cropped evenly.
    Fill,
}

/// Print sharpening, export's `OutputSharpening` by name.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Sharpening {
    Off,
    Low,
    #[default]
    Standard,
    High,
}

impl Sharpening {
    pub const ALL: [Sharpening; 4] = [
        Sharpening::Off,
        Sharpening::Low,
        Sharpening::Standard,
        Sharpening::High,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Sharpening::Off => "Off",
            Sharpening::Low => "Low",
            Sharpening::Standard => "Standard",
            Sharpening::High => "High",
        }
    }

    pub fn amount(self) -> u8 {
        match self {
            Sharpening::Off => 0,
            Sharpening::Low => 25,
            Sharpening::Standard => 50,
            Sharpening::High => 75,
        }
    }
}

/// The Print dialog's settings, remembered when a print starts (`start_export`'s rule).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PrintSettings {
    pub layout: Layout,
    pub paper: Paper,
    pub orientation: Orientation,
    /// One margin for all four sides, in millimetres.
    pub margin_mm: u8,
    /// Single only: a contact sheet always fits.
    pub fit: Fit,
    pub columns: u8,
    pub rows: u8,
    /// Between a contact sheet's cells, in millimetres.
    pub gap_mm: u8,
    /// A contact sheet's file names under the photos.
    pub captions: bool,
    /// A photo turns 90° when that makes it larger.
    pub rotate: bool,
    pub ppi: u16,
    pub sharpening: Sharpening,
    /// The printer last printed to; `None` saves a PDF.
    pub printer: Option<String>,
    pub copies: u16,
}

impl Default for PrintSettings {
    /// ⚠ Hand-written: the container-level `serde(default)` fills a missing field from this, and a derived
    /// one would give an older file a 0 PPI and a 0 × 0 sheet (`OutputSize::default`'s trap).
    fn default() -> Self {
        Self {
            layout: Layout::Single,
            paper: Paper::A4,
            orientation: Orientation::Portrait,
            margin_mm: 10,
            fit: Fit::Fit,
            columns: 4,
            rows: 5,
            gap_mm: 5,
            captions: true,
            rotate: true,
            ppi: 300,
            sharpening: Sharpening::Standard,
            printer: None,
            copies: 1,
        }
    }
}

pub const PPI_CHOICES: [u16; 4] = [150, 240, 300, 360];

/// A rectangle in points, `(x, y)` its bottom-left corner.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// The page's width and height in points, as it is turned.
pub fn page_size_pt(s: &PrintSettings) -> (f32, f32) {
    let (w, h) = s.paper.size_mm();
    let (w, h) = (w * PT_PER_MM, h * PT_PER_MM);
    match s.orientation {
        Orientation::Portrait => (w, h),
        Orientation::Landscape => (h, w),
    }
}

/// The grid: columns and rows, at least one of each.
fn grid(s: &PrintSettings) -> (usize, usize) {
    match s.layout {
        Layout::Single => (1, 1),
        Layout::ContactSheet => (usize::from(s.columns.max(1)), usize::from(s.rows.max(1))),
    }
}

/// How many photos a page holds.
pub fn per_page(s: &PrintSettings) -> usize {
    let (c, r) = grid(s);
    c * r
}

/// `n` photos' indices, page by page — the last page holds what is left.
pub fn pages(n: usize, s: &PrintSettings) -> Vec<Vec<usize>> {
    let per = per_page(s);
    (0..n)
        .collect::<Vec<_>>()
        .chunks(per)
        .map(<[usize]>::to_vec)
        .collect()
}

fn captions_shown(s: &PrintSettings) -> bool {
    s.layout == Layout::ContactSheet && s.captions
}

/// Slot `slot`'s whole cell, its caption's band included — left to right, top to bottom.
fn whole_cell(s: &PrintSettings, slot: usize) -> Rect {
    let (pw, ph) = page_size_pt(s);
    let m = f32::from(s.margin_mm) * PT_PER_MM;
    let (c, r) = grid(s);
    let g = if s.layout == Layout::ContactSheet {
        f32::from(s.gap_mm) * PT_PER_MM
    } else {
        0.0
    };
    // ⚠ Never negative: a margin or gap too wide for the paper leaves a 1-pt cell rather than a flipped one.
    let cw = ((pw - 2.0 * m - (c - 1) as f32 * g) / c as f32).max(1.0);
    let ch = ((ph - 2.0 * m - (r - 1) as f32 * g) / r as f32).max(1.0);
    let (col, row) = (slot % c, (slot / c) % r);
    Rect {
        x: m + col as f32 * (cw + g),
        y: ph - m - (row + 1) as f32 * ch - row as f32 * g,
        w: cw,
        h: ch,
    }
}

/// Slot `slot`'s photo box — the cell, less its caption's band.
pub fn cell(s: &PrintSettings, slot: usize) -> Rect {
    let mut r = whole_cell(s, slot);
    if captions_shown(s) {
        let band = CAPTION_BAND.min(r.h * 0.5);
        r.y += band;
        r.h -= band;
    }
    r
}

/// Where slot `slot`'s caption's baseline is centred, when captions show.
pub fn caption_at(s: &PrintSettings, slot: usize) -> Option<(f32, f32)> {
    captions_shown(s).then(|| {
        let r = whole_cell(s, slot);
        let band = CAPTION_BAND.min(r.h * 0.5);
        (
            r.x + r.w / 2.0,
            r.y + (band - CAPTION_PT) / 2.0 + CAPTION_PT * 0.2,
        )
    })
}

/// A photo placed in its cell: the image's rectangle as it lies on the page (turned when `rotated`) and the
/// clip — the cell for a Fill, the image itself for a Fit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placed {
    pub image: Rect,
    pub rotated: bool,
    pub clip: Rect,
}

/// A photo of `px` pixels placed in `cell`, covering it when `fill`, turned when `rotate` and its
/// orientation is the cell's other one.
///
/// ⚠ Matching the cell's orientation is always the larger fit and the smaller crop, so "turn when it makes
/// the photo larger" needs no comparison.
pub fn place(px: (usize, usize), cell: Rect, fill: bool, rotate: bool) -> Placed {
    let (w, h) = (px.0.max(1) as f32, px.1.max(1) as f32);
    let rotated = rotate && w != h && cell.w != cell.h && (w > h) != (cell.w > cell.h);
    let (ew, eh) = if rotated { (h, w) } else { (w, h) };
    let (sx, sy) = (cell.w / ew, cell.h / eh);
    let k = if fill { sx.max(sy) } else { sx.min(sy) };
    let (iw, ih) = (ew * k, eh * k);
    let image = Rect {
        x: cell.x + (cell.w - iw) / 2.0,
        y: cell.y + (cell.h - ih) / 2.0,
        w: iw,
        h: ih,
    };
    Placed {
        image,
        rotated,
        clip: if fill { cell } else { image },
    }
}

/// The size every photo is rendered at: its box in centimetres at the PPI, never enlarged (a photo too small
/// prints at a lower PPI, said in the outcome).
pub fn export_size(s: &PrintSettings) -> OutputSize {
    let b = cell(s, 0);
    let cm = |pt: f32| pt / PT_PER_MM / 10.0;
    let fill = s.layout == Layout::Single && s.fit == Fit::Fill;
    let fit = match (fill, s.rotate) {
        (false, false) => FitMode::WidthHeight,
        (false, true) => FitMode::Dimensions,
        (true, false) => FitMode::Cover,
        (true, true) => FitMode::CoverEither,
    };
    OutputSize {
        fit,
        unit: LengthUnit::Cm,
        width: cm(b.w),
        height: cm(b.h),
        ppi: s.ppi,
        dont_enlarge: true,
        ..OutputSize::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sheet(c: u8, r: u8) -> PrintSettings {
        PrintSettings {
            layout: Layout::ContactSheet,
            columns: c,
            rows: r,
            ..PrintSettings::default()
        }
    }

    #[test]
    fn a4_portrait_and_landscape_in_points() {
        let s = PrintSettings::default();
        let (w, h) = page_size_pt(&s);
        assert!(
            (w - 595.3).abs() < 0.1 && (h - 841.9).abs() < 0.1,
            "{w} × {h}"
        );
        let l = PrintSettings {
            orientation: Orientation::Landscape,
            ..s
        };
        assert_eq!(page_size_pt(&l), (h, w));
    }

    #[test]
    fn twenty_one_photos_on_a_four_by_five_sheet_make_two_pages() {
        let p = pages(21, &sheet(4, 5));
        assert_eq!(p.iter().map(Vec::len).collect::<Vec<_>>(), vec![20, 1]);
        assert_eq!(p[1], vec![20]);
        assert_eq!(pages(3, &PrintSettings::default()).len(), 3);
    }

    #[test]
    fn a_sheets_cells_sit_inside_the_margins_without_overlapping() {
        let s = sheet(3, 2);
        let (pw, ph) = page_size_pt(&s);
        let m = 10.0 * PT_PER_MM;
        let cells: Vec<Rect> = (0..6).map(|i| cell(&s, i)).collect();
        for c in &cells {
            assert!(c.x >= m - 0.01 && c.y >= m - 0.01, "{c:?}");
            assert!(
                c.x + c.w <= pw - m + 0.01 && c.y + c.h <= ph - m + 0.01,
                "{c:?}"
            );
        }
        // Slot 0 top-left, slot 1 to its right, slot 3 below it.
        assert!(cells[1].x > cells[0].x && (cells[1].y - cells[0].y).abs() < 0.01);
        assert!(cells[3].y < cells[0].y && (cells[3].x - cells[0].x).abs() < 0.01);
        assert!(cells[0].x + cells[0].w < cells[1].x, "a gap between cells");
        // The caption sits under its photo, inside the cell's band.
        let (cx, cy) = caption_at(&s, 0).unwrap();
        assert!((cx - (cells[0].x + cells[0].w / 2.0)).abs() < 0.01);
        assert!(cy < cells[0].y && cy > cells[0].y - CAPTION_BAND);
        assert_eq!(caption_at(&PrintSettings::default(), 0), None);
    }

    #[test]
    fn a_landscape_in_a_portrait_cell_turns_when_rotate_is_on_and_grows() {
        let c = Rect {
            x: 0.0,
            y: 0.0,
            w: 200.0,
            h: 300.0,
        };
        let turned = place((3000, 2000), c, false, true);
        let upright = place((3000, 2000), c, false, false);
        assert!(turned.rotated && !upright.rotated);
        assert!(turned.image.w * turned.image.h > upright.image.w * upright.image.h * 1.5);
        assert_eq!((turned.image.w, turned.image.h), (200.0, 300.0));
        assert_eq!(turned.clip, turned.image);
        // A square photo never turns.
        assert!(!place((1000, 1000), c, false, true).rotated);
    }

    #[test]
    fn fill_covers_the_cell_and_clips_to_it() {
        let c = Rect {
            x: 10.0,
            y: 20.0,
            w: 200.0,
            h: 300.0,
        };
        for (px, rotate) in [
            ((3000, 2000), false),
            ((3000, 2000), true),
            ((2000, 3000), false),
        ] {
            let p = place(px, c, true, rotate);
            assert_eq!(p.clip, c);
            assert!(p.image.x <= c.x + 0.01 && p.image.y <= c.y + 0.01, "{p:?}");
            assert!(p.image.x + p.image.w >= c.x + c.w - 0.01, "{p:?}");
            assert!(p.image.y + p.image.h >= c.y + c.h - 0.01, "{p:?}");
        }
    }

    #[test]
    fn the_export_size_is_the_photo_box_in_cm_at_the_ppi() {
        let s = PrintSettings::default();
        let o = export_size(&s);
        // A4 less 10 mm each side: 19 × 27.7 cm.
        assert!(
            (o.width - 19.0).abs() < 0.01 && (o.height - 27.7).abs() < 0.01,
            "{o:?}"
        );
        assert_eq!(
            (o.unit, o.ppi, o.fit),
            (LengthUnit::Cm, 300, FitMode::Dimensions)
        );
        assert!(o.dont_enlarge);
        let fill = PrintSettings {
            fit: Fit::Fill,
            rotate: false,
            ..s.clone()
        };
        assert_eq!(export_size(&fill).fit, FitMode::Cover);
        // A contact sheet always fits, whatever Fill says.
        let sh = PrintSettings {
            fit: Fit::Fill,
            ..sheet(4, 5)
        };
        assert_eq!(export_size(&sh).fit, FitMode::Dimensions);
        assert!(export_size(&sh).width < 5.0);
    }

    #[test]
    fn an_older_settings_file_loads_with_the_defaults() {
        // Constructed: a file from before `rows` and `ppi` existed.
        let s: PrintSettings = serde_json::from_str(r#"{"columns": 3}"#).unwrap();
        assert_eq!((s.columns, s.rows, s.ppi, s.copies), (3, 5, 300, 1));
    }
}
