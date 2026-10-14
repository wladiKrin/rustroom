//! Masks: the kinds, the AI prompts, and the adjustments a mask carries.

use crate::{
    Brush, ColorGrading, ColourRange, Dehaze, DepthRange, Hsl, LinearGradient, LocalContrast,
    LuminanceRange, NoiseReduction, RadialGradient, Refinement, Sharpening, ToneCurve,
    WhiteBalance,
};
use serde::{Deserialize, Serialize};

/// The mask primitives.
///
/// ⚠ **Not `Copy`** — `Mask::Brush` carries a `Vec<BrushStroke>`. [`MaskedAdjustment`] is
/// therefore not `Copy` either. Both are still `Clone`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Mask {
    Linear(LinearGradient),
    Radial(RadialGradient),
    Brush(Brush),
    /// ⚠ A function of the pixel's **lightness**, not of its position — see [`LuminanceRange`].
    /// Named for the quantity it selects on, leaving room for a colour range beside it.
    Luminance(LuminanceRange),
    /// ⚠ A function of the pixel's **hue**, not of its position — see [`ColourRange`]. The
    /// companion `Luminance` anticipated: "leaving room for a colour range beside it".
    ///
    /// ⚠ **Appended**, per `docs/PARALLEL-WORK.md` rule 3: `Mask` is externally tagged, so a new
    /// variant is backward compatible with every sidecar on disk, and appending is what git merges
    /// cleanly.
    Colour(ColourRange),
    /// ⚠ A neural network's answer (M9.13) — the subject, or the region under a click. Its coverage
    /// is a raster computed once from the unedited photo and cached **outside** the edits; this
    /// holds only what was asked and how to read the answer. **Appended**, rule 3 above.
    Ai(AiMask),
    /// ⚠ A band of the photo's estimated DEPTH (M10.16a) — a raster like `Ai`, from a depth network,
    /// computed once and cached outside the edits; this holds only the band. Not another `AiPrompt`:
    /// an `AiMask`'s Refine threshold means nothing for depth. **Appended**, rule 3 above.
    Depth(DepthRange),
}

/// What an AI mask asked for, and how its probability map becomes coverage.
///
/// ⚠ No pixels here: the map lives in the app's cache, keyed by the photo and this prompt, and a
/// mask whose map is not computed yet covers nothing.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AiMask {
    pub prompt: AiPrompt,
    /// −100..=100, labelled **Refine**: grows (+) or shrinks (−) the edge by moving the threshold on
    /// the soft map. ⚠ Not named `refine` — that is `MaskedAdjustment::refine`, the add/subtract list.
    pub threshold: f32,
    pub invert: bool,
}

/// What the network was asked.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub enum AiPrompt {
    /// The photo's salient subject.
    #[default]
    Subject,
    /// The region under a click, the point normalised over the **decoded** photo's own pixel grid —
    /// the space the map is stored in, so it stays put through every crop, rotation and lens change.
    Click { point: [f32; 2] },
}

impl Default for Mask {
    fn default() -> Self {
        Mask::Linear(LinearGradient::default())
    }
}

