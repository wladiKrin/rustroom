//! A masked group of adjustments, entirely on the GPU: coverage, the clone, the sub-stages and
//! the blend, in one compute pass.
//!
//! ⚠⚠ **What this deletes is a round trip in the middle of every accelerated mask.** Before it,
//! `MaskStage::apply` ran its sub-stages through a `GpuChain` — which uploads, runs and
//! **downloads the whole adjusted frame as floats** — and then blended on the CPU, reading `base`
//! plus `adjusted` plus the coverage and writing a third image. Measured on `DSCF6274.RAF` at rung
//! 3200 (6.5 Mpx, load 5.17, *not* a quiet machine), the blend alone is **10.3–11.9 ms** for a
//! brush and the clone a further **2.1–3.8 ms**.
//!
//! ⚠⚠ **A mask also used to END a GPU run.** `every_tail_stage_is_replaceable_or_a_mask` recorded
//! that it was the only thing left in the tail that could.

use crate::{GpuContext, GpuImage, GpuStage};
use edit_model::{Geometry, MaskedAdjustment};
use pipeline::{PipelineStage, Region, RenderScale};
use std::sync::Arc;

/// The CPU cost of the clone, in ms per 6.8 Mpx.
///
/// ⚠ Measured at 6.5 Mpx as **2.1–3.8 ms** across four mask kinds (`mask_breakdown`, 2026-09-15,
/// load 5.17). The bottom of the band, which is the conservative direction for a `cpu_cost`: a
/// lower number makes a run less likely to chain, and the gate must fail toward the CPU.
const CLONE_COST: f32 = 2.0;

/// The CPU cost of the blend proper, in ms per 6.8 Mpx.
///
/// ⚠⚠ **This is the blend for a mask whose coverage is a LOOKUP.** Measured at 6.5 Mpx as
/// **10.3–11.9 ms** for a brush. It is emphatically *not* the number for a range mask, whose
/// `blend` column reads 68.5–90.7 ms — because there the coverage is computed inside the blend
/// loop and that column is measuring an Oklab round trip per pixel, not a lerp. A single constant
/// for both would be wrong by about 7x, which is why `cpu_cost` sums per component instead.
const BLEND_COST: f32 = 10.0;

/// The CPU cost of a **colour range**'s coverage, in ms per 6.8 Mpx.
///
/// ⚠⚠ Derived from the measurement by subtraction, and the subtraction is the point. A colour
/// range's `blend` column is coverage *and* blend together, because `Composite::at` runs
/// `prophoto_to_oklab` + `atan2` + `hypot` inside the blend loop — which is why `mask_breakdown`
/// reports `cov=0.00` for it. Subtracting the brush's measured blend (`BLEND_COST`) leaves the
/// coverage itself.
///
/// ⚠ **Two runs, and they disagree by a lot**: 68.5–90.7 ms at load 5.17, then 97.5–111.6 ms at
/// load 4.29, both at 6.5 Mpx. That spread is the machine, not the kernel. The **lower** band's
/// floor is taken and rounded down, which is the conservative direction — a smaller number makes
/// a run less likely to chain, and the gate must fail toward the CPU.
const COLOUR_COVERAGE_COST: f32 = 58.0;

/// The CPU cost of a **luminance range**'s coverage, in ms per 6.8 Mpx.
///
/// ⚠⚠ **MEASURED on its own, and a ratio argument would have been badly wrong.** The tempting
/// reasoning is that this is `color::luminance` plus an sRGB encode where a colour range is a full
/// Oklab round trip plus `atan2` and `hypot`, so it must be a small fraction. Measured in the same
/// run (`mask_breakdown`, 2026-09-15, load 4.29, 6.5 Mpx): a luminance range's blend column reads
/// **45.6–48.1 ms** against a colour range's **97.5–111.6** — roughly **0.45x**, not a tenth.
///
/// ⚠ That is the sixth time composing or scaling a measurement would have missed in this crate,
/// after M8.3g (~2x), M8.3f (8x in attribution), M8.3o (~18%) and M8.10 (~12%). `GpuStage::cpu_cost`
/// has no default precisely so adding a stage forces the measurement rather than inheriting one.
///
/// ⚠ The band's floor minus `BLEND_COST`, rounded down: 45.6 - 10 = 35.6 -> 35.
const LUMINANCE_COVERAGE_COST: f32 = 35.0;

/// How one component's coverage is produced in the shader.
///
/// ⚠ The *kind* decides the shader text, and the shader text is the program cache's key — so a
/// mask of each kind compiles once and is reused, which is M8.3d's rule rather than a new one.
enum Component {
    /// A coverage map somebody else already rasterised — `render_engine`'s cache.
    ///
    /// ⚠⚠ **The only way a GATING brush reaches this stage.** Such a brush reads the photograph to
    /// decide its coverage, and the photograph does not exist when the twin is built. On a cache
    /// hit the map is already in hand; on a miss the twin declines and the CPU `MaskStage`
    /// rasterises through `GpuBrushRaster` and deposits, so the very next frame hits.
    ///
    /// ⚠ Measured trade, from the spec's §5.5: uploading the plane is ~2 ms at rung 3200 (6.5 MB
    /// as `u8`) against 31.8–43.5 ms to rasterise a gating brush on the GPU. Keeping the CPU-side
    /// cache and moving the plane across the bus is the cheaper answer by an order of magnitude.
    Prebuilt {
        map: std::sync::Arc<pipeline::brush::CoverageMap>,
    },
    /// A painted brush, rasterised at **encode** time.
    ///
    /// ⚠⚠ **Rasterised there rather than at construction, and that is forced.** Coverage is
    /// defined over the FRAME, and without a region the frame's size is the buffer's — which does
    /// not exist until the pass is being encoded. Rasterising at construction would mean declining
    /// every whole-frame render, i.e. the entire fit rung.
    ///
    /// ⚠ It is not a new cost: `MaskStage::apply` rasterises per apply too, unless handed a
    /// prebuilt map. The cache that avoids it is a separate concern.
    Painted {
        brush: edit_model::Brush,
        geometry: Geometry,
    },
    /// A pure function of position. `a` carries `from.xy`/`to.xy` or `center.xy`/`radius.xy`.
    Analytic {
        expr: &'static str,
        a: [f32; 4],
        feather: f32,
        invert: bool,
    },
    /// A function of the **pixel**: a luminance or colour range.
    ///
    /// ⚠⚠ These are what make the coverage dispatch's position load-bearing — it must be recorded
    /// *before* the sub-stages, or the mask reads its own output.
    Range {
        expr: &'static str,
        a: [f32; 4],
        feather: f32,
        invert: bool,
        cost: f32,
    },
}

impl Component {
    /// The WGSL expression that yields this component's coverage, reading `comps[j]`.
    ///
    /// ⚠ The index is threaded in rather than the kernel deriving it, because the same kernel
    /// serves every component of its kind — which is what keeps the shader text one program per
    /// composite *shape* rather than one per composite.
    fn expr(&self, j: usize) -> String {
        match self {
            Component::Painted { .. } | Component::Prebuilt { .. } => {
                format!("painted_at(fx, fy, {j}u)")
            }
            // ⚠ The stored expression carries a `{j}` placeholder rather than being assembled from
            // a function name and an argument list here: the four kernels genuinely differ in
            // their arguments — a range reads the hoisted facts where an analytic reads `u`/`v` —
            // so a shared assembler would need a second match to decide which, in a second place.
            Component::Analytic { expr, .. } | Component::Range { expr, .. } => {
                expr.replace("{j}", &j.to_string())
            }
        }
    }

    /// The scene facts this component's kernel reads.
    ///
    /// ⚠⚠ **Derived from the component rather than recorded beside it.** A `needs_oklab` flag set
    /// where the component is built is a second fact that can disagree with the expression, and
    /// the failure is a shader referencing `hue` that was never hoisted — a compile error at best,
    /// and at worst a stale value if some other component happened to hoist it.
    fn facts(&self) -> crate::mask_wgsl::Facts {
        let mut f = crate::mask_wgsl::Facts::default();
        if let Component::Range { expr, .. } = self {
            if expr.starts_with("cov_luminance") {
                f.luma = true;
            } else {
                f.oklab = true;
            }
        }
        f
    }

