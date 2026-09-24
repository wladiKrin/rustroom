//! The `<file>.json` edit sidecar.
//!
//! Extracted from `main.rs` so `sync` can reach it and so it is testable at all.

use edit_model::{EditLoadError, Edits};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The sidecar path for a source file: the full filename with `.json` **appended**
/// (`photo.RAF` → `photo.RAF.json`), not the extension replaced — so a RAW and a JPEG
/// sharing a basename get distinct sidecars.
pub fn path(photo: &Path) -> PathBuf {
    let mut s = photo.as_os_str().to_os_string();
    s.push(".json");
    PathBuf::from(s)
}

/// A photo's stored edits — **the original's**, copy 0.
///
/// `None` when there is no sidecar, it is malformed, or nothing is stored for copy 0. A
/// hand-edited file with a typo should read as "no edits", not take anything down.
///
/// ⚠⚠ This delegates to the copy-aware API rather than parsing the file itself, and that is not
/// tidiness. Once the sidecar became an envelope, a reader that parsed it as a bare `Edits`
/// returned `None` for **every** file written by the new code — which silently reset a synced
/// photo to neutral. Two readers of one format is the trap; there is now one.
pub fn load(photo: &Path) -> Option<Edits> {
    load_copy(photo, 0)
}

/// Write the original's edits, leaving every other copy alone.
pub fn save(photo: &Path, edits: &Edits) -> Result<(), EditLoadError> {
    let mut s = load_all(photo);
    s.set_edits(0, edits.clone());
    save_all(photo, &s)
}

/// Persist a photo's edits, **removing** the sidecar when they are neutral.
///
/// ⚠ Writing nothing and writing nothing *new* are not the same thing. A photo reset to
/// neutral must have its sidecar deleted, or the old edits are loaded straight back the next
/// time it is opened — which is exactly what "Reset all, switch away, switch back" did.
pub fn persist(photo: &Path, edits: &Edits) -> Result<(), EditLoadError> {
    persist_copy(photo, 0, edits)
}

/// Delete a photo's sidecar. Absent is not an error.
pub fn remove(photo: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(path(photo)) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

/// One saved state within a copy — a named point in that copy's editing.
///
/// ⚠⚠ **Deliberately outside `History<Edits>`, and joined to it at exactly one point.** Taking
/// one changes no `Edits`, so there is nothing for an undo stack of `Edits` to hold; *restoring*
/// one does change them, so `History::push` covers it and Ctrl+Z undoes a restore for free.
/// Undoing past a snapshot cannot touch it, because it was never in the stack. That independence
/// is what makes a snapshot unable to vanish silently — which is why this replaced the dropped
/// "persistent undo history". See `docs/superpowers/specs/2026-08-27-snapshots-design.md`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Snapshot {
    /// Identity: `max + 1` within its copy, **never reused**.
    ///
    /// ⚠ The copy-number rule, for the copy-number reason: an *unnamed* snapshot's label is
    /// derived from this, so reusing a number would silently rename an existing row.
    pub n: u32,
    /// What it is called, when it has been given a name. `None` follows the number.
    #[serde(default)]
    pub name: Option<String>,
    /// When it was taken — **local wall-clock seconds** since the epoch.
    ///
    /// ⚠ The same convention `Capture::taken` stores its date under, and for the same two
    /// reasons: rendering it needs no timezone, and wall-clock is what a person means by "when
    /// I took it".
    pub taken: i64,
    pub edits: Edits,
}

impl Snapshot {
    /// What this snapshot is called on screen.
    ///
    /// ⚠ **The one rule, with every consumer going through it** — the strip's row, the rename
    /// field's initial text, the name seeded into a virtual copy. That is `entries::shown_name`'s
    /// rule and the reason it exists: this project has already shipped a bug where the Library
    /// printed `file_name` while a copy's real name lived somewhere else, because two
    /// implementations of "what is this called" existed.
    pub fn label(&self) -> String {
        self.name
            .clone()
            .unwrap_or_else(|| format!("Snapshot {}", self.n))
    }
}

/// One upload of this copy to one Immich server.
///
/// ⚠ `status` is the server's own word (`created` / `duplicate`) kept as **text** rather than as an
/// enum: it is a record of what happened, and a future value should read back rather than fail.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UploadRecord {
    /// The server this went to, so a second server is not mistaken for the first.
    pub host: String,
    pub asset_id: String,
    pub status: String,
    /// When Rustroom uploaded it, ISO-8601 UTC.
    pub at: String,
}

/// One version of a photo.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CopyEntry {
    /// `0` is the original; copies are numbered from 1 and never reused.
    pub copy: u32,
    /// A short human label for this version — "B&W", "Client crop".
    ///
    /// ⚠⚠ **NOT a filename.** No extension, never sanitized, never reaches a path. `None`
    /// renders as the copy number. This field *was* the export filename, which is why it used
    /// to carry an extension and have to be path-safe; M6.1a's template took that job, so the
    /// premise is gone. See `docs/superpowers/specs/2026-09-01-copy-labels-design.md`.
    ///
    /// ⚠⚠ **Envelope metadata, never an `Edits` field, and that is load-bearing.** In `Edits` it
    /// would need an edit group and would then **travel with a paste, a preset or a sync** —
    /// pasting a B&W recipe onto another photo would rename it "B&W" — and "Reset adjustments"
    /// would wipe it. The cost is that `History<Edits>` cannot undo it, which is why relabelling
    /// gets its own `ActionKind`.
    ///
    /// ⚠ `alias = "name"` is what makes every sidecar written before 2026-09-01 load unchanged.
    #[serde(default, alias = "name")]
    pub label: Option<String>,
    #[serde(default)]
    pub edits: Edits,
    /// Uploads of this copy to Immich, newest last. Empty for a copy never uploaded.
    ///
    /// ⚠⚠ **Field-level `#[serde(default)]` only.** A container-level one on `Sidecar` itself is
    /// the documented data-loss trap — see `load_all`'s doc comment. This is safe for the same
    /// reason the `snapshots` field below is: the entry's other fields keep their own defaults and
    /// nothing about `Sidecar::copies` changes.
    ///
    /// ⚠ A list, not one record: uploading the same copy twice after further editing is the correct
    /// outcome, and the history of that is worth keeping.
    #[serde(default)]
    pub uploads: Vec<UploadRecord>,
    /// This copy's named saved states, oldest first.
    ///
    /// ⚠ **Per copy, not per file**, because each copy has its own `Edits` and so its own
    /// history to bookmark. The same split marks take, and the opposite of keywords, for the
    /// same underlying reason: a snapshot describes *the image you are making*.
    ///
    /// ⚠⚠ **`#[serde(default)]` belongs here and is required** — without it every sidecar
    /// written before this feature fails to parse and `load_all` silently returns an empty
    /// envelope, discarding every edit. ⚠ It must still **never** be added to `Sidecar` itself;
    /// that combination is the documented data-loss trap — see `load_all`'s doc comment.
    #[serde(default)]
    pub snapshots: Vec<Snapshot>,
    /// The highest snapshot number ever used in this copy.
    ///
    /// ⚠⚠ **Without this, deleting the *newest* snapshot frees its number.** Take two, delete the
    /// second, take another — it would be `2` again; then undo the deletion and the copy holds
    /// **two snapshots numbered 2**, where a rename or a delete hits whichever comes first.
    /// `max + 1` over the survivors cannot see a number that only the undo log still holds.
    ///
    /// ⚠ `#[serde(default)]` gives `0` for a sidecar written before this, which
    /// [`Sidecar::next_snapshot`] folds in with `max` — so an older photo degrades to the old
    /// behaviour rather than breaking.
    #[serde(default)]
    pub snapshot_hwm: u32,
}

