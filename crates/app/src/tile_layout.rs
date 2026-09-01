//! How several photos tile one viewport — the arithmetic behind Compare (X|Y) and Survey (N).
//!
//! ⚠ Deliberately egui-free — plain `f32` and a local [`Tile`] — following `thumb_window` and
//! `view_region`, whose own docs say they are kept that way so the geometry is testable without
//! a window. Tiling is exactly the kind of thing no screenshot can check: that the cells cover
//! the viewport without overlapping, and that none of them escapes it, is a *property*, and a
//! property is what a test can hold.

use crate::thumb_cache::{THUMB_BUDGET_BYTES, TYPICAL_THUMB_BYTES};

/// One photo's rectangle inside the viewport, with the viewport's top-left as the origin.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tile {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Tile {
    fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    /// Whether two tiles share any area. Touching edges do not count.
    pub fn overlaps(&self, other: &Tile) -> bool {
        self.x < other.right()
            && other.x < self.right()
            && self.y < other.bottom()
            && other.y < self.bottom()
    }
}

/// The nominal photo shape the column chooser fits into a cell.
///
/// ⚠ A *nominal* aspect, not each photo's own: the columns must be chosen before any thumbnail
/// has necessarily arrived, and a layout that reflowed as portraits loaded would be the
/// filmstrip's reflow bug in a new costume. 3:2 is the commonest frame this app will ever see.
pub const NOMINAL_ASPECT: f32 = 1.5;

/// The most photos Survey will tile at once.
///
/// ⚠⚠ **A memory bound before it is a legibility one, and that is why it is not merely a
/// preference.** `ThumbCache::retain_window` **pins** everything on screen — "nothing in
/// `window` is ever evicted, however far the budget falls" — so an uncapped Survey over a
/// 900-photo selection would pin ~600 MB past a 512 MB budget and the cache could not shed a
/// byte. The compile-time assertion below is the half that must hold; 24 is where a tile is
/// still large enough to judge on a normal screen.
pub const MAX_TILES: usize = 24;

// ⚠ **Module-level**, not associated: an associated `const` inside an `impl` is only evaluated
// where it is used, so `PrefixCache`'s first attempt at exactly this compiled happily with a
// budget of zero. The rule `thumb_cache` already states, for the same reason.
//
// ⚠ The *reasoning*, not merely the number: raising `MAX_TILES` past what the budget holds
// brings back an unevictable pinned window, and a comment alone would not stop it.
const _: () = assert!(MAX_TILES * TYPICAL_THUMB_BYTES <= THUMB_BUDGET_BYTES);

/// How many columns `n` cells want in a `w × h` viewport.
///
/// Every column count is scored by the size of an `aspect`-shaped photo fitted into the cell it
/// produces, and the largest wins — so a wide viewport lays two photos side by side and a tall
/// narrow one stacks them, with no orientation rule of its own.
pub fn columns(n: usize, w: f32, h: f32, gap: f32, aspect: f32) -> usize {
    // ⚠ `!is_finite()` first, then `<= 0.0` — never a bare `!(aspect > 0.0)`, which clippy
    // rejects and which reads as an accident even though it is the NaN-safe spelling.
    if n == 0 || !usable(w, h) || !aspect.is_finite() || aspect <= 0.0 {
        return 1;
    }
    let mut best = (1usize, f32::NEG_INFINITY);
    for c in 1..=n {
        let rows = n.div_ceil(c);
        let cw = (w - (c - 1) as f32 * gap) / c as f32;
        let ch = (h - (rows - 1) as f32 * gap) / rows as f32;
        if cw <= 0.0 || ch <= 0.0 {
            continue;
        }
        // The long edge of the largest `aspect`-shaped photo this cell can hold.
        let fitted = (cw / aspect).min(ch);
        if fitted > best.1 {
            best = (c, fitted);
        }
    }
    best.0
}

