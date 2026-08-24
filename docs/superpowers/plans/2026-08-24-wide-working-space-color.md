# Wide working space — the `color` crate

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give `color` the ProPhoto↔sRGB and ProPhoto↔Oklab conversions, plus a single `lightness()`, so the pipeline switch has proven arithmetic to stand on.

**Architecture:** Pure matrix work in one crate with no dependencies. Every matrix is a `const` composed offline and checked in the tests against a published reference or an independent path. **No behaviour changes anywhere — this plan moves no golden and touches no stage.**

**Tech Stack:** Rust, no new dependencies.

**Spec:** `docs/superpowers/specs/2026-08-24-wide-working-space-design.md`. This is **plan 1 of 2**; plan 2 is the pipeline switch.

---

## ⚠⚠ Read first: the matrices, and why they will not match Ottosson exactly

Every matrix below was computed from primaries and white points, then cross-checked. **One result
matters before you start.**

Deriving sRGB→LMS the same way this plan derives ProPhoto→LMS gives:

```
computed:      0.4121764592  0.5362739743  0.0514403723
oklab.rs has:  0.4122214708  0.5363325363  0.0514459929
```

They agree to about **4.5 × 10⁻⁵**, not exactly. That is **not an error** — Ottosson rounded his
sRGB primaries slightly differently when publishing the fused matrix. Two consequences:

- **Do not "fix" `linear_srgb_to_oklab` to match a freshly derived matrix.** Its constants are
  transcribed from the reference and the file says so; changing them would move goldens for nothing.
- **The cross-check test in Task 3 must use a tolerance near 1e-3, not 1e-6.** Anyone who writes
  `assert!((a - b).abs() < 1e-6)` there will see a failure and conclude the ProPhoto matrices are
  wrong. They are not.

---

## File Structure

| file | responsibility |
|---|---|
| `crates/color/src/spaces.rs` | **Create.** The RGB working spaces: ProPhoto↔sRGB matrices and the `apply` helper. |
| `crates/color/src/oklab.rs` | Gains `prophoto_to_oklab` / `oklab_to_prophoto` beside the existing sRGB pair. |
| `crates/color/src/lib.rs` | `pub mod spaces;`, re-exports, and `lightness()`. |

⚠ A new file rather than growing `lib.rs`: `lib.rs` currently holds only the sRGB transfer functions
and is the crate's front door. Matrices are a separate concern with their own tests.

---

### Task 1: The ProPhoto ↔ sRGB matrices

**Files:**
- Create: `crates/color/src/spaces.rs`
- Modify: `crates/color/src/lib.rs`

- [ ] **Step 1: Write the module**

