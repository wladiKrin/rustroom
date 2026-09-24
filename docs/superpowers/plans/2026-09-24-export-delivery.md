# Export Delivery Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** An export delivers to files, to Immich, or to both — and an uploaded photo's date is a choice between the original's date, the edit date and the upload date.

**Architecture:** `ExportOptions.upload_target` becomes a single `Destination` enum so "nothing at all" is unrepresentable. Immich-only stages each file in a per-run temp directory, deletes it on a **confirmed** upload and **moves it to the export folder** otherwise. The date becomes a pure function in `immich_upload`, and the mtime fallback — which produced a wrong date that looked right — is removed.

**Tech Stack:** Rust, `serde`, egui (`segmented`, `ComboBox`).

**Spec:** `docs/superpowers/specs/2026-09-24-export-delivery-design.md`

---

## Ground rules

⚠⚠ **ONE CARGO INVOCATION MODE.** Always `cargo test --workspace <name-filter>`, **never** `cargo test -p`. Give clippy its own target dir.

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo fmt --all --check; echo "FMT=$?"
CARGO_TARGET_DIR=target/clippy cargo clippy --workspace --all-targets -- -D warnings > /tmp/c.log 2>&1; echo "CLIPPY=$?"
cargo test --workspace > /tmp/t.log 2>&1; echo "TEST=$?"
```

⚠ **The gate stays separate from the commit** — it caught clippy at 101 twice in this session's earlier work. ⚠ zsh, so no `${PIPESTATUS[0]}`. ⚠ No `Co-Authored-By`. ⚠ **Never push.** Branch `immich-upload`.

⚠ A filter matching nothing **exits 0** — after adding a test, confirm the run says `N passed`, not `0 passed`, and remember the app's test binary is ~11th of 17, so **do not read only the first lines of output**.

---

## File Structure

| file | change |
|---|---|
| `crates/app/src/batch_export/options.rs` | `Destination`, `UploadDate`, `can_export`'s second argument |
| `crates/app/src/immich_upload.rs` | `upload_seconds` — the date choice as a pure function |
| `crates/app/src/batch_export/worker.rs` | staging dir, `Planned.edited_at`, delete-or-move on outcome |
| `crates/app/src/export_summary.rs` | `delivery()` replaces `upload()` |
| `crates/app/src/modals/export.rs` | the Delivery section: segmented mode + date dropdown |
| `crates/app/src/export_flow.rs` | pass the destination's target through |

---

## Task 1: `Destination` and `UploadDate`

**Files:**
- Modify: `crates/app/src/batch_export/options.rs`

- [ ] **Step 1: Write the failing tests**

Replace the three tests added by the previous milestone (`upload_is_off_by_default`,
`an_options_file_from_before_upload_still_loads`, `the_upload_target_round_trips`) with:

```rust
    /// ⚠ The default must keep files and upload nothing, so an existing user's next export is
    /// unchanged.
    #[test]
    fn the_default_destination_is_files_only() {
        let d = ExportOptions::default().destination;
        assert_eq!(d, Destination::Files);
        assert!(d.keeps_files());
        assert_eq!(d.target(), None);
    }

    /// ⚠⚠ "Upload nothing and keep nothing" must be **unrepresentable**, which is the whole reason
    /// this is one field rather than a flag beside a folder. Every variant either keeps files or
    /// carries a target.
    #[test]
    fn every_destination_delivers_somewhere() {
        for d in [
            Destination::Files,
            Destination::Target(UploadTarget::Immich),
            Destination::Both(UploadTarget::Immich),
        ] {
            assert!(
                d.keeps_files() || d.target().is_some(),
                "{d:?} delivers nowhere"
            );
        }
        assert!(!Destination::Target(UploadTarget::Immich).keeps_files());
        assert!(Destination::Both(UploadTarget::Immich).keeps_files());
        assert_eq!(
            Destination::Both(UploadTarget::Immich).target(),
            Some(UploadTarget::Immich)
        );
    }

    #[test]
    fn a_destination_round_trips() {
        for d in [
            Destination::Files,
            Destination::Target(UploadTarget::Immich),
            Destination::Both(UploadTarget::Immich),
        ] {
            let o = ExportOptions {
                destination: d,
                ..ExportOptions::default()
            };
            let back: ExportOptions =
                serde_json::from_str(&serde_json::to_string(&o).unwrap()).unwrap();
            assert_eq!(back.destination, d);
        }
    }

    /// ⚠⚠ An options file written before this field, **and one written with the day-old
    /// `upload_target`**, must both still load. The second loses only the upload preference — never
    /// the preset — which is the cost this replacement accepts rather than shimming.
    #[test]
    fn an_options_file_from_before_the_destination_still_loads() {
        let o: ExportOptions = serde_json::from_str(r#"{"dest_dir":"/tmp","quality":90}"#).unwrap();
        assert_eq!(o.destination, Destination::Files);
        assert_eq!(o.quality, 90);

        let stale: ExportOptions =
            serde_json::from_str(r#"{"quality":77,"upload_target":"Immich"}"#).unwrap();
        assert_eq!(stale.quality, 77, "the rest of the preset must survive");
        assert_eq!(
            stale.destination,
            Destination::Files,
            "the retired field is ignored, not honoured"
        );
    }

    #[test]
    fn the_default_upload_date_is_the_originals() {
        assert_eq!(ExportOptions::default().upload_date, UploadDate::Captured);
    }

    /// ⚠⚠ `can_export` must refuse a target the settings cannot serve, or a preset saved as
    /// "Immich only" starts a run with **nowhere to put anything** once the key is gone.
    #[test]
    fn can_export_needs_a_folder_only_when_files_are_kept() {
        let tmp = std::env::temp_dir();
        let with_dir = |d: Destination| ExportOptions {
            dest_dir: tmp.clone(),
            destination: d,
            ..ExportOptions::default()
        };
        let no_dir = |d: Destination| ExportOptions {
            dest_dir: std::path::PathBuf::new(),
            destination: d,
            ..ExportOptions::default()
        };

        assert!(can_export(&with_dir(Destination::Files), false));
        assert!(!can_export(&no_dir(Destination::Files), false));

        // Immich-only needs no folder at all …
        assert!(can_export(
            &no_dir(Destination::Target(UploadTarget::Immich)),
            true
        ));
        // … but does need uploading to be possible.
        assert!(!can_export(
            &no_dir(Destination::Target(UploadTarget::Immich)),
            false
        ));
        // Both needs the folder *and* the upload.
        assert!(can_export(
            &with_dir(Destination::Both(UploadTarget::Immich)),
            true
        ));
        assert!(!can_export(
            &with_dir(Destination::Both(UploadTarget::Immich)),
            false
        ));
        assert!(!can_export(
            &no_dir(Destination::Both(UploadTarget::Immich)),
            true
        ));
    }
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test --workspace destination
```
Expected: FAIL to compile — `cannot find type Destination`.

- [ ] **Step 3: Implement**

In `crates/app/src/batch_export/options.rs`, **replace** the `upload_target` field with:

```rust
    /// Where this export delivers what it renders.
    ///
    /// ⚠⚠ **One field rather than a flag beside a folder**, so "keep nothing and upload nothing" is
    /// **unrepresentable** instead of guarded. ⚠ Named for the *role*, never the service: `export`
    /// must stay able to lose the Immich integration with "no mention of immich at all".
    ///
    /// ⚠ This replaced a day-old `upload_target: Option<UploadTarget>`. A preset written with that
    /// field still loads — the container-level `#[serde(default)]` sees to it — but comes back as
    /// `Files`, so the upload preference is re-ticked once. Pinned by
    /// `an_options_file_from_before_the_destination_still_loads`.
    pub destination: Destination,
    /// Which moment an uploaded photo is dated with. Ignored when nothing is uploaded.
    pub upload_date: UploadDate,
```

Replace `upload_target: None,` in `Default` with:

```rust
            destination: Destination::Files,
            upload_date: UploadDate::Captured,
```

Replace the `UploadTarget` enum block with:

```rust
/// A destination an export can be delivered to besides the filesystem.
///
/// ⚠ One variant today, and that is not a placeholder — it is the only target that exists.
/// ⚠⚠ **Append variants, never insert**: this is serialised into settings and export presets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UploadTarget {
    Immich,
}

/// Where an export run delivers what it renders.
///
/// ⚠⚠ **Append variants, never insert** — serialised into presets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Destination {
    /// Write files and keep them. The default, and what every export did before this.
    #[default]
    Files,
    /// Deliver to the target only — **no local copy is kept**.
    Target(UploadTarget),
    /// Both.
    Both(UploadTarget),
}