/// `n` tiles in `cols` columns, filling a `w × h` viewport.
///
/// ⚠ **The last, partial row is centred**, which is what stops a survey of five in three columns
/// reading as a three-and-a-gap. A full row is centred too and lands at x = 0 exactly, so there
/// is no special case for it.
pub fn tiles(n: usize, cols: usize, w: f32, h: f32, gap: f32) -> Vec<Tile> {
    if n == 0 || cols == 0 || !usable(w, h) || !gap.is_finite() || gap < 0.0 {
        return Vec::new();
    }
    let rows = n.div_ceil(cols);
    // ⚠ `n` cells need `n*cell + (n-1)*gap`, **not** `n*cell` — the arithmetic the Library grid's
    // column count was once wrong about, and wrong in a way that only showed on a wide monitor.
    let cw = ((w - (cols - 1) as f32 * gap) / cols as f32).max(0.0);
    let ch = ((h - (rows - 1) as f32 * gap) / rows as f32).max(0.0);
    (0..n)
        .map(|i| {
            let row = i / cols;
            let col = i % cols;
            let in_row = cols.min(n - row * cols);
            let row_w = in_row as f32 * cw + (in_row - 1) as f32 * gap;
            let x0 = ((w - row_w) * 0.5).max(0.0);
            Tile::new(
                x0 + col as f32 * (cw + gap),
                row as f32 * (ch + gap),
                cw,
                ch,
            )
        })
        .collect()
}

/// Survey's layout: as many photos as are given, in whatever column count fits best.
pub fn survey(n: usize, w: f32, h: f32, gap: f32) -> Vec<Tile> {
    tiles(n, columns(n, w, h, gap, NOMINAL_ASPECT), w, h, gap)
}

/// Compare's two panes.
///
/// ⚠⚠ **Always side by side, never stacked — this deliberately does not call [`columns`].**
/// Compare exists to judge two frames of the same subject against each other, and the eye
/// compares two things far more readily left-to-right than top-to-bottom; Lightroom's Compare is
/// side by side at every window shape for the same reason. Letting the chooser stack them on a
/// portrait window would make the view's whole point depend on the window's shape.
pub fn pair(w: f32, h: f32, gap: f32) -> Vec<Tile> {
    tiles(2, 2, w, h, gap)
}