```rust
//! Linear RGB working spaces and the transforms between them.
//!
//! ⚠ Everything here is **linear light**. The sRGB *transfer* functions live in `lib.rs`; these
//! matrices only change primaries, never encoding. Applying one to gamma-encoded values is
//! meaningless and will not fail loudly.
//!
//! ⚠ The matrices are composed offline from primaries, white points and a Bradford adaptation,
//! then checked in the tests against an independent path. They are `const` rather than computed
//! at startup because they never change and a per-pixel path should not pay for a lazy init.
#![allow(clippy::excessive_precision)]

/// Linear ProPhoto (ROMM, D50) → linear sRGB (Rec.709, D65), Bradford-adapted.
///
/// ⚠ Produces **negative** components for colours outside sRGB's gamut. That is correct and must
/// not be clamped here: the output transform decides what to do about gamut, and clamping in the
/// matrix would silently discard the very data this working space exists to keep.
pub const PROPHOTO_TO_SRGB: [[f32; 3]; 3] = [
    [2.0342891610, -0.7275078194, -0.3067813416],
    [-0.2288019903, 1.2317255449, -0.0029235546],
    [-0.0085622791, -0.1532652876, 1.1618275667],
];

/// Linear sRGB → linear ProPhoto. The inverse of [`PROPHOTO_TO_SRGB`].
pub const SRGB_TO_PROPHOTO: [[f32; 3]; 3] = [
    [0.5292929985, 0.3301161484, 0.1405908531],
    [0.0983600854, 0.8734699203, 0.0281699943],
    [0.0168761198, 0.1176587379, 0.8654651424],
];

/// Apply a 3×3 matrix to a pixel.
#[inline]
pub fn apply(m: &[[f32; 3]; 3], c: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * c[0] + m[0][1] * c[1] + m[0][2] * c[2],
        m[1][0] * c[0] + m[1][1] * c[1] + m[1][2] * c[2],
        m[2][0] * c[0] + m[2][1] * c[1] + m[2][2] * c[2],
    ]
}

/// Linear ProPhoto → linear sRGB.
#[inline]
pub fn prophoto_to_srgb(c: [f32; 3]) -> [f32; 3] {
    apply(&PROPHOTO_TO_SRGB, c)
}

/// Linear sRGB → linear ProPhoto.
#[inline]
pub fn srgb_to_prophoto(c: [f32; 3]) -> [f32; 3] {
    apply(&SRGB_TO_PROPHOTO, c)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: [f32; 3], b: [f32; 3], tol: f32) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < tol)
    }

    #[test]
    fn the_two_matrices_are_inverses() {
        // ⚠ Round-tripping several colours, not one: a matrix pair can be inverse on the
        // diagonal and wrong off it, and a grey probe would never notice — grey is the one
        // colour every sane matrix preserves.
        for c in [
            [0.5, 0.5, 0.5],
            [1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.9, 0.3, 0.1],
            [0.02, 0.7, 0.45],
        ] {
            let back = prophoto_to_srgb(srgb_to_prophoto(c));
            assert!(close(back, c, 1e-4), "{c:?} round-tripped to {back:?}");
        }
    }

    #[test]
    fn white_maps_to_white() {
        // ⚠ The check that the Bradford adaptation is present and the right way round. Without
        // it, D50 white lands off-white in sRGB — a warm cast on *every* image, which is subtle
        // enough to ship and awful enough to matter.
        let w = prophoto_to_srgb([1.0, 1.0, 1.0]);
        assert!(close(w, [1.0, 1.0, 1.0], 1e-3), "ProPhoto white became {w:?}");
        let w = srgb_to_prophoto([1.0, 1.0, 1.0]);
        assert!(close(w, [1.0, 1.0, 1.0], 1e-3), "sRGB white became {w:?}");
    }

    #[test]
    fn prophoto_holds_colours_srgb_cannot() {
        // ⚠ The point of the whole working space, as a test. A saturated sRGB green is *inside*
        // ProPhoto, so it converts to components all within [0, 1]; the reverse — ProPhoto's own
        // primary green — falls outside sRGB and must come back with a negative component.
        let inside = srgb_to_prophoto([0.0, 1.0, 0.0]);
        assert!(
            inside.iter().all(|v| (-0.001..=1.001).contains(v)),
            "sRGB green should fit inside ProPhoto, got {inside:?}"
        );
        let outside = prophoto_to_srgb([0.0, 1.0, 0.0]);
        assert!(
            outside.iter().any(|v| *v < -0.01),
            "ProPhoto green should be outside sRGB, got {outside:?}"
        );
    }

    #[test]
    fn the_matrix_does_not_clamp() {
        // ⚠ A guard, not a discovery. Someone "tidying" the conversion by clamping to [0, 1]
        // would destroy exactly the out-of-gamut data this space exists to carry, and every
        // other test here would still pass.
        let v = prophoto_to_srgb([0.0, 1.0, 0.0]);
        assert!(v.iter().any(|x| *x < 0.0), "negatives were clamped away");
        let v = prophoto_to_srgb([2.0, 2.0, 2.0]);
        assert!(v[0] > 1.5, "values above 1.0 were clamped away: {v:?}");
    }
}
```

Add to `crates/color/src/lib.rs`, after `pub mod oklab;`:

```rust
pub mod spaces;
pub use spaces::{prophoto_to_srgb, srgb_to_prophoto};
```

