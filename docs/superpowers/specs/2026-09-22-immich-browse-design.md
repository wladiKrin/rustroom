# Immich, the first slice: browse albums, download on open — design

**Date:** 2026-09-22. **Branch:** `immich-browse`. **Status:** shape **A** of `docs/BACKLOG.md`'s
"Immich integration" (M9.1), chosen in conversation 2026-09-22 after the API was read rather than
assumed. Deliberately the smallest end-to-end slice that can be *used*. **Written to be executed by
a different agent**: everything it needs is in this file and the code it names.

## Read first

- `CLAUDE.md` — the conventions. The ones that bite here: **no `Co-Authored-By` trailer**; **never
  push**; every cargo command needs
  `export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"`;
  the gate is `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace`, **each exit code read from its own command, never two gates at once**
  (seven temp-dir test sites collide); every fresh test binary stalls ~22 s on this Mac, so a gate
  is 15–30 min — run it in the background with logs; launch the GUI after the work with
  `cargo run --release -p app`; **egui lives only in the bin modules**; a file past ~600 lines is
  reported, not silently grown.
- `docs/BACKLOG.md` §"Immich integration" — the three slices and the three honest costs. ⚠ Its
  *"verify this FIRST"* warning is **answered** by this spec's next section; do not re-research it.
- `crates/app/src/sources.rs:180` — documented as **"the one place a `Library` is constructed"**.
  That is the seam this slice attaches to, and nothing else in the browse path should build one.
- `crates/app/src/entries.rs` — `PhotoEntry { id, file_name, label, edits }`, and `build` for the
  local equivalent. `crates/app/src/photo_id.rs` — `PhotoId { path, copy }`, the identity type.
- `crates/app/src/thumb_loader.rs` — `LoadFn = Arc<dyn Fn(&ThumbRequest) -> Option<Thumbnail> +
  Send + Sync>`, `with_loader`, and `new`, which **captures its extra state internally** (the
  `ThumbStore` and the `LookStore`) with a comment saying why: to keep its signature and its four
  call sites untouched. This slice follows that pattern, it does not widen the signature.
- `crates/app/src/thumb_key.rs` — `ThumbKey { id: PhotoId, edits: u64 }`. Keyed on the **identity**,
  not on a path, which is what lets a not-yet-downloaded photo use the in-memory cache unchanged.
- `crates/app/src/thumb_store.rs` — `ThumbStore::of(path)` calls `fs::metadata` and returns `None`
  for a file that is not there. One of the four audit sites below.
- `crates/app/src/collections_panel.rs` — `section(ui, &mut open, id, name, count, body)` and the
  five existing calls (lines ~130, ~194, ~268, ~356, ~458). `crates/app/src/safe_name.rs` — reuse
  it for the cache filename. `crates/catalog/src/lib.rs` — `is_supported`.
- `crates/app/src/decode_worker.rs` — the idiom the Immich worker copies: a queue, N threads, a
  channel of results, `drain()` on the UI side.

## What the API actually gives us — verified 2026-09-22

Recorded so it is never re-derived. Source: Immich's OpenAPI spec (`info.version` **3.2.0**, 192
paths) and its server source.

