//! One plain sentence for every Develop slider, shown when the pointer rests on its name.
//!
//! ⚠⚠ **Keyed on the slider's scope as well as its label** (2026-09-29, the author's call). A label
//! alone could not say what "Detail" does: noise reduction and sharpening both have one, and the
//! old sentence described both ("hovering over Detail in Noise reduction also tells me about the
//! Detail in Sharpening"). Each block now sets its [`Scope`] on `Edited` before drawing, and a
//! label that means different things in different blocks lives **only** in the scoped tables —
//! never in [`GLOBAL`] — so a block that forgets its scope shows no help, and the panel tests
//! (which assert that no slider went without help) fail instead of shipping the wrong sentence.
//!
//! The house style, checked by `every_sentence_is_one_short_sentence`: one sentence that says what
//! the control does, starting with the verb, no full stop, no reset hint (double-click to reset is
//! the same for every slider and is not repeated on each), no warnings, no history.

/// Where a slider sits, when its label alone does not say what it does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Scope {
    /// Labels that mean one thing everywhere.
    #[default]
    Global,
    /// The tone curve's Parametric sliders.
    Parametric,
    /// The HSL bands, on one of the three tabs.
    Hsl(HslTab),
    /// Tint by tone (color grading).
    TintByTone,
    /// A creative look's strength.
    Look,
    NoiseReduction,
    Sharpening,
    Vignette,
    /// The Lens section's own scale.
    Lens,
    /// A brush mask's tool settings.
    Brush,
    /// A radial mask's edge.
    Radial,
    /// A luminance-range mask.
    LuminanceRange,
    /// A colour-range mask.
    ColourRange,
    /// A heal or clone patch.
    Patch,
    /// A red-eye or pet-eye correction.
    Eye,
}

/// The HSL bands' three tabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HslTab {
    Hue,
    Saturation,
    Luminance,
}

/// Labels that mean the same thing wherever they are drawn.
const GLOBAL: &[(&str, &str)] = &[
    // White balance
    ("Temp", "Shifts the colour balance warmer or cooler, in kelvin"),
    ("Tint", "Shifts the colour balance toward green or magenta"),
    // Tone
    ("Exposure", "Brightens or darkens the picture, in stops"),
    ("Contrast", "Pushes the bright and dark tones apart, or pulls them together"),
    ("Highlights", "Darkens or brightens the brightest areas"),
    ("Shadows", "Brightens or darkens the darkest areas"),
    ("Whites", "Sets how bright the brightest tones get"),
    ("Blacks", "Sets how dark the darkest tones get"),
    // Presence
    ("Texture", "Sharpens or smooths fine surface detail such as skin, grain and fabric"),
    ("Clarity", "Adds or removes contrast in mid-sized detail"),
    ("Dehaze", "Removes haze, or adds it when negative"),
    // Colour
    ("Vibrance", "Strengthens muted colours more than vivid ones, sparing skin tones"),
    ("Saturation", "Makes every colour stronger or weaker by the same amount"),
    // Lens and defringe
    ("Lens scale", "Zooms in to hide the edges that lens correction pulls in"),
    ("Purple amount", "Removes purple fringes along high-contrast edges"),
    ("Green amount", "Removes green fringes along high-contrast edges"),
    ("Min chroma", "Ignores colours too faint to be a fringe"),
    (
        "Defringe",
        "Removes colour fringes inside this mask, or shields it from the global defringe when negative",
    ),
    // Geometry
    ("Angle", "Rotates the photo to straighten it, in degrees"),
    ("Vertical", "Corrects verticals that converge, as when a building leans back"),
    ("Horizontal", "Corrects a subject that is turned to the left or right"),
    ("Aspect ⬍ ⬌", "Stretches the photo taller or wider to undo a keystone's squashing"),
    ("Scale", "Shrinks the corrected photo to fit the frame, or enlarges it to fill the corners"),
    // Masks
    ("Refine", "Grows or shrinks the edge of the selection"),
];

const PARAMETRIC: &[(&str, &str)] = &[
    (
        "Highlights",
        "Raises or lowers the brightest part of the curve",
    ),
    ("Lights", "Raises or lowers the upper midtones of the curve"),
    ("Darks", "Raises or lowers the lower midtones of the curve"),
    ("Shadows", "Raises or lowers the darkest part of the curve"),
];

