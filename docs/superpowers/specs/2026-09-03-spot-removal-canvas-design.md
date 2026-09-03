# M5.3b — the spot-removal canvas tool

*Design, 2026-09-03. Closes Phase 5.*

M5.3a landed the whole headless half in Wave 3 — `Spot`, `SpotMode`, `SpotRemovalStage`, the
`Retouch` edit group — and **nothing can place a spot except hand-editing a sidecar**. This is the
canvas tool, the panel, and the view mode that makes dust findable.

---

## 1. What M5.3a already decided, and is not reopened

- **`Spot { centre, source, radius, feather, mode }`**, all in **pre-geometry source normalised**
  coordinates, radius as a fraction of the **long edge**. `Copy`.
- **`SpotRemovalStage`** runs in the prefix between `PerspectiveStage` and `GeometryStage`, gathers
  every source disc before writing any destination, rounds the offset to whole pixels, and blends
  through a smoothstep feather. `SpotMode::Heal` shifts the source patch's mean to match the ring
  around the destination.
- **`RETOUCH_GROUP`** is index 8 and is **off** in `EditGroups::default()`, so spots do not travel
  with a paste, a preset or a sync — a placement, like Geometry and Masks.

None of that changes. This spec adds no field to `Edits` and no stage to `pipeline`.

---

## 2. The gestures

While `Tool::Retouch` is active:

| gesture | effect |
| --- | --- |
| **click** | place a spot at the current radius; source auto-picked |
| **drag out from the centre** | place, sizing live as the pointer moves; source auto-picked on release |
| **⌥-drag** | press on the blemish, release on the source to use; radius unchanged |
| **click a spot's destination or source** | select it |
| **drag either handle** | move that end of the selected spot |
| **Delete / Backspace** | remove the selected spot |

⚠ **Hit-test runs before placement**, exactly as `mask_tool::hit_test` does: a press landing on an
existing handle grabs it rather than stacking a new spot on top of it. Without that ordering a
crowded frame becomes impossible to edit.

⚠ **The drag must be `dragged_by(Primary)`, never a bare `dragged()`.** egui reports a drag for
every button, so the middle-drag pan binding would otherwise place spots as it passed over the
canvas — the trap the canvas-navigation work recorded and `tool_drag` exists for.

⚠ **A press that never becomes a drag is a click**, and egui distinguishes them by construction
(`clicked()` fires only for a press-release that did not become a drag). So "click places at the
current radius" and "drag places while sizing" are one `Sense::click_and_drag()` interaction, not
two competing ones — the rule the Library's rubber-band selection already relies on.

### The Size slider is not a second mechanism

The panel's Size slider shows and sets **the same value the drag sets**. One value, two
affordances — the idiom M5.2 shipped for white balance, where the eyedropper and the Kelvin slider
write one number. The radius persists across spots, so a run of similar dust is sized once.

⚠ A zero-length drag is a click, and a drag of less than ~3 screen points is treated as one too:
without a threshold, a click with a shaky hand would place a spot of radius ~0, which covers
nothing and is invisible on the canvas.

---

## 3. Where the code goes

Decided before writing anything, per the standing rule.

| piece | home | note |
| --- | --- | --- |
| `Tool::Retouch` | `app/src/tool.rs` (lib) | `ALL` becomes 5; `5` selects it |
| hit-test / place / drag maths | `app/src/spot_tool.rs` (lib, **no egui**) | the `mask_tool` / `crop_tool` pattern |
| auto-source search | `app/src/spot_source.rs` (lib, **no egui**) | pure, over an injected sampler |
| panel section | `app/src/develop/panel/retouch.rs` (bin) | egui only in bin modules |
| canvas overlay + gestures | `app/src/develop/canvas/spot_overlay.rs` (bin) | mirrors `mask_overlay.rs` |

⚠ **The rail glyph must be probed before it is chosen.** Four separate features in this project
have found their intended glyph absent from the bundled fonts and would have shipped tofu boxes.
Phosphor is now bundled, so the constant goes in `icons.rs` **with the icon's name in a doc
comment**, joins `icons::ALL`, and `every_rail_glyph_is_renderable` covers it automatically.

⚠ **`Tool::ALL` going from 4 to 5 is the guard that the rail cannot silently omit it** — the rail
iterates that array, so a variant missing from it compiles perfectly and is unreachable.

### State

- `Session.selected_spot: Option<usize>` — **view state, never `Edits`**, exactly like
  `selected_mask`. ⚠ Cleared on a photo switch (free — it dies with the session) **and on a
  snapshot restore**, because a restored state has a different spot list and a stale index would
  select a spot nobody selected.
- `RustroomApp.spot_tool: SpotTool { radius, feather, mode }` — session-lifetime, not persisted.
  The brush's rule.

---

## 4. Auto-source

