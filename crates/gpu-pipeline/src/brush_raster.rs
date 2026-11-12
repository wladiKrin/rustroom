//! The brush coverage rasteriser on the GPU.
//!
//! ⚠⚠ **Not a `GpuStage`, and that was checked rather than assumed.** That trait's `encode` takes
//! a [`crate::GpuImage`] and produces an image in place, and its scratch is image-sized
//! single-channel f32 planes. This needs a variable-length dab buffer and produces a `u8` coverage
//! map — so forcing it into `GpuStage` would mean sizing scratch by the image for data that is not
//! image-shaped. That is the same reasoning `GpuStage` itself rests on against `PipelineStage`.
//!
//! ⚠ It is also not a [`crate::dispatch::Dispatch`] client: that type is `workgroup_size(64)` over
//! pixels with bindings fixed by convention, and this needs one workgroup per 16×16 tile.
//! `sharpen` is the existing precedent for encoding a dispatch by hand.

use crate::{brush_wgsl, GpuContext};
use pipeline::brush::{BaseRef, CoverageMap, DabPlan};
use std::sync::Arc;

/// One dab as the shader sees it. 48 bytes, and the layout must match `Dab` in the WGSL.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GpuDab {
    /// x, y, r, hard
    pos: [f32; 4],
    /// Oklab reference in xyz, the gate ceiling in w. ⚠ A **negative** ceiling means "no gate" —
    /// the sentinel exists because WGSL has no `Option` and 0.0 is a legitimate ceiling.
    gate: [f32; 4],
    stroke: u32,
    _pad: [u32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GpuStroke {
    flow: f32,
    erase: u32,
    _pad: [u32; 2],
}

/// ⚠ 48 bytes, a multiple of 16, which a uniform binding requires.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    x0: u32,
    y0: u32,
    w: u32,
    h: u32,
    img_w: u32,
    img_h: u32,
    dab_count: u32,
    invert: u32,
    has_base: u32,
    tiles_x: u32,
    /// The frame coordinate of the base plane's `(0, 0)`. ⚠ Zero for a whole-frame render, which
    /// is every fixture plan 1 wrote — see the shader for why that made them blind to this.
    base_x0: u32,
    base_y0: u32,
    _pad: [u32; 2],
}

pub struct GpuBrushRaster {
    ctx: Arc<GpuContext>,
    /// ⚠ Injectable **only** so the overflow path is reachable at all: on the real brush the
    /// busiest 16×16 tile holds **229** dabs against [`brush_wgsl::MAX_TILE_DABS`], so nothing
    /// realistic comes near it, and *an unreachable guard is an untested one.*
    ///
    /// ⚠⚠ **An earlier version of this comment wrote BOTH numbers out as literals and both went
    /// stale** — "averages 22 dabs per tile against a capacity of 4095". The 22 was estimated from
    /// average dab area and is wrong by **10×**; the capacity predates the sweep that set it. The
    /// 229 is measured, by [`Self::rasterize_with_occupancy`], and the capacity is **named rather
    /// than repeated**, which is what stops that half going stale a second time. The constant's
    /// own doc carries the sweep and the rule the 22 broke: **an average is not a capacity.**
    capacity: u32,
    count_tests: bool,
}

impl GpuBrushRaster {
    pub fn new(ctx: &Arc<GpuContext>) -> Self {
        Self {
            ctx: ctx.clone(),
            // ⚠ M10.14g: sized from this device's workgroup storage, never one GPU's measurement.
            capacity: brush_wgsl::capacity_for(
                ctx.device.limits().max_compute_workgroup_storage_size,
            ),
            count_tests: false,
        }
    }

    /// Rasterise a planned dab list. `None` on any GPU failure at all.
    ///
    /// ⚠ `base` is `(&[f32], w, h)` rather than `&dyn BasePixels`: the shader needs the **whole
    /// plane** as contiguous bytes, and `BasePixels` is a per-pixel accessor that cannot be
    /// uploaded. Plan 2 hands it `image.data()`, which is the buffer M8.3j already uploads.
    ///
    /// ⚠ A plan that gates with **no base** rasterises positionally — the documented degradation
    /// `rasterize_in` already has (the CLI, and the tint before a frame lands), not an error.
    pub fn rasterize(
        &self,
        plan: &DabPlan,
        invert: bool,
        base: Option<BaseRef<'_>>,
    ) -> Option<CoverageMap> {
        self.rasterize_with_occupancy(plan, invert, base)
            .map(|(m, _)| m)
    }

    /// As [`Self::rasterize`], also returning **the largest number of dabs any one 16×16 tile
    /// held** — the measurement that justifies [`brush_wgsl::MAX_TILE_DABS`].
    ///
    /// ⚠ Occupancy only, deliberately. It is one `atomicMax` per workgroup, so it is free; the
    /// gate/dab-test counters are per **pixel** and are behind a compile-time flag for that reason.
    pub fn rasterize_with_occupancy(
        &self,
        plan: &DabPlan,
        invert: bool,
        base: Option<BaseRef<'_>>,
    ) -> Option<(CoverageMap, u32)> {
        self.run(plan, invert, base).map(|(m, _, _, occ)| (m, occ))
    }

    /// As [`Self::rasterize`], also returning `(gate evaluations, dab tests)`.
    ///
    /// ⚠⚠ **A COUNTER, not a flag, and it counts EVENTS rather than describing state.** Dropping
    /// the short-circuit before the gate is *correct* and merely slower, so nothing in the output
    /// can distinguish the two and only an event count can — verified in Task 4, where that exact
    /// sabotage left all eight equivalence tests green. The `PARALLEL_STROKES` lesson, and the
    /// reason `Ran { bytes }` exists: **an observable describing the current state cannot see its
    /// own absence.**
    #[cfg(test)]
    fn rasterize_counting(
        &self,
        plan: &DabPlan,
        invert: bool,
        base: Option<BaseRef<'_>>,
    ) -> Option<(CoverageMap, u32, u32)> {
        Self {
            ctx: self.ctx.clone(),
            capacity: self.capacity,
            count_tests: true,
        }
        .run(plan, invert, base)
        .map(|(m, g, d, _)| (m, g, d))
    }

    /// A rasteriser whose tile list holds `capacity` dabs, so the overflow path is reachable.
    ///
    /// ⚠ Injection is the **only** way to reach it: on the real brush the busiest tile holds 229
    /// dabs against [`brush_wgsl::MAX_TILE_DABS`], so nothing realistic comes near. *An
    /// unreachable guard is an untested one* — the correction `visualize`'s tool gate and the
    /// lens-correction identity fill each needed. See [`Self::capacity`] for why neither number is
    /// written out as a literal here any more.
    #[cfg(test)]
    fn with_capacity(ctx: &Arc<GpuContext>, capacity: u32) -> Self {
        Self {
            ctx: ctx.clone(),
            capacity,
            count_tests: false,
        }
    }

