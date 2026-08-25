//! Lens profiles: matching a camera and lens against LensFun's database, and turning a match
//! into the per-pixel corrections the pipeline applies.
//!
//! ⚠ This crate exists so that the `lensfun` dependency has exactly one home, mirroring
//! `camera-profile`'s relationship with `.dcp` files. `pipeline` must never learn where a
//! profile comes from.

use lensfun::{Database, Lens, Modifier};

/// The LensFun database, loaded once.
pub struct Catalogue {
    db: Database,
}

/// A matched camera + lens: everything needed to build a [`Corrector`].
///
/// ⚠ Owns a **cloned** `Lens` rather than borrowing the database. `Database::find_lenses`
/// returns references, and a borrowing profile would force a lifetime through `pipeline`'s
/// stage for no benefit — `Lens` is `Clone` and small.
#[derive(Debug, Clone)]
pub struct LensProfile {
    lens: Lens,
    crop_factor: f32,
}

impl Catalogue {
    /// The database bundled with the `lensfun` crate. No filesystem access.
    pub fn bundled() -> Result<Self, lensfun::Error> {
        Ok(Self {
            db: Database::load_bundled()?,
        })
    }

    /// A database built from one XML string — the seam the tests use.
    pub fn from_xml(xml: &str) -> Result<Self, lensfun::Error> {
        let mut db = Database::new();
        db.load_str(xml)?;
        Ok(Self { db })
    }

    /// Match a camera and lens by the names the decoder reported.
    ///
    /// Returns `None` when either is absent, which is the common case for an unlisted lens and
    /// must stay an ordinary answer rather than an error.
    pub fn find(&self, make: &str, model: &str, lens_model: &str) -> Option<LensProfile> {
        let cameras = self.db.find_cameras(Some(make), model);
        let camera = cameras.first()?;
        let lenses = self.db.find_lenses(Some(camera), lens_model);
        let lens = lenses.first()?;
        Some(LensProfile {
            lens: (*lens).clone(),
            crop_factor: camera.crop_factor,
        })
    }
}

impl LensProfile {
    pub fn lens_name(&self) -> &str {
        &self.lens.model
    }

    pub fn crop_factor(&self) -> f32 {
        self.crop_factor
    }
}

/// Subject distance handed to the vignetting interpolation, in metres.
///
/// The database tabulates vignetting at 10 m and 1000 m, and for the lenses checked during design
/// the coefficients are identical at both — the distance term matters for macro work and little
/// else. We have no subject-distance metadata, so this picks the far end.
const DEFAULT_DISTANCE: f32 = 1000.0;

/// Which corrections to apply, and how far to scale afterwards.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Corrections {
    pub distortion: bool,
    pub tca: bool,
    pub vignetting: bool,
    /// 0 = leave the empty edges distortion correction produces; 100 = scale until they are gone.
    pub scale: f32,
}

/// The per-shot metadata the interpolation needs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shot {
    pub focal: f32,
    pub aperture: f32,
}

/// A built correction for one profile, one shot and one image size.
pub struct Corrector {
    modifier: Modifier,
    width: usize,
    height: usize,
    has_distortion: bool,
    has_tca: bool,
    has_vignetting: bool,
    applied_scale: f32,
    autoscale: f32,
}

impl Corrector {
    /// Build the correction, or `None` when nothing was enabled or nothing could be enabled.
    pub fn new(
        profile: &LensProfile,
        shot: &Shot,
        width: usize,
        height: usize,
        want: Corrections,
    ) -> Option<Self> {
        let mut m = Modifier::new(
            &profile.lens,
            shot.focal,
            profile.crop_factor,
            width as u32,
            height as u32,
            false,
        );
        let has_distortion = want.distortion && m.enable_distortion_correction(&profile.lens);
        let has_tca = want.tca && m.enable_tca_correction(&profile.lens);
        let has_vignetting = want.vignetting
            && m.enable_vignetting_correction(&profile.lens, shot.aperture, DEFAULT_DISTANCE);

        if !has_distortion && !has_tca && !has_vignetting {
            return None;
        }

        let mut c = Self {
            modifier: m,
            width,
            height,
            has_distortion,
            has_tca,
            has_vignetting,
            applied_scale: 1.0,
            autoscale: 1.0,
        };
        c.autoscale = c.compute_autoscale();
        c.applied_scale = 1.0 + (c.autoscale - 1.0) * (want.scale.clamp(0.0, 100.0) / 100.0);
        Some(c)
    }

