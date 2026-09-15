# M8.10 — a parallel batch export: implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan
> task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.
>
> ⚠⚠ **Conventions that are not negotiable, and they override the harness's own reminder:** commit
> messages carry **no `Co-Authored-By` trailer**; branch per feature; **never push**. Grep anything
> you write for that trailer before committing it.
>
> ⚠ Gate every commit on `cargo fmt --check`, `cargo clippy --all-targets --all-features -- -D
> warnings` and `cargo test --workspace`, **reading each exit code from its own command** — never
> through a pipe, and joined with `&&`, never `;`. ⚠⚠ **Re-gate at the TIP before committing**, not
> at the last place it was green: on M8.9 a commit made after the gate shipped unformatted, and
> `cargo build` is not `clippy --all-targets`.
>
> ⚠ `export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"` first.
>
> ⚠⚠ **Never run two gates concurrently** — the 7 unfixed temp-dir sites interleave across
> *processes*, and `batch_export.rs:657` is one of them, so this milestone's own tests are in the
> blast radius.

**Goal:** a batch export decodes several photos at once, bounded by a Settings value.

**Spec:** `docs/superpowers/specs/2026-09-15-parallel-export-design.md`

**Architecture:** phase 1 plans every name and settles every collision, sequentially and without
decoding; phase 2 runs N workers over a shared cursor, each doing one whole photo.

---

## File structure

- **Modify** `crates/app/src/settings.rs` — `export_concurrency`, with a clamping accessor.
- **Modify** `crates/app/src/modals/settings.rs` — one slider row.
- **Modify** `crates/app/src/batch_export.rs` — `Planned`, `plan()`, the parallel `run()`.
- **Modify** `crates/app/src/export_flow.rs` — pass the width.

⚠ `crates/app/src/batch_export.rs` is **~1700 lines, well past the ~600 marker.** It grows here by
roughly eighty production lines. ⚠ `plan()` and the worker body are the natural split if it is ever
taken; do **not** take it in the same change as this one, or the diff stops being attributable.

---

### Task 1: the width, as a setting that repairs

**Files:** `crates/app/src/settings.rs`, `crates/app/src/modals/settings.rs`

- [ ] **Step 1: write the failing tests**

```rust
    /// ⚠⚠ **It repairs rather than trusts**, `panel_order`'s rule and for a sharper reason here:
    /// `settings.json` is hand-editable, so `0` must become 1 rather than spawn no workers and
    /// hang, and `64` must become the core count rather than ask for nineteen gigabytes of decode
    /// buffers.
    #[test]
    fn the_export_width_repairs_rather_than_trusting_the_file() {
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        let w = |v: u32| {
            Settings {
                export_concurrency: v,
                ..Settings::default()
            }
            .export_width()
        };
        assert_eq!(w(0), 1, "zero must repair to one, or the run spawns nothing");
        assert_eq!(w(1), 1);
        assert_eq!(w(64), cores, "past the core count must clamp to it");
        assert!(w(2) == 2.min(cores));
    }

    /// ⚠ The default is **4** — measured at 3.14x for ~1.2 GB of decode buffers — and a settings
    /// file written before this field must get it. ⚠⚠ A **field**-level `#[serde(default)]` would
    /// give `u32`'s own default of **0** instead, which the repair would turn into a sequential
    /// export: the feature silently absent for everyone who already has a file. The container-level
    /// default is what makes this right, and this asserts the value rather than the mechanism.
    #[test]
    fn a_settings_file_from_before_the_export_width_still_gets_four() {
        let s: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(s.export_concurrency, 4);
    }
