//! Reordering masks and a mask's components: which drops are offered, what the lists become, and
//! where a selection lands.
//!
//! ⚠ No egui. The rows' drag lives in `develop/panel/{masks,mask_components}.rs`; everything a drop
//! *decides* is here, where it can be tested without a window — `panel_order`'s rule.

use edit_model::{Mask, MaskOp, MaskedAdjustment, Refinement};

use crate::panel_order::{move_in_place, moved};

/// Where the item at `i` ends up after [`moved`]`(_, from, to)` on a list of `len`.
///
/// ⚠ Read off `moved` itself rather than re-derived by arithmetic: two rules for one move is how a
/// selection comes to follow a different row than the one that moved. An `i` outside the list is
/// returned unchanged.
pub fn index_after_move(i: usize, from: usize, to: usize, len: usize) -> usize {
    let order = moved(&(0..len).collect::<Vec<_>>(), from, to);
    order.iter().position(|&k| k == i).unwrap_or(i)
}

/// A mask's components in fold order: `None` for the base, the op for each refinement.
fn ops(m: &MaskedAdjustment) -> Vec<Option<MaskOp>> {
    std::iter::once(None)
        .chain(m.refine.iter().map(|r| Some(r.op)))
        .collect()
}

/// Whether dropping component `from` into slot `to` is offered.
///
/// ⚠⚠ **The row that ends on top must add** (the author's rule, 2026-09-29). It becomes the base,
/// and the base is an Add onto nothing; a Subtract or Intersect there would fold onto zero coverage
/// and leave nothing. Refusing the drop keeps every shape meaning what it meant — the other choice
/// was to drop its op and silently turn a hole into the shape itself.
pub fn component_drop_allowed(m: &MaskedAdjustment, from: usize, to: usize) -> bool {
    let ops = ops(m);
    if from >= ops.len() {
        return false;
    }
    matches!(moved(&ops, from, to)[0], None | Some(MaskOp::Add))
}

/// Move component `from` into slot `to`, keeping `selected` on the shape it was on.
///
/// The top row's shape becomes `mask`; every other row is a [`Refinement`] keeping its op, the old
/// base as `Add` — exact, because the base *is* an Add onto nothing. `adjustments` and `disabled`
/// are untouched.
///
/// Returns whether anything changed: false for a refused drop, an out-of-range `from`, or a drop
/// where the row already was — so the caller records an undo step only for a real move.
pub fn move_component(
    m: &mut MaskedAdjustment,
    selected: &mut usize,
    from: usize,
    to: usize,
) -> bool {
    let n = 1 + m.refine.len();
    let idx: Vec<usize> = (0..n).collect();
    let order = moved(&idx, from, to);
    if from >= n || order == idx || !component_drop_allowed(m, from, to) {
        return false;
    }
    let base = std::mem::take(&mut m.mask);
    let refine = std::mem::take(&mut m.refine);
    let mut comps: Vec<Option<(Option<MaskOp>, Mask)>> = std::iter::once(Some((None, base)))
        .chain(refine.into_iter().map(|r| Some((Some(r.op), r.mask))))
        .collect();
    let mut taken = order
        .iter()
        .map(|&k| comps[k].take().expect("a permutation names each component once"));
    // ⚠ Its op is `None` (the old base) or `Add` — `component_drop_allowed` said so above.
    let (_, top) = taken.next().expect("a mask has at least its base");
    m.mask = top;
    m.refine = taken
        .map(|(op, mask)| Refinement {
            op: op.unwrap_or(MaskOp::Add),
            mask,
        })
        .collect();
    *selected = index_after_move(*selected, from, to, n);
    true
}

