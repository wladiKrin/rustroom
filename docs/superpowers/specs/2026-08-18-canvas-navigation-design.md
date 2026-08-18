# Canvas navigation — pan moves off left-drag

**Status:** implemented, 2026-08-18. See "Outcome" at the end.

## Scope

Left-drag stops panning. Pan moves to scroll / two-finger swipe / a configurable drag button;
zoom moves to pinch and ctrl+scroll. A `Settings` field lets the pan drag binding be changed.

**This is a prerequisite for M3.1c, not part of it.** A brush wants every left-drag on the image,
which collides head-on with drag-to-pan. The alternative was an explicit Paint mode — rejected
because this project already rejected a third canvas mode once, in 3.1a, and because moving pan
*dissolves* the collision rather than managing it.

⚠ **It is sequenced separately deliberately.** It changes navigation for every photo whether or
not masks are involved, so a panning regression would otherwise hide inside a large brush diff.
It also wants the GUI in hand to tune feel, which is its own loop.

## Decisions taken

| Question | Decision |
|---|---|
| Does left-drag still pan as a fallback? | **No. Never.** One rule everywhere. |
| Zoom gesture | **Pinch and ctrl+scroll**, via egui's `zoom_delta()`. |
| Pan gestures | **Scroll / two-finger swipe** (both axes), **and a drag button**. |
| Pan drag binding | **Configurable**, three values, defaulting to middle-drag. |
| Discoverability | **One `theme::hint` line** in the Develop toolbar. |

## The gesture map

| gesture | action |
|---|---|
| plain scroll / two-finger swipe | **pan**, both axes |
| pinch, or ctrl+scroll | **zoom**, cursor-anchored |
| the configured pan drag (default middle-drag) | **pan** |
| left-drag | **tool only** — crop rect, mask handles, later the brush. Never pans. |

Concretely, in `develop/canvas.rs`:

- Zoom becomes `s.zoom * i.zoom_delta()`, replacing `s.zoom * (1.0 + scroll * 0.001)`.
  ⚠ `zoom_delta()` is a **multiplicative factor returning 1.0 when idle**, not a delta — it folds
  multi-touch pinch and `zoom_factor_delta` (ctrl+scroll), verified in
  `egui-0.35.0/src/input_state/mod.rs:559`.
- Pan becomes `s.pan += i.smooth_scroll_delta` — **both axes**, where today only `.y` is read at
  all and it drives zoom.
- The pan drag is a predicate over `resp`, not a hardcoded button — see below.

⚠ **Middle-button drags do register.** `Response::dragged_by(button)` is
`dragged() && button_down(button)`, and `is_decidedly_dragging` gates on `any_down()` rather than
on the primary button (`egui-0.35.0/src/input_state/mod.rs:1512`). Read, not run — so
`a_middle_drag_pans` exists to run it.

## Four wrinkles

- **⚠ Pan must inherit the `resp.hovered()` gate zoom has today** (`canvas.rs:151`). Without it,
  scrolling the adjustments panel or the filmstrip would pan the photo underneath. The gate
  exists; it has to widen to cover both actions.
- **⚠ Pan stays meaningless at fit.** `canvas.rs:164` zeroes `s.pan` whenever `zoom <= 1.0`, and
  that is kept — there is nothing off-screen to reveal. Consequence, stated rather than
  discovered: **two-finger swiping a fit-view photo does nothing**, which is correct and may
  read as broken.
- **⚠ Geometry mode is untouched.** It pins itself to fit resolution whatever the zoom
  (`canvas.rs:34`), so panning there has nothing to reveal either. Left-drag keeps driving the
  crop rect exactly as now.
- **⚠ ctrl+scroll, not cmd+scroll, is the mouse zoom fallback.** That is egui's own mapping and
  what feeds `zoom_delta`. Trackpad users get pinch, the better gesture. Changing it to cmd
  means synthesizing the event ourselves — deliberately not done.

## The configurable pan binding

### Why now rather than later

⚠ **The urgency is lower than it first looks, and the retrofit cost is what decides it.**

Trackpad and Magic Mouse users get one- or two-finger swipe → scroll → pan, so they are **fully
served with no middle button at all**. The genuine hole is narrower: a conventional wheel mouse
gives *vertical* scroll only, so horizontal panning needs the drag binding — and a user whose
wheel-click is broken cannot pan horizontally by any route.

