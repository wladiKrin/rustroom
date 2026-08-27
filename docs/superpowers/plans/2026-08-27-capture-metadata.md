# Capture metadata — M4.3b, Plan 1 of 2

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Read date taken, camera, lens, ISO, shutter, aperture and focal length from every photo and store them in the catalog, without making a folder take ten seconds to open.

**Architecture:** A `Capture` struct in `core-image`, produced by a reader in each decoder crate, stored in a new per-file `photo_capture` table at schema v4, filled by a background pass modelled on `ThumbLoader`.

**Tech Stack:** Rust, `rsraw` (already present), **`kamadak-exif` 0.6** (new, pure Rust), `rusqlite`. No C library, no `pkg-config` change, no CI change.

**Spec:** `docs/superpowers/specs/2026-08-27-capture-metadata-design.md` — **read its three probe findings before Task 2.** They are not guessable from the documentation.

**Plan 2 (filtering) is a separate document.** Do not start it here.

---

## Ground rules for this repo

- **Branch:** `m4.3b-capture-metadata`. **Never push** — the user pushes.
- **Commit messages carry no `Co-Authored-By` trailer.**
- **Build env** — every `cargo` command needs this first, in the same shell:
  ```bash
  export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
  ```
- **The GUI binary is `Rustroom`**: bin tests are `cargo test -p app --bin Rustroom`.
- **⚠ In a binary crate an item lands with its first caller** — eleventh instance of that trap.
- **⚠ Check exit codes, never grep output.**
- **⚠ The sabotage compile check is "did a `test result:` line appear at all"** — **not** a grep for
  `^error:`, which matches cargo's ordinary `error: test failed, to rerun pass …` line and gave a
  false "did not compile" three times two slices ago.
- ⚠ `cargo clippy --all-targets --all-features` and `cargo test --all` each take **over two
  minutes** here. Give them a generous timeout rather than assuming a hang.

Verification command used throughout:

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
```

### Already done during design, and already on the branch

`kamadak-exif` is added to `image-decode/Cargo.toml`, and three probe examples exist and are kept:
`raw-decode/examples/bench_metadata.rs`, `raw-decode/examples/raw_meta.rs`,
`image-decode/examples/exif_probe.rs`. **Do not delete them** — they are how the spec's three
findings were found and how a future reader re-checks them.

---

## File Structure

| file | responsibility | new? |
|---|---|---|
| `crates/core-image/src/lib.rs` | `Capture`, beside `Thumbnail` and for the same reason | modify |
| `crates/raw-decode/src/lib.rs` (+ `backend_rsraw.rs`) | `read_capture` for RAW | modify |
| `crates/image-decode/src/lib.rs` | `read_capture` for JPEG/TIFF/PNG | modify |
| `crates/catalog/Cargo.toml`, `src/db.rs` | schema v4, `set_capture`, `capture_of`, `files_without_capture` | modify |
| `crates/app/src/metadata_scanner.rs` | the background pass | **create** |
| `crates/app/src/main.rs`, `shell.rs` | own the scanner, drain it per frame | modify |

---

## Task 1: `Capture` in `core-image`

**Files:**
- Modify: `crates/core-image/src/lib.rs`

- [ ] **Step 1: Add the type**

Beside `Thumbnail`:

```rust
/// What a photograph records about how it was taken.
///
/// ⚠ It lives here, beside [`Thumbnail`], for the reason `Thumbnail` does: **both decoder crates
/// produce it**, and neither may depend on the other.
///
/// ⚠⚠ **Every field is `Option`, because a file with no EXIF is normal rather than an error** — a
/// scan, an export or a screenshot strips it. A `Capture` with every field `None` is a valid
/// answer and must still be *stored*, or the background pass would re-read that file on every
/// folder open forever.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Capture {
    /// When the shutter fired, as **naive local wall-clock** seconds since the epoch.
    ///
    /// ⚠⚠ Wall-clock, not a true instant, and that is deliberate. EXIF's `DateTimeOriginal`
    /// carries no timezone at all while libraw's does, so storing true UTC would make a RAW exact
    /// and its JPEG sibling a guess about the machine's zone — the pair could differ by hours.
    /// Wall-clock also *is* what a photographer means by "date taken": the shots from the 17th
    /// means the 17th where they were standing.
    pub taken: Option<i64>,
    /// `"{MAKE} {model}"`, the make uppercased. See [`Capture::camera_name`].
    pub camera: Option<String>,
    pub lens: Option<String>,
    pub iso: Option<u32>,
    /// Exposure time in seconds.
    pub shutter: Option<f32>,
    pub aperture: Option<f32>,
    /// Focal length in millimetres.
    pub focal_len: Option<f32>,
}

