//! The shared `wgpu` device.

/// A `wgpu` device and queue, shared by every GPU stage.
///
/// ⚠⚠ **`new` returns `Option`, never `Result`.** A machine with no usable adapter is not an
/// error anybody should have to handle — it means the CPU path, which must always work. The same
/// posture `SettingsStore::load` and the optional `Catalog` already take.
///
/// ⚠ `Send + Sync` matters rather than being incidental: rendering happens on the render worker
/// thread, not the UI thread. `wgpu`'s `Device` and `Queue` are both, so this is free.
#[derive(Debug)]
pub struct GpuContext {
    pub(crate) device: wgpu::Device,
    pub(crate) queue: wgpu::Queue,
    /// ⚠ The wgpu objects that outlive a render. Here rather than on `GpuChain` because
    /// `accelerate` rebuilds every chain and stage per render — see `cache.rs`.
    pub(crate) cache: crate::cache::GpuCache,
    /// The base plane a mask's gate uploaded, offered to that mask's first sub-stage chain.
    ///
    /// ⚠⚠ **Here because it is the only place both participants can reach.** `GpuBrushRaster` and
    /// `GpuChain` are built separately by `accelerate` and never meet; both hold this
    /// `Arc<GpuContext>`.
    ///
    /// ⚠⚠ **Deliberately NOT part of `cache`, whose rule is the opposite.** That one exists to
    /// outlive a render; this one must not outlive a single `MaskStage::apply`, which clears it at
    /// the end through `CoverageRasterizer::end_mask`. Folding it in would invite someone to
    /// treat a stale plane as a cache hit, and a stale plane renders the previous frame's pixels.
    shared_plane: crate::SharedPlane<crate::GpuImage>,
    adapter_name: String,
    backend: String,
    /// The largest resident image this device will accept, in bytes.
    ///
    /// ⚠⚠ **The `min` of two limits, and taking only one of them is how this was missed.** The
    /// image is allocated as a buffer *and* bound whole as a storage buffer, so
    /// `max_buffer_size` and `max_storage_buffer_binding_size` both apply — and under
    /// `Limits::default()` they are 256 MiB and **128 MiB**, so the binding is the tighter one.
    max_image_bytes: u64,
}

impl GpuContext {
    /// Acquire an adapter and device, or `None` if this machine has none we can use.
    ///
    /// ⚠⚠ **Expensive — see [`shared`], which is what callers should use.**
    pub fn new() -> Option<Self> {
        // ⚠⚠ **Not `Instance::default()`.** That uses `InstanceFlags::from_build_config()`, which
        // turns **validation on in every debug build** — and validation measured as roughly two
        // thirds of a 151 s adapter request here. This asks for the release-default flag and lets
        // `with_env()` put validation back for anyone who wants it (`WGPU_VALIDATION=1`).
        //
        // ⚠ `Backends::from_env()` first, so `WGPU_BACKEND=vulkan` still works for debugging.
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::from_env().unwrap_or(wgpu::Backends::PRIMARY),
            flags: wgpu::InstanceFlags::VALIDATION_INDIRECT_CALL.with_env(),
            memory_budget_thresholds: Default::default(),
            backend_options: Default::default(),
            display: Default::default(),
        });
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
                .ok()?;
        let info = adapter.get_info();
        // ⚠⚠ **The adapter's own limits, never `Limits::default()` — and asking for the default
        // was a live bug that killed the render worker.** `DeviceDescriptor::default()` requests
        // the downlevel-safe floor: `max_buffer_size` 256 MiB and `max_storage_buffer_binding_size`
        // **128 MiB**. A full-resolution X-T4 frame is 6246x4170 RGB f32 = **298 MiB**, so every
        // whole-image GPU render above ~11 Mpx failed `create_buffer` validation — which wgpu
        // reports through a **panic**, on the render worker thread, killing it for the rest of the
        // session. Symptom: at 1:1 the sliders stopped changing the picture while the *thumbnail*
        // kept updating, and switching photos fixed it because that builds a new engine.
        //
        // ⚠ Measured on this machine: the M2 offers **9093 MiB** per buffer and **4096 MiB** per
        // storage binding, so the ceiling we opted into was 32x below what was available.
        //
        // ⚠ `adapter.limits()` is by definition supported by that adapter, so this cannot make
        // `request_device` fail where the default would have succeeded.
        let limits = adapter.limits();
        let max_image_bytes = limits
            .max_buffer_size
            .min(limits.max_storage_buffer_binding_size);
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_limits: limits,
            ..Default::default()
        }))
        .ok()?;
        Some(Self {
            device,
            queue,
            cache: Default::default(),
            shared_plane: Default::default(),
            adapter_name: info.name,
            backend: format!("{:?}", info.backend),
            max_image_bytes,
        })
    }

    /// The slot a mask's gate parks its uploaded base plane in, for that mask's first sub-stage.
    ///
    /// ⚠ See the field: a deposit lives only for the duration of one `MaskStage::apply`.
    pub fn shared_plane(&self) -> &crate::SharedPlane<crate::GpuImage> {
        &self.shared_plane
    }

    /// The adapter's name, for the probe and for a future Settings readout.
    pub fn adapter_name(&self) -> &str {
        &self.adapter_name
    }

    /// The backend in use — `Metal`, `Vulkan`, `Dx12`, …
    pub fn backend(&self) -> &str {
        &self.backend
    }

    /// The largest resident image this device will accept, in bytes.
    ///
    /// ⚠⚠ **Consulted by [`crate::GpuImage::new`] and by nothing else, deliberately.** Every
    /// image-sized allocation in this crate is that one or is derived from it — scratch planes and
    /// the packed RGBA8 target are each exactly a **third** of the image, the readback is at most
    /// the image and is not storage-bound, and the staging buffer is image-sized and likewise not
    /// storage-bound. So one gate on the largest dominates them all, and a second would be a
    /// second rule to keep in step.
    pub fn max_image_bytes(&self) -> u64 {
        self.max_image_bytes
    }
}

