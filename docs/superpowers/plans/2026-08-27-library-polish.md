# Library polish — persistent filters and a photo context menu

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Keep the filter when the source changes, and give a photo a right-click menu — Edit, Rename, Information.

**Architecture:** The filter moves from being rebuilt per `Library` to being carried across; the grid and filmstrip cells gain `Response::context_menu`; a new modal shows everything known about one photo, reading its dimensions live rather than from the catalog.

**Tech Stack:** Rust, `egui` 0.35. No new dependencies, **no schema change**.

⚠ **This document carries its own rationale rather than pointing at a separate spec.** Two
contained changes, both described precisely by the user; a separate design document would restate
them. Larger slices keep the spec/plan split.

**Not in this slice: shift/⌘-click multi-select of dates.** Folded into **M4.3c** by the user's
call, because multi-select periods and smart collections both want *"a source described by a
rule"* and building it first risks building it twice.

---

## Why the filter should persist — a reversal, and the reasoning behind it

M4.3a-ii deliberately reset the search text and mark filters on a source switch. The argument
recorded then: *"a stale search that empties a newly opened folder reads as breakage rather than
as a filter."*

⚠⚠ **That was wrong in use, and the reason is specific.** Browsing **replaces** the source — a
collection, a keyword, a date. So filter-then-pick-a-date is the *only* way to say "five-star
picks from the 9th", and resetting the filter destroys exactly the composition that
browse-replaces-source otherwise denies. The thing the reset protects against — a filter that
hides everything — is visible and one click from fixed, because the Filter button reads
`Filter: …` whenever it is active. **That label is what makes persistence safe**, and it did not
exist when the reset was chosen.

⚠ The format filter already persists, in `Settings`. This makes the rest behave the same way
**within a session**, and deliberately does *not* persist it across a restart: a search string is
about the question being asked now, and a format preference is standing.

---

## Ground rules for this repo

- **Branch:** `library-polish`. **Never push** — the user pushes.
- **Commit messages carry no `Co-Authored-By` trailer.**
- **Build env** — every `cargo` command needs this first, in the same shell:
  ```bash
  export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
  ```
- **The GUI binary is `Rustroom`**: bin tests are `cargo test -p app --bin Rustroom`.
- **⚠ In a binary crate an item lands with its first caller** — thirteenth instance.
- **⚠ Check exit codes, never grep output.**
- **⚠ The sabotage compile check is "did a `test result:` line appear at all"**, not a grep for
  `^error:`.
- ⚠ `clippy --all-targets --all-features` and `test --all` each take **over two minutes** here.

---

## Task 1: The filter survives a source switch

**Files:** Modify `crates/app/src/main.rs`

- [ ] **Step 1: Write the failing test**

In `main.rs`'s test module:

```rust
    #[test]
    fn the_filter_survives_a_source_switch() {
        // ⚠⚠ Browsing **replaces** the source, so filter-then-pick-a-date is the only way to say
        // "five-star picks from the 9th". Resetting the filter destroys exactly the composition
        // that browse-replaces-source otherwise denies — which is why the original reset, argued
        // for on the grounds that a stale filter "reads as breakage", was wrong in use.
        let dir = test_app::TestDir::new("filterkeep");
        std::fs::write(dir.0.join("a.RAF"), b"x").unwrap();
        std::fs::write(dir.0.join("b.RAF"), b"x").unwrap();
        let ctx = test_ui::ctx();
        let mut app = test_app::for_test();

        app.open_catalog_folder(&ctx, &dir.0);
        app.filter_mut().text = "a.RAF".into();
        app.library.as_mut().unwrap().refilter(app.filter());
        assert_eq!(app.library.as_ref().unwrap().visible.len(), 1);

        // Re-open the same folder — a source switch by any other name.
        app.open_catalog_folder(&ctx, &dir.0);
        assert_eq!(
            app.filter().text,
            "a.RAF",
            "the search text was reset by a source switch"
        );
        assert_eq!(
            app.library.as_ref().unwrap().visible.len(),
            1,
            "the filter was not applied to the new source"
        );
    }
```

⚠ The accessor names above are a **sketch**; pick whatever falls out of Step 2 and make the test
match. What must not change is what it asserts.

- [ ] **Step 2: Move the filter onto `RustroomApp`**

