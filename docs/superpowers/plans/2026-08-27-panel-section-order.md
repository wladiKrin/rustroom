# Panel section order Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let the Library panel's five sections be reordered by dragging a grip in each header, with the order persisted.

**Architecture:** A `PanelSection` enum makes the order a value in `Settings`; `collections_panel::show` loops it and `match`es each variant to its existing body verbatim. A grip painted as six dots is the only drag source. Placement is two pure functions in a new lib module, so the one testable part of the interaction actually is tested.

**Tech Stack:** Rust, egui 0.35 (`CollapsingState::show_header`, `Sense::drag()`, `Painter`), serde.

**Read first:** `docs/superpowers/specs/2026-08-27-panel-section-order-design.md`, especially its three egui facts — they rule out every simpler-looking approach.

**Before you start:**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

⚠ **No `Co-Authored-By` trailer. Never push. Never merge unprompted.** Branch: `panel-section-order`.

⚠ **Check clippy's exit code before every commit.** Five commits this week went in with clippy red because a green `cargo test` was mistaken for evidence about it.

⚠⚠ **`collections_panel::show` is 551 lines of closures over disjoint `app` fields.** Task 3 moves five large bodies without changing them. **Move them verbatim.** If a body stops compiling after the move, the cause is almost always that it referenced a local defined by an *earlier* section — read the spec's note about `picked` and `selected`, hoist the local out of the panel, and do not "fix" it by reordering the arms.

---

### Task 1: `PanelSection` and a repaired order

**Files:**
- Modify: `crates/app/src/settings.rs` (near `PanelSections`, around line 395)

⚠ `settings` is a **lib** module, so this commits alone — a `pub` item in a lib is public API and does not trip `dead_code`.

- [ ] **Step 1: Write the failing tests**

Append to `mod tests` in `crates/app/src/settings.rs`:

```rust
    /// ⚠ `settings.json` is hand-editable, so the stored order can be any list at all. A section
    /// missing from it must still be reachable.
    #[test]
    fn a_stored_order_missing_a_section_gets_it_appended() {
        let s = Settings {
            panel_order: vec![PanelSection::Dates, PanelSection::Folders],
            ..Settings::default()
        };
        let got = s.panel_order();
        assert_eq!(got[0], PanelSection::Dates, "the stored prefix is kept");
        assert_eq!(got[1], PanelSection::Folders);
        assert_eq!(
            got.len(),
            PanelSection::ALL.len(),
            "and every section is present"
        );
        for sec in PanelSection::ALL {
            assert!(got.contains(&sec), "{sec:?} must be reachable");
        }
    }

    #[test]
    fn a_duplicated_section_appears_once() {
        let s = Settings {
            panel_order: vec![
                PanelSection::Folders,
                PanelSection::Folders,
                PanelSection::Dates,
            ],
            ..Settings::default()
        };
        let got = s.panel_order();
        assert_eq!(got.len(), PanelSection::ALL.len());
        assert_eq!(
            got.iter().filter(|x| **x == PanelSection::Folders).count(),
            1
        );
    }

    #[test]
    fn an_empty_stored_order_gives_the_default() {
        let s = Settings {
            panel_order: Vec::new(),
            ..Settings::default()
        };
        assert_eq!(s.panel_order(), PanelSection::ALL.to_vec());
    }

    /// ⚠ The default really is the order the panel shipped with — Folders first, because "where
    /// photos come from" is the outer concept.
    #[test]
    fn the_default_order_is_the_shipped_one() {
        assert_eq!(Settings::default().panel_order(), PanelSection::ALL.to_vec());
        assert_eq!(PanelSection::ALL[0], PanelSection::Folders);
    }

    /// ⚠ `SettingsStore::load` is infallible, so one unparseable field silently reverts **every**
    /// setting. The sibling of `a_settings_file_from_before_dialog_sizes_still_loads`.
    #[test]
    fn a_settings_file_from_before_the_panel_order_still_loads() {
        let f = TempFile::new("prepanelorder");
        std::fs::write(&f.0, r#"{"library_cell":200}"#).unwrap();
        let got = SettingsStore::new(f.0.clone()).load();
        assert_eq!(got.library_cell, 200, "the stored field must survive");
        assert_eq!(
            got.panel_order(),
            PanelSection::ALL.to_vec(),
            "and an absent order must give the default"
        );
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p app --lib panel_order 2>&1 | tail -20`
Expected: FAIL to compile — `cannot find type PanelSection`.

