# Boolean mask operations — headless (M7.1c, plan 1 of 2)

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

> ⚠ **Conventions that are not negotiable:** commit messages carry **no `Co-Authored-By` trailer**; work stays on branch `m7.1c-boolean-masks`; **never push** — the user pushes to `github.com/wladiKrin/rustroom` himself.

**Goal:** A mask can be built from several shapes composited with Add / Subtract / Intersect, correct end to end through the pipeline, the coverage cache and the prefix key — with **no GUI at all**.

**Architecture:** `MaskedAdjustment` gains `refine: Vec<Refinement>`; `Mask` is untouched and stays flat. `MaskStage` resolves one `CoverageSource` per component at construction and folds them per pixel with the product algebra, sharing the expensive per-pixel facts. The brush-coverage cache goes from one index to `(mask, component)`.

**Tech Stack:** Rust workspace, `serde`, `rayon`. Spec: `docs/superpowers/specs/2026-09-09-boolean-mask-ops-design.md`.

**Why headless first:** `pipeline` has zero GUI dependency by rule and the CLI plus the goldens are the permanent test harness, so the whole model is proved before any canvas code exists — the split M3.1a, colour grading, lens corrections, virtual copies and snapshots each used. A canvas bug can then never be confused with a model bug. Plan 2 (`2026-09-09-boolean-masks-ui.md`) adds the panel and canvas.

---

## Environment

Every `cargo` command in this plan needs:

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
```

⚠ **Read every gate's exit code from its own file, never through a pipe.** This project has recorded at least three variants of a pipeline reporting the wrong status:

```bash
cargo clippy --all-targets --all-features -- -D warnings > /tmp/c.log 2>&1; echo "CLIPPY=$?"
```

⚠ **After any scripted edit, assert the new text is present.** A `sed` that matches nothing exits 0, and `cargo fmt` reflows conditions across lines, which is how three separate sabotages in this repo came back green while changing nothing.

## File structure

| file | responsibility |
|---|---|
| **create** `crates/edit-model/src/mask_ops.rs` | `MaskOp`, `Refinement`, the fold rule, their tests |
| modify `crates/edit-model/src/lib.rs` | `mod mask_ops`, the re-export, `MaskedAdjustment.refine` |
| **create** `crates/pipeline/src/stages/mask_coverage.rs` | `CoverageSource` (moved), `lightness`/`hue_chroma` (moved), `BaseFacts`, `Composite` |
| modify `crates/pipeline/src/stages/mask.rs` | shrinks by the move; holds a `Composite` instead of one source |
| modify `crates/pipeline/src/brush/raster.rs` | composites strokes through `MaskOp::combine` |
| modify `crates/pipeline/src/lib.rs` | `BrushCoverage` nested slots, `capture_projection` |
| modify `crates/app/src/render_engine.rs` | `MaskCoverageKey` gains the component index; cap; harvest loop |
| modify `crates/pipeline/tests/golden.rs` | three new goldens |
| modify `crates/app/examples/bench_masked.rs` | composite rows |

---

### Task 1: `MaskOp`, `Refinement` and the fold rule

**Files:**
- Create: `crates/edit-model/src/mask_ops.rs`
- Modify: `crates/edit-model/src/lib.rs` (add the module and re-export)

⚠ This lands **alone**, unlike a bin-crate item: `pub` in a library crate is public API, so `-D warnings` does not call it dead code. `crates/edit-model/src/lib.rs` is ~4,000 lines and past the ~600 marker, which is why the type goes in a new module and `lib.rs` gains two lines.

- [ ] **Step 1: Write the failing tests**

Create `crates/edit-model/src/mask_ops.rs`:

```rust
//! Boolean composition **between** masks: the three ops, the refinement that carries one, and
//! the single rule that folds them.
//!
//! ⚠⚠ **Two of the three rules are not new — they are the brush's own, lifted.** `Add` is
//! exactly `brush::raster`'s across-strokes paint step and `Subtract` is exactly its `erase`;
//! `Intersect` is the third member of that same product algebra. `min`/`max` were rejected for
//! that reason: they would make subtract-*between*-masks disagree with erase-*within*-a-brush at
//! every partial coverage — a second rule wearing the first one's name, which is the failure this
//! project already paid for with the Whites slider.

use serde::{Deserialize, Serialize};

use crate::Mask;

/// How a refinement composites against the coverage built so far.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MaskOp {
    Add,
    Subtract,
    Intersect,
}

impl MaskOp {
    /// Every op, in the order the panel offers them. ⚠ Beside the type so the UI cannot invent a
    /// second ordering — the rule `FlagSet::ALL` and `Tool::ALL` already follow.
    pub const ALL: [MaskOp; 3] = [MaskOp::Add, MaskOp::Subtract, MaskOp::Intersect];

    /// The op's own word, for a component row.
    pub fn label(self) -> &'static str {
        match self {
            MaskOp::Add => "Add",
            MaskOp::Subtract => "Subtract",
            MaskOp::Intersect => "Intersect",
        }
    }

    /// The mode chip's wording, which takes a preposition.
    ///
    /// ⚠ **Not the bare op name.** All five mask-kind buttons already begin with "Add", so a chip
    /// labelled `Add` sits directly above five other things called Add and says nothing about
    /// which is which — the ambiguity that turned two adjacent sliders both called "Scale" into
    /// "Lens scale". The preposition also makes the two-click gesture read as one sentence:
    /// *Subtract from … a radial filter*.
    pub fn chip_label(self) -> &'static str {
        match self {
            MaskOp::Add => "Add to",
            MaskOp::Subtract => "Subtract from",
            MaskOp::Intersect => "Intersect with",
        }
    }

    /// Composite coverage `b` onto the running coverage `a`. Both in `0.0..=1.0`.
    ///
    /// ⚠ `Add` is `brush/raster.rs`'s paint step and `Subtract` is its `erase`, and since this
    /// milestone the raster calls *this* function — so the two cannot drift.
    pub fn combine(self, a: f32, b: f32) -> f32 {
        match self {
            // Standard "over": adds a fraction of what is still uncovered, so repeated
            // application approaches full coverage without ever exceeding it.
            MaskOp::Add => a + (1.0 - a) * b,
            MaskOp::Subtract => a * (1.0 - b),
            MaskOp::Intersect => a * b,
        }
    }
}

/// One shape composited onto a mask's coverage.
///
/// ⚠ `op` is **required**, never defaulted. A refinement with no op has no defensible reading, and
/// guessing one silently changes the picture — so a malformed one fails the load, exactly as an
/// unknown [`Mask`] tag does.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Refinement {
    pub op: MaskOp,
    pub mask: Mask,
}

