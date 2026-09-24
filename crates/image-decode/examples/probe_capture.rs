//! Print what `read_capture` makes of one file. A throwaway probe for diagnosing a date that
//! reads as absent — see the SP-3000 placeholder finding, 2026-09-24.
fn main() {
    for arg in std::env::args().skip(1) {
        let p = std::path::PathBuf::from(&arg);
        match image_decode::read_capture(&p) {
            Ok(c) => println!(
                "{arg}\n  taken  = {:?}\n  camera = {:?}",
                c.taken.map(chrono_like),
                c.camera
            ),
            Err(e) => println!("{arg}\n  error: {e}"),
        }
    }
}

/// Seconds to a readable naive stamp, without pulling chrono into this crate.
fn chrono_like(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02} ({secs})",
        rem / 3600,
        (rem % 3600) / 60,
        rem % 60
    )
}

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}
