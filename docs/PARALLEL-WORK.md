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

**Between waves:** merge everything, run the full suite once on the merged result, and *then* dispatch
the next wave. ⚠ A green run in each worktree is not evidence about the merge — three of this
project's recorded bugs were interactions between separately-correct changes.

---

## 4. The wave plan

| | A | B | C | D |
| --- | --- | --- | --- | --- |
| **W0** | **Split `canvas.rs` and `main.rs`** — prep, run alone | — | — | — |
| **W1** | M5.1a tone curve widget | M6.1 seam + filename templating | **M7.3a interpolator** *(owns goldens)* | M5.5a crop guides |
| **W2** | M5.2 WB Kelvin *(owns `edit-model`)* | M6.1b/c/d metadata, watermark, output sharpening | M7.1a luminance mask *(owns `Mask`)* | M6.5a/b compare + survey |
| **W3** | M5.3a spot headless *(owns `Edits` + stage order)* | M6.3 XMP *(owns schema **v6**)* | M5.4 auto tone | M7.6 tool gaps |
| **W4** | M5.3b spot canvas | M5.5b straighten line | M7.4 Hough verticals | M6.5d colour readout |

**Why W0 exists.** `canvas.rs` is 1,890 lines and is wanted by **six** milestones; `main.rs` is 4,375
and is touched by any new tool or modal. They are the two biggest serialisation points in the plan,
and Wave 4 is entirely canvas work that would otherwise queue behind itself. Splitting them first is
a pure refactor with the headless canvas fixture already in place to prove it moved no behaviour.

⚠ `main.rs` at 4,375 lines: the backlog says to split it "when it hurts". With four agents in it, it
hurts.

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
- **The merge itself.** Reviewing four branches is one person's job.
- **Anything whose "Done when" is a judgement about a photograph.** M5.4, M7.3 and M8.4 all end in
  looking at an image and deciding. An agent can produce the comparison; it should not sign it off.
