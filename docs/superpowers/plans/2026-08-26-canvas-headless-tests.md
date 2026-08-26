# Canvas Headless Tests Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A reusable fixture that drives `crates/app`'s real canvas code headlessly, plus regression tests for the three bug shapes that have actually occurred there.

**Architecture:** Three independent pieces, landing in dependency order. A **request counter** on `RenderEngine` makes "a render was issued" observable. A new **`canvas_geom`** bin module holds three coordinate functions lifted out of the canvas, testable with no fixture at all. A **`test_app`** fixture constructs a full `RustroomApp` with a synthetic `Session` — no disk, no libraw, no GPU — so tests can drive `canvas::show` as it ships.

**Tech Stack:** Rust, `eframe`/`egui` 0.35, `egui::Context::run_ui` with `RawInput`.

**Spec:** `docs/superpowers/specs/2026-08-26-canvas-headless-tests-design.md`

**Build environment:**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

**Standing rules:** no `Co-Authored-By`; never push; sabotage every new test and **confirm it compiled**; clippy clean checked by **exit code**.

⚠ **Check exit codes, never grep counts** — `cargo test --all | grep -c "test result: FAILED"` returns 0 when the *build* fails:

```bash
cargo test --all > /tmp/t.log 2>&1; echo "exit: $?"
```

⚠ **The bin-crate dead-code trap, which this plan hits twice.** In a **binary** crate a `pub` item reaches nothing outside the crate, so an item with no caller is `dead_code` and fails `clippy -D warnings`. Consequences here: **Task 2's functions must replace their inline originals in the same commit**, and **Task 3's fixture must land with its first test**. This trap has bitten this project seven times; do not split those commits.

⚠ **`#[cfg(test)]` does not cross the lib/bin boundary.** `render_engine` is in the **lib**; `canvas.rs` is a **bin** module. When the bin's test target builds, the lib is a dependency compiled *without* `cfg(test)`. Task 1's counter is therefore unconditional.

---

## File Structure

| File | Responsibility |
|---|---|
| `crates/app/src/render_engine.rs` (modify) | Gains an unconditional request counter |
| `crates/app/src/develop/canvas_geom.rs` (create) | Three pure coordinate functions + their tests |
| `crates/app/src/develop/mod.rs` (modify) | Declares `canvas_geom` |
| `crates/app/src/develop/canvas.rs` (modify) | Calls `canvas_geom`; gains the fixture-driven tests |
| `crates/app/src/test_app.rs` (create) | `#[cfg(test)]` fixture: a full `RustroomApp` + synthetic `Session` |
| `crates/app/src/main.rs` (modify) | Declares `test_app` |

---

### Task 1: A request counter on `RenderEngine`

**Files:**
- Modify: `crates/app/src/render_engine.rs`

- [ ] **Step 1: Write the failing test**

Add to `render_engine.rs`'s existing `#[cfg(test)] mod tests`:

```rust
#[test]
fn requests_are_counted() {
    // ⚠⚠ This counter exists because "a drag mutated `Edits` but never asked for a render" is
    // invisible from outside — the request goes into a channel and vanishes. That bug shipped
    // twice: the mask overlay kept showing the previous position until some unrelated edit
    // happened to request a frame.
    //
    // ⚠ The alternative was asserting on `Session::requested_region`, which is a *proxy* that
    // misses exactly the whole-image fit case where the bug actually occurred.
    let buf = std::sync::Arc::new(core_image::ImageBuf::new(4, 2));
    let engine = RenderEngine::new(buf, 1.0, None, || {});
    assert_eq!(engine.requests_issued(), 0, "nothing asked for yet");

    engine.request(
        edit_model::Edits::default(),
        FrameSpec {
            max_dim: 64,
            region: None,
            display_space: color::OutputSpace::Srgb,
        },
    );
    assert_eq!(engine.requests_issued(), 1);

    engine.request(
        edit_model::Edits::default(),
        FrameSpec {
            max_dim: 64,
            region: None,
            display_space: color::OutputSpace::Srgb,
        },
    );
    assert_eq!(engine.requests_issued(), 2, "each request counts");
}
```

- [ ] **Step 2: Run to verify it fails**