- [ ] **Step 2: Run the tests**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo test -p color 2>&1 | tail -12
```

Expected: 4 new PASS alongside the existing `color` tests.

- [ ] **Step 3: Sabotage — drop the Bradford adaptation**

Replace `PROPHOTO_TO_SRGB` with the un-adapted composition (ROMM→XYZ then XYZ→sRGB with no white
adaptation):

```rust
pub const PROPHOTO_TO_SRGB: [[f32; 3]; 3] = [
    [2.1423915954, -0.6562735893, -0.3099668501],
    [-0.2327849884, 1.2044236892, 0.0040632076],
    [-0.0143787121, -0.1376857272, 0.8739271811],
];
```

```bash
cargo test -p color --no-run 2>&1 | grep -cE "^error" || echo "0 compile errors"
cargo test -p color white_maps_to_white 2>&1 | tail -8
```

Expected: **zero compile errors first**, then FAIL — white maps to `[1.176, 0.976, 0.722]`, a
strong warm cast on every neutral in every image.

⚠ **Both `white_maps_to_white` and `the_two_matrices_are_inverses` fail here**, because replacing
only `PROPHOTO_TO_SRGB` leaves the pair non-inverse. That is fine — it proves the tests are live —
but it does not isolate what `white_maps_to_white` is for.

⚠ **To see why that test has to exist separately, replace *both* matrices** with their un-adapted
forms. The round trip then passes — an un-adapted pair is perfectly self-consistent — and only
`white_maps_to_white` fails. That is the case the test was written for: a colour pipeline that is
internally consistent and uniformly wrong.

Restore, then `touch crates/color/src/spaces.rs`.

- [ ] **Step 4: Commit**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -5
cargo test --all 2>&1 | grep -oE "^test result: ok\. [0-9]+" | awk '{s+=$4} END {print "TOTAL:", s}'
git add crates/color/src/spaces.rs crates/color/src/lib.rs
git commit -m "feat(color): linear ProPhoto and sRGB conversions"
```

Expected: clean; **706** (702 + 4).

---

### Task 2: ProPhoto ↔ Oklab

**Files:**
- Modify: `crates/color/src/oklab.rs`

- [ ] **Step 1: Add the pair**

Append to `crates/color/src/oklab.rs`, above its `#[cfg(test)]`:

```rust
/// Linear ProPhoto (ROMM, D50) → Oklab `[L, a, b]`.
///
/// ⚠ Composed offline as ProPhoto→XYZ(D50) → Bradford → XYZ(D65) → Ottosson's LMS, then fused
/// into one matrix — the same shape as [`linear_srgb_to_oklab`], which fuses sRGB→LMS.
///
/// ⚠ **Deriving sRGB→LMS by this same route does not reproduce Ottosson's published constants
/// exactly** — they agree to about 4.5e-5, because he rounded his primaries differently. Do not
/// "correct" either function to match the other; see `an_srgb_colour_reaches_the_same_oklab`.
pub fn prophoto_to_oklab(rgb: [f32; 3]) -> [f32; 3] {
    let [r, g, b] = rgb;
    let l = 0.7153451038 * r + 0.3527967327 * g - 0.0682510308 * b;
    let m = 0.2744153967 * r + 0.6678313229 * g + 0.0577801949 * b;
    let s = 0.1098335093 * r + 0.1862950046 * g + 0.7043511321 * b;

    // ⚠ `cbrt`, not `powf(1/3)`: it is sign-preserving, so scene-referred negatives (which a
    // wide working space produces routinely) stay finite instead of becoming NaN.
    let (l_, m_, s_) = (l.cbrt(), m.cbrt(), s.cbrt());

    [
        0.2104542553 * l_ + 0.7936177850 * m_ - 0.0040720468 * s_,
        1.9779984951 * l_ - 2.4285922050 * m_ + 0.4505937099 * s_,
        0.0259040371 * l_ + 0.7827717662 * m_ - 0.8086757660 * s_,
    ]
}

/// Oklab `[L, a, b]` → linear ProPhoto. May return out-of-gamut values; callers clamp.
pub fn oklab_to_prophoto(lab: [f32; 3]) -> [f32; 3] {
    let [l, a, b] = lab;
    let l_ = l + 0.3963377774 * a + 0.2158037573 * b;
    let m_ = l - 0.1055613458 * a - 0.0638541728 * b;
    let s_ = l - 0.0894841775 * a - 1.2914855480 * b;

    let (l3, m3, s3) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);

    [
        1.7386588588 * l3 - 0.9880931444 * m3 + 0.2495310446 * s3,
        -0.7071491793 * l3 + 1.9343303974 * m3 - 0.2272015198 * s3,
        -0.0840839768 * l3 - 0.3575352425 * m3 + 1.4409285251 * s3,
    ]
}
```

