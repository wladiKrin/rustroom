//! The export worker: planning a run, answering collisions, and writing the batch.
//!
//! ⚠ Split out of `batch_export.rs` on 2026-09-18 as a **pure move** — the `prefix_cache.rs`
//! precedent, with the test count unchanged and no golden moved. Every `⚠` below is the record of
//! a decision or a bug; **do not reword one.**
//!
//! ⚠⚠ **This file is over the project's ~600-line marker and that is accepted rather than
//! overlooked**: most of it is the tests of the code above. Splitting a worker's tests away from
//! the worker to reach a number would be worse than the number.

use super::*;
use crate::filename_template::{NameFacts, Template};
use edit_model::Edits;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

/// Progress from the worker.
#[derive(Debug, Clone, PartialEq)]
pub enum ExportEvent {
    Started {
        total: usize,
    },
    Item {
        done: usize,
        total: usize,
        name: String,
    },
    /// The worker is now **blocked** waiting for a [`Decision`].
    Collision {
        dest: PathBuf,
    },
    /// An upload did not confirm, and the file is at `at`. Sent for the **first** one only.
    ///
    /// ⚠⚠ The run **keeps going** — a network that dropped one upload has not invalidated the rest.
    /// ⚠ The report's `unconfirmed` list still holds every one; this is an addition for the first,
    /// not a replacement.
    UploadUnconfirmed {
        at: PathBuf,
        why: String,
    },
    Finished(ExportReport),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExportReport {
    pub exported: usize,
    pub skipped: usize,
    pub failed: Vec<(PathBuf, String)>,
    pub cancelled: bool,
    /// Files the server confirmed it stored (whether `created` or `duplicate`).
    pub uploaded: usize,
    /// Files written but whose upload did not confirm, with why.
    ///
    /// ⚠⚠ **"Unconfirmed", not "failed", and never retried automatically.** An SSL/EOF error
    /// means the *response* was lost, not that the server rejected the upload — measured on this
    /// tailnet during the 2026-09-24 stacking migration, 4 writes in 2,373 failed that way and
    /// **none had applied**, but that had to be *checked* rather than assumed. An automatic retry
    /// is how one edit becomes three assets.
    pub unconfirmed: Vec<(PathBuf, String)>,
    /// What the run could not do that every photo was meant to have — today, an image watermark
    /// whose copy could not be read (M10.10a). Each is a phrase the status line ends with.
    pub notes: Vec<String>,
    /// Photos whose print size their pixels could not fill, so their files carry a lower PPI.
    pub below_ppi: usize,
    /// Photos made smaller than asked to fit the file-size limit.
    pub shrunk: usize,
}

impl ExportReport {
    /// The status line: `Exported 3, 1 skipped, 1 failed, cancelled`, then each note.
    ///
    /// ⚠ Here rather than in the flow that shows it, so the line can be tested without a window.
    pub fn summary(&self) -> String {
        let mut parts = vec![format!("Exported {}", self.exported)];
        if self.skipped > 0 {
            parts.push(format!("{} skipped", self.skipped));
        }
        if !self.failed.is_empty() {
            parts.push(format!("{} failed", self.failed.len()));
        }
        if self.cancelled {
            parts.push("cancelled".to_string());
        }
        parts.extend(self.notes.iter().cloned());
        parts.join(", ")
    }
}

/// A running batch export.
///
/// Owns a thread that works through the jobs one at a time. The thread **blocks** on
/// `decision_rx` whenever it needs a collision answered, which is why the UI must drain
/// `events()` every frame and reply with [`Self::decide`].
pub struct ExportWorker {
    events: Receiver<ExportEvent>,
    decisions: Sender<Decision>,
    cancel: Arc<AtomicBool>,
}

impl ExportWorker {
    /// Start exporting `jobs` into `opts.dest_dir`.
    ///
    /// `cancel` is an input rather than something built here so that a test can flip it from
    /// *inside* the injected decode fn, which is the only way to exercise the mid-photo
    /// cancel check deterministically.
    ///
    /// ⚠ [`ExportRender`] is a parameter rather than a field of [`ExportOptions`], and that is the
    /// whole reason there is one setting instead of two: in `ExportOptions` it would be saved
    /// into every export preset, so a preset could disagree with the preview about which kernel
    /// the photograph is rendered with — and, since M8.9, about whether it renders on the GPU.
    /// Two facts that can disagree.
    #[allow(clippy::too_many_arguments)]
    pub fn spawn(
        jobs: Vec<ExportJob>,
        opts: ExportOptions,
        // ⚠ `+ Sync` because the workers share one closure across threads. The production
        // closure captures a `Demosaic` (`Copy`) and an `Option<PathBuf>` it clones per call, so
        // it already is — this narrows the bound rather than forcing a change.
        decode: impl Fn(&Path) -> Result<ExportSource, String> + Send + Sync + 'static,
        cancel: Arc<AtomicBool>,
        render: ExportRender,
        // ⚠ From `Settings::export_width`, already repaired — see its doc for why the ceiling is
        // the core count and why this is a setting rather than a constant.
        width: usize,
        // ⚠ Where creative looks and watermark images are read from. `None` for either means none
        // are available, and every photo then exports as though the file it names were missing —
        // the documented fallback for a look, a note in the report for a watermark.
        dirs: ExportDirs,
        // ⚠ `None` means "write to disk only", which is the ordinary case and not an error. A
        // narrow struct rather than `Settings`, so the worker cannot read preferences that are none
        // of its business — see `immich_upload::UploadConfig`.
        upload: Option<crate::immich_upload::UploadConfig>,
    ) -> Self {
        let (event_tx, events) = channel();
        let (decisions, decision_rx) = channel();
        let flag = cancel.clone();
        std::thread::spawn(move || {
            // ⚠⚠ **Resolved on the WORKER thread, not in `spawn`.** A 64³ cube is 786 KB read from
            // disk; doing it in `spawn` would block the UI thread that starts the export.
            let batch = Batch::read(&jobs, &opts, dirs);
            run(
                jobs,
                opts,
                decode,
                event_tx,
                decision_rx,
                flag,
                render,
                width,
                &batch,
                upload,
            );
        });
        Self {
            events,
            decisions,
            cancel,
        }
    }

    pub fn events(&self) -> &Receiver<ExportEvent> {
        &self.events
    }

    /// Answer the outstanding [`ExportEvent::Collision`], unblocking the worker.
    pub fn decide(&self, decision: Decision) {
        let _ = self.decisions.send(decision);
    }