What settles it is that **the pan branch is written exactly once**. Against a small enum it costs
about five lines more than a hardcoded `PointerButton::Middle`; retrofitting later means
rewriting shipped behaviour *and* adding the settings field and modal row anyway. It is also
additive and orthogonal — a settings field cannot cause a panning regression — so it does not
dilute the small-diff argument that sequenced this change separately.

### Shape

⚠ **A mouse button is not a modifier**, so the setting must span both kinds. Deliberately a
**closed three-value enum, not a keybinding system** — the risk here is ballooning into general
rebinding, and three values cover every device.

```rust
/// How a drag pans the canvas. Scroll and two-finger swipe always pan and are not configurable.
pub enum PanDrag {
    /// Middle-button drag. The default, and unavailable on a Magic Mouse.
    Middle,
    /// Right-button drag. Free — nothing on the canvas uses secondary click.
    Secondary,
    /// Hold Space and left-drag. ⚠ The only option needing no extra hardware, and the
    /// Photoshop/Figma convention. Space is currently unbound everywhere in this app.
    SpaceLeft,
}
```

Verified free: `grep Key::Space` finds nothing, and no `PointerButton::Secondary` /
`context_menu` exists on the canvas.

⚠ **`SpaceLeft` must be gated on `ctx.memory(|m| m.focused())` being `None`**, the rule arrow-nav
and the clipboard keys already follow — otherwise typing a space into the preset-name field would
arm panning.

### Settings integration

`Settings` gains `pan_drag: PanDrag` with `#[serde(default)]`, following `view_demosaic` as the
template. A row in the Settings modal.

⚠ **`SettingsStore::load()` is infallible**, so a field that fails to parse silently reverts
**every** setting — which is why `a_settings_file_from_before_pan_drag_still_loads` is a required
test and not a nicety. That trap has already been documented once, in the view-demosaic work.

⚠ Unlike `view_demosaic`, changing this needs **no re-decode, no worker rebuild and no change
guard** — the canvas reads `settings.pan_drag` each frame. The Settings modal's "a dialog body
runs every frame" rule does not bite here.

## Discoverability

Nobody guesses middle-drag. One `theme::hint` line in the Develop toolbar:

> Scroll or middle-drag to pan · pinch or Ctrl+scroll to zoom

(that is the wording **with the default binding**; the middle clause changes with the setting)

⚠ The hint must **name the configured binding**, not the word "middle" literally, or it becomes a
lie the moment the setting is changed. `PanDrag` therefore carries a `label()` — the same shape
`Mask::label()` took in M3.1b, and a table test over all three values so a fourth cannot ship
showing nothing.

## Testing

⚠ **Each test must fail when the thing it names is broken.** This session produced six instances
of a test that could not detect its own subject, three of them in M3.1b alone — see CLAUDE.md's
standing lesson. The sabotage step is mandatory, and a sabotage that *passes* is a finding about
the test.

Headless egui interaction is an established seam here: `clicking_a_library_grid_thumbnail_opens_it`
drives `RawInput` events through `Context::run_ui` across three passes.

| Test | What it pins | Sabotage |
|---|---|---|
| **`a_left_drag_does_not_pan`** | the whole change | restore the old `resp.dragged()` pan branch |
| `a_middle_drag_pans` | `dragged_by(Middle)` actually fires | change the branch to `Primary` |
| `scrolling_pans_both_axes` | `.x` is read, not just `.y` | drop the `.x` component |
| `scrolling_outside_the_image_does_not_pan` | the `hovered()` gate | remove the gate |
| `space_left_drag_is_ignored_while_a_text_field_has_focus` | the focus gate | remove it |
| `pan_is_zero_at_fit` | the existing `zoom <= 1.0` reset survives | delete the reset |
| `a_settings_file_from_before_pan_drag_still_loads` | infallible `load()` | drop `#[serde(default)]` |
| `every_pan_drag_has_a_label` | the hint cannot go blank | return `""` for one variant |

⚠ **`a_left_drag_does_not_pan` must assert `s.pan` is *unchanged*,** not merely that some other
branch ran. Asserting "the mask branch was taken" would pass with a stray `s.pan +=` still in
place — precisely the shape of mistake this project keeps making.

### The pure part

`view_region` exists so viewport arithmetic is testable without a window. Two functions move
there:

```rust
/// The new zoom after a gesture, clamped to the usable range.
pub fn zoom_step(zoom: f32, factor: f32) -> f32;

/// The pan correction that keeps the point under the cursor fixed while zooming.
/// Returns `(dx, dy)`; `pointer` and `anchor` are `(x, y)` in the same space.
pub fn zoom_anchor_shift(
    pointer: (f32, f32),
    anchor: (f32, f32),
    old: f32,
    new: f32,
) -> (f32, f32);
```