```bash
cd ~/Projects/rustroom
cargo test -p app --lib requests_are_counted 2>&1 | grep -E "^error" | head -3
```

Expected: `no method named requests_issued found`.

- [ ] **Step 3: Implement**

In `render_engine.rs`, add to the `RenderEngine` struct definition:

```rust
    /// How many renders have been asked for over this engine's life.
    ///
    /// ⚠⚠ **Unconditional, not `#[cfg(test)]`, and that is forced rather than sloppy.** This
    /// type lives in the **lib**; its only test-time consumer (`develop/canvas.rs`) lives in the
    /// **bin**, and when the bin's test target builds, the lib is a dependency compiled *without*
    /// `cfg(test)` — so a gated field would simply not exist where it is needed.
    ///
    /// It earns two `usize` of production surface because the alternative is untestable: a
    /// request that is never issued leaves no trace anywhere else.
    requests: std::sync::Arc<std::sync::atomic::AtomicUsize>,
```

In `RenderEngine::new`, before `Self { .. }` is constructed, add:

```rust
        let requests = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
```

and add `requests,` to the returned `Self { .. }` literal.

Replace `request` with:

```rust
    pub fn request(&self, edits: Edits, spec: FrameSpec) {
        self.requests
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let _ = self.req_tx.send(Request { edits, spec });
    }

    /// How many renders have been asked for. See the field's doc comment.
    pub fn requests_issued(&self) -> usize {
        self.requests.load(std::sync::atomic::Ordering::Relaxed)
    }
```

⚠ The counter is bumped **before** the send, and unconditionally — a `send` to a dropped receiver returns `Err`, and a request that could not be delivered is still a request that was *asked for*, which is what the tests assert about.

- [ ] **Step 4: Run to verify it passes**

```bash
cargo test -p app --lib requests_are_counted 2>&1 | grep -E "^test result"
```

Expected: `ok. 1 passed`.

- [ ] **Step 5: Sabotage**

Change `fetch_add(1, ..)` to `fetch_add(0, ..)`. Confirm it **compiles**, then:

```bash
cargo test -p app --lib requests_are_counted 2>&1 | grep -E "FAILED"
```

Expected: `requests_are_counted ... FAILED`. Restore, then `touch crates/app/src/render_engine.rs`.

⚠ Restoring a file with `mv` preserves the backup's older mtime, so cargo reuses the artifact built from the *sabotaged* source. Always `touch` after a restore.

- [ ] **Step 6: Commit**

```bash
git add crates/app/src/render_engine.rs
git commit -m "feat(app): count the renders an engine has been asked for"
```

---

### Task 2: `canvas_geom` — the coordinate maths, lifted

**Files:**
- Create: `crates/app/src/develop/canvas_geom.rs`
- Modify: `crates/app/src/develop/mod.rs`
- Modify: `crates/app/src/develop/canvas.rs`

⚠ **All three files change in ONE commit.** These are `pub fn`s in a **binary** crate; without their callers they are dead code and `clippy -D warnings` rejects them.

- [ ] **Step 1: Write the module, tests first**

Create `crates/app/src/develop/canvas_geom.rs`:

```rust
//! Where things are on the canvas.
//!
//! ⚠⚠ **Every function here works against the *frame* — the whole post-geometry image — never
//! against `img_rect`.** Once a region render is on, `img_rect` covers only the rendered slice, so
//! placing a mask against it makes the mask drift with the view as you pan. That bug shipped, and
//! it had three heads: the overlay's position, the aspect used for hit-testing, and the drag
//! deltas. Lifting the maths here is what makes all three testable without a window.

use eframe::egui;

/// The on-screen rect of the whole post-geometry image.
///
/// `full` is the fit-resolution image size, `rung_k` the current render rung's factor over that
/// fit size, and `scale` the on-screen scale. ⚠ `pan` is added to the viewport centre, so the
/// frame moves with the view while staying the *frame* — which is exactly the distinction
/// `img_rect` loses.
pub fn frame_rect(
    viewport_center: egui::Pos2,
    pan: egui::Vec2,
    full: egui::Vec2,
    rung_k: f32,
    scale: f32,
) -> egui::Rect {
    let size = egui::vec2(full.x * rung_k, full.y * rung_k) * scale;
    egui::Rect::from_center_size(viewport_center + pan, size)
}

