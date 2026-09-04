# Guide refinement: Implementation Plan

> Executed inline on branch `m7.4-contrast-floor`. Spec:
> `docs/superpowers/specs/2026-09-04-guide-refinement-design.md`.

**Goal:** Draw two rough guides down a building's edges, press **Refine guides**, and they land on
the real edges — or stay exactly where you put them and say so.

**Architecture:** `verticals` is split at the seam between *finding candidates* and *choosing two*.
`refine` reuses the whole candidate half — floor, direction gate, contiguity — and replaces only the
choice, matching each drawn guide to the strongest candidate within a snap radius.

**Tech Stack:** Rust; `pipeline` (zero GUI dependency), `crates/app` lib + bin.

**Baseline: 1888 tests, 53 goldens**, `fmt` / `clippy --all-targets --all-features -D warnings` /
`test --all` all green on `m7.4-contrast-floor`.

---

## Build environment — every shell

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

**Conventions:** no `Co-Authored-By` trailer; **never push**; all three gates green before reporting
done.

⚠ **Read every exit code from a file, never through a pipe** — `${PIPESTATUS[0]}` is bash and this
shell is zsh, so a check printing `EXIT: ` reads as a pass.

⚠⚠ **Sabotage discipline. This session alone produced three sabotages that passed while proving
nothing.** Confirm every time:
1. The **edit landed** — grep for the new text after any scripted edit.
2. The run **compiled** — a `test result:` line appears at all.
3. The **named** test failed, not merely that some suite did.
4. The **target is right** — `-p pipeline --lib` for `detect`, `-p app --bin Rustroom` for the panel.
5. The filter **matched something** — `cargo test` takes a **substring, not a regex**.

⚠ **And a fixture can be inert.** Two direction-gate sabotages passed because the fixture's gaps
could not be bridged by the thing under test. If a sabotage does not fail, suspect the fixture before
concluding the guard is redundant.

---

## File Structure

| File | Responsibility |
| --- | --- |
| **Modify** `crates/pipeline/src/perspective/detect.rs` | Split out `candidates`; add `SNAP_RADIUS`, `refine`. |
| **Modify** `crates/app/src/perspective_detect.rs` | `refine_guides`, mirroring `detect_verticals`. |
| **Modify** `crates/app/src/interaction.rs:213` | `auto_guides` branches on whether guides exist. |
| **Modify** `crates/app/src/develop/panel/geometry.rs:300` | The button's label and hover text. |
| **Modify** `crates/app/examples/detect_overlay.rs` | A `--refine` mode, to measure `SNAP_RADIUS`. |
| **Untouched** | `guides.rs`, `MIN_SEPARATION`, `MIN_EDGE_LEVELS`, `MIN_RUN_FRACTION`, the **53 goldens**. |

---

## Task 1: split the candidate half out of `verticals`

⚠⚠ **A pure refactor with no behaviour change, landed on its own so the next task's diff is only the
new rule.** The evidence it moved nothing is that **all 16 existing `detect` tests pass verbatim** —
that matters more than anything else in this plan.

**Files:** `crates/pipeline/src/perspective/detect.rs`

- [ ] **Step 1: Add the candidate struct and function**

Above `verticals`, add:

```rust
/// Everything a caller needs to turn accumulator peaks into lines, without re-deriving the frame's
/// geometry.
struct Candidates {
    /// Surviving peaks, strongest first.
    peaks: Vec<Peak>,
    cx: f32,
    cy: f32,
}

impl Candidates {
    /// Where this peak's line crosses row `y`, in pixels.
    fn at(&self, p: &Peak, y: f32) -> f32 {
        self.cx + p.offset + (y - self.cy) * p.angle.tan()
    }

    /// A peak as a frame-normalised guide spanning the full height.
    fn guide(&self, p: &Peak, w: usize, h: usize) -> Guide {
        Guide {
            from: [self.at(p, 0.0) / w as f32, 0.0],
            to: [self.at(p, h as f32) / w as f32, 1.0],
        }
    }
}
```

- [ ] **Step 2: Rename `verticals_with_floor`'s body to `candidates`**

