//! Packing a finished render for the GPU: quantized bytes, the display conversion, the histogram.

use super::frame::{Proof, RenderedFrame};
use super::proof::proof_texture;
use core_image::ImageBuf;
use pipeline::Region;
use std::sync::Arc;

/// What a finished render hands to [`pack`].
///
/// ⚠⚠ **The `Rgba8` arm carries no `ImageBuf` at all**, and that is what keeps it small: the
/// histogram already comes from the quantized bytes, and `srgb` and `rgba` become one shared
/// allocation — precisely the existing `display_space == Srgb` branch.
///
/// ⚠ It is constructible **only where M8.3g's gate held**. Two things downstream need the float
/// — Visualize Spots and soft proofing — and on this arm they are unrepresentable rather than
/// guarded against.
///
/// ⚠⚠ `display` is the texture's own plane for a non-sRGB display (2026-11-18), converted on the
/// GPU from the same float pixel before quantizing; `bytes` stays the sRGB plane the histogram and
/// the readout read. The tail fills it from the same `color::Display` this frame's `pack` gets, so
/// a non-sRGB display with no plane cannot be built — if one reached `pack`, `float` would say so.
pub enum Packed {
    Float(ImageBuf),
    Rgba8 {
        bytes: Vec<u8>,
        display: Option<Vec<u8>>,
        w: usize,
        h: usize,
    },
}

/// The float image, on the paths that need it.
///
/// ⚠ Every caller is on a path [`Packed::Rgba8`] cannot reach, because the gate that builds it
/// excludes exactly those. The message is the documentation of that invariant — if it ever
/// fires, the gate and this function have come to disagree, which is the one thing the enum is
/// shaped to prevent.
fn float(out: &Option<ImageBuf>) -> &ImageBuf {
    out.as_ref()
        .expect("this path needs the float image, and the gate keeps it for exactly these")
}

