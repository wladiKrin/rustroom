# PNG / TIFF Export Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Export to PNG and TIFF at 8 or 16 bits, with an sRGB ICC profile embedded in every format including JPEG.

**Architecture:** A flat `ExportFormat` enum in the `export` crate (which owns encoding), one `save` dispatcher replacing `save_jpeg`, and `to_rgb16` as the only new pixel code. `lcms2` emits a static sRGB profile once, attached via `ImageEncoder::set_icc_profile` — supported by all three encoders. `ExportOptions` carries the format so presets do; `dest_for` takes it, which is the change that moves *which files collide*.

**Tech Stack:** Rust, `image` 0.25 (tiff/jpeg/png features already enabled), `lcms2`, `serde`, `egui`.

**Spec:** `docs/superpowers/specs/2026-08-11-png-tiff-export-design.md`

---

## Build environment

⚠ **The setup line grows** — lcms2 is installed but off the default pkg-config path, like libraw:

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

Verified: `pkg-config --modversion lcms2` → 2.19 with that path.

⚠ **CI is stricter**: `cargo clippy --all-targets --all-features`, unpinned `stable`. **Verify by hunting**, not once — four flaky tests in this codebase never showed on a single run:

```bash
for i in $(seq 1 20); do cargo test --all 2>&1 | grep "test result: FAILED"; done
```

## File structure

| File | Change | Responsibility |
|---|---|---|
| `crates/export/src/lib.rs` | modify | `ExportFormat`, `to_rgb16`, `save`, `srgb_icc` |
| `crates/export/Cargo.toml` | modify | `serde`, `lcms2` |
| `Cargo.toml` | modify | `lcms2` workspace dep |
| `crates/app/src/batch_export.rs` | modify | `ExportOptions.format`; `dest_for` takes it; `export_one` calls `save` |
| `crates/app/src/main.rs` | modify | format dropdown; quality greys out |
| `.github/workflows/ci.yml` | modify | `liblcms2-dev` |
| `CLAUDE.md` | modify | status, setup line, next action |

---

## Task 1: `ExportFormat`, `to_rgb16`, and the `save` dispatcher

No ICC yet — that is Task 2, so a failure there is isolated from the format work.

**Files:**
- Modify: `crates/export/src/lib.rs`, `crates/export/Cargo.toml`

- [ ] **Step 1: Write the failing tests**

Append inside `mod tests` in `crates/export/src/lib.rs`:

```rust
    fn ramp(w: usize, h: usize) -> ImageBuf {
        let mut img = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let v = x as f32 / (w - 1).max(1) as f32;
                img.set_pixel(x, y, [v, 1.0 - v, 0.5]);
            }
        }
        img
    }

    fn tmp(name: &str) -> std::path::PathBuf {
        // Structural uniqueness, not clock-based: `cargo test` runs these in parallel and
        // this codebase has already had a flaky test from two of them sharing a temp path.
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let i = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir().join(format!("rustroom_export_{}_{i}_{name}", std::process::id()))
    }

    #[test]
    fn to_rgb16_quantizes_and_clamps() {
        let mut img = ImageBuf::new(2, 1);
        img.set_pixel(0, 0, [0.0, 0.5, 1.0]);
        img.set_pixel(1, 0, [-0.3, 1.4, 0.25]);
        assert_eq!(
            to_rgb16(&img),
            vec![0, 32768, 65535, 0, 65535, 16384]
        );
    }

    #[test]
    fn extensions_and_lossiness_are_fixed() {
        assert_eq!(ExportFormat::Jpeg.extension(), "jpg");
        assert_eq!(ExportFormat::Png8.extension(), "png");
        assert_eq!(ExportFormat::Png16.extension(), "png");
        assert_eq!(ExportFormat::Tiff8.extension(), "tif");
        assert_eq!(ExportFormat::Tiff16.extension(), "tif");
        assert!(ExportFormat::Jpeg.is_lossy());
        for f in ExportFormat::ALL {
            assert_eq!(f.is_lossy(), f == ExportFormat::Jpeg);
        }
        assert_eq!(ExportFormat::default(), ExportFormat::Jpeg);
    }

    #[test]
    fn every_format_writes_a_readable_file() {
        let img = ramp(8, 4);
        for f in ExportFormat::ALL {
            let path = tmp(&format!("all.{}", f.extension()));
            save(&img, &path, f, 90).unwrap_or_else(|e| panic!("{f:?}: {e}"));
            let back = image::open(&path).unwrap_or_else(|e| panic!("{f:?} reopen: {e}"));
            assert_eq!((back.width(), back.height()), (8, 4), "{f:?}");
            let _ = std::fs::remove_file(&path);
        }
    }

    #[test]
    fn sixteen_bit_output_really_carries_more_than_eight() {
        // ⚠ The test that matters. A silent 8-bit downgrade passes every other test here —
        // the file still opens, still has the right size, still looks right. This writes a
        // value 8 bits cannot represent and checks it survived the round trip.
        //
        // 1/512 = 0.001953 quantizes to 0 or 1/255 in 8-bit; in 16-bit it is 128.
        let mut img = ImageBuf::new(1, 1);
        img.set_pixel(0, 0, [1.0 / 512.0, 1.0 / 512.0, 1.0 / 512.0]);

        for f in [ExportFormat::Png16, ExportFormat::Tiff16] {
            let path = tmp(&format!("deep.{}", f.extension()));
            save(&img, &path, f, 90).unwrap();
            let back = image::open(&path).unwrap().to_rgb16();
            assert_eq!(back.get_pixel(0, 0)[0], 128, "{f:?} lost precision");
            let _ = std::fs::remove_file(&path);
        }

        // And the 8-bit formats genuinely cannot: the same value rounds to 0.
        for f in [ExportFormat::Png8, ExportFormat::Tiff8] {
            let path = tmp(&format!("shallow.{}", f.extension()));
            save(&img, &path, f, 90).unwrap();
            let back = image::open(&path).unwrap().to_rgb8();
            assert_eq!(back.get_pixel(0, 0)[0], 0, "{f:?}");
            let _ = std::fs::remove_file(&path);
        }
    }

    #[test]
    fn export_format_round_trips_json() {
        // The name is part of the on-disk format for settings and export presets.
        for f in ExportFormat::ALL {
            let back: ExportFormat =
                serde_json::from_str(&serde_json::to_string(&f).unwrap()).unwrap();
            assert_eq!(f, back);
        }
        assert_eq!(
            serde_json::to_string(&ExportFormat::Tiff16).unwrap(),
            "\"Tiff16\""
        );
    }
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test -p export 2>&1 | grep -E "^error" | head -3
```

Expected: `cannot find type 'ExportFormat' in this scope`.

- [ ] **Step 3: Add the dependencies**

`crates/export/Cargo.toml`:

```toml
serde = { workspace = true }

[dev-dependencies]
serde_json = { workspace = true }
```

- [ ] **Step 4: Implement**

Replace `save_jpeg` in `crates/export/src/lib.rs` with the format type, `to_rgb16` and the dispatcher. Keep `to_rgb8` as it is.

