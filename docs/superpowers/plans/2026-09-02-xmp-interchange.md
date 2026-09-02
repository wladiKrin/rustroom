# XMP interchange — M6.3a/b, implementation plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ratings, colour labels and keywords round-trip with Lightroom through `stem.xmp`
sidecars — read on folder open when the sidecar changed, written automatically after every mark or
keyword change, **merging** into any sidecar that already exists.

**Architecture:** A new `crates/xmp/` crate (parser + merging writer, zero GUI and zero catalog
dependency, one dependency: `quick-xml`), schema **v6** with one `photos.xmp_mtime` column, an app
**lib** module `xmp_sync` holding every rule (label table, keyword paths, import, write, the
newer-than-last-seen check), and two thin bin hooks — the read hook in `open_scan`, the write hook in
`catalog_ops.rs`.

**Spec:** `docs/superpowers/specs/2026-09-02-xmp-interchange-design.md`. Its decisions are settled
and are not re-opened here.

---

## Ground rules for this repo

- **Branch:** the worktree's own branch. **Never push**; the user merges.
- **Commit messages carry no `Co-Authored-By` trailer.**
- **Build env** — every `cargo` command needs this first, in the same shell:
  ```bash
  export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
  ```
- **The GUI binary is `Rustroom`**: bin tests are `cargo test -p app --bin Rustroom`.
- **⚠ In a binary crate an item lands with its first caller** (the dead-code trap, eleven times).
- **⚠ Unique log names** — `/tmp/w3b-<name>.log`; other agents share `/tmp`. Counts come from
  Python over the log; the exit code is the authority.
- **⚠ Every sabotage must be confirmed to have (a) compiled — a `test result:` line — (b) changed
  the file — `diff` against a backup — and (c) run at least one test — `passed + failed >= 1`.**
- **Baseline: 1627 tests, 51 goldens.** No pipeline code is touched, so every golden must stay
  byte-identical.

---

## Design decisions the spec leaves to the implementation (recorded, reported at the end)

1. **Byte-exact pass-through by span copying, not by re-serialising.** `quick-xml` is used as a
   tokenizer with positions (`buffer_position()`); every event not owned by this crate is copied
   from the source **verbatim**. The one tag that is rewritten — the first `rdf:Description` — has
   only the owned attributes' byte ranges removed, so a Lightroom sidecar's `crs:` lines come out
   byte-identical, which is what "diff the `crs:` lines" in the spec's done-when asks for.
2. **The fresh packet is the merge into a skeleton.** `write(sidecar, None)` merges into a
   built-in empty Lightroom-shaped document, so there is exactly one injection code path.
3. **Owned fields are matched by namespace, never by prefix.** `xap:Rating` (the old prefix for
   the same namespace) is owned too. Namespaces: `http://ns.adobe.com/xap/1.0/`,
   `http://purl.org/dc/elements/1.1/`, `http://ns.adobe.com/lightroom/1.0/`,
   `http://www.w3.org/1999/02/22-rdf-syntax-ns#`.
4. **Reading scans every `rdf:Description`**, because exiftool writes one per namespace; the
   merge strips owned fields from all of them and injects into the first (spec §5).
5. **A document with no `rdf:Description` is an error, not a fresh packet** — replacing an
   unrecognised file with ours is the overwrite §5 forbids.
6. **Import: absent `xmp:Rating` reads as 0 and absent `xmp:Label` as no label**; a label text
   outside the four leaves the catalog value alone (spec §1). The flag is preserved. Keywords are
   **additive** — the spec lists create-and-assign and no unassign; recorded as a caveat.
7. **The read hook runs in `open_scan`**, the one place a `Library` is built, restricted to the
   copy-0 entries on show and grouped per folder (one query per folder). A folder open, a
   collection and All Photos all see a foreign edit.
8. **`write_xmp` gets a checkbox in the Settings dialog** (`modals/settings.rs`) — a toggle with
   no UI is unreachable.

---

## File structure

