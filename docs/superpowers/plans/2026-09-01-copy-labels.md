# M6.0 Copy Labels Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A virtual copy carries a short human **label** instead of a name that doubles as its export filename; the filename it displays is the file's own, and the export filename is composed by the M6.1a template.

**Architecture:** `CopyEntry.name` becomes `CopyEntry.label` with a `serde` alias and a read rule that discards frozen derived defaults. `entries::shown_name` splits into `caption` (the filename) and `label_of` (label-or-number). `PhotoId::export_stem` is deleted and its job passes to the `{name}` / `{copy}` template tokens. The Library caption shows the filename on every row and the existing copy badge carries the label.

**Tech Stack:** Rust, serde, egui 0.35. Spec: `docs/superpowers/specs/2026-09-01-copy-labels-design.md`.

**Build environment — nothing compiles without this:**
```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

**⚠ Verification discipline for every task.** This shell is **zsh**, where `${PIPESTATUS[0]}` expands to nothing and a check printing `EXIT: ` reads as a pass. Always redirect and test `$?`:
```bash
cargo fmt --all --check > /tmp/f.log 2>&1; echo "FMT=$?"
cargo clippy --all-targets --all-features -- -D warnings > /tmp/c.log 2>&1; echo "CLIPPY=$?"
cargo test --all > /tmp/t.log 2>&1; echo "TEST=$?"
awk '/^test result:/{p+=$4; f+=$6} END{print "passed="p" failed="f}' /tmp/t.log
```
Baseline before Task 1 is **1415 passed, 0 failed**. No golden may move in any task — `git status crates/pipeline/` must stay empty throughout.

**⚠ Sabotage discipline.** After writing a test, break the thing it names and watch it fail. This project has recorded **sixteen** tests that could not detect the bug they were named for. A sabotage that fails to *compile* looks exactly like one that passes — confirm a `test result:` line appears before believing anything.

**Conventions:** no `Co-Authored-By` trailer; **never push**; commit at the end of each task.

---

## File Structure

| File | Change |
| --- | --- |
| `crates/app/src/sidecar.rs` | `CopyEntry.name` → `label` + alias; `normalise_label`; `set_name` → `set_label` (drops the copy-0 refusal) |
| `crates/app/src/entries.rs` | `PhotoEntry.name` → `label`; `shown_name` → `caption` + `label_of` |
| `crates/app/src/photo_id.rs` | `export_stem` **deleted** |
| `crates/app/src/filename_template.rs` | `{name}` = file stem; `{copy}` = label-or-number |
| `crates/app/src/batch_export.rs` | `DEFAULT_FILENAME_TEMPLATE` = `{name}_{copy}` |
| `crates/app/src/thumb.rs` | badge takes label text, not a number |
| `crates/app/src/library.rs` | caption is the filename; badge click opens the label draft |
| `crates/app/src/name_edit.rs` | `field` gains a mode; `label_draft` |
| `crates/app/src/copies.rs` | `set_copy_name` → `set_label`, allows copy 0 |
| `crates/app/src/main.rs` | `label_draft` beside `rename_draft` |
| `crates/app/src/library_filter.rs`, `rule.rs` | text matches caption **and** label |

---

## Task 1: `CopyEntry.label` and the read rule

**Files:**
- Modify: `crates/app/src/sidecar.rs`
- Modify (mechanical `.name` → `.label`): `crates/app/src/copies.rs`, `crates/app/src/entries.rs`, `crates/app/src/snapshots.rs`, `crates/app/src/session_flow.rs`

- [ ] **Step 1: Write the failing tests**

Add to `sidecar.rs`'s `mod tests`:

```rust
/// ⚠⚠ The two arms of the read rule, and the first is the one that matters. Real sidecars in
/// the author's folders stored `"DSCF6712_1.RAF"` — the *derived default*, frozen because the
/// field displayed a whole filename and committing it wrote that string back. A label nobody
/// chose must not survive as one.
#[test]
fn a_stored_derived_default_reads_as_unlabelled() {
    assert_eq!(normalise_label("DSCF6712.RAF", 1, Some("DSCF6712_1.RAF")), None);
    // The same string without its extension is equally derived.
    assert_eq!(normalise_label("DSCF6712.RAF", 1, Some("DSCF6712_1")), None);
}

