//! Which two photos Compare shows, and how the arrow keys move the candidate.
//!
//! Compare has a **reference** — the frame you are currently keeping, the Library's active photo
//! — and a **candidate**, the one being judged against it. Everything here is about picking and
//! moving the candidate.
//!
//! ⚠ Pure and egui-free, the `selection` / `crop_tool` / `thumb_window` precedent, so the
//! wrapping and the sparse-index handling are testable rather than only visible as the wrong
//! photograph on screen.
//!
//! ⚠⚠ **Every index here is an index into `entries`, never a position in `visible`.** Both
//! `Library::visible` and `Selection` hold entry indices, so the two compose directly — and
//! conflating an index with a position is the bug this project has now made three times.

/// The photos the candidate moves through.
///
/// ⚠⚠ **The selection when two or more photos are selected, else everything visible.** That is
/// the one decision in this module, and it is what makes both readings of "compare these" work:
/// pick two in the grid and press `C` to compare exactly those, or press `C` with one photo
/// selected to walk the whole folder against it. It is provisional — see the module's entry in
/// the report — but it is confined to this function.
///
/// ⚠ Intersected **in `visible` order**, so a filter that hides a selected photo cannot put it
/// back on screen through Compare. Both slices must be ascending, which `Selection::iter` and
/// `Library::visible` both guarantee — the contract `Selection::retain_visible` already states.
pub fn pool(visible: &[usize], selected: &[usize]) -> Vec<usize> {
    if selected.len() < 2 {
        return visible.to_vec();
    }
    visible
        .iter()
        .copied()
        .filter(|i| selected.binary_search(i).is_ok())
        .collect()
}

/// The pool minus the reference, rotated to begin just after it.
///
/// The rotation is what makes "the next one" mean the photo after the reference rather than the
/// first in the folder, and it makes stepping wrap correctly for free — a cycle is the same
/// cycle whichever entry it starts at.
fn others(pool: &[usize], reference: usize) -> Vec<usize> {
    let start = pool
        .iter()
        .position(|&i| i == reference)
        .map(|p| p + 1)
        .unwrap_or(0);
    pool.iter()
        .cycle()
        .skip(start)
        .take(pool.len())
        .copied()
        .filter(|&i| i != reference)
        .collect()
}

/// The candidate to show beside `reference`.
///
/// `stored` is whatever the app remembered; it is honoured only while it is still a real choice,
/// so a candidate whose photo was filtered away, deleted or promoted falls back to the next one
/// rather than leaving the pane blank.
///
/// ⚠ Self-healing by construction rather than by invalidation: nothing has to remember to clear
/// the stored candidate when the library is rebuilt, which is the whole reason it is resolved
/// here every frame instead of being kept as an index.
pub fn candidate(pool: &[usize], reference: usize, stored: Option<usize>) -> Option<usize> {
    let others = others(pool, reference);
    match stored {
        Some(s) if others.contains(&s) => Some(s),
        _ => others.first().copied(),
    }
}

/// Move the candidate one step through the pool, skipping the reference and wrapping.
///
/// ⚠ Wraps rather than stopping at the ends, unlike the grid's arrow keys. Compare is a loop
/// over a handful of frames you are choosing between, not a walk through a folder, and running
/// off the end of a two-photo comparison would leave the key doing nothing.
pub fn step(
    pool: &[usize],
    reference: usize,
    current: Option<usize>,
    forward: bool,
) -> Option<usize> {
    let others = others(pool, reference);
    if others.is_empty() {
        return None;
    }
    let pos = current
        .and_then(|c| others.iter().position(|&i| i == c))
        .unwrap_or(0);
    let n = others.len();
    let next = if forward {
        (pos + 1) % n
    } else {
        (pos + n - 1) % n
    };
    Some(others[next])
}

#[cfg(test)]
mod tests {
    use super::*;

    const VISIBLE: [usize; 5] = [0, 2, 4, 6, 8];

