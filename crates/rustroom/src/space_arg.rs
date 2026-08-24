//! Parsing `--space` for `rustroom render`.
//!
//! A plain string plus a lookup rather than a `clap::ValueEnum` derive, so `color` does not gain
//! a `clap` dependency for a CLI concern — the same trade `--format` and `--demosaic` make. The
//! error lists the valid values, which is what the derive would have bought.

use anyhow::{bail, Result};
use color::OutputSpace;

/// The tokens `--space` accepts, in the order the error message lists them.
const TOKENS: [(&str, OutputSpace); 4] = [
    ("srgb", OutputSpace::Srgb),
    ("p3", OutputSpace::DisplayP3),
    ("adobergb", OutputSpace::AdobeRgb),
    ("prophoto", OutputSpace::ProPhoto),
];

/// `srgb|p3|adobergb|prophoto`, for an error message.
///
/// Built from `TOKENS` rather than written out, so a space added to one is never missing from
/// the other.
fn token_list() -> String {
    TOKENS.iter().map(|(t, _)| *t).collect::<Vec<_>>().join("|")
}

/// Parse a `--space` token. Case-insensitive.
pub fn parse(token: &str) -> Result<OutputSpace> {
    let lower = token.to_ascii_lowercase();
    match TOKENS.iter().find(|(t, _)| *t == lower) {
        Some((_, s)) => Ok(*s),
        None => bail!("unknown space {token:?} ({})", token_list()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_space_has_exactly_one_token() {
        // ⚠ A table over `OutputSpace::ALL`, not a list of four asserts: a fifth space added
        // later must be unreachable from the CLI *loudly*, by failing here, rather than silently.
        for s in OutputSpace::ALL {
            let hits = TOKENS.iter().filter(|(_, t)| *t == s).count();
            assert_eq!(hits, 1, "{} has {hits} tokens", s.label());
        }
    }

    #[test]
    fn tokens_parse_and_are_case_insensitive() {
        assert_eq!(parse("srgb").unwrap(), OutputSpace::Srgb);
        assert_eq!(parse("P3").unwrap(), OutputSpace::DisplayP3);
        assert_eq!(parse("AdobeRGB").unwrap(), OutputSpace::AdobeRgb);
        assert_eq!(parse("prophoto").unwrap(), OutputSpace::ProPhoto);
    }

    #[test]
    fn an_unknown_token_is_refused_and_lists_the_valid_ones() {
        let err = parse("rec2020").unwrap_err().to_string();
        assert!(
            err.contains("rec2020"),
            "error should quote the input: {err}"
        );
        for (t, _) in TOKENS {
            assert!(err.contains(t), "error should list {t}: {err}");
        }
    }
}
