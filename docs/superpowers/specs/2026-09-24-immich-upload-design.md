# Uploading edited photos to Immich — design

**Date:** 2026-09-24
**Status:** approved in brainstorming, ready for a plan
**Supersedes:** nothing. **Successor to:** ⛔ two-way sync, *declined* 2026-09-24 (`docs/BACKLOG.md`)

---

## 1. What this is, and why it is not two-way sync

Rustroom edits photos and the result lands on local disk, where it stays. The framing this
integration was built for — *"Immich stores and shares, Rustroom edits"* — has a broken loop: the
finished picture never returns to where the user's photos live and are shared. This closes it.

⚠⚠ **This replaces two-way sync rather than approximating it.** The distinction that got sync
declined is **mutating versus creating**, and it is verified, not assumed: Immich's permission enum
has **168** values, among them **`asset.upload` as a grant separate from `asset.update`**. An
upload-capable key **cannot modify or delete existing assets**. Sync needed `asset.update`, ran
unattended over a whole library, and was irreversible; uploading creates a new asset, on one
deliberate action, and the worst case is an extra file the user deletes in Immich's own UI.

✅ **It also delivers, additively, much of what sync promised.** `export/src/metadata.rs` already
writes a full XMP packet carrying **rating, label, keywords, title, caption, rights and creator**, so
an uploaded edit carries its marks *inside the file* with no permission to modify anything.
⚠⚠ **UNVERIFIED and deliberately not designed around:** whether Immich maps XMP `Rating` onto its own
rating field on ingest. This integration has been bitten four times by claims about Immich taken from
a schema or from memory. **Capture a real reply before relying on it.** Nothing in this spec depends
on it being true.

### Scope, as the user set it

- **Plain upload into the timeline.** Not albums, not stacked with the original, not `archive`.
- **Any photo is uploadable** — a local RAW imported from a card, exactly like one that came from
  Immich. ⚠ This is the decision that makes the whole design an *export destination* rather than an
  Immich-source feature.
- **No library-wide "upload everything edited" sweep.** Its own follow-up.

---

## 2. The verified contract

⚠⚠ **Everything here was read from the OpenAPI spec at version **3.2.0**, which is the version the
user's own server reports.** Not from public docs and not from memory.

`POST /assets`, **`multipart/form-data`**, `AssetMediaCreateDto`:

| field | | notes |
|---|---|---|
| `assetData` | **required** | binary — the exported file |
| `fileCreatedAt` | **required** | date-time |
| `fileModifiedAt` | **required** | date-time |
| `filename` | optional | the name Immich displays |
| `isFavorite`, `visibility`, `duration`, `livePhotoVideoId`, `metadata`, `sidecarData` | optional | unused in v1 |

Response `AssetMediaResponseDto { id, status }` on **200 or 201**, with
`status` ∈ **`created` | `duplicate`**.

⚠ **Two corrections to what memory would have supplied:** there is **no `deviceAssetId`/`deviceId`
requirement** in 3.2.0 (older Immich had them), and `sidecarData` exists as an upload field. Both were
found by reading the spec, which is the point.

### What `status: duplicate` buys, and what it does not

✅ **The server does checksum dedup and reports it.** Uploading byte-identical data twice returns
`duplicate` plus the id of the existing asset, so a repeated upload of the *same export* is already
harmless and needs no client-side guard.

⚠ It does **not** solve the real case: nudge a slider, re-export, and the bytes differ, so the server
correctly reports `created` and a second asset exists. That is not a bug — it is a new picture — but
the user must be able to see it coming. See §5.

---

## 3. Architecture: an export destination

**The Export dialog already answers every question an upload has** — which photos, format, quality,
long edge, colour space, metadata scope, watermark, output sharpening, filename template, collision
handling, progress, cancel. Since any photo is uploadable, the thing uploaded is simply *an export*. A
standalone upload flow would duplicate all of it.

So: **export writes the file to disk as it does today, then the file is uploaded.** The local file
stays. A failed upload therefore never loses work, and the artefact the user asked for exists either
way.

### `ExportOptions` gains one target-agnostic field

```rust
/// Where the exported file is delivered in addition to `dest_dir`. `None` = disk only.
pub upload_target: Option<UploadTarget>,

/// ⚠ One variant today. Named for the *role*, not the service, so `export` never learns the
/// word "Immich" — see `docs/BACKLOG.md`, "Immich must be COMPLETELY optional".
pub enum UploadTarget { Immich }
```