/// The process-wide context, built at most once.
///
/// ⚠⚠ **Acquiring an adapter costs ~44–75 s in a DEBUG build and ~21 ms in release** — measured on
/// the same machine under the same load. The **second** call in the same process is **259 µs**, so
/// the cost is one-time process initialisation rather than per-call work, which is exactly what
/// makes sharing load-bearing rather than merely tidy: without it, five GPU tests would cost five
/// times a minute in the standard `cargo test --workspace` gate.
///
/// ⚠ **The root cause is NOT identified, and guessing at it would be worse than saying so.** Ruled
/// out by measurement: dependency optimisation (`opt-level = 2` on all dependencies changed
/// nothing), dependency `debug-assertions`, backend enumeration (`METAL` alone was no faster than
/// `PRIMARY`), machine contention (release stayed at 21 ms under the identical load, with another
/// process at 100% CPU), and Gatekeeper re-verification (the same binary run three times took
/// 62 s, 47 s, 44 s). Disabling debug validation is the one thing that *did* help — 151 s → 45 s —
/// and it is applied in [`GpuContext::new`].
///
/// ⚠ Practical consequence: **iterate on this crate with `cargo test --release -p gpu-pipeline`**.
pub fn shared() -> Option<&'static std::sync::Arc<GpuContext>> {
    static SHARED: std::sync::OnceLock<Option<std::sync::Arc<GpuContext>>> =
        std::sync::OnceLock::new();
    SHARED
        .get_or_init(|| GpuContext::new().map(std::sync::Arc::new))
        .as_ref()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_context_reports_the_adapter_it_found() {
        // ⚠ On this machine `None` is unreachable, and the test says so rather than pretending to
        // cover it. What it does prove is that a **headless** device works — the riskiest
        // assumption in the whole milestone, probed before the spec was written.
        let Some(ctx) = shared() else {
            eprintln!("SKIPPED a_context_reports_the_adapter_it_found: no GPU adapter here");
            return;
        };
        assert!(
            !ctx.adapter_name().is_empty(),
            "an adapter with no name means the description was never filled in"
        );
    }

    #[test]
    fn the_context_is_built_once_and_reused() {
        // ⚠⚠ The guard on the thing that keeps these tests affordable. Acquiring an adapter is a
        // ~50 s one-off in a debug build, so a `shared()` that rebuilt would multiply that by the
        // number of GPU tests — and it would still pass every other test in this crate.
        let Some(a) = shared() else { return };
        let Some(b) = shared() else { return };
        assert!(
            std::sync::Arc::ptr_eq(a, b),
            "shared() must hand back the same context, not build another"
        );
    }
}
