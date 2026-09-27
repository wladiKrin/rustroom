//! The retouch tool's gesture maths: which handle a point grabs, and what a drag does to it.
//!
//! ⚠ No egui. `mask_tool`'s rule, for its reason: the decisions are what is worth testing, and a
//! window is not needed to test them.

use edit_model::{Spot, SpotMode};

/// The smallest radius a gesture may produce, as a fraction of the long edge.
///
/// ⚠ **0.001, lowered from 0.004 after use** — reported as "the minimal size still seems too
/// large". On a 6246 px frame this is a 6 px radius, which is the size sensor dust actually is;
/// 0.004 was 25 px, comfortably bigger than most specks.
///
/// ⚠ A spot this small is only ~2 px across on screen at fit zoom, so [`hit_test`] takes a
/// **minimum grab radius** in addition to it. Without that, lowering this floor would make tiny
/// spots impossible to select again — trading one complaint for a worse one.
pub const MIN_RADIUS: f32 = 0.001;

/// ⚠⚠ **The floor's VALUE is guarded at COMPILE time, not by a test.** Reported from use: at
/// 0.004 the smallest patch was still too large for sensor dust. Every other assertion about the
/// floor is *relative* to it, so raising it back would leave them all green while reintroducing
/// the complaint. On a 6246 px frame 0.0015 is a 9 px radius; 0.004 was 25 px.
///
/// ⚠ A `const _` rather than a `#[test]`: clippy rightly rejects a runtime `assert!` on a
/// constant, and this is the stronger guard anyway — raising the floor becomes a build error
/// rather than a red test. The form `MID_BUDGET_BYTES` already uses.
const _: () = assert!(MIN_RADIUS > 0.0 && MIN_RADIUS <= 0.0015);
/// The largest. Beyond this a "spot" is a region, and that is the wrong tool.
///
/// ⚠ **0.05, lowered from 0.25 after use.** The working range for dust and blemishes is about
/// 0.1–0.5% of the long edge, so a ceiling at 25% spent 98% of the slider's travel on sizes
/// nobody uses and left the useful zone unusably cramped. 5% of a 6246 px frame is still a 312 px
/// radius — a very large patch.
pub const MAX_RADIUS: f32 = 0.05;

/// Which end of a spot a gesture has hold of.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Handle {
    /// The destination — the thing being removed.
    Centre,
    /// The source — where the pixels are copied from.
    Source,
}

/// Per-axis scaling that converts a normalised offset into **long-edge units**.
///
/// ⚠⚠ Normalised coordinates are anisotropic: `0.1` of the width and `0.1` of the height are
/// different distances unless the frame is square. [`Spot::radius`] is a fraction of the long
/// edge, so every comparison against it must be in that unit or a disc becomes an ellipse — the
/// trap `BrushStroke::radius` records, and which `mask_tool::hit_test` corrects for separately.
pub(crate) fn long_edge_scale(aspect: f32) -> (f32, f32) {
    if aspect >= 1.0 {
        // The width is the long edge, so a unit of normalised y is `1/aspect` of it.
        (1.0, 1.0 / aspect)
    } else {
        (aspect, 1.0)
    }
}

/// Distance between two normalised points, in long-edge units.
fn long_edge_distance(a: [f32; 2], b: [f32; 2], aspect: f32) -> f32 {
    let (sx, sy) = long_edge_scale(aspect);
    (((b[0] - a[0]) * sx).powi(2) + ((b[1] - a[1]) * sy).powi(2)).sqrt()
}

/// The spot and handle a normalised point grabs, if any.
///
/// ⚠ Searched **last-first**, so the most recently placed spot — the one drawn on top — wins.
///
/// ⚠ The destination is tested before the source within one spot: where the two discs overlap the
/// destination is what the user means, and preferring the source would make a spot whose source
/// sits on its own destination impossible to move.
pub fn hit_test(
    spots: &[Spot],
    p: [f32; 2],
    aspect: f32,
    min_grab: f32,
) -> Option<(usize, Handle)> {
    for (i, s) in spots.iter().enumerate().rev() {
        // ⚠⚠ **The grab radius is at least `min_grab`, which is the disc's radius only when that
        // is comfortably clickable.** [`MIN_RADIUS`] allows a spot ~2 px across on screen at fit
        // zoom; without a floor here it could be placed and then never selected again. The caller
        // supplies the floor because only it knows the screen scale.
        let grab = s.radius.max(min_grab);
        if long_edge_distance(s.centre, p, aspect) <= grab {
            return Some((i, Handle::Centre));
        }
        if long_edge_distance(s.source, p, aspect) <= grab {
            return Some((i, Handle::Source));
        }
    }
    None
}

