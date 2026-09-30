//! Geometry: crop, orientation, straighten, lens correction and perspective.

use serde::{Deserialize, Serialize};

/// Normalized crop rectangle in [0, 1]; default is the full frame.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CropRect {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl Default for CropRect {
    fn default() -> Self {
        Self {
            left: 0.0,
            top: 0.0,
            right: 1.0,
            bottom: 1.0,
        }
    }
}

impl CropRect {
    /// The rect covering the same content after the frame turns clockwise.
    ///
    /// ⚠ Without this, rotating a cropped photo leaves the crop on entirely different pixels.
    /// In the unit square a clockwise turn maps `(x, y) -> (1 - y, x)`.
    pub fn rotated_cw(self) -> Self {
        Self {
            left: 1.0 - self.bottom,
            top: self.left,
            right: 1.0 - self.top,
            bottom: self.right,
        }
    }

    /// The rect covering the same content after the frame mirrors horizontally.
    pub fn flipped_h(self) -> Self {
        Self {
            left: 1.0 - self.right,
            top: self.top,
            right: 1.0 - self.left,
            bottom: self.bottom,
        }
    }
}

/// A quarter turn, clockwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Quarter {
    #[default]
    Zero,
    Cw90,
    Cw180,
    Cw270,
}

impl Quarter {
    /// Quarter turns clockwise, 0..=3 — the arithmetic form, so composition is modular
    /// addition rather than a match table nobody can check.
    ///
    /// ⚠ Public since M9.13: the AI worker turns the photo upright by exactly this much before a
    /// network sees it.
    pub fn turns(self) -> u8 {
        match self {
            Quarter::Zero => 0,
            Quarter::Cw90 => 1,
            Quarter::Cw180 => 2,
            Quarter::Cw270 => 3,
        }
    }

    fn from_turns(t: u8) -> Self {
        match t % 4 {
            0 => Quarter::Zero,
            1 => Quarter::Cw90,
            2 => Quarter::Cw180,
            _ => Quarter::Cw270,
        }
    }

    /// Whether this turn swaps width and height.
    pub fn swaps_axes(self) -> bool {
        matches!(self, Quarter::Cw90 | Quarter::Cw270)
    }
}

/// One of the eight rigid transforms of a rectangle: mirror about the vertical axis if
/// `mirror`, **then** turn by `rotate`.
///
/// ⚠ `mirror` is one flag, not two. A vertical flip is a horizontal flip plus a half turn, so
/// storing `flip_h` and `flip_v` separately would make four of the eight states representable
/// two ways. Flip-vertical is a derived *action* — see [`Orientation::flip_v`] — not a field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Orientation {
    pub rotate: Quarter,
    pub mirror: bool,
}

impl Orientation {
    pub fn is_identity(self) -> bool {
        self == Orientation::default()
    }

    /// Whether this orientation swaps the frame's width and height.
    ///
    /// ⚠ The mirror does not transpose — it reflects — so only the odd quarter turns do. A
    /// predicate that answered "any non-identity orientation" would be right on a square fixture
    /// and wrong on every photograph.
    pub fn transposes(self) -> bool {
        matches!(self.rotate, Quarter::Cw90 | Quarter::Cw270)
    }

    pub fn rotate_cw(&mut self) {
        self.rotate = Quarter::from_turns(self.rotate.turns() + 1);
    }

    pub fn rotate_ccw(&mut self) {
        self.rotate = Quarter::from_turns(self.rotate.turns() + 3);
    }

    /// ⚠ Not just `mirror = !mirror`. The group is non-abelian: with `mirror` applied *first*,
    /// post-composing a horizontal flip gives `M ∘ rot_r = rot_{-r} ∘ M`, so the rotation
    /// negates too. Without this the buttons are each correct in isolation and wrong in
    /// sequence — guarded by `each_button_matches_its_geometric_action`, which compares against
    /// an independent corner model. ⚠ *Not* by the reachable-state count, which the wrong rule
    /// also passes: it still reaches all eight states, just as a direct product rather than a
    /// dihedral group.
    pub fn flip_h(&mut self) {
        self.rotate = Quarter::from_turns(4 - self.rotate.turns());
        self.mirror = !self.mirror;
    }

