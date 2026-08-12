# Export to PNG and TIFF — design

**Date:** 2026-08-11
**Status:** approved, ready to plan
**Scope:** `export` (the format type, 16-bit output, ICC), `app` (`ExportOptions`, `dest_for`, one dropdown), workspace (`lcms2`)

## What this adds

JPEG is currently the only output. This adds **PNG and TIFF, at 8 or 16 bits**, and — because
the ICC gap becomes indefensible once a 16-bit archival format is on the table — **embeds an
sRGB profile in every format, JPEG included.**

## What the `image` crate already gives us

Checked before designing, because it determined the whole shape:

- **PNG and TIFF both write `Rgb16`.** No lower-level crate needed.
- **PNG, TIFF *and* JPEG all implement `ImageEncoder::set_icc_profile`.** Embedding a profile
  is a byte blob, not a fight with the encoder — which is what made ICC affordable in this
  cycle rather than a separate project.

## The format type

```rust
/// In `crates/export`, which owns encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ExportFormat {
    #[default]
    Jpeg,
    Png8,
    Png16,
    Tiff8,
    Tiff16,
}

impl ExportFormat {
    /// "jpg" | "png" | "tif". ⚠ `tif` not `tiff`, and `jpg` not `jpeg` — chosen for
    /// consistency with what `dest_for` already produced, so existing exports keep their
    /// shape. Both spellings are equally valid; this one is simply fixed.
    pub fn extension(self) -> &'static str;
    pub fn label(self) -> &'static str;        // for the dropdown
    pub fn is_lossy(self) -> bool;             // only Jpeg — drives the quality slider
    pub const ALL: [ExportFormat; 5];
}
```

**Flat, rather than `format` + `bit_depth`.** There is no such thing as a 16-bit JPEG, and a
separate depth field would need a rule saying so. Flattening makes the invalid state
**unrepresentable** — the same reasoning behind `Decision::Cancel` carrying no `apply_to_all`,
where making the bad combination impossible beat enforcing it with a rule someone can forget.

It lives in `export` and gains serde there, exactly as `Demosaic` lives in `raw-decode` and
gained serde: the type goes where the capability is, so the extension and the encoder mapping
exist in one place.

## 16-bit output

```rust
/// Quantize a display-encoded image to interleaved 16-bit RGB, clamping to [0, 1].
pub fn to_rgb16(image: &ImageBuf) -> Vec<u16>;
```

**The extra bits are real, not interpolated.** The pipeline is `f32` end to end and libraw
hands us 16-bit data, so a 16-bit TIFF genuinely carries precision an 8-bit file discards.
That is the entire reason to add TIFF — an 8-bit PNG is just a bigger JPEG.

⚠ `image`'s `write_image` takes `&[u8]` and expects **native-endian** `u16` for `Rgb16`, so a
small adapter sits at the call site. `to_rgb16` returns `Vec<u16>` rather than bytes so it
stays directly testable, mirroring `to_rgb8`.

## The save API

`save_jpeg` is replaced by one dispatcher, so callers never branch on format:

```rust
/// Encode a display-referred image. `quality` is ignored unless the format is lossy.
pub fn save(
    image: &ImageBuf,
    path: &Path,
    format: ExportFormat,
    quality: u8,
) -> Result<(), ExportError>;
```

⚠ `quality` stays a plain parameter rather than becoming `Option<u8>`. The UI already keeps a
quality value while PNG is selected — greying the slider out does not discard it — so an
`Option` would mean threading "is this format lossy" through a second place that can disagree
with `is_lossy()`. One function, one authority.

`batch_export::export_one` calls it instead of `save_jpeg`, passing `opts.format`. Two call
sites in `batch_export::run` pass the format to `dest_for` — the initial destination and the
`KeepBoth` loop that probes `-1`, `-2`, … suffixes.

## ICC, via lcms2

```rust
/// The sRGB profile every export is tagged with. Generated once.
pub fn srgb_icc() -> &'static [u8];
```

Built behind a `OnceLock` from lcms2's own sRGB profile and attached with
`set_icc_profile`.

**JPEG gets it too**, not only the new formats. It has been shipping untagged all along; if we
are closing the gap, closing it everywhere costs nothing and removes an inconsistency nobody
would be able to explain later.

⚠ **This does not un-defer colour management.** lcms2 is used here for exactly one thing —
emitting a static sRGB profile. Input profiles, conversions and a working-space policy remain
deferred, and `CLAUDE.md`'s existing note (libraw emits sRGB primaries, so only the transfer
curve is needed) still holds.

### Build setup

`little-cms2` is installed on this machine but **off the default pkg-config path**, exactly as
libraw is. The setup line becomes:

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

Verified: `pkg-config --modversion lcms2` → 2.19 with that path. CI gains `liblcms2-dev`
alongside `libraw-dev`.

⚠ If the system library proves awkward on CI, the `lcms2` crate can build vendored — note it
as the fallback rather than discovering it under a red build.

## The change with teeth: `dest_for`

```rust
pub fn dest_for(dir: &Path, source: &Path, index: usize, format: ExportFormat) -> PathBuf;
```

⚠ **This is the risky part of the feature, not the encoders.** `batch_export`'s collision
logic keys on the **produced filename**, so the extension decides *which files collide*.

Today `a.RAF` and `a.jpg` both produce `a.jpg` in an empty folder — there is a test named for
exactly that case. Exporting PNG they no longer collide; but `a.RAF` and `a.png` now do.

So `two_sources_with_the_same_stem_collide_in_an_empty_folder` becomes
**format-parameterised** rather than being edited until it passes again. The property under
test is "two sources that map to one destination collide", which is format-independent; only
the *pair* that triggers it changes.

## UI

One dropdown in the export dialog. The quality slider becomes
`ui.add_enabled(format.is_lossy(), …)` so it greys out for PNG/TIFF rather than sitting there
implying it does something — the same treatment `NoiseReduction`'s Detail and `Sharpening`'s
Masking already get for being inert.

`ExportOptions` gains `format`, so **export presets carry it**, like `demosaic`.

## Tests

- **⚠ A 16-bit round trip through a real file.** Write a value 8 bits cannot represent, read
  the file back, assert it survived. **Without this, a silent 8-bit downgrade passes every
  other test in this feature** — the file would still open, still have the right dimensions,
  still look right.
- **The ICC profile is present and non-empty when read back**, via
  `ImageDecoder::icc_profile()`, for all three container formats.
- `to_rgb16` quantizes and clamps, mirroring `to_rgb8`'s existing test.
- Every format writes a file that `image::open` can read, at the right dimensions.
- `dest_for` produces the right extension per format.
- Collision behaviour **per format**, parameterised as above.
- `ExportOptions` round-trips `format` through both the settings file and a saved preset, and
  an older file without the field still loads — the forgiving-load property `Default` exists
  for.

## Accepted caveats

1. **No alpha.** The pipeline is RGB; PNG's transparency is unused.
2. **Compression is not exposed** — PNG level and TIFF compression use `image`'s defaults. A
   TIFF may therefore be large. Exposing them is a separate, easy addition if wanted.
3. **JPEG stays 8-bit**, by definition of the format.
4. **`<stem>.<ext>` naming, unchanged** — no templating, no per-file naming. Still listed as a
   `batch_export` caveat.
5. **The profile is sRGB only.** There is no wider-gamut output option, because the pipeline's
   working space is sRGB primaries — that is a colour-management feature, not an export one.