impl Refinement {
    /// The component row's name, e.g. `"Subtract Radial"`.
    pub fn label(&self) -> String {
        format!("{} {}", self.op.label(), self.mask.label())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_endpoints_are_what_the_ops_promise() {
        for op in MaskOp::ALL {
            // Nothing composited changes nothing.
            assert_eq!(op.combine(0.4, 0.0), if op == MaskOp::Intersect { 0.0 } else { 0.4 });
        }
        assert_eq!(MaskOp::Add.combine(0.0, 0.7), 0.7, "adding to nothing is the addend");
        assert_eq!(MaskOp::Subtract.combine(0.7, 1.0), 0.0, "subtracting all of it leaves none");
        assert_eq!(MaskOp::Intersect.combine(0.7, 1.0), 0.7, "intersecting everything is identity");
    }

    #[test]
    fn the_partial_case_is_the_product_algebra_not_min_max() {
        // ⚠⚠ The discriminating test. `min`/`max` agree with these rules at 0 and 1 and differ
        // everywhere in between, so only a partial case can tell the two algebras apart.
        assert!((MaskOp::Add.combine(0.5, 0.5) - 0.75).abs() < 1e-6);
        assert!((MaskOp::Intersect.combine(0.5, 0.5) - 0.25).abs() < 1e-6);
        assert!((MaskOp::Subtract.combine(0.5, 0.5) - 0.25).abs() < 1e-6);
    }

    #[test]
    fn every_op_stays_inside_the_unit_range() {
        let probes = [0.0, 0.01, 0.37, 0.5, 0.99, 1.0];
        for op in MaskOp::ALL {
            for a in probes {
                for b in probes {
                    let c = op.combine(a, b);
                    assert!((0.0..=1.0).contains(&c), "{op:?} {a} {b} -> {c}");
                }
            }
        }
    }

    #[test]
    fn the_fold_respects_list_order() {
        // ⚠ Order is observable, and the panel is append-only because of it. Subtract-then-
        // intersect is not intersect-then-subtract.
        let a = MaskOp::Intersect.combine(MaskOp::Subtract.combine(1.0, 0.5), 0.5);
        let b = MaskOp::Subtract.combine(MaskOp::Intersect.combine(1.0, 0.5), 0.5);
        assert!((a - 0.25).abs() < 1e-6);
        assert!((b - 0.25).abs() < 1e-6);
        // Same here by coincidence of the numbers; pick a pair that separates them.
        let c = MaskOp::Add.combine(MaskOp::Intersect.combine(0.5, 0.5), 0.5);
        let d = MaskOp::Intersect.combine(MaskOp::Add.combine(0.5, 0.5), 0.5);
        assert!((c - 0.625).abs() < 1e-6, "add after intersect: {c}");
        assert!((d - 0.375).abs() < 1e-6, "intersect after add: {d}");
        assert!((c - d).abs() > 0.2, "the two orders must differ");
    }

    #[test]
    fn a_refinement_names_its_op_and_its_kind() {
        let r = Refinement {
            op: MaskOp::Subtract,
            mask: Mask::Radial(crate::RadialGradient::default()),
        };
        assert_eq!(r.label(), "Subtract Radial");
    }

    #[test]
    fn a_refinement_round_trips_through_json() {
        let r = Refinement {
            op: MaskOp::Intersect,
            mask: Mask::Luminance(crate::LuminanceRange::default()),
        };
        let s = serde_json::to_string(&r).unwrap();
        assert_eq!(serde_json::from_str::<Refinement>(&s).unwrap(), r);
    }

    #[test]
    fn a_refinement_without_an_op_is_refused() {
        // ⚠ Required, not defaulted: guessing an op silently changes the picture.
        let json = r#"{"mask":{"Radial":{"center":[0.5,0.5],"radius":[0.25,0.25],"feather":50.0,"invert":false}}}"#;
        assert!(serde_json::from_str::<Refinement>(json).is_err());
    }
}
```

- [ ] **Step 2: Wire the module in**

In `crates/edit-model/src/lib.rs`, immediately after the existing `use serde::...` imports near the top of the file, add:

```rust
mod mask_ops;
pub use mask_ops::{MaskOp, Refinement};
```

- [ ] **Step 3: Run the tests to verify they fail, then pass**

```bash
cargo test -p edit-model mask_ops 2>&1 | tail -20
```

Expected on the first run before the module exists: a compile error. After Steps 1–2: `test result: ok. 6 passed`.

⚠ Read the **passed count**, not the word `ok`. A filter that matches nothing prints `0 passed … ok`, which this project has been misled by four times.

- [ ] **Step 4: Sabotage — swap the product rules for `min`/`max`**

Temporarily change `combine` to `Add => a.max(b)`, `Intersect => a.min(b)`. Run the tests.

Expected: `the_partial_case_is_the_product_algebra_not_min_max` **fails**. Restore from a copy taken before the edit — ⚠ **not** with `git checkout <file>`, which has destroyed uncommitted work in this repo once already.

- [ ] **Step 5: Commit**

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings > /tmp/c.log 2>&1; echo "CLIPPY=$?"
git add crates/edit-model/src/mask_ops.rs crates/edit-model/src/lib.rs
git commit -m "The three mask ops, and the algebra the brush already used"
```

---

### Task 2: `MaskedAdjustment.refine`, and every literal that names it

**Files:**
- Modify: `crates/edit-model/src/lib.rs:1387-1392` (`MaskedAdjustment`)
- Modify: `crates/pipeline/src/lib.rs:562` (`capture_projection`)
- Modify: ~58 struct literals across `crates/pipeline`, `crates/edit-model`, `crates/app`

⚠⚠ **This is ONE commit across three crates, and it has to be.** A field's type change cannot land crate by crate — the workspace must build at every commit, which is the rule the "Decided off" milestone recorded when `Edits.rendering` became an `Option`.

- [ ] **Step 1: Write the failing sidecar tests**

Add to `crates/edit-model/src/lib.rs`'s test module:

```rust
#[test]
fn a_mask_with_no_refinements_writes_the_bytes_it_always_did() {
    // ⚠⚠ The `skip_serializing_if` guard, and the reason it is there: a photo that has never
    // used a composite must write the same sidecar it wrote yesterday. `lens_correction`'s rule.
    let m = MaskedAdjustment {
        mask: Mask::Linear(LinearGradient::default()),
        ..Default::default()
    };
    let s = serde_json::to_string(&m).unwrap();
    assert!(!s.contains("refine"), "an empty refinement list must be absent, not `[]`: {s}");
}

#[test]
fn a_sidecar_from_before_refinements_loads_with_none() {
    let json = r#"{"mask":{"Linear":{"from":[0.0,0.0],"to":[0.0,1.0],"invert":false}}}"#;
    let m: MaskedAdjustment = serde_json::from_str(json).unwrap();
    assert!(m.refine.is_empty());
}

#[test]
fn a_composite_round_trips() {
    let m = MaskedAdjustment {
        mask: Mask::Linear(LinearGradient::default()),
        refine: vec![
            Refinement { op: MaskOp::Subtract, mask: Mask::Radial(RadialGradient::default()) },
            Refinement { op: MaskOp::Intersect, mask: Mask::Luminance(LuminanceRange::default()) },
        ],
        ..Default::default()
    };
    let s = serde_json::to_string(&m).unwrap();
    assert_eq!(serde_json::from_str::<MaskedAdjustment>(&s).unwrap(), m);
}
```

- [ ] **Step 2: Add the field**

Replace `MaskedAdjustment` at `crates/edit-model/src/lib.rs:1385-1392`:

```rust
/// One mask and the adjustments it carries.
/// ⚠ Not `Copy`: [`Mask::Brush`] carries a stroke list. Still `Clone`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct MaskedAdjustment {
    /// The composite's **base** — the first shape, before any refinement.
    pub mask: Mask,
    /// Shapes composited onto `mask`, folded **left to right**.
    ///
    /// ⚠⚠ **A list, not a recursive [`Mask`] variant.** Keeping `Mask` flat is what leaves
    /// `analytic_coverage`'s exhaustive match, `label`, `invert_mut` and every sidecar on disk
    /// untouched; the cost is that a composite cannot contain a composite, which is the nesting
    /// the recursive variant would have bought.
    ///
    /// ⚠ `skip_serializing_if`: an empty list is **absent** from the sidecar, never `[]`, so a
    /// mask that has never been refined writes the bytes it always did — `lens_correction`'s rule.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refine: Vec<Refinement>,
    pub adjustments: LocalAdjustments,
}
```

- [ ] **Step 3: Build, and let the compiler enumerate the literals**

```bash
cargo build --workspace --all-targets 2>&1 | grep -c "^error\[E0063\]"
```

Expected: ~58 errors, every one a struct literal missing `refine`.

Fix each by adding `..Default::default()` **except** the three fixtures named in Step 4. ⚠ Do this with an editor, not a bulk regex: this repo has recorded a scripted edit that duplicated a whole region and another that landed on an inner brace. After the pass, re-run the build and confirm zero `E0063`.

- [ ] **Step 4: Keep the three exhaustive fixtures exhaustive — and make them carry a refinement**

⚠⚠ **This is the recorded lesson of the masked-spatial milestone applied in advance.** `pipeline::all_edits`, `edit_model::all_nonneutral` and `edit_clipboard`'s payload exist to force a decision about every new field — and last time three fields *"landed untested by the very fixtures that exist to force a decision"* because those fixtures used `..Default::default()` for a nested struct. Give all three an explicit, **non-empty** `refine`:

- `crates/pipeline/src/lib.rs:1395` (`all_edits`)
- `crates/edit-model/src/lib.rs:2758` (`all_nonneutral`)
- `crates/app/src/edit_clipboard.rs:151`

In each, inside the existing `MaskedAdjustment { … }`, add:

```rust
                refine: vec![edit_model::Refinement {
                    op: edit_model::MaskOp::Subtract,
                    mask: edit_model::Mask::Radial(edit_model::RadialGradient::default()),
                }],
```

(drop the `edit_model::` prefix in `edit-model`'s own fixture).

- [ ] **Step 5: `capture_projection` carries the refinements**

⚠⚠ **Without this, dragging a subtracted radial leaves a stale masked-NR checkpoint on screen.** The projection is the prefix step's cache key, so a composite's *shape* — which now includes its refinements — must be in it. Replace the body at `crates/pipeline/src/lib.rs:562`:

```rust
fn capture_projection(m: &edit_model::MaskedAdjustment) -> edit_model::MaskedAdjustment {
    edit_model::MaskedAdjustment {
        mask: m.mask.clone(),
        // ⚠⚠ **The refinements are part of the SHAPE and must be in the key.** Left out, a
        // subtracted or intersected component would change what the masked noise reduction and
        // dehaze cover while the cached prefix checkpoint stayed valid — a stale picture with
        // nothing on screen to announce it. The compiler forced this decision: this is a struct
        // literal with no `..`, so the new field is an `E0063` here.
        refine: m.refine.clone(),
        adjustments: edit_model::LocalAdjustments {
            noise_reduction: m.adjustments.noise_reduction,
            dehaze: m.adjustments.dehaze,
            ..Default::default()
        },
    }
}
```

- [ ] **Step 6: Extend the prefix-key test with a third direction**

In `crates/pipeline/src/lib.rs`, inside `the_prefix_key_follows_a_masks_capture_corrections_and_ignores_the_rest` (line ~1853), after the existing assertions add:

```rust
        // ⚠ The third direction, added with refinements: a change to the composite's SHAPE must
        // move the key. The other two directions cannot see it — a refinement is neither a
        // capture correction nor a tail adjustment.
        let mut refined = base.clone();
        refined.masks[0].refine.push(edit_model::Refinement {
            op: edit_model::MaskOp::Subtract,
            mask: edit_model::Mask::Radial(edit_model::RadialGradient::default()),
        });
        assert_ne!(
            PrefixKey::new(&base, scale, None, resampler),
            PrefixKey::new(&refined, scale, None, resampler),
            "a refinement changes what the masked capture correction covers, so it must move the key"
        );
```

⚠ Adapt `base`, `scale` and `resampler` to the names the existing test already uses — read the test before editing it.

- [ ] **Step 7: Run the gates**

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings > /tmp/c.log 2>&1; echo "CLIPPY=$?"
cargo test --workspace > /tmp/t.log 2>&1; echo "TEST=$?"; grep -c "^test result: ok" /tmp/t.log
```

Expected: `CLIPPY=0`, `TEST=0`. ⚠ **All 60 `golden_*` tests must still pass byte-identically** — this task changes no pixel path:

```bash
grep -c "^test golden_.* ok$" /tmp/t.log
```

Expected: `60`.

- [ ] **Step 8: Sabotage — drop `refine` from the projection**

Change Step 5's `refine: m.refine.clone()` to `refine: Vec::new()`. Run:

```bash
cargo test -p pipeline the_prefix_key_follows 2>&1 | tail -5
```

Expected: **FAIL** on the new assertion. Restore from a copy.

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "A mask carries refinements, and the prefix key carries them too"
```

---

### Task 3: The brush composites strokes through `MaskOp::combine`

**Files:**
- Modify: `crates/pipeline/src/brush/raster.rs:366-372`

⚠ This is what makes §3's claim true rather than a comment: `Add` **is** the paint step and `Subtract` **is** `erase`, because the raster calls them.

- [ ] **Step 1: Replace the two inline expressions**

At `crates/pipeline/src/brush/raster.rs`, the stroke-composite block currently reads:

```rust
                if stroke.erase {
                    cov[idx] *= 1.0 - a;
                } else {
                    // Standard "over": a second pass adds a fraction of what is still uncovered,
                    // so repeated strokes approach full coverage without ever exceeding it.
                    cov[idx] += (1.0 - cov[idx]) * a;
                }
```

Replace with:

```rust
                // ⚠⚠ **The same two rules `MaskOp` offers between masks, called from here so the
                // two cannot drift.** `Subtract` is `c * (1 - a)` and `Add` is the standard
                // "over" — bit-identical to the expressions they replace, which is what the 60
                // byte-identical goldens prove. This is the whole argument for the product
                // algebra between masks: erase-within-a-brush and subtract-between-masks are now
                // one rule rather than two that agree only at the endpoints.
                let op = if stroke.erase {
                    edit_model::MaskOp::Subtract
                } else {
                    edit_model::MaskOp::Add
                };
                cov[idx] = op.combine(cov[idx], a);
```

- [ ] **Step 2: Run the goldens**

```bash
cargo test -p pipeline --test golden > /tmp/g.log 2>&1; echo "GOLDEN=$?"; grep -c "^test golden_.* ok$" /tmp/g.log
```

Expected: `GOLDEN=0` and `60`. ⚠ **Byte-identical is the acceptance criterion**, not "still green": `a += (1-a)*b` and `a + (1-a)*b` are the same f32 operations in the same order, so any golden that moved means the substitution was not equivalent.

- [ ] **Step 3: Run the brush raster tests**

```bash
cargo test -p pipeline brush:: 2>&1 | tail -5
```

Expected: all pass, unchanged.

- [ ] **Step 4: Commit**

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings > /tmp/c.log 2>&1; echo "CLIPPY=$?"
git add crates/pipeline/src/brush/raster.rs
git commit -m "The brush composites strokes through MaskOp, so the two rules are one"
```

---

### Task 4: Move the coverage machinery into its own module

**Files:**
- Create: `crates/pipeline/src/stages/mask_coverage.rs`
- Modify: `crates/pipeline/src/stages/mask.rs` (shrinks), `crates/pipeline/src/stages/mod.rs` (declare the module)

⚠ **A pure move, provable by unchanged tests.** `mask.rs` is 1071 lines and past the ~600 marker; moving `CoverageSource`, `lightness` and `hue_chroma` out makes it *smaller* before anything is added to it. Preparing the seam first is the sequencing that turned M3.1b's three predicted compile errors into one match arm.

- [ ] **Step 1: Create the module with the moved items**

Create `crates/pipeline/src/stages/mask_coverage.rs` and move into it, **verbatim including every doc comment**:

- `enum CoverageSource` (currently `mask.rs:17-55`) — change to `pub(super) enum CoverageSource`, and each variant's fields to `pub(super)` where `mask.rs` reads them.
- `fn lightness` (`mask.rs:66-68`) — `pub(super) fn`.
- `fn hue_chroma` (`mask.rs:84-87`) — `pub(super) fn`.

Add a module header:

```rust
//! Where a mask's coverage comes from, and how a composite's components are folded into one
//! number per pixel.
//!
//! ⚠ Split out of `stages/mask.rs`, which was 1071 lines before this milestone added to it. The
//! move is deliberately pure — every doc comment came across verbatim — so the unchanged tests in
//! `mask.rs` are the evidence it changed no behaviour.
```

⚠ **Diff the `⚠` comments before and after**, the rule W0 established: a move can silently drop a recorded finding and no test will notice.

```bash
git show HEAD:crates/pipeline/src/stages/mask.rs | grep "⚠" | sort > /tmp/before.txt
cat crates/pipeline/src/stages/mask.rs crates/pipeline/src/stages/mask_coverage.rs | grep "⚠" | sort > /tmp/after.txt
comm -23 /tmp/before.txt /tmp/after.txt
```

Expected: **no output** — nothing was dropped.

- [ ] **Step 2: Declare the module**

In `crates/pipeline/src/stages/mod.rs`, beside the existing `mod mask;`, add:

```rust
mod mask_coverage;
```

In `mask.rs`, add `use mask_coverage::{hue_chroma, lightness, CoverageSource};` (adjust to the crate's existing `use super::…` idiom — read the neighbouring stage modules first).

- [ ] **Step 3: Run the tests unchanged**

```bash
cargo test -p pipeline mask 2>&1 | tail -5
```

Expected: the same count as before the move, all passing, **with no test edited**. That is the evidence the move preserved behaviour.

- [ ] **Step 4: Commit**

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings > /tmp/c.log 2>&1; echo "CLIPPY=$?"
git add crates/pipeline/src/stages/
git commit -m "CoverageSource moves to its own module, ahead of composites"
```

---

### Task 5: `BaseFacts` and `Composite` — the shared per-pixel facts

**Files:**
- Modify: `crates/pipeline/src/stages/mask_coverage.rs`

⚠⚠ **This is the task nothing else can see.** Three colour-range components computing three Oklab round trips render **bit-identically** and only cost more, so no pixel test can distinguish the two — which is why the counter below is not optional decoration.

- [ ] **Step 1: Write the failing tests**

Append to `crates/pipeline/src/stages/mask_coverage.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use edit_model::{ColourRange, LinearGradient, LuminanceRange, Mask, MaskOp};

    fn analytic(m: Mask) -> CoverageSource {
        CoverageSource::Analytic(m)
    }

    #[test]
    fn a_composite_with_no_refinements_is_its_base() {
        let c = Composite {
            base: analytic(Mask::Linear(LinearGradient {
                from: [0.0, 0.0],
                to: [1.0, 0.0],
                invert: false,
            })),
            refine: Vec::new(),
        };
        let mut facts = BaseFacts::default();
        let got = c.at(&PixelCtx::analytic_only(0.5, 0.5, 1.0), [0.5; 3], &mut facts);
        assert!((got - 0.5).abs() < 1e-4, "got {got}");
    }

    #[test]
    fn a_subtracted_component_cuts_out_of_the_base() {
        // A base covering everything, minus a half-covering gradient.
        let c = Composite {
            base: analytic(Mask::Linear(LinearGradient {
                from: [-1.0, 0.0],
                to: [-0.5, 0.0],
                invert: false,
            })),
            refine: vec![(
                MaskOp::Subtract,
                analytic(Mask::Linear(LinearGradient {
                    from: [0.0, 0.0],
                    to: [1.0, 0.0],
                    invert: false,
                })),
            )],
        };
        let mut facts = BaseFacts::default();
        let got = c.at(&PixelCtx::analytic_only(0.5, 0.5, 1.0), [0.5; 3], &mut facts);
        assert!((got - 0.5).abs() < 1e-4, "1.0 minus 0.5 coverage is 0.5, got {got}");
    }

    #[test]
    fn three_colour_components_cost_one_oklab_round_trip() {
        // ⚠⚠ **The only test that can see the sharing.** A per-component round trip renders
        // bit-identically and merely costs three times as much, so nothing else in the suite
        // distinguishes them. `docs/BACKLOG.md` puts that round trip at 42% of a range mask's
        // coverage cost, so getting this wrong makes a composite's cost linear in a component
        // count that ought to be nearly free.
        let colour = || CoverageSource::Colour(ColourRange::default());
        let c = Composite {
            base: colour(),
            refine: vec![(MaskOp::Add, colour()), (MaskOp::Add, colour())],
        };
        let mut facts = BaseFacts::default();
        conversions().store(0, std::sync::atomic::Ordering::Relaxed);
        let _ = c.at(&PixelCtx::analytic_only(0.5, 0.5, 1.0), [0.3, 0.4, 0.5], &mut facts);
        assert_eq!(
            conversions().load(std::sync::atomic::Ordering::Relaxed),
            1,
            "three colour components must share one conversion"
        );
    }

    #[test]
    fn a_composite_of_only_analytic_shapes_converts_nothing() {
        let c = Composite {
            base: analytic(Mask::Linear(LinearGradient::default())),
            refine: vec![(MaskOp::Intersect, analytic(Mask::Linear(LinearGradient::default())))],
        };
        let mut facts = BaseFacts::default();
        conversions().store(0, std::sync::atomic::Ordering::Relaxed);
        let _ = c.at(&PixelCtx::analytic_only(0.5, 0.5, 1.0), [0.3, 0.4, 0.5], &mut facts);
        assert_eq!(conversions().load(std::sync::atomic::Ordering::Relaxed), 0);
    }

    #[test]
    fn a_luminance_component_reads_the_base_pixel() {
        let c = Composite {
            base: CoverageSource::Range(LuminanceRange::default()),
            refine: Vec::new(),
        };
        let mut facts = BaseFacts::default();
        let dark = c.at(&PixelCtx::analytic_only(0.5, 0.5, 1.0), [0.01; 3], &mut facts);
        let mut facts = BaseFacts::default();
        let bright = c.at(&PixelCtx::analytic_only(0.5, 0.5, 1.0), [0.9; 3], &mut facts);
        assert!(dark > bright, "the default range is the darker half: {dark} vs {bright}");
    }
}
```

- [ ] **Step 2: Implement `BaseFacts`, `PixelCtx` and `Composite`**

Append to `crates/pipeline/src/stages/mask_coverage.rs` (above the test module):

```rust
/// Per-pixel facts a pixel-dependent component reads, computed **at most once per pixel** and
/// shared by every component that wants them.
///
/// ⚠⚠ Three colour-range components must cost **one** Oklab round trip, not three.
/// `docs/BACKLOG.md`'s decomposition puts that round trip at 42% of a range mask's coverage cost,
/// so a per-component conversion would make a composite's cost linear in a component count that
/// ought to be nearly free — and it would be **invisible**, because the pixels come out identical.
#[derive(Default)]
pub(super) struct BaseFacts {
    lightness: Option<f32>,
    hue_chroma: Option<(f32, f32)>,
}

/// How many Oklab conversions the evaluator has performed. ⚠ A counter, not a timing: a benchmark
/// cannot fail a build, and the sharing above is otherwise unobservable.
#[cfg(test)]
pub(super) fn conversions() -> &'static std::sync::atomic::AtomicUsize {
    static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    &N
}

impl BaseFacts {
    fn lightness(&mut self, rgb: [f32; 3]) -> f32 {
        *self.lightness.get_or_insert_with(|| lightness(rgb))
    }

    fn hue_chroma(&mut self, rgb: [f32; 3]) -> (f32, f32) {
        *self.hue_chroma.get_or_insert_with(|| {
            #[cfg(test)]
            conversions().fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            hue_chroma(rgb)
        })
    }
}

/// Where one pixel sits, and the rasterized maps a painted component reads.
pub(super) struct PixelCtx<'a> {
    /// Position in the **frame's** normalised coordinates.
    pub(super) u: f32,
    pub(super) v: f32,
    pub(super) aspect: f32,
    /// Frame pixel coordinates, for indexing a coverage map.
    pub(super) fx: usize,
    pub(super) fy: usize,
    /// One entry per component, index 0 being the base. `None` where the component is not painted
    /// or its map could not be built.
    pub(super) maps: &'a [Option<std::sync::Arc<crate::brush::CoverageMap>>],
}

#[cfg(test)]
impl PixelCtx<'static> {
    /// A context for a composite with no painted component.
    fn analytic_only(u: f32, v: f32, aspect: f32) -> Self {
        Self { u, v, aspect, fx: 0, fy: 0, maps: &[] }
    }
}

/// A mask's coverage: a base plus the refinements folded onto it.
///
/// ⚠ Resolved **once at construction**, exactly as the single `CoverageSource` was, so `apply`
/// still matches outside the pixel loop.
pub(super) struct Composite {
    pub(super) base: CoverageSource,
    pub(super) refine: Vec<(edit_model::MaskOp, CoverageSource)>,
}

impl Composite {
    /// Coverage of one component, in `0.0..=1.0`.
    ///
    /// ⚠ `base_px` is the pixel **before** this mask's own adjustment, for every component. A mask
    /// keyed on its own output chases its effect — lifting the shadows moves them out of the band
    /// that selected them — and every component reading the same base is what stops there being a
    /// second rule to keep in step.
    fn source_at(
        src: &CoverageSource,
        j: usize,
        cx: &PixelCtx<'_>,
        base_px: [f32; 3],
        facts: &mut BaseFacts,
    ) -> f32 {
        match src {
            CoverageSource::Analytic(m) => m.analytic_coverage(cx.u, cx.v, cx.aspect).unwrap_or(0.0),
            CoverageSource::Range(r) => r.coverage(facts.lightness(base_px)),
            CoverageSource::Colour(r) => {
                let (hue, chroma) = facts.hue_chroma(base_px);
                r.coverage_at(hue, chroma)
            }
            // ⚠ Indexed by FRAME pixel, not by the region-relative one.
            CoverageSource::Painted { .. } => match cx.maps.get(j).and_then(|m| m.as_ref()) {
                Some(cov) => cov.at(cx.fx, cx.fy) as f32 / 255.0,
                None => 0.0,
            },
        }
    }

    /// The composite's coverage at one pixel: the base, then each refinement folded on in list
    /// order.
    pub(super) fn at(&self, cx: &PixelCtx<'_>, base_px: [f32; 3], facts: &mut BaseFacts) -> f32 {
        let mut a = Self::source_at(&self.base, 0, cx, base_px, facts).clamp(0.0, 1.0);
        for (j, (op, src)) in self.refine.iter().enumerate() {
            let b = Self::source_at(src, j + 1, cx, base_px, facts).clamp(0.0, 1.0);
            a = op.combine(a, b);
        }
        a.clamp(0.0, 1.0)
    }

    /// Whether the whole composite is provably inert given its base map.
    ///
    /// ⚠⚠ **An inert base does NOT make an inert composite.** `Subtract` and `Intersect` preserve
    /// zero (`0·(1−b)` and `0·b`), and `Add` does not (`0 + 1·b = b`). Short-circuiting on the base
    /// alone would make an added component vanish whenever the base happened to be an unpainted
    /// brush — silently, since the render would simply be the unmasked photograph.
    pub(super) fn base_inert_is_enough(&self) -> bool {
        !self
            .refine
            .iter()
            .any(|(op, _)| matches!(op, edit_model::MaskOp::Add))
    }
}
```

- [ ] **Step 3: Run the tests**

```bash
cargo test -p pipeline mask_coverage 2>&1 | tail -8
```

Expected: `test result: ok. 5 passed`.

- [ ] **Step 4: Sabotage — convert per component**

Change `hue_chroma` on `BaseFacts` to skip the memo (compute every call). Run:

```bash
cargo test -p pipeline three_colour_components 2>&1 | tail -5
```

Expected: **FAIL** with `left: 3, right: 1`. Restore from a copy.

- [ ] **Step 5: Sabotage — drop the `Add` clause**

Change `base_inert_is_enough` to `true`. Expect no test to fail **yet** — the guard has no consumer until Task 6, which is why Task 6 carries its own test. Note it and move on; ⚠ a sabotage that changes no behaviour is not evidence about a test, and saying so is better than counting it as a pass.

- [ ] **Step 6: Commit**

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings > /tmp/c.log 2>&1; echo "CLIPPY=$?"
git add crates/pipeline/src/stages/mask_coverage.rs
git commit -m "A composite's coverage, with the base pixel read once for all of it"
```

---

### Task 6: `MaskStage` renders a composite

**Files:**
- Modify: `crates/pipeline/src/stages/mask.rs`

- [ ] **Step 1: Write the failing tests**

Add to `crates/pipeline/src/stages/mask.rs`'s test module:

```rust
#[test]
fn a_subtracted_radial_is_cut_out_of_a_linear() {
    // A full-strength exposure lift under a linear gradient, with a radial subtracted from its
    // middle: the centre must be left alone while the gradient's own end is lifted.
    let mut masked = masked([0.0, 0.5], [1.0, 0.5]);
    masked.adjustments.exposure = 1.0;
    masked.refine.push(edit_model::Refinement {
        op: edit_model::MaskOp::Subtract,
        mask: edit_model::Mask::Radial(edit_model::RadialGradient {
            center: [0.9, 0.5],
            radius: [0.2, 0.5],
            feather: 0.0,
            invert: false,
        }),
    });
    let src = ImageBuf::from_fn(32, 8, |_, _| [0.25; 3]);
    let out = MaskStage::new(&masked, RenderScale::FULL, &Geometry::default()).apply(src.clone());
    // Inside the subtracted radial the pixel is untouched.
    assert!((out.pixel(30, 4)[0] - 0.25).abs() < 1e-4, "the cut-out must be untouched");
    // Outside it, where the gradient still covers, it is lifted.
    assert!(out.pixel(20, 4)[0] > 0.30, "the gradient outside the cut-out must still lift");
}

#[test]
fn an_added_component_survives_an_inert_base() {
    // ⚠⚠ **The `is_inert` hazard, as a test.** The base is an unpainted brush, which covers
    // nothing and whose map is inert; the refinement is an added gradient. Short-circuiting on
    // the base alone returns the image untouched and the added component vanishes — silently,
    // because the result is simply the unmasked photograph.
    let mut masked = MaskedAdjustment {
        mask: Mask::Brush(edit_model::Brush::default()),
        ..Default::default()
    };
    masked.adjustments.exposure = 1.0;
    masked.refine.push(edit_model::Refinement {
        op: edit_model::MaskOp::Add,
        mask: Mask::Linear(LinearGradient { from: [-1.0, 0.0], to: [-0.5, 0.0], invert: false }),
    });
    let src = ImageBuf::from_fn(16, 8, |_, _| [0.25; 3]);
    let out = MaskStage::new(&masked, RenderScale::FULL, &Geometry::default()).apply(src);
    assert!(out.pixel(8, 4)[0] > 0.30, "an added component must render over an inert base");
}

#[test]
fn a_subtracted_component_over_an_inert_base_still_short_circuits() {
    // The other half: with no `Add`, an inert base really is inert and the fast path is correct.
    let mut masked = MaskedAdjustment {
        mask: Mask::Brush(edit_model::Brush::default()),
        ..Default::default()
    };
    masked.adjustments.exposure = 1.0;
    masked.refine.push(edit_model::Refinement {
        op: edit_model::MaskOp::Subtract,
        mask: Mask::Linear(LinearGradient { from: [-1.0, 0.0], to: [-0.5, 0.0], invert: false }),
    });
    let src = ImageBuf::from_fn(16, 8, |_, _| [0.25; 3]);
    let out = MaskStage::new(&masked, RenderScale::FULL, &Geometry::default()).apply(src.clone());
    assert_eq!(out.pixel(8, 4), src.pixel(8, 4));
}
```

⚠ Adapt `masked(..)`, `ImageBuf::from_fn` and `RenderScale::FULL` to the helpers and constructors the existing test module already uses — read it first rather than assuming these names.

- [ ] **Step 2: Change the struct and the constructors**

In `MaskStage`, replace `source: CoverageSource` with:

```rust
    /// The mask's coverage: its base shape plus every refinement, resolved once at construction.
    composite: mask_coverage::Composite,
```

Replace `coverage_source` with a builder that resolves every component. `prebuilt` and `deposit` become per component — see Task 7 for the types; until then, pass the base's `prebuilt` at index 0 and `None` elsewhere so this task compiles and its tests pass:

```rust
    fn composite(
        masked: &MaskedAdjustment,
        geometry: &Geometry,
        prebuilt: &[Option<std::sync::Arc<crate::brush::CoverageMap>>],
    ) -> mask_coverage::Composite {
        let src = |m: &edit_model::Mask, j: usize| Self::source_for(m, geometry, prebuilt.get(j).cloned().flatten());
        mask_coverage::Composite {
            base: src(&masked.mask, 0),
            refine: masked
                .refine
                .iter()
                .enumerate()
                .map(|(j, r)| (r.op, src(&r.mask, j + 1)))
                .collect(),
        }
    }
```

`source_for` is the old `coverage_source` body, taking a `&Mask` rather than a `&MaskedAdjustment`. ⚠ **Keep it exhaustive** — the catch-all it used to have is the recorded trap where a new variant silently produces a mask covering nothing.

- [ ] **Step 3: Rewrite `apply`'s coverage build and pixel loop**

Replace the single `coverage` binding with one map per component, built by the same rules:

```rust
        // ⚠ One entry per component, index 0 being the base. An analytic or range component
        // rasterizes nothing and answers from inside the loop; only a painted one has a map.
        let mut maps: Vec<Option<std::sync::Arc<crate::brush::CoverageMap>>> =
            Vec::with_capacity(1 + self.composite.refine.len());
        for (j, src) in std::iter::once(&self.composite.base)
            .chain(self.composite.refine.iter().map(|(_, s)| s))
            .enumerate()
        {
            maps.push(self.build_map(src, j, &image, fw, fh, w, h, x0, y0));
        }
```

`build_map` is the existing `CoverageSource::Painted` arm of the old `match`, verbatim — including the `BaseView`, the clip derivation and the `deposit` call — with the deposit index taking `j` as well as the mask index (Task 7).

The inert short-circuit becomes:

```rust
        // ⚠⚠ **An optimisation that stops being one once an `Add` refinement exists.** An inert
        // base stays inert under `Subtract` and `Intersect` and does NOT under `Add`, where the
        // added component is the whole mask. `an_added_component_survives_an_inert_base` is what
        // fails when this clause is dropped, and nothing else in the suite can see it.
        if self.composite.base_inert_is_enough()
            && maps.first().and_then(|m| m.as_ref()).is_some_and(|c| c.is_inert())
        {
            return image;
        }
```

The pixel loop's coverage becomes:

```rust
        image.map_pixels_xy(|x, y, base| {
            let cx = mask_coverage::PixelCtx {
                u: (x0 + x) as f32 / fw as f32,
                v: (y0 + y) as f32 / fh as f32,
                aspect,
                fx: x0 + x,
                fy: y0 + y,
                maps: &maps,
            };
            let mut facts = mask_coverage::BaseFacts::default();
            let c = self.composite.at(&cx, base, &mut facts);
            let adj = adjusted.pixel(x, y);
            [
                base[0] + (adj[0] - base[0]) * c,
                base[1] + (adj[1] - base[1]) * c,
                base[2] + (adj[2] - base[2]) * c,
            ]
        })
```

⚠ `BaseFacts` is constructed **per pixel** and dropped there. It is two `Option`s on the stack; it is not shared across pixels because different pixels have different values.

- [ ] **Step 4: Run the tests**

```bash
cargo test -p pipeline mask 2>&1 | tail -8
cargo test -p pipeline --test golden > /tmp/g.log 2>&1; grep -c "^test golden_.* ok$" /tmp/g.log
```

Expected: the mask tests pass including the three new ones; **60** goldens still pass and are byte-identical — an empty `refine` must take the identical path.

- [ ] **Step 5: Sabotage — drop the `Add` clause**

Change the short-circuit to `if maps.first()…is_inert()`. Run:

```bash
cargo test -p pipeline an_added_component_survives_an_inert_base 2>&1 | tail -5
```

Expected: **FAIL**. ⚠ Confirm the *named* test failed — not merely that some suite went red. Restore from a copy.

- [ ] **Step 6: Commit**

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings > /tmp/c.log 2>&1; echo "CLIPPY=$?"
git add crates/pipeline/src/stages/
git commit -m "MaskStage renders a composite, and an added component survives an inert base"
```

---

### Task 7: `BrushCoverage` gains a component index

**Files:**
- Modify: `crates/pipeline/src/lib.rs:8-70` (`BrushCoverage`)
- Modify: `crates/pipeline/src/lib.rs:788` (the masks loop), `:347` (the prefix step)
- Modify: `crates/pipeline/src/stages/mask.rs` (`deposit` field, constructors)

- [ ] **Step 1: Write the failing tests**

Replace the existing `BrushCoverage` tests in `crates/pipeline/src/lib.rs` (~line 1035) with component-indexed versions, keeping every assertion they already make:

```rust
    #[test]
    fn a_deposit_lands_under_its_own_component() {
        let c = BrushCoverage::with_slots(&[1, 3]);
        c.deposit(1, 2, a_map());
        assert!(c.take_built(1, 2).is_some());
        assert!(c.take_built(1, 1).is_none(), "a sibling component must not see it");
        assert!(c.take_built(0, 0).is_none(), "another mask must not see it");
    }

    #[test]
    fn taking_a_deposit_clears_the_slot() {
        let c = BrushCoverage::with_slots(&[2]);
        c.deposit(0, 1, a_map());
        assert!(c.take_built(0, 1).is_some());
        assert!(c.take_built(0, 1).is_none(), "a second harvest must find nothing");
    }

    #[test]
    fn an_out_of_range_deposit_is_ignored_rather_than_panicking() {
        let c = BrushCoverage::with_slots(&[1]);
        c.deposit(7, 0, a_map());
        c.deposit(0, 9, a_map());
        assert!(c.take_built(0, 0).is_none());
    }
```

- [ ] **Step 2: Change the type**

```rust
#[derive(Default)]
pub struct BrushCoverage {
    /// Prebuilt maps, indexed `[mask][component]`, component 0 being the mask's base shape.
    ///
    /// ⚠ Nested because a composite can hold a brush in any component. Flattening with a prefix
    /// sum was the alternative and was rejected: the sum would have to be recomputed identically
    /// by the stage and by the cache, and two derivations of one index is how a sparse-versus-
    /// dense bug enters — this codebase has had four.
    pub maps: Vec<Vec<Option<std::sync::Arc<brush::CoverageMap>>>>,
    pub built: Vec<Vec<std::sync::Mutex<Option<std::sync::Arc<brush::CoverageMap>>>>>,
}

impl BrushCoverage {
    pub fn get(&self, i: usize, j: usize) -> Option<std::sync::Arc<brush::CoverageMap>> {
        self.maps.get(i)?.get(j).and_then(|m| m.clone())
    }

    /// A coverage set with one empty deposit slot per component. `counts[i]` is mask `i`'s
    /// component count — `1 + refine.len()`.
    pub fn with_slots(counts: &[usize]) -> Self {
        Self {
            maps: counts.iter().map(|&n| vec![None; n]).collect(),
            built: counts
                .iter()
                .map(|&n| (0..n).map(|_| std::sync::Mutex::new(None)).collect())
                .collect(),
        }
    }

    pub fn deposit(&self, i: usize, j: usize, map: std::sync::Arc<brush::CoverageMap>) {
        if let Some(slot) = self.built.get(i).and_then(|m| m.get(j)) {
            if let Ok(mut g) = slot.lock() {
                *g = Some(map);
            }
        }
    }

    pub fn take_built(&self, i: usize, j: usize) -> Option<std::sync::Arc<brush::CoverageMap>> {
        self.built.get(i)?.get(j).and_then(|s| s.lock().ok()?.take())
    }
}
```

⚠ Keep every existing doc comment on these items — the `Mutex`-not-`RefCell` note and the ignore-out-of-range note both record findings.

- [ ] **Step 3: Update `MaskStage`'s deposit and the masks loop**

`MaskStage`'s field becomes `deposit: Option<(std::sync::Arc<crate::BrushCoverage>, usize)>` still holding the **mask** index; `build_map` passes `(i, j)` when depositing.

`new_in_region`'s `prebuilt` parameter becomes a slice:

```rust
        prebuilt: &[Option<std::sync::Arc<crate::brush::CoverageMap>>],
```

At the masks loop (`crates/pipeline/src/lib.rs:788`), replace `brush.and_then(|b| b.get(i))` with the mask's whole component row:

```rust
                // ⚠ `i` is the position in `edits.masks`, never a counter over *active* masks —
                // an inactive mask is skipped here but still owns a row in `BrushCoverage`.
                &brush.map_or_else(Vec::new, |b| b.maps.get(i).cloned().unwrap_or_default()),
```

At the prefix step (`:347`) and `MaskStage::new` (`:118`), pass `&[]` — both already pass `None` today, and the reasons written there are unchanged.

- [ ] **Step 4: Run the tests**

```bash
cargo test -p pipeline 2>&1 | tail -8
```

Expected: all pass.

- [ ] **Step 5: Commit**

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings > /tmp/c.log 2>&1; echo "CLIPPY=$?"
git add crates/pipeline/src/
git commit -m "Brush coverage is indexed per component, not per mask"
```

---

### Task 8: `BrushCache` keys on `(mask, component)`

**Files:**
- Modify: `crates/app/src/render_engine.rs:760-935` (`CoverageSpec`, `MaskCoverageKey`, `BrushCache`)
- Modify: `crates/app/src/render_engine.rs:1143-1147` (the harvest loop)

- [ ] **Step 1: Write the failing tests**

Add to `crates/app/src/render_engine.rs`'s test module:

```rust
    #[test]
    fn two_components_of_one_mask_do_not_evict_each_other() {
        // ⚠ The cap is per *component*, not per mask: a composite of two brushes must keep both
        // maps at both live rungs, or every zoom change would re-rasterize the pair.
        let mut cache = BrushCache::default();
        let edits = composite_of_two_brushes();
        let cov = cache.get_or_build(&edits, spec());
        assert!(cov.get(0, 0).is_some(), "the base's map must be built");
        assert!(cov.get(0, 1).is_some(), "the refinement's map must be built");
        let again = cache.get_or_build(&edits, spec());
        assert!(same_map_at(&cov, &again, 0, 0), "the base must hit");
        assert!(same_map_at(&cov, &again, 0, 1), "the refinement must hit");
    }

    #[test]
    fn changing_one_component_misses_only_that_component() {
        let mut cache = BrushCache::default();
        let edits = composite_of_two_brushes();
        let a = cache.get_or_build(&edits, spec());
        let mut moved = edits.clone();
        if let Some(edit_model::Mask::Brush(b)) = moved.masks[0].refine.get_mut(0).map(|r| &mut r.mask) {
            b.strokes[0].points.push([0.7, 0.7]);
        }
        let b = cache.get_or_build(&moved, spec());
        assert!(same_map_at(&a, &b, 0, 0), "the untouched base must still hit");
        assert!(!same_map_at(&a, &b, 0, 1), "the changed refinement must miss");
    }
```

⚠ `composite_of_two_brushes()`, `spec()` and `same_map_at` are new local helpers; model them on the existing `same_map` (line ~2501) and on whatever the neighbouring cache tests already use to build an `Edits` and a `CoverageSpec`. **Read those tests first** — this project has recorded five cache tests going vacuous because they compared the outer `Arc` rather than the map.

- [ ] **Step 2: Widen the key and the cap**

```rust
struct MaskCoverageKey {
    /// This component's brush. `None` for an analytic shape, which is never cached here.
    brush: Option<edit_model::Brush>,
    /// Which component of which mask. ⚠ Both halves: a composite's base and its refinements are
    /// different coverage under one mask index.
    at: (usize, usize),
    geometry: edit_model::Geometry,
    fw: usize,
    fh: usize,
    region: Option<(usize, usize, usize, usize)>,
    base: Option<(Edits, usize)>,
}
```

`BrushCache::key(edits, i, j, spec)` reads the component's mask:

```rust
    /// The mask at `(i, j)` — component 0 is the base, `j > 0` the `j-1`th refinement.
    fn mask_at(edits: &Edits, i: usize, j: usize) -> Option<&edit_model::Mask> {
        let m = edits.masks.get(i)?;
        match j.checked_sub(1) {
            None => Some(&m.mask),
            Some(k) => m.refine.get(k).map(|r| &r.mask),
        }
    }
```

⚠⚠ **`base_id` is unchanged and must stay that way.** It truncates `edits.masks` to `i`, and every component of mask `i` reads the same base — components change the composite's *coverage*, never its *pixels*. A component index in the base identity would be wrong, not merely redundant.

The cap counts components:

```rust
    /// Two entries per **component**, for [`PrefixCache::FINALS_CAP`]'s reason: two rungs are live
    /// at once, so one entry per component would be evicted on every zoom change and rebuilt
    /// coming back.
    ///
    /// ⚠ Per component rather than per mask — a composite of three brushes needs three rows kept,
    /// and counting masks would make its own siblings evict each other.
    fn cap(edits: &Edits) -> usize {
        let n: usize = edits.masks.iter().map(|m| 1 + m.refine.len()).sum();
        2 * n.max(1)
    }
```

`get_or_build` builds a nested `Vec`, looping `j` over `0..=m.refine.len()` inside the existing `i` loop, with the same *"a gating brush is not built here, and must not be"* rule per component.

- [ ] **Step 3: Update the harvest loop**

At `crates/app/src/render_engine.rs:1143`:

```rust
    for (i, m) in edits.masks.iter().enumerate() {
        for j in 0..=m.refine.len() {
            if let Some(map) = coverage.take_built(i, j) {
                brush_cache.remember(edits, cov_spec, i, j, map);
            }
        }
    }
```

⚠ Keep the block's existing comment about `take_built` clearing each slot — it records why a missed path loses the win silently rather than caching something wrong, which is the safe direction.

- [ ] **Step 4: Run the tests**

```bash
cargo test -p app --bin Rustroom brush_cache 2>&1 | tail -8
cargo test -p app 2>&1 | tail -5
```

⚠ `render_engine` is a **lib** module. Run it without `--bin` too, or the filter matches nothing and prints `0 passed … ok`:

```bash
cargo test -p app render_engine 2>&1 | grep "test result"
```

- [ ] **Step 5: Sabotage — drop the component from the key**

Remove `at` from `MaskCoverageKey`. Run `changing_one_component_misses_only_that_component`.

Expected: **FAIL** — the two components collide on one entry. Restore from a copy.

- [ ] **Step 6: Commit**

```bash
cargo fmt
cargo clippy --all-targets --all-features -- -D warnings > /tmp/c.log 2>&1; echo "CLIPPY=$?"
git add crates/app/src/render_engine.rs
git commit -m "The coverage cache keys on the component, and the base identity does not"
```

---

### Task 9: Three goldens

**Files:**
- Modify: `crates/pipeline/tests/golden.rs`
- Create: `crates/pipeline/tests/golden/mask_subtract.png`, `mask_intersect.png`, `mask_add.png`

- [ ] **Step 1: Add the three cases**

Model them on the existing `mask_*` golden tests — read one first for the harness's exact shape. Each uses the same base linear gradient carrying a **measured** exposure lift, and differs only in the refinement's op:

```rust
fn composite_case(op: edit_model::MaskOp) -> Edits {
    Edits {
        masks: vec![edit_model::MaskedAdjustment {
            mask: edit_model::Mask::Linear(edit_model::LinearGradient {
                from: [0.0, 0.0],
                to: [1.0, 0.0],
                invert: false,
            }),
            refine: vec![edit_model::Refinement {
                op,
                mask: edit_model::Mask::Radial(edit_model::RadialGradient {
                    center: [0.5, 0.5],
                    radius: [0.3, 0.5],
                    feather: 40.0,
                    invert: false,
                }),
            }],
            adjustments: edit_model::LocalAdjustments {
                // ⚠ +0.55 EV, not +1.0 — the value M3.1b's own golden settled on after its
                // exposure reference was found to clip six of twenty samples, where it could no
                // longer tell peak coverage from any larger value.
                exposure: 0.55,
                ..Default::default()
            },
        }],
        ..Default::default()
    }
}

#[test]
fn golden_mask_subtract() {
    check_golden("mask_subtract", composite_case(edit_model::MaskOp::Subtract));
}
// … the same for mask_intersect and mask_add
```

- [ ] **Step 2: Generate and MEASURE them**

```bash
UPDATE_GOLDEN=1 cargo test -p pipeline --test golden golden_mask_ 2>&1 | tail -5
```

⚠⚠ **A golden nobody has measured is just a hash.** Write a throwaway check (or extend the existing `the_grading_goldens_do_not_clip` pattern) asserting for each new reference: **no sample pinned at 255**, at least 100 distinct values, and each of the three **differs from the other two** and from `neutral.png`. Three plausible PNGs prove nothing — a composite that silently ignored its refinements would produce three identical files that pass forever.

Record the measured numbers in the plan's completion note.

- [ ] **Step 3: Verify the other 60 did not move**

```bash
git status --short crates/pipeline/tests/golden/ | grep -v "^??" | wc -l
```

Expected: `0` — only the three new files are added, none modified.

- [ ] **Step 4: Commit**

```bash
git add crates/pipeline/tests/golden.rs crates/pipeline/tests/golden/mask_*.png
git commit -m "Three composite goldens, measured rather than eyeballed"
```

---

### Task 10: Measure the per-component cost, and run the full gate

**Files:**
- Modify: `crates/app/examples/bench_masked.rs`

⚠ **The measurement is an acceptance item, not a follow-up.** This area has five recorded cost estimates overturned by decomposition. The spec's §10 prediction is written down so it can be **checked against**, not believed.

- [ ] **Step 1: Add composite rows**

Add rows to `bench_masked`, following the file's existing row-building idiom: `base + 1 analytic`, `base + 1 luminance`, `base + 3 colour`, `base + 1 brush` — each at fit and at a native region, against the existing single-mask baseline.

- [ ] **Step 2: Run it**

```bash
cargo run --release -p app --example bench_masked ~/Desktop/fuji/DSCF6223.RAF
```

⚠ Substitute a real `.RAF` from the user's folder. Record the table in the plan's completion note and in `CLAUDE.md`.

The prediction to check against: an analytic component is a few flops; a luminance component is free once the sharing is in; **three colour components cost one Oklab round trip, not three**; a brush component costs one more rasterisation and one more cache entry.

- [ ] **Step 3: Run the full gate, each exit code from its own file**

```bash
cargo fmt --check > /tmp/f.log 2>&1; echo "FMT=$?"
cargo clippy --all-targets --all-features -- -D warnings > /tmp/c.log 2>&1; echo "CLIPPY=$?"
cargo test --workspace > /tmp/t.log 2>&1; echo "TEST=$?"
grep -c "^test golden_.* ok$" /tmp/t.log
grep "^test result" /tmp/t.log | awk '{s+=$4} END {print "passed:", s}'
```

Expected: `FMT=0`, `CLIPPY=0`, `TEST=0`, **63** goldens **run**, and a passed count that has grown by exactly the tests this plan added. ⚠ Assert the goldens **ran** — a filtered run that skips them prints `ok` too.

- [ ] **Step 4: Update `CLAUDE.md`**

Add a section for this milestone under Current status, recording: the algebra and why it is the brush's own rules; the `is_inert` hazard; `capture_projection`; the `(i, j)` cache index with the base identity unchanged; the measured table; and the accepted caveats from the spec's §11. ⚠ Do **not** move the RESUME block's NEXT ACTION yet — plan 2 is still outstanding.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "Measure a composite's per-component cost, and close the headless half"
```

---

## Done when

- `fmt` / `clippy --all-targets --all-features -D warnings` / `test --workspace` green **together**, each exit code read from its own file.
- **63 goldens run**, the 60 that predate this byte-identical.
- A composite renders correctly through `rustroom render --edits` on a hand-written sidecar carrying `refine` — the end-to-end check that needs no GUI, and the one M3.1a's headless plan used to prove its architecture.
- The measured table is recorded.
- ⚠ **Nothing is authorable yet.** A composite can only be built by hand-editing a sidecar, which is the deliberate cost of the split and the same one M3.1a and the brush's headless plan each paid. Plan 2 adds the panel and the canvas.