    fn run(
        &self,
        plan: &DabPlan,
        invert: bool,
        base: Option<BaseRef<'_>>,
    ) -> Option<(CoverageMap, u32, u32, u32)> {
        let ctx = &self.ctx;
        let device = &ctx.device;
        let (w, h) = (plan.w, plan.h);
        if w == 0 || h == 0 {
            return None;
        }

        // ── flatten the plan ──────────────────────────────────────────────────────────
        //
        // ⚠ Stroke-major, which is what makes an ascending dab index *be* stroke order — the
        // property the shader's tile list depends on.
        let mut dabs: Vec<GpuDab> = Vec::new();
        let mut strokes: Vec<GpuStroke> = Vec::with_capacity(plan.strokes.len());
        for (si, s) in plan.strokes.iter().enumerate() {
            strokes.push(GpuStroke {
                flow: s.flow,
                erase: u32::from(s.erase),
                _pad: [0; 2],
            });
            // ⚠ A stroke with no dabs or no flow contributes nothing, exactly as the CPU kernel's
            // `continue` does — so its dabs are simply not emitted and its `StrokeInfo` is never
            // referenced. Emitting them would be harmless but would inflate the tile lists.
            if s.dabs.is_empty() || s.flow <= 0.0 {
                continue;
            }
            for d in &s.dabs {
                let (gate, ceiling) = match d.gate {
                    Some((r, c)) => (r, c),
                    // ⚠ A negative ceiling is the "no gate" sentinel; see `GpuDab::gate`.
                    None => ([0.0; 3], -1.0),
                };
                dabs.push(GpuDab {
                    pos: [d.x, d.y, d.r, d.hard],
                    gate: [gate[0], gate[1], gate[2], ceiling],
                    stroke: si as u32,
                    _pad: [0; 3],
                });
            }
        }
        if dabs.is_empty() {
            return None;
        }

        let tiles_x = (w as u32).div_ceil(brush_wgsl::TILE);
        let tiles_y = (h as u32).div_ceil(brush_wgsl::TILE);
        let tiles = tiles_x.checked_mul(tiles_y)?;

        // ── buffers ───────────────────────────────────────────────────────────────────
        //
        // ⚠ `.max(16)` throughout because wgpu refuses a zero-sized buffer — the same guard
        // `Dispatch` and `GpuImage::scratch` carry.
        let storage = |label: &'static str, bytes: &[u8]| -> wgpu::Buffer {
            let b = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: (bytes.len() as u64).max(16),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            if !bytes.is_empty() {
                ctx.queue.write_buffer(&b, 0, bytes);
            }
            b
        };

        let dab_buf = storage("brush-dabs", bytemuck::cast_slice(&dabs));
        let stroke_buf = storage("brush-strokes", bytemuck::cast_slice(&strokes));

        // ⚠ The base plane is uploaded only when something actually gates. `plan.gating_rows`
        // says whether it does; a non-gating brush pays nothing at all, which is the same
        // property the CPU's lazily-built `base_lab` has.
        //
        // ⚠⚠ **Through the CACHED STAGING buffer, not `queue.write_buffer` — and the first version
        // of this got it wrong.** `GpuChain::run_with` records the measurement: 1.86 ms against
        // 3.90 for 19.6 MB, a **2.1x** difference, and it is `mapped_at_creation` that is expensive
        // rather than the allocation, so the pool matters. The base plane here is the **whole
        // frame** — 78 MB at rung 3200 — so this is the single largest item in the round trip.
        // Measured on the real brush: 15.24 ms with no base at all against 38.97 with one.
        //
        // ⚠ `slice(..bytes)` rather than `slice(..)`: a pooled buffer may be **larger** than this
        // render needs, and the copy is given `bytes` for the same reason.
        let mut staging = None;
        let mut d_stage = std::time::Duration::ZERO;
        // ⚠⚠ **Carried beside the binding rather than produced by the match, because the no-gate
        // arm has no image to produce** — and a `GpuImage` at a dummy size would be a lie the
        // dimension check in `SharedPlane::take` is there to catch.
        // ⚠⚠ **Reported, because `base_mb` alone became ambiguous the moment a base could be
        // TAKEN.** It is derived from `staging`, so a taken base and no base at all both print
        // `base_mb=0.0` — and a measurement that cannot tell "the memcpy is gone" from "there was
        // never a gate" is exactly how this project has mispriced three things.
        let mut base_src = "none";
        let mut resident: Option<crate::GpuImage> = None;
        let (base_buf, img_w, img_h, bx0, by0, has_base) = match (base, plan.gating_rows) {
            (Some(b), Some(_)) => {
                let plane = b.plane;
                let bytes = std::mem::size_of_val(plane) as u64;
                // ⚠⚠ **A `GpuImage`, not a bare buffer, and that is the whole of M8.3l.** Its
                // layout is `w*h*3` f32 — bit for bit what `ImageBuf::data()` holds and what a
                // chain uploads — and its usages are a superset of what the gate's base binding
                // needs. So the very allocation the gate reads from can be handed on to this
                // mask's first sub-stage instead of a second copy of the same bytes.
                //
                // ⚠ Declining on a length mismatch rather than assuming one: a `BaseRef` whose
                // plane disagrees with its own dimensions is a programming error, and every path
                // in this function fails toward the CPU.
                if plane.len() != b.w.checked_mul(b.h)?.checked_mul(3)? {
                    return None;
                }
                // ⚠⚠ **Take a resident plane before staging one.** When a `GpuChain` ran
                // immediately before this mask, the bytes it downloaded to make `plane` are still
                // on the device — `GpuChain::offer_output` deposits that buffer. Taking it deletes
                // the host memcpy below, measured at **2.04-2.23 ms** for a 78 MB frame and paid on
                // **every frame of a global drag**, because such a drag invalidates the coverage
                // cache's key and every frame is a miss.
                //
                // ⚠ A miss here is the ORDINARY case, not a failure: the mask may be the tail's
                // first stage, or `worth_a_chain` may have refused the run — measured, a light edit
                // set of exposure and contrast builds no chain at all. Staging then is correct.
                //
                // ⚠⚠ `take` matches on DIMENSIONS and on the depositing THREAD, and that is the
                // whole safety argument: a plane from another frame at another size cannot be
                // taken, and `end_mask` clears one that nobody took. A plane taken by the wrong
                // frame renders a **plausible photograph, not an error**.
                let img = match ctx.shared_plane().take(b.w, b.h) {
                    Some(resident_base) => {
                        base_src = "taken";
                        resident_base
                    }
                    None => {
                        base_src = "staged";
                        let img = crate::GpuImage::new(ctx, b.w, b.h)?;
                        let t_stage = std::time::Instant::now();
                        let st = ctx.cache.take_staging(device, bytes)?;
                        st.slice(..bytes)
                            .get_mapped_range_mut()
                            .copy_from_slice(bytemuck::cast_slice(plane));
                        st.unmap();
                        d_stage = t_stage.elapsed();
                        staging = Some((st, bytes));
                        img
                    }
                };
                let buf = img.buffer.clone();
                resident = Some(img);
                (buf, b.w as u32, b.h as u32, b.x0 as u32, b.y0 as u32, 1u32)
            }
            _ => (storage("brush-base", &[]), 1, 1, 0, 0, 0u32),
        };