⚠⚠ **Deliberately NOT an Immich-named field.** The user's requirement is that the integration become
switchable off with *"no mention of immich at all"*. A field called `immich` on the shared export
surface would make that impossible to honour later without a rename across every call site. With the
integration off, the enum offers no variant, the dialog draws no section, and the worker skips the
step — while the persisted value stays in the settings file and is simply never read, which is what
makes re-enabling lossless.

⚠ **The honest cost:** `ExportOptions`' own comment says the five M6.1 fields were landed together
*"so that the sub-features can be built independently without each having to edit this struct — which
is the shared surface every one of them would otherwise contend for."* A new delivery target cannot be
configured without touching it. The struct's **container-level `#[serde(default)]`** means an options
file written before this field still loads; a test pins that.

⚠ `Delivery`/`PostExportAction` (M6.1e) is **left alone**. It is documented "stored, inert" and belongs
to an unbuilt milestone, and its framing — *"what happens once the run finishes"* — is run-scoped,
whereas an upload is per-file. Appending a variant there would have bought the data model and none of
the wiring while squatting on someone else's milestone.

### Placement, against the hard crate rule

| unit | home | why |
|---|---|---|
| `Client::upload()` | `crates/immich/src/lib.rs` | zero GUI, beside `original()` |
| the multipart encoder | `crates/immich/src/multipart.rs` **(new)** | pure, and the single most test-worthy piece — `ureq` has **no** multipart helper, so this is hand-built |
| upload decisions: timestamps, filename, "already uploaded?" | `crates/app/src/immich_upload.rs` **(new lib module)** | testable with no window open |
| the per-file upload step | `batch_export/worker.rs` | the file is written, uploaded, then reported — cancel and per-file progress work unchanged |
| the dialog's Immich section | `modals/export.rs` | egui only |
| the upload-key field | `modals/settings.rs`, Immich tab | beside the browse key |

⚠ `batch_export/worker.rs` is **1,264 lines total but 439 of code+docs** (measured 2026-09-24 with
`awk '/#\[cfg\(test\)\]/{exit} {n++}'`; the rest is its own tests). ⚠⚠ `CLAUDE.md` said *"1,127 — ~190 of
code"* and **both numbers were stale** — checked rather than requoted, per the standing lesson. 439 is
still under the ~600 threshold, so adding one step is in keeping; if the upload logic grows past a few
dozen lines it belongs in `immich_upload.rs`, which is where the testable part lives anyway.

---

## 4. The credential: a second, upload-only key

**The browse key stays strictly read-only.** A separate, optional **Upload key** field holds a key
granted `asset.upload`. When it is empty, the upload option is **disabled with a hover explaining
why**, never hidden — a control that vanishes reads as a bug, and this project has taken that back out
once already.

⚠⚠ **Why two keys rather than one with the grant added.** Browsing, thumbnails and the rail run
constantly and unattended; with one key every one of those requests would travel with a write-capable
credential. Two keys make "browsing cannot write" a property of the **credential** rather than a
property of our code being correct. That is the same reasoning as slice 4's *"this cannot touch the
server"* hover, enforced one level lower.

⚠ It is **plaintext in `settings.json`**, exactly like the browse key. The keychain remains out of
scope by decision, and the help text must say so for this field too — a write-capable key in plaintext
deserves the warning more than a read-only one does.

---

## 5. Recording what was uploaded

The returned `{id, status}` goes in the photo's **sidecar**, which already travels with the photo and
already survives a *Remove downloads*:

```
"immich_upload": { "asset_id": "…", "status": "created", "at": "2026-09-24T…Z", "host": "<slug>" }
```

⚠ **Keyed by host slug**, because the same photo may be uploaded to different servers, and a bare
"uploaded" flag would lie on the second one.

The dialog then says **"already uploaded on ‹date›"** for a photo that carries the record, so a
re-export that will create a second asset is visible *before* the run rather than discovered in the
timeline afterwards. ⚠⚠ **It informs, it does not block.** A second upload after further editing is
the correct outcome, and replacing the previous one would mean **deleting** an asset — dragging back
exactly the danger that got sync declined.

⚠ `sidecar::path` **appends** `.json` to the whole filename (`abc.jpg` → `abc.jpg.json`) rather than
replacing the extension. A new writer must not "fix" that.

---

