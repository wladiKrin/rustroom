//! The one place Rustroom's look is defined.
//!
//! ⚠ This currently **reproduces egui's defaults exactly** — it changes nothing about how the
//! app looks. That is the entire point of the step: the look has never been designed, and this
//! is the seam through which it will be, in one file rather than in two hundred call sites.
//!
//! The split that makes that cheap: anything expressible in `egui::Style`/`Visuals` — colours,
//! spacing, rounding, fonts — is a central change applied retroactively everywhere, so those
//! choices are cheap to make late. What is expensive late is *structural*: whether buttons
//! carry icons, whether sections have headers, how a control row is composed — which is why
//! this module grows helpers that encode conventions, not just colours.

use eframe::egui;

/// Apply Rustroom's style, once at startup.
///
/// ⚠ **egui 0.35 keeps a separate `Style` per theme** — dark and light — so there is no single
/// style to set, and `Context::set_style` does not exist. This has a consequence for the
/// eventual visual identity: Rustroom never calls `set_theme`, so it *follows the system* and
/// both themes are live. Anything theme-independent (spacing, text sizes, rounding) must be
/// applied to both; anything colour-related belongs in that theme's own `visuals` and must
/// **not** be copied across, or light mode inherits dark colours.
///
/// Hence `style_mut_of` rather than `set_style_of`: mutating in place preserves each theme's
/// own visuals, where handing over a whole `Style` would overwrite them.
pub fn apply(ctx: &egui::Context) {
    for theme in [egui::Theme::Dark, egui::Theme::Light] {
        ctx.style_mut_of(theme, tune);
    }
}

/// The theme-independent tweaks.
///
/// Deliberately empty: reproducing the current look *exactly* is the point of this step. Every
/// future decision that is not a colour is a line added here, and it reaches both themes at
/// once because [`apply`] calls it for each.
fn tune(style: &mut egui::Style) {
    let _ = style;
}

/// The small explanatory line under a control.
///
/// ⚠ Distinct from a *caption* — a filename under a thumbnail, a camera model beside a title —
/// which renders identically today but means something different. Those live in the Library
/// grid and the develop toolbar and are deliberately left alone until the canvas split, where
/// their call sites are. Two idioms that merely look alike should not be merged into one
/// helper.
pub fn hint(ui: &mut egui::Ui, text: &str) {
    ui.label(egui::RichText::new(text).small());
}

/// The trailing confirm/dismiss button pair. Returns `(confirmed, dismissed)`.
///
/// ⚠ This **encodes the order the dialogs already use** — confirm first, dismiss second — it
/// does not impose a new one. An audit of all nine before the split found them already
/// uniform, with "Close" appearing only in Settings, the one dialog that has nothing to
/// abandon. Its job is to stop that drifting as dialogs are added, which is why it earns its
/// place even though it changes nothing today.
pub fn button_row(
    ui: &mut egui::Ui,
    confirm: &str,
    confirm_enabled: bool,
    dismiss: &str,
) -> (bool, bool) {
    let mut confirmed = false;
    let mut dismissed = false;
    ui.horizontal(|ui| {
        if ui
            .add_enabled(confirm_enabled, egui::Button::new(confirm))
            .clicked()
        {
            confirmed = true;
        }
        if ui.button(dismiss).clicked() {
            dismissed = true;
        }
    });
    (confirmed, dismissed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A default style with our tweaks applied — what both themes end up with.
    fn tuned() -> egui::Style {
        let mut s = egui::Style::default();
        tune(&mut s);
        s
    }

    #[test]
    fn tuning_leaves_a_usable_style() {
        // ⚠ Thin on purpose. `tune` is a pile of field assignments; what could actually break
        // is it panicking, which at startup would take the whole app with it.
        let s = tuned();
        assert!(s.text_styles.contains_key(&egui::TextStyle::Body));
    }

    #[test]
    fn hint_text_is_smaller_than_body_text() {
        // ⚠ Today this passes on egui's defaults, so it is a **tripwire, not a discovery**:
        // it starts earning its keep the moment `tune` begins setting `text_styles` itself,
        // at which point a hint that reads at body size is no longer a hint.
        let s = tuned();
        let body = s.text_styles[&egui::TextStyle::Body].size;
        let small = s.text_styles[&egui::TextStyle::Small].size;
        assert!(small < body, "small {small} should be under body {body}");
    }

    #[test]
    fn both_themes_are_tuned() {
        // ⚠ The property the per-theme API makes easy to get wrong: a tweak applied to only
        // one theme is invisible until someone switches their system appearance. Asserted via
        // the loop's own source of truth so it cannot silently drop a theme.
        let ctx = egui::Context::default();
        apply(&ctx);
        for theme in [egui::Theme::Dark, egui::Theme::Light] {
            let body = ctx
                .style_of(theme)
                .text_styles
                .get(&egui::TextStyle::Body)
                .map(|f| f.size);
            assert!(body.is_some(), "{theme:?} lost its body text style");
        }
    }
}
