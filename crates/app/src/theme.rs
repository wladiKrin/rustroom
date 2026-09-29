//! The one place Rustroom's look is defined.
//!
//! Dark by default with a switchable accent, light available as a setting. The rulebook is
//! `docs/DESIGN.md` (direction B, chosen 2026-09-29); its values live in [`tokens`].
//!
//! The split that makes colour cheap to revisit: anything expressible in
//! `egui::Style`/`Visuals` is a central change applied retroactively everywhere. What is
//! expensive to revisit is *structural* — whether buttons carry icons, whether sections have
//! headers, how a control row is composed — which is why this module also holds helpers that
//! encode conventions ([`hint`], [`caption`], [`button_row`]), not just colours.
//!
//! ⚠ **Hand-painted colour reads the tokens** (`tokens::of(ui)`), never a literal grey. The
//! exceptions — the histogram's channels, the crop frame, the mask and clipping overlays, the
//! red-eye circle — are listed in `docs/DESIGN.md` §4.2.

use eframe::egui;
use rustroom_app::settings::{Accent, Settings, ThemeChoice};

pub mod tokens;
pub use tokens::on_accent;

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
pub fn accent_color(accent: Accent, dark: bool) -> egui::Color32 {
    let (r, g, b) = match (accent, dark) {
        (Accent::Neutral, true) => (0x9a, 0x9a, 0x9a),
        (Accent::Neutral, false) => (0x5a, 0x5a, 0x5a),
        (Accent::SteelBlue, true) => (0x7f, 0xb0, 0xd8),
        (Accent::SteelBlue, false) => (0x2f, 0x6a, 0x94),
        (Accent::Teal, true) => (0x5f, 0xb3, 0xac),
        (Accent::Teal, false) => (0x2c, 0x6d, 0x68),
        (Accent::Violet, true) => (0x9a, 0x8f, 0xd0),
        (Accent::Violet, false) => (0x54, 0x4a, 0x8c),
        (Accent::Orange, true) => (0xe3, 0x9a, 0x5c),
        (Accent::Orange, false) => (0xb0, 0x52, 0x1a),
        (Accent::Sage, true) => (0x9c, 0xc4, 0x8a),
        (Accent::Sage, false) => (0x3e, 0x7a, 0x37),
        (Accent::Rose, true) => (0xdd, 0x8f, 0xa6),
        (Accent::Rose, false) => (0xa6, 0x3d, 0x5f),
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

/// The medium weight, for titles, the display size and the panel's section names
/// (`docs/DESIGN.md` §3) — Inter 4.1 Medium, subset exactly as the regular face is (the private-use
/// block stripped, every layout feature kept; `pyftsubset --unicodes="U+0000-DFFF,U+F900-10FFFF"
/// --layout-features='*'`), added 2026-09-29.
const MEDIUM_FONT: &[u8] = include_bytes!("../assets/Inter-Medium.ttf");

/// The named family the medium face is installed under.
pub const MEDIUM: &str = "medium";

/// The medium family, first to last: the proportional stack with Inter Medium in Inter's place,
/// for the reason `PROPORTIONAL` gives.
const MEDIUM_FAMILY: [&str; 6] = [
    "Inter-Medium",
    "Ubuntu-Light",
    "NotoEmoji-Regular",
    "phosphor",
    "emoji-icon-font",
    "Hack",
];

/// Marks a context whose fonts [`install_fonts`] installed, so the medium family may be named.
///
/// ⚠⚠ **egui panics on a font family it does not have**, and the headless harness builds bare
/// contexts that never install the bundled fonts — 133 of them. So the medium face is used only
/// where this marker says it exists; anywhere else the same size falls back to the regular face.
fn fonts_marker() -> egui::Id {
    egui::Id::new("rustroom_fonts_installed")
}

/// The medium face at `size` — or the regular face where the bundled fonts were never installed
/// (the headless harness). For a painter that has no text style to resolve.
pub fn medium_font(ctx: &egui::Context, size: f32) -> egui::FontId {
    if ctx.data(|d| d.get_temp::<bool>(fonts_marker()).unwrap_or(false)) {
        egui::FontId::new(size, egui::FontFamily::Name(MEDIUM.into()))
    } else {
        egui::FontId::proportional(size)
    }
}

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
    ctx.data_mut(|d| d.insert_temp(fonts_marker(), true));
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
    defs.font_data.insert(
        "Inter-Medium".to_owned(),
        std::sync::Arc::new(egui::FontData::from_static(MEDIUM_FONT)),
    );
    defs.families.insert(
        egui::FontFamily::Name(MEDIUM.into()),
        MEDIUM_FAMILY.iter().map(|n| (*n).to_owned()).collect(),
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
    // ⚠ The window's own chrome too: with the light theme applied the macOS title bar stayed
    // dark, a black band over a light interface (P-31, the first look at the light theme).
    ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(match settings.theme {
        ThemeChoice::Dark => egui::SystemTheme::Dark,
        ThemeChoice::Light => egui::SystemTheme::Light,
        ThemeChoice::System => egui::SystemTheme::SystemDefault,
    }));
    let medium = ctx.data(|d| d.get_temp::<bool>(fonts_marker()).unwrap_or(false));
    for (theme, dark) in [(egui::Theme::Dark, true), (egui::Theme::Light, false)] {
        let accent = accent_color(settings.accent, dark);
        ctx.style_mut_of(theme, |style| {
            tune(style);
            // ⚠ Titles and the display size in the medium face — only where it is installed; see
            // `fonts_marker`.
            if medium {
                let family = egui::FontFamily::Name(MEDIUM.into());
                style.text_styles.insert(
                    egui::TextStyle::Heading,
                    egui::FontId::new(tokens::DISPLAY, family.clone()),
                );
                style
                    .text_styles
                    .insert(title_style(), egui::FontId::new(tokens::TITLE, family));
            }
            surfaces(style, dark);
            style.visuals.selection.bg_fill = accent;
            style.visuals.widgets.hovered.bg_fill = accent;
            style.visuals.widgets.active.bg_fill = accent;
        });
    }
}

/// The theme's surfaces, text greys and hairlines, from [`tokens`] — `docs/DESIGN.md` §4.1.
///
/// ⚠ **Per theme, never in [`tune`]** — every value is a pair.
///
/// ⚠ How the tokens land in egui's slots: fields (`TextEdit`) paint `extreme_bg_color`, so that
/// is the **card**; buttons and combos paint `inactive.weak_bg_fill`, the **raised** step; their
/// hover `hovered.weak_bg_fill`, the **hover** step; labels `noninteractive.fg_stroke`, the body
/// **text**; `weak_text_color` is **text2**, which is readable — egui's default derives it by
/// alpha, which fell below 4.5:1 on the card.
fn surfaces(style: &mut egui::Style, dark: bool) {
    let t = tokens::tokens(dark);
    let v = &mut style.visuals;
    v.panel_fill = t.panel;
    v.window_fill = t.panel;
    v.window_stroke = egui::Stroke::new(1.0, t.line);
    v.extreme_bg_color = t.card;
    v.faint_bg_color = t.card;
    v.weak_text_color = Some(t.text2);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = t.panel;
    w.noninteractive.weak_bg_fill = t.panel;
    w.noninteractive.fg_stroke.color = t.text;
    w.noninteractive.bg_stroke.color = t.line;
    w.inactive.bg_fill = t.raised;
    w.inactive.weak_bg_fill = t.raised;
    w.inactive.fg_stroke.color = t.text;
    w.inactive.bg_stroke = egui::Stroke::NONE;
    w.hovered.weak_bg_fill = t.hover;
    w.hovered.bg_stroke = egui::Stroke::NONE;
    w.hovered.fg_stroke.color = t.text;
    w.active.weak_bg_fill = t.hover;
    w.active.bg_stroke = egui::Stroke::NONE;
    w.open.weak_bg_fill = t.raised;
    w.open.bg_stroke = egui::Stroke::NONE;
}

/// The Develop canvas surround: darker than the panel in both themes.
///
/// ⚠ The panel is a surface the image is judged *from*; the canvas is the surround it is
/// judged *against*, and the visual identity spec's argument — low chroma **near the image** —
/// is what this serves. ⚠ Only the Develop canvas takes it; the Library grid is a browsing
/// surface and keeps `panel_fill`.
pub fn canvas_fill(dark: bool) -> egui::Color32 {
    tokens::tokens(dark).canvas
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
    left: 14,
    right: 14,
    top: 6,
    bottom: 6,
};

/// The frame the right panel and the snapshots strip are drawn in.
pub fn panel_frame(style: &egui::Style) -> egui::Frame {
    egui::Frame::side_top_panel(style).inner_margin(PANEL_MARGIN)
}

/// The theme-independent tweaks: the type scale, the metrics and the shape — `docs/DESIGN.md`
/// §3 and §5.
///
/// ⚠ **Only theme-independent things belong here.** Anything colour-related goes in that
/// theme's own `visuals` — see [`apply`] — or light mode inherits dark colours.
///
/// ⚠⚠ **Four text sizes, set here and nowhere else.** Before 2026-09-29 `tune` never touched
/// `text_styles`, so egui's defaults (Small **9**, Body 13, Heading 18) sat beside 24 literal
/// sizes in the bin modules — ten sizes in all, the 9-pt hints the ones the author could not
/// read on a 5K display. `no_literal_font_size_outside_theme` keeps the literals out.
///
/// ⚠ Direction B is **roomier than the old compact density on purpose** (the author's choice from
/// three): a 28-pt control row where there was 16. The Develop panel scrolls; that is the trade.
fn tune(style: &mut egui::Style) {
    use egui::{FontFamily, FontId, TextStyle};
    style.text_styles = [
        (TextStyle::Small, FontId::proportional(tokens::CAPTION)),
        (TextStyle::Body, FontId::proportional(tokens::BODY)),
        (TextStyle::Button, FontId::proportional(tokens::BODY)),
        (
            TextStyle::Monospace,
            FontId::new(tokens::BODY, FontFamily::Monospace),
        ),
        (TextStyle::Heading, FontId::proportional(tokens::DISPLAY)),
        (section_style(), FontId::proportional(tokens::CAPTION)),
        (title_style(), FontId::proportional(tokens::TITLE)),
    ]
    .into();

    let sp = &mut style.spacing;
    sp.interact_size.y = tokens::CONTROL_ROW;
    sp.item_spacing = egui::vec2(8.0, 2.0);
    sp.button_padding = egui::vec2(10.0, 4.0);
    sp.slider_rail_height = 4.0;
    sp.tooltip_width = 280.0;
    sp.menu_margin = egui::Margin::same(8);
    style.interaction.tooltip_delay = 0.4;

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
        ws.corner_radius = egui::CornerRadius::same(tokens::RADIUS);
    }
    style.visuals.window_corner_radius = egui::CornerRadius::same(tokens::RADIUS_DIALOG);
    style.visuals.menu_corner_radius = egui::CornerRadius::same(tokens::RADIUS_DIALOG);

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

/// Every hover sentence goes through here (M10.6b): it is shown as the tooltip **and** becomes the
/// control's AccessKit description — which a screen reader reads, and which `hover_coverage` checks on
/// every clickable control the harnesses can draw. ⚠ `on_hover_text` itself is called nowhere else;
/// `no_bare_on_hover_text_outside_theme` holds that.
pub trait Hover {
    /// Show `sentence` on hover and record it as this control's description.
    fn hover(self, sentence: impl AsRef<str>) -> Self;
    /// `title` on its own line above `sentence` in the tooltip — an elided name shown in full — with
    /// `sentence` alone as the description.
    fn hover_titled(self, title: &str, sentence: &str) -> Self;
}

impl Hover for egui::Response {
    fn hover(self, sentence: impl AsRef<str>) -> Self {
        let s = sentence.as_ref();
        describe(&self, s);
        self.on_hover_text(s)
    }

    fn hover_titled(self, title: &str, sentence: &str) -> Self {
        describe(&self, sentence);
        self.on_hover_text(format!("{title}\n{sentence}"))
    }
}

/// Record `sentence` as a control's description without a tooltip of its own — for a node whose
/// sentence is shown by its neighbour: a slider's rail, whose sentence sits on its name.
///
/// ⚠ A no-op while AccessKit is off, which is every frame without a screen reader or a test.
pub fn describe(r: &egui::Response, sentence: &str) {
    r.ctx
        .accesskit_node_builder(r.id, |n| n.set_description(sentence.to_owned()));
}

/// A line of secondary text under a control: a value, a state, an empty list's note.
///
/// ⚠⚠ **Not an explanation.** Since 2026-09-29 explanations live on hover (`docs/DESIGN.md` §8);
/// what stays on screen is what changes — a readout, a chosen path, "No keywords yet." It draws
/// at the caption size (11 pt), never below; the 9-pt hints were what the author could not read.
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

/// The named text style for titles: dialog titles, the open photo's name, empty-state buttons.
pub fn title_style() -> egui::TextStyle {
    egui::TextStyle::Name("title".into())
}

/// The title face, resolved against `style` — the same fallback [`section_font`] carries, for
/// the same reason: a bare harness `Context` never runs [`apply`].
pub fn title_font(style: &egui::Style) -> egui::FontId {
    style
        .text_styles
        .get(&title_style())
        .cloned()
        .unwrap_or_else(|| egui::FontId::proportional(tokens::TITLE))
}

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
        .unwrap_or_else(|| egui::FontId::proportional(tokens::CAPTION))
}

