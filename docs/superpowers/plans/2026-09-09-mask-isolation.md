# Mask isolation and the render-path defects — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

> ⚠⚠ **Conventions that are not negotiable in this repository.** Commit messages carry **no
> `Co-Authored-By` trailer** and no AI attribution of any kind. **Never push** — the user pushes to
> `github.com/wladiKrin/rustroom` himself. Work stays on the existing branch `m7.6-auto-mask`.

**Goal:** While a mask's tint is shown, that mask's own adjustments leave the view, so painting
needs no render and the rung never changes mid-stroke; plus the two render-path defects the
placement instruments found.

**Architecture:** Four changes in one seam — the render-request path shared by the canvas, the
panel and the session. A `Session.suppressed_mask` mirroring the existing `suppressed_spot`; an
exemption in the interactive rung's gate for gestures that change no pixels; an exact fit-scale
size carried on the frame; and a handover that requests at the current rung.

**Tech Stack:** Rust, egui 0.35 + wgpu, `rustroom_app` lib (no egui) + `Rustroom` bin (egui).

**Spec:** `docs/superpowers/specs/2026-09-09-mask-isolation-design.md`

---

## Standing rules for every task in this plan

- **The gate before every commit**, each exit code read from its **own file**, never through a
  pipe (`${PIPESTATUS[0]}` is bash and this shell is zsh):

  ```bash
  export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
  cargo fmt --all
  cargo fmt --all --check > /tmp/f.log 2>&1; echo "FMT=$?"
  cargo clippy --all-targets --all-features -- -D warnings > /tmp/c.log 2>&1; echo "CLIPPY=$?"
  cargo test --workspace > /tmp/t.log 2>&1; echo "TEST=$?"
  grep -c "^test .* ok$" /tmp/t.log      # expect 2138 + whatever this task added
  grep -c "^test golden_.* ok$" /tmp/t.log   # expect 60, every task, every time
  ```

  ⚠ `-- -D warnings` is not optional: CI runs it, and a commit has shipped in this repository
  with clippy failing at least twelve times because a local check omitted it.

- **⚠ Read the passed-count on every filtered run.** `test result: ok` says nothing about what
  ran; `0 passed; N filtered out` is this project's most-repeated false pass.

- **Every new test is sabotage-verified**: break the thing the test names, run it, and confirm a
  `test result:` line appears **and** the named test fails. A sabotage that fails to compile looks
  exactly like one that passes. Restore from a copy afterwards and check `git status`.

- **After any scripted edit, check `git diff | grep '^-' | wc -l`** against the number of lines
  the change was meant to remove. A `sed` that matches nothing exits 0; one that matches too much
  splices in the wrong place. Both have shipped here.

- **All 60 goldens must stay byte-identical.** Nothing in this plan touches a pixel path. That is
  the assertion, not "still green".

---

## File structure

| File | Responsibility | Change |
|---|---|---|
| `crates/app/src/view_region.rs` | pure viewport maths, no egui | **add** `fit_size` |
| `crates/app/src/render_engine.rs` | render worker, frame construction | **add** `RenderedFrame::fit_size` |
| `crates/app/src/shell.rs` | frame landing | **modify** the `fit_image_size` write |
| `crates/app/src/main.rs` | `Session` fields | **add** `suppressed_mask` |
| `crates/app/src/session_flow.rs` | `view_edits`, `pump_full_res` | **modify** both |
| `crates/app/src/develop/mod.rs` | `body`, the one derivation site | **modify** |
| `crates/app/src/develop/canvas/navigation.rs` | pan gesture predicates | **add** `pan_button_down` |
| `crates/app/src/develop/canvas/mod.rs` | rung choice, request condition | **modify** |
| `crates/app/src/develop/canvas/mask_overlay.rs` | the two stroke request sites | **modify** |
| `crates/app/src/test_app.rs` | exhaustive `Session` literal | **modify** |
| `crates/app/examples/bench_pan_rung.rs` | the panning measurement | **create** |

⚠ `crates/app/src/develop/canvas/mod.rs` is already **1,730 lines** and
`crates/app/src/main.rs` about **800**. Both are past the ~600 marker and both grow a little here;
neither is split in this plan, and that is reported rather than done silently.

---

## Task 1: `view_region::fit_size` — the post-geometry size at the fit cap, exactly

**Why:** `fit_image_size` is currently `frame_dims × fit_cap / frame.max_dim`, and the frame's
dimensions come from `downscale_to`, which **rounds**. The error is therefore amplified by
`rung / fit_cap`: measured, `full.y` reads 1068.000 at rungs 1600 and 3200 against 1068.204 at
6246. This function computes the same quantity in floats from the buffer and the geometry, so it
depends on no rung at all.

**Files:**
- Modify: `crates/app/src/view_region.rs`

- [ ] **Step 1: Write the failing tests**

Append to the `mod tests` block at the bottom of `crates/app/src/view_region.rs`:

```rust
    #[test]
    fn the_fit_size_is_the_buffer_scaled_to_the_cap() {
        // An uncropped X-T4 frame: 6246x4170 capped at 1600.
        let g = edit_model::Geometry::default();
        let (w, h) = fit_size(6246, 4170, 1600, &g);
        assert!((w - 1600.0).abs() < 1e-3, "width {w}");
        // ⚠ The value the shipped code gets *wrong*: 4170 * 1600 / 6246 = 1068.204, where
        // deriving it from a rounded 2136-row frame at rung 3200 gives exactly 1068.0.
        assert!((h - 1068.2036).abs() < 1e-3, "height {h}");
    }

    #[test]
    fn the_fit_size_follows_the_crop() {
        let g = edit_model::Geometry {
            crop: edit_model::CropRect {
                left: 0.25,
                top: 0.0,
                right: 0.75,
                bottom: 0.5,
            },
            ..edit_model::Geometry::default()
        };
        let (w, h) = fit_size(6246, 4170, 1600, &g);
        assert!((w - 800.0).abs() < 1e-3, "width {w}");
        assert!((h - 534.1018).abs() < 1e-3, "height {h}");
    }

    /// ⚠⚠ A quarter turn **transposes**, and forgetting it is invisible on a square fixture and
    /// on any test that only checks the long edge. `GeometryStage` applies the orientation before
    /// the crop, so the crop fractions apply to the *turned* frame — which is why the width here
    /// comes from the crop's horizontal extent against the turned height.
    #[test]
    fn a_quarter_turn_transposes_the_fit_size() {
        let g = edit_model::Geometry {
            orientation: edit_model::Orientation {
                rotate: edit_model::Quarter::Cw90,
                mirror: false,
            },
            ..edit_model::Geometry::default()
        };
        let (w, h) = fit_size(6246, 4170, 1600, &g);
        assert!((w - 1068.2036).abs() < 1e-3, "width {w}");
        assert!((h - 1600.0).abs() < 1e-3, "height {h}");
        // A half turn does not transpose, which is what stops "any rotation swaps" passing.
        let g2 = edit_model::Geometry {
            orientation: edit_model::Orientation {
                rotate: edit_model::Quarter::Cw180,
                mirror: false,
            },
            ..edit_model::Geometry::default()
        };
        let (w2, h2) = fit_size(6246, 4170, 1600, &g2);
        assert!((w2 - 1600.0).abs() < 1e-3, "width {w2}");
        assert!((h2 - 1068.2036).abs() < 1e-3, "height {h2}");
    }

    /// A degenerate buffer must not produce a zero or a NaN: every consumer divides by this.
    #[test]
    fn a_degenerate_buffer_still_yields_a_usable_size() {
        let g = edit_model::Geometry::default();
        let (w, h) = fit_size(0, 0, 1600, &g);
        assert!(w >= 1.0 && h >= 1.0, "{w}x{h}");
        assert!(w.is_finite() && h.is_finite(), "{w}x{h}");
    }
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test -p app --lib fit_size 2>&1 | tail -20
```

Expected: **compile error**, `cannot find function \`fit_size\``. That is the failure; do not
proceed until you have seen it.

