//! The design tokens: every surface, text grey and size in `docs/DESIGN.md` §3–§5, as values.
//!
//! ⚠ **One place, read by everyone who paints.** egui's `Visuals` carry some of these (the
//! panel, the button fill); the rest — the canvas, the card, the hover step, the three text greys
//! — have no slot there, and a hand-painter that picked its own grey is how the app came to have
//! four ways of drawing "selected". A painter asks [`of`] for the current theme's set.
//!
//! ⚠ Every value is a **pair**, dark and light, the 2026-08-13 rule: a colour tuned for
//! near-black washes out on near-white. `theme`'s tests check the contrast floors over both.

use eframe::egui::{self, Color32};

/// One theme's surfaces and text greys.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tokens {
    /// The surround the photo is judged against: the Develop canvas, Compare and Survey.
    pub canvas: Color32,
    /// Bars, rails, the Develop panel, dialogs, menus.
    pub panel: Color32,
    /// A section of controls; a text field.
    pub card: Color32,
    /// Buttons, combos, chips, the lit segment.
    pub raised: Color32,
    /// Any raised or quiet control under the pointer.
    pub hover: Color32,
    /// Hairlines.
    pub line: Color32,
    /// Body text, titles, values.
    pub text: Color32,
    /// Captions, counts, secondary labels, quiet buttons at rest.
    pub text2: Color32,
    /// Decoration only: chevrons, thumbnail captions, disabled. Never text that must be read.
    pub text3: Color32,
    /// The tooltip's fill.
    pub tooltip: Color32,
}

const fn hex(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

/// The dark theme — the default.
pub const DARK: Tokens = Tokens {
    canvas: hex(0x121214),
    panel: hex(0x1b1b1e),
    card: hex(0x232327),
    raised: hex(0x303036),
    hover: hex(0x3b3b42),
    line: hex(0x2b2b30),
    text: hex(0xe6e6e8),
    text2: hex(0xa3a3a8),
    text3: hex(0x727278),
    tooltip: hex(0x0f0f11),
};

/// The light theme.
///
/// ⚠ The canvas is the *darkest* surface here too — the surround touching the photograph is
/// the most neutral and least bright one (`the_canvas_is_darker_than_the_panel_in_both_themes`).
pub const LIGHT: Tokens = Tokens {
    canvas: hex(0xdcdcdf),
    panel: hex(0xf4f4f5),
    card: hex(0xffffff),
    raised: hex(0xe6e6ea),
    hover: hex(0xdcdce1),
    line: hex(0xd9d9de),
    text: hex(0x1d1d1f),
    text2: hex(0x5f5f66),
    text3: hex(0x8a8a90),
    tooltip: hex(0x2a2a2e),
};

/// The set for a theme.
pub fn tokens(dark: bool) -> &'static Tokens {
    if dark {
        &DARK
    } else {
        &LIGHT
    }
}

/// The set the `ui` is drawing in.
pub fn of(ui: &egui::Ui) -> &'static Tokens {
    tokens(ui.visuals().dark_mode)
}

/// How much of the accent an open row or the active tool carries over its surface.
pub const TINT: f32 = 0.18;

/// Text on an accent fill: dark on the dark theme's light accents, white on the light theme's
/// dark ones. ⚠ White on the dark-theme steel blue measured **2.4:1** — the reason this exists.
pub fn on_accent(dark: bool) -> Color32 {
    if dark {
        hex(0x0e1216)
    } else {
        Color32::WHITE
    }
}

// ---- sizes (`docs/DESIGN.md` §3 and §5) -----------------------------------------------------

/// Caption: section names, counts, thumbnail captions, chips.
pub const CAPTION: f32 = 11.0;
/// Body: labels, controls, rows, menus, tooltips, values.
pub const BODY: f32 = 13.0;
/// Title: dialog titles, the open photo's name, empty-state buttons.
pub const TITLE: f32 = 15.0;
/// Display: empty-state headings, nothing else.
pub const DISPLAY: f32 = 20.0;

/// An icon inline in text.
pub const ICON_INLINE: f32 = 14.0;
/// An icon in a toolbar or a button.
pub const ICON: f32 = 16.0;
/// The tool rail's glyphs and empty states.
pub const ICON_LARGE: f32 = 24.0;

/// Buttons, fields, combos, slider rows, checkboxes.
pub const CONTROL_ROW: f32 = 28.0;
/// Rail, mask list, snapshots, presets.
pub const LIST_ROW: f32 = 30.0;
/// A section header.
pub const HEADER_ROW: f32 = 26.0;
/// The top bar.
pub const TOP_BAR: f32 = 44.0;

/// Buttons, fields, rows.
pub const RADIUS: u8 = 6;
/// A section card.
pub const RADIUS_CARD: u8 = 10;
/// Thumbnails.
pub const RADIUS_THUMB: u8 = 8;
/// Dialogs and menus.
pub const RADIUS_DIALOG: u8 = 12;
