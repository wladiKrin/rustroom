//! A monitor's colour facts from its EDID — the block every display hands the computer over its cable, the source
//! of truth when the operating system has no profile for the monitor (the display's own colours, 2026-11-18).
//!
//! ⚠ These are the MANUFACTURER's nominal primaries, not a calibration: far closer to a wide-gamut panel than sRGB
//! is, never as good as a measured profile — which is why an assigned profile wins (`rustroom_app::display_detect`).
//! Pure: bytes in, numbers out. Only the 128-byte base block is read.

/// A display's chromaticities (CIE x, y) and its transfer curve as a single power.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Primaries {
    pub red: (f64, f64),
    pub green: (f64, f64),
    pub blue: (f64, f64),
    pub white: (f64, f64),
    /// 2.2 when the EDID does not say.
    pub gamma: f64,
}

impl Primaries {
    /// sRGB's primaries and D65 white (IEC 61966-2-1); the gamma is the EDID convention's 2.2.
    pub const SRGB: Primaries = Primaries {
        red: (0.64, 0.33),
        green: (0.30, 0.60),
        blue: (0.15, 0.06),
        white: (0.3127, 0.3290),
        gamma: 2.2,
    };

    /// Whether every primary and the white lie within `tol` of `other`'s, in x and in y. The gammas are not
    /// compared: an EDID's gamma is a rounded single power, and sRGB's curve is not one.
    pub fn is_close_to(&self, other: &Primaries, tol: f64) -> bool {
        let near =
            |a: (f64, f64), b: (f64, f64)| (a.0 - b.0).abs() <= tol && (a.1 - b.1).abs() <= tol;
        near(self.red, other.red)
            && near(self.green, other.green)
            && near(self.blue, other.blue)
            && near(self.white, other.white)
    }
}

/// What Rustroom reads from an EDID.
#[derive(Debug, Clone, PartialEq)]
pub struct Edid {
    /// The monitor's name descriptor, e.g. "M27Q".
    pub name: Option<String>,
    pub primaries: Primaries,
    /// "Standard sRGB is the default colour space": the monitor shows sRGB unless it is told otherwise.
    pub srgb_default: bool,
}

/// The base block's colour facts — `None` for anything that is not a valid EDID (header, length or checksum).
pub fn parse(bytes: &[u8]) -> Option<Edid> {
    let b = bytes.get(..128)?;
    if b[..8] != [0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x00] {
        return None;
    }
    if b.iter().fold(0u8, |s, &x| s.wrapping_add(x)) != 0 {
        return None;
    }
    // Ten bits each: eight high bits in bytes 27–34, the two low bits packed into bytes 25 and 26.
    let ten = |hi: u8, lo: u8, shift: u8| {
        (((hi as u16) << 2) | ((lo >> shift) as u16 & 3)) as f64 / 1024.0
    };
    let (l1, l2) = (b[25], b[26]);
    let primaries = Primaries {
        red: (ten(b[27], l1, 6), ten(b[28], l1, 4)),
        green: (ten(b[29], l1, 2), ten(b[30], l1, 0)),
        blue: (ten(b[31], l2, 6), ten(b[32], l2, 4)),
        white: (ten(b[33], l2, 2), ten(b[34], l2, 0)),
        // 0xFF: the gamma is given elsewhere (an extension block), which is not read.
        gamma: if b[23] == 0xFF {
            2.2
        } else {
            (b[23] as f64 + 100.0) / 100.0
        },
    };
    Some(Edid {
        name: name(b),
        primaries,
        srgb_default: b[24] & 0x04 != 0,
    })
}

/// The 0xFC (monitor name) display descriptor, of the four 18-byte descriptors at 54.
fn name(b: &[u8]) -> Option<String> {
    (0..4).map(|i| &b[54 + 18 * i..72 + 18 * i]).find_map(|d| {
        if d[0] != 0 || d[1] != 0 || d[3] != 0xFC {
            return None;
        }
        let text: String = d[5..]
            .iter()
            .take_while(|&&c| c != 0x0A)
            .map(|&c| c as char)
            .collect();
        let text = text.trim().to_string();
        (!text.is_empty()).then_some(text)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// CAPTURED from the author's Gigabyte M27Q (Windows' registry, 2026-11-17), its serial number blanked and the
    /// checksum redone — `testdata/README.md`.
    const M27Q: &[u8] = include_bytes!("../testdata/m27q.edid");

    #[test]
    fn the_m27q_reports_its_wide_primaries() {
        let e = parse(M27Q).expect("a valid EDID");
        assert_eq!(e.name.as_deref(), Some("M27Q"));
        let p = e.primaries;
        let near =
            |a: (f64, f64), b: (f64, f64)| (a.0 - b.0).abs() < 1e-4 && (a.1 - b.1).abs() < 1e-4;
        assert!(near(p.red, (0.6553, 0.3242)), "{:?}", p.red);
        assert!(near(p.green, (0.2090, 0.7139)), "{:?}", p.green);
        assert!(near(p.blue, (0.1465, 0.0508)), "{:?}", p.blue);
        assert!(near(p.white, (0.3135, 0.3291)), "{:?}", p.white);
        assert_eq!(p.gamma, 2.2);
        assert!(!e.srgb_default);
        assert!(
            !p.is_close_to(&Primaries::SRGB, 0.005),
            "a wide-gamut panel is not sRGB"
        );
    }

    #[test]
    fn a_corrupt_or_short_block_is_not_an_edid() {
        assert!(parse(M27Q).is_some(), "positive control");
        let mut bad = M27Q.to_vec();
        bad[30] ^= 1;
        assert_eq!(parse(&bad), None, "checksum");
        assert_eq!(parse(&M27Q[..100]), None, "length");
        let mut header = M27Q.to_vec();
        header[0] = 1;
        assert_eq!(parse(&header), None, "header");
    }

    /// A constructed sRGB monitor: the M27Q's block with sRGB's chromaticities and the "sRGB default" bit written in
    /// — proves the bit-packing round trip and the flag, which no captured block in hand exercises.
    #[test]
    fn an_srgb_monitor_reads_as_srgb() {
        let mut b = M27Q.to_vec();
        let enc = |v: f64| (v * 1024.0).round() as u16;
        let c = [
            Primaries::SRGB.red,
            Primaries::SRGB.green,
            Primaries::SRGB.blue,
            Primaries::SRGB.white,
        ];
        let xs: Vec<u16> = c.iter().flat_map(|&(x, y)| [enc(x), enc(y)]).collect();
        for (i, v) in xs.iter().enumerate() {
            b[27 + i] = (v >> 2) as u8;
        }
        b[25] = ((xs[0] & 3) << 6 | (xs[1] & 3) << 4 | (xs[2] & 3) << 2 | (xs[3] & 3)) as u8;
        b[26] = ((xs[4] & 3) << 6 | (xs[5] & 3) << 4 | (xs[6] & 3) << 2 | (xs[7] & 3)) as u8;
        b[24] |= 0x04;
        b[127] = 0;
        b[127] = 0u8.wrapping_sub(b.iter().fold(0u8, |s, &x| s.wrapping_add(x)));
        let e = parse(&b).expect("still valid");
        assert!(e.srgb_default);
        assert!(
            e.primaries.is_close_to(&Primaries::SRGB, 0.001),
            "{:?}",
            e.primaries
        );
    }
}
