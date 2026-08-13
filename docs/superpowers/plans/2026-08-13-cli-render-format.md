# CLI `render --format` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `rustroom render` can write all five formats `export::save` supports, choosing by output extension and refining with an explicit `--format` flag.

**Architecture:** One pure function, `format_arg::resolve(path, flag) -> Result<ExportFormat>`, in a new module of the CLI binary crate. `main.rs` calls it and passes the result to the `export::save` call that currently hardcodes `ExportFormat::Jpeg`. No other crate changes.

**Tech Stack:** Rust, `clap` 4 (derive), `anyhow`. No new dependencies — `export` is already a dependency of `crates/rustroom`.

**Spec:** `docs/superpowers/specs/2026-08-13-cli-render-format-design.md`

**Build note:** every `cargo` command here needs libraw and lcms2 on the pkg-config path. Export once per shell before starting:

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

**Commit messages carry no `Co-Authored-By` trailer** (explicit user preference). Never `git push` — the user pushes themselves.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/rustroom/src/format_arg.rs` | **New.** Parse the `--format` token, infer a format from an output extension, and resolve the two into one answer. Pure — paths and strings only, no image data. Owns the crate's first tests. |
| `crates/rustroom/src/main.rs` | Declare the module, add the `--format` argument, make `--quality` an `Option<u8>`, call `resolve`, pass the result to `export::save`, and fix two now-false doc strings. |

### Why this is one task, not two

The obvious split — "add the tested module", then "wire it in" — does not work here. A `pub fn` in a **binary** crate's module reaches nothing outside the crate, so a `format_arg` that `main.rs` does not yet call is dead code in the literal sense the compiler means: `cargo clippy --all-targets -- -D warnings` fails on the bin target, because the `#[cfg(test)]` uses only exist in the test target. An intermediate commit would need an `#[allow(dead_code)]` put in and taken straight back out, which is worse than one honest commit.

So: one task, with a TDD arc inside it. Tests first, implementation second, wiring third, verification last.

---

## Task 1: `--format` on `render`

**Files:**
- Create: `crates/rustroom/src/format_arg.rs`
- Modify: `crates/rustroom/src/main.rs` — module declaration, the `Render` variant, `main`'s match arm, `fn render`

- [ ] **Step 1: Write the failing tests**

Create `crates/rustroom/src/format_arg.rs` containing **only** the imports and the test module, so the tests fail to compile against functions that do not exist yet:

```rust
//! Deciding what `rustroom render` should write.

use anyhow::{bail, Result};
use export::ExportFormat;
use std::path::Path;

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
        let e = resolve(p("out.png"), Some("tiff16")).unwrap_err().to_string();
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
```

- [ ] **Step 2: Register the module so the tests compile**

In `crates/rustroom/src/main.rs`, add directly below the three `use` lines at the top of the file:

```rust
mod format_arg;

/// What `--quality` means when it is not given. Only JPEG reads it.
const DEFAULT_JPEG_QUALITY: u8 = 92;
```

- [ ] **Step 3: Run the tests to verify they fail**

```bash
cargo test -p rustroom format_arg
```

Expected: **compile errors** — `cannot find function parse_format in this scope`, and the same for `infer_from_extension` and `resolve`.

- [ ] **Step 4: Write the implementation**

First, extend the module doc comment at the top of `crates/rustroom/src/format_arg.rs` so it reads:

```rust
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
```

Then insert this **between the `use` lines and the `#[cfg(test)]` module**:

```rust
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
    TOKENS
        .iter()
        .map(|(t, _)| *t)
        .collect::<Vec<_>>()
        .join("|")
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
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cargo test -p rustroom format_arg
```

Expected: `test result: ok. 13 passed`.

⚠ `cargo test` will also print `dead_code` warnings for the bin target, because nothing calls `resolve` yet. That is expected and disappears at step 8. Warnings do not fail `cargo test`; do **not** run `clippy -D warnings` before step 9.

- [ ] **Step 6: Prove the contradiction check can fail**

Temporarily delete the guard — in `resolve`, change `if inferred.extension() != chosen.extension() {` to `if false {` — then run:

```bash
cargo test -p rustroom format_arg
```

Expected: `a_flag_contradicting_a_known_extension_is_refused` **fails**. Restore the condition and re-run to confirm 13 pass again.

⚠ If you restore from a copy rather than by editing, run `touch crates/rustroom/src/format_arg.rs` afterwards — `mv` and `cp -p` preserve the older mtime, and cargo will happily reuse the artifact built from the sabotaged source.

- [ ] **Step 7: Add the argument and loosen `--quality`**

In `crates/rustroom/src/main.rs`, replace the whole `Render` variant of `enum Command` with:

```rust
    /// Decode a RAW, run the develop pipeline, and export it.
    Render {
        /// Input RAW file.
        input: PathBuf,
        /// Output image. Its extension picks the format unless --format says otherwise.
        output: PathBuf,
        /// Optional edits sidecar (JSON). Omitted = no edits.
        #[arg(long)]
        edits: Option<PathBuf>,
        /// Output format: jpeg, png8, png16, tiff8, tiff16. Omitted = inferred from the
        /// output extension, at 8 bits.
        #[arg(long)]
        format: Option<String>,
        /// JPEG quality 1..=100. Ignored by the lossless formats. Default 92.
        #[arg(long)]
        quality: Option<u8>,
        /// Demosaic quality: fastest, fast, better, best. Higher is slower — roughly 1.2 s
        /// to 11 s per photo on a 26 Mpx RAW.
        #[arg(long, default_value = "fast")]
        demosaic: String,
    },
```

