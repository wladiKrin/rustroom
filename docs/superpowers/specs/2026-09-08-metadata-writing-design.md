# Metadata writing — design

Date: 2026-09-08
Status: **built 2026-09-08.** Plan `2026-09-08-metadata-writing.md`.

**M6.2**, the last open item of **Phase 6 — Delivery & interchange**. With M6.5c already merged,
this closes the phase.

## What it is

Per-photo **title, caption, copyright and creator**, edited in the app, stored in the catalog, and
written into every exported file — together with the rating, label, keywords and capture facts the
catalog already holds — so a deliverable leaves with its credit and its camera data, and the
original is never opened for writing.

The roadmap's "Done when": *a copyright and caption set in the app appear in `exiftool` output on
the export and nowhere near the original.*

## What already exists, and what this is not

⚠⚠ **M6.1b already writes copyright, creator and caption into exports** — as an XMP packet, from
three strings on the export preset (`ExportMetadata`, in `export::metadata`). Those describe the
*run*: sixty files leave with one caption. Nothing per photo is stored anywhere, and an export
carries **no** camera, lens, exposure, keyword or rating at all. That is the gap, not the packet
mechanics, which stay as they are.

⚠ **The design conflict `docs/BACKLOG.md` recorded is answered and is not reopened here.** Nothing
in this design writes to an original. The two destinations are the export file and the `stem.xmp`
sidecar M6.3 already writes behind a setting.

**Scope decided with the user (2026-09-08):** the wide option — core fields, the full packet at
export with an "All metadata / Copyright only" choice, and the sidecar round trip. **Per file**,
not per copy. **Default scope: All metadata**, including for existing presets.

## The design

### 1. Catalog: a `Description` per FILE, schema v7

A new table, keyed like `photo_capture` and `photo_keywords`:

```sql
CREATE TABLE photo_description (
    folder_id INTEGER NOT NULL REFERENCES folders(id) ON DELETE CASCADE,
    file_name TEXT NOT NULL,
    title     TEXT NOT NULL DEFAULT '',
    caption   TEXT NOT NULL DEFAULT '',
    copyright TEXT NOT NULL DEFAULT '',
    creator   TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (folder_id, file_name)
);
```