1. **`GET /assets/{id}/original` serves the true original.** *"Downloads the original file of the
   specified asset"*, `application/octet-stream`. `AssetFileType` is `fullsize | preview |
   thumbnail | sidecar | encoded_video` — **the original is not in that list**, it is the asset's
   own `originalPath`; derivatives are additive. `server/src/utils/mime-types.ts` registers
   `.cr2 .cr3 .nef .arw .raf .rw2 .dng .orf .pef .srw .3fr .iiq`. ⚠⚠ **This answers the question
   `docs/BACKLOG.md:1303` said to settle before designing anything.**
2. **Three derivatives per asset**: a blurred thumbhash (inline in list responses), a small
   thumbnail, and a preview at 1440p by default. Sizes come from `AssetMediaSize` =
   `original | fullsize | preview | thumbnail` on `GET /assets/{id}/thumbnail?size=`.
3. ⚠⚠ **The thumbnail format is webp by default and is server-configurable.** The workspace builds
   `image` with `default-features = false, features = ["tiff", "jpeg", "png"]` — **it cannot decode
   what this endpoint returns today.** See "The `image` feature" below.
4. **API keys are finely scoped** — `Permission` is a ~150-value enum including `asset.read`,
   `asset.view`, `asset.download`, `album.read` as separate grants. A key for this slice needs
   exactly those four and **nothing that can write**.
5. **Not used by this slice, recorded so it is not re-researched:** `PUT /assets/{id}/metadata`
   takes an arbitrary `key: string` and a `value` of arbitrary JSON object
   (`server/src/dtos/asset.dto.ts:104`, `z.record(z.string(), z.unknown())`) and rides
   `/sync/stream`'s `AssetMetadataV1`. That is where an op-list would live if edits ever move
   server-side. `/timeline/buckets` returns columnar ids + `ratio` + `thumbhash` — the timeline
   view, deferred. Immich's own `/assets/{id}/edits` is **crop/rotate/mirror only** and is ignored
   here: always fetch the unedited original, never `?edited=true`.

## The shape, and the one idea it rests on

Shape A: **an Immich asset becomes a local file in a cache directory, and a cache directory is just
a folder.** `catalog`, sidecars, `BufferCache`, the pipeline, history, snapshots and export then all
work unchanged, because by the time any of them sees the photo it *is* a local file.

The idea that makes the browse grid possible before anything is downloaded:

> **The cache path is derived deterministically from the asset UUID, so it is known before the file
> exists.** A `PhotoId` can therefore be built for an asset that has never been fetched, which means
> a real `PhotoEntry`, a real `Library`, and the ordinary grid — layout, keyboard navigation,
> rubber-band selection, the filmstrip — with no new UI. Opening the photo makes the path real, and
> from that instant every existing code path applies with no special case.

## `crates/immich` — the client crate

New workspace member. **Zero GUI, zero egui, no knowledge of Rustroom's types.** Dependencies:
`ureq` (blocking — chosen because every background job in this app is a plain thread; `reqwest`'s
blocking API still drags in tokio and hyper), `serde`, `serde_json`, `thiserror`.

```rust
pub struct Client { /* base URL, api key, ureq agent */ }

impl Client {
    pub fn new(base_url: &str, api_key: &str) -> Result<Self, Error>;
    pub fn about(&self) -> Result<About, Error>;            // GET /server/about — version probe
    pub fn albums(&self) -> Result<Vec<AlbumSummary>, Error>; // GET /albums
    pub fn album(&self, id: &str) -> Result<Vec<Asset>, Error>; // GET /albums/{id}
    pub fn thumbnail(&self, id: &str, size: Size) -> Result<Vec<u8>, Error>;
    pub fn original(&self, id: &str, dest: &Path) -> Result<(), Error>;
}
```

- **`Asset` mirrors only what is needed**: `id`, `original_file_name`, `original_mime_type`,
  `type` (image/video), `width`, `height`, `thumbhash`, `file_modified_at`. Not Immich's full
  `AssetResponseDto`. A narrow DTO is a narrow blast radius when the API moves — 24 of its
  endpoints already carry a `Deprecated` tag.
- **`Error`** is a `thiserror` enum distinguishing at minimum `Unauthorized` (401/403),
  `NotFound` (404), `Server(u16)`, `Network`, `Io`, `Decode`. The UI needs to tell "your key is
  wrong" from "your server is down"; a single opaque error makes that impossible.
- **`original` streams to `dest`.** It does not buffer a 50 MB RAW in memory.
- **Transport:** `http://` is accepted, for a server on the LAN. ⚠ **Certificate verification is
  never disabled.** If the user's server has a self-signed certificate this slice will fail to
  connect, and that is reported as a plain error rather than worked around — silently trusting any
  certificate is not a thing to ship, and the honest fix is theirs to make on the server.
- **Tests:** DTO deserialisation against **recorded JSON fixtures** under
  `crates/immich/tests/fixtures/`, plus status-code→`Error` mapping. **No test touches the
  network.**

⚠ The `ureq` API should be read from the version actually resolved at implementation time rather
than written from memory; pin `ureq = "3"` in `[workspace.dependencies]` and adjust.

## The `image` feature

Add `webp` to the workspace's `image` features:

```toml
image = { version = "0.25", default-features = false, features = ["tiff", "jpeg", "png", "webp"] }
```

Decode the fetched bytes with **`image::load_from_memory`**, which sniffs the format from the magic
bytes, rather than assuming webp. The server's thumbnail and preview formats are both admin
settings, so either could arrive, and sniffing costs nothing.

## `crates/app/src/immich_source.rs` — the logic, no egui

A **lib** module, so all of this is unit-testable with no window and no server:

- `cache_root() -> PathBuf` — `<cache_dir>/rustroom/immich/<host_slug>/`.
- `host_slug(base_url: &str) -> String` — a filesystem-safe directory name for one server, via
  `safe_name`. Two servers must not collide.