- [ ] **Step 3: Write the implementation**

Add to `crates/app/src/settings.rs`, immediately above `PanelSections`:

```rust
/// One section of the Library panel.
///
/// ⚠ Exists so the panel's **order** can be a value rather than five literal calls. The bodies
/// stay exactly where they are; `collections_panel::show` matches this to each one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PanelSection {
    Folders,
    Collections,
    Smart,
    Keywords,
    Dates,
}

impl PanelSection {
    /// Every section, in the order the panel shipped with.
    ///
    /// ⚠ Folders first because "where photos come from" is the outer concept, and All Photos —
    /// the whole catalog — is the natural top of it. That reasoning is `collections_panel`'s and
    /// is preserved as the default rather than re-derived.
    pub const ALL: [PanelSection; 5] = [
        Self::Folders,
        Self::Collections,
        Self::Smart,
        Self::Keywords,
        Self::Dates,
    ];
}
```

Add the field to `Settings`, after `panel_sections`:

```rust
    /// The order the Library panel's sections are drawn in.
    ///
    /// ⚠⚠ **Never read directly — go through [`Settings::panel_order`]**, which repairs it. This
    /// is a `Vec` rather than an array precisely because `settings.json` is hand-editable and can
    /// hold any list at all.
    pub panel_order: Vec<PanelSection>,
```

Add to `impl Settings`:

```rust
    /// The panel's section order, guaranteed to contain every section exactly once.
    ///
    /// ⚠⚠ **Repaired rather than trusted.** The file is user-editable, so the stored order can
    /// omit a section, list one twice, or be empty. Duplicates are dropped and anything missing is
    /// appended in default order, so **a section can never become unreachable** whatever the file
    /// says. That is `FormatFilter`'s hidden-by-exception robustness: the safe state is the one
    /// you get by default, not the one you have to maintain.
    pub fn panel_order(&self) -> Vec<PanelSection> {
        let mut out: Vec<PanelSection> = Vec::with_capacity(PanelSection::ALL.len());
        for s in &self.panel_order {
            if !out.contains(s) {
                out.push(*s);
            }
        }
        for s in PanelSection::ALL {
            if !out.contains(&s) {
                out.push(s);
            }
        }
        out
    }
```

Add to `impl Default for Settings`, after `panel_sections`:

```rust
            panel_order: PanelSection::ALL.to_vec(),
```

⚠ The method and the field share a name; that is deliberate — `self.panel_order` is the raw stored
value and `self.panel_order()` the repaired one, and the call site cannot get them confused because
only the method returns a usable list. Say so in the field's doc comment.

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p app --lib settings:: 2>&1 | tail -5`
Expected: PASS.

Run: `cargo build --workspace --all-targets 2>&1 | tail -20`
Expected: **failure** in `settings_round_trip_carries_the_export_options`, whose `Settings { .. }`
literal is exhaustive on purpose. Add a **non-default** order so the round trip carries it:

```rust
            // Non-default so a field re-defaulted on load cannot pass.
            panel_order: vec![PanelSection::Dates, PanelSection::Folders],
```

⚠ That literal must then survive `load` **unrepaired** — the round-trip test asserts equality on
the raw struct, and repair happens only in the accessor. If it fails, the repair has leaked into
serde, which it must not.

- [ ] **Step 5: Sabotage**

Change `panel_order()` to `self.panel_order.clone()`.
Run: `cargo test -p app --lib settings::tests::a_stored_order 2>&1 | tail -20`
Expected: **FAIL** on the appended-section assertion. ⚠ Confirm a `test result:` line appears.
Restore and re-run.

- [ ] **Step 6: Commit**

```bash
cargo fmt && cargo clippy --all-targets --all-features -- -D warnings; echo "clippy exit: $?"
git add crates/app/src/settings.rs
git commit -m "feat(settings): the Library panel's section order, repaired on read"
```

---

### Task 2: `panel_order` — placement as pure functions

**Files:**
- Create: `crates/app/src/panel_order.rs`
- Modify: `crates/app/src/lib.rs` (declare the module)

⚠⚠ **This is the point of the whole approach.** With egui's `dnd_drag_source`/`dnd_drop_zone` this
logic would live inside egui and there would be nothing to test; here it is two pure functions with
no `Ui` in sight — and panel interaction is exactly this project's untested territory.

- [ ] **Step 1: Write the failing tests**

Create `crates/app/src/panel_order.rs`:

```rust
//! Where a dragged panel section lands.
//!
//! ⚠⚠ Pure functions with no `egui` in sight, deliberately. The alternative — egui's
//! `dnd_drag_source`/`dnd_drop_zone` — puts this logic inside the framework, where none of it can
//! be tested; and the standing warning in `CLAUDE.md` is that canvas and panel code is untested
//! territory. This is the one part of the gesture that can be made otherwise.

