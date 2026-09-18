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
    Finished(ExportReport),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExportReport {
    pub exported: usize,
    pub skipped: usize,
    pub failed: Vec<(PathBuf, String)>,
    pub cancelled: bool,
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
    ) -> Self {
        let (event_tx, events) = channel();
        let (decisions, decision_rx) = channel();
        let flag = cancel.clone();
        std::thread::spawn(move || {
            run(
                jobs,
                opts,
                decode,
                event_tx,
                decision_rx,
                flag,
                render,
                width,
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
        let mut dest = dest_named(&opts.dest_dir, &base, 0, opts.format);
        if claimed.contains(&dest) || dest.exists() {
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
                        let candidate = dest_named(&opts.dest_dir, &base, i, opts.format);
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
        });
    }
    Some(out)
}

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
) {
    let mut report = ExportReport::default();
    // ⚠ Parsed **once** per run, not per photo. ⚠ A template that will not parse falls back to
    // the default rather than failing the run: the dialog refuses to start an export with an
    // invalid one, so the only way to get here is a hand-edited preset file — and a folder named
    // the old way beats no folder at all.
    let template = Template::parse(&opts.filename_template).unwrap_or_default();

    let Some(planned) = plan(
        jobs,
        &opts,
        &template,
        &events,
        &decisions,
        &cancel,
        &mut report,
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
            s.spawn(move || loop {
                if cancel.load(Ordering::Relaxed) {
                    shared.lock().expect("export report").cancelled = true;
                    return;
                }
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(p) = planned.get(i) else { return };

                let decoded = decode(&p.source);

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

                let outcome = decoded
                    .and_then(|src| export_one(src, &p.edits, &p.dest, opts, render, &p.meta));
                {
                    // ⚠ `failed` is now in **completion** order, which is nondeterministic. Any
                    // test comparing it must sort or compare as a set.
                    let mut r = shared.lock().expect("export report");
                    match outcome {
                        Ok(()) => r.exported += 1,
                        Err(e) => r.failed.push((p.source.clone(), e)),
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
    let report = shared.into_inner().expect("export report");

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
        assert!(can_export(&good));
        assert!(
            !can_export(&ExportOptions {
                filename_template: "{camrea}".into(),
                ..good.clone()
            }),
            "an unknown token must stop the run"
        );
        assert!(
            !can_export(&ExportOptions {
                dest_dir: PathBuf::new(),
                ..good.clone()
            }),
            "an unset destination must stop the run"
        );
        assert!(!can_export(&ExportOptions {
            dest_dir: d.0.join("gone"),
            ..good
        }));
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
}
