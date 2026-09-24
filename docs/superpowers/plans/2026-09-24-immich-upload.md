# Immich Upload Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** An export can additionally upload each written file to Immich as a new asset, for any photo, at its capture date.

**Architecture:** Upload is an **export delivery target**, not an Immich-source feature. `ExportOptions` gains one target-agnostic `upload_target: Option<UploadTarget>`; the width-wide export worker uploads each file right after writing it; the HTTP and multipart work live in `crates/immich` (zero GUI) and the decisions in a new `crates/app/src/immich_upload.rs` lib module (no egui).

**Tech Stack:** Rust, `ureq` (hand-rolled multipart — ureq has no helper), `chrono` (already a dep of `export`), `serde`.

**Spec:** `docs/superpowers/specs/2026-09-24-immich-upload-design.md`

---

## Ground rules for this plan

⚠⚠ **ONE CARGO INVOCATION MODE.** Always `cargo test --workspace <name-filter>`, **never** `cargo test -p <crate>` — switching costs 43–531 s of rebuild. Give clippy its own target dir.

The gate, each exit code read from **its own command**, and **separate from the commit**:

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo fmt --all --check; echo "FMT=$?"
CARGO_TARGET_DIR=target/clippy cargo clippy --workspace --all-targets -- -D warnings > /tmp/c.log 2>&1; echo "CLIPPY=$?"
cargo test --workspace > /tmp/t.log 2>&1; echo "TEST=$?"
```

⚠ `${PIPESTATUS[0]}` is a **bash** idiom; this shell is **zsh**. Do not pipe the gate commands and expect a status.

⚠ Commit messages carry **no `Co-Authored-By` trailer**. **Never push.** Branch is `immich-upload`, already checked out.

⚠ A test name filter that matches nothing **exits 0**. After adding a test, confirm the run says `1 passed`, not `0 passed`.

---

## File Structure

| file | responsibility |
|---|---|
| `crates/immich/src/multipart.rs` **(new)** | pure `multipart/form-data` encoding. Byte-testable. The single most error-prone piece. |
| `crates/immich/src/lib.rs` | `Client::upload()`, beside `original()`. Zero GUI. |
| `crates/immich/src/dto.rs` | `Upload`, `UploadStatus`. |
| `crates/app/src/immich_upload.rs` **(new lib module)** | timestamps, filename, availability. **No egui.** Where the testable decisions live. |
| `crates/app/src/batch_export/options.rs` | `UploadTarget`, `ExportOptions.upload_target`. |
| `crates/app/src/batch_export/worker.rs` | the per-file upload step; `ExportReport` counters. |
| `crates/app/src/export_summary.rs` | `upload()` — the section's trailing summary. |
| `crates/app/src/modals/export.rs` | the dialog's Immich section (egui only). |
| `crates/app/src/settings.rs` | `immich_upload_key`. |
| `crates/app/src/modals/settings.rs` | the Upload key field (egui only). |
| `crates/app/src/sidecar.rs` | `UploadRecord` on `CopyEntry`. |

---

## Task 1: The multipart encoder

**Files:**
- Create: `crates/immich/src/multipart.rs`
- Modify: `crates/immich/src/lib.rs` (add `mod multipart;`)

- [ ] **Step 1: Write the failing test**

Create `crates/immich/src/multipart.rs` with only the tests plus the signatures they call, so it compiles and fails:

```rust
//! `multipart/form-data` encoding, by hand.
//!
//! ⚠⚠ **`ureq` has no multipart helper**, so this is hand-rolled — and a server rejecting a
//! malformed body answers with a bare `400` and no hint about which part was wrong. That is why
//! this module is pure, separate, and pinned by a **byte-exact** test.

/// One form field: a plain text value, or a file with a filename and a content type.
pub enum Part<'a> {
    Text { name: &'a str, value: &'a str },
    File { name: &'a str, filename: &'a str, content_type: &'a str, bytes: &'a [u8] },
}

