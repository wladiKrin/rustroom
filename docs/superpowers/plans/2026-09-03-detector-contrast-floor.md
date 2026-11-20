# M7.4 contrast floor: Implementation Plan

> Executed inline on branch `m7.4-contrast-floor`. Spec:
> `docs/superpowers/specs/2026-09-03-detector-contrast-floor-design.md`.

**Goal:** Make the vertical detector fire on the abbey and stay silent on the kitten, by replacing
a threshold relative to the frame's strongest gradient with an absolute one expressed in 8-bit
levels of edge contrast.

**Architecture:** `detect::verticals` display-encodes its luminance plane before the Sobel, so an
absolute threshold means something. `MIN_EDGE_LEVELS` replaces `MAG_FRACTION`, and its value is read
off a precision/recall sweep against a hand-labelled subset of the user's own RAWs rather than
asserted.

**Tech Stack:** Rust, `pipeline` (zero GUI dependency), `color::linear_to_srgb`.

**Baseline:** `main` is at **1858 tests, 53 goldens**. Track C's unmerged branch adds its own.

---

## Build environment — every shell

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

**Conventions:** no `Co-Authored-By` trailer; **never push**; `fmt` + `clippy --all-targets
--all-features -- -D warnings` + `test --all` green before reporting done.

⚠ **Read every exit code from a file, never through a pipe** — `${PIPESTATUS[0]}` is bash and this
shell is zsh, so it expands to nothing and a check printing `EXIT: ` reads as a pass:

```bash
cargo clippy --all-targets --all-features -- -D warnings > /tmp/cf-clippy.log 2>&1; echo "EXIT=$?"
```

⚠⚠ **Sabotage discipline, and this plan is written after five of mine passed while proving
nothing.** Confirm each of these every time:
1. The **edit landed** — after any scripted edit, grep for the new text.
2. The run **compiled** — a `test result:` line appears at all.
3. The **named** test failed — not merely that some suite reported a failure.
4. The **target is right** — `color_readout`'s tests live in `--lib`; running `--bin` matched eight
   unrelated tests and printed `ok`.
5. The filter **matched something** — `cargo test` takes a **substring, not a regex**; `"a\|b"`
   matched nothing and printed `0 passed … ok`.

---

## File Structure

| File | Responsibility |
| --- | --- |
| **Modify** `crates/pipeline/src/perspective/detect.rs` | The encode, `MIN_EDGE_LEVELS`, `SOBEL_STEP_GAIN`; `MAG_FRACTION` deleted. |
| **Modify** `crates/app/examples/detect_probe.rs` (178 lines) | A labels file and a sweep mode. |
| **Untouched** | `spatial::sobel`, `perspective_detect.rs`, `interaction.rs`, `panel/geometry.rs`, the app glue, the **53 goldens**. |

⚠ The encode goes **inside `detect::verticals`**, not in `analysis_luma`, so the wrong colour space
is unrepresentable at the boundary — `LensCorrectionStage`'s reasoning for building its own
`Corrector`. `analysis_luma` keeps returning linear luminance and is not edited.

---

## Task 1: rebase C onto main and get it green

⚠⚠ **Do this first and do not skip it.** Track C branched from `f6cf14e`; `main` is now 23 commits
ahead, including W4's B and D and five follow-ups that touch three of C's files. Tuning a constant
on a branch that does not build against current `main` would be measuring the wrong thing.

**Files:** whatever the rebase names.

- [ ] **Step 1: Rebase**

```bash
cd ~/Projects/rustroom
git -C .claude/worktrees/agent-a5727469fb1835fc8 rebase main > /tmp/cf-rebase.log 2>&1; echo "EXIT=$?"
git -C .claude/worktrees/agent-a5727469fb1835fc8 status --short | grep -E "^(UU|AA)" || echo "(no conflicts)"
```