Change its signature and its tail. The function keeps **every line** from `if w < 8` down to and
including the `peaks.retain(..)` contiguity filter, then returns:

```rust
fn candidates(luma: &[f32], w: usize, h: usize, floor_levels: f32) -> Candidates {
    // ... every existing line, unchanged, down to and including peaks.retain(..) ...
    peaks.sort_by(|a, b| {
        b.votes
            .partial_cmp(&a.votes)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Candidates { peaks, cx, cy }
}
```

⚠ The degenerate guards (`w < 8 || h < 8 || luma.len() != w * h`, and `strongest <= 1e-6`) return
`Candidates { peaks: Vec::new(), cx: 0.0, cy: 0.0 }` rather than `Vec::new()`.

⚠ The `sort_by` **moves up** into `candidates`, because both callers want strongest-first.

- [ ] **Step 3: Rebuild `verticals_with_floor` on top of it**

```rust
pub fn verticals_with_floor(luma: &[f32], w: usize, h: usize, floor_levels: f32) -> Vec<Guide> {
    let c = candidates(luma, w, h, floor_levels);
    if c.peaks.is_empty() {
        return Vec::new();
    }
    // ⚠⚠ **Separation, not the top two by vote.** The strongest peak is taken, then the strongest
    // peak that is far enough from it — because a vanishing point from two nearly-coincident lines
    // is ill-conditioned exactly when the correction is gentlest, which `guides.rs` records as
    // having been "reported in use as the image coming out rotated so that carefully drawn
    // vertical guides ended up not vertical at all."
    let min_sep = w as f32 * MIN_SEPARATION;
    let mut chosen = vec![&c.peaks[0]];
    if let Some(p) = c
        .peaks
        .iter()
        .skip(1)
        .find(|p| (p.offset - c.peaks[0].offset).abs() >= min_sep)
    {
        chosen.push(p);
    }
    chosen.iter().map(|p| c.guide(p, w, h)).collect()
}
```

- [ ] **Step 4: Run the whole detect suite — every test must pass VERBATIM**

```bash
cargo test -p pipeline --lib perspective::detect > /tmp/gr-t1.log 2>&1; echo "EXIT=$?"
grep "^test result" /tmp/gr-t1.log
```

Expected: **`16 passed; 0 failed`**. ⚠ If any test needs editing, **stop** — a pure refactor that
changes a test has changed behaviour, and the reason must be understood before continuing.

- [ ] **Step 5: Gates and commit**

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings > /tmp/gr-c1.log 2>&1; echo "CLIPPY=$?"
git add -A && git commit -q -m "refactor(pipeline): split candidate finding from choosing two lines