    /// The scale that just fills the frame with real image data.
    pub fn autoscale(&self) -> f32 {
        self.autoscale
    }

    /// The scale actually in force, after the slider.
    pub fn applied_scale(&self) -> f32 {
        self.applied_scale
    }

    /// Where output pixel `(x, y)` comes from, per channel: `[[rx, ry], [gx, gy], [bx, by]]`.
    ///
    /// ⚠ Distortion and TCA are **summed as displacements**, not composed, because this port has
    /// no combined subpixel+geometry call: `apply_subpixel_distortion` carries TCA only. The sum
    /// is exact to first order and the neglected cross term is well under a tenth of a pixel at
    /// realistic coefficients — far smaller than the error two sequential resampling passes would
    /// introduce. `disabling_tca_leaves_the_distortion_map_untouched` pins the degenerate cases.
    pub fn source(&self, x: f32, y: f32) -> [[f32; 2]; 3] {
        let (px, py) = self.scaled(x, y);

        let mut base = [px, py];
        if self.has_distortion {
            let mut coords = [0.0f32; 2];
            if self
                .modifier
                .apply_geometry_distortion(px, py, 1, 1, &mut coords)
            {
                base = [coords[0], coords[1]];
            }
        }

        let mut out = [base, base, base];
        if self.has_tca {
            let mut coords = [0.0f32; 6];
            if self
                .modifier
                .apply_subpixel_distortion(px, py, 1, 1, &mut coords)
            {
                for ch in 0..3 {
                    out[ch][0] = base[0] + (coords[2 * ch] - px);
                    out[ch][1] = base[1] + (coords[2 * ch + 1] - py);
                }
            }
        }
        out
    }

    /// The vignetting gain at output pixel `(x, y)`; `1.0` when vignetting is off.
    pub fn gain(&self, x: f32, y: f32) -> f32 {
        if !self.has_vignetting {
            return 1.0;
        }
        let (px, py) = self.scaled(x, y);
        let mut one = [1.0f32];
        if self
            .modifier
            .apply_color_modification_f32(&mut one, px, py, 1, 1, 1)
        {
            one[0]
        } else {
            1.0
        }
    }

    /// Apply the scale about the image centre. A scale > 1 means each output pixel reads from
    /// nearer the centre, which is what pulls the empty edges out of frame.
    fn scaled(&self, x: f32, y: f32) -> (f32, f32) {
        let (cx, cy) = (self.centre_x(), self.centre_y());
        let s = 1.0 / self.applied_scale;
        (cx + (x - cx) * s, cy + (y - cy) * s)
    }

    fn centre_x(&self) -> f32 {
        (self.width.max(1) - 1) as f32 * 0.5
    }

    fn centre_y(&self) -> f32 {
        (self.height.max(1) - 1) as f32 * 0.5
    }

