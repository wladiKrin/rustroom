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

/// The bundled icon font.
///
/// ⚠ Pinned to Phosphor 2.1.2, whose licence sits beside it. See `rustroom_app::icons`.
const ICON_FONT: &[u8] = include_bytes!("../assets/Phosphor.ttf");

/// The bundled text face.
///
/// ⚠ Inter 4.1 (OFL; licence beside it), pinned. egui's own Ubuntu Light is a light weight that
/// reads thin on grey — the first-use reviews called the type "faintly amateur" — and a regular
/// weight is the cheapest single change toward a finished-looking tool. Inter also carries the
/// keyboard glyphs (⇧ ⌘ ⌥ ⌃ → ←) Ubuntu Light lacks, which is what had the shortcuts dialog and
/// the Library's empty state drawing tofu boxes. Coverage was checked by parsing its cmap.
///
/// ⚠ Subset: the private-use block U+E000–U+F8FF is stripped (`pyftsubset --unicodes=
/// "U+0000-U+DFFF,U+F900-U+10FFFF"`), because the original claims ten of the icon codepoints
/// and would draw them instead of Phosphor — see `PROPORTIONAL`.
const TEXT_FONT: &[u8] = include_bytes!("../assets/Inter-Regular.ttf");

/// The proportional family, first to last.
///
/// ⚠⚠ **The order is the whole point.** egui takes the first font that has a glyph, and both
/// Inter and egui's `emoji-icon-font` define codepoints inside Phosphor's private-use block —
/// Inter's own U+E08A drew where the undo arrow belonged, and the emoji font's U+E626 had been
/// drawing the "zoom to fit" frame as a blob since the day it was added. So Phosphor sits
/// **ahead** of every font that claims one of its codepoints and behind only the fonts that
/// claim none, and Inter ships here with its private-use block removed (`pyftsubset`, 4.1).
/// `no_bundled_font_shadows_an_icon` checks the real fonts, not this comment.
///
/// ⚠ Phosphor cannot go first: it has glyphs for the space and the letter `a`.
const PROPORTIONAL: [&str; 6] = [
    "Inter",
    "Ubuntu-Light",
    "NotoEmoji-Regular",
    "phosphor",
    "emoji-icon-font",
    "Hack",
];

/// The monospace family. Hack first, so it keeps its text; nothing draws an icon in monospace.
const MONOSPACE: [&str; 5] = [
    "Hack",
    "Ubuntu-Light",
    "NotoEmoji-Regular",
    "phosphor",
    "emoji-icon-font",
];

/// Install the text face, the fallbacks and the icon font. Call once, at startup.
pub fn install_fonts(ctx: &egui::Context) {
    ctx.set_fonts(font_definitions());
}

/// The whole stack as one value, so the tests can take it apart font by font.
///
/// ⚠ Starts from `FontDefinitions::default()` for the font *data* — egui's four faces — and then
/// sets both families explicitly, because the order is what `PROPORTIONAL` documents.
pub(crate) fn font_definitions() -> egui::FontDefinitions {
    let mut defs = egui::FontDefinitions::default();
    defs.font_data.insert(
        "Inter".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(TEXT_FONT)),
    );
    defs.font_data.insert(
        "phosphor".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(ICON_FONT)),
    );
    defs.families.insert(
        egui::FontFamily::Proportional,
        PROPORTIONAL.iter().map(|n| (*n).to_owned()).collect(),
    );
    defs.families.insert(
        egui::FontFamily::Monospace,
        MONOSPACE.iter().map(|n| (*n).to_owned()).collect(),
    );
    defs
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
    // The whole interface, photo included, at the chosen size; egui re-lays everything out.
    ctx.set_zoom_factor(settings.ui_scale.factor());
    ctx.set_theme(match settings.theme {
        ThemeChoice::Dark => egui::ThemePreference::Dark,
        ThemeChoice::Light => egui::ThemePreference::Light,
        ThemeChoice::System => egui::ThemePreference::System,
    });
    for (theme, dark) in [(egui::Theme::Dark, true), (egui::Theme::Light, false)] {
        let accent = accent_color(settings.accent, dark);
        ctx.style_mut_of(theme, |style| {
            tune(style);
            surfaces(style, dark);
            style.visuals.selection.bg_fill = accent;
            style.visuals.widgets.hovered.bg_fill = accent;
            style.visuals.widgets.active.bg_fill = accent;
        });
    }
}

