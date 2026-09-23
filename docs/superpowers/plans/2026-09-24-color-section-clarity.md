# The Color section, made legible — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task.

**Goal:** A newcomer opening Color meets two sliders, not forty values; and no two sliders in it share a name.

**Architecture:** The `Color` edit group and every control stay exactly as they are. HSL and Color Grading move inside one `ADVANCED COLOR` collapsible (closed by default, not resettable) with plain-language sub-labels; the grading wheel's `Saturation` becomes `Strength`.

**Spec:** `docs/superpowers/specs/2026-09-24-color-section-clarity-design.md`. Read it first.

---

## Conventions

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

- **No `Co-Authored-By`. Never push.** Branch `color-section-clarity`.
- ⚠⚠ `cargo test --workspace <name-filter>`, **never `-p`**. A filter matching nothing **exits 0 having run zero tests** — check the passed count. (`folder_display` matched zero on 2026-09-23.)
- **Read `$?` from its own command.**
- ⚠⚠ **After any scripted edit:** `git diff <commit> -- <file> | grep '^-' | grep '///'` must be empty unless intended.
- ⚠ **Labels are American**: "Color", never "Colour". Comments may use British.
- ⚠ `sub_label` **uppercases** its text, so labels are written in sentence case and render as caps.

## File structure

| File | Change |
|---|---|
| `crates/app/src/test_ui.rs` | `slider_labels` helper |
| `crates/app/src/theme.rs` | `sub_label_hover` |
| `crates/app/src/develop/panel/color_wheel.rs` or `adjustments.rs` | `Saturation` → `Strength` |
| `crates/app/src/develop/panel/adjustments.rs` | the disclosure and the two sub-labels |

---

### Task 1: A guard that cannot go stale

**Files:** `crates/app/src/test_ui.rs`

- [ ] **Step 1: The helper**

```rust
/// The label of every widget that also reports a number — in practice every `Slider`.
///
/// ⚠⚠ **A role filter, not a list**, which is the whole point: it answers "what does this panel call
/// its sliders" for whatever sliders exist, so a control added tomorrow is included without anybody
/// remembering to add it. The enumerating guards this project keeps being bitten by are the
/// alternative.
///
/// ⚠ A `DragValue` has a numeric value and **no** label, so it is absent — see [`numeric_values`].
pub fn slider_labels(ctx: &egui::Context, body: impl FnMut(&mut egui::Ui)) -> Vec<String> {
    let out = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(screen()),
            ..Default::default()
        },
        body,
    );
    let Some(update) = out.platform_output.accesskit_update else {
        return Vec::new();
    };
    update
        .nodes
        .iter()
        .filter(|(_, n)| n.numeric_value().is_some())
        .filter_map(|(_, n)| n.label().map(str::to_owned))
        .collect()
}
```

- [ ] **Step 2: The failing test**

In `adjustments.rs`'s tests. ⚠ It must call the **whole** `show`, so it sees the section as shipped;
check `show`'s real signature and build `SourceInfo`/`GradeUi` the way the existing
`the_sessions_baseline_reaches_the_auto_solve` test does, or drive `color`'s body directly if `show`
needs a session.

```rust
    /// ⚠⚠ **The review, as a test.** An amateur found this section confusing, and a concrete cause was
    /// that **"Saturation" named three different things in it**: the global slider, HSL's channel and
    /// the grading wheel's slider. `hsl_bands.rs` had already recorded hitting the same hazard.
    ///
    /// ⚠ A role filter over whatever sliders exist — **not a list of expected labels**. A second
    /// "Saturation" added tomorrow fails this with nothing to maintain.
    #[test]
    fn no_two_sliders_in_the_color_section_share_a_name() {
        let ctx = crate::test_ui::ctx();
        let mut edits = edit_model::Edits::default();
        // ⚠ Open the disclosure, or the test only ever sees Vibrance and Saturation and would pass
        // against the very collision it exists to catch.
        let labels = crate::test_ui::slider_labels(&ctx, |ui| {
            /* draw the Color section with ADVANCED COLOR forced open */
        });
        let mut seen = std::collections::HashMap::<&str, usize>::new();
        for l in &labels {
            *seen.entry(l.as_str()).or_default() += 1;
        }
        let dupes: Vec<_> = seen.iter().filter(|(_, &n)| n > 1).map(|(l, _)| *l).collect();
        assert!(
            dupes.is_empty(),
            "these slider names appear more than once: {dupes:?} (all: {labels:?})"
        );
        assert!(
            labels.iter().any(|l| l == "Strength"),
            "the grading wheel's amount should read Strength: {labels:?}"
        );
    }
```

