# Batch Export Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Export the current selection to a folder as JPEGs, with quality and an optional long-edge cap, plus progress, cancellation and a per-file collision prompt.

**Architecture:** One new `app` lib module, `batch_export`, running a **sequential** worker thread with an **injected decode fn** — so the whole loop is unit-testable with no image fixtures. Each photo goes `decode → downscale_to → build_pipeline(scale) → save_jpeg`, downscaling in linear light *before* the pipeline. A blocking request/response channel pair carries collision prompts to the UI. **No pipeline stage is touched — the 23 goldens must stay byte-identical.**

**Tech Stack:** Rust workspace, `std::thread` + `mpsc`, `AtomicBool`, `egui`/`eframe` 0.35, `rfd` for the folder picker.

**Spec:** `docs/superpowers/specs/2026-08-08-batch-export-design.md` (approved — do not re-derive).

---

## File Structure

| File | Responsibility | Change |
|---|---|---|
| `crates/app/src/batch_export.rs` | options, events, the worker loop, naming and collisions | **Create** |
| `crates/app/src/lib.rs` | register the module | Modify |
| `crates/app/src/main.rs` | remove the sidecar bug; export dialog, progress line, collision dialog | Modify |
| `CLAUDE.md` | status section — Phase 2 complete | Modify |

Every command assumes:

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:$PKG_CONFIG_PATH"
```

**Package names:** crate `app`, lib `rustroom_app`, binary `rustroom-gui`. Use `cargo test -p app`.

---

## Task 1: Fix the sidecar-next-to-export bug

Smallest, most independent piece, and a real correctness fix. Do it first so it is not tangled
with the new feature.

**Files:** Modify `crates/app/src/main.rs`

- [ ] **Step 1: Remove the line**

In `export_dialog`, delete the `sidecar::save(&out, &edits)?;` line so the closure reads:

```rust
        std::thread::spawn(move || {
            let result = (|| -> anyhow::Result<()> {
                let image = decode_for_export(&src).map_err(|e| anyhow::anyhow!(e))?;
                let rendered =
                    pipeline::build_pipeline(&edits, pipeline::RenderScale::FULL).run(image);
                export::save_jpeg(&rendered, &out, 92)?;
                Ok(())
            })();
            if let Err(e) = result {
                eprintln!("export error: {e}");
            }
        });
```

- [ ] **Step 2: Explain why, in the code**

Add this doc line just above `fn export_dialog`, after its existing doc comment (or as the
first line if it has none):

```rust
    /// ⚠ Deliberately writes **no** sidecar next to the output. `sidecar::path` appends
    /// `.json`, so that would produce `export.jpg.json` — and JPEG is a supported *input*
    /// format, so reopening the exported file would load those edits and apply them a second
    /// time to pixels that already have them baked in. The source's sidecar is written by
    /// `persist_current_session` regardless.
```

- [ ] **Step 3: Verify**

```bash
cargo build -p app
cargo clippy -p app --all-targets -- -D warnings
```

Expected: builds clean, no warnings. (`sidecar` is still imported — `persist_current_session`
and `finalize_session` use it.)

- [ ] **Step 4: Commit**

```bash
cargo fmt --all
git add crates/app/src/main.rs
git commit -m "fix(app): stop writing a sidecar beside the exported file

sidecar::path appends .json, so this produced export.jpg.json -- and JPEG is a
supported input format, so reopening an exported file applied its edits a
second time to pixels that already had them baked in."
```

---

## Task 2: `batch_export` types and the naming rule

**Files:** Create `crates/app/src/batch_export.rs`; modify `crates/app/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Create `crates/app/src/batch_export.rs` with **only** this test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_destination_is_the_stem_plus_jpg() {
        let dir = PathBuf::from("/out");
        assert_eq!(
            dest_for(&dir, Path::new("/in/DSCF6509.RAF"), 0),
            PathBuf::from("/out/DSCF6509.jpg")
        );
        assert_eq!(
            dest_for(&dir, Path::new("/in/holiday.jpeg"), 0),
            PathBuf::from("/out/holiday.jpg")
        );
    }

    #[test]
    fn a_nonzero_index_adds_a_suffix() {
        // What "Keep both" uses: the first free name-1, name-2, ...
        let dir = PathBuf::from("/out");
        assert_eq!(
            dest_for(&dir, Path::new("/in/a.RAF"), 1),
            PathBuf::from("/out/a-1.jpg")
        );
        assert_eq!(
            dest_for(&dir, Path::new("/in/a.RAF"), 7),
            PathBuf::from("/out/a-7.jpg")
        );
    }

    #[test]
    fn a_source_with_no_stem_still_produces_a_name() {
        // Defensive: `file_stem` is an Option, and a path ending in `..` has none.
        let dir = PathBuf::from("/out");
        let got = dest_for(&dir, Path::new("/in/.."), 0);
        assert!(got.starts_with("/out"));
        assert_eq!(got.extension().and_then(|e| e.to_str()), Some("jpg"));
    }
}
```

- [ ] **Step 2: Register the module**

In `crates/app/src/lib.rs`, add after `pub mod action_log;`:

```rust
pub mod batch_export;
```

- [ ] **Step 3: Run to verify it fails**

```bash
cargo test -p app --lib batch_export
```

Expected: FAIL to compile — `cannot find function 'dest_for'`.

- [ ] **Step 4: Implement the types and the naming rule**

Prepend to `crates/app/src/batch_export.rs`:

```rust
//! Exporting a selection of photos to JPEG files.
//!
//! **Sequential on purpose.** The pipeline stages are already rayon-parallel *internally*, so
//! exporting several photos at once would contend for the same cores while multiplying peak
//! memory — a 26 Mpx decode is ~312 MB as f32 RGB, and `PipelineStage::apply` consumes and
//! produces buffers. One at a time keeps the ceiling at roughly one image.
//!
//! The decode fn is **injected**, the same shape `DecodeWorker::new` uses, so the whole loop —
//! naming, collisions, cancellation, progress, the failure path — is testable with no image
//! fixtures at all.

