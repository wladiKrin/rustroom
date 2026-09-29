//! What a bar shows at a given width — `docs/DESIGN.md` §6.
//!
//! ⚠⚠ **The Library bar overprinted itself at 1440 pt** (the 2026-09-29 survey: the status text
//! ran under the search field and Filter clipped to "Fi"), because its controls were laid out by
//! hope — each placed where the previous one ended, nothing asking whether the row had room.
//! This decides that first: the bin measures each item, this picks a form for each, and only
//! then does anything draw. Pure, so the rule is tested at every width without a window.
//!
//! The rule, in order: while the row is too wide, **drop labels** (an item's compact form — an
//! icon, a narrower field) from the lowest priority up; then, still too wide, **fold** hideable
//! items away (into the ⋯ menu, or simply not drawn) from the lowest priority up. An item that
//! has neither form is fixed and always shows.

/// How an item is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Form {
    /// With its label.
    Full,
    /// Its label dropped, or narrowed.
    Compact,
    /// Not drawn in the row (folded into ⋯ where the bar has one).
    Hidden,
}

/// One item of a bar, as measured by the bin.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Item {
    /// Higher stays longer.
    pub priority: u8,
    /// Its width in its full form.
    pub full: f32,
    /// Its width in its compact form, when it has one.
    pub compact: Option<f32>,
    /// Whether it may fold away.
    pub hideable: bool,
}

impl Item {
    /// An item that always shows at its one width.
    pub fn fixed(priority: u8, width: f32) -> Self {
        Self {
            priority,
            full: width,
            compact: None,
            hideable: false,
        }
    }

    fn width(&self, form: Form) -> f32 {
        match form {
            Form::Full => self.full,
            Form::Compact => self.compact.unwrap_or(self.full),
            Form::Hidden => 0.0,
        }
    }
}

/// The width `items` take in `forms`, with `gap` between every two that show.
pub fn width(items: &[Item], forms: &[Form], gap: f32) -> f32 {
    let shown: Vec<f32> = items
        .iter()
        .zip(forms)
        .filter(|(_, f)| **f != Form::Hidden)
        .map(|(i, f)| i.width(*f))
        .collect();
    shown.iter().sum::<f32>() + gap * shown.len().saturating_sub(1) as f32
}

/// A form for every item so the row fits `avail`, or as close as the fixed items allow.
///
/// ⚠ Ties in priority step down **right to left** — the item further from the row's anchor goes
/// first — so two items of equal rank do not alternate as the window is dragged.
pub fn plan(avail: f32, gap: f32, items: &[Item]) -> Vec<Form> {
    let mut forms = vec![Form::Full; items.len()];
    // Lowest priority first; among equals, the later item first.
    let mut order: Vec<usize> = (0..items.len()).collect();
    order.sort_by_key(|&i| (items[i].priority, std::cmp::Reverse(i)));
    for i in order.iter().copied() {
        if width(items, &forms, gap) <= avail {
            return forms;
        }
        if items[i].compact.is_some() {
            forms[i] = Form::Compact;
        }
    }
    for i in order {
        if width(items, &forms, gap) <= avail {
            return forms;
        }
        if items[i].hideable {
            forms[i] = Form::Hidden;
        }
    }
    forms
}

