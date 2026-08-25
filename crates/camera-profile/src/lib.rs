//! Adobe camera profiles (`.dcp`).
//!
//! ⚠ A DCP is a TIFF file with DNG tags and a different magic number, so this is a small IFD
//! reader rather than a dependency. It deliberately knows nothing about libraw or about our
//! colour code: it turns bytes into numbers, which is the one part of C2 testable in CI.
//!
//! ⚠ `CameraCalibration` is **not** parsed or applied — the X-T4 profile has none, and applying
//! it is out of scope for C2. A profile that carries one renders as if it were the identity.
//! Recorded rather than silently mis-rendered.

use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    #[error("not a camera profile")]
    NotAProfile,
    #[error("truncated at offset {0}")]
    Truncated(usize),
    #[error("missing required tag {0}")]
    MissingTag(u16),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// The matrices from a `.dcp`, and nothing else.
#[derive(Debug, Clone, PartialEq)]
pub struct CameraProfile {
    pub name: Option<String>,
    /// EXIF light-source codes. 17 = Standard A (2856 K), 21 = D65, 23 = D50.
    pub illuminant1: u16,
    pub illuminant2: u16,
    /// XYZ → camera, for each illuminant.
    pub color1: [[f32; 3]; 3],
    pub color2: [[f32; 3]; 3],
    /// Camera → XYZ (D50). Optional: older profiles carry only ColorMatrices.
    pub forward1: Option<[[f32; 3]; 3]>,
    pub forward2: Option<[[f32; 3]; 3]>,
}

const COLOR_MATRIX_1: u16 = 50721;
const COLOR_MATRIX_2: u16 = 50722;
const ILLUMINANT_1: u16 = 50778;
const ILLUMINANT_2: u16 = 50779;
const PROFILE_NAME: u16 = 50936;
const FORWARD_MATRIX_1: u16 = 50964;
const FORWARD_MATRIX_2: u16 = 50965;

impl CameraProfile {
    pub fn from_file(path: &Path) -> Result<Self, ProfileError> {
        Self::parse(&std::fs::read(path)?)
    }

