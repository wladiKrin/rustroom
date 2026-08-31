//! The non-destructive edit description (the "op-list" / sidecar model).
//! Serialized to JSON alongside the original; the pipeline is built from it.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Creative white balance layered on the camera WB. `temp`/`tint` in ~[-100, 100], 0 = neutral.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct WhiteBalance {
    /// Positive = warmer (more red, less blue).
    pub temp: f32,
    /// Positive = magenta, negative = green.
    pub tint: f32,
}

/// A point tone curve. Empty (or absent) = identity. Points are (x, y) in [0, 1].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ToneCurve {
    pub points: Vec<[f32; 2]>,
}

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
    fn turns(self) -> u8 {
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

/// One hue band's adjustments. All ~[-100, 100]; 0 = neutral.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct HslBand {
    /// Hue rotation for this band; ±100 maps to ±30°.
    pub hue: f32,
    /// Chroma scale; ±100 maps to ×0 … ×2.
    pub saturation: f32,
    /// Lightness scale; ±100 maps to ×0.5 … ×1.5.
    pub luminance: f32,
}

/// Per-hue adjustments for the 8 bands, in [`HSL_BAND_NAMES`] order.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Hsl {
    pub bands: [HslBand; 8],
}

/// Band names in `Hsl::bands` order — the one source of truth shared by the pipeline
/// (band centers) and the GUI (slider labels). The sidecar stores bands positionally, so
/// this order is part of the file format.
pub const HSL_BAND_NAMES: [&str; 8] = [
    "Red", "Orange", "Yellow", "Green", "Aqua", "Blue", "Purple", "Magenta",
];

/// Noise reduction. The two strengths are 0..100 and default to 0 (off); `detail` and
/// `smoothness` are quality modifiers that default to 50 and do nothing on their own.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NoiseReduction {
    /// Luminance NR strength, 0..100. 0 = off.
    pub luminance: f32,
    /// How hard luminance detail is protected, 0..100. Higher keeps more texture — and
    /// more noise with it. Inert while `luminance` is 0.
    pub detail: f32,
    /// Color (chroma) NR strength, 0..100. 0 = off.
    pub color: f32,
    /// Chroma blur radius, 0..100. Higher removes larger color blotches. Inert while
    /// `color` is 0.
    pub smoothness: f32,
}

impl Default for NoiseReduction {
    fn default() -> Self {
        Self {
            luminance: 0.0,
            detail: 50.0,
            color: 0.0,
            smoothness: 50.0,
        }
    }
}

impl NoiseReduction {
    /// True when these settings actually change pixels.
    ///
    /// Deliberately *not* `self != &Self::default()`: `detail` and `smoothness` only
    /// modify how the two strengths behave, so moving one on its own must not insert a
    /// stage into the pipeline.
    pub fn is_active(&self) -> bool {
        self.luminance > 0.0 || self.color > 0.0
    }
}

/// Post-crop vignette: darken or brighten the frame's edges. `amount` 0 = off; the other
/// three only shape the falloff.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Vignette {
    /// -100 darkens the edges by 2 EV, +100 brightens by 2 EV. 0 = off.
    pub amount: f32,
    /// Where the falloff sits, 0..100. Higher pushes it outward.
    pub midpoint: f32,
    /// Shape: -100 rounded rectangle … 0 ellipse following the frame … +100 true circle.
    pub roundness: f32,
    /// Transition width, 0..100. 0 is a hard edge.
    pub feather: f32,
}

impl Default for Vignette {
    fn default() -> Self {
        Self {
            amount: 0.0,
            midpoint: 50.0,
            roundness: 0.0,
            feather: 50.0,
        }
    }
}

impl Vignette {
    /// True when this actually changes pixels.
    ///
    /// Amount-only on purpose, the same rule [`NoiseReduction::is_active`] follows: midpoint,
    /// roundness and feather shape an effect that is off until `amount` is non-zero, so
    /// dragging one alone must not insert a stage.
    pub fn is_active(&self) -> bool {
        self.amount != 0.0
    }
}

/// One colour-grading wheel: a hue/saturation tint direction plus a lightness offset,
/// applied to one tonal range.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct GradeWheel {
    /// Oklab hue angle in degrees, 0..360, measured from the +a axis counterclockwise —
    /// i.e. exactly `atan2(b, a)`, with no offset to get wrong.
    ///
    /// ⚠ This is the **same angle** `BAND_HUES` uses in the HSL stage, so "red" means the
    /// same thing in both halves of the Color section. It would not if one were an sRGB hue.
    pub hue: f32,
    /// Tint strength, 0..100. 0 = no tint, whatever the hue.
    pub saturation: f32,
    /// Lightness offset for this range, -100..100.
    pub luminance: f32,
}

/// Three-way colour grading: a tint per tonal range, plus one applied everywhere.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ColorGrading {
    pub shadows: GradeWheel,
    pub midtones: GradeWheel,
    pub highlights: GradeWheel,
    /// Applied uniformly at every lightness, on top of the weighted three.
    pub global: GradeWheel,
    /// How far the three ranges overlap, 0..100. At 0 the nearest range wins outright; at
    /// 100 all three weigh 1/3 everywhere and the grade becomes a uniform tint.
    pub blending: f32,
    /// Slides all three range centres, -100..100. Positive lets the highlight tint reach
    /// further down.
    pub balance: f32,
}

impl Default for ColorGrading {
    /// ⚠ Hand-written, **not** derived: `blending` is 50 at neutral. The container-level
    /// `#[serde(default)]` fills a missing field from this, so a derived `Default` would give
    /// every partial sidecar `blending: 0` — hard-edged ranges — with nothing to notice.
    fn default() -> Self {
        Self {
            shadows: GradeWheel::default(),
            midtones: GradeWheel::default(),
            highlights: GradeWheel::default(),
            global: GradeWheel::default(),
            blending: 50.0,
            balance: 0.0,
        }
    }
}

impl ColorGrading {
    /// True when this actually changes pixels.
    ///
    /// Wheels only, the rule [`NoiseReduction::is_active`] and [`Vignette::is_active`] follow:
    /// `blending` and `balance` shape ranges that tint nothing until a wheel is turned up, and
    /// a `hue` with zero `saturation` is a direction with no magnitude.
    pub fn is_active(&self) -> bool {
        [self.shadows, self.midtones, self.highlights, self.global]
            .iter()
            .any(|w| w.saturation != 0.0 || w.luminance != 0.0)
    }
}

/// Unsharp-mask sharpening. `amount` 0 = off; the other three only shape the effect.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sharpening {
    /// Strength, 0..100. 0 = off.
    pub amount: f32,
    /// Blur radius (sigma) in **full-resolution pixels**, ~0.5..3.0. Scaled to whatever
    /// resolution is actually being rendered, so a preview approximates the export.
    pub radius: f32,
    /// Halo suppression, 0..100. 100 is a pure unsharp mask; lower values hold back
    /// sharpening where the local contrast is already high. Inert while `amount` is 0.
    pub detail: f32,
    /// Edge gate, 0..100. Higher confines sharpening to edges, sparing flat noisy areas.
    /// 0 = no gate. Inert while `amount` is 0.
    pub masking: f32,
}

impl Default for Sharpening {
    fn default() -> Self {
        Self {
            amount: 0.0,
            radius: 1.0,
            detail: 50.0,
            masking: 0.0,
        }
    }
}

impl Sharpening {
    /// True when these settings actually change pixels. Amount-only, the same rule
    /// [`NoiseReduction::is_active`] and [`Vignette::is_active`] follow.
    pub fn is_active(&self) -> bool {
        self.amount > 0.0
    }
}

/// Texture and Clarity: local contrast in two frequency bands. Both 0 = off.
///
/// Same operation as [`Sharpening`] at a different radius, which is why they share a stage
/// shape but not a struct: these are *look* controls that belong with tone, while sharpening
/// is capture- and output-specific.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct LocalContrast {
    /// Fine-band local contrast, ~[-100, 100]. Positive brings out surface detail, negative
    /// smooths it. 0 = off.
    pub texture: f32,
    /// Coarse-band local contrast, ~[-100, 100]. Positive is punchy midtone contrast,
    /// negative is a glow. 0 = off.
    pub clarity: f32,
}

impl LocalContrast {
    /// True when these settings actually change pixels.
    ///
    /// There are no modifier fields here, so this could have been `!= default()`. Written
    /// the same way [`NoiseReduction::is_active`], [`Vignette::is_active`] and
    /// [`Sharpening::is_active`] are, so the four read alike and so adding a modifier later
    /// does not change the rule.
    pub fn is_active(&self) -> bool {
        self.texture != 0.0 || self.clarity != 0.0
    }
}

/// Haze removal, via the dark-channel prior. 0 = off.
///
/// Runs in the pipeline **prefix**, beside noise reduction, because it is a correction of the
/// capture rather than a grade — and because a frame-global airlight can only be estimated
/// where the whole frame is present.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Dehaze {
    /// ~[-100, 100]. Positive removes the estimated haze, negative lays it back on —
    /// depth-aware in both directions, since both use the same transmission map. 0 = off.
    pub amount: f32,
}

impl Dehaze {
    /// True when these settings actually change pixels. Same rule as every other stage.
    pub fn is_active(&self) -> bool {
        self.amount != 0.0
    }
}

