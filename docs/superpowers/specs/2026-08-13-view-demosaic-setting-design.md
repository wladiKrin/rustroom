# A 1:1 view quality setting

**Status:** spec, 2026-08-13.

## The problem

`decode_full_res` hardcodes `Demosaic::Fast` — whatever `DecodeOptions::linear_scene()`
carries — while an export preset can ask for any of the four tiers. So what you inspect at
1:1 is not necessarily what you ship.

That has been recorded as caveat (6) of the full-resolution work since it landed. Examining
it before building anything changed the shape of the fix:

- **The default case has no mismatch.** `linear_scene()` sets `Fast`, and
  `ExportOptions::default()` uses `Demosaic::default()`, which is also `Fast`. The gap opens
  only when a user deliberately picks a slower tier for export.
- **Mirroring the export preset was rejected.** "The export preset's demosaic" is not a single
  value — there are N named presets plus `settings.export` as the last-used — and the same
  photo legitimately goes out as web-JPEG-fast *and* archive-TIFF-best. Any automatic choice
  makes the view's decode cost depend on distant state.
- **The cost is severe and recurring.** The full-resolution decode is eager: it fires ~800 ms
  after you settle on any photo. Measured end to end on an X-T4 `.RAF`, `Fast` is 2.89 s and
  `Best` is 11.42 s. Mirroring `Best` would quadruple a background cost incurred on every
  photo you look at, for a difference validated on 2026-08-11 as "marginally crisper
  hair-strand separation".
- **A far larger mismatch is already accepted.** For the first ~4 s after opening a photo the
  buffer is the half-size decode, which skips demosaic *entirely* — 2×2 binning, not a worse
  algorithm but none. That is caveat (3) of the same feature and nobody minds.

So the real need is narrower than the caveat implies: someone tuning Sharpening or Texture at
1:1 for a `Best` archival export wants to inspect at that quality. That is served by making
it an **explicit choice**, not by inferring one.

## Decisions taken

| Question | Decision |
|---|---|
| Mirror the export preset? | **No.** A separate, explicit setting. |
| Changing it with a photo open | **Re-decode that photo immediately**, so the setting visibly does something. |
| How the cost is described | **A multiplier, not seconds** — seconds are camera-specific. |

## Design

### 1. `Settings.view_demosaic: Demosaic`

```rust
pub struct Settings {
    pub confirm_preset_delete: bool,
    pub export: ExportOptions,
    pub confirm_export_preset_delete: bool,
    /// Demosaic tier for the full-resolution editing buffer — the 1:1 view.
    pub view_demosaic: Demosaic,
}
```

Default `Fast`, which is what `linear_scene()` already hardcodes, so nobody who ignores this
setting sees any change. `Settings` already carries `#[serde(default)]`, and `Demosaic`
already has `Serialize`/`Deserialize`, `Default` and `Eq` from its export use — the field
costs no new derives anywhere.

⚠ `settings.rs`'s module doc says settings move in "when they are actually wanted, not
speculatively". This one is wanted; the doc comment stands unchanged.

### 2. The tier is captured in the closure, and the worker is rebuilt on change

`DecodeWorker::decode` is an injected `Arc<dyn Fn(&Path) -> Result<…>>` with nowhere to put a
parameter. Two alternatives were rejected:

- **Adding the tier to `DecodeWorker::request`** would impose it on the *preview* worker as
  well, where demosaic is inert by documentation — a half-size decode skips it entirely. A
  shared type should not grow a field only one of its two users can act on.
- **A shared `Arc<AtomicU8>` read inside the closure** avoids the rebuild but makes the
  decode function depend on mutable state at a distance, for a setting that changes at most a
  handful of times per session.

Instead a small constructor:

```rust
/// Build the full-resolution decode worker for `tier`.
///
/// The tier is captured rather than passed per request, because `DecodeWorker` is shared
/// with the preview decode where demosaic is inert.
fn full_res_worker(tier: Demosaic, ctx: &egui::Context) -> DecodeWorker;
```

called from `RustroomApp::new` and again when the setting changes. Dropping the old worker
disconnects its result channel, so a decode already in flight finishes and discards its own
result — the same abandon-don't-abort trade the full-resolution handover already makes.

### 3. Changing it re-decodes the open photo

On a change:

1. rebuild `full_decoder` with the new tier
2. clear `full_pending` — that sequence number belongs to the discarded worker
3. for a **RAW** session only, set `buffer_is_full_res = false`

`maybe_request_full_res` then fires on the next frame: `opened_at` is long past
`full_res::SETTLE`, so there is no second dwell. The existing "Loading full resolution…"
status line reports it, and the old buffer stays on screen until the new one lands, so
nothing flickers.

⚠ RAW only. For JPEG/PNG/TIFF `buffer_is_full_res` is true because the buffer really is the
whole file, and clearing it would be a lie — though `maybe_request_full_res` would refuse
anyway on its `is_raw` check.

### 4. The warning: a multiplier, not seconds

The export dialog says "roughly 1.2 s to 11 s per photo". Those are X-T4 numbers written into
prose, and wrong on any other body. Here it matters more, because the cost is **recurring** —
the decode is eager, so a slower tier is paid on every photo you stop on, not once per export.

Ratios are stable where seconds are not, so the hint is a multiplier derived from the measured
end-to-end times (1.25 / 2.89 / 6.9 / 11.42 s):

| tier | hint |
|---|---|
| `Fastest` | about 0.4× the decode time of Fast |
| `Fast` | the default |
| `Better` | about 2.4× the decode time of Fast |
| `Best` | about 4× the decode time of Fast |

A pure `fn view_quality_hint(Demosaic) -> &'static str` in the app, beside the settings
dialog. Not in `raw-decode`: a cost multiplier measured on one body is presentation, not a
property of the algorithm, and `raw-decode` already exposes the neutral facts (`user_qual`,
`label`).

The surrounding warning text states four things:

1. **RAW only** — there is no demosaic step for JPEG, PNG or TIFF.
2. **It affects the zoomed view, not export.** Export keeps its own per-preset setting.
3. **The cost recurs per photo**, because the full-resolution decode is eager.
4. **The measured difference is small** — which is why the default is `Fast`.

### 5. The export dialog's hint is now half-false

It currently ends "Not shown in the preview, which skips demosaic entirely." That remains true
of the fit preview before the full-resolution buffer lands, and false of the 1:1 view once it
has. The clause is corrected and points at the new setting.

## Testing

Thin, and deliberately not padded — most of this is egui glue and worker rebuilding, verified
by running the app. Two tests earn their place:

- `a_settings_file_from_before_view_demosaic_still_loads` — the guard `ExportOptions` needed
  for the same reason. `SettingsStore::load()` is infallible, so a field that fails to parse
  silently reverts **every** setting, which is worse than an error.
- `every_demosaic_tier_has_a_cost_hint` — a table test over `Demosaic::ALL`, so a fifth tier
  cannot ship showing an empty hint.

## Accepted caveats

1. **A non-default tier is paid on every photo you dwell on**, not per export — roughly 4×
   the decode at `Best`. That is the point of the warning.
2. **Changing the setting spends a full decode immediately** on the open photo.
3. **The handover drops the prefix cache**, so the first render after the new buffer lands is
   cold. Identical to the original full-resolution upgrade.
4. **It does nothing for JPEG/PNG/TIFF**, which have no demosaic step.
5. **It still does not guarantee the view matches a given export**, and is not meant to — it
   lets you choose what to inspect at. Two exports of one photo at different tiers remain two
   different results.
6. **The tier does not apply to the first ~4 s after opening**, during which the buffer is the
   binned half-size decode with no demosaic at all.
