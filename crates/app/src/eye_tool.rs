//! The red-eye tool's live settings and the logic behind its gestures — no egui (`spot_tool`'s
//! shape; the gestures themselves live in the bin's `eye_overlay`).

use crate::eye_detect::Detection;
use crate::spot_tool::{MAX_RADIUS, MIN_RADIUS};
use edit_model::{Eye, EyeKind};

/// Distance of a new pet eye's catchlight from the centre, in eye radii.
pub const CATCHLIGHT_OFFSET: f32 = 0.35;
/// The rim band a resize grab accepts, as a fraction of the radius either side of the rim.
pub const RIM_BAND: f32 = 0.25;

/// Live settings, session-lifetime and not persisted (`SpotTool`'s rule).
///
/// ⚠⚠ `active` IS the Retouch tool's mode for eyes: `None` means it places patches (Heal/Clone,
/// `SpotTool::mode`), `Some(kind)` means it places eyes. One field rather than a separate mode enum
/// beside `SpotTool::mode`, so the two can never disagree about which gesture a click is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EyeTool {
    pub active: Option<EyeKind>,
    /// The radius a click that finds nothing places — and an edge drag writes back.
    pub radius: f32,
    /// A new eye inherits the last values used (the M9.11 brainstorm's choice).
    pub pupil_size: f32,
    pub darken: f32,
    /// Whether a new pet eye gets a catchlight. On by default: a dark pupil without one reads
    /// as a hole.
    pub catchlight: bool,
}

impl Default for EyeTool {
    /// ⚠ Hand-written: a derived radius of 0 would place eyes that cover nothing.
    fn default() -> Self {
        Self {
            active: None,
            radius: 0.006,
            pupil_size: 50.0,
            darken: 50.0,
            catchlight: true,
        }
    }
}

/// What a press on an existing eye does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grab {
    Move,
    Resize,
}

/// The pet-eye catchlight used when none can be derived from a click: up-left in the stage's own
/// (pre-orientation) frame.
pub const FALLBACK_CATCHLIGHT: [f32; 2] = [
    -CATCHLIGHT_OFFSET * std::f32::consts::FRAC_1_SQRT_2,
    -CATCHLIGHT_OFFSET * std::f32::consts::FRAC_1_SQRT_2,
];

/// A new eye from a detection — or at the click with the tool's radius when nothing was found,
/// because a click that does nothing is worse than a circle the user can resize.
pub fn place(
    found: Detection,
    click: [f32; 2],
    kind: EyeKind,
    t: &EyeTool,
    catchlight: Option<[f32; 2]>,
) -> Eye {
    let (centre, radius) = match found {
        Detection::Found { centre, radius } => (centre, radius),
        Detection::NotFound => (click, t.radius),
    };
    Eye {
        decode: false,
        centre,
        radius: radius.clamp(MIN_RADIUS, MAX_RADIUS),
        kind,
        pupil_size: t.pupil_size,
        darken: t.darken,
        catchlight: if kind == EyeKind::Pet {
            catchlight
        } else {
            None
        },
    }
}

/// Which eye (last placed first) and which grab `p` lands on. Distances in long-edge units, as
/// `spot_tool::hit_test`; `min_grab` keeps a tiny eye's rim grabbable.
pub fn hit_test(eyes: &[Eye], p: [f32; 2], aspect: f32, min_grab: f32) -> Option<(usize, Grab)> {
    let (sx, sy) = crate::spot_tool::long_edge_scale(aspect);
    eyes.iter().enumerate().rev().find_map(|(i, e)| {
        let d = ((p[0] - e.centre[0]) * sx).hypot((p[1] - e.centre[1]) * sy);
        let band = (RIM_BAND * e.radius).max(min_grab * 0.5);
        if (d - e.radius).abs() <= band {
            Some((i, Grab::Resize))
        } else if d < e.radius {
            Some((i, Grab::Move))
        } else {
            None
        }
    })
}

/// The catchlight offset (radius units, length [`CATCHLIGHT_OFFSET`]) pointing from `centre`
/// toward `toward`, both normalised in a frame of `dims` pixels.
///
/// ⚠ A PIXEL direction, because the stage multiplies the offset by the pixel radius on both axes.
/// The overlay passes the source point under "up-left of the click on screen", which is what makes
/// the default orientation-proof — the stage's frame is pre-orientation.
pub fn catchlight_offset(centre: [f32; 2], toward: [f32; 2], dims: (usize, usize)) -> [f32; 2] {
    let dx = (toward[0] - centre[0]) * dims.0 as f32;
    let dy = (toward[1] - centre[1]) * dims.1 as f32;
    let len = dx.hypot(dy);
    if !len.is_finite() || len <= f32::EPSILON {
        return FALLBACK_CATCHLIGHT;
    }
    [CATCHLIGHT_OFFSET * dx / len, CATCHLIGHT_OFFSET * dy / len]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_placed_eye_takes_the_tools_settings_and_the_detections_circle() {
        let t = EyeTool {
            pupil_size: 70.0,
            darken: 30.0,
            ..EyeTool::default()
        };
        let found = Detection::Found {
            centre: [0.3, 0.4],
            radius: 0.01,
        };
        let e = place(found, [0.31, 0.4], EyeKind::Red, &t, Some([-0.3, -0.3]));
        assert_eq!(
            (e.centre, e.radius, e.pupil_size, e.darken),
            ([0.3, 0.4], 0.01, 70.0, 30.0)
        );
        assert_eq!(e.catchlight, None, "a red eye never gets a catchlight");
    }

    #[test]
    fn not_found_places_at_the_click_with_the_tools_radius() {
        let t = EyeTool::default();
        let e = place(
            Detection::NotFound,
            [0.5, 0.5],
            EyeKind::Pet,
            &t,
            Some([-0.3, -0.3]),
        );
        assert_eq!((e.centre, e.radius), ([0.5, 0.5], t.radius));
        assert_eq!(e.catchlight, Some([-0.3, -0.3]));
    }

    #[test]
    fn hit_test_moves_inside_and_resizes_on_the_rim() {
        let found = Detection::Found {
            centre: [0.5, 0.5],
            radius: 0.02,
        };
        let eyes = [place(
            found,
            [0.5, 0.5],
            EyeKind::Red,
            &EyeTool::default(),
            None,
        )];
        assert_eq!(
            hit_test(&eyes, [0.5, 0.5], 1.0, 0.002),
            Some((0, Grab::Move))
        );
        assert_eq!(
            hit_test(&eyes, [0.52, 0.5], 1.0, 0.002),
            Some((0, Grab::Resize))
        );
        assert_eq!(hit_test(&eyes, [0.6, 0.5], 1.0, 0.002), None);
    }

    #[test]
    fn a_catchlight_offset_points_from_the_centre_toward_the_given_point() {
        let o = catchlight_offset([0.5, 0.5], [0.49, 0.49], (1000, 1000));
        assert!((o[0] - o[1]).abs() < 1e-6 && o[0] < 0.0, "{o:?}");
        assert!((o[0].hypot(o[1]) - CATCHLIGHT_OFFSET).abs() < 1e-5);
        // Rotated: "up-left on screen" arrives as down-left in the source, and must stay that.
        let o = catchlight_offset([0.5, 0.5], [0.49, 0.51], (1000, 1000));
        assert!(o[0] < 0.0 && o[1] > 0.0, "{o:?}");
        assert_eq!(
            catchlight_offset([0.5, 0.5], [0.5, 0.5], (1000, 1000)),
            FALLBACK_CATCHLIGHT
        );
    }
}