/// Group names in [`EditGroups`] field order — the one source of truth shared by the merge
/// logic, the checkbox UI and the tests. Mirrors [`HSL_BAND_NAMES`].
pub const EDIT_GROUP_NAMES: [&str; 8] = [
    "White Balance",
    "Tone",
    "Color",
    "Detail",
    "Lens",
    "Vignette",
    "Geometry",
    "Masks",
];

/// The `Geometry` group's index into [`EDIT_GROUP_NAMES`].
///
/// ⚠ Named rather than written as `5` at call sites: the Crop tool's own reset targets exactly
/// this group, and a magic index would silently reset the wrong one if the table ever changed
/// order. `the_named_group_indices_match_the_table` is the guard.
pub const GEOMETRY_GROUP: usize = 6;

/// The `Masks` group's index into [`EDIT_GROUP_NAMES`]. See [`GEOMETRY_GROUP`].
pub const MASKS_GROUP: usize = 7;

/// Which groups of settings a paste or a preset carries.
///
/// [`Default`] is everything **except** geometry: a crop framed for one composition reframes
/// another, so carrying it across photos is usually wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct EditGroups {
    pub white_balance: bool,
    /// Exposure, contrast, highlights, shadows, whites, blacks, tone curve, texture,
    /// clarity, dehaze.
    pub tone: bool,
    /// Saturation, vibrance, HSL.
    pub color: bool,
    /// Noise reduction and sharpening.
    pub detail: bool,
    /// Optical corrections read from a lens profile.
    ///
    /// ⚠ Its own group rather than part of `detail`. They were bundled at first, which meant
    /// syncing noise reduction also synced lens corrections — two unrelated things travelling
    /// together for no reason. Separating them is what lets "copy this photo's lens
    /// corrections" be its own choice in Paste, Sync and Save-preset.
    pub lens: bool,
    /// The creative, post-crop vignette.
    ///
    /// ⚠ `#[serde(alias = "effects")]` because this field was called `effects` and the group
    /// was displayed as "Effects". `EditGroups` carries a container-level `#[serde(default)]`,
    /// so without the alias an existing preset storing `"effects": false` would fall back to
    /// this field's default of **true** — silently starting to carry a vignette the preset was
    /// deliberately built to exclude. Renaming a serialized field is never just a rename.
    #[serde(alias = "effects")]
    pub vignette: bool,
    /// Crop and straighten.
    pub geometry: bool,
    /// Masked local adjustments.
    ///
    /// ⚠ Off by default, like `geometry` and for the same reason: a gradient positioned for
    /// one composition is meaningless on another.
    pub masks: bool,
}

impl Default for EditGroups {
    fn default() -> Self {
        Self {
            white_balance: true,
            tone: true,
            color: true,
            detail: true,
            lens: true,
            vignette: true,
            geometry: false,
            masks: false,
        }
    }
}

impl EditGroups {
    pub const ALL: Self = Self {
        white_balance: true,
        tone: true,
        color: true,
        detail: true,
        lens: true,
        vignette: true,
        geometry: true,
        masks: true,
    };

    pub const NONE: Self = Self {
        white_balance: false,
        tone: false,
        color: false,
        detail: false,
        lens: false,
        vignette: false,
        geometry: false,
        masks: false,
    };

    /// Only group `i`, indexed as in [`EDIT_GROUP_NAMES`].
    ///
    /// # Panics
    /// Panics if `i >= EDIT_GROUP_NAMES.len()`.
    pub fn only(i: usize) -> Self {
        let mut g = Self::NONE;
        *g.get_mut(i) = true;
        g
    }

    /// Whether group `i` is selected, indexed as in [`EDIT_GROUP_NAMES`].
    ///
    /// # Panics
    /// Panics if `i >= EDIT_GROUP_NAMES.len()`.
    pub fn get(&self, i: usize) -> bool {
        match i {
            0 => self.white_balance,
            1 => self.tone,
            2 => self.color,
            3 => self.detail,
            4 => self.lens,
            5 => self.vignette,
            6 => self.geometry,
            7 => self.masks,
            _ => panic!("edit group index {i} out of range"),
        }
    }

    /// Mutable access to group `i`, for the checkbox UI. Indexed as in [`EDIT_GROUP_NAMES`].
    ///
    /// # Panics
    /// Panics if `i >= EDIT_GROUP_NAMES.len()`.
    pub fn get_mut(&mut self, i: usize) -> &mut bool {
        match i {
            0 => &mut self.white_balance,
            1 => &mut self.tone,
            2 => &mut self.color,
            3 => &mut self.detail,
            4 => &mut self.lens,
            5 => &mut self.vignette,
            6 => &mut self.geometry,
            7 => &mut self.masks,
            _ => panic!("edit group index {i} out of range"),
        }
    }
}

/// A graduated filter. Coverage ramps from 0 at `from` to 1 at `to`, measured along the axis
/// between them and constant on lines perpendicular to it.
///
/// Endpoints are in **normalised frame coordinates**, as [`CropRect`] is — so a mask means the
/// same thing on the 1600 px preview and on the full-resolution export.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct LinearGradient {
    pub from: [f32; 2],
    pub to: [f32; 2],
    pub invert: bool,
}

impl LinearGradient {
    /// Coverage at `(u, v)` in normalised frame coordinates, in `0.0..=1.0`.
    ///
    /// `aspect` is the frame's width divided by its height.
    ///
    /// ⚠ **The aspect is not decoration.** Coverage is a projection onto the axis, and a
    /// projection needs a metric. In raw normalised space the metric is anisotropic, so lines
    /// perpendicular to the axis *in that space* are not perpendicular on screen — a diagonal
    /// gradient's iso-lines would visibly skew. Scaling `u` by `aspect` restores an isotropic
    /// metric before projecting. Same species of problem as the vignette's `roundness`.
    pub fn coverage(&self, u: f32, v: f32, aspect: f32) -> f32 {
        // Work in units of frame height, where both axes have the same pixel scale.
        let (ax, ay) = (self.from[0] * aspect, self.from[1]);
        let (bx, by) = (self.to[0] * aspect, self.to[1]);
        let (dx, dy) = (bx - ax, by - ay);
        let len2 = dx * dx + dy * dy;
        if len2 <= f32::EPSILON {
            // No axis to project onto. Full coverage rather than NaN: the alternative reaches
            // the blend and paints the frame with garbage.
            return 1.0;
        }
        let t = (((u * aspect) - ax) * dx + (v - ay) * dy) / len2;
        let t = t.clamp(0.0, 1.0);
        if self.invert {
            1.0 - t
        } else {
            t
        }
    }
}

/// A radial filter. Coverage is 1 inside the ellipse and ramps to 0 at its boundary.
///
/// `center` is in **normalised frame coordinates**, as [`CropRect`] is. `radius[0]` is a
/// fraction of the frame's *width* and `radius[1]` a fraction of its *height* — so a mask means
/// the same thing on the 1600 px preview and on the full-resolution export, and the default
/// radius renders as an ellipse *similar to the frame*.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct RadialGradient {
    pub center: [f32; 2],
    pub radius: [f32; 2],
    /// 0 = a hard edge at the ellipse, 100 = a ramp all the way from the centre.
    pub feather: f32,
    pub invert: bool,
}

/// ⚠ **Hand-written, not derived** — unlike [`LinearGradient`]. Container-level
/// `#[serde(default)]` fills a missing field from the *struct's* `Default`, so a derived one
/// would give every partial sidecar a zero-radius point-ellipse. `feather: 50` cannot come
/// from `f32::default()` in any case.
impl Default for RadialGradient {
    fn default() -> Self {
        Self {
            center: [0.5, 0.5],
            radius: [0.25, 0.25],
            feather: 50.0,
            invert: false,
        }
    }
}

impl RadialGradient {
    /// Below this the ramp is treated as a hard edge. The only thing between feather 0 and a
    /// division by zero — the same role `WIDTH_EPS` plays in the vignette.
    const FEATHER_EPS: f32 = 1e-4;

    /// Coverage at `(u, v)` in normalised frame coordinates, in `0.0..=1.0`.
    ///
    /// ⚠⚠ **`aspect` is deliberately unused, and this is not an oversight.** Every sibling
    /// mask function needs it: [`LinearGradient::coverage`] projects onto an axis and
    /// `mask_tool::hit_test` measures a screen-space distance, and the anisotropic scaling
    /// between normalised and screen space destroys *perpendicularity* and *distance*
    /// respectively. It does **not** destroy axis-alignment: an axis-aligned ellipse maps to an
    /// axis-aligned ellipse, so the expression below is already a correct on-screen ellipse.
    ///
    /// Rotation is precisely the feature that would bring the correction back, which is the
    /// main reason this shape is axis-aligned. `radial_coverage_is_independent_of_aspect` is
    /// the guard against someone "fixing" the omission.
    ///
    /// The parameter stays in the signature so [`Mask::coverage`] has one shape across
    /// variants.
    pub fn coverage(&self, u: f32, v: f32, _aspect: f32) -> f32 {
        let (rx, ry) = (self.radius[0], self.radius[1]);
        let c = if rx <= f32::EPSILON || ry <= f32::EPSILON {
            // A degenerate ellipse honestly covers nothing. Unlike `LinearGradient`'s guard,
            // which chooses 1.0 only because its alternative was NaN, 0.0 is the true answer
            // here and cannot paint garbage either.
            0.0
        } else {
            // Normalised elliptical distance: exactly 1.0 on the boundary.
            let dx = (u - self.center[0]) / rx;
            let dy = (v - self.center[1]) / ry;
            let d = (dx * dx + dy * dy).sqrt();
            let f = (self.feather / 100.0).clamp(0.0, 1.0);
            if f <= Self::FEATHER_EPS {
                if d <= 1.0 {
                    1.0
                } else {
                    0.0
                }
            } else {
                // Smoothstep rather than a linear ramp: a linear ramp in radial distance
                // leaves a visible crease ring where the derivative jumps. The vignette's
                // feather made the same choice for the same reason.
                let t = ((1.0 - d) / f).clamp(0.0, 1.0);
                t * t * (3.0 - 2.0 * t)
            }
        };
        if self.invert {
            1.0 - c
        } else {
            c
        }
    }
}