`git merge-tree` reports **0 conflict hunks** — three files are "changed in both" (`lib.rs`,
`interaction.rs`, `panel/geometry.rs`) but auto-merge. Expect a clean rebase.

⚠ If a conflict does appear, both sides are **additions**: keep both. `lib.rs` gains three `pub mod`
lines, `interaction.rs` gains `auto_guides` beside `solve_guides`, `geometry.rs` gains the Auto
button in the Guides block ~70 lines below B's Straighten toggle.

- [ ] **Step 2: Build, and expect the compile errors the rebase cannot see**

```bash
cargo build -p app > /tmp/cf-build.log 2>&1; echo "EXIT=$?"; grep -E "^error" -A6 /tmp/cf-build.log | head -40
```

⚠ A clean textual merge does **not** mean it compiles. The likely breakages, both from B's work
landing under C:
- `panel::geometry::show` gained C's `auto: &mut bool` parameter; `panel/mod.rs`'s caller on `main`
  has changed shape since.
- C's Auto button sits in the Guides block, which B edited when the straighten toggle's gate was
  removed.

Fix whatever the compiler names. **Do not change C's design to make it compile** — if the two
genuinely conflict in behaviour, stop and report it rather than picking.

- [ ] **Step 3: Merge C's branch into ours**

```bash
git checkout m7.4-contrast-floor
git merge --no-ff worktree-agent-a5727469fb1835fc8 -m "merge: track C's vertical detector, rebased onto main" > /tmp/cf-merge.log 2>&1; echo "EXIT=$?"
```

- [ ] **Step 4: Establish the real baseline**

```bash
cargo fmt --check > /tmp/cf-fmt.log 2>&1; echo "FMT=$?"
cargo clippy --all-targets --all-features -- -D warnings > /tmp/cf-clippy.log 2>&1; echo "CLIPPY=$?"
cargo test --all > /tmp/cf-test.log 2>&1; echo "TEST=$?"
grep "^test result:" /tmp/cf-test.log | awk '{p+=$4; f+=$6} END {print "passed:",p," failed:",f}'
grep -c "^test golden" /tmp/cf-test.log
```

**Write the number down.** It is 1858 plus C's tests, and every later step is measured against it.
⚠ Assert the golden count is **53** and that they **ran** — "0 failed" with the golden suite silently
not running is the false pass this project keeps recording.

- [ ] **Step 5: Commit the merge if anything had to be fixed**

```bash
git add -A && git commit -q -m "fix: C's detector against current main after the rebase"
```

---

## Task 2: display-encode the plane inside the detector

**Files:** `crates/pipeline/src/perspective/detect.rs`

- [ ] **Step 1: Write the failing test**

⚠⚠ **This is the only test that can see the encode being deleted.** Every existing fixture uses
strong edges that clear any floor either way, so dropping `linear_to_srgb` leaves them all green.

Add to `detect.rs`'s `mod tests`:

```rust
    /// Two vertical edges of the **same linear contrast**, one in deep shadow and one near white.
    ///
    /// Linear values: the shadow pair steps 0.02 → 0.06, the highlight pair 0.90 → 0.94. Both are
    /// a linear delta of 0.04.
    fn same_linear_delta(w: usize, h: usize, low: f32) -> Vec<f32> {
        let mut p = vec![low; w * h];
        for y in 0..h {
            for x in w / 2..w {
                p[y * w + x] = low + 0.04;
            }
        }
        p
    }

    /// ⚠⚠ **The encode is what makes an absolute floor mean anything, and only this test can see
    /// it go.** Under a *linear* Sobel these two frames are identical — the same 0.04 delta — so
    /// any threshold treats them alike. Display-encoded, the sRGB curve expands the shadows:
    /// **0.02 → 0.06 spans 30.6 8-bit levels while 0.90 → 0.94 spans 4.7** — a factor of six and a
    /// half. The shadow edge is strongly visible and the highlight edge is nearly invisible, which
    /// is exactly the judgement the detector must make and cannot make on linear values.
    ///
    /// ⚠ Those two numbers were computed, not estimated; recompute them if the fixture's values
    /// change, because a wrong number in this comment is what would make a later reader trust the
    /// wrong threshold.
    ///
    /// ⚠ This project has the same bug on record in the Whites slider: perceptual thirds read off
    /// linear luminance meant "the Whites slider did nothing on most photographs".
    #[test]
    fn a_shadow_edge_and_a_highlight_edge_of_equal_linear_contrast_are_not_equal() {
        let (w, h) = (128, 128);
        let shadow = verticals(&same_linear_delta(w, h, 0.02), w, h);
        let highlight = verticals(&same_linear_delta(w, h, 0.90), w, h);
        assert!(
            !shadow.is_empty(),
            "the shadow edge — about 40 levels of contrast — was not detected"
        );
        assert!(
            highlight.is_empty(),
            "the highlight edge — about 11 levels — was detected: {highlight:?}, so the plane is \
             being measured linearly and the floor means nothing"
        );
    }
```

