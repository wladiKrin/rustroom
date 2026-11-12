//! `SceneOffsetsStage` on the GPU (M10.12b): every mask's coverage, the summed offsets and the four
//! scene formulas in ONE pass.
//!
//! ⚠ Each mask's coverage is the chain `GpuMaskStage` already compiles — the same components, the
//! same `mask_wgsl` kernels, packed into one component list with one global index — so a mask's
//! coverage on the GPU is one implementation, whichever stage asks for it.

use std::sync::Arc;

use pipeline::PipelineStage;

use crate::context::GpuContext;
use crate::image::GpuImage;
use crate::mask::{Component, GpuMaskStage};
use crate::GpuStage;

/// A coverage plane on the GPU, handed from the offsets twin to a mask's look twin (M10.14h) — the GPU side of
/// [`pipeline::CoverageSlot`]. Filled when the offsets twin is encoded; the mask twin, encoded later in the same
/// pass, binds it as its coverage.
#[derive(Default)]
pub struct GpuCoverageSlot(std::sync::Mutex<Option<Arc<wgpu::Buffer>>>);

impl GpuCoverageSlot {
    pub fn put(&self, plane: Arc<wgpu::Buffer>) {
        *self.0.lock().unwrap_or_else(|e| e.into_inner()) = Some(plane);
    }

    pub fn get(&self) -> Option<Arc<wgpu::Buffer>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }
}

/// The GPU twin of [`pipeline::SceneOffsetsStage`].
pub struct GpuSceneOffsetsStage {
    /// The CPU twin — the equivalence reference, rebuilt from the original's parts.
    cpu: pipeline::SceneOffsetsStage,
    /// Each contributing mask's coverage chain, in the CPU stage's order.
    chains: Vec<Chain>,
    /// [`STRIDE`] floats of global values, then [`STRIDE`] per mask — what the shader reads.
    values: Vec<f32>,
    region: Option<pipeline::Region>,
    cost: f32,
    /// ⚠⚠ M10.14h: the REFERENCE photo's stages — the global look, twinned from the names the CPU stage reports —
    /// `Some` exactly when a mask reads it. They run on a copy of the input inside this stage's encode.
    reference: Option<Vec<Box<dyn GpuStage>>>,
    /// Per chain, in order: where a mask that reads the reference leaves its coverage — computed on the reference,
    /// used here, and bound by that mask's look twin.
    reads: Vec<Option<Arc<GpuCoverageSlot>>>,
}

/// One mask's coverage, as `GpuMaskStage` holds it.
struct Chain {
    base: Component,
    refine: Vec<(crate::mask_wgsl::Fold, Component)>,
    invert: bool,
}

/// Floats per row of `values`: the global row is `[gain r, gain g, gain b, ev, contrast, highlights,
/// shadows, whites, blacks]`, a mask's `[ev, contrast, highlights, shadows, whites, blacks, ln R r,
/// ln R g, ln R b]` — both padded to a round stride.
const STRIDE: usize = 12;

/// The CPU costs of the four stages this replaces, as their own twins report them (white balance
/// 4.0, exposure 1.3, contrast 1.2, tone regions 18.0 ms per 6.8 Mpx); each mask's coverage adds
/// its components' own measured cost on top.
const SCENE_COST: f32 = 4.0 + 1.3 + 1.2 + 18.0;

impl GpuSceneOffsetsStage {
    /// The twin of `cpu`, or `None` where a mask's coverage cannot be built here — a gating brush
    /// without a map, an AI mask still waiting — and the CPU stage runs instead.
    ///
    /// ⚠ A stage carrying the reference photo declines here — [`Self::with_reference`] is its constructor.
    pub fn new(cpu: &pipeline::SceneOffsetsStage, ctx: &Arc<GpuContext>) -> Option<Self> {
        if cpu.has_reference() {
            return None;
        }
        let n = cpu.contributions().count();
        Self::build(cpu, ctx, None, vec![None; n])
    }