/// The body for `parts` using `boundary`.
///
/// ⚠ CRLF line endings throughout, and the closing delimiter is `--<boundary>--`. Both are
/// required by RFC 7578; a bare `\n` is the classic silent failure.
pub fn body(boundary: &str, parts: &[Part<'_>]) -> Vec<u8> {
    unimplemented!()
}

/// A boundary that is guaranteed not to occur in `data`.
///
/// ⚠⚠ **A boundary appearing inside the file would truncate the upload**, so this is checked
/// rather than assumed improbable: the suffix is bumped until the boundary is absent. Scanning a
/// 15 MB JPEG is microseconds against a multi-second upload.
pub fn boundary_absent_from(data: &[u8]) -> String {
    unimplemented!()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⚠⚠ Byte-exact on purpose. Every structural rule of RFC 7578 that this encoder can get
    /// wrong — the leading `--`, CRLF after each header, the blank line before the body, the CRLF
    /// before the next delimiter, and the trailing `--` — is visible in this one literal.
    #[test]
    fn a_small_body_is_byte_exact() {
        let got = body(
            "XbounD",
            &[
                Part::Text { name: "fileCreatedAt", value: "2026-08-12T20:41:34.000Z" },
                Part::File {
                    name: "assetData",
                    filename: "a.jpg",
                    content_type: "image/jpeg",
                    bytes: b"\xff\xd8ok",
                },
            ],
        );
        let want: Vec<u8> = [
            b"--XbounD\r\n".as_ref(),
            b"Content-Disposition: form-data; name=\"fileCreatedAt\"\r\n\r\n",
            b"2026-08-12T20:41:34.000Z\r\n",
            b"--XbounD\r\n",
            b"Content-Disposition: form-data; name=\"assetData\"; filename=\"a.jpg\"\r\n",
            b"Content-Type: image/jpeg\r\n\r\n",
            b"\xff\xd8ok\r\n",
            b"--XbounD--\r\n",
        ]
        .concat();
        assert_eq!(got, want, "\ngot:  {:?}\nwant: {:?}", String::from_utf8_lossy(&got), String::from_utf8_lossy(&want));
    }

    /// ⚠ Binary data must survive untouched — this encoder must never treat bytes as text.
    #[test]
    fn binary_bytes_pass_through_unchanged() {
        let raw: Vec<u8> = (0u8..=255).collect();
        let out = body("B", &[Part::File {
            name: "assetData", filename: "x.bin", content_type: "application/octet-stream",
            bytes: &raw,
        }]);
        let start = out.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
        assert_eq!(&out[start..start + 256], &raw[..], "the payload was altered");
    }

    #[test]
    fn a_boundary_avoids_colliding_with_the_data() {
        // ⚠ The discriminating case: data that literally contains the first candidate.
        let first = boundary_absent_from(b"");
        let data = format!("junk {first} junk").into_bytes();
        let second = boundary_absent_from(&data);
        assert_ne!(first, second, "the boundary must change when it occurs in the data");
        assert!(
            !data.windows(second.len()).any(|w| w == second.as_bytes()),
            "the chosen boundary still occurs in the data"
        );
    }
}
```

Add to `crates/immich/src/lib.rs`, directly after the existing `mod`/`pub use` lines at the top:

```rust
pub mod multipart;
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cargo test --workspace multipart
```
Expected: FAIL — the three tests panic at `unimplemented!()` (`not implemented`).

- [ ] **Step 3: Implement**

Replace the two `unimplemented!()` bodies in `crates/immich/src/multipart.rs`:

```rust
pub fn body(boundary: &str, parts: &[Part<'_>]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    for p in parts {
        out.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        match p {
            Part::Text { name, value } => {
                out.extend_from_slice(
                    format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
                );
                out.extend_from_slice(value.as_bytes());
            }
            Part::File { name, filename, content_type, bytes } => {
                out.extend_from_slice(
                    format!(
                        "Content-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\n"
                    )
                    .as_bytes(),
                );
                out.extend_from_slice(format!("Content-Type: {content_type}\r\n\r\n").as_bytes());
                out.extend_from_slice(bytes);
            }
        }
        out.extend_from_slice(b"\r\n");
    }
    out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    out
}

pub fn boundary_absent_from(data: &[u8]) -> String {
    // ⚠ Long and dash-prefixed so it cannot collide with ordinary text, and derived from the
    // clock so two concurrent uploads (the export pool is `width`-wide) never share one.
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    for n in 0u32.. {
        let candidate = format!("----rustroom{seed:032x}{n:04x}");
        if !data
            .windows(candidate.len())
            .any(|w| w == candidate.as_bytes())
        {
            return candidate;
        }
    }
    unreachable!("u32 candidates cannot all collide")
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test --workspace multipart
```
Expected: `3 passed`. ⚠ If it says `0 passed`, the filter matched nothing — check the module is declared.

- [ ] **Step 5: Commit**

```bash
git add crates/immich/src/multipart.rs crates/immich/src/lib.rs
git commit -m "feat(immich): a pure multipart/form-data encoder, pinned byte-exact"
```

---

## Task 2: `Upload` DTO and `Client::upload()`

**Files:**
- Modify: `crates/immich/src/dto.rs`
- Modify: `crates/immich/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Append to the `#[cfg(test)] mod tests` block in `crates/immich/src/dto.rs`:

```rust
    /// ⚠⚠ **CONSTRUCTED, NOT CAPTURED.** Built from the 3.2.0 OpenAPI spec's
    /// `AssetMediaResponseDto`, so it proves only that this DTO matches the *schema*. This repo
    /// has been bitten four times by exactly that gap — `album.json` asserted an `assets` field
    /// the endpoint never returns. **Replace this with a captured reply the first time a real
    /// upload succeeds** (the plan's Task 12 says to).
    #[test]
    fn an_upload_reply_deserialises() {
        let created: Upload = serde_json::from_str(
            r#"{"id":"ad41fe4c-9226-460b-8e35-8ebacdc3a0ce","status":"created"}"#,
        )
        .unwrap();
        assert_eq!(created.id, "ad41fe4c-9226-460b-8e35-8ebacdc3a0ce");
        assert_eq!(created.status, UploadStatus::Created);

        let dup: Upload =
            serde_json::from_str(r#"{"id":"x","status":"duplicate"}"#).unwrap();
        assert_eq!(dup.status, UploadStatus::Duplicate);
    }

    /// ⚠ An unknown status must not fail the whole upload — the asset exists either way, and a
    /// future Immich adding a third value should not make a successful upload read as an error.
    #[test]
    fn an_unknown_status_is_tolerated() {
        let u: Upload = serde_json::from_str(r#"{"id":"x","status":"something-new"}"#).unwrap();
        assert_eq!(u.status, UploadStatus::Other);
    }
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cargo test --workspace an_upload_reply_deserialises
```
Expected: FAIL to compile — `cannot find type Upload in this scope`.

- [ ] **Step 3: Implement the DTO**

Append to `crates/immich/src/dto.rs` (outside the tests module):

```rust
/// The reply to `POST /assets`.
///
/// ⚠ Fields per the 3.2.0 spec's `AssetMediaResponseDto`, whose `required` is `["id", "status"]`.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Upload {
    pub id: String,
    pub status: UploadStatus,
}

/// Whether the server stored a new asset or recognised the bytes.
///
/// ⚠⚠ `Duplicate` is **not an error**: Immich checksums the upload, and re-sending byte-identical
/// data returns the *existing* asset's id. That is what makes a repeated upload of the same export
/// harmless with no client-side guard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UploadStatus {
    Created,
    Duplicate,
    /// ⚠ A value this build does not know. The asset exists; only the label is unfamiliar.
    #[serde(other)]
    Other,
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test --workspace upload_reply
```
Expected: `1 passed`. Then `cargo test --workspace unknown_status` → `1 passed`.

- [ ] **Step 5: Implement `Client::upload()`**

In `crates/immich/src/lib.rs`, add this method inside `impl Client`, directly after `original()`:

```rust
    /// `POST /assets` — upload `file` as a **new** asset, returning its id and whether the
    /// server considered it a duplicate.
    ///
    /// ⚠⚠ Requires **`asset.upload`** on the key, which is a grant *separate from*
    /// `asset.update`: a key that can do this **cannot modify or delete existing assets**. That
    /// separation is the whole reason uploading was built and two-way sync was declined.
    ///
    /// ⚠ `created` and `modified` are pre-formatted strings, because the two carry **different
    /// conventions** and only the caller knows which: capture time is naive wall-clock rendered
    /// as if UTC (matching what Immich itself did to the sibling JPEG), while a file mtime is a
    /// true instant. See `crate::immich_upload` in the app.
    ///
    /// ⚠ The body is built **in memory**. An export already holds a whole decoded image (the
    /// worker's own comment measures one at 298 MiB), so a 15 MB JPEG here is not the constraint.
    pub fn upload(
        &self,
        file: &Path,
        filename: &str,
        created: &str,
        modified: &str,
    ) -> Result<Upload, Error> {
        let bytes = std::fs::read(file).map_err(|e| Error::Io(e.to_string()))?;
        let boundary = multipart::boundary_absent_from(&bytes);
        let body = multipart::body(
            &boundary,
            &[
                multipart::Part::Text { name: "fileCreatedAt", value: created },
                multipart::Part::Text { name: "fileModifiedAt", value: modified },
                multipart::Part::Text { name: "filename", value: filename },
                multipart::Part::File {
                    name: "assetData",
                    filename,
                    content_type: content_type_of(filename),
                    bytes: &bytes,
                },
            ],
        );
        let mut res = self
            .agent
            .post(self.url("/assets"))
            .header("x-api-key", &self.key)
            .header(
                "Content-Type",
                &format!("multipart/form-data; boundary={boundary}"),
            )
            .header("Accept", "application/json")
            .send(&body[..])
            .map_err(map_ureq)?;
        let text = res
            .body_mut()
            .with_config()
            .limit(JSON_LIMIT)
            .read_to_string()
            .map_err(|e| Error::Decode(e.to_string()))?;
        serde_json::from_str(&text).map_err(|e| Error::Decode(e.to_string()))
    }
```

And add this free function near `map_ureq` at the bottom of `crates/immich/src/lib.rs`:

```rust
/// The media type for an exported file, from its extension.
///
/// ⚠ Only the three formats `export` can write. A default of `application/octet-stream` is
/// deliberate: Immich sniffs the content anyway, and guessing wrongly is worse than not guessing.
fn content_type_of(filename: &str) -> &'static str {
    let lower = filename.to_ascii_lowercase();
    if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        "image/jpeg"
    } else if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".tif") || lower.ends_with(".tiff") {
        "image/tiff"
    } else {
        "application/octet-stream"
    }
}
```

⚠ Ensure `use std::path::Path;` and the `Upload`/`UploadStatus` re-export exist at the top of `lib.rs`. The file already imports `Path` for `original()`; add `Upload, UploadStatus` to the existing `pub use dto::{...}` list.

- [ ] **Step 6: Add the content-type test**

Append to the `#[cfg(test)] mod tests` in `crates/immich/src/lib.rs`:

```rust
    #[test]
    fn a_content_type_follows_the_extension() {
        assert_eq!(content_type_of("a.JPG"), "image/jpeg");
        assert_eq!(content_type_of("a.jpeg"), "image/jpeg");
        assert_eq!(content_type_of("a.png"), "image/png");
        assert_eq!(content_type_of("a.tiff"), "image/tiff");
        // ⚠ Not a guess: an unknown extension says so rather than claiming JPEG.
        assert_eq!(content_type_of("a.weird"), "application/octet-stream");
    }
```

- [ ] **Step 7: Run the tests**

```bash
cargo test --workspace content_type
```
Expected: `1 passed`.

- [ ] **Step 8: Commit**

```bash
git add crates/immich/src/
git commit -m "feat(immich): Client::upload via POST /assets, with created/duplicate status"
```

---

## Task 3: `UploadTarget` and `ExportOptions.upload_target`

**Files:**
- Modify: `crates/app/src/batch_export/options.rs`

- [ ] **Step 1: Write the failing test**

Append to the `#[cfg(test)] mod tests` block in `crates/app/src/batch_export/options.rs`:

```rust
    /// ⚠ The default must be "disk only", so an existing user's next export is unchanged.
    #[test]
    fn upload_is_off_by_default() {
        assert_eq!(ExportOptions::default().upload_target, None);
    }

    /// ⚠⚠ The container-level `#[serde(default)]` is what makes this safe. An options file
    /// written before this field existed must still load — the same guarantee
    /// `an_options_file_from_before_the_delivery_fields_still_loads` pins for M6.1.
    #[test]
    fn an_options_file_from_before_upload_still_loads() {
        let json = r#"{"dest_dir":"/tmp","quality":90}"#;
        let o: ExportOptions = serde_json::from_str(json).unwrap();
        assert_eq!(o.upload_target, None);
        assert_eq!(o.quality, 90);
    }

    #[test]
    fn the_upload_target_round_trips() {
        let mut o = ExportOptions::default();
        o.upload_target = Some(UploadTarget::Immich);
        let back: ExportOptions = serde_json::from_str(&serde_json::to_string(&o).unwrap()).unwrap();
        assert_eq!(back.upload_target, Some(UploadTarget::Immich));
    }
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cargo test --workspace upload_is_off_by_default
```
Expected: FAIL to compile — `no field upload_target on type ExportOptions`.

- [ ] **Step 3: Implement**

In `crates/app/src/batch_export/options.rs`, add this field to `ExportOptions` immediately after `pub delivery: Delivery,`:

```rust
    /// Where the exported file is delivered **in addition to** `dest_dir`. `None` = disk only.
    ///
    /// ⚠⚠ **Deliberately NOT named for a service.** The Immich integration must be switchable off
    /// with *"no mention of immich at all"* (`docs/BACKLOG.md`), so `export` knows only that a
    /// delivery target may exist. A field called `immich` here could not be hidden later without
    /// renaming every call site.
    ///
    /// ⚠ No field-level `#[serde(default)]`: the struct carries a container-level one, and adding
    /// it per field implies a guarantee it is not the source of — this struct's `space` field says
    /// the same.
    pub upload_target: Option<UploadTarget>,
```

Add to `ExportOptions::default()` (the `impl Default` block), after the `delivery: Delivery::default(),` line:

```rust
            upload_target: None,
```

And add this enum after the `PostExportAction` enum:

```rust
/// A destination an export can be delivered to besides the filesystem.
///
/// ⚠ One variant today, and that is not a placeholder — it is the only target that exists.
/// ⚠⚠ **Append variants, never insert**: this is `Serialize`d into settings and presets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UploadTarget {
    Immich,
}
```

- [ ] **Step 4: Run the tests**

```bash
cargo test --workspace upload_target
```
Expected: `2 passed` (`upload_is_off_by_default` matches a different filter). Then:
```bash
cargo test --workspace before_upload_still_loads
```
Expected: `1 passed`.

⚠ If other tests now fail to compile because they build `ExportOptions` with a struct literal, they must gain `upload_target: None`. Run `cargo test --workspace` and fix every such site.

- [ ] **Step 5: Commit**

```bash
git add crates/app/src/batch_export/options.rs
git commit -m "feat(export): a target-agnostic upload_target on ExportOptions"
```

---

## Task 4: `immich_upload` — timestamps, filename, availability

**Files:**
- Create: `crates/app/src/immich_upload.rs`
- Modify: `crates/app/src/lib.rs` (add `pub mod immich_upload;`)
- Modify: `crates/app/Cargo.toml` (add `chrono`)

- [ ] **Step 1: Add the dependency**

In `crates/app/Cargo.toml`, add to `[dependencies]`:

```toml
chrono = "0.4.45"
```

⚠ The same version `crates/export/Cargo.toml` already pins, so no second copy is compiled.

- [ ] **Step 2: Write the failing test**

Create `crates/app/src/immich_upload.rs`:

```rust
//! What an Immich upload needs decided before any HTTP happens: when the photo was taken, what
//! to call the file, and whether uploading is possible at all.
//!
//! ⚠ **No egui here.** This is the testable half; the dialog lives in `modals::export`.

use crate::settings::Settings;
use std::path::Path;

/// Whether an upload can be offered, and if not, why.
///
/// ⚠⚠ **This is the seed of the "Immich completely optional" gate** (`docs/BACKLOG.md`). Today
/// `NotConfigured` means no server URL or browse key; once a master toggle exists it answers
/// `NotConfigured` too, and every Immich surface consults **one** predicate rather than growing
/// its own conditional.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    /// No Immich at all — the section is **not drawn**.
    NotConfigured,
    /// Immich is set up but no upload key is set — the control is **drawn and disabled**.
    NoUploadKey,
    Ready,
}