/// Pack a rendered sRGB image into an RGBA8 frame.
///
/// The quantization lives on `ImageBuf`, where the buffer and rayon already are — which is also
/// why `app` needs no `rayon` dependency of its own.
/// Quantize a rendered frame for the GPU, converting into the display's primaries on the way.
///
/// ⚠ The conversion happens **here, in float, before quantizing** — not on the packed bytes. An
/// 8-bit → 8-bit conversion compresses the range and costs roughly 0.8 bits, which shows as
/// banding on a smooth sky. It also happens *after* the pipeline rather than inside it, so the
/// histogram stays image-referred; see the spec for why that is the whole design.
///
/// ⚠ `proofed` is the **second** tail run — the same frame rendered into ProPhoto — paired with the
/// proof it was rendered for. It is `None` on the ordinary path, which is then bit-exactly what it
/// was before soft proofing existed.
pub(super) fn pack(
    packed: Packed,
    proofed: Option<(ImageBuf, Proof)>,
    region: Option<Region>,
    max_dim: usize,
    display_space: color::Display,
    visualize: Option<f32>,
) -> RenderedFrame {
    let (out, width, height, pre, converted) = match packed {
        Packed::Float(out) => {
            let (w, h) = (out.width(), out.height());
            (Some(out), w, h, None, None)
        }
        Packed::Rgba8 {
            bytes,
            display,
            w,
            h,
        } => (None, w, h, Some(bytes), display),
    };
    // ⚠ Quantized to sRGB *first*, and the histogram taken from that. The histogram describes
    // the image; the texture describes what this monitor must be driven with. Deriving one from
    // the other is the bug this ordering exists to prevent.
    //
    // ⚠⚠ On the byte path the GPU has **already done this quantization**, with the same rounding
    // rule — so the histogram is unchanged by construction rather than by a second implementation.
    // That is what makes M8.3g's `Rgba8` arm small: it needs no `ImageBuf` at all.
    let srgb8 = match pre {
        Some(b) => b,
        None => float(&out).to_rgba8(),
    };
    let hist = crate::histogram::histogram_rgba(&srgb8);
    // ⚠⚠ Wrapped **here**, above the Visualize Spots block, so `srgb` carries the photograph's
    // bytes rather than the edge view's — the same ordering `hist` on the line above depends on,
    // and what makes the colour readout report the photograph under any view mode.
    //
    // ⚠ Moved into the `Arc` rather than cloned, so the ordinary path copies nothing and the
    // sRGB branch at the bottom shares this one allocation with `rgba` instead of duplicating
    // ~7 MB (the fit rung) or ~27 MB (a native one) on every rendered frame.
    let srgb = Arc::new(srgb8);

    // ⚠⚠ Visualize Spots goes here: AFTER the histogram, BEFORE the display transform. The
    // histogram must describe the photograph and not the view mode — the rule `display_space`
    // established above — and the display transform must still run, or the grey view would be
    // driven into the wrong primaries like any other pixels. Sabotage-verified by moving the
    // histogram below this block.
    //
    // ⚠ `shown` is what the *texture* shows; `srgb` above stays the photograph. In the ordinary
    // case they are one allocation.
    let (out, shown) = match visualize {
        Some(threshold) => {
            let out = edge_image(float(&out), threshold);
            let shown = Arc::new(out.to_rgba8());
            (Some(out), shown)
        }
        None => (out, Arc::clone(&srgb)),
    };

    // Env-gated, like `RUSTROOM_DEBUG_BRUSH`: instrumentation beat reasoning twice on the brush
    // canvas, and it did again here — the setting reaching `pack` is not something the code
    // reads wrong, it is something a call site can quietly bypass.
    if std::env::var_os("RUSTROOM_DEBUG_DISPLAY").is_some() {
        eprintln!(
            "pack: {}x{} space={:?} region={} proof={:?}",
            width,
            height,
            display_space,
            region.is_some(),
            proofed.as_ref().map(|(_, p)| p),
        );
    }

    let rgba = match (proofed, converted) {
        // ⚠⚠ The proof **replaces** the display transform for the texture rather than composing
        // with it. That is the original architecture of this feature, kept through the amendment
        // that changed where the data comes from: soft proofing points the space `pack` converts
        // into at the export target instead of at the monitor. The consequence, recorded in the
        // spec rather than discovered later: while proofing, the display-profile setting has no
        // effect — which is also why the spec says the thing to look at is the *difference*
        // between proof and no proof, not the absolute colour.
        (Some((proof_out, proof)), _) => Arc::new(proof_texture(&proof_out, proof)),
        (None, _) if display_space.is_srgb() => shown,
        // ⚠ Converted on the GPU already, from the same float (`quantize.rs`'s display plane).
        (None, Some(converted)) => Arc::new(converted),
        (None, None) => {
            // ⚠ Converted from the **float** image, not from `shown`. An 8-bit → 8-bit conversion
            // compresses the range and costs roughly 0.8 bits, which shows as banding on a smooth
            // sky. Paid only for a frame the GPU did not render (no adapter, the GPU tail off, a
            // fallback) — the byte path brings the display's plane with it.
            Arc::new(float(&out).to_rgba8_with(move |c| display_space.convert(c)))
        }
    };

    RenderedFrame {
        width,
        height,
        rgba,
        srgb,
        hist,
        region,
        max_dim,
        // ⚠ A placeholder. `pack` knows neither the buffer nor the geometry; both callers
        // overwrite this immediately. It is the frame's own size, so a caller that forgot would
        // degrade to the pre-`fit_size` behaviour at the fit rung rather than to a zero.
        fit_size: (width as f32, height as f32),
        grade_view: None,
    }
}

