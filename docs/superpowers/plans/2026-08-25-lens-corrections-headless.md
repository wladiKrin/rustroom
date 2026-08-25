# Lens corrections (M3.2), plan 1 of 2 — headless

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Distortion, TCA and vignetting corrected from a lens profile, rendering correctly through the CLI and the golden harness, with no GUI code in existence.

**Architecture:** A new `lens-profile` crate owns the `lensfun` dependency, matches a camera+lens against the bundled database, and exposes a `Corrector` that answers two questions per pixel: where does this pixel come from, and what gain does it need. `pipeline` gains a `LensCorrectionStage` at the **front of the prefix**, before `GeometryStage`, because the calibration is defined in the uncropped sensor frame.

**Tech Stack:** Rust, `lensfun = "0.7"` (pure Rust, zero deps, bundled DB, LGPL-3.0-or-later).

**Spec:** `docs/superpowers/specs/2026-08-25-lens-corrections-design.md`

**Build environment** — every `cargo` command needs:

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

⚠ **`brew install lensfun` is NOT required and must not be added to the setup instructions.** The C library was installed during probing only, to compare against; the crate is pure Rust and bundles its own database. If anything in this plan appears to need pkg-config for lensfun, something has gone wrong.

**Standing rules:** no `Co-Authored-By` trailer; never push; sabotage every new test and **confirm the sabotage compiled**; `cargo clippy --all-targets --all-features -- -D warnings` clean, checked by **exit code**, never through `grep`.

⚠ **A zsh gotcha that cost time during probing:** unquoted `$VAR` does **not** word-split in zsh. To pass several paths from a variable, write `${=FILES}`. And `ls` is aliased with colour here — use `find` when capturing paths into a variable.

---

### Task 1: The `lens-profile` crate — matching

**Files:**
- Create: `crates/lens-profile/Cargo.toml`, `crates/lens-profile/src/lib.rs`
- Modify: `Cargo.toml` (workspace members)

- [ ] **Step 1: Create the crate**

`crates/lens-profile/Cargo.toml`:

```toml
[package]
name = "lens-profile"
version = "0.1.0"
edition = "2021"
license = "GPL-3.0-or-later"

[dependencies]
lensfun = "0.7"
```

Add `"crates/lens-profile"` to the workspace `members` list in the root `Cargo.toml`, keeping the existing order/style.

- [ ] **Step 2: Write the failing tests**

`crates/lens-profile/src/lib.rs` — start with only the tests plus the imports:

```rust
//! Lens profiles: matching a camera and lens against LensFun's database, and turning a match
//! into the per-pixel corrections the pipeline applies.
//!
//! ⚠ This crate exists so that the `lensfun` dependency has exactly one home, mirroring
//! `camera-profile`'s relationship with `.dcp` files. `pipeline` must never learn where a
//! profile comes from.

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
    const NO_DISTORTION: &str = r#"<distortion model="ptlens" focal="50" a="0.0" b="0.0" c="0.0"/>"#;
    const TCA: &str = r#"<tca model="poly3" focal="50" vr="1.001" vb="0.999"/>"#;
    const VIGNETTING: &str =
        r#"<vignetting model="pa" focal="50" aperture="2.8" distance="1000" k1="-0.5" k2="0.0" k3="0.0"/>"#;

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
        assert!(db.find("TestMaker", "TestCam", "No Such Lens 999mm").is_none());
    }

    #[test]
    fn an_unknown_camera_does_not_resolve() {
        let db = synthetic_db(BARREL, TCA, VIGNETTING);
        assert!(db.find("TestMaker", "No Such Cam", "TestLens 50mm").is_none());
    }

    #[test]
    fn the_bundled_database_has_the_developers_own_gear() {
        // ⚠ Not a vanity check: it is the only test that proves `Catalogue::bundled()` actually
        // loads anything. A `find` that always returns `None` would pass every synthetic test
        // above, because those build their own database.
        let db = Catalogue::bundled().expect("bundled db");
        assert!(db.find("Fujifilm", "X-T4", "XF23mmF2 R WR").is_some());
        assert!(db.find("Fujifilm", "X-T4", "XF16-80mmF4 R OIS WR").is_some());
    }
}
```

- [ ] **Step 3: Run to verify it fails**

```bash
cargo test -p lens-profile 2>&1 | tail -10
```

Expected: compile error, `cannot find type Catalogue in this scope`.

- [ ] **Step 4: Implement the matching half**

Insert above the `#[cfg(test)]` module in `crates/lens-profile/src/lib.rs`:

```rust
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
```

⚠ If `lens.model` is not the field name, run `grep -n "pub " crates/../lensfun-0.7.0/src/lens.rs` and use the actual one; do not guess twice.

- [ ] **Step 5: Run the tests**

```bash
cargo test -p lens-profile 2>&1 | grep -E "^test |test result:" | head
```

Expected: four PASS.

- [ ] **Step 6: Sabotage — drop the camera from the match**

Replace the body of `find` with a version that never looks the camera up:

```rust
    pub fn find(&self, _make: &str, _model: &str, lens_model: &str) -> Option<LensProfile> {
        let lenses = self.db.find_lenses(None, lens_model);
        let lens = lenses.first()?;
        Some(LensProfile {
            lens: (*lens).clone(),
            crop_factor: 1.0,
        })
    }
```

Confirm it **compiles**, then run:

```bash
cargo test -p lens-profile 2>&1 | grep -E "FAILED|test result:"
```