- [ ] **Step 3: Write the implementation**

Insert into `crates/app/src/view_region.rs`, immediately after the `fit_cap` function:

```rust
/// The post-geometry image size **at the fit cap**, in exact floats.
///
/// ⚠⚠ **This is what `Session::fit_image_size` means, and deriving it from a rendered frame is
/// what made that value depend on which rung last landed.** A frame's dimensions come from
/// `downscale_to`, which rounds; scaling them back up by `fit_cap / max_dim` multiplies that
/// rounding by `rung / fit_cap`. Measured on an uncropped X-T4 frame, the height read 1068.000 at
/// rungs 1600 and 3200 against a true 1068.204 at 6246. Computed here from the buffer and the
/// geometry there is no rounding to amplify, and the result is identical at every rung by
/// construction.
///
/// ⚠ `geometry` must be the **view** geometry — what the render was asked for — not
/// `Session::edits.geometry`. Geometry mode resets the crop for the preview, so the two differ
/// exactly when the crop overlay is on screen and depending on it.
///
/// ⚠ The orientation is applied before the crop, matching `GeometryStage`: a quarter turn
/// transposes the frame and the crop fractions then apply to the turned one.
pub fn fit_size(
    buffer_w: usize,
    buffer_h: usize,
    fit_cap: usize,
    geometry: &edit_model::Geometry,
) -> (f32, f32) {
    let long = buffer_w.max(buffer_h).max(1) as f32;
    let ratio = fit_cap.max(1) as f32 / long;
    let (mut w, mut h) = (buffer_w as f32 * ratio, buffer_h as f32 * ratio);
    if geometry.orientation.transposes() {
        std::mem::swap(&mut w, &mut h);
    }
    let c = geometry.crop;
    // ⚠ Floored at one pixel rather than trusted: every consumer divides by this, and a zero
    // buffer or a degenerate crop would otherwise produce an infinity that `clamp` cannot
    // repair — the NaN trap `crop_tool` already documents.
    (
        (w * (c.right - c.left)).max(1.0),
        (h * (c.bottom - c.top)).max(1.0),
    )
}
```

- [ ] **Step 4: Add `Orientation::transposes`**

In `crates/edit-model/src/lib.rs`, inside `impl Orientation`, immediately after `is_identity`:

```rust
    /// Whether this orientation swaps the frame's width and height.
    ///
    /// ⚠ The mirror does not transpose — it reflects — so only the odd quarter turns do. A
    /// predicate that answered "any non-identity orientation" would be right on a square fixture
    /// and wrong on every photograph.
    pub fn transposes(self) -> bool {
        matches!(self.rotate, Quarter::Cw90 | Quarter::Cw270)
    }
```

- [ ] **Step 5: Run the tests**

```bash
cargo test -p app --lib fit_size 2>&1 | tail -15
```

Expected: `4 passed`. ⚠ Read the **passed count**, not the `ok`.

- [ ] **Step 6: Sabotage-verify**

Change `if geometry.orientation.transposes()` to `if false`, run
`cargo test -p app --lib a_quarter_turn_transposes_the_fit_size`, and confirm it **fails** with a
`test result:` line present. Restore.

- [ ] **Step 7: Gate and commit**

Run the full gate from "Standing rules". Then:

```bash
git add crates/app/src/view_region.rs crates/edit-model/src/lib.rs
git commit -F - <<'MSG'
view_region::fit_size — the post-geometry size at the fit cap, exactly

fit_image_size is derived from a rendered frame's dimensions scaled back
up by fit_cap / max_dim. Those dimensions come from downscale_to, which
rounds, so the error is amplified by rung / fit_cap: measured, the height
reads 1068.000 at rungs 1600 and 3200 against a true 1068.204 at 6246.

Computed from the buffer and the geometry there is nothing to amplify and
the result is identical at every rung by construction.

Orientation::transposes lands with it: a quarter turn swaps the frame and
a mirror does not, which is invisible on a square fixture.
MSG
```

---

## Task 2: carry `fit_size` on the frame and read it in `shell.rs`

**Why:** the value must stay **consistent with the texture it ships with**. Deriving it in
`shell.rs` from `s.edits` instead would desynchronise it from the frame on screen for a frame or
two after any crop change, misplacing the texture — trading a 0.4 px error for a visible one.
Carrying it on the frame keeps one fact travelling with the pixels it describes, which is exactly
why `RenderedFrame::max_dim` is carried rather than inferred.

**Files:**
- Modify: `crates/app/src/render_engine.rs`
- Modify: `crates/app/src/shell.rs:129-145`

- [ ] **Step 1: Write the failing test**

Append to `mod tests` in `crates/app/src/render_engine.rs`:

```rust
    /// ⚠⚠ **The property the shipped code fails.** `fit_image_size` is what every overlay is
    /// placed against, and it must not depend on which rung last landed. Rendering the same
    /// photo at two rungs must report the same fit size.
    #[test]
    fn the_fit_size_is_the_same_at_every_rung() {
        let buf = std::sync::Arc::new(chart(64, 43));
        let mut engine_frame = |max_dim: usize| {
            render_once(
                &buf,
                &Edits::default(),
                max_dim,
                1.0,
                color::OutputSpace::Srgb,
                pipeline::Resampler::default(),
            )
            .fit_size
        };
        let lo = engine_frame(16);
        let hi = engine_frame(64);
        assert!(
            (lo.0 - hi.0).abs() < 1e-3 && (lo.1 - hi.1).abs() < 1e-3,
            "the fit size was {lo:?} at one rung and {hi:?} at another — it is being derived \
             from the rendered frame's rounded dimensions"
        );
    }
```

⚠ If a `chart` helper does not already exist in that test module, build the fixture inline with
`ImageBuf::new(64, 43)` filled with any values — the test asserts on **dimensions only**, so the
pixel content is irrelevant. Check first:
`grep -n "fn chart" crates/app/src/render_engine.rs`.

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test -p app --lib the_fit_size_is_the_same_at_every_rung 2>&1 | tail -15
```

Expected: **compile error**, `no field \`fit_size\``.

- [ ] **Step 3: Add the field**

In `crates/app/src/render_engine.rs`, inside `pub struct RenderedFrame`, immediately after the
`max_dim` field and its doc comment:

```rust
    /// The post-geometry image size **at the fit cap**, in exact floats — what the canvas places
    /// every overlay against.
    ///
    /// ⚠⚠ **Carried rather than derived by the caller, for the reason `max_dim` above is.** It was
    /// computed in `shell.rs` as `frame_dims × fit_cap / max_dim`, and a frame's dimensions come
    /// from `downscale_to`, which rounds — so the rounding was amplified by `rung / fit_cap` and
    /// the placement depended on which rung last landed. Measured: 1068.000 at rungs 1600 and
    /// 3200 against a true 1068.204 at 6246.
    ///
    /// ⚠ Deriving it in `shell.rs` from `Session::edits` instead would be exact and **worse**: it
    /// would describe a crop the frame on screen does not have until the next frame lands, so the
    /// texture would be misplaced for a frame or two after every crop change. Travelling with the
    /// pixels it describes is what makes that unrepresentable.
    pub fit_size: (f32, f32),
```

- [ ] **Step 4: Fill it in at both construction sites**

`pack` is the only constructor. Rather than adding an eighth parameter — clippy's
`too_many_arguments` already fires on that function's neighbours — give it a placeholder in `pack`
and overwrite it at the two callers that know the answer.

In `pack`, where the `RenderedFrame` literal is built, add:

```rust
        // ⚠ A placeholder. `pack` does not know the buffer or the geometry; both callers
        // overwrite this immediately. It is the frame's own size so a caller that forgot would
        // degrade to the pre-`fit_size` behaviour at the fit rung rather than to a zero.
        fit_size: (width as f32, height as f32),
```

In `render_cached`, replace the final `(frame, ran)` return expression so the frame is patched
first. Find the tail of the function and change it to:

```rust
    let mut frame = /* the existing expression that produced the frame */;
    frame.fit_size = crate::view_region::fit_size(
        buffer.width(),
        buffer.height(),
        crate::view_region::fit_cap(buffer_long, crate::PREVIEW_MAX_DIM),
        &edits.geometry,
    );
    (frame, ran)
```

⚠ `edits` here is already the **view** edits — `Session::view_edits` has run
`preview_edits`, which is what resets the crop in geometry mode — so this is the geometry the
render was actually asked for, which is the requirement in `fit_size`'s doc comment.

In `render_once`, do the same to its returned frame:

```rust
    let mut frame = pack(/* ... unchanged arguments ... */);
    frame.fit_size = crate::view_region::fit_size(
        buffer.width(),
        buffer.height(),
        crate::view_region::fit_cap(
            buffer.width().max(buffer.height()),
            crate::PREVIEW_MAX_DIM,
        ),
        &edits.geometry,
    );
    frame
```

⚠ `PREVIEW_MAX_DIM` lives in the **bin** (`main.rs`) and `render_engine` is in the **lib**, so
`crate::PREVIEW_MAX_DIM` will not resolve. Check where it is defined
(`grep -rn "PREVIEW_MAX_DIM" crates/app/src | head`) and, if it is bin-only, **move the constant
into `view_region`** and re-export it from `main.rs` so both crates read one value. Two constants
that must agree is the failure this project has paid for three times.

- [ ] **Step 5: Read it in `shell.rs`**

Replace the whole `if frame.max_dim > 0 { … }` block at `crates/app/src/shell.rs:129-145` with:

```rust
                // ⚠⚠ **Read from the frame, not recomputed here.** This used to be
                // `frame_dims × fit_cap / frame.max_dim`, which amplified `downscale_to`'s
                // rounding by `rung / fit_cap` and made the placement depend on which rung last
                // landed — measured at 1068.000 against a true 1068.204. `RenderedFrame::fit_size`
                // is exact and travels with the pixels it describes, so it cannot disagree with
                // the texture on screen.
                s.fit_image_size = egui::vec2(frame.fit_size.0, frame.fit_size.1);
```

⚠ The long comment block above it explaining the cap-ratio derivation goes with the code it
describes. Keep the paragraph beginning "⚠⚠ **A REGION frame updates this too**" — its finding
still holds and is why a region frame must not be skipped — but rewrite it to say that every frame
now carries the value, so the whole/region distinction has stopped existing here.

- [ ] **Step 6: Run the tests**

```bash
cargo test -p app --lib the_fit_size_is_the_same_at_every_rung 2>&1 | tail -15
```

Expected: `1 passed`.

- [ ] **Step 7: Sabotage-verify**

In `render_cached`, delete the `frame.fit_size = …` assignment so the placeholder survives. The
test must fail. Restore, and check `git status` shows only the intended files modified.

- [ ] **Step 8: Gate and commit**

⚠ Confirm `grep -c "^test golden_.* ok$" /tmp/t.log` reports **60**. A change to how frames are
constructed is exactly where a golden would move if something slipped.

```bash
git add -u
git commit -F - <<'MSG'
Carry the exact fit-scale size on the frame

shell.rs derived fit_image_size as frame_dims x fit_cap / frame.max_dim.
The dimensions come from downscale_to, which rounds, so the error was
amplified by rung / fit_cap and the placement depended on which rung last
landed: 1068.000 at rungs 1600 and 3200 against a true 1068.204 at 6246.

Carried on the frame rather than derived in shell.rs from Session::edits.
That form would also be exact and would be worse: it describes a crop the
frame on screen does not have until the next one lands, so the texture
would be misplaced for a frame or two after every crop change. Travelling
with the pixels it describes makes that unrepresentable.

Worth 0.4 px. The point is removing the rung from a placement reference,
not anything anyone can see.
MSG
```

---

## Task 3: `Session.suppressed_mask`, and `view_edits` drops it

**Why:** the seam for mask isolation. `suppressed_spot` already does exactly this for spot
removal; this is its mirror.

**Files:**
- Modify: `crates/app/src/main.rs` (the `Session` struct, beside `suppressed_spot`)
- Modify: `crates/app/src/session_flow.rs` (`view_edits`)
- Modify: `crates/app/src/test_app.rs` (the exhaustive `Session` literal)
- Modify: `crates/app/src/develop/mod.rs` (the one derivation site)

⚠ Tasks 3 and 4 of the original decomposition are **one commit**, because a field written nowhere
and a derivation with nothing reading it are each dead code under `-D warnings` in a **binary**
crate. This trap has fired at least five times in this repository; it is not hypothetical.

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `crates/app/src/session_flow.rs`:

```rust
    /// ⚠ The rule: the suppressed mask leaves the view and **nothing else does**. A test that
    /// only checked the count would pass against a version that dropped the wrong one.
    #[test]
    fn the_suppressed_mask_leaves_the_view_and_nothing_else_does() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        let s = app.session.as_mut().unwrap();
        s.edits.masks = vec![
            masked_exposure(0.5),
            masked_exposure(1.5),
            masked_exposure(2.5),
        ];
        s.suppressed_mask = Some(1);
        let e = s.view_edits(rustroom_app::tool::Tool::Masks);
        assert_eq!(e.masks.len(), 2, "exactly one mask must have been dropped");
        let left: Vec<f32> = e.masks.iter().map(|m| m.adjust.exposure).collect();
        assert_eq!(left, vec![0.5, 2.5], "the wrong mask was dropped: {left:?}");
    }

    #[test]
    fn no_suppression_leaves_every_mask_in_the_view() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        let s = app.session.as_mut().unwrap();
        s.edits.masks = vec![masked_exposure(0.5), masked_exposure(1.5)];
        s.suppressed_mask = None;
        assert_eq!(s.view_edits(rustroom_app::tool::Tool::Masks).masks.len(), 2);
    }

    /// An index past the end is ignored rather than panicking — the mask list can shrink under a
    /// stale suppression between one frame and the next, exactly as the spot list can.
    #[test]
    fn a_stale_suppression_index_is_ignored() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        let s = app.session.as_mut().unwrap();
        s.edits.masks = vec![masked_exposure(0.5)];
        s.suppressed_mask = Some(7);
        assert_eq!(s.view_edits(rustroom_app::tool::Tool::Masks).masks.len(), 1);
    }

    /// A linear-gradient mask carrying one exposure value, so the tests above can tell the masks
    /// apart by what they do rather than by position alone.
    fn masked_exposure(ev: f32) -> edit_model::MaskedAdjustment {
        let mut m = edit_model::MaskedAdjustment::default();
        m.adjust.exposure = ev;
        m
    }
```

⚠ Check `MaskedAdjustment`'s real shape before writing this — the field holding the adjustments
may not be called `adjust`:

```bash
grep -n "pub struct MaskedAdjustment" -A 12 crates/edit-model/src/lib.rs
```

Use whatever the struct actually declares; do **not** invent a field name.

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test -p app --bin Rustroom suppressed_mask 2>&1 | tail -20
```

Expected: **compile error**, `no field \`suppressed_mask\``.

⚠ `--bin Rustroom`, not `--bin rustroom-gui` and not `--lib`: `session_flow` is a bin module. A
filter against the wrong target returns `0 passed; N filtered out` and reads as green.

- [ ] **Step 3: Add the field**

In `crates/app/src/main.rs`, immediately after the `suppressed_spot` field and its doc comment:

```rust
    /// The mask whose adjustments are held out of the view while its tint is shown.
    ///
    /// ⚠ **View state, never `Edits`** — `suppressed_spot`'s standing, applied in the same place
    /// and for the same reason. The sidecar, a preset, a paste, a sync, an export and the CLI
    /// never see it.
    ///
    /// ⚠⚠ **One derivation site, never a set/clear pair.** It is recomputed unconditionally each
    /// frame in `develop::body` from the same three facts that decide whether the tint is drawn.
    /// The spot-removal flicker bug was two owners of one suppression — the panel released what
    /// the canvas had suppressed — and the rule that came out of it was "an owner may release only
    /// what it suppressed". A single unconditional derivation cannot acquire a second owner, and
    /// cannot disagree with what is on screen.
    suppressed_mask: Option<usize>,
```