    /// A horizontal flip and a half turn. Derived rather than stored, per the type's note.
    pub fn flip_v(&mut self) {
        self.flip_h();
        self.rotate = Quarter::from_turns(self.rotate.turns() + 2);
    }
}

/// Geometry: orientation, then straighten angle (degrees, + = clockwise), then crop. Neutral
/// by default.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Geometry {
    /// Quarter turns and mirroring, applied *before* the straighten angle.
    #[serde(default)]
    pub orientation: Orientation,
    /// Straighten angle in degrees. 0 = none.
    pub angle: f32,
    pub crop: CropRect,
}

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
    /// How far to scale the corrected image, 0..100.
    ///
    /// ⚠ Only **pincushion** correction empties the frame — it reads from beyond the sensor at
    /// the corners. Barrel correction reads from *inside*, so it never leaves a gap and scaling
    /// it discards data for nothing. A single fixed value is therefore wrong at one end or the
    /// other of any zoom, which is why this is a slider rather than a constant.
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

/// Manual perspective correction — keystone about both axes, plus the two controls that make it
/// usable.
///
/// ⚠ Its own field on [`Edits`], **not** a member of [`Geometry`]. That is what buys the separate
/// cache checkpoint: sharing `PrefixStepKey::Geometry` would make every crop drag and every
/// Straighten frame re-run the perspective resample.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Perspective {
    /// Keystone about the horizontal axis (camera pitch), ~[-100, 100].
    pub vertical: f32,
    /// Keystone about the vertical axis (camera yaw), ~[-100, 100].
    pub horizontal: f32,
    /// Counteracts the stretch a keystone correction causes, ~[-100, 100].
    pub aspect: f32,
    /// How the corrected image is scaled into the frame, −100..100.
    ///
    /// ⚠⚠ **Bipolar, and both ends exist because of a report from using it.** −100 **fits**: the
    /// whole transformed image is shrunk to stay inside the frame, black around it, nothing lost.
    /// 0 leaves the transform **raw**: black where it pulled inward, content clipped where it
    /// pushed outward. +100 **fills**: no black, content lost off the edges.
    ///
    /// ⚠ Filling was there first and defaulted to on, which read as the slider *cropping* — the
    /// scale needed to fill is enormous, measured at 3:2 as **1.92× at Vertical 40, keeping 27% of
    /// the image, and 3.31× at 60, keeping 9%.** Fitting was then added because the raw view still
    /// **throws content off the edges**, reported as *"parts of the photo outside of the crop
    /// range, so it's simply gone."*
    ///
    /// ⚠ **Defaults to −100**, so nothing is ever silently discarded: you see the whole corrected
    /// frame and scale in when you have decided what to keep — which the crop rectangle, right
    /// above, can also do.
    pub scale: f32,
}

impl Default for Perspective {
    /// ⚠ Hand-written, **not** derived: `scale` is **−100** at neutral, and the container-level
    /// `#[serde(default)]` fills a missing field from this. A derived `Default` would give every
    /// partial sidecar `scale: 0` — the raw view, which clips content — silently. The trap
    /// `LensCorrection::default`, `RadialGradient::default` and `ColorGrading::default` each carry.
    fn default() -> Self {
        Self {
            vertical: 0.0,
            horizontal: 0.0,
            aspect: 0.0,
            scale: -100.0,
        }
    }
}

