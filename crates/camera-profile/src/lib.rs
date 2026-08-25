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

/// A DNG hue/saturation/value mapping table.
///
/// ⚠ This is the **visible** half of a camera profile: the X-T4's table carries a mean absolute
/// hue shift of 11.3° and saturation scales up to 1.95, where its matrices differ from libraw's
/// by well under a percent.
#[derive(Debug, Clone, PartialEq)]
pub struct HueSatMap {
    /// `[hue, saturation, value]` divisions. ⚠ Value is often 1, making the table 2-D.
    pub dims: [usize; 3],
    /// `[hue shift in degrees, saturation scale, value scale]` per entry, hue-major.
    pub data1: Vec<[f32; 3]>,
    /// The second illuminant's table, when the profile is dual-illuminant.
    pub data2: Option<Vec<[f32; 3]>>,
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
    /// The hue/saturation mapping, when the profile has one.
    pub hue_sat_map: Option<HueSatMap>,
}

const COLOR_MATRIX_1: u16 = 50721;
const COLOR_MATRIX_2: u16 = 50722;
const ILLUMINANT_1: u16 = 50778;
const ILLUMINANT_2: u16 = 50779;
const PROFILE_NAME: u16 = 50936;
const HUE_SAT_MAP_DIMS: u16 = 50937;
const HUE_SAT_MAP_DATA_1: u16 = 50938;
const HUE_SAT_MAP_DATA_2: u16 = 50939;
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
        let mut hsm_dims: Option<[usize; 3]> = None;
        let mut hsm: [Option<Vec<[f32; 3]>>; 2] = [None, None];

        // FLOAT (type 11) triples, as the HueSatMap tables are stored.
        let f32_at = |o: usize| -> Result<f32, ProfileError> {
            let b = d.get(o..o + 4).ok_or(ProfileError::Truncated(o))?;
            Ok(if le {
                f32::from_le_bytes([b[0], b[1], b[2], b[3]])
            } else {
                f32::from_be_bytes([b[0], b[1], b[2], b[3]])
            })
        };

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
                HUE_SAT_MAP_DIMS if cnt == 3 => {
                    let mut v = [0usize; 3];
                    for (i, slot) in v.iter_mut().enumerate() {
                        *slot = u32_at(payload + i * 4)? as usize;
                    }
                    hsm_dims = Some(v);
                }
                HUE_SAT_MAP_DATA_1 | HUE_SAT_MAP_DATA_2 if cnt.is_multiple_of(3) => {
                    let entries = cnt / 3;
                    let mut v = Vec::with_capacity(entries);
                    for i in 0..entries {
                        let o = payload + i * 12;
                        v.push([f32_at(o)?, f32_at(o + 4)?, f32_at(o + 8)?]);
                    }
                    hsm[usize::from(tag == HUE_SAT_MAP_DATA_2)] = Some(v);
                }
                _ => {}
            }
        }

        // ⚠ A table whose length disagrees with its dims is **dropped**, not indexed into. A
        // short table would read past its end or silently mis-map every colour; rendering
        // without the map is the safe degradation, and the matrices still apply.
        let hue_sat_map = match (hsm_dims, hsm[0].take()) {
            (Some(dims), Some(data1)) if dims[0] * dims[1] * dims[2] == data1.len() => {
                let data2 = hsm[1].take().filter(|d| d.len() == data1.len());
                Some(HueSatMap { dims, data1, data2 })
            }
            _ => None,
        };

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
            hue_sat_map,
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

    #[test]
    fn the_x_t4_hue_sat_map_parses_to_its_known_values() {
        // ⚠ Values from the independent Python reader, as with the matrices.
        let Some(path) = fixture() else {
            eprintln!("skipping: set DCP_FIXTURE=/abs/path/to.dcp");
            return;
        };
        let p = CameraProfile::from_file(&path).expect("parse");
        let m = p.hue_sat_map.as_ref().expect("hue sat map");
        assert_eq!(m.dims, [90, 30, 1]);
        assert_eq!(m.data1.len(), 90 * 30);
        assert_eq!(m.data2.as_ref().map(|d| d.len()), Some(90 * 30));

        // ⚠ Entry 0 is hue 0 / saturation 0, and it is the identity — a profile that shifted a
        // *neutral* would be malformed. A meaningful first assertion rather than merely a number
        // that happens to be there.
        assert_eq!(m.data1[0], [0.0, 1.0, 1.0]);
        let e1 = m.data1[1];
        assert!((e1[0] - -12.117_49).abs() < 1e-4, "hue shift {}", e1[0]);
        assert!((e1[1] - 1.070_87).abs() < 1e-5, "sat scale {}", e1[1]);

        let mid = m.data1[30 * 30 + 10];
        assert!((mid[0] - -19.264_47).abs() < 1e-4, "hue shift {}", mid[0]);
        assert!((mid[1] - 1.586_781).abs() < 1e-5, "sat scale {}", mid[1]);

        let d2 = m.data2.as_ref().unwrap();
        assert!(
            (d2[1][0] - -16.583_09).abs() < 1e-4,
            "illuminant 2's table should differ from illuminant 1's, got {}",
            d2[1][0]
        );
    }

    /// Build a minimal little-endian DCP carrying the given IFD entries.
    ///
    /// ⚠ Synthetic because real profiles are always self-consistent — the guards for malformed
    /// ones can only be reached by constructing one, and an unreachable guard is an untested one.
    fn synth(entries: &[(u16, u16, u32, Vec<u8>)]) -> Vec<u8> {
        let mut head = Vec::new();
        head.extend_from_slice(b"II");
        head.extend_from_slice(&0x4352u16.to_le_bytes());
        head.extend_from_slice(&8u32.to_le_bytes());
        let mut ifd = Vec::new();
        ifd.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        // Payloads follow the IFD and its terminating next-offset word.
        let mut payload_at = 8 + 2 + entries.len() * 12 + 4;
        let mut payloads = Vec::new();
        for (tag, typ, cnt, bytes) in entries {
            ifd.extend_from_slice(&tag.to_le_bytes());
            ifd.extend_from_slice(&typ.to_le_bytes());
            ifd.extend_from_slice(&cnt.to_le_bytes());
            if bytes.len() <= 4 {
                let mut inline = bytes.clone();
                inline.resize(4, 0);
                ifd.extend_from_slice(&inline);
            } else {
                ifd.extend_from_slice(&(payload_at as u32).to_le_bytes());
                payloads.extend_from_slice(bytes);
                payload_at += bytes.len();
            }
        }
        ifd.extend_from_slice(&0u32.to_le_bytes());
        head.extend_from_slice(&ifd);
        head.extend_from_slice(&payloads);
        head
    }

    fn matrix_bytes(v: f32) -> Vec<u8> {
        let mut b = Vec::new();
        for _ in 0..9 {
            b.extend_from_slice(&((v * 10000.0) as i32).to_le_bytes());
            b.extend_from_slice(&10000i32.to_le_bytes());
        }
        b
    }

    #[test]
    fn a_hue_sat_map_whose_length_disagrees_with_its_dims_is_dropped() {
        // ⚠⚠ The guard exists because a short table would read past its end or silently mis-map
        // every colour. Real profiles never trip it, so without a synthetic one the guard is
        // dead code that looks tested — removing it left every other test green.
        let dims: Vec<u8> = [4u32, 4, 1].iter().flat_map(|v| v.to_le_bytes()).collect();
        let short: Vec<u8> = (0..3 * 3)
            .map(|_| 0.0f32)
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let d = synth(&[
            (COLOR_MATRIX_1, 10, 9, matrix_bytes(1.0)),
            (HUE_SAT_MAP_DIMS, 4, 3, dims),
            (HUE_SAT_MAP_DATA_1, 11, 9, short),
        ]);
        let p = CameraProfile::parse(&d).expect("the matrices should still parse");
        assert!(
            p.hue_sat_map.is_none(),
            "a table of 3 entries must not be accepted for dims 4x4x1"
        );
    }

    #[test]
    fn a_consistent_synthetic_hue_sat_map_is_accepted() {
        // ⚠ The other half: the guard must not reject a *valid* table, or it would silently
        // disable every profile. Same builder, matching lengths.
        let dims: Vec<u8> = [2u32, 2, 1].iter().flat_map(|v| v.to_le_bytes()).collect();
        let data: Vec<u8> = (0..4)
            .flat_map(|_| [0.0f32, 1.0, 1.0])
            .flat_map(|v| v.to_le_bytes())
            .collect();
        let d = synth(&[
            (COLOR_MATRIX_1, 10, 9, matrix_bytes(1.0)),
            (HUE_SAT_MAP_DIMS, 4, 3, dims),
            (HUE_SAT_MAP_DATA_1, 11, 12, data),
        ]);
        let p = CameraProfile::parse(&d).expect("parse");
        let m = p.hue_sat_map.expect("a consistent table must be accepted");
        assert_eq!(m.dims, [2, 2, 1]);
        assert_eq!(m.data1.len(), 4);
    }
}
