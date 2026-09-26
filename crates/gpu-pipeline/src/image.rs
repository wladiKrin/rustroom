//! A buffer that lives on the GPU across stages.

use crate::GpuContext;

/// An image resident in GPU memory.
///
/// ⚠⚠ **In-place, and spatial stages get SCRATCH rather than chain-level ping-pong.** Every
/// per-pixel stage is in-place-safe because each invocation touches only its own pixel. A spatial
/// stage reads neighbours and cannot be — so M8.3c gave it [`GpuImage::scratch`], transient planes
/// it borrows *within* its own `encode`. A separable blur goes `front → scratch → front`, so the
/// result lands back where it started and the chain's contract — every stage reads and writes the
/// one image — is unchanged from M8.3b.
///
/// ⚠ Chain-level ping-pong was the alternative and loses: it makes "which buffer is the front"
/// a fact every stage must agree about, where scratch makes the question unrepresentable.
#[derive(Debug)]
pub struct GpuImage {
    pub(crate) buffer: wgpu::Buffer,
    width: usize,
    height: usize,
    /// Pixels the buffer holds. ⚠ Can exceed `width * height` after [`Self::relabel`]: a
    /// size-changing stage writes its smaller output into the same buffer.
    capacity: usize,
}

impl GpuImage {
    /// Allocate an uninitialised resident image.
    pub fn new(ctx: &GpuContext, width: usize, height: usize) -> Option<Self> {
        let len = width.checked_mul(height)?.checked_mul(3)?;
        let bytes = u64::try_from(len).ok()?.checked_mul(4)?;
        if bytes == 0 {
            return None;
        }
        // ⚠⚠ **Refusing is the whole point: `create_buffer` does not return an error, it
        // PANICS.** wgpu reports a validation failure through the uncaptured-error handler, which
        // aborts the calling thread — and the caller here is the render worker, so one oversized
        // frame stopped the app rendering for the rest of the session. `None` instead, which every
        // caller already handles by falling back to the CPU twin.
        //
        // ⚠ The ceiling is the device's, not a constant: see [`GpuContext::max_image_bytes`]. With
        // the adapter's real limits an M2 allows 4 GiB, so nothing a camera produces reaches this
        // — it is the guard that makes the next sensor a slower render rather than a dead worker.
        if bytes > ctx.max_image_bytes() {
            return None;
        }
        let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gpu-image"),
            size: bytes,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_DST
                | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        Some(Self {
            buffer,
            width,
            height,
            capacity: width * height,
        })
    }

    /// Upload `image` into a new GPU buffer that outlives any one chain run — the resident input.
    ///
    /// ⚠ `write_buffer` stages internally and is ordered before any later submission on the same
    /// queue, so a chain run submitted after this reads the uploaded pixels.
    pub fn upload(ctx: &GpuContext, image: &core_image::ImageBuf) -> Option<Self> {
        let img = Self::new(ctx, image.width(), image.height())?;
        ctx.queue
            .write_buffer(&img.buffer, 0, bytemuck::cast_slice(image.data()));
        Some(img)
    }

    pub fn width(&self) -> usize {
        self.width
    }

    pub fn height(&self) -> usize {
        self.height
    }

    /// Pixels, which is what a dispatch is sized in.
    ///
    /// ⚠ **Pixels, never floats.** A per-float 1D dispatch at fit resolution already exceeds the
    /// 65535-workgroup limit — the bug M8.3a hit by hand.
    pub fn pixels(&self) -> u32 {
        (self.width * self.height) as u32
    }