```rust
use image::{ExtendedColorType, ImageEncoder};
use serde::{Deserialize, Serialize};

/// What to encode, and at what depth.
///
/// **Flat rather than `format` + `bit_depth`.** There is no such thing as a 16-bit JPEG, and a
/// separate depth field would need a rule saying so; flattening makes the invalid state
/// unrepresentable — the same reasoning behind `Decision::Cancel` carrying no `apply_to_all`.
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
    pub const ALL: [ExportFormat; 5] = [
        ExportFormat::Jpeg,
        ExportFormat::Png8,
        ExportFormat::Png16,
        ExportFormat::Tiff8,
        ExportFormat::Tiff16,
    ];

    /// ⚠ `tif` not `tiff`, `jpg` not `jpeg` — fixed so existing exports keep their shape.
    /// Both spellings are equally valid; this one is simply chosen.
    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Jpeg => "jpg",
            ExportFormat::Png8 | ExportFormat::Png16 => "png",
            ExportFormat::Tiff8 | ExportFormat::Tiff16 => "tif",
        }
    }

    /// Whether `quality` means anything. Only JPEG is lossy, and this is what greys the
    /// quality slider out rather than leaving it implying it does something.
    pub fn is_lossy(self) -> bool {
        matches!(self, ExportFormat::Jpeg)
    }

    pub fn label(self) -> &'static str {
        match self {
            ExportFormat::Jpeg => "JPEG",
            ExportFormat::Png8 => "PNG 8-bit",
            ExportFormat::Png16 => "PNG 16-bit",
            ExportFormat::Tiff8 => "TIFF 8-bit",
            ExportFormat::Tiff16 => "TIFF 16-bit",
        }
    }
}

/// Quantize a display-encoded image to interleaved 16-bit RGB, clamping to `[0, 1]`.
///
/// The extra bits are **real**: the pipeline is `f32` end to end and libraw hands us 16-bit
/// data, so a 16-bit file carries precision an 8-bit one discards. Returns `Vec<u16>` rather
/// than bytes so it stays directly testable; the encoder wants native-endian bytes.
pub fn to_rgb16(image: &ImageBuf) -> Vec<u16> {
    image
        .data()
        .iter()
        .map(|&v| (v.clamp(0.0, 1.0) * 65535.0).round() as u16)
        .collect()
}

/// Native-endian bytes of [`to_rgb16`], which is what `image`'s `Rgb16` encoder expects.
fn rgb16_ne_bytes(image: &ImageBuf) -> Vec<u8> {
    to_rgb16(image)
        .into_iter()
        .flat_map(|v| v.to_ne_bytes())
        .collect()
}

/// Encode a display-referred image. `quality` is ignored unless the format is lossy.
pub fn save(
    image: &ImageBuf,
    path: &Path,
    format: ExportFormat,
    quality: u8,
) -> Result<(), ExportError> {
    let (w, h) = (image.width() as u32, image.height() as u32);
    let file =
        std::io::BufWriter::new(std::fs::File::create(path).map_err(image::ImageError::IoError)?);

    match format {
        ExportFormat::Jpeg => {
            let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(file, quality);
            enc.write_image(&to_rgb8(image), w, h, ExtendedColorType::Rgb8)?;
        }
        ExportFormat::Png8 => {
            let enc = image::codecs::png::PngEncoder::new(file);
            enc.write_image(&to_rgb8(image), w, h, ExtendedColorType::Rgb8)?;
        }
        ExportFormat::Png16 => {
            let enc = image::codecs::png::PngEncoder::new(file);
            enc.write_image(&rgb16_ne_bytes(image), w, h, ExtendedColorType::Rgb16)?;
        }
        ExportFormat::Tiff8 => {
            let enc = image::codecs::tiff::TiffEncoder::new(file);
            enc.write_image(&to_rgb8(image), w, h, ExtendedColorType::Rgb8)?;
        }
        ExportFormat::Tiff16 => {
            let enc = image::codecs::tiff::TiffEncoder::new(file);
            enc.write_image(&rgb16_ne_bytes(image), w, h, ExtendedColorType::Rgb16)?;
        }
    }
    Ok(())
}
```

⚠ Delete `save_jpeg` rather than keeping it as a wrapper. One entry point means no call site can branch on format by accident, and the compiler will point at every caller.

- [ ] **Step 5: Run**

```bash
cargo test -p export 2>&1 | tail -10
```

Expected: `test result: ok.`

If `sixteen_bit_output_really_carries_more_than_eight` fails with 0 or 257, the 16-bit path is falling back to 8-bit — check `ExtendedColorType::Rgb16` is being passed and the buffer really is 2 bytes per sample.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add -A
git commit -m "feat(export): PNG and TIFF output at 8 and 16 bits"
```

---

## Task 2: The sRGB profile

**Files:**
- Modify: `Cargo.toml`, `crates/export/Cargo.toml`, `crates/export/src/lib.rs`, `.github/workflows/ci.yml`

- [ ] **Step 1: Add the dependency and confirm the API**

Workspace `Cargo.toml`:

```toml
lcms2 = "6"
```

`crates/export/Cargo.toml`:

```toml
lcms2 = { workspace = true }
```

Then confirm it builds and check the API — **do not guess it**:

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo build -p export 2>&1 | tail -5
grep -rn "pub fn new_srgb\|pub fn icc" ~/.cargo/registry/src/*/lcms2-*/src/profile.rs | head -5
```

Expected: a `Profile::new_srgb()` constructor and an `icc()` returning the serialized bytes. **If the names differ, use what the source says** — the two calls below are the only place it matters.

- [ ] **Step 2: Write the failing test**

Append inside `mod tests`:

```rust
    #[test]
    fn every_format_embeds_an_srgb_profile() {
        // ⚠ A 16-bit TIFF is an archival format; shipping it untagged means the consumer
        // guesses. JPEG is included because it has been shipping untagged all along, and an
        // inconsistency nobody can explain is worse than either choice.
        use image::ImageDecoder;
        assert!(!srgb_icc().is_empty(), "the profile should not be empty");

        let img = ramp(8, 4);
        for f in ExportFormat::ALL {
            let path = tmp(&format!("icc.{}", f.extension()));
            save(&img, &path, f, 90).unwrap();
            let mut dec = image::ImageReader::open(&path)
                .unwrap()
                .with_guessed_format()
                .unwrap()
                .into_decoder()
                .unwrap();
            let icc = dec.icc_profile().unwrap();
            assert!(
                icc.is_some_and(|p| !p.is_empty()),
                "{f:?} carries no ICC profile"
            );
            let _ = std::fs::remove_file(&path);
        }
    }
```

- [ ] **Step 3: Run to verify it fails**

```bash
cargo test -p export every_format_embeds 2>&1 | grep -E "^error|carries no ICC|test result" | head -3
```

Expected: `cannot find function 'srgb_icc'`, or — once that exists but is unused — `carries no ICC profile`.

- [ ] **Step 4: Implement**

Add to `crates/export/src/lib.rs`:

```rust
/// The sRGB profile every export is tagged with, serialized once.
///
/// ⚠ lcms2 is used here for **exactly one thing**: emitting a static sRGB profile. Input
/// profiles, conversions and a working-space policy remain deferred — this does not un-defer
/// colour management, it only stops us shipping untagged files.
///
/// Falls back to an empty slice if lcms2 ever fails to serialize, because an export that
/// writes an untagged file is far better than one that refuses to write at all.
pub fn srgb_icc() -> &'static [u8] {
    static ICC: std::sync::OnceLock<Vec<u8>> = std::sync::OnceLock::new();
    ICC.get_or_init(|| {
        lcms2::Profile::new_srgb()
            .icc()
            .unwrap_or_else(|_| Vec::new())
    })
}
```

and attach it in each arm of `save`. The encoders take it before `write_image` consumes them:

```rust
        ExportFormat::Jpeg => {
            let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(file, quality);
            enc.set_icc_profile(srgb_icc().to_vec())
                .map_err(image::ImageError::Unsupported)?;
            enc.write_image(&to_rgb8(image), w, h, ExtendedColorType::Rgb8)?;
        }
```

⚠ Propagate the `set_icc_profile` error rather than ignoring it. All three encoders support it
today; if that ever changes we want a loud failure, not silently untagged files.

Apply the same two lines to the other four arms.

- [ ] **Step 5: Run**

```bash
cargo test -p export 2>&1 | tail -8
```

Expected: `test result: ok.`

- [ ] **Step 6: Teach CI about lcms2**

In `.github/workflows/ci.yml`:

```yaml
        run: sudo apt-get update && sudo apt-get install -y libraw-dev liblcms2-dev pkg-config
```

⚠ If CI still fails to find lcms2, the `lcms2` crate can build vendored — switch to its static
feature rather than fighting the package name.

- [ ] **Step 7: Commit**

```bash
cargo fmt --all
git add -A
git commit -m "feat(export): embed an sRGB ICC profile in every exported format"
```

---

## Task 3: Carry the format through `batch_export`

**Files:**
- Modify: `crates/app/src/batch_export.rs`

- [ ] **Step 1: Write the failing tests**

In `crates/app/src/batch_export.rs`'s `mod tests`, **replace** `two_sources_with_the_same_stem_collide_in_an_empty_folder` with a format-parameterised version, and add an extension test:

```rust
    #[test]
    fn two_sources_with_the_same_stem_collide_in_an_empty_folder() {
        // ⚠ `a.RAF` and `a.jpg` both map to `a.jpg`, so the check has to consider names
        // claimed *in this run*, not only files already on disk.
        //
        // Parameterised by format because the extension decides **which** pair collides:
        // exporting PNG, `a.RAF` and `a.jpg` no longer do — but `a.RAF` and `a.png` now do.
        // The property is format-independent; only the triggering pair moves.
        for (format, other) in [
            (ExportFormat::Jpeg, "a.jpg"),
            (ExportFormat::Png8, "a.png"),
            (ExportFormat::Tiff16, "a.tif"),
        ] {
            let d = TempDir::new(&format!("same_stem_{}", format.extension()));
            let (report, _) = run_export(
                jobs(&d, &["a.RAF", other]),
                ExportOptions {
                    format,
                    ..opts(&d, None)
                },
                vec![Decision::Resolve {
                    collision: Collision::KeepBoth,
                    apply_to_all: false,
                }],
                ok_decode(),
            );
            let ext = format.extension();
            assert_eq!(report.exported, 2, "{format:?}");
            assert!(
                d.0.join(format!("a.{ext}")).exists()
                    && d.0.join(format!("a-1.{ext}")).exists(),
                "{format:?}"
            );
        }
    }

    #[test]
    fn the_destination_extension_follows_the_format() {
        let dir = PathBuf::from("/out");
        for f in ExportFormat::ALL {
            assert_eq!(
                dest_for(&dir, Path::new("/in/DSCF6509.RAF"), 0, f),
                dir.join(format!("DSCF6509.{}", f.extension())),
                "{f:?}"
            );
            assert_eq!(
                dest_for(&dir, Path::new("/in/a.RAF"), 3, f),
                dir.join(format!("a-3.{}", f.extension())),
                "{f:?}"
            );
        }
    }
```