- [ ] **Step 4: Apply it in `view_edits`**

In `crates/app/src/session_flow.rs`, inside `view_edits`, immediately after the existing
`suppressed_spot` block and before `e`:

```rust
        // ⚠⚠ **Mask isolation.** While a mask's tint is shown, that mask's adjustments leave the
        // view, so painting changes no pixels and needs no render — which is what keeps the rung
        // from changing mid-stroke. Every other mask and every global edit still render.
        //
        // ⚠ Removed rather than neutralised, `suppressed_spot`'s reasoning: a mask contributes to
        // the picture only through its adjustments, so the two are the same thing and removal
        // cannot be half-done.
        if let Some(i) = self.suppressed_mask {
            if i < e.masks.len() {
                e.masks.remove(i);
            }
        }
```

- [ ] **Step 5: Add it to the exhaustive test fixture**

In `crates/app/src/test_app.rs`, beside `suppressed_spot: None,`:

```rust
        suppressed_mask: None,
```

⚠ That literal is exhaustive on purpose — a new `Session` field is a compile error there, forcing
a decision about what a test should see. Do not reach for `..Default::default()`.

- [ ] **Step 6: Add the one derivation site**

In `crates/app/src/develop/mod.rs`, inside `pub fn body`, immediately after
`app.select_tool(picked);` and **before** `panel::show` and `canvas::show`, both of which request
renders:

```rust
    // ⚠⚠ **The one derivation site for mask isolation.** Recomputed unconditionally every frame
    // from the same three facts that decide whether the tint is drawn, so the suppression and the
    // tint cannot disagree and no second owner can appear. See `Session::suppressed_mask`.
    //
    // ⚠ Placed before both panels: each requests renders through `view_edits`, and a value
    // written after them would be one frame stale exactly where it matters.
    let suppressed = if app.tool == rustroom_app::tool::Tool::Masks && app.show_mask_tint {
        app.session.as_ref().and_then(|s| s.selected_mask)
    } else {
        None
    };
    if let Some(s) = app.session.as_mut() {
        s.suppressed_mask = suppressed;
    }
```

- [ ] **Step 7: Write the carrier test**

⚠ **This is the test that matters.** W5's record: a milestone whose maths was fully proven shipped
100% non-functional because nothing checked that the data reached the tested code. Append to
`mod tests` in `crates/app/src/develop/mod.rs` (create the module if it has none):

```rust
#[cfg(test)]
mod tests {
    use crate::test_app;
    use eframe::egui;
    use rustroom_app::tool::Tool;

    /// Drive the real `develop::body` and report what `suppressed_mask` came out as.
    fn suppression_under(tool: Tool, show_tint: bool, selected: Option<usize>) -> Option<usize> {
        let ctx = egui::Context::default();
        let mut app = test_app::for_test();
        test_app::open_test_photo(&mut app, &ctx, 80, 40);
        app.tool = tool;
        app.show_mask_tint = show_tint;
        {
            let s = app.session.as_mut().unwrap();
            s.edits.masks = vec![edit_model::MaskedAdjustment::default()];
            s.selected_mask = selected;
        }
        let c = ctx.clone();
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1400.0, 1400.0),
                )),
                ..Default::default()
            },
            |ui| super::body(&mut app, ui, &c),
        );
        app.session.as_ref().unwrap().suppressed_mask
    }

    /// ⚠⚠ **The carrier.** The three facts reaching `Session::suppressed_mask` through the real
    /// `body`, which is the hop nothing else proves — the `view_edits` tests would all pass
    /// against a version where nobody ever writes the field.
    #[test]
    fn the_shown_tint_suppresses_the_selected_mask() {
        assert_eq!(suppression_under(Tool::Masks, true, Some(0)), Some(0));
    }

    /// Each of the three facts is load-bearing on its own. A test that only checked the positive
    /// case would pass against a version that suppressed unconditionally — which would hide a
    /// mask's effect in the Edit panel, where no tint is drawn to explain it.
    #[test]
    fn nothing_is_suppressed_without_all_three_facts() {
        assert_eq!(suppression_under(Tool::Edit, true, Some(0)), None);
        assert_eq!(suppression_under(Tool::Masks, false, Some(0)), None);
        assert_eq!(suppression_under(Tool::Masks, true, None), None);
    }
}
```

⚠ The harness screen is **1400 pt**, not 700. `test_ui::screen`'s own doc records that a widget
laid out past the bottom of the rect is outside the window and reads as "the button did nothing";
the mask panel already grew past 700 once.

- [ ] **Step 8: Run every test in this task**

```bash
cargo test -p app --bin Rustroom suppressed_mask 2>&1 | tail -8
cargo test -p app --bin Rustroom suppresses_the_selected_mask 2>&1 | tail -8
cargo test -p app --bin Rustroom without_all_three_facts 2>&1 | tail -8
```

Expected: `3 passed`, `1 passed`, `1 passed`. ⚠ Read each passed-count.

- [ ] **Step 9: Sabotage-verify, twice**

1. Delete the `if let Some(i) = self.suppressed_mask` block from `view_edits`. The three
   `view_edits` tests must fail. Restore.
2. Replace the derivation in `body` with `let suppressed = None;`. **The carrier must fail and the
   `view_edits` tests must still pass** — that asymmetry is the whole point of the carrier
   existing. Restore.

- [ ] **Step 10: Gate and commit**

```bash
git add -u
git commit -F - <<'MSG'
Mask isolation: the shown mask's adjustments leave the view

While a mask's tint is shown, that mask's adjustments are dropped from
view_edits, so the picture under the brush is the one the mask was
painted against and painting changes no pixels. Every other mask and
every global edit render normally.

Session::suppressed_mask is suppressed_spot's mirror, which already does
this for spot removal, and it is applied in the same place. Removed rather
than neutralised: a mask contributes only through its adjustments.

One derivation site, recomputed unconditionally each frame from the same
three facts that decide whether the tint is drawn. The spot-removal
flicker bug was two owners of one suppression; a single unconditional
derivation cannot acquire a second.

The carrier test is the one that matters. Sabotaging the derivation leaves
every view_edits test green and fails only that one, which is the shape
that let creative LUTs ship fully tested and fully dead.
MSG
```

---

## Task 4: painting requests no render while the mask is suppressed

**Why:** with the mask suppressed the view edits are unchanged, so both request sites would
re-render an identical picture — the motion site on **every drag frame**, measured at ~16.6 ms at
1:1.

**Files:**
- Modify: `crates/app/src/develop/canvas/mask_overlay.rs` (both `s.request` sites)

- [ ] **Step 1: Write the failing test**

Append to `mod tests` in `crates/app/src/develop/canvas/mask_overlay.rs`:

```rust
    /// ⚠⚠ **Sampled on the release frame itself.** A later frame issues a request of its own once
    /// `Edits` has changed, so a count taken afterwards rises either way and the test passes with
    /// the guard deleted — the exact failure the sibling test above records.
    #[test]
    fn a_stroke_on_a_suppressed_mask_asks_for_no_render() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        with_selected_mask(&mut app);
        {
            let s = app.session.as_mut().unwrap();
            s.suppressed_mask = Some(0);
        }
        app.tool = rustroom_app::tool::Tool::Masks;
        app.show_mask_tint = true;

        let before = app.session.as_ref().unwrap().engine.requests_issued();
        let centre = canvas_centre(&mut app, &ctx);
        let _ = drag_through(&mut app, &ctx, centre, centre + egui::vec2(20.0, 8.0));
        let after = app.session.as_ref().unwrap().engine.requests_issued();

        assert_eq!(
            before, after,
            "a stroke on a suppressed mask issued {} render(s); with the mask held out of the \
             view the picture cannot change, so the rung must never be disturbed",
            after - before
        );
        assert!(
            app.session.as_ref().unwrap().history.can_undo(),
            "the release must still have committed one undo step"
        );
    }
```