- [ ] **Step 2: Run it and watch it fail**

```bash
cargo test -p pipeline --lib perspective::detect > /tmp/cf-t2.log 2>&1; echo "EXIT=$?"
grep -E "^test .*FAILED|^test result" /tmp/cf-t2.log | head -3
```

Expected: **FAIL**. With the relative `MAG_FRACTION` still in place both frames behave identically,
so one of the two assertions must break. ⚠ Confirm a `test result:` line appears — its absence means
a compile error, not a pass.

- [ ] **Step 3: Encode inside `verticals`**

In `detect.rs`, replace:

```rust
    let g = spatial::sobel(luma, w, h);
```

with:

```rust
    // ⚠⚠ **Display-encoded before the Sobel, and this is what lets the floor below be an absolute
    // number.** `luma` arrives as `color::luminance` — linear, scene-referred ProPhoto — where a
    // gradient of 0.01 is a strongly visible edge in shadow and invisible near white. One
    // threshold cannot be right across a frame in that space.
    //
    // ⚠⚠ This project has the identical bug on record in the Whites slider: "the region weights
    // read as perceptual thirds… but `l` arrives as **linear** luminance, where 2/3 is sRGB 0.85
    // and 1/3 is sRGB 0.60", so "the Whites slider did nothing on most photographs". The weights
    // are taken on display-encoded luminance now, and so is this.
    //
    // ⚠ Done **here** rather than in `perspective_detect::analysis_luma`, so a caller cannot hand
    // this function the wrong colour space — the reasoning `LensCorrectionStage` records for
    // building its own `Corrector` rather than accepting one: handing it in "would make a size
    // mismatch representable, and it would be silently wrong rather than an error."
    //
    // ⚠ `linear_to_srgb` clamps to [0, 1], which also bounds the scene-referred range. Nothing is
    // lost: a clipped highlight has no gradient to find.
    let encoded: Vec<f32> = luma.iter().copied().map(color::linear_to_srgb).collect();
    let g = spatial::sobel(&encoded, w, h);
```

Add `color` to `crates/pipeline/Cargo.toml`'s dependencies if it is not already there:

```bash
grep -n "^color" crates/pipeline/Cargo.toml || echo "NEEDS ADDING"
```

- [ ] **Step 4: Run — it may still fail, and that is expected**

The encode alone does not create an absolute floor; `MAG_FRACTION` is still relative, so the
highlight frame's own strongest gradient is still its own edge and it still fires. **Task 3 is what
makes this test pass.** Note the failure and continue.

- [ ] **Step 5: Commit the encode**

```bash
cargo fmt
git add -A && git commit -q -m "feat(pipeline): the vertical detector measures display-encoded luminance"
```

---

## Task 3: `MIN_EDGE_LEVELS` replaces `MAG_FRACTION`

**Files:** `crates/pipeline/src/perspective/detect.rs`

- [ ] **Step 1: Write the test that pins what the constant *means***

