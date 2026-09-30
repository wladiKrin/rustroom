//! What an Immich upload needs decided before any HTTP happens: when the photo was taken, what to
//! call the file, whether uploading is possible at all, and what was uploaded before.
//!
//! ⚠ **No egui here.** This is the testable half; the dialog lives in `modals::export`.

use crate::batch_export::UploadDate;
use crate::settings::Settings;
use std::path::Path;

/// Whether an upload can be offered, and if not, why.
///
/// ⚠⚠ **This is the seed of the "Immich completely optional" gate** (`docs/BACKLOG.md`). Today
/// `NotConfigured` means no server URL or browse key; once a master toggle exists it answers
/// `NotConfigured` too, and every Immich surface consults **one** predicate rather than growing its
/// own conditional.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    /// No Immich at all — the section is **not drawn**.
    NotConfigured,
    /// Immich is set up but no upload key is set — the control is **drawn and disabled**.
    NoUploadKey,
    Ready,
}

/// Whether uploading can be offered, given the settings.
pub fn availability(s: &Settings) -> Availability {
    // ⚠⚠ Through `connection`, **never** the raw fields: that function is the one gate the whole
    // integration hangs on, and reading `immich_url`/`immich_key` here would be a second condition
    // that the master toggle does not reach.
    if crate::immich_source::connection(s).is_none() {
        return Availability::NotConfigured;
    }
    if s.immich_upload_key.trim().is_empty() {
        return Availability::NoUploadKey;
    }
    Availability::Ready
}

/// The connection an export run uploads with.
///
/// ⚠⚠ **A narrow struct rather than passing `Settings` into the export worker.** The worker settles
/// filenames and renders photographs; giving it the whole preferences object would let any future
/// change in there reach code that has no business reading it. This also keeps the credential's one
/// reader obvious.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadConfig {
    pub base_url: String,
    /// ⚠ The **upload** key, never the browse key.
    pub key: String,
}

/// The upload connection, or `None` when uploading is not possible — which is the ordinary
/// "disk only" case, not an error.
pub fn config(s: &Settings) -> Option<UploadConfig> {
    if availability(s) != Availability::Ready {
        return None;
    }
    Some(UploadConfig {
        base_url: s.immich_url.trim().to_string(),
        key: s.immich_upload_key.trim().to_string(),
    })
}

