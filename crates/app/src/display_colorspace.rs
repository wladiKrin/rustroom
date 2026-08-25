//! Hand the preview's colour management to macOS.
//!
//! ⚠ **wgpu never sets `CAMetalLayer.colorspace`** — it touches only
//! `wantsExtendedDynamicRangeContent` — and Apple's SDK header says a nil colorspace means *"no
//! colormatching occurs"*. So our sRGB pixels were being driven into the panel's native primaries
//! unconverted. Setting the property tells Core Animation what they are, and it matches them to
//! the display's own **calibrated** profile — following the window between monitors, which the
//! manual setting structurally cannot do.
//!
//! ⚠ Set **once**: `wgpu_hal`'s `configure()` sets device, pixel format, framebuffer-only, EDR,
//! drawable count and drawable size, and never touches `colorspace`. The value survives every
//! resize and surface rebuild.
//!
//! ⚠ Deliberately thin, because none of it can be unit tested — it needs a live window and a real
//! Metal layer. The decision it feeds is a pure function; see `Settings::effective_display_space`.

use objc2::rc::Retained;
use objc2::Message;
use objc2_app_kit::NSView;
use objc2_core_graphics::{kCGColorSpaceSRGB, CGColorSpace};
use objc2_quartz_core::{CALayer, CAMetalLayer};
use raw_window_handle::RawWindowHandle;

/// Tag the window's Metal layer as sRGB. Returns whether it worked.
///
/// ⚠ The return value is load-bearing: a `false` puts the manual setting back in charge rather
/// than leaving the preview uncorrected. Never change this to return `()`.
pub fn tag_as_srgb(handle: RawWindowHandle) -> bool {
    let RawWindowHandle::AppKit(handle) = handle else {
        return false;
    };
    // SAFETY: eframe hands us a live `NSView` for the window we are rendering into, and this runs
    // on the main thread during app construction.
    let view: &NSView = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
    let Some(layer) = view.layer() else {
        return false;
    };
    let Some(metal) = find_metal_layer(&layer) else {
        return false;
    };
    let Some(srgb) = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB })) else {
        return false;
    };
    metal.setColorspace(Some(&srgb));
    // Read it back rather than trusting the setter: this is the whole feature, and a silent
    // no-op here would look exactly like the OS not colour-managing.
    metal.colorspace().is_some()
}

/// The view's own layer, or the first `CAMetalLayer` among its descendants.
///
/// ⚠ Both shapes are handled because wgpu may nest it: `wgpu_hal`'s own occlusion workaround
/// walks `superlayer()` *upward* from the metal layer to find the view, which only makes sense if
/// the metal layer is sometimes a sublayer. Assuming either shape would work until it didn't.
fn find_metal_layer(layer: &CALayer) -> Option<Retained<CAMetalLayer>> {
    if let Some(metal) = layer.downcast_ref::<CAMetalLayer>() {
        return Some(metal.retain());
    }
    // SAFETY: reading the sublayer list of a live layer on the main thread.
    let subs = unsafe { layer.sublayers() }?;
    subs.iter().find_map(|sub| find_metal_layer(&sub))
}
