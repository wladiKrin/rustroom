# Export delivery: files, Immich, or both — and which date an upload carries

**Date:** 2026-09-24
**Status:** approved in brainstorming, ready for a plan
**Follows:** `2026-09-24-immich-upload-design.md`, which built upload as an *addition* to an export

---

## 1. Why

Two things the first version got wrong, both found by using it.

**Uploading was never a choice.** `upload_target` made an export write files **and** upload. The user's
words: *"I might not have to carry the exports when they're already in immich."* Delivery is a
three-way choice — files, Immich, or both — not a flag on top of a file write.

**The date was not a choice either, and its fallback was actively wrong.** It used the capture time,
falling back to the source file's mtime — which for an Immich cache file is *when Rustroom downloaded
it*. That shipped a wrong date that looked plausible. ⚠ The root cause (a blank `DateTimeOriginal`
placeholder from an SP-3000 scanner) is already fixed separately; this spec removes the **fallback**
that made it invisible, and makes the date an explicit option because, in the user's words, there are
*"valid arguments for all of these cases"*.

---

## 2. One field, three states

`ExportOptions.upload_target: Option<UploadTarget>` is **replaced** by:

```rust
/// Where an export run delivers what it renders.
///
/// ⚠⚠ **One field rather than a flag beside a folder**, so "upload nothing and keep nothing" is
/// unrepresentable rather than guarded. ⚠ Named for the *role*, never the service: `export` must
/// stay able to lose the Immich integration entirely with "no mention of immich at all".
pub enum Destination {
    /// Write files and keep them. The default, and what every export did before this.
    Files,
    /// Deliver to the target only — no local copy is kept.
    Target(UploadTarget),
    /// Both.
    Both(UploadTarget),
}
```

with helpers `keeps_files(&self) -> bool` and `target(&self) -> Option<UploadTarget>`.

⚠ **One accepted cost, stated rather than shimmed:** an export preset saved with `upload_target` set
comes back as `Files` and must be re-ticked once. That field is a day old and unreleased; carrying a
compatibility shim for it would outlive its usefulness immediately. The container-level
`#[serde(default)]` means such a file still **loads** — only the upload preference is lost, never the
preset.

### `can_export` gains two rules

```rust
pub fn can_export(opts: &ExportOptions, upload_ready: bool) -> bool
```

1. A destination folder is required **only** when `destination.keeps_files()` — an Immich-only run has
   nowhere it must write to.
2. A destination carrying a target requires `upload_ready`. ⚠ Without this, a preset saved as
   "Immich only" would start a run with **nowhere to put anything** after the key was removed.

⚠ The second argument rather than reading `Settings` inside: this function is pure on purpose, which
is what lets a test see "Export goes dark" instead of someone clicking to find out.

---

## 3. Staging, and what happens when an upload does not confirm

| mode | written to | on **confirmed** upload | on failure or unconfirmed |
|---|---|---|---|
| `Files` | `dest_dir` | — | — |
| `Both` | `dest_dir` | kept | kept |
| `Target` | a fresh **temp** dir | **deleted** | **moved to `dest_dir`**; if no usable folder is set, left in place and reported by path |

⚠⚠ **Deleted only on a CONFIRMED upload, never on an attempt.** An SSL/EOF error means the *response*
was lost, not that the server rejected the file — measured on this network during the stacking
migration, 4 writes in 2,373 failed that way and **none had actually failed to apply**. Deleting on
anything less than confirmation would destroy work on a flaky link.

⚠⚠ **This shape was chosen over two simpler ones and its weak point is known: the move-on-failure path
is the one that runs least often.** So it is pinned by a test that forces a failed upload (a closed
port on loopback) and asserts the file arrives in the export folder — not by inspection.

