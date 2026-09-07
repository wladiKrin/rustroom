# An interactive rung — a responsive drag on a large window

Date: 2026-09-07
Status: **designed, not built.**

## The problem, measured in the running app

Reported twice: dragging exposure stutters. Instrumented with `RUSTROOM_DEBUG_RENDER=1` over 167
real drag frames, on the reporter's own display and photograph:

| rung | frame | region | frames | min | mean | max | fps |
|---|---|---|---|---|---|---|---|
| 1600 (fit) | 1.7 Mpx | no | 2 | 33.6 | **35.2** | 36.9 | **28** |
| **3200** | **6.8 Mpx** | **no** | **131** | **94.9** | **137.9** | **242.3** | **7.2** |
| 6246 | 6.2 Mpx | yes | 29 | 93.2 | 116.9 | 201.8 | 8.6 |

⚠⚠ **`prefix_ran 0` on all 167 frames.** The prefix cache is perfect; this is entirely tail +
pack. Every earlier suspicion about the prefix, the capture-corrections projection and the
checkpoint chain is closed by that one column.

⚠⚠ **A *global* exposure drag behaves identically**, reported independently. So `MaskStage` is not
involved — not its clone (0.4 ms of a 3.0 ms stage), not its coverage, not its blend, and not the
bounding box deferred seven times. The backlog entry that opened this work was **wrong from its
title down**; what is left is the rung.

`fit_cap` is `min(buffer_long, 1600)`, but `tier_max_dim` climbs a doubling ladder as soon as the
viewport exceeds the fit texture. On a large window that lands on **3200, whole-image**. The
measured time ratio (137.9 / 35.2 = **3.9×**) matches the pixel ratio (4.0×) almost exactly, which
is what identifies the rung as the cause rather than something incidental to it.

### ⚠ This is not waste, and an earlier framing of it was wrong

`tier_max_dim`'s `needed` is the viewport's long edge **in points**, so rung 3200 means a canvas
1601–3200 points wide — at 2× HiDPI, 3200–6400 *physical* pixels. **A 3200-texel texture is being
upsampled, not thrown away.** The 2026-08-13 spec measured exactly this and called it "a 1.44×
residual".

So the 137.9 ms buys resolution the display can genuinely use. There is no waste to reclaim: the
only moves are **render less** (softer) or **render faster** (Phase 8). This design takes the first,
during gestures only.

## Prior art: this was built once and removed

`docs/superpowers/specs/2026-08-13-progressive-fit-resolution-design.md` — **built, shipped,
debugged three times, deleted.** It rendered at 1600 while interacting and re-rendered higher when
idle. Read it before re-deriving any of this. Its three bugs:

1. **An oscillating gate.** `wants_sharpen` asked *"is the showing resolution not yet sharp?"* —
   which goes false the frame the sharp render lands, so it reverted, counted that as interaction,
   and reset its own timer.
2. **It sharpened the wrong buffer** — `sharp_cap` clamped to `buffer_long`, the half-size binned
   buffer before the full-resolution upgrade.
3. **The clamp fix made its gate unreachable** — it tested "at the base rung", true only while
   `fit` was clamped to 1.0.

⚠⚠ **AND ITS REMOVAL REASON IS AN ARGUMENT *FOR* THIS DESIGN — the inversion is the finding.** It
was removed because *"the user could not see it on or off"*. The question then was **"is the
sharper render visible?"** (no). The question now is **"is the slower render felt?"** (yes, 7.2 fps,
reported twice). If a higher rung is not visibly sharper in a still frame, dropping one *during a
gesture* costs little and buys 4× the frame rate.

⚠ Stated honestly: that "could not see it" was measured at a **1.44× residual**, and this design's
drop is **2× linear**. Suggestive, not conclusive — and this project settles judgements about
photographs on photographs.

⚠ **The problem has materially worsened since that deletion.** That spec's own table put a tail
slider at rung 3200 at **59.5 ms**; today it is **137.9 ms**. The tail has roughly doubled —
display rendering, creative looks, masks, colour grading and soft proofing all landed after.

⚠ **The seam it left is still in the code.** `develop/canvas/mod.rs:120` reads `let max_dim = rung;`
— a binding that does nothing, exactly where the override used to sit.

## The measurement that settled the one open question

The scale-aware stages take a `RenderScale` so their radii track the render resolution, keeping
their effect *proportionate* at any rung. **Proportionate is not the same as visible.**
`cargo run --release -p app --example rung_visibility <raw>` (kept), mean 8-bit levels and share of
samples moved between neutral and edited **within** each rung:

