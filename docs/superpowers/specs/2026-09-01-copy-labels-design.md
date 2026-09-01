# M6.0 — copy labels

*Design, 2026-09-01. Supersedes the naming half of the virtual-copies work (M3.4) and its
2026-08-26 follow-up.*

A virtual copy stops carrying a **name that is also its export filename**, and carries a **label**
instead: short human text, no extension, never a path. The filename it shows is the file's own.
`PhotoId::export_stem` is deleted; the export filename is composed by the M6.1a template.

---

## Why this changes now — the premise was removed, not mis-chosen

The current design is internally consistent and its reasoning is recorded in `CLAUDE.md`:

> **A COPY'S NAME *IS* ITS EXPORT NAME — it is not appended to the file's.** Naming copy 2 "Warm"
> gives `Warm.jpg` […] **This is also why the field shows a whole filename** (`DSCF6223_1.RAF`)
> rather than a bare label: what you see is what you get.

Every clause of that follows from one premise: **the name determines the export filename.** M6.1a
made a template determine it instead, so the premise is gone and the design now has a field that
looks like a filename and no longer decides one.

⚠ This is the **third** shape copy naming has taken — append, then replace, now label. That is not
evidence of thrash: "replace" was correct while a copy's name *was* its export name, and stopped
being correct the moment templating shipped. Record that, so the next reader does not "restore" a
simpler-looking earlier design.

### Four problems that follow from the lost premise

1. **A name does two jobs** — UI label and export filename. Fine while they were the same string.
2. **A named copy loses its parent.** `Warm` in the Library grid says nothing about which
   photograph it came from. A usability defect *today*, unrelated to export.
3. **The field carries an extension**, so `export_stem` strips `.RAF` and `safe_name::sanitize`
   runs across something the user believes is a label.
4. **Copy 0 cannot be labelled** — `export_stem` returns early for it. You may label copy 1 "B&W"
   and may not label the original "Colour".

### ⚠⚠ The evidence, from real sidecars rather than from argument

Three named copies existed across the author's two folders on 2026-09-01:

| sidecar | copy | stored name |
| --- | --- | --- |
| `DSCF6237.RAF.json` | 1 | `"DSC"` |
| `DSCF6712.RAF.json` | 1 | `"DSCF6712_1.RAF"` |
| `DSCF6712.RAF.json` | 2 | `"DSCF6712_2.RAF"` |

