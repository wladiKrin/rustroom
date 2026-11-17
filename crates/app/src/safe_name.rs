//! Turning a user-typed name into a filename that cannot escape its directory.
//!
//! ⚠ Shared by `presets` and `export_presets` on purpose. This is the **single
//! security-relevant function** in either store — the thing standing between a preset called
//! `../../evil` and someone else's files — and duplicating such a guard is precisely how the
//! two copies drift and one of them becomes a real bug.

/// Map `name` to a filename stem containing only ASCII alphanumerics, space, `_` and `-`.
///
/// Returns an empty string when nothing usable survives, which callers must treat as a
/// rejection rather than writing a file called `.json`.
///
/// This is a **many-to-one** map, so `a/b` and `a_b` share a file and the later save wins —
/// see `distinct_names_can_still_collide_on_one_file` in `presets`. Accepted: hashing the
/// name would fix it at the cost of a directory nobody can read by hand, which is most of
/// the point of one file per preset.
pub fn sanitize(name: &str) -> String {
    let out = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == ' ' || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim()
        .to_string();
    not_a_device(out)
}

/// `name` with a `_` after its stem when the stem is one of Windows' device names — `CON`, `PRN`, `AUX`, `NUL`,
/// `COM0`–`COM9`, `LPT0`–`LPT9` (and `¹²³`), any case — which Windows refuses as a file or folder name with any
/// extension (`nul.json`). On every system (v0.1), so one name always means one file.
fn not_a_device(name: String) -> String {
    let stem_len = name.find('.').unwrap_or(name.len());
    let mut chars = name[..stem_len].chars();
    let head = chars
        .by_ref()
        .take(3)
        .collect::<String>()
        .to_ascii_uppercase();
    let rest: Vec<char> = chars.collect();
    let device = (matches!(head.as_str(), "CON" | "PRN" | "AUX" | "NUL") && rest.is_empty())
        || (matches!(head.as_str(), "COM" | "LPT")
            && matches!(rest.as_slice(), [c] if c.is_ascii_digit() || matches!(c, '¹' | '²' | '³')));
    if device {
        format!("{}_{}", &name[..stem_len], &name[stem_len..])
    } else {
        name
    }
}

/// Map `name` to a single folder name that cannot escape its parent — the import's subfolder.
///
/// ⚠ Gentler than [`sanitize`] on purpose, and beside it so the two guards are read together: a
/// folder the user sees every day may be called "Köln 2026", which `sanitize` would turn into
/// `K_ln 2026`. What makes a name *escape* is a separator, a drive colon, a control character, or
/// being `.`/`..` — those are what this removes. Empty means "no subfolder". ⚠ v0.1: also what Windows
/// refuses in a name — `< > " | ? *` and its device names ([`not_a_device`]) — on every system.
pub fn folder_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':' | '<' | '>' | '"' | '|' | '?' | '*') || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let t = cleaned.trim().trim_end_matches('.').trim();
    if t.chars().all(|c| c == '.') {
        String::new()
    } else {
        not_a_device(t.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readable_names_stay_readable() {
        assert_eq!(sanitize("Warm Punchy 2"), "Warm Punchy 2");
        assert_eq!(sanitize("high-key_v2"), "high-key_v2");
    }

    #[test]
    fn a_traversal_attempt_cannot_escape_the_directory() {
        // ⚠ The reason this function exists, and why it is shared rather than copied.
        assert_eq!(sanitize("../../evil"), "______evil");
        assert_eq!(sanitize("a/b\\c"), "a_b_c");
        assert_eq!(sanitize("/etc/passwd"), "_etc_passwd");
        // Nothing that survives may contain a separator or a dot-segment.
        for probe in ["../x", "..", "./.", "a/../b", "C:\\win"] {
            let s = sanitize(probe);
            assert!(
                !s.contains('/') && !s.contains('\\') && !s.contains('.'),
                "{probe:?} sanitized to {s:?}"
            );
        }
    }

    #[test]
    fn a_name_with_nothing_usable_sanitizes_to_empty() {
        // Callers reject this rather than writing a file named ".json".
        //
        // ⚠ Only genuinely empty-or-blank names qualify. A name of *replaced* characters
        // does not: they become underscores, and `trim` strips whitespace, not underscores.
        // So `"\t\n"` is `"__"` — a perfectly good filename, not a rejection. An earlier
        // version of this test claimed otherwise and was simply wrong about the function.
        for name in ["", " ", "   "] {
            assert!(
                sanitize(name).is_empty(),
                "{name:?} should sanitize to empty"
            );
        }
        // Everything else survives, because a non-space character becomes `_` and `trim`
        // strips whitespace rather than underscores. Worth pinning: it means the
        // reject-on-empty guard catches *blank* names only, not merely unusable ones.
        assert_eq!(sanitize("///"), "___");
        assert_eq!(sanitize("\t\n"), "__");
        assert_eq!(sanitize("\n \t "), "_ _");
    }

    #[test]
    fn a_folder_name_keeps_letters_and_cannot_escape() {
        assert_eq!(folder_name("Köln 2026"), "Köln 2026");
        assert_eq!(folder_name("  Hamburg trip "), "Hamburg trip");
        assert_eq!(folder_name("../../etc"), ".._.._etc");
        assert_eq!(folder_name("a/b\\c:d"), "a_b_c_d");
        assert_eq!(folder_name(".."), "");
        assert_eq!(folder_name("."), "");
        assert_eq!(folder_name("   "), "");
        // A trailing dot is dropped: Windows strips it anyway, so two names would meet there.
        assert_eq!(folder_name("Trip."), "Trip");
        for hostile in ["../x", "a/../../b", "..\\..\\y", "C:\\z"] {
            let n = folder_name(hostile);
            assert!(
                std::path::Path::new(&n).components().count() <= 1
                    && !n.contains('/')
                    && !n.contains('\\'),
                "{hostile:?} → {n:?} is more than one component"
            );
        }
    }

    /// v0.1: what Windows refuses in a name — `< > " | ? *` in a folder, and its device names in either guard,
    /// with or without an extension.
    #[test]
    fn names_windows_refuses_are_made_safe() {
        assert_eq!(
            folder_name("What? <Best> \"of\" a|b*"),
            "What_ _Best_ _of_ a_b_"
        );
        for device in ["CON", "con", "Nul", "AUX", "PRN", "COM1", "lpt9", "COM¹"] {
            assert_eq!(folder_name(device), format!("{device}_"), "{device}");
        }
        assert_eq!(folder_name("nul.backup"), "nul_.backup");
        assert_eq!(sanitize("CON"), "CON_");
        assert_eq!(sanitize("com3"), "com3_");
        // Near misses are names, not devices — the positive control.
        for name in [
            "CONSOLE", "Connie", "COM", "COM10", "LPT", "nullable", "aux2", "Con trip",
        ] {
            assert_eq!(folder_name(name), name, "{name}");
            assert_eq!(sanitize(name), name, "{name}");
        }
    }
}
