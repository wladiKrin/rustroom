//! The parts of Immich's JSON this client reads.
//!
//! ⚠⚠ **Deliberately narrow.** These mirror only the fields used, not Immich's full
//! `AssetResponseDto` — 24 of its endpoints already carry a `Deprecated` tag, and a narrow DTO
//! is a narrow blast radius when the API moves.
//!
//! ⚠ Every struct ignores unknown fields (serde's default) and every enum has an `Other`
//! fallback, so a server upgrade that adds a field or a kind does not break browsing.

use serde::{Deserialize, Serialize};

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
    /// When an asset in this album last changed.
    ///
    /// ⚠⚠ **The album list cache's validator, together with `asset_count`.** Not `updated_at`: the
    /// captured reply has that identical for every album — evidently a bulk touch — so it never
    /// signals a content change. This field differs per album and is explicitly about its assets.
    ///
    /// ⚠ `Option` because the schema does not mark it required. An album without it simply cannot be
    /// validated, so its list is refetched rather than trusted.
    pub last_modified_asset_timestamp: Option<String>,
}

/// One row of `GET /timeline/buckets`.
///
/// ⚠ `time_bucket` is `YYYY-MM-DD`, and on a real 3.2.0 server it is always the **first of a
/// month** — the endpoint takes no size parameter, so the granularity is the server's choice.
/// Captured and pinned rather than assumed.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Bucket {
    pub time_bucket: String,
    pub count: u32,
}

/// `POST /search/metadata` — one page of results.
///
/// ⚠⚠ **This is how an album's assets are fetched, and `GET /albums/{id}` is not.**
/// `AlbumResponseDto` in Immich 3.2.0 carries `assetCount` and **no `assets` field at all**, so
/// asking that endpoint for assets fails with "missing field `assets`" — which is exactly what it
/// did in use, after a hand-written fixture had asserted the opposite.
#[derive(Debug, Clone, Deserialize)]
pub struct SearchResponse {
    pub assets: SearchAssets,
}

/// The `assets` half of a search response.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchAssets {
    pub items: Vec<Asset>,
    /// The cursor for the next page, when the server uses cursor paging.
    ///
    /// ⚠⚠ **A real 3.2.0 server leaves this `null` and fills `next_page` instead**, even on a
    /// reply that plainly has more pages to give. Following only this one stops after the first
    /// page and returns a **silently truncated** album — see [`Self::next_page`].
    #[serde(default)]
    pub next_cursor: Option<String>,
    /// The next page number, as a string, when the server uses page paging.
    ///
    /// ⚠⚠ This is what a real server actually sends: `"nextPage": "2", "nextCursor": null`,
    /// captured 2026-09-22 from a 9-asset album queried with `size: 2`. The spec marks the `page`
    /// *request* field deprecated, which is what led to cursor-only paging — and the spec does not
    /// say the server declines to populate `nextCursor`. Follow **whichever** the reply provides.
    #[serde(default)]
    pub next_page: Option<String>,
}

impl SearchAssets {
    /// Is there another page, and how should it be asked for?
    ///
    /// ⚠ Cursor preferred when present, so a future server that switches to cursors needs no
    /// change here. An empty string counts as absent either way: a server sending `""` must not put
    /// the loop into an endless spin.
    pub fn next(&self) -> Option<NextPage> {
        if let Some(c) = self.next_cursor.as_deref().filter(|c| !c.is_empty()) {
            return Some(NextPage::Cursor(c.to_string()));
        }
        // ⚠ Parsed, not passed through: `page` is an integer in the request and a string in the
        // reply, and a reply of "not a number" must end the loop rather than be sent back.
        self.next_page
            .as_deref()
            .filter(|p| !p.is_empty())
            .and_then(|p| p.parse::<u32>().ok())
            .map(NextPage::Page)
    }
}

/// How to ask for the page after this one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NextPage {
    Cursor(String),
    Page(u32),
}

