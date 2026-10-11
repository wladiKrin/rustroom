//! Carrying an `Edits` through the **system** clipboard.
//!
//! Why this exists rather than a bare `serde_json::to_string(&edits)`:
//!
//! 1. **egui owns the clipboard shortcuts.** `egui-winit` turns Cmd/Ctrl+C into
//!    `Event::Copy` and returns *without* emitting a `Key::C` event, so watching for the
//!    key press never fires. Worse, `Event::Paste` is only pushed when the system clipboard
//!    already holds non-empty text — so unless copying actually writes something there,
//!    Ctrl+V produces no event at all.
//! 2. **⚠ A bare `Edits` is not safely decodable.** Every field is `#[serde(default)]`, so
//!    `{}` — or any unrelated JSON object — deserializes into a perfectly valid *neutral*
//!    `Edits`. Pasting a stray snippet of JSON would silently reset the photo. The envelope's
//!    marker field is what makes "is this ours?" answerable at all.

use edit_model::Edits;
use serde::{Deserialize, Serialize};

/// Bumped if the payload shape ever changes; an unknown version is refused rather than
/// guessed at.
const FORMAT: u32 = 1;

#[derive(Serialize, Deserialize)]
struct Envelope {
    /// Marker *and* version. Its presence is what distinguishes our payload from arbitrary
    /// JSON that would otherwise deserialize into a neutral `Edits`.
    rustroom_edits: u32,
    edits: Edits,
}

/// Serialize `edits` for the system clipboard.
pub fn encode(edits: &Edits) -> String {
    serde_json::to_string_pretty(&Envelope {
        rustroom_edits: FORMAT,
        edits: edits.clone(),
    })
    .unwrap_or_default()
}