/// Move one end of a spot by a normalised delta, clamped into the frame.
///
/// ⚠ Clamped because a handle dragged off the edge cannot be grabbed again — `mask_tool`'s rule.
pub fn apply_drag(s: &mut Spot, handle: Handle, dx: f32, dy: f32) {
    let end = match handle {
        Handle::Centre => &mut s.centre,
        Handle::Source => &mut s.source,
    };
    end[0] = (end[0] + dx).clamp(0.0, 1.0);
    end[1] = (end[1] + dy).clamp(0.0, 1.0);
}

/// The radius a size-drag from `from` to `to` produces, in long-edge units.
pub fn drag_radius(from: [f32; 2], to: [f32; 2], aspect: f32) -> f32 {
    long_edge_distance(from, to, aspect).clamp(MIN_RADIUS, MAX_RADIUS)
}

/// Remove the selected spot. `true` when something was removed.
///
/// ⚠ Clears the selection rather than moving it to a neighbour — `Selection::clamp`'s rule, for
/// its reason: silently selecting a different spot means the next Delete removes one the user
/// never chose.
pub fn delete_selected(spots: &mut Vec<Spot>, selected: &mut Option<usize>) -> bool {
    let Some(i) = *selected else { return false };
    *selected = None;
    if i >= spots.len() {
        return false;
    }
    spots.remove(i);
    true
}

/// The retouch tool's live settings, held on `RustroomApp`.
///
/// ⚠ Session-lifetime, not persisted — the brush's rule. Unlike the brush's screen-point size this
/// one is frame-normalised and *could* persist; it does not, so that the two tools behave alike.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpotTool {
    /// Radius as a fraction of the long edge.
    ///
    /// ⚠ Shared with the canvas size-drag: **one value, two affordances** — the eyedropper-and-
    /// Kelvin idiom M5.2 shipped. The drag writes back here, which is what makes the panel's Size
    /// slider follow the gesture instead of showing a number that no longer describes what the
    /// next click will place.
    pub radius: f32,
    pub feather: f32,
    pub mode: SpotMode,
}

impl Default for SpotTool {
    /// ⚠ Hand-written, not derived: a derived `Default` gives radius 0, which places spots that
    /// cover nothing. The default radius is generous on purpose — A's Wave 3 finding is that the
    /// radius must *exceed* the blemish, or heal measures a ring that is still lying on it.
    fn default() -> Self {
        Self {
            radius: 0.02,
            feather: 50.0,
            mode: SpotMode::Heal,
        }
    }
}

/// A radius as pixels of a full-resolution image whose long edge is `full_long`.
///
/// ⚠ A radius is *stored* as a fraction of the long edge, because that is what makes it survive a
/// resize — but nobody thinks about dust in fractions of a frame, so the UI reads in pixels.
pub fn radius_px(radius: f32, full_long: usize) -> f32 {
    radius * full_long.max(1) as f32
}

/// The inverse of [`radius_px`], clamped into the representable range.
///
/// ⚠ Clamped here rather than at the call site, so a typed-in pixel figure cannot produce a spot
/// outside the range a gesture can — two routes to one value must land in the same place.
pub fn radius_from_px(px: f32, full_long: usize) -> f32 {
    (px / full_long.max(1) as f32).clamp(MIN_RADIUS, MAX_RADIUS)
}