Also append to `crates/app/src/settings.rs`'s `mod tests`:

```rust
    #[test]
    fn settings_round_trip_carries_the_export_format() {
        let f = TempFile::new("format_rt");
        let store = SettingsStore::new(f.0.clone());
        let s = Settings {
            export: ExportOptions {
                format: export::ExportFormat::Tiff16,
                ..ExportOptions::default()
            },
            ..Settings::default()
        };
        store.save(&s).unwrap();
        assert_eq!(store.load().export.format, export::ExportFormat::Tiff16);
    }

    #[test]
    fn a_settings_file_from_before_the_export_format_still_loads() {
        // The forgiving-load property `ExportOptions::default()` exists for.
        let f = TempFile::new("format_old");
        std::fs::write(&f.0, r#"{"export":{"quality":80}}"#).unwrap();
        let loaded = SettingsStore::new(f.0.clone()).load();
        assert_eq!(loaded.export.quality, 80, "the old field must survive");
        assert_eq!(loaded.export.format, export::ExportFormat::default());
    }
```

and to `crates/app/src/export_presets.rs`'s `mod tests`:

```rust
    #[test]
    fn a_preset_carries_the_export_format() {
        let d = TempDir::new("format");
        let store = d.store();
        store
            .save(&ExportPreset {
                name: "Archive".into(),
                options: ExportOptions {
                    format: export::ExportFormat::Tiff16,
                    ..ExportOptions::default()
                },
            })
            .unwrap();
        assert_eq!(
            store.list()[0].options.format,
            export::ExportFormat::Tiff16
        );
    }
```

- [ ] **Step 2: Run to verify they fail**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo test -p app --lib batch_export 2>&1 | grep -E "^error" | head -3
```

Expected: `this function takes 3 arguments but 4 were supplied`, and `ExportOptions` has no field `format`.

- [ ] **Step 3: Add the field and thread it through**

In `ExportOptions`:

```rust
    /// The output format and bit depth.
    pub format: export::ExportFormat,
```

in its `Default`: `format: export::ExportFormat::default(),`

`dest_for` gains the parameter and uses it:

```rust
pub fn dest_for(dir: &Path, source: &Path, index: usize, format: ExportFormat) -> PathBuf {
    let stem = source
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("export");
    let ext = format.extension();
    let name = if index == 0 {
        format!("{stem}.{ext}")
    } else {
        format!("{stem}-{index}.{ext}")
    };
    dir.join(name)
}
```

⚠ **Both** call sites in `run` pass `opts.format` — the initial `dest_for(&opts.dest_dir, &source, 0, opts.format)` and the `KeepBoth` loop probing `-1`, `-2`, ….

`export_one` calls the dispatcher:

```rust
    export::save(&rendered, dest, opts.format, opts.quality).map_err(|e| e.to_string())
```

Add `use export::ExportFormat;` to the imports.

⚠ Several tests construct `ExportOptions` exhaustively and will stop compiling; add
`..ExportOptions::default()`. Other `dest_for` call sites in tests need the extra argument.

- [ ] **Step 4: Run**

```bash
cargo test -p app --lib 2>&1 | grep -E "^test result|^error" -A 3 | head -6
```

Expected: `test result: ok.`

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add -A
git commit -m "feat(app): carry the export format through options, presets and dest_for"
```

---

## Task 4: The dropdown

**Files:**
- Modify: `crates/app/src/main.rs`

- [ ] **Step 1: Add the control and gate quality on it**

In `export_modal`, replace the quality slider line with a format picker above it:

```rust
            ui.horizontal(|ui| {
                ui.label("Format");
                egui::ComboBox::from_id_salt("format_pick")
                    .selected_text(opts.format.label())
                    .show_ui(ui, |ui| {
                        for f in export::ExportFormat::ALL {
                            ui.selectable_value(&mut opts.format, f, f.label());
                        }
                    });
            });
            // Greyed out rather than hidden for the non-lossy formats: the value is still
            // there and comes back when JPEG is reselected — the same treatment NR's Detail
            // and Sharpening's Masking get for being inert.
            ui.add_enabled(
                opts.format.is_lossy(),
                egui::Slider::new(&mut opts.quality, 1..=100).text("JPEG quality"),
            );
```

⚠ No import needed: `export` is already a dependency of `app`, so `export::ExportFormat` resolves by path.

- [ ] **Step 2: Verify the workspace**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo fmt --all && cargo fmt --all --check && echo "FMT OK"
cargo clippy --all-targets --all-features -- -D warnings 2>&1 | grep -cE "^(error|warning: )"
cargo test --all 2>&1 | grep -E "^test result"
for i in $(seq 1 20); do cargo test --all 2>&1 | grep "test result: FAILED"; done; echo done
```

Expected: fmt clean, clippy `0`, all green, no flakes.

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "feat(app): output format dropdown in the export dialog"
```

---

## Task 5: Validate on a real photo, then document

- [ ] **Step 1: Export the same frame in three formats**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo run --release -p app
```

Export one photo as **JPEG**, **PNG 16-bit** and **TIFF 16-bit** into the same folder, then:

```bash
ls -la <dest>/          # sizes: TIFF 16 should dwarf the JPEG
sips -g all <dest>/*.tif | grep -i "bits\|profile\|space"
sips -g all <dest>/*.png | grep -i "bits\|profile\|space"
```

Check: the TIFF and PNG report **16 bits per sample**, and all three report a colour profile.
⚠ `sips` is the honest check here — it reads the file the way another application would,
rather than trusting our own decoder round trip.

- [ ] **Step 2: Confirm the quality slider greys out**

Switch the dropdown to PNG 16-bit — the JPEG quality slider must go inactive, and switching
back to JPEG must restore the previous value rather than resetting it.

- [ ] **Step 3: Confirm a preset carries the format**

Save a preset as TIFF 16-bit, change the dropdown to JPEG, re-pick the preset — it should come
back as TIFF 16-bit.

- [ ] **Step 4: Update `CLAUDE.md`**

- **The setup line** — it now needs the little-cms2 pkg-config path. This is the single most
  likely thing to waste a future session's time, so update it in the dev-setup note *and* the
  RESUME block's run command.
- Crate layout: `export` now owns `ExportFormat`, `to_rgb16`, `save`, `srgb_icc`.
- A "**PNG/TIFF export done**" section: the flat enum and why; that 16-bit precision is real
  rather than interpolated; **lcms2 used only to emit a static sRGB profile, colour management
  still deferred**; that JPEG got tagged too; and the `dest_for` collision consequence with the
  parameterised test.
- Caveats: no alpha, compression not exposed, sRGB only.
- Next action — candidates with their measured case: a **second prefix cache checkpoint**
  (dehaze drags cost 87.6 ms against 13.7 ms), **parallelising the RGBA8 pack** (~35% of a
  cached frame), a **background full-res decode for true 1:1** (now ~2.9 s rather than ~11.4
  at the `Fast` demosaic default), or **Phase 3**.
- Add the spec and plan paths to the docs list.

- [ ] **Step 5: Final check and commit**

```bash
cargo fmt --all --check && cargo clippy --all-targets --all-features -- -D warnings
cargo test --all 2>&1 | tail -5
git add -A
git commit -m "docs: record PNG/TIFF export with embedded sRGB profiles"
```

**Do not push.** The user pushes to `github.com/wladiKrin/rustroom` themselves.

---

## Definition of done

- [ ] `cargo fmt --all --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --all` green
- [ ] 20 consecutive full-suite runs with no failures
- [ ] `sixteen_bit_output_really_carries_more_than_eight` passing — the guard against a silent 8-bit downgrade
- [ ] Every format verified to carry a non-empty ICC profile, by reading the file back
- [ ] A real TIFF and PNG confirmed as 16-bit **by `sips`**, not only by our own decoder
- [ ] The quality slider greys out for non-lossy formats and keeps its value
- [ ] An export preset round-trips the format
- [ ] `CLAUDE.md`'s setup line includes the little-cms2 pkg-config path
