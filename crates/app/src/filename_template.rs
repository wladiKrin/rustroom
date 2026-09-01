//! Turning a filename template like `{name}-{camera}-{seq:3}` into one export's base filename.
//!
//! ⚠ A **lib** module over plain values: no egui, no filesystem, no `ExportOptions`. Every rule
//! here — the token set, zero-padding, what happens to a separator either side of an absent
//! value, and the fallback — is testable without a window, which is what this project's record
//! says naming logic needs.
//!
//! ⚠⚠ **The base name only.** The `-N` de-duplication suffix that stops two files sharing a
//! destination is appended afterwards by `batch_export::dest_named`, and `_N` remains the
//! identity of an unnamed virtual copy. Those two numbering schemes are deliberately separate —
//! see `dest_for`'s doc — and the template is a **third** thing that must not merge with either.

use crate::safe_name;

/// The template every export used before templating existed, and the default.
///
/// ⚠ Re-exported from `batch_export`, where `ExportOptions::default` needs it, so the constant
/// exists once. It renders to exactly `PhotoId::export_stem` — see
/// `the_default_template_reproduces_the_old_naming`, which is what makes an existing user's next
/// export byte-identical.
pub use crate::batch_export::DEFAULT_FILENAME_TEMPLATE as DEFAULT;

/// The characters that read as a separator between parts of a filename.
///
/// ⚠ Used *only* to decide what to drop beside an absent value — see [`Template::render`].
const SEPARATORS: [char; 4] = ['-', '_', ' ', '.'];

/// The widest zero-padding a `{seq}` may ask for.
///
/// ⚠ A cap rather than a free `usize`: `{seq:900}` is a 900-character filename, which most
/// filesystems refuse, and the failure would arrive as a write error per photo rather than as a
/// message about the template.
const MAX_WIDTH: usize = 9;

/// One `{…}` in a template.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Token {
    /// What the Library calls this photo — see [`crate::entries::shown_name`].
    Name,
    /// This photo's 1-based position in the export run, zero-padded to `width`.
    Seq { width: usize },
    /// The capture date, `YYYY-MM-DD`.
    Date,
    /// The camera that took it, `"{MAKE} {model}"`.
    Camera,
    /// Which virtual copy this is — its number, or its name when it has one.
    Copy,
}

impl Token {
    /// The spelling that produced this token, for error messages and the token list.
    const NAMES: [&'static str; 5] = ["name", "seq", "date", "camera", "copy"];
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Segment {
    Literal(String),
    Token(Token),
}

/// What is wrong with a template, in the user's terms.
///
/// ⚠ A template that will not parse is **reported**, never silently dropped or emitted
/// literally: `{camrea}` must say so rather than exporting a folder of files all called
/// `DSCF1234-{camrea}` or, worse, all called `DSCF1234`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateError {
    /// `{bogus}` — not one of [`Token::NAMES`].
    UnknownToken(String),
    /// A `{` with no `}` after it.
    Unclosed,
    /// `{seq:x}`, or a width beyond [`MAX_WIDTH`].
    BadWidth(String),
    /// `{name:3}` — only `{seq}` is a number to pad.
    WidthNotAllowed(String),
}

impl std::fmt::Display for TemplateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownToken(t) => {
                write!(f, "Unknown token {{{t}}} — try {}", token_list())
            }
            Self::Unclosed => write!(f, "A {{ with no closing }}"),
            Self::BadWidth(w) => write!(
                f,
                "{{seq:{w}}} — the width must be a whole number up to {MAX_WIDTH}"
            ),
            Self::WidthNotAllowed(t) => {
                write!(f, "{{{t}}} takes no width — only {{seq}} can be padded")
            }
        }
    }
}

impl std::error::Error for TemplateError {}