Expected: `an_unknown_camera_does_not_resolve` FAILS — an unknown camera now resolves happily because only the lens name is consulted. Restore.

- [ ] **Step 7: Commit**

```bash
git add crates/lens-profile Cargo.toml Cargo.lock
git commit -m "feat(lens-profile): match a camera and lens against LensFun"
```

---

### Task 2: The `Corrector` — displacements, gain and autoscale

**Files:**
- Modify: `crates/lens-profile/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Add to the `tests` module:

```rust
    fn corrector(distortion: &str, want: Corrections) -> Corrector {
        let db = synthetic_db(distortion, TCA, VIGNETTING);
        let p = db.find("TestMaker", "TestCam", "TestLens 50mm").unwrap();
        Corrector::new(&p, &Shot { focal: 50.0, aperture: 2.8 }, 400, 300, want)
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
        // with no distortion and no TCA the map must be the identity to floating-point exactness,
        // not merely close.
        let c = corrector(
            NO_DISTORTION,
            Corrections {
                tca: false,
                ..ALL
            },
        );
        for (x, y) in [(0.0, 0.0), (199.5, 149.5), (399.0, 299.0), (123.0, 45.0)] {
            let s = c.source(x, y);
            for ch in 0..3 {
                assert!(
                    (s[ch][0] - x).abs() < 1e-3 && (s[ch][1] - y).abs() < 1e-3,
                    "({x}, {y}) channel {ch} mapped to {:?}",
                    s[ch]
                );
            }
        }
    }

    #[test]
    fn barrel_distortion_pulls_the_corner_inward() {
        // Correcting barrel stretches the corners outward, so the corrected corner must sample
        // from a source point *closer to the centre* than itself.
        let c = corrector(BARREL, Corrections { tca: false, ..ALL });
        let (cx, cy) = (199.5f32, 149.5);
        let s = c.source(399.0, 299.0)[1];
        let before = ((399.0f32 - cx).hypot(299.0 - cy)).abs();
        let after = ((s[0] - cx).hypot(s[1] - cy)).abs();
        assert!(after < before, "corner radius {before} -> {after}, expected inward");
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
        assert!((c.gain(199.5, 149.5) - 1.0).abs() < 1e-3, "centre must be 1.0");
        let corner = c.gain(399.0, 299.0);
        assert!(corner > 1.05, "corner gain {corner} should lift");
    }

    #[test]
    fn vignetting_is_one_everywhere_when_disabled() {
        let c = corrector(NO_DISTORTION, Corrections { vignetting: false, ..ALL });
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
    fn barrel_distortion_autoscales_above_one() {
        // Barrel correction empties the edges, so filling the frame needs a scale > 1.
        let c = corrector(BARREL, Corrections { tca: false, ..ALL });
        assert!(c.autoscale() > 1.0, "got {}", c.autoscale());
    }

    #[test]
    fn the_scale_slider_interpolates_between_one_and_the_autoscale() {
        // 0 = leave the empty edges, 100 = scale until they are gone.
        let auto = corrector(BARREL, Corrections { tca: false, ..ALL }).autoscale();
        let none = corrector(BARREL, Corrections { tca: false, scale: 0.0, ..ALL });
        let full = corrector(BARREL, Corrections { tca: false, scale: 100.0, ..ALL });
        let half = corrector(BARREL, Corrections { tca: false, scale: 50.0, ..ALL });
        assert!((none.applied_scale() - 1.0).abs() < 1e-4);
        assert!((full.applied_scale() - auto).abs() < 1e-4);
        let want = 1.0 + (auto - 1.0) * 0.5;
        assert!((half.applied_scale() - want).abs() < 1e-4, "{} vs {want}", half.applied_scale());
    }
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test -p lens-profile 2>&1 | grep -E "^error" | head -4
```

Expected: `cannot find type Corrector` / `Corrections` / `Shot`.

- [ ] **Step 3: Implement the `Corrector`**

Add to `crates/lens-profile/src/lib.rs`, above the tests:

```rust
use lensfun::Modifier;

/// Subject distance handed to the vignetting interpolation, in metres.
///
/// The database tabulates vignetting at 10 m and 1000 m, and for the lenses checked during
/// design the coefficients are identical at both — the distance term matters for macro work and
/// little else. We have no subject-distance metadata, so this picks the far end.
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
        let mut build = |scale_probe: bool| {
            let mut m = Modifier::new(
                &profile.lens,
                shot.focal,
                profile.crop_factor,
                width as u32,
                height as u32,
                false,
            );
            let d = want.distortion && m.enable_distortion_correction(&profile.lens);
            let t = want.tca && m.enable_tca_correction(&profile.lens);
            let v = !scale_probe
                && want.vignetting
                && m.enable_vignetting_correction(&profile.lens, shot.aperture, DEFAULT_DISTANCE);
            (m, d, t, v)
        };

        let (modifier, has_distortion, has_tca, has_vignetting) = build(false);
        if !has_distortion && !has_tca && !has_vignetting {
            return None;
        }

        let mut c = Self {
            modifier,
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
    /// realistic coefficients — far smaller than the error two sequential resampling passes
    /// would introduce. `disabling_tca_leaves_the_distortion_map_untouched` pins the degenerate
    /// cases exactly.
    pub fn source(&self, x: f32, y: f32) -> [[f32; 2]; 3] {
        // Scaling is about the image centre: a scale > 1 means each output pixel reads from
        // nearer the centre, which is what pulls the empty edges out of frame.
        let (cx, cy) = (self.centre_x(), self.centre_y());
        let s = 1.0 / self.applied_scale;
        let (px, py) = (cx + (x - cx) * s, cy + (y - cy) * s);

        let mut base = [px, py];
        if self.has_distortion {
            let mut coords = [0.0f32; 2];
            if self.modifier.apply_geometry_distortion(px, py, 1, 1, &mut coords) {
                base = [coords[0], coords[1]];
            }
        }

        let mut out = [base, base, base];
        if self.has_tca {
            let mut coords = [0.0f32; 6];
            if self.modifier.apply_subpixel_distortion(px, py, 1, 1, &mut coords) {
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
        let (cx, cy) = (self.centre_x(), self.centre_y());
        let s = 1.0 / self.applied_scale;
        let (px, py) = (cx + (x - cx) * s, cy + (y - cy) * s);
        let mut px1 = [1.0f32];
        if self
            .modifier
            .apply_color_modification_f32(&mut px1, px, py, 1, 1, 1)
        {
            px1[0]
        } else {
            1.0
        }
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
    /// ⚠ **Our own implementation — this port has no `GetAutoScale`.** It follows the C
    /// library's approach: probe the four corners and the four edge midpoints, ask what scale
    /// each would need, and take the largest.
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
            // How far outside the frame did this probe land, as a fraction of its own offset?
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
```

- [ ] **Step 4: Run the tests**

```bash
cargo test -p lens-profile 2>&1 | grep -E "^test |test result:" | head -20
```

Expected: thirteen PASS. ⚠ If `barrel_distortion_pulls_the_corner_inward` fails with the radius *growing*, the `ptlens` sign convention is the opposite of what `BARREL` assumes — flip the `b` coefficient's sign in the constant and say so in a comment, rather than inverting the production maths.

- [ ] **Step 5: Sabotage 1 — drop the scaling from `source`**

Change `let s = 1.0 / self.applied_scale;` to `let s = 1.0;` in `source`. Confirm it compiles, then run.

Expected: `the_scale_slider_interpolates_between_one_and_the_autoscale` still passes (it reads `applied_scale`, not the map), while nothing else fails — **this is a real gap, so add the test that closes it** before moving on:

```rust
    #[test]
    fn the_applied_scale_actually_moves_the_map() {
        // ⚠ `applied_scale()` being right proves nothing about it being *used*. Without this,
        // deleting the scaling from `source` fails no test at all.
        let unscaled = corrector(BARREL, Corrections { tca: false, scale: 0.0, ..ALL });
        let scaled = corrector(BARREL, Corrections { tca: false, scale: 100.0, ..ALL });
        let a = unscaled.source(399.0, 299.0)[1];
        let b = scaled.source(399.0, 299.0)[1];
        assert!(
            (a[0] - b[0]).abs() > 1.0,
            "scaling did not move the corner: {a:?} vs {b:?}"
        );
    }
```

Re-run with the sabotage still in place: it must now FAIL. Then restore and confirm all fourteen pass.

- [ ] **Step 6: Sabotage 2 — return 1.0 from `compute_autoscale`**

Replace its body with `1.0`. Confirm it compiles, then run.

Expected: `barrel_distortion_autoscales_above_one`, `the_scale_slider_interpolates_between_one_and_the_autoscale` and `the_applied_scale_actually_moves_the_map` FAIL. Restore.

- [ ] **Step 7: Lint and commit**

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings > /tmp/c.log 2>&1; echo "clippy exit: $?"
git add -A
git commit -m "feat(lens-profile): distortion, TCA, vignetting and autoscale"
```

---

### Task 3: Lens metadata from the decoder

**Files:**
- Modify: `crates/raw-decode/src/lib.rs` (`RawMetadata`)
- Modify: `crates/raw-decode/src/backend_rsraw.rs`
- Keep: `crates/raw-decode/examples/lens_probe.rs` (already written during design; commit it here)

- [ ] **Step 1: Add the type and the field**

In `crates/raw-decode/src/lib.rs`, above `RawMetadata`:

```rust
/// What the camera recorded about the lens, when it recorded anything.
///
/// ⚠ Deliberately does **not** carry `rsraw::LensInfo`'s `min_focal`/`max_focal` or `mounts`.
/// Probing showed the focal range reporting `16-16` for a 16-80 zoom and the mount returning the
/// garbage string `"IL UM"`. The per-shot `focal` below comes from `raw.focal_len()`, which is
/// correct, and the mount is not needed because matching goes through the camera.
#[derive(Debug, Clone, PartialEq)]
pub struct LensMetadata {
    /// e.g. `"FUJIFILM"`.
    pub make: String,
    /// e.g. `"XF16-80mmF4 R OIS WR"` — matches LensFun's `<model>` verbatim for the lenses checked.
    pub name: String,
    /// Focal length of *this shot*, in mm.
    pub focal: f32,
    /// f-number of this shot.
    pub aperture: f32,
}
```

and add to `RawMetadata`:

```rust
    /// `None` when the file records no usable lens name.
    pub lens: Option<LensMetadata>,
```

- [ ] **Step 2: Populate it**

In `crates/raw-decode/src/backend_rsraw.rs`, where `RawMetadata` is constructed, add:

```rust
        lens: {
            let l = processed_raw.lens_info();
            let name = l.lens_name.trim().to_string();
            // An empty name is the ordinary case for an adapted or unchipped lens, and must
            // stay `None` rather than becoming an unmatchable empty string.
            if name.is_empty() {
                None
            } else {
                Some(crate::LensMetadata {
                    make: l.lens_make.trim().to_string(),
                    name,
                    focal: processed_raw.focal_len(),
                    aperture: processed_raw.aperture(),
                })
            }
        },
```

⚠ `processed_raw` is whatever the local binding for the `rsraw::RawImage` is called in that function — use the existing name. `lens_info()` must be read while the `RawImage` is still alive, like `rgb_cam`.

- [ ] **Step 3: Build and fix the call sites**

```bash
cargo build --all 2>&1 | grep -E "^error" -A 5 | head -20
```

Expected: `missing field lens` wherever `RawMetadata` is constructed in tests. Add `lens: None` to each.

- [ ] **Step 4: Verify against real files**

```bash
FILES=$(find "$HOME/Desktop/fuji" -name '*.RAF' | sort | sed -n '1p;90p')
cargo run --release -q -p raw-decode --example lens_probe -- ${=FILES} 2>&1 | head -14
```

Expected: `XF23mmF2 R WR` and `XF16-80mmF4 R OIS WR` with sensible focal and aperture. ⚠ Note `${=FILES}` — zsh does not word-split otherwise.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(raw-decode): surface the lens the shot was taken with"
```

---

### Task 4: The `Edits` type

**Files:**
- Modify: `crates/edit-model/src/lib.rs`
- Modify: `crates/pipeline/src/lib.rs` (`all_edits` fixture)
- Modify: `crates/app/src/edit_clipboard.rs` (payload fixture)

- [ ] **Step 1: Write the failing tests**

Add to `edit-model`'s test module:

```rust
    #[test]
    fn a_sidecar_without_scale_still_gets_a_hundred() {
        // ⚠ Third instance of this trap, after `RadialGradient` and `ColorGrading`: `scale`
        // is 100 at neutral, so `Default` must be hand-written. A derived one would give every
        // partial sidecar zero scale — black edges — silently.
        let e: Edits =
            serde_json::from_str(r#"{"lens_correction":{"distortion":true}}"#).unwrap();
        assert_eq!(e.lens_correction.scale, 100.0);
        assert!(e.lens_correction.distortion);
    }

    #[test]
    fn the_scale_alone_is_inert() {
        let l = LensCorrection {
            scale: 0.0,
            ..LensCorrection::default()
        };
        assert!(!l.is_active());
    }

    #[test]
    fn any_enabled_correction_is_active() {
        for l in [
            LensCorrection { distortion: true, ..LensCorrection::default() },
            LensCorrection { tca: true, ..LensCorrection::default() },
            LensCorrection { vignetting: true, ..LensCorrection::default() },
        ] {
            assert!(l.is_active(), "{l:?} should be active");
        }
    }

    #[test]
    fn lens_correction_is_neutral_by_default() {
        // ⚠ Load-bearing. The GUI turns these on by preference for photos with no sidecar; the
        // op-list default must stay neutral so the CLI, the goldens and every existing sidecar
        // are untouched.
        assert!(!LensCorrection::default().is_active());
    }
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test -p edit-model 2>&1 | grep -E "^error" | head -3
```

Expected: `cannot find type LensCorrection`.

- [ ] **Step 3: Add the type**

In `crates/edit-model/src/lib.rs`, after the `impl Sharpening` block:

```rust
/// Optical corrections read from a lens profile. Neutral when nothing is enabled.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LensCorrection {
    /// Correct geometric distortion (barrel / pincushion).
    pub distortion: bool,
    /// Correct transverse chromatic aberration.
    pub tca: bool,
    /// Correct light falloff toward the corners.
    pub vignetting: bool,
    /// How far to scale the corrected image, 0..100. 0 leaves the empty edges that distortion
    /// correction produces at the wide end; 100 scales until they are gone.
    pub scale: f32,
}

impl Default for LensCorrection {
    /// ⚠ Hand-written, **not** derived: `scale` is 100 at neutral, and the container-level
    /// `#[serde(default)]` fills a missing field from this. A derived `Default` would give every
    /// partial sidecar `scale: 0`, silently.
    fn default() -> Self {
        Self {
            distortion: false,
            tca: false,
            vignetting: false,
            scale: 100.0,
        }
    }
}

impl LensCorrection {
    /// True when this actually changes pixels.
    ///
    /// The three corrections only; `scale` is a modifier that does nothing until one of them is
    /// on — the rule [`NoiseReduction::is_active`] and [`Vignette::is_active`] follow.
    pub fn is_active(&self) -> bool {
        self.distortion || self.tca || self.vignetting
    }
}
```

Add the field to `Edits`, immediately after `pub sharpening: Sharpening,`:

```rust
    pub lens_correction: LensCorrection,
```

- [ ] **Step 4: Assign the group**

Add `lens_correction,` to `merge_from`'s destructuring pattern after `sharpening,`, and to the **Detail** block:

```rust
        if groups.detail {
            self.noise_reduction = *noise_reduction;
            self.sharpening = *sharpening;
            self.lens_correction = *lens_correction;
        }
```

⚠ Detail is this codebase's capture-specific group — *"noise, lens, output size"* — so lens correction belongs there and no new group is needed.

- [ ] **Step 5: Repair the three exhaustive fixtures**

They are exhaustive on purpose; list the field rather than switching them to `..Default::default()`.

In `edit-model`'s `all_nonneutral()`, after `sharpening: ...`:

```rust
            lens_correction: LensCorrection {
                distortion: true,
                tca: true,
                vignetting: false,
                scale: 60.0,
            },
```

In `crates/pipeline/src/lib.rs`'s `all_edits()`:

```rust
            lens_correction: edit_model::LensCorrection {
                distortion: true,
                tca: false,
                vignetting: true,
                scale: 80.0,
            },
```

In `crates/app/src/edit_clipboard.rs`'s payload fixture:

```rust
            lens_correction: edit_model::LensCorrection {
                distortion: true,
                ..Default::default()
            },
```

- [ ] **Step 6: Run the whole suite**

```bash
cargo test --all 2>&1 | grep -cE "test result: FAILED"
```

Expected: `0`. All 43 goldens still pass — no stage exists yet.

- [ ] **Step 7: Sabotage — derive `Default`**

Add `Default` to the derive list and delete the hand-written impl. Confirm it compiles, then:

```bash
cargo test -p edit-model a_sidecar_without_scale 2>&1 | grep -E "left|right|test result"
```

Expected: FAIL with `left: 0.0, right: 100.0`. Restore.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat(edit-model): lens corrections in the op-list"
```

---

### Task 5: The pipeline stage

**Files:**
- Create: `crates/pipeline/src/stages/lens_correction.rs`
- Modify: `crates/pipeline/src/stages/mod.rs`, `crates/pipeline/src/lib.rs`, `crates/pipeline/Cargo.toml`

- [ ] **Step 1: Add the dependency**

In `crates/pipeline/Cargo.toml`, add to `[dependencies]`:

```toml
lens-profile = { path = "../lens-profile" }
```

- [ ] **Step 2: Write the stage**

Create `crates/pipeline/src/stages/lens_correction.rs`:

```rust
//! Optical corrections from a lens profile: distortion, TCA and vignetting.
//!
//! ⚠ Runs **first in the prefix, before `GeometryStage`**, because the calibration is defined in
//! the uncropped, unrotated sensor frame. Being in the prefix also means it always runs
//! whole-buffer, so it needs no `Region` and no kernel margin despite being a spatial resample.

use crate::PipelineStage;
use core_image::ImageBuf;
use lens_profile::Corrector;

/// Bilinearly sample `src` at fractional `(sx, sy)`, one channel; outside the frame reads black.
///
/// ⚠ Per channel rather than per pixel, because TCA gives each channel its own source point.
fn sample_channel(src: &ImageBuf, sx: f32, sy: f32, ch: usize) -> f32 {
    let (w, h) = (src.width() as i32, src.height() as i32);
    let (x0, y0) = (sx.floor() as i32, sy.floor() as i32);
    let (fx, fy) = (sx - x0 as f32, sy - y0 as f32);
    let at = |x: i32, y: i32| -> f32 {
        if x < 0 || y < 0 || x >= w || y >= h {
            0.0
        } else {
            src.pixel(x as usize, y as usize)[ch]
        }
    };
    let top = at(x0, y0) * (1.0 - fx) + at(x0 + 1, y0) * fx;
    let bot = at(x0, y0 + 1) * (1.0 - fx) + at(x0 + 1, y0 + 1) * fx;
    top * (1.0 - fy) + bot * fy
}

pub struct LensCorrectionStage {
    corrector: Corrector,
}

impl LensCorrectionStage {
    pub fn new(corrector: Corrector) -> Self {
        Self { corrector }
    }
}

impl PipelineStage for LensCorrectionStage {
    fn name(&self) -> &'static str {
        "lens_correction"
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        let (w, h) = (image.width(), image.height());
        let mut out = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let (fx, fy) = (x as f32, y as f32);
                let src = self.corrector.source(fx, fy);
                let gain = self.corrector.gain(fx, fy);
                let mut px = [0.0f32; 3];
                for ch in 0..3 {
                    px[ch] = sample_channel(&image, src[ch][0], src[ch][1], ch) * gain;
                }
                out.set_pixel(x, y, px);
            }
        }
        out
    }
}
```

Add `pub mod lens_correction;` to `crates/pipeline/src/stages/mod.rs`, alphabetically after `pub mod geometry;`.

- [ ] **Step 3: Wire it into the prefix**

In `crates/pipeline/src/lib.rs`, add the export beside the others:

```rust
pub use stages::lens_correction::LensCorrectionStage;
```

Add the key variant to `PrefixStepKey`:

```rust
    LensCorrection(edit_model::LensCorrection),
```

⚠ The lens *metadata* is deliberately absent from the key: the render engine is rebuilt per photo, so per-photo constants cannot vary within one cache.

`build_prefix_steps` needs the corrector, which the caller supplies. Change its signature and add the step **first**:

```rust
pub fn build_prefix_steps(
    edits: &Edits,
    scale: RenderScale,
    lens: Option<lens_profile::Corrector>,
) -> Vec<PrefixStep> {
    let mut steps: Vec<PrefixStep> = Vec::new();

    // ⚠ First, before geometry: the calibration is defined in the uncropped, unrotated sensor
    // frame, so it must precede both the crop and the straighten.
    if edits.lens_correction.is_active() {
        if let Some(c) = lens {
            steps.push(PrefixStep {
                key: PrefixStepKey::LensCorrection(edits.lens_correction),
                stage: Box::new(LensCorrectionStage::new(c)),
            });
        }
    }
    // ... existing geometry / noise_reduction / dehaze steps unchanged ...
```

Update `PrefixKey::new` to pass `None`, and add a delegating wrapper so no existing caller churns — the `Region` and `BrushCoverage` precedent:

```rust
impl PrefixKey {
    pub fn new(edits: &Edits, scale: RenderScale) -> Self {
        let steps = build_prefix_steps(edits, scale, None);
        Self::upto(&steps, steps.len(), scale)
    }
}
```

⚠ `PrefixKey::new` passing `None` is **correct and must not be "fixed"**: the key is derived from the *edits*, and `LensCorrection`'s key variant carries them. A corrector is only needed to build the stage, never to key it. But note the consequence, and add a test for it in Step 5: with a corrector absent the step is skipped entirely, so `PrefixKey::new` and the real prefix can disagree in *length*. That is acceptable because a missing corrector is constant for a photo, but it must be stated rather than discovered.

Thread `lens` through **`build_pipeline_split_in_region_with_brush`** as a sixth parameter
(`lens: Option<lens_profile::Corrector>`) and give the existing wrappers `None`.

⚠ **Extend that function rather than adding a seventh entry point.** Six parameters is still under
clippy's `too_many_arguments` threshold of seven; a `FrameSpec`-style struct is the answer *when*
that fires, not before. Task 6 calls this exact name — keep the two in step.

- [ ] **Step 4: Build**

```bash
cargo build --all 2>&1 | grep -E "^error" -A 6 | head -30
```

Fix any call sites the compiler names. Expected: clean once the wrappers pass `None`.

- [ ] **Step 5: Write the pipeline tests**

Add to `crates/pipeline/src/lib.rs`'s test module:

```rust
    #[test]
    fn lens_correction_runs_first_in_the_prefix() {
        // Before geometry, because the calibration is defined in the uncropped sensor frame.
        let edits = Edits {
            lens_correction: edit_model::LensCorrection {
                distortion: true,
                ..Default::default()
            },
            geometry: edit_model::Geometry {
                angle: 2.0,
                ..Default::default()
            },
            ..Edits::default()
        };
        let steps = build_prefix_steps(&edits, RenderScale::FULL, test_corrector());
        let names: Vec<&'static str> = steps.iter().map(|s| s.stage.name()).collect();
        assert_eq!(names, vec!["lens_correction", "geometry"]);
    }

    #[test]
    fn lens_correction_is_skipped_when_no_profile_matched() {
        // ⚠ The ordinary case for an unlisted lens, and it must be silent rather than an error.
        let edits = Edits {
            lens_correction: edit_model::LensCorrection {
                distortion: true,
                ..Default::default()
            },
            ..Edits::default()
        };
        let steps = build_prefix_steps(&edits, RenderScale::FULL, None);
        assert!(steps.is_empty());
    }

    #[test]
    fn the_scale_alone_inserts_no_stage() {
        let edits = Edits {
            lens_correction: edit_model::LensCorrection {
                scale: 0.0,
                ..Default::default()
            },
            ..Edits::default()
        };
        assert!(build_prefix_steps(&edits, RenderScale::FULL, test_corrector()).is_empty());
    }
```

with this helper in the same module:

```rust
    /// A corrector built from a synthetic profile — no RAW file, no bundled database.
    fn test_corrector() -> Option<lens_profile::Corrector> {
        let xml = r#"<lensdatabase version="1">
            <camera><maker>TestMaker</maker><model>TestCam</model>
                <mount>TestMount</mount><cropfactor>1.0</cropfactor></camera>
            <lens><maker>TestMaker</maker><model>TestLens 50mm</model>
                <mount>TestMount</mount><cropfactor>1.0</cropfactor>
                <calibration>
                    <distortion model="ptlens" focal="50" a="0.0" b="0.05" c="0.0"/>
                </calibration></lens>
        </lensdatabase>"#;
        let db = lens_profile::Catalogue::from_xml(xml).unwrap();
        let p = db.find("TestMaker", "TestCam", "TestLens 50mm").unwrap();
        lens_profile::Corrector::new(
            &p,
            &lens_profile::Shot { focal: 50.0, aperture: 2.8 },
            64,
            16,
            lens_profile::Corrections {
                distortion: true,
                tca: false,
                vignetting: false,
                scale: 100.0,
            },
        )
    }
```

Add `lens-profile` to `crates/pipeline/Cargo.toml`'s `[dev-dependencies]` only if it is not already a normal dependency — it is, so nothing extra is needed.

- [ ] **Step 6: Run and sabotage**

```bash
cargo test -p pipeline lens_correction 2>&1 | grep -E "^test |test result:" | head
```

Expected: three PASS.

Sabotage: move the lens-correction block to *after* the geometry block in `build_prefix_steps`. Confirm it compiles, then run.

Expected: `lens_correction_runs_first_in_the_prefix` FAILS with `["geometry", "lens_correction"]`. Restore.

- [ ] **Step 7: Lint and commit**

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings > /tmp/c.log 2>&1; echo "clippy exit: $?"
cargo test --all 2>&1 | grep -cE "test result: FAILED"
git add -A
git commit -m "feat(pipeline): lens correction at the front of the prefix"
```

Expected: `clippy exit: 0`, `0` failures, 43 goldens unchanged.

---

### Task 6: Goldens

**Files:**
- Modify: `crates/pipeline/tests/golden.rs`
- Create: four PNGs under `crates/pipeline/tests/golden/`

- [ ] **Step 1: Add a chart, a synthetic profile and four cases**

In `crates/pipeline/tests/golden.rs`, add after `grade_ramp_chart`:

```rust
/// A grid of straight lines — the only chart on which distortion is legible at all.
///
/// ⚠ A ramp or a colour chart cannot show a geometric correction; straight lines bowing is the
/// whole visible effect. Lines are two pixels wide so a sub-pixel resample cannot erase them.
fn grid_chart() -> ImageBuf {
    const GW: usize = 96;
    const GH: usize = 64;
    let mut img = ImageBuf::new(GW, GH);
    for y in 0..GH {
        for x in 0..GW {
            let on_line = x % 16 < 2 || y % 16 < 2;
            let v = if on_line { 0.85 } else { 0.12 };
            set_srgb(&mut img, x, y, [v, v, v]);
        }
    }
    img
}

/// A synthetic lens with coefficients chosen here, not read from the bundled database.
///
/// ⚠ This is what makes lens-correction goldens possible at all: a real profile needs a real
/// camera and lens, which a synthetic chart does not have, and a fixture-gated test is no
/// evidence in CI.
fn lens_corrector(c: lens_profile::Corrections, w: usize, h: usize) -> lens_profile::Corrector {
    let xml = r#"<lensdatabase version="1">
        <camera><maker>TestMaker</maker><model>TestCam</model>
            <mount>TestMount</mount><cropfactor>1.0</cropfactor></camera>
        <lens><maker>TestMaker</maker><model>TestLens 50mm</model>
            <mount>TestMount</mount><cropfactor>1.0</cropfactor>
            <calibration>
                <distortion model="ptlens" focal="50" a="0.0" b="0.05" c="0.0"/>
                <tca model="poly3" focal="50" vr="1.004" vb="0.996"/>
                <vignetting model="pa" focal="50" aperture="2.8" distance="1000"
                            k1="-0.5" k2="0.0" k3="0.0"/>
            </calibration></lens>
    </lensdatabase>"#;
    let db = lens_profile::Catalogue::from_xml(xml).unwrap();
    let p = db.find("TestMaker", "TestCam", "TestLens 50mm").unwrap();
    lens_profile::Corrector::new(
        &p,
        &lens_profile::Shot { focal: 50.0, aperture: 2.8 },
        w,
        h,
        c,
    )
    .expect("corrector")
}

fn check_lens_golden(name: &str, c: lens_profile::Corrections) {
    let chart = grid_chart();
    let (w, h) = (chart.width(), chart.height());
    let edits = Edits {
        lens_correction: edit_model::LensCorrection {
            distortion: c.distortion,
            tca: c.tca,
            vignetting: c.vignetting,
            scale: c.scale,
        },
        ..Edits::default()
    };
    let (prefix, tail) = pipeline::build_pipeline_split_in_region_with_brush(
        &edits,
        RenderScale::FULL,
        None,
        None,
        color::OutputSpace::Srgb,
        Some(lens_corrector(c, w, h)),
    );
    let mut stages = prefix.into_stages();
    stages.extend(tail.into_stages());
    compare_or_update(name, pipeline::Pipeline::new(stages).run(chart));
}

#[test]
fn golden_lens_distortion() {
    check_lens_golden(
        "lens_distortion",
        lens_profile::Corrections { distortion: true, tca: false, vignetting: false, scale: 0.0 },
    );
}

#[test]
fn golden_lens_distortion_scaled() {
    check_lens_golden(
        "lens_distortion_scaled",
        lens_profile::Corrections { distortion: true, tca: false, vignetting: false, scale: 100.0 },
    );
}

#[test]
fn golden_lens_tca() {
    check_lens_golden(
        "lens_tca",
        lens_profile::Corrections { distortion: false, tca: true, vignetting: false, scale: 0.0 },
    );
}

#[test]
fn golden_lens_vignetting() {
    check_lens_golden(
        "lens_vignetting",
        lens_profile::Corrections { distortion: false, tca: false, vignetting: true, scale: 0.0 },
    );
}
```

⚠ The helper is named `lens_corrector` here and `test_corrector` in Task 5's `pipeline` lib tests
— different files, different return types (`Corrector` vs `Option<Corrector>`). Do not unify them;
do not confuse them.

Add `lens-profile` to `crates/pipeline/Cargo.toml`'s `[dev-dependencies]` if the golden test cannot see it, and `edit_model` / `pipeline` imports as needed.

- [ ] **Step 2: Generate and check**

```bash
UPDATE_GOLDEN=1 cargo test -p pipeline --test golden 2>&1 | grep "test result:"
ls crates/pipeline/tests/golden/ | wc -l
git status --short crates/pipeline/tests/golden/ | grep -c "^ M "
```

Expected: 47 files; `0` modified — the 43 existing references must be byte-identical.

- [ ] **Step 3: Confirm the four differ from one another**

```bash
cd crates/pipeline/tests/golden && for a in lens_distortion lens_distortion_scaled lens_tca lens_vignetting; do
  for b in lens_distortion lens_distortion_scaled lens_tca lens_vignetting; do
    [ "$a" \< "$b" ] && { cmp -s $a.png $b.png && echo "IDENTICAL: $a $b" || true; }
  done; done; echo "(no IDENTICAL lines above = all four discriminate)"; cd -
```

- [ ] **Step 4: Look at them**

Open `lens_distortion.png` and confirm the grid lines are visibly *bowed* relative to `neutral`-style straightness, and that `lens_distortion_scaled.png` has no black edge. ⚠ A golden nobody has looked at is just a hash.

- [ ] **Step 5: Commit**

```bash
git add crates/pipeline/tests/golden.rs crates/pipeline/tests/golden/lens_*.png crates/pipeline/Cargo.toml
git commit -m "test(pipeline): goldens for lens correction"
```

---

### Task 7: Validate on a real photograph

- [ ] **Step 1: Wire the CLI**

In `crates/rustroom/src/main.rs`'s `render` path, after decoding, build a corrector from the decoded lens metadata and pass it to the pipeline:

```rust
    let lens = decoded.metadata.lens.as_ref().and_then(|l| {
        let db = lens_profile::Catalogue::bundled().ok()?;
        let p = db.find(&decoded.metadata.make, &decoded.metadata.model, &l.name)?;
        lens_profile::Corrector::new(
            &p,
            &lens_profile::Shot { focal: l.focal, aperture: l.aperture },
            image.width(),
            image.height(),
            lens_profile::Corrections {
                distortion: edits.lens_correction.distortion,
                tca: edits.lens_correction.tca,
                vignetting: edits.lens_correction.vignetting,
                scale: edits.lens_correction.scale,
            },
        )
    });
```

Add `lens-profile` to `crates/rustroom/Cargo.toml`.

⚠ `Catalogue::bundled()` decompresses 56 XML files, so build it **once per render**, never per photo in a loop — and note the cost in Step 3.

- [ ] **Step 2: Render with and without**

```bash
SP=/private/tmp/claude-501/-Users-me-Projects-rustroom/58ca40ba-db49-4da9-800f-b23dd64a2ed6/scratchpad
echo '{}' > "$SP/off.json"
echo '{"lens_correction":{"distortion":true,"tca":true,"vignetting":true,"scale":100.0}}' > "$SP/on.json"
RAW=$(find "$HOME/Desktop/fuji" -name '*.RAF' | sort | head -1)
cargo run --release -q -p rustroom -- render "$RAW" "$SP/lens_off.jpg" --edits "$SP/off.json"
cargo run --release -q -p rustroom -- render "$RAW" "$SP/lens_on.jpg" --edits "$SP/on.json"
```

- [ ] **Step 3: Measure, then look**

⚠ **The number to interrogate is the XF23's 4.79× corner lift** — 2.3 stops, plausible for a fast prime wide open but the single most likely thing to be wrong. Extend `crates/app/examples/grade_probe.rs`'s approach: compare corner brightness on and off, and check it against 4.79× rather than accepting whatever appears.

Then open both JPEGs and look at: straight lines near the frame edge (should straighten), the corners (should brighten, not blow out), and colour fringing on high-contrast edges at the periphery (should reduce).

- [ ] **Step 4: Measure the cost**

```bash
RAW=$(find "$HOME/Desktop/fuji" -name '*.RAF' | sort | head -1)
cargo run --release -q -p app --example bench_preview "$RAW" 2>&1 | tail -20
```

Add a lens-correction case to `bench_preview` following the existing pattern, and record the number. ⚠ It is a **prefix** stage, so the cost is paid on a geometry change and on the first render, not on every tail slider — say which in the record.

- [ ] **Step 5: Record the brush-mask interaction in the backlog**

The spec defers the principled fix; `docs/BACKLOG.md` is where deferred work with its reasoning
lives, and an entry that exists only in a spec is one nobody will find. Add under
*Blocked on a design question* → no, under **Editing tools**:

```markdown
- **Brush strokes shift when lens corrections are toggled.** Strokes are anchored in
  "pre-geometry" coordinates, which means *the input to `GeometryStage`* — and lens correction now
  runs ahead of it, so enabling or disabling it redefines that basis. The shift is up to ~3.5% at
  the corners and ~0 at the centre. ⚠ The principled fix is to include distortion in `FrameMap` so
  strokes anchor in raw sensor coordinates and travel *with* the correction — which is what
  "a stroke must stay on the cat" actually demands, since the correction moves the cat. It needs
  the **forward** distortion map and the `lensfun` crate exposes only the inverse, so it means an
  iterative solve per stroke point. Deferred on the grounds that lens corrections are enabled once,
  early, not toggled mid-edit.
```

- [ ] **Step 6: Full green check and commit**

```bash
cargo fmt --all --check; echo "fmt exit: $?"
cargo clippy --all-targets --all-features -- -D warnings > /tmp/c.log 2>&1; echo "clippy exit: $?"
cargo test --all 2>&1 | grep -cE "test result: FAILED"
git add -A
git commit -m "feat(rustroom): apply lens corrections from the CLI"
```

---

## Definition of done for plan 1

- `lens-profile` matches a camera+lens and answers per-pixel source and gain, with autoscale.
- `raw-decode` reports the lens; `Edits.lens_correction` lives in the **Detail** group and is neutral by default.
- `LensCorrectionStage` runs first in the prefix.
- 47 goldens; the 43 pre-existing ones **byte-identical**.
- `fmt`, `clippy -D warnings`, `test --all` green.
- Validated on a real `.RAF`, with the 4.79× corner lift specifically interrogated.
- The preview cost measured and recorded.
- No GUI code — that is plan 2.
