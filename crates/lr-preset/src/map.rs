//! A preset's Lightroom settings onto Rustroom's edits: same name and range where Lightroom's meaning
//! matches, converted where only the scale differs, and **named** where there is no counterpart.

use crate::{Settings, Value};
use edit_model::{EditGroups, Edits, LensCorrection, HSL_BAND_NAMES};

/// A preset mapped: the edits, the groups it carries, and what it sets that Rustroom cannot apply.
#[derive(Debug, Clone, PartialEq)]
pub struct Mapped {
    pub edits: Edits,
    pub groups: EditGroups,
    pub left_out: Vec<&'static str>,
}

/// The settings that make a preset carry each group. ⚠ Lightroom writes **every** setting of a group
/// it includes — zeros too — so a group is carried exactly when one of its settings is present.
const WB: &[&str] = &[
    "WhiteBalance",
    "Temperature",
    "Tint",
    "IncrementalTemperature",
    "IncrementalTint",
];
const TONE: &[&str] = &[
    "Exposure2012",
    "Contrast2012",
    "Highlights2012",
    "Shadows2012",
    "Whites2012",
    "Blacks2012",
    "Texture",
    "Clarity2012",
    "Dehaze",
    "ToneCurvePV2012",
    "ToneCurvePV2012Red",
    "ToneCurvePV2012Green",
    "ToneCurvePV2012Blue",
    "ParametricShadows",
    "ParametricDarks",
    "ParametricLights",
    "ParametricHighlights",
    "ParametricShadowSplit",
    "ParametricMidtoneSplit",
    "ParametricHighlightSplit",
];
const COLOR: &[&str] = &["Vibrance", "Saturation", "ConvertToGrayscale"];
const COLOR_PREFIXES: &[&str] = &[
    "HueAdjustment",
    "SaturationAdjustment",
    "LuminanceAdjustment",
    "ColorGrade",
    "SplitToning",
];
const DETAIL: &[&str] = &[
    "Sharpness",
    "SharpenRadius",
    "SharpenDetail",
    "SharpenEdgeMasking",
    "LuminanceSmoothing",
    "LuminanceNoiseReductionDetail",
    "ColorNoiseReduction",
    "ColorNoiseReductionSmoothness",
];
/// Lightroom's Effects: the post-crop vignette and, since M10.11b, grain.
const VIGNETTE: &[&str] = &[
    "PostCropVignetteAmount",
    "PostCropVignetteMidpoint",
    "PostCropVignetteRoundness",
    "PostCropVignetteFeather",
    "GrainAmount",
    "GrainSize",
    "GrainFrequency",
];
const LENS: &[&str] = &[
    "LensProfileEnable",
    "AutoLateralCA",
    "DefringePurpleAmount",
    "DefringeGreenAmount",
];

fn num(s: &Settings, k: &str) -> Option<f32> {
    s.get(k).and_then(Value::num).map(|v| v as f32)
}

fn set(field: &mut f32, s: &Settings, k: &str) {
    if let Some(v) = num(s, k) {
        *field = v;
    }
}

fn truthy(s: &Settings, k: &str) -> bool {
    s.get(k).is_some_and(Value::as_bool)
}

fn nonzero(s: &Settings, k: &str) -> bool {
    num(s, k).is_some_and(|v| v != 0.0)
}

/// A structure (a mask, a spot) that is there and not empty.
fn present(s: &Settings, k: &str) -> bool {
    match s.get(k) {
        Some(Value::List(v)) => !v.is_empty(),
        Some(v) => v.as_bool(),
        None => false,
    }
}

fn any(s: &Settings, keys: &[&str]) -> bool {
    keys.iter().any(|k| s.contains_key(*k))
}

/// A Lightroom curve — `x, y` pairs on 0–255 — as Rustroom's points on 0–1; the identity as none.
fn points(v: &Value) -> Vec<[f32; 2]> {
    let n = v.numbers();
    let p: Vec<[f32; 2]> = n
        .as_chunks::<2>()
        .0
        .iter()
        .map(|[x, y]| [(x / 255.0) as f32, (y / 255.0) as f32])
        .collect();
    if p == [[0.0, 0.0], [1.0, 1.0]] {
        Vec::new()
    } else {
        p
    }
}