⚠⚠ **Per file, and that is a decision, not an omission (user's call).** A title says what is *in*
the picture, which every copy shares — the rule keywords already follow. Only marks describe *the
image you made*. Lightroom gives virtual copies independent metadata; this is a recorded
divergence, to be added to `01-lightroom-capabilities.md`'s list.

⚠ **A new `from < 7` gate, never an edit to 1–6.** Both migration-simulation tests
(`a_version_one_catalog_upgrades_and_keeps_its_rows` and its sibling) carry a drop list that must
gain `DROP TABLE photo_description` — the v3, v4 and v6 work each found the second list only by
sabotage.

```rust
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Description {
    pub title: String,
    pub caption: String,
    pub copyright: String,
    pub creator: String,
}
impl Description {
    /// Trimmed — a field holding only spaces counts as absent, as `ExportMetadata::is_empty`.
    pub fn is_empty(&self) -> bool;
}

impl Catalog {
    /// `None` for a photo with no row — which is the ordinary state.
    pub fn description_of(&self, photo: &Path) -> Result<Option<Description>, CatalogError>;
    /// Upsert. ⚠ An all-empty description DELETES the row, so "absent" and "blank" are one
    /// state and the table holds only photos that say something.
    pub fn set_description(&self, photo: &Path, d: &Description) -> Result<(), CatalogError>;
}
```

⚠ **No per-folder map on `Library`, deliberately.** `captures` is held in memory because the
filter bar reads it every frame. Nothing reads descriptions per frame: the Information dialog reads
one row when it opens, and the export reads one per selected photo when the run starts —
`keyword_paths` already queries per photo there. A `descriptions_in` accessor and a
`reload_descriptions` pass would be machinery for a consumer that does not exist.

### 2. Editor: the Information dialog gains a Description block

`modals/info.rs`. Below Capture, a **Description** block with four fields — Title, Caption,
Copyright, Creator — editable, single photo. Caption is multi-line; the other three are single-line.

**The draft lives in the modal.** `Modal::PhotoInfo` gains a `description: Option<DescriptionDraft>`,
filled on the first frame the way `dims` is (⚠ a whole-file or catalog read in a dialog body that
runs sixty times a second is the `apply_view_demosaic` mistake). The draft type is a lib module —
`crates/app/src/description.rs`, no egui — holding `original` and `edited`:

```rust
pub struct DescriptionDraft { pub original: Description, pub edited: Description }
impl DescriptionDraft {
    pub fn changed(&self) -> bool;
}
/// Write `draft.edited` if it differs from `original`. Returns whether anything was written.
pub fn commit(cat: &Catalog, photo: &Path, draft: &DescriptionDraft) -> bool;
```

**Commit on close**, whichever way the dialog closes. ⚠⚠ **Only when changed**, and the reason is
the sidecar: `apply_mark_to` records that *"rewriting the same bytes moves the modification time,
which is the one value the whole conflict rule turns on."* A dialog opened to look and closed
unchanged must write nothing. After a changed commit the dialog calls `write_xmp_for(&[path])`,
the one funnel every mark and keyword change already uses, so the setting check and the RAW-only
rule cannot be forgotten here.

⚠ The block carries the load-bearing hint, in the words the export dialog already uses: *"Shared by
every copy of this file. Written into exports and, for RAW files, the XMP sidecar. The original is
never modified."* A field marked "Copyright" is exactly where someone expects the promise to break.

**Not in scope:** applying a description to a whole selection (the export dialog's copyright and
creator remain the batch route), undo (marks and keywords have none either), and filtering by
caption text.

### 3. Export: a per-file payload and a scope

**`ExportMetadata` gains one field**, container-level `#[serde(default)]` already carrying it for
every stored preset:

```rust
pub enum MetadataScope { #[default] All, CopyrightOnly }
pub struct ExportMetadata { pub copyright, pub creator, pub caption: String, pub scope: MetadataScope }
```

⚠⚠ **`All` is the default, for existing presets too (user's call).** This *does* change the bytes
of an existing user's next export — it gains camera, exposure, keywords and rating — which is the
point of the milestone, and what every other tool does by default. It is the one deliberate
exception to the M6.1 seam's "defaults to do nothing" rule, and this paragraph is where that is
recorded. The dialog offers the two as a radio pair under the three fields; the section header's
"doing something" dot shows when a field is filled **or the scope is not the default** — `All`
is the default and therefore not hidden state, which is the rule
`a_section_that_is_doing_something_says_so_in_its_header` guards. *(Corrected while planning.)*

**The three preset strings become defaults that a photo's own value overrides.** A photo's
non-blank title, caption, copyright or creator wins; a blank one falls back to the preset's. The
preset has no title field and gains none — a title is per photo by nature.

**`ExportJob` carries what the worker cannot query.** It already carries `capture` *"because the
worker runs on its own thread and has no catalog connection"*; it gains `description`, the copy's
`marks` and the file's `keywords` (paths, from `xmp_sync::keyword_paths`), all gathered in
`export_jobs` on the UI thread. Single-file mode has no catalog and gets defaults, as it does for
`capture`.

**A per-file payload in the `export` crate**, which has no catalog dependency and keeps none:

```rust
pub struct FileMetadata {
    pub title: String, pub caption: String, pub copyright: String, pub creator: String,
    pub rating: u8,                    // 0 = absent
    pub label: Option<String>,         // already mapped to XMP text by the app
    pub keywords: Vec<Vec<String>>,    // paths, root first
    pub capture: core_image::Capture,
}
impl FileMetadata {
    /// `ExportMetadata`'s defaults under the photo's own values, then cut to `scope`.
    pub fn resolve(opts: &ExportMetadata, photo: FileMetadata) -> FileMetadata;
}
pub fn xmp_packet(meta: &FileMetadata) -> Option<String>;
pub fn embed(path: &Path, format: ExportFormat, meta: &FileMetadata) -> Result<(), ExportError>;
```

`resolve` under `CopyrightOnly` keeps copyright and creator and clears everything else, so the
packet writer has one rule — write what is present — and the scope is applied in exactly one
place. `xmp_packet` returns `None` when nothing is present, so an export of an EXIF-less,
uncatalogued file with empty fields stays byte-identical, as today.

**What `All` writes**, every property the Dublin Core, XMP, Lightroom, TIFF, EXIF and `aux`
namespaces already give a name — nothing Rustroom-specific:

| catalog | XMP | form |
|---|---|---|
| title | `dc:title` | `rdf:Alt`, `x-default` |
| caption | `dc:description` | `rdf:Alt`, `x-default` |
| copyright | `dc:rights` | `rdf:Alt`, `x-default` |
| creator | `dc:creator` | `rdf:Seq` of one |
| rating, when > 0 | `xmp:Rating` | text |
| label, when any | `xmp:Label` | the same four words the sidecar writes |
| keywords | `dc:subject` **and** `lr:hierarchicalSubject` | `rdf:Bag`; leaves, and `Europe\|France\|Paris` — Lightroom's own two forms, as the sidecar writes them |
| camera | `tiff:Model` | text |
| lens | `aux:Lens` | text |
| taken | `exif:DateTimeOriginal` | `YYYY-MM-DDThh:mm:ss`, no zone — the value is naive wall-clock by `Capture::taken`'s own rule |
| iso | `exif:ISOSpeedRatings` | `rdf:Seq` of one |
| shutter | `exif:ExposureTime` | rational: `1/N` below a second, `N/10` from one up |
| aperture | `exif:FNumber` | rational `N/10` |
| focal_len | `exif:FocalLength` | rational `N/10` |

⚠ **`tiff:Model` carries the whole stored camera name and there is no `tiff:Make`.** The catalog
holds one normalised `"{MAKE} {model}"` string, by the rule `Capture::camera_name` documents,
and the make cannot be split back off it soundly (`NIKON CORPORATION NIKON Z 6`). Writing the
name whole into Model is honest; inventing a Make is not. Recorded as an accepted caveat.

⚠ **The rationals are `N/10`, not reduced.** exiftool renders `56/10` as `5.6` and `350/10` as
`35.0 mm`; a reduced fraction would be prettier in the XML and identical everywhere anyone looks.
A shutter below a second is `1/round(1/secs)` because `1/250` is what a photographer wrote, and
`4/1000` is not.

⚠ `chrono` joins the `export` crate's dependencies for the date — already in the tree via `app`,
pure Rust. A hand-rolled civil-date conversion is a dozen lines and a thing to get silently wrong,
the CRC-32 argument again.

⚠ **Two XMP packet skeletons remain**, `export::metadata::xmp_packet` and `xmp::write`, and the
M6.3 note at both stands: one writes a fresh document for a file the encoder just produced, the
other merges into a document Lightroom may own. The keyword bag formatting is now in both; it is
six lines, and sharing it would make `export` depend on `xmp` for six lines.

### 4. Sidecar round trip: the four fields join the `xmp` crate's owned set

`xmp::Sidecar` gains `title`, `caption`, `rights`, `creator: Option<String>`; `Owned` gains four
variants matched **by namespace** like the rest: `dc:title`, `dc:description`, `dc:rights`,
`dc:creator`. Reading accepts the element form (the first `rdf:li` of the `Alt`/`Seq`) and the
attribute form. Writing emits the same `Alt`/`Seq` shapes §3 does. Merging drops the four wherever
they appear and injects ours — the existing rule, four more names.

⚠ **A foreign `dc:creator` with several entries reads as its first and is written back as one.**
The catalog holds one creator string; a co-credit is not a field Rustroom has. Accepted caveat,
noted at the reader.

`xmp_sync::sidecar_of` fills the four from `description_of`. `xmp_sync::apply` writes them into
the catalog with the rule ratings follow: **a field absent from a newer sidecar clears the
catalog's** — that is what makes deleting a caption in Lightroom reach Rustroom, exactly as
removing a label does. The `write_xmp` setting's documentation gains "or description".

### 5. Testing

⚠⚠ **W5's lesson governs the shape of this suite: the milestone whose maths was most thoroughly
tested shipped 100% non-functional, because nothing proved the data reached the tested code.** So
alongside the unit tests there are two carriers:

- **Catalog row → worker → bytes.** A `batch_export` test stores a caption in a catalog, builds
  jobs through `export_jobs`, runs the worker, and finds the caption inside the exported file's
  packet — the existing `the copyright never reached the file` test, one layer deeper.
- **Dialog → catalog.** A headless test opens `Modal::PhotoInfo` on a catalogued photo, feeds text
  into the Caption field through egui events (the `grid_frame` pattern in `library.rs`), closes
  the dialog, and reads the caption back from the catalog. ⚠ Every M5.3b defect was in dialog and
  panel wiring and none was caught by a test before it was reported.

Unit tests, by crate:

- **catalog**: round trip; two copies of one file share one description; an all-empty description
  removes the row; an unknown photo is `None`; a v6 catalog upgrades and both simulations drop the
  table.
- **xmp**: the four read from elements; a title reads from an attribute; a fresh packet writes them
  and omits unset ones; merging replaces a foreign title and keeps every other byte; a multi-entry
  creator reads its first. **xmp_sync**: a description reaches the sidecar and back; a newer
  sidecar's caption imports; an absent field clears.
- **export**: the photo's field wins over the dialog's, a blank falls back; `CopyrightOnly` writes
  nothing but rights and creator; `All` writes each property once; a shutter below a second is a
  unit fraction and one above is tenths; an EXIF-less capture writes no capture property; keywords
  land in both forms; an empty payload writes no packet. `every_format_carries_the_packet_and_still_decodes`
  runs against the full payload.
- **app**: `commit` writes only when changed; the export dialog's header dot follows the scope.

**Done-when, by hand:** export one photo with a title and caption set, run `exiftool` on the export
and on the original — the fields appear on the first and not the second. ⚠ Not automated:
`exiftool` is on this machine and not on CI, and the packet tests parse the same bytes.

**No golden moves.** Nothing here touches a pixel.

### 6. Files, and the ones already over the line

| file | change |
|---|---|
| `crates/catalog/src/db.rs` | table, `Description`, two accessors, v7 gate, tests. ⚠ Already ~2,200 lines with its tests; say so in the report. |
| `crates/app/src/description.rs` | **new** lib module: `DescriptionDraft`, `commit` |
| `crates/app/src/modals/mod.rs`, `modals/info.rs` | the draft in the modal; the block; commit on close |
| `crates/export/src/metadata.rs` | `MetadataScope`, `FileMetadata`, `resolve`, the packet, `embed`'s signature, `chrono` |
| `crates/app/src/batch_export.rs` | `ExportJob` fields; build the payload before `embed`. ⚠ Already ~1,500 lines; say so. |
| `crates/app/src/export_flow.rs` | gather description, marks, keywords per job |
| `crates/app/src/modals/export.rs` | the scope radio; the header dot |
| `crates/xmp/src/lib.rs` | four owned fields, read and merge |
| `crates/app/src/xmp_sync.rs` | `sidecar_of`, `apply` |
| `crates/app/src/settings.rs` | the `write_xmp` doc sentence |
| `01-lightroom-capabilities.md`, `03-plan-of-action.md`, `docs/BACKLOG.md` | the divergence; M6.2 ✅ and Phase 6 ✅; the backlog row |

**Order:** catalog → export crate → export flow and dialog → xmp crate → xmp_sync → description
module and dialog → docs. Each step green on its own.
