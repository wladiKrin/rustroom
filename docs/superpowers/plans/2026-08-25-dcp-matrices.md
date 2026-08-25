# Read a DCP's matrices (C2) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Parse an Adobe `.dcp` camera profile and use its dual-illuminant `ForwardMatrix` for the camera→working-space transform, opt-in so nothing existing changes.

**Architecture:** A new dependency-free crate parses the DCP into a plain struct. `color::camera` interpolates between the two illuminants using the camera's own white balance and composes camera→XYZ(D50)→ProPhoto. `raw-decode` loads a profile only when told to, feeding the seam C1 already built.

**Tech Stack:** Rust, no new third-party dependencies (the DCP is a TIFF IFD we read ourselves).

**Spec:** `docs/superpowers/specs/2026-08-25-dcp-matrices-design.md`

**Before anything:**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
git checkout -b dcp-matrices
```

Fixtures used throughout:

```
DCP=/Applications/RawTherapee.app/Contents/Resources/share/dcpprofiles/FUJIFILM X-T4.dcp
RAW=~/Desktop/fuji/DSCF6509.RAF
```

⚠ Both are on this machine only. Every test touching them is **gated on an env var** and skips
otherwise, exactly as `RAW_FIXTURE` already does — CI has neither.

---

## File Structure

| file | responsibility |
|---|---|
| `crates/camera-profile/` | **create** — parse a `.dcp`; no libraw, no colour maths |
| `crates/color/src/camera.rs` | *modify* — illuminant interpolation, camera→ProPhoto from a profile |
| `crates/raw-decode/src/lib.rs` | *modify* — `DecodeOptions.camera_profile` |
| `crates/raw-decode/src/backend_rsraw.rs` | *modify* — use the profile's matrix when given |
| `crates/raw-decode/examples/dcp_compare.rs` | **create** — render both paths, report the difference |

---

### Task 1: parse the DCP

**Files:**
- Create: `crates/camera-profile/Cargo.toml`, `crates/camera-profile/src/lib.rs`
- Modify: root `Cargo.toml` (workspace members)

- [ ] **Step 1: Create the crate**

`crates/camera-profile/Cargo.toml`:

```toml
[package]
name = "camera-profile"
version = "0.1.0"
edition.workspace = true
license.workspace = true

[dependencies]
thiserror = { workspace = true }
```

Add `"crates/camera-profile"` to the workspace `members` list in the root `Cargo.toml`.

- [ ] **Step 2: Write the failing tests**

Create `crates/camera-profile/src/lib.rs` containing only this test module:

```rust
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
        let cm2 = p.color2;
        assert!((cm2[0][0] - 0.9798).abs() < 1e-4, "got {}", cm2[0][0]);
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
}
```

- [ ] **Step 3: Run to verify it fails**

Run: `cargo test -p camera-profile`
Expected: FAIL — `cannot find type CameraProfile`.

- [ ] **Step 4: Implement the parser**

Prepend to `crates/camera-profile/src/lib.rs`:

```rust
//! Adobe camera profiles (`.dcp`).
//!
//! ⚠ A DCP is a TIFF file with DNG tags and a different magic number, so this is a small IFD
//! reader rather than a dependency. It deliberately knows nothing about libraw or about our
//! colour code: it turns bytes into numbers, which is the one part of C2 testable in CI.
//!
//! ⚠ `CameraCalibration` is parsed but **not** returned — the X-T4 profile has none, and
//! applying it is out of scope for C2. A profile that carries one will render as if it were the
//! identity. Recorded rather than silently mis-rendered.

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
        let i32_at = |o: usize| -> Result<i32, ProfileError> { Ok(u32_at(o)? as i32) };

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
            let payload = if size > 4 { u32_at(e + 8)? as usize } else { e + 8 };

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
                            let num = i32_at(o)? as f32;
                            let den = i32_at(o + 4)? as f32;
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
            // the interpolation below collapses to a constant rather than needing a special case.
            color2: mats[1].or(mats[0]).ok_or(ProfileError::MissingTag(COLOR_MATRIX_2))?,
            forward1: mats[2],
            forward2: mats[3].or(mats[2]),
        })
    }
}
```

- [ ] **Step 5: Run the tests**

```bash
DCP_FIXTURE="/Applications/RawTherapee.app/Contents/Resources/share/dcpprofiles/FUJIFILM X-T4.dcp" cargo test -p camera-profile -- --nocapture
```

Expected: PASS, all three.

- [ ] **Step 6: Sabotage**

| sabotage | must fail |
|---|---|
| read the matrix transposed (`payload + (c * 3 + r) * 8`) | `the_forward_matrix_rows_sum_to_the_d50_white_point` |
| swap `FORWARD_MATRIX_1`/`FORWARD_MATRIX_2` slots | `the_x_t4_profile_parses_to_its_known_values` |
| accept any magic number | `a_file_that_is_not_a_profile_is_refused` |

Confirm each compiles first, then revert.

- [ ] **Step 7: Commit**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
git add -A
git commit -m "feat(camera-profile): parse a DCP's matrices"
```