verticals_with_floor becomes candidates() plus the separation rule on top. Pure
move: all 16 detect tests pass unchanged, which is the evidence it changed no
behaviour. refine() needs the candidate half and must not grow a second copy of
it -- two candidate pipelines would drift."
```

---

## Task 2: `SNAP_RADIUS` and `refine`

**Files:** `crates/pipeline/src/perspective/detect.rs`

- [ ] **Step 1: Write the four failing tests**

Add to `mod tests`:

```rust
    /// A guide drawn as a straight vertical at frame fraction `at`, spanning the full height.
    fn drawn_at(at: f32) -> Guide {
        Guide {
            from: [at, 0.0],
            to: [at, 1.0],
        }
    }

    /// ⚠ A guide drawn slightly off a real edge lands on it — the feature working at all.
    #[test]
    fn a_guide_drawn_near_an_edge_snaps_onto_it() {
        let (w, h) = (512, 342);
        let mut p = dark(w, h);
        let x = 0.5 * (w - 1) as f32;
        line(&mut p, w, h, x, 2.0, x, h as f32 - 3.0);
        // Drawn 1.5% of the frame width off the real edge, well inside the radius.
        let out = refine(&p, w, h, &[drawn_at(0.515)]);
        assert_eq!(out.len(), 1);
        assert!(
            (out[0].from[0] - 0.5).abs() < 0.01,
            "the guide did not snap onto the edge: {:?}",
            out[0]
        );
    }

    /// ⚠⚠ **A guide with no candidate in range comes back BIT-IDENTICAL.** `assert_eq!` on the
    /// guide, not a tolerance: "unchanged" is exact, and a tolerance would hide a small unwanted
    /// move. Without this rule the silhouette beside Walhalla's building corner captures a line
    /// aimed at the corner, and snapping re-creates the bug it exists to fix.
    #[test]
    fn a_guide_far_from_every_edge_is_returned_unchanged() {
        let (w, h) = (512, 342);
        let mut p = dark(w, h);
        let x = 0.2 * (w - 1) as f32;
        line(&mut p, w, h, x, 2.0, x, h as f32 - 3.0);
        let drawn = drawn_at(0.8);
        let out = refine(&p, w, h, &[drawn]);
        assert_eq!(out, vec![drawn], "a guide was dragged to a distant edge");
    }

    /// ⚠⚠ **The one test that can distinguish "strongest in range" from "nearest in range."**
    /// A weak edge sits 1% away and a strong one 2.5% away, both inside the radius. Every other
    /// test here passes against either rule.
    #[test]
    fn inside_the_radius_the_strongest_candidate_wins_not_the_nearest() {
        let (w, h) = (512, 342);
        let mut p = dark(w, h);
        let weak = 0.51 * (w - 1) as f32;
        let strong = 0.475 * (w - 1) as f32;
        // The weak edge is short, so it clears contiguity but collects far fewer votes.
        line(&mut p, w, h, weak, 2.0, weak, 0.30 * h as f32);
        line(&mut p, w, h, strong, 2.0, strong, h as f32 - 3.0);
        let out = refine(&p, w, h, &[drawn_at(0.5)]);
        assert_eq!(out.len(), 1);
        assert!(
            (out[0].from[0] - 0.475).abs() < 0.01,
            "the nearer weak edge won over the stronger one: {:?}",
            out[0]
        );
    }

    /// ⚠ A horizontal-family guide has nothing to snap to — the detector finds verticals only —
    /// so it is returned untouched even on a frame full of strong verticals. Silently "improving"
    /// it would be inventing a result.
    #[test]
    fn a_horizontal_guide_is_never_refined() {
        let (w, h) = (512, 342);
        let mut p = dark(w, h);
        let x = 0.5 * (w - 1) as f32;
        line(&mut p, w, h, x, 2.0, x, h as f32 - 3.0);
        let drawn = Guide {
            from: [0.1, 0.5],
            to: [0.9, 0.52],
        };
        let out = refine(&p, w, h, &[drawn]);
        assert_eq!(out, vec![drawn], "a horizontal guide was refined");
    }
