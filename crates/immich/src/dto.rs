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
}
