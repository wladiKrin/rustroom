//! The Map view's location field (M10.18b): coordinates typed directly, or a place looked up on
//! OpenStreetMap's Nominatim. No egui, no network — the request is the caller's.
//!
//! ⚠⚠ Nominatim's policy: at most one request a second, a real User-Agent, and **no search-as-you-type**
//! on the public server — the field searches only on Enter, and [`Throttle`] holds the rate.

use std::time::{Duration, Instant};

/// Who Rustroom says it is to OpenStreetMap's servers — their usage policies require one.
pub const USER_AGENT: &str = "Rustroom/0.1 (+https://github.com/wladiKrin/rustroom)";

/// One match.
#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    pub name: String,
    pub at: (f64, f64),
}

/// The search request for `q`.
pub fn query_url(q: &str) -> String {
    let mut enc = String::new();
    for b in q.trim().bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                enc.push(b as char)
            }
            b' ' => enc.push('+'),
            _ => enc.push_str(&format!("%{b:02X}")),
        }
    }
    format!("https://nominatim.openstreetmap.org/search?format=jsonv2&limit=5&q={enc}")
}

/// The matches in a Nominatim reply. ⚠ Its `lat` and `lon` are STRINGS (captured 2026-10-28); a row
/// missing either, or out of range, is left out.
pub fn parse(json: &str) -> Vec<Place> {
    #[derive(serde::Deserialize)]
    struct Row {
        display_name: Option<String>,
        lat: Option<String>,
        lon: Option<String>,
    }
    let Ok(rows) = serde_json::from_str::<Vec<Row>>(json) else {
        return Vec::new();
    };
    rows.into_iter()
        .filter_map(|r| {
            let la: f64 = r.lat?.parse().ok()?;
            let lo: f64 = r.lon?.parse().ok()?;
            in_range(la, lo).then(|| Place {
                name: r.display_name.unwrap_or_default(),
                at: (la, lo),
            })
        })
        .collect()
}

fn in_range(la: f64, lo: f64) -> bool {
    la.is_finite() && lo.is_finite() && la.abs() <= 90.0 && lo.abs() <= 180.0
}

/// `"38.80, -9.38"` or `"38.80 -9.38"` as a position — `None` for anything else, a place name included.
pub fn parse_coordinates(q: &str) -> Option<(f64, f64)> {
    let parts: Vec<&str> = q
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .collect();
    let [a, b] = parts.as_slice() else {
        return None;
    };
    let (la, lo) = (a.parse::<f64>().ok()?, b.parse::<f64>().ok()?);
    in_range(la, lo).then_some((la, lo))
}

/// At most one request a second.
#[derive(Default)]
pub struct Throttle {
    last: Option<Instant>,
}

impl Throttle {
    /// Whether a request may go out at `now`; a yes is recorded as the last one.
    pub fn may_send(&mut self, now: Instant) -> bool {
        let ok = self
            .last
            .is_none_or(|t| now.saturating_duration_since(t) >= Duration::from_secs(1));
        if ok {
            self.last = Some(now);
        }
        ok
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real reply, captured — `tests/fixtures/nominatim_sintra.md` says how.
    #[test]
    fn parse_reads_the_captured_reply() {
        let json = include_str!("../tests/fixtures/nominatim_sintra.json");
        let p = parse(json);
        assert_eq!(p.len(), 3, "{p:?}");
        assert_eq!(p[0].name, "Sintra, Lisboa, Portugal");
        assert!((p[0].at.0 - 38.8355446).abs() < 1e-9 && (p[0].at.1 + 9.3522371).abs() < 1e-9);
        assert!(parse("not json").is_empty());
    }

    #[test]
    fn the_query_is_encoded() {
        assert_eq!(
            query_url(" São Paulo & co "),
            "https://nominatim.openstreetmap.org/search?format=jsonv2&limit=5&q=S%C3%A3o+Paulo+%26+co"
        );
    }

    #[test]
    fn coordinates_parse_and_range_check() {
        assert_eq!(parse_coordinates("38.80, -9.38"), Some((38.80, -9.38)));
        assert_eq!(parse_coordinates("38.80 -9.38"), Some((38.80, -9.38)));
        assert_eq!(parse_coordinates("91, 0"), None);
        assert_eq!(parse_coordinates("0, 181"), None);
        assert_eq!(parse_coordinates("Sintra"), None);
        assert_eq!(parse_coordinates("1, 2, 3"), None);
    }

    #[test]
    fn the_throttle_allows_one_a_second() {
        let mut t = Throttle::default();
        let t0 = Instant::now();
        assert!(t.may_send(t0));
        assert!(!t.may_send(t0 + Duration::from_millis(999)));
        assert!(t.may_send(t0 + Duration::from_millis(1000)));
    }
}
