//! What the preview is converted into for the monitor in front of the user (2026-11-18): one of the manual setting's
//! fixed spaces, or the monitor's own profile — an assigned ICC profile or its EDID's primaries — sampled once through
//! LittleCMS into a 3-D table.
//!
//! ⚠ The DISPLAY path only — the canvas, thumbnails and rendered previews. Never an export, never inside the pipeline,
//! never the histogram (`render_engine::pack`'s ordering).

use crate::edid::Primaries;
use crate::OutputSpace;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Mutex, OnceLock};

/// Grid points per axis of a profile's table. ⚠ Chosen by measurement against LittleCMS's own transform
/// (`the_table_matches_littlecms`: 17 → 0.55, 33 → 0.14, 65 → 0.04 8-bit steps at worst on the M27Q, root-spaced),
/// never by feel.
pub const GRID: usize = 33;

// ⚠⚠ **The grid is spaced by a SQUARE ROOT** — its points sit at `(i / (n − 1))²` on each axis and a lookup indexes by
// `√x` — measured into existence (2026-11-18). A display curve that rises steeply from black (Adobe RGB's pure 2.2
// power, the many monitor ICC profiles with a gamma curve) cannot be followed by straight lines between points 1/32
// apart: evenly spaced, an Adobe RGB table was **4.66 steps** off its formula and a gamma-2.2 ICC profile 4.66 off
// LittleCMS. Spaced by the root, at the same 33 points: 0.32 and 0.32, and better in every other case measured too
// (P3 0.26 → 0.13, the M27Q 0.28 → 0.14). ⛔ Raising the entries to 2.2 instead was measured and refused: it fixed the
// gamma curves and broke sRGB's (P3 0.26 → 2.10), whose straight toe it made steep.

/// The display the preview is converted for.
///
/// ⚠ `Copy`, like the `OutputSpace` it replaced on the display path: a profile is INTERNED for the process's life,
/// one per distinct source (a handful of monitors at most), so every `FrameSpec` copy stays free.
#[derive(Debug, Clone, Copy)]
pub enum Display {
    /// One of the manual setting's spaces — `Srgb` means no conversion at all.
    Space(OutputSpace),
    /// A monitor's own profile.
    Profile(&'static DisplayProfile),
}

impl Display {
    /// No conversion: what every fast path requires.
    pub const SRGB: Display = Display::Space(OutputSpace::Srgb);

    /// ⚠ The ONE test every fast-path gate reads: a profile is never "sRGB", even one close to it.
    pub fn is_srgb(&self) -> bool {
        matches!(self, Display::Space(OutputSpace::Srgb))
    }

    /// One pixel, sRGB-encoded in and the display's encoding out — in float, before quantizing.
    pub fn convert(&self, c: [f32; 3]) -> [f32; 3] {
        match self {
            Display::Space(s) => {
                let conv = s.from_srgb(c.map(crate::srgb_to_linear));
                conv.map(|v| s.encode(v))
            }
            Display::Profile(p) => p.lookup(c),
        }
    }

    /// The table the GPU converts through (2026-11-18): a profile's own, or a fixed space's sampled from its exact
    /// conversion; `None` for sRGB, which converts nothing. ⚠ The CPU keeps a fixed space's exact formula — the table
    /// is within a quarter of a step of it (`a_fixed_spaces_table_is_within_a_quarter_of_a_step`).
    pub fn gpu_table(&self) -> Option<&'static DisplayProfile> {
        match *self {
            Display::Profile(p) => Some(p),
            Display::Space(OutputSpace::Srgb) => None,
            Display::Space(s) => DisplayProfile::for_space(s),
        }
    }

    /// One 8-bit sRGB pixel in, the display's 8-bit pixel out — for thumbnails, which have no float behind them.
    pub fn convert_srgb8(&self, c: [u8; 3]) -> [u8; 3] {
        match self {
            Display::Space(s) => s.convert_srgb8(c),
            Display::Profile(p) => p
                .lookup(c.map(|v| v as f32 / 255.0))
                .map(|v| (v.clamp(0.0, 1.0) * 255.0).round() as u8),
        }
    }
}

/// Equal when they convert alike: the same space, or profiles made from the same source.
impl PartialEq for Display {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Display::Space(a), Display::Space(b)) => a == b,
            (Display::Profile(a), Display::Profile(b)) => a == b,
            _ => false,
        }
    }
}

