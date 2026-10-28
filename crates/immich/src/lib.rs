//! A narrow client for an Immich server.
//!
//! ⚠ **Zero GUI, and no knowledge of Rustroom's types.** It speaks Immich's JSON and returns
//! plain data; mapping that onto `CatalogEntry`/`PhotoId` is `app`'s job, in `immich_source`.

pub mod dto;
pub mod error;
pub mod multipart;

pub use dto::{
    About, AlbumSummary, Asset, AssetPerson, AssetType, Bucket, ExifInfo, FaceBox, NextPage,
    PeoplePage, Person, SearchAssets, SearchResponse, Stack, StackMember, Upload, UploadStatus,
};
pub use error::Error;

use std::io::Write;
use std::path::Path;

/// How much JSON one response may be. See `get_text`.
///
/// ⚠⚠ ureq caps a body read at 10 MB by default (`MAX_BODY_SIZE`), and an album's JSON passes
/// that at roughly 25,000 assets. Raised explicitly so a large album fails as "too large" only
/// at an absurd size, not silently at a plausible one.
const JSON_LIMIT: u64 = 256 * 1024 * 1024;

/// Assets per search page. ⚠ 1000 is the maximum the server accepts (`MetadataSearchDto.size`),
/// and the default is 250 — so asking for the maximum is four times fewer round trips.
const PAGE_SIZE: u32 = 1000;

/// How many bytes an in-memory image read may be. See `get_bytes`.
const IMAGE_LIMIT: u64 = 64 * 1024 * 1024;

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

/// What a search is scoped to.
///
/// ⚠⚠ One type for both, so an album and a month share the paged loop, its `nextPage`-before-
/// `nextCursor` rule and its round-trip ceiling. Those were learned from a real server and must not
/// be reimplemented per caller.
pub enum Filter {
    /// ⚠ `albumIds` is deprecated in 3.2.0 and is still the only way to scope a search to an
    /// album: `isNotInAlbum` is deprecated too, and the timeline endpoints omit `originalFileName`,
    /// which decides the cache path's extension and so the decoder. A deliberate dependency.
    Album(String),
    /// RFC 3339 bounds: `after` inclusive, `before` exclusive.
    TakenBetween { after: String, before: String },
    /// Every asset a person is in (M10.18c).
    Person(String),
}

