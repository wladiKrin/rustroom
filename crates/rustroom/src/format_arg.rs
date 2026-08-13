//! Deciding what `rustroom render` should write.
//!
//! The CLI takes an explicit output *filename*, unlike the GUI's batch export which takes a
//! folder — so the path itself carries most of the answer, and `--format` only has to supply
//! what an extension cannot express: `.png` cannot distinguish 8-bit from 16-bit, and `.tif`
//! likewise. That ambiguity is why `ExportFormat` is flat rather than `format` + `bit_depth`.
//!
//! This lives in the CLI rather than in `export` for the reason already written into
//! `--demosaic`'s comment in `main.rs`: a command-line token vocabulary is a CLI concern, and
//! `export` should not grow one for a consumer the GUI does not share.

use anyhow::{bail, Result};
use export::ExportFormat;
use std::path::Path;

/// The tokens `--format` accepts, in the order the error messages list them.
const TOKENS: [(&str, ExportFormat); 5] = [
    ("jpeg", ExportFormat::Jpeg),
    ("png8", ExportFormat::Png8),
    ("png16", ExportFormat::Png16),
    ("tiff8", ExportFormat::Tiff8),
    ("tiff16", ExportFormat::Tiff16),
];

/// `jpeg|png8|png16|tiff8|tiff16`, for an error message.
///
/// Built from `TOKENS` rather than written out, so a format added to one is never missing
/// from the other.
fn token_list() -> String {
    TOKENS.iter().map(|(t, _)| *t).collect::<Vec<_>>().join("|")
}

/// Parse a `--format` token.
///
/// A plain string plus a lookup rather than a `clap::ValueEnum` derive, so `export` does not
/// gain a `clap` dependency for a CLI concern — the same trade `--demosaic` makes. The error
/// lists the valid values, which is what the derive would have bought.
///
/// ⚠ One token per format and **no aliases**, deliberately asymmetric with
/// [`infer_from_extension`], which does accept them. An extension is something the user
/// already has; a flag is something they are told.
fn parse_format(token: &str) -> Result<ExportFormat> {
    let lower = token.to_ascii_lowercase();
    match TOKENS.iter().find(|(t, _)| *t == lower.as_str()) {
        Some((_, f)) => Ok(*f),
        None => bail!("unknown format {token:?} ({})", token_list()),
    }
}

/// The format an output path implies, or `None` when its extension says nothing.
///
/// Accepts the real-world spellings — `.jpeg` beside `.jpg`, `.tiff` beside `.tif` — and
/// compares lowercased, the convention the Library format filter already set. The depth is
/// the 8-bit member of each family; `--format` is how you ask for 16.
fn infer_from_extension(path: &Path) -> Option<ExportFormat> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" => Some(ExportFormat::Jpeg),
        "png" => Some(ExportFormat::Png8),
        "tif" | "tiff" => Some(ExportFormat::Tiff8),
        _ => None,
    }
}