use crate::settings::PanelSection;

/// Which slot a section dragged to `pointer_y` should land in.
///
/// `header_tops` is each drawn section header's top edge, in order. The result is an *insertion*
/// index in `0..=header_tops.len()`.
///
/// ⚠ A section is claimed by the slot whose header it has been dragged **past the top of**, so
/// dropping anywhere above the first header gives 0 and anywhere below the last gives the end.
pub fn drop_index(header_tops: &[f32], pointer_y: f32) -> usize {
    header_tops.iter().filter(|top| pointer_y >= **top).count()
}

/// The order after moving the section at `from` into position `to`.
///
/// ⚠ Both indices are clamped rather than trusted: `from` comes from a drag that may have started
/// before a re-sort, and `to` from a pointer position. An out-of-range index must not panic in a
/// paint loop.
pub fn moved(order: &[PanelSection], from: usize, to: usize) -> Vec<PanelSection> {
    let mut out = order.to_vec();
    if out.is_empty() || from >= out.len() {
        return out;
    }
    let item = out.remove(from);
    // ⚠ `to` is an *insertion* index into the list **including** the item being moved, so removing
    // it first shifts every later slot down by one.
    let to = to.min(out.len());
    let to = if to > from { to.saturating_sub(1) } else { to };
    out.insert(to.min(out.len()), item);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOPS: [f32; 3] = [10.0, 50.0, 90.0];

    #[test]
    fn a_drop_above_everything_lands_first() {
        assert_eq!(drop_index(&TOPS, 0.0), 0);
    }

    #[test]
    fn a_drop_below_everything_lands_last() {
        assert_eq!(drop_index(&TOPS, 1000.0), 3);
    }

    #[test]
    fn a_drop_between_two_headers_lands_between_them() {
        assert_eq!(drop_index(&TOPS, 60.0), 2);
    }

    /// ⚠ Exactly on a boundary — the case a naive implementation gets wrong and no manual test
    /// would ever notice, because a pointer rarely lands on an exact pixel edge.
    #[test]
    fn a_drop_exactly_on_a_header_top_claims_that_slot() {
        assert_eq!(drop_index(&TOPS, 50.0), 2);
    }

    #[test]
    fn an_empty_panel_has_one_slot() {
        assert_eq!(drop_index(&[], 5.0), 0);
    }

    fn order() -> Vec<PanelSection> {
        PanelSection::ALL.to_vec()
    }

    /// The motivating case, in the words it was asked in: folders below collections.
    #[test]
    fn dragging_folders_below_collections_puts_it_second() {
        let got = moved(&order(), 0, 2);
        assert_eq!(got[0], PanelSection::Collections);
        assert_eq!(got[1], PanelSection::Folders);
        assert_eq!(got.len(), PanelSection::ALL.len());
    }

    #[test]
    fn dragging_upward_works_too() {
        let got = moved(&order(), 4, 0);
        assert_eq!(got[0], PanelSection::Dates);
        assert_eq!(got[1], PanelSection::Folders);
    }

    /// ⚠ Dropping where it already is must be a no-op, not an off-by-one shuffle.
    #[test]
    fn dropping_a_section_where_it_already_is_changes_nothing() {
        for i in 0..PanelSection::ALL.len() {
            assert_eq!(moved(&order(), i, i), order(), "moving {i} onto itself");
            assert_eq!(
                moved(&order(), i, i + 1),
                order(),
                "or onto the slot just after itself"
            );
        }
    }

    /// ⚠ Every move must be a permutation — nothing lost, nothing duplicated. A property over all
    /// pairs, because an off-by-one only shows at particular indices.
    #[test]
    fn every_move_is_a_permutation() {
        let n = PanelSection::ALL.len();
        for from in 0..n {
            for to in 0..=n {
                let got = moved(&order(), from, to);
                assert_eq!(got.len(), n, "moving {from} to {to} changed the length");
                for s in PanelSection::ALL {
                    assert!(got.contains(&s), "moving {from} to {to} lost {s:?}");
                }
            }
        }
    }

    /// ⚠ Out-of-range indices are clamped, never a panic: this runs inside a paint loop.
    #[test]
    fn out_of_range_indices_do_not_panic() {
        assert_eq!(moved(&order(), 99, 0), order());
        assert_eq!(moved(&order(), 0, 99).len(), PanelSection::ALL.len());
        assert!(moved(&[], 0, 0).is_empty());
    }
}
```

- [ ] **Step 2: Declare the module**

In `crates/app/src/lib.rs`, in alphabetical position:

```rust
pub mod panel_order;
```

- [ ] **Step 3: Run**

Run: `cargo test -p app --lib panel_order 2>&1 | tail -6`
Expected: PASS, 10 tests.

- [ ] **Step 4: Sabotage**

Change `drop_index`'s comparison from `>=` to `>`.
Expected: **FAIL** on `a_drop_exactly_on_a_header_top_claims_that_slot` only — which is that
test's whole reason for existing.

Then remove the `if to > from` adjustment in `moved`.
Expected: **FAIL** on `dropping_a_section_where_it_already_is_changes_nothing` and
`dragging_folders_below_collections_puts_it_second`.

⚠ Confirm a `test result:` line appears for each. Restore after both.

- [ ] **Step 5: Commit**

```bash
cargo fmt && cargo clippy --all-targets --all-features -- -D warnings; echo "clippy exit: $?"
git add crates/app/src/panel_order.rs crates/app/src/lib.rs
git commit -m "feat(panel): where a dragged section lands, as pure functions"
```

---

### Task 3: Draw the sections from the order

**Files:**
- Modify: `crates/app/src/collections_panel.rs` (`section`, and the body of `show`)

⚠⚠ **Move the five bodies verbatim.** This task changes *where* they are called from and nothing
about what they do. Any behaviour change here is a bug, and the file is 551 lines of closures over
disjoint `app` fields.

- [ ] **Step 1: Rewrite `section` on `CollapsingState`**

⚠ `CollapsingHeader` has no slot for header content, so the grip needs `CollapsingState::show_header`.

Replace `section` in `crates/app/src/collections_panel.rs`:

```rust
/// One collapsible panel section, with a drag grip in its header.
///
/// Returns the grip's `Response`, which is the **only** thing that senses a drag.
///
/// ⚠⚠ Built on `CollapsingState` rather than `CollapsingHeader` because that one has no slot for
/// header content, and the grip has to live in the header.
///
/// ⚠⚠ **The grip is the only drag source, and that is forced.** `Ui::dnd_drag_source` registers a
/// drag `interact` over the *whole* rect it wraps, and hit-test ties go to whatever registered
/// last — so making the section draggable would swallow every button, field and list inside it,
/// which is the exact tie-break that broke Library thumbnails once. Making the *header* draggable
/// would swallow the click that toggles it.
fn section<R>(
    ui: &mut egui::Ui,
    open: &mut bool,
    id: &str,
    title: &str,
    body: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::Response {
    let state_id = ui.make_persistent_id(id);
    let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
        ui.ctx(),
        state_id,
        *open,
    );
    let mut grip = None;
    let header = state.show_header(ui, |ui| {
        grip = Some(drag_grip(ui));
        ui.label(egui::RichText::new(title).heading());
    });
    state.show_body_indented(&header.0.response, ui, body);
    if header.0.response.clicked() {
        *open = !*open;
    }
    grip.expect("the header closure always runs")
}