    /// The twin of a stage carrying the reference photo (M10.14h): `reference` the twins of
    /// [`pipeline::SceneOffsetsStage::reference_names`], in order; `slots` one per contributing mask, `Some` exactly
    /// where the CPU stage's own slot is. `None` on any mismatch — the CPU stage runs instead.
    ///
    /// ⚠⚠ The caller (the accelerator's walk) builds this only when every mask that reads one of `slots` gets a
    /// look twin bound to it, in the same chain: a CPU mask would read its CPU slot, which a GPU offsets stage never
    /// fills.
    pub fn with_reference(
        cpu: &pipeline::SceneOffsetsStage,
        ctx: &Arc<GpuContext>,
        reference: Vec<Box<dyn GpuStage>>,
        slots: Vec<Option<Arc<GpuCoverageSlot>>>,
    ) -> Option<Self> {
        let names = cpu.reference_names();
        let matches = cpu.has_reference()
            && reference.len() == names.len()
            && reference.iter().zip(&names).all(|(g, n)| g.name() == *n)
            && slots.len() == cpu.slots().count()
            && slots
                .iter()
                .zip(cpu.slots())
                .all(|(g, c)| g.is_some() == c.is_some());
        if !matches {
            return None;
        }
        Self::build(cpu, ctx, Some(reference), slots)
    }

    fn build(
        cpu: &pipeline::SceneOffsetsStage,
        _ctx: &Arc<GpuContext>,
        reference: Option<Vec<Box<dyn GpuStage>>>,
        reads: Vec<Option<Arc<GpuCoverageSlot>>>,
    ) -> Option<Self> {
        let mut values = vec![0.0f32; STRIDE];
        values[..9].copy_from_slice(&cpu.global_values());
        let mut chains = Vec::new();
        let mut region = None;
        for (stage, offsets) in cpu.contributions() {
            let (masked, geometry, r) = stage.mask_description()?;
            region = r;
            let pre = stage.prebuilt_coverage();
            let at = |k: usize| pre.get(k).cloned().flatten();
            let base = GpuMaskStage::component(&masked.mask, geometry, at(0))?;
            let refine = masked
                .refine
                .iter()
                .enumerate()
                .map(|(k, r)| {
                    GpuMaskStage::component(&r.mask, geometry, at(k + 1))
                        .map(|c| (crate::mask::fold_of(r.op), c))
                })
                .collect::<Option<Vec<_>>>()?;
            chains.push(Chain {
                base,
                refine,
                invert: masked.invert,
            });
            let mut row = [0.0f32; STRIDE];
            row[..9].copy_from_slice(&offsets);
            values.extend_from_slice(&row);
        }
        let cost = SCENE_COST
            + chains
                .iter()
                .flat_map(|c| std::iter::once(&c.base).chain(c.refine.iter().map(|(_, k)| k)))
                .map(Component::coverage_cost)
                .sum::<f32>()
            + reference
                .iter()
                .flatten()
                .map(|s| s.cpu_cost())
                .sum::<f32>();
        Some(Self {
            // ⚠ M10.14h: the copy SHARES the reference and the slots, so a chain failing on the GPU renders the CPU
            // answer and fills the slots the CPU masks then read.
            cpu: cpu.duplicate()?,
            chains,
            values,
            region,
            cost,
            reference,
            reads,
        })
    }

    /// Every component of every mask, in one global order — the index `comps[j]` and the packed
    /// maps are laid down in.
    fn components(&self) -> impl Iterator<Item = &Component> {
        self.chains
            .iter()
            .flat_map(|c| std::iter::once(&c.base).chain(c.refine.iter().map(|(_, k)| k)))
    }

    /// How many masks read the reference — the planes this stage writes.
    fn reading(&self) -> usize {
        self.reads.iter().flatten().count()
    }

