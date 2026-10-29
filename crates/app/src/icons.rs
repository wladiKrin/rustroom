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
/// `bandaids` — the Retouch tool.
///
/// ⚠ The plaster, which is Photoshop's healing-brush symbol and reads as "spot removal" at a
/// glance. Verified present in the bundled 2.1.2 TTF by parsing its cmap, not assumed: four
/// separate features in this project have picked a glyph that turned out to be absent from the
/// bundled fonts and would have shipped a tofu box.
pub const RETOUCH: char = '\u{e0b2}';

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

/// `squares-four` — the Library half of the app-mode switch.
pub const LIBRARY: char = '\u{e464}';
/// `sliders` — the Develop half of the app-mode switch.
///
/// ⚠ The vertical sliders, not `sliders-horizontal`: that one is the Edit tool ([`EDIT`]), and
/// the same picture for the module and for one tool inside it would say they are one thing.
pub const DEVELOP: char = '\u{e432}';
/// `grid-four` — the Library's Grid layout.
pub const GRID: char = '\u{e296}';
/// `columns` — Compare, two photos side by side.
pub const COMPARE: char = '\u{e546}';
/// `grid-nine` — Survey, the whole selection at once.
pub const SURVEY: char = '\u{ec8c}';
/// `map-trifold` — the Library's Map view (M10.18b). ⚠ Taken from the 2.1.2 `style.css` (whose
/// `grid-four` agrees with [`GRID`]) and found in the bundled TTF's cmap by `fc-query`.
pub const MAP: char = '\u{e31a}';
/// `users` — the Library's People view (M10.18c). ⚠ From the 2.1.2 `style.css`, found in the bundled TTF's cmap
/// by `fc-query`.
pub const PEOPLE: char = '\u{e4d6}';
/// `funnel` — the Library's Filter menu.
pub const FILTER: char = '\u{e266}';
/// `sort-ascending` — the sort order combo.
pub const SORT: char = '\u{e444}';
/// `export` — the Develop toolbar's Export button.
pub const EXPORT: char = '\u{eaf0}';
/// `dots-three` — a "more" menu.
///
/// ⚠ **Nothing draws this as of 2026-09-23.** It held the ⋯ menu that wrapped Reindex, which became a
/// button of its own once it was clear the menu never held anything else. Kept, glyph and all, because
/// the next Library action wanting that corner will want the menu back — and because `ALL`'s coverage
/// sweep is what proves the glyph is in the bundled font.
pub const MORE: char = '\u{e1fe}';
/// `arrows-out-simple` — the thumbnail-size slider's glyph.
pub const THUMB_SIZE: char = '\u{e0a6}';
/// `gear` — edit a smart collection's rule.
pub const GEAR: char = '\u{e270}';
/// `copy` — make a virtual copy from a snapshot.
pub const COPY: char = '\u{e1ca}';

/// `plus-circle` — the mask mode that starts a new mask.
///
/// ⚠ This and the three below: codepoints read from Phosphor 2.1.2's `src/regular/style.css`
/// (unpkg, 2026-09-29) — the same sheet gave `sliders-horizontal`, `crop`, `circle-half`, `star`
/// and `bandaids` exactly as above — and each found in the bundled TTF's cmap, not assumed.
pub const MASK_NEW: char = '\u{e3d6}';
/// `plus` — the mask mode that adds the next shape to the selected mask.
pub const MASK_ADD: char = '\u{e3d4}';
/// `minus` — the mask mode that cuts the next shape out of the selected mask.
pub const MASK_SUBTRACT: char = '\u{e32a}';
/// `intersect` — the mask mode that keeps only where the mask and the next shape overlap.
pub const MASK_INTERSECT: char = '\u{e2d2}';

/// `paint-brush` — Retouch's Paint task (M10.19d).
///
/// ⚠ This and the next three were looked up by name in the bundled 2.1.2 TTF itself — its ligature table maps each
/// icon's name to its glyph — and the same parse put `bandaids` on [`RETOUCH`] and `magnifying-glass` on
/// [`ZOOM_ACTUAL`], which is what made it trustworthy.
pub const PAINT: char = '\u{e6f0}';
/// `binoculars` — Retouch's Find task: looking for people or dust.
pub const FIND: char = '\u{ea64}';
/// `hand-pointing` — Find's Pick gesture: a click picks or unpicks what was found.
pub const PICK: char = '\u{e29a}';
/// `selection-plus` — Find's Add more gesture: a drawing looks again there and adds what it finds.
pub const ADD_MORE: char = '\u{e69c}';

/// Every icon the app draws.
///
/// ⚠ The coverage tests sweep this, so an icon missing from it is one nothing checks.
pub const ALL: [char; 34] = [
    EDIT,
    CROP,
    MASKS,
    PRESETS,
    RETOUCH,
    UNDO,
    REDO,
    ZOOM_OUT,
    ZOOM_IN,
    ZOOM_FIT,
    ZOOM_ACTUAL,
    BEFORE_AFTER,
    LIBRARY,
    DEVELOP,
    GRID,
    COMPARE,
    SURVEY,
    MAP,
    PEOPLE,
    FILTER,
    SORT,
    EXPORT,
    MORE,
    THUMB_SIZE,
    GEAR,
    COPY,
    MASK_NEW,
    MASK_ADD,
    MASK_SUBTRACT,
    MASK_INTERSECT,
    PAINT,
    FIND,
    PICK,
    ADD_MORE,
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