/// A six-dot drag grip. Returns its response, which senses a drag.
///
/// ⚠⚠ **Painted, not a font glyph.** This project has three recorded cases of a chosen glyph being
/// absent from the bundled fonts and shipping as tofu — the rail's four, the mark badges, the
/// keyword ticks — each found only by probing. A grip *is* six dots, so drawing it needs no font,
/// no probe and no coverage test, and it takes the theme's colour for free.
fn drag_grip(ui: &mut egui::Ui) -> egui::Response {
    let size = egui::vec2(10.0, ui.spacing().interact_size.y);
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::drag());
    let resp = resp.on_hover_cursor(egui::CursorIcon::Grab);
    let c = if resp.hovered() || resp.dragged() {
        ui.visuals().strong_text_color()
    } else {
        ui.visuals().weak_text_color()
    };
    let p = ui.painter();
    for row in 0..3 {
        for col in 0..2 {
            let x = rect.center().x + (col as f32 - 0.5) * 4.0;
            let y = rect.center().y + (row as f32 - 1.0) * 4.0;
            p.circle_filled(egui::pos2(x, y), 1.2, c);
        }
    }
    resp
}
```

⚠ **Verify `show_header`'s real return type against the vendored source** at
`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/egui-0.35.0/src/containers/collapsing_header.rs`
line 129 before trusting `header.0.response` — adapt the destructuring rather than guessing.

- [ ] **Step 2: Loop the order**

In `show`, replace the five sequential `section(..)` calls with a loop over the repaired order.
Each existing body moves into its `match` arm **unchanged**:

```rust
                    // ⚠ The order is a value now. The `match` is what lets five closures over five
                    // different sets of `app` fields keep their exact previous form while their
                    // order becomes data.
                    let order = app.settings.panel_order();
                    // Each header's top edge this frame, for `panel_order::drop_index`.
                    let mut tops: Vec<f32> = Vec::with_capacity(order.len());
                    for sec in &order {
                        let top = ui.cursor().top();
                        let grip = match sec {
                            PanelSection::Folders => {
                                section(ui, &mut sections.folders, "sec_folders", "Folders", |ui| {
                                    /* the existing Folders body, verbatim */
                                })
                            }
                            PanelSection::Collections => { /* … */ }
                            PanelSection::Smart => { /* … */ }
                            PanelSection::Keywords => { /* … */ }
                            PanelSection::Dates => { /* … */ }
                        };
                        tops.push(top);
                        if grip.drag_started() {
                            dragging = Some(*sec);
                        }
                    }