        let cov_bytes = (w * h * 4) as u64;
        let cov_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("brush-cov"),
            size: cov_bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let counters = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("brush-counters"),
            size: 16,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        ctx.queue.write_buffer(&counters, 0, &[0u8; 16]);

        let params = Params {
            x0: plan.x0 as u32,
            y0: plan.y0 as u32,
            w: w as u32,
            h: h as u32,
            img_w,
            img_h,
            dab_count: dabs.len() as u32,
            invert: u32::from(invert),
            has_base,
            tiles_x,
            base_x0: bx0,
            base_y0: by0,
            _pad: [0; 2],
        };
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("brush-params"),
            size: std::mem::size_of::<Params>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        ctx.queue
            .write_buffer(&uniform, 0, bytemuck::bytes_of(&params));

        // ── the program, from the cache ───────────────────────────────────────────────
        //
        // ⚠ Keyed on the shader TEXT, so the injected capacity and the test-counter flag each
        // produce their own pipeline rather than serving a stale one — `cache::program`'s rule,
        // and the reason it hashes the source rather than the label.
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
        let layout_entries = [
            entry(0, wgpu::BufferBindingType::Storage { read_only: true }),
            entry(1, wgpu::BufferBindingType::Storage { read_only: true }),
            entry(2, wgpu::BufferBindingType::Storage { read_only: true }),
            entry(3, wgpu::BufferBindingType::Storage { read_only: false }),
            entry(4, wgpu::BufferBindingType::Uniform),
            entry(5, wgpu::BufferBindingType::Storage { read_only: false }),
            entry(6, wgpu::BufferBindingType::Storage { read_only: false }),
        ];
        let shader = brush_wgsl::source(self.capacity, self.count_tests);
        let (layout, pipelines) = ctx.cache.program(
            device,
            "brush-raster",
            &shader,
            &layout_entries,
            &["main", "pack"],
        );
        // M10.14g: the bytes the readback carries — the map four samples to a word.
        let words = (w * h).div_ceil(4).max(1);
        let packed_bytes = (words * 4) as u64;
        let packed = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("brush-cov-packed"),
            size: packed_bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("brush-raster"),
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: dab_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: stroke_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: base_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: cov_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: uniform.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: counters.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: packed.as_entire_binding(),
                },
            ],
        });

        // ⚠ One workgroup per TILE, laid out 1-D and unpacked in the shader from `tiles_x`. The
        // 65535-per-dimension limit applies here too: a 26 Mpx box is ~101,000 tiles, so the
        // second dimension is not optional.
        let gx = tiles.clamp(1, 65535);
        let gy = tiles.div_ceil(gx);

        let t_submit = std::time::Instant::now();
        let mut enc = device.create_command_encoder(&Default::default());
        if let Some((st, bytes)) = &staging {
            enc.copy_buffer_to_buffer(st, 0, &base_buf, 0, *bytes);
        }
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipelines[0]);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(gx, gy, 1);
            // ⚠ In the same pass, after the raster — the in-pass barrier `GpuChain` measures.
            let groups = words.div_ceil(256);
            let px = groups.clamp(1, 65535);
            pass.set_pipeline(&pipelines[1]);
            pass.dispatch_workgroups(px as u32, groups.div_ceil(px) as u32, 1);
        }

        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("brush-readback"),
            size: packed_bytes,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        enc.copy_buffer_to_buffer(&packed, 0, &readback, 0, packed_bytes);
        let counter_read = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("brush-counter-readback"),
            size: 16,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        enc.copy_buffer_to_buffer(&counters, 0, &counter_read, 0, 16);
        ctx.queue.submit(Some(enc.finish()));

        // ⚠ The readback idiom is `GpuChain::run_with`'s, deliberately reused rather than
        // reinvented: `map_async`, then `poll(Wait)`, then receive.
        let slice = readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let cslice = counter_read.slice(..);
        let (ctx2, crx) = std::sync::mpsc::channel();
        cslice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = ctx2.send(r);
        });
        let _ = device.poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        });
        rx.recv().ok()?.ok()?;
        crx.recv().ok()?.ok()?;

        let d_gpu = t_submit.elapsed();
        let t_read = std::time::Instant::now();
        // ⚠⚠ Narrowed straight from the mapped range. This used to `to_vec()` the whole readback
        // into a `Vec<u32>` — **27.35 MB at rung 3200** — whose only use was the narrowing a few
        // lines below. The mapped range is borrowed until `unmap`, so the narrowing has to happen
        // here rather than after it, which is the reason the old code copied at all.
        // ⚠ M10.14g: and since then the GPU narrows — the mapped bytes ARE the map, a quarter of the old wire.
        let mapped = slice.get_mapped_range();
        let data: Vec<u8> = mapped[..w * h].to_vec();
        drop(mapped);
        let counts: Vec<u32> = bytemuck::cast_slice(&cslice.get_mapped_range()).to_vec();
        readback.unmap();
        counter_read.unmap();
        // ⚠ Returned only **here**, after `poll(Wait)` has confirmed the submission finished — the
        // GPU reads this buffer during the submit, so handing it back any earlier would let the
        // next render map a buffer still being read. `GpuChain::run_with`'s rule, verbatim.
        let base_mb = staging.as_ref().map_or(0.0, |(_, b)| *b as f64 / 1.0e6);
        if let Some((st, _)) = staging {
            ctx.cache.put_staging(st);
        }
        // ⚠⚠ **Deposited only HERE, after `poll(Wait)` has confirmed the submission finished** —
        // the same rule, and for the same reason, that governs when the staging buffer goes back
        // to the pool one line above. The GPU reads this buffer during the submit, so handing it
        // on any earlier would let the sub-stage chain mutate a plane still being read.
        //
        // ⚠ Whoever does not take it simply drops it at this mask's `end_mask`; nothing downstream
        // is obliged to want it, which is what keeps this an offer rather than a contract.
        if let Some(img) = resident {
            ctx.shared_plane().deposit(img.width(), img.height(), img);
        }

        // ⚠ Coverage crosses the wire as one `u32` per sample, and that is DELIBERATE: packing
        // four samples into one word in the shader would mean four threads read-modify-writing the
        // same address — a race. See the shader. Narrowing it on the CPU is the cost of that
        // choice; ⚠⚠ it is a considered trade rather than an oversight, and shrinking the wire
        // needs a restructured dispatch (one thread per four samples) or a second pack pass.
        let d_read = t_read.elapsed();

        // ⚠⚠ Kept, the `RUSTROOM_DEBUG_MASK` / `RUSTROOM_DEBUG_BRUSH` precedent. The split it
        // reports — staging memcpy against everything the submit covers against the readback — is
        // what says whether sharing an already-resident buffer is worth a mechanism, and this
        // project has been wrong about exactly that kind of subtraction three times.
        if std::env::var("RUSTROOM_DEBUG_MASK").is_ok() {
            eprintln!(
                "BRUSHGPU box={}x{} dabs={} base={base_src}/{:.1}MB stage={:.2} gpu={:.2} \
                 read={:.2}",
                w,
                h,
                dabs.len(),
                base_mb,
                d_stage.as_secs_f64() * 1e3,
                d_gpu.as_secs_f64() * 1e3,
                d_read.as_secs_f64() * 1e3,
            );
        }
        Some((
            CoverageMap {
                x0: plan.x0,
                y0: plan.y0,
                w,
                h,
                data,
                outside: plan.outside,
            },
            counts[0],
            counts[1],
            counts[2],
        ))
    }
}

