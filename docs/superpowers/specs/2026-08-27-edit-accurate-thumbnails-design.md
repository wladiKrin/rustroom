# Edit-accurate Library thumbnails

**Date:** 2026-08-27
**Status:** design agreed, ready to plan.

## What this is

The Library grid and the Develop filmstrip show the camera's **embedded JPEG**, so a thumbnail
never reflects your edits. Virtual copies raised the stakes: every copy of one file shows a
*pixel-identical* thumbnail, so a black-and-white copy appears in colour and is told apart only by
its badge and its name.

This renders the edit pipeline over that embedded JPEG instead, so a thumbnail shows what you did
to the photo. It fixes the copy case **and** every ordinary photo.

⚠ Kept out of M3.4 deliberately: doing it alongside virtual copies would have meant two large
simultaneous changes to the Library, which is the area with this project's worst defect record.
That reason has expired.

## The measurement that chose the design

`cargo run --release -p app --example bench_thumb_render <raw> [out-dir]`, kept. On an X-T4 `.RAF`:

| source | cost | note |
|---|---|---|
| **A** — embedded JPEG + pipeline | **95.6 ms** | extract 90.6 + **render 4.9** |
| **B** — half-size decode + pipeline | **933.9 ms** | decode 928.3 + rest 5.6 |
| C — extract only (today) | 118.7 ms | 512×341 |

**B is 9.8× A**, and the ratio understates it: the render itself is ~5 ms either way, so **the
whole difference is the decode**. That matters most on *invalidation*. A thumbnail must re-render
when its edits change; with A that is another ~95 ms, with B it is another ~930 ms **and cannot be
avoided by caching**, since a decoded buffer is 78 MB and a folder's worth cannot be held.

For an 830-photo folder across six workers: **A ≈ 13 s, B ≈ 2 min** — and B pays that again on
every sync or preset application.

### What A costs in fidelity, judged by looking

Both renders of the same photo with the same edits (−100 saturation, +0.8 EV) show the edit
faithfully: both are clearly monochrome, both are clearly brighter. The difference is the **base**.

⚠⚠ **A is more contrasty and its highlights are fully clipped where B holds detail.** The embedded
JPEG is 8-bit and display-referred, so the headroom is already gone. **Highlight recovery is
therefore invisible in the grid**: pull Highlights down in Develop and the thumbnail cannot show
the recovered detail, because it is not in the JPEG to recover. Shadows are compressed the same
way.

That is the honest limitation, and it is accepted: the alternative costs 10× and lags a second
behind every slider.

## Architecture

### The cache key becomes `(PhotoId, u64)`

⚠⚠ **This inverts a decision `thumb_cache` documents deliberately.** Today it is keyed by
`PathBuf` so that *copies of one file share one texture* — which is precisely the bug being fixed.
Two of the three properties that fell out of path-keying survive: `rebuild_entries` still
invalidates nothing, and indices still never leave the UI, which is what keeps this project's
sparse-versus-dense index bugs away from the boundary.

The `u64` is a **fingerprint of that copy's `Edits`**. A changed edit is then simply a cache miss,
so sync, paste, a preset click, undo, redo and a snapshot restore all behave correctly with **no
invalidation code of their own**. That is `PrefixKey`'s existing pattern, which deliberately fails
toward a miss.

⚠ The alternative — an explicit `cache.forget(id)` at every edit site — was rejected: the panel,
sync, paste, presets, undo, redo and snapshot restore all write edits, and the one that forgets
shows a stale picture with nothing to notice it.

### The fingerprint is a hash of the serialized edits

```rust
/// A fingerprint of one copy's edits, for the thumbnail cache key.
///
/// ⚠⚠ Hashed from `serde_json::to_vec(&edits)` rather than field by field, and that is the point:
/// it is **exhaustive by construction**. `Edits` has around forty fields and cannot derive `Hash`
/// (it holds `f32`), so a hand-written hash would silently omit any field added later — and the
/// symptom is a stale thumbnail, which nothing would notice. Same reasoning as `merge_from`'s
/// exhaustive destructure.
pub fn edits_fingerprint(edits: &Edits) -> u64
```

⚠ A serialization failure is impossible for `Edits` in practice, but must degrade to a value that
**varies** (e.g. include a counter) rather than a constant, or every photo would collide on one
key and show each other's thumbnails.

### Where the edits come from is two places, and that is not an oversight

- **The open photo:** `Session::edits`. ⚠ `persist_current_session` runs only on a photo switch or
  on quit, so the sidecar is stale *exactly while you are editing* — which is when the filmstrip
  most needs to update.
- **Every other photo:** a fingerprint captured in `PhotoEntry` when `entries::build` read the
  sidecar. ⚠ That function already reads every sidecar (measured 4.6 ms for 830 files), so this
  costs nothing new.

### The loader renders

`ThumbLoader`'s want-list carries `(PhotoId, Edits)` rather than a path, and a worker **extracts
and renders**, so nothing lands on the UI thread.

⚠ Its `load` becomes a boxed closure; today it is a plain `fn` pointer, which cannot capture. The
injectable seam stays — it is what makes the whole loop unit-testable with no image fixtures.

⚠ Rejected: extract in the worker and render on the UI thread. That is 4.9 ms × N per frame.

### Pending state: the previous thumbnail

On an exact-key miss the cache falls back to **any** entry for that `PhotoId`, so a re-render shows
the slightly-stale picture rather than flashing grey. It is the same photo with slightly different
edits, so it is very close, and it resolves in about 100 ms.

### Memory is unchanged, but entries can outnumber files

The extracted JPEG is **not** kept beside the texture — that would double a 512 MB budget to save
90 ms of off-thread work. An invalidated thumbnail re-extracts.

⚠ A file with three copies now holds three distinct textures where it held one, against the same
`THUMB_BUDGET_BYTES`, so a heavily-copied folder evicts sooner.

## Testing

- **The fingerprint changes when any field does.** ⚠ Asserted over a *fully non-neutral* `Edits`
  cleared one edit group at a time, in the spirit of `edit_model`'s partition tests — a test that
  names three fields cannot see the fortieth.
- **The same edits give the same fingerprint**, so a thumbnail is not re-rendered on every frame.
- **Two copies of one file with different edits get different keys**, which is the whole feature.
- **A cache miss on the exact key falls back to the same photo's previous entry**, and never to a
  *different* photo's.
- **The loader's want-list carries edits through to the render fn**, using the injected-loader seam
  so no image fixtures are needed.
- ⚠ **Appearance is out of scope for the harness**, as always here. The verification is by hand:
  a black-and-white copy must show in black and white beside its colour original.

## Accepted caveats

1. **Highlight recovery is invisible in the grid**, and shadow recovery likewise — the 8-bit JPEG
   has no headroom. The most likely thing to be reported as a bug.
2. **The thumbnail shows the camera's rendering plus your edits**, not Rustroom's own render, so it
   does not match the export exactly even with no edits.
3. **Entries can outnumber files**, so a heavily-copied folder evicts sooner.
4. **An edit costs a re-extract** (~90 ms, off-thread) rather than being served from a kept source.
5. **Still no on-disk thumbnail cache** — everything is recomputed on each folder open, as today.
6. **JPEG/PNG/TIFF sources have no embedded preview** and render from a downscale of themselves,
   which is *more* accurate than the RAW path, not less.