/// The Visualize Spots view of a rendered frame: the edge view of its luminance, written back as
/// grey.
///
/// ⚠ Taken on the **encoded** output, not linear light. This is a view aid, not a measurement,
/// and an edge in encoded values is closer to what the eye reads as an edge — which is what
/// makes dust findable. `color::luminance` is only a weighted sum here.
fn edge_image(out: &ImageBuf, threshold: f32) -> ImageBuf {
    let (w, h) = (out.width(), out.height());
    let luma: Vec<f32> = out
        .data()
        .as_chunks::<3>()
        .0
        .iter()
        .map(|p| color::luminance(*p))
        .collect();
    let edges = crate::visualize::edge_view(&luma, w, h, threshold);
    let mut data = Vec::with_capacity(edges.len() * 3);
    for e in edges {
        data.extend_from_slice(&[e, e, e]);
    }
    ImageBuf::from_vec(w, h, data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render_engine::render::render_once;
    use edit_model::Edits;

    #[test]
    fn the_display_space_changes_the_packed_pixels_and_srgb_does_not() {
        // ⚠ Two claims. The sRGB half is the regression guard — it is the default, and if it
        // moved every preview would shift for people who never touched the setting. The P3 half
        // proves the parameter is actually reaching `pack`; without it the setting would be a
        // dropdown that does nothing, which is the failure this whole task risks.
        let mut buf = ImageBuf::new(2, 1);
        buf.set_pixel(0, 0, color::srgb_to_prophoto([0.9, 0.1, 0.1]));
        buf.set_pixel(1, 0, color::srgb_to_prophoto([0.2, 0.6, 0.3]));

        let srgb = render_once(
            &buf,
            &Edits::default(),
            2,
            1.0,
            color::Display::Space(color::OutputSpace::Srgb),
            pipeline::Resampler::default(),
            None,
        );
        let p3 = render_once(
            &buf,
            &Edits::default(),
            2,
            1.0,
            color::Display::Space(color::OutputSpace::DisplayP3),
            pipeline::Resampler::default(),
            None,
        );

        assert_ne!(srgb.rgba, p3.rgba, "the display space did not reach `pack`");
        // ⚠ The corrected image must be *less* saturated in red, not more — an inverted
        // transform would also differ, and would reproduce the very bug this fixes while
        // passing an `assert_ne!`.
        assert!(
            p3.rgba[0] < srgb.rgba[0],
            "P3 red {} should be below sRGB red {}",
            p3.rgba[0],
            srgb.rgba[0]
        );
    }

    #[test]
    fn the_histogram_is_unaffected_by_the_display_space() {
        // ⚠⚠ The property this whole design exists to protect, and it was **shipped broken
        // once**: the transform went into `pack`, and `shell.rs` derived the histogram from
        // `pack`'s output — so the histogram silently became display-referred. A histogram that
        // moves when you change monitors cannot tell you what is clipping.
        let mut buf = ImageBuf::new(2, 1);
        buf.set_pixel(0, 0, color::srgb_to_prophoto([0.95, 0.15, 0.1]));
        buf.set_pixel(1, 0, color::srgb_to_prophoto([0.2, 0.6, 0.3]));

        let srgb = render_once(
            &buf,
            &Edits::default(),
            2,
            1.0,
            color::Display::Space(color::OutputSpace::Srgb),
            pipeline::Resampler::default(),
            None,
        );
        let p3 = render_once(
            &buf,
            &Edits::default(),
            2,
            1.0,
            color::Display::Space(color::OutputSpace::DisplayP3),
            pipeline::Resampler::default(),
            None,
        );

        assert_ne!(
            srgb.rgba, p3.rgba,
            "the display space did not reach the texture"
        );
        assert_eq!(
            srgb.hist, p3.hist,
            "the histogram must describe the image, not the monitor"
        );
    }

    /// The fixture `the_histogram_is_unaffected_by_the_display_space` uses, reused rather than
    /// re-invented: a two-pixel buffer with a saturated red and a mid green, which is what makes
    /// the display transform actually move something.
    fn render_for_test(space: color::Display) -> RenderedFrame {
        let mut buf = ImageBuf::new(2, 1);
        buf.set_pixel(0, 0, color::srgb_to_prophoto([0.95, 0.15, 0.1]));
        buf.set_pixel(1, 0, color::srgb_to_prophoto([0.2, 0.6, 0.3]));
        render_once(
            &buf,
            &Edits::default(),
            2,
            1.0,
            space,
            pipeline::Resampler::default(),
            None,
        )
    }

    /// ⚠⚠ **`srgb` must be sRGB whatever the display space, and `rgba` must not.** Nothing else
    /// can see this: a colour readout built on `rgba` looks entirely plausible and is simply
    /// wrong on a wide-gamut monitor. This is the sibling of
    /// `the_histogram_is_unaffected_by_the_display_space`, one layer over — and the histogram is
    /// asserted here too, so the two instrument channels cannot drift apart.
    #[test]
    fn the_srgb_channel_is_unaffected_by_the_display_space() {
        let a = render_for_test(color::Display::Space(color::OutputSpace::Srgb));
        let b = render_for_test(color::Display::Space(color::OutputSpace::DisplayP3));
        assert_eq!(
            *a.srgb, *b.srgb,
            "the sRGB channel moved with the display space"
        );
        assert_ne!(
            *a.rgba, *b.rgba,
            "fixture check: the display transform did nothing, so this test proves nothing"
        );
        assert_eq!(a.hist, b.hist);
    }

    /// ⚠ In the sRGB case the two must be **one** allocation, or every rendered frame pays a
    /// multi-megabyte clone — ~7 MB at the fit rung, ~27 MB at a native one. Asserted
    /// structurally rather than by measuring.
    #[test]
    fn the_srgb_case_shares_one_allocation() {
        let f = render_for_test(color::Display::Space(color::OutputSpace::Srgb));
        assert!(
            Arc::ptr_eq(&f.rgba, &f.srgb),
            "sRGB rendered two buffers where one would do"
        );
    }

    /// A monitor's own profile (2026-11-18) reaches the texture like a fixed space does — and, like it, never the
    /// histogram or the sRGB channel. A wide panel's profile drives the saturated red DOWN, not up.
    #[test]
    fn a_monitors_profile_moves_the_texture_and_nothing_else() {
        let wide = color::edid::Primaries {
            red: (0.68, 0.32),
            green: (0.21, 0.71),
            blue: (0.15, 0.06),
            white: (0.3127, 0.3290),
            gamma: 2.2,
        };
        let profile = color::DisplayProfile::from_primaries("wide", &wide).expect("a profile");
        let a = render_for_test(color::Display::SRGB);
        let b = render_for_test(color::Display::Profile(profile));
        assert_ne!(*a.rgba, *b.rgba, "the profile did not reach the texture");
        assert!(b.rgba[0] < a.rgba[0], "red {} vs {}", b.rgba[0], a.rgba[0]);
        assert_eq!(*a.srgb, *b.srgb, "the sRGB channel moved with the monitor");
        assert_eq!(a.hist, b.hist, "the histogram moved with the monitor");
    }

    /// ⚠⚠ The two arms of [`Packed`] must produce the **same frame**, because the GPU's
    /// quantization and `to_rgba8` are the same rounding of the same number — M8.3g's whole
    /// premise. If this ever fails, the shader and `ImageBuf::quantize` have diverged.
    ///
    /// ⚠ And the histogram must come from the bytes rather than being recomputed: on the byte path
    /// there is no float left to recompute it from, which is what makes that arm small.
    #[test]
    fn packing_bytes_gives_the_same_frame_as_packing_the_float() {
        let buf = ImageBuf::from_vec(2, 1, vec![0.0, 0.5, 1.0, -0.2, 0.25, 1.7]);
        let want = pack(
            Packed::Float(buf.clone()),
            None,
            None,
            16,
            color::Display::Space(color::OutputSpace::Srgb),
            None,
        );
        let got = pack(
            Packed::Rgba8 {
                bytes: buf.to_rgba8(),
                display: None,
                w: 2,
                h: 1,
            },
            None,
            None,
            16,
            color::Display::Space(color::OutputSpace::Srgb),
            None,
        );
        assert_eq!(*got.rgba, *want.rgba, "texture");
        assert_eq!(*got.srgb, *want.srgb, "the photograph's own bytes");
        assert_eq!(got.hist, want.hist, "histogram");
        assert_eq!((got.width, got.height), (2, 1));
        assert!(
            Arc::ptr_eq(&got.rgba, &got.srgb),
            "the two must be one allocation, as on the float sRGB path"
        );
    }

    /// ⚠ A display's plane from the GPU (2026-11-18) IS the texture, and the sRGB bytes beside it stay the
    /// photograph's — the histogram and the readout — exactly as the float path's CPU conversion leaves them.
    #[test]
    fn a_display_plane_from_the_gpu_is_the_texture_and_nothing_else() {
        let buf = ImageBuf::from_vec(2, 1, vec![0.0, 0.5, 1.0, 0.8, 0.25, 0.1]);
        let p3 = color::Display::Space(color::OutputSpace::DisplayP3);
        let want = pack(Packed::Float(buf.clone()), None, None, 16, p3, None);
        let plane = buf.to_rgba8_with(move |c| p3.convert(c));
        assert_ne!(plane, buf.to_rgba8(), "premise: P3 moves these pixels");
        let got = pack(
            Packed::Rgba8 {
                bytes: buf.to_rgba8(),
                display: Some(plane.clone()),
                w: 2,
                h: 1,
            },
            None,
            None,
            16,
            p3,
            None,
        );
        assert_eq!(*got.rgba, plane, "the texture is the display's plane");
        assert_eq!(*got.rgba, *want.rgba, "as the float path converts it");
        assert_eq!(*got.srgb, *want.srgb, "the photograph's own bytes");
        assert_eq!(got.hist, want.hist, "histogram");
    }

    /// ⚠⚠ `srgb` is taken **before** Visualize Spots replaces the image, so the readout reports
    /// the photograph and not the grey edge view — the ordering `hist` already depends on, and
    /// the reason `pack` wraps the `Arc` above that block rather than below it. Nothing in the
    /// canvas can see this: an edge-view readout is a plausible-looking number.
    #[test]
    fn the_srgb_channel_is_unaffected_by_visualize_spots() {
        // A hard edge, so the edge view is far from the photograph and the textures must differ.
        let mut buf = ImageBuf::new(16, 4);
        for y in 0..4 {
            for x in 0..16 {
                let v = if x < 8 { 0.1 } else { 0.8 };
                buf.set_pixel(x, y, [v, v, v]);
            }
        }
        let plain = pack(
            Packed::Float(buf.clone()),
            None,
            None,
            16,
            color::Display::Space(color::OutputSpace::Srgb),
            None,
        );
        let vis = pack(
            Packed::Float(buf),
            None,
            None,
            16,
            color::Display::Space(color::OutputSpace::Srgb),
            Some(12.0),
        );
        assert_ne!(
            *plain.rgba, *vis.rgba,
            "fixture check: the view mode did not reach the pixels"
        );
        assert_eq!(
            *plain.srgb, *vis.srgb,
            "the view mode reached the readout's bytes"
        );
    }

    /// ⚠⚠ The histogram must be **unaffected** by Visualize Spots, for the reason the display
    /// transform records: `pack` takes the histogram from its sRGB output *before* the post-step,
    /// and a view mode that poisoned it would make the histogram lie about the photograph. This
    /// is the test that the ordering was inherited rather than reinvented.
    #[test]
    fn the_histogram_is_unaffected_by_visualize_spots() {
        // A hard edge, so the edge view is far from black and the two textures must differ.
        let mut buf = ImageBuf::new(16, 4);
        for y in 0..4 {
            for x in 0..16 {
                let v = if x < 8 { 0.1 } else { 0.8 };
                buf.set_pixel(x, y, [v, v, v]);
            }
        }
        let plain = pack(
            Packed::Float(buf.clone()),
            None,
            None,
            16,
            color::Display::Space(color::OutputSpace::Srgb),
            None,
        );
        let vis = pack(
            Packed::Float(buf),
            None,
            None,
            16,
            color::Display::Space(color::OutputSpace::Srgb),
            Some(12.0),
        );
        assert_ne!(
            plain.rgba, vis.rgba,
            "the view mode did not reach the pixels"
        );
        assert_eq!(plain.hist, vis.hist, "the view mode reached the histogram");
    }
}