#[test]
fn a_real_label_survives_with_any_extension_stripped() {
    assert_eq!(normalise_label("DSCF6237.RAF", 1, Some("DSC")), Some("DSC".into()));
    assert_eq!(normalise_label("DSCF6237.RAF", 1, Some("Warm.RAF")), Some("Warm".into()));
}

/// ⚠ A label test needs a labelled *and* an unlabelled case: a fixture with only one cannot
/// tell "label-or-number" from "number always".
#[test]
fn an_absent_label_stays_absent() {
    assert_eq!(normalise_label("DSCF6237.RAF", 1, None), None);
    assert_eq!(normalise_label("DSCF6237.RAF", 0, None), None);
}

/// ⚠ Copy 0 may be labelled now, so the derived-default rule must not fire on it — there is no
/// derived default for the original, whose name is simply the file's.
#[test]
fn copy_zero_keeps_a_label() {
    assert_eq!(normalise_label("DSCF6237.RAF", 0, Some("Colour")), Some("Colour".into()));
}

/// The `serde` alias is what lets every sidecar written before this change load unchanged.
#[test]
fn a_sidecar_written_with_name_still_loads() {
    let json = r#"{"copies":[{"copy":1,"name":"DSC","edits":{}}],"copy_hwm":1}"#;
    let s: Sidecar = serde_json::from_str(json).expect("must parse");
    assert_eq!(s.copies[0].label.as_deref(), Some("DSC"));
}
```

- [ ] **Step 2: Run them to verify they fail**

```bash
cargo test -p app --lib sidecar:: > /tmp/t1.log 2>&1; echo "EXIT=$?"; grep -c "^error" /tmp/t1.log
```
Expected: compile errors — `normalise_label` not found, and `label` is not a field of `CopyEntry`.

- [ ] **Step 3: Rename the field and add the alias**

In `crates/app/src/sidecar.rs`, replace the `name` field of `CopyEntry` with:

```rust
    /// A short human label for this version — "B&W", "Client crop".
    ///
    /// ⚠⚠ **NOT a filename.** No extension, never sanitized, never reaches a path. `None`
    /// renders as the copy number. This field *was* the export filename, which is why it used
    /// to carry an extension and be path-safe; M6.1a's template took that job, so the premise
    /// is gone. See `docs/superpowers/specs/2026-09-01-copy-labels-design.md`.
    ///
    /// ⚠⚠ **Envelope metadata, never an `Edits` field, and that is load-bearing.** In `Edits` it
    /// would need an edit group and would then **travel with a paste, a preset or a sync** —
    /// pasting a B&W recipe onto another photo would rename it "B&W" — and "Reset adjustments"
    /// would wipe it. The cost is that `History<Edits>` cannot undo it, which is why relabelling
    /// gets its own `ActionKind`.
    ///
    /// ⚠ `alias = "name"` is what makes every sidecar written before 2026-09-01 load unchanged.
    #[serde(default, alias = "name")]
    pub label: Option<String>,
```

- [ ] **Step 4: Add the read rule**

Add to `crates/app/src/sidecar.rs`, above `load_all`:

```rust
/// A stored label, with the values nobody chose removed.
///
/// ⚠⚠ Rule 1 exists because of real data: of three named copies in the author's folders, two
/// stored the *derived default* (`"DSCF6712_1.RAF"`), frozen because the old field displayed a
/// whole filename and committing it wrote that string back. Those are not labels.
///
/// ⚠ It is safe in the case that looks risky — if someone typed the default deliberately, this
/// is indistinguishable from the accident, and the rendered outcome is identical because the
/// badge shows the number either way.
///
/// ⚠ Deliberately **not** "strip a leading `{stem}_` prefix": same visible outcome by a rule
/// that can be wrong, since it would mangle a genuine label beginning with the stem.
pub fn normalise_label(file_name: &str, copy: u32, stored: Option<&str>) -> Option<String> {
    let stored = stored?;
    let bare = strip_extension(stored);
    if copy != 0 {
        let derived = crate::file_rename::default_copy_name(file_name, copy);
        if stored == derived || bare == strip_extension(&derived) {
            return None;
        }
    }
    if bare.is_empty() {
        return None;
    }
    Some(bare.to_string())
}