    /// The CPU cost this component's coverage would have had, in ms per 6.8 Mpx.
    ///
    /// ⚠ **Zero for a painted or analytic component, and that is the measurement rather than a
    /// placeholder.** A painted component is a table lookup and an analytic one a handful of
    /// arithmetic ops; `mask_breakdown` reads `cov=0.00` for both, because what little they cost
    /// is inside the blend's own per-pixel work and is already in `BLEND_COST`.
    ///
    /// ⚠⚠ A **range** component is emphatically not zero — it is an Oklab round trip or an sRGB
    /// encode per pixel, and it is the whole reason a colour range's `blend` column reads 68.5–90.7
    /// ms against a brush's 10.3–11.9. Each carries its own measured number.
    fn coverage_cost(&self) -> f32 {
        match self {
            Component::Painted { .. } | Component::Prebuilt { .. } | Component::Analytic { .. } => {
                0.0
            }
            Component::Range { cost, .. } => *cost,
        }
    }
}

/// `MaskOp` as the shader's own fold.
///
/// ⚠⚠ **An exhaustive match, deliberately: a fourth `MaskOp` must be a compile error here.** A
/// catch-all mapping an unknown op to `Add` would composite it plausibly and wrongly — the trap
/// `CoverageSource`'s own `other =>` arm already sprang once, producing a mask covering nothing.
fn fold_of(op: edit_model::MaskOp) -> crate::mask_wgsl::Fold {
    use crate::mask_wgsl::Fold;
    match op {
        edit_model::MaskOp::Add => Fold::Add,
        edit_model::MaskOp::Subtract => Fold::Subtract,
        edit_model::MaskOp::Intersect => Fold::Intersect,
    }
}

/// A masked group of adjustments, on the GPU.
pub struct GpuMaskStage {
    /// The mask's own sub-stages, already accelerated by the caller.
    ///
    /// ⚠ Built by `accelerate`, not here: it owns run detection, and reproducing that inside this
    /// constructor would be a second copy of the rule that decides what a chain contains.
    subs: Vec<Box<dyn GpuStage>>,
    /// The CPU twin, which is also this stage's equivalence reference.
    cpu: pipeline::MaskStage,
    /// The mask's base component, and the refinements folded onto it **in list order**.
    ///
    /// ⚠ Two fields rather than one `Vec` with an `Option<Fold>` per entry, so "a refinement in
    /// position zero" and "a base carrying an op" are both unrepresentable. `Composite`'s shape.
    base: Component,
    refine: Vec<(crate::mask_wgsl::Fold, Component)>,
    region: Option<Region>,
    cost: f32,
}