| file | responsibility | new? |
|---|---|---|
| `Cargo.toml` (root) | workspace member `crates/xmp`, `quick-xml` in `[workspace.dependencies]` | modify |
| `crates/xmp/Cargo.toml`, `src/lib.rs` | `Sidecar`, `read`, `write`, `XmpError` | **create** |
| `crates/catalog/src/db.rs` | v6 gate, `xmp_mtimes_in`, `set_xmp_mtime`, both drop lists, tests | modify (⚠ 3,027 lines) |
| `crates/app/Cargo.toml`, `src/lib.rs` | `xmp` dependency, `pub mod xmp_sync` | modify |
| `crates/app/src/xmp_sync.rs` | label table, keyword paths, sidecar path, import, write, newer check | **create** |
| `crates/app/src/settings.rs` | `write_xmp: bool` (default true) | modify (⚠ 1,571 lines) |
| `crates/app/src/modals/settings.rs` | the checkbox | modify |
| `crates/app/src/sources.rs` | read hook in `open_scan` + app tests | modify |
| `crates/app/src/catalog_ops.rs` | write hook after marks and keyword changes | modify |

---

## Task 1: the `xmp` crate reads ratings and labels

- [ ] Add `crates/xmp` to the workspace and `quick-xml = "0.41"` to workspace deps.
- [ ] `crates/xmp/src/lib.rs`: `Sidecar`, `XmpError`, `read`. Tests:
  - `a_rating_and_label_read_from_attributes` — Lightroom shape (`xmp:Rating="4" xmp:Label="Red"`
    on `rdf:Description`).
  - `a_rating_and_label_read_from_elements` — `<xmp:Rating>3</xmp:Rating>` children.
  - `the_old_xap_prefix_is_the_same_namespace` — `xap:Rating` on a description bound to the xmp
    namespace reads the same.
  - `a_rating_in_a_second_description_is_found` — exiftool's one-description-per-namespace shape.
- [ ] Run: `cargo test -p xmp`; sabotage: drop the element branch → the element test fails.
- [ ] Commit: `feat(xmp): read ratings and labels from a sidecar`

## Task 2: keywords, flat and hierarchical

- [ ] Tests: `hierarchical_subjects_parse_to_paths`, `a_flat_subject_is_a_path_of_one`,
  `the_hierarchical_form_wins_when_both_are_present`, `an_entity_in_a_keyword_is_unescaped`.
- [ ] Implement `dc:subject` / `lr:hierarchicalSubject` collection with `read_text` + `unescape`.
- [ ] Sabotage: prefer the flat form → the "wins" test fails.
- [ ] Commit: `feat(xmp): read flat and hierarchical keywords`

## Task 3: a fresh packet, and the round trip

- [ ] Tests: `a_fresh_packet_reads_back_as_written`, `a_fresh_packet_writes_both_keyword_forms`,
  `an_unset_field_is_absent_from_a_fresh_packet`, `a_keyword_with_markup_characters_is_escaped`.
