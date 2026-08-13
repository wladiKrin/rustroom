# A sharp fit view, without slow sliders — design

Date: 2026-08-13
Status: **built, then removed.** The measurements below stand; the design's premise did not.

> ## ⚠ Why this was backed out
>
> The premise — "the fit view is soft because `PREVIEW_MAX_DIM` ignores `pixels_per_point`" —
> was **half right and misdiagnosed**. The real cause on a large display was one line in the
> canvas:
>
> ```rust
> let fit = (avail.x / full.x).min(avail.y / full.y).min(1.0);
> ```
>
> That `.min(1.0)` meant "never magnify past 100%" — but 100% of an arbitrary 1600 px preview,
> not of the image. On a 2302-point-wide canvas it drew the photo at 1600 points: **letterboxed
> at 70% of the window, and upscaled 2× by the display on top**. The user had been zooming in to
> compensate, which put them on a native rung where this feature deliberately never fired.
>
> Removing the clamp let the photo fill the window *and* made the existing zoom ladder climb to
> the 3200 rung on its own — closing most of the gap with no new machinery. The residual was
> 3200 texels across 4604 physical pixels, a 1.44× upsample, against 1.75× before.
>
> Closing that last 1.44× costs **~2× the render** (3200² → 4608²), and the user could see no
> difference with it on or off. So it was removed: `sharpen.rs`, the interaction tracking, the
> `FINALS_CAP` bump and the debug print all came out.
>
> **What survived, and was the actual fix:** the clamp removal; deciding a *region* render by
> visibility rather than by the rung; and deriving `fit_image_size` from any whole-image frame
> via the ratio of caps.
>
> **The lesson, which cost three bugs to learn:** the measurement that opened this document was
> real, but it measured the *consequence*. Nobody checked what the fit view was actually doing
> on the display in question until the instrumentation went in — and that took one print to
> settle what four rounds of reasoning had not.

## The problem

`PREVIEW_MAX_DIM = 1600` is hardcoded, and **nothing in the app reads `pixels_per_point`**. egui
lays out in points; a Retina display renders two physical pixels per point. So on a laptop with
a ~1400-point canvas the fit view is drawn across **~2800 physical pixels from a 1600-texel
texture** — upscaled ~1.75×, visibly soft on fine detail. Every HiDPI display has this.

But correcting it is not free. Measured on the full-resolution buffer:

| fit cap | Mpx | prefix drag (NR) | **tail slider** |
|---|---|---|---|
| **1600** (today) | 1.7 | 31.2 ms | **14.6 ms** |
| 2000 | 2.7 | 48.2 ms | 23.6 ms |
| 2400 | 3.8 | 72.8 ms | 34.8 ms |
| **2800** (DPI-correct) | 5.2 | 92.7 ms | **45.9 ms** |
| 3200 | 6.8 | 119.5 ms | 59.5 ms |

Simply raising the cap makes every fit-mode slider **3.1× slower**. The two properties —
sharpness and responsiveness — are wanted at different *times*, which is what makes a
progressive answer available rather than a compromise one.

## The design

**Render at 1600 while interacting; re-render at the canvas's true resolution once idle.**

- A slider drag, pan, zoom or resize renders at `PREVIEW_MAX_DIM`, exactly as today — 14.6 ms.
- ~200 ms after the last interaction, one further render at `sharp_cap`.
- Both rungs stay in the prefix cache's `finals` tier, so returning to the fast rung on the next
  drag is a cache hit and the sharp pass after it is too.

This is the same shape as the full-resolution decode's settle timer, and reuses the same egui
lesson: **the timer must schedule its own repaint**, or an idle window never notices it expired.

## 1. `sharp_cap` — non-circular by construction

```rust
/// The fit resolution the canvas can actually display, in texels.
pub fn sharp_cap(avail_long_points: f32, pixels_per_point: f32, fit_cap: usize, buffer_long: usize) -> usize;
```

