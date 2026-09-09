//! Boolean composition **between** masks: the three ops, the refinement that carries one, and
//! the single rule that folds them.
//!
//! ⚠⚠ **Two of the three rules are not new — they are the brush's own, lifted.** [`MaskOp::Add`]
//! is exactly `brush::raster`'s across-strokes paint step and [`MaskOp::Subtract`] is exactly its
//! `erase`; [`MaskOp::Intersect`] is the third member of that same product algebra. `min`/`max`
//! were rejected for precisely that reason: they agree with these rules at 0 and 1 and differ
//! everywhere in between, so choosing them would make subtract-*between*-masks disagree with
//! erase-*within*-a-brush at every partial coverage — a second rule wearing the first one's name,
//! which is the failure this project already paid for with the Whites slider and with
//! `hardness`-versus-`feather`.
//!
//! ⚠ The trade the product rules make, stated rather than discovered: they are **not
//! idempotent**. `0.5 ∩ 0.5` is `0.25` and `0.5 ∪ 0.5` is `0.75`, where `min`/`max` would leave
//! both at `0.5`. On a feathered gradient intersected with a luminance range that means the ramp
//! darkens where both are partial — the correct reading of *"both conditions hold"* under
//! independence, and the thing most likely to be reported as "the intersection is too weak". If
//! it is, that is a conversation about the algebra, **not a constant to tune**.

use serde::{Deserialize, Serialize};

use crate::Mask;

/// How a refinement composites against the coverage built so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MaskOp {
    Add,
    Subtract,
    Intersect,
}

impl MaskOp {
    /// Every op, in the order the panel offers them.
    ///
    /// ⚠ Beside the type so the UI cannot invent a second ordering — the rule `FlagSet::ALL` and
    /// `Tool::ALL` already follow, and what makes a fourth op unable to ship with no chip.
    pub const ALL: [MaskOp; 3] = [MaskOp::Add, MaskOp::Subtract, MaskOp::Intersect];

    /// The op's own word, for a component row.
    pub fn label(self) -> &'static str {
        match self {
            MaskOp::Add => "Add",
            MaskOp::Subtract => "Subtract",
            MaskOp::Intersect => "Intersect",
        }
    }

    /// The mode chip's wording, which takes a preposition.
    ///
    /// ⚠ **Not the bare op name.** All five mask-kind buttons already begin with "Add", so a chip
    /// labelled `Add` sits directly above five other things called Add and says nothing about
    /// which is which — the ambiguity that turned two adjacent sliders both called "Scale" into
    /// "Lens scale". The preposition also makes the two-click gesture read as one sentence:
    /// *Subtract from … a radial filter*.
    pub fn chip_label(self) -> &'static str {
        match self {
            MaskOp::Add => "Add to",
            MaskOp::Subtract => "Subtract from",
            MaskOp::Intersect => "Intersect with",
        }
    }

    /// Composite coverage `b` onto the running coverage `a`. Both in `0.0..=1.0`.
    ///
    /// ⚠ `Add` is `brush/raster.rs`'s paint step and `Subtract` is its `erase`, and since this
    /// milestone **the raster calls this function** — so the two cannot drift. That is the whole
    /// argument for the product algebra: erase-within-a-brush and subtract-between-masks are one
    /// rule rather than two that happen to agree at the endpoints.
    pub fn combine(self, a: f32, b: f32) -> f32 {
        match self {
            // Standard "over": adds a fraction of what is still uncovered, so repeated
            // application approaches full coverage without ever exceeding it.
            MaskOp::Add => a + (1.0 - a) * b,
            MaskOp::Subtract => a * (1.0 - b),
            MaskOp::Intersect => a * b,
        }
    }
}

/// One shape composited onto a mask's coverage.
///
/// ⚠ `op` is **required, never defaulted**. A refinement with no op has no defensible reading, and
/// guessing one silently changes the picture — so a malformed one fails the load, exactly as an
/// unknown [`Mask`] tag does.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Refinement {
    pub op: MaskOp,
    pub mask: Mask,
}

impl Refinement {
    /// The component row's name, e.g. `"Subtract Radial"`.
    ///
    /// ⚠ Built from [`MaskOp::label`] and [`Mask::label`] rather than written out, so a row can
    /// never name a kind the mask does not have.
    pub fn label(&self) -> String {
        format!("{} {}", self.op.label(), self.mask.label())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LuminanceRange, RadialGradient};