fn strip_extension(s: &str) -> &str {
    match s.rsplit_once('.') {
        Some((stem, _)) if !stem.is_empty() => stem,
        _ => s,
    }
}
```

- [ ] **Step 5: Apply the rule at load, and rename `set_name`**

In `load_all`, after the envelope parses, normalise every copy. Replace the `if value.get("copies").is_some()` arm's body with:

```rust
        let mut s: Sidecar = serde_json::from_value(value).unwrap_or_default();
        // ⚠ Applied on read rather than as a migration pass: no version bump, no backfill, and
        // the value normalises on the next write.
        let file_name = photo
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        for c in &mut s.copies {
            c.label = normalise_label(&file_name, c.copy, c.label.as_deref());
        }
        s
```

In the legacy arm and in `set_name`'s `CopyEntry` literal, replace `name: None` / `name,` with `label: None` / `label,`. Rename `set_name` to `set_label` and its parameter `name` to `label`.

- [ ] **Step 6: Fix the mechanical fallout**

```bash
cargo build -p app 2>&1 | grep -E "^error" | head -30
```
Every remaining error is `.name` on a `CopyEntry` or a `set_name` call. Rename each to `.label` / `set_label`. **Do not** change `PhotoEntry.name` yet — that is Task 2.

- [ ] **Step 7: Run the tests**

```bash
cargo test --all > /tmp/t.log 2>&1; echo "TEST=$?"
awk '/^test result:/{p+=$4; f+=$6} END{print "passed="p" failed="f}' /tmp/t.log
```
Expected: PASS, count **1420** (1415 + 5).

- [ ] **Step 8: Sabotage**

Delete the `if copy != 0 { ... }` block from `normalise_label`, rebuild, run. Expected: `a_stored_derived_default_reads_as_unlabelled` **fails**. Confirm a `test result:` line appears — a sabotage that fails to compile is not evidence. Restore.

- [ ] **Step 9: Commit**

```bash
git add -A crates/app/src
git commit -m "feat(sidecar): a copy carries a label, not a filename

The field was the export filename, which is why it carried an extension
and had to be path-safe. M6.1a's template took that job, so the premise
is gone. A serde alias keeps every existing sidecar loading, and a read
rule discards the derived defaults that real sidecars had frozen."
```

---

## Task 2: `entries::caption` and `label_of`

**Files:**
- Modify: `crates/app/src/entries.rs`

- [ ] **Step 1: Write the failing tests**

```rust
/// ⚠⚠ The caption is the **file's** name on every row, including a copy's. A copy used to show
/// its own name instead, which meant a labelled copy gave no sign of which photograph it came
/// from — a defect independent of export.
#[test]
fn the_caption_is_the_file_name_on_every_copy() {
    assert_eq!(caption("DSCF1234.RAF", 0), "DSCF1234.RAF");
    assert_eq!(caption("DSCF1234.RAF", 2), "DSCF1234.RAF");
}

#[test]
fn the_label_falls_back_to_the_copy_number() {
    assert_eq!(label_of(2, None), Some("2".to_string()));
    assert_eq!(label_of(2, Some("B&W")), Some("B&W".to_string()));
}

/// ⚠ An unlabelled original has nothing to badge; a labelled one does. Both halves are needed —
/// a fixture with only the first cannot tell "never badge copy 0" from "badge its label".
#[test]
fn the_original_badges_only_when_it_has_a_label() {
    assert_eq!(label_of(0, None), None);
    assert_eq!(label_of(0, Some("Colour")), Some("Colour".to_string()));
}
```

- [ ] **Step 2: Run to verify failure**

```bash
cargo test -p app --lib entries:: > /tmp/t2.log 2>&1; echo "EXIT=$?"
```
Expected: FAIL — `caption` and `label_of` not found.

- [ ] **Step 3: Replace `shown_name` with the two functions**

In `crates/app/src/entries.rs`, delete `shown_name` and add:

```rust
/// What the Library grid, the filmstrip and the Develop footer print under a photo.
///
/// ⚠⚠ **The file's own name, on every row including a copy's.** One rule, no special case —
/// which is what lets a copy show which photograph it belongs to. A copy's own text lives in
/// [`label_of`] and is drawn in the badge.
pub fn caption(file_name: &str, _copy: u32) -> String {
    file_name.to_string()
}