`avail_long_points × pixels_per_point`, rounded **up to a multiple of `SHARP_STEP` (256)**, then
clamped to `fit_cap ..= buffer_long`.

⚠ **It depends only on the viewport and the DPI — never on a rendered size.** That is deliberate
and load-bearing: deriving a render cap from a rendered size is exactly the feedback loop that
shrank a cropped photo to a few pixels this morning. `sharp_cap` cannot participate in one.

⚠ **Quantised to 256** so dragging a window edge does not mint a new `max_dim` every frame and
thrash the prefix cache — the same reasoning as the zoom ladder's doubling rungs.

**Deliberately generous.** The image can never be drawn larger than the viewport, so the
viewport's long edge is a safe upper bound on the image's on-screen long edge. For a portrait
image in a wide window this over-renders. Using the image's own aspect would be tighter, but the
displayed aspect is *post-geometry* — and reaching for a post-geometry quantity here is precisely
the mistake above.

## 2. The rung, and what must not move

```rust
let rung = view_region::tier_max_dim(fit_cap, buffer_long, fit, zoom);
let native = view_region::is_native(rung, fit_cap);
let max_dim = if !native && idle { sharp } else { rung };
```

⚠ **`native` is decided from the ladder rung, before the sharpen override.** `sharp > fit_cap`,
so testing `is_native(max_dim, fit_cap)` after the override would report native and request a
*region* while at fit zoom — the whole image would vanish.

⚠ **A sharp frame must not become `fit_image_size`.** It already cannot: the canvas accepts a
whole-image frame as the fit size only when `frame.max_dim == fit_cap`, and a sharp frame carries
`sharp_cap`. That guard was added this morning for a different reason and turns out to be exactly
what makes this safe. Placement still works because `pk` uses the frame's own `rendered_long`.

## 3. Detecting interaction

A local flag in `develop_body`, consumed after the panels close — the established pattern for
`photo_edited` and `want_full_res`, and the one that avoids a second `&mut self` borrow inside a
panel closure.

Any frame that issues a render request for a reason **other than sharpening** counts as
interaction: a request is only issued when an edit, pan, zoom, region or rung actually changed,
so that *is* the definition of "something happened".

`App::last_interaction: Instant`; `sharpen::SETTLE = 200 ms`.

## 4. `FINALS_CAP` 2 → 3

Three distinct `max_dim` values are now in play — the fast fit rung, the sharp fit rung, and a
native rung — competing for two protected cache slots. At two they would evict each other and
every transition would recompute a prefix.

Cost: one more complete prefix output. Worst case ~21 + 64 + 313 = ~398 MB in `finals`, against
~334 MB today.

## 5. Testing

| test | asserts |
|---|---|
| `sharp_cap_follows_the_display` | 1400 points at 2× gives 2816 (2800 rounded up to a 256 multiple); at 1× gives 1600, i.e. the fast rung, so a non-HiDPI display changes nothing |
| `sharp_cap_is_quantised` | a sweep of nearby widths yields the same value, so resizing does not thrash the cache |
| `sharp_cap_never_exceeds_the_buffer` | clamped above by `buffer_long` and below by `fit_cap` |
| `a_sharpen_is_only_wanted_at_the_fit_rung` | never while zoomed, never while a sharper frame is already showing |
| `a_sharpen_waits_for_the_settle` | below `SETTLE` no, at or above yes |
| the 31 goldens | byte-identical — no pixel path changes |

## Accepted caveats

1. **A ~200 ms lag before the view sharpens** after each edit. Deliberate; it is what keeps the
   drag itself fast.
2. **~64 MB more cache** for the extra finals slot, and a sharp render costs ~46 ms when it runs.
3. **Over-renders a portrait image in a wide window**, per §1.
4. **The sharp pass re-runs the prefix at its own resolution** the first time, so the first
   sharpen after a prefix change costs ~93 ms rather than ~46. Cached thereafter.
5. **Non-HiDPI displays get nothing**, correctly: `sharp_cap` collapses to `fit_cap` there.