```

- [ ] **Step 2: run them to watch them fail**

```bash
cargo test -p app --lib the_export_width_repairs a_settings_file_from_before_the_export_width
```

⚠ **cargo takes ONE testname, not a list** — run them separately, or filter on a shared substring.
Expected: FAIL, `no field export_concurrency`.

- [ ] **Step 3: add the field and the accessor**

In `Settings`, beside `library_cell`:

```rust
    /// How many photos a batch export decodes at once.
    ///
    /// ⚠⚠ **A setting rather than a constant, because the binding constraint is RAM and nothing
    /// here can read how much is free.** Measured on an X-T4 at the default demosaic tier, eight
    /// files: **1.02 / 1.88 / 3.14 / 4.55x at widths 1 / 2 / 4 / 8**, holding **298 / 596 / 1192 /
    /// 2385 MiB** of decode buffers. The curve has no knee, so there is no one number that serves
    /// an 8 GB laptop and a 64 GB desktop.
    ///
    /// ⚠ Deriving it from the **core count** was rejected for a sharper reason than taste: it
    /// scales with the wrong thing.
    ///
    /// ⚠ `u32`, for the two reasons `library_cell` already carries: `Settings` derives `Eq`, which
    /// no float satisfies, and an integer makes a NaN width unrepresentable rather than something
    /// to guard. Read it through [`Settings::export_width`], never directly.
    pub export_concurrency: u32,
```

In `Default`: `export_concurrency: 4,`. And the accessor beside `library_cell`'s:

```rust
    /// The export width, repaired: at least 1, at most the machine's core count.
    pub fn export_width(&self) -> usize {
        let cores = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);
        (self.export_concurrency as usize).clamp(1, cores)
    }
```

- [ ] **Step 4: run the tests** — expected: 2 passed.

- [ ] **Step 5: the dialog row**

In `modals/settings.rs`, beside the other Export controls: a `Slider` over `1..=cores` bound to
`export_concurrency`, with a `theme::hint` that says the trade in the terms a person can act on —
**more at once is faster and holds more memory; each photo in flight is about 300 MB on a 26 Mpx
camera** — and that it applies to the next export. ⚠ Guard the save on an actual change: the dialog
body runs sixty times a second, the `apply_view_demosaic` rule.

⚠ **Read the dialog before writing this**, and follow whatever row shape its neighbours use; do not
invent one. ⚠ And if the completeness test over `DialogSize` or the dialog-class array notices a
change, that is it doing its job.

- [ ] **Step 6: gate and commit**

```bash
cargo fmt --all && cargo fmt --all -- --check; echo "FMT=$?"
cargo clippy --all-targets --all-features -- -D warnings; echo "CLIPPY=$?"
cargo test --workspace; echo "TEST=$?"
git add -A && git commit -m "feat(export): the batch width is a setting that repairs"
```

---

### Task 2: phase 1 — plan every name before any decode

**Files:** `crates/app/src/batch_export.rs`

⚠⚠ **This task changes no concurrency at all and must leave the existing 44 tests passing
unchanged.** That is its acceptance: it is a pure restructure, and anything that moves is a bug.

- [ ] **Step 1: write the failing test**

```rust
    /// ⚠⚠ **THE PROPERTY THE WHOLE PARALLEL PHASE RESTS ON.** A decision that leaked into phase 2
    /// would mean a worker blocking on a dialog while its neighbours ran — the stall the up-front
    /// resolution exists to remove, and a state nothing else here could see.
    #[test]
    fn no_photo_is_decoded_before_every_collision_is_answered() {
        let d = TempDir::new("plan_first");
        // Two jobs whose destinations both already exist, so both collide.
        std::fs::write(d.0.join("a.jpg"), b"x").unwrap();
        std::fs::write(d.0.join("b.jpg"), b"x").unwrap();
        let decoded = Arc::new(AtomicUsize::new(0));
        let seen = decoded.clone();
        let worker = ExportWorker::spawn(
            vec![job("a.RAF"), job("b.RAF")],
            opts(&d, None),
            move |_: &Path| {
                seen.fetch_add(1, Ordering::Relaxed);
                Ok(source(flat(8, 4)))
            },
            Arc::new(AtomicBool::new(false)),
            CPU,
            1,
        );
        // The first collision arrives with nothing decoded.
        let first = next_collision(&worker);
        assert_eq!(
            decoded.load(Ordering::Relaxed),
            0,
            "nothing may be decoded while a collision is outstanding"
        );
        worker.decide(Decision::Resolve {
            collision: Collision::Overwrite,
            apply_to_all: false,
        });
        // ⚠ And the SECOND one must also arrive before any decode — which is the half that
        // distinguishes "resolved up front" from "resolved one photo ahead".
        let _second = next_collision(&worker);
        assert_eq!(
            decoded.load(Ordering::Relaxed),
            0,
            "the second collision must also precede every decode"
        );
        worker.decide(Decision::Resolve {
            collision: Collision::Overwrite,
            apply_to_all: false,
        });
        let report = drain_to_finished(&worker);
        assert_eq!(report.exported, 2);
    }