⚠ `quality` loses its `default_value_t = 92`. The default moves to `DEFAULT_JPEG_QUALITY` (added in step 2) so that "explicitly passed" stays distinguishable from "defaulted" — which is what lets the warning in step 8 fire only when the user actually asked for something inert.

- [ ] **Step 8: Call it**

In `main`, replace the `Command::Render { .. }` arm:

```rust
        Command::Render {
            input,
            output,
            edits,
            format,
            quality,
            demosaic,
        } => render(
            &input,
            &output,
            edits.as_deref(),
            format.as_deref(),
            quality,
            &demosaic,
        ),
```

Replace `fn render`'s signature:

```rust
fn render(
    input: &Path,
    output: &Path,
    edits_path: Option<&Path>,
    format: Option<&str>,
    quality: Option<u8>,
    demosaic: &str,
) -> Result<()> {
```

Directly **below that signature and above** the existing `// A plain string plus a match rather than a clap::ValueEnum derive` comment, insert:

```rust
    // Resolved before the decode, so a bad --format fails in milliseconds rather than after
    // a multi-second decode and render.
    let format = format_arg::resolve(output, format)?;
    let quality = match quality {
        Some(q) if !format.is_lossy() => {
            eprintln!(
                "warning: --quality {q} is ignored by {} (only JPEG is lossy)",
                format.label()
            );
            q
        }
        Some(q) => q,
        None => DEFAULT_JPEG_QUALITY,
    };
```

Finally replace the `export::save` call at the end of the function:

```rust
    export::save(&rendered, output, format, quality)
        .with_context(|| format!("writing {}", output.display()))?;
```

- [ ] **Step 9: Run the full check suite**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
```

Expected: no warnings, all tests pass. A `dead_code` warning here means step 8's `format_arg::resolve` call did not land.

- [ ] **Step 10: Verify the help text**

```bash
cargo run --release -p rustroom -- render --help
```

Expected: `--format <FORMAT>` and `--quality <QUALITY>` both listed, `--quality` with no `[default: 92]` marker, and the `output` argument described as picking the format by extension.

- [ ] **Step 11: Verify the errors, which need no RAW file**

```bash
cargo run --release -p rustroom -- render /nonexistent.RAF out.foo
```

Expected: exits non-zero with `cannot tell what format to write for out.foo — pass --format (jpeg|png8|png16|tiff8|tiff16)`. ⚠ The format is resolved **before** the decode, so the missing input file is not what fails here — that ordering is the point of step 8.

```bash
cargo run --release -p rustroom -- render /nonexistent.RAF out.png --format tiff16
```

Expected: exits non-zero with `--format tiff16 writes .tif, but out.png ends in .png`.

```bash
cargo run --release -p rustroom -- render /nonexistent.RAF out.png --format webp
```

Expected: exits non-zero with `unknown format "webp" (jpeg|png8|png16|tiff8|tiff16)`.

- [ ] **Step 12: Verify a real render end to end**

This needs a RAW file. The repo's convention is the `RAW_FIXTURE` environment variable holding an absolute path — the same one `raw-decode`'s gated smoke test uses. Set it to the user's X-T4 `.RAF`, the file every other feature was validated against:

```bash
export RAW_FIXTURE=/absolute/path/to/DSCF0001.RAF
```

```bash
cargo run --release -p rustroom -- render "$RAW_FIXTURE" /tmp/fmt-test.png --format png16
```

Expected: `Wrote /tmp/fmt-test.png`. Then confirm it really is a 16-bit PNG:

```bash
sips -g format -g bitsPerSample /tmp/fmt-test.png
```

Expected: `format: png` and `bitsPerSample: 16`. ⚠ Checking the *depth* is what makes this worth running: a silent 8-bit downgrade produces a file that opens correctly and looks right, which is exactly the failure `sixteen_bit_output_really_carries_more_than_eight` exists to catch in `export`.

Then the regression case, which must behave exactly as before:

```bash
cargo run --release -p rustroom -- render "$RAW_FIXTURE" /tmp/fmt-test.jpg
```

Expected: `Wrote /tmp/fmt-test.jpg`, and `sips -g format /tmp/fmt-test.jpg` reports `jpeg`.

And the warning path, which must print one line and still succeed:

```bash
cargo run --release -p rustroom -- render "$RAW_FIXTURE" /tmp/fmt-test.tif --format tiff8 --quality 50
```

Expected: `warning: --quality 50 is ignored by TIFF 8-bit (only JPEG is lossy)` on stderr, then `Wrote /tmp/fmt-test.tif`.

- [ ] **Step 13: Commit**

```bash
git add crates/rustroom/src/format_arg.rs crates/rustroom/src/main.rs
git commit -m "feat(cli): render writes every format export supports"
```

- [ ] **Step 14: Update `CLAUDE.md`**

Add a short section in the "Current status" block, in the house style of the sections already there: what landed; the inference-plus-flag rule and why a contradiction is refused rather than silently honoured; why the compatibility check compares `ExportFormat::extension()` on both sides rather than raw strings; the deliberate asymmetry that inference accepts `.jpeg`/`.tiff` while the flag takes exactly five tokens; the `--quality` `Option<u8>` and the warning it buys; and the accepted caveats from the spec — notably that `render in.RAF out.foo` now errors where it used to write a JPEG.

Then move the **▶ NEXT ACTION** on to the next item in the "What is left" list — matching the 1:1 view's demosaic to the export preset's — renumber that list, and add this spec and plan to the docs list at the bottom of the file.

- [ ] **Step 15: Commit the documentation**

```bash
git add CLAUDE.md docs/superpowers/plans/2026-08-13-cli-render-format.md
git commit -m "docs: record the CLI render --format work"
```