impl GpuMaskStage {
    /// Build a GPU twin for a mask, or decline.
    ///
    /// ⚠⚠ **Declining is always safe and is the gate's own rule** — fail toward the CPU, where a
    /// wrong answer costs milliseconds and never pixels. `accelerate` leaves the `MaskStage` in
    /// place when this returns `None`.
    ///
    /// ⚠ It declines a **gating** brush in any position, base or refinement: such a brush needs
    /// base pixels, which only exist at encode time, and without a map already in hand there is
    /// nothing to upload. One declining component declines the whole mask — a composite is a
    /// single coverage plane, so there is no half of it to accelerate.
    ///
    /// ⚠⚠ `prebuilt` is indexed **per component, base first**, which is `BrushCoverage`'s own
    /// `[mask][component]` shape. A shorter slice is not an error — it is what every caller that
    /// has no cache at all passes — so a missing entry reads as "nothing prebuilt" rather than
    /// panicking on the render worker.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        masked: &MaskedAdjustment,
        geometry: &Geometry,
        region: Option<Region>,
        scale: RenderScale,
        baseline: color::wb::Illuminant,
        prebuilt: &[Option<std::sync::Arc<pipeline::brush::CoverageMap>>],
        subs: Vec<Box<dyn GpuStage>>,
        _ctx: &Arc<GpuContext>,
    ) -> Option<Self> {
        let at = |k: usize| prebuilt.get(k).cloned().flatten();
        let base = Self::component(&masked.mask, geometry, at(0))?;
        let refine = masked
            .refine
            .iter()
            .enumerate()
            .map(|(k, r)| Self::component(&r.mask, geometry, at(k + 1)).map(|c| (fold_of(r.op), c)))
            .collect::<Option<Vec<_>>>()?;

        // ⚠ Every component's coverage is charged, because the CPU evaluates every one of them per
        // pixel — `Composite::at` folds the whole chain and has no early-out. The blend and the
        // clone are charged once, because there is one of each however long the chain is.
        let cost = CLONE_COST
            + BLEND_COST
            + base.coverage_cost()
            + refine.iter().map(|(_, c)| c.coverage_cost()).sum::<f32>()
            + subs.iter().map(|s| s.cpu_cost()).sum::<f32>();

        Some(Self {
            subs,
            cpu: pipeline::MaskStage::new_in_region(
                masked,
                scale,
                region,
                geometry,
                &[],
                baseline,
                None,
            ),
            base,
            refine,
            region,
            cost,
        })
    }

    /// One mask kind as a shader component, or `None` where no GPU path exists yet.
    ///
    /// ⚠⚠ **Exhaustive on purpose — a catch-all here is the trap `CoverageSource` already
    /// records.** That match once read `other => Analytic(..)`, so a new variant whose
    /// `analytic_coverage` returned `None` fell through and produced a mask covering **nothing**,
    /// silently. Naming every variant makes a new one a compile error in exactly one place.
    fn component(
        mask: &edit_model::Mask,
        geometry: &Geometry,
        prebuilt: Option<std::sync::Arc<pipeline::brush::CoverageMap>>,
    ) -> Option<Component> {
        match mask {
            edit_model::Mask::Brush(brush) => {
                // ⚠⚠ **A prebuilt map wins whatever the brush is.** It was rasterised by the CPU
                // stage at the same frame size and cached, so uploading it is both cheaper and the
                // only route a gating brush has.
                if let Some(map) = prebuilt {
                    return Some(Component::Prebuilt { map });
                }
                // ⚠⚠ Without one, a gating brush cannot be built here at all: it reads the
                // photograph to decide its coverage, and the photograph does not exist until the
                // pass is being encoded. `Brush::needs_base` is the same question `BrushCache`
                // asks before refusing to serve one, and for the same reason — a positionally
                // rasterised gating brush is a silent divergence, not an error.
                //
                // ⚠ Declining leaves the CPU `MaskStage`, which rasterises through
                // `GpuBrushRaster` and **deposits**, so the next frame has a map and hits.
                if brush.needs_base() {
                    return None;
                }
                Some(Component::Painted {
                    brush: brush.clone(),
                    geometry: *geometry,
                })
            }
            edit_model::Mask::Linear(g) => Some(Component::Analytic {
                expr: "cov_linear(u, v, params.aspect, {j}u)",
                a: [g.from[0], g.from[1], g.to[0], g.to[1]],
                feather: 0.0,
                invert: g.invert,
            }),
            edit_model::Mask::Radial(g) => Some(Component::Analytic {
                // ⚠ No aspect argument, matching the CPU exactly. See `ANALYTIC`'s doc.
                expr: "cov_radial(u, v, {j}u)",
                a: [g.center[0], g.center[1], g.radius[0], g.radius[1]],
                feather: g.feather,
                invert: g.invert,
            }),
            edit_model::Mask::Luminance(r) => Some(Component::Range {
                expr: "cov_luminance(lum, {j}u)",
                a: [r.low, r.high, 0.0, 0.0],
                feather: r.feather,
                invert: r.invert,
                cost: LUMINANCE_COVERAGE_COST,
            }),
            edit_model::Mask::Colour(r) => Some(Component::Range {
                expr: "cov_colour(hue, chroma, {j}u)",
                // ⚠ `a2` is `min_chroma`, the gate's own threshold — a per-mask value, not a
                // constant, because a saturated sign and a pastel wall need different answers.
                a: [r.low, r.high, r.min_chroma, 0.0],
                feather: r.feather,
                invert: r.invert,
                cost: COLOUR_COVERAGE_COST,
            }),
        }
    }

    fn shader(&self) -> String {
        format!(
            "{}{}{}{}{}{}{}{}{}{}{}",
            // ⚠ The three shared preludes, concatenated the way every other consumer does it —
            // `linear_to_srgb` for a luminance range, `prophoto_luminance` for its luma, and the
            // Oklab pair for a colour range. Substituted, never transcribed.
            crate::srgb_wgsl::SRGB_WGSL,
            crate::luma_wgsl::luma_wgsl(),
            crate::oklab_wgsl::OKLAB_WGSL,
            crate::mask_wgsl::constants(),
            crate::mask_wgsl::PRELUDE,
            crate::mask_wgsl::PAINTED,
            crate::mask_wgsl::ANALYTIC,
            crate::mask_wgsl::RANGES,
            crate::mask_wgsl::SEED,
            crate::mask_wgsl::coverage_entry(
                &self.base.expr(0),
                &self.refine_exprs(),
                self.facts()
            ),
            crate::mask_wgsl::BLEND,
        )
    }

    /// The base and every refinement, in chain order.
    ///
    /// ⚠ One iterator both the shader builder and the component buffer read, so the index a
    /// component is *compiled* against and the slot it is *uploaded* to cannot disagree. Written
    /// twice they would drift silently: the mask would simply fold the wrong shapes.
    fn components(&self) -> impl Iterator<Item = &Component> {
        std::iter::once(&self.base).chain(self.refine.iter().map(|(_, c)| c))
    }

    fn refine_exprs(&self) -> Vec<(crate::mask_wgsl::Fold, String)> {
        self.refine
            .iter()
            .enumerate()
            .map(|(k, (op, c))| (*op, c.expr(k + 1)))
            .collect()
    }

    /// The union of every component's scene facts.
    ///
    /// ⚠⚠ **The union, which is what makes three colour components cost ONE Oklab round trip.**
    /// `BaseFacts` reaches the same result on the CPU by memoising per pixel; here the chain is
    /// known when the shader is built, so the conversion is simply emitted once above it — and
    /// emitted not at all for a mask that needs no pixel, which the CPU's lazy memo also manages
    /// but only by checking an `Option` per pixel per component.
    fn facts(&self) -> crate::mask_wgsl::Facts {
        self.components()
            .map(Component::facts)
            .fold(crate::mask_wgsl::Facts::default(), |a, b| {
                crate::mask_wgsl::Facts {
                    luma: a.luma || b.luma,
                    oklab: a.oklab || b.oklab,
                }
            })
    }

    /// The frame-wide uniform: what every component shares.
    fn params(&self, img: &GpuImage, row_groups: u32) -> [u8; 48] {
        let (x0, y0, fw, fh) = self.frame(img);
        let mut p = [0u8; 48];
        let mut put = |off: usize, v: u32| p[off..off + 4].copy_from_slice(&v.to_le_bytes());
        put(0, img.pixels());
        put(4, row_groups);
        put(8, img.width() as u32);
        put(16, x0);
        put(20, y0);
        put(24, fw);
        put(28, fh);
        // ⚠ The FRAME's aspect, `fw / fh`, exactly as `MaskStage::apply` derives it. A region's
        // own aspect is a different number and would skew a diagonal gradient's iso-lines.
        p[32..36].copy_from_slice(&(fw as f32 / fh as f32).to_le_bytes());
        p
    }

    /// Where this buffer sits in the frame, and how big the frame is.
    fn frame(&self, img: &GpuImage) -> (u32, u32, u32, u32) {
        match self.region {
            Some(r) => (r.x0 as u32, r.y0 as u32, r.full_w as u32, r.full_h as u32),
            None => (0, 0, img.width() as u32, img.height() as u32),
        }
    }

    /// One 64-byte `Comp` per component, in chain order.
    ///
    /// ⚠ `maps` is indexed by the same chain order, so a painted component's box and its word
    /// offset come from the very entry `pack` concatenated — the two are built from one walk.
    fn comps(&self, maps: &[Option<std::sync::Arc<pipeline::brush::CoverageMap>>]) -> Vec<u8> {
        const STRIDE: usize = 64;
        let mut out = vec![0u8; STRIDE * (1 + self.refine.len())];
        let mut off_words = 0u32;
        for (j, c) in self.components().enumerate() {
            let b = j * STRIDE;
            let put = |p: &mut Vec<u8>, at: usize, v: u32| {
                p[b + at..b + at + 4].copy_from_slice(&v.to_le_bytes())
            };
            let putf = |p: &mut Vec<u8>, at: usize, v: f32| {
                p[b + at..b + at + 4].copy_from_slice(&v.to_le_bytes())
            };
            match c {
                Component::Painted { .. } | Component::Prebuilt { .. } => {
                    if let Some(m) = maps.get(j).and_then(|m| m.as_ref()) {
                        put(&mut out, 28, m.outside as u32);
                        put(&mut out, 32, m.x0 as u32);
                        put(&mut out, 36, m.y0 as u32);
                        put(&mut out, 40, m.w as u32);
                        put(&mut out, 44, m.h as u32);
                        put(&mut out, 48, off_words);
                        off_words += words_for(m);
                    }
                }
                Component::Analytic {
                    a, feather, invert, ..
                }
                | Component::Range {
                    a, feather, invert, ..
                } => {
                    for (k, v) in a.iter().enumerate() {
                        putf(&mut out, k * 4, *v);
                    }
                    putf(&mut out, 16, *feather);
                    put(&mut out, 24, u32::from(*invert));
                }
            }
        }
        out
    }

    /// Rasterise a painted component over the frame, or `None` for the other kinds.
    ///
    /// ⚠ `frame` is the FRAME's dimensions — the region's `full_w`/`full_h` when there is one, and
    /// the buffer's own otherwise. A brush rasterised at a region's size slides as you pan, which
    /// is the note `MaskStage::apply` already carries.
    fn rasterize(
        &self,
        frame: (usize, usize),
    ) -> Vec<Option<std::sync::Arc<pipeline::brush::CoverageMap>>> {
        self.components()
            .map(|c| match c {
                // ⚠ Already rasterised, and at the same `fw`/`fh` by construction — the coverage
                // cache's key carries them, which is why it must never lose them.
                Component::Prebuilt { map } => Some(map.clone()),
                Component::Painted { brush, geometry } => Some(std::sync::Arc::new(
                    pipeline::brush::rasterize(brush, geometry, frame.0, frame.1),
                )),
                Component::Analytic { .. } | Component::Range { .. } => None,
            })
            .collect()
    }

    /// Every painted component's coverage, concatenated into one buffer of `u32` words.
    ///
    /// ⚠ WGSL has no `u8`. ⚠ `.max(1)` so the buffer is never zero-sized, which wgpu refuses — the
    /// guard `Dispatch` and `GpuImage::scratch` already carry. A mask with no painted component at
    /// all uploads one zero word that the shader never reads, because the binding still has to
    /// exist.
    ///
    /// ⚠⚠ **It walks the components in the same order `comps` does**, which is what makes the
    /// offsets it hands out and the offsets uploaded beside the boxes the same numbers.
    fn pack(&self, maps: &[Option<std::sync::Arc<pipeline::brush::CoverageMap>>]) -> Vec<u32> {
        let mut packed: Vec<u32> = Vec::new();
        for m in maps.iter().flatten() {
            let start = packed.len();
            packed.resize(start + words_for(m) as usize, 0);
            for (k, v) in m.data.iter().enumerate() {
                packed[start + (k >> 2)] |= (*v as u32) << ((k & 3) * 8);
            }
        }
        if packed.is_empty() {
            packed.push(0);
        }
        packed
    }
}

/// How many `u32` words one coverage map occupies.
///
/// ⚠ **One definition, read by `pack` when it lays a map down and by `comps` when it records the
/// next component's offset.** Two derivations of this is how a composite of brushes comes to read
/// its neighbour's coverage — a plausible mask, not an error.
fn words_for(m: &pipeline::brush::CoverageMap) -> u32 {
    (m.w * m.h).div_ceil(4) as u32
}