/// What the copy badge carries: the label, else the copy number, else nothing.
///
/// ⚠ The ordering is the whole rule, and step 1 applies to **copy 0 too** — a labelled original
/// badges "Colour". Only an *unlabelled* original has nothing to say.
pub fn label_of(copy: u32, label: Option<&str>) -> Option<String> {
    match (copy, label) {
        (_, Some(l)) if !l.is_empty() => Some(l.to_string()),
        (0, _) => None,
        (c, _) => Some(c.to_string()),
    }
}
```

- [ ] **Step 4: Rename `PhotoEntry.name` and its method**

Rename the field `name` → `label`, and replace `PhotoEntry::label()` with:

```rust
    /// The caption under this row — see [`caption`].
    pub fn caption(&self) -> String {
        caption(&self.file_name, self.id.copy)
    }

    /// This row's badge text, if it has one — see [`label_of`].
    pub fn badge(&self) -> Option<String> {
        label_of(self.id.copy, self.label.as_deref())
    }
```

- [ ] **Step 5: Fix the fallout**

```bash
cargo build -p app 2>&1 | grep -E "^error" | head -40
```
Update each site: `entries::shown_name(&e.file_name, e.id.copy, e.name.as_deref())` becomes `e.caption()` where a caption is wanted. **`library_filter.rs` and `rule.rs` are Task 6** — for now make them call `e.caption()` so the build passes; their behaviour changes there.

- [ ] **Step 6: Run the tests**

Expected: PASS, count **1423**.

- [ ] **Step 7: Sabotage**

Change `label_of`'s first arm to `(0, _) => None` placed *first*, rebuild, run. Expected: `the_original_badges_only_when_it_has_a_label` **fails** on the labelled case. Restore.

- [ ] **Step 8: Commit**

```bash
git add -A crates/app/src
git commit -m "refactor(entries): the caption is the filename, the badge is the label

shown_name did two jobs. Splitting them is what lets a copy show which
photograph it came from, which it could not do while its own name
replaced the caption."
```

---

## Task 3: delete `export_stem`, retarget the tokens

**Files:**
- Modify: `crates/app/src/photo_id.rs` (delete `export_stem`), `crates/app/src/filename_template.rs`, `crates/app/src/batch_export.rs`

- [ ] **Step 1: Write the failing tests**

In `filename_template.rs`'s `mod tests`:

```rust
/// ⚠⚠ The default template must reproduce the old naming for **unlabelled** copies, byte for
/// byte — that is what makes this change invisible to every existing export.
#[test]
fn the_default_template_reproduces_the_old_naming_for_unlabelled_copies() {
    let f = NameFacts { file_stem: "DSCF1234", copy: 0, copy_label: None, ..facts() };
    assert_eq!(render(DEFAULT, &f), "DSCF1234");
    let f2 = NameFacts { copy: 2, ..f };
    assert_eq!(render(DEFAULT, &f2), "DSCF1234_2");
}

/// ⚠⚠ This is what proves the old rule was **generalised, not dropped**: "a copy's name is its
/// export name" is now expressible as the template `{copy}`.
#[test]
fn the_copy_token_alone_reproduces_the_old_named_copy_export() {
    let f = NameFacts { file_stem: "DSCF1234", copy: 2, copy_label: Some("Warm"), ..facts() };
    assert_eq!(render("{copy}", &f), "Warm");
}

#[test]
fn name_is_the_bare_stem_and_carries_no_copy_identity() {
    let f = NameFacts { file_stem: "DSCF1234", copy: 2, copy_label: Some("Warm"), ..facts() };
    assert_eq!(render("{name}", &f), "DSCF1234");
    assert_eq!(render("{name}_{copy}", &f), "DSCF1234_Warm");
}

/// ⚠ A labelled copy 0 contributes its label — the case `export_stem` could not express at all,
/// because it returned early for the original.
#[test]
fn a_labelled_original_contributes_its_label() {
    let f = NameFacts { file_stem: "DSCF1234", copy: 0, copy_label: Some("Colour"), ..facts() };
    assert_eq!(render("{name}_{copy}", &f), "DSCF1234_Colour");
}