- `cache_path(root: &Path, asset: &Asset) -> PathBuf` — **deterministic**: `<uuid>-<safe original
  file name>`. The same asset yields the same path forever, so a re-download restores byte-identical
  identity and nothing that recorded the path dangles.
- `entries_from_album(assets: &[Asset], root: &Path) -> Vec<PhotoEntry>` — synthetic `PhotoId`s at
  copy 0, `file_name` from `original_file_name` (the **human** name, not the UUID), `edits` read
  from the sidecar beside the cache path when one exists, `label: None`. **Skips** anything that is
  not an image or that `catalog::is_supported` rejects; skipped assets are absent, not greyed out.
- `is_cached(id: &PhotoId) -> bool` — does the file exist yet.

## The rail section

`collections_panel.rs` is **840 lines**, so the section's body goes in a new **bin** module
`crates/app/src/immich_panel.rs`, called from one line beside the five existing `section(...)`
calls. A sixth header, **at the same level as Folders and Collections**, collapsed state persisted
under `sec_immich` exactly like the others.

Its three states:

1. **Not configured** — one weak line pointing at Settings. No error, nothing alarming.
2. **Configured** — the album list: name and asset count per row. Fetched once when the section is
   first expanded, not every frame. A row click loads that album through `sources.rs`'s
   `Library`-building function and shows it in the grid.
3. **Failed** — the `Error` variant's message, plus a Retry. "Unauthorized" must read differently
   from "could not reach the server"; that is what the error enum is for.

⚠⚠ **Every one of those fetches goes through the worker below, never inline in `show`** — a blocking
HTTP call inside a frame is a frozen window. So each is a hop: the click enqueues a job, the frame
ends, the result arrives on a later frame and *then* the `Library` is built. The section draws from
whatever state it has (unfetched / in flight / loaded / failed) rather than calling the client.

## Thumbnails

One extra branch at the top of the closure `ThumbLoader::new` builds: **if the request's path is
under the Immich cache root and the file is not there**, fetch `?size=thumbnail`, decode it with
`load_from_memory`, and return it. Otherwise the existing `load_thumbnail` path runs untouched.

- The connection is captured as an `Arc<Mutex<Option<Connection>>>` the app owns, following the
  `ThumbStore`/`LookStore` precedent, so pasting a key in Settings does **not** rebuild the loader
  and drop the open folder's thumbnails.
- **The on-disk `ThumbStore` is skipped for remote thumbnails** in this slice. Its key is
  `(path, mtime, size)` from `fs::metadata`, which cannot be built for a file that does not exist.
  The in-memory `ThumbCache` still applies, keyed on `PhotoId` as it already is. Consequence: album
  thumbnails re-fetch after a restart. Acceptable for a first slice; the fix is a key of
  `(asset id, updated_at)` and it is deliberately not here.
- Once a photo **has** been downloaded, the branch falls through and the ordinary local path — disk
  store included — takes over with no special case at all.

## The worker, and open → download

**One** worker for everything the UI needs from the server, in `decode_worker.rs`'s idiom: a queue,
one or two threads, a channel of results, `drain()` on the UI side. Not a second thread pool per
job kind — a `Job` enum:

```rust
enum Job { About, Albums, Album(String), Original { asset: Asset, dest: PathBuf } }
```

`About` backs the Settings Test button, `Albums` and `Album` the rail section, `Original` the open.
Thumbnails are the one exception and do **not** come through here: they already have a worker pool
with a queue the UI replaces wholesale, which is `ThumbLoader`, and that is the right place for
them.

- **Stream to `<dest>.part`, then rename on success.** ⚠⚠ A cancelled, failed or killed download
  must never leave a file that looks like a valid cached original — the whole design rests on
  "the path exists" meaning "the bytes are all there".
- On open, if `is_cached` is false, enqueue and show progress on the cell; when the rename lands,
  the **ordinary** open path runs, because the file is now where `PhotoId` always said it was.
- On failure the photo stays not-cached and the error is reported. No retry storm, no partial file.
- A stale `.part` from a previous crashed run is deleted when its asset is next requested.

## Settings

A section in the Settings dialog with two fields — **Server URL** and **API key** — plus a Test
button that calls `about()` and reports what came back. Stored in `settings.json`, read and written
through a **two-function accessor** in `immich_source.rs` so that moving to the OS keychain later
is a one-file change.