⚠ The temp directory is per **run**, named with the process id **and** a counter (`immich_list_cache`'s
pattern, not `immich_source`'s pid-only one), and removed at the end of the run if empty. A directory
left behind holding files is **not** an error: it means something did not confirm.

⚠ **"Reported by path" means `ExportReport::unconfirmed` carries the file's FINAL resting place**, not
the temp path it was written to — so the one place the user looks names somewhere the file actually is.
When the move itself fails (an unwritable folder), the temp path is reported instead, together with why,
because a path that does not exist is worse than an ugly one.

### Collisions and naming under `Target`

- **No collision prompts.** A fresh temp directory cannot collide with anything, and asking about one
  would be a question with no meaning.
- ⚠ **The in-run `claimed` set stays.** Two sources sharing a stem (`a.RAF`, `a.jpg`) must still not
  overwrite each other inside the staging directory.
- ⚠ **The filename template still matters** — it is the name Immich displays, so it is not
  "just a local detail" under this mode.

---

## 4. Which date an upload carries

```rust
/// Which moment an uploaded photo is dated with.
pub enum UploadDate {
    /// The original's own capture date. The default, and what the first version did.
    Captured,
    /// When this photo's edits were last changed.
    Edited,
    /// When the upload happens.
    Uploaded,
}
```

| variant | source | when that is unavailable |
|---|---|---|
| `Captured` | `Capture::taken` | **the upload time** |
| `Edited` | `sidecar::edited_at` | **the upload time** |
| `Uploaded` | now | — |

⚠⚠ **THE MTIME FALLBACK IS REMOVED, AND THAT IS THE POINT.** `Captured` previously fell back to the
source file's mtime, which for a file in the Immich cache is **when Rustroom downloaded it** — so a
photo whose date could not be read was filed under the download date, a wrong answer that looked
right. Falling back to the **upload time** is honest about ignorance: the photo appears at the top of
the timeline, where it is noticed, and nothing invents a plausible past.

⚠⚠ **`Edited` MUST be read at PLAN time, not at upload time.** `immich_upload::record` writes the
upload into the photo's sidecar, which bumps the sidecar's mtime — and `edited_at` *is* that mtime. Read
late, "the edit time" silently becomes "the time of the last upload", and worse, it would differ
between the first upload and every one after. So `Planned` carries an `edited_at` settled before
anything is written.

⚠ `fileModifiedAt` stays the **exported file's** own mtime under all three options. That genuinely is
when this rendering was made, and it is a different question from which moment the photograph is
filed under.

---

## 5. The dialog

The **Upload** section is renamed **Delivery** and holds both controls:

- the three-way mode, as a segmented control: **Files only / Immich only / Files and Immich**
- the date, as a dropdown: **Original date / Edit date / Upload date**

Its header summarises itself, as every section in this dialog does — e.g. *"Immich only, original
date"*, or *"Files only"* when no target is chosen (the date is then irrelevant and is not mentioned).

⚠⚠ **Both controls live in ONE section deliberately, rather than putting the mode in Location where it
arguably belongs.** The requirement is that switching Immich off leaves *no mention of it anywhere*;
keeping every mention inside one section keeps that a **single condition** instead of a growing list of
places to remember. With Immich unconfigured the section is absent and files-only is simply what
happens, exactly as today.

⚠ Location is **not** greyed out under Immich-only. It is where a file goes if its upload does not
confirm, so it stays usable, and its own summary says so.

⚠ The date dropdown is disabled when the mode is `Files only` — it has nothing to act on — rather than
hidden, which this project has already taken back out once.

---

## 6. Where the code goes

| unit | home |
|---|---|
| `Destination`, `UploadDate`, `can_export` | `batch_export/options.rs` |
| staging dir, delete/move on outcome | `batch_export/worker.rs` |
| date choice → a timestamp | `immich_upload.rs` (lib, no egui) |
| the section's summary | `export_summary.rs` |
| the two controls | `modals/export.rs` (egui only) |

⚠ The date decision belongs in `immich_upload` and **not** in the worker: it is a pure function of
`(UploadDate, Capture, edited_at, now)` and is exactly the sort of thing that must be testable without
opening a window.

---

## 7. Testing

**Without a server, and required:**

1. `Destination::default()` is `Files`; an options file written before it still loads; each variant
   round-trips.
2. `can_export`: no folder needed for `Target`; a folder needed for `Files` and `Both`; **false** for
   any target when `upload_ready` is false.
3. The date choice, as a pure function: each variant picks its source, and ⚠ **`Captured` with no
   capture date yields the upload time, not a file mtime** — the regression this spec exists to
   prevent.
4. ⚠⚠ **A failed upload under `Target` leaves the file in `dest_dir`** — the least-exercised path,
   forced with a closed port on loopback.
5. ⚠ **A confirmed upload under `Target` leaves no file behind**, in the temp dir or the export folder.
   Without this the delete is unverified and "Immich only" quietly keeps everything.
6. `Files` alone still writes exactly what it wrote before, and the **75 goldens stay unmoved**.

**Only a real server can settle, so it must be seen before the milestone closes:**

- That an Immich-only run leaves nothing on disk and the photo is on the server.
- That each date option produces the date it claims, checked against the asset.

---

## 8. Out of scope

- Albums, stacking the upload with its original, `visibility`, `sidecarData`.
- A library-wide "upload everything edited" sweep.
- The OS keychain; the upload key stays plaintext.
- ⛔ Two-way sync — declined, with reasons, in `docs/BACKLOG.md`.

---

## 9. Definition of done

1. All three modes work, and `Files` is unchanged from today with the goldens unmoved.
2. An Immich-only run leaves **no** file behind on success, and leaves the file **findable** on failure.
3. Each of the three date options produces the date it names, verified against the server.
4. With Immich unconfigured, the dialog contains no mention of it and behaves exactly as before.
5. ⚠ Seen working, not merely green — every Immich defect in this project so far was found by use.
