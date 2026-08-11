# Demosaic backend selection — design

**Date:** 2026-08-11
**Status:** approved, ready to plan
**Scope:** `raw-decode` (the type and one libraw param), `app` (`ExportOptions`, preset, one dropdown), `rustroom` (one CLI flag)

First item past Phase 2. Small, and the largest measured win available.

## Why

**We never set libraw's `user_qual`, so every full-resolution decode inherits AHD — its
slowest setting — because nobody chose.** That is ~93% of an 11 s export, and a 47-photo
batch costs ~9 minutes rather than ~2.

## The measurement this design is built on

Taken before designing, on the X-T4 `.RAF`, full resolution, release build. `unpack` is a
flat ~845 ms in every case and is not included:

| `user_qual` | Bayer name | process | output |
|---|---|---|---|
| 0 | bilinear | **400 ms** | distinct |
| 1 | VNG | 2,049 ms | distinct |
| 2 | PPG | 6,888 ms | distinct |
| 3 | AHD | 10,575 ms | distinct |
| 4 | DCB | 10,508 ms | **byte-identical to 3** |

Reproduce with `cargo run --release -p raw-decode --example demosaic_probe <raw> <out-dir>`.

Three findings, each of which changed the design:

1. **⚠ On X-Trans, `user_qual ≥ 3` collapses.** AHD and DCB produce *byte-identical* output at
   identical cost, because libraw routes X-Trans through Markesteijn variants rather than the
   Bayer algorithms these names denote. So there are **four** distinct outcomes on this
   camera, not five or seven.
2. **⚠ The Bayer algorithm names are therefore misleading as labels.** Selecting "DCB" on a
   Fuji gets you Markesteijn. A name that is false on the user's own camera is worse than no
   name.
3. **The quality gap is real but subtle.** Comparing centre crops of fur, `user_qual 3` has
   slightly crisper hair-strand definition than `0`; `0` is a touch softer. It is not a
   dramatic difference — and it costs **26×** the time. That is what makes the default a
   judgement call rather than an obvious "use the best".

## The type

`Demosaic` lives in **`raw-decode`**: it is a decode concept, and the `user_qual` mapping
should exist in exactly one place.

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Demosaic {
    /// libraw `user_qual` 0 — bilinear on Bayer.
    Fastest,
    /// libraw `user_qual` 1 — VNG on Bayer. The default.
    #[default]
    Fast,
    /// libraw `user_qual` 2 — PPG on Bayer.
    Better,
    /// libraw `user_qual` 3 — AHD on Bayer.
    Best,
}

impl Demosaic {
    pub fn user_qual(self) -> i32;
}
```

⚠ This adds `serde` to `raw-decode`, which is a small leak of an app concern into a low-level
crate. Accepted deliberately: the alternative is a mirror enum in `app` plus a **second copy
of the `user_qual` mapping**, and a mapping that exists twice is a mapping that will disagree.

**Four tiers, not seven.** DCB is measurably identical to `Best` on X-Trans, and DHT/AAHD
would need validation on a Bayer body we do not have. Four honest options beat seven where
three are indistinguishable.

**Default is `Fast`.** 2.0 s against 10.6 s — a 47-photo batch drops from ~9 min to ~2.3 —
while remaining visibly better than bilinear. `Best` is one dropdown click away when a frame
deserves it. ⚠ This changes output relative to anything exported before today; the previous
value was never chosen, and this is early development.

## Labelled by tier, not by algorithm

The dropdown reads **Fastest / Fast / Better / Best**.

This follows directly from finding 2. A tier name is true on *every* sensor; an algorithm name
is true only on Bayer. The `user_qual` value and its Bayer meaning belong in the code doc
comments and `CLAUDE.md`, where they are accurate as implementation detail rather than
misleading as a label.

## Plumbing

- `DecodeOptions` gains `demosaic: Demosaic`. `backend_rsraw::decode` sets
  `params.user_qual = opts.demosaic.user_qual()` in the block that already sets
  `output_color`, `no_auto_bright` and `gamm`.
- `ExportOptions` gains it, so the **preset carries it** — structurally right, because this
  only affects full-resolution decodes, which is exactly what export does.
- The CLI's `render` gains `--demosaic`, for parity: it silently inherits AHD today too.

**⚠ `DecodeOptions::preview()` carries the field but it is inert.** `half_size` makes libraw
skip demosaic entirely (2×2 binning), so this setting has *no effect whatsoever* on the
preview. Documented at the field rather than left to be rediscovered — and asserted, below.

Nothing else needs to know. In particular `PrefixKey` and `BufferCache` are unaffected: the
setting changes only export decodes, which are not cached.

## Tests

- `user_qual` mapping pinned to hard numbers (0/1/2/3), `Default` is `Fast`, serde round-trip.
- `ExportOptions` round-trips it through **both** the settings file and a saved preset.
- An older settings file and an older preset without the field still load, defaulting — the
  same forgiving-load property `ExportOptions::default()` exists to provide.
- **⚠ One test gated on `RAW_FIXTURE`**, like the existing decode smoke test: two tiers must
  produce **different** pixels at full resolution and **identical** pixels at half size. That
  turns "the preview cannot show this" from a comment into an assertion, and it is the single
  claim in this design most likely to be quietly wrong.

`demosaic_probe` stays as a permanent diagnostic rather than being deleted: it is how the
mapping gets re-validated after a libraw upgrade, or on a Bayer body, and it is what produced
the evidence above.

## Accepted caveats

1. **⚠ You cannot preview the choice.** The preview skips demosaic, so judging it means
   exporting a frame at two settings and comparing crops at 1:1. The tier labels are honest
   about being a speed/quality trade rather than pretending to describe a visible control.
2. **⚠ On X-Trans the tiers describe speed, not algorithm.** libraw runs Markesteijn variants
   regardless, and `≥3` collapses. On a Bayer body the Bayer names do apply — which is exactly
   why the label cannot use them.
3. **Changing the default changes output** relative to previous exports.
4. **⚠ `unpack` is ~845 ms and unavoidable, so the headline ratios overstate the win.** The
   real per-photo cost is `unpack + process`:

   | tier | end to end | speedup vs `Best` |
   |---|---|---|
   | `Fastest` | 1.25 s | **9.2×** |
   | `Fast` (default) | 2.89 s | **3.9×** |
   | `Best` | 11.42 s | 1× |

   So moving the default to `Fast` is a **3.9×** improvement, not the 5× the process-only
   column suggests, and `Fastest` is 9.2× rather than 26×. Quote these numbers, not the
   process-only ones.
5. Not exposed per-photo; it is an export setting, not a develop edit.