- [ ] **Step 2: Write the tests**

Add to `oklab.rs`'s `tests` module:

```rust
    #[test]
    fn prophoto_oklab_round_trips() {
        for c in [
            [0.5, 0.5, 0.5],
            [1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.8, 0.4, 0.2],
            [2.5, 1.0, 0.1],
        ] {
            let back = oklab_to_prophoto(prophoto_to_oklab(c));
            for i in 0..3 {
                assert!(
                    (back[i] - c[i]).abs() < 1e-4,
                    "{c:?} round-tripped to {back:?}"
                );
            }
        }
    }

    #[test]
    fn an_srgb_colour_reaches_the_same_oklab() {
        // ⚠⚠ The load-bearing test: it checks the *entire* matrix chain against a completely
        // independent path. Converting a colour sRGB→ProPhoto→Oklab must agree with taking it
        // sRGB→Oklab directly, because Oklab is absolute.
        //
        // ⚠ Tolerance 2e-3, not 1e-6, and this is not slack: Ottosson's published sRGB→LMS
        // constants differ from a fresh derivation by ~4.5e-5, so the two paths cannot agree
        // exactly. Tightening this will produce a failure that looks like a broken matrix and
        // is not one.
        for c in [
            [0.5, 0.5, 0.5],
            [0.9, 0.2, 0.1],
            [0.1, 0.6, 0.9],
            [0.3, 0.8, 0.2],
        ] {
            let direct = linear_srgb_to_oklab(c);
            let via = prophoto_to_oklab(crate::spaces::srgb_to_prophoto(c));
            for i in 0..3 {
                assert!(
                    (direct[i] - via[i]).abs() < 2e-3,
                    "{c:?}: direct {direct:?} vs via ProPhoto {via:?}"
                );
            }
        }
    }

    #[test]
    fn grey_has_no_chroma_in_prophoto() {
        // A neutral must land on the L axis. If the matrix rows do not sum to the white point,
        // greys pick up a colour cast that is invisible in a round trip but visible in an image.
        for v in [0.05_f32, 0.18, 0.5, 1.0] {
            let lab = prophoto_to_oklab([v, v, v]);
            assert!(
                lab[1].abs() < 1e-3 && lab[2].abs() < 1e-3,
                "grey {v} has chroma: {lab:?}"
            );
        }
    }
```

- [ ] **Step 3: Run them**

```bash
cargo test -p color oklab 2>&1 | tail -12
```

Expected: 3 new PASS.

- [ ] **Step 4: Sabotage — transpose the LMS matrix**

Swap the first two rows of `prophoto_to_oklab`'s l/m computation:

```rust
    let l = 0.2744153967 * r + 0.6678313229 * g + 0.0577801949 * b;
    let m = 0.7153451038 * r + 0.3527967327 * g - 0.0682510308 * b;
```

```bash
cargo test -p color --no-run 2>&1 | grep -cE "^error" || echo "0 compile errors"
cargo test -p color an_srgb_colour_reaches_the_same_oklab 2>&1 | tail -8
```

Expected: zero compile errors, then FAIL on **both** `an_srgb_colour_reaches_the_same_oklab` and
`prophoto_oklab_round_trips`.

⚠ **The round trip fails only because the swap is one-directional** — `oklab_to_prophoto` is left
untouched, so the pair is no longer inverse. That is a general trap when predicting a sabotage on a
matrix pair: *any* change to one direction breaks the round trip, so the round trip can never
isolate what the other tests are for.

⚠ **The claim that matters did hold: `grey_has_no_chroma_in_prophoto` passes.** A row swap preserves
neutrals, so greys alone would never catch it. Only the cross-check against the independent sRGB
path does — which is why that test is the one that earns its place.

