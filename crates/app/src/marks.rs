//! What a key does to a photo's marks, and which photos a star filter shows.
//!
//! ⚠ Pure, so both rules are testable without a window — and both are easy to get subtly wrong:
//! which key clears, and whether an unrated photo passes a "1 star or more" filter.

use catalog::db::{Flag, Marks};

/// What a culling key asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkAction {
    Rating(u8),
    Flag(Flag),
    Label(u8),
}

/// The culling key for `c`, if it is one.
///
/// ⚠⚠ **Library only.** `1`–`4` select rail tools in Develop, which is exactly where Lightroom
/// puts star ratings — the collision is resolved by view, because the rail exists only in Develop
/// and culling happens in the grid.
pub fn action_for(c: char) -> Option<MarkAction> {
    match c {
        // ⚠ `0` clears rather than being "zero stars": they are the same value, and calling it
        // "clear" is what makes the key discoverable.
        '0'..='5' => Some(MarkAction::Rating(c as u8 - b'0')),
        '6'..='9' => Some(MarkAction::Label(c as u8 - b'5')),
        'p' | 'P' => Some(MarkAction::Flag(Flag::Pick)),
        'x' | 'X' => Some(MarkAction::Flag(Flag::Reject)),
        'u' | 'U' => Some(MarkAction::Flag(Flag::None)),
        _ => None,
    }
}

/// Apply an action to a photo's marks.
pub fn apply(marks: Marks, action: MarkAction) -> Marks {
    match action {
        MarkAction::Rating(r) => Marks { rating: r, ..marks },
        MarkAction::Flag(f) => Marks { flag: f, ..marks },
        // ⚠ Pressing the same colour again clears it, so one key both sets and unsets. Without
        // this there is no way to remove a label at all.
        MarkAction::Label(l) => Marks {
            label: if marks.label == l { 0 } else { l },
            ..marks
        },
    }
}

/// Whether `marks` passes an "at least this many stars" filter.
///
/// ⚠ `min == 0` shows everything **including rejected photos**. Hiding rejects by default would
/// make them look deleted, and nothing here deletes anything.
pub fn passes(marks: Marks, min: u8) -> bool {
    marks.rating >= min
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_digits_map_to_stars_and_colours() {
        assert_eq!(action_for('0'), Some(MarkAction::Rating(0)));
        assert_eq!(action_for('5'), Some(MarkAction::Rating(5)));
        assert_eq!(action_for('6'), Some(MarkAction::Label(1)));
        assert_eq!(action_for('9'), Some(MarkAction::Label(4)));
    }

    #[test]
    fn the_flag_keys_are_lightrooms() {
        assert_eq!(action_for('p'), Some(MarkAction::Flag(Flag::Pick)));
        assert_eq!(action_for('x'), Some(MarkAction::Flag(Flag::Reject)));
        assert_eq!(action_for('u'), Some(MarkAction::Flag(Flag::None)));
        assert_eq!(action_for('P'), Some(MarkAction::Flag(Flag::Pick)));
    }

    #[test]
    fn an_unbound_key_does_nothing() {
        // ⚠ The discriminating half: a mapping that answered `Some` for everything would make
        // every keystroke in the grid rate a photo.
        for c in ['a', 'z', ' ', '-', 'G'] {
            assert_eq!(action_for(c), None, "{c:?} should not be a culling key");
        }
    }

    #[test]
    fn setting_a_rating_leaves_the_flag_and_label_alone() {
        // ⚠ Three orthogonal marks. A rating key that reset the flag would silently undo a cull.
        let before = Marks {
            rating: 1,
            flag: Flag::Pick,
            label: 3,
        };
        let after = apply(before, MarkAction::Rating(4));
        assert_eq!(after.rating, 4);
        assert_eq!(after.flag, Flag::Pick);
        assert_eq!(after.label, 3);
    }

    #[test]
    fn pressing_the_same_colour_twice_clears_it() {
        // ⚠ Without this there is no way to remove a label at all.
        let m = apply(Marks::default(), MarkAction::Label(2));
        assert_eq!(m.label, 2);
        assert_eq!(apply(m, MarkAction::Label(2)).label, 0);
        assert_eq!(
            apply(m, MarkAction::Label(3)).label,
            3,
            "a different colour replaces"
        );
    }

    #[test]
    fn zero_clears_the_rating() {
        let m = apply(Marks::default(), MarkAction::Rating(4));
        assert_eq!(apply(m, MarkAction::Rating(0)).rating, 0);
    }

    #[test]
    fn a_zero_star_filter_shows_everything_including_rejects() {
        // ⚠ Hiding rejected photos by default would make them look deleted, and nothing here
        // deletes anything.
        let rejected = Marks {
            rating: 0,
            flag: Flag::Reject,
            label: 0,
        };
        assert!(passes(rejected, 0));
        assert!(passes(Marks::default(), 0));
    }

    #[test]
    fn a_star_filter_is_a_minimum_not_an_exact_match() {
        // ⚠ "3 stars" means "3 or better". An exact match would hide your best photographs from a
        // filter meant to find them, which is the wrong way round to be wrong.
        let three = Marks {
            rating: 3,
            ..Marks::default()
        };
        let five = Marks {
            rating: 5,
            ..Marks::default()
        };
        assert!(passes(three, 3));
        assert!(passes(five, 3), "5 stars must pass a 3-star filter");
        assert!(!passes(three, 4));
    }
}
