//! Background render worker: renders `Edits` against a cached linear editing buffer,
//! off the UI thread, coalescing superseded requests.

use core_image::ImageBuf;
use edit_model::Edits;
use pipeline::{build_pipeline, build_pipeline_split, PrefixKey, RenderScale};
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

/// A finished preview frame (display sRGB, RGBA8).
pub struct RenderedFrame {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

struct Request {
    edits: Edits,
    max_dim: usize,
}

/// Owns a worker thread that renders against a fixed editing buffer.
pub struct RenderEngine {
    req_tx: Sender<Request>,
    res_rx: Receiver<RenderedFrame>,
    _handle: JoinHandle<()>,
}

impl RenderEngine {
    /// `source_scale` is the size of `buffer` relative to a full-resolution decode of the
    /// same file (0.5 for a half-size RAW decode, 1.0 for JPEG/PNG/TIFF).
    /// `on_done` is called after each render completes (e.g. `ctx.request_repaint`).
    pub fn new(
        buffer: Arc<ImageBuf>,
        source_scale: f32,
        on_done: impl Fn() + Send + 'static,
    ) -> Self {
        let (req_tx, req_rx) = channel::<Request>();
        let (res_tx, res_rx) = channel::<RenderedFrame>();
        let handle = thread::spawn(move || {
            let mut cache: Option<(CacheKey, Arc<ImageBuf>)> = None;
            while let Ok(mut req) = req_rx.recv() {
                // Coalesce: skip to the most recent pending request.
                loop {
                    match req_rx.try_recv() {
                        Ok(newer) => req = newer,
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => return,
                    }
                }
                let frame =
                    render_cached(&buffer, &req.edits, req.max_dim, source_scale, &mut cache);
                if res_tx.send(frame).is_err() {
                    return;
                }
                on_done();
            }
        });
        Self {
            req_tx,
            res_rx,
            _handle: handle,
        }
    }

    /// Queue a render at the given working resolution.
    pub fn request(&self, edits: Edits, max_dim: usize) {
        let _ = self.req_tx.send(Request { edits, max_dim });
    }

