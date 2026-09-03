# Working several agents at once

Phases 5–7 were planned to be worked concurrently. This file is the protocol for doing that without
producing a merge that nobody can review.

**Read this before dispatching anything.** The roadmap says *what* to build; this says *who may touch
which file, when*, and *where new code belongs*.

---

## 1. The partition is by collision surface, not by feature

The instinct is to give each agent a feature. That is wrong here, because features in this codebase
do not map onto files — several unrelated features all have to edit the same six lines of
`edit-model/src/lib.rs`, and two agents doing that concurrently produce a conflict in a file whose
whole purpose is to be exhaustive.

So work is partitioned by **which shared surface it must write to**. These are the contended ones:

| Surface | File(s) | Wanted by |
| --- | --- | --- |
| `Edits` field list **+ its 5 exhaustive sites** | `edit-model/src/lib.rs`, `merge_from`'s destructure, `EditGroups`, `pipeline::all_edits`, `edit_model::all_nonneutral`, `app::edit_clipboard`'s payload | M5.1b, M5.2a, M5.3a |
| Pipeline stage order, `PrefixStepKey`, `PrefixKey` | `pipeline/src/lib.rs` | M5.3a, M7.2, M7.3b |
| Develop sliders | `develop/panel/adjustments.rs` | M5.1, M5.2, M5.6, M7.6 |
| **The canvas** | `develop/canvas.rs` | M5.3b, M5.5a, M5.5b, M6.5d, M7.1, M7.4 |
| Export options + dialog | `batch_export.rs`, `modals/export.rs` | all of M6.1, M6.2, M6.4 |
| **Catalog schema version number** | `catalog/src/db.rs` (`SCHEMA_VERSION`) | M6.3 only |
| `RustroomApp` + its exhaustive `test_app.rs` literal | `main.rs`, `test_app.rs` | any new tool or modal |
| `Command` enum, menu model, `DialogSize` array | `commands.rs`, `menu.rs`, `theme.rs` | any new dialog or shortcut |
| **Goldens** | `pipeline/tests/golden.rs` + 49 PNGs | M7.3 *moves all of them*; M5.3a / M7.1 / M7.2 *add* |

⚠ Several of these are exhaustive **on purpose** — the `test_app.rs` literal and `merge_from`'s
destructure exist so that a new field is a *compile error* until someone decides what it means. That
is excellent for correctness and hostile to concurrent edits. Do not weaken them to make merging
easier; schedule around them instead.

---

## 2. Five rules

**1. One writer per surface per wave.** Two agents may not both add an `Edits` field in the same
wave. The wave table below assigns each surface an owner.

**2. ⚠⚠ Schema versions are reserved in writing, not discovered.** `SCHEMA_VERSION` is **5** today.
Two agents both writing a `if from < 6` migration is not a merge conflict — it is **data corruption
on a real user's catalog**, and the second migration to run finds its objects already present. Only
one track per wave may touch `catalog/src/db.rs`, and the reservation goes in the dispatch prompt.

**3. Append, never insert.** New `Edits` fields go at the **end** of the struct and the **end** of
every exhaustive fixture. Git merges appends cleanly and conflicts on mid-struct insertions almost
every time. The same applies to `Command`, `EDIT_GROUP_NAMES` and the `DialogSize` array.

**4. ⚠ Goldens are a single-writer resource.** M7.3a moves all 49 reference images. Anything else
that adds a golden must land strictly **before or after** it, never during. A wave containing M7.3
gets no other golden-touching work.

**5. Headless first, per the M3.1 precedent.** Every canvas feature splits in two: *model, stage and
tests with no GUI in existence*, then *canvas UI*. The headless half lives in its own stage file and
is nearly collision-free — parallelise those aggressively. The canvas half contends — serialise it.
This is not merely a merge tactic: it is why M3.1a could prove the entire masking architecture
against goldens and a real `.RAF` before any canvas code existed, so a canvas bug could never be
confused with a model bug.

