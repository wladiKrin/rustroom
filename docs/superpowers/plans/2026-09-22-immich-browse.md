# Immich's first slice — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** An "Immich" section in the left rail, at the same level as Folders and Collections, that lists the server's albums, fills the ordinary grid with thumbnails fetched from the server, and downloads the RAW original when a photo is opened so the existing develop pipeline and export work on it unchanged.

**Architecture:** Shape A of `docs/BACKLOG.md`'s Immich integration — an asset becomes a local file in a cache directory, and a cache directory is just a folder. The cache path is derived **deterministically from the asset UUID**, so it is known before the file exists; that lets `catalog::CatalogEntry` rows be built for undownloaded assets and fed to `RustroomApp::open_scan`, the one place a `Library` is constructed. No catalog schema change (`db.rs` stays at v7). New crate `crates/immich` holds the HTTP client with zero GUI dependency.

**Tech Stack:** Rust, `ureq` 3 (blocking — the app has no async runtime and every background job is a plain thread), `serde`/`serde_json`, `thiserror`, the `image` crate with a newly added `webp` feature, `egui` 0.35 in the bin modules only.

**Spec:** `docs/superpowers/specs/2026-09-22-immich-browse-design.md`. Read it before Task 1.

---

## Conventions for every task

Run this once per shell before any cargo command:

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

- **Commit messages carry NO `Co-Authored-By` trailer.** Project rule, overrides any default.
- **Never push.** The user pushes.
- Branch is already `immich-browse`. Do not create another.
- **egui only in bin modules** (`immich_panel.rs`, `modals/settings.rs`, `collections_panel.rs`, `main.rs`). `immich_source.rs` and `immich_worker.rs` are lib modules and must not import egui.
- A per-task test run is `cargo test -p <crate> <filter>`. **The full gate runs once, in Task 12** — a fresh test binary stalls ~22 s on this Mac, so the workspace gate is 15–30 minutes and is not run per task.
- ⚠ **Never run two `cargo test` invocations concurrently** — seven temp-dir test sites collide.

## File structure

| File | Responsibility |
|---|---|
| `Cargo.toml` | Workspace member, `ureq`, `webp` added to `image` features |
| `crates/immich/Cargo.toml` | The client crate's manifest |
| `crates/immich/src/lib.rs` | `Client`, `Size`, re-exports. The only file that performs HTTP |
| `crates/immich/src/dto.rs` | `About`, `AlbumSummary`, `Asset`, `AssetType` — serde only, no HTTP |
| `crates/immich/src/error.rs` | `Error` enum and status-code mapping |
| `crates/immich/tests/fixtures/*.json` | Recorded response bodies. No test touches the network |
| `crates/app/src/immich_source.rs` | Lib, no egui: cache paths, `entries_from_album`, `is_cached`, the settings accessor |
| `crates/app/src/immich_worker.rs` | Lib, no egui: the one worker, its `Job` enum, `request`/`poll` |
| `crates/app/src/immich_panel.rs` | Bin: the rail section's body and its three states |
| `crates/app/src/collections_panel.rs` | One `section(...)` call added (file is 840 lines — nothing else) |
| `crates/app/src/thumb_loader.rs` | The remote branch in `new`'s closure (file is 1288 lines — nothing else) |
| `crates/app/src/sources.rs` | `open_immich_album`, beside the existing loaders |
| `crates/app/src/settings.rs` | Two `String` fields (file is 2280 lines — nothing else) |
| `crates/app/src/modals/settings.rs` | Two draft fields and a Connection group |
| `crates/app/src/main.rs` | `LibrarySource::ImmichAlbum`, module declarations |
| `crates/app/src/lib.rs` | `pub mod immich_source; pub mod immich_worker;` |

---

### Task 1: The crate skeleton and its error type

