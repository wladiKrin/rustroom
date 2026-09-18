# `ProfileRendering` becomes a required argument

**Date:** 2026-09-18
**Status:** approved
**Closes the class behind:** `docs/BACKLOG.md` → *"The whole-pipeline builders default
`ProfileRendering` SILENTLY"*

## What this is

Six builders hand `ProfileRendering::default()` down without being asked:

`build_pipeline`, `build_pipeline_with`, `build_pipeline_to`, `build_pipeline_spec`,
`build_pipeline_split`, `build_pipeline_split_in_region`.

A call site therefore renders without the camera profile and without the user's `.cube`, and
**nothing says so** — no compile error, no warning, no failing test. The default is
indistinguishable from a deliberate choice. That cost three defects in one day: the export lost the
curve, the baseline **and** the look; a thumbnail lost the look.

Each builder gains a `profile: ProfileRendering<'_>` parameter. **~146 call sites**, wide and
shallow.

## `ProfileRendering::colorimetric()`

A named constructor for "the primaries and the transfer curve, nothing else" — what the goldens, the
CLI and most tests genuinely want.

⚠⚠ **This is what makes the parameter worth 146 edits.** `ProfileRendering::default()` at a call
site reads as *"nothing special here"*; `colorimetric()` reads as a statement a reviewer can
disagree with. A required argument creates a decision point; a **named** argument makes the wrong
decision visible.

⚠ `Default` is kept — four sites use `..ProfileRendering::default()` struct-update — but the
builders stop applying it behind the caller's back.

## ⚠⚠ What this does NOT do, stated before the work rather than after

**It is not a proof.** A developer adding a call site will type whatever compiles, and the compiler
cannot know which rendering is right. What this buys is a decision point and a name that looks wrong
in review. A hard guarantee would need something like a test asserting every user-facing render path
passes a *resolved* rendering, which is a different and harder piece of work. **Do not record this
as "the class is closed"** — record it as "the silent default is gone".

## A finding from the survey

`render_engine::render_once` — the **reference** the cached path is tested against — also passes
`ProfileRendering::default()`. That is defensible (a reference should not move with a profile) but it
is currently implicit. After this change it is explicit, and the reason belongs in a comment there.

## Testing

⚠⚠ **This must be behaviour-preserving, and the test count is the observable**: every call site keeps
the rendering it had, so **2557 tests and 74 goldens must be unchanged**. A golden that moves means a
call site was given a rendering it did not have before.

⚠ No new tests are needed for a mechanical change, and adding one that asserts "the argument exists"
would be a compile-time fact dressed as a test. The guard here is the goldens.

## Done when

- The six builders take the argument; no builder applies `ProfileRendering::default()` implicitly.
- **2557 tests, 74 goldens, unchanged.**
- FMT=0, CLIPPY=0, TEST=0, each from its own command.