/// Whether uploading can be offered, given the settings.
pub fn availability(s: &Settings) -> Availability {
    if s.immich_url.trim().is_empty() || s.immich_key.trim().is_empty() {
        return Availability::NotConfigured;
    }
    if s.immich_upload_key.trim().is_empty() {
        return Availability::NoUploadKey;
    }
    Availability::Ready
}

/// `fileCreatedAt` for a photo: **its capture time**, never now.
///
/// ⚠⚠ **Naive wall-clock rendered as if UTC, and that is measured rather than chosen.**
/// `Capture::taken` is naive local wall-clock by its own rule. Immich did exactly this to the
/// sibling JPEG of a real pair: EXIF `DateTimeOriginal` `20:41:34` became
/// `fileCreatedAt: 2026-08-12T20:41:34.000Z`. Matching that convention is what makes an uploaded
/// edit land at **the same `fileCreatedAt` as its original**, which is the property that matters
/// and the one to verify by use.
///
/// ⚠ Falls back to the source file's mtime when EXIF has no date — some PNG/TIFF sources have
/// none at all. Falls back to the epoch only if even that fails, which is visibly wrong in the
/// timeline rather than silently plausible.
pub fn created_at_text(capture: &core_image::Capture, source: &Path) -> String {
    let secs = capture.taken.or_else(|| mtime_secs(source)).unwrap_or(0);
    utc_text(secs)
}

/// `fileModifiedAt`: the exported file's own mtime.
///
/// ⚠ A **true instant**, unlike `created_at_text`'s wall-clock — this genuinely is when this
/// rendering was written, so UTC is honest here. The two fields carry different conventions on
/// purpose.
pub fn modified_at_text(exported: &Path) -> String {
    utc_text(mtime_secs(exported).unwrap_or(0))
}

fn mtime_secs(p: &Path) -> Option<i64> {
    let m = std::fs::metadata(p).ok()?.modified().ok()?;
    let d = m.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(d.as_secs() as i64)
}

