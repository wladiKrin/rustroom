//! The seven kinds of mask a user can add, with their menu labels and templates.
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
    /// The photo's subject, found by a network (M9.13).
    Subject,
    /// The region under a click, found by a network (M9.13).
    Select,
}

impl MaskKind {
    /// Menu order: the positional shapes first, then the two ranges, then the two AI masks.
    pub const ALL: [MaskKind; 7] = [
        MaskKind::Gradient,
        MaskKind::Radial,
        MaskKind::Brush,
        MaskKind::Luminance,
        MaskKind::Colour,
        MaskKind::Subject,
        MaskKind::Select,
    ];

    /// Whether this kind is found by a network — it needs a model downloaded and a moment to
    /// compute.
    pub fn is_ai(self) -> bool {
        matches!(self, MaskKind::Subject | MaskKind::Select)
    }

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
            MaskKind::Subject => "Subject",
            MaskKind::Select => "Select by click",
        }
    }

    /// What the kind's button says on hover: the name and what the shape does on the image.
    ///
    /// ⚠ These carried the panel's hints until 2026-09-19 (user's call: hover text, not lines
    /// of small print under the controls).
    pub fn hover(self) -> &'static str {
        match self {
            MaskKind::Gradient => "Adds a mask that fades across a straight line",
            MaskKind::Radial => "Adds an elliptical mask",
            MaskKind::Brush => "Adds a mask you paint with a brush",
            MaskKind::Luminance => "Adds a mask that selects tones by brightness",
            MaskKind::Colour => "Adds a mask that selects colours by hue",
            MaskKind::Subject => "Adds a mask of the main subject, found automatically",
            MaskKind::Select => "Adds a mask of whatever you click, found automatically",
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
            Mask::Ai(edit_model::AiMask {
                prompt: edit_model::AiPrompt::Subject,
                ..
            }) => MaskKind::Subject,
            Mask::Ai(_) => MaskKind::Select,
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
            MaskKind::Subject => Mask::Ai(edit_model::AiMask::default()),
            // ⚠ Aimed at the centre until the first click on the canvas re-aims it — a mask that
            // exists at once, like every other kind, rather than a mode waiting for input.
            MaskKind::Select => Mask::Ai(edit_model::AiMask {
                prompt: edit_model::AiPrompt::Click { point: [0.5, 0.5] },
                ..Default::default()
            }),
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
