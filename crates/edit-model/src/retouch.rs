//! Retouching: spot removal and eye correction.

use serde::{Deserialize, Serialize};

/// One retouch patch: pixels copied from `source` onto `centre`.
///
/// ⚠ **`centre` and `source` are in pre-geometry SOURCE normalised coordinates**, like
/// [`BrushStroke::points`] and unlike the analytic masks. A spot is content-referred — a dust
/// spot is on the sensor, a blemish on the subject — so it must not slide when the crop changes.
/// `SpotRemovalStage` therefore runs on `GeometryStage`'s *input*, which is also the space
/// `FrameMap::inverse_point` already maps a canvas click into.
///
/// ⚠⚠ **Stays `Copy`.** `Mask` lost `Copy` for the brush's stroke list and three call sites paid
/// for it; a spot is five numbers and an enum.
///
/// ⚠ No container-level `#[serde(default)]`, deliberately: a spot with its `centre` silently
/// defaulted to the top-left corner would be a patch nobody placed. Only `mode` is optional,
/// because [`SpotMode`]'s `Default` says what an unstated mode means.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Spot {
    /// Destination centre, normalised per axis over the **post-lens, pre-perspective** frame —
    /// since M7.3b, when perspective was fused into the geometry resample. Before that it was the
    /// post-perspective frame the brush still uses; no sidecar with both spots and an active
    /// perspective existed when it changed.
    pub centre: [f32; 2],
    /// Source centre, same space.
    pub source: [f32; 2],
    /// Radius as a fraction of the pre-geometry frame's **long edge** — [`BrushStroke::radius`]'s
    /// rule, for its reason: `centre` lives in an anisotropic space, and a radius measured
    /// against width but compared against height would make every patch an ellipse.
    pub radius: f32,
    /// 0 = hard edge, 100 = the ramp runs from the centre to the rim. [`RadialGradient::feather`]'s
    /// convention and its smoothstep.
    pub feather: f32,
    #[serde(default)]
    pub mode: SpotMode,
}

/// How a [`Spot`]'s source pixels are laid onto its destination.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SpotMode {
    /// Straight copy, blended by the feather.
    Clone,
    /// Clone with the source patch's mean RGB shifted to match the ring around the destination —
    /// what makes a spot on a sky *disappear* rather than leave a disc of slightly different sky.
    #[default]
    Heal,
}

/// One red- or pet-eye correction: a circle around a pupil, detected at the click and stored.
///
/// ⚠ **The stage never detects** (M9.11 spec, approach A): an eye that re-detected on every render
/// would move whenever an upstream edit changed the pixels. The circle is found once, in the app,
/// and from then on it is an edit like any other.
///
/// ⚠ Same space as [`Spot`]: post-lens, pre-perspective frame, normalised per axis; `radius` a
/// fraction of the long edge. Same `Copy` and no container-level `serde(default)`, for `Spot`'s
/// reasons — an eye with a defaulted `centre` would be one nobody placed.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Eye {
    pub centre: [f32; 2],
    pub radius: f32,
    pub kind: EyeKind,
    /// 0..=100: how far the correction reaches into weakly red (pet: dimmer) pixels. The circle
    /// still bounds it.
    pub pupil_size: f32,
    /// 0..=100: how dark the corrected pupil ends up.
    pub darken: f32,
    /// Pet only: the catchlight's offset from `centre`, in **radius** units of the stage's frame
    /// (a pixel direction, so both axes share one scale). `None` = no catchlight.
    ///
    /// ⚠ Stored, not derived in the stage: the stage's frame is pre-orientation, so "up-left on
    /// screen" is only knowable in the app.
    #[serde(default)]
    pub catchlight: Option<[f32; 2]>,
}

/// What kind of flash reflection an [`Eye`] corrects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum EyeKind {
    /// A human red pupil: the red is removed and the pupil darkened.
    #[default]
    Red,
    /// An animal's green, yellow or white eyeshine: replaced by a dark pupil.
    Pet,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Edits;

    #[test]
    fn a_spot_round_trips_and_an_unstated_mode_means_heal() {
        let e = Edits {
            spots: vec![Spot {
                centre: [0.25, 0.75],
                source: [0.5, 0.75],
                radius: 0.02,
                feather: 60.0,
                mode: SpotMode::Clone,
            }],
            ..Edits::default()
        };
        let text = serde_json::to_string(&e).unwrap();
        let back: Edits = serde_json::from_str(&text).unwrap();
        assert_eq!(back, e);

        // `mode` is the one optional field; everything else a spot needs to be a spot.
        let hand_written =
            r#"{"spots":[{"centre":[0.1,0.2],"source":[0.3,0.2],"radius":0.01,"feather":0.0}]}"#;
        let e: Edits = serde_json::from_str(hand_written).unwrap();
        assert_eq!(e.spots[0].mode, SpotMode::Heal);
        assert_eq!(e.spots[0].centre, [0.1, 0.2]);

        let no_centre = r#"{"spots":[{"source":[0.3,0.2],"radius":0.01,"feather":0.0}]}"#;
        assert!(
            serde_json::from_str::<Edits>(no_centre).is_err(),
            "a spot nobody placed must not be invented at the corner"
        );

        // An older sidecar has no `spots` key and must load with none.
        let e: Edits = serde_json::from_str(r#"{"exposure": 0.5}"#).unwrap();
        assert!(e.spots.is_empty());
    }

    #[test]
    fn an_eye_round_trips_and_an_unstated_catchlight_is_none() {
        let e = Edits {
            eyes: vec![Eye {
                centre: [0.4, 0.3],
                radius: 0.01,
                kind: EyeKind::Pet,
                pupil_size: 70.0,
                darken: 20.0,
                catchlight: Some([-0.3, -0.3]),
            }],
            ..Edits::default()
        };
        let back: Edits = serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
        assert_eq!(back, e);
        let hand = r#"{"eyes":[{"centre":[0.1,0.2],"radius":0.01,"kind":"Red","pupil_size":50.0,"darken":50.0}]}"#;
        let e: Edits = serde_json::from_str(hand).unwrap();
        assert_eq!(e.eyes[0].catchlight, None);
        assert_eq!(e.eyes[0].kind, EyeKind::Red);
        let no_centre =
            r#"{"eyes":[{"radius":0.01,"kind":"Red","pupil_size":50.0,"darken":50.0}]}"#;
        assert!(
            serde_json::from_str::<Edits>(no_centre).is_err(),
            "an eye nobody placed must not be invented at the corner"
        );
        let old: Edits = serde_json::from_str(r#"{"exposure": 0.5}"#).unwrap();
        assert!(old.eyes.is_empty());
    }
}
