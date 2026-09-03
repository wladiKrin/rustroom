# M5.3b Plan 2 — Visualize Spots

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A high-contrast edge view of the canvas that makes sensor dust findable, so the retouch tool can be used for the job it exists for.

**Architecture:** A view-only post-step in `render_engine::pack`, exactly where the display-space
transform goes and for the same reason — as a pipeline stage it would make the histogram
meaningless. `FrameSpec` carries the threshold; nothing reaches `Edits`, the sidecar, an export or
the CLI.

**Tech Stack:** Rust workspace; the existing `pipeline::spatial::gaussian_blur`.

**Spec:** `docs/superpowers/specs/2026-09-03-spot-removal-canvas-design.md` §6

**Depends on:** Plan 1 (`docs/superpowers/plans/2026-09-03-spot-removal-placement.md`) — the
Retouch panel is where the controls live.

---

## Ground rules

Same as Plan 1: the `PKG_CONFIG_PATH` export, `cargo fmt` + `clippy -D warnings` (check the exit
code) before every commit, no `Co-Authored-By` trailer, and **all 53 goldens byte-identical**.

---

## File structure

| File | Responsibility |
| --- | --- |
| `crates/app/src/visualize.rs` | **create** — the edge render, pure, over a plane |
| `crates/app/src/lib.rs` | **modify** — declare it |
| `crates/app/src/render_engine.rs` | **modify** — `FrameSpec.visualize`, applied in `pack` |
| `crates/app/src/main.rs` | **modify** — `RustroomApp.visualize_spots: Option<f32>` |
| `crates/app/src/develop/panel/retouch.rs` | **modify** — the checkbox and threshold slider |
| `crates/app/src/develop/canvas/mod.rs` | **modify** — pass it into the `FrameSpec` |

---

## Task 1: The edge render

**Files:**
- Create: `crates/app/src/visualize.rs`
- Modify: `crates/app/src/lib.rs`

- [ ] **Step 1: Declare the module**

In `crates/app/src/lib.rs`:

```rust
pub mod visualize;
```

- [ ] **Step 2: Write the failing tests**

Create `crates/app/src/visualize.rs` with only the test module:

```rust
//! The Visualize Spots view: a high-contrast edge render that makes sensor dust findable.
//!
//! ⚠ A **view mode, not an edit**. It never reaches `Edits`, the sidecar, an export or the CLI.

#[cfg(test)]
mod tests {
    use super::*;

    /// A flat plane with one small dark disc — a dust spot.
    fn dusty(w: usize, h: usize) -> Vec<f32> {
        let mut p = vec![0.6f32; w * h];
        let (cx, cy, r) = (w as f32 / 2.0, h as f32 / 2.0, 4.0);
        for y in 0..h {
            for x in 0..w {
                let d = ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt();
                if d < r {
                    p[y * w + x] = 0.3;
                }
            }
        }
        p
    }

    /// ⚠⚠ The whole point: the spot must stand out from the flat field far more than it does in
    /// the original. A test that only checked "the output differs" would pass against any
    /// transform at all, including one that made dust *less* visible.
    #[test]
    fn a_dust_spot_stands_out_further_than_it_did() {
        let (w, h) = (64, 64);
        let src = dusty(w, h);
        let out = edge_view(&src, w, h, DEFAULT_THRESHOLD);
        let at = |p: &[f32], x: usize, y: usize| p[y * w + x];
        // Contrast between the spot's rim and the flat field, before and after.
        let before = (at(&src, 32, 32) - at(&src, 8, 8)).abs();
        let after = (at(&out, 32, 32) - at(&out, 8, 8)).abs();
        assert!(after > before * 1.5, "before={before} after={after}");
    }

    /// ⚠ A flat field must come back flat and near black, or every photograph would look like it
    /// were covered in dust.
    #[test]
    fn a_flat_field_is_flat_and_dark() {
        let out = edge_view(&vec![0.42; 32 * 32], 32, 32, DEFAULT_THRESHOLD);
        let max = out.iter().copied().fold(0.0f32, f32::max);
        assert!(max < 0.05, "a flat field lit up: {max}");
    }

    /// ⚠ The threshold is a **gain**, so raising it shows more. A slider mapped to a hard cutoff
    /// would show nothing over most of its travel.
    #[test]
    fn a_higher_threshold_shows_more() {
        let (w, h) = (64, 64);
        let src = dusty(w, h);
        let low: f32 = edge_view(&src, w, h, 4.0).iter().sum();
        let high: f32 = edge_view(&src, w, h, 40.0).iter().sum();
        assert!(high > low * 1.5, "low={low} high={high}");
    }

    /// ⚠ Total: the output is always in `[0, 1]`, whatever the gain, or `to_rgb8` would clip
    /// arbitrarily and the view would read as solid white.
    #[test]
    fn the_output_stays_in_range_at_any_threshold() {
        let src = dusty(32, 32);
        for t in [0.0, 1.0, 50.0, 1e6, f32::INFINITY] {
            let out = edge_view(&src, 32, 32, t);
            assert!(
                out.iter().all(|v| (0.0..=1.0).contains(v)),
                "threshold {t} left the range"
            );
        }
    }

    /// ⚠ Degenerate sizes must not panic — the canvas can ask for a zero-width region.
    #[test]
    fn an_empty_plane_is_handled() {
        assert!(edge_view(&[], 0, 0, DEFAULT_THRESHOLD).is_empty());
    }
}
```