```rust
    /// A single vertical step of a known contrast, in 8-bit levels, on a mid-grey field.
    ///
    /// Built in **display** units and converted back to linear, because that is the space the
    /// constant is expressed in and the detector re-encodes internally.
    fn step_of_levels(w: usize, h: usize, levels: f32) -> Vec<f32> {
        let base = 0.5f32;
        let hi = base + levels / 255.0;
        let mut p = vec![color::srgb_to_linear(base); w * h];
        for y in 0..h {
            for x in w / 2..w {
                p[y * w + x] = color::srgb_to_linear(hi);
            }
        }
        p
    }

    /// ⚠⚠ **The constant means what it says.** Without dividing the Sobel response by its
    /// step gain the threshold is four times what the name claims, and no "does it detect?" test
    /// can see a silent factor of four — it would simply behave as though the constant were 4×
    /// larger, which looks like a tuning choice rather than a bug.
    #[test]
    fn the_floor_is_expressed_in_levels_of_edge_contrast() {
        let (w, h) = (128, 128);
        let below = MIN_EDGE_LEVELS - 2.0;
        let above = MIN_EDGE_LEVELS + 2.0;
        assert!(
            verticals(&step_of_levels(w, h, below), w, h).is_empty(),
            "an edge of {below} levels cleared a floor of {MIN_EDGE_LEVELS}"
        );
        assert!(
            !verticals(&step_of_levels(w, h, above), w, h).is_empty(),
            "an edge of {above} levels did not clear a floor of {MIN_EDGE_LEVELS}"
        );
    }
```

- [ ] **Step 2: Run — expect a compile failure naming `MIN_EDGE_LEVELS`**

```bash
cargo test -p pipeline --lib perspective::detect > /tmp/cf-t3.log 2>&1; echo "EXIT=$?"; grep -E "^error" -A4 /tmp/cf-t3.log | head -8
```

- [ ] **Step 3: Replace the constant**

Delete `MAG_FRACTION` and its doc comment entirely. Add:

```rust
/// The contrast an edge must carry to cast a vote, in **8-bit levels across the edge**.
///
/// ⚠⚠ **Absolute, where this was a fraction of the frame's own strongest gradient — and that
/// relative form was the cause of both of M7.4's measured failures.** It was simultaneously too
/// permissive on a soft frame, where a smooth sky gradient clears a quarter of a small maximum
/// (Walhalla: two lines found *in the sky*, giving Vertical −42.5 and columns leaning left), and
/// too strict on a contrasty one, where something brighter than the building sets the bar above
/// the building's own edges (Weltenburg Abbey: nothing found at all). ⚠ A relative threshold also
/// structurally cannot express *"there is no vertical in this photograph"*, which is why
/// `MIN_LENGTH_FRACTION` had to be added mid-flight just to stop a landscape returning two lines.
///
/// ⚠ Only meaningful because [`verticals`] display-encodes first: in linear light the same number
/// is a strong edge in shadow and an invisible one near white.
///
/// ⚠⚠ **Chosen from the sweep, not asserted.** `detect_probe --sweep` reports precision and recall
/// against a hand-labelled corpus; this value is the one read off that curve. Record the numbers
/// here when Task 5 sets it, or nobody can revisit the choice — the rule `MEDIAN_TARGET` and
/// `TEXTURE_GUIDED_EPS` both follow.
pub const MIN_EDGE_LEVELS: f32 = 8.0;

/// Sobel's response to a unit step: the kernel sums to 4 on the leading side.
///
/// ⚠ Dividing by it is what makes [`MIN_EDGE_LEVELS`] mean the contrast *across an edge* rather
/// than a raw Sobel magnitude, which is four times larger and a quantity nobody can reason about.
const SOBEL_STEP_GAIN: f32 = 4.0;
```

Then replace the vote gate. Delete these lines:

```rust
    let strongest = g.mag.iter().copied().fold(0.0f32, f32::max);
    if strongest <= 1e-6 {
        return Vec::new();
    }
    let floor = strongest * MAG_FRACTION;
```

with:

```rust
    // ⚠ An absolute floor, in the encoded units the Sobel just measured. Nothing about the rest of
    // the frame can raise or lower it — which is the whole point.
    let floor = MIN_EDGE_LEVELS / 255.0 * SOBEL_STEP_GAIN;
```

⚠ **The magnitudes still weight the accumulator**, so a strong edge is still preferred over a faint
one of the same length. Only *who may vote* has changed. Do not also change the accumulator.

- [ ] **Step 4: Run both new tests**

```bash
cargo test -p pipeline --lib perspective::detect > /tmp/cf-t3.log 2>&1; echo "EXIT=$?"
grep -E "^test .*FAILED|^test result" /tmp/cf-t3.log | head -6
```

Expected: `a_shadow_edge_and_a_highlight_edge_…` and `the_floor_is_expressed_in_levels_…` both pass.

⚠ **Some existing fixtures will now fail, and that is expected** — they paint 0.05 and 1.0 linear,
which encode to ~0.24 and 1.0, and every threshold now means something different. **Re-judge each
against what the fixture is for.** Never nudge a value until the test goes green: this project
records three tests that asserted something false about correct code, and
`texture_minus_100_removes_the_fine_band`'s threshold moving twice, *"each time because the model
changed and never to make a red test green."* Write the reason for each change in the fixture's own
comment.

- [ ] **Step 5: Sabotage**

| # | Sabotage | Must fail | Target |
| --- | --- | --- | --- |
| 1 | Delete the `linear_to_srgb` map, Sobel the raw `luma` | `a_shadow_edge_and_a_highlight_edge_…` | `-p pipeline --lib` |
| 2 | Drop the `SOBEL_STEP_GAIN` division | `the_floor_is_expressed_in_levels_…` | `-p pipeline --lib` |
| 3 | `let floor = 0.0;` | `a_flat_frame_finds_nothing` | `-p pipeline --lib` |
| 4 | `let floor = 1e9;` | `two_converging_verticals_are_found_…` | `-p pipeline --lib` |

For each: confirm the edit landed (`grep`), confirm a `test result:` line appears, confirm the
**named** test failed, revert.

- [ ] **Step 6: Commit**

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings > /tmp/cf-clippy.log 2>&1; echo "CLIPPY=$?"
git add -A && git commit -q -m "feat(pipeline): an absolute contrast floor in levels, replacing the relative one"
```

---

## Task 4: the labelled corpus and the sweep

**Files:** `crates/app/examples/detect_probe.rs`

- [ ] **Step 1: Define the labels format**

A plain text file, one frame per line, `<verdict> <path>`:

```
fire   ~/Desktop/fuji/DSCF6296.RAF
silent ~/Desktop/fuji/DSCF6301.RAF
```

⚠ **Not committed.** It names photographs on one machine, so it follows the `RAW_FIXTURE` /
`DCP_FIXTURE` convention: *"Fixtures this session's tools depend on, none of them in the repo."*
Add `*.labels` to `.gitignore`.

- [ ] **Step 2: Add the sweep mode**

Append to `detect_probe.rs`:

```rust
/// One labelled frame.
struct Labelled {
    path: std::path::PathBuf,
    /// Whether the detector *should* find two verticals here.
    fire: bool,
}

/// Read a labels file: `<fire|silent> <path>` per line, `#` comments and blanks ignored.
fn read_labels(p: &std::path::Path) -> std::io::Result<Vec<Labelled>> {
    let text = std::fs::read_to_string(p)?;
    let mut out = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((verdict, path)) = line.split_once(char::is_whitespace) else {
            eprintln!("  ⚠ skipping unparseable line: {line}");
            continue;
        };
        let fire = match verdict.trim() {
            "fire" => true,
            "silent" => false,
            other => {
                eprintln!("  ⚠ skipping unknown verdict {other:?}: {line}");
                continue;
            }
        };
        out.push(Labelled {
            path: std::path::PathBuf::from(path.trim()),
            fire,
        });
    }
    Ok(out)
}

