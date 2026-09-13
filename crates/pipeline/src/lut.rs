//! Parsing and sampling a 3-D lookup table in the `.cube` format.
//!
//! ⚠ Pure: text in, [`Cube`] out. No I/O and no GUI — reading the file is the app's job, so this
//! is testable with a string literal and needs no fixture on disk.

/// A 3-D lookup table: `size³` RGB entries with **red varying fastest**.
pub struct Cube {
    pub size: usize,
    entries: Vec<[f32; 3]>,
    /// The input domain the file declares, per channel. `[0, 1]` unless it says otherwise.
    ///
    /// ⚠⚠ Ignoring this is **silently** wrong rather than loudly wrong: the cube is simply indexed
    /// over the wrong interval, misplacing every sample while still producing a plausible image.
    /// A real ACES LMT declares `LUT_3D_INPUT_RANGE 0.0927061729 1.0196260353`.
    dmin: [f32; 3],
    dmax: [f32; 3],
}

/// Why a `.cube` could not be read.
///
/// ⚠ An enum rather than a string, because the panel has to *say* what is wrong with a file the
/// user dropped in — see the spec's §9.2: a missing or broken look renders without the look and
/// says so, never silently.
#[derive(Debug, PartialEq, Eq)]
pub enum CubeError {
    NoSize,
    BadSize(usize),
    WrongCount { want: usize, got: usize },
    Shaper,
    BadDomain,
}

impl std::fmt::Display for CubeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CubeError::NoSize => write!(f, "no LUT_3D_SIZE line"),
            CubeError::BadSize(n) => write!(
                f,
                "LUT_3D_SIZE {n} is not usable (need 2..={})",
                Cube::MAX_SIZE
            ),
            CubeError::WrongCount { want, got } => {
                write!(f, "expected {want} entries, found {got}")
            }
            CubeError::BadDomain => write!(
                f,
                "the declared input domain is empty or inverted (DOMAIN_MIN/DOMAIN_MAX or \
                 LUT_3D_INPUT_RANGE)"
            ),
            CubeError::Shaper => write!(
                f,
                "this .cube carries a 1-D input shaper (LUT_1D_SIZE), which means a log or \
                 scene-referred input this build does not support"
            ),
        }
    }
}

impl std::error::Error for CubeError {}

impl Cube {
    /// The largest cube we will read. 128³ is 2 097 152 entries, ~25 MB as `[f32; 3]`; 256³ — the
    /// format's own ceiling — would be ~201 MB, which a malformed file must not be able to ask for.
    ///
    /// ⚠⚠ This was **64**, on the reasoning that "beyond that a file is more likely corrupt than
    /// intentional". The first real `.cube` ever put through this project falsified that: **65³ is
    /// Resolve's high-quality default**, and `resolve-LMT ACES v0.1.1.cube` is exactly 65. A bound
    /// chosen from a guess about what files look like, rather than from what they are.
    const MAX_SIZE: usize = 128;