```

⚠ `job`, `next_collision` and `drain_to_finished` are helpers this module already has in some form —
**read the existing tests and reuse them** rather than adding near-duplicates; several already drain
events and answer collisions.

- [ ] **Step 2: run it to watch it fail** — expected: the second assertion fails, because today the
first photo is decoded before the second collision is raised.

- [ ] **Step 3: split `run` into `plan` and the loop**

```rust
/// One photo, with its destination already settled.
///
/// ⚠⚠ **Everything that needs a decision is resolved before this exists**, which is what lets the
/// export run several photos at once: phase 2 has no dialogs, no `claimed` set and no shared
/// mutable naming state — only work.
struct Planned {
    source: PathBuf,
    dest: PathBuf,
    edits: Edits,
    meta: export::FileMetadata,
}

/// Settle every filename and every collision, without decoding anything.
///
/// ⚠ A `stat` per photo — milliseconds for a hundred — which is why this can run up front at all.
/// Returns the photos to export, plus the skips and cancellation already recorded.
fn plan(
    jobs: Vec<ExportJob>,
    opts: &ExportOptions,
    template: &Template,
    events: &Sender<ExportEvent>,
    decisions: &Receiver<Decision>,
    cancel: &AtomicBool,
    report: &mut ExportReport,
) -> Vec<Planned> {
    // The body of today's loop down to (and including) the collision `match`, with the decode and
    // the export removed — `claimed.insert(dest)` moves here, since a destination is claimed by
    // being planned rather than by being written.
    todo!("lift verbatim from the existing loop")
}
```

⚠⚠ **`claimed.insert(dest)` moves from "after a successful write" to "when planned", and that is a
real behaviour change in the right direction.** Today a photo whose export *fails* leaves its name
unclaimed, so a later photo can take it; planned up front, the name is reserved either way. That is
the only reading that works concurrently, and it is also the one that cannot produce two photos
racing for one name.

- [ ] **Step 4: run the tests**

```bash
cargo test -p app --lib batch_export
```

Expected: **45 passed** — the existing 44 unchanged plus the new one. ⚠⚠ **Any pre-existing test
that moves is a bug in this restructure, not a test to update.**

- [ ] **Step 5: gate and commit**

```bash
git add -A && git commit -m "refactor(export): every filename and collision is settled before any decode"
```

---

### Task 3: phase 2 — N workers over a shared cursor

**Files:** `crates/app/src/batch_export.rs`, `crates/app/src/export_flow.rs`

- [ ] **Step 1: write the failing carrier test**

```rust
    /// ⚠⚠ **THE CARRIER, and it needs BOTH halves.** A width-4 run and a width-1 run produce the
    /// same files by design, so "the output is right" passes with the concurrency deleted; and
    /// "it ran concurrently" passes against a run that produced the wrong files. Only the pair
    /// says the feature is there and correct.
    ///
    /// ⚠⚠ **Concurrency is asserted as PEAK SIMULTANEOUS DECODES, never by wall clock** —
    /// `ThumbLoader`'s rule, and for its reason: a timing assertion is a CI flake, and this
    /// project has three on record. The count lives in the **injected decode fn**, so no
    /// production observable is needed at all.
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
            let live = Arc::new(AtomicUsize::new(0));
            let peak = Arc::new(AtomicUsize::new(0));
            let (l, p) = (live.clone(), peak.clone());
            let worker = ExportWorker::spawn(
                (0..8).map(|i| job(&format!("p{i}.RAF"))).collect(),
                opts(dir, None),
                move |_: &Path| {
                    let n = l.fetch_add(1, Ordering::Relaxed) + 1;
                    p.fetch_max(n, Ordering::Relaxed);
                    // ⚠ Long enough that the workers genuinely overlap, short enough that the
                    // test is not a sleep. The ASSERTION is on the peak, never on elapsed time.
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    l.fetch_sub(1, Ordering::Relaxed);
                    Ok(source(flat(8, 4)))
                },
                Arc::new(AtomicBool::new(false)),
                CPU,
                width,
            );
            let report = drain_to_finished(&worker);
            assert_eq!(report.exported, 8, "width {width} exported {}", report.exported);
            peak.load(Ordering::Relaxed)
        };
        let one = TempDir::new("wide_one");
        let four = TempDir::new("wide_four");
        assert_eq!(run_at(1, &one), 1, "width 1 must never overlap");
        assert!(run_at(4, &four) > 1, "width 4 must overlap");
        assert_eq!(names(&one), names(&four), "the two widths must write the same files");
    }
