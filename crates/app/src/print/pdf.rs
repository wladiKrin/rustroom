//! A print's PDF, written by hand — `immich::multipart`'s precedent: the format needed is small, and a crate
//! would bring far more than it.
//!
//! ⚠ Each photo is its export's JPEG embedded AS IS (`/DCTDecode`) — never decoded or re-encoded — tagged with
//! the sRGB profile the export wrote it in. A Fill's crop is a clipping path, so no pixel is touched here.
//! Captions are in the built-in Helvetica, WinAnsi-encoded; a character outside WinAnsi prints as `?`.

use super::layout::{Placed, CAPTION_PT};
use std::fmt::Write as _;

/// One photo on a page: its JPEG, its pixel size, and where it lies.
pub struct PageImage {
    pub jpeg: Vec<u8>,
    pub px: (usize, usize),
    pub placed: Placed,
}

/// A caption: its centre's `x`, its baseline's `y`, the widest it may be, and its text.
pub struct Caption {
    pub x: f32,
    pub y: f32,
    pub max_w: f32,
    pub text: String,
}

/// One page: its size in points, its photos and its captions.
pub struct PageContent {
    pub size: (f32, f32),
    pub images: Vec<PageImage>,
    pub captions: Vec<Caption>,
}

/// The fixed objects' numbers.
const CATALOG: usize = 1;
const PAGES: usize = 2;
const FONT: usize = 3;
const ICC: usize = 4;