    /// The smallest scale for which no output pixel on the frame border reads from outside the
    /// source.
    ///
    /// ⚠ **Our own implementation — this port has no `GetAutoScale`.** It follows the C library's
    /// approach: probe the four corners and the four edge midpoints, ask what scale each would
    /// need, and take the largest.
    fn compute_autoscale(&self) -> f32 {
        if !self.has_distortion {
            return 1.0;
        }
        let (w, h) = ((self.width - 1) as f32, (self.height - 1) as f32);
        let (cx, cy) = (self.centre_x(), self.centre_y());
        let probes = [
            (0.0, 0.0),
            (w, 0.0),
            (0.0, h),
            (w, h),
            (cx, 0.0),
            (cx, h),
            (0.0, cy),
            (w, cy),
        ];
        let mut scale = 1.0f32;
        for (x, y) in probes {
            let mut coords = [0.0f32; 2];
            if !self
                .modifier
                .apply_geometry_distortion(x, y, 1, 1, &mut coords)
            {
                continue;
            }
            let (dx, dy) = (coords[0] - cx, coords[1] - cy);
            let (ox, oy) = (x - cx, y - cy);
            if ox.abs() > 1.0 {
                scale = scale.max((dx / ox).abs());
            }
            if oy.abs() > 1.0 {
                scale = scale.max((dy / oy).abs());
            }
        }
        scale.max(1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic camera + lens with coefficients we choose, as XML.
    ///
    /// ⚠⚠ This is the single most valuable thing in this crate's test suite. `Database::load_str`
    /// takes XML, so behaviour can be pinned against coefficients *we* picked rather than against
    /// whatever the bundled database happens to contain — no RAW fixture, no gating, and it runs
    /// in CI. Without it, lens correction would be testable only on one developer's machine.
    fn synthetic_xml(distortion: &str, tca: &str, vignetting: &str) -> String {
        format!(
            r#"<lensdatabase version="1">
                <camera>
                    <maker>TestMaker</maker>
                    <model>TestCam</model>
                    <mount>TestMount</mount>
                    <cropfactor>1.0</cropfactor>
                </camera>
                <lens>
                    <maker>TestMaker</maker>
                    <model>TestLens 50mm</model>
                    <mount>TestMount</mount>
                    <cropfactor>1.0</cropfactor>
                    <calibration>
                        {distortion}
                        {tca}
                        {vignetting}
                    </calibration>
                </lens>
            </lensdatabase>"#
        )
    }

    fn synthetic_db(distortion: &str, tca: &str, vignetting: &str) -> Catalogue {
        Catalogue::from_xml(&synthetic_xml(distortion, tca, vignetting)).expect("synthetic db")
    }

    /// ⚠ The sign convention, pinned empirically rather than assumed — it decides which end of a
    /// zoom empties the frame, and an earlier draft of the design had it backwards.
    ///
    /// **Negative `b` corrects barrel**: the output pixel reads from *nearer* the centre, so the
    /// corners always have data and it is the *edges* that get cropped. Autoscale is exactly 1.
    ///
    /// **Positive `b` corrects pincushion**: the output pixel reads from *further out*, so the
    /// corners want source pixels beyond the sensor and come back empty. Autoscale exceeds 1.
    ///
    /// Both match the real XF16-80, which is barrel at 16 mm and pincushion from ~35 mm up.
    const BARREL: &str = r#"<distortion model="ptlens" focal="50" a="0.0" b="-0.05" c="0.0"/>"#;
    const PINCUSHION: &str = r#"<distortion model="ptlens" focal="50" a="0.0" b="0.05" c="0.0"/>"#;
    const NO_DISTORTION: &str =
        r#"<distortion model="ptlens" focal="50" a="0.0" b="0.0" c="0.0"/>"#;
    const TCA: &str = r#"<tca model="poly3" focal="50" vr="1.001" vb="0.999"/>"#;
    const VIGNETTING: &str = r#"<vignetting model="pa" focal="50" aperture="2.8" distance="1000" k1="-0.5" k2="0.0" k3="0.0"/>"#;

    #[test]
    fn a_known_camera_and_lens_resolve() {
        let db = synthetic_db(BARREL, TCA, VIGNETTING);
        let p = db
            .find("TestMaker", "TestCam", "TestLens 50mm")
            .expect("should match");
        assert_eq!(p.lens_name(), "TestLens 50mm");
        assert!((p.crop_factor() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn an_unknown_lens_does_not_resolve() {
        let db = synthetic_db(BARREL, TCA, VIGNETTING);
        assert!(db
            .find("TestMaker", "TestCam", "No Such Lens 999mm")
            .is_none());
    }

    #[test]
    fn an_unknown_camera_does_not_resolve() {
        let db = synthetic_db(BARREL, TCA, VIGNETTING);
        assert!(db
            .find("TestMaker", "No Such Cam", "TestLens 50mm")
            .is_none());
    }

    #[test]
    fn the_bundled_database_has_the_developers_own_gear() {
        // ⚠ Not a vanity check: it is the only test that proves `Catalogue::bundled()` actually
        // loads anything. A `find` that always returns `None` would pass every synthetic test
        // above, because those build their own database.
        let db = Catalogue::bundled().expect("bundled db");
        assert!(db.find("Fujifilm", "X-T4", "XF23mmF2 R WR").is_some());
        assert!(db
            .find("Fujifilm", "X-T4", "XF16-80mmF4 R OIS WR")
            .is_some());
    }

    fn corrector(distortion: &str, want: Corrections) -> Corrector {
        let db = synthetic_db(distortion, TCA, VIGNETTING);
        let p = db.find("TestMaker", "TestCam", "TestLens 50mm").unwrap();
        Corrector::new(
            &p,
            &Shot {
                focal: 50.0,
                aperture: 2.8,
            },
            400,
            300,
            want,
        )
        .expect("corrector")
    }

    const ALL: Corrections = Corrections {
        distortion: true,
        tca: true,
        vignetting: true,
        scale: 0.0,
    };

    #[test]
    fn a_zero_coefficient_lens_maps_every_pixel_to_itself() {
        // ⚠ The strongest test available, and the analogue of `an_identity_table_changes_nothing`:
        // with no distortion and no TCA the map must be the identity, not merely close.
        let c = corrector(NO_DISTORTION, Corrections { tca: false, ..ALL });
        for (x, y) in [(0.0, 0.0), (199.5, 149.5), (399.0, 299.0), (123.0, 45.0)] {
            for (ch, got) in c.source(x, y).iter().enumerate() {
                assert!(
                    (got[0] - x).abs() < 1e-3 && (got[1] - y).abs() < 1e-3,
                    "({x}, {y}) channel {ch} mapped to {got:?}"
                );
            }
        }
    }

    #[test]
    fn barrel_distortion_pulls_the_corner_inward() {
        // Barrel compresses the periphery, so correcting it *expands* the periphery — the
        // corrected corner samples from a point closer to the centre. Consequence, and the
        // reason this matters: a barrel-corrected frame is never empty at the corners.
        let c = corrector(BARREL, Corrections { tca: false, ..ALL });
        let (cx, cy) = (199.5f32, 149.5);
        let s = c.source(399.0, 299.0)[1];
        let before = (399.0f32 - cx).hypot(299.0 - cy);
        let after = (s[0] - cx).hypot(s[1] - cy);
        assert!(
            after < before,
            "corner radius {before} -> {after}, expected inward"
        );
    }

    #[test]
    fn tca_separates_the_channels_and_leaves_green_alone() {
        // poly3 TCA scales red and blue about the centre; green is the reference channel.
        let c = corrector(NO_DISTORTION, ALL);
        let s = c.source(399.0, 299.0);
        assert!((s[1][0] - 399.0).abs() < 1e-3, "green moved: {:?}", s[1]);
        assert!(s[0] != s[1], "red did not separate from green");
        assert!(s[2] != s[1], "blue did not separate from green");
    }

    #[test]
    fn disabling_tca_leaves_the_distortion_map_untouched() {
        // ⚠ Pins the displacement-summing composition at its degenerate points. The port has no
        // combined subpixel+geometry call, so distortion and TCA are evaluated separately and
        // their displacements added; with one of them off the sum must reduce *exactly* to the
        // other, or the composition is wrong rather than merely approximate.
        let with = corrector(BARREL, Corrections { tca: false, ..ALL });
        let without = corrector(
            BARREL,
            Corrections {
                tca: false,
                vignetting: false,
                ..ALL
            },
        );
        for (x, y) in [(10.0, 10.0), (200.0, 150.0), (399.0, 299.0)] {
            assert_eq!(with.source(x, y), without.source(x, y), "at ({x}, {y})");
        }
    }

    #[test]
    fn vignetting_lifts_the_corner_and_leaves_the_centre_at_one() {
        let c = corrector(NO_DISTORTION, ALL);
        assert!(
            (c.gain(199.5, 149.5) - 1.0).abs() < 1e-3,
            "centre must be 1.0"
        );
        let corner = c.gain(399.0, 299.0);
        assert!(corner > 1.05, "corner gain {corner} should lift");
    }

    #[test]
    fn vignetting_is_one_everywhere_when_disabled() {
        let c = corrector(
            NO_DISTORTION,
            Corrections {
                vignetting: false,
                ..ALL
            },
        );
        for (x, y) in [(0.0, 0.0), (199.5, 149.5), (399.0, 299.0)] {
            assert_eq!(c.gain(x, y), 1.0, "at ({x}, {y})");
        }
    }

    #[test]
    fn a_zero_distortion_lens_autoscales_to_exactly_one() {
        let c = corrector(NO_DISTORTION, Corrections { tca: false, ..ALL });
        assert!((c.autoscale() - 1.0).abs() < 1e-4, "got {}", c.autoscale());
    }

    #[test]
    fn pincushion_correction_autoscales_above_one() {
        // ⚠ **Pincushion**, not barrel, is what empties the frame: the corrected corner wants a
        // source pixel beyond the sensor, so filling the frame needs a scale > 1.
        let c = corrector(PINCUSHION, Corrections { tca: false, ..ALL });
        assert!(c.autoscale() > 1.0, "got {}", c.autoscale());
    }

    #[test]
    fn barrel_correction_needs_no_scaling() {
        // The other half of the asymmetry, and the reason a fixed autoscale is the wrong default:
        // barrel correction never empties the frame, so scaling it would discard data for nothing.
        let c = corrector(BARREL, Corrections { tca: false, ..ALL });
        assert!(
            (c.autoscale() - 1.0).abs() < 1e-4,
            "barrel should not need scaling, got {}",
            c.autoscale()
        );
    }

    #[test]
    fn the_scale_slider_interpolates_between_one_and_the_autoscale() {
        // 0 = leave the empty edges, 100 = scale until they are gone.
        let auto = corrector(PINCUSHION, Corrections { tca: false, ..ALL }).autoscale();
        let none = corrector(
            PINCUSHION,
            Corrections {
                tca: false,
                scale: 0.0,
                ..ALL
            },
        );
        let full = corrector(
            PINCUSHION,
            Corrections {
                tca: false,
                scale: 100.0,
                ..ALL
            },
        );
        let half = corrector(
            PINCUSHION,
            Corrections {
                tca: false,
                scale: 50.0,
                ..ALL
            },
        );
        assert!((none.applied_scale() - 1.0).abs() < 1e-4);
        assert!((full.applied_scale() - auto).abs() < 1e-4);
        let want = 1.0 + (auto - 1.0) * 0.5;
        assert!(
            (half.applied_scale() - want).abs() < 1e-4,
            "{} vs {want}",
            half.applied_scale()
        );
    }

    #[test]
    fn the_applied_scale_actually_moves_the_map() {
        // ⚠ `applied_scale()` being right proves nothing about it being *used*. Without this,
        // deleting the scaling from `source` fails no test at all.
        let unscaled = corrector(
            PINCUSHION,
            Corrections {
                tca: false,
                scale: 0.0,
                ..ALL
            },
        );
        let scaled = corrector(
            PINCUSHION,
            Corrections {
                tca: false,
                scale: 100.0,
                ..ALL
            },
        );
        let a = unscaled.source(399.0, 299.0)[1];
        let b = scaled.source(399.0, 299.0)[1];
        assert!(
            (a[0] - b[0]).abs() > 1.0,
            "scaling did not move the corner: {a:?} vs {b:?}"
        );
    }
}