/// Equal when made from the same source (their tables are then the same).
impl PartialEq for DisplayProfile {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for DisplayProfile {}

/// A monitor's profile as a table from sRGB-encoded values to the monitor's encoded values.
pub struct DisplayProfile {
    /// What Settings shows, e.g. "M27Q" or a profile's file name.
    pub name: String,
    /// A digest of the source (the ICC bytes, or the primaries), so two builds from one source compare equal.
    id: u64,
    /// `n³` entries, red varying fastest.
    table: Vec<[f32; 3]>,
    /// Points per axis — kept, not derived per lookup (a cube root per pixel was measurable).
    n: usize,
}

/// Name and id only — the table is 36k entries (and `RUSTROOM_DEBUG_DISPLAY` prints a `Display`).
impl std::fmt::Debug for DisplayProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DisplayProfile({:?}, {:016x})", self.name, self.id)
    }
}

impl DisplayProfile {
    /// From a monitor's EDID primaries and white, with sRGB's tone curve (the spec: only the primaries are
    /// corrected — the EDID's single gamma cannot describe the panel's curve). `None` if LittleCMS refuses them.
    pub fn from_primaries(name: &str, p: &Primaries) -> Option<&'static Self> {
        let xy = |(x, y): (f64, f64)| lcms2::CIExyY { x, y, Y: 1.0 };
        let prim = lcms2::CIExyYTRIPLE {
            Red: xy(p.red),
            Green: xy(p.green),
            Blue: xy(p.blue),
        };
        let curve = srgb_curve();
        let dst = lcms2::Profile::new_rgb(&xy(p.white), &prim, &[&curve, &curve, &curve]).ok()?;
        let mut h = std::collections::hash_map::DefaultHasher::new();
        "primaries".hash(&mut h);
        for v in [p.red, p.green, p.blue, p.white] {
            v.0.to_bits().hash(&mut h);
            v.1.to_bits().hash(&mut h);
        }
        let id = h.finish();
        Self::interned(id, || Self::sample(name, id, &dst, GRID))
    }

    /// From an ICC profile's bytes, used whole. `None` if LittleCMS cannot read it or cannot transform into it.
    pub fn from_icc(name: &str, bytes: &[u8]) -> Option<&'static Self> {
        let dst = lcms2::Profile::new_icc(bytes).ok()?;
        let mut h = std::collections::hash_map::DefaultHasher::new();
        "icc".hash(&mut h);
        bytes.hash(&mut h);
        let id = h.finish();
        Self::interned(id, || Self::sample(name, id, &dst, GRID))
    }

    /// A fixed space as a table, sampled from its exact conversion — for the GPU, which converts every display through
    /// a table ([`Display::gpu_table`]).
    fn for_space(s: OutputSpace) -> Option<&'static Self> {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        "space".hash(&mut h);
        s.hash(&mut h);
        let id = h.finish();
        let exact = Display::Space(s);
        Self::interned(id, || {
            Self::filled(
                &format!("{s:?}"),
                id,
                GRID,
                grid(GRID).into_iter().map(|c| exact.convert(c)).collect(),
            )
        })
    }

    /// The process's one profile for this source: built the first time, found the next (the name of the first build
    /// is kept — one source, one table).
    fn interned(id: u64, build: impl FnOnce() -> Option<Self>) -> Option<&'static Self> {
        static ALL: OnceLock<Mutex<HashMap<u64, &'static DisplayProfile>>> = OnceLock::new();
        let all = ALL.get_or_init(|| Mutex::new(HashMap::new()));
        if let Some(p) = all.lock().expect("display profiles").get(&id) {
            return Some(p);
        }
        let p: &'static Self = Box::leak(Box::new(build()?));
        Some(*all.lock().expect("display profiles").entry(id).or_insert(p))
    }

    /// The table, `n` points per axis, from LittleCMS's sRGB → `dst` (relative colorimetric, float).
    fn sample(name: &str, id: u64, dst: &lcms2::Profile, n: usize) -> Option<Self> {
        let src = lcms2::Profile::new_srgb();
        let t: lcms2::Transform<[f32; 3], [f32; 3]> = lcms2::Transform::new(
            &src,
            lcms2::PixelFormat::RGB_FLT,
            dst,
            lcms2::PixelFormat::RGB_FLT,
            lcms2::Intent::RelativeColorimetric,
        )
        .ok()?;
        let input = grid(n);
        let mut table = vec![[0.0f32; 3]; input.len()];
        t.transform_pixels(&input, &mut table);
        Self::filled(name, id, n, table)
    }

    /// A sampled table, checked and clipped.
    fn filled(name: &str, id: u64, n: usize, mut table: Vec<[f32; 3]>) -> Option<Self> {
        if table.iter().flatten().any(|v| !v.is_finite()) {
            return None;
        }
        // A display narrower than sRGB cannot show what lies outside it: clipped, as relative colorimetric means.
        for e in &mut table {
            *e = e.map(|v| v.clamp(0.0, 1.0));
        }
        Some(Self {
            name: name.to_string(),
            id,
            table,
            n,
        })
    }

    /// Trilinear lookup on the root-spaced grid; inputs outside `[0, 1]` clamp to the table's edge.
    pub fn lookup(&self, rgb: [f32; 3]) -> [f32; 3] {
        let n = self.n;
        let last = n - 1;
        let mut lo = [0usize; 3];
        let mut fr = [0f32; 3];
        for c in 0..3 {
            let x = rgb[c].clamp(0.0, 1.0).sqrt() * last as f32;
            let i = (x.floor() as usize).min(last - 1);
            lo[c] = i;
            fr[c] = x - i as f32;
        }
        // The eight corners fetched once each — not once per channel (it is on every displayed pixel).
        let (dy, dz) = (n, n * n);
        let i = (lo[2] * n + lo[1]) * n + lo[0];
        let t = &self.table;
        let (c000, c100, c010, c110) = (t[i], t[i + 1], t[i + dy], t[i + dy + 1]);
        let (c001, c101, c011, c111) =
            (t[i + dz], t[i + dz + 1], t[i + dz + dy], t[i + dz + dy + 1]);
        let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
        std::array::from_fn(|k| {
            let x00 = lerp(c000[k], c100[k], fr[0]);
            let x10 = lerp(c010[k], c110[k], fr[0]);
            let x01 = lerp(c001[k], c101[k], fr[0]);
            let x11 = lerp(c011[k], c111[k], fr[0]);
            lerp(lerp(x00, x10, fr[1]), lerp(x01, x11, fr[1]), fr[2])
        })
    }

    /// The `n³` entries at the root-spaced grid points, red varying fastest — what the GPU uploads and looks up as
    /// [`Self::lookup`] does.
    pub fn table(&self) -> &[[f32; 3]] {
        &self.table
    }

    /// Points per axis of [`Self::table`].
    pub fn n(&self) -> usize {
        self.n
    }
}