/// Whether a viewport has usable extent. ⚠ `!is_finite()` comes first and is not redundant:
/// `w <= 0.0` is `false` for `NaN`, so a NaN width would sail past a `<=` test alone.
fn usable(w: f32, h: f32) -> bool {
    w.is_finite() && h.is_finite() && w > 0.0 && h > 0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: f32 = 1600.0;
    const H: f32 = 900.0;
    const GAP: f32 = 8.0;

    /// The three properties every layout must have, checked together because each is a
    /// different mistake: a dropped cell, an overlap, and a cell hanging off the edge.
    fn check(tiles: &[Tile], n: usize, w: f32, h: f32) {
        assert_eq!(tiles.len(), n, "wrong number of tiles");
        for (i, t) in tiles.iter().enumerate() {
            assert!(
                t.w >= 0.0 && t.h >= 0.0,
                "tile {i} has negative size: {t:?}"
            );
            assert!(
                t.x >= -0.01 && t.y >= -0.01 && t.right() <= w + 0.01 && t.bottom() <= h + 0.01,
                "tile {i} escapes the viewport: {t:?} in {w}x{h}"
            );
        }
        for i in 0..tiles.len() {
            for j in (i + 1)..tiles.len() {
                assert!(
                    !tiles[i].overlaps(&tiles[j]),
                    "tiles {i} and {j} overlap: {:?} / {:?}",
                    tiles[i],
                    tiles[j]
                );
            }
        }
    }

    #[test]
    fn every_survey_size_tiles_the_viewport_without_overlapping_or_escaping() {
        // ⚠ Swept rather than spot-checked: the partial-row centring and the gap arithmetic are
        // both only wrong at *particular* counts, and a single n would miss whichever one it is.
        for n in 1..=MAX_TILES {
            check(&survey(n, W, H, GAP), n, W, H);
        }
    }

    #[test]
    fn every_survey_size_tiles_a_tall_narrow_viewport_too() {
        // A portrait window is where the column chooser makes its opposite choice, so the same
        // properties have to hold there — and a gap term that was right in one orientation and
        // wrong in the other would pass a landscape-only sweep.
        for n in 1..=MAX_TILES {
            check(&survey(n, 500.0, 1400.0, GAP), n, 500.0, 1400.0);
        }
    }

    #[test]
    fn the_gaps_really_separate_the_tiles() {
        // ⚠ Not implied by the no-overlap check: tiles that merely *touch* do not overlap, so a
        // layout ignoring `gap` entirely passes that and looks wrong on screen.
        let t = tiles(4, 2, W, H, GAP);
        assert!(
            (t[1].x - t[0].right() - GAP).abs() < 0.01,
            "no horizontal gap: {:?} then {:?}",
            t[0],
            t[1]
        );
        assert!(
            (t[2].y - t[0].bottom() - GAP).abs() < 0.01,
            "no vertical gap: {:?} then {:?}",
            t[0],
            t[2]
        );
    }

    #[test]
    fn a_full_row_starts_at_the_left_edge() {
        // The centring must be a no-op on a full row, or every layout would be inset by half a
        // gap and the viewport would never be filled.
        let t = tiles(4, 2, W, H, GAP);
        assert!(t[0].x.abs() < 0.01, "{:?}", t[0]);
    }

    #[test]
    fn a_partial_last_row_is_centred() {
        // ⚠ Five in three columns: the last row holds two, and they belong in the middle rather
        // than pushed left leaving a hole. The discriminating assertion is *symmetry* — the space
        // to the left of the row equals the space to its right.
        let t = tiles(5, 3, W, H, GAP);
        let left = t[3].x;
        let right = W - t[4].right();
        assert!(left > 1.0, "the last row was not inset at all: {:?}", t[3]);
        assert!(
            (left - right).abs() < 0.01,
            "last row is not centred: {left} left, {right} right"
        );
    }

    #[test]
    fn a_wide_viewport_puts_two_photos_side_by_side() {
        assert_eq!(columns(2, W, H, GAP, NOMINAL_ASPECT), 2);
    }

    #[test]
    fn a_tall_narrow_viewport_stacks_two_photos() {
        // ⚠ The other half of the chooser. Without it a 400 pt wide window would put two photos
        // in 196 pt columns, which is smaller than stacking them — and the same test with only
        // the wide case would pass against a `columns` hardcoded to 2.
        assert_eq!(columns(2, 400.0, 1200.0, GAP, NOMINAL_ASPECT), 1);
    }

    #[test]
    fn the_chooser_picks_the_count_that_makes_the_photos_largest() {
        // The property behind both cases above, stated over a sweep: no other column count can
        // fit a larger photo than the one chosen.
        for n in 1..=12 {
            let best = columns(n, W, H, GAP, NOMINAL_ASPECT);
            let fitted = |c: usize| {
                let rows = n.div_ceil(c);
                let cw = (W - (c - 1) as f32 * GAP) / c as f32;
                let ch = (H - (rows - 1) as f32 * GAP) / rows as f32;
                (cw / NOMINAL_ASPECT).min(ch)
            };
            for c in 1..=n {
                assert!(
                    fitted(c) <= fitted(best) + 0.01,
                    "n = {n}: {c} columns fit {} > {} at the chosen {best}",
                    fitted(c),
                    fitted(best)
                );
            }
        }
    }

    #[test]
    fn compare_stays_side_by_side_on_a_portrait_window() {
        // ⚠⚠ The decision `pair` exists to carry. `columns(2, ..)` would stack these, and
        // Compare must not: the view's whole point is a left-to-right judgement. Sabotaging
        // `pair` into `survey(2, ..)` fails exactly here and nowhere else.
        let t = pair(500.0, 1400.0, GAP);
        check(&t, 2, 500.0, 1400.0);
        assert!(
            t[1].x > t[0].x && (t[0].y - t[1].y).abs() < 0.01,
            "the panes are not side by side: {:?} / {:?}",
            t[0],
            t[1]
        );
        assert!(
            (t[0].w - t[1].w).abs() < 0.01,
            "the panes are not equal: {:?} / {:?}",
            t[0],
            t[1]
        );
    }

    #[test]
    fn a_degenerate_viewport_yields_no_tiles_rather_than_nonsense() {
        // ⚠ NaN is the one that matters: `as` casts saturate and comparisons against NaN are
        // false, so without the guard the caller would be handed NaN rects that draw nothing and
        // hit-test to nothing — a view that is silently dead rather than obviously broken.
        for (w, h) in [
            (0.0, H),
            (W, 0.0),
            (-10.0, H),
            (f32::NAN, H),
            (W, f32::INFINITY),
        ] {
            assert!(survey(4, w, h, GAP).is_empty(), "{w} x {h}");
            assert!(pair(w, h, GAP).is_empty(), "{w} x {h}");
        }
        assert!(survey(0, W, H, GAP).is_empty(), "zero photos");
    }

    #[test]
    fn a_viewport_too_small_for_its_tiles_still_yields_them_all() {
        // A window dragged very small must not make photos disappear from the survey: the tiles
        // collapse toward zero and come back, rather than the view emptying itself.
        let t = survey(9, 20.0, 20.0, GAP);
        check(&t, 9, 20.0, 20.0);
    }
}