    #[test]
    fn the_endpoints_are_what_the_ops_promise() {
        assert_eq!(
            MaskOp::Add.combine(0.4, 0.0),
            0.4,
            "adding nothing changes nothing"
        );
        assert_eq!(
            MaskOp::Subtract.combine(0.4, 0.0),
            0.4,
            "subtracting nothing changes nothing"
        );
        assert_eq!(
            MaskOp::Intersect.combine(0.4, 0.0),
            0.0,
            "intersecting nothing leaves nothing"
        );
        assert_eq!(
            MaskOp::Add.combine(0.0, 0.7),
            0.7,
            "adding to nothing is the addend"
        );
        assert_eq!(
            MaskOp::Subtract.combine(0.7, 1.0),
            0.0,
            "subtracting all of it leaves none"
        );
        assert_eq!(
            MaskOp::Intersect.combine(0.7, 1.0),
            0.7,
            "intersecting everything is the identity"
        );
    }

    #[test]
    fn the_partial_case_is_the_product_algebra_not_min_max() {
        // ⚠⚠ The discriminating test, and the only one that is. `min`/`max` agree with these
        // rules at 0 and 1 — every assertion in `the_endpoints_are_what_the_ops_promise` passes
        // against them — and differ everywhere in between, so only a partial case can tell the
        // two algebras apart.
        assert!((MaskOp::Add.combine(0.5, 0.5) - 0.75).abs() < 1e-6);
        assert!((MaskOp::Intersect.combine(0.5, 0.5) - 0.25).abs() < 1e-6);
        assert!((MaskOp::Subtract.combine(0.5, 0.5) - 0.25).abs() < 1e-6);
    }

    #[test]
    fn every_op_stays_inside_the_unit_range() {
        let probes = [0.0, 0.01, 0.37, 0.5, 0.99, 1.0];
        for op in MaskOp::ALL {
            for a in probes {
                for b in probes {
                    let c = op.combine(a, b);
                    assert!((0.0..=1.0).contains(&c), "{op:?} {a} {b} -> {c}");
                }
            }
        }
    }

    #[test]
    fn the_fold_respects_list_order() {
        // ⚠ Order is observable, which is why the panel appends rather than inserting: an
        // add-after-intersect is not an intersect-after-add.
        let c = MaskOp::Add.combine(MaskOp::Intersect.combine(0.5, 0.5), 0.5);
        let d = MaskOp::Intersect.combine(MaskOp::Add.combine(0.5, 0.5), 0.5);
        assert!((c - 0.625).abs() < 1e-6, "add after intersect: {c}");
        assert!((d - 0.375).abs() < 1e-6, "intersect after add: {d}");
        assert!((c - d).abs() > 0.2, "the two orders must differ");
    }

    #[test]
    fn every_op_has_its_own_two_labels() {
        // ⚠ Over the whole vocabulary rather than one example: a guard that enumerates a list
        // stops guarding the moment the list grows, which is the sharpest finding of the
        // masked-grade milestone.
        let mut rows: Vec<&str> = MaskOp::ALL.iter().map(|o| o.label()).collect();
        rows.sort_unstable();
        rows.dedup();
        assert_eq!(rows.len(), MaskOp::ALL.len(), "two ops share a row label");

        let mut chips: Vec<&str> = MaskOp::ALL.iter().map(|o| o.chip_label()).collect();
        chips.sort_unstable();
        chips.dedup();
        assert_eq!(chips.len(), MaskOp::ALL.len(), "two ops share a chip label");

        // ⚠ The chip must not be the bare op name, or it collides with the five "Add …" buttons.
        for op in MaskOp::ALL {
            assert_ne!(
                op.chip_label(),
                op.label(),
                "{op:?}'s chip needs its preposition"
            );
        }
    }

    #[test]
    fn a_refinement_names_its_op_and_its_kind() {
        let r = Refinement {
            op: MaskOp::Subtract,
            mask: Mask::Radial(RadialGradient::default()),
        };
        assert_eq!(r.label(), "Subtract Radial");
    }

    #[test]
    fn a_refinement_round_trips_through_json() {
        let r = Refinement {
            op: MaskOp::Intersect,
            mask: Mask::Luminance(LuminanceRange::default()),
        };
        let s = serde_json::to_string(&r).unwrap();
        assert_eq!(serde_json::from_str::<Refinement>(&s).unwrap(), r);
    }

    #[test]
    fn a_refinement_without_an_op_is_refused() {
        // ⚠ Required, not defaulted: guessing an op silently changes the picture, and a
        // hand-edited sidecar is exactly where a missing key comes from.
        let json = r#"{"mask":{"Radial":{"center":[0.5,0.5],"radius":[0.25,0.25],"feather":50.0,"invert":false}}}"#;
        assert!(serde_json::from_str::<Refinement>(json).is_err());
    }
}
