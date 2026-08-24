//! Which Develop tool the rail has selected.
//!
//! ⚠ Lives on `RustroomApp`, not on `Session`, so the selection **survives a photo switch** —
//! today's `geometry_mode` resets when you arrow to the next photo, and with a rail it should
//! not. Lightroom does not reset it either.
//!
//! ⚠ No egui here: `lib.rs`'s rule is what keeps these units testable without a windowing
//! system. The rendering lives in the bin, in `develop/rail.rs`.

/// A Develop tool. `Edit` is the default and shows the global adjustment sections.
// ⚠ `Hash` because `Command::SelectTool(Tool)` is a key in the native menu's id map.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Hash)]
pub enum Tool {
    #[default]
    Edit,
    Crop,
    Masks,
    Presets,
}

impl Tool {
    /// Every tool, in rail order. The rail iterates this, so a variant missing from it is
    /// unreachable in the UI while compiling perfectly.
    pub const ALL: [Tool; 4] = [Self::Edit, Self::Crop, Self::Masks, Self::Presets];

    /// The rail's text label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Edit => "Edit",
            Self::Crop => "Crop",
            Self::Masks => "Masks",
            Self::Presets => "Presets",
        }
    }

    /// The rail's glyph.
    ///
    /// ⚠ These are **bundled Phosphor icons**, not Unicode characters that egui's own fonts
    /// happen to contain. The previous set — ☰ ▣ ◑ ★ — was picked from the roughly twenty
    /// renderable characters in egui's default stack, which is why they were approximations.
    /// `every_rail_glyph_is_renderable` still guards them, and now guards them against the font
    /// that actually ships.
    pub fn glyph(self) -> char {
        match self {
            Self::Edit => crate::icons::EDIT,
            Self::Crop => crate::icons::CROP,
            Self::Masks => crate::icons::MASKS,
            Self::Presets => crate::icons::PRESETS,
        }
    }

    /// The heading shown above this tool's controls, or `None` for the adjustment stack.
    ///
    /// ⚠ `Edit` returns `None` because its content **is** the five adjustment sections, which
    /// carry their own edit-group headers — a heading above them would be a level of nesting
    /// for nothing.
    ///
    /// ⚠ The panel is **exclusive**: exactly one tool's controls are shown at a time, so the
    /// global sliders are reachable only under `Edit`. An earlier design put the tool's
    /// controls in a *drawer above* a permanent adjustment stack; that was reversed in use —
    /// see the spec's "Exclusive, reversed 2026-08-24" section.
    pub fn panel_heading(self) -> Option<&'static str> {
        match self {
            Self::Edit => None,
            Self::Crop => Some("Crop & Geometry"),
            Self::Masks => Some("Masks"),
            Self::Presets => Some("Presets"),
        }
    }

    /// Whether this tool puts the canvas in geometry mode.
    ///
    /// ⚠ This **replaces the stored `Session::geometry_mode` field**. Two facts that could
    /// disagree became one — the same correction `render_max_dim` needed when it was a field
    /// beside `requested_region`.
    pub fn is_geometry(self) -> bool {
        matches!(self, Self::Crop)
    }

    /// The tool for a 1–4 keypress, if any.
    pub fn from_digit(d: u32) -> Option<Tool> {
        Self::ALL.get(d.checked_sub(1)? as usize).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_edit_shows_the_adjustment_stack() {
        // ⚠ Both halves matter. Edit being default is what makes the panel at rest the five
        // adjustment sections; Edit being the *only* tool without a heading is what stops a
        // future tool silently rendering untitled.
        assert_eq!(Tool::default(), Tool::Edit);
        let unheaded: Vec<Tool> = Tool::ALL
            .into_iter()
            .filter(|t| t.panel_heading().is_none())
            .collect();
        assert_eq!(unheaded, vec![Tool::Edit]);
    }

    #[test]
    fn only_crop_is_a_geometry_tool() {
        // ⚠ Pins the derived property that replaced the stored field. Stated over ALL rather
        // than as `assert!(Tool::Crop.is_geometry())`, so a second tool accidentally
        // returning true cannot pass.
        let geo: Vec<Tool> = Tool::ALL.into_iter().filter(|t| t.is_geometry()).collect();
        assert_eq!(geo, vec![Tool::Crop]);
    }

    #[test]
    fn labels_and_glyphs_are_all_distinct() {
        // Two tools sharing either would give the rail two identical-looking entries.
        let mut labels: Vec<&str> = Tool::ALL.iter().map(|t| t.label()).collect();
        labels.sort_unstable();
        labels.dedup();
        assert_eq!(labels.len(), 4, "two tools share a label");
        let mut glyphs: Vec<char> = Tool::ALL.iter().map(|t| t.glyph()).collect();
        glyphs.sort_unstable();
        glyphs.dedup();
        assert_eq!(glyphs.len(), 4, "two tools share a glyph");
    }

    #[test]
    fn the_digits_select_the_rail_in_order() {
        // ⚠ 1-based, matching the labels a user sees. Off-by-one here means every shortcut
        // picks the neighbouring tool — plausible-looking and completely wrong.
        assert_eq!(Tool::from_digit(1), Some(Tool::Edit));
        assert_eq!(Tool::from_digit(2), Some(Tool::Crop));
        assert_eq!(Tool::from_digit(3), Some(Tool::Masks));
        assert_eq!(Tool::from_digit(4), Some(Tool::Presets));
        assert_eq!(Tool::from_digit(0), None, "there is no tool 0");
        assert_eq!(Tool::from_digit(5), None);
    }
}