⚠ **The key is stored in plaintext**, which `docs/BACKLOG.md` correctly calls *"the easy path and
probably not the right one"*. Accepted deliberately for this slice, with the keychain named as the
follow-up. The field's help text should say to paste a **read-only** key (`asset.read`,
`asset.view`, `asset.download`, `album.read`) so the exposure is bounded to reads — a key that
cannot write cannot damage the library if the file leaks.

## The new invariant, and its audit

> **A `PhotoId` may name a file that does not exist yet.**

This is new and it is the real risk in this slice. It is bounded by making the Immich browse grid
the **only** place synthetic ids exist. The audit is therefore limited to what that path reaches:

| Site | What it must do |
|---|---|
| `thumb_store::ThumbStore::of` | Already returns `None` on a missing file — **verify**, don't assume |
| `sidecar::load` | Must yield default `Edits`, not an error, when neither file nor sidecar exists |
| `photo_info` | Must render without EXIF rather than showing zeroes or panicking |
| `metadata_scanner` | Must not enqueue, or must skip, a photo whose file is absent |

Each gets a test that passes a `PhotoId` for a nonexistent path.

## What does not change

Nothing in `pipeline`, `gpu-pipeline`, `color`, `export`, `core-image`, `edit-model`, `raw-decode`,
`image-decode`, `xmp` or `camera-profile`. **No catalog migration** — `db.rs` stays at v7, and this
is the central advantage of shape A over a remote-source abstraction. `BufferCache`, `decode`,
`full_res`, `sidecar`, history, snapshots and `batch_export` are untouched.

## Deliberately out of scope

Timeline browsing; catalog rows for Immich photos (**and therefore ratings, flags and colour labels
do not persist on them** — decided 2026-09-22, "we'll deal with the catalog later"); upload and
write-back; cache eviction (the cache grows without bound in this slice, **by design**, because
edits live in sidecars beside the cached files and evicting would destroy them); the OS keychain;
edits in Immich metadata; videos; Immich's own crops; shared/partner assets; search.

## Files

| File | Change |
|---|---|
| `Cargo.toml` | new member `crates/immich`; `ureq = "3"`; `webp` added to `image` features |
| `crates/immich/` | **new crate** — client, DTOs, errors, JSON fixtures and tests |
| `crates/app/src/immich_source.rs` | **new lib module** — cache paths, `entries_from_album`, `is_cached`, the settings accessor |
| `crates/app/src/immich_panel.rs` | **new bin module** — the rail section's body |
| `crates/app/src/immich_worker.rs` | **new lib module** — the one worker, its `Job` enum and results |
| `crates/app/src/collections_panel.rs` | ⚠ 840 lines — **one line**, the sixth `section(...)` |
| `crates/app/src/thumb_loader.rs` | ⚠ 1288 lines — the remote branch in `new`'s closure |
| `crates/app/src/sources.rs` | 571 lines — one more `Library` loader beside the existing ones |
| `crates/app/src/session_flow.rs` | ⚠ 1394 lines — the download-then-open hop |
| `crates/app/src/settings.rs` | ⚠ 2280 lines — two fields |
| `crates/app/src/modals/settings.rs` | 486 lines — the section's UI |
| `crates/app/src/lib.rs` / `main.rs` | module declarations |

⚠ Four of these are already past the ~600-line threshold `CLAUDE.md` names. Every addition to them
is one line to a few; all real code lands in the new modules.

## Testing

- **`crates/immich`**: DTO deserialisation from recorded fixtures; status-code→`Error` mapping;
  a malformed body is an error, not a panic. No network.
- **`immich_source`**: `cache_path` is deterministic (same asset twice → same path); two servers
  get different roots; `host_slug` survives a URL with a port, a path and odd characters;
  `entries_from_album` skips videos and unsupported formats and keeps the human filename;
  `is_cached` is false before and true after a file is created.
- **`immich_worker`**: with an injected fetcher that fails midway, **no file exists at the
  destination** and a `.part` is not mistaken for a download; success renames exactly once.
- **The invariant**: the four audit sites, each given a `PhotoId` for a nonexistent path.
- **The rail section**: follow `collections_panel.rs`'s existing harness (`click_title`, the
  persisted-open-flag tests) for the three states.
- ⚠ **No golden images.** This slice adds no pipeline stage and must leave all 74 goldens unmoved;
  if one moves, something is wrong.

## Done when

The section appears under Collections, an album's photos fill the grid with thumbnails from the
server, opening one downloads the original and lands in Develop with the full pipeline working, the
edit persists to a sidecar beside the cached file, and export produces a file. Marks do not stick,
and that is expected.