```rust
pub fn pick(
    sample: impl Fn([f32; 2]) -> [f32; 3],
    dest: [f32; 2],
    radius: f32,
    aspect: f32,
    existing: &[Spot],
) -> Option<[f32; 2]>
```

Sample the **ring around the destination** as the target signature, then score candidates at 16
angles × 5 distances (2r…6r). Score is the colour distance from that ring **plus** the variance
inside the candidate disc — a flat patch copies more cleanly than an edge. Reject any candidate
that leaves the frame, overlaps the destination disc, or lands on an existing spot's destination.
`None` when nothing qualifies, in which case the caller places the source **3r straight up**,
clamped to keep the disc inside the frame — a spot the user can drag beats a click that silently
did nothing.

⚠⚠ **The ring is sampled at 1.2r–1.6r, outset from the rim rather than flush with it, and this is
A's Wave 3 finding arriving one layer up.** That finding — *the retouch radius must exceed the
blemish, or heal measures a ring that is still lying on the spot* — applies to the **search**
identically: a ring flush with a too-small radius **is the blemish**, so the signature is the
blemish's colour and the search confidently returns a source that looks like the thing being
removed. It gets its own test with a fixture whose blemish is larger than `r`.

⚠ Discs are subsampled every third pixel. A mean and a variance need nothing finer, and it keeps a
large-radius search in single-digit milliseconds rather than ~100 ms.

### The injected sampler, and why it is not a buffer

⚠⚠ **`FrameMap::inverse_point` returns post-lens, post-perspective coordinates; `Session.buffer`
is the raw decode. Indexing one with the other is wrong by the distortion** — recorded at ~3.5% at
the corners, which on a 6246 px frame is ~220 px, **larger than a typical spot radius**. Near the
edges the search would sample a different piece of the picture, score against the wrong target, and
return a confidently wrong source. Silently.

The fix needs the **corrected → raw** direction, and both stages already expose exactly that:

```
stage-input point
  → pipeline::perspective::Perspective::source_of   (normalised both sides)
  → lens_profile::Corrector::source                 (pixels; per-channel, for TCA)
  → read Session.buffer
```

⚠ `Corrector::source` returns **three** coordinates, one per channel. The sampler reads each
channel at its own, which costs nothing and is more faithful than picking green for all three.

⚠ The sampler is **injected** rather than the search taking a buffer, so `spot_source` stays a
pure function with no dependency on `lens_profile` or `perspective`, and its tests pass a synthetic
closure with no image at all. The `DecodeWorker` injected-decode pattern, for its reason.

⚠ Missing corrections are the identity, not an error: a JPEG has no lens profile and a neutral
perspective is the identity homography, so the sampler degrades to a plain buffer read.

---

## 5. The overlay

Every spot while Retouch is active: a destination circle, a source circle, a line between them, the
selected one brighter. A spot whose destination or source falls outside the current crop is
drawn and simply clipped by `img_rect` — it still exists and still affects the render, so hiding it
would make a spot the user cannot find but can see the effect of. Drawn under `Tool::Retouch` only — the canvas already matches on the tool
exhaustively, so a fifth variant forces the decision rather than defaulting.

⚠⚠ **Every coordinate goes against `frame_rect`, never `img_rect`.** That is the M3.1a drift bug —
at high zoom `img_rect` covers only the *rendered region*, so an overlay placed against it follows
the view while panning, and its aspect is the region's rather than the frame's. `img_rect` survives
as the **clip** and nothing else.

⚠⚠ **Spots are stored pre-geometry and the overlay draws on the post-geometry frame**, so each one
maps forward through `FrameMap::point`. Unlike the analytic masks, whose frame-relative coordinates
need no mapping at all, this is not optional: without it a cropped or straightened photo draws
every spot in the wrong place.

⚠ **A drag must call `s.request`.** `draw_crop_overlay` deliberately does not, because
`preview_edits` strips the crop in geometry mode so the overlay *is* the feedback — copying that
here is the exact false analogy that shipped the M3.1a mask bug, where the image did not update
until an unrelated slider was touched.

⚠ **Painted, not meshed.** Circles are `Shape::closed_line` over `ELLIPSE_SEGMENTS`, as the radial
mask already does; egui has no ellipse primitive.

⚠ **Doubled stroke: a dark halo under a light mark.** A thin white circle is invisible against a
bright sky, which is the fix the crop overlay needed after being reported.

---

## 6. Visualize Spots

`FrameSpec` gains `visualize: Option<f32>` — the threshold, `None` when off — applied in
**`pack`**, after the pipeline.

⚠⚠ **In `pack`, never as a pipeline stage**, and the reason is recorded rather than reasoned: the
display transform went into `pack` because a stage would make the **histogram** display-referred,
and `pack` already takes the histogram from its sRGB output *before* the post-step. Visualize is
the same shape and inherits that ordering. As a stage it would also need a place in the cache key
for something that is not an edit at all.