/// Everything stored for one file.
///
/// ⚠⚠ **Deliberately NOT `#[serde(default)]`, and that omission is load-bearing.** `copies` being
/// a *required* field is what stops a legacy bare-`Edits` sidecar from ever being mistaken for an
/// envelope. Adding the attribute here — the obvious "forward compatibility" move — combined with
/// an untagged dispatch in `load_all` is what silently reads every existing sidecar as an empty
/// copy list. Verified by doing exactly that: untagged **alone** is harmless and every test
/// passes; untagged **plus** this attribute loses the edits.
///
/// `load_all`'s explicit dispatch means adding it would be safe today. Both guards are kept
/// because either one alone is enough, and neither is obvious to the next reader.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Sidecar {
    pub copies: Vec<CopyEntry>,
    /// The highest copy number ever used for this file.
    ///
    /// ⚠⚠ **Without this, deleting the *newest* copy frees its number.** Make two copies, delete
    /// copy 2, make another — it would be `2` again; then undo the deletion (`CopyDelete` keeps
    /// the removed entry precisely so it can come back) and the file holds **two copies numbered
    /// 2**. They share an export filename and a history-stash key, and the catalog upserts on
    /// `(folder_id, file_name, copy)`, so the Library shows **one** row for the pair and the
    /// other is unreachable. `max + 1` over the survivors cannot see a number that only the undo
    /// log still holds.
    ///
    /// ⚠ **Field-level `#[serde(default)]` only.** A *container*-level one on `Sidecar` is the
    /// documented data-loss trap — see this struct's own doc comment and `load_all`'s. Adding it
    /// here is safe because `copies` stays required, which is what the trap turns on.
    ///
    /// ⚠ Reset by [`Sidecar::compact`]: Reindex exists to close gaps, so it must hand the
    /// numbering back to the compacted maximum rather than keep counting from the old high.
    #[serde(default)]
    pub copy_hwm: u32,
}

impl Sidecar {
    pub fn get(&self, copy: u32) -> Option<&CopyEntry> {
        self.copies.iter().find(|c| c.copy == copy)
    }

    /// The number a new copy should take: one past the highest ever used here.
    ///
    /// ⚠ `max + 1`, never "first free". Reusing a number would point a new copy at an old
    /// copy's export filename and history stash.
    pub fn next_copy(&self) -> u32 {
        // ⚠ The high-water mark **and** the live maximum, not either alone: the mark is 0 in a
        // sidecar written before it existed, and the live maximum cannot see a number that only
        // the undo log can still bring back.
        let live = self.copies.iter().map(|c| c.copy).max().unwrap_or(0);
        live.max(self.copy_hwm) + 1
    }

    /// One copy's snapshots, oldest first. Empty when that copy has none, or is not there.
    pub fn snapshots_of(&self, copy: u32) -> &[Snapshot] {
        self.get(copy).map_or(&[], |c| c.snapshots.as_slice())
    }

    /// The number a new snapshot of `copy` should take: one past the highest ever used **there**.
    ///
    /// ⚠ `max + 1`, never "first free" and never `count + 1`. An unnamed snapshot's label is
    /// derived from its number, so reusing one silently renames an existing row — the same
    /// reasoning `next_copy` carries.
    ///
    /// ⚠ Per copy: two copies of one file number their snapshots independently, because a
    /// snapshot belongs to a copy's own editing.
    pub fn next_snapshot(&self, copy: u32) -> u32 {
        // ⚠ The high-water mark **and** the live maximum, not either alone: the mark is 0 in a
        // sidecar written before it existed, and the live maximum cannot see a deleted number
        // that the undo log can still bring back.
        let live = self
            .snapshots_of(copy)
            .iter()
            .map(|s| s.n)
            .max()
            .unwrap_or(0);
        let hwm = self.get(copy).map_or(0, |c| c.snapshot_hwm);
        live.max(hwm) + 1
    }

    /// Append a snapshot to `copy`, creating the copy's entry if it has none.
    pub fn add_snapshot(&mut self, copy: u32, snapshot: Snapshot) {
        let n = snapshot.n;
        match self.copies.iter_mut().find(|c| c.copy == copy) {
            Some(c) => {
                c.snapshots.push(snapshot);
                // ⚠ Raised here rather than in the caller, so every route in — taking one, and
                // undoing a deletion — keeps the mark true without having to remember to.
                c.snapshot_hwm = c.snapshot_hwm.max(n);
            }
            None => self.copies.push(CopyEntry {
                copy,
                label: None,
                edits: Edits::default(),
                snapshots: vec![snapshot],
                snapshot_hwm: n,
                uploads: Vec::new(),
            }),
        }
        self.copies.sort_by_key(|c| c.copy);
    }

    /// Take snapshot `n` out of `copy`, **returning it** so an undo can put it back.
    ///
    /// ⚠ Returned rather than discarded because nothing else holds it — `ActionKind::CopyDelete`
    /// carries a whole `CopyEntry` for exactly this reason.
    pub fn remove_snapshot(&mut self, copy: u32, n: u32) -> Option<Snapshot> {
        let c = self.copies.iter_mut().find(|c| c.copy == copy)?;
        let i = c.snapshots.iter().position(|s| s.n == n)?;
        Some(c.snapshots.remove(i))
    }

    /// Rename snapshot `n` of `copy`. Reports whether it was there to rename.
    pub fn set_snapshot_name(&mut self, copy: u32, n: u32, name: Option<String>) -> bool {
        let Some(c) = self.copies.iter_mut().find(|c| c.copy == copy) else {
            return false;
        };
        match c.snapshots.iter_mut().find(|s| s.n == n) {
            Some(s) => {
                s.name = name;
                true
            }
            None => false,
        }
    }

    /// Replace (or add) one copy's edits, leaving its name alone.
    pub fn set_edits(&mut self, copy: u32, edits: Edits) {
        // ⚠ Raised here rather than at the call sites, so every route that brings a copy into
        // existence keeps the mark true without having to remember to.
        self.copy_hwm = self.copy_hwm.max(copy);
        match self.copies.iter_mut().find(|c| c.copy == copy) {
            Some(c) => c.edits = edits,
            None => self.copies.push(CopyEntry {
                copy,
                label: None,
                edits,
                snapshots: Vec::new(),
                snapshot_hwm: 0,
                uploads: Vec::new(),
            }),
        }
        self.copies.sort_by_key(|c| c.copy);
    }

    /// Replace (or add) one copy's label, leaving its edits alone.
    pub fn set_label(&mut self, copy: u32, label: Option<String>) {
        // ⚠ Raised here rather than at the call sites, so every route that brings a copy into
        // existence keeps the mark true without having to remember to.
        self.copy_hwm = self.copy_hwm.max(copy);
        match self.copies.iter_mut().find(|c| c.copy == copy) {
            Some(c) => c.label = label,
            None => self.copies.push(CopyEntry {
                copy,
                label,
                edits: Edits::default(),
                snapshots: Vec::new(),
                snapshot_hwm: 0,
                uploads: Vec::new(),
            }),
        }
        self.copies.sort_by_key(|c| c.copy);
    }