/// Map a preset's settings.
pub fn to_edits(s: &Settings) -> Mapped {
    let mut e = Edits::default();
    let has_prefix = |p: &str| s.keys().any(|k| k.starts_with(p));
    let groups = EditGroups {
        white_balance: any(s, WB),
        tone: any(s, TONE),
        color: any(s, COLOR) || COLOR_PREFIXES.iter().any(|p| has_prefix(p)),
        detail: any(s, DETAIL),
        lens: any(s, LENS),
        vignette: any(s, VIGNETTE),
        ..EditGroups::NONE
    };
    let mut left_out: Vec<&'static str> = Vec::new();

    // White balance: a Kelvin target, or none at all for "As Shot".
    match s.get("WhiteBalance").and_then(Value::text) {
        Some("As Shot") => {}
        Some("Auto") => left_out.push("Auto white balance"),
        _ => {
            if let Some(k) = num(s, "Temperature") {
                e.white_balance.kelvin = Some(k);
                e.white_balance.tint =
                    num(s, "Tint").map_or(0.0, |t| (t * 100.0 / 150.0).clamp(-100.0, 100.0));
            }
        }
    }
    if nonzero(s, "IncrementalTemperature") || nonzero(s, "IncrementalTint") {
        left_out.push("a relative white balance");
    }

    // Tone.
    set(&mut e.exposure, s, "Exposure2012");
    set(&mut e.contrast, s, "Contrast2012");
    set(&mut e.highlights, s, "Highlights2012");
    set(&mut e.shadows, s, "Shadows2012");
    set(&mut e.whites, s, "Whites2012");
    set(&mut e.blacks, s, "Blacks2012");
    set(&mut e.local_contrast.texture, s, "Texture");
    set(&mut e.local_contrast.clarity, s, "Clarity2012");
    set(&mut e.dehaze.amount, s, "Dehaze");
    for (key, curve) in [
        ("ToneCurvePV2012", &mut e.tone_curve),
        ("ToneCurvePV2012Red", &mut e.tone_curve_red),
        ("ToneCurvePV2012Green", &mut e.tone_curve_green),
        ("ToneCurvePV2012Blue", &mut e.tone_curve_blue),
    ] {
        if let Some(v) = s.get(key) {
            curve.points = points(v);
        }
    }
    let pc = &mut e.tone_curve_parametric;
    set(&mut pc.shadows, s, "ParametricShadows");
    set(&mut pc.darks, s, "ParametricDarks");
    set(&mut pc.lights, s, "ParametricLights");
    set(&mut pc.highlights, s, "ParametricHighlights");
    for (i, k) in [
        "ParametricShadowSplit",
        "ParametricMidtoneSplit",
        "ParametricHighlightSplit",
    ]
    .iter()
    .enumerate()
    {
        if let Some(v) = num(s, k) {
            pc.splits[i] = v / 100.0;
        }
    }

    // Colour.
    set(&mut e.vibrance, s, "Vibrance");
    set(&mut e.saturation, s, "Saturation");
    for (i, band) in HSL_BAND_NAMES.iter().enumerate() {
        let b = &mut e.hsl.bands[i];
        set(&mut b.hue, s, &format!("HueAdjustment{band}"));
        set(&mut b.saturation, s, &format!("SaturationAdjustment{band}"));
        set(&mut b.luminance, s, &format!("LuminanceAdjustment{band}"));
    }
    // ⚠ Lightroom's colour grading keeps the shadow and highlight wheels' hue and saturation under the
    // older split-toning names, and only their luminance under `ColorGrade…`.
    let g = &mut e.color_grading;
    for (wheel, hue, sat, lum) in [
        (
            &mut g.shadows,
            "SplitToningShadowHue",
            "SplitToningShadowSaturation",
            "ColorGradeShadowLum",
        ),
        (
            &mut g.midtones,
            "ColorGradeMidtoneHue",
            "ColorGradeMidtoneSat",
            "ColorGradeMidtoneLum",
        ),
        (
            &mut g.highlights,
            "SplitToningHighlightHue",
            "SplitToningHighlightSaturation",
            "ColorGradeHighlightLum",
        ),
        (
            &mut g.global,
            "ColorGradeGlobalHue",
            "ColorGradeGlobalSat",
            "ColorGradeGlobalLum",
        ),
    ] {
        if let Some(h) = num(s, hue) {
            wheel.hue = lr_hue_to_oklab(f64::from(h));
        }
        set(&mut wheel.saturation, s, sat);
        set(&mut wheel.luminance, s, lum);
    }
    set(&mut g.blending, s, "ColorGradeBlending");
    set(&mut g.balance, s, "SplitToningBalance");
    if truthy(s, "ConvertToGrayscale") {
        e.saturation = -100.0;
        if s.keys()
            .any(|k| k.starts_with("GrayMixer") && nonzero(s, k))
        {
            left_out.push("the black & white mix");
        }
    }

    // Detail. ⚠ Lightroom's sharpening runs to 150; Rustroom's to 100.
    if let Some(v) = num(s, "Sharpness") {
        e.sharpening.amount = v.min(100.0);
    }
    set(&mut e.sharpening.radius, s, "SharpenRadius");
    set(&mut e.sharpening.detail, s, "SharpenDetail");
    set(&mut e.sharpening.masking, s, "SharpenEdgeMasking");
    set(&mut e.noise_reduction.luminance, s, "LuminanceSmoothing");
    set(
        &mut e.noise_reduction.detail,
        s,
        "LuminanceNoiseReductionDetail",
    );
    set(&mut e.noise_reduction.color, s, "ColorNoiseReduction");
    set(
        &mut e.noise_reduction.smoothness,
        s,
        "ColorNoiseReductionSmoothness",
    );

    // Vignette.
    set(&mut e.vignette.amount, s, "PostCropVignetteAmount");
    set(&mut e.vignette.midpoint, s, "PostCropVignetteMidpoint");
    set(&mut e.vignette.roundness, s, "PostCropVignetteRoundness");
    set(&mut e.vignette.feather, s, "PostCropVignetteFeather");
    // M10.11b: grain, one to one — Lightroom's Frequency is the Roughness slider.
    set(&mut e.grain.amount, s, "GrainAmount");
    set(&mut e.grain.size, s, "GrainSize");
    set(&mut e.grain.roughness, s, "GrainFrequency");

    // Lens: the profile's switch and chromatic aberration; defringe amounts (0–20 there, 0–100 here).
    let profile = truthy(s, "LensProfileEnable");
    let tca = truthy(s, "AutoLateralCA");
    e.lens_correction = (profile || tca).then_some(LensCorrection {
        distortion: profile,
        vignetting: profile,
        tca,
        ..LensCorrection::default()
    });
    if let Some(v) = num(s, "DefringePurpleAmount") {
        e.defringe.purple_amount = (v * 5.0).min(100.0);
    }
    if let Some(v) = num(s, "DefringeGreenAmount") {
        e.defringe.green_amount = (v * 5.0).min(100.0);
    }

    // What there is no counterpart for — named only when it would change something.
    if [
        "MaskGroupBasedCorrections",
        "PaintBasedCorrections",
        "GradientBasedCorrections",
        "CircularGradientBasedCorrections",
    ]
    .iter()
    .any(|k| present(s, k))
    {
        left_out.push("masks");
    }
    if truthy(s, "HasCrop") {
        left_out.push("crop");
    }
    let profile_name = s.get("CameraProfile").and_then(Value::text).unwrap_or("");
    if s.get("Look")
        .and_then(Value::text)
        .is_some_and(|l| !l.is_empty())
        || !matches!(profile_name, "" | "Adobe Standard" | "Adobe Color")
    {
        left_out.push("the profile");
    }
    if [
        "RedHue",
        "RedSaturation",
        "GreenHue",
        "GreenSaturation",
        "BlueHue",
        "BlueSaturation",
        "ShadowTint",
    ]
    .iter()
    .any(|k| nonzero(s, k))
    {
        left_out.push("the camera calibration");
    }
    if truthy(s, "AutoTone") {
        left_out.push("Auto tone");
    }
    if [
        "PerspectiveVertical",
        "PerspectiveHorizontal",
        "PerspectiveRotate",
        "PerspectiveAspect",
        "PerspectiveX",
        "PerspectiveY",
        "PerspectiveUpright",
    ]
    .iter()
    .any(|k| nonzero(s, k))
        || num(s, "PerspectiveScale").is_some_and(|v| v != 100.0)
    {
        left_out.push("perspective");
    }
    if nonzero(s, "LensManualDistortionAmount") || nonzero(s, "VignetteAmount") {
        left_out.push("manual lens corrections");
    }
    if ["RetouchAreas", "RetouchInfo"]
        .iter()
        .any(|k| present(s, k))
    {
        left_out.push("spot removal");
    }
    if ["Brightness", "FillLight", "Recovery"]
        .iter()
        .any(|k| nonzero(s, k))
        || (nonzero(s, "Exposure") && !s.contains_key("Exposure2012"))
    {
        left_out.push("settings of the pre-2012 process");
    }
    left_out.dedup();
    Mapped {
        edits: e,
        groups,
        left_out,
    }
}