| edit | rung 1600 | rung 3200 | rung 6246 |
|---|---|---|---|
| exposure +0.5 *(control)* | 11.459 / 75.00% | 11.453 / 74.99% | 11.444 / 74.98% |
| **sharpening 70** | **0.001 / 0.14%** | 0.419 / 37.88% | 1.273 / 60.58% |
| texture 70 | 0.189 / 18.84% | 0.591 / 46.54% | 1.028 / 57.39% |
| clarity 70 | 1.039 / 53.92% | 1.134 / 56.35% | 1.314 / 59.51% |
| noise reduction 50/50 | 0.623 / 45.37% | 0.816 / 50.93% | 0.964 / 53.60% |
| dehaze 60 | 9.708 / 74.92% | 8.125 / 74.58% | 5.974 / 73.55% |

⚠ **The control is the load-bearing row.** Exposure has no kernel, so it must read identically at
every rung; flat to three decimals is what says the harness is sound before any spatial row is
believed.

⚠⚠ **Sharpening vanishes at the fit rung: 0.419 → 0.001, 420× weaker, 0.14% of samples moved.**
`CLAUDE.md` predicted precisely this — at fit scale a full-resolution σ of 1.0 becomes 0.256 px,
"a kernel of [0.0005, 0.999, 0.0005], i.e. the identity" — and it is now measured rather than
reasoned.

⚠ **The collapse is specific to the drop *into* the fit rung.** Zoomed, 6246 → 3200 leaves
sharpening at 37.88% moved, plainly visible. Texture drops 3.1× and survives; clarity is only 8%
weaker, because σ = 40 full-resolution px is still 10 px at rung 1600; NR and dehaze are barely
affected.