impl Destination {
    /// Whether a file survives the run, which is what decides if a folder is required.
    pub fn keeps_files(self) -> bool {
        matches!(self, Destination::Files | Destination::Both(_))
    }

    /// The upload target, when there is one.
    pub fn target(self) -> Option<UploadTarget> {
        match self {
            Destination::Files => None,
            Destination::Target(t) | Destination::Both(t) => Some(t),
        }
    }
}

/// Which moment an uploaded photo is dated with.
///
/// ⚠ The user's framing: there are *"valid arguments for all of these cases"*, so it is an option
/// rather than a rule. ⚠⚠ **Append variants, never insert.**
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum UploadDate {
    /// The original photograph's own capture date. The default.
    #[default]
    Captured,
    /// When this photo's edits were last changed.
    Edited,
    /// When the upload happens.
    Uploaded,
}

impl UploadDate {
    pub const ALL: [UploadDate; 3] = [
        UploadDate::Captured,
        UploadDate::Edited,
        UploadDate::Uploaded,
    ];

    pub fn label(self) -> &'static str {
        match self {
            UploadDate::Captured => "Original date",
            UploadDate::Edited => "Edit date",
            UploadDate::Uploaded => "Upload date",
        }
    }
}
```

Replace `can_export` with:

```rust
/// Whether a run can start at all.
///
/// ⚠ A pure function rather than a condition inside the egui closure — that is what makes "Export
/// goes dark" something a test can see rather than something to verify by clicking. `upload_ready`
/// is passed in for the same reason: reading `Settings` here would make it impure.
pub fn can_export(opts: &ExportOptions, upload_ready: bool) -> bool {
    let folder_ok = !opts.destination.keeps_files() || destination_ready(&opts.dest_dir);
    // ⚠⚠ A destination naming a target it cannot reach must not start: a preset saved as
    // "Immich only" would otherwise run with nowhere to put anything once the key was removed.
    let target_ok = opts.destination.target().is_none() || upload_ready;
    folder_ok && target_ok && Template::parse(&opts.filename_template).is_ok()
}
```

- [ ] **Step 4: Fix every call site the compiler names**

```bash
cargo test --workspace --no-run 2>&1 | grep -E '^error|--> ' | head -20
```
Expected: errors at `worker.rs` (four `can_export` calls in tests) and `modals/export.rs:185`.
For the **worker tests**, pass `true` — they are about folders and templates, not credentials.
For `modals/export.rs:185`, Task 5 replaces that line; for now pass
`upload_avail == rustroom_app::immich_upload::Availability::Ready`.

- [ ] **Step 5: Run the tests**

```bash
cargo test --workspace destination
```
Expected: the four destination tests pass. Then:
```bash
cargo test --workspace can_export_needs_a_folder
```
Expected: `1 passed`.

- [ ] **Step 6: Commit**

```bash
git add crates/app/src/batch_export/options.rs crates/app/src/batch_export/worker.rs crates/app/src/modals/export.rs
git commit -m "feat(export): one Destination field so 'deliver nowhere' is unrepresentable"
```

---

## Task 2: The date choice, as a pure function

**Files:**
- Modify: `crates/app/src/immich_upload.rs`

- [ ] **Step 1: Write the failing test**

Append to the `#[cfg(test)] mod tests` block in `crates/app/src/immich_upload.rs`:

```rust
    /// ⚠⚠ **`Captured` WITH NO CAPTURE DATE MUST YIELD THE UPLOAD TIME, NOT A FILE MTIME.** The
    /// removed fallback used the source file's mtime, which for a file in the Immich cache is *when
    /// Rustroom downloaded it* — so a photo whose date could not be read was filed under the
    /// download date, a wrong answer that looked right. This is the regression guard for that.
    #[test]
    fn each_date_option_picks_its_own_source() {
        let now = 1_800_000_000;
        let taken = Some(1_763_559_144); // 2025-11-19T13:32:24
        let edited = Some(1_790_000_000);

        assert_eq!(upload_seconds(UploadDate::Captured, taken, edited, now), 1_763_559_144);
        assert_eq!(upload_seconds(UploadDate::Edited, taken, edited, now), 1_790_000_000);
        assert_eq!(upload_seconds(UploadDate::Uploaded, taken, edited, now), now);

        // ⚠ The absent cases fall to `now` — never to a plausible-looking past.
        assert_eq!(upload_seconds(UploadDate::Captured, None, edited, now), now);
        assert_eq!(upload_seconds(UploadDate::Edited, taken, None, now), now);
    }

    /// ⚠ The discriminating negative: `Captured` must not quietly use the edit date when the
    /// capture date is missing. Two absent-value fallbacks that both pointed at each other would
    /// make the options indistinguishable.
    #[test]
    fn a_missing_capture_date_does_not_borrow_the_edit_date() {
        let now = 1_800_000_000;
        let got = upload_seconds(UploadDate::Captured, None, Some(1_790_000_000), now);
        assert_eq!(got, now, "Captured fell back to the edit date");
    }
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test --workspace each_date_option
```
Expected: FAIL to compile — `cannot find function upload_seconds`.

- [ ] **Step 3: Implement**

In `crates/app/src/immich_upload.rs`, **replace** `created_at_text` with:

```rust
/// Which second an uploaded photo is dated with.
///
/// ⚠⚠ **A pure function of four values, and deliberately so** — this is the whole date decision,
/// and it must be checkable without a window, a server or a file on disk.
///
/// ⚠⚠ **There is no file-mtime fallback any more, and its removal is the point.** `Captured` used
/// to fall back to the source file's mtime; for a file in the Immich cache that is **when Rustroom
/// downloaded it**, so a photo whose date could not be read was filed under the download date — a
/// wrong answer that looked plausible, which is the worst kind. An absent value now yields `now`:
/// the photo lands at the top of the timeline where it is noticed, and nothing invents a past.
pub fn upload_seconds(
    which: UploadDate,
    taken: Option<i64>,
    edited: Option<i64>,
    now: i64,
) -> i64 {
    match which {
        UploadDate::Captured => taken.unwrap_or(now),
        UploadDate::Edited => edited.unwrap_or(now),
        UploadDate::Uploaded => now,
    }
}

/// [`upload_seconds`] as the string Immich's `fileCreatedAt` wants.
///
/// ⚠⚠ Naive wall-clock rendered as if UTC, which is **measured rather than chosen**: Immich did
/// exactly this to the sibling JPEG of a real RAW+JPEG pair, turning EXIF `20:41:34` into
/// `fileCreatedAt: 2026-08-12T20:41:34.000Z`. Matching it is what puts an uploaded edit at the same
/// `fileCreatedAt` as its original.
pub fn created_at_text(which: UploadDate, taken: Option<i64>, edited: Option<i64>) -> String {
    utc_text(upload_seconds(which, taken, edited, now_secs()))
}
```

Add the import at the top of the file:

```rust
use crate::batch_export::UploadDate;
```

Add to the tests module's imports if needed — `use super::*;` already covers it.

⚠ **Delete the now-unused `mtime_secs` call in `created_at_text` only** — `mtime_secs` itself is still
used by `modified_at_text`, so leave the helper in place.

- [ ] **Step 4: Update the two tests that called the old signature**

The existing `the_created_date_is_the_capture_time_not_now`,
`a_photo_with_no_exif_date_falls_back_to_the_file_mtime` and
`with_no_date_and_no_file_the_result_is_the_epoch` all call `created_at_text(&capture, &path)`.
⚠⚠ **The second and third describe behaviour this task deliberately removes**, so delete them and
replace with one that pins the new rule:

```rust
    /// ⚠ The formatted form, so the wire value is pinned and not just the seconds.
    #[test]
    fn the_created_text_formats_the_chosen_second() {
        let taken = Some(1_787_824_800); // 2026-08-27T10:00:00 wall-clock
        assert_eq!(
            created_at_text(UploadDate::Captured, taken, None),
            "2026-08-27T10:00:00.000Z"
        );
        // ⚠ No capture date means *now*, so the only safe assertion is that it is not the epoch and
        // not the old mtime behaviour — the exact second is the clock's.
        let got = created_at_text(UploadDate::Captured, None, None);
        assert_ne!(got, "1970-01-01T00:00:00.000Z");
        assert!(got.ends_with('Z') && got.len() == 24, "shape: {got}");
    }
```

