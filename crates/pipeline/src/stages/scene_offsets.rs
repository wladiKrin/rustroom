//! A mask's scene sliders as **offsets** to the global ones — Lightroom's model (M10.12b).
//!
//! In place of the global white balance, exposure, contrast and tone regions whenever an active
//! mask carries one of them: per pixel, the value used is `global + Σ coverage_i × offset_i`, then
//! the four stages' own arithmetic in their order, through the functions they themselves call. So a
//! mask's +1 EV at full coverage IS the global +1 EV there, half coverage is +0.5 EV, overlapping
//! masks add — and where no mask covers, the output is bit-identical to the four stages.
//!
//! ⚠⚠ Why it exists: a mask used to run its own white balance, exposure, contrast and tone regions
//! AFTER the global tone curve, saturation, HSL and grading — a mask's +1 EV landed on darks the
//! curve had already shaped (`examples/probe_mask_offsets.rs`: 14.7 levels mean, 85 at p99, 77 % of
//! pixels off by more than 3 levels on DSCF6291).

use crate::stages::{contrast, exposure, tone_regions};
use crate::{MaskStage, PipelineStage, RenderScale};
use core_image::ImageBuf;
use edit_model::{Edits, Geometry, MaskedAdjustment};

/// The global scene values.
struct Scene {
    /// The global white balance's gains (`[1, 1, 1]` when inactive).
    gains: [f32; 3],
    ev: f32,
    contrast: f32,
    /// `[highlights, shadows, whites, blacks]`, −100..=100.
    regions: [f32; 4],
}

/// One mask's contribution: where it covers, and what it adds there.
struct Contribution {
    /// A coverage-only stage: the mask's shape with no adjustments — `coverage_plane` is the one fold.
    coverage: MaskStage,
    ev: f32,
    contrast: f32,
    regions: [f32; 4],
    /// `ln(R)`, `R` the gains of the global white balance shifted by this mask, over the global gains.
    ln_r: [f32; 3],
    /// The mask's position in `edits.masks`.
    index: usize,
    /// Where this mask's coverage goes for its look stage — `Some` exactly when it
    /// [`reads_reference`], and then that coverage is computed on the reference photo.
    slot: Option<std::sync::Arc<CoverageSlot>>,
}

/// A coverage plane handed from [`SceneOffsetsStage`] to one mask's look stage (M10.12b): a mask
/// that reads the photograph and carries a scene offset selects ONCE, on the reference photo, and
/// its look sliders blend by that same plane — so neither half chases the other's effect.
///
/// ⚠ An `Arc` built with the pipeline and shared by the two stages, which run in order in one
/// render; each render overwrites it. A poisoned lock is taken as it stands, never a panic: the
/// worst a stale plane can do is select like the previous frame.
#[derive(Default)]
pub struct CoverageSlot(std::sync::Mutex<Option<std::sync::Arc<Vec<f32>>>>);

impl CoverageSlot {
    pub fn put(&self, plane: std::sync::Arc<Vec<f32>>) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(plane);
    }

    pub fn get(&self) -> Option<std::sync::Arc<Vec<f32>>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

pub struct SceneOffsetsStage {
    global: Scene,
    masks: Vec<Contribution>,
    /// The global look — the four scene stages at the global values, the curve, local contrast,
    /// saturation, HSL and the grade (`build.rs`' `global_look`) — run on a copy of the input to give
    /// the **reference photo**: the globally edited photo, before any mask. `Some` only when a
    /// contributing mask [`reads_reference`], so its cost is paid only there.
    ///
    /// ⚠ Behind an `Arc` so [`Self::duplicate`] — a GPU twin's CPU fallback (M10.14h) — shares the same stages
    /// rather than rebuilding them: `apply` takes `&self`.
    reference: Option<std::sync::Arc<Vec<Box<dyn PipelineStage>>>>,
    /// Kept for [`Self::duplicate`], which rebuilds the coverage-only stages.
    scale: RenderScale,
    baseline: color::wb::Illuminant,
}

