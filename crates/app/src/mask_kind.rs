//! The five kinds of mask a user can add, with their menu labels and templates.
//!
//! ⚠ A lib module, not a panel detail: the Add mask menu and the three Refine menus all list
//! these, and the canvas may one day too. One list, one set of defaults.

use edit_model::Mask;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaskKind {
    Gradient,
    Radial,
    Brush,
    Luminance,
    Colour,
}

impl MaskKind {
    /// Menu order: the positional shapes first, then the two ranges.
    pub const ALL: [MaskKind; 5] = [
        MaskKind::Gradient,
        MaskKind::Radial,
        MaskKind::Brush,
        MaskKind::Luminance,
        MaskKind::Colour,
    ];

    /// What the menu item says.
    ///
    /// ⚠ The two ranges say "range": a menu entry reading just "Luminance" beside "Brush" does
    /// not say it selects *by* luminance rather than painting it.
    pub fn menu_label(self) -> &'static str {
        match self {
            MaskKind::Gradient => "Gradient",
            MaskKind::Radial => "Radial",
            MaskKind::Brush => "Brush",
            MaskKind::Luminance => "Luminance range",
            MaskKind::Colour => "Colour range",
        }
    }

    /// The kind of an existing mask.
    pub fn of(mask: &Mask) -> MaskKind {
        match mask {
            Mask::Linear(_) => MaskKind::Gradient,
            Mask::Radial(_) => MaskKind::Radial,
            Mask::Brush(_) => MaskKind::Brush,
            Mask::Luminance(_) => MaskKind::Luminance,
            Mask::Colour(_) => MaskKind::Colour,
        }
    }

    /// A fresh mask of this kind, with a visible default.
    pub fn template(self) -> Mask {
        match self {
            // A visible default: top to bottom across the middle third, so the handles land on
            // screen and the ramp is immediately obvious.
            MaskKind::Gradient => Mask::Linear(edit_model::LinearGradient {
                from: [0.5, 0.33],
                to: [0.5, 0.67],
                invert: false,
            }),
            // ⚠ The hand-written `Default`: centred, a quarter of each axis, feather 50. Both
            // radii being fractions of their own axis makes the default ellipse *similar to the
            // frame*, which is usually the shape being reached for.
            MaskKind::Radial => Mask::Radial(edit_model::RadialGradient::default()),
            // ⚠ `Brush::default()` has no strokes, so the mask is inert until painted — which is
            // exactly why the overlay defaults on.
            MaskKind::Brush => Mask::Brush(edit_model::Brush::default()),
            // ⚠ The hand-written `Default`: the darker half with a soft shoulder. Visible
            // immediately and one click of Invert from the highlights, where a full-range
            // default would be indistinguishable from a global adjustment.
            MaskKind::Luminance => Mask::Luminance(edit_model::LuminanceRange::default()),
            // ⚠ The hand-written `Default`: the warm half of the wheel with a soft shoulder and
            // the chroma gate already at HSL's measured value, so the mask is doing something
            // recognisable before any slider is touched.
            MaskKind::Colour => Mask::Colour(edit_model::ColourRange::default()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_templates_its_own_variant() {
        // ⚠ Stated as a round trip through `of`, so a template that built the wrong variant —
        // a copy-paste slip in a five-arm match — cannot pass. No wildcard in `of`: a sixth
        // variant forces a decision here.
        for k in MaskKind::ALL {
            assert_eq!(MaskKind::of(&k.template()), k, "{k:?}");
        }
    }

    #[test]
    fn menu_labels_are_distinct() {
        let mut seen: Vec<&str> = MaskKind::ALL.iter().map(|k| k.menu_label()).collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), MaskKind::ALL.len(), "two kinds share a label");
    }
}