**Decision (user's call): no per-slider exception.** One gate, no table to maintain, nothing to
plumb from the panel to the canvas. Sharpening reads at nothing mid-drag and appears on release —
acceptable because `CLAUDE.md` already directs that it be judged at 1:1, which is where the drop
keeps it visible.

## The design

### 1. `Settings.drag_quality` — and it is the off switch

```rust
/// How much resolution a live gesture may trade for responsiveness.
pub enum DragQuality { Full, Faster, Fastest }
```

`ALL`, `label()` and `rungs_dropped() -> u32` (0 / 1 / 2) live **beside the type**, the rule
`EDIT_GROUP_NAMES`, `Tool::ALL` and `FlagSet::ALL` already follow, so the Settings UI cannot invent
a second ordering or a second mapping. A named enum rather than a bare integer follows `PanDrag`,
`Demosaic`, `ThemeChoice` and `Accent`.

Default **`Faster`**. A new field under `Settings`' container-level `#[serde(default)]` reaches
every existing settings file, so there is no migration — and `a_settings_file_from_before_drag_quality_still_loads`
is the guard, because `SettingsStore::load` is infallible and one unparseable field silently
reverts *every* setting.

⚠⚠ **`Full` is `rungs_dropped() == 0`, and it is the whole reason this is a setting rather than a
constant.** When the GPU port lands and the tail runs 5–15× faster, the feature is switched off
from Settings rather than removed from the code. **The setting is the deletion mechanism.** It also
lets a slower or faster machine pick its own point on the trade, which a hardcoded rung cannot.

⚠ `Fastest` differs from `Faster` **only when zoomed**: fitted, a two-rung drop from 3200 clamps at
`fit_cap` and lands where one rung already did. Recorded so it is not later filed as a bug.

### 2. `view_region::rung_below` — ladder-aware, never a halving

```rust
/// The rung `n` steps below `rung` on the ladder `tier_max_dim` walks, floored at `fit_cap`.
pub fn rung_below(rung: usize, fit_cap: usize, buffer_long: usize, n: u32) -> usize;
```

⚠⚠ **It rebuilds the ladder and steps back; it must never halve.** The ladder doubles from
`fit_cap` and **clamps** at `buffer_long`, so its top rung is **6246, not 6400** — halving that
gives 3123, which is *not a rung `tier_max_dim` can produce*. Feeding one to the renderer mints a
new `max_dim`, hence a new `CacheKey`, and grows the prefix cache's key set without bound while
looking entirely correct on screen.

⚠ `n == 0` returns `rung` unchanged, so the disabled path is **the same code** rather than a branch
around it — there is no second route through the renderer to keep in step.

### 3. The gate

At `develop/canvas/mod.rs:120`, replacing the vestigial `let max_dim = rung;`:

```rust
// Hoisted ABOVE the `&mut app.session` borrow — see below.
let drop = if dragging { app.settings.drag_quality.rungs_dropped() } else { 0 };
...
let max_dim = view_region::rung_below(rung, fit_cap, s.buffer_long, drop);
```

with `dragging = ui.ctx().input(|i| i.pointer.is_decidedly_dragging())`.

⚠ **Both `dragging` and `drop` are read into locals before the session is borrowed**, the rule the
`Tool` hoist already follows: `app.settings` is an immutable borrow of `app` and `&mut app.session`
a mutable one, so reading the setting *after* the session borrow is `E0502`. `DragQuality` is
`Copy`, so the hoist is free.

⚠⚠ **The gate is time-based, not state-based, and that is what makes it structurally unable to
oscillate.** The removed feature asked a question the *render* could answer, so the render answered
it and the gate flipped. This asks a question about the pointer, which no render can affect.

⚠ **`is_decidedly_dragging` excludes a plain click** — its condition includes
`!could_any_button_be_click() && !any_click()` — which is what stops a click costing two renders
(one down, one back). Verified in egui 0.35.0's source, not assumed.

⚠ Its documented edges, both benign: it is **false on the first frame of a drag** and **can be true
on the release frame**. The worker drains its queue to the newest request, so the first frame's
full-rung request is discarded before it renders; the release is restored one frame later.

**No settle timer.** `canvas/mod.rs:170` already re-requests whenever `max_dim != s.render_rung`, so
the release edge triggers the full-quality render by itself. The removed feature needed a 200 ms
timer only because its gate was "idle for long enough".

### 4. What must not move

- ⚠⚠ **`native` is computed from the dropped `max_dim`, which is the *opposite* of the old spec's
  §2 warning — and correct, because our override goes DOWN where theirs went up.** Theirs could
  report native at fit zoom and make the whole image vanish; ours cannot. At rung 1600 whole-image
  is right, and a fitted view at a high rung is caught by `covers_whole_image` returning `None`.
- **The canvas owns `render_rung`; the panel reads it** through `render_max_dim()`. The panel is
  drawn *before* the canvas, so on the first drag frame it requests the previous rung and the canvas
  immediately re-requests the new one — coalesced to the newest, costing nothing.
- ⚠ **`s.rendered_region` is in rendered pixels at whatever rung produced it**, so comparing it
  against a `want` computed at a different rung is apples to oranges. It is already harmless: line
  170's `|| max_dim != s.render_rung` forces a request on any rung change regardless of `covered`.
  **Do not "simplify" that disjunction away.**
- **`FINALS_CAP` stays at 2.** The old spec raised it to 3 because its three rungs were live
  *within one mode*; ours are two per mode (fitted {1600, 3200}, zoomed {3200, 6246}). Cross-mode
  transitions already recompute today. `RUSTROOM_DEBUG_RENDER` reports `prefix_ran`, so this is
  **checkable rather than assumed** — raise it only if that column stops reading 0.

## Testing

| test | asserts |
|---|---|
| `rung_below_returns_a_real_ladder_rung` | every result over the whole ladder × `n` ∈ 0..=3 is a value `tier_max_dim` can produce — the halving trap |
| `rung_below_is_the_identity_at_zero` | the disabled path is the same code |
| `rung_below_floors_at_the_fit_cap` | including `n` larger than the ladder is deep |
| `rung_below_steps_down_the_uneven_top_rung` | 6246 → 3200, **not** 3123 |
| `drag_quality_maps_to_rungs` | `Full` is 0, and the enum's `ALL` covers every variant |
| `a_settings_file_from_before_drag_quality_still_loads` | the forgiving load, sabotage-verified by removing `#[serde(default)]` |
| the 59 goldens | byte-identical — no pixel path changes at all |

⚠ **What no test here can cover**, stated rather than glossed: that the gate actually reaches the
canvas, and that a drag feels better. `shell.rs` cannot be driven headlessly (`eframe::Frame` is
not constructible in a test — the tier-2 gap this project already records), and "does 28 fps feel
right" is a judgement about a photograph. Both are checked by running the GUI with
`RUSTROOM_DEBUG_RENDER=1` and reading the rung column.

## Accepted caveats

1. **Sharpening reads at 0.001 levels mid-drag** and appears on release — measured, and the reason
   `CLAUDE.md` says to judge it at 1:1, where the drop keeps it visible.
2. **Texture drops 3.1×**; clarity, NR and dehaze are barely affected.
3. **The image is visibly softer during a gesture.** At `Faster` on a large HiDPI window that is
   roughly a 2.9× upsample. This is the trade, not a side effect.
4. **The first drag frame may render at the full rung**, since `is_decidedly_dragging` is false on
   the frame a drag starts. Usually invisible, because the worker coalesces it away.
5. **`Fastest` equals `Faster` in the fitted view**, differing only when zoomed.
6. **Nothing here helps the first render after opening a photo** — the prefix cache is cold then, as
   always.
7. **This does not make the tail faster.** It renders less of it. The tail's cost at 6.8 Mpx is
   Phase 8's problem, and this is Phase 8's first item pulled forward — that phase is explicitly
   "cheap wins, then `wgpu`".