    /// Put a deleted copy back, from the undo log's own copy of it.
    ///
    /// ⚠ A funnel rather than a bare `copies.push` at the call site: pushing directly bypasses
    /// the high-water mark, which is the one thing standing between an undo and two copies
    /// sharing a number.
    pub fn restore_copy(&mut self, entry: CopyEntry) {
        self.copy_hwm = self.copy_hwm.max(entry.copy);
        self.copies.retain(|c| c.copy != entry.copy);
        self.copies.push(entry);
        self.copies.sort_by_key(|c| c.copy);
    }

    /// Renumber the copies so they run 1, 2, 3… with no gaps, and report what moved.
    ///
    /// ⚠⚠ **This deliberately breaks the "copy numbers are never reused" invariant, and that is
    /// why it is a button rather than something that happens on its own.** Numbers are stable
    /// precisely so a copy's export filename and its undo stack cannot shift under it; the cost
    /// of that is `_1, _3, _7` after some deleting, which is the mess Reindex exists to tidy. An
    /// explicit action the user asked for is a different thing from a silent renumber.
    ///
    /// ⚠ A **named** copy keeps its name, so its export filename does not move — only unnamed
    /// copies, whose name is derived from the number, are renamed by this.
    ///
    /// ⚠ The caller must re-key or drop anything keyed on `PhotoId`, above all the history
    /// stash: without that, old copy 1's undo stack would attach to whatever becomes the new
    /// copy 1.
    pub fn compact(&mut self) -> Vec<(u32, u32)> {
        self.copies.sort_by_key(|c| c.copy);
        let mut moves = Vec::new();
        let mut next = 1;
        for c in self.copies.iter_mut() {
            // ⚠ Copy 0 *is* the file and is never renumbered — there is nothing else it could be.
            if c.copy == 0 {
                continue;
            }
            if c.copy != next {
                moves.push((c.copy, next));
                c.copy = next;
            }
            next += 1;
        }
        // ⚠⚠ **Reset, not raised.** Reindex exists to close gaps, so it must hand the numbering
        // back to the compacted maximum — keeping the old high would make the very next copy
        // `_10` on a file that now runs 1, 2, 3, which is the mess this button removes.
        //
        // ⚠ Safe only because Reindex already breaks the never-reused invariant deliberately and
        // drops the undo stacks of everything it renumbered — see this function's doc comment.
        self.copy_hwm = next - 1;
        moves
    }

    pub fn label_of(&self, copy: u32) -> Option<&str> {
        self.get(copy).and_then(|c| c.label.as_deref())
    }

    /// Whether this sidecar is worth writing at all.
    ///
    /// ⚠ Only true for "nothing but a neutral, unnamed original". A name or a second copy is
    /// worth a file even with neutral edits, or making a copy and then resetting it would delete
    /// the copy.
    pub fn is_neutral(&self) -> bool {
        match self.copies.as_slice() {
            [] => true,
            // ⚠ `snapshots.is_empty()` is the third clause of a rule this comment already
            // states: a copy holding saved states is worth a file however neutral its *current*
            // edits are. Without it, "take a snapshot, then Reset adjustments" deletes the
            // sidecar and every snapshot in it.
            [only] => {
                only.copy == 0
                    && only.label.is_none()
                    && only.edits == Edits::default()
                    && only.snapshots.is_empty()
            }
            _ => false,
        }
    }
}

/// When `photo`'s edits were last written, or `None` if it has never been edited.
///
/// ⚠ The sidecar's modification time is what "last edited" means here — there is no edit
/// timestamp stored inside it, and adding one would be a format change for something the
/// filesystem already records.
///
/// ⚠ A `stat` per photo, not a read: the sidecar's *contents* cost 4.6 ms for 830 files, and
/// asking for the timestamp alone is far less than that.
pub fn edited_at(photo: &Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path(photo)).ok()?.modified().ok()
}

/// Every version stored for `photo`.
///
/// ⚠⚠ **The dispatch is explicit rather than `#[serde(untagged)]`, and the reason is narrower
/// than it first looks — measured, not assumed.**
///
/// An untagged enum of `{ Envelope, Legacy }` is *by itself* safe here, because `Sidecar::copies`
/// is a required field and a legacy bare-`Edits` object therefore fails to match `Envelope`. But
/// it becomes silent data loss the moment anyone adds `#[serde(default)]` to `Sidecar` — at which
/// point every legacy sidecar matches `Envelope` with an empty copy list and every edit anyone
/// ever made is discarded, with a green test suite.
///
/// Both halves were verified by sabotage: untagged alone passed all 15 tests, untagged plus the
/// attribute failed `a_legacy_bare_edits_sidecar_loads_as_copy_zero_with_its_edits_intact`.
/// Explicit dispatch is immune to that combination, which is why it is worth the four extra
/// lines. The same class of trap already cost this project a test that passed while reading
/// nothing (`an_existing_linear_sidecar_still_loads`, M3.1b).
/// A stored label, with the values nobody chose removed.
///
/// ⚠⚠ Rule 1 exists because of real data: of three named copies in the author's folders, two
/// stored the *derived default* (`"DSCF6712_1.RAF"`), frozen because the old field displayed a
/// whole filename and committing it wrote that string back. Those are not labels.
///
/// ⚠ It is safe in the case that looks risky — if someone typed the default deliberately, this is
/// indistinguishable from the accident, and the rendered outcome is identical because the badge
/// shows the number either way.
///
/// ⚠ Deliberately **not** "strip a leading `{stem}_` prefix": same visible outcome by a rule that
/// can be wrong, since it would mangle a genuine label beginning with the stem.
pub fn normalise_label(file_name: &str, copy: u32, stored: Option<&str>) -> Option<String> {
    let stored = stored?;
    let bare = strip_extension(stored);
    if copy != 0 {
        let derived = crate::file_rename::default_copy_name(file_name, copy);
        if stored == derived || bare == strip_extension(&derived) {
            return None;
        }
    }
    if bare.is_empty() {
        return None;
    }
    Some(bare.to_string())
}

fn strip_extension(s: &str) -> &str {
    match s.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => s,
    }
}

pub fn load_all(photo: &Path) -> Sidecar {
    let Ok(text) = std::fs::read_to_string(path(photo)) else {
        return Sidecar::default();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Sidecar::default();
    };
    if value.get("copies").is_some() {
        let mut s: Sidecar = serde_json::from_value(value).unwrap_or_default();
        // ⚠ Applied on read rather than as a migration pass: no version bump, no backfill, and
        // the value normalises on the next write.
        let file_name = photo
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        for c in &mut s.copies {
            c.label = normalise_label(&file_name, c.copy, c.label.as_deref());
        }
        s
    } else {
        // Legacy: the whole file is one bare `Edits`, which is copy 0.
        match serde_json::from_value::<Edits>(value) {
            Ok(edits) => Sidecar {
                copies: vec![CopyEntry {
                    copy: 0,
                    label: None,
                    edits,
                    // A sidecar this old predates snapshots by definition.
                    snapshots: Vec::new(),
                    snapshot_hwm: 0,
                    uploads: Vec::new(),
                }],
                // A sidecar this old predates the mark; copy 0 is the only number in use.
                copy_hwm: 0,
            },
            Err(_) => Sidecar::default(),
        }
    }
}

