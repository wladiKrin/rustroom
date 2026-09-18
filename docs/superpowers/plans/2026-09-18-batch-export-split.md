# Plan — splitting `batch_export.rs`

Spec: `docs/superpowers/specs/2026-09-18-batch-export-split.md`
Branch: `batch-export-split`

## Task 1 — record the baseline

`cargo test --release --workspace -q` → **2547**. Write the number down before touching anything;
it is the only check that catches a silently dropped test.

## Task 2 — partition mechanically, not by hand

Parse the file into top-level items by scanning column-0 item starts, and assign each to a target
module by name. ⚠ The items are **not contiguous by subject** — `ExportEvent`/`ExportReport` sit at
208/225 but belong to `worker`, while `ExportSource`/`ExportRender` sit at 548/569 and belong to
`render`. Line-range slicing would silently mis-assign; a name-keyed map cannot.

## Task 3 — the same for the tests

Assign each `#[test]` and each helper to a module. The four shared fixtures go to
`#[cfg(test)] mod fixtures`. ⚠ Do not reformat, re-indent or reword anything carried across.

## Task 4 — imports

Each submodule declares its own `use` lines; `mod.rs` re-exports with `pub use <m>::*` so existing
callers (`ExportSource`, `ExportOptions`, `dest_named`, `DEFAULT_FILENAME_TEMPLATE`,
`ExportRender`) keep working unchanged.

## Task 5 — verify it was a move

- `cargo test` reports **exactly 2547**.
- FMT / CLIPPY / TEST, each exit code from its own command, **before** committing.
- ⚠ Never two gates at once — `TempDir` tags collide.
- `git show --stat` touches nothing outside `crates/app/src/batch_export*`.
