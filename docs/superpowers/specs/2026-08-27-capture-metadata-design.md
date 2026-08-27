# Capture metadata — M4.3b

**Date:** 2026-08-27
**Status:** plan 1 built 2026-08-27; plan 2 (filtering) outstanding
**Slice of:** M4.3 (see `2026-08-27-windowed-browsing-design.md` for the four-way decomposition)

## What this adds

Date taken, camera, lens, ISO, shutter, aperture and focal length — read from every photo, stored
in the catalog, and filterable. M4.3a made the whole library browsable and narrowable by what the
catalog already knew; this is the first slice that puts *new* facts into it.

Not in this slice: **smart collections** (M4.3c), which closes M4.3.

## Two plans

- **Plan 1 — capture (invisible).** The dependency, two readers, schema v4, the background pass.
  Like M4.1's plan 1: *if you can tell it shipped, something is wrong* — except the catalog fills.
- **Plan 2 — filtering.** `LibraryFilter` gains the criteria; the popup gains controls.

---

## Measured first, and it decided the architecture

`cargo run --release -p raw-decode --example bench_metadata ~/Desktop/fuji 40`:

| | per file | 830 files |
|---|---|---|
| read the whole file | 11–15 ms | ~10–13 s |
| **libraw parse the header** | **0.2 ms** | 0.2 s |

⚠⚠ **Reading is 98% of the cost and the parse is free**, because `RawImage::open` takes a `&[u8]`
and a RAF is ~29 MB. That rules out doing this inline on folder open, which today is instant plus
4.6 ms of sidecar reads.

⚠ **A prefix read was tested and rejected, and the way it failed is the reason.** At 64 KB, 256 KB
and 1 MB libraw opens **nothing**; at 4 MB it opens **20 of 40**. A truncated read would work for
part of a library and silently miss the rest — the worst failure shape, and invisible until someone
noticed a date filter quietly omitting photos.

### Consequence: a background pass, its own, once per folder

`ThumbLoader`'s worker shape with its own queue — the files in the current source that have no
capture row yet. Results drain per frame and are written to the catalog.

⚠⚠ **It must NOT piggyback on the thumbnail loader**, even though that already reads every file,
because M4.3a-i made thumbnails **windowed**. Metadata attached to them would exist only for photos
that had been scrolled past, and *you cannot filter by a date that was never read*. Accepted cost:
a file is read twice on a first visit.

⚠ **Read once, never re-read.** A photograph's exposure does not change, so there is no
invalidation rule and no mtime comparison. A genuinely replaced file keeps its old metadata until
the folder is forgotten.

⚠ **The backfill needs no migration step.** The 860 existing rows have no capture row, so they are
simply what the pass finds first.

---

## Three findings from probing the readers

Each was found by running both readers against **the same shot**, `DSCF6223.RAF` and
`DSCF6223.JPG`. None was predictable from the documentation.

### ⚠⚠ 1. The two readers disagree about the camera's name

| | RAW (`rsraw`) | JPEG (`kamadak-exif`) |
|---|---|---|
| make | `"Fujifilm"` | `"FUJIFILM"` |
| model | `"X-T4"` | `"X-T4"` |

A RAW+JPEG pair from one body would appear as **two cameras** in a filter. This is the same class
of problem colour-management C4 solved for profile matching, where CLAUDE.md records *"libraw
reports `"Fujifilm X-T4"` where the profile is `"FUJIFILM X-T4"`, so equality finds nothing"* —
except here it is our own two readers disagreeing with each other.

**Decision: normalize at capture — `camera` is stored as `"{MAKE} {model}"` with the make
uppercased.** Deterministic, needs no heuristic and no alias table, both readers agree by
construction, and uppercase is what most cameras write in EXIF and what the body prints on itself.

Rejected: comparing case-insensitively while storing what was read, which is non-deterministic
about which spelling a dropdown shows; and a case heuristic, which is a guess we cannot test
against bodies nobody here owns.

