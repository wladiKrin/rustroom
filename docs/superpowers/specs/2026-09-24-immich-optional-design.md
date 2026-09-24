# Immich, completely optional behind one gate

**Date:** 2026-09-24
**Status:** approved in brainstorming, ready for a plan

---

## 1. Why, and what "optional" means

The user's requirement, from 2026-09-24:

> *"the whole immich integration should in the end be completely optional … a toggle in the immich
> header in settings with which one can completely disable the immich support, i.e. in that case there
> should be **no mention of immich at all**."*

⚠⚠ **"No mention at all" is stronger than "the feature does nothing"**, and it is why this is worth
doing before more surfaces exist: a gate retrofitted across code that assumes Immich exists is the
expensive version of this change.

⚠ **The control is a chunky sliding switch, not a tick box** — the user's call after seeing it:
enabling or disabling a whole integration is a decision, not one preference among many. It lives in
`segmented.rs` beside `chip`, whose two hard-won lessons it copies: `mark_changed()`, without which
every `changed()` reader sees nothing, and an explicit `enabled` gate, because a hand-drawn widget gets
no greying from `add_enabled_ui`.

⚠ **The Settings tab is the one exception, and it has to be.** It holds the switch; hiding it would
leave no way back on. So "no mention" means the *working* UI — rail, toolbar, grid, export, Folders.
Settings is where features are turned on and off, so naming one there is not a mention, it is the
control.

**A wart this fixes on the way.** `PanelSection::Immich` is drawn **unconditionally** today, so a user
who has never configured Immich sees an empty rail section called "Immich". That is noise, and the same
predicate removes it.

---

## 2. One predicate, and it already exists

```rust
// crates/app/src/immich_source.rs
pub fn connection(s: &Settings) -> Option<Connection>
```

⚠⚠ **This is the only place the URL and key are read, and nine call sites route through it** — the
worker's fetcher, `refresh_shared`, `immich_panel`, `copies`, `immich_flow`. So the gate is one
condition inside it:

```rust
pub fn connection(s: &Settings) -> Option<Connection> {
    if !s.immich_enabled {
        return None;
    }
    // … unchanged …
}
```

Everything downstream then declines on its own: the worker fetches nothing, `refresh_shared` clears the
shared cell, and `remote_thumbnail` — which already declines without a connection — falls through to
the ordinary local path.

`immich_upload::availability` returns `NotConfigured` when the connection is absent, so the **export
Location ticks are already wired** and need no change at all. That is the payoff of having built the
gate's seed during the upload milestone.

### The setting

```rust
/// Whether the Immich integration exists at all.
pub immich_enabled: bool,
```

⚠⚠ **It defaults to `true`, and that is the `gpu_tail` trap, not laziness.** `Settings` carries a
**container-level** `#[serde(default)]`, so a missing field takes whatever `Default` says. Defaulting to
`false` would make Immich **vanish on upgrade** for anyone already using it. A test pins the default so
a later tidy-up cannot flip it.

⚠ A fresh user loses nothing by the default being `true`: with no URL or key the connection is `None`
anyway, so every surface is already hidden.

---

## 3. What the gate touches

| surface | with Immich off | how |
|---|---|---|
| rail section `PanelSection::Immich` | **not drawn** | the section is skipped when `connection()` is `None` — which also removes the empty section an unconfigured user sees today |
| Settings → Immich tab | **stays**, holding the switch; everything below it hidden | the one deliberate exception |
| Export → Location ticks | already absent | `availability` → `NotConfigured`, no change needed |
| toolbar Reindex / Download / Remove downloads, and their hovers | unreachable | already conditional on `LibrarySource::ImmichAlbum`/`ImmichBucket`, which cannot be entered with no rail section |
| grid's circled ↓ badge | unreachable | same — conditional on the source being Immich |
| `ImmichWorker`, shared thumbnail connection | inert | `connection()` is `None`, so the fetcher has nothing and `refresh_shared` clears the cell |
| Folders → *"immich (downloaded)"* | **stays, renamed** | see §4 |

⚠ **Nothing is deleted.** The cached originals, both thumbnail stores and the list cache stay on disk,
so re-enabling is lossless: edits, marks and thumbnails are all still there. Slice 4's **Remove
downloads** remains the deliberate way to reclaim the space, and putting a destructive action on a
settings toggle was considered and rejected.