/// Recover an `Edits` from clipboard text, or `None` if this text is not ours.
///
/// Refusing is the common case — the clipboard usually holds something else entirely — so
/// this must never guess.
pub fn decode(text: &str) -> Option<Edits> {
    let env: Envelope = serde_json::from_str(text).ok()?;
    (env.rustroom_edits == FORMAT).then_some(env.edits)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Edits {
        Edits {
            exposure: 1.25,
            contrast: -30.0,
            ..Edits::default()
        }
    }

    #[test]
    fn round_trips_through_text() {
        assert_eq!(decode(&encode(&sample())), Some(sample()));
    }

    #[test]
    fn an_empty_json_object_is_refused() {
        // ⚠ The hazard this module exists for. `Edits` is `#[serde(default)]` throughout,
        // so `{}` IS a valid neutral `Edits` — decoding it would silently wipe the photo's
        // settings on a stray paste.
        assert_eq!(decode("{}"), None);
        assert_eq!(decode(r#"{"exposure": 5.0}"#), None);
    }

    #[test]
    fn unrelated_clipboard_text_is_refused() {
        for text in ["", "hello world", "[1,2,3]", "null", "{ not json"] {
            assert_eq!(decode(text), None, "{text:?} should not decode");
        }
    }

    #[test]
    fn a_future_format_version_is_refused_not_guessed() {
        let text = encode(&sample()).replace("\"rustroom_edits\": 1", "\"rustroom_edits\": 99");
        assert!(text.contains("99"), "test rewrote the wrong field");
        assert_eq!(decode(&text), None);
    }

    #[test]
    fn the_payload_survives_every_group() {
        // The clipboard must carry the whole `Edits`, not just the fields someone
        // remembered — this is the same coverage concern `merge_from` has.
        let mut hsl = edit_model::Hsl::default();
        hsl.bands[2].hue = 40.0;
        let rich = Edits {
            version: 0,
            white_balance: edit_model::WhiteBalance {
                kelvin: Some(4200.0),
                tint: -5.0,
            },
            exposure: 0.75,
            contrast: 15.0,
            highlights: -20.0,
            shadows: 25.0,
            whites: 10.0,
            blacks: -10.0,
            tone_curve: edit_model::ToneCurve {
                points: vec![[0.0, 0.0], [0.4, 0.5], [1.0, 1.0]],
            },
            // ⚠⚠ **Three DIFFERENT curves**, so a paste that dropped the channel index would fail
            // here rather than pass — the fixture trap paid for on 2026-09-23.
            tone_curve_red: edit_model::ToneCurve {
                points: vec![[0.0, 0.15], [1.0, 1.0]],
            },
            tone_curve_green: edit_model::ToneCurve {
                points: vec![[0.0, 0.0], [0.5, 0.55], [1.0, 1.0]],
            },
            tone_curve_blue: edit_model::ToneCurve {
                points: vec![[0.0, 0.0], [1.0, 0.85]],
            },
            tone_curve_parametric: edit_model::ParametricCurve {
                shadows: 20.0,
                darks: -10.0,
                lights: 15.0,
                highlights: -25.0,
                splits: [0.2, 0.45, 0.8],
            },
            saturation: 12.0,
            vibrance: 8.0,
            hsl,
            color_grading: edit_model::ColorGrading {
                midtones: edit_model::GradeWheel {
                    hue: 160.0,
                    saturation: 25.0,
                    luminance: 5.0,
                },
                blending: 40.0,
                balance: -10.0,
                ..Default::default()
            },
            local_contrast: edit_model::LocalContrast {
                texture: 30.0,
                clarity: -15.0,
            },
            dehaze: edit_model::Dehaze { amount: -20.0 },
            noise_reduction: edit_model::NoiseReduction {
                luminance: 20.0,
                ..Default::default()
            },
            sharpening: edit_model::Sharpening {
                amount: 55.0,
                ..Default::default()
            },
            defringe: edit_model::Defringe {
                green_amount: 25.0,
                ..Default::default()
            },
            lens_correction: Some(edit_model::LensCorrection {
                distortion: true,
                ..Default::default()
            }),
            vignette: edit_model::Vignette {
                amount: -25.0,
                ..Default::default()
            },
            perspective: edit_model::Perspective {
                vertical: 25.0,
                ..Default::default()
            },
            geometry: edit_model::Geometry {
                angle: 1.5,
                ..Default::default()
            },
            masks: vec![edit_model::MaskedAdjustment {
                mask: edit_model::Mask::Linear(edit_model::LinearGradient {
                    from: [0.0, 0.0],
                    to: [1.0, 1.0],
                    invert: false,
                }),
                // ⚠ Non-empty on purpose: this payload fixture exists to prove every part of an
                // `Edits` survives the clipboard envelope, and an empty list would prove nothing
                // about the half a composite lives in.
                refine: vec![edit_model::Refinement {
                    op: edit_model::MaskOp::Subtract,
                    mask: edit_model::Mask::Radial(edit_model::RadialGradient::default()),
                }],
                adjustments: edit_model::LocalAdjustments {
                    exposure: 0.75,
                    ..Default::default()
                },
                disabled: false,
            }],
            spots: vec![edit_model::Spot {
                centre: [0.3, 0.4],
                source: [0.5, 0.4],
                radius: 0.05,
                feather: 30.0,
                mode: edit_model::SpotMode::Clone,
            }],
            eyes: vec![edit_model::Eye {
                centre: [0.6, 0.35],
                radius: 0.01,
                kind: edit_model::EyeKind::Pet,
                pupil_size: 60.0,
                darken: 40.0,
                catchlight: Some([-0.35, -0.35]),
            }],
            rendering: Some(edit_model::Rendering::Camera),
            look: edit_model::Look {
                file: Some("portra.cube".into()),
                space: edit_model::LutSpace::Linear,
                strength: 80.0,
            },
            negative: edit_model::Negative {
                on: true,
                base: [0.8, 0.5, 0.3],
                gamma: 0.65,
                low: [0.1, 0.1, 0.1],
                high: [1.9, 2.0, 2.2],
                mono: false,
            },
        };
        assert_eq!(decode(&encode(&rich)), Some(rich));
    }
}