/// Precision and recall of "the detector fired" against the labels, for one floor value.
///
/// ⚠⚠ **Recall is the half that matters and the half the old evidence lacked.** The statistic
/// track C had was a fire *rate* — "fires on 54% of 70" — against which a floor that silences
/// everything scores perfectly and is useless. Only recall against the frames that *should* fire
/// exposes that.
fn score(results: &[(bool, bool)]) -> (f32, f32, usize, usize) {
    let tp = results.iter().filter(|(want, got)| *want && *got).count();
    let fp = results.iter().filter(|(want, got)| !*want && *got).count();
    let fnn = results.iter().filter(|(want, got)| *want && !*got).count();
    let precision = if tp + fp == 0 {
        f32::NAN
    } else {
        tp as f32 / (tp + fp) as f32
    };
    let recall = if tp + fnn == 0 {
        f32::NAN
    } else {
        tp as f32 / (tp + fnn) as f32
    };
    (precision, recall, fp, fnn)
}
```

⚠ The sweep itself cannot vary `MIN_EDGE_LEVELS` at runtime — it is a `const`. **Take the floor as a
CLI argument in the probe and pass it to a `verticals_with_floor(luma, w, h, floor_levels)`
variant**, with `verticals` delegating to it at `MIN_EDGE_LEVELS`. That keeps one implementation and
makes the sweep possible:

```rust
// in detect.rs
pub fn verticals(luma: &[f32], w: usize, h: usize) -> Vec<Guide> {
    verticals_with_floor(luma, w, h, MIN_EDGE_LEVELS)
}