/// What to write, given the output path and `--format` if it was passed.
///
/// With a flag, the flag wins — but a flag contradicting an extension we *do* recognise is
/// refused rather than producing a silently mislabelled file. An extension we cannot read is
/// not checked at all: the user was explicit, and second-guessing that is worse.
pub fn resolve(path: &Path, flag: Option<&str>) -> Result<ExportFormat> {
    let inferred = infer_from_extension(path);
    let Some(flag) = flag else {
        return inferred.ok_or_else(|| {
            anyhow::anyhow!(
                "cannot tell what format to write for {} — pass --format ({})",
                path.display(),
                token_list()
            )
        });
    };
    let chosen = parse_format(flag)?;
    // ⚠ Compare what each side's `ExportFormat` would be *named*, not the raw extension
    // string: that is what lets `.jpeg` and `.tiff` through, and what lets `--format png16`
    // into `out.png` pass while `tiff16` into `out.png` fails.
    if let Some(inferred) = inferred {
        if inferred.extension() != chosen.extension() {
            bail!(
                "--format {flag} writes .{}, but {} ends in .{}",
                chosen.extension(),
                path.display(),
                inferred.extension()
            );
        }
    }
    Ok(chosen)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> &Path {
        Path::new(s)
    }

    #[test]
    fn every_token_parses_to_its_format() {
        assert_eq!(parse_format("jpeg").unwrap(), ExportFormat::Jpeg);
        assert_eq!(parse_format("png8").unwrap(), ExportFormat::Png8);
        assert_eq!(parse_format("png16").unwrap(), ExportFormat::Png16);
        assert_eq!(parse_format("tiff8").unwrap(), ExportFormat::Tiff8);
        assert_eq!(parse_format("tiff16").unwrap(), ExportFormat::Tiff16);
    }

    #[test]
    fn a_token_is_case_insensitive() {
        assert_eq!(parse_format("PNG16").unwrap(), ExportFormat::Png16);
    }

    #[test]
    fn an_unknown_token_names_every_valid_one() {
        // The error message is the only documentation a mistyped flag gets, so it has to
        // list the alternatives — the same job `--demosaic`'s error already does.
        let e = parse_format("webp").unwrap_err().to_string();
        for token in ["jpeg", "png8", "png16", "tiff8", "tiff16"] {
            assert!(e.contains(token), "{e:?} should mention {token}");
        }
    }

    #[test]
    fn each_extension_infers_its_format() {
        assert_eq!(infer_from_extension(p("a.jpg")), Some(ExportFormat::Jpeg));
        assert_eq!(infer_from_extension(p("a.jpeg")), Some(ExportFormat::Jpeg));
        assert_eq!(infer_from_extension(p("a.png")), Some(ExportFormat::Png8));
        assert_eq!(infer_from_extension(p("a.tif")), Some(ExportFormat::Tiff8));
        assert_eq!(infer_from_extension(p("a.tiff")), Some(ExportFormat::Tiff8));
    }

    #[test]
    fn inference_ignores_the_case_of_the_extension() {
        // A camera or a Windows user hands us `.JPG` routinely.
        assert_eq!(infer_from_extension(p("a.JPG")), Some(ExportFormat::Jpeg));
        assert_eq!(infer_from_extension(p("a.TIFF")), Some(ExportFormat::Tiff8));
    }

    #[test]
    fn an_extension_we_do_not_know_infers_nothing() {
        assert_eq!(infer_from_extension(p("a.foo")), None);
        assert_eq!(infer_from_extension(p("a")), None);
    }

    #[test]
    fn a_jpeg_path_with_no_flag_is_still_a_jpeg() {
        // ⚠ The regression guard. This is the only invocation that exists today, and it
        // must keep working exactly as before.
        assert_eq!(resolve(p("out.jpg"), None).unwrap(), ExportFormat::Jpeg);
    }

    #[test]
    fn with_no_flag_the_extension_decides() {
        assert_eq!(resolve(p("out.png"), None).unwrap(), ExportFormat::Png8);
        assert_eq!(resolve(p("out.tiff"), None).unwrap(), ExportFormat::Tiff8);
    }

    #[test]
    fn with_no_flag_an_unreadable_extension_names_the_fix() {
        // ⚠ A deliberate behaviour change: this used to write a JPEG called `out.foo`.
        for path in ["out.foo", "out"] {
            let e = resolve(p(path), None).unwrap_err().to_string();
            assert!(e.contains("--format"), "{e:?} should name the flag");
            assert!(e.contains(path), "{e:?} should name the path");
        }
    }

    #[test]
    fn the_flag_supplies_the_depth_the_extension_cannot() {
        // The case the flag exists for: `.png` cannot say 8-bit from 16-bit.
        assert_eq!(
            resolve(p("out.png"), Some("png16")).unwrap(),
            ExportFormat::Png16
        );
        assert_eq!(
            resolve(p("out.tif"), Some("tiff16")).unwrap(),
            ExportFormat::Tiff16
        );
    }

    #[test]
    fn a_flag_contradicting_a_known_extension_is_refused() {
        let e = resolve(p("out.png"), Some("tiff16"))
            .unwrap_err()
            .to_string();
        assert!(e.contains("png"), "{e:?} should name the extension");
        assert!(e.contains("tiff16"), "{e:?} should name the flag");
    }

    #[test]
    fn a_flag_wins_over_an_extension_we_cannot_read() {
        // Not second-guessed: the user was explicit, and refusing an explicit flag is worse
        // than writing a PNG named `.foo`.
        assert_eq!(
            resolve(p("out.foo"), Some("png8")).unwrap(),
            ExportFormat::Png8
        );
        assert_eq!(resolve(p("out"), Some("png8")).unwrap(), ExportFormat::Png8);
    }

    #[test]
    fn a_flag_agreeing_with_the_extension_is_fine() {
        // ⚠ `.jpeg` against a format whose own `extension()` is `"jpg"` — the case that
        // rules out comparing raw extension strings.
        assert_eq!(
            resolve(p("out.jpeg"), Some("jpeg")).unwrap(),
            ExportFormat::Jpeg
        );
    }
}