/// The theme's surfaces: the panel, the popups, body text and hairlines.
///
/// ⚠ **Per theme, never in [`tune`]** — every value here is a pair, the visual identity
/// spec's rule. Dark mode is the one that moves: egui's `gray(140)` body text on `gray(27)`
/// is 4.6:1, readable but dim beside the `gray(180)` button text next to it, and every surface
/// was the same `gray(27)` so nothing read as a panel. Light mode's `gray(80)` on `gray(248)`
/// is already 7.5:1 and keeps its text; only its hairlines soften.
///
/// ⚠ Separators get *weaker*, not stronger: section headers carry the structure now, so the
/// hairlines become guides rather than walls.
fn surfaces(style: &mut egui::Style, dark: bool) {
    let v = &mut style.visuals;
    if dark {
        let panel = egui::Color32::from_gray(32);
        v.panel_fill = panel;
        v.window_fill = panel;
        v.widgets.noninteractive.bg_fill = panel;
        v.widgets.noninteractive.weak_bg_fill = panel;
        v.widgets.noninteractive.fg_stroke.color = egui::Color32::from_gray(166);
        v.widgets.noninteractive.bg_stroke.color = egui::Color32::from_gray(46);
    } else {
        v.widgets.noninteractive.bg_stroke.color = egui::Color32::from_gray(205);
    }
}

/// The Develop canvas surround: darker than the panel in both themes.
///
/// ⚠ The panel is a surface the image is judged *from*; the canvas is the surround it is
/// judged *against*, and the visual identity spec's argument — low chroma **near the image** —
/// is what this serves. ⚠ Only the Develop canvas takes it; the Library grid is a browsing
/// surface and keeps `panel_fill`.
pub fn canvas_fill(dark: bool) -> egui::Color32 {
    if dark {
        egui::Color32::from_gray(22)
    } else {
        egui::Color32::from_gray(222)
    }
}

/// The frame the Develop canvas is drawn in.
pub fn canvas_frame(style: &egui::Style) -> egui::Frame {
    egui::Frame::central_panel(style).fill(canvas_fill(style.visuals.dark_mode))
}

/// The inner margin of the right panel and the snapshots strip.
///
/// ⚠ egui's `side_top_panel` frame has a much smaller one, which is what left content flush
/// against the panel edge. Deliberately not applied through `tune`'s spacing, which every
/// menu and popup shares.
pub const PANEL_MARGIN: egui::Margin = egui::Margin {
    left: 10,
    right: 10,
    top: 6,
    bottom: 6,
};