impl Mask {
    /// Coverage at `(u, v)` in normalised frame coordinates, for the **analytic** shapes.
    ///
    /// `None` means the shape is **not** a function of position — a brush, which is sampled data,
    /// or a luminance range, which reads the pixel. It is an `Option` rather than a `0.0` fallback
    /// because a fallback compiles, runs, and silently produces a mask that covers nothing; the
    /// `Option` forces every caller to decide.
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
            // A function of the pixel, not of `(u, v)` — `LuminanceRange::coverage` answers for
            // this one, from inside `MaskStage`'s pixel loop where the sample is in hand.
            Mask::Luminance(_) => None,
            // ⚠⚠ `None` for the same reason, and it is worth stating plainly because the obvious
            // reading of "a new variant must return `Some(..)`" is unimplementable here: a colour
            // range selects on the pixel's **hue and chroma**, and this function is handed a
            // position and an aspect. `Some(_)` would have to invent a number, which is the
            // silent-mask failure the `Option` exists to prevent, one level worse than the `0.0`
            // fallback it replaced. `ColourRange::coverage_at` answers, from `MaskStage`'s pixel
            // loop, exactly as the luminance range's does.
            Mask::Colour(_) => None,
            // A raster from outside the pipeline — `MaskStage`'s `CoverageSource::Raster` answers.
            Mask::Ai(_) | Mask::Depth(_) => None,
        }
    }

    /// The shape's name, for the mask list in the UI.
    pub fn label(&self) -> &'static str {
        match self {
            Mask::Linear(_) => "Gradient",
            Mask::Radial(_) => "Radial",
            Mask::Brush(_) => "Brush",
            Mask::Luminance(_) => "Luminance",
            Mask::Colour(_) => "Colour",
            Mask::Ai(AiMask {
                prompt: AiPrompt::Subject,
                ..
            }) => "Subject",
            Mask::Ai(_) => "Selection",
            Mask::Depth(_) => "Depth",
        }
    }

    /// Whether this component's coverage comes from a map the app computes outside the edits — an AI
    /// mask's or a depth range's — and so covers nothing until that map has arrived.
    ///
    /// ⚠ Exhaustive on purpose: the ONE answer to "does this wait for a map", which the render's,
    /// the tint's and the panel's checks all ask (M10.16a).
    pub fn reads_a_map(&self) -> bool {
        match self {
            Mask::Ai(_) | Mask::Depth(_) => true,
            Mask::Linear(_)
            | Mask::Radial(_)
            | Mask::Brush(_)
            | Mask::Luminance(_)
            | Mask::Colour(_) => false,
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
            Mask::Luminance(r) => &mut r.invert,
            Mask::Colour(r) => &mut r.invert,
            Mask::Ai(a) => &mut a.invert,
            Mask::Depth(r) => &mut r.invert,
        }
    }
}