**Files:**
- Modify: `Cargo.toml`
- Create: `crates/immich/Cargo.toml`, `crates/immich/src/lib.rs`, `crates/immich/src/error.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/immich/src/error.rs` containing only this test module at first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_401_is_unauthorized_not_a_generic_server_error() {
        assert!(matches!(Error::from_status(401), Error::Unauthorized));
        assert!(matches!(Error::from_status(403), Error::Unauthorized));
    }

    #[test]
    fn a_404_is_not_found() {
        assert!(matches!(Error::from_status(404), Error::NotFound));
    }

    #[test]
    fn any_other_failure_carries_its_code() {
        assert!(matches!(Error::from_status(503), Error::Server(503)));
    }

    /// ⚠ The UI has to tell "your key is wrong" from "your server is down", so these must not
    /// collapse into one message.
    #[test]
    fn the_three_kinds_read_differently() {
        let a = Error::Unauthorized.to_string();
        let b = Error::NotFound.to_string();
        let c = Error::Server(503).to_string();
        assert_ne!(a, b);
        assert_ne!(b, c);
        assert_ne!(a, c);
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

```bash
cargo test -p immich
```

Expected: FAIL — the package does not exist yet.

- [ ] **Step 3: Create the manifest and wire the workspace**

`crates/immich/Cargo.toml`:

```toml
[package]
name = "immich"
version = "0.1.0"
edition.workspace = true
license.workspace = true
repository.workspace = true

[dependencies]
serde = { workspace = true }
serde_json = { workspace = true }
thiserror = { workspace = true }
ureq = { workspace = true }
```

In the root `Cargo.toml`, add `"crates/immich",` to `[workspace] members` (after `"crates/image-decode",` keeps it tidy), and add to `[workspace.dependencies]`:

```toml
ureq = "3"
```

⚠ Also in the root `Cargo.toml`, change the `image` line to add `webp` — Immich's thumbnails are webp by default and the workspace cannot decode them today:

```toml
image = { version = "0.25", default-features = false, features = ["tiff", "jpeg", "png", "webp"] }
```

- [ ] **Step 4: Write the error type above the test module**

At the top of `crates/immich/src/error.rs`:

```rust
//! What can go wrong talking to an Immich server.
//!
//! ⚠⚠ The variants exist so the UI can say **which** thing went wrong. A single opaque error
//! makes "your API key is wrong" indistinguishable from "your server is unreachable", which are
//! the two failures a self-hosted setup actually hits and which need opposite responses.

/// A failed call to the server.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the server rejected the API key")]
    Unauthorized,
    #[error("the server has no such item")]
    NotFound,
    #[error("the server returned an error (HTTP {0})")]
    Server(u16),
    #[error("could not reach the server: {0}")]
    Network(String),
    #[error("the server's reply could not be understood: {0}")]
    Decode(String),
    #[error("writing the downloaded file failed: {0}")]
    Io(String),
}

impl Error {
    /// Map an HTTP status to the variant the UI should show.
    pub fn from_status(code: u16) -> Self {
        match code {
            401 | 403 => Error::Unauthorized,
            404 => Error::NotFound,
            other => Error::Server(other),
        }
    }
}
```

Create `crates/immich/src/lib.rs`:

```rust
//! A narrow client for an Immich server.
//!
//! ⚠ **Zero GUI, and no knowledge of Rustroom's types.** It speaks Immich's JSON and returns
//! plain data; mapping that onto `CatalogEntry`/`PhotoId` is `app`'s job, in `immich_source`.

pub mod error;

pub use error::Error;
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cargo test -p immich
```

Expected: PASS, 4 tests.

- [ ] **Step 6: Commit**

```bash
git add Cargo.toml crates/immich
git commit -m "The immich crate's skeleton and an error type that distinguishes a bad key from an unreachable server

Also adds webp to the image crate's features: Immich's thumbnails are webp by
default and the workspace was built with tiff/jpeg/png only, so it could not have
decoded what the thumbnail endpoint returns."
```

---

### Task 2: The DTOs

**Files:**
- Create: `crates/immich/src/dto.rs`, `crates/immich/tests/fixtures/albums.json`, `crates/immich/tests/fixtures/album.json`, `crates/immich/tests/fixtures/about.json`
- Modify: `crates/immich/src/lib.rs`

- [ ] **Step 1: Write the fixtures**

`crates/immich/tests/fixtures/about.json` — trimmed from a real `GET /server/about`:

```json
{ "version": "3.2.0", "versionUrl": "https://github.com/immich-app/immich/releases/tag/v3.2.0" }
```

`crates/immich/tests/fixtures/albums.json` — trimmed from `GET /albums`:

```json
[
  { "id": "6f2e1a94-0c44-4f51-8a7d-9b1c2e3f4a5b", "albumName": "Iceland 2025", "assetCount": 412 },
  { "id": "7a3f2b05-1d55-4062-9b8e-ac2d3f4a5b6c", "albumName": "Family", "assetCount": 38 }
]
```

`crates/immich/tests/fixtures/album.json` — trimmed from `GET /albums/{id}`. ⚠ Deliberately mixes a RAW, a video and an unsupported RAW so the filtering in Task 4 has something real to reject:

```json
{
  "id": "6f2e1a94-0c44-4f51-8a7d-9b1c2e3f4a5b",
  "albumName": "Iceland 2025",
  "assets": [
    {
      "id": "11111111-1111-4111-8111-111111111111",
      "type": "IMAGE",
      "originalFileName": "DSCF1234.RAF",
      "originalMimeType": "image/x-fuji-raf",
      "width": 6240, "height": 4160,
      "thumbhash": "1QcSHQRnh493V4dIh4eXh1h4kJUI",
      "fileModifiedAt": "2025-08-14T09:21:33.000Z"
    },
    {
      "id": "22222222-2222-4222-8222-222222222222",
      "type": "VIDEO",
      "originalFileName": "MOV_0002.MP4",
      "originalMimeType": "video/mp4",
      "width": 3840, "height": 2160,
      "thumbhash": "2QcSHQRnh493V4dIh4eXh1h4kJUI",
      "fileModifiedAt": "2025-08-14T10:02:00.000Z"
    },
    {
      "id": "33333333-3333-4333-8333-333333333333",
      "type": "IMAGE",
      "originalFileName": "P1000003.PEF",
      "originalMimeType": "image/x-pentax-pef",
      "width": 4928, "height": 3264,
      "thumbhash": "3QcSHQRnh493V4dIh4eXh1h4kJUI",
      "fileModifiedAt": "2025-08-15T11:00:00.000Z"
    },
    {
      "id": "44444444-4444-4444-8444-444444444444",
      "type": "IMAGE",
      "originalFileName": "holiday snap.JPG",
      "originalMimeType": "image/jpeg",
      "width": 4000, "height": 3000,
      "thumbhash": "4QcSHQRnh493V4dIh4eXh1h4kJUI",
      "fileModifiedAt": "2025-08-16T12:30:00.000Z"
    }
  ]
}
```

- [ ] **Step 2: Write the failing test**

Append to `crates/immich/src/dto.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const ABOUT: &str = include_str!("../tests/fixtures/about.json");
    const ALBUMS: &str = include_str!("../tests/fixtures/albums.json");
    const ALBUM: &str = include_str!("../tests/fixtures/album.json");

    #[test]
    fn about_reads_the_version() {
        let a: About = serde_json::from_str(ABOUT).expect("about must parse");
        assert_eq!(a.version, "3.2.0");
    }

    #[test]
    fn the_album_list_reads_names_and_counts() {
        let v: Vec<AlbumSummary> = serde_json::from_str(ALBUMS).expect("albums must parse");
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].album_name, "Iceland 2025");
        assert_eq!(v[0].asset_count, 412);
    }

    #[test]
    fn one_album_reads_its_assets() {
        let a: AlbumDetail = serde_json::from_str(ALBUM).expect("album must parse");
        assert_eq!(a.assets.len(), 4);
        assert_eq!(a.assets[0].original_file_name, "DSCF1234.RAF");
        assert!(matches!(a.assets[0].asset_type, AssetType::Image));
        assert!(matches!(a.assets[1].asset_type, AssetType::Video));
    }

    /// ⚠ Immich keeps adding fields; a DTO that fails on an unknown one breaks on every server
    /// upgrade. Deserialisation must ignore what it does not know.
    #[test]
    fn an_unknown_field_is_ignored_rather_than_failing() {
        let json = r#"{"id":"11111111-1111-4111-8111-111111111111","type":"IMAGE",
            "originalFileName":"a.RAF","width":1,"height":1,"thumbhash":"x",
            "fileModifiedAt":"2025-08-14T09:21:33.000Z","somethingNewIn3_3":true}"#;
        let a: Asset = serde_json::from_str(json).expect("an unknown field must not fail the parse");
        assert_eq!(a.original_file_name, "a.RAF");
    }

    /// ⚠ `originalMimeType` is optional in the spec. A missing one must not fail the album.
    #[test]
    fn a_missing_mime_type_is_none_not_an_error() {
        let json = r#"{"id":"11111111-1111-4111-8111-111111111111","type":"IMAGE",
            "originalFileName":"a.RAF","width":1,"height":1,"thumbhash":"x",
            "fileModifiedAt":"2025-08-14T09:21:33.000Z"}"#;
        let a: Asset = serde_json::from_str(json).expect("a missing mime type must parse");
        assert!(a.original_mime_type.is_none());
    }

    /// ⚠⚠ An unrecognised asset type must not fail the whole album either — Immich could add one.
    #[test]
    fn an_unknown_asset_type_is_other_not_a_parse_failure() {
        let json = r#"{"id":"11111111-1111-4111-8111-111111111111","type":"AUDIO",
            "originalFileName":"a.mp3","width":0,"height":0,"thumbhash":"x",
            "fileModifiedAt":"2025-08-14T09:21:33.000Z"}"#;
        let a: Asset = serde_json::from_str(json).expect("an unknown type must parse");
        assert!(matches!(a.asset_type, AssetType::Other));
    }
}
```

- [ ] **Step 3: Run it to verify it fails**

```bash
cargo test -p immich dto
```

Expected: FAIL — `About`, `AlbumSummary`, `AlbumDetail`, `Asset`, `AssetType` do not exist.

- [ ] **Step 4: Write the DTOs above the test module**

At the top of `crates/immich/src/dto.rs`:

```rust
//! The parts of Immich's JSON this client reads.
//!
//! ⚠⚠ **Deliberately narrow.** These mirror only the fields used, not Immich's full
//! `AssetResponseDto` — 24 of its endpoints already carry a `Deprecated` tag, and a narrow DTO
//! is a narrow blast radius when the API moves.
//!
//! ⚠ Every struct ignores unknown fields (serde's default) and every enum has an `Other`
//! fallback, so a server upgrade that adds a field or a kind does not break browsing.

use serde::Deserialize;

/// `GET /server/about` — the version probe.
#[derive(Debug, Clone, Deserialize)]
pub struct About {
    pub version: String,
}

/// One row of `GET /albums`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumSummary {
    pub id: String,
    pub album_name: String,
    pub asset_count: u32,
}

/// `GET /albums/{id}` — the album plus its assets.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumDetail {
    pub id: String,
    pub album_name: String,
    pub assets: Vec<Asset>,
}

/// What kind of thing an asset is.
///
/// ⚠ `Other` catches anything Immich adds later. Without it one unknown kind would fail the
/// whole album's parse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub enum AssetType {
    #[serde(rename = "IMAGE")]
    Image,
    #[serde(rename = "VIDEO")]
    Video,
    #[serde(other)]
    Other,
}

/// One asset, narrowed to what the browse grid and the download need.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Asset {
    pub id: String,
    #[serde(rename = "type")]
    pub asset_type: AssetType,
    /// The name the file had on the camera — what the grid shows, never the UUID.
    pub original_file_name: String,
    pub original_mime_type: Option<String>,
    pub width: u32,
    pub height: u32,
    /// Base64 thumbhash. Carried for the blurred placeholder; unused in this slice.
    pub thumbhash: String,
    /// RFC 3339. Kept as a string here and parsed in `app`, so this crate needs no time crate.
    pub file_modified_at: String,
}
```

Add to `crates/immich/src/lib.rs`:

```rust
pub mod dto;

pub use dto::{About, AlbumDetail, AlbumSummary, Asset, AssetType};
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cargo test -p immich
```

Expected: PASS, 10 tests.

- [ ] **Step 6: Commit**

```bash
git add crates/immich
git commit -m "Immich's DTOs, narrowed to what browsing needs, with fixtures that include a video and an unsupported RAW

Unknown fields and unknown asset types are ignored rather than failing the parse:
a server upgrade that adds either must not stop an album from opening."
```

---

### Task 3: The client's HTTP calls

**Files:**
- Modify: `crates/immich/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Append to `crates/immich/src/lib.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_trailing_slash_on_the_base_url_does_not_double_up() {
        let a = Client::new("https://photos.example.com/", "k").expect("must build");
        let b = Client::new("https://photos.example.com", "k").expect("must build");
        assert_eq!(a.url("/albums"), b.url("/albums"));
        assert_eq!(a.url("/albums"), "https://photos.example.com/api/albums");
    }

    /// ⚠ Immich is served under `/api`. Forgetting it gives a 404 on every call, which reads as
    /// "the server has no albums" rather than "the URL is wrong".
    #[test]
    fn the_api_prefix_is_added_once_even_if_the_user_typed_it() {
        let c = Client::new("https://photos.example.com/api", "k").expect("must build");
        assert_eq!(c.url("/albums"), "https://photos.example.com/api/albums");
    }

    #[test]
    fn an_empty_url_or_key_is_refused_before_any_request() {
        assert!(Client::new("", "k").is_err());
        assert!(Client::new("https://photos.example.com", "").is_err());
    }

    /// ⚠⚠ The size values are Immich's `AssetMediaSize` spelling. A typo here is a 400 at runtime
    /// and nowhere else.
    #[test]
    fn the_sizes_spell_immichs_own_values() {
        assert_eq!(Size::Thumbnail.as_str(), "thumbnail");
        assert_eq!(Size::Preview.as_str(), "preview");
        assert_eq!(Size::Fullsize.as_str(), "fullsize");
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

```bash
cargo test -p immich
```

Expected: FAIL — `Client` and `Size` do not exist.

- [ ] **Step 3: Write the client**

Replace `crates/immich/src/lib.rs`'s body (keeping the module doc comment, the `pub mod`s and the test module) with:

```rust
pub mod dto;
pub mod error;

pub use dto::{About, AlbumDetail, AlbumSummary, Asset, AssetType};
pub use error::Error;

use std::io::Write;
use std::path::Path;

/// Which rendition of an asset to fetch. Immich's own `AssetMediaSize` spelling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Size {
    Thumbnail,
    Preview,
    Fullsize,
}

impl Size {
    pub fn as_str(self) -> &'static str {
        match self {
            Size::Thumbnail => "thumbnail",
            Size::Preview => "preview",
            Size::Fullsize => "fullsize",
        }
    }
}

/// A connection to one Immich server.
///
/// ⚠ **Certificate verification is never disabled.** A self-signed certificate fails to connect
/// and says so; silently trusting any certificate is not something to ship, and the fix belongs
/// on the server.
pub struct Client {
    /// Base URL including the `/api` prefix, with no trailing slash.
    base: String,
    key: String,
    agent: ureq::Agent,
}

impl Client {
    pub fn new(base_url: &str, api_key: &str) -> Result<Self, Error> {
        let trimmed = base_url.trim().trim_end_matches('/');
        if trimmed.is_empty() {
            return Err(Error::Network("no server URL is set".into()));
        }
        if api_key.trim().is_empty() {
            return Err(Error::Unauthorized);
        }
        // ⚠ The user may or may not have typed the `/api` prefix. Adding it unconditionally
        // would give `/api/api`, and leaving it off gives a 404 on every call.
        let base = if trimmed.ends_with("/api") {
            trimmed.to_string()
        } else {
            format!("{trimmed}/api")
        };
        Ok(Self {
            base,
            key: api_key.trim().to_string(),
            agent: ureq::Agent::new_with_defaults(),
        })
    }

    /// The absolute URL for an API path beginning with `/`.
    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base, path)
    }

    /// GET `path` and deserialise the JSON body.
    fn get_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, Error> {
        let body = self.get_text(path)?;
        serde_json::from_str(&body).map_err(|e| Error::Decode(e.to_string()))
    }

    fn get_text(&self, path: &str) -> Result<String, Error> {
        let mut res = self
            .agent
            .get(&self.url(path))
            .header("x-api-key", &self.key)
            .header("Accept", "application/json")
            .call()
            .map_err(map_ureq)?;
        res.body_mut()
            .read_to_string()
            .map_err(|e| Error::Decode(e.to_string()))
    }

    fn get_bytes(&self, path: &str) -> Result<Vec<u8>, Error> {
        let mut res = self
            .agent
            .get(&self.url(path))
            .header("x-api-key", &self.key)
            .call()
            .map_err(map_ureq)?;
        res.body_mut()
            .with_config()
            .limit(u64::MAX)
            .read_to_vec()
            .map_err(|e| Error::Network(e.to_string()))
    }

    /// `GET /server/about` — the version probe, and what the Settings Test button calls.
    pub fn about(&self) -> Result<About, Error> {
        self.get_json("/server/about")
    }

    /// `GET /albums`.
    pub fn albums(&self) -> Result<Vec<AlbumSummary>, Error> {
        self.get_json("/albums")
    }

    /// `GET /albums/{id}` — the album's assets, in the album's own order.
    pub fn album(&self, id: &str) -> Result<Vec<Asset>, Error> {
        let detail: AlbumDetail = self.get_json(&format!("/albums/{id}"))?;
        Ok(detail.assets)
    }

    /// `GET /assets/{id}/thumbnail?size=…` — the raw encoded bytes, format unspecified.
    ///
    /// ⚠ The server decides whether that is webp or jpeg (both are admin settings), so the
    /// caller must sniff rather than assume. `image::load_from_memory` does.
    pub fn thumbnail(&self, id: &str, size: Size) -> Result<Vec<u8>, Error> {
        self.get_bytes(&format!("/assets/{}/thumbnail?size={}", id, size.as_str()))
    }

    /// `GET /assets/{id}/original` — **the true original file**, streamed to `dest`.
    ///
    /// ⚠⚠ Streamed, never buffered: these are 25–50 MB RAWs and several may be in flight.
    ///
    /// ⚠⚠ `dest` is written **in full or not at all** by the caller's rename discipline — this
    /// function writes exactly where it is told, so callers pass a `.part` path.
    ///
    /// ⚠ Never `?edited=true`. Immich's own edits are crop/rotate/mirror and Rustroom applies
    /// its own geometry; importing theirs would apply a crop twice.
    pub fn original(&self, id: &str, dest: &Path) -> Result<(), Error> {
        let mut res = self
            .agent
            .get(&self.url(&format!("/assets/{id}/original")))
            .header("x-api-key", &self.key)
            .call()
            .map_err(map_ureq)?;
        let mut file = std::fs::File::create(dest).map_err(|e| Error::Io(e.to_string()))?;
        let mut reader = res.body_mut().as_reader();
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            let n = std::io::Read::read(&mut reader, &mut buf).map_err(|e| Error::Network(e.to_string()))?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n]).map_err(|e| Error::Io(e.to_string()))?;
        }
        file.flush().map_err(|e| Error::Io(e.to_string()))?;
        Ok(())
    }
}

/// Map a `ureq` failure onto this crate's error, keeping the status distinction the UI needs.
fn map_ureq(e: ureq::Error) -> Error {
    match e {
        ureq::Error::StatusCode(code) => Error::from_status(code),
        other => Error::Network(other.to_string()),
    }
}
```

⚠⚠ **`ureq` 3's exact method names must be checked against the version Cargo resolves** — read `cargo doc -p ureq --open` or the resolved source rather than trusting this block. The shape (agent, header, call, body reader) is right; `with_config().limit(..)` and `as_reader()` are the details most likely to have moved. Fix them here, do not work around them at call sites.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p immich
```

Expected: PASS, 14 tests.

- [ ] **Step 5: Clippy this crate alone before moving on**

```bash
cargo clippy -p immich --all-targets -- -D warnings
```

Expected: no warnings.

- [ ] **Step 6: Commit**

```bash
git add crates/immich
git commit -m "The Immich client: albums, thumbnails, and the original streamed to a destination

The /api prefix is added exactly once whether or not the user typed it, since
getting it wrong 404s every call and reads as an empty library. The original is
streamed rather than buffered, and never requested with ?edited=true."
```

---

### Task 4: Cache paths and album-to-entries mapping

**Files:**
- Create: `crates/app/src/immich_source.rs`
- Modify: `crates/app/src/lib.rs`, `crates/app/Cargo.toml`

- [ ] **Step 1: Write the failing test**

Create `crates/app/src/immich_source.rs` with only this test module at first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn asset(id: &str, name: &str, kind: immich::AssetType) -> immich::Asset {
        immich::Asset {
            id: id.to_string(),
            asset_type: kind,
            original_file_name: name.to_string(),
            original_mime_type: None,
            width: 100,
            height: 100,
            thumbhash: "x".into(),
            file_modified_at: "2025-08-14T09:21:33.000Z".into(),
        }
    }

    /// ⚠⚠ **The whole design rests on this.** The path must be knowable before the file exists
    /// and identical every time, or a re-download would produce a second identity and anything
    /// that recorded the first would dangle.
    #[test]
    fn a_cache_path_is_the_same_every_time_for_the_same_asset() {
        let root = std::path::Path::new("/c/immich/host");
        let a = asset("11111111-1111-4111-8111-111111111111", "DSCF1234.RAF", immich::AssetType::Image);
        assert_eq!(cache_path(root, &a), cache_path(root, &a));
    }

    #[test]
    fn a_cache_path_keeps_the_uuid_and_the_extension() {
        let root = std::path::Path::new("/c/immich/host");
        let a = asset("11111111-1111-4111-8111-111111111111", "DSCF1234.RAF", immich::AssetType::Image);
        let p = cache_path(root, &a);
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        assert!(name.starts_with("11111111-1111-4111-8111-111111111111"));
        assert!(name.ends_with(".RAF"), "the extension decides the decoder: {name}");
    }

    /// ⚠ A filename from the server is untrusted input. It must not be able to escape the cache.
    #[test]
    fn a_hostile_original_name_cannot_escape_the_cache_directory() {
        let root = std::path::Path::new("/c/immich/host");
        let a = asset("11111111-1111-4111-8111-111111111111", "../../../etc/passwd.RAF", immich::AssetType::Image);
        let p = cache_path(root, &a);
        assert_eq!(p.parent().unwrap(), root, "must stay directly inside the cache root");
        assert!(!p.to_string_lossy().contains(".."));
    }

    #[test]
    fn two_servers_get_different_roots() {
        assert_ne!(
            host_slug("https://photos.example.com"),
            host_slug("https://other.example.com")
        );
    }

    #[test]
    fn a_host_slug_survives_a_port_a_path_and_odd_characters() {
        let s = host_slug("https://photos.example.com:2283/immich?x=1");
        assert!(!s.is_empty());
        assert!(!s.contains('/'), "a slug is one directory name: {s}");
        assert!(!s.contains(':'), "{s}");
    }

    /// ⚠ `host_slug` must never return an empty string — that would put the cache at the root of
    /// every server's shared directory and collide them all.
    #[test]
    fn a_useless_url_still_yields_a_usable_slug() {
        assert!(!host_slug("://").is_empty());
        assert!(!host_slug("").is_empty());
    }

    #[test]
    fn videos_and_unsupported_raws_are_skipped_and_the_rest_keep_their_human_names() {
        let root = std::path::Path::new("/c/immich/host");
        let assets = vec![
            asset("11111111-1111-4111-8111-111111111111", "DSCF1234.RAF", immich::AssetType::Image),
            asset("22222222-2222-4222-8222-222222222222", "MOV_0002.MP4", immich::AssetType::Video),
            asset("33333333-3333-4333-8333-333333333333", "P1000003.PEF", immich::AssetType::Image),
            asset("44444444-4444-4444-8444-444444444444", "holiday snap.JPG", immich::AssetType::Image),
        ];
        let entries = entries_from_album(&assets, root);
        let names: Vec<&str> = entries.iter().map(|e| e.file_name.as_str()).collect();
        assert_eq!(names, vec!["DSCF1234.RAF", "holiday snap.JPG"]);
    }

    #[test]
    fn an_entry_carries_the_servers_modified_time_not_now() {
        let root = std::path::Path::new("/c/immich/host");
        let a = asset("11111111-1111-4111-8111-111111111111", "DSCF1234.RAF", immich::AssetType::Image);
        let e = &entries_from_album(&[a], root)[0];
        let secs = e.modified.duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        // 2025-08-14T09:21:33Z
        assert_eq!(secs, 1_755_163_293);
    }

    /// ⚠ An unparseable timestamp must not drop the photo — it sorts oldest, the same rule
    /// `CatalogEntry::modified` already documents for an unreadable file.
    #[test]
    fn an_unparseable_timestamp_becomes_the_epoch_rather_than_dropping_the_photo() {
        let root = std::path::Path::new("/c/immich/host");
        let mut a = asset("11111111-1111-4111-8111-111111111111", "DSCF1234.RAF", immich::AssetType::Image);
        a.file_modified_at = "not a date".into();
        let entries = entries_from_album(&[a], root);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].modified, std::time::UNIX_EPOCH);
    }

    #[test]
    fn is_cached_is_false_before_the_file_exists_and_true_after() {
        let dir = std::env::temp_dir().join(format!("rustroom_immich_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.RAF");
        let _ = std::fs::remove_file(&p);
        assert!(!is_cached(&p));
        std::fs::write(&p, b"x").unwrap();
        assert!(is_cached(&p));
        let _ = std::fs::remove_file(&p);
        let _ = std::fs::remove_dir(&dir);
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

```bash
cargo test -p app immich_source
```

Expected: FAIL — the module is not declared and its functions do not exist.

- [ ] **Step 3: Write the module above the test module**

⚠ The timestamp needs parsing without a new dependency. Immich emits RFC 3339 with milliseconds and a `Z`; parse the fixed-width fields by hand and convert with a days-from-civil calculation. That is less code than it sounds and avoids adding `chrono`/`time` to this workspace.

At the top of `crates/app/src/immich_source.rs`:

```rust
//! Turning Immich assets into the rows the ordinary Library is built from.
//!
//! ⚠⚠ **The idea the whole Immich slice rests on**: the cache path is derived deterministically
//! from the asset UUID, so it is known **before the file exists**. That is what lets a
//! `CatalogEntry` — and therefore a `PhotoId`, a `PhotoEntry` and a whole `Library` — be built
//! for an asset that has never been downloaded, with no new grid, no new selection model and no
//! new filmstrip. Opening the photo makes the path real and every existing code path then
//! applies with no special case.
//!
//! ⚠ **No egui here.** This is the unit-testable half; the rail section lives in
//! `immich_panel.rs`.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// A filesystem-safe directory name for one server.
///
/// ⚠ Two servers must not share a cache directory: the same UUID on two servers is two
/// different photos. ⚠ Never empty — an empty slug would put every server's cache in one place
/// and collide them all.
pub fn host_slug(base_url: &str) -> String {
    let s = crate::safe_name::sanitize(base_url.trim());
    let s = s.trim_matches('_').trim().to_string();
    if s.is_empty() {
        "server".to_string()
    } else {
        s
    }
}

/// Where this server's originals are cached.
pub fn cache_root(base_url: &str) -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("rustroom")
        .join("immich")
        .join(host_slug(base_url))
}

/// The one path an asset's original always occupies.
///
/// ⚠⚠ Deterministic, and that is load-bearing: re-downloading an evicted or deleted file must
/// restore **byte-identical identity**, or anything that recorded the old path dangles.
///
/// ⚠ The UUID leads so two cameras' `DSC_0001.NEF` cannot collide. The extension is preserved
/// because it is what picks the decoder. The original name is sanitised — a filename from the
/// server is untrusted input and must not be able to escape the cache directory.
pub fn cache_path(root: &Path, asset: &immich::Asset) -> PathBuf {
    let ext = Path::new(&asset.original_file_name)
        .extension()
        .map(|e| e.to_string_lossy().to_string())
        .unwrap_or_default();
    let stem = crate::safe_name::sanitize(&asset.id);
    let name = if ext.is_empty() {
        stem
    } else {
        format!("{}.{}", stem, crate::safe_name::sanitize(&ext))
    };
    root.join(name)
}

/// Has this asset's original been downloaded yet?
pub fn is_cached(path: &Path) -> bool {
    path.is_file()
}

/// The album's assets as the rows `RustroomApp::open_scan` consumes.
///
/// ⚠⚠ Returns `catalog::CatalogEntry`, **not** `PhotoEntry`: `open_scan` calls
/// `entries::build(&scan, sidecar::load_all)` itself, so producing `CatalogEntry` reuses the
/// whole loader — sidecars, virtual copies, the filter, the `ThumbLoader` — rather than half of
/// it.
///
/// ⚠ Skips videos and anything `catalog::is_supported` rejects. Its `RAW_EXTENSIONS` is narrower
/// than Immich's (no `pef`, `srw`, `3fr`, `iiq`), so some RAWs on the server are genuinely not
/// editable here. Skipped assets are **absent**, not greyed out.
pub fn entries_from_album(assets: &[immich::Asset], root: &Path) -> Vec<catalog::CatalogEntry> {
    assets
        .iter()
        .filter(|a| a.asset_type == immich::AssetType::Image)
        .filter_map(|a| {
            let path = cache_path(root, a);
            if !catalog::is_supported(&path) {
                return None;
            }
            let edited = crate::sidecar::edited_at(&path);
            Some(catalog::CatalogEntry {
                path,
                file_name: a.original_file_name.clone(),
                modified: parse_rfc3339(&a.file_modified_at),
                edited,
            })
        })
        .collect()
}

/// Parse `2025-08-14T09:21:33.000Z` into a `SystemTime`.
///
/// ⚠ Hand-rolled rather than adding `chrono` or `time` to this workspace for one field.
/// ⚠ An unparseable value yields `UNIX_EPOCH` rather than dropping the photo — the same rule
/// `CatalogEntry::modified` already documents for a file whose metadata cannot be read, and it
/// sorts to the oldest end instead of failing the album.
fn parse_rfc3339(s: &str) -> SystemTime {
    fn num(s: &str) -> Option<i64> {
        s.parse().ok()
    }
    let bytes = s.as_bytes();
    if bytes.len() < 19 {
        return UNIX_EPOCH;
    }
    let (y, mo, d) = (num(&s[0..4]), num(&s[5..7]), num(&s[8..10]));
    let (h, mi, sec) = (num(&s[11..13]), num(&s[14..16]), num(&s[17..19]));
    let (Some(y), Some(mo), Some(d), Some(h), Some(mi), Some(sec)) = (y, mo, d, h, mi, sec) else {
        return UNIX_EPOCH;
    };
    let days = days_from_civil(y, mo, d);
    let secs = days * 86_400 + h * 3_600 + mi * 60 + sec;
    if secs < 0 {
        return UNIX_EPOCH;
    }
    UNIX_EPOCH + Duration::from_secs(secs as u64)
}

/// Days since 1970-01-01 for a proleptic Gregorian date. Howard Hinnant's `days_from_civil`.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}
```

Add to `crates/app/src/lib.rs`, in alphabetical position among the `pub mod` lines:

```rust
pub mod immich_source;
```

Add to `crates/app/Cargo.toml`'s `[dependencies]`:

```toml
immich = { path = "../immich" }
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p app immich_source
```

Expected: PASS, 10 tests. If `an_entry_carries_the_servers_modified_time_not_now` fails, the expected epoch second is wrong in the test, not the parser — check with `date -u -j -f "%Y-%m-%dT%H:%M:%SZ" "2025-08-14T09:21:33Z" +%s`.

- [ ] **Step 5: Commit**

```bash
git add crates/app/Cargo.toml crates/app/src/lib.rs crates/app/src/immich_source.rs
git commit -m "Deterministic Immich cache paths, and an album mapped to CatalogEntry rows

The path is derived from the asset UUID so it is known before the file exists,
which is what lets the ordinary Library be built for photos that have not been
downloaded. Returning CatalogEntry rather than PhotoEntry reuses the whole of
open_scan instead of half of it.

A filename from the server is untrusted input and is sanitised: it cannot escape
the cache directory."
```

---

### Task 5: The settings fields and their accessor

**Files:**
- Modify: `crates/app/src/settings.rs`, `crates/app/src/immich_source.rs`

- [ ] **Step 1: Write the failing test**

Append to `crates/app/src/immich_source.rs`'s test module:

```rust
    #[test]
    fn a_connection_needs_both_a_url_and_a_key() {
        let mut s = crate::settings::Settings::default();
        assert!(connection(&s).is_none(), "unconfigured by default");
        s.immich_url = "https://photos.example.com".into();
        assert!(connection(&s).is_none(), "a URL alone is not a connection");
        s.immich_key = "abc".into();
        let c = connection(&s).expect("both set is a connection");
        assert_eq!(c.url, "https://photos.example.com");
        assert_eq!(c.key, "abc");
    }

    /// ⚠ Whitespace pasted around a key is the commonest way this fails, and the failure looks
    /// like "the server rejected the key".
    #[test]
    fn surrounding_whitespace_is_trimmed_off_both() {
        let mut s = crate::settings::Settings::default();
        s.immich_url = "  https://photos.example.com  ".into();
        s.immich_key = "  abc\n".into();
        let c = connection(&s).expect("must be a connection");
        assert_eq!(c.url, "https://photos.example.com");
        assert_eq!(c.key, "abc");
    }
```

- [ ] **Step 2: Run it to verify it fails**

```bash
cargo test -p app immich_source
```

Expected: FAIL — `Settings` has no `immich_url`, and `connection` does not exist.

- [ ] **Step 3: Add the fields to `Settings`**

In `crates/app/src/settings.rs`, inside `pub struct Settings`, add:

```rust
    /// The Immich server's base URL, e.g. `https://photos.example.com`. Empty means unconfigured.
    ///
    /// ⚠ No field-level `#[serde(default)]`: the struct carries a container-level one, so a
    /// missing field already reads as `String::default()`, which is the right "not configured".
    pub immich_url: String,
    /// The Immich API key.
    ///
    /// ⚠⚠ **Stored in plaintext**, accepted deliberately for this slice with the OS keychain as
    /// the named follow-up. Read and written only through `immich_source::connection` and
    /// `immich_source::set_connection`, so moving it to a keychain is a change to one file.
    ///
    /// ⚠ The Settings field's help text tells the user to paste a **read-only** key
    /// (`asset.read`, `asset.view`, `asset.download`, `album.read`). A key that cannot write
    /// cannot damage the library if this file leaks, which is what bounds the exposure.
    pub immich_key: String,
```

And in its `Default` impl, beside the other fields:

```rust
            immich_url: String::new(),
            immich_key: String::new(),
```

- [ ] **Step 4: Add the accessor to `immich_source.rs`**

Above its test module:

```rust
/// A configured server, url and key both present.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connection {
    pub url: String,
    pub key: String,
}

