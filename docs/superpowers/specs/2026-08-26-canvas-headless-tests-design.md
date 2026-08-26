# Headless tests for canvas and panel code — design

**Date:** 2026-08-26
**Scope:** a reusable test fixture that drives `crates/app`'s real canvas code headlessly, plus the
first regression tests written against it.
**Not a feature.** This is test infrastructure, taken first *because* the next phase adds more UI.

## The problem, stated precisely

CLAUDE.md has recorded eleven times that a GUI defect was found by using the app and not by a test.
Measured rather than asserted, the picture is narrower and more actionable than that suggests:

| function in `develop/canvas.rs` | lines | tests |
|---|---|---|
| `show` — the canvas itself | 238 | **0** |
| `draw_mask_overlay` | 224 | **0** |
| `draw_crop_overlay` | 92 | **0** |
| `paint_tint` / `paint_mask` | ~190 | **0** |
| `tool_drag` + `pan_delta` | ~50 | 11 |

⚠ **The harness is not unused — it is used wherever a function was already small and pure enough to
call.** Every untested function takes `&mut RustroomApp` and needs a live `Session`, which needs a
render-engine worker thread and GPU textures. **That barrier is the actual problem**, and it is why
the eleven existing tests cluster on two thirty-line helpers.

## Feasibility — established before designing

- `RenderEngine::new(Arc<ImageBuf>, f32, Option<LensShot>, on_done)` takes a plain buffer. A
  synthetic 8×4 `ImageBuf` satisfies it.
- `ctx.load_texture` works on a headless `egui::Context` — `thumb.rs`'s existing tests prove it.
- `RustroomApp` has **43 fields**, of which only four are awkward: two `DecodeWorker`s, which
  already take an *injected* decode fn, and two stores, which already take an injected path.

**So the fixture needs no disk, no libraw and no GPU.** Nothing in production has to change to make
it possible.

## Approach

Chosen: **a test-only app constructor, plus opportunistic extraction of the coordinate maths.**

Rejected: *narrowing the seam* — refactoring `show` to take a small `CanvasCtx` so tests build less.
It is a real refactor of the largest, least-covered function in the crate, i.e. changing untested
code in order to test it. And three of the six known bugs were in the **wiring between pieces**,
which is exactly what a narrower seam hides.

The fixture tests the code **as it ships**.

## 1. The fixture

Two entry points, in a `#[cfg(test)]` module shared by the canvas and panel tests:

```rust
fn for_test(ctx: &egui::Context) -> RustroomApp;              // no session
fn open_test_photo(app: &mut RustroomApp, ctx: &egui::Context, w: usize, h: usize);
```

- The decode fn is a stub returning a synthetic buffer; the stores point at temp directories.
- `open_test_photo` installs a `Session` over a synthetic `ImageBuf`, textures loaded through
  `ctx.load_texture`.
- The render engine's worker thread is **real and does spawn**. Tests never wait on it — they
  assert on requests and state, never on rendered pixels.

⚠ **Temp-dir uniqueness must be structural** — process id plus a monotonic counter, never the clock.
Two tests entering the same tick collide, `cargo test` runs them in parallel, and this project has
already paid for that twice (`image-decode`'s `temp_path`, and `app`'s `TempDir` tag reuse, the
latter surfacing as an *unrelated pre-existing test* failing during a sabotage run).

## 2. The request counter

`RenderEngine` gains an `AtomicUsize`, incremented in `request`, read by `requests_issued()`.
`request` already takes `&self`, so an atomic fits with no signature change.

⚠⚠ **It must NOT be `#[cfg(test)]`-gated, and the reason is easy to get wrong.** `render_engine`
lives in the **lib**; `canvas.rs` is a **bin** module. When the bin's test target compiles, the lib
is built as a dependency *without* `cfg(test)` — so a gated field would not exist where the tests
need it. The counter is unconditional production surface: two `usize` and a doc comment.

**Why it earns that cost:** bug shape A — "mutates `Edits` but never calls `s.request`" — is
invisible from outside, because the request goes into a channel and vanishes. The alternative was
asserting on `requested_region`/`render_rung`, which is a *proxy* that misses exactly the
whole-image fit case where the bug actually occurred.

## 3. The lifted coordinate functions — `develop/canvas_geom.rs`

Three pure functions, chosen because bug shape B lived in precisely these:

| function | what it fixes |
|---|---|
| `frame_rect(center, pan, full, rung_k, scale)` | must use the whole-image frame, not `img_rect`, or the overlay drifts while panning |
| `normalized_point(p, frame_rect)` | the `nx`/`ny` clamp, currently written inline **twice** |
| `frame_aspect(frame_rect)` | taken from `img_rect` once, which silently corrupted hit-testing at high zoom |

Lifting these makes the drift and aspect bugs testable **with no fixture at all** — fast, and they
stay covered even if the fixture later rots.

## 4. The tests

Named for the real bugs, driven through `canvas::show` on the fixture:

- **A drag on a mask handle issues a render.** Needs the counter. The original bug: the image kept
  showing the mask's previous position until some unrelated edit happened to request a frame.
- **The overlay stays put while panning.** Against `frame_rect`, not `img_rect`.
- **A drag commits on release even when `interact_pointer_pos` is `None`.** egui often reports no
  interact position on the release frame; nesting `drag_stopped` inside that `if let` silently
  skipped both the render and the history push.

Plus direct tests of the three geometry functions.

⚠ **The harness must run enough passes.** `Response` state is per-frame; a single-pass harness reads
zeros, which is what made `a_left_drag_does_not_pan` pass vacuously once already. And a click is
only known on the *release* pass, so anything it opens cannot exist until the frame after — the
`name_edit` tests needed **four** passes for this reason.

## Scope — deliberately excluded

- **No pixel assertions, no golden screenshots.** Verifying what was painted needs a rendering
  backend and makes every test brittle to a one-pixel layout change.
- ⚠ **This cannot catch "the tint looks wrong" or "the wgpu vertex buffer was 711 MB".** Both were
  found by using the app and would be again. The fixture covers *wiring*, which is where five of
  the six known bugs were — not appearance.
- **The panel is not converted wholesale.** The fixture is built so panel tests can use it; writing
  them is follow-on work, not this slice.

## Testing the tests

Every new test gets the sabotage step: break the thing it names, **confirm the sabotage compiled**,
and watch it fail. ⚠ Two harness-level traps this project has already hit, both of which make every
result meaningless: a sabotage that fails to *compile* looks exactly like one that passes, and a
matcher that greps failure lines by bare test name finds nothing, because cargo prints module paths.
Check the harness against a sabotage known to be caught before trusting a clean run.

## Accepted caveats

1. **Two `usize` of unconditional test-only surface on `RenderEngine`.**
2. **The worker thread really spawns** in every fixture test — cheap, but not free.
3. **Appearance is out of scope**, so the two most spectacular past bugs remain uncatchable here.
4. **43 fields to construct**, so a new `RustroomApp` field means updating the fixture. Following
   this project's `Settings`-literal precedent, that is treated as a feature: the compiler forces a
   decision rather than silently defaulting.