    fn shader(&self) -> String {
        use crate::mask_wgsl::{chain, fact_lines, Facts};
        // ⚠ The global component index — `components()`' order, the one `comps[j]` and the packed
        // maps are laid down in: each mask's base, then its refinements, then the next mask's.
        let mut start = 0usize;
        let mut facts = Facts::default();
        let mut body = String::new();
        // M10.14h: the reference kernel's — each reading mask's coverage on the REFERENCE, into its plane.
        let mut ref_facts = Facts::default();
        let mut ref_body = String::new();
        let mut plane = 0usize;
        for (k, c) in self.chains.iter().enumerate() {
            let base = c.base.expr(start);
            let refine: Vec<(crate::mask_wgsl::Fold, String)> = c
                .refine
                .iter()
                .enumerate()
                .map(|(r, (op, comp))| (*op, comp.expr(start + 1 + r)))
                .collect();
            start += 1 + c.refine.len();
            let reads = self.reads.get(k).is_some_and(Option::is_some);
            for f in std::iter::once(&c.base)
                .chain(c.refine.iter().map(|(_, k)| k))
                .map(Component::facts)
            {
                let into = if reads { &mut ref_facts } else { &mut facts };
                into.luma |= f.luma;
                into.oklab |= f.oklab;
            }
            if reads {
                // ⚠⚠ Selected on the reference photo, ONCE: the plane this kernel reads is the one the mask's look
                // twin blends by, so neither half chases the other's effect (M10.12b).
                ref_body.push_str(&chain(&format!("a{k}"), &base, &refine, c.invert));
                ref_body.push_str(&format!("    plane{plane}[i] = clamp(a{k}, 0.0, 1.0);\n"));
                body.push_str(&format!("    let a{k} = plane{plane}[i];\n"));
                plane += 1;
            } else {
                body.push_str(&chain(&format!("a{k}"), &base, &refine, c.invert));
            }
            let o = STRIDE * (k + 1);
            body.push_str(&format!(
                "    let c{k} = clamp(a{k}, 0.0, 1.0);\n\
                 \x20   if (c{k} != 0.0) {{\n\
                 \x20       ev = ev + c{k} * scene[{o}u];\n\
                 \x20       con = con + c{k} * scene[{}u];\n\
                 \x20       hi = hi + c{k} * scene[{}u];\n\
                 \x20       sh = sh + c{k} * scene[{}u];\n\
                 \x20       wh = wh + c{k} * scene[{}u];\n\
                 \x20       bl = bl + c{k} * scene[{}u];\n\
                 \x20       lr = lr + c{k} * scene[{}u];\n\
                 \x20       lg = lg + c{k} * scene[{}u];\n\
                 \x20       lb = lb + c{k} * scene[{}u];\n\
                 \x20   }}\n",
                o + 1,
                o + 2,
                o + 3,
                o + 4,
                o + 5,
                o + 6,
                o + 7,
                o + 8,
            ));
        }
        format!(
            "{}{}{}{}{}{}{}{}\n\
             const PIVOT: f32 = {:?};\n\
             const STRENGTH: f32 = {:?};\n\
             const END_STRENGTH: f32 = {:?};\n\
             @group(0) @binding(8) var<storage, read> scene: array<f32>;\n{}",
            crate::srgb_wgsl::SRGB_WGSL,
            crate::luma_wgsl::luma_wgsl(),
            crate::oklab_wgsl::OKLAB_WGSL,
            crate::mask_wgsl::constants(),
            crate::mask_wgsl::PRELUDE,
            crate::mask_wgsl::PAINTED,
            crate::mask_wgsl::ANALYTIC,
            crate::mask_wgsl::RANGES,
            pipeline::CONTRAST_PIVOT,
            pipeline::TONE_REGIONS_STRENGTH,
            pipeline::TONE_REGIONS_END_STRENGTH,
            kernel(&fact_lines(facts), &body),
        ) + &reference_kernels(plane, &fact_lines(ref_facts), &ref_body)
    }

    fn params(&self, img: &GpuImage, row_groups: u32) -> [u8; 48] {
        let (x0, y0, fw, fh) = match self.region {
            Some(r) => (r.x0 as u32, r.y0 as u32, r.full_w as u32, r.full_h as u32),
            None => (0, 0, img.width() as u32, img.height() as u32),
        };
        let mut p = [0u8; 48];
        let mut put = |off: usize, v: u32| p[off..off + 4].copy_from_slice(&v.to_le_bytes());
        put(0, img.pixels());
        put(4, row_groups);
        put(8, img.width() as u32);
        put(16, x0);
        put(20, y0);
        put(24, fw);
        put(28, fh);
        // ⚠ The FRAME's aspect, as `MaskStage::apply` derives it — a region's own would skew a
        // diagonal gradient.
        p[32..36].copy_from_slice(&(fw as f32 / fh as f32).to_le_bytes());
        p
    }
}