/// What a mask can carry: **everything the tail applies per pixel or per mask**, which since
/// M7.2 is most of the tail.
///
/// ⚠ Not an arbitrary subset, and no longer the per-pixel set the earlier version of this
/// comment described. It is exactly the set that lives in the **tail**, where "run the stage on
/// a clone, then blend the result by coverage" is correct. A spatial stage in that list —
/// local contrast, sharpening — reads neighbours from *outside* the mask, which is what avoids
/// a hard edge at the mask boundary and is the behaviour wanted.
///
/// ⚠⚠ **What is NOT here is the prefix: dehaze and noise reduction.** Both run before geometry
/// while [`crate::Edits::masks`] is applied in the tail, so a masked one would act *after* the
/// tone curve — a different operation from the global stage wearing the same name, which is the
/// failure the Whites slider already cost this project once. Dehaze additionally estimates a
/// frame-global airlight, which has no meaning confined to a region. See `docs/BACKLOG.md`,
/// "The rest of the stages inside a mask", Part B.
///
/// ⚠⚠ **Two of these run in the PREFIX and the rest in the tail.** `noise_reduction` and `dehaze`
/// are corrections of the *capture*, and they are placed where the global stages of the same name
/// run rather than where [`MaskedAdjustment`] happens to be applied. `pipeline::capture_stages`
/// builds that pair; `pipeline::adjustment_stages` builds everything else, and the two partition
/// this struct.
///
/// ⚠ **Not `Copy`** — [`ToneCurve`] carries a `Vec`. [`MaskedAdjustment`] was already not `Copy`
/// for [`Mask::Brush`]'s stroke list, so dropping it here cost **zero** call sites, measured
/// with `cargo check --workspace --all-targets` rather than assumed. Still `Clone`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct LocalAdjustments {
    /// Noise reduction confined to the mask — denoise a shadowed corner without smearing the
    /// rest of the frame.
    ///
    /// ⚠⚠ **This runs in the PREFIX, not where the mask's other adjustments run** — the only
    /// field here that does, together with `dehaze`. It is placed exactly where the global stage
    /// runs, so a masked Luminance 50 means what a global Luminance 50 means.
    pub noise_reduction: NoiseReduction,
    /// Dehaze confined to the mask — clear a distant ridge and leave the foreground alone.
    ///
    /// ⚠⚠ **Prefix, like `noise_reduction`, and for a sharper reason: the airlight.** It is
    /// estimated from the brightest 0.1% of the buffer the stage is handed. In the tail that
    /// buffer is the visible *region*, so a tail-side masked dehaze would re-estimate its
    /// airlight as you panned and the photograph would change under you. The prefix always runs
    /// whole-buffer, which is what makes a frame-global airlight possible at all.
    pub dehaze: Dehaze,
    /// ⚠ **Legacy since M10.12b**: an absolute white balance, as masks had until 2026-10-12. Read
    /// as the shift it amounted to (`pipeline::mask_wb_shift`) and cleared by the panel's first
    /// Temp or Tint edit; nothing new writes it.
    pub white_balance: WhiteBalance,
    /// Lightroom's local Temp, −100..=100: a **shift** of the photo's own white balance where the
    /// mask covers — `+` warmer (M10.12b). Like every scene slider here, an offset added to the
    /// global value per pixel by `pipeline`'s `SceneOffsetsStage`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub temp: f32,
    /// Lightroom's local Tint, −100..=100: a shift toward magenta (`+`) or green, in the global
    /// Tint's units (M10.12b).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub tint: f32,
    pub exposure: f32,
    pub contrast: f32,
    pub highlights: f32,
    pub shadows: f32,
    pub whites: f32,
    pub blacks: f32,
    /// A point tone curve confined to the mask — dodge and burn with a curve.
    ///
    /// ⚠ Runs **after** the tone regions and **before** local contrast, mirroring the global
    /// chain: a local contrast measured before the curve would then be re-mapped by it.
    ///
    /// ⚠ This field is why [`LocalAdjustments`] is not `Copy`.
    pub tone_curve: ToneCurve,
    pub saturation: f32,
    pub vibrance: f32,
    /// Local defringe, −100..=100 (M9.6), Lightroom's meaning: **negative protects** the masked
    /// area from the global defringe, positive adds defringe there with the global hue ranges.
    /// ⚠ Runs in the PREFIX, in the one defringe step after geometry, not with this mask's other
    /// adjustments — global and local are one computation.
    pub defringe: f32,
    /// Fine and coarse local contrast, carried by the mask.
    ///
    /// ⚠ Negative `texture` is what makes skin softening reachable without masked noise
    /// reduction, which is a later slice — it removes the fine band and softens edges within it.
    pub local_contrast: LocalContrast,
    /// ⚠ Full parity with the global control, `masking` included — which inside a mask is a
    /// second gate inside a gate, since the mask has already answered *where*. Kept because
    /// parity costs nothing here and an edge gate is defensible for a loose mask over mixed
    /// content. A deliberate choice (2026-09-04), not an oversight.
    pub sharpening: Sharpening,
    /// ⚠ 24 values — an order of magnitude more state than anything else here, and the reason a
    /// mask's panel section is markedly taller than it was. Also the **most expensive** thing a
    /// mask can carry: measured, global HSL is +34.9 ms at preview against Clarity's +8.8 ms.
    /// "Per-pixel" says nothing about cost — the price is an Oklab round trip plus `atan2`,
    /// `cos` and `sin` for every pixel.
    pub hsl: Hsl,
    /// Three-way colour grading confined to the mask — split-tone a sky, warm a face.
    ///
    /// ⚠ Runs **after** HSL, mirroring the global chain: HSL corrects colours that are *in* the
    /// picture while a grade is laid on top, so reversed an HSL band tweak would act on the tint
    /// just added.
    ///
    /// ⚠⚠ **Its exclusion from masks was re-decided, not inherited (2026-09-05).** The stated
    /// reason was *"`LocalAdjustments` is per-pixel-only and a grade is global look"*; M7.2 made
    /// the first half false, and the second half is a judgement about taste rather than about
    /// structure.
    ///
    /// ⚠ `blending` and `balance` are carried too. They are properties of the *range
    /// decomposition* shared by all four wheels, so omitting them would make a masked grade a
    /// different control from the global one wearing the same name.
    pub color_grading: ColorGrading,
}

impl LocalAdjustments {
    /// Whether this would change any pixel. An inactive set inserts no stages at all.
    pub fn is_active(&self) -> bool {
        *self != Self::default()
    }