impl SceneOffsetsStage {
    /// The stage for `edits`, or `None` when no active mask carries a scene offset — then the tail is
    /// built exactly as before (`build.rs`).
    ///
    /// ⚠ `reference` builds the global look's stages; it is called only when a contributing mask
    /// [`reads_reference`] — the one place the reference's cost is decided.
    pub fn new(
        edits: &Edits,
        scale: RenderScale,
        region: Option<crate::Region>,
        geometry: &Geometry,
        brush: Option<&std::sync::Arc<crate::BrushCoverage>>,
        baseline: color::wb::Illuminant,
        reference: impl FnOnce() -> Vec<Box<dyn PipelineStage>>,
    ) -> Option<Self> {
        let gains = crate::global_wb_gains(&edits.white_balance, baseline);
        let masks: Vec<Contribution> = edits
            .masks
            .iter()
            .enumerate()
            // ⚠ The position in `edits.masks`, never a counter over contributing masks — the
            // brush rows are indexed by it (`build.rs`' rule for `MaskStage`).
            .filter(|(_, m)| !m.disabled && m.adjustments.has_scene_offsets())
            .map(|(i, m)| {
                let a = &m.adjustments;
                let (temp, tint) = crate::mask_wb_shift(a, baseline);
                let ln_r = if temp == 0.0 && tint == 0.0 {
                    [0.0; 3]
                } else {
                    let shifted =
                        crate::shifted_white_balance(&edits.white_balance, baseline, temp, tint);
                    let r = crate::white_balance_gains(&shifted, baseline);
                    [
                        (r[0] / gains[0]).ln(),
                        (r[1] / gains[1]).ln(),
                        (r[2] / gains[2]).ln(),
                    ]
                };
                let reads = reads_reference(m);
                Contribution {
                    // ⚠⚠ **A deposit only for a mask that reads the reference.** Its coverage is
                    // built HERE, on the reference photo, which is what the brush cache keys such a
                    // mask's gating brush on (`render_engine`'s `brush_cache::base_id`, through the
                    // same `reads_reference`). Any other mask's coverage is positional here, and its
                    // gating brush — if it has one — is built by its own `MaskStage`, on that
                    // stage's base: a map deposited from here would be filed under the wrong base.
                    coverage: MaskStage::new_in_region(
                        &shape_only(m),
                        scale,
                        region,
                        geometry,
                        brush.map_or(&[][..], |b| b.row(i)),
                        baseline,
                        brush
                            .filter(|_| reads)
                            .map(|b| (std::sync::Arc::clone(b), i)),
                    ),
                    ev: a.exposure,
                    contrast: a.contrast,
                    regions: [a.highlights, a.shadows, a.whites, a.blacks],
                    ln_r,
                    index: i,
                    slot: reads.then(Default::default),
                }
            })
            .collect();
        if masks.is_empty() {
            return None;
        }
        let reference = masks
            .iter()
            .any(|m| m.slot.is_some())
            .then(|| std::sync::Arc::new(reference()));
        Some(Self {
            global: Scene {
                gains,
                ev: edits.exposure,
                contrast: edits.contrast,
                regions: [edits.highlights, edits.shadows, edits.whites, edits.blacks],
            },
            masks,
            reference,
            scale,
            baseline,
        })
    }

    /// The global values: `[gain r, gain g, gain b, ev, contrast, highlights, shadows, whites,
    /// blacks]` — what a GPU twin uploads.
    pub fn global_values(&self) -> [f32; 9] {
        let g = &self.global;
        let r = g.regions;
        [
            g.gains[0], g.gains[1], g.gains[2], g.ev, g.contrast, r[0], r[1], r[2], r[3],
        ]
    }

    /// Each contributing mask: its coverage-only stage (shape, geometry, region and prebuilt
    /// maps through `mask_description`/`prebuilt_coverage`) and its offsets `[ev, contrast,
    /// highlights, shadows, whites, blacks, ln R r, ln R g, ln R b]`.
    pub fn contributions(&self) -> impl Iterator<Item = (&MaskStage, [f32; 9])> {
        self.masks.iter().map(|m| {
            let r = m.regions;
            (
                &m.coverage,
                [
                    m.ev, m.contrast, r[0], r[1], r[2], r[3], m.ln_r[0], m.ln_r[1], m.ln_r[2],
                ],
            )
        })
    }

    /// Whether this stage runs the reference photo — some contributing mask reads it.
    pub fn has_reference(&self) -> bool {
        self.reference.is_some()
    }

