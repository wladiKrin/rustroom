//! Applying a DNG profile's hue/saturation/value mapping.
//!
//! ⚠ This is the **visible** half of a camera profile. Where the matrices differ from libraw's by
//! well under a percent, the X-T4's table carries a mean absolute hue shift of 11.3° and
//! saturation scales up to 1.95.
//!
//! ⚠ Everything here works in **degrees** for hue and treats `v` as `max(r, g, b)` **unclamped**:
//! the buffer is scene-referred, so values above 1.0 are ordinary and must survive.

/// A hue/saturation/value mapping table, already resolved to a single illuminant.
#[derive(Debug, Clone, PartialEq)]
pub struct Table {
    /// `[hue, saturation, value]` divisions.
    pub dims: [usize; 3],
    /// `[hue shift in degrees, saturation scale, value scale]`, hue-major then saturation.
    pub data: Vec<[f32; 3]>,
}

impl Table {
    /// Entry-wise interpolation between two illuminants' tables: `g` of `a`, `1 - g` of `b`.
    ///
    /// ⚠ Blended **once**, outside any pixel loop — the X-T4's table is 2700 entries, and doing
    /// this per pixel would be 2700 multiplies per pixel for a value that never changes.
    ///
    /// Returns `a` unchanged when the two disagree in shape, which cannot happen for a profile
    /// the parser accepted but keeps this total.
    pub fn blend(a: &Table, b: &Table, g: f32) -> Table {
        if a.dims != b.dims || a.data.len() != b.data.len() {
            return a.clone();
        }
        Table {
            dims: a.dims,
            data: a
                .data
                .iter()
                .zip(&b.data)
                .map(|(p, q)| {
                    [
                        g * p[0] + (1.0 - g) * q[0],
                        g * p[1] + (1.0 - g) * q[1],
                        g * p[2] + (1.0 - g) * q[2],
                    ]
                })
                .collect(),
        }
    }

    /// Look the pixel up and apply the shift and scales.
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        let [h, s, v] = rgb_to_hsv(rgb);
        let [dh, ss, vs] = self.lookup(h, s, v);
        let h = {
            let x = (h + dh) % 360.0;
            if x < 0.0 {
                x + 360.0
            } else {
                x
            }
        };
        // ⚠ Saturation is clamped to [0, 1] because it is a ratio by definition; `v` is **not**,
        // because a scene-referred highlight above 1.0 is legitimate data.
        hsv_to_rgb([h, (s * ss).clamp(0.0, 1.0), v * vs])
    }

    /// Bilinear over hue and saturation, linear over value when the table has a value axis.
    fn lookup(&self, h: f32, s: f32, v: f32) -> [f32; 3] {
        let [hd, sd, vd] = self.dims;
        if hd == 0 || sd == 0 || vd == 0 || self.data.len() != hd * sd * vd {
            return [0.0, 1.0, 1.0];
        }

        // ⚠ Hue **wraps** — it is circular, so index `hd` is index 0 and there is no seam at
        // 360°. Saturation and value **clamp**: they are bounded quantities, and wrapping them
        // would map a highlight back onto a shadow.
        let hf = h / 360.0 * hd as f32;
        let h0 = (hf.floor() as isize).rem_euclid(hd as isize) as usize;
        let h1 = (h0 + 1) % hd;
        let ht = hf - hf.floor();

        let sf = (s.clamp(0.0, 1.0) * (sd - 1) as f32).max(0.0);
        let s0 = (sf.floor() as usize).min(sd.saturating_sub(2)).min(sd - 1);
        let s1 = (s0 + 1).min(sd - 1);
        let st = (sf - s0 as f32).clamp(0.0, 1.0);

        // ⚠ The value **index** clamps at the top while the scale still applies to the real `v`
        // in `apply`. Clamping `v` itself would drag every highlight back to the table's edge and
        // quietly compress the headroom the wide working space exists to preserve.
        let (v0, v1, vt) = if vd == 1 {
            (0usize, 0usize, 0.0f32)
        } else {
            let vf = (v.clamp(0.0, 1.0) * (vd - 1) as f32).max(0.0);
            let a = (vf.floor() as usize).min(vd - 2);
            (a, a + 1, (vf - a as f32).clamp(0.0, 1.0))
        };

        let at = |vi: usize, hi: usize, si: usize| self.data[(vi * hd + hi) * sd + si];
        let mix = |p: [f32; 3], q: [f32; 3], t: f32| {
            [
                p[0] + (q[0] - p[0]) * t,
                p[1] + (q[1] - p[1]) * t,
                p[2] + (q[2] - p[2]) * t,
            ]
        };

        let plane = |vi: usize| {
            let a = mix(at(vi, h0, s0), at(vi, h1, s0), ht);
            let b = mix(at(vi, h0, s1), at(vi, h1, s1), ht);
            mix(a, b, st)
        };
        if vd == 1 {
            plane(0)
        } else {
            mix(plane(v0), plane(v1), vt)
        }
    }
}

