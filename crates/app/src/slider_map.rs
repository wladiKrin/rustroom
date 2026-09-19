//! The pure half of the panel slider: where a value sits on its rail, what the accent fill
//! spans, and where the zero tick goes.
//!
//! ⚠ No egui here — this is what the tests reach for the geometry, and the bin widget only
//! paints what these return. A rail position `t` is `0.0` at the range's start and `1.0` at
//! its end.

/// Whether a range has zero strictly inside it — the bipolar case, where the fill runs from
/// zero rather than from the left edge.
pub fn straddles_zero(min: f32, max: f32) -> bool {
    min < 0.0 && max > 0.0
}

/// Whether a log mapping is usable: both ends strictly positive.
fn log_ok(min: f32, max: f32) -> bool {
    min > 0.0 && max > 0.0 && min != max
}

/// The rail position of `value`, clamped to `0.0..=1.0`.
pub fn t_of(value: f32, min: f32, max: f32, log: bool) -> f32 {
    if min == max {
        return 0.0;
    }
    let t = if log && log_ok(min, max) {
        let v = value.max(min.min(max));
        (v.ln() - min.ln()) / (max.ln() - min.ln())
    } else {
        (value - min) / (max - min)
    };
    t.clamp(0.0, 1.0)
}

/// The value at rail position `t`, clamped to the range.
pub fn value_of(t: f32, min: f32, max: f32, log: bool) -> f32 {
    let t = t.clamp(0.0, 1.0);
    let v = if log && log_ok(min, max) {
        (min.ln() + t * (max.ln() - min.ln())).exp()
    } else {
        min + t * (max - min)
    };
    v.clamp(min.min(max), min.max(max))
}

/// Where zero sits on a bipolar rail, `None` for a unipolar range.
pub fn zero_t(min: f32, max: f32) -> Option<f32> {
    straddles_zero(min, max).then(|| t_of(0.0, min, max, false))
}

/// The span of the accent fill: from zero to the handle on a bipolar rail, from the left
/// edge to the handle otherwise. Always `(lo, hi)` with `lo <= hi`.
pub fn fill_span(t: f32, min: f32, max: f32) -> (f32, f32) {
    match zero_t(min, max) {
        Some(z) => (t.min(z), t.max(z)),
        None => (0.0, t),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_linear_rail_round_trips_at_both_ends_and_the_middle() {
        for (t, want) in [(0.0, -100.0), (0.5, 0.0), (1.0, 100.0)] {
            assert_eq!(value_of(t, -100.0, 100.0, false), want);
            assert_eq!(t_of(want, -100.0, 100.0, false), t);
        }
    }

    #[test]
    fn a_log_rail_puts_the_geometric_middle_at_the_centre() {
        // 1..100: the centre is 10, not 50.5 — the whole point of the log mapping for a
        // radius whose useful zone sits at the bottom of a 50x range.
        assert!((value_of(0.5, 1.0, 100.0, true) - 10.0).abs() < 1e-4);
        assert!((t_of(10.0, 1.0, 100.0, true) - 0.5).abs() < 1e-6);
        assert_eq!(value_of(0.0, 1.0, 100.0, true), 1.0);
        assert_eq!(value_of(1.0, 1.0, 100.0, true), 100.0);
    }

    #[test]
    fn a_log_rail_with_a_non_positive_end_falls_back_to_linear() {
        // ln(0) is -inf; a caller asking for log on 0..100 must get something usable.
        assert_eq!(value_of(0.5, 0.0, 100.0, true), 50.0);
    }

    #[test]
    fn positions_are_clamped_to_the_rail() {
        assert_eq!(t_of(500.0, -100.0, 100.0, false), 1.0);
        assert_eq!(t_of(-500.0, -100.0, 100.0, false), 0.0);
        assert_eq!(value_of(2.0, 0.0, 10.0, false), 10.0);
    }

    #[test]
    fn the_fill_runs_from_zero_on_a_bipolar_rail_and_from_the_left_otherwise() {
        // ⚠ Probed at 0.3 and 0.7, never only at 0.5 where a wrong branch is inert.
        assert_eq!(fill_span(0.3, -100.0, 100.0), (0.3, 0.5));
        assert_eq!(fill_span(0.7, -100.0, 100.0), (0.5, 0.7));
        assert_eq!(fill_span(0.3, 0.0, 100.0), (0.0, 0.3));
        assert_eq!(fill_span(0.7, 0.0, 100.0), (0.0, 0.7));
    }

    #[test]
    fn the_zero_tick_follows_an_asymmetric_range() {
        // -5..15: zero is a quarter of the way along, not in the middle.
        assert_eq!(zero_t(-5.0, 15.0), Some(0.25));
        assert_eq!(zero_t(0.0, 100.0), None);
        assert_eq!(zero_t(0.5, 3.0), None);
    }
}
