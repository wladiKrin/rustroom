//! The icon codepoints, from Phosphor Icons 2.1.2 (MIT).
//!
//! ⚠ **Every constant names its Phosphor icon in a doc comment, and that is not decoration.** A
//! bare `'\u{e30e}'` at a call site is a magic literal that cannot be checked against anything.
//! The name is what makes a codepoint traceable back to a picture, and what makes a font upgrade
//! auditable: fetch that version's `style.css` and compare the `.ph-<name>:before` contents.
//!
//! ⚠ The font is **pinned to 2.1.2** in `crates/app/assets/`. These codepoints were verified
//! against that version specifically.
//!
//! ⚠ Lives in the lib because `tool::Tool::glyph` needs it, and `tool` is a lib module. No egui
//! here — these are plain `char`s.

/// `sliders-horizontal` — the Edit tool.
pub const EDIT: char = '\u{e434}';
/// `crop` — the Crop tool.
pub const CROP: char = '\u{e1d4}';
/// `circle-half` — the Masks tool.
pub const MASKS: char = '\u{e18c}';
/// `star` — the Presets tool.
pub const PRESETS: char = '\u{e46a}';

/// `arrow-u-up-left` — undo.
///
/// ⚠ The u-turn arrow, not the circular `arrow-counter-clockwise`: this is the curved left arrow
/// every editor uses for undo, and the circular one reads as "rotate".
pub const UNDO: char = '\u{e08a}';
/// `arrow-u-up-right` — redo. See [`UNDO`].
pub const REDO: char = '\u{e08c}';

/// `magnifying-glass-minus` — zoom out.
pub const ZOOM_OUT: char = '\u{e30e}';
/// `magnifying-glass-plus` — zoom in.
pub const ZOOM_IN: char = '\u{e310}';
/// `frame-corners` — zoom to fit.
///
/// ⚠ **Not a fourth magnifying glass.** Phosphor has exactly three variants — plain, plus and
/// minus — so a glass for "fit" does not exist. `frame-corners` is the conventional fit-to-window
/// symbol; the departure is deliberate and recorded in the spec.
pub const ZOOM_FIT: char = '\u{e626}';
/// `magnifying-glass` — zoom to 1:1.
pub const ZOOM_ACTUAL: char = '\u{e30c}';

/// `square-split-horizontal` — the before/after toggle.
pub const BEFORE_AFTER: char = '\u{e870}';

/// Every icon the app draws.
///
/// ⚠ The coverage tests sweep this, so an icon missing from it is one nothing checks.
pub const ALL: [char; 11] = [
    EDIT,
    CROP,
    MASKS,
    PRESETS,
    UNDO,
    REDO,
    ZOOM_OUT,
    ZOOM_IN,
    ZOOM_FIT,
    ZOOM_ACTUAL,
    BEFORE_AFTER,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_is_in_the_private_use_area() {
        // ⚠⚠ This catches the one failure the coverage tests structurally cannot. A mistyped
        // constant that lands on a *real* character — say U+0430, a Cyrillic 'а' — resolves
        // happily in Ubuntu-Light and renders as a letter. `has_glyph` returns true, so every
        // coverage test passes, and the button quietly shows the wrong thing.
        //
        // Phosphor's range is the BMP Private Use Area, U+E000..=U+F8FF.
        for c in ALL {
            assert!(
                ('\u{e000}'..='\u{f8ff}').contains(&c),
                "U+{:04X} is not a private-use codepoint, so it is not an icon",
                c as u32
            );
        }
    }

    #[test]
    fn no_two_icons_are_the_same_character() {
        // Two names for one codepoint means one of the two buttons is showing the wrong picture,
        // and reading the table will not reveal which — the constants look different.
        let mut seen = ALL;
        seen.sort_unstable();
        let total = seen.len();
        let mut dedup = seen.to_vec();
        dedup.dedup();
        assert_eq!(dedup.len(), total, "two icons share a codepoint");
    }
}