⚠ Check clippy's **exit code**, not grep output.

---

### Task 2: interpolate the illuminants and compose

**Files:**
- Modify: `crates/color/src/camera.rs`
- Modify: `crates/color/Cargo.toml`

- [ ] **Step 1: Write the failing tests**

Add to `crates/color/src/camera.rs`'s test module:

```rust
    /// The X-T4 profile's real values, from the spec.
    const FM1: [[f32; 3]; 3] = [
        [0.6666, 0.1486, 0.1491],
        [0.2533, 0.7037, 0.0430],
        [0.0015, 0.0350, 0.7886],
    ];
    const FM2: [[f32; 3]; 3] = [
        [0.6140, 0.1279, 0.2224],
        [0.2698, 0.6609, 0.0693],
        [0.0002, 0.0160, 0.8088],
    ];

    #[test]
    fn the_blend_reaches_both_endpoints_exactly() {
        // ⚠ An interpolation that is wrong in the middle is often still right at the ends — but
        // one that is wrong at the ends is wrong everywhere. These pin the cheap half exactly;
        // the middle is validated against RawTherapee, which is a different kind of check.
        assert_eq!(blend(FM1, FM2, 1.0), FM1);
        assert_eq!(blend(FM1, FM2, 0.0), FM2);
    }

    #[test]
    fn the_blend_is_monotonic_and_stays_between_them() {
        for g in [0.0, 0.25, 0.5, 0.75, 1.0] {
            let m = blend(FM1, FM2, g);
            for i in 0..3 {
                for j in 0..3 {
                    let (lo, hi) = (FM1[i][j].min(FM2[i][j]), FM1[i][j].max(FM2[i][j]));
                    assert!(m[i][j] >= lo - 1e-6 && m[i][j] <= hi + 1e-6, "g={g} [{i}][{j}]");
                }
            }
        }
    }

    #[test]
    fn a_blended_forward_matrix_still_lands_on_d50_white() {
        // ⚠ The DNG constraint has to survive interpolation, not merely hold at the endpoints —
        // it does, because the constraint is linear in the matrix. If this fails, `blend` is not
        // a convex combination.
        for g in [0.0, 0.3, 0.5, 0.8, 1.0] {
            let m = blend(FM1, FM2, g);
            let want = [0.9642, 1.0, 0.8249];
            for (i, row) in m.iter().enumerate() {
                let sum: f32 = row.iter().sum();
                assert!((sum - want[i]).abs() < 2e-3, "g={g} row {i} sums to {sum}");
            }
        }
    }

    #[test]
    fn the_weight_follows_the_dng_reciprocal_rule() {
        // ⚠ DNG interpolates in **reciprocal** temperature (mireds), not linearly in kelvin.
        // Linear-in-kelvin is the obvious wrong implementation and is visibly off in the middle.
        // 2856 K and 5000 K: the midpoint in mireds is ~3635 K, not 3928 K.
        assert!((illuminant_weight(2856.0, 2856.0, 5000.0) - 1.0).abs() < 1e-6);
        assert!((illuminant_weight(5000.0, 2856.0, 5000.0) - 0.0).abs() < 1e-6);
        let mid = illuminant_weight(3635.0, 2856.0, 5000.0);
        assert!((mid - 0.5).abs() < 0.02, "mired midpoint gave {mid}");
        // Clamped outside the range rather than extrapolated.
        assert_eq!(illuminant_weight(2000.0, 2856.0, 5000.0), 1.0);
        assert_eq!(illuminant_weight(9000.0, 2856.0, 5000.0), 0.0);
    }

    #[test]
    fn a_neutral_camera_pixel_is_neutral_through_a_forward_matrix() {
        // ⚠ The point of a ForwardMatrix: its rows sum to D50 white, so a neutral camera reading
        // must land on D50 white in XYZ and therefore neutral in ProPhoto, which is a D50 space.
        let out = apply(&forward_to_prophoto(FM1), [1.0, 1.0, 1.0]);
        assert!(
            (out[0] - out[1]).abs() < 2e-3 && (out[1] - out[2]).abs() < 2e-3,
            "neutral in, {out:?} out"
        );
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p color camera`
Expected: FAIL — `blend`, `illuminant_weight`, `forward_to_prophoto` not found.