### ⚠⚠ 2. `LensModel` comes back padded with 51 empty strings

`display_value()` yields `"XF23mmF2 R WR", "", "", "", …` — fifty-one of them. Storing that is what
a naïve implementation does. **The lens must be read from `Field::value` and taken as the first
non-empty entry.**

### ⚠⚠ 3. RAW carries a timezone and JPEG does not

`rsraw::datetime()` returns `2026-05-17T19:26:41+02:00`; EXIF's `DateTimeOriginal` is
`2026-05-17 19:26:41` with no zone at all.

**Decision: store the local wall-clock, as naive seconds, for both.** Take `.naive_local()` from
the RAW's `DateTime<Local>` and read the JPEG's string as it stands.

Two reasons. **The two readers then agree by construction** on a RAW+JPEG pair — with true UTC the
RAW would be exact and the JPEG a guess about the machine's zone, and the pair could differ by
hours. And **wall-clock is what a photographer means by "date taken"**: "the shots from the 17th"
means the 17th where they were standing, not in UTC.

⚠ The cost, stated: sorting photos from two timezones interleaves by local clock rather than by
true instant. For a date *filter* that is the desired behaviour; for a strict timeline it is not.

---

## Plan 1 — capture

### 1. One new dependency: `kamadak-exif` 0.6

Pure Rust, no C library, **no `pkg-config` entry and no CI change** — the property that made the
pure-Rust `lensfun` acceptable where the C library was not. It pulls in one transitive crate
(`mutate_once`).

⚠ **RAW needs nothing new.** `rsraw` already exposes `datetime`, `normalized_make`,
`normalized_model`, `lens_info`, `iso_speed`, `shutter`, `aperture` and `focal_len`, and `chrono`
is already in the tree through it.

### 2. `Capture` lives in `core-image`

Both decoders produce it, so it goes exactly where `Thumbnail` went, for the reason `Thumbnail`
went there.

```rust
pub struct Capture {
    pub taken: Option<i64>,       // naive local wall-clock, unix seconds
    pub camera: Option<String>,   // "{MAKE} {model}"
    pub lens: Option<String>,
    pub iso: Option<u32>,
    pub shutter: Option<f32>,     // seconds
    pub aperture: Option<f32>,
    pub focal_len: Option<f32>,   // mm
}
```

⚠ **Every field is `Option`, because a file with no EXIF is normal, not an error** — a scan, an
export or a screenshot strips it. A `Capture` with every field `None` is still a valid answer and
must be *stored*, or the pass would re-read that file on every folder open forever.

⚠ **`catalog` gains a dependency on `core-image`**, whose own only dependency is `rayon`. The
alternatives were an eight-parameter `set_capture` (which trips `too_many_arguments`) or a
duplicate row type; a dependency on one small first-party crate is the cheapest of the three.

### 3. Schema v4 — `photo_capture`, keyed on `(folder_id, file_name)`

⚠⚠ **Per *file*, like `photo_keywords` — not per copy like marks.** Capture metadata describes
*the exposure*, which every copy of a file shares; a rating describes *the image you made*, which
they do not. CLAUDE.md states that split for keywords and notes both tables carry a comment
explaining it because it looks like an oversight. This is the third table to follow it, and it
carries the same comment.

⚠ It also makes drift unrepresentable: per-copy storage would need a rule for what a new copy
inherits, and rules can disagree.

⚠ **A new gate (`if from < 4`), never an edit to versions 1–3.** A migration that has already run
on someone's machine cannot be changed — the rule v2 and v3 each recorded.

⚠ **`a_version_one_catalog_upgrades_and_keeps_its_rows` must learn about v4's table.** CLAUDE.md
records that every new migration makes the older downgrade-simulation test incomplete: it fakes an
old catalog by *undoing* what later versions added, so a new table missing from its drop list makes
`migrate` re-run a step whose objects still exist. The failure is loud, but only because that test
exists.