impl Perspective {
    /// Whether this warrants a pipeline stage.
    ///
    /// ⚠ The three *corrections* only — `scale` is a modifier, and dragging it alone must not
    /// insert a stage that would resample the whole image for nothing. The rule
    /// `NoiseReduction`, `Vignette` and `Sharpening` already follow.
    pub fn is_active(&self) -> bool {
        self.vertical != 0.0 || self.horizontal != 0.0 || self.aspect != 0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Edits;

    /// Every state reachable from the identity by the four buttons.
    fn reachable() -> std::collections::BTreeSet<(u8, bool)> {
        let key = |o: &Orientation| (o.rotate as u8, o.mirror);
        let mut seen = std::collections::BTreeSet::new();
        let mut frontier = vec![Orientation::default()];
        seen.insert(key(&Orientation::default()));
        // ⚠ The annotation is required: each `fn` item has its own distinct type, so an
        // unannotated array of them does not infer a common element type.
        let acts: [fn(&mut Orientation); 4] = [
            Orientation::rotate_cw,
            Orientation::rotate_ccw,
            Orientation::flip_h,
            Orientation::flip_v,
        ];
        while let Some(o) = frontier.pop() {
            for act in acts {
                let mut next = o;
                act(&mut next);
                if seen.insert(key(&next)) {
                    frontier.push(next);
                }
            }
        }
        seen
    }

    #[test]
    fn the_generators_produce_exactly_eight_states() {
        // A rectangle has exactly eight rigid transforms, and the buttons must reach all of
        // them and no more.
        //
        // ⚠ **This is necessary but nowhere near sufficient**, and I checked rather than
        // assumed: a `flip_h` that only flips `mirror` — the wrong, abelian rule — *also*
        // reaches all eight, as a direct product instead of a dihedral group. The count cannot
        // tell the two apart. `each_button_matches_its_geometric_action` is the test that can.
        assert_eq!(reachable().len(), 8);
    }

    /// The eight states, built directly rather than by exploration.
    fn all_states() -> Vec<Orientation> {
        let mut v = Vec::new();
        for rotate in [Quarter::Zero, Quarter::Cw90, Quarter::Cw180, Quarter::Cw270] {
            for mirror in [false, true] {
                v.push(Orientation { rotate, mirror });
            }
        }
        v
    }

    /// Where the unit square's corners land under `o`.
    ///
    /// An **independent model** of what an orientation means — "mirror about x, then turn
    /// clockwise" written out geometrically — so composition can be checked against geometry
    /// rather than against the very rules under test.
    fn corners(o: Orientation) -> Vec<(i32, i32)> {
        [(0, 0), (1, 0), (0, 1), (1, 1)]
            .into_iter()
            .map(|(x, y)| {
                let x = if o.mirror { 1 - x } else { x };
                match o.rotate {
                    Quarter::Zero => (x, y),
                    Quarter::Cw90 => (1 - y, x),
                    Quarter::Cw180 => (1 - x, 1 - y),
                    Quarter::Cw270 => (y, 1 - x),
                }
            })
            .collect()
    }

    #[test]
    fn each_button_matches_its_geometric_action() {
        // ⚠ The real guard on the non-abelian composition. Pressing a button must move the
        // stored state exactly as the corresponding *geometric* transform moves the frame —
        // from every one of the eight starting states, not just from the identity.
        //
        // This is what catches `flip_h` forgetting to negate the rotation, which the
        // reachable-state count does not.
        let cw = |(x, y): (i32, i32)| (1 - y, x);
        let ccw = |(x, y): (i32, i32)| (y, 1 - x);
        let fh = |(x, y): (i32, i32)| (1 - x, y);
        let fv = |(x, y): (i32, i32)| (x, 1 - y);

        /// A button, the geometric transform it should match, and its name.
        type Case = (
            fn(&mut Orientation),
            fn((i32, i32)) -> (i32, i32),
            &'static str,
        );
        let cases: [Case; 4] = [
            (Orientation::rotate_cw, cw, "rotate_cw"),
            (Orientation::rotate_ccw, ccw, "rotate_ccw"),
            (Orientation::flip_h, fh, "flip_h"),
            (Orientation::flip_v, fv, "flip_v"),
        ];

        for start in all_states() {
            for (act, geom, name) in cases {
                let mut moved = start;
                act(&mut moved);
                let want: Vec<_> = corners(start).into_iter().map(geom).collect();
                assert_eq!(corners(moved), want, "{name} from {start:?}");
            }
        }
    }

    #[test]
    fn each_generator_returns_to_identity() {
        let mut o = Orientation::default();
        for _ in 0..4 {
            o.rotate_cw();
        }
        assert_eq!(o, Orientation::default());

        let mut o = Orientation::default();
        for _ in 0..4 {
            o.rotate_ccw();
        }
        assert_eq!(o, Orientation::default());

        let flips: [fn(&mut Orientation); 2] = [Orientation::flip_h, Orientation::flip_v];
        for act in flips {
            let mut o = Orientation::default();
            act(&mut o);
            assert_ne!(o, Orientation::default(), "a flip must change something");
            act(&mut o);
            assert_eq!(o, Orientation::default());
        }
    }

    #[test]
    fn flipping_after_a_rotation_negates_it() {
        // ⚠ The non-abelian rule, stated directly so a failure names itself rather than only
        // showing up as a wrong state count. `M ∘ rot_r = rot_{-r} ∘ M`.
        let mut o = Orientation::default();
        o.rotate_cw(); // 90
        o.flip_h();
        assert_eq!(o.rotate, Quarter::Cw270, "90 must become 270 under a flip");
        assert!(o.mirror);
    }

    #[test]
    fn a_vertical_flip_is_a_horizontal_flip_and_a_half_turn() {
        // Keeps the UI's four buttons honest against the two stored fields.
        for start in [Quarter::Zero, Quarter::Cw90, Quarter::Cw180, Quarter::Cw270] {
            for mirror in [false, true] {
                let base = Orientation {
                    rotate: start,
                    mirror,
                };
                let mut via_v = base;
                via_v.flip_v();

                let mut via_h = base;
                via_h.flip_h();
                via_h.rotate_cw();
                via_h.rotate_cw();

                assert_eq!(via_v, via_h, "from {base:?}");
            }
        }
    }

    #[test]
    fn a_clockwise_turn_moves_the_left_half_to_the_top() {
        // ⚠ The crop must follow the rotation, or rotating a cropped photo lands the crop on
        // entirely different content. Checked by *content*, not by arithmetic.
        let left_half = CropRect {
            left: 0.0,
            top: 0.0,
            right: 0.5,
            bottom: 1.0,
        };
        let top_half = CropRect {
            left: 0.0,
            top: 0.0,
            right: 1.0,
            bottom: 0.5,
        };
        assert_eq!(left_half.rotated_cw(), top_half);
    }

    #[test]
    fn four_crop_rotations_restore_the_rect() {
        // ⚠ Round trips are exact only to `f32` rounding: `1.0 - (1.0 - 0.1)` is 0.100000024,
        // not 0.1. The tolerance is 1e-6 of a *normalised* rect — a millionth of the frame,
        // sub-pixel on any image ever shot — so this is a statement about floats, not a
        // weakened assertion.
        let close = |a: CropRect, b: CropRect| {
            let d = |x: f32, y: f32| (x - y).abs() < 1e-6;
            d(a.left, b.left) && d(a.top, b.top) && d(a.right, b.right) && d(a.bottom, b.bottom)
        };
        let r = CropRect {
            left: 0.1,
            top: 0.2,
            right: 0.7,
            bottom: 0.9,
        };
        let mut got = r;
        for _ in 0..4 {
            got = got.rotated_cw();
        }
        assert!(close(got, r), "four turns gave {got:?}, want {r:?}");

        // And a horizontal flip is its own inverse.
        assert!(close(r.flipped_h().flipped_h(), r));

        // A turn must actually move an asymmetric rect, or the test above passes trivially.
        assert!(!close(r.rotated_cw(), r));
    }

    #[test]
    fn geometry_default_is_neutral() {
        let g = Geometry::default();
        assert_eq!(g.angle, 0.0);
        assert_eq!(g.crop, CropRect::default());
        assert_eq!(
            g.crop,
            CropRect {
                left: 0.0,
                top: 0.0,
                right: 1.0,
                bottom: 1.0
            }
        );
    }

    #[test]
    fn partial_geometry_json_defaults_full_crop() {
        let e: Edits = serde_json::from_str(r#"{"geometry": {"angle": 2.5}}"#).unwrap();
        assert_eq!(e.geometry.angle, 2.5);
        assert_eq!(e.geometry.crop, CropRect::default());
    }

    #[test]
    fn a_sidecar_without_scale_still_gets_a_hundred() {
        // ⚠ Third instance of this trap, after `RadialGradient` and `ColorGrading`: `scale` is
        // 100 at neutral, so `Default` must be hand-written. A derived one would give every
        // partial sidecar zero scale — black edges — silently.
        let e: Edits = serde_json::from_str(r#"{"lens_correction":{"distortion":true}}"#).unwrap();
        assert_eq!(e.lens_correction.unwrap().scale, 100.0);
        assert!(e.lens_correction.unwrap().distortion);
    }

    #[test]
    fn the_lens_scale_alone_is_inert() {
        let l = LensCorrection {
            scale: 0.0,
            ..LensCorrection::default()
        };
        assert!(!l.is_active());
    }

    #[test]
    fn any_enabled_lens_correction_is_active() {
        for l in [
            LensCorrection {
                distortion: true,
                ..LensCorrection::default()
            },
            LensCorrection {
                tca: true,
                ..LensCorrection::default()
            },
            LensCorrection {
                vignetting: true,
                ..LensCorrection::default()
            },
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

    /// ⚠⚠ The value, not the mechanism — and it is **−100 (fit)**, deliberately. It went 100 →
    /// 0 → −100 over two rounds of use, and each move answered a specific report: 100 read as the
    /// slider *cropping* (the fill scale is 1.92× at Vertical 40, keeping 27% of the image), and 0
    /// still **threw content off the edges**. Only −100 discards nothing.
    ///
    /// ⚠ At neutral every value of `scale` is the identity anyway, since both fit and fill are
    /// 1.0 with no correction — so this is about what happens on the *first drag*, not about the
    /// neutral render.
    #[test]
    fn a_neutral_perspective_fits_rather_than_fills() {
        assert_eq!(Perspective::default().scale, -100.0);
        assert!(!Perspective::default().is_active());
    }

    /// ⚠ `scale` is a modifier, so dragging it alone must not insert a stage.
    #[test]
    fn scale_alone_does_not_activate_the_perspective_stage() {
        let p = Perspective {
            scale: 0.0,
            ..Default::default()
        };
        assert!(!p.is_active(), "scale is a modifier, not a correction");
        for probe in [
            Perspective {
                vertical: 1.0,
                ..Default::default()
            },
            Perspective {
                horizontal: 1.0,
                ..Default::default()
            },
            Perspective {
                aspect: 1.0,
                ..Default::default()
            },
        ] {
            assert!(probe.is_active(), "{probe:?} must activate");
        }
    }

    /// ⚠ A sidecar written before this field existed must load with the *neutral* perspective,
    /// not a zeroed one. `#[serde(default)]` at the container level is what does it.
    #[test]
    fn a_sidecar_from_before_perspective_still_loads() {
        let json = r#"{"version":1,"exposure":0.5}"#;
        let e: Edits = serde_json::from_str(json).unwrap();
        assert_eq!(e.perspective, Perspective::default());
        assert_eq!(e.exposure, 0.5);
    }
}