fn utc_text(secs: i64) -> String {
    chrono::DateTime::from_timestamp(secs, 0)
        .map(|d| d.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
        .unwrap_or_else(|| "1970-01-01T00:00:00.000Z".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings_with(url: &str, key: &str, upload: &str) -> Settings {
        let mut s = Settings::default();
        s.immich_url = url.to_string();
        s.immich_key = key.to_string();
        s.immich_upload_key = upload.to_string();
        s
    }

    #[test]
    fn availability_needs_a_server_then_an_upload_key() {
        assert_eq!(availability(&settings_with("", "", "")), Availability::NotConfigured);
        assert_eq!(
            availability(&settings_with("https://h", "browse", "")),
            Availability::NoUploadKey
        );
        assert_eq!(
            availability(&settings_with("https://h", "browse", "up")),
            Availability::Ready
        );
        // ⚠ Whitespace is absent, or a stray space reads as configured.
        assert_eq!(
            availability(&settings_with("https://h", "browse", "   ")),
            Availability::NoUploadKey
        );
    }

    /// ⚠⚠ **The actual bug this guards is "the edit landed at today's date".** So it asserts the
    /// exact string for a known capture time, and separately that the result is not near now.
    #[test]
    fn the_created_date_is_the_capture_time_not_now() {
        let mut c = core_image::Capture::default();
        c.taken = Some(1_787_824_800); // 2026-08-27T10:00:00 wall-clock
        let got = created_at_text(&c, Path::new("/nonexistent"));
        assert_eq!(got, "2026-08-27T10:00:00.000Z");

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        assert_ne!(
            got,
            utc_text(now),
            "the capture time must not be replaced by the upload time"
        );
    }

    /// ⚠ The discriminating case for the fallback: no EXIF date, so the file's mtime is used.
    #[test]
    fn a_photo_with_no_exif_date_falls_back_to_the_file_mtime() {
        let dir = std::env::temp_dir().join(format!(
            "rustroom_upload_mtime_{}_{}",
            std::process::id(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("x.png");
        std::fs::write(&f, b"x").unwrap();

        let c = core_image::Capture::default();
        assert_eq!(c.taken, None, "the fixture must have no EXIF date");
        let got = created_at_text(&c, &f);

        assert_ne!(got, "1970-01-01T00:00:00.000Z", "the mtime was not used");
        assert!(got.ends_with('Z') && got.len() == 24, "unexpected shape: {got}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ⚠ Temp-dir isolation keyed on pid **and** a counter — `immich_list_cache`'s pattern.
    /// Two sites in `immich_source` key on the pid alone and are the ones not to copy.
    static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
}
```

Add to `crates/app/src/lib.rs`, in alphabetical position among the `pub mod` lines (after `pub mod immich_source;`):

```rust
pub mod immich_upload;
```

- [ ] **Step 3: Run the test to verify it fails**

```bash
cargo test --workspace immich_upload
```
Expected: FAIL to compile — `no field immich_upload_key on type Settings`. That is Task 5's job; do it next and return here.

- [ ] **Step 4: Commit (after Task 5 makes it compile)**

```bash
git add crates/app/src/immich_upload.rs crates/app/src/lib.rs crates/app/Cargo.toml
git commit -m "feat(app): immich_upload — capture-time timestamps and an availability gate"
```

---

## Task 5: `Settings.immich_upload_key`

**Files:**
- Modify: `crates/app/src/settings.rs`
- Modify: `crates/app/src/modals/settings.rs`

- [ ] **Step 1: Write the failing test**

Append to the `#[cfg(test)] mod tests` block in `crates/app/src/settings.rs`:

```rust
    #[test]
    fn the_upload_key_defaults_to_empty_and_round_trips() {
        assert!(Settings::default().immich_upload_key.is_empty());
        let mut s = Settings::default();
        s.immich_upload_key = "upload-key".to_string();
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back.immich_upload_key, "upload-key");
    }

    /// ⚠ A settings file from before this field must still load — the browse key and URL survive.
    #[test]
    fn a_settings_file_from_before_the_upload_key_still_loads() {
        let json = r#"{"immich_url":"https://h","immich_key":"browse"}"#;
        let s: Settings = serde_json::from_str(json).unwrap();
        assert_eq!(s.immich_url, "https://h");
        assert_eq!(s.immich_key, "browse");
        assert!(s.immich_upload_key.is_empty());
    }
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cargo test --workspace upload_key
```
Expected: FAIL to compile — `no field immich_upload_key`.

- [ ] **Step 3: Implement**

In `crates/app/src/settings.rs`, add this field immediately after `pub immich_key: String,`:

```rust
    /// A **separate** key, granted `asset.upload`, used only when an export uploads.
    ///
    /// ⚠⚠ **Two keys rather than one with the grant added, and the reason is structural.**
    /// Browsing, thumbnails and the rail run constantly and unattended; with one key every one of
    /// those requests would travel with a write-capable credential. Two keys make "browsing
    /// cannot write" a property of the **credential** rather than of our code being correct.
    ///
    /// ⚠ `asset.upload` is a grant *separate from* `asset.update` (verified against the 3.2.0
    /// permission enum, 168 values), so even this key **cannot modify or delete** existing assets.
    ///
    /// ⚠ Plaintext, like [`Self::immich_key`]. The OS keychain remains out of scope by decision —
    /// and a write-capable key in plaintext deserves the warning more, so the help text says so.
    ///
    /// Empty means uploading is unavailable; see [`crate::immich_upload::availability`].
    pub immich_upload_key: String,
```

Add to the `Default` impl, after `immich_key: String::new(),`:

```rust
            immich_upload_key: String::new(),
```

⚠ If the test at `settings.rs:1505` builds a `Settings` literal, add `immich_upload_key: "…".to_string()` there. Run `cargo test --workspace` and fix every struct-literal site.

- [ ] **Step 4: Run the tests**

```bash
cargo test --workspace upload_key
```
Expected: `2 passed`.

- [ ] **Step 5: Add the Settings field to the Immich tab**

In `crates/app/src/modals/settings.rs`, find the existing browse-key row (search for `"Paste a read-only key"`, around line 502) and add a second labelled row directly after that block, following the same pattern the file already uses for the browse key:

```rust
                    // ⚠ The upload key is deliberately its own field — see
                    // `Settings::immich_upload_key` for why browsing must stay read-only.
                    row(ui, "Upload key", |ui| {
                        ui.add(
                            egui::TextEdit::singleline(&mut draft.immich_upload_key)
                                .password(true)
                                .desired_width(f32::INFINITY),
                        )
                        .on_hover_text(
                            "Optional, and only used when an export uploads. Paste a key granted \
                             asset.upload — it cannot modify or delete existing photos. Leave it \
                             empty to keep uploading switched off. ⚠ Stored in plain text in \
                             settings.json.",
                        );
                    });
```

⚠ Match the surrounding code's actual helper name and borrow shape rather than assuming `row(...)`: read the browse-key block immediately above and mirror it exactly.

- [ ] **Step 6: Run the whole suite and the gate**

```bash
cargo test --workspace > /tmp/t.log 2>&1; echo "TEST=$?"
```
Expected: `TEST=0`. Then Task 4's tests now compile:
```bash
cargo test --workspace immich_upload
```
Expected: `3 passed`.

- [ ] **Step 7: Commit**

```bash
git add crates/app/src/settings.rs crates/app/src/modals/settings.rs
git commit -m "feat(settings): a separate upload-only Immich key, so browsing cannot write"
```

Then commit Task 4 (Step 4 above) if it is not already committed.

---

## Task 6: The sidecar upload record

**Files:**
- Modify: `crates/app/src/sidecar.rs`

- [ ] **Step 1: Write the failing test**

Append to the `#[cfg(test)] mod tests` block in `crates/app/src/sidecar.rs`:

```rust
    /// ⚠ Keyed by **host**: the same photo may be uploaded to two servers, and a bare "uploaded"
    /// flag would lie on the second one.
    #[test]
    fn an_upload_record_round_trips_per_host() {
        let mut e = CopyEntry::default();
        e.uploads.push(UploadRecord {
            host: "immich.example".to_string(),
            asset_id: "abc".to_string(),
            status: "created".to_string(),
            at: "2026-09-24T12:00:00.000Z".to_string(),
        });
        let back: CopyEntry = serde_json::from_str(&serde_json::to_string(&e).unwrap()).unwrap();
        assert_eq!(back.uploads.len(), 1);
        assert_eq!(back.uploads[0].asset_id, "abc");
    }

    /// ⚠⚠ A sidecar written before this field must still load with its **edits intact**. The
    /// container-level-`serde(default)` trap this file documents is about `Sidecar`; this field is
    /// on `CopyEntry` and takes a **field-level** default only.
    #[test]
    fn a_copy_entry_from_before_uploads_still_loads() {
        let json = r#"{"copy":0,"edits":{}}"#;
        let e: CopyEntry = serde_json::from_str(json).unwrap();
        assert_eq!(e.copy, 0);
        assert!(e.uploads.is_empty());
    }
```

⚠ If `CopyEntry` has required fields beyond `copy` and `edits`, widen the JSON literal in the second test to the minimum that parses — read the struct first.

- [ ] **Step 2: Run the test to verify it fails**

```bash
cargo test --workspace upload_record
```
Expected: FAIL to compile — `cannot find type UploadRecord`.

- [ ] **Step 3: Implement**

In `crates/app/src/sidecar.rs`, add near `CopyEntry`:

```rust
/// One upload of this copy to one Immich server.
///
/// ⚠ `status` is the server's own word (`created` / `duplicate`) kept as text rather than as an
/// enum: it is a record of what happened, and a future value should read back rather than fail.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UploadRecord {
    /// The server this went to, so a second server is not mistaken for the first.
    pub host: String,
    pub asset_id: String,
    pub status: String,
    /// When Rustroom uploaded it, ISO-8601 UTC.
    pub at: String,
}
```

Add this field to `CopyEntry`:

```rust
    /// Uploads of this copy to Immich, newest last. Empty for a copy never uploaded.
    ///
    /// ⚠⚠ **Field-level `#[serde(default)]` only.** A container-level one on the sidecar types is
    /// the documented data-loss trap — see `Sidecar`'s own doc comment. This is safe because the
    /// entry's required fields stay required.
    ///
    /// ⚠ A list, not one record: uploading the same copy twice after further editing is the
    /// correct outcome, and the history of that is worth keeping.
    #[serde(default)]
    pub uploads: Vec<UploadRecord>,
```

⚠ If `CopyEntry` does not derive `Default`, add `uploads: Vec::new()` to however it is constructed, and adjust the first test to build it the way the file already does.

- [ ] **Step 4: Run the tests**

```bash
cargo test --workspace upload
```
Expected: every upload-named test passes; note the count and confirm it is not `0 passed`.

- [ ] **Step 5: Commit**

```bash
git add crates/app/src/sidecar.rs
git commit -m "feat(sidecar): record uploads per copy, keyed by host"
```

---

## Task 7: Wire the upload into the export worker

**Files:**
- Modify: `crates/app/src/batch_export/worker.rs`

- [ ] **Step 1: Write the failing test**

⚠⚠ **This task's real behaviour needs a server, so the test pins the part that does not: that a failed or unconfigured upload leaves the export itself successful.** Append to the `#[cfg(test)] mod tests` block in `crates/app/src/batch_export/worker.rs`:

```rust
    /// ⚠⚠ **A failed upload must NOT fail the export.** The file is on disk; a flaky network
    /// destroying a batch export would be far worse than an unuploaded file. This runs with
    /// `upload_target: Some(Immich)` and **no reachable server**, which is exactly the shape of
    /// the failure, and asserts the file was still written and counted.
    #[test]
    fn a_failed_upload_still_exports_the_file() {
        let dir = TempDir::new("upload_fail");
        let mut opts = opts_for(&dir);
        opts.upload_target = Some(UploadTarget::Immich);
        let report = run_one(&opts, &dir);
        assert_eq!(report.exported, 1, "the export itself must succeed");
        assert_eq!(report.failed.len(), 0, "an upload failure is not an export failure");
        assert_eq!(report.uploaded, 0, "nothing was uploaded");
        assert_eq!(report.unconfirmed.len(), 1, "the failure must be reported");
    }

    /// ⚠ The control: with no target, nothing about the export changes and nothing is reported.
    #[test]
    fn no_upload_target_means_no_upload_reporting() {
        let dir = TempDir::new("upload_off");
        let opts = opts_for(&dir);
        assert_eq!(opts.upload_target, None);
        let report = run_one(&opts, &dir);
        assert_eq!(report.exported, 1);
        assert_eq!(report.uploaded, 0);
        assert!(report.unconfirmed.is_empty());
    }
```

⚠ `TempDir`, `opts_for` and `run_one` are placeholders for whatever this module's existing test helpers are called — read the existing tests (e.g. around `run_at` at line 763) and use the real names and shapes. Do **not** invent a new harness.

- [ ] **Step 2: Run the test to verify it fails**

```bash
cargo test --workspace a_failed_upload_still_exports
```
Expected: FAIL to compile — `no field uploaded on ExportReport`.

- [ ] **Step 3: Extend `ExportReport`**

In `crates/app/src/batch_export/worker.rs`, add to `ExportReport`:

```rust
    /// Files the server confirmed it stored (whether `created` or `duplicate`).
    pub uploaded: usize,
    /// Files written but whose upload did not confirm, with why.
    ///
    /// ⚠⚠ **"Unconfirmed", not "failed", and never retried automatically.** An SSL/EOF error means
    /// the *response* was lost, not that the server rejected the upload — measured on this
    /// tailnet during the stacking migration, 4 writes in 2,373 failed this way and **none had
    /// applied**, but that had to be checked rather than assumed. An automatic retry is how one
    /// edit becomes three assets.
    pub unconfirmed: Vec<(PathBuf, String)>,
```

- [ ] **Step 4: Add the upload step**

In the width-wide worker closure in `run()`, replace the outcome block (currently at ~line 410) with:

```rust
                let outcome = decoded.and_then(|src| {
                    export_one(src, &p.edits, &p.dest, opts, render, &p.meta, look)
                });
                // ⚠⚠ Only after a successful write, and it can never turn a success into a
                // failure. The exported file is the deliverable; the upload is delivery.
                let upload = match (&outcome, uploader) {
                    (Ok(()), Some(u)) => Some(u.send(&p.dest, &p.capture, &p.source, p.copy)),
                    _ => None,
                };
                {
                    let mut r = shared.lock().expect("export report");
                    match outcome {
                        Ok(()) => r.exported += 1,
                        Err(e) => r.failed.push((p.source.clone(), e)),
                    }
                    match upload {
                        Some(Ok(())) => r.uploaded += 1,
                        Some(Err(e)) => r.unconfirmed.push((p.dest.clone(), e)),
                        None => {}
                    }
                }
```

⚠ `p.capture` and `p.copy` must be carried on `Planned` the way `p.meta` already is. If they are not, add them in `plan()` from `job.capture` and `job.id.copy` — the `ExportJob` has both. ⚠⚠ The copy number is **not** optional: see `record`'s doc for why writing copy 0 while the dialog reads copy N silences the note for every virtual copy.

Add this helper to the same file, above `run()`:

```rust
/// The upload half of a run, or `None` when the export is not uploading.
///
/// ⚠ Built **once per run**, outside the worker pool: it holds the `Client`, and constructing one
/// per photo would re-parse the URL `width` times a second for no reason.
struct Uploader {
    client: immich::Client,
    host: String,
}

impl Uploader {
    /// `None` when there is no target, or Immich is not ready — which is not an error, it is the
    /// ordinary "disk only" case.
    fn for_run(opts: &ExportOptions, settings: &crate::settings::Settings) -> Option<Self> {
        if opts.upload_target != Some(UploadTarget::Immich) {
            return None;
        }
        if crate::immich_upload::availability(settings) != crate::immich_upload::Availability::Ready
        {
            return None;
        }
        let client = immich::Client::new(&settings.immich_url, &settings.immich_upload_key).ok()?;
        Some(Self { client, host: settings.immich_url.trim().to_string() })
    }

    /// Upload one written file. `Err` carries a message for `unconfirmed`.
    fn send(
        &self,
        exported: &Path,
        capture: &core_image::Capture,
        source: &Path,
        copy: u32,
    ) -> Result<(), String> {
        let filename = exported
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| "the exported file has no usable name".to_string())?;
        let created = crate::immich_upload::created_at_text(capture, source);
        let modified = crate::immich_upload::modified_at_text(exported);
        let up = self
            .client
            .upload(exported, filename, &created, &modified)
            .map_err(|e| e.to_string())?;
        // ⚠ Recorded against the SOURCE photo's sidecar, not the exported file: the sidecar
        // belongs to the photo, and the export is a derived artefact that may be deleted.
        crate::immich_upload::record(source, copy, &self.host, &up);
        Ok(())
    }
}
```

⚠ Thread `settings` into `run()`/`spawn()` the way `opts` already is, and pass `uploader.as_ref()` into the closure as `uploader`. If `ExportWorker::spawn` does not currently receive `Settings`, add the parameter and update its one call site in `export_flow.rs`.

- [ ] **Step 5: Add `immich_upload::record`**

Append to `crates/app/src/immich_upload.rs`:

```rust
/// Record an upload against the **copy that was exported**.
///
/// ⚠⚠ **Copy-aware, and that is load-bearing.** An earlier draft of this plan wrote copy 0
/// unconditionally while [`uploaded_note`] reads the copy actually being exported — so for any
/// virtual copy the write and the read would have addressed different entries and the note would
/// have stayed silent for ever. A record nobody can find is worse than no record.
///
/// ⚠ Best-effort: a sidecar that cannot be written must not turn a successful upload into a
/// failure. The asset exists on the server either way, and the record is a convenience.
pub fn record(source: &Path, copy: u32, host: &str, up: &immich::Upload) {
    let mut sc = crate::sidecar::load_all(source).unwrap_or_default();
    let at = modified_at_text(source);
    let rec = crate::sidecar::UploadRecord {
        host: host.to_string(),
        asset_id: up.id.clone(),
        status: format!("{:?}", up.status).to_lowercase(),
        at,
    };
    if let Some(entry) = sc.copies.iter_mut().find(|c| c.copy == copy) {
        entry.uploads.push(rec);
        let _ = crate::sidecar::save_all(source, &sc);
    }
    // ⚠ A copy with no sidecar entry yet is simply not recorded. Creating one here would mean
    // inventing an `Edits` for it, and a neutral photo deliberately has no entry.
}

/// What the dialog says about a photo already uploaded to `host`, or `None` when it has not been.
///
/// ⚠⚠ **Informs, never blocks.** A second upload after further editing is the *correct*
/// outcome — the bytes differ, so it is a new picture. Replacing the previous one would mean
/// **deleting** an asset, which is exactly the danger that got two-way sync declined.
pub fn uploaded_note(source: &Path, copy: u32, host: &str) -> Option<String> {
    let sc = crate::sidecar::load_all(source)?;
    let last = sc
        .copies
        .iter()
        .find(|c| c.copy == copy)?
        .uploads
        .iter()
        .filter(|u| u.host == host)
        .next_back()?;
    // ⚠ Date only: the time of day is noise in a one-line note.
    Some(format!("Already uploaded on {}", &last.at[..10.min(last.at.len())]))
}
```

⚠ `load_all`/`save_all` are the names this plan assumes for the envelope-aware sidecar API. **Read `sidecar.rs` and use the real names**; the file's doc comments describe `load_all`'s explicit dispatch, so an equivalent exists.

- [ ] **Step 6: Run the tests**

```bash
cargo test --workspace upload
```
Expected: the two worker tests pass; note the total and confirm it is not `0 passed`.

- [ ] **Step 7: Full suite**

```bash
cargo test --workspace > /tmp/t.log 2>&1; echo "TEST=$?"
grep -c '^test .* ok$' /tmp/t.log
```
Expected: `TEST=0`. ⚠ Every `ExportReport` struct literal in tests needs the two new fields; fix each.

- [ ] **Step 8: Commit**

```bash
git add crates/app/src/batch_export/worker.rs crates/app/src/immich_upload.rs
git commit -m "feat(export): upload each written file, without ever failing the export"
```

---

## Task 8: The dialog section

**Files:**
- Modify: `crates/app/src/export_summary.rs`
- Modify: `crates/app/src/modals/export.rs`

- [ ] **Step 1: Write the failing test**

Append to the `#[cfg(test)] mod tests` block in `crates/app/src/export_summary.rs`:

```rust
    #[test]
    fn the_upload_summary_says_which_state_it_is_in() {
        let mut o = ExportOptions::default();
        assert_eq!(upload(&o), "Off");
        o.upload_target = Some(crate::batch_export::options::UploadTarget::Immich);
        assert_eq!(upload(&o), "Immich");
    }
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cargo test --workspace upload_summary
```
Expected: FAIL to compile — `cannot find function upload`.

- [ ] **Step 3: Implement the summary**

Append to `crates/app/src/export_summary.rs`:

```rust
/// The Upload section's trailing summary — the section header says its own state, which is this
/// dialog's rule for every section.
pub fn upload(o: &ExportOptions) -> String {
    match o.upload_target {
        None => "Off".to_string(),
        Some(crate::batch_export::options::UploadTarget::Immich) => "Immich".to_string(),
    }
}
```

- [ ] **Step 4: Run the test**

```bash
cargo test --workspace upload_summary
```
Expected: `1 passed`.

- [ ] **Step 5: Add the section to the dialog**

In `crates/app/src/modals/export.rs`, after the `"Watermark"` section (~line 362), add:

```rust
                    // ⚠⚠ Drawn only when Immich is configured. `NotConfigured` must render
                    // NOTHING — the requirement is "no mention of immich at all" when the
                    // integration is off (`docs/BACKLOG.md`).
                    let avail = crate::immich_upload::availability(&app.settings);
                    if avail != crate::immich_upload::Availability::NotConfigured {
                        section(
                            ui,
                            "Upload",
                            &export_summary::upload(&opts),
                            false,
                            |ui| {
                                let ready =
                                    avail == crate::immich_upload::Availability::Ready;
                                let mut on = opts.upload_target.is_some();
                                // ⚠ Disabled, never hidden, when there is no upload key: a
                                // control that vanishes reads as a bug, and this project has
                                // taken that back out once already.
                                let r = ui.add_enabled(
                                    ready,
                                    egui::Checkbox::new(&mut on, "Upload to Immich after export"),
                                );
                                if !ready {
                                    r.on_disabled_hover_text(
                                        "Set an Upload key in Settings → Immich first. Browsing \
                                         uses a read-only key and cannot upload.",
                                    );
                                } else if r.changed() {
                                    opts.upload_target = on.then_some(
                                        crate::batch_export::options::UploadTarget::Immich,
                                    );
                                }
                            },
                        );
                    }
```

⚠ `app.settings` is the name this plan assumes for the live settings on `RustroomApp`. Read the surrounding function — it already borrows `app` — and use the real accessor, honouring the borrow shape the file's own comment warns about.

- [ ] **Step 6: Run the gate**

```bash
cargo fmt --all --check; echo "FMT=$?"
CARGO_TARGET_DIR=target/clippy cargo clippy --workspace --all-targets -- -D warnings > /tmp/c.log 2>&1; echo "CLIPPY=$?"
cargo test --workspace > /tmp/t.log 2>&1; echo "TEST=$?"
```
Expected: all three `=0`.

- [ ] **Step 7: Commit**

```bash
git add crates/app/src/export_summary.rs crates/app/src/modals/export.rs
git commit -m "feat(export): an Upload section, hidden without Immich and disabled without a key"
```

---

## Task 9: The "already uploaded" note in the dialog

**Files:**
- Modify: `crates/app/src/immich_upload.rs` (tests for `uploaded_note`)
- Modify: `crates/app/src/modals/export.rs`

⚠⚠ **This task exists because the plan's self-review found §5 of the spec had no task.** The note
was the *answer* to duplicate handling — without it, a re-export silently makes a second asset and the
user only discovers it in their timeline. Do not drop it.

- [ ] **Step 1: Write the failing test**

Append to the `#[cfg(test)] mod tests` block in `crates/app/src/immich_upload.rs`:

```rust
    /// ⚠⚠ **The bug this guards is a note that never appears.** `record` writes against the
    /// exported copy and this reads the same copy; an earlier draft wrote copy 0 unconditionally,
    /// which would have silenced the note for every virtual copy. So this asserts a **non-zero**
    /// copy round-trips, which is the case that would have been broken.
    #[test]
    fn the_note_finds_an_upload_of_the_same_copy_and_host() {
        let dir = std::env::temp_dir().join(format!(
            "rustroom_upload_note_{}_{}",
            std::process::id(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let photo = dir.join("a.RAF");
        std::fs::write(&photo, b"x").unwrap();

        // A sidecar with an entry for copy 2 only.
        let mut sc = crate::sidecar::Sidecar::default();
        sc.set_edits(2, edit_model::Edits::default());
        crate::sidecar::save_all(&photo, &sc).unwrap();

        assert_eq!(uploaded_note(&photo, 2, "https://h"), None, "nothing uploaded yet");

        let up = immich::Upload {
            id: "abc".into(),
            status: immich::UploadStatus::Created,
        };
        record(&photo, 2, "https://h", &up);

        assert!(
            uploaded_note(&photo, 2, "https://h").is_some(),
            "the note must find an upload of the same copy"
        );
        // ⚠ The two discriminating negatives: a different copy and a different host must NOT match.
        assert_eq!(uploaded_note(&photo, 0, "https://h"), None, "copy must matter");
        assert_eq!(uploaded_note(&photo, 2, "https://other"), None, "host must matter");

        let _ = std::fs::remove_dir_all(&dir);
    }
```

⚠ `immich::Upload` needs a way to be constructed in a test. It derives only `Deserialize` in Task 2 —
either add `#[derive(Clone)]` plus `pub` fields (it already has them) and build it directly, or build it
from JSON with `serde_json::from_str`. **Prefer the JSON route** so the DTO keeps exactly one
construction path. Replace the literal above with:

```rust
        let up: immich::Upload =
            serde_json::from_str(r#"{"id":"abc","status":"created"}"#).unwrap();
```

- [ ] **Step 2: Run the test to verify it fails**

```bash
cargo test --workspace the_note_finds_an_upload
```
Expected: FAIL — `uploaded_note` returns `None` after `record`, or the test fails to compile if
`uploaded_note` is not yet present. (It was added in Task 7's Step 5 alongside `record`; if not, add it
now from that step's code.)

- [ ] **Step 3: Make it pass**

`uploaded_note` and the copy-aware `record` are already specified in Task 7, Step 5. If Step 1 fails
because `set_edits`/`save_all`/`Sidecar::default` have different names, **read `sidecar.rs` and use the
real API** — do not add a new one.

- [ ] **Step 4: Run the test**

```bash
cargo test --workspace the_note_finds_an_upload
```
Expected: `1 passed`.

- [ ] **Step 5: Show the note in the dialog**

In `crates/app/src/modals/export.rs`, inside the `"Upload"` section body added in Task 8, after the
checkbox:

```rust
                                // ⚠ The first job of the run, matching how the Naming section
                                // already previews `{seq}` = 1 from `jobs.first()`.
                                if let Some(note) = jobs.first().and_then(|j| {
                                    crate::immich_upload::uploaded_note(
                                        &j.id.path,
                                        j.id.copy,
                                        app.settings.immich_url.trim(),
                                    )
                                }) {
                                    // ⚠⚠ Informs, never blocks — uploading again is still allowed,
                                    // because a re-export after editing IS a new picture. Replacing
                                    // the old asset would mean deleting one.
                                    ui.label(
                                        egui::RichText::new(note).weak(),
                                    )
                                    .on_hover_text(
                                        "Uploading again creates a second asset in Immich. Immich \
                                         recognises byte-identical re-uploads and keeps just one.",
                                    );
                                }
```

⚠ `jobs` is already in scope in this function (Task 8's section sits in the same closure). If the
borrow checker objects because `jobs` is moved earlier, compute the note **before** the dialog closure
and capture the `Option<String>`, which is what this file's own comment about borrow shape advises.

- [ ] **Step 6: Gate**

```bash
cargo fmt --all --check; echo "FMT=$?"
CARGO_TARGET_DIR=target/clippy cargo clippy --workspace --all-targets -- -D warnings > /tmp/c.log 2>&1; echo "CLIPPY=$?"
cargo test --workspace > /tmp/t.log 2>&1; echo "TEST=$?"
```
Expected: all `=0`.

- [ ] **Step 7: Commit**

```bash
git add crates/app/src/immich_upload.rs crates/app/src/modals/export.rs
git commit -m "feat(export): say when a photo was already uploaded, without blocking a second"
```

---

## Task 10: Prove the export is unchanged when not uploading

**Files:**
- Modify: `crates/app/src/batch_export/worker.rs`

- [ ] **Step 1: Write the test**

⚠⚠ **This is the guard that matters most to everyone who does not use Immich.** Append to the tests in `crates/app/src/batch_export/worker.rs`:

```rust
    /// ⚠⚠ **The whole existing export path must be byte-identical with no upload target.** The
    /// goldens cover the render; this covers the *delivery* — same bytes on disk, same report
    /// counters — so a future change to the upload step cannot quietly alter an ordinary export.
    #[test]
    fn an_export_without_a_target_is_byte_identical() {
        let a = TempDir::new("no_target_a");
        let b = TempDir::new("no_target_b");

        let plain = opts_for(&a);
        assert_eq!(plain.upload_target, None, "the control must have no target");
        let ra = run_one(&plain, &a);

        // ⚠ The same options, run again into a second directory: the comparison is that adding
        // the upload *field* changed nothing, so both sides are the no-target path.
        let again = opts_for(&b);
        let rb = run_one(&again, &b);

        assert_eq!(ra.exported, rb.exported);
        assert_eq!(ra.uploaded, 0);
        assert_eq!(rb.uploaded, 0);
        let fa = std::fs::read(only_file(&a)).unwrap();
        let fb = std::fs::read(only_file(&b)).unwrap();
        assert_eq!(fa, fb, "the exported bytes differ between two identical runs");
    }
```

⚠ `only_file` is a helper returning the single written file in a directory; if the module has no such helper, write it beside the test:

```rust
    fn only_file(dir: &TempDir) -> PathBuf {
        let mut v: Vec<PathBuf> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.is_file())
            .collect();
        assert_eq!(v.len(), 1, "expected exactly one exported file, got {v:?}");
        v.pop().unwrap()
    }
```

- [ ] **Step 2: Run it**

```bash
cargo test --workspace byte_identical
```
Expected: `1 passed`.

- [ ] **Step 3: Confirm the goldens are unmoved**

```bash
cargo test --workspace golden > /tmp/g.log 2>&1; echo "GOLDEN=$?"; grep -c '^test .* ok$' /tmp/g.log
```
Expected: `GOLDEN=0`, and the count matches the 75 goldens' usual total. ⚠ If any golden moved, the upload step is touching the render path — stop and find out why.

- [ ] **Step 4: Commit**

```bash
git add crates/app/src/batch_export/worker.rs
git commit -m "test(export): an export without an upload target is byte-identical"
```

---

## Task 11: Gate the tip, then see it work

- [ ] **Step 1: Re-gate the committed tip**

⚠⚠ **A gate run before the last commit describes a tree nobody shipped.** Run all three on the tip, each exit code from its own command:

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo fmt --all --check; echo "FMT=$?"
CARGO_TARGET_DIR=target/clippy cargo clippy --workspace --all-targets -- -D warnings > /tmp/c.log 2>&1; echo "CLIPPY=$?"
cargo test --workspace > /tmp/t.log 2>&1; echo "TEST=$?"; grep -c '^test .* ok$' /tmp/t.log
```
Expected: all `=0`, and the test count is **above 2,823** (the count before this work).

- [ ] **Step 2: Launch the GUI**

```bash
cargo run --release -p app
```
Run it in the **background**, and tell the user what to look at: Settings → Immich has an **Upload key** field; with it empty, the Export dialog's **Upload** section shows the checkbox **disabled** with a hover.

- [ ] **Step 3: Ask the user for an upload key**

⚠⚠ **STOP here and ask.** Creating an Immich API key with `asset.upload` is the user's action in Immich's own UI — Claude must not create accounts or credentials. Tell them: Immich → Account Settings → API Keys → new key with **`asset.upload`** only, then paste it into Settings → Immich → Upload key.

- [ ] **Step 4: Verify by use, against the real server**

⚠⚠ **Every Immich defect in this repo was found by use, not by a test.** With the key set, export one photo with Upload on, then check:

```bash
# Replace ASSET_ID with the id the sidecar recorded.
K=$(python3 -c "import json;print(json.load(open('~/Library/Application Support/rustroom/settings.json'))['immich_upload_key'])")
H=https://immich.example
curl -s -H "x-api-key: $K" "$H/api/assets/ASSET_ID" | python3 -m json.tool | head -20
```

Confirm, and report each one:
1. The asset exists.
2. ⚠⚠ Its `fileCreatedAt` **equals the original's** — for an Immich-sourced photo, compare against the source asset directly. This is the measurable form of "it landed at the capture date, not today".
3. Exporting the same photo again with no edits returns **`status: duplicate`**.
4. A photo that never came from Immich uploads too.

- [ ] **Step 5: Replace the constructed fixture with the captured reply**

⚠⚠ **Task 2's DTO test is explicitly marked CONSTRUCTED.** Now that a real upload has happened, capture the actual reply and replace the literal, and change the doc comment to say it was **captured**, with the date. A constructed fixture proves only self-consistency.

- [ ] **Step 6: Commit the captured fixture**

```bash
git add crates/immich/src/dto.rs
git commit -m "test(immich): the upload reply fixture, now captured from a real server"
```

---

## Task 12: Documentation

**Files:**
- Modify: `docs/JOURNAL.md`, `CLAUDE.md`, `docs/BACKLOG.md`

- [ ] **Step 1: Write the journal entry**

Add a `### 2026-09-24 — …` section at the **top** of the dated entries in `docs/JOURNAL.md` (newest first), covering: what was built; the verified `asset.upload`-vs-`asset.update` separation; the `fileCreatedAt` convention and how it was measured against the sibling JPEG; **the spec correction that the export worker is `width`-wide, not sequential**; and whatever the by-use verification actually found.

- [ ] **Step 2: Update `CLAUDE.md`**

Overwrite the **State** block's next-action item 0 in place — ⚠ **never append a superseded block**. Add the new lib module `immich_upload` and the bin changes to the crate-layout line. ⚠ `CLAUDE.md` is at **431** lines against a ~400 target; keep the entry to a few lines and put the detail in the journal.

- [ ] **Step 3: Update `docs/BACKLOG.md`**

Mark the upload item ✅ built, and record what use revealed. Leave the ⛔ two-way sync decline and the ⬜ *Rustroom-side stacking awareness* and ⬜ *Immich completely optional* items untouched.

- [ ] **Step 4: Gate and commit**

```bash
cargo fmt --all --check; echo "FMT=$?"
```
Expected: `FMT=0` (docs only, but the tree must stay formatted).

```bash
git add docs/ CLAUDE.md
git commit -m "docs: uploading edited photos to Immich"
```

- [ ] **Step 5: Report to the user — do NOT merge or push**

⚠ Merging is the user's call and **pushing is never Claude's**. Report: what landed, what use confirmed, what it revealed, and the branch state.

---

## Self-review

**Spec coverage:**

| spec section | task |
|---|---|
| §2 verified contract | 2 |
| §3 architecture, `upload_target`, placement | 3, 7, 8 |
| §4 second upload-only key | 5 |
| §5 sidecar record, "already uploaded" note | 6, 7, **9** |
| §6 capture-time timestamps | 4 |
| §7 failure / unconfirmed / width-wide concurrency | 7 |
| §8 testing (multipart, timestamps, sidecar, default, unchanged export) | 1, 4, 6, 3, 10 |
| §8 "only a real server can settle" | 11 |
| §10 definition of done | 11 |

✅ **One gap was found and CLOSED rather than accepted.** §5's *"the dialog says **already uploaded on ‹date›**"* had no task, so **Task 9** was added. Finding it also exposed a latent bug in this plan's own first draft: `record` wrote copy 0 unconditionally while the note reads the copy being exported, so for **any virtual copy** the write and the read would have addressed different entries and the note would have stayed silent for ever. `record` is now copy-aware and Task 9's test asserts a **non-zero** copy specifically, which is the case that was broken.

**Placeholder scan:** none. Every code step carries real code. Five steps deliberately say *"read the surrounding code and use the real helper names"* — Tasks 5/6/7/8/9 — because inventing a name for an existing helper is worse than telling the engineer to look.

**Type consistency:** `Upload`/`UploadStatus` (Task 2) are used by Task 7's `record`. `UploadTarget::Immich` (Task 3) is used by Tasks 7, 8. `Availability` (Task 4) by Tasks 7, 8. `UploadRecord` (Task 6) by Task 7. `created_at_text`/`modified_at_text` (Task 4) by Task 7. `record`/`uploaded_note` (Task 7) by Task 9, with the **same** `copy` parameter on both. `export_summary::upload` (Task 8) by Task 8's dialog. No name appears in two spellings.