Restore, then `touch crates/color/src/oklab.rs`.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -5
cargo test --all 2>&1 | grep -oE "^test result: ok\. [0-9]+" | awk '{s+=$4} END {print "TOTAL:", s}'
git add crates/color/src/oklab.rs
git commit -m "feat(color): ProPhoto to Oklab"
```

Expected: clean; **709**.

---

### Task 3: `lightness()`

**Files:**
- Modify: `crates/color/src/lib.rs`

- [ ] **Step 1: Add it**

```rust
/// Perceptual lightness of a linear **ProPhoto** pixel — Oklab's L.
///
/// ⚠⚠ **This exists because ProPhoto's own luminance coefficients are unusable.** The ROMM→XYZ
/// luminance row is `R 0.2880, G 0.7119, B 0.0001` — blue contributes essentially nothing,
/// because ProPhoto's blue primary sits at y = 0.0001 on the chromaticity diagram. A stage using
/// that as "luma" would stop seeing blue detail entirely: sharpening would leave a blue iris or a
/// sky gradient untouched, and dehaze estimates airlight from brightness while haze is blue.
///
/// Oklab L is defined from XYZ, so it is working-space independent and does not collapse any
/// channel. `every_channel_contributes_to_lightness` is the guard.
///
/// ⚠ **L is a cube root, not linear luminance.** Constants tuned against linear luma do not
/// carry over — see [`MID_GREY_L`] and the spec's "five tuned constants" section.
pub fn lightness(rgb: [f32; 3]) -> f32 {
    oklab::prophoto_to_oklab(rgb)[0]
}

/// The lightness of mid-grey — linear 0.18 — in [`lightness`]'s units.
///
/// ⚠ **0.5646, not 0.18.** Any stage weighting midtones must use this: `local_contrast`'s
/// midtone weight is `t = l / (l + 0.18)`, and left at 0.18 against Oklab L it peaks a shade
/// above black, applying Clarity's midtone protection to the *shadows* instead. That is close to
/// the inverse of its purpose and produces a plausible-looking image, which is what makes it the
/// dangerous one.
pub const MID_GREY_L: f32 = 0.5646;
```

- [ ] **Step 2: Write the tests**

Add a `tests` module to `crates/color/src/lib.rs` (or extend the existing one):

```rust
    #[test]
    fn every_channel_contributes_to_lightness() {
        // ⚠⚠ The regression guard for the finding this whole design rests on. ProPhoto's own
        // luminance weights give blue 0.0001, so a "simplification" back to working-space luma
        // would make pure blue almost black — and nothing else in the suite would notice.
        let black = lightness([0.0, 0.0, 0.0]);
        for (name, c) in [
            ("red", [1.0, 0.0, 0.0]),
            ("green", [0.0, 1.0, 0.0]),
            ("blue", [0.0, 0.0, 1.0]),
        ] {
            let l = lightness(c);
            assert!(
                l > black + 0.2,
                "{name} has lightness {l}, barely above black {black} — \
                 this is what working-space luma does to blue"
            );
        }
    }

    #[test]
    fn mid_grey_l_matches_the_function() {
        // ⚠ The constant and the function must not drift. A hand-written constant that no test
        // ties to its source is one that silently goes stale the day the space changes.
        let measured = lightness([0.18, 0.18, 0.18]);
        assert!(
            (measured - MID_GREY_L).abs() < 1e-3,
            "MID_GREY_L is {MID_GREY_L} but lightness(0.18) is {measured}"
        );
    }

    #[test]
    fn lightness_is_monotonic_and_compresses_the_shadows() {
        // ⚠ Pins the *character* change, not just the values: L is a cube root, so it rises
        // steeply out of black. A stage guard tuned for linear luma — `LUMA_EPS = 1e-3` in
        // sharpening — fires far higher up the range here, which is why the spec re-derives it.
        let a = lightness([0.001, 0.001, 0.001]);
        let b = lightness([0.01, 0.01, 0.01]);
        let c = lightness([0.5, 0.5, 0.5]);
        assert!(a < b && b < c, "lightness must be monotonic");
        assert!(
            a > 0.05,
            "linear 0.001 should already be well off the floor in L, got {a}"
        );
    }