    /// Allocate `n` single-channel planes of `width × height` floats.
    ///
    /// ⚠⚠ **Transient within a stage, and SHARED between the stages of a chain.** No stage may
    /// assume a plane still holds what it wrote — the chain lends the same planes to every stage
    /// in turn, which is exactly what makes the **max** rather than the sum the right allocation.
    ///
    /// ⚠ Single-channel, not RGB: every consumer here works on a luma plane, so allocating three
    /// channels would triple the memory for nothing.
    pub(crate) fn scratch(
        ctx: &GpuContext,
        width: usize,
        height: usize,
        n: usize,
    ) -> Vec<wgpu::Buffer> {
        // ⚠ `.max(4)` because **wgpu refuses a zero-sized buffer**. `GpuImage::new` already
        // rejects a degenerate image, so this is defence in depth rather than a live path.
        let bytes = ((width * height * 4) as u64).max(4);
        (0..n)
            .map(|_| {
                ctx.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("gpu-scratch"),
                    size: bytes,
                    usage: wgpu::BufferUsages::STORAGE,
                    mapped_at_creation: false,
                })
            })
            .collect()
    }

    /// Re-label the image after a stage changed its size.
    ///
    /// ⚠⚠ **Only ever shrinks or permutes**, so the output is always the leading `w * h` pixels of
    /// the same buffer — GPU geometry crops or swaps axes, nothing grows. A request to grow is a
    /// bug in a stage's `output_dims` and panics here rather than reading past the buffer.
    pub(crate) fn relabel(&mut self, width: usize, height: usize) {
        assert!(
            width * height <= self.capacity,
            "a GPU stage grew the image: {width}x{height} > {} pixels",
            self.capacity
        );
        self.width = width;
        self.height = height;
    }

    pub(crate) fn bytes(&self) -> u64 {
        (self.width * self.height * 3 * 4) as u64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_zero_sized_image_is_refused_rather_than_allocated() {
        // ⚠ `wgpu` panics on a zero-size buffer, and a zero-width image is reachable — the same
        // degenerate case `map_pixels_xy` guards for.
        let Some(ctx) = crate::shared() else {
            eprintln!("SKIPPED: no GPU adapter on this machine");
            return;
        };
        assert!(GpuImage::new(ctx, 0, 10).is_none());
        assert!(GpuImage::new(ctx, 10, 0).is_none());
        assert!(GpuImage::new(ctx, 4, 4).is_some());
    }

    #[test]
    fn an_image_past_the_devices_ceiling_is_refused_rather_than_allocated() {
        // ⚠⚠ **This is the M8.3p crash as a test, and it discriminates by PANICKING without the
        // guard** — `wgpu::Device::create_buffer` reports a validation failure by aborting the
        // calling thread, so a missing gate does not merely return the wrong value here.
        //
        // ⚠ The size is chosen to clear `usize` overflow — which the two `checked_mul`s above
        // already reject — so the only thing that can refuse it is the ceiling itself.
        let Some(ctx) = crate::shared() else {
            eprintln!("SKIPPED: no GPU adapter on this machine");
            return;
        };
        let side = 200_000; // 4e10 px x 12 bytes = 480 GB, past any device
        assert!(
            GpuImage::new(ctx, side, side).is_none(),
            "an image past max_image_bytes must be declined, never handed to create_buffer"
        );
    }

    #[test]
    fn a_full_resolution_frame_fits_this_device() {
        // ⚠⚠ **The regression, stated in the units the bug arrived in.** A 26 Mpx X-T4 frame is
        // 298 MiB of RGB f32; under `Limits::default()` the storage-binding ceiling is 128 MiB, so
        // this was refused — and before the guard above, fatally. It passes only because
        // `GpuContext::new` now asks the adapter for what it actually offers.
        //
        // ⚠ A device that genuinely cannot take 298 MiB would fail this, and that is the honest
        // outcome: the GPU path cannot carry a full-resolution frame there and the reader should
        // know rather than wonder why 1:1 is slow.
        let Some(ctx) = crate::shared() else { return };
        assert!(
            GpuImage::new(ctx, 6246, 4170).is_some(),
            "a full-resolution frame must fit: needed {} bytes, device allows {}",
            6246u64 * 4170 * 3 * 4,
            ctx.max_image_bytes()
        );
    }

    #[test]
    fn the_dispatch_is_sized_in_pixels_not_floats() {
        let Some(ctx) = crate::shared() else { return };
        let img = GpuImage::new(ctx, 100, 50).expect("allocatable");
        assert_eq!(img.pixels(), 5000, "pixels, not 15000 floats");
        assert_eq!(img.bytes(), 100 * 50 * 3 * 4);
    }
}