⚠ `with_selected_mask`, `canvas_centre` and `drag_through` already exist in that module's tests
(`grep -n "fn with_selected_mask\|fn canvas_centre\|fn drag_through" crates/app/src/develop/canvas/mask_overlay.rs`).
Reuse them; do not write a second harness — a harness duplicated per test is how the two drift.

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test -p app --bin Rustroom a_stroke_on_a_suppressed_mask 2>&1 | tail -15
```

Expected: FAIL, `a stroke on a suppressed mask issued 2 render(s)` (or 1 — the count depends on
how many drag frames the harness produces; any non-zero difference is the failure).

- [ ] **Step 3: Add the guard**

In `crates/app/src/develop/canvas/mask_overlay.rs`, above `draw_mask_overlay`, add:

```rust
/// Ask for a render unless this mask is being held out of the view.
///
/// ⚠⚠ **This is what makes painting free.** With the mask suppressed the view edits are
/// unchanged, so a request would re-render a byte-identical picture — on the motion site that is
/// every drag frame, measured at ~16.6 ms at 1:1. Skipping it is also what keeps the rung from
/// changing: the canvas drops a rung for a live gesture, and a gesture that asks for nothing
/// leaves the rung where it was.
///
/// ⚠ One helper rather than the same condition written at both call sites: the motion render and
/// the release render must agree about this, and two copies is how they would stop.
fn request_unless_suppressed(s: &Session, tool: Tool, mask: usize) {
    if s.suppressed_mask == Some(mask) {
        return;
    }
    s.request(s.view_edits(tool), s.render_max_dim(), s.requested_region);
}
```

Then replace **both** call sites in that file:

- inside the `drag_stopped` branch, `s.request(s.view_edits(tool), s.render_max_dim(), s.requested_region);`
  becomes `request_unless_suppressed(s, tool, i);`
- in the trailing `if moved { … }` block, the same substitution.

⚠ The history push in the `drag_stopped` branch stays **unguarded**. A stroke is still an edit and
must still be undoable; only the render is skipped.

⚠ Check the exact imports the file already has for `Session` and `Tool` before writing the
signature — it may refer to them by a different path.

- [ ] **Step 4: Run the tests**

```bash
cargo test -p app --bin Rustroom mask_overlay 2>&1 | tail -8
```

Expected: every pre-existing test in that module still passes, plus the new one. ⚠ The
pre-existing `a_brush_drag_asks_for_a_render` must **still pass** — it uses no suppression, and if
it fails the guard is firing when it should not.

- [ ] **Step 5: Sabotage-verify**

Change the guard body to `if false { return; }`. The new test must fail. Restore.

- [ ] **Step 6: Gate and commit**

```bash
git add -u
git commit -F - <<'MSG'
A stroke on a suppressed mask asks for no render

With the mask held out of the view the picture cannot change, so both
request sites would re-render a byte-identical frame — the motion site on
every drag frame, measured at ~16.6 ms at 1:1.

One helper rather than the condition written twice: the motion render and
the release render must agree about this.

The history push stays unguarded. A stroke is still an edit and must still
be undoable; only the render is skipped.

The count is sampled on the release frame itself. A later frame issues a
request of its own once Edits has changed, so a count taken afterwards
rises either way and the test would pass with the guard deleted.
MSG
```

---

## Task 5: `navigation::pan_button_down`, and the rung rule

**Why:** mask isolation alone does **not** stop the rung ladder. `is_decidedly_dragging()` is true
while painting, so the canvas still drops a rung and `max_dim != s.render_rung` still forces a
render. And the same disjunct fires on the first frame of a pan even when `covered` is true,
discarding a texture that already covers the viewport.

**Files:**
- Modify: `crates/app/src/develop/canvas/navigation.rs`
- Modify: `crates/app/src/develop/canvas/mod.rs:83` and `:170-232`

⚠ One commit: `pan_button_down` is a `pub(crate) fn` in a **binary** crate, so it is dead code
under `-D warnings` until its caller exists.

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `crates/app/src/develop/canvas/navigation.rs`:

```rust
    /// Whether `pan_button_down` fires for `button` under `binding`, with the pan button held.
    fn pan_down_for(binding: PanDrag, button: egui::PointerButton) -> bool {
        let ctx = egui::Context::default();
        let mut out = false;
        let pos = egui::pos2(100.0, 100.0);
        for pass in 0..2 {
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(400.0, 400.0),
                )),
                ..Default::default()
            };
            input.events.push(egui::Event::PointerMoved(pos));
            if pass == 0 {
                input.events.push(egui::Event::PointerButton {
                    pos,
                    button,
                    pressed: true,
                    modifiers: Default::default(),
                });
            }
            let c = ctx.clone();
            let _ = ctx.run_ui(input, |_ui| {
                if pass == 1 {
                    out = c.input(|i| pan_button_down(i, binding, false));
                }
            });
        }
        out
    }

    /// ⚠ The predicate must agree with `pan_delta` about which button pans, or the interactive
    /// rung would exempt a gesture that is not a pan — or fail to exempt one that is.
    #[test]
    fn the_pan_predicate_matches_the_binding() {
        assert!(pan_down_for(PanDrag::Middle, egui::PointerButton::Middle));
        assert!(!pan_down_for(PanDrag::Middle, egui::PointerButton::Primary));
        assert!(pan_down_for(
            PanDrag::Secondary,
            egui::PointerButton::Secondary
        ));
        assert!(!pan_down_for(
            PanDrag::Secondary,
            egui::PointerButton::Middle
        ));
    }

    /// ⚠ Space+left pans only while nothing has keyboard focus — the rule `pan_delta` already
    /// follows, so a space typed into the preset-name field cannot arm panning.
    #[test]
    fn space_left_needs_the_space_key_and_no_text_focus() {
        // With no space held, a primary drag is not a pan under any binding.
        assert!(!pan_down_for(
            PanDrag::SpaceLeft,
            egui::PointerButton::Primary
        ));
    }
```

- [ ] **Step 2: Run to verify they fail**

```bash
cargo test -p app --bin Rustroom pan_predicate 2>&1 | tail -15
```

Expected: **compile error**, `cannot find function \`pan_button_down\``.

- [ ] **Step 3: Write `pan_button_down`**

In `crates/app/src/develop/canvas/navigation.rs`, immediately above `pan_delta`:

```rust
/// Whether the configured pan button is currently held.
///
/// ⚠⚠ **A predicate on the INPUT, not on a `Response`**, which is what lets the interactive
/// rung's gate consult it before any widget has been allocated. `pan_delta` answers the same
/// question from a `Response` because it also needs the delta; these two must never disagree
/// about which button pans, which is why they sit next to each other.
///
/// ⚠ `text_focused` is threaded through for the same reason `pan_delta` takes it: without it a
/// space typed into the Save-preset field would read as an armed pan.
pub(crate) fn pan_button_down(
    input: &egui::InputState,
    binding: PanDrag,
    text_focused: bool,
) -> bool {
    match binding {
        PanDrag::Middle => input.pointer.button_down(egui::PointerButton::Middle),
        PanDrag::Secondary => input.pointer.button_down(egui::PointerButton::Secondary),
        PanDrag::SpaceLeft => {
            !text_focused
                && input.key_down(egui::Key::Space)
                && input.pointer.button_down(egui::PointerButton::Primary)
        }
    }
}
```

⚠ Confirm the space-key check matches what `pan_delta` uses — read the lines above it and copy the
same expression rather than inventing one.

- [ ] **Step 4: Write the rung-rule tests**

Append to `mod tests` in `crates/app/src/develop/canvas/mod.rs`:

```rust
    /// ⚠⚠ **The defect, as a test.** The request condition ends in
    /// `|| max_dim != s.render_rung`, which fires even when `covered` is true — so the first
    /// frame of a pan discarded a texture that already covered the viewport and replaced it with
    /// one a rung lower, for nothing.
    #[test]
    fn a_pan_inside_the_margin_asks_for_nothing_and_leaves_the_rung_alone() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        app.settings.drag_quality = rustroom_app::settings::DragQuality::Faster;
        let (before_rung, before_reqs) = {
            let s = app.session.as_ref().unwrap();
            (s.render_rung, s.engine.requests_issued())
        };
        let centre = canvas_centre(&mut app, &ctx);
        // A middle-button drag is a pan under the default binding.
        let _ = drag_through_with(
            &mut app,
            &ctx,
            centre,
            centre + egui::vec2(6.0, 4.0),
            egui::PointerButton::Middle,
        );
        let s = app.session.as_ref().unwrap();
        assert_eq!(
            s.render_rung, before_rung,
            "a pan changed the render rung, so the whole view went soft for nothing"
        );
        assert_eq!(
            s.engine.requests_issued(),
            before_reqs,
            "a pan inside the rendered margin asked for a render"
        );
    }

    /// ⚠ The other half, and it is what stops the fix disabling a feature confirmed by use. A test
    /// that only checked the pan would pass against a version that never drops the rung at all.
    #[test]
    fn a_slider_drag_still_drops_the_rung() {
        assert_eq!(
            drop_for(true, None),
            1,
            "a drag that changes the picture must still drop a rung — the interactive rung took \
             a masked-exposure drag from 137.9 ms to 40.0 ms and is confirmed by use"
        );
        assert_eq!(drop_for(false, None), 0, "nothing is being dragged");
        assert_eq!(
            drop_for(true, Some(0)),
            0,
            "a gesture on a suppressed mask changes no pixels"
        );
    }
```

⚠ `drag_through_with` and `drop_for` do not exist yet. Write them as small helpers in the same
test module: `drag_through_with` is `drag_through` with the button as a parameter (refactor
`drag_through` to delegate to it, keeping its existing signature so no existing test changes), and
`drop_for(dragging, suppressed)` calls the pure helper introduced in Step 5. If `drag_through`'s
current shape makes the button hard to thread, write the four-pass gesture inline in the new test
instead — but do **not** change what any existing test asserts.

- [ ] **Step 5: Write the rung rule**

In `crates/app/src/develop/canvas/mod.rs`, add a pure helper above `pub fn show`:

```rust
/// How many rungs a live gesture should drop.
///
/// ⚠⚠ **The interactive drop engages for a drag that changes the PICTURE, not one that only
/// changes the view.** A pan changes which part of the frame is on screen; a gesture on a mask
/// whose adjustments are held out of the view changes nothing at all. Neither needs a render, and
/// dropping the rung for one discards a texture that already covers the viewport and replaces it
/// with a softer one for nothing.
///
/// ⚠⚠ **Still a question about the gesture, never about the render, which is what keeps it from
/// oscillating.** The 2026-08-13 progressive-fit feature asked "is the showing resolution not yet
/// sharp?" — a question the render itself answers, so the render answered it, the gate flipped,
/// and it counted that as interaction. Nothing here can be affected by what a render produces.
///
/// ⚠ A slider drag must still drop, which is the half that is confirmed by use: 137.9 ms to
/// 40.0 ms, 7.2 to 25 fps. Every non-canvas request site renders at `render_max_dim()`, i.e.
/// whatever rung the canvas last set, so a rule that simply stopped the canvas requesting would
/// disable the feature outright.
fn rungs_to_drop(dragging: bool, quality_drop: u32, view_only_gesture: bool) -> u32 {
    if !dragging || view_only_gesture {
        0
    } else {
        quality_drop
    }
}
```

Then rewrite the gate at `crates/app/src/develop/canvas/mod.rs:83`:

```rust
    // ⚠ Hoisted with the rest, i.e. read before `app.session` is borrowed mutably below.
    let app_suppressed_mask = app.session.as_ref().and_then(|s| s.suppressed_mask);
    let text_focused = ui.ctx().memory(|m| m.focused()).is_some();
    // ⚠ The press ORIGIN, not the current pointer position: a drag that began on a panel slider
    // and wandered over the canvas is still a slider drag and must still drop the rung.
    let press_on_canvas = ui
        .ctx()
        .input(|i| i.pointer.press_origin())
        .is_some_and(|p| ui.max_rect().contains(p));
    let panning = ui
        .ctx()
        .input(|i| navigation::pan_button_down(i, app_pan_drag, text_focused));
    let view_only_gesture =
        press_on_canvas && (panning || app_suppressed_mask.is_some());
    let app_drag_drop = rungs_to_drop(
        ui.ctx().input(|i| i.pointer.is_decidedly_dragging()),
        app.settings.drag_quality.rungs_dropped(),
        view_only_gesture,
    );
```

⚠ Keep the existing block of `⚠` comments above that gate — every one of them still holds — and
add the new reasoning rather than replacing it.

⚠ `app_pan_drag` is already hoisted a few lines above; do not hoist it twice.

- [ ] **Step 6: Run the tests**

```bash
cargo test -p app --bin Rustroom a_pan_inside_the_margin 2>&1 | tail -8
cargo test -p app --bin Rustroom a_slider_drag_still_drops 2>&1 | tail -8
cargo test -p app --bin Rustroom the_pan_predicate 2>&1 | tail -8
```

Expected: `1 passed`, `1 passed`, `1 passed`.

- [ ] **Step 7: Sabotage-verify, twice**

1. `fn rungs_to_drop(...) -> u32 { if !dragging { 0 } else { quality_drop } }` — i.e. drop the
   exemption. `a_pan_inside_the_margin_asks_for_nothing_and_leaves_the_rung_alone` and the
   suppressed case of `a_slider_drag_still_drops_the_rung` must fail.
2. `fn rungs_to_drop(...) -> u32 { 0 }` — i.e. disable the feature.
   `a_slider_drag_still_drops_the_rung` must fail. **This is the sabotage that proves the fix did
   not silently delete a feature confirmed by use.**

Restore after each and check `git status`.

- [ ] **Step 8: Gate and commit**

```bash
git add -u
git commit -F - <<'MSG'
The rung drops for a gesture that changes the picture, not the view

The request condition ends in `|| max_dim != s.render_rung`, which fires
even when `covered` is true — so the first frame of a pan discarded a
texture that already covered the viewport and rendered a softer one for
nothing. The same applied to a paint stroke once the mask is suppressed,
which is why mask isolation alone did not keep the resolution: the rung
still dropped and the rung change still forced a render.

The exemption is a pan gesture, or any canvas gesture on a mask whose
adjustments are held out of the view, and only when the drag began on the
canvas — a drag that started on a slider and wandered over the image is
still a slider drag.

Still a question about the gesture, never about the render. The 2026-08-13
progressive-fit feature asked a question the render itself answered, so
the render answered it and the gate oscillated. Nothing here can be.

pan_button_down lands with its caller: a pub(crate) fn in a binary crate
is dead code under -D warnings until something calls it.
MSG
```

---

## Task 6: the handover requests at the current rung

**Why:** at 7.72× zoom the full-resolution handover landed an **8.9× magnified 1600 px texture,
held for 18 frames** — four times the softening `Faster` is meant to give, once per photo.

**Files:**
- Modify: `crates/app/src/session_flow.rs:552-560`

- [ ] **Step 1: Write the failing test**

Append to `mod tests` in `crates/app/src/session_flow.rs`:

```rust
    /// ⚠⚠ **The handover must not throw the view back to fit resolution.** The explicit request is
    /// necessary — without it the canvas sees an unchanged rung and a covered region and asks for
    /// nothing, so the upgrade is invisible. Only the resolution was wrong: at 7.72x zoom
    /// `fit_cap` is an 8.9x magnified blur, measured held for 18 frames.
    #[test]
    fn the_handover_requests_at_the_current_rung() {
        let ctx = egui::Context::default();
        let mut app = crate::test_app::for_test();
        crate::test_app::open_test_photo(&mut app, &ctx, 80, 40);
        {
            let s = app.session.as_mut().unwrap();
            s.render_rung = 64;
        }
        // The rung the handover asks for, read back from what it stored.
        app.pump_full_res(&ctx);
        let s = app.session.as_ref().unwrap();
        assert_ne!(
            s.render_rung,
            s.fit_cap(),
            "the handover reset the view to fit resolution, which at zoom is an 8.9x blur"
        );
    }
```