⚠ **Plain tuples, not `egui::Vec2`.** `view_region`'s module doc says it is "deliberately free of
`egui` types so it can be tested directly", and every existing signature there
(`tier_max_dim`, `visible_rect`, `with_margin`, `contains`) holds to that with `usize`/`f32` and
its own `RegionRect`. Taking an `egui::Vec2` here would breach the one rule the module exists to
enforce. The caller converts at the boundary, as it already does for `RegionRect`.

Both are currently inline in `canvas.rs` (lines 152–160). Extracting them is not tidying: the
cursor-anchor arithmetic is exactly the kind of thing that is wrong by a sign and invisible in
review.

## What does not change

- `GeometryStage`, the pipeline, and every crate outside `app`. **Zero goldens touched.**
- Geometry mode's left-drag, and the crop overlay.
- Mask handle dragging — still left-drag, still "only if the drag started on a handle".
- The zoom range (`1.0..=8.0`) and cursor-anchored zooming.
- The Library grid and filmstrip, whose own scroll areas keep consuming scroll.

## Accepted caveats

1. **A habit change felt on every photo**, immediately. That is the point, not a side effect.
2. **Two-finger swipe at fit does nothing**, because there is nothing to pan to.
3. **No kinetic or inertial panning** — a swipe moves exactly its delta, and momentum is a
   separate feel question.
4. **Three bindings, not general rebinding.** A fourth device convention would need a code change.
5. **The hint is Develop-only.** The Library grid has no pan.
6. **ctrl+scroll rather than cmd+scroll on macOS**, inherited from egui.

## Outcome

**Implemented 2026-08-18.** 602 tests, up from 582. No golden moved and nothing outside
`crates/app` was touched.

### Two things the spec got wrong, both found by reading egui's source while planning

1. **⚠ Any button starts a widget drag.** `interaction.rs` matches `PointerEvent::Pressed { .. }`
   and **ignores the button** when setting `potential_drag_id`, so `resp.dragged()` is true for a
   middle- or right-drag too. The spec's gesture map quietly assumed otherwise. **Consequence:
   without gating, the new pan button would also drag crop rectangles and mask handles.** That
   became its own task, landing *before* the pan change so a tool regression could not hide
   inside the remap — and it is confirmed empirically, not just by reading: with the gate
   removed, a middle-drag and a right-drag both drive the tools.
2. **⚠ The pan-at-fit reset was narrower than the spec claimed.** It sat *inside* the scroll
   branch, so a left-drag at fit genuinely could push a fitting image off-centre into grey with
   nothing putting it back until the next scroll. It is unconditional now — a small deliberate
   behaviour fix rather than a port.

### Two bugs the tests caught before they shipped

Both would have reached the user; neither was visible in review.

- **⚠⚠ A re-entrant lock that panics.** `pan_delta` first took `&egui::InputState`, but
  `Response::dragged_by` calls `self.ctx.input(..)` internally — holding the lock across it
  panics in epaint's mutex. **It hides in casual testing** because `dragged_by` is
  `dragged() && ctx.input(..)`, and `dragged()` reads a plain flag, so the `&&` short-circuits
  and never takes the second lock *unless something is actually being dragged*. It would have
  crashed on the first real middle-drag. `pan_delta` takes plain values now, which is a better
  signature anyway.
- **⚠⚠ A harness bug that made the most important test vacuous.** `drag_delta()` is per-frame,
  and the first harness moved the pointer once on pass 2 while reading on pass 3 — so it read
  `[0,0]`, and **`a_left_drag_does_not_pan` would have passed with the gating deleted
  entirely.** Found by instrumenting rather than by re-reading: one `eprintln` showed
  `dragged=true, down_mid=true, delta=[0,0]` and the cause was immediate. The pointer now keeps
  moving on every pass, and the sabotage step confirms the test fails when a stray pan is
  reintroduced. **This is the eighth recorded instance in this project of a test that could not
  detect its own subject, and the first caused by the *harness* rather than the assertion.**

### Settled rather than guessed

**The scroll sign is right by egui's own convention**, not by trial: `Event::MouseWheel`'s doc
says the delta "indicates how to move the *content* being viewed … positive values, the content
should move to the right and down". Panning is exactly moving the content, so `pan += delta`
follows. Recorded because the obvious alternative — flipping signs until it feels right — leaves
nothing behind for the next person.