/// M10.14h's two extra entry points, when `planes` masks read the reference: `scene_copy` copies the input
/// (`src`) into the reference image bound at 0, and `scene_reference` computes each reading mask's coverage on it.
/// Nothing when no mask reads it, so such a stage's shader is the one it always was.
fn reference_kernels(planes: usize, facts: &str, body: &str) -> String {
    if planes == 0 {
        return String::new();
    }
    let mut out = String::new();
    for r in 0..planes {
        out.push_str(&format!(
            "@group(0) @binding({}) var<storage, read_write> plane{r}: array<f32>;\n",
            9 + r
        ));
    }
    out.push_str(&format!(
        r#"@group(0) @binding({}) var<storage, read> src: array<f32>;

@compute @workgroup_size(64)
fn scene_copy(@builtin(global_invocation_id) id: vec3<u32>) {{
    let i = id.y * (params.row_groups * 64u) + id.x;
    if (i >= params.pixels) {{ return; }}
    let b = i * 3u;
    img[b] = src[b];
    img[b + 1u] = src[b + 1u];
    img[b + 2u] = src[b + 2u];
}}

@compute @workgroup_size(64)
fn scene_reference(@builtin(global_invocation_id) id: vec3<u32>) {{
    let i = id.y * (params.row_groups * 64u) + id.x;
    if (i >= params.pixels) {{ return; }}
    let x = i % params.w;
    let y = i / params.w;
    let fx = params.x0 + x;
    let fy = params.y0 + y;
    let u = f32(fx) / f32(params.fw);
    let v = f32(fy) / f32(params.fh);
    let b = i * 3u;
    let px = vec3<f32>(img[b], img[b + 1u], img[b + 2u]);
{facts}{body}}}
"#,
        9 + planes
    ));
    out
}

/// The one entry point: each mask's coverage from the pixel as it arrives, the summed values, then
/// the four stages' formulas in their order — `SceneOffsetsStage::apply`, line for line.
fn kernel(facts: &str, body: &str) -> String {
    format!(
        r#"
@compute @workgroup_size(64)
fn scene_offsets(@builtin(global_invocation_id) id: vec3<u32>) {{
    let i = id.y * (params.row_groups * 64u) + id.x;
    if (i >= params.pixels) {{ return; }}
    let x = i % params.w;
    let y = i / params.w;
    let fx = params.x0 + x;
    let fy = params.y0 + y;
    let u = f32(fx) / f32(params.fw);
    let v = f32(fy) / f32(params.fh);
    let b = i * 3u;
    let px = vec3<f32>(img[b], img[b + 1u], img[b + 2u]);
{facts}    var ev = scene[3u];
    var con = scene[4u];
    var hi = scene[5u];
    var sh = scene[6u];
    var wh = scene[7u];
    var bl = scene[8u];
    var lr = 0.0;
    var lg = 0.0;
    var lb = 0.0;
{body}    var p = px * vec3<f32>(scene[0u] * exp(lr), scene[1u] * exp(lg), scene[2u] * exp(lb));
    if (ev != 0.0) {{
        p = p * exp2(ev);
    }}
    if (con != 0.0) {{
        let slope = 1.0 + con / 100.0;
        p = max(vec3<f32>(PIVOT) + (p - vec3<f32>(PIVOT)) * slope, vec3<f32>(0.0));
    }}
    if (hi != 0.0 || sh != 0.0 || wh != 0.0 || bl != 0.0) {{
        let e = linear_to_srgb(clamp(prophoto_luminance(p.x, p.y, p.z), 0.0, 1.0));
        let d = 1.0 - e;
        let gain = 1.0 + STRENGTH * (sh / 100.0 * (d * d) + hi / 100.0 * (e * e))
            + END_STRENGTH * (bl / 100.0 * (d * d * d) + wh / 100.0 * (e * e * e));
        p = p * max(gain, 0.0);
    }}
    img[b] = p.x;
    img[b + 1u] = p.y;
    img[b + 2u] = p.z;
}}
"#
    )
}

