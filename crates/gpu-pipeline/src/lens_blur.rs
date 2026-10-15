//! Lens Blur as compute shaders (M10.16d2) — the CPU reference (`pipeline::stages::lens_blur`) line for
//! line, so a Lens Blur drag follows the slider.
//!
//! ⚠⚠ **Its inputs are resolved on the CPU, at construction:** the planes as bytes at the frame's size,
//! the focus, the highlight level (from a fixed grid — `pipeline::lens_blur::highlight_level`) and the
//! levels. Every one is the SAME value the CPU stage computes, through the same functions, so the two
//! differ only by floating-point order — never by an input.

use crate::{GpuContext, GpuImage, GpuStage};
use pipeline::PipelineStage;
use std::sync::Arc;

/// The most levels one frame may need — ½ … 1/256: a disc of 2,048 px at the coarsest.
pub(crate) const MAX_LEVELS: usize = 8;

/// Lens Blur, on the GPU.
pub struct GpuLensBlurStage {
    p: pipeline::lens_blur::Params,
    /// `pipeline::lens_blur::highlight_level`, or `None` with no Boost — the uniform's `boost_k` is
    /// then 0, as the CPU skips the boost.
    l99: Option<f32>,
    depth: Vec<u8>,
    alpha: Option<Vec<u8>>,
    /// Per level: its width, its height, its scale `k`, its first cell.
    levels: Vec<[u32; 4]>,
    cells: u64,
    cpu: pipeline::lens_blur::LensBlurStage,
}

impl GpuLensBlurStage {
    /// `None` — the CPU step then runs — for a blur under half a pixel, more than [`MAX_LEVELS`]
    /// levels, or buffers past the device's limit.
    pub fn new(
        lb: &edit_model::LensBlur,
        inputs: &pipeline::lens_blur::Inputs,
        image: &core_image::ImageBuf,
        ctx: &GpuContext,
    ) -> Option<Self> {
        let (w, h) = (image.width(), image.height());
        let p = pipeline::lens_blur::Params::new(lb, inputs.focus, w.max(h));
        if w == 0 || h == 0 || p.radius_px < 0.5 {
            return None;
        }
        // ⚠ Every level up to the largest radius the frame can hold, sources or not: an empty level
        // gathers nothing and adds zero, so it is the CPU's `0..=level_of(max_c)` exactly.
        let top = pipeline::lens_blur::level_of(p.radius_px);
        if top + 1 > MAX_LEVELS {
            return None;
        }
        let mut levels = Vec::with_capacity(top + 1);
        let mut cells = 0u64;
        for l in 0..=top {
            let k = 1usize << (l + 1);
            let (lw, lh) = (w.div_ceil(k), h.div_ceil(k));
            levels.push([lw as u32, lh as u32, k as u32, cells as u32]);
            cells += (lw * lh) as u64;
        }
        if !fits(cells, (w * h) as u64, ctx.max_image_bytes()) {
            return None;
        }
        let l99 = if p.boost > 0.0 {
            pipeline::lens_blur::highlight_level(image)
        } else {
            None
        };
        Some(Self {
            p,
            l99,
            depth: pipeline::lens_blur::plane_bytes(&*inputs.depth.0, w, h),
            alpha: inputs
                .subject
                .as_ref()
                .map(|s| pipeline::lens_blur::plane_bytes(&*s.0, w, h)),
            levels,
            cells,
            cpu: pipeline::lens_blur::LensBlurStage::new(
                lb,
                inputs.focus,
                inputs.depth.0.clone(),
                inputs.subject.as_ref().map(|s| s.0.clone()),
            ),
        })
    }

    /// The uniform: twelve scalars, then `lv: array<vec4<u32>, 8>` at offset 48 — 176 bytes.
    fn uniform(&self, w: u32, h: u32) -> [u8; 176] {
        let mut b = [0u8; 176];
        let mut put = |at: usize, v: [u8; 4]| b[at..at + 4].copy_from_slice(&v);
        put(0, w.to_le_bytes());
        put(4, h.to_le_bytes());
        put(8, (self.levels.len() as u32).to_le_bytes());
        put(12, crate::lens_blur_wgsl::kind(self.p.bokeh).to_le_bytes());
        put(16, self.p.focus.to_le_bytes());
        put(20, self.p.half_range.to_le_bytes());
        put(24, self.p.radius_px.to_le_bytes());
        // ⚠ `boost`'s own arithmetic: `3 · boost / 100`, nothing when the CPU would skip it.
        let k = match self.l99 {
            Some(_) => 3.0 * self.p.boost / 100.0,
            None => 0.0,
        };
        put(28, k.to_le_bytes());
        put(32, self.l99.unwrap_or(1.0).to_le_bytes());
        put(36, u32::from(self.alpha.is_some()).to_le_bytes());
        for (l, lv) in self.levels.iter().enumerate() {
            for (c, v) in lv.iter().enumerate() {
                put(48 + 16 * l + 4 * c, v.to_le_bytes());
            }
        }
        b
    }
}

