# M6.3a/b — XMP interchange: read on scan, write beside the JSON

*Design, 2026-09-02. Wave 3, track B.*

Ratings, colour labels and keywords round-trip with Lightroom through `stem.xmp` sidecars. Today
they live only in a SQLite catalog no other application can read, which travels with neither the
photos nor the sidecars — and a week of keywording an archive **is** work.

**Slice c** — mapping the develop settings (`crs:*`) — is **deferred**. It is best-effort by
nature and large, and nothing in a+b depends on it.

---

## 1. What round-trips, and what does not

| Field | XMP | Direction | Note |
| --- | --- | --- | --- |
| rating 0–5 | `xmp:Rating` | both | direct |
| colour label 1–4 | `xmp:Label` | both | Rustroom's labels are **red, blue, yellow, green** (`MAX_LABEL` = 4, deliberately not Adobe's five); map 1→`Red`, 2→`Blue`, 3→`Yellow`, 4→`Green`. Any other text (`Purple`, custom) reads as *no label* and the catalog value is left alone. |
| keywords | `dc:subject` (flat bag) **and** `lr:hierarchicalSubject` (`Europe\|France\|Paris`) | both | written as both, Lightroom's own form; read prefers the hierarchical one when present and creates the path's missing keywords. |
| pick / reject | — | **neither** | Lightroom itself keeps flags catalog-only and never writes them to XMP. Record it; do not invent a field. |
| virtual copies | — | copy 0 only | XMP has no notion of a copy. Marks on copies ≥ 1 never reach the sidecar. |

---

## 2. Naming — Lightroom-compatible, RAW only (user's call)

The sidecar is **`stem.xmp`** beside the file (`photo.with_extension("xmp")`), the only name
Lightroom looks for. **RAW files only**, by `catalog::is_raw`: Lightroom embeds XMP *inside*
JPEG/PNG/TIFF rather than writing a sidecar, and Rustroom never modifies originals, so there is no
convention to match for those. A RAW+JPEG pair's JPEG keeps catalog-only marks this wave.

⚠ This is the collision `sidecar::path`'s `.RAF.json` naming was introduced to avoid, taken
knowingly: a sidecar Lightroom cannot find defeats the milestone's entire "done when".

---

## 3. The crate

**`crates/xmp/`**, zero GUI and zero catalog dependency — the "crate first" rule. One dependency:
**`quick-xml`** (pure Rust), chosen over `roxmltree` because §5 needs to *rewrite* a document, not
only read one.

```rust
pub struct Sidecar {
    pub rating: Option<u8>,
    pub label: Option<String>,          // the raw XMP text; mapping to 1–4 is the app's
    pub keywords: Vec<Vec<String>>,     // hierarchical paths; a flat subject is a path of one
}
pub fn read(text: &str) -> Result<Sidecar, XmpError>;
/// Produce the text to write: a fresh packet when `existing` is `None`, otherwise `existing` with
/// only the fields this crate owns replaced. See §5.
pub fn write(sidecar: &Sidecar, existing: Option<&str>) -> Result<String, XmpError>;
```

⚠ `export::metadata::xmp_packet` already writes an XMP packet for *export* metadata (copyright,
caption). It is not reused here: making `export` depend on `xmp`, or the reverse, is a layering
change this wave does not need. Two small packet skeletons is the honest cost; say so in a comment
at both.

**Reading:** `xmp:Rating` and `xmp:Label` appear as **attributes** on `rdf:Description` in
Lightroom's output and as **child elements** in some other writers' — accept both.

---

## 4. When it is read — on scan, only when the file changed

**Schema v6**: one new column, `photos.xmp_mtime INTEGER NOT NULL DEFAULT 0` — the modification
time of the sidecar as last **imported or written** by Rustroom.

On folder open, for every RAW entry: `stat` `stem.xmp`; if it exists and its mtime is **strictly
greater** than the row's `xmp_mtime`, import its rating, label and keywords into the catalog for
copy 0 and record the mtime. That is the whole conflict rule — *any change we have not seen is a
foreign edit and wins* — and it needs no timestamp on the marks themselves, because §5 makes our
own writes update `xmp_mtime` so they never re-import.

- The read is a **new gate**, `if from < 6`, never an edit to 1–5. ⚠⚠ **Both**
  migration-simulation tests (`a_version_one_catalog_upgrades_and_keeps_its_rows` and its sibling)
  carry a drop list that must gain the new column — the v3 and v4 work each found the second list
  only by sabotage.