/// ⚠⚠ The label keeps what was typed; only the **filename** is sanitized. Today the name itself
/// is sanitized, and that inversion is easy to implement backwards — it would look correct until
/// someone typed a slash.
#[test]
fn a_label_never_escapes_into_the_path() {
    let f = NameFacts { file_stem: "DSCF1234", copy: 1, copy_label: Some("../../evil"), ..facts() };
    let out = render("{name}_{copy}", &f);
    assert!(!out.contains('/'), "a label reached the path: {out}");
    assert!(!out.contains(".."), "a label reached the path: {out}");
}
```

- [ ] **Step 2: Run to verify failure**

```bash
cargo test -p app --lib filename_template > /tmp/t3.log 2>&1; echo "EXIT=$?"
```
Expected: FAIL — `copy_label` is not a field of `NameFacts`.

- [ ] **Step 3: Retarget the tokens**

In `filename_template.rs`, rename `NameFacts.copy_name` → `copy_label`, and replace the `Token::Name` and `Token::Copy` arms of `resolve` with:

```rust
        // ⚠⚠ The **bare file stem**, carrying no copy identity, so `{name}` and `{copy}` are
        // orthogonal and `{name}_{copy}` reads correctly instead of doubling. ⚠ A template of
        // bare `{name}` therefore gives every copy of a file the same base, and only `-N`
        // de-duplication keeps their exports apart — which is why `DEFAULT_FILENAME_TEMPLATE`
        // is `{name}_{copy}`.
        Token::Name => Some(facts.file_stem.to_string()),
        // ⚠ The ordering is the whole rule, and step 1 applies to copy 0 too: a labelled
        // original contributes "Colour". Only an unlabelled original is absent, and its
        // separator then collapses.
        Token::Copy => match (facts.copy, facts.copy_label) {
            (_, Some(l)) if !l.is_empty() => Some(l.to_string()),
            (0, _) => None,
            (c, _) => Some(c.to_string()),
        },
```

Delete the `PhotoId::export_stem` function from `photo_id.rs` entirely, plus its tests.

- [ ] **Step 4: Change the default template**

In `batch_export.rs`:

```rust
pub const DEFAULT_FILENAME_TEMPLATE: &str = "{name}_{copy}";
```

- [ ] **Step 5: Fix the fallout**

```bash
cargo build -p app 2>&1 | grep -E "^error" | head -20
```
`batch_export.rs:278` passes `copy_name` to `dest_named` via `export_stem` — replace that call with the template render already used for the templated path.

- [ ] **Step 6: Run the tests**

Expected: PASS, count **1428**. ⚠ Some existing `filename_template` tests assert the *old* semantics (`{name}` returning the copy's name, and `{name}_{copy}` doubling to `DSCF1234_2_2`). Update those assertions — they encode the behaviour being replaced, including the doubling this change exists to remove. **Do not** update `the_default_template_reproduces_the_old_naming_for_unlabelled_copies`; if that one fails, the implementation is wrong.

- [ ] **Step 7: Sabotage**

Swap `Token::Copy`'s first two arms so `(0, _) => None` comes first. Expected: `a_labelled_original_contributes_its_label` **fails**. Restore.

- [ ] **Step 8: Commit**

```bash
git add -A crates/app/src
git commit -m "feat(export): {name} is the stem, {copy} is the label

