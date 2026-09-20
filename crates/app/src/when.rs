//! A snapshot's time as a short phrase: `15:41` today, `yesterday 15:41`, else `Sep 19 15:41`.
//!
//! ⚠ Both arguments are **local wall-clock seconds** since the epoch, the convention
//! `Snapshot::taken` and `Capture::taken` store — so the comparison needs no timezone, and
//! the caller passes "now" in the same convention rather than `Utc::now()`.

/// The short phrase for `taken`, judged against `now`.
pub fn short(taken: i64, now: i64) -> String {
    let Some(t) = chrono::DateTime::from_timestamp(taken, 0) else {
        return "—".to_string();
    };
    let Some(n) = chrono::DateTime::from_timestamp(now, 0) else {
        return t.format("%b %-d %H:%M").to_string();
    };
    let days = n
        .date_naive()
        .signed_duration_since(t.date_naive())
        .num_days();
    match days {
        0 => t.format("%H:%M").to_string(),
        1 => t.format("yesterday %H:%M").to_string(),
        _ => t.format("%b %-d %H:%M").to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400;

    #[test]
    fn today_yesterday_and_a_date() {
        // 2026-09-20 15:41:07 as wall-clock seconds.
        let taken = 1_789_918_867;
        assert_eq!(short(taken, taken + 3_600), "15:41");
        assert_eq!(short(taken, taken + DAY), "yesterday 15:41");
        assert_eq!(short(taken, taken + 3 * DAY), "Sep 20 15:41");
        // ⚠ Calendar days, not 24-hour spans: 23:50 seen at 00:10 is yesterday.
        let late = 1_789_948_200; // 2026-09-20 23:50:00
        assert_eq!(short(late, late + 20 * 60), "yesterday 23:50");
    }
}