**A sixth, situational: land a seam commit before a fan-out.** Before M6.1's five sub-features, one
agent adds all five `ExportOptions` fields inert plus the dialog skeleton. After that commit, a–e
are genuinely independent.

---

## 3. How agents are actually dispatched

⚠ **Parallel branches are possible** — this was initially assumed otherwise. The `Agent` tool takes
`isolation: "worktree"`, which gives each agent its own git worktree on its own branch. They work
simultaneously and are merged here.

**⚠⚠ The setting that makes waves chain correctly.** Worktrees branch from `origin/<default-branch>`
by default. Since **this project never pushes from a session** — the user pushes himself — a Wave 2
agent would branch from `origin/main` and silently **miss everything Wave 1 merged into local
`main`**. `.claude/settings.json` therefore sets:

```json
{ "worktree": { "baseRef": "head" } }
```

so each worktree branches from local `HEAD`. **Verify this before dispatching a wave that depends on
the previous one.**

**Every dispatch prompt must carry:**
- the build environment, or the agent cannot compile at all:
  `export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"`
- **which surfaces it owns this wave, and which it must not touch**
- the placement rules in §5 below
- the standing conventions: no `Co-Authored-By` trailer, **never push**, `cargo fmt` +
  `clippy -D warnings` + `test --all` green before reporting done
- ⚠ the sabotage requirement: after writing a test, break the thing it names and watch it fail.
  This project has recorded **sixteen** instances of a test that could not detect the bug it was
  named for.

**Merge order within a wave: the owner of the most contended surface merges first.** Everyone else
rebases onto it. Merging the isolated tracks first only means the contended one rebases against more.

**⚠⚠ AGENTS SHARE `/tmp` AND THE SCRATCHPAD, AND THIS PRODUCES FALSE READINGS THAT LOOK LIKE
CATASTROPHES.** On 2026-09-01 two concurrent agents both wrote `/tmp/t.log`; one read the other's
run and saw **901 passed** where its own was 1501, which reads as a massive regression. The
orchestrator hit the same thing from the other side — `/tmp/t.log` on `main` had been overwritten by
an agent, giving **1495** where the true count was **1475**, and several minutes went into explaining
a number that was never real. A generic `sabotage.sh` in the shared scratchpad was also silently
replaced mid-run by another agent's version that `cd`s elsewhere.

**Every dispatch prompt must require unique filenames** — `/tmp/<something>-<agent-suffix>.log` —
and the orchestrator must do the same for its own runs. ⚠ This is the same family as the
`${PIPESTATUS[0]}` trap: **a number that is real, but not a measurement of what you think you
measured.** Neither shows up as an error.

⚠ A second consequence, which cost a wrong conclusion here: **`git diff main..HEAD` on an agent
branch is misleading once another branch has merged**, because it shows the *other* branch's changes
as differences. Use `git merge-base main HEAD` to check what an agent actually touched.

**Between waves:** merge everything, run the full suite once on the merged result, and *then* dispatch
the next wave. ⚠ A green run in each worktree is not evidence about the merge — three of this
project's recorded bugs were interactions between separately-correct changes.

---

## 4. The wave plan