```

- [ ] **Step 2: Run, expect a compile failure naming `refine`**

```bash
cargo test -p pipeline --lib perspective::detect > /tmp/gr-t2.log 2>&1; echo "EXIT=$?"
grep -E "^error" -A4 /tmp/gr-t2.log | head -8
```

- [ ] **Step 3: Add the constant**

Beside `MIN_SEPARATION`:

```rust
/// How far a candidate may sit from the guide it refines, as a fraction of frame width.
///
/// ⚠⚠ **The bound is load-bearing, not a tuning detail.** Without it, a strong edge that is not the
/// one you aimed at captures your line — the person's silhouette beside Walhalla's building corner
/// is the measured case — and refinement re-creates the very bug it exists to fix. It must be small
/// enough that a wrong snap is close to impossible.
///
/// ⚠ Measured with `detect_overlay --refine`, not asserted. This milestone has already had one
/// guess overturned by measurement: the spec's "starting band of 6–10 levels" for
/// [`MIN_EDGE_LEVELS`] fired on 84% of frames.
pub const SNAP_RADIUS: f32 = 0.03;
```

- [ ] **Step 4: Implement `refine`**

```rust
/// Each guide replaced by the strongest real edge near it, or returned exactly as drawn.
///
/// ⚠⚠ **It reuses `candidates` whole — floor, direction gate, contiguity, peak ranking — and
/// replaces only the choice.** So an edge that failed contiguity can never be snapped to, and every
/// property the contrast-floor and contiguity work established still holds here.
///
/// ⚠ [`MIN_SEPARATION`] deliberately does **not** apply: it exists to stop the *detector* choosing
/// an ill-conditioned pair, and here the pair is the user's own. Drawing two near-parallel guides is
/// already possible and already ill-conditioned; this makes nothing worse.
pub fn refine(luma: &[f32], w: usize, h: usize, guides: &[Guide]) -> Vec<Guide> {
    let c = candidates(luma, w, h, MIN_EDGE_LEVELS);
    let radius = SNAP_RADIUS * w as f32;
    guides
        .iter()
        .map(|d| {
            // ⚠ Verticals only, and the family test is `|dy| >= |dx|` — the same rule
            // `guide_overlay` uses to colour a guide, so there is one definition of "vertical"
            // rather than two that can disagree.
            if (d.to[1] - d.from[1]).abs() < (d.to[0] - d.from[0]).abs() {
                return *d;
            }
            // ⚠⚠ The metric is the **largest gap anywhere down the frame**, which subsumes angle
            // and offset in one number: a candidate matching the angle but sitting 20 px away
            // fails, and so does one starting on the line and diverging. Two thresholds in two
            // units would be a quantity nobody can judge by eye.
            let drawn_at = |y: f32| {
                let t = y / h as f32;
                (d.from[0] + (d.to[0] - d.from[0]) * t) * w as f32
            };
            c.peaks
                .iter()
                .find(|p| {
                    let gap = |y: f32| (c.at(p, y) - drawn_at(y)).abs();
                    gap(0.0).max(gap(h as f32)) <= radius
                })
                .map_or(*d, |p| c.guide(p, w, h))
        })
        .collect()
}
```

⚠ `find` on a list already sorted strongest-first **is** "the strongest within the radius" — no
second sort. ⚠ The gap is evaluated at the two ends only: both lines are straight, so their
separation is monotonic between them and the maximum is at an endpoint.

- [ ] **Step 5: Run — all four pass, and the existing 16 still do**

```bash
cargo test -p pipeline --lib perspective::detect > /tmp/gr-t3.log 2>&1; echo "EXIT=$?"
grep "^test result" /tmp/gr-t3.log
```

Expected: **`20 passed; 0 failed`**.

- [ ] **Step 6: Sabotage**

| # | Sabotage | Must fail |
| --- | --- | --- |
| 1 | `radius = f32::INFINITY` | `a_guide_far_from_every_edge_is_returned_unchanged` |
| 2 | `radius = 0.0` | `a_guide_drawn_near_an_edge_snaps_onto_it` |
| 3 | Pick the **nearest** in range instead of the first (`min_by` on the gap) | `inside_the_radius_the_strongest_candidate_wins_not_the_nearest` |
| 4 | Delete the family check (`return *d` for horizontals) | `a_horizontal_guide_is_never_refined` |

⚠ For each: grep the edit landed, confirm a `test result:` line appeared, confirm the **named** test
failed, revert.

- [ ] **Step 7: Gates and commit**

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings > /tmp/gr-c2.log 2>&1; echo "CLIPPY=$?"
cargo test --all > /tmp/gr-full.log 2>&1; echo "TEST=$?" >> /tmp/gr-full.log
grep "^test result:" /tmp/gr-full.log | awk '{p+=$4; f+=$6} END {print p, f}'
git add -A && git commit -q -m "feat(pipeline): refine a drawn guide onto the strongest edge near it"
```

---

## Task 3: the app glue

**Files:** `crates/app/src/perspective_detect.rs`

- [ ] **Step 1: Add `refine_guides`, mirroring `detect_verticals`**

```rust
/// The drawn guides, each moved onto the strongest real edge near it.
///
/// ⚠ Same analysis plane as [`detect_verticals`] — Guide space, post lens correction and
/// orientation, pre-straighten, pre-perspective, pre-crop — which is the space `Session.guides`
/// are already in, so no coordinate conversion is needed or wanted.
pub fn refine_guides(
    buffer: &ImageBuf,
    edits: &Edits,
    lens: Option<LensShot>,
    resampler: Resampler,
    guides: &[Guide],
) -> Vec<Guide> {
    let (luma, w, h) = analysis_luma(buffer, edits, lens, resampler);
    detect::refine(&luma, w, h, guides)
}
```

