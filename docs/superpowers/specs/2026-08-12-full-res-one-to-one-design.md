# A full-resolution buffer for true 1:1 — design

Date: 2026-08-12
Status: approved

## Goal

What Rustroom calls "100%" is not the image. The editing buffer is a **half-size** RAW decode,
and libraw's half-size path **skips demosaic entirely** — it bins 2×2 sensor cells — while
export runs a real demosaic algorithm. So inspecting sharpening or noise reduction at 1:1 today
means judging detail at roughly **twice** the radius it will actually have, on pixels made a
different way from the ones that get exported.

Decode the full-resolution image in the background and render 1:1 against that.

## What the measurements said

X-T4 `.RAF`, release. `Demosaic::Fast` (the default) throughout.

| | value |
|---|---|
| half decode (today) | 985 ms → 3123×2085, 78 MB |
| **full decode** | **3026 ms** → 6246×4170, **313 MB** |
| `downscale_to(1600)` from half | 9.1 ms |
| `downscale_to(1600)` from full | **27.3 ms** |
| geometry (angle 3°) whole-buffer at full res | 203 ms |
| noise reduction 50/50 at full res | 504 ms |
| dehaze 60 at full res | 562 ms |
| **region tail 1400×900 at full res** | **13.4 ms** |

Four conclusions, and each one shapes a decision below:

1. **Editing at true 1:1 is affordable.** A tail slider costs 13.4 ms, because the tail runs
   only on the visible region. This is the common case and it is fine.
2. **Prefix sliders at true 1:1 are not.** The prefix runs *whole-buffer* by construction — that
   is what lets dehaze estimate a frame-global airlight and what stops noise reduction seaming —
   so noise reduction or dehaze costs ~0.5 s per frame while zoomed.
3. **The fit path must not be made worse to improve 1:1.** `downscale_to` triples to 27.3 ms
   from a full-resolution source. Fit-mode editing is the common case and must not regress.
4. **Memory is again the binding constraint.** The buffer is 313 MB and every prefix checkpoint
   is another 313 MB.

## Decisions taken

- **Trigger: on zoom, not on open.** Browsing a folder costs nothing extra.
- **Cache budget raised to 512 MiB, plus a new 6-entry cap.** See §4.
- **Three render tiers, not two.** This one emerged while writing the spec; see §1, and the
  correction note there.

## 1. Three render tiers

⚠ **A correction to the design as first presented.** The plan was "native mode uses the full-res
buffer". That is wrong, and quietly expensive.

Native mode today spans on-screen scales from ~0.51 to 1.0 rendered-pixels-per-screen-pixel,
because `max_dim` is quantized to exactly two values. Swapping the native buffer from 3123 to
6246 doubles `k` and therefore **halves** the on-screen scale, to a range of ~0.26–1.0. The
render region is derived from the viewport *in rendered pixels*, so at 0.26 a 1400×900 viewport
needs a **5385×3462** region — 18.6 Mpx instead of 1.26 Mpx. The 13.4 ms tail becomes ~180 ms,
and the "true 1:1" feature would make slider response worse over most of its own zoom range.

The fix is to keep the invariant the two-tier design already had — *on-screen scale stays within
(0.5, 1.0] in every tier* — by adding a tier rather than replacing one:

| tier | `max_dim` | active when | on-screen scale |
|---|---|---|---|
| Fit | `PREVIEW_MAX_DIM` (1600) | `fit · zoom ≤ 1.0` | ≤ 1.0 |
| NativeHalf | half buffer long edge (3123) | `1.0 < fit · zoom ≤ r` | (0.51, 1.0] |
| NativeFull | full buffer long edge (6246) | `fit · zoom > r` | (0.5, 1.0] up to `2r`, then > 1.0 |

where `r = half_buffer_long / fit_long` (≈1.95 on the X-T4) — the point at which the NativeHalf
texture is itself being magnified past its own pixels, exactly the reasoning that sets the
Fit→NativeHalf boundary at 1.0.

⚠ **State the invariant precisely or a test written from it will be wrong.** It holds for the
*native* tiers only, and NativeFull's upper end is deliberately open: beyond `fit · zoom = 2r`
(≈3.9, i.e. 2:1 on screen) the full-resolution texture is genuinely being magnified, because
there is no more data in the file. Fit has no lower bound either — zoomed out it is arbitrarily
small, which is what "fit" means. So the testable statement is: **within a native tier and below
`2r`, on-screen scale lies in (0.5, 1.0]** — never magnifying, and never rendering more than
about twice the pixels displayed.

