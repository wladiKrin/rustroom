//! Lens profiles: matching a camera and lens against LensFun's database, and turning a match
//! into the per-pixel corrections the pipeline applies.
//!
//! ⚠ This crate exists so that the `lensfun` dependency has exactly one home, mirroring
//! `camera-profile`'s relationship with `.dcp` files. `pipeline` must never learn where a
//! profile comes from.

use lensfun::{Database, Lens};

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

    /// A barrel distortion strong enough to be unmistakable.
    const BARREL: &str = r#"<distortion model="ptlens" focal="50" a="0.0" b="0.05" c="0.0"/>"#;
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
}
