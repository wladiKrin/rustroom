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
    pub width: u32,
    pub height: u32,
    /// Base64 thumbhash. Carried for the blurred placeholder; unused in this slice.
    pub thumbhash: String,
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

    /// ⚠ Parses a captured reply, with all twelve fields this DTO does not model present in it —
    /// `albumUsers`, `startDate`, `order`, `lastModifiedAssetTimestamp` and the rest.
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
        assert!(!a.thumbhash.is_empty());
        assert_eq!((a.width, a.height), (1200, 1600));
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