    /// Whether this carries a **scene** slider — white balance, exposure, contrast or a tone
    /// region — which acts as an offset to the global one at its own position (M10.12b), rather
    /// than after the global look like the rest.
    pub fn has_scene_offsets(&self) -> bool {
        self.white_balance.is_active()
            || [
                self.temp,
                self.tint,
                self.exposure,
                self.contrast,
                self.highlights,
                self.shadows,
                self.whites,
                self.blacks,
            ]
            .iter()
            .any(|v| *v != 0.0)
    }
}

fn is_zero(v: &f32) -> bool {
    *v == 0.0
}

/// One mask and the adjustments it carries.
/// ⚠ Not `Copy`: [`Mask::Brush`] carries a stroke list. Still `Clone`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct MaskedAdjustment {
    /// The composite's **base** — the first shape, before any refinement.
    pub mask: Mask,
    /// Shapes composited onto `mask`, folded **left to right** by [`MaskOp`].
    ///
    /// ⚠⚠ **A list, not a recursive [`Mask`] variant.** Keeping `Mask` flat is what leaves
    /// [`Mask::analytic_coverage`]'s exhaustive match, [`Mask::label`], [`Mask::invert_mut`] and
    /// every sidecar on disk untouched; the cost is that a composite cannot contain a composite,
    /// which is the nesting the recursive variant would have bought.
    ///
    /// ⚠ Order is **observable** — subtract-then-intersect is not intersect-then-subtract — so
    /// the panel appends rather than inserting.
    ///
    /// ⚠ `skip_serializing_if`: an empty list is **absent** from the sidecar, never `[]`, so a
    /// mask that has never been refined writes the bytes it always did. `lens_correction`'s rule.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refine: Vec<Refinement>,
    pub adjustments: LocalAdjustments,
    /// Switched off: the mask's adjustments are not applied — in the view, the export,
    /// anywhere — until it is switched back on. The panel's toggle beside the eye (2026-09-20).
    ///
    /// ⚠ **Part of the edits, not view state**, because the user asked for an off mask to stay
    /// out of the export: it travels with the mask through the sidecar, a paste, a sync, a
    /// preset and undo. The patch hold-out is view state and deliberately different.
    ///
    /// ⚠ `skip_serializing_if`: an enabled mask writes the bytes it always did, so every sidecar
    /// on disk is unchanged and an old build reading a new sidecar simply applies the mask.
    /// `disabled` rather than `enabled` so `Default` (false) is the applied state and no
    /// hand-written `Default` is needed.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub disabled: bool,
    /// Inverts the **whole composite**: coverage is `1 −` the rows folded top to bottom (M10.12a).
    ///
    /// ⚠ Not a shape's own `invert` ([`Mask::invert_mut`]), which flips one shape **before** it
    /// folds. On a one-shape mask the two are the same coverage, which is why the panel draws this
    /// one only for a composite, or while it is on (`mask_actions::whole_invert_drawn`).
    ///
    /// ⚠ `skip_serializing_if`: an uninverted mask writes the bytes it always did — every sidecar,
    /// preset and rendered-preview key on disk is unchanged. `disabled`'s rule.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub invert: bool,
}