/// What kind of thing an asset is.
///
/// ⚠ `Other` catches anything Immich adds later. Without it one unknown kind would fail the
/// whole album's parse.
/// ⚠ `Serialize` because a stored asset list is written back as JSON. `#[serde(other)]` is a
/// deserialise-only attribute, so `Other` writes as `"Other"` and reads back through that same
/// fallback — round-tripped by a test rather than assumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum AssetType {
    #[serde(rename = "IMAGE")]
    Image,
    #[serde(rename = "VIDEO")]
    Video,
    #[serde(other)]
    Other,
}

/// One asset, narrowed to what the browse grid and the download need.
/// ⚠ `Serialize` is here for the on-disk asset-list cache (`app`'s `immich_list_cache`). The other
/// DTOs in this file stay read-only deliberately: a type that can be written invites someone to
/// write it, and nothing else here is ever sent anywhere.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Asset {
    pub id: String,
    #[serde(rename = "type")]
    pub asset_type: AssetType,
    /// The name the file had on the camera — what the grid shows, never the UUID.
    pub original_file_name: String,
    pub original_mime_type: Option<String>,
    /// ⚠⚠ `Option`, because the schema marks these `required: true` **and** `nullable: true` —
    /// "required" means the key is present, not that it has a value. Carried, not used.
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// Base64 thumbhash. Carried for the blurred placeholder; unused so far.
    ///
    /// ⚠⚠ **`Option` for the same reason, and this one was found in use**: an asset whose thumbnail
    /// has not been generated sends `null`, and deserialisation happens before the image filter, so a
    /// single such asset failed an entire month of the timeline.
    pub thumbhash: Option<String>,
    /// RFC 3339. Kept as a string here and parsed in `app`, so this crate needs no time crate.
    pub file_modified_at: String,
    /// When the asset record last changed, RFC 3339.
    ///
    /// ⚠ Used as the **cache version** for this asset's thumbnail — see
    /// `app`'s `thumb_store::Identity::Remote`. A favourite toggle changes it and costs one
    /// refetch, which is the deliberate trade against `thumbhash`, whose perceptual hash could
    /// survive a re-encode unchanged.
    pub updated_at: String,
    /// Whether the asset is a favourite in Immich (M10.15c) — in **every** search reply, a plain
    /// bool (captured 2026-10-13, `withExif` or not).
    ///
    /// ⚠ `default`: the lists cached before M10.15c lack it, and must still parse.
    #[serde(default)]
    pub is_favorite: bool,
    /// The asset's EXIF block — sent only when a search asks `withExif: true` (M10.15c).
    ///
    /// ⚠ Only the rating and the position are modelled: a narrow DTO is one the next server version
    /// cannot break with a field it reshapes.
    #[serde(default)]
    pub exif_info: Option<ExifInfo>,
    /// The people Immich found in the asset, with their face boxes — sent when a search asks
    /// `withPeople: true` (M10.18c). ⚠ CONSTRUCTED from the published schema (`PersonWithFacesResponseDto`):
    /// confirm against a captured reply. ⚠ `None` — no field at all, as in a list cached before M10.18c — is not
    /// "nobody": only `Some` (even empty) replaces an asset's Immich faces.
    #[serde(default)]
    pub people: Option<Vec<AssetPerson>>,
}

/// One person in an asset, and where their faces are (M10.18c).
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetPerson {
    pub id: String,
    /// ⚠ Empty for a person nobody named on the server.
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub is_hidden: bool,
    #[serde(default)]
    pub faces: Vec<FaceBox>,
}

/// A face's box in the pixels of the image Immich measured it on (`imageWidth` × `imageHeight`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FaceBox {
    pub image_width: f64,
    pub image_height: f64,
    pub bounding_box_x1: f64,
    pub bounding_box_y1: f64,
    pub bounding_box_x2: f64,
    pub bounding_box_y2: f64,
}