impl GpuStage for GpuSceneOffsetsStage {
    fn name(&self) -> &'static str {
        "scene_offsets"
    }

    fn cpu_cost(&self) -> f32 {
        self.cost
    }

    /// ⚠ Four planes the shader never touches: `mask_wgsl::PRELUDE` declares `base_r/g/b` and `cov`
    /// at bindings 2-5 for `GpuMaskStage`, and the prelude is shared rather than forked.
    ///
    /// ⚠ M10.14h: or more, for a reference stage that asks — they run before this stage's own kernel, so the planes
    /// are lent to each in turn (`GpuChain`'s rule: the max, never the sum).
    fn scratch_planes(&self) -> usize {
        self.reference
            .iter()
            .flatten()
            .map(|s| s.scratch_planes())
            .fold(4, usize::max)
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
        let planes = self.reading();
        let mut entries = vec![
            entry(0, rw),
            entry(1, wgpu::BufferBindingType::Uniform),
            entry(2, rw),
            entry(3, rw),
            entry(4, rw),
            entry(5, rw),
            entry(6, ro),
            entry(7, ro),
            entry(8, ro),
        ];
        if planes > 0 {
            entries.extend((0..planes).map(|r| entry(9 + r as u32, rw)));
            entries.push(entry(9 + planes as u32, ro));
        }
        let entry_points: &[&'static str] = if planes > 0 {
            &["scene_offsets", "scene_reference", "scene_copy"]
        } else {
            &["scene_offsets"]
        };
        let (layout, pipelines) = ctx.cache.program(
            device,
            "scene_offsets",
            &self.shader(),
            &entries,
            entry_points,
        );
        // ⚠ The FRAME, which without a region is the buffer — `GpuMaskStage::encode`'s rule.
        let frame = match self.region {
            Some(r) => (r.full_w, r.full_h),
            None => (img.width(), img.height()),
        };
        let maps = crate::mask::rasterize_components(self.components(), frame);
        let words = crate::mask::pack_words(&maps);
        let comps = crate::mask::comps_bytes(self.components(), &maps);

        let buffer = |label: &'static str, bytes: &[u8], usage: wgpu::BufferUsages| {
            let b = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: bytes.len() as u64,
                usage: usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            ctx.queue.write_buffer(&b, 0, bytes);
            b
        };
        let uniform = buffer(
            "scene-offsets-params",
            &self.params(img, row_groups),
            wgpu::BufferUsages::UNIFORM,
        );
        let packed = buffer(
            "scene-offsets-coverage",
            bytemuck::cast_slice(&words),
            wgpu::BufferUsages::STORAGE,
        );
        let comp_buf = buffer(
            "scene-offsets-components",
            &comps,
            wgpu::BufferUsages::STORAGE,
        );
        let values = buffer(
            "scene-offsets-values",
            bytemuck::cast_slice(&self.values),
            wgpu::BufferUsages::STORAGE,
        );
        // ⚠⚠ M10.14h: the reference photo — the input copied, the global look's twins run on the copy — and each
        // reading mask's coverage computed on it, into a plane this stage reads below and the mask's look twin binds.
        // A reference image the device cannot hold selects on the input instead: the CPU's empty-slot rule.
        let reference = match (&self.reference, planes) {
            (Some(stages), p) if p > 0 => {
                GpuImage::new(ctx, img.width(), img.height()).map(|r| (stages, r))
            }
            _ => None,
        };
        let plane_bufs: Vec<Arc<wgpu::Buffer>> = (0..planes)
            .map(|_| {
                Arc::new(device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("scene-offsets-plane"),
                    size: (img.pixels().max(1) as u64) * 4,
                    usage: wgpu::BufferUsages::STORAGE,
                    mapped_at_creation: false,
                }))
            })
            .collect();
        let group = |image: &wgpu::Buffer, src: &wgpu::Buffer, label: &'static str| {
            let mut bound = vec![
                image.as_entire_binding(),
                uniform.as_entire_binding(),
                scratch[0].as_entire_binding(),
                scratch[1].as_entire_binding(),
                scratch[2].as_entire_binding(),
                scratch[3].as_entire_binding(),
                packed.as_entire_binding(),
                comp_buf.as_entire_binding(),
                values.as_entire_binding(),
            ];
            if planes > 0 {
                bound.extend(plane_bufs.iter().map(|b| b.as_entire_binding()));
                bound.push(src.as_entire_binding());
            }
            let entries: Vec<wgpu::BindGroupEntry<'_>> = bound
                .into_iter()
                .enumerate()
                .map(|(i, resource)| wgpu::BindGroupEntry {
                    binding: i as u32,
                    resource,
                })
                .collect();
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &layout,
                entries: &entries,
            })
        };
        if planes > 0 {
            // ⚠ The copy's source is the input; on the main group `src` is unused and bound to a read-only buffer —
            // one buffer read-write and read-only in one group is a validation error.
            let (image, src) = match &reference {
                Some((_, r)) => (&r.buffer, &img.buffer),
                None => (&img.buffer, &values),
            };
            let on_ref = group(image, src, "scene-offsets-reference");
            if let Some((stages, r)) = &reference {
                pass.set_pipeline(&pipelines[2]);
                pass.set_bind_group(0, &on_ref, &[]);
                pass.dispatch_workgroups(row_groups, col_groups, 1);
                for st in stages.iter() {
                    st.encode(ctx, pass, r, &scratch[..st.scratch_planes()]);
                }
            }
            pass.set_pipeline(&pipelines[1]);
            pass.set_bind_group(0, &on_ref, &[]);
            pass.dispatch_workgroups(row_groups, col_groups, 1);
            for (slot, plane) in self.reads.iter().flatten().zip(&plane_bufs) {
                slot.put(Arc::clone(plane));
            }
        }
        let bind = group(&img.buffer, &values, "scene-offsets-bind");
        pass.set_pipeline(&pipelines[0]);
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
    use edit_model::{
        Brush, BrushStroke, ColourRange, Edits, Geometry, LinearGradient, LocalAdjustments,
        LuminanceRange, Mask, MaskOp, MaskedAdjustment, RadialGradient, Refinement,
    };
    use pipeline::{RenderScale, SceneOffsetsStage};

    fn ctx() -> Option<Arc<GpuContext>> {
        match crate::shared() {
            Some(c) => Some(c.clone()),
            None => {
                eprintln!("SKIPPED: no GPU adapter on this machine");
                None
            }
        }
    }

    fn scene(w: usize, h: usize) -> ImageBuf {
        let mut v = Vec::with_capacity(w * h * 3);
        for i in 0..w * h {
            for stride in [37usize, 61, 89] {
                v.push(((i * stride) % 313) as f32 / 313.0 * 1.4 - 0.1);
            }
        }
        ImageBuf::from_vec(w, h, v)
    }

    fn masked(mask: Mask, a: LocalAdjustments) -> MaskedAdjustment {
        MaskedAdjustment {
            mask,
            adjustments: a,
            ..Default::default()
        }
    }

    fn cpu_for(e: &Edits, region: Option<pipeline::Region>) -> SceneOffsetsStage {
        SceneOffsetsStage::new(
            e,
            RenderScale::FULL,
            region,
            &Geometry::default(),
            None,
            color::wb::Illuminant::default(),
            Vec::new,
        )
        .expect("a mask with a scene offset builds the stage")
    }

    fn both(ctx: &Arc<GpuContext>, e: &Edits, src: &ImageBuf) -> (ImageBuf, ImageBuf) {
        let cpu = cpu_for(e, None);
        let want = cpu.apply(src.clone());
        let g = GpuSceneOffsetsStage::new(&cpu, ctx).expect("the twin builds");
        let got = crate::GpuChain::new(ctx.clone(), vec![Box::new(g)]).apply(src.clone());
        (want, got)
    }

    fn assert_agrees(label: &str, want: &ImageBuf, got: &ImageBuf, src: &ImageBuf) {
        let moved = crate::compare(src, want);
        assert!(
            moved.worst_levels > 8.0,
            "fixture check ({label}): the CPU stage must move the picture, moved {}",
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

    fn radial(feather: f32) -> Mask {
        Mask::Radial(RadialGradient {
            center: [0.45, 0.5],
            radius: [0.3, 0.26],
            feather,
            invert: false,
        })
    }

    fn diagonal() -> Mask {
        Mask::Linear(LinearGradient {
            from: [0.15, 0.2],
            to: [0.8, 0.75],
            invert: false,
        })
    }

    #[test]
    fn every_mask_kind_and_slider_renders_like_the_cpu() {
        let Some(ctx) = ctx() else { return };
        let src = scene(64, 48);
        let global = Edits {
            exposure: 0.3,
            contrast: 20.0,
            highlights: -30.0,
            white_balance: edit_model::WhiteBalance {
                kelvin: Some(6000.0),
                tint: 5.0,
            },
            ..Default::default()
        };
        let mut composite = masked(
            radial(30.0),
            LocalAdjustments {
                shadows: 50.0,
                ..Default::default()
            },
        );
        composite.refine.push(Refinement {
            op: MaskOp::Subtract,
            mask: Mask::Radial(RadialGradient {
                center: [0.5, 0.5],
                radius: [0.1, 0.1],
                feather: 0.0,
                invert: false,
            }),
        });
        composite.invert = true;
        let cases: Vec<(&str, Vec<MaskedAdjustment>)> = vec![
            (
                "linear exposure",
                vec![masked(
                    diagonal(),
                    LocalAdjustments {
                        exposure: 1.0,
                        ..Default::default()
                    },
                )],
            ),
            (
                "radial contrast",
                vec![masked(
                    radial(40.0),
                    LocalAdjustments {
                        contrast: 40.0,
                        ..Default::default()
                    },
                )],
            ),
            (
                "two overlapping",
                vec![
                    masked(
                        radial(50.0),
                        LocalAdjustments {
                            exposure: 0.7,
                            ..Default::default()
                        },
                    ),
                    masked(
                        diagonal(),
                        LocalAdjustments {
                            tint: 30.0,
                            contrast: -20.0,
                            ..Default::default()
                        },
                    ),
                ],
            ),
            ("inverted composite", vec![composite]),
        ];
        for base in [Edits::default(), global] {
            for (label, masks) in &cases {
                let e = Edits {
                    masks: masks.clone(),
                    ..base.clone()
                };
                let (want, got) = both(&ctx, &e, &src);
                assert_agrees(label, &want, &got, &src);
            }
        }
    }

    #[test]
    fn a_region_renders_as_that_part_of_the_whole() {
        let Some(ctx) = ctx() else { return };
        let e = Edits {
            masks: vec![masked(
                diagonal(),
                LocalAdjustments {
                    exposure: 1.0,
                    ..Default::default()
                },
            )],
            ..Default::default()
        };
        let src = scene(64, 48);
        let (x0, y0, w, h) = (20, 10, 30, 25);
        let region = pipeline::Region {
            full_w: 64,
            full_h: 48,
            x0,
            y0,
        };
        let cpu = cpu_for(&e, Some(region));
        let part = src.crop(x0, y0, w, h);
        let want = cpu.apply(part.clone());
        let g = GpuSceneOffsetsStage::new(&cpu, &ctx).expect("the twin builds");
        let got = crate::GpuChain::new(ctx.clone(), vec![Box::new(g)]).apply(part.clone());
        assert_agrees("region", &want, &got, &part);
    }

    fn stroke(auto_mask: bool) -> Mask {
        Mask::Brush(Brush {
            strokes: vec![BrushStroke {
                decode: false,
                points: (0..12).map(|i| [0.1 + i as f32 * 0.07, 0.5]).collect(),
                radius: 0.2,
                hardness: 50.0,
                flow: 100.0,
                auto_mask,
                tolerance: 25.0,
                erase: false,
            }],
            invert: false,
        })
    }

    #[test]
    fn a_brush_renders_like_the_cpu() {
        let Some(ctx) = ctx() else { return };
        let e = Edits {
            masks: vec![masked(
                stroke(false),
                LocalAdjustments {
                    exposure: 1.0,
                    ..Default::default()
                },
            )],
            ..Default::default()
        };
        let src = scene(64, 48);
        let (want, got) = both(&ctx, &e, &src);
        assert_agrees("brush", &want, &got, &src);
    }

    /// ⚠ M10.12b2: a range mask with a scene offset selects on the reference photo — the globally
    /// edited photo, which the GPU does not compute — so the twin declines and the CPU stage runs.
    /// (Range components are still compiled by the shared chain builder; since b2 they never reach
    /// this twin.)
    #[test]
    fn a_stage_with_a_reference_declines() {
        let Some(ctx) = ctx() else { return };
        for mask in [
            Mask::Luminance(LuminanceRange {
                low: 40.0,
                high: 100.0,
                feather: 20.0,
                invert: false,
            }),
            Mask::Colour(ColourRange::default()),
        ] {
            let e = Edits {
                masks: vec![masked(
                    mask,
                    LocalAdjustments {
                        highlights: -60.0,
                        temp: 50.0,
                        ..Default::default()
                    },
                )],
                ..Default::default()
            };
            let cpu = cpu_for(&e, None);
            assert!(
                cpu.has_reference(),
                "fixture check: the range reads the reference"
            );
            assert!(GpuSceneOffsetsStage::new(&cpu, &ctx).is_none());
        }
    }

    /// A gating brush decides its coverage from the photograph — the CPU stage runs instead.
    #[test]
    fn a_gating_brush_declines() {
        let Some(ctx) = ctx() else { return };
        let e = Edits {
            masks: vec![masked(
                stroke(true),
                LocalAdjustments {
                    exposure: 1.0,
                    ..Default::default()
                },
            )],
            ..Default::default()
        };
        assert!(GpuSceneOffsetsStage::new(&cpu_for(&e, None), &ctx).is_none());
    }
}
