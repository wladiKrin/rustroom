# Composition Guides as a View Feature — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The compositional guides draw under every Develop tool instead of only under Crop, so the existing **View → Composition guide** entry and the `O` key mean what they say.

**Architecture:** One call moves. `draw_guides` is invoked from inside `draw_crop_overlay`, which the canvas calls only under `Tool::Crop`. It moves out into `canvas::show`, where the Crop tool keeps drawing it against the **crop rect** and every other tool draws it against **`frame_rect`, clipped through `overlay_clip(img_rect)`**. Guide Mode draws none.

**Tech Stack:** Rust, egui 0.35. Spec: `docs/superpowers/specs/2026-09-01-global-composition-guides-design.md`.

**Build environment — nothing compiles without this:**
```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

**⚠ Verification discipline.** This shell is **zsh**, where `${PIPESTATUS[0]}` expands to nothing and a check printing `EXIT: ` reads as a pass. Redirect and test `$?`:
```bash
cargo fmt --all --check > /tmp/f.log 2>&1; echo "FMT=$?"
cargo clippy --all-targets --all-features -- -D warnings > /tmp/c.log 2>&1; echo "CLIPPY=$?"
cargo test --all > /tmp/t.log 2>&1; echo "TEST=$?"
awk '/^test result:/{p+=$4; f+=$6} END{print "passed="p" failed="f}' /tmp/t.log
```
Baseline is **1451 passed, 0 failed**. **No golden may move** — `git status crates/pipeline/` must stay empty throughout; this change touches no pipeline code at all.

**⚠⚠ Sabotage discipline.** After writing a test, break the thing it names and watch it fail — and confirm it **ran**: a filter matching no test prints `test result: ok. 0 passed; 0 failed; N filtered out`, which reads as success. Assert `ran = passed + failed >= 1`. ⚠ `canvas` is a **bin** module, so use `cargo test -p app --bin Rustroom <name>`; `--lib` cannot see it and will report zero tests having run.

**Conventions:** no `Co-Authored-By` trailer; **never push**; commit at the end of each task.

---

## File Structure

| File | Change |
| --- | --- |
| `crates/app/src/develop/canvas/crop_overlay.rs` | `draw_guides` becomes `pub(super)`; the call inside `draw_crop_overlay` is removed; `draw_crop_overlay` loses its `guide` parameter |
| `crates/app/src/develop/canvas/mod.rs` | draws the guides after the tool match, choosing the rect per tool |

No other file changes. `Settings.composition_guide`, `Command::CycleCompositionGuide`, the `O` binding, the View menu entry and `composition_guides::segments` are all already built and are untouched.

---

## Task 1: Draw the guides under every tool

**Files:**
- Modify: `crates/app/src/develop/canvas/crop_overlay.rs`
- Modify: `crates/app/src/develop/canvas/mod.rs`

- [ ] **Step 1: Write the failing tests**

Add to `crates/app/src/develop/canvas/mod.rs`'s `mod tests`:

```rust
/// ⚠⚠ **The defect this fixes, and the only test shape that can see it.** The guides were drawn
/// from inside `draw_crop_overlay`, which the canvas calls only under `Tool::Crop` — so the
/// View menu entry and the `O` key did nothing in three of the four tools. A test that checks
/// one tool cannot distinguish "works everywhere" from "works in the tool I happened to pick".
/// Same argument as `the_strip_is_present_under_every_tool`.
#[test]
fn the_composition_guides_are_drawn_under_every_tool() {
    for tool in Tool::ALL {
        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();
        test_app::open_test_photo(&mut app, &ctx, 400, 300);
        app.settings.composition_guide = CompositionGuide::Thirds;
        app.tool = tool;
        assert!(
            guide_lines_drawn(&mut app, &ctx) > 0,
            "no composition guides under {tool:?}"
        );
    }
}

/// ⚠ The deliberate exception. Perspective Guide Mode strips the crop from the preview *and*
/// draws no crop rect, so a guide there would divide a rectangle the user is not going to keep —
/// and a guide that divides the wrong rectangle is worse than none, because it is trusted.
#[test]
fn guide_mode_draws_no_composition_guides() {
    let ctx = test_ui::ctx();
    let mut app = test_app::for_test();
    test_app::open_test_photo(&mut app, &ctx, 400, 300);
    app.settings.composition_guide = CompositionGuide::Thirds;
    app.tool = Tool::Crop;
    app.session.as_mut().expect("a session").show_guides = true;
    assert_eq!(
        guide_lines_drawn(&mut app, &ctx),
        0,
        "Guide Mode must draw no composition guides"
    );
}