    /// Ask the worker to stop.
    ///
    /// **The photo in flight is not exported.** libraw's `unpack` cannot be interrupted, so
    /// its decode runs to completion on the worker thread — but the result is discarded
    /// rather than written, and the caller can drop this `ExportWorker` immediately instead
    /// of waiting. That is the same "abandon, don't abort" trade `DecodeWorker` makes for
    /// superseded previews: identical outcome to a true abort, no unsafe FFI.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// One photo, with its destination already settled.
///
/// ⚠⚠ **Everything that needs a decision is resolved before this exists**, which is what lets an
/// export run several photos at once: the phase that decodes has no dialogs in it, no `claimed`
/// set, and no shared mutable naming state — only work.
struct Planned {
    source: PathBuf,
    dest: PathBuf,
    edits: Edits,
    meta: export::FileMetadata,
    /// ⚠ Carried for the upload's `fileCreatedAt`, which must be the **capture** time.
    capture: core_image::Capture,
    /// ⚠⚠ Carried so an upload is recorded against the copy that was exported. Writing copy 0
    /// instead would silence the dialog's "already uploaded" note for every virtual copy.
    copy: u32,
    /// The sidecar's mtime, for `UploadDate::Edited`.
    ///
    /// ⚠⚠ **Settled HERE, at plan time, and not at upload time.** `immich_upload::record` writes
    /// the upload into the photo's sidecar, which bumps that very mtime — and `edited_at` *is* that
    /// mtime. Read late, "the edit date" silently becomes "the date of the last upload", and would
    /// differ between the first upload and every one after.
    edited_at: Option<i64>,
}

/// Settle every filename and every collision, without decoding anything.
///
/// ⚠⚠ **Up front rather than as the run goes, and that is a deliberate change in what the person
/// exporting sees** (user's call, 2026-09-15). The old loop **blocked** on a decision mid-run — it
/// asked about photo 3 while 4–20 waited — which concurrently is a stall behind one dialog and a
/// state that has to be reasoned about per photo. Settled first, the prompts all arrive before
/// anything starts: answer once, or once with *apply to all*, and walk away.
///
/// ⚠ Affordable because it is a `stat` per photo — milliseconds for a hundred — where a decode is
/// seconds.
///
/// ⚠⚠ **`claimed` records a name when it is PLANNED, not when it is written.** The old loop
/// inserted after a successful export, so a photo whose export *failed* left its name free for a
/// later one to take. Reserving at plan time is the only reading that works with several photos in
/// flight, and it is also the one under which two photos can never race for one name.
///
/// `None` means the event channel is gone — the app is going away, so stop rather than spin, and
/// do not bother sending a report nobody will read.
#[allow(clippy::too_many_arguments)]
fn plan(
    jobs: Vec<ExportJob>,
    opts: &ExportOptions,
    template: &Template,
    events: &Sender<ExportEvent>,
    decisions: &Receiver<Decision>,
    cancel: &AtomicBool,
    report: &mut ExportReport,
    // ⚠ The directory files are actually written to: the staging directory under Immich-only, and
    // `opts.dest_dir` otherwise. `opts` is still passed for everything else.
    out_dir: &Path,
    // ⚠⚠ False for a staging run: a **fresh temp directory cannot collide with anything**, so
    // asking the user about one would be a question with no meaning. ⚠ `claimed` still applies —
    // two sources sharing a stem must not overwrite each other inside the staging directory.
    check_collisions: bool,
) -> Option<Vec<Planned>> {
    // Names produced during this run. ⚠ Without this, two sources sharing a stem
    // (`a.RAF`, `a.jpg`) would silently overwrite each other in an empty folder.
    let mut claimed: HashSet<PathBuf> = HashSet::new();
    let mut remembered: Option<Collision> = None;
    let mut out: Vec<Planned> = Vec::new();

    for (seq, job) in jobs.into_iter().enumerate() {
        // ⚠ Resolved per job, before the loop takes the job apart: the preset's defaults under
        // this photo's own words, cut to the scope. `export_one` then only embeds.
        let meta = export::FileMetadata::resolve(&opts.metadata, file_metadata(&job));
        let ExportJob {
            id,
            name,
            edits,
            capture,
            ..
        } = job;
        let source = id.path.clone();
        // ⚠ A second handle, because `source` is moved into the `Planned` literal below and the
        // sidecar's mtime has to be read from the same path.
        let source_for_edit = source.clone();
        if cancel.load(Ordering::Relaxed) {
            report.cancelled = true;
            break;
        }

        // ⚠ `{seq}` is 1-based and positional in the job list, which is the order the Library is
        // showing. It is assigned here, *before* the collision check, so a skipped photo does not
        // renumber the ones after it — a name is a property of the photo, not of what happened to
        // its neighbours.
        let base = template.render(&NameFacts {
            file_stem: source
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("export"),
            copy: id.copy,
            copy_label: name.as_deref(),
            seq: seq + 1,
            taken: capture.taken,
            camera: capture.camera.as_deref(),
        });
        let mut dest = dest_named(out_dir, &base, 0, opts.format);
        // ⚠ Under a staging run only `claimed` matters: the directory is new, so `exists()` can
        // only ever be a name this same run already reserved.
        if claimed.contains(&dest) || (check_collisions && dest.exists()) {
            if !check_collisions {
                // ⚠ Two sources sharing a stem inside the staging directory — resolved silently by
                // suffixing, because there is no user-visible file to ask about.
                let mut i = 1;
                loop {
                    let candidate = dest_named(out_dir, &base, i, opts.format);
                    if !claimed.contains(&candidate) {
                        dest = candidate;
                        break;
                    }
                    i += 1;
                }
                claimed.insert(dest.clone());
                out.push(Planned {
                    source,
                    dest,
                    edits,
                    meta,
                    capture,
                    copy: id.copy,
                    edited_at: sidecar_seconds(&source_for_edit),
                });
                continue;
            }
            let collision = match remembered {
                Some(c) => c,
                None => {
                    events
                        .send(ExportEvent::Collision { dest: dest.clone() })
                        .ok()?;
                    // Blocks until the UI answers. A disconnected channel means the app is
                    // going away, so stop rather than spin.
                    match decisions.recv() {
                        Ok(Decision::Resolve {
                            collision,
                            apply_to_all,
                        }) => {
                            if apply_to_all {
                                remembered = Some(collision);
                            }
                            collision
                        }
                        Ok(Decision::Cancel) | Err(_) => {
                            report.cancelled = true;
                            break;
                        }
                    }
                }
            };
            match collision {
                Collision::Skip => {
                    // ⚠ A skipped photo is dropped here and never reaches the export, so it is
                    // absent from `total` as well. It is reported by `report.skipped`, which the
                    // dialog already shows — counting it in the progress line would mean counting
                    // a photo that was never exported.
                    report.skipped += 1;
                    continue;
                }
                Collision::Overwrite => {}
                Collision::KeepBoth => {
                    let mut i = 1;
                    loop {
                        // ⚠ The *same* base, so "Keep both" appends `-N` to whatever the
                        // template produced rather than quietly falling back to the old naming.
                        let candidate = dest_named(out_dir, &base, i, opts.format);
                        if !claimed.contains(&candidate) && !candidate.exists() {
                            dest = candidate;
                            break;
                        }
                        i += 1;
                    }
                }
            }
        }

        claimed.insert(dest.clone());
        out.push(Planned {
            source,
            dest,
            edits,
            meta,
            capture,
            copy: id.copy,
            edited_at: sidecar_seconds(&source_for_edit),
        });
    }
    Some(out)
}

/// The photo's sidecar mtime in seconds, for `UploadDate::Edited`.
fn sidecar_seconds(photo: &Path) -> Option<i64> {
    crate::sidecar::edited_at(photo).and_then(|t| {
        t.duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|d| d.as_secs() as i64)
    })
}

/// A per-run directory for files that are uploaded and **not kept**.
///
/// ⚠ Keyed on the process id **and** a counter — `immich_list_cache`'s pattern. The two sites in
/// `immich_source` that key on the pid alone are the ones **not** to copy: two runs in one process
/// would otherwise share a directory.
static STAGE_SEQ: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Make the staging directory for a run, or `None` if it cannot be created.
///
/// ⚠ `None` is not fatal: the caller falls back to `dest_dir`, which is worse for tidiness and much
/// better than failing a whole export over a temp directory.
fn staging_dir() -> Option<PathBuf> {
    let d = std::env::temp_dir().join(format!(
        "rustroom_export_stage_{}_{}",
        std::process::id(),
        STAGE_SEQ.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&d).ok().map(|_| d)
}

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

/// The upload half of a run.
///
/// ⚠ Built **once per run**, outside the worker pool: it holds the `Client`, and constructing one
/// per photo would re-parse the URL for every photograph for no reason.
struct Uploader {
    client: immich::Client,
    host: String,
}

impl Uploader {
    /// `None` when the connection will not build — a malformed URL, say. Not an error: the export
    /// still runs and every photo is reported unconfirmed, which is visible without being fatal.
    fn new(cfg: crate::immich_upload::UploadConfig) -> Option<Self> {
        let client = immich::Client::new(&cfg.base_url, &cfg.key).ok()?;
        Some(Self {
            client,
            host: cfg.base_url,
        })
    }

    /// Upload one written file. `Err` carries a message for `unconfirmed`.
    #[allow(clippy::too_many_arguments)]
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
}

// ⚠ Mirrors `spawn`'s parameter list, which carries the same allow. ⚠⚠ Ten arguments is past the
// point where a `RunConfig { render, width, batch, upload }` would read better — noted rather than
// done here, because changing this signature churns the concurrency carrier tests and belongs in its
// own change.
#[allow(clippy::too_many_arguments)]
fn run(
    jobs: Vec<ExportJob>,
    opts: ExportOptions,
    decode: impl Fn(&Path) -> Result<ExportSource, String> + Sync,
    events: Sender<ExportEvent>,
    decisions: Receiver<Decision>,
    cancel: Arc<AtomicBool>,
    render: ExportRender,
    width: usize,
    batch: &Batch,
    upload: Option<crate::immich_upload::UploadConfig>,
) {
    let mut report = ExportReport::default();
    // ⚠ Said once for the run, before anything is written: every photo goes out without the mark.
    if let Err(note) = &batch.logo {
        report.notes.push(note.clone());
    }
    // ⚠ Parsed **once** per run, not per photo. ⚠ A template that will not parse falls back to
    // the default rather than failing the run: the dialog refuses to start an export with an
    // invalid one, so the only way to get here is a hand-edited preset file — and a folder named
    // the old way beats no folder at all.
    let template = Template::parse(&opts.filename_template).unwrap_or_default();

    // ⚠ Built here rather than in `spawn`, the same rule `resolve_looks` follows: this is the
    // worker thread, and the UI thread that starts an export should not do the work.
    let uploader = upload.and_then(Uploader::new);

    // ⚠⚠ Immich-only stages here and keeps nothing; `Files`/`Both` write straight to `dest_dir`.
    // ⚠ Falls back to `dest_dir` when a temp directory cannot be made — the export is worth more
    // than the tidiness.
    let staging = (!opts.destination.keeps_files())
        .then(staging_dir)
        .flatten();
    let out_dir = staging.clone().unwrap_or_else(|| opts.dest_dir.clone());
    // ⚠⚠ **`swap(true)`, not a load-then-store.** The pool is `width`-wide, so several photos can
    // fail in the same instant; `swap` makes exactly one of them win and open exactly one pop-up.
    let told = AtomicBool::new(false);

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

    // ⚠ The **planned** count, after skips — see `plan`'s note on why a skipped photo is not in it.
    let total = planned.len();
    if events.send(ExportEvent::Started { total }).is_err() {
        return;
    }

    // ⚠⚠ **A shared cursor rather than static chunks**, `ThumbLoader`'s rule and for its reason: a
    // selection mixes RAW and JPEG whose decode costs differ by an order of magnitude, so chunking
    // would leave workers idle on the tail.
    let next = std::sync::atomic::AtomicUsize::new(0);
    let finished = std::sync::atomic::AtomicUsize::new(0);
    let shared = std::sync::Mutex::new(report);

    // ⚠⚠ **Each worker does a WHOLE photo — decode, render, write — rather than a pool of decoders
    // feeding one renderer.** The render is ~8% of a photo (0.6 s against a 7.7 s decode), so
    // handing it over would buy that 8% and cost a channel, a buffer and a second place for a
    // 298 MiB image to be alive. The honest consequence is that **peak memory is `width` whole
    // photos**, which is exactly what the setting exists to bound.
    //
    // ⚠ `thread::scope` rather than detached threads: the workers borrow `planned`, `opts` and
    // `decode`, and the scope is what makes that sound without an `Arc` around each.
    std::thread::scope(|s| {
        for _ in 0..width.max(1) {
            // ⚠ A clone per worker rather than sharing one `Sender`: `mpsc::Sender` is `Clone` by
            // design for exactly this, and it needs no assumption about its `Sync`ness.
            let events = events.clone();
            let next = &next;
            let finished = &finished;
            let shared = &shared;
            let decode = &decode;
            let planned = &planned;
            let opts = &opts;
            let cancel = &cancel;
            let uploader = uploader.as_ref();
            let staging = &staging;
            let told = &told;
            s.spawn(move || loop {
                if cancel.load(Ordering::Relaxed) {
                    shared.lock().expect("export report").cancelled = true;
                    return;
                }
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(p) = planned.get(i) else { return };

                // ⚠ M9.13: the photo's cached AI maps ride the source, looked up exactly as the
                // preview looks them up (`ai_coverage::maps_for`), so both render the same mask.
                let decoded = decode(&p.source).map(|mut src| {
                    if let Some(dir) = crate::ai_cache::dir() {
                        src.ai = crate::ai_coverage::maps_for(
                            &p.source,
                            &p.edits,
                            render.ai_subject,
                            &dir,
                        );
                    }
                    src
                });

                // ⚠ Checked *again* after the decode, which is the slow part. Cancelling during a
                // ~11 s decode must not still write that photo out: libraw cannot be interrupted,
                // so the decode finishes, but its result is discarded here.
                //
                // ⚠⚠ **With several in flight this now abandons up to `width` decodes rather than
                // one** — up to ~30 s of background CPU on a four-wide run, spent and discarded.
                // Inherent, and the same "abandon, don't abort" trade the sequential version
                // already made for one.
                if cancel.load(Ordering::Relaxed) {
                    shared.lock().expect("export report").cancelled = true;
                    return;
                }

                // ⚠⚠ **Per JOB, not per batch, and that distinction is the general case.** Every
                // job carries its own `Edits`, so a selection can name several different `.cube`s
                // — a batch-wide look would silently render some photographs with another's. The
                // *loading* is still per batch: `resolve_looks` read each distinct name once.
                //
                // ⚠ `is_active()` gates it for `build_pipeline_split_in_region_with_brush`'s own
                // reason: a look at zero strength must not suppress the camera rendering.
                let look = p
                    .edits
                    .look
                    .file
                    .as_deref()
                    .filter(|_| p.edits.look.is_active())
                    .and_then(|f| batch.looks.get(f));
                let outcome = decoded.and_then(|src| {
                    export_one(
                        src,
                        &p.edits,
                        &p.dest,
                        opts,
                        render,
                        &p.meta,
                        look,
                        batch.logo(),
                    )
                });
                // ⚠⚠ Only after a successful write, and it can never turn a success into a
                // failure. The exported file is the deliverable; the upload is delivery.
                let upload = match (&outcome, uploader) {
                    (Ok(_), Some(u)) => Some(u.send(
                        &p.dest,
                        &p.capture,
                        &p.source,
                        p.copy,
                        opts.upload_date,
                        p.edited_at,
                    )),
                    _ => None,
                };
                {
                    // ⚠ `failed` is now in **completion** order, which is nondeterministic. Any
                    // test comparing it must sort or compare as a set.
                    let mut r = shared.lock().expect("export report");
                    match outcome {
                        Ok(w) => {
                            r.exported += 1;
                            r.below_ppi += usize::from(w.below_ppi);
                            r.shrunk += usize::from(w.shrunk);
                        }
                        Err(e) => r.failed.push((p.source.clone(), e)),
                    }
                    match upload {
                        // ⚠⚠ Deleted **only** on a confirmed upload. An SSL/EOF error means the
                        // *response* was lost, not that the server rejected the file — 4 of 2,373
                        // writes failed that way on this network and **none had actually failed to
                        // apply**. Deleting on anything less would destroy work on a flaky link.
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
                            // ⚠ The first one only, and the send is outside nothing — it is a
                            // channel push, so it cannot block the worker holding this lock.
                            if !told.swap(true, Ordering::Relaxed) {
                                let _ = events.send(ExportEvent::UploadUnconfirmed {
                                    at: at.clone(),
                                    why: e.clone(),
                                });
                            }
                            r.unconfirmed.push((at, e));
                        }
                        None => {}
                    }
                }

                // ⚠⚠ Sent on **completion**, and `done` counts completions rather than positions.
                // With several in flight the fourth photo can land before the second, so a
                // position would jump about where a count only ever rises. It is out of order by
                // **name**, and honestly so.
                let done = finished.fetch_add(1, Ordering::Relaxed) + 1;
                let _ = events.send(ExportEvent::Item {
                    done,
                    total,
                    name: file_label(&p.source),
                });
            });
        }
    });
    let mut report = shared.into_inner().expect("export report");
    // ⚠ Said once for the run, after the counts, with the numbers the photos were held to (M10.10b).
    let photos = |n: usize| format!("{n} photo{}", if n == 1 { "" } else { "s" });
    if report.below_ppi > 0 {
        let note = format!("{} below {} PPI", photos(report.below_ppi), opts.size.ppi);
        report.notes.push(note);
    }
    if report.shrunk > 0 {
        let note = format!(
            "{} made smaller to fit {} KB",
            photos(report.shrunk),
            opts.limit.kb
        );
        report.notes.push(note);
    }

    // ⚠ Only when empty. A staging directory still holding files is **not** an error — it means
    // something did not confirm and could not be rescued, and `unconfirmed` already names each one.
    if let Some(dir) = &staging {
        let _ = std::fs::remove_dir(dir);
    }

    let _ = events.send(ExportEvent::Finished(report));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::batch_export::fixtures::*;
    use crate::photo_id::PhotoId;
    use export::ExportFormat;

    /// Jobs carrying capture metadata, for the tokens that come from the catalog.
    fn jobs_with(dir: &TempDir, files: &[(&str, core_image::Capture)]) -> Vec<ExportJob> {
        files
            .iter()
            .map(|(n, c)| ExportJob {
                id: PhotoId::original(dir.0.join(n)),
                name: None,
                edits: Edits::default(),
                capture: c.clone(),
                description: Default::default(),
                marks: Default::default(),
                keywords: Vec::new(),
            })
            .collect()
    }

    /// 2026-08-27 10:00:00 as naive wall-clock seconds, on an X-T4.
    fn shot() -> core_image::Capture {
        core_image::Capture {
            taken: Some(1_787_824_800),
            camera: Some("FUJIFILM X-T4".to_string()),
            ..Default::default()
        }
    }

    fn templated(dir: &TempDir, template: &str) -> ExportOptions {
        ExportOptions {
            filename_template: template.to_string(),
            ..opts(dir, None)
        }
    }

    #[test]
    fn the_default_base_is_what_dest_for_has_always_produced() {
        // ⚠⚠ The regression guard for the whole feature: `dest_for` is now defined *through*
        // `dest_named`, so the two cannot drift — and the default template renders to the same
        // base, which is what makes an existing user's next export byte-identical.
        let dir = PathBuf::from("/out");
        for (copy, name) in [(0u32, None), (2, None), (3, Some("Warm"))] {
            let base = crate::filename_template::Template::default().render(&NameFacts {
                file_stem: "a",
                copy,
                copy_label: name,
                seq: 1,
                taken: None,
                camera: None,
            });
            assert_eq!(
                dest_named(&dir, &base, 0, ExportFormat::Jpeg),
                dest_for(
                    &dir,
                    Path::new("/in/a.RAF"),
                    copy,
                    name,
                    0,
                    ExportFormat::Jpeg
                ),
                "copy {copy}"
            );
        }
    }

    #[test]
    fn a_run_needs_both_a_destination_and_a_template_that_parses() {
        // ⚠⚠ Each half asserted with the *other* half valid, which is what makes this two
        // properties rather than one. A version that only looked at the destination would pass
        // any test that varied the destination alone — and would ship a dialog whose Export
        // button stays lit over a typo.
        let d = TempDir::new("can_export");
        let good = templated(&d, "{name}-{seq:3}");
        assert!(can_export(&good, true));
        assert!(
            !can_export(
                &ExportOptions {
                    filename_template: "{camrea}".into(),
                    ..good.clone()
                },
                true
            ),
            "an unknown token must stop the run"
        );
        assert!(
            !can_export(
                &ExportOptions {
                    dest_dir: PathBuf::new(),
                    ..good.clone()
                },
                true
            ),
            "an unset destination must stop the run"
        );
        assert!(!can_export(
            &ExportOptions {
                dest_dir: d.0.join("gone"),
                ..good
            },
            true
        ));
    }

    #[test]
    fn a_template_names_the_files_it_writes() {
        // End to end through the worker, because the tokens being right says nothing about the
        // worker reaching for them — the sabotage that skipped rendering entirely left every
        // `filename_template` unit test green.
        let d = TempDir::new("template_names");
        let (report, _) = run_export(
            jobs_with(&d, &[("a.RAF", shot()), ("b.RAF", shot())]),
            templated(&d, "{date}_{seq:3}"),
            vec![],
            ok_decode(),
        );
        assert_eq!(report.exported, 2);
        assert!(d.0.join("2026-08-27_001.jpg").exists());
        assert!(d.0.join("2026-08-27_002.jpg").exists());
        assert!(!d.0.join("a.jpg").exists(), "the old naming must be gone");
    }

    #[test]
    fn seq_starts_at_one_and_follows_the_job_order() {
        // ⚠ 1-based and positional in the list the caller handed over, which is the order the
        // Library is showing. A 0-based `{seq}` would put `000` on the first photo of every run.
        let d = TempDir::new("template_seq");
        let (report, _) = run_export(
            jobs(&d, &["z.RAF", "a.RAF", "m.RAF"]),
            templated(&d, "{seq:2}"),
            vec![],
            ok_decode(),
        );
        assert_eq!(report.exported, 3);
        for n in ["01.jpg", "02.jpg", "03.jpg"] {
            assert!(d.0.join(n).exists(), "{n} missing");
        }
        assert!(!d.0.join("00.jpg").exists(), "the run starts at 1");
    }

    #[test]
    fn a_photo_with_no_camera_leaves_no_dangling_separator() {
        // ⚠⚠ Ordinary rather than exotic: two of the author's own exported JPEGs carry no EXIF at
        // all, so a mixed selection hits both branches of the collapsing rule in one run.
        let d = TempDir::new("template_absent");
        let (report, _) = run_export(
            jobs_with(
                &d,
                &[("a.RAF", shot()), ("b.jpg", core_image::Capture::default())],
            ),
            templated(&d, "{name}-{camera}-{seq:2}"),
            vec![],
            ok_decode(),
        );
        assert_eq!(report.exported, 2);
        assert!(d.0.join("a-FUJIFILM X-T4-01.jpg").exists());
        assert!(
            d.0.join("b-02.jpg").exists(),
            "the file with no camera keeps one separator, not two"
        );
        assert!(!d.0.join("b--02.jpg").exists());
    }

    #[test]
    fn a_template_that_will_not_parse_falls_back_to_the_old_naming() {
        // ⚠ The dialog refuses to start an export with an invalid template, so the only way here
        // is a hand-edited preset file. A folder named the old way beats no folder at all — and
        // beats a folder of files called `{camrea}`.
        let d = TempDir::new("template_bad");
        let (report, _) = run_export(
            jobs(&d, &["a.RAF"]),
            templated(&d, "{camrea}"),
            vec![],
            ok_decode(),
        );
        assert_eq!(report.exported, 1);
        assert!(d.0.join("a.jpg").exists());
    }

    #[test]
    fn keep_both_appends_its_suffix_to_the_templated_name() {
        // ⚠⚠ The template is a **third** number and must not merge with `-N` (de-duplication) or
        // `_N` (copy identity). A template that drops `{name}` makes two photos want one name,
        // which is exactly when `-N` has to still work.
        let d = TempDir::new("template_dedup");
        let (report, _) = run_export(
            jobs(&d, &["a.RAF", "b.RAF"]),
            templated(&d, "shoot"),
            vec![Decision::Resolve {
                collision: Collision::KeepBoth,
                apply_to_all: true,
            }],
            ok_decode(),
        );
        assert_eq!(report.exported, 2);
        assert!(d.0.join("shoot.jpg").exists() && d.0.join("shoot-1.jpg").exists());
    }

    #[test]
    fn every_photo_produces_a_jpeg() {
        let d = TempDir::new("all");
        let (report, events) = run_export(
            jobs(&d, &["a.RAF", "b.RAF"]),
            opts(&d, None),
            vec![],
            ok_decode(),
        );

        assert_eq!(report.exported, 2);
        assert_eq!(report.skipped, 0);
        assert!(report.failed.is_empty() && !report.cancelled);
        assert!(d.0.join("a.jpg").exists() && d.0.join("b.jpg").exists());
        assert_eq!(events.first(), Some(&ExportEvent::Started { total: 2 }));
        let items = events
            .iter()
            .filter(|e| matches!(e, ExportEvent::Item { .. }))
            .count();
        assert_eq!(items, 2, "each photo reports progress exactly once");
    }

    /// ⚠⚠ **THE PROPERTY THE WHOLE PARALLEL PHASE RESTS ON.** A decision that leaked into the
    /// export phase would mean a worker blocking on a dialog while its neighbours ran — the stall
    /// the up-front resolution exists to remove, and a state nothing else here could see.
    ///
    /// ⚠ **Both collisions, because one alone passes against "resolved one photo ahead".** A loop
    /// that merely moved the prompt before *its own* decode would raise the first collision with
    /// nothing decoded and then decode photo 1 before asking about photo 2.
    ///
    /// ⚠⚠ **Recorded honestly: today this property is STRUCTURAL rather than guarded** — `plan`
    /// returns before `run` decodes anything, so there is no cheap sabotage that can break the
    /// ordering without rebuilding the old interleaved loop wholesale. What this test is for is
    /// the *future* regression: someone folding the decode back into the planning loop to "save a
    /// pass". It would fail immediately, and nothing else would.
    /// ⚠⚠ **THE CARRIER, and it needs BOTH halves.** A wide run and a sequential one produce the
    /// same files **by design**, so "the output is right" passes with the concurrency deleted; and
    /// "it ran concurrently" passes against a run that wrote the wrong files. Only the pair says
    /// the feature is there *and* correct — the W5 failure mode, where creative LUTs shipped fully
    /// tested and 100% non-functional.
    ///
    /// ⚠⚠ **Concurrency is asserted as PEAK SIMULTANEOUS DECODES, never by wall clock** —
    /// `ThumbLoader`'s rule and for its reason: a timing assertion is a CI flake, and this project
    /// has three on record. The count lives in the **injected decode fn**, so no production
    /// observable is needed at all.
    #[test]
    fn a_wide_export_decodes_several_at_once_and_writes_the_same_files() {
        let names = |dir: &TempDir| {
            let mut v: Vec<String> = std::fs::read_dir(&dir.0)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            v.sort();
            v
        };
        let run_at = |width: usize, dir: &TempDir| -> usize {
            let live = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let peak = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let (l, p) = (live.clone(), peak.clone());
            let worker = ExportWorker::spawn(
                jobs(
                    dir,
                    &[
                        "p0.RAF", "p1.RAF", "p2.RAF", "p3.RAF", "p4.RAF", "p5.RAF", "p6.RAF",
                        "p7.RAF",
                    ],
                ),
                opts(dir, None),
                move |_: &Path| {
                    let n = l.fetch_add(1, Ordering::Relaxed) + 1;
                    p.fetch_max(n, Ordering::Relaxed);
                    // ⚠ Long enough that the workers genuinely overlap, short enough that the test
                    // is not a sleep. **The assertion is on the peak, never on elapsed time.**
                    std::thread::sleep(std::time::Duration::from_millis(30));
                    l.fetch_sub(1, Ordering::Relaxed);
                    Ok(source(flat(8, 4)))
                },
                Arc::new(AtomicBool::new(false)),
                CPU,
                width,
                ExportDirs::default(),
                None,
            );
            let report = finished(&worker);
            assert_eq!(
                report.exported, 8,
                "width {width} exported {}",
                report.exported
            );
            peak.load(Ordering::Relaxed)
        };
        let one = TempDir::new("wide_one");
        let four = TempDir::new("wide_four");
        assert_eq!(run_at(1, &one), 1, "width 1 must never overlap");
        assert!(
            run_at(4, &four) > 1,
            "width 4 must decode more than one photo at a time"
        );
        assert_eq!(
            names(&one),
            names(&four),
            "the two widths must write the same files"
        );
    }

    #[test]
    fn no_photo_is_decoded_before_every_collision_is_answered() {
        let d = TempDir::new("plan_first");
        // Both destinations already exist, so both collide.
        std::fs::write(d.0.join("a.jpg"), b"x").unwrap();
        std::fs::write(d.0.join("b.jpg"), b"x").unwrap();
        let decoded = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = decoded.clone();
        let worker = ExportWorker::spawn(
            jobs(&d, &["a.RAF", "b.RAF"]),
            opts(&d, None),
            move |_: &Path| {
                seen.fetch_add(1, Ordering::Relaxed);
                Ok(source(flat(8, 4)))
            },
            Arc::new(AtomicBool::new(false)),
            CPU,
            1,
            ExportDirs::default(),
            None,
        );
        let mut collisions = 0;
        let mut report = ExportReport::default();
        while let Ok(ev) = worker.events().recv() {
            match ev {
                ExportEvent::Collision { .. } => {
                    collisions += 1;
                    assert_eq!(
                        decoded.load(Ordering::Relaxed),
                        0,
                        "collision {collisions} was raised after {} decodes — a decision reached \
                         the export phase",
                        decoded.load(Ordering::Relaxed)
                    );
                    worker.decide(Decision::Resolve {
                        collision: Collision::Overwrite,
                        apply_to_all: false,
                    });
                }
                ExportEvent::Finished(r) => {
                    report = r;
                    break;
                }
                _ => {}
            }
        }
        assert_eq!(collisions, 2, "fixture check: both photos must collide");
        assert_eq!(report.exported, 2);
    }

    #[test]
    fn two_sources_with_the_same_stem_collide_in_an_empty_folder() {
        // ⚠ `a.RAF` and `a.jpg` both map to `a.jpg`, so the check has to consider names
        // claimed *in this run*, not only files already on disk.
        //
        // Parameterised by format because the extension decides **which** pair collides:
        // exporting PNG, `a.RAF` and `a.jpg` no longer do — but `a.RAF` and `a.png` now do.
        // The property is format-independent; only the triggering pair moves.
        for (format, other) in [
            (ExportFormat::Jpeg, "a.jpg"),
            (ExportFormat::Png8, "a.png"),
            (ExportFormat::Tiff16, "a.tif"),
        ] {
            let d = TempDir::new(&format!("same_stem_{}", format.extension()));
            let (report, _) = run_export(
                jobs(&d, &["a.RAF", other]),
                ExportOptions {
                    format,
                    ..opts(&d, None)
                },
                vec![Decision::Resolve {
                    collision: Collision::KeepBoth,
                    apply_to_all: false,
                }],
                ok_decode(),
            );
            let ext = format.extension();
            assert_eq!(report.exported, 2, "{format:?}");
            assert!(
                d.0.join(format!("a.{ext}")).exists() && d.0.join(format!("a-1.{ext}")).exists(),
                "{format:?}"
            );
        }
    }

    #[test]
    fn the_destination_extension_follows_the_format() {
        let dir = PathBuf::from("/out");
        for f in ExportFormat::ALL {
            assert_eq!(
                dest_for(&dir, Path::new("/in/DSCF6509.RAF"), 0, None, 0, f),
                dir.join(format!("DSCF6509.{}", f.extension())),
                "{f:?}"
            );
            assert_eq!(
                dest_for(&dir, Path::new("/in/a.RAF"), 0, None, 3, f),
                dir.join(format!("a-3.{}", f.extension())),
                "{f:?}"
            );
        }
    }

    #[test]
    fn skip_leaves_the_existing_file_untouched() {
        let d = TempDir::new("skip");
        std::fs::write(d.0.join("a.jpg"), b"original").unwrap();
        let (report, _) = run_export(
            jobs(&d, &["a.RAF"]),
            opts(&d, None),
            vec![Decision::Resolve {
                collision: Collision::Skip,
                apply_to_all: false,
            }],
            ok_decode(),
        );
        assert_eq!((report.exported, report.skipped), (0, 1));
        assert_eq!(std::fs::read(d.0.join("a.jpg")).unwrap(), b"original");
    }

    #[test]
    fn overwrite_replaces_it() {
        let d = TempDir::new("overwrite");
        std::fs::write(d.0.join("a.jpg"), b"original").unwrap();
        let (report, _) = run_export(
            jobs(&d, &["a.RAF"]),
            opts(&d, None),
            vec![Decision::Resolve {
                collision: Collision::Overwrite,
                apply_to_all: false,
            }],
            ok_decode(),
        );
        assert_eq!(report.exported, 1);
        assert_ne!(std::fs::read(d.0.join("a.jpg")).unwrap(), b"original");
    }

    #[test]
    fn keep_both_finds_the_first_free_suffix() {
        let d = TempDir::new("keep_both");
        std::fs::write(d.0.join("a.jpg"), b"x").unwrap();
        std::fs::write(d.0.join("a-1.jpg"), b"x").unwrap();
        let (report, _) = run_export(
            jobs(&d, &["a.RAF"]),
            opts(&d, None),
            vec![Decision::Resolve {
                collision: Collision::KeepBoth,
                apply_to_all: false,
            }],
            ok_decode(),
        );
        assert_eq!(report.exported, 1);
        assert!(d.0.join("a-2.jpg").exists());
    }

    #[test]
    fn apply_to_all_stops_further_prompts_and_reuses_the_choice() {
        let d = TempDir::new("apply_all");
        for n in ["a.jpg", "b.jpg"] {
            std::fs::write(d.0.join(n), b"original").unwrap();
        }
        // Exactly one answer supplied: a second prompt would make `run_export` cancel, and
        // the assertions below would fail.
        let (report, events) = run_export(
            jobs(&d, &["a.RAF", "b.RAF"]),
            opts(&d, None),
            vec![Decision::Resolve {
                collision: Collision::Skip,
                apply_to_all: true,
            }],
            ok_decode(),
        );
        assert_eq!((report.exported, report.skipped), (0, 2));
        let prompts = events
            .iter()
            .filter(|e| matches!(e, ExportEvent::Collision { .. }))
            .count();
        assert_eq!(prompts, 1, "the second collision must not prompt again");
        assert_eq!(std::fs::read(d.0.join("b.jpg")).unwrap(), b"original");
    }

    #[test]
    fn cancelling_at_a_prompt_ends_the_run() {
        let d = TempDir::new("cancel_prompt");
        std::fs::write(d.0.join("a.jpg"), b"original").unwrap();
        let (report, _) = run_export(
            jobs(&d, &["a.RAF", "b.RAF"]),
            opts(&d, None),
            vec![Decision::Cancel],
            ok_decode(),
        );
        assert!(report.cancelled);
        assert_eq!(report.exported, 0);
        assert!(!d.0.join("b.jpg").exists(), "the rest must be untouched");
    }

    #[test]
    fn a_decode_failure_is_collected_and_the_batch_continues() {
        // One unreadable file must not cost the other photos their export.
        let d = TempDir::new("fail_one");
        let (report, _) = run_export(
            jobs(&d, &["bad.RAF", "good.RAF"]),
            opts(&d, None),
            vec![],
            |p: &Path| {
                if p.to_string_lossy().contains("bad") {
                    Err("no".to_string())
                } else {
                    Ok(source(flat(8, 4)))
                }
            },
        );
        assert_eq!(report.exported, 1);
        assert_eq!(report.failed.len(), 1);
        assert!(report.failed[0].0.ends_with("bad.RAF"));
        assert!(d.0.join("good.jpg").exists());
    }

    /// Drain events until the run reports itself finished.
    fn finished(worker: &ExportWorker) -> ExportReport {
        while let Ok(ev) = worker.events().recv() {
            if let ExportEvent::Finished(r) = ev {
                return r;
            }
        }
        panic!("the worker ended without a Finished event");
    }

    #[test]
    fn a_flag_already_set_stops_the_run_before_the_first_photo() {
        // libraw's `unpack` is not interruptible, so cancellation is checked between items.
        //
        // ⚠ The flag is passed in **already set**, not stored after spawning. An earlier
        // version called `worker.cancel()` after `spawn`, which races: `spawn` starts the
        // thread immediately, so whether the first photo was exported depended on who won.
        // It won on macOS every time and lost on Linux CI — `left: 1, right: 0`. If a test
        // needs the worker cancelled before it runs, the only way to say so without a race
        // is to hand it a flag that is already true.
        let d = TempDir::new("cancel_flag");
        let worker = ExportWorker::spawn(
            jobs(&d, &["a.RAF", "b.RAF"]),
            opts(&d, None),
            ok_decode(),
            Arc::new(AtomicBool::new(true)),
            CPU,
            1,
            ExportDirs::default(),
            None,
        );
        let report = finished(&worker);
        assert!(report.cancelled);
        assert_eq!(report.exported, 0);
        assert!(!d.0.join("a.jpg").exists(), "nothing should be written");
    }

    #[test]
    fn cancel_reaches_the_worker_and_stops_the_next_photo() {
        // Covers the public `cancel()` — and with it that `spawn` hands the worker the *same*
        // flag the handle keeps, which a one-line store can still get wrong.
        //
        // A handshake, not a sleep: the decode announces that it has entered the first photo
        // and blocks until the test has cancelled, so the worker cannot race ahead. Two
        // resume permits are sent so that a *regression* — the worker wrongly starting the
        // second photo — fails an assertion instead of deadlocking the suite.
        let d = TempDir::new("cancel_api");
        let (started_tx, started_rx) = std::sync::mpsc::channel::<()>();
        let (resume_tx, resume_rx) = std::sync::mpsc::channel::<()>();
        // ⚠ Behind a `Mutex` because the decode fn is now **shared across workers** and so must be
        // `Sync`, which `mpsc::Receiver` is not. At width 1 nothing contends; the lock is what
        // makes the handshake expressible under the wider bound rather than a synchronisation the
        // test needs.
        let resume_rx = std::sync::Mutex::new(resume_rx);
        let worker = ExportWorker::spawn(
            jobs(&d, &["a.RAF", "b.RAF"]),
            opts(&d, None),
            move |_: &Path| {
                let _ = started_tx.send(());
                let _ = resume_rx.lock().expect("resume").recv();
                Ok(source(flat(8, 4)))
            },
            Arc::new(AtomicBool::new(false)),
            CPU,
            1,
            ExportDirs::default(),
            None,
        );

        started_rx.recv().expect("worker never reached a decode");
        worker.cancel();
        let _ = resume_tx.send(());
        let _ = resume_tx.send(());

        let report = finished(&worker);
        assert!(report.cancelled);
        // "Abandon, don't abort": the photo in flight finishes decoding but is discarded.
        assert_eq!(report.exported, 0);
        assert!(
            !d.0.join("a.jpg").exists(),
            "the cancelled photo was written"
        );
        assert!(
            !d.0.join("b.jpg").exists(),
            "the second photo must never start"
        );
    }

    #[test]
    fn cancelling_during_a_decode_does_not_write_that_photo() {
        // ⚠ The whole point of the post-decode check. libraw cannot be interrupted, so the
        // decode of the photo in flight completes — but the file the user cancelled must not
        // appear anyway. The decode fn flips the flag mid-photo, which is exactly what a
        // click on Cancel does while a decode is running.
        let d = TempDir::new("cancel_mid");
        let flag = Arc::new(AtomicBool::new(false));
        let in_decode = flag.clone();
        let worker = ExportWorker::spawn(
            jobs(&d, &["a.RAF", "b.RAF"]),
            opts(&d, None),
            move |_: &Path| {
                in_decode.store(true, Ordering::Relaxed);
                Ok(source(flat(8, 4)))
            },
            flag,
            CPU,
            1,
            ExportDirs::default(),
            None,
        );

        let mut report = ExportReport::default();
        while let Ok(ev) = worker.events().recv() {
            if let ExportEvent::Finished(r) = ev {
                report = r;
                break;
            }
        }
        assert!(report.cancelled);
        assert_eq!(
            report.exported, 0,
            "the cancelled photo must not be written"
        );
        assert!(!d.0.join("a.jpg").exists());
        assert!(!d.0.join("b.jpg").exists());
    }

    #[test]
    fn a_copy_exports_with_an_underscore_suffix() {
        let dir = std::path::PathBuf::from("/out");
        assert_eq!(
            dest_for(
                &dir,
                Path::new("/in/DSCF6223.RAF"),
                0,
                None,
                0,
                ExportFormat::Jpeg
            ),
            dir.join("DSCF6223.jpg"),
            "the original keeps the plain stem"
        );
        assert_eq!(
            dest_for(
                &dir,
                Path::new("/in/DSCF6223.RAF"),
                2,
                None,
                0,
                ExportFormat::Jpeg
            ),
            dir.join("DSCF6223_2.jpg")
        );
    }

    #[test]
    fn a_labelled_copy_exports_with_its_label_beside_the_file_name() {
        // ⚠⚠ M6.0 reverses what this test used to assert. The label used to *replace* the stem,
        // giving `Warm.jpg`, because the name **was** the export filename. Now the default
        // template is `{name}_{copy}`, so the export keeps the photograph it came from.
        //
        // ⚠ The old behaviour is not lost: it is the template `{copy}`, which
        // `the_copy_token_alone_reproduces_the_old_named_copy_export` pins.
        let dir = std::path::PathBuf::from("/out");
        assert_eq!(
            dest_for(
                &dir,
                Path::new("/in/DSCF6223.RAF"),
                2,
                Some("Warm"),
                0,
                ExportFormat::Jpeg
            ),
            dir.join("DSCF6223_Warm.jpg")
        );
    }

    #[test]
    fn a_label_on_the_original_reaches_the_export_too() {
        // ⚠⚠ Also reversed by M6.0, and deliberately. Copy 0 could not be labelled at all before,
        // because a "name" on it would have been a second filename. A label is not a filename, so
        // the original can be "Colour" beside its "B&W" copy — and that must reach the export.
        let dir = std::path::PathBuf::from("/out");
        assert_eq!(
            dest_for(
                &dir,
                Path::new("/in/a.RAF"),
                0,
                Some("Colour"),
                0,
                ExportFormat::Jpeg
            ),
            dir.join("a_Colour.jpg")
        );
    }

    #[test]
    fn two_copies_sharing_a_name_still_get_distinct_files() {
        // ⚠ Nothing stops two copies being called the same thing, so the name alone cannot be
        // relied on to be unique. The pre-existing `-N` de-duplication is what catches it, which
        // is the reason the two schemes were kept separate rather than merged.
        let dir = std::path::PathBuf::from("/out");
        let first = dest_for(
            &dir,
            Path::new("/in/a.RAF"),
            1,
            Some("Warm"),
            0,
            ExportFormat::Jpeg,
        );
        let second = dest_for(
            &dir,
            Path::new("/in/a.RAF"),
            2,
            Some("Warm"),
            1,
            ExportFormat::Jpeg,
        );
        assert_eq!(first, dir.join("a_Warm.jpg"));
        assert_eq!(second, dir.join("a_Warm-1.jpg"));
        assert_ne!(first, second);
    }

    #[test]
    fn the_two_numbering_schemes_compose_rather_than_collide() {
        // ⚠ `_N` is **identity** — which virtual copy, stable forever. `-N` is
        // **de-duplication** — the Nth file wanting this name in this folder, assigned at export
        // time. Different meanings, so different separators, and both can appear at once.
        let dir = std::path::PathBuf::from("/out");
        assert_eq!(
            dest_for(&dir, Path::new("/in/a.RAF"), 2, None, 1, ExportFormat::Jpeg),
            dir.join("a_2-1.jpg")
        );
        assert_eq!(
            dest_for(&dir, Path::new("/in/a.RAF"), 0, None, 3, ExportFormat::Jpeg),
            dir.join("a-3.jpg"),
            "an original that collides is unchanged from before this feature"
        );
    }

    /// ⚠⚠ **A FAILED UPLOAD MUST NOT FAIL THE EXPORT.** The file is on disk; a flaky network
    /// destroying a batch export would be far worse than an unuploaded file. The connection points
    /// at a closed port on loopback, which is the failure shape and refuses immediately rather than
    /// waiting on a timeout — so this is deterministic and fast, with no server involved.
    #[test]
    fn a_failed_upload_still_exports_the_file_and_is_reported_unconfirmed() {
        let d = TempDir::new("upload_fail");
        let worker = ExportWorker::spawn(
            jobs(&d, &["a.RAF"]),
            opts(&d, None),
            ok_decode(),
            Arc::new(AtomicBool::new(false)),
            CPU,
            1,
            ExportDirs::default(),
            // ⚠ Port 1 on loopback: nothing listens, so the connection is refused at once.
            Some(crate::immich_upload::UploadConfig {
                base_url: "http://127.0.0.1:1".to_string(),
                key: "irrelevant".to_string(),
            }),
        );
        let report = finished(&worker);
        assert_eq!(report.exported, 1, "the export itself must succeed");
        assert!(
            report.failed.is_empty(),
            "an upload failure is not an export failure: {:?}",
            report.failed
        );
        assert!(d.0.join("a.jpg").exists(), "the file must still be written");
        assert_eq!(report.uploaded, 0, "nothing was uploaded");
        assert_eq!(
            report.unconfirmed.len(),
            1,
            "the failure must be reported, not swallowed"
        );
    }

    /// ⚠ The control: with no connection nothing is attempted and nothing is reported, so the
    /// counters above cannot be passing for an unrelated reason.
    #[test]
    fn no_upload_connection_means_no_upload_reporting() {
        let d = TempDir::new("upload_off");
        let worker = ExportWorker::spawn(
            jobs(&d, &["a.RAF"]),
            opts(&d, None),
            ok_decode(),
            Arc::new(AtomicBool::new(false)),
            CPU,
            1,
            ExportDirs::default(),
            None,
        );
        let report = finished(&worker);
        assert_eq!(report.exported, 1);
        assert_eq!(report.uploaded, 0);
        assert!(
            report.unconfirmed.is_empty(),
            "nothing should be reported when nothing was attempted"
        );
    }

    /// ⚠⚠ **THE LEAST-EXERCISED PATH, AND THE REASON THIS SHAPE WAS CHOSEN OVER TWO SIMPLER ONES.**
    /// Immich-only stages in a temp directory; when the upload does not confirm the file must end up
    /// in the export folder, findable, rather than swept with the temp directory. Forced with a
    /// closed port on loopback, which refuses at once rather than waiting on a timeout.
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
            ExportDirs::default(),
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
            "the staged file must be moved where the user can find it; the folder held {:?}",
            std::fs::read_dir(&d.0)
                .unwrap()
                .flatten()
                .map(|e| e.path())
                .collect::<Vec<_>>()
        );
        // ⚠ And the report must name where it actually is, not the temp path it was written to.
        assert_eq!(report.unconfirmed[0].0, landed);
    }

    /// ⚠ The control for the staging half: with no target at all, `Files` keeps its file exactly
    /// where it always did. Without this the move above could pass against a mode that never staged.
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
            ExportDirs::default(),
            None,
        );
        let report = finished(&worker);
        assert_eq!(report.exported, 1);
        assert!(d.0.join("a.jpg").exists());
        assert!(report.unconfirmed.is_empty());
    }

    /// ⚠⚠ **A staging run must never leave a file in the temp directory.** The confirmed-delete half
    /// needs a server, so this pins the reachable half: whatever happens, the file the report names
    /// is inside the export folder and exists. Without it, "Immich only" could quietly strand files
    /// somewhere the user never looks.
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
            ExportDirs::default(),
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

    /// ⚠⚠ **"Just once for the first one" is the requirement, so the pair is what makes this
    /// discriminating:** two photos fail, and there must be **one** event and **two** report entries.
    /// Asserting only the event count would pass against a run that lost the second failure entirely.
    #[test]
    fn only_the_first_unconfirmed_upload_raises_the_notice() {
        let d = TempDir::new("told_once");
        let mut o = opts(&d, None);
        o.destination = Destination::Both(UploadTarget::Immich);
        let worker = ExportWorker::spawn(
            jobs(&d, &["a.RAF", "b.RAF"]),
            o,
            ok_decode(),
            Arc::new(AtomicBool::new(false)),
            CPU,
            // ⚠ Width 1 so the two failures are ordered; the `swap` guard is what makes the wide
            // case safe, and a width of 1 is the case whose counts are deterministic.
            1,
            ExportDirs::default(),
            Some(crate::immich_upload::UploadConfig {
                base_url: "http://127.0.0.1:1".to_string(),
                key: "irrelevant".to_string(),
            }),
        );
        let mut notices = 0;
        let mut report = None;
        while let Ok(ev) = worker.events().recv() {
            match ev {
                ExportEvent::UploadUnconfirmed { at, .. } => {
                    notices += 1;
                    assert!(at.exists(), "the notice must name a file that exists");
                }
                ExportEvent::Finished(r) => {
                    report = Some(r);
                    break;
                }
                _ => {}
            }
        }
        let report = report.expect("the worker ended without a Finished event");
        assert_eq!(notices, 1, "exactly one notice for a run with two failures");
        assert_eq!(
            report.unconfirmed.len(),
            2,
            "both failures must still be reported"
        );
    }

    /// Export one flat photo named `name` with `o`, and read the written PNG back.
    fn exported_png(
        d: &TempDir,
        name: &str,
        o: ExportOptions,
        dirs: ExportDirs,
    ) -> (ExportReport, image::RgbImage) {
        let worker = ExportWorker::spawn(
            jobs(d, &[name]),
            o,
            |_: &Path| Ok(source(flat(200, 100))),
            Arc::new(AtomicBool::new(false)),
            CPU,
            1,
            dirs,
            None,
        );
        let r = finished(&worker);
        let stem = Path::new(name).file_stem().unwrap().to_string_lossy();
        let img = image::open(d.0.join(format!("{stem}.png")))
            .expect("the export")
            .to_rgb8();
        (r, img)
    }

    /// M10.10a: the logo the run read reaches the written file, in its corner and nowhere else.
    #[test]
    fn an_image_watermark_reaches_the_written_file() {
        let d = TempDir::new("wm_drawn");
        let store = d.0.join("watermarks");
        std::fs::create_dir_all(&store).unwrap();
        write_png(&store.join("logo.png"), 20, 10, [255, 0, 0, 255]);
        let dirs = || ExportDirs {
            looks: None,
            watermarks: Some(store.clone()),
        };
        let mut o = opts(&d, None);
        o.format = ExportFormat::Png8;
        let (_, plain) = exported_png(&d, "plain.RAF", o.clone(), dirs());
        o.watermark = Watermark {
            kind: WatermarkKind::Image,
            image: "logo.png".into(),
            image_width: 20,
            opacity: 100,
            ..Watermark::default()
        };
        let (r, marked) = exported_png(&d, "marked.RAF", o, dirs());
        assert!(r.notes.is_empty(), "{:?}", r.notes);
        let mut red = 0;
        for (x, y, p) in marked.enumerate_pixels() {
            if p != plain.get_pixel(x, y) {
                assert!(x >= 100 && y >= 50, "ink at ({x},{y}), outside its corner");
                if p[0] > 200 && p[1] < 60 {
                    red += 1;
                }
            }
        }
        // 20% of a 200 px long edge: a 40×20 logo, 800 pixels.
        assert!(red > 600, "{red} red pixels");
    }

    /// M10.10a: a copy that is gone is named, and the photo still exports — never silently
    /// without its mark. A text mark needs no store and says nothing.
    #[test]
    fn a_missing_watermark_image_is_named_and_the_export_still_runs() {
        let d = TempDir::new("wm_missing");
        let mut o = opts(&d, None);
        o.format = ExportFormat::Png8;
        o.watermark.text = "© me".into();
        let (r, _) = exported_png(&d, "text.RAF", o.clone(), ExportDirs::default());
        assert!(r.notes.is_empty(), "a text mark: {:?}", r.notes);
        o.watermark.kind = WatermarkKind::Image;
        o.watermark.image = "gone.png".into();
        let dirs = ExportDirs {
            looks: None,
            watermarks: Some(d.0.join("watermarks")),
        };
        let (r, _) = exported_png(&d, "image.RAF", o, dirs);
        assert_eq!(r.exported, 1, "the photo still exports");
        assert_eq!(r.notes, ["the watermark image gone.png could not be read"]);
    }

    #[test]
    fn the_summary_reads_as_before_and_appends_a_note() {
        let r = ExportReport {
            exported: 2,
            skipped: 1,
            ..ExportReport::default()
        };
        assert_eq!(r.summary(), "Exported 2, 1 skipped");
        let r = ExportReport {
            exported: 2,
            failed: vec![(PathBuf::from("a.RAF"), "no".into())],
            cancelled: true,
            notes: vec!["the watermark image x.png could not be read".into()],
            ..ExportReport::default()
        };
        assert_eq!(
            r.summary(),
            "Exported 2, 1 failed, cancelled, the watermark image x.png could not be read"
        );
    }

    /// Export one `grainy` 600 × 400 photo as a JPEG with `o`: the report, the file's bytes, its size.
    fn exported_jpeg(
        d: &TempDir,
        name: &str,
        o: ExportOptions,
    ) -> (ExportReport, Vec<u8>, (u32, u32)) {
        let worker = ExportWorker::spawn(
            jobs(d, &[name]),
            o,
            |_: &Path| Ok(source(grainy(600, 400))),
            Arc::new(AtomicBool::new(false)),
            CPU,
            1,
            ExportDirs::default(),
            None,
        );
        let r = finished(&worker);
        let stem = Path::new(name).file_stem().unwrap().to_string_lossy();
        let Ok(bytes) = std::fs::read(d.0.join(format!("{stem}.jpg"))) else {
            return (r, Vec::new(), (0, 0));
        };
        let img = image::load_from_memory(&bytes).expect("a JPEG");
        (r, bytes.clone(), (img.width(), img.height()))
    }

    /// M10.10b: a limit the quality can meet is met by the quality alone; one it cannot is met by
    /// fewer pixels, and the report says so.
    #[test]
    fn a_file_size_limit_is_met_by_the_quality_then_by_the_pixels() {
        let d = TempDir::new("limit_met");
        let o = opts(&d, None);
        // ⚠ The bounds are measured on this very photo, not invented: the limit sits between the
        // file at the floor and the file at 100.
        let at = |q: u8, name: &str| {
            let (_, bytes, _) = exported_jpeg(
                &d,
                name,
                ExportOptions {
                    quality: q,
                    ..o.clone()
                },
            );
            bytes.len()
        };
        let (floor, best) = (at(40, "q40.RAF"), at(100, "q100.RAF"));
        assert!(best > floor * 2, "the sample: {floor} at 40, {best} at 100");
        let between = (floor + best) / 2;
        let kb = (between / 1000) as u32;
        // ⚠ Quality 100 asked, whose file is over the limit by construction: the limit, not the
        // slider, must decide. With the default 90 this half passed before the limit existed.
        let limited = ExportOptions {
            quality: 100,
            limit: FileSizeLimit { on: true, kb },
            ..o.clone()
        };
        let (r, bytes, dims) = exported_jpeg(&d, "fits.RAF", limited);
        assert!(
            bytes.len() <= kb as usize * 1000,
            "{} over {kb} KB",
            bytes.len()
        );
        assert!(bytes.len() > floor, "the quality was not simply floored");
        assert_eq!(dims, (600, 400), "the quality alone met it");
        assert!(r.notes.is_empty(), "{:?}", r.notes);

        let kb = ((floor / 3) / 1000).max(1) as u32;
        let tight = ExportOptions {
            limit: FileSizeLimit { on: true, kb },
            ..o
        };
        let (r, bytes, dims) = exported_jpeg(&d, "shrunk.RAF", tight);
        assert!(
            bytes.len() <= kb as usize * 1000,
            "{} over {kb} KB",
            bytes.len()
        );
        assert!(dims.0 < 600 && dims.1 < 400, "{dims:?}");
        assert!(
            (dims.0 as f32 / dims.1 as f32 - 1.5).abs() < 0.05,
            "the shape kept: {dims:?}"
        );
        assert_eq!(r.exported, 1);
        assert_eq!(r.notes, [format!("1 photo made smaller to fit {kb} KB")]);
    }

    /// M10.10b: metadata bigger than the limit cannot be met by any picture — that photo fails with
    /// a reason, and nothing over the limit is written.
    #[test]
    fn a_limit_smaller_than_the_metadata_fails_that_photo() {
        let d = TempDir::new("limit_meta");
        let mut o = opts(&d, None);
        o.metadata.caption = "a long caption ".repeat(140);
        o.limit = FileSizeLimit { on: true, kb: 1 };
        let (r, bytes, _) = exported_jpeg(&d, "a.RAF", o);
        assert_eq!(r.exported, 0);
        assert_eq!(r.failed.len(), 1, "{r:?}");
        assert!(r.failed[0].1.contains("1 KB"), "{}", r.failed[0].1);
        assert!(bytes.is_empty(), "nothing written");
    }

    /// M10.10b: a print size its pixels cannot fill is named once for the run.
    #[test]
    fn photos_below_the_asked_ppi_are_named() {
        let d = TempDir::new("below_ppi");
        let o = ExportOptions {
            size: OutputSize {
                fit: FitMode::LongEdge,
                unit: LengthUnit::Cm,
                long_edge: 100.0,
                ppi: 300,
                ..OutputSize::default()
            },
            ..opts(&d, None)
        };
        let (r, _, dims) = exported_jpeg(&d, "a.RAF", o);
        assert_eq!(dims, (600, 400), "its own pixels");
        assert_eq!(r.notes, ["1 photo below 300 PPI"]);
        let r = ExportReport {
            exported: 3,
            below_ppi: 2,
            notes: vec!["2 photos below 300 PPI".into()],
            ..ExportReport::default()
        };
        assert_eq!(r.summary(), "Exported 3, 2 photos below 300 PPI");
    }
}