export_stem is deleted; the template composes the filename. The old rule
'a copy's name is its export name' survives as the template {copy}, so it
is generalised from hardcoded to chosen rather than dropped."
```

---

## Task 4: caption and badge in the grid

**Files:**
- Modify: `crates/app/src/thumb.rs:52-97`, `crates/app/src/library.rs`, `crates/app/src/filmstrip.rs`

- [ ] **Step 1: Write the failing test**

In `library.rs`'s test module:

```rust
/// ⚠ The grid must print the file's name under a copy, not the copy's own text. Sabotaging the
/// caption back to the label is what this catches.
#[test]
fn the_grid_captions_a_copy_with_its_file_name() {
    let e = PhotoEntry {
        id: PhotoId { path: "/x/DSCF1234.RAF".into(), copy: 2 },
        file_name: "DSCF1234.RAF".into(),
        label: Some("B&W".into()),
        edits: Edits::default(),
    };
    assert_eq!(e.caption(), "DSCF1234.RAF");
    assert_eq!(e.badge(), Some("B&W".to_string()));
}
```

- [ ] **Step 2: Run to verify it fails**

```bash
cargo test -p app --bin Rustroom the_grid_captions > /tmp/t4.log 2>&1; echo "EXIT=$?"
```

- [ ] **Step 3: Widen the badge**

In `crates/app/src/thumb.rs`, change the `copy: u32` parameter to `badge: Option<&str>`, replace `if copy != 0 {` with `if let Some(text) = badge {`, and `copy.to_string()` with `text.to_string()`.

- [ ] **Step 4: Update both callers**

In `library.rs` and `filmstrip.rs`, pass `entry.badge().as_deref()` where `entry.id.copy` was passed, and `entry.caption()` where the caption string was built.

- [ ] **Step 5: Run the tests**

Expected: PASS, count **1429**.

- [ ] **Step 6: Sabotage**

Change `caption` to return `self.label.clone().unwrap_or_else(|| self.file_name.clone())`. Expected: `the_grid_captions_a_copy_with_its_file_name` **fails**. Restore.

- [ ] **Step 7: Commit**

```bash
git add -A crates/app/src
git commit -m "feat(library): caption is the filename, badge carries the label"
```

---

## Task 5: split the drafts, and edit the label

**Files:**
- Modify: `crates/app/src/main.rs` (add `label_draft`), `crates/app/src/name_edit.rs`, `crates/app/src/copies.rs`, `crates/app/src/library.rs`, `crates/app/src/develop/panel/`

- [ ] **Step 1: Write the failing tests**

In `copies.rs`'s test module:

```rust
/// ⚠ Copy 0 may be labelled now. `set_copy_name` refused it, because a copy 0 "name" was the
/// filename; a label is not a filename, so the refusal has no premise left.
#[test]
fn the_original_can_be_labelled() {
    let (mut app, dir) = app_with_photo("DSCF1234.RAF");
    let id = PhotoId { path: dir.join("DSCF1234.RAF"), copy: 0 };
    assert!(app.set_label(&id, Some("Colour".into())));
    let s = sidecar::load_all(&id.path);
    assert_eq!(s.copies.iter().find(|c| c.copy == 0).unwrap().label.as_deref(), Some("Colour"));
}

/// ⚠ The two drafts are separate because one field doing both jobs is the defect underneath
/// every problem this change fixes. Opening a label draft must not arm a file rename.
#[test]
fn a_label_draft_does_not_arm_a_file_rename() {
    let (mut app, dir) = app_with_photo("DSCF1234.RAF");
    let id = PhotoId { path: dir.join("DSCF1234.RAF"), copy: 1 };
    app.label_draft = Some((id.clone(), "B&W".into()));
    assert!(app.rename_draft.is_none());
}
```

- [ ] **Step 2: Run to verify failure**

Expected: FAIL — `set_label` and `label_draft` not found.

- [ ] **Step 3: Add the field**

In `main.rs`, beside `rename_draft`:

```rust
    /// The label being typed, if any — a scratch draft exactly like `rename_draft`.
    ///
    /// ⚠⚠ **Separate from `rename_draft` on purpose.** One draft serving both jobs, with its
    /// meaning depending on whether the row was a copy, is the defect underneath every problem
    /// M6.0 fixes. A label and a filename are different strings with different rules.
    label_draft: Option<(PhotoId, String)>,
```

Add `label_draft: None` to the exhaustive literal in `test_app.rs`.

- [ ] **Step 4: Rename `set_copy_name` and drop the copy-0 refusal**

In `copies.rs`, rename `set_copy_name` → `set_label`, rename its `name` parameter to `label`, **delete** the `if id.is_original() { return false; }` block and its comment, and call `sidecar.set_label(...)`. Clear `self.label_draft` where it cleared `self.rename_draft`.

- [ ] **Step 5: Give `name_edit::field` a mode**

Add above `field`:

```rust
/// Which string a name field is editing.
///
/// ⚠ The field used to infer this from the copy number. That inference is what made one control
/// mean two things, and copy 0 being labellable removes the last basis for it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Editing {
    /// The file on disk — every copy follows, since they share it.
    FileName,
    /// This copy's label. Never a path, never sanitized here.
    Label,
}
```

Take `mode: Editing` as a parameter, and use `entries::caption(file_name, id.copy)` as the initial text for `FileName` and `label.unwrap_or_default()` for `Label`.

- [ ] **Step 6: Wire the two entry points**

In `library.rs`: caption click opens `rename_draft` with `Editing::FileName`; badge click opens `label_draft` with `Editing::Label`. In the Develop panel, bind the existing field to `label_draft` with `Editing::Label`.

- [ ] **Step 7: Run the tests**

Expected: PASS, count **1431**.

- [ ] **Step 8: Sabotage**

Reinstate `if id.is_original() { return false; }` in `set_label`. Expected: `the_original_can_be_labelled` **fails**. Restore.

- [ ] **Step 9: Commit**

```bash
git add -A crates/app/src
git commit -m "feat(app): edit what you see — caption renames the file, badge sets the label

rename_draft served both jobs with its meaning depending on the copy
number. Splitting it is the actual defect fix; copy 0 becoming labellable
removed the last basis for the inference."
```

---

## Task 6: search matches caption and label

**Files:**
- Modify: `crates/app/src/library_filter.rs:181-185`, `crates/app/src/rule.rs:207`

- [ ] **Step 1: Write the failing test**

In `library_filter.rs`:

```rust
/// ⚠ Searching "B&W" must find the copy labelled that. The filter matched one combined string
/// before; now that caption and label are separate, it must match **either**.
#[test]
fn the_text_filter_matches_the_label_as_well_as_the_file_name() {
    let e = entry("DSCF1234.RAF", 1, Some("B&W"));
    assert!(matches_text(&e, "b&w"), "must find a photo by its label");
    assert!(matches_text(&e, "dscf1234"), "must still find it by its file name");
    assert!(!matches_text(&e, "sepia"), "must not match an unrelated term");
}
```

- [ ] **Step 2: Run to verify it fails**

Expected: FAIL on the first assertion — the filter matches only the caption after Task 2.

- [ ] **Step 3: Match either**

Replace the `shown` binding in `library_filter.rs` and `rule.rs` with:

```rust
            // ⚠ Caption **or** label: they are two strings now, and a photo is findable by
            // whichever one the user remembers.
            let caption = entries::caption(&entry.file_name, entry.id.copy).to_lowercase();
            let label = entry.label.as_deref().unwrap_or_default().to_lowercase();
            let hit = caption.contains(&needle) || label.contains(&needle);
```

- [ ] **Step 4: Run the full suite**

Expected: PASS, count **1432**.

- [ ] **Step 5: Sabotage**

Drop the `|| label.contains(&needle)` term. Expected: the new test **fails** on its first assertion. Restore.

- [ ] **Step 6: Final gate**

```bash
cargo fmt --all --check > /tmp/f.log 2>&1; echo "FMT=$?"
cargo clippy --all-targets --all-features -- -D warnings > /tmp/c.log 2>&1; echo "CLIPPY=$?"
cargo test --all > /tmp/t.log 2>&1; echo "TEST=$?"
awk '/^test result:/{p+=$4; f+=$6} END{print "passed="p" failed="f}' /tmp/t.log
git status --porcelain crates/pipeline/ | wc -l
```
Expected: `FMT=0`, `CLIPPY=0`, `TEST=0`, **1432 passed / 0 failed**, **0** pipeline files touched.

- [ ] **Step 7: Validate against the real sidecars**

```bash
cargo run --release -q -p app --example sidecar_check ~/Desktop/fuji
cargo run --release -q -p app --example sidecar_check ~/Desktop/fuji_new
```
Expected: `31 sidecars, 31 copies` and `18 sidecars, 18 copies`, with snapshot counts unchanged (13 and 0). ⚠ Then copy one file from `~/Desktop/rustroom-sidecar-backup-20260901-131723/` into a scratch folder and run `sidecar_check` on it — that is the **only** fixture holding a real pre-change label, and it is what proves the read rule works on data rather than on a literal.

- [ ] **Step 8: Commit**

```bash
git add -A crates/app/src
git commit -m "feat(library): search matches a photo's label as well as its file name"
```

---

## Self-Review

**Spec coverage:** §1 model → Task 1. §2 display → Tasks 2, 4. §3 editing → Task 5. §4 export → Task 3. §5 read rule → Task 1. §6 filter → Task 6; *labels need not be unique* needs no code (nothing enforces uniqueness) and *a label does not travel with a paste* is preserved by leaving the field on `CopyEntry` rather than `Edits`, which Task 1 does not change. §7 testing → the four named properties appear in Tasks 1, 3 and 5. §8 caveats → no code.

**Type consistency:** `label: Option<String>` on both `CopyEntry` and `PhotoEntry`; `NameFacts.copy_label`; `set_label` on both `Sidecar` and `RustroomApp`; `entries::caption` / `entries::label_of` / `PhotoEntry::caption` / `PhotoEntry::badge`; `name_edit::Editing`. Consistent across tasks.

**Counts:** 1415 → 1420 → 1423 → 1428 → 1429 → 1431 → **1432**.