/// One continuous drag of the brush.
///
/// ⚠ `Default` is hand-written, not derived: `flow` must default to **100**, not to
/// `f32::default()`. Container-level `#[serde(default)]` fills a missing field from the struct's
/// `Default`, so a derived one would make every stroke in every pre-flow sidecar invisible. The
/// same trap `RadialGradient`'s feather hit.
///
/// ⚠ **Points are in pre-geometry SOURCE normalised coordinates**, unlike [`LinearGradient`] and
/// [`RadialGradient`], which are in *frame* coordinates. `MaskStage` runs after `GeometryStage`,
/// so a frame-anchored stroke would slide across the image content whenever the crop changed —
/// invisible for a gradient, ruinous for a stroke painted onto a subject. The asymmetry is
/// principled: a gradient is compositional, a stroke is content-referred.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BrushStroke {
    pub points: Vec<[f32; 2]>,
    /// Radius as a fraction of the source image's **long edge**.
    ///
    /// ⚠ The long edge, not the width — `points` are normalised per axis and so live in an
    /// anisotropic space. A radius measured against width but compared against height would make
    /// every dab an ellipse. Choosing an isotropic unit avoids the correction that
    /// [`LinearGradient::coverage`] and `mask_tool::hit_test` both need.
    pub radius: f32,
    /// 0 = maximally soft, 100 = a hard edge.
    ///
    /// ⚠ Arithmetically this is `100 − feather` in [`RadialGradient`]'s maths. It keeps the brush
    /// word because that is what every brush calls it. **Do not "unify" the two** — the inversion
    /// would be silent, since both spellings look plausible.
    pub hardness: f32,
    /// How much of full coverage one pass of this stroke lays down, 0..=100.
    ///
    /// ⚠ **This is the strength control, and `hardness` is not.** Hardness shapes a dab's *edge*;
    /// even at hardness 0 a dab's centre reaches full coverage. Without flow the softest possible
    /// brush still painted at full strength and repeated passes did nothing at all — which is
    /// exactly what it felt like in use, and why this was pulled forward from a later milestone
    /// rather than left deferred.
    ///
    /// ⚠ **Defaults to 100**, so a stroke written before this field existed keeps its meaning.
    pub flow: f32,
    /// Subtract from coverage rather than adding to it.
    pub erase: bool,
}

impl Default for BrushStroke {
    fn default() -> Self {
        Self {
            points: Vec::new(),
            radius: 0.0,
            hardness: 0.0,
            flow: 100.0,
            erase: false,
        }
    }
}

/// A painted mask: an ordered list of strokes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Brush {
    pub strokes: Vec<BrushStroke>,
    /// Flip coverage, as both analytic shapes do.
    pub invert: bool,
}

/// The mask primitives.
///
/// ⚠ **Not `Copy`** — `Mask::Brush` carries a `Vec<BrushStroke>`. [`MaskedAdjustment`] is
/// therefore not `Copy` either. Both are still `Clone`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Mask {
    Linear(LinearGradient),
    Radial(RadialGradient),
    Brush(Brush),
}

impl Default for Mask {
    fn default() -> Self {
        Mask::Linear(LinearGradient::default())
    }
}

impl Mask {
    /// Coverage at `(u, v)` in normalised frame coordinates, for the **analytic** shapes.
    ///
    /// `None` means the shape is sampled data — a brush — and must be rasterized instead. It is
    /// an `Option` rather than a `0.0` fallback because a fallback compiles, runs, and silently
    /// produces a mask that covers nothing; the `Option` forces every caller to decide.
    ///
    /// `aspect` is the frame's width divided by its height. ⚠ Whether a variant *needs* it
    /// differs: see [`LinearGradient::coverage`] for why a projection does, and
    /// [`RadialGradient::coverage`] for why an axis-aligned ellipse does not.
    pub fn analytic_coverage(&self, u: f32, v: f32, aspect: f32) -> Option<f32> {
        match self {
            Mask::Linear(g) => Some(g.coverage(u, v, aspect)),
            Mask::Radial(r) => Some(r.coverage(u, v, aspect)),
            // Sampled data — `pipeline::brush::rasterize` answers for this one.
            Mask::Brush(_) => None,
        }
    }

    /// The shape's name, for the mask list in the UI.
    pub fn label(&self) -> &'static str {
        match self {
            Mask::Linear(_) => "Gradient",
            Mask::Radial(_) => "Radial",
            Mask::Brush(_) => "Brush",
        }
    }

    /// The `invert` flag, wherever it lives on this shape.
    ///
    /// Every shape carries one and none of them means anything shape-specific by it, so the
    /// panel offers a single checkbox rather than a match. Returning `&mut bool` rather than a
    /// getter/setter pair is what lets `ui.checkbox` drive it directly.
    pub fn invert_mut(&mut self) -> &mut bool {
        match self {
            Mask::Linear(g) => &mut g.invert,
            Mask::Radial(r) => &mut r.invert,
            Mask::Brush(b) => &mut b.invert,
        }
    }
}

/// What a mask can carry: the **per-pixel** adjustments, and only those.
///
/// ⚠ Not an arbitrary subset. It is exactly the set where "apply the stage, then blend the
/// result by coverage" is both correct and cheap, because each output pixel depends only on
/// its own input pixel. A spatial stage — noise reduction, sharpening, clarity, dehaze, HSL —
/// reads neighbours *outside* the mask, so "masked" needs a separate decision for each and is
/// out of scope here.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct LocalAdjustments {
    pub white_balance: WhiteBalance,
    pub exposure: f32,
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub whites: f32,
    pub blacks: f32,
    pub saturation: f32,
    pub vibrance: f32,
}

impl LocalAdjustments {
    /// Whether this would change any pixel. An inactive set inserts no stages at all.
    pub fn is_active(&self) -> bool {
        *self != Self::default()
    }
}

/// One mask and the adjustments it carries.
/// ⚠ Not `Copy`: [`Mask::Brush`] carries a stroke list. Still `Clone`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct MaskedAdjustment {
    pub mask: Mask,
    pub adjustments: LocalAdjustments,
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
    /// How far to scale the corrected image, 0..100.
    ///
    /// ⚠ 0 leaves the empty corners the correction opens; 100 scales until they are gone. A
    /// keystone empties far more of the frame than pincushion distortion does, which is why this
    /// defaults to 100 where [`LensCorrection::scale`] could reasonably have defaulted to 0.
    pub scale: f32,
}

impl Default for Perspective {
    /// ⚠ Hand-written, **not** derived: `scale` is 100 at neutral, and the container-level
    /// `#[serde(default)]` fills a missing field from this. A derived `Default` would give every
    /// partial sidecar `scale: 0`, silently — the trap `LensCorrection::default`,
    /// `RadialGradient::default` and `ColorGrading::default` each carry.
    fn default() -> Self {
        Self {
            vertical: 0.0,
            horizontal: 0.0,
            aspect: 0.0,
            scale: 100.0,
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

/// All edits applied to one image. Every field is neutral by default.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Edits {
    /// Schema version for future migrations.
    pub version: u32,
    pub white_balance: WhiteBalance,
    /// Exposure in stops (EV). 0 = no change.
    pub exposure: f32,
    /// Contrast, ~[-100, 100]. 0 = no change.
    pub contrast: f32,
    /// Highlights, ~[-100, 100]. Positive brightens highlights.
    pub highlights: f32,
    /// Shadows, ~[-100, 100]. Positive brightens shadows.
    pub shadows: f32,
    /// Whites endpoint, ~[-100, 100]. Positive lifts the top end.
    pub whites: f32,
    /// Blacks endpoint, ~[-100, 100]. Positive lifts the bottom end.
    pub blacks: f32,
    pub tone_curve: ToneCurve,
    /// Saturation, ~[-100, 100]. -100 = monochrome, 0 = no change, +100 = 2x chroma.
    pub saturation: f32,
    /// Vibrance, ~[-100, 100]. Like saturation, but weighted toward already-muted colors.
    pub vibrance: f32,
    pub hsl: Hsl,
    pub color_grading: ColorGrading,
    pub local_contrast: LocalContrast,
    pub dehaze: Dehaze,
    pub noise_reduction: NoiseReduction,
    pub vignette: Vignette,
    pub sharpening: Sharpening,
    pub lens_correction: LensCorrection,
    pub perspective: Perspective,
    pub geometry: Geometry,
    /// Local adjustments, each confined to a mask. Applied in order, after the global
    /// per-pixel adjustments and before the vignette.
    pub masks: Vec<MaskedAdjustment>,
}

impl Edits {
    /// Load edits from a JSON sidecar file.
    pub fn from_json_file(path: &Path) -> Result<Self, EditLoadError> {
        let text = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&text)?)
    }