- [ ] **Step 3: Run to verify they fail**

```bash
cargo test -p app --lib visualize 2>&1 | grep -E "error\[E0|test result"
```
Expected: FAIL to compile — `cannot find function edge_view`.

- [ ] **Step 4: Implement**

Prepend to `crates/app/src/visualize.rs`:

```rust
/// The blur radius the high-pass is taken against, in **rendered** pixels.
///
/// ⚠ Rendered, not scene, pixels — and deliberately **not** scaled by the render scale. The point
/// is to make dust visible on screen, and dust is a few pixels across wherever you are looking
/// from. Unlike the spots themselves, which are normalised, this is a screen-space effect.
pub const SIGMA: f32 = 6.0;

/// The default threshold gain.
pub const DEFAULT_THRESHOLD: f32 = 12.0;
/// The slider's range.
pub const MIN_THRESHOLD: f32 = 1.0;
pub const MAX_THRESHOLD: f32 = 50.0;

/// A high-contrast edge view of one luminance plane.
///
/// `|luma − blur(luma)|`, gain-mapped by `threshold` and clamped. Dust and sensor spots read as
/// bright rings against flat grey.
///
/// ⚠ The threshold is a **gain**, not a cutoff: a cutoff would show nothing at all over most of a
/// slider's travel, which makes it impossible to judge by eye.
pub fn edge_view(luma: &[f32], w: usize, h: usize, threshold: f32) -> Vec<f32> {
    if w == 0 || h == 0 || luma.len() != w * h {
        return Vec::new();
    }
    let gain = if threshold.is_finite() {
        threshold.clamp(MIN_THRESHOLD, MAX_THRESHOLD)
    } else {
        MAX_THRESHOLD
    };
    let base = pipeline::spatial::gaussian_blur(luma, w, h, SIGMA);
    luma.iter()
        .zip(&base)
        .map(|(v, b)| ((v - b).abs() * gain).clamp(0.0, 1.0))
        .collect()
}
```

⚠ Check that `pipeline::spatial` is public and `gaussian_blur` reachable from `app`:
`grep -n "pub mod spatial" crates/pipeline/src/lib.rs`. If it is private, make it `pub` in the same
commit and say so in the message — that is a deliberate, minimal visibility change, the way
`range_weights` was made `pub` for the grading strip.

- [ ] **Step 5: Run the tests**

```bash
cargo test -p app --lib visualize 2>&1 | grep -E "test result|panicked|assertion"
```
Expected: PASS, 5 tests.

- [ ] **Step 6: Sabotage — a cutoff instead of a gain**