/// The smallest box worth a round trip, in pixels.
///
/// ⚠⚠ **SWEPT, AND THE STARTING VALUE WAS TOO LOW BY ~8x.** It began at
/// `brush::raster::SEQUENTIAL_FLOOR_PX`'s 64 Ki because that constant answers the same question on
/// the CPU side — and measured, at 126,504 px the GPU is **2.3x slower**, so that floor would have
/// handed small brushes to a path that loses. Measured by `brush_raster_probe`'s box-size sweep,
/// Apple M2, two runs, best of 5 warm rounds each:
///
/// | box px | CPU | GPU | ratio |
/// |---|---|---|---|
/// | 2,176 | 0.05 ms | 1.6 | 0.03x |
/// | 8,064 | 0.19 | 1.6 | 0.12x |
/// | 31,752 | 0.81 | 1.6 | 0.48x |
/// | 126,504 | 0.74 | 1.7 | 0.43x |
/// | 440,860 | 2.67 | 1.8-3.4 | 0.80-1.50x |
/// | 1,759,688 | 9.1 | 6.9-8.5 | 1.05-1.36x |
///
/// ⚠⚠ **The GPU has a ~1.6 ms floor whatever the box size** — M8.3g decomposed that as a **1.28 ms
/// fixed download cost no size reduction touches**, plus the dispatch. So below a few hundred
/// thousand pixels the round trip simply cannot be repaid, and the curve is a floor rather than a
/// slope.
///
/// ⚠ **512 Ki sits just above the ambiguous 440,860 point**, where two runs straddled parity
/// (0.80x and 1.50x). Refusing there is the trait's own rule — *fail toward the CPU* — because a
/// wrong answer is milliseconds and never pixels.
///
/// ⚠⚠ **Measured on a NON-GATING brush, which is the GPU's weakest case, so this is conservative
/// for a gating one.** An auto-masked brush costs the CPU an Oklab conversion per pixel — the real
/// brush measures ~20 ns/px against this sweep's ~5 — so its crossover is several times lower and
/// this floor refuses some gating brushes the GPU would win. **A per-gating floor is a recorded
/// follow-up, not an oversight**; the case that matters here (3.05 Mpx) is 6x above this either
/// way.
const MIN_GPU_BOX_PX: usize = 512 * 1024;

/// ⚠ The trait lives in `pipeline` and is implemented here — the direction that keeps wgpu out of
/// the CLI, `export` and all 63 goldens.
impl pipeline::CoverageRasterizer for GpuBrushRaster {
    /// ⚠⚠ **Guard 2 of the shared upload, and without it a cached-coverage frame renders the
    /// PREVIOUS frame's pixels.** Such a frame rasterises nothing, so it deposits nothing — and a
    /// sub-stage chain finding the last apply's plane still resident would take it, producing a
    /// plausible photograph rather than an error. Clearing as every apply *ends* bounds a
    /// deposit's life to the mask that made it.
    ///
    /// ⚠⚠ It also bounds the **incoming** hand-off, which is the one the end-placement exists for:
    /// a preceding `GpuChain` deposits its output for this gate to take, and a gate that declines
    /// (a box under the floor, or no gating rows at all) must not leave that plane for the *next*
    /// mask — whose base is this mask's output, not its input. Same dimensions, same thread, a
    /// plausible photograph.
    fn end_mask(&self) {
        self.ctx.shared_plane().clear();
    }