/// A plane's bytes padded to whole `u32` words — the shader reads byte `i` of word `i / 4`.
fn packed(bytes: &[u8]) -> Vec<u8> {
    let mut v = bytes.to_vec();
    v.resize(bytes.len().div_ceil(4).max(1) * 4, 0);
    v
}

/// Whether the buffers fit the device: the sources (48 bytes a cell), the gathered light (32 a cell)
/// and the packed planes, each at most `max_bytes`.
fn fits(cells: u64, pixels: u64, max_bytes: u64) -> bool {
    48 * cells <= max_bytes && pixels.div_ceil(4) * 4 <= max_bytes
}

impl GpuStage for GpuLensBlurStage {
    fn name(&self) -> &'static str {
        "lens_blur"
    }

    fn cpu_cost(&self) -> f32 {
        500.0
    }

    fn encode(
        &self,
        ctx: &GpuContext,
        pass: &mut wgpu::ComputePass,
        img: &GpuImage,
        _scratch: &[wgpu::Buffer],
    ) {
        use crate::lens_blur_wgsl::{BUILD, COMPOSITE, GATHER};
        let device = &ctx.device;
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
        let layout_entries = [
            entry(0, rw),
            entry(1, wgpu::BufferBindingType::Uniform),
            entry(2, ro),
            entry(3, ro),
            entry(4, rw),
            entry(5, rw),
        ];
        let names: Vec<&'static str> = BUILD
            .iter()
            .chain(GATHER.iter())
            .copied()
            .chain([COMPOSITE])
            .collect();
        let (layout, pipelines) = ctx.cache.program(
            device,
            "lens_blur",
            &crate::lens_blur_wgsl::shader(),
            &layout_entries,
            &names,
        );
        let buffer = |label: &'static str, size: u64, usage: wgpu::BufferUsages| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage,
                mapped_at_creation: false,
            })
        };
        let upload = |label: &'static str, bytes: &[u8], usage: wgpu::BufferUsages| {
            let b = buffer(
                label,
                bytes.len() as u64,
                usage | wgpu::BufferUsages::COPY_DST,
            );
            ctx.queue.write_buffer(&b, 0, bytes);
            b
        };
        let storage = wgpu::BufferUsages::STORAGE;
        let uniform = upload(
            "lens-blur-params",
            &self.uniform(img.width() as u32, img.height() as u32),
            wgpu::BufferUsages::UNIFORM,
        );
        let depth = upload("lens-blur-depth", &packed(&self.depth), storage);
        let alpha = upload(
            "lens-blur-alpha",
            &packed(self.alpha.as_deref().unwrap_or(&[])),
            storage,
        );
        let src = buffer("lens-blur-sources", 48 * self.cells, storage);
        let got = buffer("lens-blur-gathered", 32 * self.cells, storage);
        let bound = [
            img.buffer.as_entire_binding(),
            uniform.as_entire_binding(),
            depth.as_entire_binding(),
            alpha.as_entire_binding(),
            src.as_entire_binding(),
            got.as_entire_binding(),
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
            label: Some("lens-blur-bind"),
            layout: &layout,
            entries: &entries,
        });
        // ⚠ 64 threads a group, the groups split 2-D past the per-dimension limit; the shader
        // rebuilds the flat index from `num_workgroups`.
        let mut run = |name: &str, count: u64| {
            let idx = names.iter().position(|n| *n == name).expect("compiled");
            let groups = count.div_ceil(64).max(1) as u32;
            let gx = groups.min(65_535);
            pass.set_pipeline(&pipelines[idx]);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(gx, groups.div_ceil(gx), 1);
        };
        for (l, lv) in self.levels.iter().enumerate() {
            run(BUILD[l], lv[0] as u64 * lv[1] as u64);
        }
        for (l, lv) in self.levels.iter().enumerate() {
            run(GATHER[l], lv[0] as u64 * lv[1] as u64);
        }
        run(COMPOSITE, img.pixels() as u64);
    }

    fn cpu(&self) -> &dyn PipelineStage {
        &self.cpu
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GpuChain;
    use core_image::ImageBuf;
    use edit_model::{Bokeh, LensBlur, LensFocus};
    use pipeline::lens_blur::{Inputs, LensBlurStage};
    use pipeline::RasterSource;

    fn ctx() -> Option<Arc<GpuContext>> {
        match crate::shared() {
            Some(c) => Some(c.clone()),
            None => {
                eprintln!("SKIPPED: no GPU adapter on this machine");
                None
            }
        }
    }

    /// The subject disc, in the frame's normalised coordinates with the aspect corrected: its distance
    /// from the centre, in units of the frame's height.
    fn from_subject(u: f32, v: f32, aspect: f32) -> f32 {
        ((u - 0.4) * aspect).hypot(v - 0.45)
    }

    const SUBJECT_R: f32 = 0.17;
    const NEAR_FROM: f32 = 0.88;

    fn plane(w: usize, h: usize, f: impl Fn(f32, f32) -> f32) -> pipeline::brush::CoverageMap {
        pipeline::brush::CoverageMap {
            x0: 0,
            y0: 0,
            w,
            h,
            data: (0..w * h)
                .map(|i| {
                    let (u, v) = (
                        ((i % w) as f32 + 0.5) / w as f32,
                        ((i / w) as f32 + 0.5) / h as f32,
                    );
                    (f(u, v).clamp(0.0, 1.0) * 255.0).round() as u8
                })
                .collect(),
            outside: 0,
        }
    }

    /// The subject at 0.3, a near strip along the bottom at 0.05, the background ramping 0.4 → 1.0.
    struct Depth;
    impl RasterSource for Depth {
        fn coverage(&self, w: usize, h: usize) -> pipeline::brush::CoverageMap {
            let aspect = w as f32 / h as f32;
            plane(w, h, |u, v| {
                if from_subject(u, v, aspect) <= SUBJECT_R {
                    0.3
                } else if v >= NEAR_FROM {
                    0.05
                } else {
                    0.4 + 0.6 * u
                }
            })
        }
    }

    /// The subject's alpha: soft over its outer 0.02 — partial cover, the hair case — or nobody found.
    enum Alpha {
        Soft,
        Empty,
    }
    impl RasterSource for Alpha {
        fn coverage(&self, w: usize, h: usize) -> pipeline::brush::CoverageMap {
            let aspect = w as f32 / h as f32;
            plane(w, h, |u, v| match self {
                Alpha::Soft => (SUBJECT_R - from_subject(u, v, aspect)) / 0.02,
                Alpha::Empty => 0.0,
            })
        }
    }

    /// The scene: the subject's colour, a checked near strip, five 3 × 3 lights — two in the top
    /// corners, where the cat-eye's shift is largest — and a striped background.
    fn scene(w: usize, h: usize) -> ImageBuf {
        let aspect = w as f32 / h as f32;
        let lights = [
            (0.78, 0.2),
            (0.87, 0.36),
            (0.7, 0.55),
            (0.03, 0.05),
            (0.97, 0.05),
        ]
        .map(|(lx, ly): (f32, f32)| ((lx * w as f32) as usize, (ly * h as f32) as usize));
        let mut img = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let (u, v) = ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
                let px = if from_subject(u, v, aspect) <= SUBJECT_R {
                    [0.55, 0.18, 0.08]
                } else if v >= NEAR_FROM {
                    if (x / 6 + y / 6) % 2 == 0 {
                        [0.30, 0.32, 0.12]
                    } else {
                        [0.08, 0.10, 0.04]
                    }
                } else if lights
                    .iter()
                    .any(|&(lx, ly)| x.abs_diff(lx) <= 1 && y.abs_diff(ly) <= 1)
                {
                    [4.0, 3.6, 2.8]
                } else {
                    let stripe = if (x / 4) % 2 == 0 { 0.06 } else { 0.0 };
                    let g = 0.10 + 0.10 * v + stripe;
                    [g * 0.8, g * 0.9, g * 1.1]
                };
                img.set_pixel(x, y, px);
            }
        }
        img
    }

    fn lens_blur(bokeh: Bokeh, focus: LensFocus) -> LensBlur {
        LensBlur {
            on: true,
            amount: 100.0,
            focus,
            focal_range: 10.0,
            bokeh,
            boost: 60.0,
        }
    }

    fn inputs(focus: f32, alpha: Option<Alpha>) -> Inputs {
        Inputs {
            focus,
            depth: (Arc::new(Depth), 1),
            subject: alpha.map(|a| (Arc::new(a) as Arc<dyn RasterSource>, 2)),
        }
    }

    /// ⚠ Shape (c) first: the CPU render must differ from the input, or the comparison proves nothing.
    fn parity(name: &str, (w, h): (usize, usize), lb: LensBlur, i: Inputs) {
        let Some(ctx) = ctx() else { return };
        let img = scene(w, h);
        let cpu = LensBlurStage::new(
            &lb,
            i.focus,
            i.depth.0.clone(),
            i.subject.as_ref().map(|s| s.0.clone()),
        )
        .apply(img.clone());
        assert!(
            !crate::compare(&img, &cpu).is_exact(),
            "{name}: fixture check — the blur must change the picture"
        );
        let stage = GpuLensBlurStage::new(&lb, &i, &img, &ctx).expect("the twin builds");
        let gpu = GpuChain::new(ctx.clone(), vec![Box::new(stage)]).apply(img.clone());
        let d = crate::compare(&cpu, &gpu);
        assert!(
            d.worst_levels < 1.0,
            "{name}: worst {} levels over {} of {} samples",
            d.worst_levels,
            d.differing,
            d.total
        );
    }

    #[test]
    fn each_shape_matches_the_cpu() {
        for bokeh in [Bokeh::Circle, Bokeh::Blades, Bokeh::Ring, Bokeh::CatEye] {
            parity(
                &format!("{bokeh:?}"),
                (320, 200),
                lens_blur(bokeh, LensFocus::Subject),
                inputs(0.3, Some(Alpha::Soft)),
            );
        }
    }

    /// Point mode reads no subject: no sharp layer, the depth alone draws the edges.
    #[test]
    fn point_focus_matches_the_cpu() {
        parity(
            "Point",
            (320, 200),
            lens_blur(Bokeh::Circle, LensFocus::Point { point: [0.7, 0.4] }),
            inputs(0.7, None),
        );
    }

    /// Review Focus 2: Subject mode whose map found nobody — the subject path, alpha 0 everywhere.
    #[test]
    fn an_empty_subject_matches_the_cpu() {
        parity(
            "Subject, empty",
            (320, 200),
            lens_blur(Bokeh::Circle, LensFocus::Subject),
            inputs(0.3, Some(Alpha::Empty)),
        );
    }

    /// Review Focus 1: sides that no level's scale divides — the edge cells hold partial blocks.
    #[test]
    fn odd_sizes_match_the_cpu() {
        parity(
            "301 × 203",
            (301, 203),
            lens_blur(Bokeh::CatEye, LensFocus::Subject),
            inputs(0.3, Some(Alpha::Soft)),
        );
    }

    /// Review Focus 4: a frame needing two levels — their offsets and the composite across them.
    #[test]
    fn two_levels_match_the_cpu() {
        let lb = lens_blur(Bokeh::Circle, LensFocus::Subject);
        let r = pipeline::lens_blur::Params::new(&lb, 0.3, 640).radius_px;
        assert!(
            pipeline::lens_blur::level_of(r) >= 1,
            "fixture check: {r} px needs a second level"
        );
        parity("two levels", (640, 400), lb, inputs(0.3, Some(Alpha::Soft)));
    }

    /// Review Focus 5: each of the three allocations is checked against the device's limit.
    #[test]
    fn the_buffers_must_fit_the_device() {
        let cells = 1_000u64;
        let pixels = 30_000u64;
        assert!(fits(cells, pixels, 48 * cells));
        assert!(!fits(cells, pixels, 48 * cells - 1), "the sources");
        assert!(!fits(cells, 4 * 48 * cells + 1, 48 * cells), "the planes");
        assert!(fits(cells, pixels, u64::MAX));
    }

    #[test]
    fn declines_below_half_a_pixel() {
        let Some(ctx) = ctx() else { return };
        let img = scene(10, 8);
        let lb = lens_blur(Bokeh::Circle, LensFocus::Subject);
        assert!(pipeline::lens_blur::Params::new(&lb, 0.3, 10).radius_px < 0.5);
        assert!(GpuLensBlurStage::new(&lb, &inputs(0.3, Some(Alpha::Soft)), &img, &ctx).is_none());
    }

    #[test]
    fn the_stage_is_named_like_the_cpu_one() {
        let Some(ctx) = ctx() else { return };
        let img = scene(64, 40);
        let lb = lens_blur(Bokeh::Circle, LensFocus::Subject);
        let s = GpuLensBlurStage::new(&lb, &inputs(0.3, Some(Alpha::Soft)), &img, &ctx)
            .expect("builds");
        assert_eq!(s.name(), s.cpu().name());
    }
}