/// The document, every page in order.
pub fn write(pages: &[PageContent]) -> Vec<u8> {
    let mut w = Writer::default();
    w.out.extend_from_slice(b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n");
    // Each page takes its own object, its content stream's, and one per image, numbered from 5.
    let mut next = ICC + 1;
    let mut page_ids = Vec::new();
    let mut layout = Vec::new();
    for p in pages {
        let page = next;
        let content = next + 1;
        let images: Vec<usize> = (0..p.images.len()).map(|i| next + 2 + i).collect();
        next += 2 + p.images.len();
        page_ids.push(page);
        layout.push((page, content, images));
    }
    w.object(
        CATALOG,
        format!("<< /Type /Catalog /Pages {PAGES} 0 R >>").as_bytes(),
    );
    let kids: Vec<String> = page_ids.iter().map(|id| format!("{id} 0 R")).collect();
    w.object(
        PAGES,
        format!(
            "<< /Type /Pages /Kids [{}] /Count {} >>",
            kids.join(" "),
            pages.len()
        )
        .as_bytes(),
    );
    w.object(
        FONT,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>",
    );
    w.stream(ICC, "/N 3", export::icc_for(color::OutputSpace::Srgb));
    for (p, (page, content, images)) in pages.iter().zip(&layout) {
        let xobjects: String = images
            .iter()
            .enumerate()
            .map(|(i, id)| format!("/Im{i} {id} 0 R "))
            .collect();
        w.object(
            *page,
            format!(
                "<< /Type /Page /Parent {PAGES} 0 R /MediaBox [0 0 {} {}] \
                 /Resources << /Font << /F1 {FONT} 0 R >> /XObject << {xobjects}>> >> \
                 /Contents {content} 0 R >>",
                num(p.size.0),
                num(p.size.1)
            )
            .as_bytes(),
        );
        w.stream(*content, "", &content_stream(p));
        for (img, id) in p.images.iter().zip(images) {
            w.stream(
                *id,
                &format!(
                    "/Type /XObject /Subtype /Image /Width {} /Height {} \
                     /ColorSpace [/ICCBased {ICC} 0 R] /BitsPerComponent 8 /Filter /DCTDecode",
                    img.px.0, img.px.1
                ),
                &img.jpeg,
            );
        }
    }
    w.finish(next)
}

/// A page's drawing: each photo clipped and placed, then each caption.
fn content_stream(p: &PageContent) -> Vec<u8> {
    let mut s = String::new();
    for (i, img) in p.images.iter().enumerate() {
        let (r, c) = (img.placed.image, img.placed.clip);
        // ⚠ An image is drawn into the unit square, so the matrix scales it to its rectangle; a turned one
        // runs its width up the rectangle's height.
        let m = if img.placed.rotated {
            [0.0, r.h, -r.w, 0.0, r.x + r.w, r.y]
        } else {
            [r.w, 0.0, 0.0, r.h, r.x, r.y]
        };
        let _ = writeln!(
            s,
            "q {} {} {} {} re W n {} {} {} {} {} {} cm /Im{i} Do Q",
            num(c.x),
            num(c.y),
            num(c.w),
            num(c.h),
            num(m[0]),
            num(m[1]),
            num(m[2]),
            num(m[3]),
            num(m[4]),
            num(m[5])
        );
    }
    let mut out = s.into_bytes();
    for c in &p.captions {
        let bytes = fit_caption(&win_ansi(&c.text), c.max_w);
        let x = c.x - width_pt(&bytes) / 2.0;
        out.extend_from_slice(
            format!("BT /F1 {} Tf {} {} Td (", num(CAPTION_PT), num(x), num(c.y)).as_bytes(),
        );
        out.extend_from_slice(&escape(&bytes));
        out.extend_from_slice(b") Tj ET\n");
    }
    out
}

/// A number as PDF writes it: two decimals, no trailing zeros.
fn num(v: f32) -> String {
    let s = format!("{v:.2}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" {
        "0".into()
    } else {
        s.to_string()
    }
}

/// `text` in WinAnsi, `?` for anything it cannot hold.
fn win_ansi(text: &str) -> Vec<u8> {
    text.chars()
        .map(|ch| match ch {
            ' '..='~' => ch as u8,
            // ⚠ WinAnsi is Latin-1 from 0xA0 up.
            '\u{a0}'..='\u{ff}' => ch as u32 as u8,
            '€' => 0x80,
            '…' => 0x85,
            '‘' => 0x91,
            '’' => 0x92,
            '“' => 0x93,
            '”' => 0x94,
            '–' => 0x96,
            '—' => 0x97,
            _ => b'?',
        })
        .collect()
}

/// A PDF string's body: `\`, `(` and `)` escaped, every byte past ASCII as an octal escape.
fn escape(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    for &b in bytes {
        match b {
            b'\\' | b'(' | b')' => out.extend_from_slice(&[b'\\', b]),
            0x20..=0x7e => out.push(b),
            _ => out.extend_from_slice(format!("\\{b:03o}").as_bytes()),
        }
    }
    out
}

/// Helvetica's advance widths for 0x20–0x7E, in thousandths of an em (its AFM).
const HELVETICA: [u16; 95] = [
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, 556, 556, 556,
    556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, 1015, 667, 667, 722, 722, 667,
    611, 778, 722, 278, 500, 667, 556, 833, 722, 778, 667, 778, 722, 667, 611, 722, 667, 944, 667,
    667, 611, 278, 278, 278, 469, 556, 333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500,
    222, 833, 556, 556, 556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584,
];

/// A WinAnsi string's width at the caption's size, in points. ⚠ A byte past ASCII is taken as 556, an
/// average letter: close enough to centre a caption.
fn width_pt(bytes: &[u8]) -> f32 {
    let em: u32 = bytes
        .iter()
        .map(|&b| match b {
            0x20..=0x7e => u32::from(HELVETICA[usize::from(b - 0x20)]),
            _ => 556,
        })
        .sum();
    em as f32 / 1000.0 * CAPTION_PT
}

/// `bytes` shortened with `...` until it is at most `max_w` wide.
fn fit_caption(bytes: &[u8], max_w: f32) -> Vec<u8> {
    if width_pt(bytes) <= max_w {
        return bytes.to_vec();
    }
    let mut cut = bytes.to_vec();
    while !cut.is_empty() {
        cut.pop();
        let mut t = cut.clone();
        t.extend_from_slice(b"...");
        if width_pt(&t) <= max_w {
            return t;
        }
    }
    Vec::new()
}

/// The bytes written so far and where each object starts.
#[derive(Default)]
struct Writer {
    out: Vec<u8>,
    offsets: Vec<(usize, usize)>,
}

impl Writer {
    fn object(&mut self, id: usize, body: &[u8]) {
        self.offsets.push((id, self.out.len()));
        self.out
            .extend_from_slice(format!("{id} 0 obj\n").as_bytes());
        self.out.extend_from_slice(body);
        self.out.extend_from_slice(b"\nendobj\n");
    }

    fn stream(&mut self, id: usize, dict: &str, data: &[u8]) {
        let mut body = format!("<< {dict} /Length {} >>\nstream\n", data.len()).into_bytes();
        body.extend_from_slice(data);
        body.extend_from_slice(b"\nendstream");
        self.object(id, &body);
    }

    /// The cross-reference table and trailer; `size` is one past the last object's number.
    fn finish(mut self, size: usize) -> Vec<u8> {
        let xref = self.out.len();
        let mut at = vec![0usize; size];
        for (id, off) in &self.offsets {
            at[*id] = *off;
        }
        let mut t = format!("xref\n0 {size}\n0000000000 65535 f \n");
        for off in &at[1..] {
            let _ = writeln!(t, "{off:010} 00000 n ");
        }
        let _ = write!(
            t,
            "trailer\n<< /Size {size} /Root {CATALOG} 0 R >>\nstartxref\n{xref}\n%%EOF\n"
        );
        self.out.extend_from_slice(t.as_bytes());
        self.out
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::print::layout::{place, Rect};

    /// A constructed JPEG of `w` × `h` pixels.
    pub(crate) fn jpeg(w: u32, h: u32) -> Vec<u8> {
        let img =
            image::RgbImage::from_fn(w, h, |x, y| image::Rgb([(x * 7) as u8, (y * 5) as u8, 90]));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Jpeg).unwrap();
        out.into_inner()
    }

    fn page(rotate: bool, captions: &[&str]) -> PageContent {
        let cell = Rect {
            x: 50.0,
            y: 50.0,
            w: 200.0,
            h: 300.0,
        };
        PageContent {
            size: (595.0, 842.0),
            images: vec![PageImage {
                jpeg: jpeg(60, 40),
                px: (60, 40),
                placed: place((60, 40), cell, false, rotate),
            }],
            captions: captions
                .iter()
                .map(|t| Caption {
                    x: 150.0,
                    y: 40.0,
                    max_w: 200.0,
                    text: (*t).to_string(),
                })
                .collect(),
        }
    }

    fn text(pdf: &[u8]) -> String {
        String::from_utf8_lossy(pdf).into_owned()
    }

    #[test]
    fn every_xref_offset_points_at_its_object() {
        let pdf = write(&[page(false, &["one.jpg"]), page(true, &[])]);
        let t = text(&pdf);
        // ⚠ Found in the BYTES: the lossy text's replacement characters shift every offset past an image.
        let xref = pdf
            .windows(6)
            .rposition(|w| w == b"\nxref\n".as_slice())
            .unwrap()
            + 1;
        let tail = String::from_utf8(pdf[xref..].to_vec()).unwrap();
        let start: usize = tail[tail.rfind("startxref\n").unwrap() + 10..]
            .lines()
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert_eq!(start, xref);
        let rows: Vec<&str> = tail
            .lines()
            .skip(3)
            .take_while(|l| l.ends_with(" n "))
            .collect();
        // Catalog, pages, font, ICC, then two pages of page + content + one image.
        assert_eq!(rows.len(), 4 + 2 * 3);
        for (i, row) in rows.iter().enumerate() {
            let off: usize = row[..10].parse().unwrap();
            let head = format!("{} 0 obj\n", i + 1);
            assert_eq!(
                &pdf[off..off + head.len()],
                head.as_bytes(),
                "object {}",
                i + 1
            );
        }
        assert!(t.contains("/Count 2"));
        assert!(t.contains("/Filter /DCTDecode"));
        assert!(t.ends_with("%%EOF\n"));
    }

    #[test]
    fn a_caption_is_escaped_and_what_winansi_lacks_is_a_question_mark() {
        let t = text(&write(&[page(false, &["a(b)ł\\é"])]));
        // ⚠ é is WinAnsi 0xE9, written as the octal escape \351.
        assert!(t.contains("(a\\(b\\)?\\\\\\351) Tj"), "{t}");
    }

    #[test]
    fn a_long_caption_is_cut_to_its_cell() {
        let long = "a_very_long_file_name_that_will_never_fit_under_a_small_cell.jpg";
        let cut = fit_caption(&win_ansi(long), 60.0);
        assert!(width_pt(&cut) <= 60.0 && cut.ends_with(b"..."));
        assert_eq!(fit_caption(b"x.jpg", 60.0), b"x.jpg");
    }

    #[test]
    fn a_turned_photo_is_drawn_through_a_turning_matrix() {
        let upright = text(&write(&[page(false, &[])]));
        let turned = text(&write(&[page(true, &[])]));
        let cm = |t: &str| {
            let l = t
                .lines()
                .find(|l| l.contains(" cm /Im0 Do"))
                .unwrap()
                .to_owned();
            let parts: Vec<String> = l.split(' ').map(str::to_owned).collect();
            let at = parts.iter().position(|p| p == "cm").unwrap();
            parts[at - 6..at].to_vec()
        };
        let (u, r) = (cm(&upright), cm(&turned));
        assert_eq!((u[1].as_str(), u[2].as_str()), ("0", "0"), "{u:?}");
        assert_eq!((r[0].as_str(), r[3].as_str()), ("0", "0"), "{r:?}");
        assert!(r[2].starts_with('-'), "{r:?}");
    }
}
