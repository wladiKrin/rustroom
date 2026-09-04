# W5 track B — soft proofing: Implementation Plan

> **For the agent executing this:** use `superpowers:executing-plans`, task by task. Spec:
> `docs/superpowers/specs/2026-09-04-soft-proofing-design.md`. **Every decision in that spec is
> settled — none is yours to re-open.** If you meet a question it does not answer,
> `PARALLEL-WORK.md` §6 applies: **stop and report**, do not choose.

**Goal:** A View-menu toggle renders the canvas through a chosen output space, with an optional
overlay marking the pixels that space cannot hold.

**Architecture:** `RenderRequest.display_space` already exists and `pack` already converts into it —
soft proofing points that field at the export target instead of the monitor. The warning is painted
in `pack`, after the histogram.

**Tech Stack:** Rust; `crates/color`, `crates/app` (lib + bin). **No `pipeline`, no `edit-model`.**

**Baseline:** `main` at **1896 tests, 53 goldens**, all gates green.

---

## ⚠⚠ Surfaces this track OWNS, and surfaces it must not touch

**Owns, exclusively this wave:** `render_engine::pack`, `Session` view state, `commands.rs`,
`menu.rs`, the `DialogSize` array, `Settings`.

**Barred — another track owns these:**
- `Edits` **entirely**, and its five exhaustive sites → **track A**
- `pipeline/src/lib.rs`, stage order, any stage → **track A**
- `Mask`, `MaskStage`, `develop/panel/masks.rs` → **track C**

⚠⚠ **This track writes NO `Edits` field and adds NO golden.** A proof is a property of the *view*,
exactly like `display_space` — it must not reach the sidecar, must not travel with a paste or a
preset, and must not appear in any golden. **If you find yourself editing `edit-model`, stop: you
are in another track's surface.**

---

## Build environment — every shell

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

**Conventions:** no `Co-Authored-By` trailer; **never push**; `fmt` + `clippy --all-targets
--all-features -- -D warnings` + `test --all` green before reporting done.

⚠ **Read every exit code from a file, never through a pipe** — `${PIPESTATUS[0]}` is bash and this
shell is zsh.

⚠⚠ **Sabotage discipline. Nine sabotages passed while proving nothing in the two sessions before
this wave, three of them the FIXTURE's fault rather than the guard's.** Every time: grep the edit
landed; confirm a `test result:` line appeared; confirm the **named** test failed; check the target
(`-p color --lib`, `-p app --bin Rustroom`); remember `cargo test` takes a **substring, not a
regex**. **If a sabotage does not fail, suspect the fixture first.**

---

## ⚠⚠ Read this before writing anything

`render_engine::pack` already:

1. runs the pipeline,
2. **quantises to sRGB and derives the histogram from that**,
3. *then* converts into `display_space`.

That ordering is not incidental — it is a bug this project shipped and caught by re-reading what
consumes the bytes:

> the histogram became display-referred anyway … **check what actually consumes the bytes, not just
> where the transform sits.**

`the_histogram_is_unaffected_by_the_display_space` is the guard. **It must stay green, and it must be
extended to the proof.**

---

## File Structure

| File | Responsibility |
| --- | --- |
| **Modify** `crates/color/src/output_space.rs` | `is_out_of_gamut` — pure, exact, no threshold. |
| **Modify** `crates/app/src/render_engine.rs` | Proof space + warning in `pack`, after the histogram. |
| **Modify** `crates/app/src/main.rs` (`Session`) | `soft_proof: Option<OutputSpace>`, `gamut_warning: bool`. |
| **Modify** `crates/app/src/commands.rs`, `menu.rs` | The View entries. |

---

## Task 1: the out-of-gamut test, in `color`

**Files:** `crates/color/src/output_space.rs`

- [ ] **Step 1: Write the failing tests**

