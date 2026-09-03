//! The straighten angle a drawn line asks for — pure maths, no egui, no canvas.
//!
//! ⚠ Lives in the lib because this is a unit-testable rule: the `crop_tool` / `mask_tool` /
//! `panel_order` pattern. The gesture that produces the two points is in the bin module.

/// Below this, in normalised frame units, a drawn line is treated as a tap and asks for nothing.
pub const MIN_LEN: f32 = 0.01;

/// The straighten angle a line drawn from `from` to `to` asks for, in degrees.
///
/// `from` and `to` are in `[0, 1]` frame coordinates; `aspect` is width over height.
/// `None` for a line too short to have a direction.
///
/// ⚠⚠ **Measured in SCREEN space, aspect-corrected.** Normalised frame space is anisotropic —
/// `x` spans the width and `y` the height — so an `atan2` on the raw normalised delta gives an
/// angle nobody drew. This is the fourth appearance of that trap in this codebase:
/// `LinearGradient::coverage`, `mask_tool::hit_test` and `guide_overlay` each had to handle it,
/// while `RadialGradient::coverage` deliberately needs **no** correction for a reason specific to
/// axis-aligned ellipses. Do not assume it cancels here — on a 3:2 frame a normalised 45° line is
/// at 33.69° on screen.
///
/// ⚠⚠ **A line has an orientation, not a direction.** The same line drawn right-to-left must give
/// the same answer, so both directions are folded into one half-plane before measuring. Without
/// that, `guides::solve`'s `mean_angle` records the failure: two lines traced opposite ways
/// "average to roughly a right angle — garbage."
///
/// ⚠ The family is inferred from the corrected delta, reusing
/// `pipeline::perspective::guides::Guide::family`'s rule (`|dy| >= |dx|` is a vertical, with an
/// exact 45° landing in `Vertical`): a near-horizontal line is levelled to 0°, a near-vertical one
/// squared to 90°. That is what Lightroom's straighten tool does, and it means you draw along
/// whatever is meant to be straight without telling the app which it is.
///
/// ⚠ The sign convention is `GeometryStage`'s, via `guides::to_frame`: a **positive** angle turns
/// the content clockwise on screen (frame coordinates have `y` pointing down). So a horizon
/// falling to the right asks for a negative angle, and a plumb line leaning right-as-it-descends
/// asks for a positive one.
pub fn angle_for_line(from: [f32; 2], to: [f32; 2], aspect: f32) -> Option<f32> {
    // ⚠ Guarded, not trusted: a degenerate aspect would make every angle NaN, and a NaN angle
    // compares false against every clamp and would reach `Edits` unchanged.
    if !aspect.is_finite() || aspect <= 0.0 {
        return None;
    }
    let (dxn, dyn_) = (to[0] - from[0], to[1] - from[1]);
    if (dxn * dxn + dyn_ * dyn_).sqrt() < MIN_LEN {
        return None;
    }
    // → screen space.
    let (mut dx, mut dy) = (dxn * aspect, dyn_);
    // One half-plane. The axis folded on is the family's own, so the fold cannot flip the family.
    let vertical = dy.abs() >= dx.abs();
    if (vertical && dy < 0.0) || (!vertical && dx < 0.0) {
        dx = -dx;
        dy = -dy;
    }
    // The rotation that takes this direction to its family's axis.
    let angle = if vertical {
        // Angle from vertical, positive clockwise; zeroing it needs exactly that rotation.
        dx.atan2(dy).to_degrees()
    } else {
        // Angle from horizontal; levelling needs its negation.
        -dy.atan2(dx).to_degrees()
    };
    if !angle.is_finite() {
        return None;
    }
    Some(angle.clamp(-45.0, 45.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 3:2 frame, so the aspect correction has something to do.
    const A: f32 = 1.5;

    /// A line already level asks for nothing.
    #[test]
    fn a_level_line_asks_for_no_rotation() {
        let a = angle_for_line([0.2, 0.5], [0.8, 0.5], A).expect("a usable line");
        assert!(a.abs() < 1e-4, "expected ~0, got {a}");
    }

    /// A line already plumb asks for nothing — the vertical family, squared to 90°.
    #[test]
    fn a_plumb_line_asks_for_no_rotation() {
        let a = angle_for_line([0.5, 0.2], [0.5, 0.8], A).expect("a usable line");
        assert!(a.abs() < 1e-4, "expected ~0, got {a}");
    }

    /// ⚠⚠ **The aspect correction, and the test has LEVERAGE.** On a 3:2 frame a line whose
    /// normalised delta is `(0.4, 0.4)` is at 45° in *normalised* space and at
    /// `atan2(0.4, 0.4 * 1.5) = 33.69°` on *screen*. Those two answers differ by 11.3°, far
    /// outside any tolerance, so this test can actually see the correction being dropped.
    ///
    /// ⚠ It is deliberately **diagonal**. M3.1a records the sibling failure:
    /// `hit_testing_the_line_is_aspect_correct` "used a horizontal axis with the probe directly
    /// below it, where the perpendicular distance is purely vertical — it passed with the aspect
    /// correction deleted." An axis-aligned line here has a zero component, so scaling it changes
    /// nothing and the test is blind.
    #[test]
    fn the_angle_is_measured_in_screen_space_not_normalised_space() {
        let a = angle_for_line([0.3, 0.3], [0.7, 0.7], A).expect("a usable line");
        // A horizontal family: level it, so the rotation is minus the screen angle.
        assert!(
            (a + 33.690_1).abs() < 0.01,
            "expected the screen angle -33.69, got {a} (45.0 means the aspect was ignored)"
        );
    }

    /// ⚠⚠ **A line has an orientation, not a direction.** `guides.rs`'s `mean_angle` records what
    /// happens without folding both into one half-plane: "two guides traced in opposite
    /// directions average to roughly a right angle — garbage."
    #[test]
    fn drawing_the_same_line_backwards_gives_the_same_angle() {
        let f = angle_for_line([0.2, 0.4], [0.8, 0.6], A).expect("a usable line");
        let b = angle_for_line([0.8, 0.6], [0.2, 0.4], A).expect("a usable line");
        assert!((f - b).abs() < 1e-4, "forwards {f}, backwards {b}");
    }

    /// The family is inferred from the aspect-corrected delta, `Guide::family`'s rule — so a
    /// steep line is squared to plumb rather than dragged 80° to level.
    #[test]
    fn a_steep_line_is_squared_to_plumb_not_levelled() {
        // Screen delta (0.1 * 1.5, 0.6) — clearly vertical.
        let a = angle_for_line([0.45, 0.2], [0.55, 0.8], A).expect("a usable line");
        assert!(
            a.abs() < 20.0,
            "a vertical-family line should ask for a small rotation, got {a}"
        );
    }

    /// A tap, or a line with no length, must not rotate the photograph by an arbitrary amount.
    /// `guide_overlay.rs` drops a zero-length guide on release for the same reason.
    #[test]
    fn a_degenerate_line_asks_for_nothing() {
        assert_eq!(angle_for_line([0.5, 0.5], [0.5, 0.5], A), None);
        assert_eq!(angle_for_line([0.5, 0.5], [0.502, 0.501], A), None);
    }

    /// The slider's own range.
    ///
    /// ⚠⚠ **Reported as INERT, not as a caught sabotage.** The family rule makes the clamp
    /// unreachable *by construction*: a horizontal-family line has `|dy| < |dx|`, so its screen
    /// angle is strictly inside ±45, and a vertical-family one has `|dx| <= |dy|`, so its angle
    /// from vertical is in ±45. There is therefore **no input that can reach the clamp**, and
    /// deleting it fails nothing. It is kept as defence in depth — a future change to the family
    /// rule would otherwise write an out-of-range angle into `Edits` silently — and this test
    /// asserts the *range property* the function promises rather than pretending to cover the
    /// clamp. CLAUDE.md's handling of A's degenerate-radius guard, recorded rather than papered
    /// over with an invented fixture.
    #[test]
    fn the_angle_is_clamped_to_the_sliders_range() {
        for (from, to) in [
            ([0.1, 0.9], [0.9, 0.1]),
            ([0.1, 0.1], [0.9, 0.9]),
            ([0.5, 0.1], [0.51, 0.9]),
        ] {
            let a = angle_for_line(from, to, A).expect("a usable line");
            assert!((-45.0..=45.0).contains(&a), "out of range: {a}");
        }
    }
}