/// A screen point as a fraction of the frame, clamped to it.
///
/// ⚠ Clamped because a drag continues outside the image and a mask handle dragged off-frame must
/// stop at the edge rather than fly away.
pub fn normalized_point(p: egui::Pos2, frame: egui::Rect) -> (f32, f32) {
    let nx = ((p.x - frame.left()) / frame.width()).clamp(0.0, 1.0);
    let ny = ((p.y - frame.top()) / frame.height()).clamp(0.0, 1.0);
    (nx, ny)
}

/// The frame's aspect ratio, for the hit-testing that measures distance in *screen* space.
///
/// ⚠ Normalised mask coordinates are anisotropic, so a distance test needs this correction. Taking
/// it from `img_rect` instead — which happened — silently corrupted hit-testing at high zoom,
/// because a region's aspect is not the frame's.
pub fn frame_aspect(frame: egui::Rect) -> f32 {
    frame.width() / frame.height()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32, w: f32, h: f32) -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(w, h))
    }

    #[test]
    fn the_frame_is_centred_on_the_viewport_plus_the_pan() {
        let r = frame_rect(
            egui::pos2(100.0, 50.0),
            egui::vec2(10.0, -5.0),
            egui::vec2(80.0, 40.0),
            1.0,
            1.0,
        );
        assert_eq!(r.center(), egui::pos2(110.0, 45.0));
        assert_eq!(r.size(), egui::vec2(80.0, 40.0));
    }

    #[test]
    fn the_rung_and_the_scale_both_size_the_frame() {
        // ⚠ Both multiply. A frame sized by only one of them is the bug where the overlay is the
        // right shape and the wrong size, which reads as "the mask is offset".
        let r = frame_rect(
            egui::Pos2::ZERO,
            egui::Vec2::ZERO,
            egui::vec2(80.0, 40.0),
            2.0,
            0.5,
        );
        assert_eq!(r.size(), egui::vec2(80.0, 40.0), "2.0 * 0.5 = 1.0");
        let r2 = frame_rect(
            egui::Pos2::ZERO,
            egui::Vec2::ZERO,
            egui::vec2(80.0, 40.0),
            2.0,
            1.0,
        );
        assert_eq!(r2.size(), egui::vec2(160.0, 80.0));
    }

    #[test]
    fn panning_moves_the_frame_and_nothing_else() {
        // ⚠⚠ The regression guard for the drift bug. The frame must translate with the pan and
        // keep its size — if it were derived from the rendered region instead, it would *shrink*
        // to the visible slice and the mask would appear to follow the view.
        let a = frame_rect(
            egui::pos2(50.0, 50.0),
            egui::Vec2::ZERO,
            egui::vec2(60.0, 30.0),
            1.0,
            1.0,
        );
        let b = frame_rect(
            egui::pos2(50.0, 50.0),
            egui::vec2(25.0, 0.0),
            egui::vec2(60.0, 30.0),
            1.0,
            1.0,
        );
        assert_eq!(a.size(), b.size(), "panning must not resize the frame");
        assert_eq!(b.center() - a.center(), egui::vec2(25.0, 0.0));
    }

    #[test]
    fn a_point_normalises_against_the_frames_own_corner() {
        let f = rect(20.0, 10.0, 80.0, 40.0);
        assert_eq!(normalized_point(egui::pos2(20.0, 10.0), f), (0.0, 0.0));
        assert_eq!(normalized_point(egui::pos2(100.0, 50.0), f), (1.0, 1.0));
        assert_eq!(normalized_point(egui::pos2(60.0, 30.0), f), (0.5, 0.5));
    }

    #[test]
    fn a_point_outside_the_frame_clamps_rather_than_escaping() {
        // ⚠ A drag continues past the image edge; a handle dragged off-frame must stop at the
        // edge, not fly to an unreachable coordinate.
        let f = rect(0.0, 0.0, 100.0, 50.0);
        assert_eq!(normalized_point(egui::pos2(-40.0, -40.0), f), (0.0, 0.0));
        assert_eq!(normalized_point(egui::pos2(500.0, 500.0), f), (1.0, 1.0));
    }

    #[test]
    fn the_aspect_is_the_frames_not_a_regions() {
        // ⚠ The value that was wrongly taken from `img_rect`. On a 2:1 frame the correction is
        // 2.0; a region covering a square slice of it would report 1.0, and every hit test at
        // high zoom would be wrong by that factor.
        assert_eq!(frame_aspect(rect(0.0, 0.0, 100.0, 50.0)), 2.0);
        assert_eq!(frame_aspect(rect(0.0, 0.0, 50.0, 100.0)), 0.5);
    }
}
```

- [ ] **Step 2: Declare the module**

In `crates/app/src/develop/mod.rs`, after `pub mod canvas;`:

```rust
pub mod canvas_geom;
```

- [ ] **Step 3: Run to verify the tests pass and the build fails on dead code**

```bash
cargo test -p app --bin Rustroom canvas_geom 2>&1 | grep -E "^test result"
```

Expected: `ok. 6 passed`.

```bash
cargo clippy --all-targets --all-features -- -D warnings > /tmp/c.log 2>&1; echo "exit: $?"; grep -c "never used" /tmp/c.log
```

Expected: **non-zero exit**, with `never used` warnings — this is the bin-crate dead-code trap, and confirms Step 4 is required in the same commit.

- [ ] **Step 4: Replace the inline originals in `canvas.rs`**

Find this, at roughly line 216:

```rust
                    let frame_size = egui::vec2(full.x * pk, full.y * pk) * pscale;
                    let frame_rect =
                        egui::Rect::from_center_size(ui.max_rect().center() + s.pan, frame_size);