/// A new spot at `centre` sourcing from `source`, taking the tool's current settings.
pub fn place(centre: [f32; 2], source: [f32; 2], tool: SpotTool) -> Spot {
    Spot {
        centre,
        source,
        radius: tool.radius.clamp(MIN_RADIUS, MAX_RADIUS),
        feather: tool.feather,
        mode: tool.mode,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use edit_model::{Spot, SpotMode};

    fn spot(cx: f32, cy: f32, sx: f32, sy: f32, r: f32) -> Spot {
        Spot {
            centre: [cx, cy],
            source: [sx, sy],
            radius: r,
            feather: 50.0,
            mode: SpotMode::Heal,
        }
    }

    /// ⚠⚠ The destination wins a tie. Both discs are grabbable, and where they overlap the
    /// destination is the one the user means — it is the thing being removed. A hit-test that
    /// preferred the source would make a spot whose source sits on its own destination
    /// impossible to move.
    #[test]
    fn the_destination_wins_when_both_discs_contain_the_point() {
        let s = [spot(0.5, 0.5, 0.52, 0.5, 0.05)];
        assert_eq!(
            hit_test(&s, [0.51, 0.5], 1.0, 0.0),
            Some((0, Handle::Centre))
        );
    }

    #[test]
    fn the_source_is_grabbable_where_the_destination_is_not() {
        let s = [spot(0.2, 0.5, 0.8, 0.5, 0.05)];
        assert_eq!(
            hit_test(&s, [0.8, 0.5], 1.0, 0.0),
            Some((0, Handle::Source))
        );
        assert_eq!(
            hit_test(&s, [0.2, 0.5], 1.0, 0.0),
            Some((0, Handle::Centre))
        );
        assert_eq!(
            hit_test(&s, [0.5, 0.5], 1.0, 0.0),
            None,
            "empty space grabs nothing"
        );
    }

    /// ⚠⚠ **A spot far smaller than a click can land on must still be grabbable.**
    /// [`MIN_RADIUS`] allows ~2 px on screen at fit zoom, so without the floor a dust patch could
    /// be placed and then never selected, moved or deleted again. Reported as "the minimal size
    /// still seems too large"; lowering the floor without this would have traded one complaint
    /// for a worse one.
    #[test]
    fn a_tiny_spot_is_still_grabbable_through_the_minimum() {
        let s = [spot(0.5, 0.5, 0.2, 0.2, MIN_RADIUS)];
        let near = [0.5 + MIN_RADIUS * 4.0, 0.5];
        assert_eq!(
            hit_test(&s, near, 1.0, 0.0),
            None,
            "fixture check: without a floor this point misses"
        );
        assert_eq!(
            hit_test(&s, near, 1.0, MIN_RADIUS * 8.0),
            Some((0, Handle::Centre)),
            "the minimum grab radius was not applied"
        );
    }

    /// ⚠ The floor only ever *widens* the grab: a spot larger than it keeps its own radius, or a
    /// big patch would become impossible to click past.
    #[test]
    fn the_minimum_never_shrinks_a_large_spots_grab() {
        let s = [spot(0.5, 0.5, 0.2, 0.2, 0.10)];
        assert_eq!(
            hit_test(&s, [0.59, 0.5], 1.0, 0.001),
            Some((0, Handle::Centre)),
            "a large spot lost its own reach to the floor"
        );
    }

    /// ⚠ The later spot wins, because it is drawn on top and is what the user sees.
    #[test]
    fn the_last_spot_placed_is_the_one_grabbed() {
        let s = [
            spot(0.5, 0.5, 0.2, 0.2, 0.05),
            spot(0.5, 0.5, 0.8, 0.8, 0.05),
        ];
        assert_eq!(
            hit_test(&s, [0.5, 0.5], 1.0, 0.0),
            Some((1, Handle::Centre))
        );
    }

    /// ⚠⚠ The radius is a fraction of the **long edge**, so on a frame twice as wide as it is
    /// tall, one normalised unit of height is only half a unit of the long edge. A hit-test that
    /// compared the raw normalised distance would make every disc an ellipse to grab — the aspect
    /// trap `BrushStroke::radius` records.
    #[test]
    fn hit_testing_is_round_on_a_wide_frame() {
        let s = [spot(0.5, 0.5, 0.9, 0.9, 0.10)];
        // aspect 2.0 → the width is the long edge, so 0.09 of the width is 0.09 long-edge units.
        assert!(
            hit_test(&s, [0.59, 0.5], 2.0, 0.0).is_some(),
            "inside on the long axis"
        );
        assert!(
            hit_test(&s, [0.62, 0.5], 2.0, 0.0).is_none(),
            "outside on the long axis"
        );
        // ⚠ The discriminating probe: 0.15 of the *height* is only 0.075 long-edge units, so it
        // is INSIDE — while a naive comparison against 0.10 would call it outside.
        assert!(
            hit_test(&s, [0.5, 0.65], 2.0, 0.0).is_some(),
            "the vertical extent was measured in the wrong unit"
        );
        assert!(
            hit_test(&s, [0.5, 0.75], 2.0, 0.0).is_none(),
            "outside vertically"
        );
    }

    #[test]
    fn dragging_the_centre_moves_only_the_destination() {
        let mut s = spot(0.5, 0.5, 0.2, 0.2, 0.05);
        apply_drag(&mut s, Handle::Centre, 0.1, -0.05);
        assert!((s.centre[0] - 0.6).abs() < 1e-6 && (s.centre[1] - 0.45).abs() < 1e-6);
        assert_eq!(s.source, [0.2, 0.2], "the source moved with it");
    }

    #[test]
    fn dragging_the_source_moves_only_the_source() {
        let mut s = spot(0.5, 0.5, 0.2, 0.2, 0.05);
        apply_drag(&mut s, Handle::Source, -0.1, 0.05);
        assert!((s.source[0] - 0.1).abs() < 1e-6 && (s.source[1] - 0.25).abs() < 1e-6);
        assert_eq!(s.centre, [0.5, 0.5], "the destination moved with it");
    }

    /// ⚠ Clamped into the frame: a handle dragged off the edge cannot be grabbed again, which is
    /// the failure `mask_tool::apply_drag` clamps for.
    #[test]
    fn a_drag_cannot_push_a_handle_out_of_the_frame() {
        let mut s = spot(0.5, 0.5, 0.5, 0.5, 0.05);
        apply_drag(&mut s, Handle::Centre, 5.0, 5.0);
        assert!((0.0..=1.0).contains(&s.centre[0]) && (0.0..=1.0).contains(&s.centre[1]));
        apply_drag(&mut s, Handle::Source, -5.0, -5.0);
        assert!((0.0..=1.0).contains(&s.source[0]) && (0.0..=1.0).contains(&s.source[1]));
    }

    /// ⚠⚠ Sizing measures the drag against the **long edge**, the same unit the radius is stored
    /// in. Measuring it against the width would make the same gesture produce a different radius
    /// on a portrait frame than on a landscape one.
    #[test]
    fn drag_radius_is_the_distance_in_long_edge_units() {
        // ⚠ The probes sit inside `[MIN_RADIUS, MAX_RADIUS]`. They used to be 0.12, which was
        // fine at the old 0.25 ceiling and hits the clamp at the new 0.05 one — a fixture that
        // silently stopped testing the property it names. The property is unchanged.
        //
        // aspect 2.0 → the long edge is the width, so dx is already in long-edge units.
        let r = drag_radius([0.5, 0.5], [0.53, 0.5], 2.0);
        assert!((r - 0.03).abs() < 1e-6, "got {r}");
        // The same *visual* distance vertically is half as many long-edge units, so the drag has
        // to be twice as long in normalised y to produce the same radius.
        let r = drag_radius([0.5, 0.5], [0.5, 0.56], 2.0);
        assert!((r - 0.03).abs() < 1e-6, "got {r}");
    }

    /// ⚠ The pixel conversion round-trips, and its inverse clamps: a figure outside the
    /// representable range must land where a gesture would, not somewhere only typing can reach.
    #[test]
    fn the_pixel_conversion_round_trips_and_clamps() {
        let long = 6246;
        for r in [MIN_RADIUS, 0.002, 0.01, MAX_RADIUS] {
            let back = radius_from_px(radius_px(r, long), long);
            assert!((back - r).abs() < 1e-6, "{r} came back as {back}");
        }
        assert_eq!(radius_from_px(0.0, long), MIN_RADIUS);
        assert_eq!(radius_from_px(1e9, long), MAX_RADIUS);
        // ⚠ A degenerate long edge must not divide by zero — the panel can be drawn before a
        // photo is open.
        assert!(radius_from_px(10.0, 0).is_finite());
        assert!(radius_px(0.01, 0).is_finite());
    }

    /// ⚠ A radius below the floor is a spot that covers nothing and cannot be seen or grabbed —
    /// which is what a click with a shaky hand would otherwise place.
    #[test]
    fn the_radius_has_a_floor_and_a_ceiling() {
        assert_eq!(drag_radius([0.5, 0.5], [0.5, 0.5], 1.0), MIN_RADIUS);
        assert_eq!(drag_radius([0.5, 0.5], [9.0, 9.0], 1.0), MAX_RADIUS);
    }

    /// ⚠ Deleting clears the selection rather than moving it to a neighbour — `Selection::clamp`'s
    /// rule: silently selecting a *different* spot means the next Delete removes something the
    /// user never chose.
    #[test]
    fn deleting_clears_the_selection() {
        let mut spots = vec![
            spot(0.2, 0.2, 0.4, 0.4, 0.02),
            spot(0.6, 0.6, 0.8, 0.8, 0.02),
        ];
        let mut sel = Some(0);
        assert!(delete_selected(&mut spots, &mut sel));
        assert_eq!(spots.len(), 1);
        assert_eq!(sel, None, "a neighbour was selected");
        assert!(
            !delete_selected(&mut spots, &mut None),
            "nothing selected, nothing deleted"
        );
    }

    /// ⚠ An out-of-range index must not panic: a snapshot restore can leave one behind.
    #[test]
    fn deleting_a_stale_index_is_a_no_op() {
        let mut spots = vec![spot(0.2, 0.2, 0.4, 0.4, 0.02)];
        let mut sel = Some(9);
        assert!(!delete_selected(&mut spots, &mut sel));
        assert_eq!(spots.len(), 1);
        assert_eq!(sel, None, "the stale index was left in place");
    }

    /// ⚠ The default radius is generous on purpose: A's Wave 3 finding is that the radius must
    /// **exceed** the blemish, or heal measures a ring that is still lying on it.
    #[test]
    fn the_default_tool_is_usable_rather_than_zero() {
        let t = SpotTool::default();
        assert!(t.radius >= MIN_RADIUS * 4.0, "got {}", t.radius);
        assert_eq!(t.mode, SpotMode::Heal, "Heal is the useful default");
    }
}
