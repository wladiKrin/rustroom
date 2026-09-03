# Geometry-mode navigation: Implementation Plan

> Executed inline on branch `geometry-navigation`. Spec:
> `docs/superpowers/specs/2026-09-03-geometry-navigation-design.md`.

**Goal:** Pan and zoom under the Geometry tool with the crop rectangle, a shared loupe on the
straighten line, and a straighten line that clears the perspective guides instead of being gated by
them.

**Order: smallest risk first.** Task 3 touches coordinate handling that has worked since M1.4b, so it
lands last, behind two commits that can be judged on their own.

**Baseline: 1845 tests, 53 goldens.** No golden may move — nothing here is in `pipeline`.

---

## Task 1: a straighten line clears the guides; the gate goes

**Files:** `develop/canvas/straighten_overlay.rs`, `develop/panel/geometry.rs`

- [ ] **Step 1: Write the failing test** in `panel/geometry.rs`'s `mod tests`

```rust
    /// ⚠⚠ Drawing a straighten line clears the guides, because it writes `geometry.angle` — one of
    /// the three values a guide solve owns — so the guides would otherwise describe a solve that no
    /// longer holds. The same rule editing the Straighten slider by hand already follows.
    ///
    /// ⚠ Asserts **both halves**: the guides go and the perspective sliders stay. A version that
    /// cleared the whole solve would pass a guides-only assertion, and `Clear`'s own contract is
    /// "the sliders they produced are left alone."
    #[test]
    fn a_straighten_line_clears_the_guides_and_leaves_the_sliders_alone() {
        let mut s = session();
        s.edits.perspective.vertical = 30.0;
        s.guides.push(pipeline::perspective::guides::Guide {
            from: [0.2, 0.2],
            to: [0.2, 0.8],
        });
        s.guides.push(pipeline::perspective::guides::Guide {
            from: [0.8, 0.2],
            to: [0.8, 0.8],
        });
        crate::develop::canvas::straighten_overlay::settle_for_test(&mut s, 4.0);
        assert!(s.guides.is_empty(), "the guides survived a straighten line");
        assert_eq!(s.active_guide, None);
        assert_eq!(
            s.edits.perspective.vertical, 30.0,
            "the straighten line cleared the perspective solve too"
        );
        assert_eq!(s.edits.geometry.angle, 4.0);
    }
```

⚠ That needs a seam. Rather than a test-only shim, factor the release path out of
`draw_straighten_overlay` into `pub(super) fn settle(s: &mut Session, angle: f32, tool: Tool) -> bool`
and call it from both. One rule, one place — the reason `apply_cell_action` and `name_edit` exist.

- [ ] **Step 2: Run it, expect FAIL** (`settle_for_test` undefined).

- [ ] **Step 3: Implement.** In `straighten_overlay.rs`, inside the `drag_stopped` block, after the
  angle is applied:

```rust
                // ⚠⚠ **A straighten line is a hand edit through another affordance, so it clears
                // the guides** — the rule `panel/geometry.rs` already states one line above its own
                // gate: "editing by hand throws the guides away, because the solve no longer
                // describes what you see." Straighten is one of the three values a solve writes.
                //
                // ⚠ The *sliders* are deliberately left alone, which is `Clear`'s contract: "the
                // sliders they produced are left alone." Clearing them here would discard a
                // correction the user never asked to undo.
                //
                // ⚠ The panel's own hand-edit check cannot do this for us: it captures
                // `solved_before` earlier in the same frame, so an angle written by the canvas on
                // the *previous* frame is already in `s.edits` and does not read as an edit.
                s.guides.clear();
                s.active_guide = None;
```

- [ ] **Step 4: Drop the gate** in `panel/geometry.rs` — delete `can_straighten`, the `add_enabled`
  wrapper and the `on_disabled_hover_text`, leaving a plain always-enabled button.

- [ ] **Step 5: Delete the two tests that pinned the gate** —
  `the_straighten_line_toggle_is_disabled_while_guides_are_in_force` and its twin — and say so in the
  commit. ⚠ They asserted the behaviour being removed; leaving them inverted would be a test of
  nothing.

- [ ] **Step 6: Sabotage.** Remove `s.guides.clear()` → the new test fails. Also clear
  `s.edits.perspective` → the same test fails on the slider assertion. Revert both.

- [ ] **Step 7:** `cargo fmt`, `clippy -D warnings`, `test --all`, commit.

---

## Task 2: one loupe, shared

**Files:** new `develop/canvas/loupe.rs`; `guide_overlay.rs`; `straighten_overlay.rs`;
`canvas/mod.rs`

- [ ] **Step 1: Move, do not copy.** Cut `LOUPE_SIZE`, `LOUPE_MAG` and `fn draw_loupe` out of
  `guide_overlay.rs` into `develop/canvas/loupe.rs`, changing `fn draw_loupe` to
  `pub(super) fn draw_loupe`. Add `mod loupe;` to `canvas/mod.rs` and
  `use super::loupe::draw_loupe;` to `guide_overlay.rs`.

⚠ **Its `img_rect` sampling survives unchanged** — its own comment explains why, and that reasoning
is about where the *texture* is, so Task 3 does not touch it.

⚠ Add to the module doc: *"Shared by the guide and straighten overlays. `test_support.rs` states the
rule — a helper duplicated per overlay is how the two would drift."*

- [ ] **Step 2: Run the suite.** The guide overlay's existing coverage is the evidence the move
  changed nothing. Test count must be **unchanged**.