- [ ] **Step 5: Run the tests**

```bash
cargo test --workspace date_option
```
Expected: `1 passed`. Then:
```bash
cargo test --workspace created_text
```
Expected: `1 passed`.

- [ ] **Step 6: Commit**

```bash
git add crates/app/src/immich_upload.rs
git commit -m "feat(upload): the date is a choice, and the misleading mtime fallback is gone"
```

---

## Task 3: Staging, and delete-or-move on the outcome

**Files:**
- Modify: `crates/app/src/batch_export/worker.rs`

- [ ] **Step 1: Write the failing tests**

Append to the tests in `crates/app/src/batch_export/worker.rs`:

```rust
    /// ⚠⚠ **THE LEAST-EXERCISED PATH, AND THE REASON THIS SHAPE WAS CHOSEN OVER TWO SIMPLER ONES.**
    /// Immich-only stages in a temp directory; when the upload does not confirm the file must end up
    /// in the export folder, findable, rather than swept with the temp directory. Forced with a
    /// closed port on loopback, which refuses at once.
    #[test]
    fn an_unconfirmed_upload_moves_the_staged_file_into_the_export_folder() {
        let d = TempDir::new("stage_fail");
        let mut o = opts(&d, None);
        o.destination = Destination::Target(UploadTarget::Immich);
        let worker = ExportWorker::spawn(
            jobs(&d, &["a.RAF"]),
            o,
            ok_decode(),
            Arc::new(AtomicBool::new(false)),
            CPU,
            1,
            None,
            Some(crate::immich_upload::UploadConfig {
                base_url: "http://127.0.0.1:1".to_string(),
                key: "irrelevant".to_string(),
            }),
        );
        let report = finished(&worker);
        assert_eq!(report.exported, 1, "the render and write still succeeded");
        assert_eq!(report.uploaded, 0);
        assert_eq!(report.unconfirmed.len(), 1);
        let landed = d.0.join("a.jpg");
        assert!(
            landed.exists(),
            "the staged file must be moved where the user can find it; dir held {:?}",
            std::fs::read_dir(&d.0).unwrap().flatten().map(|e| e.path()).collect::<Vec<_>>()
        );
        // ⚠ And the report must name where it actually is, not the temp path it was written to.
        assert_eq!(report.unconfirmed[0].0, landed);
    }

    /// ⚠ The control for the delete half: with no target at all, `Files` keeps its file exactly
    /// where it always did. Without this the move above could pass against a mode that never
    /// staged anything.
    #[test]
    fn files_only_keeps_the_file_in_the_export_folder() {
        let d = TempDir::new("files_only");
        let o = opts(&d, None);
        assert_eq!(o.destination, Destination::Files);
        let worker = ExportWorker::spawn(
            jobs(&d, &["a.RAF"]),
            o,
            ok_decode(),
            Arc::new(AtomicBool::new(false)),
            CPU,
            1,
            None,
            None,
        );
        let report = finished(&worker);
        assert_eq!(report.exported, 1);
        assert!(d.0.join("a.jpg").exists());
        assert!(report.unconfirmed.is_empty());
    }
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test --workspace moves_the_staged_file
```
Expected: FAIL — the file is not in `d.0` (it is still in a temp dir), or a compile error on
`Destination` if Task 1's import is missing.

- [ ] **Step 3: Implement the staging directory**

In `crates/app/src/batch_export/worker.rs`, add near the top of the file:

```rust
/// A per-run directory for files that are uploaded and not kept.
///
/// ⚠ Keyed on the process id **and** a counter — `immich_list_cache`'s pattern. The two sites in
/// `immich_source` that key on the pid alone are the ones **not** to copy: two runs in one process
/// would share a directory.
static STAGE_SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Make the staging directory for a run, or `None` if it cannot be created.
///
/// ⚠ `None` is not fatal: the caller falls back to `dest_dir`, which is worse for tidiness and
/// better than failing a whole export over a temp directory.
fn staging_dir() -> Option<PathBuf> {
    let d = std::env::temp_dir().join(format!(
        "rustroom_export_stage_{}_{}",
        std::process::id(),
        STAGE_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&d).ok().map(|_| d)
}
```

In `run`, after the `uploader` is built, add:

```rust
    // ⚠⚠ Immich-only stages here and keeps nothing. `Files`/`Both` write straight to `dest_dir`.
    // ⚠ Falls back to `dest_dir` when a temp directory cannot be made — the export is worth more
    // than the tidiness.
    let staging = (!opts.destination.keeps_files())
        .then(staging_dir)
        .flatten();
    let out_dir = staging.clone().unwrap_or_else(|| opts.dest_dir.clone());
```

Change the `plan(...)` call to pass `&out_dir` and whether collisions matter:

```rust
    let Some(planned) = plan(
        jobs,
        &opts,
        &template,
        &events,
        &decisions,
        &cancel,
        &mut report,
        &out_dir,
        staging.is_none(),
    ) else {
        return;
    };
```

- [ ] **Step 4: Teach `plan` the output directory**

Change `plan`'s signature — it already carries `#[allow(clippy::too_many_arguments)]`:

```rust
    report: &mut ExportReport,
    // ⚠ The directory files are actually written to, which is the staging directory under
    // Immich-only and `opts.dest_dir` otherwise. `opts` is still passed for everything else.
    out_dir: &Path,
    // ⚠⚠ False for a staging run: a **fresh temp directory cannot collide with anything**, so
    // asking the user about one would be a question with no meaning. ⚠ `claimed` still applies —
    // two sources sharing a stem must not overwrite each other inside the staging directory.
    check_collisions: bool,
) -> Option<Vec<Planned>> {
```

Inside `plan`, wherever `opts.dest_dir` was used to build the destination, use `out_dir`, and wrap the
collision resolution so it is skipped when `!check_collisions`. Find the `dest_for(...)` call and the
`Collision` handling; the shape becomes:

```rust
        // ⚠ `dest_named` rather than `dest_for` when collisions are not checked: the latter's whole
        // job is to detect an existing file and ask, which a fresh staging directory never has.
        let dest = if check_collisions {
            // …existing collision path, with `out_dir` in place of `&opts.dest_dir`…
        } else {
            let mut n = 0;
            loop {
                let cand = dest_named(out_dir, &base, n, opts.format);
                if !claimed.contains(&cand) {
                    break cand;
                }
                n += 1;
            }
        };
```

⚠ **Read the existing body before editing it** and keep its `claimed.insert(dest.clone())`,
`remembered` handling and `report.skipped` accounting exactly as they are — this step only redirects
the directory and bypasses the *prompt*.

- [ ] **Step 5: Carry `edited_at` on `Planned`**

Add to `Planned`:

```rust
    /// The sidecar's mtime, for `UploadDate::Edited`.
    ///
    /// ⚠⚠ **Settled HERE, at plan time, and not at upload time.** `immich_upload::record` writes the
    /// upload into the photo's sidecar, which bumps that very mtime — so read late, "the edit date"
    /// silently becomes "the date of the last upload", and differs between the first upload and
    /// every one after.
    edited_at: Option<i64>,
```

and in the `out.push(Planned { … })` literal:

```rust
            edited_at: crate::sidecar::edited_at(&source).and_then(|t| {
                t.duration_since(std::time::UNIX_EPOCH)
                    .ok()
                    .map(|d| d.as_secs() as i64)
            }),
```

⚠ `source` is moved into the literal in the existing code — compute `edited_at` **before** the
`Planned { … }` literal if the borrow checker objects, and pass the variable in.

- [ ] **Step 6: Delete or move, on the outcome**

Replace the report-recording block in the worker closure with:

```rust
                {
                    let mut r = shared.lock().expect("export report");
                    match outcome {
                        Ok(()) => r.exported += 1,
                        Err(e) => r.failed.push((p.source.clone(), e)),
                    }
                    match upload {
                        // ⚠⚠ Deleted **only** on a confirmed upload. An SSL/EOF error means the
                        // *response* was lost, not that the server rejected the file — 4 of 2,373
                        // writes failed that way on this network and none had actually failed to
                        // apply. Deleting on anything less would destroy work on a flaky link.
                        Some(Ok(())) => {
                            r.uploaded += 1;
                            if staging.is_some() {
                                let _ = std::fs::remove_file(&p.dest);
                            }
                        }
                        Some(Err(e)) => {
                            let at = if staging.is_some() {
                                rescue(&p.dest, &opts.dest_dir)
                            } else {
                                p.dest.clone()
                            };
                            r.unconfirmed.push((at, e));
                        }
                        None => {}
                    }
                }
```

Add the helper beside `staging_dir`:

```rust
/// Move a staged file somewhere the user will find it, returning where it ended up.
///
/// ⚠⚠ The **least-exercised path** in this feature, which is why it has its own test rather than a
/// reading. A file whose upload did not confirm must not be swept with the temp directory.
///
/// ⚠ Returns the *temp* path when there is nowhere to move it or the move fails — a path that does
/// not exist is worse than an ugly one, and `unconfirmed` is the one place the user looks.
fn rescue(staged: &Path, dest_dir: &Path) -> PathBuf {
    if !destination_ready(dest_dir) {
        return staged.to_path_buf();
    }
    let Some(name) = staged.file_name() else {
        return staged.to_path_buf();
    };
    let to = dest_dir.join(name);
    // ⚠ `rename` first: a same-filesystem move is atomic and instant. It fails across devices, and
    // a temp directory very often is one, so copy-then-remove is the fallback rather than the plan.
    if std::fs::rename(staged, &to).is_ok() {
        return to;
    }
    match std::fs::copy(staged, &to) {
        Ok(_) => {
            let _ = std::fs::remove_file(staged);
            to
        }
        Err(_) => staged.to_path_buf(),
    }
}
```

- [ ] **Step 7: Remove the staging directory at the end of the run**

Immediately before `let _ = events.send(ExportEvent::Finished(report));`:

```rust
    // ⚠ Only when empty. A staging directory still holding files is **not** an error — it means
    // something did not confirm and could not be rescued, and `unconfirmed` already names each one.
    if let Some(dir) = &staging {
        let _ = std::fs::remove_dir(dir);
    }
```

- [ ] **Step 8: Run the tests**

```bash
cargo test --workspace staged_file
```
Expected: `1 passed`. Then:
```bash
cargo test --workspace files_only_keeps
```
Expected: `1 passed`.

- [ ] **Step 9: Add the delete-half test and run it**