impl Capture {
    /// Build the stored camera name from a make and a model.
    ///
    /// ⚠⚠ **The make is uppercased, and that is load-bearing.** The two readers disagree about it:
    /// for one and the same shot, libraw says `"Fujifilm"` and EXIF says `"FUJIFILM"`. Without
    /// normalizing, a RAW+JPEG pair from one body appears as **two cameras** in a filter — the
    /// same class of problem colour-management C4 solved for profile matching, except here it is
    /// our own two readers disagreeing with each other.
    ///
    /// Uppercase rather than a heuristic because it is deterministic, needs no alias table, and is
    /// what most cameras write in EXIF and print on themselves.
    pub fn camera_name(make: &str, model: &str) -> Option<String> {
        let make = make.trim();
        let model = model.trim();
        match (make.is_empty(), model.is_empty()) {
            (true, true) => None,
            (true, false) => Some(model.to_string()),
            (false, true) => Some(make.to_uppercase()),
            (false, false) => Some(format!("{} {model}", make.to_uppercase())),
        }
    }

    /// True when nothing at all was read. ⚠ Still worth storing — see the type's note.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}
```

- [ ] **Step 2: Write the tests**

In `core-image`'s test module:

```rust
    #[test]
    fn the_two_readers_agree_on_one_camera() {
        // ⚠⚠ The finding this normalization exists for: for the *same shot*, libraw reports
        // "Fujifilm" and EXIF reports "FUJIFILM". Unnormalized, one body shows as two cameras.
        let from_raw = Capture::camera_name("Fujifilm", "X-T4");
        let from_jpeg = Capture::camera_name("FUJIFILM", "X-T4");
        assert_eq!(from_raw, from_jpeg);
        assert_eq!(from_raw.as_deref(), Some("FUJIFILM X-T4"));
    }

    #[test]
    fn a_missing_half_of_the_camera_name_still_yields_something() {
        assert_eq!(Capture::camera_name("", "X-T4").as_deref(), Some("X-T4"));
        assert_eq!(Capture::camera_name("Canon", "").as_deref(), Some("CANON"));
        assert_eq!(Capture::camera_name("  ", " "), None);
    }