```

⚠ **`/* the existing … body, verbatim */` above is shorthand for this plan only.** Move the real
closure bodies across; do not retype or "tidy" them. They are, in `collections_panel.rs` before any
edit:

| section | lines |
|---|---|
| Folders | 89–137 |
| Collections | 139–197 |
| Smart | 199–259 |
| Keywords | 261–352 |
| Dates | 354–432 |

⚠ Those are the whole `section(..)` calls including their closing `);`. Take the **closure body**
from each — the part between `|ui| {` and its matching `}` — and put it in the arm.

⚠ `dragging` is a `Option<PanelSection>` local declared before the panel, applied after it — the
shape every panel in this app uses, because the closure already borrows `app`.

- [ ] **Step 3: Build and work through the fallout**

Run: `cargo build --workspace --all-targets 2>&1 | tail -30`
Expected: errors only where a moved body referenced a local defined by an earlier section. ⚠ Hoist
that local **above the panel**, beside `picked` and `selected` — do not reorder the arms to make it
compile, or the order stops being permutable, which is the whole feature.

- [ ] **Step 4: Verify nothing else moved**

Run: `cargo test --all 2>&1 | grep "test result:" | awk -F'[ ;]' '{p+=$4; f+=$6} END {print "passed="p, "failed="f}'`
Expected: no failures.

- [ ] **Step 5: Commit**

```bash
cargo fmt && cargo clippy --all-targets --all-features -- -D warnings; echo "clippy exit: $?"
git add crates/app/src/collections_panel.rs
git commit -m "refactor(panel): draw the sections from a stored order"
```

---

### Task 4: The drag

**Files:**
- Modify: `crates/app/src/collections_panel.rs`
- Modify: `crates/app/src/main.rs` (one field on `RustroomApp`)
- Modify: `crates/app/src/test_app.rs` (the exhaustive literal)

- [ ] **Step 1: Add the drag state**

In `crates/app/src/main.rs`, add to `RustroomApp`:

```rust
    /// The panel section being dragged, if one is.
    ///
    /// ⚠ On the app rather than a local, because a drag spans frames. Cleared on release.
    panel_drag: Option<rustroom_app::settings::PanelSection>,
```

Add `panel_drag: None,` to its initialiser **and** to the exhaustive literal in
`crates/app/src/test_app.rs::for_test` — that literal is exhaustive on purpose, so the compiler
will name it.

- [ ] **Step 2: Track the drag and paint the insertion line**

Three locals declared **before** the panel, applied after it — the shape every panel here uses,
because the closure already borrows `app`:

```rust
    // ⚠ One coherent set: which section a grip started dragging this frame, whether a grip was
    // released this frame, and where the pointer says it would land. The move is applied only when
    // all three line up, after the panel closes.
    let mut drag_started: Option<PanelSection> = None;
    let mut drag_released = false;
    let mut drop_to: Option<usize> = None;