- [ ] **Step 3: Implement**

Add to `crates/color/src/camera.rs`, above the tests:

```rust
use crate::spaces::XYZ_D50_TO_PROPHOTO;

/// Correlated colour temperature, in kelvin, for an EXIF light-source code.
///
/// ⚠ Only the codes that appear in real profiles. An unknown code falls back to D65 rather than
/// failing: a slightly wrong illuminant renders a slightly wrong colour, where a hard error would
/// make a whole camera unusable.
pub fn illuminant_temperature(code: u16) -> f32 {
    match code {
        1 => 5500.0,  // Daylight
        2 => 4200.0,  // Fluorescent
        3 => 2856.0,  // Tungsten
        17 => 2856.0, // Standard A
        18 => 4874.0, // Standard B
        19 => 6774.0, // Standard C
        20 => 5503.0, // D55
        21 => 6504.0, // D65
        22 => 7504.0, // D75
        23 => 5003.0, // D50
        _ => 6504.0,
    }
}

/// The weight for illuminant 1, given a temperature and the two calibration temperatures.
///
/// ⚠⚠ **Interpolated in reciprocal temperature (mireds), not linearly in kelvin** — that is the
/// DNG specification's rule, and linear-in-kelvin is the obvious wrong implementation. It agrees
/// at the endpoints and is visibly off in the middle, which is the worst way to be wrong.
///
/// Clamped rather than extrapolated: outside the calibrated range the nearer matrix is the best
/// available answer, and extrapolating two measured matrices produces nonsense.
pub fn illuminant_weight(temp: f32, temp1: f32, temp2: f32) -> f32 {
    let (lo, hi) = if temp1 < temp2 { (temp1, temp2) } else { (temp2, temp1) };
    if temp <= lo {
        return if temp1 < temp2 { 1.0 } else { 0.0 };
    }
    if temp >= hi {
        return if temp1 < temp2 { 0.0 } else { 1.0 };
    }
    let g = (1.0 / temp - 1.0 / temp2) / (1.0 / temp1 - 1.0 / temp2);
    g.clamp(0.0, 1.0)
}

/// A convex combination of two matrices: `g` of `a`, `1 - g` of `b`.
pub fn blend(a: [[f32; 3]; 3], b: [[f32; 3]; 3], g: f32) -> [[f32; 3]; 3] {
    let mut out = [[0.0f32; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            out[i][j] = g * a[i][j] + (1.0 - g) * b[i][j];
        }
    }
    out
}

/// Compose a DCP `ForwardMatrix` (camera → XYZ D50) into camera → linear ProPhoto.
///
/// ⚠ **No chromatic adaptation here, and that is not an omission.** A ForwardMatrix outputs XYZ
/// at D50 and ProPhoto *is* a D50 space, so the white points already agree. Inserting a Bradford
/// step would be a second, wrong adaptation.
pub fn forward_to_prophoto(forward: [[f32; 3]; 3]) -> [[f32; 3]; 3] {
    let mut out = [[0.0f32; 3]; 3];
    for (i, row) in out.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = (0..3).map(|k| XYZ_D50_TO_PROPHOTO[i][k] * forward[k][j]).sum();
        }
    }
    out
}
```