impl Filter {
    /// The request body for one page.
    ///
    /// ⚠ Crate-visible so the two shapes can be asserted without a server.
    pub(crate) fn body(&self, size: u32) -> serde_json::Value {
        match self {
            // ⚠ `withExif` (M10.15c): Immich's rating lives in `exifInfo`, which a search returns
            // only when asked — 55% more bytes, ~180 ms per 1,000 assets, measured 2026-10-13.
            // ⚠ `withPeople` (M10.18c): an asset's people and face boxes, for the People list.
            Filter::Album(id) => serde_json::json!({
                "albumIds": [id], "size": size, "withExif": true, "withPeople": true,
            }),
            Filter::TakenBetween { after, before } => serde_json::json!({
                "takenAfter": after,
                "takenBefore": before,
                "size": size,
                "withExif": true,
                "withPeople": true,
            }),
            Filter::Person(id) => serde_json::json!({
                "personIds": [id], "size": size, "withExif": true, "withPeople": true,
            }),
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

    /// POST `body` as JSON to `path` and deserialise the JSON reply.
    fn post_json<T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &serde_json::Value,
    ) -> Result<T, Error> {
        let mut res = self
            .agent
            .post(self.url(path))
            .header("x-api-key", &self.key)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            // ⚠ Serialised here and sent as a plain body rather than with `send_json`, which lives
            // behind ureq's non-default `json` feature. This crate already depends on `serde_json`
            // directly, so enabling a second copy of it through ureq buys nothing.
            .send(serde_json::to_string(body).map_err(|e| Error::Decode(e.to_string()))?)
            .map_err(map_ureq)?;
        let text = res
            .body_mut()
            .with_config()
            .limit(JSON_LIMIT)
            .read_to_string()
            .map_err(|e| Error::Decode(e.to_string()))?;
        serde_json::from_str(&text).map_err(|e| Error::Decode(e.to_string()))
    }

    fn get_text(&self, path: &str) -> Result<String, Error> {
        let mut res = self
            .agent
            .get(self.url(path))
            .header("x-api-key", &self.key)
            .header("Accept", "application/json")
            .call()
            .map_err(map_ureq)?;
        res.body_mut()
            .with_config()
            .limit(JSON_LIMIT)
            .read_to_string()
            .map_err(|e| Error::Decode(e.to_string()))
    }

    /// ⚠ A limit, not `u64::MAX`: this reads into memory, and the guard against a misconfigured
    /// server streaming something enormous is worth keeping. Thumbnails and previews are orders
    /// of magnitude below it.
    fn get_bytes(&self, path: &str) -> Result<Vec<u8>, Error> {
        let mut res = self
            .agent
            .get(self.url(path))
            .header("x-api-key", &self.key)
            .call()
            .map_err(map_ureq)?;
        res.body_mut()
            .with_config()
            .limit(IMAGE_LIMIT)
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

    /// Every asset in an album, following the cursor to the last page.
    ///
    /// ⚠⚠ **`POST /search/metadata`, not `GET /albums/{id}`.** `AlbumResponseDto` carries
    /// `assetCount` and **no assets**, so the obvious endpoint cannot answer this at all — it
    /// fails with "missing field `assets`", which is how this was found in use.
    ///
    /// ⚠ `albumIds` is marked **deprecated** in 3.2.0 and is nevertheless the only way to scope a
    /// metadata search to an album: `isNotInAlbum` is deprecated too, and the timeline endpoints
    /// return a columnar shape with **no `originalFileName`**, which is what decides the cache
    /// path's extension and therefore the decoder. A deliberate dependency on a deprecated field,
    /// recorded here so the next person does not think it was an oversight.
    ///
    /// ⚠ Paged by `cursor`, which is not deprecated, rather than by `page`, which is.
    pub fn album(&self, id: &str) -> Result<Vec<Asset>, Error> {
        self.search(&Filter::Album(id.to_string()))
    }

    /// `GET /stacks` — every stack the key can see, in one call.
    ///
    /// ⚠⚠ **6.6 MB for 2,379 stacks, measured twice at ~2.9 s**, because the reply embeds the full
    /// asset DTO for every member. There is no lean mode: the endpoint's only parameter is
    /// `primaryAssetId`. So callers keep a **projection** rather than this, and fetch it off the UI
    /// thread — see `immich_stack_cache`.
    pub fn stacks(&self) -> Result<Vec<Stack>, Error> {
        self.get_json("/stacks")
    }

    /// `GET /people` — every named, visible person on the server (M10.18c), paged until the server says there are
    /// no more. ⚠ Needs `person.read`; a key without it answers 403, which the caller reads as "no people".
    pub fn people(&self) -> Result<Vec<Person>, Error> {
        let mut out = Vec::new();
        for page in 1..=200 {
            let p: PeoplePage =
                self.get_json(&format!("/people?withHidden=false&page={page}&size=500"))?;
            out.extend(
                p.people
                    .into_iter()
                    .filter(|p| !p.is_hidden && !p.name.trim().is_empty()),
            );
            if !p.has_next_page {
                break;
            }
        }
        Ok(out)
    }

    /// The assets `person` is in — the search, paged.
    pub fn person(&self, id: &str) -> Result<Vec<Asset>, Error> {
        self.search(&Filter::Person(id.to_string()))
    }

    /// `GET /timeline/buckets` — the whole library's month list with counts, in one call.
    ///
    /// ⚠ Monthly on a real 3.2.0 server, and the endpoint takes no size parameter, so the
    /// granularity is the server's. `buckets_real.json` is the captured evidence.
    pub fn buckets(&self) -> Result<Vec<Bucket>, Error> {
        self.get_json("/timeline/buckets")
    }

    /// `PUT /assets` — set the rating and/or favourite of several assets at once (M10.15d, Send marks).
    ///
    /// ⚠⚠ **The one call in this crate that changes an existing asset.** Permission `asset.update`;
    /// the reply is 204 with no body, so nothing is read from it. From the published 3.2.0 schema — the
    /// first real reply is recorded in M10.15d's journal entry.
    pub fn update_marks(
        &self,
        ids: &[String],
        rating: Option<Option<i32>>,
        favourite: Option<bool>,
    ) -> Result<(), Error> {
        let body = marks_body(ids, rating, favourite);
        self.agent
            .put(self.url("/assets"))
            .header("x-api-key", &self.key)
            .header("Content-Type", "application/json")
            .send(serde_json::to_string(&body).map_err(|e| Error::Decode(e.to_string()))?)
            .map_err(map_ureq)?;
        Ok(())
    }

    /// Every asset matching `filter`, following the server's paging to the last page.
    ///
    /// ⚠⚠ **`POST /search/metadata`, not `GET /albums/{id}`.** `AlbumResponseDto` carries
    /// `assetCount` and **no assets**, so the obvious endpoint cannot answer this at all — it fails
    /// with "missing field `assets`", which is how that was found in use.
    ///
    /// ⚠⚠ Paging follows **whichever** signal the reply carries. A real 3.2.0 server answers with
    /// `nextCursor: null` and `nextPage: "2"`, so a cursor-only loop returns a **silently truncated**
    /// result; cursors are honoured first in case a later server switches to them.
    pub fn search(&self, filter: &Filter) -> Result<Vec<Asset>, Error> {
        let mut out: Vec<Asset> = Vec::new();
        let mut next: Option<NextPage> = None;
        // ⚠ A hard ceiling on round trips. A server that paged for ever would otherwise hang the
        // worker thread silently; 1000 pages at the maximum page size is a million assets.
        for _ in 0..1000 {
            let mut body = filter.body(PAGE_SIZE);
            match &next {
                Some(NextPage::Cursor(c)) => {
                    body["cursor"] = serde_json::Value::String(c.clone());
                }
                Some(NextPage::Page(n)) => {
                    body["page"] = serde_json::Value::from(*n);
                }
                None => {}
            }
            let page: SearchResponse = self.post_json("/search/metadata", &body)?;
            let more = page.assets.next();
            out.extend(page.assets.items);
            match more {
                Some(n) => next = Some(n),
                None => return Ok(out),
            }
        }
        Ok(out)
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
    /// ⚠⚠ Callers pass a `.part` path and rename on success, so a partial download never sits
    /// at the path the rest of the app reads.
    ///
    /// ⚠ Never `?edited=true`. Immich's own edits are crop/rotate/mirror and Rustroom applies
    /// its own geometry; importing theirs would apply a crop twice.
    pub fn original(&self, id: &str, dest: &Path) -> Result<(), Error> {
        let mut res = self
            .agent
            .get(self.url(&format!("/assets/{id}/original")))
            .header("x-api-key", &self.key)
            .call()
            .map_err(map_ureq)?;
        let mut file = std::fs::File::create(dest).map_err(|e| Error::Io(e.to_string()))?;
        let body = res.body_mut();
        let mut reader = body.as_reader();
        let mut buf = vec![0u8; 256 * 1024];
        loop {
            let n = std::io::Read::read(&mut reader, &mut buf)
                .map_err(|e| Error::Network(e.to_string()))?;
            if n == 0 {
                break;
            }
            file.write_all(&buf[..n])
                .map_err(|e| Error::Io(e.to_string()))?;
        }
        file.flush().map_err(|e| Error::Io(e.to_string()))?;
        Ok(())
    }

    /// `POST /assets` — upload `file` as a **new** asset, returning its id and whether the
    /// server considered it a duplicate.
    ///
    /// ⚠⚠ Requires **`asset.upload`** on the key, which is a grant *separate from*
    /// `asset.update`: a key that can do this **cannot modify or delete existing assets**. That
    /// separation is the whole reason uploading was built and two-way sync was declined.
    ///
    /// ⚠ `created` and `modified` are pre-formatted strings, because the two carry **different
    /// conventions** and only the caller knows which: capture time is naive wall-clock rendered as
    /// if UTC (matching what Immich itself did to the sibling JPEG of a real pair), while a file
    /// mtime is a true instant. See `immich_upload` in the app.
    ///
    /// ⚠ The body is built **in memory**. An export already holds a whole decoded image (the
    /// export worker's own comment measures one at 298 MiB), so a 15 MB JPEG here is not the
    /// constraint that matters.
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
                multipart::Part::Text {
                    name: "fileCreatedAt",
                    value: created,
                },
                multipart::Part::Text {
                    name: "fileModifiedAt",
                    value: modified,
                },
                multipart::Part::Text {
                    name: "filename",
                    value: filename,
                },
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
}

/// The media type for an exported file, from its extension.
///
/// ⚠ Only the formats `export` can write. A default of `application/octet-stream` is deliberate:
/// Immich sniffs the content anyway, and guessing wrongly is worse than not guessing.
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

/// The body of `PUT /assets` (M10.15d): the ids, and only the fields being set.
///
/// ⚠ `rating: Some(None)` is an explicit `null` — an undo back to *unrated*; `None` leaves the field
/// out, so the server keeps whatever it has. Nothing else `AssetBulkUpdateDto` allows (dates, location,
/// description, visibility) can be expressed here, on purpose.
pub(crate) fn marks_body(
    ids: &[String],
    rating: Option<Option<i32>>,
    favourite: Option<bool>,
) -> serde_json::Value {
    let mut m = serde_json::Map::new();
    m.insert("ids".into(), serde_json::json!(ids));
    if let Some(r) = rating {
        m.insert("rating".into(), serde_json::json!(r));
    }
    if let Some(f) = favourite {
        m.insert("isFavorite".into(), serde_json::json!(f));
    }
    serde_json::Value::Object(m)
}

/// Map a `ureq` failure onto this crate's error, keeping the status distinction the UI needs.
///
/// ⚠ `http_status_as_error` defaults to `true` in ureq 3, so a 4xx/5xx really does arrive here
/// as `StatusCode` rather than as a successful response.
fn map_ureq(e: ureq::Error) -> Error {
    match e {
        ureq::Error::StatusCode(code) => Error::from_status(code),
        other => Error::Network(other.to_string()),
    }
}

#[cfg(test)]
mod tests {

    /// M10.18c: a person's search names them, and every search asks for the people.
    #[test]
    fn a_person_filter_asks_for_person_ids() {
        let b = Filter::Person("p1".into()).body(100);
        assert_eq!(b["personIds"][0], "p1");
        assert_eq!(b["withPeople"], true);
        assert_eq!(Filter::Album("a".into()).body(1)["withPeople"], true);
    }

    /// CONSTRUCTED from the published schema — the GUI check confirms it on the author's server.
    #[test]
    fn people_reply_parses() {
        let p: PeoplePage = serde_json::from_str(
            r#"{"total":3,"hidden":1,"hasNextPage":false,"people":[
                {"id":"p1","name":"Anna","birthDate":null,"thumbnailPath":"/x","isHidden":false,"isFavorite":false},
                {"id":"p2","name":"","isHidden":false},
                {"id":"p3","name":"Ben","isHidden":true}]}"#,
        )
        .unwrap();
        assert_eq!(p.people.len(), 3);
        assert!(!p.has_next_page);
        let a: Asset = serde_json::from_str(
            r#"{"id":"a","type":"IMAGE","originalFileName":"x.jpg","originalMimeType":null,"width":null,
                "height":null,"thumbhash":null,"fileModifiedAt":"t","updatedAt":"t",
                "people":[{"id":"p1","name":"Anna","isHidden":false,"faces":[{"id":"f1","imageWidth":1000,
                "imageHeight":500,"boundingBoxX1":100,"boundingBoxY1":50,"boundingBoxX2":200,"boundingBoxY2":150,
                "sourceType":"machine-learning"}]}]}"#,
        )
        .unwrap();
        assert_eq!(
            a.people[0].faces[0].normalised(),
            Some([0.1, 0.1, 0.2, 0.3])
        );
    }

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

