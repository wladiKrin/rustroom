# Plan — `ProfileRendering` becomes a required argument

Spec: `docs/superpowers/specs/2026-09-18-profile-rendering-required.md`
Branch: `profile-rendering-required`

## Task 1 — `ProfileRendering::colorimetric()`

`crates/pipeline/src/lib.rs`, beside the struct. Its doc says what it *is*, not what it lacks.

## Task 2 — the six builders take the argument

`build_pipeline`, `_with`, `_to`, `_spec`, `build_pipeline_split`, `build_pipeline_split_in_region`.
Each stops constructing a default and passes the caller's through.

## Task 3 — update ~146 call sites

⚠ Mechanically, by builder, smallest first — `_with` (2), `_to` (3), `_spec` (8), then the splits,
then `build_pipeline` (65). ⚠⚠ **Every site that today gets the default must receive
`colorimetric()` and nothing else.** A site given a real rendering by mistake moves a golden, which
is what the gate below is for.

⚠ `render_once`'s site gets a comment saying why a **reference** stays colorimetric.

## Task 4 — the gate is the goldens

- **2557 tests, 74 goldens, unchanged.** A moved golden means a call site changed behaviour.
- FMT / CLIPPY / TEST, each exit code from its own command, before committing.
- ⚠ Nothing to confirm by use: a behaviour-preserving change has nothing to look at. Say so rather
  than asking for a look.
