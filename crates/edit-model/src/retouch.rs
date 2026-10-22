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
    /// M10.19d: made by Find dust — the Retouches list says "Dust N". The patch renders as any other.
    #[serde(default, skip_serializing_if = "is_false")]
    pub dust: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
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

/// One content-aware removal (M10.16c1): a painted stroke — or a person found by Find people (M10.16c3) —
/// whose area a network fills from its surroundings.
///
/// ⚠⚠ **In DECODE space, unlike [`Spot`] and [`Eye`]:** `points` are normalised over the undeveloped
/// decode — sensor orientation, before lens correction — because the fill is computed from the decode
/// and pasted onto it, right after AI denoise. A spot's post-lens frame would put the fill where the
/// lens warp moved it.
///
/// ⚠ The fill itself is not here: it is derived (by the app, once, cached) from the stroke or the person's
/// box, `detect`, `variant` and the network — never from `opacity`, which only blends it.
///
/// ⚠ No container-level `serde(default)`, `Spot`'s reason: a removal with its points defaulted would be
/// one nobody painted. Only the three settings and `person` are optional — a person's removal says so with
/// empty `points`, which it always writes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Removal {
    /// The stroke's dab centres, normalised per axis over the decode.
    pub points: Vec<[f32; 2]>,
    /// The brush radius as a fraction of the decode's **long edge** ([`Spot::radius`]'s rule).
    pub radius: f32,
    /// Detect objects: the stroke grows to the object under it (MobileSAM) before the fill.
    #[serde(default)]
    pub detect: bool,
    /// Refresh's counter: each value fills from another context window.
    #[serde(default)]
    pub variant: u32,
    /// 0..=100: how much of the fill is laid over the photo.
    #[serde(default = "full_opacity")]
    pub opacity: f32,
    /// M10.16c3 — a person found by Find people: their box, normalised over the decode. Such a removal has no
    /// stroke; its hole is MobileSAM's object from the box.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub person: Option<[f32; 4]>,
}

fn full_opacity() -> f32 {
    100.0
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

    /// M10.19d: a patch Find dust made is marked so the list can say "Dust N"; an unmarked patch serializes as it
    /// always did, so no saved photo's file changes.
    #[test]
    fn a_dust_mark_round_trips_and_a_plain_spot_is_unchanged() {
        let plain = Spot {
            centre: [0.25, 0.75],
            source: [0.5, 0.75],
            radius: 0.02,
            feather: 60.0,
            mode: SpotMode::Heal,
            dust: false,
        };
        let text = serde_json::to_string(&plain).unwrap();
        assert!(!text.contains("dust"), "{text}");
        let marked = Spot {
            dust: true,
            ..plain
        };
        let back: Spot = serde_json::from_str(&serde_json::to_string(&marked).unwrap()).unwrap();
        assert!(back.dust);
    }

    #[test]
    fn a_spot_round_trips_and_an_unstated_mode_means_heal() {
        let e = Edits {
            spots: vec![Spot {
                centre: [0.25, 0.75],
                source: [0.5, 0.75],
                radius: 0.02,
                feather: 60.0,
                mode: SpotMode::Clone,
                dust: false,
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

    /// M10.16c1: a removal round-trips; an unstated `detect`, `variant` and `opacity` mean off, the first
    /// fill and full strength; a removal nobody painted (no points) is refused; none is not written.
    #[test]
    fn a_removal_round_trips_and_its_optional_fields_default() {
        let e = Edits {
            removals: vec![Removal {
                points: vec![[0.1, 0.2], [0.15, 0.22]],
                radius: 0.03,
                detect: true,
                variant: 4,
                opacity: 55.0,
                person: None,
            }],
            ..Edits::default()
        };
        let back: Edits = serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
        assert_eq!(back, e);
        let hand = r#"{"removals":[{"points":[[0.1,0.2]],"radius":0.03}]}"#;
        let r = &serde_json::from_str::<Edits>(hand).unwrap().removals[0];
        assert_eq!((r.detect, r.variant, r.opacity), (false, 0, 100.0));
        let unpainted = r#"{"removals":[{"radius":0.03}]}"#;
        assert!(
            serde_json::from_str::<Edits>(unpainted).is_err(),
            "a removal nobody painted must not be invented"
        );
        let none = serde_json::to_string(&Edits::default()).unwrap();
        assert!(!none.contains("removals"), "{none}");
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