```

Inside the section loop, from each grip's response:

```rust
                        if grip.drag_started() {
                            drag_started = Some(*sec);
                        }
                        if grip.drag_stopped() {
                            drag_released = true;
                        }
```

And after the loop, still inside the scroll area so the line paints over the sections:

```rust
                    // ⚠ Painted after every section, so the line is on top of them rather than
                    // under the next one.
                    if app.panel_drag.is_some() {
                        if let Some(y) = ui.ctx().pointer_interact_pos().map(|p| p.y) {
                            let at = rustroom_app::panel_order::drop_index(&tops, y);
                            drop_to = Some(at);
                            let line_y =
                                tops.get(at).copied().unwrap_or_else(|| ui.cursor().top());
                            ui.painter().hline(
                                ui.max_rect().x_range(),
                                line_y,
                                egui::Stroke::new(2.0, ui.visuals().selection.bg_fill),
                            );
                        }
                    }
```

- [ ] **Step 3: Apply on release**

After the panel closes:

```rust
    // ⚠ A drag begins on the frame a grip reports `drag_started`, and `app.panel_drag` is what
    // carries it across frames — a `Response` only knows about the frame it was built in.
    if let Some(sec) = drag_started {
        app.panel_drag = Some(sec);
    }
    if drag_released {
        if let (Some(from), Some(to)) = (app.panel_drag, drop_to) {
            let order = app.settings.panel_order();
            if let Some(i) = order.iter().position(|s| *s == from) {
                let next = rustroom_app::panel_order::moved(&order, i, to);
                if next != order {
                    app.settings.panel_order = next;
                    // ⚠ Written when the drag **settles**, not per frame — the
                    // `apply_view_demosaic` rule, which `CLAUDE.md` cites more than a dozen times.
                    app.save_settings();
                }
            }
        }
        // ⚠ Cleared whatever happened, including a release with no drop position: a stuck
        // `panel_drag` would leave the insertion line painted forever.
        app.panel_drag = None;
    }
```

- [ ] **Step 4: Build, test, lint**

```bash
cargo build --workspace --all-targets 2>&1 | tail -20
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings; echo "clippy exit: $?"
cargo test --all 2>&1 | grep "test result:" | awk -F'[ ;]' '{p+=$4; f+=$6} END {print "passed="p, "failed="f}'
```

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(panel): drag a section's grip to reorder it"
```

---

### Task 5: Look at it

⚠⚠ **The acceptance test.** The grip's appearance, the insertion line and the feel of the drag are
exactly what the headless harness cannot see.

- [ ] **Step 1: Launch**

```bash
cargo run --release -p app 2>&1 | tail -5 &
```

- [ ] **Step 2: The thing that was asked for**

Open a folder. **Drag Folders below Collections.** That is the request, in its own words.

- [ ] **Step 3: Collapsing still works**

Click each of the five headers. They must still collapse and expand. ⚠ If a click is being
swallowed, the grip's rect is overlapping the header's — the failure mode facts 1 and 2 predict.

- [ ] **Step 4: Nothing inside a section broke**

With Collections expanded, click a collection, the `+`, and the `−`. With Keywords expanded, type
a name and press Enter. **Every one must still work** — this is what wrapping a section in a drag
source would have broken, and the check that it was not.

- [ ] **Step 5: The insertion line**

Drag a grip slowly down the panel. A line must show where it will land, and it must track the
pointer rather than lag a section behind.

- [ ] **Step 6: It persists**

Reorder, quit, relaunch. The order must come back. Then check the file:

```bash
python3 -c "import json,os;print(json.load(open(os.path.expanduser('~/Library/Application Support/rustroom/settings.json'))).get('panel_order'))"
```

- [ ] **Step 7: The repair**

Hand-edit `panel_order` in that file to `["Dates"]`, relaunch, and confirm **all five** sections
are present with Dates first. Then set it to `[]` and confirm the default order returns.

- [ ] **Step 8: Docs**

Add a section to `CLAUDE.md`'s "Current status" in the house style — the three egui facts, the ⚠
findings, the test count, the accepted caveats. Remove the **Rearranging the Library panel's
sections** entry from `docs/BACKLOG.md`. Update the `▶ NEXT ACTION`.

```bash
git add -A && git commit -m "docs: panel section reordering shipped"
```

⚠ **Do not push and do not merge.** Report the branch state.
