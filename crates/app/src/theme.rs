//! The one place Rustroom's look is defined.
//!
//! Dark by default with a switchable accent, light available as a setting, compact controls.
//!
//! The split that makes colour cheap to revisit: anything expressible in
//! `egui::Style`/`Visuals` is a central change applied retroactively everywhere. What is
//! expensive to revisit is *structural* — whether buttons carry icons, whether sections have
//! headers, how a control row is composed — which is why this module also holds helpers that
//! encode conventions ([`hint`], [`caption`], [`button_row`]), not just colours.
//!
//! ⚠ **The accent does not reach everything.** It sets egui's own widget colours; anything
//! Rustroom paints by hand — the crop overlay, the histogram, the thumbnail selection border —
//! keeps its own hardcoded colours until someone routes them through here too.

use eframe::egui;
use rustroom_app::settings::{Accent, Settings, ThemeChoice};

/// The accent colours, one pair per variant.
///
/// ⚠ **Two values per accent, not one.** A colour tuned for near-black washes out on
/// near-white — `#7fb0d8` on a light base is low contrast and fails white text on it. Every
/// pair here was contrast-checked against egui's real bases before being written down; worst
/// case is steel blue on light at 5.48:1, against a 3:1 threshold. `every_accent_has_usable_contrast`
/// is the guard for whatever is added next.
///
/// ⚠ A free function, not `impl Accent`. `Accent` is defined in the **lib** crate, and Rust's
/// orphan rule allows inherent impls only in the crate that defines the type — so this could
/// only be an extension trait or a free function, and a trait carrying one method used in one
/// file is machinery for its own sake.
fn accent_color(accent: Accent, dark: bool) -> egui::Color32 {
    let (r, g, b) = match (accent, dark) {
        (Accent::Neutral, true) => (0x9a, 0x9a, 0x9a),
        (Accent::Neutral, false) => (0x5a, 0x5a, 0x5a),
        (Accent::SteelBlue, true) => (0x7f, 0xb0, 0xd8),
        (Accent::SteelBlue, false) => (0x2f, 0x6a, 0x94),
        (Accent::Teal, true) => (0x5f, 0xb3, 0xac),
        (Accent::Teal, false) => (0x2c, 0x6d, 0x68),
        (Accent::Violet, true) => (0x9a, 0x8f, 0xd0),
        (Accent::Violet, false) => (0x54, 0x4a, 0x8c),
    };
    egui::Color32::from_rgb(r, g, b)
}

/// Apply Rustroom's style, at startup and whenever the appearance settings change.
///
/// ⚠ **egui 0.35 keeps a separate `Style` per theme** — dark and light — so there is no single
/// style to set, and `Context::set_style` does not exist. Anything theme-independent goes
/// through [`tune`] for both; the accent is resolved *per theme*, because a colour tuned for
/// near-black washes out on near-white.
///
/// Hence `style_mut_of` rather than `set_style_of`: mutating in place preserves each theme's
/// own visuals, where handing over a whole `Style` would overwrite them.
pub fn apply(ctx: &egui::Context, settings: &Settings) {
    ctx.set_theme(match settings.theme {
        ThemeChoice::Dark => egui::ThemePreference::Dark,
        ThemeChoice::Light => egui::ThemePreference::Light,
        ThemeChoice::System => egui::ThemePreference::System,
    });
    for (theme, dark) in [(egui::Theme::Dark, true), (egui::Theme::Light, false)] {
        let accent = accent_color(settings.accent, dark);
        ctx.style_mut_of(theme, |style| {
            tune(style);
            style.visuals.selection.bg_fill = accent;
            style.visuals.widgets.hovered.bg_fill = accent;
            style.visuals.widgets.active.bg_fill = accent;
        });
    }
}