    pub fn parse(text: &str) -> Result<Self, CubeError> {
        let mut size = None;
        let mut entries = Vec::new();
        let mut dmin = [0.0f32; 3];
        let mut dmax = [1.0f32; 3];
        let nums = |rest: &str| -> Vec<f32> {
            rest.split_whitespace()
                .filter_map(|t| t.parse().ok())
                .collect()
        };
        for line in text.lines() {
            let line = line.trim();
            // ⚠ `#` comments and the metadata keywords this format allows are skipped rather than
            // refused: a real .cube commonly carries TITLE and DOMAIN_MIN/MAX, and refusing them
            // would reject files that are perfectly valid.
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some(rest) = line.strip_prefix("LUT_3D_SIZE") {
                size = rest.trim().parse::<usize>().ok();
                continue;
            }
            // ⚠⚠ A 1-D input shaper's values share the numeric stream with the cube's, so they
            // would be read as entries. The count check does catch that — but reports "expected
            // 274625 entries, found 278720", which is true and tells the user nothing. Refused by
            // name instead, because a shaper means a log or scene-referred input that W5-0
            // deliberately excluded.
            if line.starts_with("LUT_1D_SIZE") {
                return Err(CubeError::Shaper);
            }
            // ⚠ `LUT_3D_INPUT_RANGE lo hi` is scalar and applies to all three channels;
            // `DOMAIN_MIN`/`DOMAIN_MAX` are per channel. Both say the same thing.
            if let Some(rest) = line.strip_prefix("LUT_3D_INPUT_RANGE") {
                if let [lo, hi] = nums(rest)[..] {
                    dmin = [lo; 3];
                    dmax = [hi; 3];
                }
                continue;
            }
            if let Some(rest) = line.strip_prefix("DOMAIN_MIN") {
                if let [r, g, b] = nums(rest)[..] {
                    dmin = [r, g, b];
                }
                continue;
            }
            if let Some(rest) = line.strip_prefix("DOMAIN_MAX") {
                if let [r, g, b] = nums(rest)[..] {
                    dmax = [r, g, b];
                }
                continue;
            }
            if line.starts_with(char::is_alphabetic) {
                continue;
            }
            let v: Vec<f32> = line
                .split_whitespace()
                .filter_map(|t| t.parse().ok())
                .collect();
            if let [r, g, b] = v[..] {
                entries.push([r, g, b]);
            }
        }
        let size = size.ok_or(CubeError::NoSize)?;
        if !(2..=Self::MAX_SIZE).contains(&size) {
            return Err(CubeError::BadSize(size));
        }
        // ⚠ Written through `partial_cmp` rather than as `hi <= lo`, because a **NaN** bound must
        // be refused as well: NaN fails every comparison, so `hi <= lo` is false for it and the
        // positive form would let it straight through to the division. `partial_cmp` returns
        // `None` for NaN, which is neither `Greater` nor usable, so the one test covers both.
        if (0..3).any(|c| {
            !matches!(
                dmax[c].partial_cmp(&dmin[c]),
                Some(std::cmp::Ordering::Greater)
            )
        }) {
            return Err(CubeError::BadDomain);
        }
        let want = size * size * size;
        if entries.len() != want {
            return Err(CubeError::WrongCount {
                want,
                got: entries.len(),
            });
        }
        Ok(Self {
            size,
            entries,
            dmin,
            dmax,
        })
    }

    /// The table itself, flat, in the format's own ordering (red fastest).
    ///
    /// ⚠⚠ Exposed so `gpu-pipeline` uploads **this** table rather than rebuilding one. Two
    /// constructions of one table is how the two come to disagree about a look nobody re-derives
    /// — the rule `ToneCurveStage::lut`, `ColorGradingStage::lut` and `HueTable::entries` already
    /// follow, and the reason they follow it.
    pub fn entries(&self) -> &[[f32; 3]] {
        &self.entries
    }

    /// The declared input domain's lower bound, per channel.
    ///
    /// ⚠ A shader that indexed over `[0, 1]` regardless would be **silently** wrong rather than
    /// loudly wrong — misplacing every sample while still producing a plausible image. A real ACES
    /// LMT declares `LUT_3D_INPUT_RANGE 0.0927061729 1.0196260353`, so this is reachable in
    /// practice.
    pub fn dmin(&self) -> [f32; 3] {
        self.dmin
    }

    /// The declared input domain's upper bound, per channel. See [`Cube::dmin`].
    pub fn dmax(&self) -> [f32; 3] {
        self.dmax
    }

    /// The entry at integer cube coordinates. ⚠ Red varies fastest — the format's own ordering.
    pub fn at(&self, r: usize, g: usize, b: usize) -> [f32; 3] {
        self.entries[(b * self.size + g) * self.size + r]
    }