/// Write every version, unconditionally.
pub fn save_all(photo: &Path, s: &Sidecar) -> Result<(), EditLoadError> {
    let text = serde_json::to_string_pretty(s)?;
    std::fs::write(path(photo), text)?;
    Ok(())
}

/// One copy's stored edits, or `None` when that copy has none.
pub fn load_copy(photo: &Path, copy: u32) -> Option<Edits> {
    load_all(photo).get(copy).map(|c| c.edits.clone())
}

/// Persist one copy's edits, leaving every other copy — and this copy's name — untouched.
///
/// ⚠ Read-modify-write, because several versions share one file. An overwrite would drop the
/// other copies, and it would drop this copy's name, which is not an edit.
pub fn persist_copy(photo: &Path, copy: u32, edits: &Edits) -> Result<(), EditLoadError> {
    let mut s = load_all(photo);
    s.set_edits(copy, edits.clone());
    persist_all(photo, &s)
}

/// Write every version, or delete the file when there is nothing worth keeping.
pub fn persist_all(photo: &Path, s: &Sidecar) -> Result<(), EditLoadError> {
    if s.is_neutral() {
        return remove(photo).map_err(EditLoadError::from);
    }
    save_all(photo, s)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unique temp directory for one test, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        /// ⚠ The tag is **not** relied on for uniqueness. Two tests that happened to pick the
        /// same tag would destroy each other's directory here — `remove_dir_all` runs first, and
        /// `cargo test` runs tests in parallel. That already happened once while writing the
        /// virtual-copies tests, and it surfaced as an unrelated pre-existing test failing during
        /// a sabotage run, which is the worst possible time to be chasing a phantom.
        ///
        /// Uniqueness is structural instead — process id plus a monotonic counter — the same
        /// remedy `image-decode`'s `temp_path` needed after taking uniqueness from the clock.
        fn new(tag: &str) -> Self {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static N: AtomicUsize = AtomicUsize::new(0);
            let n = N.fetch_add(1, Ordering::Relaxed);
            let p = std::env::temp_dir()
                .join(format!("rustroom_sidecar_{tag}_{}_{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
        fn photo(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn the_path_appends_json_to_the_whole_filename() {
        // Not "replace the extension": a RAW and a JPEG sharing a basename must get
        // distinct sidecars.
        assert_eq!(
            path(Path::new("/a/photo.RAF")),
            PathBuf::from("/a/photo.RAF.json")
        );
        assert_eq!(
            path(Path::new("/a/photo.jpg")),
            PathBuf::from("/a/photo.jpg.json")
        );
        assert_ne!(path(Path::new("/a/p.RAF")), path(Path::new("/a/p.jpg")));
    }

    #[test]
    fn save_then_load_round_trips() {
        let d = TempDir::new("roundtrip");
        let photo = d.photo("x.RAF");
        let e = Edits {
            exposure: 1.5,
            ..Edits::default()
        };
        save(&photo, &e).unwrap();
        assert_eq!(load(&photo), Some(e));
    }

    #[test]
    fn persisting_neutral_edits_deletes_the_sidecar() {
        // ⚠ The "Reset all" bug: `save` was skipped for neutral edits but the old file was
        // left behind, so switching away and back loaded the edits straight back in.
        let d = TempDir::new("persist_neutral");
        let photo = d.photo("x.RAF");
        persist(
            &photo,
            &Edits {
                exposure: 1.5,
                ..Edits::default()
            },
        )
        .unwrap();
        assert!(path(&photo).exists(), "a non-neutral edit should be stored");

        persist(&photo, &Edits::default()).unwrap();
        assert!(
            !path(&photo).exists(),
            "resetting to neutral must delete the sidecar, not leave a stale one"
        );
        assert_eq!(load(&photo), None);
    }

    #[test]
    fn persisting_neutral_edits_with_no_sidecar_is_not_an_error() {
        // The common case: open a photo, change nothing, switch away.
        let d = TempDir::new("persist_none");
        persist(&d.photo("x.RAF"), &Edits::default()).unwrap();
    }

    #[test]
    fn an_absent_or_malformed_sidecar_loads_as_none() {
        // The same forgiving posture the preset and settings stores take.
        let d = TempDir::new("absent");
        let photo = d.photo("x.RAF");
        assert_eq!(load(&photo), None);
        std::fs::write(path(&photo), "{ not json").unwrap();
        assert_eq!(load(&photo), None);
    }

    #[test]
    fn removing_an_absent_sidecar_is_not_an_error() {
        let d = TempDir::new("remove");
        let photo = d.photo("x.RAF");
        remove(&photo).unwrap();
        save(&photo, &Edits::default()).unwrap();
        assert!(path(&photo).exists());
        remove(&photo).unwrap();
        assert!(!path(&photo).exists());
    }

    #[test]
    fn a_legacy_bare_edits_sidecar_loads_as_copy_zero_with_its_edits_intact() {
        // ⚠⚠ THE test of this feature: every sidecar written before today is a bare `Edits`
        // object, and reading one as an empty copy list would discard a user's entire edit
        // history without failing anything.
        //
        // ⚠ It asserts the **payload**, not the variant — M3.1b's
        // `an_existing_linear_sidecar_still_loads` asserted only the variant and passed while
        // reading nothing at all. Watched failing against an untagged dispatch combined with
        // `#[serde(default)]` on `Sidecar`; see `load_all` for why it takes both.
        let dir = TempDir::new("legacy_load");
        let photo = dir.0.join("a.RAF");
        std::fs::write(
            path(&photo),
            r#"{"exposure":1.25,"contrast":40.0,"vibrance":-15.0}"#,
        )
        .unwrap();

        let s = load_all(&photo);
        assert_eq!(s.copies.len(), 1, "a legacy sidecar is exactly one copy");
        assert_eq!(s.copies[0].copy, 0);
        assert_eq!(s.copies[0].edits.exposure, 1.25, "exposure was lost");
        assert_eq!(s.copies[0].edits.contrast, 40.0, "contrast was lost");
        assert_eq!(s.copies[0].edits.vibrance, -15.0, "vibrance was lost");
    }

    #[test]
    fn an_envelope_round_trips_numbers_names_and_edits() {
        let dir = TempDir::new("envelope_round_trip");
        let photo = dir.0.join("a.RAF");
        let want = Sidecar {
            copies: vec![
                CopyEntry {
                    copy: 0,
                    label: None,
                    edits: Edits {
                        exposure: 0.5,
                        ..Edits::default()
                    },
                    snapshots: Vec::new(),
                    snapshot_hwm: 0,
                    uploads: Vec::new(),
                },
                CopyEntry {
                    copy: 3,
                    label: None,
                    edits: Edits {
                        saturation: -100.0,
                        ..Edits::default()
                    },
                    snapshots: Vec::new(),
                    snapshot_hwm: 0,
                    uploads: Vec::new(),
                },
            ],
            copy_hwm: 0,
        };
        save_all(&photo, &want).unwrap();
        assert_eq!(load_all(&photo), want);
    }

    #[test]
    fn an_absent_sidecar_is_an_empty_envelope() {
        let dir = TempDir::new("absent");
        assert_eq!(load_all(&dir.0.join("nope.RAF")), Sidecar::default());
    }

    #[test]
    fn a_malformed_sidecar_is_an_empty_envelope_rather_than_a_panic() {
        // The existing posture: a hand-edited file with a typo reads as "no edits", it does not
        // take anything down.
        let dir = TempDir::new("malformed");
        let photo = dir.0.join("a.RAF");
        std::fs::write(path(&photo), "{ not json at all").unwrap();
        assert_eq!(load_all(&photo), Sidecar::default());
    }

    /// ⚠⚠ **Deleting the newest copy must not free its number.** `delete_virtual_copy` retains
    /// every other copy and `ActionKind::CopyDelete` keeps the removed entry so undo can put it
    /// back — so if a *new* copy takes the freed number first, the undo lands a second copy with
    /// the same number. Two copies numbered 2 share an export filename and a history-stash key,
    /// and the catalog upserts on `(folder, file, copy)`, so the Library shows **one** row for
    /// them and the other is unreachable.
    ///
    /// The same bug snapshots had, in the mechanism snapshots copied their numbering from.
    #[test]
    fn deleting_the_newest_copy_does_not_free_its_number() {
        let mut s = Sidecar::default();
        s.set_edits(0, Edits::default());

        let a = s.next_copy();
        s.set_edits(a, Edits::default());
        let b = s.next_copy();
        s.set_edits(b, Edits::default());
        assert_eq!((a, b), (1, 2), "fixture: two copies, numbered 1 and 2");

        // What `delete_virtual_copy` does.
        s.copies.retain(|c| c.copy != b);

        assert_eq!(
            s.next_copy(),
            3,
            "2 was used once and must never be handed out again"
        );
    }

    /// ⚠⚠ **The collision the mark exists to prevent, through the real API.** Make two copies,
    /// delete the newest, make another, then undo the deletion — every copy must still have its
    /// own number. `restore_copy` is the funnel the undo arm goes through for exactly this.
    #[test]
    fn restoring_a_deleted_copy_after_making_another_cannot_duplicate_a_number() {
        let mut s = Sidecar::default();
        s.set_edits(0, Edits::default());
        s.set_edits(
            s.next_copy(),
            Edits {
                exposure: 1.0,
                ..Edits::default()
            },
        );
        let doomed = s.next_copy();
        s.set_edits(
            doomed,
            Edits {
                exposure: 2.0,
                ..Edits::default()
            },
        );

        let removed = s.get(doomed).cloned().expect("copy 2 exists");
        s.copies.retain(|c| c.copy != doomed);
        s.set_edits(
            s.next_copy(),
            Edits {
                exposure: 3.0,
                ..Edits::default()
            },
        );

        s.restore_copy(removed);

        let mut ns: Vec<u32> = s.copies.iter().map(|c| c.copy).collect();
        let before = ns.len();
        ns.sort_unstable();
        ns.dedup();
        assert_eq!(ns.len(), before, "every copy must have its own number");
        assert_eq!(ns, vec![0, 1, 2, 3]);
    }

    #[test]
    fn copy_numbers_are_never_reused() {
        // ⚠ Renumbering would silently change a copy's export filename and invalidate its
        // history-stash key — the undo stack would attach to a different version than the one
        // on screen.
        let mut s = Sidecar::default();
        assert_eq!(s.next_copy(), 1, "the first copy is 1, not 0");
        for n in 0..=2 {
            s.copies.push(CopyEntry {
                copy: n,
                label: None,
                edits: Edits::default(),
                snapshots: Vec::new(),
                snapshot_hwm: 0,
                uploads: Vec::new(),
            });
        }
        s.copies.retain(|c| c.copy != 1);
        assert_eq!(s.next_copy(), 3, "2 is gone but must not come back");
    }

    #[test]
    fn a_sidecar_holding_only_a_neutral_original_is_neutral() {
        // ⚠ Preserves the existing "Reset all deletes the sidecar" behaviour, which was itself a
        // bug fix: skipping the write left a stale file to be loaded straight back.
        let mut s = Sidecar::default();
        assert!(s.is_neutral(), "empty");
        s.copies.push(CopyEntry {
            copy: 0,
            label: None,
            edits: Edits::default(),
            snapshots: Vec::new(),
            snapshot_hwm: 0,
            uploads: Vec::new(),
        });
        assert!(s.is_neutral(), "a neutral, unnamed original only");

        let two = Sidecar {
            copies: vec![
                CopyEntry {
                    copy: 0,
                    label: None,
                    edits: Edits::default(),
                    snapshots: Vec::new(),
                    snapshot_hwm: 0,
                    uploads: Vec::new(),
                },
                CopyEntry {
                    copy: 1,
                    label: None,
                    edits: Edits::default(),
                    snapshots: Vec::new(),
                    snapshot_hwm: 0,
                    uploads: Vec::new(),
                },
            ],
            copy_hwm: 0,
        };
        assert!(!two.is_neutral(), "a copy is worth keeping the file for");
    }

    fn only_original(edits: Edits) -> Sidecar {
        let mut s = Sidecar::default();
        s.copies.push(CopyEntry {
            copy: 0,
            label: None,
            edits,
            snapshots: Vec::new(),
            snapshot_hwm: 0,
            uploads: Vec::new(),
        });
        s
    }

    /// ⚠⚠ THE M5.4b LEFTOVER, AS A TEST. Before the `Option`, an otherwise-untouched photo whose
    /// rendering was switched off read as `Edits::default()`, so `persist` deleted its sidecar
    /// and the preference switched the rendering back on at the next open. Decided off is not
    /// the default now, so the file is written and kept.
    #[test]
    fn a_decided_off_rendering_or_lens_is_worth_a_sidecar() {
        for edits in [
            Edits {
                rendering: Some(edit_model::Rendering::None),
                ..Edits::default()
            },
            Edits {
                lens_correction: Some(edit_model::LensCorrection::default()),
                ..Edits::default()
            },
        ] {
            assert!(
                !only_original(edits.clone()).is_neutral(),
                "decided-off read as neutral: {edits:?}"
            );
        }
    }

    /// And the other half: a fresh photo under off-preferences is `None`/`None`, which IS
    /// neutral, so no sidecar is written for it — no litter.
    #[test]
    fn an_undecided_photo_is_still_neutral() {
        assert!(only_original(Edits::default()).is_neutral());
    }

    /// End to end on disk: persist a decided-off rendering, load it back, and it is still
    /// decided off — the reported symptom was that it came back *on*.
    #[test]
    fn a_decided_off_rendering_survives_persist_and_load() {
        let dir = TempDir::new("decided_off");
        let photo = dir.0.join("a.RAF");
        let off = Edits {
            rendering: Some(edit_model::Rendering::None),
            ..Edits::default()
        };
        persist(&photo, &off).unwrap();
        assert!(path(&photo).exists(), "the sidecar was deleted as neutral");
        assert_eq!(load(&photo), Some(off));
    }
    /// A sidecar with copy 0 at `e0` and copy 1 at `e1`.
    fn two_copies(photo: &Path, e0: f32, e1: f32) {
        save_all(
            photo,
            &Sidecar {
                copies: vec![
                    CopyEntry {
                        copy: 0,
                        label: None,
                        edits: Edits {
                            exposure: e0,
                            ..Edits::default()
                        },
                        snapshots: Vec::new(),
                        snapshot_hwm: 0,
                        uploads: Vec::new(),
                    },
                    CopyEntry {
                        copy: 1,
                        label: None,
                        edits: Edits {
                            exposure: e1,
                            ..Edits::default()
                        },
                        snapshots: Vec::new(),
                        snapshot_hwm: 0,
                        uploads: Vec::new(),
                    },
                ],
                copy_hwm: 0,
            },
        )
        .unwrap();
    }

    #[test]
    fn persisting_one_copy_leaves_the_others_alone() {
        // ⚠ Read-modify-write, not overwrite. Two copies of one file live in one file, so
        // saving one must not drop the other.
        let dir = TempDir::new("persist_one");
        let photo = dir.0.join("a.RAF");
        two_copies(&photo, 1.0, 5.0);

        persist_copy(
            &photo,
            0,
            &Edits {
                exposure: 2.0,
                ..Edits::default()
            },
        )
        .unwrap();

        let s = load_all(&photo);
        assert_eq!(s.get(0).unwrap().edits.exposure, 2.0, "copy 0 not updated");
        assert_eq!(s.get(1).unwrap().edits.exposure, 5.0, "copy 1 lost");
    }

    #[test]
    fn resetting_the_only_copy_to_neutral_deletes_the_sidecar() {
        // The behaviour `persist` already had, preserved through the envelope: skipping the
        // write instead would leave a stale file to be loaded straight back — which is exactly
        // what "Reset all, switch away, switch back" used to do.
        let dir = TempDir::new("neutral_delete");
        let photo = dir.0.join("a.RAF");
        persist_copy(
            &photo,
            0,
            &Edits {
                exposure: 1.0,
                ..Edits::default()
            },
        )
        .unwrap();
        assert!(path(&photo).exists());
        persist_copy(&photo, 0, &Edits::default()).unwrap();
        assert!(!path(&photo).exists(), "a neutral original leaves no file");
    }

    #[test]
    fn resetting_one_of_two_copies_keeps_the_sidecar() {
        // ⚠ The case the naive rule gets wrong: "edits are neutral, so delete the file" would
        // throw away the *other* copy.
        let dir = TempDir::new("neutral_two");
        let photo = dir.0.join("a.RAF");
        two_copies(&photo, 1.0, 2.0);
        persist_copy(&photo, 0, &Edits::default()).unwrap();
        assert!(path(&photo).exists(), "copy 1 must survive");
        assert_eq!(load_all(&photo).get(1).unwrap().edits.exposure, 2.0);
    }

    #[test]
    fn loading_one_copy_returns_only_that_copys_edits() {
        let dir = TempDir::new("load_one");
        let photo = dir.0.join("a.RAF");
        two_copies(&photo, 1.0, 2.0);
        assert_eq!(load_copy(&photo, 0).unwrap().exposure, 1.0);
        assert_eq!(load_copy(&photo, 1).unwrap().exposure, 2.0);
        assert!(load_copy(&photo, 9).is_none(), "an absent copy is None");
    }
    /// ⚠⚠ The two arms of the read rule, and the first is the one that matters. Real sidecars in
    /// the author's folders stored `"DSCF6712_1.RAF"` — the *derived default*, frozen because the
    /// field displayed a whole filename and committing it wrote that string back. A label nobody
    /// chose must not survive as one.
    #[test]
    fn a_stored_derived_default_reads_as_unlabelled() {
        assert_eq!(
            normalise_label("DSCF6712.RAF", 1, Some("DSCF6712_1.RAF")),
            None
        );
        // The same string without its extension is equally derived.
        assert_eq!(normalise_label("DSCF6712.RAF", 1, Some("DSCF6712_1")), None);
    }

    #[test]
    fn a_real_label_survives_with_any_extension_stripped() {
        assert_eq!(
            normalise_label("DSCF6237.RAF", 1, Some("DSC")),
            Some("DSC".into())
        );
        assert_eq!(
            normalise_label("DSCF6237.RAF", 1, Some("Warm.RAF")),
            Some("Warm".into())
        );
    }

    /// ⚠ A label test needs a labelled *and* an unlabelled case: a fixture with only one cannot
    /// tell "label-or-number" from "number always".
    #[test]
    fn an_absent_label_stays_absent() {
        assert_eq!(normalise_label("DSCF6237.RAF", 1, None), None);
        assert_eq!(normalise_label("DSCF6237.RAF", 0, None), None);
    }

    /// ⚠ Copy 0 may be labelled now, so the derived-default rule must not fire on it — there is
    /// no derived default for the original, whose name is simply the file's.
    #[test]
    fn copy_zero_keeps_a_label() {
        assert_eq!(
            normalise_label("DSCF6237.RAF", 0, Some("Colour")),
            Some("Colour".into())
        );
    }

    /// The `serde` alias is what lets every sidecar written before this change load unchanged.
    #[test]
    fn a_sidecar_written_with_name_still_loads() {
        let json = r#"{"copies":[{"copy":1,"name":"DSC","edits":{}}],"copy_hwm":1}"#;
        let s: Sidecar = serde_json::from_str(json).expect("must parse");
        assert_eq!(s.copies[0].label.as_deref(), Some("DSC"));
    }

    /// ⚠⚠ Copy 0 may be labelled, and it must **persist**. `set_copy_name` refused the original
    /// outright, because a copy-0 "name" would have been a second filename beside the real one.
    /// A label is not a filename, so the refusal has no premise left and "Colour" beside "B&W"
    /// is expressible.
    #[test]
    fn a_label_on_the_original_round_trips() {
        let mut s = Sidecar::default();
        s.set_label(0, Some("Colour".into()));
        s.set_label(1, Some("B&W".into()));
        let json = serde_json::to_string(&s).expect("serialises");
        let back: Sidecar = serde_json::from_str(&json).expect("parses");
        assert_eq!(back.label_of(0), Some("Colour"));
        assert_eq!(back.label_of(1), Some("B&W"));
    }
}

#[cfg(test)]
mod naming_tests {
    use super::*;

    fn edited(v: f32) -> Edits {
        Edits {
            exposure: v,
            ..Edits::default()
        }
    }

    #[test]
    fn naming_a_copy_leaves_every_edit_and_the_other_copies_alone() {
        // ⚠⚠ This is the guard for what was reported: renaming a copy used to rename the *file*,
        // which every sibling copy shares. A name now touches exactly one copy's `name` and
        // nothing else — not the edits, not the siblings, not the file.
        let mut s = Sidecar::default();
        s.set_edits(0, edited(1.0));
        s.set_edits(1, edited(2.0));
        s.set_edits(2, edited(3.0));

        s.set_label(1, Some("Warm".into()));

        assert_eq!(s.label_of(1), Some("Warm"));
        assert_eq!(s.label_of(0), None, "the original is untouched");
        assert_eq!(s.label_of(2), None, "so is the sibling");
        assert_eq!(s.get(0).unwrap().edits, edited(1.0));
        assert_eq!(
            s.get(1).unwrap().edits,
            edited(2.0),
            "its own edits survive"
        );
        assert_eq!(s.get(2).unwrap().edits, edited(3.0));
    }

    #[test]
    fn clearing_a_name_puts_the_copy_back_to_its_number() {
        let mut s = Sidecar::default();
        s.set_edits(1, edited(2.0));
        s.set_label(1, Some("Warm".into()));
        s.set_label(1, None);
        assert_eq!(s.label_of(1), None);
        assert_eq!(
            s.get(1).unwrap().edits,
            edited(2.0),
            "undo must not reset edits"
        );
    }

    #[test]
    fn a_named_copy_keeps_the_sidecar_alive_even_when_neutral() {
        // ⚠ Otherwise naming a copy and then resetting it would delete the copy, because
        // `persist_all` removes a sidecar it considers neutral.
        let mut s = Sidecar::default();
        s.set_label(0, Some("named".into()));
        assert!(
            !s.is_neutral(),
            "a name is worth a file even with neutral edits"
        );
    }

    #[test]
    fn setting_edits_does_not_clear_a_name() {
        // ⚠ Every slider drag calls `set_edits`. Clearing the name there would erase it on the
        // next keystroke after renaming — invisible until the export came out wrong.
        let mut s = Sidecar::default();
        s.set_label(1, Some("Warm".into()));
        s.set_edits(1, edited(2.0));
        assert_eq!(s.label_of(1), Some("Warm"));
    }
}

#[cfg(test)]
mod compact_tests {
    use super::*;

    fn edited(v: f32) -> Edits {
        Edits {
            exposure: v,
            ..Edits::default()
        }
    }

    #[test]
    fn deleting_the_first_copy_makes_the_second_copy_one_again() {
        // ⚠ The case this exists for: two copies, delete the first, and `_2` is left with no
        // `_1` beside it.
        let mut s = Sidecar::default();
        s.set_edits(0, edited(1.0));
        s.set_edits(2, edited(3.0));

        let moves = s.compact();

        assert_eq!(moves, vec![(2, 1)]);
        assert_eq!(
            s.get(1).unwrap().edits,
            edited(3.0),
            "it kept its own edits"
        );
        assert!(s.get(2).is_none(), "and vacated its old number");
        assert_eq!(
            s.get(0).unwrap().edits,
            edited(1.0),
            "the original is untouched"
        );
    }

    #[test]
    fn already_contiguous_copies_do_not_move() {
        // ⚠ An empty move list is what lets the caller skip writing the file and dropping undo
        // stacks for nothing.
        let mut s = Sidecar::default();
        s.set_edits(0, edited(1.0));
        s.set_edits(1, edited(2.0));
        s.set_edits(2, edited(3.0));
        assert!(s.compact().is_empty());
        assert_eq!(s.get(1).unwrap().edits, edited(2.0));
        assert_eq!(s.get(2).unwrap().edits, edited(3.0));
    }

    #[test]
    fn copies_keep_their_order_and_their_names() {
        // ⚠ Ascending by old number, so the *order* people see never changes — only the gaps
        // close. Renumbering that also reordered would move photos around the grid.
        let mut s = Sidecar::default();
        s.set_edits(3, edited(3.0));
        s.set_edits(7, edited(7.0));
        s.set_label(7, Some("Warm".into()));
        s.set_edits(5, edited(5.0));

        assert_eq!(s.compact(), vec![(3, 1), (5, 2), (7, 3)]);

        assert_eq!(s.get(1).unwrap().edits, edited(3.0));
        assert_eq!(s.get(2).unwrap().edits, edited(5.0));
        assert_eq!(s.get(3).unwrap().edits, edited(7.0));
        assert_eq!(
            s.label_of(3),
            Some("Warm"),
            "a named copy keeps its name, so its export filename does not move"
        );
    }

    #[test]
    fn a_file_with_no_stored_original_still_numbers_its_copies_from_one() {
        // ⚠ A sidecar can hold copies and no copy 0 — `persist_copy(.., 1, ..)` on an unedited
        // photo does exactly that. Numbering from 0 here would make a copy claim to be the file.
        let mut s = Sidecar::default();
        s.set_edits(4, edited(4.0));
        assert_eq!(s.compact(), vec![(4, 1)]);
        assert!(s.get(0).is_none());
        assert_eq!(s.get(1).unwrap().edits, edited(4.0));
    }

    #[test]
    fn the_next_copy_number_follows_a_compaction() {
        // ⚠ `next_copy` is `max + 1`; after compacting it must not hand out a number that is
        // already taken, or a new copy would overwrite an existing one.
        let mut s = Sidecar::default();
        s.set_edits(3, edited(3.0));
        s.set_edits(9, edited(9.0));
        s.compact();
        assert_eq!(s.next_copy(), 3);
        assert!(s.get(3).is_none(), "and that number really is free");
    }
}

/// Snapshots: labelling, storage, numbering and the neutrality rule.
///
/// ⚠ Its own module rather than more of `tests`, following this file's existing partition —
/// `naming_tests` and `compact_tests` are organised the same way.
#[cfg(test)]
mod snapshot_tests {
    use super::*;

    /// A unique temp directory for one test, removed on drop.
    ///
    /// ⚠ Uniqueness is **structural** — process id plus a counter — never the tag alone. Two
    /// tests sharing a tag destroy each other's directory here, because `remove_dir_all` runs
    /// first and `cargo test` runs in parallel. That has already happened once in this file.
    struct Dir(PathBuf);

    impl Dir {
        fn new(tag: &str) -> Self {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static N: AtomicUsize = AtomicUsize::new(0);
            let n = N.fetch_add(1, Ordering::Relaxed);
            let p = std::env::temp_dir()
                .join(format!("rustroom_snap_{tag}_{}_{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn snap(n: u32, exposure: f32) -> Snapshot {
        Snapshot {
            n,
            name: None,
            taken: 0,
            edits: Edits {
                exposure,
                ..Edits::default()
            },
        }
    }

    #[test]
    fn an_unnamed_snapshot_is_labelled_by_its_number() {
        assert_eq!(snap(3, 0.0).label(), "Snapshot 3");
    }

    #[test]
    fn a_named_snapshot_is_labelled_by_its_name() {
        let mut s = snap(3, 0.0);
        s.name = Some("Before the crop".to_string());
        assert_eq!(s.label(), "Before the crop");
    }

    /// ⚠ The guard on the field's `#[serde(default)]`. Without it every sidecar written before
    /// this feature fails to parse, `load_all` returns `Sidecar::default()`, and **every edit
    /// anyone has ever made is silently discarded** with no error anywhere.
    #[test]
    fn a_sidecar_from_before_snapshots_still_loads_its_edits() {
        let dir = Dir::new("presnap");
        let photo = dir.0.join("a.RAF");
        std::fs::write(
            path(&photo),
            r#"{"copies":[{"copy":0,"name":null,"edits":{"exposure":1.5}}]}"#,
        )
        .unwrap();

        let s = load_all(&photo);
        assert_eq!(s.copies.len(), 1, "the copy must survive");
        assert_eq!(s.copies[0].edits.exposure, 1.5, "its edits must survive");
        assert!(
            s.copies[0].snapshots.is_empty(),
            "and it must read as having no snapshots"
        );
    }

    #[test]
    fn snapshots_round_trip_through_the_file() {
        let dir = Dir::new("roundtrip");
        let photo = dir.0.join("a.RAF");
        let mut s = Sidecar::default();
        s.set_edits(0, Edits::default());
        let mut one = snap(1, 0.75);
        one.name = Some("Warm".to_string());
        one.taken = 1_756_000_000;
        s.copies[0].snapshots.push(one);
        save_all(&photo, &s).unwrap();

        assert_eq!(
            load_all(&photo),
            s,
            "the whole envelope must survive the round trip"
        );
    }

    /// ⚠⚠ Take a snapshot, then press "Reset adjustments": `persist_all` must **not** delete the
    /// sidecar. `is_neutral` already documents this hazard for names — "a name or a second copy
    /// is worth a file even with neutral edits" — and snapshots are the third case of that rule.
    #[test]
    fn resetting_a_photo_does_not_delete_its_snapshots() {
        let dir = Dir::new("neutral");
        let photo = dir.0.join("a.RAF");
        let mut s = Sidecar::default();
        s.set_edits(0, Edits::default());
        s.copies[0].snapshots.push(snap(1, 2.0));

        assert!(
            !s.is_neutral(),
            "a copy holding snapshots is worth a file however neutral its own edits are"
        );
        persist_all(&photo, &s).unwrap();
        assert!(path(&photo).exists(), "the sidecar must still be on disk");
        assert_eq!(load_all(&photo).copies[0].snapshots.len(), 1);
    }

    /// The other half: nothing stored at all is still worth deleting.
    #[test]
    fn a_neutral_photo_with_no_snapshots_is_still_neutral() {
        let mut s = Sidecar::default();
        s.set_edits(0, Edits::default());
        assert!(s.is_neutral());
    }

    /// ⚠ `max + 1`, never `count + 1`. Take three, delete the second, take a fourth: it must be
    /// `Snapshot 4`. Reusing `3` would point the new row at an existing label.
    #[test]
    fn snapshot_numbers_are_never_reused() {
        let mut s = Sidecar::default();
        s.set_edits(0, Edits::default());
        for i in 1..=3 {
            let n = s.next_snapshot(0);
            assert_eq!(n, i);
            s.add_snapshot(0, snap(n, i as f32));
        }
        s.remove_snapshot(0, 2);
        assert_eq!(
            s.next_snapshot(0),
            4,
            "the highest ever used is 3, so the next is 4"
        );
    }

    /// ⚠⚠ **Deleting the newest snapshot must not free its number**, or undoing that deletion
    /// after taking another leaves two snapshots numbered the same in one copy — where a rename
    /// or a delete would hit whichever came first. The live maximum alone cannot see a number
    /// that only the undo log still holds, which is what `snapshot_hwm` is for.
    #[test]
    fn deleting_the_newest_snapshot_does_not_free_its_number() {
        let mut s = Sidecar::default();
        s.set_edits(0, Edits::default());
        s.add_snapshot(0, snap(1, 1.0));
        s.add_snapshot(0, snap(2, 2.0));
        s.remove_snapshot(0, 2);
        assert_eq!(
            s.next_snapshot(0),
            3,
            "2 was used once and must never be handed out again"
        );
    }

    /// ⚠ A sidecar written before the high-water mark existed reads it as `0`, so numbering must
    /// fall back to the live maximum rather than restarting at 1 and colliding.
    #[test]
    fn a_snapshot_list_with_no_recorded_mark_still_numbers_forward() {
        let mut s = Sidecar::default();
        s.set_edits(0, Edits::default());
        s.copies[0].snapshots.push(snap(1, 1.0));
        s.copies[0].snapshots.push(snap(2, 2.0));
        assert_eq!(s.copies[0].snapshot_hwm, 0, "fixture: no mark recorded");
        assert_eq!(s.next_snapshot(0), 3);
    }

    /// Numbering is per copy: copy 1's first snapshot is 1 even when copy 0 has two.
    #[test]
    fn snapshot_numbering_is_per_copy() {
        let mut s = Sidecar::default();
        s.set_edits(0, Edits::default());
        s.set_edits(1, Edits::default());
        s.add_snapshot(0, snap(1, 1.0));
        s.add_snapshot(0, snap(2, 2.0));
        assert_eq!(s.next_snapshot(1), 1);
    }

    #[test]
    fn removing_a_snapshot_returns_it_so_undo_can_put_it_back() {
        let mut s = Sidecar::default();
        s.set_edits(0, Edits::default());
        s.add_snapshot(0, snap(1, 1.0));
        s.add_snapshot(0, snap(2, 2.0));

        let taken = s.remove_snapshot(0, 1).expect("snapshot 1 exists");
        assert_eq!(taken.n, 1);
        assert_eq!(taken.edits.exposure, 1.0);
        assert_eq!(s.snapshots_of(0).len(), 1);
        assert!(s.remove_snapshot(0, 1).is_none(), "removing twice is None");
    }

    #[test]
    fn a_snapshot_can_be_renamed_and_unnamed() {
        let mut s = Sidecar::default();
        s.set_edits(0, Edits::default());
        s.add_snapshot(0, snap(1, 1.0));

        assert!(s.set_snapshot_name(0, 1, Some("Warm".to_string())));
        assert_eq!(s.snapshots_of(0)[0].label(), "Warm");
        assert!(s.set_snapshot_name(0, 1, None));
        assert_eq!(s.snapshots_of(0)[0].label(), "Snapshot 1");
        assert!(
            !s.set_snapshot_name(0, 9, Some("nope".to_string())),
            "renaming a snapshot that is not there must report that it did nothing"
        );
    }

    /// Adding to a copy with no entry yet creates it — the shape `set_edits` already has.
    #[test]
    fn adding_a_snapshot_to_an_absent_copy_creates_it() {
        let mut s = Sidecar::default();
        s.add_snapshot(2, snap(1, 1.0));
        assert_eq!(s.snapshots_of(2).len(), 1);
        assert_eq!(s.copies[0].copy, 2);
    }

    #[test]
    fn snapshots_of_an_absent_copy_is_empty() {
        assert!(Sidecar::default().snapshots_of(7).is_empty());
    }

    /// ⚠ Keyed by **host**: the same photo may be uploaded to two servers, and a bare "uploaded"
    /// flag would lie on the second one.
    #[test]
    fn an_upload_record_round_trips_per_host() {
        let mut sc = Sidecar::default();
        sc.set_edits(0, Edits::default());
        sc.copies[0].uploads.push(UploadRecord {
            host: "immich.example".to_string(),
            asset_id: "abc".to_string(),
            status: "created".to_string(),
            at: "2026-09-24T12:00:00.000Z".to_string(),
        });
        let back: Sidecar = serde_json::from_str(&serde_json::to_string(&sc).unwrap()).unwrap();
        assert_eq!(back.copies[0].uploads.len(), 1);
        assert_eq!(back.copies[0].uploads[0].asset_id, "abc");
    }

    /// ⚠⚠ A sidecar written before this field must still load **with its edits intact** — the
    /// container-level-`serde(default)` trap this file documents is about `Sidecar`; this field is
    /// on `CopyEntry` and takes a **field-level** default only.
    #[test]
    fn a_copy_entry_from_before_uploads_still_loads() {
        let json = r#"{"copy":0,"edits":{"exposure":1.5}}"#;
        let e: CopyEntry = serde_json::from_str(json).unwrap();
        assert_eq!(e.copy, 0);
        assert_eq!(e.edits.exposure, 1.5, "the edits must survive");
        assert!(e.uploads.is_empty());
    }
}
