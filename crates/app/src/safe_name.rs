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
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == ' ' || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim()
        .to_string()
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
}