    /// The reference's stages by name, in order — what a GPU twin builds its own from (M10.14h); empty without a
    /// reference.
    pub fn reference_names(&self) -> Vec<&'static str> {
        self.reference
            .as_ref()
            .map_or_else(Vec::new, |r| r.iter().map(|s| s.name()).collect())
    }

    /// Each contributing mask's slot, in [`Self::contributions`]' order — `Some` for one that reads the reference.
    pub fn slots(&self) -> impl Iterator<Item = Option<&std::sync::Arc<CoverageSlot>>> {
        self.masks.iter().map(|m| m.slot.as_ref())
    }

    /// Mask `i`'s slot (`i` its position in `edits.masks`), for its look `MaskStage` — `Some` when
    /// that mask [`reads_reference`].
    pub fn slot_for(&self, i: usize) -> Option<std::sync::Arc<CoverageSlot>> {
        self.masks
            .iter()
            .find(|m| m.index == i)
            .and_then(|m| m.slot.clone())
    }

    /// A copy rebuilt from its parts — a GPU twin's own CPU reference (`GpuStage::cpu`), which a chain that fails
    /// on the GPU renders instead.
    ///
    /// ⚠⚠ M10.14h: it SHARES the reference's stages and the masks' slots — so a fallback fills the very slots the
    /// masks' look stages read, exactly as the original would. A copy with empty slots would leave those masks
    /// selecting on their own bases.
    pub fn duplicate(&self) -> Option<Self> {
        Some(Self {
            global: Scene { ..self.global },
            masks: self
                .masks
                .iter()
                .map(|m| {
                    let (masked, geometry, region) = m.coverage.description();
                    Contribution {
                        coverage: MaskStage::new_in_region(
                            masked,
                            self.scale,
                            region,
                            geometry,
                            m.coverage.prebuilt_coverage(),
                            self.baseline,
                            None,
                        ),
                        ev: m.ev,
                        contrast: m.contrast,
                        regions: m.regions,
                        ln_r: m.ln_r,
                        index: m.index,
                        slot: m.slot.clone(),
                    }
                })
                .collect(),
            reference: self.reference.clone(),
            scale: self.scale,
            baseline: self.baseline,
        })
    }
}

impl PipelineStage for SceneOffsetsStage {
    fn name(&self) -> &'static str {
        "scene_offsets"
    }

    fn scene_offsets(&self) -> Option<&SceneOffsetsStage> {
        Some(self)
    }

    fn apply(&self, image: ImageBuf) -> ImageBuf {
        let w = image.width();
        // ⚠⚠ The reference photo, for the masks that read it: the global look on a copy — the
        // globally edited photo, before any mask (M10.12b, the author's choice). Each such mask's
        // ONE coverage drives its offsets here and, through its slot, its look stage later.
        let reference = self.reference.as_ref().map(|stages| {
            crate::per_pixel::run_stages(stages.iter().map(|s| s.as_ref()), image.clone())
        });
        let planes: Vec<std::sync::Arc<Vec<f32>>> = self
            .masks
            .iter()
            .map(|m| {
                let from = match (&m.slot, &reference) {
                    (Some(_), Some(r)) => r,
                    _ => &image,
                };
                std::sync::Arc::new(m.coverage.coverage_plane(from))
            })
            .collect();
        for (m, plane) in self.masks.iter().zip(&planes) {
            if let Some(slot) = &m.slot {
                slot.put(std::sync::Arc::clone(plane));
            }
        }
        let g = &self.global;
        image.map_pixels_xy(|x, y, p| {
            let i = y * w + x;
            let (mut ev, mut con, mut reg, mut ln_r) = (g.ev, g.contrast, g.regions, [0.0f32; 3]);
            for (m, plane) in self.masks.iter().zip(&planes) {
                let c = plane[i];
                // ⚠ Skipped at zero, so where no mask covers every sum is the global value
                // bit for bit — never `g + 0.0 × offset`, which is the same here but says less.
                if c == 0.0 {
                    continue;
                }
                ev += c * m.ev;
                con += c * m.contrast;
                for (r, o) in reg.iter_mut().zip(&m.regions) {
                    *r += c * o;
                }
                for (l, o) in ln_r.iter_mut().zip(&m.ln_r) {
                    *l += c * o;
                }
            }
            // The four stages' order: white balance, exposure, contrast, tone regions.
            let gains = [
                g.gains[0] * ln_r[0].exp(),
                g.gains[1] * ln_r[1].exp(),
                g.gains[2] * ln_r[2].exp(),
            ];
            let mut p = [p[0] * gains[0], p[1] * gains[1], p[2] * gains[2]];
            if ev != 0.0 {
                let f = exposure::factor(ev);
                p = [p[0] * f, p[1] * f, p[2] * f];
            }
            // ⚠ Only when non-zero: the pivot arithmetic is not exactly the identity at slope 1,
            // and the four-stage tail inserts no contrast stage at 0.
            if con != 0.0 {
                p = contrast::apply(p, con);
            }
            if reg != [0.0; 4] {
                let a = [
                    reg[0] / 100.0,
                    reg[1] / 100.0,
                    reg[2] / 100.0,
                    reg[3] / 100.0,
                ];
                let gain = tone_regions::gain(color::luminance(p), a).max(0.0);
                p = [p[0] * gain, p[1] * gain, p[2] * gain];
            }
            p
        })
    }
}