```rust
    /// ⚠⚠ **The discriminating fixture is a colour in gamut for ONE space and out for another.** A
    /// colour flagged everywhere cannot tell a working test from one that ignores the space
    /// argument entirely — which is the shape of bug this whole function exists to avoid.
    #[test]
    fn a_saturated_green_is_inside_prophoto_and_outside_srgb() {
        // ProPhoto green, expressed in the working space.
        let g = [0.0f32, 1.0, 0.0];
        assert!(!is_out_of_gamut(g, OutputSpace::ProPhoto), "it is the working space");
        assert!(is_out_of_gamut(g, OutputSpace::Srgb), "sRGB cannot hold ProPhoto green");
    }

    /// ⚠ Both halves. A test that flags everything and one that flags nothing each pass a
    /// one-sided assertion.
    #[test]
    fn a_neutral_grey_is_in_gamut_everywhere() {
        for s in OutputSpace::ALL {
            assert!(!is_out_of_gamut([0.5, 0.5, 0.5], s), "grey flagged in {s:?}");
        }
    }

    /// ⚠ Exact, not a threshold: out of gamut means a channel below 0 or above 1 after conversion,
    /// and there is no tuning constant to get wrong.
    #[test]
    fn the_boundary_is_exact() {
        assert!(!is_out_of_gamut([1.0, 1.0, 1.0], OutputSpace::Srgb), "white is in gamut");
    }
```

- [ ] **Step 2: Run, expect a compile failure naming `is_out_of_gamut`.**

- [ ] **Step 3: Implement**

```rust
/// Whether this working-space colour falls outside `space`.
///
/// ⚠⚠ **Exact, not a threshold.** A colour is out of gamut when any channel lands below 0 or above
/// 1 after conversion into the target's primaries and *before* clipping. There is no tuning
/// constant here, and inventing one would be inventing a judgement.
///
/// ⚠ Measured before the transfer curve, on linear values: the curve is monotonic, so it cannot
/// bring a negative channel back into range, and applying it first would only obscure by how much.
pub fn is_out_of_gamut(rgb: [f32; 3], space: OutputSpace) -> bool {
    let c = /* the same matrix this module already uses to convert into `space` */;
    c.iter().any(|v| *v < 0.0 || *v > 1.0)
}
```

⚠ Reuse the matrix already in this module. A second conversion path is how the proof and the export
come to disagree about what is in gamut — which would be worse than no proof at all.

- [ ] **Step 4: Run — all three pass.**

- [ ] **Step 5: Sabotage**

| # | Sabotage | Must fail |
| --- | --- | --- |
| 1 | Ignore `space`, always test against ProPhoto | `a_saturated_green_is_inside_prophoto_and_outside_srgb` |
| 2 | `< 0.0 \|\| > 1.0` → `< 0.0` only | the same test |
| 3 | Return `true` always | `a_neutral_grey_is_in_gamut_everywhere` |

- [ ] **Step 6: Commit.**

---

## Task 2: the proof in `pack`

**Files:** `crates/app/src/render_engine.rs`

- [ ] **Step 1: Write the failing test**

```rust
    /// ⚠⚠ **The histogram describes the PHOTOGRAPH, never the view mode.** It is derived from
    /// `pack`'s sRGB quantisation, so a proof applied before that point silently makes it
    /// display-referred — which is the bug the display-profile work shipped and caught by
    /// re-reading what consumes the bytes.
    ///
    /// ⚠ The existing test covers `display_space` only; this extends it to the proof, which is a
    /// second transform at the same place and would leave that one passing.
    #[test]
    fn the_histogram_is_unaffected_by_the_soft_proof() {
        /* pack the same frame with soft_proof None and Some(OutputSpace::Srgb);
           assert_eq! on the two histograms, byte for byte */
    }

    /// ⚠ With the proof off, `pack` is bit-exactly what it was — so nothing about the ordinary path
    /// moved.
    #[test]
    fn the_proof_off_path_is_unchanged() { /* assert_eq! on the packed bytes */ }
```