The render is a high-pass on luminance — `|luma − gaussian_blur(luma, σ)|` — gain-mapped by the
threshold and written as grey. It reuses `spatial::gaussian_blur` rather than introducing an
algorithm. Dust and sensor spots read as bright rings on flat grey.

⚠ It is a **view mode, not an edit**: it never reaches `Edits`, the sidecar, an export or the CLI,
and toggling it re-renders without touching the prefix cache — `FrameSpec` is not part of
`CacheKey`, which is `(PrefixKey, max_dim)`.

⚠ The toggle lives in the Retouch panel and **turns itself off when the tool changes**. A canvas
stuck in a high-contrast grey view after switching to Edit reads as a broken render, and it is the
single most likely thing here to be reported as a bug.

⚠ σ is **6 rendered pixels**, not scene pixels, so the view looks the same at fit and at 1:1.
Unlike the spots themselves, which are normalised, this is a screen-space effect and deliberately
does *not* take the render scale: the point is to make dust visible on screen, and dust is a few
pixels across wherever you are looking from.

⚠ The threshold slider is a **gain**, 1×–50×, defaulting to 12×. A slider that mapped to a hard
cutoff would show nothing at all over most of its travel; a gain always shows *something*, which is
what lets it be judged by eye.

---

## 7. Two plans, one spec

**Plan 1 — placement.** `Tool::Retouch` and its glyph, `spot_tool`, `spot_source`, the panel
section, the canvas overlay and gestures. A working feature on its own.

**Plan 2 — Visualize Spots.** `FrameSpec.visualize`, the `pack` post-step, the panel controls.

They are independent: plan 2 touches no spot state, and plan 1 needs nothing from it.

---

## 8. Testing

The suite tests arithmetic well and interaction hardly at all, and this is interaction. The
headless canvas fixture (`test_app` + `test_ui`) exists precisely for this and is the requirement,
not a nice-to-have.

**`spot_tool` and `spot_source`** — pure, so ordinary unit tests: hit-test precedence
(destination over source over empty space), the drag threshold, placement at the stored radius, the
ring outset, a candidate rejected for overlapping the destination, `None` when nothing qualifies.

**Canvas**, through `test_ui`'s five-pass drag harness:
- a click places exactly one spot, and a second click on that spot selects rather than stacking;
- a drag sizes it — assert the **radius**, not merely that a spot appeared;
- ⌥-drag uses the released point as the source and leaves the radius alone;
- ⚠ **a drag requests a render on the drag frame itself** — sampled on that frame, because a later
  frame issues one anyway and in the real app that frame never comes (egui idles), which is exactly
  how the M3.1a bug hid;
- ⚠ **a drag that grabs nothing changes nothing** — the guard on the guard, without which "a spot
  appeared after a drag" means nothing.

**Sabotages that must be run**, because each names a failure this project has already shipped once:
placing against `img_rect` instead of `frame_rect`; skipping `FrameMap::point` in the overlay;
dropping `s.request`; a bare `dragged()`; the ring sampled flush with the rim; Visualize reaching
the histogram.

**Goldens: all 53 must stay byte-identical.** Nothing here touches a stage, and that is the proof.

---

## 9. Accepted caveats

1. **Toggling lens correction or perspective after placing a spot shifts it**, exactly as it shifts
   a brush stroke. See §10 — not fixed here, and not fixable here.
2. **Discs only.** M5.3a's model is a disc; there is no freehand healing brush.
3. **No panel list of spots**, unlike Masks — forty dust spots make a useless list, and Lightroom
   has none either. A count and Clear all.
4. Tool state (radius, feather, mode) is session-lifetime, not persisted.
5. Auto-source scores a mean and a variance, so it prefers flat patches and will pick poorly on
   fine repeating texture. ⌥-drag is the answer, which is why it exists.
6. Visualize Spots is a fixed high-pass with one threshold, not an adaptive dust detector: it makes
   dust *findable*, it does not find it for you.

---

## 10. What this deliberately does not fix

⚠⚠ **Spots and brush strokes are anchored in `GeometryStage`'s input, which sits downstream of
lens correction and perspective — so the coordinate stays put while the content under it moves.**
The principled fix is to anchor in true **sensor** space and map forward when rasterizing, which
needs a **forward** distortion map lensfun does not expose.

⚠ It is backlogged as **"anchor strokes and spots in sensor space"**, not as "extend `FrameMap`":
`FrameMap` is not wrong. It models orientation → straighten → crop faithfully, which is its entire
domain. The bug is that the space it maps *into* is the wrong anchor. Naming the entry after the
file would send the next reader to patch a function that has nothing wrong with it.

⚠ It affects the brush identically and predates this milestone, so it is its own change.