/// Whether mask `m` selects on the **reference photo** — the globally edited photo, before any mask
/// (M10.12b): it is on, it reads the photograph, and it carries a scene offset. Its offsets and its
/// look sliders then share that one coverage, so nothing chases its own effect.
///
/// ⚠⚠ ONE definition for the stage that builds the reference and the brush cache that keys a gating
/// brush's map on it — two predicates would let the map be filed under one base and read under another.
pub fn reads_reference(m: &MaskedAdjustment) -> bool {
    !m.disabled && m.adjustments.has_scene_offsets() && m.reads_pixels()
}

/// `m`'s shape — base, refinements, whole-mask Invert — with no adjustments: what its coverage is.
pub(crate) fn shape_only(m: &MaskedAdjustment) -> MaskedAdjustment {
    MaskedAdjustment {
        mask: m.mask.clone(),
        refine: m.refine.clone(),
        invert: m.invert,
        adjustments: Default::default(),
        disabled: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edit_model::{LinearGradient, LocalAdjustments, Mask, RadialGradient};

    /// A small frame of varied colours across the whole range, headroom included.
    fn scene(w: usize, h: usize) -> ImageBuf {
        let mut img = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let t = x as f32 / (w - 1) as f32;
                let s = y as f32 / (h - 1) as f32;
                img.set_pixel(x, y, [t * 1.2, 0.1 + 0.6 * s, 0.05 + 0.5 * (1.0 - t) * s]);
            }
        }
        img
    }

    fn everything() -> Mask {
        Mask::Linear(LinearGradient {
            from: [-1.0, 0.0],
            to: [-0.5, 0.0],
            invert: false,
        })
    }

    fn masked(mask: Mask, a: LocalAdjustments) -> MaskedAdjustment {
        MaskedAdjustment {
            mask,
            adjustments: a,
            ..Default::default()
        }
    }

    fn stage(e: &Edits) -> SceneOffsetsStage {
        SceneOffsetsStage::new(
            e,
            RenderScale::FULL,
            None,
            &Geometry::default(),
            None,
            color::wb::Illuminant::default(),
            Vec::new,
        )
        .expect("a mask with a scene offset builds the stage")
    }

    fn ev(v: f32) -> LocalAdjustments {
        LocalAdjustments {
            exposure: v,
            ..Default::default()
        }
    }

    /// M10.14h: a copy — a GPU twin's CPU fallback — keeps the reference and FILLS THE ORIGINAL'S SLOTS, so a chain
    /// failing on the GPU leaves the reading masks' look stages the plane they blend by; and it reports the
    /// reference's stages by name, which the twin builds its own from.
    #[test]
    fn a_copy_keeps_the_reference_and_fills_the_same_slots() {
        let e = Edits {
            masks: vec![masked(
                Mask::Luminance(edit_model::LuminanceRange::default()),
                ev(1.0),
            )],
            ..Default::default()
        };
        let original = SceneOffsetsStage::new(
            &e,
            RenderScale::FULL,
            None,
            &Geometry::default(),
            None,
            color::wb::Illuminant::default(),
            || vec![Box::new(crate::ExposureStage::new(0.5)) as Box<dyn PipelineStage>],
        )
        .expect("the stage");
        assert!(original.has_reference());
        assert_eq!(original.reference_names(), vec!["exposure"]);
        let slot = original.slot_for(0).expect("the range reads the reference");
        let copy = original.duplicate().expect("a copy");
        assert!(copy.has_reference() && slot.get().is_none());
        let (w, h) = (12, 8);
        let out = copy.apply(scene(w, h));
        // ⚠ Read BEFORE the original runs, which would fill the slot itself.
        assert_eq!(
            slot.get().map(|p| p.len()),
            Some(w * h),
            "the copy filled the original's slot"
        );
        assert_eq!(
            out,
            original.apply(scene(w, h)),
            "the copy renders as the original"
        );
    }

    #[test]
    fn half_coverage_is_half_an_offset() {
        let ramp = Mask::Linear(LinearGradient {
            from: [0.0, 0.0],
            to: [1.0, 0.0],
            invert: false,
        });
        let m = masked(ramp, ev(1.0));
        let e = Edits {
            masks: vec![m.clone()],
            ..Default::default()
        };
        let img = scene(64, 16);
        let cov = MaskStage::new(&shape_only(&m), RenderScale::FULL, &Geometry::default())
            .coverage_plane(&img);
        let out = stage(&e).apply(img.clone());
        let x = 32;
        let c = cov[8 * 64 + x];
        assert!(c > 0.2 && c < 0.8, "fixture: partial coverage, {c}");
        let want = img.pixel(x, 8)[0] * exposure::factor(c);
        assert!(
            (out.pixel(x, 8)[0] - want).abs() < 1e-6,
            "{} vs {want}",
            out.pixel(x, 8)[0]
        );
    }

    #[test]
    fn overlapping_masks_add() {
        let e = Edits {
            masks: vec![masked(everything(), ev(0.5)), masked(everything(), ev(0.5))],
            ..Default::default()
        };
        let img = scene(32, 8);
        let want = crate::ExposureStage::new(1.0).apply(img.clone());
        assert_eq!(stage(&e).apply(img), want);
    }

    #[test]
    fn an_inverted_mask_offsets_outside_its_shape() {
        let disc = Mask::Radial(RadialGradient {
            center: [0.5, 0.5],
            radius: [0.25, 0.25],
            feather: 0.0,
            invert: false,
        });
        let mut m = masked(disc, ev(1.0));
        m.invert = true;
        let e = Edits {
            masks: vec![m],
            ..Default::default()
        };
        let img = scene(64, 64);
        let out = stage(&e).apply(img.clone());
        assert_eq!(out.pixel(32, 32), img.pixel(32, 32), "inside: no offset");
        let (a, b) = (out.pixel(2, 2)[0], img.pixel(2, 2)[0] * 2.0);
        assert!((a - b).abs() < 1e-6, "outside: +1 EV, {a} vs {b}");
    }

    #[test]
    fn a_waiting_ai_mask_offsets_nothing() {
        let mut m = masked(Mask::Ai(edit_model::AiMask::default()), ev(1.0));
        m.invert = true;
        let e = Edits {
            masks: vec![m],
            ..Default::default()
        };
        let img = scene(32, 8);
        assert_eq!(stage(&e).apply(img.clone()), img);
    }

    #[test]
    fn a_region_is_offset_as_that_part_of_the_whole() {
        let ramp = Mask::Linear(LinearGradient {
            from: [0.1, 0.0],
            to: [0.9, 1.0],
            invert: false,
        });
        let e = Edits {
            masks: vec![masked(ramp, ev(1.0))],
            ..Default::default()
        };
        let img = scene(64, 48);
        let whole = stage(&e).apply(img.clone());
        let (x0, y0, w, h) = (20, 10, 30, 25);
        let region = crate::Region {
            full_w: 64,
            full_h: 48,
            x0,
            y0,
        };
        let part = SceneOffsetsStage::new(
            &e,
            RenderScale::FULL,
            Some(region),
            &Geometry::default(),
            None,
            color::wb::Illuminant::default(),
            Vec::new,
        )
        .unwrap()
        .apply(img.crop(x0, y0, w, h));
        assert_eq!(part, whole.crop(x0, y0, w, h));
    }
}