## 6. Timestamps — the correctness trap

⚠⚠ **`fileCreatedAt` is the photo's CAPTURE time, never the upload time.** Sending "now" would land
every edit at today's date instead of beside the original it came from, which defeats the purpose and
is invisible until the user scrolls their timeline. Source order:

1. The capture facts Rustroom already decodes (`Capture`, EXIF).
2. ⚠ Fall back to the source file's **mtime** when EXIF carries no date — some PNG/TIFF sources have
   none at all.

`fileModifiedAt` is the exported file's own mtime, which is honest: that *is* when this rendering was
made.

---

## 7. Failure, cancellation, progress

Per-file, inside the existing loop, so all three come from machinery that exists:

- A failed upload **does not fail the export.** The file is on disk; the run reports the photo as
  exported-but-not-uploaded and continues. ⚠ The opposite — aborting the run — would make a flaky
  network destroy a batch export.
- **Cancel** is the export's existing cancel.
- ⚠⚠ **An SSL/EOF failure means the RESPONSE was lost, not that the upload did not happen.** Measured
  during the stacking migration on 2026-09-24: 4 of 2,373 writes failed this way on this tailnet, and
  none had applied — but that had to be *checked*, not assumed. So a failed upload is reported as
  **"unconfirmed"**, never retried automatically. An automatic retry is how one edit becomes three
  assets.
- ⚠ Sequential, one file at a time. The download path needed a bounded queue because it fanned out a
  thread per asset; the export worker is already a single sequential loop, so it needs no cap — and
  uploading full-size JPEGs in parallel would be the wrong thing to add anyway.

---

## 8. Testing

**What can be tested without a server, and must be:**

1. **The multipart encoder** — given fields and bytes, the body has the boundary in the right places,
   `Content-Disposition` per part, CRLF line endings, and the terminating `--boundary--`. ⚠ This is
   hand-rolled, it is the piece most likely to be subtly wrong, and a server rejecting it gives a
   400 with no hint. Pin one **byte-exact** small body.
2. **Timestamp derivation** — EXIF date wins; mtime fallback when absent; ⚠ and a test that the
   result is *not* "now", which is the actual bug.
3. **The sidecar record** — round-trips, is keyed by host, and an old sidecar without the field still
   loads.
4. **`upload_target: None` is the default**, and an `ExportOptions` file written before the field
   still loads.
5. ⚠⚠ **A test that the export is unchanged when `upload_target` is `None`** — the whole existing
   export path must be byte-identical, which the goldens then also confirm.

**What only a real server can settle, and must therefore be done by use before the milestone closes:**

- That a real `POST /assets` accepts this body at all.
- That the uploaded edit appears at the **capture date**, not today.
- That `status: duplicate` comes back on a second identical upload.

⚠⚠ **Do not write a fixture asserting the upload response shape and call it evidence.** M9.1's
hand-written `album.json` asserted an `assets` field the endpoint has never returned, and passed
against 2,678 green tests while the first real album open failed immediately. **Capture the reply from
a real upload and say in the fixture that it was captured.**

---

## 9. Out of scope, by decision

Each is its own later change, not an oversight:

- **Albums** (needs an album grant), **stacking the edit with its original** (⚠ `PUT /stacks/{id}`
  takes only `primaryAssetId` — there is **no "add to stack"**, so it means `DELETE` then re-`POST`
  with three ids), **`visibility: archive`**, **`sidecarData`**.
- **Upload without keeping a local copy.**
- **The library-wide "upload everything edited" sweep.**
- **The OS keychain** — the upload key is plaintext, like the browse key.
- **Reading Immich's own `rating`/`isFavorite`** — the inbound direction, still unbuilt.
- ⛔ **Two-way sync** — declined with reasons; do not re-propose from a list.

---

## 10. Definition of done

1. An export with `upload_target: Some(Immich)` writes the file to disk **and** creates an asset on the
   server, at the photo's capture date, for a photo that never came from Immich.
2. With `upload_target: None`, the export path is unchanged and the goldens are unmoved.
3. With no upload key set, the control is **visible and disabled**, with a hover saying why.
4. A photo already uploaded says so in the dialog, and uploading again is still permitted.
5. A failed upload leaves the exported file intact and is reported as unconfirmed.
6. ⚠ The whole thing has been **seen working against the real server** — almost every GUI-wiring defect
   in this repo was found by use, not by a test, and every Immich defect so far was.