/// Lightroom's hue-wheel angle (HSV hue, degrees) as Rustroom's Oklab hue — through the colour it
/// names, the way the colour wheel draws it (sRGB → linear → Oklab, `atan2(b, a)`).
pub fn lr_hue_to_oklab(deg: f64) -> f32 {
    let h = deg.rem_euclid(360.0) / 60.0;
    let x = (1.0 - ((h % 2.0) - 1.0).abs()) as f32;
    let (r, g, b) = match h as u32 {
        0 => (1.0, x, 0.0),
        1 => (x, 1.0, 0.0),
        2 => (0.0, 1.0, x),
        3 => (0.0, x, 1.0),
        4 => (x, 0.0, 1.0),
        _ => (1.0, 0.0, x),
    };
    let lin = [r, g, b].map(color::srgb_to_linear);
    let lab = color::oklab::linear_srgb_to_oklab(lin);
    lab[2].atan2(lab[1]).to_degrees().rem_euclid(360.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Settings {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/questtion")
            .join(name);
        crate::xmp::read(&std::fs::read_to_string(p).unwrap())
            .unwrap()
            .settings
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.5
    }

    /// Lightroom's red, green and blue land where Oklab puts sRGB red, green and blue.
    #[test]
    fn a_lightroom_hue_lands_on_the_oklab_hue_of_its_colour() {
        for (lr, ok) in [(0.0, 29.2), (120.0, 142.5), (240.0, 264.1)] {
            let got = lr_hue_to_oklab(lr);
            assert!(close(got, ok), "{lr}° → {got}, want ~{ok}");
        }
    }

    /// ⚠⚠ The groups exactly as the file carries them: Preset 4 sets tone and colour only.
    #[test]
    fn a_preset_carries_exactly_its_groups() {
        let m = to_edits(&fixture("Preset 4.xmp"));
        assert!(m.groups.tone && m.groups.color, "{:?}", m.groups);
        assert!(
            !m.groups.white_balance && !m.groups.detail && !m.groups.vignette && !m.groups.lens,
            "{:?}",
            m.groups
        );
        let m = to_edits(&fixture("Play Prom Try 2.xmp"));
        assert!(
            m.groups.white_balance && m.groups.detail && m.groups.vignette,
            "{:?}",
            m.groups
        );
    }

    /// Values: a curve's points from 0–255, the HSL band, white balance, colour grading's hue.
    #[test]
    fn values_are_converted() {
        let s = fixture("Preset 4.xmp");
        let m = to_edits(&s);
        let p = &m.edits.tone_curve.points;
        assert_eq!(p.len(), 4, "{p:?}");
        assert!(
            close(p[1][0] * 255.0, 53.0) && close(p[2][1] * 255.0, 168.0),
            "{p:?}"
        );
        assert!(close(
            m.edits.color_grading.midtones.hue,
            lr_hue_to_oklab(53.0)
        ));
        assert!(close(m.edits.color_grading.midtones.saturation, 33.0));
        assert!(close(m.edits.color_grading.shadows.luminance, -23.0));
        assert!(close(m.edits.color_grading.blending, 50.0));
        let m = to_edits(&fixture("Play Prom Try 2.xmp"));
        assert_eq!(m.edits.white_balance.kelvin, Some(4200.0));
        assert!(close(m.edits.hsl.bands[0].hue, 10.0), "Red's hue");
        assert!(close(m.edits.sharpening.amount, 40.0));
    }

    /// Black & white is saturation −100; "As Shot" is no white balance of its own.
    #[test]
    fn black_and_white_and_as_shot() {
        let m = to_edits(&fixture("BW 2 +1.2.xmp"));
        assert!(close(m.edits.saturation, -100.0));
        assert!(m.groups.white_balance && m.edits.white_balance.kelvin.is_none());
    }

    /// What has no counterpart is named — only when it would change something.
    #[test]
    fn what_is_left_out_is_named_only_when_set() {
        let m = to_edits(&fixture("Darken Background Mask.xmp"));
        assert!(m.left_out.contains(&"masks"), "{:?}", m.left_out);
        assert_eq!(m.groups, EditGroups::NONE, "a mask alone carries nothing");
        let m = to_edits(&fixture("Preset 4.xmp"));
        assert!(
            m.left_out.contains(&"the profile"),
            "its Look: {:?}",
            m.left_out
        );
        let plain = fixture("Open Mic Night 3.xmp");
        assert!(
            !to_edits(&plain).left_out.contains(&"grain"),
            "a zero grain is nothing"
        );
        let mut grainy = plain.clone();
        grainy.insert("GrainAmount".into(), crate::Value::Num(25.0));
        // ⚠ M10.11b: grain has a counterpart now — never left out again.
        assert!(!to_edits(&grainy).left_out.contains(&"grain"));
    }

    /// M10.11b: Lightroom's grain maps one to one, and carries the Effects group even alone.
    #[test]
    fn lightrooms_grain_maps_across() {
        let mut s = Settings::new();
        s.insert("GrainAmount".into(), crate::Value::Num(25.0));
        s.insert("GrainSize".into(), crate::Value::Num(30.0));
        s.insert("GrainFrequency".into(), crate::Value::Num(60.0));
        let m = to_edits(&s);
        assert_eq!(
            (
                m.edits.grain.amount,
                m.edits.grain.size,
                m.edits.grain.roughness
            ),
            (25.0, 30.0, 60.0)
        );
        assert!(m.groups.vignette, "grain alone carries the Effects group");
        assert!(m.left_out.is_empty(), "{:?}", m.left_out);
        let mut zero = Settings::new();
        zero.insert("GrainAmount".into(), crate::Value::Num(0.0));
        assert_eq!(to_edits(&zero).edits.grain, edit_model::Grain::default());
    }
}