- [ ] **Step 2: Run, expect failure.**

- [ ] **Step 3: Implement** — `pack` takes the proof space and the warning flag. The proof replaces
  the space it converts into; the warning is painted **after the histogram and before the display
  transform**, following Visualize Spots:

```rust
    // ⚠⚠ A view mode applied HERE — after the histogram, before the display transform — never a
    // pipeline stage. `visualize` established this rule and `display_space` established why the
    // histogram must come first.
```

⚠ The warning colour is a **fixed constant, not the theme accent**: the accent is user-switchable
including a `Neutral` setting whose whole point is to have no colour, which would defeat the warning
exactly when someone had chosen it. The crop overlay records this reasoning for its fixed red.

- [ ] **Step 4: Run — both pass, and the existing `display_space` test is still green.**

- [ ] **Step 5: Sabotage**

| # | Sabotage | Must fail |
| --- | --- | --- |
| 1 | Derive the histogram **after** the proof | `the_histogram_is_unaffected_by_the_soft_proof` |
| 2 | Paint the warning when the flag is false | `the_proof_off_path_is_unchanged` |
| 3 | Use the accent colour for the warning | the fixed-colour assertion |

- [ ] **Step 6: Commit.**

---

## Task 3: the view state and the menu

**Files:** `crates/app/src/main.rs` (`Session`), `commands.rs`, `menu.rs`

- [ ] **Step 1:** Add to `Session`, **at the end** of the struct:

```rust
    /// The output space the canvas is being proofed into, or `None`.
    ///
    /// ⚠ On `Session`, not `RustroomApp`: `Session::request` is the one funnel every render goes
    /// through and has no app access. That is why `display_space` lives here — and, as there, the
    /// cost is that it resets on a photo switch.
    soft_proof: Option<color::OutputSpace>,
    /// Whether out-of-gamut pixels are marked. A separate toggle: proofing without the warning is
    /// the common case, and a warning you cannot switch off is chrome.
    gamut_warning: bool,
```

⚠ `Session` has an exhaustive literal in `test_app.rs` — the compiler will name it. Append there too.

- [ ] **Step 2:** Add `Command::SoftProof(Option<OutputSpace>)` and `Command::GamutWarning`,
  **appended** to the enum, and View-menu entries built from `OutputSpace::DISPLAYS`.

⚠ The `Command` match is exhaustive and will refuse to compile until both are handled — that is what
it is for.

⚠ `every_shortcut_maps_to_an_egui_key` is a completeness test over the shortcut table. If a new entry
takes a chord, **complete the table, never weaken the test** — that rule is already recorded.

- [ ] **Step 3:** Changing either must re-render: `s.request(..)` with the new space, guarded on an
  actual change. ⚠ A menu body runs every frame it is open; an unguarded request is the
  `apply_view_demosaic` mistake, which this file records **eleven** times.

- [ ] **Step 4: Test** that both menu entries exist and dispatch, via the existing panel/menu
  harness. Commit.

---

## Task 4: verify

- [ ] `fmt --check`, `clippy --all-targets --all-features -D warnings`, `test --all` — **all three,
  each exit code from a file.**
- [ ] `git diff --stat main -- crates/pipeline/tests` **empty** — this track adds no golden — and 53
  must have **run**.
- [ ] `git diff --stat main -- crates/edit-model crates/pipeline/src` **empty**. ⚠ This is the
  wave-discipline check: any diff there means this track entered another's surface.
- [ ] State the test arithmetic.
- [ ] **The GUI is the user's**, not the agent's (`AXIsProcessTrusted()` is false). Hand over a
  checklist: proof P3 on a saturated photograph and see the canvas change; turn the warning on and
  see the pixels sRGB cannot hold; confirm the histogram does not move in any of the four
  combinations.
- [ ] Report any sabotage that did not fail, **named as such**.
