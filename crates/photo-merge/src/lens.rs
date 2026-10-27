//! A frame's lens, corrected inside the panorama's one resample (M10.17b): distortion, TCA and
//! vignetting read through the frame's LensFun profile while it is projected — never a separate pass.
//!
//! ⚠ The corrector is sampled ONCE per frame on a coarse grid ([`GRID_STEP`]) and interpolated: a
//! LensFun call per pixel costs ~60 ns × 3, which over a 93-Mpx canvas is tens of seconds (the per-pixel
//! cost the pipeline measured at 400 ms of a 6.8-Mpx prefix, `Corrector::fill_row`'s note).

use crate::frame::Frame;
use lens_profile::{Corrections, Corrector, LensProfile, Shot};

/// The coarse grid's spacing, in frame pixels. Measured on the XF23 at f/2 (corner gain 4.8×): 16 px
/// left 0.022 px and 0.15 % of gain at the corner of a 1200-px frame; 8 px is under 0.01 px and 0.05 %
/// (`the_grid_follows_the_corrector`) — and a real frame, five times wider, is finer still.
pub const GRID_STEP: usize = 8;

/// Everything the projection needs to read a corrected frame.
pub enum FrameLens {
    /// The profile's corrections, as a grid.
    Corrected(LensGrid),
    /// The frame names a lens LensFun does not know — stitched uncorrected, and the dialog says so.
    NoProfile(String),
    /// The frame names no lens at all (a manual lens, an adapted one).
    NoLens,
}

/// A corrector sampled at every [`GRID_STEP`]-th pixel, with the last row and column at the edge.
pub struct LensGrid {
    cols: usize,
    rows: usize,
    width: usize,
    height: usize,
    /// Per node, the source position of each channel.
    src: Vec<[[f32; 2]; 3]>,
    /// Per node, the vignetting gain.
    gain: Vec<f32>,
}

/// All three corrections and no rescale: the stitch's own outline takes the empty corners a pincushion
/// correction leaves, so scaling them away would only throw pixels out.
const WANT: Corrections = Corrections {
    distortion: true,
    tca: true,
    vignetting: true,
    scale: 0.0,
};

impl FrameLens {
    /// The frame's lens from its metadata and LensFun's bundled database.
    pub fn for_frame(meta: &raw_decode::RawMetadata, width: usize, height: usize) -> Self {
        let Some(l) = meta.lens.as_ref() else {
            return FrameLens::NoLens;
        };
        let found = lens_profile::Catalogue::shared()
            .and_then(|db| db.find(&meta.make, &meta.model, &l.name));
        match found {
            Some(p) => Self::from_profile(
                &p,
                &Shot {
                    focal: l.focal,
                    aperture: l.aperture,
                },
                width,
                height,
            )
            .unwrap_or_else(|| FrameLens::NoProfile(l.name.clone())),
            None => FrameLens::NoProfile(l.name.clone()),
        }
    }

    /// The grid for `profile` at this shot and frame size; `None` when the profile corrects nothing.
    pub fn from_profile(
        profile: &LensProfile,
        shot: &Shot,
        width: usize,
        height: usize,
    ) -> Option<Self> {
        let c = Corrector::new(profile, shot, width, height, WANT)?;
        Some(FrameLens::Corrected(LensGrid::sample(&c, width, height)))
    }

    /// Where corrected pixel `(x, y)` reads each channel from, and its vignetting gain — `None` when
    /// the frame is read as it is.
    #[inline]
    pub fn at(&self, x: f32, y: f32) -> Option<([[f32; 2]; 3], f32)> {
        match self {
            FrameLens::Corrected(g) => Some(g.at(x, y)),
            _ => None,
        }
    }

    /// A lens given by `at(x, y) → (each channel's source, gain)`, sampled on the grid — tests and
    /// constructed fixtures, where no LensFun profile describes the distortion.
    pub fn from_fn(
        width: usize,
        height: usize,
        at: impl Fn(f32, f32) -> ([[f32; 2]; 3], f32),
    ) -> Self {
        FrameLens::Corrected(LensGrid::from_fn(width, height, at))
    }

    /// The lens LensFun did not know, for the dialog's line.
    pub fn missing(&self) -> Option<&str> {
        match self {
            FrameLens::NoProfile(name) => Some(name),
            _ => None,
        }
    }
}

/// `f` read through `lens`: the same size, every pixel at its corrected position, vignetting lifted — what
/// dense registration needs, since two frames are related by a homography only once their distortion is
/// gone. A pixel whose read touched a clipped sample is written clipped (65535), and one the lens reads
/// from outside the frame is written black.
pub fn corrected(f: &Frame, lens: &FrameLens) -> Frame {
    use rayon::prelude::*;
    if !matches!(lens, FrameLens::Corrected(_)) {
        return f.clone();
    }
    let s = crate::warp::Sampler::new();
    let (w, h) = (f.width, f.height);
    let px: Vec<u16> = (0..h)
        .into_par_iter()
        .flat_map_iter(|y| {
            let s = &s;
            (0..w).flat_map(
                move |x| match s.raw_at_corrected(f, lens, x as f32, y as f32) {
                    Some((_, true)) => [u16::MAX; 3],
                    Some((v, false)) => [0, 1, 2].map(|c| {
                        (v[c] * f.pre_mul[c] * 65535.0).round().clamp(0.0, 65535.0) as u16
                    }),
                    None => [0; 3],
                },
            )
        })
        .collect();
    Frame::new(w, h, px, f.pre_mul, f.ev)
}