```

- [ ] **Step 2: run it to watch it fail** — expected: `spawn` takes no width.

- [ ] **Step 3: make the decode fn shareable and take the width**

`spawn` and `run` gain `width: usize`, and the decode bound gains **`+ Sync`**:

```rust
        decode: impl Fn(&Path) -> Result<ExportSource, String> + Send + Sync + 'static,
```

⚠ The production closure in `export_flow.rs` captures a `Demosaic` (`Copy`) and an
`Option<PathBuf>` it clones per call, so it already **is** `Sync` — this narrows the bound rather
than forcing a change. ⚠ `export_flow.rs` passes `self.settings.export_width()`.

- [ ] **Step 4: write the parallel phase**

```rust
    let total = planned.len();
    if events.send(ExportEvent::Started { total }).is_err() {
        return;
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    let finished = std::sync::atomic::AtomicUsize::new(0);
    let shared = std::sync::Mutex::new(report);
    std::thread::scope(|s| {
        for _ in 0..width.max(1) {
            // ⚠ A clone per worker rather than sharing one `Sender`: `mpsc::Sender` is `Clone` by
            // design for exactly this, and it needs no assumption about its `Sync`ness.
            let events = events.clone();
            s.spawn(|| {
                let events = events;
                loop {
                    if cancel.load(Ordering::Relaxed) {
                        shared.lock().unwrap().cancelled = true;
                        return;
                    }
                    // ⚠ A shared cursor rather than static chunks, `ThumbLoader`'s rule: a
                    // selection mixes RAW and JPEG whose decode costs differ by an order of
                    // magnitude, so chunking would leave workers idle on the tail.
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some(p) = planned.get(i) else { return };

                    let decoded = decode(&p.source);

                    // ⚠ Checked *again* after the decode, which is the slow part — "abandon,
                    // don't abort", because libraw's `unpack` cannot be interrupted. The decode
                    // finishes and its result is discarded rather than written.
                    if cancel.load(Ordering::Relaxed) {
                        shared.lock().unwrap().cancelled = true;
                        return;
                    }

                    let outcome =
                        decoded.and_then(|src| export_one(src, &p.edits, &p.dest, opts, render, &p.meta));
                    {
                        let mut r = shared.lock().unwrap();
                        match outcome {
                            Ok(()) => r.exported += 1,
                            Err(e) => r.failed.push((p.source.clone(), e)),
                        }
                    }
                    // ⚠⚠ Sent on COMPLETION, and `done` counts completions rather than positions.
                    // With four in flight the fourth photo can land before the second, so a
                    // position would jump about; a count only ever rises. It is out of order by
                    // name, and honestly so.
                    let done = finished.fetch_add(1, Ordering::Relaxed) + 1;
                    let _ = events.send(ExportEvent::Item {
                        done,
                        total,
                        name: file_label(&p.source),
                    });
                }
            });
        }
    });
    let report = shared.into_inner().unwrap();
    let _ = events.send(ExportEvent::Finished(report));
```

⚠ `std::thread::scope` rather than spawning detached threads: the workers borrow `planned`, `opts`
and `decode`, and the scope is what makes that sound without an `Arc` around each.

⚠ **`report.failed` is now in completion order, which is nondeterministic.** Any test comparing it
must sort or compare as a set; say so where one does.

- [ ] **Step 5: run the tests**

```bash
cargo test -p app --lib batch_export
```

⚠⚠ Expect some **pre-existing** tests to need the new `spawn` argument — that is the compiler doing
its job. ⚠ But if a pre-existing test's *assertions* have to change, stop and read why: the only
legitimate ones are `failed`'s ordering and the meaning of `Item.done`.

- [ ] **Step 6: sabotages**

Each restoring from a `cp` copy named per file — **never `git checkout`**, which has destroyed
uncommitted work in this repo once already. And confirm a `test result:` line appears before
believing anything: **a sabotage that fails to compile reads exactly like one that passes.**

| sabotage | expected |
|---|---|
| `for _ in 0..1` instead of `0..width` | the carrier fails on "width 4 must overlap", and **only** that |
| drop the post-decode cancel check | the cancellation test fails |
| `Item.done` sent before the work rather than after | the progress test fails |
| a worker takes `i` without `fetch_add` (always 0) | the carrier fails on the exported count |
| `plan` claims nothing (`claimed` never inserted) | the two-sources-one-stem collision test fails |

- [ ] **Step 7: gate and commit**

```bash
git add -A && git commit -m "feat(export): a batch decodes several photos at once, bounded by the setting"
```

---

### Task 4: measure the real thing

⚠⚠ **This task is the reason the spec projects no end-to-end ratio.** Composing component
measurements has missed in this crate three times; the sweep priced **decodes alone**, and a real
export also renders, encodes and writes, with the renders now contending for the same rayon pool.

- [ ] **Step 1: measure a real batch**

Export the same 8 RAWs from the GUI at width 1 and at width 4, with `RUSTROOM_DEBUG_RENDER=1`, and
record three runs of each plus the machine's load. ⚠ **Report a band, never a figure** — this file
has had to retract a headline for quoting a point reading.

- [ ] **Step 2: watch the memory**

⚠ Peak RSS at width 4 on a 26 Mpx camera should be roughly **1.2 GB of decode buffers plus the
renders**. If it is markedly more, something is holding a decoded buffer longer than its own photo —
which is the one failure mode this design can have, and it will not show as a wrong picture.

- [ ] **Step 3: write the result into `CLAUDE.md`**

A band, the load, and the honest statement that the end-to-end figure is **lower than the 3.14×** the
decode sweep gave, because the renders contend.

- [ ] **Step 4: launch the GUI**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo run --release -p app
```

What to look at: **export eight photos and watch the progress line** — it should move in bursts of
four and name photos out of order, which is the feature rather than a defect. ⚠ Then **export into a
folder that already has the files**, which is where the up-front prompts now appear — all of them
before anything starts. ⚠ Then **cancel mid-run**: the photos in flight must not appear on disk.
⚠⚠ And **the exported files must be identical to what width 1 produces** — that is the one thing no
amount of speed makes up for.