**Only the first is a label anyone chose.** The other two are the *derived defaults*, frozen into
storage — the field displayed `DSCF6712_1.RAF` and committing it wrote that string back.
`CLAUDE.md` documents a rule meant to prevent exactly this (*"typing the default back into the
field also stores `None` […] freezing the derived string would silently strand the copy on the old
stem"*), so either the rule has a hole or these predate it. Either reading gives the same finding:
**a field that looks like a filename invites you to freeze a name you never chose.**

⚠ Those three copies were **deleted by the user** on 2026-09-01 (backup at
`~/Desktop/rustroom-sidecar-backup-20260901-131723/`), so this change needs no migration on the
author's data. The read rule in §5 exists for the backup, and for anyone else's catalogue.

---

## 1. Model

```rust
pub struct CopyEntry {
    /// `0` is the original; copies are numbered from 1 and never reused.
    pub copy: u32,
    /// A short human label for this version — "B&W", "Client crop".
    ///
    /// ⚠⚠ **NOT a filename.** No extension, never sanitized, never reaches a path. `None`
    /// renders as the copy number. Compare the field this replaces, which *was* the export
    /// filename and therefore had to carry an extension and be path-safe.
    #[serde(alias = "name")]
    pub label: Option<String>,
    #[serde(default)]
    pub edits: Edits,
    #[serde(default)]
    pub snapshots: Vec<Snapshot>,
}
```

- **Identity is unchanged: `PhotoId { path, copy }`.** Labels are mutable and may collide, so they
  key nothing — not `BufferCache`, not `HistoryStash`, not snapshots, not the sidecar. ⚠ This is
  the single most important thing not to "improve": presentation must not enter identity.
- **Copy 0 may be labelled**, like any other version. The special case bought nothing once the
  label stopped being the filename, and it cost a comment at every read site.
- **`#[serde(alias = "name")]`** is what makes every existing sidecar load unchanged. ⚠ The
  container-level `#[serde(default)]` trap recorded for `Sidecar` does **not** apply here — this is
  a field alias on an existing required-ish field, not a new defaulted container.

---

## 2. Display

- **Caption is the file stem**, on every row, uniform width.
- **Badge carries the label, or the copy number when the label is unset.** An unlabelled copy 0
  shows **no badge**, exactly as today.

`entries::shown_name` splits into **`entries::caption`** (the filename) and **`entries::label_of`**
(label-or-number). One function currently does both jobs; the split is the point.

⚠ Rejected: appending the label to the caption (`DSCF1234 · B&W`). It is more scannable and it
makes captions ragged, truncating long labels against a cell width the user can resize. The badge
already exists to mark copies and is the right home.

---

## 3. Editing — "edit what you see", plus one discoverable home

| Affordance | Edits |
| --- | --- |
| Caption click (Library grid) | the **file** — `std::fs::rename`, every copy follows, sidecar moves with it |
| Badge click | that copy's **label** |
| Develop panel name field | that copy's **label** (rebound) |

⚠ `rename_draft` currently serves both jobs and its meaning depends on whether the row is a copy.
It **splits into `rename_draft` and `label_draft`**. That dual role is the defect underneath every
problem in this document.

⚠ The panel field exists because a badge is small and clicking it to edit is not discoverable on
its own. Two entry points for one string is acceptable *because they are the same string* — unlike
the Presets panel, which was removed for being two homes for one **feature**.

⚠ `name_edit::commit_gesture` is reused unchanged for both fields — the extraction that already
stopped the copy-rename and collections fields drifting.

---

## 4. Export

- **`PhotoId::export_stem` is deleted.** It exists only to answer "what is this copy's filename",
  which the template now answers.
- `{name}` → the **file stem**.
- `{copy}` resolves in this order, and the ordering is the whole rule:
  1. the **label**, if one is set — on any copy, **including copy 0**;
  2. otherwise the **copy number**, when it is non-zero;
  3. otherwise **absent** — an unlabelled copy 0 — so the adjacent separator collapses per
     M6.1a's rule and `{name}_{copy}` renders as plain `DSCF1234`.

  ⚠ Step 1 comes before step 2 for every copy, so a **labelled copy 0** contributes its label and
  `{name}_{copy}` gives `DSCF1234_Colour`. That is the case the old `export_stem` could not express
  at all, since it returned early for copy 0.
- `DEFAULT_FILENAME_TEMPLATE` becomes **`{name}_{copy}`**.

⚠⚠ **The old rule survives as a template.** *"A copy's name is its export name"* is exactly the
template **`{copy}`**. It stops being hardcoded and becomes a choice — which is what adding
templating was for, and why this is a generalisation of the earlier decision rather than a reversal
of it.

---

## 5. Reading old sidecars — a read rule, not a migration

No version bump, no backfill pass; the value normalises on the next write.

1. A stored value **equal to the old derived default** — `{stem}_{copy}`, with or without a
   trailing extension — reads as **`None`**.
2. Anything else has a **trailing extension stripped** and becomes the label.

⚠ Rule 1 undoes the accidental freezing the evidence above shows. It is safe in the case that looks
risky: if someone *deliberately* typed the default, rule 1 is indistinguishable from an accident —
and the rendered outcome is identical (the badge shows the number either way), so nothing is lost.

⚠ Rejected: stripping a leading `{stem}_` prefix by pattern. Same visible outcome, by a rule that
can be wrong — it would mangle a genuine label beginning with the stem.

---

## 6. Decided without asking, cheap to reverse

- **The text filter matches caption *and* label.** Searching "B&W" should find it.
- **Labels need not be unique.** Two copies may share one; `-N` de-duplication keeps their exports
  apart, which is what it already exists for.
- **A label does not travel with a paste, preset or sync.** Envelope metadata, never an `Edits`
  field — the rule the copy name already follows, and for its reason: in `Edits` it would need an
  edit group, would travel with a B&W recipe onto another photo, and would be wiped by "Reset
  adjustments".

---

## 7. Testing

Beyond the ordinary unit coverage, four properties earn named tests:

1. **A label never reaches a path.** The strongest single guard — assert an export of a copy
   labelled `../../evil` or `B&W` produces a sanitized *filename* while the **label keeps what was
   typed**. Today's rule sanitizes the name itself; that must stop.
2. **The read rule, both arms.** `"DSCF6712_1.RAF"` on copy 1 of `DSCF6712` → `None`; `"DSC"` →
   `Some("DSC")`. ⚠ Use the **backup sidecars** as the fixture shape — they are the real thing.
3. **The default template reproduces the old naming for unlabelled copies, byte-identically.**
   `{name}_{copy}` on copy 2 of `DSCF1234` → `DSCF1234_2`; on copy 0 → `DSCF1234`.
4. **`{copy}` alone reproduces the old *named*-copy export** — a copy labelled `Warm` → `Warm`.
   This is what proves the behaviour was generalised rather than dropped.

⚠ Sabotage each: this project has recorded **sixteen** instances of a test that could not detect
the bug it named. In particular, a fixture with an **unlabelled** copy cannot distinguish
"label-or-number" from "number always" — every label test needs a labelled *and* an unlabelled case.

---

## 8. Accepted caveats

1. **A labelled copy's export filename changes** — `DSCF1234_B&W.jpg` rather than `B&W.jpg`, unless
   the template is `{copy}`. Affects **zero files** in the author's catalogue, since the only
   labelled copy was deleted; it would affect anyone restoring the backup.
2. **The rename field on a copy stops showing a filename.** Visible, and the point.
3. **Badge-click-to-edit is not self-advertising.** The panel field is the mitigation.
4. **Labels are not unique**, so two copies can look alike in the grid.
5. **Stale catalog rows.** `photos` rows for the deleted copies remain; they cannot appear in the
   Library (entries are built from sidecars) but any marks on them linger. Not swept by this change.
6. **`copy_hwm` is untouched**, so a future copy of `DSCF6237` becomes 2 rather than re-taking 1 —
   the documented "numbers are never reused" rule.

---

## 9. Files affected

`sidecar.rs` (the field and the read rule) · `entries.rs` (`caption` / `label_of`, replacing
`shown_name`) · `photo_id.rs` (`export_stem` deleted) · `filename_template.rs` (`{name}`, `{copy}`)
· `batch_export.rs` (default template) · `library.rs` (caption, badge, badge editing) ·
`develop/panel/` (the field's binding) · `name_edit.rs` (two drafts) · `library_filter.rs` and
`rule.rs` (text matching) · `main.rs` (`rename_draft` / `label_draft`).

⚠ `batch_export.rs` is **1465 lines** and `develop/panel/geometry.rs` **742** — both already over
the ~600 threshold before this change. Splitting them is not in this scope, and should happen
between waves rather than inside one.
