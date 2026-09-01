//! Parsing `--resampler` for `rustroom render`.
//!
//! A plain string plus a lookup rather than a `clap::ValueEnum` derive, so `pipeline` does not
//! gain a `clap` dependency for a CLI concern — the same trade `--format`, `--demosaic` and
//! `--space` make. The error lists the valid values, which is what the derive would have bought.

use anyhow::{bail, Result};
use pipeline::Resampler;

/// The tokens `--resampler` accepts, in the order the error message lists them.
const TOKENS: [(&str, Resampler); 4] = [
    ("bilinear", Resampler::Bilinear),
    ("mitchell", Resampler::Mitchell),
    ("catmullrom", Resampler::CatmullRom),
    ("lanczos3", Resampler::Lanczos3),
];

/// `bilinear|mitchell|catmullrom|lanczos3`, for an error message.
///
/// Built from `TOKENS` rather than written out, so a kernel added to one is never missing from
/// the other.
fn token_list() -> String {
    TOKENS.iter().map(|(t, _)| *t).collect::<Vec<_>>().join("|")
}

/// Parse a `--resampler` token. Case-insensitive; a hyphen is accepted for `catmull-rom`, which
/// is how [`Resampler::label`] spells it.
pub fn parse(token: &str) -> Result<Resampler> {
    let lower = token.to_ascii_lowercase().replace('-', "");
    match TOKENS.iter().find(|(t, _)| *t == lower) {
        Some((_, k)) => Ok(*k),
        None => bail!("unknown resampler {token:?} ({})", token_list()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⚠ A table over `Resampler::ALL`, not a list of four asserts: a fifth kernel added later
    /// must be unreachable from the CLI *loudly*, by failing here, rather than silently.
    #[test]
    fn every_resampler_has_exactly_one_token() {
        for k in Resampler::ALL {
            let hits = TOKENS.iter().filter(|(_, t)| *t == k).count();
            assert_eq!(hits, 1, "{} has {hits} tokens", k.label());
        }
    }

    #[test]
    fn tokens_parse_and_are_case_insensitive() {
        assert_eq!(parse("bilinear").unwrap(), Resampler::Bilinear);
        assert_eq!(parse("Mitchell").unwrap(), Resampler::Mitchell);
        assert_eq!(parse("CATMULLROM").unwrap(), Resampler::CatmullRom);
        assert_eq!(parse("lanczos3").unwrap(), Resampler::Lanczos3);
    }

    /// ⚠ The label the GUI shows is "Catmull-Rom", and someone reading it will type the hyphen.
    /// Refusing that would be a needlessly narrow vocabulary for no gain.
    #[test]
    fn the_hyphen_the_label_uses_is_accepted() {
        assert_eq!(parse("catmull-rom").unwrap(), Resampler::CatmullRom);
    }

    #[test]
    fn an_unknown_token_is_refused_and_lists_the_valid_ones() {
        let err = parse("nearest").unwrap_err().to_string();
        assert!(
            err.contains("nearest"),
            "error should quote the input: {err}"
        );
        for (t, _) in TOKENS {
            assert!(err.contains(t), "error should list {t}: {err}");
        }
    }
}