/// The frame the right panel and the snapshots strip are drawn in.
pub fn panel_frame(style: &egui::Style) -> egui::Frame {
    egui::Frame::side_top_panel(style).inner_margin(PANEL_MARGIN)
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

    // One radius for every widget state. egui ships 2/2/3/2/2 — hovered widgets change shape
    // as well as colour, which reads as a flicker on a row of chips.
    let w = &mut style.visuals.widgets;
    for ws in [
        &mut w.noninteractive,
        &mut w.inactive,
        &mut w.hovered,
        &mut w.active,
        &mut w.open,
    ] {
        ws.corner_radius = egui::CornerRadius::same(4);
    }

    // The section header and sub-label face: smaller than body, uppercased by its callers.
    style
        .text_styles
        .insert(section_style(), egui::FontId::proportional(SECTION_PT));

    // ⚠ Both of these are **off by default in egui**, and their absence is why the shipped
    // sliders looked nothing like the mockups they were chosen from — a grey rectangle on a
    // grey rail rather than a coloured track with a dot.
    //
    // `slider_trailing_fill` paints the rail left of the handle with `selection.bg_fill`,
    // which is already the accent — so the accent ends up carrying "where is this set",
    // which is the signal the comparison was actually judged on.
    style.visuals.slider_trailing_fill = true;
    style.visuals.handle_shape = egui::style::HandleShape::Circle;

    // ⚠ Not done, deliberately: colouring the handle *at rest* would mean setting
    // `widgets.inactive.bg_fill`, which is shared by every button **and** by the slider rail
    // itself — so the rail and the whole toolbar would go accent-coloured too. The trailing
    // fill already carries the signal.
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

/// A [`hint`] that says what is *wrong* with what was typed.
///
/// ⚠ Same size as a hint and a different colour, because it sits in the same place under the
/// same control — a validation message that looked identical to an explanation would be read as
/// one. ⚠ `error_fg_color` rather than a constant: this app tunes dark and light separately, and
/// a hardcoded red is the mistake the crop overlay's fixed colours already document.
pub fn error_hint(ui: &mut egui::Ui, text: &str) {
    let colour = ui.visuals().error_fg_color;
    ui.label(egui::RichText::new(text).small().color(colour));
}

/// The "there is nothing here yet" line filling an empty canvas or grid.
///
/// ⚠⚠ **`selectable(false)` is the whole reason this exists.** egui's labels are selectable by
/// default, so a `Label` senses click-and-drag for text selection and shows the **I-beam over its
/// whole rect** — and inside `centered_and_justified` that rect is the *entire panel*. The empty
/// Library therefore turned the cursor into a text caret across the whole window, which reads as
/// the app being in some editing state it is not.
///
/// ⚠ Fixed here rather than by turning `interaction.selectable_labels` off globally: selection is
/// wanted on ordinary labels, and it is *justification* that makes it a problem.
pub fn empty_state(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add(egui::Label::new(text).selectable(false))
}

/// Whether a name field was just committed with Enter.
///
/// ⚠ **Enter, not a bare `lost_focus()`** — clicking away is abandoning, not committing, the rule
/// the copy-rename field already follows.
///
/// ⚠ Written once so every in-place name field — collection, keyword, smart collection,
/// snapshot — cannot disagree about what commits. Two guards for one rule is how they drift.
pub fn committed(ui: &egui::Ui, resp: &egui::Response) -> bool {
    resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
}

/// The named text style section headers and sub-labels use.
///
/// ⚠ A named style rather than `Small`: `Small` is the hint face at 9 pt, and a header at hint
/// size is not a header.
pub fn section_style() -> egui::TextStyle {
    egui::TextStyle::Name("section".into())
}

/// The section face's size in points.
const SECTION_PT: f32 = 11.0;

/// The section face, resolved against `style` — or the same face by value when the style
/// was never tuned.
///
/// ⚠⚠ **The fallback is load-bearing, and 77 tests said so.** `TextStyle::resolve` panics on
/// a name the style does not carry, and every headless harness builds a bare
/// `egui::Context` that never runs [`apply`]. Resolving through this means a widget drawn in
/// a harness gets the same 11 pt face production gets, rather than a panic that reads as
/// seventy-seven unrelated failures.
pub fn section_font(style: &egui::Style) -> egui::FontId {
    style
        .text_styles
        .get(&section_style())
        .cloned()
        .unwrap_or_else(|| egui::FontId::proportional(SECTION_PT))
}

/// A sub-label: the name of a block of controls inside a section — Presence, Noise Reduction,
/// Color Grading. Uppercased, in the section face, in the weak text colour, with a little air
/// above and no hairline: the hairline was the old idiom and the name replaces it.
///
/// ⚠ Not for a value ("No presets yet.") and not for a control's own text; those stay
/// `label`s. Plain labels produce no AccessKit node, so nothing in the tests reaches these.
pub fn sub_label(ui: &mut egui::Ui, text: &str) {
    let _ = sub_label_response(ui, text);
}

/// [`sub_label`] with a hover explaining what the block is.
///
/// ⚠ The plain-language name is what a beginner can act on; the jargon it replaces — `HSL`,
/// `color grading` — is what an experienced user searches for, so it goes here rather than being lost.
/// The idiom the Settings dialog already uses for its descriptions.
pub fn sub_label_hover(ui: &mut egui::Ui, text: &str, hover: &str) {
    sub_label_response(ui, text).on_hover_text(hover);
}

/// The one body both go through, so the two cannot drift apart in spacing, case or colour.
fn sub_label_response(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.add_space(4.0);
    let colour = ui.visuals().weak_text_color();
    let font = section_font(ui.style());
    ui.add(
        egui::Label::new(
            egui::RichText::new(text.to_uppercase())
                .font(font)
                .color(colour),
        )
        .selectable(false),
    )
}

/// A sub-label with `trailing` content right-aligned on the same row — a view toggle or a
/// button that belongs to the block: Color Grading's "Show range", the tone curve's "Reset".
///
/// ⚠⚠ **A row pinned to the width it was given, with the label painted rather than added.**
/// The first version was `horizontal` → label → `with_layout(right_to_left)`, and `sub_label`
/// was a call to it with nothing trailing — which made every sub-label a **full-width row**.
/// `tone_curve::show` then nested one inside its own `horizontal` beside a right-aligned
/// button, and that row came out one `item_spacing` wider than the panel; egui widened the
/// panel to fit, the row widened with it, and the panel grew 8 pt **every frame** until it hit
/// the window ("the sidecar keeps expanding whenever I move the mouse", 2026-09-19). Two
/// rules fall out of it: a plain `sub_label` is a plain label, and a row with right-aligned
/// content is allocated at an explicit size so nothing inside it can ask for more.
///
/// ⚠ The trailing closure runs in a right-to-left layout, so its first widget lands at the
/// right edge.
pub fn sub_label_with(ui: &mut egui::Ui, text: &str, trailing: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(4.0);
    let colour = ui.visuals().weak_text_color();
    let font = section_font(ui.style());
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_uppercase(), font, colour);
    let height = galley.size().y.max(ui.spacing().interact_size.y);
    let row = egui::Rect::from_min_size(
        ui.next_widget_position(),
        egui::vec2(ui.available_width(), height),
    );
    ui.allocate_ui_with_layout(
        row.size(),
        egui::Layout::right_to_left(egui::Align::Center),
        trailing,
    );
    ui.painter().galley(
        egui::pos2(row.left(), row.center().y - galley.size().y / 2.0),
        galley,
        colour,
    );
}

