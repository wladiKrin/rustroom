# Honouring Immich's server-side stacks

**Date:** 2026-09-25
**Status:** approved in brainstorming, ready for a plan
**Completes:** the second half of the stacking work; the first is
`2026-09-25-local-stacking-design.md`

---

## 1. Why this is a separate feature

Local stacking groups same-stem files, and **cannot** help here: `immich_source::cache_path` names
every cached file `<asset UUID>.<ext>`, so no two Immich files ever share a stem. Measured: **0 stem
pairs against 2,913 cached assets**, where both camera folders are 100%.

Meanwhile the server holds **2,379 stacks** — created by the one-time migration on 2026-09-24 — and
Rustroom's Immich grid still shows both halves of every pair.

⚠ The wiring is already right: Immich albums and the timeline go through `open_scan` →
`library_state::browser_entries`, the one function that groups. This feature supplies the missing
*input*, not a second grouping path.

---

## 2. The whole idea, in one line

`stack::key_of` returns `(folder, upper-cased stem)`. For a row whose asset is in a server stack it
returns **`(cache_root, "stack:" + stack_id)`** instead.

⚠⚠ **Everything else then works unchanged** — collapsing, expanding, the count badge, the toolbar
buttons, the primary rule, order preservation, the copy-0 restriction. That is the reason to do it
this way rather than build a parallel mechanism: there is one grouping implementation and one set of
tests behind it.

So `group` gains one argument — a map from **asset id → stack id** — and an empty map degrades to
exactly today's behaviour.

⚠ The asset id is already the path stem; `immich_source::asset_id_of` is exactly that function.

---

## 3. What is fetched, and what is kept

⚠⚠ **`GET /stacks` is 6.6 MB, measured twice at ~2.9 s**, because it embeds the **full asset DTO**
for every member — 20-plus fields each — when Rustroom needs two UUIDs. There is no lean mode: the
endpoint's only parameter is `primaryAssetId`.

So a new `immich_stack_cache` stores the **projection**, roughly 350 KB:

```
asset id  →  stack id
stack id  →  member count      (for the badge)
```

### The DTO must stay narrow, and the fixture must be captured

```rust
pub struct Stack { pub id: String, pub primary_asset_id: String, pub assets: Vec<StackMember> }
pub struct StackMember { pub id: String }
```

⚠⚠ **The fixture is a captured reply, trimmed — and it says so.** This integration has been bitten
four times by fixtures written from a schema: `album.json` asserted an `assets` field the endpoint
never returns and passed against 2,678 green tests. A real reply is already captured for this one.

### Invariants measured on the real reply, and what the code does anyway

| measured 2026-09-25 | value |
|---|---|
| stacks | 2,379, **all exactly 2 members** |
| member ids | 4,758, **all distinct** — an asset is in at most one stack |
| primary also a member | **2,379 of 2,379** |

⚠⚠ **These are facts about this server today, not guarantees from the API**, so the code must not
depend on them. An asset appearing in two stacks takes the **last** one seen, deterministically, and
a stack with fewer than two members is **dropped** — a stack of one would render a badge reading "1",
which `Stack::count`'s own doc forbids.

---

## 4. Fetching and freshness

A new `Job::Stacks` on `ImmichWorker`, mirroring `Albums` and `Buckets`, so the ~2.9 s never blocks
the UI — the grid regroups when it lands. The projection is cached beside the asset lists, and
**⋯ → Reindex drops it**, which is how asset lists already behave and matches the mental model the
user already has.

⚠⚠ **No validator is possible, and that is a real limitation rather than an oversight.** Asset lists
revalidate on `(assetCount, lastModifiedAssetTimestamp)`; `StackResponseDto` carries only `id`,
`primaryAssetId` and `assets` — there is nothing to compare. **So a stack created in Immich does not
appear in Rustroom until Reindex**, and that is accepted (the user's call, of three options).

⚠ A failed or absent fetch means **no grouping**, never an error and never a stale guess: the grid
shows both halves, exactly as it does today. A cache that can stop a folder opening is worse than no
cache — `immich_list_cache`'s rule, and it applies here.

---

## 5. Which member the collapsed cell shows

**The local rule** — `Stacking::primary`, `Edited, else RAW` by default (the user's call, of three).
So *Show* in Appearance means the same thing wherever a photo came from.

⚠ This works because cache paths carry the real extension (`<uuid>.RAF`, `<uuid>.JPG`), taken from
the asset's `originalFileName`, so `catalog::is_raw` transfers unchanged.

⚠⚠ **A consequence to expect rather than discover:** the 2,379 stacks were given **JPEG** as primary
by the migration script, so an untouched pair shows the **RAW** in Rustroom while Immich's own web UI
shows the JPEG. That is the price of one rule instead of two, and it was chosen knowingly.

⚠ The server's `primaryAssetId` is therefore parsed and **deliberately unused** for display. It is
kept in the DTO because dropping it would make the captured fixture diverge from the reply, which is
the thing this integration keeps getting wrong.

⚠ `Stacking::formats` still applies, so excluding JPEG from stacking excludes it here too.

---

## 6. Where the code goes

| unit | home |
|---|---|
| `Stack`/`StackMember` DTO, `Client::stacks()` | `crates/immich` — zero GUI |
| the projection, its disk cache | `crates/app/src/immich_stack_cache.rs` **(new lib module)** |
| `Job::Stacks`, its outcome | `immich_worker.rs` |
| the key change | `stack.rs` — one function, `key_of` |
| passing the map in | `library_state::browser_entries` |
| dropping it on Reindex | `copies.rs`, beside the existing Immich reindex |

⚠ `stack.rs` stays pure and egui-free; the map is an argument, not a lookup it performs.

---

## 7. Testing

**Without a server:**

1. ⚠ **A captured reply deserialises** into the narrow DTO, and the fixture's doc says it was
   captured and when.
2. The projection: 2 stacks of 2 give 4 asset→stack entries and 2 counts.
3. ⚠⚠ **A stack of one member is dropped** — `Stack::count` forbids a badge reading "1".
4. ⚠ **An asset listed in two stacks resolves deterministically**, so a malformed reply cannot make
   the grid flicker between groupings.
5. `key_of` with the map: an asset in a stack keys on the stack; one outside it keys on its stem, so
   a mixed folder still behaves.
6. ⚠⚠ **Two Immich rows in one stack collapse to one row**, with `count: 2` — end to end through
   `group`, with the sample assertion that the input held two rows.
7. ⚠ **An empty map changes nothing**: the same input yields the same output as today, which is what
   makes this safe for every non-Immich folder.
8. The 75 goldens stay unmoved — nothing here touches rendering.

**Only use can settle:** that an album of 900 assets collapses to ~450 cells, that Reindex picks up a
stack made in Immich, and that the badge and the toolbar behave the same as they do locally.

---

## 8. Out of scope

- **Creating or removing stacks from Rustroom.** `stack.create` and `stack.delete` are grants the
  browse key does not have, and the Settings help text says to keep it read-only.
- **Reconciling the primary** with Immich's own choice — §5 accepts the divergence.
- Any change to local stacking, which is done.

---

## 9. Definition of done

1. An Immich album of paired assets shows roughly half as many cells, each badged `2`.
2. The badge expands and collapses, and **Stack all / Expand all** work, exactly as locally.
3. ⋯ → **Reindex** picks up a stack created in Immich after the cache was written.
4. With the cache absent or the fetch failed, the grid shows both halves and nothing errors.
5. Non-Immich folders are byte-identical in behaviour; goldens unmoved.
6. ⚠ Seen working, not merely green.