/// The table's inputs, `n` points per axis over `[0, 1]` at `(i / (n - 1))²`, red varying fastest.
fn grid(n: usize) -> Vec<[f32; 3]> {
    let step = 1.0 / (n - 1) as f32;
    let mut input = Vec::with_capacity(n * n * n);
    for b in 0..n {
        for g in 0..n {
            for r in 0..n {
                input.push([r, g, b].map(|i| (i as f32 * step).powi(2)));
            }
        }
    }
    input
}

/// sRGB's own curve as LittleCMS's parametric type 4 — the same one `export` builds its sRGB profile with.
fn srgb_curve() -> lcms2::ToneCurve {
    lcms2::ToneCurve::new_parametric(4, &[2.4, 1.0 / 1.055, 0.055 / 1.055, 1.0 / 12.92, 0.04045])
        .expect("sRGB parametric curve")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deterministic spread of sRGB-encoded samples, corners and edges included — and ⚠ as many again NEAR BLACK
    /// (all three channels under 0.08, or one under 0.05), where a steep display curve is hardest to follow: the even
    /// spread alone holds few of them.
    fn samples() -> Vec<[f32; 3]> {
        let mut s: u32 = 12345;
        let mut next = || {
            s = s.wrapping_mul(1_103_515_245).wrapping_add(12345);
            (s >> 8) as f32 / (1u32 << 24) as f32
        };
        let mut v: Vec<[f32; 3]> = (0..4096).map(|_| [next(), next(), next()]).collect();
        for _ in 0..2048 {
            v.push([next() * 0.08, next() * 0.08, next() * 0.08]);
            v.push([next() * 0.05, next(), next()]);
        }
        v.extend([
            [0.0; 3],
            [1.0; 3],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.02, 0.02, 0.02],
        ]);
        v
    }

    fn m27q() -> Primaries {
        crate::edid::parse(include_bytes!("../testdata/m27q.edid"))
            .unwrap()
            .primaries
    }

    /// The largest difference, in 8-bit steps, between two conversions over the samples.
    ///
    /// ⚠ NaN wins the fold (`AGENTS.md`: `f32::max` ignores NaN, so a plain fold would read a NaN conversion as
    /// a perfect 0) — every `err < bound` below then fails on a non-finite value.
    fn max_steps(a: impl Fn([f32; 3]) -> [f32; 3], b: impl Fn([f32; 3]) -> [f32; 3]) -> f32 {
        let worst = |m: f32, v: f32| {
            if m.is_nan() || v.is_nan() {
                f32::NAN
            } else {
                m.max(v)
            }
        };
        samples()
            .into_iter()
            .map(|c| {
                let (x, y) = (a(c), b(c));
                (0..3).map(|k| (x[k] - y[k]).abs()).fold(0.0f32, worst) * 255.0
            })
            .fold(0.0f32, worst)
    }

    /// ⚠ Measured, then pinned: a fixed space's GPU table against the exact formula the CPU keeps, for every space
    /// but sRGB (which has no table), clipped as the quantizer clips.
    #[test]
    fn a_fixed_spaces_table_is_within_a_third_of_a_step() {
        assert!(Display::SRGB.gpu_table().is_none(), "sRGB converts nothing");
        for s in OutputSpace::ALL
            .into_iter()
            .filter(|s| *s != OutputSpace::Srgb)
        {
            let exact = Display::Space(s);
            assert!(
                max_steps(|c| exact.convert(c), |c| c) > 5.0,
                "fixture check: {s:?} must move something"
            );
            let table = exact.gpu_table().expect("a table");
            assert!(std::ptr::eq(table, exact.gpu_table().unwrap()), "interned");
            let err = max_steps(
                |c| table.lookup(c),
                |c| exact.convert(c).map(|v| v.clamp(0.0, 1.0)),
            );
            eprintln!("{s:?}: the table is {err:.3} steps from the formula");
            assert!(err < 1.0 / 3.0, "{s:?}: {err:.3} steps");
        }
        let m = DisplayProfile::from_primaries("M27Q", &m27q()).unwrap();
        assert!(
            std::ptr::eq(Display::Profile(m).gpu_table().unwrap(), m),
            "a profile's GPU table is its own"
        );
    }

    /// ⚠ The size measured, not guessed: the table at GRID against LittleCMS's own float transform, on the M27Q.
    #[test]
    fn the_table_matches_littlecms() {
        let p = m27q();
        let xy = |(x, y): (f64, f64)| lcms2::CIExyY { x, y, Y: 1.0 };
        let curve = srgb_curve();
        let prim = lcms2::CIExyYTRIPLE {
            Red: xy(p.red),
            Green: xy(p.green),
            Blue: xy(p.blue),
        };
        let dst = lcms2::Profile::new_rgb(&xy(p.white), &prim, &[&curve, &curve, &curve]).unwrap();
        let t: lcms2::Transform<[f32; 3], [f32; 3]> = lcms2::Transform::new(
            &lcms2::Profile::new_srgb(),
            lcms2::PixelFormat::RGB_FLT,
            &dst,
            lcms2::PixelFormat::RGB_FLT,
            lcms2::Intent::RelativeColorimetric,
        )
        .unwrap();
        let exact = |c: [f32; 3]| {
            let mut o = [[0.0f32; 3]];
            t.transform_pixels(&[c], &mut o);
            o[0].map(|v| v.clamp(0.0, 1.0))
        };
        for n in [17, 33, 65] {
            let table = DisplayProfile::sample("m27q", 0, &dst, n).unwrap();
            eprintln!(
                "grid {n}: max {:.3} 8-bit steps",
                max_steps(|c| table.lookup(c), exact)
            );
        }
        let table = DisplayProfile::from_primaries("m27q", &p).unwrap();
        let err = max_steps(|c| table.lookup(c), exact);
        assert!(err < 0.5, "GRID {GRID}: {err:.3} steps from LittleCMS");
    }

    /// ⚠⚠ The case the root-spaced grid exists for: a monitor's ICC profile with a pure gamma-2.2 curve (Adobe RGB's primaries),
    /// against LittleCMS's own transform into it — the curve that rises steeply from black.
    #[test]
    fn a_gamma_profile_matches_littlecms_near_black() {
        let xy = |(x, y): (f64, f64)| lcms2::CIExyY { x, y, Y: 1.0 };
        let prim = lcms2::CIExyYTRIPLE {
            Red: xy((0.64, 0.33)),
            Green: xy((0.21, 0.71)),
            Blue: xy((0.15, 0.06)),
        };
        let curve = lcms2::ToneCurve::new(2.2);
        let icc = lcms2::Profile::new_rgb(&xy((0.3127, 0.3290)), &prim, &[&curve, &curve, &curve])
            .unwrap()
            .icc()
            .unwrap();
        // ⚠ The reference reads the SAME bytes the table does: the file stores the gamma and the primaries at its own
        // precision, and against the in-memory profile that alone measured 0.8 steps.
        let dst = lcms2::Profile::new_icc(&icc).unwrap();
        let t: lcms2::Transform<[f32; 3], [f32; 3]> = lcms2::Transform::new(
            &lcms2::Profile::new_srgb(),
            lcms2::PixelFormat::RGB_FLT,
            &dst,
            lcms2::PixelFormat::RGB_FLT,
            lcms2::Intent::RelativeColorimetric,
        )
        .unwrap();
        let exact = |c: [f32; 3]| {
            let mut o = [[0.0f32; 3]];
            t.transform_pixels(&[c], &mut o);
            o[0].map(|v| v.clamp(0.0, 1.0))
        };
        let table = DisplayProfile::from_icc("gamma 2.2", &icc).unwrap();
        let err = max_steps(|c| table.lookup(c), exact);
        eprintln!("gamma 2.2 profile: the table is {err:.3} steps from LittleCMS");
        assert!(err < 1.0 / 3.0, "{err:.3} steps from LittleCMS");
    }

    /// sRGB's own primaries are the identity — the spec's promise that only the primaries are corrected.
    #[test]
    fn srgb_primaries_change_nothing() {
        let p = DisplayProfile::from_primaries("srgb", &Primaries::SRGB).unwrap();
        let err = max_steps(|c| p.lookup(c), |c| c);
        assert!(err < 0.5, "{err:.3} steps from the identity");
    }

    /// Two implementations, one answer: a Display P3 ICC profile through LittleCMS and the table agrees with the
    /// manual setting's P3 matrix.
    #[test]
    fn a_p3_profile_agrees_with_the_p3_space() {
        let xy = |(x, y): (f64, f64)| lcms2::CIExyY { x, y, Y: 1.0 };
        let prim = lcms2::CIExyYTRIPLE {
            Red: xy((0.680, 0.320)),
            Green: xy((0.265, 0.690)),
            Blue: xy((0.150, 0.060)),
        };
        let curve = srgb_curve();
        let icc = lcms2::Profile::new_rgb(&xy((0.3127, 0.3290)), &prim, &[&curve, &curve, &curve])
            .unwrap()
            .icc()
            .unwrap();
        let profile = Display::Profile(DisplayProfile::from_icc("p3", &icc).unwrap());
        let space = Display::Space(OutputSpace::DisplayP3);
        assert!(
            max_steps(|c| space.convert(c), |c| c) > 5.0,
            "fixture check: P3 must move something"
        );
        let err = max_steps(|c| profile.convert(c), |c| space.convert(c));
        assert!(
            err < 1.0,
            "{err:.3} steps between the ICC table and the P3 matrix"
        );
    }

    /// On the M27Q, sRGB's pure green must be driven with the panel's red and blue mixed in: the panel's own green
    /// is far wider and bluer, so on its own it is the oversaturated green this fixes. ⚠ Measured
    /// (0.572, 0.986, 0.237) — the green channel stays near full; the bounds sit below the measurement rather than
    /// being a guess (`AGENTS.md`: a threshold you invented).
    #[test]
    fn the_m27q_tames_srgb_green() {
        let d = Display::Profile(DisplayProfile::from_primaries("M27Q", &m27q()).unwrap());
        let g = d.convert([0.0, 1.0, 0.0]);
        assert!(g[0] > 0.5 && g[2] > 0.2, "{g:?}");
        let grey = d.convert([0.5, 0.5, 0.5]);
        assert!(
            grey.iter().all(|v| (v - 0.5).abs() < 0.01),
            "a grey stays grey (D65 white): {grey:?}"
        );
        assert!(!d.is_srgb());
        assert!(Display::SRGB.is_srgb());
    }

    #[test]
    fn displays_are_equal_by_source() {
        let a = Display::Profile(DisplayProfile::from_primaries("a", &m27q()).unwrap());
        let b = Display::Profile(DisplayProfile::from_primaries("b", &m27q()).unwrap());
        let srgb = Display::Profile(DisplayProfile::from_primaries("s", &Primaries::SRGB).unwrap());
        assert_eq!(a, b, "one source, one display");
        assert!(
            std::ptr::eq(
                DisplayProfile::from_primaries("a", &m27q()).unwrap(),
                DisplayProfile::from_primaries("c", &m27q()).unwrap()
            ),
            "interned: one table per source"
        );
        assert_ne!(a, srgb);
        assert_ne!(a, Display::SRGB);
        assert_eq!(
            Display::Space(OutputSpace::DisplayP3),
            Display::Space(OutputSpace::DisplayP3)
        );
    }
}