use core_image::ImageBuf;
use edit_model::Edits;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

/// What to export, and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportOptions {
    pub dest_dir: PathBuf,
    /// JPEG quality, 1..=100.
    pub quality: u8,
    /// Fit within this many pixels on the long edge. `None` = full resolution.
    pub max_edge: Option<usize>,
}

/// What to do about one destination that already exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Collision {
    Skip,
    Overwrite,
    KeepBoth,
}

/// The user's answer to a collision prompt.
///
/// ⚠ `Cancel` carries **no** `apply_to_all`, on purpose: ticking "apply to all" and then
/// cancelling must arm nothing, and making that unrepresentable is better than enforcing it
/// with a rule someone can forget. Compare `Settings::apply_delete_confirm`, where the flag
/// and the outcome are separate values held together only by a test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Resolve {
        collision: Collision,
        apply_to_all: bool,
    },
    Cancel,
}

/// Progress from the worker.
#[derive(Debug, Clone, PartialEq)]
pub enum ExportEvent {
    Started { total: usize },
    Item { done: usize, total: usize, name: String },
    /// The worker is now **blocked** waiting for a [`Decision`].
    Collision { dest: PathBuf },
    Finished(ExportReport),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExportReport {
    pub exported: usize,
    pub skipped: usize,
    pub failed: Vec<(PathBuf, String)>,
    pub cancelled: bool,
}

/// The destination for one source file: `<dir>/<stem>.jpg`, or `<dir>/<stem>-N.jpg`.
///
/// A source with no file stem falls back to `export`, so a pathological path still yields a
/// writable name rather than panicking.
pub fn dest_for(dir: &Path, source: &Path, index: usize) -> PathBuf {
    let stem = source
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("export");
    let name = if index == 0 {
        format!("{stem}.jpg")
    } else {
        format!("{stem}-{index}.jpg")
    };
    dir.join(name)
}
```

- [ ] **Step 5: Run to verify**

```bash
cargo test -p app --lib batch_export
cargo clippy -p app --all-targets -- -D warnings
```

Expected: 3 tests PASS, no warnings.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add crates/app/src/batch_export.rs crates/app/src/lib.rs
git commit -m "feat(app): add batch export types and the destination naming rule"
```

---

## Task 3: Exporting one photo

**Files:** Modify `crates/app/src/batch_export.rs`

- [ ] **Step 1: Write the failing tests**

Append inside `#[cfg(test)] mod tests`:

```rust
    /// A unique temp directory for one test, removed on drop.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let p = std::env::temp_dir().join(format!("rustroom_export_{tag}"));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// A flat mid-gray image of the given size, standing in for a decode.
    fn flat(w: usize, h: usize) -> ImageBuf {
        let mut img = ImageBuf::new(w, h);
        for y in 0..h {
            for x in 0..w {
                img.set_pixel(x, y, [0.25, 0.25, 0.25]);
            }
        }
        img
    }

    fn opts(dir: &TempDir, max_edge: Option<usize>) -> ExportOptions {
        ExportOptions {
            dest_dir: dir.0.clone(),
            quality: 90,
            max_edge,
        }
    }

    /// Read back a written JPEG's dimensions.
    fn jpeg_size(path: &Path) -> (u32, u32) {
        let img = image::open(path).expect("a readable jpeg");
        (img.width(), img.height())
    }

    #[test]
    fn export_one_writes_a_jpeg_at_full_size_without_a_cap() {
        let d = TempDir::new("one_full");
        let dest = d.0.join("a.jpg");
        export_one(flat(40, 20), &Edits::default(), &dest, &opts(&d, None)).unwrap();
        assert_eq!(jpeg_size(&dest), (40, 20));
    }

    #[test]
    fn the_long_edge_cap_is_honoured() {
        let d = TempDir::new("one_cap");
        let dest = d.0.join("a.jpg");
        export_one(flat(40, 20), &Edits::default(), &dest, &opts(&d, Some(10))).unwrap();
        assert_eq!(jpeg_size(&dest), (10, 5));
    }

    #[test]
    fn a_cap_larger_than_the_image_does_not_upscale() {
        // `downscale_to` never upscales; the export must not either.
        let d = TempDir::new("one_big_cap");
        let dest = d.0.join("a.jpg");
        export_one(flat(40, 20), &Edits::default(), &dest, &opts(&d, Some(999))).unwrap();
        assert_eq!(jpeg_size(&dest), (40, 20));
    }

    #[test]
    fn an_unwritable_destination_is_an_error_not_a_panic() {
        let d = TempDir::new("one_bad");
        let dest = d.0.join("no_such_dir").join("a.jpg");
        assert!(export_one(flat(4, 4), &Edits::default(), &dest, &opts(&d, None)).is_err());
    }

    #[test]
    fn the_render_scale_follows_the_downscale() {
        // ⚠ The reason the resize happens *before* the pipeline: the spatial stages have to
        // be told the output resolution, or a resized export would apply full-resolution
        // radii to a small image.
        assert_eq!(scale_for(40, None).get(), 1.0);
        assert_eq!(scale_for(40, Some(10)).get(), 0.25);
        // No upscaling, so a cap above the source is still 1.0.
        assert_eq!(scale_for(40, Some(999)).get(), 1.0);
    }
```

- [ ] **Step 2: Add the `image` dev-dependency**

`jpeg_size` reads the written file back. In `crates/app/Cargo.toml`, add after the
`[dependencies]` block:

```toml
[dev-dependencies]
image = { workspace = true }
```

- [ ] **Step 3: Run to verify it fails**

```bash
cargo test -p app --lib batch_export
```

Expected: FAIL to compile — `cannot find function 'export_one'`.

- [ ] **Step 4: Implement**

Add to `crates/app/src/batch_export.rs`, after `dest_for`:

```rust
/// The render scale a `max_edge` cap implies for a source whose long edge is `source_long`.
///
/// Mirrors `render_engine::render_scale`, with `source_scale` fixed at 1.0 because an export
/// always decodes at full resolution — never through `DecodeOptions::preview()`.
pub fn scale_for(source_long: usize, max_edge: Option<usize>) -> pipeline::RenderScale {
    let Some(max_edge) = max_edge else {
        return pipeline::RenderScale::FULL;
    };
    if source_long == 0 || max_edge >= source_long {
        // `downscale_to` never upscales, so neither does the scale.
        return pipeline::RenderScale::FULL;
    }
    pipeline::RenderScale::new(max_edge as f32 / source_long as f32)
}

/// Render one decoded image through `edits` and write it as a JPEG.
///
/// ⚠ The downscale happens **before** the pipeline, not after. Rendering at full resolution
/// and shrinking the result would resample **sRGB-encoded** pixels, which is gamma-incorrect;
/// `downscale_to` here operates on the linear decode, where averaging is meaningful. It is
/// also the path the preview already takes, and it is faster, since every stage then runs on
/// fewer pixels.
pub fn export_one(
    image: ImageBuf,
    edits: &Edits,
    dest: &Path,
    opts: &ExportOptions,
) -> Result<(), String> {
    let source_long = image.width().max(image.height());
    let scaled = match opts.max_edge {
        Some(max) => image.downscale_to(max),
        None => image,
    };
    let scale = scale_for(source_long, opts.max_edge);
    let rendered = pipeline::build_pipeline(edits, scale).run(scaled);
    export::save_jpeg(&rendered, dest, opts.quality).map_err(|e| e.to_string())
}
```

- [ ] **Step 5: Run to verify**

```bash
cargo test -p app --lib batch_export
cargo clippy -p app --all-targets -- -D warnings
```

Expected: 8 tests PASS, no warnings.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add crates/app/Cargo.toml crates/app/src/batch_export.rs
git commit -m "feat(app): export one photo, downscaling in linear before the pipeline"
```

---

## Task 4: The worker loop

**Files:** Modify `crates/app/src/batch_export.rs`

- [ ] **Step 1: Write the failing tests**

Append inside `#[cfg(test)] mod tests`:

```rust
    /// Drive a whole export to completion, answering every collision prompt with `answer`.
    ///
    /// Returns the report plus every event, so a test can assert on progress as well as the
    /// outcome. `decode` receives the source path so a test can fail one specific photo.
    fn run_export(
        jobs: Vec<(PathBuf, Edits)>,
        opts: ExportOptions,
        answer: Vec<Decision>,
        decode: impl Fn(&Path) -> Result<ImageBuf, String> + Send + 'static,
    ) -> (ExportReport, Vec<ExportEvent>) {
        let worker = ExportWorker::spawn(jobs, opts, decode);
        let mut answers = answer.into_iter();
        let mut events = Vec::new();
        let mut report = ExportReport::default();
        while let Ok(ev) = worker.events().recv() {
            match &ev {
                ExportEvent::Collision { .. } => {
                    let d = answers.next().unwrap_or(Decision::Cancel);
                    worker.decide(d);
                }
                ExportEvent::Finished(r) => report = r.clone(),
                _ => {}
            }
            let finished = matches!(ev, ExportEvent::Finished(_));
            events.push(ev);
            if finished {
                break;
            }
        }
        (report, events)
    }

    fn jobs(dir: &TempDir, names: &[&str]) -> Vec<(PathBuf, Edits)> {
        names
            .iter()
            .map(|n| (dir.0.join(n), Edits::default()))
            .collect()
    }

    fn ok_decode() -> impl Fn(&Path) -> Result<ImageBuf, String> + Send + 'static {
        |_: &Path| Ok(flat(8, 4))
    }

    #[test]
    fn every_photo_produces_a_jpeg() {
        let d = TempDir::new("all");
        let (report, events) =
            run_export(jobs(&d, &["a.RAF", "b.RAF"]), opts(&d, None), vec![], ok_decode());

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

    #[test]
    fn two_sources_with_the_same_stem_collide_in_an_empty_folder() {
        // ⚠ `a.RAF` and `a.jpg` both map to `a.jpg`, so the check has to consider names
        // claimed *in this run*, not only files already on disk.
        let d = TempDir::new("same_stem");
        let (report, _) = run_export(
            jobs(&d, &["a.RAF", "a.jpg"]),
            opts(&d, None),
            vec![Decision::Resolve {
                collision: Collision::KeepBoth,
                apply_to_all: false,
            }],
            ok_decode(),
        );
        assert_eq!(report.exported, 2);
        assert!(d.0.join("a.jpg").exists() && d.0.join("a-1.jpg").exists());
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
                    Ok(flat(8, 4))
                }
            },
        );
        assert_eq!(report.exported, 1);
        assert_eq!(report.failed.len(), 1);
        assert!(report.failed[0].0.ends_with("bad.RAF"));
        assert!(d.0.join("good.jpg").exists());
    }

    #[test]
    fn the_cancel_flag_stops_the_run_between_photos() {
        // libraw's `unpack` is not interruptible, so cancellation is checked between items;
        // setting the flag before the run starts must produce an immediate, clean stop.
        let d = TempDir::new("cancel_flag");
        let worker = ExportWorker::spawn(jobs(&d, &["a.RAF", "b.RAF"]), opts(&d, None), ok_decode());
        worker.cancel();
        let mut report = ExportReport::default();
        while let Ok(ev) = worker.events().recv() {
            if let ExportEvent::Finished(r) = ev {
                report = r;
                break;
            }
        }
        assert!(report.cancelled);
        assert_eq!(report.exported, 0);
    }
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test -p app --lib batch_export
```

Expected: FAIL to compile — `cannot find type 'ExportWorker'`.

- [ ] **Step 3: Implement the worker**

Add to `crates/app/src/batch_export.rs`, after `export_one`:

```rust
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
    /// Start exporting `jobs` — `(source path, that photo's edits)` — into `opts.dest_dir`.
    pub fn spawn(
        jobs: Vec<(PathBuf, Edits)>,
        opts: ExportOptions,
        decode: impl Fn(&Path) -> Result<ImageBuf, String> + Send + 'static,
    ) -> Self {
        let (event_tx, events) = channel();
        let (decisions, decision_rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        std::thread::spawn(move || {
            run(jobs, opts, decode, event_tx, decision_rx, flag);
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

    /// Ask the worker to stop. Takes effect **between photos**: libraw's `unpack` is not
    /// interruptible, so the photo in flight finishes first — up to ~11 s.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

fn run(
    jobs: Vec<(PathBuf, Edits)>,
    opts: ExportOptions,
    decode: impl Fn(&Path) -> Result<ImageBuf, String>,
    events: Sender<ExportEvent>,
    decisions: Receiver<Decision>,
    cancel: Arc<AtomicBool>,
) {
    let total = jobs.len();
    let mut report = ExportReport::default();
    // Names produced during this run. ⚠ Without this, two sources sharing a stem
    // (`a.RAF`, `a.jpg`) would silently overwrite each other in an empty folder.
    let mut claimed: HashSet<PathBuf> = HashSet::new();
    let mut remembered: Option<Collision> = None;

    if events.send(ExportEvent::Started { total }).is_err() {
        return;
    }

    for (done, (source, edits)) in jobs.into_iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            report.cancelled = true;
            break;
        }

        let mut dest = dest_for(&opts.dest_dir, &source, 0);
        if claimed.contains(&dest) || dest.exists() {
            let collision = match remembered {
                Some(c) => c,
                None => {
                    if events
                        .send(ExportEvent::Collision { dest: dest.clone() })
                        .is_err()
                    {
                        return;
                    }
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
                    report.skipped += 1;
                    let _ = events.send(ExportEvent::Item {
                        done: done + 1,
                        total,
                        name: file_label(&source),
                    });
                    continue;
                }
                Collision::Overwrite => {}
                Collision::KeepBoth => {
                    let mut i = 1;
                    loop {
                        let candidate = dest_for(&opts.dest_dir, &source, i);
                        if !claimed.contains(&candidate) && !candidate.exists() {
                            dest = candidate;
                            break;
                        }
                        i += 1;
                    }
                }
            }
        }

        let _ = events.send(ExportEvent::Item {
            done: done + 1,
            total,
            name: file_label(&source),
        });

        match decode(&source).and_then(|img| export_one(img, &edits, &dest, &opts)) {
            Ok(()) => {
                report.exported += 1;
                claimed.insert(dest);
            }
            Err(e) => report.failed.push((source, e)),
        }
    }

    let _ = events.send(ExportEvent::Finished(report));
}

/// A short display name for the progress line.
fn file_label(path: &Path) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}
```

- [ ] **Step 4: Run to verify**

```bash
cargo test -p app --lib batch_export
cargo clippy -p app --all-targets -- -D warnings
```

Expected: 17 tests PASS, no warnings.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all
git add crates/app/src/batch_export.rs
git commit -m "feat(app): add the sequential batch-export worker"
```

---

## Task 5: The export dialog and progress UI

**Files:** Modify `crates/app/src/main.rs`

- [ ] **Step 1: Add the imports and state**

Add to the `use` block:

```rust
use rustroom_app::batch_export::{
    Collision, Decision, ExportEvent, ExportOptions, ExportReport, ExportWorker,
};
```

In `enum Modal`, add before `Settings`:

```rust
    /// Choosing where and how to export the selection.
    ExportSettings,
```

In `struct RustroomApp`, add after `sync_groups`:

```rust
    /// Last-used export options, so a second export does not re-ask for everything.
    export_opts: ExportOptions,
    /// The running batch export, if any.
    exporter: Option<ExportWorker>,
    /// Progress line for the running export.
    export_progress: String,
    /// The destination awaiting a collision answer; `Some` means the worker is blocked.
    export_collision: Option<PathBuf>,
    /// Scratch state for the collision dialog's "apply to all" tick.
    export_apply_all: bool,
```

In `RustroomApp::new`, add after `sync_groups: EditGroups::default(),`:

```rust
            export_opts: ExportOptions {
                dest_dir: PathBuf::new(),
                quality: 92,
                max_edge: None,
            },
            exporter: None,
            export_progress: String::new(),
            export_collision: None,
            export_apply_all: false,
```

- [ ] **Step 2: Add the pump and the launcher**

Add these to the `impl RustroomApp` block containing `run_sync`:

```rust
    /// Sources and edits for a batch export: every selected photo, using the open photo's
    /// **in-memory** edits so unsaved changes are exported, and each other photo's sidecar.
    fn export_jobs(&self) -> Vec<(PathBuf, Edits)> {
        let open = self.session.as_ref().map(|s| (s.path.clone(), s.edits.clone()));
        let Some(lib) = &self.library else {
            return open.into_iter().collect();
        };
        lib.selection
            .iter()
            .filter_map(|i| lib.entries.get(i).map(|e| e.path.clone()))
            .map(|p| match &open {
                Some((op, edits)) if *op == p => (p.clone(), edits.clone()),
                _ => {
                    let edits = sidecar::load(&p).unwrap_or_default();
                    (p, edits)
                }
            })
            .collect()
    }

    /// Start exporting the selection.
    fn start_export(&mut self) {
        let jobs = self.export_jobs();
        if jobs.is_empty() {
            self.status = "Nothing selected to export".to_string();
            return;
        }
        let opts = self.export_opts.clone();
        self.export_progress = format!("Exporting 0 of {}…", jobs.len());
        self.export_collision = None;
        self.exporter = Some(ExportWorker::spawn(jobs, opts, |p: &std::path::Path| {
            decode_for_export(p)
        }));
    }

    /// Drain the exporter's events once per frame.
    fn pump_export(&mut self, ctx: &egui::Context) {
        let Some(worker) = &self.exporter else { return };
        let mut finished: Option<ExportReport> = None;
        while let Ok(event) = worker.events().try_recv() {
            match event {
                ExportEvent::Started { total } => {
                    self.export_progress = format!("Exporting 0 of {total}…");
                }
                ExportEvent::Item { done, total, name } => {
                    self.export_progress = format!("Exporting {done} of {total} — {name}");
                }
                ExportEvent::Collision { dest } => {
                    self.export_collision = Some(dest);
                    self.export_apply_all = false;
                }
                ExportEvent::Finished(report) => finished = Some(report),
            }
        }
        if let Some(r) = finished {
            self.exporter = None;
            self.export_progress.clear();
            self.export_collision = None;
            let mut parts = vec![format!("Exported {}", r.exported)];
            if r.skipped > 0 {
                parts.push(format!("{} skipped", r.skipped));
            }
            if !r.failed.is_empty() {
                parts.push(format!("{} failed", r.failed.len()));
            }
            if r.cancelled {
                parts.push("cancelled".to_string());
            }
            self.status = parts.join(", ");
        }
        // The worker runs off the UI thread, so nothing else would wake the app up.
        if self.exporter.is_some() {
            ctx.request_repaint_after(Duration::from_millis(100));
        }
    }
```

- [ ] **Step 3: Call the pump**

In `fn ui`, immediately after `self.handle_clipboard_keys(&ctx);`, add:

```rust
        self.pump_export(&ctx);
```

- [ ] **Step 4: Add the toolbar button and the progress line**

In `develop_toolbar`, immediately after the `Sync {}…` button block, add:

```rust
            if ui
                .add_enabled(selected > 1, egui::Button::new(format!("Export {selected}…")))
                .clicked()
            {
                self.modal = Modal::ExportSettings;
            }
```

Then, in `fn ui`'s top panel, replace

```rust
                if !self.status.is_empty() {
                    ui.label(&self.status);
                }
```

with

```rust
                if !self.export_progress.is_empty() {
                    ui.separator();
                    ui.label(&self.export_progress);
                    if ui.button("Cancel").clicked() {
                        if let Some(w) = &self.exporter {
                            w.cancel();
                        }
                        // libraw's unpack is not interruptible, so say why nothing has
                        // stopped yet rather than looking frozen.
                        self.export_progress = "Cancelling after this photo…".to_string();
                    }
                } else if !self.status.is_empty() {
                    ui.label(&self.status);
                }
```

- [ ] **Step 5: Build**

```bash
cargo build -p app
```

Expected: builds, with dead-code warnings for `export_collision`, `export_apply_all` and the
`Modal::ExportSettings` variant — all forward references consumed by Task 6.

**Do not run `clippy -- -D warnings` here**: those warnings are real and would fail the gate.
Run the strict lint at the end of Task 6, when the code is actually complete. Papering over
them with `#[allow(dead_code)]` would mean remembering to remove three attributes later.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
git add crates/app/src/main.rs
git commit -m "feat(app): wire the batch-export worker into the app with a progress line"
```

---

## Task 6: The export and collision dialogs

**Files:** Modify `crates/app/src/main.rs`

- [ ] **Step 1: Add both dialogs**

Add these next to `sync_modal`:

```rust
    /// Where and how to export the selection.
    fn export_modal(&mut self, ctx: &egui::Context) {
        let mut opts = self.export_opts.clone();
        let count = self.export_jobs().len();
        let mut close = false;
        let mut start = false;
        // egui has no folder field, so the picker writes into a local and the label shows it.
        let mut pick_dir = false;

        let dismissed = modal_dialog(ctx, "export_settings", "Export", |ui| {
            ui.label(format!("Export {count} photos as JPEG."));
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Destination…").clicked() {
                    pick_dir = true;
                }
                let shown = if opts.dest_dir.as_os_str().is_empty() {
                    "(none chosen)".to_string()
                } else {
                    opts.dest_dir.display().to_string()
                };
                ui.label(shown);
            });
            ui.add(egui::Slider::new(&mut opts.quality, 1..=100).text("JPEG quality"));

            let mut resize = opts.max_edge.is_some();
            ui.checkbox(&mut resize, "Fit within a long edge");
            if resize {
                let mut edge = opts.max_edge.unwrap_or(2048);
                ui.add(egui::Slider::new(&mut edge, 256..=8192).text("Long edge (px)"));
                opts.max_edge = Some(edge);
            } else {
                opts.max_edge = None;
            }

            ui.separator();
            ui.horizontal(|ui| {
                let ready = !opts.dest_dir.as_os_str().is_empty();
                if ui
                    .add_enabled(ready, egui::Button::new("Export"))
                    .clicked()
                {
                    start = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });

        if pick_dir {
            if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                opts.dest_dir = dir;
            }
        }
        self.export_opts = opts;
        if start {
            self.start_export();
            close = true;
        }
        if close || dismissed {
            self.modal = Modal::None;
        }
    }

    /// One destination already exists: what should happen to it?
    ///
    /// Drawn whenever `export_collision` is set, which means the worker is **blocked**
    /// waiting. Not a `Modal` variant, because an export can be started and then collide
    /// long after the export dialog closed.
    fn collision_modal(&mut self, ctx: &egui::Context) {
        let Some(dest) = self.export_collision.clone() else {
            return;
        };
        let name = dest
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut apply_all = self.export_apply_all;
        let mut decision: Option<Decision> = None;

        let dismissed = modal_dialog(ctx, "export_collision", "File already exists", |ui| {
            ui.label(format!("“{name}” is already in the destination folder."));
            ui.checkbox(&mut apply_all, "Apply to all remaining");
            ui.separator();
            ui.horizontal(|ui| {
                for (label, collision) in [
                    ("Skip", Collision::Skip),
                    ("Overwrite", Collision::Overwrite),
                    ("Keep both", Collision::KeepBoth),
                ] {
                    if ui.button(label).clicked() {
                        decision = Some(Decision::Resolve {
                            collision,
                            apply_to_all: apply_all,
                        });
                    }
                }
                if ui.button("Cancel export").clicked() {
                    decision = Some(Decision::Cancel);
                }
            });
        });

        self.export_apply_all = apply_all;
        // ⚠ Dismissing by clicking outside cancels the run — and `Decision::Cancel` carries
        // no `apply_to_all`, so the tick cannot leak out of a cancel by construction.
        if dismissed {
            decision = Some(Decision::Cancel);
        }
        if let Some(d) = decision {
            if let Some(w) = &self.exporter {
                w.decide(d);
            }
            self.export_collision = None;
        }
    }
```

- [ ] **Step 2: Register both**

In `draw_modals`, add `Export` to the local `Which` enum, map
`Modal::ExportSettings => Which::Export` in the first match, and
`Which::Export => self.export_modal(ctx)` in the second.

Then, at the **end** of `draw_modals`, after the dispatch match, add:

```rust
        // Independent of `self.modal`: a collision arrives while no dialog is open.
        if self.export_collision.is_some() {
            self.collision_modal(ctx);
        }
```

- [ ] **Step 3: Build, lint and test**

```bash
cargo build -p app --release
cargo clippy --workspace --all-targets -- -D warnings
cargo test --all
```

Expected: all green, no warnings.

- [ ] **Step 4: Commit**

```bash
cargo fmt --all
git add crates/app/src/main.rs
git commit -m "feat(app): add the export and collision dialogs"
```

---

## Task 7: Verify, then check it by hand

**Files:** none (verification only)

- [ ] **Step 1: Full workspace verification**

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --all
```

Check **exit codes**, not just tail output.

- [ ] **Step 2: Confirm the goldens are untouched**

```bash
cargo test -p pipeline --test golden
git status --short crates/pipeline/tests/golden
```

Expected: 23 pass, `git status` prints nothing.

- [ ] **Step 3: Launch the GUI and exercise it**

```bash
cargo run --release -p app
```

Report to the user what to look at:

1. Open a folder, open a photo, select 3–4 in the filmstrip.
2. **Export 4…** → pick a destination, set quality 85, tick the long-edge cap at 1600 →
   Export. The progress line counts up and the app stays responsive.
3. The exported files are ~1600 px on the long edge and carry the edits.
4. **No `.json` files** appear beside the exports.
5. Run the same export again → the collision dialog appears, centred. Try **Skip**, then
   **Keep both** (expect `name-1.jpg`), then **Overwrite**.
6. Run again, tick **Apply to all remaining** with **Skip** — only one prompt appears.
7. Run again and hit **Cancel export** at the first prompt — the run stops, nothing further
   is written.
8. Start a large export and press **Cancel** in the toolbar — the line reads "Cancelling
   after this photo…" and stops within one photo's time.
9. Export a single photo with the old **Export…** button, then open the exported JPEG in
   Rustroom — it must look like the export, **not** doubly-processed.

- [ ] **Step 4: Confirm the output on disk**

```bash
ls -la <the destination folder> | head -20
```

Expected: one `.jpg` per exported photo, no `.json`.

---

## Task 8: Update `CLAUDE.md`

**Files:** Modify `CLAUDE.md`

- [ ] **Step 1: Update the crate layout**

Add `batch_export` to the `app/` line's lib module list.

- [ ] **Step 2: Rewrite the RESUME HERE next action**

Batch export was the last Phase 2 gap. The next action becomes **texture / clarity / dehaze**
(local-contrast siblings of sharpening, reusing `gaussian_blur` and the `RenderScale` seam).
Renumber the remaining items: texture/clarity/dehaze (1), cross-photo undo history (2,
deferred add-on). Note that **Phase 2 is complete**.

- [ ] **Step 3: Add the status section**

Insert above **"Multi-selection & sync done"**, in the house style:

```markdown
**Batch export done — Phase 2 complete.** The last gap, and the one that could not borrow
sync's shortcuts.

- **⚠ Unlike sync, this decodes.** Sync is 200 sidecar writes; batch export is 200 full-resolution
  decodes at ~11 s each. That one difference is what forced a worker thread, a progress line
  and cancellation — none of which sync needed.
- **`app::batch_export` is sequential on purpose.** The pipeline stages are already
  rayon-parallel *internally*, so exporting several photos at once would contend for the same
  cores while multiplying peak memory — a 26 Mpx decode is ~312 MB as f32 RGB. One at a time
  keeps the ceiling at roughly one image.
- **The decode fn is injected**, the `DecodeWorker` pattern, so the entire loop — naming,
  collisions, cancellation, progress, the failure path — is unit-tested **with no image
  fixtures**. Only the decode itself is untestable, and it is one line.
- **⚠ The resize happens *before* the pipeline, not after.** Rendering at full resolution and
  shrinking the result would resample **sRGB-encoded** pixels, which is gamma-incorrect;
  `downscale_to` runs on the linear decode instead, where averaging is meaningful. It is also
  the path the preview already takes, so it reuses `downscale_to` and the `RenderScale` seam
  **with no change to `pipeline` at all**, and it is faster. Consequence: sharpening and NR
  radii scale with the output, preserving the *look* rather than the pixel radius, so a resized
  export is not a downsampled full-res export and there is no output-sharpening pass.
- **⚠ A collision can happen in an empty folder.** `a.RAF` and `a.jpg` both map to `a.jpg`, so
  the worker tracks the names it has **claimed this run**, not just what is on disk.
- **⚠ `Decision::Cancel` carries no `apply_to_all`.** Ticking "apply to all" and then
  cancelling must arm nothing — and here that is unrepresentable rather than enforced by a
  rule, which is strictly better than `Settings::apply_delete_confirm`, where the flag and the
  outcome are separate values held together only by a test.
- **⚠ Cancellation is between photos.** libraw's `unpack` is not interruptible — the same
  finding behind "abandon, don't abort" for preview decodes — so the photo in flight finishes
  first, up to ~11 s. The progress line says "Cancelling after this photo…" so the delay is
  explained rather than mysterious.
- **A pre-existing bug fixed on the way in:** `export_dialog` wrote a sidecar next to the
  *exported* file. `sidecar::path` appends `.json`, producing `export.jpg.json` — and JPEG is a
  supported **input** format, so reopening an exported file loaded those edits and applied them
  **a second time** to pixels that already had them baked in. `persist_current_session` already
  writes the source's sidecar, so the line was redundant as well as wrong.
- **Accepted caveats:** (1) cancelling waits for the photo in flight; (2) a resized export is
  not a downsampled full-res export; (3) JPEG only — TIFF and PNG are a separate feature;
  (4) no filename templating, just `<stem>.jpg`; (5) no undo — export creates rather than
  destroys, and overwriting is guarded by the prompt; (6) export options are not persisted
  between runs; (7) sequential, so 47 photos is ~47 × 11 s — the `user_qual` demosaic finding
  is the lever that would move that, tracked separately.
```

- [ ] **Step 4: Update the docs index**

Add `2026-08-08-batch-export-design.md` to the `specs/` list and `2026-08-08-batch-export.md`
to the `plans/` list.

- [ ] **Step 5: Final verification**

```bash
cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --all
```

- [ ] **Step 6: Commit**

```bash
git add CLAUDE.md docs/superpowers/plans/2026-08-08-batch-export.md
git commit -m "docs: record batch export"
```

---

## Done when

- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test --all` green.
- The 23 golden references are byte-identical.
- In the GUI: select several photos, export with a long-edge cap, watch progress, cancel
  mid-run; re-run into the same folder and exercise Skip / Overwrite / Keep both, with and
  without "apply to all".
- Exporting a photo and reopening the result no longer double-applies its edits.
- `CLAUDE.md` updated — Phase 2 complete.