| | A | B | C | D |
| --- | --- | --- | --- | --- |
| **W0** ✅ | **Split `canvas.rs`** — done 2026-09-01 | **Split `main.rs`** — done 2026-09-01 | — | — |
| **W1** ✅ | M5.1a tone curve + monotone spline | M6.1 seam + filename templating | M7.3a measured → **M7.3 shipped as a Settings choice** | M5.5a crop guides → now **View-wide** |
| **W2** ✅ | M5.2 WB Kelvin ✅ | M6.1b/c/d ✅ | M7.1a luminance mask ✅ | M6.5a/b compare + survey ✅ |
| **W3** ✅ | M5.3a spot headless ✅ | M6.3a/b XMP ✅ | M5.4a auto tone ✅ *(targets await the user's eye)* | M7.6 ×3 ✅ |
| **W4** 🟡 | M5.3b spot canvas ✅ *(inline, 2026-09-03)* | M5.5b straighten line | M7.4 Hough verticals | M6.5d colour readout |

**W4 status (2026-09-03): A is done, B/C/D are not started, and the wave was never dispatched as a
wave.** M5.3b ran **inline** because it went through five rounds of use-and-fix on the canvas, each
reported by the user between commits — a shape a parallel worktree cannot serve. **M5.4b** (display
rendering) also landed inline on 2026-09-03, outside any wave, because it was blocking three other
milestones. ⚠ The remaining three W4 tracks are still parallelisable on the original partition —
M5.5b and M7.4 both touch `crop_overlay.rs`/`guides` and would need the surface split before
dispatch; M6.5d is a **canvas hover** (not Library-side, as an earlier draft of this line said) and
touches `canvas/mod.rs`, which B's dispatch line also does. ⚠ `mask_overlay.rs` is now 455 lines
and `spot_overlay.rs` 462 — the "next file to watch" note below held, since M5.3b got its own
module rather than growing `mask_overlay.rs`; `canvas/mod.rs` is 992 including its tests (474 of
production code).

### W4 — to be run by a DIFFERENT agent (user's call, 2026-09-03)

**Read this whole file first, then §5, then `CLAUDE.md`'s M5.3b, guided-perspective and M5.2
sections.** Three tracks, none yet specced. ⚠⚠ **W4-0 comes first and is the orchestrator's job,
inline, with the user:** brainstorm → spec for each track, settling the decisions listed under it.
§6 is unambiguous — an agent given an open design choice picks one silently and the choice ships.
Then verify `.claude/settings.json` still has `"baseRef": "head"`, dispatch all three with the §3
prompt contents, and merge **D → B → C**.

| Track | Owns | Must not touch |
| --- | --- | --- |
| **B** M5.5b straighten line | new lib `app/src/straighten_line.rs` (pure: two frame points + aspect → angle), new bin `develop/canvas/straighten_overlay.rs`, **one dispatch line** in `canvas/mod.rs`'s `Tool::Crop` arm, `crop_overlay.rs` only if the gesture must be arbitrated there | `guide_overlay.rs`, `panel/geometry.rs`, `Session.guides`/`show_guides`, `Edits`, `pipeline`, goldens, D's hover code |
| **C** M7.4 Hough verticals | new `pipeline/src/perspective/detect.rs` (the analysis — `auto_tone.rs` is the precedent for an analysis pass living in `pipeline`), an `Auto` button in `panel/geometry.rs` under Guide Mode, one app method writing `Session.guides` (in `interaction.rs` beside `select_tool`, or a new lib glue module) | `canvas/**` entirely — the inferred guides go into `Session.guides` and the existing overlay draws and edits them for free — `Edits`, `crop_tool.rs`, goldens, `visualize.rs` (read it, do not change it) |
| **D** M6.5d colour readout | new lib `app/src/color_readout.rs` (pure: `[f32;3]` → the strings shown), the **hover read at the end of `canvas/mod.rs::show`**, the status cluster in `shell.rs`, and **extracting `spot_overlay::sampler_for`** into a shared bin helper both it and the retouch tool call | `crop_overlay.rs`, `guide_overlay.rs`, `panel/**`, `Session` fields other than what the hover needs, `Edits`, `pipeline` |

**Merge order D → B → C**: `canvas/mod.rs` is the contended surface (D's hover block, B's one
dispatch line), so D lands first and B rebases one line; C touches nothing the others do.

**Decisions to settle in W4-0, per track** — none of these may be left to the agent:

- **B.** (1) The gesture: Lightroom's is ⌘-drag inside the crop tool; here the crop rect already
  owns every drag in `Tool::Crop`, so it is a modifier-drag (the retouch tool's ⌥-drag precedent)
  or a small armed mode — pick one. (2) Whether the crop auto-shrinks to exclude the black corners
  after the angle lands (Lightroom's "constrain to image"), or is left to the user. (3) Whether the
  drawn line is shown mapped through the resulting angle after release (it lands level by
  construction, which is the only feedback that says the gesture worked). ⚠ Facts the agent needs:
  the angle is `atan2` of the line **in screen space, aspect-corrected** — normalised space is
  anisotropic, the trap `LinearGradient::coverage`, `mask_tool::hit_test` and the guide overlay each
  had to handle; the canvas `resp` is `Sense::drag()` and never reports `clicked()`; crop mode
  **pins the fit rung**, so the overlay is placed by fit-mode maths; `guides::solve` returns roll as
  its third value but a single line needs no solver — the roadmap's "nearly free" note refers to
  the *write path* into Straighten, which guided perspective already exercises.
- **C.** (1) What the detector runs on: the fit-rung **rendered frame** (frame-normalised, which is
  what `Guide` stores) versus the pre-geometry decode buffer — and if the frame, whether with the
  current perspective stripped, since lines already corrected have no vanishing point to find.
  (2) Verticals only (the roadmap's wording) or both families. (3) Whether the button *proposes*
  guides for the user to accept, or solves immediately with the guides left visible and editable —
  the roadmap's "Done when" says one button corrects **and** the inferred guides are visible and
  editable, which is the second. (4) The Hough window: near-vertical only (±20°) keeps it cheap and
  specific; a full 180° Hough finds every horizon and edge of a table. ⚠ Reuse, not rebuild:
  `app::visualize::edge_view` is already a luminance edge map (⚠ in the **app lib**, so `pipeline`
  cannot call it — either move it down to `pipeline::spatial` first, in C's own first commit, or
  recompute `|luma − gaussian_blur|` there; do not copy it); `pipeline::perspective::guides::solve`
  is the pure solver and needs exactly two guides per family; `Guide { from, to }` in frame units.
  ⚠ This is the one track whose "Done when" is a judgement about a photograph — the agent produces
  the before/after on a real off-axis building, **the user signs it off** (§6).
- **D.** (1) What is read: the **rendered** value (what you see, every edit applied — Lightroom's
  choice) versus the decode buffer (the eyedropper's choice, because it must not chase its own
  correction — a readout has no such loop). (2) Which numbers: sRGB 0–255, Lightroom's 0–100 %, Lab
  or Oklab — and whether it is one pixel or a 5×5 mean under the cursor. (3) Where it shows: the
  right-aligned status cluster `shell.rs` owns is the obvious home; the Develop toolbar must not
  grow a second right-aligned region (the recorded two-regions-in-one-row bug). ⚠ Reuse:
  `spot_overlay::sampler_for` composes `Homography::source_of` → `Corrector::source` → buffer read
  and is the sampler for the *buffer* choice; for the *rendered* choice the texture's RGBA is on
  hand in `canvas/mod.rs` and needs no inverse mapping at all. The eyedropper's `inverse_point` call
  at `canvas/mod.rs:441` is the screen→frame mapping either way.

**Standing for all three:** the §3 prompt contents; unique `/tmp/<x>-<track>.log` names; the
sabotage rule with `ran = passed + failed ≥ 1`; **53 goldens byte-identical** — no track here has
any business moving one; ⚠ **every one of these lands in canvas or panel code, which this project's
record says is where every defect is found by use and none by a test** — so each track's report
must say what was *run in the GUI*, not only what passed.

**W2 is closed.** ⚠ M5.2's Task 3 needed one correction — it threaded the source's *camera
matrix* into the pipeline, but the stage runs on linear ProPhoto, so `RenderSpec` carries a
baseline **illuminant** instead. See `CLAUDE.md`'s M5.2 section.

**W3 — specced 2026-09-02, all four design forks settled by the user before dispatch.** Specs:
`2026-09-02-spot-removal-headless-design.md` (A), `2026-09-02-xmp-interchange-design.md` (B),
`2026-09-02-auto-tone-design.md` (C), `2026-09-02-tool-gaps-design.md` (D).

| Track | Owns | Must not touch |
| --- | --- | --- |
| **A** M5.3a | `edit-model` (+ `Spot`, `Retouch` group, all 5 exhaustive sites), `pipeline/lib.rs` stage order + `PrefixStepKey`, new `stages/spot_removal.rs`, `golden.rs` **+2 cases appended**, 2 new PNGs | `develop/`, `Settings`, `catalog`, `local_contrast.rs`, `adjustments.rs` |
| **B** M6.3a/b | new `crates/xmp/`, `catalog/db.rs` (**schema v6**, both drop lists), `catalog_ops.rs`, the folder-open path, `settings.rs` (+`write_xmp`) | `edit-model`, `pipeline`, `develop/`, `export/` |
| **C** M5.4a | new `pipeline/auto_tone.rs`, the Auto button in `adjustments.rs` + `panel/mod.rs`, a probe example | `Edits` (**no new field**), stage order, goldens, `Settings`, canvas, D's files |
| **D** M7.6 ×3 | `local_contrast.rs`, `color_grading.rs` (visibility only), `color_wheel.rs`, PNGs `clarity_plus/minus` + `texture_minus` **re-authored**, one hover string in `adjustments.rs` | `Edits`, `pipeline/lib.rs`, `golden.rs`, canvas, `dehaze.rs` |

**Merge order: D → A → C → B.** D moves goldens and A adds them, so D lands first and A rebases
onto its PNGs; C and B touch nothing the others do. The one shared file is `adjustments.rs` — C
adds a button, D changes one hover string — which rebases trivially.

⚠ **The four decisions taken (user's call, 2026-09-02), so no agent re-opens them:** A ships
**Clone + Heal** (heal = mean-matched clone; content-aware fill stays 5.3c); B's sidecar is
**`stem.xmp`, RAW only** (Lightroom's name; JPEG/PNG/TIFF marks stay catalog-only) and is written
**automatically** with a `Settings` toggle, **merging** into any existing sidecar rather than
replacing it; C is **5.4a only** — exposure/whites/blacks from percentiles, a button, not applied
on open, the user signs off the photographs; D takes the **three items that need no `Edits`
change** and defers grading-in-masks and auto-mask to W4 and dehaze's achromatic guard to its own
brainstorm.

**W3 landed 2026-09-02: 1627 → 1723 tests, 51 → 53 goldens, merged D → A → C → B with one
conflict.** ⚠⚠ **All four agents were killed mid-flight by a session rate limit and every one
resumed with its context intact** — `SendMessage` to the agent id, not a fresh spawn. Nothing was
lost; each had already committed its plan. **That is the recovery path for a rate-limited wave**,
and it is far cheaper than re-dispatching.

⚠ **The one conflict was the one predicted**: C extracted the Tone section body into `fn tone` while
D added a hover string inside it. Resolved by hand in favour of C's extraction with D's string
carried into the new function — a merge no agent could have done, which is why §6 says the merge is
one person's job.

⚠⚠ **Two ownership exceptions, both correct, both flagged by the agent rather than hidden:**
**A edited `adjustments.rs`** (barred to it) because two test assertions enumerate the edit groups
the panel does *not* show, and a ninth group is exactly what they exist to flag — `cargo test --all`
cannot be green without it, so the instructions genuinely conflicted and it chose the gate.
**D exposed `range_weights` as `ColorGradingStage::range_weights`** rather than the free function
the spec named, because reaching the free function needs `pub mod stages` in `pipeline/src/lib.rs`
— **A's file**. Verified: `mod stages` really is private. Both are the behaviour the dispatch brief
asks for — report the conflict, do not silently pick.

⚠ **B used `git merge-base main HEAD`, not `git diff main`, to check which goldens it had touched** —
a plain diff shows D's re-authored PNGs and reads as a violation. §3 records this trap; B is the
first track to hit it and get it right unprompted.

**Why W0 existed.** `canvas.rs` was 1,890 lines and wanted by **six** milestones; `main.rs` was 4,375
and touched by any new tool or modal. They were the two biggest serialisation points in the plan, and
Wave 4 is entirely canvas work that would otherwise have queued behind itself.

### ⚠⚠ What W0 proved, and the recipe to reuse

**Two agents refactored 6,265 lines concurrently and the merge had ZERO conflicts.** That was not
luck; it came from one constraint given to both, and it generalises:

> **Type *definitions* stay put; only `impl` blocks and free functions move.**

An inherent impl only has to be in the same **crate**, not the same module, so ~90 methods left
`main.rs` with `self` staying `self` and no call-site churn — while `develop/canvas.rs`'s
`use crate::RustroomApp` kept resolving, which is what let the other agent work in the same tree at
the same time. ⚠ **When two tracks must touch related code, look for the constraint that makes their
files disjoint rather than assuming they must be serialised.**

Result: `canvas.rs` → 7 modules (largest 439); `main.rs` **4,375 → 681**, with 13 new modules.
1322 tests before and after, 49 goldens untouched, `fmt`/`clippy -D warnings`/`test --all` green on
the **merged** result, not merely in each worktree.

**Three findings worth carrying into the next wave:**

1. **⚠⚠ A SABOTAGE MUST BE CONFIRMED TO HAVE *RUN*, NOT MERELY COMPILED.** The old rule here was
   "check that a `test result:` line appears, because a sabotage that fails to compile looks like
   one that passes". **That is not enough** — a filter matching no test also prints that line, with
   `0 passed; 0 failed; N filtered out`, and reads as success. Assert **`ran = passed + failed ≥ 1`**.
   Bitten **three times** on 2026-09-01: a mistyped module path, and twice from the target —
   `name_edit` and `canvas` are **bin** modules, so `cargo test -p app --lib` reports zero tests
   having run and exits 0. ⚠ A fourth variant: a sabotage whose *text* no longer matches because
   `cargo fmt` reflowed the code — the replacement silently applies to nothing. **Confirm the file
   actually changed**, not just that the command succeeded. ⚠ When in doubt, `cargo test --all -- --list | grep
   <name>` tells you the real path and target.
2. **⚠⚠ `${PIPESTATUS[0]}` IS BASH; THIS SHELL IS ZSH, AND IT EXPANDS TO NOTHING — so a check that
   prints `EXIT: ` reads as a pass.** This is a *third* variant of a trap already recorded twice in
   `CLAUDE.md` (a `clippy | grep` pipeline reporting grep's status; a sabotage harness grepping for
   the wrong line shape). **Redirect to a file and test `$?` directly**, never through a pipeline:
   `cargo clippy ... > /tmp/c.log 2>&1; echo "EXIT=$?"`. In zsh the array is `$pipestatus`.
3. **A refactor that changes the test count moved behaviour.** "Unchanged at 1322" is the assertion,
   not "still green".
4. **Diff the `⚠` comments, not just the code.** This project encodes its findings in them; a move
   can drop one silently and no test will notice. `grep "⚠" | sort` on both sides, then `comm`.
   The canvas split lost none of its 113.

⚠ **`mask_overlay.rs` (439) is the next file to watch** — both **M7.1a** (luminance mask, W2) and
**M5.3b** (spot canvas, W4) land in it. It has room now and will not after two more features.

⚠ **Two deliberate compromises made by the `main.rs` split**, worth knowing before building on it:
every moved method is now `pub(crate)` (a private `fn` in an inherent impl is visible only in its own
module and descendants, so moving it out of the root would have broken every caller), and each new
module opens with `use crate::*;` rather than an explicit import list — the glob is what makes the
move *provably* behaviour-preserving, since an explicit list can silently change which name resolves.
Tightening those is optional cleanup, not a bug.

**Why M7.3 is in W1 and alone with the goldens.** It moves all 49 references. Nothing else in W1
touches a golden.

**Why M5.2 and M5.3a are in different waves.** Both must write the `Edits` surface. They are the
clearest example of the partition doing its job.

---

## 5. ⚠⚠ Where new code goes — read this before writing any

**Standing instruction, 2026-09-01: think about placement before writing, do not dump code into
`main.rs` and leave it for a later split.** Every large file in this repo got that way one
reasonable-seeming addition at a time. An agent that adds 300 lines to `main.rs` has not finished the
task; it has moved work onto whoever splits it next.

**The crate boundary is the first question, and it is a hard rule:**

- `pipeline`, `color`, `export`, `core-image`, `edit-model`, `catalog`, `raw-decode`,
  `image-decode`, `camera-profile`, `lens-profile` — **zero GUI dependency**, driven headless by the
  CLI and the tests. Nothing here may know egui exists.
- `crates/app/src/lib.rs` and its modules — **no egui.** This is where unit-testable logic lives
  (`history`, `selection`, `action_log`, `sync`, `batch_export`, `view_region`, …). If a thing can be
  tested without a window, it belongs here.
- `crates/app` **bin** modules — `theme`, `modals`, `develop`, `library`, `filmstrip`, `thumb`,
  `shell`, `rail`. egui lives here and only here.

**Then the module question, by kind of thing:**

| Adding | Goes in |
| --- | --- |
| A pipeline stage | `crates/pipeline/src/stages/<name>.rs`, one stage per file |
| An edit parameter | `crates/edit-model/src/lib.rs` — **and** its `EditGroups` group, `merge_from`'s destructure, and the three exhaustive fixtures |
| A reusable image primitive | `core-image`, or `pipeline/src/spatial.rs` for kernels |
| Logic behind a UI gesture | a **lib** module in `crates/app/src`, taking plain types — the `crop_tool` / `mask_tool` / `panel_order` pattern |
| The gesture itself | the relevant bin module under `develop/` or beside `library.rs` |
| A dialog | `crates/app/src/modals/<flow>.rs` — grouped by *flow*, not one file per dialog |
| A pure function a test wants to reach | anywhere in a **lib**; ⚠ never a `const` or helper inside a bin module that no test can import |

**⚠⚠ The bin-crate dead-code trap, which has now bitten ten times.** In a **binary** crate a `pub`
item with no production caller is dead code, and `clippy -D warnings` rejects it. So **an item lands
in the same commit as its first caller** — you cannot add a helper in one commit and use it in the
next. Plan the commit split around that, not around tidiness.

**Two questions that catch most misplacements:**
1. *Can this be tested without opening a window?* If yes and it is in a bin module, it is in the
   wrong place.
2. *Would a second caller want this?* If yes, it is a lib module, not a private helper.

**⚠ If a file you are editing is already over ~600 lines, say so in your report** rather than adding
to it silently. That is how the two W0 files got here.

---

## 6. What does not parallelise

Stated so nobody tries:

- **A design question.** M5.4b (where a display rendering lives) and M5.2's sidecar migration each
  need a decision before any code, and the decision cannot be split across agents. Brainstorm those
  in the main session first, then dispatch the implementation.

  ⚠⚠ **AND THE MECHANICAL REASON THIS MATTERS: A BACKGROUND AGENT HAS NO CHANNEL TO THE USER.**
  `AskUserQuestion` belongs to the orchestrating session, not to a subagent. An agent that meets a
  design question can only **stop and report it**; the loop back is *agent stops → orchestrator
  relays → user answers → orchestrator re-messages the agent, which resumes with its context intact*.
  That works, and it stalls one branch for a full round trip while the other three run ahead. ⚠ So
  **settle the decisions before dispatch** — and where a decision genuinely needs evidence first,
  scope that agent to **measure and report** rather than to build, which turns an accidental stall
  into a planned checkpoint. M7.3a in W1 is exactly that shape (user's call, 2026-09-01).
- ⚠ **Never let an agent decide a question you would have wanted asked.** An agent given an open
  design choice does not stall — it picks one silently and the choice ships. That is the failure
  mode this section exists to prevent, and it is worse than the stall.
- **The merge itself.** Reviewing four branches is one person's job.
- **Anything whose "Done when" is a judgement about a photograph.** M5.4, M7.3 and M8.4 all end in
  looking at an image and deciding. An agent can produce the comparison; it should not sign it off.