/// Move mask `from` into slot `to`, carrying its overlay flag and keeping `selected` on the mask it
/// was on. Returns whether anything changed.
///
/// ⚠⚠ `shown` is index-aligned with `masks` (`Session::mask_shown`) and moves by the same
/// permutation — or after a move every eye would belong to a different mask. It is padded to the
/// masks' length first, `Session::sync_mask_shown`'s rule, so a short list cannot misalign it.
pub fn move_mask(
    masks: &mut Vec<MaskedAdjustment>,
    shown: &mut Vec<bool>,
    selected: &mut Option<usize>,
    from: usize,
    to: usize,
) -> bool {
    let n = masks.len();
    let idx: Vec<usize> = (0..n).collect();
    if from >= n || moved(&idx, from, to) == idx {
        return false;
    }
    shown.resize(n, false);
    move_in_place(masks, from, to);
    move_in_place(shown, from, to);
    if let Some(s) = selected.as_mut() {
        *s = index_after_move(*s, from, to, n);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use edit_model::RadialGradient;

    /// A hard-edged disc at `(x, 0.5)` — the x is each shape's identity in these tests.
    fn disc(x: f32, r: f32) -> Mask {
        Mask::Radial(RadialGradient {
            center: [x, 0.5],
            radius: [r, r],
            feather: 0.0,
            invert: false,
        })
    }

    fn x_of(m: &Mask) -> f32 {
        match m {
            Mask::Radial(r) => r.center[0],
            other => panic!("expected a disc, got {other:?}"),
        }
    }

    fn composite(base: Mask, refine: &[(MaskOp, Mask)]) -> MaskedAdjustment {
        MaskedAdjustment {
            mask: base,
            refine: refine
                .iter()
                .map(|(op, mask)| Refinement {
                    op: *op,
                    mask: mask.clone(),
                })
                .collect(),
            ..Default::default()
        }
    }

    /// The fold `MaskStage` performs, over analytic shapes only.
    fn coverage(m: &MaskedAdjustment, u: f32, v: f32) -> f32 {
        let at = |s: &Mask| s.analytic_coverage(u, v, 1.0).expect("an analytic shape");
        m.refine
            .iter()
            .fold(at(&m.mask), |c, r| r.op.combine(c, at(&r.mask)))
    }

    fn max_difference(a: &MaskedAdjustment, b: &MaskedAdjustment) -> f32 {
        let mut worst = 0.0f32;
        for i in 0..=40 {
            for j in 0..=40 {
                let (u, v) = (i as f32 / 40.0, j as f32 / 40.0);
                worst = worst.max((coverage(a, u, v) - coverage(b, u, v)).abs());
            }
        }
        worst
    }

    /// Base A, minus B, plus C — B and C overlap, A does not reach them.
    fn a_minus_b_plus_c() -> MaskedAdjustment {
        composite(
            disc(0.25, 0.2),
            &[
                (MaskOp::Subtract, disc(0.5, 0.15)),
                (MaskOp::Add, disc(0.55, 0.15)),
            ],
        )
    }

    /// ⚠⚠ Proven to differ FIRST (standing lesson (c)): a feature that reorders a fold is worth
    /// nothing if the orders it reaches all look alike, and every assertion below would pass on a
    /// fold that ignored order.
    #[test]
    fn a_reorder_that_changes_the_fold_changes_the_coverage() {
        let before = a_minus_b_plus_c();
        let mut after = before.clone();
        let mut sel = 0;
        assert!(move_component(&mut after, &mut sel, 2, 1), "the move was refused");
        assert_eq!(after.refine[0].op, MaskOp::Add, "fixture: C now before B");
        let d = max_difference(&before, &after);
        assert!(d > 0.9, "(A − B) ∪ C and (A ∪ C) − B must differ, got {d}");
    }

    #[test]
    fn exchanging_the_base_with_an_add_keeps_the_coverage() {
        // [A, +C, −B] → [C, +A, −B]: the base is an Add onto nothing, so the exchange is exact.
        let before = composite(
            disc(0.25, 0.2),
            &[
                (MaskOp::Add, disc(0.55, 0.15)),
                (MaskOp::Subtract, disc(0.5, 0.15)),
            ],
        );
        let mut after = before.clone();
        let mut sel = 0;
        assert!(move_component(&mut after, &mut sel, 1, 0));
        assert_eq!(x_of(&after.mask), 0.55, "C is the base now");
        assert_eq!(after.refine[0].op, MaskOp::Add, "and A adds");
        assert_eq!(x_of(&after.refine[0].mask), 0.25);
        let d = max_difference(&before, &after);
        assert!(d < 1e-6, "the exchange changed the coverage by {d}");
    }

    /// Base A, then −B, +C, ∩D, +E: every op, and two Adds so more than one shape may go on top.
    fn mixed() -> MaskedAdjustment {
        composite(
            disc(0.1, 0.1),
            &[
                (MaskOp::Subtract, disc(0.2, 0.1)),
                (MaskOp::Add, disc(0.3, 0.1)),
                (MaskOp::Intersect, disc(0.4, 0.1)),
                (MaskOp::Add, disc(0.5, 0.1)),
            ],
        )
    }

    /// Each shape's op keyed by its identity; the base reads as `Add`, which is what it is.
    fn op_by_shape(m: &MaskedAdjustment) -> Vec<(u32, MaskOp)> {
        let mut out: Vec<(u32, MaskOp)> = std::iter::once((x_of(&m.mask), MaskOp::Add))
            .chain(m.refine.iter().map(|r| (x_of(&r.mask), r.op)))
            .map(|(x, op)| ((x * 100.0).round() as u32, op))
            .collect();
        out.sort_by_key(|(k, _)| *k);
        out
    }

    /// ⚠ Over EVERY `(from, to)`, because an off-by-one only shows at particular indices.
    #[test]
    fn every_move_keeps_every_shape_and_its_op_and_is_offered_only_when_the_top_adds() {
        let m = mixed();
        let n = 1 + m.refine.len();
        let ops = ops(&m);
        let mut offered = 0;
        let mut refused = 0;
        for from in 0..n {
            for to in 0..=n {
                let top = moved(&ops, from, to)[0];
                let want = matches!(top, None | Some(MaskOp::Add));
                assert_eq!(
                    component_drop_allowed(&m, from, to),
                    want,
                    "{from} → {to}"
                );
                let mut got = m.clone();
                let mut sel = 0;
                let changed = move_component(&mut got, &mut sel, from, to);
                if !want {
                    refused += 1;
                    assert!(!changed, "{from} → {to} was refused but changed the mask");
                    assert_eq!(got, m, "{from} → {to} was refused but changed the mask");
                    continue;
                }
                offered += 1;
                assert_eq!(got.refine.len(), m.refine.len(), "{from} → {to} lost a shape");
                assert_eq!(
                    op_by_shape(&got),
                    op_by_shape(&m),
                    "{from} → {to} changed an op"
                );
            }
        }
        // ⚠ Assert the sample (standing lesson (b)): a rule that refused everything, or nothing,
        // would pass the loop above.
        assert!(offered > 10 && refused > 5, "offered {offered}, refused {refused}");
    }

    #[test]
    fn a_subtract_or_intersect_is_never_moved_to_the_top() {
        let m = mixed();
        let mut sel = 0;
        for (from, to) in [(1, 0), (3, 0), (0, 2)] {
            // (0, 2) moves the base below −B, which would put −B on top.
            let mut got = m.clone();
            assert!(!move_component(&mut got, &mut sel, from, to), "{from} → {to}");
            assert_eq!(got, m);
        }
    }

    #[test]
    fn the_selected_shape_stays_selected() {
        // Selected: +C at 2.
        for (from, to, want) in [
            (2, 1, 1), // C itself moves up
            (1, 3, 1), // −B moves below C, so C shifts up
            (4, 1, 3), // +E moves above C, so C shifts down
            (4, 5, 2), // a move below C leaves it alone
        ] {
            let mut m = mixed();
            let mut sel = 2;
            move_component(&mut m, &mut sel, from, to);
            assert_eq!(sel, want, "{from} → {to}");
            let x = if sel == 0 {
                x_of(&m.mask)
            } else {
                x_of(&m.refine[sel - 1].mask)
            };
            assert_eq!(x, 0.3, "{from} → {to}: the selection left +C");
        }
    }

    #[test]
    fn a_drop_where_the_row_already_is_changes_nothing() {
        let m = mixed();
        for i in 0..=m.refine.len() {
            for to in [i, i + 1] {
                let mut got = m.clone();
                let mut sel = 0;
                assert!(!move_component(&mut got, &mut sel, i, to), "{i} → {to}");
                assert_eq!(got, m);
            }
        }
    }

    #[test]
    fn out_of_range_indices_change_nothing() {
        let m = mixed();
        let mut got = m.clone();
        let mut sel = 7;
        assert!(!move_component(&mut got, &mut sel, 99, 0));
        assert_eq!((got, sel), (m, 7));
        let mut masks = vec![mixed()];
        let mut shown = vec![true];
        let mut selected = Some(0);
        assert!(!move_mask(&mut masks, &mut shown, &mut selected, 5, 0));
        assert_eq!(selected, Some(0));
    }

    fn one_shape(x: f32) -> MaskedAdjustment {
        composite(disc(x, 0.1), &[])
    }

    #[test]
    fn a_mask_moves_with_its_eye_and_the_selection_follows() {
        let mut masks = vec![one_shape(0.1), one_shape(0.2), one_shape(0.3)];
        let mut shown = vec![true, false, false];
        let mut selected = Some(0);
        assert!(move_mask(&mut masks, &mut shown, &mut selected, 0, 3));
        let xs: Vec<f32> = masks.iter().map(|m| x_of(&m.mask)).collect();
        assert_eq!(xs, [0.2, 0.3, 0.1]);
        assert_eq!(shown, [false, false, true], "the eye stayed behind");
        assert_eq!(selected, Some(2), "the selection stayed behind");
    }

    #[test]
    fn another_mask_moving_across_the_selection_shifts_it() {
        let mut masks = vec![one_shape(0.1), one_shape(0.2), one_shape(0.3)];
        let mut shown = vec![false; 3];
        let mut selected = Some(1);
        assert!(move_mask(&mut masks, &mut shown, &mut selected, 2, 0));
        assert_eq!(selected, Some(2));
        assert_eq!(x_of(&masks[2].mask), 0.2);
    }

    #[test]
    fn a_short_eye_list_is_padded_before_it_moves() {
        let mut masks = vec![one_shape(0.1), one_shape(0.2)];
        let mut shown = vec![true];
        let mut selected = None;
        assert!(move_mask(&mut masks, &mut shown, &mut selected, 0, 2));
        assert_eq!(shown, [false, true]);
        assert_eq!(selected, None);
    }

    #[test]
    fn a_mask_dropped_where_it_is_changes_nothing() {
        let mut masks = vec![one_shape(0.1), one_shape(0.2)];
        let mut shown = vec![true, false];
        let mut selected = Some(0);
        assert!(!move_mask(&mut masks, &mut shown, &mut selected, 0, 1));
        assert_eq!(shown, [true, false]);
    }
}