/// A sub-label: the name of a block of controls inside a section — Presence, Noise reduction,
/// Color grading. In the caption face and the secondary grey, **as written** (sentence case,
/// `docs/DESIGN.md` §7.5 — the capitals belong to the Library rail alone), with air above and no
/// hairline.
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
    ui.add_space(8.0);
    let colour = ui.visuals().weak_text_color();
    let font = medium_font(ui.ctx(), section_font(ui.style()).size);
    ui.add(egui::Label::new(egui::RichText::new(text).font(font).color(colour)).selectable(false))
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
    ui.add_space(8.0);
    let colour = ui.visuals().weak_text_color();
    let font = medium_font(ui.ctx(), section_font(ui.style()).size);
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, colour);
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
            Self::Compact => (340.0, 160.0),
            // ⚠ Taller with direction B's 28-pt rows: at 320 the Appearance tab needed a
            // scrollbar for five rows (seen 2026-09-29).
            Self::Standard => (520.0, 440.0),
            Self::Wide => (680.0, 560.0),
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

/// The accent-filled button: a dialog footer's primary verb, the bar's Export.
///
/// ⚠ One painter for both, so the bar and the dialogs cannot drift apart. ⚠ The text is
/// [`on_accent`] — dark on the dark theme's light accents, white on the light theme's dark ones
/// (`text_on_every_accent_is_readable`); white on the dark steel blue was 2.4:1. A disabled one
/// is drawn greyed, never hidden — the Save button with an empty name must still be seen.
pub fn primary_button(ui: &mut egui::Ui, text: &str, enabled: bool) -> egui::Response {
    let accent = ui.visuals().selection.bg_fill;
    let (fill, colour) = if enabled {
        (accent, on_accent(ui.visuals().dark_mode))
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
            crate::library::EMPTY_LIBRARY_TEXT.to_owned(),
            crate::develop::canvas::EMPTY_DEVELOP_TEXT.to_owned(),
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
        // ⚠ The medium family too: a medium face that kept its private-use block would draw its own
        // glyphs where the icons belong, exactly as Inter Regular did before it was subset.
        let medium = &defs.families[&egui::FontFamily::Name(MEDIUM.into())];
        let first_medium = |c: char| medium.iter().find(|n| maps(n, c)).map(String::as_str);
        for c in rustroom_app::icons::ALL {
            assert_eq!(
                first_medium(c),
                Some("phosphor"),
                "U+{:04X} would be drawn by the wrong font in the medium family",
                c as u32
            );
        }
        assert_eq!(first_medium('a'), Some("Inter-Medium"));
        assert_eq!(
            first_with('\u{2715}'),
            None,
            "✕ is in no bundled font — the control"
        );
    }

    #[test]
    fn titles_are_medium_where_the_fonts_are_installed_and_regular_where_not() {
        // ⚠ Both halves: the installed context gets the medium family for titles and the display
        // size; a bare context (every headless harness) gets the regular face, because naming a
        // family egui does not have panics.
        let with = egui::Context::default();
        install_fonts(&with);
        apply(&with, &Settings::default());
        let s = with.style_of(egui::Theme::Dark);
        let medium = egui::FontFamily::Name(MEDIUM.into());
        assert_eq!(title_font(&s).family, medium);
        assert_eq!(s.text_styles[&egui::TextStyle::Heading].family, medium);
        assert_eq!(
            s.text_styles[&egui::TextStyle::Body].family,
            egui::FontFamily::Proportional
        );
        let _ = with.run_ui(egui::RawInput::default(), |ui| {
            ui.heading("A title drawn in the medium face");
        });
        let without = egui::Context::default();
        apply(&without, &Settings::default());
        let s = without.style_of(egui::Theme::Dark);
        assert_eq!(title_font(&s).family, egui::FontFamily::Proportional);
        assert_eq!(
            medium_font(&without, 13.0).family,
            egui::FontFamily::Proportional
        );
    }

    #[test]
    fn the_type_scale_is_four_sizes() {
        // ⚠⚠ `docs/DESIGN.md` §3: caption 11, body 13, title 15, display 20, and nothing below
        // 11. Asserted over **every** installed style, so a fifth size added to `tune` fails
        // here rather than drifting in. egui's own default Small was 9 — the unreadable hints.
        let s = tuned();
        assert!(s.text_styles.len() >= 7, "sample: {}", s.text_styles.len());
        let mut sizes: Vec<u32> = s.text_styles.values().map(|f| f.size as u32).collect();
        sizes.sort_unstable();
        sizes.dedup();
        assert_eq!(sizes, vec![11, 13, 15, 20], "the installed sizes");
        let body = s.text_styles[&egui::TextStyle::Body].size;
        let small = s.text_styles[&egui::TextStyle::Small].size;
        assert!(small < body, "small {small} should be under body {body}");
        assert_eq!(title_font(&s).size, tokens::TITLE);
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
        let items = [
            (C::A, "Hue", ""),
            (C::B, "Saturation", ""),
            (C::D, "Luminance", ""),
        ];
        let ctx = crate::test_ui::ctx();
        let mut cur = C::A;
        let found = crate::test_ui::labelled_centres(&ctx, Vec::new(), |ui| {
            segmented(ui, &mut cur, &items);
        });
        for (_, l, _) in items {
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
            assert_eq!(ws.corner_radius, egui::CornerRadius::same(tokens::RADIUS));
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
    fn text_on_every_accent_is_readable() {
        // ⚠⚠ The primary button and the lit app switch paint text on the accent. White on the dark
        // theme's steel blue measured 2.4:1 — below even the non-text floor — so the text colour is
        // a pair too (`on_accent`), and this sweeps it over every accent at the text floor.
        assert!(Accent::ALL.len() >= 7, "sample");
        for accent in Accent::ALL {
            for dark in [true, false] {
                let c = contrast(on_accent(dark), accent_color(accent, dark));
                assert!(
                    c >= 4.5,
                    "text on {accent:?} ({}): {c:.2}:1 is below 4.5:1",
                    if dark { "dark" } else { "light" }
                );
            }
        }
    }

    #[test]
    fn the_text_greys_are_readable_where_they_are_used() {
        // Body text and the secondary grey sit on the panel, inside cards and on buttons; both must
        // clear the text floor on all three. The tertiary grey is decoration only (chevrons,
        // thumbnail captions, placeholders) and sits on the panel and in fields, never on a button,
        // so it is held to the 3:1 non-text floor there. ⚠ The light theme's first value, #9a9aa0,
        // measured 2.55:1 on the panel — found by this test, not lowered to pass it.
        for dark in [true, false] {
            let t = tokens::tokens(dark);
            for (name, surface, text3) in [
                ("panel", t.panel, true),
                ("card", t.card, true),
                ("raised", t.raised, false),
            ] {
                let mut greys = vec![("text", t.text, 4.5), ("text2", t.text2, 4.5)];
                if text3 {
                    greys.push(("text3", t.text3, 3.0));
                }
                for (grey, colour, floor) in greys {
                    let c = contrast(colour, surface);
                    assert!(
                        c >= floor,
                        "{grey} on {name} (dark={dark}): {c:.2}:1 is below {floor}:1"
                    );
                }
            }
        }
    }

    #[test]
    fn the_dark_surfaces_step_up_in_order() {
        // Elevation is the only depth in direction B: each step must be lighter than the one under
        // it, or a card disappears into its panel.
        let t = tokens::tokens(true);
        let steps = [t.canvas, t.panel, t.card, t.raised, t.hover];
        assert!(
            steps.windows(2).all(|p| luminance(p[0]) < luminance(p[1])),
            "dark surfaces out of order"
        );
        // Light mode: the canvas is the darkest surface and the card the lightest.
        let l = tokens::tokens(false);
        for s in [l.panel, l.card, l.raised] {
            assert!(luminance(l.canvas) < luminance(s));
            assert!(luminance(s) <= luminance(l.card));
        }
    }

    #[test]
    fn the_theme_paints_the_tokens() {
        // ⚠ The tokens are only the rulebook if egui's own slots carry them: a field, a button and a
        // label must come out in the card, raised and text values in both themes.
        let ctx = egui::Context::default();
        apply(&ctx, &Settings::default());
        for (theme, dark) in [(egui::Theme::Dark, true), (egui::Theme::Light, false)] {
            let v = ctx.style_of(theme).visuals.clone();
            let t = tokens::tokens(dark);
            assert_eq!(v.panel_fill, t.panel);
            assert_eq!(v.text_edit_bg_color(), t.card);
            assert_eq!(v.widgets.inactive.weak_bg_fill, t.raised);
            assert_eq!(v.widgets.noninteractive.fg_stroke.color, t.text);
            assert_eq!(v.weak_text_color(), t.text2);
        }
    }

    /// Which characters of `b` are code — not inside a string, a char literal or a comment.
    fn code_mask(b: &[char]) -> Vec<bool> {
        let mut mask = vec![true; b.len()];
        let mut i = 0;
        let ident = |c: char| c.is_alphanumeric() || c == '_';
        while i < b.len() {
            let c = b[i];
            let next = b.get(i + 1).copied();
            let start = i;
            if c == '/' && next == Some('/') {
                while i < b.len() && b[i] != '\n' {
                    i += 1;
                }
            } else if c == '/' && next == Some('*') {
                let mut depth = 0;
                while i < b.len() {
                    if b[i] == '/' && b.get(i + 1) == Some(&'*') {
                        depth += 1;
                        i += 2;
                    } else if b[i] == '*' && b.get(i + 1) == Some(&'/') {
                        depth -= 1;
                        i += 2;
                        if depth == 0 {
                            break;
                        }
                    } else {
                        i += 1;
                    }
                }
            } else if c == 'r'
                && (next == Some('"') || next == Some('#'))
                && (i == 0 || !ident(b[i - 1]))
            {
                // A raw string: r"…" or r#"…"#.
                let mut j = i + 1;
                let mut hashes = 0;
                while b.get(j) == Some(&'#') {
                    hashes += 1;
                    j += 1;
                }
                if b.get(j) != Some(&'"') {
                    i += 1;
                    continue;
                }
                j += 1;
                'raw: while j < b.len() {
                    if b[j] == '"' && (0..hashes).all(|k| b.get(j + 1 + k) == Some(&'#')) {
                        j += 1 + hashes;
                        break 'raw;
                    }
                    j += 1;
                }
                i = j;
            } else if c == '"' {
                i += 1;
                while i < b.len() && b[i] != '"' {
                    i += if b[i] == '\\' { 2 } else { 1 };
                }
                i += 1;
            } else if c == '\'' && (next == Some('\\') || b.get(i + 2) == Some(&'\'')) {
                // A char literal — '{', '\'' — and not a lifetime, which has no closing quote.
                i += 1;
                while i < b.len() && b[i] != '\'' {
                    i += if b[i] == '\\' { 2 } else { 1 };
                }
                i += 1;
            } else {
                i += 1;
                continue;
            }
            for m in mask.iter_mut().take(i.min(b.len())).skip(start) {
                *m = false;
            }
        }
        mask
    }

    /// `src` with every `#[cfg(test)]` item blanked — its characters spaces, its newlines kept, so
    /// line numbers still match — and nothing else.
    ///
    /// ⚠⚠ **Not "up to the first `#[cfg(test)]`"**, which is what three sweeps here did until M10.6b:
    /// a test-only item placed mid-file — `retouch.rs` has one at line 81 — hid every production line
    /// after it, and three `on_hover_text` calls there slipped past both the hover sweep and its
    /// guard. The coverage walk is what noticed.
    pub(crate) fn production_code(src: &str) -> String {
        let b: Vec<char> = src.chars().collect();
        let mask = code_mask(&b);
        let marker: Vec<char> = "#[cfg(test)]".chars().collect();
        let mut out = b.clone();
        let mut i = 0;
        while i + marker.len() <= b.len() {
            if !(mask[i] && b[i..i + marker.len()] == marker[..]) {
                i += 1;
                continue;
            }
            // The item ends at its body's closing brace, or at a `;` or `,` outside any bracket.
            let mut j = i + marker.len();
            let (mut paren, mut end) = (0i32, None);
            while j < b.len() {
                if mask[j] {
                    match b[j] {
                        '(' | '[' => paren += 1,
                        ')' | ']' => paren -= 1,
                        ';' | ',' if paren == 0 => {
                            end = Some(j);
                            break;
                        }
                        '{' if paren == 0 => {
                            let mut depth = 0;
                            while j < b.len() {
                                if mask[j] {
                                    if b[j] == '{' {
                                        depth += 1;
                                    } else if b[j] == '}' {
                                        depth -= 1;
                                        if depth == 0 {
                                            break;
                                        }
                                    }
                                }
                                j += 1;
                            }
                            end = Some(j);
                            break;
                        }
                        _ => {}
                    }
                }
                j += 1;
            }
            let end = end.unwrap_or(b.len() - 1).min(b.len() - 1);
            for c in out.iter_mut().take(end + 1).skip(i) {
                if *c != '\n' {
                    *c = ' ';
                }
            }
            i = end + 1;
        }
        out.into_iter().collect()
    }

    #[test]
    fn production_code_keeps_what_follows_a_test_item_and_blanks_only_the_item() {
        let src = "fn a() {}\n#[cfg(test)]\nfn helper() { let s = \"}\"; }\nfn b() { x.on_hover_text(\"hi\"); }\n#[cfg(test)]\nmod tests {\n    fn t() { let c = '}'; let s = \"#[cfg(test)]\"; }\n}\nstruct S {\n    #[cfg(test)]\n    pub(crate) n: usize,\n    kept: u8,\n}\n";
        let out = production_code(src);
        assert_eq!(
            out.lines().count(),
            src.lines().count(),
            "line numbers must survive"
        );
        // ⚠ The positive control: production code after a test item is still there.
        assert!(out.contains("fn b() { x.on_hover_text"), "{out}");
        assert!(
            out.contains("fn a() {}") && out.contains("kept: u8"),
            "{out}"
        );
        // …and every test item is gone, braces inside its strings and chars notwithstanding.
        for gone in ["helper", "mod tests", "fn t()", "pub(crate) n"] {
            assert!(!out.contains(gone), "{gone} survived:\n{out}");
        }
    }

    /// The literal font sizes in `source`, up to its test module: `FontId::proportional(<digit>`,
    /// `FontId::new(<digit>`, `FontId::monospace(<digit>` and `.size(<digit>`.
    fn literal_sizes(source: &str) -> Vec<String> {
        let code = production_code(source);
        let code = code.as_str();
        let mut found = Vec::new();
        for (n, line) in code.lines().enumerate() {
            for pat in [
                "FontId::proportional(",
                "FontId::new(",
                "FontId::monospace(",
                ".size(",
            ] {
                let mut rest = line;
                while let Some(i) = rest.find(pat) {
                    rest = &rest[i + pat.len()..];
                    if rest.trim_start().starts_with(|c: char| c.is_ascii_digit()) {
                        found.push(format!("{}: {}", n + 1, line.trim()));
                    }
                }
            }
        }
        found
    }

    #[test]
    fn no_literal_font_size_outside_theme() {
        // ⚠⚠ Ten sizes crept in exactly this way — one reasonable-looking `.size(15.0)` at a time.
        // Sizes come from the text styles or `theme::tokens`; this walks every app source file
        // outside the theme and fails on a literal. ⚠ The sample and a positive control first, so a
        // walk that found no files or a matcher that matches nothing cannot pass.
        assert_eq!(
            literal_sizes("let f = egui::FontId::proportional(9.0);\nx.size(15.0)").len(),
            2,
            "the matcher misses the shape it exists to catch"
        );
        assert!(literal_sizes("egui::FontId::proportional(tokens::BODY)").is_empty());
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            for e in std::fs::read_dir(&dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "rs") {
                    files.push(p);
                }
            }
        }
        assert!(files.len() > 100, "sample: {} files", files.len());
        let mut offenders = Vec::new();
        for f in &files {
            let rel = f.strip_prefix(&root).unwrap().to_string_lossy().to_string();
            if rel == "theme.rs" || rel.starts_with("theme/") {
                continue;
            }
            for hit in literal_sizes(&std::fs::read_to_string(f).unwrap()) {
                offenders.push(format!("{rel}:{hit}"));
            }
        }
        assert!(
            offenders.is_empty(),
            "literal font sizes outside theme (use a TextStyle or theme::tokens):\n{}",
            offenders.join("\n")
        );
    }

    /// The string literal starting at `s[i]` (which is `"`), with `\` line continuations and the
    /// common escapes resolved — enough to read a hover sentence out of the source.
    fn literal_at(s: &str, i: usize) -> String {
        let b: Vec<char> = s[i + 1..].chars().collect();
        let (mut out, mut k) = (String::new(), 0);
        while k < b.len() {
            match b[k] {
                '\\' if b.get(k + 1) == Some(&'\n') => {
                    k += 2;
                    while k < b.len() && (b[k] == ' ' || b[k] == '\t') {
                        k += 1;
                    }
                }
                '\\' => {
                    out.push(match b.get(k + 1) {
                        Some('n') => '\n',
                        Some(c) => *c,
                        None => '\\',
                    });
                    k += 2;
                }
                '"' => break,
                c => {
                    out.push(c);
                    k += 1;
                }
            }
        }
        out
    }

    /// Every literal hover sentence in the app's sources, with where it is.
    fn literal_hovers() -> Vec<(String, String)> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut stack = vec![root.clone()];
        let mut found = Vec::new();
        while let Some(dir) = stack.pop() {
            for e in std::fs::read_dir(&dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                if p.extension().is_none_or(|x| x != "rs") {
                    continue;
                }
                let src = std::fs::read_to_string(&p).unwrap();
                let code = production_code(&src);
                let code = code.as_str();
                let rel = p.strip_prefix(&root).unwrap().display().to_string();
                // ⚠ The named hover constants too — the retouch modes, the profile note, the DNG note —
                // which read exactly like call-site hovers once drawn.
                // ⚠⚠ No space after `=`: rustfmt moves a long literal to the next line, and the
                // pattern with the space was blind to every such constant — seven of them, the DNG
                // and profile notes included, until a full-stop sabotage on a new one went green.
                // An empty state's prose is `*_TEXT`, not a hover (DESIGN.md §7.14).
                for pat in [
                    "on_hover_text(",
                    ".hover(",
                    "hover: ",
                    "HINT: &str =",
                    "HELP: &str =",
                    "NOTE: &str =",
                    "HOVER: &str =",
                    "NOTHING: &str =",
                ] {
                    let mut from = 0;
                    while let Some(at) = code[from..].find(pat) {
                        let start = from + at + pat.len();
                        from = start;
                        let rest = &code[start..];
                        let lead = rest.len() - rest.trim_start().len();
                        if rest.trim_start().starts_with('"') {
                            let t = literal_at(code, start + lead);
                            let line = code[..start].lines().count();
                            found.push((format!("{rel}:{line}"), t));
                        }
                    }
                }
            }
        }
        found
    }

    /// ⚠⚠ **The author's rule, 2026-09-29: a hover "should simply state what it does exactly in
    /// one, clear sentence", and never "Double-click to reset".** Swept over every literal hover in
    /// the sources so a paragraph cannot creep back in one reasonable-looking call at a time — the
    /// way the 177 of them had become essays, warnings and measurements. `slider_help` checks its
    /// own table the same way. ⚠ Sample and a positive control first.
    /// ⚠ The helper's whole point: the sentence becomes the control's description.
    #[test]
    fn a_hover_is_the_controls_description() {
        let ctx = crate::test_ui::ctx();
        let nodes = crate::test_ui::nodes(&ctx, |ui| {
            let _ = ui.button("Go").hover("Goes somewhere");
        });
        let go = nodes.iter().find(|n| n.label == "Go").expect("drawn");
        assert!(go.clickable, "a button offers a click");
        assert_eq!(go.description.as_deref(), Some("Goes somewhere"));
    }

    /// ⚠⚠ Every hover goes through `Hover`, or the description — and the coverage walk — is
    /// bypassed. Swept over the non-test code of every source; a positive control first: this file's
    /// own call must be found, or the sweep is reading nothing.
    #[test]
    fn no_bare_on_hover_text_outside_theme() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut stack = vec![root.clone()];
        let (mut own, mut bare) = (0, Vec::new());
        while let Some(dir) = stack.pop() {
            for e in std::fs::read_dir(&dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                if p.extension().is_none_or(|x| x != "rs") {
                    continue;
                }
                let src = std::fs::read_to_string(&p).unwrap();
                let code = production_code(&src);
                let code = code.as_str();
                let n = code.matches(".on_hover_text(").count();
                let rel = p.strip_prefix(&root).unwrap().display().to_string();
                if rel == "theme.rs" {
                    own += n;
                } else if n > 0 {
                    bare.push(format!("{rel}: {n}"));
                }
            }
        }
        assert!(own > 0, "the sweep did not find theme.rs's own call");
        assert!(bare.is_empty(), "on_hover_text outside theme.rs: {bare:?}");
    }

    #[test]
    fn every_hover_is_one_short_sentence() {
        assert_eq!(
            literal_at("\"One \\\n     sentence.\"", 0),
            "One sentence.",
            "the reader must join a continued literal"
        );
        let all = literal_hovers();
        assert!(all.len() > 80, "sample: {}", all.len());
        let mut bad = Vec::new();
        for (at, t) in &all {
            let t = t.trim();
            if t.is_empty() {
                continue;
            }
            if let Some(fault) = rustroom_app::slider_help::sentence_fault(t) {
                bad.push(format!("{at}: {fault}: {t:?}"));
            }
        }
        assert!(
            bad.is_empty(),
            "hovers that are not one short sentence:\n{}",
            bad.join("\n")
        );
    }

    #[test]
    fn the_metrics_follow_the_design_language() {
        // `tune` accidentally becoming a no-op should fail, and so should a control row that is
        // not the rulebook's 28 — every button, combo and slider row inherits it.
        let s = tuned();
        assert_eq!(s.spacing.interact_size.y, tokens::CONTROL_ROW);
        assert_ne!(
            s.spacing.interact_size.y,
            egui::Style::default().spacing.interact_size.y,
            "tune changed nothing"
        );
        assert_eq!(
            s.visuals.window_corner_radius,
            egui::CornerRadius::same(tokens::RADIUS_DIALOG)
        );
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
