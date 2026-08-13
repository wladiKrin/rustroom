# `--format` on the CLI's `render`

**Status:** spec, 2026-08-13.

## The problem

`crates/rustroom/src/main.rs:118` hardcodes the output format:

```rust
export::save(&rendered, output, export::ExportFormat::Jpeg, quality)
```

`export::save` has dispatched five formats since the PNG/TIFF work, and `ExportFormat`
already carries `ALL`, `extension()`, `is_lossy()`, `label()`, `Default` and serde. The GUI
uses all of it. The CLI simply never got the parameter, so `rustroom render` can produce a
JPEG and nothing else — recorded at the time as caveat (4) of that feature.

This is the gap, and it is one argument wide. What makes it worth a spec is not the plumbing
but deciding **where the format comes from**, because the CLI takes an explicit output
filename while the GUI's batch export takes a folder.

## Decisions taken

| Question | Decision |
|---|---|
| Flag or output extension? | **Both, with inference as the default.** No flag: infer from the extension. With a flag: it wins and supplies the depth an extension cannot express, but contradicting a *recognised* extension is an error. |
| No flag, unrecognised or absent extension? | **Error, naming the fix.** A behaviour change: today it silently writes a JPEG. |

### Why not flag-only

`--format` defaulting to `jpeg` with the path treated as opaque is less code and has no
inference rule to explain. It also means `rustroom render in.RAF out.png` silently writes a
JPEG named `.png`. That is the quiet-wrongness failure mode this codebase repeatedly refuses.

### Why not extension-only

`.png` cannot distinguish `Png8` from `Png16`, and `.tif` cannot distinguish `Tiff8` from
`Tiff16`. Extension-only would make two of the five formats unreachable from the CLI. That
ambiguity is not incidental — it is exactly why `ExportFormat` is flat rather than
`format` + `bit_depth`.

## Design

### 1. One pure function, in `crates/rustroom/src/format_arg.rs`

```rust
/// The five tokens `--format` accepts. One per format, no aliases.
fn parse_format(token: &str) -> anyhow::Result<ExportFormat>;

/// The format an output path implies, or `None` if its extension says nothing.
/// `.jpg`/`.jpeg` → Jpeg, `.png` → Png8, `.tif`/`.tiff` → Tiff8.
fn infer_from_extension(path: &Path) -> Option<ExportFormat>;

/// What to write, given the output path and the `--format` flag if it was passed.
pub fn resolve(path: &Path, flag: Option<&str>) -> anyhow::Result<ExportFormat>;
```

`resolve` is the whole feature:

- **flag given** — parse it; then, *only if* `infer_from_extension` recognises the path,
  reject a contradiction. An unrecognised extension is not checked, because the user was
  explicit.
- **no flag** — infer, or error naming the fix.

It touches no image data, so it is testable with nothing but paths.

**It lives in the CLI, not in `export`**, for the reason already written into `--demosaic`'s
comment in `main.rs`: a CLI token vocabulary is a CLI concern, and `export` should not grow
one for a consumer the GUI does not share. A separate file rather than more of `main.rs`
because `main.rs` is a thin dispatcher today and should stay one.

### 2. The compatibility check reuses inference rather than comparing strings

`ExportFormat::extension()` returns `"jpg"`, `"png"`, `"tif"` — but users write `out.jpeg`
and `out.tiff`. So the check is

```rust
if let Some(inferred) = infer_from_extension(path) {
    if inferred.extension() != chosen.extension() {
        anyhow::bail!(/* names both */);
    }
}
```

Comparing what each side's `ExportFormat` *would be named* rather than the raw strings is
what makes `out.jpeg` and `out.tiff` work: inference normalises the aliases, so one function
serves both jobs. It also means the check passes for `--format png16` into `out.png`, which
is the case the flag exists for — both sides are `"png"`, and only the depth differs. Extensions compare
lowercased, the convention the Library format filter already set.

⚠ Note the deliberate asymmetry: **inference accepts aliases, the flag does not.** Five
tokens, one per format — `jpeg`, `png8`, `png16`, `tiff8`, `tiff16` — and the error lists
them, exactly as `--demosaic` lists its four.

### 3. `--quality` becomes `Option<u8>`

It is inert for four of the five formats: `export::save` ignores it outside JPEG, and the GUI
greys its slider out via `is_lossy()`. Making the argument an `Option` distinguishes
"explicitly passed" from "defaulted", so `--quality 60 --format png16` prints one line on
stderr rather than silently doing nothing. A warning, not an error — the combination is not
wrong, merely inert. The default stays 92, applied after parsing.

### 4. Documentation strings

Two are now false and change with the code: the `Render` subcommand's "export a JPEG", and
its `output` argument's "Output JPEG".

## Testing

The first tests in `crates/rustroom`. A bin-only crate takes `#[cfg(test)]` modules the same
as any other, as `app`'s `main.rs` already demonstrates. No image fixtures — every case is a
path and a flag.

- each of the five tokens parses to its format
- an unknown token errors, and the message names all five
- each extension infers: `.jpg`, `.jpeg`, `.png`, `.tif`, `.tiff`, and the same uppercased
- no flag and an unrecognised extension errors
- no flag and no extension at all errors
- `--format png16` with `out.png` yields `Png16` — the flag supplying the depth
- `--format tiff16` with `out.png` errors
- `--format png8` with `out.foo` is honoured, the extension being unrecognised
- **regression guard:** `out.jpg` with no flag is still `Jpeg`, which is today's behaviour and
  the only invocation that exists

## Accepted caveats

1. **`rustroom render in.RAF out.foo` now errors** where it used to write a JPEG. The
   behaviour change chosen above; the common `out.jpg` case is untouched.
2. **A flag paired with an unrecognised extension still produces a mislabelled file** —
   `--format png8` into `out.foo` writes a PNG called `.foo`. Deliberate: the user was
   explicit, and second-guessing an explicit flag is worse than honouring it.
3. **No `--format` on `decode`.** That subcommand writes a display-referred 8-bit TIFF by
   hand through `image::save_buffer` rather than through `export` at all. It is a debug dump,
   not an export; unifying the two is a separate question.
4. **JPEG remains the default** when the extension says so, so nothing about existing
   invocations changes.