/// As [`verticals`], with the contrast floor supplied — for the sweep in `detect_probe`.
///
/// ⚠ `verticals` delegates here rather than the two sharing copied code, so the sweep measures
/// exactly what production runs. A second implementation is how a swept value comes to be right
/// for a detector nobody ships.
pub fn verticals_with_floor(luma: &[f32], w: usize, h: usize, floor_levels: f32) -> Vec<Guide> {
```

- [ ] **Step 3: Print the curve**

The sweep runs the labelled set at each of a range of floors and prints one row per value:

```
levels  precision  recall   false-fires  misses
  4.0      0.55     1.00        9           0
  6.0      0.71     1.00        5           0
  8.0      0.86     0.92        2           1
 10.0      0.90     0.75        1           3
```

…then names the frames wrong at each, so a value can be judged by *which* photographs it gets wrong
rather than by a number alone.

- [ ] **Step 4: Verify it refuses to run vacuously**

⚠ With no labels file the probe must **say so and exit non-zero**, not print an empty table that
reads as a pass. Check:

```bash
cargo run --release -p app --example detect_probe -- --sweep /nonexistent.labels > /tmp/cf-probe.log 2>&1; echo "EXIT=$?"; cat /tmp/cf-probe.log
```

Expected: a clear message and a non-zero exit.

- [ ] **Step 5: Commit**

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings > /tmp/cf-clippy.log 2>&1; echo "CLIPPY=$?"
git add -A && git commit -q -m "test(app): a labelled sweep for the detector's contrast floor"
```

---

## Task 5: choose the constant

⚠⚠ **This task needs the user.** The labels are theirs to write; the plan cannot invent them.

- [ ] **Step 1: Ask for the labels**

Five are known from C's report and go in first:

```
fire   <abbey>       # DSCF6296 — the cleanest converging verticals available
fire   <walhalla>    # DSCF6270 — must find the building, not the sky
silent <kitten>      # blurred close-up, previously +95.3
silent <poster>      # flat wall, previously +88.9
silent <tree>        # tree trunk, previously −38.4
```

Ask for 10–20 more from the 70.

- [ ] **Step 2: Run the sweep**

```bash
cargo run --release -p app --example detect_probe -- --sweep ~/Desktop/fuji/verticals.labels > /tmp/cf-sweep.log 2>&1; echo "EXIT=$?"; cat /tmp/cf-sweep.log
```

- [ ] **Step 3: Choose, and record why**

Set `MIN_EDGE_LEVELS` and **write the sweep's numbers into its doc comment** — the precision and
recall at the chosen value and at its neighbours.

⚠ Prefer **recall** where the curve is flat: a miss is a button that appears broken with nothing on
screen to act on, while a false fire draws a *visible, editable* guide. That is the same asymmetry
`MAX_LEAN_DEG` was chosen on, and it is already recorded there.

⚠ **If no value gives acceptable precision and recall together, say so and stop.** That is a
finding: it would mean the floor alone is insufficient and the conditioning test §7 of the spec
defers becomes necessary. **Do not tune `MIN_SEPARATION` or `MIN_LENGTH_FRACTION` to compensate** —
the spec holds them fixed precisely so this change can be judged.

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -q -m "tune(pipeline): MIN_EDGE_LEVELS chosen from the labelled sweep"
```

---

## Task 6: verify, and hand the photographs back

- [ ] **Step 1: The gates, each exit code read from a file**

```bash
cargo fmt --check > /tmp/cf-fmt.log 2>&1; echo "FMT=$?"
cargo clippy --all-targets --all-features -- -D warnings > /tmp/cf-clippy.log 2>&1; echo "CLIPPY=$?"
cargo test --all > /tmp/cf-test.log 2>&1; echo "TEST=$?"
grep "^test result:" /tmp/cf-test.log | awk '{p+=$4; f+=$6} END {print "passed:",p," failed:",f}'
grep -c "^test golden" /tmp/cf-test.log
```

⚠ All four together. `cargo test` being green is **not** evidence about clippy — eight recorded
instances, one of them in this same session.

- [ ] **Step 2: Goldens untouched**

```bash
git diff --stat main -- crates/pipeline/tests
```

Expected **empty**, and 53 golden tests must have **run**.

- [ ] **Step 3: The five named frames, by hand**

Run the probe on each and confirm: abbey **fires on the building**; Walhalla fires on the columns or
declines; kitten, poster and tree **silent**.

- [ ] **Step 4: The GUI**

```bash
cargo run --release -p app
```

Open the abbey, Geometry tool, press **Auto**. The perspective sliders move, Guide Mode switches on,
two guides sit **on the building**, dragging one re-solves, Ctrl+Z undoes it in one step. Then press
Auto on a landscape: it must report "need two" in the status line rather than sitting silent.

⚠ The agent cannot drive the GUI here — `AXIsProcessTrusted()` is false and `screencapture` is
blocked — so this step is the user's.

- [ ] **Step 5: Produce the before/after and STOP**

⚠⚠ **M7.4's "Done when" is a judgement about a photograph.** §6: *"An agent can produce the
comparison; it should not sign it off."* Hand over the two images and the sweep numbers, and say
plainly that the judgement is the user's.

- [ ] **Step 6: Report**

- The **sweep table** and the value chosen, with the frames each candidate got wrong.
- The test count before and after, and that the 53 goldens ran and are byte-identical.
- **Which existing fixtures moved and why** — each justified by what the fixture is for.
- **Any sabotage that did not fail, and any that was inert**, named as such.
- ⚠ Whether `MIN_SEPARATION`'s 0.17 sky pair still occurs. If it does, that is the evidence for
  raising it — **as its own change**, not folded into this one.

---

## Not in scope

`MIN_SEPARATION` (0.15), `MIN_LENGTH_FRACTION` (0.35), `MAX_LEAN_DEG` (30°), verticals only, and no
conditioning test on the vanishing point. The spec holds all of them fixed so this change can be
judged on its own.