- [ ] **Step 2: Build and commit**

```bash
cargo build -p app > /tmp/gr-b3.log 2>&1; echo "BUILD=$?"
cargo fmt
git add -A && git commit -q -m "feat(app): refine_guides, the glue behind Refine guides"
```

---

## Task 4: the button and the branch

**Files:** `crates/app/src/interaction.rs:213`, `crates/app/src/develop/panel/geometry.rs:300`

- [ ] **Step 1: Write the failing panel test**

In `geometry.rs`'s `mod tests`:

```rust
    /// ⚠⚠ **The label carries the state.** One button doing two things with nothing on screen
    /// saying which is the shape that produced this project's greyed-out straighten-button
    /// confusion, whose lesson is "a rule the user cannot see is one they cannot correct."
    ///
    /// ⚠ Asserts **both** labels, and that each is ABSENT in the other state. A test that only
    /// looked for "Refine guides" would pass against a panel drawing both buttons at once.
    #[test]
    fn the_auto_button_says_which_of_its_two_jobs_it_will_do() {
        let ctx = test_ui::ctx();
        let seen = |label: &str, with_guide: bool| {
            let mut s = session();
            if with_guide {
                s.guides.push(pipeline::perspective::guides::Guide {
                    from: [0.2, 0.2],
                    to: [0.2, 0.8],
                });
            }
            let mut aspect = None;
            let mut ed = Edited::default();
            test_ui::find(&ctx, label, |ui| {
                show(
                    ui,
                    &mut s,
                    &mut aspect,
                    &mut CompositionGuide::Off,
                    false,
                    &mut ed,
                    &mut false,
                );
            })
            .is_some()
        };
        assert!(seen("Auto", false), "with no guides it should offer to find them");
        assert!(!seen("Refine guides", false), "nothing to refine yet");
        assert!(seen("Refine guides", true), "with a guide it should offer to refine");
        assert!(!seen("Auto", true), "the two labels must not both be drawn");
    }
```

⚠ `test_ui::find(ctx, label, body) -> Option<Pos2>` lays the panel out once and locates the widget;
this is the exact shape `the_lens_scale_appears_only_when_a_profile_matched_and_distortion_is_on`
already uses at `geometry.rs:581`. `show`'s seven arguments are as that test passes them.

- [ ] **Step 2: Run, expect FAIL** (`Refine guides` not found).

- [ ] **Step 3: Change the button**

Replace the `ui.button("Auto")` block at `geometry.rs:300`:

```rust
    // ⚠⚠ The label reports which of the two jobs the click will do, because the behaviour depends
    // on whether guides exist and nothing else on screen says so.
    let refining = !s.guides.is_empty();
    let label = if refining { "Refine guides" } else { "Auto" };
    let hint = if refining {
        "Move each guide you drew onto the nearest strong edge. A guide with no edge near it is \
         left exactly where you put it."
    } else {
        "Find two vertical edges in the photograph and solve the tilt from them. The guides it \
         infers are left on screen so you can nudge, refine or delete them."
    };
    if ui.button(label).on_hover_text(hint).clicked() {
        *auto = true;
    }
```

- [ ] **Step 4: Branch `auto_guides`**

In `interaction.rs`, at the top of `auto_guides` after the session is borrowed:

```rust
        // ⚠⚠ **Refinement when guides exist, detection when they do not.** The whole tail below —
        // `show_guides`, the re-solve, the render request and the single history push — is shared,
        // because one click must stay one Ctrl+Z whichever job it did.
        let found = if s.guides.is_empty() {
            rustroom_app::perspective_detect::detect_verticals(
                &s.buffer,
                &s.edits,
                s.lens.shot.clone(),
                s.resampler,
            )
        } else {
            let drawn = s.guides.clone();
            let out = rustroom_app::perspective_detect::refine_guides(
                &s.buffer,
                &s.edits,
                s.lens.shot.clone(),
                s.resampler,
                &drawn,
            );
            // ⚠ Report both outcomes. Without this you cannot tell a guide that snapped from one
            // that did not, which is the difference between a precise line and a rough one.
            let moved = out.iter().zip(&drawn).filter(|(a, b)| a != b).count();
            self.status = if moved == drawn.len() {
                format!("Refined {moved} of {} guides.", drawn.len())
            } else {
                format!(
                    "Refined {moved} of {} — no edge found near the {}.",
                    drawn.len(),
                    if drawn.len() - moved == 1 { "other" } else { "others" }
                )
            };
            out
        };
```