    pub fn parse(d: &[u8]) -> Result<Self, ProfileError> {
        if d.len() < 8 {
            return Err(ProfileError::NotAProfile);
        }
        // ⚠ A DCP's magic is 0x4352 where a TIFF's is 42. Both byte orders are accepted because
        // the format allows both, even though every profile seen in practice is little-endian.
        let le = match &d[..2] {
            b"II" => true,
            b"MM" => false,
            _ => return Err(ProfileError::NotAProfile),
        };
        let u16_at = |o: usize| -> Result<u16, ProfileError> {
            let b = d.get(o..o + 2).ok_or(ProfileError::Truncated(o))?;
            Ok(if le {
                u16::from_le_bytes([b[0], b[1]])
            } else {
                u16::from_be_bytes([b[0], b[1]])
            })
        };
        let u32_at = |o: usize| -> Result<u32, ProfileError> {
            let b = d.get(o..o + 4).ok_or(ProfileError::Truncated(o))?;
            Ok(if le {
                u32::from_le_bytes([b[0], b[1], b[2], b[3]])
            } else {
                u32::from_be_bytes([b[0], b[1], b[2], b[3]])
            })
        };

        let magic = u16_at(2)?;
        if magic != 42 && magic != 0x4352 {
            return Err(ProfileError::NotAProfile);
        }
        let ifd = u32_at(4)? as usize;
        let count = u16_at(ifd)? as usize;

        let mut name = None;
        let (mut ill1, mut ill2) = (0u16, 0u16);
        let mut mats: [Option<[[f32; 3]; 3]>; 4] = [None; 4];

        for i in 0..count {
            let e = ifd + 2 + i * 12;
            let tag = u16_at(e)?;
            let typ = u16_at(e + 2)?;
            let cnt = u32_at(e + 4)? as usize;
            let elem = match typ {
                1 | 2 => 1,
                3 => 2,
                4 | 11 => 4,
                5 | 10 | 12 => 8,
                _ => 1,
            };
            let size = elem * cnt;
            let payload = if size > 4 {
                u32_at(e + 8)? as usize
            } else {
                e + 8
            };

            match tag {
                ILLUMINANT_1 => ill1 = u16_at(payload)?,
                ILLUMINANT_2 => ill2 = u16_at(payload)?,
                PROFILE_NAME if typ == 2 && cnt > 1 => {
                    let bytes = d
                        .get(payload..payload + cnt - 1)
                        .ok_or(ProfileError::Truncated(payload))?;
                    name = Some(String::from_utf8_lossy(bytes).into_owned());
                }
                COLOR_MATRIX_1 | COLOR_MATRIX_2 | FORWARD_MATRIX_1 | FORWARD_MATRIX_2
                    if cnt == 9 =>
                {
                    let mut m = [[0.0f32; 3]; 3];
                    for (r, row) in m.iter_mut().enumerate() {
                        for (c, v) in row.iter_mut().enumerate() {
                            let o = payload + (r * 3 + c) * 8;
                            let num = u32_at(o)? as i32 as f32;
                            let den = u32_at(o + 4)? as i32 as f32;
                            // A zero denominator is malformed; 0.0 keeps the parse total rather
                            // than producing an infinity that would poison the matrix.
                            *v = if den == 0.0 { 0.0 } else { num / den };
                        }
                    }
                    let slot = match tag {
                        COLOR_MATRIX_1 => 0,
                        COLOR_MATRIX_2 => 1,
                        FORWARD_MATRIX_1 => 2,
                        _ => 3,
                    };
                    mats[slot] = Some(m);
                }
                _ => {}
            }
        }

        Ok(Self {
            name,
            illuminant1: ill1,
            illuminant2: ill2,
            color1: mats[0].ok_or(ProfileError::MissingTag(COLOR_MATRIX_1))?,
            // ⚠ A single-illuminant profile is legal: ColorMatrix2 then repeats ColorMatrix1, so
            // the interpolation collapses to a constant rather than needing a special case.
            color2: mats[1]
                .or(mats[0])
                .ok_or(ProfileError::MissingTag(COLOR_MATRIX_2))?,
            forward1: mats[2],
            forward2: mats[3].or(mats[2]),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `DCP_FIXTURE=/abs/path/to.dcp` to exercise a real profile.
    ///
    /// ⚠ Gated because the file ships with RawTherapee and is not in this repo — CI has no
    /// profile, and vendoring Adobe's is the licensing question C4 owns.
    fn fixture() -> Option<std::path::PathBuf> {
        std::env::var("DCP_FIXTURE").ok().map(Into::into)
    }

    #[test]
    fn the_x_t4_profile_parses_to_its_known_values() {
        // ⚠ These numbers were extracted by an independent Python reader, not by the code under
        // test — the whole point of a known-value test. They are recorded in the spec too.
        let Some(path) = fixture() else {
            eprintln!("skipping: set DCP_FIXTURE=/abs/path/to.dcp");
            return;
        };
        let p = CameraProfile::from_file(&path).expect("parse");
        assert_eq!(p.name.as_deref(), Some("FUJIFILM X-T4"));
        assert_eq!(p.illuminant1, 17);
        assert_eq!(p.illuminant2, 23);
        let fm1 = p.forward1.expect("forward1");
        assert!((fm1[0][0] - 0.6666).abs() < 1e-4, "got {}", fm1[0][0]);
        assert!((fm1[2][2] - 0.7886).abs() < 1e-4, "got {}", fm1[2][2]);
        assert!(
            (p.color2[0][0] - 0.9798).abs() < 1e-4,
            "got {}",
            p.color2[0][0]
        );
    }

    #[test]
    fn the_forward_matrix_rows_sum_to_the_d50_white_point() {
        // ⚠⚠ The DNG specification's own constraint, and the strongest self-check available: a
        // parser that transposes the matrix or mis-reads the rationals fails it. D50 is
        // (0.9642, 1.0, 0.8249).
        let Some(path) = fixture() else {
            eprintln!("skipping: set DCP_FIXTURE=/abs/path/to.dcp");
            return;
        };
        let p = CameraProfile::from_file(&path).expect("parse");
        for m in [p.forward1, p.forward2].into_iter().flatten() {
            let want = [0.9642, 1.0, 0.8249];
            for (i, row) in m.iter().enumerate() {
                let sum: f32 = row.iter().sum();
                assert!(
                    (sum - want[i]).abs() < 2e-3,
                    "forward row {i} sums to {sum}, want {}",
                    want[i]
                );
            }
        }
    }

    #[test]
    fn a_file_that_is_not_a_profile_is_refused() {
        assert!(CameraProfile::parse(b"not a dcp at all").is_err());
        assert!(CameraProfile::parse(&[]).is_err());
    }

    #[test]
    fn a_valid_byte_order_with_the_wrong_magic_is_refused() {
        // ⚠ The discriminating case, and it was missing: `b"not a dcp"` is rejected by the
        // *byte-order* check before magic is ever looked at, so removing the magic check left
        // every test passing. A file that genuinely starts "II" — any little-endian TIFF-ish
        // thing that is not a profile — is what this catches.
        let mut d = Vec::from(b"II");
        d.extend_from_slice(&99u16.to_le_bytes()); // neither 42 nor 0x4352
        d.extend_from_slice(&8u32.to_le_bytes());
        d.extend_from_slice(&0u16.to_le_bytes());
        assert!(matches!(
            CameraProfile::parse(&d),
            Err(ProfileError::NotAProfile)
        ));
    }

    #[test]
    fn a_plain_tiff_magic_is_accepted_but_a_profile_without_matrices_is_not() {
        // ⚠ Magic 42 is allowed — the format permits it — so refusal has to come from the
        // *content*: no ColorMatrix1 means this is not a camera profile whatever its header says.
        let mut d = Vec::from(b"II");
        d.extend_from_slice(&42u16.to_le_bytes());
        d.extend_from_slice(&8u32.to_le_bytes());
        d.extend_from_slice(&0u16.to_le_bytes()); // zero IFD entries
        assert!(matches!(
            CameraProfile::parse(&d),
            Err(ProfileError::MissingTag(_))
        ));
    }
}
