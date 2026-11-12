//! A chromaticity's correlated colour temperature by **Robertson's method** (1968) — what Adobe's DNG SDK uses
//! (`dng_temperature`), so a camera profile's two calibrations are blended on the same temperature it computes
//! (M10.13a).
//!
//! ⚠⚠ Replaced McCamy's cubic, which drifts far from the Planckian locus: a green-tinted neutral read 3369 K where
//! the SDK reads 4403 K, a 23 % error that moved the profile's matrix by 0.03.
//!
//! The table is Robertson's published isotemperature lines (Wyszecki & Stiles): reciprocal megakelvin, the line's
//! point on the locus in CIE 1960 (u, v), and its slope.

/// `(mired, u, v, slope)`.
const LINES: [(f64, f64, f64, f64); 31] = [
    (0.0, 0.18006, 0.26352, -0.24341),
    (10.0, 0.18066, 0.26589, -0.25479),
    (20.0, 0.18133, 0.26846, -0.26876),
    (30.0, 0.18208, 0.27119, -0.28539),
    (40.0, 0.18293, 0.27407, -0.30470),
    (50.0, 0.18388, 0.27709, -0.32675),
    (60.0, 0.18494, 0.28021, -0.35156),
    (70.0, 0.18611, 0.28342, -0.37915),
    (80.0, 0.18740, 0.28668, -0.40955),
    (90.0, 0.18880, 0.28997, -0.44278),
    (100.0, 0.19032, 0.29326, -0.47888),
    (125.0, 0.19462, 0.30141, -0.58204),
    (150.0, 0.19962, 0.30921, -0.70471),
    (175.0, 0.20525, 0.31647, -0.84901),
    (200.0, 0.21142, 0.32312, -1.0182),
    (225.0, 0.21807, 0.32909, -1.2168),
    (250.0, 0.22511, 0.33439, -1.4512),
    (275.0, 0.23247, 0.33904, -1.7298),
    (300.0, 0.24010, 0.34308, -2.0637),
    (325.0, 0.24702, 0.34655, -2.4681),
    (350.0, 0.25591, 0.34951, -2.9641),
    (375.0, 0.26400, 0.35200, -3.5814),
    (400.0, 0.27218, 0.35407, -4.3633),
    (425.0, 0.28039, 0.35577, -5.3762),
    (450.0, 0.28863, 0.35714, -6.7262),
    (475.0, 0.29685, 0.35823, -8.5955),
    (500.0, 0.30505, 0.35907, -11.324),
    (525.0, 0.31320, 0.35968, -15.628),
    (550.0, 0.32129, 0.36011, -23.325),
    (575.0, 0.32931, 0.36038, -40.770),
    (600.0, 0.33724, 0.36051, -116.45),
];

/// The correlated colour temperature of chromaticity `(x, y)`, in kelvin — found between the two isotemperature
/// lines the point lies between, interpolated by its distance to each. Past the table's ends the end lines' answer.
pub fn temperature(x: f64, y: f64) -> f64 {
    let d = 1.5 - x + 6.0 * y;
    let (u, v) = (2.0 * x / d, 3.0 * y / d);
    let mut last_dt = 0.0;
    for i in 1..LINES.len() {
        let (_, lu, lv, slope) = LINES[i];
        let len = (1.0 + slope * slope).sqrt();
        let (du, dv) = (1.0 / len, slope / len);
        // Signed distance of the point from line `i`; it changes sign between the two lines that bracket it.
        let dt = -(u - lu) * dv + (v - lv) * du;
        if dt <= 0.0 || i == LINES.len() - 1 {
            let dt = -dt.min(0.0);
            let f = if i == 1 { 0.0 } else { dt / (last_dt + dt) };
            return 1.0e6 / (LINES[i - 1].0 * f + LINES[i].0 * (1.0 - f));
        }
        last_dt = dt;
    }
    unreachable!("the loop returns on its last line")
}

#[cfg(test)]
mod tests {
    use super::temperature;

    /// Points ON the locus read their own temperature: D65's chromaticity is 6504 K, Standard A's 2856 K.
    #[test]
    fn the_standard_illuminants_read_their_own_temperatures() {
        let d65 = temperature(0.31271, 0.32902);
        assert!((d65 - 6504.0).abs() < 15.0, "{d65}");
        let a = temperature(0.44757, 0.40745);
        assert!((a - 2856.0).abs() < 5.0, "{a}");
        let d50 = temperature(0.34567, 0.35850);
        assert!((d50 - 5003.0).abs() < 10.0, "{d50}");
    }
}