### 4. `MetadataScanner` in `app`

Modelled on `ThumbLoader`: injected reader fn for testing, worker count from the same headroom
rule, results drained per frame. Its queue is *not* windowed — see above.

⚠ Writes land in the catalog as they arrive rather than in one batch at the end, so a folder closed
half way keeps what was read.

### 5. Kept probes

`raw-decode/examples/bench_metadata.rs` (the measurement above, including the prefix-read test) and
`image-decode/examples/exif_probe.rs` / `raw-decode/examples/raw_meta.rs`, which document what each
reader actually returns for one real file. They are how the three findings above were found and how
a future reader checks them again.

---

## Plan 2 — filtering, sketched only

`LibraryFilter` gains a date range and a camera set. ⚠ **egui has no date picker**, so the date UI
is genuinely open — `YYYY-MM-DD` text fields, relative presets ("this year", "last 30 days"), or
both. Settled when the data is in the catalog to look at, not before.

---

## Testing

**Readers** — gated on the real fixtures (`RAW_FIXTURE`, and a JPEG beside it), since neither
reader can be exercised without a real file. ⚠ A gated test **skips** without its fixture, so a
green CI run does not mean it ran; that is stated rather than hidden.

**Pure and testable without fixtures:** the camera normalization (`"Fujifilm"` and `"FUJIFILM"`
must produce the same string), the lens de-padding (given a padded value, one clean string), and
the wall-clock conversion.

**Catalog** — round trip; a `Capture` with every field `None` still marks the file as read; the v4
migration keeps existing rows; `forget_folder` cascades to `photo_capture`.

**Scanner** — with an injected reader: every queued file is read exactly once; a file already in
the catalog is not re-read; results reach the catalog as they arrive.

### Sabotages to run

- store the make as read → the two-cameras-for-one-body test fails
- use `display_value()` for the lens → the de-padding test fails
- use the RAW's UTC instant rather than its wall-clock → the RAW/JPEG agreement test fails
- skip storing an all-`None` capture → the "not re-read" test fails
- drop `photo_capture` from the v1 test's drop list → that test fails

⚠ The check for whether a sabotage compiled is *"did a `test result:` line appear at all"*, not a
grep for `^error:` — that matches cargo's ordinary test-failure line and produced a false "did not
compile" three times two slices ago.

## Verified on the real catalog (2026-08-27)

Schema v4 migrated the author's live catalog with **every row preserved** — folders, photos,
collections, keywords, keyword assignments and collection memberships all matched a
pre-migration backup exactly.

A background scan of the 832-file folder produced:

- **832 capture rows of 832 files**, 830 with a date and a camera.
- **`SELECT DISTINCT camera` returns exactly one body** — `FUJIFILM X-T4` — across RAW *and* JPEG.
  That is finding 1's normalization holding at scale, not just on the probe pair.
- **415 of 415 RAW+JPEG pairs agree on every field** (date, camera, lens, ISO), from two entirely
  different readers.
- **Two rows are entirely empty**, both exported JPEGs carrying no EXIF — stored rather than
  skipped, which is exactly what stops them being re-read forever.
- Re-opening the folder queues **nothing**.

## Accepted caveats

1. **~10 s of background work the first time a folder is opened**, then never again.
2. **A file is read twice on a first visit** — once by the thumbnail loader, once by the scanner.
   The price of the two having different scopes.
3. **No re-read**, so a replaced file keeps its old metadata until the folder is forgotten.
4. **Wall-clock, not true instants** — see finding 3.
5. **Camera and lens are free text** with no alias table, beyond uppercasing the make. The same
   decision C4 recorded for profile matching.
6. **GPS is available from `rsraw` and deliberately not captured** — it needs a map to be worth
   anything.
7. **PNG and TIFF get whatever EXIF they carry, which is usually none.** They are not camera
   files; nothing special is done for them.