/// The configured connection, or `None` when either half is missing.
///
/// ⚠⚠ **The only reader of `Settings::immich_key`.** Everything else asks for a `Connection`, so
/// moving the key into the OS keychain later changes this function and nothing else.
pub fn connection(s: &crate::settings::Settings) -> Option<Connection> {
    let url = s.immich_url.trim();
    let key = s.immich_key.trim();
    if url.is_empty() || key.is_empty() {
        return None;
    }
    Some(Connection {
        url: url.to_string(),
        key: key.to_string(),
    })
}

/// The only writer of those two fields, for the same reason.
pub fn set_connection(s: &mut crate::settings::Settings, url: &str, key: &str) {
    s.immich_url = url.trim().to_string();
    s.immich_key = key.trim().to_string();
}
```

- [ ] **Step 5: Run the tests to verify they pass**

```bash
cargo test -p app immich_source
```

Expected: PASS, 12 tests. ⚠ Other tests in `app` that construct `Settings` literally (not via `default()`) will fail to compile — add the two fields there too.

- [ ] **Step 6: Commit**

```bash
git add crates/app/src/settings.rs crates/app/src/immich_source.rs
git commit -m "The Immich server URL and API key, behind the one accessor that reads them

Both fields go through immich_source::connection/set_connection so moving the key
to the OS keychain later is a change to one file. No field-level serde(default):
Settings carries a container-level one and a second would be the trap the GPU
default flip already hit."
```

---

### Task 6: The worker

**Files:**
- Create: `crates/app/src/immich_worker.rs`
- Modify: `crates/app/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Create `crates/app/src/immich_worker.rs` with only this test module at first:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("rustroom_iw_{}_{}", tag, std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Block until `f` is true or the deadline passes, polling the worker.
    fn settle(w: &mut ImmichWorker, mut f: impl FnMut(&mut ImmichWorker) -> bool) {
        let start = std::time::Instant::now();
        while start.elapsed() < std::time::Duration::from_secs(5) {
            if f(w) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the worker never produced a result");
    }

    #[test]
    fn an_albums_job_comes_back_as_an_albums_result() {
        let mut w = ImmichWorker::new(
            Arc::new(|job: Job| match job {
                Job::Albums => Outcome::Albums(Ok(vec![])),
                _ => unreachable!(),
            }),
            || {},
        );
        w.request(Job::Albums);
        let mut got = Vec::new();
        settle(&mut w, |w| {
            got.extend(w.poll());
            !got.is_empty()
        });
        assert!(matches!(got[0], Outcome::Albums(Ok(_))));
    }

    /// ⚠⚠ **The discipline the whole cache rests on.** A download that fails partway must leave
    /// *nothing* at the destination — the design reads "the path exists" as "the bytes are all
    /// there", so a truncated RAW sitting at the real path would be decoded as if it were whole.
    #[test]
    fn a_download_that_fails_partway_leaves_no_file_at_the_destination() {
        let dir = temp_dir("fail");
        let dest = dir.join("11111111.RAF");
        let mut w = ImmichWorker::new(
            Arc::new({
                let dest = dest.clone();
                move |job: Job| match job {
                    Job::Original { dest: part, .. } => {
                        // Write some bytes to the .part, then fail — a dropped connection.
                        std::fs::write(&part, b"half a raw").unwrap();
                        assert_ne!(part, dest, "the fetcher must be given the .part path");
                        Outcome::Original {
                            dest: dest.clone(),
                            result: Err("connection reset".into()),
                        }
                    }
                    _ => unreachable!(),
                }
            }),
            || {},
        );
        w.request(Job::Original {
            asset_id: "11111111".into(),
            dest: dest.clone(),
        });
        let mut got = Vec::new();
        settle(&mut w, |w| {
            got.extend(w.poll());
            !got.is_empty()
        });
        assert!(!dest.exists(), "a failed download must not leave the real path in place");
        assert!(
            !part_path(&dest).exists(),
            "and it must not leave a .part behind either"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_successful_download_renames_into_place_exactly_once() {
        let dir = temp_dir("ok");
        let dest = dir.join("22222222.RAF");
        let mut w = ImmichWorker::new(
            Arc::new(move |job: Job| match job {
                Job::Original { dest: part, .. } => {
                    std::fs::write(&part, b"a whole raw").unwrap();
                    Outcome::Original {
                        dest: part.with_extension("RAF"),
                        result: Ok(()),
                    }
                }
                _ => unreachable!(),
            }),
            || {},
        );
        w.request(Job::Original {
            asset_id: "22222222".into(),
            dest: dest.clone(),
        });
        let mut got = Vec::new();
        settle(&mut w, |w| {
            got.extend(w.poll());
            !got.is_empty()
        });
        assert!(dest.is_file(), "a finished download must be at the real path");
        assert_eq!(std::fs::read(&dest).unwrap(), b"a whole raw");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ⚠ Two opens of the same photo must not race two downloads onto one path.
    #[test]
    fn the_same_asset_requested_twice_is_only_fetched_once() {
        let dir = temp_dir("dedupe");
        let dest = dir.join("33333333.RAF");
        let calls = Arc::new(AtomicUsize::new(0));
        let c = calls.clone();
        let mut w = ImmichWorker::new(
            Arc::new(move |job: Job| {
                c.fetch_add(1, Ordering::SeqCst);
                std::thread::sleep(std::time::Duration::from_millis(50));
                match job {
                    Job::Original { dest: part, .. } => {
                        std::fs::write(&part, b"x").unwrap();
                        Outcome::Original { dest: part, result: Ok(()) }
                    }
                    _ => unreachable!(),
                }
            }),
            || {},
        );
        let job = Job::Original { asset_id: "33333333".into(), dest: dest.clone() };
        w.request(job.clone());
        w.request(job);
        let mut got = Vec::new();
        settle(&mut w, |w| {
            got.extend(w.poll());
            !got.is_empty()
        });
        std::thread::sleep(std::time::Duration::from_millis(100));
        got.extend(w.poll());
        assert_eq!(calls.load(Ordering::SeqCst), 1, "one fetch for two requests");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
```

- [ ] **Step 2: Run it to verify it fails**

```bash
cargo test -p app immich_worker
```

Expected: FAIL — the module is not declared.

- [ ] **Step 3: Write the worker above the test module**

```rust
//! The one background worker for everything the UI needs from an Immich server.
//!
//! ⚠⚠ **One worker, not one per job kind.** `About` backs the Settings Test button, `Albums`
//! and `Album` the rail section, `Original` the open. Thumbnails are the single exception and do
//! **not** come through here: `ThumbLoader` already owns a pool whose queue the UI replaces
//! wholesale, which is exactly right for them and wrong for these.
//!
//! ⚠⚠ **Every server call goes through here, never inline in `show`.** A blocking HTTP call
//! inside a frame is a frozen window. So each is a hop: the click enqueues, the frame ends, the
//! result arrives on a later frame.
//!
//! ⚠ **No egui here.** The fetcher is injected, so every test below runs with no network.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

/// What the UI wants from the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Job {
    /// `GET /server/about` — the Settings Test button.
    About,
    /// `GET /albums`.
    Albums,
    /// `GET /albums/{id}`.
    Album { id: String, name: String },
    /// `GET /assets/{id}/original`, streamed to `dest`.
    ///
    /// ⚠ `dest` is the **real** cache path. The worker writes to `dest.part` and renames; the
    /// fetcher is handed the `.part`.
    Original { asset_id: String, dest: PathBuf },
}

/// What came back.
pub enum Outcome {
    About(Result<String, String>),
    Albums(Result<Vec<immich::AlbumSummary>, String>),
    Album {
        id: String,
        name: String,
        result: Result<Vec<immich::Asset>, String>,
    },
    Original {
        dest: PathBuf,
        result: Result<(), String>,
    },
}

/// The injected fetcher. Takes the job **with `dest` already pointing at the `.part` file**.
pub type FetchFn = Arc<dyn Fn(Job) -> Outcome + Send + Sync>;

/// Where the partial download lives until it is complete.
///
/// ⚠ A suffix rather than a sibling directory, so a crash leaves the evidence next to the thing
/// it belongs to and the cleanup is obvious.
pub fn part_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_os_string();
    s.push(".part");
    PathBuf::from(s)
}

pub struct ImmichWorker {
    fetch: FetchFn,
    on_done: Arc<dyn Fn() + Send + Sync>,
    tx: Sender<Outcome>,
    rx: Receiver<Outcome>,
    /// Which asset ids are being downloaded right now, so two opens do not race one path.
    in_flight: HashSet<String>,
    /// Which non-download jobs are in flight, so an expanded section does not refetch per frame.
    pending_list: bool,
}

impl ImmichWorker {
    pub fn new(fetch: FetchFn, on_done: impl Fn() + Send + Sync + 'static) -> Self {
        let (tx, rx) = channel();
        Self {
            fetch,
            on_done: Arc::new(on_done),
            tx,
            rx,
            in_flight: HashSet::new(),
            pending_list: false,
        }
    }

    /// Is this asset's original being fetched right now?
    pub fn is_downloading(&self, asset_id: &str) -> bool {
        self.in_flight.contains(asset_id)
    }

    /// Is a list request (albums or one album) outstanding?
    pub fn is_listing(&self) -> bool {
        self.pending_list
    }

    /// Enqueue a job. Duplicate downloads of one asset, and a second list while one is in
    /// flight, are dropped rather than queued.
    pub fn request(&mut self, job: Job) {
        match &job {
            Job::Original { asset_id, .. } => {
                if !self.in_flight.insert(asset_id.clone()) {
                    return;
                }
            }
            Job::Albums | Job::Album { .. } => {
                if self.pending_list {
                    return;
                }
                self.pending_list = true;
            }
            Job::About => {}
        }

        let fetch = self.fetch.clone();
        let tx = self.tx.clone();
        let on_done = self.on_done.clone();
        std::thread::spawn(move || {
            let outcome = match &job {
                Job::Original { asset_id, dest } => {
                    let part = part_path(dest);
                    // ⚠ A stale .part from a crashed run must not be appended to or mistaken
                    // for progress.
                    let _ = std::fs::remove_file(&part);
                    if let Some(parent) = dest.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    let out = fetch(Job::Original {
                        asset_id: asset_id.clone(),
                        dest: part.clone(),
                    });
                    let ok = matches!(
                        &out,
                        Outcome::Original { result: Ok(()), .. }
                    );
                    if ok {
                        // ⚠⚠ The rename is the commit. Until it happens the real path does not
                        // exist, so nothing can read a half-written RAW.
                        if let Err(e) = std::fs::rename(&part, dest) {
                            let _ = std::fs::remove_file(&part);
                            Outcome::Original {
                                dest: dest.clone(),
                                result: Err(format!("could not move the download into place: {e}")),
                            }
                        } else {
                            Outcome::Original {
                                dest: dest.clone(),
                                result: Ok(()),
                            }
                        }
                    } else {
                        let _ = std::fs::remove_file(&part);
                        match out {
                            Outcome::Original { result, .. } => Outcome::Original {
                                dest: dest.clone(),
                                result,
                            },
                            other => other,
                        }
                    }
                }
                _ => fetch(job.clone()),
            };
            let _ = tx.send(outcome);
            on_done();
        });
    }

    /// Take whatever has finished. Call once per frame.
    pub fn poll(&mut self) -> Vec<Outcome> {
        let mut out = Vec::new();
        while let Ok(o) = self.rx.try_recv() {
            match &o {
                Outcome::Original { dest, .. } => {
                    // The id is not carried back, so clear by the path's stem, which is the id.
                    if let Some(stem) = dest.file_stem().map(|s| s.to_string_lossy().to_string()) {
                        self.in_flight.remove(&stem);
                    }
                }
                Outcome::Albums(_) | Outcome::Album { .. } => self.pending_list = false,
                Outcome::About(_) => {}
            }
            out.push(o);
        }
        out
    }
}
```

Add to `crates/app/src/lib.rs`:

```rust
pub mod immich_worker;
```

⚠ `Job` derives `Clone` because the dedupe test requests the same job twice; keep that derive.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p app immich_worker
```

Expected: PASS, 4 tests.

- [ ] **Step 5: Commit**

```bash
git add crates/app/src/lib.rs crates/app/src/immich_worker.rs
git commit -m "One Immich worker: albums, the version probe, and downloads that commit by rename

A download writes to <dest>.part and renames on success, so a failure leaves
nothing at the real path. The whole cache design reads 'the path exists' as 'the
bytes are all there', and a truncated RAW at the real path would be decoded as
though it were whole.

Two opens of one photo are one fetch, and a stale .part from a crashed run is
removed rather than appended to."
```

---

### Task 7: The real fetcher, wiring the worker to the client

**Files:**
- Modify: `crates/app/src/immich_worker.rs`

- [ ] **Step 1: Write the failing test**

Append to `immich_worker.rs`'s test module:

```rust
    /// ⚠ Without a configured connection the fetcher must produce an error outcome rather than
    /// panicking or silently doing nothing — the section shows that message.
    #[test]
    fn the_real_fetcher_without_a_connection_reports_it() {
        let f = live_fetcher(None);
        match f(Job::Albums) {
            Outcome::Albums(Err(msg)) => assert!(!msg.is_empty()),
            _ => panic!("an unconfigured fetcher must report an error"),
        }
    }

    /// ⚠ An unreachable host must come back as an error, not a hang or a panic. `.invalid` is
    /// reserved by RFC 2606 and never resolves, so this needs no network.
    #[test]
    fn an_unreachable_server_is_an_error_not_a_panic() {
        let f = live_fetcher(Some(crate::immich_source::Connection {
            url: "http://immich.invalid".into(),
            key: "k".into(),
        }));
        match f(Job::Albums) {
            Outcome::Albums(Err(_)) => {}
            _ => panic!("an unreachable server must be an error"),
        }
    }
```

- [ ] **Step 2: Run it to verify it fails**

```bash
cargo test -p app immich_worker
```

Expected: FAIL — `live_fetcher` does not exist.

- [ ] **Step 3: Write it**

Above the test module in `immich_worker.rs`:

```rust
/// The real fetcher: a `FetchFn` that talks to the server, or reports that none is configured.
///
/// ⚠ Built from a snapshot of the connection rather than reading `Settings` on the worker
/// thread. `Settings` lives on the UI side and is not `Sync` to share casually; a snapshot also
/// means a job already in flight finishes against the server it started with.
pub fn live_fetcher(conn: Option<crate::immich_source::Connection>) -> FetchFn {
    Arc::new(move |job: Job| {
        let client = match conn
            .as_ref()
            .ok_or_else(|| "no Immich server is configured — set one in Settings".to_string())
            .and_then(|c| immich::Client::new(&c.url, &c.key).map_err(|e| e.to_string()))
        {
            Ok(c) => c,
            Err(e) => {
                return match job {
                    Job::About => Outcome::About(Err(e)),
                    Job::Albums => Outcome::Albums(Err(e)),
                    Job::Album { id, name } => Outcome::Album { id, name, result: Err(e) },
                    Job::Original { dest, .. } => Outcome::Original { dest, result: Err(e) },
                }
            }
        };
        match job {
            Job::About => Outcome::About(
                client
                    .about()
                    .map(|a| a.version)
                    .map_err(|e| e.to_string()),
            ),
            Job::Albums => Outcome::Albums(client.albums().map_err(|e| e.to_string())),
            Job::Album { id, name } => {
                let result = client.album(&id).map_err(|e| e.to_string());
                Outcome::Album { id, name, result }
            }
            Job::Original { asset_id, dest } => Outcome::Original {
                dest: dest.clone(),
                result: client.original(&asset_id, &dest).map_err(|e| e.to_string()),
            },
        }
    })
}
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p app immich_worker
```

Expected: PASS, 6 tests. The unreachable-host test takes a second or two while DNS fails.

- [ ] **Step 5: Commit**

```bash
git add crates/app/src/immich_worker.rs
git commit -m "The live fetcher, built from a snapshot of the connection

An unconfigured server and an unreachable one both come back as error outcomes
the section can show, rather than as a panic or a silent no-op."
```

---

### Task 8: Remote thumbnails

**Files:**
- Modify: `crates/app/src/immich_source.rs`, `crates/app/src/thumb_loader.rs`

- [ ] **Step 1: Write the failing test**

Append to `immich_source.rs`'s test module:

```rust
    /// A 1×1 red PNG. ⚠ Deliberately **not** a webp: the point is that the decode sniffs the
    /// format, because the server's thumbnail and preview formats are both admin settings.
    const TINY_PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
        0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08, 0xD7, 0x63, 0xF8,
        0xCF, 0xC0, 0x00, 0x00, 0x03, 0x01, 0x01, 0x00, 0x18, 0xDD, 0x8D, 0xB0, 0x00, 0x00, 0x00,
        0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    #[test]
    fn encoded_bytes_become_a_thumbnail_whatever_format_they_are_in() {
        let t = decode_thumbnail(TINY_PNG).expect("a PNG must decode");
        assert_eq!((t.width, t.height), (1, 1));
        assert_eq!(t.rgba.len(), 4);
    }

    #[test]
    fn rubbish_bytes_are_none_rather_than_a_panic() {
        assert!(decode_thumbnail(b"not an image at all").is_none());
    }

    #[test]
    fn a_path_inside_the_cache_root_is_recognised_as_an_immich_photo() {
        let root = std::path::Path::new("/c/rustroom/immich/host");
        assert!(is_immich_path(root, &root.join("11111111.RAF")));
        assert!(!is_immich_path(root, std::path::Path::new("/home/w/Pictures/a.RAF")));
    }
```

- [ ] **Step 2: Run it to verify it fails**

```bash
cargo test -p app immich_source
```

Expected: FAIL — `decode_thumbnail` and `is_immich_path` do not exist.

- [ ] **Step 3: Write them**

Above the test module in `immich_source.rs`:

```rust
/// Decode thumbnail bytes the server sent.
///
/// ⚠⚠ Sniffs the format with `load_from_memory` rather than assuming webp. Immich's thumbnail
/// and preview formats are **both admin settings**, so either could arrive — and the workspace
/// only gained the `webp` feature for this.
pub fn decode_thumbnail(bytes: &[u8]) -> Option<core_image::Thumbnail> {
    let img = image::load_from_memory(bytes).ok()?;
    let rgba = img.to_rgba8();
    Some(core_image::Thumbnail {
        width: rgba.width() as usize,
        height: rgba.height() as usize,
        rgba: rgba.into_raw(),
    })
}

/// Is this path one of the Immich cache's?
///
/// ⚠ The test the thumbnail loader branches on. A plain prefix check, not a canonicalisation —
/// the file may not exist yet, which is the whole point.
pub fn is_immich_path(root: &Path, path: &Path) -> bool {
    path.starts_with(root)
}

/// The asset id a cache path was built from — its stem.
///
/// ⚠ The inverse of `cache_path`, and the reason the UUID leads rather than trails.
pub fn asset_id_of(path: &Path) -> Option<String> {
    path.file_stem().map(|s| s.to_string_lossy().to_string())
}
```

No new `app` dependency is needed: `core-image` (line 22), `image` (line 46), `serde`, `serde_json`, `thiserror` and `dirs` are all already in `crates/app/Cargo.toml`. Only the `immich = { path = "../immich" }` line from Task 4 was new.

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cargo test -p app immich_source
```

Expected: PASS, 15 tests.

- [ ] **Step 5: Add the branch to `ThumbLoader::new`**

In `crates/app/src/thumb_loader.rs`, in `ThumbLoader::new`, beside where `store` and `looks` are captured, add:

```rust
        // ⚠⚠ Captured here for the same reason the store and the looks are, and with the same
        // consequence: `ThumbLoader::new`'s signature and its four call sites stay untouched.
        //
        // ⚠ An `Arc<Mutex<..>>` the app owns, **not** a snapshot: pasting a key in Settings must
        // not rebuild the loader, because rebuilding drops the open folder's whole thumbnail
        // cache (~41 ms × N, about 34 s on an 830-photo folder).
        let immich = crate::immich_state();
```

and change the closure passed to `with_loader` to try the remote branch first:

```rust
            Arc::new(move |r: &ThumbRequest| {
                // ⚠⚠ **Only when the file is not there yet.** Once a photo has been downloaded
                // this falls through and the ordinary local path — on-disk store included —
                // takes over with no special case at all.
                if let Some(t) = crate::immich_thumb(&immich, r) {
                    return Some(t);
                }
                load_thumbnail(
                    r,
                    pipeline::Resampler::default(),
                    Some(&store),
                    Some((&looks, &look_cache)),
                )
            }),
```

Then add to `immich_source.rs`:

```rust
/// The connection the thumbnail loader reads, shared with the app so a Settings change is seen
/// without rebuilding the loader.
pub type SharedConnection = std::sync::Arc<std::sync::Mutex<Option<Connection>>>;

/// Fetch and decode one Immich thumbnail, or `None` if this is not an Immich photo, the file is
/// already downloaded, or no server is configured.
///
/// ⚠ The on-disk `ThumbStore` is deliberately **not** used here: its key is `(path, mtime,
/// size)` from `fs::metadata`, which cannot be built for a file that does not exist. The
/// in-memory `ThumbCache` still applies, keyed on `PhotoId` as it already is, so the cost is
/// that album thumbnails re-fetch after a restart. Accepted for this slice; the fix is a key of
/// `(asset id, updated_at)`.
pub fn remote_thumbnail(shared: &SharedConnection, path: &Path) -> Option<core_image::Thumbnail> {
    if is_cached(path) {
        return None;
    }
    let conn = shared.lock().ok()?.clone()?;
    if !is_immich_path(&cache_root(&conn.url), path) {
        return None;
    }
    let id = asset_id_of(path)?;
    let client = immich::Client::new(&conn.url, &conn.key).ok()?;
    let bytes = client.thumbnail(&id, immich::Size::Thumbnail).ok()?;
    decode_thumbnail(&bytes)
}
```

In `crates/app/src/main.rs`, add the two helpers the closure calls — a `OnceLock` holding the shared connection, so `ThumbLoader::new` can reach it without a signature change:

```rust
/// The Immich connection the thumbnail loader reads.
///
/// ⚠ A process-wide `OnceLock` rather than a field on `RustroomApp`, because
/// `ThumbLoader::new` is constructed from `open_scan` where `self` is already mutably borrowed,
/// and widening its signature would touch its four call sites for no gain. Updated by
/// `apply_settings`.
fn immich_state() -> rustroom_app::immich_source::SharedConnection {
    static STATE: std::sync::OnceLock<rustroom_app::immich_source::SharedConnection> =
        std::sync::OnceLock::new();
    STATE
        .get_or_init(|| std::sync::Arc::new(std::sync::Mutex::new(None)))
        .clone()
}

fn immich_thumb(
    shared: &rustroom_app::immich_source::SharedConnection,
    r: &rustroom_app::thumb_loader::ThumbRequest,
) -> Option<core_image::Thumbnail> {
    rustroom_app::immich_source::remote_thumbnail(shared, r.id.file())
}
```

⚠ `thumb_loader.rs` is in the **lib**, so `crate::immich_state()` there resolves inside `rustroom_app`, not `main`. Put both helpers in `crates/app/src/immich_source.rs` instead and call them as `crate::immich_source::shared()` and `crate::immich_source::remote_thumbnail(&immich, r.id.file())` — adjust the two lines in `ThumbLoader::new` accordingly. Verify which crate the file belongs to before writing the call.

- [ ] **Step 6: Set the shared connection when settings are applied**

In `crates/app/src/settings_apply.rs` (or wherever `apply_settings` lives — find it with `grep -rn "fn apply_" crates/app/src/settings_apply.rs`), after settings are written, add:

```rust
        // ⚠ Pushed into the shared cell rather than rebuilding the ThumbLoader, which would drop
        // the open folder's thumbnails.
        if let Ok(mut slot) = rustroom_app::immich_source::shared().lock() {
            *slot = rustroom_app::immich_source::connection(&self.settings);
        }
```

- [ ] **Step 7: Build and run the app's tests**

```bash
cargo test -p app immich
```

Expected: PASS. If the crate-path adjustment in Step 5 was not made, this is where it fails to compile.

- [ ] **Step 8: Commit**

```bash
git add crates/app/src
git commit -m "Immich thumbnails through ThumbLoader's existing pool

One branch in the closure new() already builds, taken only while the original is
absent: once a photo is downloaded it falls through to the ordinary local path,
on-disk store included. The bytes are sniffed rather than assumed to be webp,
because the server's thumbnail format is an admin setting.

The connection lives in a shared cell so pasting a key does not rebuild the
loader and drop the open folder's thumbnails."
```

---

### Task 9: Opening an album

**Files:**
- Modify: `crates/app/src/main.rs`, `crates/app/src/sources.rs`

- [ ] **Step 1: Add the `LibrarySource` variant**

In `crates/app/src/main.rs`, in `enum LibrarySource`:

```rust
    /// One Immich album, its photos cached locally on demand.
    ///
    /// ⚠ The name is carried, not looked up: the rail may not have fetched the album list when
    /// the title bar wants to say where these photos came from.
    ImmichAlbum { id: String, name: String },
```

- [ ] **Step 2: Add the loader to `sources.rs`**

Beside the existing loaders:

```rust
    /// Show one Immich album's photos, downloading nothing.
    ///
    /// ⚠⚠ Goes through `open_scan`, the one place a `Library` is constructed, exactly like a
    /// folder — which is the entire point of shape A. The rows name cache paths that mostly do
    /// not exist yet; `open_scan`'s own callees must tolerate that, which is what the
    /// missing-file tests in Task 11 pin.
    pub(crate) fn open_immich_album(
        &mut self,
        ctx: &egui::Context,
        id: String,
        name: String,
        assets: &[immich::Asset],
    ) -> usize {
        let Some(conn) = rustroom_app::immich_source::connection(&self.settings) else {
            self.status = "No Immich server is configured — set one in Settings".to_string();
            return 0;
        };
        let root = rustroom_app::immich_source::cache_root(&conn.url);
        let scan = rustroom_app::immich_source::entries_from_album(assets, &root);
        let n = scan.len();
        self.open_scan(ctx, LibrarySource::ImmichAlbum { id, name: name.clone() }, scan);
        self.status = format!("{n} photos in “{name}”");
        n
    }
```

- [ ] **Step 3: Handle the worker's outcomes once per frame**

Find where the app polls its other workers each frame (`grep -rn "\.poll()" crates/app/src/main.rs crates/app/src/*.rs | head`) and add beside them:

```rust
        for outcome in self.immich.poll() {
            match outcome {
                rustroom_app::immich_worker::Outcome::Albums(Ok(list)) => {
                    self.immich_albums = Some(Ok(list));
                }
                rustroom_app::immich_worker::Outcome::Albums(Err(e)) => {
                    self.immich_albums = Some(Err(e));
                }
                rustroom_app::immich_worker::Outcome::Album { id, name, result } => match result {
                    Ok(assets) => {
                        self.open_immich_album(ctx, id, name, &assets);
                    }
                    Err(e) => self.status = format!("Could not open “{name}”: {e}"),
                },
                rustroom_app::immich_worker::Outcome::About(Ok(v)) => {
                    self.status = format!("Connected to Immich {v}");
                }
                rustroom_app::immich_worker::Outcome::About(Err(e)) => {
                    self.status = format!("Immich: {e}");
                }
                rustroom_app::immich_worker::Outcome::Original { dest, result } => match result {
                    // ⚠ The file is now where the PhotoId always said it was, so the ordinary
                    // open path applies with no special case.
                    Ok(()) => self.open_photo_at(ctx, &dest),
                    Err(e) => self.status = format!("Download failed: {e}"),
                },
            }
        }
```

⚠ `open_photo_at` is a placeholder for whatever this app already calls to open a photo by path — find the real one with `grep -rn "fn open_photo\|fn show_photo\|fn enter_develop" crates/app/src/session_flow.rs` and use it. Do **not** add a new opening path.

Add the two fields to `RustroomApp` in `main.rs`:

```rust
    /// The Immich worker. ⚠ Constructed with the live fetcher from the current settings.
    immich: rustroom_app::immich_worker::ImmichWorker,
    /// The album list, once fetched: `None` = never asked, `Some(Err)` = the failure to show.
    immich_albums: Option<Result<Vec<immich::AlbumSummary>, String>>,
```

and initialise them where the struct is built:

```rust
            immich: rustroom_app::immich_worker::ImmichWorker::new(
                rustroom_app::immich_worker::live_fetcher(
                    rustroom_app::immich_source::connection(&settings),
                ),
                {
                    let ctx = cc.egui_ctx.clone();
                    move || ctx.request_repaint()
                },
            ),
            immich_albums: None,
```

- [ ] **Step 4: Build**

```bash
cargo build -p app
```

Expected: compiles. Fix any name mismatches against the real `open_photo_at` and the real settings-construction site.

- [ ] **Step 5: Commit**

```bash
git add crates/app/src
git commit -m "An Immich album opens through open_scan, like a folder

The rows name cache paths that mostly do not exist yet, which is the point of
shape A: the grid, the selection, the filmstrip and the filter are the ordinary
ones. A finished download hands the path to the app's existing open path."
```

---

### Task 10: The rail section

**Files:**
- Create: `crates/app/src/immich_panel.rs`
- Modify: `crates/app/src/collections_panel.rs`, `crates/app/src/main.rs`

- [ ] **Step 1: Write the section**

Create `crates/app/src/immich_panel.rs`:

```rust
//! The Immich section of the left rail — a sixth header, at the same level as Folders and
//! Collections.
//!
//! ⚠ **A bin module**: egui lives here, never in `immich_source` or `immich_worker`.
//!
//! ⚠⚠ Nothing here calls the server. A click enqueues a job on `RustroomApp::immich` and the
//! result arrives on a later frame — a blocking HTTP call inside a frame is a frozen window.

use crate::list_row::{self, RowSpec};
use crate::RustroomApp;
use eframe::egui;

/// What a click in this section asked for. Applied after the list closes, the shape every panel
/// in this app uses: egui borrows `app` through the closure, so a click cannot act immediately.
pub enum Action {
    None,
    Refresh,
    Open { id: String, name: String },
}

/// Draw the section's body. Returns what was clicked.
pub fn body(app: &RustroomApp, ui: &mut egui::Ui) -> Action {
    let mut action = Action::None;

    if rustroom_app::immich_source::connection(&app.settings).is_none() {
        ui.label(
            egui::RichText::new("No server configured — add one in Settings")
                .weak()
                .small(),
        );
        return action;
    }

    match &app.immich_albums {
        None => {
            if app.immich.is_listing() {
                ui.label(egui::RichText::new("Loading albums…").weak().small());
            } else {
                action = Action::Refresh;
            }
        }
        Some(Err(e)) => {
            // ⚠ The message distinguishes a rejected key from an unreachable server, which is
            // the whole reason immich::Error has variants.
            ui.label(egui::RichText::new(e).weak().small());
            if ui.small_button("Retry").clicked() {
                action = Action::Refresh;
            }
        }
        Some(Ok(albums)) => {
            if albums.is_empty() {
                ui.label(egui::RichText::new("No albums on the server").weak().small());
            }
            for a in albums {
                let selected = matches!(
                    app.library.as_ref().map(|l| &l.source),
                    Some(crate::LibrarySource::ImmichAlbum { id, .. }) if id == &a.id
                );
                let r = list_row::row(
                    ui,
                    RowSpec {
                        name: &a.album_name,
                        count: Some(a.asset_count as usize),
                        meta: "",
                        selected,
                        indent: 0.0,
                        hover: "Photos are downloaded when you open them",
                        actions: &[],
                    },
                );
                if r.clicked {
                    action = Action::Open {
                        id: a.id.clone(),
                        name: a.album_name.clone(),
                    };
                }
            }
        }
    }

    action
}
```

⚠ `RowSpec`'s exact fields and `row`'s return type must be checked against `crates/app/src/list_row.rs` — the folder section at `collections_panel.rs:130` is the worked example. `count` may be `Option<usize>` or another integer width; match it.

- [ ] **Step 2: Add the section to the rail**

In `crates/app/src/collections_panel.rs`, beside the five existing `section(...)` calls (after the Folders one reads naturally), add:

```rust
                                let n = app
                                    .immich_albums
                                    .as_ref()
                                    .and_then(|r| r.as_ref().ok())
                                    .map(|v| v.len().to_string())
                                    .unwrap_or_default();
                                section(ui, &mut sections.immich, "sec_immich", "Immich", &n, |ui| {
                                    immich_action = crate::immich_panel::body(app, ui);
                                });
```

Declare `let mut immich_action = crate::immich_panel::Action::None;` beside the existing `action` variable, and apply it after the list closes, beside where the other actions are applied:

```rust
    match immich_action {
        crate::immich_panel::Action::None => {}
        crate::immich_panel::Action::Refresh => {
            app.immich.request(rustroom_app::immich_worker::Job::Albums);
        }
        crate::immich_panel::Action::Open { id, name } => {
            app.immich
                .request(rustroom_app::immich_worker::Job::Album { id, name });
        }
    }
```

Add `immich: bool` to whatever struct holds the sections' open flags (find it with `grep -rn "sections.folders" crates/app/src | head -3`, then the struct's definition), following the existing fields' shape.

Add `mod immich_panel;` to `crates/app/src/main.rs` beside the other bin modules.

- [ ] **Step 3: Build and run the app's tests**

```bash
cargo build -p app && cargo test -p app immich
```

Expected: compiles, tests pass.

- [ ] **Step 4: Commit**

```bash
git add crates/app/src
git commit -m "The Immich section in the rail, beside Folders and Collections

Three states: unconfigured says so quietly, a failure shows which failure it was,
and a loaded list opens an album on click. Nothing in the section calls the
server — a click enqueues and the result lands on a later frame."
```

---

### Task 11: Download on open, and the missing-file audit

**Files:**
- Modify: `crates/app/src/session_flow.rs`, plus whichever of the audit sites need a guard
- Modify: `crates/app/src/immich_source.rs` (tests)

- [ ] **Step 1: Write the failing test**

Append to `immich_source.rs`'s test module:

```rust
    /// ⚠⚠ **The new invariant, pinned.** A `CatalogEntry` may name a file that does not exist,
    /// and `open_scan` calls `sidecar::load_all`, `import_xmp`, `queue_metadata`,
    /// `reload_marks` and `reload_captures` on every row. None of them may fail on an absent
    /// file.
    #[test]
    fn a_sidecar_for_a_file_that_does_not_exist_is_default_edits_not_an_error() {
        let p = std::path::Path::new("/nonexistent/rustroom/11111111-1111-4111-8111-111111111111.RAF");
        let s = crate::sidecar::load_all(p);
        assert!(s.is_neutral(), "no file and no sidecar must read as neutral");
    }

    #[test]
    fn edited_at_for_a_file_that_does_not_exist_is_none() {
        let p = std::path::Path::new("/nonexistent/rustroom/22222222-2222-4222-8222-222222222222.RAF");
        assert!(crate::sidecar::edited_at(p).is_none());
    }
```

- [ ] **Step 2: Run it to verify it fails or passes**

```bash
cargo test -p app immich_source
```

Expected: **either outcome is informative.** If it passes, `sidecar` already tolerates absent files — record that and move on. If it fails, fix `sidecar::load_all`/`edited_at` to return the neutral value rather than erroring, and re-run.

- [ ] **Step 3: Audit the other three sites by reading them**

```bash
grep -n "fn import_xmp" -A30 crates/app/src/*.rs
grep -n "fn queue_metadata" -A20 crates/app/src/*.rs
grep -n "fn reload_marks" -A20 crates/app/src/*.rs
```

For each, confirm it skips a row whose file is absent. Where it does not, add the guard — `if !path.is_file() { continue; }` — with a comment naming why:

```rust
            // ⚠⚠ An Immich row names a cache path that may not exist yet. Absent is normal here,
            // not an error to report per row.
            if !e.path.is_file() {
                continue;
            }
```

- [ ] **Step 4: Make opening an uncached photo download it**

In `crates/app/src/session_flow.rs`, at the top of the function that opens a photo (the one Task 9 wired `Outcome::Original` to), add:

```rust
        // ⚠⚠ An Immich photo may not be on disk yet. Enqueue the download and return; the
        // worker's Ok outcome re-enters this function with the file in place, so there is one
        // opening path rather than two.
        if !path.is_file() {
            if let Some(id) = rustroom_app::immich_source::asset_id_of(path) {
                if rustroom_app::immich_source::connection(&self.settings).is_some() {
                    self.immich.request(rustroom_app::immich_worker::Job::Original {
                        asset_id: id,
                        dest: path.to_path_buf(),
                    });
                    self.status = "Downloading from Immich…".to_string();
                    return;
                }
            }
            self.status = format!("{} is not there", path.display());
            return;
        }
```

⚠ Check the real function's signature and return type before pasting — it may return a value rather than `()`, in which case return the same "did nothing" value the not-found branch already uses.

- [ ] **Step 5: Run the app's tests**

```bash
cargo test -p app
```

Expected: PASS. A failure here is most likely a test that constructs `Settings` literally and now needs the two new fields.

- [ ] **Step 6: Commit**

```bash
git add crates/app/src
git commit -m "Opening an Immich photo downloads it first, and the absent-file audit

A CatalogEntry may name a file that is not there yet, which is new. The four
places open_scan reaches — the sidecar read, the XMP import, the metadata scan
and the marks reload — are each pinned against an absent file, and opening
enqueues the download and re-enters the one existing open path when it lands."
```

---

### Task 12: The Settings UI, then the full gate

**Files:**
- Modify: `crates/app/src/modals/settings.rs`

- [ ] **Step 1: Add the draft fields**

In `SettingsDraft`:

```rust
    immich_url: String,
    immich_key: String,
```

In `from_settings`:

```rust
            immich_url: s.immich_url.clone(),
            immich_key: s.immich_key.clone(),
```

In `apply` (beside the other `apply_*` writes):

```rust
        rustroom_app::immich_source::set_connection(
            &mut app.settings,
            &self.immich_url,
            &self.immich_key,
        );
```

- [ ] **Step 2: Draw the group**

On the tab where a connection belongs, following the surrounding rows' layout:

```rust
            ui.add_space(GAP);
            ui.label("Immich");
            ui.horizontal(|ui| {
                ui.label("Server").on_hover_text("e.g. https://photos.example.com");
                ui.text_edit_singleline(&mut draft.immich_url);
            });
            ui.horizontal(|ui| {
                ui.label("API key").on_hover_text(
                    "Paste a read-only key (asset.read, asset.view, asset.download, album.read). \
                     It is stored in plain text in settings.json, so a key that cannot write \
                     cannot damage your library.",
                );
                ui.add(egui::TextEdit::singleline(&mut draft.immich_key).password(true));
                if ui.button("Test").clicked() {
                    // ⚠ Tests what is *typed*, not what is saved — the point is to check a key
                    // before applying it.
                    app.immich.request(rustroom_app::immich_worker::Job::About);
                }
            });
```

⚠ The Test button must build its client from the **draft**, not from `app.settings`, or it tests the old key. If `Job::About` cannot carry a connection, add one to the variant: `About { conn: Option<Connection> }`, and have `live_fetcher` prefer it. Make that change in `immich_worker.rs` with a test, in the shape of Task 7's.

- [ ] **Step 3: Build and run the app's tests**

```bash
cargo build -p app && cargo test -p app
```

Expected: PASS.

- [ ] **Step 4: Run the full gate, each exit code from its own command**

⚠ In the background with logs — a fresh test binary stalls ~22 s on this Mac, so this is 15–30 minutes. ⚠ **Never two `cargo test` runs at once.**

```bash
cargo fmt --all -- --check
```

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

```bash
cargo test --workspace
```

Expected: all three clean, and **all 74 goldens unmoved** — this slice adds no pipeline stage, so a moved golden means something is wrong.

- [ ] **Step 5: Commit**

```bash
git add crates/app/src
git commit -m "The Immich connection in Settings, on the draft like everything else

The Test button checks the key that is typed rather than the one that is saved,
which is the point of testing before applying. The help text says to paste a
read-only key and why: it is stored in plain text, and a key that cannot write
cannot damage the library."
```

---

### Task 13: Run it

- [ ] **Step 1: Launch the GUI**

```bash
cargo run --release -p app
```

- [ ] **Step 2: Walk the path**

1. Settings → paste the server URL and a read-only API key → **Test** → expect "Connected to Immich <version>".
2. Apply. The rail's **Immich** section lists albums with counts.
3. Click an album. The grid fills with thumbnails from the server; nothing has been downloaded.
4. Open a photo. It downloads, then lands in Develop with the full pipeline.
5. Make an edit, switch photos, come back — the edit persisted to a sidecar beside the cached file.
6. Export it.

**Expected to be wrong, and fine:** ratings, flags and colour labels do not stick on Immich photos — there are no catalog rows, deferred deliberately. Album thumbnails re-fetch after a restart — the on-disk store is skipped for absent files.

- [ ] **Step 3: Report what use found**

⚠⚠ Almost every GUI-wiring defect in this repo was found by use, not by a test. A milestone is not closed when it is green; it is closed when it has been seen. Write what happened into `docs/JOURNAL.md` as a new dated section at the top, and update `CLAUDE.md`'s live-state block **in place** — do not append a superseded block.

---

## Self-review against the spec

| Spec section | Task |
|---|---|
| `crates/immich` client, DTOs, errors, fixtures, no network in tests | 1, 2, 3 |
| The `image` webp feature | 1 |
| `immich_source`: cache paths, `host_slug`, `entries_from_album`, `is_cached` | 4 |
| Settings fields and the accessor | 5, 12 |
| The one worker and its `Job` enum | 6, 7 |
| Temp-then-rename download discipline | 6 |
| Thumbnails through `ThumbLoader`'s pool; store skipped | 8 |
| The rail section's three states | 10 |
| `LibrarySource::ImmichAlbum` and `open_scan` reuse | 9 |
| Open → download → the existing open path | 9, 11 |
| The absent-file invariant audit | 11 |
| Certificate verification never disabled | 3 |
| Never `?edited=true` | 3 |
| No golden movement | 12 |
| Seen in use | 13 |

**Known soft spots, called out rather than hidden:**

- **`ureq` 3's exact API** (Task 3) — the shape is right, the method names must be checked against the resolved version. Flagged in the task itself.
- **`list_row::RowSpec`'s exact fields** (Task 10) and **the real open-photo function's name** (Tasks 9, 11) — both need reading before pasting; the worked examples are named.
- **Which Settings tab the Immich group belongs on** (Task 12) is left to whoever sees the dialog. The four tabs exist; pick the one that reads right rather than adding a fifth.