/// A `WxH` size in points, for the `RUSTROOM_DEBUG_WINDOW` switch — the widths this module is
/// checked at are the widths a person needs to open the window at.
pub fn parse_size(v: &str) -> Option<(f32, f32)> {
    let (w, h) = v.trim().split_once(['x', 'X'])?;
    let (w, h) = (w.trim().parse::<f32>().ok()?, h.trim().parse::<f32>().ok()?);
    (w >= 200.0 && h >= 200.0).then_some((w, h))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_size_parses_and_nonsense_does_not() {
        assert_eq!(parse_size("1440x875"), Some((1440.0, 875.0)));
        assert_eq!(parse_size(" 800 X 600 "), Some((800.0, 600.0)));
        for bad in ["", "1440", "x875", "1440x", "wide", "10x10"] {
            assert_eq!(parse_size(bad), None, "{bad:?}");
        }
    }

    fn item(priority: u8, full: f32, compact: Option<f32>, hideable: bool) -> Item {
        Item {
            priority,
            full,
            compact,
            hideable,
        }
    }

    #[test]
    fn everything_shows_when_there_is_room() {
        let items = [
            item(90, 100.0, Some(40.0), false),
            item(10, 80.0, None, true),
        ];
        assert_eq!(plan(1000.0, 8.0, &items), vec![Form::Full, Form::Full]);
    }

    #[test]
    fn labels_drop_before_anything_hides() {
        // The low item could hide, the high one could compact: compacting comes first even though
        // it touches the higher priority — a label is cheaper to lose than a control.
        let items = [
            item(90, 200.0, Some(60.0), false),
            item(10, 100.0, None, true),
        ];
        // Full: 200 + 8 + 100 = 308. Compacting the first: 60 + 8 + 100 = 168.
        assert_eq!(plan(200.0, 8.0, &items), vec![Form::Compact, Form::Full]);
    }

    #[test]
    fn the_lowest_priority_hides_first() {
        let items = [
            item(50, 100.0, None, true),
            item(20, 100.0, None, true),
            item(90, 100.0, None, false),
        ];
        // 3 × 100 + 2 × 8 = 316; one hidden = 208.
        assert_eq!(
            plan(250.0, 8.0, &items),
            vec![Form::Full, Form::Hidden, Form::Full]
        );
        assert_eq!(
            plan(120.0, 8.0, &items),
            vec![Form::Hidden, Form::Hidden, Form::Full]
        );
    }

    #[test]
    fn a_fixed_item_never_changes() {
        let items = [Item::fixed(0, 500.0), item(99, 100.0, None, true)];
        // Too narrow even for the fixed item alone: the hideable one goes, the fixed one stays.
        assert_eq!(plan(100.0, 8.0, &items), vec![Form::Full, Form::Hidden]);
    }

    #[test]
    fn gaps_are_counted_only_between_shown_items() {
        let items = [item(1, 50.0, None, true), item(2, 50.0, None, true)];
        assert_eq!(width(&items, &[Form::Full, Form::Full], 10.0), 110.0);
        assert_eq!(width(&items, &[Form::Hidden, Form::Full], 10.0), 50.0);
        assert_eq!(width(&items, &[Form::Hidden, Form::Hidden], 10.0), 0.0);
    }

    #[test]
    fn equal_priorities_step_down_from_the_far_end() {
        let items = [item(40, 100.0, None, true), item(40, 100.0, None, true)];
        assert_eq!(plan(150.0, 8.0, &items), vec![Form::Full, Form::Hidden]);
    }

    /// Items shaped like the Library's status row, measured on the real app at 13 pt: the status
    /// line, two format chips, the sort combo, the size slider and ⋯.
    fn status_row() -> Vec<Item> {
        vec![
            item(90, 160.0, Some(90.0), false),
            item(40, 150.0, None, true),
            item(80, 110.0, Some(36.0), false),
            item(60, 130.0, None, true),
            Item::fixed(99, 28.0),
        ]
    }

    #[test]
    fn the_result_fits_whenever_the_fixed_items_do() {
        // ⚠ A sweep rather than three widths: the bug this module exists for appeared at one width
        // nobody had tried. Every width either fits, or every item is already at its minimum.
        let items = status_row();
        let minimum = width(
            &items,
            &items
                .iter()
                .map(|i| {
                    if i.hideable {
                        Form::Hidden
                    } else if i.compact.is_some() {
                        Form::Compact
                    } else {
                        Form::Full
                    }
                })
                .collect::<Vec<_>>(),
            8.0,
        );
        assert!(minimum > 100.0, "sample: the fixed floor is {minimum}");
        let mut saw_hidden = false;
        let mut saw_full = false;
        for avail in (100..=2000).step_by(10).map(|w| w as f32) {
            let forms = plan(avail, 8.0, &items);
            let w = width(&items, &forms, 8.0);
            assert!(
                w <= avail || (w - minimum).abs() < 0.01,
                "at {avail}: {w} wide, {forms:?}"
            );
            saw_hidden |= forms.contains(&Form::Hidden);
            saw_full |= forms.iter().all(|f| *f == Form::Full);
        }
        // ⚠ Both ends reached, or the sweep proved nothing about folding.
        assert!(
            saw_hidden && saw_full,
            "the sweep never folded or never fitted"
        );
    }
}