/// The tokens, for an error message and for the dialog's hint.
///
/// ⚠ Built from [`Token::NAMES`] rather than written out, so a sixth token cannot be missing
/// from one of the two places that list them — the rule `--format`'s `token_list` already
/// follows.
pub fn token_list() -> String {
    Token::NAMES
        .iter()
        .map(|n| format!("{{{n}}}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Each token paired with what it puts in the filename, for the export dialog.
///
/// ⚠⚠ The descriptions live **beside the tokens**, not in the dialog, for the reason
/// `Token::NAMES` already does: a sixth token cannot then ship with no explanation, and the
/// wording cannot drift from what `resolve` actually does.
///
/// ⚠ `{copy}` gets the longest line on purpose — it was the one reported as unclear, and it is
/// the only token whose *absence* changes the text around it.
pub fn token_help() -> [(&'static str, &'static str); 5] {
    [
        ("{name}", "the file's own name, without its extension"),
        (
            "{seq}",
            "position in this export, from 1 — {seq:3} pads it to 001",
        ),
        ("{date}", "when the photo was taken, as YYYY-MM-DD"),
        ("{camera}", "the camera that took it"),
        (
            "{copy}",
            "a virtual copy's label, or its number when unlabelled. Empty on the original — and \
             the separator next to it disappears with it, so {name}_{copy} is just the filename",
        ),
    ]
}

/// A parsed filename template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    segments: Vec<Segment>,
}

impl Default for Template {
    fn default() -> Self {
        // The default is a constant this module owns, so it cannot fail to parse.
        Self::parse(DEFAULT).expect("the default template must parse")
    }
}

/// Everything one photo contributes to its own filename.
///
/// ⚠ Plain values, deliberately: no `ExportJob`, no `Capture`, no `Library`. That is what lets
/// every rule below be exercised from a unit test with nothing constructed.
#[derive(Debug, Clone, Copy, Default)]
pub struct NameFacts<'a> {
    /// The source file's stem — `DSCF1234` for `DSCF1234.RAF`.
    pub file_stem: &'a str,
    /// `0` for the original.
    pub copy: u32,
    /// This copy's own name, when it has one.
    pub copy_label: Option<&'a str>,
    /// 1-based position in this export run.
    pub seq: usize,
    /// Capture time as **naive wall-clock** seconds — the catalog's own convention, so no
    /// timezone arithmetic happens anywhere here. `None` when the file carries no EXIF date.
    pub taken: Option<i64>,
    /// `"{MAKE} {model}"`, or `None` when nothing was recorded.
    pub camera: Option<&'a str>,
}

impl Template {
    pub fn parse(src: &str) -> Result<Self, TemplateError> {
        let mut segments = Vec::new();
        let mut lit = String::new();
        let mut chars = src.chars();
        while let Some(c) = chars.next() {
            if c != '{' {
                lit.push(c);
                continue;
            }
            let mut body = String::new();
            let mut closed = false;
            for c in chars.by_ref() {
                if c == '}' {
                    closed = true;
                    break;
                }
                body.push(c);
            }
            if !closed {
                return Err(TemplateError::Unclosed);
            }
            let token = parse_token(&body)?;
            if !lit.is_empty() {
                segments.push(Segment::Literal(std::mem::take(&mut lit)));
            }
            segments.push(Segment::Token(token));
        }
        if !lit.is_empty() {
            segments.push(Segment::Literal(lit));
        }
        Ok(Self { segments })
    }

    /// The base filename for one photo — no extension, no de-duplication suffix.
    ///
    /// ⚠⚠ **A token with no value takes one separator with it.** `{name}-{camera}-{seq}` on a
    /// photo whose camera was never recorded gives `DSCF1234-001`, not `DSCF1234--001`. The rule
    /// is: an empty token strips the *trailing* separators of the literal before it, or — when
    /// there is no such literal, or it has none — the *leading* separators of the literal after
    /// it. That handles a token at either end and two absent tokens in a row, which a plain
    /// "collapse repeated separators" pass over the finished string would not, and it never
    /// touches a separator the user typed away from an absent value.
    ///
    /// ⚠ The whole result goes through `safe_name::sanitize` **once, at the end**: the template
    /// is free text that becomes a path, and that function is the single guard this project has
    /// against a name escaping its directory. Sanitizing per token instead would run it four
    /// times and still leave the literals unguarded.
    ///
    /// ⚠ A template that renders to nothing falls back to the name — `{camera}` alone on a photo
    /// with no camera must not produce a file called `.jpg`.
    pub fn render(&self, facts: &NameFacts) -> String {
        let mut pieces: Vec<Piece> = self
            .segments
            .iter()
            .map(|s| match s {
                Segment::Literal(l) => Piece::Literal(l.clone()),
                Segment::Token(t) => Piece::Value(resolve(*t, facts)),
            })
            .collect();

        for i in 0..pieces.len() {
            if !matches!(pieces[i], Piece::Value(None)) {
                continue;
            }
            if i > 0 && trim_trailing_separators(&mut pieces[i - 1]) {
                continue;
            }
            if i + 1 < pieces.len() {
                trim_leading_separators(&mut pieces[i + 1]);
            }
        }

        let joined: String = pieces
            .iter()
            .map(|p| match p {
                Piece::Literal(l) => l.as_str(),
                Piece::Value(v) => v.as_deref().unwrap_or(""),
            })
            .collect();
        let out = safe_name::sanitize(&joined);
        if out.is_empty() {
            // ⚠ The file's own stem. `export_stem` is gone; a template that renders to nothing
            // still has to produce a filename, and the stem is the only thing always available.
            facts.file_stem.to_string()
        } else {
            out
        }
    }
}

enum Piece {
    Literal(String),
    Value(Option<String>),
}

/// Strip a literal's trailing separators; `true` if there were any to strip.
fn trim_trailing_separators(p: &mut Piece) -> bool {
    let Piece::Literal(s) = p else { return false };
    let trimmed = s.trim_end_matches(SEPARATORS);
    if trimmed.len() == s.len() {
        return false;
    }
    *s = trimmed.to_string();
    true
}

fn trim_leading_separators(p: &mut Piece) {
    if let Piece::Literal(s) = p {
        *s = s.trim_start_matches(SEPARATORS).to_string();
    }
}

fn parse_token(body: &str) -> Result<Token, TemplateError> {
    let (name, width) = match body.split_once(':') {
        Some((n, w)) => {
            let parsed: usize = w
                .parse()
                .map_err(|_| TemplateError::BadWidth(w.to_string()))?;
            if parsed > MAX_WIDTH {
                return Err(TemplateError::BadWidth(w.to_string()));
            }
            (n, Some(parsed))
        }
        None => (body, None),
    };
    let no_width = |t: Token| match width {
        Some(_) => Err(TemplateError::WidthNotAllowed(name.to_string())),
        None => Ok(t),
    };
    match name {
        "name" => no_width(Token::Name),
        "date" => no_width(Token::Date),
        "camera" => no_width(Token::Camera),
        "copy" => no_width(Token::Copy),
        "seq" => Ok(Token::Seq {
            width: width.unwrap_or(0),
        }),
        other => Err(TemplateError::UnknownToken(other.to_string())),
    }
}

/// One token's value, or `None` when this photo has nothing to put there.
fn resolve(token: Token, facts: &NameFacts) -> Option<String> {
    match token {
        // ⚠⚠ The **bare file stem**, carrying no copy identity — so `{name}` and `{copy}` are
        // orthogonal and `{name}_{copy}` reads correctly instead of doubling. ⚠ A template of
        // bare `{name}` therefore gives every copy of a file the same base, and only `-N`
        // de-duplication keeps their exports apart; that is why `DEFAULT_FILENAME_TEMPLATE` is
        // `{name}_{copy}` rather than `{name}`.
        Token::Name => Some(facts.file_stem.to_string()),
        // Never absent: a position in a list always exists.
        Token::Seq { width } => Some(format!("{:0width$}", facts.seq, width = width)),
        // ⚠ `from_timestamp` reads the stored value as UTC, which is exactly what returns the
        // wall-clock the camera wrote — the payoff of the catalog storing naive seconds. Same
        // route `modals::info::taken_text` takes.
        Token::Date => facts.taken.and_then(|s| {
            chrono::DateTime::from_timestamp(s, 0).map(|d| d.format("%Y-%m-%d").to_string())
        }),
        Token::Camera => non_blank(facts.camera),
        // ⚠ The ordering is the whole rule, and step 1 applies to copy 0 too: a **labelled**
        // original contributes "Colour", which `export_stem` could not express at all because it
        // returned early for copy 0. Only an *unlabelled* original is absent, and its adjacent
        // separator then collapses.
        Token::Copy => match (facts.copy, non_blank(facts.copy_label)) {
            (_, Some(l)) => Some(l),
            (0, _) => None,
            (c, _) => Some(c.to_string()),
        },
    }
}

fn non_blank(s: Option<&str>) -> Option<String> {
    s.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A landscape shot, fully described: every token has a value.
    fn facts() -> NameFacts<'static> {
        NameFacts {
            file_stem: "DSCF1234",
            copy: 0,
            copy_label: None,
            seq: 7,
            // 2026-08-27 10:00:00, as naive wall-clock seconds.
            taken: Some(1_787_824_800),
            camera: Some("FUJIFILM X-T4"),
        }
    }

    fn render(src: &str, f: &NameFacts) -> String {
        Template::parse(src).expect("should parse").render(f)
    }

    #[test]
    fn a_literal_with_no_token_is_used_as_is() {
        assert_eq!(render("export", &facts()), "export");
    }

    #[test]
    fn every_token_resolves() {
        let f = facts();
        assert_eq!(render("{name}", &f), "DSCF1234");
        assert_eq!(render("{seq}", &f), "7");
        assert_eq!(render("{date}", &f), "2026-08-27");
        assert_eq!(render("{camera}", &f), "FUJIFILM X-T4");
        let c = NameFacts { copy: 2, ..f };
        assert_eq!(render("{copy}", &c), "2");
    }

    #[test]
    fn seq_is_zero_padded_to_the_width_asked_for() {
        let f = facts();
        assert_eq!(render("{seq:3}", &f), "007");
        assert_eq!(
            render("{seq:1}", &f),
            "7",
            "a width below the number is a no-op"
        );
        assert_eq!(render("{seq:0}", &f), "7");
        let big = NameFacts { seq: 1234, ..f };
        assert_eq!(
            render("{seq:3}", &big),
            "1234",
            "padding never truncates the number"
        );
    }

    #[test]
    fn tokens_compose_with_literals_around_them() {
        assert_eq!(
            render("{date}_{name}_{seq:3}", &facts()),
            "2026-08-27_DSCF1234_007"
        );
    }

    // ─── the absent-value rule ───────────────────────────────────────────────────────────

    #[test]
    fn an_absent_value_takes_the_separator_before_it() {
        // ⚠⚠ The case the whole collapsing rule exists for. Two of the author's own exported
        // JPEGs carry no EXIF at all, so this is ordinary rather than exotic.
        let f = NameFacts {
            camera: None,
            ..facts()
        };
        assert_eq!(render("{name}-{camera}-{seq:3}", &f), "DSCF1234-007");
        assert!(
            !render("{name}-{camera}-{seq:3}", &f).contains("--"),
            "a dangling separator is the bug this rule prevents"
        );
    }

    #[test]
    fn an_absent_value_at_the_start_takes_the_separator_after_it() {
        // There is no preceding literal to strip, so the rule has to reach forwards instead.
        let f = NameFacts {
            camera: None,
            ..facts()
        };
        assert_eq!(render("{camera}-{name}", &f), "DSCF1234");
    }

    #[test]
    fn two_absent_values_in_a_row_leave_no_separator_at_all() {
        // ⚠ The case a plain "collapse repeated separators" pass gets wrong: after the first
        // token strips the literal following it, the second has an *empty* literal before it and
        // must reach forwards in turn.
        let f = NameFacts {
            camera: None,
            taken: None,
            ..facts()
        };
        assert_eq!(render("{date}-{camera}-{seq:3}", &f), "007");
        assert_eq!(render("{name}_{date}_{camera}", &f), "DSCF1234");
    }

    #[test]
    fn a_separator_away_from_an_absent_value_is_untouched() {
        // ⚠ The other half of the rule: it removes the separator *beside* what vanished, and
        // nothing else. Without this the test above would pass with "strip every separator".
        let f = NameFacts {
            camera: None,
            ..facts()
        };
        assert_eq!(render("{name}-{seq:3}-{camera}", &f), "DSCF1234-007");
        assert_eq!(
            render("my-shoot_{camera}", &f),
            "my-shoot",
            "a hyphen inside a literal is not a boundary"
        );
    }

    #[test]
    fn an_unlabelled_original_contributes_no_copy_part() {
        // ⚠ The separator collapses with it, so `{name}_{copy}` is plain `DSCF1234`.
        let f = NameFacts {
            file_stem: "DSCF1234",
            copy: 0,
            copy_label: None,
            ..facts()
        };
        assert_eq!(render("{name}_{copy}", &f), "DSCF1234");
    }

    #[test]
    fn a_labelled_copy_contributes_its_label_to_the_copy_token() {
        // ⚠⚠ Replaces `a_named_copy_contributes_its_name_without_an_extension`, which asserted
        // that `{name}` returned the *copy's* name. Under M6.0 `{name}` is the bare stem and the
        // label belongs to `{copy}` — the two are orthogonal, which is the whole point.
        let f = NameFacts {
            file_stem: "DSCF1234",
            copy: 2,
            copy_label: Some("Warm"),
            ..facts()
        };
        assert_eq!(render("{copy}", &f), "Warm");
        assert_eq!(render("{name}", &f), "DSCF1234");
    }

    #[test]
    fn a_template_that_renders_to_nothing_falls_back_to_the_name() {
        // ⚠ Otherwise the export is a file called `.jpg`, one per photo, all colliding.
        let f = NameFacts {
            camera: None,
            ..facts()
        };
        assert_eq!(render("{camera}", &f), "DSCF1234");
        // ⚠ `safe_name::sanitize` trims **whitespace**, not separators — so a blank template
        // falls back and a literal `---` does not. That asymmetry is `sanitize`'s, not this
        // module's, and the first version of this test asserted the opposite about correct code.
        assert_eq!(render("   ", &f), "DSCF1234");
        assert_eq!(
            render("---", &f),
            "---",
            "a typed literal is a legitimate name"
        );
    }

    #[test]
    fn a_template_cannot_escape_its_directory() {
        // ⚠ The one security-relevant line here: the template is free text that becomes a path,
        // exactly as a preset's name is. Same guard, applied once over the finished string.
        let got = render("../../{name}", &facts());
        assert!(!got.contains('/'), "got {got}");
        assert!(!render("{name}/../evil", &facts()).contains('/'));
    }

    #[test]
    fn an_unknown_token_is_reported_rather_than_dropped() {
        // ⚠⚠ Not silently removed and not emitted literally: a folder of files all called
        // `DSCF1234-{camrea}` is the second-worst outcome, and all of them called `DSCF1234` —
        // colliding into one — is the worst.
        let err = Template::parse("{name}-{camrea}").unwrap_err();
        assert_eq!(err, TemplateError::UnknownToken("camrea".into()));
        assert!(
            err.to_string().contains("camrea") && err.to_string().contains("{camera}"),
            "the message must name what was typed and what was meant: {err}"
        );
    }

    #[test]
    fn an_unclosed_brace_is_reported() {
        assert_eq!(
            Template::parse("{name").unwrap_err(),
            TemplateError::Unclosed
        );
        assert_eq!(
            Template::parse("a-{seq:3").unwrap_err(),
            TemplateError::Unclosed
        );
    }

    #[test]
    fn a_stray_closing_brace_is_just_a_character() {
        // Nothing to report: `}` on its own is unambiguous, and refusing it would reject a
        // filename someone can legitimately want.
        assert_eq!(render("a}b", &facts()), "a_b", "sanitize maps it to _");
    }

    #[test]
    fn a_bad_or_oversized_width_is_reported() {
        assert_eq!(
            Template::parse("{seq:x}").unwrap_err(),
            TemplateError::BadWidth("x".into())
        );
        assert_eq!(
            Template::parse("{seq:900}").unwrap_err(),
            TemplateError::BadWidth("900".into()),
            "a 900-character filename fails per photo at write time instead"
        );
        assert!(Template::parse(&format!("{{seq:{MAX_WIDTH}}}")).is_ok());
    }

    #[test]
    fn only_seq_takes_a_width() {
        // ⚠ `{camera:3}` means nothing, and accepting it silently would teach a padding syntax
        // that works on one token and is ignored on four.
        for t in ["name", "date", "camera", "copy"] {
            assert_eq!(
                Template::parse(&format!("{{{t}:3}}")).unwrap_err(),
                TemplateError::WidthNotAllowed(t.to_string()),
                "{t}"
            );
        }
    }

    #[test]
    fn the_token_list_names_every_token() {
        // ⚠ Built from `Token::NAMES`, so a sixth token cannot be missing from the error message
        // or the dialog's hint — and this asserts the *parser* accepts each one, which is what
        // makes the list a promise rather than a string.
        let list = token_list();
        for n in Token::NAMES {
            assert!(
                list.contains(&format!("{{{n}}}")),
                "{n} missing from {list}"
            );
            assert!(
                Template::parse(&format!("{{{n}}}")).is_ok(),
                "{n} is listed but does not parse"
            );
        }
    }

    #[test]
    fn a_template_is_reusable_across_photos() {
        // The worker parses once per run and renders per photo; nothing may carry over.
        let t = Template::parse("{name}-{seq:3}").unwrap();
        let a = t.render(&NameFacts {
            file_stem: "a",
            seq: 1,
            ..facts()
        });
        let b = t.render(&NameFacts {
            file_stem: "b",
            seq: 2,
            ..facts()
        });
        assert_eq!((a.as_str(), b.as_str()), ("a-001", "b-002"));
    }

    /// ⚠⚠ The default template must reproduce the old naming for **unlabelled** copies, byte for
    /// byte — that is what makes this change invisible to every existing export.
    #[test]
    fn the_default_template_reproduces_the_old_naming_for_unlabelled_copies() {
        let f = NameFacts {
            file_stem: "DSCF1234",
            copy: 0,
            copy_label: None,
            ..facts()
        };
        assert_eq!(render(DEFAULT, &f), "DSCF1234");
        let f2 = NameFacts { copy: 2, ..f };
        assert_eq!(render(DEFAULT, &f2), "DSCF1234_2");
    }

    /// ⚠⚠ This is what proves the old rule was **generalised, not dropped**: "a copy's name is
    /// its export name" is now expressible as the template `{copy}`.
    #[test]
    fn the_copy_token_alone_reproduces_the_old_named_copy_export() {
        let f = NameFacts {
            file_stem: "DSCF1234",
            copy: 2,
            copy_label: Some("Warm"),
            ..facts()
        };
        assert_eq!(render("{copy}", &f), "Warm");
    }

    #[test]
    fn name_is_the_bare_stem_and_carries_no_copy_identity() {
        let f = NameFacts {
            file_stem: "DSCF1234",
            copy: 2,
            copy_label: Some("Warm"),
            ..facts()
        };
        assert_eq!(render("{name}", &f), "DSCF1234");
        assert_eq!(render("{name}_{copy}", &f), "DSCF1234_Warm");
    }

    /// ⚠ A labelled copy 0 contributes its label — the case `export_stem` could not express at
    /// all, because it returned early for the original.
    #[test]
    fn a_labelled_original_contributes_its_label() {
        let f = NameFacts {
            file_stem: "DSCF1234",
            copy: 0,
            copy_label: Some("Colour"),
            ..facts()
        };
        assert_eq!(render("{name}_{copy}", &f), "DSCF1234_Colour");
    }

    /// ⚠⚠ Carried over from the deleted `an_export_name_cannot_escape_its_directory`, which
    /// guarded `export_stem`. The label is free text that reaches a path, so the guard must
    /// follow it here — but note the inversion M6.0 makes: only the **filename** is sanitized,
    /// while the label itself keeps what the user typed.
    #[test]
    fn a_label_never_escapes_into_the_path() {
        for evil in ["../../evil", "a/b"] {
            let f = NameFacts {
                file_stem: "DSCF1234",
                copy: 1,
                copy_label: Some(evil),
                ..facts()
            };
            let out = render("{name}_{copy}", &f);
            assert!(!out.contains('/'), "a label reached the path: {out}");
            assert!(!out.contains(".."), "a label reached the path: {out}");
        }
    }

    /// ⚠ Every token must have an explanation, or a sixth one ships with no description and the
    /// dialog silently stops being a reference. The pairing is what `Token::NAMES` exists for.
    #[test]
    fn every_token_is_explained() {
        let help = token_help();
        assert_eq!(help.len(), Token::NAMES.len());
        for name in Token::NAMES {
            let want = format!("{{{name}}}");
            let row = help.iter().find(|(t, _)| *t == want);
            let (_, what) = row.unwrap_or_else(|| panic!("no explanation for {want}"));
            assert!(!what.is_empty(), "{want} has an empty explanation");
        }
    }
}