impl LensGrid {
    fn sample(c: &Corrector, width: usize, height: usize) -> Self {
        Self::from_fn(width, height, |x, y| (c.source(x, y), c.gain(x, y)))
    }

    fn from_fn(width: usize, height: usize, at: impl Fn(f32, f32) -> ([[f32; 2]; 3], f32)) -> Self {
        let cols = width.div_ceil(GRID_STEP) + 1;
        let rows = height.div_ceil(GRID_STEP) + 1;
        let node =
            |i: usize, extent: usize| ((i * GRID_STEP) as f32).min(extent.saturating_sub(1) as f32);
        let mut src = Vec::with_capacity(cols * rows);
        let mut gain = Vec::with_capacity(cols * rows);
        for j in 0..rows {
            for i in 0..cols {
                let (s, g) = at(node(i, width), node(j, height));
                src.push(s);
                gain.push(g);
            }
        }
        Self {
            cols,
            rows,
            width,
            height,
            src,
            gain,
        }
    }

    /// Bilinear between the four nodes around `(x, y)`. The last node of a row or column sits on the
    /// frame's edge, not a whole step on, so the cell there is narrower.
    fn at(&self, x: f32, y: f32) -> ([[f32; 2]; 3], f32) {
        let locate = |v: f32, extent: usize, n: usize| {
            let last = extent.saturating_sub(1) as f32;
            let v = v.clamp(0.0, last);
            let i = ((v / GRID_STEP as f32) as usize).min(n - 2);
            let (a, b) = (
                (i * GRID_STEP) as f32,
                (((i + 1) * GRID_STEP) as f32).min(last),
            );
            let t = if b > a { (v - a) / (b - a) } else { 0.0 };
            (i, t)
        };
        let (i, tx) = locate(x, self.width, self.cols);
        let (j, ty) = locate(y, self.height, self.rows);
        let k = |i: usize, j: usize| j * self.cols + i;
        let w = [
            ((1.0 - tx) * (1.0 - ty), k(i, j)),
            (tx * (1.0 - ty), k(i + 1, j)),
            ((1.0 - tx) * ty, k(i, j + 1)),
            (tx * ty, k(i + 1, j + 1)),
        ];
        let mut src = [[0f32; 2]; 3];
        let mut gain = 0f32;
        for (wt, n) in w {
            for ch in 0..3 {
                src[ch][0] += wt * self.src[n][ch][0];
                src[ch][1] += wt * self.src[n][ch][1];
            }
            gain += wt * self.gain[n];
        }
        (src, gain)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::frame::Frame;
    use crate::warp::Sampler;

    /// The developer's own lens, from the bundled database — `the_bundled_database_has_the_developers_own_gear`
    /// proves it is there.
    pub(crate) fn xf23(w: usize, h: usize) -> FrameLens {
        let db = lens_profile::Catalogue::bundled().expect("bundled db");
        let p = db.find("Fujifilm", "X-T4", "XF23mmF2 R WR").expect("XF23");
        FrameLens::from_profile(
            &p,
            &Shot {
                focal: 23.0,
                aperture: 2.0,
            },
            w,
            h,
        )
        .expect("corrects")
    }

    /// Each channel's value is its own x coordinate — what a read returns says where it read.
    fn x_ramp(w: usize, h: usize) -> Frame {
        let px: Vec<u16> = (0..w * h)
            .flat_map(|i| [((i % w) * 40) as u16; 3])
            .collect();
        Frame::new(w, h, px, [1.0; 3], None)
    }

    fn corrector(w: usize, h: usize) -> Corrector {
        let db = lens_profile::Catalogue::bundled().expect("bundled db");
        let p = db.find("Fujifilm", "X-T4", "XF23mmF2 R WR").expect("XF23");
        Corrector::new(
            &p,
            &Shot {
                focal: 23.0,
                aperture: 2.0,
            },
            w,
            h,
            WANT,
        )
        .expect("corrects")
    }

    #[test]
    fn the_grid_follows_the_corrector() {
        let (w, h) = (1200, 800);
        let c = corrector(w, h);
        let g = xf23(w, h);
        let mut worst = 0f32;
        for (x, y) in [
            (0.0, 0.0),
            (13.3, 7.7),
            (600.0, 400.0),
            (1199.0, 799.0),
            (977.5, 31.25),
        ] {
            let (src, gain) = g.at(x, y).expect("corrected");
            let want = c.source(x, y);
            for ch in 0..3 {
                for k in 0..2 {
                    worst = worst.max((src[ch][k] - want[ch][k]).abs());
                }
            }
            let g = c.gain(x, y);
            assert!(
                ((gain - g) / g).abs() < 5e-4,
                "gain at ({x}, {y}): {gain} vs {g}"
            );
        }
        assert!(worst < 0.01, "{worst} px");
    }

    /// ⚠ The control is in the same test: at a corner the lens moves the read by more than 2 px, so
    /// a sampler that ignored the lens would be off by that much.
    #[test]
    fn a_profiled_frame_reads_at_its_lens_source() {
        let (w, h) = (1200, 800);
        let f = x_ramp(w, h);
        let lens = xf23(w, h);
        let c = corrector(w, h);
        let s = Sampler::new();
        let (x, y) = (8.0, 8.0);
        let src = c.source(x, y);
        assert!(
            (src[1][0] - x).abs() > 2.0,
            "the lens must move a corner read ({:?})",
            src[1]
        );
        let (got, clipped) = s.raw_at_corrected(&f, &lens, x, y).expect("inside");
        assert!(!clipped);
        for ch in 0..3 {
            let want = src[ch][0] * 40.0 / 65535.0 * c.gain(x, y);
            assert!(
                (got[ch] - want).abs() < 2e-5,
                "channel {ch}: {} vs {want}",
                got[ch]
            );
        }
    }

    #[test]
    fn vignetting_lifts_the_corners() {
        let (w, h) = (1200, 800);
        let f = Frame::new(w, h, vec![20_000; w * h * 3], [1.0; 3], None);
        let lens = xf23(w, h);
        let s = Sampler::new();
        let centre = s.raw_at_corrected(&f, &lens, 600.0, 400.0).unwrap().0[1];
        let corner = s.raw_at_corrected(&f, &lens, 40.0, 40.0).unwrap().0[1];
        assert!(corner > centre * 1.02, "corner {corner}, centre {centre}");
    }

    #[test]
    fn no_profile_is_the_plain_read() {
        let (w, h) = (64, 48);
        let f = x_ramp(w, h);
        let s = Sampler::new();
        for lens in [
            FrameLens::NoLens,
            FrameLens::NoProfile("Mystery 35mm".into()),
        ] {
            let (got, clipped) = s.raw_at_corrected(&f, &lens, 20.5, 11.0).unwrap();
            assert!(!clipped);
            assert_eq!(got, s.raw_at(&f, 20.5, 11.0).unwrap());
        }
    }

    /// Unlike [`Sampler::raw_at`], a corrected read still returns a clipped value — and says so — so
    /// a sky blown in every frame is still a sky, not a hole.
    #[test]
    fn a_clipped_read_is_returned_and_flagged() {
        let mut px: Vec<u16> = vec![1000; 12 * 9 * 3];
        px[(4 * 12 + 6) * 3] = 65535;
        let f = Frame::new(12, 9, px, [1.0; 3], None);
        let (_, clipped) = Sampler::new()
            .raw_at_corrected(&f, &FrameLens::NoLens, 5.5, 4.0)
            .expect("inside");
        assert!(clipped);
    }

    #[test]
    fn a_corrected_frame_holds_the_corrected_reads() {
        let (w, h) = (1200, 800);
        // A dim ramp: f/2's corner gain (4.8×) must not lift it past the u16 ceiling.
        let px: Vec<u16> = (0..w * h).flat_map(|i| [((i % w) * 8) as u16; 3]).collect();
        let f = Frame::new(w, h, px, [1.0; 3], None);
        let lens = xf23(w, h);
        let c = corrected(&f, &lens);
        let s = Sampler::new();
        for (x, y) in [(8usize, 8usize), (600, 400), (1100, 700)] {
            let (want, _) = s.raw_at_corrected(&f, &lens, x as f32, y as f32).unwrap();
            let got = c.raw(x, y);
            assert!(
                (got[1] - want[1]).abs() < 2.0 / 65535.0,
                "({x}, {y}): {got:?} vs {want:?}"
            );
        }
        let plain = corrected(&f, &FrameLens::NoLens);
        assert_eq!(plain.samples, f.samples, "no lens is the frame itself");
    }

    #[test]
    fn a_frame_without_a_lens_name_has_no_lens() {
        let meta = raw_decode::RawMetadata {
            width: 10,
            height: 10,
            make: "FUJIFILM".into(),
            model: "X-T4".into(),
            lens: None,
            as_shot: None,
        };
        assert!(matches!(
            FrameLens::for_frame(&meta, 10, 10),
            FrameLens::NoLens
        ));
    }

    #[test]
    fn an_unknown_lens_is_named() {
        let meta = raw_decode::RawMetadata {
            width: 10,
            height: 10,
            make: "FUJIFILM".into(),
            model: "X-T4".into(),
            lens: Some(raw_decode::LensMetadata {
                make: "Nobody".into(),
                name: "Mystery 35mm".into(),
                focal: 35.0,
                aperture: 2.0,
            }),
            as_shot: None,
        };
        assert_eq!(
            FrameLens::for_frame(&meta, 10, 10).missing(),
            Some("Mystery 35mm")
        );
    }
}