    /// Write edits to a JSON sidecar file (pretty-printed).
    pub fn to_json_file(&self, path: &Path) -> Result<(), EditLoadError> {
        let text = serde_json::to_string_pretty(self)?;
        std::fs::write(path, text)?;
        Ok(())
    }

    /// Overwrite this photo's settings with `src`'s, for the selected groups only.
    ///
    /// `version` is a schema marker rather than an edit, so it never transfers — the target
    /// keeps its own.
    ///
    /// ⚠ The destructuring below is **exhaustive on purpose**. Adding a field to `Edits`
    /// breaks this function until whoever adds it decides which group owns it. Without that,
    /// a new field would silently never transfer through a paste or a preset, and nothing
    /// would fail — the same class of bug `PrefixKey` guards against in the pipeline.
    pub fn merge_from(&mut self, src: &Edits, groups: EditGroups) {
        let Edits {
            version: _,
            white_balance,
            exposure,
            contrast,
            highlights,
            shadows,
            whites,
            blacks,
            tone_curve,
            saturation,
            vibrance,
            hsl,
            color_grading,
            local_contrast,
            dehaze,
            noise_reduction,
            sharpening,
            lens_correction,
            perspective,
            vignette,
            geometry,
            masks,
        } = src;

        if groups.white_balance {
            self.white_balance = *white_balance;
        }
        if groups.tone {
            self.exposure = *exposure;
            self.contrast = *contrast;
            self.highlights = *highlights;
            self.shadows = *shadows;
            self.whites = *whites;
            self.blacks = *blacks;
            self.tone_curve = tone_curve.clone();
            self.local_contrast = *local_contrast;
            self.dehaze = *dehaze;
        }
        if groups.color {
            self.saturation = *saturation;
            self.vibrance = *vibrance;
            self.hsl = *hsl;
            self.color_grading = *color_grading;
        }
        if groups.detail {
            self.noise_reduction = *noise_reduction;
            self.sharpening = *sharpening;
        }
        if groups.lens {
            self.lens_correction = *lens_correction;
        }
        if groups.vignette {
            self.vignette = *vignette;
        }
        if groups.geometry {
            self.geometry = *geometry;
            // ⚠ Perspective is compositional, exactly as the crop is: a keystone framed for one
            // photograph is meaningless on another. So it rides the Geometry group, which
            // defaults **off** for presets, paste and sync.
            self.perspective = *perspective;
        }
        if groups.masks {
            self.masks = masks.clone();
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum EditLoadError {
    #[error("reading edits file: {0}")]
    Io(#[from] std::io::Error),
    #[error("parsing edits JSON: {0}")]
    Json(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn edits_default_roundtrips_json() {
        let edits = Edits::default();
        let json = serde_json::to_string(&edits).unwrap();
        let back: Edits = serde_json::from_str(&json).unwrap();
        assert_eq!(edits, back);
    }

    #[test]
    fn empty_object_deserializes_to_default() {
        let back: Edits = serde_json::from_str("{}").unwrap();
        assert_eq!(back, Edits::default());
    }

    #[test]
    fn neutral_is_default_and_all_zero() {
        let e = Edits::default();
        assert_eq!(e.exposure, 0.0);
        assert_eq!(e.white_balance, WhiteBalance::default());
        assert!(e.tone_curve.points.is_empty());
    }

    #[test]
    fn partial_json_fills_neutral_defaults() {
        let e: Edits = serde_json::from_str(r#"{"exposure": 1.5}"#).unwrap();
        assert_eq!(e.exposure, 1.5);
        assert_eq!(e.contrast, 0.0);
    }

    #[test]
    fn the_named_group_indices_match_the_table() {
        // ⚠ A tripwire, not a discovery. The two constants exist so a per-tool reset does not
        // write a magic `5` or `6`; this is what stops them going stale if the table is
        // reordered, which nothing else would catch — a wrong index still compiles and still
        // resets *something*.
        assert_eq!(EDIT_GROUP_NAMES[GEOMETRY_GROUP], "Geometry");
        assert_eq!(EDIT_GROUP_NAMES[MASKS_GROUP], "Masks");
    }

    #[test]
    fn hsl_defaults_to_eight_neutral_bands() {
        let e = Edits::default();
        assert_eq!(e.hsl.bands.len(), 8);
        assert!(e.hsl.bands.iter().all(|b| *b == HslBand::default()));
        assert_eq!(HSL_BAND_NAMES.len(), 8);
    }

    #[test]
    fn old_sidecar_without_hsl_still_loads() {
        let e: Edits = serde_json::from_str(r#"{"exposure": 0.5}"#).unwrap();
        assert_eq!(e.hsl, Hsl::default());
    }

    #[test]
    fn hsl_roundtrips_json() {
        let mut hsl = Hsl::default();
        hsl.bands[0].saturation = -40.0;
        hsl.bands[5].hue = 25.0;
        hsl.bands[3].luminance = 60.0;
        let e = Edits {
            hsl,
            ..Default::default()
        };
        let json = serde_json::to_string(&e).unwrap();
        let back: Edits = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
    }

    #[test]
    fn color_fields_default_to_neutral() {
        let e = Edits::default();
        assert_eq!(e.saturation, 0.0);
        assert_eq!(e.vibrance, 0.0);
    }

    #[test]
    fn old_sidecar_without_color_fields_still_loads() {
        let e: Edits = serde_json::from_str(r#"{"exposure": 0.5, "contrast": 10.0}"#).unwrap();
        assert_eq!(e.exposure, 0.5);
        assert_eq!(e.saturation, 0.0);
        assert_eq!(e.vibrance, 0.0);
    }

    #[test]
    fn color_fields_roundtrip_json() {
        let e = Edits {
            saturation: 25.0,
            vibrance: -40.0,
            ..Default::default()
        };
        let json = serde_json::to_string(&e).unwrap();
        let back: Edits = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
    }

    #[test]
    fn noise_reduction_defaults_to_off_with_midpoint_modifiers() {
        let nr = NoiseReduction::default();
        assert_eq!(nr.luminance, 0.0);
        assert_eq!(nr.color, 0.0);
        // The two modifiers sit at their midpoint, not zero: they describe *how* to
        // denoise, and zero would mean "smooth everything" / "smallest radius".
        assert_eq!(nr.detail, 50.0);
        assert_eq!(nr.smoothness, 50.0);
        assert_eq!(Edits::default().noise_reduction, nr);
    }

    #[test]
    fn noise_reduction_is_active_only_when_a_strength_is_up() {
        assert!(!NoiseReduction::default().is_active());
        // Modifiers alone must NOT activate the stage — otherwise dragging Detail would
        // insert a stage that costs an Oklab round trip and changes nothing.
        assert!(!NoiseReduction {
            detail: 100.0,
            smoothness: 0.0,
            ..Default::default()
        }
        .is_active());
        assert!(NoiseReduction {
            luminance: 1.0,
            ..Default::default()
        }
        .is_active());
        assert!(NoiseReduction {
            color: 1.0,
            ..Default::default()
        }
        .is_active());
    }

    #[test]
    fn old_sidecar_without_noise_reduction_still_loads() {
        let e: Edits = serde_json::from_str(r#"{"exposure": 0.5}"#).unwrap();
        assert_eq!(e.noise_reduction, NoiseReduction::default());
    }

    #[test]
    fn partial_noise_reduction_json_keeps_modifier_defaults() {
        let e: Edits = serde_json::from_str(r#"{"noise_reduction": {"color": 40.0}}"#).unwrap();
        assert_eq!(e.noise_reduction.color, 40.0);
        assert_eq!(e.noise_reduction.luminance, 0.0);
        assert_eq!(e.noise_reduction.detail, 50.0);
        assert_eq!(e.noise_reduction.smoothness, 50.0);
    }

    #[test]
    fn noise_reduction_roundtrips_json() {
        let e = Edits {
            noise_reduction: NoiseReduction {
                luminance: 30.0,
                detail: 70.0,
                color: 55.0,
                smoothness: 20.0,
            },
            ..Default::default()
        };
        let json = serde_json::to_string(&e).unwrap();
        let back: Edits = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
    }

    #[test]
    fn vignette_defaults_to_off() {
        let v = Vignette::default();
        assert_eq!(v.amount, 0.0);
        // Midpoint and feather sit at the middle of their 0..100 range; roundness's
        // neutral is 0, which is also the middle of its -100..100 range.
        assert_eq!(v.midpoint, 50.0);
        assert_eq!(v.feather, 50.0);
        assert_eq!(v.roundness, 0.0);
        assert_eq!(Edits::default().vignette, v);
    }

    #[test]
    fn vignette_is_active_only_on_amount() {
        assert!(!Vignette::default().is_active());
        // The other three only shape an effect that is off until amount is non-zero.
        assert!(!Vignette {
            midpoint: 10.0,
            roundness: -100.0,
            feather: 0.0,
            ..Default::default()
        }
        .is_active());
        assert!(Vignette {
            amount: -1.0,
            ..Default::default()
        }
        .is_active());
        assert!(Vignette {
            amount: 1.0,
            ..Default::default()
        }
        .is_active());
    }

    #[test]
    fn old_sidecar_without_vignette_still_loads() {
        let e: Edits = serde_json::from_str(r#"{"exposure": 0.5}"#).unwrap();
        assert_eq!(e.vignette, Vignette::default());
    }

    #[test]
    fn partial_vignette_json_keeps_modifier_defaults() {
        let e: Edits = serde_json::from_str(r#"{"vignette": {"amount": -40.0}}"#).unwrap();
        assert_eq!(e.vignette.amount, -40.0);
        assert_eq!(e.vignette.midpoint, 50.0);
        assert_eq!(e.vignette.feather, 50.0);
        assert_eq!(e.vignette.roundness, 0.0);
    }

    #[test]
    fn vignette_roundtrips_json() {
        let e = Edits {
            vignette: Vignette {
                amount: -55.0,
                midpoint: 30.0,
                roundness: 80.0,
                feather: 20.0,
            },
            ..Default::default()
        };
        let json = serde_json::to_string(&e).unwrap();
        let back: Edits = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
    }

    #[test]
    fn sharpening_defaults_to_off() {
        let s = Sharpening::default();
        assert_eq!(s.amount, 0.0);
        assert_eq!(s.masking, 0.0);
        // Radius is a physical pixel quantity, not a 0..100 slider; 1.0 px is the usual
        // starting point. Detail sits at its midpoint like every other modifier.
        assert_eq!(s.radius, 1.0);
        assert_eq!(s.detail, 50.0);
        assert_eq!(Edits::default().sharpening, s);
    }

    #[test]
    fn sharpening_is_active_only_on_amount() {
        assert!(!Sharpening::default().is_active());
        // Same rule as NoiseReduction and Vignette: the modifiers shape an effect that is
        // off until amount is non-zero, so dragging one alone must not insert a stage.
        assert!(!Sharpening {
            radius: 3.0,
            detail: 0.0,
            masking: 90.0,
            ..Default::default()
        }
        .is_active());
        assert!(Sharpening {
            amount: 1.0,
            ..Default::default()
        }
        .is_active());
    }

    #[test]
    fn old_sidecar_without_sharpening_still_loads() {
        let e: Edits = serde_json::from_str(r#"{"exposure": 0.5}"#).unwrap();
        assert_eq!(e.sharpening, Sharpening::default());
    }

    #[test]
    fn partial_sharpening_json_keeps_modifier_defaults() {
        let e: Edits = serde_json::from_str(r#"{"sharpening": {"amount": 60.0}}"#).unwrap();
        assert_eq!(e.sharpening.amount, 60.0);
        assert_eq!(e.sharpening.radius, 1.0);
        assert_eq!(e.sharpening.detail, 50.0);
        assert_eq!(e.sharpening.masking, 0.0);
    }

    #[test]
    fn sharpening_roundtrips_json() {
        let e = Edits {
            sharpening: Sharpening {
                amount: 70.0,
                radius: 2.5,
                detail: 30.0,
                masking: 80.0,
            },
            ..Default::default()
        };
        let json = serde_json::to_string(&e).unwrap();
        let back: Edits = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
    }

    #[test]
    fn local_contrast_defaults_to_off() {
        let lc = LocalContrast::default();
        assert_eq!(lc.texture, 0.0);
        assert_eq!(lc.clarity, 0.0);
        assert_eq!(Edits::default().local_contrast, lc);
    }

    #[test]
    fn local_contrast_is_active_when_either_slider_moves() {
        assert!(!LocalContrast::default().is_active());
        assert!(LocalContrast {
            texture: 1.0,
            clarity: 0.0
        }
        .is_active());
        assert!(LocalContrast {
            texture: 0.0,
            clarity: -1.0
        }
        .is_active());
    }

    #[test]
    fn old_sidecar_without_local_contrast_still_loads() {
        let e: Edits = serde_json::from_str(r#"{"exposure": 0.5}"#).unwrap();
        assert_eq!(e.local_contrast, LocalContrast::default());
    }

    #[test]
    fn local_contrast_roundtrips_json() {
        let e = Edits {
            local_contrast: LocalContrast {
                texture: 35.0,
                clarity: -60.0,
            },
            ..Default::default()
        };
        let json = serde_json::to_string(&e).unwrap();
        let back: Edits = serde_json::from_str(&json).unwrap();
        assert_eq!(e, back);
    }

    #[test]
    fn local_contrast_travels_with_the_tone_group() {
        // Texture and Clarity are *look*, which is what a preset carries; Detail is
        // capture-specific (noise, lens, output size). The generic partition tests above
        // prove disjointness and coverage — this pins which group it landed in.
        let src = all_nonneutral();
        let mut got = Edits::default();
        got.merge_from(&src, EditGroups::only(1)); // 1 == "Tone"
        assert_eq!(got.local_contrast, src.local_contrast);
        let mut not_detail = Edits::default();
        not_detail.merge_from(&src, EditGroups::only(3)); // 3 == "Detail"
        assert_eq!(not_detail.local_contrast, LocalContrast::default());
    }

    #[test]
    fn dehaze_defaults_to_off() {
        assert_eq!(Dehaze::default().amount, 0.0);
        assert_eq!(Edits::default().dehaze, Dehaze::default());
    }

    #[test]
    fn dehaze_is_active_in_both_directions() {
        assert!(!Dehaze::default().is_active());
        assert!(Dehaze { amount: 1.0 }.is_active());
        assert!(Dehaze { amount: -1.0 }.is_active());
    }

    #[test]
    fn old_sidecar_without_dehaze_still_loads() {
        let e: Edits = serde_json::from_str(r#"{"exposure": 0.5}"#).unwrap();
        assert_eq!(e.dehaze, Dehaze::default());
    }

    #[test]
    fn dehaze_roundtrips_json() {
        let e = Edits {
            dehaze: Dehaze { amount: -45.0 },
            ..Default::default()
        };
        let back: Edits = serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
        assert_eq!(e, back);
    }

    #[test]
    fn dehaze_travels_with_the_tone_group() {
        // ⚠ Tone, not Detail — even though dehaze sits beside noise reduction in the
        // *pipeline*. Pipeline order is about mathematical correctness; groups are about what
        // a person wants to copy between photos, and dehaze travels with a look.
        let src = all_nonneutral();
        let mut got = Edits::default();
        got.merge_from(&src, EditGroups::only(1)); // 1 == "Tone"
        assert_eq!(got.dehaze, src.dehaze);
        let mut not_detail = Edits::default();
        not_detail.merge_from(&src, EditGroups::only(3)); // 3 == "Detail"
        assert_eq!(not_detail.dehaze, Dehaze::default());
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

    /// An `Edits` with every field non-neutral, so a field that fails to transfer shows up.
    ///
    /// Deliberately does NOT use `..Default::default()`: adding a field to `Edits` should
    /// break this function, forcing whoever adds it to give it a group.
    fn all_nonneutral() -> Edits {
        let mut hsl = Hsl::default();
        hsl.bands[0].saturation = 20.0;
        Edits {
            version: 7,
            white_balance: WhiteBalance {
                temp: 10.0,
                tint: 5.0,
            },
            exposure: 0.5,
            contrast: 10.0,
            highlights: -10.0,
            shadows: 10.0,
            whites: 5.0,
            blacks: -5.0,
            tone_curve: ToneCurve {
                points: vec![[0.0, 0.0], [0.5, 0.6], [1.0, 1.0]],
            },
            saturation: 10.0,
            vibrance: 10.0,
            hsl,
            color_grading: ColorGrading {
                shadows: GradeWheel {
                    hue: 220.0,
                    saturation: 35.0,
                    luminance: -10.0,
                },
                highlights: GradeWheel {
                    hue: 40.0,
                    saturation: 30.0,
                    luminance: 8.0,
                },
                blending: 70.0,
                balance: 20.0,
                ..ColorGrading::default()
            },
            local_contrast: LocalContrast {
                texture: 40.0,
                clarity: -25.0,
            },
            dehaze: Dehaze { amount: -30.0 },
            noise_reduction: NoiseReduction {
                luminance: 30.0,
                color: 30.0,
                ..Default::default()
            },
            sharpening: Sharpening {
                amount: 60.0,
                ..Default::default()
            },
            lens_correction: LensCorrection {
                distortion: true,
                tca: true,
                vignetting: false,
                scale: 60.0,
            },
            perspective: Perspective {
                vertical: 40.0,
                horizontal: -20.0,
                aspect: 15.0,
                scale: 80.0,
            },
            vignette: Vignette {
                amount: -30.0,
                ..Default::default()
            },
            geometry: Geometry {
                orientation: Orientation {
                    rotate: Quarter::Cw90,
                    mirror: true,
                },
                angle: 2.0,
                crop: CropRect {
                    left: 0.1,
                    top: 0.1,
                    right: 0.9,
                    bottom: 0.9,
                },
            },
            masks: vec![MaskedAdjustment {
                mask: Mask::Linear(LinearGradient {
                    from: [0.0, 0.0],
                    to: [1.0, 1.0],
                    invert: false,
                }),
                adjustments: LocalAdjustments {
                    exposure: 0.75,
                    ..Default::default()
                },
            }],
        }
    }

    #[test]
    fn merging_all_groups_reproduces_the_source() {
        // ⚠ The coverage half of the partition proof. A field belonging to NO group would
        // silently never transfer — and this is what catches it, because the result would
        // keep the target's default for that field.
        let src = all_nonneutral();
        let mut got = Edits::default();
        got.merge_from(&src, EditGroups::ALL);
        assert_eq!(got, Edits { version: 0, ..src });
    }

    #[test]
    fn version_never_transfers() {
        // `version` is a schema marker, not an edit. The target keeps its own.
        let src = all_nonneutral();
        assert_ne!(src.version, 0);
        let mut got = Edits {
            version: 3,
            ..Edits::default()
        };
        got.merge_from(&src, EditGroups::ALL);
        assert_eq!(got.version, 3);
    }

    #[test]
    fn merging_no_groups_changes_nothing() {
        let src = all_nonneutral();
        let mut got = Edits::default();
        got.merge_from(&src, EditGroups::NONE);
        assert_eq!(got, Edits::default());
    }

    #[test]
    fn every_group_transfers_something() {
        // A group that moves no fields is dead weight, and would make the disjointness
        // test below pass vacuously for that group.
        let src = all_nonneutral();
        for (i, name) in EDIT_GROUP_NAMES.iter().enumerate() {
            let mut got = Edits::default();
            got.merge_from(&src, EditGroups::only(i));
            assert_ne!(got, Edits::default(), "group {name} transferred nothing");
        }
    }

    #[test]
    fn the_groups_are_disjoint() {
        // ⚠ The other half of the partition proof, and it needs no field list of its own.
        //
        // Clear group `g` from a fully non-neutral source. If some field belonged to both
        // `g` and `h`, clearing `g` would change what `h` transfers — so comparing group
        // `h`'s transfer before and after catches any overlap generically.
        let src = all_nonneutral();
        for (g, g_name) in EDIT_GROUP_NAMES.iter().enumerate() {
            let mut cleared = src.clone();
            cleared.merge_from(&Edits::default(), EditGroups::only(g));
            for (h, h_name) in EDIT_GROUP_NAMES.iter().enumerate() {
                if h == g {
                    continue;
                }
                let mut from_src = Edits::default();
                from_src.merge_from(&src, EditGroups::only(h));
                let mut from_cleared = Edits::default();
                from_cleared.merge_from(&cleared, EditGroups::only(h));
                assert_eq!(
                    from_src, from_cleared,
                    "clearing {g_name} changed what {h_name} transfers — the groups overlap"
                );
            }
        }
    }

    #[test]
    fn clearing_then_restoring_a_group_is_a_round_trip() {
        // Proves group `g` covers *exactly* the fields that `cleared` lost — not fewer.
        let src = all_nonneutral();
        for (g, name) in EDIT_GROUP_NAMES.iter().enumerate() {
            let mut round = src.clone();
            round.merge_from(&Edits::default(), EditGroups::only(g));
            assert_ne!(round, src, "group {name} did nothing");
            round.merge_from(&src, EditGroups::only(g));
            assert_eq!(
                round, src,
                "group {name} did not restore everything it cleared"
            );
        }
    }

    #[test]
    fn the_default_selection_excludes_geometry() {
        // A crop framed for one composition reframes another, and a preset built on a 3:2
        // crop would reshape every photo it touches.
        let g = EditGroups::default();
        assert!(!g.geometry);
        assert!(g.white_balance && g.tone && g.color && g.detail && g.vignette);
    }

    #[test]
    fn edit_groups_roundtrip_json_and_default_when_absent() {
        let g = EditGroups {
            geometry: true,
            tone: false,
            ..EditGroups::default()
        };
        let back: EditGroups = serde_json::from_str(&serde_json::to_string(&g).unwrap()).unwrap();
        assert_eq!(g, back);
        // An older file missing the field entirely still loads.
        let partial: EditGroups = serde_json::from_str(r#"{"geometry": true}"#).unwrap();
        assert!(partial.geometry);
        assert_eq!(partial.tone, EditGroups::default().tone);
    }

    #[test]
    fn group_accessors_match_the_name_table() {
        let mut g = EditGroups::NONE;
        for i in 0..EDIT_GROUP_NAMES.len() {
            assert!(!g.get(i));
            *g.get_mut(i) = true;
            assert!(g.get(i));
        }
        assert_eq!(g, EditGroups::ALL);
    }

    #[test]
    fn json_file_roundtrip() {
        let e = Edits {
            exposure: 1.25,
            geometry: Geometry {
                angle: 3.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let path = std::env::temp_dir().join("rustroom_edits_roundtrip.json");
        e.to_json_file(&path).unwrap();
        let back = Edits::from_json_file(&path).unwrap();
        assert_eq!(e, back);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_sidecar_from_before_masks_still_loads() {
        // `Edits` carries `#[serde(default)]`, so an older sidecar must load with no masks
        // rather than failing — the same forgiving-load property every added field needs.
        let e: Edits = serde_json::from_str(r#"{"exposure": 0.5}"#).unwrap();
        assert_eq!(e.exposure, 0.5);
        assert!(e.masks.is_empty());
    }

    #[test]
    fn a_masked_adjustment_round_trips_through_json() {
        let e = Edits {
            masks: vec![MaskedAdjustment {
                mask: Mask::Linear(LinearGradient {
                    from: [0.1, 0.2],
                    to: [0.8, 0.9],
                    invert: true,
                }),
                adjustments: LocalAdjustments {
                    exposure: -1.5,
                    ..Default::default()
                },
            }],
            ..Edits::default()
        };
        let text = serde_json::to_string(&e).unwrap();
        let back: Edits = serde_json::from_str(&text).unwrap();
        assert_eq!(back, e);
    }

    #[test]
    fn masks_do_not_transfer_by_default() {
        // ⚠ The same treatment geometry gets, for the same reason: a gradient positioned for
        // one composition is meaningless on another, so carrying it across photos by default
        // would be wrong more often than right.
        assert!(!EditGroups::default().masks);
        // ⚠ Via `get(6)` rather than the field: that is the path the dialog and the generic
        // partition tests index through, so this also checks the new arm exists — and a bare
        // `EditGroups::ALL.masks` is a compile-time constant, which clippy rejects.
        assert!(
            EditGroups::ALL.get(6),
            "but it must be reachable when asked for"
        );
    }

    #[test]
    fn the_masks_group_transfers_masks_and_nothing_else() {
        let src = all_nonneutral();
        let mut dst = Edits::default();
        // ⚠ The named constant, not a literal — this test hardcoded `6` and broke the moment a
        // group was inserted before Masks, which is exactly what `MASKS_GROUP` exists to prevent.
        dst.merge_from(&src, EditGroups::only(MASKS_GROUP));
        assert_eq!(dst.masks, src.masks);
        assert_eq!(dst.exposure, 0.0, "only the masks group was selected");
    }

    #[test]
    fn coverage_ramps_from_zero_at_from_to_one_at_to() {
        let g = LinearGradient {
            from: [0.0, 0.0],
            to: [1.0, 0.0],
            invert: false,
        };
        assert!((g.coverage(0.0, 0.0, 1.0) - 0.0).abs() < 1e-6);
        assert!((g.coverage(1.0, 0.0, 1.0) - 1.0).abs() < 1e-6);
        assert!((g.coverage(0.5, 0.0, 1.0) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn coverage_clamps_outside_the_axis() {
        let g = LinearGradient {
            from: [0.25, 0.0],
            to: [0.75, 0.0],
            invert: false,
        };
        assert_eq!(g.coverage(0.0, 0.0, 1.0), 0.0);
        assert_eq!(g.coverage(1.0, 0.0, 1.0), 1.0);
    }

    #[test]
    fn coverage_is_constant_perpendicular_to_the_axis() {
        // A graduated filter's whole point: it varies along one direction only.
        let g = LinearGradient {
            from: [0.0, 0.0],
            to: [1.0, 0.0],
            invert: false,
        };
        let a = g.coverage(0.3, 0.0, 1.0);
        for v in [0.1, 0.5, 0.9] {
            assert!((g.coverage(0.3, v, 1.0) - a).abs() < 1e-6);
        }
    }

    #[test]
    fn invert_mirrors_the_ramp() {
        let g = LinearGradient {
            from: [0.0, 0.0],
            to: [1.0, 0.0],
            invert: true,
        };
        assert!((g.coverage(0.0, 0.0, 1.0) - 1.0).abs() < 1e-6);
        assert!((g.coverage(1.0, 0.0, 1.0) - 0.0).abs() < 1e-6);
    }

    #[test]
    fn a_diagonal_gradient_is_not_skewed_by_a_non_square_frame() {
        // ⚠⚠ The subtle one, and the reason `coverage` takes an aspect at all.
        //
        // A *line* through two points survives anisotropic scaling — but lines *perpendicular*
        // to it do not. Compute coverage in raw normalised space and a diagonal gradient's
        // iso-lines skew on screen, which reads as a rendering glitch rather than a coordinate
        // error. This is the vignette's `roundness` problem in a different hat.
        //
        // On a 2:1 frame, take two points equidistant from the axis *in pixel space* and
        // assert they get the same coverage.
        let aspect = 2.0f32;
        let g = LinearGradient {
            from: [0.0, 0.0],
            to: [1.0, 1.0],
            invert: false,
        };
        // In units of frame height the axis runs (0,0) -> (2,1); its perpendicular is
        // (1,-2)/sqrt(5). Offsets are converted back to normalised by dividing x by `aspect`.
        let (px, py) = (1.0f32 / 5.0f32.sqrt(), -2.0f32 / 5.0f32.sqrt());
        let d = 0.1f32;
        let c1 = g.coverage(0.5 + d * px / aspect, 0.5 + d * py, aspect);
        let c2 = g.coverage(0.5 - d * px / aspect, 0.5 - d * py, aspect);
        assert!(
            (c1 - c2).abs() < 1e-5,
            "perpendicular offsets gave {c1} and {c2} — the gradient is skewed"
        );
    }

    #[test]
    fn a_degenerate_gradient_covers_everything() {
        // Both endpoints equal: no axis to project onto. Returns full coverage rather than
        // dividing by zero — the safe direction, since the alternative is NaN reaching the
        // blend and painting the frame with garbage.
        let g = LinearGradient {
            from: [0.5, 0.5],
            to: [0.5, 0.5],
            invert: false,
        };
        assert_eq!(g.coverage(0.1, 0.9, 1.5), 1.0);
    }

    /// An ellipse in the middle of the frame, half as wide as the frame and 60% as tall.
    fn radial() -> RadialGradient {
        RadialGradient {
            center: [0.5, 0.5],
            radius: [0.25, 0.3],
            feather: 50.0,
            invert: false,
        }
    }

    #[test]
    fn radial_coverage_is_one_at_the_centre_and_zero_outside() {
        let r = radial();
        assert!((r.coverage(0.5, 0.5, 1.0) - 1.0).abs() < 1e-6);
        // Well outside on each axis.
        assert_eq!(r.coverage(0.95, 0.5, 1.0), 0.0);
        assert_eq!(r.coverage(0.05, 0.5, 1.0), 0.0);
        assert_eq!(r.coverage(0.5, 0.95, 1.0), 0.0);
        assert_eq!(r.coverage(0.5, 0.05, 1.0), 0.0);
        // Exactly on the boundary: the ramp has reached zero.
        assert!(r.coverage(0.75, 0.5, 1.0).abs() < 1e-6);
    }

    #[test]
    fn radial_coverage_is_independent_of_aspect() {
        // ⚠⚠ The load-bearing test of this whole feature, and the only thing standing between
        // this design and a future reader "fixing" the missing aspect correction that three
        // sibling functions all genuinely need.
        //
        // An axis-aligned ellipse is immune to the anisotropic scaling between normalised and
        // screen space, because that scaling maps an axis-aligned ellipse to an axis-aligned
        // ellipse. `LinearGradient::coverage` needs the correction only because it projects
        // onto an axis, and *perpendicularity* is what anisotropic scaling destroys.
        //
        // ⚠ The probe sits off-centre on BOTH axes. A point on either axis would be
        // insensitive to a correction applied to the other one — which is the precise shape of
        // the mistake `hit_testing_the_line_is_aspect_correct` made in 3.1a.
        let r = radial();
        let (u, v) = (0.62, 0.58);
        let base = r.coverage(u, v, 1.0);
        assert!(
            base > 0.0 && base < 1.0,
            "probe must land on the ramp: {base}"
        );
        for aspect in [0.33, 1.5, 3.0, 10.0] {
            assert!(
                (r.coverage(u, v, aspect) - base).abs() < 1e-6,
                "aspect {aspect} changed coverage: {} vs {base}",
                r.coverage(u, v, aspect)
            );
        }
    }

    #[test]
    fn feather_zero_is_a_hard_edge() {
        let r = RadialGradient {
            feather: 0.0,
            ..radial()
        };
        // Just inside and just outside the boundary at u = 0.75.
        assert_eq!(r.coverage(0.74, 0.5, 1.0), 1.0);
        assert_eq!(r.coverage(0.76, 0.5, 1.0), 0.0);

        // ⚠⚠ **The probe that actually tests the `FEATHER_EPS` guard is this one**, exactly on
        // the boundary — and the two above cannot substitute for it.
        //
        // Without the guard the ramp computes `(1.0 - d) / 0.0`. For `d < 1` that is `+inf`,
        // which clamps to 1.0; for `d > 1` it is `-inf`, which clamps to 0.0. IEEE infinity
        // arithmetic therefore produces the *correct* answer everywhere except at `d == 1.0`,
        // where it is `0.0 / 0.0` — NaN, which `f32::clamp` propagates rather than clamping,
        // and which would reach the mask blend and paint garbage.
        //
        // This was found by deleting the guard and watching the two assertions above pass. A
        // test that names a guard it cannot detect the removal of is the exact failure this
        // codebase has now hit four times.
        //
        // `center[0] + radius[0]` is 0.5 + 0.25, all exact in binary, so `d` is exactly 1.0.
        let on_boundary = r.coverage(0.75, 0.5, 1.0);
        assert!(!on_boundary.is_nan(), "the boundary must not produce NaN");
        assert_eq!(on_boundary, 1.0, "a hard edge includes its own boundary");
    }

    #[test]
    fn feather_one_hundred_ramps_from_the_centre() {
        let r = RadialGradient {
            feather: 100.0,
            ..radial()
        };
        // At the centre, full; halfway to the boundary, strictly between; at it, zero.
        assert!((r.coverage(0.5, 0.5, 1.0) - 1.0).abs() < 1e-6);
        let mid = r.coverage(0.625, 0.5, 1.0);
        assert!(mid > 0.0 && mid < 1.0, "{mid}");
        assert!(r.coverage(0.75, 0.5, 1.0).abs() < 1e-6);
        // Contrast with feather 50, where the inner half is still flat at full coverage.
        assert!((radial().coverage(0.625, 0.5, 1.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn radial_invert_flips_coverage() {
        let r = RadialGradient {
            invert: true,
            ..radial()
        };
        assert!(r.coverage(0.5, 0.5, 1.0).abs() < 1e-6);
        assert!((r.coverage(0.95, 0.5, 1.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_zero_radius_radial_is_inert_not_nan() {
        // ⚠ Reachable only from a hand-edited sidecar — `Default` and the panel both build a
        // usable ellipse — but a NaN here would reach the blend and paint the frame with
        // garbage, exactly as `LinearGradient`'s zero-length guard prevents.
        //
        // The guard yields pre-invert coverage 0.0 and then falls through the invert, so an
        // inverted point-ellipse covers everything. That is the limit of the surrounding
        // behaviour rather than a special case: as the radius shrinks, an inverted mask covers
        // all but a vanishing point.
        let r = RadialGradient {
            radius: [0.0, 0.0],
            ..radial()
        };
        assert_eq!(r.coverage(0.5, 0.5, 1.0), 0.0);
        assert_eq!(r.coverage(0.1, 0.9, 2.0), 0.0);
        assert!(!r.coverage(0.5, 0.5, 1.0).is_nan());
        let inv = RadialGradient { invert: true, ..r };
        assert_eq!(inv.coverage(0.5, 0.5, 1.0), 1.0);
    }

    #[test]
    fn a_radial_default_is_a_usable_ellipse() {
        // ⚠ NOT a derived `Default`. Container-level `#[serde(default)]` fills missing fields
        // from the struct's `Default`, so a derived one would give every partial sidecar a
        // zero-radius point-ellipse — and `feather: 50` cannot come from `f32::default()`
        // under any arrangement.
        let d = RadialGradient::default();
        assert_eq!(d.center, [0.5, 0.5]);
        assert!(d.radius[0] > 0.0 && d.radius[1] > 0.0);
        assert_eq!(d.feather, 50.0);
        assert!(!d.invert);
        // It covers its own centre, which a derived default would not.
        assert!((d.coverage(0.5, 0.5, 1.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_partial_radial_json_gets_the_hand_written_defaults() {
        let r: RadialGradient = serde_json::from_str(r#"{"center":[0.25,0.75]}"#).unwrap();
        assert_eq!(r.center, [0.25, 0.75]);
        assert_eq!(r.radius, RadialGradient::default().radius);
        assert_eq!(r.feather, 50.0);
    }

    #[test]
    fn a_radial_round_trips_through_json() {
        let src = Mask::Radial(RadialGradient {
            center: [0.25, 0.75],
            radius: [0.4, 0.1],
            feather: 12.5,
            invert: true,
        });
        let back: Mask = serde_json::from_str(&serde_json::to_string(&src).unwrap()).unwrap();
        assert_eq!(back, src);
    }

    #[test]
    fn a_brush_defaults_to_no_strokes() {
        // An empty brush must be inert rather than covering everything: a mask added but not yet
        // painted has to do nothing at all.
        assert!(Brush::default().strokes.is_empty());
    }

    #[test]
    fn a_brush_round_trips_through_json() {
        let src = Brush {
            invert: false,
            strokes: vec![
                BrushStroke {
                    points: vec![[0.1, 0.2], [0.3, 0.4]],
                    radius: 0.05,
                    hardness: 60.0,
                    flow: 100.0,
                    erase: false,
                },
                BrushStroke {
                    points: vec![[0.5, 0.5]],
                    radius: 0.02,
                    hardness: 100.0,
                    flow: 100.0,
                    erase: true,
                },
            ],
        };
        let back: Brush = serde_json::from_str(&serde_json::to_string(&src).unwrap()).unwrap();
        assert_eq!(back, src);
    }

    #[test]
    fn a_partial_stroke_json_gets_defaults() {
        let s: BrushStroke = serde_json::from_str(r#"{"points":[[0.5,0.5]]}"#).unwrap();
        assert_eq!(s.points, vec![[0.5, 0.5]]);
        assert!(!s.erase, "a stroke paints unless it says otherwise");
    }

    #[test]
    fn the_analytic_masks_answer_coverage_directly() {
        // Both existing shapes are pure functions of position, so they answer `Some`. The brush
        // will not, which is what this method's `Option` exists to express.
        let lin = Mask::Linear(LinearGradient {
            from: [0.0, 0.0],
            to: [1.0, 0.0],
            invert: false,
        });
        assert_eq!(lin.analytic_coverage(0.5, 0.5, 1.0), Some(0.5));
        let rad = Mask::Radial(RadialGradient::default());
        assert_eq!(rad.analytic_coverage(0.5, 0.5, 1.0), Some(1.0));
    }

    #[test]
    fn a_brush_has_no_analytic_coverage() {
        // ⚠ The whole reason `analytic_coverage` returns an Option. A `0.0` here would compile,
        // run, and produce a mask that silently covers nothing.
        let m = Mask::Brush(Brush::default());
        assert_eq!(m.analytic_coverage(0.5, 0.5, 1.0), None);
    }

    #[test]
    fn every_mask_variant_has_a_label_and_an_invert() {
        // A table over every variant, so a fourth cannot ship with an empty list row or an
        // `invert` the panel's checkbox cannot reach.
        for mut m in [
            Mask::Linear(LinearGradient::default()),
            Mask::Radial(RadialGradient::default()),
            Mask::Brush(Brush::default()),
        ] {
            assert!(!m.label().is_empty());
            *m.invert_mut() = true;
            assert!(*m.invert_mut(), "invert_mut must expose the real field");
        }
    }

    #[test]
    fn an_existing_sidecar_without_a_brush_still_loads() {
        // ⚠ `Mask` is externally tagged, so every sidecar written before M3.1c holds
        // `{"Linear": {...}}`. Adding a variant is backward compatible — but a switch to an
        // untagged representation would silently break every file on disk.
        //
        // ⚠ Assert the PAYLOAD, not the variant. Under `#[serde(untagged)]` serde reads
        // `LinearGradient` from the whole object, and its container-level `#[serde(default)]`
        // makes the unknown "Linear" key be ignored and every field default — so the variant
        // still matches with `from`/`to` silently `[0,0]`.
        let json = r#"{"masks":[{"mask":{"Linear":{"from":[0.0,0.0],"to":[1.0,0.0],
                      "invert":false}},"adjustments":{"exposure":1.0}}]}"#;
        let e: Edits = serde_json::from_str(json).unwrap();
        assert_eq!(e.masks.len(), 1);
        assert_eq!(e.masks[0].adjustments.exposure, 1.0);
        let Mask::Linear(g) = &e.masks[0].mask else {
            panic!("expected a linear gradient, got {:?}", e.masks[0].mask);
        };
        assert_eq!(
            g.to,
            [1.0, 0.0],
            "the endpoints must survive the round trip"
        );
    }

    #[test]
    fn a_sidecar_without_blending_still_gets_fifty() {
        // ⚠ `blending` defaults to 50, not 0, so `ColorGrading::default` must be hand-written.
        // A derived `Default` would give every partial sidecar hard-edged ranges, silently —
        // the same trap `RadialGradient::default` records.
        let e: Edits =
            serde_json::from_str(r#"{"color_grading":{"shadows":{"saturation":40.0}}}"#).unwrap();
        assert_eq!(e.color_grading.blending, 50.0);
        assert_eq!(e.color_grading.shadows.saturation, 40.0);
    }

    #[test]
    fn a_hue_with_no_saturation_is_inert() {
        // A hue is a direction, not a strength. Rotating the wheel with the saturation at zero
        // must not insert a stage.
        let g = ColorGrading {
            shadows: GradeWheel {
                hue: 210.0,
                saturation: 0.0,
                luminance: 0.0,
            },
            ..ColorGrading::default()
        };
        assert!(!g.is_active());
    }

    #[test]
    fn blending_and_balance_alone_are_inert() {
        // The rule `NoiseReduction::is_active` and `Vignette::is_active` already follow: a
        // modifier shapes an effect that is off until a wheel is turned up.
        let g = ColorGrading {
            blending: 100.0,
            balance: -80.0,
            ..ColorGrading::default()
        };
        assert!(!g.is_active());
    }

    #[test]
    fn any_wheel_with_saturation_or_luminance_is_active() {
        for g in [
            ColorGrading {
                shadows: GradeWheel {
                    saturation: 30.0,
                    ..GradeWheel::default()
                },
                ..ColorGrading::default()
            },
            ColorGrading {
                midtones: GradeWheel {
                    luminance: -20.0,
                    ..GradeWheel::default()
                },
                ..ColorGrading::default()
            },
            ColorGrading {
                highlights: GradeWheel {
                    saturation: 5.0,
                    ..GradeWheel::default()
                },
                ..ColorGrading::default()
            },
            ColorGrading {
                global: GradeWheel {
                    luminance: 1.0,
                    ..GradeWheel::default()
                },
                ..ColorGrading::default()
            },
        ] {
            assert!(g.is_active(), "{g:?} should be active");
        }
    }

    #[test]
    fn a_sidecar_without_scale_still_gets_a_hundred() {
        // ⚠ Third instance of this trap, after `RadialGradient` and `ColorGrading`: `scale` is
        // 100 at neutral, so `Default` must be hand-written. A derived one would give every
        // partial sidecar zero scale — black edges — silently.
        let e: Edits = serde_json::from_str(r#"{"lens_correction":{"distortion":true}}"#).unwrap();
        assert_eq!(e.lens_correction.scale, 100.0);
        assert!(e.lens_correction.distortion);
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

    #[test]
    fn lens_corrections_travel_on_their_own_group_not_with_detail() {
        // ⚠ The reason `lens` was split out of `detail`. They were bundled at first, which meant
        // syncing noise reduction to another photo also switched its lens corrections on or off —
        // two unrelated things travelling together. This pins both halves of the separation.
        let src = all_nonneutral();
        let lens_group = EDIT_GROUP_NAMES.iter().position(|n| *n == "Lens").unwrap();

        let mut only_detail = Edits::default();
        only_detail.merge_from(&src, EditGroups::only(3));
        assert_eq!(
            only_detail.lens_correction,
            LensCorrection::default(),
            "the Detail group must not carry lens corrections"
        );
        assert_eq!(only_detail.noise_reduction, src.noise_reduction);

        let mut only_lens = Edits::default();
        only_lens.merge_from(&src, EditGroups::only(lens_group));
        assert_eq!(only_lens.lens_correction, src.lens_correction);
        assert_eq!(
            only_lens.noise_reduction,
            NoiseReduction::default(),
            "the Lens group must not carry anything else"
        );
    }

    #[test]
    fn a_preset_that_excluded_effects_still_excludes_the_vignette() {
        // ⚠ The whole reason `vignette` carries `#[serde(alias = "effects")]`. `EditGroups` has a
        // container-level `#[serde(default)]`, so without the alias this `false` would be dropped
        // and the field would fall back to its default of **true** — a preset deliberately built
        // to leave the vignette alone would silently start applying one. A renamed serialized
        // field is a migration, not a rename.
        let g: EditGroups = serde_json::from_str(r#"{"effects":false}"#).unwrap();
        assert!(!g.vignette, "the old key must still be honoured");

        // And the new name works, so the alias has not replaced the real field.
        let g: EditGroups = serde_json::from_str(r#"{"vignette":false}"#).unwrap();
        assert!(!g.vignette);
    }

    #[test]
    fn the_vignette_group_is_named_for_what_it_carries() {
        // It holds exactly one thing, and calling it "Effects" said otherwise.
        assert_eq!(EDIT_GROUP_NAMES[5], "Vignette");
    }

    /// ⚠ The value, not the mechanism. A derived `Default` would give `scale: 0`, and every
    /// sidecar written without the field would silently come back with the correction scaled
    /// away. Sabotage: `#[derive(Default)]` on `Perspective`.
    #[test]
    fn a_neutral_perspective_scales_fully() {
        assert_eq!(Perspective::default().scale, 100.0);
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

    /// ⚠ Both directions. That it *travels* with Geometry proves the arm exists; that it does
    /// **not** travel with any other group proves it was not put in two.
    #[test]
    fn perspective_travels_on_the_geometry_group_and_no_other() {
        let src = all_nonneutral();
        let mut with = Edits::default();
        with.merge_from(&src, EditGroups::only(GEOMETRY_GROUP));
        assert_eq!(with.perspective, src.perspective);

        for (g, name) in EDIT_GROUP_NAMES.iter().enumerate() {
            if g == GEOMETRY_GROUP {
                continue;
            }
            let mut without = Edits::default();
            without.merge_from(&src, EditGroups::only(g));
            assert_eq!(
                without.perspective,
                Perspective::default(),
                "group {g} ({name}) must not carry perspective"
            );
        }
    }
}