    fn rasterize(
        &self,
        plan: &DabPlan,
        invert: bool,
        base: Option<BaseRef<'_>>,
    ) -> Option<CoverageMap> {
        // ⚠⚠ **Fails toward the CPU, which is the trait's own rule.** A tiny box cannot repay a
        // round trip: the coverage download carries a fixed ~1.3 ms no size reduction touches
        // (M8.3g's decomposition) on top of 4 bytes a pixel.
        if plan.w.checked_mul(plan.h)? < MIN_GPU_BOX_PX {
            return None;
        }
        // ⚠ Guarded for `GpuChain::apply`'s reason: this runs on the **render worker**, and a
        // `wgpu` validation error is reported by panicking rather than by returning. Declining
        // costs the CPU rasteriser; panicking used to cost every remaining frame of the session.
        crate::guarded(|| GpuBrushRaster::rasterize(self, plan, invert, base))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edit_model::{Brush, BrushStroke, Geometry};

    fn ctx() -> Option<Arc<GpuContext>> {
        match crate::shared() {
            Some(c) => Some(c.clone()),
            None => {
                eprintln!("SKIPPED: no GPU adapter on this machine");
                None
            }
        }
    }

    /// One plain stroke, no gate, no erase — the smallest case that exercises the tile scan, the
    /// shared list, `dab_alpha` and the composite.
    fn one_stroke() -> Brush {
        Brush {
            strokes: vec![BrushStroke {
                points: (0..20).map(|i| [0.2 + i as f32 * 0.02, 0.5]).collect(),
                radius: 0.03,
                hardness: 40.0,
                flow: 100.0,
                auto_mask: false,
                tolerance: 25.0,
                ..Default::default()
            }],
            invert: false,
        }
    }

    /// ⚠⚠ **The whole-map comparison against the CPU rasteriser, in 8-bit levels.** Coverage *is*
    /// a `u8`, so "within 1 level" is a statement about the stored value rather than about a
    /// rendering of it — which is why this is far stricter than a pixel-pipeline equivalence test.
    ///
    /// ⚠ M8.3i's frozen-copy **bit-identity** standard deliberately cannot hold here: the gate
    /// needs `cbrt` and a matrix on the GPU, and M8.3a established that GPU and CPU float differ
    /// in the last bits (subnormals flush, transcendentals differ). A real change of standard,
    /// argued in the spec's §7 rather than quietly adopted.
    fn assert_agrees(gpu: &CoverageMap, cpu: &CoverageMap) {
        assert_eq!(
            (gpu.x0, gpu.y0, gpu.w, gpu.h),
            (cpu.x0, cpu.y0, cpu.w, cpu.h),
            "the boxes must agree"
        );
        assert_eq!(gpu.outside, cpu.outside, "outside must agree");
        let mut worst = 0i32;
        let mut differing = 0usize;
        let mut at = (0usize, 0usize);
        for y in 0..cpu.h {
            for x in 0..cpu.w {
                let i = y * cpu.w + x;
                let d = (i32::from(gpu.data[i]) - i32::from(cpu.data[i])).abs();
                if d > 0 {
                    differing += 1;
                }
                if d > worst {
                    worst = d;
                    at = (x, y);
                }
            }
        }
        assert!(
            worst <= 1,
            "worst {worst} levels at {at:?}, {differing} of {} samples differ",
            cpu.w * cpu.h
        );
    }

    /// A soft, partial-flow stroke along `y`, so two of them overlap at partial coverage.
    fn soft(y: f32) -> BrushStroke {
        BrushStroke {
            points: (0..12).map(|i| [0.25 + i as f32 * 0.03, y]).collect(),
            radius: 0.05,
            hardness: 0.0,
            flow: 45.0,
            auto_mask: false,
            tolerance: 25.0,
            ..Default::default()
        }
    }

    /// ⚠⚠ **The two-level combine, and the only fixture that can see it flattened.** Two strokes
    /// overlapping at partial coverage: a flat `max` over all dabs agrees with the composite at 0
    /// and 1 and differs everywhere in between, so a kernel that took one flat `max` passes every
    /// single-stroke test and fails this one.
    #[test]
    fn two_overlapping_strokes_composite_rather_than_max() {
        let Some(ctx) = ctx() else { return };
        let b = Brush {
            strokes: vec![soft(0.48), soft(0.52)],
            invert: false,
        };
        let g = Geometry::default();
        let cpu = pipeline::brush::rasterize(&b, &g, 400, 300);
        // ⚠ The fixture check that makes the test mean anything: there must be samples strictly
        // between 0 and 255, or `max` and the composite would agree everywhere.
        assert!(
            cpu.data.iter().any(|&v| v > 10 && v < 245),
            "fixture check: the overlap must be at PARTIAL coverage"
        );
        let plan = pipeline::brush::plan_dabs(&b, &g, 400, 300, None, None).unwrap();
        let gpu = GpuBrushRaster::new(&ctx)
            .rasterize(&plan, b.invert, None)
            .unwrap();
        assert_agrees(&gpu, &cpu);
    }

    /// ⚠⚠ **The floor is reachable only through the TRAIT, and nothing else in this file uses it.**
    /// Every other test calls the inherent `rasterize`, which has no floor — so without this the
    /// guard would ship untested, and *an unreachable guard is an untested one*.
    #[test]
    fn a_box_below_the_floor_is_declined_so_the_cpu_keeps_it() {
        let Some(ctx) = ctx() else { return };
        use pipeline::CoverageRasterizer;
        let tiny = Brush {
            strokes: vec![BrushStroke {
                decode: false,
                points: vec![[0.5, 0.5]],
                radius: 0.01,
                hardness: 50.0,
                flow: 100.0,
                auto_mask: false,
                tolerance: 25.0,
                erase: false,
            }],
            invert: false,
        };
        let g = Geometry::default();
        let plan = pipeline::brush::plan_dabs(&tiny, &g, 400, 300, None, None).unwrap();
        assert!(
            plan.w * plan.h < super::MIN_GPU_BOX_PX,
            "fixture check: the box must be below the floor, got {}",
            plan.w * plan.h
        );
        let r = GpuBrushRaster::new(&ctx);
        assert!(
            CoverageRasterizer::rasterize(&r, &plan, tiny.invert, None).is_none(),
            "a box below the floor must be declined"
        );
        // ⚠ And the inherent method still does it — the floor is the trait's policy, not a
        // limitation of the rasteriser, which is what lets the sweep above measure past it.
        assert!(
            r.rasterize(&plan, tiny.invert, None).is_some(),
            "the inherent rasterizer has no floor and must still work"
        );
    }

    /// The base seen the way `MaskStage::BaseView` sees it: frame coordinates in, region-local out.
    ///
    /// ⚠ Written to match `BaseView::at` exactly — `saturating_sub` then clamp — because that is
    /// the behaviour the shader has to agree with, not a re-derivation of what it ought to be.
    struct RegionBase<'a> {
        plane: &'a [f32],
        w: usize,
        h: usize,
        x0: usize,
        y0: usize,
    }
    impl pipeline::brush::BasePixels for RegionBase<'_> {
        fn at(&self, x: usize, y: usize) -> [f32; 3] {
            let lx = x.saturating_sub(self.x0).min(self.w - 1);
            let ly = y.saturating_sub(self.y0).min(self.h - 1);
            let i = (ly * self.w + lx) * 3;
            [self.plane[i], self.plane[i + 1], self.plane[i + 2]]
        }
    }

    /// ⚠⚠ **The guard plan 1 structurally could not have: every one of its fixtures is
    /// whole-frame, so `x0 == y0 == 0` and a shader ignoring the origin agrees perfectly.** A
    /// region render's buffer covers only part of the frame, and reading it without the offset
    /// gates the auto-mask against pixels from somewhere else — a plausible mask, not an error.
    #[test]
    fn a_region_base_is_read_from_its_own_origin() {
        let Some(ctx) = ctx() else { return };
        let (plane, fw, fh) = half_base(400, 300);
        // The region: right of and below the frame's origin, so BOTH offsets are exercised.
        let (rx0, ry0, rw, rh) = (120usize, 60usize, 240usize, 180usize);
        assert!(
            rx0 > 0 && ry0 > 0,
            "fixture check: the origin must be non-zero, or this is another whole-frame test"
        );
        let mut sub = vec![0.0f32; rw * rh * 3];
        for y in 0..rh {
            let src = ((ry0 + y) * fw + rx0) * 3;
            sub[y * rw * 3..(y + 1) * rw * 3].copy_from_slice(&plane[src..src + rw * 3]);
        }

        // ⚠ Along the boundary, for Task 4's reason: a stroke crossing it cuts nothing, because
        // each dab references its own centre and the within-stroke combine is a `max`.
        let b = Brush {
            strokes: vec![BrushStroke {
                decode: false,
                points: (0..16).map(|i| [0.47, 0.25 + i as f32 * 0.02]).collect(),
                radius: 0.06,
                hardness: 50.0,
                flow: 100.0,
                auto_mask: true,
                tolerance: 25.0,
                erase: false,
            }],
            invert: false,
        };
        let g = Geometry::default();
        let view = RegionBase {
            plane: &sub,
            w: rw,
            h: rh,
            x0: rx0,
            y0: ry0,
        };
        let clip = Some((rx0, ry0, rw, rh));
        let cpu = pipeline::brush::rasterize_in(&b, &g, fw, fh, clip, Some(&view));
        let plan = pipeline::brush::plan_dabs(&b, &g, fw, fh, clip, Some(&view)).unwrap();
        assert!(
            plan.gating_rows.is_some(),
            "fixture check: the plan must gate, or the origin is never read at all"
        );
        let gpu = GpuBrushRaster::new(&ctx)
            .rasterize(
                &plan,
                b.invert,
                Some(BaseRef {
                    plane: &sub,
                    w: rw,
                    h: rh,
                    x0: rx0,
                    y0: ry0,
                }),
            )
            .unwrap();
        assert_agrees(&gpu, &cpu);
    }

    /// ⚠⚠ **The overflow path must RE-SCAN, never clip.** A tile whose shared list fills falls
    /// back to walking the whole dab list per pixel — correct, slower, confined to that tile. A
    /// kernel that silently used the first `capacity` dabs would render a plausible-but-wrong
    /// mask, which is the failure mode this whole module is written against.
    #[test]
    fn a_tile_that_overflows_its_shared_list_still_agrees() {
        let Some(ctx) = ctx() else { return };
        let b = one_stroke();
        let g = Geometry::default();
        let cpu = pipeline::brush::rasterize(&b, &g, 400, 300);
        let plan = pipeline::brush::plan_dabs(&b, &g, 400, 300, None, None).unwrap();
        // ⚠ A capacity of 1 forces every tile the stroke touches down the overflow path, which is
        // the only way to reach it at all.
        let gpu = GpuBrushRaster::with_capacity(&ctx, 1)
            .rasterize(&plan, b.invert, None)
            .unwrap();
        assert_agrees(&gpu, &cpu);
    }

    /// ⚠⚠ **The only guard that can see the short-circuit dropped.** Task 4 confirmed by sabotage
    /// that moving the gate above it leaves every equivalence test green — it is *correct* and
    /// merely slower. So this counts **gate evaluations against dab tests**, which is an event
    /// count rather than a description of state.
    ///
    /// ⚠⚠ **The bound is 0.5, and it was SWEPT rather than chosen — the first attempt guessed a
    /// tenth and was simply wrong about this fixture.** Measured both ways on it:
    ///
    /// | | gate evaluations | share |
    /// |---|---|---|
    /// | **with** the short-circuit | 35,854 of 121,275 | **29.6%** |
    /// | **without** (gate moved above it) | 121,275 of 121,275 | **100.0%** |
    ///
    /// The without-case is exactly **all** of them by construction, so any bound in (0.30, 1.00)
    /// discriminates; 0.5 sits with ~1.7x margin below and 2x above. ⚠ A tenth was not a safer
    /// number, it was a **failing** one — and adjusting it upward until the test passed, without
    /// measuring the other state, would have left a bound nobody could defend.
    ///
    /// ⚠ Why only ~70% of gate evaluations are skipped rather than more: the skip's effectiveness
    /// depends on dab overlap and on the order dabs are visited, and this is one stroke. M8.3i's
    /// `gate_rows=1718 of 2019` is a **different** quantity — box rows that gate at all, not dab
    /// tests skipped — so the two are not comparable and should not be read as such.
    ///
    /// ⚠ A ratio rather than an absolute, so the fixture can change without the test going
    /// vacuous.
    #[test]
    fn the_short_circuit_keeps_the_gate_off_most_dabs() {
        let Some(ctx) = ctx() else { return };
        let (plane, bw, bh) = half_base(400, 300);
        let mut b = one_stroke();
        b.strokes[0].auto_mask = true;
        b.strokes[0].radius = 0.06;
        let g = Geometry::default();
        let base = SliceBase(&plane, bw);
        let plan = pipeline::brush::plan_dabs(&b, &g, bw, bh, None, Some(&base)).unwrap();
        let (_, gate_tests, dab_tests) = GpuBrushRaster::new(&ctx)
            .rasterize_counting(
                &plan,
                b.invert,
                Some(BaseRef {
                    plane: &plane,
                    w: bw,
                    h: bh,
                    x0: 0,
                    y0: 0,
                }),
            )
            .unwrap();
        assert!(dab_tests > 0, "fixture check: dabs must be tested at all");
        assert!(
            gate_tests > 0,
            "fixture check: the gate must run at all, or this measures nothing"
        );
        assert!(
            f64::from(gate_tests) < f64::from(dab_tests) * 0.5,
            "the gate ran on {gate_tests} of {dab_tests} dab tests — the short-circuit is gone"
        );
    }

    /// A base whose halves differ in Oklab, so the gate genuinely discriminates rather than
    /// passing everything.
    ///
    /// ⚠⚠ **Two near-NEUTRAL greys differing almost entirely in LIGHTNESS, and the first version
    /// of this fixture was wrong in the interesting direction.** It used a dark green against a
    /// bright magenta — which differ in both lightness and chroma, so it looked like the stronger
    /// choice. But the gate's ceiling at tolerance 25 is only **0.095**, and those two are so far
    /// apart that the **chroma distance alone already exceeds it**: dropping the lightness term
    /// left the gate fully off either way and the sabotage **passed**.
    ///
    /// ⚠ The discriminating fixture is the opposite of extreme. Two greys have a chroma distance
    /// near zero — below the gate's `lo` of 0.0475, so chroma alone would pass everything — while
    /// their lightness distance is far above the ceiling. So the term this fixture exists to guard
    /// is the only term that decides the answer. **"Most different" is not "the content under
    /// test"**, which is the same correction the dehaze probe's "haziest frames" needed.
    fn half_base(w: usize, h: usize) -> (Vec<f32>, usize, usize) {
        let mut v = vec![0.0f32; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let p = if x < w / 2 {
                    [0.18, 0.18, 0.18]
                } else {
                    [0.55, 0.55, 0.55]
                };
                let i = (y * w + x) * 3;
                v[i..i + 3].copy_from_slice(&p);
            }
        }
        (v, w, h)
    }

    /// The same plane seen through `BasePixels`, so the CPU and the GPU are handed identical data.
    struct SliceBase<'a>(&'a [f32], usize);
    impl pipeline::brush::BasePixels for SliceBase<'_> {
        fn at(&self, x: usize, y: usize) -> [f32; 3] {
            let i = (y * self.1 + x) * 3;
            [self.0[i], self.0[i + 1], self.0[i + 2]]
        }
    }

    /// ⚠⚠ **The gate, and the fixture check is what makes it a test.** A stroke crossing the
    /// colour boundary must be cut off at it, so the CPU map is first asserted to contain both
    /// painted and blocked samples — a gate that passed everything would agree trivially.
    #[test]
    fn an_auto_masked_stroke_agrees_with_the_cpu_gate() {
        let Some(ctx) = ctx() else { return };
        let (plane, bw, bh) = half_base(400, 300);
        let b = Brush {
            strokes: vec![BrushStroke {
                decode: false,
                // Starts in the dark half and crosses into the light one.
                // ⚠⚠ **The stroke runs ALONG the boundary, entirely inside the dark half — it
                // does not cross it, and that correction is the whole fixture.** A stroke crossing
                // the edge cuts nothing: each dab references the pixel under **its own centre**,
                // dabs are laid a quarter-radius apart, and the within-stroke combine is a `max` —
                // so on the far side of the edge there is always a nearby dab whose reference
                // matches, and it wins the max. Measured: a crossing stroke gave `cut = 0`.
                //
                // ⚠ Running parallel to the edge is also what the feature is *for*: every dab
                // references the dark side, so the part of each dab reaching across is blocked.
                // That is "paint along an edge and let it stop at the edge".
                points: (0..24).map(|i| [0.47, 0.2 + i as f32 * 0.025]).collect(),
                // ⚠ Wide enough to reach across: 0.06 of the 400 px long edge is 24 px, against a
                // boundary 12 px away, so half of each dab lands on the light side.
                radius: 0.06,
                hardness: 60.0,
                flow: 100.0,
                auto_mask: true,
                tolerance: 25.0,
                erase: false,
            }],
            invert: false,
        };
        let g = Geometry::default();
        let base = SliceBase(&plane, bw);
        let cpu = pipeline::brush::rasterize_in(&b, &g, bw, bh, None, Some(&base));

        // ⚠⚠ **The fixture check compares against the POSITIONAL raster, not against a count of
        // dark samples.** Counting `v < 20` over the whole box was the first version and it is
        // nearly vacuous: most of the box is background the stroke never touched, so it passes
        // whether or not the gate does anything. What this asserts instead is the property the
        // test is named for — that the gate **blocks coverage the same brush would otherwise
        // paint**.
        let mut ungated = b.clone();
        ungated.strokes[0].auto_mask = false;
        let positional = pipeline::brush::rasterize(&ungated, &g, bw, bh);
        let cut = (0..cpu.data.len())
            .filter(|&i| i32::from(positional.data[i]) - i32::from(cpu.data[i]) > 40)
            .count();
        let painted = cpu.data.iter().filter(|&&v| v > 200).count();
        assert!(
            painted > 100 && cut > 100,
            "fixture check: the gate must paint ({painted}) AND cut coverage the positional \
             raster lays down ({cut})"
        );
        let plan = pipeline::brush::plan_dabs(&b, &g, bw, bh, None, Some(&base)).unwrap();
        assert!(
            plan.gating_rows.is_some(),
            "fixture check: the plan must report a gating span"
        );
        let gpu = GpuBrushRaster::new(&ctx)
            .rasterize(
                &plan,
                b.invert,
                Some(BaseRef {
                    plane: &plane,
                    w: bw,
                    h: bh,
                    x0: 0,
                    y0: 0,
                }),
            )
            .unwrap();
        assert_agrees(&gpu, &cpu);
    }

    /// ⚠ A gating plan with **no base** paints positionally — the documented degradation
    /// `rasterize_in` already has (the CLI, and the tint before a frame lands), not an error. Both
    /// sides must degrade the same way, or the GPU path would quietly paint a different mask
    /// wherever the app has no base to offer.
    #[test]
    fn a_gating_plan_with_no_base_paints_positionally_on_both_sides() {
        let Some(ctx) = ctx() else { return };
        let mut b = one_stroke();
        b.strokes[0].auto_mask = true;
        let g = Geometry::default();
        let cpu = pipeline::brush::rasterize_in(&b, &g, 400, 300, None, None);
        assert!(
            cpu.data.iter().any(|&v| v > 200),
            "fixture check: with no base the stroke must paint at full strength"
        );
        let plan = pipeline::brush::plan_dabs(&b, &g, 400, 300, None, None).unwrap();
        let gpu = GpuBrushRaster::new(&ctx)
            .rasterize(&plan, b.invert, None)
            .unwrap();
        assert_agrees(&gpu, &cpu);
    }

    /// A paint stroke and a narrower erase stroke over it, both at partial flow.
    fn paint_then_erase() -> (BrushStroke, BrushStroke) {
        let paint = BrushStroke {
            decode: false,
            points: (0..14).map(|i| [0.2 + i as f32 * 0.04, 0.5]).collect(),
            radius: 0.06,
            hardness: 30.0,
            flow: 80.0,
            erase: false,
            auto_mask: false,
            tolerance: 25.0,
        };
        let mut rub = paint.clone();
        rub.erase = true;
        rub.flow = 70.0;
        rub.radius = 0.03;
        (paint, rub)
    }

    /// ⚠ Erase is `c * (1 - a)` — the `MaskOp::Subtract` rule, not `min`. The two agree at 0 and 1
    /// and diverge at every partial coverage, which is what this fixture's partial flow reaches.
    #[test]
    fn an_erase_stroke_over_a_paint_stroke_agrees() {
        let Some(ctx) = ctx() else { return };
        let (paint, rub) = paint_then_erase();
        let b = Brush {
            strokes: vec![paint, rub],
            invert: false,
        };
        let g = Geometry::default();
        let cpu = pipeline::brush::rasterize(&b, &g, 400, 300);
        assert!(
            cpu.data.iter().any(|&v| v > 10 && v < 245),
            "fixture check: the erase must leave PARTIAL coverage"
        );
        let plan = pipeline::brush::plan_dabs(&b, &g, 400, 300, None, None).unwrap();
        let gpu = GpuBrushRaster::new(&ctx)
            .rasterize(&plan, b.invert, None)
            .unwrap();
        assert_agrees(&gpu, &cpu);
    }

    /// ⚠⚠ **Stroke ORDER is observable once erase is in play** — erase-then-paint restores where
    /// paint-then-erase removes. A kernel that walked its tile list in an arbitrary order passes
    /// both tests above and fails this one, so the `assert_ne!` is what gives it teeth.
    #[test]
    fn swapping_two_strokes_changes_the_answer_and_both_agree() {
        let Some(ctx) = ctx() else { return };
        let (paint, rub) = paint_then_erase();
        let g = Geometry::default();
        let forward = Brush {
            strokes: vec![paint.clone(), rub.clone()],
            invert: false,
        };
        let reverse = Brush {
            strokes: vec![rub, paint],
            invert: false,
        };
        let a = pipeline::brush::rasterize(&forward, &g, 400, 300);
        let z = pipeline::brush::rasterize(&reverse, &g, 400, 300);
        assert_ne!(
            a.data, z.data,
            "fixture check: the order must be observable, or this test proves nothing"
        );
        let r = GpuBrushRaster::new(&ctx);
        for (b, cpu) in [(forward, a), (reverse, z)] {
            let plan = pipeline::brush::plan_dabs(&b, &g, 400, 300, None, None).unwrap();
            assert_agrees(&r.rasterize(&plan, b.invert, None).unwrap(), &cpu);
        }
    }

    /// ⚠ An inverted brush with **no strokes** covers EVERYTHING — `outside` is 255, not 0. That is
    /// a property of the caller rather than of the kernel (`plan_dabs` returns `None`), and it is
    /// asserted here because it was a real bug found in use on the CPU side: the inversion was
    /// confined to a rectangle around the strokes and grew as more was painted.
    #[test]
    fn an_inverted_brush_with_no_strokes_covers_everything() {
        let b = Brush {
            strokes: Vec::new(),
            invert: true,
        };
        let g = Geometry::default();
        let cpu = pipeline::brush::rasterize(&b, &g, 400, 300);
        assert_eq!(cpu.outside, 255);
        assert!(!cpu.is_inert());
        assert!(
            pipeline::brush::plan_dabs(&b, &g, 400, 300, None, None).is_none(),
            "no strokes must plan no dabs, so the GPU path is never entered"
        );
    }

    /// ⚠ And an inverted brush that DOES paint, so the kernel's own `1 - cover` is exercised.
    #[test]
    fn an_inverted_painted_brush_agrees() {
        let Some(ctx) = ctx() else { return };
        let mut b = one_stroke();
        b.invert = true;
        let g = Geometry::default();
        let cpu = pipeline::brush::rasterize(&b, &g, 400, 300);
        assert_eq!(
            cpu.outside, 255,
            "fixture check: inverted means outside 255"
        );
        let plan = pipeline::brush::plan_dabs(&b, &g, 400, 300, None, None).unwrap();
        let gpu = GpuBrushRaster::new(&ctx)
            .rasterize(&plan, b.invert, None)
            .unwrap();
        assert_agrees(&gpu, &cpu);
    }

    #[test]
    fn one_plain_stroke_agrees_with_the_cpu_rasterizer() {
        let Some(ctx) = ctx() else { return };
        let b = one_stroke();
        let g = Geometry::default();
        let cpu = pipeline::brush::rasterize(&b, &g, 400, 300);
        assert!(
            cpu.data.iter().any(|&v| v > 0),
            "fixture check: the CPU map must be non-empty"
        );
        let plan = pipeline::brush::plan_dabs(&b, &g, 400, 300, None, None)
            .expect("fixture check: the brush must produce dabs");
        let gpu = GpuBrushRaster::new(&ctx)
            .rasterize(&plan, b.invert, None)
            .expect("the GPU raster must succeed on an available adapter");
        assert_agrees(&gpu, &cpu);
    }

    /// M10.14g: the coverage crosses the wire four samples to a word — and a box whose sample count is not a multiple
    /// of four must still read back whole, its last word's spare bytes dropped rather than shifting the map.
    #[test]
    fn a_box_of_any_size_reads_back_whole_through_the_packed_wire() {
        let Some(ctx) = ctx() else { return };
        let b = one_stroke();
        let g = Geometry::default();
        let mut odd = 0;
        for (fw, fh) in [(400, 300), (401, 303), (397, 301), (403, 299)] {
            let cpu = pipeline::brush::rasterize(&b, &g, fw, fh);
            let plan = pipeline::brush::plan_dabs(&b, &g, fw, fh, None, None).expect("dabs");
            let gpu = GpuBrushRaster::new(&ctx)
                .rasterize(&plan, b.invert, None)
                .expect("the GPU raster");
            if !(cpu.w * cpu.h).is_multiple_of(4) {
                odd += 1;
            }
            assert_agrees(&gpu, &cpu);
        }
        assert!(
            odd > 0,
            "fixture check: some box must not be a multiple of four"
        );
    }

    /// M10.14g: the tile capacity is the device's — three workgroups resident, held to the measured plateau — and a
    /// 16 KB device, WebGPU's guaranteed minimum, keeps the value the sweep chose.
    #[test]
    fn the_tile_capacity_follows_the_devices_workgroup_storage() {
        assert_eq!(brush_wgsl::capacity_for(16384), brush_wgsl::MAX_TILE_DABS);
        assert_eq!(brush_wgsl::capacity_for(32768), 2048);
        assert_eq!(brush_wgsl::capacity_for(8192), 512);
        for limit in [16384u32, 24576, 32768, 49152, 65536] {
            let cap = brush_wgsl::capacity_for(limit);
            // The kernel's storage at this capacity, three times over, must fit — the residency the cliff was about.
            assert!(3 * (4 * cap + 1032) <= limit, "{limit}: {cap}");
        }
        if let Some(ctx) = ctx() {
            let want =
                brush_wgsl::capacity_for(ctx.device.limits().max_compute_workgroup_storage_size);
            assert_eq!(GpuBrushRaster::new(&ctx).capacity, want);
        }
    }
}