`Library.filter` becomes `RustroomApp.filter`, because **a filter is a view preference, not a
property of the loaded set** — which is precisely what the bug was. Keeping it on `Library` and
copying it across in `open_scan` would work too and was rejected: it leaves two places that can
disagree about which filter is current, and this project has paid for that shape three times
(`render_max_dim`, `Session::geometry_mode`, the thumbnail loader's shadow copy).

- Delete `filter` from `struct Library`; add to `RustroomApp`:

```rust
    /// Everything that narrows what the Library shows.
    ///
    /// ⚠⚠ On the **app**, not on `Library`, so it survives a source switch. Browsing replaces the
    /// source, so filter-then-pick-a-date is the only way to compose the two — and a filter reset
    /// on every switch destroyed exactly that. ⚠ It is safe to persist because the Filter button
    /// reads `Filter: …` whenever it is active, so a filter that hides everything is visible.
    ///
    /// ⚠ Session-only. The *format* half is separately persisted in `Settings`, because a format
    /// preference is standing where a search string is about the question being asked now.
    filter: rustroom_app::library_filter::LibraryFilter,
```

- `Library::refilter(&mut self, filter: &LibraryFilter)`. The compiler names every caller.
- `open_scan` no longer builds a filter. ⚠ Instead it seeds `self.filter.format` from
  `self.settings.hidden_formats` **only when no library was open**, so the standing format
  preference still applies on a first open without clobbering a session's choices.
- `format_bullets`, `mark_filter` and `search_field` read `app.filter` instead of `lib.filter`.

⚠ **`refilter` takes the filter by reference rather than reading it from `self`**, because `self`
is `RustroomApp` there and `refilter` is a `Library` method. Passing it keeps `Library` ignorant of
the app, which is what lets `library_of` build one in a test.

- [ ] **Step 3: Verify, then sabotage**

```bash
cargo test -p app --bin Rustroom the_filter_survives 2>&1 | grep -E "^test result:"
```

Sabotage: make `open_scan` assign `self.filter = Default::default()`. Confirm a `test result:` line
appears, then that the test fails. Restore.

- [ ] **Step 4: Full verification and commit**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
git add -A && git commit -m "fix(app): the filter survives a source switch"
```

---

## Task 2: Dimensions, read live

**Files:** Modify `crates/raw-decode/src/lib.rs`, `crates/image-decode/src/lib.rs`

- [ ] **Step 1: Add a reader to each decoder**

```rust
// raw-decode
/// The photo's pixel dimensions, without decoding it.
///
/// ⚠⚠ Read **live**, never from the catalog, and that is forced rather than chosen. Plan 1's
/// scanner queues files that have **no capture row**, so adding a `width`/`height` column now
/// would leave every already-scanned file NULL forever — the new field would apply only to photos
/// nobody owns yet. Fixing that needs a `capture_version` column and a re-read pass; reading one
/// photo on demand needs neither.
///
/// ⚠ Costs a whole-file read (~11–15 ms for a RAF) because `RawImage::open` takes a `&[u8]`. That
/// is fine for one photo when a dialog opens, and is exactly why the *scan* does not do it per
/// file for free.
pub fn dimensions(path: &Path) -> Option<(u32, u32)>;
```

For RAW, use the same `RawImage::open` path as `read_capture` and read `width()`/`height()`.
⚠ **Check whether those report the *raw sensor* size or the processed size** — CLAUDE.md records
that `sips` reports a RAF's unrotated sensor dimensions and that this sent an investigation down a
blind alley once. If they are unrotated, apply `sizes.flip` the way `apply_flip` already does, or
say in the doc comment that the value is the sensor's.

For images, `image::image_dimensions(path).ok()` reads only the header — cheap. ⚠ It does **not**
apply EXIF orientation, so a portrait JPEG reports landscape; either apply the orientation the way
`open_oriented` does or document it. **Whichever you choose, the two decoders must agree**, or a
RAW+JPEG pair reports different dimensions for the same photograph.

- [ ] **Step 2: Test what can be tested**

Gated on `RAW_FIXTURE` plus a JPEG beside it, asserting both are `Some` and **that a RAW and its
JPEG sibling agree** — the `pair_check` property, one field further. ⚠ Gated tests **skip**
without their fixture; say so.

- [ ] **Step 3: Verify and commit**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
RAW_FIXTURE=$HOME/Desktop/fuji/DSCF6223.RAF cargo test -p raw-decode
git add -A && git commit -m "feat(decode): report a photo's dimensions without decoding it"
```

---

## Task 3: The Information modal

**Files:** Modify `crates/app/src/modals/mod.rs` (+ a new `modals/info.rs`), `crates/app/src/main.rs`

- [ ] **Step 1: The modal**

`Modal::PhotoInfo { id: PhotoId }`, dispatched like the others. It shows, in sections:

- **File** — shown name, full path, copy number, file size, modified time.
- **Image** — dimensions and megapixels, read once when the dialog opens.
- **Capture** — taken, camera, lens, ISO, shutter, aperture, focal length.
- **Marks** — rating, flag, label; and the photo's keywords.

⚠ **Every value is `Option` and an absent one shows as `—`, never as a blank row.** A missing field
is information: it says this file carries no EXIF, which is exactly the case two of the author's
JPEGs are in.

⚠ **Dimensions are read once, on open, into the modal's own state** — not per frame. A dialog body
runs sixty times a second, and a whole-file read there would be the `apply_view_demosaic` mistake
in its most expensive costume yet.

⚠ Shutter is displayed as a **fraction** (`1/50 s`) below one second and as seconds above it. The
stored value is `0.0199999995529652`; showing that is technically honest and practically useless.

- [ ] **Step 2: Test the formatting**

The dialog is glue, but the value formatting is not — put it in pure functions and test those:

```rust
    #[test]
    fn a_fast_shutter_shows_as_a_fraction_and_a_slow_one_as_seconds() {
        assert_eq!(shutter_text(0.02), "1/50 s");
        assert_eq!(shutter_text(2.0), "2 s");
        assert_eq!(shutter_text(0.5), "1/2 s");
    }

    #[test]
    fn an_absent_value_shows_as_a_dash_rather_than_a_blank() {
        // ⚠ A missing field is information — it says the file carries no EXIF.
        assert_eq!(opt_text(None::<u32>), "—");
        assert_eq!(opt_text(Some(1600u32)), "1600");
    }

    #[test]
    fn a_file_size_is_shown_in_units_a_person_reads() {
        assert_eq!(size_text(505_252), "493 KB");
        assert_eq!(size_text(76_802_608), "73.2 MB");
    }
```

⚠ Check the exact rounding these produce before asserting — **fix the assertion to match correct
code, not the other way round.** Three tests in this session asserted something false about right
code; run them first and read the failure before touching the implementation.

- [ ] **Step 3: Commit** (with Task 4 — the modal has no caller until the menu exists; thirteenth
instance of the bin-crate dead-code rule).

---

## Task 4: The right-click menu

**Files:** Modify `crates/app/src/library.rs`, `crates/app/src/filmstrip.rs`, `crates/app/src/main.rs`

- [ ] **Step 1: Add the menu to both cells**

`Response::context_menu` on the cell response in each view:

```rust
                                resp.context_menu(|ui| {
                                    if ui.button("Edit").clicked() {
                                        menu = Some((i, CellAction::Edit));
                                        ui.close();
                                    }
                                    if ui.button("Rename").clicked() {
                                        menu = Some((i, CellAction::Rename));
                                        ui.close();
                                    }
                                    if ui.button("Information…").clicked() {
                                        menu = Some((i, CellAction::Info));
                                        ui.close();
                                    }
                                });
```

⚠ Check `Response::context_menu`'s exact signature and how a menu is dismissed in egui 0.35 —
`ui.close()` is a guess. **Read the vendored source**, as the muda work had to: CLAUDE.md records
two documentation facts that were wrong and cost a compile each.

⚠ **Collected into a local and applied after the closure**, like every other action in these
files — the closure holds `lib`, and all three actions need `&mut app`.

- [ ] **Step 2: Wire the three actions**

- **Edit** → `app.open_entry(ctx, i)`, which already selects and opens in Develop.
- **Rename** → set the rename draft for that entry, which is what a click on the caption already
  does. ⚠ Reuse `name_edit`'s draft rather than adding a second path — the two fields drifting is
  the bug that made copy names invisible in the grid.
- **Information…** → `app.modal = Modal::PhotoInfo { id }`.

⚠ **Right-clicking a photo that is not selected should select it first**, or Edit and Rename act
on something other than what was right-clicked. Check what egui does about selection on a
secondary click — it does *not* fire `clicked()`, so this needs doing explicitly.

- [ ] **Step 3: Verify, run and check by hand**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all
cargo run --release -p app
```

None of this is test-covered:

1. Right-click a grid thumbnail → the three items appear.
2. **Right-click a photo that is not selected** → it becomes selected, and Edit opens *that* photo.
3. Rename puts the caption into edit mode with focus.
4. Information shows dimensions, capture data and marks; **the two EXIF-less JPEGs show `—` rather
   than blanks**.
5. The same menu works in the Develop filmstrip.
6. **Set a filter, then click a date** — the filter holds, and the button still reads `Filter: …`.

- [ ] **Step 4: Commit**

```bash
git add -A && git commit -m "feat(app): a right-click menu on a photo, with an Information dialog"
```

---

## Task 5: Docs

- [ ] `CLAUDE.md` — a section covering: the filter-persistence **reversal** and why the earlier
  reasoning was wrong; that the `Filter: …` label is what makes persistence safe; the filter moving
  to `RustroomApp` rather than being copied; **the finding that a new `Capture` field cannot be
  backfilled** under the row-presence design, and that this is why dimensions are read live; and
  the caveats. Update the test count, and record that **shift/⌘-click multi-select of dates is
  folded into M4.3c**.
- [ ] The Obsidian roadmap — note the multi-select decision under M4.3c.
- [ ] Final `rustup update stable`, full verification by **exit code**, commit. **Do not push.**

---

## Definition of done

- `fmt`, `clippy --all-targets --all-features -D warnings`, `test --all` all exit 0.
- **All 47 goldens byte-identical.**
- A filter set in one source is still in force after browsing to another.
- Right-click on a photo offers Edit, Rename and Information, and Information shows dimensions plus
  everything the catalog knows, with `—` for what is absent.