impl GpuStage for GpuMaskStage {
    fn name(&self) -> &'static str {
        "mask"
    }

    /// ⚠⚠ **Summed from the parts rather than a constant, because a mask's CPU cost is
    /// mask-kind dependent in a way no other stage's is** — 68.5–90.7 ms for a colour range
    /// against 10.3–11.9 for a brush, both at 6.5 Mpx. A single number would be wrong by ~7x one
    /// way or the other.
    ///
    /// ⚠ The sub-stages' own costs are included because this stage subsumes them: it is the whole
    /// mask, so what the gate is comparing against the transfer is the whole mask's CPU work.
    ///
    /// ⚠ The rasterisation is deliberately **not** counted. It happens at construction on the CPU
    /// either way, so it is not work this stage saves.
    fn cpu_cost(&self) -> f32 {
        self.cost
    }

    /// ⚠⚠ **Four for this stage plus the most any sub-stage wants, and the sum is FORCED.**
    /// `GpuChain` lends the *same* planes to every stage — its own doc says "nothing written here
    /// survives the call" — so a `base` copy parked in shared planes would be clobbered by the
    /// first sub-stage that wants scratch (sharpening 4, local contrast 5, NR 4). Asking for the
    /// total and lending the tail onward keeps that contract intact while giving the sub-stages
    /// exactly the shape they already expect.
    ///
    /// ⚠ The **max** over sub-stages, not the sum — planes are transient *within* a stage, so two
    /// sub-stages wanting four each can share the same four. That is `GpuChain`'s own rule.
    fn scratch_planes(&self) -> usize {
        4 + self
            .subs
            .iter()
            .map(|s| s.scratch_planes())
            .max()
            .unwrap_or(0)
    }

    fn encode(
        &self,
        ctx: &GpuContext,
        pass: &mut wgpu::ComputePass,
        img: &GpuImage,
        scratch: &[wgpu::Buffer],
    ) {
        let device = &ctx.device;
        let (row_groups, col_groups) = crate::dispatch::Dispatch::grid(img.pixels());

        let entry = |binding: u32, ty: wgpu::BufferBindingType| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let rw = wgpu::BufferBindingType::Storage { read_only: false };
        let ro = wgpu::BufferBindingType::Storage { read_only: true };

        let (layout, pipelines) = ctx.cache.program(
            device,
            "mask",
            &self.shader(),
            &[
                entry(0, rw),
                entry(1, wgpu::BufferBindingType::Uniform),
                entry(2, rw),
                entry(3, rw),
                entry(4, rw),
                entry(5, rw),
                entry(6, ro),
                entry(7, ro),
            ],
            &["seed", "coverage", "blend"],
        );

        // ⚠⚠ The FRAME, which without a region is the buffer itself. Coverage is defined over the
        // frame, so rasterising at a region's size would slide the mask as you pan.
        let frame = match self.region {
            Some(r) => (r.full_w, r.full_h),
            None => (img.width(), img.height()),
        };
        // ⚠ Rasterised, packed and described from ONE walk of the chain, so a component's box,
        // its word offset and the bytes at that offset cannot disagree about which brush they be-
        // long to.
        let maps = self.rasterize(frame);
        let words = self.pack(&maps);
        let comps = self.comps(&maps);

        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mask-params"),
            size: 48,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        ctx.queue
            .write_buffer(&uniform, 0, &self.params(img, row_groups));

        let packed = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mask-coverage"),
            size: (words.len() * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        ctx.queue
            .write_buffer(&packed, 0, bytemuck::cast_slice(&words));

        let comp_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mask-components"),
            size: comps.len() as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        ctx.queue.write_buffer(&comp_buf, 0, &comps);

        let bound = [
            img.buffer.as_entire_binding(),
            uniform.as_entire_binding(),
            scratch[0].as_entire_binding(),
            scratch[1].as_entire_binding(),
            scratch[2].as_entire_binding(),
            scratch[3].as_entire_binding(),
            packed.as_entire_binding(),
            comp_buf.as_entire_binding(),
        ];
        let entries: Vec<wgpu::BindGroupEntry<'_>> = bound
            .into_iter()
            .enumerate()
            .map(|(i, resource)| wgpu::BindGroupEntry {
                binding: i as u32,
                resource,
            })
            .collect();
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("mask-bind"),
            layout: &layout,
            entries: &entries,
        });

        // 1. seed — the clone, on the GPU.
        // 2. coverage — reads `img`, which is still the BASE at this point.
        //
        // ⚠⚠ **The order is the data dependency, and it IS observable now.** This comment used to
        // say the opposite and to ask for the sabotage to be re-run once the range kinds landed;
        // that has been done. Moving `coverage` after the sub-stages fails four tests — both range
        // masks, the composite of three colour ranges, and the mixed luminance/colour composite —
        // because reading `img` after the sub-stages keys the mask on its own output, and lifting
        // the shadows moves them out of the band that selected them. That is exactly what
        // `Composite::at`'s `base_px` parameter exists to prevent on the CPU.
        //
        // ⚠ It was genuinely inert when first written, and that was a property of the *fixtures*
        // rather than a hole in them: `painted_at` reads the uploaded map and never touches `img`,
        // so for a painted or analytic mask the position provably cannot matter. Such a mask emits
        // no `img` read at all today — see `a_chain_that_needs_no_pixel_never_reads_the_image`.
        for compute in pipelines.iter().take(2) {
            pass.set_pipeline(compute);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(row_groups, col_groups, 1);
        }

        // 3. the sub-stages, turning `img` from the base into the adjusted image in place.
        //
        // ⚠⚠ **`&scratch[4..]` is the single most dangerous expression in this stage.** Handing
        // over the whole slice lets a sub-stage write `base_r/g/b` and `cov`, and the result is a
        // plausible photograph blended against the wrong thing rather than an error.
        for sub in &self.subs {
            sub.encode(ctx, pass, img, &scratch[4..]);
        }

        // 4. blend.
        //
        // ⚠ The bind group is re-set because every sub-stage set its own — pipeline and bind-group
        // state is per dispatch, not per pass.
        pass.set_pipeline(&pipelines[2]);
        pass.set_bind_group(0, &bind, &[]);
        pass.dispatch_workgroups(row_groups, col_groups, 1);
    }

    fn cpu(&self) -> &dyn PipelineStage {
        &self.cpu
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_image::ImageBuf;
    use edit_model::{Brush, BrushStroke, LocalAdjustments, Mask};

    fn ctx() -> Option<Arc<GpuContext>> {
        match crate::shared() {
            Some(c) => Some(c.clone()),
            None => {
                eprintln!("SKIPPED: no GPU adapter on this machine");
                None
            }
        }
    }

    /// A horizontal stroke across the middle of the frame, **not** auto-masked.
    ///
    /// ⚠ `auto_mask: false` is load-bearing for this task rather than incidental: a gating brush
    /// decides its coverage from the photograph, which does not exist until the pass is being
    /// encoded, so `GpuMaskStage::new` declines one. `a_gating_brush_is_declined` pins that.
    fn painted(invert: bool) -> MaskedAdjustment {
        MaskedAdjustment {
            mask: Mask::Brush(Brush {
                strokes: vec![BrushStroke {
                    points: (0..24).map(|i| [0.15 + i as f32 * 0.03, 0.5]).collect(),
                    radius: 0.12,
                    hardness: 40.0,
                    flow: 100.0,
                    auto_mask: false,
                    tolerance: 25.0,
                    ..Default::default()
                }],
                invert,
            }),
            // ⚠ A strong, unambiguous adjustment. A subtle one would let an equivalence test pass
            // against a stage that blended at the wrong coverage.
            adjustments: LocalAdjustments {
                exposure: 1.5,
                ..Default::default()
            },
            refine: Vec::new(),
        }
    }

    /// ⚠⚠ **GENUINELY COLOURED, and that is not a detail — the first version was not.** It stepped
    /// consecutive values into R, G and B, so every pixel came out near-neutral with an Oklab
    /// chroma of almost zero. A painted, analytic or luminance mask cannot tell the difference,
    /// because none of them looks at hue — but a colour range's chroma gate correctly rejected the
    /// entire frame, and `a_colour_range_mask_renders_the_same_picture` failed its own fixture
    /// check with "moved 0". Same correction `saturation`'s fixture needed, for the same reason.
    ///
    /// ⚠ The three channels use **coprime strides** so hue sweeps the whole wheel rather than
    /// tracing one line through colour space, and the range spans `-0.1..1.3` so the fixture also
    /// carries negatives and above-white samples — a `[0, 1]` fixture cannot see a clamp.
    fn scene(w: usize, h: usize) -> ImageBuf {
        let mut v = Vec::with_capacity(w * h * 3);
        for i in 0..w * h {
            for (k, stride) in [37usize, 61, 89].iter().enumerate() {
                let _ = k;
                v.push(((i * stride) % 313) as f32 / 313.0 * 1.4 - 0.1);
            }
        }
        ImageBuf::from_vec(w, h, v)
    }

    /// ⚠ The sub-stage list must match what the CPU twin builds from the same adjustments, or the
    /// comparison is between two different masks. `accelerate` maintains that invariant in
    /// production; here it is one exposure either side — written once so the whole-image and
    /// region fixtures cannot drift apart on it.
    fn subs_for(ctx: &Arc<GpuContext>, m: &MaskedAdjustment) -> Vec<Box<dyn GpuStage>> {
        vec![Box::new(crate::GpuExposureStage::new(
            m.adjustments.exposure,
            ctx.clone(),
        ))]
    }

    fn gpu_stage(ctx: &Arc<GpuContext>, m: &MaskedAdjustment) -> Option<GpuMaskStage> {
        GpuMaskStage::new(
            m,
            &Geometry::default(),
            None,
            RenderScale::FULL,
            color::wb::Illuminant::default(),
            &[],
            subs_for(ctx, m),
            ctx,
        )
    }

    /// ⚠⚠ The equivalence test, and its **fixture check is half of it**. Without the second
    /// assertion this passes against a stage that copies its input straight through — which is
    /// precisely what a mask whose coverage came back zero everywhere would do.
    #[test]
    fn a_painted_mask_renders_the_same_picture() {
        let Some(ctx) = ctx() else { return };
        let (w, h) = (96, 64);
        let src = scene(w, h);
        let m = painted(false);

        let want = pipeline::MaskStage::new(&m, RenderScale::FULL, &Geometry::default())
            .apply(src.clone());
        let stage = gpu_stage(&ctx, &m).expect("a plain brush must build");
        let got = crate::GpuChain::new(ctx.clone(), vec![Box::new(stage)]).apply(src.clone());

        let moved = crate::compare(&src, &want);
        assert!(
            moved.worst_levels > 8.0,
            "fixture check: the CPU mask must actually move the picture, moved {} levels",
            moved.worst_levels
        );

        let d = crate::compare(&want, &got);
        assert!(
            d.worst_levels < 1.0,
            "a painted mask diverged by {} levels over {:.3}% of samples",
            d.worst_levels,
            d.differing_percent()
        );
    }

    /// ⚠⚠ **An inverted brush covers the whole frame EXCEPT what was painted**, which is the case
    /// the `outside` value exists for — and its absence was a real bug found in use, confining the
    /// inversion to a rectangle around the strokes. A non-inverted fixture cannot see it, because
    /// there `outside` is 0 and reading the wrong thing outside the box changes nothing.
    #[test]
    fn an_inverted_painted_mask_covers_everything_outside_the_box() {
        let Some(ctx) = ctx() else { return };
        let (w, h) = (96, 64);
        let src = scene(w, h);
        let m = painted(true);

        let want = pipeline::MaskStage::new(&m, RenderScale::FULL, &Geometry::default())
            .apply(src.clone());
        let stage = gpu_stage(&ctx, &m).expect("a plain brush must build");
        let got = crate::GpuChain::new(ctx.clone(), vec![Box::new(stage)]).apply(src.clone());

        // ⚠ The corner is far outside any stroke, so it is covered only via `outside`.
        let corner = crate::compare(&src, &want).worst_levels;
        assert!(
            corner > 8.0,
            "fixture check: the inverted mask must move the frame's corners"
        );

        let d = crate::compare(&want, &got);
        assert!(
            d.worst_levels < 1.0,
            "an inverted painted mask diverged by {} levels",
            d.worst_levels
        );
    }

    /// ⚠⚠ **THE FIXTURE THAT CAN SEE THE SCRATCH OFFSET, and nothing else in this module can.**
    ///
    /// Sabotaging `&scratch[4..]` to `scratch` — handing sub-stages the planes holding `base` and
    /// the coverage — left every other test in this module **green**, because their only
    /// sub-stage is a per-pixel exposure that asks for zero scratch planes and therefore never
    /// writes any. A fixture must contain the thing a wrong implementation is attracted to; that
    /// correction is the `spot_source` lesson and the `a_dab_is_round_on_a_wide_frame` lesson
    /// arriving in a third place.
    ///
    /// ⚠ Sharpening asks for **four** planes (luma, blurred, difference, mask), so under the
    /// sabotage it writes straight over `base_r/g/b` and `cov` and the blend mixes the adjusted
    /// image against garbage.
    #[test]
    fn a_mask_whose_sub_stage_uses_scratch_renders_the_same_picture() {
        let Some(ctx) = ctx() else { return };
        let (w, h) = (96, 64);
        let src = scene(w, h);
        let mut m = painted(false);
        m.adjustments.exposure = 0.0;
        m.adjustments.sharpening = edit_model::Sharpening {
            amount: 80.0,
            radius: 2.0,
            detail: 50.0,
            masking: 0.0,
        };

        let want = pipeline::MaskStage::new(&m, RenderScale::FULL, &Geometry::default())
            .apply(src.clone());

        let subs: Vec<Box<dyn GpuStage>> = vec![Box::new(crate::GpuSharpenStage::new(
            &m.adjustments.sharpening,
            RenderScale::FULL,
            &ctx,
        ))];
        let stage = GpuMaskStage::new(
            &m,
            &Geometry::default(),
            None,
            RenderScale::FULL,
            color::wb::Illuminant::default(),
            &[],
            subs,
            &ctx,
        )
        .expect("builds");
        assert_eq!(
            stage.scratch_planes(),
            8,
            "fixture check: the sub-stage must actually ask for scratch, or this test is blind"
        );
        let got = crate::GpuChain::new(ctx.clone(), vec![Box::new(stage)]).apply(src.clone());

        let moved = crate::compare(&src, &want);
        assert!(
            moved.worst_levels > 4.0,
            "fixture check: the CPU mask must move the picture, moved {}",
            moved.worst_levels
        );
        let d = crate::compare(&want, &got);
        assert!(
            d.worst_levels < 1.0,
            "a mask with a scratch-using sub-stage diverged by {} levels over {:.3}%",
            d.worst_levels,
            d.differing_percent()
        );
    }

    fn with_mask(m: Mask) -> MaskedAdjustment {
        MaskedAdjustment {
            mask: m,
            adjustments: LocalAdjustments {
                exposure: 1.5,
                ..Default::default()
            },
            refine: Vec::new(),
        }
    }

    /// Render one mask both ways at `frame`, and return `(cpu, gpu, source)`.
    fn both(
        ctx: &Arc<GpuContext>,
        m: &MaskedAdjustment,
        w: usize,
        h: usize,
    ) -> (ImageBuf, ImageBuf, ImageBuf) {
        let src = scene(w, h);
        let want =
            pipeline::MaskStage::new(m, RenderScale::FULL, &Geometry::default()).apply(src.clone());
        let stage = gpu_stage(ctx, m).expect("builds");
        let got = crate::GpuChain::new(ctx.clone(), vec![Box::new(stage)]).apply(src.clone());
        (want, got, src)
    }

    fn assert_agrees(label: &str, want: &ImageBuf, got: &ImageBuf, src: &ImageBuf) {
        let moved = crate::compare(src, want);
        assert!(
            moved.worst_levels > 8.0,
            "fixture check ({label}): the CPU mask must move the picture, moved {}",
            moved.worst_levels
        );
        let d = crate::compare(want, got);
        assert!(
            d.worst_levels < 1.0,
            "{label} diverged by {} levels over {:.3}% of samples",
            d.worst_levels,
            d.differing_percent()
        );
    }

    /// ⚠ A **diagonal** gradient, deliberately. An axis-aligned one has a zero component in its
    /// direction vector, and the aspect correction is what stops the iso-lines skewing — so an
    /// axis-aligned fixture is exactly the blind spot `dragging_an_edge_resizes_about_a_fixed_centre`
    /// already recorded for this codebase: a zero component is a blind spot a real gesture never has.
    #[test]
    fn a_linear_gradient_mask_renders_the_same_picture() {
        let Some(ctx) = ctx() else { return };
        let m = with_mask(Mask::Linear(edit_model::LinearGradient {
            from: [0.15, 0.2],
            to: [0.8, 0.85],
            invert: false,
        }));
        let (want, got, src) = both(&ctx, &m, 96, 64);
        assert_agrees("a linear gradient", &want, &got, &src);
    }

    #[test]
    fn a_radial_mask_renders_the_same_picture() {
        let Some(ctx) = ctx() else { return };
        for feather in [0.0f32, 45.0] {
            let m = with_mask(Mask::Radial(edit_model::RadialGradient {
                center: [0.45, 0.55],
                radius: [0.3, 0.22],
                feather,
                invert: false,
            }));
            let (want, got, src) = both(&ctx, &m, 96, 64);
            assert_agrees(
                &format!("a radial mask at feather {feather}"),
                &want,
                &got,
                &src,
            );
        }
    }

    /// ⚠⚠ **`RadialGradient::coverage` takes an aspect and IGNORES it**, and this is the guard
    /// against someone "fixing" that in the shader. An axis-aligned ellipse maps to an
    /// axis-aligned ellipse under anisotropic scaling, so the expression is already a correct
    /// on-screen ellipse; `radial_coverage_is_independent_of_aspect` says the same on the CPU.
    ///
    /// ⚠ A **square** fixture cannot see this — there the aspect is 1.0 and the bug is inert. The
    /// two frames below are wide and tall, and the mask's coverage must be identical in both.
    #[test]
    fn the_radial_kernel_ignores_the_aspect_exactly_as_the_cpu_does() {
        let Some(ctx) = ctx() else { return };
        let m = with_mask(Mask::Radial(edit_model::RadialGradient {
            center: [0.5, 0.5],
            radius: [0.3, 0.3],
            feather: 40.0,
            invert: false,
        }));
        for (w, h) in [(128usize, 32usize), (32, 128)] {
            let (want, got, src) = both(&ctx, &m, w, h);
            assert_agrees(
                &format!("a radial mask on a {w}x{h} frame"),
                &want,
                &got,
                &src,
            );
        }
    }

    #[test]
    fn a_luminance_range_mask_renders_the_same_picture() {
        let Some(ctx) = ctx() else { return };
        for feather in [0.0f32, 50.0] {
            let m = with_mask(Mask::Luminance(edit_model::LuminanceRange {
                low: 25.0,
                high: 70.0,
                feather,
                invert: false,
            }));
            let (want, got, src) = both(&ctx, &m, 96, 64);
            assert_agrees(
                &format!("a luminance range at feather {feather}"),
                &want,
                &got,
                &src,
            );
        }
    }

    /// ⚠⚠ **The index clamp, and it is the `ProfileHueSatMap` trap this project has met three
    /// times.** A scene-referred sample above white encodes to a lightness above 100; the
    /// *selector* is clamped so `high: 100` correctly means "and everything brighter", while the
    /// **pixel keeps its headroom** for the adjustment that follows. Clamping the pixel instead
    /// would drag every highlight back to white and destroy the latitude the wide working space
    /// exists for.
    ///
    /// ⚠ A fixture confined to `[0, 1]` cannot see this at all.
    #[test]
    fn a_sample_above_white_selects_at_the_top_and_keeps_its_headroom() {
        let Some(ctx) = ctx() else { return };
        let m = with_mask(Mask::Luminance(edit_model::LuminanceRange {
            low: 60.0,
            high: 100.0,
            feather: 0.0,
            invert: false,
        }));
        // Three pixels: well above white, just above white, and a dark one outside the band.
        let src = ImageBuf::from_vec(3, 1, vec![4.0, 4.0, 4.0, 1.2, 1.2, 1.2, 0.02, 0.02, 0.02]);
        let want = pipeline::MaskStage::new(&m, RenderScale::FULL, &Geometry::default())
            .apply(src.clone());
        let stage = gpu_stage(&ctx, &m).expect("builds");
        let got = crate::GpuChain::new(ctx.clone(), vec![Box::new(stage)]).apply(src.clone());

        // ⚠ Compared as RAW FLOATS, not through `compare`: `compare` quantizes to 8-bit levels,
        // where everything above 1.0 reads as 255 and the headroom is invisible. That is the
        // clamp lesson M8.3e recorded for the output transform, in a new place.
        for (i, (g, w)) in got.data().iter().zip(want.data()).enumerate() {
            assert!((g - w).abs() < 1e-3, "sample {i}: gpu {g} vs cpu {w}");
        }
        assert!(
            want.data()[0] > 1.0,
            "fixture check: the above-white pixel must still be above white after the mask, was {}",
            want.data()[0]
        );
    }

    #[test]
    fn a_colour_range_mask_renders_the_same_picture() {
        let Some(ctx) = ctx() else { return };
        // ⚠ A band that CROSSES ZERO — `low > high` — which is the single most commonly masked
        // colour: sRGB red sits at 29.2 degrees and magenta at 328.4, either side of 0. A
        // non-wrapping band cannot express it, and this is the fixture that exercises the wrap.
        for (low, high) in [(340.0f32, 20.0f32), (90.0, 160.0)] {
            let m = with_mask(Mask::Colour(edit_model::ColourRange {
                low,
                high,
                feather: 30.0,
                min_chroma: 0.02,
                invert: false,
            }));
            let (want, got, src) = both(&ctx, &m, 96, 64);
            assert_agrees(&format!("a colour range {low}..{high}"), &want, &got, &src);
        }
    }

    /// ⚠⚠ **WGSL's `%` keeps the sign of the dividend; `rem_euclid` does not — and the
    /// discriminating input is NOT simply a negative hue.** The two differ by a multiple of 360,
    /// which **cancels** in `offset - span`, so the `past` shoulder agrees either way. Only the
    /// `before` shoulder — a pixel whose hue sits just *below* `low` — can tell them apart.
    ///
    /// ⚠⚠ The first version of this test used a band across 0 degrees and negative hues, looked
    /// exactly right, and **passed under the `%` sabotage**: every one of its pixels landed on the
    /// `past` shoulder. It is the standing lesson again — *a test can name a bug it cannot
    /// detect* — and it was caught only because the sabotage step ran.
    ///
    /// ⚠ Measured for this fixture: `rem_euclid` yields band values of **0.028 / 0.697 / 0.997 /
    /// 1.000 / 0.000** where `%` yields **1.0 for every pixel**. Four of the five discriminate.
    #[test]
    fn the_hue_wrap_agrees_with_rem_euclid_on_the_before_shoulder() {
        let Some(ctx) = ctx() else { return };
        let m = with_mask(Mask::Colour(edit_model::ColourRange {
            low: 200.0,
            high: 260.0,
            feather: 25.0,
            min_chroma: 0.01,
            invert: false,
        }));
        // Green through cyan to blue, crossing the band's lower shoulder, plus a magenta far
        // outside it. Every one of these has `hue - low < 0`.
        let src = ImageBuf::from_vec(
            5,
            1,
            vec![
                0.12, 0.40, 0.16, // hue 159.5, band 0.028
                0.10, 0.34, 0.30, // hue -176.4, band 0.697
                0.10, 0.30, 0.38, // hue -161.4, band 0.997
                0.10, 0.24, 0.46, // hue -143.1, band 1.000
                0.45, 0.10, 0.35, // hue -15.9, band 0.000
            ],
        );
        let want = pipeline::MaskStage::new(&m, RenderScale::FULL, &Geometry::default())
            .apply(src.clone());
        let stage = gpu_stage(&ctx, &m).expect("builds");
        let got = crate::GpuChain::new(ctx.clone(), vec![Box::new(stage)]).apply(src.clone());

        // ⚠⚠ The fixture check that stops this going blind again: the band must land at
        // INTERMEDIATE values here. A fixture where every pixel reads 0 or 1 is one where `%` and
        // `rem_euclid` can agree by accident, which is exactly how the first version passed.
        let ratios: Vec<f32> = (0..5)
            .map(|i| want.data()[i * 3 + 1] / src.data()[i * 3 + 1])
            .collect();
        assert!(
            ratios.iter().any(|r| *r > 1.05 && *r < 2.7),
            "fixture check: at least one pixel must be PARTIALLY covered, ratios {ratios:?}"
        );

        let d = crate::compare(&want, &got);
        assert!(
            d.worst_levels < 1.0,
            "the hue wrap diverged by {} levels over {:.3}%",
            d.worst_levels,
            d.differing_percent()
        );
    }

    /// ⚠⚠ **This test asserted something FALSE about correct code, and the correction is the
    /// useful part.** It was written to say an inverted colour range leaves neutrals alone.
    /// Measured, the CPU gives a neutral coverage **1.0** under inversion — `c = band * gate` is 0
    /// for a pixel with no chroma, and `1 - 0` is 1. That is right: inverting means "everything
    /// except these colours", and a grey is not the band's hue, so the complement selects it.
    ///
    /// ⚠ What inversion must actually do is **exclude the in-band saturated colour**, and that is
    /// what is pinned below — together with the GPU agreeing with the CPU on all three pixels,
    /// which is what would catch the gate being applied on the wrong side of the flip.
    #[test]
    fn an_inverted_colour_range_excludes_the_colours_it_names() {
        let Some(ctx) = ctx() else { return };
        let m = with_mask(Mask::Colour(edit_model::ColourRange {
            low: 90.0,
            high: 160.0,
            feather: 20.0,
            min_chroma: 0.02,
            invert: true,
        }));
        // Two near-neutrals and one saturated green, which the un-inverted band would select.
        let src = ImageBuf::from_vec(
            3,
            1,
            vec![0.30, 0.30, 0.30, 0.62, 0.61, 0.63, 0.12, 0.45, 0.16],
        );
        let want = pipeline::MaskStage::new(&m, RenderScale::FULL, &Geometry::default())
            .apply(src.clone());
        let stage = gpu_stage(&ctx, &m).expect("builds");
        let got = crate::GpuChain::new(ctx.clone(), vec![Box::new(stage)]).apply(src.clone());

        let d = crate::compare(&want, &got);
        assert!(
            d.worst_levels < 1.0,
            "an inverted colour range diverged by {} levels",
            d.worst_levels
        );
        // ⚠ And the property itself, on the CPU reference, so the test says what it means rather
        // than only that the two agree: the **saturated green** is the colour the band names, so
        // inverting must leave it essentially untouched.
        for i in 6..9 {
            assert!(
                (want.data()[i] - src.data()[i]).abs() < 0.02,
                "the in-band colour was selected by an INVERTED range: {} -> {}",
                src.data()[i],
                want.data()[i]
            );
        }
        // ⚠ And the measured counterpart, recorded rather than assumed: a neutral IS selected,
        // because it is not the band's hue. A test asserting otherwise is asserting something
        // false about correct code — which this one did until it was run.
        assert!(
            (want.data()[0] - src.data()[0]).abs() > 0.1,
            "a neutral must be selected by an inverted colour range: {} -> {}",
            src.data()[0],
            want.data()[0]
        );
    }

    /// ⚠ A gating brush reads the photograph, which does not exist at construction. Declining is
    /// the gate's own rule — fail toward the CPU — and `accelerate` leaves the `MaskStage` alone.
    /// A base with refinements folded onto it, in list order.
    fn composite(base: Mask, refine: &[(edit_model::MaskOp, Mask)]) -> MaskedAdjustment {
        let mut m = with_mask(base);
        m.refine = refine
            .iter()
            .map(|(op, mask)| edit_model::Refinement {
                op: *op,
                mask: mask.clone(),
            })
            .collect();
        m
    }

    fn a_gradient() -> Mask {
        Mask::Linear(edit_model::LinearGradient {
            from: [0.15, 0.2],
            to: [0.8, 0.75],
            invert: false,
        })
    }

    fn an_ellipse() -> Mask {
        Mask::Radial(edit_model::RadialGradient {
            center: [0.45, 0.5],
            radius: [0.3, 0.26],
            feather: 45.0,
            invert: false,
        })
    }

    /// ⚠⚠ **The fold, against `Composite::at` itself.** Each op is compared with the CPU's own
    /// `MaskOp::combine`, which is the brush raster's algebra — so a fold spelled `min`/`max`
    /// here, which agrees at 0 and 1 and differs at every partial coverage, fails.
    #[test]
    fn every_fold_renders_the_same_picture() {
        let Some(ctx) = ctx() else { return };
        for op in edit_model::MaskOp::ALL {
            let m = composite(a_gradient(), &[(op, an_ellipse())]);
            let (want, got, src) = both(&ctx, &m, 64, 48);
            assert_agrees(&format!("{op:?}"), &want, &got, &src);
        }
    }

    /// ⚠⚠ **Three folds rendering alike would pass the test above**, because each side would use
    /// the same wrong op. The grading goldens' rule, applied before the equivalence is believed.
    #[test]
    fn the_three_folds_render_three_different_pictures() {
        let Some(ctx) = ctx() else { return };
        let out: Vec<ImageBuf> = edit_model::MaskOp::ALL
            .iter()
            .map(|op| {
                both(
                    &ctx,
                    &composite(a_gradient(), &[(*op, an_ellipse())]),
                    64,
                    48,
                )
                .1
            })
            .collect();
        for (a, b) in [(0, 1), (0, 2), (1, 2)] {
            let d = crate::compare(&out[a], &out[b]);
            assert!(
                d.worst_levels > 8.0,
                "fixture check: {:?} and {:?} must differ, worst {}",
                edit_model::MaskOp::ALL[a],
                edit_model::MaskOp::ALL[b],
                d.worst_levels
            );
        }
    }

    /// ⚠⚠ **The test the concatenated coverage buffer exists for.** Two painted components share
    /// one binding and are told apart by their word offsets; an offset applied in samples rather
    /// than words, or dropped entirely, makes the second read the first's coverage — a plausible
    /// mask, and of the wrong shape.
    #[test]
    fn a_composite_of_two_brushes_reads_each_ones_own_coverage() {
        let Some(ctx) = ctx() else { return };
        let mut second = painted(false);
        if let Mask::Brush(b) = &mut second.mask {
            // ⚠ Deliberately a different box from the first: two identical brushes would read the
            // same bytes whatever the offsets, which is exactly the blind spot being avoided.
            b.strokes[0].points = (0..10).map(|i| [0.5, 0.2 + i as f32 * 0.05]).collect();
            b.strokes[0].radius = 0.08;
        }
        let m = composite(
            painted(false).mask,
            &[(edit_model::MaskOp::Add, second.mask)],
        );
        let (want, got, src) = both(&ctx, &m, 64, 48);
        assert_agrees("two brushes", &want, &got, &src);
    }

    /// ⚠ Three colour ranges end to end: the hoist is asserted on the generated text by
    /// `three_colour_components_hoist_one_oklab_round_trip`, and this says the picture it produces
    /// is still the CPU's.
    #[test]
    fn a_composite_of_three_colour_ranges_renders_the_same_picture() {
        let Some(ctx) = ctx() else { return };
        let range = |low: f32, high: f32| {
            Mask::Colour(edit_model::ColourRange {
                low,
                high,
                feather: 30.0,
                min_chroma: 0.02,
                invert: false,
            })
        };
        let m = composite(
            range(0.0, 60.0),
            &[
                (edit_model::MaskOp::Add, range(100.0, 160.0)),
                (edit_model::MaskOp::Subtract, range(200.0, 260.0)),
            ],
        );
        let (want, got, src) = both(&ctx, &m, 64, 48);
        assert_agrees("three colour ranges", &want, &got, &src);
    }

    /// ⚠⚠ **The test `facts()`'s union exists for, and four other composite tests could not see
    /// it.** Every one of them mixes components of a single *kind*, so "the union over the chain"
    /// and "the base's own facts" agree on all of them — taking the base's alone left them green.
    /// Found by running the sabotage, not by reading.
    ///
    /// ⚠ Here the base is a shape, which reads no pixel, and the refinement is a range, which
    /// reads two. A hoist driven by the base alone emits nothing, and the chain then references a
    /// `hue` that was never declared.
    #[test]
    fn a_shape_refined_by_a_range_hoists_what_the_refinement_needs() {
        let Some(ctx) = ctx() else { return };
        let m = composite(
            a_gradient(),
            &[(
                edit_model::MaskOp::Intersect,
                Mask::Colour(edit_model::ColourRange {
                    low: 90.0,
                    high: 200.0,
                    feather: 30.0,
                    min_chroma: 0.02,
                    invert: false,
                }),
            )],
        );
        let stage = gpu_stage(&ctx, &m).expect("builds");
        let src = stage.shader();
        assert!(
            src.contains("let hue"),
            "fixture check: the refinement's fact must be hoisted:\n{src}"
        );
        let (want, got, source) = both(&ctx, &m, 64, 48);
        assert_agrees("shape refined by a range", &want, &got, &source);
    }

    /// ⚠ Both facts at once, from two different kinds. A union that took only the *first* fact it
    /// saw would hoist the luma and leave the colour component's `hue` undeclared.
    #[test]
    fn a_luminance_and_a_colour_component_hoist_both_facts() {
        let Some(ctx) = ctx() else { return };
        let m = composite(
            Mask::Luminance(edit_model::LuminanceRange {
                low: 20.0,
                high: 80.0,
                feather: 25.0,
                invert: false,
            }),
            &[(
                edit_model::MaskOp::Add,
                Mask::Colour(edit_model::ColourRange {
                    low: 0.0,
                    high: 90.0,
                    feather: 30.0,
                    min_chroma: 0.02,
                    invert: false,
                }),
            )],
        );
        let stage = gpu_stage(&ctx, &m).expect("builds");
        let src = stage.shader();
        assert!(src.contains("let lum"), "{src}");
        assert!(src.contains("let hue"), "{src}");
        let (want, got, source) = both(&ctx, &m, 64, 48);
        assert_agrees("luminance + colour", &want, &got, &source);
    }

    /// ⚠⚠ **Every component's coverage is charged, and the sabotage that found this was caught by
    /// nothing else.** `Composite::at` folds the whole chain per pixel with no early-out, so a
    /// refinement's coverage is work the CPU really does.
    ///
    /// ⚠ Undercounting is wrong in the SAFE direction — the gate must fail toward the CPU — which
    /// is exactly why nothing else sees it: the picture is identical and only the speed is lost.
    /// What it strands is a cheap base refined by an expensive range, which is the shape the fold
    /// exists for.
    ///
    /// ⚠⚠ **It asserts on `cpu_cost` rather than on whether the mask was substituted, because a
    /// mask's own cost is NOT observable at the gate.** `worth_a_chain` sums a whole run, and
    /// every tail ends in `output_transform` at 28.6 ms — which clears the 27.0 ms bar on its own,
    /// M8.3h's finding. A decision-level test would pass whatever this stage reported, and its
    /// fixture check is what said so.
    #[test]
    fn a_refinements_coverage_is_charged_too() {
        let Some(ctx) = ctx() else { return };
        let range = Mask::Colour(edit_model::ColourRange {
            low: 0.0,
            high: 90.0,
            feather: 30.0,
            min_chroma: 0.02,
            invert: false,
        });
        let bare = gpu_stage(&ctx, &composite(a_gradient(), &[])).expect("builds");
        let refined = gpu_stage(
            &ctx,
            &composite(a_gradient(), &[(edit_model::MaskOp::Intersect, range)]),
        )
        .expect("builds");
        let d = refined.cpu_cost() - bare.cpu_cost();
        assert!(
            (d - COLOUR_COVERAGE_COST).abs() < 0.01,
            "a colour-range refinement must add its own coverage cost: {d} vs {COLOUR_COVERAGE_COST}"
        );
    }

    /// ⚠⚠ **The only test in this crate that can see the mask's region handling, and its absence
    /// let a sabotage through.** Taking the aspect from the *rendered buffer* instead of the frame
    /// is a no-op whenever the buffer IS the frame — which every other fixture here is — so the
    /// whole-image tests are structurally blind to it. `VignetteStage` needed exactly this test
    /// for exactly this reason.
    ///
    /// ⚠ The mask is a **diagonal** gradient: an axis-aligned one has a zero component in its
    /// direction vector, and the aspect correction is what stops the iso-lines skewing, so an
    /// axis-aligned fixture could not witness a wrong aspect either.
    #[test]
    fn a_region_is_masked_as_that_part_of_the_whole() {
        let Some(ctx) = ctx() else { return };
        let (fw, fh) = (64usize, 48usize);
        // ⚠⚠ **The region's own aspect must DIFFER from the frame's, and the first version of this
        // test had 24x18 against 64x48 — both exactly 4:3.** The sabotage that takes the aspect
        // from the rendered buffer instead of the frame was still not caught, because on that
        // fixture the two numbers are equal. A blind spot of the same family as an axis-aligned
        // gradient, and one a real region almost never has.
        let (x0, y0, rw, rh) = (20usize, 12usize, 30usize, 12usize);
        assert!(
            ((rw as f32 / rh as f32) - (fw as f32 / fh as f32)).abs() > 0.5,
            "fixture check: the region's aspect must differ from the frame's, or this test cannot \
             see an aspect taken from the wrong one"
        );
        let m = with_mask(a_gradient());
        let src = scene(fw, fh);

        // The whole frame on the CPU, then the same rectangle cut out of it.
        let whole = pipeline::MaskStage::new(&m, RenderScale::FULL, &Geometry::default())
            .apply(src.clone());
        let cut = |img: &ImageBuf| {
            let mut o = ImageBuf::new(rw, rh);
            for y in 0..rh {
                for x in 0..rw {
                    o.set_pixel(x, y, img.pixel(x + x0, y + y0));
                }
            }
            o
        };
        let want = cut(&whole);

        // The same rectangle rendered as a region of that frame.
        let region = Region {
            full_w: fw,
            full_h: fh,
            x0,
            y0,
        };
        let stage = GpuMaskStage::new(
            &m,
            &Geometry::default(),
            Some(region),
            RenderScale::FULL,
            color::wb::Illuminant::default(),
            &[],
            subs_for(&ctx, &m),
            &ctx,
        )
        .expect("builds");
        let got = crate::GpuChain::new(ctx.clone(), vec![Box::new(stage)]).apply(cut(&src));

        assert_agrees("region", &want, &got, &cut(&src));
    }

    /// How far each mask kind's GPU twin diverges from its CPU one, in 8-bit levels.
    ///
    /// ⚠ `#[ignore]`d and kept, the `probe_box` / `probe_rt` precedent: a number nobody can
    /// re-measure is a number nobody can defend. The equivalence tests assert a bound of one
    /// level; this says where each kind actually sits inside it, which is what makes tightening
    /// that bound later a decision rather than a guess.
    ///
    /// `cargo test --release -p gpu-pipeline probe_divergence -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn probe_divergence() {
        let Some(ctx) = ctx() else { return };
        let colour = |low: f32, high: f32| {
            Mask::Colour(edit_model::ColourRange {
                low,
                high,
                feather: 30.0,
                min_chroma: 0.02,
                invert: false,
            })
        };
        let cases: Vec<(&str, MaskedAdjustment)> = vec![
            ("painted brush", painted(false)),
            ("linear gradient", with_mask(a_gradient())),
            ("radial", with_mask(an_ellipse())),
            (
                "luminance range",
                with_mask(Mask::Luminance(edit_model::LuminanceRange {
                    low: 20.0,
                    high: 80.0,
                    feather: 25.0,
                    invert: false,
                })),
            ),
            ("colour range", with_mask(colour(0.0, 90.0))),
            (
                "COMPOSITE 3 colour ranges",
                composite(
                    colour(0.0, 60.0),
                    &[
                        (edit_model::MaskOp::Add, colour(100.0, 160.0)),
                        (edit_model::MaskOp::Subtract, colour(200.0, 260.0)),
                    ],
                ),
            ),
            (
                "COMPOSITE gradient n range",
                composite(
                    a_gradient(),
                    &[(edit_model::MaskOp::Intersect, colour(150.0, 260.0))],
                ),
            ),
        ];
        println!("\n  mask kind                    worst levels   % of samples");
        for (label, m) in &cases {
            let (want, got, _) = both(&ctx, m, 128, 96);
            let d = crate::compare(&want, &got);
            println!(
                "  {label:<28} {:>8.5}   {:>8.4}%",
                d.worst_levels,
                d.differing_percent()
            );
        }
    }

    /// ⚠ A gating brush in a REFINEMENT declines the whole mask, not just that component. A
    /// composite is one coverage plane, so there is no half of it to accelerate.
    #[test]
    fn a_gating_brush_in_a_refinement_declines_the_whole_mask() {
        let Some(ctx) = ctx() else { return };
        let mut gating = painted(false);
        if let Mask::Brush(b) = &mut gating.mask {
            b.strokes[0].auto_mask = true;
        }
        let m = composite(a_gradient(), &[(edit_model::MaskOp::Add, gating.mask)]);
        assert!(
            gpu_stage(&ctx, &m).is_none(),
            "a gating refinement with no cached map must decline the mask"
        );
        assert!(
            gpu_stage(&ctx, &composite(a_gradient(), &[])).is_some(),
            "fixture check: the same base alone must still build"
        );
    }

    #[test]
    fn a_gating_brush_is_declined() {
        let Some(ctx) = ctx() else { return };
        let mut m = painted(false);
        let Mask::Brush(b) = &mut m.mask else {
            unreachable!()
        };
        b.strokes[0].auto_mask = true;
        assert!(
            gpu_stage(&ctx, &m).is_none(),
            "a gating brush must be declined, not rasterized without its base"
        );
    }

    /// ⚠⚠ **The sum is forced by `GpuChain` lending the SAME planes to every stage.** A stage
    /// asking for only its own four would have `base` clobbered by the first sub-stage wanting
    /// scratch — sharpening 4, local contrast 5, NR 4 — and the result is a plausible photograph
    /// blended against the wrong thing.
    #[test]
    fn the_scratch_request_covers_the_mask_and_its_widest_sub_stage() {
        let Some(ctx) = ctx() else { return };
        let m = painted(false);
        let stage = gpu_stage(&ctx, &m).expect("builds");
        // One exposure sub-stage, which is per-pixel and asks for none.
        assert_eq!(stage.scratch_planes(), 4);
    }
}
