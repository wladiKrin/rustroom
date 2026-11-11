//! A Fuji's maker note: the film simulation a photo was shot with, and whether its JPEG settings were the
//! defaults a look can be learned from.
//!
//! ⚠ The note is the camera's own IFD — `FUJIFILM`, then a little-endian offset to it, every offset inside it
//! relative to the note's start. Read from the RAF's embedded JPEG's EXIF (`image_decode::maker_note`).

use std::path::Path;

/// What the camera rendered a photo's JPEG with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JpegLook {
    /// The film simulation's name, as Fuji's menus write it — the learned look's key, with the body.
    pub film: String,
    /// Shot at the JPEG settings a look is learned from: dynamic range 100 %, highlight and shadow tone 0,
    /// colour 0. ⚠ Any of those shifts the JPEG away from the simulation itself.
    pub plain: bool,
}

const FILM_MODE: u16 = 0x1401;
/// Fuji's "Saturation" — the colour setting, and the monochrome simulations' name.
const SATURATION: u16 = 0x1003;
const DEVELOPMENT_DR: u16 = 0x1403;
const SHADOW_TONE: u16 = 0x1040;
const HIGHLIGHT_TONE: u16 = 0x1041;

/// The colour simulations by their `FilmMode` code (exiftool's Fuji table).
fn colour_film(code: u32) -> Option<&'static str> {
    Some(match code {
        0x000 => "Provia",
        0x100 => "Studio Portrait",
        0x110 => "Studio Portrait Enhanced Saturation",
        0x120 => "Astia",
        0x130 => "Studio Portrait Increased Sharpness",
        0x200 | 0x400 => "Velvia",
        0x300 => "Studio Portrait Ex",
        0x500 => "Pro Neg. Std",
        0x501 => "Pro Neg. Hi",
        0x600 => "Classic Chrome",
        0x700 => "Eterna",
        0x800 => "Classic Neg.",
        0x900 => "Eterna Bleach Bypass",
        0xa00 => "Nostalgic Neg.",
        0xb00 => "Reala Ace",
        _ => return None,
    })
}

/// The monochrome simulations, which Fuji records under `Saturation` rather than `FilmMode`.
fn mono_film(code: u32) -> Option<&'static str> {
    Some(match code {
        0x300 => "Monochrome",
        0x301 => "Monochrome + R",
        0x302 => "Monochrome + Ye",
        0x303 => "Monochrome + G",
        0x310 => "Sepia",
        0x500 => "Acros",
        0x501 => "Acros + R",
        0x502 => "Acros + Ye",
        0x503 => "Acros + G",
        _ => return None,
    })
}

/// The look a Fuji file's JPEG was rendered with — `None` for another make, a file without a maker note, or
/// a simulation this table does not name.
pub fn read(path: &Path) -> Option<JpegLook> {
    parse(&image_decode::maker_note(path).ok()??)
}

/// [`read`] on the maker note's bytes.
pub fn parse(note: &[u8]) -> Option<JpegLook> {
    let tags = entries(note)?;
    let get = |t: u16| tags.iter().find(|(k, _)| *k == t).map(|(_, v)| *v);
    let sat = get(SATURATION).unwrap_or(0);
    let film = match mono_film(sat) {
        Some(m) => m,
        None => colour_film(get(FILM_MODE)?)?,
    };
    let mono = mono_film(sat).is_some();
    let plain = get(DEVELOPMENT_DR).is_none_or(|v| v == 100)
        && get(SHADOW_TONE).is_none_or(|v| v == 0)
        && get(HIGHLIGHT_TONE).is_none_or(|v| v == 0)
        && (mono || sat == 0);
    Some(JpegLook {
        film: film.to_owned(),
        plain,
    })
}

/// The note's IFD: each tag with its first value, for the short, long and signed-long types only.
fn entries(note: &[u8]) -> Option<Vec<(u16, u32)>> {
    if !note.starts_with(b"FUJIFILM") {
        return None;
    }
    let u16_at = |i: usize| Some(u16::from_le_bytes(note.get(i..i + 2)?.try_into().ok()?));
    let u32_at = |i: usize| Some(u32::from_le_bytes(note.get(i..i + 4)?.try_into().ok()?));
    let ifd = u32_at(8)? as usize;
    let n = u16_at(ifd)? as usize;
    let mut out = Vec::with_capacity(n);
    for k in 0..n {
        let at = ifd + 2 + 12 * k;
        let (tag, typ, count) = (u16_at(at)?, u16_at(at + 2)?, u32_at(at + 4)?);
        let v = match (typ, count) {
            // SHORT, inline.
            (3, 1) => u32::from(u16_at(at + 8)?),
            // LONG / SLONG, inline.
            (4 | 9, 1) => u32_at(at + 8)?,
            _ => continue,
        };
        out.push((tag, v));
    }
    Some(out)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A constructed maker note holding `tags` as SHORTs (types and layout as an X-T4 writes them).
    pub(crate) fn note(tags: &[(u16, u16)]) -> Vec<u8> {
        let mut b = b"FUJIFILM".to_vec();
        b.extend_from_slice(&12u32.to_le_bytes());
        b.extend_from_slice(&(tags.len() as u16).to_le_bytes());
        for (t, v) in tags {
            b.extend_from_slice(&t.to_le_bytes());
            b.extend_from_slice(&3u16.to_le_bytes());
            b.extend_from_slice(&1u32.to_le_bytes());
            b.extend_from_slice(&u32::from(*v).to_le_bytes());
        }
        b.extend_from_slice(&0u32.to_le_bytes());
        b
    }

    #[test]
    fn a_colour_simulation_at_default_settings_is_plain() {
        let l = parse(&note(&[
            (FILM_MODE, 0x500),
            (SATURATION, 0),
            (DEVELOPMENT_DR, 100),
        ]))
        .unwrap();
        assert_eq!(
            l,
            JpegLook {
                film: "Pro Neg. Std".into(),
                plain: true
            }
        );
        assert_eq!(
            parse(&note(&[(FILM_MODE, 0x600)])).unwrap().film,
            "Classic Chrome"
        );
    }

    #[test]
    fn a_tone_colour_or_range_setting_is_not_plain() {
        for extra in [
            (DEVELOPMENT_DR, 200),
            (HIGHLIGHT_TONE, 16),
            (SHADOW_TONE, 0xfff0),
            (SATURATION, 0x80),
        ] {
            let l = parse(&note(&[(FILM_MODE, 0x600), extra])).unwrap();
            assert!(!l.plain, "{extra:?}");
        }
    }

    #[test]
    fn monochrome_is_named_by_its_saturation_code() {
        let l = parse(&note(&[(FILM_MODE, 0x000), (SATURATION, 0x501)])).unwrap();
        assert_eq!(
            l,
            JpegLook {
                film: "Acros + R".into(),
                plain: true
            }
        );
    }

    #[test]
    fn another_make_or_an_unknown_film_is_none() {
        assert!(parse(b"Nikon\0\x02\x10").is_none());
        assert!(parse(&note(&[(FILM_MODE, 0x7777)])).is_none());
        assert!(
            parse(&note(&[])).is_none(),
            "no film mode, no monochrome: nothing to name"
        );
    }
}