impl MaskedAdjustment {
    /// Whether any component — the base or a refinement — reads the photograph rather than the
    /// position: a luminance or colour range, or a brush that gates on the image
    /// ([`Brush::needs_base`]). M10.12b: such a mask carrying a scene offset selects on the
    /// globally edited photo (`pipeline::reads_reference`).
    ///
    /// ⚠ Exhaustive on purpose: a new `Mask` variant is a compile error here, never a silent
    /// "reads nothing".
    pub fn reads_pixels(&self) -> bool {
        std::iter::once(&self.mask)
            .chain(self.refine.iter().map(|r| &r.mask))
            .any(|k| match k {
                Mask::Brush(b) => b.needs_base(),
                Mask::Luminance(_) | Mask::Colour(_) => true,
                Mask::Linear(_) | Mask::Radial(_) | Mask::Ai(_) | Mask::Depth(_) => false,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Edits, GradeWheel, HslBand, MaskOp};

    /// M10.16a: a depth range travels, names itself, inverts, and is a raster — never analytic.
    #[test]
    fn a_depth_mask_round_trips_and_answers_as_a_raster() {
        let r = crate::DepthRange {
            near: 30.0,
            far: 70.0,
            feather: 20.0,
            invert: false,
        };
        let mut m = Mask::Depth(r);
        let back: Mask = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
        assert_eq!(back, m);
        assert_eq!(m.label(), "Depth");
        assert_eq!(m.analytic_coverage(0.5, 0.5, 1.5), None);
        *m.invert_mut() = true;
        assert!(matches!(
            m,
            Mask::Depth(crate::DepthRange { invert: true, .. })
        ));
    }

    /// ⚠ Every variant listed by hand, so a new one is a decision here rather than a default.
    #[test]
    fn exactly_the_ai_and_depth_components_read_a_map() {
        let every = [
            (Mask::Linear(LinearGradient::default()), false),
            (Mask::Radial(RadialGradient::default()), false),
            (Mask::Brush(Brush::default()), false),
            (Mask::Luminance(LuminanceRange::default()), false),
            (Mask::Colour(ColourRange::default()), false),
            (Mask::Ai(AiMask::default()), true),
            (Mask::Depth(crate::DepthRange::default()), true),
        ];
        for (m, reads) in &every {
            assert_eq!(m.reads_a_map(), *reads, "{}", m.label());
            // A label per variant — the exhaustive match below would not compile without each.
            match m {
                Mask::Linear(_)
                | Mask::Radial(_)
                | Mask::Brush(_)
                | Mask::Luminance(_)
                | Mask::Colour(_)
                | Mask::Ai(_)
                | Mask::Depth(_) => {}
            }
        }
        let depth = MaskedAdjustment {
            mask: Mask::Depth(crate::DepthRange::default()),
            ..Default::default()
        };
        assert!(
            !depth.reads_pixels(),
            "a depth map is not the photo's pixels"
        );
    }

    /// ⚠ Both halves, and both on their own. A test setting both could not tell a wired field
    /// from an unwired one sitting beside a wired one.
    #[test]
    fn a_masked_noise_reduction_or_dehaze_makes_the_set_active() {
        assert!(!LocalAdjustments::default().is_active());

        let denoised = LocalAdjustments {
            noise_reduction: NoiseReduction {
                luminance: 40.0,
                ..NoiseReduction::default()
            },
            ..Default::default()
        };
        assert!(
            denoised.is_active(),
            "masked noise reduction must be active"
        );

        let hazed = LocalAdjustments {
            dehaze: Dehaze { amount: 60.0 },
            ..Default::default()
        };
        assert!(hazed.is_active(), "masked dehaze must be active");
    }

    /// ⚠ `is_active` is amount-only on both, reused rather than reinvented: Detail and Smoothness
    /// shape an effect that is off until Luminance or Color is turned up, so moving one alone
    /// must not insert a whole-buffer stage into the prefix.
    #[test]
    fn masked_noise_reduction_modifiers_alone_are_inert() {
        let a = LocalAdjustments {
            noise_reduction: NoiseReduction {
                detail: 90.0,
                smoothness: 10.0,
                ..NoiseReduction::default()
            },
            ..Default::default()
        };
        assert!(
            !a.noise_reduction.is_active(),
            "detail and smoothness alone must insert no stage"
        );
    }

    /// ⚠ A sidecar written before today has neither field, and both must read as neutral — with
    /// `detail` and `smoothness` at 50 from `NoiseReduction`'s hand-written `Default`, not 0.
    #[test]
    fn a_mask_written_before_the_capture_fields_reads_them_neutral() {
        let json = r#"{"mask":{"Linear":{"from":[0.0,0.0],"to":[1.0,0.0]}},
                       "adjustments":{"exposure":0.5}}"#;
        let m: MaskedAdjustment = serde_json::from_str(json).unwrap();
        assert_eq!(
            m.adjustments.exposure, 0.5,
            "fixture check: the json was read"
        );
        assert_eq!(m.adjustments.noise_reduction, NoiseReduction::default());
        assert_eq!(m.adjustments.noise_reduction.detail, 50.0);
        assert_eq!(m.adjustments.dehaze, Dehaze::default());
    }

    /// ⚠ Both halves. A mask carrying only a curve, and a mask carrying only a grade, must each
    /// read as active on their own — a test that set both could not tell a wired field from an
    /// unwired one sitting next to a wired one.
    #[test]
    fn a_masked_curve_or_a_masked_grade_makes_the_set_active() {
        assert!(
            !LocalAdjustments::default().is_active(),
            "the default set must stay inactive, or every mask would insert stages"
        );

        let curved = LocalAdjustments {
            tone_curve: ToneCurve {
                points: vec![[0.0, 0.0], [0.5, 0.7], [1.0, 1.0]],
            },
            ..Default::default()
        };
        assert!(curved.is_active(), "a masked tone curve must be active");

        let graded = LocalAdjustments {
            color_grading: ColorGrading {
                shadows: GradeWheel {
                    hue: 220.0,
                    saturation: 40.0,
                    luminance: -10.0,
                },
                ..ColorGrading::default()
            },
            ..Default::default()
        };
        assert!(graded.is_active(), "a masked colour grade must be active");
    }

    /// ⚠ A sidecar written before today has neither field. It must read as neutral, and the grade
    /// must come back with `blending: 50` from `ColorGrading`'s hand-written `Default` — a derived
    /// one would give it 0, i.e. hard-edged ranges, silently.
    #[test]
    fn a_mask_written_before_these_fields_reads_them_neutral() {
        let json = r#"{"mask":{"Linear":{"from":[0.0,0.0],"to":[1.0,0.0]}},
                       "adjustments":{"exposure":0.5}}"#;
        let m: MaskedAdjustment = serde_json::from_str(json).unwrap();
        assert_eq!(
            m.adjustments.exposure, 0.5,
            "fixture check: the json was read"
        );
        assert!(m.adjustments.tone_curve.points.is_empty());
        assert_eq!(m.adjustments.color_grading, ColorGrading::default());
        assert_eq!(
            m.adjustments.color_grading.blending, 50.0,
            "a missing grade must come back at the hand-written neutral, not f32::default()"
        );
    }