Replace the map with `.map(|(v, b)| if (v - b).abs() > 0.02 { 1.0 } else { 0.0 })` and re-run.

Expected: `a_higher_threshold_shows_more` **FAILS**. Restore.

- [ ] **Step 7: Commit**

```bash
cargo fmt --all
cargo clippy --all --all-targets --all-features -- -D warnings; echo "CLIPPY=$?"
git add crates/app/src/visualize.rs crates/app/src/lib.rs
git commit -m "feat(app): a high-contrast edge view for finding dust

|luma - blur(luma)|, gain-mapped. Reuses spatial::gaussian_blur rather than
introducing an algorithm.

⚠ The threshold is a gain, not a cutoff: a cutoff shows nothing over most of a
slider's travel, which makes it impossible to judge by eye.

⚠ Sigma is in RENDERED pixels and deliberately does not take the render scale.
The point is to make dust visible on screen, and dust is a few pixels across
wherever you are looking from."
```

---

## Task 2: `FrameSpec.visualize`, applied in `pack`

**Files:**
- Modify: `crates/app/src/render_engine.rs`

- [ ] **Step 1: Add the field**

In `crates/app/src/render_engine.rs`, on `FrameSpec`:

```rust
    /// The Visualize Spots threshold, or `None` when the view is off.
    ///
    /// ⚠⚠ Applied in `pack`, *after* the pipeline — **never as a pipeline stage**, and the reason
    /// is recorded rather than reasoned: the display transform went into `pack` because a stage
    /// would make the **histogram** display-referred, and `pack` already takes the histogram from
    /// its sRGB output *before* the post-step. This is the same shape and inherits that ordering.
    /// As a stage it would also want a place in the cache key for something that is not an edit.
    ///
    /// ⚠ A view-only property of the *request*, like `display_space` — it never reaches `Edits`,
    /// the sidecar, an export or the CLI, and toggling it invalidates no checkpoint, because
    /// `CacheKey` is `(PrefixKey, max_dim)`.
    pub visualize: Option<f32>,
```

- [ ] **Step 2: Fill in every construction site**

```bash
cargo build --all --all-targets 2>&1 | grep -E "^error\[E0063\]" -A 4 | head -20
```
Add `visualize: None,` to each `FrameSpec { .. }` literal, including `FrameSpec::fit`.

- [ ] **Step 3: Write the failing test**

In `crates/app/src/render_engine.rs`'s test module:

```rust
    /// ⚠⚠ The histogram must be **unaffected** by Visualize Spots, for the reason the display
    /// transform records: `pack` takes the histogram from its sRGB output *before* the post-step,
    /// and a view mode that poisoned it would make the histogram lie about the photograph. This
    /// is the test that the ordering was inherited rather than reinvented.
    #[test]
    fn the_histogram_is_unaffected_by_visualize_spots() {
        let buf = std::sync::Arc::new(gradient(32, 32));
        let plain = render_once_spec(&buf, FrameSpec { visualize: None, ..FrameSpec::fit(32) });
        let vis = render_once_spec(&buf, FrameSpec { visualize: Some(12.0), ..FrameSpec::fit(32) });
        assert_eq!(plain.histogram, vis.histogram, "the view mode reached the histogram");
        assert_ne!(plain.rgba, vis.rgba, "the view mode did not reach the pixels");
    }
```

⚠ `gradient` and `render_once_spec` are placeholders for whatever the existing tests in that module
use to build a buffer and render a `FrameSpec` — **copy the shape of
`the_histogram_is_unaffected_by_the_display_space`**, which is the exact precedent and already
exists. `grep -n "the_histogram_is_unaffected" crates/app/src/render_engine.rs`.

- [ ] **Step 4: Run to verify it fails**

```bash
cargo test -p app --lib visualize_spots 2>&1 | grep -E "test result|panicked|assertion"
```
Expected: FAIL — `the view mode did not reach the pixels` (the field is carried but unused).

- [ ] **Step 5: Apply it in `pack`**

In `pack`, **after** the histogram is taken and **before** the display transform:

```rust
    // ⚠⚠ After the histogram, before the display transform. The histogram must describe the
    // photograph, not the view mode — the rule `display_space` established one line below.
    if let Some(threshold) = visualize {
        let luma: Vec<f32> = px
            .data()
            .as_chunks::<3>()
            .0
            .iter()
            .map(|p| color::luminance(*p))
            .collect();
        let edges = rustroom_app::visualize::edge_view(&luma, px.width(), px.height(), threshold);
        for (p, e) in px.data_mut().as_chunks_mut::<3>().0.iter_mut().zip(&edges) {
            *p = [*e, *e, *e];
        }
    }
```

⚠ Adapt to `pack`'s real local names and to whether `ImageBuf` exposes `data_mut` — check with
`grep -n "pub fn data_mut\|pub fn data" crates/core-image/src/lib.rs`. If it does not, build a new
`ImageBuf` with `ImageBuf::from_vec` instead of mutating in place. The **ordering** relative to the
histogram is the part that must not change.

- [ ] **Step 6: Run the tests**

```bash
cargo test -p app --lib 2>&1 | grep -E "test result|panicked" | head
```
Expected: PASS.

- [ ] **Step 7: Sabotage — take the histogram after the view mode**

Move the histogram line below the `if let Some(threshold)` block and re-run.

Expected: `the_histogram_is_unaffected_by_visualize_spots` **FAILS**. Restore.

- [ ] **Step 8: Commit**

```bash
cargo fmt --all
cargo clippy --all --all-targets --all-features -- -D warnings; echo "CLIPPY=$?"
git add crates/app/src/render_engine.rs
git commit -m "feat(app): carry the Visualize Spots threshold on FrameSpec

⚠⚠ Applied in pack, after the histogram and before the display transform —
never as a pipeline stage. That is the rule display_space established: a stage
would make the histogram describe the view mode rather than the photograph, and
would want a place in the cache key for something that is not an edit at all.
Sabotage-verified by moving the histogram below it."
```

---

## Task 3: The panel controls

**Files:**
- Modify: `crates/app/src/main.rs`
- Modify: `crates/app/src/develop/panel/retouch.rs`
- Modify: `crates/app/src/develop/canvas/mod.rs`

- [ ] **Step 1: Add the app state**

In `crates/app/src/main.rs`, on `RustroomApp`:

```rust
    /// The Visualize Spots threshold while the view is on.
    ///
    /// ⚠ On `RustroomApp`, not `Session`: it is a property of how you are *working*, not of the
    /// photograph, so it survives a photo switch the way the tool selection does.
    visualize_spots: Option<f32>,
```

Add `visualize_spots: None,` to every `RustroomApp { .. }` literal the compiler names, including
`test_app.rs`'s exhaustive one.

- [ ] **Step 2: Turn it off when the tool changes**