const TINT_BY_TONE: &[(&str, &str)] = &[
    ("Hue", "Sets the colour of this range's tint"),
    ("Strength", "Sets how strongly this range is tinted"),
    ("Luminance", "Brightens or darkens this range"),
    (
        "Blending",
        "Sets how far the shadow and highlight tints overlap",
    ),
    (
        "Balance",
        "Moves the boundary between the shadow and highlight tints",
    ),
];

const LOOK: &[(&str, &str)] = &[("Strength", "Sets how much of the look is applied")];

const NOISE_REDUCTION: &[(&str, &str)] = &[
    ("Luminance", "Smooths the grain in brightness"),
    (
        "Detail",
        "Keeps fine texture from being smoothed away with the grain",
    ),
    ("Color", "Removes coloured speckles from the noise"),
    ("Smoothness", "Blends away larger blotches of colour noise"),
];

const SHARPENING: &[(&str, &str)] = &[
    ("Amount", "Sets how strongly edges are sharpened"),
    (
        "Radius (px)",
        "Sets how wide each sharpened edge is, in pixels",
    ),
    ("Detail", "Sharpens fine texture as well as edges"),
    (
        "Masking",
        "Limits sharpening to edges, sparing smooth areas like sky and skin",
    ),
];

const VIGNETTE: &[(&str, &str)] = &[
    (
        "Amount",
        "Darkens the corners, or lightens them when positive",
    ),
    (
        "Midpoint",
        "Sets how far the vignette reaches toward the centre",
    ),
    (
        "Roundness",
        "Makes the vignette rounder or more rectangular",
    ),
    ("Feather", "Softens the vignette's edge"),
];

const LENS: &[(&str, &str)] = &[(
    "Scale",
    "Zooms in to hide the edges that lens correction pulls in",
)];

const BRUSH: &[(&str, &str)] = &[
    ("Size", "Sets the brush diameter, in pixels of the photo"),
    ("Hardness", "Makes the brush's edge crisp or soft"),
    (
        "Flow",
        "Sets how much each stroke adds, so low values build up gradually",
    ),
    (
        "Tolerance",
        "Sets how different a colour may be from the one under the brush and still be painted",
    ),
];

const RADIAL: &[(&str, &str)] = &[("Feather", "Softens the edge of the ellipse")];

const LUMINANCE_RANGE: &[(&str, &str)] = &[
    (
        "Range low",
        "Sets the darkest tone the mask selects, from 0 black to 100 white",
    ),
    (
        "Range high",
        "Sets the brightest tone the mask selects, from 0 black to 100 white",
    ),
    ("Feather", "Softens the ends of the tonal range"),
];

const COLOUR_RANGE: &[(&str, &str)] = &[
    (
        "Hue low",
        "Sets where the selected band of colours starts, in degrees around the wheel",
    ),
    (
        "Hue high",
        "Sets where the selected band of colours ends, in degrees around the wheel",
    ),
    ("Feather", "Softens the ends of the colour band"),
    (
        "Min chroma",
        "Leaves out colours too faint to have a reliable hue",
    ),
];

const PATCH: &[(&str, &str)] = &[
    ("Size", "Sets the patch's radius, in pixels of the photo"),
    ("Feather", "Softens the patch's edge"),
];

const EYE: &[(&str, &str)] = &[
    (
        "Pupil Size",
        "Sets how far the correction reaches into the eye",
    ),
    ("Darken", "Sets how dark the corrected pupil becomes"),
];

fn table(scope: Scope) -> &'static [(&'static str, &'static str)] {
    match scope {
        Scope::Global | Scope::Hsl(_) => &[],
        Scope::Parametric => PARAMETRIC,
        Scope::TintByTone => TINT_BY_TONE,
        Scope::Look => LOOK,
        Scope::NoiseReduction => NOISE_REDUCTION,
        Scope::Sharpening => SHARPENING,
        Scope::Vignette => VIGNETTE,
        Scope::Lens => LENS,
        Scope::Brush => BRUSH,
        Scope::Radial => RADIAL,
        Scope::LuminanceRange => LUMINANCE_RANGE,
        Scope::ColourRange => COLOUR_RANGE,
        Scope::Patch => PATCH,
        Scope::Eye => EYE,
    }
}