⚠ `XYZ_D50_TO_PROPHOTO` does not exist yet. Add it to `crates/color/src/spaces.rs`, derived from
the ROMM primaries at D50 — **and verify it by the test above**, which requires D50 white to come
out neutral. ProPhoto's XYZ(D50) matrix is the inverse of its primaries matrix; the published
values are:

```rust
/// XYZ (D50) → linear ProPhoto (ROMM). ⚠ No adaptation: both are D50.
pub const XYZ_D50_TO_PROPHOTO: [[f32; 3]; 3] = [
    [1.3459433, -0.2556075, -0.0511118],
    [-0.5445989, 1.5081673, 0.0205351],
    [0.0000000, 0.0000000, 1.2118128],
];
```

- [ ] **Step 4: Run the tests**

Run: `cargo test -p color`
Expected: PASS.

⚠ If `a_neutral_camera_pixel_is_neutral_through_a_forward_matrix` fails, the XYZ→ProPhoto matrix
or the composition order is wrong — **not the tolerance**. Check by hand that
`XYZ_D50_TO_PROPHOTO × (0.9642, 1.0, 0.8249)` is close to `(1, 1, 1)`.

- [ ] **Step 5: Sabotage**

| sabotage | must fail |
|---|---|
| linear-in-kelvin weight | `the_weight_follows_the_dng_reciprocal_rule` |
| `blend` returns `a` always | `the_blend_reaches_both_endpoints_exactly` |
| transpose the composition | `a_neutral_camera_pixel_is_neutral_through_a_forward_matrix` |

- [ ] **Step 6: Commit**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
git add -A
git commit -m "feat(color): dual-illuminant interpolation and the ForwardMatrix composition"
```

---

### Task 3: use the profile in the decode

**Files:**
- Modify: `crates/raw-decode/src/lib.rs`
- Modify: `crates/raw-decode/src/backend_rsraw.rs`
- Modify: `crates/raw-decode/Cargo.toml`

- [ ] **Step 1: Add the option**

`crates/raw-decode/Cargo.toml` gains `camera-profile = { path = "../camera-profile" }`.

In `DecodeOptions`:

```rust
    /// A `.dcp` camera profile to use instead of libraw's matrix.
    ///
    /// ⚠ `None` — the default — keeps C1's libraw path exactly. That is what makes this opt-in
    /// change unable to regress anything, and it keeps the C1 oracle live for comparison.
    /// Discovery and the UI are C4.
    pub camera_profile: Option<std::path::PathBuf>,
```

`camera_profile: None` in all three constructors.

- [ ] **Step 2: Use it in the backend**

In `backend_rsraw.rs`, where C1 built the matrix, prefer the profile when present:

```rust
    let image = if opts.camera_native {
        let m = {
            let c = &raw.as_mut().color;
            let mut rgb_cam = [[0.0f32; 3]; 3];
            for (i, row) in rgb_cam.iter_mut().enumerate() {
                row.copy_from_slice(&c.rgb_cam[i][..3]);
            }
            // ⚠ `cam_mul` is the white balance libraw actually applied, which is what the DNG
            // interpolation is defined against — not a temperature the user typed.
            let cam_mul = [c.cam_mul[0], c.cam_mul[1], c.cam_mul[2]];
            match opts.camera_profile.as_deref().and_then(profile_matrix(cam_mul)) {
                Some(m) => m,
                None => color::camera::camera_to_prophoto(rgb_cam),
            }
        };
        ImageBuf::from_vec(width, height, data).map_pixels(|p| color::spaces::apply(&m, p))
    } else {
        ImageBuf::from_vec(width, height, data)
    };