    #[test]
    fn one_selected_photo_compares_against_the_whole_visible_list() {
        assert_eq!(pool(&VISIBLE, &[4]), VISIBLE.to_vec());
        assert_eq!(pool(&VISIBLE, &[]), VISIBLE.to_vec());
    }

    #[test]
    fn two_or_more_selected_photos_are_the_whole_pool() {
        assert_eq!(pool(&VISIBLE, &[2, 6]), vec![2, 6]);
    }

    #[test]
    fn a_selected_photo_that_the_filter_hides_never_reaches_the_pool() {
        // ⚠⚠ Tested against a **sparse** visible list, because that is where the two index
        // spaces could be confused. `retain_visible` should have pruned this already, so this is
        // defence in depth — and the failure it guards against is showing a photo the filter
        // says is not there, which reads as the filter being broken.
        assert_eq!(pool(&VISIBLE, &[2, 3, 6]), vec![2, 6]);
    }

    #[test]
    fn the_candidate_is_the_photo_after_the_reference() {
        // ⚠ *After* it, not the first in the list — pressing `C` on the third frame of a burst
        // must offer the fourth, not send you back to the first.
        assert_eq!(candidate(&VISIBLE, 4, None), Some(6));
    }

    #[test]
    fn the_candidate_is_never_the_reference_itself() {
        for &r in VISIBLE.iter() {
            let c = candidate(&VISIBLE, r, Some(r));
            assert_ne!(c, Some(r), "reference {r} was offered as its own candidate");
            assert!(c.is_some(), "reference {r} got no candidate at all");
        }
    }

    #[test]
    fn a_remembered_candidate_is_kept() {
        assert_eq!(candidate(&VISIBLE, 0, Some(8)), Some(8));
    }

    #[test]
    fn a_remembered_candidate_that_has_gone_falls_back() {
        // ⚠ The self-healing half. A candidate filtered away, deleted, or just promoted to
        // reference must not leave the right-hand pane empty with no way to refill it.
        assert_eq!(candidate(&VISIBLE, 0, Some(99)), Some(2));
    }

    #[test]
    fn a_pool_of_one_offers_no_candidate() {
        // The empty state Compare has to draw: one photo cannot be compared with anything.
        assert_eq!(candidate(&[4], 4, None), None);
        assert_eq!(candidate(&[], 4, None), None);
        assert_eq!(step(&[4], 4, None, true), None);
    }

    #[test]
    fn stepping_walks_the_pool_and_wraps_both_ways() {
        // Reference 4, so the others in rotated order are 6, 8, 0, 2.
        let p = &VISIBLE[..];
        assert_eq!(step(p, 4, Some(6), true), Some(8));
        assert_eq!(step(p, 4, Some(2), true), Some(6), "forward did not wrap");
        assert_eq!(step(p, 4, Some(6), false), Some(2), "back did not wrap");
        assert_eq!(step(p, 4, Some(8), false), Some(6));
    }

    #[test]
    fn stepping_never_lands_on_the_reference() {
        // ⚠ The property, over every start and both directions: a step that could land on the
        // reference would show the same photograph in both panes, which is the one state
        // Compare must never be in. A `(pos + 1) % pool.len()` on the *unfiltered* pool does
        // exactly that, and only a sweep catches it — a single step from a single start does not.
        let p = &VISIBLE[..];
        for &r in VISIBLE.iter() {
            let mut cur = candidate(p, r, None);
            for _ in 0..(2 * p.len()) {
                for forward in [true, false] {
                    let next = step(p, r, cur, forward);
                    assert!(next.is_some(), "ran out of candidates for reference {r}");
                    assert_ne!(next, Some(r), "stepped onto the reference {r}");
                }
                cur = step(p, r, cur, true);
            }
        }
    }

    #[test]
    fn stepping_from_an_unknown_candidate_still_moves() {
        // A stale candidate plus an arrow key must go somewhere rather than doing nothing.
        assert!(step(&VISIBLE, 4, Some(99), true).is_some());
    }
}