/// The segmented controls live in [`crate::segmented`]; re-exported so the panels' call sites
/// read `theme::segmented` as they always did.
pub use crate::segmented::{segmented, segmented_sized};

/// How wide a dialog wants to be.
///
/// ⚠⚠ **`egui::Modal` has no width knob at all** — its only sizing surface is `frame` and `area`,
/// so width comes entirely from content. That is *why* every dialog in this app was a different
/// width. The class is the app's own answer, named at the call site and resolved here.
///
/// ⚠ Three rather than one, because a two-button "Delete preset?" at the rule editor's width reads
/// as padding rather than design. Three rather than two, because Settings and Information want to
/// sit between a confirmation and the widest thing in the app.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DialogSize {
    /// A confirmation: a sentence and two buttons.
    Compact,
    /// A form: a handful of labelled controls.
    Standard,
    /// Dense or long content — a rule, a shortcut list, a metadata table.
    Wide,
}

impl DialogSize {
    /// Every class, narrowest first. ⚠ The tests iterate this, so a variant missing from it is
    /// unguarded.
    ///
    /// ⚠ `#[cfg(test)]` because `theme` is a **binary** module, where a `pub` item with no
    /// production caller is a `dead_code` error under `clippy -D warnings` — the same reason
    /// `base_fill` below lives in the test module. Nothing in production needs to enumerate the
    /// classes; each call site names exactly one.
    #[cfg(test)]
    pub const ALL: [DialogSize; 3] = [Self::Compact, Self::Standard, Self::Wide];