⚠⚠ **Forcing the disclosure open matters more than it looks.** With it closed the test sees two
sliders, passes, and guards nothing. If forcing it open is awkward, set its `CollapsingState` via
`ui.ctx()` with the same id the code uses, and **assert the label list has more than a dozen entries**
so a silently-closed disclosure fails rather than passes.

- [ ] **Step 3: Run it — expect a failure naming `Saturation`.**

```bash
cargo test --workspace no_two_sliders > /tmp/t.log 2>&1; echo "EXIT: $?"; grep -E 'appear more than once|panicked' /tmp/t.log | head
```

⚠ If it passes at this point, the test is not seeing the section. Fix the test before the code.

- [ ] **Step 4: Commit the failing guard** — on its own, so the diff shows the defect existed.

---

### Task 2: Strength, and the hover helper

**Files:** `crates/app/src/theme.rs`, and wherever the grading sliders are labelled

- [ ] **Step 1: `sub_label_hover`**

Beside `sub_label`, sharing its body rather than copying it:

```rust
/// [`sub_label`] with a hover.
///
/// ⚠ The plain-language name is what a beginner can act on; the jargon it replaces — `HSL`,
/// `color grading` — is what an experienced user searches for, so it goes here. The idiom Settings
/// already uses for its descriptions.
pub fn sub_label_hover(ui: &mut egui::Ui, text: &str, hover: &str) {
    // ⚠ Same layout as `sub_label`; only the response is kept, so the two cannot drift apart.
    …
}
```

⚠ Refactor `sub_label` to return its `Response` and have both go through one body, **or** have
`sub_label_hover` call `sub_label` and attach the hover to `ui.response()`. Do not duplicate the
layout.

- [ ] **Step 2: The rename**

`Saturation` → `Strength` on the grading wheel's slider only, with:

```rust
            // ⚠⚠ `Strength`, not `Saturation`: the global Saturation slider and HSL's Saturation
            // channel both mean **the image's chroma**, while this means **how much tint is applied**
            // — the same word for the opposite subject, and a named cause of the "the Color section is
            // confusing" review (2026-09-24).
            //
            // ⚠ `Hue` and `Luminance` are deliberately left alone: they repeat by word but not by
            // meaning, and Lightroom labels its wheels the same way.
```

⚠ Check whether any hover text, preset name or test string says "Saturation" for this control and
update it too — `grep -rn 'Saturation' crates/app/src` before and after.

- [ ] **Step 3: Run the guard — the duplicate should be gone but the test may still fail on `Strength`'s absence until the rename lands. Then run the whole suite.**

- [ ] **Step 4: Commit**

---

### Task 3: The disclosure

**Files:** `crates/app/src/develop/panel/adjustments.rs`

- [ ] **Step 1: Wrap HSL and Grading**

Inside `section(ui, SECTIONS[2], …)`, after the two sliders and the separator:

```rust
        // ⚠⚠ **Closed by default, and that is the change.** A newcomer opening Color met ~40 values
        // behind 15 sliders, two selectors and a wheel; they now meet two sliders, with the rest one
        // click away behind a name that says what it is for (user's call, 2026-09-24, after a review
        // by an amateur photographer).
        //
        // ⚠ `resettable(false)`: only the section's own header resets the `Color` group, or two reset
        // affordances would claim to reset different things.
        //
        // ⚠⚠ **Not a `Section`.** `Section::name()` is `EDIT_GROUP_NAMES[i]` and `edit_groups()` is
        // derived from `SECTIONS`, so a real section here would claim an edit group that does not
        // exist and put the panel and the Sync dialog out of step — `hsl_bands.rs` and this file both
        // already warn about it. Splitting the `Color` group is its own change, with a version bump.
        let mut adv = super::section::header(
            ui,
            ui.make_persistent_id("color_advanced"),
            false,
            super::section::Header::new("Advanced color"),
        );
        adv.state.show_body_unindented(ui, |ui| { … });
```

