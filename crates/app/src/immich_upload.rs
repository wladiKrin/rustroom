//! What an Immich upload needs decided before any HTTP happens: when the photo was taken, what to
//! call the file, whether uploading is possible at all, and what was uploaded before.
//!
//! ⚠ **No egui here.** This is the testable half; the dialog lives in `modals::export`.

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
    if s.immich_url.trim().is_empty() || s.immich_key.trim().is_empty() {
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

/// `fileCreatedAt` for a photo: **its capture time**, never now.
///
/// ⚠⚠ **Naive wall-clock rendered as if UTC, and that is measured rather than chosen.**
/// `Capture::taken` is naive local wall-clock by its own rule. Immich did exactly this to the
/// sibling JPEG of a real pair: EXIF `DateTimeOriginal` `20:41:34` became
/// `fileCreatedAt: 2026-08-12T20:41:34.000Z`. Matching that convention is what makes an uploaded
/// edit land at **the same `fileCreatedAt` as its original**, which is the property that matters
/// and the one to verify by use.
///
/// ⚠ Falls back to the source file's mtime when EXIF has no date — some PNG/TIFF sources have none
/// at all. Falls back to the epoch only if even that fails, which is visibly wrong in the timeline
/// rather than silently plausible.
pub fn created_at_text(capture: &core_image::Capture, source: &Path) -> String {
    let secs = capture.taken.or_else(|| mtime_secs(source)).unwrap_or(0);
    utc_text(secs)
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

    /// ⚠ Temp-dir isolation keyed on pid **and** a counter — `immich_list_cache`'s pattern. Two
    /// sites in `immich_source` key on the pid alone and are the ones not to copy.
    static COUNTER: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!(
            "rustroom_upload_{tag}_{}_{}",
            std::process::id(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
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

    /// ⚠⚠ **The actual bug this guards is "the edit landed at today's date".** So it asserts the
    /// exact string for a known capture time, and separately that the result is not now.
    #[test]
    fn the_created_date_is_the_capture_time_not_now() {
        // 2026-08-27T10:00:00 wall-clock
        let c = core_image::Capture {
            taken: Some(1_787_824_800),
            ..core_image::Capture::default()
        };
        let got = created_at_text(&c, Path::new("/nonexistent-on-purpose"));
        assert_eq!(got, "2026-08-27T10:00:00.000Z");
        assert_ne!(
            got,
            utc_text(now_secs()),
            "the capture time must not be replaced by the upload time"
        );
    }

    /// ⚠ The discriminating case for the fallback: no EXIF date, so the file's mtime is used.
    #[test]
    fn a_photo_with_no_exif_date_falls_back_to_the_file_mtime() {
        let dir = temp_dir("mtime");
        let f = dir.join("x.png");
        std::fs::write(&f, b"x").unwrap();

        let c = core_image::Capture::default();
        assert_eq!(c.taken, None, "the fixture must have no EXIF date");
        let got = created_at_text(&c, &f);

        assert_ne!(got, "1970-01-01T00:00:00.000Z", "the mtime was not used");
        assert_eq!(got.len(), 24, "unexpected shape: {got}");
        assert!(got.ends_with('Z'), "unexpected shape: {got}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// ⚠ A missing file and no EXIF is the only case that reaches the epoch, and it must be
    /// *visibly* wrong rather than plausibly recent.
    #[test]
    fn with_no_date_and_no_file_the_result_is_the_epoch() {
        let c = core_image::Capture::default();
        assert_eq!(
            created_at_text(&c, Path::new("/nonexistent-on-purpose")),
            "1970-01-01T00:00:00.000Z"
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
}