```

with, above `decode`:

```rust
/// Load a profile and derive camera → ProPhoto from it.
///
/// ⚠ Returns `None` on any failure — a missing or malformed profile falls back to libraw's
/// matrix rather than failing the decode. A photo that renders slightly differently beats a photo
/// that does not open, and the fallback is the well-tested C1 path.
fn profile_matrix(cam_mul: [f32; 3]) -> impl Fn(&Path) -> Option<[[f32; 3]; 3]> {
    move |path| {
        let p = camera_profile::CameraProfile::from_file(path).ok()?;
        let forward1 = p.forward1?;
        let forward2 = p.forward2?;
        let t1 = color::camera::illuminant_temperature(p.illuminant1);
        let t2 = color::camera::illuminant_temperature(p.illuminant2);
        // ⚠ The camera neutral is the *reciprocal* of the white-balance multipliers: `cam_mul`
        // scales the channels up to make a neutral object neutral, so the neutral object's own
        // camera reading is 1/cam_mul.
        let neutral = [1.0 / cam_mul[0], 1.0 / cam_mul[1], 1.0 / cam_mul[2]];
        let temp = color::camera::neutral_temperature(neutral, p.color1, p.color2, t1, t2);
        let g = color::camera::illuminant_weight(temp, t1, t2);
        Some(color::camera::forward_to_prophoto(color::camera::blend(
            forward1, forward2, g,
        )))
    }
}
```

⚠ `neutral_temperature` is the iterative DNG solve and does not exist yet — add it to
`color::camera` as part of this task:

```rust
/// The colour temperature implied by a camera neutral, solved the way the DNG spec does.
///
/// ⚠⚠ **Iterative because the answer depends on itself**: the matrix used to convert the neutral
/// to XYZ depends on the temperature, which is derived from that XYZ. Thirty passes is far more
/// than it needs — it converges in a handful — and a fixed count avoids any question of a loop
/// that does not terminate on a malformed profile.
///
/// ⚠ The temperature comes from McCamy's approximation, which is accurate to about 2% over the
/// range profiles actually use. That error moves the interpolation weight slightly and is far
/// below the difference between the two matrices; it is recorded rather than hidden, and the
/// RawTherapee comparison is what bounds it in practice.
pub fn neutral_temperature(
    neutral: [f32; 3],
    color1: [[f32; 3]; 3],
    color2: [[f32; 3]; 3],
    t1: f32,
    t2: f32,
) -> f32 {
    let mut temp = (t1 + t2) * 0.5;
    for _ in 0..30 {
        let g = illuminant_weight(temp, t1, t2);
        let m = blend(color1, color2, g);
        let Some(inv) = invert(&m) else { return temp };
        let xyz = crate::spaces::apply(&inv, neutral);
        let sum = xyz[0] + xyz[1] + xyz[2];
        if sum.abs() < 1e-9 {
            return temp;
        }
        let (x, y) = (xyz[0] / sum, xyz[1] / sum);
        // McCamy's approximation.
        let n = (x - 0.3320) / (0.1858 - y);
        let next = 449.0 * n * n * n + 3525.0 * n * n + 6823.3 * n + 5520.33;
        if !next.is_finite() {
            return temp;
        }
        let next = next.clamp(1500.0, 15000.0);
        if (next - temp).abs() < 1.0 {
            return next;
        }
        temp = next;
    }
    temp
}

/// 3×3 inverse, or `None` when the matrix is singular.
fn invert(m: &[[f32; 3]; 3]) -> Option<[[f32; 3]; 3]> {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    if det.abs() < 1e-12 {
        return None;
    }
    let mut out = [[0.0f32; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            let sub: Vec<f32> = (0..3)
                .filter(|r| *r != i)
                .flat_map(|r| (0..3).filter(move |c| *c != j).map(move |c| m[r][c]))
                .collect();
            let cof = sub[0] * sub[3] - sub[1] * sub[2];
            out[j][i] = cof * if (i + j) % 2 == 0 { 1.0 } else { -1.0 } / det;
        }
    }
    Some(out)
}
```

with a test that `invert` round-trips and that a singular matrix returns `None`.

- [ ] **Step 3: Build and run the suite**

Run: `cargo clippy --all-targets --all-features -- -D warnings && cargo test --all`
Expected: PASS, **39 goldens byte-identical** — `camera_profile` defaults to `None`, so nothing
existing renders differently.

- [ ] **Step 4: Commit**

```bash
cargo fmt --all
git add -A
git commit -m "feat(raw-decode): render through a DCP when one is given"
```

---

### Task 4: compare against RawTherapee, then update the docs

**Files:**
- Create: `crates/raw-decode/examples/dcp_compare.rs`

- [ ] **Step 1: Report the difference between the two paths**

```rust
//! Compare the DCP render against libraw's matrix.
//!
//! `cargo run --release -p raw-decode --example dcp_compare <raw> <dcp>`
use raw_decode::{decode_raw, DecodeOptions};
use std::path::{Path, PathBuf};