Find where the panel detects a tool change (`grep -n "app.tool" crates/app/src/develop/panel/mod.rs`
and the frame loop's tool-change edge check). Add:

```rust
        // ⚠ A canvas stuck in a high-contrast grey view after switching to Edit reads as a broken
        // render, and it is the single most likely thing here to be reported as a bug.
        if tool != Tool::Retouch {
            app.visualize_spots = None;
        }
```

- [ ] **Step 3: Write the failing test**

In `crates/app/src/develop/panel/retouch.rs`'s test module:

```rust
    /// ⚠ It must not survive leaving the tool: a canvas stuck in the grey edge view after
    /// switching to Edit reads as a broken render.
    #[test]
    fn visualize_turns_itself_off_when_the_tool_changes() {
        let ctx = crate::test_ui::ctx();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 64, 64);
        app.tool = rustroom_app::tool::Tool::Retouch;
        app.visualize_spots = Some(12.0);
        app.tool = rustroom_app::tool::Tool::Edit;
        crate::test_ui::click(&ctx, "Auto", |ui| {
            crate::develop::panel::show(&mut app, ui);
        });
        assert_eq!(app.visualize_spots, None, "the view mode outlived its tool");
    }
```

⚠ "Auto" is a label that exists under `Tool::Edit`; the click is only a way to run one panel frame.
If the panel's structure makes that awkward, drive one frame with `ctx.run_ui` instead — copy
`merely_drawing_the_panel_changes_nothing`'s shape from Plan 1 Task 6.

- [ ] **Step 4: Run to verify it fails**

```bash
cargo test -p app --bin Rustroom visualize_turns 2>&1 | grep -E "test result|panicked"
```
Expected: FAIL — `the view mode outlived its tool`.

- [ ] **Step 5: Add the controls**

In `crates/app/src/develop/panel/retouch.rs`, extend `show`'s signature with
`visualize: &mut Option<f32>` and add, after the mode selector:

```rust
    ui.separator();
    let mut on = visualize.is_some();
    if ui
        .checkbox(&mut on, "Visualize spots")
        .on_hover_text("A high-contrast view that makes sensor dust visible. Affects only what you see — never the render, the export or the histogram.")
        .changed()
    {
        *visualize = on.then_some(rustroom_app::visualize::DEFAULT_THRESHOLD);
    }
    if let Some(t) = visualize.as_mut() {
        ui.add(
            egui::Slider::new(
                t,
                rustroom_app::visualize::MIN_THRESHOLD..=rustroom_app::visualize::MAX_THRESHOLD,
            )
            .text("Threshold"),
        )
        .on_hover_text("Higher shows more. It is a gain, not a cutoff, so it always shows something.");
    }
```

Pass `&mut app.visualize_spots` from `panel/mod.rs`.

- [ ] **Step 6: Pass it into the render request**

In `crates/app/src/develop/canvas/mod.rs`, where the `FrameSpec` is built, set
`visualize: app.visualize_spots,`.

⚠ Toggling it must trigger a re-render. Check how the canvas decides to request — if it compares
the spec against the last one, the new field is covered automatically; if not, add
`app.visualize_spots` to whatever change detection exists, and say which in the commit.

- [ ] **Step 7: Run everything**

```bash
cargo fmt --all
cargo clippy --all --all-targets --all-features -- -D warnings; echo "CLIPPY=$?"
cargo test --all 2>&1 | grep -c "test result: FAILED"
git status --porcelain crates/pipeline/tests/golden/ | wc -l
```
Expected: clippy 0, zero FAILED, zero dirty goldens.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat(develop): a Visualize Spots toggle in the Retouch panel

⚠ It turns itself off when the tool changes. A canvas stuck in a high-contrast
grey view after switching to Edit reads as a broken render, and is the single
most likely thing here to be reported as a bug.

⚠ On RustroomApp rather than Session: it is a property of how you are working,
not of the photograph."
```

---

## Task 4: Verify by hand

- [ ] **Step 1: Full green**

```bash
cargo fmt --all --check; echo "FMT=$?"
cargo clippy --all --all-targets --all-features -- -D warnings; echo "CLIPPY=$?"
cargo test --all 2>&1 | grep -E "test result:" | awk -F'[ ;]' '{p+=$4; f+=$6} END {print "passed="p" failed="f}'
git status --porcelain crates/pipeline/tests/golden/ | wc -l
```
Expected: FMT=0, CLIPPY=0, failed=0, goldens dirty=0.

- [ ] **Step 2: Run the GUI**

```bash
cargo run --release -p app
```
Open a RAW with visible sky, press `4`, and check:
1. Ticking **Visualize spots** turns the canvas into a grey edge view within a frame or two.
2. The **histogram does not change** when it is ticked — that is the whole architectural point.
3. Raising **Threshold** shows more; lowering it shows less; neither extreme is blank or solid.
4. Placing a spot while Visualize is on still works, and the patch is visible in the edge view.
5. Switching to **Edit** turns it off, and the photograph comes back.
6. **Export while it is on** — the file must be the normal render, not the grey view.

- [ ] **Step 3: Report each check as pass or fail with what was seen.**

Do not claim it works without having run it.