- [ ] Implement `write` as merge-into-skeleton (Task 4's engine, applied to the built-in document).
- [ ] Commit: `feat(xmp): write a Lightroom-shaped packet`

## Task 4: the merge — the load-bearing test

- [ ] Tests, on a hand-written Lightroom fixture carrying `crs:Exposure2012`,
  `crs:ToneCurvePV2012` (a `rdf:Seq` block) and a `<crs:Look>` block:
  - `merging_keeps_every_foreign_byte_and_changes_only_the_rating` — every line containing `crs:`
    is identical before and after; the rating is the new one; the label and keywords are the new
    ones.
  - `merging_replaces_owned_child_elements_in_place` — an existing `<dc:subject>` bag and an
    element-form rating are removed, not duplicated (`read` after `write` sees exactly ours).
  - `merging_into_an_empty_description_tag_opens_it` — `<rdf:Description … />`.
  - `a_document_with_no_description_is_refused`.
- [ ] Implement the span-copying merge.
- [ ] **Sabotage: make `write` ignore `existing` and emit a fresh packet → the first test must
  fail.** Also: skip only the attribute, not the element → the second fails.
- [ ] Commit: `feat(xmp): merge into an existing sidecar rather than replacing it`

## Task 5: malformed input is an error

- [ ] Tests: `malformed_xml_is_an_error_not_a_panic` (mismatched end tag), `an_unclosed_document_is_an_error`.
- [ ] Implement the depth check at EOF.
- [ ] Sabotage: remove the depth check → the unclosed test fails.
- [ ] Commit: `feat(xmp): refuse malformed sidecars`

## Task 6: schema v6

- [ ] `SCHEMA_VERSION = 6`; `if from < 6` gate adding `xmp_mtime INTEGER NOT NULL DEFAULT 0`.
- [ ] `xmp_mtimes_in(folder) -> Vec<(PathBuf, i64)>` (copy 0, one query), `set_xmp_mtime(photo, secs)`.
- [ ] Both drop lists gain `ALTER TABLE photos DROP COLUMN xmp_mtime;`.
- [ ] Tests: `a_version_five_catalog_upgrades_and_keeps_everything` (marks, keyword, collection
  survive; `xmp_mtime` reads 0), `xmp_mtime_defaults_to_zero_and_round_trips`,
  `xmp_mtimes_come_back_for_copy_zero_only`.
- [ ] Sabotage: remove the new drop-list line from one migration test → it fails with
  "duplicate column"; remove the v6 gate → the v5 test fails.
- [ ] Commit: `feat(catalog): schema v6 records when a photo's XMP sidecar was last seen`

## Task 7: `Settings.write_xmp`

- [ ] Field appended, default `true`, exhaustive literal gets `write_xmp: false`, test
  `a_settings_file_from_before_write_xmp_still_loads` asserts `true`.
- [ ] Checkbox in `modals/settings.rs`, change-guarded.
- [ ] Sabotage: default `false` → the still-loads test fails.
- [ ] Commit: `feat(settings): a toggle for writing XMP sidecars`

## Task 8: `xmp_sync` — the rules, headless

- [ ] `crates/app/src/xmp_sync.rs`: `sidecar_path`, `takes_sidecar`, `label_to_xmp`,
  `label_from_xmp`, `keyword_paths`, `sidecar_of`, `write_sidecar`, `import`, `import_newer`.
- [ ] Tests (in-memory catalog + `TempDir`):
  - `the_label_table_is_total_over_one_to_max_label` — every `1..=MAX_LABEL` maps out and back.
  - `a_label_outside_the_four_leaves_the_catalog_value_alone`.
  - `an_absent_label_clears_and_an_absent_rating_is_zero`.
  - `importing_creates_the_keyword_path_and_assigns_the_leaf`.
  - `the_flag_survives_an_import`.
  - `a_jpeg_never_gets_a_sidecar`.
  - `writing_then_reading_round_trips_through_the_catalog`.
  - `writing_merges_into_a_lightroom_sidecar` (the `crs:` lines survive end to end).
  - `a_newer_sidecar_imports_and_an_older_one_does_not`.
  - `our_own_write_does_not_re_import`.
- [ ] Sabotages: `>=` instead of `>` in the newer check; skip `set_xmp_mtime` after a write;
  `is_raw` check removed.
- [ ] Commit: `feat(app): xmp_sync — import, write and the newer-than-last-seen rule`

## Task 9: the hooks

- [ ] `open_scan`: `import_newer` over the copy-0 entries before `reload_marks`.
- [ ] `catalog_ops.rs`: `write_xmp_for(files)` after `apply_mark_to` (copy 0 only) and after each of
  the three keyword assign/unassign sites.
- [ ] Bin tests in `sources.rs`: `a_raw_with_a_newer_sidecar_imports_on_open`,
  `rating_a_raw_writes_its_sidecar`, `a_jpeg_gets_no_sidecar_when_rated`,
  `the_write_toggle_is_honoured`.
- [ ] Sabotage: remove the hook call → the open test fails; remove the write hook → the rating
  test fails.
- [ ] Commit: `feat(app): read XMP on open and write it after every mark or keyword change`

## Task 10: verification

- [ ] `cargo fmt --all --check`, `cargo clippy --all-targets --all-features -- -D warnings`,
  `cargo test --all` — exit 0 each; count = 1627 + new tests; 51 goldens byte-identical
  (`git status` shows no PNG changed).
- [ ] `exiftool -Rating -Label -Subject -HierarchicalSubject` on a sidecar written by the app
  (it is installed at `/opt/homebrew/bin/exiftool`).
- [ ] Report.