- One catalog query per folder for the `xmp_mtime`s, the `marks_in` rule — never one per photo.
- The stat is the same per-file cost the "last edited" sort already pays for the JSON sidecar;
  measured there at 4.6 ms for 830 files. Do it on the open path, not in the metadata scanner: a
  4 KB file is not a 29 MB RAW.
- Keywords: walk the path, creating each missing level under its parent (`keywords()` +
  `create_keyword(name, parent)`), then `assign_keyword` the leaf. ⚠ Unique names are per `(parent, name)`, which
  is what makes `Paris` under `France` and under `Texas` two keywords.

---

## 5. When it is written — automatically, merging (user's call)

After every mark change (`apply_mark_to`) and every keyword assign/unassign, for each affected
**RAW** photo, when `Settings.write_xmp` is true (default **true**; a plain `bool`, container-level
serde default already covers the missing key): build the `Sidecar` from the catalog's copy-0 marks
and the file's keywords, write `stem.xmp`, then record its mtime as `xmp_mtime`.

⚠⚠ **NEVER OVERWRITE A SIDECAR SOMEBODY ELSE WROTE — MERGE INTO IT.** A folder edited in Lightroom
already has `stem.xmp` files carrying that person's **entire develop history** (`crs:*`). Replacing
the file with a fresh packet holding a rating would silently destroy it, with nothing to notice
until they reopened the folder in Lightroom. So `write` streams the existing document through
`quick-xml`, drops the four owned fields wherever they appear (attribute or element), injects ours
into the first `rdf:Description`, and passes **everything else through unchanged**. A fresh packet
is written only when no file exists.

**The load-bearing test:** a hand-written Lightroom-shaped fixture with `crs:Exposure2012`,
`crs:ToneCurvePV2012` and a `<crs:Look>` block round-trips with every `crs:` byte intact and only
the rating changed. Sabotage it by writing a fresh packet instead of merging — it must fail.

---

## 6. Tests

`crates/xmp`:
1. Rating and label read from attributes **and** from elements.
2. Hierarchical subjects parse to paths; a flat `dc:subject` becomes a path of one; both forms
   written.
3. The merge preserves foreign content (§5) and replaces owned fields in place.
4. A fresh packet reads back as written (round trip — meaningful only beside 3, alone it is
   self-consistency).
5. Malformed XML is an `Err`, never a panic: the file is hand-editable.

`catalog`: v6 migrates a v5 catalog with rows intact; `xmp_mtime` defaults to 0; both drop lists.

`app`: a RAW with a newer sidecar imports on open and an older one does not (fixture files on disk,
`TempDir`); our own write does not re-import on the next open; a JPEG never gets a sidecar; label
text outside the four is ignored; the label mapping is a total table with a test over
`1..=MAX_LABEL`.

**Done when:** a fixture folder "rated in Lightroom" opens in Rustroom with its stars and keywords;
rating a RAW in Rustroom produces a `stem.xmp` that `exiftool -Rating -Label -Subject` reads back
(run it if installed, and say so either way); and an LR sidecar with develop settings is not
clobbered — checked by diffing the `crs:` lines before and after.

---

## 7. Ownership

**Yours:** new `crates/xmp/` + workspace member, `catalog/src/db.rs` (v6, column, one query, both
migration tests), `app/src/catalog_ops.rs` (write hook), the folder-open path
(`sources.rs`/`open_scan`, the read hook), `settings.rs` (+`write_xmp`), `Cargo.lock`.

**Not yours:** `edit-model`, `pipeline`, anything in `develop/`, `export/` (leave `xmp_packet`
alone), the canvas.

---

## 8. Accepted caveats

1. JPEG/PNG/TIFF marks stay catalog-only; a RAW+JPEG pair rated in Rustroom shows stars in
   Lightroom on the RAW only.
2. Flags never round-trip, in either direction.
3. Copies ≥ 1 never reach the sidecar.
4. With `write_xmp` off the sidecar goes stale and the catalog is authoritative; turning it back on
   does not backfill — the next mark change on each photo does.
5. Develop settings are not mapped (slice c).
6. A sidecar edited by hand to an *older* mtime is not re-imported — the rule is "newer than last
   seen", and that is the honest reading of it.