```

Replace with:

```rust
                    let frame_rect = canvas_geom::frame_rect(
                        ui.max_rect().center(),
                        s.pan,
                        full,
                        pk,
                        pscale,
                    );
```

Find this, in `draw_mask_overlay` at roughly line 475:

```rust
    let aspect = frame_rect.width() / frame_rect.height();
    let mut moved = false;
    if let Some(p) = resp.interact_pointer_pos() {
        let nx = ((p.x - frame_rect.left()) / frame_rect.width()).clamp(0.0, 1.0);
        let ny = ((p.y - frame_rect.top()) / frame_rect.height()).clamp(0.0, 1.0);
```

Replace with:

```rust
    let aspect = canvas_geom::frame_aspect(frame_rect);
    let mut moved = false;
    if let Some(p) = resp.interact_pointer_pos() {
        let (nx, ny) = canvas_geom::normalized_point(p, frame_rect);
```

Add the import near the top of `canvas.rs`, beside the other `use crate::` lines:

```rust
use crate::develop::canvas_geom;
```

- [ ] **Step 5: Verify nothing moved**

```bash
cargo clippy --all-targets --all-features -- -D warnings > /tmp/c.log 2>&1; echo "clippy: $?"
cargo test --all > /tmp/t.log 2>&1; echo "test: $?"
```

Expected: both `0`. The 47 goldens are untouched — this crate has none, and no pipeline code moved.

- [ ] **Step 6: Sabotage**

In `canvas_geom::frame_rect`, change `viewport_center + pan` to `viewport_center`. Confirm it **compiles**, then:

```bash
cargo test -p app --bin Rustroom canvas_geom 2>&1 | grep "FAILED"
```

Expected: `panning_moves_the_frame_and_nothing_else ... FAILED`. Restore and `touch`.

- [ ] **Step 7: Commit**

```bash
git add crates/app/src/develop/canvas_geom.rs crates/app/src/develop/mod.rs crates/app/src/develop/canvas.rs
git commit -m "refactor(app): lift the canvas frame maths into a tested module"
```

---

### Task 3: The fixture

**Files:**
- Create: `crates/app/src/test_app.rs`
- Modify: `crates/app/src/main.rs`

⚠ **Lands with its smoke test in one commit** — a `#[cfg(test)]` module whose functions nothing calls is dead code.

- [ ] **Step 1: Write the fixture**

Create `crates/app/src/test_app.rs`:

```rust
//! A whole `RustroomApp`, built for tests.
//!
//! ⚠⚠ **This exists because the canvas is the least-tested code in the project and the barrier was
//! always construction, never intent.** Every untested drawing function takes `&mut RustroomApp`
//! and needs a live `Session`; a `Session` needs a render engine and textures. It turns out none of
//! that needs disk, libraw or a GPU — a synthetic `ImageBuf` and `ctx.load_texture` suffice.
//!
//! ⚠ The render engine's worker thread really does spawn. Tests must never wait on it: assert on
//! *requests and state*, never on rendered pixels.

use crate::*;
use eframe::egui;

/// A temp directory that removes itself.
///
/// ⚠ Uniqueness is **structural** — process id plus a counter — never the clock. Two tests entering
/// the same tick collide, `cargo test` runs them in parallel, and this project has paid for that
/// twice: once in `image-decode`, once in `app`, where it surfaced as an *unrelated pre-existing
/// test* failing during a sabotage run.
pub struct TestDir(pub std::path::PathBuf);

impl TestDir {
    pub fn new(tag: &str) -> Self {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "rustroom_testapp_{tag}_{}_{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A flat grey buffer of the given size.
pub fn buffer(w: usize, h: usize) -> std::sync::Arc<core_image::ImageBuf> {
    let mut b = core_image::ImageBuf::new(w, h);
    for v in b.data.iter_mut() {
        *v = 0.5;
    }
    std::sync::Arc::new(b)
}

/// An `RgbaImage`-shaped texture of the given size, for `Session`'s two texture fields.
fn texture(ctx: &egui::Context, name: &str, w: usize, h: usize) -> egui::TextureHandle {
    let img = egui::ColorImage::new([w, h], vec![egui::Color32::GRAY; w * h]);
    ctx.load_texture(name, img, egui::TextureOptions::default())
}

/// A `RustroomApp` with no photo open.
///
/// ⚠ Every field is listed explicitly rather than through a `Default`. Following this project's
/// exhaustive-`Settings`-literal precedent, that is deliberate: a new field on `RustroomApp`
/// becomes a **compile error here**, forcing a decision about what a test should see.
pub fn for_test(ctx: &egui::Context) -> RustroomApp {
    let dir = TestDir::new("app");
    let stub = |_: &std::path::Path| -> Result<(core_image::ImageBuf, String, f32, LensFacts), String> {
        Ok((
            core_image::ImageBuf::new(8, 4),
            "test".to_string(),
            1.0,
            LensFacts::default(),
        ))
    };
    RustroomApp {
        camera_profiles_cache: Vec::new(),
        os_manages_colour: false,
        view: AppView::Develop,
        last_view: AppView::Develop,
        library: None,
        session: None,
        pending: None,
        decoder: DecodeWorker::new(stub, 2, || {}),
        full_decoder: DecodeWorker::new(stub, 1, || {}),
        full_pending: None,
        cache: BufferCache::default(),
        histories: HistoryStash::default(),
        status: String::new(),
        show_filmstrip: false,
        brush_size: 40.0,
        brush_hardness: 50.0,
        brush_flow: 100.0,
        brush_erase: false,
        show_mask_tint: false,
        hsl_channel: develop::panel::adjustments::HslChannel::Hue,
        grade_range: develop::panel::color_wheel::GradeRange::Midtones,
        rename_draft: None,
        cell_zoom_pending: false,
        tool: Tool::Edit,
        crop_aspect: None,
        clipboard: None,
        paste_groups: EditGroups::default(),
        sync_groups: EditGroups::default(),
        export_draft: ExportOptions::default(),
        exporter: None,
        export_progress: String::new(),
        export_collision: None,
        export_apply_all: false,
        filmstrip_scrolled_to: None,
        export_preset_store: ExportPresetStore::new(dir.0.join("export_presets")),
        export_preset_list: Vec::new(),
        selected_export_preset: None,
        preset_store: PresetStore::new(dir.0.join("presets"), dir.0.join("trash")),
        preset_list: Vec::new(),
        settings_store: SettingsStore::new(dir.0.join("settings.json")),
        settings: Settings::default(),
        modal: Modal::None,
        actions: ActionLog::default(),
    }
}

/// Open a synthetic photo, so the canvas has something to draw.
///
/// ⚠ `fit_scale` and `fit_image_size` are set as a real session would have them after one frame,
/// because the canvas derives placement from them and a zero would make every rect degenerate.
pub fn open_test_photo(app: &mut RustroomApp, ctx: &egui::Context, w: usize, h: usize) {
    let buf = buffer(w, h);
    let engine = RenderEngine::new(buf.clone(), 1.0, None, || {});
    app.session = Some(Session {
        path: std::path::PathBuf::from("/test/a.RAF"),
        copy: 0,
        name: None,
        engine,
        edits: Edits::default(),
        texture: Some(texture(ctx, "frame", w, h)),
        baseline: texture(ctx, "baseline", w, h),
        hist: [[0; 256]; 3],
        make_model: "Test Camera".to_string(),
        lens: LensFacts::default(),
        zoom: 1.0,
        fit_scale: 1.0,
        pan: egui::Vec2::ZERO,
        history: History::new(Edits::default()),
        show_before: false,
        active_handle: None,
        selected_mask: None,
        active_mask_handle: None,
        rendered_region: None,
        requested_region: None,
        fit_image_size: egui::vec2(w as f32, h as f32),
        rendered_long: w.max(h) as f32,
        buffer_long: w.max(h),
        buffer_is_full_res: true,
        opened_at: std::time::Instant::now(),
        render_rung: 0,
        display_space: color::OutputSpace::Srgb,
    });
}
```

- [ ] **Step 2: Declare the module**

In `crates/app/src/main.rs`, beside the other bin module declarations:

```rust
#[cfg(test)]
mod test_app;
```

- [ ] **Step 3: Write the smoke test**

Add to `crates/app/src/develop/canvas.rs`'s `#[cfg(test)] mod tests`:

```rust
    #[test]
    fn the_fixture_opens_a_photo_and_the_canvas_draws_it() {
        // ⚠ The smoke test that makes the fixture worth having: it proves `show` can be driven
        // headlessly at all. Everything else in this module builds on it.
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test(&ctx);
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        assert!(app.session.is_some());

        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 300.0),
                )),
                ..Default::default()
            },
            |ui| {
                super::show(&mut app, ui, ui.ctx());
            },
        );
    }
```

- [ ] **Step 4: Run**

```bash
cargo test -p app --bin Rustroom the_fixture_opens 2>&1 | grep -E "^test result|panicked" -A 4
```

Expected: `ok. 1 passed`. ⚠ If it panics on a borrow or a missing field, fix the fixture — do **not** weaken the test.

- [ ] **Step 5: Verify clean**

```bash
cargo clippy --all-targets --all-features -- -D warnings > /tmp/c.log 2>&1; echo "clippy: $?"
cargo test --all > /tmp/t.log 2>&1; echo "test: $?"
```

Expected: both `0`.

- [ ] **Step 6: Commit**

```bash
git add crates/app/src/test_app.rs crates/app/src/main.rs crates/app/src/develop/canvas.rs
git commit -m "test(app): a headless fixture that drives the real canvas"
```

---

### Task 4: The three regression tests

**Files:**
- Modify: `crates/app/src/develop/canvas.rs`

- [ ] **Step 1: Write a multi-pass drag harness plus the three tests**

Add to `canvas.rs`'s `#[cfg(test)] mod tests`:

```rust
    /// Drive `show` through a full press → move → release drag at the given points.
    ///
    /// ⚠⚠ **Four passes, and the count is load-bearing.** `Response` state is per-frame, so a
    /// single-pass harness reads zeros — that is what made `a_left_drag_does_not_pan` pass
    /// vacuously once already. And a release is only *known* on its own pass, so anything it
    /// triggers cannot be observed until the frame after.
    fn drag_through(
        app: &mut RustroomApp,
        ctx: &egui::Context,
        from: egui::Pos2,
        to: egui::Pos2,
    ) {
        for pass in 0..4 {
            let pos = if pass == 0 { from } else { to };
            let mut events = vec![egui::Event::PointerMoved(pos)];
            if pass == 1 {
                events.push(egui::Event::PointerButton {
                    pos: from,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                });
            }
            if pass == 2 {
                events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                });
            }
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(400.0, 300.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    super::show(app, ui, ui.ctx());
                },
            );
        }
    }

    /// A session with one linear-gradient mask selected, and the Masks tool active.
    fn with_selected_mask(app: &mut RustroomApp) {
        app.tool = crate::tool::Tool::Masks;
        let s = app.session.as_mut().unwrap();
        s.edits.masks.push(edit_model::MaskedAdjustment::default());
        s.selected_mask = Some(0);
    }

    #[test]
    fn dragging_a_mask_handle_asks_for_a_render() {
        // ⚠⚠ The regression guard for a bug that shipped: `draw_mask_overlay` mutated `Edits` but
        // never called `s.request`, so the image kept showing the mask's previous position until
        // some *unrelated* edit happened to request a frame. That is why moving a slider appeared
        // to "fix" it.
        //
        // ⚠ The cause was an inherited false analogy — it was modelled on `draw_crop_overlay`,
        // which needs no request *because* `preview_edits` strips the crop in geometry mode, so
        // the overlay itself is the feedback. Masks are not stripped.
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test(&ctx);
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        with_selected_mask(&mut app);

        let before = app.session.as_ref().unwrap().engine.requests_issued();
        drag_through(
            &mut app,
            &ctx,
            egui::pos2(200.0, 150.0),
            egui::pos2(260.0, 150.0),
        );
        let after = app.session.as_ref().unwrap().engine.requests_issued();

        assert!(
            after > before,
            "a mask drag must ask for a render; before={before} after={after}"
        );
    }

    #[test]
    fn the_mask_overlay_is_placed_against_the_frame_not_the_view() {
        // ⚠⚠ The regression guard for the drift bug: the overlay was placed against `img_rect`,
        // which once a region render is on covers only the *rendered slice*. The mask then
        // followed the visible part of the image as you panned.
        //
        // ⚠ Asserted through `canvas_geom::frame_rect` rather than by reading pixels: the frame's
        // *size* must not change with the pan, which is precisely what `img_rect` would do.
        let full = egui::vec2(80.0, 40.0);
        let unpanned = super::canvas_geom::frame_rect(
            egui::pos2(200.0, 150.0),
            egui::Vec2::ZERO,
            full,
            1.0,
            2.0,
        );
        let panned = super::canvas_geom::frame_rect(
            egui::pos2(200.0, 150.0),
            egui::vec2(50.0, -20.0),
            full,
            1.0,
            2.0,
        );
        assert_eq!(
            unpanned.size(),
            panned.size(),
            "panning must not resize the frame the mask is placed against"
        );
        assert_eq!(panned.center() - unpanned.center(), egui::vec2(50.0, -20.0));
    }

    #[test]
    fn a_mask_drag_commits_on_release() {
        // ⚠⚠ The regression guard for the third bug shape: `drag_stopped` was nested inside
        // `if let Some(p) = resp.interact_pointer_pos()`, and on the release frame egui often
        // reports **no** interact position — so both the render and the history push were
        // silently skipped, and a finished stroke could be painted over by a stale frame.
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test(&ctx);
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        with_selected_mask(&mut app);
        let before = app.session.as_ref().unwrap().edits.clone();

        drag_through(
            &mut app,
            &ctx,
            egui::pos2(180.0, 150.0),
            egui::pos2(280.0, 160.0),
        );

        let after = &app.session.as_ref().unwrap().edits;
        assert_ne!(
            &before, after,
            "the drag must have moved the mask by the time the release frame is over"
        );
    }
```

- [ ] **Step 2: Run**

```bash
cargo test -p app --bin Rustroom 2>&1 | grep -E "^test result|panicked" -A 4 | head -12
```

Expected: all pass.

⚠ If `dragging_a_mask_handle_asks_for_a_render` or `a_mask_drag_commits_on_release` fails, the drag may not be landing on a handle. Adjust the drag points so `from` sits on a handle of the default mask — do **not** relax the assertion.

- [ ] **Step 3: Sabotage each of the three**

Use a helper that checks compilation first:

```bash
cat > /tmp/sab.py <<'PY'
import subprocess, shutil
def run(label, path, old, new, expect):
    bak = path + ".bak"; shutil.copy(path, bak)
    s = open(path).read(); assert old in s, f"{label}: pattern not found"
    open(path,"w").write(s.replace(old, new, 1))
    c = subprocess.run(["cargo","test","-p","app","--bin","Rustroom"],capture_output=True,text=True)
    out = c.stdout + c.stderr
    compiled = "error[E" not in out and "could not compile" not in out
    hits = [l for l in out.splitlines() if "FAILED" in l and expect in l]
    print(f"{label}: " + ("SABOTAGE DID NOT COMPILE — meaningless" if not compiled
          else "CAUGHT" if hits else "NOT CAUGHT"))
    shutil.move(bak, path); subprocess.run(["touch", path])
PY
```

⚠⚠ **Confirm the harness itself works before trusting a clean run.** A matcher that greps failure lines by *bare* test name finds nothing, because cargo prints module paths — that made all seven sabotages in a previous round report "NOT CAUGHT" when every one was actually caught. The helper above matches by substring for that reason.

Sabotages to run:
1. In `canvas.rs`'s mask drag path, delete the `s.request(..)` call that follows a handle move → expect `dragging_a_mask_handle_asks_for_a_render` CAUGHT.
2. In `canvas_geom::frame_rect`, drop the `+ pan` → expect `the_mask_overlay_is_placed_against_the_frame_not_the_view` CAUGHT.
3. In `canvas.rs`, move the `drag_stopped` handling *inside* `if let Some(p) = resp.interact_pointer_pos()` → expect `a_mask_drag_commits_on_release` CAUGHT.

- [ ] **Step 4: Verify clean**

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings > /tmp/c.log 2>&1; echo "clippy: $?"
cargo test --all > /tmp/t.log 2>&1; echo "test: $?"
grep -hoE "[0-9]+ passed" /tmp/t.log | awk '{s+=$1} END {print "tests:", s}'
```

Expected: both `0`; test count up by roughly 10 from 933.

- [ ] **Step 5: Commit**

```bash
git add crates/app/src/develop/canvas.rs
git commit -m "test(app): regression guards for the three canvas bug shapes"
```

---

### Task 5: Record it

**Files:**
- Modify: `CLAUDE.md`

- [ ] **Step 1: Update the standing warning**

The file currently says canvas and panel work is "untested territory". Replace that claim with what is now true: the fixture exists, three bug shapes are guarded, and **appearance is still uncovered**. Add a section for this work following the established format — what it does, the ⚠ findings, and accepted caveats.

Include specifically:
- The fixture needs **no disk, libraw or GPU**, and where it lives.
- ⚠ The counter on `RenderEngine` is **unconditional because `#[cfg(test)]` does not cross the lib/bin boundary**.
- ⚠ The four-pass drag harness, and why fewer passes read zeros.
- ⚠ What this still cannot catch: appearance, and the two most spectacular past bugs (the 711 MB vertex buffer, the wrong-looking tint).

- [ ] **Step 2: Update the state line**

In the RESUME block, update the test count and mark step 1 of the agreed order of work as complete.

- [ ] **Step 3: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: record the canvas test fixture"
```

---

## Self-review

**Spec coverage:** fixture → Task 3; request counter → Task 1; the three lifted functions → Task 2; the three named regression tests → Task 4; sabotage discipline → Steps in each task; scope exclusions → stated in Task 4's caveats and Task 5.

**Placeholders:** none — every step carries the code or the exact command.

**Type consistency:** `requests_issued()` is defined in Task 1 and used in Task 4. `canvas_geom::{frame_rect, normalized_point, frame_aspect}` are defined in Task 2 and used in Tasks 2 and 4. `test_app::{for_test, open_test_photo, TestDir, buffer}` are defined in Task 3 and used in Tasks 3 and 4.

⚠ **One known risk, flagged rather than hidden:** Task 3's `for_test` lists 43 fields from a struct that may have drifted. If a field name or type is wrong, the compiler names it — fix the fixture, and do not reach for `..Default::default()`, which would defeat the point of the exhaustive literal.