/// Which second an uploaded photo is dated with.
///
/// ⚠⚠ **A pure function of four values, and deliberately so** — this is the whole date decision, and
/// it must be checkable without a window, a server or a file on disk.
///
/// ⚠⚠ **THERE IS NO FILE-MTIME FALLBACK ANY MORE, AND ITS REMOVAL IS THE POINT.** `Captured` used to
/// fall back to the source file's mtime; for a file in the Immich cache that is **when Rustroom
/// downloaded it**, so a photo whose date could not be read was filed under the download date — a
/// wrong answer that *looked* plausible, which is the worst kind. An absent value now yields `now`:
/// the photo lands at the top of the timeline where it is noticed, and nothing invents a past.
pub fn upload_seconds(which: UploadDate, taken: Option<i64>, edited: Option<i64>, now: i64) -> i64 {
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

/// `fileModifiedAt`: the exported file's own mtime.
///
/// ⚠ A **true instant**, unlike [`created_at_text`]'s wall-clock — this genuinely is when this
/// rendering was written, so UTC is honest here. The two fields carry different conventions on
/// purpose.
pub fn modified_at_text(exported: &Path) -> String {
    utc_text(mtime_secs(exported).unwrap_or(0))
}

fn mtime_secs(p: &Path) -> Option<i64> {
    let m = std::fs::metadata(p).ok()?.modified().ok()?;
    let d = m.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(d.as_secs() as i64)
}

fn utc_text(secs: i64) -> String {
    chrono::DateTime::from_timestamp(secs, 0)
        .map(|d| d.format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string())
        .unwrap_or_else(|| "1970-01-01T00:00:00.000Z".to_string())
}

/// Record an upload against the **copy that was exported**.
///
/// ⚠⚠ **Copy-aware, and that is load-bearing.** An earlier draft wrote copy 0 unconditionally
/// while [`uploaded_note`] reads the copy actually being exported — so for any virtual copy the
/// write and the read would have addressed different entries and the note would have stayed silent
/// for ever. A record nobody can find is worse than no record.
///
/// ⚠ Best-effort: a sidecar that cannot be written must not turn a successful upload into a
/// failure. The asset exists on the server either way, and the record is a convenience.
pub fn record(source: &Path, copy: u32, host: &str, up: &immich::Upload) {
    let mut sc = crate::sidecar::load_all(source);
    let at = utc_text(now_secs());
    let rec = crate::sidecar::UploadRecord {
        host: host.to_string(),
        asset_id: up.id.clone(),
        status: format!("{:?}", up.status).to_lowercase(),
        at,
    };
    if let Some(entry) = sc.copies.iter_mut().find(|c| c.copy == copy) {
        entry.uploads.push(rec);
        let _ = crate::sidecar::save_all(source, &sc);
    }
    // ⚠ A copy with no sidecar entry yet is simply not recorded. Creating one here would mean
    // inventing an `Edits` for it, and a neutral photo deliberately has no entry.
}

/// What the dialog says about a photo already uploaded to `host`, or `None` when it has not been.
///
/// ⚠⚠ **Informs, never blocks.** A second upload after further editing is the *correct* outcome —
/// the bytes differ, so it is a new picture. Replacing the previous one would mean **deleting** an
/// asset, which is exactly the danger that got two-way sync declined.
pub fn uploaded_note(source: &Path, copy: u32, host: &str) -> Option<String> {
    let sc = crate::sidecar::load_all(source);
    let last = sc
        .copies
        .iter()
        .find(|c| c.copy == copy)?
        .uploads
        .iter()
        .rfind(|u| u.host == host)?;
    // ⚠ Date only: the time of day is noise in a one-line note.
    Some(format!(
        "Already uploaded on {}",
        &last.at[..10.min(last.at.len())]
    ))
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh directory of its own — `test_dir` makes it unique per test and per run.
    fn temp_dir(tag: &str) -> std::path::PathBuf {
        test_dir::path(&format!("upload-{tag}"))
    }

    fn settings_with(url: &str, key: &str, upload: &str) -> Settings {
        Settings {
            immich_url: url.to_string(),
            immich_key: key.to_string(),
            immich_upload_key: upload.to_string(),
            ..Settings::default()
        }
    }

    #[test]
    fn availability_needs_a_server_then_an_upload_key() {
        assert_eq!(
            availability(&settings_with("", "", "")),
            Availability::NotConfigured
        );
        assert_eq!(
            availability(&settings_with("https://h", "browse", "")),
            Availability::NoUploadKey
        );
        assert_eq!(
            availability(&settings_with("https://h", "browse", "up")),
            Availability::Ready
        );
        // ⚠ Whitespace is absent, or a stray space reads as configured.
        assert_eq!(
            availability(&settings_with("https://h", "browse", "   ")),
            Availability::NoUploadKey
        );
        // ⚠ An upload key without a browse key is still NotConfigured: uploading is an addition to
        // a working Immich setup, not a way to have one.
        assert_eq!(
            availability(&settings_with("https://h", "", "up")),
            Availability::NotConfigured
        );
    }

    /// ⚠⚠ **The bug this guards is a note that never appears.** `record` writes against the
    /// exported copy and this reads the same copy; an earlier draft of the plan wrote copy 0
    /// unconditionally, which would have silenced the note for **every virtual copy**. So this uses
    /// a **non-zero** copy, which is the case that would have been broken.
    #[test]
    fn the_note_finds_an_upload_of_the_same_copy_and_host() {
        let dir = temp_dir("note");
        let photo = dir.join("a.RAF");
        std::fs::write(&photo, b"x").unwrap();

        // A sidecar with an entry for copy 2 only.
        let mut sc = crate::sidecar::Sidecar::default();
        sc.set_edits(2, edit_model::Edits::default());
        crate::sidecar::save_all(&photo, &sc).unwrap();

        assert_eq!(
            uploaded_note(&photo, 2, "https://h"),
            None,
            "nothing uploaded yet"
        );

        let up: immich::Upload =
            serde_json::from_str(r#"{"id":"abc","status":"created"}"#).unwrap();
        record(&photo, 2, "https://h", &up);

        assert!(
            uploaded_note(&photo, 2, "https://h").is_some(),
            "the note must find an upload of the same copy"
        );
        // ⚠ The two discriminating negatives: a different copy and a different host must NOT match.
        assert_eq!(
            uploaded_note(&photo, 0, "https://h"),
            None,
            "copy must matter"
        );
        assert_eq!(
            uploaded_note(&photo, 2, "https://other"),
            None,
            "host must matter"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ⚠ `config` must be `None` for every state that is not `Ready`, so the worker cannot be
    /// handed a half-set connection.
    #[test]
    fn a_config_exists_only_when_everything_is_ready() {
        assert!(config(&settings_with("", "", "")).is_none());
        assert!(config(&settings_with("https://h", "browse", "")).is_none());
        let c = config(&settings_with("  https://h  ", "browse", "  up  ")).unwrap();
        // ⚠ Trimmed, or a pasted key with a trailing newline becomes an unauthorized header.
        assert_eq!(c.base_url, "https://h");
        assert_eq!(c.key, "up");
    }

    /// ⚠⚠ **`Captured` WITH NO CAPTURE DATE MUST YIELD THE UPLOAD TIME, NOT A FILE MTIME.** The
    /// removed fallback used the source file's mtime, which for a file in the Immich cache is *when
    /// Rustroom downloaded it* — so a photo whose date could not be read was filed under the
    /// download date. This is the regression guard for that.
    #[test]
    fn each_date_option_picks_its_own_source() {
        let now = 1_800_000_000;
        let taken = Some(1_763_559_144); // 2025-11-19T13:32:24
        let edited = Some(1_790_000_000);

        assert_eq!(
            upload_seconds(UploadDate::Captured, taken, edited, now),
            1_763_559_144
        );
        assert_eq!(
            upload_seconds(UploadDate::Edited, taken, edited, now),
            1_790_000_000
        );
        assert_eq!(
            upload_seconds(UploadDate::Uploaded, taken, edited, now),
            now
        );

        // ⚠ The absent cases fall to `now` — never to a plausible-looking past.
        assert_eq!(upload_seconds(UploadDate::Captured, None, edited, now), now);
        assert_eq!(upload_seconds(UploadDate::Edited, taken, None, now), now);
    }

    /// ⚠ The discriminating negative: `Captured` must not quietly use the edit date when the capture
    /// date is missing. Two fallbacks pointing at each other would make the options
    /// indistinguishable.
    #[test]
    fn a_missing_capture_date_does_not_borrow_the_edit_date() {
        let now = 1_800_000_000;
        let got = upload_seconds(UploadDate::Captured, None, Some(1_790_000_000), now);
        assert_eq!(got, now, "Captured fell back to the edit date");
    }

    /// ⚠ The formatted form, so the wire value is pinned and not just the seconds.
    #[test]
    fn the_created_text_formats_the_chosen_second() {
        let taken = Some(1_787_824_800); // 2026-08-27T10:00:00 wall-clock
        assert_eq!(
            created_at_text(UploadDate::Captured, taken, None),
            "2026-08-27T10:00:00.000Z"
        );
        // ⚠ No capture date means *now*, so the safe assertions are that it is neither the epoch nor
        // the old mtime behaviour; the exact second is the clock's.
        let got = created_at_text(UploadDate::Captured, None, None);
        assert_ne!(got, "1970-01-01T00:00:00.000Z");
        assert!(got.ends_with('Z') && got.len() == 24, "shape: {got}");
    }

    /// ⚠⚠ **The one predicate must reach the surface built before it.** The export dialog's Location
    /// ticks are drawn on `Availability::NotConfigured`, and `availability` reads the connection — so
    /// the master toggle hides them with no change to the dialog at all. This is what "one gate"
    /// buys, stated as a test.
    #[test]
    fn the_master_toggle_reaches_the_export_ticks() {
        let mut s = settings_with("https://h", "browse", "up");
        assert_eq!(
            availability(&s),
            Availability::Ready,
            "a fully configured Immich must be Ready"
        );
        s.immich_enabled = false;
        assert_eq!(
            availability(&s),
            Availability::NotConfigured,
            "the toggle must read as NotConfigured, which is what hides the ticks entirely"
        );
    }
}
