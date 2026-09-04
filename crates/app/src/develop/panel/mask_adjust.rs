//! The adjustments a mask carries — the per-pixel set it has always had, plus the tail-native
//! stages M7.2 added.
//!
//! ⚠ Its own module because `masks.rs` was 575 lines before this and these controls add ~120.
//! The rest of that file is shape properties and brush settings, which is a different job.

use crate::develop::panel::{adjustments, hsl_bands, Edited};
use edit_model::LocalAdjustments;
use eframe::egui;

/// Draw every adjustment slider for one mask.
///
/// ⚠⚠ **The order mirrors the global chain**, which is both the panel's existing rule and the
/// pipeline's: White Balance, Tone, Colour, then the spatial stages. It is not cosmetic — it is
/// the order `adjustment_stages` inserts them in, so what you read top to bottom is what runs.
pub fn show(
    ui: &mut egui::Ui,
    a: &mut LocalAdjustments,
    baseline: color::wb::Illuminant,
    channel: &mut adjustments::HslChannel,
    ed: &mut Edited,
) {
    // ⚠ The same control as the global one, and the same baseline: a mask declares the
    // light *this region* was under, measured against what the whole photo is balanced
    // to. Two different meanings for one number is how they drift.
    adjustments::white_balance(ui, &mut a.white_balance, baseline, None, ed);
    ed.slider(ui, &mut a.exposure, -5.0..=5.0, "Exposure");
    ed.slider(ui, &mut a.contrast, -100.0..=100.0, "Contrast");
    ed.slider(ui, &mut a.highlights, -100.0..=100.0, "Highlights");
    ed.slider(ui, &mut a.shadows, -100.0..=100.0, "Shadows");
    ed.slider(ui, &mut a.whites, -100.0..=100.0, "Whites");
    ed.slider(ui, &mut a.blacks, -100.0..=100.0, "Blacks");
    // ⚠ The same order as the global panel: White Balance, then Tone, then
    // Color with Vibrance first. One layout learned once, and it keeps
    // holding as either list changes.
    ed.slider(ui, &mut a.vibrance, -100.0..=100.0, "Vibrance");
    ed.slider(ui, &mut a.saturation, -100.0..=100.0, "Saturation");

    ui.separator();
    // ⚠ Negative Texture is the skin-softening tool, and the reason deferring masked noise
    // reduction costs nothing: it removes the fine band and softens edges within it.
    ed.slider(ui, &mut a.local_contrast.texture, -100.0..=100.0, "Texture");
    ed.slider(ui, &mut a.local_contrast.clarity, -100.0..=100.0, "Clarity");

    ui.separator();
    // ⚠ The mask's own channel, never the global panel's — sharing one would silently change
    // which channel the global panel shows when you switch back to it.
    hsl_bands::show(ui, &mut a.hsl, channel, ed);

    ui.separator();
    ui.label("Sharpening");
    // ⚠⚠ Full parity with the global control, `Masking` included — which here is a second gate
    // inside a gate, since the mask has already answered *where*. Kept deliberately (2026-09-04)
    // rather than trimmed: parity costs nothing, and an edge gate is defensible for a loose mask
    // over mixed content.
    ed.slider(ui, &mut a.sharpening.amount, 0.0..=100.0, "Amount");
    ed.slider(ui, &mut a.sharpening.radius, 0.5..=3.0, "Radius");
    ed.slider(ui, &mut a.sharpening.detail, 0.0..=100.0, "Detail");
    ed.slider(ui, &mut a.sharpening.masking, 0.0..=100.0, "Masking");
}
