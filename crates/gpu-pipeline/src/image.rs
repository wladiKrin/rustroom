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
}

impl GpuImage {
    /// Allocate an uninitialised resident image.
    pub fn new(ctx: &GpuContext, width: usize, height: usize) -> Option<Self> {
        let len = width.checked_mul(height)?.checked_mul(3)?;
        let bytes = u64::try_from(len).ok()?.checked_mul(4)?;
        if bytes == 0 {
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
        })
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
    fn the_dispatch_is_sized_in_pixels_not_floats() {
        let Some(ctx) = crate::shared() else { return };
        let img = GpuImage::new(ctx, 100, 50).expect("allocatable");
        assert_eq!(img.pixels(), 5000, "pixels, not 15000 floats");
        assert_eq!(img.bytes(), 100 * 50 * 3 * 4);
    }
}