    /// Trilinear sample. Inputs outside `[0, 1]` clamp to the cube's edge.
    pub fn lookup(&self, rgb: [f32; 3]) -> [f32; 3] {
        let n = self.size - 1;
        let mut lo = [0usize; 3];
        let mut fr = [0f32; 3];
        for c in 0..3 {
            // ⚠ The CLAMP is here, on the index. A `.cube` stores an absolute output colour, so
            // above 1.0 there is nothing to scale — the `ProfileHueSatMap` trick of clamping the
            // index and applying to the real value does NOT transfer, because that table stores a
            // scale. See the spec, §4.
            // ⚠ Normalise out of the declared domain FIRST, then clamp. Clamping the raw value
            // to [0, 1] before the remap would be the old bug wearing a different hat.
            let t = (rgb[c] - self.dmin[c]) / (self.dmax[c] - self.dmin[c]);
            let x = t.clamp(0.0, 1.0) * n as f32;
            let i = (x.floor() as usize).min(n.saturating_sub(1));
            lo[c] = i;
            fr[c] = x - i as f32;
        }
        let hi = [(lo[0] + 1).min(n), (lo[1] + 1).min(n), (lo[2] + 1).min(n)];
        let mut out = [0f32; 3];
        for (i, o) in out.iter_mut().enumerate() {
            let c000 = self.at(lo[0], lo[1], lo[2])[i];
            let c100 = self.at(hi[0], lo[1], lo[2])[i];
            let c010 = self.at(lo[0], hi[1], lo[2])[i];
            let c110 = self.at(hi[0], hi[1], lo[2])[i];
            let c001 = self.at(lo[0], lo[1], hi[2])[i];
            let c101 = self.at(hi[0], lo[1], hi[2])[i];
            let c011 = self.at(lo[0], hi[1], hi[2])[i];
            let c111 = self.at(hi[0], hi[1], hi[2])[i];
            // ⚠ `a + (b - a) * t` and not `a*(1-t) + b*t`: at `t = 0` the first form returns `a`
            // exactly, which is what makes an identity cube bit-exact at every grid point. The
            // second form does not in f32 — the same `1.0 - (1.0 - f) != f` trap the resampler
            // records.
            let lerp = |a: f32, b: f32, t: f32| a + (b - a) * t;
            let c00 = lerp(c000, c100, fr[0]);
            let c10 = lerp(c010, c110, fr[0]);
            let c01 = lerp(c001, c101, fr[0]);
            let c11 = lerp(c011, c111, fr[0]);
            *o = lerp(lerp(c00, c10, fr[1]), lerp(c01, c11, fr[1]), fr[2]);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The smallest legal cube: `n³`, the identity.
    fn identity_text(n: usize) -> String {
        let mut s = format!("LUT_3D_SIZE {n}\n");
        for b in 0..n {
            for g in 0..n {
                for r in 0..n {
                    let f = |i: usize| i as f32 / (n - 1) as f32;
                    s.push_str(&format!("{} {} {}\n", f(r), f(g), f(b)));
                }
            }
        }
        s
    }

    /// ⚠⚠ **Red varies fastest.** Getting the index order wrong produces a cube that parses, has
    /// the right entry count, and transposes the colour axes — a plausible wrong image, which is
    /// this project's worst failure shape. A non-identity fixture is what catches it.
    #[test]
    fn the_red_axis_varies_fastest() {
        // 2×2×2 where every entry is its own (r, g, b) index — so a transposition is visible.
        let text = "LUT_3D_SIZE 2\n\
                    0 0 0\n1 0 0\n0 1 0\n1 1 0\n\
                    0 0 1\n1 0 1\n0 1 1\n1 1 1\n";
        let cube = Cube::parse(text).expect("parses");
        assert_eq!(cube.size, 2);
        // Entry 1 is (r=1, g=0, b=0): red stepped first.
        assert_eq!(cube.at(1, 0, 0), [1.0, 0.0, 0.0]);
        assert_eq!(cube.at(0, 1, 0), [0.0, 1.0, 0.0]);
        assert_eq!(cube.at(0, 0, 1), [0.0, 0.0, 1.0]);
    }

    /// ⚠⚠ **An identity LUT is BIT-EXACTLY the identity.** `assert_eq!`, no tolerance: a shaper or
    /// an interpolator that is *nearly* right is exactly what a tolerance hides, and this is the
    /// test `docs/BACKLOG.md` names as the one available exact check.
    #[test]
    fn an_identity_cube_is_bit_exactly_the_identity() {
        let cube = Cube::parse(&identity_text(17)).expect("parses");
        for &v in &[0.0f32, 0.25, 0.5, 0.75, 1.0, 0.123_456] {
            let got = cube.lookup([v, v, v]);
            assert_eq!(got, [v, v, v], "identity lookup moved {v}");
        }
    }

    /// ⚠ A value above 1.0 clamps to the cube's edge. It must not wrap, extrapolate or produce NaN.
    #[test]
    fn a_value_above_one_clamps_to_the_cube_edge() {
        let cube = Cube::parse(&identity_text(2)).expect("parses");
        assert_eq!(cube.lookup([2.0, -1.0, 0.5]), [1.0, 0.0, 0.5]);
    }

    /// ⚠ A malformed file is REFUSED, never silently treated as the identity — a look that does
    /// nothing is indistinguishable from a look that is not applied.
    #[test]
    fn a_malformed_cube_is_refused() {
        assert!(Cube::parse("not a cube at all").is_err());
        assert!(
            Cube::parse("LUT_3D_SIZE 3\n0 0 0\n").is_err(),
            "too few entries"
        );
        assert!(Cube::parse("LUT_3D_SIZE 0\n").is_err(), "degenerate size");
    }

    /// ⚠ A real `.cube` carries `TITLE` and often `DOMAIN_MIN`/`DOMAIN_MAX` and `#` comments.
    /// Refusing those would reject perfectly valid files.
    #[test]
    fn metadata_lines_and_comments_are_skipped() {
        let text = "# a comment\n\
                    TITLE \"a look\"\n\
                    DOMAIN_MIN 0.0 0.0 0.0\n\
                    DOMAIN_MAX 1.0 1.0 1.0\n\
                    \n\
                    LUT_3D_SIZE 2\n\
                    0 0 0\n1 0 0\n0 1 0\n1 1 0\n\
                    0 0 1\n1 0 1\n0 1 1\n1 1 1\n";
        let cube = Cube::parse(text).expect("parses despite the metadata");
        assert_eq!(cube.size, 2);
    }

    /// ⚠⚠ **65 is a STANDARD size, not a corrupt file.** Resolve's high-quality default is 65³,
    /// and the checkpoint's real `resolve-LMT ACES v0.1.1.cube` is exactly that. The old
    /// `MAX_SIZE = 64` refused it on the reasoning that "beyond 64 a file is more likely corrupt
    /// than intentional", which the first real LUT tried on this project falsified.
    #[test]
    fn a_sixty_five_cube_is_accepted() {
        let cube = Cube::parse(&identity_text(65)).expect("65 is a standard size");
        assert_eq!(cube.size, 65);
        assert_eq!(cube.lookup([0.5, 0.5, 0.5]), [0.5, 0.5, 0.5]);
    }

    /// ⚠⚠ **A DECLARED INPUT DOMAIN IS HONOURED, AND IGNORING IT IS SILENTLY WRONG.** The parser
    /// skipped every line starting with a letter, so `LUT_3D_INPUT_RANGE` was dropped and the cube
    /// was indexed over `[0, 1]` whatever it declared — misplacing every sample with no error.
    /// The real ACES LMT declares `LUT_3D_INPUT_RANGE 0.0927061729 1.0196260353`.
    #[test]
    fn a_declared_input_range_is_honoured() {
        // An identity cube over the domain [0, 2]: an input of 1.0 sits halfway along it, so it
        // must come back as 0.5. Read over [0, 1] instead it would come back as 1.0.
        let text = format!(
            "LUT_3D_INPUT_RANGE 0.0 2.0
{}",
            identity_text(2)
        );
        let cube = Cube::parse(&text).expect("parses");
        assert_eq!(cube.lookup([1.0, 1.0, 1.0]), [0.5, 0.5, 0.5]);
    }

    /// ⚠⚠ **A SHAPER LUT IS REFUSED BY NAME.** A `.cube` may carry a 1-D input shaper ahead of the
    /// 3-D table; its values sit in the same numeric stream, so they would be read as cube entries.
    /// The count check catches that today, but reports `expected 274625 entries, found 278720` —
    /// true, and useless to anyone holding the file. A shaper carries a log/scene-referred input
    /// that W5-0 deliberately excluded, so this says so instead of guessing.
    /// ⚠⚠ **A DEGENERATE DOMAIN DIVIDES BY ZERO AND THE RESULT IS NaN, NOT AN ERROR.** `NaN`
    /// survives `clamp` untouched, `NaN as usize` saturates to 0, and the lookup then returns the
    /// cube's first entry for every pixel — a uniform image with nothing raised. Refused at parse,
    /// where it is still describable.
    #[test]
    fn a_degenerate_domain_is_refused() {
        let flat = format!("DOMAIN_MIN 1 1 1\nDOMAIN_MAX 1 1 1\n{}", identity_text(2));
        assert_eq!(Cube::parse(&flat).err(), Some(CubeError::BadDomain));
        let inverted = format!("LUT_3D_INPUT_RANGE 1.0 0.0\n{}", identity_text(2));
        assert_eq!(Cube::parse(&inverted).err(), Some(CubeError::BadDomain));
        // ⚠ `"nan".parse::<f32>()` succeeds, so a malformed file really can declare one — and NaN
        // fails every comparison, so a naive `hi <= lo` test lets it through to the division.
        let nan = format!("DOMAIN_MAX nan nan nan\n{}", identity_text(2));
        assert_eq!(Cube::parse(&nan).err(), Some(CubeError::BadDomain));
    }

    #[test]
    fn a_shaper_lut_is_refused_by_name() {
        let text = "LUT_1D_SIZE 2\n\
                    0 0 0\n1 1 1\n\
                    LUT_3D_SIZE 2\n\
                    0 0 0\n1 0 0\n0 1 0\n1 1 0\n\
                    0 0 1\n1 0 1\n0 1 1\n1 1 1\n";
        assert_eq!(Cube::parse(text).err(), Some(CubeError::Shaper));
    }
}