    /// The smallest this class may be dragged to.
    ///
    /// ⚠ A floor, not a target: the width is what the class actually asserts, and the height only
    /// stops a dialog being dragged shut.
    pub fn min(self) -> (f32, f32) {
        match self {
            Self::Compact => (300.0, 80.0),
            Self::Standard => (400.0, 120.0),
            Self::Wide => (520.0, 200.0),
        }
    }

    /// The size a dialog of this class opens at when it has never been resized.
    pub fn default_size(self) -> (f32, f32) {
        match self {
            Self::Compact => (340.0, 140.0),
            Self::Standard => (480.0, 320.0),
            Self::Wide => (640.0, 480.0),
        }
    }
}

/// The inner margin every dialog's frame gets.
///
/// ⚠⚠ **`egui::Modal`'s default frame is `Frame::popup`, whose inner margin is
/// `style.spacing.menu_margin` — `Margin::same(6)`.** That is egui's default, not a choice this
/// project made, and it is the bulk of the reported "very small and cramped".
///
/// ⚠ Applied per modal rather than by raising `menu_margin` in [`tune`], which is shared with
/// every real context menu and would fatten those too.
pub const DIALOG_MARGIN: i8 = 14;

/// The frame a dialog is drawn in.
///
/// ⚠ `style_of(ctx.theme())`, not `global_style()`: a frame's fill, stroke and shadow are all
/// theme-dependent, and this app tunes dark and light **separately** — see [`apply`]. The global
/// style would give light mode the wrong frame. (`Context::style()` no longer exists in egui
/// 0.35; `Modal`'s own default uses the `Ui`'s style, which is not available before the modal
/// opens.)
pub fn dialog_frame(ctx: &egui::Context) -> egui::Frame {
    egui::Frame::popup(&ctx.style_of(ctx.theme())).inner_margin(DIALOG_MARGIN)
}

