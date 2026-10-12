//! What the masks panel's right-click menus and its "Invert mask" box do to the edits: change a
//! row's op, duplicate a mask, and flip a mask's inversion (M10.12a).
//!
//! ⚠ No egui. The menus and the checkbox live in `develop/panel/{masks,mask_components}.rs`;
//! everything they *decide* is here, where it can be tested without a window — `mask_order`'s rule.

use edit_model::{MaskOp, MaskedAdjustment};

/// Set row `j`'s op — `0` is the base, `j > 0` the `j-1`th refinement.
///
/// Returns whether anything changed: `false` for the base (it has no op — it is the Add every other
/// row folds onto, `mask_order`'s top-row rule), an out-of-range row, or the op it already has, so
/// the caller records an undo step only for a real change.
pub fn set_op(m: &mut MaskedAdjustment, j: usize, op: MaskOp) -> bool {
    let Some(r) = j.checked_sub(1).and_then(|k| m.refine.get_mut(k)) else {
        return false;
    };
    if r.op == op {
        return false;
    }
    r.op = op;
    true
}

/// Whether the panel draws "Invert mask" for `m`.
///
/// ⚠ For a composite, and for any mask whose flag is on — so a composite inverted and then cut back
/// to one shape never hides a live flag. A one-shape mask otherwise has only its shape's Invert,
/// which is the same coverage (the author's call, 2026-10-12).
pub fn whole_invert_drawn(m: &MaskedAdjustment) -> bool {
    !m.refine.is_empty() || m.invert
}

/// Flip the inversion the panel shows for `m` as a whole: the mask's own flag where "Invert mask"
/// is drawn, the shape's own Invert otherwise. Exact either way — on one shape they coincide.
pub fn flip_whole_invert(m: &mut MaskedAdjustment) {
    if whole_invert_drawn(m) {
        m.invert = !m.invert;
    } else {
        let f = m.mask.invert_mut();
        *f = !*f;
    }
}

/// Insert a copy of mask `i` directly below it, select the copy and show its overlay — what the
/// panel does for a mask it adds itself. With `invert`, the copy covers what the original leaves
/// out ([`flip_whole_invert`]). Returns whether anything changed (`false` for an `i` out of range).
///
/// ⚠⚠ `shown` is index-aligned with `masks` (`Session::mask_shown`) and is padded to the masks'
/// length **before** the insert, `move_mask`'s rule — it arrives short after an undo, a paste or a
/// snapshot, and inserting into a short list would hand every later mask its neighbour's eye.
pub fn duplicate(
    masks: &mut Vec<MaskedAdjustment>,
    shown: &mut Vec<bool>,
    selected: &mut Option<usize>,
    i: usize,
    invert: bool,
) -> bool {
    let Some(src) = masks.get(i) else {
        return false;
    };
    let mut copy = src.clone();
    if invert {
        flip_whole_invert(&mut copy);
    }
    shown.resize(masks.len(), false);
    masks.insert(i + 1, copy);
    shown.insert(i + 1, true);
    *selected = Some(i + 1);
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use edit_model::{Mask, RadialGradient, Refinement};

    fn disc(x: f32) -> Mask {
        Mask::Radial(RadialGradient {
            center: [x, 0.5],
            radius: [0.1, 0.1],
            feather: 0.0,
            invert: false,
        })
    }

    fn composite() -> MaskedAdjustment {
        MaskedAdjustment {
            mask: disc(0.1),
            refine: vec![
                Refinement {
                    op: MaskOp::Subtract,
                    mask: disc(0.2),
                },
                Refinement {
                    op: MaskOp::Add,
                    mask: disc(0.3),
                },
            ],
            ..Default::default()
        }
    }

    #[test]
    fn set_op_changes_one_refinement_and_reports_it() {
        let mut m = composite();
        assert!(set_op(&mut m, 2, MaskOp::Intersect));
        assert_eq!(m.refine[1].op, MaskOp::Intersect);
        assert_eq!(m.refine[0].op, MaskOp::Subtract, "only row 2 changes");
    }

    #[test]
    fn set_op_refuses_the_base_a_missing_row_and_the_same_op() {
        let mut m = composite();
        let before = m.clone();
        assert!(!set_op(&mut m, 0, MaskOp::Intersect), "the base has no op");
        assert!(!set_op(&mut m, 3, MaskOp::Intersect), "no row 3");
        assert!(!set_op(&mut m, 1, MaskOp::Subtract), "already Subtract");
        assert_eq!(m, before);
    }

    #[test]
    fn duplicate_inserts_the_copy_below_selects_it_and_shows_it() {
        let mut second = composite();
        second.mask = disc(0.9);
        second.disabled = true;
        let mut masks = vec![composite(), second];
        let mut shown = vec![false, false];
        let mut sel = Some(0);
        assert!(duplicate(&mut masks, &mut shown, &mut sel, 1, false));
        assert_eq!(masks.len(), 3);
        assert_eq!(
            masks[2], masks[1],
            "a copy keeps everything, `disabled` included"
        );
        assert_eq!(shown, vec![false, false, true]);
        assert_eq!(sel, Some(2));
    }

    /// Review Focus 3: `shown` arrives short after an undo, a paste or a snapshot.
    #[test]
    fn duplicate_pads_a_short_eye_list_before_inserting() {
        let mut masks = vec![composite(), composite(), composite()];
        let mut shown = vec![true];
        let mut sel = None;
        assert!(duplicate(&mut masks, &mut shown, &mut sel, 0, false));
        assert_eq!(
            shown,
            vec![true, true, false, false],
            "mask 0 keeps its eye, the copy shows"
        );
        assert!(
            !duplicate(&mut masks, &mut shown, &mut sel, 9, false),
            "out of range"
        );
        assert_eq!(masks.len(), 4);
    }

    #[test]
    fn duplicate_and_invert_flips_the_flag_on_a_composite_and_the_shape_on_one_shape() {
        let mut masks = vec![composite()];
        let (mut shown, mut sel) = (Vec::new(), None);
        duplicate(&mut masks, &mut shown, &mut sel, 0, true);
        assert!(masks[1].invert, "a composite's copy flips its flag");
        assert!(!*masks[1].mask.invert_mut(), "and not its base shape");

        let mut one = vec![MaskedAdjustment {
            mask: disc(0.5),
            ..Default::default()
        }];
        duplicate(&mut one, &mut shown, &mut sel, 0, true);
        assert!(!one[1].invert, "a one-shape mask keeps its flag off");
        assert!(
            *one[1].mask.invert_mut(),
            "and flips its shape's own Invert"
        );
    }

    /// Review Focus 4.
    #[test]
    fn a_live_flag_stays_drawn_after_the_composite_shrinks_to_one_shape() {
        let mut m = composite();
        assert!(whole_invert_drawn(&m));
        m.invert = true;
        m.refine.clear();
        assert!(
            whole_invert_drawn(&m),
            "the flag is on, so its box must stay"
        );
        flip_whole_invert(&mut m);
        assert!(!m.invert, "it flips the flag");
        assert!(!*m.mask.invert_mut(), "not the shape");
        assert!(
            !whole_invert_drawn(&m),
            "once off, a one-shape mask has only its shape's Invert"
        );
    }
}