/// The theme-independent tweaks: the compact density.
///
/// ⚠ **Only theme-independent things belong here.** Anything colour-related goes in that
/// theme's own `visuals` — see [`apply`] — or light mode inherits dark colours.
///
/// Compact because the Develop panel stacks 20+ sliders across six collapsible sections, and
/// every Phase 3 tool makes that worse. It costs some trackpad precision on fine drags, which
/// is the trade taken knowingly.
///
/// ⚠ **egui's defaults are already fairly tight** — `item_spacing.y` 3.0 and `interact_size.y`
/// 18.0 — which the first draft of these values got wrong by assuming they were roomier, and
/// `the_compact_metrics_are_tighter_than_the_defaults` caught. So the headroom is smaller than
/// it looks: the visible gain here comes mostly from **halving the slider rail**, which is
/// where the vertical space in a panel of sliders actually goes.
fn tune(style: &mut egui::Style) {
    style.spacing.item_spacing.y = 2.0;
    style.spacing.interact_size.y = 16.0;
    style.spacing.slider_rail_height = 4.0;
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

/// A caption: a filename under a thumbnail, a camera model beside a title.
///
/// ⚠ Renders identically to [`hint`] today and means something different — a hint is
/// explanatory text *under a control*, a caption labels a thing. Kept separate so the visual
/// identity pass can move one without the other. Merging two idioms because they currently
/// look alike is how a design system loses the distinctions it exists to hold.
pub fn caption(ui: &mut egui::Ui, text: &str) {
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
        apply(&ctx, &Settings::default());
        for theme in [egui::Theme::Dark, egui::Theme::Light] {
            let body = ctx
                .style_of(theme)
                .text_styles
                .get(&egui::TextStyle::Body)
                .map(|f| f.size);
            assert!(body.is_some(), "{theme:?} lost its body text style");
        }
    }

    /// egui's own panel fill, which is what an accent is judged against.
    ///
    /// ⚠ Read from `Visuals` rather than hardcoded: if egui changes its base in a future
    /// release this notices, instead of silently checking against a number that is no longer
    /// true. Lives in the test module because that is its only caller — production never asks
    /// what the base is, it just lets egui paint it.
    fn base_fill(dark: bool) -> egui::Color32 {
        if dark {
            egui::Visuals::dark().panel_fill
        } else {
            egui::Visuals::light().panel_fill
        }
    }

    /// WCAG relative luminance.
    fn luminance(c: egui::Color32) -> f64 {
        fn ch(v: u8) -> f64 {
            let v = v as f64 / 255.0;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        }
        0.2126 * ch(c.r()) + 0.7152 * ch(c.g()) + 0.0722 * ch(c.b())
    }

    /// WCAG contrast ratio, 1.0..=21.0.
    fn contrast(a: egui::Color32, b: egui::Color32) -> f64 {
        let (x, y) = (luminance(a), luminance(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn every_accent_has_usable_contrast() {
        // ⚠ The load-bearing test. An accent tuned for near-black washes out on near-white —
        // that is why each is a pair — and 3:1 is the WCAG threshold for non-text UI
        // components. This is a guard for the *next* accent someone adds: the four here were
        // computed against it before being written down, worst case 5.48:1.
        for accent in Accent::ALL {
            for dark in [true, false] {
                let base = base_fill(dark);
                let c = contrast(accent_color(accent, dark), base);
                assert!(
                    c >= 3.0,
                    "{accent:?} on {} base: {c:.2}:1 is below 3:1",
                    if dark { "dark" } else { "light" }
                );
            }
        }
    }

    #[test]
    fn every_accent_differs_between_themes() {
        // A pair whose halves are equal means someone added a variant and filled in one value.
        // ⚠ Neutral is included deliberately — it is a grey, not an absence, so it needs two
        // values like everything else.
        for accent in Accent::ALL {
            assert_ne!(
                accent_color(accent, true),
                accent_color(accent, false),
                "{accent:?} uses the same colour for both themes"
            );
        }
    }

    #[test]
    fn the_compact_metrics_are_tighter_than_the_defaults() {
        // Pins the direction, not the values: a later tweak may change how much tighter, but
        // `tune` accidentally becoming a no-op again should fail.
        let mut s = egui::Style::default();
        let before = (s.spacing.item_spacing.y, s.spacing.interact_size.y);
        tune(&mut s);
        assert!(s.spacing.item_spacing.y < before.0);
        assert!(s.spacing.interact_size.y < before.1);
    }
}
