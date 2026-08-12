# Eager full-resolution decode, and one engine — design

Date: 2026-08-12
Status: approved

Supersedes the trigger and buffer-ownership decisions in
`2026-08-12-full-res-one-to-one-design.md`. The tier *idea* from that document survives; its
zoom trigger and its two-engine structure do not.

## Why the trigger moves

Zoom-triggered decoding was chosen to avoid spending 3 s and 313 MB on photos nobody inspects.
Using it revealed the flaw, and measuring it quantified the flaw:

| tier | mean diff vs a properly demosaiced render | samples differing >2/255 | max |
|---|---|---|---|
| Fit (≤1× zoom) | 0.91 | **5.5%** | 83 |
| **NativeHalf (1.0–1.95× zoom)** | 1.69 | **22.6%** | 211 |

⚠ **The gap is not at fit — it is at moderate zoom.** At 1.0–1.95×, an ordinary editing zoom,
nearly a quarter of the image is visibly wrong and the zoom threshold is never crossed. The
half-size path bins 2×2 sensor cells instead of interpolating, so edges carry false colour that
a real demosaic reconstructs. That is what the user saw at 1:1 and correctly suspected was
present sooner.

**Lightroom, for reference.** Its Develop module always works from a full decode of the
original; it has no half-size-then-upgrade model. The Library previews (Minimal / Standard /
1:1, cached in `.lrdata`) are a separate browsing cache, not what Develop renders from. Adobe's
one sanctioned downgrade is **Smart Previews** — an opt-in 2560 px lossy DNG, with edits
reapplied to the original at export. (Unverified: whether current Lightroom Classic still uses a
cheaper demosaic at fit zoom internally, as Camera Raw historically did.)

## Decisions

1. **Trigger: once the `Session` has settled**, not on zoom.
2. **Every tier renders from the full buffer** once it exists.
3. **Therefore one engine**, whose buffer is replaced — the half-size engine becomes dead weight
   the moment the upgrade lands.
4. **The tier ladder generalises**, replacing the fixed three-variant enum.

The cost this was going to carry no longer exists: `downscale_to` is now parallel, so fit
sourced from the full buffer is **8.6 ms against today's 9.1 ms** from the half one.

## 1. The tier ladder

`RenderTier`, `tier_for` and `clamped` go. In their place, one function:

```rust
/// The `max_dim` to render at: the rungs are `fit_long`, doubling, capped at `buffer_long`.
pub fn tier_max_dim(fit_long: usize, buffer_long: usize, fit: f32, zoom: f32) -> usize;
```

Rungs double so on-screen scale can never fall below ~0.5, and are quantized so `max_dim` does
not vary continuously with zoom — which would rebuild the prefix cache on every scroll tick.
The ladder falls out of the buffer rather than being enumerated, so it yields **two** rungs on a
3123 buffer (exactly today's pre-upgrade behaviour) and **three** on a 6246 one, on any sensor,
with no enum to extend.

`is_native` becomes `max_dim > fit_long`.

**The invariant, stated precisely** — the loose form would produce a failing test, as it did
before. For a chosen rung `m`, with on-screen scale `fit·zoom·fit_long / m`:

- if `m > fit_long`, then `scale > 0.5` — never rendering more than ~2× the displayed pixels;
- and additionally `scale ≤ 1.0` **unless `m == buffer_long`**, where magnification is
  unavoidable because the file holds no more data.

At `m == fit_long` there is no lower bound: zoomed out, fit is arbitrarily small. That is what
fit means.

## 2. One engine, replaced

`FullRes`, `Session::full`, `Session::active_full`, the dual polling and the engine routing in
`Session::request` all come out. `Session` instead gains:

```rust
    /// Whether `engine`'s buffer is already the full-resolution decode.
    upgraded: bool,
    /// When this session was created — the dwell the upgrade trigger waits on.
    opened_at: Instant,
    /// The resolution the canvas last chose, so an edit made from a panel re-renders at the
    /// tier currently on screen rather than snapping back to fit.
    render_max_dim: usize,
```

On arrival the session **replaces** `engine` with one over the new buffer and updates
`buffer_long`. Dropping the old engine disconnects its channel, so its worker thread returns on
its own.

⚠ **`rendered_region` and `requested_region` must still be cleared**, for the same reason as
before: region rects are in *rendered* pixels, and those coordinates change with the buffer.

⚠ **The old engine's prefix cache is lost.** Accepted: it is a one-off cost at the moment of
upgrade, and keeping it would mean keeping the half-size buffer alive to no purpose.

`Session::request` stays as a one-line delegate. It no longer routes anywhere, but "every render
request goes through one place" is a property worth keeping, and reverting five call sites is
churn for nothing.

## 3. The trigger

```rust
/// How long a session must have been open before its full-resolution decode starts.
pub const SETTLE: Duration = Duration::from_millis(800);

pub fn wants_upgrade(is_raw: bool, upgraded: bool, in_flight: bool, age: Duration) -> bool;
```

⚠ **The dwell is not redundant with the existing debounce.** There is already an implicit ~1.2 s
before a `Session` exists (180 ms debounce plus a ~1 s half decode) — but a **`BufferCache` hit
creates a `Session` instantly**, so arrowing back through recently viewed photos would fire a
3 s decode each. The dwell closes that.

⚠ **The trigger must schedule a repaint while it waits.** egui does not redraw an idle window,
so without `request_repaint_after` the dwell could expire with nothing to notice it and the
decode would start only when the user next moved the mouse.

The check moves out of the canvas closure into its own `&mut self` method called from `ui()`.
It no longer depends on the tier, which removes the flag-through-the-panel-closure dance and
the hoisted `upgrade_in_flight` local along with it.

`full_res::native_long` goes: with one buffer there is nothing to choose between.

## 4. Testing

| test | asserts |
|---|---|
| `the_ladder_doubles_from_fit_to_the_buffer` | rungs are 1600/3123 on a half buffer and 1600/3200/6246 on a full one, at the boundary zooms |
| `a_rung_below_the_buffer_never_magnifies` | the precise invariant of §1, swept over zoom, on both buffer sizes |
| `the_ladder_is_quantized_not_continuous` | small zoom changes within a rung return the *same* `max_dim` — the property that keeps the cache alive |
| `a_tiny_image_has_only_one_rung` | `buffer_long <= fit_long` yields `fit_long` at every zoom, no division or overflow |
| `the_dwell_gates_the_upgrade` | below `SETTLE` no; at or above, yes |
| `an_upgrade_is_not_requested_twice` | kept — neither in flight nor once upgraded |
| `only_raw_wants_an_upgrade` | kept |
| `a_full_size_decode_is_twice_the_half_size_one` | kept, gated on `RAW_FIXTURE` |
| the 29 goldens | byte-identical |

## Non-goals

- **Caching the full-resolution buffer across photo switches.** Still per-session.
- **Matching the export preset's demosaic.** Still `Demosaic::Fast`; contained follow-up.
- **A preference to disable it.** Considered (Lightroom's Smart Preview toggle is the analogue)
  and rejected: a settings field plus a rarely exercised code path, for a choice few would change.

## Accepted caveats

1. **~3 s of background CPU and 313 MB per photo you stop on**, whether or not you zoom.
2. **The prefix cache is lost at the moment of upgrade**, so the first render after it is cold.
3. **Between opening and the upgrade landing (~4 s), every tier is still the binned half-size
   decode.** Better than before, when it was until you zoomed past ~1.95× — but not instant.
4. **Peak memory is unchanged in shape** but now reached on every photo you dwell on rather than
   only zoomed ones.
5. **A photo switched away from and back re-decodes**, per the non-goal above.
