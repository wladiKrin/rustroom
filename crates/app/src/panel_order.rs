//! Where a dragged row lands — a panel section, a mask, a mask's component.
//!
//! ⚠⚠ Pure functions with no `egui` in sight, deliberately. The alternative — egui's
//! `dnd_drag_source`/`dnd_drop_zone` — puts this logic inside the framework, where none of it can
//! be tested; and the standing warning in `CLAUDE.md` is that canvas and panel code is untested
//! territory. This is the one part of the gesture that can be made otherwise.

/// Which slot a section dragged to `pointer_y` should land in.
///
/// `header_tops` is each drawn section header's top edge, in order. The result is an *insertion*
/// index in `0..=header_tops.len()`.
///
/// ⚠ A section is claimed by the slot whose header it has been dragged **past the top of**, so
/// dropping anywhere above the first header gives 0 and anywhere below the last gives the end.
///
/// ⚠ Any ascending boundaries work: the mask rows pass their **midpoints** instead
/// (`mask_components::drop_slot`), since a 30-pt row is too short for the past-the-top rule.
pub fn drop_index(header_tops: &[f32], pointer_y: f32) -> usize {
    header_tops.iter().filter(|top| pointer_y >= **top).count()
}

/// The order after moving the item at `from` into position `to`.
///
/// ⚠ Both indices are clamped rather than trusted: `from` comes from a drag that may have started
/// before a re-sort, and `to` from a pointer position. An out-of-range index must not panic in a
/// paint loop.
pub fn moved<T: Clone>(order: &[T], from: usize, to: usize) -> Vec<T> {
    let mut out = order.to_vec();
    move_in_place(&mut out, from, to);
    out
}

/// [`moved`] without the copy — for lists whose items are expensive to clone (a mask's strokes).
///
/// ⚠ The one implementation of the rule; `moved` calls it, so the two cannot disagree.
pub fn move_in_place<T>(v: &mut Vec<T>, from: usize, to: usize) {
    if v.is_empty() || from >= v.len() {
        return;
    }
    let item = v.remove(from);
    // ⚠ `to` is an *insertion* index into the list **including** the item being moved, so removing
    // it first shifts every later slot down by one.
    let to = if to > from { to.saturating_sub(1) } else { to };
    v.insert(to.min(v.len()), item);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::PanelSection;

    const TOPS: [f32; 3] = [10.0, 50.0, 90.0];

    #[test]
    fn a_drop_above_everything_lands_first() {
        assert_eq!(drop_index(&TOPS, 0.0), 0);
    }

    #[test]
    fn a_drop_below_everything_lands_last() {
        assert_eq!(drop_index(&TOPS, 1000.0), 3);
    }

    #[test]
    fn a_drop_between_two_headers_lands_between_them() {
        assert_eq!(drop_index(&TOPS, 60.0), 2);
    }

    /// ⚠ Exactly on a boundary — the case a naive implementation gets wrong and no manual test
    /// would ever notice, because a pointer rarely lands on an exact pixel edge.
    #[test]
    fn a_drop_exactly_on_a_header_top_claims_that_slot() {
        assert_eq!(drop_index(&TOPS, 50.0), 2);
    }

    #[test]
    fn an_empty_panel_has_one_slot() {
        assert_eq!(drop_index(&[], 5.0), 0);
    }

    fn order() -> Vec<PanelSection> {
        PanelSection::ALL.to_vec()
    }

    /// The motivating case, in the words it was asked in: folders below collections.
    #[test]
    fn dragging_folders_below_collections_puts_it_second() {
        let got = moved(&order(), 0, 2);
        assert_eq!(got[0], PanelSection::Collections);
        assert_eq!(got[1], PanelSection::Folders);
        assert_eq!(got.len(), PanelSection::ALL.len());
    }

    #[test]
    fn dragging_upward_works_too() {
        let got = moved(&order(), 4, 0);
        assert_eq!(got[0], PanelSection::Dates);
        assert_eq!(got[1], PanelSection::Folders);
    }

    /// ⚠ Dropping where it already is must be a no-op, not an off-by-one shuffle.
    #[test]
    fn dropping_a_section_where_it_already_is_changes_nothing() {
        for i in 0..PanelSection::ALL.len() {
            assert_eq!(moved(&order(), i, i), order(), "moving {i} onto itself");
            assert_eq!(
                moved(&order(), i, i + 1),
                order(),
                "or onto the slot just after itself"
            );
        }
    }

    /// ⚠ Every move must be a permutation — nothing lost, nothing duplicated. A property over all
    /// pairs, because an off-by-one only shows at particular indices.
    #[test]
    fn every_move_is_a_permutation() {
        let n = PanelSection::ALL.len();
        for from in 0..n {
            for to in 0..=n {
                let got = moved(&order(), from, to);
                assert_eq!(got.len(), n, "moving {from} to {to} changed the length");
                for s in PanelSection::ALL {
                    assert!(got.contains(&s), "moving {from} to {to} lost {s:?}");
                }
            }
        }
    }

    /// ⚠ Out-of-range indices are clamped, never a panic: this runs inside a paint loop.
    #[test]
    fn out_of_range_indices_do_not_panic() {
        assert_eq!(moved(&order(), 99, 0), order());
        assert_eq!(moved(&order(), 0, 99).len(), PanelSection::ALL.len());
        assert!(moved::<PanelSection>(&[], 0, 0).is_empty());
    }
}