/// The accent-filled button with white text: a dialog footer's primary verb, the bar's Export.
///
/// ⚠ One painter for both, so the bar and the dialogs cannot drift apart. ⚠ White on the
/// accent in both themes: every accent pair was contrast-checked against white text
/// (`every_accent_has_usable_contrast`). A disabled one is drawn greyed, never hidden — the
/// Save button with an empty name must still be seen.
pub fn primary_button(ui: &mut egui::Ui, text: &str, enabled: bool) -> egui::Response {
    let accent = ui.visuals().selection.bg_fill;
    let (fill, colour) = if enabled {
        (accent, egui::Color32::WHITE)
    } else {
        (accent.gamma_multiply(0.35), ui.visuals().weak_text_color())
    };
    ui.add_enabled(
        enabled,
        egui::Button::new(egui::RichText::new(text).color(colour)).fill(fill),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_state_label_does_not_claim_the_text_cursor() {
        // ⚠⚠ egui's labels are **selectable by default**, so a `Label` senses click-and-drag for
        // text selection and shows the I-beam over its whole rect — and inside
        // `centered_and_justified` that rect is the *entire panel*. The empty Library therefore
        // turned the cursor into a text caret across the whole window.
        let ctx = egui::Context::default();
        let mut ours = None;
        let mut plain = None;
        let _ = ctx.run_ui(egui::RawInput::default(), |ui| {
            ui.centered_and_justified(|ui| {
                ours = Some(empty_state(ui, "nothing here").sense);
            });
            plain = Some(ui.label("an ordinary label").sense);
        });
        assert!(
            !ours.unwrap().senses_drag(),
            "an empty-state label must not sense a drag, or it owns the cursor"
        );
        // ⚠ The contrast is what makes the assertion mean something: an ordinary label *does*
        // sense it, so the test is about `selectable(false)` rather than about egui's defaults.
        assert!(
            plain.unwrap().senses_drag(),
            "an ordinary label still selects — this test would say nothing otherwise"
        );
    }

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

    /// ⚠⚠ Every non-ASCII character in a menu label, a shortcut label, a tool name or one of the
    /// fixed hint strings must be drawable by the installed stack, or it shows as a tofu box —
    /// which is how the empty Library said "File □ Open folder…" and the shortcuts dialog wrote
    /// "□⌘O" for a shift chord, on the very first screen a new user sees. The sample size is
    /// asserted first so a model that came back empty could not pass.
    #[test]
    fn every_menu_string_is_renderable() {
        use rustroom_app::menu::Item;
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        // A frame must run first: a fresh `Context` has not built its atlas.
        let _ = ctx.run_ui(egui::RawInput::default(), |_ui| {});
        let id = egui::FontId::proportional(14.0);
        let mut strings: Vec<String> = vec![
            crate::multi::compare::INSTRUCTION.to_owned(),
            crate::multi::survey::INSTRUCTION.to_owned(),
            crate::library::EMPTY_LIBRARY_HINT.to_owned(),
        ];
        strings.extend(
            rustroom_app::tool::Tool::ALL
                .iter()
                .map(|t| t.label().to_owned()),
        );
        for (title, lines) in crate::modals::EXTRA_KEYS {
            strings.push((*title).to_owned());
            for (what, key) in *lines {
                strings.push((*what).to_owned());
                strings.push((*key).to_owned());
            }
        }
        let recent = [std::path::PathBuf::from("/tmp/Bilder — Katzen")];
        for section in rustroom_app::menu::model_with(&recent) {
            strings.push(section.title.to_owned());
            rustroom_app::menu::walk(&section.items, &mut |item| match item {
                Item::Action {
                    label, shortcut, ..
                } => {
                    strings.push(label.clone());
                    if let Some(sc) = shortcut {
                        strings.push(sc.label());
                    }
                }
                Item::Submenu { label, .. } => strings.push(label.clone()),
                Item::Separator => {}
            });
        }
        assert!(strings.len() > 60, "sample: {}", strings.len());
        let symbols: Vec<char> = strings
            .iter()
            .flat_map(|s| s.chars())
            .filter(|c| !c.is_ascii())
            .collect();
        assert!(symbols.len() > 10, "sample: {} symbols", symbols.len());
        for s in &strings {
            for c in s.chars().filter(|c| !c.is_ascii()) {
                assert!(
                    ctx.fonts_mut(|f| f.has_glyph(&id, c)),
                    "{c:?} (U+{:04X}) in {s:?} is in no bundled font and would draw as a box",
                    c as u32
                );
            }
        }
    }

    /// ⚠⚠ Every icon must come from Phosphor, not from a text font that happens to define the
    /// same private-use codepoint — Inter drew the undo and redo arrows, the crop, the retouch and
    /// the filter glyphs as its own shapes the day it became the text face, and egui's emoji
    /// font had been drawing "zoom to fit" wrong all along. Checked against the **real cmap
    /// tables** of the fonts in production order: the first that maps the codepoint must be
    /// Phosphor. The letter `a` is the positive control (Phosphor has one; Inter must win), and
    /// ▴ pins that Hack is still reachable behind the icons.
    ///
    /// ⚠ Not `Fonts::has_glyph`: in egui 0.35 that compares the resolving *face* with the
    /// replacement face, so a single-font stack answers `false` for every character it has.
    #[test]
    fn no_bundled_font_shadows_an_icon() {
        use read_fonts::{FontRef, TableProvider};
        let defs = font_definitions();
        let order = &defs.families[&egui::FontFamily::Proportional];
        let maps = |name: &str, c: char| {
            let data = &defs.font_data[name].font;
            let font = FontRef::new(data).unwrap_or_else(|e| panic!("{name}: {e}"));
            font.cmap()
                .is_ok_and(|cmap| cmap.map_codepoint(c as u32).is_some())
        };
        let first_with = |c: char| order.iter().find(|n| maps(n, c)).map(String::as_str);
        assert!(rustroom_app::icons::ALL.len() > 20, "sample");
        for c in rustroom_app::icons::ALL {
            assert_eq!(
                first_with(c),
                Some("phosphor"),
                "U+{:04X} would be drawn by the wrong font",
                c as u32
            );
        }
        assert_eq!(first_with('a'), Some("Inter"));
        assert_eq!(first_with('▴'), Some("Hack"));
        assert_eq!(
            first_with('\u{2715}'),
            None,
            "✕ is in no bundled font — the control"
        );
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

    /// The panel fill each theme actually ends up with — what an accent is judged against.
    ///
    /// ⚠ Read through `apply`, not from `Visuals::dark()`: since the surfaces pass the panel
    /// is no longer egui's default, and checking accents against a base nobody paints would
    /// say nothing. Lives in the test module because that is its only caller — production
    /// never asks what the base is, it just paints it.
    fn base_fill(dark: bool) -> egui::Color32 {
        let ctx = egui::Context::default();
        apply(&ctx, &Settings::default());
        let theme = if dark {
            egui::Theme::Dark
        } else {
            egui::Theme::Light
        };
        ctx.style_of(theme).visuals.panel_fill
    }

    #[test]
    fn body_text_is_readable_on_the_panel() {
        // ⚠ 4.5:1 is WCAG AA for text, a stricter bar than the accents' 3:1: a slider label
        // is read, an accent is only noticed. Dark text sat at 4.6:1 before this pass.
        let ctx = egui::Context::default();
        apply(&ctx, &Settings::default());
        for (theme, dark) in [(egui::Theme::Dark, true), (egui::Theme::Light, false)] {
            let v = ctx.style_of(theme).visuals.clone();
            let c = contrast(v.widgets.noninteractive.fg_stroke.color, v.panel_fill);
            assert!(
                c >= 4.5,
                "{} body text on its panel: {c:.2}:1 is below 4.5:1",
                if dark { "dark" } else { "light" }
            );
        }
    }

    #[test]
    fn the_canvas_is_darker_than_the_panel_in_both_themes() {
        // ⚠ Pins the *direction* per theme, so a swapped pair — the mistake the accent pairs
        // exist to prevent — cannot ship. The surround argument is the visual identity spec's:
        // the darkest, most neutral surface is the one touching the photograph — in light mode
        // too, where the spec's first draft said "lighter" and this test said otherwise.
        for dark in [true, false] {
            assert!(
                luminance(canvas_fill(dark)) < luminance(base_fill(dark)),
                "dark={dark}: the canvas must be darker than the panel"
            );
        }
    }

    #[test]
    fn the_panel_frame_has_an_inner_margin() {
        // A frame with egui's default margin is what left "Camera profile" flush against
        // the panel edge.
        let s = tuned();
        let m = panel_frame(&s).inner_margin;
        assert!(m.left >= 8 && m.right >= 8, "margin {m:?}");
    }

    #[test]
    fn the_section_style_is_installed_in_both_themes() {
        let ctx = egui::Context::default();
        apply(&ctx, &Settings::default());
        for theme in [egui::Theme::Dark, egui::Theme::Light] {
            let s = ctx.style_of(theme);
            let section = s.text_styles.get(&section_style()).map(|f| f.size);
            let body = s.text_styles[&egui::TextStyle::Body].size;
            assert!(
                section.is_some_and(|x| x < body),
                "{theme:?}: section {section:?} vs body {body}"
            );
        }
    }

    #[test]
    fn every_segment_is_reachable_by_its_label_and_a_click_selects_only_it() {
        #[derive(Clone, Copy, PartialEq, Debug)]
        enum C {
            A,
            B,
            D,
        }
        let items = [(C::A, "Hue"), (C::B, "Saturation"), (C::D, "Luminance")];
        let ctx = crate::test_ui::ctx();
        let mut cur = C::A;
        let found = crate::test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
            segmented(ui, &mut cur, &items);
        });
        for (_, l) in items {
            assert!(
                found.iter().any(|(f, _)| f == l),
                "{l} not reachable: {found:?}"
            );
        }
        crate::test_ui::click(&ctx, "Luminance", |ui| {
            segmented(ui, &mut cur, &items);
        });
        assert_eq!(cur, C::D);
        crate::test_ui::click(&ctx, "Saturation", |ui| {
            segmented(ui, &mut cur, &items);
        });
        assert_eq!(cur, C::B);
    }

    #[test]
    fn a_segmented_control_wraps_rather_than_overlapping_its_labels() {
        // ⚠ Both widths: a control that always wrapped would pass the narrow half, one that
        // never wrapped the wide half — and the wide half is the panel at a comfortable size.
        let items = [
            (0u8, "New mask", ""),
            (1, "Add", ""),
            (2, "Subtract", ""),
            (3, "Intersect", ""),
        ];
        let rows_at = |width: f32| {
            let ctx = crate::test_ui::ctx();
            let mut cur = 0u8;
            let found = crate::test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
                ui.allocate_ui(egui::vec2(width, 200.0), |ui| {
                    let font = egui::TextStyle::Body.resolve(ui.style());
                    segmented_sized(ui, &mut cur, &items, 28.0, font);
                });
            });
            let mut ys: Vec<i32> = found.iter().map(|(_, p)| p.y.round() as i32).collect();
            ys.sort_unstable();
            ys.dedup();
            (found.len(), ys.len())
        };
        assert_eq!(rows_at(600.0), (4, 1), "wide: one row");
        assert_eq!(
            rows_at(220.0),
            (4, 2),
            "the panel's default width: two rows of two"
        );
    }

    #[test]
    fn every_widget_state_shares_one_corner_radius() {
        let s = tuned();
        let w = &s.visuals.widgets;
        for ws in [
            &w.noninteractive,
            &w.inactive,
            &w.hovered,
            &w.active,
            &w.open,
        ] {
            assert_eq!(ws.corner_radius, egui::CornerRadius::same(4));
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

    #[test]
    fn the_icon_font_is_registered() {
        // ⚠ Without this, a font that failed to embed — a truncated download, a woff2 that
        // `ab_glyph` cannot parse — would ship as tofu boxes with nothing failing at build time.
        // ⚠ A frame must run before the atlas exists; a fresh `Context` has not built one.
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        let _ = ctx.run_ui(egui::RawInput::default(), |_ui| {});
        let id = egui::FontId::proportional(14.0);
        assert!(
            ctx.fonts_mut(|f| f.has_glyph(&id, rustroom_app::icons::ZOOM_IN)),
            "the icon font is not in the stack"
        );
    }

    #[test]
    fn installing_the_icon_font_keeps_the_ordinary_ones() {
        // ⚠ The failure mode of getting this wrong is total: replacing the font stack rather than
        // adding to it leaves every label in the app unrenderable, which no icon test would catch.
        let ctx = egui::Context::default();
        install_fonts(&ctx);
        let _ = ctx.run_ui(egui::RawInput::default(), |_ui| {});
        let id = egui::FontId::proportional(14.0);
        assert!(
            ctx.fonts_mut(|f| f.has_glyph(&id, 'A')),
            "lost the Latin font"
        );
    }

    /// ⚠ A table test over **every** variant, so a fourth class cannot ship inconsistent.
    #[test]
    fn every_dialog_class_defaults_to_at_least_its_own_minimum() {
        for class in DialogSize::ALL {
            let (min_w, min_h) = class.min();
            let (def_w, def_h) = class.default_size();
            assert!(
                def_w >= min_w && def_h >= min_h,
                "{class:?} defaults to {def_w}x{def_h}, below its minimum {min_w}x{min_h}"
            );
        }
    }

    /// ⚠ The classes must actually be ordered, or naming one at a call site means nothing.
    #[test]
    fn the_dialog_classes_get_wider_in_order() {
        let w: Vec<f32> = DialogSize::ALL.iter().map(|c| c.min().0).collect();
        assert!(
            w.windows(2).all(|p| p[0] < p[1]),
            "minimum widths must strictly increase, got {w:?}"
        );
    }
}
