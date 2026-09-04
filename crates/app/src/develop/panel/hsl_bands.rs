//! The eight HSL band sliders for one channel, with the segmented channel selector above them.
//!
//! ⚠⚠ Shared by the **global panel** and by a **mask**, because `adjustments.rs`'s own comment
//! beside this block already states the rule: *"One layout learned once."* Duplicating it is how
//! the two would drift on band order, on range, or on which channel the selector is showing —
//! and `HSL_BAND_NAMES` is already the single source of truth for the names, so a second copy of
//! the loop would be the only thing left free to disagree.

use crate::develop::panel::adjustments::HslChannel;
use crate::develop::panel::Edited;
use eframe::egui;

/// Draw the channel selector and the eight band sliders for `hsl`.
///
/// ⚠ `channel` is `&mut` because the selector writes it, and each caller owns its **own**
/// channel state. A mask must not share the global panel's: switching to a mask would then
/// silently change which channel the global panel is showing when you switch back.
pub fn show(
    ui: &mut egui::Ui,
    hsl: &mut edit_model::Hsl,
    channel: &mut HslChannel,
    ed: &mut Edited,
) {
    // ⚠ A selector, not three nested CollapsingHeaders. Nesting cost a third level *and*
    // put a "Saturation" header under a "Saturation" slider meaning something else.
    ui.horizontal(|ui| {
        for c in HslChannel::ALL {
            ui.selectable_value(channel, c, c.label());
        }
    });
    for (i, name) in edit_model::HSL_BAND_NAMES.iter().enumerate() {
        ed.slider(
            ui,
            channel.value_mut(&mut hsl.bands[i]),
            -100.0..=100.0,
            name,
        );
    }
}