⚠⚠ **The `found.len() < 2` early return must NOT apply to the refinement branch.** Refining one
guide is legitimate — the second may be drawn afterwards — and the existing message ("Draw them by
hand in Guide Mode") is nonsense when they already have. Guard it with `if s.guides.is_empty() &&
found.len() < 2`.

⚠ The `self.status` assignment at the end of `auto_guides` currently overwrites unconditionally with
"Perspective solved from two detected verticals." Set it only in the detection branch, or the
refinement message is destroyed one line after it is written.

- [ ] **Step 5: Run the app suite**

```bash
cargo test -p app --bin Rustroom > /tmp/gr-t4.log 2>&1; echo "EXIT=$?"
grep -E "^test .*FAILED|^test result" /tmp/gr-t4.log
```

- [ ] **Step 6: Sabotage**

| # | Sabotage | Must fail | Target |
| --- | --- | --- | --- |
| 1 | `let refining = false;` | `the_auto_button_says_which_of_its_two_jobs_it_will_do` | `--bin Rustroom` |
| 2 | Always call `detect_verticals` | that test still passes — ⚠ **expected**, and it means the *branch* has no test; add one asserting `s.guides` is unchanged in length after refining a single guide | `--bin Rustroom` |

- [ ] **Step 7: Gates and commit**

---

## Task 5: measure `SNAP_RADIUS`

⚠⚠ **0.03 is a guess until this task runs.** The spec forbids shipping it unmeasured.

**Files:** `crates/app/examples/detect_overlay.rs`

- [ ] **Step 1: Add a `--refine` mode**

```bash
cargo run --release -p app --example detect_overlay -- --refine <raw> 0.30 0.85
```

In `detect_overlay.rs`, add beside `main` and dispatch to it when the first argument is `--refine`:

```rust
/// Refine straight vertical guides drawn at the given frame fractions, and report how far each
/// moved — the measurement `SNAP_RADIUS` is chosen from.
fn probe_refine(path: &str, at: &[f32]) {
    let p = std::path::Path::new(path);
    let decoded = match raw_decode::decode_raw(p, &raw_decode::DecodeOptions::linear_scene()) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("{}: decode failed: {e}", p.display());
            std::process::exit(2);
        }
    };
    let m = &decoded.metadata;
    let lens = m.lens.as_ref().and_then(|li| {
        let db = lens_profile::Catalogue::bundled().ok()?;
        let profile = db.find(&m.make, &m.model, &li.name)?;
        Some(lens_profile::LensShot {
            profile,
            shot: lens_profile::Shot {
                focal: li.focal,
                aperture: li.aperture,
            },
        })
    });
    let mut edits = Edits::default();
    edits.lens_correction.distortion = true;
    edits.lens_correction.tca = true;
    edits.lens_correction.vignetting = true;

    let drawn: Vec<Guide> = at
        .iter()
        .map(|&x| Guide {
            from: [x, 0.0],
            to: [x, 1.0],
        })
        .collect();
    let (luma, w, h) =
        perspective_detect::analysis_luma(&decoded.image, &edits, lens.clone(), Resampler::default());
    let out = perspective_detect::refine_guides(
        &decoded.image,
        &edits,
        lens,
        Resampler::default(),
        &drawn,
    );

    let mut rgb = vec![0u8; w * h * 3];
    for (i, &l) in luma.iter().enumerate() {
        let v = (color::linear_to_srgb(l) * 255.0).round().clamp(0.0, 255.0) as u8;
        rgb[i * 3..i * 3 + 3].copy_from_slice(&[v, v, v]);
    }
    println!("{}: {w}x{h}", p.display());
    for (d, r) in drawn.iter().zip(&out) {
        // ⚠ The same metric `refine` uses: the largest gap at either end, as a fraction of width.
        let gap = (r.from[0] - d.from[0]).abs().max((r.to[0] - d.to[0]).abs());
        println!(
            "  drawn {:.4} -> refined {:.4}..{:.4}   moved {:.4} of frame width{}",
            d.from[0],
            r.from[0],
            r.to[0],
            gap,
            if r == d { "  (UNCHANGED)" } else { "" }
        );
        draw(&mut rgb, w, h, d, [255, 200, 40]);
        draw(&mut rgb, w, h, r, [40, 220, 120]);
    }
    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let outp = std::env::temp_dir().join(format!("refine_{stem}.png"));
    match image::save_buffer(&outp, &rgb, w as u32, h as u32, image::ColorType::Rgb8) {
        Ok(()) => println!("  wrote {}", outp.display()),
        Err(e) => eprintln!("  could not write {}: {e}", outp.display()),
    }
}
```

⚠ Amber is the line as drawn, green the refined one, so the move is visible on the PNG. A guide that
did not move prints `(UNCHANGED)` and its two colours coincide.

- [ ] **Step 2: Measure on the abbey and Walhalla**

Read the building's true edges off the PNGs already produced (`/tmp/final_abbey.png`,
`/tmp/final_walhalla.png`), pick guide positions a person would plausibly draw — within a few percent
— and record **how far each had to move to reach the right edge**.

- [ ] **Step 3: Choose the radius from that, and record the numbers in its doc comment**

⚠ Prefer the **smaller** value where the evidence is flat: a radius too small leaves a guide where
you drew it, which is recoverable and visible; one too large silently captures the wrong edge, which
is the failure this whole design exists to prevent.

- [ ] **Step 4: Commit**

---

## Task 6: verify, and hand the photographs back

- [ ] **Step 1: All three gates, each exit code read from a file**

```bash
cargo fmt --check > /tmp/gr-fmt.log 2>&1; echo "FMT=$?"
cargo clippy --all-targets --all-features -- -D warnings > /tmp/gr-c6.log 2>&1; echo "CLIPPY=$?"
cargo test --all > /tmp/gr-t6.log 2>&1; echo "TEST=$?"
grep "^test result:" /tmp/gr-t6.log | awk '{p+=$4; f+=$6} END {print "passed:",p," failed:",f}'
grep -c "^test golden" /tmp/gr-t6.log
```

⚠ All three together. `cargo test` being green is **not** evidence about clippy — eight recorded
instances, one of them this session.

- [ ] **Step 2: Goldens untouched**

```bash
git diff --stat main -- crates/pipeline/tests
```

Expected **empty**, and 53 golden tests must have **run**.

- [ ] **Step 3: Test count arithmetic.** 1888 + 4 (Task 2) + 1 (Task 4) + any added by Task 4's
  sabotage 2. State the sum.

- [ ] **Step 4: The GUI**

```bash
cargo run --release -p app
```

Open the abbey, Geometry, Guide Mode. Draw two rough guides down the building's edges. The button
should now read **Refine guides**; press it and they should land on the real edges. Then draw a guide
in open sky and refine again — it must **stay put** and the status line must say so.

⚠ The agent cannot drive the GUI (`AXIsProcessTrusted()` is false), so this step is the user's.

- [ ] **Step 5: Produce the before/after and STOP**

⚠⚠ **M7.4's "Done when" is a judgement about a photograph.** §6 of `PARALLEL-WORK.md`: *"An agent can
produce the comparison; it should not sign it off."*

- [ ] **Step 6: Report** — the measured radius and what it was chosen from; the test arithmetic; that
  the 53 goldens ran and are byte-identical; **any sabotage that did not fail, named as such**; and
  whether Auto's own two picks are still wrong on the two known frames, which this change does not
  claim to fix.

---

## Not in scope

The rectangle-constrained re-detect (spec §9.5); `MIN_SEPARATION`, `MIN_EDGE_LEVELS` and
`MIN_RUN_FRACTION` all unchanged; horizontal-family detection.