    #[test]
    fn an_album_filter_and_a_date_filter_are_different_bodies() {
        let a = Filter::Album("6f2e1a94-0c44-4f51-8a7d-9b1c2e3f4a5b".into());
        let d = Filter::TakenBetween {
            after: "2025-11-01T00:00:00.000Z".into(),
            before: "2025-12-01T00:00:00.000Z".into(),
        };
        let ab = a.body(PAGE_SIZE);
        let db = d.body(PAGE_SIZE);
        assert!(
            ab.get("albumIds").is_some(),
            "an album filter sends albumIds"
        );
        assert!(ab.get("takenAfter").is_none());
        assert_eq!(db["takenAfter"], "2025-11-01T00:00:00.000Z");
        assert_eq!(db["takenBefore"], "2025-12-01T00:00:00.000Z");
        assert!(db.get("albumIds").is_none());
        assert_eq!(db["size"], PAGE_SIZE);
        // M10.15c: the rating lives in `exifInfo`, which a search returns only when asked.
        assert_eq!(ab["withExif"], true, "an album asks for the rating");
        assert_eq!(db["withExif"], true, "a month asks for the rating");
    }

    /// M10.15d: only the fields being set travel — never a field this send does not mean to change.
    #[test]
    fn a_marks_body_carries_only_what_changes() {
        let ids = vec!["a".to_string(), "b".to_string()];
        let r = marks_body(&ids, Some(Some(5)), None);
        assert_eq!(r, serde_json::json!({"ids": ["a", "b"], "rating": 5}));
        let f = marks_body(&ids, None, Some(true));
        assert_eq!(
            f,
            serde_json::json!({"ids": ["a", "b"], "isFavorite": true})
        );
        let both = marks_body(&ids, Some(Some(-1)), Some(false));
        assert_eq!(
            both,
            serde_json::json!({"ids": ["a", "b"], "rating": -1, "isFavorite": false})
        );
        // ⚠ An undo to unrated sends an explicit null — omitting it would leave the stars in place.
        let unrated = marks_body(&ids, Some(None), None);
        assert_eq!(
            unrated,
            serde_json::json!({"ids": ["a", "b"], "rating": null})
        );
    }

    /// ⚠⚠ The size values are Immich's `AssetMediaSize` spelling. A typo here is a 400 at runtime
    /// and nowhere else.
    #[test]
    fn the_sizes_spell_immichs_own_values() {
        assert_eq!(Size::Thumbnail.as_str(), "thumbnail");
        assert_eq!(Size::Preview.as_str(), "preview");
        assert_eq!(Size::Fullsize.as_str(), "fullsize");
    }

    #[test]
    fn a_content_type_follows_the_extension() {
        assert_eq!(content_type_of("a.JPG"), "image/jpeg");
        assert_eq!(content_type_of("a.jpeg"), "image/jpeg");
        assert_eq!(content_type_of("a.png"), "image/png");
        assert_eq!(content_type_of("a.tiff"), "image/tiff");
        // ⚠ Not a guess: an unknown extension says so rather than claiming JPEG.
        assert_eq!(content_type_of("a.weird"), "application/octet-stream");
    }
}