with the body holding, in order:

```rust
            theme::sub_label_hover(
                ui,
                "Adjust individual colors",
                "HSL — shift the hue, saturation or brightness of one color at a time, leaving the \
                 others alone.",
            );
            hsl_bands::show(ui, &mut edits.hsl, hsl_channel, ed);
            // … then the existing Color Grading block, its sub-label becoming:
            theme::sub_label_with(ui, "Tint by tone", |ui| { /* Show range, unchanged */ });
```

⚠ `sub_label_with` has no hover parameter; if "Tint by tone" should also hover *"Color grading — …"*,
either extend it or accept the hover on the checkbox only. **Decide and say which** — do not leave it
half-done.

⚠ The `edits`/`ed` borrows: the existing closure already threads them, so the new nesting must not
capture `edits` twice. If it fights, bind what the body needs before the closure, the way
`color_wheel::show`'s scope already does.

- [ ] **Step 2: The other three tests from the spec**

- `ADVANCED COLOR` is closed by default and Vibrance/Saturation are still reachable — assert
  `slider_labels` with the disclosure untouched contains `Vibrance` and **not** `Blending`.
- `edit_groups()` still returns exactly the six sections' groups (⚠ that the new collapsible did not
  change what "Reset adjustments" touches).
- `sub_label_hover` uppercases like `sub_label`.

- [ ] **Step 3: Gate**

```bash
cargo fmt --all
cargo fmt --all -- --check > /tmp/f.log 2>&1; echo "FMT: $?"
CARGO_TARGET_DIR=target/clippy cargo clippy --workspace --all-targets -- -D warnings > /tmp/c.log 2>&1; echo "CLIPPY: $?"
cargo test --workspace > /tmp/t.log 2>&1; echo "TEST: $?"
```

Expected: all **0**, **74 goldens unmoved** — no pipeline stage is touched, and **no `Edits` field
changes**, so no sidecar or golden movement at all.

- [ ] **Step 4: Commit**

---

### Task 4: Look at it

```bash
cargo run --release -p app
```

1. Develop → **Color**: two sliders and `ADVANCED COLOR`, closed.
2. Open it: `ADJUST INDIVIDUAL COLORS` then `TINT BY TONE`, hovers explaining HSL and color grading.
3. The grading wheel's amount reads **Strength**.
4. ⚠ The section's **reset** still resets the whole Color group, including what is hidden.
5. ⚠ **Sync** still shows one `Color` checkbox and still syncs HSL and grading with it.
6. ⚠ A **mask's** own HSL block is shared code (`hsl_bands`) — check it is unchanged.
7. The disclosure's open/closed state survives switching photos and restarting.

Then the journal and `CLAUDE.md` (**409** lines; something moves out, not in).

## Self-review

| Spec requirement | Task |
|---|---|
| Duplicate-slider-name guard, role-filtered not enumerated | 1 |
| Disclosure forced open in the test, or a size assertion | 1 |
| `Saturation` → `Strength`, `Hue`/`Luminance` untouched | 2 |
| `sub_label_hover`, no duplicated layout | 2 |
| Two plain-language blocks, jargon in hovers | 3 |
| Closed by default; not resettable; not a `Section` | 3 |
| `edit_groups()` unchanged | 3 |
| Color edit group **not** split | — |
| No `Edits` change ⇒ no golden or sidecar movement | 3 |
| Seen in use | 4 |

**Soft spots, flagged:**
- **A residual collision remains**, and it is accepted: HSL's channel selector still offers a
  **`Saturation`** *tab* while the global **`Saturation`** *slider* is visible above it when the
  disclosure is open. Every editor names that channel Saturation, so renaming would cost recognition;
  the test is scoped to **sliders**, which is where the misleading pair was. ⚠ If the reviewer still
  trips on it, the answer is a block name, not a rename.
- **`adjustments.rs` is ~600 lines** and gains a nesting level. Report its size after.