```rust
    /// ⚠⚠ **Without this the delete is unverified and "Immich only" quietly keeps everything.** It
    /// cannot use a real server, so it drives the same code with a *stubbed* confirmation by using
    /// `Destination::Target` with **no** connection: nothing is uploaded, so nothing is deleted —
    /// and the staged file must therefore be rescued into the export folder, not left in temp.
    #[test]
    fn a_staging_run_never_leaves_a_file_in_the_temp_directory() {
        let d = TempDir::new("stage_tidy");
        let mut o = opts(&d, None);
        o.destination = Destination::Target(UploadTarget::Immich);
        let worker = ExportWorker::spawn(
            jobs(&d, &["a.RAF"]),
            o,
            ok_decode(),
            Arc::new(AtomicBool::new(false)),
            CPU,
            1,
            None,
            Some(crate::immich_upload::UploadConfig {
                base_url: "http://127.0.0.1:1".to_string(),
                key: "irrelevant".to_string(),
            }),
        );
        let report = finished(&worker);
        let at = &report.unconfirmed[0].0;
        assert!(
            at.starts_with(&d.0),
            "the file was left outside the export folder at {at:?}"
        );
        assert!(at.exists(), "the reported path must exist");
    }
```

```bash
cargo test --workspace temp_directory
```
Expected: `1 passed`.

- [ ] **Step 10: Gate and commit**

```bash
cargo fmt --all; cargo fmt --all --check; echo "FMT=$?"
CARGO_TARGET_DIR=target/clippy cargo clippy --workspace --all-targets -- -D warnings > /tmp/c.log 2>&1; echo "CLIPPY=$?"
cargo test --workspace > /tmp/t.log 2>&1; echo "TEST=$?"
```
Expected: all `=0`.

```bash
git add crates/app/src/batch_export/worker.rs
git commit -m "feat(export): stage Immich-only files, delete on confirmation, rescue on doubt"
```

---

## Task 4: Pass the date through to the upload

**Files:**
- Modify: `crates/app/src/batch_export/worker.rs`

- [ ] **Step 1: Change `Uploader::send`**

```rust
    /// Upload one written file. `Err` carries a message for `unconfirmed`.
    fn send(
        &self,
        exported: &Path,
        capture: &core_image::Capture,
        source: &Path,
        copy: u32,
        which: UploadDate,
        edited_at: Option<i64>,
    ) -> Result<(), String> {
        let filename = exported
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| "the exported file has no usable name".to_string())?;
        let created = crate::immich_upload::created_at_text(which, capture.taken, edited_at);
        let modified = crate::immich_upload::modified_at_text(exported);
        let up = self
            .client
            .upload(exported, filename, &created, &modified)
            .map_err(|e| e.to_string())?;
        // ⚠ Recorded against the SOURCE photo's sidecar, not the exported file: the sidecar belongs
        // to the photo, and under Immich-only the export is deleted moments later.
        crate::immich_upload::record(source, copy, &self.host, &up);
        Ok(())
    }
```

- [ ] **Step 2: Update the call site**

```rust
                    (Ok(()), Some(u)) => Some(u.send(
                        &p.dest,
                        &p.capture,
                        &p.source,
                        p.copy,
                        opts.upload_date,
                        p.edited_at,
                    )),
```

- [ ] **Step 3: Build and run the whole suite**

```bash
cargo test --workspace > /tmp/t.log 2>&1; echo "TEST=$?"; grep -c '^test .* ok$' /tmp/t.log
```
Expected: `TEST=0`.

- [ ] **Step 4: Commit**

```bash
git add crates/app/src/batch_export/worker.rs
git commit -m "feat(export): the upload carries the date the options ask for"
```

---

## Task 5: The Delivery section

**Files:**
- Modify: `crates/app/src/export_summary.rs`
- Modify: `crates/app/src/modals/export.rs`

- [ ] **Step 1: Write the failing summary test**

Replace `the_upload_summary_says_which_state_it_is_in` in
`crates/app/src/export_summary.rs` with:

```rust
    /// ⚠ Every section header in this dialog says its own state, so a collapsed dialog reads as a
    /// description of the export. ⚠ The date is mentioned only when something is uploaded — under
    /// "Files only" it has nothing to act on and naming it would be noise.
    #[test]
    fn the_delivery_summary_says_the_mode_and_the_date() {
        use crate::batch_export::{Destination, UploadDate, UploadTarget};
        let mut o = ExportOptions::default();
        assert_eq!(delivery(&o), "Files only");

        o.destination = Destination::Target(UploadTarget::Immich);
        assert_eq!(delivery(&o), "Immich only, original date");

        o.destination = Destination::Both(UploadTarget::Immich);
        o.upload_date = UploadDate::Uploaded;
        assert_eq!(delivery(&o), "Files and Immich, upload date");
    }
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test --workspace delivery_summary
```
Expected: FAIL to compile — `cannot find function delivery`.

- [ ] **Step 3: Implement the summary**

Replace `pub fn upload` in `crates/app/src/export_summary.rs` with:

```rust
/// The Delivery section's trailing summary.
pub fn delivery(o: &ExportOptions) -> String {
    use crate::batch_export::Destination;
    let mode = match o.destination {
        Destination::Files => return "Files only".to_string(),
        Destination::Target(_) => "Immich only",
        Destination::Both(_) => "Files and Immich",
    };
    // ⚠ Lower-cased so the phrase reads as one sentence rather than two labels.
    format!("{mode}, {}", o.upload_date.label().to_lowercase())
}
```

- [ ] **Step 4: Run the test**

```bash
cargo test --workspace delivery_summary
```
Expected: `1 passed`.

- [ ] **Step 5: Replace the dialog section**

In `crates/app/src/modals/export.rs`, replace the whole `if upload_avail != … { section(ui, "Upload", …) }`
block added by the previous milestone with:

```rust
                    // ⚠⚠ Drawn only when Immich is configured, and it holds **every** Immich
                    // mention in this dialog — which is what keeps "switch it off and no mention
                    // remains" a single condition rather than a list of places to remember.
                    if upload_avail != rustroom_app::immich_upload::Availability::NotConfigured {
                        section(
                            ui,
                            "Delivery",
                            &export_summary::delivery(&opts),
                            false,
                            |ui| {
                                let ready =
                                    upload_avail == rustroom_app::immich_upload::Availability::Ready;
                                let mut mode = opts.destination;
                                let r = ui.add_enabled_ui(ready, |ui| {
                                    crate::segmented::segmented(
                                        ui,
                                        &mut mode,
                                        &[
                                            (Destination::Files, "Files only"),
                                            (
                                                Destination::Target(UploadTarget::Immich),
                                                "Immich only",
                                            ),
                                            (
                                                Destination::Both(UploadTarget::Immich),
                                                "Files and Immich",
                                            ),
                                        ],
                                    )
                                });
                                if ready {
                                    opts.destination = mode;
                                } else {
                                    r.response.on_disabled_hover_text(
                                        "Set an Upload key in Settings → Immich first. Browsing \
                                         uses a read-only key, which cannot upload.",
                                    );
                                }
                                // ⚠ Disabled rather than hidden when nothing is uploaded: it has
                                // nothing to act on, and a control that vanishes reads as a bug.
                                ui.add_enabled_ui(opts.destination.target().is_some(), |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label("Date");
                                        egui::ComboBox::from_id_salt("upload_date")
                                            .selected_text(opts.upload_date.label())
                                            .show_ui(ui, |ui| {
                                                for d in
                                                    rustroom_app::batch_export::UploadDate::ALL
                                                {
                                                    ui.selectable_value(
                                                        &mut opts.upload_date,
                                                        d,
                                                        d.label(),
                                                    );
                                                }
                                            });
                                    });
                                });
                                if let Some(note) = &already_uploaded {
                                    // ⚠⚠ Informs, never blocks — a re-export after editing IS a new
                                    // picture, and replacing the old asset would mean deleting one.
                                    ui.label(egui::RichText::new(note).weak()).on_hover_text(
                                        "Uploading again adds a second photo to Immich. Immich \
                                         recognises a byte-identical re-upload and keeps only one.",
                                    );
                                }
                            },
                        );
                    }
```

Add to the imports at the top of `modals/export.rs`:

```rust
use rustroom_app::batch_export::{Destination, UploadTarget};
```

And change the footer line to pass `upload_ready`:

```rust
        Footer::confirm(
            "Export",
            can_export(
                &opts,
                upload_avail == rustroom_app::immich_upload::Availability::Ready,
            ),
            "Cancel",
        )
        .note(&note),
```

⚠ `upload_avail` is computed before the closure already; if the borrow checker objects to using it
inside, copy it into a local `let ready = …;` before `let out = dialog(…)` — `Availability` is `Copy`.

- [ ] **Step 6: Update the section-name test**

`the_sections_the_presets_and_the_footer_are_offered` lists section names. It does not include
"Upload", so it needs no change — but `without_immich_configured_the_upload_section_is_absent` and
`with_immich_but_no_upload_key_the_section_appears` both look for `"Upload"`. Change both to
`"Delivery"`, and rename them:

```rust
    /// ⚠⚠ **"No mention of immich at all" when the integration is off** (`docs/BACKLOG.md`) — so an
    /// unconfigured Immich must draw **no** Delivery section, not a disabled one.
    #[test]
    fn without_immich_configured_the_delivery_section_is_absent() {
        let drawn = with_app(|app| {
            app.settings.immich_url.clear();
            app.settings.immich_key.clear();
        });
        assert!(
            !drawn.iter().any(|l| l == "Delivery"),
            "a Delivery section was drawn with no Immich configured: {drawn:?}"
        );
        // ⚠ The sample assertion: the dialog really drew, so the absence means something.
        assert!(
            drawn.iter().any(|l| l == "Watermark"),
            "the dialog did not draw at all: {drawn:?}"
        );
    }

    /// ⚠ Configured but with no upload key: the section appears and its controls are **disabled**,
    /// because a control that vanishes reads as a bug.
    #[test]
    fn with_immich_but_no_upload_key_the_delivery_section_appears() {
        let drawn = with_app(|app| {
            app.settings.immich_url = "https://immich.example".to_string();
            app.settings.immich_key = "browse-key".to_string();
            app.settings.immich_upload_key.clear();
        });
        assert!(
            drawn.iter().any(|l| l == "Delivery"),
            "the Delivery section should appear once Immich is configured: {drawn:?}"
        );
    }
```

- [ ] **Step 7: Add a test that the three modes are offered**

```rust
    /// ⚠ The segmented control's three columns are reachable by label, which is how the harness
    /// sees them. Without this the section could draw and offer nothing.
    #[test]
    fn the_three_delivery_modes_are_offered() {
        let drawn = with_app(|app| {
            app.settings.immich_url = "https://immich.example".to_string();
            app.settings.immich_key = "browse-key".to_string();
            app.settings.immich_upload_key = "upload-key".to_string();
        });
        for want in ["Files only", "Immich only", "Files and Immich", "Original date"] {
            assert!(
                drawn.iter().any(|l| l == want),
                "{want:?} missing: {drawn:?}"
            );
        }
    }
```

- [ ] **Step 8: Run and gate**

```bash
cargo test --workspace delivery
```
Expected: every delivery-named test passes; confirm the count is not `0 passed`.

```bash
cargo fmt --all; cargo fmt --all --check; echo "FMT=$?"
CARGO_TARGET_DIR=target/clippy cargo clippy --workspace --all-targets -- -D warnings > /tmp/c.log 2>&1; echo "CLIPPY=$?"
cargo test --workspace > /tmp/t.log 2>&1; echo "TEST=$?"
```
Expected: all `=0`.