⚠ This test only means something if the handover actually runs. `pump_full_res` drains
`self.full_decoder`, so the fixture must have a full-resolution result waiting. Check how
`test_app` injects decodes (`grep -n "full_decoder" crates/app/src/test_app.rs`) and seed one; if
that is not reachable from the fixture, **say so and assert on a extracted pure helper instead** —
add `fn handover_request(render_rung: usize, fit_cap: usize) -> usize` returning `render_rung`, test
that, and call it from `pump_full_res`. Do **not** write a test that passes without the handover
having run; add a fixture check (`assert!(handover_ran, "this test proves nothing")`) as the
creative-LUT handover tests do.

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test -p app --bin Rustroom the_handover_requests_at_the_current_rung 2>&1 | tail -15
```

Expected: FAIL, the rung equal to the fit cap.

- [ ] **Step 3: Change the request**

In `crates/app/src/session_flow.rs`, replace:

```rust
                    s.render_rung = s.fit_cap();
                    s.request(s.view_edits(tool), s.fit_cap(), None);
```

with:

```rust
                    // ⚠⚠ **At the CURRENT rung, not the fit cap.** The explicit request is
                    // necessary — without it the canvas sees an unchanged rung and a covered
                    // region and asks for nothing, so the upgrade is invisible until something
                    // else happens to trigger a render. Only the resolution was wrong: at 7.72x
                    // zoom `fit_cap` is an 8.9x magnified blur, and it was measured held for 18
                    // frames on every photo.
                    //
                    // ⚠ The region stays `None`. `rendered_region` and `requested_region` were
                    // just cleared, because region rects are in *rendered* pixels and those
                    // coordinates change with the buffer — so there is no region left that
                    // describes anything. The canvas re-derives one on its next frame; this
                    // request exists only to make the upgrade visible.
                    s.request(s.view_edits(tool), s.render_rung, None);
```

⚠ Delete the `s.render_rung = s.fit_cap();` line. It exists to make the canvas re-request, which
the cleared regions already achieve, and keeping it would make the request above ask for the fit
cap again by another route.

- [ ] **Step 4: Run the test**

```bash
cargo test -p app --bin Rustroom the_handover_requests_at_the_current_rung 2>&1 | tail -8
```

Expected: `1 passed`.

- [ ] **Step 5: Sabotage-verify**

Put `s.fit_cap()` back as the request's `max_dim`. The test must fail. Restore.

- [ ] **Step 6: Gate and commit**

```bash
git add -u
git commit -F - <<'MSG'
The full-resolution handover requests at the current rung

At 7.72x zoom it landed an 8.9x magnified 1600 px texture and held it for
18 frames — four times the softening Faster is meant to give, once per
photo, measured over 836 instrumented frames.

The explicit request stays: without it the canvas sees an unchanged rung
and a covered region and asks for nothing, so the upgrade is invisible.
Only the resolution was wrong.

render_rung is no longer reset to the fit cap. Clearing the region rects
already forces the canvas to re-request, and the reset would have asked
for the fit cap again by another route.
MSG
```

---

## Task 7: measure whether the drop is worth anything while panning

**Why:** the interactive rung's 137.9 ms was measured on a **fitted, whole-image** render of
6.8 Mpx. Zoomed, a render is region-bounded to the viewport — at the reporter's zoom, 951×539.
Whether the drop is worth its blur there is a number nobody has.

**Files:**
- Create: `crates/app/examples/bench_pan_rung.rs`

- [ ] **Step 1: Write the probe**

```rust
//! What does dropping a rung actually buy while PANNING at zoom?
//!
//! Run: `cargo run --release -p app --example bench_pan_rung <raw>`
//!
//! ⚠⚠ **This exists to settle one decision and nothing else.** The interactive rung was measured
//! at 137.9 ms on a *fitted, whole-image* render of 6.8 Mpx, and that is the number its design
//! rests on. Zoomed, a render is region-bounded to the viewport: at the reporter's 7.72x on a
//! 2167x1229 pt canvas the region is 951x539 rendered pixels, about 0.5 Mpx. If dropping a rung
//! there is worth single-digit milliseconds it is not worth a 2x blur across the whole view.
//!
//! ⚠ Panning changes no edits, so the prefix is **cached** and only the tail runs. The prefix is
//! therefore built once per rung and excluded from the timing, which is what makes this the cost
//! of a pan rather than the cost of a first render.
//!
//! ⚠ Read the two rungs as a RATIO, not as a prediction of a frame time. `bench_masked`'s own
//! note applies: a synthetic bench prices a stage, not a session, and it understated the running
//! app by 2.6x once.
use edit_model::Edits;
use pipeline::RenderScale;
use std::time::Instant;

const FIT_CAP: usize = 1600;
/// The reporter's canvas and zoom, from the `RUSTROOM_DEBUG_PLACE` log.
const AVAIL: (f32, f32) = (2167.2, 1229.5);
const FIT: f32 = 1.151217;
const ZOOM: f32 = 7.7168;

fn best_of<T>(n: usize, mut f: impl FnMut() -> T) -> (T, f64) {
    let mut best = f64::INFINITY;
    let mut out = None;
    for _ in 0..n {
        let t = Instant::now();
        let v = f();
        best = best.min(t.elapsed().as_secs_f64() * 1000.0);
        out = Some(v);
    }
    (out.unwrap(), best)
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: bench_pan_rung <raw>");
    let path = std::path::Path::new(&path);
    let opts = raw_decode::DecodeOptions::linear_scene();
    let d = raw_decode::decode_raw(path, &opts).expect("decode");
    let lens = d.metadata.lens.as_ref().and_then(|l| {
        lens_profile::Catalogue::shared()
            .and_then(|db| db.find(&d.metadata.make, &d.metadata.model, &l.name))
            .map(|profile| lens_profile::LensShot {
                profile,
                shot: lens_profile::Shot {
                    focal: l.focal,
                    aperture: l.aperture,
                },
            })
    });
    let buf = std::sync::Arc::new(d.image);
    let buffer_long = buf.width().max(buf.height());
    println!("buffer {}x{}", buf.width(), buf.height());

    // ⚠ Lens corrections ON and a realistic tail, as the GUI has them: a bench on `Edits::default()`
    // prices a pipeline nobody runs.
    let edits = Edits {
        lens_correction: Some(edit_model::LensCorrection {
            distortion: true,
            tca: true,
            vignetting: true,
            scale: 0.0,
        }),
        exposure: 0.35,
        contrast: 20.0,
        sharpening: edit_model::Sharpening {
            amount: 40.0,
            ..edit_model::Sharpening::default()
        },
        ..Edits::default()
    };

    for rung in [3200usize, 6246.min(buffer_long)] {
        let scale = RenderScale::new(rung as f32 / buffer_long as f32);
        let k = rung as f32 / FIT_CAP as f32;
        let px_per_rendered = FIT * ZOOM / k;
        let rw = (AVAIL.0 / px_per_rendered).ceil() as usize;
        let rh = (AVAIL.1 / px_per_rendered).ceil() as usize;
        let region = rustroom_app::view_region::RegionRect {
            x: 100,
            y: 100,
            w: rw,
            h: rh,
        };
        let (prefix, _) = pipeline::build_pipeline_split_in_region_with_brush(
            &edits,
            scale,
            None,
            None,
            pipeline::RenderSpec::default(),
            lens.clone(),
            pipeline::ProfileRendering::default(),
        );
        let mid = prefix.run(core_image::ImageBuf::clone(&buf).downscale_to(rung));
        let (_, tail_ms) = best_of(5, || {
            let (_, tail) = pipeline::build_pipeline_split_in_region_with_brush(
                &edits,
                scale,
                Some(region),
                None,
                pipeline::RenderSpec::default(),
                lens.clone(),
                pipeline::ProfileRendering::default(),
            );
            tail.run(mid.clone())
        });
        println!(
            "  rung {rung:>5}  region {rw}x{rh} ({:.2} Mpx)  TAIL {tail_ms:8.1} ms",
            (rw * rh) as f64 / 1e6
        );
    }
    println!(
        "\nPanning changes no edits, so this tail is the whole per-frame cost. If the two rows are\n\
         within a few ms, the rung drop is buying nothing while panning and costs a 2x blur."
    );
}
```

⚠ `build_pipeline_split_in_region_with_brush`'s exact parameter list must be copied from
`bench_crop_rung.rs`, which already calls it — do not guess the argument order. And check whether
`RegionRect` is re-exported from `rustroom_app::view_region` or must come from `render_engine`.

- [ ] **Step 2: Build and run it**

```bash
cargo run --release -p app --example bench_pan_rung ~/Desktop/fuji/DSCF6223.RAF 2>&1 | tail -10
```

- [ ] **Step 3: Decide from the number, and write the decision down**

- If the two rungs are **within ~5 ms**: extend the `view_only_gesture` exemption in
  `rungs_to_drop` to cover panning **unconditionally** — it already does, via `panning`; keep it,
  and record the measured number in the function's doc comment.
- If the drop is worth **more than ~15 ms**: the pan exemption is too expensive. Narrow
  `view_only_gesture` to `press_on_canvas && app_suppressed_mask.is_some()` — dropping `panning`
  from it — so painting stays sharp and panning keeps the drop. Record the number and the reversal
  in the doc comment and in the spec's §4.3.
- In between: keep the exemption and say so, with the number.

⚠ **Whichever way it goes, the number goes into the doc comment.** This project has five recorded
cases of a cost being reasoned wrong and measured right; a constant defended by an argument rather
than a measurement is the thing being avoided.

- [ ] **Step 4: Gate and commit**

```bash
git add crates/app/examples/bench_pan_rung.rs
git add -u
git commit -F - <<'MSG'
Measure what a rung drop buys while panning at zoom