/// ⚠ And `Off` must draw none anywhere, or the counter above is measuring something else and
/// every assertion built on it is meaningless.
#[test]
fn the_off_setting_draws_no_composition_guides() {
    let ctx = test_ui::ctx();
    let mut app = test_app::for_test();
    test_app::open_test_photo(&mut app, &ctx, 400, 300);
    app.settings.composition_guide = CompositionGuide::Off;
    app.tool = Tool::Edit;
    assert_eq!(guide_lines_drawn(&mut app, &ctx), 0);
}

/// Run one canvas frame and count the line segments it painted.
///
/// ⚠ Counts `Shape::LineSegment` rather than looking for a widget: guides are *painted*, so
/// `test_ui` cannot find them — it only sees interactive widgets through AccessKit.
fn guide_lines_drawn(app: &mut RustroomApp, ctx: &egui::Context) -> usize {
    let before = painted_segments(app, ctx, CompositionGuide::Off);
    let after = painted_segments(app, ctx, app.settings.composition_guide);
    after.saturating_sub(before)
}

/// Paint one frame with `guide` and return how many line segments landed.
fn painted_segments(
    app: &mut RustroomApp,
    ctx: &egui::Context,
    guide: CompositionGuide,
) -> usize {
    let was = app.settings.composition_guide;
    app.settings.composition_guide = guide;
    let out = ctx.run(egui::RawInput {
        screen_rect: Some(test_ui::screen()),
        ..Default::default()
    }, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            super::show(app, ui, ctx);
        });
    });
    app.settings.composition_guide = was;
    let mut n = 0;
    for cs in out.shapes {
        count_segments(&cs.shape, &mut n);
    }
    n
}