    /// Return the newest finished frame, if any (drops older ones).
    pub fn poll_latest(&self) -> Option<RenderedFrame> {
        let mut latest = None;
        while let Ok(f) = self.res_rx.try_recv() {
            latest = Some(f);
        }
        latest
    }
}

/// The long edge `downscale_to(max_dim)` will produce, computed without doing the downscale.
///
/// The preview cache needs the render scale on a cache *hit*, when there is no downscaled
/// image to measure. `downscale_to` never upscales and caps the long edge, so this is exact
/// rather than an estimate — asserted against it in the tests.
fn rendered_long_edge(buffer_long: usize, max_dim: usize) -> usize {
    if max_dim == 0 {
        buffer_long
    } else {
        buffer_long.min(max_dim)
    }
}

/// How far a render is from the full-resolution original: the source buffer may already
/// be a half-size decode (`source_scale`), and `downscale_to` shrinks it further. The
/// long edge is the reference because that is what `downscale_to` caps.
fn render_scale(buffer_long: usize, rendered_long: usize, source_scale: f32) -> RenderScale {
    if buffer_long == 0 {
        return RenderScale::FULL;
    }
    RenderScale::new(source_scale * rendered_long as f32 / buffer_long as f32)
}

/// Pack a rendered sRGB image into an RGBA8 frame.
fn pack(out: ImageBuf) -> RenderedFrame {
    let (width, height) = (out.width(), out.height());
    let mut rgba = Vec::with_capacity(width * height * 4);
    for px in out.data().chunks_exact(3) {
        rgba.push((px[0].clamp(0.0, 1.0) * 255.0).round() as u8);
        rgba.push((px[1].clamp(0.0, 1.0) * 255.0).round() as u8);
        rgba.push((px[2].clamp(0.0, 1.0) * 255.0).round() as u8);
        rgba.push(255);
    }
    RenderedFrame {
        width,
        height,
        rgba,
    }
}

/// The cached prefix output is valid for one set of prefix parameters at one working
/// resolution.
type CacheKey = (PrefixKey, usize);

/// Render one request, reusing the cached prefix output when the prefix parameters are
/// unchanged.
///
/// The prefix — geometry and noise reduction — is by far the expensive half once noise
/// reduction is on, and its parameters do not change when an adjustment slider moves. See
/// `docs/superpowers/specs/2026-08-06-preview-prefix-cache-design.md`.
fn render_cached(
    buffer: &ImageBuf,
    edits: &Edits,
    max_dim: usize,
    source_scale: f32,
    cache: &mut Option<(CacheKey, Arc<ImageBuf>)>,
) -> RenderedFrame {
    let buffer_long = buffer.width().max(buffer.height());
    let scale = render_scale(
        buffer_long,
        rendered_long_edge(buffer_long, max_dim),
        source_scale,
    );
    let key = (PrefixKey::new(edits, scale), max_dim);
    let (prefix, tail) = build_pipeline_split(edits, scale);

    let mid = match cache {
        Some((k, img)) if *k == key => img.clone(), // Arc clone: free
        _ => {
            let img = Arc::new(prefix.run(buffer.downscale_to(max_dim)));
            *cache = Some((key, img.clone()));
            img
        }
    };

    // `PipelineStage::apply` consumes its input — the ownership that keeps the future GPU
    // seam clean — so a cached image must be copied before the tail can run. Measured at
    // 0.6 ms against the 9 ms downscale it avoids.
    pack(tail.run((*mid).clone()))
}

/// Render `edits` against `buffer` at a working resolution capped by `max_dim`.
/// Pure, synchronous and **uncached** — used for one-off renders (e.g. the before/after
/// baseline) and as the reference the cached path is tested against.
///
/// `source_scale` is the size of `buffer` relative to a full-resolution decode of the
/// same file; it and the downscale below together tell the pipeline's spatial stages how
/// to scale their pixel radii.
pub fn render_once(
    buffer: &ImageBuf,
    edits: &Edits,
    max_dim: usize,
    source_scale: f32,
) -> RenderedFrame {
    let preview = buffer.downscale_to(max_dim);
    let scale = render_scale(
        buffer.width().max(buffer.height()),
        preview.width().max(preview.height()),
        source_scale,
    );
    pack(build_pipeline(edits, scale).run(preview)) // sRGB, 0..=1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tiny() -> Arc<ImageBuf> {
        let mut img = ImageBuf::new(8, 8);
        for y in 0..8 {
            for x in 0..8 {
                img.set_pixel(x, y, [0.5, 0.5, 0.5]);
            }
        }
        Arc::new(img)
    }