The interactive rung's 137.9 ms was measured on a fitted whole-image
render of 6.8 Mpx. Zoomed, a render is region-bounded to the viewport —
951x539 at the reporter's zoom, about 0.5 Mpx — and panning changes no
edits, so the prefix is cached and the tail is the whole per-frame cost.

The probe is kept: this is a decision that should be re-made against a
number when the tail changes, not re-argued.
MSG
```

---

## Task 8: the GUI round, and the write-up

⚠ **This project's record says every defect in canvas and panel work is found by using it, not by
a test — thirteen times.** The tests above cover wiring; they cover no appearance and no feel.

**Files:**
- Modify: `CLAUDE.md` (a new section, and the ▶ NEXT ACTION block)

- [ ] **Step 1: Launch the GUI**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
RUSTROOM_DEBUG_PLACE=1 cargo run --release -p app
```

- [ ] **Step 2: The checklist to put in front of the user**

1. **A brush mask with "Show mask" on, at 1:1.** The mask's own adjustment should vanish while the
   tint is up, and the photo under the brush should stay **sharp for the whole stroke** — no soft
   phase, no sharpening on release.
2. **⚠⚠ The reported shift.** Draw at zoom and watch one high-contrast edge across the whole
   stroke. The measurements say nothing moves; if something still does, it is not the render, and
   the next things to instrument are the tint's placement grid and the region-scoped coverage
   rasterisation. **Report it either way** — a "no longer visible" is the confirmation, and a
   "still there" is the finding.
3. **Untick "Show mask".** The mask's effect must come straight back, at full quality.
4. **A slider belonging to the shown mask.** It does nothing visible — the accepted caveat. Confirm
   it feels like a rule rather than a bug.
5. **Pan at 1:1 with the mask hidden.** Small pans inside the margin should now stay sharp. Larger
   ones behave according to Task 7's measurement.
6. **Open a fresh photo and wait ~4 s at 1:1** for the full-resolution handover. It should sharpen,
   not blur first.
7. **A global slider with "Show mask" on.** It must still drop a rung and still feel responsive —
   this is the half that could have been silently deleted.

- [ ] **Step 3: Write the CLAUDE.md section**

Add a section titled **"Mask isolation, and the placement shift that was not there"** immediately
below the ▶ RESUME HERE block's most recent entry. It must record, at minimum:

- The **measurement that overturned the brief**: placement rung-invariant to 0.25 px, exactly zero
  vertically and why that is structural; content rung-invariant to 0.020 fit px with the lens-off
  control at exactly zero. Both instruments kept, and how to run them.
- That the hypothesis this work started from — `fit_image_size` drifting per rung — was **real and
  far too small**, so nobody re-derives it as the cause.
- That **mask isolation alone does not stop the rung ladder**, and why: `is_decidedly_dragging()`
  is true while painting.
- The **accepted caveat** that a slider on the shown mask does nothing visible.
- The **panning measurement** from Task 7 and the decision it produced.
- The two files past the ~600-line marker that grew: `develop/canvas/mod.rs` and `main.rs`.

⚠ Update the ▶ NEXT ACTION block: with auto-mask and this landed, **M7.6 is closed**. The next
actions after it are **M7.1c** boolean mask operations, **M7.3b** fusing perspective into
`GeometryStage`, then **Phase 8** opening with **M8.4, the demosaic decision**.

⚠ `grep -n "Co-Authored-By" CLAUDE.md docs/superpowers/plans/2026-09-09-mask-isolation.md` must
return nothing before committing. A plan document carried one once, because both the template and
the harness reminder show it.

- [ ] **Step 4: Gate and commit**

```bash
git add -u
git commit -F - <<'MSG'
CLAUDE.md: mask isolation, and the placement shift that was not there

Records the measurement that overturned the brief. The photograph does not
move at a rung transition: placement is rung-invariant to 0.25 px and
exactly zero vertically, which is structural since fit = avail.y / full.y
cancels full.y; content is rung-invariant to 0.020 fit px with the
lens-off control at exactly zero.

The hypothesis this work started from was real and two orders of magnitude
too small. Recorded so nobody re-derives it as the cause.

Also records that mask isolation alone does not stop the rung ladder,
which is the thing most likely to be assumed.
MSG
```

---

## Self-review

**Spec coverage.** §3 mask isolation → Tasks 3 and 4. §4 the rung rule → Task 5, with §4.3's
measurement as Task 7. §5 `fit_image_size` → Tasks 1 and 2. §6 the handover → Task 6. §7 testing →
distributed through every task, with the goldens asserted at each gate. §2's honest record of the
measurements → Task 8's write-up. §8's caveats → Task 8's checklist item 4.

**Placeholder scan.** No "TBD" or "handle edge cases". Three steps say "check the real shape before
writing this" and name the exact `grep` — that is a verification instruction with a command, not a
placeholder, and it is there because this plan's code was written from reading the tree rather than
from memory and two struct shapes could not be confirmed without opening files the plan should not
inline wholesale.

**Type consistency.** `fit_size` is the function in `view_region` **and** the field on
`RenderedFrame`; the field is set from the function at both construction sites. `suppressed_mask`
is spelled identically in `main.rs`, `session_flow.rs`, `test_app.rs`, `develop/mod.rs`,
`mask_overlay.rs` and `canvas/mod.rs`. `rungs_to_drop` and `pan_button_down` are each defined once
and called once. `Orientation::transposes` is defined in Task 1 and used only there.

**Known gaps, stated rather than hidden.** Task 6's test may not be reachable from `test_app` if
the fixture cannot seed a full-resolution decode; the step says what to do instead and forbids a
test that passes without the handover running. Task 5's `drag_through_with` may need a refactor of
an existing helper; the step forbids changing what any existing test asserts.
