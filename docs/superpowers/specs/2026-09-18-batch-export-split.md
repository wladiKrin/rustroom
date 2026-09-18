# Splitting `batch_export.rs`

**Date:** 2026-09-18
**Status:** approved

## What this is

`crates/app/src/batch_export.rs` is **2,335 lines** — the largest file in the crate and well past
this project's own ~600-line marker. It becomes a directory module of five files.

⚠⚠ **A PURE MOVE**, on `prefix_cache.rs`'s precedent — *"moved out of `render_engine.rs` verbatim
on 2026-09-14, as a pure move with the test count unchanged and no golden moved."* Same posture
here: **2547 tests before and after**, not one `⚠` comment reworded, no logic touched. Any
behaviour change belongs in its own commit, and finding one mid-move is a reason to stop and make
it separately, not to fold it in.

## The shape

| file | contents |
|---|---|
| `mod.rs` | module docs, `mod`/`pub use` re-exports, and `#[cfg(test)] mod fixtures` |
| `options.rs` | `ExportJob`, `ExportOptions`, `Delivery`, `PostExportAction`, `banding_warning`, `can_export`, `DEFAULT_FILENAME_TEMPLATE` |
| `destination.rs` | `Collision`, `Decision`, `dest_for`, `dest_named`, `destination_ready`, `file_label` |
| `render.rs` | `scale_for`, `file_metadata`, `accelerated_steps*`, `export_one`, `ExportSource`, `ExportRender` |
| `worker.rs` | `ExportEvent`, `ExportReport`, `ExportWorker`, `Planned`, `plan`, `run` |

Each test moves to the module holding its **subject**.

## The shared fixtures

Measured rather than guessed: `opts` has **24** uses spanning the whole test module, `source` 14 and
`flat` 11. `TempDir` is shared likewise. Those four go in `#[cfg(test)] mod fixtures`; every other
helper (`jpeg_size`, `edged`, `lens_fixture`, `lens_on`, `exported_with`, `pixels`, `jobs`,
`ok_decode`, `run_export`, `jobs_with`, `shot`, `templated`, `finished`) is local to one subject and
travels with it.

⚠⚠ **`TempDir` is one of the seven temp-dir isolation sites this repo records.** It derives its path
from a caller-supplied tag, so **two tests sharing a tag collide**. Moving it must not change a
single tag, and **two gates must never be run concurrently**.

## Known and accepted

⚠ **`worker.rs` lands around 1,000 lines**, over the ~600 marker: ~190 lines of code and ~810 of
tests. Splitting a worker's tests away from the worker to hit a number would be worse than the
number. **Stated rather than quietly shipped.**

## Done when

- Every file under ~1,000 lines, and only `worker.rs` near it.
- `cargo test` reports **exactly 2547** — a move that changes the count has dropped or duplicated a
  test, which is the failure this whole posture exists to catch.
- FMT=0, CLIPPY=0, TEST=0, each read from its own command. 74 goldens unmoved.
- `git show --stat` shows no file outside `crates/app/src/batch_export*`.