impl FaceBox {
    /// `[x0, y0, x1, y1]` normalised over the image, or `None` for a box with no image size.
    pub fn normalised(&self) -> Option<[f32; 4]> {
        (self.image_width > 0.0 && self.image_height > 0.0).then(|| {
            [
                self.bounding_box_x1 / self.image_width,
                self.bounding_box_y1 / self.image_height,
                self.bounding_box_x2 / self.image_width,
                self.bounding_box_y2 / self.image_height,
            ]
            .map(|v| v.clamp(0.0, 1.0) as f32)
        })
    }
}

/// One page of `GET /people` (M10.18c). ⚠ CONSTRUCTED from the published schema (`PeopleResponseDto`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeoplePage {
    #[serde(default)]
    pub people: Vec<Person>,
    #[serde(default)]
    pub has_next_page: bool,
}

/// One person on the server.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Person {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub is_hidden: bool,
}

/// The part of an asset's `exifInfo` Rustroom reads.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExifInfo {
    /// Stars 1–5, or `null` when unrated.
    ///
    /// ⚠⚠ **−1 means rejected** where the server's schema lets a client *write* a rating, while the
    /// reply's schema says 1–5 only — a contradiction no real reply has settled, so the reader
    /// (`app`'s `immich_catalog::server_marks`) handles −1 rather than trusting it away.
    #[serde(default)]
    pub rating: Option<i32>,
    /// Where it was taken, decimal degrees (M10.18b). ⚠ Nullable, and `null` on every photo without
    /// GPS. ⚠ Constructed from the schema — confirm against a captured reply from the author's server.
    #[serde(default)]
    pub latitude: Option<f64>,
    #[serde(default)]
    pub longitude: Option<f64>,
}

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

/// One stack: an ordered group of assets the server treats as one photograph.
///
/// ⚠⚠ **Deliberately narrow.** The real reply embeds the **full asset DTO** for every member —
/// 20-plus fields each, 6.6 MB for 2,379 stacks — and Rustroom needs two UUIDs. Widening this means
/// parsing and keeping data nothing reads.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stack {
    pub id: String,
    /// The member Immich shows for this stack.
    ///
    /// ⚠ Parsed and **deliberately unused for display**: Rustroom applies its own `Stacking::primary`
    /// rule so the Appearance setting means one thing everywhere (user's call, 2026-09-25). Kept in
    /// the DTO because dropping a field the reply carries is how a fixture drifts from reality.
    pub primary_asset_id: String,
    pub assets: Vec<StackMember>,
}

