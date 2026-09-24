# Location absorbs Delivery, and a failed upload says so

**Date:** 2026-09-24
**Status:** approved in brainstorming, ready for a plan
**Refines:** `2026-09-24-export-delivery-design.md`, which added Delivery as its **own** section

---

## 1. Why

The three delivery modes work, and the shape was wrong. The user, after using it:

> *"I'm unhappy with the name and it being a separate thing from location, I think we should reconcile
> them."*

**"Where does this go" is one question, so it belongs in one section.** A separate *Delivery* section
asked half of it at the bottom of the dialog while *Location* asked the other half at the top. And two
further gaps use exposed:

- With one photo selected, nothing said **which date the photo would actually get**, so there was no
  way to know where to look for it in Immich.
- A failed upload landed the file in the export folder correctly — and said so only in a report line.
  *"That's pretty important to know."*

⚠ **Location is already the first section**, so folding Delivery into it *is* the requested "move it to
the top". No reordering is needed, and none should be invented.

---

## 2. Location, reconciled

One section, first in the dialog, with two ticks — the local one carrying the path, the Immich one
carrying the date:

```
LOCATION                                     exports + Immich
  ☑ Local      [ ~/Pictures/exports ]  [Choose…]
  ☑ Immich     Date [ Original date ▾ ]   → 19 Nov 2025, 13:32
```

**"Delivery" disappears** as a section and as a word. `export_summary::delivery` folds into
`export_summary::location`.

⚠⚠ **When Immich is not configured, Location draws exactly what it drew before this whole feature
existed: the folder row, and no ticks.** Two reasons, and both are load-bearing. A lone "Local" tick
with nowhere else to go is a control that cannot mean anything — this project's rule is that an
unusable control goes *dark*, but a control with no alternative should not exist at all. And the
requirement that switching Immich off leaves *"no mention of immich at all"* is easiest to keep true
when the section has a single unconditional shape to fall back to.

⚠ The Immich tick is also drawn, and **disabled with a hover**, when Immich is configured but no upload
key is set — that is a real choice the user can make available, unlike the case above.

---

## 3. The model gains an empty state

```rust
pub enum Destination {
    #[default]
    Files,
    Target(UploadTarget),
    Both(UploadTarget),
    /// Neither tick set.
    Nowhere,
}
```

⚠ **Appended, never inserted**, and **not** the default.

Both ticks can be cleared. `can_export` then refuses `Nowhere`, Export goes dark, and **the footer note
says why** — the same way the dialog already explains an unparsable filename template. That was chosen
over refusing the click or auto-ticking the other box: a checkbox that silently ignores a click, or one
that changes a control the user did not touch, is worse than a dark button with a sentence beside it.

⚠ `keeps_files()` is **false** for `Nowhere` and `target()` is `None`, so every existing caller treats
it as "write nothing, upload nothing" — which is exactly right, and the only thing standing between it
and a pointless run is `can_export`.

---

## 4. The date a single photo will get

Beside the dropdown, **only when exactly one photo is selected**, the resolved moment:

⚠⚠ **Computed by calling the same `immich_upload::upload_seconds` the upload itself calls.** A second
formatting of "what the date will be" is a second implementation that can disagree with the first, and
the whole point of showing it is that the user can trust it.

⚠ Shown for one photo only. For a selection each photo has its own capture and edit date, so a single
number would be a lie — and the one case where it matters ("where do I look for this in Immich") is the
single-photo case.

⚠ It needs `sidecar::edited_at` for that photo, read **before** the dialog closure, which already
borrows `app.settings` mutably — the borrow shape this file's own comment says has cost a compile error
twice.

---

## 5. The pop-up when an upload does not confirm

A new event:

```rust
ExportEvent::UploadUnconfirmed { at: PathBuf, why: String },
```

sent for the **first** unconfirmed upload of a run, and opening `Modal::UploadUnconfirmed { at, why }`
— a one-button notice saying the upload did not confirm, **where the file is now**, and that it was
**not** deleted.

⚠⚠ **"First only" is an `AtomicBool::swap(true)`, not a plain flag.** The export pool is `width`-wide,
so several photos can fail in the same instant; `swap` makes exactly one of them win. A `load`-then-
`store` would let two modals race.

⚠ It fires **when the failure happens**, not at the end of the run, because the point is to be told
early — and the export **keeps going**, because a network that dropped one upload has not invalidated
the rest.

⚠ There is no other modal it can collide with: every collision is resolved up front in `plan`, before
anything is rendered or uploaded, so no dialog is open while photos are being uploaded.

⚠ The report's `unconfirmed` list stays exactly as it is. The pop-up is an **addition** for the first
one, not a replacement — a run with nine failures still lists nine.

---

## 6. Where the code goes

| unit | home |
|---|---|
| `Destination::Nowhere`, `can_export` | `batch_export/options.rs` |
| the footer's reason for a dark Export | `export_summary.rs` |
| `ExportEvent::UploadUnconfirmed`, the `swap` guard | `batch_export/worker.rs` |
| `Modal::UploadUnconfirmed` | `modals/mod.rs` |
| the notice itself | `modals/export.rs` (egui only) |
| the two ticks, the dropdown, the resolved date | `modals/export.rs` |
| the event → modal wiring | `export_flow.rs` |

⚠ `export_summary::delivery` is **deleted**, not left unused — its text moves into `location`.

---

## 7. Testing

**Without a server, and required:**

1. `Destination::Nowhere` — `keeps_files()` false, `target()` `None`, `can_export` **false**, and it
   round-trips. ⚠ Plus: the default is still `Files`, so an existing user is untouched.
2. The footer note names the reason under `Nowhere`, and is unchanged otherwise.
3. `export_summary::location` for all four states: folder only, Immich only, both, and neither.
4. ⚠⚠ **The resolved date shown equals `upload_seconds` for the same inputs** — the guard against the
   preview and the upload drifting apart.
5. ⚠ **Only the first failure sends the event**: a two-photo run against a closed port produces
   **one** `UploadUnconfirmed` and **two** `unconfirmed` report entries. That is the whole "just once"
   requirement, and the pair is what makes it discriminating.
6. ⚠ With Immich unconfigured, the Location section draws **no ticks** and **no** mention of Immich,
   while still drawing the folder row.
7. `Files` alone is unchanged and the **75 goldens stay unmoved**.

**Only use can settle:**

- That the pop-up actually appears, reads well, and names a path the user can find.
- That the resolved date matches what Immich then shows for the photo.

---

## 8. Out of scope

- Any change to the three modes' behaviour — staging, delete-on-confirmation and rescue are as built.
- Albums, stacking an upload with its original, `visibility`, `sidecarData`.
- A library-wide "upload everything edited" sweep.
- ⛔ Two-way sync — declined with reasons in `docs/BACKLOG.md`.

---

## 9. Definition of done

1. One **Location** section, first in the dialog, with the two ticks; no "Delivery" anywhere.
2. Unticking both darkens Export and the footer says why.
3. A single selected photo shows the date it will be filed under, and it matches Immich afterwards.
4. A failed upload raises **one** pop-up naming where the file is.
5. With Immich unconfigured the dialog is exactly what it was before any of this, goldens unmoved.
6. ⚠ Seen working, not merely green.