- [ ] **Step 3: Commit the pure move on its own**, so the next commit's diff is only the new caller.

- [ ] **Step 4: Write the failing test** in `canvas/mod.rs`'s `mod tests`:

```rust
    /// ⚠ The loupe is drawn at the live end of a straighten drag and not after release — the
    /// guide overlay's rule, for its reason: a loupe pinned up permanently is chrome, and it
    /// covers the photograph it exists to help you see.
    #[test]
    fn the_straighten_loupe_shows_while_dragging_and_not_after() {
        let ctx = egui::Context::default();
        let mut app = crop_app(&ctx);
        let centre = super::test_support::canvas_centre(&mut app, &ctx);
        app.session.as_mut().expect("a session").straighten_armed = true;
        let during = super::test_support::shapes_during_drag(
            &mut app,
            &ctx,
            centre - egui::vec2(60.0, 20.0),
            centre + egui::vec2(60.0, 20.0),
        );
        let after = super::test_support::shape_count(&mut app, &ctx);
        assert!(
            during > after,
            "no loupe during the drag: {during} shapes dragging, {after} after"
        );
    }
```

⚠ If counting shapes proves too blunt to discriminate, assert on the **image** shape specifically —
the loupe is the only `Shape::Mesh`/`painter.image` in that overlay. **Do not weaken it to "the
counts differ"**; a test that cannot name what it saw is the failure mode this project records
sixteen times.

- [ ] **Step 5: Implement.** In `straighten_overlay.rs`, after the line is drawn, while a drag is
  live:

```rust
    // ⚠ At the **live end** of the line — the point being placed — and only while dragging. The
    // guide overlay shows it for the endpoint being dragged and for the same reason: precision
    // where you are aiming, nothing in the way once you have let go.
    if let (Some((_, to)), Some(tex)) = (s.straighten_drag, s.texture.as_ref()) {
        draw_loupe(&p, tex, to_screen(to), img_rect, frame_rect);
    }
```

- [ ] **Step 6: Sabotage.** Draw the loupe unconditionally → the test fails (it would be present
  after release too). Remove the call → fails. Revert.

- [ ] **Step 7:** gates, commit.

---

## Task 3: pan and zoom with the crop rectangle

⚠⚠ **M3.1a's three-headed bug, and only one head was visible then.** All three are invisible at fit
and wrong at zoom, so **every sabotage here must be run zoomed** or it proves nothing — which is
exactly why track D's `frame_rect` sabotage came back inert in W4.

**Files:** `crop_overlay.rs`, `canvas/mod.rs`

- [ ] **Step 1: Write the two failing tests** in `crop_overlay.rs`'s `mod tests`:

```rust
    /// ⚠⚠ **Head 3 of M3.1a's bug, and it needs ZOOM to be visible.** At fit, `img_rect` and
    /// `frame_rect` are bit-identical, so a drag test at fit passes against either. Zoomed, a
    /// delta divided by the region's width moves the handle by the wrong amount — recorded there
    /// as "a handle dragged while zoomed moved by the wrong amount", which "would have surfaced
    /// later as 'handles feel wrong when zoomed' with no obvious cause."
    #[test]
    fn a_zoomed_crop_drag_moves_the_crop_by_what_the_pointer_did() { /* see below */ }

    /// ⚠ Panning must not move the crop. Panning changes `frame_rect`'s origin, so if placement or
    /// the hit-test is still `img_rect`-based the crop appears to slide — the original
    /// mask-overlay symptom.
    #[test]
    fn panning_does_not_move_the_crop() { /* see below */ }
```

Both set `s.zoom` above 1.0 before driving `show`, so a region render is requested.

- [ ] **Step 2: Run, expect FAIL.**

- [ ] **Step 3: Implement — four coordinates move, two stay.**

`crop_screen_rect(s, frame_rect)`; hit-test normalisation and drag deltas against `frame_rect`;
`rects_around(img_rect, ..)` and both `painter_at` clips **unchanged**. `draw_crop_overlay` gains a
`frame_rect` parameter. Update `draw_guides`'s aspect comment, which currently justifies itself by
naming `img_rect`.

- [ ] **Step 4: Delete both gates** in `canvas/mod.rs`:

```rust
    // was: if tool.is_geometry() && !s.show_guides { fit_cap } else { … }
    let rung = view_region::tier_max_dim(fit_cap, s.buffer_long, fit, s.zoom);
```

```rust
    // was: if !tool.is_geometry() || s.show_guides { …zoom and pan… }
```

⚠ Both call sites — line 238 and line 318 — pass `frame_rect`.

- [ ] **Step 5: ⚠ The four existing crop tests must pass VERBATIM.** They are the evidence the
  fit-resolution path did not move, and that matters more than the new tests. If one needs editing,
  stop and work out why before changing it.

- [ ] **Step 6: Sabotage, ALL AT ZOOM.** Revert each of the three coordinates to `img_rect` in turn;
  each must fail a named test. ⚠ If one does not, the test is not zoomed enough to see it — fix the
  test, not the tolerance.

- [ ] **Step 7:** gates, full suite, goldens, commit.

---

## Task 4: verify

- [ ] `cargo fmt --check`, `clippy --all-targets --all-features -D warnings`, `test --all`, each exit
  code read from a file.
- [ ] `git diff --stat main -- crates/pipeline/tests` empty; 53 goldens.
- [ ] Test count: 1845 minus the 2 gate tests, plus the new ones. State the arithmetic.
- [ ] Launch the GUI and hand the user a checklist — I cannot drive it (`AXIsProcessTrusted()` is
  false).