---

## 4. The downloaded photos stay usable

The user's call, of three options offered: **the folder stays visible as an ordinary folder.** They are
real local JPEGs with edits and marks, and hiding them would put the user's own work out of reach.

⚠⚠ **This works with no special case because `remote_thumbnail` already declines once the original is
present** — its own doc says so: *"a downloaded photo falls through to the ordinary local path — on-disk
store included — with no special case at all."* So thumbnails, editing, export and marks all keep
working through the local path they already use.

**The name.** ⚠⚠ **REVISED BY THE USER AFTER SEEING IT (2026-09-24): the folder keeps
"immich (downloaded)" whether or not the integration is on**, and its hover keeps the real path.

A neutral **"Downloaded photos"** was built first, together with a `folder_hover` helper that replaced
the path — because the path itself (`~/Library/Caches/rustroom/immich/<server>`) says the word *and*
names the server. Both were then taken back out: *"Can we keep the immich (downloaded) name."*

⚠ **So "no mention of immich at all" now has one stated exception, and it is the user relaxing their
own requirement.** The reasoning is sound: those are their photographs, they know where they came from,
and a name that hides it helps nobody while making the folder harder to recognise. The test asserts the
**section** is absent rather than that the word is, which is the claim the design actually makes.

⚠ Worth recording anyway, because it was a real finding: the earlier draft of this section claimed
folder rows do not hover their path, based on grepping `on_hover_text` — which misses it, because the
rail builds rows through `list_row::RowSpec { hover: &path, .. }`. If the requirement ever tightens
again, that is the second place to change.

---

## 5. What stays out of `connection()`

⚠ **`row_is_browsable` is not changed.** It hides a catalog row whose *file is absent* for Immich rows,
and that is about a missing file, not about the integration. A download the user removed should stay
hidden from catalog-wide views whether or not Immich is switched on.

⚠ **The catalog rows themselves are untouched.** They are ordinary rows for local files; with the
folder visible as an ordinary folder they are exactly right as they are.

---

## 6. Testing

**Required, all without a server:**

1. `immich_enabled` defaults to **`true`**, round-trips, and ⚠ **a settings file written before it
   still loads with Immich on** — the upgrade case, and the whole reason for the default.
2. `connection()` returns `None` when `immich_enabled` is false **even with a valid URL and key**, and
   `Some` when it is true. ⚠ The pair is what makes it discriminating.
3. `availability()` is `NotConfigured` when disabled, so the export ticks vanish — proving the one
   predicate reaches the surface built before it.
4. `folder_display_name` returns `"immich (downloaded)"` for the cache root **regardless of the
   toggle** — the user's revision, and the test says so rather than implying a rename that no longer
   happens.
5. ⚠⚠ **The rail section is absent when disabled and present when configured** — both halves, because
   an absence alone passes against a panel that drew nothing.
6. ⚠ **The rail offers no Immich *section* when disabled, and nothing else mentions it** — asserted
   over the labels the headless harness reports, with the downloaded folder's row as the one allowed
   name. The requirement as a test rather than a list of places to remember.
7. ⚠⚠ **The switch flips on a click and reports `changed()`** — both of which `chip` shipped wrong
   once, and a hand-drawn widget that does not `mark_changed()` reads as "not clickable". Plus a
   disabled switch that neither flips nor reports, since `add_enabled_ui` cannot gate a hand-drawn
   control.
7. The 75 goldens stay unmoved — nothing here touches rendering.

**Only use can settle:**

- That switching it off really leaves nothing on screen, and switching it back on restores the albums,
  the Timeline and the downloaded folder's name.
- That the downloaded photos still open, edit and export with the integration off.

---

## 7. Out of scope

- Deleting or evicting any cache on disable.
- The OS keychain; the keys stay plaintext.
- ⬜ Rustroom-side stacking awareness — its own backlog item.
- ⛔ Two-way sync — declined with reasons in `docs/BACKLOG.md`.

---

## 8. Definition of done

1. One predicate, `connection()`, gates every surface; no surface grows its own condition.
2. With the toggle off, no visible string says "immich" outside the Settings tab.
3. The downloaded photos remain visible, editable and exportable, under a neutral name.
4. Switching back on restores everything, with no data lost.
5. An existing settings file loads with Immich **on**.
6. ⚠ Seen working, not merely green.
