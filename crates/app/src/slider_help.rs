//! One plain sentence for every Develop slider, shown when the pointer rests on its name.
//!
//! ⚠ Keyed on the **label** alone, because that is all `Edited::slider` knows. A handful of
//! labels serve two sections ("Amount", "Detail", "Luminance"); their sentences name both
//! meanings rather than guessing. The panel tests assert that every slider they draw has an
//! entry here, so a new slider without one fails the gate instead of shipping mute.
//!
//! ⚠ Every sentence ends with the period and nothing else: `Edited::slider` appends the reset
//! hint, so it is written once, not fifty times.

/// The help sentence for a slider label, if it has one.
pub fn text(label: &str) -> Option<&'static str> {
    Some(match label {
        // White balance
        "Temp" => "Warmer or cooler overall, in kelvin — the colour of the light the photo was taken in.",
        "Tint" => "Green to magenta, the other axis of white balance.",
        // Tone
        "Exposure" => "Overall brightness, in stops: +1 is twice as bright.",
        "Contrast" => "Spreads the tones apart (bright brighter, dark darker) or pulls them together.",
        "Highlights" => "Brings the bright parts down or up without touching the rest.",
        "Shadows" => "Lifts the dark parts or deepens them, leaving the bright parts alone.",
        "Whites" => "Where pure white sits: down to recover a blown sky, up for more punch.",
        "Blacks" => "Where pure black sits: up for a lifted, softer look, down for depth.",
        // Presence
        "Texture" => "Fine detail — pores, grain, fabric — sharper or smoother, without haloes.",
        "Clarity" => "Punch in the mid-sized detail; negative gives a soft, dreamy look.",
        "Dehaze" => "Cuts through haze and mist, or adds it. Also raises contrast and saturation.",
        // Colour
        "Vibrance" => "Boosts the muted colours most and protects skin tones — the safer saturation.",
        "Saturation" => "All colours stronger or weaker alike; −100 is black and white.",
        // Tint by tone
        "Hue" => "Which colour the tint leans toward, around the wheel.",
        "Strength" => "How much of the tint or the look is applied.",
        "Blending" => "How far the shadow and highlight tints overlap in the midtones.",
        "Balance" => "Shifts the crossover between the shadow tint and the highlight tint.",
        // HSL bands
        "Red" | "Orange" | "Yellow" | "Green" | "Aqua" | "Blue" | "Purple" | "Magenta" => {
            "Just this colour: its hue, saturation or brightness, whichever tab is open."
        }
        // Parametric curve
        "Lights" => "The brighter midtones, without moving the extremes.",
        "Darks" => "The darker midtones, without moving the extremes.",
        // Detail — noise reduction and sharpening share three labels
        "Luminance" => "In noise reduction, how much grain is smoothed; under Tint by tone, how bright that range is.",
        "Detail" => "How much fine texture survives: in noise reduction, texture kept; in sharpening, texture sharpened.",
        "Color" => "Smooths the coloured speckle in noise, without blurring the picture.",
        "Smoothness" => "How gently colour noise is blended; higher hides blotches.",
        "Amount" => "How strong it is: the sharpening in Detail, the darkened corners in Vignette.",
        "Radius (px)" => "How wide a halo sharpening may draw; small for fine detail, larger for soft photos.",
        "Masking" => "Sharpens edges only; higher leaves smooth areas such as sky and skin alone.",
        // Lens and defringe
        "Lens scale" => "Zooms in slightly to hide the edges lens correction pulls in.",
        "Purple amount" => "How much purple colour fringing along dark edges is removed.",
        "Green amount" => "How much green colour fringing along dark edges is removed.",
        "Defringe" => "Negative protects this area from the Lens panel's defringe; positive removes purple and green fringes here with its hue ranges.",
        "Min chroma" => "How saturated a colour must be before defringing treats it as a fringe.",
        // Vignette
        "Midpoint" => "How far from the corners the darkening starts.",
        "Roundness" => "Rounder or squarer corners of the vignette.",
        "Feather" => "How soft the edge is: of the vignette, or of a retouch patch.",
        // Geometry
        "Angle" => "Rotates to straighten a tilted horizon, in degrees. Editing it by hand clears any perspective guides.",
        "Vertical" => "Keystone about the horizontal axis — for a subject leaning back or forward.",
        "Horizontal" => "Keystone about the vertical axis — for a subject turned left or right.",
        "Aspect ⬍ ⬌" => "Stretches the frame: negative taller ⬍, positive wider ⬌. Counteracts the squashing a keystone correction causes.",
        "Scale" => "−100 fits the whole corrected image inside the frame; 0 leaves it raw, which clips whatever the correction pushes off the edges; +100 zooms in until no black corners remain.",
        // Masks
        "Refine" => "Grows (+) or shrinks (−) the selection's edge. The network's answer is kept, so this is instant.",
        "Size" => "How big the brush or the patch is, in pixels of the photo.",
        "Flow" => "How much each brush stroke adds; low builds up gradually.",
        "Hardness" => "A crisp or a soft brush edge.",
        "Tolerance" => "How different a colour may be and still count as the one you picked.",
        "Range low" => "The darkest brightness the mask includes.",
        "Range high" => "The brightest brightness the mask includes.",
        "Hue low" => "The start of the colour range the mask includes.",
        "Hue high" => "The end of the colour range the mask includes.",
        // Retouch
        "Darken" => "How dark the corrected pupil becomes.",
        "Pupil Size" => "How large the corrected pupil is drawn.",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The labels the panels that have no rendering harness use; the harnessed panels assert
    /// coverage of what they actually draw.
    #[test]
    fn the_unharnessed_labels_have_help() {
        for label in [
            "Refine",
            "Size",
            "Flow",
            "Hardness",
            "Tolerance",
            "Range low",
            "Range high",
            "Hue low",
            "Hue high",
            "Min chroma",
            "Darken",
            "Pupil Size",
            "Purple amount",
            "Green amount",
            "Defringe",
            "Lens scale",
            "Lights",
            "Darks",
        ] {
            let t = text(label).unwrap_or_else(|| panic!("{label} has no help"));
            assert!(t.ends_with('.'), "{label}: {t:?} should end with a period");
            assert!(
                !t.contains("Double-click"),
                "{label}: the reset hint is appended once, by the slider"
            );
        }
    }

    #[test]
    fn an_unknown_label_has_none() {
        assert_eq!(text("Flux capacitor"), None);
    }
}