fn find(t: &[(&str, &'static str)], label: &str) -> Option<&'static str> {
    t.iter().find(|(l, _)| *l == label).map(|(_, s)| *s)
}

/// The HSL band names, as the panel labels them.
const BANDS: [&str; 8] = [
    "Red", "Orange", "Yellow", "Green", "Aqua", "Blue", "Purple", "Magenta",
];

/// The help sentence for a slider, if it has one: the scope's own table first, then [`GLOBAL`].
pub fn text(scope: Scope, label: &str) -> Option<String> {
    if let Scope::Hsl(tab) = scope {
        if BANDS.contains(&label) {
            // The band as a plural — "the reds", "the aquas" — which is how a colour range is said.
            let band = format!("{}s", label.to_lowercase());
            return Some(match tab {
                HslTab::Hue => format!("Shifts the {band} toward the neighbouring hues"),
                HslTab::Saturation => format!("Makes the {band} stronger or weaker"),
                HslTab::Luminance => format!("Makes the {band} brighter or darker"),
            });
        }
    }
    find(table(scope), label)
        .or_else(|| find(GLOBAL, label))
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCOPES: [Scope; 16] = [
        Scope::Global,
        Scope::Parametric,
        Scope::Hsl(HslTab::Hue),
        Scope::Hsl(HslTab::Saturation),
        Scope::Hsl(HslTab::Luminance),
        Scope::TintByTone,
        Scope::Look,
        Scope::NoiseReduction,
        Scope::Sharpening,
        Scope::Vignette,
        Scope::Lens,
        Scope::Brush,
        Scope::Radial,
        Scope::LuminanceRange,
        Scope::ColourRange,
        Scope::Patch,
    ];

    /// Every sentence the table can produce, with where it came from.
    fn all() -> Vec<(String, String)> {
        let mut out = Vec::new();
        for scope in SCOPES.iter().copied().chain([Scope::Eye]) {
            for (label, _) in table(scope).iter().chain(GLOBAL) {
                out.push((
                    format!("{scope:?}/{label}"),
                    text(scope, label).expect("listed"),
                ));
            }
            for band in BANDS {
                if let Some(t) = text(scope, band) {
                    out.push((format!("{scope:?}/{band}"), t));
                }
            }
        }
        out
    }

    /// ⚠⚠ The author's rule (2026-09-29): "it should simply state what it does exactly in one,
    /// clear sentence", and no "Double-click to reset" on each. Checked over every sentence the
    /// table can produce, so a new entry cannot drift back into paragraphs.
    #[test]
    fn every_sentence_is_one_short_sentence() {
        let all = all();
        assert!(all.len() > 60, "sample: {}", all.len());
        for (at, t) in &all {
            assert!(!t.ends_with('.'), "{at}: no full stop — {t:?}");
            assert!(
                !t.contains(". ") && !t.contains("; "),
                "{at}: one sentence — {t:?}"
            );
            assert!(
                t.chars().count() <= 100,
                "{at}: short — {} chars",
                t.chars().count()
            );
            assert!(!t.to_lowercase().contains("double-click"), "{at}: {t:?}");
            assert!(!t.contains('⚠'), "{at}: {t:?}");
            assert!(t.starts_with(char::is_uppercase), "{at}: {t:?}");
        }
    }

    /// ⚠ The reported case, both halves: each "Detail" names its own block and not the other.
    #[test]
    fn a_shared_label_says_what_it_does_where_it_is() {
        let nr = text(Scope::NoiseReduction, "Detail").unwrap();
        let sh = text(Scope::Sharpening, "Detail").unwrap();
        assert_ne!(nr, sh);
        assert!(!nr.to_lowercase().contains("sharpen"), "{nr}");
        assert!(sh.to_lowercase().contains("sharpen"), "{sh}");
        let hue = text(Scope::Hsl(HslTab::Hue), "Red").unwrap();
        let lum = text(Scope::Hsl(HslTab::Luminance), "Red").unwrap();
        assert!(
            hue.contains("reds") && lum.contains("brighter"),
            "{hue} / {lum}"
        );
    }

    /// ⚠⚠ A label that means different things must have **no** global sentence, so a block that
    /// forgets to set its scope gets nothing — and the panel tests fail — rather than a sentence
    /// about somewhere else. Only the labels a scope overrides on purpose may appear in both.
    #[test]
    fn an_ambiguous_label_needs_its_scope() {
        for label in [
            "Detail",
            "Amount",
            "Luminance",
            "Color",
            "Feather",
            "Size",
            "Hue",
            "Strength",
        ] {
            assert_eq!(
                text(Scope::Global, label),
                None,
                "{label} has a global sentence"
            );
        }
    }

    #[test]
    fn an_unknown_label_has_none() {
        assert_eq!(text(Scope::Global, "Flux capacitor"), None);
    }
}