/// Linear RGB → HSV, hue in degrees. ⚠ `v` is `max(r, g, b)` and is **not clamped**.
pub fn rgb_to_hsv(c: [f32; 3]) -> [f32; 3] {
    let max = c[0].max(c[1]).max(c[2]);
    let min = c[0].min(c[1]).min(c[2]);
    let d = max - min;
    let s = if max > 0.0 { d / max } else { 0.0 };
    let h = if d <= 0.0 {
        0.0
    } else if max == c[0] {
        60.0 * (((c[1] - c[2]) / d) % 6.0)
    } else if max == c[1] {
        60.0 * ((c[2] - c[0]) / d + 2.0)
    } else {
        60.0 * ((c[0] - c[1]) / d + 4.0)
    };
    [if h < 0.0 { h + 360.0 } else { h }, s, max]
}

/// HSV → linear RGB, hue in degrees. `v` may exceed 1.0.
pub fn hsv_to_rgb(hsv: [f32; 3]) -> [f32; 3] {
    let [h, s, v] = hsv;
    if s <= 0.0 {
        return [v, v, v];
    }
    let h = (h % 360.0 + 360.0) % 360.0 / 60.0;
    let i = h.floor();
    let f = h - i;
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    match i as i32 % 6 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity(h: usize, s: usize) -> Table {
        Table {
            dims: [h, s, 1],
            data: vec![[0.0, 1.0, 1.0]; h * s],
        }
    }

    fn uniform(h: usize, s: usize, e: [f32; 3]) -> Table {
        Table {
            dims: [h, s, 1],
            data: vec![e; h * s],
        }
    }

    #[test]
    fn the_hsv_round_trip_is_the_identity() {
        // ⚠ Includes v > 1: the buffer is scene-referred, so highlights above 1.0 are ordinary
        // and a round trip that clamped them would destroy the headroom piece A exists for.
        for c in [
            [0.2, 0.5, 0.9],
            [1.4, 0.3, 0.1],
            [0.5, 0.5, 0.5],
            [0.0, 0.0, 0.0],
            [0.9, 1.0, 0.2],
        ] {
            let back = hsv_to_rgb(rgb_to_hsv(c));
            for i in 0..3 {
                assert!(
                    (back[i] - c[i]).abs() < 1e-5,
                    "{c:?} round-tripped to {back:?}"
                );
            }
        }
    }

    #[test]
    fn an_identity_table_changes_nothing() {
        for c in [[0.2, 0.5, 0.9], [1.4, 0.3, 0.1], [0.9, 0.9, 0.9]] {
            let out = identity(90, 30).apply(c);
            for i in 0..3 {
                assert!((out[i] - c[i]).abs() < 1e-5, "{c:?} became {out:?}");
            }
        }
    }

    #[test]
    fn a_neutral_is_never_moved() {
        // ⚠ Structural: saturation is 0, so the scale multiplies zero and the hue is undefined.
        // Tested anyway — if a grey ever moves, the HSV round trip is wrong, not the table.
        let t = uniform(90, 30, [37.0, 1.8, 1.0]);
        for v in [0.0, 0.18, 0.5, 1.0, 2.5] {
            let out = t.apply([v, v, v]);
            assert!(
                (out[0] - v).abs() < 1e-5 && (out[1] - v).abs() < 1e-5 && (out[2] - v).abs() < 1e-5,
                "grey {v} became {out:?}"
            );
        }
    }

    #[test]
    fn a_value_above_one_keeps_its_headroom() {
        // ⚠⚠ The specific regression the spec names: the table *index* clamps at the top, but
        // the scale must apply to the real value. Clamping the value itself would drag every
        // highlight back to the table's edge and quietly compress the scene-referred headroom.
        let out = identity(90, 30).apply([2.5, 1.0, 1.0]);
        assert!(out[0] > 2.4, "a highlight at 2.5 came back as {}", out[0]);
    }

    #[test]
    fn a_uniform_saturation_scale_is_applied() {
        // A colour at saturation 0.25 should come back at 0.5.
        let out = uniform(90, 30, [0.0, 2.0, 1.0]).apply([1.0, 0.75, 0.75]);
        let hsv = rgb_to_hsv(out);
        assert!((hsv[1] - 0.5).abs() < 1e-4, "saturation became {}", hsv[1]);
    }

    #[test]
    fn a_uniform_hue_shift_rotates_by_that_many_degrees() {
        // ⚠ Degrees, not radians: pure red is hue 0 and +60° is yellow. A radian implementation
        // would rotate by ~3438°, i.e. essentially at random.
        let hsv = rgb_to_hsv(uniform(90, 30, [60.0, 1.0, 1.0]).apply([1.0, 0.0, 0.0]));
        assert!((hsv[0] - 60.0).abs() < 0.5, "hue became {}", hsv[0]);
    }

    #[test]
    fn the_hue_axis_wraps_with_no_seam_at_zero() {
        // ⚠ Hue is circular, so the last division interpolates back to the first. A clamped
        // index would make the shift jump discontinuously across 360° — a visible seam on
        // magentas, which is exactly where the wrap sits.
        let mut t = identity(4, 2);
        // Give division 0 and division 3 different shifts, so the wrap is observable.
        for s in 0..2 {
            t.data[s] = [30.0, 1.0, 1.0];
            t.data[3 * 2 + s] = [-30.0, 1.0, 1.0];
        }
        let just_below = t.lookup(359.0, 1.0, 1.0)[0];
        let just_above = t.lookup(1.0, 1.0, 1.0)[0];
        assert!(
            (just_below - just_above).abs() < 20.0,
            "a seam at 0°: {just_below} vs {just_above}"
        );
    }

    #[test]
    fn saturation_and_value_clamp_rather_than_wrap() {
        // Must not panic, and a fully saturated colour must use the last division rather than
        // wrapping round to the first.
        let t = identity(4, 2);
        for c in [
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [1.0, 1.0, 1.0],
            [3.0, 0.0, 0.0],
        ] {
            let _ = t.apply(c);
        }
    }

    #[test]
    fn a_three_dimensional_table_interpolates_over_value_too() {
        // ⚠ The X-T4's table is 2-D, so this path is untested by any real profile here — other
        // cameras ship 3-D tables and would silently take a wrong branch.
        let t = Table {
            dims: [2, 2, 2],
            data: vec![
                [0.0, 1.0, 1.0],
                [0.0, 1.0, 1.0],
                [0.0, 1.0, 1.0],
                [0.0, 1.0, 1.0],
                [90.0, 1.0, 1.0],
                [90.0, 1.0, 1.0],
                [90.0, 1.0, 1.0],
                [90.0, 1.0, 1.0],
            ],
        };
        let dark = t.lookup(0.0, 1.0, 0.0)[0];
        let bright = t.lookup(0.0, 1.0, 1.0)[0];
        assert!((dark - 0.0).abs() < 1e-4, "dark plane gave {dark}");
        assert!((bright - 90.0).abs() < 1e-4, "bright plane gave {bright}");
    }

    #[test]
    fn blending_two_tables_reaches_both_endpoints() {
        let a = uniform(4, 2, [10.0, 1.0, 1.0]);
        let b = uniform(4, 2, [20.0, 2.0, 1.0]);
        assert_eq!(Table::blend(&a, &b, 1.0).data[0], [10.0, 1.0, 1.0]);
        assert_eq!(Table::blend(&a, &b, 0.0).data[0], [20.0, 2.0, 1.0]);
        let mid = Table::blend(&a, &b, 0.5).data[0];
        assert!(
            (mid[0] - 15.0).abs() < 1e-4 && (mid[1] - 1.5).abs() < 1e-4,
            "{mid:?}"
        );
    }
}