Two things fall out for free:

- **The full decode is triggered later and more rarely** — only past ~1.95× fit zoom, which is
  precisely where the half-size buffer stops having the pixels. That is a better trigger than
  "any native mode" on its own terms, not merely a cheaper one.
- **Region sizes stay bounded**, so the 13.4 ms tail figure holds across the whole zoom range.

`view_region::needs_native(fit, zoom)` generalises to a `RenderTier` decision. Keeping it in
`view_region` — pure, `egui`-free — is the same argument that put the viewport maths there: an
off-by-one in a tier boundary would otherwise only show up as a mysteriously slow or blurry
view.

## 2. Two buffers, two engines

`Session.engine` and the half-size buffer **stay exactly as they are** and serve Fit and
NativeHalf. `Session` gains:

```rust
struct FullRes {
    engine: RenderEngine,
    /// Long edge of the full-resolution buffer — `max_dim` for the NativeFull tier.
    long: usize,
}

// on Session
full: Option<FullRes>,
```

`RenderEngine` owns its buffer for its thread's lifetime, so a second buffer means a second
engine. That is the cheap option here rather than the expensive one: it needs **no change to
`render_engine.rs`** beyond the two cache constants, each engine gets its own prefix cache
sized to its own resolution, and Fit-mode rendering is byte-for-byte unchanged — including
never paying the 27.3 ms downscale.

The **active engine** is `full` when the tier is NativeFull and `full` is present, and `engine`
otherwise. Two consequences to implement deliberately:

- **Both engines are drained every frame, but only the active engine's newest frame is
  applied.** Discarding the other's is safe *because* any change of active engine clears
  `requested_region`, which forces a fresh request. That coupling is load-bearing: without it a
  discarded frame would never be replaced.
- **`Session::render_max_dim` and the three `buffer_long` uses in the canvas** become tier
  lookups. `native_long()` returns `full.long` when present and the half buffer's long edge
  otherwise, so an absent or failed full decode degrades to exactly today's behaviour.

## 3. The trigger and the handover

A new `app::full_res` module holds the decision as pure functions, testable without `egui` or
libraw:

```rust
/// Whether to start a full-resolution decode.
pub fn wants_upgrade(is_raw: bool, tier: RenderTier, has_full: bool, in_flight: bool) -> bool;
```

⚠ **`is_raw` is not a detail.** JPEG, PNG and TIFF are already decoded at full size with
`source_scale` 1.0, so their 1:1 view is already honest and an "upgrade" would be a pointless
3-second re-decode of the same pixels.

The decode reuses **`DecodeWorker`** with a second instance at `max_concurrent: 1`, injected
with `DecodeOptions::linear_scene()`. This buys the abandon-don't-abort semantics, the sequence
numbering that drops stale results, and all four of its existing tests, for the price of one
constructor call. `max_concurrent: 1` because a 313 MB decode is not something to run two of.

**On arrival**, the session builds a `RenderEngine` over the new buffer, stores it, and clears
`rendered_region` and `requested_region`. ⚠ **That clear is load-bearing:** region rects are in
*rendered* pixels, and those coordinates double when the buffer does. Without it the next frame
would reuse a rect describing the wrong half of the image.

**The baseline the worker renders is discarded.** `DecodeWorker` always renders one; here the
existing preview-resolution baseline is kept, so before/after does not change under the user.
The waste is one ~60 ms render on a background thread during a 3 s decode.

**A failed decode is reported in the status line and otherwise ignored** — `full` stays `None`
and NativeFull silently falls back to NativeHalf, which is today's behaviour.

## 4. Cache sizing

Two constants in `render_engine.rs`:

```rust
const MID_BUDGET_BYTES: usize = 512 << 20;
const MIDS_CAP: usize = 6;
```

**512 MiB rather than 320.** A full-res checkpoint is 312.5 MB; 320 MiB (335.5 MB) clears it by
7%, covering sensors to 28.0 Mpx. 512 MiB covers **44.7 Mpx**, which matters because Fujifilm's
current bodies (X-T5, X-H2) are 40.2 Mpx and would get *zero* native checkpoints at 320. On the
X-T4 the two values behave identically — one checkpoint either way, since two would need
≥626 MB — so this is free today and buys the next camera.