    fn wait_for_frame(engine: &RenderEngine) -> RenderedFrame {
        for _ in 0..200 {
            if let Some(f) = engine.poll_latest() {
                return f;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        panic!("no frame produced");
    }

    /// A buffer with varied content, so a stale cache actually shows up as different bytes.
    /// (The flat gray `tiny()` would hide almost any staleness.)
    fn varied() -> Arc<ImageBuf> {
        let mut img = ImageBuf::new(16, 16);
        for y in 0..16 {
            for x in 0..16 {
                img.set_pixel(
                    x,
                    y,
                    [x as f32 / 16.0, y as f32 / 16.0, ((x + y) % 5) as f32 / 5.0],
                );
            }
        }
        Arc::new(img)
    }

    #[test]
    fn every_cached_frame_matches_a_cold_render() {
        // The property that matters: caching must be invisible in the output. The sequence
        // walks a pure hit, a downstream-only change (hit), an NR change (miss), a return to
        // an earlier state (miss, since there is one entry) and a geometry change (miss).
        let buf = varied();
        let a = Edits {
            exposure: 0.5,
            noise_reduction: edit_model::NoiseReduction {
                luminance: 60.0,
                color: 60.0,
                ..Default::default()
            },
            ..Edits::default()
        };
        let b = Edits {
            exposure: -1.0,
            ..a.clone()
        }; // downstream only
        let c = Edits {
            noise_reduction: edit_model::NoiseReduction {
                luminance: 10.0,
                color: 10.0,
                ..Default::default()
            },
            ..a.clone()
        }; // prefix change
        let d = Edits {
            geometry: edit_model::Geometry {
                angle: 4.0,
                ..Default::default()
            },
            ..a.clone()
        }; // prefix change

        // Guard against the whole test passing trivially: these must actually differ.
        let cold_a = render_once(&buf, &a, 8, 1.0);
        assert_ne!(cold_a.rgba, render_once(&buf, &b, 8, 1.0).rgba);
        assert_ne!(cold_a.rgba, render_once(&buf, &c, 8, 1.0).rgba);
        assert_ne!(cold_a.rgba, render_once(&buf, &d, 8, 1.0).rgba);

        let engine = RenderEngine::new(buf.clone(), 1.0, || {});
        for (i, e) in [&a, &a, &b, &c, &a, &d].iter().enumerate() {
            engine.request((*e).clone(), 8);
            let got = wait_for_frame(&engine);
            let cold = render_once(&buf, e, 8, 1.0);
            assert_eq!(got.rgba, cold.rgba, "frame {i} differs from a cold render");
        }
    }

    #[test]
    fn the_predicted_long_edge_matches_downscale_to() {
        // The cache keys off this prediction, so it must agree with what downscale_to
        // actually produces — including the case where the image is already small enough
        // and downscale_to just clones.
        for (w, h, max_dim) in [(8, 8, 4), (10, 5, 4), (5, 10, 4), (4, 4, 16), (7, 3, 7)] {
            let img = ImageBuf::new(w, h);
            let actual = img.downscale_to(max_dim);
            let actual_long = actual.width().max(actual.height());
            assert_eq!(
                rendered_long_edge(w.max(h), max_dim),
                actual_long,
                "{w}x{h} -> max_dim {max_dim}"
            );
        }
    }

    #[test]
    fn render_scale_combines_the_decode_and_the_downscale() {
        // A half-size RAW decode downscaled again by 2 is a quarter of full resolution;
        // the spatial stages must be told that, or a preview lies about the export.
        assert_eq!(render_scale(8, 4, 0.5).get(), 0.25);
        // A full-size source rendered at full size is exactly 1.0.
        assert_eq!(render_scale(8, 8, 1.0).get(), 1.0);
        // Non-square: the long edge is the reference, matching downscale_to.
        assert_eq!(render_scale(10, 5, 1.0).get(), 0.5);
    }

    #[test]
    fn produces_rgba_frame_of_expected_size() {
        let engine = RenderEngine::new(tiny(), 1.0, || {});
        engine.request(Edits::default(), 4); // downscale 8 -> 4
        let f = wait_for_frame(&engine);
        assert_eq!((f.width, f.height), (4, 4));
        assert_eq!(f.rgba.len(), 4 * 4 * 4);
        // mid-gray linear 0.5 -> sRGB ~188; alpha 255
        assert!(f.rgba[0] > 180 && f.rgba[0] < 195);
        assert_eq!(f.rgba[3], 255);
    }

    #[test]
    fn exposure_brightens() {
        let engine = RenderEngine::new(tiny(), 1.0, || {});
        engine.request(
            Edits {
                exposure: 2.0,
                ..Edits::default()
            },
            4,
        );
        let f = wait_for_frame(&engine);
        assert!(f.rgba[0] > 240); // much brighter
    }

    #[test]
    fn render_once_matches_expected_gray() {
        let img = tiny();
        let f = render_once(&img, &Edits::default(), 4, 1.0);
        assert_eq!((f.width, f.height), (4, 4));
        assert!(f.rgba[0] > 180 && f.rgba[0] < 195);
    }
}