- [ ] **Step 9: Commit**

```bash
git add crates/app/src/export_summary.rs crates/app/src/modals/export.rs
git commit -m "feat(export): a Delivery section holding the mode and the upload date"
```

---

## Task 6: The goldens, and the tip

- [ ] **Step 1: Confirm the goldens are unmoved**

```bash
cargo test --workspace --test golden > /tmp/g.log 2>&1; echo "GOLDEN=$?"; grep -c '^test .* ok$' /tmp/g.log
```
Expected: `GOLDEN=0` and **75**. ⚠ If any golden moved, `Files` is not unchanged — stop and find out
why before going further.

- [ ] **Step 2: Re-gate the committed tip**

⚠⚠ **A gate run before the last commit describes a tree nobody shipped.**

```bash
cargo fmt --all --check; echo "FMT=$?"
CARGO_TARGET_DIR=target/clippy cargo clippy --workspace --all-targets -- -D warnings > /tmp/c.log 2>&1; echo "CLIPPY=$?"
cargo test --workspace > /tmp/t.log 2>&1; echo "TEST=$?"; grep -c '^test .* ok$' /tmp/t.log
```
Expected: all `=0`, count **above 2,852**.

- [ ] **Step 3: Launch the GUI**

```bash
cargo run --release -p app
```
In the **background**. Tell the user what to look at: the Export dialog's **Delivery** section with
three modes and a Date dropdown; the dropdown greys out under **Files only**; and the section is
absent entirely if the Immich URL is cleared.

- [ ] **Step 4: Verify by use, against the real server**

⚠⚠ **Every Immich defect in this repo was found by use, not by a test.** Ask the user to run, and
report each:

1. **Immich only** on one photo → the photo is on the server and **nothing is left in the export
   folder**.
2. **Files and Immich** → both the file and the asset exist.
3. Each of the three **dates**, checked against the asset:
   ```bash
   K=$(python3 -c "import json;print(json.load(open('~/Library/Application Support/rustroom/settings.json'))['immich_key'])")
   H=https://immich.example
   curl -s -m 30 -X POST -H "x-api-key: $K" -H 'Content-Type: application/json' \
     -d '{"createdAfter":"2026-09-24T00:00:00.000Z","size":50,"page":1}' \
     "$H/api/search/metadata" | python3 -c "
   import json,sys
   for a in json.load(sys.stdin)['assets']['items']:
       print(a['originalFileName'], a.get('fileCreatedAt'), a.get('createdAt'))"
   ```
   Expected: **Original date** gives the capture date, **Upload date** gives today, **Edit date**
   gives when the edits were last saved.
4. ⚠ An **Immich-only run with the server unreachable** (turn off the tailnet) leaves the file in the
   export folder — the rescue path, in production rather than in a test.

- [ ] **Step 5: Commit anything use reveals, then document**

Add a `### 2026-09-24 — …` section at the **top** of the dated entries in `docs/JOURNAL.md`: the
three modes, the rescue-on-doubt rule and why deletion needs *confirmation*, the removal of the mtime
fallback, and whatever use revealed. Overwrite `CLAUDE.md`'s state block **in place** — ⚠ never append
a superseded block — and mark the item in `docs/BACKLOG.md`. ⚠ `CLAUDE.md` is at **431** lines against
a ~400 target: a few lines here, the detail in the journal.

```bash
cargo fmt --all --check; echo "FMT=$?"
git add docs/ CLAUDE.md
git commit -m "docs: export delivery — files, Immich or both, and the upload date"
```

- [ ] **Step 6: Report — do NOT merge or push**

⚠ Merging is the user's call and **pushing is never Claude's**.

---

## Self-review

**Spec coverage:**

| spec section | task |
|---|---|
| §2 `Destination`, `can_export` | 1 |
| §3 staging, delete on confirmation, rescue, temp cleanup, collisions | 3 |
| §4 the three dates, mtime fallback removed, `Edited` read at plan time | 2, 3 (step 5), 4 |
| §5 the Delivery section, disabled date, absent when unconfigured | 5 |
| §6 placement | 1, 2, 3, 5 |
| §7 tests 1–6 | 1, 2, 3, 6 |
| §9 definition of done | 6 |

**Placeholder scan:** none. Two steps say *"read the existing body before editing"* (Task 3 steps 4
and 5) — deliberate, because `plan`'s collision block is intricate and inventing its shape here would
be worse than telling the engineer to look at it.

**Type consistency:** `Destination::{Files,Target,Both}` and its `keeps_files`/`target` (Task 1) are
used by Tasks 3, 5. `UploadDate::{Captured,Edited,Uploaded}` + `ALL` + `label` (Task 1) by Tasks 2, 4,
5. `upload_seconds`/`created_at_text(which, taken, edited)` (Task 2) by Task 4. `Planned.edited_at`
(Task 3) by Task 4. `rescue`/`staging_dir` (Task 3) used only there. `export_summary::delivery`
(Task 5) by Task 5's dialog. `can_export(opts, upload_ready)` (Task 1) by Task 5's footer.

⚠ **One gap found and closed:** the spec's §7 test 5 — *"a confirmed upload under `Target` leaves no
file behind"* — cannot be written without a server, since nothing in-process returns `Ok` from a real
upload. Task 3 step 9 substitutes the reachable half (a staging run never leaves a file in the temp
directory) and Task 6 step 4.1 covers the confirmed case **by use**, which is where it honestly
belongs. Stated rather than silently dropped.