**⚠ The 6-entry cap is what makes raising the budget safe, and it fixes a pre-existing leak.**
Dragging Straighten mints a *new* post-geometry key every frame, so the mids tier fills with
stale checkpoints nobody will ever return to, bounded only by the byte budget. A bigger budget
therefore means more retained garbage. A render's useful chain is at most 3 mids, so a cap of 6
keeps the current chain plus one previous state and discards the rest. Bytes stop being the
thing that bounds garbage.

Worst-case memory for one photo, all three prefix stages active, zoomed to NativeFull:

| | |
|---|---|
| half buffer (shared with `BufferCache`) | 78 MB |
| other 2 `BufferCache` entries | 156 MB |
| full-res buffer | 313 MB |
| fit engine cache (≤6 mids + final @ 21 MB) | 147 MB |
| native engine cache (1 mid + final @ 313 MB) | 626 MB |
| **total** | **~1.32 GB** |

Lower than the ~1.51 GB a 320 MiB budget without the cap would have reached.

## 5. Testing

The valuable tests here are the pure ones; the rest is verified by eye, because "the image is
actually sharper" is not something a unit test can assert without a RAW fixture in the repo.

| test | asserts |
|---|---|
| `the_tier_boundaries_follow_the_buffer_ratio` (`view_region`) | Fit below 1.0, NativeHalf between 1.0 and `r`, NativeFull above — including exactly at both boundaries |
| `a_native_tier_never_magnifies_its_own_texture` (`view_region`) | the invariant the tiers exist for, in the precise form given in §1: over a sweep of zooms from 1.0 to `2r`, on-screen scale in the chosen native tier stays in (0.5, 1.0] |
| `an_absent_full_buffer_falls_back_to_native_half` (`full_res`) | `native_long()` with `full: None` equals today's value |
| `only_raw_wants_an_upgrade` (`full_res`) | JPEG/PNG/TIFF never trigger a decode |
| `an_upgrade_is_not_requested_twice` (`full_res`) | neither while one is in flight nor once one has landed |
| `the_mids_cap_bounds_stale_checkpoints` (`render_engine`) | inserting many distinct mids leaves at most `MIDS_CAP`, newest kept |
| `the_budget_still_evicts_before_the_cap` (`render_engine`) | with entries larger than the budget, bytes bind first |
| `a_full_size_decode_is_twice_the_half_size_one` (gated on `RAW_FIXTURE`) | `decode_full_res` really returns full resolution and `source_scale` 1.0 — without it a typo would leave the feature inert but plausible |
| the 29 goldens | byte-identical; no pipeline or stage is touched |

## Non-goals

- **The export path.** It already decodes full-resolution; nothing changes.
- **Non-RAW formats.** Already honest at 1:1.
- **Prefetching the next photo's full decode.** Considered and rejected: it bets on a 1:1 pass
  over a shoot, and is wrong whenever you zoom into exactly one frame.
- **Caching the full-res buffer across photo switches.** It would quadruple `BufferCache`'s
  footprint. Dropped with the session.
- **Making prefix sliders fast at 1:1.** Bounded by one checkpoint, §4; the rest is inherent.
- **Parallelising `downscale_to`.** It stays on the half-size path, where it is 9.1 ms. Worth
  doing eventually; not needed for this.

## Accepted caveats

1. **A ~3 s wait the first time you zoom past ~1.95×**, showing the half-size render meanwhile
   with a status line. Subsequent zooms on the same photo are instant.
2. **~1.32 GB worst case for one photo** while zoomed with everything on.
3. **Prefix sliders cost ~0.2–0.6 s per frame at NativeFull.** One checkpoint softens it; the
   rest is the price of a whole-buffer prefix. Tail sliders stay at 13.4 ms.
4. **⚠ The 1:1 view uses `Demosaic::Fast`, not the export preset's setting.** Matching it means
   plumbing a parameter through `DecodeWorker`'s injected decode fn and changing its tests. The
   measured Fast-vs-Best gap is small — which is why export defaults to Fast — so this is a
   contained follow-up rather than a gap. Until then, a 1:1 view still differs from a `Best`
   export in kind, though far less than today's binned half-size does.
5. **Switching photos and back re-decodes**, per the non-goal above.
6. **The before/after baseline stays preview-resolution**, so toggling it while zoomed still
   shows a softer image. Pre-existing and unchanged.
7. **Two engines mean two prefix caches**, so the same edit can be computed twice — once per
   buffer. Correct, and the alternative (sharing checkpoints across resolutions) is meaningless
   since the pixels differ.