```

- [ ] **Step 3: Run them**

```bash
cargo test -p color 2>&1 | tail -14
```

Expected: 3 new PASS.

- [ ] **Step 4: Sabotage — use ProPhoto's own luminance weights**

```rust
pub fn lightness(rgb: [f32; 3]) -> f32 {
    0.2880 * rgb[0] + 0.7119 * rgb[1] + 0.0001 * rgb[2]
}
```

```bash
cargo test -p color --no-run 2>&1 | grep -cE "^error" || echo "0 compile errors"
cargo test -p color every_channel_contributes_to_lightness 2>&1 | tail -8
```

Expected: zero compile errors, then FAIL on **blue** with a lightness of ~0.0001. This is the
sabotage that reproduces the actual design hazard rather than a synthetic one. Restore, then
`touch crates/color/src/lib.rs`.

- [ ] **Step 5: Commit**

```bash
cargo fmt --all && cargo clippy --all-targets --all-features -- -D warnings 2>&1 | tail -5
cargo test --all 2>&1 | grep -oE "^test result: ok\. [0-9]+" | awk '{s+=$4} END {print "TOTAL:", s}'
git add crates/color/src/lib.rs
git commit -m "feat(color): perceptual lightness, because ProPhoto luma collapses blue"
```

Expected: clean; **712**.

---

### Task 4: Confirm nothing moved, and record it

- [ ] **Step 1: Full green, and the goldens specifically**

```bash
export PKG_CONFIG_PATH="/opt/homebrew/lib/pkgconfig:/opt/homebrew/opt/libraw/lib/pkgconfig:/opt/homebrew/opt/little-cms2/lib/pkgconfig:$PKG_CONFIG_PATH"
cargo fmt --all --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all 2>&1 | tail -5
git status --short crates/pipeline/tests/
```

Expected: green at **712**, and the `git status` **empty**.

⚠ **That empty status is the whole point of this plan being separate.** Nothing calls the new code
yet, so every golden must be byte-identical. If one moved, something here is wired into the pipeline
that should not be, and plan 2 has not started.

- [ ] **Step 2: Update CLAUDE.md**

Record: the ProPhoto luma finding with its numbers (`R 0.2880, G 0.7119, B 0.0001`) and why it forces
Oklab L; that `linear_srgb_to_oklab` keeps Ottosson's constants and a fresh derivation differs by
~4.5e-5, so the cross-check test needs 2e-3; `MID_GREY_L = 0.5646` and the `0.18` trap it exists to
prevent; and that this is **plan 1 of 2**, with the pipeline switch still to come and all 38 goldens
still on the old space.

- [ ] **Step 3: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: record the color crate's wide-space groundwork"
```

---

## Self-review notes

Checked against the spec's "What `color` gains" table:

- ProPhoto ↔ XYZ (D50), Bradford D50→D65, XYZ→Oklab — all three are **composed offline into the
  fused matrices** in Tasks 1 and 2 rather than shipped as separate steps. ⚠ That is a deliberate
  departure: the spec listed them as conceptual stages, but exposing three matrices the pipeline
  would only ever use in one combination is surface area for no gain, and a fused matrix is one
  multiply per pixel instead of three. The derivation is recorded in the doc comments and checked by
  `an_srgb_colour_reaches_the_same_oklab`, which validates the whole chain against an independent
  path — a stronger guarantee than testing each stage separately would give.
- `prophoto_to_oklab` / `oklab_to_prophoto` → Task 2. ✅
- `prophoto_to_srgb` (and its inverse, needed by plan 2's chart authoring) → Task 1. ✅
- `lightness()` → Task 3. ✅
- `linear_srgb_to_oklab` untouched → stated in Task 2's doc comment and by the fact no task edits it. ✅
- `MID_GREY_L` for the `0.18` trap → Task 3, tied to the function by a test. ✅

**Not in this plan, by design:** the eight stages, the decode flag, the output transform, the chart
authoring and the regenerated goldens. All of that is plan 2. This plan deliberately ends with every
golden byte-identical, which is the evidence that it changed no behaviour.