    /// An `Edits` with every field non-neutral, so a field that fails to transfer shows up.
    ///
    /// Deliberately does NOT use `..Default::default()`: adding a field to `Edits` should
    /// break this function, forcing whoever adds it to give it a group.
    /// A mask can carry the tail-native adjustments, and the default stays inert.
    ///
    /// ⚠ The second assertion is the load-bearing one: `Sharpening::default()` is **hand-written**
    /// (radius 1.0, detail 50.0), not all-zeros. If `is_active` ever compared fields rather than
    /// the whole struct against its default, every existing sidecar would silently gain stages.
    #[test]
    fn a_mask_can_carry_the_tail_native_adjustments() {
        let a = LocalAdjustments {
            local_contrast: LocalContrast {
                texture: 40.0,
                clarity: -30.0,
            },
            sharpening: Sharpening {
                amount: 55.0,
                ..Sharpening::default()
            },
            hsl: Hsl {
                bands: [HslBand {
                    hue: 12.0,
                    saturation: 0.0,
                    luminance: 0.0,
                }; 8],
            },
            ..LocalAdjustments::default()
        };
        assert!(
            a.is_active(),
            "a mask carrying only spatial values must count as active"
        );
        assert!(
            !LocalAdjustments::default().is_active(),
            "the default must stay inert"
        );
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
    fn a_mask_with_no_refinements_writes_the_bytes_it_always_did() {
        // ⚠⚠ The `skip_serializing_if` guard, and the reason it is there: a photo that has never
        // used a composite must write the same sidecar it wrote yesterday. `lens_correction`'s
        // rule, applied to a `Vec` rather than an `Option`.
        let m = MaskedAdjustment {
            mask: Mask::Linear(LinearGradient::default()),
            ..Default::default()
        };
        let s = serde_json::to_string(&m).unwrap();
        assert!(
            !s.contains("refine"),
            "an empty refinement list must be absent, never `[]`: {s}"
        );
    }

    #[test]
    fn a_sidecar_from_before_refinements_loads_with_none() {
        let json = r#"{"mask":{"Linear":{"from":[0.0,0.0],"to":[0.0,1.0],"invert":false}}}"#;
        let m: MaskedAdjustment = serde_json::from_str(json).unwrap();
        assert!(m.refine.is_empty());
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
                // ⚠ A composite, so the round trip covers the refinements rather than defaulting
                // them away — the half of the sidecar a new field is most likely to be lost in.
                refine: vec![Refinement {
                    op: MaskOp::Intersect,
                    mask: Mask::Luminance(LuminanceRange::default()),
                }],
                adjustments: LocalAdjustments {
                    exposure: -1.5,
                    ..Default::default()
                },
                disabled: false,
                invert: false,
            }],
            ..Edits::default()
        };
        let text = serde_json::to_string(&e).unwrap();
        let back: Edits = serde_json::from_str(&text).unwrap();
        assert_eq!(back, e);
    }

    #[test]
    fn every_mask_variant_has_a_label_and_an_invert() {
        // A table over every variant, so a fourth cannot ship with an empty list row or an
        // `invert` the panel's checkbox cannot reach.
        for mut m in [
            Mask::Linear(LinearGradient::default()),
            Mask::Radial(RadialGradient::default()),
            Mask::Brush(Brush::default()),
            Mask::Luminance(LuminanceRange::default()),
            Mask::Colour(ColourRange::default()),
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
    fn an_ai_mask_round_trips_both_prompts_and_inverts() {
        for prompt in [
            AiPrompt::Subject,
            AiPrompt::Click {
                point: [0.25, 0.75],
            },
        ] {
            let m = Mask::Ai(AiMask {
                prompt,
                threshold: -30.0,
                invert: false,
            });
            let back: Mask = serde_json::from_str(&serde_json::to_string(&m).unwrap()).unwrap();
            assert_eq!(back, m);
        }
        let mut m = Mask::Ai(AiMask::default());
        *m.invert_mut() = true;
        assert!(matches!(m, Mask::Ai(AiMask { invert: true, .. })));
        assert_eq!(m.label(), "Subject");
        assert_eq!(m.analytic_coverage(0.5, 0.5, 1.5), None);
        // A partial AI mask fills from its defaults.
        let m: Mask = serde_json::from_str(r#"{"Ai":{"prompt":"Subject"}}"#).unwrap();
        assert_eq!(m, Mask::Ai(AiMask::default()));
    }
}

#[cfg(test)]
mod disabled_mask_tests {
    use super::*;

    #[test]
    fn a_switched_off_mask_round_trips_and_an_enabled_one_writes_nothing_new() {
        // ⚠ Both halves: the flag must survive the sidecar, and an enabled mask must serialise
        // exactly as before this field existed, so every sidecar on disk is unchanged.
        let on = MaskedAdjustment::default();
        let text = serde_json::to_string(&on).unwrap();
        assert!(
            !text.contains("disabled"),
            "an enabled mask wrote the new field: {text}"
        );
        let off = MaskedAdjustment {
            disabled: true,
            ..Default::default()
        };
        let text = serde_json::to_string(&off).unwrap();
        assert!(text.contains("\"disabled\":true"), "{text}");
        let back: MaskedAdjustment = serde_json::from_str(&text).unwrap();
        assert!(back.disabled);
        // ⚠ And a sidecar from before the field reads as enabled.
        let old: MaskedAdjustment = serde_json::from_str("{}").unwrap();
        assert!(!old.disabled);
    }

    #[test]
    fn a_whole_mask_invert_round_trips_and_an_uninverted_mask_writes_nothing_new() {
        // ⚠ Both halves, `disabled`'s rule: the flag survives the sidecar, and a mask that is
        // not inverted serialises exactly as before the field existed.
        //
        // ⚠ Read as a JSON *object*, not as text: the shape nested under "mask" serialises its
        // own `"invert":false`, so a substring test for "invert" fails on correct code.
        let plain = serde_json::to_value(MaskedAdjustment::default()).unwrap();
        assert!(
            plain.get("invert").is_none(),
            "an uninverted mask wrote the flag: {plain}"
        );
        let inverted = MaskedAdjustment {
            invert: true,
            ..Default::default()
        };
        let v = serde_json::to_value(&inverted).unwrap();
        assert_eq!(v["invert"], true, "{v}");
        let mut back: MaskedAdjustment = serde_json::from_value(v).unwrap();
        assert!(back.invert);
        assert!(
            !*back.mask.invert_mut(),
            "the shape's own Invert is a separate flag"
        );
        let old: MaskedAdjustment = serde_json::from_str("{}").unwrap();
        assert!(
            !old.invert,
            "a sidecar from before the field reads uninverted"
        );
    }
}

#[cfg(test)]
mod scene_offset_field_tests {
    use super::*;
    use crate::{BrushStroke, MaskOp};

    /// M10.12b: a mask reads the photograph through ANY component — base or refinement — that is a
    /// range or a gating brush; position-only shapes and AI maps do not.
    #[test]
    fn a_mask_reads_pixels_through_any_range_or_gating_component() {
        let lin = Mask::Linear(LinearGradient::default());
        let m = |mask: Mask, refine: Vec<Refinement>| MaskedAdjustment {
            mask,
            refine,
            ..Default::default()
        };
        assert!(!m(lin.clone(), vec![]).reads_pixels());
        assert!(!m(Mask::Radial(Default::default()), vec![]).reads_pixels());
        assert!(!m(Mask::Ai(Default::default()), vec![]).reads_pixels());
        assert!(m(Mask::Luminance(LuminanceRange::default()), vec![]).reads_pixels());
        assert!(m(Mask::Colour(ColourRange::default()), vec![]).reads_pixels());
        let range = Refinement {
            op: MaskOp::Intersect,
            mask: Mask::Luminance(LuminanceRange::default()),
        };
        assert!(
            m(lin.clone(), vec![range]).reads_pixels(),
            "a refinement counts"
        );
        let gating = Brush {
            strokes: vec![BrushStroke {
                auto_mask: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(gating.needs_base(), "fixture check");
        assert!(m(Mask::Brush(gating), vec![]).reads_pixels());
        let plain = Brush {
            strokes: vec![BrushStroke::default()],
            ..Default::default()
        };
        assert!(!m(Mask::Brush(plain), vec![]).reads_pixels());
    }

    #[test]
    fn temp_and_tint_round_trip_and_a_default_writes_neither() {
        // ⚠ Read as a JSON object: `white_balance` nests its own `tint`, so a substring test for
        // "tint" fails on correct code (M10.12a's lesson).
        let plain = serde_json::to_value(LocalAdjustments::default()).unwrap();
        assert!(plain.get("temp").is_none(), "{plain}");
        assert!(plain.get("tint").is_none(), "{plain}");
        let a = LocalAdjustments {
            temp: 30.0,
            tint: -12.0,
            ..Default::default()
        };
        let back: LocalAdjustments =
            serde_json::from_value(serde_json::to_value(&a).unwrap()).unwrap();
        assert_eq!((back.temp, back.tint), (30.0, -12.0));
    }

    #[test]
    fn has_scene_offsets_sees_each_scene_slider_and_nothing_else() {
        let each: Vec<(&str, LocalAdjustments)> = vec![
            (
                "exposure",
                LocalAdjustments {
                    exposure: 0.5,
                    ..Default::default()
                },
            ),
            (
                "contrast",
                LocalAdjustments {
                    contrast: 10.0,
                    ..Default::default()
                },
            ),
            (
                "highlights",
                LocalAdjustments {
                    highlights: -10.0,
                    ..Default::default()
                },
            ),
            (
                "shadows",
                LocalAdjustments {
                    shadows: 10.0,
                    ..Default::default()
                },
            ),
            (
                "whites",
                LocalAdjustments {
                    whites: 10.0,
                    ..Default::default()
                },
            ),
            (
                "blacks",
                LocalAdjustments {
                    blacks: -10.0,
                    ..Default::default()
                },
            ),
            (
                "temp",
                LocalAdjustments {
                    temp: 10.0,
                    ..Default::default()
                },
            ),
            (
                "tint",
                LocalAdjustments {
                    tint: 10.0,
                    ..Default::default()
                },
            ),
            (
                "a legacy Kelvin",
                LocalAdjustments {
                    white_balance: WhiteBalance {
                        kelvin: Some(6500.0),
                        tint: 0.0,
                    },
                    ..Default::default()
                },
            ),
        ];
        for (name, a) in &each {
            assert!(a.has_scene_offsets(), "{name} is a scene offset");
        }
        assert!(!LocalAdjustments::default().has_scene_offsets());
        let look = LocalAdjustments {
            saturation: 20.0,
            ..Default::default()
        };
        assert!(!look.has_scene_offsets(), "saturation is a look slider");
    }
}