fn count_segments(shape: &egui::Shape, n: &mut usize) {
    match shape {
        egui::Shape::LineSegment { .. } => *n += 1,
        egui::Shape::Vec(v) => {
            for s in v {
                count_segments(s, n);
            }
        }
        _ => {}
    }
}
```

Add these imports at the top of that `mod tests`:

```rust
use rustroom_app::composition_guides::CompositionGuide;
use rustroom_app::tool::Tool;
```

- [ ] **Step 2: Run them to verify they fail**

```bash
cargo test -p app --bin Rustroom the_composition_guides_are_drawn_under_every_tool > /tmp/t1.log 2>&1; echo "EXIT=$?"
awk '/^test result:/{print "ran="$4+$6" passed="$4" failed="$6}' /tmp/t1.log
```
Expected: `ran=1 failed=1`, with the message `no composition guides under Edit`. ⚠ If it reports `ran=0`, the filter matched nothing — check the target is `--bin Rustroom`, not `--lib`.

- [ ] **Step 3: Make `draw_guides` reachable and drop the call from the crop overlay**

In `crates/app/src/develop/canvas/crop_overlay.rs`:

Change the signature to `pub(super)`:

```rust
pub(super) fn draw_guides(painter: &egui::Painter, rect: egui::Rect, guide: CompositionGuide) {
```

Delete the call and replace its comment (it currently sits just above `marks.rect_stroke`):

```rust
    // ⚠⚠ The guides are **no longer drawn here.** They moved to `canvas::show` so that every tool
    // gets them, not only Crop — the View menu entry and the `O` key did nothing in the other
    // three. The Crop tool still passes the **crop rect**, because the preview strips the crop and
    // that rect is the photograph-to-be; see the spec's one rule.
    marks.rect_stroke(
```

Remove the now-unused `guide` parameter from `draw_crop_overlay`:

```rust
pub(super) fn draw_crop_overlay(
    ui: &egui::Ui,
    s: &mut Session,
    resp: &egui::Response,
    img_rect: egui::Rect,
    crop_aspect: Option<f32>,
) -> bool {
```

- [ ] **Step 4: Draw them from the canvas**

In `crates/app/src/develop/canvas/mod.rs`, drop `app_guide` from the `draw_crop_overlay` call:

```rust
                                photo_edited |= draw_crop_overlay(
                                    ui,
                                    s,
                                    &resp,
                                    img_rect,
                                    app.crop_aspect,
                                );
```

Then, immediately **after** the `match tool { … }` block closes, add:

```rust
                    // ⚠⚠ **One rule: the guides divide what will be the final photograph.**
                    // Under Crop the preview strips the crop, so the crop rect *is* that
                    // photograph; everywhere else the crop is applied, so the frame is. It only
                    // looks like two behaviours because Crop is the one place the two differ.
                    //
                    // ⚠ Guide Mode is the exception: there the preview strips the crop **and** no
                    // crop rect is drawn, so a guide would divide a rectangle nobody keeps. A
                    // guide that divides the wrong rectangle is worse than no guide, because it
                    // gets trusted.
                    //
                    // ⚠ `frame_rect`, never `img_rect`, for the reason the mask and perspective
                    // overlays both record: once a region render is on, `img_rect` covers only the
                    // rendered *slice*. Guides must divide the whole photograph. `img_rect`
                    // survives as the clip — widened by `overlay_clip`, because a diagonal reaches
                    // the corners and a tight clip shaves half the stroke off there.
                    let guide_rect = match tool {
                        Tool::Crop if s.show_guides => None,
                        Tool::Crop => Some(crop_overlay::crop_screen_rect(s, img_rect)),
                        _ => Some(frame_rect),
                    };
                    if let Some(r) = guide_rect {
                        let p = ui.painter_at(canvas_geom::overlay_clip(img_rect));
                        crop_overlay::draw_guides(&p, r, app_guide);
                    }
```

- [ ] **Step 5: Expose the crop rect the overlay already computes**

`draw_crop_overlay` computes the crop's screen rect at `crop_overlay.rs:97` via a local
`to_screen` closure. Extract it so the canvas places the guides against the *same* rect the overlay
draws, rather than a second copy — two computations of one rectangle is how they drift.

In `crop_overlay.rs`, add above `draw_crop_overlay`:

```rust
/// The crop rectangle's position on screen.
///
/// ⚠ Extracted so `canvas::show` places the guides against the **same** rect the overlay draws.
/// Two computations of one rectangle is exactly how the crop border and the guides would come to
/// disagree by a pixel, with nothing to say which was right.
pub(super) fn crop_screen_rect(s: &Session, img_rect: egui::Rect) -> egui::Rect {
    let g = s.edits.geometry.crop;
    let to_screen = |nx: f32, ny: f32| {
        egui::pos2(
            img_rect.left() + nx * img_rect.width(),
            img_rect.top() + ny * img_rect.height(),
        )
    };
    egui::Rect::from_min_max(to_screen(g.left, g.top), to_screen(g.right, g.bottom))
}
```

Then in `draw_crop_overlay`, replace lines 97–98

```rust
    let crop_rect =
        egui::Rect::from_min_max(to_screen(g.left, g.top), to_screen(g.right, g.bottom));
```

with

```rust
    let crop_rect = crop_screen_rect(s, img_rect);
```

⚠ Leave the `to_screen` closure and the `let g = …` binding where they are — the handle hit-testing
below uses both, and removing them is a larger change than this task wants. `clippy` will not
complain: both still have callers.

⚠ The four pre-existing crop tests are what prove the extraction moved no behaviour. They must pass
**unchanged** — if one needs editing, the extraction is wrong.

- [ ] **Step 6: Run the tests**

```bash
cargo test --all > /tmp/t.log 2>&1; echo "TEST=$?"
awk '/^test result:/{p+=$4; f+=$6} END{print "passed="p" failed="f}' /tmp/t.log
```
Expected: PASS, count **1454** (1451 + 3).

- [ ] **Step 7: Sabotage — three, each confirmed to have run**

**(a) Restore the tool restriction.** Change the canvas's `guide_rect` match to `Tool::Crop => Some(...), _ => None`.
```bash
cargo test -p app --bin Rustroom the_composition_guides_are_drawn_under_every_tool > /tmp/s.log 2>&1; echo "EXIT=$?"
awk '/^test result:/{print "ran="$4+$6" failed="$6}' /tmp/s.log
```
Expected: `ran=1 failed=1`. Restore.

**(b) Draw in Guide Mode.** Delete the `Tool::Crop if s.show_guides => None` arm.
Expected: `guide_mode_draws_no_composition_guides` fails. Restore.

**(c) Substitute `img_rect` for `frame_rect`.** ⚠ This one is expected to be **inert** at fit resolution, where the two are equal — say so in your report rather than claiming a catch. It is the case a fit-resolution fixture structurally cannot see, and the reason the spec flags it.

- [ ] **Step 8: Full gate**

```bash
cargo fmt --all --check > /tmp/f.log 2>&1; echo "FMT=$?"
cargo clippy --all-targets --all-features -- -D warnings > /tmp/c.log 2>&1; echo "CLIPPY=$?"
cargo test --all > /tmp/t.log 2>&1; echo "TEST=$?"
awk '/^test result:/{p+=$4; f+=$6} END{print "passed="p" failed="f}' /tmp/t.log
git status --porcelain crates/pipeline/ | wc -l
```
Expected: `FMT=0`, `CLIPPY=0`, `TEST=0`, **1454 passed / 0 failed**, **0** pipeline files touched.

- [ ] **Step 9: Commit**

```bash
git add -A crates/app/src
git commit -m "feat(canvas): composition guides draw under every tool

They were drawn from inside draw_crop_overlay, which the canvas calls only
under Tool::Crop -- so the View menu entry and the O key did nothing in
Edit, Masks and Presets.

One rule: the guides divide what will be the final photograph. Under Crop
the preview strips the crop, so the crop rect is that photograph;
everywhere else the crop is applied, so the frame is.

Guide Mode is the exception. There the preview strips the crop and no crop
rect is drawn, so a guide would divide a rectangle nobody keeps -- and a
guide that divides the wrong rectangle is worse than none, because it gets
trusted."
```

---

## Task 2: Verify in the running app

**Files:** none.

- [ ] **Step 1: Launch**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo build --release -p app > /tmp/rel.log 2>&1; echo "BUILD=$?"
nohup ./target/release/Rustroom > /tmp/gui.log 2>&1 & disown
```

⚠ `nohup … & disown`, not a plain `&`: a backgrounded GUI gets SIGTERM'd when the task wrapper is torn down, which reads as the app having exited. Confirm with `pgrep -f "target/release/Rustroom"`.

- [ ] **Step 2: Check what tests cannot**

Press `O` in **Edit**, **Masks** and **Presets** and confirm the guides appear in each. Then in **Crop**, confirm they still track the crop rectangle as you drag it. Then turn on **Guide Mode** and confirm they vanish.

⚠ Then **zoom to 1:1 and pan**. This is the case no test here covers: the guides must divide the whole photograph and stay put relative to it, not re-divide the visible slice. If they jump or re-space while panning, `frame_rect` was substituted for `img_rect` somewhere.

⚠ And look at **Masks with a brush mask selected** — the accepted risk in the spec is clutter there, against a coverage tint, handle circles and a brush cursor. If it is too much, that is a finding worth reporting, not a bug to fix silently.

---

## Self-Review

**Spec coverage:** §1 the rule → Task 1 Steps 4–5. §1 Guide Mode exception → Step 4's match arm, tested in Step 1. §1 coordinates → Step 4's `frame_rect` + `overlay_clip`. §2 scope (all four tools) → Step 1's `Tool::ALL` loop. §3 what does not change → nothing in the plan touches `Settings`, `menu.rs`, `commands.rs` or `composition_guides.rs`. §4 testing, all three properties → Step 1 covers two; the third (zoomed) is **explicitly** handed to Task 2 Step 2 rather than faked, because the fixture renders at fit where `frame_rect == img_rect`. §5 caveats → no code.

**Placeholder scan:** none. Every step carries its code or its exact command.

**Type consistency:** `draw_guides(&egui::Painter, egui::Rect, CompositionGuide)` unchanged, only its visibility. `crop_screen_rect(&Session, egui::Rect) -> egui::Rect` is defined in Step 5 and used in Step 4. `draw_crop_overlay` loses exactly one parameter, and its one call site is updated in the same step.

**Counts:** 1451 → **1454**.