/// A stack member. ⚠ Only the id: see [`Stack`] for why this is not the whole asset.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct StackMember {
    pub id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    const ABOUT: &str = include_str!("../tests/fixtures/about.json");
    const ALBUMS: &str = include_str!("../tests/fixtures/albums.json");
    const SEARCH: &str = include_str!("../tests/fixtures/search_album.json");
    /// ⚠⚠ **A real reply**, captured 2026-09-22 from a 9-asset album queried with `size: 2`,
    /// scrubbed of the owner id, the storage paths and the checksums. It is the only evidence that
    /// `nextCursor` is null while `nextPage` is set — the schema does not say so anywhere.
    const SEARCH_REAL: &str = include_str!("../tests/fixtures/search_real.json");
    /// ⚠⚠ **Captured**, not constructed: a real `GET /timeline/buckets` reply. It is the only
    /// evidence that buckets are monthly and newest-first — the endpoint takes no size parameter, so
    /// granularity is the server's choice and the schema does not state it. ⚠ Three months are
    /// dropped from it deliberately, so a **gap** is exercised; a tree builder that assumed
    /// contiguity would pass on a dense fixture and fail on a real library.
    const BUCKETS_REAL: &str = include_str!("../tests/fixtures/buckets_real.json");
    /// ⚠⚠ **A real reply from a real server**, captured 2026-09-22 and scrubbed of the owner's
    /// name and email. The others in this directory are shaped from the OpenAPI schema; this one
    /// is not, and that difference matters — a hand-written `album.json` once asserted that
    /// `GET /albums/{id}` returns an `assets` array, which it does not, and the test passed because
    /// it was checking my assumption against itself rather than against a server.
    const ALBUMS_REAL: &str = include_str!("../tests/fixtures/albums_real.json");
    /// ⚠⚠ **Captured**, 2026-10-13, from the author's server (v3.2.0): `POST /search/metadata` with
    /// `isFavorite: true, withExif: true` — his one favourite — trimmed to the fields this DTO reads
    /// plus a few it does not; the location and the rest of `exifInfo` removed.
    const SEARCH_MARKS_REAL: &str = include_str!("../tests/fixtures/search_marks_real.json");

    #[test]
    fn a_real_favourite_reads_its_heart_and_its_null_rating() {
        let r: SearchResponse = serde_json::from_str(SEARCH_MARKS_REAL).expect("must parse");
        let a = &r.assets.items[0];
        assert!(a.is_favorite);
        assert_eq!(a.exif_info.as_ref().map(|e| e.rating), Some(None));
    }

    /// ⚠ **Constructed**: no captured asset carries stars or a reject. The shapes follow the
    /// server's schema (`ExifResponseDto.rating`: integer, nullable).
    #[test]
    fn a_rating_and_a_reject_are_read_from_exif() {
        let with = |rating: i32| -> Asset {
            serde_json::from_value(serde_json::json!({
                "id": "a", "type": "IMAGE", "originalFileName": "a.jpg", "originalMimeType": null,
                "width": 1, "height": 1, "thumbhash": null,
                "fileModifiedAt": "2025-01-01T00:00:00.000Z",
                "updatedAt": "2025-01-01T00:00:00.000Z",
                "isFavorite": false, "exifInfo": {"rating": rating}
            }))
            .unwrap()
        };
        assert_eq!(with(3).exif_info.unwrap().rating, Some(3));
        assert_eq!(with(-1).exif_info.unwrap().rating, Some(-1));
    }

    /// ⚠⚠ The lists cached before M10.15c lack both fields — they must still parse.
    #[test]
    fn an_old_cached_asset_without_the_new_fields_parses() {
        let a: Asset = serde_json::from_value(serde_json::json!({
            "id": "a", "type": "IMAGE", "originalFileName": "a.jpg", "originalMimeType": null,
            "width": 1, "height": 1, "thumbhash": null,
            "fileModifiedAt": "2025-01-01T00:00:00.000Z",
            "updatedAt": "2025-01-01T00:00:00.000Z"
        }))
        .unwrap();
        assert!(!a.is_favorite);
        assert_eq!(a.exif_info, None);
    }

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

    /// ⚠ Parses a captured reply, with the eleven fields this DTO does not model present in it —
    /// `albumUsers`, `startDate`, `order` and the rest. (`lastModifiedAssetTimestamp` was among them
    /// until it became the cache validator; see
    /// `a_real_albums_reply_carries_the_per_album_change_signal`.)
    #[test]
    fn a_real_albums_reply_parses_and_its_extra_fields_are_ignored() {
        let v: Vec<AlbumSummary> =
            serde_json::from_str(ALBUMS_REAL).expect("a captured /albums reply must parse");
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].album_name, "analog");
        assert_eq!(v[0].asset_count, 872);
        assert_eq!(v[0].id, "a6e6f5aa-8be7-454e-9328-82dc0cc2ab2b");
        assert_eq!(v[1].album_name, "Rom");
        assert_eq!(v[1].asset_count, 112);
    }

    /// ⚠⚠ The album validator is `(assetCount, lastModifiedAssetTimestamp)`, **not** `updatedAt`:
    /// the captured reply shows `updatedAt` identical for both albums — a bulk touch — so it says
    /// nothing about an album's contents. This asserts the real captured values, which is the only
    /// way to know the field is there and means what it appears to.
    #[test]
    fn a_real_albums_reply_carries_the_per_album_change_signal() {
        let v: Vec<AlbumSummary> = serde_json::from_str(ALBUMS_REAL).expect("must parse");
        assert_eq!(
            v[0].last_modified_asset_timestamp.as_deref(),
            Some("2026-08-14T08:29:23.862Z")
        );
        assert_eq!(
            v[1].last_modified_asset_timestamp.as_deref(),
            Some("2026-06-14T12:11:35.623Z")
        );
        assert_ne!(
            v[0].last_modified_asset_timestamp, v[1].last_modified_asset_timestamp,
            "it must differ per album, which is the whole point of using it"
        );
    }

    #[test]
    fn a_real_buckets_reply_is_monthly_newest_first_with_counts() {
        let v: Vec<Bucket> = serde_json::from_str(BUCKETS_REAL).expect("buckets must parse");
        assert_eq!(v.len(), 6);
        assert_eq!(v[0].time_bucket, "2026-08-01");
        assert_eq!(v[0].count, 76);
        assert!(
            v.iter().all(|b| b.time_bucket.ends_with("-01")),
            "every bucket must be the first of a month"
        );
        assert!(
            v[0].time_bucket > v[1].time_bucket,
            "the server returns newest first"
        );
    }

    #[test]
    fn a_search_page_reads_its_assets() {
        let r: SearchResponse = serde_json::from_str(SEARCH).expect("a search page must parse");
        assert_eq!(r.assets.items.len(), 4);
        assert_eq!(r.assets.items[0].original_file_name, "DSCF1234.RAF");
        assert!(matches!(r.assets.items[0].asset_type, AssetType::Image));
        assert!(matches!(r.assets.items[1].asset_type, AssetType::Video));
    }

    /// ⚠ A `null` cursor is the last page, and must not read as an error or as a cursor of "null".
    #[test]
    fn a_null_next_cursor_with_no_next_page_is_the_last_page() {
        let r: SearchResponse = serde_json::from_str(SEARCH).expect("must parse");
        assert!(r.assets.next_cursor.is_none());
        assert!(r.assets.next().is_none(), "both absent means done");
    }

    /// ⚠⚠ **The regression for the truncation bug.** A real server answers a paged album with
    /// `nextCursor: null` and `nextPage: "2"`. Following the cursor alone stops after page one and
    /// returns a silently short album — no error, just missing photos. Hidden in the UI only
    /// because a 1000-asset page covers most albums in one request.
    #[test]
    fn a_real_reply_signals_its_next_page_by_number_not_by_cursor() {
        let r: SearchResponse =
            serde_json::from_str(SEARCH_REAL).expect("a captured search reply must parse");
        assert_eq!(r.assets.items.len(), 2);
        assert!(
            r.assets.next_cursor.is_none(),
            "the server leaves this null"
        );
        assert_eq!(r.assets.next_page.as_deref(), Some("2"));
        assert_eq!(
            r.assets.next(),
            Some(NextPage::Page(2)),
            "and there must be a next page to fetch"
        );
    }

    /// ⚠ `updatedAt` versions a cached thumbnail, so it must really be present. The captured
    /// reply confirms it.
    /// ⚠⚠ **The bug this test exists for, reported in use 2026-09-23:** opening February 2026 failed
    /// with *"invalid type: null, expected a string"*. `thumbhash` is marked `required: true` **and**
    /// `nullable: true` in the schema — "required" here means the key is present, **not** that it has
    /// a value. An asset whose thumbnail has not been generated, or a video, sends `null`, and because
    /// deserialisation happens before the image filter, **one such asset failed a whole month**.
    ///
    /// ⚠ `width` and `height` carry the same pair of flags. Those three are the only nullable fields
    /// this DTO models — audited against the schema rather than guessed at, after the first fix
    /// attempt would have been "make thumbhash optional and see".
    #[test]
    fn a_null_thumbhash_width_or_height_parses_rather_than_failing_the_page() {
        let json = r#"{"id":"11111111-1111-4111-8111-111111111111","type":"IMAGE",
            "originalFileName":"pending.jpg","width":null,"height":null,"thumbhash":null,
            "fileModifiedAt":"2026-02-03T10:00:00.000Z","updatedAt":"2026-02-03T10:00:00.000Z"}"#;
        let a: Asset =
            serde_json::from_str(json).expect("a null thumbhash must not fail the parse");
        assert_eq!(a.original_file_name, "pending.jpg");
        assert!(a.thumbhash.is_none());
        assert!(a.width.is_none());
        assert!(a.height.is_none());
    }

    #[test]
    fn a_real_reply_carries_updated_at() {
        let r: SearchResponse = serde_json::from_str(SEARCH_REAL).expect("must parse");
        assert_eq!(r.assets.items[0].updated_at, "2026-04-27T12:34:32.784Z");
    }

    /// ⚠ The asset fields this DTO requires are all present in a real reply — the ones the cache
    /// path and the thumbnail depend on.
    #[test]
    fn a_real_reply_carries_every_field_the_dto_requires() {
        let r: SearchResponse = serde_json::from_str(SEARCH_REAL).expect("must parse");
        let a = &r.assets.items[0];
        assert_eq!(a.original_file_name, "IMG-20240823-WA0000.jpg");
        assert_eq!(a.original_mime_type.as_deref(), Some("image/jpeg"));
        assert!(matches!(a.asset_type, AssetType::Image));
        // ⚠ Present in *this* captured reply, but the type is Option because the schema marks it
        // nullable — see `a_null_thumbhash_width_or_height_parses_rather_than_failing_the_page`.
        assert_eq!(a.thumbhash.as_deref(), Some("pfcJHQT3aM1GyGb3lVqHllY3cDAE"));
        assert_eq!((a.width, a.height), (Some(1200), Some(1600)));
        assert_eq!(a.file_modified_at, "2024-08-23T14:22:56.000Z");
    }

    /// ⚠ A cursor wins when the server sends both, so a future server switching to cursors needs
    /// no change.
    #[test]
    fn a_cursor_is_preferred_over_a_page_number() {
        let json = r#"{"albums":{"total":0,"count":0,"items":[],"facets":[]},
            "assets":{"total":0,"count":0,"items":[],"facets":[],
            "nextPage":"7","nextCursor":"abc"}}"#;
        let r: SearchResponse = serde_json::from_str(json).expect("must parse");
        assert_eq!(r.assets.next(), Some(NextPage::Cursor("abc".into())));
    }

    /// ⚠ An unparseable page ends the loop rather than being echoed back to the server.
    #[test]
    fn a_nonsense_next_page_ends_the_paging() {
        let json = r#"{"albums":{"total":0,"count":0,"items":[],"facets":[]},
            "assets":{"total":0,"count":0,"items":[],"facets":[],
            "nextPage":"not a number","nextCursor":null}}"#;
        let r: SearchResponse = serde_json::from_str(json).expect("must parse");
        assert!(r.assets.next().is_none());
    }

    /// ⚠⚠ **The regression this whole change exists for.** `AlbumResponseDto` has no `assets`
    /// field, so a DTO that expects one fails on a real reply. The album list endpoint is still
    /// used — for the list — and must keep parsing without one.
    #[test]
    fn an_album_row_has_no_assets_field_and_that_is_correct() {
        let v: Vec<AlbumSummary> = serde_json::from_str(ALBUMS).expect("albums must parse");
        assert_eq!(v[0].asset_count, 412, "the count comes from the album row");
    }

    /// ⚠ Immich keeps adding fields; a DTO that fails on an unknown one breaks on every server
    /// upgrade. Deserialisation must ignore what it does not know.
    #[test]
    fn an_unknown_field_is_ignored_rather_than_failing() {
        let json = r#"{"id":"11111111-1111-4111-8111-111111111111","type":"IMAGE",
            "originalFileName":"a.RAF","width":1,"height":1,"thumbhash":"x",
            "fileModifiedAt":"2025-08-14T09:21:33.000Z","updatedAt":"2026-01-01T00:00:00.000Z","somethingNewIn3_3":true}"#;
        let a: Asset =
            serde_json::from_str(json).expect("an unknown field must not fail the parse");
        assert_eq!(a.original_file_name, "a.RAF");
    }

    /// ⚠⚠ A stored list is a `Vec<Asset>` written back out as JSON, so `Asset` has to serialise as
    /// well as deserialise — it derived only `Deserialize`.
    ///
    /// ⚠ `AssetType` carries `#[serde(other)]`, which is a **deserialise-only** attribute. This
    /// round-trips all three variants rather than assuming the enum survives a write: `Other` must
    /// come back as `Other`, not fail and not silently become `Image`.
    #[test]
    fn an_asset_round_trips_through_json_including_an_unknown_type() {
        let json = r#"[{"id":"11111111-1111-4111-8111-111111111111","type":"IMAGE",
            "originalFileName":"a.RAF","originalMimeType":"image/x-fuji-raf","width":10,"height":20,
            "thumbhash":"t","fileModifiedAt":"2025-01-01T00:00:00.000Z",
            "updatedAt":"2025-01-02T00:00:00.000Z"},
            {"id":"22222222-2222-4222-8222-222222222222","type":"VIDEO",
            "originalFileName":"b.MP4","width":null,"height":null,"thumbhash":null,
            "fileModifiedAt":"2025-01-01T00:00:00.000Z",
            "updatedAt":"2025-01-02T00:00:00.000Z"},
            {"id":"33333333-3333-4333-8333-333333333333","type":"AUDIO",
            "originalFileName":"c.mp3","width":null,"height":null,"thumbhash":null,
            "fileModifiedAt":"2025-01-01T00:00:00.000Z",
            "updatedAt":"2025-01-02T00:00:00.000Z"}]"#;
        let before: Vec<Asset> = serde_json::from_str(json).expect("must parse");
        let written = serde_json::to_string(&before).expect("must serialise");
        let after: Vec<Asset> = serde_json::from_str(&written).expect("must parse back");

        assert_eq!(after.len(), 3);
        assert!(matches!(after[0].asset_type, AssetType::Image));
        assert!(matches!(after[1].asset_type, AssetType::Video));
        assert!(
            matches!(after[2].asset_type, AssetType::Other),
            "an unknown type must survive a write as Other, not become Image"
        );
        assert_eq!(after[0].original_file_name, "a.RAF");
        assert_eq!(
            after[0].original_mime_type.as_deref(),
            Some("image/x-fuji-raf")
        );
        assert_eq!((after[0].width, after[0].height), (Some(10), Some(20)));
        assert!(after[1].thumbhash.is_none());
        assert_eq!(after[0].updated_at, "2025-01-02T00:00:00.000Z");
    }

    /// ⚠ `originalMimeType` is optional in the spec. A missing one must not fail the album.
    #[test]
    fn a_missing_mime_type_is_none_not_an_error() {
        let json = r#"{"id":"11111111-1111-4111-8111-111111111111","type":"IMAGE",
            "originalFileName":"a.RAF","width":1,"height":1,"thumbhash":"x",
            "fileModifiedAt":"2025-08-14T09:21:33.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#;
        let a: Asset = serde_json::from_str(json).expect("a missing mime type must parse");
        assert!(a.original_mime_type.is_none());
    }

    /// ⚠⚠ An unrecognised asset type must not fail the whole album either — Immich could add one.
    #[test]
    fn an_unknown_asset_type_is_other_not_a_parse_failure() {
        let json = r#"{"id":"11111111-1111-4111-8111-111111111111","type":"AUDIO",
            "originalFileName":"a.mp3","width":0,"height":0,"thumbhash":"x",
            "fileModifiedAt":"2025-08-14T09:21:33.000Z","updatedAt":"2026-01-01T00:00:00.000Z"}"#;
        let a: Asset = serde_json::from_str(json).expect("an unknown type must parse");
        assert!(matches!(a.asset_type, AssetType::Other));
    }

    /// ✅ **CAPTURED from a real 3.2.0 server on 2026-09-24**, not constructed — a re-upload of an
    /// already-uploaded file, byte for byte, replying **HTTP 200** with exactly this body. The
    /// asset count was 16,118 before and after, so the server really did dedupe and create
    /// nothing.
    ///
    /// ⚠⚠ The previous version of this test was hand-written from the OpenAPI schema and said so.
    /// It is replaced because a constructed fixture proves only self-consistency — M9.1's
    /// `album.json` asserted an `assets` field that endpoint has never returned and passed against
    /// 2,678 green tests. **Note what the real reply shows that the schema did not:** the field
    /// order is `status` then `id`, and a duplicate is **200**, not 201.
    #[test]
    fn an_upload_reply_deserialises() {
        // The captured duplicate reply, verbatim.
        let dup: Upload = serde_json::from_str(
            r#"{"status":"duplicate","id":"83d3bcc5-043b-47a0-a0db-9025d96f7a17"}"#,
        )
        .unwrap();
        assert_eq!(dup.status, UploadStatus::Duplicate);
        assert_eq!(dup.id, "83d3bcc5-043b-47a0-a0db-9025d96f7a17");

        // ⚠ `created` is the same shape with the other status word — the first upload of that same
        // file returned it, and the asset it names is the one the duplicate above points back to.
        let created: Upload = serde_json::from_str(
            r#"{"status":"created","id":"83d3bcc5-043b-47a0-a0db-9025d96f7a17"}"#,
        )
        .unwrap();
        assert_eq!(created.status, UploadStatus::Created);
    }

    /// ⚠ An unknown status must not fail the whole upload — the asset exists either way, and a
    /// future Immich adding a third value should not make a successful upload read as an error.
    #[test]
    fn an_unknown_upload_status_is_tolerated() {
        let u: Upload = serde_json::from_str(r#"{"id":"x","status":"something-new"}"#).unwrap();
        assert_eq!(u.status, UploadStatus::Other);
    }

    /// ✅ **CAPTURED from the real server on 2026-09-25**, then trimmed to the fields this DTO reads
    /// — the reply itself carries 20-plus fields per member and is 6.6 MB for 2,379 stacks.
    ///
    /// ⚠⚠ Captured rather than written from the schema, which this integration has been bitten by
    /// four times: `album.json` asserted an `assets` field that endpoint never returns and passed
    /// against 2,678 green tests.
    #[test]
    fn a_captured_stack_reply_deserialises() {
        let captured = r#"[{
            "id": "63ab0329-7794-453a-ba8e-88df9a41e842",
            "primaryAssetId": "ad41fe4c-9226-460b-8e35-8ebacdc3a0ce",
            "assets": [
                {"id": "ad41fe4c-9226-460b-8e35-8ebacdc3a0ce"},
                {"id": "cd22ec81-5f2d-4a42-b532-ebc5891375eb"}
            ]
        }]"#;
        let v: Vec<Stack> = serde_json::from_str(captured).unwrap();
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].id, "63ab0329-7794-453a-ba8e-88df9a41e842");
        assert_eq!(
            v[0].primary_asset_id,
            "ad41fe4c-9226-460b-8e35-8ebacdc3a0ce"
        );
        assert_eq!(v[0].assets.len(), 2);
        assert_eq!(v[0].assets[1].id, "cd22ec81-5f2d-4a42-b532-ebc5891375eb");
    }

    /// ⚠ The real reply's members carry many more fields, and unknown ones must be **ignored**
    /// rather than fail the whole fetch — which is what keeps this DTO narrow *and* safe.
    #[test]
    fn unknown_member_fields_are_ignored() {
        let v: Vec<Stack> = serde_json::from_str(
            r#"[{"id":"s","primaryAssetId":"a","assets":[
                 {"id":"a","checksum":"x","duplicateId":null,"isArchived":false,"height":4160}]}]"#,
        )
        .unwrap();
        assert_eq!(v[0].assets[0].id, "a");
    }
}