fn main() {
    let mut a = std::env::args().skip(1);
    let raw = PathBuf::from(a.next().expect("usage: dcp_compare <raw> <dcp>"));
    let dcp = PathBuf::from(a.next().expect("usage: dcp_compare <raw> <dcp>"));

    let base = decode_raw(
        Path::new(&raw),
        &DecodeOptions { half_size: true, ..DecodeOptions::linear_scene() },
    ).expect("libraw path");
    let prof = decode_raw(
        Path::new(&raw),
        &DecodeOptions {
            half_size: true,
            camera_profile: Some(dcp),
            ..DecodeOptions::linear_scene()
        },
    ).expect("dcp path");

    let (mut max, mut sum, mut n) = (0.0f32, 0.0f64, 0usize);
    for y in 0..base.image.height() {
        for x in 0..base.image.width() {
            let (p, q) = (base.image.pixel(x, y), prof.image.pixel(x, y));
            for i in 0..3 {
                let d = (p[i] - q[i]).abs();
                max = max.max(d);
                sum += d as f64;
                n += 1;
            }
        }
    }
    println!("DCP vs libraw: max {max:.5}  mean {:.6}", sum / n as f64);
    println!("  (a *small* difference is the expected result — see the spec)");
}
```

- [ ] **Step 2: Run it**

```bash
cargo run --release -p raw-decode --example dcp_compare "$RAW" "$DCP"
```

⚠ Expect a **small but non-zero** difference. Zero means the profile is not being used at all;
a large one means the interpolation is wrong.

- [ ] **Step 3: Compare against RawTherapee — the independent oracle**

```bash
/Applications/RawTherapee.app/Contents/MacOS/rawtherapee-cli -o /tmp/rt.tif -t -b16 -c "$RAW"
```

Render the same file through `rustroom render` and compare the two TIFFs' mid-tone patches.
⚠ They will **not** match exactly: RawTherapee applies its own tone curve and default settings.
What must agree is the **hue and relative channel balance** of neutral and near-neutral areas. If
ours is visibly warmer or cooler than RawTherapee's, the illuminant interpolation is inverted —
that is the specific failure this comparison exists to catch.

⚠ If they cannot be brought into agreement, **say so and leave `camera_profile` off by default**
rather than shipping a plausible-looking transform. The spec names that as the honest outcome.

- [ ] **Step 4: Update `CLAUDE.md`**

House style: what landed, the ⚠ findings (the reciprocal-mired rule; the ForwardMatrix white-point
constraint as a parser self-check; no chromatic adaptation because both are D50; McCamy's
approximation and its bound; the opt-in default), the measured DCP-vs-libraw difference, the test
count, and the caveats. Update **▶ NEXT ACTION** to **C3**.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "docs: record reading a DCP's matrices (C2)"
```

⚠ **Do not push.**

---

## Self-Review

**Spec coverage:** parse the DCP in its own crate (Task 1) ✓; known values from an independent
reader (Task 1) ✓; ForwardMatrix white-point self-check (Tasks 1, 2) ✓; dual-illuminant
interpolation on the camera's own white balance (Tasks 2, 3) ✓; no chromatic adaptation, both D50
(Task 2) ✓; opt-in via `camera_profile` so nothing regresses (Task 3) ✓; endpoints pinned exactly
and the middle validated against RawTherapee (Tasks 2, 4) ✓; `CameraCalibration` parsed-not-applied
and recorded (Task 1 doc comment, Task 4) ✓; "stop rather than ship a plausible transform"
(Task 4) ✓.

**Type consistency:** `CameraProfile { name, illuminant1, illuminant2, color1, color2, forward1,
forward2 }` is defined in Task 1 and read in Task 3. `illuminant_temperature`, `illuminant_weight`,
`blend`, `forward_to_prophoto`, `neutral_temperature` are defined in Tasks 2–3 and used in Task 3
with those signatures. `XYZ_D50_TO_PROPHOTO` is added in Task 2 and used there.
