//! The "new version available" notice (`v0.1` piece 4, goal 1): every decision behind it, with no egui and no
//! network — the request is handed in, so no test ever goes online.
//!
//! Once a day at most, at startup, Rustroom asks GitHub for its newest published release; a newer one than the
//! running build shows one quiet line in the status bar until the user hides that version. Offline, an error, or a
//! reply that is not a version: silent.
//!
//! ⚠⚠ **Nothing from the reply but a version number is ever used** — the release page opened is derived from it
//! ([`release_page`]), never the reply's own `html_url`, so no address from the network is ever opened.
//! ⚠ It is the one request Rustroom makes that the user did not set up: [`COMPILED_IN`] turns it off at build
//! time, for packages something else updates.

use serde::Deserialize;

/// A release's version, `major.minor.patch` — compared as numbers, so `0.10.0` is newer than `0.9.0`.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version(pub u32, pub u32, pub u32);

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// `0.2.0` or `v0.2.0` — exactly three whole numbers; anything else (`v1.2`, `nightly`, `0.2.0-dev`) is `None`.
pub fn parse_version(s: &str) -> Option<Version> {
    let s = s.strip_prefix('v').unwrap_or(s);
    let mut parts = s.split('.').map(|p| {
        if !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()) {
            p.parse::<u32>().ok()
        } else {
            None
        }
    });
    let v = Version(parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(v)
}

/// The running build's version — Cargo's, any `-suffix` (`0.2.0-dev`) dropped.
pub fn current() -> Version {
    let own = env!("CARGO_PKG_VERSION");
    parse_version(own.split('-').next().unwrap_or(own))
        .expect("the app's Cargo.toml version is x.y.z")
}

/// A day in seconds — the longest a check is trusted.
pub const DAY_SECS: u64 = 86_400;

/// Whether a check is due, from the last one's Unix time (`0` = never). A last check in the FUTURE — a clock moved
/// back, a copied settings file — is due, never "not for years".
pub fn is_due(last_checked: u64, now: u64) -> bool {
    last_checked == 0 || now < last_checked || now - last_checked >= DAY_SECS
}

/// The version the status line offers: the newest a check found, if newer than the running build and newer than
/// the one the user hid.
pub fn shown(current: Version, latest: Option<&str>, dismissed: Option<&str>) -> Option<Version> {
    let latest = parse_version(latest?)?;
    let hidden = dismissed.and_then(parse_version);
    (latest > current && hidden.is_none_or(|h| latest > h)).then_some(latest)
}

#[derive(Deserialize)]
struct Release {
    tag_name: String,
}

/// The version in GitHub's `releases/latest` reply; `None` for anything else.
pub fn latest_from_reply(json: &str) -> Option<Version> {
    let r: Release = serde_json::from_str(json).ok()?;
    parse_version(&r.tag_name)
}

/// The release's page on GitHub, derived from its version.
pub fn release_page(v: Version) -> String {
    format!("https://github.com/wladiKrin/rustroom/releases/tag/v{v}")
}

/// Off when built with `RUSTROOM_NO_UPDATE_CHECK` set (any value) — for a distribution's own package or a
/// Flatpak, which something else updates. The Settings switch is hidden then too.
pub const COMPILED_IN: bool = option_env!("RUSTROOM_NO_UPDATE_CHECK").is_none();

/// Whether something else updates this copy: a Flatpak. (The Store's package identity joins it with the Store
/// build — piece 4's goal 2.)
pub fn updated_elsewhere() -> bool {
    #[cfg(target_os = "linux")]
    {
        std::path::Path::new("/.flatpak-info").exists() || std::env::var_os("FLATPAK_ID").is_some()
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

/// Whether this copy may ask at all: compiled in, switched on, and nothing else updating it.
pub fn may_check(switch_on: bool) -> bool {
    COMPILED_IN && switch_on && !updated_elsewhere()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_compare_as_numbers() {
        assert!(parse_version("0.10.0") > parse_version("0.9.0"));
        assert_eq!(parse_version("v1.2.3"), Some(Version(1, 2, 3)));
    }

    #[test]
    fn an_unreadable_reply_is_no_version() {
        assert_eq!(
            latest_from_reply(r#"{"tag_name":"v0.2.0","html_url":"x"}"#),
            Some(Version(0, 2, 0)),
            "fixture check: a real reply is read"
        );
        for reply in [
            r#"{"tag_name":"nightly"}"#,
            r#"{"tag_name":"v1.2"}"#,
            r#"{"tag_name":""}"#,
            "{}",
            "<html>",
        ] {
            assert_eq!(latest_from_reply(reply), None, "{reply}");
        }
    }

    #[test]
    fn the_running_version_parses() {
        let own = env!("CARGO_PKG_VERSION").split('-').next().unwrap();
        assert_eq!(Some(current()), parse_version(own));
    }

    #[test]
    fn a_check_is_due_once_a_day() {
        assert!(is_due(0, 5), "never checked");
        assert!(!is_due(1000, 1000 + DAY_SECS - 1));
        assert!(is_due(1000, 1000 + DAY_SECS));
    }

    #[test]
    fn a_check_from_the_future_is_due() {
        assert!(is_due(2_000, 1_000));
    }

    #[test]
    fn a_newer_release_shows() {
        assert_eq!(
            shown(Version(0, 1, 0), Some("v0.2.0"), None),
            Some(Version(0, 2, 0))
        );
    }

    #[test]
    fn an_older_release_shows_nothing() {
        assert_eq!(shown(Version(0, 2, 0), Some("v0.1.0"), None), None);
        assert_eq!(shown(Version(0, 2, 0), Some("v0.2.0"), None), None);
    }

    #[test]
    fn a_dismissed_version_stays_quiet_until_a_newer_one() {
        let v010 = Version(0, 1, 0);
        assert_eq!(shown(v010, Some("v0.2.0"), Some("v0.2.0")), None);
        assert_eq!(
            shown(v010, Some("v0.3.0"), Some("v0.2.0")),
            Some(Version(0, 3, 0))
        );
    }

    #[test]
    fn the_release_page_is_derived() {
        assert_eq!(
            release_page(Version(0, 2, 0)),
            "https://github.com/wladiKrin/rustroom/releases/tag/v0.2.0"
        );
    }
}