    #[test]
    fn an_empty_capture_is_recognisable_but_still_a_value() {
        // ⚠ It must be storable: a file with no EXIF that is not recorded would be re-read on
        // every folder open forever.
        assert!(Capture::default().is_empty());
        assert!(!Capture {
            iso: Some(100),
            ..Default::default()
        }
        .is_empty());
    }
```

- [ ] **Step 3: Verify and commit**

```bash
cargo test -p core-image 2>&1 | grep -E "^test result:"
```

Expected: three more than before.

```bash
git add crates/core-image/src/lib.rs
git commit -m "feat(core-image): a Capture type both decoders can produce"
```

---

## Task 2: The two readers

**Files:**
- Modify: `crates/raw-decode/src/lib.rs`, `crates/raw-decode/src/backend_rsraw.rs`, `crates/raw-decode/Cargo.toml`
- Modify: `crates/image-decode/src/lib.rs`

⚠ **Read the spec's "Three findings from probing the readers" first.** The padded lens value and
the timezone asymmetry are both invisible until they bite.

- [ ] **Step 1: The RAW reader**

`raw-decode` needs `chrono` (already in the tree via `rsraw`) to reach `.naive_local()`:

```bash
cargo add chrono --package raw-decode
```

In `backend_rsraw.rs`:

```rust
/// Read what the file records about how it was taken, without decoding it.
///
/// ⚠ Costs ~0.2 ms of parsing on top of reading the file, which is ~11–15 ms for a 29 MB RAF —
/// measured, and the reason the caller is a background pass rather than the folder scan.
pub fn read_capture(bytes: &[u8]) -> Result<Capture, DecodeError> {
    let raw = RawImage::open(bytes).map_err(|e| DecodeError::Backend(e.to_string()))?;
    let lens = raw.lens_info();
    Ok(Capture {
        // ⚠⚠ `.naive_local()`, NOT the UTC instant. EXIF carries no timezone, so storing true
        // instants would make a RAW exact and its JPEG sibling a guess — see `Capture::taken`.
        taken: raw.datetime().map(|d| d.naive_local().and_utc().timestamp()),
        camera: Capture::camera_name(&raw.normalized_make(), &raw.normalized_model()),
        lens: non_empty(&lens.lens_name),
        iso: non_zero_u32(raw.iso_speed()),
        shutter: non_zero_f32(raw.shutter()),
        aperture: non_zero_f32(raw.aperture()),
        focal_len: non_zero_f32(raw.focal_len()),
    })
}

/// ⚠ libraw reports **0** for a value it does not have, not an absence. Storing 0 would make
/// "f/0" and "ISO 0" real-looking values in a filter.
fn non_zero_u32(v: u32) -> Option<u32> {
    (v != 0).then_some(v)
}

fn non_zero_f32(v: f32) -> Option<f32> {
    (v.is_finite() && v > 0.0).then_some(v)
}

fn non_empty(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}
```

⚠ **Check `DecodeError`'s actual variants** and use whichever this crate already uses for a backend
failure — the name above is a guess. And check whether `lens_info()` returns owned `String`s
(it does, per `rsraw::LensInfo`) so `&lens.lens_name` is right.

Re-export from `raw-decode/src/lib.rs`:

```rust
/// Read a RAW's capture metadata. ⚠ Reads the whole file: `RawImage::open` takes a `&[u8]`.
pub fn read_capture(path: &Path) -> Result<Capture, DecodeError> {
    let bytes = std::fs::read(path)?;
    backend_rsraw::read_capture(&bytes)
}
```

- [ ] **Step 2: The JPEG/TIFF reader**

In `image-decode/src/lib.rs`:

```rust
/// Read an image file's EXIF, if it has any.
///
/// ⚠ PNG and TIFF usually carry none. That is not an error: an empty `Capture` is a valid answer.
pub fn read_capture(path: &Path) -> Result<Capture, DecodeError> {
    let file = std::fs::File::open(path)?;
    let mut r = std::io::BufReader::new(&file);
    let Ok(ex) = exif::Reader::new().read_from_container(&mut r) else {
        // ⚠ No EXIF is normal, not a failure. Returning an error here would make the scanner
        // retry this file on every folder open forever.
        return Ok(Capture::default());
    };
    let make = string_of(&ex, exif::Tag::Make).unwrap_or_default();
    let model = string_of(&ex, exif::Tag::Model).unwrap_or_default();
    Ok(Capture {
        taken: ex
            .get_field(exif::Tag::DateTimeOriginal, exif::In::PRIMARY)
            .and_then(|f| match &f.value {
                exif::Value::Ascii(v) => v.first().cloned(),
                _ => None,
            })
            .and_then(|b| exif::DateTime::from_ascii(&b).ok())
            .and_then(naive_seconds),
        camera: Capture::camera_name(&make, &model),
        lens: string_of(&ex, exif::Tag::LensModel),
        iso: ex
            .get_field(exif::Tag::PhotographicSensitivity, exif::In::PRIMARY)
            .and_then(|f| f.value.get_uint(0)),
        shutter: rational_of(&ex, exif::Tag::ExposureTime),
        aperture: rational_of(&ex, exif::Tag::FNumber),
        focal_len: rational_of(&ex, exif::Tag::FocalLength),
    })
}

/// ⚠⚠ Reads `Field::value`, **never `display_value()`**. EXIF string fields are arrays, and a real
/// file returns the lens name followed by **51 empty strings** — `display_value()` renders all of
/// them, so a naïve implementation stores `"XF23mmF2 R WR", "", "", …`.
fn string_of(ex: &exif::Exif, tag: exif::Tag) -> Option<String> {
    let f = ex.get_field(tag, exif::In::PRIMARY)?;
    let exif::Value::Ascii(parts) = &f.value else {
        return None;
    };
    first_non_empty(parts)
}

/// The first non-empty entry of an EXIF string array.
///
/// ⚠ Split out from `string_of` **so it is testable without building an EXIF container**. The
/// padding is the whole finding, and a test that cannot reach it is not a test of it.
fn first_non_empty(parts: &[Vec<u8>]) -> Option<String> {
    parts
        .iter()
        .map(|b| String::from_utf8_lossy(b).trim().to_string())
        .find(|s| !s.is_empty())
}

fn rational_of(ex: &exif::Exif, tag: exif::Tag) -> Option<f32> {
    let f = ex.get_field(tag, exif::In::PRIMARY)?;
    let v = match &f.value {
        exif::Value::Rational(r) => r.first().map(|r| r.to_f64())?,
        exif::Value::SRational(r) => r.first().map(|r| r.to_f64())?,
        _ => return None,
    };
    (v.is_finite() && v > 0.0).then_some(v as f32)
}

/// ⚠ Interpreted as wall-clock, matching the RAW reader — see `Capture::taken`.
fn naive_seconds(dt: exif::DateTime) -> Option<i64> {
    use chrono::NaiveDate;
    NaiveDate::from_ymd_opt(dt.year as i32, dt.month as u32, dt.day as u32)?
        .and_hms_opt(dt.hour as u32, dt.minute as u32, dt.second as u32)
        .map(|d| d.and_utc().timestamp())
}
```

`image-decode` needs `chrono` too:

```bash
cargo add chrono --package image-decode
```

⚠ **Every one of `exif::Value`'s variant names, `Rational::to_f64`, `DateTime::from_ascii` and
`Field::value`'s shape must be checked against `kamadak-exif` 0.6.1 itself** — read
`~/.cargo/registry/src/*/kamadak-exif-0.6.1/src/`. The probe example proves the *tags* are present
and populated; it does not prove these API names. Fix the code to match reality, not the reverse.

- [ ] **Step 3: Test what can be tested without a fixture**

Both readers need real files, so the *gated* tests below are the only end-to-end proof. ⚠ **A gated
test skips without its fixture, so a green CI run does not mean it ran.** Say so in the comment.

In `raw-decode`:

```rust
    #[test]
    fn a_missing_value_is_none_rather_than_zero() {
        // ⚠ libraw reports 0 for a value it does not have. Storing 0 would put "f/0" and "ISO 0"
        // into a filter as real-looking options.
        assert_eq!(non_zero_u32(0), None);
        assert_eq!(non_zero_u32(1600), Some(1600));
        assert_eq!(non_zero_f32(0.0), None);
        assert_eq!(non_zero_f32(f32::NAN), None);
        assert_eq!(non_zero_f32(2.0), Some(2.0));
    }

    #[test]
    #[ignore = "needs RAW_FIXTURE"]
    fn a_real_raw_reports_its_capture() {
        let Ok(path) = std::env::var("RAW_FIXTURE") else {
            return;
        };
        let c = read_capture(std::path::Path::new(&path)).expect("read");
        assert!(c.taken.is_some(), "no capture time");
        assert!(c.camera.is_some(), "no camera");
        assert!(c.iso.is_some() && c.aperture.is_some(), "{c:?}");
        // ⚠ The normalization, end to end: whatever libraw's casing, the make is uppercase here.
        let cam = c.camera.unwrap();
        let make = cam.split_whitespace().next().unwrap();
        assert_eq!(make, make.to_uppercase(), "the make was not normalized");
    }
```

In `image-decode`:

```rust
    #[test]
    fn a_padded_string_field_is_read_as_one_value() {
        // ⚠⚠ The finding, and it is not hypothetical: for a real X-T4 JPEG, `LensModel` comes back
        // as the lens name followed by **51 empty strings**, and `display_value()` renders every
        // one of them. Reproduced here at the level the padding actually lives.
        let padded: Vec<Vec<u8>> = std::iter::once(b"XF23mmF2 R WR".to_vec())
            .chain(std::iter::repeat_n(Vec::new(), 51))
            .collect();
        assert_eq!(first_non_empty(&padded).as_deref(), Some("XF23mmF2 R WR"));
    }

    #[test]
    fn a_field_of_nothing_but_padding_is_none() {
        let empty: Vec<Vec<u8>> = std::iter::repeat_n(Vec::new(), 8).collect();
        assert_eq!(first_non_empty(&empty), None);
        assert_eq!(first_non_empty(&[]), None);
    }

    #[test]
    fn leading_padding_is_skipped_rather_than_taken() {
        // ⚠ "First non-empty", not "first" — a field whose first slot is blank must not read as
        // absent.
        let parts = vec![Vec::new(), b"  ".to_vec(), b"X-T4".to_vec()];
        assert_eq!(first_non_empty(&parts).as_deref(), Some("X-T4"));
    }
```

⚠ `std::iter::repeat_n` is stable since 1.82 and this project is on 1.98; if clippy prefers
`repeat(..).take(..)` on the pinned toolchain, follow clippy.

⚠ **An end-to-end JPEG test is deliberately not attempted here.** `image-decode`'s orientation
tests hand-build an EXIF APP1 segment because *the `image` crate reads EXIF but cannot write it* —
that was proportionate for eight orientation values sharing one two-byte field, and is not for
seven tags of three different types. The real end-to-end proof is the gated fixture test plus the
`exif_probe` example, and saying so is better than a test that exercises less than it appears to.

- [ ] **Step 4: Verify and commit**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
RAW_FIXTURE=~/Desktop/fuji/DSCF6223.RAF cargo test -p raw-decode -- --ignored
git add -A
git commit -m "feat(decode): read capture metadata from RAW and from EXIF"
```

---

## Task 3: Schema v4 and the catalog API

**Files:**
- Modify: `crates/catalog/Cargo.toml` (add `core-image`), `crates/catalog/src/db.rs`

- [ ] **Step 1: Write the failing tests**

```rust
    #[test]
    fn a_capture_round_trips() {
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF"]))
            .unwrap();
        let c = Capture {
            taken: Some(1_779_000_000),
            camera: Some("FUJIFILM X-T4".into()),
            lens: Some("XF23mmF2 R WR".into()),
            iso: Some(1600),
            shutter: Some(0.02),
            aperture: Some(2.0),
            focal_len: Some(23.0),
        };
        cat.set_capture(Path::new("/a/one.RAF"), &c).unwrap();
        assert_eq!(cat.capture_of(Path::new("/a/one.RAF")).unwrap(), Some(c));
    }

    #[test]
    fn an_empty_capture_is_still_recorded() {
        // ⚠⚠ A file with no EXIF must be *marked as read*, or the background pass re-reads it on
        // every folder open forever — 11–15 ms of file reading each time, for nothing.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF", "two.RAF"]))
            .unwrap();
        cat.set_capture(Path::new("/a/one.RAF"), &Capture::default())
            .unwrap();
        assert_eq!(
            cat.capture_of(Path::new("/a/one.RAF")).unwrap(),
            Some(Capture::default())
        );
        assert_eq!(
            cat.files_without_capture(Path::new("/a")).unwrap(),
            vec!["two.RAF".to_string()],
            "a file recorded as having no EXIF must not be queued again"
        );
    }

    #[test]
    fn capture_is_per_file_so_every_copy_shares_it() {
        // ⚠⚠ Per *file*, like `photo_keywords` and unlike marks: capture metadata describes the
        // **exposure**, which every copy of a file shares. A rating describes the image you made,
        // which they do not.
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF"]))
            .unwrap();
        cat.add_copy(Path::new("/a/one.RAF"), 1).unwrap();
        let c = Capture {
            iso: Some(400),
            ..Default::default()
        };
        cat.set_capture(Path::new("/a/one.RAF"), &c).unwrap();
        // One row, and it is found whichever copy is asked about — the query keys on the file.
        assert_eq!(cat.capture_of(Path::new("/a/one.RAF")).unwrap(), Some(c));
        assert_eq!(
            cat.files_without_capture(Path::new("/a")).unwrap(),
            Vec::<String>::new()
        );
    }

    #[test]
    fn forgetting_a_folder_cascades_to_its_capture_rows() {
        let cat = Catalog::open_in_memory().unwrap();
        cat.sync_folder(Path::new("/a"), &entries_in("/a", &["one.RAF"]))
            .unwrap();
        cat.set_capture(
            Path::new("/a/one.RAF"),
            &Capture {
                iso: Some(100),
                ..Default::default()
            },
        )
        .unwrap();
        let id = cat.folders().unwrap()[0].id;
        cat.forget_folder(id).unwrap();
        // ⚠ The raw count, not a join — the lesson from `forget_folder`'s own cascade test, where
        // every join-based assertion passed vacuously with the cascade disabled.
        let left: i64 = cat
            .conn
            .query_row("SELECT count(*) FROM photo_capture", [], |r| r.get(0))
            .unwrap();
        assert_eq!(left, 0, "orphaned capture rows survived");
    }
```

⚠ And **extend the v1 simulation's drop list** in
`a_version_one_catalog_upgrades_and_keeps_its_rows` with `DROP TABLE photo_capture;`. CLAUDE.md
records why: every new migration makes that test incomplete, and without the drop `migrate` re-runs
a step whose objects still exist and fails with "table already exists".

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test -p catalog 2>&1 | tail -20
```

Expected: compile errors — `set_capture`, `capture_of`, `files_without_capture` do not exist.

- [ ] **Step 3: Add the dependency and the migration**

`crates/catalog/Cargo.toml`:

```toml
core-image = { path = "../core-image" }
```

⚠ Its only dependency is `rayon`. The alternatives were an eight-parameter `set_capture` (which
trips `too_many_arguments`) or a duplicate row type.

Bump `SCHEMA_VERSION` to 4 and add a **new gate**:

```rust
        if from < 4 {
            // ⚠⚠ A **new gate**, never an edit to versions 1–3. A migration that has already run
            // on someone's machine cannot be changed.
            //
            // ⚠⚠ Keyed on the **file**, like `photo_keywords` and unlike marks: capture metadata
            // describes the *exposure*, which every copy of a file shares, while a rating
            // describes the image you made, which they do not. **Do not unify them.**
            //
            // ⚠ A row exists as soon as a file has been *read*, even if it had no EXIF at all —
            // every column nullable, and the row's presence is what stops it being re-read.
            self.conn.execute_batch(
                "BEGIN;
                 CREATE TABLE photo_capture (
                     folder_id INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
                     file_name TEXT NOT NULL,
                     taken     INTEGER,
                     camera    TEXT,
                     lens      TEXT,
                     iso       INTEGER,
                     shutter   REAL,
                     aperture  REAL,
                     focal_len REAL,
                     PRIMARY KEY (folder_id, file_name)
                 );
                 PRAGMA user_version = 4;
                 COMMIT;",
            )?;
        }
```

- [ ] **Step 4: Add the three methods**

Follow `assign_keyword`'s shape for resolving `(folder_id, file_name)` from a path — read it and
reuse whatever helper it uses (`split(photo)`), rather than writing a second one.

```rust
    /// Record what a file's EXIF said. ⚠ An all-`None` `Capture` is still stored: the row's
    /// presence is what marks the file as read.
    pub fn set_capture(&self, photo: &Path, c: &Capture) -> Result<(), CatalogError>;

    /// What was recorded for a file, or `None` if it has never been read.
    pub fn capture_of(&self, photo: &Path) -> Result<Option<Capture>, CatalogError>;

    /// The file names in `folder` with no capture row yet — the background pass's queue.
    ///
    /// ⚠ DISTINCT on `file_name`: `photos` has one row per (file, copy) and capture is per file,
    /// so without it a photo with three copies would be queued three times.
    pub fn files_without_capture(&self, folder: &Path) -> Result<Vec<String>, CatalogError>;
```

- [ ] **Step 5: Run, then sabotage**

```bash
cargo test -p catalog 2>&1 | grep -E "^test result:"
```

Then, each checked for a `test result:` line before believing it:

| sabotage | expected |
|---|---|
| `set_capture` early-returns when `c.is_empty()` | `an_empty_capture_is_still_recorded` fails |
| `files_without_capture` drops its `DISTINCT` | `capture_is_per_file_so_every_copy_shares_it` fails |
| remove `DROP TABLE photo_capture` from the v1 test | that test fails with "table already exists" |
| `PRAGMA foreign_keys = OFF` | `forgetting_a_folder_cascades_to_its_capture_rows` fails |

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(catalog): schema v4 stores per-file capture metadata"
```

---

## Task 4: The background pass

**Files:**
- Create: `crates/app/src/metadata_scanner.rs`
- Modify: `crates/app/src/lib.rs`, `crates/app/src/main.rs`, `crates/app/src/shell.rs`

- [ ] **Step 1: Write the scanner**

Model it on `thumb_loader.rs` — **read that file and follow its shape**: an injected reader fn and
thread count for testing, workers parked on a condvar, a `Drop` that wakes them, results drained
per frame.

```rust
//! A background pass that reads every photo's capture metadata into the catalog.
//!
//! ⚠⚠ **Its own pass, NOT piggybacked on `ThumbLoader`** — even though that already reads every
//! file — because thumbnails are **windowed**. Metadata attached to them would exist only for
//! photos that had been scrolled past, and *you cannot filter by a date that was never read*.
//! Accepted cost: a file is read twice on a first visit.
//!
//! ⚠ Measured: ~11–15 ms per RAW, almost all of it reading the file (libraw's parse is 0.2 ms).
//! ~10 s for a folder of 830, once, in the background.

pub struct MetadataResult {
    pub path: PathBuf,
    pub capture: Capture,
}

#[derive(Default)]
struct Queue {
    pending: VecDeque<PathBuf>,
    queued: HashSet<PathBuf>,
    stop: bool,
}

pub struct MetadataScanner {
    rx: Receiver<MetadataResult>,
    queue: Arc<(Mutex<Queue>, Condvar)>,
    _handles: Vec<JoinHandle<()>>,
}

impl MetadataScanner {
    pub fn new(on_progress: impl Fn() + Send + Sync + 'static) -> Self;
    pub fn with_reader(
        read: fn(&Path) -> Capture,
        threads: usize,
        on_progress: impl Fn() + Send + Sync + 'static,
    ) -> Self;
    /// Queue these paths. ⚠ **Additive**, unlike `ThumbLoader::want`, which *replaces*.
    /// ⚠ `queued` de-duplicates, so re-queuing a folder mid-pass does not read anything twice.
    pub fn scan(&self, paths: &[PathBuf]);
    pub fn drain(&self) -> Vec<MetadataResult>;
}
```

⚠⚠ **`scan` is additive where `ThumbLoader::want` replaces, and that difference is the point.** The
thumbnail queue is "what is on screen now", so replacing it *is* the cancellation. This queue is
"what still needs reading", and dropping it on a source switch would leave folders permanently
half-scanned.

⚠ The reader dispatches on extension, exactly as `thumb_loader::load_thumbnail` does:
`catalog::is_raw(path)` → `raw_decode::read_capture`, else `image_decode::read_capture`. A failure
yields `Capture::default()` rather than being dropped — **an unreadable file must still be marked
as read**, or it is retried forever.

- [ ] **Step 2: Test it with an injected reader**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn paths(n: usize) -> Vec<PathBuf> {
        (0..n)
            .map(|i| PathBuf::from(format!("/nonexistent/meta_{i:04}.raf")))
            .collect()
    }

    /// Collect until `want` results have arrived or the deadline passes.
    fn collect(s: &MetadataScanner, want: usize) -> Vec<MetadataResult> {
        let mut out = Vec::new();
        for _ in 0..400 {
            out.extend(s.drain());
            if out.len() >= want {
                break;
            }
            thread::sleep(std::time::Duration::from_millis(5));
        }
        out.sort_by(|a, b| a.path.cmp(&b.path));
        out
    }

    fn iso(_p: &Path) -> Capture {
        Capture {
            iso: Some(100),
            ..Default::default()
        }
    }

    #[test]
    fn every_queued_path_is_read_exactly_once() {
        static READS: AtomicUsize = AtomicUsize::new(0);
        fn counted(_p: &Path) -> Capture {
            READS.fetch_add(1, Ordering::SeqCst);
            Capture::default()
        }
        READS.store(0, Ordering::SeqCst);
        let s = MetadataScanner::with_reader(counted, 4, || {});
        s.scan(&paths(50));
        let got = collect(&s, 50);
        assert_eq!(got.len(), 50);
        assert_eq!(READS.load(Ordering::SeqCst), 50, "a file was read twice");
        let names: Vec<PathBuf> = got.iter().map(|r| r.path.clone()).collect();
        assert_eq!(names, paths(50));
    }

    #[test]
    fn a_second_scan_adds_to_the_queue_rather_than_replacing_it() {
        // ⚠⚠ The difference from `ThumbLoader::want`, which replaces. Replacing here would leave
        // a folder permanently half-scanned whenever the source was switched mid-pass.
        let s = MetadataScanner::with_reader(iso, 1, || {});
        s.scan(&paths(3));
        s.scan(&[PathBuf::from("/nonexistent/extra.raf")]);
        let got = collect(&s, 4);
        assert_eq!(got.len(), 4, "the first batch was dropped");
        assert!(got.iter().any(|r| r.path.ends_with("extra.raf")));
    }

    #[test]
    fn re_queuing_the_same_path_reads_it_once() {
        // ⚠ Re-opening a folder mid-pass must not double the work.
        static READS: AtomicUsize = AtomicUsize::new(0);
        fn counted(_p: &Path) -> Capture {
            READS.fetch_add(1, Ordering::SeqCst);
            thread::sleep(std::time::Duration::from_millis(5));
            Capture::default()
        }
        READS.store(0, Ordering::SeqCst);
        let s = MetadataScanner::with_reader(counted, 1, || {});
        let p = paths(4);
        s.scan(&p);
        s.scan(&p);
        assert_eq!(collect(&s, 4).len(), 4);
        thread::sleep(std::time::Duration::from_millis(60));
        assert_eq!(READS.load(Ordering::SeqCst), 4, "paths were read twice");
    }

    #[test]
    fn a_file_with_nothing_readable_still_produces_a_result() {
        // ⚠ An unreadable file must be marked as read, or it is retried on every folder open —
        // 11–15 ms of file reading each time, forever, for nothing.
        let s = MetadataScanner::with_reader(|_| Capture::default(), 2, || {});
        s.scan(&paths(2));
        let got = collect(&s, 2);
        assert_eq!(got.len(), 2);
        assert!(got.iter().all(|r| r.capture.is_empty()));
    }

    #[test]
    fn dropping_the_scanner_wakes_parked_workers() {
        // ⚠⚠ The same trap `ThumbLoader` hit — the fifteenth instance of a test naming a bug it
        // cannot detect. A test counting *reads started* cannot see a missing `Drop`, because a
        // worker parked on the condvar starts none either way. Each worker holds a clone of the
        // queue `Arc`, so the strong count **is** the live-worker count.
        let s = MetadataScanner::with_reader(|_| Capture::default(), 3, || {});
        let queue = s.queue.clone();
        assert_eq!(Arc::strong_count(&queue), 5, "scanner + this clone + 3 workers");
        thread::sleep(std::time::Duration::from_millis(20));
        drop(s);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while Arc::strong_count(&queue) > 1 && std::time::Instant::now() < deadline {
            thread::sleep(std::time::Duration::from_millis(1));
        }
        assert_eq!(
            Arc::strong_count(&queue),
            1,
            "parked workers never woke and are leaked for the life of the process"
        );
    }
}
```

⚠ That last one is copied deliberately: CLAUDE.md records it as the fifteenth instance of a test
naming a bug it cannot detect. Do not re-learn it.

- [ ] **Step 3: Wire it in**

- `RustroomApp` gains `metadata: MetadataScanner`, built once in `new`.
- `open_scan` queues `cat.files_without_capture(folder)` for the source's folders. ⚠ For
  `AllPhotos` and collections that is *several* folders — one query each, as `reload_marks`
  already does.
- `shell.rs` drains it per frame beside the thumbnail drain, writing each result with
  `cat.set_capture`.

⚠ **`scan` must be called only when the queue would actually change**, not every frame — the
`apply_view_demosaic` rule, now cited seven times in `CLAUDE.md`. Queuing on load is enough.

- [ ] **Step 4: Verify, run, measure**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
cargo run --release -p app
```

By hand: open your 830-file folder, wait, and confirm with

```bash
sqlite3 "$HOME/Library/Application Support/rustroom/catalog.db" \
  "PRAGMA user_version; SELECT count(*) FROM photo_capture; SELECT count(*) FROM photo_capture WHERE taken IS NOT NULL; SELECT DISTINCT camera FROM photo_capture;"
```

Expected: version 4, a row count that climbs to the folder's file count, dates present, and
**exactly one camera per body** — that last is the normalization working end to end, and is the
check most worth doing.

⚠ Also confirm the UI stays responsive while it runs, and that **reopening the folder queues
nothing** (the count does not restart).

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(app): fill the catalog's capture metadata in the background"
```

---

## Task 5: Docs

- [ ] **Step 1**

- Spec header → `**Status:** plan 1 built 2026-08-27; plan 2 outstanding`, and replace its estimated
  timings with what the real run showed.
- `CLAUDE.md` — a section in house style: the measurement that chose the architecture, the
  prefix-read rejection and *how* it failed, the three probe findings (camera casing, padded lens,
  wall-clock), why the pass is separate from the thumbnail loader, why an empty capture is stored,
  per-file storage, and the caveats. Update the test count; next action **plan 2 — filtering**.
- The Obsidian roadmap — mark M4.3b plan 1 done.

- [ ] **Step 2: Final verification and commit**

```bash
rustup update stable
cargo fmt --all --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
echo "exit=$?"
```

⚠ Check the exit code, not the output. **Do not push.**

---

## Definition of done

- `fmt`, `clippy --all-targets --all-features -D warnings`, `test --all` all exit 0.
- **All 47 goldens byte-identical** — nothing here touches `pipeline`.
- The real catalog reaches schema v4 with a capture row per file, dates populated, and **one
  camera string per body** across RAW and JPEG.
- Reopening a scanned folder queues nothing.
- The UI stays responsive while a folder is being scanned.
